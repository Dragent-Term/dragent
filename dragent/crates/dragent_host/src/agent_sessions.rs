//! Exact provider conversation identity, independent of a daemon PTY lifetime.
//!
//! The launch helper and provider hooks run inside the host-owned terminal,
//! including while the GUI is disconnected. Records are keyed by pane UUID;
//! a generation fences late hooks from a previous invocation. Never discover
//! conversations by cwd, provider name, or "most recent" session.
//!
//! How each agent reports and resumes its conversation is data
//! ([`crate::agent_recovery::SessionRecovery`]): built-ins come from
//! [`crate::agent_recovery::builtin`], custom agents are staged per pane by
//! the GUI before launch ([`stage_custom_agent`]).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent_recovery::{self, HookKind, SessionIdStrategy, SessionRecovery};
use crate::agent_user_hooks;

pub const RUN_ARG: &str = "--dragent-agent-run";
pub const HOOK_ARG: &str = "--dragent-agent-session-hook";

const STAGED_AGENT: &str = "custom-agent.json";
/// The agent's last reply, kept for automation runs.
const REPLY: &str = "reply.json";
/// Longest reply kept, in bytes.
const REPLY_LIMIT: usize = 32 * 1024;
/// Upper bound for a provider command that mints a conversation id.
const MINT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    #[serde(default = "dragent_paths::local_organisation_id")]
    pub organisation_id: String,
    pub pane_uuid: Uuid,
    pub generation: Uuid,
    /// Built-in executable name, or a custom agent's `custom:<slug>`.
    pub provider: String,
    /// Captured account identity; legacy records remain on the system login.
    #[serde(default)]
    pub account: Option<crate::agent_accounts::Binding>,
    pub cwd: PathBuf,
    pub executable: String,
    /// Only interactive launch options, never the original user prompt.
    pub resume_options: Option<Vec<String>>,
    pub conversation_id: Option<String>,
    pub active: bool,
    #[serde(default)]
    pub resumed: bool,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
    pub workspace_id: String,
    pub terminal_id: String,
    /// Recovery descriptor captured at launch, so a resume after reboot uses
    /// the same mechanism even if the agent's definition changed since.
    /// Absent in records written before descriptors existed.
    #[serde(default)]
    pub recovery: Option<SessionRecovery>,
    /// The conversation received a prompt: given at launch, piped in, typed
    /// into the pane, or reported by a prompt hook. Records from before this
    /// was tracked count as used, so nothing is ever started over by mistake.
    #[serde(default = "assume_used")]
    pub used: bool,
}

fn assume_used() -> bool {
    true
}

/// One provider hook report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    pub session_id: String,
    pub event: String,
    pub source: Option<String>,
    pub subagent: bool,
    /// What the agent said last, when the provider reports it on Stop.
    pub last_message: Option<String>,
}

impl HookEvent {
    fn parse(data: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Ok(Self {
            session_id: text("session_id").ok_or("missing provider session id")?,
            event: text("hook_event_name").unwrap_or_default(),
            source: text("source"),
            subagent: text("agent_id").or_else(|| text("agentId")).is_some(),
            last_message: text("last_assistant_message").filter(|text| !text.trim().is_empty()),
        })
    }
}

impl AgentSession {
    pub fn resume_failed(&self) -> bool {
        self.resumed && !self.active && self.last_exit_code.is_some_and(|code| code != 0)
    }

    /// Nothing can be resumed, and nothing would be lost by starting over:
    /// the conversation never received a prompt.
    pub fn nothing_to_resume(&self) -> bool {
        !self.used
            && self.resume_options.is_some()
            && (self.resume_failed() || (self.active && self.conversation_id.is_none()))
    }

    /// Apply a hook report; returns whether the record changed. `by_ancestry`
    /// reports come from user-level hooks, which also fire for the same agent
    /// run nested inside this one: those only confirm the pane's own id.
    pub fn apply_hook(&mut self, event: &HookEvent, by_ancestry: bool) -> bool {
        let before = (self.conversation_id.clone(), self.used);
        let id = Some(event.session_id.clone());
        let known = self.conversation_id.is_some();
        match (event.event.as_str(), event.source.as_deref()) {
            ("UserPromptSubmit", _) => {
                if !known || (!by_ancestry && self.conversation_id != id) {
                    self.conversation_id = id.clone();
                }
                if self.conversation_id == id {
                    self.used = true;
                }
            }
            // A cleared conversation starts empty; a picked one exists.
            (_, Some("clear")) => {
                self.conversation_id = id;
                self.used = false;
            }
            (_, Some("resume")) => {
                self.conversation_id = id;
                self.used = true;
            }
            (_, Some("startup")) if by_ancestry && known => {}
            _ => {
                if self.conversation_id != id {
                    self.conversation_id = id;
                }
            }
        }
        before != (self.conversation_id.clone(), self.used)
    }

    pub fn recovery(&self) -> Option<SessionRecovery> {
        self.recovery
            .clone()
            .or_else(|| agent_recovery::builtin(&self.provider))
    }
}

/// A custom agent's recovery descriptor, staged by the GUI for one pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StagedAgent {
    provider: String,
    executable: String,
    recovery: SessionRecovery,
}

/// Declare the custom agent launched next in `pane`. The launch helper only
/// applies it to an invocation of the same executable.
pub fn stage_custom_agent(
    pane: Uuid,
    provider: &str,
    executable: &str,
    recovery: &SessionRecovery,
) -> Result<(), String> {
    let dir = private_directory(pane)?;
    let staged = StagedAgent {
        provider: provider.to_owned(),
        executable: agent_recovery::executable_name(executable).to_owned(),
        recovery: recovery.clone(),
    };
    write_atomic(
        &dir,
        STAGED_AGENT,
        &serde_json::to_vec(&staged).map_err(|e| e.to_string())?,
    )
}

/// Provider id and recovery descriptor for an invocation of `executable`.
/// Stage an explicit account before the pane's first invocation. No credentials.
pub fn stage_account(pane: Uuid, binding: &crate::agent_accounts::Binding) -> Result<(), String> {
    crate::agent_accounts::Registry::load()?.validate(binding)?;
    if load(pane).is_some() { return Err("Account switching requires a new session".into()); }
    write_atomic(&private_directory(pane)?, "account.json", &serde_json::to_vec(binding).map_err(|_| "Could not stage account")?)
}
fn launch_account(pane: Uuid, provider: &str, cwd: &Path) -> Result<crate::agent_accounts::Binding, String> {
    use crate::agent_accounts::{Binding, Registry, Selection};
    let registry = Registry::load()?;
    // A pane is pinned for its lifetime, including fresh/retry/reboot commands.
    let binding = if let Some(previous) = load(pane).filter(|p| p.provider == provider) {
        previous.account.unwrap_or_else(|| Binding::system(provider))
    } else {
        match fs::read(directory(pane).join("account.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Staged account is invalid")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => registry.resolve(provider, crate::agent_accounts::project_for(cwd, &registry).as_deref(), &Selection::Inherit)?,
            Err(_) => return Err("Could not read staged account".into()),
        }
    };
    if binding.provider != provider { return Err("Selected account belongs to a different agent".into()); }
    registry.validate(&binding)?;
    Ok(binding)
}

fn resolve(pane: Uuid, executable: &str) -> Option<(String, SessionRecovery)> {
    let name = agent_recovery::executable_name(executable);
    if let Some(recovery) = agent_recovery::builtin(name) {
        return Some((name.to_owned(), recovery));
    }
    let bytes = fs::read(directory(pane).join(STAGED_AGENT)).ok()?;
    let staged: StagedAgent = serde_json::from_slice(&bytes).ok()?;
    (staged.executable == name).then_some((staged.provider, staged.recovery))
}

fn exit_code(status: &std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1)
    }
}

fn directory(pane: Uuid) -> PathBuf {
    dragent_paths::organisation_dir()
        .join("agent-sessions")
        .join(pane.to_string())
}

/// Marker file of a pane whose agent was put to sleep to free memory.
const ASLEEP: &str = "asleep";

/// Mark a pane's agent as asleep (its process was stopped on purpose and
/// resumes on the same conversation when the pane is opened), or awake.
/// While asleep, restoring the pane never starts its agent by itself.
pub fn set_asleep(pane: Uuid, asleep: bool) -> Result<(), String> {
    let marker = directory(pane).join(ASLEEP);
    if asleep {
        let dir = private_directory(pane)?;
        write_atomic(&dir, ASLEEP, b"1")
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Whether a pane's agent was put to sleep and hasn't been woken.
pub fn is_asleep(pane: Uuid) -> bool {
    directory(pane).join(ASLEEP).is_file()
}

/// A durable request to wake a sleeping agent, read by the app's sleep
/// sweep. One small file per pane: idempotent, the latest reason wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WakeRequest {
    reason: String,
    at_ms: u64,
}

fn wake_dir() -> PathBuf {
    dragent_paths::organisation_dir()
        .join("agent-sessions")
        .join("wake-requests")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn request_wake_in(wake: &Path, pane: Uuid, reason: &str) -> Result<(), String> {
    fs::create_dir_all(wake).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(wake, fs::Permissions::from_mode(0o700));
    }
    let request = WakeRequest {
        reason: reason.to_owned(),
        at_ms: now_ms(),
    };
    write_atomic(
        wake,
        &format!("{pane}.json"),
        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
    )
}

fn take_wake_requests_in(wake: &Path) -> Vec<(Uuid, String)> {
    let Ok(entries) = fs::read_dir(wake) else {
        return Vec::new();
    };
    // Rename-then-read: a request written while this runs lands as a fresh
    // file at the original path (or is the content renamed), so it is
    // returned now or left for the next sweep, never deleted unseen.
    let mut taken: Vec<(Uuid, String, u64)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let stem = name.strip_suffix(".json").unwrap_or(&name);
        let Ok(pane) = Uuid::parse_str(stem) else {
            continue;
        };
        let source = entry.path();
        let claimed = wake.join(format!(".taken-{}-{name}", Uuid::new_v4().simple()));
        if fs::rename(&source, &claimed).is_err() {
            continue;
        }
        let contents = fs::read(&claimed);
        let _ = fs::remove_file(&claimed);
        let Ok(bytes) = contents else {
            continue;
        };
        if let Ok(request) = serde_json::from_slice::<WakeRequest>(&bytes) {
            taken.push((pane, request.reason, request.at_ms));
        } else if let Ok(text) = String::from_utf8(bytes) {
            let reason = text.trim().to_owned();
            if !reason.is_empty() {
                taken.push((pane, reason, 0));
            }
        }
    }
    taken.sort_by_key(|(_, _, at_ms)| *at_ms);
    taken
        .into_iter()
        .map(|(pane, reason, _)| (pane, reason))
        .collect()
}

