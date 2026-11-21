//! Dragent shell integration.
//!
//! Generates small bootstrap scripts for zsh, bash, fish, PowerShell, and
//! nushell that report the shell's command lifecycle over a private OSC escape
//! channel (`OSC 7979`):
//!
//! - `preexec` — fired just before a submitted command runs, carrying the
//!   command text.
//! - `precmd` — fired when the shell returns to the prompt, carrying the exit
//!   code of the previous command and the current working directory.
//! - `clear` — fired after the screen is cleared (`clear`, ctrl-L).
//!
//! The emulator side (`dragent_terminal`) intercepts these sequences, so they
//! never appear in visible output. This gives the UI exact block boundaries
//! and real exit codes instead of prompt-text heuristics.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;

/// Shells we know how to bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
    Nu,
    /// POSIX sh or anything we don't recognize: no hooks available.
    Other,
}

impl ShellKind {
    /// Detect from a shell program path (e.g. `$SHELL`).
    pub fn from_program(program: &str) -> Self {
        let name = std::path::Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(program)
            .to_ascii_lowercase();
        // Strip a Windows ".exe" so "bash.exe"/"zsh.exe"/"pwsh.exe" are recognized
        // (otherwise bash.exe → Other → no shell-integration hooks on Windows).
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "zsh" => Self::Zsh,
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            "pwsh" | "powershell" => Self::PowerShell,
            "nu" => Self::Nu,
            _ => Self::Other,
        }
    }
}

// ── Zsh ───────────────────────────────────────────────────────────────────────

const ZSH_BOOTSTRAP: &str = r#"# Dragent shell integration (zsh). Generated file - safe to delete.
if [[ -n "${DRAGENT_HOOKS_ACTIVE:-}" ]]; then
  return 0
fi
DRAGENT_HOOKS_ACTIVE=1

__dragent_report() {
  printf '\033]7979;%s\033\\' "$1"
}

__dragent_on_precmd() {
  local __dragent_exit=$?
  __dragent_report "precmd;${__dragent_exit};${PWD}"
  # Windows ConPTY only: paint the rendered drain fence — one prompt cell with
  # the sentinel background (RGB 1,2,3) the emulator recognizes. ConPTY
  # re-emits cell content strictly in order (unlike OSC/DECSET, which it
  # forwards ahead of pending repaints), so this cell marks the true end of
  # the previous command's output.
  [[ -n "${DRAGENT_CONPTY:-}" ]] && printf '\033[48;2;1;2;3m \033[49m'
}

__dragent_on_preexec() {
  __dragent_report "preexec;$1"
  # Windows ConPTY only: empty the console buffer (screen + scrollback) before
  # the command runs. ConPTY re-emits the whole viewport on scroll/resize
  # repaints; with the buffer cleared, a repaint can only re-emit THIS
  # command's output, never earlier prompts/commands.
  [[ -n "${DRAGENT_CONPTY:-}" ]] && printf '\033[2J\033[3J\033[H'
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __dragent_on_precmd
add-zsh-hook preexec __dragent_on_preexec

__dragent_clear_screen() {
  zle .clear-screen
  __dragent_report "clear"
}
zle -N __dragent_clear_screen
bindkey '^L' __dragent_clear_screen

clear() {
  command clear "$@"
  __dragent_report "clear"
}
"#;

// ── Bash ──────────────────────────────────────────────────────────────────────

const BASH_BOOTSTRAP: &str = r#"# Dragent shell integration (bash). Generated file - safe to delete.
if [ -n "${DRAGENT_HOOKS_ACTIVE:-}" ]; then
  return 0 2>/dev/null || true
fi
DRAGENT_HOOKS_ACTIVE=1

__dragent_report() {
  printf '\033]7979;%s\033\\' "$1"
}

# Ready only once a prompt has run its PROMPT_COMMAND (see __dragent_prompt_ready).
__dragent_at_prompt=

__dragent_on_precmd() {
  local __dragent_exit=$?
  # Not ready yet: other PROMPT_COMMAND entries still run after this one,
  # and the DEBUG trap must not take one of them for the user's command.
  # __dragent_prompt_ready, kept last, marks the prompt ready.
  __dragent_at_prompt=
  __dragent_report "precmd;${__dragent_exit};${PWD}"
  __dragent_keep_ready_last
  # Rendered drain fence — see the zsh hook for the full explanation.
  [ -n "${DRAGENT_CONPTY:-}" ] && printf '\033[48;2;1;2;3m \033[49m'
}

__dragent_on_preexec() {
  # The DEBUG trap fires for every simple command; only report the first
  # one after a prompt, and never our own bookkeeping.
  [ -n "${COMP_LINE:-}" ] && return
  [ -z "${__dragent_at_prompt:-}" ] && return
  case "$BASH_COMMAND" in
    __dragent_*) return ;;
  esac
  __dragent_at_prompt=
  __dragent_report "preexec;$BASH_COMMAND"
  # Windows ConPTY only: empty the console buffer so scroll/resize repaints
  # can only re-emit the current command's output (see zsh hook).
  [ -n "${DRAGENT_CONPTY:-}" ] && printf '\033[2J\033[3J\033[H'
}

