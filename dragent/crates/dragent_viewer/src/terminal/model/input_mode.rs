/// Which AI tool is currently active in this pane.
///
/// The full registry of terminal AI agents we recognize. Adding one is three
/// lines: a variant, a `from_command` arm, and its `is_alt_screen`/`label`
/// classification — every downstream behavior (AI panel placement, submit
/// style, sidebar label, voice/attachment routing) derives from these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiTool {
    /// Claude Code — alt-screen TUI.
    Claude,
    /// OpenAI Codex — runs as a normal-block REPL (as we drive it).
    Codex,
    /// Ollama chat — normal-block REPL.
    Ollama,
    /// Google Gemini CLI (and its Antigravity successor `agy`) — alt-screen TUI.
    Gemini,
    /// GitHub Copilot CLI (`copilot` or `gh copilot`) — alt-screen TUI.
    Copilot,
    /// Aider — git-native pair programmer, normal-screen REPL.
    Aider,
    /// OpenCode — alt-screen TUI.
    OpenCode,
    /// Crush (Charm) — alt-screen TUI.
    Crush,
    /// Goose (Block) — normal-screen session REPL.
    Goose,
    /// Qwen Code — alt-screen TUI (gemini-cli lineage).
    Qwen,
    /// Amp (Sourcegraph) — alt-screen TUI.
    Amp,
    /// Cursor CLI (`cursor-agent`) — alt-screen TUI.
    CursorAgent,
    /// Factory Droid — alt-screen TUI.
    Droid,
    /// Grok CLI — alt-screen TUI.
    Grok,
    /// Cline CLI — alt-screen TUI.
    Cline,
    /// Plandex — normal-screen REPL.
    Plandex,
    /// Auggie — normal-screen REPL until it enters the alt screen.
    Auggie,
    /// Autohand Code — normal-screen REPL until it enters the alt screen.
    Autohand,
    /// CodeBuddy Code — normal-screen REPL until it enters the alt screen.
    CodeBuddy,
    /// Codebuff — normal-screen REPL until it enters the alt screen.
    Codebuff,
    /// Command Code — normal-screen REPL until it enters the alt screen.
    CommandCode,
    /// Continue — normal-screen REPL until it enters the alt screen.
    Continue,
    /// Devin — normal-screen REPL until it enters the alt screen.
    Devin,
    /// Freebuff — normal-screen REPL until it enters the alt screen.
    Freebuff,
    /// Hermes Agent — normal-screen REPL until it enters the alt screen.
    Hermes,
    /// Jules — normal-screen REPL until it enters the alt screen.
    Jules,
    /// Junie — normal-screen REPL until it enters the alt screen.
    Junie,
    /// Kilocode — normal-screen REPL until it enters the alt screen.
    Kilocode,
    /// Kimi — normal-screen REPL until it enters the alt screen.
    Kimi,
    /// Kiro (AWS) — normal-screen REPL until it enters the alt screen.
    Kiro,
    /// Letta — normal-screen REPL until it enters the alt screen.
    Letta,
    /// MiMo Code — normal-screen REPL until it enters the alt screen.
    MiMoCode,
    /// Mistral Vibe — normal-screen REPL until it enters the alt screen.
    MistralVibe,
    /// Muse Code — normal-screen REPL until it enters the alt screen.
    Muse,
    /// Oh My Pi — normal-screen REPL until it enters the alt screen.
    OhMyPi,
    /// Pi — normal-screen REPL until it enters the alt screen.
    Pi,
    /// Prime Agent — normal-screen REPL until it enters the alt screen.
    PrimeAgent,
    /// Qoder CLI — normal-screen REPL until it enters the alt screen.
    Qoder,
    /// Rovo Dev — normal-screen REPL until it enters the alt screen.
    Rovo,
    /// Zero — normal-screen REPL until it enters the alt screen.
    Zero,
}

