//! Import-specifier rewriting on file renames.
//!
//! Moved verbatim; same behavior.

//! Bounded project-tree discovery for the future file surface.

use super::documents_entries::{canonical_project_root, relative_to_root, resolve_existing_entry};
use super::documents_io::save_text;
use super::project_documents::{IGNORED_DIRECTORIES, IGNORED_FILES};
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
pub fn update_imports_for_rename(
    root: &Path,
    source: &Path,
    destination: &Path,
    protected_paths: &HashSet<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let root = canonical_project_root(root)?;
    let source_absolute = resolve_existing_entry(&root, source)?;
    let source = relative_to_root(&root, &source_absolute)?;
    let mut files = Vec::new();
    collect_import_source_files(&root, &root, &mut files)?;
    let mut changed = Vec::new();
    for absolute_path in files {
        let relative_path = relative_to_root(&root, &absolute_path)?;
        let text = fs::read_to_string(&absolute_path)
            .map_err(|error| format!("Read import source {}: {error}", relative_path.display()))?;
        let updated = rewrite_module_specifiers(&root, &relative_path, &text, &source, destination);
        let Some(updated) = updated else {
            continue;
        };
        if protected_paths.contains(&relative_path) {
            return Err(format!(
                "Save {} before renaming so its imports can be updated safely.",
                relative_path.display()
            ));
        }
        save_text(&root, &relative_path, &updated, None)?;
        changed.push(relative_path);
    }
    Ok(changed)
}

pub(crate) fn collect_import_source_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| format!("Read project folder: {error}"))? {
        let entry = entry.map_err(|error| format!("Read project entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Read project entry type: {error}"))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| IGNORED_DIRECTORIES.contains(&name) || name.starts_with('.'))
            {
                continue;
            }
            collect_import_source_files(root, &path, files)?;
            continue;
        }
        if files.len() >= 25_000 {
            return Err("Project is too large for the local import-update fallback".to_string());
        }
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
                )
            });
        if supported
            && path
                .metadata()
                .is_ok_and(|metadata| metadata.len() <= 2 * 1024 * 1024)
            && path.starts_with(root)
        {
            files.push(path);
        }
    }
    Ok(())
}

pub(crate) fn rewrite_module_specifiers(
    root: &Path,
    importer: &Path,
    text: &str,
    source: &Path,
    destination: &Path,
) -> Option<String> {
    let mut replacements = Vec::new();
    for range in module_specifier_ranges(text) {
        let specifier = &text[range.clone()];
        let replacement = if specifier.starts_with('.') {
            rewrite_relative_specifier(root, importer, specifier, source, destination)
        } else {
            rewrite_alias_specifier(root, specifier, source, destination)
        };
        if let Some(replacement) = replacement.filter(|value| value != specifier) {
            replacements.push((range, replacement));
        }
    }
    if replacements.is_empty() {
        return None;
    }
    let mut updated = text.to_string();
    for (range, replacement) in replacements.into_iter().rev() {
        updated.replace_range(range, &replacement);
    }
    Some(updated)
}

pub(crate) fn module_specifier_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if bytes[index] == b'`' {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else if bytes[index] == b'`' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if !matches!(bytes[index], b'\'' | b'"') {
            index += 1;
            continue;
        }
        let quote = bytes[index];
        let start = index + 1;
        index = start;
        let mut escaped = false;
        while index < bytes.len() && bytes[index] != quote {
            if bytes[index] == b'\\' {
                escaped = true;
                index = (index + 2).min(bytes.len());
            } else if bytes[index] == b'\n' || bytes[index] == b'\r' {
                break;
            } else {
                index += 1;
            }
        }
        if index >= bytes.len() || bytes[index] != quote {
            continue;
        }
        if !escaped && is_module_specifier_prefix(&text[..start - 1]) {
            ranges.push(start..index);
        }
        index += 1;
    }
    ranges
}

