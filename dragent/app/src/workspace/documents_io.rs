//! Read-only previews, markdown links, saving, and checkpoints.
//!
//! Moved verbatim; same behavior.

//! Bounded project-tree discovery for the future file surface.

use super::documents_ignore::ignore;
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
pub struct ReadOnlyDocument {
    pub relative_path: std::path::PathBuf,
    pub text: String,
    /// Local visual assets are opened read-only by the editor surface instead
    /// of being decoded into the text buffer.
    pub visual_path: Option<PathBuf>,
    pub error: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFingerprint {
    pub content_hash: u64,
    pub byte_size: u64,
    pub modified_unix_ms: Option<i64>,
}

impl FileFingerprint {
    /// Content-only fingerprint (remote files, where mtimes come from
    /// another clock).
    pub(crate) fn from_content(bytes: &[u8], modified_unix_ms: Option<i64>) -> Self {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self {
            content_hash: hash,
            byte_size: bytes.len() as u64,
            modified_unix_ms,
        }
    }

    fn from_bytes(metadata: &fs::Metadata, bytes: &[u8]) -> Self {
        // Stable FNV-1a avoids randomized process hash seeds and lets recovery
        // snapshots remain comparable across application versions/restarts.
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self {
            content_hash: hash,
            byte_size: metadata.len(),
            modified_unix_ms: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64),
        }
    }
}

pub fn read_preview(root: &Path, relative_path: &Path) -> ReadOnlyDocument {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::read_preview(root, relative_path);
    }
    let failure = |message: String| ReadOnlyDocument {
        relative_path: relative_path.to_path_buf(),
        text: String::new(),
        visual_path: None,
        error: Some(message),
        fingerprint: None,
    };
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        log::warn!(
            "[editor-open] rejected unsafe path root={} relative={}",
            root.display(),
            relative_path.display()
        );
        return failure("File is outside the project".to_string());
    }
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            log::warn!(
                "[editor-open] root unavailable root={} relative={} error={}",
                root.display(),
                relative_path.display(),
                error
            );
            return failure("Project folder is unavailable".to_string());
        }
    };
    let candidate = root.join(relative_path);
    let candidate = match candidate.canonicalize() {
        Ok(candidate) => candidate,
        Err(error) => {
            log::warn!(
                "[editor-open] file unavailable root={} relative={} candidate={} error={}",
                root.display(),
                relative_path.display(),
                candidate.display(),
                error
            );
            return failure(
                "File is unavailable. Refresh the file tree and try again.".to_string(),
            );
        }
    };
    if !candidate.starts_with(&root) || !candidate.is_file() {
        log::warn!(
            "[editor-open] resolved outside root root={} relative={} candidate={}",
            root.display(),
            relative_path.display(),
            candidate.display()
        );
        return failure("File is outside the project".to_string());
    }
    let mut file = match fs::File::open(&candidate) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "[editor-open] open failed candidate={} error={}",
                candidate.display(),
                error
            );
            return failure("File could not be opened".to_string());
        }
    };
    if is_visual_asset(&candidate) {
        return ReadOnlyDocument {
            relative_path: relative_path.to_path_buf(),
            text: String::new(),
            visual_path: Some(candidate),
            error: None,
            fingerprint: None,
        };
    }
    let metadata = file.metadata().ok();
    // Do not impose a preview-sized editor limit here. The old 2 MiB read cap
    // made an otherwise ordinary local file impossible to edit. Reading is
    // already performed on the workspace background task; feature reductions
    // for large documents are decided after the text is loaded by the buffer
    // capability policy.
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return failure("File could not be read".to_string());
    }
    if bytes.contains(&0) {
        return failure("Binary files cannot be previewed".to_string());
    }
    let fingerprint = metadata
        .as_ref()
        .map(|metadata| FileFingerprint::from_bytes(metadata, &bytes));
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

