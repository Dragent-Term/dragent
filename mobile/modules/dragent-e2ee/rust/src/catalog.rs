//! §6.5 catalog at rest, phone side: verify and open a sealed blob.
//!
//! The order is fixed so nothing from an unverified blob is acted on:
//! decode → the blob names the expected desktop → this phone's wrap exists →
//! high-water check (rollback and equivocation are refused before any
//! decryption) → unwrap with Noise X, the sender must be the **pinned**
//! desktop key and the wrapped version must match → open the body. The
//! high-water mark is advanced in the pin store ([`crate::PinStore`]); the
//! caller persists it with `commit()`.
//!
//! The blob already accepted at the high-water mark (same version **and**
//! same body hash) is verified and opened again and returned as `Accepted`
//! with the mark unchanged: the phone keeps the mark across restarts but not
//! the plaintext, so it must be able to reopen the current blob. Anything
//! older is still a rollback, and the same version with another body is
//! still equivocation.

use dragent_e2ee::codec::catalog_blob;
use dragent_e2ee::noise::{CatalogPrologue, PublicKey, StaticKeypair};
use dragent_e2ee::seal::catalog::{self, CatalogError};
use dragent_e2ee::seal::pin_store::{CatalogCheck, PinStore};
use dragent_e2ee::Id;
use sha2::Digest;

use crate::E2eeError;

/// The outcome of opening a catalog blob.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CatalogResult {
    /// Newer than the high-water mark, verified and decrypted.
    Accepted {
        /// The catalog version.
        version: u64,
        /// The catalog JSON (UTF-8 bytes).
        json: Vec<u8>,
    },
    // Not produced any more: the blob already accepted at the high-water
    // mark is reopened and returned as `Accepted` (see the module docs). The
    // variant and its doc stay so the generated Swift/Kotlin bindings match.
    /// The blob already accepted at the high-water mark: nothing new.
    Unchanged {
        /// The version.
        version: u64,
    },
}

impl From<CatalogError> for E2eeError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::WrongSender => E2eeError::WrongSender,
            CatalogError::Random => E2eeError::Random,
            CatalogError::Malformed | CatalogError::BadVersion => E2eeError::malformed(e),
            CatalogError::Rejected | CatalogError::VersionMismatch => E2eeError::Rejected {
                detail: format!("catalog: {e:?}"),
            },
        }
    }
}

pub(crate) struct OpenArgs<'a> {
    pub identity: &'a StaticKeypair,
    pub desktop_pin: &'a PublicKey,
    pub account: Id,
    pub phone: Id,
    pub desktop: Id,
    pub blob: &'a [u8],
}

pub(crate) fn open(store: &mut PinStore, a: OpenArgs<'_>) -> Result<CatalogResult, E2eeError> {
    let blob = catalog_blob::decode(a.blob).map_err(E2eeError::malformed)?;
    if blob.desktop != a.desktop {
        return Err(E2eeError::Rejected {
            detail: "catalog blob names another desktop".into(),
        });
    }
    let wrap = blob
        .recipients
        .iter()
        .find(|r| r.phone == a.phone)
        .ok_or_else(|| E2eeError::Rejected {
            detail: "catalog blob has no wrap for this phone".into(),
        })?;
    let body_sha: [u8; 32] = sha2::Sha256::digest(&blob.body).into();
    let advance = match store.catalog_check(&a.desktop, blob.version, &body_sha) {
        CatalogCheck::Accept => true,
        // Same version and same body as the accepted blob: reopen it.
        CatalogCheck::Unchanged => false,
        CatalogCheck::Rollback => return Err(E2eeError::CatalogRollback),
        CatalogCheck::Equivocation => return Err(E2eeError::CatalogEquivocation),
    };
    let prologue = CatalogPrologue {
        account: a.account,
        desktop: a.desktop,
        phone: a.phone,
        version: blob.version,
    };
    let k = catalog::unwrap_key(a.identity, a.desktop_pin, &prologue, &wrap.wrap)?;
    let json = catalog::open_body(
        &k,
        &a.account,
        &a.desktop,
        blob.version,
        &blob.nonce,
        &blob.body,
    )?;
    if advance {
        store.catalog_accept(a.desktop, blob.version, &body_sha);
    }
    Ok(CatalogResult::Accepted {
        version: blob.version,
        json: json.to_vec(),
    })
}
