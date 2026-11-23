//! Secure credential storage for the refresh token.
//!
//! Storage strategy:
//!   Primary — platform app data/dragent/credentials (0o600 on Unix)
//!   CI      — DRAGENT_REFRESH_TOKEN env var (checked first)
//!
//! Writes are atomic (temp + fsync + rename) — see `file_save` for why a lost
//! write costs the user their session.
//!
//! The OS keychain is deliberately not used here. The token rotates on every
//! refresh, so a keychain copy is written constantly, and on macOS each write
//! from a build the item doesn't trust is a permission popup: every rebuild of
//! a dev binary, since its code signature changes. A copy also can't add safety
//! because it goes stale, and presenting a stale token trips the backend's reuse
//! detection, which revokes the whole token family. The owner-only credentials
//! file is the de-facto standard used by gh, gcloud, and aws-cli for this.
//!
//! Migration (Release edition only): if the old ~/.dragent/token file exists it
//! is migrated to the platform path on first load, then deleted. A Dev build
//! never touches it: it belongs to the installed app.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::warn;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Primary credentials file in the platform's local app data directory.
///
/// Tests get a throwaway one instead. A test that reached the API client
/// (an agent turn posting a tool result) read the person's real refresh token
/// and exchanged it: the server rotates refresh tokens and revokes the whole
/// family on reuse, so every test run signed the running app out (or, with a
/// refused token, deleted the file outright).
fn credentials_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return Some(test_credentials_dir().join("credentials"));
    }
    #[cfg(not(test))]
    Some(dragent_paths::platform_support().join("credentials"))
}

/// Legacy path from the initial implementation. Only used for one-time migration.
fn legacy_token_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return Some(test_credentials_dir().join("legacy-token"));
    }
    #[cfg(not(test))]
    legacy_token_path_for(dragent_paths::edition(), dirs::home_dir()?.as_path())
}

/// The legacy token file belongs to the installed (Release) app: a Dev build
/// must never read, migrate or delete it.
#[cfg_attr(test, allow(dead_code))]
fn legacy_token_path_for(edition: dragent_paths::Edition, user_home: &Path) -> Option<PathBuf> {
    match edition {
        dragent_paths::Edition::Release => Some(user_home.join(".dragent").join("token")),
        dragent_paths::Edition::Dev => None,
    }
}

/// Where [`load_refresh_token`] found the token, for logs (never the token).
pub fn refresh_token_source() -> String {
    if std::env::var("DRAGENT_REFRESH_TOKEN").is_ok_and(|t| !t.trim().is_empty()) {
        return "DRAGENT_REFRESH_TOKEN".to_string();
    }
    match credentials_path() {
        Some(path) => format!("credentials file {}", path.display()),
        None => "credentials file".to_string(),
    }
}

/// Where a test build keeps its credentials: never the person's.
#[cfg(test)]
fn test_credentials_dir() -> PathBuf {
    std::env::temp_dir().join(format!("dragent-test-credentials-{}", std::process::id()))
}

// ── File I/O ──────────────────────────────────────────────────────────────────

fn file_load(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Write the token **atomically and durably**: temp file in the same directory
/// → fsync → rename over the target.
///
/// A truncate-in-place write (the previous implementation) loses the session if
/// the machine dies mid-write — laptop sleep/power-off, force quit, OOM kill.
/// The file is left empty or partial, `file_load` filters it out, and the user
/// is silently signed out on the next launch. That window is hit often here
/// because every refresh ROTATES the token: the server revokes the old one the
/// moment it issues the new one, so a lost write means the only valid token is
/// gone for good (and re-presenting the old one trips the backend's reuse
/// detection, which revokes the whole token family).
///
/// `rename` is atomic on POSIX, so a reader sees either the old token or the
/// new one — never a truncated one. The fsync before the rename guarantees the
/// bytes are on disk first; the directory fsync after makes the rename itself
/// durable across a power cut.
fn file_save(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create credentials dir")?;

        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(parent) {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = std::fs::set_permissions(parent, perms);
            }
        }
    }

    // Same directory as the target so the rename stays on one filesystem.
    let tmp = path.with_extension("tmp");

    #[cfg(target_family = "unix")]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .context("open temp credentials file")?;
        f.write_all(token.as_bytes())
            .context("write temp credentials file")?;
        f.sync_all().context("fsync temp credentials file")?;
    }

    #[cfg(not(target_family = "unix"))]
    {
        std::fs::write(&tmp, token).context("write temp credentials file")?;
    }

    // Atomic swap. On failure, clean up so we don't leave a stray temp file.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::from(e).context("rename credentials into place"));
    }

    // Make the rename itself durable (best-effort; not all platforms allow it).
    #[cfg(target_family = "unix")]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load the saved refresh token. Returns `None` if not signed in.
