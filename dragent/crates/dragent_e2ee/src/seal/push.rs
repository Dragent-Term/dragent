//! §8 push seal: Noise X from the desktop identity key to the phone's push
//! key under the push prologue, over a fixed 1,024-byte plaintext, so every
//! sealed notification is exactly 1,120 bytes (1,494 base64url characters).
//!
//! The plaintext layout (and its limits) is the codec's; this module only
//! insists on the fixed size. The recipient looks the returned sender key up
//! in its push-pin list and checks it against `desktop_id` inside the
//! plaintext; an unknown key is a failure (show the placeholder).

use zeroize::Zeroizing;

use crate::noise::{x, NoiseError, PublicKey, PushPrologue, StaticKeypair};

/// The fixed plaintext size.
pub const PLAINTEXT_LEN: usize = 1024;
/// The fixed sealed size: 32 + 48 + 1,024 + 16.
pub const SEALED_LEN: usize = PLAINTEXT_LEN + x::OVERHEAD;

/// Seal one notification plaintext.
pub fn seal(
    desktop_identity: &StaticKeypair,
    phone_push_key: &PublicKey,
    prologue: &PushPrologue,
    plaintext: &[u8; PLAINTEXT_LEN],
) -> Result<[u8; SEALED_LEN], NoiseError> {
    let m = x::seal(
        desktop_identity,
        phone_push_key,
        &prologue.bytes(),
        plaintext,
    )?;
    m.try_into().map_err(|_| NoiseError::Internal)
}

/// Open a sealed notification with the phone's push key. Returns the sender's
/// static key (to look up in the push-pin list) and the plaintext. Any error
/// means: show the constant placeholder.
pub fn open(
    phone_push_key: &StaticKeypair,
    prologue: &PushPrologue,
    sealed: &[u8],
) -> Result<(PublicKey, Zeroizing<[u8; PLAINTEXT_LEN]>), NoiseError> {
    if sealed.len() != SEALED_LEN {
        return Err(NoiseError::Malformed);
    }
    let (sender, pt) = x::open(phone_push_key, &prologue.bytes(), sealed)?;
    let mut out = Zeroizing::new([0u8; PLAINTEXT_LEN]);
    if pt.len() != PLAINTEXT_LEN {
        return Err(NoiseError::Malformed);
    }
    out.copy_from_slice(&pt);
    Ok((sender, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Id;

    #[test]
    fn fixed_size_round_trip() {
        assert_eq!(SEALED_LEN, 1120);
        let desktop = StaticKeypair::generate().unwrap();
        let push = StaticKeypair::generate().unwrap();
        let pro = PushPrologue {
            account: Id([1; 16]),
            phone: Id([2; 16]),
        };
        let mut pt = [0u8; PLAINTEXT_LEN];
        pt[0] = 1;
        let sealed = seal(&desktop, &push.public(), &pro, &pt).unwrap();
        let (sender, back) = open(&push, &pro, &sealed).unwrap();
        assert_eq!(sender, desktop.public());
        assert_eq!(&back[..], &pt[..]);
        // Wrong phone (prologue), wrong key, truncated: all fail.
        let other = PushPrologue {
            account: Id([1; 16]),
            phone: Id([9; 16]),
        };
        assert!(open(&push, &other, &sealed).is_err());
        assert!(open(&desktop, &pro, &sealed).is_err());
        assert_eq!(
            open(&push, &pro, &sealed[..1119]).unwrap_err(),
            NoiseError::Malformed
        );
    }
}