/// Resolve a Markdown destination to an existing file inside `project_root`.
///
/// Normal Markdown paths are tried relative to the source document first and
/// then relative to the project root. Absolute project paths and home paths
/// are accepted only when their canonical destination remains in the project.
/// Some generated Markdown incorrectly prefixes local files with `http://`;
/// those targets are treated as local only when a matching project file
/// exists, so genuine web links still fall through to the browser.
pub fn resolve_markdown_project_link(
    project_root: &Path,
    source: &Path,
    target: &str,
) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with('#')
        || target.starts_with("//")
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
    {
        return None;
    }

    let target = target
        .strip_prefix("https://")
        .or_else(|| target.strip_prefix("http://"))
        .unwrap_or(target);
    if target.contains("://") {
        return None;
    }
    let path = target.split(['#', '?']).next().unwrap_or_default().trim();
    if path.is_empty() {
        return None;
    }

    let root = project_root.canonicalize().ok()?;
    let raw = Path::new(path);
    let mut candidates = Vec::new();
    if let Some(home_relative) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(home_relative));
        }
    } else if raw.is_absolute() {
        // `/docs/file.md` is the common project-root Markdown form. Also try
        // the path as a real filesystem path for links copied from Finder or
        // a terminal, while the canonical containment check below keeps it
        // constrained to this project.
        candidates.push(raw.to_path_buf());
        candidates.push(root.join(path.trim_start_matches('/')));
    } else {
        let source_relative = source.strip_prefix(&root).unwrap_or(source);
        candidates.push(
            root.join(source_relative.parent().unwrap_or_else(|| Path::new("")))
                .join(raw),
        );
        candidates.push(root.join(raw));
    }

    for candidate in candidates {
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if !candidate.is_file() {
            continue;
        }
        if let Ok(relative) = candidate.strip_prefix(&root) {
            if !relative.as_os_str().is_empty() {
                return Some(relative.to_path_buf());
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownLinkTarget {
    ProjectFile(PathBuf),
    ExternalUrl(String),
}

/// Classify a rendered Markdown link using the same URL-or-file ordering used
/// by desktop editors: safely resolvable workspace files open in the editor,
/// while URI schemes are delegated to the operating system.
pub fn classify_markdown_link(
    project_root: &Path,
    source: &Path,
    target: &str,
) -> Option<MarkdownLinkTarget> {
    let target = target.trim();
    if target.is_empty() || target.starts_with('#') {
        return None;
    }

    // Preserve compatibility with generated links such as
    // `http://demo-script.md`: if that spelling maps to a real file beside the
    // source document, local navigation wins. Otherwise it is a normal URL.
    if let Some(path) = resolve_markdown_project_link(project_root, source, target) {
        return Some(MarkdownLinkTarget::ProjectFile(path));
    }

    if let Ok(url) = url::Url::parse(target) {
        if url.scheme() == "file" {
            if let Ok(path) = url.to_file_path() {
                if let Some(path) =
                    resolve_markdown_project_link(project_root, source, &path.to_string_lossy())
                {
                    return Some(MarkdownLinkTarget::ProjectFile(path));
                }
                return None;
            }
        }
        return Some(MarkdownLinkTarget::ExternalUrl(target.to_string()));
    }

    // Protocol-relative browser links are valid in Markdown even though they
    // cannot be parsed without a base URL.
    if target.starts_with("//") {
        return Some(MarkdownLinkTarget::ExternalUrl(format!("https:{target}")));
    }

    None
}

#[cfg(test)]

pub(crate) mod markdown_link_tests {
    use super::{classify_markdown_link, resolve_markdown_project_link, MarkdownLinkTarget};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn markdown_link_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dragent-markdown-links-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("docs/start")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("docs/api.md"), "api").unwrap();
        std::fs::write(root.join("docs/start/demo-script.md"), "demo").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        root
    }

    #[test]
    fn resolves_relative_and_project_root_markdown_links() {
        let root = markdown_link_root();
        assert_eq!(
            resolve_markdown_project_link(
                &root,
                Path::new("docs/start/readme.md"),
                "../api.md#usage"
            ),
            Some(PathBuf::from("docs/api.md"))
        );
        assert_eq!(
            resolve_markdown_project_link(
                &root,
                Path::new("docs/readme.md"),
                "/src/main.rs?plain=1"
            ),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            resolve_markdown_project_link(
                &root,
                Path::new("docs/start/readme.md"),
                "http://demo-script.md"
            ),
            Some(PathBuf::from("docs/start/demo-script.md"))
        );
        let absolute = root
            .join("docs/api.md")
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        assert_eq!(
            resolve_markdown_project_link(&root, Path::new("README.md"), &absolute),
            Some(PathBuf::from("docs/api.md"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_external_and_escaping_markdown_links() {
        let root = markdown_link_root();
        assert_eq!(
            resolve_markdown_project_link(&root, Path::new("readme.md"), "https://example.com"),
            None
        );
        assert_eq!(
            resolve_markdown_project_link(&root, Path::new("readme.md"), "../secret.txt"),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_browser_and_project_links_through_one_route() {
        let root = markdown_link_root();
        let source = Path::new("docs/start/readme.md");
        assert_eq!(
            classify_markdown_link(&root, source, "https://example.com/docs?q=1#intro"),
            Some(MarkdownLinkTarget::ExternalUrl(
                "https://example.com/docs?q=1#intro".into()
            ))
        );
        assert_eq!(
            classify_markdown_link(&root, source, "//example.com/docs"),
            Some(MarkdownLinkTarget::ExternalUrl(
                "https://example.com/docs".into()
            ))
        );
        assert_eq!(
            classify_markdown_link(&root, source, "demo-script.md"),
            Some(MarkdownLinkTarget::ProjectFile(PathBuf::from(
                "docs/start/demo-script.md"
            )))
        );
        assert_eq!(
            classify_markdown_link(&root, source, "http://demo-script.md"),
            Some(MarkdownLinkTarget::ProjectFile(PathBuf::from(
                "docs/start/demo-script.md"
            )))
        );
        let file_url = url::Url::from_file_path(root.join("docs/api.md"))
            .unwrap()
            .to_string();
        assert_eq!(
            classify_markdown_link(&root, source, &file_url),
            Some(MarkdownLinkTarget::ProjectFile(PathBuf::from(
                "docs/api.md"
            )))
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Formats that modern editors present as a visual preview. Keep
/// this list explicit so arbitrary binary files are never handed to the image
/// decoder or loaded into memory accidentally.

pub fn is_visual_asset(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" | "tif" | "tiff"
    )
}

pub fn save_text(
    root: &Path,
    relative_path: &Path,
    text: &str,
    expected: Option<FileFingerprint>,
) -> Result<FileFingerprint, String> {
    if crate::remote_fs::is_remote(root) {
        return super::documents_remote::save_text(root, relative_path, text, expected);
    }
    let (candidate, permissions) = resolve_writable_file(root, relative_path)?;
    if let Some(expected) = expected {
        let bytes =
            fs::read(&candidate).map_err(|error| format!("Read file before save: {error}"))?;
        let metadata = fs::metadata(&candidate)
            .map_err(|error| format!("Read file metadata before save: {error}"))?;
        if FileFingerprint::from_bytes(&metadata, &bytes) != expected {
            return Err(
                "File changed on disk. Compare, overwrite, or revert before saving.".into(),
            );
        }
    }
    let parent = candidate.parent().ok_or("File has no parent folder")?;
    let file_name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("File name is invalid")?;
    let temporary = parent.join(format!(".{file_name}.dragent-{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Create temporary file: {error}"))?;
        file.set_permissions(permissions)
            .map_err(|error| format!("Preserve file permissions: {error}"))?;
        file.write_all(text.as_bytes())
            .map_err(|error| format!("Write file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Flush file: {error}"))?;
        replace_file(&temporary, &candidate)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    let metadata = fs::metadata(&candidate).map_err(|error| format!("Read saved file: {error}"))?;
    Ok(FileFingerprint::from_bytes(&metadata, text.as_bytes()))
}

pub fn write_checkpoint(root: &Path, relative_path: &Path, diff: &str) -> Result<PathBuf, String> {
    if diff.len() > 2 * 1024 * 1024 {
        return Err("Checkpoint is too large".to_string());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Project folder is unavailable: {error}"))?;
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("File is outside the project".to_string());
    }
    let directory = dragent_paths::workspace(&root).join("checkpoints");
    dragent_paths::with_migration_lock(|| {
        let old = root.join(".dragent/checkpoints");
        dragent_paths::move_preserving_existing(&old, &directory)?;
        dragent_paths::preserve_conflict(
            &old,
            &dragent_paths::workspace(&root).join("legacy-conflicts/checkpoints"),
        )
    })
    .map_err(|error| format!("Migrate checkpoints: {error}"))?;
    let _ = fs::remove_dir(root.join(".dragent"));
    fs::create_dir_all(&directory).map_err(|error| format!("Create checkpoint folder: {error}"))?;
    let safe_name = relative_path
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    let path = directory.join(format!(
        "{}-{}.diff",
        chrono::Utc::now().timestamp_millis(),
        safe_name
    ));
    fs::write(&path, diff).map_err(|error| format!("Write checkpoint: {error}"))?;
    trim_checkpoints(&directory, 20);
    Ok(path)
}

pub(crate) fn resolve_writable_file(
    root: &Path,
    relative_path: &Path,
) -> Result<(PathBuf, fs::Permissions), String> {
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("File is outside the project".to_string());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Project folder is unavailable: {error}"))?;
    let candidate = root
        .join(relative_path)
        .canonicalize()
        .map_err(|error| format!("File is unavailable: {error}"))?;
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return Err("File is outside the project".to_string());
    }
    let permissions = fs::metadata(&candidate)
        .map_err(|error| format!("Read file permissions: {error}"))?
        .permissions();
    Ok((candidate, permissions))
}

#[cfg(not(windows))]
pub(crate) fn replace_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(temporary, destination).map_err(|error| format!("Replace file: {error}"))
}

#[cfg(windows)]
pub(crate) fn replace_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    // Windows does not allow `rename` over an existing file. Keep the write
    // bounded and flushed, then replace it through the standard filesystem API.
    fs::remove_file(destination).map_err(|error| format!("Replace file: {error}"))?;
    fs::rename(temporary, destination).map_err(|error| format!("Replace file: {error}"))
}

pub(crate) fn trim_checkpoints(directory: &Path, keep: usize) {
    let Ok(read_dir) = fs::read_dir(directory) else {
        return;
    };
    let mut files: Vec<_> = read_dir
        .flatten()
        .filter(|entry| entry.path().is_file())
        .collect();
    files.sort_by_key(|entry| entry.file_name());
    let remove_count = files.len().saturating_sub(keep);
    for entry in files.into_iter().take(remove_count) {
        let _ = fs::remove_file(entry.path());
    }
}

pub(crate) fn visit_children(
    root: &Path,
    current: &Path,
    ignores: &ignore::IgnoreStack,
    entries: &mut Vec<DocumentEntry>,
) {
    if crate::remote_fs::is_remote(current) {
        return super::documents_remote::visit_children(root, current, entries);
    }
    let Ok(read_dir) = fs::read_dir(current) else {
        return;
    };
    let mut children: Vec<_> = read_dir.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        let Ok(file_type) = child.file_type() else {
            continue;
        };
        // Resolve symlinks to their target type instead of dropping them.
        // Broken links show as files so they are visible rather than
        // silently vanishing.
        let (is_dir, is_file) = if file_type.is_symlink() {
            match fs::metadata(&path) {
                Ok(meta) => (meta.is_dir(), meta.is_file()),
                Err(_) => (false, true),
            }
        } else {
            (file_type.is_dir(), file_type.is_file())
        };
        if !is_dir && !is_file {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if is_dir {
            if child
                .file_name()
                .to_str()
                .is_some_and(|name| IGNORED_DIRECTORIES.contains(&name))
            {
                continue;
            }
            if let Some(document) = DocumentRef::from_relative_path(relative.clone()) {
                let is_ignored = ignores.is_ignored(&relative, true);
                entries.push(DocumentEntry {
                    document,
                    is_directory: true,
                    byte_size: None,
                    is_ignored,
                });
            }
        } else if is_file {
            if child
                .file_name()
                .to_str()
                .is_some_and(|name| IGNORED_FILES.contains(&name))
            {
                continue;
            }
            if let Some(document) = DocumentRef::from_relative_path(relative.clone()) {
                let byte_size = fs::metadata(&path).ok().map(|metadata| metadata.len());
                let is_ignored = ignores.is_ignored(&relative, false);
                entries.push(DocumentEntry {
                    document,
                    is_directory: false,
                    byte_size,
                    is_ignored,
                });
            }
        }
    }
}

/// Recursive companion to `visit_children` for the initial snapshot.
/// Single-level `discover_directory` stays on-demand for expansion; this
/// fills deep paths upfront (cycle-safe via canonical dirs). `.gitignore`
/// files are loaded as each directory is entered so deeper rules apply to
/// their own subtrees.
pub(crate) fn visit_recursive(
    root: &Path,
    current: &Path,
    current_rel: &Path,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<PathBuf>,
    ignores: &mut ignore::IgnoreStack,
    entries: &mut Vec<DocumentEntry>,
) {
    if depth > max_depth {
        return;
    }
    // Load this directory's own `.gitignore` (root's was loaded by
    // `for_root`; deeper ones here). Popped after the subtree.
    let pushed = if depth == 0 {
        false
    } else {
        ignores.descend(root, current_rel)
    };
    let before = entries.len();
    visit_children(root, current, ignores, entries);
    // Recurse only into directories discovered at this level — skipping
    // heavy dependency/build outputs (still listed above, expandable on
    // demand) and stopping past the entry budget so refreshes stay fast.
    let dirs: Vec<PathBuf> = entries[before..]
        .iter()
        .filter(|entry| {
            entry.is_directory
                && entries.len() < MAX_AUTO_WALK_ENTRIES
                && !entry
                    .document
                    .relative_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| LAZY_DIRECTORIES.contains(&name))
        })
        .map(|entry| root.join(&entry.document.relative_path))
        .collect();
    let rels: Vec<PathBuf> = entries[before..]
        .iter()
        .filter(|entry| entry.is_directory)
        .map(|entry| entry.document.relative_path.clone())
        .collect();
    for (dir, rel) in dirs.into_iter().zip(rels) {
        // Cycle guard: canonicalize (follows symlinked dirs) and skip
        // already-visited inodes-paths.
        if let Ok(canonical) = dir.canonicalize() {
            if !visited.insert(canonical) {
                continue;
            }
        }
        visit_recursive(
            root,
            &dir,
            &rel,
            depth + 1,
            max_depth,
            visited,
            ignores,
            entries,
        );
    }
    if pushed {
        ignores.ascend();
    }
}
