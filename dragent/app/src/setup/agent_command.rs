//! Prompt delivery for editable terminal agent commands.
//! Keep shell syntax in one place so saved settings and launches cannot drift.

use std::path::{Path, PathBuf};

pub(crate) fn validate(
    command: &str,
    prompt_command: &str,
    suffix: &str,
) -> Result<(), &'static str> {
    if command.trim().is_empty() {
        return Err("Enter a launch command. The prompt command can be empty to reuse it.");
    }
    if [command, prompt_command, suffix]
        .iter()
        .any(|value| value.chars().any(char::is_control))
    {
        return Err("Commands must be single-line text without control characters.");
    }
    Ok(())
}

/// PTY normalization for prompts: line endings, ANSI sequences, controls,
/// and tabs must not become shell editing keystrokes.
fn sanitize_prompt(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let chars: Vec<char> = normalized.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let (kind, start) = if c == '\u{1b}' && i + 1 < chars.len() {
            (chars[i + 1], i + 2)
        } else if c == '\u{9b}' {
            ('[', i + 1)
        } else if c == '\u{9d}' {
            (']', i + 1)
        } else {
            ('\0', i)
        };
        if kind == '[' {
            let mut end = start;
            while end < chars.len() && (' '..='?').contains(&chars[end]) {
                end += 1;
            }
            if end < chars.len() && ('@'..='~').contains(&chars[end]) {
                i = end + 1;
                continue;
            }
        } else if kind == ']' {
            let mut end = start;
            while end < chars.len() && chars[end] != '\n' {
                if chars[end] == '\u{7}' || chars[end] == '\u{9c}' {
                    i = end + 1;
                    break;
                }
                if chars[end] == '\u{1b}' && chars.get(end + 1) == Some(&'\\') {
                    i = end + 2;
                    break;
                }
                end += 1;
            }
            if i > start {
                continue;
            }
        }
        if c == '\t' {
            out.push_str("    ");
        } else if c == '\n' || !c.is_control() {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Where a launch command finds its prompt.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PromptSource<'a> {
    /// A file the shell can read (see [`stage_prompt`]), so the command the
    /// terminal echoes stays one short line however long the prompt is.
    File(&'a Path),
    /// Spelled out in the command, for a shell on another machine.
    Inline(&'a str),
}

#[derive(Clone, Copy, PartialEq)]
enum ShellKind {
    Posix,
    Fish,
    PowerShell,
}

fn shell_kind(shell: &str) -> ShellKind {
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "fish" => ShellKind::Fish,
        "pwsh" | "powershell" => ShellKind::PowerShell,
        _ => ShellKind::Posix,
    }
}

fn posix_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn fish_literal(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// PowerShell also closes a single-quoted string at typographic quotes.
fn powershell_literal(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// A double-quoted PowerShell string on one line: line breaks become `n.
fn powershell_expandable(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\n' => out.push_str("`n"),
            '`' | '$' | '"' | '\u{201c}' | '\u{201d}' | '\u{201e}' => {
                out.push('`');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Write `prompt` to a new private file in `folder` for [`PromptSource::File`],
/// removing the folder's files older than a week on the way.
pub(crate) fn stage_prompt_in(folder: &Path, prompt: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in std::fs::read_dir(folder)?.flatten() {
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > week));
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let path = folder.join(format!("{}.md", uuid::Uuid::new_v4().simple()));
    let mut text = sanitize_prompt(prompt);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&path)?, text.as_bytes())?;
    Ok(path)
}

/// [`stage_prompt_in`] the organisation's `agent-prompts` folder.
pub(crate) fn stage_prompt(prompt: &str) -> std::io::Result<PathBuf> {
    stage_prompt_in(
        &dragent_paths::organisation_dir().join("agent-prompts"),
        prompt,
    )
}

/// The shell line that starts an agent on a prompt. It is one line, except
/// the heredoc an inline stdin prompt needs in a POSIX shell: a multi-line
/// command that reaches a shell before it reads pastes arrives line by line,
/// and the shell echoes each line after a continuation prompt.
pub(crate) fn build_prompt_command(
    command: &str,
    prompt_command: &str,
    suffix: &str,
    stdin: bool,
    source: PromptSource,
    shell: &str,
    random_id: &str,
) -> Result<String, &'static str> {
    validate(command, prompt_command, suffix)?;
    let cli = if prompt_command.trim().is_empty() {
        command.trim()
    } else {
        prompt_command.trim()
    };
    let suffix = if suffix.trim().is_empty() {
        String::new()
    } else {
        format!(" {}", suffix.trim())
    };
    let kind = shell_kind(shell);
    let prompt = match source {
        PromptSource::File(path) => {
            let path = path.to_string_lossy();
            return Ok(match kind {
                // Like the inline forms, command substitution drops trailing
                // newlines and keeps the prompt one argument.
                ShellKind::Posix if stdin => format!("{cli}{suffix} < {}", posix_literal(&path)),
                ShellKind::Posix => format!("{cli} \"$(cat {})\"{suffix}", posix_literal(&path)),
                ShellKind::Fish if stdin => format!("{cli}{suffix} < {}", fish_literal(&path)),
                ShellKind::Fish => format!(
                    "{cli} (cat {} | string collect --allow-empty){suffix}",
                    fish_literal(&path)
                ),
                ShellKind::PowerShell => {
                    let text = format!(
                        "[IO.File]::ReadAllText({}).TrimEnd([char]10)",
                        powershell_literal(&path)
                    );
                    if stdin {
                        format!("{text} | {cli}{suffix}")
                    } else {
                        format!("{cli} ({text}){suffix}")
                    }
                }
            });
        }
        PromptSource::Inline(prompt) => sanitize_prompt(prompt),
    };
    if kind == ShellKind::Fish {
        // Fish has no heredocs. A literal printf supplies the same payload;
        // string collect keeps a multiline prompt in exactly one argument.
        let quoted = fish_literal(&prompt);
        return Ok(if stdin {
            format!("printf '%s\\n' {quoted} | begin; {cli}{suffix}; end")
        } else {
            format!("{cli} (printf '%s' {quoted} | string collect --allow-empty){suffix}")
        });
    }
    if kind == ShellKind::PowerShell {
        let text = powershell_expandable(prompt.trim_end_matches('\n'));
        return Ok(if stdin {
            format!("{text} | {cli}{suffix}")
        } else {
            format!("{cli} {text}{suffix}")
        });
    }
    if stdin {
        let mut delimiter = format!("DRAGENT_PROMPT_{}", random_id.replace('-', ""));
        while prompt.contains(&delimiter) {
            delimiter.push_str("_X");
        }
        return Ok(format!(
            "{cli}{suffix} <<'{delimiter}'\n{prompt}\n{delimiter}"
        ));
    }
    // A literal shell argument avoids old macOS bash's nested-heredoc
    // parsing bugs with quotes/backticks inside a prompt. Like command
    // substitution, strip trailing newlines and keep one argument.
    let prompt = prompt.trim_end_matches('\n');
    if !prompt.contains('\n') {
        return Ok(format!("{cli} {}{suffix}", posix_literal(prompt)));
    }
    // printf turns each \n back into a line break, keeping the command on one
    // line; every backslash of the prompt itself is doubled so stays literal.
    let escaped = prompt.replace('\\', "\\\\").replace('\n', "\\n");
    Ok(format!(
        "{cli} \"$(printf '%b' {})\"{suffix}",
        posix_literal(&escaped)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shells of `shells` this machine has (a Linux runner may lack zsh).
    fn installed<const N: usize>(shells: [&'static str; N]) -> impl Iterator<Item = &'static str> {
        shells
            .into_iter()
            .filter(|shell| std::path::Path::new(shell).exists())
    }

    fn run(shell: &str, script: &str) -> String {
        let output = std::process::Command::new(shell)
            .args(["-c", script])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn argument_prompt_and_suffix_reach_shell_unchanged() {
        let prompt = "quotes ' \" $HOME $(exit 42) `exit 42` \\ unicode λ\nsecond line";
        for shell in installed(["/bin/sh", "/bin/bash", "/bin/zsh"]) {
            let command = build_prompt_command(
                "capture",
                "capture --before",
                "--after ; printf done",
                false,
                PromptSource::Inline(prompt),
                shell,
                "test",
            )
            .unwrap();
            let script = format!("capture() {{ printf '<%s>' \"$@\"; }}; {command}");
            assert_eq!(
                run(shell, &script),
                format!("<--before><{prompt}><--after>done")
            );
        }
    }

    #[test]
    fn stdin_fallback_suffix_and_delimiter_collision() {
        for shell in installed(["/bin/sh", "/bin/bash", "/bin/zsh"]) {
            let prompt = "first\nDRAGENT_PROMPT_test\nlast $HOME 'quote'";
            let command = build_prompt_command(
                "capture --before",
                "",
                "--after",
                true,
                PromptSource::Inline(prompt),
                shell,
                "test",
            )
            .unwrap();
            let script = format!("capture() {{ printf '<%s>' \"$@\"; cat; }}; {command}");
            assert_eq!(
                run(shell, &script),
                format!("<--before><--after>{prompt}\n")
            );
        }
    }

    #[test]
    fn prompt_controls_are_normalized_and_invalid_settings_rejected() {
        assert_eq!(
            sanitize_prompt("a\r\nb\rc\t\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\u{3}"),
            "a\nb\nc    red"
        );
        assert!(validate("", "agent", "").is_err());
        assert!(validate("agent\nexit", "", "").is_err());
        assert!(validate("agent", "", "").is_ok());
    }

    #[test]
    fn fish_delivery_when_available() {
        let Some(shell) = [
            "/opt/homebrew/bin/fish",
            "/usr/local/bin/fish",
            "/usr/bin/fish",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists()) else {
            eprintln!("Fish integration skipped: Fish is not installed.");
            return;
        };
        let prompt = "quote ' backslash \\ $HOME\nsecond line";
        for stdin in [false, true] {
            let command = build_prompt_command(
                "capture",
                "",
                "--after",
                stdin,
                PromptSource::Inline(prompt),
                shell,
                "test",
            )
            .unwrap();
            let script = format!(
                "function capture; printf '<%s>' $argv; {}; end; {command}",
                if stdin { "cat" } else { "" }
            );
            assert_eq!(
                run(shell, &script),
                if stdin {
                    format!("<--after>{prompt}\n")
                } else {
                    format!("<{prompt}><--after>")
                }
            );
        }
    }

    /// A scratch folder whose name needs quoting in every shell.
    fn scratch_folder() -> PathBuf {
        std::env::temp_dir().join(format!(
            "dragent prompt's {}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    const TRICKY: &str = "quotes ' \" $HOME $(exit 42) `exit 42` \\ \\n %s \\c unicode λ “smart” ‘single’\nsecond line\n\nlast";

    #[test]
    fn launch_commands_stay_on_one_line() {
        let file = Path::new("/tmp/prompt.md");
        for shell in ["/bin/zsh", "/usr/bin/fish", "pwsh.exe"] {
            for stdin in [false, true] {
                for source in [PromptSource::File(file), PromptSource::Inline(TRICKY)] {
                    let command = build_prompt_command(
                        "capture", "", "--after", stdin, source, shell, "test",
                    )
                    .unwrap();
                    // Only a POSIX heredoc (an inline stdin prompt) spans lines.
                    let heredoc =
                        stdin && shell == "/bin/zsh" && matches!(source, PromptSource::Inline(_));
                    // Fish keeps its tested multi-line literal for remote shells.
                    let fish_inline =
                        shell == "/usr/bin/fish" && matches!(source, PromptSource::Inline(_));
                    if !heredoc && !fish_inline {
                        assert!(!command.contains('\n'), "{shell} stdin={stdin}: {command}");
                    }
                }
            }
        }
    }

    #[test]
    fn prompt_files_reach_the_shell_unchanged() {
        let folder = scratch_folder();
        let path = stage_prompt_in(&folder, TRICKY).unwrap();
        for shell in installed(["/bin/sh", "/bin/bash", "/bin/zsh"]) {
            let command = build_prompt_command(
                "capture",
                "capture --before",
                "--after",
                false,
                PromptSource::File(&path),
                shell,
                "test",
            )
            .unwrap();
            let script = format!("capture() {{ printf '<%s>' \"$@\"; }}; {command}");
            assert_eq!(
                run(shell, &script),
                format!("<--before><{TRICKY}><--after>")
            );
            let command = build_prompt_command(
                "capture",
                "",
                "--after",
                true,
                PromptSource::File(&path),
                shell,
                "test",
            )
            .unwrap();
            let script = format!("capture() {{ printf '<%s>' \"$@\"; cat; }}; {command}");
            assert_eq!(run(shell, &script), format!("<--after>{TRICKY}\n"));
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn staged_prompts_are_private_and_expire_after_a_week() {
        let folder = scratch_folder();
        std::fs::create_dir_all(&folder).unwrap();
        let old = folder.join("old.md");
        std::fs::write(&old, "old").unwrap();
        let eight_days = std::time::Duration::from_secs(8 * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - eight_days)
            .unwrap();
        let recent = folder.join("recent.md");
        std::fs::write(&recent, "recent").unwrap();
        let path = stage_prompt_in(&folder, "a\r\nb\u{1b}[31m").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
        assert!(!old.exists());
        assert!(recent.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn powershell_delivery_when_available() {
        let Some(shell) = [
            "/usr/local/bin/pwsh",
            "/opt/homebrew/bin/pwsh",
            "/usr/bin/pwsh",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ]
        .into_iter()
        .find(|path| Path::new(path).exists()) else {
            eprintln!("PowerShell integration skipped: pwsh is not installed.");
            return;
        };
        let pwsh = |script: &str| {
            let output = std::process::Command::new(shell)
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        // A function stands in for the agent so the check runs on every platform.
        let capture =
            "function capture { foreach ($a in $args) { [Console]::Out.Write(\"<$a>\") }; \
                       foreach ($line in $input) { [Console]::Out.Write(\"$line|\") } }";
        let folder = scratch_folder();
        let path = stage_prompt_in(&folder, TRICKY).unwrap();
        for source in [PromptSource::File(&path), PromptSource::Inline(TRICKY)] {
            let command =
                build_prompt_command("capture", "", "--after", false, source, shell, "test")
                    .unwrap();
            assert_eq!(
                pwsh(&format!("{capture}; {command}")),
                format!("<{TRICKY}><--after>"),
                "{command}"
            );
            let command =
                build_prompt_command("capture", "", "--after", true, source, shell, "test")
                    .unwrap();
            // A piped string is one object, so the whole prompt arrives as one line of input.
            assert_eq!(
                pwsh(&format!("{capture}; {command}")),
                format!("<--after>{TRICKY}|"),
                "{command}"
            );
        }
        std::fs::remove_dir_all(folder).unwrap();
    }
}