__dragent_prompt_ready() {
  __dragent_at_prompt=1
}

# Keeps __dragent_prompt_ready the last PROMPT_COMMAND entry, as a string or
# as a bash 5.1+ array, whatever other scripts added since.
__dragent_keep_ready_last() {
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    local -a __dragent_kept=()
    local __dragent_entry
    for __dragent_entry in "${PROMPT_COMMAND[@]}"; do
      [ "$__dragent_entry" = "__dragent_prompt_ready" ] || __dragent_kept+=("$__dragent_entry")
    done
    PROMPT_COMMAND=("${__dragent_kept[@]}" "__dragent_prompt_ready")
  else
    case "$PROMPT_COMMAND" in
      *__dragent_prompt_ready) ;;
      *)
        PROMPT_COMMAND="${PROMPT_COMMAND//;__dragent_prompt_ready/}"
        PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__dragent_prompt_ready"
        ;;
    esac
  fi
}

if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
  PROMPT_COMMAND=("__dragent_on_precmd" "${PROMPT_COMMAND[@]}")
else
  PROMPT_COMMAND="__dragent_on_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
__dragent_keep_ready_last
trap '__dragent_on_preexec' DEBUG

clear() {
  # `clear` is an external program (ncurses), NOT a bash builtin — `builtin clear`
  # errors "clear: not a shell builtin". Use `command` to run the real clear while
  # still bypassing this function (avoids recursion).
  command clear "$@"
  __dragent_report "clear"
}
"#;

// ── Fish ──────────────────────────────────────────────────────────────────────
//
// Fish notes:
// - `fish_preexec` fires before each command (Fish 2.4+).
// - `$status` inside `fish_postexec` is unreliable (always 0 on some Fish
//   versions). We wrap `fish_prompt` instead so we capture `$status` as the
//   very first thing — before any sub-call can reset it. This is the pattern
//   used by Starship and other terminal integrations.
// - `functions -c SRC DST` copies an existing function so the user's original
//   prompt is preserved and called from within our wrapper.
// - Requires Fish 2.4+; Fish 3.3+ recommended for stable event behaviour.

const FISH_BOOTSTRAP: &str = r#"# Dragent shell integration (fish). Generated file - safe to delete.
if set -q DRAGENT_HOOKS_ACTIVE
    return 0
end
set -gx DRAGENT_HOOKS_ACTIVE 1

function __dragent_report
    printf '\033]7979;%s\033\\' $argv[1]
end

# preexec: fires right before an interactive command is executed.
# $argv[1] is the command string verbatim.
function __dragent_on_preexec --on-event fish_preexec
    __dragent_report "preexec;$argv[1]"
end

# precmd: wrap fish_prompt to capture $status reliably (must be the first line).
if functions -q fish_prompt
    functions -c fish_prompt __dragent_orig_fish_prompt
end

function fish_prompt
    set -l __dragent_exit $status  # capture before any sub-call resets it
    __dragent_report "precmd;$__dragent_exit;$PWD"
    if functions -q __dragent_orig_fish_prompt
        __dragent_orig_fish_prompt
    else
        printf '%s%s%s> ' (set_color blue) (prompt_pwd) (set_color normal)
    end
end

# ctrl+L: emit clear then perform the built-in clear-screen action.
function __dragent_clear_screen
    __dragent_report "clear"
    commandline -f clear-screen
end
bind ctrl-l __dragent_clear_screen
bind -M insert ctrl-l __dragent_clear_screen

