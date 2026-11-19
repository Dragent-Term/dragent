//! Per-agent conversation recovery, described as data.
//!
//! Agents resume differently (a flag, a subcommand, resume-or-create) and
//! expose their conversation id differently (a provider hook, an id Dragent
//! chooses before launch, or an id a provider command mints). One
//! [`SessionRecovery`] describes a single agent's mechanism plus the launch
//! grammar needed to replay its interactive options without replaying a
//! one-shot prompt. The launch helper and the restore path interpret it; no
//! provider is special-cased outside [`builtin`].
//!
//! An agent without an exact-identity mechanism has no descriptor. It is
//! never resumed by guessing a "latest" or per-directory conversation.

use serde::{Deserialize, Serialize};

/// Placeholder for the conversation id inside argument templates.
pub const ID: &str = "{id}";

/// How Dragent learns which provider conversation a launch belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionIdStrategy {
    /// A provider-native hook reports the id at startup and again whenever
    /// the user switches conversation inside the agent (`/clear`, `/resume`).
    Hook { hook: HookKind },
    /// Dragent chooses a UUID and passes it with `args` on a new launch.
    Preassigned { args: Vec<String> },
    /// `command` (arguments to the agent executable, run in the launch
    /// directory) prints a new conversation id; it is passed with `args`.
    Minted {
        command: Vec<String>,
        args: Vec<String>,
    },
}

/// Hook integrations the launch helper knows how to install per invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookKind {
    /// Claude Code: an invocation-local plugin with a `SessionStart` hook.
    ClaudePlugin,
    /// Codex: a trusted `SessionStart` hook passed through `-c` overrides.
    CodexSessionStart,
    /// OpenCode family: an inline plugin added through `config_env`.
    OpenCodePlugin { config_env: String },
    /// Agents that read hooks only from their user-level config. Dragent
    /// merges one entry into `file` (under `section`, if any); it resolves the
    /// reporting pane by process ancestry, so it is inert for agents started
    /// outside Dragent and needs no environment (some agents clear it).
    UserConfig {
        /// Path relative to `$HOME`, or to the XDG config dir when `xdg`.
        file: String,
        #[serde(default)]
        xdg: bool,
        /// Object key holding the event map; `None` when events are top level.
        #[serde(default)]
        section: Option<String>,
        /// JSON object used when the file does not exist yet.
        #[serde(default)]
        seed: Option<String>,
    },
}

/// What a positional (non-flag) launch argument means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Positionals {
    /// An initial prompt for the interactive session; dropped on resume.
    #[default]
    InteractivePrompt,
    /// A prompt that runs once and exits; the launch is not a session.
    OneShotPrompt,
    /// A path or subcommand whose meaning cannot be replayed safely.
    Unsupported,
}

/// Accepted shape of a conversation id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdFormat {
    /// Letters, digits, `-` and `_` (provider-native ids such as `ses_…`).
    #[default]
    Token,
    /// A UUID. Rejects pickers' search terms, `latest`, and list indices.
    Uuid,
}

/// One agent's recovery mechanism and launch grammar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecovery {
    pub strategy: SessionIdStrategy,
    /// Arguments resuming [`ID`]. A leading subcommand stays first; flags
    /// follow the preserved launch options.
    pub resume: Vec<String>,
    #[serde(default)]
    pub id_format: IdFormat,
    /// Flags whose value names the conversation this launch resumes.
    #[serde(default)]
    pub resume_flags: Vec<String>,
    /// Leading subcommands whose positional value names the resumed
    /// conversation (`codex resume <id>`).
    #[serde(default)]
    pub resume_subcommands: Vec<String>,
    /// Leading subcommands that branch another conversation; stripped with
    /// their optional id, the launch then records its own identity.
    #[serde(default)]
    pub fork_subcommands: Vec<String>,
    /// Flags whose value names a new conversation (`--session-id <uuid>`).
    #[serde(default)]
    pub id_flags: Vec<String>,
    /// Flags selecting an unnamed conversation (latest, picker, fork).
    /// Stripped and never replayed.
    #[serde(default)]
    pub continue_flags: Vec<String>,
    /// Flags whose value is an initial interactive prompt; stripped.
    #[serde(default)]
    pub prompt_flags: Vec<String>,
    /// Flags that make the launch non-interactive or otherwise not a
    /// resumable session (in addition to [`COMMON_ONE_SHOT`]).
    #[serde(default)]
    pub one_shot_flags: Vec<String>,
    /// Options that take a separate value; preserved together with it.
    #[serde(default)]
    pub value_flags: Vec<String>,
    /// Maintenance subcommands; such launches are never resumed.
    #[serde(default)]
    pub subcommands: Vec<String>,
    #[serde(default)]
    pub positionals: Positionals,
}

