//! The two storage roots used by Dragent, and which edition this build is.
//!
//! User and workspace state belongs under `~/.dragent` (`~/.dragent-dev` for
//! a Dev edition build, see [`edition`]). Only credentials, downloaded
//! models, language servers, and MCP tokens use platform storage.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use sha2::{Digest, Sha256};

/// Which kind of build this is.
///
/// Release builds come only from the release workflow, which compiles with
/// `DRAGENT_RELEASE_EDITION=1`. Every other build, debug or `--release`, ours
/// or a contributor's, is the Dev edition: its own data folder, background
/// services, Keychain entries and link scheme, so running it never touches
/// the installed Dragent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Release,
    Dev,
}

const fn edition_from(flag: Option<&str>) -> Edition {
    match flag {
        Some(value) if value.len() == 1 && value.as_bytes()[0] == b'1' => Edition::Release,
        _ => Edition::Dev,
    }
}

/// Fixed at compile time; Cargo rebuilds when the variable changes.
const EDITION: Edition = edition_from(option_env!("DRAGENT_RELEASE_EDITION"));

pub fn edition() -> Edition {
    EDITION
}

pub fn is_dev() -> bool {
    EDITION == Edition::Dev
}

/// Keychain service for the app's own secrets (provider keys, E2EE keys,
/// MCP and integration tokens).
pub fn keychain_service() -> &'static str {
    match EDITION {
        Edition::Release => "com.dragent.app",
        Edition::Dev => "com.dragent.app.dev",
    }
}

/// Keychain service for remote machine (SSH) credentials.
pub fn remote_keychain_service() -> &'static str {
    match EDITION {
        Edition::Release => "com.dragent.remote",
        Edition::Dev => "com.dragent.remote.dev",
    }
}

/// The scheme of the app's own links (`dragent://…`).
pub fn url_scheme() -> &'static str {
    match EDITION {
        Edition::Release => "dragent",
        Edition::Dev => "dragent-dev",
    }
}

/// The name people see for this build.
pub fn app_name() -> &'static str {
    match EDITION {
        Edition::Release => "Dragent",
        Edition::Dev => "Dragent Dev",
    }
}

/// Overrides the Dev edition's data folder.
pub const DEV_HOME_ENV: &str = "DRAGENT_DEV_HOME";

/// The edition's data folder.
///
/// Release: `DRAGENT_HOME`, else `~/.dragent`.
///
/// Dev: `DRAGENT_DEV_HOME` unless it is the release folder or inside it,
/// else `DRAGENT_HOME` unless it is the release folder or inside it, else
/// `~/.dragent-dev`. A dev build started from a terminal of the installed app
/// inherits that app's `DRAGENT_HOME`; using it would run the dev build on
/// the installed app's data and services.
///
/// Test binaries (either edition) never use the person's real folders: a
/// result inside `~/.dragent` or `~/.dragent-dev` becomes a throwaway folder
/// for the test process (see [`test_home`]). Folders a test points the
/// variables at are honoured.
///
/// Not cached: tests point `DRAGENT_HOME` at a fresh folder at runtime.
pub fn home() -> PathBuf {
    let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = match EDITION {
        Edition::Release => var("DRAGENT_HOME").unwrap_or_else(|| user_home.join(".dragent")),
        Edition::Dev => dev_home(var(DEV_HOME_ENV), var("DRAGENT_HOME"), &user_home),
    };
    match test_home() {
        Some(test_home) if is_real_data_folder(&home, &user_home) => test_home.to_path_buf(),
        _ => home,
    }
}

/// [`home`] for the Dev edition, over explicit inputs.
fn dev_home(dev_home: Option<PathBuf>, dragent_home: Option<PathBuf>, user_home: &Path) -> PathBuf {
    let release = user_home.join(".dragent");
    for (name, path) in [(DEV_HOME_ENV, dev_home), ("DRAGENT_HOME", dragent_home)] {
        let Some(path) = path else { continue };
        if !is_within(&path, &release) {
            return path;
        }
        log_ignored_home(name, &path);
    }
    user_home.join(".dragent-dev")
}

