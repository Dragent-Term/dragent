//! Closing a shell for good: hang up its whole process tree, then kill what
//! ignored the hangup.
//!
//! Closing the PTY master makes the kernel SIGHUP the shell's session, and
//! the shell forwards that to its jobs — but a process that traps HUP, or a
//! child that detached into its own process group/session (`setsid`, `nohup`,
//! agent tool servers), survives it. An explicit close must not leave such
//! processes running, so after a short grace period every survivor of the
//! tree captured at close time is sent SIGKILL, re-checked a few times in
//! case a straggler forked while it was being killed.
//!
//! Only processes that belonged to the captured tree (by pid, process group,
//! or parentage) are ever signalled, never a bare pid from the table, so a
//! recycled pid that now belongs to something unrelated is left alone.

use std::collections::HashSet;
use std::process::Command;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Grace between the hangup and the first SIGKILL round.
const KILL_ESCALATION_GRACE: Duration = Duration::from_millis(1000);
/// Re-check delays after the first SIGKILL round; stops early once nothing
/// from the tree is left.
const KILL_VERIFY_DELAYS: [Duration; 4] = [
    Duration::from_millis(300),
    Duration::from_millis(700),
    Duration::from_millis(1500),
    Duration::from_millis(2500),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcRow {
    pid: i32,
    ppid: i32,
    pgid: i32,
}

/// Every process as `(pid, ppid, pgid)`. `ps` is the portable source on both
/// macOS and Linux; an unreadable table yields an empty snapshot (the kernel
/// hangup still happens, only the escalation has nothing to act on).
fn snapshot() -> Vec<ProcRow> {
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "pgid="])
        .output()
    else {
        return Vec::new();
    };
    parse_table(&String::from_utf8_lossy(&output.stdout))
}

fn parse_table(text: &str) -> Vec<ProcRow> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(str::parse::<i32>);
            match (cols.next(), cols.next(), cols.next()) {
                (Some(Ok(pid)), Some(Ok(ppid)), Some(Ok(pgid))) => {
                    Some(ProcRow { pid, ppid, pgid })
                }
                _ => None,
            }
        })
        .collect()
}

/// What an escalation may signal: known pids and process groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Tree {
    pids: HashSet<i32>,
    pgids: HashSet<i32>,
}

impl Tree {
    /// Grow `self` with every process in `table` that is in a known group or
    /// descends from a known pid (to a fixed point). Never admits init, the
    /// caller itself, or the caller's own group.
    fn absorb(&mut self, table: &[ProcRow]) {
        let me = std::process::id() as i32;
        let my_pgid = unsafe { libc::getpgrp() };
        let admissible = |row: &ProcRow| row.pid > 1 && row.pid != me && row.pgid != my_pgid;
        loop {
            let before = (self.pids.len(), self.pgids.len());
            for row in table.iter().filter(|row| admissible(row)) {
                if self.pids.contains(&row.ppid) || self.pgids.contains(&row.pgid) {
                    self.pids.insert(row.pid);
                    if row.pgid > 1 {
                        self.pgids.insert(row.pgid);
                    }
                }
            }
            if (self.pids.len(), self.pgids.len()) == before {
                break;
            }
        }
    }

    /// Members of the tree still present in `table` (never init or the
    /// calling process).
    fn alive_in(&self, table: &[ProcRow]) -> Vec<ProcRow> {
        let me = std::process::id() as i32;
        table
            .iter()
            .filter(|row| row.pid > 1 && row.pid != me)
            .filter(|row| self.pids.contains(&row.pid) || self.pgids.contains(&row.pgid))
            .copied()
            .collect()
    }
}

fn capture(root: i32, table: &[ProcRow]) -> Tree {
    let my_pgid = unsafe { libc::getpgrp() };
    let mut tree = Tree::default();
    tree.pids.insert(root);
    // The shell leads its own session/group after login_tty. Right after
    // spawn it can still sit in our group; never adopt that group.
    let pgid = table
        .iter()
        .find(|row| row.pid == root)
        .map_or(root, |row| row.pgid);
    if pgid > 1 && pgid != my_pgid {
        tree.pgids.insert(pgid);
    }
    tree.absorb(table);
    tree
}

fn signal_tree(tree: &Tree, alive: &[ProcRow], signal: i32) {
    let me = std::process::id() as i32;
    let my_pgid = unsafe { libc::getpgrp() };
    let groups: HashSet<i32> = alive
        .iter()
        .map(|row| row.pgid)
        .filter(|pgid| *pgid > 1 && *pgid != my_pgid && tree.pgids.contains(pgid))
        .collect();
    for pgid in groups {
        unsafe {
            libc::kill(-pgid, signal);
        }
    }
    for row in alive {
        if row.pid > 1 && row.pid != me {
            unsafe {
                libc::kill(row.pid, signal);
            }
        }
    }
}

