//! Language navigation parsing helpers for the workspace.
//!
//! Responsible for:
//! - parsing LSP JSON responses into navigation locations and text edits
//! - position/range conversions between LSP, UTF-16, and document coords
//! - import-specifier resolution and reference previews
//!
//! Not responsible for:
//! - running language servers (dragent_language_services broker)
//! - rendering navigation UI (WorkspaceView handlers)
//!
//! All functions here are free functions with no View dependencies.

use std::path::{Path, PathBuf};

use dragent_editor::{lsp_position_to_char_index, TextPosition, TextRange};

use super::LanguageNavigationCapabilities;

#[derive(Clone, Debug)]
pub(crate) struct LanguageLocation {
    pub(crate) relative_path: PathBuf,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) struct DefinitionHoverResolution {
    pub(crate) request_id: u64,
    pub(crate) project_root: PathBuf,
    pub(crate) source_relative_path: PathBuf,
    pub(crate) symbol_range: std::ops::Range<usize>,
    pub(crate) locations: Vec<LanguageLocation>,
}

#[derive(Clone, Debug)]
pub(crate) struct LanguageTextEdit {
    pub(crate) relative_path: PathBuf,
    pub(crate) range: TextRange,
    pub(crate) new_text: String,
}

pub(crate) fn lsp_position(value: &serde_json::Value) -> Option<TextPosition> {
    Some(TextPosition {
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
        character: u32::try_from(value.get("character")?.as_u64()?).ok()?,
    })
}

pub(crate) fn lsp_range(value: &serde_json::Value) -> Option<TextRange> {
    Some(TextRange {
        start: lsp_position(value.get("start")?)?,
        end: lsp_position(value.get("end")?)?,
    })
}

pub(crate) fn lsp_uri_relative_path(root: &Path, uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

pub(crate) fn parse_language_location(
    root: &Path,
    value: &serde_json::Value,
) -> Option<LanguageLocation> {
    let uri = value
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("targetUri").and_then(serde_json::Value::as_str))?;
    let range = value
        .get("range")
        // LocationLink's selection range identifies the symbol itself; its
        // targetRange may cover the complete declaration/body.
        .or_else(|| value.get("targetSelectionRange"))
        .or_else(|| value.get("targetRange"))
        .and_then(lsp_range)?;
    Some(LanguageLocation {
        relative_path: lsp_uri_relative_path(root, uri)?,
        range,
    })
}

pub(crate) fn parse_language_locations(
    root: &Path,
    value: &serde_json::Value,
) -> Vec<LanguageLocation> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_language_location(root, item))
                .collect()
        })
        .or_else(|| parse_language_location(root, value).map(|location| vec![location]))
        .unwrap_or_default()
}

pub(crate) fn language_navigation_title(kind: super::LanguageNavigationKind) -> &'static str {
    match kind {
        super::LanguageNavigationKind::Definition => "Definitions",
        super::LanguageNavigationKind::TypeDefinition => "Type definitions",
        super::LanguageNavigationKind::Implementation => "Implementations",
        super::LanguageNavigationKind::References => "References",
    }
}

pub(crate) fn language_navigation_empty_message(
    kind: super::LanguageNavigationKind,
) -> &'static str {
    match kind {
        super::LanguageNavigationKind::Definition => "No definition found",
        super::LanguageNavigationKind::TypeDefinition => "No type definition found",
        super::LanguageNavigationKind::Implementation => "No implementations found",
        super::LanguageNavigationKind::References => "No references found",
    }
}

pub(crate) fn language_provider_capability(value: Option<&serde_json::Value>) -> bool {
    value.is_some_and(|value| {
        value
            .as_bool()
            .or_else(|| value.as_object().map(|_| true))
            .unwrap_or(false)
    })
}

/// Convert an LSP UTF-16 column to a char index within `line`.
pub(crate) fn utf16_column_to_char(line: &str, utf16_column: usize) -> usize {
    let mut utf16 = 0usize;
    for (index, ch) in line.chars().enumerate() {
        if utf16 >= utf16_column {
            return index;
        }
        utf16 += ch.len_utf16();
    }
    line.chars().count()
}

