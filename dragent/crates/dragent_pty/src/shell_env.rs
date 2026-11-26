use std::path::Path;
#[cfg(not(windows))]
use std::process::Command;

use portable_pty::CommandBuilder;

/// Shells that support a login (`-l`) invocation to load the user's profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    Zsh,
    Bash,
    Other,
}

impl ShellFamily {
    pub fn from_program(program: &str) -> Self {
        let name = Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(program)
            .to_ascii_lowercase();
        // On Windows the shell is e.g. "bash.exe" / "zsh.exe" — strip the suffix
        // so it's still recognized. Otherwise it falls to `Other`, gets no login
        // `-l` flag, never sources /etc/profile, and PATH lacks /usr/bin →
        // "bash: ls: command not found" (exactly the MSYS/Git-Bash symptom).
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "zsh" => Self::Zsh,
            "bash" => Self::Bash,
            _ => Self::Other,
        }
    }
}

/// Resolve the interactive shell to spawn.
///
/// GUI `.app` launches on macOS often omit `SHELL` and ship a minimal `PATH`.
/// Fall back to the account's login shell (from Directory Service) before `/bin/sh`.
pub fn resolve_shell(preferred: Option<String>) -> String {
    if let Some(shell) = preferred.filter(|s| !s.is_empty()) {
        if Path::new(&shell).is_file() {
            return shell;
        }
        eprintln!("dragent: preferred shell not found: {shell}");
    }

    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() && Path::new(&shell).is_file() {
            return shell;
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(shell) = macos_user_shell() {
        if Path::new(&shell).is_file() {
            return shell;
        }
    }

    // Git Bash is the Windows default; PowerShell only when it isn't installed.
    #[cfg(windows)]
    if let Some(bash) = windows_git_bash_paths().into_iter().next() {
        return bash;
    }

    for candidate in default_shell_candidates() {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }

    #[cfg(windows)]
    {
        // Prefer a full path so the spawn never depends on PATH resolution.
        if let Some(ps) = windows_powershell_path() {
            return ps;
        }
        return "powershell.exe".to_string();
    }

    #[cfg(not(windows))]
    {
        "/bin/sh".to_string()
    }
}

/// Full path to the built-in Windows PowerShell 5.1, resolved from `%SystemRoot%`.
#[cfg(windows)]
fn windows_powershell_path() -> Option<String> {
    let root = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("windir"))
        .ok()?;
    let full = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    Path::new(&full).is_file().then_some(full)
}

/// Installed Git Bash launchers, machine-wide installs before a per-user one.
/// `bin\bash.exe` (not `usr\bin\bash.exe`) is the wrapper that sets up the
/// MSYS environment, so `/usr/bin` tools are on PATH.
#[cfg(windows)]
fn windows_git_bash_paths() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (var, suffix) in [
        ("ProgramFiles", "Git\\bin\\bash.exe"),
        ("ProgramW6432", "Git\\bin\\bash.exe"),
        ("ProgramFiles(x86)", "Git\\bin\\bash.exe"),
        ("LOCALAPPDATA", "Programs\\Git\\bin\\bash.exe"),
    ] {
        let Ok(base) = std::env::var(var) else {
            continue;
        };
        let full = format!("{}\\{suffix}", base.trim_end_matches('\\'));
        if Path::new(&full).is_file() && !out.iter().any(|p| p.eq_ignore_ascii_case(&full)) {
            out.push(full);
        }
    }
    out
}

/// A shell the user can launch a new tab with (populates the "+" new-tab menu).
#[derive(Debug, Clone)]
pub struct ShellChoice {
    /// Human-readable name, e.g. "Windows PowerShell", "Git Bash", "zsh".
    pub label: String,
    /// Full path to the shell executable, passed to `resolve_shell` as preferred.
    pub path: String,
}