impl AiTool {
    /// Detect the tool from the leading token(s) of a submitted command.
    pub fn from_command(cmd: &str) -> Option<Self> {
        let mut tokens = cmd.split_whitespace();
        let first = tokens.next().unwrap_or("");
        // Launched by path (`/opt/homebrew/bin/claude`, `~/.local/bin/codex`):
        // the binary's own name decides.
        let first = first.rsplit('/').next().unwrap_or(first);
        match first {
            "claude" | "claude-code" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "ollama" => Some(Self::Ollama),
            "gemini" | "agy" => Some(Self::Gemini),
            "copilot" => Some(Self::Copilot),
            // Copilot also ships as a gh subcommand: `gh copilot …`
            "gh" if tokens.next() == Some("copilot") => Some(Self::Copilot),
            "aider" => Some(Self::Aider),
            "opencode" => Some(Self::OpenCode),
            "crush" => Some(Self::Crush),
            "goose" => Some(Self::Goose),
            "qwen" => Some(Self::Qwen),
            "amp" => Some(Self::Amp),
            // Cursor's CLI installs as `cursor-agent`, aliases as plain
            // `agent`, and also runs as a subcommand of the cursor binary.
            "cursor-agent" | "agent" => Some(Self::CursorAgent),
            "cursor" if tokens.next() == Some("agent") => Some(Self::CursorAgent),
            "droid" => Some(Self::Droid),
            "grok" => Some(Self::Grok),
            "cline" => Some(Self::Cline),
            "plandex" => Some(Self::Plandex),
            "auggie" => Some(Self::Auggie),
            "autohand" => Some(Self::Autohand),
            "codebuddy" => Some(Self::CodeBuddy),
            "codebuff" => Some(Self::Codebuff),
            "command-code" => Some(Self::CommandCode),
            "cn" => Some(Self::Continue),
            "devin" => Some(Self::Devin),
            "freebuff" => Some(Self::Freebuff),
            "hermes" => Some(Self::Hermes),
            "jules" => Some(Self::Jules),
            "junie" => Some(Self::Junie),
            "kilo" | "kilocode" => Some(Self::Kilocode),
            "kimi" => Some(Self::Kimi),
            "kiro-cli" | "kiro" => Some(Self::Kiro),
            "letta" => Some(Self::Letta),
            "mimo" => Some(Self::MiMoCode),
            "vibe" => Some(Self::MistralVibe),
            "muse" => Some(Self::Muse),
            "omp" => Some(Self::OhMyPi),
            "pi" => Some(Self::Pi),
            "prime-agent" => Some(Self::PrimeAgent),
            "qodercli" => Some(Self::Qoder),
            "rovodev" => Some(Self::Rovo),
            "zero" => Some(Self::Zero),
            _ => None,
        }
    }

    /// Whether this tool is EXPECTED to own the alternate screen.
    ///
    /// REPL (false) is the safe default: Ink-based CLIs (gemini, codex,
    /// copilot, qwen, cline, grok, amp) redraw in the NORMAL buffer and never
    /// enter the alt screen — classifying one as alt-screen leaves the pane
    /// waiting for a mode that never comes (no AI panel, clipped bottom rows).
    /// The reverse is harmless: if a REPL-classified tool genuinely enters
    /// the alt screen, `InputMode::on_alt_screen_entered` upgrades it to
    /// AiAltScreen automatically. Only tools VERIFIED to run full-screen
    /// (Claude Code; the bubbletea TUIs) are true here.
    pub fn is_alt_screen(self) -> bool {
        matches!(self, Self::Claude | Self::OpenCode | Self::Crush)
    }

    /// Delay between writing the prompt text and the CR that submits it
    /// (REPL-style tools only). Ink UIs treat a burst of bytes as a paste;
    /// the CR must arrive AFTER the paste has been processed or it is
    /// swallowed into the input instead of submitting (observed with Gemini:
    /// text landed in its input but never sent). Codex/Ollama are proven at
    /// 20ms; the heavier Ink apps get a comfortable margin.
    pub fn submit_cr_delay_ms(self) -> u64 {
        match self {
            Self::Codex | Self::Ollama => 20,
            _ => 160,
        }
    }

