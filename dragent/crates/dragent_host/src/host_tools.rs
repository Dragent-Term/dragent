//! Which command-line tools this machine has, and how to install the system
//! ones. Settings → Machines shows the results and runs the installs.
//!
//! Lookups search the PATH the user's own shell sets up, captured fresh for
//! each lookup from an interactive login shell (`-ilc`, like a terminal
//! tab). Tools set up in `~/.bashrc` (nvm, rustup) therefore count, and a
//! tool an installer just added is found without restarting this runtime,
//! whose own environment was fixed when it started.
//!
//! Running a tool ([`command`]) follows the same search when the tool isn't on
//! this process's PATH. An app opened from the Finder or a desktop launcher
//! starts with only the system folders on it, whatever the user's shell sets up.
//! On Windows the PATH is complete but dates from launch, so the registry's
//! current copy stands in for the shell's: an install made while the app is
//! open is found without a restart.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub use crate::development::{Elevation, ToolCopy, ToolPin, ToolQuery, ToolReport, ToolStatus, ToolsReport};

/// Tools the machine page lists under System tools.
pub const SYSTEM_TOOLS: &[&str] = &[
    "git", "rg", "node", "npm", "python3", "go", "cargo", "docker", "gh", "glab",
];

/// The most names one lookup answers.
pub const MAX_LOOKUP: usize = 256;

const SHELL_TIMEOUT: Duration = Duration::from_secs(6);
/// How long [`command`] keeps a captured PATH before asking the shell again.
#[cfg(unix)]
const RUN_DIRS_TTL: Duration = Duration::from_secs(300);
const NPM_TIMEOUT: Duration = Duration::from_secs(5);
const MARK: &str = "__DRAGENT_PATH__";

/// A bare executable name, never a path or shell syntax.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// Whether each name resolves to an executable, in the order given.
pub fn lookup(names: &[String]) -> Vec<(String, bool)> {
    let dirs = search_dirs();
    names
        .iter()
        .take(MAX_LOOKUP)
        .map(|name| {
            let found = valid_name(name) && find_in(name, &dirs).is_some();
            (name.clone(), found)
        })
        .collect()
}

/// Where tools are looked up: the shell's PATH, this process's PATH, then
/// npm's global bin and the folders installers often use without editing a
/// profile.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = shell_and_own_dirs();
    if let Some(npm) = find_in("npm", &dirs) {
        if let Some(bin) = npm_global_bin(&npm) {
            push_dir(&mut dirs, bin);
        }
    }
    push_usual_dirs(&mut dirs);
    dirs
}

fn shell_and_own_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = shell_path() {
        push_paths(&mut dirs, &path);
    }
    if let Some(path) = std::env::var_os("PATH") {
        push_paths(&mut dirs, &path.to_string_lossy());
    }
    dirs
}

/// Homebrew's folders and `~/.local/bin`. They also find a tool when a shell
/// profile hangs or fails and so reports no PATH.
fn push_usual_dirs(dirs: &mut Vec<PathBuf>) {
    #[cfg(unix)]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin"] {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            push_dir(dirs, dir);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let local = home.join(".local/bin");
        if local.is_dir() {
            push_dir(dirs, local);
        }
    }
}

/// The PATH `name` has to run with, when this process's own can't reach it.
/// `None` leaves the child's environment alone, and starts no shell, so a
/// tool that's already reachable costs nothing extra.
fn path_reaching(name: &str) -> Option<std::ffi::OsString> {
    if !valid_name(name) {
        return None;
    }
    let own: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if find_in(name, &own).is_some() {
        return None;
    }
    let dirs = run_dirs();
    find_in(name, &dirs)?;
    std::env::join_paths(dirs).ok()
}

#[cfg(unix)]
fn run_dirs() -> Vec<PathBuf> {
    use std::sync::{Mutex, PoisonError};
    use std::time::Instant;

    static CACHE: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    // Held through the probe, so a page fetching many projects at once starts
    // one shell rather than one each.
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((captured, dirs)) = cache.as_ref() {
        if captured.elapsed() < RUN_DIRS_TTL {
            return dirs.clone();
        }
    }
    let mut dirs = shell_and_own_dirs();
    push_usual_dirs(&mut dirs);
    *cache = Some((Instant::now(), dirs.clone()));
    dirs
}

/// Read every time: it costs little, and a setup screen looks for a tool again
/// right after installing it.
#[cfg(windows)]
fn run_dirs() -> Vec<PathBuf> {
    let mut dirs = shell_and_own_dirs();
    push_usual_dirs(&mut dirs);
    dirs
}

fn push_paths(dirs: &mut Vec<PathBuf>, path: &str) {
    for dir in std::env::split_paths(path) {
        push_dir(dirs, dir);
    }
}

fn push_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_absolute() && !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// The first executable `name` in `dirs`.
pub fn find_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(name)).find(|path| is_executable(path))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    ["", "exe", "cmd", "bat"].iter().any(|ext| {
        let candidate = if ext.is_empty() { path.to_path_buf() } else { path.with_extension(ext) };
        candidate.is_file()
    })
}

