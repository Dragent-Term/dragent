# Noise test vectors (§12.1)

`kk.json`, `ikpsk1.json` and `x.json` hold every vector for
`Noise_KK_25519_ChaChaPoly_SHA256`, `Noise_IKpsk1_25519_ChaChaPoly_SHA256` and
`Noise_X_25519_ChaChaPoly_SHA256` from the cacophony and snow vector sets that
ship inside the `snow` 0.10.0 crate package (`tests/vectors/cacophony.txt`,
`tests/vectors/snow.txt`), copied unmodified apart from an added `source`
field. They are run by `src/noise/vectors.rs` through this crate's own
resolver (zeroising X25519 and ChaChaPoly adapters) and, for the two-way
patterns, through `noise::Transport` with explicit per-direction nonces.

The phone module (E4) and the NSE test target run the same files.
