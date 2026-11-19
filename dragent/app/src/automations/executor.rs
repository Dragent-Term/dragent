//! Executing one claimed run: prepare its workspace, then hand the prompt to
//! its agent — the same worktree and launch path a prompt typed into the
//! project composer takes, minus the window. A terminal agent keeps running
//! in a host terminal; a window then puts the workspace in its sidebar and
//! attaches a tab to it. Dragent's own agent lives in a window pane, so
//! those runs finish there instead (see [`Outcome::NeedsWindow`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dragent_automations::{
    codes, copy_files, Run, RunError, RunStep, RunTerminal, Scheduler, Store, WorkspaceMode,
};

use crate::workspace::project_workspace_creation as creation;
use crate::workspace::PromptAgent;

/// Terminal size before any window attaches; resized on attach.
const HEADLESS_COLS: u16 = 120;
const HEADLESS_ROWS: u16 = 32;

/// Agent key for Dragent's own agent.
pub(crate) const DRAGENT_AGENT: &str = "dragent";

/// Leads the prompt a terminal agent gets: nobody watches a run, so it
/// shouldn't stop to ask. (Dragent's own agent gets this in its system
/// prompt instead.)
const UNATTENDED_PREFACE: &str = "This is a scheduled Dragent automation run and nobody is \
watching this session. Don't stop to ask questions or wait for confirmation: choose the sensible \
option, carry on, and end with a short summary of what you did and anything you assumed. If \
nothing needs anyone's attention, start that summary with \"Nothing to report\".\n\n";

/// What a terminal agent is sent for `prompt`.
fn agent_prompt(prompt: &str) -> String {
    format!("{UNATTENDED_PREFACE}{prompt}")
}

/// The catalog agent a stored key names, Dragent's own included.
pub(crate) fn prompt_agent(key: &str) -> Option<PromptAgent> {
    PromptAgent::ALL.into_iter().find(|agent| agent.command_key() == key)
}

pub(crate) fn agent_for_key(key: &str) -> Option<PromptAgent> {
    PromptAgent::ALL
        .into_iter()
        .find(|agent| *agent != PromptAgent::Dragent && agent.command_key() == key)
}

/// Flags that start a terminal agent without approval prompts. Empty when
/// none is known.
fn skip_approval_flags(agent: PromptAgent) -> &'static [&'static str] {
    use PromptAgent as A;
    match agent {
        A::Claude | A::CodeBuddy => &["--dangerously-skip-permissions", "--permission-mode bypassPermissions"],
        A::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
        A::Gemini | A::Qwen => &["--yolo", "-y", "--approval-mode=yolo", "--approval-mode yolo"],
        A::Copilot => &["--allow-all-tools", "--allow-all", "--yolo"],
        A::Cursor => &["--force", "-f"],
        A::Amp => &["--dangerously-allow-all"],
        A::Grok => &["--always-approve"],
        A::Kiro => &["--trust-all-tools"],
        A::Mistral => &["--auto-approve"],
        A::Devin => &["--permission-mode dangerous", "--permission-mode=dangerous"],
        A::Autohand => &["--unrestricted"],
        A::Continue | A::Kilocode => &["--auto"],
        A::Zero => &["--skip-permissions-unsafe"],
        A::Hermes | A::Cline | A::CommandCode | A::Letta | A::Qoder | A::Rovo => &["--yolo"],
        _ => &[],
    }
}

/// Whether `command` passes one of `flags` (each a word, or words, of its own).
fn has_flag(command: &str, flags: &[&str]) -> bool {
    let words = format!(" {} ", command.split_whitespace().collect::<Vec<_>>().join(" "));
    flags.iter().any(|flag| words.contains(&format!(" {flag} ")))
}

/// Whether `agent`, launched the way Settings → Agents says, works through a
/// run without stopping for an approval. `settings` is read once by the
/// caller (no PATH probes here, so it's cheap enough for a page refresh).
pub(crate) fn runs_unattended(agent: PromptAgent, settings: &crate::setup::UiSettings) -> bool {
    // Dragent's own agent never waits in a run; OpenCode and Pi allow their
    // tools out of the box.
    if matches!(
        agent,
        PromptAgent::Dragent | PromptAgent::OpenCode | PromptAgent::Pi | PromptAgent::OhMyPi
    ) {
        return true;
    }
    let config = crate::workspace::agent_command_config(agent, settings);
    let cli = if config.prompt_command.trim().is_empty() {
        &config.command
    } else {
        &config.prompt_command
    };
    has_flag(cli, skip_approval_flags(agent))
}

