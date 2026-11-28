//! Native, resource-bounded language-server process broker.
//!
//! The broker discovers project, managed, and system tools and never runs on
//! the UI thread. A trusted caller may explicitly enable catalogued managed
//! installs. It owns one process per `(project root, server ID)` key, frames
//! JSON-RPC messages, bounds unread server events, and makes failure observable.

mod catalog;
mod client;
mod client_api;
mod client_requests;
mod client_sessions;
mod client_worker;
mod discovery;
mod framing;
mod installer;
mod protocol;

pub use catalog::{CatalogCandidate, CatalogInstall, CatalogServer, LanguageServerCatalog};
pub use client::{
    LanguageServiceClient, LanguageServiceClientConfig, LanguageServiceEvent, SyncedDocument,
};

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};
use thiserror::Error;

const DEFAULT_MAX_SERVERS: usize = 8;
const EVENT_QUEUE_CAPACITY: usize = 256;
const STDERR_TAIL_BYTES: usize = 4096;

/// Create a command suitable for background work in the GUI application.
///
/// On Windows every executable is created with `CREATE_NO_WINDOW`. Batch-file
/// shims are invoked by a likewise hidden `cmd.exe`; CreateProcess cannot run
/// `.cmd`/`.bat` files directly.
pub(crate) fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let program = program.as_ref();
        let is_batch = Path::new(program)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            });
        if is_batch {
            let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
            let mut command = Command::new(shell);
            command
                .args(["/d", "/s", "/c", "call"])
                .arg(program)
                .creation_flags(CREATE_NO_WINDOW);
            return command;
        }
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LanguageServerKey {
    pub project_root: PathBuf,
    pub server_id: String,
}

impl LanguageServerKey {
    pub fn new(project_root: impl Into<PathBuf>, server_id: impl Into<String>) -> Self {
        Self {
            project_root: project_root.into(),
            server_id: server_id.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ServerEvent {
    Message(Value),
    TransportClosed,
    TransportError(String),
}

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("no language server is registered with ID {0}")]
    UnsupportedServer(String),
    #[error("no installed language server was found for {0}")]
    ServerUnavailable(String),
    #[error("language-server installation failed: {0}")]
    ManagedInstallFailed(String),
    #[error("language-server limit reached ({0})")]
    CapacityReached(usize),
    #[error("project root is not a directory: {0}")]
    InvalidProjectRoot(PathBuf),
    #[error("failed to start language server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("language server did not expose {0}")]
    MissingPipe(&'static str),
    #[error("language-server transport lock is poisoned")]
    TransportPoisoned,
    #[error("failed to write LSP message: {0}")]
    Write(#[source] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct BrokerConfig {
    pub max_servers: usize,
    /// Explicit server binaries, used by advanced configuration and tests.
    /// Arguments still come from the language registry.
    pub executable_overrides: HashMap<String, PathBuf>,
    pub catalog: Arc<LanguageServerCatalog>,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            max_servers: DEFAULT_MAX_SERVERS,
            executable_overrides: HashMap::new(),
            catalog: Arc::new(LanguageServerCatalog::load()),
        }
    }
}

pub struct LanguageServerBroker {
    config: BrokerConfig,
    catalog: Arc<LanguageServerCatalog>,
    sessions: Mutex<HashMap<LanguageServerKey, LanguageServerSession>>,
}

impl Default for LanguageServerBroker {
    fn default() -> Self {
        Self::new(BrokerConfig::default())
    }
}

impl LanguageServerBroker {
    pub fn new(config: BrokerConfig) -> Self {
        let catalog = config.catalog.clone();
        Self {
            config,
            catalog,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn discover(&self, server_id: &str) -> Result<DiscoveredServer, BrokerError> {
        self.discover_for_project(server_id, Path::new("."))
    }

    pub fn discover_for_project(
        &self,
        server_id: &str,
        project_root: &Path,
    ) -> Result<DiscoveredServer, BrokerError> {
        let definition = self
            .catalog
            .server(server_id)
            .ok_or_else(|| BrokerError::UnsupportedServer(server_id.to_string()))?;
        if let Some(executable) = self.config.executable_overrides.get(server_id) {
            let Some(candidate) = definition.candidates.first().cloned() else {
                return Err(BrokerError::ServerUnavailable(server_id.to_string()));
            };
            return executable
                .is_file()
                .then(|| DiscoveredServer {
                    executable: executable.clone(),
                    candidate,
                })
                .ok_or_else(|| BrokerError::ServerUnavailable(server_id.to_string()));
        }
        definition
            .candidates
            .iter()
            .find_map(|candidate| {
                discovery::find_project_executable(project_root, &candidate.executable)
                    // Prefer a workspace-aware user/toolchain binary
                    // over Dragent's global managed cache.
                    .or_else(|| {
                        discovery::find_executable_for_project(&candidate.executable, project_root)
                    })
                    .or_else(|| discovery::find_managed_executable(&candidate.executable))
                    .map(|executable| DiscoveredServer {
                        executable,
                        candidate: candidate.clone(),
                    })
            })
            .ok_or_else(|| BrokerError::ServerUnavailable(server_id.to_string()))
    }

    pub fn ensure_started(&self, key: LanguageServerKey) -> Result<(), BrokerError> {
        if !key.project_root.is_dir() {
            return Err(BrokerError::InvalidProjectRoot(key.project_root));
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BrokerError::TransportPoisoned)?;
        if let Some(session) = sessions.get_mut(&key) {
            if session.is_running() {
                return Ok(());
            }
            sessions.remove(&key);
        }
        if sessions.len() >= self.config.max_servers {
            return Err(BrokerError::CapacityReached(self.config.max_servers));
        }
        let discovered = self.discover_for_project(&key.server_id, &key.project_root)?;
        let session = LanguageServerSession::spawn(&key, discovered)?;
        sessions.insert(key, session);
        Ok(())
    }

    /// Install the catalogued server outside the project, then return its path.
    /// This is called only by the background coordinator for a trusted project.
    pub fn install_managed(
        &self,
        server_id: &str,
        project_root: &Path,
    ) -> Result<PathBuf, BrokerError> {
        let definition = self
            .catalog
            .server(server_id)
            .ok_or_else(|| BrokerError::UnsupportedServer(server_id.to_string()))?;
        let install = definition.install.as_ref().ok_or_else(|| {
            BrokerError::ManagedInstallFailed(format!(
                "no managed installer is catalogued for {server_id}"
            ))
        })?;
        installer::install(install, &definition.candidates, project_root)
            .map_err(BrokerError::ManagedInstallFailed)
    }

    pub fn send_notification(
        &self,
        key: &LanguageServerKey,
        method: &str,
        params: Value,
    ) -> Result<(), BrokerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BrokerError::TransportPoisoned)?;
        let session = sessions
            .get_mut(key)
            .ok_or_else(|| BrokerError::ServerUnavailable(key.server_id.clone()))?;
        session.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }

    pub fn send_request(
        &self,
        key: &LanguageServerKey,
        method: &str,
        params: Value,
    ) -> Result<u64, BrokerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BrokerError::TransportPoisoned)?;
        let session = sessions
            .get_mut(key)
            .ok_or_else(|| BrokerError::ServerUnavailable(key.server_id.clone()))?;
        let id = session.next_request_id;
        session.next_request_id = session.next_request_id.saturating_add(1);
        session.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        Ok(id)
    }

    pub fn send_response(
        &self,
        key: &LanguageServerKey,
        id: Value,
        result: Value,
    ) -> Result<(), BrokerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BrokerError::TransportPoisoned)?;
        let session = sessions
            .get_mut(key)
            .ok_or_else(|| BrokerError::ServerUnavailable(key.server_id.clone()))?;
        session.send(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }

    pub fn send_error_response(
        &self,
        key: &LanguageServerKey,
        id: Value,
        code: i64,
        message: &str,
    ) -> Result<(), BrokerError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BrokerError::TransportPoisoned)?;
        let session = sessions
            .get_mut(key)
            .ok_or_else(|| BrokerError::ServerUnavailable(key.server_id.clone()))?;
        session.send(json!({
            "jsonrpc":"2.0",
            "id":id,
            "error":{"code":code,"message":message}
        }))
    }

    pub fn try_recv(&self, key: &LanguageServerKey) -> Option<ServerEvent> {
        let sessions = self.sessions.lock().ok()?;
        sessions.get(key)?.try_recv()
    }

    pub fn stop(&self, key: &LanguageServerKey) -> bool {
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        let Some(session) = sessions.remove(key) else {
            return false;
        };
        session.stop();
        true
    }

    /// The running server's process id.
    pub fn pid(&self, key: &LanguageServerKey) -> Option<u32> {
        self.sessions.lock().ok()?.get(key).map(|session| session.child.id())
    }

    pub fn running_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or(0)
    }

    pub fn is_running(&self, key: &LanguageServerKey) -> bool {
        self.sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.get_mut(key).map(LanguageServerSession::is_running))
            .unwrap_or(false)
    }

    pub fn exit_reason(&self, key: &LanguageServerKey) -> Option<String> {
        let sessions = self.sessions.lock().ok()?;
        let session = sessions.get(key)?;
        Some(session.exit_reason())
    }
}