/// Extract the line containing a reference plus the match's char range within
/// that line, for peek-style preview rows.
pub(crate) fn reference_line_preview(
    text: &str,
    range: TextRange,
) -> Option<(String, usize, usize)> {
    let line = text.lines().nth(range.start.line as usize)?;
    let start = utf16_column_to_char(line, range.start.character as usize);
    let end = if range.end.line == range.start.line {
        utf16_column_to_char(line, range.end.character as usize)
    } else {
        line.chars().count()
    };
    let (start, end) = (start.min(end), end.max(start));
    Some((line.to_string(), start, end))
}

pub(crate) fn language_navigation_capabilities(
    capabilities: &serde_json::Value,
) -> LanguageNavigationCapabilities {
    LanguageNavigationCapabilities {
        definition: language_provider_capability(capabilities.get("definitionProvider")),
        type_definition: language_provider_capability(capabilities.get("typeDefinitionProvider")),
        implementation: language_provider_capability(capabilities.get("implementationProvider")),
        references: language_provider_capability(capabilities.get("referencesProvider")),
    }
}

/// Resolve a relative import specifier (`"./a/b/c"`, `"../../x"`) to a file
/// inside `root`, trying the usual extension and index-file candidates. Only
/// used as the fallback for string links the language server could not
/// resolve; alias resolution (`@/…`) stays with the language server.
pub(crate) fn resolve_import_specifier(
    root: &Path,
    source_relative_path: &Path,
    specifier: &str,
) -> Option<PathBuf> {
    let base = source_relative_path.parent()?;
    let mut target = base.to_path_buf();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if !target.pop() {
                    return None;
                }
            }
            other => target.push(other),
        }
    }
    const CANDIDATE_SUFFIXES: &[&str] = &[
        "",
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".mjs",
        ".cjs",
        ".json",
        ".css",
        ".scss",
        "/index.ts",
        "/index.tsx",
        "/index.js",
        "/index.jsx",
    ];
    for suffix in CANDIDATE_SUFFIXES {
        let mut candidate = root.join(&target).into_os_string();
        candidate.push(suffix);
        let candidate = PathBuf::from(candidate);
        if candidate.is_file() {
            return Some(
                candidate
                    .strip_prefix(root)
                    .unwrap_or(&candidate)
                    .to_path_buf(),
            );
        }
    }
    None
}

pub(crate) fn parse_text_edits_for_path(
    root: &Path,
    relative_path: &Path,
    value: &serde_json::Value,
) -> Vec<LanguageTextEdit> {
    let parse_edits = |path: PathBuf, edits: &serde_json::Value| {
        edits
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|edit| {
                Some(LanguageTextEdit {
                    relative_path: path.clone(),
                    range: lsp_range(edit.get("range")?)?,
                    new_text: edit.get("newText")?.as_str()?.to_owned(),
                })
            })
            .collect::<Vec<_>>()
    };
    if let Some(edits) = value.as_array() {
        return parse_edits(
            relative_path.to_path_buf(),
            &serde_json::Value::Array(edits.clone()),
        );
    }
    let mut result = Vec::new();
    if let Some(changes) = value.get("changes").and_then(serde_json::Value::as_object) {
        for (uri, edits) in changes {
            if let Some(path) = lsp_uri_relative_path(root, uri) {
                result.extend(parse_edits(path, edits));
            }
        }
    }
    if let Some(document_changes) = value
        .get("documentChanges")
        .and_then(serde_json::Value::as_array)
    {
        for change in document_changes {
            let Some(uri) = change
                .get("textDocument")
                .and_then(|doc| doc.get("uri"))
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            if let Some(path) = lsp_uri_relative_path(root, uri) {
                if let Some(edits) = change.get("edits") {
                    result.extend(parse_edits(path, edits));
                }
            }
        }
    }
    result
}