/// Where project-less session runs work, under the organisation's data.
pub(crate) fn sessions_root(org_dir: &Path) -> PathBuf {
    org_dir.join("automation-sessions")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn default_shell() -> String {
    dragent_pty::resolve_shell(
        crate::console::editor_settings::EditorSettings::global()
            .terminal
            .default_shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok()),
    )
}

/// How a run left the worker.
pub(crate) enum Outcome {
    /// Recorded as started, failed, or cancelled.
    Done,
    /// Workspace ready; a window has to start the agent (Dragent's own).
    NeedsWindow(Run),
}

/// Run on a worker thread; every outcome but a hand-off to a window is
/// recorded here.
pub(crate) fn execute(
    run: Run,
    store: Arc<Store>,
    scheduler: Scheduler,
    cancelled: Arc<AtomicBool>,
    org_dir: PathBuf,
) -> Outcome {
    match execute_inner(&run, &store, &scheduler, &cancelled, &org_dir) {
        Ok(outcome) => outcome,
        Err(error) => {
            if let Err(write) = scheduler.mark_failed(&store, &run.id, error, now_ms()) {
                log::error!("[automations] could not record run {} failure: {write:#}", run.id);
            }
            Outcome::Done
        }
    }
}

fn store_error(error: anyhow::Error) -> RunError {
    RunError::new(RunStep::Run, "store").with_message(format!("{error:#}"))
}

fn execute_inner(
    run: &Run,
    store: &Store,
    scheduler: &Scheduler,
    cancelled: &AtomicBool,
    org_dir: &Path,
) -> Result<Outcome, RunError> {
    let snapshot = &run.snapshot;
    let project = (!snapshot.is_session()).then(|| snapshot.project_root.clone());
    if let Some(project) = &project {
        if crate::remote_fs::resolve(project).is_some() {
            return Err(RunError::new(RunStep::Workspace, "remote_project")
                .with_message(super::REMOTE_PROJECT_MESSAGE));
        }
        if !project.is_dir() {
            return Err(RunError::new(RunStep::Workspace, "project_missing").with_message(format!(
                "{} is no longer available.",
                project.display()
            )));
        }
    }

    // A watching automation goes on only when its search found something new.
    let watched;
    let run = match &snapshot.watch {
        None => run,
        Some(watch) => match super::watch::check(run, watch, store, scheduler)? {
            Some(updated) => {
                watched = updated;
                &watched
            }
            None => return Ok(Outcome::Done),
        },
    };
    let snapshot = &run.snapshot;

    let prepared = prepare_workspace(run, project.as_deref(), store, org_dir)?;
    if cancelled.load(Ordering::Acquire) {
        return Ok(Outcome::Done);
    }
    let advanced = scheduler
        .mark_starting_agent(
            store,
            &run.id,
            prepared.path.clone(),
            prepared.branch.clone(),
            prepared.warning.clone(),
        )
        .map_err(store_error)?;
    let Some(starting) = advanced else {
        // Cancelled while the workspace was being prepared.
        return Ok(Outcome::Done);
    };

    if snapshot.agent == DRAGENT_AGENT {
        return Ok(Outcome::NeedsWindow(starting));
    }

    if snapshot.continue_session {
        if let Some((terminal, waiting)) = live_previous_terminal(run, store, &prepared.path) {
            if waiting {
                // Its Enter could answer the approval the agent is showing.
                scheduler
                    .mark_skipped(
                        store,
                        &run.id,
                        RunError::new(RunStep::Agent, codes::PREVIOUS_WAITING),
                        now_ms(),
                    )
                    .map_err(store_error)?;
                return Ok(Outcome::Done);
            }
            deliver_prompt(&terminal, &agent_prompt(&snapshot.prompt))?;
            scheduler
                .mark_started(store, &run.id, Some(terminal), true, now_ms())
                .map_err(store_error)?;
            return Ok(Outcome::Done);
        }
    }

    let terminal_root = project.clone().unwrap_or_else(|| prepared.path.clone());
    let terminal = start_agent(run, &terminal_root, &prepared.path)?;
    if cancelled.load(Ordering::Acquire) {
        close_terminal(&terminal);
        return Ok(Outcome::Done);
    }
    let started = scheduler
        .mark_started(store, &run.id, Some(terminal.clone()), false, now_ms())
        .map_err(store_error)?;
    if started.is_none() {
        // Cancelled while the agent was starting: don't leave it running.
        close_terminal(&terminal);
    }
    Ok(Outcome::Done)
}