/// Record that sleeping agent `pane` is needed awake. Idempotent: the latest
/// reason and time win. A pane with no session record is a no-op success, so
/// callers never have to check first.
pub fn request_wake(pane: Uuid, reason: &str) -> Result<(), String> {
    if load(pane).is_none() {
        return Ok(());
    }
    request_wake_in(&wake_dir(), pane, reason)
}

/// Return and clear all pending wake requests. A request written while this
/// runs is never lost: it is returned now or left for the next call.
pub fn take_wake_requests() -> Vec<(Uuid, String)> {
    take_wake_requests_in(&wake_dir())
}

fn private_directory(pane: Uuid) -> Result<PathBuf, String> {
    let dir = directory(pane);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

/// Shared between helpers and restore workers. A process crash releases the
/// OS lock; no stale sentinel can permanently prevent recovery.
pub fn lock(pane: Uuid) -> Result<File, String> {
    let dir = private_directory(pane)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(dir.join("session.lock"))
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    Ok(file)
}

pub fn load(pane: Uuid) -> Option<AgentSession> {
    let bytes = fs::read(directory(pane).join("session.json")).ok()?;
    let record: AgentSession = serde_json::from_slice(&bytes).ok()?;
    (record.pane_uuid == pane && record.organisation_id == dragent_paths::organisation_id())
        .then_some(record)
}

/// Latest provider activity, bound to a launch generation and conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentActivity {
    pub generation: Uuid,
    pub conversation_id: String,
    pub event: String,
    pub at_ms: u64,
}

impl AgentActivity {
    /// Bootstrap snapshots never notify. A new terminal event does, even if
    /// a complete short turn happened between polls and both snapshots say Stop.
    pub fn notification_after(
        &self,
        previous: Option<&(Uuid, u64, String)>,
    ) -> Option<&'static str> {
        let previous = previous?;
        if previous.0 != self.generation
            || previous.1 > self.at_ms
            || (previous.1 == self.at_ms && previous.2 == self.event)
        {
            return None;
        }
        match self.event.as_str() {
            "Stop" => Some("Finished responding"),
            "PermissionRequest" => Some("Needs your input or permission"),
            "Failed" => Some("Encountered an error"),
            _ => None,
        }
    }
}

/// The reply that ended the agent's latest turn, as its provider reported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReply {
    pub generation: Uuid,
    pub conversation_id: String,
    pub text: String,
    pub at_ms: u64,
}

/// The pane's launch record and its latest reply, also after the agent
/// exited (a finished automation run still shows what it said). Read on a
/// worker.
pub fn load_reply(pane: Uuid) -> Option<AgentReply> {
    let record = load(pane)?;
    let reply: AgentReply =
        serde_json::from_slice(&fs::read(directory(pane).join(REPLY)).ok()?).ok()?;
    (reply.generation == record.generation
        && record.conversation_id.as_deref() == Some(reply.conversation_id.as_str()))
    .then_some(reply)
}

/// `text` cut to at most `limit` bytes on a character boundary.
fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text[..end].trim_end())
}

/// When the person last looked at this agent, so what they've already seen
/// stays seen across restarts. The first time the app meets a session,
/// whatever it's doing counts as known: `baseline_ms` is recorded and returned.
/// Read on a worker.
pub fn activity_seen_or(pane: Uuid, baseline_ms: u64) -> u64 {
    if let Some(seen) = activity_seen(pane) {
        return seen;
    }
    mark_activity_seen(pane, baseline_ms);
    baseline_ms
}

/// When the person last looked at this agent, if ever. Read on a worker.
pub fn activity_seen(pane: Uuid) -> Option<u64> {
    fs::read_to_string(directory(pane).join(ACTIVITY_SEEN))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// The person looked at this agent at `at_ms`. Read on a worker.
pub fn mark_activity_seen(pane: Uuid, at_ms: u64) {
    if let Err(error) = write_atomic(
        &directory(pane),
        ACTIVITY_SEEN,
        at_ms.to_string().as_bytes(),
    ) {
        log::warn!("[agent-sessions] couldn't note {pane} as seen: {error}");
    }
}

/// The activity of an agent that stopped without reporting it (see
/// `agent_quiet`): not working, and not known to have finished.
pub const ACTIVITY_QUIET: &str = "Quiet";
const ACTIVITY_SEEN: &str = "activity-seen";

/// An active agent reported as working or asking, which may have stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsettled {
    pub pane: Uuid,
    pub terminal: String,
    pub generation: Uuid,
    pub conversation_id: String,
    pub event: String,
    pub at_ms: u64,
}

/// Every such agent. Read on a worker. Finished sessions cost one small read.
pub fn unsettled() -> Vec<Unsettled> {
    let Ok(entries) = fs::read_dir(dragent_paths::organisation_dir().join("agent-sessions")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pane = Uuid::parse_str(entry.file_name().to_str()?).ok()?;
            let bytes = fs::read(entry.path().join("activity.json")).ok()?;
            let last: AgentActivity = serde_json::from_slice(&bytes).ok()?;
            if !matches!(last.event.as_str(), "Start" | "PermissionRequest") {
                return None;
            }
            let activity = load_activity(pane)?;
            Some(Unsettled {
                pane,
                terminal: load(pane)?.terminal_id,
                generation: activity.generation,
                conversation_id: activity.conversation_id,
                event: activity.event,
                at_ms: activity.at_ms,
            })
        })
        .collect()
}

