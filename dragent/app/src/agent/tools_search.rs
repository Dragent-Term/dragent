//! Search tools: files walk, grep, codebase search.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_background::*;
use super::tools_command::*;
use super::tools_files::*;
use super::tools_mcp_auth::*;
use super::tools_shell::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;
pub(crate) fn exec_search_files(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let pattern = match str_arg(args, "pattern") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let excludes: Vec<String> = args["exclude"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let root = resolve_path(raw_path);
    let needle = pattern.to_lowercase();

    let mut matches = Vec::new();
    let mut budget = 500usize; // cap results to keep output bounded
    search_files_walk(
        &root,
        &root,
        &needle,
        &excludes,
        &mut matches,
        &mut budget,
        0,
    );

    if matches.is_empty() {
        return (
            format!(
                "No files matching {pattern:?} under {}",
                root.to_string_lossy()
            ),
            false,
        );
    }
    (matches.join("\n"), false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn search_files_walk(
    root: &std::path::Path,
    dir: &std::path::Path,
    needle: &str,
    excludes: &[String],
    out: &mut Vec<String>,
    budget: &mut usize,
    depth: usize,
) {
    if depth > 8 || *budget == 0 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if *budget == 0 {
            return;
        }
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if excludes.iter().any(|ex| name.contains(ex.as_str())) {
            continue;
        }
        if name.contains(needle) {
            out.push(
                p.strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned(),
            );
            *budget -= 1;
        }
        if p.is_dir() && !name.starts_with('.') && name != "node_modules" && name != "target" {
            search_files_walk(root, &p, needle, excludes, out, budget, depth + 1);
        }
    }
}

// ── get_file_info ──────────────────────────────────────────────────────────────
// Metadata for a single path: type, size, timestamps, and permissions.

pub(crate) fn exec_get_file_info(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let resolved = resolve_path(raw_path);
    let path = resolved.to_string_lossy().into_owned();

    let meta = match std::fs::symlink_metadata(&resolved) {
        Ok(m) => m,
        Err(e) => return (format!("Cannot stat {path}: {e}"), true),
    };

    let kind = if meta.is_dir() {
        "directory"
    } else if meta.file_type().is_symlink() {
        "symlink"
    } else {
        "file"
    };

    let mut out = String::new();
    out.push_str(&format!("path:      {path}\n"));
    out.push_str(&format!("type:      {kind}\n"));
    out.push_str(&format!("size:      {} bytes\n", meta.len()));
    out.push_str(&format!("modified:  {}\n", fmt_sys_time(meta.modified())));
    out.push_str(&format!("created:   {}\n", fmt_sys_time(meta.created())));
    out.push_str(&format!("accessed:  {}\n", fmt_sys_time(meta.accessed())));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        out.push_str(&format!(
            "mode:      {:o}\n",
            meta.permissions().mode() & 0o777
        ));
    }
    out.push_str(&format!("readonly:  {}\n", meta.permissions().readonly()));
    (out, false)
}

// ── list_allowed_directories ───────────────────────────────────────────────────
// The workspace root(s) these filesystem tools are scoped to.

pub(crate) fn exec_list_allowed_directories(_args: &serde_json::Value) -> (String, bool) {
    let mut roots = vec![workspace_root()];
    roots.extend(crate::agent::permission_store::PermissionStore::load().always_allow_read_roots);
    roots.sort();
    roots.dedup();
    let rendered = roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    (format!("Allowed read directories:\n{rendered}"), false)
}

// ── grep ──────────────────────────────────────────────────────────────────────
// mode: "files_with_matches" (default), "content", "count"
// context_lines: N lines before and after each match (like grep -C N)

pub(crate) fn exec_grep(args: &serde_json::Value) -> (String, bool) {
    let pattern = match str_arg(args, "pattern") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let raw_path = args["path"].as_str().unwrap_or(".");
    let resolved_search = resolve_path(raw_path);
    let search_path = resolved_search.to_string_lossy().into_owned();
    let search_path = search_path.as_str();
    let mode = args["mode"].as_str().unwrap_or("content");
    let ctx_lines = args["context_lines"].as_u64().unwrap_or(0) as usize;

    let re = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return (format!("Invalid regex pattern: {e}"), true),
    };

    match mode {
        "files_with_matches" => {
            let mut files: Vec<String> = Vec::new();
            grep_files(&re, Path::new(search_path), &mut files, 0);
            if files.is_empty() {
                (
                    format!("No files matching '{pattern}' in {search_path}"),
                    false,
                )
            } else {
                (files.join("\n"), false)
            }
        }
        "count" => {
            let mut results: Vec<String> = Vec::new();
            grep_count(&re, Path::new(search_path), &mut results, 0);
            if results.is_empty() {
                (
                    format!("No matches for '{pattern}' in {search_path}"),
                    false,
                )
            } else {
                (results.join("\n"), false)
            }
        }
        _ => {
            // "content" — show matching lines with optional context
            let mut matches: Vec<String> = Vec::new();
            grep_content(&re, Path::new(search_path), ctx_lines, &mut matches, 0);
            if matches.is_empty() {
                (
                    format!("No matches for '{pattern}' in {search_path}"),
                    false,
                )
            } else {
                (matches.join("\n"), false)
            }
        }
    }
}

