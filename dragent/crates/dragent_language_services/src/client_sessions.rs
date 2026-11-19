//! Background document/LSP coordinator.
//!
//! UI code submits bounded, versioned document snapshots. This worker owns all
//! process operations and JSON-RPC state so server startup, failure, and idle
//! cleanup can never block rendering.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::client_api::MAX_RESTART_ATTEMPTS;
use crate::client_api::REQUEST_TIMEOUT;
use crate::client_api::STARTING_TIMEOUT;
use crate::client_worker::document_sync_kind;
use crate::client_worker::is_deterministic_startup_failure;
use crate::client_worker::parse_diagnostics;
use crate::client_worker::restart_delay;
use crate::client_worker::SessionStatus;
use crate::client_worker::Worker;
use crate::LanguageServiceEvent;
use crate::{BrokerError, LanguageServerKey, ServerEvent};

impl Worker {
    pub(crate) fn poll_servers(&mut self) {
        let keys = self.sessions.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            for _ in 0..64 {
                let Some(event) = self.broker.try_recv(&key) else {
                    break;
                };
                match event {
                    ServerEvent::Message(message) => self.handle_server_message(&key, message),
                    ServerEvent::TransportClosed => {
                        self.fail_session(&key, "language server exited".to_string());
                        break;
                    }
                    ServerEvent::TransportError(reason) => {
                        self.fail_session(&key, reason);
                        break;
                    }
                }
            }
        }
    }

    pub(crate) fn handle_server_message(&mut self, key: &LanguageServerKey, message: Value) {
        let initialize_id = self
            .sessions
            .get(key)
            .and_then(|session| match session.status {
                SessionStatus::Starting { initialize_id, .. } => Some(initialize_id),
                _ => None,
            });
        if initialize_id.is_some_and(|id| message.get("id").and_then(Value::as_u64) == Some(id)) {
            if let Some(error) = message.get("error") {
                self.fail_session(key, format!("initialize failed: {error}"));
                return;
            }
            if let Err(error) = self.broker.send_notification(key, "initialized", json!({})) {
                self.fail_session(key, error.to_string());
                return;
            }
            let capabilities = message
                .pointer("/result/capabilities")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let paths = if let Some(session) = self.sessions.get_mut(key) {
                let elapsed = match session.status {
                    SessionStatus::Starting { started_at, .. } => started_at.elapsed(),
                    _ => Duration::ZERO,
                };
                log::info!(
                    "[lsp-lifecycle] initialize complete server={} root={} elapsed_ms={}",
                    key.server_id,
                    key.project_root.display(),
                    elapsed.as_millis()
                );
                session.status = SessionStatus::Ready;
                session.sync_kind = document_sync_kind(&message);
                session.capabilities = capabilities.clone();
                session.restart_attempts = 0;
                session
                    .documents
                    .values_mut()
                    .for_each(|doc| doc.opened = false);
                session.documents.keys().cloned().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            self.emit(LanguageServiceEvent::Ready {
                key: key.clone(),
                capabilities,
            });
            for path in paths {
                if !self
                    .sessions
                    .get(key)
                    .is_some_and(|session| matches!(session.status, SessionStatus::Ready))
                {
                    break;
                }
                self.send_open(key, &path);
            }
            self.dispatch_queued_requests(key);
            return;
        }
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            if let Some(pending) = self.pending_requests.remove(&(key.clone(), id)) {
                let result = message
                    .get("error")
                    .map(|error| Err(error.to_string()))
                    .unwrap_or_else(|| Ok(message.get("result").cloned().unwrap_or(Value::Null)));
                self.emit(LanguageServiceEvent::Response {
                    token: pending.token,
                    key: key.clone(),
                    relative_path: pending.relative_path,
                    result,
                });
                return;
            }
        }
        if message.get("method").is_some() && message.get("id").is_some() {
            if let Err(error) = self.respond_to_server_request(key, &message) {
                self.fail_session(key, error.to_string());
            }
            return;
        }
        if let Some(diagnostics) = parse_diagnostics(key, &message) {
            let current_version = self
                .sessions
                .get(key)
                .and_then(|session| session.documents.get(&diagnostics.relative_path))
                .map(|document| document.version);
            if diagnostics.version.is_none() || diagnostics.version == current_version {
                log::info!(
                    "[lsp-lifecycle] diagnostics received server={} path={} count={}",
                    key.server_id,
                    diagnostics.relative_path.display(),
                    diagnostics.items.len()
                );
                self.emit(LanguageServiceEvent::Diagnostics(diagnostics));
            }
            return;
        }
        self.emit(LanguageServiceEvent::ServerMessage {
            key: key.clone(),
            message,
        });
    }

    pub(crate) fn respond_to_server_request(
        &self,
        key: &LanguageServerKey,
        message: &Value,
    ) -> Result<(), BrokerError> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(());
        };
        match message.get("method").and_then(Value::as_str) {
            Some("workspace/configuration") => {
                let count = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                self.broker
                    .send_response(key, id, Value::Array(vec![Value::Null; count]))
            }
            Some("client/registerCapability" | "client/unregisterCapability")
            | Some("window/workDoneProgress/create") => {
                self.broker.send_response(key, id, Value::Null)
            }
            Some("workspace/workspaceFolders") => self.broker.send_response(
                key,
                id,
                json!([{
                    "uri": crate::file_uri(&key.project_root),
                    "name": key.project_root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")
                }]),
            ),
            Some("workspace/applyEdit") => self.broker.send_response(
                key,
                id,
                json!({
                    "applied": false,
                    "failureReason": "Workspace edits require explicit editor approval"
                }),
            ),
            Some(method) => self.broker.send_error_response(
                key,
                id,
                -32601,
                &format!("Dragent does not support server request {method}"),
            ),
            None => Ok(()),
        }
    }

    pub(crate) fn fail_session(&mut self, key: &LanguageServerKey, reason: String) {
        self.broker.stop(key);
        let Some(session) = self.sessions.get_mut(key) else {
            return;
        };
        session
            .documents
            .values_mut()
            .for_each(|doc| doc.opened = false);
        session.restart_attempts = session.restart_attempts.saturating_add(1);
        let deterministic_startup_failure =
            matches!(session.status, SessionStatus::Starting { .. })
                && is_deterministic_startup_failure(&reason);
        if deterministic_startup_failure
            || session.restart_attempts > MAX_RESTART_ATTEMPTS
            || session.documents.is_empty()
        {
            session.status = SessionStatus::Failed;
            self.emit(LanguageServiceEvent::Failed {
                key: key.clone(),
                reason,
            });
            return;
        }
        let delay = restart_delay(session.restart_attempts);
        let attempt = session.restart_attempts;
        session.status = SessionStatus::Backoff {
            retry_at: Instant::now() + delay,
        };
        self.emit(LanguageServiceEvent::RestartScheduled {
            key: key.clone(),
            attempt,
            delay,
            reason,
        });
    }

    pub(crate) fn maintain_sessions(&mut self) {
        let now = Instant::now();
        let initialize_timeouts = self
            .sessions
            .iter()
            .filter_map(|(key, session)| match session.status {
                SessionStatus::Starting { started_at, .. }
                    if now.saturating_duration_since(started_at) >= self.initialize_timeout =>
                {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for key in initialize_timeouts {
            self.fail_permanently(
                &key,
                format!(
                    "language server initialize timed out after {}ms",
                    self.initialize_timeout.as_millis()
                ),
            );
        }
        let expired = self
            .pending_requests
            .iter()
            .filter(|(_, pending)| {
                now.saturating_duration_since(pending.started_at) >= REQUEST_TIMEOUT
            })
            .map(|((key, _), pending)| (key.clone(), pending.token, pending.relative_path.clone()))
            .collect::<Vec<_>>();
        self.pending_requests.retain(|_, pending| {
            now.saturating_duration_since(pending.started_at) < REQUEST_TIMEOUT
        });
        for (key, token, relative_path) in expired {
            self.emit(LanguageServiceEvent::Response {
                token,
                key,
                relative_path,
                result: Err("language-server request timed out".to_string()),
            });
        }
        let exited = self
            .sessions
            .iter()
            .filter_map(|(key, session)| {
                matches!(
                    session.status,
                    SessionStatus::Starting { .. } | SessionStatus::Ready
                )
                .then(|| key.clone())
            })
            .filter(|key| !self.broker.is_running(key))
            .collect::<Vec<_>>();
        for key in exited {
            let reason = self
                .broker
                .exit_reason(&key)
                .unwrap_or_else(|| "language server exited".to_string());
            self.fail_session(&key, reason);
        }

        // A `Starting` session whose initialize never completed must not be
        // stuck forever: fail it into Backoff so the retry sweep restarts it.
        let stuck_starting = self
            .sessions
            .iter()
            .filter_map(|(key, session)| {
                matches!(
                    session.status,
                    SessionStatus::Starting { started_at, .. }
                        if now.saturating_duration_since(started_at) >= STARTING_TIMEOUT
                )
                .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in stuck_starting {
            self.fail_session(&key, "initialize timed out".to_string());
        }

        let retry = self
            .sessions
            .iter()
            .filter_map(|(key, session)| {
                matches!(session.status, SessionStatus::Backoff { retry_at } if retry_at <= now)
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in retry {
            self.start_if_due(&key);
        }

        let idle = self
            .sessions
            .iter()
            .filter_map(|(key, session)| {
                (session.documents.is_empty()
                    && now.saturating_duration_since(session.last_used) >= self.idle_timeout)
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in idle {
            self.broker.stop(&key);
            self.sessions.remove(&key);
            self.emit(LanguageServiceEvent::Stopped(key));
        }
    }

    pub(crate) fn emit(&self, event: LanguageServiceEvent) {
        // Lifecycle and diagnostics are state transitions, not telemetry. Do
        // not silently lose Ready/Failed/did-diagnostics when the UI is busy.
        // Backpressure is confined to this background worker.
        if self.events.send(event).is_ok() {
            let _ = self.event_wakeups.try_send(());
        }
    }

    pub(crate) fn fail_permanently(&mut self, key: &LanguageServerKey, reason: String) {
        self.broker.stop(key);
        if let Some(session) = self.sessions.get_mut(key) {
            session.status = SessionStatus::Failed;
            session
                .documents
                .values_mut()
                .for_each(|document| document.opened = false);
        }
        self.emit(LanguageServiceEvent::Failed {
            key: key.clone(),
            reason,
        });
    }

    pub(crate) fn require_install(&mut self, key: &LanguageServerKey) {
        self.broker.stop(key);
        if let Some(session) = self.sessions.get_mut(key) {
            session.status = SessionStatus::Failed;
            session
                .documents
                .values_mut()
                .for_each(|document| document.opened = false);
        }
        self.emit(LanguageServiceEvent::InstallRequired(key.clone()));
    }
}
