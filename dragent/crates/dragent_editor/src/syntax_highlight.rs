//! Whole-document syntax highlighting from tree-sitter grammars.
//!
//! The grammar's own highlight queries decide what every token is — including
//! embedded languages (JSX in TSX, CSS/JS in HTML) — so nothing here knows any
//! language's rules. The only table is the theme-slot -> `TokenKind` mapping,
//! the same role a color theme plays in any editor.

use crate::folding::syntax_grammar_name;
use crate::highlighting::{HighlightSpan, TokenKind};
use crate::Language;
use arborium::{GrammarStore, Highlighter};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Compiled grammars and queries are expensive to build, so every editor
/// shares one store; each highlight run gets a cheap `Highlighter` over it.
fn shared_store() -> Arc<GrammarStore> {
    static STORE: OnceLock<Arc<GrammarStore>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let store = GrammarStore::new();
            register_query_overrides(&store);
            Arc::new(store)
        })
        .clone()
}

/// Our additions to a stock grammar's highlight query, as query files — the
/// way other editors tune highlighting per language, rather than code.
const TSX_HIGHLIGHT_OVERRIDES: &str = include_str!("../queries/jsx-overrides.scm");

/// Register grammars whose highlight query we extend. Any grammar not listed
/// here is compiled lazily by the store with its stock queries.
fn register_query_overrides(store: &GrammarStore) {
    use arborium_highlight::{CompiledGrammar, GrammarConfig};

    // Both grammars that parse JSX (`.tsx`, and `.js`/`.jsx`) share the
    // `jsx_text` node, so both get the same override.
    macro_rules! jsx_override {
        ($name:literal, $lang:ident) => {{
            use arborium::$lang as grammar;
            let highlights = format!(
                "{}\n{}",
                &*grammar::HIGHLIGHTS_QUERY,
                TSX_HIGHLIGHT_OVERRIDES
            );
            match CompiledGrammar::new(GrammarConfig {
                language: grammar::language().into(),
                highlights_query: &highlights,
                injections_query: grammar::INJECTIONS_QUERY,
                locals_query: grammar::LOCALS_QUERY,
            }) {
                Ok(compiled) => {
                    store.insert($name, Arc::new(compiled));
                }
                // Keep the stock grammar rather than losing highlighting.
                Err(err) => log::warn!("[syntax] {} highlight override rejected: {err}", $name),
            }
        }};
    }
    jsx_override!("tsx", lang_tsx);
    jsx_override!("javascript", lang_javascript);
}

/// The grammar for a Markdown code fence's info string (` ```tsx `,
/// ` ```c-sharp `, ` ```dockerfile `): arborium's own name/extension table,
/// so every compiled grammar is reachable from a fence.
pub fn grammar_for_fence(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    arborium::detect_language(&format!("fence.{tag}"))
}

/// `highlight_document` with a small process-wide cache, for views that
/// rebuild their elements every frame (agent replies, previews) and would
/// otherwise re-parse the same snippet repeatedly.
pub fn highlight_document_cached(
    grammar: &'static str,
    text: &str,
) -> Option<Arc<DocumentHighlights>> {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::Mutex;

    type Cache = Mutex<HashMap<(&'static str, u64, usize), Option<Arc<DocumentHighlights>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key = (grammar, hasher.finish(), text.len());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let result = highlight_document(grammar, text).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(key, result.clone());
    }
    result
}

/// The grammar to highlight a file with. The path decides first, so every
/// grammar compiled into the app is used — not just the handful our
/// `Language` enum names — falling back to the enum for extension-less files.
pub fn grammar_for(path: Option<&Path>, language: Language) -> Option<&'static str> {
    path.and_then(Path::to_str)
        .and_then(arborium::detect_language)
        .or_else(|| path.and_then(grammar_for_file_name))
        .or_else(|| syntax_grammar_name(language))
}

