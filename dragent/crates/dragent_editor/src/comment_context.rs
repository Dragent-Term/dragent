//! Comment syntax resolved from the real parsed structure of the document,
//! not a single fixed answer per file.
//!
//! A `.tsx` file is mostly TypeScript (`//`), but a JSX element's text
//! children can't take a line comment — it isn't valid syntax there — and
//! need `{/* */}` instead; inside a `{expression}` within that same JSX,
//! plain `//` is valid again. An HTML `<script>`/`<style>` body comments
//! like its own embedded language, not like HTML's `<!-- -->`. A Markdown
//! fenced code block (` ```rust `) comments like whatever language its fence
//! declares, not like Markdown. Three different embedding mechanisms, so
//! three different real parsers, matching whichever one Dragent already
//! uses for that language elsewhere rather than one bespoke notion of "what
//! language am I in": the same `arborium` (tree-sitter) grammars
//! `folding.rs` parses TSX/HTML with for fold regions, and the same
//! `pulldown-cmark` parser `dragent_viewer`'s markdown renderer uses to find
//! a fence's declared language for syntax highlighting.
//!
//! Deliberately a fresh, synchronous, uncached parse each call: `InputEditor`
//! also keeps a `FoldParser` with a live tree-sitter tree, but it reparses
//! asynchronously (see `schedule_fold_refresh`'s "must never become a hidden
//! Tree-sitter entry point" note) and can briefly lag the buffer right after
//! an edit. Toggling a comment is a deliberate, infrequent keystroke, not a
//! hot per-render path, so paying for one extra parse here to be sure it's
//! answering against the *current* text is the right trade.

use crate::folding::syntax_grammar_name;
use crate::language::CommentStyle;
use crate::Language;
use arborium::tree_sitter::{Node, Parser};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser as MarkdownParser, Tag, TagEnd};
use std::path::Path;

/// Comment syntax that applies at `byte_position` in `text`. Falls back to
/// `language`'s single fixed answer (`Language::comment_style`) whenever
/// there's no parser available for this language, parsing fails, or the
/// language has no region-dependent comment rules to begin with.
pub fn comment_style_at(
    language: Language,
    text: &str,
    byte_position: usize,
) -> Option<CommentStyle> {
    let default = language.comment_style();
    if language == Language::Markdown {
        return markdown_comment_style(text, byte_position).or(default);
    }
    let Some(grammar_name) = syntax_grammar_name(language) else {
        return default;
    };
    let Some(grammar) = arborium::get_language(grammar_name) else {
        return default;
    };
    let mut parser = Parser::new();
    if parser.set_language(&grammar).is_err() {
        return default;
    }
    let Some(tree) = parser.parse(text, None) else {
        return default;
    };
    let byte_position = byte_position.min(text.len());
    let Some(node) = tree
        .root_node()
        .descendant_for_byte_range(byte_position, byte_position)
    else {
        return default;
    };

    let resolved = match language {
        Language::TypeScript | Language::JavaScript => jsx_comment_style(node),
        Language::Html => html_embedded_comment_style(node),
        _ => None,
    };
    resolved.or(default)
}

/// Walk up from `node` to the first ancestor whose tree-sitter node kind
/// decides the answer. `jsx_expression` (`{...}` inside JSX) is always
/// encountered before the `jsx_element`/`jsx_fragment` it lives in, so
/// checking kinds top-down as we ascend is sufic ient — there's no case
/// where the "wrong" one would be seen first. Public for `comment_style_at`
/// but also unit-tested directly for the specific kind ordering it relies on.
fn jsx_comment_style(node: Node<'_>) -> Option<CommentStyle> {
    let mut current = Some(node);
    while let Some(n) = current {
        match n.kind() {
            // Back in a JS/TS expression, even though lexically nested
            // inside JSX — plain `//` is valid here.
            "jsx_expression" => return Some(CommentStyle::Line("//")),
            // Direct markup position: text content, or landing on the
            // element/fragment node itself (a "gap" not owned by a more
            // specific child, e.g. between two children).
            "jsx_text" | "jsx_element" | "jsx_fragment" => {
                return Some(CommentStyle::Block("{/*", "*/}"));
            }
            // Still inside the tag syntax itself (attributes, tag name) —
            // not markup-children position yet.
            "jsx_opening_element" | "jsx_closing_element" | "jsx_attribute" => {
                return Some(CommentStyle::Line("//"));
            }
            _ => {}
        }
        current = n.parent();
    }
    None
}

