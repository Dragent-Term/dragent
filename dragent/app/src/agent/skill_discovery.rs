//! Agent Skills support following the standard lifecycle and the Agent Skills spec.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub const AGENTS_DIR_NAME: &str = ".agents";
pub const SKILLS_DIR_NAME: &str = "skills";
pub const SKILL_FILE_NAME: &str = "SKILL.md";
pub const MAX_SKILL_FILE_SIZE: usize = 100 * 1024;
pub const MAX_SKILL_DESCRIPTION_LEN: usize = 1024;
pub const MAX_SKILL_DESCRIPTIONS_SIZE: usize = 50 * 1024;
/// Dragent's own slash commands. A skill with one of these names is still
/// run by its `/:name` form.
pub const RESERVED_SLASH_COMMANDS: &[&str] =
    &["agent", "shell", "clear", "plan", "debug", "multitask", "ask", "model"];

const CREATE_SKILL_BODY: &str = "Create or update an Agent Skill that follows the Agent Skills specification.\n\nStore each skill at `.agents/skills/<name>/SKILL.md` for a project or `~/.agents/skills/<name>/SKILL.md` globally. Use lowercase letters, digits, and hyphens for the name. Keep the description concise and explicit about when the skill should be used. Put supporting scripts, references, and assets beside SKILL.md and refer to them relative to the skill directory.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    BuiltIn,
    Global,
    ProjectLocal { project_root: PathBuf },
}

impl SkillSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Global => "global",
            Self::ProjectLocal { .. } => "project-local",
        }
    }
    fn precedence(&self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Global => 1,
            Self::ProjectLocal { .. } => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSkill {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub source_path: PathBuf,
    pub directory_path: PathBuf,
    pub disable_model_invocation: bool,
    pub load_warnings: Vec<String>,
    built_in_body: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct SkillLoadIssue {
    pub path: PathBuf,
    pub message: String,
}

impl DiscoveredSkill {
    pub fn dir(&self) -> &Path {
        &self.directory_path
    }
    pub fn summary(&self) -> String {
        format!(
            "  {} [{}] — {}\n",
            self.name,
            self.source.label(),
            self.description
        )
    }
    pub fn slash_name(&self) -> String {
        match &self.source {
            SkillSource::ProjectLocal { project_root } => {
                let folder = project_root.file_name().and_then(|name| name.to_str()).unwrap_or("project");
                let namespace = folder.split_whitespace().collect::<Vec<_>>().join("-");
                format!("/{}:{}", if namespace.is_empty() { "project" } else { &namespace }, self.name)
            }
            SkillSource::BuiltIn | SkillSource::Global => format!("/:{}", self.name),
        }
    }
    /// Whether a typed slash command names this skill: its own `/:name` or
    /// `/<project>:name`, or plain `/name` when Dragent has no command of
    /// that name.
    pub fn answers_to(&self, command: &str) -> bool {
        command == self.slash_name()
            || (command.strip_prefix('/') == Some(self.name.as_str())
                && !RESERVED_SLASH_COMMANDS.contains(&self.name.as_str()))
    }
    pub fn load_body(&self) -> Result<String, String> {
        if let Some(body) = self.built_in_body {
            return Ok(body.to_string());
        }
        let content = std::fs::read_to_string(&self.source_path)
            .map_err(|e| format!("Failed to read {}: {e}", self.source_path.display()))?;
        if content.len() > MAX_SKILL_FILE_SIZE {
            return Err("Skill exceeds the 100KB size limit".into());
        }
        split_frontmatter(&content).map(|(_, body)| body.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SkillMetadata {
    name: String,
    description: String,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
    #[serde(
        default,
        rename = "allowed-tools",
        skip_serializing_if = "Option::is_none"
    )]
    allowed_tools: Option<serde_yaml::Value>,
}

pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 64 {
        return Err("Name must contain 1–64 characters");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("Name cannot start or end with a hyphen");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Use only lowercase letters, numbers, and hyphens");
    }
    Ok(())
}

pub fn validate_description(description: &str) -> Result<(), &'static str> {
    if description.trim().is_empty() {
        return Err("Description is required");
    }
    if description.chars().count() > MAX_SKILL_DESCRIPTION_LEN {
        return Err("Description must be 1024 characters or fewer");
    }
    Ok(())
}

fn split_frontmatter(content: &str) -> Result<(SkillMetadata, &str), String> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or("SKILL.md must begin with YAML frontmatter")?;
    let (yaml, body) = rest
        .split_once("\n---")
        .ok_or("SKILL.md frontmatter is missing its closing ---")?;
    let metadata: SkillMetadata =
        serde_yaml::from_str(yaml).map_err(|e| format!("Invalid skill frontmatter: {e}"))?;
    validate_name(&metadata.name).map_err(str::to_string)?;
    validate_description(&metadata.description).map_err(str::to_string)?;
    Ok((metadata, body.trim_start_matches(['\r', '\n'])))
}

