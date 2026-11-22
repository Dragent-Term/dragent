//! Background auto-update, modeled on Warp's updater (see dev/warp
//! app/src/autoupdate) but deliberately smaller:
//!
//!   check `latest.json` → compare versions → download the platform asset →
//!   apply (macOS: atomic bundle swap now, new version on next launch;
//!          Windows: stage the Inno Setup installer, run it silently on the
//!          NEXT app start and relaunch).
//!
//! Manifest URL resolution (same pattern as `api::backend_url`):
//!   1. `DRAGENT_UPDATE_MANIFEST_URL` (runtime env — dev/testing)
//!   2. `DRAGENT_UPDATE_MANIFEST_URL_DEFAULT` (baked by release CI)
//!   3. none → updater disabled.
//!
//! Safety rails: disabled in debug builds unless the runtime env var is set,
//! disabled entirely by `DRAGENT_NO_AUTOUPDATE=1`, and macOS only applies when
//! the app actually runs from a writable `.app` bundle (so `cargo run` and
//! translocated/quarantined copies are never touched).

#[cfg(target_os = "macos")]
mod mac;
#[cfg(any(target_os = "macos", windows, test))]
mod signature;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_authenticode;

#[cfg(windows)]
pub use windows::apply_pending_on_startup;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
// Check shortly after startup so users do not need to open Settings or click
// an explicit check before an available release is discovered.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Set once an update has been fully downloaded and staged/applied; the UI can
/// poll this to show a "Restart to update" affordance.
static UPDATE_READY: AtomicBool = AtomicBool::new(false);
static READY_VERSION: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
static AVAILABLE_VERSION: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
static CHECK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    /// No newer release is known yet.
    Unknown,
    /// A newer release was found and is being downloaded/staged.
    Available(String),
    /// The update is ready and will be used after the app restarts.
    Ready(String),
}

/// The version waiting to take effect on next launch, if any.
pub fn update_ready_version() -> Option<String> {
    if UPDATE_READY.load(Ordering::Relaxed) {
        READY_VERSION.lock().clone()
    } else {
        None
    }
}

/// Current update state for UI surfaces such as the account menu.
pub fn update_state() -> UpdateState {
    if let Some(version) = update_ready_version() {
        return UpdateState::Ready(version);
    }
    AVAILABLE_VERSION
        .lock()
        .clone()
        .map_or(UpdateState::Unknown, UpdateState::Available)
}

/// Ask the updater to check immediately. The normal background loop still
/// performs periodic checks; this is the explicit check started from the UI.
pub fn request_check() {
    if !CHECK_IN_PROGRESS.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            if let Some(url) = manifest_url() {
                #[cfg(any(target_os = "macos", windows))]
                if let Err(e) = check_once(&url) {
                    log::warn!("[autoupdate] manual check failed: {e}");
                }
            }
            CHECK_IN_PROGRESS.store(false, Ordering::Release);
        });
    }
}

/// Relaunch the application after the current process has terminated.
///
/// The short-lived helper is intentional: on Windows the new process must not
/// start until the old process has released the install lock, and on macOS it
/// avoids launching two copies while the old app is still shutting down.
pub fn restart_to_update() {
    let Ok(executable) = std::env::current_exe() else {
        log::error!("[autoupdate] unable to resolve current executable for restart");
        return;
    };

    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "sleep 1; exec \"$1\"",
            "dragent-restart",
            &executable.to_string_lossy(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    #[cfg(windows)]
    let spawned = {
        let command = format!(
            "timeout /t 1 /nobreak >NUL & start \"\" \"{}\"",
            executable.display()
        );
        std::process::Command::new("cmd.exe")
            .args(["/C", &command])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };

    #[cfg(not(any(target_os = "macos", windows)))]
    let spawned: Result<std::process::Child, std::io::Error> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "automatic restart is only supported on macOS and Windows",
    ));

    match spawned {
        Ok(_) => {
            log::info!("[autoupdate] relaunch helper started; exiting current process");
            std::process::exit(0);
        }
        Err(error) => log::error!("[autoupdate] failed to start relaunch helper: {error}"),
    }
}

// ── Manifest ──────────────────────────────────────────────────────────────────

fn manifest_url() -> Option<String> {
    std::env::var("DRAGENT_UPDATE_MANIFEST_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| option_env!("DRAGENT_UPDATE_MANIFEST_URL_DEFAULT").map(str::to_string))
}

/// Key into the manifest's `platforms` map for this build.
fn platform_key() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "darwin-aarch64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "darwin-x86_64";
    #[cfg(all(windows, target_arch = "x86_64"))]
    return "windows-x86_64";
    #[cfg(all(windows, target_arch = "aarch64"))]
    return "windows-aarch64";
    #[allow(unreachable_code)]
    "unsupported"
}

/// A release version, ordered as semantic versions are: `0.2.9-rc.1` <
/// `0.2.9-rc.2` < `0.2.9-rc.10` < `0.2.9`. A pre-release (`-rc.N`) is older
/// than the release it leads to, so a release candidate's testers move on
/// to each later candidate and then to the release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppVersion {
    core: (u64, u64, u64),
    /// Dot-separated pre-release identifiers; empty for a release.
    pre: Vec<String>,
}

impl Ord for AppVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(x), Ok(y)) => x.cmp(&y),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for AppVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Parse "1.2.3", "v1.2.3" or "1.2.3-rc.4" (build metadata after `+` is
/// ignored).
fn parse_version(v: &str) -> Option<AppVersion> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((_, "")) => return None,
        Some((core, pre)) => (core, pre),
        None => (v, ""),
    };
    let mut it = core.splitn(3, '.');
    let maj = it.next()?.parse().ok()?;
    let min = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    let pre: Vec<String> = if pre.is_empty() {
        Vec::new()
    } else {
        pre.split('.').map(str::to_string).collect()
    };
    if pre.iter().any(|id| id.is_empty()) {
        return None;
    }
    Some(AppVersion {
        core: (maj, min, patch),
        pre,
    })
}