/// Informational or detached modes, never an interactive conversation.
pub const COMMON_ONE_SHOT: &[&str] = &[
    "--help",
    "-h",
    "--version",
    "-V",
    "-v",
    "--print",
    "--no-session-persistence",
    "--bare",
    "--bg",
    "--background",
    "--cloud",
    "--remote",
    "--attach",
    "--pure",
];

const CONTINUE: &[&str] = &["--continue", "--last", "--fork", "--fork-session"];

/// Value-taking options shared by the hook-integrated agents.
const HOOK_AGENT_VALUE_FLAGS: &[&str] = &[
    "--model",
    "-m",
    "--agents",
    "--effort",
    "--autocompact",
    "--fallback-model",
    "--client-data-url",
    "--system-prompt-file",
    "--append-system-prompt-file",
    "--allowed-tools",
    "--disallowed-tools",
    "--agent",
    "--permission-mode",
    "--settings",
    "--plugin-dir",
    "--setting-sources",
    "--allowedTools",
    "--disallowedTools",
    "--tools",
    "--add-dir",
    "--system-prompt",
    "--append-system-prompt",
    "--mcp-config",
    "--profile",
    "--sandbox",
    "--ask-for-approval",
    "--cd",
    "-C",
    "--config",
    "--enable",
    "--disable",
    "--local-provider",
    "--hostname",
    "--port",
    "--log-level",
];

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn joined(parts: &[&[&str]]) -> Vec<String> {
    parts.iter().flat_map(|p| strings(p)).collect()
}

fn blank(strategy: SessionIdStrategy, resume: &[&str]) -> SessionRecovery {
    SessionRecovery {
        strategy,
        resume: strings(resume),
        id_format: IdFormat::Token,
        resume_flags: Vec::new(),
        resume_subcommands: Vec::new(),
        fork_subcommands: Vec::new(),
        id_flags: Vec::new(),
        continue_flags: strings(CONTINUE),
        prompt_flags: Vec::new(),
        one_shot_flags: Vec::new(),
        value_flags: Vec::new(),
        subcommands: Vec::new(),
        positionals: Positionals::InteractivePrompt,
    }
}

fn opencode_family(config_env: &str) -> SessionRecovery {
    SessionRecovery {
        resume_flags: strings(&["--session", "-s"]),
        prompt_flags: strings(&["--prompt"]),
        continue_flags: joined(&[CONTINUE, &["-c"]]),
        value_flags: strings(HOOK_AGENT_VALUE_FLAGS),
        positionals: Positionals::Unsupported,
        ..blank(
            SessionIdStrategy::Hook {
                hook: HookKind::OpenCodePlugin {
                    config_env: config_env.into(),
                },
            },
            &["--session", ID],
        )
    }
}