/// Record that `agent` is now `event`, unless a report came in since it was
/// read.
pub fn mark_activity(agent: &Unsettled, event: &str) -> Result<(), String> {
    let _lock = lock(agent.pane)?;
    let current = load_activity(agent.pane);
    if current.is_none_or(|current| {
        current.generation != agent.generation
            || current.event != agent.event
            || current.at_ms != agent.at_ms
    }) {
        return Ok(());
    }
    let activity = AgentActivity {
        generation: agent.generation,
        conversation_id: agent.conversation_id.clone(),
        event: event.into(),
        at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    write_atomic(
        &directory(agent.pane),
        "activity.json",
        &serde_json::to_vec(&activity).map_err(|e| e.to_string())?,
    )
}

/// Read on a worker, never from a render callback. Old launches cannot leak state.
pub fn load_activity(pane: Uuid) -> Option<AgentActivity> {
    let record = load(pane)?;
    if !record.active {
        return None;
    }
    // A surviving process may still have the old, incomplete plugin loaded.
    // Its Start alone is not reliable evidence of current activity.
    if fs::read_to_string(directory(pane).join("activity-version"))
        .ok()?
        .trim()
        != record.generation.to_string()
    {
        return None;
    }
    let activity: AgentActivity =
        serde_json::from_slice(&fs::read(directory(pane).join("activity.json")).ok()?).ok()?;
    (activity.generation == record.generation
        && record.conversation_id.as_deref() == Some(activity.conversation_id.as_str()))
    .then_some(activity)
}

/// Whether an agent's reports say it is waiting for a prompt, not working or
/// asking for permission. Without any report, only an agent that was never
/// given a prompt counts: its hooks may simply not be running.
fn between_turns(event: Option<&str>, used: bool) -> bool {
    match event {
        Some("Stop" | "Failed") => true,
        Some(_) => false,
        None => !used,
    }
}

/// Whether the pane's agent is sitting idle at its prompt, for putting it to
/// sleep. Like [`is_between_turns`], and also: an agent resumed into this
/// launch that has reported nothing since. Its hooks report the next prompt,
/// so silence means nobody has asked it anything since it came back (the
/// caller also checks that its terminal is quiet).
pub fn is_resting(pane: Uuid) -> bool {
    let Some(record) = load(pane).filter(|record| record.active) else {
        return false;
    };
    let activity = load_activity(pane);
    resting(activity.as_ref().map(|a| a.event.as_str()), record.used, record.resumed)
}

fn resting(event: Option<&str>, used: bool, resumed: bool) -> bool {
    between_turns(event, used) || (event.is_none() && resumed)
}

/// Whether the pane's agent can be restarted without interrupting anything.
/// Read on a worker.
pub fn is_between_turns(pane: Uuid) -> bool {
    let Some(record) = load(pane).filter(|record| record.active) else {
        return false;
    };
    let activity = load_activity(pane);
    between_turns(activity.as_ref().map(|a| a.event.as_str()), record.used)
}

fn save(record: &AgentSession) -> Result<(), String> {
    write_atomic(
        &directory(record.pane_uuid),
        "session.json",
        &serde_json::to_vec(record).map_err(|e| e.to_string())?,
    )
}

/// Durable replace: a power loss leaves either the old or the new file.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let tmp = dir.join(format!("{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn quote(value: &str, shell: &str) -> String {
    let shell = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .trim_end_matches(".exe");
    if matches!(shell, "pwsh" | "powershell") {
        format!("'{}'", value.replace('\'', "''"))
    } else if shell == "fish" {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Windows GUI executables do not behave as foreground console commands.
/// Use the shipped console host for the monitor so PowerShell waits for it
/// and the agent inherits the original ConPTY console and I/O handles.
pub fn helper_executable() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    if executable
        .file_name()
        .is_some_and(|name| name != "dragent-host.exe")
    {
        let host = executable.with_file_name("dragent-host.exe");
        if !host.is_file() {
            return Err("Missing agent session helper: dragent-host.exe".into());
        }
        return Ok(host);
    }
    Ok(executable)
}

/// Insert a helper before a direct provider invocation. Arguments remain
/// shell syntax, so heredoc/piped prompt delivery and user flags still work.
/// Unsupported executables/compound prefixes are left intact.
pub fn wrap_command(pane: Uuid, shell: &str, command: &str) -> String {
    let Ok(helper) = helper_executable() else {
        return command.to_owned();
    };
    wrap_command_with_helper(pane, shell, command, &helper)
}

pub fn wrap_command_with_helper(pane: Uuid, shell: &str, command: &str, helper: &Path) -> String {
    wrap_command_in_context(pane, shell, command, helper, Some(&dragent_paths::organisation_arg()))
}

/// The remote launcher selects its own organisation and runtime. A local
/// organisation argument must never redirect a remote helper's storage.
pub fn wrap_remote_command(pane: Uuid, shell: &str, command: &str) -> String {
    wrap_command_in_context(pane, shell, command, Path::new("dragent"), None)
}

/// An explicit app launch can identify a pinned provider whose launcher has
/// another filename. Typed shell commands continue through ordinary detection.
pub fn wrap_remote_provider_command(pane: Uuid, shell: &str, command: &str, provider: &str) -> String {
    if agent_recovery::builtin(provider).is_none() { return command.into(); }
    format!("{} {RUN_ARG} {pane} --provider {} -- {}", quote("dragent", shell), quote(provider, shell), command.trim_start())
}

fn wrap_command_in_context(pane: Uuid, shell: &str, command: &str, helper: &Path, organisation: Option<&str>) -> String {
    let trimmed = command.trim_start();
    let start = if trimmed.starts_with("& ") {
        &trimmed[2..]
    } else {
        trimmed
    };
    let (executable, _) = first_word(start);
    if resolve(pane, &executable).is_none() {
        return command.to_owned();
    }
    let invoke = if matches!(
        shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(shell)
            .trim_end_matches(".exe"),
        "pwsh" | "powershell"
    ) {
        "& "
    } else {
        ""
    };
    format!(
        "{invoke}{} {RUN_ARG} {pane}{} -- {start}",
        quote(&helper.to_string_lossy(), shell),
        organisation.map(|org|format!(" {}", quote(org, shell))).unwrap_or_default()
    )
}

fn first_word(command: &str) -> (String, usize) {
    let mut word = String::new();
    let mut quoted = None;
    let mut escaped = false;
    for (i, c) in command.char_indices() {
        if escaped {
            word.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && quoted != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quoted {
            if c == q {
                quoted = None;
            } else {
                word.push(c);
            }
        } else if matches!(c, '\'' | '"') {
            quoted = Some(c);
        } else if c.is_whitespace() {
            return (word, i);
        } else {
            word.push(c);
        }
    }
    (word, command.len())
}

/// Command resuming the recorded conversation with its original launch
/// options. `None` without an exact id, a resumable launch, or when `cwd`
/// is not the directory the conversation belongs to.
pub fn resume_command(record: &AgentSession, cwd: &Path, shell: &str) -> Option<String> {
    if !record.active || record.cwd != cwd.canonicalize().ok()? {
        return None;
    }
    let recovery = record.recovery()?;
    let id = record
        .conversation_id
        .as_deref()
        .filter(|id| recovery.valid_id(id))?;
    let argv = recovery.resume_argv(&record.executable, record.resume_options.as_ref()?, id);
    Some(shell_command(&argv, shell))
}

/// Command starting a new conversation with the recorded launch options,
/// for an explicit "Start New Session".
pub fn fresh_command(record: &AgentSession, shell: &str) -> Option<String> {
    let mut argv = vec![record.executable.clone()];
    argv.extend(record.resume_options.clone()?);
    Some(shell_command(&argv, shell))
}

fn shell_command(argv: &[String], shell: &str) -> String {
    argv.iter()
        .enumerate()
        .map(|(i, a)| {
            if i == 0
                && a.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            {
                a.clone()
            } else {
                quote(a, shell)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Command starting the pane's agent again after its unused conversation
/// could not be resumed. A conversation id Dragent chose is reused, so the
/// pane keeps the same identity.
pub fn start_over_command(record: &AgentSession, shell: &str) -> Option<String> {
    let recovery = record.recovery()?;
    let mut argv = vec![record.executable.clone()];
    argv.extend(record.resume_options.clone()?);
    if let (SessionIdStrategy::Preassigned { .. }, Some(id)) = (
        &recovery.strategy,
        record
            .conversation_id
            .as_deref()
            .filter(|id| recovery.valid_id(id)),
    ) {
        argv.extend(recovery.new_conversation_args(id));
    }
    Some(shell_command(&argv, shell))
}

/// The agent session running in host terminal `terminal_id`, if any.
pub fn pane_for_terminal(terminal_id: &str) -> Option<Uuid> {
    let text = fs::read_to_string(terminal_index(terminal_id)?).ok()?;
    Uuid::parse_str(text.trim()).ok()
}

fn terminal_index(terminal_id: &str) -> Option<PathBuf> {
    let safe = !terminal_id.is_empty()
        && terminal_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    safe.then(|| {
        dragent_paths::organisation_dir()
            .join("agent-sessions")
            .join("terminals")
            .join(terminal_id)
    })
}

/// Map a host terminal to the pane whose agent runs in it.
fn index_terminal(terminal_id: &str, pane: Uuid) -> Result<(), String> {
    let path = terminal_index(terminal_id).ok_or("invalid terminal id")?;
    let dir = path.parent().ok_or("invalid terminal index")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    write_atomic(dir, terminal_id, pane.to_string().as_bytes())
}

/// The user submitted a line to a host terminal. The agent running there
/// has now received a prompt, whatever the agent is.
pub fn note_submitted_input(terminal_id: &str) {
    let Some(pane) = terminal_index(terminal_id)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| Uuid::parse_str(text.trim()).ok())
    else {
        return;
    };
    let unused = |r: &AgentSession| !r.used && r.active && r.terminal_id == terminal_id;
    if !load(pane).is_some_and(|r| unused(&r)) {
        return;
    }
    let Ok(_lock) = lock(pane) else {
        return;
    };
    if let Some(mut record) = load(pane).filter(unused) {
        record.used = true;
        let _ = save(&record);
    }
}

/// Whether an agent executable has a resumable identity mechanism.
pub fn supports_resume(pane: Uuid, executable: &str) -> bool {
    resolve(pane, executable).is_some()
}

/// Transport recovery is deliberately separate from visual restoration. The
/// GUI only applies this result to the original pane; it never activates it.
pub struct RestoreTerminal<'a> {
    pub pane_uuid: Uuid,
    pub workspace_id: &'a str,
    pub home_id: &'a str,
    pub terminal_id: &'a str,
    pub root: &'a str,
    pub cwd: &'a str,
    pub shell: Option<&'a str>,
    pub allow_new_shell: bool,
    /// Monitors a resumed agent. A development build can lack it: terminals
    /// still restore, and an agent resumes unmonitored as [`wrap_command`] does.
    pub helper: Option<&'a Path>,
}

/// Transport chosen for a restored pane. An empty `terminal_id` means no
/// process may be bound: the pane stays explicitly unavailable.
#[derive(Debug, Clone)]
pub struct Restored {
    pub home_id: String,
    pub terminal_id: String,
    /// A new process was started to resume the recorded conversation
    /// (machine reboot), rather than reattaching a surviving one.
    pub resumed: bool,
    /// The pane's agent was put to sleep: nothing was started, and the pane
    /// wakes it when it is opened. `terminal_id` is empty.
    pub asleep: bool,
}

impl Restored {
    fn attached(home_id: String, terminal_id: String) -> Self {
        Self {
            home_id,
            terminal_id,
            resumed: false,
            asleep: false,
        }
    }
}

pub fn restore_terminal(
    client: &crate::HostClient,
    request: RestoreTerminal<'_>,
) -> Result<Restored, String> {
    let _recovery_lock = lock(request.pane_uuid)?;
    let mut record = load(request.pane_uuid);
    // A failed list is a transport failure, never proof that a process died.
    let sessions = client.list_terminals(request.workspace_id)?;
    if let Some(failed) = record.as_ref().filter(|r| {
        !request.allow_new_shell
            && r.resume_failed()
            && r.workspace_id == request.workspace_id
            && Path::new(request.cwd).canonicalize().is_ok_and(|cwd| r.cwd == cwd)
    }) {
        // An agent pane must report a failed exact-ID resume even though
        // its parent shell survived. Ordinary terminals may keep using that
        // shell, so apply this before reattachment only for agent panes.
        let _ = client.close_terminal(request.workspace_id, &failed.terminal_id);
        return Ok(Restored::attached(request.home_id.into(), String::new()));
    }
    if let Some(session) = sessions.iter().find(|session| {
        session.id == request.terminal_id && matches!(session.state, crate::TerminalState::Running)
    }) {
        return Ok(Restored::attached(
            session.worktree_id.clone(),
            session.id.clone(),
        ));
    }
    if let Some(record) = record.as_ref().filter(|r| {
        r.active
            && r.workspace_id == request.workspace_id
            && r.cwd
                == PathBuf::from(request.cwd)
                    .canonicalize()
                    .unwrap_or_default()
    }) {
        if let Some(session) = sessions.iter().find(|session| {
            session.id == record.terminal_id
                && matches!(session.state, crate::TerminalState::Running)
        }) {
            return Ok(Restored::attached(
                session.worktree_id.clone(),
                session.id.clone(),
            ));
        }
    }
    // A sleeping agent stays asleep across restarts: its pane wakes it when
    // it is opened, not every launch (which would undo the memory it saved).
    if !request.allow_new_shell && is_asleep(request.pane_uuid) {
        return Ok(Restored {
            home_id: request.home_id.into(),
            terminal_id: String::new(),
            resumed: false,
            asleep: true,
        });
    }
    let shell_name = request.shell.unwrap_or("sh");
    if let Some(record) = record.as_mut() {
        // A restored agent pane represents the conversation even if its
        // previous process exited. Reopening that pane should resume it.
        if !request.allow_new_shell {
            record.active = true;
        }
        if let Some(command) = resume_command(record, Path::new(request.cwd), shell_name) {
            let home =
                client.ensure_terminal_home(request.workspace_id, request.root, request.cwd)?;
            let tracked = match request.helper {
                Some(helper) => {
                    wrap_command_with_helper(request.pane_uuid, shell_name, &command, helper)
                }
                None => {
                    log::warn!(
                        "resuming {} without the session helper pane={}",
                        record.provider,
                        request.pane_uuid
                    );
                    command
                }
            };
            let session = client.create_terminal_with_shell(
                request.workspace_id,
                &home.id,
                Some(&tracked),
                Some(request.cwd),
                80,
                24,
                request.shell,
            )?;
            if let Err(error) = bind_transport(record, request.workspace_id, &session.id) {
                let _ = client.close_terminal(request.workspace_id, &session.id);
                return Err(error);
            }
            log::info!(
                "resuming persisted {} conversation pane={}",
                record.provider,
                request.pane_uuid
            );
            return Ok(Restored {
                home_id: home.id,
                terminal_id: session.id,
                resumed: true,
                asleep: false,
            });
        }
    }
    // An inactive agent record belongs to a command that already finished;
    // it must not strand an otherwise ordinary terminal pane after reboot.
    if request.allow_new_shell && record.as_ref().is_none_or(|record| !record.active) {
        let home = client.ensure_terminal_home(request.workspace_id, request.root, request.cwd)?;
        let session = client.create_terminal_with_shell(
            request.workspace_id,
            &home.id,
            None,
            Some(request.cwd),
            80,
            24,
            request.shell,
        )?;
        return Ok(Restored::attached(home.id, session.id));
    }
    Ok(Restored::attached(request.home_id.into(), String::new()))
}

/// Persist a new transport identity before returning it to the GUI. Called
/// under the pane lock, so a second restoring window adopts this same PTY.
pub fn bind_transport(
    record: &mut AgentSession,
    workspace: &str,
    terminal: &str,
) -> Result<(), String> {
    record.workspace_id = workspace.to_owned();
    record.terminal_id = terminal.to_owned();
    save(record)
}

fn read_hook_event() -> Result<HookEvent, String> {
    let mut data = String::new();
    // Stop reports carry the whole last reply.
    std::io::stdin()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut data)
        .map_err(|e| e.to_string())?;
    HookEvent::parse(&data)
}

fn hook(pane: Uuid, generation: Uuid) -> Result<(), String> {
    apply_hook(pane, generation, &read_hook_event()?, false)
}

/// A user-level hook: the pane is the nearest registered launch monitor
/// above this process. Silent when there is none (agent started elsewhere).
fn ancestry_hook() -> Result<(), String> {
    let event = read_hook_event()?;
    let Some(monitor) = agent_user_hooks::process_table()
        .and_then(|table| agent_user_hooks::owning_monitor(&table, std::process::id()))
    else {
        return Ok(());
    };
    // Hook environments may be cleared: the monitor names its organisation.
    dragent_paths::select_organisation(&monitor.organisation_id);
    apply_hook(monitor.pane, monitor.generation, &event, true)
}

fn activity_event(event: &HookEvent) -> Option<&'static str> {
    if event.subagent {
        return None;
    }
    match event.event.as_str() {
        "UserPromptSubmit" | "Start" | "PreToolUse" | "PostToolUse" => Some("Start"),
        "Stop" | "SessionIdle" | "SessionEnd" | "Interrupt" => Some("Stop"),
        "PermissionRequest" => Some("PermissionRequest"),
        "Failed" | "StopFailure" => Some("Failed"),
        _ => None,
    }
}

fn apply_hook(
    pane: Uuid,
    generation: Uuid,
    event: &HookEvent,
    by_ancestry: bool,
) -> Result<(), String> {
    if event.subagent {
        return Ok(());
    }
    let _lock = lock(pane)?;
    let Some(mut record) = load(pane).filter(|r| r.generation == generation && r.active) else {
        return Ok(());
    };
    let recovery = record.recovery();
    if by_ancestry
        && !matches!(
            recovery.as_ref().map(|r| &r.strategy),
            Some(SessionIdStrategy::Hook {
                hook: HookKind::UserConfig { .. }
            })
        )
    {
        // A user-level hook of some other agent run inside this pane.
        return Ok(());
    }
    if !recovery.is_some_and(|r| r.valid_id(&event.session_id)) {
        return Err("invalid provider session id".into());
    }
    // Activity hooks from nested sessions must not replace the parent binding.
    let activity = activity_event(event);
    if activity.is_some()
        && event.event != "UserPromptSubmit"
        && record
            .conversation_id
            .as_deref()
            .is_some_and(|id| id != event.session_id)
    {
        return Ok(());
    }
    if record.apply_hook(event, by_ancestry) {
        save(&record)?;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    match (activity, &event.last_message) {
        (Some("Stop"), Some(text)) => {
            let reply = AgentReply {
                generation,
                conversation_id: event.session_id.clone(),
                text: clip(text, REPLY_LIMIT),
                at_ms: now_ms,
            };
            write_atomic(
                &directory(pane),
                REPLY,
                &serde_json::to_vec(&reply).map_err(|e| e.to_string())?,
            )?;
        }
        // A new turn: the last reply no longer answers it.
        (Some("Start"), _) => {
            let _ = fs::remove_file(directory(pane).join(REPLY));
        }
        _ => {}
    }
    if let Some(event_name) = activity {
        if load_activity(pane).is_some_and(|last| last.event == event_name) {
            return Ok(());
        }
        let activity = AgentActivity {
            generation,
            conversation_id: event.session_id.clone(),
            event: event_name.into(),
            at_ms: now_ms,
        };
        write_atomic(
            &directory(pane),
            "activity.json",
            &serde_json::to_vec(&activity).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}

/// Codex trusts an individual normalized hook identity. Trust only the
/// app-owned recorder in session flags, preserving all other hook policies.
/// Matches codex-rs/{hooks/src/engine/discovery,config/src/fingerprint}.rs.
fn codex_capture_config(command: &str) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let matcher = "startup|resume|clear|compact|fork";
    let mut identity = serde_json::json!({
        "event_name":"session_start", "matcher":matcher,
        "hooks":[{"type":"command", "command":command, "timeout":5, "async":false}]
    });
    identity.sort_all_objects();
    let hash = Sha256::digest(serde_json::to_vec(&identity).unwrap());
    let hash = format!("sha256:{hash:x}");
    #[cfg(windows)]
    let source = r"C:\<session-flags>\config.toml:session_start:0:0";
    #[cfg(not(windows))]
    let source = "/<session-flags>/config.toml:session_start:0:0";
    let mut args = vec![
        "-c".into(),
        format!(
            "hooks.SessionStart=[{{matcher={},hooks=[{{type=\"command\",command={},timeout=5}}]}}]",
            serde_json::to_string(matcher).unwrap(),
            serde_json::to_string(command).unwrap()
        ),
        "-c".into(),
        format!(
            "hooks.state={{{}={{trusted_hash={}}}}}",
            serde_json::to_string(source).unwrap(),
            serde_json::to_string(&hash).unwrap()
        ),
    ];
    // SessionStart captures identity; turn hooks drive activity independently.
    for (event, normalized) in [
        ("UserPromptSubmit", "user_prompt_submit"),
        ("PostToolUse", "post_tool_use"),
        ("PermissionRequest", "permission_request"),
        ("Stop", "stop"),
        ("Interrupt", "interrupt"),
        ("SessionEnd", "session_end"),
    ] {
        let mut identity = serde_json::json!({
            "event_name":normalized, "matcher":null,
            "hooks":[{"type":"command", "command":command, "timeout":5, "async":false}]
        });
        identity.sort_all_objects();
        let hash = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&identity).unwrap())
        );
        let source = source.replace("session_start", normalized);
        args.extend([
            "-c".into(),
            format!(
                "hooks.{event}=[{{hooks=[{{type=\"command\",command={},timeout=5}}]}}]",
                serde_json::to_string(command).unwrap()
            ),
            "-c".into(),
            format!(
                "hooks.state.{}={{trusted_hash={}}}",
                serde_json::to_string(&source).unwrap(),
                serde_json::to_string(&hash).unwrap()
            ),
        ]);
    }
    args
}

fn install_capture(
    command: &mut Command,
    record: &AgentSession,
    hook: &HookKind,
    args: &mut Vec<String>,
) -> Result<(), String> {
    let helper = std::env::current_exe().map_err(|e| e.to_string())?;
    // Hooks name their organisation: a provider may run them with a
    // cleared environment, and the record lives in that organisation's store.
    let organisation = format!(
        "{}={}",
        dragent_paths::ORGANISATION_ARG,
        record.organisation_id
    );
    let helper_args = vec![
        helper.to_string_lossy().to_string(),
        HOOK_ARG.into(),
        record.pane_uuid.to_string(),
        record.generation.to_string(),
        organisation.clone(),
    ];
    // Provider command hooks use their native shell, not the pane's shell.
    #[cfg(windows)]
    let hook_command = format!(
        "\"{}\" {HOOK_ARG} {} {} \"{organisation}\"",
        helper.display(),
        record.pane_uuid,
        record.generation
    );
    #[cfg(not(windows))]
    let hook_command = helper_args
        .iter()
        .map(|a| quote(a, "sh"))
        .collect::<Vec<_>>()
        .join(" ");
    match hook {
        HookKind::CodexSessionStart => args.extend(codex_capture_config(&hook_command)),
        HookKind::ClaudePlugin => {
            // A local plugin adds our hook alongside existing user/project hooks;
            // a --settings override would replace their SessionStart definition.
            let plugin = directory(record.pane_uuid).join(format!("claude-{}", record.generation));
            fs::create_dir_all(plugin.join(".claude-plugin")).map_err(|e| e.to_string())?;
            fs::create_dir_all(plugin.join("hooks")).map_err(|e| e.to_string())?;
            fs::write(
                plugin.join(".claude-plugin/plugin.json"),
                r#"{"name":"dragent","version":"1.0.0"}"#,
            )
            .map_err(|e| e.to_string())?;
            let entry = serde_json::json!([{"hooks":[{"type":"command","command":hook_command,"timeout":5}]}]);
            let hooks = serde_json::json!({"hooks":{"SessionStart":entry,"UserPromptSubmit":entry,"PreToolUse":entry,"PostToolUse":entry,"Stop":entry,"StopFailure":entry,"SessionEnd":entry,"PermissionRequest":entry}});
            fs::write(plugin.join("hooks/hooks.json"), hooks.to_string())
                .map_err(|e| e.to_string())?;
            // Shared Agent Skills ride along in the same plugin. Best effort:
            // the agent must start even if a skill can't be linked.
            if let Some(home) = dirs::home_dir() {
                if let Err(error) =
                    crate::agent_skills::link_into_claude_plugin(&plugin, &record.cwd, &home)
                {
                    log::warn!("[agent-sessions] linking skills for Claude Code: {error}");
                }
                if crate::agent_skills::dragent_command_installed() {
                    if let Err(error) =
                        crate::agent_skills::add_bundled_skills(&plugin, &record.cwd, &home)
                    {
                        log::warn!("[agent-sessions] adding Dragent skills: {error}");
                    }
                }
            }
            args.extend(["--plugin-dir".into(), plugin.to_string_lossy().to_string()]);
            // The status line reports the account's plan limits (see
            // `agent_usage`) and then shows the person's own status line.
            // A session started with its own --settings keeps it untouched.
            let own_settings = args
                .iter()
                .any(|arg| arg == "--settings" || arg.starts_with("--settings="));
            if !own_settings {
                let account = crate::agent_usage::account_key(record.account.as_ref());
                #[cfg(windows)]
                let status_line = format!(
                    "\"{}\" {} {account}",
                    helper.display(),
                    crate::agent_usage::STATUS_LINE_ARG
                );
                #[cfg(not(windows))]
                let status_line = [
                    helper.to_string_lossy().to_string(),
                    crate::agent_usage::STATUS_LINE_ARG.to_string(),
                    account,
                ]
                .iter()
                .map(|a| quote(a, "sh"))
                .collect::<Vec<_>>()
                .join(" ");
                args.extend([
                    "--settings".into(),
                    crate::agent_usage::claude_settings(&status_line),
                ]);
            }
        }
        HookKind::OpenCodePlugin { config_env } => {
            let file =
                directory(record.pane_uuid).join(format!("capture-{}.mjs", record.generation));
            // The plugin is invocation-local. Ignore child-agent sessions and
            // obtain the actual selected root session on user message/switch.
            let script = include_str!("opencode_activity.mjs").replace(
                "__DRAGENT_HELPER_ARGS__",
                &serde_json::to_string(&helper_args).unwrap(),
            );
            fs::write(&file, script).map_err(|e| e.to_string())?;
            let variable = config_env.as_str();
            let mut config: serde_json::Value = match std::env::var(variable) {
                Ok(existing) => serde_json::from_str(&existing)
                    .map_err(|e| format!("invalid {variable}: {e}"))?,
                Err(_) => serde_json::json!({}),
            };
            let plugins = config
                .as_object_mut()
                .ok_or("provider inline config must be an object")?
                .entry("plugin")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or("provider plugins must be an array")?;
            #[cfg(windows)]
            let uri = format!("file:///{}", file.to_string_lossy().replace('\\', "/"));
            #[cfg(not(windows))]
            let uri = format!("file://{}", file.display());
            plugins.push(serde_json::Value::String(uri));
            command.env(variable, config.to_string());
        }
        HookKind::UserConfig {
            file,
            xdg,
            section,
            seed,
        } => {
            // Best effort: without it the conversation is simply unnamed and
            // is never resumed; the agent itself must still start. The hook
            // is a POSIX shell script resolved through `ps`: Unix only.
            #[cfg(unix)]
            let _ = agent_user_hooks::ensure_installed(
                file,
                *xdg,
                section.as_deref(),
                seed.as_deref(),
                &helper,
            );
            #[cfg(not(unix))]
            let _ = (file, xdg, section, seed);
        }
    }
    if matches!(
        hook,
        HookKind::ClaudePlugin | HookKind::CodexSessionStart | HookKind::OpenCodePlugin { .. }
    ) {
        write_atomic(
            &directory(record.pane_uuid),
            "activity-version",
            record.generation.to_string().as_bytes(),
        )?;
    }
    Ok(())
}

/// The program to spawn for an agent invocation. On Windows a bare name only
/// matches `<name>.exe`, but npm installs agents as `.cmd` shims that the
/// shell found through PATHEXT: resolve it the same way, or wrapping would
/// stop those agents from starting.
pub(crate) fn program(executable: &str) -> PathBuf {
    #[cfg(windows)]
    if let Some(found) = resolve_program(
        executable,
        std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default(),
        &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()),
        |path| path.is_file(),
    ) {
        return found;
    }
    PathBuf::from(executable)
}

