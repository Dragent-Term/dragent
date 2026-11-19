//! UI-independent editor domains shared by native and web surfaces.

mod breakpoints;
mod buffer_search;
mod comment_context;
mod file_kind;
mod folding;
mod guides;
mod highlighting;
mod intelligence;
mod language;
mod language_services;
mod line_changes;
mod syntax_highlight;

pub use breakpoints::{Breakpoint, BreakpointRegistry, BreakpointState, BreakpointStore};
pub use buffer_search::{find_buffer_matches, BufferMatch, BufferSearchOptions};
pub use comment_context::comment_style_at;
pub use file_kind::FileKind;
pub use folding::{FoldIndex, FoldParser, FoldRegion, FoldRegionKind, FoldState};
pub use guides::IndentGuideIndex;
pub use highlighting::{HighlightSpan, HighlightState, LexicalHighlighter, TokenKind};
pub use intelligence::{
    char_index_to_lsp_position, lsp_position_to_char_index, lsp_range_to_char_range, Diagnostic,
    DiagnosticSeverity, DocumentDiagnostics, LanguageRequest, LanguageRequestKind,
    ResolvedDiagnostic, TextPosition, TextRange,
};
pub use language::{CommentStyle, Language};
pub use language_services::{
    ActivationRule, LanguageCapability, LanguageServerDefinition, LanguageServerRole,
    LanguageServiceDefinition, LanguageServiceRegistry, ServerCandidate,
};
pub use line_changes::LineChangeKind;
pub use syntax_highlight::{
    grammar_for, grammar_for_fence, highlight_document, highlight_document_cached,
    DocumentHighlights,
};