/// The user's login shell: `$SHELL`, else their account's shell, else sh.
#[cfg(unix)]
pub fn login_shell() -> PathBuf {
    let account = || {
        // SAFETY: getpwuid returns a pointer into static storage (or null);
        // the shell string is copied out before any other libc call.
        unsafe {
            let entry = libc::getpwuid(libc::getuid());
            if entry.is_null() || (*entry).pw_shell.is_null() {
                return None;
            }
            let shell = std::ffi::CStr::from_ptr((*entry).pw_shell);
            Some(PathBuf::from(shell.to_string_lossy().into_owned()))
        }
    };
    std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(account())
        .find(|shell| shell.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// The user's login shell on non-Unix platforms: the same PowerShell/`SHELL`
/// resolution terminals use, so callers get a shell path that actually exists.
#[cfg(not(unix))]
pub fn login_shell() -> PathBuf {
    PathBuf::from(dragent_pty::resolve_shell(None))
}

/// The PATH an interactive login shell ends up with. `None` when the shell
/// fails, or takes longer than a few seconds (a profile waiting for input).
#[cfg(unix)]
fn shell_path() -> Option<String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;

    let shell = login_shell();
    let fish = shell.file_name().is_some_and(|name| name == "fish");
    // Markers keep profile banners out of the value.
    let script = if fish {
        format!("printf '\\n{MARK}%s{MARK}\\n' (string join : $PATH)")
    } else {
        format!("printf '\\n{MARK}%s{MARK}\\n' \"$PATH\"")
    };
    let mut command = Command::new(&shell);
    command
        .arg("-ilc")
        .arg(script)
        .env("DRAGENT_TOOL_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own session: no controlling terminal to grab, and the whole group
    // can be stopped if a profile hangs.
    // SAFETY: setsid is async-signal-safe and touches no shared state.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;
    let pid = child.id() as libc::pid_t;
    let mut stdout = child.stdout.take()?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Stop at the closing marker: something a profile started in the
        // background may hold the pipe open long after the shell exits.
        let mut output = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    output.extend_from_slice(&chunk[..read]);
                    if parse_marked(&String::from_utf8_lossy(&output)).is_some() {
                        break;
                    }
                }
            }
        }
        let _ = send.send(String::from_utf8_lossy(&output).into_owned());
    });
    let output = receive.recv_timeout(SHELL_TIMEOUT).ok();
    // SAFETY: signals the probe's own process group (setsid made it the
    // leader); the shell has normally exited already.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
    output.as_deref().and_then(parse_marked).filter(|path| !path.is_empty())
}

/// The PATH the registry holds now, which an install made since launch is in.
#[cfg(windows)]
fn shell_path() -> Option<String> {
    dragent_pty::fresh_windows_path()
}

fn parse_marked(output: &str) -> Option<String> {
    let start = output.find(MARK)? + MARK.len();
    let end = output[start..].find(MARK)? + start;
    Some(output[start..end].trim().to_string())
}

