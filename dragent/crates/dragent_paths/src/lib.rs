//! The two storage roots used by Dragent.
//!
//! User and workspace state belongs under `~/.dragent`. Only credentials,
//! downloaded models, language servers, and MCP tokens use platform storage.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use sha2::{Digest, Sha256};

pub fn home() -> PathBuf {
    if let Some(path) = std::env::var_os("DRAGENT_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dragent")
}

/// The organisation whose local data this process reads and writes.
///
/// The desktop app selects it from the signed-in user's active organisation
/// (the server-side user record), and services it starts receive it as an
/// explicit [`ORGANISATION_ARG`]. Nothing reads it from the environment: an
/// inherited variable would silently bind a child to whichever organisation
/// happened to be active when its parent started.
static SELECTED_ORGANISATION: RwLock<Option<String>> = RwLock::new(None);

/// Bumped on every selection, so per-selection caches can tell a return to an
/// organisation apart from never having left it.
static SELECTION: AtomicU64 = AtomicU64::new(0);

/// Command-line flag carrying a service's organisation (`--dragent-organisation=<id>`).
pub const ORGANISATION_ARG: &str = "--dragent-organisation";

/// Selected organisation; `local` until one is selected (signed out, or a
/// service started without [`ORGANISATION_ARG`]).
pub fn organisation_id() -> String {
    SELECTED_ORGANISATION
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(local_organisation_id)
}

/// Select the organisation for the rest of this process. The caller owns the
/// transition: flush writers and detach from the previous organisation's host
/// before switching, since paths resolve against the new selection at once.
pub fn select_organisation(id: &str) {
    let id = id.trim();
    let value = (!id.is_empty() && id != "local").then(|| id.to_string());
    *SELECTED_ORGANISATION
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
    SELECTION.fetch_add(1, Ordering::AcqRel);
}

/// Number of organisation selections made by this process so far.
pub fn organisation_selection() -> u64 {
    SELECTION.load(Ordering::Acquire)
}

pub fn local_organisation_id() -> String {
    "local".into()
}

/// The argument that hands the selected organisation to a child service.
pub fn organisation_arg() -> String {
    format!("{ORGANISATION_ARG}={}", organisation_id())
}

/// Apply and remove an [`ORGANISATION_ARG`] from a service's arguments. Only
/// arguments before a `--` separator are inspected: everything after it
/// belongs to the wrapped command.
pub fn take_organisation_arg(args: Vec<String>) -> Vec<String> {
    let prefix = format!("{ORGANISATION_ARG}=");
    let mut rest = Vec::with_capacity(args.len());
    let mut separated = false;
    for arg in args {
        if !separated {
            if arg == "--" {
                separated = true;
            } else if let Some(id) = arg.strip_prefix(&prefix) {
                select_organisation(id);
                continue;
            }
        }
        rest.push(arg);
    }
    rest
}

/// Service entry points call this first so every path resolves against the
/// organisation their parent selected.
pub fn select_organisation_from_process_args() {
    take_organisation_arg(std::env::args().skip(1).collect());
}

/// Opaque IDs are case sensitive. Never use a lossy slug as an ownership key.
pub fn organisation_key(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn organisation_dir_for(id: &str) -> PathBuf {
    if id == "local" {
        // Existing data belongs to local; retain all live daemon paths.
        home()
    } else {
        home().join("organisations").join(organisation_key(id))
    }
}

pub fn organisation_dir() -> PathBuf {
    organisation_dir_for(&organisation_id())
}

#[cfg(test)]
mod organisation_tests {
    use super::*;

    #[test]
    fn organisation_arg_selects_and_is_removed_only_before_separator() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let rest = take_organisation_arg(args(&[
            "--dragent-agent-run",
            "pane",
            "--dragent-organisation=Acme!",
            "--",
            "claude",
            "--dragent-organisation=user-arg",
        ]));
        assert_eq!(organisation_id(), "Acme!");
        assert_eq!(
            rest,
            args(&["--dragent-agent-run", "pane", "--", "claude", "--dragent-organisation=user-arg"])
        );
        assert_eq!(organisation_arg(), "--dragent-organisation=Acme!");
        select_organisation("local");
        assert_eq!(organisation_id(), "local");
        select_organisation("  ");
        assert_eq!(organisation_id(), "local");
    }

    #[test]
    fn exact_ids_have_distinct_private_namespaces() {
        assert_eq!(organisation_dir_for("local"), home());
        for (a, b) in [("Acme", "acme"), ("Acme!", "Acme?"), ("../Acme", "Acme")] {
            assert_ne!(organisation_dir_for(a), organisation_dir_for(b));
            assert!(organisation_dir_for(a).starts_with(home().join("organisations")));
        }
        assert_eq!(organisation_key("Acme"), organisation_key("Acme"));
    }
}

