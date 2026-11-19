// Sealed store tests (§6.5, §2.2): routing only — desktop id, recipient
// phone ids, blob version, padded size, publish times. No catalog field is
// ever stored, listed, or returned.
package relay

import (
	"context"
	"fmt"
	"testing"
	"time"
)

func mustOwner(t *testing.T, user, device string) string {
	t.Helper()
	owner, ok := ownerKey(user, device)
	if !ok {
		t.Fatalf("ownerKey(%q, %q) rejected", user, device)
	}
	return owner
}

func putSealed(t *testing.T, store *MemorySealedStore, user, desktop string, version uint64) {
	t.Helper()
	owner, ok := ownerKey(user, desktop)
	if !ok {
		t.Fatalf("ownerKey(%q, %q) rejected", user, desktop)
	}
	if err := store.PutSealed(context.Background(), owner, makeSealedBlob(t, desktop, version)); err != nil {
		t.Fatal(err)
	}
}

func TestSealedDiscoveryListsOnlyOwnAccount(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	putSealed(t, store, "user-a", sealedTestDesktop, 1)
	putSealed(t, store, "user-a", sealedTestDesktop2, 2)
	putSealed(t, store, "user-b", sealedTestDesktop, 9)

	var _ SealedStore = store

	machines, err := store.ListSealed(ctx, "user-a")
	if err != nil {
		t.Fatal(err)
	}
	if len(machines) != 2 {
		t.Fatalf("user-a machines = %d, want 2", len(machines))
	}
	if machines[0].DeviceID != sealedTestDesktop || machines[1].DeviceID != sealedTestDesktop2 {
		t.Fatalf("machines not sorted/scoped: %+v", machines)
	}
	for _, m := range machines {
		if !m.Online || m.LastSeenAt <= 0 {
			t.Fatalf("fresh publish must be online at last-seen: %+v", m)
		}
		if m.BlobVersion == 0 || m.BlobSize != sealedBodyBucket {
			t.Fatalf("routing summary wrong: %+v", m)
		}
		if m.DeviceID == "" {
			t.Fatal("machine without device id")
		}
	}
	if machines[0].BlobVersion != 1 || machines[1].BlobVersion != 2 {
		t.Fatalf("blob versions wrong: %+v", machines)
	}

	other, err := store.ListSealed(ctx, "user-b")
	if err != nil {
		t.Fatal(err)
	}
	if len(other) != 1 || other[0].BlobVersion != 9 {
		t.Fatalf("user-b machines = %+v", other)
	}

	none, err := store.ListSealed(ctx, "nobody")
	if err != nil {
		t.Fatal(err)
	}
	if none == nil || len(none) != 0 {
		t.Fatalf("unknown account must list an empty array, got %+v", none)
	}
}

func TestSealedOnlineExpiryUsesInjectedClock(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	now := time.Now()
	store.SetClock(func() time.Time { return now })
	putSealed(t, store, "user-a", sealedTestDesktop, 4)

	blob, found, err := store.GetSealed(ctx, mustOwner(t, "user-a", sealedTestDesktop))
	if err != nil || !found {
		t.Fatalf("GetSealed = %+v, %v, %v", blob, found, err)
	}
	if blob.Desktop != sealedTestDesktop || blob.Version != 4 {
		t.Fatalf("blob wrong: %+v", blob)
	}

	// 31s later the lease expires but the blob stays readable.
	now = now.Add(31 * time.Second)
	listed, err := store.ListSealed(ctx, "user-a")
	if err != nil {
		t.Fatal(err)
	}
	if len(listed) != 1 || listed[0].Online {
		t.Fatalf("machine past the 30s lease must read offline: %+v", listed)
	}
	if listed[0].BlobVersion != 4 || listed[0].BlobSize != sealedBodyBucket {
		t.Fatalf("offline routing lost: %+v", listed[0])
	}
	if _, found, _ := store.GetSealed(ctx, mustOwner(t, "user-a", sealedTestDesktop2)); found {
		t.Fatal("unknown device must read absent")
	}
}

