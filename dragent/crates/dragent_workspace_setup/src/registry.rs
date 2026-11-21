//! Ownership records for launched processes and init outcomes.
//!
//! Launched processes are recorded with pid, start time, profile and
//! worktree in a state directory (the app passes
//! `organisation_dir().join("workspace-setup")`). Stopping signals ONLY a
//! recorded pid whose current start time still matches the recorded one:
//! pid reuse can never kill something else. Nothing is ever killed by port
//! or executable name.
//!
//! Reads never create files: `setup check` and `setup status` leave no
//! state behind. Only recording an init or a launch writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Outcome of the last `init` for one worktree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitRecord {
    pub worktree: String,
    pub at_ms: i64,
    pub ok: bool,
    pub steps_run: usize,
    pub steps_skipped: usize,
    pub failed_step: Option<usize>,
    pub failed_description: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_tail: String,
    pub missing_env: Vec<String>,
    pub copied: Vec<String>,
}

/// One running (or recently stopped) launched profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchRecord {
    pub profile: String,
    pub worktree: String,
    pub pid: u32,
    pub start_time: u64,
    pub log_path: String,
    pub started_at_ms: i64,
    pub command: String,
    pub ready: Option<ReadyState>,
}

/// Serializable form of a readiness probe.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadyState {
    pub ready: bool,
    pub detail: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn init_file(state_dir: &Path) -> PathBuf {
    state_dir.join("init.json")
}

fn launches_file(state_dir: &Path) -> PathBuf {
    state_dir.join("launches.json")
}

fn read_init_map(state_dir: &Path) -> BTreeMap<String, InitRecord> {
    std::fs::read_to_string(init_file(state_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn read_launches(state_dir: &Path) -> Vec<LaunchRecord> {
    std::fs::read_to_string(launches_file(state_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    // Write atomically: temp file then rename, so a concurrent `status`
    // never reads half a record.
    let tmp = path.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    )
    .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(())
}

/// Record the init outcome for `worktree`.
pub fn record_init(state_dir: &Path, record: &InitRecord) -> Result<(), String> {
    let mut map = read_init_map(state_dir);
    map.insert(record.worktree.clone(), record.clone());
    write_json(
        &init_file(state_dir),
        &serde_json::to_value(&map).unwrap_or_default(),
    )
}

/// The last init outcome for `worktree`, if any. Never creates files.
pub fn load_init(state_dir: &Path, worktree: &Path) -> Option<InitRecord> {
    let map = read_init_map(state_dir);
    let key = worktree.display().to_string();
    map.get(&key).cloned().or_else(|| {
        // Match canonicalized forms too (macOS /var -> /private/var).
        let canonical = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf());
        map.get(&canonical.display().to_string()).cloned()
    })
}

/// Record (or replace) a launch for `worktree` + `profile`.
pub fn record_launch(state_dir: &Path, record: &LaunchRecord) -> Result<(), String> {
    let mut launches = read_launches(state_dir);
    launches
        .retain(|known| !(known.worktree == record.worktree && known.profile == record.profile));
    launches.push(record.clone());
    write_json(
        &launches_file(state_dir),
        &serde_json::to_value(&launches).unwrap_or_default(),
    )
}

/// Forget a launch record without signalling anything.
pub fn remove_launch(state_dir: &Path, worktree: &str, profile: &str) -> Result<(), String> {
    let mut launches = read_launches(state_dir);
    launches.retain(|known| !(known.worktree == worktree && known.profile == profile));
    write_json(
        &launches_file(state_dir),
        &serde_json::to_value(&launches).unwrap_or_default(),
    )
}

/// Launches for `worktree` (or all when `None`), newest first.
/// Never creates files.
pub fn list_launches(state_dir: &Path, worktree: Option<&Path>) -> Vec<LaunchRecord> {
    let mut launches = read_launches(state_dir);
    if let Some(worktree) = worktree {
        let key = worktree.display().to_string();
        let canonical = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf())
            .display()
            .to_string();
        launches.retain(|record| record.worktree == key || record.worktree == canonical);
    }
    launches.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));
    launches
}

/// The OS start time of `pid` (seconds since the epoch), if it exists.
pub fn process_start_time(pid: u32) -> Option<u64> {
    let mut system = sysinfo::System::new();
    let id = sysinfo::Pid::from(pid as usize);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[id]), true);
    // A zombie has exited and only waits for its parent to reap it: it is
    // gone, though Linux still lists it with its old start time.
    system
        .process(id)
        .filter(|process| process.status() != sysinfo::ProcessStatus::Zombie)
        .map(|process| process.start_time())
}