pub fn parse_skill_file(content: &str, path: &Path) -> Result<DiscoveredSkill, String> {
    if content.len() > MAX_SKILL_FILE_SIZE {
        return Err("Skill exceeds the 100KB size limit".into());
    }
    let (metadata, _) = split_frontmatter(content)?;
    Ok(DiscoveredSkill {
        name: metadata.name,
        description: metadata.description,
        source: SkillSource::Global,
        source_path: path.to_path_buf(),
        directory_path: path.parent().unwrap_or(Path::new("")).to_path_buf(),
        disable_model_invocation: metadata.disable_model_invocation,
        load_warnings: Vec::new(),
        built_in_body: None,
    })
}

fn scan_skills_dir(dir: &Path, source: SkillSource) -> Vec<DiscoveredSkill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    entries
        .into_iter()
        .filter_map(|entry| {
            let directory = entry.path();
            if !directory.is_dir() {
                return None;
            }
            let path = directory.join(SKILL_FILE_NAME);
            if std::fs::metadata(&path).ok()?.len() > MAX_SKILL_FILE_SIZE as u64 {
                log::warn!("[skills] ignoring oversized skill {}", path.display());
                return None;
            }
            let content = std::fs::read_to_string(&path).ok()?;
            match parse_skill_file(&content, &path) {
                Ok(mut skill) => {
                    skill.source = source.clone();
                    Some(skill)
                }
                Err(error) => {
                    log::warn!("[skills] failed to load {}: {error}", path.display());
                    None
                }
            }
        })
        .collect()
}

pub fn global_skills_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(AGENTS_DIR_NAME)
        .join(SKILLS_DIR_NAME)
}
pub fn project_skills_dir(root: &Path) -> PathBuf {
    root.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME)
}

/// User-created project skills in `.dragent` are read without creating that
/// directory. App-owned project state is stored in the home directory.
fn project_dragent_skills_dir(root: &Path) -> PathBuf {
    root.join(".dragent").join(SKILLS_DIR_NAME)
}

pub fn resolve_project_root(path: &Path) -> PathBuf {
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    start
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .unwrap_or(start)
        .to_path_buf()
}

pub(crate) fn built_in_skills() -> Vec<DiscoveredSkill> {
    let mut skills = vec![DiscoveredSkill {
        name: "create-skill".into(),
        description: "Create or update an Agent Skill with the correct structure and metadata."
            .into(),
        source: SkillSource::BuiltIn,
        source_path: "<built-in>/create-skill/SKILL.md".into(),
        directory_path: "<built-in>/create-skill".into(),
        disable_model_invocation: false,
        load_warnings: Vec::new(),
        built_in_body: Some(CREATE_SKILL_BODY),
    }];
    #[cfg(not(target_family = "wasm"))]
    skills.extend(
        dragent_host::agent_skills::BUNDLED_SKILLS
            .iter()
            .filter_map(|skill| bundled_skill(skill, bundle_dir())),
    );
    skills
}