/// Recovery for a built-in agent CLI (executable basename). `None` means the
/// agent offers no exact conversation identity Dragent can resume.
///
/// Agents deliberately absent, per their documented CLIs: Crush, Vibe,
/// Auggie, Hermes, Antigravity, Cline, Qoder, Letta and Amp resume by id but
/// give a launcher no way to choose or learn that id; Kiro, Continue, Rovo
/// Dev and others resume only "the latest" conversation.
pub fn builtin(executable: &str) -> Option<SessionRecovery> {
    Some(match executable {
        "claude" => SessionRecovery {
            resume_flags: strings(&["--resume", "-r"]),
            id_flags: strings(&["--session-id"]),
            continue_flags: joined(&[CONTINUE, &["-c"]]),
            one_shot_flags: strings(&["-p"]),
            value_flags: strings(HOOK_AGENT_VALUE_FLAGS),
            subcommands: strings(&[
                "auth", "mcp", "plugin", "plugins", "install", "update", "doctor", "agents",
                "attach", "logs", "stop", "rm",
            ]),
            ..blank(
                SessionIdStrategy::Hook {
                    hook: HookKind::ClaudePlugin,
                },
                &["--resume", ID],
            )
        },
        "codex" => SessionRecovery {
            resume_subcommands: strings(&["resume"]),
            fork_subcommands: strings(&["fork"]),
            value_flags: joined(&[HOOK_AGENT_VALUE_FLAGS, &["-c", "-p", "-s", "-a"]]),
            subcommands: strings(&[
                "exec",
                "e",
                "login",
                "logout",
                "mcp",
                "mcp-server",
                "app-server",
                "completion",
                "debug",
                "app",
                "sandbox",
                "apply",
            ]),
            ..blank(
                SessionIdStrategy::Hook {
                    hook: HookKind::CodexSessionStart,
                },
                &["resume", ID],
            )
        },
        "opencode" => opencode_family("OPENCODE_CONFIG_CONTENT"),
        "mimo" => opencode_family("MIMOCODE_CONFIG_CONTENT"),
        // `--session-id` starts a new conversation with that UUID;
        // `--resume` accepts an exact UUID (also `latest`/an index, rejected
        // by IdFormat::Uuid).
        "gemini" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_flags: strings(&["--resume", "-r"]),
            id_flags: strings(&["--session-id"]),
            prompt_flags: strings(&["-i", "--prompt-interactive"]),
            one_shot_flags: strings(&[
                "-p",
                "--prompt",
                "--acp",
                "--experimental-acp",
                "-l",
                "--list-extensions",
                "--list-sessions",
                "--delete-session",
                "--session-file",
                "-w",
                "--worktree",
            ]),
            value_flags: strings(&[
                "-m",
                "--model",
                "--approval-mode",
                "--policy",
                "--admin-policy",
                "--allowed-mcp-server-names",
                "--allowed-tools",
                "-e",
                "--extensions",
                "--include-directories",
                "-o",
                "--output-format",
            ]),
            subcommands: strings(&[
                "mcp",
                "extensions",
                "extension",
                "skills",
                "skill",
                "hooks",
                "hook",
                "gemma",
            ]),
            ..blank(
                SessionIdStrategy::Preassigned {
                    args: strings(&["--session-id", ID]),
                },
                &["--resume", ID],
            )
        },
        // Gemini-derived; a positional prompt is one-shot here.
        "qwen" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_flags: strings(&["--resume", "-r"]),
            id_flags: strings(&["--session-id"]),
            continue_flags: joined(&[CONTINUE, &["-c"]]),
            prompt_flags: strings(&["-i", "--prompt-interactive"]),
            one_shot_flags: strings(&[
                "-p",
                "--prompt",
                "--acp",
                "--experimental-acp",
                "--worktree",
                "--json-schema",
                "--input-format",
                "-o",
                "--output-format",
            ]),
            value_flags: strings(&[
                "-m",
                "--model",
                "--fallback-model",
                "--system-prompt",
                "--append-system-prompt",
                "--output-style",
                "--sandbox-image",
                "--approval-mode",
                "--channel",
                "--allowed-mcp-server-names",
                "--mcp-config",
                "--allowed-tools",
                "-e",
                "--extensions",
                "--openai-logging-dir",
                "--openai-api-key",
                "--openai-base-url",
                "--proxy",
                "--core-tools",
                "--exclude-tools",
                "--disabled-slash-commands",
                "--auth-type",
                "--max-session-turns",
                "--max-wall-time",
                "--max-tool-calls",
                "--max-subagent-depth",
                "--telemetry-target",
                "--telemetry-otlp-endpoint",
                "--telemetry-otlp-protocol",
                "--telemetry-outfile",
            ]),
            subcommands: strings(&[
                "auth",
                "board",
                "channel",
                "extensions",
                "hooks",
                "mcp",
                "review",
                "sandbox",
                "serve",
                "sessions",
                "update",
            ]),
            positionals: Positionals::OneShotPrompt,
            ..blank(
                SessionIdStrategy::Preassigned {
                    args: strings(&["--session-id", ID]),
                },
                &["--resume", ID],
            )
        },
        // `--session-id` resumes an existing id or names a new session;
        // resume uses the strict `--resume` so a lost conversation is
        // reported, never silently recreated.
        "copilot" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_flags: strings(&["--resume", "-r"]),
            id_flags: strings(&["--session-id"]),
            prompt_flags: strings(&["-i", "--interactive"]),
            one_shot_flags: strings(&[
                "-p",
                "--prompt",
                "--acp",
                "--connect",
                "--share",
                "--share-gist",
            ]),
            value_flags: strings(&[
                "--model",
                "--reasoning-effort",
                "--context",
                "--auto-tier",
                "--agent",
                "-n",
                "--name",
                "-C",
                "--log-dir",
                "--extension-sdk-path",
                "--log-level",
                "--stream",
                "--output-format",
                "--add-dir",
                "--attachment",
                "--disable-mcp-server",
                "--add-github-mcp-toolset",
                "--add-github-mcp-tool",
                "--plugin-dir",
                "--additional-mcp-config",
                "--max-autopilot-continues",
                "--mode",
                "--dynamic-retrieval",
                "--enable-mcp-server",
                "--max-ai-credits",
                "--usage-output-file",
            ]),
            subcommands: strings(&[
                "app",
                "login",
                "help",
                "init",
                "update",
                "version",
                "sessions",
                "memories",
                "plugin",
                "mcp",
                "skill",
                "instruction",
                "lsp",
                "completion",
            ]),
            positionals: Positionals::Unsupported,
            ..blank(
                SessionIdStrategy::Preassigned {
                    args: strings(&["--session-id", ID]),
                },
                &["--resume", ID],
            )
        },
        // Claude-Code-compatible CLI: `--session-id` must be a UUID.
        "codebuddy" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_flags: strings(&["--resume", "-r"]),
            id_flags: strings(&["--session-id"]),
            continue_flags: joined(&[CONTINUE, &["-c"]]),
            one_shot_flags: strings(&["-p"]),
            value_flags: strings(HOOK_AGENT_VALUE_FLAGS),
            subcommands: strings(&["mcp", "config", "update", "doctor", "install"]),
            ..blank(
                SessionIdStrategy::Preassigned {
                    args: strings(&["--session-id", ID]),
                },
                &["--resume", ID],
            )
        },
        // `--session <id>` resumes that session or creates it.
        "kimi" => SessionRecovery {
            resume_flags: strings(&["--session", "-S", "-r"]),
            continue_flags: joined(&[CONTINUE, &["-c"]]),
            one_shot_flags: strings(&["-p", "--prompt", "--output-format"]),
            value_flags: strings(&[
                "-m",
                "--model",
                "--skills-dir",
                "--agent",
                "--agent-file",
                "--add-dir",
            ]),
            subcommands: strings(&[
                "login",
                "acp",
                "web",
                "doctor",
                "export",
                "migrate",
                "upgrade",
                "install-desktop",
                "provider",
                "vis",
            ]),
            ..blank(
                SessionIdStrategy::Preassigned {
                    args: strings(&["--session", ID]),
                },
                &["--session", ID],
            )
        },
        // `create-chat` prints the id of a new empty chat.
        "cursor-agent" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_flags: strings(&["--resume"]),
            one_shot_flags: strings(&["-p", "--print", "--list-models", "-w", "--worktree"]),
            value_flags: strings(&[
                "--api-key",
                "-H",
                "--header",
                "-e",
                "--endpoint",
                "--output-format",
                "--mode",
                "--model",
                "--sandbox",
                "--workspace",
                "--add-dir",
                "--plugin-dir",
            ]),
            subcommands: strings(&[
                "persist",
                "install-shell-integration",
                "uninstall-shell-integration",
                "login",
                "logout",
                "mcp",
                "status",
                "whoami",
                "update",
                "upgrade",
                "create-chat",
                "generate-rule",
                "rule",
                "ls",
                "resume",
                "help",
            ]),
            ..blank(
                SessionIdStrategy::Minted {
                    command: strings(&["create-chat"]),
                    args: strings(&["--resume", ID]),
                },
                &["--resume", ID],
            )
        },
        // Hooks only from user config: `~/.factory/hooks.json` (events top
        // level). `-s/--session-id` continues an existing session. User-config
        // hooks run a POSIX shim and resolve panes through `ps`: Unix only.
        #[cfg(unix)]
        "droid" => SessionRecovery {
            resume_flags: strings(&["--resume", "-r", "--session-id", "-s"]),
            prompt_flags: strings(&["-f", "--file"]),
            one_shot_flags: strings(&["-w", "--worktree"]),
            value_flags: strings(&[
                "-m",
                "--model",
                "--auto",
                "--restrict-tools",
                "--additional-tools",
                "--disabled-tools",
                "--cwd",
                "--tag",
                "--append-system-prompt",
                "--append-system-prompt-file",
                "--settings",
            ]),
            subcommands: strings(&[
                "exec", "search", "find", "rules", "mcp", "plugin", "computer", "daemon", "update",
            ]),
            ..blank(
                SessionIdStrategy::Hook {
                    hook: HookKind::UserConfig {
                        file: ".factory/hooks.json".into(),
                        xdg: false,
                        section: None,
                        seed: None,
                    },
                },
                &["--resume", ID],
            )
        },
        // Hooks from the `hooks` block of the XDG `muse/settings.json`, run
        // with a cleared environment. `resume` takes an exact session UUID.
        #[cfg(unix)]
        "muse" => SessionRecovery {
            id_format: IdFormat::Uuid,
            resume_subcommands: strings(&["resume"]),
            prompt_flags: strings(&["--image"]),
            one_shot_flags: strings(&[
                "--no-session-log",
                "-w",
                "--worktree",
                "--worktree-existing",
            ]),
            value_flags: strings(&[
                "--agents",
                "--provider",
                "--preset",
                "--model",
                "--reasoning-effort",
                "--base-url",
                "--workspace",
                "--worktree-base",
                "--approval-mode",
                "--permission-profile",
                "--approval-judge",
                "--sandbox-network",
                "--echo-delay-ms",
            ]),
            subcommands: strings(&[
                "exec",
                "config",
                "export",
                "trace",
                "skills",
                "plugins",
                "sandbox",
                "schema",
                "serve",
                "session-message",
                "mcp",
                "auth",
                "login",
                "logout",
                "init",
            ]),
            ..blank(
                SessionIdStrategy::Hook {
                    hook: HookKind::UserConfig {
                        file: "muse/settings.json".into(),
                        xdg: true,
                        section: Some("hooks".into()),
                        seed: Some(r#"{"schema_version":1}"#.into()),
                    },
                },
                &["resume", ID],
            )
        },
        _ => return None,
    })
}