/// The first `name` + extension that exists, searching `dirs` in order (or
/// beside `name` when it is a path). A name with an extension is left to
/// the OS, which resolves it exactly.
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_program(
    name: &str,
    dirs: Vec<PathBuf>,
    extensions: &str,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = Path::new(name);
    if name.is_empty() || path.extension().is_some() {
        return None;
    }
    let bases = if path.components().count() > 1 || path.is_absolute() {
        vec![path.to_path_buf()]
    } else {
        dirs.into_iter().map(|dir| dir.join(name)).collect()
    };
    bases.into_iter().find_map(|base| {
        extensions
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| {
                let mut candidate = base.clone().into_os_string();
                candidate.push(ext);
                PathBuf::from(candidate)
            })
            .find(|candidate| exists(candidate))
    })
}

/// Ask the provider for a new conversation id (`cursor-agent create-chat`).
/// Any failure leaves the launch unnamed: it is never matched up later.
fn mint(
    executable: &str,
    command: &[String],
    cwd: &Path,
    recovery: &SessionRecovery,
    account: &crate::agent_accounts::Binding,
) -> Option<String> {
    let mut process = Command::new(program(executable));
    crate::agent_accounts::configure(&mut process, account).ok()?;
    let mut child = process
        .args(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + MINT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let mut out = String::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_string(&mut out)
        .ok()?;
    let id = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()?;
    recovery.valid_id(id).then(|| id.to_owned())
}

fn run(pane: Uuid, argv: Vec<String>) -> Result<i32, String> {
    let executable = argv.first().ok_or("missing executable")?.clone();
    let (provider, recovery) = resolve(pane, &executable).ok_or("unsupported provider")?;
    let mut args = argv[1..].to_vec();
    let cwd = std::env::current_dir()
        .and_then(|p| p.canonicalize())
        .map_err(|e| e.to_string())?;
    let account = launch_account(pane, &provider, &cwd)?;
    crate::remote_mcp::register_account(&account)?;
    if account.selection != crate::agent_accounts::Selection::System {
        let status = crate::agent_accounts::status(&account, &program(&executable))?;
        if status != "Signed in" { return Err("Selected account needs sign-in in Settings → Agent Accounts".into()); }
    }
    let launch = recovery.parse_launch(&args);
    // A prompt given on the command line or piped in (heredoc delivery)
    // starts the conversation immediately.
    let prompted = launch.as_ref().is_some_and(|launch| launch.prompted)
        || !std::io::IsTerminal::is_terminal(&std::io::stdin());
    let resume_options = launch.map(|launch| launch.options);
    let resumed = recovery.is_resume(&args);
    let mut conversation_id = recovery.selected_id(&args);
    // A new interactive conversation gets its exact identity before the
    // provider starts, so it is durable even if the machine dies mid-turn.
    let mut named = Vec::new();
    if resume_options.is_some() && conversation_id.is_none() && !resumed {
        let id = match &recovery.strategy {
            SessionIdStrategy::Preassigned { .. } => Some(Uuid::new_v4().to_string()),
            SessionIdStrategy::Minted { command, .. } => {
                mint(&executable, command, &cwd, &recovery, &account)
            }
            SessionIdStrategy::Hook { .. } => None,
        };
        if let Some(id) = id {
            named = recovery.new_conversation_args(&id);
            conversation_id = Some(id);
        }
    }
    let mut record = AgentSession {
        organisation_id: dragent_paths::organisation_id(),
        pane_uuid: pane,
        generation: Uuid::new_v4(),
        provider,
        account: Some(account.clone()),
        cwd,
        executable,
        resume_options,
        resumed,
        last_exit_code: None,
        conversation_id,
        active: true,
        workspace_id: std::env::var("DRAGENT_HOST_WORKSPACE_ID").unwrap_or_default(),
        terminal_id: std::env::var("DRAGENT_HOST_TERMINAL_ID").unwrap_or_default(),
        recovery: Some(recovery.clone()),
        used: resumed || prompted,
    };
    let mut command = Command::new(program(&record.executable));
    crate::agent_accounts::configure(&mut command, &account)?;
    {
        let _lock = lock(pane)?;
        // Resuming the pane's own conversation keeps what is known about it.
        if let Some(previous) = load(pane).filter(|previous| {
            record.conversation_id.is_some() && previous.conversation_id == record.conversation_id
        }) {
            record.used = previous.used || prompted;
        }
        save(&record)?;
        if !record.terminal_id.is_empty() {
            let _ = index_terminal(&record.terminal_id, pane);
        }
    }
    // User-level hooks find this monitor by process ancestry.
    let registered = matches!(
        &recovery.strategy,
        SessionIdStrategy::Hook {
            hook: HookKind::UserConfig { .. }
        }
    ) && record.resume_options.is_some()
        && agent_user_hooks::process_started(std::process::id())
            .map(|started| {
                agent_user_hooks::register(&agent_user_hooks::Monitor {
                    organisation_id: record.organisation_id.clone(),
                    pane,
                    generation: record.generation,
                    started,
                })
                .is_ok()
            })
            .unwrap_or(false);
    #[cfg(unix)]
    let parent = unsafe { libc::getppid() };
    // Inject flags ahead of a `--` separator/initial prompt; adding them after
    // that separator would turn the hook configuration into prompt text.
    let original_len = args.len();
    let capture = match &recovery.strategy {
        SessionIdStrategy::Hook { hook } if record.resume_options.is_some() => {
            install_capture(&mut command, &record, hook, &mut args)
        }
        _ => Ok(()),
    };
    args.extend(named);
    // Move injected flags in front of original positional arguments.
    let added = args.split_off(original_len);
    args.splice(0..0, added);
    let spawned = capture.and_then(|_| command.args(&args).spawn().map_err(|e| e.to_string()));
    #[cfg(unix)]
    unsafe {
        // The child already inherited the shell's normal signal disposition.
        // Let Ctrl-C interrupt it while this monitor survives to record exit.
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        // Shutdown and hang-up reach this monitor too: note them and keep
        // waiting, so an agent that exits cleanly on SIGTERM is not mistaken
        // for the user ending the conversation.
        libc::signal(
            libc::SIGTERM,
            note_shutdown as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            note_shutdown as *const () as libc::sighandler_t,
        );
    }
    #[cfg(windows)]
    console_events::install();
    let result = spawned.and_then(|mut child| child.wait().map_err(|e| e.to_string()));
    if registered {
        agent_user_hooks::unregister();
    }
    let code = result.as_ref().map(exit_code).unwrap_or(1);
    #[cfg(unix)]
    let interrupted = result
        .as_ref()
        .is_ok_and(|status| killed_externally(status) || shutting_down(parent));
    #[cfg(windows)]
    let interrupted = (result.is_ok() && console_events::closing())
        || crate::pty_supervisor::session_interrupted_by_update(&record.terminal_id);
    #[cfg(not(any(unix, windows)))]
    let interrupted = false;
    {
        let _lock = lock(pane)?;
        if let Some(current) = load(pane).filter(|r| r.generation == record.generation) {
            record = current;
            // Shutdown/hang-up is not the user ending the conversation: it
            // stays resumable for the next boot.
            record.active = interrupted;
            record.last_exit_code = Some(code);
            save(&record)?;
        }
    }
    #[cfg(windows)]
    console_events::recorded();
    result?;
    Ok(code)
}

/// Windows delivers Ctrl+C and console close/logoff/shutdown to every process
/// on the console. The monitor leaves interrupts to the agent (otherwise the
/// shell would take the console back while the agent still runs), and notes
/// a closing console so an agent that then exits cleanly stays resumable.
#[cfg(windows)]
mod console_events {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };

    static CLOSING: AtomicBool = AtomicBool::new(false);
    static RECORDED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn handle(event: u32) -> i32 {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => 1,
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                CLOSING.store(true, Ordering::SeqCst);
                // The process ends when this returns: first let the monitor
                // record that its agent was interrupted rather than ended.
                let deadline = Instant::now() + Duration::from_secs(3);
                while !RECORDED.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                1
            }
            _ => 0,
        }
    }

    pub fn install() {
        unsafe {
            SetConsoleCtrlHandler(Some(handle), 1);
        }
    }

    /// A close/shutdown event arrived, allowing it the same grace as Unix.
    pub fn closing() -> bool {
        let deadline = Instant::now() + super::SHUTDOWN_GRACE;
        while !CLOSING.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    pub fn recorded() {
        RECORDED.store(true, Ordering::SeqCst);
    }
}

