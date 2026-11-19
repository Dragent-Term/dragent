use std::path::Path;

/// Determines if the current path is inside a git repository
pub fn is_git_repo(cwd: &Path) -> bool {
    if crate::remote_fs::is_remote(cwd) {
        return crate::git_cmd::GitCommand::new()
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(cwd)
            .output()
            .is_ok_and(|output| output.status.success());
    }
    let mut current = Some(cwd);
    while let Some(path) = current {
        if path.join(".git").exists() {
            return true;
        }
        current = path.parent();
    }
    false
}

/// Determines if the current path is a Rust project
pub fn is_rust_project(cwd: &Path) -> bool {
    cwd.join("Cargo.toml").exists()
}

/// Determines if the current path is a Python project
pub fn is_python_project(cwd: &Path) -> bool {
    cwd.join("requirements.txt").exists()
        || cwd.join("pyproject.toml").exists()
        || cwd.join("Pipfile").exists()
}
