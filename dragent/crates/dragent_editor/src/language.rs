use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Json,
    Html,
    Css,
    Markdown,
    Shell,
    Yaml,
    Toml,
    Hcl,
    #[default]
    PlainText,
}

impl Language {
    /// Compact language marker used by file-tree surfaces. These labels mirror
    /// the familiar file-type affordance without coupling the
    /// editor domain to a particular icon renderer.
    pub fn icon_label(self) -> &'static str {
        match self {
            Self::Json => "{}",
            Self::TypeScript => "TS",
            Self::JavaScript => "JS",
            Self::Rust => "R",
            Self::Python => "Py",
            Self::Go => "Go",
            Self::Html => "<>",
            Self::Css => "#",
            Self::Markdown => "M",
            Self::Shell => "$",
            Self::Yaml => "Y",
            Self::Toml => "T",
            Self::Hcl => "HCL",
            Self::PlainText => "·",
        }
    }

    /// Material Icon Theme asset selected for this language.
    pub fn icon_asset(self) -> &'static str {
        match self {
            Self::Rust => "icons/rust.svg",
            Self::TypeScript => "icons/typescript.svg",
            Self::JavaScript => "icons/javascript.svg",
            Self::Json => "icons/json.svg",
            Self::Python => "icons/python.svg",
            Self::Go => "icons/go.svg",
            Self::Html => "icons/html.svg",
            Self::Css => "icons/css.svg",
            Self::Markdown => "icons/markdown.svg",
            Self::Shell => "icons/shellcheck.svg",
            Self::Yaml => "icons/yaml.svg",
            Self::Toml => "icons/toml.svg",
            Self::Hcl => "icons/terraform.svg",
            Self::PlainText => "icons/file.svg",
        }
    }

    /// Canonical formatter executable and arguments for this language. The
    /// caller must still check availability and run it off the UI thread.
    pub fn formatter(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            Self::Rust => Some(("rustfmt", &["--emit", "stdout"][..])),
            Self::TypeScript | Self::JavaScript | Self::Json | Self::Css | Self::Markdown => {
                Some(("prettier", &["--stdin-filepath"][..]))
            }
            Self::Python => Some(("black", &["-"][..])),
            Self::Go => Some(("gofmt", &[][..])),
            Self::Shell => Some(("shfmt", &["-"][..])),
            Self::Yaml | Self::Toml | Self::Hcl | Self::Html | Self::PlainText => None,
        }
    }

    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        match extension.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "go" => Self::Go,
            "json" | "jsonc" => Self::Json,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "md" | "mdx" => Self::Markdown,
            "sh" | "bash" | "zsh" | "fish" => Self::Shell,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "hcl" | "tf" | "tfvars" => Self::Hcl,
            _ if name == "Cargo.lock" => Self::Toml,
            _ if matches!(name, "Dockerfile" | "Makefile") => Self::Shell,
            _ => Self::PlainText,
        }
    }

    pub fn supports_breakpoints(self) -> bool {
        matches!(
            self,
            Self::Rust
                | Self::TypeScript
                | Self::JavaScript
                | Self::Python
                | Self::Go
                | Self::Shell
        )
    }

    /// Comment syntax for the toggle-comment editor command (Cmd+/ / Ctrl+/).
    /// `None` for languages with no comment syntax that's safe to insert
    /// (plain JSON has none — inserting `//` breaks strict parsers — and
    /// plain text has no notion of "code" to comment out).
    pub fn comment_style(self) -> Option<CommentStyle> {
        match self {
            Self::Rust | Self::TypeScript | Self::JavaScript | Self::Go => {
                Some(CommentStyle::Line("//"))
            }
            Self::Python | Self::Shell | Self::Yaml | Self::Toml | Self::Hcl => {
                Some(CommentStyle::Line("#"))
            }
            Self::Css => Some(CommentStyle::Block("/*", "*/")),
            Self::Html | Self::Markdown => Some(CommentStyle::Block("<!--", "-->")),
            Self::Json | Self::PlainText => None,
        }
    }
}

/// How a language spells "comment out this code" for the toggle-comment
/// command. `Line` prefixes every touched line independently (the common
/// case for real code); `Block` wraps the whole touched span in one pair of
/// delimiters (languages, like CSS/HTML, with no per-line comment token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentStyle {
    Line(&'static str),
    Block(&'static str, &'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_source_paths() {
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")),
            Language::Rust
        );
        assert_eq!(
            Language::from_path(Path::new("app/page.tsx")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("README.md")),
            Language::Markdown
        );
        assert_eq!(
            Language::from_path(Path::new("infra/main.tf")),
            Language::Hcl
        );
        assert_eq!(Language::from_path(Path::new("Cargo.lock")), Language::Toml);
    }

    #[test]
    fn exposes_editor_file_markers_and_formatters() {
        assert_eq!(
            Language::from_path(Path::new("config.json")).icon_label(),
            "{}"
        );
        assert_eq!(
            Language::from_path(Path::new("src/app.tsx")).icon_label(),
            "TS"
        );
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")).formatter(),
            Some(("rustfmt", &["--emit", "stdout"][..]))
        );
        assert_eq!(
            Language::from_path(Path::new("README.md")).formatter(),
            Some(("prettier", &["--stdin-filepath"][..]))
        );
    }

    #[test]
    fn comment_style_matches_each_language_own_syntax() {
        assert_eq!(
            Language::Rust.comment_style(),
            Some(CommentStyle::Line("//"))
        );
        assert_eq!(Language::Go.comment_style(), Some(CommentStyle::Line("//")));
        assert_eq!(
            Language::Python.comment_style(),
            Some(CommentStyle::Line("#"))
        );
        assert_eq!(
            Language::Toml.comment_style(),
            Some(CommentStyle::Line("#"))
        );
        assert_eq!(
            Language::Css.comment_style(),
            Some(CommentStyle::Block("/*", "*/"))
        );
        assert_eq!(
            Language::Html.comment_style(),
            Some(CommentStyle::Block("<!--", "-->"))
        );
        // Strict JSON has no comment syntax to insert without breaking the
        // file, and plain text has no "code" to comment out.
        assert_eq!(Language::Json.comment_style(), None);
        assert_eq!(Language::PlainText.comment_style(), None);
    }
}