fn updates_dir() -> PathBuf {
    dragent_paths::home().join("cache").join("updates")
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Entry point: spawn from `main` on a background thread.
pub fn start_background() {
    if std::env::var_os("DRAGENT_NO_AUTOUPDATE").is_some() {
        log::info!("[autoupdate] disabled via DRAGENT_NO_AUTOUPDATE");
        return;
    }
    // Debug builds never auto-update unless explicitly pointed at a manifest.
    if cfg!(debug_assertions) && std::env::var_os("DRAGENT_UPDATE_MANIFEST_URL").is_none() {
        return;
    }
    let Some(url) = manifest_url() else {
        log::info!("[autoupdate] no manifest URL configured — updater off");
        return;
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = url;
        return;
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        std::thread::sleep(FIRST_CHECK_DELAY);
        loop {
            if !CHECK_IN_PROGRESS.swap(true, Ordering::AcqRel) {
                let result = check_once(&url);
                CHECK_IN_PROGRESS.store(false, Ordering::Release);
                if let Err(e) = result {
                    log::warn!("[autoupdate] check failed: {e}");
                }
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn check_once(manifest_url: &str) -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let body = ureq::get(manifest_url)
        .timeout(Duration::from_secs(30))
        .set("Cache-Control", "no-cache")
        .call()
        .map_err(|e| format!("fetch manifest: {e}"))?
        .into_string()
        .map_err(|e| format!("read manifest: {e}"))?;
    let release = signature::verify_manifest(&body)?;

    let (Some(latest), Some(cur)) = (parse_version(&release.version), parse_version(current))
    else {
        return Err(format!(
            "unparsable version: latest={} current={current}",
            release.version
        ));
    };
    if latest <= cur {
        *AVAILABLE_VERSION.lock() = None;
        log::info!(
            "[autoupdate] up to date (current {current}, latest {})",
            release.version
        );
        return Ok(());
    }
    // Already downloaded+staged this version in this run — nothing to do.
    if READY_VERSION.lock().as_deref() == Some(release.version.as_str()) {
        return Ok(());
    }

    let key = platform_key();
    let Some(asset) = release.platforms.get(key) else {
        return Err(format!("manifest has no asset for platform '{key}'"));
    };
    if !asset.url.starts_with("https://") {
        return Err(format!("refusing non-https update url: {}", asset.url));
    }

    log::info!(
        "[autoupdate] update available: {current} → {} — downloading {}",
        release.version,
        asset.url
    );
    *AVAILABLE_VERSION.lock() = Some(release.version.clone());
    let downloaded = download(&asset.url, &release.version)?;
    let file = std::fs::File::open(&downloaded).map_err(|e| format!("open update: {e}"))?;
    signature::verify_artifact(file, &asset.sha256)?;

    #[cfg(target_os = "macos")]
    mac::apply(&downloaded, &release.version)?;
    #[cfg(windows)]
    windows::stage(&downloaded, &release.version, &body)?;

    *READY_VERSION.lock() = Some(release.version.clone());
    UPDATE_READY.store(true, Ordering::Relaxed);
    log::info!(
        "[autoupdate] version {} ready — takes effect on next launch",
        release.version
    );
    Ok(())
}

/// Download the asset into the per-version updates dir; resumes nothing, but
/// reuses an existing complete-looking file from an earlier attempt.
#[cfg(any(target_os = "macos", windows))]
fn download(url: &str, version: &str) -> Result<PathBuf, String> {
    let file_name = url.rsplit('/').next().unwrap_or("update.bin");
    let dir = updates_dir().join(version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let dest = dir.join(file_name);

    let resp = ureq::get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .call()
        .map_err(|e| format!("download: {e}"))?;
    let expected: Option<u64> = resp.header("Content-Length").and_then(|v| v.parse().ok());
    if let (Some(len), Ok(meta)) = (expected, std::fs::metadata(&dest)) {
        if meta.len() == len {
            log::info!("[autoupdate] reusing already-downloaded {}", dest.display());
            return Ok(dest);
        }
    }

    let tmp = dest.with_extension("part");
    {
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
        std::io::copy(&mut reader, &mut file).map_err(|e| format!("write: {e}"))?;
    }
    if let (Some(len), Ok(meta)) = (expected, std::fs::metadata(&tmp)) {
        if meta.len() != len {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("truncated download: {} of {len} bytes", meta.len()));
        }
    }
    std::fs::rename(&tmp, &dest).map_err(|e| format!("finalize download: {e}"))?;
    Ok(dest)
}

/// Best-effort cleanup of updates for other/older versions.
#[cfg(any(target_os = "macos", windows))]
fn cleanup_other_versions(keep: &str) {
    let dir = updates_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_and_ordering() {
        let v = |s| parse_version(s).unwrap();
        assert_eq!(v("1.2.3"), v("v1.2.3"));
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(parse_version("garbage").is_none());
        assert!(parse_version("1.2.3-").is_none());
        // Release candidates: each later one, then the release.
        assert!(v("0.2.9-rc.1") < v("0.2.9-rc.2"));
        assert!(v("0.2.9-rc.9") < v("0.2.9-rc.10"));
        assert!(v("0.2.9-rc.10") < v("0.2.9"));
        assert!(v("0.2.8") < v("0.2.9-rc.1"));
        assert!(v("0.2.9") < v("0.2.10-rc.1"));
        assert_eq!(v("0.2.9-rc.1+build.5"), v("0.2.9-rc.1"));
    }
}
