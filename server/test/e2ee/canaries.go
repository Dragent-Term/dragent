package e2ee

import (
	"bytes"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"net/url"
	"sync"
	"testing"
)

// canaries is one unique secret per §2.1 flow. The random suffix keeps every
// run distinct so a match can only come from this run's traffic, and the
// fixed prefixes name the flow the canary stands for.
type canaries struct {
	// F1 terminal stream.
	input, output, screenGrid, keystroke string
	// F2 served inside the channel.
	tabTitle, fileName, fileContent, path, branch string
	// F4 catalog.
	machineName, projectName, agentState string
	// F5 coordination.
	coordGoal, coordMessage string
	// F6 provider work items.
	workItemTitle, workItemQuery string
	// F8 attention stream.
	attentionTitle, attentionBody string
	// F9 push.
	pushTitle, pushBody string
	// F11 pairing.
	deviceName string

	all []string
}

func newCanaries(t *testing.T) *canaries {
	t.Helper()
	var suffix [8]byte
	if _, err := rand.Read(suffix[:]); err != nil {
		t.Fatal(err)
	}
	mk := func(flow string) string {
		return "e2ee-canary-" + flow + "-" + hex.EncodeToString(suffix[:])
	}
	c := &canaries{
		input:          mk("input"),
		output:         mk("output"),
		screenGrid:     mk("screen"),
		keystroke:      mk("key"),
		tabTitle:       mk("tabtitle"),
		fileName:       mk("filename"),
		fileContent:    mk("filecontent"),
		path:           mk("path"),
		branch:         mk("branch"),
		machineName:    mk("machine"),
		projectName:    mk("project"),
		agentState:     mk("agentstate"),
		coordGoal:      mk("coordgoal"),
		coordMessage:   mk("coordmsg"),
		workItemTitle:  mk("workitem"),
		workItemQuery:  mk("workquery"),
		attentionTitle: mk("attentiontitle"),
		attentionBody:  mk("attentionbody"),
		pushTitle:      mk("pushtitle"),
		pushBody:       mk("pushbody"),
		deviceName:     mk("devicename"),
	}
	c.all = []string{
		c.input, c.output, c.screenGrid, c.keystroke,
		c.tabTitle, c.fileName, c.fileContent, c.path, c.branch,
		c.machineName, c.projectName, c.agentState,
		c.coordGoal, c.coordMessage,
		c.workItemTitle, c.workItemQuery,
		c.attentionTitle, c.attentionBody,
		c.pushTitle, c.pushBody,
		c.deviceName,
	}
	return c
}

// sealer stands in for the endpoint Noise seals: AES-256-GCM under a
// per-test random key with random nonces. The key never leaves the test, so
// server-side bytes can only be ciphertext. Two seals of the same plaintext
// differ (fresh nonce), matching the §12.10 "random-looking bodies" check.
type sealer struct {
	aead cipher.AEAD
}

func newSealer(t *testing.T) *sealer {
	t.Helper()
	var key [32]byte
	if _, err := rand.Read(key[:]); err != nil {
		t.Fatal(err)
	}
	block, err := aes.NewCipher(key[:])
	if err != nil {
		t.Fatal(err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		t.Fatal(err)
	}
	return &sealer{aead: aead}
}

func (s *sealer) seal(plain string) []byte {
	nonce := make([]byte, s.aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		panic(err)
	}
	return s.aead.Seal(nonce, nonce, []byte(plain), nil)
}

func (s *sealer) open(t *testing.T, ct []byte) string {
	t.Helper()
	nonce := ct[:s.aead.NonceSize()]
	plain, err := s.aead.Open(nil, nonce, ct[s.aead.NonceSize():], nil)
	if err != nil {
		t.Fatalf("endpoint could not open its own seal: %v", err)
	}
	return string(plain)
}

// recorder collects every byte the server sends, stores or logs during a
// run: forwarded frame bytes (network), stored blobs and records, HTTP
// responses, mock push payloads, and captured log lines.
type recorder struct {
	mu      sync.Mutex
	network [][]byte
	stored  [][]byte
	replies [][]byte
	pushes  [][]byte
	logs    bytes.Buffer
}

func (r *recorder) addNetwork(b []byte) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.network = append(r.network, append([]byte(nil), b...))
}

func (r *recorder) addStored(b []byte) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.stored = append(r.stored, append([]byte(nil), b...))
}

func (r *recorder) addReply(b []byte) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.replies = append(r.replies, append([]byte(nil), b...))
}

func (r *recorder) addPush(b []byte) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pushes = append(r.pushes, append([]byte(nil), b...))
}

// serverBytes returns everything the server emitted or kept, excluding the
// endpoint-held seals above. Nothing here may carry a canary.
func (r *recorder) serverBytes() []byte {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []byte
	for _, group := range [][][]byte{r.network, r.stored, r.replies, r.pushes} {
		for _, b := range group {
			out = append(out, b...)
		}
	}
	out = append(out, r.logs.Bytes()...)
	return out
}

// canaryForms renders one canary in every encoding §12.10 names: raw and the
// base64 (standard and URL), hex and URL-escaped forms a server could
// produce when it copies content into logs, JSON, tokens or URLs.
func canaryForms(c string) []string {
	return []string{
		c,
		base64.StdEncoding.EncodeToString([]byte(c)),
		base64.URLEncoding.EncodeToString([]byte(c)),
		base64.RawURLEncoding.EncodeToString([]byte(c)),
		hex.EncodeToString([]byte(c)),
		url.QueryEscape(c),
	}
}

// assertNoCanary fails when any canary appears in any encoding in haystack.
func assertNoCanary(t *testing.T, where string, haystack []byte, c *canaries) {
	t.Helper()
	for _, canary := range c.all {
		for _, form := range canaryForms(canary) {
			if bytes.Contains(haystack, []byte(form)) {
				t.Fatalf("%s carries canary %q (as %q)", where, canary, form)
			}
		}
	}
}

// assertEndpointDelivery seals every canary, opens each seal again, and
// checks the plaintext survived — the §12.10 companion assertion that every
// canary arrives at its intended endpoint even though the server sees only
// ciphertext.
func assertEndpointDelivery(t *testing.T, s *sealer, c *canaries) {
	t.Helper()
	for _, canary := range c.all {
		if got := s.open(t, s.seal(canary)); got != canary {
			t.Fatalf("endpoint delivery lost canary %q (got %q)", canary, got)
		}
	}
}

// assertRandomLooking fails when a ciphertext body is degenerate (empty, all
// one byte, or byte-identical to the plaintext): §12.10 requires frames and
// blobs to carry random-looking bodies under their headers.
func assertRandomLooking(t *testing.T, what string, body []byte, plain string) {
	t.Helper()
	if len(body) == 0 {
		t.Fatalf("%s body is empty", what)
	}
	if string(body) == plain {
		t.Fatalf("%s body is the plaintext", what)
	}
	same := true
	for _, b := range body {
		if b != body[0] {
			same = false
			break
		}
	}
	if same {
		t.Fatalf("%s body is a constant byte run", what)
	}
	if a, b := newSealer(t).seal(plain), newSealer(t).seal(plain); fmt.Sprintf("%x", a) == fmt.Sprintf("%x", b) {
		t.Fatalf("%s seals are deterministic across keys", what)
	}
}