    /// Human-readable name for chips and sidebar labels.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Ollama => "Ollama",
            Self::Gemini => "Gemini CLI",
            Self::Copilot => "Copilot CLI",
            Self::Aider => "Aider",
            Self::OpenCode => "OpenCode",
            Self::Crush => "Crush",
            Self::Goose => "Goose",
            Self::Qwen => "Qwen Code",
            Self::Amp => "Amp",
            Self::CursorAgent => "Cursor Agent",
            Self::Droid => "Droid",
            Self::Grok => "Grok CLI",
            Self::Cline => "Cline",
            Self::Plandex => "Plandex",
            Self::Auggie => "Auggie",
            Self::Autohand => "Autohand Code",
            Self::CodeBuddy => "CodeBuddy Code",
            Self::Codebuff => "Codebuff",
            Self::CommandCode => "Command Code",
            Self::Continue => "Continue",
            Self::Devin => "Devin",
            Self::Freebuff => "Freebuff",
            Self::Hermes => "Hermes Agent",
            Self::Jules => "Jules",
            Self::Junie => "Junie",
            Self::Kilocode => "Kilocode",
            Self::Kimi => "Kimi",
            Self::Kiro => "Kiro (AWS)",
            Self::Letta => "Letta",
            Self::MiMoCode => "MiMo Code",
            Self::MistralVibe => "Mistral Vibe",
            Self::Muse => "Muse Code",
            Self::OhMyPi => "Oh My Pi",
            Self::Pi => "Pi",
            Self::PrimeAgent => "Prime Agent",
            Self::Qoder => "Qoder CLI",
            Self::Rovo => "Rovo Dev",
            Self::Zero => "Zero",
        }
    }

    /// Canonical CLI binary for this tool (the name typed to launch it).
    /// Used to resolve registry artwork/detection without hard-coding
    /// per-tool images at each call site. Mirrors the representatives in
    /// [`from_command`](Self::from_command): multi-spelling tools resolve
    /// to their primary binary here.
    pub fn cli(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Ollama => "ollama",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Aider => "aider",
            Self::OpenCode => "opencode",
            Self::Crush => "crush",
            Self::Goose => "goose",
            Self::Qwen => "qwen",
            Self::Amp => "amp",
            Self::CursorAgent => "cursor-agent",
            Self::Droid => "droid",
            Self::Grok => "grok",
            Self::Cline => "cline",
            Self::Plandex => "plandex",
            Self::Auggie => "auggie",
            Self::Autohand => "autohand",
            Self::CodeBuddy => "codebuddy",
            Self::Codebuff => "codebuff",
            Self::CommandCode => "command-code",
            Self::Continue => "cn",
            Self::Devin => "devin",
            Self::Freebuff => "freebuff",
            Self::Hermes => "hermes",
            Self::Jules => "jules",
            Self::Junie => "junie",
            Self::Kilocode => "kilo",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro-cli",
            Self::Letta => "letta",
            Self::MiMoCode => "mimo",
            Self::MistralVibe => "vibe",
            Self::Muse => "muse",
            Self::OhMyPi => "omp",
            Self::Pi => "pi",
            Self::PrimeAgent => "prime-agent",
            Self::Qoder => "qodercli",
            Self::Rovo => "rovodev",
            Self::Zero => "zero",
        }
    }

    /// Reverse of [`label`](Self::label): reconstruct the tool from the name a
    /// host broadcasts in its shared-session surface state, so a viewer's mirror
    /// can show the same AI-tool input panel the host shows.
    pub fn from_label(label: &str) -> Option<Self> {
        [
            Self::Claude,
            Self::Codex,
            Self::Ollama,
            Self::Gemini,
            Self::Copilot,
            Self::Aider,
            Self::OpenCode,
            Self::Crush,
            Self::Goose,
            Self::Qwen,
            Self::Amp,
            Self::CursorAgent,
            Self::Droid,
            Self::Grok,
            Self::Cline,
            Self::Plandex,
            Self::Auggie,
            Self::Autohand,
            Self::CodeBuddy,
            Self::Codebuff,
            Self::CommandCode,
            Self::Continue,
            Self::Devin,
            Self::Freebuff,
            Self::Hermes,
            Self::Jules,
            Self::Junie,
            Self::Kilocode,
            Self::Kimi,
            Self::Kiro,
            Self::Letta,
            Self::MiMoCode,
            Self::MistralVibe,
            Self::Muse,
            Self::OhMyPi,
            Self::Pi,
            Self::PrimeAgent,
            Self::Qoder,
            Self::Rovo,
            Self::Zero,
        ]
        .into_iter()
        .find(|t| t.label() == label)
    }
}

