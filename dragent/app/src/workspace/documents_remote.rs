//! The document layer for remote projects: the same operations as the local
//! `documents_*` functions, served by the project's machine. Each local
//! function delegates here when its root is a remote stand-in path, so the
//! Files tree, editor, and explorer actions are the same UI for both.
use super::documents_io::{is_visual_asset, FileFingerprint, ReadOnlyDocument};
use super::documents_entries::validate_entry_name;
use super::project_documents::{IGNORED_DIRECTORIES, IGNORED_FILES};
use crate::remote_fs;
use dragent_workspace_model::{DocumentEntry, DocumentRef};
use std::path::{Component, Path, PathBuf};

fn inside(relative: &Path) -> Result<(), String> {
    let escapes = relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes {
        Err("Entry is outside the project".into())
    } else {
        Ok(())
    }
}

/// Immediate children of `current`, as explorer entries relative to `root`.
pub(crate) fn visit_children(root: &Path, current: &Path, entries: &mut Vec<DocumentEntry>) {
    let listing = match remote_fs::list(current) {
        Ok(listing) => listing,
        Err(error) => {
            log::warn!("[remote-files] list {} failed: {error}", current.display());
            return;
        }
    };
    let base = current.strip_prefix(root).unwrap_or(Path::new("")).to_path_buf();
    for entry in listing {
        let skipped = if entry.directory {
            IGNORED_DIRECTORIES.contains(&entry.path.as_str())
        } else {
            IGNORED_FILES.contains(&entry.path.as_str())
        };
        if skipped {
            continue;
        }
        let Some(document) = DocumentRef::from_relative_path(base.join(&entry.path)) else {
            continue;
        };
        entries.push(DocumentEntry {
            document,
            is_directory: entry.directory,
            byte_size: (!entry.directory).then_some(entry.bytes),
            is_ignored: false,
        });
    }
}

pub(crate) fn fingerprint(bytes: &[u8]) -> FileFingerprint {
    FileFingerprint::from_content(bytes, None)
}

pub(crate) fn read_preview(root: &Path, relative_path: &Path) -> ReadOnlyDocument {
    let failure = |message: String| ReadOnlyDocument {
        relative_path: relative_path.to_path_buf(),
        text: String::new(),
        visual_path: None,
        error: Some(message),
        fingerprint: None,
    };
    if inside(relative_path).is_err() {
        return failure("File is outside the project".into());
    }
    let path = root.join(relative_path);
    if is_visual_asset(&path) {
        return failure("Images on a remote machine can't be previewed yet.".into());
    }
    let bytes = match remote_fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return failure(format!("File could not be read: {error}")),
    };
    if bytes.contains(&0) {
        return failure("Binary files cannot be previewed".into());
    }
    let fingerprint = Some(fingerprint(&bytes));
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    };
    ReadOnlyDocument {
        relative_path: relative_path.to_path_buf(),
        text,
        visual_path: None,
        error: None,
        fingerprint,
    }
}

pub(crate) fn save_text(
    root: &Path,
    relative_path: &Path,
    text: &str,
    expected: Option<FileFingerprint>,
) -> Result<FileFingerprint, String> {
    inside(relative_path)?;
    let path = root.join(relative_path);
    if let Some(expected) = expected {
        let current = remote_fs::read(&path).map_err(|e| format!("Read file before save: {e}"))?;
        if fingerprint(&current) != expected {
            return Err("File changed on disk. Compare, overwrite, or revert before saving.".into());
        }
    }
    remote_fs::write(&path, text.as_bytes())?;
    Ok(fingerprint(text.as_bytes()))
}

pub(crate) fn create_entry(
    root: &Path,
    relative_parent: &Path,
    name: &str,
    directory: bool,
) -> Result<PathBuf, String> {
    if !relative_parent.as_os_str().is_empty() {
        inside(relative_parent)?;
    }
    let name = validate_entry_name(name)?;
    let relative = relative_parent.join(name);
    let destination = root.join(&relative);
    if remote_fs::exists(&destination) {
        return Err(format!("{name} already exists"));
    }
    if directory {
        remote_fs::create_dir_all(&destination).map_err(|e| format!("Create folder: {e}"))?;
    } else {
        remote_fs::create_file(&destination).map_err(|e| format!("Create file: {e}"))?;
    }
    Ok(relative)
}