/// npm's global bin directory, when it exists.
fn npm_global_bin(npm: &Path) -> Option<PathBuf> {
    let mut child = Command::new(npm)
        .args(["config", "get", "prefix"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < NPM_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || prefix.is_empty() {
        return None;
    }
    let bin = PathBuf::from(prefix).join("bin");
    bin.is_dir().then_some(bin)
}

/// The longest a version check may run. A tool that is still running then
/// (one that ignores `--version` and starts up) still counts as working.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Version checks running at once.
const PROBE_BATCH: usize = 12;

/// Status, version and copies of each tool, and how installs get
/// administrator rights.
pub fn report(queries: &[ToolQuery]) -> ToolsReport {
    let dirs = search_dirs();
    let path_env = std::env::join_paths(&dirs).ok();
    let pins = pins();
    let mut tools = Vec::new();
    for batch in queries[..queries.len().min(MAX_LOOKUP)].chunks(PROBE_BATCH) {
        std::thread::scope(|scope| {
            let running: Vec<_> = batch
                .iter()
                .map(|query| {
                    let (dirs, path_env, pin) = (&dirs, path_env.as_deref(), pins.get(&query.name));
                    scope.spawn(move || tool_report(query, dirs, path_env, pin))
                })
                .collect();
            for (query, handle) in batch.iter().zip(running) {
                tools.push(handle.join().unwrap_or_else(|_| ToolReport {
                    name: query.name.clone(),
                    status: ToolStatus::Missing,
                    version: None,
                    copies: Vec::new(),
                    pin: None,
                    resolved: None,
                    detail: Some("Checking this tool failed.".into()),
                }));
            }
        });
    }
    ToolsReport {
        tools,
        elevation: elevation(&dirs),
    }
}

fn tool_report(
    query: &ToolQuery,
    dirs: &[PathBuf],
    path_env: Option<&std::ffi::OsStr>,
    pin: Option<&ToolPin>,
) -> ToolReport {
    let name = query.name.clone();
    let copies = if valid_name(&name) { all_copies(&name, dirs) } else { Vec::new() };
    let first = copies.first().map(|copy| copy.path.clone());
    let mut detail = None;
    let resolved = match pin {
        Some(ToolPin::Path(path)) if is_executable(Path::new(path)) => Some(path.clone()),
        Some(ToolPin::Path(path)) => {
            detail = Some(format!("The chosen copy {path} is gone, so the first one on the PATH is used."));
            first
        }
        Some(ToolPin::Command(command)) => Some(command.clone()),
        None => first,
    };
    let Some(resolved) = resolved else {
        return ToolReport {
            name,
            status: ToolStatus::Missing,
            version: None,
            copies,
            pin: pin.cloned(),
            resolved: None,
            detail,
        };
    };
    let program = match pin {
        Some(ToolPin::Command(command)) => Program::Shell(command),
        _ => Program::Path(&resolved),
    };
    let (status, version, broken) = check_version(program, &query.version_args, path_env);
    ToolReport {
        name,
        status,
        version,
        copies,
        pin: pin.cloned(),
        resolved: Some(resolved),
        detail: broken.or(detail),
    }
}

/// Every copy of `name` on the search path, one per actual file (on
/// merged-/usr systems /bin/git and /usr/bin/git are one copy).
pub fn all_copies(name: &str, dirs: &[PathBuf]) -> Vec<ToolCopy> {
    let mut copies: Vec<ToolCopy> = Vec::new();
    for dir in dirs {
        let path = dir.join(name);
        if !is_executable(&path) {
            continue;
        }
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let real_path = real.to_string_lossy().into_owned();
        if copies.iter().any(|copy| copy.real_path == real_path) {
            continue;
        }
        copies.push(ToolCopy {
            path: path.to_string_lossy().into_owned(),
            source: source_of(&real_path).to_string(),
            real_path,
        });
    }
    copies
}

/// How a copy was installed, judged from where it really lives.
pub fn source_of(real_path: &str) -> &'static str {
    let path = real_path.to_ascii_lowercase();
    let has = |part: &str| path.contains(part);
    if has("/node_modules/") {
        "npm"
    } else if has("/.nvm/") {
        "nvm"
    } else if has("/.volta/") {
        "Volta"
    } else if has("/.asdf/") {
        "asdf"
    } else if has("/mise/") {
        "mise"
    } else if has("/fnm") {
        "fnm"
    } else if has("/cellar/") || has("/homebrew/") || has("/linuxbrew/") || has("/caskroom/") {
        "Homebrew"
    } else if has("/.rustup/") {
        "rustup"
    } else if has("/.cargo/") {
        "Cargo"
    } else if has("/snap/") {
        "Snap"
    } else if has("/nix/store/") || has("/.nix-profile/") {
        "Nix"
    } else if has("/.local/") {
        "Installer"
    } else if ["/usr/bin/", "/bin/", "/usr/sbin/", "/sbin/", "/usr/lib/", "/usr/libexec/", "/library/developer/"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        "System"
    } else {
        "Manual"
    }
}

#[derive(Clone, Copy)]
enum Program<'a> {
    Path(&'a str),
    /// A pinned command line, run by `sh`.
    Shell(&'a str),
}

type VersionResult = (ToolStatus, Option<String>, Option<String>);

/// Runs the tool to read its version and tell a working copy from a broken
/// one. Results for an unchanged file are reused.
fn check_version(program: Program, args: &[String], path_env: Option<&std::ffi::OsStr>) -> VersionResult {
    static CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, VersionResult>>> =
        std::sync::Mutex::new(None);
    let key = match program {
        Program::Path(path) => std::fs::metadata(path).ok().map(|meta| {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
            format!("{path}\0{}\0{:?}\0{}", args.join(" "), modified, meta.len())
        }),
        Program::Shell(_) => None,
    };
    if let Some(key) = &key {
        if let Some(hit) = CACHE.lock().ok().and_then(|cache| cache.as_ref()?.get(key).cloned()) {
            return hit;
        }
    }
    let result = run_version(program, args, path_env);
    if let Some(key) = key {
        if let Ok(mut cache) = CACHE.lock() {
            let cache = cache.get_or_insert_with(Default::default);
            if cache.len() > 512 {
                cache.clear();
            }
            cache.insert(key, result.clone());
        }
    }
    result
}

fn run_version(program: Program, args: &[String], path_env: Option<&std::ffi::OsStr>) -> VersionResult {
    let mut command = match program {
        Program::Path(path) => {
            let mut command = Command::new(path);
            command.args(args);
            command
        }
        Program::Shell(line) => {
            let mut command = Command::new("sh");
            command.arg("-c").arg(format!("{line} {}", args.join(" ")));
            command
        }
    };
    if let Some(path) = path_env {
        // Scripts like `#!/usr/bin/env node` find their interpreter the way
        // a terminal would.
        command.env("PATH", path);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; the group can then be stopped
        // as a whole on timeout.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return (ToolStatus::Broken, None, Some(format!("It can't be started: {error}"))),
    };
    let pid = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    let Ok(output) = receive.recv_timeout(VERSION_TIMEOUT) else {
        #[cfg(unix)]
        // SAFETY: stops only the check's own process group.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        // It runs; it just doesn't print a version and exit.
        return (ToolStatus::Ready, None, None);
    };
    let Ok(output) = output else {
        return (ToolStatus::Ready, None, None);
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = output.status.signal() {
            return (ToolStatus::Broken, None, Some(format!("It crashed when started (signal {signal}).")));
        }
    }
    // 126: can't execute; 127: its interpreter or a library is missing.
    if matches!(output.status.code(), Some(126 | 127)) {
        let reason = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("it can't start");
        return (ToolStatus::Broken, None, Some(format!("It can't start: {reason}")));
    }
    (ToolStatus::Ready, version_in(&text), None)
}

/// The first version number in a tool's `--version` output: "git version
/// 2.43.0" → 2.43.0, "go1.22.1" → 1.22.1, "v20.11.1" → 20.11.1.
pub fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | '"' | '\'' | '/'))
        .map(|token| token.trim_matches(|c: char| matches!(c, ':' | '.' | '[' | ']')))
        .find_map(|token| {
            let start = token.find(|c: char| c.is_ascii_digit())?;
            let candidate = &token[start..];
            let prefix = &token[..start];
            let dotted = candidate.split(['-', '+']).next()?;
            let parts: Vec<&str> = dotted.split('.').collect();
            let numeric = parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            (numeric && prefix.chars().all(|c| c.is_ascii_alphabetic()) && prefix.len() <= 8)
                .then(|| candidate.to_string())
        })
}

