//! Runtime language-server catalog.
//!
//! Built-in definitions are merged with user-installed JSON manifests. The
//! manifest format is deliberately data-only: it can describe discovery,
//! activation and a bounded npm install, but cannot inject shell commands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dragent_editor::{ActivationRule, Language, LanguageServerRole, LanguageServiceRegistry};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogCandidate {
    pub executable: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogInstall {
    BuiltIn(String),
    Npm { packages: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct CatalogServer {
    pub id: String,
    pub display_name: String,
    pub role: LanguageServerRole,
    pub languages: Vec<Language>,
    pub candidates: Vec<CatalogCandidate>,
    pub install: Option<CatalogInstall>,
    pub enabled_by_default: bool,
    activation: CatalogActivation,
}

impl CatalogServer {
    pub fn supports_language(&self, language: Language) -> bool {
        self.languages.contains(&language)
    }

    pub fn applies(&self, language: Language, root: &Path, text: &str) -> bool {
        self.enabled_by_default
            && self.supports_language(language)
            && self.activation.matches(root, text)
    }
}

#[derive(Clone, Debug, Default)]
struct CatalogActivation {
    files: Vec<String>,
    packages: Vec<String>,
    content: Vec<String>,
    always: bool,
}

impl CatalogActivation {
    fn matches(&self, root: &Path, text: &str) -> bool {
        self.always
            || self.files.iter().any(|path| root.join(path).exists())
            || package_json_contains(root, &self.packages)
            || self.content.iter().any(|needle| text.contains(needle))
    }
}

#[derive(Clone, Debug)]
pub struct LanguageServerCatalog {
    servers: HashMap<String, CatalogServer>,
}

impl Default for LanguageServerCatalog {
    fn default() -> Self {
        Self::load()
    }
}

impl LanguageServerCatalog {
    pub fn load() -> Self {
        let mut catalog = Self::built_in();
        for path in manifest_paths() {
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_json::from_str::<ManifestFile>(&text).map_err(|error| error.to_string())
                }) {
                Ok(file) => {
                    for manifest in file.into_servers() {
                        match CatalogServer::try_from(manifest) {
                            Ok(server) => {
                                catalog.servers.insert(server.id.clone(), server);
                            }
                            Err(reason) => log_manifest_error(&path, &reason),
                        }
                    }
                }
                Err(reason) => log_manifest_error(&path, &reason),
            }
        }
        catalog
    }

    pub fn built_in() -> Self {
        let servers = LanguageServiceRegistry::servers()
            .iter()
            .map(|server| {
                let activation = match server.activation {
                    ActivationRule::Always => CatalogActivation {
                        always: true,
                        ..CatalogActivation::default()
                    },
                    ActivationRule::ProjectEvidence {
                        files,
                        packages,
                        content,
                    } => CatalogActivation {
                        files: strings(files),
                        packages: strings(packages),
                        content: strings(content),
                        always: false,
                    },
                };
                let server = CatalogServer {
                    id: server.id.to_string(),
                    display_name: server.display_name.to_string(),
                    role: server.role,
                    languages: server.languages.to_vec(),
                    candidates: server
                        .candidates
                        .iter()
                        .map(|candidate| CatalogCandidate {
                            executable: candidate.executable.to_string(),
                            args: strings(candidate.args),
                        })
                        .collect(),
                    install: server
                        .managed_install
                        .map(|id| CatalogInstall::BuiltIn(id.to_string())),
                    enabled_by_default: server.enabled_by_default,
                    activation,
                };
                (server.id.clone(), server)
            })
            .collect();
        Self { servers }
    }

    pub fn server(&self, id: &str) -> Option<&CatalogServer> {
        self.servers.get(id)
    }

    pub fn servers_for_language(&self, language: Language) -> Vec<&CatalogServer> {
        let mut servers = self
            .servers
            .values()
            .filter(|server| server.supports_language(language))
            .collect::<Vec<_>>();
        servers.sort_by_key(|server| {
            (
                server.role != LanguageServerRole::Primary,
                server.display_name.clone(),
            )
        });
        servers
    }

    pub fn applicable_servers(
        &self,
        language: Language,
        root: &Path,
        text: &str,
    ) -> Vec<&CatalogServer> {
        self.servers_for_language(language)
            .into_iter()
            .filter(|server| server.applies(language, root, text))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ManifestFile {
    One(ServerManifest),
    Many { servers: Vec<ServerManifest> },
}

impl ManifestFile {
    fn into_servers(self) -> Vec<ServerManifest> {
        match self {
            Self::One(server) => vec![server],
            Self::Many { servers } => servers,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerManifest {
    id: String,
    display_name: String,
    #[serde(default = "default_role")]
    role: String,
    languages: Vec<String>,
    candidates: Vec<CandidateManifest>,
    #[serde(default)]
    enabled_by_default: bool,
    #[serde(default)]
    activation: ActivationManifest,
    install: Option<InstallManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateManifest {
    executable: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationManifest {
    #[serde(default)]
    always: bool,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    content: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum InstallManifest {
    Npm { packages: Vec<String> },
}

impl TryFrom<ServerManifest> for CatalogServer {
    type Error = String;

    fn try_from(manifest: ServerManifest) -> Result<Self, Self::Error> {
        if manifest.id.is_empty()
            || !manifest
                .id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        {
            return Err("server ID must contain only letters, numbers, '-', '_' or '.'".into());
        }
        if manifest.candidates.is_empty() {
            return Err(format!("{} has no executable candidates", manifest.id));
        }
        let role = match manifest.role.as_str() {
            "primary" => LanguageServerRole::Primary,
            "supplementary" => LanguageServerRole::Supplementary,
            other => return Err(format!("unsupported server role {other}")),
        };
        let languages = manifest
            .languages
            .iter()
            .map(|language| {
                parse_language(language).ok_or_else(|| format!("unsupported language {language}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if languages.is_empty() {
            return Err(format!("{} has no supported languages", manifest.id));
        }
        let install = manifest
            .install
            .map(|install| match install {
                InstallManifest::Npm { packages } => {
                    validate_npm_packages(&packages)?;
                    Ok::<CatalogInstall, String>(CatalogInstall::Npm { packages })
                }
            })
            .transpose()?;
        Ok(Self {
            id: manifest.id,
            display_name: manifest.display_name,
            role,
            languages,
            candidates: manifest
                .candidates
                .into_iter()
                .map(|candidate| CatalogCandidate {
                    executable: candidate.executable,
                    args: candidate.args,
                })
                .collect(),
            install,
            enabled_by_default: manifest.enabled_by_default,
            activation: CatalogActivation {
                files: manifest.activation.files,
                packages: manifest.activation.packages,
                content: manifest.activation.content,
                always: manifest.activation.always,
            },
        })
    }
}

fn parse_language(value: &str) -> Option<Language> {
    match value.to_ascii_lowercase().as_str() {
        "rust" => Some(Language::Rust),
        "typescript" | "tsx" => Some(Language::TypeScript),
        "javascript" | "jsx" => Some(Language::JavaScript),
        "python" => Some(Language::Python),
        "go" => Some(Language::Go),
        "json" | "jsonc" => Some(Language::Json),
        "html" => Some(Language::Html),
        "css" | "scss" => Some(Language::Css),
        "markdown" | "mdx" => Some(Language::Markdown),
        "shell" | "bash" => Some(Language::Shell),
        "yaml" => Some(Language::Yaml),
        "toml" => Some(Language::Toml),
        "hcl" | "terraform" => Some(Language::Hcl),
        _ => None,
    }
}

fn manifest_paths() -> Vec<PathBuf> {
    let directory = dragent_paths::platform_support().join("language-servers");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect()
}

fn package_json_contains(root: &Path, packages: &[String]) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    packages
        .iter()
        .any(|package| text.contains(&format!("\"{package}\"")))
}

fn validate_npm_packages(packages: &[String]) -> Result<(), String> {
    if packages.is_empty() || packages.len() > 16 {
        return Err("npm install must contain between 1 and 16 packages".to_string());
    }
    for package in packages {
        if package.starts_with('-')
            || package.is_empty()
            || !package
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "@/._-".contains(character))
        {
            return Err(format!("unsafe npm package specifier {package:?}"));
        }
    }
    Ok(())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn default_role() -> String {
    "supplementary".to_string()
}

fn log_manifest_error(path: &Path, reason: &str) {
    eprintln!("[lsp-catalog] ignored {}: {reason}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_only_custom_server() {
        let manifest: ManifestFile = serde_json::from_str(
            r#"{
              "id":"custom-web-ls",
              "display_name":"Custom Web LS",
              "languages":["typescript","javascript"],
              "candidates":[{"executable":"custom-web-ls","args":["--stdio"]}],
              "activation":{"always":true},
              "install":{"source":"npm","packages":["custom-web-ls@1.2.3"]}
            }"#,
        )
        .unwrap();
        let server = CatalogServer::try_from(manifest.into_servers().pop().unwrap()).unwrap();
        assert_eq!(server.id, "custom-web-ls");
        assert!(matches!(server.install, Some(CatalogInstall::Npm { .. })));
    }

    #[test]
    fn rejects_command_fields_and_npm_options() {
        let command = serde_json::from_str::<ManifestFile>(
            r#"{
              "id":"unsafe",
              "display_name":"Unsafe",
              "languages":["typescript"],
              "candidates":[{"executable":"unsafe-ls"}],
              "command":"rm -rf project"
            }"#,
        );
        assert!(command.is_err());

        let manifest: ManifestFile = serde_json::from_str(
            r#"{
              "id":"unsafe-npm",
              "display_name":"Unsafe npm",
              "languages":["typescript"],
              "candidates":[{"executable":"unsafe-ls"}],
              "install":{"source":"npm","packages":["--global"]}
            }"#,
        )
        .unwrap();
        assert!(CatalogServer::try_from(manifest.into_servers().pop().unwrap()).is_err());
    }

    #[test]
    fn every_built_in_install_recipe_has_an_implementation() {
        let catalog = LanguageServerCatalog::built_in();
        for server in catalog.servers.values() {
            if let Some(CatalogInstall::BuiltIn(install_id)) = &server.install {
                assert!(
                    crate::installer::supports_built_in(install_id),
                    "{} advertises unsupported installer {install_id}",
                    server.id
                );
            }
        }
    }
}