# `clear` command override.
function clear
    command clear $argv
    __dragent_report "clear"
end
"#;

// ── PowerShell ────────────────────────────────────────────────────────────────
//
// PowerShell notes (architecture follows the two production references:
// VS Code's shellIntegration.ps1 and Windows Terminal's integration tutorial):
// - preexec: there is no native hook, so we wrap `PSConsoleHostReadLine` — the
//   function the console host calls to read one input line (provided by
//   PSReadLine). It fires exactly once per ACCEPTED input, after editing is
//   finished, so multi-line constructs, pasted blocks, and rebound keys all
//   work — unlike a PSReadLine Enter-key handler, which fires per keypress and
//   misses every other way a line can be accepted. (VS Code's approach.)
// - precmd: wraps the `prompt` function. `$?` MUST be read in the very first
//   expression — every completed statement (even an assignment) resets it.
//   Exit codes use Windows Terminal's recipe: `$?` true → 0; a PowerShell
//   (non-native) error is detected by comparing `$Error[0]`'s HistoryId with
//   the latest history entry → 1; otherwise `$LASTEXITCODE` carries the native
//   exit code (it is sticky, so it can't be used alone).
// - cwd reports `$PWD.ProviderPath` for the FileSystem provider (a UNC/registry
//   PSDrive path would confuse the emulator's cd tracking; `.Path` can carry a
//   PSDrive prefix like `Env:\`).
// - `$?` is restored for the user's own prompt via `Write-Error '' -ea Ignore`
//   so custom prompts (oh-my-posh, starship) still see the real failure state.
// - ESC is written as [char]0x1b for compatibility with PowerShell 5.1
//   (backtick-e `e was added in PowerShell 6.0). No `&&`, `?:` or other 7.x-only
//   syntax anywhere — the script must parse on 5.1.
// - [Console]::Write() bypasses PowerShell's output pipeline and emits the
//   OSC bytes directly (Write-Host appends a newline and would corrupt the
//   cursor position mid-line).
// - LanguageMode guard: ConstrainedLanguage blocks [Console]::Write method
//   calls; install nothing rather than break the shell.
// - Requires PowerShell 5.1+ with PSReadLine installed (default on Win 10+
//   and all PowerShell 7.x). Without PSReadLine, precmd/exit codes still work;
//   preexec does not (same degradation as VS Code).

const POWERSHELL_BOOTSTRAP: &str = r#"# Dragent shell integration (PowerShell). Generated file - safe to delete.
if ($Global:DRAGENT_HOOKS_ACTIVE) { return }
if ($ExecutionContext.SessionState.LanguageMode -ne "FullLanguage") { return }
$Global:DRAGENT_HOOKS_ACTIVE = $true

$Global:__DragentState = @{
    OrigPrompt    = $function:prompt
    OrigReadLine  = $null
    LastHistoryId = -1
}