/// This build's skill packages unpacked on disk, so the agent reads their
/// references and runs their helpers with its ordinary tools. Unpacked once
/// per process (see `agent_skills::materialize_bundle`).
#[cfg(not(target_family = "wasm"))]
pub(crate) fn bundle_dir() -> Option<&'static Path> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let root = dragent_host::agent_skills::bundle_root();
        match dragent_host::agent_skills::materialize_bundle(&root) {
            Ok(dir) => Some(dir),
            Err(error) => {
                log::warn!("[skills] couldn't unpack the bundled skills into {}: {error}", root.display());
                None
            }
        }
    })
    .as_deref()
}

#[cfg(not(target_family = "wasm"))]
fn bundled_skill(
    skill: &'static dragent_host::agent_skills::BundledSkill,
    unpacked: Option<&Path>,
) -> Option<DiscoveredSkill> {
    let (metadata, body) = split_frontmatter(skill.content).ok()?;
    let (directory_path, load_warnings) = match unpacked {
        Some(root) => (root.join(skill.name), Vec::new()),
        // Without files on disk only the instructions work; say so rather
        // than hand the model a path its tools can't read.
        None => (
            PathBuf::new(),
            (skill.files.len() > 1)
                .then(|| "Its supporting files couldn't be unpacked, so only its instructions are available.".to_string())
                .into_iter()
                .collect(),
        ),
    };
    Some(DiscoveredSkill {
        source_path: directory_path.join(SKILL_FILE_NAME),
        directory_path,
        name: metadata.name,
        description: metadata.description,
        source: SkillSource::BuiltIn,
        disable_model_invocation: metadata.disable_model_invocation,
        load_warnings,
        built_in_body: Some(body),
    })
}

/// Identical managed shared copies — every file of the package, nothing
/// added — are one built-in skill in Dragent’s UI and catalog. A copy the
/// user changed in any way remains visible and keeps its precedence.
fn is_bundled_mirror(skill: &DiscoveredSkill) -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        let Some(bundled) = dragent_host::agent_skills::bundled(&skill.name) else {
            return false;
        };
        let dir = &skill.directory_path;
        return bundled
            .files
            .iter()
            .all(|file| std::fs::read(dir.join(file.path)).is_ok_and(|bytes| bytes == file.bytes))
            && skill_resources(dir, usize::MAX).len() + 1 == bundled.files.len();
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = skill;
        false
    }
}

