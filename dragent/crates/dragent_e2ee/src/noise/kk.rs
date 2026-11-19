//! `Noise_KK_25519_ChaChaPoly_SHA256`: the live channel between a paired phone
//! (initiator) and desktop (responder). Both static keys are pinned (§3.1).
//!
//! ```text
//! KK:  -> s            (pre-message: phone's pinned key)
//!      <- s            (pre-message: desktop's pinned key)
//!      ...
//!      -> e, es, ss     HS1  payload {"v":1,"caps":[…]}   (replayable: no side effects)
//!      <- e, ee, se     HS2  payload caps
//! ```
//!
//! Then the phone sends DATA ctr 0 = CTRL `hello`, which is what confirms the
//! phone's keys to the desktop (§4.2); see [`crate::channel`].

use zeroize::Zeroizing;

use super::{
    hs_read, hs_write, KkPrologue, NoiseError, PublicKey, Setup, StaticKeypair, Transport, KK,
};

/// The phone after sending HS1 (`hs_sent`).
pub struct KkInitiator {
    hs: snow::HandshakeState,
}

impl KkInitiator {
    /// Build HS1 to the desktop whose key is pinned as `desktop_pin`.
    pub fn start(
        local: &StaticKeypair,
        desktop_pin: &PublicKey,
        prologue: &KkPrologue,
        payload: &[u8],
    ) -> Result<(Self, Vec<u8>), NoiseError> {
        let p = prologue.bytes();
        let mut setup = Setup::new(KK, &p);
        setup.local_secret = Some(local.secret_slice());
        setup.remote_public = Some(desktop_pin.as_bytes());
        let mut hs = setup.build(true)?;
        let hs1 = hs_write(&mut hs, payload)?;
        Ok((KkInitiator { hs }, hs1))
    }

    /// Process HS2. On success the handshake is complete: the returned
    /// transport authenticates the pinned desktop, and the payload is the
    /// desktop's caps.
    pub fn read_hs2(mut self, hs2: &[u8]) -> Result<(Transport, Zeroizing<Vec<u8>>), NoiseError> {
        let payload = hs_read(&mut self.hs, hs2)?;
        Ok((Transport::from_handshake(self.hs)?, payload))
    }
}

/// The desktop after a valid HS1 (`listen` → about to send HS2).
pub struct KkResponder {
    hs: snow::HandshakeState,
}

impl KkResponder {
    /// Process HS1 from the phone whose key is pinned as `phone_pin` (looked up
    /// by the frame's relay-written `peer`). Any error means CLOSE
    /// `unknown_peer` and no state kept (§4.2).
    ///
    /// HS1 can be replayed by the relay, so nothing in its payload may cause a
    /// side effect; act only after the channel is confirmed.
    pub fn read_hs1(
        local: &StaticKeypair,
        phone_pin: &PublicKey,
        prologue: &KkPrologue,
        hs1: &[u8],
    ) -> Result<(Self, Zeroizing<Vec<u8>>), NoiseError> {
        let p = prologue.bytes();
        let mut setup = Setup::new(KK, &p);
        setup.local_secret = Some(local.secret_slice());
        setup.remote_public = Some(phone_pin.as_bytes());
        let mut hs = setup.build(false)?;
        let payload = hs_read(&mut hs, hs1)?;
        Ok((KkResponder { hs }, payload))
    }

    /// Build HS2. The returned transport is not yet confirmed: the desktop
    /// sends no DATA and serves nothing until the phone's `hello` arrives.
    pub fn write_hs2(mut self, payload: &[u8]) -> Result<(Transport, Vec<u8>), NoiseError> {
        let hs2 = hs_write(&mut self.hs, payload)?;
        Ok((Transport::from_handshake(self.hs)?, hs2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ChannelId, Id};

    pub(crate) fn prologue() -> KkPrologue {
        KkPrologue {
            account: Id([1; 16]),
            phone: Id([2; 16]),
            desktop: Id([3; 16]),
            chan: ChannelId([4; 16]),
        }
    }

    #[test]
    fn handshake_and_transport() {
        let phone = StaticKeypair::generate().unwrap();
        let desktop = StaticKeypair::generate().unwrap();
        let (init, hs1) =
            KkInitiator::start(&phone, &desktop.public(), &prologue(), br#"{"v":1}"#).unwrap();
        let (resp, p1) =
            KkResponder::read_hs1(&desktop, &phone.public(), &prologue(), &hs1).unwrap();
        assert_eq!(&p1[..], br#"{"v":1}"#);
        let (dt, hs2) = resp.write_hs2(b"caps").unwrap();
        let (pt, p2) = init.read_hs2(&hs2).unwrap();
        assert_eq!(&p2[..], b"caps");
        assert_eq!(pt.handshake_hash(), dt.handshake_hash());
        assert_eq!(pt.remote_static(), desktop.public());
        assert_eq!(dt.remote_static(), phone.public());
        let ct = pt.seal(0, b"hello").unwrap();
        assert_eq!(&dt.open(0, &ct).unwrap()[..], b"hello");
        assert_eq!(dt.open(1, &ct).unwrap_err(), NoiseError::Rejected);
    }

    #[test]
    fn wrong_pin_or_prologue_is_rejected() {
        let phone = StaticKeypair::generate().unwrap();
        let desktop = StaticKeypair::generate().unwrap();
        let other = StaticKeypair::generate().unwrap();
        let (_, hs1) = KkInitiator::start(&phone, &desktop.public(), &prologue(), b"").unwrap();
        // Desktop has a different key pinned for this phone.
        assert!(KkResponder::read_hs1(&desktop, &other.public(), &prologue(), &hs1).is_err());
        // Prologue mismatch (different channel id).
        let mut p = prologue();
        p.chan = ChannelId([5; 16]);
        assert!(KkResponder::read_hs1(&desktop, &phone.public(), &p, &hs1).is_err());
        // Phone pinned the wrong desktop key.
        let (_, hs1) = KkInitiator::start(&phone, &other.public(), &prologue(), b"").unwrap();
        assert!(KkResponder::read_hs1(&desktop, &phone.public(), &prologue(), &hs1).is_err());
    }

    #[test]
    fn hs2_from_wrong_desktop_is_rejected() {
        let phone = StaticKeypair::generate().unwrap();
        let desktop = StaticKeypair::generate().unwrap();
        let impostor = StaticKeypair::generate().unwrap();
        let (init, hs1) = KkInitiator::start(&phone, &desktop.public(), &prologue(), b"").unwrap();
        // The impostor can't read HS1 (es/ss use the real desktop key)...
        assert!(KkResponder::read_hs1(&impostor, &phone.public(), &prologue(), &hs1).is_err());
        // ...and an HS2 from another handshake doesn't verify.
        let (_, hs1b) = KkInitiator::start(&phone, &desktop.public(), &prologue(), b"").unwrap();
        let (r, _) = KkResponder::read_hs1(&desktop, &phone.public(), &prologue(), &hs1b).unwrap();
        let (_, hs2b) = r.write_hs2(b"").unwrap();
        assert!(init.read_hs2(&hs2b).is_err());
        let _ = hs1;
    }

    #[test]
    fn bit_flips_fail() {
        let phone = StaticKeypair::generate().unwrap();
        let desktop = StaticKeypair::generate().unwrap();
        let (_, mut hs1) =
            KkInitiator::start(&phone, &desktop.public(), &prologue(), b"x").unwrap();
        let last = hs1.len() - 1;
        hs1[last] ^= 0x80;
        assert!(KkResponder::read_hs1(&desktop, &phone.public(), &prologue(), &hs1).is_err());
    }
}
