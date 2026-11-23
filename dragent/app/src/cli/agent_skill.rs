//! Dragent’s focused skill packages in other agents’ shared folders,
//! teaching the `dragent` command: `~/.agents/skills` (Codex, OpenCode,
//! Cursor, Gemini) and `~/.qwen/skills` (Qwen, once it's set up). Claude Code
//! and Dragent's own agent get the same packages without any of this.
//!
//! On unless the user turns it off. Ownership is recorded per file, with the
//! hash of what Dragent wrote, so:
//! - a file Dragent wrote and nobody changed updates with a newer Dragent;
//! - a file the user edited or created is never overwritten or removed;
//! - a file (or whole skill) the user deleted stays deleted on background
//!   syncs and comes back only when they turn the skills on again;
//! - turning them off removes only files still exactly as Dragent wrote them.
//!
//! A skill whose SKILL.md the user customized is theirs as a whole: its
//! resources are left as they are too, so their instructions keep pointing
//! at the files they were written against. New or changed files are staged
//! first and moved into place with SKILL.md last, so agents never read one
//! build's instructions beside another's scripts.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const RECORD_FILE: &str = "agent-skill.json";
const COMPUTER_RECORD_FILE: &str = "computer-skill.json";
const SKILL_FILE: &str = "SKILL.md";
const STAGING_DIR: &str = ".dragent-staging";

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    #[serde(default = "on")]
    enabled: bool,
    /// Every file Dragent wrote, with the hash of what it wrote. Kept under
    /// its original key, which earlier builds read.
    #[serde(default, rename = "written")]
    files: BTreeMap<PathBuf, String>,
}

fn on() -> bool {
    true
}

impl Default for Record {
    fn default() -> Self {
        Self {
            enabled: true,
            files: BTreeMap::new(),
        }
    }
}

/// What a sync did, for Settings to report.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// Files that aren't Dragent's (edited or added by the user): left as they are.
    pub kept: Vec<PathBuf>,
}

impl Outcome {
    fn extend(&mut self, other: Outcome) {
        self.written.extend(other.written);
        self.removed.extend(other.removed);
        self.kept.extend(other.kept);
    }
}

/// A skill package as the installer sees it.
struct Package<'a> {
    name: &'a str,
    /// (relative path, bytes, executable)
    files: Vec<(&'a str, &'a [u8], bool)>,
}