/// How installs can get administrator rights. Checked once per runtime: a
/// refused `sudo` is logged (and on some systems mailed to admins), so it
/// is only tried for users in an administrator group.
fn elevation(dirs: &[PathBuf]) -> Elevation {
    static ONCE: std::sync::OnceLock<Elevation> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no preconditions.
            if unsafe { libc::geteuid() } == 0 {
                return Elevation::Root;
            }
        }
        let Some(sudo) = find_in("sudo", dirs) else {
            return Elevation::Unavailable;
        };
        let groups = Command::new("id")
            .arg("-Gn")
            .stdin(Stdio::null())
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        if !groups.split_whitespace().any(|group| matches!(group, "sudo" | "wheel" | "admin")) {
            return Elevation::Unavailable;
        }
        let quiet = Command::new(sudo)
            .args(["-n", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if quiet.is_ok_and(|status| status.success()) {
            Elevation::PasswordlessSudo
        } else {
            Elevation::SudoWithPassword
        }
    })
}

fn pins_file() -> PathBuf {
    dragent_paths::home().join("tool-pins.json")
}

type Pins = std::collections::BTreeMap<String, ToolPin>;

fn load_pins(file: &Path) -> Pins {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The copies chosen on this machine, re-read when the file changes.
pub fn pins() -> Pins {
    static CACHE: std::sync::Mutex<Option<(Option<std::time::SystemTime>, Pins)>> = std::sync::Mutex::new(None);
    let file = pins_file();
    let modified = std::fs::metadata(&file).and_then(|meta| meta.modified()).ok();
    let Ok(mut cache) = CACHE.lock() else {
        return Pins::new();
    };
    if let Some((seen, pins)) = cache.as_ref() {
        if *seen == modified {
            return pins.clone();
        }
    }
    let pins = load_pins(&file);
    *cache = Some((modified, pins.clone()));
    pins
}

/// Choose (or with `None`, forget) the copy of `name` used on this machine.
pub fn set_pin(name: &str, pin: Option<ToolPin>) -> Result<(), String> {
    write_pin(&pins_file(), name, pin)
}

fn write_pin(file: &Path, name: &str, pin: Option<ToolPin>) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("{name:?} isn't a tool name"));
    }
    match &pin {
        Some(ToolPin::Path(path)) => {
            if !Path::new(path).is_absolute() || !is_executable(Path::new(path)) {
                return Err(format!("{path} isn't an executable file on this machine"));
            }
        }
        Some(ToolPin::Command(command)) => {
            let command = command.trim();
            if command.is_empty() || command.len() > 300 || command.contains(['\n', '\r', '\0']) {
                return Err("Enter the command on one line (up to 300 characters)".into());
            }
        }
        None => {}
    }
    let mut pins = load_pins(file);
    match pin {
        Some(ToolPin::Command(command)) => pins.insert(name.to_string(), ToolPin::Command(command.trim().to_string())),
        Some(pin) => pins.insert(name.to_string(), pin),
        None => pins.remove(name),
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    let json = serde_json::to_vec_pretty(&pins).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())
}

/// The words a chosen copy runs as, when one is chosen for `name`: agents
/// launch with them in place of the bare name.
pub fn launch_words(name: &str) -> Option<Vec<String>> {
    match pins().get(name)? {
        ToolPin::Path(path) => is_executable(Path::new(path)).then(|| vec![path.clone()]),
        ToolPin::Command(command) => {
            let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
            (!words.is_empty()).then_some(words)
        }
    }
}

/// A `Command` for `name` that runs the copy chosen on this machine, like
/// `Command::new(name)` when none is, and finds a tool this process's PATH
/// can't reach. The child gets the wider PATH too, because tools start others
/// (`gh` runs `git`).
pub fn command(name: &str) -> Command {
    let mut command = match launch_words(name) {
        Some(words) => {
            let mut command = Command::new(&words[0]);
            command.args(&words[1..]);
            command
        }
        None => Command::new(name),
    };
    if let Some(path) = command.get_program().to_str().and_then(path_reaching) {
        command.env("PATH", path);
    }
    // The app and host are GUI-subsystem processes with no console to share,
    // so a console child would otherwise pop up its own window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Package names for one tool, per package manager.
struct Packages {
    apt: &'static str,
    dnf: &'static str,
    zypper: &'static str,
    pacman: &'static str,
    apk: &'static str,
    brew: &'static str,
}

fn packages(tool: &str) -> Option<Packages> {
    let same = |name: &'static str| Packages {
        apt: name,
        dnf: name,
        zypper: name,
        pacman: name,
        apk: name,
        brew: name,
    };
    Some(match tool {
        "git" => same("git"),
        "rg" => same("ripgrep"),
        "gh" => same("gh"),
        "glab" => same("glab"),
        "node" | "npm" => Packages {
            apt: "nodejs npm",
            dnf: "nodejs npm",
            zypper: "nodejs npm",
            pacman: "nodejs npm",
            apk: "nodejs npm",
            brew: "node",
        },
        "python3" => Packages {
            pacman: "python",
            brew: "python",
            ..same("python3")
        },
        "go" => Packages {
            apt: "golang-go",
            dnf: "golang",
            brew: "go",
            ..same("go")
        },
        _ => return None,
    })
}

