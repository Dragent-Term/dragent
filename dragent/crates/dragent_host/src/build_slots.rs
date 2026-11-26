//! One pool of compiler slots for every terminal's builds.
//!
//! `cargo` and `make` in any number of terminals take their parallelism from
//! one jobserver instead of each using the whole machine. The terminal
//! service owns it: a named FIFO on Unix, a named semaphore on Windows, at a
//! path (name) that stays the same when the service is replaced.
//!
//! Slots are taken by the builds themselves, so a build that is killed takes
//! its slots with it, and a service replaced mid-build can't know how many
//! are out. The service therefore sets the pool right again whenever nothing
//! on the machine compiles, when nobody can hold a slot.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Folder under the organisation dir holding the pool.
pub const BUILD_SLOTS_DIRNAME: &str = "build-slots";
#[cfg(unix)]
const FIFO_FILENAME: &str = "jobserver";
/// What terminals are told, for as long as the pool exists.
const STATE_FILENAME: &str = "state.json";
/// How often a pool is looked over.
pub const TEND_EVERY: Duration = Duration::from_secs(20);
/// Quiet looks in a row before lost slots are put back.
const QUIET_LOOKS: u32 = 2;

fn slots_dir(org_dir: &Path) -> PathBuf {
    org_dir.join(BUILD_SLOTS_DIRNAME)
}

/// How many slots this machine gets.
pub fn machine_slots() -> usize {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    dragent_resource_policy::build_slots::slot_count(cpus, system.total_memory())
}

/// The pool, open for as long as this lives.
pub struct Pool {
    slots: usize,
    auth: String,
    quiet: u32,
    tokens: tokens::Tokens,
}

impl Pool {
    /// Open the organisation's pool, filled unless builds already use it.
    pub fn open(org_dir: &Path, slots: usize) -> std::io::Result<Self> {
        let slots = slots.max(1);
        let dir = slots_dir(org_dir);
        std::fs::create_dir_all(&dir)?;
        let (tokens, auth) = tokens::Tokens::open(&dir, slots)?;
        let state = serde_json::json!({ "auth": auth, "slots": slots }).to_string();
        crate::agent_sessions::write_atomic(&dir, STATE_FILENAME, state.as_bytes())
            .map_err(std::io::Error::other)?;
        Ok(Self {
            slots,
            auth,
            quiet: 0,
            tokens,
        })
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    /// The value of `--jobserver-auth`.
    pub fn auth(&self) -> &str {
        &self.auth
    }

    /// Put back slots that killed builds took with them, and take out any
    /// there are too many of. `compiling` is only asked when slots are out.
    pub fn tend(&mut self, compiling: impl FnOnce() -> bool) {
        if self.tokens.trim(self.slots) || compiling() {
            self.quiet = 0;
            return;
        }
        self.quiet += 1;
        if self.quiet >= QUIET_LOOKS {
            self.quiet = 0;
            self.tokens.refill(self.slots);
        }
    }
}

/// Whether anything on this machine compiles, and so may hold a slot.
pub fn compiling() -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    let name = |process: &sysinfo::Process| process.name().to_string_lossy().into_owned();
    let processes = system
        .processes()
        .values()
        .filter(|process| process.thread_kind().is_none());
    any_compiles(processes.map(|process| {
        let parent = process.parent().and_then(|pid| system.process(pid));
        let parent = parent.map(|parent| (parent.pid().as_u32(), name(parent)));
        (name(process), parent)
    }))
}

/// Whether any of the processes, each with its parent, may hold a slot.
/// Whatever Cargo runs counts, by whatever name (build scripts have many): it
/// holds a slot only while something of its runs. That counts a test run
/// too, which only puts off returning lost slots, where giving out slots
/// that are still held would undo the limit.
fn any_compiles(processes: impl Iterator<Item = (String, Option<(u32, String)>)>) -> bool {
    let mut make_jobs = std::collections::HashMap::new();
    for (name, parent) in processes {
        if is_compiler(&name) {
            return true;
        }
        let Some((pid, parent)) = parent else {
            continue;
        };
        if program(&parent) == "cargo" {
            return true;
        }
        if is_make(&parent) {
            *make_jobs.entry(pid).or_insert(0) += 1;
        }
    }
    // A make's first job runs on the slot every build has of its own.
    make_jobs.values().any(|jobs| *jobs > 1)
}

