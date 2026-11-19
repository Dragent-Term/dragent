//! Shared desktop/host launch request types.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// Show `path` in a window: select it if it's a workspace in the
    /// sidebar, else open the folder as a project.
    Open { path: PathBuf },
    /// A new workspace for `project` (a worktree, or the project's own
    /// checkout with `local`), with `agent` started on `prompt` in it.
    CreateWorkspace {
        project: PathBuf,
        /// Agent key (`claude`, `codex`, `dragent`, …). Absent: the app's
        /// current choice when there's a prompt, else just a terminal.
        agent: Option<String>,
        prompt: String,
        /// Name of the new branch; generated from the prompt when absent.
        branch: Option<String>,
        /// Branch to start from; the project's default when absent.
        base_branch: Option<String>,
        /// The workspace's name in the sidebar.
        #[serde(default)]
        name: Option<String>,
        /// Start from this pull request's head.
        #[serde(default)]
        pull_request: Option<u32>,
        /// Use the project's own checkout instead of a new worktree.
        #[serde(default)]
        local: bool,
        /// A shell command to run in the workspace once it's open.
        #[serde(default)]
        command: Option<String>,
        /// Sidebar folders to file it in.
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        model: Option<String>,
        /// Open it in the sidebar without switching to it (a team's
        /// workers start while the person works elsewhere).
        #[serde(default)]
        background: bool,
    },
    /// Another agent in an existing workspace, in a tab of its own.
    StartAgent {
        #[serde(default)]
        account: crate::agent_accounts::Selection,
        workspace: PathBuf,
        agent: Option<String>,
        prompt: String,
        model: Option<String>,
    },
    /// Close a workspace, removing its worktree (the command line already
    /// checked for unsaved work and running agents).
    DeleteWorkspace { path: PathBuf },
    /// The organisations the signed-in user belongs to.
    Organisations,
    /// Switch the app to an organisation (id, slug or name).
    SwitchOrganisation { organisation: String },
    /// The signed-in user.
    Whoami,
    /// The machines Dragent reaches over SSH, as the app sees them now:
    /// connected or not, the last error, what runs there.
    Machines,
    /// A plugin's MCP server entries changed: add or replace `upsert`
    /// (switched off unless already on and unchanged), remove `remove` (with
    /// their tokens and approvals), and withdraw the approval of `revoke`.
    PluginServers {
        #[serde(default)]
        upsert: Vec<crate::mcp_config::McpServerEntry>,
        #[serde(default)]
        remove: Vec<String>,
        #[serde(default)]
        revoke: Vec<String>,
    },
    /// Each named MCP server: configured, on, approved, connected, tools.
    PluginServerStatus { names: Vec<String> },
    /// Call one tool of MCP server `server` with `arguments`, through the
    /// person's approvals, to prove it works.
    PluginCheck {
        server: String,
        tool: String,
        #[serde(default)]
        arguments: serde_json::Value,
    },
    /// Change a setting (`None` resets it), as the Settings page would.
    Setting { key: String, value: Option<String> },
    /// Send feedback to the Dragent team as the signed-in user.
    Feedback { text: String },
    /// End the agent in pane `pane` by closing the tab that shows it.
    /// `data.tab_closed` is false when no tab in the window shows it.
    StopAgent { pane: String },
    /// Run one of the connected-service tools (`integrations::tools`) for
    /// a connection kept on Dragent's server, which only the app reaches.
    /// `data.output` is the tool's answer.
    IntegrationTool {
        tool: String,
        arguments: serde_json::Value,
        root: PathBuf,
    },
    /// `dragent work`, from the terminal of an agent CLI working on a work
    /// item task: the app finds the task by the terminal (session `pane`,
    /// host `terminal`). `data.output` is the answer.
    WorkTool {
        pane: Option<String>,
        terminal: Option<String>,
        tool: String,
        arguments: serde_json::Value,
    },
    /// `dragent browser`, from an agent's or a person's terminal: drive the
    /// browser beside that terminal (session `pane`, host `terminal`).
    /// `data.output` is the answer.
    BrowserTool {
        pane: Option<String>,
        terminal: Option<String>,
        tool: String,
        arguments: serde_json::Value,
    },
    /// `dragent open-url`, the `BROWSER` of Dragent's terminals: open a link
    /// the way a click in the app does (a web page in Dragent's browser
    /// unless the user turned that off; anything else in its system app).
    OpenLink { url: String },
    /// `dragent panes`, from an agent's or a person's terminal: list, open,
    /// focus or close the tabs and panes of that terminal's workspace
    /// (session `pane`, host `terminal`). `data.output` is the answer.
    PaneTool {
        pane: Option<String>,
        terminal: Option<String>,
        tool: String,
        arguments: serde_json::Value,
    },
    /// Computer use for the agent in that terminal: read or work a desktop
    /// app, within Settings → Computer use (on/off, allowed apps, asking
    /// first). `data.output` is the answer, `data.result` the same as data.
    ComputerTool {
        pane: Option<String>,
        terminal: Option<String>,
        tool: String,
        arguments: serde_json::Value,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The started agent's id (`dragent agents …` take it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Anything else the request returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Why it failed, as a status and code agents can act on. `error`
    /// keeps the message for older command lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<crate::outcome::Failure>,
}

impl Response {
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// A failure with its status and code.
    pub fn failure(failure: crate::outcome::Failure) -> Self {
        Self {
            error: Some(failure.message.clone()),
            failure: Some(failure),
            ..Self::default()
        }
    }

    pub fn done() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }

    pub fn with_data(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            ..Self::default()
        }
    }
}