/// Conventional file names with no (or a misleading) extension. arborium
/// maps extensions only, so `Makefile` would otherwise go unhighlighted.
fn grammar_for_file_name(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let alias = match name.as_str() {
        "makefile" | "gnumakefile" => "make",
        "cmakelists.txt" => "cmake",
        "justfile" | ".justfile" => "just",
        "dockerfile" | "containerfile" => "dockerfile",
        ".bashrc" | ".bash_profile" | ".profile" => "bash",
        ".zshrc" | ".zprofile" | ".zshenv" => "zsh",
        "gemfile" | "rakefile" | "podfile" => "ruby",
        "build" | "workspace" => "starlark",
        _ if name.starts_with("dockerfile.") => "dockerfile",
        _ => return None,
    };
    arborium::detect_language(&format!("file.{alias}"))
}

/// Per-line highlight spans: `lines[i]` holds line `i`'s spans as byte ranges
/// relative to the start of that line, sorted and non-overlapping.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentHighlights {
    pub lines: Vec<Vec<HighlightSpan>>,
}

/// Highlight a whole document. `None` when the grammar isn't compiled into
/// the app or parsing fails — callers fall back to the lexical highlighter.
pub fn highlight_document(grammar: &str, text: &str) -> Option<DocumentHighlights> {
    let mut highlighter = Highlighter::with_store(shared_store());
    let spans = highlighter.highlight_spans(grammar, text).ok()?;
    let tokens = arborium_highlight::spans_to_flat_tokens(text, spans);

    let mut line_starts = vec![0usize];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); line_starts.len()];

    for token in tokens {
        let Some(kind) = token_kind(token.tag) else {
            continue;
        };
        let (start, end) = (token.start as usize, token.end as usize);
        // A token can span lines (block comments, template strings).
        let mut line = line_starts
            .partition_point(|&s| s <= start)
            .saturating_sub(1);
        while line < line_starts.len() && line_starts[line] < end {
            let line_start = line_starts[line];
            let line_end = line_starts
                .get(line + 1)
                .map_or(text.len(), |&next| next - 1);
            let from = start.max(line_start);
            let to = end.min(line_end);
            if from < to {
                lines[line].push(HighlightSpan {
                    range: (from - line_start)..(to - line_start),
                    kind,
                });
            }
            line += 1;
        }
    }
    Some(DocumentHighlights { lines })
}