/// Single source of truth for what the pane's input layer is doing.
///
/// The state machine drives:
/// - Which bottom bar variant is rendered
/// - How keystrokes are routed
/// - Whether SIGWINCH is sent on resize
/// - What the sidebar tab label shows
///
/// All transitions go through `TerminalSession` methods — no component
/// mutates `InputMode` directly.
#[derive(Clone, PartialEq, Debug)]
pub enum InputMode {
    /// No command running.  User edits in the command editor.
    Idle,

    /// A command is running and is NOT an AI tool.
    /// Keystrokes in passthrough mode go directly to the PTY.
    Running {
        /// The command string, for display in the running bar.
        command: std::sync::Arc<str>,
    },

    /// An AI REPL (Codex, Ollama) is running in normal block mode.
    /// The AI panel replaces the input bar at the bottom.
    AiRepl {
        tool: AiTool,
        /// Whether the rich multi-line editor is expanded.
        panel_open: bool,
    },

    /// A full-screen TUI (vim, htop, man, etc.) is active.
    /// The block list is hidden; only the canvas is shown.
    AltScreen,

    /// Claude Code is active in alt-screen mode.
    /// Like `AltScreen` but renders the AI panel overlay.
    AiAltScreen { tool: AiTool, panel_open: bool },
}

impl InputMode {
    /// `true` while a non-alt-screen AI REPL (Codex/Ollama) is active.
    pub fn is_ai_repl(&self) -> bool {
        matches!(self, Self::AiRepl { .. })
    }

    /// Produce the mode that results from a command being submitted.
    /// Called by `TerminalSession` on `begin_command`.
    pub fn from_submitted_command(cmd: &str) -> Self {
        let arc: std::sync::Arc<str> = cmd.into();
        match AiTool::from_command(cmd) {
            Some(tool) if tool.is_alt_screen() => Self::AiAltScreen {
                tool,
                panel_open: false,
            },
            Some(tool) => Self::AiRepl {
                tool,
                panel_open: false,
            },
            None => Self::Running { command: arc },
        }
    }

    /// Produce the mode that results when the alt-screen is entered.
    pub fn on_alt_screen_entered(self) -> Self {
        match self {
            Self::AiAltScreen { .. } => self,
            Self::AiRepl { tool, .. } => Self::AiAltScreen {
                tool,
                panel_open: true,
            },
            _ => Self::AltScreen,
        }
    }

    /// Produce the mode that results when the alt-screen exits.
    pub fn on_alt_screen_exited(self) -> Self {
        match self {
            Self::AiAltScreen { tool, .. } => Self::AiRepl {
                tool,
                panel_open: false,
            },
            _ => Self::Idle,
        }
    }

    /// Produce the mode after a block completes (command exited).
    pub fn on_block_completed(self) -> Self {
        match self {
            // Some agents are classified as alternate-screen tools before
            // they actually enter it (e.g. Claude's first-run setup). A real
            // shell completion must return their input to the prompt too.
            Self::AiRepl { .. } | Self::AiAltScreen { .. } | Self::Running { .. } => Self::Idle,
            other => other,
        }
    }
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_new_cli_agents() {
        assert_eq!(AiTool::from_command("gemini"), Some(AiTool::Gemini));
        assert_eq!(AiTool::from_command("agy --continue"), Some(AiTool::Gemini));
        assert_eq!(AiTool::from_command("copilot"), Some(AiTool::Copilot));
        assert_eq!(
            AiTool::from_command("gh copilot suggest"),
            Some(AiTool::Copilot)
        );
        assert_eq!(AiTool::from_command("gh pr view"), None);
        assert_eq!(
            AiTool::from_command("aider --model gpt-5"),
            Some(AiTool::Aider)
        );
        assert_eq!(
            AiTool::from_command("cursor-agent"),
            Some(AiTool::CursorAgent)
        );
        assert_eq!(AiTool::from_command("agent"), Some(AiTool::CursorAgent));
        assert_eq!(
            AiTool::from_command("cursor agent --resume"),
            Some(AiTool::CursorAgent)
        );
        assert_eq!(AiTool::from_command("cursor ."), None);
        assert_eq!(AiTool::from_command("opencode ."), Some(AiTool::OpenCode));
        assert_eq!(AiTool::from_command("ls -la"), None);
        assert_eq!(AiTool::from_command("/opt/homebrew/bin/claude --resume"), Some(AiTool::Claude));
        assert_eq!(AiTool::from_command("~/.local/bin/codex"), Some(AiTool::Codex));
        assert_eq!(AiTool::from_command("/usr/bin/"), None);
    }

