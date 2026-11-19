//! `Noise_X_25519_ChaChaPoly_SHA256`: one-way sealed messages (§3.1), used for
//! the catalog key wrap (§6.5) and push notifications (§8).
//!
//! ```text
//! X:  <- s                  (pre-message: recipient key)
//!     ...
//!     -> e, es, s, ss        one message: 32 (e) + 48 (encrypted s) + payload + 16 (tag)
//! ```
//!
//! The recipient learns the sender's static key from the message and must
//! check it against its pins; [`open`] returns it for that purpose. Typed
//! wrappers live in [`crate::seal::catalog`] and [`crate::seal::push`].

use zeroize::Zeroizing;

use super::{hs_read, hs_write, NoiseError, PublicKey, Setup, StaticKeypair, MAX_MESSAGE_LEN, X};

/// Bytes an X message adds to its payload: 32 + 48 + 16.
pub const OVERHEAD: usize = 32 + 48 + 16;
/// The largest payload one X message carries.
pub const MAX_PAYLOAD: usize = MAX_MESSAGE_LEN - OVERHEAD;

/// Seal `plaintext` from `sender` to `recipient` under `prologue`.
pub(crate) fn seal(
    sender: &StaticKeypair,
    recipient: &PublicKey,
    prologue: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, NoiseError> {
    if plaintext.len() > MAX_PAYLOAD {
        return Err(NoiseError::Malformed);
    }
    let mut setup = Setup::new(X, prologue);
    setup.local_secret = Some(sender.secret_slice());
    setup.remote_public = Some(recipient.as_bytes());
    let mut hs = setup.build(true)?;
    hs_write(&mut hs, plaintext)
}

/// Open a message sealed to `recipient` under `prologue`. Returns the sender's
/// static key (unverified: the caller checks it against its pins) and the
/// plaintext.
pub(crate) fn open(
    recipient: &StaticKeypair,
    prologue: &[u8],
    message: &[u8],
) -> Result<(PublicKey, Zeroizing<Vec<u8>>), NoiseError> {
    if message.len() < OVERHEAD {
        return Err(NoiseError::Malformed);
    }
    let mut setup = Setup::new(X, prologue);
    setup.local_secret = Some(recipient.secret_slice());
    let mut hs = setup.build(false)?;
    let plaintext = hs_read(&mut hs, message)?;
    if !hs.is_handshake_finished() {
        return Err(NoiseError::Internal);
    }
    let sender = hs
        .get_remote_static()
        .and_then(PublicKey::from_slice)
        .ok_or(NoiseError::Internal)?;
    Ok((sender, plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip_and_size() {
        let sender = StaticKeypair::generate().unwrap();
        let recipient = StaticKeypair::generate().unwrap();
        let m = seal(&sender, &recipient.public(), b"p", &[7u8; 1024]).unwrap();
        assert_eq!(m.len(), 1024 + OVERHEAD);
        assert_eq!(m.len(), 1120);
        let (from, pt) = open(&recipient, b"p", &m).unwrap();
        assert_eq!(from, sender.public());
        assert_eq!(&pt[..], &[7u8; 1024][..]);
    }

    #[test]
    fn wrong_recipient_prologue_or_tamper_fails() {
        let sender = StaticKeypair::generate().unwrap();
        let recipient = StaticKeypair::generate().unwrap();
        let other = StaticKeypair::generate().unwrap();
        let mut m = seal(&sender, &recipient.public(), b"p", b"hi").unwrap();
        assert!(open(&other, b"p", &m).is_err());
        assert!(open(&recipient, b"q", &m).is_err());
        m[40] ^= 1; // inside the encrypted static key
        assert!(open(&recipient, b"p", &m).is_err());
        assert!(open(&recipient, b"p", &m[..10]).is_err());
    }

    #[test]
    fn oversize_payload_refused() {
        let sender = StaticKeypair::generate().unwrap();
        let recipient = StaticKeypair::generate().unwrap();
        assert!(seal(&sender, &recipient.public(), b"", &vec![0; MAX_PAYLOAD]).is_ok());
        assert_eq!(
            seal(&sender, &recipient.public(), b"", &vec![0; MAX_PAYLOAD + 1]).unwrap_err(),
            NoiseError::Malformed
        );
    }
}