struct Prepared {
    path: PathBuf,
    branch: Option<String>,
    warning: Option<String>,
}

fn workspace_failed(message: String) -> RunError {
    RunError::new(RunStep::Workspace, "workspace_failed").with_message(message)
}

fn prepare_workspace(
    run: &Run,
    project: Option<&Path>,
    store: &Store,
    org_dir: &Path,
) -> Result<Prepared, RunError> {
    let Some(project) = project else {
        return prepare_session(run, org_dir);
    };
    match &run.snapshot.workspace {
        WorkspaceMode::Pinned { path } => {
            let pinned = path.clone().unwrap_or_else(|| project.to_path_buf());
            if pinned.is_dir() {
                return Ok(Prepared {
                    branch: creation_branch(&pinned),
                    path: pinned,
                    warning: None,
                });
            }
            // The pinned workspace is gone: this run gets a fresh one.
            let mut prepared = new_worktree(run, project, None, false, store)?;
            prepared.warning = Some(format!(
                "{} no longer exists, so this run used a new worktree.",
                pinned.display()
            ));
            Ok(prepared)
        }
        WorkspaceMode::NewWorktree {
            base_branch,
            push_branch,
        } => new_worktree(run, project, base_branch.as_deref(), *push_branch, store),
        WorkspaceMode::ExistingBranch { branch } => {
            if let Some(path) = creation::worktree_for_branch(project, branch) {
                return Ok(Prepared {
                    path,
                    branch: Some(branch.clone()),
                    warning: None,
                });
            }
            let path = creation::checkout_existing_branch(project, branch).map_err(workspace_failed)?;
            copy_ignored(project, &path, store);
            Ok(Prepared {
                path,
                branch: Some(branch.clone()),
                warning: None,
            })
        }
    }
}

fn creation_branch(path: &Path) -> Option<String> {
    crate::workspace::project_naming::local_branch_name(path)
}

fn new_worktree(
    run: &Run,
    project: &Path,
    base_branch: Option<&str>,
    push: bool,
    store: &Store,
) -> Result<Prepared, RunError> {
    let start = creation::latest_start_point(project, base_branch).map_err(workspace_failed)?;
    let desired = format!("automation/{}", run.name);
    let (path, branch) =
        creation::create_worktree(project, &desired, &start).map_err(workspace_failed)?;
    crate::workspace::workspace_setup::maybe_init_worktree(&path);
    copy_ignored(project, &path, store);
    let warning = push
        .then(|| {
            let base = base_branch
                .map(str::to_string)
                .or_else(|| creation_branch(project));
            creation::publish_branch(&path, &branch, base.as_deref())
                .err()
                .map(|reason| format!("The branch wasn't published: {reason}"))
        })
        .flatten();
    Ok(Prepared {
        path,
        branch: Some(branch),
        warning,
    })
}

/// Bring the project's local, gitignored configuration along.
fn copy_ignored(project: &Path, worktree: &Path, store: &Store) {
    let main = crate::workspace::project_naming::worktree_main_checkout(project)
        .unwrap_or_else(|| project.to_path_buf());
    let patterns = store.copy_patterns(&main).unwrap_or_default();
    if patterns.is_empty() {
        return;
    }
    let candidates = creation::ignored_files(&main);
    let copied = copy_files::copy_selected(&main, worktree, &candidates, &patterns);
    if !copied.is_empty() {
        log::info!("[automations] copied {} ignored files into {}", copied.len(), worktree.display());
    }
}