impl Package<'static> {
    fn bundled(skill: &'static dragent_host::agent_skills::BundledSkill) -> Self {
        Self {
            name: skill.name,
            files: skill
                .files
                .iter()
                .map(|file| (file.path, file.bytes, file.executable))
                .collect(),
        }
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn record_path() -> PathBuf {
    dragent_paths::home().join(RECORD_FILE)
}

fn load(path: &Path) -> Record {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(path: &Path, record: &Record) {
    if let Ok(bytes) = serde_json::to_vec_pretty(record) {
        if let Err(error) = std::fs::write(path, bytes) {
            log::warn!("[cli] couldn't save {}: {error}", path.display());
        }
    }
}

/// The shared skill folders other agents read, for this home folder.
fn roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![home.join(".agents/skills")];
    if home.join(".qwen").is_dir() {
        roots.push(home.join(".qwen/skills"));
    }
    roots
}

pub fn enabled() -> bool {
    load(&record_path()).enabled
}

/// Bring the folders in line with the setting (at startup). A Dev build
/// never writes into the shared skill folders other agents read.
pub fn sync() -> Outcome {
    if dragent_paths::is_dev() {
        log::info!("[cli] Dev build: leaving shared skill folders alone");
        return Outcome::default();
    }
    let Some(home) = dirs::home_dir() else {
        return Outcome::default();
    };
    let path = record_path();
    let mut record = load(&path);
    let outcome = apply_suite(&mut record, &home, false);
    let computer_path = dragent_paths::home().join(COMPUTER_RECORD_FILE);
    if computer_path.is_file() {
        let mut computer = load(&computer_path);
        apply_computer(&mut computer, &home, false);
        save(&computer_path, &computer);
    }
    save(&path, &record);
    outcome
}

/// The user turned it on or off. A Dev build records the choice in its own
/// home but never writes into the shared skill folders.
pub fn set_enabled(enabled: bool) -> Outcome {
    let path = record_path();
    let mut record = load(&path);
    record.enabled = enabled;
    if dragent_paths::is_dev() {
        log::info!("[cli] Dev build: leaving shared skill folders alone");
        save(&path, &record);
        return Outcome::default();
    }
    let Some(home) = dirs::home_dir() else {
        return Outcome::default();
    };
    let outcome = apply_suite(&mut record, &home, true);
    save(&path, &record);
    outcome
}

/// Desktop guidance keeps its separate opt-in record and Settings control.
fn apply_suite(record: &mut Record, home: &Path, asked: bool) -> Outcome {
    let mut outcome = Outcome::default();
    for skill in dragent_host::agent_skills::BUNDLED_SKILLS {
        if skill.name == dragent_host::agent_skills::COMPUTER_SKILL_NAME {
            continue;
        }
        outcome.extend(apply_package(record, home, &Package::bundled(skill), asked));
    }
    outcome
}

fn apply_computer(record: &mut Record, home: &Path, asked: bool) -> Outcome {
    match dragent_host::agent_skills::bundled(dragent_host::agent_skills::COMPUTER_SKILL_NAME) {
        Some(skill) => apply_package(record, home, &Package::bundled(skill), asked),
        None => Outcome::default(),
    }
}

/// `asked`: the user just chose this, so content they deleted earlier comes
/// back when they turn it on.
fn apply_package(record: &mut Record, home: &Path, package: &Package, asked: bool) -> Outcome {
    let mut outcome = Outcome::default();
    for root in roots(home) {
        let dir = root.join(package.name);
        if record.enabled {
            install(record, &dir, package, asked, &mut outcome);
        } else {
            uninstall(record, &dir, package, &mut outcome);
        }
    }
    outcome
}

/// How a file on disk relates to what Dragent would write there.
#[derive(Debug, PartialEq)]
enum FileState {
    /// Not there, and Dragent never wrote it.
    Absent,
    /// Dragent wrote it; it's gone now: the user deleted it.
    Deleted,
    /// Exactly what Dragent would write now.
    Current,
    /// What Dragent wrote earlier, untouched since.
    Stale,
    /// Edited or created by someone else, or behind a link.
    Theirs,
}

fn state(record: &Record, path: &Path, wanted: Option<&[u8]>, root: &Path) -> FileState {
    if !plain_path(root, path) {
        return FileState::Theirs;
    }
    let recorded = record.files.get(path);
    match std::fs::read(path) {
        Err(_) if recorded.is_some() => FileState::Deleted,
        Err(_) => FileState::Absent,
        Ok(bytes) => {
            let found = hash(&bytes);
            if wanted.is_some_and(|wanted| hash(wanted) == found) {
                FileState::Current
            } else if recorded == Some(&found) {
                FileState::Stale
            } else {
                FileState::Theirs
            }
        }
    }
}

/// Whether `path` and every folder between `root`'s skill folder and it is
/// a plain file or folder: Dragent never writes through a link.
fn plain_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    true
}

fn install(record: &mut Record, dir: &Path, package: &Package, asked: bool, outcome: &mut Outcome) {
    let Some(root) = dir.parent() else {
        return;
    };
    let skill_md = dir.join(SKILL_FILE);
    let entry = package
        .files
        .iter()
        .find(|(path, ..)| *path == SKILL_FILE)
        .map(|(_, bytes, _)| *bytes);
    match state(record, &skill_md, entry, root) {
        // Their own skill under this name, or one they customized: theirs
        // as a whole, resources included.
        FileState::Theirs => {
            forget_under(record, dir);
            outcome.kept.push(skill_md);
            return;
        }
        // They removed the skill: it stays removed until they ask.
        FileState::Deleted if !asked => return,
        _ => {}
    }

    let mut writes: Vec<(PathBuf, &[u8], bool)> = Vec::new();
    for (relative, bytes, executable) in &package.files {
        let target = dir.join(relative);
        match state(record, &target, Some(bytes), root) {
            FileState::Current => {
                record.files.insert(target, hash(bytes));
            }
            FileState::Theirs => {
                record.files.remove(&target);
                outcome.kept.push(target);
            }
            FileState::Deleted if !asked => {}
            FileState::Absent | FileState::Deleted | FileState::Stale => {
                writes.push((target, bytes, *executable));
            }
        }
    }

    // Files an earlier Dragent wrote that this package no longer has.
    let wanted: Vec<PathBuf> = package.files.iter().map(|(p, ..)| dir.join(p)).collect();
    let leftovers: Vec<PathBuf> = record
        .files
        .keys()
        .filter(|path| path.starts_with(dir) && !wanted.contains(path))
        .cloned()
        .collect();
    for path in leftovers {
        match state(record, &path, None, root) {
            FileState::Stale => {
                if std::fs::remove_file(&path).is_ok() {
                    outcome.removed.push(path.clone());
                }
            }
            FileState::Theirs => outcome.kept.push(path.clone()),
            _ => {}
        }
        record.files.remove(&path);
    }

    if writes.is_empty() {
        remove_empty_dirs(dir);
        return;
    }
    match write_staged(dir, &writes) {
        Ok(()) => {
            for (target, bytes, _) in writes {
                record.files.insert(target.clone(), hash(bytes));
                outcome.written.push(target);
            }
        }
        Err(error) => log::warn!("[cli] installing {}: {error}", dir.display()),
    }
    remove_empty_dirs(dir);
}

