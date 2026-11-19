# Desktop update trust and release handoff

**Release blocker:** no release public key is provisioned in this checkout.
The updater deliberately refuses every update until a real key is baked into
the app and CI publishes signed manifests with artifact digests. Existing
unsigned manifests and legacy Windows pending markers are rejected. Do not
ship this change before completing the steps below. No private key has been
created or stored by this task, and no release workflow has been changed.

## Trust gates

Every supported desktop update authenticates `stable/latest.json` using a
compiled-in Ed25519 public key, then streams the downloaded artifact through
SHA-256 and compares it with the signed digest. Download-cache reuse goes
through the same digest check. There is no runtime key override. Production
keys can be put into the rotation-ready `RELEASE_PUBLIC_KEYS` list or supplied
as comma-separated 32-byte hex public keys in the **build-time** environment
variable `DRAGENT_UPDATE_PUBLIC_KEYS` (read with `option_env!`). A malformed
build-time key fails closed. Roll out old+new keys before switching signers,
and remove compromised/retired keys in a later release.

macOS additionally runs `/usr/bin/codesign --verify --deep --strict` with this
test requirement before the existing atomic bundle swap:

```text
=anchor apple generic and certificate leaf[subject.OU] = "HG9R2F47TR" and identifier "com.dragent.app"
```

The Team ID comes from `.github/workflows/self-hosted-release.yml`'s
notarization configuration; the bundle identifier comes from
`scripts/package-macos.sh` and the app Info.plist. The replacement must pass
even when the currently installed bundle is unsigned or damaged.

Windows persists the signed manifest with the pending marker and verifies
its signature, platform, version and installer digest again immediately
before launch. The installer is held open with only `FILE_SHARE_READ`,
preventing write/delete replacement between verification and process creation.
The pending marker is removed before launch so failures do not loop.

The release workflow currently builds unsigned Windows installers. The lead
explicitly chose signed manifests as their mandatory authentication gate.
`PUBLISHER_CERTIFICATE_SHA256` is an initially empty, rotation-ready list of
SHA-256 fingerprints of the DER publisher certificates. Once signing is
provisioned, add audited publisher fingerprints to require **both** release
authentication and Authenticode. The native verifier calls `WinVerifyTrust`
with Authenticode policy, no UI and chain revocation checks, accepts only
status zero, and compares the verified primary signer's leaf certificate
fingerprint, never the timestamp signer or an independently parsed certificate.
Any verification/pin failure refuses execution and logs a warning.

Linux currently has no updater engine, installer, apply operation or supported
manifest platform key; its background updater returns without downloading.
Shared signature/digest tests are portable, but adding signed Linux release
artifacts and a Linux installation/apply mechanism is separate work.

## Provision the key (person/release administrator)

Use a trusted machine with Python's `cryptography` package installed. This
creates a new Ed25519 PKCS#8 PEM private key with mode 0600 and prints only
its raw 32-byte hex public key. Keep the private file out of the repository.

```sh
python3 - <<'PY'
import os
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

key = Ed25519PrivateKey.generate()
pem = key.private_bytes(serialization.Encoding.PEM,
                        serialization.PrivateFormat.PKCS8,
                        serialization.NoEncryption())
fd = os.open('release-manifest-private.pem', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'wb') as f:
    f.write(pem)
print(key.public_key().public_bytes(serialization.Encoding.Raw,
                                   serialization.PublicFormat.Raw).hex())
PY
gh secret set RELEASE_MANIFEST_ED25519_PRIVATE_KEY < release-manifest-private.pem
```

Store the printed public key in the repository Actions variable
`RELEASE_MANIFEST_PUBLIC_KEYS`, and set this environment variable in **every**
desktop build job before compiling:

```yaml
DRAGENT_UPDATE_PUBLIC_KEYS: ${{ vars.RELEASE_MANIFEST_PUBLIC_KEYS }}
```

The private key belongs only in the GitHub Actions secret
`RELEASE_MANIFEST_ED25519_PRIVATE_KEY`. Keep a secure offline backup according
to the release administrator's key policy. Do not expose it in logs, build
arguments, artifacts or the app. A runtime environment variable cannot
replace the baked public key.

## CI manifest signing step

First compute SHA-256 over each exact final upload artifact (macOS: after
signing, notarization and stapling; Windows: after any signing). Add a
lowercase `sha256` hex string beside each platform's `url`. When merging
platform entries from another publisher job, retain that platform's final
digest, and re-sign the entire merged `stable` object. Sign only after all
merge/version/date mutations; upload the resulting manifest as one file.
Each independent publisher needs the private-key secret and the signing step.

The manifest has exactly two root fields, `stable` and `signature`:

```json
{
  "stable": {
    "platforms": {
      "windows-x86_64": {"sha256": "<64 hex digits>", "url": "https://cdn.example/setup.exe"}
    },
    "pub_date": null,
    "version": "1.2.3"
  },
  "signature": "<128 hex digits>"
}
```

Sign the following bytes, with no trailing newline:

1. The ASCII domain prefix `dragent-desktop-update-v1` followed by one LF byte.
2. The compact UTF-8 JSON encoding of `stable`, with every object key sorted,
   no insignificant whitespace, and an explicit `pub_date: null` if absent.
   Required stable fields are `platforms`, `pub_date`, `version`; each platform
   contains exactly `sha256`, `url`. No numbers or unknown fields are allowed.

The canonical serializer in `signature.rs` declares fields in that order
and uses a sorted platform map. The following Python step produces identical
signing bytes for this schema. Add it to both release workflows, with
`cryptography` installed and a dependency version pinned by the CI owner:

```yaml
- name: Sign final update manifest
  working-directory: dragent
  env:
    RELEASE_MANIFEST_ED25519_PRIVATE_KEY: ${{ secrets.RELEASE_MANIFEST_ED25519_PRIVATE_KEY }}
  run: |
    python3 - <<'PY'
    import json, os
    from pathlib import Path
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    path = Path('dist/latest.json')
    stable = json.loads(path.read_text())['stable']
    stable.setdefault('pub_date', None)
    assert set(stable) == {'platforms', 'pub_date', 'version'}
    for asset in stable['platforms'].values():
        assert set(asset) == {'sha256', 'url'}
        assert len(bytes.fromhex(asset['sha256'])) == 32
    canonical = json.dumps(stable, sort_keys=True, ensure_ascii=False,
                           separators=(',', ':')).encode('utf-8')
    key = serialization.load_pem_private_key(
        os.environ['RELEASE_MANIFEST_ED25519_PRIVATE_KEY'].encode(), password=None)
    assert isinstance(key, Ed25519PrivateKey)
    signature = key.sign(b'dragent-desktop-update-v1\n' + canonical).hex()
    path.write_text(json.dumps({'stable': stable, 'signature': signature},
                               ensure_ascii=False, separators=(',', ':')))
    PY
```

Before publishing, CI should verify that this signature validates against the
public key baked into the desktop builds and that every uploaded artifact's
digest matches the signed manifest. The task's unit tests include a canonical
byte vector, key rotation, wrong key, unsigned/tampered manifests, malformed
digest, changed artifact, and native macOS rejection of another valid signer.
