//! Background document/LSP coordinator.
//!
//! UI code submits bounded, versioned document snapshots. This worker owns all
//! process operations and JSON-RPC state so server startup, failure, and idle
//! cleanup can never block rendering.

pub use super::client_api::*;

mod tests {
    use super::*;
    use crate::client_worker::{
        document_sync_kind, incremental_change, is_deterministic_startup_failure,
        parse_diagnostics, request_payload, restart_delay, supports_file_operation,
        DocumentSyncKind,
    };
    use crate::LanguageServerCatalog;
    use crate::LanguageServerKey;
    use dragent_editor::{
        DiagnosticSeverity, Language, LanguageRequest, LanguageRequestKind, TextPosition,
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn restart_backoff_is_bounded() {
        assert_eq!(restart_delay(1), Duration::from_millis(500));
        assert_eq!(restart_delay(3), Duration::from_secs(2));
        assert_eq!(restart_delay(99), Duration::from_secs(15));
    }

    #[test]
    fn broken_executable_errors_do_not_enter_a_restart_loop() {
        assert!(is_deterministic_startup_failure(
            "Error: Cannot find module '/managed/server/lib/main.js'"
        ));
        assert!(is_deterministic_startup_failure("code: 'MODULE_NOT_FOUND'"));
        assert!(!is_deterministic_startup_failure(
            "connection reset while the server was running"
        ));
    }

    #[test]
    fn unsupported_documents_do_not_create_keys() {
        let document = SyncedDocument {
            project_root: PathBuf::from("/tmp/project"),
            relative_path: PathBuf::from("LICENSE"),
            text: String::new(),
        };
        assert!(document.keys(&LanguageServerCatalog::built_in()).is_empty());
    }

    #[test]
    fn javascript_and_typescript_share_server_identity() {
        let root = PathBuf::from("/tmp/project");
        let javascript = SyncedDocument {
            project_root: root.clone(),
            relative_path: PathBuf::from("src/app.js"),
            text: String::new(),
        };
        let typescript = SyncedDocument {
            project_root: root,
            relative_path: PathBuf::from("src/app.ts"),
            text: String::new(),
        };
        let catalog = LanguageServerCatalog::built_in();
        assert_eq!(javascript.keys(&catalog)[0], typescript.keys(&catalog)[0]);
    }

    #[test]
    fn managed_downloads_are_denied_by_default() {
        assert!(!LanguageServiceClientConfig::default().allow_managed_downloads);
    }

    #[test]
    fn detects_static_file_rename_capabilities() {
        let capabilities = json!({
            "workspace": {
                "fileOperations": {
                    "willRename": {"filters": []},
                    "didRename": {"filters": []}
                }
            }
        });
        assert!(supports_file_operation(&capabilities, "willRename"));
        assert!(supports_file_operation(&capabilities, "didRename"));
        assert!(!supports_file_operation(&capabilities, "willDelete"));
        assert!(!supports_file_operation(
            &json!({"workspace":{"fileOperations":{"willRename": false}}}),
            "willRename"
        ));
    }

    #[test]
    fn parses_versioned_diagnostics_into_editor_types() {
        let root = std::env::temp_dir().join("dragent-diagnostic-test");
        let key = LanguageServerKey::new(root.clone(), "test-ls");
        let uri = crate::file_uri(&root.join("src/main.rs")).unwrap();
        let message = json!({
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "version": 7,
                "diagnostics": [{
                    "range": {
                        "start": {"line": 2, "character": 4},
                        "end": {"line": 2, "character": 9}
                    },
                    "severity": 2,
                    "message": "unused value",
                    "source": "test"
                }]
            }
        });
        let diagnostics = parse_diagnostics(&key, &message).unwrap();
        assert_eq!(diagnostics.project_root, root);
        assert_eq!(diagnostics.relative_path, PathBuf::from("src/main.rs"));
        assert_eq!(diagnostics.version, Some(7));
        assert_eq!(diagnostics.items[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn reads_full_and_incremental_server_sync_capabilities() {
        assert_eq!(
            document_sync_kind(&json!({"result":{"capabilities":{"textDocumentSync":2}}})),
            DocumentSyncKind::Incremental
        );
        assert_eq!(
            document_sync_kind(
                &json!({"result":{"capabilities":{"textDocumentSync":{"change":1}}}})
            ),
            DocumentSyncKind::Full
        );
    }

    #[test]
    fn incremental_edit_uses_utf16_positions() {
        let (range, replacement) = incremental_change("one\n🙂cat", "one\n🙂coat");
        assert_eq!(replacement, "o");
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 3);
        assert_eq!(range.end.character, 3);
    }

    #[test]
    fn completion_requests_use_lsp_utf16_positions() {
        let key = LanguageServerKey::new(std::env::temp_dir(), "test-ls");
        let (method, params) = request_payload(
            &key,
            std::path::Path::new("src/main.rs"),
            &LanguageRequest {
                position: TextPosition {
                    line: 3,
                    character: 8,
                },
                kind: LanguageRequestKind::Completion,
            },
        )
        .unwrap();
        assert_eq!(method, "textDocument/completion");
        assert_eq!(params["position"]["line"], 3);
        assert_eq!(params["position"]["character"], 8);
    }

    #[test]
    fn navigation_requests_use_distinct_lsp_methods() {
        let key = LanguageServerKey::new(std::env::temp_dir(), "test-ls");
        let path = std::path::Path::new("src/main.rs");
        for (kind, expected_method) in [
            (LanguageRequestKind::Definition, "textDocument/definition"),
            (
                LanguageRequestKind::TypeDefinition,
                "textDocument/typeDefinition",
            ),
            (
                LanguageRequestKind::Implementation,
                "textDocument/implementation",
            ),
            (
                LanguageRequestKind::References {
                    include_declaration: true,
                },
                "textDocument/references",
            ),
        ] {
            let (method, params) = request_payload(
                &key,
                path,
                &LanguageRequest {
                    position: TextPosition {
                        line: 2,
                        character: 6,
                    },
                    kind,
                },
            )
            .unwrap();
            assert_eq!(method, expected_method);
            assert_eq!(params["position"]["line"], 2);
            assert_eq!(params["position"]["character"], 6);
        }
    }

    #[test]
    fn background_client_shuts_down_cleanly_without_sessions() {
        let client = LanguageServiceClient::start(LanguageServiceClientConfig {
            max_servers: 1,
            idle_timeout: Duration::from_millis(10),
            ..LanguageServiceClientConfig::default()
        })
        .unwrap();
        client
            .open(SyncedDocument {
                project_root: std::env::temp_dir(),
                relative_path: PathBuf::from("LICENSE"),
                text: "plain text".to_string(),
            })
            .unwrap();
        drop(client);
    }

    #[cfg(unix)]
    #[test]
    fn completes_initialize_handshake_with_a_server_process() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("dragent-lsp-client-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() {}\n").unwrap();
        std::fs::write(root.join("src/second.rs"), "pub fn second() {}\n").unwrap();
        let server = root.join("fake-rust-analyzer.sh");
        std::fs::write(
            &server,
            r#"#!/bin/sh
IFS= read -r header || exit 1
while IFS= read -r line; do
  [ "$line" = "$(printf '\r')" ] && break
done
body='{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":2}}}'
printf 'Content-Length: %s\r\n\r\n%s' "${#body}" "$body"
cat > lsp-messages.log
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&server).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&server, permissions).unwrap();

        let mut overrides = HashMap::new();
        overrides.insert("rust-analyzer".to_string(), server);
        let client = LanguageServiceClient::start(LanguageServiceClientConfig {
            max_servers: 1,
            idle_timeout: Duration::from_secs(1),
            executable_overrides: overrides,
            allow_managed_downloads: false,
            catalog: Arc::new(LanguageServerCatalog::built_in()),
            initialize_timeout: Duration::from_secs(5),
        })
        .unwrap();
        client
            .open(SyncedDocument {
                project_root: root.clone(),
                relative_path: PathBuf::from("src/lib.rs"),
                text: "pub fn value() {}\n".to_string(),
            })
            .unwrap();
        client
            .open(SyncedDocument {
                project_root: root.clone(),
                relative_path: PathBuf::from("src/second.rs"),
                text: "pub fn second() {}\n".to_string(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut ready = false;
        let mut did_open = false;
        while Instant::now() < deadline {
            if matches!(client.try_recv(), Some(LanguageServiceEvent::Ready { .. })) {
                ready = true;
            }
            did_open =
                std::fs::read_to_string(root.join("lsp-messages.log")).is_ok_and(|messages| {
                    messages.contains("textDocument/didOpen")
                        && messages.contains("src/lib.rs")
                        && messages.contains("src/second.rs")
                });
            if ready && did_open {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.restart_all().unwrap();
        let restart_deadline = Instant::now() + Duration::from_secs(8);
        let mut restart_started = false;
        let mut restart_ready = false;
        let mut reopened = false;
        while Instant::now() < restart_deadline {
            match client.try_recv() {
                Some(LanguageServiceEvent::Starting(_)) => restart_started = true,
                Some(LanguageServiceEvent::Ready { .. }) if restart_started => {
                    restart_ready = true;
                }
                _ => {}
            }
            reopened = restart_ready
                && std::fs::read_to_string(root.join("lsp-messages.log")).is_ok_and(|messages| {
                    messages.contains("src/lib.rs") && messages.contains("src/second.rs")
                });
            if reopened {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client
            .change(SyncedDocument {
                project_root: root.clone(),
                relative_path: PathBuf::from("src/lib.rs"),
                text: "pub fn changed() {}\n".to_string(),
            })
            .unwrap();
        let change_deadline = Instant::now() + Duration::from_secs(3);
        let mut did_change = false;
        while Instant::now() < change_deadline {
            did_change =
                std::fs::read_to_string(root.join("lsp-messages.log")).is_ok_and(|messages| {
                    messages.contains("textDocument/didChange") && messages.contains("\"range\"")
                });
            if did_change {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.shutdown();
        let _ = std::fs::remove_dir_all(&root);
        assert!(ready, "fake language server never completed initialization");
        assert!(
            did_open,
            "documents opened during startup were not both attached"
        );
        assert!(
            reopened,
            "restart did not use the shared startup path and reattach both documents"
        );
        assert!(
            did_change,
            "incremental didChange was not sent for an incremental server"
        );
    }

    #[cfg(unix)]
    #[test]
    fn initialization_timeout_becomes_failed_instead_of_starting_forever() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "dragent-lsp-timeout-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let server = root.join("hung-rust-analyzer.sh");
        std::fs::write(&server, "#!/bin/sh\ncat >/dev/null\n").unwrap();
        let mut permissions = std::fs::metadata(&server).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&server, permissions).unwrap();

        let mut overrides = HashMap::new();
        overrides.insert("rust-analyzer".to_string(), server);
        let client = LanguageServiceClient::start(LanguageServiceClientConfig {
            max_servers: 1,
            idle_timeout: Duration::from_secs(1),
            executable_overrides: overrides,
            allow_managed_downloads: false,
            catalog: Arc::new(LanguageServerCatalog::built_in()),
            initialize_timeout: Duration::from_millis(75),
        })
        .unwrap();
        client
            .open(SyncedDocument {
                project_root: root.clone(),
                relative_path: PathBuf::from("src/lib.rs"),
                text: "pub fn value() {}\n".to_string(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut failed = false;
        while Instant::now() < deadline {
            if matches!(client.try_recv(), Some(LanguageServiceEvent::Failed { .. })) {
                failed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.shutdown();
        let _ = std::fs::remove_dir_all(&root);
        assert!(failed, "hung initialize remained in Starting");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "manual test requiring a locally installed rust-analyzer"]
    fn manual_real_rust_analyzer_auto_starts_and_publishes_diagnostics() {
        let root = std::env::temp_dir().join(format!(
            "dragent-real-rust-analyzer-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"dragent_lsp_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let source = "pub fn broken() { let count: u32 = \"hello\"; }\n";
        std::fs::write(root.join("src/lib.rs"), source).unwrap();

        let client = LanguageServiceClient::start(LanguageServiceClientConfig {
            max_servers: 1,
            idle_timeout: Duration::from_secs(1),
            initialize_timeout: Duration::from_secs(30),
            ..LanguageServiceClientConfig::default()
        })
        .unwrap();
        let started_at = Instant::now();
        client
            .open(SyncedDocument {
                project_root: root.clone(),
                relative_path: PathBuf::from("src/lib.rs"),
                text: source.to_string(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut ready_at = None;
        let mut diagnostics_at = None;
        while Instant::now() < deadline {
            match client.try_recv() {
                Some(LanguageServiceEvent::Ready { capabilities, .. }) => {
                    eprintln!(
                        "real rust-analyzer capabilities sync={:?}",
                        capabilities.get("textDocumentSync")
                    );
                    ready_at = Some(started_at.elapsed());
                }
                Some(LanguageServiceEvent::Diagnostics(diagnostics))
                    if !diagnostics.items.is_empty() =>
                {
                    diagnostics_at = Some(started_at.elapsed());
                    break;
                }
                Some(LanguageServiceEvent::Failed { reason, .. }) => {
                    panic!("real rust-analyzer startup failed: {reason}");
                }
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        client.shutdown();
        let _ = std::fs::remove_dir_all(&root);
        eprintln!(
            "real rust-analyzer: ready={:?} diagnostics={:?}",
            ready_at, diagnostics_at
        );
        assert!(ready_at.is_some(), "rust-analyzer never became Ready");
        assert!(
            diagnostics_at.is_some(),
            "rust-analyzer did not publish the obvious type diagnostic"
        );
    }
}