function Global:__DragentReport {
    param([string]$Payload)
    $esc = [char]0x1b
    [Console]::Write("${esc}]7979;${Payload}${esc}\")
}

# Loaded as the shell's startup command, this runs before the console host
# imports PSReadLine for its first prompt; import it now so the preexec hook
# below is installed (the host then reuses the loaded module).
if (-not (Get-Module -Name PSReadLine)) {
    Import-Module PSReadLine -ErrorAction SilentlyContinue
}

if (Get-Module -Name PSReadLine -ErrorAction SilentlyContinue) {
    # The app submits commands as "<Ctrl-U><command>\r" so any half-typed input
    # is cleared first (bash/zsh bind C-u natively). PSReadLine 2.0 (the version
    # that ships with Windows PowerShell 5.1) does NOT bind Ctrl+u — the raw
    # 0x15 byte would be INSERTED into the buffer, poisoning every submitted
    # command with an invisible control character ("ls" becomes "\x15ls" →
    # CommandNotFoundException). Bind it explicitly. RevertLine exists in every
    # PSReadLine version and clears the whole line regardless of cursor position.
    Set-PSReadLineKeyHandler -Chord Ctrl+u -Function RevertLine

    # preexec: wrap PSConsoleHostReadLine, the function the console host calls
    # to read one input line (PSReadLine provides the original). Runs once per
    # ACCEPTED input — after all editing, on every acceptance path — then the
    # returned string is what the engine executes.
    $Global:__DragentState.OrigReadLine = $function:PSConsoleHostReadLine
    function Global:PSConsoleHostReadLine {
        $__line = $Global:__DragentState.OrigReadLine.Invoke()
        if (-not [string]::IsNullOrWhiteSpace($__line)) {
            __DragentReport "preexec;$__line"
            # Windows ConPTY only: empty the console buffer so scroll/resize
            # repaints can only re-emit the current command's output.
            if ($env:DRAGENT_CONPTY) {
                $e = [char]0x1b
                [Console]::Write("${e}[2J${e}[3J${e}[H")
            }
        }
        $__line
    }
}

# precmd: wrap prompt to report exit code + cwd after each command.
function global:prompt {
    # $? must be read in the FIRST expression: every completed statement —
    # including an innocuous assignment like `$x = $global:LASTEXITCODE` —
    # succeeds and resets $? to $true, so reading it any later would make every
    # failed cmdlet report exit code 0.
    $__ok = $?
    $__ec = $global:LASTEXITCODE
    Set-StrictMode -Off

    # Windows Terminal's exit-code recipe: $? true → 0. Otherwise, if the most
    # recent error came from the command just executed (matching history id),
    # it was a PowerShell error (cmdlet/parse), which never touches
    # $LASTEXITCODE → report 1. Otherwise $LASTEXITCODE holds the native
    # command's real exit code.
    $__hist = Get-History -Count 1
    $exitCode = if ($__ok) { 0 }
        elseif ($null -ne $__hist -and $global:Error.Count -gt 0 -and
                $global:Error[0].InvocationInfo.HistoryId -eq $__hist.Id) { 1 }
        elseif ($null -ne $__ec -and $__ec -ne 0) { $__ec }
        else { 1 }

    $__cwd = if ($PWD.Provider.Name -eq 'FileSystem') { $PWD.ProviderPath } else { $PWD.Path }
    __DragentReport "precmd;$exitCode;$__cwd"

    # Fence for the emulator's Windows drain logic. ConPTY forwards OSC and
    # DECSET sequences OUT OF ORDER — ahead of screen text it is still
    # repainting — so neither the precmd report above nor a mode toggle can
    # mark the end of a command's output. The only thing ConPTY re-emits
    # strictly in order is CELL CONTENT, so paint one real cell (a space) with
    # a sentinel background color the emulator recognizes (RGB 1,2,3 — see
    # FENCE_BG_SENTINEL in dragent_terminal). It becomes an invisible leading
    # cell of the prompt line, which the app never displays.
    if ($env:DRAGENT_CONPTY) {
        $e = [char]0x1b
        [Console]::Write("${e}[48;2;1;2;3m ${e}[49m")
    }

    # Restore $? so a custom prompt (oh-my-posh, starship) sees the real state.
    if (-not $__ok) { Write-Error '' -ErrorAction Ignore }

    if ($Global:__DragentState.OrigPrompt) {
        & $Global:__DragentState.OrigPrompt
    } else {
        "PS $($PWD.Path)$('>' * ($nestedPromptLevel + 1)) "
    }

    # Restore $LASTEXITCODE so the user's prompt still sees the right value.
    $global:LASTEXITCODE = $__ec
}

# clear: intercept Clear-Host (aliased as 'clear' and 'cls'). Write the clear
# escape sequences DIRECTLY — do NOT call [Console]::Clear() or the original
# Clear-Host (which use Console/RawUI buffer operations). Under a ConPTY pipe
# those hang or throw, wedging the shell mid-command so it never returns to the
# prompt (no precmd → stuck "running", input dead). A raw byte write is safe and
# is exactly what `clear` does on Unix.
function global:Clear-Host {
    __DragentReport "clear"
}
"#;

// ── Nushell ───────────────────────────────────────────────────────────────────
//
// Nushell notes:
// - `pre_execution` hook fires before each command. `commandline` returns the
//   command text reliably since nushell 0.78. Requires nushell 0.78+.
// - `pre_prompt` hook fires before each prompt and `$env.LAST_EXIT_CODE` holds
//   the exit code of the last external process.
// - `def` inside an `if` block does NOT escape to the parent scope in nushell,
//   so the `clear` override must be defined at the top level of the file, not
//   inside the `DRAGENT_HOOKS_ACTIVE` guard.
// - Hooks are appended (not replaced) using the `upsert` + `append` pattern to
//   avoid clobbering hooks registered by other integrations (e.g. starship).
// - `\u{1b}` inside `$"..."` string interpolation is nushell's ESC literal.
// - `is-not-empty` is a built-in filter (equivalent to `| not (is-empty)`).
// - The source command is `source '/path/to/file.nu'`; nushell does not honour
//   the leading-space history-suppression convention used by zsh/bash/fish.

const NUSHELL_BOOTSTRAP: &str = r#"# Dragent shell integration (nushell). Generated file - safe to delete.
# Requires nushell 0.78+ for reliable `commandline` capture in pre_execution.

# `def` must be at the top level — it does NOT escape `if` block scope.
def clear [] {
    ^clear
    print -n $"\u{1b}]7979;clear\u{1b}\\"
}

if 'DRAGENT_HOOKS_ACTIVE' not-in $env {
    $env.DRAGENT_HOOKS_ACTIVE = "1"

    $env.config = ($env.config | upsert hooks.pre_execution (
        ($env.config | get -i hooks.pre_execution | default []) | append {||
            let cmd = (commandline)
            if ($cmd | is-not-empty) {
                print -n $"\u{1b}]7979;preexec;($cmd)\u{1b}\\"
            }
        }
    ))

    $env.config = ($env.config | upsert hooks.pre_prompt (
        ($env.config | get -i hooks.pre_prompt | default []) | append {||
            let exit_code = ($env.LAST_EXIT_CODE? | default 0)
            print -n $"\u{1b}]7979;precmd;($exit_code);($env.PWD)\u{1b}\\"
        }
    ))
}
"#;

// ── Public API ────────────────────────────────────────────────────────────────

/// The bootstrap script body for a shell, if integration is supported.
pub fn bootstrap_script(kind: ShellKind) -> Option<&'static str> {
    match kind {
        ShellKind::Zsh => Some(ZSH_BOOTSTRAP),
        ShellKind::Bash => Some(BASH_BOOTSTRAP),
        ShellKind::Fish => Some(FISH_BOOTSTRAP),
        ShellKind::PowerShell => Some(POWERSHELL_BOOTSTRAP),
        ShellKind::Nu => Some(NUSHELL_BOOTSTRAP),
        ShellKind::Other => None,
    }
}

