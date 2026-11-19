//! Background document/LSP coordinator.
//!
//! UI code submits bounded, versioned document snapshots. This worker owns all
//! process operations and JSON-RPC state so server startup, failure, and idle
//! cleanup can never block rendering.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dragent_editor::{
    Diagnostic, DiagnosticSeverity, DocumentDiagnostics, Language, LanguageRequest,
    LanguageRequestKind, TextPosition, TextRange,
};
use serde_json::{json, Value};

use crate::client_api::Command;
use crate::client_api::DocumentState;
use crate::client_api::WORKER_TICK;
use crate::LanguageServiceClientConfig;
use crate::LanguageServiceEvent;
use crate::{BrokerConfig, LanguageServerBroker, LanguageServerCatalog, LanguageServerKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentSyncKind {
    None,
    Full,
    Incremental,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionStatus {
    Starting {
        initialize_id: u64,
        started_at: Instant,
    },
    Ready,
    Backoff {
        retry_at: Instant,
    },
    Failed,
}

#[derive(Debug)]
pub(crate) struct SessionState {
    pub(crate) documents: HashMap<PathBuf, DocumentState>,
    pub(crate) status: SessionStatus,
    pub(crate) restart_attempts: u32,
    pub(crate) last_used: Instant,
    pub(crate) sync_kind: DocumentSyncKind,
    pub(crate) capabilities: Value,
}

#[derive(Debug)]
pub(crate) struct PendingRequest {
    pub(crate) token: u64,
    pub(crate) relative_path: PathBuf,
    pub(crate) started_at: Instant,
}

#[derive(Debug)]
pub(crate) struct QueuedRequest {
    pub(crate) token: u64,
    pub(crate) project_root: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) request: LanguageRequest,
}

pub(crate) struct Worker {
    pub(crate) catalog: Arc<LanguageServerCatalog>,
    pub(crate) broker: LanguageServerBroker,
    pub(crate) sessions: HashMap<LanguageServerKey, SessionState>,
    pub(crate) primary_overrides: HashMap<(PathBuf, Language), String>,
    pub(crate) events: SyncSender<LanguageServiceEvent>,
    pub(crate) event_wakeups: async_channel::Sender<()>,
    pub(crate) idle_timeout: Duration,
    pub(crate) initialize_timeout: Duration,
    pub(crate) allow_managed_downloads: bool,
    pub(crate) pending_requests: HashMap<(LanguageServerKey, u64), PendingRequest>,
    pub(crate) queued_requests: Vec<QueuedRequest>,
}
impl Worker {
    pub(crate) fn new(
        config: LanguageServiceClientConfig,
        events: SyncSender<LanguageServiceEvent>,
        event_wakeups: async_channel::Sender<()>,
    ) -> Self {
        let catalog = config.catalog.clone();
        Self {
            broker: LanguageServerBroker::new(BrokerConfig {
                max_servers: config.max_servers,
                executable_overrides: config.executable_overrides,
                catalog: catalog.clone(),
            }),
            catalog,
            sessions: HashMap::new(),
            primary_overrides: HashMap::new(),
            events,
            event_wakeups,
            idle_timeout: config.idle_timeout,
            initialize_timeout: config.initialize_timeout,
            allow_managed_downloads: config.allow_managed_downloads,
            pending_requests: HashMap::new(),
            queued_requests: Vec::new(),
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<Command>) {
        loop {
            match commands.recv_timeout(WORKER_TICK) {
                Ok(command) => {
                    self.handle_command(command);
                    for _ in 0..63 {
                        match commands.try_recv() {
                            Ok(command) => self.handle_command(command),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.poll_servers();
            self.maintain_sessions();
        }
    }

    pub(crate) fn handle_command(&mut self, command: Command) {
        match command {
            Command::Open(document) => self.open(document),
            Command::Change(document) => self.change(document),
            Command::Save(document) => self.save(document),
            Command::Close {
                project_root,
                relative_path,
            } => self.close(project_root, relative_path),
            Command::RestartServer(server_id) => self.restart_server(&server_id),
            Command::RestartAll => self.restart_all(),
            Command::StartServer {
                document,
                server_id,
            } => self.start_server(document, server_id),
            Command::StopServer(server_id) => self.stop_server(&server_id),
            Command::InstallServer {
                project_root,
                server_id,
            } => self.install_server(project_root, server_id),
            Command::Request {
                token,
                project_root,
                relative_path,
                request,
            } => self.request(token, project_root, relative_path, request),
            Command::WillRename {
                token,
                project_root,
                source,
                destination,
            } => self.will_rename(token, project_root, source, destination),
            Command::DidRename {
                project_root,
                source,
                destination,
            } => self.did_rename(project_root, source, destination),
        }
    }
}

pub(crate) fn request_payload(
    key: &LanguageServerKey,
    relative_path: &std::path::Path,
    request: &LanguageRequest,
) -> Option<(&'static str, Value)> {
    let uri = crate::file_uri(&key.project_root.join(relative_path))?;
    let position = json!({
        "line": request.position.line,
        "character": request.position.character,
    });
    let document = json!({ "uri": uri });
    let range_value = |range: TextRange| {
        json!({
            "start": { "line": range.start.line, "character": range.start.character },
            "end": { "line": range.end.line, "character": range.end.character },
        })
    };
    Some(match &request.kind {
        LanguageRequestKind::Completion => (
            "textDocument/completion",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::Hover => (
            "textDocument/hover",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::Definition => (
            "textDocument/definition",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::TypeDefinition => (
            "textDocument/typeDefinition",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::Implementation => (
            "textDocument/implementation",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::References {
            include_declaration,
        } => (
            "textDocument/references",
            json!({ "textDocument": document, "position": position, "context": { "includeDeclaration": include_declaration } }),
        ),
        LanguageRequestKind::SignatureHelp => (
            "textDocument/signatureHelp",
            json!({ "textDocument": document, "position": position }),
        ),
        LanguageRequestKind::DocumentSymbols => (
            "textDocument/documentSymbol",
            json!({ "textDocument": document }),
        ),
        LanguageRequestKind::CodeActions { range } => (
            "textDocument/codeAction",
            json!({ "textDocument": document, "range": range_value(*range), "context": { "diagnostics": [] } }),
        ),
        LanguageRequestKind::Formatting {
            tab_size,
            insert_spaces,
        } => (
            "textDocument/formatting",
            json!({ "textDocument": document, "options": { "tabSize": tab_size, "insertSpaces": insert_spaces } }),
        ),
        LanguageRequestKind::Rename { new_name } => (
            "textDocument/rename",
            json!({ "textDocument": document, "position": position, "newName": new_name }),
        ),
        LanguageRequestKind::InlayHints { range } => (
            "textDocument/inlayHint",
            json!({ "textDocument": document, "range": range_value(*range) }),
        ),
        LanguageRequestKind::SemanticTokens => (
            "textDocument/semanticTokens/full",
            json!({ "textDocument": document }),
        ),
    })
}

pub(crate) fn supports_file_operation(capabilities: &Value, operation: &str) -> bool {
    capabilities
        .pointer(&format!("/workspace/fileOperations/{operation}"))
        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
}

pub(crate) fn document_sync_kind(initialize_response: &Value) -> DocumentSyncKind {
    let sync = initialize_response.pointer("/result/capabilities/textDocumentSync");
    let kind = sync
        .and_then(Value::as_u64)
        .or_else(|| sync?.get("change")?.as_u64())
        .unwrap_or(1);
    match kind {
        0 => DocumentSyncKind::None,
        2 => DocumentSyncKind::Incremental,
        _ => DocumentSyncKind::Full,
    }
}

/// Produces one minimal contiguous LSP edit. Positions are UTF-16 code units,
/// as required by LSP, even when the changed line contains non-ASCII text.
pub(crate) fn incremental_change<'a>(old: &str, new: &'a str) -> (TextRange, &'a str) {
    let mut prefix = 0;
    for (old_char, new_char) in old.chars().zip(new.chars()) {
        if old_char != new_char {
            break;
        }
        prefix += old_char.len_utf8();
    }

    let mut suffix = 0;
    for (old_char, new_char) in old[prefix..].chars().rev().zip(new[prefix..].chars().rev()) {
        if old_char != new_char {
            break;
        }
        suffix += old_char.len_utf8();
    }
    let old_end = old.len().saturating_sub(suffix);
    let new_end = new.len().saturating_sub(suffix);
    (
        TextRange {
            start: lsp_position(&old[..prefix]),
            end: lsp_position(&old[..old_end]),
        },
        &new[prefix..new_end],
    )
}

fn lsp_position(prefix: &str) -> TextPosition {
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let line_text = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    TextPosition {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(line_text.encode_utf16().count()).unwrap_or(u32::MAX),
    }
}

pub(crate) fn parse_diagnostics(
    key: &LanguageServerKey,
    message: &Value,
) -> Option<DocumentDiagnostics> {
    (message.get("method")?.as_str()? == "textDocument/publishDiagnostics").then_some(())?;
    let params = message.get("params")?;
    let path = url::Url::parse(params.get("uri")?.as_str()?)
        .ok()?
        .to_file_path()
        .ok()?;
    let relative_path = path.strip_prefix(&key.project_root).ok()?.to_path_buf();
    let items = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(parse_diagnostic)
        .collect();
    Some(DocumentDiagnostics {
        project_root: key.project_root.clone(),
        relative_path,
        version: params
            .get("version")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok()),
        server_id: key.server_id.clone(),
        items,
    })
}

fn parse_diagnostic(value: &Value) -> Option<Diagnostic> {
    let range = value.get("range")?;
    let position = |name: &str| -> Option<TextPosition> {
        let value = range.get(name)?;
        Some(TextPosition {
            line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
            character: u32::try_from(value.get("character")?.as_u64()?).ok()?,
        })
    };
    let code = value.get("code").and_then(|code| {
        code.as_str()
            .map(str::to_owned)
            .or_else(|| code.as_i64().map(|code| code.to_string()))
    });
    Some(Diagnostic {
        range: TextRange {
            start: position("start")?,
            end: position("end")?,
        },
        severity: DiagnosticSeverity::from_lsp(value.get("severity").and_then(Value::as_u64)),
        message: value.get("message")?.as_str()?.to_string(),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        code,
    })
}

pub(crate) fn restart_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(5);
    Duration::from_millis(500_u64.saturating_mul(1_u64 << exponent)).min(Duration::from_secs(15))
}

pub(crate) fn is_deterministic_startup_failure(reason: &str) -> bool {
    let reason = reason.to_ascii_lowercase();
    [
        "cannot find module",
        "module_not_found",
        "permission denied",
        "exec format error",
        "bad interpreter",
    ]
    .iter()
    .any(|signature| reason.contains(signature))
}
