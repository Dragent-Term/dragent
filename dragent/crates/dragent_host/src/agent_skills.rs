//! Agent Skills for agents that don't read the shared skill folders, and
//! the catalog of Dragent's own skill packages.
//!
//! Skills follow the open Agent Skills layout: `<name>/SKILL.md` under
//! `~/.agents/skills` (every project) or a project's `.agents/skills` or
//! `.dragent/skills`. Codex and OpenCode read those folders themselves;
//! Claude Code only reads `.claude/skills`, so Dragent links the shared
//! skills into the plugin it already starts Claude Code with. They then
//! appear as `/dragent:<name>`.
//!
//! Dragent's own skills are whole packages: SKILL.md plus the references,
//! helper scripts and small assets it points to, all compiled in from
//! `skills/<name>/` (see `build.rs`). Every consumer — the native agent, the
//! shared-folder installer and session plugins — reads [`BUNDLED_SKILLS`],
//! so instructions and resources always come from the same build.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SKILL_FILE: &str = "SKILL.md";
/// Written last when a package is unpacked: its presence means complete.
const COMPLETE_MARKER: &str = ".complete";

/// One file of a bundled skill package.
#[derive(Clone, Copy, Debug)]
pub struct BundledFile {
    /// `/`-separated path inside the package, e.g. `references/recipes.md`.
    pub path: &'static str,
    pub bytes: &'static [u8],
    /// Helper scripts are installed executable.
    pub executable: bool,
}

/// One catalog used by the native agent, external-agent installer and Claude plugin.
#[derive(Clone, Copy, Debug)]
pub struct BundledSkill {
    pub name: &'static str,
    /// Its SKILL.md.
    pub content: &'static str,
    /// Every file of the package, SKILL.md included.
    pub files: &'static [BundledFile],
}

impl BundledSkill {
    pub fn file(&self, path: &str) -> Option<&'static BundledFile> {
        self.files.iter().find(|file| file.path == path)
    }
}

// `BUNDLED_SKILLS` and `BUNDLE_VERSION`, generated from `skills/`.
include!(concat!(env!("OUT_DIR"), "/bundled_skills.rs"));

/// Dragent's own skill: how agents use the `dragent` command.
pub const DRAGENT_SKILL: &str = include_str!("../skills/dragent/SKILL.md");
/// Its name, as agents list it.
pub const DRAGENT_SKILL_NAME: &str = "dragent";
pub const COMPUTER_SKILL: &str = include_str!("../skills/dragent-computer/SKILL.md");
pub const COMPUTER_SKILL_NAME: &str = "dragent-computer";

pub fn bundled(name: &str) -> Option<&'static BundledSkill> {
    BUNDLED_SKILLS.iter().find(|skill| skill.name == name)
}

