use std::any::Any;
use std::path::PathBuf;

use dragent_workspace_model::{
    AcceptanceCriterionId, ArtifactId, AttentionItemId, BuiltInLayout, DecisionId,
    ExecutionAttemptId, ExecutionReviewResolution, PlanTaskId, RequirementId, WorkContractId,
    WorkItemId,
};
use warpui::color::ColorU;
use warpui::elements::DropTargetData;

use super::sidebar::SidebarViewMode;

/// What "Anyone with the link" grants in the share sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAccess {
    /// Only people explicitly invited can join (the link alone grants nothing).
    None,
    /// Anyone with the link can watch.
    View,
    /// Anyone with the link can edit (still must sign in).
    Edit,
}

/// Agent chosen in the project prompt. The full CLI catalog stays visible even
/// when a binary is missing; installation status controls selection in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptAgent {
    Dragent,
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Amp,
    Copilot,
    Antigravity,
    Auggie,
    Autohand,
    Charm,
    Cline,
    CodeBuddy,
    Codebuff,
    CommandCode,
    Continue,
    Cursor,
    Devin,
    Droid,
    Freebuff,
    Goose,
    Grok,
    Hermes,
    Jules,
    Junie,
    Kilocode,
    Kimi,
    Kiro,
    Letta,
    MiMoCode,
    Mistral,
    Muse,
    OhMyPi,
    Pi,
    PrimeAgent,
    Qoder,
    Qwen,
    Rovo,
    Zero,
}