fn prepare_session(run: &Run, org_dir: &Path) -> Result<Prepared, RunError> {
    let path = match &run.snapshot.workspace {
        WorkspaceMode::Pinned { path: Some(path) } => path.clone(),
        _ => sessions_root(org_dir).join(&run.name),
    };
    std::fs::create_dir_all(&path).map_err(|error| {
        workspace_failed(format!("Could not create {}: {error}", path.display()))
    })?;
    Ok(Prepared {
        path,
        branch: None,
        warning: None,
    })
}

/// The terminal the latest earlier run of this automation started in the
/// same workspace, if its agent is still running, and whether that agent
/// is waiting for a person.
fn live_previous_terminal(run: &Run, store: &Store, workspace: &Path) -> Option<(RunTerminal, bool)> {
    let previous = store.started_runs(&run.automation_id, 20).ok()?;
    let earlier = previous
        .into_iter()
        .filter(|earlier| earlier.id != run.id)
        .find(|earlier| earlier.workspace_path.as_deref() == Some(workspace))?;
    let waiting = earlier.progress.as_ref().is_some_and(|progress| {
        progress.state == dragent_automations::AgentState::NeedsInput && !progress.closed
    });
    let terminal = earlier.terminal?;
    let client = dragent_host::HostClient::connect_existing().ok()?;
    let session = client.terminal(&terminal.workspace_id, &terminal.session_id).ok()?;
    matches!(session.state, dragent_host::protocol::TerminalState::Running).then_some((terminal, waiting))
}

/// Type the prompt into a running agent as one paste, then submit it.
fn deliver_prompt(terminal: &RunTerminal, prompt: &str) -> Result<(), RunError> {
    let failed = |error: String| RunError::new(RunStep::Agent, "continue_failed").with_message(error);
    let client = dragent_host::HostClient::connect_existing().map_err(failed)?;
    let mut input = Vec::with_capacity(prompt.len() + 16);
    input.extend_from_slice(b"\x1b[200~");
    input.extend_from_slice(prompt.as_bytes());
    input.extend_from_slice(b"\x1b[201~");
    client
        .send_terminal_input(&terminal.workspace_id, &terminal.session_id, &input)
        .map_err(failed)?;
    // Submit separately, after the paste lands, so it isn't read as part of it.
    std::thread::sleep(std::time::Duration::from_millis(150));
    client
        .send_terminal_input(&terminal.workspace_id, &terminal.session_id, b"\r")
        .map_err(failed)
}

fn start_agent(run: &Run, project: &Path, cwd: &Path) -> Result<RunTerminal, RunError> {
    let agent = agent_for_key(&run.snapshot.agent).ok_or_else(|| {
        RunError::new(RunStep::Agent, "unknown_agent")
            .with_message(format!("Unknown agent “{}”.", run.snapshot.agent))
    })?;
    let config = crate::workspace::prompt_agent_interactive_command(agent).ok_or_else(|| {
        RunError::new(RunStep::Agent, "agent_unavailable").with_message(format!(
            "{} isn't installed or its command isn't configured.",
            agent.label()
        ))
    })?;
    let shell = default_shell();
    // Runs start in a terminal of the local host, which can read the prompt file.
    let command = config
        .build_prompt_command(&agent_prompt(&run.snapshot.prompt), &shell, true)
        .map_err(|error| RunError::new(RunStep::Agent, "launch_command").with_message(error))?;
    start_command(&command, project, cwd, &shell)
}

/// Start `command` in a host terminal at `cwd`, tracking its conversation
/// like a pane-launched agent so it can resume after a reboot.
pub(crate) fn start_command(
    command: &str,
    project: &Path,
    cwd: &Path,
    shell: &str,
) -> Result<RunTerminal, RunError> {
    let pane_uuid = uuid::Uuid::new_v4();
    let command = dragent_host::agent_sessions::wrap_command(pane_uuid, shell, command);

    let host_error =
        |error: String| RunError::new(RunStep::Agent, "host_unavailable").with_message(error);
    let client = dragent_host::HostClient::ensure_host().map_err(host_error)?;
    let project = project.to_string_lossy().to_string();
    let cwd = cwd.to_string_lossy().to_string();
    let workspace_id = client.create_local_workspace(&project).map_err(host_error)?;
    let home = client
        .ensure_terminal_home(&workspace_id, &project, &cwd)
        .map_err(host_error)?;
    let session = client
        .create_terminal_with_shell(
            &workspace_id,
            &home.id,
            Some(&command),
            Some(&cwd),
            HEADLESS_COLS,
            HEADLESS_ROWS,
            Some(shell),
        )
        .map_err(|error| RunError::new(RunStep::Agent, "start_failed").with_message(error))?;
    Ok(RunTerminal {
        workspace_id,
        home_id: home.id,
        session_id: session.id,
        pane_uuid: Some(pane_uuid.to_string()),
    })
}