///
/// Load order:
///   1. DRAGENT_REFRESH_TOKEN env var (CI / Docker)
///   2. Platform app data/dragent/credentials (the store)
///   3. Legacy ~/.dragent/token — migrated then deleted
pub fn load_refresh_token() -> Option<String> {
    // 1. Env var override (CI / Docker)
    if let Ok(t) = std::env::var("DRAGENT_REFRESH_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 2. Primary file store — authoritative, always written on save.
    if let Some(path) = credentials_path() {
        if let Some(t) = file_load(&path) {
            return Some(t);
        }
    }

    // 3. Legacy migration: ~/.dragent/token → XDG path + delete old file
    if let Some(legacy) = legacy_token_path() {
        if let Some(t) = file_load(&legacy) {
            if let Some(dest) = credentials_path() {
                match file_save(&dest, &t) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&legacy);
                        log::info!("[auth] migrated token from {:?} to {:?}", legacy, dest);
                    }
                    Err(e) => {
                        warn!("[auth] migration failed, keeping legacy file: {e:#}");
                    }
                }
            }
            return Some(t);
        }
    }

    None
}

/// Whether a sign-in is saved on this computer, looked up in the same places
/// as [`load_refresh_token`] but without migrating or changing anything.
/// For the command line, which must never use or refresh the token itself:
/// every refresh rotates it, which would sign the app out.
pub fn has_saved_session() -> bool {
    std::env::var("DRAGENT_REFRESH_TOKEN").is_ok_and(|token| !token.trim().is_empty())
        || credentials_path().is_some_and(|path| file_load(&path).is_some())
        || legacy_token_path().is_some_and(|path| file_load(&path).is_some())
}

/// Persist the refresh token securely.
///
/// Writes to the owner-only credentials file.
pub fn save_refresh_token(token: &str) -> Result<()> {
    let path = credentials_path().context("could not determine config dir")?;
    file_save(&path, token)?;
    Ok(())
}

/// Remove the stored refresh token from all locations.
pub fn clear_refresh_token() -> Result<()> {
    if let Some(path) = credentials_path() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = legacy_token_path() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Env vars are process-global; tests run in parallel. Every test that
    /// touches DRAGENT_REFRESH_TOKEN must hold this lock or they race each
    /// other (set → other test's remove → load sees nothing → flaky failure).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn env_var_takes_priority() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        env::set_var("DRAGENT_REFRESH_TOKEN", "env-test-token");
        let result = load_refresh_token();
        env::remove_var("DRAGENT_REFRESH_TOKEN");
        assert_eq!(result.as_deref(), Some("env-test-token"));
    }

    #[test]
    fn env_var_whitespace_is_ignored() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        env::set_var("DRAGENT_REFRESH_TOKEN", "   ");
        env::remove_var("DRAGENT_REFRESH_TOKEN");
        // verifies trim + empty check don't panic
    }

    #[test]
    fn credentials_and_legacy_paths_differ() {
        assert_ne!(credentials_path(), legacy_token_path());
    }

    #[test]
    fn file_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials");
        file_save(&path, "my-token").expect("save");
        assert_eq!(file_load(&path).as_deref(), Some("my-token"));
    }

    /// A rewrite must never leave the file empty or partial: the old token has
    /// already been revoked by rotation, so a truncated write costs the session.
    #[test]
    fn file_save_overwrite_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials");

        file_save(&path, "first-token").expect("first save");
        assert_eq!(file_load(&path).as_deref(), Some("first-token"));

        // Overwrite with a shorter value — the truncate-in-place bug would have
        // left trailing bytes of the previous token behind.
        file_save(&path, "second").expect("overwrite");
        assert_eq!(file_load(&path).as_deref(), Some("second"));

        // The temp file must be renamed away, not left lying around.
        assert!(
            !path.with_extension("tmp").exists(),
            "temp credentials file should not survive a successful save"
        );
    }

    #[test]
    fn file_save_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("dragent")
            .join("credentials");
        file_save(&path, "tok").expect("save into missing dir");
        assert_eq!(file_load(&path).as_deref(), Some("tok"));
    }

    #[test]
    fn file_load_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials");
        std::fs::write(&path, "  tok123\n").unwrap();
        assert_eq!(file_load(&path).as_deref(), Some("tok123"));
    }

    #[test]
    fn legacy_migration_moves_file() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(".dragent").join("token");
        let dest = dir
            .path()
            .join("config")
            .join("dragent")
            .join("credentials");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "old-token").unwrap();

        file_save(&dest, "old-token").expect("migrate");
        std::fs::remove_file(&legacy).expect("delete legacy");

        assert!(!legacy.exists(), "legacy file should be gone");
        assert_eq!(file_load(&dest).as_deref(), Some("old-token"));
    }

    #[test]
    fn dev_edition_never_reads_the_release_legacy_token() {
        let home = Path::new("/home/someone");
        assert_eq!(
            legacy_token_path_for(dragent_paths::Edition::Dev, home),
            None
        );
        assert_eq!(
            legacy_token_path_for(dragent_paths::Edition::Release, home),
            Some(home.join(".dragent").join("token"))
        );
    }

    #[test]
    fn a_test_build_never_uses_the_real_credentials_file() {
        let real = dragent_paths::platform_support().join("credentials");
        assert_ne!(credentials_path().as_deref(), Some(real.as_path()));
        assert!(credentials_path()
            .unwrap()
            .starts_with(std::env::temp_dir()));
    }

    #[test]
    fn save_load_round_trip() {
        let token = format!("test-token-{}", uuid::Uuid::new_v4());
        save_refresh_token(&token).expect("save");
        assert_eq!(load_refresh_token().as_deref(), Some(token.as_str()));
        clear_refresh_token().expect("clear");
    }
}