/// Write the bootstrap script to a stable temp location and return its path.
pub fn install_bootstrap(kind: ShellKind) -> anyhow::Result<Option<PathBuf>> {
    let Some(script) = bootstrap_script(kind) else {
        return Ok(None);
    };
    let extension = match kind {
        ShellKind::Zsh => "zsh",
        ShellKind::Bash => "bash",
        ShellKind::Fish => "fish",
        ShellKind::PowerShell => "ps1",
        ShellKind::Nu => "nu",
        ShellKind::Other => unreachable!(),
    };
    // Named by content and never rewritten: other shells may be reading it
    // while a new terminal starts. Rewriting a shared file in place fails on
    // Windows while it is open (sharing violation, os error 32) and could hand
    // a starting shell a half-written script anywhere.
    let stem = format!("dragent_hooks-{:016x}", content_hash(script.as_bytes()));
    let dir = std::env::temp_dir();
    let path = dir.join(format!("{stem}.{extension}"));
    if path.is_file() {
        return Ok(Some(path));
    }
    // Unique per call: one process creates several terminals concurrently.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let attempt = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let partial = dir.join(format!("{stem}.{}-{attempt}.tmp", std::process::id()));
    std::fs::File::create(&partial)
        .and_then(|mut file| file.write_all(script.as_bytes()))
        .context("failed to write shell bootstrap file")?;
    if let Err(error) = std::fs::rename(&partial, &path) {
        let _ = std::fs::remove_file(&partial);
        // Another process installed the same content first.
        if !path.is_file() {
            return Err(error).context("failed to install shell bootstrap file");
        }
    }
    Ok(Some(path))
}