/// Write a whole package into `dir`.
pub fn write_package(skill: &BundledSkill, dir: &Path) -> io::Result<()> {
    for file in skill.files {
        let target = dir.join(file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, file.bytes)?;
        set_executable(&target, file.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn set_executable(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub fn set_executable(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

/// Where this build's packages are unpacked for agents that read skills
/// from disk (Dragent's own agent). Managed by Dragent: edits here are lost
/// on the next version.
pub fn bundle_root() -> PathBuf {
    dragent_paths::home().join("skills").join("bundled")
}

/// Unpack this build's packages into `root/<version>/<name>/…` once and
/// return that folder. A version is written whole to a staging folder and
/// then renamed into place, so readers never see a half-written version or
/// one build's instructions beside another's scripts. The previous version
/// is kept for sessions that started on it; older ones are removed.
pub fn materialize_bundle(root: &Path) -> io::Result<PathBuf> {
    let version = root.join(BUNDLE_VERSION);
    if version.join(COMPLETE_MARKER).is_file() {
        return Ok(version);
    }
    fs::create_dir_all(root)?;
    let staging = root.join(format!(".staging-{BUNDLE_VERSION}-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for skill in BUNDLED_SKILLS {
        write_package(skill, &staging.join(skill.name))?;
    }
    fs::write(staging.join(COMPLETE_MARKER), BUNDLE_VERSION)?;
    if version.exists() {
        // An incomplete leftover from an interrupted run.
        if version.join(COMPLETE_MARKER).is_file() {
            let _ = fs::remove_dir_all(&staging);
            return Ok(version);
        }
        fs::remove_dir_all(&version)?;
    }
    if let Err(error) = fs::rename(&staging, &version) {
        let _ = fs::remove_dir_all(&staging);
        // Another Dragent process won the race with the same content.
        if version.join(COMPLETE_MARKER).is_file() {
            return Ok(version);
        }
        return Err(error);
    }
    prune_versions(root);
    Ok(version)
}

/// Keep the current version and the newest other complete one.
fn prune_versions(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name() != BUNDLE_VERSION)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    others.sort();
    let staging_age = std::time::Duration::from_secs(60 * 60);
    let keep = others
        .iter()
        .rev()
        .find(|(_, path)| path.join(COMPLETE_MARKER).is_file())
        .map(|(_, path)| path.clone());
    for (modified, path) in others {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let stale_staging = name.starts_with(".staging-")
            && modified.elapsed().is_ok_and(|age| age > staging_age);
        if Some(&path) == keep.as_ref() || (name.starts_with(".staging-") && !stale_staging) {
            continue;
        }
        let _ = fs::remove_dir_all(&path);
    }
}

/// The `dragent` launcher agents start the tools bridge with, if the app
/// has run on this machine.
/// Set by Dragent on the tools bridge it gives an agent it started: `dragent
/// mcp` serves only with it (or inside a Dragent terminal).
pub const AGENT_PANE_ENV: &str = "DRAGENT_AGENT_PANE";

/// Whether this process runs for an agent Dragent started, or in one of its
/// terminals.
pub fn inside_dragent() -> bool {
    [AGENT_PANE_ENV, "DRAGENT_HOST_TERMINAL_ID"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

pub fn dragent_launcher() -> Option<PathBuf> {
    // On a remote machine, its dispatcher picks the session's organisation.
    if let Some(launcher) = crate::remote_mcp::launcher() {
        return Some(launcher);
    }
    let launcher = if cfg!(windows) { "dragent.cmd" } else { "dragent" };
    [
        dragent_paths::organisation_dir().join("bin"),
        dragent_paths::home().join("bin"),
    ]
    .into_iter()
    .map(|dir| dir.join(launcher))
    .find(|path| path.is_file())
}

/// A folder holding this build's skills as `skills/<name>/…`, for agents
/// given an extra config folder at launch (OpenCode). Written once per
/// build, like [`materialize_bundle`].
pub fn materialize_config_dir(root: &Path) -> io::Result<PathBuf> {
    let dir = root.join(format!("config-{BUNDLE_VERSION}"));
    if dir.join(COMPLETE_MARKER).is_file() {
        return Ok(dir);
    }
    let staging = root.join(format!(".staging-config-{BUNDLE_VERSION}-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for skill in BUNDLED_SKILLS {
        write_package(skill, &staging.join("skills").join(skill.name))?;
    }
    fs::write(staging.join(COMPLETE_MARKER), BUNDLE_VERSION)?;
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    if let Err(error) = fs::rename(&staging, &dir) {
        let _ = fs::remove_dir_all(&staging);
        if dir.join(COMPLETE_MARKER).is_file() {
            return Ok(dir);
        }
        return Err(error);
    }
    // Earlier builds' folders, unless a running session may still read them.
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let old = name.starts_with("config-") && entry.path() != dir;
            let aged = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > std::time::Duration::from_secs(7 * 24 * 3600)));
            if old && aged {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(dir)
}

/// Whether this machine has a `dragent` launcher (the app has run here).
/// Machines reached over SSH don't, and their agents shouldn't be told to
/// use a command that isn't there.
pub fn dragent_command_installed() -> bool {
    let launcher = if cfg!(windows) { "dragent.cmd" } else { "dragent" };
    [
        dragent_paths::home().join("bin"),
        dragent_paths::organisation_dir().join("bin"),
    ]
    .iter()
    .any(|dir| dir.join(launcher).is_file())
}

/// Add Dragent's skill to the Claude Code plugin unless a skill of that name
/// is already there or in Claude's own folders (the user's wins).
pub fn add_dragent_skill(plugin: &Path, cwd: &Path, home: &Path) -> io::Result<bool> {
    let project = project_root(cwd);
    let taken = [
        plugin.join("skills"),
        home.join(".claude/skills"),
        project.join(".claude/skills"),
    ]
    .iter()
    .any(|dir| dir.join(DRAGENT_SKILL_NAME).join(SKILL_FILE).is_file());
    if taken {
        return Ok(false);
    }
    let Some(skill) = bundled(DRAGENT_SKILL_NAME) else {
        return Ok(false);
    };
    write_package(skill, &plugin.join("skills").join(DRAGENT_SKILL_NAME))?;
    Ok(true)
}

/// Add the focused pack, complete with each skill's resources, to a
/// session-local plugin after linking shared skills. Existing shared,
/// project and agent-native skills of the same name take priority.
pub fn add_bundled_skills(plugin: &Path, cwd: &Path, home: &Path) -> io::Result<usize> {
    let project = project_root(cwd);
    let mut added = 0;
    for skill in BUNDLED_SKILLS {
        let taken = [
            plugin.join("skills"),
            home.join(".claude/skills"),
            project.join(".claude/skills"),
        ]
        .iter()
        .any(|dir| dir.join(skill.name).join(SKILL_FILE).is_file());
        if taken {
            continue;
        }
        write_package(skill, &plugin.join("skills").join(skill.name))?;
        added += 1;
    }
    Ok(added)
}

/// Link every shared skill Claude Code doesn't already load into
/// `<plugin>/skills`, replacing what an earlier launch put there. A project
/// skill wins over a global one of the same name.
pub fn link_into_claude_plugin(plugin: &Path, cwd: &Path, home: &Path) -> io::Result<usize> {
    let project = project_root(cwd);
    let skills = shared_skills(home, &project);
    let target = plugin.join("skills");
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    let mut linked = 0;
    for (name, source) in skills {
        let own = [home.join(".claude/skills"), project.join(".claude/skills")];
        if own.iter().any(|dir| dir.join(&name).join(SKILL_FILE).is_file()) {
            continue;
        }
        fs::create_dir_all(&target)?;
        link_dir(&source, &target.join(&name))?;
        linked += 1;
    }
    Ok(linked)
}

/// Shared skills by folder name, global first so project ones replace them.
fn shared_skills(home: &Path, project: &Path) -> BTreeMap<String, PathBuf> {
    let mut skills = BTreeMap::new();
    for dir in [
        home.join(".agents/skills"),
        project.join(".agents/skills"),
        project.join(".dragent/skills"),
    ] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !name.starts_with('.') && path.join(SKILL_FILE).is_file() {
                skills.insert(name, path);
            }
        }
    }
    skills
}

/// The repository containing `cwd`, or `cwd` itself outside one.
fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// A link keeps the skill live: edits show up in the next Claude Code
/// session without another copy.
#[cfg(unix)]
fn link_dir(source: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

/// Windows needs extra rights for directory symlinks, so the skill is copied.
#[cfg(not(unix))]
fn link_dir(source: &Path, link: &Path) -> io::Result<()> {
    fs::create_dir_all(link)?;
    for entry in fs::read_dir(source)?.flatten() {
        let path = entry.path();
        let destination = link.join(entry.file_name());
        if path.is_dir() {
            link_dir(&path, &destination)?;
        } else {
            fs::copy(&path, &destination)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_pack_reaches_claude_and_respects_native_and_project_skills() {
        let root = std::env::temp_dir().join(format!("dragent-pack-{}",uuid::Uuid::new_v4()));
        let home=root.join("home"); let project=root.join("repo"); let plugin=root.join("plugin");
        fs::create_dir_all(project.join(".git")).unwrap();
        skill(&home.join(".claude/skills"),"dragent-panes","native user's own");
        skill(&project.join(".agents/skills"),"dragent-browser","project override");
        link_into_claude_plugin(&plugin,&project,&home).unwrap();
        let added=add_bundled_skills(&plugin,&project,&home).unwrap();
        assert_eq!(added,BUNDLED_SKILLS.len()-2);
        assert!(!plugin.join("skills/dragent-panes").exists());
        assert_eq!(fs::read_to_string(plugin.join("skills/dragent-browser/SKILL.md")).unwrap(),"project override");
        for name in ["dragent-orchestrate","dragent-computer","dragent-standup"] {
            assert!(plugin.join("skills").join(name).join(SKILL_FILE).is_file());
        }
        assert_eq!(add_bundled_skills(&plugin,&project,&home).unwrap(),0);
        // Relaunch reconstructs the plugin and then adds the same complete bundle.
        link_into_claude_plugin(&plugin,&project,&home).unwrap();
        add_bundled_skills(&plugin,&project,&home).unwrap();
        assert!(plugin.join("skills/dragent-orchestrate/SKILL.md").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dragent_skill_joins_the_plugin_unless_one_exists() {
        let root = std::env::temp_dir().join(format!("dragent-skill-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let project = root.join("repo");
        let plugin = root.join("plugin");
        fs::create_dir_all(project.join(".git")).unwrap();
        assert!(add_dragent_skill(&plugin, &project, &home).unwrap());
        let written = fs::read_to_string(plugin.join("skills/dragent").join(SKILL_FILE)).unwrap();
        assert!(written.starts_with("---\nname: dragent\n"));
        assert!(!add_dragent_skill(&plugin, &project, &home).unwrap(), "once");

        let own = root.join("plugin-2");
        skill(&project.join(".claude/skills"), "dragent", "the user's own");
        assert!(!add_dragent_skill(&own, &project, &home).unwrap(), "the user's wins");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn packages_carry_every_file_and_unpack_whole() {
        for skill in BUNDLED_SKILLS {
            let md = skill.file(SKILL_FILE).expect("SKILL.md in the package");
            assert_eq!(md.bytes, skill.content.as_bytes());
            for file in skill.files {
                assert!(!file.path.starts_with('/') && !file.path.split('/').any(|p| p == ".." || p.is_empty()));
            }
        }
        let root = std::env::temp_dir().join(format!("dragent-bundle-{}", uuid::Uuid::new_v4()));
        let first = materialize_bundle(&root).unwrap();
        assert_eq!(first, root.join(BUNDLE_VERSION));
        for skill in BUNDLED_SKILLS {
            for file in skill.files {
                assert_eq!(fs::read(first.join(skill.name).join(file.path)).unwrap(), file.bytes);
            }
        }
        // Already complete: reused as it is.
        assert_eq!(materialize_bundle(&root).unwrap(), first);
        // An older version is kept once, anything older goes.
        for (age, name) in [(3, "old-1"), (2, "old-2")] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(COMPLETE_MARKER), name).unwrap();
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age * 3600);
            fs::File::options().write(true).open(dir.join(COMPLETE_MARKER)).unwrap().set_modified(when).unwrap();
            fs::File::open(&dir).unwrap().set_modified(when).ok();
        }
        prune_versions(&root);
        assert!(root.join(BUNDLE_VERSION).is_dir());
        assert_eq!(root.join("old-1").exists() as u8 + root.join("old-2").exists() as u8, 1);
        fs::remove_dir_all(root).unwrap();
    }

    fn skill(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir.join(name)).unwrap();
        fs::write(dir.join(name).join(SKILL_FILE), body).unwrap();
    }

    #[test]
    fn shared_skills_reach_the_plugin_once() {
        let root = std::env::temp_dir().join(format!("dragent-skills-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let project = root.join("repo");
        let plugin = root.join("plugin");
        fs::create_dir_all(project.join(".git")).unwrap();
        skill(&home.join(".agents/skills"), "review", "global");
        skill(&home.join(".agents/skills"), "deploy", "global");
        skill(&project.join(".agents/skills"), "review", "project");
        skill(&project.join(".dragent/skills"), "lint", "project");
        skill(&home.join(".claude/skills"), "deploy", "claude's own");
        fs::create_dir_all(home.join(".agents/skills/notes")).unwrap();

        let linked = link_into_claude_plugin(&plugin, &project.join("src"), &home).unwrap();

        let skills = plugin.join("skills");
        assert_eq!(linked, 2);
        assert_eq!(fs::read_to_string(skills.join("review/SKILL.md")).unwrap(), "project");
        assert_eq!(fs::read_to_string(skills.join("lint/SKILL.md")).unwrap(), "project");
        assert!(!skills.join("deploy").exists(), "Claude Code already has deploy");
        assert!(!skills.join("notes").exists(), "a folder without SKILL.md isn't a skill");

        fs::remove_dir_all(home.join(".agents/skills")).unwrap();
        fs::remove_dir_all(project.join(".agents/skills")).unwrap();
        fs::remove_dir_all(project.join(".dragent/skills")).unwrap();
        assert_eq!(link_into_claude_plugin(&plugin, &project, &home).unwrap(), 0);
        assert!(!skills.exists(), "a relaunch drops skills that were removed");
        fs::remove_dir_all(&root).unwrap();
    }
}