/// Walk up from `node` to the nearest enclosing `<script>`/`<style>` tag, if
/// any — nested `{`/`(`/`[` inside either body (object literals, function
/// calls, CSS selectors) are still that body's own language, so the
/// enclosing *tag*, not the innermost delimiter, decides this.
fn html_embedded_comment_style(node: Node<'_>) -> Option<CommentStyle> {
    let mut current = Some(node);
    while let Some(n) = current {
        match n.kind() {
            "script_element" => return Some(CommentStyle::Line("//")),
            "style_element" => return Some(CommentStyle::Block("/*", "*/")),
            _ => {}
        }
        current = n.parent();
    }
    None
}

/// `arborium` has no Markdown grammar in this build, so this asks a real
/// CommonMark parser (`pulldown-cmark`, the same one `dragent_viewer`'s
/// renderer already uses to find a fence's declared language for syntax
/// highlighting — see `markdown_code_language` there) which fenced code
/// block, if any, contains `byte_position`, then recurses into that fence's
/// own language. Recursing (not just returning a flat style) means a JSX
/// fence inside a Markdown file still gets full JSX-aware treatment, for
/// free — the same function answering both questions.
fn markdown_comment_style(text: &str, byte_position: usize) -> Option<CommentStyle> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let mut fence_language: Option<String> = None;
    for (event, range) in MarkdownParser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                fence_language = Some(
                    info.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            Event::End(TagEnd::CodeBlock) => fence_language = None,
            _ => {}
        }
        if range.contains(&byte_position) {
            let language = fence_comment_context_language(fence_language.as_deref()?)?;
            // A fence's own byte range is relative to the *same* document,
            // so no offset translation is needed to recurse.
            return comment_style_at(language, text, byte_position);
        }
    }
    None
}