/// The shells actually installed on this machine, detected once and cached.
pub fn available_shells() -> &'static [ShellChoice] {
    use std::sync::OnceLock;
    static SHELLS: OnceLock<Vec<ShellChoice>> = OnceLock::new();
    SHELLS.get_or_init(detect_shells)
}

fn detect_shells() -> Vec<ShellChoice> {
    let mut out: Vec<ShellChoice> = Vec::new();
    let push = |out: &mut Vec<ShellChoice>, label: &str, path: String| {
        if Path::new(&path).is_file() && !out.iter().any(|s| s.path == path) {
            out.push(ShellChoice {
                label: label.to_string(),
                path,
            });
        }
    };

    // Only offer shells that dragent can integrate with (they emit the OSC 7979
    // preexec/precmd hooks that drive command blocks). cmd.exe, WSL, and POSIX
    // `sh` have no such integration, so a tab running them would collapse into a
    // single never-completing block — we deliberately do NOT list them.
    #[cfg(windows)]
    {
        // Git Bash first: it's the default `resolve_shell` picks.
        for bash in windows_git_bash_paths() {
            push(&mut out, "Git Bash", bash);
        }
        if let Some(ps) = windows_powershell_path() {
            push(&mut out, "Windows PowerShell", ps);
        }
        push(
            &mut out,
            "PowerShell 7",
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe".to_string(),
        );
        push(
            &mut out,
            "PowerShell 7",
            "C:\\Program Files\\PowerShell\\7-preview\\pwsh.exe".to_string(),
        );
    }

    #[cfg(unix)]
    {
        // The user's login shell first, if it's one we integrate with.
        if let Ok(sh) = std::env::var("SHELL") {
            match ShellFamily::from_program(&sh) {
                ShellFamily::Zsh => push(&mut out, "zsh", sh),
                ShellFamily::Bash => push(&mut out, "bash", sh),
                // POSIX sh / anything unrecognized has no integration — skip.
                ShellFamily::Other => {}
            }
        }
        for (path, label) in [
            ("/bin/zsh", "zsh"),
            ("/usr/bin/zsh", "zsh"),
            ("/bin/bash", "bash"),
            ("/usr/bin/bash", "bash"),
            ("/opt/homebrew/bin/bash", "bash (Homebrew)"),
            ("/usr/local/bin/bash", "bash (Homebrew)"),
            ("/opt/homebrew/bin/fish", "fish"),
            ("/usr/local/bin/fish", "fish"),
            ("/usr/bin/fish", "fish"),
            ("/opt/homebrew/bin/nu", "nushell"),
        ] {
            push(&mut out, label, path.to_string());
        }
    }

    out
}

fn default_shell_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        // Full paths ONLY. Bare names like "powershell.exe"/"cmd.exe" would be
        // `is_file()`-checked relative to the CWD — so a launch with CWD set to
        // C:\Windows\System32 (taskbar / jump-list / Explorer) spuriously matches
        // the cmd.exe that lives there, and the user gets cmd instead of a real
        // shell. Windows PowerShell 5.1 is covered by `windows_powershell_path()`.
        &["C:\\Program Files\\PowerShell\\7\\pwsh.exe"]
    } else {
        &["/bin/zsh", "/bin/bash", "/bin/sh"]
    }
}

/// Configure a PTY child so it inherits the same tool paths as Terminal.app.
pub fn configure_shell_command(cmd: &mut CommandBuilder, shell: &str) {
    apply_shell_environment(cmd, shell);
}

