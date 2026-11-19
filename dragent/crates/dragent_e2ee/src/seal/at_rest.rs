//! XChaCha20-Poly1305 for bodies at rest (the catalog body and the pin store),
//! with random 24-byte nonces (§3.1).

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::rng::{fill_random, RandomError};

/// Nonce length.
pub const NONCE_LEN: usize = 24;
/// Tag length.
pub const TAG_LEN: usize = 16;

/// A 32-byte symmetric key, wiped on drop.
pub struct SymmetricKey(Zeroizing<[u8; 32]>);

impl SymmetricKey {
    /// A fresh random key.
    pub fn generate() -> Result<Self, RandomError> {
        let mut k = Zeroizing::new([0u8; 32]);
        fill_random(&mut k[..])?;
        Ok(SymmetricKey(k))
    }

    /// A key read from the keychain, secure store, or an unwrapped catalog wrap.
    pub fn from_bytes(bytes: Zeroizing<[u8; 32]>) -> Self {
        SymmetricKey(bytes)
    }

    /// The raw key, for storing it in the keychain or wrapping it.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SymmetricKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SymmetricKey(..)")
    }
}

/// Authentication failed (wrong key, wrong associated data, or tampering).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenError;

/// Encrypt `plaintext` bound to `ad` under a fresh random nonce.
pub fn seal(
    key: &SymmetricKey,
    ad: &[u8],
    plaintext: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), RandomError> {
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce)?;
    let ct = XChaCha20Poly1305::new((&*key.0).into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: ad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption of an in-memory buffer");
    Ok((nonce, ct))
}

/// Decrypt and authenticate.
pub fn open(
    key: &SymmetricKey,
    nonce: &[u8; NONCE_LEN],
    ad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    XChaCha20Poly1305::new((&*key.0).into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: ad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| OpenError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_binding() {
        let k = SymmetricKey::generate().unwrap();
        let (n, ct) = seal(&k, b"ad", b"secret").unwrap();
        assert_eq!(ct.len(), 6 + TAG_LEN);
        assert_eq!(&open(&k, &n, b"ad", &ct).unwrap()[..], b"secret");
        assert_eq!(open(&k, &n, b"other", &ct).unwrap_err(), OpenError);
        let k2 = SymmetricKey::generate().unwrap();
        assert_eq!(open(&k2, &n, b"ad", &ct).unwrap_err(), OpenError);
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(open(&k, &n, b"ad", &bad).unwrap_err(), OpenError);
        let (n2, _) = seal(&k, b"ad", b"secret").unwrap();
        assert_ne!(n, n2, "nonces are random");
    }

    /// XChaCha20-Poly1305 known-answer test from
    /// draft-irtf-cfrg-xchacha-03 Appendix A.3.1.
    #[test]
    fn xchacha_known_answer() {
        let h = |s: &str| crate::ids::hex_decode_lower(s).unwrap();
        let key: [u8; 32] = h("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
            .try_into()
            .unwrap();
        let nonce: [u8; 24] = h("404142434445464748494a4b4c4d4e4f5051525354555657")
            .try_into()
            .unwrap();
        let ad = h("50515253c0c1c2c3c4c5c6c7");
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let expected = h(concat!(
            "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb",
            "731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452",
            "2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9",
            "21f9664c97637da9768812f615c68b13b52e",
            "c0875924c1c7987947deafd8780acf49"
        ));
        let k = SymmetricKey::from_bytes(Zeroizing::new(key));
        assert_eq!(&open(&k, &nonce, &ad, &expected).unwrap()[..], &pt[..]);
    }
}