/// The `Language` a fenced code block's info string (`js` in ` ```js `)
/// resolves to for comment purposes — mirrors
/// `dragent_viewer::terminal::render::markdown_tables::markdown_code_language`,
/// which does the same lookup for syntax-highlighting an identical fence;
/// kept in sync by hand since `dragent_editor` cannot depend on that crate
/// (dependency direction runs the other way).
fn fence_comment_context_language(tag: &str) -> Option<Language> {
    if tag.is_empty() {
        return None;
    }
    Some(match tag.to_ascii_lowercase().as_str() {
        // A markdown-labeled fence nested in markdown has nothing more
        // specific to resolve to than the flat Markdown default already is
        // — returning `Language::Markdown` here would recurse straight back
        // into this same function forever.
        "md" | "markdown" => return None,
        "rs" | "rust" => Language::Rust,
        "ts" | "tsx" | "typescript" => Language::TypeScript,
        "js" | "jsx" | "javascript" => Language::JavaScript,
        "py" | "python" => Language::Python,
        "go" | "golang" => Language::Go,
        "json" | "jsonc" | "json5" => Language::Json,
        "html" | "htm" => Language::Html,
        "css" | "scss" | "sass" | "less" => Language::Css,
        "sh" | "bash" | "zsh" | "shell" => Language::Shell,
        "yaml" | "yml" => Language::Yaml,
        "toml" => Language::Toml,
        "hcl" | "terraform" | "tf" => Language::Hcl,
        other => Language::from_path(Path::new(&format!("fence.{other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_typescript_uses_line_comments() {
        let text = "const x = 1;";
        assert_eq!(
            comment_style_at(Language::TypeScript, text, text.len()),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn jsx_markup_children_need_a_block_comment() {
        let text = "function App() {\n  return <div>\n    hello\n  </div>;\n}";
        let position = text.find("hello").unwrap();
        assert_eq!(
            comment_style_at(Language::TypeScript, text, position),
            Some(CommentStyle::Block("{/*", "*/}"))
        );
    }

    #[test]
    fn jsx_expression_container_is_still_plain_javascript() {
        // Inside `{ }` nested within JSX children — back in JS/TS, `//` is
        // valid again even though we're lexically inside <div>.
        let text = "const el = <div>{\n  value\n}</div>;";
        let position = text.find("value").unwrap();
        assert_eq!(
            comment_style_at(Language::TypeScript, text, position),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn jsx_attribute_position_before_the_tag_closes_is_plain_code() {
        let text = "const el = <div className=\"card\">{}</div>;";
        let position = text.find("className").unwrap();
        assert_eq!(
            comment_style_at(Language::TypeScript, text, position),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn plain_javascript_jsx_matches_typescript() {
        let text = "const el = <span>hi</span>;";
        let position = text.find("hi").unwrap();
        assert_eq!(
            comment_style_at(Language::JavaScript, text, position),
            Some(CommentStyle::Block("{/*", "*/}"))
        );
    }

    #[test]
    fn html_script_body_comments_like_javascript() {
        let text = "<html>\n<script>\n  const x = 1;\n</script>\n</html>";
        let position = text.find("const x").unwrap();
        assert_eq!(
            comment_style_at(Language::Html, text, position),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn html_style_body_comments_like_css() {
        let text = "<html>\n<style>\n  .card { color: red; }\n</style>\n</html>";
        let position = text.find(".card").unwrap();
        assert_eq!(
            comment_style_at(Language::Html, text, position),
            Some(CommentStyle::Block("/*", "*/"))
        );
    }

    #[test]
    fn html_script_body_nested_braces_stay_javascript() {
        // A nested object literal inside <script> must not be mistaken for
        // "no longer inside script" — the enclosing *tag*, not the innermost
        // delimiter, decides the language here (unlike the JSX case).
        let text = "<script>\n  const cfg = { a: 1 };\n  const y = 2;\n</script>";
        let position = text.find("a: 1").unwrap();
        assert_eq!(
            comment_style_at(Language::Html, text, position),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn plain_html_markup_uses_html_comments() {
        let text = "<html>\n<body>\n  <p>hello</p>\n</body>\n</html>";
        let position = text.find("hello").unwrap();
        assert_eq!(
            comment_style_at(Language::Html, text, position),
            Some(CommentStyle::Block("<!--", "-->"))
        );
    }

    #[test]
    fn languages_without_a_parser_use_the_flat_default() {
        assert_eq!(
            comment_style_at(Language::Rust, "fn main() {}", 3),
            Some(CommentStyle::Line("//"))
        );
        assert_eq!(comment_style_at(Language::Json, "{}", 1), None);
    }

    #[test]
    fn markdown_prose_uses_html_style_comments() {
        let text = "# Title\n\nSome prose here.\n";
        let position = text.find("prose").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Block("<!--", "-->"))
        );
    }

    #[test]
    fn markdown_fenced_rust_block_comments_like_rust() {
        let text = "# Title\n\n```rust\nlet x = 1;\n```\n\nMore prose.\n";
        let position = text.find("let x").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Line("//"))
        );
    }

    #[test]
    fn markdown_fenced_css_block_comments_like_css() {
        let text = "```css\n.card { color: red; }\n```\n";
        let position = text.find(".card").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Block("/*", "*/"))
        );
    }

    #[test]
    fn markdown_fenced_tsx_block_is_jsx_aware() {
        // The fence-language cascade must recurse into full tree-sitter-based
        // resolution, not just a flat per-fence answer: JSX inside a ```tsx
        // fence inside a .md file still needs `{/* */}` in its markup.
        let text = "````tsx\nconst el = <div>\n  hello\n</div>;\n````\n";
        let position = text.find("hello").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Block("{/*", "*/}"))
        );
    }

    #[test]
    fn markdown_fence_labeled_markdown_does_not_recurse_forever() {
        let text = "````markdown\nSome nested markdown.\n````\n";
        let position = text.find("nested").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Block("<!--", "-->"))
        );
    }

    #[test]
    fn markdown_unfenced_after_a_code_block_stays_prose() {
        let text = "```js\nconst x = 1;\n```\n\nBack to prose.\n";
        let position = text.find("Back to prose").unwrap();
        assert_eq!(
            comment_style_at(Language::Markdown, text, position),
            Some(CommentStyle::Block("<!--", "-->"))
        );
    }
}
