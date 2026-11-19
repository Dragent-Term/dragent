//! Lightweight, deterministic task routing for the agent system prompt.
//!
//! This is intentionally local and cheap. It does not try to solve the task;
//! it gives the main model a useful starting protocol and prevents generic
//! exploration when the request clearly belongs to a known workflow.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskKind {
    Conversational,
    General,
}

fn classify(query: &str) -> TaskKind {
    let q = query.trim().to_ascii_lowercase();
    let q = q.trim_matches(|c: char| c.is_ascii_punctuation()).trim();
    // This is only a latency fast path for messages that are entirely a social
    // acknowledgement. All actual task intent is interpreted by the model from
    // the complete request; no keyword list routes implementation behavior.
    if q.is_empty()
        || matches!(
            q,
            "hello"
                | "hi"
                | "hey"
                | "thanks"
                | "thank you"
                | "good morning"
                | "good afternoon"
                | "good evening"
        )
    {
        return TaskKind::Conversational;
    }
    TaskKind::General
}

fn existing_runner_roots() -> Vec<String> {
    let mut roots = Vec::new();
    for key in ["ACTIONS_RUNNER_ROOT", "DRAGENT_ACTIONS_RUNNER_ROOT"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() && Path::new(&value).is_dir() {
                roots.push(format!("{key}: {value}"));
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join("actions-runner");
        if candidate.is_dir() {
            roots.push(format!("default runner root: {}", candidate.display()));
        }
    }
    roots
}

/// Build the compact workflow block appended to the model's system prompt.
pub fn build(query: &str, cwd: &Path, os: &str, shell: &str) -> String {
    if classify(query) == TaskKind::Conversational {
        return String::new();
    }
    let cwd_display = cwd.to_string_lossy();

    let runner_roots = existing_runner_roots();
    let known = if runner_roots.is_empty() {
        "No known self-hosted runner root was detected locally; ask before doing a broad search."
            .to_string()
    } else {
        runner_roots.join("; ")
    };

    format!(
        "## Task profile\n\n\
User objective: {query}\n\
Runtime: OS={os}; shell={shell}; working directory={cwd_display}\n\
Known runner locations: {known}\n\n\
Execution contract: infer whether the user wants an answer, investigation, plan, or implementation from the full request and conversation. Preserve useful evidence already gathered, inspect only what is missing, act when action is requested, verify changes, and stop once the objective is complete."
    )
}

#[cfg(test)]
mod tests {
    use super::{build, classify, TaskKind};
    use std::path::Path;

    #[test]
    fn real_tasks_use_the_universal_execution_contract() {
        let block = build(
            "Could you make DepartmentsStep easier to use?",
            Path::new("/workspace"),
            "macos",
            "zsh",
        );
        assert!(block.contains("infer whether the user wants"));
        assert!(block.contains("act when action is requested"));
        assert!(!block.contains("Task category"));
    }

    #[test]
    fn includes_runtime_and_objective() {
        let block = build(
            "Why is the runner offline?",
            Path::new("/workspace"),
            "macos",
            "zsh",
        );
        assert!(block.contains("working directory=/workspace"));
        assert!(block.contains("User objective: Why is the runner offline?"));
        assert!(block.contains("stop once the objective is complete"));
    }

    #[test]
    fn greeting_fast_path_does_not_swallow_a_task() {
        assert_eq!(classify("hello"), TaskKind::Conversational);
        assert_eq!(
            classify("hello, please build a login page"),
            TaskKind::General
        );
        assert_eq!(classify("thanks, now fix the test"), TaskKind::General);
    }
}
