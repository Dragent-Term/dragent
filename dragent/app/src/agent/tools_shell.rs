//! Shell dispatch: cwd scopes, permissions, execute entry points.
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
use super::tools_search::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;

// ── Per-pane identity ────────────────────────────────────────────────────────
// All globals below are keyed by pane_uuid so each terminal tab has fully
// isolated state. The thread-local is set at the top of execute() so every
// helper function downstream picks up the right pane without needing a
// parameter threaded through every call.
thread_local! {
    static PANE_ID: std::cell::Cell<Uuid> = const { std::cell::Cell::new(Uuid::nil()) };
}
pub(crate) fn current_pane() -> Uuid {
    PANE_ID.with(|c| c.get())
}

// ── Shell state ───────────────────────────────────────────────────────────────
// Persists cwd and environment variables across run_command calls within the
// same agent session. Without this, `cd /project` in one call has no effect
// on the next call because each invocation spawns a fresh shell.

#[derive(Default, Clone)]
pub(crate) struct ShellState {
    pub(crate) cwd: Option<String>,
    pub(crate) env: HashMap<String, String>,
}

pub(crate) static SHELL_STATE: Lazy<std::sync::Mutex<HashMap<Uuid, ShellState>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

// ── Project CWD ───────────────────────────────────────────────────────────────
// The absolute path of the project root / working directory for this session.
// Used to resolve relative file paths provided by the LLM so tools never need
// the LLM to construct absolute paths (which can break with spaces in paths).

pub(crate) static PROJECT_CWD: Lazy<std::sync::Mutex<HashMap<Uuid, std::path::PathBuf>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Per-call external read grants issued by the approval UI. They are keyed by
/// pipeline call id and removed immediately after that call finishes.
pub(crate) static ONE_SHOT_READ_ROOTS: Lazy<
    std::sync::Mutex<HashMap<String, Vec<std::path::PathBuf>>>,
> = Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Set the project working directory for this pane.
/// Called before the first tool call so all relative paths resolve correctly.
pub fn set_project_cwd(cwd: &std::path::Path, pane_id: Uuid) {
    if let Ok(mut map) = PROJECT_CWD.lock() {
        map.insert(pane_id, cwd.to_path_buf());
    }
}

/// Returns true if `path` resolves to a location outside the session's project
/// root. Used to gate `read_file` calls that reach beyond the current workspace.
/// Called from the controller's model thread (not a tool thread), so takes
/// pane_id explicitly rather than relying on the thread-local.
pub fn is_outside_project_cwd(path: &str, pane_id: Uuid) -> bool {
    let resolved = permission_path(resolve_path_for_pane(path, pane_id));
    project_cwd_for_pane(pane_id)
        .map(|cwd| !resolved.starts_with(permission_path(cwd)))
        .unwrap_or(false)
}

pub(crate) fn project_cwd_for_pane(pane_id: Uuid) -> Option<std::path::PathBuf> {
    PROJECT_CWD.lock().ok()?.get(&pane_id).cloned()
}

/// Whether an approved command targets the directory currently displayed by
/// the user's terminal. Only such commands may borrow the visible PTY. A tool
/// request with another cwd must use the isolated executor so it cannot run in
/// the wrong project or silently move the user's shell.
pub fn run_command_targets_terminal_cwd(
    args_json: &str,
    terminal_cwd: &Path,
    pane_id: Uuid,
) -> bool {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return false;
    };
    let Some(requested) = args.get("cwd").and_then(|value| value.as_str()) else {
        return true;
    };
    if requested.trim().is_empty() {
        return true;
    }

    let requested = Path::new(requested);
    let requested = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        project_cwd_for_pane(pane_id)
            .unwrap_or_else(|| terminal_cwd.to_path_buf())
            .join(requested)
    };
    permission_path(requested) == permission_path(terminal_cwd.to_path_buf())
}

/// Conservative detection for shell builtins that can mutate the persistent
/// visible shell directory. False positives merely send a command through the
/// isolated executor, which is the safer surface.
pub fn command_may_change_directory(command: &str) -> bool {
    command
        .split([';', '\n', '&', '|'])
        .map(str::trim_start)
        .any(|segment| {
            let first = segment.split_ascii_whitespace().next().unwrap_or_default();
            matches!(first, "cd" | "chdir" | "pushd" | "popd")
                || (first == "builtin"
                    && segment
                        .split_ascii_whitespace()
                        .nth(1)
                        .is_some_and(|word| matches!(word, "cd" | "pushd" | "popd")))
        })
}

