//! Windows update applier: the downloaded artifact is the Inno Setup installer
//! (our .iss keeps a constant AppId, so a re-run is an in-place upgrade — the
//! same mechanism Warp uses). We stage it plus a pending-marker; on the NEXT
//! app start `apply_pending_on_startup` runs the installer silently and exits,
//! and the installer's `/update=1` Run entry relaunches the new version.

use std::fs::{File, OpenOptions};
use std::io::Seek;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Pending {
    version: String,
    installer: PathBuf,
    // Keep the original signed manifest: never trust a digest in pending.json
    // without authenticating it again immediately before installer execution.
    manifest: String,
}

// Windows releases are currently unsigned (.github/workflows/release.yml).
// Until the publisher certificate is provisioned, release authenticity comes
// from the mandatory signed manifest + artifact digest. Add audited SHA-256
// certificate thumbprints here to ALSO require Authenticode. During rotation
// keep both publisher certificates until older clients have migrated.
const PUBLISHER_CERTIFICATE_SHA256: &[[u8; 32]] = &[];

fn pending_path() -> PathBuf {
    super::updates_dir().join("pending.json")
}

/// Record the downloaded installer as ready to apply on next start.
pub(super) fn stage(installer: &Path, version: &str, manifest: &str) -> Result<(), String> {
    let pending = Pending {
        version: version.to_string(),
        installer: installer.to_path_buf(),
        manifest: manifest.to_string(),
    };
    let _verified = verify_pending(&pending)?;
    let json = serde_json::to_string(&pending).map_err(|e| e.to_string())?;
    std::fs::write(pending_path(), json).map_err(|e| format!("write pending marker: {e}"))?;
    super::cleanup_other_versions(version);
    Ok(())
}

/// Called synchronously at the very top of `main` (before any UI): if a newer
/// version's installer is staged, run it silently and exit — the installer
/// waits for this process to finish, upgrades in place, and relaunches.
///
/// The marker is removed BEFORE spawning so a failing installer can't create
/// an install loop; the next background check re-stages if still applicable.
pub fn apply_pending_on_startup() {
    let marker = pending_path();
    let Ok(raw) = std::fs::read_to_string(&marker) else {
        return;
    };
    let _ = std::fs::remove_file(&marker);

    let pending = match serde_json::from_str::<Pending>(&raw) {
        Ok(pending) => pending,
        Err(e) => {
            log::warn!("[autoupdate] refusing invalid/unsigned pending installer marker: {e}");
            return;
        }
    };
    // The file stays open without write/delete sharing until CreateProcess
    // returns, preventing replacement between verification and execution.
    let (installer, verified_file) = match verify_pending(&pending) {
        Ok(verified) => verified,
        Err(e) => {
            log::warn!("[autoupdate] refusing staged installer: {e}");
            return;
        }
    };
    let current = super::parse_version(env!("CARGO_PKG_VERSION"));
    let staged = super::parse_version(&pending.version);
    match (staged, current) {
        (Some(s), Some(c)) if s > c => {}
        // Already on (or past) the staged version — stale marker, clean up.
        _ => {
            let _ = std::fs::remove_dir_all(super::updates_dir().join(&pending.version));
            return;
        }
    }
    // The installer replaces files our background services run from, and it
    // can't close them itself (they have no window): stop them first. Their
    // terminal sessions are recorded as interrupted by the update, so panes
    // resume their agents once the new version starts.
    log::info!("[autoupdate] stopping background services before installing");
    dragent_host::pty_supervisor::stop_services_for_update();
    let log_arg = format!(
        "/LOG={}",
        super::updates_dir().join("install.log").display()
    );
    // Same silent-upgrade invocation Warp uses (Inno Setup setupcmdline):
    // /SILENT keeps a progress bar visible; /update=1 makes our [Run] entry
    // relaunch the app after the upgrade even in silent mode.
    let spawned = Command::new(&installer)
        .args([
            "/SP-",
            "/SILENT",
            "/NOCANCEL",
            "/NORESTART",
            "/update=1",
            &log_arg,
        ])
        .spawn();
    drop(verified_file);
    match spawned {
        Ok(_) => {
            log::info!(
                "[autoupdate] handing off to installer for {} — exiting",
                pending.version
            );
            std::process::exit(0);
        }
        Err(e) => log::warn!("[autoupdate] failed to launch staged installer: {e}"),
    }
}

fn verify_pending(pending: &Pending) -> Result<(PathBuf, File), String> {
    let release = super::signature::verify_manifest(&pending.manifest)?;
    if release.version != pending.version {
        return Err("pending version does not match signed release".into());
    }
    let asset = release
        .platforms
        .get(super::platform_key())
        .ok_or_else(|| "signed manifest has no installer for this platform".to_string())?;
    let installer =
        std::fs::canonicalize(&pending.installer).map_err(|e| format!("resolve installer: {e}"))?;
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ, deliberately no WRITE or DELETE.
        .open(&installer)
        .map_err(|e| format!("lock installer for verification: {e}"))?;
    super::signature::verify_artifact(&mut file, &asset.sha256)?;
    if !PUBLISHER_CERTIFICATE_SHA256.is_empty() {
        file.rewind()
            .map_err(|e| format!("rewind installer: {e}"))?;
        super::windows_authenticode::verify_publisher(
            &installer,
            &file,
            PUBLISHER_CERTIFICATE_SHA256,
        )?;
    }
    Ok((installer, file))
}