/// Kill chains still running. A daemon exiting mid-chain would drop the
/// SIGKILL escalation, so shutdown waits on this first
/// ([`wait_for_pending_kills`]).
static PENDING: Mutex<usize> = Mutex::new(0);
static PENDING_DONE: Condvar = Condvar::new();

/// Hang up the tree rooted at the shell `root` now, then escalate to SIGKILL
/// for survivors on a background thread.
pub fn hangup_tree(root: i32) {
    if root <= 1 {
        return;
    }
    let table = snapshot();
    let mut tree = capture(root, &table);
    let alive = tree.alive_in(&table);
    if alive.is_empty() {
        // Table unavailable: at least hang up the shell's own group.
        unsafe {
            libc::kill(-root, libc::SIGHUP);
        }
    } else {
        signal_tree(&tree, &alive, libc::SIGHUP);
    }

    *PENDING.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    std::thread::spawn(move || {
        std::thread::sleep(KILL_ESCALATION_GRACE);
        let mut delays = KILL_VERIFY_DELAYS.iter();
        loop {
            let table = snapshot();
            // Pick up anything forked since the capture.
            tree.absorb(&table);
            let alive = tree.alive_in(&table);
            if alive.is_empty() {
                break;
            }
            signal_tree(&tree, &alive, libc::SIGKILL);
            match delays.next() {
                Some(delay) => std::thread::sleep(*delay),
                None => break,
            }
        }
        let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
        *pending = pending.saturating_sub(1);
        PENDING_DONE.notify_all();
    });
}

/// Block until every escalation chain has finished, or `timeout` passes.
/// Returns whether all chains completed.
pub fn wait_for_pending_kills(timeout: Duration) -> bool {
    let pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    let (pending, _) = PENDING_DONE
        .wait_timeout_while(pending, timeout, |n| *n > 0)
        .unwrap_or_else(|p| p.into_inner());
    *pending == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: i32, ppid: i32, pgid: i32) -> ProcRow {
        ProcRow { pid, ppid, pgid }
    }

    #[test]
    fn parses_ps_output() {
        let rows = parse_table("  100   1 100\n 101 100  100\ngarbage\n 200 101 200\n");
        assert_eq!(
            rows,
            vec![row(100, 1, 100), row(101, 100, 100), row(200, 101, 200)]
        );
    }

    #[test]
    fn capture_follows_groups_and_detached_descendants() {
        // Shell 100 leads group 100; job 101 in it; 200 detached into its
        // own group but is still a child of 101; 300 unrelated.
        let table = vec![
            row(100, 1, 100),
            row(101, 100, 100),
            row(200, 101, 200),
            row(201, 200, 200),
            row(300, 1, 300),
        ];
        let tree = capture(100, &table);
        let alive: HashSet<i32> = tree.alive_in(&table).iter().map(|r| r.pid).collect();
        assert_eq!(alive, HashSet::from([100, 101, 200, 201]));
    }

    #[test]
    fn orphaned_survivor_stays_in_the_tree_after_its_parent_dies() {
        let table = vec![row(100, 1, 100), row(200, 100, 200)];
        let mut tree = capture(100, &table);
        // Shell gone, the detached child reparented to init.
        let later = vec![row(200, 1, 200), row(300, 1, 300)];
        tree.absorb(&later);
        let alive: Vec<i32> = tree.alive_in(&later).iter().map(|r| r.pid).collect();
        assert_eq!(alive, vec![200]);
    }

    #[test]
    fn never_adopts_this_process_group() {
        let my_pgid = unsafe { libc::getpgrp() };
        // A just-spawned shell still in our group; a sibling shares it.
        let table = vec![row(100, 1, my_pgid), row(555, 1, my_pgid)];
        let tree = capture(100, &table);
        assert!(!tree.pgids.contains(&my_pgid));
        let alive: Vec<i32> = tree.alive_in(&table).iter().map(|r| r.pid).collect();
        assert_eq!(alive, vec![100]);
    }

    #[test]
    fn never_targets_init_or_this_process() {
        let me = std::process::id() as i32;
        let table = vec![row(100, 1, 100), row(me, 100, 100), row(1, 0, 1)];
        let tree = capture(100, &table);
        assert!(!tree.pids.contains(&me));
        assert!(!tree.pids.contains(&1));
    }
}
