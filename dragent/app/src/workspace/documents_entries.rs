//! Project document discovery, watching, and entry operations.
//!
//! Moved verbatim; same behavior.

//! Bounded project-tree discovery for the future file surface.

use super::documents_ignore::ignore;
use super::documents_io::{visit_children, visit_recursive};
use super::project_documents::{
    IGNORED_DIRECTORIES, IGNORED_FILES, LAZY_DIRECTORIES, MAX_AUTO_WALK_ENTRIES,
};
use dragent_workspace_model::{DocumentEntry, DocumentRef, ProjectDocumentIndex};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

// Version-control internals and OS thumbnails excluded from the tree —
// never user build output. Build outputs (`node_modules`, `target`,
// `dist`, `build`, `coverage`) are intentionally LISTED: the explorer
// shows everything scanned. The `is_ignored` flag + a future panel toggle
// can hide ignored paths on demand.

pub fn discover(root: &Path) -> ProjectDocumentIndex {
    // Remote trees load level by level, like expanded folders.
    if crate::remote_fs::is_remote(root) {
        return discover_directory(root, Path::new(""));
    }
    // Recursive snapshot: deep files exist in the index without requiring
    // every ancestor to be expanded first. Depth is 5 outside repos and
    // unlimited inside a git repo (bounded by MAX_PROJECT_DOCUMENTS in
    // `replace`).
    let max_depth = if root.join(".git").is_dir() {
        usize::MAX
    } else {
        5
    };
    let mut entries = Vec::new();
    let mut visited = HashSet::new();
    if let Ok(canonical) = root.canonicalize() {
        visited.insert(canonical);
    }
    let mut ignores = ignore::IgnoreStack::for_root(root);
    visit_recursive(
        root,
        root,
        Path::new(""),
        0,
        max_depth,
        &mut visited,
        &mut ignores,
        &mut entries,
    );
    let mut index = ProjectDocumentIndex::default();
    index.replace(entries);
    index
}

/// Rebuild the visible explorer projection: root entries plus the children of
/// directories the user has already expanded. This keeps watcher refreshes
/// bounded while preserving the currently visible tree.
pub fn discover_loaded_directories(
    root: &Path,
    loaded_directories: &[PathBuf],
) -> ProjectDocumentIndex {
    let mut index = discover_directory(root, Path::new(""));
    for relative_directory in loaded_directories {
        if relative_directory.as_os_str().is_empty() {
            continue;
        }
        let children = discover_directory(root, relative_directory);
        index.merge(children.entries().to_vec());
    }
    index
}

#[derive(Clone, Debug)]
pub struct TreeWatcherEvent {
    pub root: PathBuf,
    pub changed_paths: Vec<PathBuf>,
}