/// A skill folder's files besides SKILL.md, relative and `/`-separated,
/// skipping hidden ones; at most `limit`.
pub fn skill_resources(dir: &Path, limit: usize) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>, limit: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if out.len() >= limit {
                return;
            }
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(base, &path, out, limit),
                Ok(kind) if kind.is_file() => {
                    let Ok(relative) = path.strip_prefix(base) else {
                        continue;
                    };
                    let relative = relative
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    if relative != SKILL_FILE_NAME {
                        out.push(relative);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    if !dir.as_os_str().is_empty() {
        walk(dir, dir, &mut out, limit);
    }
    out
}

fn user_global_skills() -> Vec<DiscoveredSkill> {
    scan_skills_dir(&global_skills_dir(), SkillSource::Global).into_iter().filter(|s| !is_bundled_mirror(s)).collect()
}

/// Metadata-only flat scan. It is intentionally uncached, so externally added,
/// removed, or edited skills are reflected on the next UI render or agent turn.
pub fn discover_skills(project_root: &Path) -> Vec<DiscoveredSkill> {
    let project_root = resolve_project_root(project_root);
    let project_source = SkillSource::ProjectLocal {
        project_root: project_root.clone(),
    };
    let project_skills =
        if crate::workspace::trust_store::WorkspaceTrustStore::load().permits(&project_root, crate::settings::AppSettings::global().agent.workspace_trust_enabled) {
            scan_skills_dir(&project_skills_dir(&project_root), project_source.clone())
                .into_iter()
                .chain(scan_skills_dir(
                    &project_dragent_skills_dir(&project_root),
                    project_source,
                ))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
    let all = built_in_skills()
        .into_iter()
        .chain(user_global_skills())
        .chain(project_skills);
    let mut index: HashMap<String, DiscoveredSkill> = HashMap::new();
    for skill in all {
        if index
            .get(&skill.name)
            .is_none_or(|old| old.source.precedence() < skill.source.precedence())
        {
            index.insert(skill.name.clone(), skill);
        }
    }
    let mut skills: Vec<_> = index.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub fn skills_for_scope(project_root: Option<&Path>) -> Vec<DiscoveredSkill> {
    match project_root {
        Some(root) => {
            let root = resolve_project_root(root);
            scan_skills_dir(
                &project_skills_dir(&root),
                SkillSource::ProjectLocal {
                    project_root: root.clone(),
                },
            )
            .into_iter()
            .chain(scan_skills_dir(
                &project_dragent_skills_dir(&root),
                SkillSource::ProjectLocal { project_root: root },
            ))
            .collect()
        }
        None => user_global_skills(),
    }
}

pub fn skill_load_issues(project_root: Option<&Path>) -> Vec<SkillLoadIssue> {
    let roots = match project_root {
        Some(root) => {
            let root = resolve_project_root(root);
            vec![project_skills_dir(&root), project_dragent_skills_dir(&root)]
        }
        None => vec![global_skills_dir()],
    };
    roots.iter().flat_map(|root| skill_load_issues_in(root)).collect()
}

fn skill_load_issues_in(root: &Path) -> Vec<SkillLoadIssue> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join(SKILL_FILE_NAME);
            if !path.is_file() {
                return None;
            }
            let result = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|content| parse_skill_file(&content, &path).map(|_| ()));
            result.err().map(|message| SkillLoadIssue { path, message })
        })
        .collect()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn skills_prompt_block(project_root: &Path) -> String {
    let mut output = String::from("<available_skills>\n");
    let mut used = 0;
    for skill in discover_skills(project_root)
        .into_iter()
        .filter(|s| !s.disable_model_invocation)
    {
        let size = skill.name.len() + skill.description.len();
        if used + size > MAX_SKILL_DESCRIPTIONS_SIZE {
            log::warn!("[skills] catalog reached its 50KB metadata budget");
            break;
        }
        used += size;
        output.push_str(&format!("  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n", xml_escape(&skill.name), xml_escape(&skill.description), xml_escape(&skill.source_path.to_string_lossy())));
    }
    output.push_str("</available_skills>");
    output
}

/// The skill a message runs when its first word is a skill command, with
/// the rest of the message as the skill's arguments. A plain `/name` that is
/// a real path on disk stays a path.
pub fn invoked_skill(message: &str, project_root: &Path) -> Option<(DiscoveredSkill, String)> {
    split_command(message)?;
    invoked_among(message, discover_skills(project_root))
}

fn invoked_among(
    message: &str,
    skills: impl IntoIterator<Item = DiscoveredSkill>,
) -> Option<(DiscoveredSkill, String)> {
    let (command, arguments) = split_command(message)?;
    skills
        .into_iter()
        .find(|skill| skill.answers_to(command))
        .map(|skill| (skill, arguments.to_string()))
}

/// A leading slash command and the text after it.
fn split_command(message: &str) -> Option<(&str, &str)> {
    let message = message.trim_start();
    let command = message.split_whitespace().next()?;
    if !command.starts_with('/') || command.len() < 2 {
        return None;
    }
    if !command.contains(':') && Path::new(command).exists() {
        return None;
    }
    Some((command, message[command.len()..].trim()))
}

