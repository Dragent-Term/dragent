//! Agent tool display helpers.
//!
//! Tool names, labels, and read-only classification. Moved verbatim from
//! `app/src/agent/controller/tools.rs` (pure, zero WarpUI deps).

pub fn is_checkpoint_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "write_file"
            | "edit_file"
            | "batch_edit"
            | "apply_patch"
            | "create_directory"
            | "move_file"
    )
}

pub fn is_read_only_tool_op(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file"
            | "list_directory"
            | "search_codebase"
            | "grep"
            | "find"
            | "web_search"
            | "fetch_url"
            | "fetch_web_page"
            | "read_multiple_files"
            | "list_directory_with_sizes"
            | "directory_tree"
            | "search_files"
            | "get_file_info"
            | "list_allowed_directories"
    )
}

/// Returns a human-readable activity label for a tool call, shown briefly in
/// the Loading block just before it is replaced by the ToolStep block.
pub fn tool_activity_label(tool_name: &str) -> &'static str {
    match tool_name {
        "read_file"
        | "read_multiple_files"
        | "list_directory"
        | "list_directory_with_sizes"
        | "directory_tree"
        | "get_file_info"
        | "list_allowed_directories" => "Reading…",
        "write_file" | "edit_file" | "batch_edit" | "apply_patch" | "create_directory"
        | "move_file" => "Writing…",
        "search_codebase" | "grep" | "find" | "search_files" => "Searching…",
        "run_command" => "Running…",
        "github_create_pull_request" => "Creating pull request…",
        "web_search" | "fetch_url" => "Searching web…",
        _ => "Working…",
    }
}

/// Converts a snake_case tool name to a human-readable "Title Case" label
/// displayed on ToolStep blocks while a tool is running or after it completes.
/// e.g. "run_command" → "Run Command", "read_file" → "Read File"
pub fn tool_display_name(tool_name: &str) -> String {
    // MCP tools arrive prefixed as "mcp__<server>__<tool>" (prompts/resources as
    // "prompt__…"/"resource__…""). Show only the humanized tool action — never the
    // "MCP" marker or the server name — so a block reads "Read File", "Grep",
    // "Move File", etc. This also lets MCP file/search tools fall into the same
    // activity-group buckets as the built-in ones.
    humanize_tool_action(strip_mcp_prefix(tool_name))
}

/// Drop the `mcp__`/`prompt__`/`resource__` marker and the `<server>` segment,
/// leaving just the bare tool name. Non-MCP tool names pass through unchanged.
pub fn strip_mcp_prefix(name: &str) -> &str {
    for prefix in ["mcp__", "prompt__", "resource__"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `rest` is "<server>__<tool>"; drop the server segment if present.
            return rest.find("__").map(|i| &rest[i + 2..]).unwrap_or(rest);
        }
    }
    name
}

/// Title-case an `snake_case` (or `__`-joined) tool name into a display label,
/// skipping empty segments so leftover `__` never yields double spaces.
pub fn humanize_tool_action(name: &str) -> String {
    name.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