impl PromptAgent {
    pub const ALL: [PromptAgent; 39] = [
        PromptAgent::Dragent,
        PromptAgent::Claude,
        PromptAgent::Codex,
        PromptAgent::Gemini,
        PromptAgent::OpenCode,
        PromptAgent::Amp,
        PromptAgent::Copilot,
        PromptAgent::Antigravity,
        PromptAgent::Auggie,
        PromptAgent::Autohand,
        PromptAgent::Charm,
        PromptAgent::Cline,
        PromptAgent::CodeBuddy,
        PromptAgent::Codebuff,
        PromptAgent::CommandCode,
        PromptAgent::Continue,
        PromptAgent::Cursor,
        PromptAgent::Devin,
        PromptAgent::Droid,
        PromptAgent::Freebuff,
        PromptAgent::Goose,
        PromptAgent::Grok,
        PromptAgent::Hermes,
        PromptAgent::Jules,
        PromptAgent::Junie,
        PromptAgent::Kilocode,
        PromptAgent::Kimi,
        PromptAgent::Kiro,
        PromptAgent::Letta,
        PromptAgent::MiMoCode,
        PromptAgent::Mistral,
        PromptAgent::Muse,
        PromptAgent::OhMyPi,
        PromptAgent::Pi,
        PromptAgent::PrimeAgent,
        PromptAgent::Qoder,
        PromptAgent::Qwen,
        PromptAgent::Rovo,
        PromptAgent::Zero,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PromptAgent::Dragent => "Dragent",
            PromptAgent::Claude => "Claude Code",
            PromptAgent::Codex => "Codex",
            PromptAgent::Gemini => "Gemini CLI",
            PromptAgent::OpenCode => "OpenCode",
            PromptAgent::Amp => "Amp",
            PromptAgent::Copilot => "GitHub Copilot",
            PromptAgent::Antigravity => "Antigravity",
            PromptAgent::Auggie => "Auggie",
            PromptAgent::Autohand => "Autohand Code",
            PromptAgent::Charm => "Charm",
            PromptAgent::Cline => "Cline",
            PromptAgent::CodeBuddy => "CodeBuddy Code",
            PromptAgent::Codebuff => "Codebuff",
            PromptAgent::CommandCode => "Command Code",
            PromptAgent::Continue => "Continue",
            PromptAgent::Cursor => "Cursor",
            PromptAgent::Devin => "Devin",
            PromptAgent::Droid => "Droid",
            PromptAgent::Freebuff => "Freebuff",
            PromptAgent::Goose => "Goose",
            PromptAgent::Grok => "Grok",
            PromptAgent::Hermes => "Hermes Agent",
            PromptAgent::Jules => "Jules",
            PromptAgent::Junie => "Junie",
            PromptAgent::Kilocode => "Kilocode",
            PromptAgent::Kimi => "Kimi",
            PromptAgent::Kiro => "Kiro (AWS)",
            PromptAgent::Letta => "Letta",
            PromptAgent::MiMoCode => "MiMo Code",
            PromptAgent::Mistral => "Mistral Vibe",
            PromptAgent::Muse => "Muse Code",
            PromptAgent::OhMyPi => "Oh My Pi",
            PromptAgent::Pi => "Pi",
            PromptAgent::PrimeAgent => "Prime Agent",
            PromptAgent::Qoder => "Qoder CLI",
            PromptAgent::Qwen => "Qwen Code",
            PromptAgent::Rovo => "Rovo Dev",
            PromptAgent::Zero => "Zero",
        }
    }

    pub fn image_path(self, is_dark: bool) -> Option<&'static str> {
        match self {
            PromptAgent::Dragent => Some("icons/dragent-icon.png"),
            PromptAgent::Claude => Some("agent-images/agents/claude.svg"),
            PromptAgent::Codex => Some(if is_dark {
                "agent-images/agents/codex-white.svg"
            } else {
                "agent-images/agents/codex.svg"
            }),
            PromptAgent::Gemini => Some("agent-images/agents/gemini.svg"),
            PromptAgent::OpenCode => Some(if is_dark {
                "agent-images/agents/opencode-white.svg"
            } else {
                "agent-images/agents/opencode.svg"
            }),
            PromptAgent::Amp => Some("agent-images/agents/amp.svg"),
            PromptAgent::Copilot => Some(if is_dark {
                "agent-images/agents/copilot-white.svg"
            } else {
                "agent-images/agents/copilot.svg"
            }),
            PromptAgent::Antigravity => Some("agent-images/agents/antigravity.svg"),
            PromptAgent::Auggie => Some(if is_dark {
                "agent-images/Auggie-white.svg"
            } else {
                "agent-images/Auggie.svg"
            }),
            PromptAgent::Autohand => Some(if is_dark {
                "agent-images/autohand-white.svg"
            } else {
                "agent-images/autohand.svg"
            }),
            PromptAgent::Charm => Some("agent-images/charm.png"),
            PromptAgent::Cline => Some("agent-images/cline.png"),
            PromptAgent::CodeBuddy => Some("agent-images/codebuddy.svg"),
            PromptAgent::Codebuff => Some("agent-images/codebuff.png"),
            PromptAgent::CommandCode => Some("agent-images/commandcode.svg"),
            PromptAgent::Continue => Some("agent-images/continue.png"),
            PromptAgent::Cursor => Some(if is_dark {
                "agent-images/cursor-white.svg"
            } else {
                "agent-images/cursor.svg"
            }),
            PromptAgent::Devin => Some(if is_dark {
                "agent-images/agents/devin-white.svg"
            } else {
                "agent-images/agents/devin.svg"
            }),
            PromptAgent::Droid => Some(if is_dark {
                "agent-images/agents/droid-white.svg"
            } else {
                "agent-images/agents/droid.svg"
            }),
            PromptAgent::Freebuff => Some("agent-images/codebuff.png"),
            PromptAgent::Goose => Some("agent-images/goose.png"),
            PromptAgent::Grok => Some(if is_dark {
                "agent-images/agents/grok-white.svg"
            } else {
                "agent-images/agents/grok.svg"
            }),
            PromptAgent::Hermes => Some("agent-images/agents/hermes.png"),
            PromptAgent::Jules => Some("agent-images/jules.svg"),
            PromptAgent::Junie => Some("agent-images/junie-color.png"),
            PromptAgent::Kilocode => Some("agent-images/kilocode.png"),
            PromptAgent::Kimi => Some(if is_dark {
                "agent-images/agents/kimi-white.svg"
            } else {
                "agent-images/agents/kimi.svg"
            }),
            PromptAgent::Kiro => Some("agent-images/agents/kiro.svg"),
            PromptAgent::Letta => Some(if is_dark {
                "agent-images/letta-white.svg"
            } else {
                "agent-images/letta.svg"
            }),
            PromptAgent::MiMoCode => Some(if is_dark {
                "agent-images/mimocode-white.svg"
            } else {
                "agent-images/mimocode.svg"
            }),
            PromptAgent::Mistral => Some("agent-images/agents/vibe.svg"),
            PromptAgent::Muse => Some("agent-images/agents/muse.png"),
            PromptAgent::OhMyPi | PromptAgent::Pi => Some(if is_dark {
                "agent-images/agents/pi-white.svg"
            } else {
                "agent-images/agents/pi.svg"
            }),
            PromptAgent::PrimeAgent => Some(if is_dark {
                "agent-images/prime-agent-white.svg"
            } else {
                "agent-images/prime-agent.svg"
            }),
            PromptAgent::Qoder => Some("agent-images/qoder.svg"),
            PromptAgent::Qwen => Some("agent-images/qwen.svg"),
            PromptAgent::Rovo => Some("agent-images/atlassian.png"),
            PromptAgent::Zero => Some("agent-images/zero.svg"),
        }
    }

    pub fn cli(self) -> Option<&'static str> {
        match self {
            PromptAgent::Dragent => None,
            PromptAgent::Claude => Some("claude"),
            PromptAgent::Codex => Some("codex"),
            PromptAgent::Gemini => Some("gemini"),
            PromptAgent::OpenCode => Some("opencode"),
            PromptAgent::Amp => Some("amp"),
            PromptAgent::Copilot => Some("copilot"),
            PromptAgent::Antigravity => Some("agy"),
            PromptAgent::Auggie => Some("auggie"),
            PromptAgent::Autohand => Some("autohand"),
            PromptAgent::Charm => Some("crush"),
            PromptAgent::Cline => Some("cline"),
            PromptAgent::CodeBuddy => Some("codebuddy"),
            PromptAgent::Codebuff => Some("codebuff"),
            PromptAgent::CommandCode => Some("command-code"),
            PromptAgent::Continue => Some("cn"),
            PromptAgent::Cursor => Some("cursor-agent"),
            PromptAgent::Devin => Some("devin"),
            PromptAgent::Droid => Some("droid"),
            PromptAgent::Freebuff => Some("freebuff"),
            PromptAgent::Goose => Some("goose"),
            PromptAgent::Grok => Some("grok"),
            PromptAgent::Hermes => Some("hermes"),
            PromptAgent::Jules => Some("jules"),
            PromptAgent::Junie => Some("junie"),
            PromptAgent::Kilocode => Some("kilo"),
            PromptAgent::Kimi => Some("kimi"),
            PromptAgent::Kiro => Some("kiro-cli"),
            PromptAgent::Letta => Some("letta"),
            PromptAgent::MiMoCode => Some("mimo"),
            PromptAgent::Mistral => Some("vibe"),
            PromptAgent::Muse => Some("muse"),
            PromptAgent::OhMyPi => Some("omp"),
            PromptAgent::Pi => Some("pi"),
            PromptAgent::PrimeAgent => Some("prime-agent"),
            PromptAgent::Qoder => Some("qodercli"),
            PromptAgent::Qwen => Some("qwen"),
            PromptAgent::Rovo => Some("rovodev"),
            PromptAgent::Zero => Some("zero"),
        }
    }

    pub fn command_key(self) -> &'static str {
        self.cli().unwrap_or("dragent")
    }

    pub fn install_command(self) -> Option<&'static str> {
        if cfg!(target_os = "windows") {
            self.windows_install_command()
        } else {
            self.unix_install_command()
        }
    }

    /// Official native installers run through Windows PowerShell explicitly,
    /// so they work whichever shell the installer terminal opens. The
    /// progress bar is off: Windows PowerShell's `Invoke-WebRequest` slows
    /// to a crawl redrawing it, and the installers download 50-100 MB.
    /// `Set-Variable` avoids a `$` the outer shell would expand.
    fn windows_install_command(self) -> Option<&'static str> {
        match self {
            PromptAgent::Claude => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://claude.ai/install.ps1 | iex""#,
            ),
            PromptAgent::Antigravity => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://antigravity.google/cli/install.ps1 | iex""#,
            ),
            PromptAgent::Cursor => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm 'https://cursor.com/install?win32=true' | iex""#,
            ),
            PromptAgent::Devin => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://static.devin.ai/cli/setup.ps1 | iex""#,
            ),
            PromptAgent::Droid => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://app.factory.ai/cli/windows | iex""#,
            ),
            PromptAgent::Goose => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; Invoke-WebRequest -Uri https://raw.githubusercontent.com/aaif-goose/goose/main/download_cli.ps1 -OutFile download_cli.ps1; .\download_cli.ps1; Remove-Item download_cli.ps1""#,
            ),
            PromptAgent::Grok => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://x.ai/cli/install.ps1 | iex""#,
            ),
            PromptAgent::Hermes => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://hermes-agent.nousresearch.com/install.ps1 | iex""#,
            ),
            PromptAgent::Junie => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://junie.jetbrains.com/install.ps1 | iex""#,
            ),
            PromptAgent::Kimi => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://code.kimi.com/kimi-code/install.ps1 | iex""#,
            ),
            PromptAgent::Kiro => Some(
                r#"powershell -NoProfile -ExecutionPolicy Bypass -Command "Set-Variable ProgressPreference SilentlyContinue; irm https://cli.kiro.dev/install.ps1 | iex""#,
            ),
            PromptAgent::Codex => Some("npm install -g @openai/codex"),
            PromptAgent::Gemini => Some("npm install -g @google/gemini-cli"),
            PromptAgent::OpenCode => Some("npm install -g opencode-ai"),
            PromptAgent::Amp => Some("npm install -g @ampcode/cli"),
            PromptAgent::Copilot => Some("npm install -g @github/copilot"),
            PromptAgent::Auggie => Some("npm install -g @augmentcode/auggie"),
            PromptAgent::Autohand => Some("npm install -g autohand-cli"),
            PromptAgent::Charm => Some("npm install -g @charmland/crush"),
            PromptAgent::Cline => Some("npm install -g cline"),
            PromptAgent::CodeBuddy => Some("npm install -g @tencent-ai/codebuddy-code"),
            PromptAgent::Codebuff => Some("npm install -g codebuff"),
            PromptAgent::CommandCode => Some("npm install -g command-code"),
            PromptAgent::Continue => Some("npm install -g @continuedev/cli"),
            PromptAgent::Freebuff => Some("npm install -g freebuff"),
            PromptAgent::Jules => Some("npm install -g @google/jules"),
            PromptAgent::Kilocode => Some("npm install -g @kilocode/cli"),
            PromptAgent::Letta => Some("npm install -g @letta-ai/letta-code"),
            PromptAgent::MiMoCode => Some("npm install -g @mimo-ai/cli"),
            PromptAgent::Pi => Some("npm install -g @earendil-works/pi-coding-agent"),
            PromptAgent::Qoder => Some("npm install -g @qoder-ai/qodercli"),
            PromptAgent::Qwen => Some("npm install -g @qwen-code/qwen-code"),
            PromptAgent::Zero => Some("npm install -g @gitlawb/zero"),
            _ => None,
        }
    }

    /// The installer for macOS and Linux, including remote machines, whatever
    /// this computer runs.
    pub fn unix_install_command(self) -> Option<&'static str> {
        match self {
            PromptAgent::Claude => Some("curl -fsSL https://claude.ai/install.sh | bash"),
            PromptAgent::Codex => Some("npm install -g @openai/codex"),
            PromptAgent::Gemini => Some("npm install -g @google/gemini-cli"),
            PromptAgent::OpenCode => Some("npm install -g opencode-ai"),
            PromptAgent::Amp => Some("npm install -g @ampcode/cli"),
            PromptAgent::Copilot => Some("npm install -g @github/copilot"),
            PromptAgent::Antigravity => Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
            PromptAgent::Auggie => Some("npm install -g @augmentcode/auggie"),
            PromptAgent::Autohand => Some("npm install -g autohand-cli"),
            PromptAgent::Charm => Some("npm install -g @charmland/crush"),
            PromptAgent::Cline => Some("npm install -g cline"),
            PromptAgent::CodeBuddy => Some("npm install -g @tencent-ai/codebuddy-code"),
            PromptAgent::Codebuff => Some("npm install -g codebuff"),
            PromptAgent::CommandCode => Some("npm install -g command-code"),
            PromptAgent::Continue => Some("npm install -g @continuedev/cli"),
            PromptAgent::Cursor => Some("curl https://cursor.com/install -fsS | bash"),
            PromptAgent::Devin => Some("curl -fsSL https://cli.devin.ai/install.sh | bash"),
            PromptAgent::Droid => Some("curl -fsSL https://app.factory.ai/cli | sh"),
            PromptAgent::Freebuff => Some("npm install -g freebuff"),
            PromptAgent::Goose => Some("curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | bash"),
            PromptAgent::Grok => Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
            PromptAgent::Hermes => Some("curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash"),
            PromptAgent::Jules => Some("npm install -g @google/jules"),
            PromptAgent::Junie => Some("curl -fsSL https://junie.jetbrains.com/install.sh | bash"),
            PromptAgent::Kilocode => Some("npm install -g @kilocode/cli"),
            PromptAgent::Kimi => Some("curl -LsSf https://code.kimi.com/install.sh | bash"),
            PromptAgent::Kiro => Some("curl -fsSL https://cli.kiro.dev/install | bash"),
            PromptAgent::Letta => Some("npm install -g @letta-ai/letta-code"),
            PromptAgent::MiMoCode => Some("npm install -g @mimo-ai/cli"),
            PromptAgent::Mistral => Some("curl -LsSf https://mistral.ai/vibe/install.sh | bash"),
            PromptAgent::Muse => Some("curl -fsSL https://dev.meta.ai/install.sh | bash"),
            PromptAgent::OhMyPi => Some("curl -fsSL https://omp.sh/install | sh"),
            PromptAgent::Pi => Some("npm install -g @earendil-works/pi-coding-agent"),
            PromptAgent::PrimeAgent => Some("curl -fsSL https://app.primeintellect.ai/prime-agent/install.sh | sh"),
            PromptAgent::Qoder => Some("npm install -g @qoder-ai/qodercli"),
            PromptAgent::Qwen => Some("npm install -g @qwen-code/qwen-code"),
            PromptAgent::Rovo => Some("acli rovodev auth login"),
            PromptAgent::Zero => Some("npm install -g @gitlawb/zero"),
            _ => None,
        }
    }

    pub fn from_saved(value: &str) -> Self {
        match value {
            "Dragent" => PromptAgent::Dragent,
            "Claude" => PromptAgent::Claude,
            "Codex" => PromptAgent::Codex,
            "Gemini" => PromptAgent::Gemini,
            "OpenCode" => PromptAgent::OpenCode,
            "Amp" => PromptAgent::Amp,
            "Copilot" => PromptAgent::Copilot,
            "Antigravity" => PromptAgent::Antigravity,
            "Auggie" => PromptAgent::Auggie,
            "Autohand" => PromptAgent::Autohand,
            "Charm" => PromptAgent::Charm,
            "Cline" => PromptAgent::Cline,
            "CodeBuddy" => PromptAgent::CodeBuddy,
            "Codebuff" => PromptAgent::Codebuff,
            "CommandCode" => PromptAgent::CommandCode,
            "Continue" => PromptAgent::Continue,
            "Cursor" => PromptAgent::Cursor,
            "Devin" => PromptAgent::Devin,
            "Droid" => PromptAgent::Droid,
            "Freebuff" => PromptAgent::Freebuff,
            "Goose" => PromptAgent::Goose,
            "Grok" => PromptAgent::Grok,
            "Hermes" => PromptAgent::Hermes,
            "Jules" => PromptAgent::Jules,
            "Junie" => PromptAgent::Junie,
            "Kilocode" => PromptAgent::Kilocode,
            "Kimi" => PromptAgent::Kimi,
            "Kiro" => PromptAgent::Kiro,
            "Letta" => PromptAgent::Letta,
            "MiMoCode" => PromptAgent::MiMoCode,
            "Mistral" => PromptAgent::Mistral,
            "Muse" => PromptAgent::Muse,
            "OhMyPi" => PromptAgent::OhMyPi,
            "Pi" => PromptAgent::Pi,
            "PrimeAgent" => PromptAgent::PrimeAgent,
            "Qoder" => PromptAgent::Qoder,
            "Qwen" => PromptAgent::Qwen,
            "Rovo" => PromptAgent::Rovo,
            "Zero" => PromptAgent::Zero,
            "None" => PromptAgent::Dragent,
            _ => PromptAgent::Claude,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMenu {
    Add,
    /// Coordinate agents: who leads, which agents it may use, and models.
    Team,
    LeadModel,
    Links(super::project_links::LinkKind),
    Agent,
    Device,
    Project,
    Worktree,
    Branch,
}

pub struct PromptWorktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Navigation request kinds shared by the editor context menu and workspace
/// LSP coordinator. These map to distinct LSP requests, not text searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageNavigationKind {
    Definition,
    TypeDefinition,
    Implementation,
    References,
}

/// The operation that should continue after a native folder selection (and,
/// when necessary, the workspace trust gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectFolderIntent {
    OpenFolder,
    NewWork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeRecordEditTarget {
    WorkBriefOutcome,
    WorkBriefSummary,
    NewRequirement,
    Requirement(RequirementId),
    NewDecision,
    Decision(DecisionId),
    /// Resolve an open decision inline with resolution text (sets status to
    /// Resolved via UpsertDecision). Distinct from Decision, which rewords
    /// the question and never resolves it.
    DecisionResolution(DecisionId),
    NewAcceptanceCriterion,
    AcceptanceCriterion(AcceptanceCriterionId),
    NewArtifact,
    Artifact(ArtifactId),
    NewPlanTask,
    PlanTask(PlanTaskId),
    RoleMission(WorkContractId),
    RoleCapabilities(WorkContractId),
    RoleTools(WorkContractId),
    RoleWriteScope(WorkContractId),
    RolePermissions(WorkContractId),
    RoleExpectedOutputs(WorkContractId),
    RoleInputArtifacts(WorkContractId),
    RoleSuccessCriteria(WorkContractId),
    RoleEscalationRules(WorkContractId),
    RoleExecutionLocation(WorkContractId),
    RoleModelRoute(WorkContractId),
    RoleBudget(WorkContractId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapePlanTaskMove {
    Up,
    Down,
}

/// Presentation route within one stable work-item editor surface. This is
/// deliberately independent from canonical lifecycle state and app layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemView {
    Conversation,
    Plan,
    Coordinate,
    Changes,
    Knowledge,
    Review,
    Agent,
}

impl WorkItemView {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Plan => "Plan",
            Self::Coordinate => "Coordinate",
            Self::Changes => "Changes",
            Self::Knowledge => "Knowledge",
            Self::Review => "Review",
            Self::Agent => "Agent",
        }
    }

    pub const fn persistence_key(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Plan => "plan",
            Self::Coordinate => "coordinate",
            Self::Changes => "changes",
            Self::Knowledge => "knowledge",
            Self::Review => "review",
            Self::Agent => "agent",
        }
    }

    pub fn from_persistence_key(key: Option<&str>) -> Self {
        match key {
            Some("plan") => Self::Plan,
            Some("coordinate") => Self::Coordinate,
            Some("changes") => Self::Changes,
            Some("knowledge") => Self::Knowledge,
            Some("review") => Self::Review,
            Some("agent") => Self::Agent,
            _ => Self::Conversation,
        }
    }
}

