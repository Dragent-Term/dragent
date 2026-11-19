//! Conservative detection for requests whose outcome should be a shell command.
//!
//! This runs locally before an agent request is sent. It is intentionally a
//! high-confidence heuristic: ambiguous requests stay on the normal coding
//! agent path instead of being turned into executable commands.

/// Return true for an explicit request to change or operate the local shell
/// environment (for example, switching branches or changing directories).
pub fn is_command_action_request(query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    let words: Vec<&str> = q
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return false;
    }

    let action = words.iter().any(|word| {
        matches!(
            *word,
            "change"
                | "switch"
                | "checkout"
                | "move"
                | "go"
                | "set"
                | "use"
                | "start"
                | "stop"
                | "restart"
                | "kill"
                | "run"
                | "install"
                | "uninstall"
                | "update"
                | "pull"
                | "push"
                | "merge"
                | "rebase"
                | "export"
                | "unset"
                | "execute"
        )
    });
    let target = words.iter().any(|word| {
        matches!(
            *word,
            "branch"
                | "branches"
                | "directory"
                | "folder"
                | "cwd"
                | "path"
                | "location"
                | "remote"
                | "port"
                | "server"
                | "process"
                | "environment"
                | "variable"
                | "dependency"
                | "package"
                | "shell"
                | "worktree"
                | "dir"
        )
    });
    let test_target = words
        .iter()
        .any(|word| matches!(*word, "test" | "tests" | "build" | "check" | "checks"));
    let code_context = words.iter().any(|word| {
        matches!(
            *word,
            "function"
                | "component"
                | "code"
                | "implementation"
                | "logic"
                | "class"
                | "module"
                | "page"
                | "method"
        )
    });

    // Direct shell-state phrasing is high confidence even without a target
    // noun: "cd into the project", "git switch main", etc.
    let known_command = words.iter().any(|word| {
        matches!(
            *word,
            "yarn"
                | "npm"
                | "pnpm"
                | "bun"
                | "cargo"
                | "go"
                | "pytest"
                | "make"
                | "gradle"
                | "mvn"
                | "docker"
                | "podman"
        )
    });
    let direct_command = words
        .first()
        .is_some_and(|word| matches!(*word, "cd" | "chdir"))
        || (words.first() == Some(&"git")
            && words.get(1).is_some_and(|word| {
                matches!(*word, "checkout" | "switch" | "push" | "pull" | "rebase")
            }))
        || (action && known_command);

    direct_command || (action && !code_context && (target || (test_target && known_command)))
}

/// True only when the user asked to change the visible shell's working
/// directory. Mentioning that a command belongs in another folder is not
/// enough: that case should use the background executor with a `cwd`.
pub fn explicitly_requests_terminal_cwd_change(query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    let words: Vec<&str> = q
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let names_cd = words.iter().any(|word| matches!(*word, "cd" | "chdir"));
    let rejects_cd = q.contains("do not cd")
        || q.contains("don't cd")
        || q.contains("dont cd")
        || q.contains("without cd");
    if names_cd && !rejects_cd {
        return true;
    }

    let change = words
        .iter()
        .any(|word| matches!(*word, "change" | "move" | "switch" | "go"));
    let directory = words.iter().any(|word| {
        matches!(
            *word,
            "directory" | "folder" | "cwd" | "path" | "location" | "dir"
        )
    });
    change && directory
}

#[cfg(test)]
mod tests {
    use super::{explicitly_requests_terminal_cwd_change, is_command_action_request};

    #[test]
    fn recognizes_shell_environment_actions() {
        for query in [
            "change the git branch to main",
            "switch to the feature branch",
            "change my working directory to the server folder",
            "start the dev server on port 3000",
            "install the missing package",
            "cd into the backend directory",
            "run yarn test",
        ] {
            assert!(is_command_action_request(query), "not recognized: {query}");
        }
    }

    #[test]
    fn leaves_coding_requests_on_the_normal_agent_path() {
        for query in [
            "change this function to use a cache",
            "fix the branch logic in this component",
            "add a package loading state to the page",
            "review the server implementation",
            "run a test",
        ] {
            assert!(!is_command_action_request(query), "misclassified: {query}");
        }
    }

    #[test]
    fn distinguishes_visible_cd_requests_from_commands_targeting_another_folder() {
        assert!(explicitly_requests_terminal_cwd_change(
            "cd to the backend and run the tests"
        ));
        assert!(explicitly_requests_terminal_cwd_change(
            "please cd to the backend, then run the tests"
        ));
        assert!(explicitly_requests_terminal_cwd_change(
            "change my working directory to backend"
        ));
        assert!(!explicitly_requests_terminal_cwd_change(
            "run the tests in the backend folder"
        ));
        assert!(!explicitly_requests_terminal_cwd_change(
            "use backend as the command path"
        ));
        assert!(!explicitly_requests_terminal_cwd_change(
            "run it in backend without cd"
        ));
    }
}
