//! Background document/LSP coordinator.
//!
//! UI code submits bounded, versioned document snapshots. This worker owns all
//! process operations and JSON-RPC state so server startup, failure, and idle
//! cleanup can never block rendering.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use dragent_editor::{DocumentDiagnostics, Language, LanguageRequest};
use serde_json::Value;

use crate::client_worker::Worker;
use crate::{LanguageServerCatalog, LanguageServerKey};

const COMMAND_CAPACITY: usize = 128;

const EVENT_CAPACITY: usize = 256;

pub(crate) const WORKER_TICK: Duration = Duration::from_millis(25);

pub(crate) const MAX_RESTART_ATTEMPTS: u32 = 5;

pub(crate) const MAX_PENDING_REQUESTS: usize = 256;

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// If `initialize` has not completed within this window the session is
/// considered stuck (lost response, hung process) and is failed + retried.
/// Without this a session could sit in `Starting` forever — the expired
/// request removed the pending entry but nothing transitioned the session.
pub(crate) const STARTING_TIMEOUT: Duration = Duration::from_secs(10);

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct SyncedDocument {
    pub project_root: PathBuf,
    pub relative_path: PathBuf,
    pub text: String,
}

impl SyncedDocument {
    #[cfg(test)]
    pub(crate) fn keys(&self, catalog: &LanguageServerCatalog) -> Vec<LanguageServerKey> {
        let language = Language::from_path(&self.relative_path);
        catalog
            .applicable_servers(language, &self.project_root, &self.text)
            .into_iter()
            .map(|server| LanguageServerKey::new(self.project_root.clone(), server.id.clone()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum LanguageServiceEvent {
    InstallRequired(LanguageServerKey),
    Downloading(LanguageServerKey),
    Installed(LanguageServerKey),
    Starting(LanguageServerKey),
    Ready {
        key: LanguageServerKey,
        capabilities: Value,
    },
    Stopped(LanguageServerKey),
    /// Stopped because none of its files was on screen for a while; it starts
    /// again by itself when one is shown or asked about.
    Asleep(LanguageServerKey),
    /// The server's process, for measuring what it holds.
    Process {
        key: LanguageServerKey,
        pid: u32,
    },
    RestartScheduled {
        key: LanguageServerKey,
        attempt: u32,
        delay: Duration,
        reason: String,
    },
    Failed {
        key: LanguageServerKey,
        reason: String,
    },
    ServerMessage {
        key: LanguageServerKey,
        message: Value,
    },
    Diagnostics(DocumentDiagnostics),
    Response {
        token: u64,
        key: LanguageServerKey,
        relative_path: PathBuf,
        result: Result<Value, String>,
    },
}

#[derive(Clone, Debug)]
pub struct LanguageServiceClientConfig {
    pub max_servers: usize,
    pub idle_timeout: Duration,
    pub executable_overrides: HashMap<String, PathBuf>,
    pub catalog: Arc<LanguageServerCatalog>,
    /// The caller sets this only for a trusted workspace.
    pub allow_managed_downloads: bool,
    /// Detects a server that never answers `initialize`; it never controls the
    /// successful startup path.
    pub initialize_timeout: Duration,
}

impl Default for LanguageServiceClientConfig {
    fn default() -> Self {
        Self {
            max_servers: 8,
            idle_timeout: Duration::from_secs(120),
            executable_overrides: HashMap::new(),
            catalog: Arc::new(LanguageServerCatalog::load()),
            allow_managed_downloads: false,
            initialize_timeout: INITIALIZE_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Command {
    Open(SyncedDocument),
    Change(SyncedDocument),
    Save(SyncedDocument),
    Close {
        project_root: PathBuf,
        relative_path: PathBuf,
    },
    RestartServer(String),
    RestartAll,
    StartServer {
        document: SyncedDocument,
        server_id: String,
    },
    StopServer(String),
    /// The file on screen (`None`: no file is).
    Visible(Option<(PathBuf, PathBuf)>),
    InstallServer {
        project_root: PathBuf,
        server_id: String,
    },
    Request {
        token: u64,
        project_root: PathBuf,
        relative_path: PathBuf,
        request: LanguageRequest,
    },
    WillRename {
        token: u64,
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    },
    DidRename {
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientSendError {
    #[error("language-service worker is unavailable")]
    Disconnected,
    #[error("language-service command queue is full")]
    Busy,
}

pub struct LanguageServiceClient {
    commands: Option<SyncSender<Command>>,
    events: Mutex<Receiver<LanguageServiceEvent>>,
    event_wakeups: async_channel::Receiver<()>,
    worker: Option<JoinHandle<()>>,
    next_request_token: AtomicU64,
}

impl LanguageServiceClient {
    pub fn start(config: LanguageServiceClientConfig) -> std::io::Result<Self> {
        let (commands_tx, commands_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (events_tx, events_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let (event_wakeups_tx, event_wakeups_rx) = async_channel::unbounded();
        let worker = thread::Builder::new()
            .name("dragent-language-services".to_string())
            .spawn(move || Worker::new(config, events_tx, event_wakeups_tx).run(commands_rx))?;
        Ok(Self {
            commands: Some(commands_tx),
            events: Mutex::new(events_rx),
            event_wakeups: event_wakeups_rx,
            worker: Some(worker),
            next_request_token: AtomicU64::new(1),
        })
    }

    pub fn open(&self, document: SyncedDocument) -> Result<(), ClientSendError> {
        self.send(Command::Open(document))
    }

    pub fn change(&self, document: SyncedDocument) -> Result<(), ClientSendError> {
        self.send(Command::Change(document))
    }

    pub fn save(&self, document: SyncedDocument) -> Result<(), ClientSendError> {
        self.send(Command::Save(document))
    }

    pub fn close(
        &self,
        project_root: PathBuf,
        relative_path: PathBuf,
    ) -> Result<(), ClientSendError> {
        self.send(Command::Close {
            project_root,
            relative_path,
        })
    }

    pub fn try_recv(&self) -> Option<LanguageServiceEvent> {
        self.events.lock().ok()?.try_recv().ok()
    }

    /// A wakeup is sent for every queued lifecycle/diagnostic event. WarpUI
    /// consumes this as a stream, so server state reaches the UI without a
    /// timer or an unrelated editor event.
    pub fn event_wakeups(&self) -> async_channel::Receiver<()> {
        self.event_wakeups.clone()
    }

    pub fn restart_server(&self, server_id: impl Into<String>) -> Result<(), ClientSendError> {
        self.send(Command::RestartServer(server_id.into()))
    }

    pub fn restart_all(&self) -> Result<(), ClientSendError> {
        self.send(Command::RestartAll)
    }

    /// Explicitly starts a catalogued server for one document, even when its
    /// automatic activation rule did not select it. This powers the editor's
    /// language-server picker without weakening automatic activation.
    pub fn start_server(
        &self,
        document: SyncedDocument,
        server_id: impl Into<String>,
    ) -> Result<(), ClientSendError> {
        self.send(Command::StartServer {
            document,
            server_id: server_id.into(),
        })
    }

    /// Tell the worker which file is on screen: servers none of whose files
    /// has been shown for the idle timeout are put to sleep, and wake when one
    /// is shown again.
    pub fn set_visible(
        &self,
        document: Option<(PathBuf, PathBuf)>,
    ) -> Result<(), ClientSendError> {
        self.send(Command::Visible(document))
    }

    pub fn stop_server(&self, server_id: impl Into<String>) -> Result<(), ClientSendError> {
        self.send(Command::StopServer(server_id.into()))
    }

    pub fn install_server(
        &self,
        project_root: PathBuf,
        server_id: impl Into<String>,
    ) -> Result<(), ClientSendError> {
        self.send(Command::InstallServer {
            project_root,
            server_id: server_id.into(),
        })
    }

    pub fn request(
        &self,
        project_root: PathBuf,
        relative_path: PathBuf,
        request: LanguageRequest,
    ) -> Result<u64, ClientSendError> {
        let token = self.next_request_token.fetch_add(1, Ordering::Relaxed);
        self.send(Command::Request {
            token,
            project_root,
            relative_path,
            request,
        })?;
        Ok(token)
    }

    /// Ask a capable language server for the workspace edits that must be
    /// applied before a file or folder is renamed (for example, import paths).
    pub fn will_rename(
        &self,
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    ) -> Result<u64, ClientSendError> {
        let token = self.next_request_token.fetch_add(1, Ordering::Relaxed);
        self.send(Command::WillRename {
            token,
            project_root,
            source,
            destination,
        })?;
        Ok(token)
    }

    /// Notify all capable servers after the filesystem rename succeeds.
    pub fn did_rename(
        &self,
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    ) -> Result<(), ClientSendError> {
        self.send(Command::DidRename {
            project_root,
            source,
            destination,
        })
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), ClientSendError> {
        let sender = self
            .commands
            .as_ref()
            .ok_or(ClientSendError::Disconnected)?;
        sender.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => ClientSendError::Busy,
            TrySendError::Disconnected(_) => ClientSendError::Disconnected,
        })
    }

    #[cfg(test)]
    pub(crate) fn shutdown(mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for LanguageServiceClient {
    fn drop(&mut self) {
        // Disconnecting the command channel is the shutdown signal. The worker
        // then drops its broker, which closes every child and joins each reader.
        // Never join here: this handle is owned by WarpUI and a language-server
        // process that is slow to exit must not block the UI during restart.
        self.commands.take();
        self.worker.take();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DocumentState {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) opened: bool,
}
