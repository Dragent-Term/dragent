//! The crypto resolver handed to `snow`.
//!
//! `snow` 0.10.0 does not wipe key material when its internal objects are
//! dropped, and with our feature set (§3.2: no `use-getrandom`) its default
//! resolver has no RNG. This resolver therefore supplies:
//!
//! - the RNG: the operating system generator via `getrandom` 0.3.4;
//! - X25519: `x25519-dalek` 2.0.1 [`StaticSecret`], which zeroises on drop;
//! - ChaChaPoly: `chacha20poly1305` 0.10.1 with the Noise nonce layout
//!   (32 zero bits, then the 64-bit counter little-endian), holding its key in
//!   a [`Zeroizing`] buffer;
//! - SHA-256: `snow`'s own implementation (`sha2` 0.10.9) unchanged.
//!
//! These are adapters over the pinned primitives, written to match `snow`'s
//! default resolver byte for byte; the Noise test vectors (`vectors/noise/`)
//! check that. Nothing else is resolved, so a misconfigured pattern fails to
//! build instead of silently picking another primitive.
//!
//! Residual (for the P2 audit): `snow`'s handshake `SymmetricState` (chaining
//! key, handshake hash) and its stack temporaries during `split` are not wiped
//! by `snow` itself.

use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use snow::params::{CipherChoice, DHChoice, HashChoice};
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::types::{Cipher, Dh, Hash, Random};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::rng::OsRandom;

const KEYLEN: usize = 32;
const TAGLEN: usize = 16;

/// The only resolver this crate gives to `snow`.
#[derive(Default)]
pub(crate) struct E2eeResolver;

impl CryptoResolver for E2eeResolver {
    fn resolve_rng(&self) -> Option<Box<dyn Random>> {
        Some(Box::new(OsRandom))
    }

    fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
        match choice {
            DHChoice::Curve25519 => Some(Box::<ZeroizingX25519>::default()),
            _ => None,
        }
    }

    fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
        match choice {
            HashChoice::SHA256 => DefaultResolver.resolve_hash(choice),
            _ => None,
        }
    }

    fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
        match choice {
            CipherChoice::ChaChaPoly => Some(Box::<ZeroizingChaChaPoly>::default()),
            _ => None,
        }
    }
}

/// X25519 for `snow`, backed by `x25519-dalek`'s zeroising secret.
#[derive(Default)]
pub(crate) struct ZeroizingX25519 {
    secret: Option<StaticSecret>,
    public: [u8; KEYLEN],
}

impl ZeroizingX25519 {
    fn install(&mut self, bytes: Zeroizing<[u8; KEYLEN]>) {
        let secret = StaticSecret::from(*bytes);
        self.public = XPublicKey::from(&secret).to_bytes();
        self.secret = Some(secret);
    }
}

impl Dh for ZeroizingX25519 {
    fn name(&self) -> &'static str {
        "25519"
    }

    fn pub_len(&self) -> usize {
        KEYLEN
    }

    fn priv_len(&self) -> usize {
        KEYLEN
    }

    fn set(&mut self, privkey: &[u8]) {
        let mut bytes = Zeroizing::new([0u8; KEYLEN]);
        let n = privkey.len().min(KEYLEN);
        bytes[..n].copy_from_slice(&privkey[..n]);
        self.install(bytes);
    }

    fn generate(&mut self, rng: &mut dyn Random) -> Result<(), snow::Error> {
        let mut bytes = Zeroizing::new([0u8; KEYLEN]);
        rng.try_fill_bytes(&mut bytes[..])?;
        self.install(bytes);
        Ok(())
    }

    fn pubkey(&self) -> &[u8] {
        &self.public
    }

    fn privkey(&self) -> &[u8] {
        match &self.secret {
            Some(s) => s.as_bytes(),
            None => &[0u8; KEYLEN],
        }
    }

    fn dh(&self, pubkey: &[u8], out: &mut [u8]) -> Result<(), snow::Error> {
        let secret = self.secret.as_ref().ok_or(snow::Error::Dh)?;
        if pubkey.len() < KEYLEN || out.len() < KEYLEN {
            return Err(snow::Error::Dh);
        }
        let mut theirs = [0u8; KEYLEN];
        theirs.copy_from_slice(&pubkey[..KEYLEN]);
        let shared = secret.diffie_hellman(&XPublicKey::from(theirs));
        // Refuse low-order public keys (an all-zero shared secret). The Noise
        // spec allows either behaviour; refusing is the conservative one.
        if !shared.was_contributory() {
            return Err(snow::Error::Dh);
        }
        out[..KEYLEN].copy_from_slice(shared.as_bytes());
        Ok(())
    }
}