/// Executable basename without Windows launcher extensions.
pub fn executable_name(executable: &str) -> &str {
    executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
}

fn flag_name(arg: &str) -> &str {
    arg.split('=').next().unwrap_or(arg)
}

fn listed(list: &[String], flag: &str) -> bool {
    list.iter().any(|f| f == flag)
}

impl SessionRecovery {
    /// Whether `id` is an exact, shell-safe conversation id for this agent.
    pub fn valid_id(&self, id: &str) -> bool {
        let token = !id.is_empty()
            && id.len() <= 200
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        token
            && match self.id_format {
                IdFormat::Token => true,
                IdFormat::Uuid => uuid::Uuid::parse_str(id).is_ok(),
            }
    }

    /// The launch continues an existing (named or guessed) conversation.
    pub fn is_resume(&self, args: &[String]) -> bool {
        args.first()
            .is_some_and(|a| listed(&self.resume_subcommands, a))
            || args.iter().any(|a| {
                let flag = flag_name(a);
                listed(&self.resume_flags, flag) || listed(&self.continue_flags, flag)
            })
    }

    /// The exact conversation id named by the launch arguments, if any.
    pub fn selected_id(&self, args: &[String]) -> Option<String> {
        if args
            .first()
            .is_some_and(|a| listed(&self.resume_subcommands, a))
        {
            return args.get(1).filter(|s| self.valid_id(s)).cloned();
        }
        for (i, arg) in args.iter().enumerate() {
            for flag in self.resume_flags.iter().chain(&self.id_flags) {
                let value = if arg == flag {
                    args.get(i + 1).map(String::as_str)
                } else {
                    arg.strip_prefix(&format!("{flag}="))
                };
                if let Some(value) = value.filter(|s| self.valid_id(s)) {
                    return Some(value.to_owned());
                }
            }
        }
        None
    }