pub(crate) fn rename_entry_destination(root: &Path, source: &Path, name: &str) -> Result<PathBuf, String> {
    inside(source)?;
    let name = validate_entry_name(name)?;
    let destination = source
        .parent()
        .ok_or_else(|| "Project root cannot be renamed".to_string())?
        .join(name);
    if destination != source && remote_fs::exists(&root.join(&destination)) {
        return Err(format!("{name} already exists"));
    }
    Ok(destination)
}

pub(crate) fn rename_entry(root: &Path, source: &Path, name: &str) -> Result<PathBuf, String> {
    let destination = rename_entry_destination(root, source, name)?;
    if destination == source {
        return Ok(destination);
    }
    remote_fs::rename(&root.join(source), &root.join(&destination))
        .map_err(|e| format!("Rename entry: {e}"))?;
    Ok(destination)
}

fn unique_destination(root: &Path, directory: &Path, name: &str, is_file: bool) -> PathBuf {
    let direct = directory.join(name);
    if !remote_fs::exists(&root.join(&direct)) {
        return direct;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("item");
    let extension = is_file.then(|| path.extension().and_then(|e| e.to_str())).flatten();
    for index in 1usize.. {
        let suffix = if index == 1 { " copy".to_string() } else { format!(" copy {index}") };
        let mut candidate = format!("{stem}{suffix}");
        if let Some(extension) = extension {
            candidate.push('.');
            candidate.push_str(extension);
        }
        let candidate = directory.join(candidate);
        if !remote_fs::exists(&root.join(&candidate)) {
            return candidate;
        }
    }
    unreachable!("copy destination search is unbounded")
}

pub(crate) fn copy_entry(root: &Path, source: &Path, directory: &Path) -> Result<PathBuf, String> {
    inside(source)?;
    if !directory.as_os_str().is_empty() {
        inside(directory)?;
    }
    if directory.starts_with(source) {
        return Err("A folder cannot be copied into itself".into());
    }
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Project root cannot be copied".to_string())?;
    let is_file = !remote_fs::is_dir(&root.join(source));
    let destination = unique_destination(root, directory, name, is_file);
    remote_fs::copy(&root.join(source), &root.join(&destination)).map_err(|e| format!("Copy entry: {e}"))?;
    Ok(destination)
}

pub(crate) fn move_entry(root: &Path, source: &Path, directory: &Path) -> Result<PathBuf, String> {
    inside(source)?;
    if !directory.as_os_str().is_empty() {
        inside(directory)?;
    }
    if directory.starts_with(source) {
        return Err("A folder cannot be moved into itself".into());
    }
    let name = source
        .file_name()
        .ok_or_else(|| "Project root cannot be moved".to_string())?;
    let destination = directory.join(name);
    if destination == source {
        return Ok(destination);
    }
    if remote_fs::exists(&root.join(&destination)) {
        return Err(format!("{} already exists", destination.display()));
    }
    remote_fs::rename(&root.join(source), &root.join(&destination)).map_err(|e| format!("Move entry: {e}"))?;
    Ok(destination)
}

pub(crate) fn delete_entry(root: &Path, relative_path: &Path) -> Result<(), String> {
    inside(relative_path)?;
    if relative_path.as_os_str().is_empty() {
        return Err("Project root cannot be deleted".into());
    }
    remote_fs::remove(&root.join(relative_path)).map_err(|e| format!("Delete entry: {e}"))
}

/// Remote folders change without local filesystem events; poll instead.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn spawn_tree_poller(
    root: PathBuf,
    tx: async_channel::Sender<super::documents_entries::TreeWatcherEvent>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let poll_stop = stop.clone();
    std::thread::Builder::new()
        .name("remote-tree-poller".into())
        .spawn(move || {
            let _ = tx.try_send(super::documents_entries::TreeWatcherEvent {
                root: root.clone(),
                changed_paths: Vec::new(),
            });
            while !poll_stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(5));
                if poll_stop.load(Ordering::Relaxed) || tx.is_closed() {
                    return;
                }
                let _ = tx.try_send(super::documents_entries::TreeWatcherEvent {
                    root: root.clone(),
                    changed_paths: Vec::new(),
                });
            }
        })
        .expect("spawn remote tree poller");
    stop
}