/// FNV-1a: a stable name for a script's exact content.
fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The line to send to an interactive shell to load the bootstrap.
///
/// For zsh/bash the leading space keeps the command out of history
/// (`HIST_IGNORE_SPACE` / `ignorespace`). Fish also respects a leading space
/// by default. PowerShell uses dot-sourcing (`. 'path'`) and does not use the
/// leading-space convention.
pub fn source_command(kind: ShellKind, path: &std::path::Path) -> String {
    match kind {
        ShellKind::Zsh | ShellKind::Bash => format!(" builtin source '{}'", posix_path(path)),
        ShellKind::Fish => format!(" source '{}'", posix_path(path)),
        ShellKind::PowerShell => {
            // Dot-sourcing (`. file.ps1`) an unsigned script is blocked by the
            // default Windows ExecutionPolicy (Restricted/RemoteSigned), so the
            // hook would silently fail to load. Reading the file and running it
            // via `Invoke-Expression` is treated like interactive input and is
            // NOT gated by ExecutionPolicy — the reliable way to load integration.
            // The leading space mirrors the POSIX variants: it is harmless to the
            // parser and guards against conhost occasionally swallowing the first
            // byte written right after spawn (the same reason write_command
            // prepends Ctrl-U — which is not an option here because the Ctrl+u
            // binding is installed by this very script).
            format!(" Get-Content -Raw '{}' | Invoke-Expression", path.display())
        }
        // No leading-space convention in nushell; plain source is correct.
        ShellKind::Nu => format!("source '{}'", posix_path(path)),
        ShellKind::Other => unreachable!(),
    }
}