impl Drop for LanguageServerBroker {
    fn drop(&mut self) {
        if let Ok(sessions) = self.sessions.get_mut() {
            for (_, session) in sessions.drain() {
                session.stop();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveredServer {
    pub executable: PathBuf,
    pub candidate: CatalogCandidate,
}

struct LanguageServerSession {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<ServerEvent>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<String>>,
    next_request_id: u64,
}

impl LanguageServerSession {
    fn spawn(key: &LanguageServerKey, server: DiscoveredServer) -> Result<Self, BrokerError> {
        let mut command = background_command(&server.executable);
        command
            .args(&server.candidate.args)
            .current_dir(&key.project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = crate::discovery::runtime_path() {
            command.env("PATH", path);
        }
        let mut child = command.spawn().map_err(BrokerError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or(BrokerError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(BrokerError::MissingPipe("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(BrokerError::MissingPipe("stderr"))?;
        let (events_tx, events_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let stderr_sink = stderr_tail.clone();
        let stderr_thread = match thread::Builder::new()
            .name(format!("dragent-lsp-{}-stderr", key.server_id))
            .spawn(move || read_server_stderr(stderr, stderr_sink))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrokerError::Spawn(error));
            }
        };
        let reader_thread = match thread::Builder::new()
            .name(format!("dragent-lsp-{}", key.server_id))
            .spawn(move || read_server(stdout, events_tx))
        {
            Ok(reader_thread) => reader_thread,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_thread.join();
                return Err(BrokerError::Spawn(error));
            }
        };
        Ok(Self {
            child,
            stdin,
            events: events_rx,
            reader_thread: Some(reader_thread),
            stderr_thread: Some(stderr_thread),
            stderr_tail,
            next_request_id: 1,
        })
    }

    fn send(&mut self, message: Value) -> Result<(), BrokerError> {
        framing::write_message(&mut self.stdin, &message).map_err(BrokerError::Write)
    }

    fn try_recv(&self) -> Option<ServerEvent> {
        match self.events.try_recv() {
            Ok(ServerEvent::TransportClosed) => {
                Some(ServerEvent::TransportError(self.exit_reason()))
            }
            Ok(event) => Some(event),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn exit_reason(&self) -> String {
        let stderr = self
            .stderr_tail
            .lock()
            .map(|tail| tail.trim().to_string())
            .unwrap_or_default();
        if stderr.is_empty() {
            "language server exited".to_string()
        } else {
            format!("language server exited: {stderr}")
        }
    }

    fn stop(mut self) {
        // Close stdin first. Some language-server launchers wait for EOF (and
        // may have spawned a child process) before they release stdout/stderr.
        // Killing only the launcher can otherwise leave those pipe readers
        // blocked forever during application shutdown.
        drop(self.stdin);
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Closing the receiver also releases a reader blocked by a full bounded
        // queue, so shutdown cannot strand a background thread indefinitely.
        drop(self.events);
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        if let Some(stderr_thread) = self.stderr_thread.take() {
            let _ = stderr_thread.join();
        }
    }
}

fn read_server_stderr(mut stderr: std::process::ChildStderr, tail: Arc<Mutex<String>>) {
    let mut buffer = [0_u8; 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let chunk = String::from_utf8_lossy(&buffer[..count]);
                if let Ok(mut output) = tail.lock() {
                    output.push_str(&chunk);
                    if output.len() > STDERR_TAIL_BYTES {
                        let mut start = output.len() - STDERR_TAIL_BYTES;
                        while !output.is_char_boundary(start) {
                            start += 1;
                        }
                        output.drain(..start);
                    }
                }
            }
            Err(_) => break,
        }
    }
}

fn read_server(stdout: std::process::ChildStdout, events: SyncSender<ServerEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match framing::read_message(&mut reader) {
            Ok(Some(message)) => {
                if events.send(ServerEvent::Message(message)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = events.try_send(ServerEvent::TransportClosed);
                break;
            }
            Err(error) => {
                let _ = events.try_send(ServerEvent::TransportError(error.to_string()));
                break;
            }
        }
    }
}

pub fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_is_bounded_without_starting_processes() {
        let broker = LanguageServerBroker::new(BrokerConfig {
            max_servers: 2,
            ..BrokerConfig::default()
        });
        assert_eq!(broker.running_count(), 0);
    }

    #[test]
    fn creates_file_uri_for_absolute_path() {
        let root = std::env::current_dir().unwrap();
        assert!(file_uri(&root).unwrap().starts_with("file://"));
    }

    #[cfg(windows)]
    #[test]
    fn background_batch_commands_use_the_hidden_command_interpreter() {
        let command = background_command(r"C:\tools\language-server.cmd");
        assert!(Path::new(command.get_program())
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd.exe")));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            ["/d", "/s", "/c", "call", r"C:\tools\language-server.cmd"]
        );
    }
}