/// System-prompt section for a skill the user ran by its command. The
/// instructions travel with the request itself, so skills only the user
/// may start (`disable-model-invocation`) work too.
pub fn invoked_skill_prompt(message: &str, project_root: &Path) -> Option<String> {
    let (skill, arguments) = invoked_skill(message, project_root)?;
    let envelope = match render_skill_envelope(&skill) {
        Ok(envelope) => envelope,
        Err(error) => {
            log::warn!("[skills] loading {} for its command: {error}", skill.name);
            return None;
        }
    };
    let mut section = format!(
        "## Skill run by the user\n\nThe user's message starts with the command for the `{}` skill. Follow the skill's instructions below for this request",
        skill.name
    );
    if arguments.is_empty() {
        section.push_str(".\n\n");
    } else {
        section.push_str(", applied to the text after the command.\n\n");
    }
    section.push_str(&envelope);
    if !arguments.is_empty() {
        section.push_str(&format!(
            "\n<user_arguments>{}</user_arguments>",
            xml_escape(&arguments)
        ));
    }
    Some(section)
}

pub fn warm_skills_prompt(project_root: &Path) {
    let _ = discover_skills(project_root);
}

/// Most supporting files listed with a loaded skill.
const LISTED_RESOURCES: usize = 40;

pub fn render_skill_envelope(skill: &DiscoveredSkill) -> Result<String, String> {
    let body = skill.load_body()?;
    let mut envelope = format!(
        "<skill_content name=\"{}\">\n<source>{}</source>\n",
        xml_escape(&skill.name),
        skill.source.label()
    );
    if skill.directory_path.as_os_str().is_empty() {
        envelope.push_str("This skill's supporting files aren't available; follow its instructions only.\n");
    } else {
        envelope.push_str(&format!(
            "<directory>{}</directory>\nRelative paths in this skill resolve against &lt;directory&gt;: read references with your file tools and run helper scripts from there.\n",
            xml_escape(&skill.directory_path.to_string_lossy())
        ));
        let resources = skill_resources(&skill.directory_path, LISTED_RESOURCES + 1);
        if !resources.is_empty() {
            envelope.push_str("<resources>\n");
            for resource in resources.iter().take(LISTED_RESOURCES) {
                envelope.push_str(&xml_escape(resource));
                envelope.push('\n');
            }
            if resources.len() > LISTED_RESOURCES {
                envelope.push_str("…more files in the directory\n");
            }
            envelope.push_str("</resources>\n");
        }
    }
    for warning in &skill.load_warnings {
        envelope.push_str(&format!("<note>{}</note>\n", xml_escape(warning)));
    }
    envelope.push_str(&format!("\n{}\n</skill_content>", xml_escape(&body)));
    Ok(envelope)
}

pub fn format_skill_list(skills: &[DiscoveredSkill]) -> String {
    if skills.is_empty() {
        return format!(
            "No skills found. Add global skills to {} or project skills to .agents/skills.",
            global_skills_dir().display()
        );
    }
    let mut out = format!("{} skill(s) available:\n\n", skills.len());
    for skill in skills {
        out.push_str(&skill.summary());
    }
    out
}