pub(crate) fn is_module_specifier_prefix(prefix: &str) -> bool {
    let prefix = prefix.trim_end();
    if prefix.ends_with("from")
        && prefix[..prefix.len() - "from".len()]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_character(character))
    {
        return true;
    }
    if prefix.ends_with("import")
        && prefix[..prefix.len() - "import".len()]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_character(character))
    {
        return true;
    }
    let Some(before_parenthesis) = prefix.strip_suffix('(').map(str::trim_end) else {
        return false;
    };
    ["import", "require"].iter().any(|keyword| {
        before_parenthesis.ends_with(keyword)
            && before_parenthesis[..before_parenthesis.len() - keyword.len()]
                .chars()
                .next_back()
                .is_none_or(|character| !is_identifier_character(character))
    })
}

pub(crate) fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '$')
}

pub(crate) fn rewrite_alias_specifier(
    root: &Path,
    specifier: &str,
    source: &Path,
    destination: &Path,
) -> Option<String> {
    let parts = specifier.split('/').collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let tail = &parts[1..];
    let source_parts = module_path_parts(source);
    let destination_parts = module_path_parts(destination);
    let maximum = tail.len().min(source_parts.len());
    for count in (1..=maximum).rev() {
        let source_suffix = &source_parts[source_parts.len() - count..];
        if source_suffix
            .iter()
            .map(String::as_str)
            .ne(tail[..count].iter().copied())
        {
            continue;
        }
        let candidate = source.join(tail[count..].iter().copied().collect::<PathBuf>());
        if !module_target_exists(root, &candidate) {
            continue;
        }
        let destination_suffix = &destination_parts[destination_parts.len() - count..];
        let mut updated = vec![parts[0].to_string()];
        updated.extend(destination_suffix.iter().cloned());
        updated.extend(tail[count..].iter().map(|part| (*part).to_string()));
        return Some(updated.join("/"));
    }
    None
}

pub(crate) fn rewrite_relative_specifier(
    root: &Path,
    importer: &Path,
    specifier: &str,
    source: &Path,
    destination: &Path,
) -> Option<String> {
    let importer_parent = importer.parent().unwrap_or(Path::new(""));
    let target = normalize_module_path(importer_parent, specifier)?;
    if !module_target_exists(root, &target) {
        return None;
    }
    let source_key = module_path_without_extension(source);
    let destination_key = module_path_without_extension(destination);
    let suffix = target.strip_prefix(&source_key).ok()?;
    let new_target = destination_key.join(suffix);
    Some(relative_module_specifier(importer_parent, &new_target))
}

pub(crate) fn module_path_parts(path: &Path) -> Vec<String> {
    module_path_without_extension(path)
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
        .collect()
}

pub(crate) fn module_path_without_extension(path: &Path) -> PathBuf {
    let mut path = path.to_path_buf();
    if path.extension().is_some() {
        path.set_extension("");
    }
    path
}

pub(crate) fn normalize_module_path(base: &Path, specifier: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop().then_some(())?;
            }
            part => path.push(part),
        }
    }
    Some(path)
}

pub(crate) fn module_target_exists(root: &Path, relative: &Path) -> bool {
    const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];
    let direct = root.join(relative);
    if direct.is_file() || direct.is_dir() {
        return true;
    }
    if EXTENSIONS
        .iter()
        .any(|extension| direct.with_extension(extension).is_file())
    {
        return true;
    }
    EXTENSIONS
        .iter()
        .any(|extension| direct.join("index").with_extension(extension).is_file())
}

pub(crate) fn relative_module_specifier(from: &Path, to: &Path) -> String {
    let from_parts = from.components().collect::<Vec<_>>();
    let to_parts = to.components().collect::<Vec<_>>();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_string(); from_parts.len().saturating_sub(common)];
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    let result = parts.join("/");
    if result.starts_with('.') {
        result
    } else {
        format!("./{result}")
    }
}
