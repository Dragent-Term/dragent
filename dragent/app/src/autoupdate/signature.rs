//! Release authentication. See README.md for the signed manifest wire format.

use std::collections::BTreeMap;
use std::io::Read;

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// RELEASE BLOCKER: replace this placeholder with audited release public keys
// before shipping. An empty set deliberately trusts no manifest. Never put a
// test/example key here. Keep old and new keys during a planned rotation.
const RELEASE_PUBLIC_KEYS: &[[u8; 32]] = &[];
const SIGNATURE_DOMAIN: &[u8] = b"dragent-desktop-update-v1\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    stable: Release,
    signature: String,
}

// Field order is part of the canonical format, as are sorted platform keys,
// compact UTF-8 JSON and an explicit null for an absent publication date.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    pub platforms: BTreeMap<String, PlatformAsset>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlatformAsset {
    pub sha256: String,
    pub url: String,
}

fn signing_bytes(release: &Release) -> Result<Vec<u8>, String> {
    let mut bytes = SIGNATURE_DOMAIN.to_vec();
    serde_json::to_writer(&mut bytes, release).map_err(|e| format!("canonical manifest: {e}"))?;
    Ok(bytes)
}

pub(super) fn verify_manifest(body: &str) -> Result<Release, String> {
    let mut keys = RELEASE_PUBLIC_KEYS.to_vec();
    // Trusted build configuration only: a runtime environment variable must
    // never be able to change the release trust root.
    if let Some(configured) = option_env!("DRAGENT_UPDATE_PUBLIC_KEYS") {
        for key in configured.split(',') {
            keys.push(decode_hex::<32>(key.trim(), "release public key")?);
        }
    }
    verify_manifest_with_keys(body, &keys)
}

fn verify_manifest_with_keys(body: &str, keys: &[[u8; 32]]) -> Result<Release, String> {
    let manifest: Manifest =
        serde_json::from_str(body).map_err(|e| format!("signed manifest: {e}"))?;
    let signature = Signature::from_bytes(&decode_hex::<64>(
        &manifest.signature,
        "manifest signature",
    )?);
    let bytes = signing_bytes(&manifest.stable)?;
    if !keys.iter().any(|key| {
        VerifyingKey::from_bytes(key).is_ok_and(|key| key.verify_strict(&bytes, &signature).is_ok())
    }) {
        return Err("manifest signature does not match a pinned release key".into());
    }
    // These strings later become path components. Reject signed but malformed
    // releases before any filesystem operation.
    let version = &manifest.stable.version;
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || version == "."
        || version == ".."
    {
        return Err("invalid signed release version".into());
    }
    for asset in manifest.stable.platforms.values() {
        decode_hex::<32>(&asset.sha256, "artifact SHA-256")?;
    }
    Ok(manifest.stable)
}

fn decode_hex<const N: usize>(value: &str, what: &str) -> Result<[u8; N], String> {
    let mut bytes = [0; N];
    hex::decode_to_slice(value, &mut bytes).map_err(|e| format!("invalid {what}: {e}"))?;
    Ok(bytes)
}