pub fn write_skill(
    project_root: Option<&Path>,
    name: &str,
    description: &str,
    body: &str,
    disable_model_invocation: bool,
) -> Result<PathBuf, String> {
    validate_name(name).map_err(str::to_string)?;
    validate_description(description).map_err(str::to_string)?;
    if body.trim().is_empty() {
        return Err("Skill instructions are required".into());
    }
    let root = project_root
        .map(|root| project_skills_dir(&resolve_project_root(root)))
        .unwrap_or_else(global_skills_dir);
    let path = root.join(name).join(SKILL_FILE_NAME);
    if path.exists() {
        return Err(format!(
            "A skill named '{name}' already exists in this scope"
        ));
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| format!("Failed to create skill directory: {e}"))?;
    let yaml = serde_yaml::to_string(&SkillMetadata {
        name: name.into(),
        description: description.into(),
        disable_model_invocation,
        allowed_tools: None,
    })
    .map_err(|e| e.to_string())?;
    let content = format!("---\n{}---\n\n{}\n", yaml, body.trim());
    if content.len() > MAX_SKILL_FILE_SIZE {
        return Err("Skill exceeds the 100KB size limit".into());
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(path)
}

pub fn delete_skill(skill: &DiscoveredSkill) -> Result<(), String> {
    if matches!(skill.source, SkillSource::BuiltIn) {
        return Err("Built-in skills cannot be deleted".into());
    }
    trash_skill_directory(&skill.directory_path).map(|_| ())
}

pub fn trash_skill_directory(directory: &Path) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Home directory is unavailable")?;
    #[cfg(target_os = "macos")]
    let trash = home.join(".Trash");
    #[cfg(all(unix, not(target_os = "macos")))]
    let trash = home.join(".local/share/Trash/files");
    #[cfg(windows)]
    return Err("Move this skill directory to the Recycle Bin manually".into());
    #[cfg(unix)]
    {
        std::fs::create_dir_all(&trash)
            .map_err(|error| format!("Could not open Trash: {error}"))?;
        let base = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("skill");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let destination = trash.join(format!("{base}-{stamp}"));
        std::fs::rename(directory, &destination)
            .map_err(|error| format!("Could not move skill to Trash: {error}"))?;
        Ok(destination)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn focused_pack_loads_and_can_be_invoked_with_or_without_unpacked_files() {
        let mut names = std::collections::HashSet::new();
        for entry in dragent_host::agent_skills::BUNDLED_SKILLS {
            let skill = super::bundled_skill(entry, None).expect("valid bundled skill");
            assert_eq!(skill.name, entry.name);
            assert!(names.insert(skill.name.clone()), "skill names must be unique");
            assert!(!skill.disable_model_invocation);
            assert!(!skill.load_body().unwrap().trim().is_empty());
            let command = format!("/:{} a concrete task", skill.name);
            let (found, arguments) = super::invoked_among(&command, vec![skill]).expect("explicit invocation resolves");
            assert_eq!(arguments, "a concrete task");
            let envelope = super::render_skill_envelope(&found).unwrap();
            assert!(!envelope.contains("<built-in>"), "never a path ordinary tools can't read");
        }
        let builtins = super::built_in_skills();
        for name in names { assert!(builtins.iter().any(|s| s.name == name)); }
    }

    use super::*;
    fn valid(name: &str, description: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}")
    }

    #[test]
    fn unpacked_builtins_expose_real_resources() {
        let root = tempfile::tempdir().unwrap();
        let unpacked = dragent_host::agent_skills::materialize_bundle(root.path()).unwrap();
        for entry in dragent_host::agent_skills::BUNDLED_SKILLS {
            let skill = bundled_skill(entry, Some(&unpacked)).unwrap();
            assert!(skill.source_path.is_file(), "{}", skill.source_path.display());
            let resources = skill_resources(skill.dir(), usize::MAX);
            assert_eq!(resources.len() + 1, entry.files.len());
            let envelope = render_skill_envelope(&skill).unwrap();
            for resource in resources.iter().take(LISTED_RESOURCES) {
                assert!(envelope.contains(resource.as_str()));
                assert!(skill.dir().join(resource).is_file(), "listed files are readable");
            }
        }
    }

    #[test]
    fn shared_mirrors_deduplicate_but_user_changes_still_override() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("dragent");
        let package = dragent_host::agent_skills::bundled("dragent").unwrap();
        dragent_host::agent_skills::write_package(package, &skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        let shared = parse_skill_file(package.content, &path).unwrap();
        assert!(is_bundled_mirror(&shared));
        // A file of the user's own makes it their copy.
        std::fs::write(skill_dir.join("notes.md"), "mine").unwrap();
        assert!(!is_bundled_mirror(&shared));
        std::fs::remove_file(skill_dir.join("notes.md")).unwrap();
        std::fs::write(&path, format!("{}\nUser additions", package.content)).unwrap();
        assert!(!is_bundled_mirror(&shared));
        assert_eq!(shared.source, SkillSource::Global);
        assert!(shared.source.precedence() > SkillSource::BuiltIn.precedence());
    }

    #[test]
    fn skill_commands_find_their_skill() {
        let skill = |name: &str| {
            let content = valid(name, "Does a thing.", "Body");
            parse_skill_file(&content, Path::new("/skills/x/SKILL.md")).unwrap()
        };
        let skills = || vec![skill("review"), skill("shell"), skill("tmp")];

        let (found, arguments) = invoked_among("/:review the auth module", skills()).unwrap();
        assert_eq!((found.name.as_str(), arguments.as_str()), ("review", "the auth module"));
        let (found, arguments) = invoked_among("  /review", skills()).unwrap();
        assert_eq!((found.name.as_str(), arguments.as_str()), ("review", ""));
        assert!(invoked_among("/shell", skills()).is_none(), "/shell is Dragent's own command");
        assert_eq!(invoked_among("/:shell", skills()).unwrap().0.name, "shell");
        #[cfg(unix)]
        assert!(invoked_among("/tmp", skills()).is_none(), "a real path stays a path");
        assert_eq!(invoked_among("/:tmp", skills()).unwrap().0.name, "tmp");
        assert!(invoked_among("please /review", skills()).is_none());
        assert!(invoked_among("/reviewer", skills()).is_none());
    }

    #[test]
    fn project_skill_commands_work_with_spaces_in_the_folder_name() {
        let mut skill = parse_skill_file(&valid("review", "Review changes", "Instructions"), Path::new("SKILL.md")).unwrap();
        skill.source = SkillSource::ProjectLocal { project_root: "/work/My Project".into() };
        assert_eq!(skill.slash_name(), "/My-Project:review");
        let (found, arguments) = invoked_among("/My-Project:review staged files", vec![skill]).unwrap();
        assert_eq!(found.name, "review");
        assert_eq!(arguments, "staged files");
    }

    #[test]
    fn strict_frontmatter_validation() {
        assert!(parse_skill_file("Body only", Path::new("x/SKILL.md")).is_err());
        assert!(parse_skill_file(
            &valid("Bad Name", "Description", "Body"),
            Path::new("x/SKILL.md")
        )
        .is_err());
        assert!(parse_skill_file(
            &valid("good-name", "Description", "Body"),
            Path::new("x/SKILL.md")
        )
        .is_ok());
    }

    #[test]
    fn flat_scan_ignores_resource_skills() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("outer/references/inner");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            temp.path().join("outer/SKILL.md"),
            valid("outer", "Outer", "Body"),
        )
        .unwrap();
        std::fs::write(nested.join("SKILL.md"), valid("inner", "Inner", "Body")).unwrap();
        assert_eq!(scan_skills_dir(temp.path(), SkillSource::Global).len(), 1);
    }

    #[test]
    fn problems_include_both_project_skill_locations() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        for folder in [".agents", ".dragent"] {
            let path = project.path().join(folder).join("skills/broken/SKILL.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "missing frontmatter").unwrap();
        }
        let issues = skill_load_issues(Some(project.path()));
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().all(|issue| issue.message.contains("frontmatter")));
    }

    #[test]
    fn body_is_loaded_on_demand() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("demo");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SKILL_FILE_NAME);
        std::fs::write(&path, valid("demo", "Demo", "First")).unwrap();
        let skill = parse_skill_file(&std::fs::read_to_string(&path).unwrap(), &path).unwrap();
        std::fs::write(&path, valid("demo", "Demo", "Second")).unwrap();
        assert_eq!(skill.load_body().unwrap(), "Second");
    }

}