/// What a package-manager command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageAction {
    Install,
    Update,
    Remove,
}

/// Whether the system package manager handles `tool` (Rust goes through
/// rustup, docker through its own guide).
pub fn uses_package_manager(tool: &str) -> bool {
    packages(tool).is_some()
}

/// A command for the user's terminal that runs `action` for these tools
/// with the machine's package manager, merged into one call. It runs under
/// `sh` whatever the user's shell (fish included), asks for `sudo` only when
/// not root, and exits non-zero when it can't.
pub fn package_command(action: PackageAction, tools: &[&str], macos: bool) -> Result<String, String> {
    let mut list: Vec<Packages> = Vec::new();
    for tool in tools {
        list.push(packages(tool).ok_or_else(|| {
            format!("Dragent can't install {tool} with a package manager. Choose git, rg, node, npm, python3, go, gh or glab.")
        })?);
    }
    let names = |field: fn(&Packages) -> &'static str| {
        let mut all: Vec<&str> = Vec::new();
        for packages in &list {
            for name in field(packages).split(' ') {
                if !all.contains(&name) {
                    all.push(name);
                }
            }
        }
        all.join(" ")
    };
    let label = tools.join(", ");
    let script = if macos {
        brew_script(action, &label, &names(|p| p.brew))
    } else {
        linux_script(
            action,
            &label,
            [
                names(|p| p.apt),
                names(|p| p.dnf),
                names(|p| p.zypper),
                names(|p| p.pacman),
                names(|p| p.apk),
                names(|p| p.brew),
            ],
        )
    };
    Ok(format!("sh -c {}", crate::remote::quote(&script)))
}

/// Rust's own installer, which needs no administrator rights.
pub fn rustup_command(action: PackageAction) -> String {
    let script = match action {
        PackageAction::Install => "if command -v curl >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; else echo 'Installing Rust needs curl.' >&2; exit 1; fi",
        PackageAction::Update => "rustup=$(command -v rustup || ls \"$HOME/.cargo/bin/rustup\" 2>/dev/null); if [ -z \"$rustup\" ]; then echo 'This Rust was not installed with rustup; update it the way it was installed.' >&2; exit 1; fi; \"$rustup\" update",
        PackageAction::Remove => "rustup=$(command -v rustup || ls \"$HOME/.cargo/bin/rustup\" 2>/dev/null); if [ -z \"$rustup\" ]; then echo 'This Rust was not installed with rustup; remove it the way it was installed.' >&2; exit 1; fi; \"$rustup\" self uninstall -y",
    };
    format!("sh -c {}", crate::remote::quote(script))
}

/// The install command the runtime offers for `tool` on this machine (the
/// `InstallCommand` request; the desktop builds richer choices itself).
pub fn install_command(tool: &str) -> Result<String, String> {
    match tool {
        "cargo" => Ok(rustup_command(PackageAction::Install)),
        "codex" => Ok(format!("sh -c {}", crate::remote::quote(&npm_global_install("@openai/codex")))),
        "claude" => Ok(format!("sh -c {}", crate::remote::quote("curl -fsSL https://claude.ai/install.sh | bash"))),
        _ => package_command(PackageAction::Install, &[tool], cfg!(target_os = "macos")).map_err(|_| {
            format!("Dragent can't install {tool} automatically. Choose git, rg, node, npm, python3, go, gh, glab or cargo.")
        }),
    }
}

/// A Homebrew command for these packages (`--cask` ones included), for the
/// user's terminal; it says so when Homebrew isn't installed.
pub fn brew_command(action: PackageAction, label: &str, packages: &str) -> String {
    format!("sh -c {}", crate::remote::quote(&brew_script(action, label, packages)))
}

fn brew_script(action: PackageAction, label: &str, packages: &str) -> String {
    let verb = match action {
        PackageAction::Install => "install",
        PackageAction::Update => "upgrade",
        PackageAction::Remove => "uninstall",
    };
    format!(
        "brew=$(command -v brew || ls /opt/homebrew/bin/brew /usr/local/bin/brew 2>/dev/null | head -n 1); \
         if [ -z \"$brew\" ]; then echo 'Managing {label} needs Homebrew, which is not installed on this Mac. Install it from https://brew.sh, then try again.' >&2; exit 1; fi; \
         \"$brew\" {verb} {packages}"
    )
}

/// apt's package lists are refreshed unless that happened in the last ten
/// minutes.
const APT_REFRESH: &str = "if [ -z \"$(find /var/lib/apt/lists -maxdepth 0 -mmin -10 2>/dev/null)\" ]; then $sudo apt-get update || exit 1; fi;";