/// Resolve `.`/`..` lexically when canonicalization is unavailable. Existing
/// paths use filesystem canonicalization so a symlink cannot escape a grant.
pub(crate) fn permission_path(path: std::path::PathBuf) -> std::path::PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Absolute paths a filesystem read tool intends to inspect. This is shared by
/// the controller (approval decision) and executor (hard enforcement), so a new
/// read tool cannot accidentally gain a different security boundary.
pub fn read_paths_for_tool(name: &str, args_json: &str, pane_id: Uuid) -> Vec<std::path::PathBuf> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return Vec::new();
    };
    let mut raw = Vec::new();
    match name {
        "read_file"
        | "list_directory"
        | "list_directory_with_sizes"
        | "directory_tree"
        | "search_files"
        | "get_file_info" => {
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                raw.push(path);
            }
        }
        "grep" => raw.push(args.get("path").and_then(|v| v.as_str()).unwrap_or(".")),
        "search_codebase" => raw.push(
            args.get("repo_root")
                .and_then(|v| v.as_str())
                .unwrap_or("."),
        ),
        "read_multiple_files" => {
            if let Some(paths) = args.get("paths").and_then(|v| v.as_array()) {
                raw.extend(paths.iter().filter_map(|v| v.as_str()));
            }
        }
        "run_command" => {
            if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
                if crate::agent::approval_policy::is_safe_readonly_command(command) {
                    return read_paths_from_safe_command(command, pane_id);
                }
            }
        }
        _ => {}
    }
    raw.into_iter()
        .map(|path| permission_path(resolve_path_for_pane(path, pane_id)))
        .collect()
}

/// Extract explicit path arguments from the conservative read-only shell
/// command allowlist. This is not a general shell parser: metacharacters are
/// already rejected by `is_safe_readonly_command`, and this tokenizer only
/// preserves quoted paths (notably workspace names containing spaces).
pub(crate) fn read_paths_from_safe_command(
    command: &str,
    pane_id: Uuid,
) -> Vec<std::path::PathBuf> {
    let tokens = shell_like_tokens(command);
    let Some(program) = tokens
        .first()
        .and_then(|token| Path::new(token).file_name())
        .and_then(|name| name.to_str())
    else {
        return Vec::new();
    };
    const PATH_READERS: &[&str] = &[
        "ls", "cat", "head", "tail", "less", "more", "file", "stat", "wc", "du", "grep", "rg",
        "fd", "find", "tree", "basename", "dirname", "realpath", "readlink", "git",
    ];
    if !PATH_READERS.contains(&program) {
        return Vec::new();
    }

    tokens
        .into_iter()
        .skip(1)
        .filter_map(|token| {
            let candidate = token
                .split_once('=')
                .map(|(_, value)| value)
                .unwrap_or(token.as_str());
            if candidate.starts_with('-') {
                return None;
            }
            let expanded = if candidate == "~" {
                dirs::home_dir()?
            } else if let Some(rest) = candidate.strip_prefix("~/") {
                dirs::home_dir()?.join(rest)
            } else {
                let path = Path::new(candidate);
                if !(path.is_absolute()
                    || candidate == "."
                    || candidate == ".."
                    || candidate.starts_with("./")
                    || candidate.starts_with("../"))
                {
                    return None;
                }
                path.to_path_buf()
            };
            Some(permission_path(resolve_path_for_pane(
                expanded.to_string_lossy().as_ref(),
                pane_id,
            )))
        })
        .collect()
}