/// Theme-slot tag -> editor token color. `None` means plain text (variables,
/// JSX/markup text, embedded-region wrappers).
fn token_kind(tag: &str) -> Option<TokenKind> {
    let name = arborium_theme::tag_to_name(tag).unwrap_or(tag);
    Some(match name {
        "keyword" => TokenKind::Keyword,
        // Markup/JSX element names take the keyword color, as in VS Code.
        "tag" | "title" => TokenKind::Keyword,
        "function" => TokenKind::Function,
        "type" | "constructor" | "namespace" => TokenKind::Type,
        "string" | "literal" | "link" => TokenKind::String,
        "comment" => TokenKind::Comment,
        "number" => TokenKind::Number,
        "constant" | "label" => TokenKind::Constant,
        "operator" => TokenKind::Operator,
        "punctuation" => TokenKind::Delimiter,
        "property" | "attribute" => TokenKind::Property,
        "macro" => TokenKind::Macro,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_on(doc: &DocumentHighlights, text: &str, line: usize, word: &str) -> Vec<TokenKind> {
        let line_text = text.lines().nth(line).unwrap();
        let at = line_text.find(word).unwrap();
        doc.lines[line]
            .iter()
            .filter(|s| s.range.start < at + word.len() && at < s.range.end)
            .map(|s| s.kind)
            .collect()
    }

    /// Text between tags is prose in every markup grammar: never a type,
    /// string, number or keyword color — including text on its own line and
    /// JSX in plain `.js`/`.jsx` files.
    #[test]
    fn markup_text_is_plain_in_every_markup_grammar() {
        let cases: &[(&str, &str, usize, &str)] = &[
            (
                "tsx",
                "const A = () => (\n  <div>\n    Hey Hello World\n  </div>\n);\n",
                2,
                "Hello",
            ),
            (
                "tsx",
                "const A = () => <p>Hey Hello, it's 2 Real</p>;\n",
                0,
                "Hello",
            ),
            (
                "javascript",
                "const A = () => (\n  <div>\n    Hey Hello World\n  </div>\n);\n",
                2,
                "Hello",
            ),
            (
                "javascript",
                "const A = () => <p>Real, and fake</p>;\n",
                0,
                "Real",
            ),
            ("html", "<div>\n  Hey Hello World\n</div>\n", 1, "Hello"),
            ("html", "<p>Hey Hello</p>\n", 0, "Hello"),
            (
                "vue",
                "<template>\n  <p>Hey Hello World</p>\n</template>\n",
                1,
                "Hello",
            ),
            ("svelte", "<p>Hey Hello World</p>\n", 0, "Hello"),
            ("xml", "<note>\n  Hey Hello World\n</note>\n", 1, "Hello"),
        ];
        for (grammar, src, line, word) in cases {
            let doc = highlight_document(grammar, src)
                .unwrap_or_else(|| panic!("{grammar} grammar missing"));
            let kinds = kinds_on(&doc, src, *line, word);
            assert!(
                kinds.is_empty(),
                "{grammar}: {word} colored {kinds:?} in {src:?}"
            );
        }
    }

    #[test]
    fn css_values_are_not_types() {
        let src = ".a {\n  font-family: Inter, Arial;\n}\n";
        for grammar in ["css", "scss"] {
            let doc = highlight_document(grammar, src).expect("grammar compiled in");
            let kinds = kinds_on(&doc, src, 1, "Arial");
            assert!(!kinds.contains(&TokenKind::Type), "{grammar}: {kinds:?}");
        }
    }

    /// Every file type a user commonly opens resolves to a compiled grammar
    /// and actually produces highlights.
    #[test]
    fn common_languages_have_working_grammars() {
        let samples = [
            ("a.rs", "fn main() { let x = 1; }"),
            ("a.go", "package main\nfunc main() {}"),
            ("a.py", "def f():\n    return 1"),
            ("a.ts", "const x: number = 1;"),
            ("a.tsx", "const A = () => <p>x</p>;"),
            ("a.jsx", "const A = () => <p>x</p>;"),
            ("a.js", "function f() { return 1; }"),
            ("a.html", "<p class=\"x\">hi</p>"),
            ("a.css", ".a { color: red; }"),
            ("a.scss", ".a { .b { color: red; } }"),
            ("a.json", "{\"a\": 1}"),
            ("a.yaml", "a: 1"),
            ("a.toml", "a = 1"),
            ("a.sh", "echo hi"),
            ("a.java", "class A { void f() {} }"),
            ("a.c", "int main() { return 0; }"),
            ("a.cpp", "int main() { return 0; }"),
            ("a.cs", "class A { void F() {} }"),
            ("a.rb", "def f\n  1\nend"),
            ("a.php", "<?php function f() { return 1; }"),
            ("a.swift", "func f() -> Int { return 1 }"),
            ("a.kt", "fun f(): Int = 1"),
            ("a.lua", "local x = 1"),
            ("a.sql", "SELECT 1;"),
            ("a.md", "# Title\n\ntext"),
            ("a.dart", "void main() { var x = 1; }"),
            ("a.zig", "pub fn main() void {}"),
            ("a.vue", "<template><p>x</p></template>"),
            ("a.svelte", "<p>x</p>"),
            ("a.graphql", "query { a }"),
            ("a.proto", "syntax = \"proto3\";"),
            ("Makefile", "all:\n\techo hi"),
            ("Dockerfile", "FROM alpine\nRUN echo hi"),
            (".zshrc", "export A=1"),
            ("a.ex", "defmodule A do\nend"),
            ("a.scala", "object A { def f = 1 }"),
        ];
        let mut failures = Vec::new();
        for (path, src) in samples {
            let Some(grammar) = grammar_for(Some(Path::new(path)), Language::PlainText) else {
                failures.push(format!("{path}: no grammar detected"));
                continue;
            };
            match highlight_document(grammar, src) {
                None => failures.push(format!("{path}: {grammar} failed")),
                Some(doc) if doc.lines.iter().all(|l| l.is_empty()) => {
                    failures.push(format!("{path}: {grammar} produced no highlights"))
                }
                Some(_) => {}
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn fence_tags_resolve_to_grammars() {
        for (tag, grammar) in [
            ("tsx", "tsx"),
            ("rust", "rust"),
            ("rs", "rust"),
            ("py", "python"),
            ("bash", "bash"),
            ("go", "go"),
            ("html", "html"),
        ] {
            assert_eq!(grammar_for_fence(tag), Some(grammar), "{tag}");
        }
        assert_eq!(grammar_for_fence(""), None);
    }

    #[test]
    fn jsx_text_is_plain_foreground() {
        // The bug: a word lexer colored "Real" and ", and fake" differently.
        // JSX text is prose: one plain run, not word-by-word colors and not
        // the string color (see queries/jsx-overrides.scm).
        let src = "const App = () => <p className=\"x\">Real, and fake</p>;\n";
        let doc = highlight_document("tsx", src).expect("tsx grammar is compiled in");
        let at = src.find("Real").unwrap();
        let len = "Real, and fake".len();
        let covering: Vec<_> = doc.lines[0]
            .iter()
            .filter(|s| s.range.start < at + len && at < s.range.end)
            .collect();
        assert!(
            covering.is_empty(),
            "JSX text must be plain foreground, got {covering:?}"
        );
        assert_eq!(kinds_on(&doc, src, 0, "const"), vec![TokenKind::Keyword]);
        assert_eq!(kinds_on(&doc, src, 0, "\"x\""), vec![TokenKind::String]);
        assert_eq!(
            kinds_on(&doc, src, 0, "className"),
            vec![TokenKind::Property]
        );
    }

    #[test]
    fn jsx_text_is_plain_in_javascript_files_too() {
        let src = "const App = () => <p>Real, and fake</p>;\n";
        let doc = highlight_document("javascript", src).expect("javascript grammar is compiled in");
        let at = src.find("Real").unwrap();
        let len = "Real, and fake".len();
        assert!(
            doc.lines[0]
                .iter()
                .all(|s| !(s.range.start < at + len && at < s.range.end)),
            "JSX text in .jsx must be plain, got {:?}",
            doc.lines[0]
        );
    }

    #[test]
    fn multi_line_tokens_are_split_per_line() {
        let src = "/* one\n   two */\nfn main() {}\n";
        let doc = highlight_document("rust", src).unwrap();
        assert_eq!(
            doc.lines[0],
            vec![HighlightSpan {
                range: 0..6,
                kind: TokenKind::Comment
            }]
        );
        assert_eq!(
            doc.lines[1],
            vec![HighlightSpan {
                range: 0..9,
                kind: TokenKind::Comment
            }]
        );
        assert_eq!(kinds_on(&doc, src, 2, "fn"), vec![TokenKind::Keyword]);
    }

    #[test]
    fn grammar_comes_from_the_path_for_any_compiled_language() {
        assert_eq!(
            grammar_for(Some(Path::new("App.tsx")), Language::TypeScript),
            Some("tsx")
        );
        assert_eq!(
            grammar_for(Some(Path::new("Main.java")), Language::PlainText),
            Some("java")
        );
        assert_eq!(grammar_for(None, Language::Rust), Some("rust"));
    }

    #[test]
    fn unknown_grammar_falls_back() {
        assert!(highlight_document("not-a-language", "x").is_none());
    }
}