fn linux_script(action: PackageAction, label: &str, [apt, dnf, zypper, pacman, apk, brew]: [String; 6]) -> String {
    use PackageAction::*;
    let (apt, dnf, yum, zypper, pacman, apk, brew) = match action {
        Install => (
            format!("{APT_REFRESH} $sudo apt-get install -y {apt}"),
            format!("$sudo dnf install -y {dnf}"),
            format!("$sudo yum install -y {dnf}"),
            format!("$sudo zypper --non-interactive install {zypper}"),
            format!("$sudo pacman -S --noconfirm --needed {pacman}"),
            format!("$sudo apk add {apk}"),
            format!("brew install {brew}"),
        ),
        Update => (
            format!("{APT_REFRESH} $sudo apt-get install -y --only-upgrade {apt}"),
            format!("$sudo dnf upgrade -y {dnf}"),
            format!("$sudo yum update -y {dnf}"),
            format!("$sudo zypper --non-interactive update {zypper}"),
            format!("$sudo pacman -S --noconfirm {pacman}"),
            format!("$sudo apk upgrade {apk}"),
            format!("brew upgrade {brew}"),
        ),
        Remove => (
            format!("$sudo apt-get remove -y {apt}"),
            format!("$sudo dnf remove -y {dnf}"),
            format!("$sudo yum remove -y {dnf}"),
            format!("$sudo zypper --non-interactive remove {zypper}"),
            format!("$sudo pacman -R --noconfirm {pacman}"),
            format!("$sudo apk del {apk}"),
            format!("brew uninstall {brew}"),
        ),
    };
    format!(
        "{ELEVATE} \
         if command -v apt-get >/dev/null 2>&1; then {apt}; \
         elif command -v dnf >/dev/null 2>&1; then {dnf}; \
         elif command -v yum >/dev/null 2>&1; then {yum}; \
         elif command -v zypper >/dev/null 2>&1; then {zypper}; \
         elif command -v pacman >/dev/null 2>&1; then {pacman}; \
         elif command -v apk >/dev/null 2>&1; then {apk}; \
         elif command -v brew >/dev/null 2>&1; then {brew}; \
         else echo 'No supported package manager (apt, dnf, yum, zypper, pacman, apk or Homebrew) was found. Manage {label} yourself, then check again.' >&2; exit 1; fi"
    )
}

/// Sets `$sudo` to "" for root, "sudo" otherwise; stops when neither works.
const ELEVATE: &str = "if [ \"$(id -u)\" -eq 0 ]; then sudo=''; \
     elif command -v sudo >/dev/null 2>&1; then sudo=sudo; \
     else echo 'Installing needs administrator rights, and sudo is not available. Ask an administrator to install it.' >&2; exit 1; fi;";

/// `npm install -g`, with `sudo` only when npm's global folder isn't the
/// user's own (a system Node.js); nvm, Homebrew and user prefixes never
/// need it. A prefix npm hasn't created yet is judged by its nearest
/// existing parent.
pub fn npm_global_install(package: &str) -> String {
    npm_global(&format!("install -g {package}"))
}

/// `npm uninstall -g`, with `sudo` under the same rule as installing.
pub fn npm_global_uninstall(package: &str) -> String {
    npm_global(&format!("uninstall -g {package}"))
}