    /// Interactive launch options to replay on resume: one-shot prompts and
    /// conversation selection are removed. `None` when the launch is not a
    /// resumable interactive session.
    pub fn resume_options(&self, args: &[String]) -> Option<Vec<String>> {
        self.parse_launch(args).map(|launch| launch.options)
    }

    /// [`Self::resume_options`] plus whether the launch itself carried a
    /// prompt, which means the conversation was used from its first moment.
    pub fn parse_launch(&self, args: &[String]) -> Option<Launch> {
        let mut out = Vec::new();
        let mut prompted = false;
        let mut i = 0;
        if args.first().is_some_and(|a| {
            listed(&self.resume_subcommands, a) || listed(&self.fork_subcommands, a)
        }) {
            i = 1;
            if args.get(i).is_some_and(|a| !a.starts_with('-')) {
                i += 1;
            }
        }
        while let Some(arg) = args.get(i) {
            i += 1;
            if arg == "--" {
                if self.positionals == Positionals::OneShotPrompt && i < args.len() {
                    return None;
                }
                prompted |= i < args.len();
                break;
            }
            let flag = flag_name(arg);
            if listed(&self.resume_flags, flag)
                || listed(&self.id_flags, flag)
                || listed(&self.prompt_flags, flag)
            {
                let separate =
                    !arg.contains('=') && args.get(i).is_some_and(|a| !a.starts_with('-'));
                prompted |= listed(&self.prompt_flags, flag) && (separate || arg.contains('='));
                if separate {
                    i += 1;
                }
                continue;
            }
            if listed(&self.continue_flags, flag) {
                continue;
            }
            if COMMON_ONE_SHOT.contains(&flag) || listed(&self.one_shot_flags, flag) {
                return None;
            }
            if !arg.starts_with('-') {
                if listed(&self.subcommands, arg) {
                    return None;
                }
                match self.positionals {
                    // Omit the prompt, but keep options following it.
                    Positionals::InteractivePrompt => {
                        prompted = true;
                        continue;
                    }
                    Positionals::OneShotPrompt | Positionals::Unsupported => return None,
                }
            }
            out.push(arg.clone());
            if listed(&self.value_flags, flag) && !arg.contains('=') {
                out.push(args.get(i)?.clone());
                i += 1;
            }
        }
        Some(Launch {
            options: out,
            prompted,
        })
    }