pub(super) fn verify_artifact(mut file: impl Read, expected: &str) -> Result<(), String> {
    let expected = decode_hex::<32>(expected, "artifact SHA-256")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("hash update: {e}"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let actual: [u8; 32] = hash.finalize().into();
    if actual != expected {
        return Err("update artifact SHA-256 does not match the signed manifest".into());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn macos_codesign_arguments() -> [&'static str; 5] {
    [
        "--verify",
        "--deep",
        "--strict",
        "--test-requirement",
        // Source: .github/workflows/self-hosted-release.yml notarization Team ID.
        "=anchor apple generic and certificate leaf[subject.OU] = \"HG9R2F47TR\" and identifier \"com.dragent.app\"",
    ]
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn macos_verification_succeeded(exit_code: Option<i32>) -> bool {
    exit_code == Some(0)
}

#[cfg(any(windows, test))]
pub(super) fn windows_signer_allowed(
    trust_status: i32,
    signer: Option<&[u8; 32]>,
    pins: &[[u8; 32]],
) -> Result<(), String> {
    // WinVerifyTrust returns LONG, not HRESULT. ONLY zero means trusted.
    if trust_status != 0 {
        return Err(format!(
            "Authenticode verification failed: {trust_status:#x}"
        ));
    }
    if !signer.is_some_and(|signer| pins.contains(signer)) {
        return Err("Authenticode signer is not a pinned Dragent publisher".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_manifest(key: &SigningKey) -> String {
        let release = Release {
            platforms: BTreeMap::from([(
                "windows-x86_64".into(),
                PlatformAsset {
                    sha256: hex::encode(Sha256::digest(b"installer")),
                    url: "https://example.test/setup.exe".into(),
                },
            )]),
            pub_date: None,
            version: "1.2.3".into(),
        };
        serde_json::json!({
            "stable": release,
            "signature": hex::encode(key.sign(&signing_bytes(&release).unwrap()).to_bytes()),
        })
        .to_string()
    }

    #[test]
    fn good_manifest_and_artifact() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let release =
            verify_manifest_with_keys(&signed_manifest(&key), &[key.verifying_key().to_bytes()])
                .unwrap();
        verify_artifact(
            &b"installer"[..],
            &release.platforms["windows-x86_64"].sha256,
        )
        .unwrap();
    }

    #[test]
    fn wrong_key_and_empty_pins_refused() {
        let body = signed_manifest(&SigningKey::from_bytes(&[7; 32]));
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes();
        assert!(verify_manifest_with_keys(&body, &[other]).is_err());
        assert!(verify_manifest_with_keys(&body, &[]).is_err());
    }

    #[test]
    fn tampered_manifest_refused() {
        let key = SigningKey::from_bytes(&[7; 32]);
        for (from, to) in [
            ("1.2.3", "9.9.9"),
            ("example.test", "attacker.test"),
            (&hex::encode(Sha256::digest(b"installer")), &"00".repeat(32)),
        ] {
            let body = signed_manifest(&key).replace(from, to);
            assert!(verify_manifest_with_keys(&body, &[key.verifying_key().to_bytes()]).is_err());
        }
    }

    #[test]
    fn unsigned_manifest_refused() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut body: serde_json::Value = serde_json::from_str(&signed_manifest(&key)).unwrap();
        body.as_object_mut().unwrap().remove("signature");
        assert!(
            verify_manifest_with_keys(&body.to_string(), &[key.verifying_key().to_bytes()])
                .is_err()
        );
    }

    #[test]
    fn tampered_artifact_and_malformed_hash_refused() {
        let hash = hex::encode(Sha256::digest(b"installer"));
        assert!(verify_artifact(&b"tampered installer"[..], &hash).is_err());
        assert!(verify_artifact(&b"installer"[..], "").is_err());
    }

    #[test]
    fn canonical_bytes_are_stable_across_json_order_and_whitespace() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let body = signed_manifest(&key);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(verify_manifest_with_keys(
            &serde_json::to_string_pretty(&value).unwrap(),
            &[key.verifying_key().to_bytes()],
        )
        .is_ok());
        let release = verify_manifest_with_keys(&body, &[key.verifying_key().to_bytes()]).unwrap();
        assert_eq!(
            String::from_utf8(signing_bytes(&release).unwrap()).unwrap(),
            format!(
                "dragent-desktop-update-v1\n{{\"platforms\":{{\"windows-x86_64\":{{\"sha256\":\"{}\",\"url\":\"https://example.test/setup.exe\"}}}},\"pub_date\":null,\"version\":\"1.2.3\"}}",
                hex::encode(Sha256::digest(b"installer")),
            ),
        );
    }

    #[test]
    fn python_canonicalization_and_node_ed25519_vector_matches() {
        // Independently generated using the README's Python canonicalization
        // and Node crypto.sign, with the fixed test-only seed [7; 32].
        let body = signed_manifest(&SigningKey::from_bytes(&[7; 32]));
        let manifest: Manifest = serde_json::from_str(&body).unwrap();
        assert_eq!(
            manifest.signature,
            concat!(
                "bf273bc6f60c9f4b7c7be55f80cf4e9280cac2da9b19a060a7fb1a32d6cac4c099",
                "f9a2fa0d5dfc03074e851a885b845dddf3626b728c4918ffc27dd735544902"
            ),
        );
    }

    #[test]
    fn release_key_rotation_accepts_either_pinned_key() {
        let old = SigningKey::from_bytes(&[7; 32]);
        let new = SigningKey::from_bytes(&[8; 32]);
        let pins = [
            old.verifying_key().to_bytes(),
            new.verifying_key().to_bytes(),
        ];
        assert!(verify_manifest_with_keys(&signed_manifest(&old), &pins).is_ok());
        assert!(verify_manifest_with_keys(&signed_manifest(&new), &pins).is_ok());
    }

    #[test]
    fn macos_gate_requires_our_team_and_rejects_all_verification_failures() {
        let args = macos_codesign_arguments();
        assert_eq!(
            &args[..4],
            &["--verify", "--deep", "--strict", "--test-requirement"]
        );
        assert!(args[4].contains("anchor apple generic"));
        assert!(args[4].contains("certificate leaf[subject.OU] = \"HG9R2F47TR\""));
        assert!(args[4].contains("identifier \"com.dragent.app\""));
        assert!(macos_verification_succeeded(Some(0))); // good designated signer
        assert!(!macos_verification_succeeded(Some(3))); // valid, other team / requirement
        assert!(!macos_verification_succeeded(Some(1))); // unsigned / tampered
        assert!(!macos_verification_succeeded(None)); // killed verifier
    }

    #[test]
    fn authenticode_requires_good_signature_and_pinned_leaf() {
        let good = [1; 32];
        let rotated = [2; 32];
        let other = [3; 32];
        let pins = [good, rotated];
        assert!(windows_signer_allowed(0, Some(&good), &pins).is_ok());
        assert!(windows_signer_allowed(0, Some(&rotated), &pins).is_ok());
        assert!(windows_signer_allowed(0, Some(&other), &pins).is_err());
        assert!(windows_signer_allowed(0, None, &pins).is_err());
        assert!(windows_signer_allowed(0, Some(&good), &[]).is_err());
        assert!(windows_signer_allowed(0x800b0100u32 as i32, None, &pins).is_err()); // unsigned
        assert!(windows_signer_allowed(0x80096010u32 as i32, Some(&good), &pins).is_err()); // tampered
        assert!(windows_signer_allowed(1, Some(&good), &pins).is_err()); // positive warning
    }
}