pub(crate) fn apply_text_edits(
    text: &str,
    cursor: usize,
    edits: &[LanguageTextEdit],
) -> Option<(String, usize)> {
    if edits.is_empty() {
        return None;
    }
    let mut local = edits.to_vec();
    local.sort_by_key(|edit| std::cmp::Reverse(lsp_position_to_char_index(text, edit.range.start)));
    let mut chars: Vec<char> = text.chars().collect();
    let mut new_cursor = cursor.min(chars.len());
    for edit in local {
        let start = lsp_position_to_char_index(text, edit.range.start).min(chars.len());
        let end = lsp_position_to_char_index(text, edit.range.end)
            .max(start)
            .min(chars.len());
        let old_len = end - start;
        let replacement: Vec<char> = edit.new_text.chars().collect();
        if start <= new_cursor {
            new_cursor = if new_cursor <= end {
                start + replacement.len()
            } else {
                new_cursor - old_len + replacement.len()
            };
        }
        chars.splice(start..end, replacement);
    }
    Some((chars.into_iter().collect(), new_cursor))
}

/// Snapshot a single completed block's output for one-shot error harvesting.
/// Runs once per command completion — never on the streaming-output hot path.
pub(crate) fn completed_block_snapshot(
    model: &crate::terminal::model::terminal_model::TerminalModel,
    block_id: usize,
) -> Option<super::project_sidebar::BlockSnapshot> {
    crate::terminal::model::terminal_model::completed_block_snapshot(model, block_id)
}