/// Start a debounced recursive watcher for the explorer. Unlike the Git
/// watcher, this also works for ordinary non-repository folders.
#[cfg(not(target_family = "wasm"))]
pub fn spawn_tree_watcher(
    root: PathBuf,
    tx: async_channel::Sender<TreeWatcherEvent>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode, Watcher};
    use notify_debouncer_full::NoCache;
    use std::sync::atomic::{AtomicBool, Ordering};

    if crate::remote_fs::is_remote(&root) {
        return super::documents_remote::spawn_tree_poller(root, tx);
    }
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let watcher_stop = stop.clone();
    let watch_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    std::thread::Builder::new()
        .name("project-tree-watcher".into())
        .spawn(move || {
            let (raw_tx, raw_rx) = std::sync::mpsc::channel();
            let mut debouncer = match notify_debouncer_full::new_debouncer_opt::<
                _,
                RecommendedWatcher,
                NoCache,
            >(
                std::time::Duration::from_millis(100),
                None,
                raw_tx,
                NoCache::new(),
                notify_debouncer_full::notify::Config::default(),
            ) {
                Ok(watcher) => watcher,
                Err(error) => {
                    log::warn!("[editor-tree] watcher setup failed root={} error={error}", root.display());
                    return;
                }
            };
            if let Err(error) = debouncer.watch(&watch_root, RecursiveMode::Recursive) {
                log::warn!("[editor-tree] watch failed root={} error={error}", root.display());
                return;
            }
            log::info!("[editor-tree] watching root={}", root.display());
            // FSEvents registration becomes observable shortly after `watch`
            // returns. The ready refresh closes that small startup window
            // without blocking the UI thread.
            std::thread::sleep(std::time::Duration::from_millis(250));
            let _ = tx.try_send(TreeWatcherEvent {
                root: root.clone(),
                changed_paths: Vec::new(),
            });
            while !watcher_stop.load(Ordering::Relaxed) {
                match raw_rx.recv_timeout(std::time::Duration::from_secs(1)) {
                    Ok(Ok(events)) => {
                        let mut changed_paths = events
                            .into_iter()
                            .flat_map(|event| event.event.paths)
                            .filter_map(|path| {
                                path.strip_prefix(&watch_root)
                                    .or_else(|_| path.strip_prefix(&root))
                                    .ok()
                                    .map(Path::to_path_buf)
                            })
                            .filter(|path| {
                                !path.components().any(|component| {
                                    matches!(component, std::path::Component::Normal(name)
                                        if IGNORED_DIRECTORIES.iter().any(|ignored| name == std::ffi::OsStr::new(ignored)))
                                })
                            })
                            .filter(|path| {
                                path.file_name().is_none_or(|name| {
                                    !IGNORED_FILES
                                        .iter()
                                        .any(|ignored| name == std::ffi::OsStr::new(ignored))
                                })
                            })
                            .collect::<Vec<_>>();
                        changed_paths.sort();
                        changed_paths.dedup();
                        if !changed_paths.is_empty() {
                            let _ = tx.try_send(TreeWatcherEvent {
                                root: root.clone(),
                                changed_paths,
                            });
                        }
                    }
                    Ok(Err(errors)) => {
                        log::warn!("[editor-tree] watcher error root={} errors={errors:?}", root.display());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("spawn project tree watcher");
    stop
}

#[cfg(target_family = "wasm")]
pub fn spawn_tree_watcher(
    _root: PathBuf,
    _tx: async_channel::Sender<TreeWatcherEvent>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// Discover only the immediate children of a project-relative directory.
/// Directory contents are loaded on demand by the explorer when the user
/// expands a folder. This keeps large repositories responsive and prevents a
/// deep directory from pushing later top-level siblings out of the view.
pub fn discover_directory(root: &Path, relative_directory: &Path) -> ProjectDocumentIndex {
    let mut entries = Vec::new();
    let directory = root.join(relative_directory);
    let ignores = ignore::IgnoreStack::for_dir(root, relative_directory);
    visit_children(root, &directory, &ignores, &mut entries);
    let mut index = ProjectDocumentIndex::default();
    index.replace(entries);
    index
}

/// Create a file or directory under a project-relative parent. Explorer
/// mutations always validate path components and never replace an existing
/// entry implicitly.
pub fn create_entry(
    root: &Path,
    relative_parent: &Path,
    name: &str,
    directory: bool,
) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::create_entry(root, relative_parent, name, directory);
    }
    let root = canonical_project_root(root)?;
    let parent = resolve_existing_directory(&root, relative_parent)?;
    let name = validate_entry_name(name)?;
    let destination = parent.join(name);
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    if directory {
        fs::create_dir(&destination).map_err(|error| format!("Create folder: {error}"))?;
    } else {
        fs::File::create(&destination).map_err(|error| format!("Create file: {error}"))?;
    }
    relative_to_root(&root, &destination)
}

/// Rename one project entry without allowing it to move through path syntax.
pub fn rename_entry(root: &Path, source: &Path, name: &str) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::rename_entry(root, source, name);
    }
    let destination = rename_entry_destination(root, source, name)?;
    let root = canonical_project_root(root)?;
    let source = resolve_existing_entry(&root, source)?;
    let absolute_destination = root.join(&destination);
    if absolute_destination == source {
        return Ok(destination);
    }
    fs::rename(&source, &absolute_destination).map_err(|error| format!("Rename entry: {error}"))?;
    Ok(destination)
}

/// Validate a proposed explorer rename and return its project-relative target
/// without mutating the filesystem. This lets language servers update imports
/// before the actual move occurs.
pub fn rename_entry_destination(root: &Path, source: &Path, name: &str) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::rename_entry_destination(root, source, name);
    }
    let root = canonical_project_root(root)?;
    let source = resolve_existing_entry(&root, source)?;
    let name = validate_entry_name(name)?;
    let parent = source
        .parent()
        .ok_or_else(|| "Project root cannot be renamed".to_string())?;
    let destination = parent.join(name);
    if destination != source && destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    relative_to_root(&root, &destination)
}

/// Update JavaScript/TypeScript module specifiers that resolve through a file
/// or folder being renamed. This is the guarded fallback for language servers
/// that advertise file operations but return no `willRenameFiles` edits (path
/// aliases are a common example). It never performs broad string replacement.

pub fn copy_entry(
    root: &Path,
    source: &Path,
    relative_destination_directory: &Path,
) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::copy_entry(root, source, relative_destination_directory);
    }
    let root = canonical_project_root(root)?;
    let source = resolve_existing_entry(&root, source)?;
    let destination_directory = resolve_existing_directory(&root, relative_destination_directory)?;
    if source.is_dir() && destination_directory.starts_with(&source) {
        return Err("A folder cannot be copied into itself".to_string());
    }
    let file_name = source
        .file_name()
        .ok_or_else(|| "Project root cannot be copied".to_string())?;
    let destination = unique_copy_destination(&destination_directory, file_name, source.is_file());
    copy_entry_recursive(&source, &destination)?;
    relative_to_root(&root, &destination)
}

