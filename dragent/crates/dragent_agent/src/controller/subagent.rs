//! Subagent (worker) policy: nesting depth, roles, and tool allowlists.
//!
//! Pure policy for `spawn_subagent` execution. The app enforces it at the
//! dispatch sites (`exec_spawn_subagent` depth gate, worker tool-call arm
//! role gate); the future local engine inherits the same module unchanged.
//!
//! Moved-in behavior notes:
//! - Depth used to be enforced only by prompt text ("Do NOT spawn additional
//!   sub-agents"). Prompt text is advisory; this is a hard gate.
//! - Workers used to have one shape (full tool access). `Explore` is a
//!   read-only research role: it cannot mutate anything, so coordinators can
//!   fan research out without handing every worker the keys.

/// Maximum nesting depth for spawned workers. A depth-0 caller (the terminal
/// agent or a coordinator) may spawn; a depth-1 worker may not spawn further.
/// Mirrors opencode's `subagent_depth` default of 1.
pub const MAX_SUBAGENT_DEPTH: u32 = 1;

/// Worker role selected via the `role` arg of `spawn_subagent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkerRole {
    /// Full tool access. Executes the subtask and summarizes.
    #[default]
    Worker,
    /// Read-only research. Gathers evidence, never mutates.
    Explore,
}

impl WorkerRole {
    /// Parse the `role` spawn arg. Unknown values are an error (fail fast so
    /// a typo doesn't silently run a full worker).
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "worker" => Ok(Self::Worker),
            "explore" => Ok(Self::Explore),
            other => Err(format!(
                "Unknown worker role '{other}'. Valid roles: worker, explore."
            )),
        }
    }

    /// Role name as used in the spawn args and log lines.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Explore => "explore",
        }
    }
}

/// True when a caller at `depth` may spawn another worker.
pub fn depth_allows_spawn(depth: u32) -> bool {
    depth < MAX_SUBAGENT_DEPTH
}

/// Tool error returned to the model when a worker tries to spawn past the
/// depth limit. Guides it to do the work directly instead of delegating.
pub fn depth_reject_message() -> String {
    format!(
        "Subagent depth limit reached ({MAX_SUBAGENT_DEPTH}): workers cannot spawn \
         further subagents. Do the work directly with your own tools instead of \
         delegating, or return what you have so the coordinator can split the \
         task differently."
    )
}

/// Tools an `Explore` worker may call. Pure reads only: no writes, no shell,
/// no spawning, no questions/todos (coordination belongs to the parent).
pub const EXPLORE_ALLOWED_TOOLS: &[&str] = &[
    "read_file",
    "read_multiple_files",
    "list_directory",
    "list_directory_with_sizes",
    "directory_tree",
    "get_file_info",
    "list_allowed_directories",
    "search_codebase",
    "search_files",
    "grep",
    "find",
    "web_search",
    "fetch_url",
    "fetch_web_page",
];

/// True when `role` may execute `tool_name`.
pub fn role_allows_tool(role: WorkerRole, tool_name: &str) -> bool {
    match role {
        WorkerRole::Worker => true,
        WorkerRole::Explore => EXPLORE_ALLOWED_TOOLS.contains(&tool_name),
    }
}

/// Tool error returned to an explore worker's loop when it attempts a
/// mutating tool. Tells it to report back instead of acting.
pub fn role_deny_message(role: WorkerRole, tool_name: &str) -> String {
    match role {
        WorkerRole::Worker => unreachable!("workers allow every tool"),
        WorkerRole::Explore => format!(
            "Explore workers are read-only: '{tool_name}' is not available. \
             Gather evidence with read/search tools and return your findings \
             to the coordinator instead of acting on them."
        ),
    }
}

/// System prompt for a full worker. Intentionally minimal so the
/// static/dynamic boundary remains stable for prompt caching.
pub fn worker_system_prompt() -> &'static str {
    "You are a Worker agent executing a subtask assigned by a Coordinator. \
     You have full tool access. Complete the assigned task using available tools, \
     then write a concise summary of what you did and any outcomes. \
     If a tool reports that an external path needs user permission, stop and return \
     the exact path to the Coordinator; do not retry through a shell command. \
     Do NOT spawn additional sub-agents."
}

/// System prompt for a read-only explore worker. Research-only: find facts,
/// report them, change nothing.
pub fn explore_system_prompt() -> &'static str {
    "You are an Explore agent researching for a Coordinator. You are READ-ONLY: \
     use read and search tools to gather evidence, then report your findings \
     concisely with file paths and line references. Do not write files, edit \
     files, run commands, or change anything. If the evidence points to a fix, \
     describe it — do not apply it. Do NOT spawn additional sub-agents."
}

/// System prompt for `role`.
pub fn system_prompt_for_role(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Worker => worker_system_prompt(),
        WorkerRole::Explore => explore_system_prompt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_boundary_allows_first_level_only() {
        assert!(depth_allows_spawn(0));
        assert!(!depth_allows_spawn(1));
        assert!(!depth_allows_spawn(7));
    }

    #[test]
    fn role_parse_accepts_known_rejects_unknown() {
        assert_eq!(WorkerRole::parse("worker"), Ok(WorkerRole::Worker));
        assert_eq!(WorkerRole::parse("explore"), Ok(WorkerRole::Explore));
        assert!(WorkerRole::parse("admin").is_err());
        assert!(WorkerRole::parse("").is_err());
    }

    #[test]
    fn explore_role_is_read_only() {
        for tool in [
            "read_file",
            "grep",
            "search_codebase",
            "list_directory",
            "web_search",
        ] {
            assert!(role_allows_tool(WorkerRole::Explore, tool), "{tool}");
        }
        for tool in [
            "write_file",
            "edit_file",
            "run_command",
            "spawn_subagent",
            "ask_user",
            "todowrite",
        ] {
            assert!(!role_allows_tool(WorkerRole::Explore, tool), "{tool}");
        }
        assert!(role_allows_tool(WorkerRole::Worker, "run_command"));
    }

    #[test]
    fn prompts_are_distinct_and_non_empty() {
        assert!(!worker_system_prompt().is_empty());
        assert!(!explore_system_prompt().is_empty());
        assert_ne!(worker_system_prompt(), explore_system_prompt());
        assert!(explore_system_prompt().contains("READ-ONLY"));
    }

    #[test]
    fn reject_messages_guide_the_model() {
        assert!(depth_reject_message().contains("Do the work directly"));
        assert!(role_deny_message(WorkerRole::Explore, "write_file").contains("read-only"));
    }
}