fn program(name: &str) -> &str {
    name.strip_suffix(".exe").unwrap_or(name)
}

fn is_make(name: &str) -> bool {
    matches!(program(name), "make" | "gmake" | "mingw32-make" | "ninja")
}

fn is_compiler(name: &str) -> bool {
    let name = program(name);
    matches!(
        name,
        "rustc"
            | "rustdoc"
            | "clippy-driver"
            | "sccache"
            | "cc"
            | "cc1"
            | "cc1plus"
            | "c++"
            | "gcc"
            | "g++"
            | "clang"
            | "clang++"
            | "cl"
            | "link"
            | "ld"
            | "lld"
            | "collect2"
    )
}

/// What terminals of `org_dir` are told, while its pool exists.
pub fn auth(org_dir: &Path) -> Option<String> {
    let raw = std::fs::read(slots_dir(org_dir).join(STATE_FILENAME)).ok()?;
    let state: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let auth = state.get("auth")?.as_str()?;
    if let Some(path) = auth.strip_prefix("fifo:") {
        Path::new(path).exists().then(|| auth.to_string())
    } else {
        cfg!(windows).then(|| auth.to_string())
    }
}

/// Which variables can carry the pool here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakeSupport {
    /// The system's make understands it too.
    Both,
    /// Cargo only: make is absent, or old enough to ignore it.
    CargoOnly,
    /// Neither: GNU make 4.2 and 4.3 stop with an error at a named FIFO, and
    /// Cargo hands the pool on to the make that a build script runs.
    Neither,
}

/// [`MakeSupport`] of the system's make, asked once.
pub fn make_support() -> MakeSupport {
    static SUPPORT: std::sync::OnceLock<MakeSupport> = std::sync::OnceLock::new();
    *SUPPORT.get_or_init(|| {
        let mut command = std::process::Command::new("make");
        command.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let said = command.stdin(std::process::Stdio::null()).output();
        let said = said.map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
        support_of(&said.unwrap_or_default(), cfg!(windows))
    })
}

fn support_of(version: &str, windows: bool) -> MakeSupport {
    let number = version
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GNU Make "))
        .and_then(|rest| {
            let mut parts = rest.trim().split('.');
            let major: u32 = parts.next()?.parse().ok()?;
            let minor: u32 = parts.next()?.parse().ok()?;
            Some((major, minor))
        });
    match number {
        Some(number) if number >= (4, 4) || (windows && number >= (4, 2)) => MakeSupport::Both,
        Some((4, 2 | 3)) => MakeSupport::Neither,
        _ => MakeSupport::CargoOnly,
    }
}

/// Shell prefix telling a terminal's builds about the pool. Values already
/// set are kept.
pub fn prefix(
    kind: dragent_shell_hooks::ShellKind,
    quote: &dyn Fn(&str) -> String,
    auth: Option<&str>,
    support: MakeSupport,
) -> String {
    let keys: &[&str] = match support {
        MakeSupport::Both => &["MAKEFLAGS", "CARGO_MAKEFLAGS"],
        MakeSupport::CargoOnly => &["CARGO_MAKEFLAGS"],
        MakeSupport::Neither => &[],
    };
    // The value is one word of a list: a space would end it.
    let Some(auth) = auth.filter(|auth| !auth.contains(char::is_whitespace)) else {
        return String::new();
    };
    let value = format!("--jobserver-auth={auth}");
    keys.iter()
        .map(|key| crate::build_cache::conditional_default_export(kind, quote, key, &value))
        .collect()
}

#[cfg(unix)]
mod tokens {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{Error, ErrorKind, Read, Result, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    /// The FIFO, open both ways so it keeps its slots and never blocks.
    pub struct Tokens {
        file: File,
        /// Open for reading only: asked through the other, macOS answers for
        /// its writing side, which holds nothing.
        reader: File,
    }

    impl Tokens {
        pub fn open(dir: &Path, slots: usize) -> Result<(Self, String)> {
            let path = fifo_path(dir)?;
            make_fifo(&path)?;
            let file = File::options()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)?;
            let reader = File::options()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)?;
            let tokens = Self { file, reader };
            // What builds that outlived the last service left stays: they
            // give back what they hold. Nobody left, nothing in it.
            if tokens.available() == 0 {
                tokens.refill(slots);
            }
            Ok((tokens, format!("fifo:{}", path.display())))
        }