/// On Windows, rewrite a native path (`C:\a\b`) to the MSYS / Git-Bash POSIX form
/// (`/c/a/b`) so POSIX shells (bash/zsh/fish/nu run under Git-Bash/MSYS) can
/// `source` it — a Windows-format path fails `source` in those shells. No-op on
/// non-Windows and for paths without a drive letter.
pub fn posix_path(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    #[cfg(windows)]
    {
        let bytes = s.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let drive = (bytes[0] as char).to_ascii_lowercase();
            let rest = s[2..].replace('\\', "/");
            return format!("/{drive}{rest}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_install_is_idempotent_and_named_by_content() {
        let first = install_bootstrap(ShellKind::Bash).unwrap().unwrap();
        let again = install_bootstrap(ShellKind::Bash).unwrap().unwrap();
        assert_eq!(first, again);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            bootstrap_script(ShellKind::Bash).unwrap()
        );
        assert_ne!(
            first,
            install_bootstrap(ShellKind::PowerShell).unwrap().unwrap()
        );
    }

    /// A shell reading the script (PowerShell's Get-Content shares read only)
    /// must not make the next terminal's install fail with os error 32.
    #[cfg(windows)]
    #[test]
    fn bootstrap_install_succeeds_while_a_shell_is_reading_it() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        let path = install_bootstrap(ShellKind::PowerShell).unwrap().unwrap();
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let concurrent: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| install_bootstrap(ShellKind::PowerShell)))
            .collect();
        for install in concurrent {
            assert_eq!(install.join().unwrap().unwrap().unwrap(), path);
        }
    }

    #[test]
    fn detects_shell_kinds() {
        assert_eq!(ShellKind::from_program("/bin/zsh"), ShellKind::Zsh);
        assert_eq!(
            ShellKind::from_program("/opt/homebrew/bin/bash"),
            ShellKind::Bash
        );
        assert_eq!(ShellKind::from_program("/usr/bin/fish"), ShellKind::Fish);
        assert_eq!(ShellKind::from_program("fish"), ShellKind::Fish);
        assert_eq!(
            ShellKind::from_program("/usr/bin/pwsh"),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::from_program("pwsh.exe"), ShellKind::PowerShell);
        assert_eq!(
            ShellKind::from_program("powershell.exe"),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::from_program("/usr/bin/nu"), ShellKind::Nu);
        assert_eq!(ShellKind::from_program("nu"), ShellKind::Nu);
        assert_eq!(ShellKind::from_program("/bin/sh"), ShellKind::Other);
        // Windows: bash.exe must be recognized so shell-integration hooks install.
        assert_eq!(ShellKind::from_program("bash.exe"), ShellKind::Bash);
    }

    #[test]
    fn scripts_reference_the_hook_channel() {
        for kind in [
            ShellKind::Zsh,
            ShellKind::Bash,
            ShellKind::Fish,
            ShellKind::PowerShell,
            ShellKind::Nu,
        ] {
            let script = bootstrap_script(kind).unwrap();
            assert!(script.contains("7979;"), "missing OSC channel in {kind:?}");
            assert!(script.contains("precmd;"), "missing precmd in {kind:?}");
            assert!(script.contains("preexec;"), "missing preexec in {kind:?}");
            assert!(script.contains("clear"), "missing clear in {kind:?}");
        }
        assert!(bootstrap_script(ShellKind::Other).is_none());
    }

    #[test]
    fn source_command_formats() {
        let p = std::path::Path::new("/tmp/dragent_hooks.zsh");
        assert_eq!(
            source_command(ShellKind::Zsh, p),
            " builtin source '/tmp/dragent_hooks.zsh'"
        );

        let p = std::path::Path::new("/tmp/dragent_hooks.bash");
        assert_eq!(
            source_command(ShellKind::Bash, p),
            " builtin source '/tmp/dragent_hooks.bash'"
        );

        let p = std::path::Path::new("/tmp/dragent_hooks.fish");
        assert_eq!(
            source_command(ShellKind::Fish, p),
            " source '/tmp/dragent_hooks.fish'"
        );

        let p = std::path::Path::new("/tmp/dragent_hooks.ps1");
        assert_eq!(
            source_command(ShellKind::PowerShell, p),
            " Get-Content -Raw '/tmp/dragent_hooks.ps1' | Invoke-Expression"
        );

        let p = std::path::Path::new("/tmp/dragent_hooks.nu");
        assert_eq!(
            source_command(ShellKind::Nu, p),
            "source '/tmp/dragent_hooks.nu'"
        );
    }

    #[test]
    fn fish_script_wraps_prompt_and_binds_ctrl_l() {
        let script = bootstrap_script(ShellKind::Fish).unwrap();
        assert!(script.contains("fish_preexec"));
        assert!(script.contains("fish_prompt"));
        assert!(script.contains("__dragent_orig_fish_prompt"));
        assert!(script.contains("bind ctrl-l"));
    }

    #[test]
    fn powershell_script_uses_psreadline_and_wraps_prompt() {
        let script = bootstrap_script(ShellKind::PowerShell).unwrap();
        assert!(script.contains("PSReadLine"));
        // preexec comes from wrapping the console host's read function, not an
        // Enter key chord (which breaks multi-line input and alternate accepts).
        assert!(script.contains("PSConsoleHostReadLine"));
        assert!(script.contains("OrigReadLine"));
        // Ctrl+u must be bound: the app prefixes submissions with 0x15, which
        // PSReadLine 2.0 (PowerShell 5.1) would otherwise insert literally.
        assert!(script.contains("Set-PSReadLineKeyHandler -Chord Ctrl+u -Function RevertLine"));
        assert!(script.contains("OrigPrompt"));
        assert!(script.contains("[char]0x1b"));
        assert!(script.contains("LASTEXITCODE"));
        // $? must be captured in the prompt's first statement.
        let ok_pos = script.find("$__ok = $?").expect("$? capture missing");
        let prompt_pos = script.find("function global:prompt").unwrap();
        assert!(ok_pos > prompt_pos && ok_pos - prompt_pos < 400);
        // Rendered ConPTY fence cell so block completion doesn't rely on a
        // lossy timeout (sentinel bg RGB 1,2,3 — FENCE_BG_SENTINEL).
        assert!(script.contains("[48;2;1;2;3m"));
    }

    #[test]
    fn nushell_script_uses_config_hooks_and_top_level_clear() {
        let script = bootstrap_script(ShellKind::Nu).unwrap();
        // guard
        assert!(script.contains("DRAGENT_HOOKS_ACTIVE"));
        // hooks registered via config upsert
        assert!(script.contains("pre_execution"));
        assert!(script.contains("pre_prompt"));
        assert!(script.contains("commandline"));
        assert!(script.contains("is-not-empty"));
        assert!(script.contains("LAST_EXIT_CODE"));
        // ESC via nushell unicode literal
        assert!(script.contains(r"\u{1b}"));
        // `def clear []` must appear before the `if` guard (top-level placement)
        let def_pos = script.find("def clear []").expect("def clear [] not found");
        let if_pos = script
            .find("if 'DRAGENT_HOOKS_ACTIVE'")
            .expect("guard not found");
        assert!(def_pos < if_pos, "def clear [] must precede the if guard");
    }
}