pub(crate) fn shell_like_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn external_read_paths_for_tool(
    name: &str,
    args_json: &str,
    pane_id: Uuid,
) -> Vec<std::path::PathBuf> {
    let Some(cwd) = project_cwd_for_pane(pane_id).map(permission_path) else {
        return Vec::new();
    };
    // Skill folders are read without asking: the shared one, and this
    // build's unpacked skill packages (their references and helpers).
    #[allow(unused_mut)]
    let mut skill_roots = vec![crate::agent::skill_discovery::global_skills_dir()];
    #[cfg(not(target_family = "wasm"))]
    skill_roots.push(dragent_host::agent_skills::bundle_root());
    let skill_roots: Vec<std::path::PathBuf> = skill_roots
        .into_iter()
        .map(|root| std::fs::canonicalize(&root).unwrap_or(root))
        .collect();
    read_paths_for_tool(name, args_json, pane_id)
        .into_iter()
        .filter(|path| {
            if path.starts_with(&cwd) {
                return false;
            }
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            !skill_roots.iter().any(|root| canonical.starts_with(root))
        })
        .collect()
}

/// Where a write would land, for the permission check: the deepest folder
/// that exists, resolved through symlinks, plus the rest of the path. A
/// file that doesn't exist yet under a symlinked folder is placed where the
/// write would really go, not where its spelling suggests.
pub(crate) fn write_permission_path(path: std::path::PathBuf) -> std::path::PathBuf {
    let normal = permission_path(path);
    let mut existing = normal.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return normal,
        }
    }
    let mut out = existing
        .canonicalize()
        .unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// Absolute paths a file-changing tool intends to write, move, create or
/// delete. Shared by the controller (asks first) and the executor (refuses
/// without approval), like [`read_paths_for_tool`] for reads.
pub fn write_paths_for_tool(name: &str, args_json: &str, pane_id: Uuid) -> Vec<std::path::PathBuf> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return Vec::new();
    };
    let mut raw: Vec<String> = Vec::new();
    let mut push = |value: Option<&serde_json::Value>| {
        if let Some(path) = value.and_then(|v| v.as_str()) {
            raw.push(path.to_string());
        }
    };
    match name {
        "write_file" | "edit_file" | "create_directory" | "delete_file" => push(args.get("path")),
        "move_file" => {
            push(args.get("source"));
            push(args.get("destination"));
        }
        "batch_edit" => {
            if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
                for edit in edits {
                    push(edit.get("path"));
                }
            }
        }
        "apply_patch" => {
            if let Some(patch) = args.get("patch").and_then(|v| v.as_str()) {
                if let Ok(files) = crate::agent::tools_files::parse_patch(patch) {
                    raw.extend(files.into_iter().map(|file| file.path));
                }
            }
        }
        _ => {}
    }
    raw.iter()
        .map(|path| write_permission_path(resolve_path_for_pane(path, pane_id)))
        .collect()
}

/// Write targets outside the pane's project. Like reads outside it, these
/// ask in every approval mode: "Accept edits" means edits to the project,
/// not to `~/.zshrc`.
pub fn external_write_paths_for_tool(
    name: &str,
    args_json: &str,
    pane_id: Uuid,
) -> Vec<std::path::PathBuf> {
    let Some(cwd) = project_cwd_for_pane(pane_id).map(permission_path) else {
        return Vec::new();
    };
    write_paths_for_tool(name, args_json, pane_id)
        .into_iter()
        .filter(|path| !path.starts_with(&cwd))
        .collect()
}

/// Directory persisted by “Always allow”. A directory request grants itself;
/// a file request grants its containing directory so related reads do not
/// prompt one file at a time.
pub fn recommended_read_grant_root(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}

pub fn grant_external_read_once(call_id: &str, roots: Vec<std::path::PathBuf>) {
    if let Ok(mut grants) = ONE_SHOT_READ_ROOTS.lock() {
        grants.insert(
            call_id.to_string(),
            roots.into_iter().map(permission_path).collect(),
        );
    }
}

/// Resolve using the calling tool thread's pane (set via thread-local in execute()).
pub(crate) fn resolve_path(path: &str) -> std::path::PathBuf {
    resolve_path_for_pane(path, current_pane())
}

pub(crate) fn resolve_path_for_pane(path: &str, pane_id: Uuid) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let base = PROJECT_CWD
        .lock()
        .ok()
        .and_then(|m| m.get(&pane_id).cloned())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.join(p)
}

