// Sealed discovery tests (§6.5, F4): machines.list is routing only —
// desktop id, online, last seen, blob version, padded size. The live
// catalog travels inside the KK channel; the socket catalog stream carries
// the at-rest sealed blob. No names, no content.
package relay

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

// This fixture enters through the shipping desktop publish route, then reads
// through the actual authenticated mobile socket. The sealed store is shared
// between the catalog Handler and the RPC handler, like main.go wires it.
func machineSyncServer(t *testing.T) (*httptest.Server, *RPCHandler) {
	t.Helper()
	store := NewMemoryStore()
	sealed := NewMemorySealedStore()
	handler := NewHandler(store)
	handler.SetSealedStore(sealed)
	h := NewRPCHandler(nil, nil)
	h.SetSealedStore(sealed)
	h.ConfigureSocket(func(context.Context, string) (SocketIdentity, error) {
		return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Minute)}, nil
	}, store, nil)
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	handler.RegisterRoutes(r)
	h.RegisterSocketRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)
	return srv, h
}

func publishMachineSealed(t *testing.T, srv *httptest.Server, user, device string, version uint64) {
	t.Helper()
	if status := publishSealed(t, srv, user, device, makeSealedBlob(t, device, version)); status != http.StatusOK {
		t.Fatalf("desktop publish rejected: %d", status)
	}
}

func TestMachineDiscoveryIsRoutingOnly(t *testing.T) {
	srv, _ := machineSyncServer(t)
	publishMachineSealed(t, srv, "account-a", sealedTestDesktop, 3)
	publishMachineSealed(t, srv, "account-b", sealedTestDesktop2, 9)
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	socketWrite(t, ctx, c, socketRequest("machines.list", ""))
	discovery := socketRead(t, ctx, c)
	if discovery["ok"] != true {
		t.Fatal(discovery)
	}
	machines := discovery["result"].(map[string]any)["machines"].([]any)
	if len(machines) != 1 {
		t.Fatalf("discovery leaked/missed a machine: %v", machines)
	}
	row := machines[0].(map[string]any)
	if row["device_id"] != sealedTestDesktop {
		t.Fatalf("wrong machine: %v", row)
	}
	if row["online"] != true || row["blob_version"] != float64(3) || row["blob_size"] != float64(sealedBodyBucket) {
		t.Fatalf("routing summary wrong: %v", row)
	}
	for _, forbidden := range []string{"name", "platform", "version", "title", "path", "branch"} {
		if _, present := row[forbidden]; present {
			t.Fatalf("discovery carries content field %q: %v", forbidden, row)
		}
	}
	raw, _ := json.Marshal(machines)
	for _, needle := range []string{"My desktop", "Private", "dragent", "title"} {
		if strings.Contains(string(raw), needle) {
			t.Fatalf("discovery leaked %q: %s", needle, raw)
		}
	}
}

func TestSocketCatalogStreamCarriesSealedBlob(t *testing.T) {
	srv, _ := machineSyncServer(t)
	publishMachineSealed(t, srv, "account-a", sealedTestDesktop, 5)
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	socketWrite(t, ctx, c, map[string]any{"type": "rpc-subscribe", "v": 1, "id": uuid.NewString(), "stream": "catalog", "params": map[string]any{"device_id": sealedTestDesktop}})
	snapshot := socketRead(t, ctx, c)
	if snapshot["stream"] != "catalog" || snapshot["seq"] != float64(1) {
		t.Fatal(snapshot)
	}
	payload := snapshot["payload"].(map[string]any)
	blob := payload["catalog"].(map[string]any)
	if blob["desktop"] != sealedTestDesktop || blob["version"] != float64(5) {
		t.Fatalf("sealed blob wrong: %v", blob)
	}
	if _, present := blob["projects"]; present {
		t.Fatalf("socket catalog carries plaintext: %v", blob)
	}
	machine := payload["machine"].(map[string]any)
	if machine["device_id"] != sealedTestDesktop || machine["blob_version"] != float64(5) {
		t.Fatalf("machine summary wrong: %v", machine)
	}
	if _, present := machine["name"]; present {
		t.Fatalf("machine summary carries a name: %v", machine)
	}
	socketWrite(t, ctx, c, map[string]any{"type": "rpc-unsubscribe", "v": 1, "id": uuid.NewString(), "stream": "catalog"})
	if got := socketRead(t, ctx, c); got["ok"] != true {
		t.Fatal(got)
	}
}

func TestForeignCatalogSubscriptionIsRejectedBeforeSnapshot(t *testing.T) {
	srv, _ := machineSyncServer(t)
	publishMachineSealed(t, srv, "account-b", sealedTestDesktop2, 9)
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	id := uuid.NewString()
	socketWrite(t, ctx, c, map[string]any{"type": "rpc-subscribe", "v": 1, "id": id, "stream": "catalog", "params": map[string]any{"device_id": sealedTestDesktop2}})
	reply := socketRead(t, ctx, c)
	if reply["type"] != "rpc-reply" || reply["id"] != id || reply["ok"] != false {
		t.Fatalf("foreign catalog admitted: %v", reply)
	}
	raw, _ := json.Marshal(reply)
	if strings.Contains(string(raw), "Private") {
		t.Fatal("foreign metadata leaked")
	}
}

func TestPlaintextCatalogPublishIsGoneOverSocketFixture(t *testing.T) {
	srv, _ := machineSyncServer(t)
	// The old plaintext catalog shape is 410 on the same fixture server
	// the desktop publishes sealed blobs through.
	raw, _ := json.Marshal(map[string]any{
		"device_id": sealedTestDesktop,
		"tabs":      []any{map[string]any{"id": 7, "kind": "terminal", "title": "shell"}},
	})
	req, _ := http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/catalog", strings.NewReader(string(raw)))
	req.Header.Set("X-Test-User", "account-a")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusGone {
		t.Fatalf("plaintext publish status = %d, want 410", resp.StatusCode)
	}
}
