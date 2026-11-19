//! Closing a shell for good: end what it started along with it.
//!
//! Terminating the shell alone leaves its children running: an agent, a dev
//! server, a build. Each keeps the folder it was started in as its working
//! directory, and Windows will not delete or rename a folder in that state,
//! so a closed workspace's checkout could not be removed. An explicit close
//! therefore terminates every process descended from the shell, as the Unix
//! side does (see `unix::process_tree`).
//!
//! Only processes whose chain of living parents leads to the shell are
//! terminated. Windows reuses process ids and never clears a parent id when
//! the parent exits, so a process counts as a child only if it started no
//! earlier than the process now bearing its parent's id.

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    pid: u32,
    parent: u32,
}

/// A process to terminate: its id and when it started, which together name
/// one process even after the id is reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    pid: u32,
    started: u64,
}

/// Every running process. An unreadable table yields none, and the close
/// then ends the shell alone.
fn snapshot() -> Vec<Row> {
    let mut rows = Vec::new();
    unsafe {
        let table = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if table == INVALID_HANDLE_VALUE {
            return rows;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(table, &mut entry) != 0;
        while more {
            rows.push(Row {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
            });
            more = Process32NextW(table, &mut entry) != 0;
        }
        CloseHandle(table);
    }
    rows
}

/// When `pid` started, as a FILETIME count. `None` once it has exited or
/// when this user may not query it.
fn started(pid: u32) -> Option<u64> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let started = creation_time(process);
        CloseHandle(process);
        started
    }
}

unsafe fn creation_time(process: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    let mut times: [FILETIME; 4] = std::mem::zeroed();
    let [created, exited, kernel, user] = &mut times;
    (GetProcessTimes(process, created, exited, kernel, user) != 0)
        .then(|| (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
}

/// The processes in `table` descended from `root`, never this process.
fn descendants(root: u32, table: &[Row], started: impl Fn(u32) -> Option<u64>) -> Vec<Member> {
    let me = std::process::id();
    let Some(root_started) = started(root) else {
        return Vec::new();
    };
    let mut tree = HashMap::from([(root, root_started)]);
    let mut found = Vec::new();
    loop {
        let before = found.len();
        for row in table {
            if row.pid == 0 || row.pid == me || tree.contains_key(&row.pid) {
                continue;
            }
            let Some(&parent_started) = tree.get(&row.parent) else {
                continue;
            };
            // Started before its "parent": that id belonged to another
            // process when this one was created.
            let Some(child_started) = started(row.pid).filter(|at| *at >= parent_started) else {
                continue;
            };
            tree.insert(row.pid, child_started);
            found.push(Member {
                pid: row.pid,
                started: child_started,
            });
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Everything `shell` has started that is still running. Call while the
/// shell is alive: its children are found through it.
pub fn capture(shell: u32) -> Vec<Member> {
    descendants(shell, &snapshot(), started)
}

/// Terminate the captured processes that are still the ones captured.
pub fn terminate(members: &[Member]) {
    for member in members {
        unsafe {
            let process = OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                member.pid,
            );
            if process.is_null() {
                continue;
            }
            if creation_time(process) == Some(member.started) {
                TerminateProcess(process, 1);
            }
            CloseHandle(process);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_whole_tree_and_nothing_beside_it() {
        let table = [
            Row { pid: 10, parent: 1 },
            Row { pid: 11, parent: 10 },
            Row { pid: 12, parent: 11 },
            Row { pid: 20, parent: 1 },
            Row { pid: 21, parent: 20 },
        ];
        let found = descendants(10, &table, |pid| Some(u64::from(pid)));
        let pids: Vec<u32> = found.iter().map(|member| member.pid).collect();
        assert_eq!(pids, [11, 12]);
    }

    #[test]
    fn a_reused_parent_id_adopts_no_older_process() {
        // 30 names 10 as its parent but started before it: an earlier
        // process had that id.
        let table = [Row { pid: 10, parent: 1 }, Row { pid: 30, parent: 10 }];
        let started = |pid| Some(if pid == 30 { 5 } else { 100 });
        assert!(descendants(10, &table, started).is_empty());
    }

    #[test]
    fn an_exited_shell_has_no_tree() {
        let table = [Row { pid: 11, parent: 10 }];
        assert!(descendants(10, &table, |pid| (pid != 10).then_some(1)).is_empty());
    }

    #[test]
    fn terminates_what_a_shell_started() {
        let mut shell = std::process::Command::new("cmd.exe")
            .args(["/c", "ping -n 60 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut tree = capture(shell.id());
        while tree.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            tree = capture(shell.id());
        }
        assert!(!tree.is_empty(), "ping never started");
        let _ = shell.kill();
        terminate(&tree);
        let _ = shell.wait();
        let alive = |table: &[Row]| tree.iter().any(|m| table.iter().any(|row| row.pid == m.pid));
        while alive(&snapshot()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!alive(&snapshot()));
    }
}