pub fn platform_support() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("dragent")
}

/// A stable private directory for data associated with a project. The project
/// itself is never modified to create or store Dragent runtime state.
pub fn workspace(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let id = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    home().join("workspaces").join(id)
}

/// Delete one of Dragent's own data directories (never a user's project).
/// On Windows a file another process is still closing refuses deletion
/// briefly, so removal is retried for a few seconds. Already gone is success.
pub fn remove_data_dir(dir: &Path) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut delay = std::time::Duration::from_millis(100);
    loop {
        match fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) if cfg!(windows) && std::time::Instant::now() < deadline => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(std::time::Duration::from_secs(1));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Serialize migrations started by the desktop app and its helper services.
pub fn with_migration_lock<T>(f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let root = home();
    fs::create_dir_all(&root)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(root.join(".migration.lock"))?;
    lock.lock()?;
    let result = f();
    let unlock_result = lock.unlock();
    result.and_then(|value| unlock_result.map(|()| value))
}

/// Move an existing tree without replacing a newer file already at its
/// destination. A same-volume rename is preferred; cross-volume moves copy
/// each file before deleting the source. Safe to retry after interruption.
pub fn move_preserving_existing(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if !destination.exists() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(source, destination).is_ok() {
            return Ok(());
        }
    }
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            move_preserving_existing(&entry.path(), &destination.join(entry.file_name()))?;
        }
        let _ = fs::remove_dir(source);
    } else if !destination.exists() {
        fs::copy(source, destination)?;
        fs::remove_file(source)?;
    }
    Ok(())
}

/// Keep a conflicting legacy file in private storage instead of silently
/// deleting it or leaving runtime state in its old location.
pub fn preserve_conflict(source: &Path, backup: &Path) -> io::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for index in 0..1000 {
        let candidate = if index == 0 {
            backup.to_path_buf()
        } else {
            backup.with_file_name(format!(
                "{}-{index}",
                backup.file_name().unwrap_or_default().to_string_lossy()
            ))
        };
        if !candidate.exists() {
            return move_preserving_existing(source, &candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("too many migration conflicts for {}", source.display()),
    ))
}

/// One-time, restart-safe relocation from the old platform config/data/cache
/// folders. The four platform-owned categories stay there. This runs before
/// the desktop app opens any databases or starts its background services.
pub fn migrate_legacy_storage() -> io::Result<()> {
    if std::env::var_os("DRAGENT_HOME").is_some() {
        return Ok(());
    }
    with_migration_lock(migrate_legacy_storage_inner)
}

fn migrate_legacy_storage_inner() -> io::Result<()> {
    let target = home();
    let platform = platform_support();
    let config = dirs::config_dir().map(|dir| dir.join("dragent"));
    let data = dirs::data_local_dir().map(|dir| dir.join("dragent"));

    // Consolidate the four platform-owned categories first. Older builds
    // split them between config and data-local on Linux and Windows.
    for source in [&config, &data].into_iter().flatten() {
        if *source != platform {
            for name in ["credentials", "models", "language-servers", "mcp-tokens"] {
                let old = source.join(name);
                move_preserving_existing(&old, &platform.join(name))?;
                let backup = if name == "credentials" {
                    platform.join("credentials.legacy")
                } else {
                    platform.join(name).join("legacy-conflicts")
                };
                preserve_conflict(&old, &backup)?;
            }
        }
    }
    let mut sources = vec![platform.clone()];
    for source in [config, data].into_iter().flatten() {
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    for (index, source) in sources.into_iter().enumerate() {
        if !source.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&source)? {
            let entry = entry?;
            let name = entry.file_name();
            if [
                "credentials",
                "credentials.legacy",
                "models",
                "language-servers",
                "mcp-tokens",
            ]
            .iter()
            .any(|reserved| name == *reserved)
            {
                continue;
            }
            let destination = if source == platform && name == "updates" {
                target.join("cache").join(&name)
            } else {
                target.join(&name)
            };
            move_preserving_existing(&entry.path(), &destination)?;
            if entry.path().exists() {
                preserve_conflict(
                    &entry.path(),
                    &target
                        .join("legacy-conflicts")
                        .join(format!("platform-{index}"))
                        .join(entry.file_name()),
                )?;
            }
        }
    }
    if let Some(cache) = dirs::cache_dir() {
        let old_cache = cache.join("dragent");
        if old_cache != platform {
            move_preserving_existing(&old_cache, &target.join("cache"))?;
            preserve_conflict(&old_cache, &target.join("legacy-conflicts/cache"))?;
        }
    }
    Ok(())
}