fn log_ignored_home(name: &str, path: &Path) {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        log::info!(
            "Dragent Dev: ignoring {name}={} (inside the installed app's data folder); \
             set {DEV_HOME_ENV} to a folder outside it to choose this build's folder",
            path.display()
        );
    });
}

/// Whether `path` is `user_home`'s `.dragent` or `.dragent-dev` (or inside one).
fn is_real_data_folder(path: &Path, user_home: &Path) -> bool {
    is_within(path, &user_home.join(".dragent")) || is_within(path, &user_home.join(".dragent-dev"))
}

/// The throwaway data folder used when this process is a Rust test binary,
/// so `cargo test` in any crate never writes the person's data folders.
///
/// Test binaries are detected by where Cargo puts them
/// (`target/<profile>/deps/`); the app's own binaries sit one level up. The
/// folder is per process, under the system temp folder.
pub fn test_home() -> Option<&'static Path> {
    static TEST_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    TEST_HOME
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            if !is_test_binary_path(&exe) {
                return None;
            }
            let dir =
                std::env::temp_dir().join(format!("dragent-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            Some(dir)
        })
        .as_deref()
}

/// Whether `exe` is where Cargo puts test binaries (`.../deps/<name>`).
fn is_test_binary_path(exe: &Path) -> bool {
    exe.parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == "deps")
}