fn npm_global(subcommand: &str) -> String {
    format!(
        "if ! command -v npm >/dev/null 2>&1; then echo 'This needs npm. Install node from System tools first.' >&2; exit 1; fi; \
         target=\"$(npm prefix -g)/lib/node_modules\"; \
         while [ ! -e \"$target\" ]; do target=$(dirname \"$target\"); done; \
         if [ -w \"$target\" ]; then npm {subcommand}; \
         else {ELEVATE} $sudo npm {subcommand}; fi"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_executables() {
        for name in ["git", "cursor-agent", "python3", "g++", "kiro-cli"] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", "-rf", "../bin/sh", "git; touch x", "a b", "$(id)", ".hidden"] {
            assert!(!valid_name(name), "{name}");
        }
    }

    #[test]
    fn marked_output_ignores_profile_noise() {
        let output = format!("Welcome!\nlast login…\n{MARK}/usr/bin:/home/me/.nvm/bin{MARK}\nbye");
        assert_eq!(parse_marked(&output).unwrap(), "/usr/bin:/home/me/.nvm/bin");
        assert_eq!(parse_marked("no markers"), None);
    }

    #[test]
    fn lookup_finds_executables_only() {
        let dir = std::env::temp_dir().join(format!("dragent-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("mytool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let plain = dir.join("notes");
        std::fs::write(&plain, "text").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let dirs = vec![dir.clone()];
        assert_eq!(find_in("mytool", &dirs), Some(tool));
        #[cfg(unix)]
        assert_eq!(find_in("notes", &dirs), None);
        assert_eq!(find_in("missing", &dirs), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_path_is_captured() {
        // This machine's own login shell must report some PATH.
        let path = shell_path().expect("login shell PATH");
        assert!(path.contains('/'), "{path}");
        assert!(lookup(&["sh".into()])[0].1, "sh resolves");
    }

    #[test]
    fn install_commands_run_under_sh_and_fail_loudly() {
        for tool in ["git", "rg", "node", "npm", "python3", "go", "cargo", "codex", "claude"] {
            let command = install_command(tool).unwrap();
            assert!(command.starts_with("sh -c '"), "{tool}: {command}");
        }
        let git = install_command("git").unwrap();
        if cfg!(target_os = "macos") {
            assert!(git.contains("brew") && git.contains("exit 1"), "{git}");
        } else {
            assert!(git.contains("apt-get") && git.contains("$sudo") && git.contains("exit 1"), "{git}");
        }
        assert!(install_command("docker").is_err());
        assert!(install_command("git; touch injected").is_err());
    }

    /// Installs ripgrep for real, then finds it without a restart. Run it
    /// only in a disposable Linux container, e.g. `docker run --rm … cargo
    /// test -p dragent_host --lib -- --ignored installs_for_real`.
    #[cfg(unix)]
    #[test]
    #[ignore = "installs a package; run in a disposable container"]
    fn installs_for_real() {
        let rg = || lookup(&["rg".to_string()])[0].1;
        let run = |command: String| {
            let status = Command::new("sh").arg("-c").arg(&command).status().unwrap();
            assert!(status.success(), "{command}");
        };
        let macos = cfg!(target_os = "macos");
        assert!(!rg(), "the container starts without rg");
        run(install_command("rg").unwrap());
        assert!(rg(), "rg is found right after installing");
        let report = report(&[ToolQuery { name: "rg".into(), version_args: vec!["--version".into()] }]);
        assert_eq!(report.tools[0].status, ToolStatus::Ready);
        assert!(report.tools[0].version.is_some(), "{:?}", report.tools[0]);
        // Update (apt's lists are fresh, so no second refresh), then remove.
        run(package_command(PackageAction::Update, &["rg"], macos).unwrap());
        assert!(rg(), "still there after updating");
        run(package_command(PackageAction::Remove, &["rg"], macos).unwrap());
        assert!(!rg(), "gone after removing");
    }

    /// `npm install -g` for a system Node.js (sudo) or a user-owned one (no
    /// sudo), then finds the package's command. Container-only, like
    /// [`installs_for_real`]; needs npm, and sudo for a non-root user.
    #[cfg(unix)]
    #[test]
    #[ignore = "installs a package; run in a disposable container"]
    fn npm_installs_for_real() {
        let found = || lookup(&["cowsay".to_string()])[0].1;
        assert!(!found(), "the container starts without cowsay");
        let command = format!("sh -c {}", crate::remote::quote(&npm_global_install("cowsay")));
        let status = Command::new("sh").arg("-c").arg(&command).status().unwrap();
        assert!(status.success(), "{command}");
        assert!(found(), "cowsay is found right after installing");
    }

    #[test]
    fn versions_are_read_from_typical_output() {
        let cases = [
            ("git version 2.43.0", "2.43.0"),
            ("go version go1.22.1 linux/arm64", "1.22.1"),
            ("v20.11.1", "20.11.1"),
            ("2.0.14 (Claude Code)", "2.0.14"),
            ("Docker version 27.0.3, build 7d4bcd8", "27.0.3"),
            ("Python 3.12.3", "3.12.3"),
            ("cargo 1.92.0 (ded5c06cf 2025-12-08)", "1.92.0"),
            ("codex-cli 0.46.0", "0.46.0"),
            ("ripgrep 14.1.0

features:+pcre2", "14.1.0"),
            ("tool 1.2.3-beta.1", "1.2.3-beta.1"),
        ];
        for (output, version) in cases {
            assert_eq!(version_in(output).as_deref(), Some(version), "{output}");
        }
        assert_eq!(version_in("usage: tool [options]"), None);
        assert_eq!(version_in("built 2025-12-08"), None);
    }

    #[test]
    fn sources_are_judged_from_install_locations() {
        let cases = [
            ("/home/me/.nvm/versions/node/v20/lib/node_modules/@openai/codex/bin/codex.js", "npm"),
            ("/home/me/.nvm/versions/node/v20.11.1/bin/node", "nvm"),
            ("/opt/homebrew/Cellar/ripgrep/14.1.0/bin/rg", "Homebrew"),
            ("/home/me/.cargo/bin/cargo", "Cargo"),
            ("/home/me/.local/share/claude/versions/2.0.14", "Installer"),
            ("/usr/bin/git", "System"),
            ("/snap/bin/go", "Snap"),
            ("/opt/tools/bin/thing", "Manual"),
        ];
        for (path, source) in cases {
            assert_eq!(source_of(path), source, "{path}");
        }
    }

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn reports_tell_working_broken_and_missing_apart() {
        let root = std::env::temp_dir().join(format!("dragent-report-{}", std::process::id()));
        let (first, second) = (root.join("first"), root.join("second"));
        executable(&first, "good", "#!/bin/sh\necho 'good version 1.4.2'\n");
        executable(&second, "good", "#!/bin/sh\necho 'good version 9.9.9'\n");
        // The same file reached twice (a symlinked bin dir) is one copy.
        std::os::unix::fs::symlink(&first, root.join("alias")).unwrap();
        executable(&first, "nointerp", "#!/no/such/interpreter\n");
        executable(&first, "missinglib", "#!/bin/sh\necho 'error while loading shared libraries' >&2\nexit 127\n");
        executable(&first, "noflag", "#!/bin/sh\necho 'unknown option --version' >&2\nexit 2\n");
        let dirs = vec![first.clone(), root.join("alias"), second.clone()];
        let query = |name: &str| ToolQuery { name: name.into(), version_args: vec!["--version".into()] };

        let good = tool_report(&query("good"), &dirs, None, None);
        assert_eq!(good.status, ToolStatus::Ready);
        assert_eq!(good.version.as_deref(), Some("1.4.2"));
        assert_eq!(good.copies.len(), 2, "{:?}", good.copies);
        assert_eq!(good.resolved.as_deref(), Some(first.join("good").to_str().unwrap()));

        // Choosing the second copy runs that one.
        let pinned = ToolPin::Path(second.join("good").to_string_lossy().into_owned());
        let chosen = tool_report(&query("good"), &dirs, None, Some(&pinned));
        assert_eq!(chosen.version.as_deref(), Some("9.9.9"));
        // A chosen command runs through sh.
        let command = ToolPin::Command(format!("{} --quiet", second.join("good").display()));
        assert_eq!(tool_report(&query("good"), &dirs, None, Some(&command)).version.as_deref(), Some("9.9.9"));
        // A chosen copy that's gone falls back, and says so.
        let gone = ToolPin::Path(root.join("gone/good").to_string_lossy().into_owned());
        let fallback = tool_report(&query("good"), &dirs, None, Some(&gone));
        assert_eq!(fallback.version.as_deref(), Some("1.4.2"));
        assert!(fallback.detail.unwrap().contains("is gone"));

        for broken in ["nointerp", "missinglib"] {
            let report = tool_report(&query(broken), &dirs, None, None);
            assert_eq!(report.status, ToolStatus::Broken, "{broken}");
            assert!(report.detail.is_some(), "{broken}");
        }
        // Rejecting --version doesn't make a tool broken.
        assert_eq!(tool_report(&query("noflag"), &dirs, None, None).status, ToolStatus::Ready);
        assert_eq!(tool_report(&query("absent"), &dirs, None, None).status, ToolStatus::Missing);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pins_are_saved_and_validated() {
        let root = std::env::temp_dir().join(format!("dragent-pins-{}", std::process::id()));
        let tool = executable(&root.join("bin"), "claude", "#!/bin/sh\n");
        let file = root.join("tool-pins.json");
        let path = tool.to_string_lossy().into_owned();
        write_pin(&file, "claude", Some(ToolPin::Path(path.clone()))).unwrap();
        write_pin(&file, "codex", Some(ToolPin::Command("  npx -y @openai/codex ".into()))).unwrap();
        let pins = load_pins(&file);
        assert_eq!(pins.get("claude"), Some(&ToolPin::Path(path)));
        assert_eq!(pins.get("codex"), Some(&ToolPin::Command("npx -y @openai/codex".into())));
        write_pin(&file, "claude", None).unwrap();
        assert!(!load_pins(&file).contains_key("claude"));
        assert!(write_pin(&file, "git", Some(ToolPin::Path("relative/git".into()))).is_err());
        assert!(write_pin(&file, "git", Some(ToolPin::Path(root.join("none").to_string_lossy().into()))).is_err());
        assert!(write_pin(&file, "git", Some(ToolPin::Command("two\nlines".into()))).is_err());
        assert!(write_pin(&file, "../git", None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// How installs get administrator rights, against what the container
    /// was set up with: `DRAGENT_EXPECT_ELEVATION=root|passwordless|password|none`.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs a prepared account; run in a disposable container"]
    fn elevation_matches_the_account() {
        let expected = match std::env::var("DRAGENT_EXPECT_ELEVATION").as_deref() {
            Ok("root") => Elevation::Root,
            Ok("passwordless") => Elevation::PasswordlessSudo,
            Ok("password") => Elevation::SudoWithPassword,
            Ok("none") => Elevation::Unavailable,
            other => panic!("set DRAGENT_EXPECT_ELEVATION, not {other:?}"),
        };
        assert_eq!(report(&[]).elevation, expected);
    }

    #[cfg(unix)]
    #[test]
    fn this_machine_reports_git() {
        let report = report(&[
            ToolQuery { name: "git".into(), version_args: vec!["--version".into()] },
            ToolQuery { name: "dragent-missing-tool".into(), version_args: vec![] },
        ]);
        assert_eq!(report.tools[0].status, ToolStatus::Ready);
        assert!(report.tools[0].version.is_some(), "{:?}", report.tools[0]);
        assert_eq!(report.tools[1].status, ToolStatus::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn install_scripts_are_valid_sh() {
        let mut commands: Vec<String> = ["git", "rg", "node", "python3", "go", "cargo", "codex"]
            .iter()
            .map(|tool| install_command(tool).unwrap())
            .collect();
        for action in [PackageAction::Install, PackageAction::Update, PackageAction::Remove] {
            for macos in [false, true] {
                commands.push(package_command(action, &["git", "rg", "node", "npm"], macos).unwrap());
            }
            commands.push(rustup_command(action));
        }
        commands.push(format!("sh -c {}", crate::remote::quote(&npm_global_uninstall("cowsay"))));
        for command in commands {
            // `sh -n` parses without running anything.
            let script = command.strip_prefix("sh -c ").unwrap();
            let status = Command::new("sh")
                .arg("-c")
                .arg(format!("sh -n -c {script}"))
                .status()
                .unwrap();
            assert!(status.success(), "{command}");
        }
    }
}