pub(crate) fn grep_files(re: &regex::Regex, path: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 8 || out.len() >= 250 {
        return;
    }
    if path.is_file() {
        if is_binary_extension(path) {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            if re.is_match(&content) {
                out.push(path.display().to_string());
            }
        }
    } else if path.is_dir() {
        if should_skip_dir(path) {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                grep_files(re, &entry.path(), out, depth + 1);
            }
        }
    }
}

pub(crate) fn grep_count(re: &regex::Regex, path: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 8 || out.len() >= 250 {
        return;
    }
    if path.is_file() {
        if is_binary_extension(path) {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            let count = content.lines().filter(|l| re.is_match(l)).count();
            if count > 0 {
                out.push(format!("{}:{}", path.display(), count));
            }
        }
    } else if path.is_dir() {
        if should_skip_dir(path) {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                grep_count(re, &entry.path(), out, depth + 1);
            }
        }
    }
}

pub(crate) fn grep_content(
    re: &regex::Regex,
    path: &Path,
    ctx_lines: usize,
    out: &mut Vec<String>,
    depth: usize,
) {
    if depth > 8 || out.len() >= 500 {
        return;
    }
    if path.is_file() {
        if is_binary_extension(path) {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            // Find all matching line indices
            let mut match_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| re.is_match(l))
                .map(|(i, _)| i)
                .collect();

            if match_indices.is_empty() {
                return;
            }

            // Expand each match to include context, merge overlapping ranges
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for &idx in &match_indices {
                let start = idx.saturating_sub(ctx_lines);
                let end = (idx + ctx_lines + 1).min(total);
                if let Some(last) = ranges.last_mut() {
                    if start <= last.1 {
                        last.1 = end.max(last.1);
                        continue;
                    }
                }
                ranges.push((start, end));
            }

            let path_str = path.display().to_string();
            let width = total.to_string().len();
            let mut first_block = true;

            for (start, end) in ranges {
                if !first_block {
                    out.push("--".to_string());
                }
                first_block = false;
                for i in start..end {
                    let marker = if re.is_match(lines[i]) { ":" } else { "-" };
                    out.push(format!("{path_str}:{}{marker}{}", i + 1, lines[i]));
                    if out.len() >= 500 {
                        return;
                    }
                }
            }
        }
    } else if path.is_dir() {
        if should_skip_dir(path) {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            let mut sorted: Vec<_> = entries.flatten().collect();
            sorted.sort_by_key(|e| e.file_name());
            for entry in sorted {
                grep_content(re, &entry.path(), ctx_lines, out, depth + 1);
                if out.len() >= 500 {
                    return;
                }
            }
        }
    }
}

pub(crate) fn should_skip_dir(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.starts_with('.')
        || matches!(
            name.as_ref(),
            "node_modules" | "target" | "vendor" | "__pycache__"
        )
}

pub(crate) fn is_binary_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "svg"
            | "pdf"
            | "zip"
            | "tar"
            | "gz"
            | "wasm"
            | "exe"
            | "bin"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "mp4"
            | "mp3"
            | "lock"
    )
}

// ── search_codebase ───────────────────────────────────────────────────────────

pub(crate) fn exec_search_codebase(args: &serde_json::Value) -> (String, bool) {
    let query = match str_arg(args, "query") {
        Ok(q) => q,
        Err(e) => return (e, true),
    };
    let raw_root = args["repo_root"].as_str().unwrap_or(".");
    let root_path = resolve_path(raw_root);

    use crate::agent::permissions::AllowedRoots;
    use crate::agent::tools::SearchCodebaseExecutor;

    let allowed = AllowedRoots::new([root_path.clone()]);
    let executor = SearchCodebaseExecutor::new(allowed);
    let result = executor.execute(&query, &root_path);

    if result.hits.is_empty() {
        return ("No results found.".to_string(), false);
    }

    let output = result
        .hits
        .iter()
        .map(|h| {
            let label = if h.score.is_some() {
                "[semantic]"
            } else {
                "[grep]"
            };
            format!(
                "{label} {}:{}-{}\n{}",
                h.path.display(),
                h.byte_start,
                h.byte_end,
                h.preview
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (output, false)
}

// ── Background task completion notifications ──────────────────────────────────