func TestSealedHeartbeatRefreshesLease(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	now := time.Now()
	store.SetClock(func() time.Time { return now })
	putSealed(t, store, "user-a", sealedTestDesktop, 1)
	// Heartbeat 20s later extends the lease.
	now = now.Add(20 * time.Second)
	putSealed(t, store, "user-a", sealedTestDesktop, 2)
	now = now.Add(20 * time.Second)
	listed, err := store.ListSealed(ctx, "user-a")
	if err != nil {
		t.Fatal(err)
	}
	if len(listed) != 1 || !listed[0].Online {
		t.Fatalf("heartbeat should have kept the machine online: %+v", listed)
	}
	if listed[0].BlobVersion != 2 {
		t.Fatalf("latest blob version not served: %+v", listed[0])
	}
}

func TestSealedRejectsDesktopMismatch(t *testing.T) {
	store := NewMemorySealedStore()
	owner := mustOwner(t, "user-a", sealedTestDesktop)
	blob := makeSealedBlob(t, sealedTestDesktop2, 1)
	if err := store.PutSealed(context.Background(), owner, blob); err == nil {
		t.Fatal("desktop-mismatched blob accepted")
	}
	if _, found, _ := store.GetSealed(context.Background(), owner); found {
		t.Fatal("rejected blob stored")
	}
}

func TestSealedEntriesBoundedPerAccount(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	now := time.Now()
	store.SetClock(func() time.Time { return now })
	for i := 0; i < maxMachinesPerAccount+5; i++ {
		now = now.Add(time.Second)
		desktop := fmt.Sprintf("00000000-0000-4000-8000-%012d", i)
		putSealed(t, store, "user-a", desktop, 1)
	}
	// Another account's devices must not count toward (or be evicted by)
	// the first account's bound.
	putSealed(t, store, "user-b", sealedTestDesktop, 1)

	machines, err := store.ListSealed(ctx, "user-a")
	if err != nil {
		t.Fatal(err)
	}
	if len(machines) != maxMachinesPerAccount {
		t.Fatalf("machines = %d, want bounded %d", len(machines), maxMachinesPerAccount)
	}
	other, err := store.ListSealed(ctx, "user-b")
	if err != nil {
		t.Fatal(err)
	}
	if len(other) != 1 {
		t.Fatalf("other account affected by bound: %+v", other)
	}
}

func TestSealedBlobsDoNotAliasStore(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	blob := makeSealedBlob(t, sealedTestDesktop, 1)
	owner := mustOwner(t, "user-a", sealedTestDesktop)
	if err := store.PutSealed(ctx, owner, blob); err != nil {
		t.Fatal(err)
	}
	blob.Recipients[0].Phone = sealedTestPhone2
	got, found, err := store.GetSealed(ctx, owner)
	if err != nil || !found {
		t.Fatalf("GetSealed = %+v, %v, %v", got, found, err)
	}
	if got.Recipients[0].Phone != sealedTestPhone {
		t.Fatalf("stored blob aliases caller: %+v", got)
	}
	got.Recipients[0].Phone = sealedTestPhone2
	again, _, _ := store.GetSealed(ctx, owner)
	if again.Recipients[0].Phone != sealedTestPhone {
		t.Fatalf("stored blob aliases reader: %+v", again)
	}
}

func TestSealedPutRejectsInvalidOwnerWithoutMutating(t *testing.T) {
	store := NewMemorySealedStore()
	blob := makeSealedBlob(t, sealedTestDesktop, 1)
	for _, owner := range []string{"", "device", "device\x00user", "device\x00\x00desktop"} {
		if err := store.PutSealed(context.Background(), owner, blob); err == nil {
			t.Fatalf("owner %q accepted", owner)
		}
	}
	if _, found, _ := store.GetSealed(context.Background(), mustOwner(t, "user-a", sealedTestDesktop)); found {
		t.Fatal("rejected put mutated the store")
	}
}