/// Clear persisted shell state. Called when the user clears the conversation
/// so a new session starts from a clean environment.
/// The CWD is reset to the session's project directory so the next command
/// starts in the right place without needing an explicit cwd argument.
pub fn clear_shell_state(pane_id: Uuid) {
    let project_cwd = PROJECT_CWD
        .lock()
        .ok()
        .and_then(|m| m.get(&pane_id).map(|p| p.to_string_lossy().into_owned()));
    if let Ok(mut map) = SHELL_STATE.lock() {
        map.insert(
            pane_id,
            ShellState {
                cwd: project_cwd,
                env: HashMap::new(),
            },
        );
    }
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

// Execute a tool locally. Returns (output, is_error).
// Called from a background thread — must not touch any warpui/model state.
// `pane_id` isolates shell state, cwd, and REPL sessions per terminal tab.
// The pipeline call_id of the tool currently executing on this thread.
// Set by `execute_with_call_id`; used as the registry key for run_command so
// ToolStep blocks (keyed by the same call_id) can render live elapsed/output.
thread_local! {
    pub(crate) static CALL_ID: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The pipeline call id of the tool currently executing on this thread, if any.
pub(crate) fn current_call_id() -> Option<String> {
    CALL_ID.with(|c| c.borrow().clone())
}

/// Like [`execute`], but tags the execution with the pipeline call id.
pub fn execute_with_call_id(
    call_id: &str,
    name: &str,
    args_json: &str,
    pane_id: Uuid,
) -> (String, bool) {
    CALL_ID.with(|c| *c.borrow_mut() = Some(call_id.to_string()));
    // REPL snippets can mutate the project just like shell commands. The
    // run_command path owns its checkpoint lifecycle internally; REPL
    // execution is synchronous, so bracket it here when the caller has
    // registered this call id.
    if name == "repl" {
        crate::agent::checkpoint_store::begin_shell_checkpoint(call_id);
    }
    let result = execute(name, args_json, pane_id);
    if name == "repl" {
        crate::agent::checkpoint_store::finish_shell_checkpoint(call_id);
    }
    CALL_ID.with(|c| *c.borrow_mut() = None);
    if let Ok(mut grants) = ONE_SHOT_READ_ROOTS.lock() {
        grants.remove(call_id);
    }
    result
}

pub fn execute(name: &str, args_json: &str, pane_id: Uuid) -> (String, bool) {
    PANE_ID.with(|c| c.set(pane_id));
    let mut args: serde_json::Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => return (format!("Failed to parse tool args: {e}"), true),
    };

    // Hook point (opencode tool.execute.before parity): subscribers observe
    // the call and may rewrite `args`. Rewrites are re-serialized for the
    // dispatch below; untouched payloads cost one comparison.
    let mut args_json_owned = args_json.to_string();
    {
        let mut payload = serde_json::json!({"tool": name, "args": args});
        crate::agent::hooks::trigger(
            crate::agent::hooks::HookName::ToolExecuteBefore,
            &mut payload,
        );
        if let Some(rewritten) = payload.get("args") {
            if *rewritten != args {
                args = rewritten.clone();
                args_json_owned = args.to_string();
            }
        }
    }
    let args_json = args_json_owned.as_str();

    let t = web_time::Instant::now();
    let summary = summarise_args(name, &args);
    log::info!("[tool] {} — {}", name, summary);

    // Approval is not merely UI: enforce the same path boundary here so worker
    // agents and any future execution path cannot bypass the controller. A
    // persisted grant is directory-scoped; an Allow-once grant is call-scoped.
    let all_read_paths = read_paths_for_tool(name, args_json, pane_id);
    if !all_read_paths.is_empty() {
        let protected_check =
            crate::agent::permissions::AllowedRoots::new(all_read_paths.iter().cloned());
        if let Some((path, reason)) = all_read_paths.iter().find_map(|path| {
            protected_check
                .check(path)
                .err()
                .filter(|reason| {
                    matches!(reason, crate::agent::permissions::DenyReason::SensitivePath)
                })
                .map(|reason| (path, reason))
        }) {
            return (
                format!("Permission denied for {}: {reason}", path.display()),
                true,
            );
        }
    }

    let external_paths = external_read_paths_for_tool(name, args_json, pane_id);
    if !external_paths.is_empty() {
        let persisted = crate::agent::permission_store::PermissionStore::load();
        let one_shot = current_call_id()
            .and_then(|call_id| {
                ONE_SHOT_READ_ROOTS
                    .lock()
                    .ok()
                    .and_then(|grants| grants.get(&call_id).cloned())
            })
            .unwrap_or_default();
        let authorized = external_paths.iter().all(|path| {
            persisted.is_read_path_allowed(path)
                || one_shot.iter().any(|root| path.starts_with(root))
        });
        if !authorized {
            return (
                format!(
                    "Permission required: reading outside the project needs user approval: {}",
                    external_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                true,
            );
        }
    }

    // Writes get the same boundary: sensitive locations are refused, and a
    // change outside the project needs this call's approval (a persisted
    // read grant doesn't cover writing).
    let all_write_paths = write_paths_for_tool(name, args_json, pane_id);
    if !all_write_paths.is_empty() {
        let protected_check =
            crate::agent::permissions::AllowedRoots::new(all_write_paths.iter().cloned());
        if let Some((path, reason)) = all_write_paths.iter().find_map(|path| {
            protected_check
                .check(path)
                .err()
                .filter(|reason| {
                    matches!(reason, crate::agent::permissions::DenyReason::SensitivePath)
                })
                .map(|reason| (path, reason))
        }) {
            return (
                format!("Permission denied for {}: {reason}", path.display()),
                true,
            );
        }
    }
    let external_writes = external_write_paths_for_tool(name, args_json, pane_id);
    if !external_writes.is_empty() {
        let one_shot = current_call_id()
            .and_then(|call_id| {
                ONE_SHOT_READ_ROOTS
                    .lock()
                    .ok()
                    .and_then(|grants| grants.get(&call_id).cloned())
            })
            .unwrap_or_default();
        if !external_writes
            .iter()
            .all(|path| one_shot.iter().any(|root| path.starts_with(root)))
        {
            return (
                format!(
                    "Permission required: changing files outside the project needs user approval: {}",
                    external_writes
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                true,
            );
        }
    }

    let result = match name {
        #[cfg(not(target_family = "wasm"))]
        "computer" => match args["action"].as_str() {
            Some(action) => {
                match crate::cli::computer::agent_call(action, args.clone(), pane_id.to_string()) {
                    Ok(output) => (output, false),
                    Err(error) => (error, true),
                }
            }
            None => ("computer needs an action".into(), true),
        },
        "read_file" => exec_read_file(&args),
        "read_multiple_files" => exec_read_multiple_files(&args),
        "write_file" => exec_write_file(&args),
        "edit_file" => exec_edit_file(&args),
        "batch_edit" => exec_batch_edit(&args),
        "apply_patch" => exec_apply_patch(&args),
        "create_directory" => exec_create_directory(&args),
        "list_directory" => exec_list_directory(&args),
        "list_directory_with_sizes" => exec_list_directory_with_sizes(&args),
        "directory_tree" => exec_directory_tree(&args),
        "move_file" => exec_move_file(&args),
        "search_files" => exec_search_files(&args),
        "get_file_info" => exec_get_file_info(&args),
        "list_allowed_directories" => exec_list_allowed_directories(&args),
        "grep" => exec_grep(&args),
        "search_codebase" => exec_search_codebase(&args),
        "run_command" => exec_run_command(&args),
        "github_create_pull_request" => exec_github_create_pull_request(&args),
        "manage_task" => exec_manage_task(&args),
        "spawn_subagent" => exec_spawn_subagent(&args),
        // Web tools on the cloud path are executed by the SERVER (see the
        // backend's webtools package): those events arrive display-only and
        // never reach this dispatch. Direct provider steps have no server,
        // so fetch_url executes locally; web_search has no local provider
        // and stays an explicit graceful error.
        "fetch_url" => exec_fetch_url(&args),
        "web_search" => (
            "Web search runs on Dragent cloud and is unavailable in local direct mode. \
             Use fetch_url with a specific URL, or ask the user for the information instead."
                .to_string(),
            true,
        ),
        "skill" => exec_skill(&args),
        "repl" => exec_repl(&args),
        "sleep" => exec_sleep(&args),
        "mcp_auth" => exec_mcp_auth(&args),
        // ask_user is resolved by the controller through an interactive card, not
        // executed here. Reaching this arm means the interception was bypassed;
        // fail loudly rather than hang the stream.
        "ask_user" => (
            "ask_user is handled by the UI, not tool execution.".to_string(),
            true,
        ),
        // todowrite is resolved by the controller into the live Todos card, not
        // executed here — same guard as ask_user.
        "todowrite" => (
            "todowrite is handled by the UI, not tool execution.".to_string(),
            true,
        ),
        // Coordination, for a tab that leads it: the tab is the lead.
        #[cfg(not(target_family = "wasm"))]
        name if name.starts_with(crate::cli::team::DRAGENT_PREFIX) => {
            match crate::cli::team::call_as_lead(
                name.trim_start_matches(crate::cli::team::DRAGENT_PREFIX),
                &args,
                &pane_id.to_string(),
            ) {
                Ok(output) => (output, false),
                Err(error) => (error, true),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        name if name.starts_with(crate::integrations::tools::AGENT_PREFIX) => {
            let root = project_cwd_for_pane(pane_id)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            match crate::integrations::tools::call(
                name.trim_start_matches(crate::integrations::tools::AGENT_PREFIX),
                &args,
                &root,
            ) {
                Ok(output) => (output, false),
                Err(error) => (error, true),
            }
        }
        name if name.starts_with("mcp__") => crate::agent::mcp::call_tool(name, args_json),
        name if name.starts_with("prompt__") => crate::agent::mcp::call_prompt(name, args_json),
        name if name.starts_with("resource__") => crate::agent::mcp::call_resource(name),
        unknown => (format!("Unknown tool: {unknown}"), true),
    };

    let (mut output, is_err) = result;
    // A structured failure reaches the model as the same object the command
    // line and MCP server return, not its transport encoding.
    #[cfg(not(target_family = "wasm"))]
    if is_err {
        if let Some(failure) = dragent_host::outcome::Failure::decode(&output) {
            output = serde_json::to_string_pretty(&failure.to_json()).unwrap_or(output);
        }
    }
    // Hook point (opencode tool.execute.after parity): subscribers observe
    // and may rewrite `output` (e.g. redact, annotate) before it flows to
    // metrics, logs, and the model.
    {
        let mut payload = serde_json::json!({"tool": name, "output": output, "is_error": is_err});
        crate::agent::hooks::trigger(
            crate::agent::hooks::HookName::ToolExecuteAfter,
            &mut payload,
        );
        if let Some(replacement) = payload.get("output").and_then(|v| v.as_str()) {
            if replacement != output {
                output = replacement.to_string();
            }
        }
    }
    // Structured per-call metrics: one greppable line per tool execution so
    // latency/error-rate/token questions have data, not anecdotes.
    log::info!(
        "[tool-metrics] tool={} ms={} out_bytes={} out_tokens~={} err={}",
        name,
        t.elapsed().as_millis(),
        output.len(),
        approx_tokens(&output),
        is_err,
    );
    if is_err {
        log::warn!("[tool] {} FAILED: {}", name, utf8_prefix(&output, 120));
    }
    (output, is_err)
}

pub(crate) fn summarise_args(name: &str, args: &serde_json::Value) -> String {
    match name {
        "computer" => format!(
            "{} {}",
            args["action"].as_str().unwrap_or("?"),
            args["app"].as_str().unwrap_or("")
        ),
        "read_file" | "write_file" | "edit_file" | "list_directory" => {
            args["path"].as_str().unwrap_or("?").to_string()
        }
        "grep" => format!(
            "pattern={:?} path={}",
            args["pattern"].as_str().unwrap_or("?"),
            args["path"].as_str().unwrap_or(".")
        ),
        "run_command" => utf8_prefix(args["command"].as_str().unwrap_or("?"), 80).to_string(),
        "github_create_pull_request" => format!(
            "base={} head={} title={}",
            args["base"].as_str().unwrap_or("?"),
            args["head"].as_str().unwrap_or("?"),
            args["title"].as_str().unwrap_or("?")
        ),
        "search_codebase" => args["query"].as_str().unwrap_or("?").to_string(),
        "batch_edit" => format!(
            "{} edits",
            args["edits"].as_array().map(|a| a.len()).unwrap_or(0)
        ),
        "spawn_subagent" => utf8_prefix(args["query"].as_str().unwrap_or("?"), 80).to_string(),
        "fetch_url" => args["url"].as_str().unwrap_or("?").to_string(),
        "web_search" => args["query"].as_str().unwrap_or("?").to_string(),
        "skill" => format!(
            "name={} args={}",
            args["name"].as_str().unwrap_or("?"),
            args["args"].as_str().unwrap_or("")
        ),
        "repl" => format!(
            "lang={} code={}",
            args["language"].as_str().unwrap_or("python"),
            utf8_prefix(args["code"].as_str().unwrap_or("?"), 60)
        ),
        "sleep" => format!("{}ms", args["ms"].as_u64().unwrap_or(0)),
        "mcp_auth" => args["server_name"].as_str().unwrap_or("?").to_string(),
        // Coordination: the task (or who a message is for, or a new task's
        // title) says enough.
        name if name.starts_with("coord_") => ["title", "task", "to"]
            .iter()
            .find_map(|key| args[*key].as_str())
            .map(|text| utf8_prefix(text, 80).to_string())
            .or_else(|| {
                args["tasks"].as_array().map(|tasks| {
                    tasks
                        .iter()
                        .filter_map(|task| task.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            })
            .unwrap_or_default(),
        _ => {
            let serialized = args.to_string();
            utf8_prefix(&serialized, 80).to_string()
        }
    }
}

// ── fetch_url (local direct-mode executor) ──────────────────────────────────

/// Download cap: enough for documentation pages, bounded for context.
const FETCH_MAX_BYTES: usize = 512 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 30;

/// Validate a fetch target. Only http(s); no control characters that could
/// smuggle request splitting into the URL.
fn validated_fetch_url(url: &str) -> Result<String, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("fetch_url needs a `url` argument.".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "Only http(s) URLs can be fetched, got: {}",
            utf8_prefix(&url, 80)
        ));
    }
    if url.chars().any(|c| c.is_control()) {
        return Err("URL contains control characters.".to_string());
    }
    Ok(url)
}

/// Strip a page down to readable text: drop script/style blocks, turn block
/// tags into newlines, remove the rest, decode common entities, collapse
/// whitespace. Pure and offline-testable; the network half stays in
/// `exec_fetch_url`.
fn html_to_text(html: &str) -> String {
    // Drop script/style blocks (case-insensitive).
    let mut without_blocks = String::with_capacity(html.len());
    {
        let lower = html.to_ascii_lowercase();
        let mut i = 0;
        while i < html.len() {
            let rest = &lower[i..];
            let is_script = rest.starts_with("<script");
            let is_style = rest.starts_with("<style");
            if is_script || is_style {
                let tag = if is_script { "script" } else { "style" };
                let close = format!("</{tag}");
                match lower[i..].find(&close) {
                    Some(end) => {
                        // Skip past `</tag>` and its closing `>`.
                        let after = i + end + close.len();
                        i = lower[after..]
                            .find('>')
                            .map(|k| after + k + 1)
                            .unwrap_or(html.len());
                        without_blocks.push('\n');
                    }
                    None => break,
                }
            } else {
                // Copy one char (UTF-8 safe: advance by char boundary).
                let ch = html[i..].chars().next().unwrap();
                without_blocks.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    // Block tags → newline, inline tags → removed.
    let mut text = String::with_capacity(without_blocks.len());
    {
        let bytes = without_blocks.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'<' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'>' {
                    j += 1;
                }
                let tag_head: String = without_blocks[i + 1..j.min(bytes.len())]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '/')
                    .collect();
                let tag = tag_head.to_ascii_lowercase();
                if matches!(
                    tag.as_str(),
                    "p" | "div"
                        | "br"
                        | "li"
                        | "ul"
                        | "ol"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "tr"
                        | "table"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "nav"
                        | "blockquote"
                        | "pre"
                        | "hr"
                ) {
                    text.push('\n');
                }
                i = (j + 1).min(bytes.len());
            } else {
                let ch = without_blocks[i..].chars().next().unwrap();
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    // Entities + whitespace collapse (max one blank line).
    let decoded = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let mut out = String::with_capacity(decoded.len());
    let mut blank_run = 0;
    for line in decoded.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run == 1 {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        // Collapse interior runs of whitespace.
        let mut prev_space = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !prev_space {
                    out.push(' ');
                }
                prev_space = true;
            } else {
                out.push(ch);
                prev_space = false;
            }
        }
        out.push('\n');
    }
    out.trim().to_string()
}

pub(crate) fn exec_fetch_url(args: &serde_json::Value) -> (String, bool) {
    let url = match validated_fetch_url(args["url"].as_str().unwrap_or("")) {
        Ok(u) => u,
        Err(e) => return (e, true),
    };
    #[cfg(target_family = "wasm")]
    {
        let _ = &url;
        return (
            "fetch_url is unavailable in the web viewer.".to_string(),
            true,
        );
    }
    #[cfg(not(target_family = "wasm"))]
    {
        use std::io::Read as _;
        let response = ureq::builder()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout_read(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .build()
            .get(&url)
            .set("User-Agent", "Dragent/1.0 (+https://dragent.dev)")
            .call();
        let response = match response {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                let detail = detail
                    .split_whitespace()
                    .take(20)
                    .collect::<Vec<_>>()
                    .join(" ");
                return (format!("Fetch failed (HTTP {code}): {detail}"), true);
            }
            Err(ureq::Error::Transport(t)) => {
                return (format!("Fetch failed: {t}"), true);
            }
        };
        let content_type = response.header("content-type").unwrap_or("").to_lowercase();
        let mut body = Vec::new();
        if let Err(e) = response
            .into_reader()
            .take(FETCH_MAX_BYTES as u64 + 1)
            .read_to_end(&mut body)
        {
            return (format!("Fetch failed while reading: {e}"), true);
        }
        if body.len() > FETCH_MAX_BYTES {
            return (
                format!("Page exceeds the {FETCH_MAX_BYTES}-byte fetch cap; narrow the URL."),
                true,
            );
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        let rendered = if content_type.contains("html") {
            html_to_text(&text)
        } else if content_type.is_empty()
            || ["text/", "json", "xml", "javascript"]
                .iter()
                .any(|t| content_type.contains(t))
        {
            text
        } else {
            return (
                format!("Unsupported content type for text fetch: {content_type}"),
                true,
            );
        };
        if rendered.trim().is_empty() {
            return ("The page contained no readable text.".to_string(), true);
        }
        (
            crate::agent::pipeline::stream::bounded_tool_output(rendered),
            false,
        )
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::{html_to_text, validated_fetch_url};

    #[test]
    fn url_validation_accepts_http_only() {
        assert!(validated_fetch_url("https://example.com/x").is_ok());
        assert!(validated_fetch_url("http://localhost:8000/").is_ok());
        assert!(validated_fetch_url("").is_err());
        assert!(validated_fetch_url("ftp://x/y").is_err());
        assert!(validated_fetch_url("javascript:alert(1)").is_err());
        assert!(validated_fetch_url("https://x/\nInjected: 1").is_err());
    }

    #[test]
    fn html_strips_scripts_and_tags() {
        let out = html_to_text(
            "<html><head><style>.x{color:red}</style><script>alert(1)</script></head>\
             <body><h1>Title</h1><p>Hello <b>world</b></p></body></html>",
        );
        assert!(out.contains("Title"), "got: {out}");
        assert!(out.contains("Hello world"), "got: {out}");
        assert!(!out.contains("alert"), "got: {out}");
        assert!(!out.contains("color"), "got: {out}");
        assert!(!out.contains('<'), "got: {out}");
    }

    #[test]
    fn html_decodes_entities_and_collapses_space() {
        let out = html_to_text("<p>A &amp; B&nbsp;&nbsp; C</p><p>D</p>");
        assert_eq!(out, "A & B C\nD");
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod coordination_tests {
    use super::*;

    #[test]
    fn a_lead_tab_runs_only_the_leads_tools_and_names_them_briefly() {
        // A worker's tool isn't the lead's: refused before anything is asked
        // of the background service.
        let (output, failed) = execute("coord_task_done", "{}", Uuid::new_v4());
        assert!(
            failed && output.contains("isn't one of the lead's tools"),
            "{output}"
        );
        assert_eq!(
            summarise_args(
                "coord_task_add",
                &serde_json::json!({"title": "Invite API", "prompt": "…"})
            ),
            "Invite API"
        );
        assert_eq!(
            summarise_args(
                "coord_task_start",
                &serde_json::json!({"tasks": ["t2", "t3"]})
            ),
            "t2, t3"
        );
    }
}