/// Write every file to a staging folder first, then move them into place
/// with SKILL.md last.
fn write_staged(dir: &Path, writes: &[(PathBuf, &[u8], bool)]) -> Result<(), String> {
    let staging = dir.join(STAGING_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    let staged: Vec<(PathBuf, &PathBuf)> = writes
        .iter()
        .enumerate()
        .map(|(index, (target, ..))| (staging.join(index.to_string()), target))
        .collect();
    let prepare = || -> std::io::Result<()> {
        std::fs::create_dir_all(&staging)?;
        for ((temp, _), (_, bytes, executable)) in staged.iter().zip(writes) {
            std::fs::write(temp, bytes)?;
            dragent_host::agent_skills::set_executable(temp, *executable)?;
        }
        Ok(())
    };
    if let Err(error) = prepare() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("staging {}: {error}", dir.display()));
    }
    let mut order: Vec<&(PathBuf, &PathBuf)> = staged.iter().collect();
    order.sort_by_key(|(_, target)| target.file_name().is_some_and(|n| n == SKILL_FILE));
    let mut result = Ok(());
    for (temp, target) in order {
        let moved = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(temp, target));
        if let Err(error) = moved {
            result = Err(format!("write {}: {error}", target.display()));
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn uninstall(record: &mut Record, dir: &Path, package: &Package, outcome: &mut Outcome) {
    let Some(root) = dir.parent() else {
        return;
    };
    let skill_md = dir.join(SKILL_FILE);
    let entry = package
        .files
        .iter()
        .find(|(path, ..)| *path == SKILL_FILE)
        .map(|(_, bytes, _)| *bytes);
    if state(record, &skill_md, entry, root) == FileState::Theirs {
        forget_under(record, dir);
        outcome.kept.push(skill_md);
        return;
    }
    let mut paths: Vec<PathBuf> = package.files.iter().map(|(p, ..)| dir.join(p)).collect();
    paths.extend(record.files.keys().filter(|p| p.starts_with(dir)).cloned());
    paths.sort();
    paths.dedup();
    for path in paths {
        let current = package
            .files
            .iter()
            .find(|(p, ..)| dir.join(p) == path)
            .map(|(_, bytes, _)| *bytes);
        match state(record, &path, current, root) {
            // Only what Dragent recorded writing, still as it wrote it.
            FileState::Current | FileState::Stale if record.files.contains_key(&path) => {
                if std::fs::remove_file(&path).is_ok() {
                    outcome.removed.push(path.clone());
                }
            }
            FileState::Theirs => outcome.kept.push(path.clone()),
            _ => {}
        }
        record.files.remove(&path);
    }
    remove_empty_dirs(dir);
}

fn forget_under(record: &mut Record, dir: &Path) {
    record.files.retain(|path, _| !path.starts_with(dir));
}

/// Remove folders left empty under (and including) `dir`, never one that
/// still holds anything.
fn remove_empty_dirs(dir: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

pub(crate) fn computer_enabled() -> bool {
    let path = dragent_paths::home().join(COMPUTER_RECORD_FILE);
    path.is_file() && load(&path).enabled
}

pub(crate) fn set_computer_enabled(enabled: bool) -> Outcome {
    let path = dragent_paths::home().join(COMPUTER_RECORD_FILE);
    let mut record = load(&path);
    record.enabled = enabled;
    if dragent_paths::is_dev() {
        log::info!("[cli] Dev build: leaving shared skill folders alone");
        save(&path, &record);
        return Outcome::default();
    }
    let Some(home) = dirs::home_dir() else {
        return Outcome::default();
    };
    let outcome = apply_computer(&mut record, &home, true);
    save(&path, &record);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package<'a>(files: &[(&'a str, &'a str)]) -> Package<'a> {
        Package {
            name: "demo",
            files: files
                .iter()
                .map(|(path, body)| (*path, body.as_bytes(), path.starts_with("scripts/")))
                .collect(),
        }
    }

    const V1: &[(&str, &str)] = &[
        ("SKILL.md", "---\nname: demo\ndescription: one\n---\nv1\n"),
        ("references/guide.md", "guide v1"),
        ("scripts/collect.sh", "#!/bin/sh\necho v1\n"),
        ("references/old.md", "only in v1"),
    ];
    const V2: &[(&str, &str)] = &[
        ("SKILL.md", "---\nname: demo\ndescription: two\n---\nv2\n"),
        ("references/guide.md", "guide v2"),
        ("scripts/collect.sh", "#!/bin/sh\necho v2\n"),
        ("references/new.md", "new in v2"),
    ];

    fn dir(home: &Path) -> PathBuf {
        home.join(".agents/skills/demo")
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn installs_whole_packages_and_updates_only_untouched_files() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut record = Record::default();
        let first = apply_package(&mut record, home, &package(V1), false);
        assert_eq!(first.written.len(), 4);
        assert_eq!(read(dir(home).join("references/guide.md")), "guide v1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir(home).join("scripts/collect.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "helpers are executable");
        }
        // The user edits one resource and adds a file of their own.
        std::fs::write(dir(home).join("references/guide.md"), "my notes").unwrap();
        std::fs::write(dir(home).join("references/mine.md"), "mine").unwrap();

        let update = apply_package(&mut record, home, &package(V2), false);
        assert_eq!(read(dir(home).join("SKILL.md")), V2[0].1);
        assert_eq!(read(dir(home).join("scripts/collect.sh")), V2[2].1);
        assert_eq!(read(dir(home).join("references/new.md")), "new in v2");
        assert_eq!(read(dir(home).join("references/guide.md")), "my notes", "customized resource kept");
        assert!(update.kept.contains(&dir(home).join("references/guide.md")));
        assert!(!dir(home).join("references/old.md").exists(), "a file the package dropped goes");
        assert_eq!(read(dir(home).join("references/mine.md")), "mine");
        assert!(!dir(home).join(STAGING_DIR).exists());

        // Turning it off removes only Dragent's untouched files.
        record.enabled = false;
        let removed = apply_package(&mut record, home, &package(V2), true);
        assert!(removed.removed.contains(&dir(home).join("SKILL.md")));
        assert_eq!(read(dir(home).join("references/guide.md")), "my notes");
        assert_eq!(read(dir(home).join("references/mine.md")), "mine");
        assert!(!dir(home).join("scripts").exists(), "an emptied folder goes");
        assert!(record.files.is_empty());
    }

    #[test]
    fn deletions_stay_until_the_user_turns_skills_on_again() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut record = Record::default();
        apply_package(&mut record, home, &package(V1), false);
        // One resource deleted: background syncs leave it deleted.
        std::fs::remove_file(dir(home).join("references/guide.md")).unwrap();
        apply_package(&mut record, home, &package(V1), false);
        assert!(!dir(home).join("references/guide.md").exists());
        // The whole skill deleted: nothing comes back on its own, not even
        // files a newer package adds.
        std::fs::remove_dir_all(dir(home)).unwrap();
        assert!(apply_package(&mut record, home, &package(V2), false).written.is_empty());
        assert!(!dir(home).exists());
        // Asking again restores the complete package.
        apply_package(&mut record, home, &package(V2), true);
        for (path, body) in V2 {
            assert_eq!(read(dir(home).join(path)), *body);
        }
    }

    #[test]
    fn a_customized_skill_is_left_whole() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut record = Record::default();
        apply_package(&mut record, home, &package(V1), false);
        std::fs::write(dir(home).join("SKILL.md"), "---\nname: demo\ndescription: mine\n---\nmine\n").unwrap();
        let outcome = apply_package(&mut record, home, &package(V2), false);
        assert!(outcome.written.is_empty());
        assert_eq!(read(dir(home).join("references/guide.md")), "guide v1", "its resources aren't swapped under it");
        record.enabled = false;
        assert!(apply_package(&mut record, home, &package(V1), true).removed.is_empty());
        assert!(dir(home).join("SKILL.md").is_file());
        assert!(dir(home).join("references/guide.md").is_file());
    }

    #[test]
    fn a_users_own_skill_of_the_same_name_is_untouched() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        std::fs::create_dir_all(dir(home)).unwrap();
        std::fs::write(dir(home).join("SKILL.md"), "theirs").unwrap();
        let mut record = Record::default();
        let outcome = apply_package(&mut record, home, &package(V1), true);
        assert!(outcome.written.is_empty());
        assert_eq!(outcome.kept, vec![dir(home).join("SKILL.md")]);
        assert!(!dir(home).join("references").exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_links() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(home.join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir(home)).unwrap();
        let mut record = Record::default();
        assert!(apply_package(&mut record, home, &package(V1), true).written.is_empty());
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
        // A link to a resource inside a real package is left alone too.
        std::fs::remove_file(dir(home)).unwrap();
        apply_package(&mut record, home, &package(V1), true);
        std::fs::remove_file(dir(home).join("references/guide.md")).unwrap();
        std::fs::write(elsewhere.join("target.md"), "outside").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("target.md"), dir(home).join("references/guide.md")).unwrap();
        apply_package(&mut record, home, &package(V2), true);
        assert_eq!(read(elsewhere.join("target.md")), "outside");
    }

    #[test]
    fn records_from_earlier_builds_carry_over() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let skill_md = dir(home).join("SKILL.md");
        std::fs::create_dir_all(dir(home)).unwrap();
        std::fs::write(&skill_md, "old dragent text").unwrap();
        let old = serde_json::json!({ "enabled": true, "written": { skill_md.to_str().unwrap(): hash(b"old dragent text") } });
        let mut record: Record = serde_json::from_value(old).unwrap();
        apply_package(&mut record, home, &package(V1), false);
        assert_eq!(read(skill_md), V1[0].1, "Dragent's earlier copy updates");
        assert!(dir(home).join("scripts/collect.sh").is_file());
        let saved = serde_json::to_value(&record).unwrap();
        assert!(saved.get("written").is_some(), "earlier builds keep reading the record");
    }

    #[test]
    fn qwen_gets_a_copy_once_it_is_set_up() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert_eq!(roots(home).len(), 1);
        std::fs::create_dir(home.join(".qwen")).unwrap();
        assert_eq!(roots(home)[1], home.join(".qwen/skills"));
        let mut record = Record::default();
        apply_package(&mut record, home, &package(V1), false);
        assert!(home.join(".qwen/skills/demo/references/guide.md").is_file());
    }

    #[test]
    fn the_bundled_suite_installs_without_desktop_guidance() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut record = Record::default();
        apply_suite(&mut record, home, false);
        for skill in dragent_host::agent_skills::BUNDLED_SKILLS {
            let installed = home.join(".agents/skills").join(skill.name);
            if skill.name == dragent_host::agent_skills::COMPUTER_SKILL_NAME {
                assert!(!installed.exists(), "desktop guidance has its own opt-in");
                continue;
            }
            for file in skill.files {
                assert_eq!(std::fs::read(installed.join(file.path)).unwrap(), file.bytes);
            }
        }
        let mut computer = Record::default();
        apply_computer(&mut computer, home, true);
        assert!(home.join(".agents/skills/dragent-computer/SKILL.md").is_file());
    }

    #[test]
    fn dev_builds_write_nothing_into_shared_skill_folders() {
        // Test builds are the Dev edition. Toggling the teaching on and the
        // startup sync must both write nothing into the shared folders,
        // while the choice itself is still recorded in the Dev home.
        if !dragent_paths::is_dev() {
            return;
        } // only meaningful in the Dev edition
        let home = tempfile::tempdir().unwrap();
        let dev_home = tempfile::tempdir().unwrap();
        crate::test_env::with_vars(
            &[
                ("HOME", Some(home.path())),
                (dragent_paths::DEV_HOME_ENV, Some(dev_home.path())),
            ],
            || {
                let outcome = set_enabled(true);
                assert!(outcome.written.is_empty() && outcome.removed.is_empty());
                assert!(enabled());
                let outcome = set_computer_enabled(true);
                assert!(outcome.written.is_empty() && outcome.removed.is_empty());
                let outcome = sync();
                assert!(outcome.written.is_empty() && outcome.removed.is_empty());
                assert!(!home.path().join(".agents/skills").exists());
                assert!(!home.path().join(".qwen").exists());
            },
        );
    }
}
