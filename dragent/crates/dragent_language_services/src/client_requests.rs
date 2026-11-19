//! Background document/LSP coordinator.
//!
//! UI code submits bounded, versioned document snapshots. This worker owns all
//! process operations and JSON-RPC state so server startup, failure, and idle
//! cleanup can never block rendering.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use dragent_editor::{Language, LanguageRequest, LanguageServerRole};
use serde_json::json;

use crate::client_api::DocumentState;
use crate::client_api::MAX_PENDING_REQUESTS;
use crate::client_worker::incremental_change;
use crate::client_worker::request_payload;
use crate::client_worker::supports_file_operation;
use crate::client_worker::DocumentSyncKind;
use crate::client_worker::PendingRequest;
use crate::client_worker::QueuedRequest;
use crate::client_worker::SessionState;
use crate::client_worker::SessionStatus;
use crate::client_worker::Worker;
use crate::protocol;
use crate::LanguageServiceEvent;
use crate::SyncedDocument;
use crate::{BrokerError, LanguageServerKey};

impl Worker {
    pub(crate) fn request(
        &mut self,
        token: u64,
        project_root: PathBuf,
        relative_path: PathBuf,
        request: LanguageRequest,
    ) {
        if self.pending_requests.len() >= MAX_PENDING_REQUESTS {
            self.emit(LanguageServiceEvent::Response {
                token,
                key: LanguageServerKey::new(project_root, "busy"),
                relative_path,
                result: Err("too many pending language-server requests".to_string()),
            });
            return;
        }
        let mut keys = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.project_root == project_root
                    && matches!(session.status, SessionStatus::Ready)
                    && session.documents.contains_key(&relative_path)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.sort_by_key(|key| {
            self.catalog
                .server(&key.server_id)
                .map_or(2, |server| match server.role {
                    LanguageServerRole::Primary => 0,
                    LanguageServerRole::Supplementary => 1,
                })
        });
        let Some(key) = keys.into_iter().next() else {
            let waiting_for_server = self.sessions.iter().any(|(key, session)| {
                key.project_root == project_root
                    && session.documents.contains_key(&relative_path)
                    && !matches!(session.status, SessionStatus::Failed)
            });
            if waiting_for_server {
                self.queued_requests.push(QueuedRequest {
                    token,
                    project_root,
                    relative_path,
                    request,
                });
                return;
            }
            self.emit(LanguageServiceEvent::Response {
                token,
                key: LanguageServerKey::new(project_root, "unavailable"),
                relative_path,
                result: Err("no ready language server for this document".to_string()),
            });
            return;
        };
        let Some((method, params)) = request_payload(&key, &relative_path, &request) else {
            self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path,
                result: Err("could not create a file URI for the document".to_string()),
            });
            return;
        };
        match self.broker.send_request(&key, method, params) {
            Ok(id) => {
                self.pending_requests.insert(
                    (key, id),
                    PendingRequest {
                        token,
                        relative_path,
                        started_at: Instant::now(),
                    },
                );
            }
            Err(error) => self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path,
                result: Err(error.to_string()),
            }),
        }
    }

    pub(crate) fn will_rename(
        &mut self,
        token: u64,
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    ) {
        if self.pending_requests.len() >= MAX_PENDING_REQUESTS {
            self.emit(LanguageServiceEvent::Response {
                token,
                key: LanguageServerKey::new(project_root, "busy"),
                relative_path: source,
                result: Err("too many pending language-server requests".to_string()),
            });
            return;
        }
        let mut keys = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.project_root == project_root
                    && matches!(session.status, SessionStatus::Ready)
                    && supports_file_operation(&session.capabilities, "willRename")
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.sort_by_key(|key| {
            self.catalog
                .server(&key.server_id)
                .map_or(2, |server| match server.role {
                    LanguageServerRole::Primary => 0,
                    LanguageServerRole::Supplementary => 1,
                })
        });
        let Some(key) = keys.into_iter().next() else {
            self.emit(LanguageServiceEvent::Response {
                token,
                key: LanguageServerKey::new(project_root, "unsupported"),
                relative_path: source,
                result: Err(
                    "no language server supports import updates for file renames".to_string(),
                ),
            });
            return;
        };
        let Some(old_uri) = crate::file_uri(&key.project_root.join(&source)) else {
            self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path: source,
                result: Err("could not create the old file URI".to_string()),
            });
            return;
        };
        let Some(new_uri) = crate::file_uri(&key.project_root.join(&destination)) else {
            self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path: source,
                result: Err("could not create the new file URI".to_string()),
            });
            return;
        };
        let params = json!({
            "files": [{"oldUri": old_uri, "newUri": new_uri}]
        });
        match self
            .broker
            .send_request(&key, "workspace/willRenameFiles", params)
        {
            Ok(id) => {
                self.pending_requests.insert(
                    (key, id),
                    PendingRequest {
                        token,
                        relative_path: source,
                        started_at: Instant::now(),
                    },
                );
            }
            Err(error) => self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path: source,
                result: Err(error.to_string()),
            }),
        }
    }

    pub(crate) fn did_rename(
        &mut self,
        project_root: PathBuf,
        source: PathBuf,
        destination: PathBuf,
    ) {
        for (key, session) in &mut self.sessions {
            if key.project_root != project_root {
                continue;
            }
            let remapped = session
                .documents
                .keys()
                .filter_map(|path| {
                    path.strip_prefix(&source)
                        .ok()
                        .map(|suffix| (path.clone(), destination.join(suffix)))
                })
                .collect::<Vec<_>>();
            for (old_path, new_path) in remapped {
                if let Some(document) = session.documents.remove(&old_path) {
                    session.documents.insert(new_path, document);
                }
            }
        }
        let keys = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.project_root == project_root
                    && matches!(session.status, SessionStatus::Ready)
                    && supports_file_operation(&session.capabilities, "didRename")
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            let Some(old_uri) = crate::file_uri(&key.project_root.join(&source)) else {
                continue;
            };
            let Some(new_uri) = crate::file_uri(&key.project_root.join(&destination)) else {
                continue;
            };
            if let Err(error) = self.broker.send_notification(
                &key,
                "workspace/didRenameFiles",
                json!({"files": [{"oldUri": old_uri, "newUri": new_uri}]}),
            ) {
                log::warn!(
                    "[lsp] didRename failed server={} source={} destination={} error={error}",
                    key.server_id,
                    source.display(),
                    destination.display()
                );
            }
        }
    }

    pub(crate) fn dispatch_queued_requests(&mut self, ready_key: &LanguageServerKey) {
        let mut remaining = Vec::with_capacity(self.queued_requests.len());
        let mut ready = Vec::new();
        for request in self.queued_requests.drain(..) {
            let document_ready = request.project_root == ready_key.project_root
                && self
                    .sessions
                    .get(ready_key)
                    .is_some_and(|session| session.documents.contains_key(&request.relative_path));
            if document_ready {
                ready.push(request);
            } else {
                remaining.push(request);
            }
        }
        self.queued_requests = remaining;
        for request in ready {
            self.request(
                request.token,
                request.project_root,
                request.relative_path,
                request.request,
            );
        }
    }

    pub(crate) fn install_server(&mut self, project_root: PathBuf, server_id: String) {
        let key = LanguageServerKey::new(project_root, server_id);
        self.emit(LanguageServiceEvent::Downloading(key.clone()));
        match self
            .broker
            .install_managed(&key.server_id, &key.project_root)
        {
            Ok(_) => {
                self.emit(LanguageServiceEvent::Installed(key.clone()));
                if let Some(session) = self.sessions.get_mut(&key) {
                    session.restart_attempts = 0;
                    session.status = SessionStatus::Backoff {
                        retry_at: Instant::now(),
                    };
                    self.start_if_due(&key);
                }
            }
            Err(error) => self.emit(LanguageServiceEvent::Failed {
                key,
                reason: error.to_string(),
            }),
        }
    }

    pub(crate) fn start_server(&mut self, document: SyncedDocument, server_id: String) {
        let language = Language::from_path(&document.relative_path);
        let Some(server) = self.catalog.server(&server_id) else {
            self.emit(LanguageServiceEvent::Failed {
                key: LanguageServerKey::new(document.project_root, server_id),
                reason: "language server is not in the runtime catalog".to_string(),
            });
            return;
        };
        if !server.supports_language(language) {
            self.emit(LanguageServiceEvent::Failed {
                key: LanguageServerKey::new(document.project_root, server_id),
                reason: format!("server does not support {language:?}"),
            });
            return;
        }
        let role = server.role;
        if role == LanguageServerRole::Primary {
            self.primary_overrides
                .insert((document.project_root.clone(), language), server_id.clone());
            let replaced = self
                .sessions
                .keys()
                .filter(|key| {
                    key.project_root == document.project_root
                        && key.server_id != server_id
                        && self
                            .catalog
                            .server(&key.server_id)
                            .is_some_and(|candidate| {
                                candidate.role == LanguageServerRole::Primary
                                    && candidate.supports_language(language)
                            })
                })
                .cloned()
                .collect::<Vec<_>>();
            for key in replaced {
                self.broker.stop(&key);
                self.sessions.remove(&key);
                self.emit(LanguageServiceEvent::Stopped(key));
            }
        }
        let key = LanguageServerKey::new(document.project_root.clone(), server_id);
        self.broker.stop(&key);
        self.sessions.remove(&key);
        self.open_for_server(key, &document);
    }

    pub(crate) fn restart_server(&mut self, server_id: &str) {
        let keys = self
            .sessions
            .keys()
            .filter(|key| key.server_id == server_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.broker.stop(&key);
            if let Some(session) = self.sessions.get_mut(&key) {
                session.restart_attempts = 0;
                session
                    .documents
                    .values_mut()
                    .for_each(|document| document.opened = false);
                session.status = SessionStatus::Backoff {
                    retry_at: Instant::now(),
                };
            }
            self.start_if_due(&key);
        }
    }

    pub(crate) fn restart_all(&mut self) {
        let server_ids = self
            .sessions
            .keys()
            .map(|key| key.server_id.clone())
            .collect::<std::collections::HashSet<_>>();
        for server_id in server_ids {
            self.restart_server(&server_id);
        }
    }

    pub(crate) fn stop_server(&mut self, server_id: &str) {
        let keys = self
            .sessions
            .keys()
            .filter(|key| key.server_id == server_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.broker.stop(&key);
            if let Some(session) = self.sessions.get_mut(&key) {
                session.status = SessionStatus::Failed;
                session
                    .documents
                    .values_mut()
                    .for_each(|document| document.opened = false);
            }
            self.emit(LanguageServiceEvent::Stopped(key));
        }
    }

    pub(crate) fn open(&mut self, document: SyncedDocument) {
        let keys = self.document_keys(&document);
        log::info!(
            "[lsp-lifecycle] document detected language={:?} root={} path={} servers={:?}",
            Language::from_path(&document.relative_path),
            document.project_root.display(),
            document.relative_path.display(),
            keys.iter()
                .map(|key| key.server_id.as_str())
                .collect::<Vec<_>>()
        );
        for key in keys {
            self.open_for_server(key, &document);
        }
    }

    pub(crate) fn document_keys(&self, document: &SyncedDocument) -> Vec<LanguageServerKey> {
        let language = Language::from_path(&document.relative_path);
        let mut servers =
            self.catalog
                .applicable_servers(language, &document.project_root, &document.text);
        // Auto-enable: any catalogued server for this language whose binary is
        // already downloaded/installed starts automatically. Activation rules
        // (package.json markers, content needles) only gate automatic
        // DOWNLOADS — they never keep an available server idle.
        let included: std::collections::HashSet<&str> =
            servers.iter().map(|server| server.id.as_str()).collect();
        for server in self.catalog.servers_for_language(language) {
            if included.contains(server.id.as_str()) {
                continue;
            }
            match self
                .broker
                .discover_for_project(&server.id, &document.project_root)
            {
                Ok(discovered) => {
                    log::info!(
                        "[lsp-auto] enabling downloaded server={} binary={}",
                        server.id,
                        discovered.executable.display()
                    );
                    servers.push(server);
                }
                Err(_) => {
                    log::info!(
                        "[lsp-auto] skipping server={} (binary not downloaded; enable via tray to install)",
                        server.id
                    );
                }
            }
        }
        log::info!(
            "[lsp-auto] document {} ({language:?}) -> servers: {}",
            document.relative_path.display(),
            servers
                .iter()
                .map(|server| server.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(primary) = self
            .primary_overrides
            .get(&(document.project_root.clone(), language))
        {
            servers.retain(|server| server.role != LanguageServerRole::Primary);
            if let Some(server) = self.catalog.server(primary) {
                servers.push(server);
            }
        }
        servers
            .into_iter()
            .map(|server| LanguageServerKey::new(document.project_root.clone(), server.id.clone()))
            .collect()
    }

    pub(crate) fn open_for_server(&mut self, key: LanguageServerKey, document: &SyncedDocument) {
        if self
            .sessions
            .get(&key)
            .is_some_and(|session| session.documents.contains_key(&document.relative_path))
        {
            // `ensure` is idempotent. Re-selecting an already-open editor tab
            // must not reset its version or send a second didOpen.
            self.change_for_server(key.clone(), document);
            self.start_if_due(&key);
            return;
        }
        let state = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| SessionState {
                documents: HashMap::new(),
                status: SessionStatus::Backoff {
                    retry_at: Instant::now(),
                },
                restart_attempts: 0,
                last_used: Instant::now(),
                sync_kind: DocumentSyncKind::Full,
                capabilities: json!({}),
            });
        state.last_used = Instant::now();
        state.documents.insert(
            document.relative_path.clone(),
            DocumentState {
                text: document.text.clone(),
                version: 1,
                opened: false,
            },
        );
        if matches!(state.status, SessionStatus::Ready) {
            self.send_open(&key, &document.relative_path);
        } else if matches!(state.status, SessionStatus::Failed) {
            state.restart_attempts = 0;
            state.status = SessionStatus::Backoff {
                retry_at: Instant::now(),
            };
        }
        self.start_if_due(&key);
    }

    pub(crate) fn change(&mut self, document: SyncedDocument) {
        let mut keys = self.document_keys(&document);
        for key in self.sessions.keys() {
            if key.project_root == document.project_root
                && self
                    .sessions
                    .get(key)
                    .is_some_and(|session| session.documents.contains_key(&document.relative_path))
                && !keys.contains(key)
            {
                keys.push(key.clone());
            }
        }
        for key in keys {
            self.change_for_server(key, &document);
        }
    }

    pub(crate) fn change_for_server(&mut self, key: LanguageServerKey, document: &SyncedDocument) {
        let Some(session) = self.sessions.get_mut(&key) else {
            self.open_for_server(key, document);
            return;
        };
        session.last_used = Instant::now();
        let state = session
            .documents
            .entry(document.relative_path.clone())
            .or_insert(DocumentState {
                text: String::new(),
                version: 0,
                opened: false,
            });
        if state.text == document.text {
            return;
        }
        let previous_text = std::mem::replace(&mut state.text, document.text.clone());
        state.version = state.version.saturating_add(1).max(1);
        if matches!(session.status, SessionStatus::Ready) && state.opened {
            let params = match session.sync_kind {
                DocumentSyncKind::None => None,
                DocumentSyncKind::Full => {
                    protocol::did_change(&key, &document.relative_path, &state.text, state.version)
                }
                DocumentSyncKind::Incremental => {
                    let (range, replacement) = incremental_change(&previous_text, &state.text);
                    protocol::did_change_incremental(
                        &key,
                        &document.relative_path,
                        range,
                        replacement,
                        state.version,
                    )
                }
            };
            if let Some(params) = params {
                if let Err(error) =
                    self.broker
                        .send_notification(&key, "textDocument/didChange", params)
                {
                    self.fail_session(&key, error.to_string());
                }
            }
        }
    }

    pub(crate) fn save(&mut self, document: SyncedDocument) {
        self.change(document.clone());
        let keys = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.project_root == document.project_root
                    && matches!(session.status, SessionStatus::Ready)
                    && session
                        .documents
                        .get(&document.relative_path)
                        .is_some_and(|doc| doc.opened)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(params) = protocol::did_save(&key, &document.relative_path, &document.text)
            {
                if let Err(error) =
                    self.broker
                        .send_notification(&key, "textDocument/didSave", params)
                {
                    self.fail_session(&key, error.to_string());
                }
            }
        }
    }

    pub(crate) fn close(&mut self, project_root: PathBuf, relative_path: PathBuf) {
        let keys = self
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.project_root == project_root && session.documents.contains_key(&relative_path)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.close_for_server(&key, &relative_path);
        }
    }

    pub(crate) fn close_for_server(&mut self, key: &LanguageServerKey, relative_path: &PathBuf) {
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        let was_open = session
            .documents
            .get(relative_path)
            .is_some_and(|doc| doc.opened);
        let ready = matches!(session.status, SessionStatus::Ready);
        let mut close_error = None;
        if ready && was_open {
            if let Some(params) = protocol::did_close(key, relative_path) {
                if let Err(error) =
                    self.broker
                        .send_notification(key, "textDocument/didClose", params)
                {
                    close_error = Some(error.to_string());
                }
            }
        }
        if let Some(session) = self.sessions.get_mut(key) {
            session.documents.remove(relative_path);
            session.last_used = Instant::now();
        }
        if let Some(error) = close_error {
            self.fail_session(key, error);
        }
    }

    pub(crate) fn start_if_due(&mut self, key: &LanguageServerKey) {
        let due = self
            .sessions
            .get(key)
            .is_some_and(|session| match session.status {
                SessionStatus::Backoff { retry_at } => retry_at <= Instant::now(),
                _ => false,
            });
        if !due {
            return;
        }
        let discovered = self
            .broker
            .discover_for_project(&key.server_id, &key.project_root);
        let installable = self
            .catalog
            .server(&key.server_id)
            .is_some_and(|server| server.install.is_some());
        if matches!(discovered, Err(BrokerError::ServerUnavailable(_))) && installable {
            if !self.allow_managed_downloads {
                self.require_install(key);
                return;
            }
            self.emit(LanguageServiceEvent::Downloading(key.clone()));
            if let Err(error) = self
                .broker
                .install_managed(&key.server_id, &key.project_root)
            {
                self.fail_permanently(key, error.to_string());
                return;
            }
        }
        match self.broker.ensure_started(key.clone()).and_then(|_| {
            self.broker
                .send_request(key, "initialize", protocol::initialize_params(key))
        }) {
            Ok(initialize_id) => {
                let started_at = Instant::now();
                if let Some(session) = self.sessions.get_mut(key) {
                    session.status = SessionStatus::Starting {
                        initialize_id,
                        started_at,
                    };
                }
                log::info!(
                    "[lsp-lifecycle] initialize sent server={} root={} request_id={}",
                    key.server_id,
                    key.project_root.display(),
                    initialize_id
                );
                self.emit(LanguageServiceEvent::Starting(key.clone()));
            }
            Err(
                error @ (BrokerError::UnsupportedServer(_)
                | BrokerError::ServerUnavailable(_)
                | BrokerError::InvalidProjectRoot(_)),
            ) => self.fail_permanently(key, error.to_string()),
            Err(error) => self.fail_session(key, error.to_string()),
        }
    }

    pub(crate) fn send_open(&mut self, key: &LanguageServerKey, path: &PathBuf) {
        let params = self.sessions.get(key).and_then(|session| {
            let document = session.documents.get(path)?;
            protocol::did_open(key, path, &document.text, document.version)
        });
        let Some(params) = params else { return };
        match self
            .broker
            .send_notification(key, "textDocument/didOpen", params)
        {
            Ok(()) => {
                log::info!(
                    "[lsp-lifecycle] didOpen sent server={} root={} path={}",
                    key.server_id,
                    key.project_root.display(),
                    path.display()
                );
                if let Some(document) = self
                    .sessions
                    .get_mut(key)
                    .and_then(|s| s.documents.get_mut(path))
                {
                    document.opened = true;
                }
            }
            Err(error) => self.fail_session(key, error.to_string()),
        }
    }
}