/// Configure a std::process::Command so it inherits the same tool paths as Terminal.app.
// `shell` is unix-only now: `login_environment_for_shell` below is the sole
// user of it, and that call is `#[cfg(not(windows))]`.
#[cfg_attr(windows, allow(unused_variables))]
pub fn configure_std_shell_command(cmd: &mut std::process::Command, shell: &str) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "dragent");

    #[cfg(windows)]
    {
        cmd.env("DRAGENT_CONPTY", "1");
        if let Some(path) = fresh_windows_path() {
            cmd.env("PATH", path);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            cmd.env("HOME", home);
        }
    }

    // `needs_login_environment_patch`'s marker list (Homebrew, nvm/fnm, cargo/bin
    // in Unix path form) can never match a Windows-style `PATH`, so it's always
    // true there — meaning `login_environment_for_shell`'s `shell -l -c '...'`
    // probe always ran, inheriting THIS (possibly stale) process's own env and
    // then overwriting the `PATH` set above with whatever it found. Verified via
    // a live PTY spawn: without this gate, a freshly-installed tool (Node) that
    // `fresh_windows_path()` correctly found was invisible in the spawned shell
    // because this probe clobbered it a few lines later. `fresh_windows_path()`
    // is Windows' real fix for what this mechanism exists for on macOS (a GUI
    // `.app` launch shipping a stripped PATH); there's nothing left for it to do
    // here, so skip it — and skip spawning an extra subprocess on every PTY open.
    #[cfg(not(windows))]
    if needs_login_environment_patch() {
        for (key, value) in login_environment_for_shell(shell) {
            cmd.env(key, value);
        }
    }
}

/// Whether to ask Claude Code for its full-screen renderer. Claude otherwise
/// picks between that and an inline one per version and account, and inline
/// it paints inside a command block instead of taking over the pane the way
/// Dragent's agent panes expect. Either variable already set is the user's
/// own choice and wins.
fn claude_full_screen(is_set: impl Fn(&str) -> bool) -> bool {
    !is_set("CLAUDE_CODE_NO_FLICKER") && !is_set("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN")
}

/// Add `-l` so zsh/bash load the user's login profile (`.zprofile`, etc.).
pub fn apply_login_shell_arg(cmd: &mut CommandBuilder, shell: &str) {
    if ShellFamily::from_program(shell) != ShellFamily::Other {
        cmd.arg("-l");
    }
}

/// Colour switches that describe the environment this process was started in,
/// not the terminal it is opening. The daemon can be started by a `dragent`
/// command inside an agent's tool shell, which sets `NO_COLOR=1` for its plain
/// output: inherited, every agent in every later terminal drew without colour.
/// Someone who wants these set exports them in their shell profile, which the
/// new shell still reads.
const INHERITED_COLOR_SWITCHES: [&str; 4] =
    ["NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE", "FORCE_COLOR"];

#[cfg_attr(windows, allow(unused_variables))]
fn apply_shell_environment(cmd: &mut CommandBuilder, shell: &str) {
    for key in INHERITED_COLOR_SWITCHES {
        cmd.env_remove(key);
    }
    // A hand-off successor daemon carries its predecessor's listener, token
    // and snapshot path in its environment; shells must not inherit them.
    for key in dragent_paths::PTY_HANDOFF_VARS {
        cmd.env_remove(key);
    }
    // A test's services close when the test process ends; a shell (and
    // anything started from it) must never inherit that.
    cmd.env_remove(crate::EXIT_WITH_PID_ENV);
    cmd.env("TERM", "xterm-256color");
    // We paint 24-bit colour. Without this, apps round their colours to the
    // 256-colour table: Claude Code's #f8f8f2 text arrives as pure white.
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "dragent");
    if claude_full_screen(|key| std::env::var_os(key).is_some()) {
        cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
    }

    // Tell the shell-integration hooks they run under Windows ConPTY. ConPTY is
    // a screen buffer that re-emits the ENTIRE visible viewport whenever it
    // repaints (scrolling once the screen is full, resizes) — including prompts
    // and output from PREVIOUS commands — into whatever block is capturing. The
    // hooks use this flag to clear that buffer at `preexec`, so the buffer only
    // ever holds the current command and repaints become harmless. The clear is
    // a buffer mutation (ordered with rendered output), unlike OSC reports
    // which ConPTY forwards early — see the drain/fence logic in
    // terminal_session_core. Never set on Unix PTYs (clean ordered streams).
    #[cfg(windows)]
    {
        cmd.env("DRAGENT_CONPTY", "1");
        if let Some(path) = fresh_windows_path() {
            cmd.env("PATH", path);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            cmd.env("HOME", home);
        }
    }

    // See the matching comment in `configure_std_shell_command`: this probe
    // always ran on Windows (its Homebrew/nvm/cargo-in-Unix-form markers never
    // match a Windows PATH) and its result overwrote `fresh_windows_path()`
    // above with a probe that inherits this process's own possibly-stale env —
    // silently discarding a freshly-installed tool. Unix-only now.
    #[cfg(not(windows))]
    if needs_login_environment_patch() {
        for (key, value) in login_environment_for_shell(shell) {
            cmd.env(key, value);
        }
    }
}

