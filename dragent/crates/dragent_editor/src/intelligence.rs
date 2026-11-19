//! UI-independent language intelligence shared by editor surfaces.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: u32,
    /// UTF-16 code-unit offset, as required by the LSP protocol.
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// A diagnostic paired with the editor's character-index range. Keeping the
/// original payload lets presentation surfaces provide hover details and
/// problem navigation without converting LSP positions while painting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDiagnostic {
    pub range: std::ops::Range<usize>,
    pub diagnostic: Diagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentDiagnostics {
    pub project_root: PathBuf,
    pub relative_path: PathBuf,
    pub version: Option<i32>,
    pub server_id: String,
    pub items: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanguageRequestKind {
    Completion,
    Hover,
    Definition,
    TypeDefinition,
    Implementation,
    References { include_declaration: bool },
    SignatureHelp,
    DocumentSymbols,
    CodeActions { range: TextRange },
    Formatting { tab_size: u32, insert_spaces: bool },
    Rename { new_name: String },
    InlayHints { range: TextRange },
    SemanticTokens,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageRequest {
    pub position: TextPosition,
    pub kind: LanguageRequestKind,
}

/// Convert a character index used by the native editor into an LSP UTF-16
/// position. This is intentionally linear and allocation-free; callers use it
/// only for the current cursor, not while painting every visible row.
pub fn char_index_to_lsp_position(text: &str, char_index: usize) -> TextPosition {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in text.chars().take(char_index) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(value.len_utf16() as u32);
        }
    }
    TextPosition { line, character }
}

pub fn lsp_position_to_char_index(text: &str, position: TextPosition) -> usize {
    let mut line = 0_u32;
    let mut utf16_column = 0_u32;
    let mut char_index = 0_usize;
    for value in text.chars() {
        if line == position.line {
            if value == '\n'
                || utf16_column.saturating_add(value.len_utf16() as u32) > position.character
            {
                return char_index;
            }
            utf16_column = utf16_column.saturating_add(value.len_utf16() as u32);
            char_index += 1;
            if utf16_column == position.character {
                return char_index;
            }
        } else {
            char_index += 1;
            if value == '\n' {
                line = line.saturating_add(1);
            }
        }
    }
    char_index
}

pub fn lsp_range_to_char_range(text: &str, range: TextRange) -> std::ops::Range<usize> {
    let start = lsp_position_to_char_index(text, range.start);
    let end = lsp_position_to_char_index(text, range.end).max(start);
    start..end
}

impl DiagnosticSeverity {
    pub fn from_lsp(value: Option<u64>) -> Self {
        match value {
            Some(1) => Self::Error,
            Some(2) => Self::Warning,
            Some(3) => Self::Information,
            Some(4) => Self::Hint,
            _ => Self::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_severity_mapping_is_stable() {
        assert_eq!(
            DiagnosticSeverity::from_lsp(Some(2)),
            DiagnosticSeverity::Warning
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp(None),
            DiagnosticSeverity::Error
        );
    }

    #[test]
    fn cursor_positions_use_utf16_columns() {
        assert_eq!(
            char_index_to_lsp_position("a😀b\nnext", 3),
            TextPosition {
                line: 0,
                character: 4
            }
        );
        assert_eq!(
            char_index_to_lsp_position("a😀b\nnext", 5),
            TextPosition {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn lsp_ranges_convert_back_to_native_character_offsets() {
        let text = "a😀b\nnext";
        assert_eq!(
            lsp_range_to_char_range(
                text,
                TextRange {
                    start: TextPosition {
                        line: 0,
                        character: 1
                    },
                    end: TextPosition {
                        line: 0,
                        character: 3
                    },
                }
            ),
            1..2
        );
    }
}