fn close_terminal(terminal: &RunTerminal) {
    if let Ok(client) = dragent_host::HostClient::connect_existing() {
        let _ = client.close_terminal(&terminal.workspace_id, &terminal.session_id);
    }
}

/// Human wording for a recorded failure or skip.
pub(crate) fn describe_error(error: &RunError) -> String {
    match error.code.as_str() {
        codes::MISSED_WINDOW => "Skipped: it couldn't start before its next scheduled time".into(),
        codes::AUTOMATION_PAUSED => "Skipped: the automation was paused".into(),
        codes::AUTOMATION_DELETED => "Skipped: the automation was deleted".into(),
        codes::ALREADY_RUNNING => "Skipped: the previous run was still starting".into(),
        codes::CANCELLED => "Skipped by you".into(),
        codes::NOTHING_NEW | codes::WATCH_LIMIT | codes::WATCH_FAILED => error
            .message
            .clone()
            .unwrap_or_else(|| "Its search found nothing new".into()),
        codes::INTERRUPTED => "Stopped: Dragent closed while the run was starting".into(),
        codes::PREVIOUS_WAITING => {
            "Skipped: the session it continues is still waiting for you".into()
        }
        codes::PLAN_LIMIT => {
            "Skipped: the free plan runs 4 automations. Upgrade to Pro to run them all".into()
        }
        "no_window" => "Couldn't start: Dragent's agent needs an open Dragent window".into(),
        _ => error.message.clone().unwrap_or_else(|| match error.step {
            RunStep::Workspace => "Couldn't prepare the workspace".into(),
            RunStep::Agent => "Couldn't start the agent".into(),
            RunStep::Queue | RunStep::Run => format!("Failed ({})", error.code),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_run_unattended_only_with_their_skip_approval_option() {
        let mut settings = crate::setup::UiSettings::default();
        // The defaults: Claude and Codex skip approvals, Gemini only edits.
        assert!(runs_unattended(PromptAgent::Claude, &settings));
        assert!(runs_unattended(PromptAgent::Codex, &settings));
        assert!(runs_unattended(PromptAgent::Dragent, &settings));
        assert!(!runs_unattended(PromptAgent::Gemini, &settings));
        assert!(!runs_unattended(PromptAgent::Droid, &settings));

        // A customised command decides.
        let mut gemini = crate::workspace::default_agent_command_config(PromptAgent::Gemini);
        gemini.prompt_command = "gemini --approval-mode yolo".into();
        settings.agent_command_configs.insert("gemini".into(), gemini);
        assert!(runs_unattended(PromptAgent::Gemini, &settings));
        let mut claude = crate::workspace::default_agent_command_config(PromptAgent::Claude);
        claude.prompt_command = "claude --model opus".into();
        settings.agent_command_configs.insert("claude".into(), claude);
        assert!(!runs_unattended(PromptAgent::Claude, &settings));
    }

    #[test]
    fn flags_match_whole_words() {
        assert!(has_flag("cursor-agent -f -p", &["--force", "-f"]));
        assert!(!has_flag("cursor-agent --fast", &["--force", "-f"]));
        assert!(has_flag("qwen  --approval-mode   yolo -i", &["--approval-mode yolo"]));
    }

    #[test]
    fn terminal_agents_hear_that_nobody_is_watching() {
        let prompt = agent_prompt("Review the diff");
        assert!(prompt.starts_with("This is a scheduled Dragent automation run"));
        assert!(prompt.ends_with("\n\nReview the diff"));
        // The phrase the inbox files away on.
        assert!(prompt.contains(&format!("\"{}\"", dragent_automations::NOTHING_TO_REPORT)));
    }
}
