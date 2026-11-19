//! Declarative language and language-server catalog.
//!
//! Languages describe editor behavior. Servers are independent identities that
//! can serve several languages, and a document may attach to several servers.
//! Native process lifecycle and installation remain outside this UI-independent
//! crate so the same catalog can be rendered by native and web surfaces.

use std::path::Path;

use crate::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LanguageCapability {
    Completion,
    Diagnostics,
    Hover,
    SignatureHelp,
    Definition,
    Declaration,
    References,
    Implementation,
    DocumentSymbols,
    WorkspaceSymbols,
    Rename,
    CodeActions,
    Formatting,
    SemanticTokens,
    InlayHints,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerCandidate {
    pub executable: &'static str,
    pub args: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageServerRole {
    Primary,
    Supplementary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationRule {
    Always,
    ProjectEvidence {
        files: &'static [&'static str],
        packages: &'static [&'static str],
        content: &'static [&'static str],
    },
}

impl ActivationRule {
    fn matches(self, project_root: &Path, document_text: &str) -> bool {
        match self {
            Self::Always => true,
            Self::ProjectEvidence {
                files,
                packages,
                content,
            } => {
                files.iter().any(|path| project_root.join(path).exists())
                    || package_json_contains(project_root, packages)
                    || content.iter().any(|needle| document_text.contains(needle))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageServerDefinition {
    pub id: &'static str,
    pub display_name: &'static str,
    pub role: LanguageServerRole,
    pub languages: &'static [Language],
    pub candidates: &'static [ServerCandidate],
    pub managed_install: Option<&'static str>,
    pub enabled_by_default: bool,
    pub activation: ActivationRule,
}

impl LanguageServerDefinition {
    pub fn supports_language(self, language: Language) -> bool {
        self.languages.contains(&language)
    }

    pub fn applies(self, language: Language, project_root: &Path, document_text: &str) -> bool {
        self.enabled_by_default
            && self.supports_language(language)
            && self.activation.matches(project_root, document_text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageServiceDefinition {
    pub language: Language,
    pub language_id: &'static str,
    pub project_markers: &'static [&'static str],
    pub capabilities: &'static [LanguageCapability],
}

impl LanguageServiceDefinition {
    pub fn supports(self, capability: LanguageCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn matches_project(self, root: &Path) -> bool {
        self.project_markers
            .iter()
            .any(|marker| root.join(marker).exists())
    }
}

pub struct LanguageServiceRegistry;

impl LanguageServiceRegistry {
    pub fn definition(language: Language) -> Option<&'static LanguageServiceDefinition> {
        LANGUAGES
            .iter()
            .find(|definition| definition.language == language)
    }

    pub fn for_path(path: &Path) -> Option<&'static LanguageServiceDefinition> {
        Self::definition(Language::from_path(path))
    }

    pub fn definitions() -> &'static [LanguageServiceDefinition] {
        LANGUAGES
    }

    pub fn server(id: &str) -> Option<&'static LanguageServerDefinition> {
        SERVERS.iter().find(|server| server.id == id)
    }

    pub fn servers() -> &'static [LanguageServerDefinition] {
        SERVERS
    }

    pub fn applicable_servers(
        language: Language,
        project_root: &Path,
        document_text: &str,
    ) -> Vec<&'static LanguageServerDefinition> {
        SERVERS
            .iter()
            .filter(|server| server.applies(language, project_root, document_text))
            .collect()
    }
}

fn package_json_contains(project_root: &Path, packages: &[&str]) -> bool {
    if packages.is_empty() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(project_root.join("package.json")) else {
        return false;
    };
    packages
        .iter()
        .any(|package| text.contains(&format!("\"{package}\"")))
}

use LanguageCapability as C;

const FULL_CODE: &[LanguageCapability] = &[
    C::Completion,
    C::Diagnostics,
    C::Hover,
    C::SignatureHelp,
    C::Definition,
    C::Declaration,
    C::References,
    C::Implementation,
    C::DocumentSymbols,
    C::WorkspaceSymbols,
    C::Rename,
    C::CodeActions,
    C::Formatting,
    C::SemanticTokens,
    C::InlayHints,
];
const DATA_LANGUAGE: &[LanguageCapability] = &[
    C::Completion,
    C::Diagnostics,
    C::Hover,
    C::DocumentSymbols,
    C::Formatting,
];
const MARKUP_LANGUAGE: &[LanguageCapability] = &[
    C::Completion,
    C::Diagnostics,
    C::Hover,
    C::Definition,
    C::DocumentSymbols,
    C::Formatting,
];

const WEB_CODE: &[Language] = &[Language::TypeScript, Language::JavaScript];
const TAILWIND_LANGUAGES: &[Language] = &[
    Language::TypeScript,
    Language::JavaScript,
    Language::Html,
    Language::Css,
];
const ESLINT_LANGUAGES: &[Language] = &[
    Language::TypeScript,
    Language::JavaScript,
    Language::Html,
    Language::Css,
    Language::Json,
    Language::Markdown,
];

const SERVERS: &[LanguageServerDefinition] = &[
    server(
        "rust-analyzer",
        "Rust Analyzer",
        LanguageServerRole::Primary,
        &[Language::Rust],
        &[candidate("rust-analyzer", &[])],
        Some("rust-analyzer"),
        true,
        ActivationRule::Always,
    ),
    server(
        "vtsls",
        "vtsls",
        LanguageServerRole::Primary,
        WEB_CODE,
        &[candidate("vtsls", &["--stdio"])],
        Some("vtsls"),
        true,
        ActivationRule::Always,
    ),
    server(
        "eslint",
        "ESLint",
        LanguageServerRole::Supplementary,
        ESLINT_LANGUAGES,
        &[candidate("vscode-eslint-language-server", &["--stdio"])],
        Some("vscode-extracted"),
        true,
        ActivationRule::ProjectEvidence {
            files: &[
                "eslint.config.js",
                "eslint.config.mjs",
                "eslint.config.cjs",
                "eslint.config.ts",
                ".eslintrc",
                ".eslintrc.js",
                ".eslintrc.json",
            ],
            packages: &["eslint"],
            content: &[],
        },
    ),
    server(
        "tailwindcss",
        "Tailwind CSS",
        LanguageServerRole::Supplementary,
        TAILWIND_LANGUAGES,
        &[candidate("tailwindcss-language-server", &["--stdio"])],
        Some("tailwindcss"),
        true,
        ActivationRule::ProjectEvidence {
            files: &[
                "tailwind.config.js",
                "tailwind.config.cjs",
                "tailwind.config.mjs",
                "tailwind.config.ts",
                "tailwind.config.cts",
                "tailwind.config.mts",
            ],
            packages: &["tailwindcss"],
            content: &[
                "@import \"tailwindcss\"",
                "@import 'tailwindcss'",
                "@tailwind",
                "@config",
            ],
        },
    ),
    server(
        "basedpyright",
        "basedpyright",
        LanguageServerRole::Primary,
        &[Language::Python],
        &[
            candidate("basedpyright-langserver", &["--stdio"]),
            candidate("pyright-langserver", &["--stdio"]),
        ],
        Some("basedpyright"),
        true,
        ActivationRule::Always,
    ),
    server(
        "gopls",
        "gopls",
        LanguageServerRole::Primary,
        &[Language::Go],
        &[candidate("gopls", &[])],
        Some("gopls"),
        true,
        ActivationRule::Always,
    ),
    server(
        "html",
        "HTML Language Server",
        LanguageServerRole::Primary,
        &[Language::Html],
        &[candidate("vscode-html-language-server", &["--stdio"])],
        Some("vscode-extracted"),
        true,
        ActivationRule::Always,
    ),
    server(
        "css",
        "CSS Language Server",
        LanguageServerRole::Primary,
        &[Language::Css],
        &[candidate("vscode-css-language-server", &["--stdio"])],
        Some("vscode-extracted"),
        true,
        ActivationRule::Always,
    ),
    server(
        "json",
        "JSON Language Server",
        LanguageServerRole::Primary,
        &[Language::Json],
        &[candidate("vscode-json-language-server", &["--stdio"])],
        Some("vscode-extracted"),
        true,
        ActivationRule::Always,
    ),
    server(
        "markdown",
        "Marksman",
        LanguageServerRole::Primary,
        &[Language::Markdown],
        &[candidate("marksman", &["server"])],
        Some("marksman"),
        true,
        ActivationRule::Always,
    ),
    server(
        "yaml",
        "YAML Language Server",
        LanguageServerRole::Primary,
        &[Language::Yaml],
        &[candidate("yaml-language-server", &["--stdio"])],
        Some("yaml"),
        true,
        ActivationRule::Always,
    ),
    server(
        "terraform-ls",
        "Terraform Language Server",
        LanguageServerRole::Primary,
        &[Language::Hcl],
        &[candidate("terraform-ls", &["serve"])],
        Some("terraform-ls"),
        true,
        ActivationRule::Always,
    ),
    server(
        "bash",
        "Bash Language Server",
        LanguageServerRole::Primary,
        &[Language::Shell],
        &[candidate("bash-language-server", &["start"])],
        Some("bash"),
        true,
        ActivationRule::Always,
    ),
];

const fn candidate(executable: &'static str, args: &'static [&'static str]) -> ServerCandidate {
    ServerCandidate { executable, args }
}

const fn server(
    id: &'static str,
    display_name: &'static str,
    role: LanguageServerRole,
    languages: &'static [Language],
    candidates: &'static [ServerCandidate],
    managed_install: Option<&'static str>,
    enabled_by_default: bool,
    activation: ActivationRule,
) -> LanguageServerDefinition {
    LanguageServerDefinition {
        id,
        display_name,
        role,
        languages,
        candidates,
        managed_install,
        enabled_by_default,
        activation,
    }
}

const LANGUAGES: &[LanguageServiceDefinition] = &[
    language(
        Language::Rust,
        "rust",
        &["Cargo.toml", "rust-project.json"],
        FULL_CODE,
    ),
    language(
        Language::TypeScript,
        "typescriptreact",
        &["tsconfig.json", "jsconfig.json", "package.json"],
        FULL_CODE,
    ),
    language(
        Language::JavaScript,
        "javascriptreact",
        &["jsconfig.json", "tsconfig.json", "package.json"],
        FULL_CODE,
    ),
    language(
        Language::Python,
        "python",
        &["pyproject.toml", "requirements.txt", "setup.py"],
        FULL_CODE,
    ),
    language(Language::Go, "go", &["go.mod", "go.work"], FULL_CODE),
    language(Language::Html, "html", &["package.json"], MARKUP_LANGUAGE),
    language(Language::Css, "css", &["package.json"], MARKUP_LANGUAGE),
    language(Language::Json, "jsonc", &["package.json"], DATA_LANGUAGE),
    language(
        Language::Yaml,
        "yaml",
        &[".yamllint", "docker-compose.yml"],
        DATA_LANGUAGE,
    ),
    language(
        Language::Hcl,
        "terraform",
        &[".terraform.lock.hcl", ".terraform", "*.tf"],
        FULL_CODE,
    ),
    language(
        Language::Shell,
        "shellscript",
        &[".shellcheckrc", "Makefile"],
        FULL_CODE,
    ),
    language(
        Language::Markdown,
        "markdown",
        &["README.md", ".marksman.toml"],
        MARKUP_LANGUAGE,
    ),
];

const fn language(
    language: Language,
    language_id: &'static str,
    project_markers: &'static [&'static str],
    capabilities: &'static [LanguageCapability],
) -> LanguageServiceDefinition {
    LanguageServiceDefinition {
        language,
        language_id,
        project_markers,
        capabilities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_multiple_servers_for_tailwind_typescript() {
        let root =
            std::env::temp_dir().join(format!("dragent-language-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"tailwindcss":"latest","eslint":"latest"}}"#,
        )
        .unwrap();
        let servers = LanguageServiceRegistry::applicable_servers(Language::TypeScript, &root, "");
        let ids = servers.iter().map(|server| server.id).collect::<Vec<_>>();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(ids, vec!["vtsls", "eslint", "tailwindcss"]);
    }

    #[test]
    fn alternate_primary_is_not_started_by_default() {
        let servers = LanguageServiceRegistry::applicable_servers(
            Language::TypeScript,
            Path::new("/missing"),
            "",
        );
        assert_eq!(
            servers.iter().map(|server| server.id).collect::<Vec<_>>(),
            vec!["vtsls"]
        );
    }

    #[test]
    fn plain_text_has_no_language_profile() {
        assert!(LanguageServiceRegistry::for_path(Path::new("LICENSE")).is_none());
    }

    #[test]
    fn every_language_profile_has_a_primary_server() {
        for language in LanguageServiceRegistry::definitions() {
            assert!(
                LanguageServiceRegistry::servers().iter().any(|server| {
                    server.role == LanguageServerRole::Primary
                        && server.supports_language(language.language)
                }),
                "{:?} advertises language services without a primary server",
                language.language
            );
        }
    }

    #[test]
    fn enabled_built_in_servers_are_actually_installable() {
        for server in LanguageServiceRegistry::servers()
            .iter()
            .filter(|server| server.enabled_by_default)
        {
            assert!(
                server.managed_install.is_some(),
                "{} is enabled but has no managed installer",
                server.id
            );
        }
    }
}