/// Move a file or directory into a project-relative directory. If another entry
/// has the same name, the move is rejected rather than silently renaming or
/// overwriting the user's data.
pub fn move_entry(
    root: &Path,
    source: &Path,
    relative_destination_directory: &Path,
) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::move_entry(root, source, relative_destination_directory);
    }
    let root = canonical_project_root(root)?;
    let source = resolve_existing_entry(&root, source)?;
    let destination_directory = resolve_existing_directory(&root, relative_destination_directory)?;
    if source.is_dir() && destination_directory.starts_with(&source) {
        return Err("A folder cannot be moved into itself".to_string());
    }
    let file_name = source
        .file_name()
        .ok_or_else(|| "Project root cannot be moved".to_string())?;
    let destination = destination_directory.join(file_name);
    if destination == source {
        return relative_to_root(&root, &source);
    }
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    fs::rename(&source, &destination).map_err(|error| format!("Move entry: {error}"))?;
    relative_to_root(&root, &destination)
}

pub fn delete_entry(root: &Path, relative_path: &Path) -> Result<(), String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::delete_entry(root, relative_path);
    }
    let root = canonical_project_root(root)?;
    let path = resolve_existing_entry(&root, relative_path)?;
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| format!("Delete folder: {error}"))
    } else {
        fs::remove_file(path).map_err(|error| format!("Delete file: {error}"))
    }
}

pub fn absolute_entry_path(root: &Path, relative_path: &Path) -> Result<PathBuf, String> {
    if crate::remote_fs::is_remote(root) {
        return Ok(root.join(relative_path));
    }
    let root = canonical_project_root(root)?;
    resolve_existing_entry(&root, relative_path)
}

pub(crate) fn canonical_project_root(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize()
        .map_err(|error| format!("Project folder is unavailable: {error}"))
}

pub(crate) fn resolve_existing_entry(root: &Path, relative_path: &Path) -> Result<PathBuf, String> {
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Entry is outside the project".to_string());
    }
    let unresolved_candidate = root.join(relative_path);
    let unresolved_metadata = fs::symlink_metadata(&unresolved_candidate)
        .map_err(|error| format!("Read entry metadata: {error}"))?;
    if unresolved_metadata.file_type().is_symlink() {
        return Err("Symbolic links cannot be changed from the explorer yet".to_string());
    }
    let candidate = unresolved_candidate
        .canonicalize()
        .map_err(|error| format!("Entry is unavailable: {error}"))?;
    if !candidate.starts_with(root) || candidate == root {
        return Err("Entry is outside the project".to_string());
    }
    Ok(candidate)
}

pub(crate) fn resolve_existing_directory(
    root: &Path,
    relative_path: &Path,
) -> Result<PathBuf, String> {
    if relative_path.as_os_str().is_empty() {
        return Ok(root.to_path_buf());
    }
    let path = resolve_existing_entry(root, relative_path)?;
    if !path.is_dir() {
        return Err("Paste destination is not a folder".to_string());
    }
    Ok(path)
}

pub(crate) fn validate_entry_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    let mut components = Path::new(name).components();
    let valid = !name.is_empty()
        && !matches!(name, "." | "..")
        && matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !valid {
        return Err("Use a single file or folder name without path separators".to_string());
    }
    Ok(name)
}

pub(crate) fn relative_to_root(root: &Path, path: &Path) -> Result<PathBuf, String> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| "Entry is outside the project".to_string())
}

pub(crate) fn unique_copy_destination(
    directory: &Path,
    name: &std::ffi::OsStr,
    is_file: bool,
) -> PathBuf {
    let direct = directory.join(name);
    if !direct.exists() {
        return direct;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("item");
    let extension = is_file.then(|| path.extension()).flatten();
    for index in 1usize.. {
        let suffix = if index == 1 {
            " copy".to_string()
        } else {
            format!(" copy {index}")
        };
        let mut candidate_name = format!("{stem}{suffix}");
        if let Some(extension) = extension.and_then(|value| value.to_str()) {
            candidate_name.push('.');
            candidate_name.push_str(extension);
        }
        let candidate = directory.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("copy destination search is unbounded")
}

pub(crate) fn copy_entry_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(source).map_err(|error| format!("Read entry metadata: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("Folders containing symbolic links cannot be copied yet".to_string());
    }
    if source.is_file() {
        fs::copy(source, destination)
            .map(|_| ())
            .map_err(|error| format!("Copy file: {error}"))?;
        return Ok(());
    }
    fs::create_dir(destination).map_err(|error| format!("Create copied folder: {error}"))?;
    for entry in fs::read_dir(source).map_err(|error| format!("Read folder: {error}"))? {
        let entry = entry.map_err(|error| format!("Read folder entry: {error}"))?;
        let metadata = entry
            .file_type()
            .map_err(|error| format!("Read entry type: {error}"))?;
        if metadata.is_symlink() {
            return Err("Folders containing symbolic links cannot be copied yet".to_string());
        }
        copy_entry_recursive(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}