/// Whether `path` is `base` or inside it.
///
/// Both are resolved first ([`resolve`]: `.`/`..` segments, trailing
/// slashes, and symlinks of the parts that exist), then compared component
/// by component, ignoring case on macOS and Windows (their default file
/// systems do).
fn is_within(path: &Path, base: &Path) -> bool {
    fn parts(path: &Path) -> Vec<String> {
        resolve(path)
            .components()
            .map(|c| {
                let text = c.as_os_str().to_string_lossy();
                if cfg!(any(windows, target_os = "macos")) {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect()
    }
    let (path, base) = (parts(path), parts(base));
    path.len() >= base.len() && path[..base.len()] == base[..]
}

/// `path` with symlinks resolved: canonicalised when it exists, otherwise
/// normalised lexically and its longest existing prefix canonicalised.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let path = normalise(path);
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            return rest.iter().rev().fold(canonical, |p, name| p.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// Remove `.` segments and apply `..` lexically (`..` at the root stays at
/// the root).
fn normalise(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let at_root = matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                );
                if at_root {
                    continue;
                }
                if matches!(
                    out.components().next_back(),
                    None | Some(Component::ParentDir)
                ) {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Variables a PTY daemon hands its successor during a session handoff. A
/// process started from a Dragent terminal can inherit them; only the PTY
/// daemon role may act on them.
pub const PTY_HANDOFF_VARS: [&str; 5] = [
    "DRAGENT_PTY_LISTENER_FD",
    "DRAGENT_PTY_LISTENER_SOCKET",
    "DRAGENT_PTY_HANDOFF_SNAPSHOT",
    "DRAGENT_PTY_HANDOFF_FD",
    "DRAGENT_PTY_TOKEN",
];

/// Variables naming the Dragent terminal a process runs in. The `dragent`
/// command and agent wrappers read them; the app and its background
/// services must not carry another terminal's identity.
pub const HOST_TERMINAL_VARS: [&str; 2] = ["DRAGENT_HOST_WORKSPACE_ID", "DRAGENT_HOST_TERMINAL_ID"];

/// The names in `present` that belong to any of `groups` (pure; see
/// [`strip_inherited_vars`]). Windows variable names ignore case.
pub fn inherited_vars_in<'a>(
    present: impl IntoIterator<Item = &'a str>,
    groups: &[&[&str]],
) -> Vec<&'a str> {
    let matches = |name: &str| {
        groups.iter().flat_map(|group| group.iter()).any(|known| {
            if cfg!(windows) {
                known.eq_ignore_ascii_case(name)
            } else {
                *known == name
            }
        })
    };
    present.into_iter().filter(|name| matches(name)).collect()
}

/// Remove inherited variables of `groups` from this process's environment.
///
/// Call at the very start of `main`, before any thread exists: changing the
/// environment while another thread reads it is undefined behaviour on Unix.
pub fn strip_inherited_vars(groups: &[&[&str]]) {
    let present: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    for name in inherited_vars_in(present.iter().map(String::as_str), groups) {
        std::env::remove_var(name);
    }
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
            args(&[
                "--dragent-agent-run",
                "pane",
                "--",
                "claude",
                "--dragent-organisation=user-arg"
            ])
        );
        assert_eq!(organisation_arg(), "--dragent-organisation=Acme!");
        select_organisation("local");
        assert_eq!(organisation_id(), "local");
        select_organisation("  ");
        assert_eq!(organisation_id(), "local");
    }

    #[test]
    fn exact_ids_have_distinct_private_namespaces() {
        // `home()` is read twice per assertion: hold the environment still.
        let _env = super::edition_tests::env_lock();
        assert_eq!(organisation_dir_for("local"), home());
        for (a, b) in [("Acme", "acme"), ("Acme!", "Acme?"), ("../Acme", "Acme")] {
            assert_ne!(organisation_dir_for(a), organisation_dir_for(b));
            assert!(organisation_dir_for(a).starts_with(home().join("organisations")));
        }
        assert_eq!(organisation_key("Acme"), organisation_key("Acme"));
    }
}

#[cfg(test)]
mod edition_tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Serialises tests that change or depend on process-wide variables.
    pub(super) fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Set (or clear) variables for one test, restoring them afterwards.
    fn with_env<T>(vars: &[(&str, Option<&Path>)], f: impl FnOnce() -> T) -> T {
        let _env = env_lock();
        let saved: Vec<_> = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let result = f();
        for (name, value) in saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        result
    }

    fn user_home() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }

    #[test]
    fn release_flag_is_exactly_one() {
        assert_eq!(edition_from(Some("1")), Edition::Release);
        for other in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("11"),
            Some(" 1"),
        ] {
            assert_eq!(edition_from(other), Edition::Dev, "{other:?}");
        }
    }

    #[test]
    fn tests_build_the_dev_edition() {
        // Test builds never get DRAGENT_RELEASE_EDITION: if this fails, the
        // guard tests below are not checking the edition people develop with.
        assert_eq!(edition(), Edition::Dev);
        assert!(is_dev());
    }

    #[test]
    fn dev_edition_never_uses_release_identity() {
        assert_ne!(keychain_service(), "com.dragent.app");
        assert_ne!(remote_keychain_service(), "com.dragent.remote");
        assert_ne!(url_scheme(), "dragent");
        assert_ne!(app_name(), "Dragent");
        let release = user_home().join(".dragent");
        for inherited in [release.clone(), release.join("organisations/x")] {
            let home = with_env(
                &[(DEV_HOME_ENV, None), ("DRAGENT_HOME", Some(&inherited))],
                home,
            );
            assert!(!is_within(&home, &release), "{}", home.display());
        }
        let home = with_env(&[(DEV_HOME_ENV, None), ("DRAGENT_HOME", None)], home);
        assert!(!is_within(&home, &release), "{}", home.display());
    }

    #[test]
    fn dev_home_ignores_the_release_home() {
        let user = Path::new("/Users/someone");
        let ignored = [
            "/Users/someone/.dragent",
            "/Users/someone/.dragent/",
            "/Users/someone/./.dragent",
            "/Users/someone/.dragent/organisations/abc",
        ];
        for inherited in ignored {
            assert_eq!(
                dev_home(None, Some(PathBuf::from(inherited)), user),
                user.join(".dragent-dev"),
                "{inherited}"
            );
        }
    }

    #[test]
    fn dev_home_honours_other_folders_and_the_dev_override() {
        let user = Path::new("/Users/someone");
        for kept in [
            "/tmp/test-home",
            "/Users/someone/.dragent-dev",
            "/Users/someone/.dragentx",
        ] {
            assert_eq!(
                dev_home(None, Some(PathBuf::from(kept)), user),
                PathBuf::from(kept)
            );
        }
        assert_eq!(
            dev_home(
                Some(PathBuf::from("/tmp/dev")),
                Some(PathBuf::from("/tmp/other")),
                user
            ),
            PathBuf::from("/tmp/dev")
        );
        assert_eq!(dev_home(None, None, user), user.join(".dragent-dev"));
    }

    #[test]
    fn home_reads_the_environment() {
        let release = user_home().join(".dragent");
        let dev_default = user_home().join(".dragent-dev");
        let test_home = test_home()
            .expect("tests run from a test binary")
            .to_path_buf();
        let elsewhere = std::env::temp_dir().join("dragent-paths-test-home");
        let dev = std::env::temp_dir().join("dragent-paths-test-dev-home");
        let cases: [(Option<&Path>, Option<&Path>, PathBuf); 6] = [
            (None, Some(&release), test_home.clone()),
            (None, Some(&elsewhere), elsewhere.clone()),
            (Some(&dev), Some(&elsewhere), dev.clone()),
            (None, None, test_home.clone()),
            // Even a deliberate override never reaches the real folders.
            (Some(&dev_default), None, test_home.clone()),
            (Some(&release), Some(&elsewhere), elsewhere.clone()),
        ];
        for (dev_var, home_var, expected) in cases {
            let got = with_env(&[(DEV_HOME_ENV, dev_var), ("DRAGENT_HOME", home_var)], home);
            assert_eq!(got, expected, "{dev_var:?} {home_var:?}");
        }
    }

    #[test]
    fn test_binaries_get_their_own_folder() {
        let test_home = test_home().expect("tests run from a test binary");
        assert!(test_home.is_dir());
        assert!(!is_real_data_folder(test_home, &user_home()));
        assert!(is_test_binary_path(Path::new(
            "/r/target/debug/deps/dragent_paths-1a2b"
        )));
        assert!(is_test_binary_path(Path::new(
            r"C:/r/target/release/deps/app-1a2b.exe"
        )));
        assert!(!is_test_binary_path(Path::new("/r/target/debug/dragent")));
        assert!(!is_test_binary_path(Path::new(
            "/Applications/Dragent.app/Contents/MacOS/Dragent"
        )));
    }

    #[test]
    fn dev_home_resolves_dots_before_comparing() {
        let user = Path::new("/Users/someone");
        for inherited in [
            "/Users/someone/x/../.dragent",
            "/Users/someone/x/y/../../.dragent/orgs",
            "/Users/someone/.dragent/a/..",
        ] {
            assert_eq!(
                dev_home(None, Some(PathBuf::from(inherited)), user),
                user.join(".dragent-dev"),
                "{inherited}"
            );
        }
        assert_eq!(
            dev_home(
                None,
                Some(PathBuf::from("/Users/someone/.dragent/../other")),
                user
            ),
            PathBuf::from("/Users/someone/.dragent/../other")
        );
    }

    #[test]
    fn dev_home_ignores_case_where_the_file_system_does() {
        let user = Path::new("/Users/someone");
        let got = dev_home(None, Some(PathBuf::from("/Users/someone/.DRAGENT")), user);
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(got, user.join(".dragent-dev"));
        } else {
            assert_eq!(got, PathBuf::from("/Users/someone/.DRAGENT"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn dev_home_resolves_symlinks_to_the_release_home() {
        let dir =
            std::env::temp_dir().join(format!("dragent-paths-symlink-{}", std::process::id()));
        let user = dir.join("user");
        std::fs::create_dir_all(user.join(".dragent/organisations")).unwrap();
        let alias = dir.join("alias");
        let _ = std::fs::remove_file(&alias);
        std::os::unix::fs::symlink(user.join(".dragent"), &alias).unwrap();
        for inherited in [
            alias.clone(),
            alias.join("organisations"),
            alias.join("new/child"),
        ] {
            assert_eq!(
                dev_home(None, Some(inherited.clone()), &user),
                user.join(".dragent-dev"),
                "{}",
                inherited.display()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dev_home_override_inside_the_release_home_is_refused() {
        let user = Path::new("/Users/someone");
        for refused in ["/Users/someone/.dragent", "/Users/someone/.dragent/dev"] {
            assert_eq!(
                dev_home(Some(PathBuf::from(refused)), None, user),
                user.join(".dragent-dev"),
                "{refused}"
            );
            // A usable DRAGENT_HOME still applies after the refused override.
            assert_eq!(
                dev_home(
                    Some(PathBuf::from(refused)),
                    Some(PathBuf::from("/tmp/h")),
                    user
                ),
                PathBuf::from("/tmp/h")
            );
        }
    }

    #[test]
    fn inherited_vars_are_picked_by_group() {
        let present = [
            "PATH",
            "DRAGENT_PTY_LISTENER_FD",
            "DRAGENT_PTY_TOKEN",
            "DRAGENT_HOST_TERMINAL_ID",
            "DRAGENT_HOME",
        ];
        assert_eq!(
            inherited_vars_in(present, &[&PTY_HANDOFF_VARS]),
            ["DRAGENT_PTY_LISTENER_FD", "DRAGENT_PTY_TOKEN"]
        );
        assert_eq!(
            inherited_vars_in(present, &[&PTY_HANDOFF_VARS, &HOST_TERMINAL_VARS]),
            [
                "DRAGENT_PTY_LISTENER_FD",
                "DRAGENT_PTY_TOKEN",
                "DRAGENT_HOST_TERMINAL_ID"
            ]
        );
        assert!(inherited_vars_in(present, &[]).is_empty());
    }

    #[test]
    fn platform_support_is_scoped_by_edition() {
        let base = Path::new("/Users/someone/Library/Application Support");
        assert_eq!(
            platform_support_in(base, Edition::Release),
            base.join("dragent")
        );
        assert_eq!(
            platform_support_in(base, Edition::Dev),
            base.join("dragent-dev")
        );
        // Tests are debug builds, so Dev: the live folder is the dev one.
        assert_eq!(
            platform_support().file_name().and_then(|n| n.to_str()),
            Some("dragent-dev")
        );
    }

    #[test]
    fn strip_removes_only_the_named_groups() {
        let _env = env_lock();
        std::env::set_var("DRAGENT_PTY_HANDOFF_SNAPSHOT", "/x");
        std::env::set_var("DRAGENT_HOST_WORKSPACE_ID", "w");
        strip_inherited_vars(&[&PTY_HANDOFF_VARS]);
        assert!(std::env::var_os("DRAGENT_PTY_HANDOFF_SNAPSHOT").is_none());
        assert_eq!(
            std::env::var("DRAGENT_HOST_WORKSPACE_ID").as_deref(),
            Ok("w")
        );
        strip_inherited_vars(&[&HOST_TERMINAL_VARS]);
        assert!(std::env::var_os("DRAGENT_HOST_WORKSPACE_ID").is_none());
    }
}

/// The edition's platform storage folder (credentials, MCP tokens, models,
/// language servers): `<data-local>/dragent` for Release,
/// `<data-local>/dragent-dev` for Dev, so a dev build never reads, rotates
/// or deletes the installed app's sign-in token or other secrets.
pub fn platform_support() -> PathBuf {
    platform_support_in(
        &dirs::data_local_dir()
            .or_else(dirs::config_dir)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))),
        EDITION,
    )
}

/// [`platform_support`] under an explicit platform folder and edition.
fn platform_support_in(base: &Path, edition: Edition) -> PathBuf {
    base.join(match edition {
        Edition::Release => "dragent",
        Edition::Dev => "dragent-dev",
    })
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
    // The legacy folders belong to the installed (Release) app: a Dev build
    // must never move them into its own data folder.
    if is_dev() || std::env::var_os("DRAGENT_HOME").is_some() {
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