    /// Every agent the launcher can start (its CLI binary) is recognized, so
    /// each gets the same input panel: rich input, voice and attachments.
    #[test]
    fn every_launcher_agent_gets_the_ai_input_panel() {
        let launcher_binaries = [
            "claude", "codex", "gemini", "opencode", "amp", "copilot", "agy", "auggie", "autohand",
            "crush", "cline", "codebuddy", "codebuff", "command-code", "cn", "cursor-agent", "devin",
            "droid", "freebuff", "goose", "grok", "hermes", "jules", "junie", "kilo", "kimi",
            "kiro-cli", "letta", "mimo", "vibe", "muse", "omp", "pi", "prime-agent", "qodercli",
            "qwen", "rovodev", "zero",
        ];
        for binary in launcher_binaries {
            let tool = AiTool::from_command(binary).unwrap_or_else(|| panic!("{binary} isn't recognized"));
            // Its label and canonical binary round-trip, so a shared-session
            // viewer shows the same panel as the host.
            assert_eq!(AiTool::from_label(tool.label()), Some(tool), "{binary}");
            assert_eq!(AiTool::from_command(tool.cli()), Some(tool), "{binary}");
        }
        // Words that merely contain a tool's name are not tools.
        for other in ["pip install x", "pie", "zeroize", "continue", "kilometers", ""] {
            assert_eq!(AiTool::from_command(other), None, "{other}");
        }
    }

    #[test]
    fn repl_vs_alt_screen_classification() {
        // Ink-based CLIs live in the normal buffer — REPL treatment (the
        // gemini misclassification clipped its UI at the window bottom).
        for repl in [
            AiTool::Codex,
            AiTool::Ollama,
            AiTool::Aider,
            AiTool::Goose,
            AiTool::Plandex,
            AiTool::Gemini,
            AiTool::Qwen,
            AiTool::Copilot,
            AiTool::Cline,
            AiTool::Grok,
            AiTool::Amp,
            AiTool::CursorAgent,
            AiTool::Droid,
            AiTool::Hermes,
            AiTool::Kimi,
            AiTool::Pi,
        ] {
            assert!(!repl.is_alt_screen(), "{repl:?} should be a REPL");
        }
        for tui in [AiTool::Claude, AiTool::OpenCode, AiTool::Crush] {
            assert!(tui.is_alt_screen(), "{tui:?} should be alt-screen");
        }
    }

    #[test]
    fn submitted_command_routes_by_classification() {
        assert!(matches!(
            InputMode::from_submitted_command("aider"),
            InputMode::AiRepl {
                tool: AiTool::Aider,
                ..
            }
        ));
        assert!(matches!(
            InputMode::from_submitted_command("gemini"),
            InputMode::AiRepl {
                tool: AiTool::Gemini,
                ..
            }
        ));
        assert!(matches!(
            InputMode::from_submitted_command("claude"),
            InputMode::AiAltScreen {
                tool: AiTool::Claude,
                ..
            }
        ));
        // A REPL-classified tool that really enters the alt screen upgrades.
        let upgraded = InputMode::from_submitted_command("gemini").on_alt_screen_entered();
        assert!(matches!(
            upgraded,
            InputMode::AiAltScreen {
                tool: AiTool::Gemini,
                ..
            }
        ));
    }
}