/// ChaCha20-Poly1305 for `snow` with a key that is wiped on drop.
#[derive(Default)]
pub(crate) struct ZeroizingChaChaPoly {
    key: Zeroizing<[u8; KEYLEN]>,
}

impl ZeroizingChaChaPoly {
    fn nonce(n: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&n.to_le_bytes());
        nonce
    }

    #[cfg(test)]
    pub(crate) fn key_is_zero(&self) -> bool {
        self.key.iter().all(|&b| b == 0)
    }

    #[cfg(test)]
    pub(crate) fn wipe(&mut self) {
        self.key.zeroize();
    }
}

impl Cipher for ZeroizingChaChaPoly {
    fn name(&self) -> &'static str {
        "ChaChaPoly"
    }

    fn set(&mut self, key: &[u8; KEYLEN]) {
        self.key.copy_from_slice(key);
    }

    fn encrypt(&self, nonce: u64, authtext: &[u8], plaintext: &[u8], out: &mut [u8]) -> usize {
        let len = plaintext.len();
        out[..len].copy_from_slice(plaintext);
        let tag = ChaCha20Poly1305::new((&*self.key).into())
            .encrypt_in_place_detached(&Self::nonce(nonce).into(), authtext, &mut out[..len])
            // Only fails for messages longer than 2^38 bytes; Noise caps at 65,535.
            .expect("ChaCha20-Poly1305 message within length limit");
        out[len..len + TAGLEN].copy_from_slice(&tag);
        len + TAGLEN
    }

    fn decrypt(
        &self,
        nonce: u64,
        authtext: &[u8],
        ciphertext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, snow::Error> {
        if ciphertext.len() < TAGLEN {
            return Err(snow::Error::Decrypt);
        }
        let len = ciphertext.len() - TAGLEN;
        out[..len].copy_from_slice(&ciphertext[..len]);
        let result = ChaCha20Poly1305::new((&*self.key).into()).decrypt_in_place_detached(
            &Self::nonce(nonce).into(),
            authtext,
            &mut out[..len],
            ciphertext[len..].into(),
        );
        if result.is_err() {
            // Don't leave unauthenticated bytes behind in the caller's buffer.
            out[..len].zeroize();
            return Err(snow::Error::Decrypt);
        }
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_round_trips_and_rejects_tamper() {
        let mut c = ZeroizingChaChaPoly::default();
        c.set(&[7u8; 32]);
        let mut ct = [0u8; 5 + 16];
        assert_eq!(c.encrypt(3, b"ad", b"hello", &mut ct), 21);
        let mut pt = [0u8; 5];
        assert_eq!(c.decrypt(3, b"ad", &ct, &mut pt).unwrap(), 5);
        assert_eq!(&pt, b"hello");
        assert!(c.decrypt(4, b"ad", &ct, &mut pt).is_err());
        ct[0] ^= 1;
        assert!(c.decrypt(3, b"ad", &ct, &mut pt).is_err());
        assert_eq!(pt, [0u8; 5], "failed open leaves no plaintext behind");
    }

    #[test]
    fn cipher_key_wipes() {
        let mut c = ZeroizingChaChaPoly::default();
        c.set(&[9u8; 32]);
        assert!(!c.key_is_zero());
        c.wipe();
        assert!(c.key_is_zero());
    }

    #[test]
    fn dh_rejects_low_order_points() {
        let mut d = ZeroizingX25519::default();
        d.generate(&mut OsRandom).unwrap();
        let mut out = [0u8; 32];
        assert!(d.dh(&[0u8; 32], &mut out).is_err());
        let mut one = [0u8; 32];
        one[0] = 1;
        assert!(d.dh(&one, &mut out).is_err());
    }

    #[test]
    fn resolver_only_resolves_the_pinned_suite() {
        let r = E2eeResolver;
        assert!(r.resolve_hash(&HashChoice::Blake2s).is_none());
        assert!(r.resolve_cipher(&CipherChoice::AESGCM).is_none());
        assert!(r.resolve_rng().is_some());
    }
}
