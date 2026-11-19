//! Conservative service process identity and liveness probes.
//!
//! Unix daemon updates preserve the old PTY owner when session handoff fails.
//! Windows updates shut down sessions and may stop a verified daemon process
//! if an acknowledged shutdown stalls. Legacy host migration stops only the
//! host while its separate PTYs survive.

use std::time::{Duration, Instant};

/// Grace between SIGTERM and SIGKILL.
const TERMINATE_GRACE: Duration = Duration::from_secs(3);

/// Is `pid` a live process whose command line carries `service_arg` (our
/// host/daemon flag)? Guards against signalling a recycled pid.
pub fn is_service_process(pid: u32, service_arg: &str) -> bool {
    #[cfg(unix)]
    {
        if pid <= 1 || pid == std::process::id() {
            return false;
        }
        std::process::Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .is_some_and(|out| String::from_utf8_lossy(&out.stdout).contains(service_arg))
    }
    #[cfg(windows)]
    {
        if pid <= 1 || pid == std::process::id() {
            return false;
        }
        // Query command line as the current user before opening a kill handle.
        // If CIM is unavailable, defer rather than guessing from an image name.
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}').CommandLine"),
            ])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .is_some_and(|out| String::from_utf8_lossy(&out.stdout).contains(service_arg))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, service_arg);
        false
    }
}

/// Exists and is not a zombie (a killed child its parent never reaped
/// still answers `kill -0`, but it no longer runs or holds anything).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    !std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim_start()
                .starts_with('Z')
        })
}

/// Stop an outdated service process: SIGTERM, then SIGKILL after a grace
/// period. Returns whether it is gone.
pub fn terminate_outdated(pid: u32, service_arg: &str) -> bool {
    if !is_service_process(pid, service_arg) {
        return false;
    }
    #[cfg(unix)]
    {
        log::warn!("stopping outdated service process pid={pid} ({service_arg})");
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + TERMINATE_GRACE;
        while alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if alive(pid) {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        !alive(pid)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let stopped =
            TerminateProcess(handle, 0) != 0 && WaitForSingleObject(handle, 3000) == WAIT_OBJECT_0;
        CloseHandle(handle);
        stopped
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn only_matching_service_processes_are_recognised() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        assert!(!is_service_process(pid, "--dragent-host-service"));
        assert!(is_service_process(pid, "sleep"));
        // Never ourselves, never init.
        assert!(!is_service_process(std::process::id(), "test"));
        assert!(!is_service_process(1, "launchd"));
        assert!(!terminate_outdated(pid, "--dragent-host-service"));
        assert!(alive(pid), "a non-matching process must be left alone");
        // A real outdated service is not our child (init reaps it); reap
        // this one ourselves so it doesn't linger as a zombie.
        let reaper = std::thread::spawn(move || child.wait());
        assert!(terminate_outdated(pid, "sleep"));
        let _ = reaper.join();
    }
}

/// Conservative liveness probe: permission failures mean possibly alive.
/// A dead manifest must not prevent starting a service after a reboot.
pub fn process_may_be_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 259;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok == 0 || code == 259
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

/// Reject recycled PIDs before treating a persisted service as still alive.
pub fn service_may_be_alive(pid: u32, started_at_ms: u64, service_arg: &str) -> bool {
    if !process_may_be_alive(pid) {
        return false;
    }
    #[cfg(unix)]
    {
        let _ = started_at_ms;
        is_service_process(pid, service_arg)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let _ = service_arg;
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return true;
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(handle);
        let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        let created_ms = (ticks / 10_000).saturating_sub(11_644_473_600_000);
        ok == 0 || created_ms <= started_at_ms
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (started_at_ms, service_arg);
        true
    }
}