/// Which share-sheet dropdown is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareDropdown {
    /// The role selector next to the email-invite field.
    InviteRole,
    /// The "Anyone with the link" access selector.
    LinkAccess,
    /// A specific participant's access selector (keyed by participant id).
    Participant(String),
}

/// Which edge of a pane a tab was dropped on, used to pick split axis/position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// Payload carried by each pane-edge drop zone. `Draggable::on_drop` downcasts the
/// hovered `DropTarget`'s data to this to learn which pane + edge received the tab.
#[derive(Debug, Clone, Copy)]
pub struct PaneDropZone {
    pub target_tab_id: usize,
    pub side: DropSide,
}

impl DropTargetData for PaneDropZone {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceAgentTabId {
    pub work_item_id: WorkItemId,
    pub task_id: PlanTaskId,
    pub attempt_id: Option<ExecutionAttemptId>,
}

#[derive(Debug, Clone)]
pub enum WorkspaceAction {
    OpenPullRequests,
    /// Open the pull requests page on the review at `url`.
    OpenPullRequest {
        url: String,
    },
    ClosePullRequests,
    ToggleLayoutMenu,
    CloseLayoutMenu,
    SelectWorkspaceLayout(BuiltInLayout),
    CloseWorkItemTab,
    SelectWorkspaceAgentTab(WorkspaceAgentTabId),
    CloseWorkspaceAgentTab(WorkspaceAgentTabId),
    SelectWorkItemView(WorkItemView),
    /// Accept the shaped plan, move the work item to ReadyToExecute, and hand
    /// scheduler-eligible work to its configured execution authority.
    ApprovePlanAndCoordinate,
    ResolveWorkspaceAttention {
        attention_item_id: AttentionItemId,
        approve: bool,
    },
    /// Resolve attention with the human's own words (decision answers,
    /// failure next-steps). Empty responses fall back to the canned resolve
    /// path. Permission items never take this path: only Approved/Rejected
    /// unpark a tool call.
    ResolveWorkspaceAttentionWithResponse {
        attention_item_id: AttentionItemId,
        approve: bool,
        response: String,
    },
    /// Approve this tool call and auto-approve future consent requests for
    /// the same tool within this work item (session standing approval; each
    /// auto-approval still emits a canonical ResolveAttention event).
    AlwaysAllowWorkspaceTool {
        attention_item_id: AttentionItemId,
    },
    ToggleWorkspaceAttentionBell,
    /// Notify-only: a terminal pane's confirm state changed. Dirties the
    /// workspace view so the window-level confirm mirror renders in the
    /// same frame (view dirtiness is per-view; the terminal's own notify
    /// would otherwise leave the mirror a cycle behind the click).
    TerminalConfirmChanged,
    /// Pin a dropdown detail card (None returns to the request list).
    SelectWorkspaceAttentionItem {
        attention_item_id: Option<AttentionItemId>,
    },
    /// Switch to the item's work item and open its Coordinate screen.
    RouteToWorkspaceAttention {
        attention_item_id: AttentionItemId,
    },
    RestoreWorkspaceCheckpoint {
        attempt_id: ExecutionAttemptId,
    },
    ResolveWorkspaceExecutionReview {
        attempt_id: ExecutionAttemptId,
        resolution: ExecutionReviewResolution,
    },
    /// Open the inspectable runtime agent tab for one execution attempt.
    /// Read-only navigation: selects stable identities (task and attempt)
    /// and presents the Agent view. Never starts, pauses, or otherwise
    /// alters execution.
    OpenWorkspaceAgentTimeline {
        task_id: Option<PlanTaskId>,
        attempt_id: Option<ExecutionAttemptId>,
    },
    /// Expand or collapse the agent timeline to its full durable history.
    /// Presentation state only; canonical history is unaffected.
    ToggleWorkspaceAgentTimelineExpanded,
    /// Record an explicit, attributable human waiver for one approved
    /// acceptance criterion.
    WaiveWorkspaceCriterion {
        acceptance_criterion_id: AcceptanceCriterionId,
    },
    /// Cancel the active Coordinate attempt through workspace authority.
    CancelWorkspaceExecution,
    /// Pause an active attempt or resume a canonically paused work item.
    ToggleWorkspaceExecutionPause,
    /// Work Item Flow step 3: the user confirmed requirements AND clicked
    /// Approve Shape. Marks the work item approved and starts the shaping
    /// turn that creates the shape. Without this, turns stay in Gather mode
    /// and can never emit proposal JSON.
    ApproveShape,
    /// Back out of an armed ("Confirm remove") deletion without removing
    /// anything. Shown next to the confirm button while a remove is armed.
    CancelRemoveConfirm,
    /// Dismiss the coordination error card. Clears view-local error state
    /// only — canonical work, plan, and team state are untouched, so the next
    /// Start Coordination re-validates from scratch.
    DismissCoordinationError,
    /// Persist the current Shape composer body on the active work-item thread.
    SubmitShapeMessage,
    /// Stop the in-flight Principal turn. Bumps the request epoch so late
    /// background events are ignored — same Stop affordance as the terminal
    /// agent's send/stop button.
    StopPrincipalTurn,
    /// Toggle the Principal composer model picker (global model, same source
    /// as the terminal agent input — NOT the per-role route picker).
    ToggleShapeModelMenu,
    /// Copy a Principal conversation message to the clipboard (same copy
    /// affordance as the terminal agent's user/agent blocks).
    CopyShapeMessageText {
        text: String,
    },
    /// Select the global agent model for future Principal turns — same
    /// persistence path as the terminal agent's SetAgentModel.
    SelectShapeModel {
        model_id: String,
        effort: String,
    },
    /// Add/remove the same file context supported by the standard agent
    /// composer, but keep it scoped to the active Principal conversation.
    OpenShapeAttachmentPicker,
    AttachShapeFiles {
        paths: Vec<PathBuf>,
    },
    RemoveShapeAttachment {
        index: usize,
    },
    /// Start or stop dictation into the Principal conversation composer.
    ToggleShapeVoiceInput,
    /// Focus the Shape conversation with a natural-language custom-role draft.
    BeginDescribeRole,
    SelectProposedRole {
        work_contract_id: WorkContractId,
    },
    ToggleShapeRoleModelPicker {
        work_contract_id: WorkContractId,
    },
    SelectShapeRoleModel {
        work_contract_id: WorkContractId,
        model_route: Option<String>,
    },
    CloseProposedRole,
    ApproveProposedRole {
        work_contract_id: WorkContractId,
    },
    StartShapeRecordEdit {
        target: ShapeRecordEditTarget,
    },
    MoveShapePlanTask {
        task_id: PlanTaskId,
        direction: ShapePlanTaskMove,
    },
    RemoveShapePlanTask {
        task_id: PlanTaskId,
    },
    RemoveShapeRecord {
        target: ShapeRecordEditTarget,
    },
    SaveShapeRecordEdit,
    CancelShapeRecordEdit,
    /// Choose a project directory and open its work-item development surface.
    OpenFolder,
    ShowProjectClone,
    ShowProjectSsh,
    ShowMachines,
    Remote(super::remote_project::RemoteAction),
    AddProject(super::add_project::AddProjectAction),
    ConnectSshDestination {
        destination: super::ssh::SshDestination,
    },
    CloseProjectStartForm,
    SubmitProjectStartForm,
    ConnectGitHub,
    /// Navbar Review chevron dropdown.
    ToggleReviewMenu,
    CloseReviewMenu,
    /// Create a pull request for the active work (interactive `gh` terminal).
    CreatePullRequest,
    /// Push the active branch (interactive terminal; sets upstream when
    /// the branch has none).
    PushBranch,
    /// Stage (or unstage, toggling on the live index) a file when no
    /// terminal owns the git state (folder-only workspace).
    StageGitFile {
        rel_path: String,
    },
    /// Discard a file's changes when no terminal owns the git state.
    /// First click arms a two-click confirm in the panel; second executes.
    DiscardGitFile {
        rel_path: String,
    },
    /// Stage everything when no terminal owns the git state.
    StageAllGitFiles,
    /// Unstage everything when no terminal owns the git state.
    UnstageAllGitFiles,
    /// Discard all changes without a terminal (two-click arm in the panel).
    /// `staged_only` restores staged files; otherwise every tracked file.
    DiscardAllGitFiles {
        staged_only: bool,
    },
    /// Re-run the Review tab PR fetch immediately.
    RefreshReviewPr,
    OpenRecentProject {
        root: PathBuf,
    },
    /// Sidebar add-project menu: pick a parent folder, create a fresh
    /// "Untitled project" directory inside it, and open it.
    CreateNewProject,
    /// Sidebar add-project popover (window overlay, like the new-tab picker).
    ToggleAddProjectMenu,
    CloseAddProjectMenu,
    ShowAddProjectCloneForm,
    /// Submit the popover clone form (reads the clone URL editor).
    SubmitAddProjectClone,
    /// Centered create-project modal (name + location + git init).
    OpenCreateProject,
    CloseCreateProject,
    BrowseCreateProjectLocation,
    CreateProjectLocationSelected {
        location: PathBuf,
    },
    SubmitCreateProject,
    /// Centered new-worktree modal: branch prompt prefilled with the local
    /// branch, checked out as a subfolder under the managed worktrees root.
    ShowNewWorktreeForm,
    RenameWorktreeBranch {
        root: PathBuf,
        branch: String,
    },
    RefreshWorktreeDetails {
        root: PathBuf,
    },
    CloseNewWorktree,
    SubmitNewWorktree,
    /// Folder picked for opening is not a git checkout: initialize it and
    /// continue the open, or dismiss and stay on the start surface.
    ConfirmGitInit,
    DismissGitInit,
    /// Prompt-first project screen menus and submit.
    OpenPromptMenu {
        menu: PromptMenu,
    },
    ClosePromptMenus,
    ClearPromptAgentSearch,
    SelectPromptAgent {
        agent: PromptAgent,
    },
    /// The prompt goes to a team: the chosen agent leads it.
    TogglePromptTeam,
    SetPromptLeadModel {
        model: Option<String>,
    },
    BrowsePromptModelTab {
        tab: crate::teams::models::ModelTab,
    },
    PickPromptAgentModel {
        agent: PromptAgent,
        model: Option<String>,
    },
    TogglePromptModelFavorite {
        favorite: crate::settings::AgentModelFavorite,
    },
    /// An agent the team's lead may (or may no longer) give work to.
    TogglePromptTeamAgent {
        agent: PromptAgent,
    },
    /// How the team's lead weighs cost against strength.
    SetPromptTeamCost {
        cost: dragent_host::team::Cost,
    },
    OpenPromptAgentInstall {
        agent: PromptAgent,
    },
    /// Install a user-provided custom agent through the same installer
    /// modal/flow as built-ins, resolved from the agent registry.
    OpenCustomAgentInstall {
        agent_id: String,
    },
    ClosePromptAgentInstall,
    /// A click in the pop-up terminal sheet: its terminal takes the keyboard.
    FocusPopupTerminal,
    CheckPromptAgentInstall,
    /// The install sheet's "Retry with administrator rights" (a machine job
    /// that stopped for lack of permission).
    RetryRemoteToolElevated,
    SubmitPrompt,
    CancelReviewSetup,
    OpenProjectWorkspacePrompt {
        root: PathBuf,
    },
    ToggleProjectContextMenu {
        root: PathBuf,
    },
    CloseProjectContextMenu,
    OpenProjectInFinder {
        root: PathBuf,
    },
    NewProjectGroup {
        root: PathBuf,
    },
    CancelProjectGroup,
    RemoveProjectGroup {
        root: PathBuf,
        group: String,
    },
    SubmitProjectGroup,
    MoveWorkspaceToGroup {
        tab_id: usize,
        project_root: PathBuf,
        group: Option<String>,
    },
    ReorderWorkspaceTab {
        tab_id: usize,
        target_tab_id: usize,
        project_root: PathBuf,
        group: Option<String>,
    },
    /// Move a project in the sidebar onto `target_root`'s place.
    ReorderProject {
        root: PathBuf,
        target_root: PathBuf,
    },
    ImportProjectWorktrees {
        root: PathBuf,
    },
    ConfirmImportProjectWorktrees,
    CancelImportProjectWorktrees,
    HideProjectFromSidebar {
        root: PathBuf,
    },
    ShowProjectInSidebar {
        root: PathBuf,
    },
    RequestArchiveProject {
        root: PathBuf,
    },
    RequestDeleteProject {
        root: PathBuf,
    },
    CancelProjectRemoval,
    ConfirmProjectRemoval,
    OpenPromptAttachmentPicker,
    AttachPromptFiles {
        paths: Vec<PathBuf>,
    },
    OpenPromptLinkKind {
        kind: super::project_links::LinkKind,
    },
    SelectPromptLink {
        index: usize,
    },
    RemovePromptLink {
        index: usize,
    },
    RemovePromptAttachment {
        index: usize,
    },
    TogglePromptVoiceInput,
    SelectPromptCurrentCheckout,
    OpenPromptWorktree {
        path: PathBuf,
    },
    CreatePromptWorktreeFromBranch {
        branch: String,
    },
    TogglePromptRemote,
    ConnectPromptSsh,
    OpenNewWorktreeFromPrompt,
    OpenDragentRepo,
    /// Open the clone dialog with this repository URL filled in.
    CloneRepositoryFromUrl {
        url: String,
    },
    BrowseCloneLocation,
    CloneLocationSelected {
        parent: PathBuf,
    },
    CloneDestinationSelected {
        parent: PathBuf,
    },
    ProjectFolderSelected {
        root: PathBuf,
        intent: ProjectFolderIntent,
    },
    /// Enter the existing Work Item surface for the selected project.
    LaunchProjectWork,
    /// Start Dragent's current built-in terminal agent in the selected project.
    LaunchProjectDragent,
    /// Start a supported external coding agent in a normal project terminal.
    LaunchProjectCodex,
    LaunchProjectClaude,
    LaunchProjectOpenCode,
    LaunchProjectTerminal,
    /// Leave the project launcher without starting a process and reveal Files.
    BrowseProjectFiles,
    NewTab,
    /// Create a new top-level workspace tab (Worktree = workspace > Tab >
    /// Panes): the current surface parks verbatim and a fresh surface opens
    /// with one new pane-tab. The previous layout is never mutated.
    NewWorkspaceTab,
    /// [`Self::NewWorkspaceTab`] in the shell at `shell` (Windows offers Git
    /// Bash and PowerShell side by side).
    NewShellWorkspaceTab {
        shell: String,
    },
    /// A new workspace tab running `dragent --help`: the Dragent command
    /// line, ready to try.
    NewDragentCliTab,
    /// A new workspace tab holding a browser.
    NewBrowserWorkspaceTab,
    /// A browser pane-tab in the current workspace tab, optionally opening
    /// an address (pages that ask for a new window use this).
    NewBrowserTab {
        url: Option<dragent_workspace_model::BrowserUrl>,
    },
    /// Split the current pane and open a browser beside it.
    SplitActiveTabWithBrowser,
    /// A new work item (the earlier workflow, behind Settings → Agent →
    /// Work items): shape it with the lead agent, then a team of agents
    /// carries out the approved plan.
    NewWorkItemTab,
    /// The project's prompt screen with Team on: the request goes to a team
    /// led by the agent picked there.
    OpenTeamPrompt,
    /// Switch to another top-level workspace tab, parking the current
    /// surface (panes, splits, selection) and restoring the target's.
    /// Blocked while a worktree-removal close is in flight.
    SelectProjectWorkspace {
        workspace_id: usize,
    },
    CloseProjectWorkspace {
        workspace_id: usize,
    },
    /// Put the workspace's idle agents to sleep now.
    SleepProjectWorkspace {
        workspace_id: usize,
    },
    SelectWorkspaceTab {
        workspace_tab_id: usize,
    },
    /// Close a top-level workspace tab. Panes with foreground work route
    /// through `CloseWorkspaceTabConfirmed`; the last remaining tab resets
    /// to a fresh surface instead of disappearing.
    CloseWorkspaceTab {
        workspace_tab_id: usize,
    },
    /// Close a workspace tab after the running-process confirmation.
    CloseWorkspaceTabConfirmed {
        workspace_tab_id: usize,
    },
    /// Dismiss the workspace-tab close confirmation modal.
    CancelCloseWorkspaceTab,
    /// Open the inline rename editor for a workspace tab in the header strip.
    StartRenameWorkspaceTab {
        workspace_tab_id: usize,
    },
    /// Commit the header-strip rename editor text to the workspace tab name.
    CommitRenameWorkspaceTab {
        workspace_tab_id: usize,
    },
    /// Close the header-strip rename editor without renaming.
    CancelRenameWorkspaceTab,
    /// Move a workspace tab onto another's slot in the header strip (a
    /// drag and drop).
    MoveWorkspaceTabTo {
        workspace_tab_id: usize,
        target_workspace_tab_id: usize,
    },
    /// Move a workspace tab one slot left in the header strip order.
    MoveWorkspaceTabLeft {
        workspace_tab_id: usize,
    },
    /// Move a workspace tab one slot right in the header strip order.
    MoveWorkspaceTabRight {
        workspace_tab_id: usize,
    },
    /// Expand or collapse the agent quick-access rail beside the right
    /// sidebar (icons-only vs icon + name + install actions).
    SetAgentRailExpanded {
        expanded: bool,
    },
    /// Open an installed agent as a brand-new top-level workspace tab:
    /// parks the current surface, creates a fresh one, launches the agent
    /// there, and activates it. Uninstalled agents never reach this path —
    /// the rail expands to reveal their install action instead.
    OpenAgentWithAccount {
        agent_id: String,
        account: dragent_host::agent_accounts::Selection,
    },
    /// Show the account picker popover for a rail launch. Picking an option
    /// dispatches OpenAgentWithAccount; closing or launching without a pick
    /// falls back to system login via the shared resolver.
    OpenAgentAccountPicker {
        agent_id: String,
    },
    CloseAgentAccountPicker,
    OpenAgentInNewWorkspaceTab {
        /// Stable agent-registry id (`"claude"`, `"dragent"`,
        /// `"custom:<slug>"`, …). The handler resolves the definition, so
        /// built-ins and customs share this path.
        agent_id: String,
    },
    /// Open a brand-new OS window (empty workspace).
    NewWindow,
    /// Window caption controls (custom titlebar on Windows — the OS draws no
    /// native minimize/maximize/close buttons on a borderless window).
    MinimizeWindow,
    ToggleMaximizeWindow,
    CloseWindow,
    NewAgentTab,
    /// New terminal tab running a specific shell (full path), chosen from the
    /// "+" new-tab picker.
    NewTabWithShell(String),
    /// Close the editor pane (reserved layout id, no tab). Terminals and
    /// documents are untouched; the layout dissolves if nothing remains.
    RemoveEditorPane,
    /// A split divider moved: shift the divider's weight share by the pixel
    /// delta within the measured split extent. The tree owns weights; the
    /// renderer only converts.
    SplitDividerDragged {
        split_id: super::pane_layout::SplitId,
        divider_idx: usize,
        delta_px: f32,
        extent_px: f32,
    },
    SelectTab {
        tab_id: usize,
    },
    /// Click inside a split pane: mosaic-style focus move. Same project =
    /// focus + active-tab bookkeeping only (no work-item park/unpark, no
    /// work-surface or document yank). Anything else falls through to the
    /// full `SelectTab` path.
    FocusSplitPane {
        tab_id: usize,
    },
    CloseTab {
        tab_id: usize,
    },
    /// Close a tab after the running-process confirmation was accepted.
    CloseTabConfirmed {
        tab_id: usize,
    },
    /// Dismiss the tab-close confirmation modal.
    CancelCloseTab,
    CloseOtherTabs {
        tab_id: usize,
    },
    SplitActiveTabHorizontal,
    SplitActiveTabVertical,
    /// Split a specific split pane below itself (stacked). Selects the
    /// pane's tab first so the new tab inherits its live directory.
    SplitTabHorizontal {
        tab_id: usize,
    },
    /// Split a specific split pane beside itself (side-by-side).
    SplitTabVertical {
        tab_id: usize,
    },
    /// A sidebar tab drag has begun — show pane drop zones.
    BeginTabDrag,
    /// A sidebar tab drag ended without a valid drop — hide drop zones.
    EndTabDrag,
    /// A tab was dropped onto `target_tab_id`'s `side` edge: move that existing
    /// session into a split (no fresh shell).
    DropTabIntoSplit {
        src_tab_id: usize,
        target_tab_id: usize,
        side: DropSide,
    },
    /// Pop a pane OUT of the current split without closing it — the tab keeps its
    /// live session and becomes a standalone sidebar tab.
    RemoveTabFromSplit {
        tab_id: usize,
    },
    CloseActive,
    SwitchTabNext,
    SwitchTabPrev,
    ToggleLeftPanel,
    ToggleRightPanel,
    TogglePortsPanel,
    /// Show or hide the host agents panel (agent runs across open projects).
    ToggleAgentsPanel,
    /// Dev-only: show or hide the performance overlay (frame time, memory,
    /// input latency). Also forced on by `DRAGENT_PERF_OVERLAY=1`.
    TogglePerfOverlay,
    /// Re-resolve host agent runs for every open project root.
    RefreshAgentRuns,
    /// Close the panel and bind a controlling mirror to this host run.
    FocusAgentRun {
        run_id: String,
    },
    /// Stop the host run, then refresh the panel rows.
    StopAgentRun {
        run_id: String,
    },
    /// Hide the agents panel without touching any run.
    CloseAgentsPanel,
    /// Keep or offer one agent in "Put idle agents to sleep?" (its pane).
    ToggleSleepAgent {
        pane: uuid::Uuid,
    },
    /// Put the ticked ones to sleep.
    ConfirmSleepAgents,
    /// Let them all run, and don't ask for a while.
    KeepAgentsAwake,
    /// Start an agent that waits for memory now (its pane or terminal).
    StartAgentAnyway {
        id: String,
    },
    /// Let an agent paused for its memory run again (its pane).
    ResumeAgent {
        id: String,
    },
    /// Close the tab of an agent that waits or is paused (its pane).
    StopHeldAgent {
        id: String,
    },
    ToggleLanguageServicesPanel,
    RestartLanguageServices,
    StopLanguageServices,
    RestartLanguageServer {
        server_id: String,
    },
    StopLanguageServer {
        server_id: String,
    },
    InstallLanguageServer {
        server_id: String,
    },
    TrustPendingWorkspace,
    DismissWorkspaceTrust,
    // Sidebar
    SetSidebarViewMode(SidebarViewMode),
    FocusSidebarSearch,
    SidebarSearchInput {
        text: String,
    },
    ToggleTabMenu {
        tab_id: usize,
    },
    CloseTabMenu,
    /// Start sharing a tab and open the share sheet.
    ShareSession {
        tab_id: usize,
    },
    /// Re-open the share sheet for an already-active session (link + QR).
    OpenShareSheet,
    /// Ask to stop sharing — opens the "End session?" confirmation.
    RequestStopSharing,
    /// Dismiss the "End session?" confirmation without ending.
    CancelStopSharing,
    /// Host picked what a participant should see when the session opens:
    /// `include_scrollback` = replay everything already in this terminal;
    /// otherwise start them on a clean screen (only output from now on).
    ConfirmShareScope {
        tab_id: usize,
        include_scrollback: bool,
    },
    /// Dismiss the share-scope chooser without sharing.
    CancelShareScope,
    /// Stop sharing and end the session (the confirmed action).
    StopSharing,
    /// Idle-warning modal: keep the shared session alive (resets the inactivity
    /// countdown and dismisses the warning).
    KeepSessionAlive,
    /// Idle-warning modal: end the shared session now, due to inactivity.
    EndIdleSession,
    /// Hide the share sheet (sharing continues).
    CloseShareSheet,
    /// Copy a value (the invite link) to the clipboard.
    CopyShareValue {
        value: String,
    },
    /// Copy a forwarded address and identify the matching local port in the
    /// confirmation message.
    CopyPortForwardValue {
        port: u16,
        value: String,
    },
    OpenPortForward {
        url: String,
    },
    /// Email an invite to the address currently in the sheet's email field, at
    /// the selected invite role.
    InviteEmail,
    /// Create a host-side loopback port forward from the port field.
    CreatePortForward,
    /// Stop a host-side loopback port forward.
    ClosePortForward {
        forward_id: String,
    },
    /// Set the role the email-invite field will grant (View vs Edit).
    SetInviteRole {
        control: bool,
    },
    /// Change what "Anyone with the link" grants (none / view / edit).
    SetLinkAccess {
        access: LinkAccess,
    },
    /// Change a participant's access, or remove them.
    SetParticipantAccess {
        participant: String,
        control: bool,
    },
    RemoveParticipant {
        participant: String,
    },
    /// Open / close the QR sub-sheet.
    OpenShareQr,
    CloseShareQr,
    /// Open a share-sheet dropdown (invite role / link access / a participant),
    /// or dismiss whichever is open.
    OpenShareDropdown {
        kind: ShareDropdown,
    },
    CloseShareDropdown,
    /// Host: approve or decline a pending access request.
    ResolveAccessRequest {
        request_id: String,
        approve: bool,
    },
    /// Join a shared session as a viewer — opens a mirror tab and connects.
    JoinSession {
        session: String,
        secret: String,
    },
    /// Viewer: ask the host to grant this participant Edit (control).
    RequestSessionEdit,
    /// Toggle the ⋮ share popover in the content header.
    ToggleContentHeaderMenu,
    /// Dismiss the ⋮ share popover.
    CloseContentHeaderMenu,
    /// Host: toggle the participants popover (opened from the presence stack).
    ToggleParticipantsMenu,
    /// Host: dismiss the participants popover.
    CloseParticipantsMenu,
    /// Host: open a participant's access sub-menu (View / Edit / Remove).
    OpenParticipantAccess {
        participant: String,
    },
    /// Host: dismiss the participant access sub-menu.
    CloseParticipantAccess,
    /// Dismiss the "sign in to share" prompt (Skip for now).
    CloseLoginPrompt,
    /// Open a URL in the user's browser (Terms / Privacy links).
    OpenExternalUrl {
        url: String,
    },
    /// Star Dragent's repository with the person's `gh` account, or open it
    /// on GitHub when that can't be done here.
    StarOnGitHub,
    /// Open Dragent's X account in the system browser.
    FollowOnX,
    StartRenameTab {
        tab_id: usize,
    },
    CommitRenameTab {
        tab_id: usize,
    },
    CancelRenameTab,
    MoveTabUp {
        tab_id: usize,
    },
    MoveTabDown {
        tab_id: usize,
    },
    SetTabColor {
        tab_id: usize,
        color: Option<ColorU>,
    },
    CopyTabTitle {
        tab_id: usize,
    },
    CopyTabCwd {
        tab_id: usize,
    },
    /// Submit a command to the active terminal pane (same as pressing Enter in the input bar).
    RunActivePaneCommand {
        command: String,
    },
    /// Open a project-relative file in the metadata-only read preview.
    OpenReadOnlyDocument {
        root: PathBuf,
        relative_path: PathBuf,
    },
    /// Promote the current project-search results into the main editor.
    OpenProjectSearchResults,
    /// Select a document that is already open in the editor tab strip.
    SelectOpenDocument {
        relative_path: PathBuf,
    },
    /// Move an open editor tab to the slot occupied by another tab. The
    /// document buffers remain untouched; only their presentation order moves.
    ReorderDocumentTab {
        source_path: PathBuf,
        target_path: PathBuf,
    },
    /// Close one document tab without closing the editor surface itself.
    CloseDocumentTab {
        relative_path: PathBuf,
    },
    /// Follow a link from a rendered Markdown document. Project files are
    /// opened in the editor; web URLs are delegated to the system browser.
    OpenMarkdownLink {
        source_path: PathBuf,
        target: String,
    },
    /// Open a file path detected in terminal output (a normal block, an
    /// agent card, or the alt-screen) in the project editor. `path` is
    /// already resolved to an absolute path against the originating block's
    /// cwd; `line`/`column` come from a `:line:col` stack-trace-style suffix
    /// when present (1-based, as printed by compilers/shells).
    OpenTerminalPath {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Attach the editor symbol at `cursor` (file + 1-based line range) to
    /// an agent's composer, like a file attach with a line number. Targets
    /// the active running agent (active tab preferred, else first running
    /// agent), falling back to a fresh Dragent tab when none is running.
    AttachEditorSymbolToAgent {
        path: PathBuf,
        line: u32,
        end_line: u32,
        symbol_name: String,
    },
    /// Same as above but always opens a brand-new Dragent chat carrying the
    /// symbol, never reusing a running agent.
    AttachEditorSymbolToNewAgent {
        path: PathBuf,
        line: u32,
        end_line: u32,
        symbol_name: String,
    },
    /// Replace an unavailable host session in the same pane with a new one:
    /// resume the pane's recorded agent conversation, or with `fresh` start
    /// a new one (re-running the command that was running there).
    RestartHostSession {
        pane_uuid: uuid::Uuid,
        fresh: bool,
    },
    OpenBufferSearch {
        seed_selection: bool,
    },
    CloseBufferSearch,
    BufferSearchNext,
    BufferSearchPrevious,
    ToggleBufferSearchCaseSensitive,
    ToggleBufferSearchWholeWord,
    ToggleBufferSearchRegex,
    ToggleBufferSearchReplace,
    BufferSearchReplaceCurrent,
    BufferSearchReplaceAll,
    /// Switch a Markdown document between editable source and rendered preview.
    ToggleMarkdownPreview,
    /// Toggle the active terminal pane below the open document editor.
    ToggleDocumentTerminal,
    /// Create another terminal in the dock, inheriting the selected dock terminal's CWD.
    NewDockedTerminal,
    SelectDockedTerminal {
        terminal_id: usize,
    },
    CloseDockedTerminal {
        terminal_id: usize,
    },
    /// Create a terminal beside the selected dock terminal, using the same CWD.
    SplitDockedTerminal,
    /// Switch the document terminal between its docked height and the full editor area.
    ToggleDockedTerminalMaximized,
    CloseOpenDocument,
    SaveOpenDocumentAndClose,
    DiscardOpenDocumentAndClose,
    CancelCloseOpenDocument,
    ToggleProjectDirectory {
        relative_path: PathBuf,
    },
    ExplorerCopyEntry {
        relative_path: PathBuf,
    },
    ExplorerCutEntry {
        relative_path: PathBuf,
    },
    ExplorerPasteInto {
        relative_directory: PathBuf,
    },
    ExplorerDuplicateEntry {
        relative_path: PathBuf,
    },
    ExplorerCopyPath {
        relative_path: PathBuf,
        absolute: bool,
    },
    ExplorerStartCreate {
        relative_parent: PathBuf,
        directory: bool,
    },
    ExplorerStartRename {
        relative_path: PathBuf,
    },
    ExplorerCommitPrompt,
    ExplorerCancelPrompt,
    ExplorerRequestDelete {
        relative_path: PathBuf,
        directory: bool,
    },
    ExplorerConfirmDelete,
    ExplorerCancelDelete,
    ExplorerDismissError,
    ExplorerRefresh,
    SaveOpenDocument,
    RequestLanguageCompletion {
        cursor: usize,
    },
    /// Request a language-server navigation result for the active document.
    RequestLanguageNavigation {
        cursor: usize,
        kind: super::LanguageNavigationKind,
    },
    /// Resolve a modifier-hover symbol without navigating yet. `string_link`
    /// carries the import specifier when the hovered range is a path-like
    /// string literal, so the file can be resolved even when the language
    /// server returns no definition for the specifier itself.
    RequestDefinitionHover {
        request_id: u64,
        cursor: usize,
        symbol_range: std::ops::Range<usize>,
        string_link: Option<String>,
    },
    /// Navigate using the validated result cached for this modifier hover.
    OpenDefinitionHover {
        request_id: u64,
        cursor: usize,
    },
    /// Request document formatting from the active language server.
    RequestLanguageFormatting,
    /// Request code actions for the clicked editor position.
    RequestLanguageCodeActions {
        cursor: usize,
    },
    /// Start an LSP rename flow for the clicked editor position.
    StartLanguageRename {
        cursor: usize,
    },
    /// Submit the replacement name from the document rename field.
    RequestLanguageRename {
        cursor: usize,
        new_name: String,
    },
    /// Close the document rename field without sending an LSP request.
    CancelLanguageRename,
    /// Open the right sidebar and select either the file explorer or the
    /// source-control workspace. Unlike ToggleRightPanel, this is idempotent:
    /// selecting an already-active tab still reveals a previously closed panel.
    SetProjectSidebarTab {
        tab: super::project_sidebar::SidebarTab,
    },
    /// Pull the active terminal repository's current branch from origin.
    PullActiveGitBranch,
    /// Push the active terminal repository's current branch to origin.
    PushActiveGitBranch,
    /// Open a file's unified diff (working-tree or staged) in the editor area.
    OpenGitDiff {
        rel_path: String,
        staged: bool,
    },
    /// Open one aggregate worktree-vs-HEAD diff document.
    OpenGitWorktreeDiff,
    RefreshDiffCommentAgents,
    SendDiffComment,
    ShowErrorNotice {
        title: String,
        message: String,
    },
    DismissErrorNotice,
    CopyErrorNotice,
    /// Open the immutable candidate changes captured for one agent execution.
    OpenWorkspaceAttemptChanges {
        attempt_id: ExecutionAttemptId,
    },
    /// Open the patch and metadata for a historical commit.
    OpenGitCommitDiff {
        sha: String,
        short_sha: String,
    },
    /// Commit everything staged, using the message typed in the
    /// source-control panel. `push` also pushes the branch afterwards.
    CommitGitStaged {
        message: String,
        push: bool,
    },
    /// Generate a commit-message suggestion from staged, unstaged, and
    /// untracked changes without modifying the index.
    GenerateGitCommitMessage,
    /// Open a language-server location selected from the result list.
    OpenLanguageLocation {
        root: PathBuf,
        relative_path: PathBuf,
        range: dragent_editor::TextRange,
    },
    /// Apply a code action selected from the language-server result list.
    ApplyLanguageCodeAction {
        action: serde_json::Value,
    },
    /// Close language-server result popovers.
    DismissLanguageResults,
    /// Open the terminal/workspace workflow command palette.
    OpenCommandPalette,
    RevertOpenDocument,
    ApproveOpenDocument,
    RejectOpenDocument,
    CheckpointOpenDocument,
    BlurAll,
    /// Open/close the session-type picker that appears below the + button in the header.
    ToggleNewTabPicker,
    /// Open/close the session-type picker anchored beside the sidebar "New session" button.
    ToggleNewTabPickerFromSidebar,
    /// Open the ⋮ share menu for a split pane (anchored to that pane).
    ToggleSplitShareMenu {
        tab_id: usize,
    },
    CloseNewTabPicker,
    OpenSettings,
    CloseSettings,
    /// Show / leave the Automations page.
    OpenAutomations,
    CloseAutomations,
    /// The Teams page, on one team when it's named.
    OpenTeams {
        team: Option<String>,
    },
    /// A coordination's agent, from its sidebar card: its tab, else its
    /// workspace.
    OpenCoordinationPane {
        pane: String,
        workspace: Option<std::path::PathBuf>,
    },
    /// The organisation's Tasks page.
    OpenTasks,
    /// Leave out the task being started from the new-workspace prompt.
    ClearPromptTask,
    /// An automation result in the navbar's notifications: show its
    /// automation on the Automations page, which also clears the notification.
    OpenAutomationNotice {
        run_id: String,
        automation_id: String,
    },
    /// Everything automations came back with: the page's inbox.
    OpenAutomationInbox,
    /// Open one automation (a `<scheme>://automations/<id>` link).
    OpenAutomationLink(String),
    /// Show one automation run (a clicked run alert).
    OpenAutomationRun(String),
    SignInFromHeader,
    /// Toggle the signed-in account menu hanging off the header avatar.
    ToggleAccountMenu,
    CloseAccountMenu,
    /// The header Usage popover: agents, their accounts and plan limits.
    ToggleUsageMenu,
    CloseUsageMenu,
    /// Show one agent's tab in the usage popover.
    SelectUsageAgent(PromptAgent),
    /// Earlier agent conversations in this folder, and continuing one.
    OpenPastSessions,
    ClosePastSessions,
    ClearPastSessionsSearch,
    /// Delete a listed conversation for good: ask, then do it.
    AskDeletePastSession(usize),
    CancelDeletePastSession,
    DeletePastSession(usize),
    /// Give the space deleted conversations left inside an agent's database
    /// (OpenCode, MiMo) back to the disk.
    CompactAgentHistory(String),
    ResumePastSession(usize),
    /// Sign out from the header account menu or the sidebar organisation
    /// area: asks for confirmation first unless the user opted out.
    SignOutFromHeader,
    /// "Log out?" confirmation: confirm, cancel, and "Don't ask again".
    ConfirmSignOut,
    #[cfg(not(target_family = "wasm"))]
    AnswerComputerAsk {
        request_id: String,
        allow: bool,
        remember: bool,
    },
    CancelSignOut,
    ToggleLogoutDontAsk,
    /// Open the hosted documentation in the browser.
    OpenDocumentation,
    /// Upgrade / manage plan: Settings → Billing & Usage.
    OpenBillingFromHeader,
    /// Sidebar organisation area: open/close the organisation menu.
    ToggleOrgMenu,
    /// Close the organisation menu, its submenus, and the Help menu.
    CloseOrgMenu,
    /// Hovering an organisation-menu row opens its submenu, or closes any.
    HoverOrgSubmenu(Option<super::org_menu::OrgSubmenu>),
    /// Open/close the sidebar Help menu.
    ToggleHelpMenu,
    /// Make `id` the user's active organisation. The app then switches its
    /// local data to that organisation.
    SwitchOrganisation {
        id: String,
    },
    /// Settings → Members with the new-organisation form open.
    CreateOrganisation,
    /// Open Settings on a specific page.
    OpenSettingsSection(crate::settings::SettingsSection),
    /// Give feedback dialog.
    OpenFeedback,
    /// Focus the feedback text (a click below its first line).
    FocusFeedbackDetails,
    CloseFeedback,
    SubmitFeedback,
    ToggleFeedbackDiagnostics,
    AttachFeedbackImages,
    RemoveFeedbackAttachment {
        index: usize,
    },
    /// Check for an app update, or restart when an update is ready.
    UpdateAppFromHeader,
    /// Stage a single hunk.
    StageGitHunk {
        rel_path: String,
        hunk_index: usize,
        old_start: usize,
        new_start: usize,
        /// false: INDEX → WORKTREE (stage); true: HEAD → INDEX (unstage).
        staged: bool,
    },
    /// Request discard of a single hunk — shows confirmation sheet.
    RequestDiscardGitHunk {
        rel_path: String,
        hunk_index: usize,
        old_start: usize,
        new_start: usize,
    },
    ConfirmDiscardGitHunk,
    CancelDiscardGitHunk,
}

#[cfg(test)]
mod work_item_view_tests {
    use super::WorkItemView;

    #[test]
    fn work_item_view_persistence_keys_round_trip() {
        for view in [
            WorkItemView::Conversation,
            WorkItemView::Plan,
            WorkItemView::Coordinate,
            WorkItemView::Changes,
            WorkItemView::Knowledge,
            WorkItemView::Review,
            WorkItemView::Agent,
        ] {
            assert_eq!(
                WorkItemView::from_persistence_key(Some(view.persistence_key())),
                view
            );
        }
        assert_eq!(
            WorkItemView::from_persistence_key(Some("unknown-future-view")),
            WorkItemView::Conversation
        );
    }
}
