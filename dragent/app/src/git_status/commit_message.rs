//! AI commit-message generation from the repository's complete change set.
//!
//! This module is deliberately read-only: generating a suggestion must never
//! stage files or otherwise mutate the index. The prompt distinguishes HEAD →
//! INDEX changes from INDEX → WORKTREE changes and also includes untracked
//! files, so partially staged repositories are represented correctly.

use std::path::Path;
use std::sync::Arc;

use crate::agent::AgentApi;

const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_UNTRACKED_FILE_BYTES: usize = 12 * 1024;

pub fn generate_for_repository(
    cwd: &Path,
    agent_api: &Arc<dyn AgentApi>,
    model: &str,
) -> Result<String, String> {
    let changes = collect_repository_changes(cwd)?;
    if changes.files.is_empty() {
        return Err("There are no Git changes to summarize".to_string());
    }

    let commitlint = detect_commitlint_config(cwd);
    let prompt = build_prompt(&changes, commitlint.as_deref());
    match call_agent_step(agent_api, model, &prompt) {
        Ok(message) => {
            let message = clean_model_message(&message);
            if message.is_empty() {
                log::warn!(
                    "[git-commit-ai] cloud model returned an empty message; using local fallback"
                );
                Ok(local_commit_message(&changes))
            } else {
                Ok(message)
            }
        }
        Err(error) => {
            // Signed-out users and temporarily unavailable providers still get
            // a useful suggestion. This is intentionally deterministic and
            // local: no second network request and no mutation of the index.
            log::warn!("[git-commit-ai] {error}; using local fallback");
            Ok(local_commit_message(&changes))
        }
    }
}

#[derive(Default)]
struct RepositoryChanges {
    files: Vec<String>,
    staged: String,
    unstaged: String,
    untracked: String,
}

