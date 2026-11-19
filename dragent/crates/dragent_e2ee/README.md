# dragent_e2ee

End-to-end encryption core for the phone relay and push notifications.
Specification: `docs/design/e2ee.md` (revision 9). One Rust implementation
for the desktop app, the iOS/Android native module and the iOS Notification
Service Extension.

| Module | What | Spec |
|---|---|---|
| `noise` | KK (live channel), IKpsk1 (pairing), X (catalog wrap, push) on the pinned crates; §5 prologues; identity keys; stateless explicit-nonce transport | §3, §5, §6.2 |
| `channel` | §4.2 state machine (no data before confirmation), §4.3 receiver (any gap is `protocol_error`), §4.6 resume (drain, `input_lost`, implementer notes 1–3) | §4.2, §4.3, §4.6 |
| `seal` | sealed pin store, XChaCha20-Poly1305 at rest, catalog key wrap, fixed-size push seal | §6.1, §6.5, §8 |
| `codec` | wire codecs (header, record, TERM, parts, blobs) — owned by E1b | §4.1, §4.4, §4.5 |

Everything is pure state except `seal::pin_store::{read_file, write_atomic, commit}`.
Time is a caller-supplied monotonic `now_ms`.

## Using it from the app (E3a, E3b, E3c, E9a)

The app depends on this crate **optionally**, behind the app feature `e2ee`,
which is off by default and not enabled in release builds until the P2 audit
gate and cut-over (§3.3, §10). Gate every use:

```rust
#[cfg(feature = "e2ee")]
mod e2e_channel;
```

and build/test with:

```sh
cargo check -p dragent                  # must pass (feature off)
cargo check -p dragent --features e2ee  # must pass (feature on)
cargo test  -p dragent --features e2ee <your tests>
```

## Dependency pins (§3.2)

`snow =0.10.0` with features exactly `use-curve25519`, `use-chacha20poly1305`,
`use-sha2` (not `std`: in snow 0.10.0 it switches on ring, blake2 and
getrandom 0.2). Because that leaves snow without an RNG, and because snow does
not wipe keys, the crate hands snow its own resolver: the OS RNG via
`getrandom =0.3.4`, X25519 via `x25519-dalek =2.0.1` `StaticSecret`
(zeroise-on-drop; low-order points refused) and ChaChaPoly via
`chacha20poly1305 =0.10.1` with a zeroising key. SHA-256 is snow's own. The
Noise vectors run through these adapters.

`scripts/check-deps.sh` (also run by `tests/dep_pins.rs`) fails if snow gains
any other feature, if anything but getrandom 0.3.4 is linked on any target, or
if ring/blake2/aes-gcm/p256 appear.

Handshake state: every `snow::HandshakeState` is consumed by value into the
transport (or dropped at the end of the X seal/open call); no copy is kept.
Its ephemeral and static DH keys are our zeroising type. snow's internal
`SymmetricState` (chaining key, hash) is not wiped by snow; that is a P2 audit
item (§3.3) — snow itself is not patched.

## Pin store (`state.v1`)

`magic "DRGPINS1" | generation u64 BE | nonce[24] | XChaCha20-Poly1305(JSON)`,
AD = magic ‖ generation. State key and generation live in the keychain. Write:
seal as `g+1`, temp file, fsync, rename, fsync dir, then bump the keychain to
`g+1`. Read accepts file generation `g` or `g+1` (crash between rename and
bump; `Opened::bump_keychain_to` says to finish it); anything else, a failed
tag, or a missing file when `g > 0` is `StoreError::Lost`: fail closed and
pair again.

## Encodings chosen here (not spelled out in the design)

- Pairing PSK salt: the desktop id in canonical lowercase text form.
- Catalog body AD: `"dragent catalog v1"` ‖ account (16 raw bytes) ‖ desktop
  (16 raw bytes) ‖ version (u64 BE) — 58 bytes.
- Catalog wrap plaintext: `K` (32) ‖ version (u64 BE); wrap is 136 bytes.