#[cfg(unix)]
static SHUTDOWN_SIGNALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn note_shutdown(_: libc::c_int) {
    SHUTDOWN_SIGNALLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// How long a clean agent exit waits for signs of a system shutdown. The OS
/// signals every process nearly at once, so an agent that handled its own
/// SIGTERM may exit a moment before this monitor hears about it.
#[cfg(any(unix, windows))]
const SHUTDOWN_GRACE: Duration = Duration::from_millis(150);

/// This monitor was signalled to terminate or hang up, or its shell died.
#[cfg(unix)]
fn shutting_down(parent: libc::pid_t) -> bool {
    let signalled = || {
        SHUTDOWN_SIGNALLED.load(std::sync::atomic::Ordering::SeqCst)
            || unsafe { libc::getppid() } != parent
    };
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while !signalled() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// The agent was killed from outside (system shutdown, terminal hang-up),
/// rather than exiting or being interrupted by the user.
#[cfg(unix)]
fn killed_externally(status: &std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status
        .signal()
        .is_some_and(|signal| matches!(signal, libc::SIGHUP | libc::SIGTERM | libc::SIGKILL))
}

/// Dispatch before GUI initialization, storage migrations, and logger setup.
/// Hooks are silent; launching an agent inherits the original terminal I/O.
pub fn run_if_requested() -> Option<i32> {
    // Older hooks and launches carry no organisation argument: `local`.
    let mut args =
        dragent_paths::take_organisation_arg(std::env::args().skip(1).collect()).into_iter();
    let mode = args.next()?;
    if mode == "--dragent-agent-account-login" {
        return Some(match args.next().and_then(|s| s.parse().ok()).ok_or_else(|| "Missing account ID".to_string()).and_then(crate::agent_accounts::login) {
            Ok(code) => code,
            Err(error) => { eprintln!("Agent account: {error}"); 1 }
        });
    }
    #[cfg(not(target_family = "wasm"))]
    if mode == crate::agent_usage::STATUS_LINE_ARG {
        // Claude Code's status line: never fails, prints what it shows.
        let account = args.next().unwrap_or_else(|| "system".into());
        return Some(crate::agent_usage::run_status_line(&account));
    }
    if mode == agent_user_hooks::ANCESTRY_HOOK_ARG {
        // Runs for every agent that has the user-level hook, inside Dragent
        // or not: never fail or print, the agent may surface hook errors.
        let _ = ancestry_hook();
        return Some(0);
    }
    if mode != RUN_ARG && mode != HOOK_ARG {
        return None;
    }
    let result = (|| {
        let pane =
            Uuid::parse_str(&args.next().ok_or("missing pane uuid")?).map_err(|e| e.to_string())?;
        if mode == HOOK_ARG {
            let generation = Uuid::parse_str(&args.next().ok_or("missing generation")?)
                .map_err(|e| e.to_string())?;
            hook(pane, generation)?;
            Ok(0)
        } else {
            let separator = args.next();
            let provider = if separator.as_deref() == Some("--provider") {
                let provider = args.next().ok_or("Missing provider")?;
                if args.next().as_deref() != Some("--") { return Err("missing argv separator".into()); }
                Some(provider)
            } else {
                if separator.as_deref() != Some("--") { return Err("missing argv separator".into()); }
                None
            };
            let argv:Vec<String> = args.collect();
            if let Some(provider) = provider {
                let recovery = agent_recovery::builtin(&provider).ok_or("Unsupported remote provider")?;
                stage_custom_agent(pane, &provider, argv.first().ok_or("Missing executable")?, &recovery)?;
            }
            run(pane, argv)
        }
    })();
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Agent session recovery: {error}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(provider: &str) -> AgentSession {
        AgentSession {
            organisation_id: dragent_paths::organisation_id(),
            pane_uuid: Uuid::new_v4(),
            generation: Uuid::new_v4(),
            provider: provider.into(),
            account: None,
            cwd: std::env::current_dir().unwrap().canonicalize().unwrap(),
            executable: provider.into(),
            resume_options: Some(vec!["--model".into(), "test-model".into()]),
            conversation_id: Some("session_A-123".into()),
            active: true,
            resumed: false,
            last_exit_code: None,
            workspace_id: "workspace".into(),
            terminal_id: "pty".into(),
            // Records written before descriptors existed resolve built-ins.
            recovery: None,
            used: true,
        }
    }

    fn resume_options(provider: &str, args: &[String]) -> Option<Vec<String>> {
        agent_recovery::builtin(provider)
            .unwrap()
            .resume_options(args)
    }

    fn is_resume(provider: &str, args: &[String]) -> bool {
        agent_recovery::builtin(provider).unwrap().is_resume(args)
    }

    #[test]
    fn only_a_finished_turn_or_an_unused_session_is_between_turns() {
        assert!(between_turns(Some("Stop"), true));
        assert!(between_turns(Some("Failed"), true));
        assert!(!between_turns(Some("Start"), true));
        assert!(!between_turns(Some("PermissionRequest"), true));
        assert!(between_turns(None, false));
        // Used, but nothing reported: its hooks may not be running.
        assert!(!between_turns(None, true));
        // Resumed and asked nothing since: resting, so it may sleep.
        assert!(resting(None, true, true));
        assert!(!resting(None, true, false), "a fresh launch given a prompt may be working");
        assert!(!resting(Some("Start"), true, true));
    }

    #[test]
    fn exact_resume_commands_never_guess_latest() {
        for provider in ["claude", "codex", "opencode", "mimo"] {
            let r = record(provider);
            let command = resume_command(&r, &r.cwd, "zsh").unwrap();
            assert!(command.contains("'session_A-123'"), "{command}");
            assert!(!command.contains("--last") && !command.contains("--continue"));
            assert!(command.contains(if provider == "claude" {
                "--resume"
            } else if provider == "codex" {
                "'resume'"
            } else {
                "--session"
            }));
            let mut b = r.clone();
            b.conversation_id = Some("session_B-456".into());
            assert_ne!(command, resume_command(&b, &b.cwd, "zsh").unwrap());
        }
    }

    #[test]
    fn no_resume_without_identity_or_matching_directory() {
        let mut r = record("claude");
        assert!(resume_command(&r, Path::new("/not-a-directory"), "sh").is_none());
        r.conversation_id = None;
        assert!(resume_command(&r, &r.cwd, "sh").is_none());
        r.conversation_id = Some("wrong; shell injection".into());
        assert!(resume_command(&r, &r.cwd, "sh").is_none());
        r.conversation_id = Some("valid".into());
        r.active = false;
        assert!(resume_command(&r, &r.cwd, "sh").is_none());
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn strips_prompts_and_selection_but_preserves_launch_options() {
        assert_eq!(
            resume_options(
                "claude",
                &strings(&[
                    "--model",
                    "sonnet",
                    "--resume",
                    "old",
                    "--fork-session",
                    "original prompt"
                ])
            ),
            Some(strings(&["--model", "sonnet"]))
        );
        assert_eq!(
            resume_options(
                "codex",
                &strings(&[
                    "resume",
                    "old",
                    "--last",
                    "-c",
                    "model='custom'",
                    "--",
                    "original prompt"
                ])
            ),
            Some(strings(&["-c", "model='custom'"]))
        );
        for provider in ["opencode", "mimo"] {
            assert_eq!(
                resume_options(
                    provider,
                    &strings(&[
                        "--session=old",
                        "--prompt",
                        "do not replay",
                        "--model",
                        "model"
                    ])
                ),
                Some(strings(&["--model", "model"]))
            );
            assert!(resume_options(provider, &strings(&["run", "one shot"])).is_none());
        }
        assert!(resume_options("claude", &strings(&["--print", "one shot"])).is_none());
        assert!(!is_resume("claude", &strings(&["--session-id", "new-id"])));
    }

    #[test]
    fn remote_wrapper_uses_the_hosts_context_and_preserves_the_command() {
        let pane = Uuid::new_v4();
        let command = "claude --model sonnet 'hello $world'";
        let wrapped = wrap_remote_command(pane, "sh", command);
        assert!(wrapped.starts_with("'dragent' --dragent-agent-run "));
        assert!(wrapped.ends_with(command));
        assert!(!wrapped.contains("--dragent-organisation"));
        assert_eq!(wrap_remote_command(pane, "sh", "echo hello"), "echo hello");
    }

    #[test]
    fn wrapper_preserves_shell_prompt_delivery() {
        let pane = Uuid::new_v4();
        let command = "claude --model sonnet <<'PROMPT'\nhello $world\nPROMPT";
        let wrapped =
            wrap_command_with_helper(pane, "zsh", command, Path::new("/app with spaces/dragent"));
        assert!(wrapped.ends_with(command));
        assert_eq!(
            dragent_terminal::shell_integration::agent_display_command(&wrapped),
            command
        );
        assert_eq!(
            wrap_command_with_helper(pane, "sh", "echo test", Path::new("/app")),
            "echo test"
        );
        assert_eq!(quote("a'b", "pwsh"), "'a''b'");
        assert_eq!(quote("a'b", "fish"), "'a\\'b'");
        assert_eq!(quote("a'b", "zsh"), "'a'\\''b'");
    }

    #[test]
    fn codex_trust_is_scoped_to_the_exact_recorder() {
        let config = codex_capture_config("recorder");
        assert!(config[3].contains("hooks.state={"));
        assert!(config[3].contains("session_start:0:0"));
        assert!(config[3]
            .contains("sha256:c2ea4c9baad70a65f6c7b3848b1b2f30af9b0afbfface5567b25a04e89013a25"));
        assert_ne!(config[3], codex_capture_config("changed recorder")[3]);
        assert!(!config.join(" ").contains("bypass-hook-trust"));
    }

    #[test]
    fn preserves_prompt_suffix_flags_and_excludes_maintenance_commands() {
        assert_eq!(
            resume_options("claude", &strings(&["original prompt", "--effort", "high"])),
            Some(strings(&["--effort", "high"]))
        );
        for command in ["exec", "login", "app-server", "mcp-server"] {
            assert!(resume_options("codex", &strings(&[command])).is_none());
        }
        assert_eq!(
            resume_options("codex", &strings(&["fork", "source-id", "--model", "test"])),
            Some(strings(&["--model", "test"]))
        );
    }

    #[test]
    fn failed_resume_is_distinct_from_normal_exit() {
        let mut r = record("codex");
        r.active = false;
        r.last_exit_code = Some(2);
        assert!(!r.resume_failed());
        r.resumed = true;
        assert!(r.resume_failed());
        r.last_exit_code = Some(0);
        assert!(!r.resume_failed());
    }

    #[test]
    fn preassigned_agents_resume_their_own_uuid() {
        let id = "0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51";
        for (provider, shape) in [
            ("gemini", "--resume"),
            ("qwen", "--resume"),
            ("copilot", "--resume"),
            ("codebuddy", "--resume"),
            ("kimi", "--session"),
            ("cursor-agent", "--resume"),
        ] {
            let mut r = record(provider);
            r.conversation_id = Some(id.into());
            let command = resume_command(&r, &r.cwd, "zsh").unwrap();
            assert!(
                command.ends_with(&format!("'{shape}' '{id}'")),
                "{provider}: {command}"
            );
            assert!(command.starts_with(&format!("{provider} '--model' 'test-model'")));
            // Non-UUID ids never reach a UUID agent's resume command.
            if provider != "kimi" {
                r.conversation_id = Some("latest".into());
                assert!(resume_command(&r, &r.cwd, "zsh").is_none(), "{provider}");
            }
        }
    }

    #[test]
    fn fresh_session_reuses_options_without_the_conversation() {
        let mut r = record("gemini");
        r.conversation_id = Some("0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51".into());
        assert_eq!(
            fresh_command(&r, "zsh").unwrap(),
            "gemini '--model' 'test-model'"
        );
        r.resume_options = None;
        assert!(fresh_command(&r, "zsh").is_none());
    }

    #[test]
    fn custom_agents_use_their_recorded_descriptor() {
        let mut r = record("custom:acme");
        r.executable = "acme".into();
        r.conversation_id = Some("0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51".into());
        // Unknown provider without a descriptor: never resumed.
        assert!(resume_command(&r, &r.cwd, "sh").is_none());
        r.recovery = Some(SessionRecovery {
            resume: vec!["--conversation".into(), agent_recovery::ID.into()],
            ..agent_recovery::builtin("gemini").unwrap()
        });
        assert_eq!(
            resume_command(&r, &r.cwd, "sh").unwrap(),
            "acme '--model' 'test-model' '--conversation' '0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51'"
        );
    }

    #[test]
    fn only_agents_with_exact_identity_are_wrapped() {
        let pane = Uuid::new_v4();
        let helper = Path::new("/app");
        for agent in [
            "gemini --approval-mode=auto_edit",
            "qwen",
            "copilot",
            "cursor-agent",
        ] {
            assert_ne!(wrap_command_with_helper(pane, "sh", agent, helper), agent);
        }
        let user_config =
            ["droid", "muse"].map(|agent| wrap_command_with_helper(pane, "sh", agent, helper));
        assert_eq!(user_config != ["droid", "muse"], cfg!(unix));
        for agent in ["crush", "vibe", "auggie", "goose"] {
            assert_eq!(wrap_command_with_helper(pane, "sh", agent, helper), agent);
        }
    }

    fn event(id: &str, name: &str, source: Option<&str>) -> HookEvent {
        HookEvent {
            session_id: id.into(),
            event: name.into(),
            source: source.map(str::to_owned),
            subagent: false,
            last_message: None,
        }
    }

    #[test]
    fn activity_notifications_dedupe_and_do_not_replay_on_reconnect() {
        let activity = AgentActivity {
            generation: Uuid::new_v4(),
            conversation_id: "root".into(),
            event: "Stop".into(),
            at_ms: 20,
        };
        assert_eq!(activity.notification_after(None), None);
        assert_eq!(
            activity.notification_after(Some(&(activity.generation, 20, "Stop".into()))),
            None
        );
        assert_eq!(
            activity.notification_after(Some(&(Uuid::new_v4(), 10, "Start".into()))),
            None
        );
        assert_eq!(
            activity.notification_after(Some(&(activity.generation, 10, "Stop".into()))),
            Some("Finished responding")
        );
        // Going quiet isn't news: nothing says it finished.
        let quiet = AgentActivity {
            generation: activity.generation,
            conversation_id: activity.conversation_id.clone(),
            event: ACTIVITY_QUIET.into(),
            at_ms: 30,
        };
        assert_eq!(
            quiet.notification_after(Some(&(activity.generation, 10, "Start".into()))),
            None
        );
        assert!(!between_turns(Some(ACTIVITY_QUIET), true));
        for event in ["PermissionRequest", "Failed"] {
            let next = AgentActivity {
                event: event.into(),
                ..activity.clone()
            };
            assert!(next
                .notification_after(Some(&(activity.generation, 10, "Start".into())))
                .is_some());
        }
    }

    #[test]
    fn activity_tracks_turns_not_process_lifetime() {
        for (event_name, state) in [
            ("SessionStart", None),
            ("UserPromptSubmit", Some("Start")),
            ("PermissionRequest", Some("PermissionRequest")),
            ("PostToolUse", Some("Start")),
            ("Stop", Some("Stop")),
            ("StopFailure", Some("Failed")),
            ("Interrupt", Some("Stop")),
        ] {
            assert_eq!(activity_event(&event("root", event_name, None)), state);
        }
        let child = HookEvent::parse(
            r#"{"session_id":"root","hook_event_name":"Stop","agent_id":"child"}"#,
        )
        .unwrap();
        assert_eq!(activity_event(&child), None);
    }

    #[test]
    fn codex_installs_activity_hooks_in_addition_to_identity() {
        let config = codex_capture_config("recorder");
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "PermissionRequest",
            "Interrupt",
        ] {
            assert!(config
                .iter()
                .any(|arg| arg.starts_with(&format!("hooks.{event}="))));
        }
    }

    #[test]
    fn hook_reports_track_identity_and_use() {
        let mut r = record("claude");
        r.conversation_id = None;
        r.used = false;
        assert!(r.apply_hook(&event("a", "SessionStart", Some("startup")), false));
        assert_eq!(r.conversation_id.as_deref(), Some("a"));
        assert!(!r.used);
        assert!(r.apply_hook(&event("a", "UserPromptSubmit", None), false));
        assert!(r.used);
        // /clear starts an empty conversation; picking one resumes a used one.
        assert!(r.apply_hook(&event("b", "SessionStart", Some("clear")), false));
        assert_eq!((r.conversation_id.as_deref(), r.used), (Some("b"), false));
        assert!(r.apply_hook(&event("c", "SessionStart", Some("resume")), false));
        assert_eq!((r.conversation_id.as_deref(), r.used), (Some("c"), true));
        assert!(!r.apply_hook(&event("c", "SessionStart", Some("compact")), false));
    }

    #[test]
    fn user_level_hooks_ignore_nested_runs_of_the_same_agent() {
        let mut r = record("muse");
        r.conversation_id = None;
        r.used = false;
        assert!(r.apply_hook(&event("own", "SessionStart", Some("startup")), true));
        // A nested run inside the pane reports its own startup and prompts.
        assert!(!r.apply_hook(&event("nested", "SessionStart", Some("startup")), true));
        assert!(!r.apply_hook(&event("nested", "UserPromptSubmit", None), true));
        assert_eq!((r.conversation_id.as_deref(), r.used), (Some("own"), false));
        assert!(r.apply_hook(&event("own", "UserPromptSubmit", None), true));
        assert!(r.used);
        HookEvent::parse(
            r#"{"session_id":"x","hook_event_name":"SessionStart","source":"startup"}"#,
        )
        .unwrap();
        assert!(HookEvent::parse(r#"{"hook_event_name":"Stop"}"#).is_err());
    }

    #[test]
    fn stop_reports_keep_the_last_reply() {
        let stop = HookEvent::parse(
            r#"{"session_id":"x","hook_event_name":"Stop","last_assistant_message":"All clear"}"#,
        )
        .unwrap();
        assert_eq!(stop.last_message.as_deref(), Some("All clear"));
        let blank = HookEvent::parse(
            r#"{"session_id":"x","hook_event_name":"Stop","last_assistant_message":"  "}"#,
        )
        .unwrap();
        assert_eq!(blank.last_message, None);
        assert_eq!(clip("short", 10), "short");
        // Never splits a character.
        assert_eq!(clip("héllo", 2), "h…");
    }

    #[test]
    fn only_unused_conversations_are_started_over() {
        let mut r = record("gemini");
        r.conversation_id = Some("0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51".into());
        r.resumed = true;
        r.active = false;
        r.last_exit_code = Some(1);
        r.used = false;
        assert!(r.nothing_to_resume());
        // A used conversation that fails to resume is reported, never replaced.
        r.used = true;
        assert!(!r.nothing_to_resume());
        // A never-named conversation (created lazily by the agent) was unused.
        let mut lazy = record("opencode");
        lazy.conversation_id = None;
        lazy.used = false;
        assert!(lazy.nothing_to_resume());
        // Records from before usage tracking count as used.
        let legacy: AgentSession = serde_json::from_value({
            let mut v = serde_json::to_value(&lazy).unwrap();
            v.as_object_mut().unwrap().remove("used");
            v
        })
        .unwrap();
        assert!(legacy.used && !legacy.nothing_to_resume());
        // Starting over keeps the id Dragent chose; hook agents get a new one.
        assert!(start_over_command(&r, "zsh")
            .unwrap()
            .ends_with("'--session-id' '0190a8f2-7c1e-7d11-9b1c-4d0c2c7f2a51'"));
        assert_eq!(
            start_over_command(&record("claude"), "zsh").unwrap(),
            "claude '--model' 'test-model'"
        );
    }

    #[test]
    fn windows_style_program_resolution_finds_npm_shims() {
        let dirs = vec![PathBuf::from("/a"), PathBuf::from("/npm")];
        let exts = ".COM;.EXE;.BAT;.CMD";
        let exists = |files: &'static [&'static str]| {
            move |p: &Path| files.iter().any(|f| Path::new(f) == p)
        };
        // npm installs `codex.cmd`; the shell found it through PATHEXT.
        assert_eq!(
            resolve_program("codex", dirs.clone(), exts, exists(&["/npm/codex.CMD"])),
            Some(PathBuf::from("/npm/codex.CMD"))
        );
        // PATH order first, then PATHEXT order.
        assert_eq!(
            resolve_program(
                "claude",
                dirs.clone(),
                exts,
                exists(&["/a/claude.CMD", "/npm/claude.EXE"])
            ),
            Some(PathBuf::from("/a/claude.CMD"))
        );
        // Explicit extensions and missing programs are left to the OS.
        assert_eq!(
            resolve_program("claude.exe", dirs.clone(), exts, exists(&[])),
            None
        );
        assert_eq!(
            resolve_program("missing", dirs.clone(), exts, exists(&[])),
            None
        );
        assert_eq!(
            resolve_program("/tools/kimi", dirs, exts, exists(&["/tools/kimi.EXE"])),
            Some(PathBuf::from("/tools/kimi.EXE"))
        );
    }

    fn wake_scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-wake-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn wake_requests_round_trip_and_clear() {
        let dir = wake_scratch();
        let pane = Uuid::new_v4();
        assert!(take_wake_requests_in(&dir).is_empty());
        request_wake_in(&dir, pane, "team news").unwrap();
        let taken = take_wake_requests_in(&dir);
        assert_eq!(taken, vec![(pane, "team news".to_string())]);
        // Taking clears: nothing left.
        assert!(take_wake_requests_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wake_requests_keep_only_the_latest_reason() {
        let dir = wake_scratch();
        let pane = Uuid::new_v4();
        request_wake_in(&dir, pane, "first").unwrap();
        request_wake_in(&dir, pane, "second").unwrap();
        let taken = take_wake_requests_in(&dir);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].0, pane);
        assert_eq!(taken[0].1, "second");
        assert!(take_wake_requests_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wake_requests_cover_many_panes_and_skip_non_panes() {
        let dir = wake_scratch();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        request_wake_in(&dir, a, "a").unwrap();
        request_wake_in(&dir, b, "b").unwrap();
        std::fs::write(dir.join("not-a-uuid.json"), b"{}").unwrap();
        std::fs::write(dir.join(".taken-stale"), b"stale").unwrap();
        let mut taken = take_wake_requests_in(&dir);
        taken.sort_by_key(|(pane, _)| *pane);
        // Sorted the same way as the result, whichever uuid sorts first.
        let mut expected = vec![(a, "a".to_string()), (b, "b".to_string())];
        expected.sort_by_key(|(pane, _)| *pane);
        assert_eq!(taken, expected);
        // A request written after take starts is never lost: it stays for
        // the next take (or is returned now), never deleted unseen.
        request_wake_in(&dir, a, "again").unwrap();
        let taken = take_wake_requests_in(&dir);
        assert_eq!(taken, vec![(a, "again".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wake_request_without_a_session_record_is_a_quiet_no_op() {
        // A random pane has no session record: must succeed without writing.
        let pane = Uuid::new_v4();
        assert!(load(pane).is_none());
        assert!(request_wake(pane, "news").is_ok());
        assert!(!is_asleep(pane));
    }
}
