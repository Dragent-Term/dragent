//! Shell-integration protocol: OSC 133 (standard) + OSC 7979 (Dragent).
//!
//! # OSC 133 — Semantic shell prompt zones
//!
//! The FinalTerm / iTerm2 / VSCode OSC 133 protocol marks the boundaries of
//! shell-prompt and command-output zones so the terminal can understand which
//! output belongs to which command.
//!
//! ```text
//! ESC ] 133 ; A ST   — start of prompt (mark_a)
//! ESC ] 133 ; B ST   — end of prompt / start of command input (mark_b)
//! ESC ] 133 ; C ST   — end of command input / start of command output (mark_c)
//! ESC ] 133 ; D [;exit_code] ST  — end of command output (mark_d)
//! ESC ] 133 ; P ; <key=value pairs> ST  — property update
//! ```
//!
//! # OSC 7979 — Dragent native hooks
//!
//! ```text
//! ESC ] 7979 ; preexec ; <command> ST
//! ESC ] 7979 ; precmd  ; <exit_code> ; <cwd> ST
//! ESC ] 7979 ; clear ST
//! ```

/// Semantic prompt zone marker (OSC 133).
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SemanticMarker {
    /// Start of shell prompt (`A`).
    PromptStart,
    /// End of prompt / start of user input (`B`).
    PromptEnd,
    /// End of user input / start of command output (`C`).
    CommandStart,
    /// End of command output (`D`), with optional exit code.
    CommandEnd { exit_code: Option<i32> },
    /// Arbitrary property attached to a zone (`P`).
    Property { key: String, value: String },
}

/// Parse an OSC 133 payload (the part after `"133;"`) into a
/// [`SemanticMarker`], or return `None` if the payload is unrecognised.
pub fn parse_osc133(payload: &str) -> Option<SemanticMarker> {
    // payload is everything after the "133;" prefix, e.g. "A" or "D;1" or "P;k=v"
    let mut parts = payload.splitn(2, ';');
    let code = parts.next()?.trim();
    let rest = parts.next().unwrap_or_default();

    match code {
        "A" => Some(SemanticMarker::PromptStart),
        "B" => Some(SemanticMarker::PromptEnd),
        "C" => Some(SemanticMarker::CommandStart),
        "D" => {
            let exit_code = if rest.is_empty() {
                None
            } else {
                rest.trim().parse::<i32>().ok()
            };
            Some(SemanticMarker::CommandEnd { exit_code })
        }
        "P" => {
            // rest is "key=value" or "key=value;key2=value2"
            // Just grab the first key=value pair.
            let mut kv = rest.splitn(2, '=');
            let key = kv.next().unwrap_or_default().trim().to_string();
            let value = kv.next().unwrap_or_default().trim().to_string();
            if key.is_empty() {
                None
            } else {
                Some(SemanticMarker::Property { key, value })
            }
        }
        _ => None,
    }
}

/// Structured shell lifecycle events from Dragent's OSC 7979 protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// The shell is about to execute `command` (preexec).
    Preexec { command: String },
    /// The shell returned to prompt: `exit_code` + current `cwd` (precmd).
    Precmd { exit_code: i32, cwd: String },
    /// The user cleared the screen.
    Clear,
    /// The rendered drain fence (Windows ConPTY): a sentinel background color
    /// the shell hooks paint on one prompt cell. ConPTY forwards OSC and DECSET
    /// sequences OUT OF ORDER, ahead of screen text it is still repainting —
    /// but cell content and its SGR attributes are re-emitted strictly in
    /// order. A sentinel *cell* therefore cannot arrive before the command
    /// output that precedes it, making it the only trustworthy end-of-output
    /// marker. Emitted by the emulator when SGR sets the sentinel background
    /// (`FENCE_BG_SENTINEL` in `emulator.rs`). ConPTY repaints can re-emit the
    /// cell, so consumers must treat repeats as no-ops.
    Fence,
}

/// Parse an OSC 7979 payload (the part after `"7979;"`) into a
/// [`HookEvent`], or `None` if unrecognised.
pub fn parse_hook_payload(payload: &str) -> Option<HookEvent> {
    let mut parts = payload.splitn(2, ';');
    let kind = parts.next()?;
    let rest = parts.next().unwrap_or_default();

    match kind {
        "preexec" => Some(HookEvent::Preexec {
            command: agent_display_command(rest.trim()).to_string(),
        }),
        "precmd" => {
            let mut fields = rest.splitn(2, ';');
            let exit_code = fields.next()?.trim().parse::<i32>().ok()?;
            let cwd = fields.next().unwrap_or_default().to_string();
            Some(HookEvent::Precmd { exit_code, cwd })
        }
        "clear" => Some(HookEvent::Clear),
        _ => None,
    }
}

/// The private launch monitor is transport plumbing. Preserve the provider's
/// actual command in checkpoints, command blocks, and agent input routing.
pub fn agent_display_command(command: &str) -> &str {
    if let Some((_, tail)) = command.split_once(" --dragent-agent-run ") {
        if let Some((prefix, actual)) = tail.split_once(" -- ") {
            // `<pane-uuid> ['--dragent-organisation=<id>']`; older launches
            // carry no organisation.
            let mut words = prefix.split(' ');
            let pane = words.next().unwrap_or_default();
            let organisation_only = words.all(|word| {
                word.trim_matches(|c| c == '\'' || c == '"')
                    .starts_with("--dragent-organisation=")
            });
            if organisation_only
                && pane.len() == 36
                && pane.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            {
                return actual;
            }
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_display_command_strips_the_recovery_wrapper() {
        let pane = "0f1ea82a-e4b3-43e8-b62a-849d04ee32ed";
        for wrapped in [
            format!("'/app/dragent' --dragent-agent-run {pane} -- claude --model x"),
            format!(
                "'/app/dragent' --dragent-agent-run {pane} '--dragent-organisation=5b0c' -- claude --model x"
            ),
        ] {
            assert_eq!(agent_display_command(&wrapped), "claude --model x");
        }
        let odd = format!("x --dragent-agent-run {pane} --other -- claude");
        assert_eq!(agent_display_command(&odd), odd);
    }

    #[test]
    fn osc133_prompt_start() {
        assert_eq!(parse_osc133("A"), Some(SemanticMarker::PromptStart));
    }

    #[test]
    fn osc133_command_end_with_code() {
        assert_eq!(
            parse_osc133("D;42"),
            Some(SemanticMarker::CommandEnd {
                exit_code: Some(42)
            })
        );
    }

    #[test]
    fn osc133_command_end_no_code() {
        assert_eq!(
            parse_osc133("D"),
            Some(SemanticMarker::CommandEnd { exit_code: None })
        );
    }

    #[test]
    fn osc133_property() {
        assert_eq!(
            parse_osc133("P;k=v"),
            Some(SemanticMarker::Property {
                key: "k".into(),
                value: "v".into()
            })
        );
    }

    #[test]
    fn hook_preexec() {
        assert_eq!(
            parse_hook_payload("preexec;cargo build"),
            Some(HookEvent::Preexec {
                command: "cargo build".into()
            })
        );
    }

    #[test]
    fn hook_precmd_with_semicolon_in_cwd() {
        assert_eq!(
            parse_hook_payload("precmd;0;/tmp/a;b"),
            Some(HookEvent::Precmd {
                exit_code: 0,
                cwd: "/tmp/a;b".into()
            })
        );
    }
}