#[cfg(not(target_family = "wasm"))]
fn collect_repository_changes(cwd: &Path) -> Result<RepositoryChanges, String> {
    use crate::process_ext::NoWindow;
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
        let output = crate::git_cmd::GitCommand::new()
            .no_window()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .map_err(|error| format!("Could not run git: {error}"))?;
        if !output.status.success() {
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if reason.is_empty() {
                format!("git {} failed", args.join(" "))
            } else {
                reason
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    let root = super::repo_root(cwd);
    let status = git(&root, &["status", "--short"])?;
    let files = status
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let staged = git(&root, &["diff", "--cached", "--no-ext-diff", "--unified=3"])?;
    let unstaged = git(&root, &["diff", "--no-ext-diff", "--unified=3"])?;
    let untracked_paths = git(&root, &["ls-files", "--others", "--exclude-standard"])?;

    let mut untracked = String::new();
    for relative in untracked_paths.lines().filter(|path| !path.is_empty()) {
        untracked.push_str("\n--- untracked: ");
        untracked.push_str(relative);
        untracked.push_str(" ---\n");
        match std::fs::read(root.join(relative)) {
            Ok(bytes) if bytes.iter().take(8_192).any(|byte| *byte == 0) => {
                untracked.push_str("[binary file]\n");
            }
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                untracked.push_str(truncate_utf8(&text, MAX_UNTRACKED_FILE_BYTES));
                if bytes.len() > MAX_UNTRACKED_FILE_BYTES {
                    untracked.push_str("\n… [file truncated]\n");
                }
            }
            Err(error) => {
                untracked.push_str(&format!("[could not read file: {error}]\n"));
            }
        }
    }

    Ok(RepositoryChanges {
        files,
        staged,
        unstaged,
        untracked,
    })
}

#[cfg(target_family = "wasm")]
fn collect_repository_changes(_cwd: &Path) -> Result<RepositoryChanges, String> {
    Err("AI commit generation is unavailable in the web viewer".to_string())
}

fn build_prompt(changes: &RepositoryChanges, commitlint: Option<&str>) -> String {
    let mut prompt = String::new();
    if let Some(config) = commitlint {
        prompt.push_str("The repository enforces these commitlint rules:\n```\n");
        prompt.push_str(config);
        prompt.push_str("\n```\nFollow them exactly.\n\n");
    } else {
        prompt.push_str(
            "Use Conventional Commits: <type>(<scope>): <short description>.\n\
             Choose the type and scope from the actual intent of the changes.\n\n",
        );
    }

    prompt.push_str("Changed files (complete inventory):\n");
    for file in &changes.files {
        prompt.push_str("- ");
        prompt.push_str(file);
        prompt.push('\n');
    }

    append_section(
        &mut prompt,
        "STAGED CHANGES (HEAD → INDEX)",
        &changes.staged,
    );
    append_section(
        &mut prompt,
        "UNSTAGED CHANGES (INDEX → WORKTREE)",
        &changes.unstaged,
    );
    append_section(&mut prompt, "UNTRACKED FILES", &changes.untracked);

    prompt.push_str(
        "\nWrite one commit message that accurately summarizes the whole change set. \
         Reply with only the commit message, without quotes or code fences. \
         Keep the subject at 72 characters or fewer. For non-trivial changes, \
         add a blank line followed by a concise multiline body.\n",
    );

    if prompt.len() > MAX_PROMPT_BYTES {
        let excerpt = truncate_utf8(&prompt, MAX_PROMPT_BYTES);
        format!("{excerpt}\n… [diff content truncated; file inventory above is complete]\n")
    } else {
        prompt
    }
}

fn append_section(prompt: &mut String, heading: &str, content: &str) {
    prompt.push_str("\n\n## ");
    prompt.push_str(heading);
    prompt.push_str("\n```diff\n");
    if content.trim().is_empty() {
        prompt.push_str("(none)");
    } else {
        prompt.push_str(content);
    }
    prompt.push_str("\n```\n");
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn clean_model_message(value: &str) -> String {
    let trimmed = value.trim();
    let without_fence = trimmed
        .strip_prefix("```text")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    without_fence.trim_matches(['\"', '\'']).trim().to_string()
}

/// Conservative local fallback used when cloud generation is unavailable.
/// It uses the complete changed-file inventory, chooses a Conventional Commit
/// category when the repository shape makes that unambiguous, and emits a
/// multiline body for broad changes so the multiline composer remains useful.
fn local_commit_message(changes: &RepositoryChanges) -> String {
    let normalized = changes
        .files
        .iter()
        .map(|path| path.replace('\\', "/").to_ascii_lowercase())
        .collect::<Vec<_>>();
    let all_match = |predicate: fn(&str) -> bool| normalized.iter().all(|path| predicate(path));

    let (kind, description) = if all_match(is_dependency_file) {
        ("chore(deps)", "update dependencies")
    } else if all_match(is_documentation_file) {
        ("docs", "update documentation")
    } else if all_match(is_test_file) {
        ("test", "update tests")
    } else if all_match(is_ci_file) {
        ("ci", "update workflows")
    } else if normalized.len() == 1 {
        let file_name = std::path::Path::new(&changes.files[0])
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project files");
        return format!("chore: update {file_name}");
    } else {
        ("chore", "update project files")
    };

    let subject = format!("{kind}: {description}");
    if normalized.len() <= 3 {
        subject
    } else {
        format!(
            "{subject}\n\nUpdate {} changed files across the repository.",
            normalized.len()
        )
    }
}

fn is_dependency_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or(path),
        "cargo.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "poetry.lock"
            | "uv.lock"
            | "go.sum"
    )
}

fn is_documentation_file(path: &str) -> bool {
    path.starts_with("docs/")
        || path.contains("/docs/")
        || matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("md" | "mdx" | "rst" | "adoc")
        )
}