/// The User + System `PATH` freshly read from the registry, merged with
/// whatever this (dragent.exe's own) process already inherited.
///
/// A process's environment is a snapshot taken at launch. Installers (Node,
/// Python, anything using an MSI/registry-based installer) update the User or
/// System `PATH` in the registry and broadcast `WM_SETTINGCHANGE`, but an
/// ALREADY-RUNNING process — including dragent.exe itself if it was started
/// before the install — never picks that broadcast up; it keeps the PATH it
/// was born with, and so does every child it spawns. A brand new standalone
/// terminal window doesn't have this problem because its process is created
/// fresh, reading the CURRENT registry values. Rather than requiring a full
/// app restart every time the user installs a CLI tool, read the registry
/// PATH fresh on every new shell spawn and use that instead of trusting
/// dragent.exe's own (possibly stale) inherited copy.
///
/// Registry entries are placed FIRST so a newly-installed tool is found even
/// if an older copy happens to sit later in the app's own inherited PATH;
/// the app's own PATH is appended after (deduped, case-insensitively — Windows
/// paths are case-insensitive) so anything already relying on it keeps working.
#[cfg(windows)]
pub fn fresh_windows_path() -> Option<String> {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

    let user = CURRENT_USER
        .open("Environment")
        .ok()
        .and_then(|k| k.get_string("Path").ok())
        .map(|s| expand_env_strings(&s));
    let system = LOCAL_MACHINE
        .open(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
        .ok()
        .and_then(|k| k.get_string("Path").ok())
        .map(|s| expand_env_strings(&s));

    if user.is_none() && system.is_none() {
        return None;
    }

    let inherited = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut parts: Vec<&str> = Vec::new();
    for p in system
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .chain(user.as_deref().unwrap_or_default().split(';'))
        .chain(inherited.split(';'))
    {
        if p.is_empty() {
            continue;
        }
        // Windows paths are case-insensitive; a trailing slash is cosmetic.
        let key = p.trim_end_matches('\\').to_ascii_lowercase();
        if seen.insert(key) {
            parts.push(p);
        }
    }
    Some(parts.join(";"))
}

/// Expand `%VAR%` references. `RegGetValueW` (which `windows_registry` wraps)
/// returns `REG_EXPAND_SZ` values LITERAL — e.g. the System PATH comes back as
/// `%SystemRoot%\system32;...` — so this must run before the value is usable.
#[cfg(windows)]
fn expand_env_strings(s: &str) -> String {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

    let src: Vec<u16> = std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Two-call pattern: first call (null buffer) returns the required size.
    let needed = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return s.to_string();
    }
    let mut buf = vec![0u16; needed as usize];
    let written = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), needed) };
    if written == 0 {
        return s.to_string();
    }
    // `written` includes the null terminator; trim it before decoding.
    let os = std::ffi::OsString::from_wide(&buf[..(written as usize).saturating_sub(1)]);
    os.to_string_lossy().into_owned()
}

/// True when the parent process has the stripped environment typical of Finder launches.
#[cfg(not(windows))]
fn needs_login_environment_patch() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    if path.is_empty() {
        return true;
    }

    // Homebrew, nvm/fnm shims, and user-local bins are usually missing from GUI PATH.
    let markers = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        ".nvm",
        ".fnm",
        ".volta",
        ".cargo/bin",
    ];
    !markers.iter().any(|marker| path.contains(marker))
}

