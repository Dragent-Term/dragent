use std::path::Path;

use dragent_editor::{Language, LanguageServiceRegistry, TextRange};
use serde_json::{json, Value};

use crate::{file_uri, LanguageServerKey};

pub fn initialize_params(key: &LanguageServerKey) -> Value {
    let root_uri = file_uri(&key.project_root);
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Dragent", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "capabilities": {
            "workspace": {
                "workspaceFolders": true,
                "configuration": true,
                "applyEdit": false,
                "workspaceEdit": {"documentChanges": true},
                "fileOperations": {
                    "willRename": true,
                    "didRename": true
                }
            },
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "willSave": false,
                    "didSave": true
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "versionSupport": true,
                    "codeDescriptionSupport": true
                },
                "completion": {
                    "completionItem": {
                        "snippetSupport": false,
                        "documentationFormat": ["markdown", "plaintext"],
                        "insertReplaceSupport": true,
                        "labelDetailsSupport": true
                    },
                    "contextSupport": true
                },
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "signatureHelp": {
                    "signatureInformation": {
                        "documentationFormat": ["markdown", "plaintext"],
                        "parameterInformation": {"labelOffsetSupport": true},
                        "activeParameterSupport": true
                    },
                    "contextSupport": true
                },
                "definition": {"linkSupport": true},
                "references": {},
                "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                "codeAction": {"dataSupport": true},
                "formatting": {},
                "rename": {"prepareSupport": true},
                "inlayHint": {"resolveSupport": {"properties": ["tooltip", "textEdits", "label.tooltip", "label.location", "label.command"]}},
                "semanticTokens": {
                    "requests": {"range": true, "full": {"delta": true}},
                    "tokenTypes": [
                        "namespace", "type", "class", "enum", "interface", "struct",
                        "typeParameter", "parameter", "variable", "property", "enumMember",
                        "event", "function", "method", "macro", "keyword", "modifier",
                        "comment", "string", "number", "regexp", "operator", "decorator"
                    ],
                    "tokenModifiers": [
                        "declaration", "definition", "readonly", "static", "deprecated",
                        "abstract", "async", "modification", "documentation", "defaultLibrary"
                    ],
                    "formats": ["relative"],
                    "overlappingTokenSupport": false,
                    "multilineTokenSupport": true
                }
            }
        },
        "workspaceFolders": [{
            "uri": file_uri(&key.project_root),
            "name": key.project_root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")
        }]
    })
}

pub fn did_open(key: &LanguageServerKey, path: &Path, text: &str, version: i32) -> Option<Value> {
    let language_id = LanguageServiceRegistry::definition(Language::from_path(path))?.language_id;
    Some(json!({
        "textDocument": {
            "uri": file_uri(&key.project_root.join(path))?,
            "languageId": language_id,
            "version": version,
            "text": text
        }
    }))
}

pub fn did_change(key: &LanguageServerKey, path: &Path, text: &str, version: i32) -> Option<Value> {
    Some(json!({
        "textDocument": {
            "uri": file_uri(&key.project_root.join(path))?,
            "version": version
        },
        "contentChanges": [{"text": text}]
    }))
}

pub fn did_change_incremental(
    key: &LanguageServerKey,
    path: &Path,
    range: TextRange,
    text: &str,
    version: i32,
) -> Option<Value> {
    Some(json!({
        "textDocument": {
            "uri": file_uri(&key.project_root.join(path))?,
            "version": version
        },
        "contentChanges": [{
            "range": {
                "start": {"line": range.start.line, "character": range.start.character},
                "end": {"line": range.end.line, "character": range.end.character}
            },
            "text": text
        }]
    }))
}

pub fn did_save(key: &LanguageServerKey, path: &Path, text: &str) -> Option<Value> {
    Some(json!({
        "textDocument": {"uri": file_uri(&key.project_root.join(path))?},
        "text": text
    }))
}

pub fn did_close(key: &LanguageServerKey, path: &Path) -> Option<Value> {
    Some(json!({
        "textDocument": {"uri": file_uri(&key.project_root.join(path))?}
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> LanguageServerKey {
        LanguageServerKey::new(std::env::current_dir().unwrap(), "rust-analyzer")
    }

    #[test]
    fn open_uses_registered_language_and_full_text() {
        let params = did_open(&key(), Path::new("src/main.rs"), "fn main() {}", 3).unwrap();
        assert_eq!(params["textDocument"]["languageId"], "rust");
        assert_eq!(params["textDocument"]["version"], 3);
        assert_eq!(params["textDocument"]["text"], "fn main() {}");
    }

    #[test]
    fn initialize_identifies_the_workspace() {
        let params = initialize_params(&key());
        assert!(params["rootUri"].as_str().unwrap().starts_with("file://"));
        assert_eq!(params["workspaceFolders"].as_array().unwrap().len(), 1);
        assert_eq!(
            params["capabilities"]["textDocument"]["completion"]["completionItem"]
                ["snippetSupport"],
            false
        );
        assert!(
            params["capabilities"]["textDocument"]["semanticTokens"]["tokenTypes"]
                .as_array()
                .is_some_and(|types| !types.is_empty())
        );
        assert_eq!(
            params["capabilities"]["workspace"]["fileOperations"]["willRename"],
            true
        );
        assert_eq!(
            params["capabilities"]["workspace"]["fileOperations"]["didRename"],
            true
        );
    }

    #[test]
    fn change_is_a_full_versioned_sync() {
        let params = did_change(&key(), Path::new("src/lib.rs"), "pub fn value() {}", 9).unwrap();
        assert_eq!(params["textDocument"]["version"], 9);
        assert_eq!(params["contentChanges"][0]["text"], "pub fn value() {}");
    }

    #[test]
    fn incremental_change_contains_a_utf16_range() {
        let params = did_change_incremental(
            &key(),
            Path::new("src/lib.rs"),
            TextRange {
                start: dragent_editor::TextPosition {
                    line: 2,
                    character: 4,
                },
                end: dragent_editor::TextPosition {
                    line: 2,
                    character: 6,
                },
            },
            "value",
            10,
        )
        .unwrap();
        assert_eq!(params["textDocument"]["version"], 10);
        assert_eq!(params["contentChanges"][0]["range"]["start"]["line"], 2);
        assert_eq!(params["contentChanges"][0]["text"], "value");
    }
}
