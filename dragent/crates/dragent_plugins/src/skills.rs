//! A plugin's skills in the shared skills folder (`~/.agents/skills`), where
//! Dragent's agent and other agents find them.
//!
//! Ownership is per file: the record keeps the hash of what the plugin
//! wrote. A file is replaced or removed only while it still has that hash;
//! one the person edited or deleted stays as they left it. A skill folder
//! the plugin doesn't own (the person's, another tool's, Dragent's own
//! names) is never touched.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::manifest::Manifest;

/// Skill folder → file → hash of what the plugin wrote.
pub type Owned = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `skill/file` written or replaced.
    pub written: Vec<String>,
    /// Files the person edited or deleted, left as they are.
    pub kept: Vec<String>,
    pub removed: Vec<String>,
    /// Skills not placed, with why.
    pub refused: Vec<String>,
}

pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn file_hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| hash(&bytes))
}

/// No link anywhere from `root` down to `path`: writing through one could
/// land outside the skills folder.
fn through_link(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    let mut at = root.to_path_buf();
    for part in relative.components() {
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return true;
        }
    }
    false
}

fn write(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    if through_link(root, path) {
        return Err(format!("{} goes through a link: left alone", path.display()));
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension("dragent-incoming");
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

/// Remove the folders of `skill` left empty, deepest first.
fn prune(root: &Path, skill: &str) {
    fn empty_out(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut empty = true;
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if !(is_dir && empty_out(&path)) {
                empty = false;
            }
        }
        empty && std::fs::remove_dir(dir).is_ok()
    }
    let folder = root.join(skill);
    if !through_link(root, &folder) {
        empty_out(&folder);
    }
}

/// Take out what this plugin placed for `skill` and still has the hash it
/// wrote; keep everything else.
fn take_out(root: &Path, skill: &str, files: &BTreeMap<String, String>, report: &mut Report) {
    for (file, recorded) in files {
        let path = root.join(skill).join(file);
        match file_hash(&path) {
            Some(current) if &current == recorded && !through_link(root, &path) => {
                if std::fs::remove_file(&path).is_ok() {
                    report.removed.push(format!("{skill}/{file}"));
                }
            }
            Some(_) => report.kept.push(format!("{skill}/{file}")),
            None => {}
        }
    }
    prune(root, skill);
}

/// Place the skills of the package at `package` under `root`, given what
/// this plugin placed before. `reserved` says which names aren't for
/// plugins (Dragent's own, other plugins'). Answers what's owned now.
pub fn provision(
    root: &Path,
    package: &Path,
    manifest: &Manifest,
    previous: &Owned,
    reserved: &dyn Fn(&str) -> bool,
) -> (Owned, Report) {
    let mut owned = Owned::new();
    let mut report = Report::default();
    for skill in &manifest.skills {
        let source = package.join(skill.trim_start_matches("./"));
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let before = previous.get(&name);
        let target = root.join(&name);
        if before.is_none() {
            if reserved(&name) {
                report.refused.push(format!("{name}: that name belongs to Dragent or another plugin"));
                continue;
            }
            if std::fs::symlink_metadata(&target).is_ok() {
                report.refused.push(format!(
                    "{name}: a skill of that name is already in {} (someone else's): left alone",
                    root.display()
                ));
                continue;
            }
        }
        let Ok((files, _)) = crate::package::files(&source) else {
            report.refused.push(format!("{name}: its files couldn't be read"));
            continue;
        };
        // SKILL.md last, so a half-written skill is never picked up.
        let mut files = files;
        files.sort_by_key(|file| file == "SKILL.md");
        let mut mine = BTreeMap::new();
        for file in &files {
            let Ok(bytes) = std::fs::read(source.join(file)) else {
                continue;
            };
            let new = hash(&bytes);
            let path = target.join(file);
            let recorded = before.and_then(|files| files.get(file));
            let current = file_hash(&path);
            let ours = match (&current, recorded) {
                (None, Some(_)) => {
                    // The person deleted it: that stands.
                    report.kept.push(format!("{name}/{file}"));
                    continue;
                }
                (None, None) => true,
                (Some(current), Some(recorded)) => current == recorded || *current == new,
                (Some(current), None) => *current == new,
            };
            if !ours {
                report.kept.push(format!("{name}/{file}"));
                if let Some(recorded) = recorded {
                    // Still the plugin's file, edited: keep watching it.
                    mine.insert(file.clone(), recorded.clone());
                }
                continue;
            }
            if current.as_deref() != Some(new.as_str()) {
                match write(root, &path, &bytes) {
                    Ok(()) => report.written.push(format!("{name}/{file}")),
                    Err(error) => {
                        report.refused.push(format!("{name}/{file}: {error}"));
                        continue;
                    }
                }
            }
            mine.insert(file.clone(), new);
        }
        // Files the new version no longer has.
        if let Some(before) = before {
            let gone: BTreeMap<String, String> = before
                .iter()
                .filter(|(file, _)| !files.contains(file))
                .map(|(file, hash)| (file.clone(), hash.clone()))
                .collect();
            take_out(root, &name, &gone, &mut report);
        }
        owned.insert(name, mine);
    }
    // Skills the new version no longer has.
    for (name, files) in previous {
        if !owned.contains_key(name) {
            take_out(root, name, files, &mut report);
        }
    }
    (owned, report)
}

/// Take a plugin's skills out of `root`, keeping what the person changed.
/// Answers what the plugin still owns: the files the person edited or
/// deleted, which stay as they left them when its skills are placed again.
pub fn unprovision(root: &Path, previous: &Owned) -> (Owned, Report) {
    let mut report = Report::default();
    let mut left = Owned::new();
    for (name, files) in previous {
        take_out(root, name, files, &mut report);
        let still = files
            .iter()
            .filter(|(file, _)| !report.removed.contains(&format!("{name}/{file}")))
            .map(|(file, hash)| (file.clone(), hash.clone()))
            .collect();
        left.insert(name.clone(), still);
    }
    (left, report)
}

/// `owned` without the files missing from `root`, so placing the skills
/// again puts them back (for an explicit `sync`; an update leaves a file the
/// person deleted deleted).
pub fn forget_missing(root: &Path, owned: &Owned) -> Owned {
    owned
        .iter()
        .map(|(name, files)| {
            let present = files
                .iter()
                .filter(|(file, _)| std::fs::symlink_metadata(root.join(name).join(file)).is_ok())
                .map(|(file, hash)| (file.clone(), hash.clone()))
                .collect();
            (name.clone(), present)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::tests::sample;

    fn place(root: &Path, package: &Path, previous: &Owned) -> (Owned, Report) {
        let manifest = Manifest::read(package).unwrap();
        provision(root, package, &manifest, previous, &|name| name.starts_with("dragent"))
    }

    #[test]
    fn updates_and_removal_leave_the_persons_edits_alone() {
        let package = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        sample(package.path(), "1.0.0", "First version.");
        let (owned, report) = place(root.path(), package.path(), &Owned::new());
        assert_eq!(report.written.len(), 2);
        let skill_md = root.path().join("acme-tickets/SKILL.md");
        assert!(std::fs::read_to_string(&skill_md).unwrap().contains("First version."));

        // The person edits the reference; a new version changes both files.
        let fields = root.path().join("acme-tickets/references/fields.md");
        std::fs::write(&fields, "My own notes\n").unwrap();
        sample(package.path(), "2.0.0", "Second version.");
        std::fs::write(package.path().join("skills/acme-tickets/references/fields.md"), "Fields v2\n").unwrap();
        let (owned, report) = place(root.path(), package.path(), &owned);
        assert!(std::fs::read_to_string(&skill_md).unwrap().contains("Second version."));
        assert_eq!(std::fs::read_to_string(&fields).unwrap(), "My own notes\n");
        assert_eq!(report.kept, vec!["acme-tickets/references/fields.md"]);

        // Taking them out (switching the plugin off) removes only what's
        // still as the plugin wrote it; placing them again restores that
        // and still leaves the person's edit.
        let (left, report) = unprovision(root.path(), &owned);
        assert_eq!(report.removed, vec!["acme-tickets/SKILL.md"]);
        assert!(!skill_md.exists());
        assert_eq!(std::fs::read_to_string(&fields).unwrap(), "My own notes\n");
        let (owned, report) = place(root.path(), package.path(), &left);
        assert_eq!(report.written, vec!["acme-tickets/SKILL.md"]);
        assert_eq!(report.kept, vec!["acme-tickets/references/fields.md"]);
        assert!(std::fs::read_to_string(&skill_md).unwrap().contains("Second version."));

        // All gone (the plugin off, the person's file deleted too): placed
        // again whole.
        let (left, _) = unprovision(root.path(), &owned);
        std::fs::remove_file(&fields).unwrap();
        let (_, report) = place(root.path(), package.path(), &forget_missing(root.path(), &left));
        assert_eq!(report.written.len(), 2, "{report:?}");
        assert_eq!(std::fs::read_to_string(&fields).unwrap(), "Fields v2\n");
    }

    #[test]
    fn someone_elses_skill_and_dragents_names_are_never_overwritten() {
        let package = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        sample(package.path(), "1.0.0", "x");
        std::fs::create_dir_all(root.path().join("acme-tickets")).unwrap();
        std::fs::write(root.path().join("acme-tickets/SKILL.md"), "Mine\n").unwrap();
        let (owned, report) = place(root.path(), package.path(), &Owned::new());
        assert!(owned.is_empty());
        assert!(report.refused[0].contains("someone else's"), "{report:?}");
        assert_eq!(std::fs::read_to_string(root.path().join("acme-tickets/SKILL.md")).unwrap(), "Mine\n");
    }

    #[test]
    fn a_deleted_file_stays_deleted_and_links_are_not_followed() {
        let package = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        sample(package.path(), "1.0.0", "x");
        let (owned, _) = place(root.path(), package.path(), &Owned::new());
        std::fs::remove_file(root.path().join("acme-tickets/references/fields.md")).unwrap();
        let (owned, report) = place(root.path(), package.path(), &owned);
        assert!(!root.path().join("acme-tickets/references/fields.md").exists());
        assert!(report.kept.contains(&"acme-tickets/references/fields.md".to_string()));
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::remove_dir_all(root.path().join("acme-tickets/references")).ok();
            std::os::unix::fs::symlink(outside.path(), root.path().join("acme-tickets/references")).unwrap();
            let mut previous = owned.clone();
            previous.get_mut("acme-tickets").unwrap().remove("references/fields.md");
            let (_, report) = place(root.path(), package.path(), &previous);
            assert!(report.refused.iter().any(|line| line.contains("link")), "{report:?}");
            assert!(!outside.path().join("fields.md").exists());
        }
    }
}