#[cfg(not(windows))]
fn login_environment_for_shell(shell: &str) -> Vec<(String, String)> {
    let family = ShellFamily::from_program(shell);
    if family == ShellFamily::Other {
        return Vec::new();
    }

    let script = r#"PATH="$PATH" command printf '%s\0%s\0' "$PATH" "$HOME""#;
    let mut probe = Command::new(shell);
    probe.args(["-l", "-c", script]);
    // The app is a GUI-subsystem binary with no console for children to
    // inherit; without CREATE_NO_WINDOW this probe (bash.exe under Git Bash)
    // flashes a console window every time a session spawns.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        probe.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = match probe.output() {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            eprintln!(
                "dragent: login shell env probe failed (status {:?})",
                output.status.code()
            );
            return Vec::new();
        }
        Err(err) => {
            eprintln!("dragent: login shell env probe failed: {err:#}");
            return Vec::new();
        }
    };

    let mut parts = output.split(|b| *b == 0).filter(|chunk| !chunk.is_empty());
    let mut vars = Vec::new();
    if let Some(path) = parts.next() {
        if let Ok(path) = std::str::from_utf8(path) {
            if !path.is_empty() {
                vars.push(("PATH".to_string(), path.to_string()));
            }
        }
    }
    if let Some(home) = parts.next() {
        if let Ok(home) = std::str::from_utf8(home) {
            if !home.is_empty() {
                vars.push(("HOME".to_string(), home.to_string()));
            }
        }
    }
    vars
}

#[cfg(target_os = "macos")]
fn macos_user_shell() -> Option<String> {
    let user = std::env::var("USER").ok().filter(|name| !name.is_empty())?;
    let output = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{user}"), "UserShell"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("UserShell: ")
                .map(str::trim)
                .map(str::to_owned)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shell_families() {
        assert_eq!(ShellFamily::from_program("/bin/zsh"), ShellFamily::Zsh);
        assert_eq!(
            ShellFamily::from_program("/opt/homebrew/bin/bash"),
            ShellFamily::Bash
        );
        assert_eq!(ShellFamily::from_program("/bin/sh"), ShellFamily::Other);
        // Windows: bash.exe must still be recognized (so it gets the login `-l`).
        assert_eq!(ShellFamily::from_program("bash.exe"), ShellFamily::Bash);
        assert_eq!(ShellFamily::from_program("ZSH.EXE"), ShellFamily::Zsh);
    }

    #[test]
    fn claude_full_screen_unless_the_user_chose() {
        assert!(claude_full_screen(|_| false));
        assert!(!claude_full_screen(
            |key| key == "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"
        ));
        assert!(!claude_full_screen(|key| key == "CLAUDE_CODE_NO_FLICKER"));
    }

    #[cfg(not(windows))]
    #[test]
    fn resolve_shell_prefers_existing_preferred_value() {
        // A shell this machine has that isn't $SHELL, so picking it can only
        // come from the preference (Linux runners have no zsh).
        let current = std::env::var("SHELL").unwrap_or_default();
        let preferred = ["/bin/zsh", "/bin/bash", "/bin/sh"]
            .into_iter()
            .find(|shell| Path::new(shell).is_file() && *shell != current)
            .expect("a second shell");
        assert_eq!(resolve_shell(Some(preferred.to_string())), preferred);
    }

    // The login-environment probe only exists off Windows.
    #[cfg(not(windows))]
    #[test]
    fn login_environment_probe_returns_path_for_zsh() {
        let shell = resolve_shell(None);
        let family = ShellFamily::from_program(&shell);
        if family == ShellFamily::Other {
            return;
        }
        let vars = login_environment_for_shell(&shell);
        let path = vars.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v);
        assert!(path.is_some(), "expected PATH from login shell");
        assert!(!path.unwrap().is_empty());
    }
}
