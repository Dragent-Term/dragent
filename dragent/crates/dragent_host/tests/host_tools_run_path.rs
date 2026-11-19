//! Running a tool from an app opened by the Finder or a desktop launcher, whose
//! PATH holds only system folders. Each case re-runs this binary, because the
//! captured PATH lasts for the whole process.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const TOOL: &str = "dragent-run-path-tool";
const CASE: &str = "DRAGENT_RUN_PATH_CASE";
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn scratch(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dragent-run-path-{}-{case}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// This binary, in the same environment an app from the Finder would have.
fn run_case(case: &str, home: &Path, path: &str, shell: &Path) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture"])
        .env_clear()
        .env(CASE, case)
        .env("HOME", home)
        .env("PATH", path)
        .env("SHELL", shell)
        .env("DRAGENT_HOME", home.join("dragent"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{case} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// Not a test on its own: the half of each case that runs in the child.
#[test]
fn child() {
    let Ok(case) = std::env::var(CASE) else { return };
    let output = dragent_host::host_tools::command(TOOL).output();
    match case.as_str() {
        "missing" => assert_eq!(output.unwrap_err().kind(), std::io::ErrorKind::NotFound),
        _ => {
            let output = output.expect("the tool starts");
            println!("TOOL-PATH={}", String::from_utf8_lossy(&output.stdout).trim());
        }
    }
}

fn tool_path(stdout: &str) -> &str {
    stdout
        .lines()
        .find_map(|line| line.split("TOOL-PATH=").nth(1))
        .unwrap_or_else(|| panic!("the tool didn't run:\n{stdout}"))
}

/// A shell reporting `path`, leaving `seen` behind so a case can tell it ran.
fn shell_reporting(dir: &Path, path: &str, seen: &Path) -> PathBuf {
    let shell = dir.join("shell");
    script(
        &shell,
        &format!(
            "touch '{}'\nprintf '\\n__DRAGENT_PATH__%s__DRAGENT_PATH__\\n' '{path}'",
            seen.display()
        ),
    );
    shell
}

#[test]
fn a_tool_only_the_shell_knows_runs_and_its_children_get_that_path() {
    let dir = scratch("shell");
    let tools = dir.join("tools");
    script(&tools.join(TOOL), "printf '%s' \"$PATH\"");
    let shell = shell_reporting(&dir, &tools.display().to_string(), &dir.join("seen"));

    let stdout = run_case("found", &dir, SYSTEM_PATH, &shell);

    let path = tool_path(&stdout);
    assert!(path.split(':').any(|entry| entry == tools.to_str().unwrap()), "{path}");
    assert!(path.split(':').any(|entry| entry == "/usr/bin"), "{path}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failing_shell_still_leaves_the_usual_folders() {
    let dir = scratch("failing");
    script(&dir.join(".local/bin").join(TOOL), "printf '%s' \"$PATH\"");
    let shell = dir.join("shell");
    script(&shell, "exit 1");

    let stdout = run_case("found", &dir, SYSTEM_PATH, &shell);

    assert!(tool_path(&stdout).contains(".local/bin"), "{stdout}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_tool_already_on_the_path_starts_no_shell_and_keeps_that_path() {
    let dir = scratch("reachable");
    let tools = dir.join("tools");
    script(&tools.join(TOOL), "printf '%s' \"$PATH\"");
    let seen = dir.join("seen");
    let shell = shell_reporting(&dir, "/somewhere/else", &seen);
    let path = format!("{}:{SYSTEM_PATH}", tools.display());

    let stdout = run_case("found", &dir, &path, &shell);

    assert_eq!(tool_path(&stdout), path);
    assert!(!seen.exists(), "the login shell must not start for a reachable tool");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_tool_nowhere_still_reports_not_found() {
    let dir = scratch("missing");
    let shell = shell_reporting(&dir, "/somewhere/else", &dir.join("seen"));

    run_case("missing", &dir, SYSTEM_PATH, &shell);
    let _ = std::fs::remove_dir_all(dir);
}
