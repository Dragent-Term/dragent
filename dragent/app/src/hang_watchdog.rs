//! Main-thread hang reports. The main thread stamps a heartbeat every
//! second; a watcher thread notices when the stamp stops while Dragent is
//! the active app (App Nap slows a background app's timers, which isn't a
//! hang) and records what every thread was doing, once per stall, in
//! `<DRAGENT_HOME>/logs/hang-<time>.txt`, with the recent log beside it.
//! Nothing leaves the machine.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

const BEAT: Duration = Duration::from_secs(1);
const HANG_AFTER: Duration = Duration::from_secs(5);
/// Reports kept; older ones are deleted when a new one is written.
const KEEP_REPORTS: usize = 10;

/// Milliseconds since `start()`, monotonic and paused while the computer
/// sleeps, so waking up never reads as a hang.
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static APP_ACTIVE: AtomicBool = AtomicBool::new(false);

fn uptime_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub struct HangWatchdog;

impl Entity for HangWatchdog {
    type Event = ();
}

impl SingletonEntity for HangWatchdog {}

impl HangWatchdog {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::beat(ctx);
        std::thread::Builder::new()
            .name("dragent-hang-watchdog".into())
            .spawn(watch)
            .ok();
        Self
    }

    fn beat(ctx: &mut ModelContext<Self>) {
        APP_ACTIVE.store(ctx.windows().app_is_active(), Ordering::Relaxed);
        LAST_BEAT_MS.store(uptime_ms(), Ordering::Relaxed);
        ctx.spawn(Timer::after(BEAT), |_, _, ctx| Self::beat(ctx));
    }
}

/// Whether a heartbeat this old, last seen with the app in front, is a hang.
fn is_hang(age: Duration, app_active: bool) -> bool {
    app_active && age >= HANG_AFTER
}

fn watch() {
    let mut reported = false;
    loop {
        std::thread::sleep(BEAT);
        let age = Duration::from_millis(uptime_ms().saturating_sub(LAST_BEAT_MS.load(Ordering::Relaxed)));
        if age < HANG_AFTER {
            reported = false;
            continue;
        }
        if reported || !is_hang(age, APP_ACTIVE.load(Ordering::Relaxed)) {
            continue;
        }
        reported = true;
        log::error!("[hang] main thread unresponsive for {} ms", age.as_millis());
        record(&dragent_paths::home().join("logs"));
    }
}

fn record(dir: &Path) {
    if let Err(error) = std::fs::create_dir_all(dir) {
        log::error!("[hang] creating {}: {error}", dir.display());
        return;
    }
    prune(dir, KEEP_REPORTS.saturating_sub(1));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let report = dir.join(format!("hang-{stamp}.txt"));
    let _ = std::fs::write(dir.join(format!("hang-{stamp}.log")), crate::diagnostics::recent_log());
    #[cfg(target_os = "macos")]
    {
        // `sample` reads this process's threads for 3 seconds; it needs no
        // help from the stuck main thread.
        let sampled = std::process::Command::new("/usr/bin/sample")
            .arg(std::process::id().to_string())
            .arg("3")
            .arg("-file")
            .arg(&report)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match sampled {
            Ok(status) if status.success() => log::error!("[hang] report saved to {}", report.display()),
            Ok(status) => log::error!("[hang] sample exited with {status}"),
            Err(error) => log::error!("[hang] couldn't run sample: {error}"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    log::error!("[hang] recent log saved beside {}", report.display());
}

/// Keep the newest `keep` hang reports (a report is its `.txt` and `.log`).
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut stamps: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter_map(|name| name.strip_prefix("hang-")?.split('.').next().map(str::to_string))
        .collect();
    stamps.sort();
    stamps.dedup();
    let excess = stamps.len().saturating_sub(keep);
    for stamp in &stamps[..excess] {
        for extension in ["txt", "log"] {
            let _ = std::fs::remove_file(dir.join(format!("hang-{stamp}.{extension}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_stall_in_front_is_a_hang() {
        assert!(is_hang(Duration::from_secs(6), true));
        assert!(!is_hang(Duration::from_secs(6), false), "App Nap throttles background timers");
        assert!(!is_hang(Duration::from_secs(2), true));
    }

    #[test]
    fn pruning_keeps_the_newest_reports() {
        let dir = tempfile::tempdir().unwrap();
        for stamp in ["20261022-130000", "20261022-140000", "20261022-150000"] {
            std::fs::write(dir.path().join(format!("hang-{stamp}.txt")), "").unwrap();
            std::fs::write(dir.path().join(format!("hang-{stamp}.log")), "").unwrap();
        }
        std::fs::write(dir.path().join("other.txt"), "").unwrap();
        prune(dir.path(), 2);
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "hang-20261022-140000.log",
                "hang-20261022-140000.txt",
                "hang-20261022-150000.log",
                "hang-20261022-150000.txt",
                "other.txt",
            ]
        );
    }
}
