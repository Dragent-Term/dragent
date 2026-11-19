//! Every git command the app runs goes through the host service.
//!
//! [`GitCommand`] mirrors the part of `std::process::Command` the app's git
//! code uses (`arg`, `args`, `current_dir`, `env`, `env_remove`, `stdin`/
//! `stdout`/`stderr`, `output`, `status`), so a call site only changes its
//! constructor. The command runs in the host process (`RunGit`), which owns
//! repository work the same way it owns terminals; results come back as an
//! ordinary [`std::process::Output`].
//!
//! Unit tests never start a host (the test binary would be spawned as one),
//! so under `cfg(test)` git runs locally. If the host cannot be reached at
//! all, git also runs locally and the fallback is logged, so git features
//! keep working while the host restarts.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};

/// A git invocation, run by the host.
#[derive(Debug, Default, Clone)]
pub struct GitCommand {
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
    env_remove: Vec<String>,
}

impl GitCommand {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_string_lossy().into_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.env.push((
            key.as_ref().to_string_lossy().into_owned(),
            value.as_ref().to_string_lossy().into_owned(),
        ));
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.env_remove
            .push(key.as_ref().to_string_lossy().into_owned());
        self
    }

    /// Accepted for drop-in compatibility: output is always captured.
    pub fn stdin(&mut self, _cfg: Stdio) -> &mut Self {
        self
    }

    /// Accepted for drop-in compatibility: output is always captured.
    pub fn stdout(&mut self, _cfg: Stdio) -> &mut Self {
        self
    }

    /// Accepted for drop-in compatibility: output is always captured.
    pub fn stderr(&mut self, _cfg: Stdio) -> &mut Self {
        self
    }

    pub fn output(&self) -> io::Result<Output> {
        self.run(&[])
    }

    /// Run with `input` piped to git's stdin (e.g. a patch for `git apply`).
    pub fn output_with_stdin(&self, input: &[u8]) -> io::Result<Output> {
        self.run(input)
    }

    pub fn status(&self) -> io::Result<ExitStatus> {
        self.run(&[]).map(|output| output.status)
    }

    /// Working directory plus args, with a leading `-C <dir>` folded into the
    /// working directory (so it is not applied twice).
    fn resolved(&self) -> (PathBuf, Vec<String>) {
        let mut args = self.args.clone();
        let mut cwd = self.cwd.clone();
        if args.first().map(String::as_str) == Some("-C") && args.len() >= 2 {
            let dir = PathBuf::from(args.remove(1));
            args.remove(0);
            cwd = Some(match cwd {
                Some(base) if dir.is_relative() => base.join(dir),
                _ => dir,
            });
        }
        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(std::env::temp_dir);
        (cwd, args)
    }

    fn run(&self, stdin: &[u8]) -> io::Result<Output> {
        let (cwd, args) = self.resolved();
        // Remote project folders run git on their own machine.
        if let Some(remote) = crate::remote_fs::resolve(&cwd) {
            return self.run_remote(&remote, &args, stdin);
        }
        #[cfg(not(test))]
        {
            match dragent_host::HostClient::ensure_host() {
                Ok(client) => {
                    let (code, stdout, stderr) = client
                        .run_git_with(
                            &cwd.to_string_lossy(),
                            &args,
                            stdin,
                            &self.env,
                            &self.env_remove,
                        )
                        .map_err(io::Error::other)?;
                    return Ok(Output {
                        status: exit_status(code),
                        stdout,
                        stderr,
                    });
                }
                Err(error) => {
                    log::warn!("[git] host unavailable ({error}); running git locally");
                }
            }
        }
        self.run_locally(&cwd, &args, stdin)
    }

    fn run_remote(
        &self,
        remote: &crate::remote_fs::RemotePath,
        args: &[String],
        stdin: &[u8],
    ) -> io::Result<Output> {
        let client = crate::remote_fs::client(&remote.machine_id).map_err(io::Error::other)?;
        let args: Vec<String> = args
            .iter()
            .map(|arg| crate::remote_fs::translate_arg(&remote.machine_id, arg))
            .collect();
        let (code, stdout, stderr) = client
            .run_git_with(&remote.path, &args, stdin, &self.env, &self.env_remove)
            .map_err(io::Error::other)?;
        Ok(Output {
            status: exit_status(code),
            stdout: alias_git_paths(&remote.machine_id, &args, stdout),
            stderr,
        })
    }

    fn run_locally(&self, cwd: &Path, args: &[String], stdin: &[u8]) -> io::Result<Output> {
        use crate::process_ext::NoWindow;
        let mut command = std::process::Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .stdin(if stdin.is_empty() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .no_window();
        if let Some(ceilings) = dragent_host::git_ceiling_env() {
            command.env("GIT_CEILING_DIRECTORIES", ceilings);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        for key in &self.env_remove {
            command.env_remove(key);
        }
        let mut child = command.spawn()?;
        let writer = child.stdin.take().map(|mut pipe| {
            let input = stdin.to_vec();
            std::thread::spawn(move || {
                let _ = io::Write::write_all(&mut pipe, &input);
            })
        });
        let output = child.wait_with_output();
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        output
    }
}

impl crate::process_ext::NoWindow for GitCommand {
    /// The host (or the local fallback) already suppresses console windows.
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

/// An `ExitStatus` carrying `code` (`-1`: ended without an exit code).
fn exit_status(code: i32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if code < 0 {
            // Terminated by a signal: no exit code, not a success.
            ExitStatus::from_raw(libc::SIGKILL)
        } else {
            ExitStatus::from_raw((code & 0xff) << 8)
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = code;
        unreachable!("git runs only on desktop platforms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_dash_c_becomes_the_working_directory() {
        let mut git = GitCommand::new();
        git.arg("-C").arg("/repo").args(["status", "--short"]);
        let (cwd, args) = git.resolved();
        assert_eq!(cwd, PathBuf::from("/repo"));
        assert_eq!(args, vec!["status", "--short"]);

        let mut git = GitCommand::new();
        git.current_dir("/base").arg("-C").arg("sub").arg("log");
        let (cwd, args) = git.resolved();
        assert_eq!(cwd, PathBuf::from("/base/sub"));
        assert_eq!(args, vec!["log"]);

        // `-C` later in the command is left to git.
        let mut git = GitCommand::new();
        git.current_dir("/r").args(["log", "-C"]);
        assert_eq!(
            git.resolved(),
            (
                PathBuf::from("/r"),
                vec!["log".to_string(), "-C".to_string()]
            )
        );
    }

    #[test]
    fn runs_git_with_stdin_and_env() {
        let dir = std::env::temp_dir().join(format!("dragent-gitcmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            GitCommand::new()
                .current_dir(&dir)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        // stdin: hash an object from piped input.
        let out = GitCommand::new()
            .current_dir(&dir)
            .args(["hash-object", "--stdin"])
            .output_with_stdin(b"hello\n")
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        // env reaches git.
        let out = GitCommand::new()
            .current_dir(&dir)
            .env("GIT_AUTHOR_NAME", "Env Probe")
            .args(["var", "GIT_AUTHOR_IDENT"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("Env Probe"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Absolute paths printed by path-reporting git commands come from the
/// remote machine; map them into its stand-in so callers keep one path space.
fn alias_git_paths(machine_id: &str, args: &[String], stdout: Vec<u8>) -> Vec<u8> {
    // Locate the command after Git's global options; never rewrite arbitrary
    // output just because it contains an absolute-looking string.
    let mut i = 0;
    while let Some(arg) = args.get(i).map(String::as_str) {
        if matches!(
            arg,
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
        ) {
            i += 2;
        } else if arg.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    let command = args.get(i).map(String::as_str);
    let options = &args[i.min(args.len())..];
    let rev_paths = command == Some("rev-parse")
        && options.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--show-toplevel"
                    | "--show-prefix"
                    | "--git-dir"
                    | "--absolute-git-dir"
                    | "--git-common-dir"
                    | "--git-path"
            )
        });
    let worktrees =
        command == Some("worktree") && args.get(i + 1).map(String::as_str) == Some("list");
    if !rev_paths && !worktrees {
        return stdout;
    }
    let Ok(text) = std::str::from_utf8(&stdout) else {
        return stdout;
    };
    let porcelain = options.iter().any(|arg| arg == "--porcelain");
    let separator = if options.iter().any(|arg| arg == "-z") {
        '\0'
    } else {
        '\n'
    };
    let mut out = String::with_capacity(text.len() + 64);
    for record in text.split_inclusive(separator) {
        let (body, suffix) = record
            .strip_suffix(separator)
            .map(|s| (s, &record[record.len() - 1..]))
            .unwrap_or((record, ""));
        if worktrees && porcelain {
            if let Some(path) = body.strip_prefix("worktree ") {
                // Git quotes special paths in line-delimited porcelain. Leave
                // quoted records intact; NUL porcelain carries literal paths.
                out.push_str("worktree ");
                out.push_str(&alias_one(machine_id, path));
            } else {
                out.push_str(body);
            }
        } else if worktrees && !porcelain {
            // Human-readable list separates path and commit with >=2 spaces.
            if let Some(split) = body.find("  ") {
                out.push_str(&alias_one(machine_id, &body[..split]));
                out.push_str(&body[split..]);
            } else {
                out.push_str(body);
            }
        } else {
            out.push_str(&alias_one(machine_id, body));
        }
        out.push_str(suffix);
    }
    out.into_bytes()
}

fn alias_one(machine_id: &str, path: &str) -> String {
    if !path.starts_with('/') {
        return path.to_string();
    }
    crate::remote_fs::alias(machine_id, path)
        .to_string_lossy()
        .into_owned()
}