pub(crate) fn completion_suffix(
    result: &serde_json::Value,
    text: &str,
    cursor: usize,
) -> Option<String> {
    let items = result
        .as_array()
        .or_else(|| result.get("items").and_then(serde_json::Value::as_array))?;
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|character| !character.is_alphanumeric() && *character != '_')
        .map_or(0, |index| index + 1);
    let prefix = chars[start..cursor].iter().collect::<String>();
    items.iter().find_map(|item| {
        // Snippet placeholders need a snippet session, not literal insertion.
        if item
            .get("insertTextFormat")
            .and_then(serde_json::Value::as_u64)
            == Some(2)
        {
            return None;
        }
        let insertion = item
            .get("textEdit")
            .and_then(|edit| edit.get("newText"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| item.get("insertText").and_then(serde_json::Value::as_str))
            .or_else(|| item.get("label").and_then(serde_json::Value::as_str))?;
        insertion
            .strip_prefix(&prefix)
            .filter(|suffix| !suffix.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod language_completion_tests {
    use super::{
        apply_text_edits, completion_suffix, parse_language_locations, reference_line_preview,
        resolve_import_specifier, utf16_column_to_char, LanguageTextEdit,
    };
    use dragent_editor::{TextPosition, TextRange};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn reference_preview_extracts_line_and_match_range() {
        let text = "fn main() {\n    let total = calculate_total(items);\n}\n";
        let range = TextRange {
            start: TextPosition {
                line: 1,
                character: 16, // UTF-16 col of `calculate_total`
            },
            end: TextPosition {
                line: 1,
                character: 31,
            },
        };
        let (line, start, end) = reference_line_preview(text, range).unwrap();
        assert_eq!(line, "    let total = calculate_total(items);");
        assert_eq!(&line[start..end], "calculate_total");
    }

    #[test]
    fn utf16_columns_map_through_surrogate_pairs() {
        // Each 😀 is 1 char but 2 UTF-16 code units.
        let line = "😀😀 needle";
        assert_eq!(utf16_column_to_char(line, 0), 0);
        assert_eq!(utf16_column_to_char(line, 4), 2);
        assert_eq!(utf16_column_to_char(line, 5), 3);
        assert_eq!(utf16_column_to_char(line, 100), line.chars().count());
    }

    #[test]
    fn reference_preview_clamps_multiline_matches_to_one_line() {
        let text = "alpha\nbeta\n";
        let range = TextRange {
            start: TextPosition {
                line: 1,
                character: 0,
            },
            end: TextPosition {
                line: 2,
                character: 2,
            },
        };
        let (line, start, end) = reference_line_preview(text, range).unwrap();
        assert_eq!(line, "beta");
        assert_eq!(&line[start..end], "beta");
    }

    #[test]
    fn import_specifier_resolves_relative_paths_with_candidate_extensions() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("frontend/src/components");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("UserProfile.tsx"), "export const X = 1;").unwrap();
        let shared = root.path().join("frontend/src/shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("index.ts"), "export const Y = 2;").unwrap();
        std::fs::write(src.join("data.json"), "{}").unwrap();

        let source = std::path::Path::new("frontend/src/components/Page.tsx");

        // "./UserProfile" → UserProfile.tsx
        let resolved = resolve_import_specifier(root.path(), source, "./UserProfile").unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("frontend/src/components/UserProfile.tsx")
        );
        // "../shared" → shared/index.ts
        let resolved = resolve_import_specifier(root.path(), source, "../shared").unwrap();
        assert_eq!(resolved, PathBuf::from("frontend/src/shared/index.ts"));
        // "./data.json" → exact file wins before extension probing
        let resolved = resolve_import_specifier(root.path(), source, "./data.json").unwrap();
        assert_eq!(resolved, PathBuf::from("frontend/src/components/data.json"));
        // Missing target → None (no fake links).
        assert!(resolve_import_specifier(root.path(), source, "./Missing").is_none());
        // Alias specifiers are left to the language server.
        assert!(
            resolve_import_specifier(root.path(), source, "@/components/UserProfile").is_none()
        );
    }

    #[test]
    fn derives_ghost_suffix_from_the_current_identifier() {
        let result = json!({"items": [{"label": "println"}]});
        assert_eq!(
            completion_suffix(&result, "fn main() { print", 17),
            Some("ln".to_string())
        );
    }

    #[test]
    fn ignores_snippets_until_a_snippet_session_exists() {
        let result = json!([{
            "label": "println!",
            "insertText": "println!(${1})",
            "insertTextFormat": 2
        }]);
        assert_eq!(completion_suffix(&result, "pri", 3), None);
    }

    #[test]
    fn parses_location_links_into_project_relative_paths() {
        let root = PathBuf::from("/tmp/project");
        let uri = url::Url::from_file_path(root.join("src/lib.rs"))
            .unwrap()
            .to_string();
        let locations = parse_language_locations(
            &root,
            &json!([{
                "uri": uri,
                "range": {
                    "start": {"line": 3, "character": 4},
                    "end": {"line": 3, "character": 10}
                }
            }]),
        );
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].relative_path, PathBuf::from("src/lib.rs"));
        assert_eq!(locations[0].range.start.line, 3);
    }

    #[test]
    fn location_links_prefer_the_precise_target_selection_range() {
        let root = PathBuf::from("/tmp/project");
        let uri = url::Url::from_file_path(root.join("src/lib.rs"))
            .unwrap()
            .to_string();
        let locations = parse_language_locations(
            &root,
            &json!({
                "targetUri": uri,
                "targetRange": {
                    "start": {"line": 20, "character": 0},
                    "end": {"line": 24, "character": 1}
                },
                "targetSelectionRange": {
                    "start": {"line": 20, "character": 7},
                    "end": {"line": 20, "character": 23}
                }
            }),
        );

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 20);
        assert_eq!(locations[0].range.start.character, 7);
        assert_eq!(locations[0].range.end.character, 23);
    }

    #[test]
    fn applies_multiple_language_edits_in_reverse_document_order() {
        let edits = vec![
            LanguageTextEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                range: TextRange {
                    start: TextPosition {
                        line: 1,
                        character: 0,
                    },
                    end: TextPosition {
                        line: 1,
                        character: 3,
                    },
                },
                new_text: "two".to_string(),
            },
            LanguageTextEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                range: TextRange {
                    start: TextPosition {
                        line: 0,
                        character: 0,
                    },
                    end: TextPosition {
                        line: 0,
                        character: 3,
                    },
                },
                new_text: "one".to_string(),
            },
        ];
        let result = apply_text_edits("old\nold", 7, &edits).unwrap();
        assert_eq!(result.0, "one\ntwo");
        assert_eq!(result.1, 7);
    }
}