/// Whether the recorded process is still the same one (start time matches).
pub fn is_same_process(record: &LaunchRecord) -> bool {
    process_start_time(record.pid).is_some_and(|start| start == record.start_time)
}

/// Whether the recorded process still runs (and is still the same one).
pub fn is_running(record: &LaunchRecord) -> bool {
    is_same_process(record)
}

/// Stop a recorded launch: SIGTERM its process group, then SIGKILL after a
/// short grace. Refuses when the pid is gone or its start time changed
/// (possible pid reuse): nothing is signalled in that case.
pub fn stop_recorded(record: &LaunchRecord) -> Result<bool, String> {
    let current = process_start_time(record.pid);
    match current {
        None => Ok(false),
        Some(start) if start != record.start_time => Err(format!(
            "pid {} is no longer the recorded {} launch (start time changed); nothing was signalled",
            record.pid, record.profile
        )),
        Some(_) => {
            signal_group(record.pid, false);
            // Give it a moment to exit before escalating.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match process_start_time(record.pid) {
                    None => return Ok(true),
                    Some(start) if start != record.start_time => return Ok(true),
                    _ => {}
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            match process_start_time(record.pid) {
                None => Ok(true),
                Some(start) if start != record.start_time => Ok(true),
                Some(_) => {
                    signal_group(record.pid, true);
                    std::thread::sleep(Duration::from_millis(300));
                    Ok(process_start_time(record.pid).is_none_or(|start| start != record.start_time))
                }
            }
        }
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, force: bool) {
    unsafe {
        // Negative pid = the group created for the launch.
        libc::killpg(
            pid as i32,
            if force { libc::SIGKILL } else { libc::SIGTERM },
        );
    }
}

#[cfg(windows)]
fn signal_group(pid: u32, force: bool) {
    let mut command = std::process::Command::new("taskkill");
    command.arg("/PID").arg(pid.to_string()).arg("/T");
    if force {
        command.arg("/F");
    }
    use std::process::Stdio;
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn signal_group(_pid: u32, _force: bool) {}

/// Where a profile's log goes. Unique per launch so concurrent launches of
/// the same profile never share a file.
pub fn log_path_for(state_dir: &Path, worktree: &Path, profile: &str) -> PathBuf {
    let name = worktree
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "worktree".into());
    let safe_profile: String = profile
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe_name: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    state_dir.join(format!("logs/{safe_name}-{safe_profile}-{}.log", now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn spawn_sleep(secs: &str) -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        let mut command = std::process::Command::new("sleep");
        command.arg(secs);
        command.process_group(0);
        command.spawn().expect("sleep")
    }

    #[test]
    fn reads_are_empty_without_writes() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("workspace-setup");
        // Pure reads: nothing is created as a side effect.
        assert!(load_init(&state, Path::new("/repo")).is_none());
        assert!(list_launches(&state, None).is_empty());
        assert!(!state.exists());
    }

    #[test]
    #[cfg(unix)]
    fn stop_never_signals_an_unrecorded_or_reused_pid() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let mut child = spawn_sleep("30");
        let pid = child.id();
        let start = process_start_time(pid).expect("start time");
        let record = LaunchRecord {
            profile: "dev".into(),
            worktree: "/repo".into(),
            pid,
            start_time: start,
            log_path: String::new(),
            started_at_ms: now_ms(),
            command: "sleep 30".into(),
            ready: None,
        };
        // A forged record for a pid we never recorded with a wrong time is
        // refused without signalling: use our own pid with a bad time.
        let forged = LaunchRecord {
            start_time: start.wrapping_add(1),
            ..record.clone()
        };
        assert!(stop_recorded(&forged).is_err());
        // The real child is untouched.
        assert!(child.try_wait().unwrap().is_none());
        // A genuinely recorded stop works.
        record_launch(state, &record).unwrap();
        assert!(stop_recorded(&record).unwrap());
        // Reaped or gone.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = child.kill();
        let _ = child.wait();
        // Stopping again after exit reports nothing to stop.
        assert!(!stop_recorded(&record).unwrap());
    }

    #[test]
    fn init_round_trips_per_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("ws");
        let record = InitRecord {
            worktree: "/repo".into(),
            at_ms: 1,
            ok: true,
            steps_run: 2,
            steps_skipped: 1,
            failed_step: None,
            failed_description: None,
            exit_code: None,
            timed_out: false,
            output_tail: String::new(),
            missing_env: Vec::new(),
            copied: vec![".env".into()],
        };
        record_init(&state, &record).unwrap();
        let loaded = load_init(&state, Path::new("/repo")).unwrap();
        assert!(loaded.ok);
        assert_eq!(loaded.copied, vec![".env".to_string()]);
    }
}