fn is_test_file(path: &str) -> bool {
    path.contains("/tests/")
        || path.starts_with("tests/")
        || path.contains("/__tests__/")
        || path.contains(".test.")
        || path.contains(".spec.")
}

fn is_ci_file(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        || path.starts_with(".gitlab/ci/")
        || path == ".gitlab-ci.yml"
}

fn detect_commitlint_config(cwd: &Path) -> Option<String> {
    const CONFIG_FILES: &[&str] = &[
        ".commitlintrc",
        ".commitlintrc.json",
        ".commitlintrc.yaml",
        ".commitlintrc.yml",
        ".commitlintrc.js",
        ".commitlintrc.cjs",
        "commitlint.config.js",
        "commitlint.config.cjs",
        "commitlint.config.mjs",
        "commitlint.config.ts",
    ];
    for name in CONFIG_FILES {
        if let Ok(content) = std::fs::read_to_string(cwd.join(name)) {
            return Some(format!("# {name}\n{content}"));
        }
    }
    if let Ok(package) = std::fs::read_to_string(cwd.join("package.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&package) {
            if let Some(config) = value.get("commitlint") {
                return serde_json::to_string_pretty(config).ok();
            }
        }
    }
    None
}

fn call_agent_step(api: &Arc<dyn AgentApi>, model: &str, prompt: &str) -> Result<String, String> {
    use crate::agent::api::types::{AgentMessage, AgentStepRequest};
    let request = AgentStepRequest {
        model_name: model.to_string(),
        system: "You write precise Git commit messages from repository diffs. Follow repository rules and output only the commit message.".to_string(),
        messages: vec![AgentMessage::user(prompt)],
        tools: vec![],
        refresh_token: None,
        access_token: None,
    };
    api.agent_step(request)
        .map(|response| response.content)
        .map_err(|error| format!("Could not generate a commit message: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_distinguishes_all_git_layers() {
        let changes = RepositoryChanges {
            files: vec!["src/main.rs".to_string(), "README.md".to_string()],
            staged: "+staged".to_string(),
            unstaged: "+unstaged".to_string(),
            untracked: "new file".to_string(),
        };
        let prompt = build_prompt(&changes, None);
        assert!(prompt.contains("HEAD → INDEX"));
        assert!(prompt.contains("INDEX → WORKTREE"));
        assert!(prompt.contains("UNTRACKED FILES"));
        assert!(prompt.contains("src/main.rs"));
    }

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        assert_eq!(truncate_utf8("ab😀cd", 5), "ab");
        assert_eq!(truncate_utf8("ab😀cd", 6), "ab😀");
    }

    #[test]
    fn strips_common_model_wrappers() {
        assert_eq!(
            clean_model_message("```text\nfix: repair sync\n```"),
            "fix: repair sync"
        );
        assert_eq!(
            clean_model_message("\"feat: add history\""),
            "feat: add history"
        );
    }

    #[test]
    fn local_fallback_uses_specific_conventional_categories() {
        let docs = RepositoryChanges {
            files: vec!["docs/setup.md".to_string(), "README.md".to_string()],
            ..Default::default()
        };
        assert_eq!(local_commit_message(&docs), "docs: update documentation");

        let dependencies = RepositoryChanges {
            files: vec!["Cargo.lock".to_string(), "ui/package-lock.json".to_string()],
            ..Default::default()
        };
        assert_eq!(
            local_commit_message(&dependencies),
            "chore(deps): update dependencies"
        );
    }

    #[test]
    fn local_fallback_uses_multiline_body_for_broad_changes() {
        let changes = RepositoryChanges {
            files: vec![
                "app/a.rs".to_string(),
                "app/b.rs".to_string(),
                "crates/c.rs".to_string(),
                "README.md".to_string(),
            ],
            ..Default::default()
        };
        let message = local_commit_message(&changes);
        assert!(message.starts_with("chore: update project files\n\n"));
        assert!(message.contains("Update 4 changed files"));
    }
}