        pub fn available(&self) -> usize {
            let mut bytes: libc::c_int = 0;
            // SAFETY: asks one open descriptor for one integer.
            let status =
                unsafe { libc::ioctl(self.reader.as_raw_fd(), libc::FIONREAD, &mut bytes) };
            if status == 0 {
                bytes.max(0) as usize
            } else {
                0
            }
        }

        /// Take out what is over `slots`; whether all are in.
        pub fn trim(&self, slots: usize) -> bool {
            let available = self.available();
            if available > slots {
                let mut spare = vec![0; available - slots];
                let _ = (&self.file).read(&mut spare);
            }
            available >= slots
        }

        pub fn refill(&self, slots: usize) {
            let missing = slots.saturating_sub(self.available());
            let _ = (&self.file).write_all(&vec![b'+'; missing]);
        }
    }

    /// In `dir`, or where the path would hold a space (which the value that
    /// carries it can't), in a private folder under the temporary one.
    fn fifo_path(dir: &Path) -> Result<PathBuf> {
        let path = dir.join(super::FIFO_FILENAME);
        if !path.to_string_lossy().contains(char::is_whitespace) {
            return Ok(path);
        }
        // SAFETY: always succeeds.
        let uid = unsafe { libc::getuid() };
        let private = private_dir(&std::env::temp_dir(), uid)?;
        let name = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(dir.as_os_str().as_bytes());
            digest
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        Ok(private.join(name))
    }

    pub(super) fn private_dir(temp: &Path, uid: u32) -> Result<PathBuf> {
        let private = temp.join(format!("dragent-build-slots-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&private) {
            Err(error) if error.kind() != ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        // Someone else's folder by that name is not used.
        let meta = std::fs::symlink_metadata(&private)?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(Error::other(format!("{} isn't private", private.display())));
        }
        Ok(private)
    }

    fn make_fifo(path: &Path) -> Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_fifo() => return Ok(()),
            Ok(_) => std::fs::remove_file(path)?,
            Err(_) => {}
        }
        let name = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: `name` is a valid string for the call.
        if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
            return Err(Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod tokens {
    use std::io::{Error, Result};
    use std::path::Path;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CreateSemaphoreW, ReleaseSemaphore};

    /// The semaphore. Windows drops it with its last handle, so this one
    /// stays open. The handle is kept as a number: it is this process's, not
    /// a thread's.
    pub struct Tokens(isize);

    impl Tokens {
        pub fn open(dir: &Path, slots: usize) -> Result<(Self, String)> {
            let name = {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
                let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
                format!("DragentBuildSlots-{hash}")
            };
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let count = slots as i32;
            // SAFETY: `wide` ends in zero and outlives the call. A semaphore
            // that exists is opened as it is.
            let handle = unsafe { CreateSemaphoreW(std::ptr::null(), count, count, wide.as_ptr()) };
            if handle.is_null() {
                return Err(Error::last_os_error());
            }
            Ok((Self(handle as isize), name))
        }

        /// There are never too many, and how many are out can't be read.
        pub fn trim(&self, _slots: usize) -> bool {
            false
        }

        pub fn refill(&self, slots: usize) {
            for _ in 0..slots {
                // SAFETY: the handle is open; a full semaphore refuses.
                if unsafe { ReleaseSemaphore(self.0 as _, 1, std::ptr::null_mut()) } == 0 {
                    break;
                }
            }
        }
    }

    impl Drop for Tokens {
        fn drop(&mut self) {
            // SAFETY: closed once.
            unsafe { CloseHandle(self.0 as _) };
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod tokens {
    pub struct Tokens;

    impl Tokens {
        pub fn open(_dir: &std::path::Path, _slots: usize) -> std::io::Result<(Self, String)> {
            Err(std::io::ErrorKind::Unsupported.into())
        }

        pub fn trim(&self, _slots: usize) -> bool {
            true
        }

        pub fn refill(&self, _slots: usize) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dragent-slots-{tag}-{}", uuid::Uuid::new_v4()))
    }

    /// A build that takes `count` slots from the pool and is killed.
    #[cfg(unix)]
    fn lose(pool: &Pool, count: usize) {
        use std::io::Read;
        let path = pool.auth().strip_prefix("fifo:").unwrap();
        let mut build = std::fs::File::options()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        build.read_exact(&mut vec![0; count]).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn slots_a_killed_build_took_come_back_once_nothing_compiles() {
        let org = org("lost");
        let mut pool = Pool::open(&org, 3).unwrap();
        assert_eq!(pool.tokens.available(), 3);
        lose(&pool, 2);
        assert_eq!(pool.tokens.available(), 1);
        for _ in 0..5 {
            pool.tend(|| true);
        }
        assert_eq!(pool.tokens.available(), 1, "a build may still hold them");
        pool.tend(|| false);
        assert_eq!(
            pool.tokens.available(),
            1,
            "one quiet look could fall between two compiles"
        );
        pool.tend(|| true);
        pool.tend(|| false);
        assert_eq!(pool.tokens.available(), 1);
        pool.tend(|| false);
        assert_eq!(pool.tokens.available(), 3);
        // Nothing missing: the processes aren't even looked at.
        pool.tend(|| panic!("not asked"));
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn slots_given_back_after_a_refill_do_not_stay_in_excess() {
        use std::io::Write;
        let org = org("excess");
        let mut pool = Pool::open(&org, 2).unwrap();
        (&std::fs::File::options()
            .read(true)
            .write(true)
            .open(pool.auth().strip_prefix("fifo:").unwrap())
            .unwrap())
            .write_all(b"+++")
            .unwrap();
        assert_eq!(pool.tokens.available(), 5);
        pool.tend(|| true);
        assert_eq!(pool.tokens.available(), 2);
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_service_neither_doubles_nor_empties_the_pool() {
        let org = org("replaced");
        let first = Pool::open(&org, 4).unwrap();
        lose(&first, 3);
        // The next one starts while the last still runs and builds go on.
        let second = Pool::open(&org, 4).unwrap();
        assert_eq!(second.auth(), first.auth());
        assert_eq!(second.tokens.available(), 1, "three are out with builds");
        drop(first);
        drop(second);
        // Nobody had it open: the FIFO starts empty, and is filled.
        // (A child another test is just starting can hold it for a moment.)
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while Pool::open(&org, 4).unwrap().tokens.available() != 4 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&org);
    }

    #[test]
    fn terminals_are_told_only_about_a_pool_that_exists() {
        let org = org("auth");
        assert_eq!(auth(&org), None);
        let pool = Pool::open(&org, 2).unwrap();
        assert_eq!(auth(&org).as_deref(), Some(pool.auth()));
        let _ = std::fs::remove_dir_all(&org);
        assert_eq!(auth(&org), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_with_a_space_gets_a_pool_at_one_without() {
        let org = org("a space").join("in it");
        let pool = Pool::open(&org, 2).unwrap();
        let path = pool.auth().strip_prefix("fifo:").unwrap().to_string();
        assert!(!path.contains(' '), "{path}");
        assert_eq!(pool.tokens.available(), 2);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(org.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_folder_by_our_name_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let temp = org("shared");
        let uid = unsafe { libc::getuid() };
        std::fs::create_dir_all(&temp).unwrap();
        let private = tokens::private_dir(&temp, uid).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(tokens::private_dir(&temp, uid).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn a_make_that_would_stop_at_the_pool_never_hears_of_it() {
        let unix = |text| support_of(text, false);
        assert_eq!(unix("GNU Make 4.4.1\nBuilt for x"), MakeSupport::Both);
        assert_eq!(unix("GNU Make 5.0"), MakeSupport::Both);
        assert_eq!(unix("GNU Make 4.3\n"), MakeSupport::Neither);
        assert_eq!(unix("GNU Make 4.2.1"), MakeSupport::Neither);
        assert_eq!(unix("GNU Make 3.81"), MakeSupport::CargoOnly);
        assert_eq!(unix(""), MakeSupport::CargoOnly);
        assert_eq!(support_of("GNU Make 4.3", true), MakeSupport::Both);
        let quote = |s: &str| format!("'{s}'");
        let bash = dragent_shell_hooks::ShellKind::Bash;
        let with = |auth, support| prefix(bash, &quote, auth, support);
        assert!(with(Some("fifo:/x"), MakeSupport::Neither).is_empty());
        let cargo = with(Some("fifo:/x"), MakeSupport::CargoOnly);
        assert!(cargo.contains("export CARGO_MAKEFLAGS='--jobserver-auth=fifo:/x'"));
        assert!(!cargo.contains("export MAKEFLAGS"));
        assert!(with(Some("fifo:/x"), MakeSupport::Both).contains("export MAKEFLAGS"));
        assert!(with(None, MakeSupport::Both).is_empty());
        assert!(with(Some("fifo:/a b"), MakeSupport::Both).is_empty());
    }

    #[test]
    fn whatever_cargo_runs_may_hold_a_slot() {
        let process = |name: &str, parent: Option<(u32, &str)>| {
            (
                name.to_string(),
                parent.map(|(pid, name)| (pid, name.to_string())),
            )
        };
        let compiles = |list: Vec<_>| any_compiles(list.into_iter());
        assert!(!compiles(vec![process("cargo", Some((1, "zsh")))]));
        assert!(compiles(vec![process("a1b2c3", Some((7, "cargo")))]));
        assert!(compiles(vec![process("x", Some((7, "cargo.exe")))]));
        let job = || process("sh", Some((9, "make")));
        assert!(!compiles(vec![job()]), "make's own slot");
        assert!(compiles(vec![job(), job()]));
    }

    #[test]
    fn compilers_are_known_by_name_on_every_system() {
        for name in ["rustc", "rustc.exe", "clang", "link.exe"] {
            assert!(is_compiler(name), "{name}");
        }
        for name in ["cargo", "make", "zsh", "node"] {
            assert!(!is_compiler(name), "{name}");
        }
    }

    /// Two `cargo build` runs with a single-token pool serialise: each
    /// workspace has three members with sleepy build scripts (three parallel
    /// chains), so sharing one token takes ~2× the solo time. (Proves cargo
    /// honours the `fifo:` jobserver it inherits: a single-unit crate would
    /// run on cargo's implicit token without ever touching the pool.)
    #[test]
    fn two_cargo_processes_share_the_pool_tokens() {
        if std::process::Command::new("cargo")
            .arg("--version")
            .output()
            .map(|out| !out.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: no cargo on PATH");
            return;
        }
        let base = std::env::temp_dir().join(format!(
            "dragent-js-cargo-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_dir_all(&base);
        // One-token pool of our own (machine pools are bigger).
        let org = base.join("org");
        let pool = Pool::open(&org, 1).unwrap();
        let auth = pool.auth().to_string();
        // The holder must stay open while the cargos run.
        let _pool = pool;
        let sleep_secs = 3u64;
        for name in ["ws-a", "ws-b"] {
            let root = base.join(name);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(
                root.join("Cargo.toml"),
                format!(
                    "[workspace]\nmembers = [\"m1\", \"m2\", \"m3\"]\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nm1 = {{ path = \"m1\", package = \"m1-{name}\" }}\nm2 = {{ path = \"m2\", package = \"m2-{name}\" }}\nm3 = {{ path = \"m3\", package = \"m3-{name}\" }}\n"
                ),
            )
            .unwrap();
            std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
            for member in ["m1", "m2", "m3"] {
                let dir = root.join(member);
                std::fs::create_dir_all(dir.join("src")).unwrap();
                std::fs::write(
                    dir.join("Cargo.toml"),
                    format!("[package]\nname = \"{member}-{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
                )
                .unwrap();
                std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
                std::fs::write(
                    dir.join("build.rs"),
                    format!(
                        "fn main() {{ std::thread::sleep(std::time::Duration::from_secs({sleep_secs})); }}\n"
                    ),
                )
                .unwrap();
            }
        }
        let run_one = |name: &str, target: &Path| {
            std::process::Command::new("cargo")
                .args(["build", "--offline"])
                .current_dir(base.join(name))
                .env("CARGO_TARGET_DIR", target)
                .env_remove("MAKEFLAGS")
                .env("CARGO_MAKEFLAGS", format!("--jobserver-auth={auth}"))
                .env_remove("CARGO_BUILD_JOBS")
                .output()
        };
        let started = std::time::Instant::now();
        let (a, b) = std::thread::scope(|scope| {
            let ta = scope.spawn(|| run_one("ws-a", &base.join("target-a")));
            let tb = scope.spawn(|| run_one("ws-b", &base.join("target-b")));
            (ta.join().unwrap(), tb.join().unwrap())
        });
        let elapsed = started.elapsed();
        let a = a.expect("cargo a runs");
        let b = b.expect("cargo b runs");
        assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        // Serialised on one token: ~2× the solo build. Parallel would be ~sleep.
        assert!(
            elapsed.as_secs() >= 2 * sleep_secs - 1,
            "two builds took {elapsed:?}: tokens were not shared"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
