// Package e2ee is the §12.10 ciphertext-only proof: the real Go relay,
// catalog store, pairing, push (with mock providers) and HTTP logging run
// under a recorder, and the test asserts that no §2.1 canary appears in any
// encoding in any byte the server sends, stores or logs, while every canary
// still arrives at its intended endpoint.
//
// The server holds no decryption key by construction (§2.2): canaries travel
// inside AES-GCM seals made with a per-test random key that never leaves the
// test, standing in for the Noise seals the desktop and phone produce. The
// server only ever sees the ciphertext.
package e2ee