    /// Arguments naming a new conversation `id` at launch (preassigned or
    /// minted strategies); empty for hook-reported identities.
    pub fn new_conversation_args(&self, id: &str) -> Vec<String> {
        match &self.strategy {
            SessionIdStrategy::Preassigned { args } | SessionIdStrategy::Minted { args, .. } => {
                substitute(args, id)
            }
            SessionIdStrategy::Hook { .. } => Vec::new(),
        }
    }

    /// Full argv resuming `id` with the preserved `options`.
    pub fn resume_argv(&self, executable: &str, options: &[String], id: &str) -> Vec<String> {
        let resume = substitute(&self.resume, id);
        let mut argv = vec![executable.to_owned()];
        if resume.first().is_some_and(|a| !a.starts_with('-')) {
            argv.extend(resume);
            argv.extend(options.iter().cloned());
        } else {
            argv.extend(options.iter().cloned());
            argv.extend(resume);
        }
        argv
    }
}

/// An interactive launch, as understood from its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// Options replayed on resume.
    pub options: Vec<String>,
    /// An initial prompt was given on the command line.
    pub prompted: bool,
}

fn substitute(template: &[String], id: &str) -> Vec<String> {
    template.iter().map(|a| a.replace(ID, id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        strings(items)
    }

    /// Every built-in with a descriptor on this platform.
    fn all() -> Vec<&'static str> {
        let mut all = vec![
            "claude",
            "codex",
            "opencode",
            "mimo",
            "gemini",
            "qwen",
            "copilot",
            "codebuddy",
            "kimi",
            "cursor-agent",
        ];
        if cfg!(unix) {
            all.extend(["droid", "muse"]);
        }
        all
    }

    #[test]
    fn every_builtin_resumes_an_exact_id_and_never_latest() {
        let id = "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51";
        for name in all() {
            let r = builtin(name).unwrap();
            assert!(r.valid_id(id), "{name}");
            assert!(!r.valid_id("wrong; injection"), "{name}");
            let argv = r.resume_argv(name, &args(&["--model", "m"]), id);
            assert!(argv.iter().any(|a| a == id), "{name}: {argv:?}");
            assert!(
                !argv
                    .iter()
                    .any(|a| a == "latest" || a == "--continue" || a == "--last"),
                "{name}: {argv:?}"
            );
            assert_eq!(argv[0], *name);
        }
    }

    #[test]
    fn uuid_agents_reject_pickers_indices_and_latest() {
        let mut uuid_agents = vec!["gemini", "qwen", "copilot", "codebuddy", "cursor-agent"];
        if cfg!(unix) {
            uuid_agents.push("muse");
        }
        for name in uuid_agents {
            let r = builtin(name).unwrap();
            for guess in ["latest", "5", "search-term"] {
                assert!(!r.valid_id(guess), "{name} accepted {guess}");
            }
        }
        let gemini = builtin("gemini").unwrap();
        assert_eq!(gemini.selected_id(&args(&["--resume", "latest"])), None);
        assert!(gemini.is_resume(&args(&["--resume", "latest"])));
    }

    #[test]
    fn new_conversations_get_an_identity_before_launch() {
        let id = "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51";
        for (name, expected) in [
            ("gemini", vec!["--session-id", id]),
            ("qwen", vec!["--session-id", id]),
            ("copilot", vec!["--session-id", id]),
            ("codebuddy", vec!["--session-id", id]),
            ("kimi", vec!["--session", id]),
            ("cursor-agent", vec!["--resume", id]),
        ] {
            let r = builtin(name).unwrap();
            assert_eq!(r.new_conversation_args(id), args(&expected), "{name}");
            // The injected argument is itself recognised as the selection.
            assert_eq!(r.selected_id(&args(&expected)).as_deref(), Some(id));
        }
        assert!(
            builtin("claude")
                .unwrap()
                .new_conversation_args(id)
                .is_empty()
        );
    }

    #[test]
    fn launch_grammar_keeps_options_and_drops_prompts() {
        let gemini = builtin("gemini").unwrap();
        assert_eq!(
            gemini.resume_options(&args(&[
                "--approval-mode=auto_edit",
                "fix the bug",
                "-m",
                "pro"
            ])),
            Some(args(&["--approval-mode=auto_edit", "-m", "pro"]))
        );
        assert_eq!(
            gemini.resume_options(&args(&["-i", "fix the bug", "--yolo"])),
            Some(args(&["--yolo"]))
        );
        assert!(gemini.resume_options(&args(&["-p", "one shot"])).is_none());
        assert!(gemini.resume_options(&args(&["mcp", "list"])).is_none());

        let qwen = builtin("qwen").unwrap();
        assert_eq!(
            qwen.resume_options(&args(&["--approval-mode=yolo", "-i", "task"])),
            Some(args(&["--approval-mode=yolo"]))
        );
        assert!(qwen.resume_options(&args(&["one shot prompt"])).is_none());

        let copilot = builtin("copilot").unwrap();
        assert_eq!(
            copilot.resume_options(&args(&["--allow-tool=write", "-i", "task", "--model", "x"])),
            Some(args(&["--allow-tool=write", "--model", "x"]))
        );
        assert!(copilot.resume_options(&args(&["-p", "one shot"])).is_none());

        let kimi = builtin("kimi").unwrap();
        assert!(kimi.resume_options(&args(&["-p", "one shot"])).is_none());
        assert_eq!(
            kimi.resume_options(&args(&["--auto", "--continue"])),
            Some(args(&["--auto"]))
        );

        let cursor = builtin("cursor-agent").unwrap();
        assert_eq!(
            cursor.resume_options(&args(&["--model", "gpt-5", "build it", "--force"])),
            Some(args(&["--model", "gpt-5", "--force"]))
        );
        assert!(cursor.resume_options(&args(&["-p", "x"])).is_none());
        assert!(cursor.resume_options(&args(&["ls"])).is_none());
    }

    #[test]
    fn resume_shape_follows_each_agent() {
        let id = "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51";
        let opts = args(&["--model", "m"]);
        assert_eq!(
            builtin("codex").unwrap().resume_argv("codex", &opts, id),
            args(&["codex", "resume", id, "--model", "m"])
        );
        assert_eq!(
            builtin("gemini").unwrap().resume_argv("gemini", &opts, id),
            args(&["gemini", "--model", "m", "--resume", id])
        );
        assert_eq!(
            builtin("kimi").unwrap().resume_argv("kimi", &opts, id),
            args(&["kimi", "--model", "m", "--session", id])
        );
    }

    #[test]
    fn descriptors_round_trip_for_custom_agents() {
        let custom = r#"{
            "strategy": {"kind": "preassigned", "args": ["--conversation", "{id}"]},
            "resume": ["--conversation", "{id}"],
            "id_format": "uuid",
            "resume_flags": ["--conversation"]
        }"#;
        let r: SessionRecovery = serde_json::from_str(custom).unwrap();
        assert_eq!(r.positionals, Positionals::InteractivePrompt);
        assert!(r.continue_flags.is_empty());
        let back: SessionRecovery =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        for name in all() {
            let r = builtin(name).unwrap();
            let back: SessionRecovery =
                serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
            assert_eq!(back, r, "{name}");
        }
    }

    #[test]
    fn unsupported_agents_have_no_descriptor() {
        // User-config hooks need a POSIX shell and `ps`.
        #[cfg(not(unix))]
        for name in ["droid", "muse"] {
            assert!(builtin(name).is_none(), "{name}");
        }
        for name in ["crush", "vibe", "auggie", "kiro-cli", "cn", "goose", "amp"] {
            assert!(builtin(name).is_none(), "{name}");
        }
    }

    #[test]
    fn launch_reports_an_initial_prompt() {
        let prompted = |name: &str, a: &[&str]| {
            builtin(name)
                .unwrap()
                .parse_launch(&args(a))
                .map(|launch| launch.prompted)
        };
        assert_eq!(prompted("claude", &["--model", "m"]), Some(false));
        assert_eq!(prompted("claude", &["fix the bug"]), Some(true));
        assert_eq!(prompted("codex", &["--", "fix the bug"]), Some(true));
        assert_eq!(prompted("codex", &["--"]), Some(false));
        assert_eq!(prompted("opencode", &["--prompt", "task"]), Some(true));
        assert_eq!(prompted("opencode", &["--prompt=task"]), Some(true));
        assert_eq!(prompted("copilot", &["-i", "task"]), Some(true));
        assert_eq!(prompted("qwen", &["one shot"]), None);
        #[cfg(unix)]
        {
            assert_eq!(prompted("droid", &["-f", "prompt.md"]), Some(true));
            assert_eq!(prompted("muse", &["--model", "x"]), Some(false));
            // Resuming names a conversation; it is not a new prompt.
            assert_eq!(
                prompted("muse", &["resume", "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51"]),
                Some(false)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn user_config_agents_resume_their_reported_id() {
        let id = "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51";
        let droid = builtin("droid").unwrap();
        assert_eq!(
            droid.resume_argv("droid", &args(&["--model", "m"]), id),
            args(&["droid", "--model", "m", "--resume", id])
        );
        assert!(droid.resume_options(&args(&["exec", "one shot"])).is_none());
        assert!(droid.is_resume(&args(&["-s", "session-abc"])));
        let muse = builtin("muse").unwrap();
        assert_eq!(
            muse.resume_argv("muse", &args(&["--model", "m"]), id),
            args(&["muse", "resume", id, "--model", "m"])
        );
        assert_eq!(
            muse.selected_id(&args(&["resume", id])).as_deref(),
            Some(id)
        );
        assert!(muse.resume_options(&args(&["--no-session-log"])).is_none());
        for recovery in [droid, muse] {
            assert!(matches!(
                recovery.strategy,
                SessionIdStrategy::Hook {
                    hook: HookKind::UserConfig { .. }
                }
            ));
            assert!(recovery.new_conversation_args(id).is_empty());
        }
    }
}
