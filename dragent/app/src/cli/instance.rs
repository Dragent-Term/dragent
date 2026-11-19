//! Whether the Dragent app is running, and starting it from the command line.
//!
//! Every app process holds a shared lock on `<DRAGENT_HOME>/app.lock` for
//! its lifetime; the operating system drops it when the process ends,
//! however it ends. The command line only starts the app when it can take
//! that lock exclusively, so it never starts a second copy (which would sign
//! the first one out).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::sync::OnceLock;

static HELD: OnceLock<File> = OnceLock::new();

fn lock_path() -> PathBuf {
    dragent_paths::home().join("app.lock")
}

fn open_lock() -> std::io::Result<File> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).truncate(false).write(true).open(path)
}

/// App side: mark this process as a running app until it exits.
pub fn hold() {
    match open_lock().and_then(|file| file.lock_shared().map(|()| file)) {
        Ok(file) => {
            let _ = HELD.set(file);
        }
        Err(error) => log::warn!("[cli] couldn't take the app lock: {error}"),
    }
}

/// Whether any Dragent app process is running.
pub fn app_running() -> bool {
    lock_held() || same_binary_app_running()
}

fn lock_held() -> bool {
    let Ok(file) = open_lock() else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => false,
    }
}

/// An app from before the lock existed: a process of this binary that
/// isn't a command or one of its services (those all carry a
/// `--dragent-…` flag). Matched by executable path, since the command line
/// may name it relatively. Windows needs no check: a second app there hands
/// over to the first and exits.
fn same_binary_app_running() -> bool {
    #[cfg(unix)]
    {
        let Some(exe) = std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok()) else {
            return false;
        };
        let own = std::process::id();
        let Ok(output) = std::process::Command::new("ps").args(["-axo", "pid=,command="]).output() else {
            return false;
        };
        String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            let Some((pid, command)) = line.trim().split_once(' ') else {
                return false;
            };
            let Ok(pid) = pid.parse::<u32>() else {
                return false;
            };
            pid != own
                && !command.contains("--dragent-")
                && executable(pid).is_some_and(|path| path == exe)
        })
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The executable file process `pid` runs.
#[cfg(target_os = "macos")]
fn executable(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; 4 * 1024];
    let length = unsafe {
        libc::proc_pidpath(pid as libc::c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32)
    };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    let path = PathBuf::from(String::from_utf8(buffer).ok()?);
    Some(path.canonicalize().unwrap_or(path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn executable(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// Start the app, detached from this terminal.
pub fn launch_app() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    // An installed app opens through the system, like a Dock click.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
    {
        let status = std::process::Command::new("open")
            .arg("-a")
            .arg(bundle)
            .status()
            .map_err(|error| format!("couldn't start Dragent: {error}"))?;
        return if status.success() {
            Ok(())
        } else {
            Err("couldn't start Dragent".into())
        };
    }
    let mut command = std::process::Command::new(&exe);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        // Its own process group: closing this terminal doesn't close the app.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("couldn't start Dragent: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test process runs the test binary: as an app process of "its"
    /// binary it counts, because it isn't a command or a service.
    #[cfg(unix)]
    #[test]
    fn a_process_of_the_same_binary_is_found_by_its_executable() {
        let own = std::process::id();
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(executable(own), Some(exe));
    }

    /// The lock answers "running" while another handle holds it shared,
    /// and "not running" once it's released.
    #[test]
    fn a_shared_holder_reads_as_running() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.lock");
        let holder = OpenOptions::new().create(true).truncate(false).write(true).open(&path).unwrap();
        holder.lock_shared().unwrap();
        let probe = OpenOptions::new().write(true).open(&path).unwrap();
        assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
        holder.unlock().unwrap();
        assert!(probe.try_lock().is_ok());
    }
}
