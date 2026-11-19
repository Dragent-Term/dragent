//! Phase-1 host lifecycle tests: real `dragent-host` binary processes.
//!
//! Each test uses an isolated endpoint file. Tests serialize on a global
//! lock because they re-point one shared env var.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dragent_host::{Event, HostClient};

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

struct Isolated {
    _guard: std::sync::MutexGuard<'static, ()>,
    endpoint: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap();
        let endpoint = std::env::temp_dir().join(format!(
            "dragent-host-test-{}-{}.json",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&endpoint);
        // Lock file shares the endpoint stem.
        let lock = endpoint.with_extension("json.lock");
        let _ = std::fs::remove_file(&lock);
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe {
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &endpoint);
        }
        Self {
            _guard: guard,
            endpoint,
        }
    }

    fn ensure(&self) -> HostClient {
        HostClient::ensure_host_with(&host_bin()).expect("host should come up")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        // Best-effort shutdown so hosts never leak between runs.
        if let Ok(client) = HostClient::connect_existing() {
            let _ = client.shutdown();
        }
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::fs::remove_file(&self.endpoint);
        unsafe {
            std::env::remove_var(dragent_host::ENDPOINT_FILE_ENV);
        }
    }
}

#[test]
fn host_starts_and_serves_identity_snapshot_and_heartbeat() {
    let iso = Isolated::new("basic");
    let client = iso.ensure();
    assert!(client.ping());
    let info = client.host_info().expect("info");
    assert!(info.pid > 0);
    assert!(!info.host_id.is_empty());
    assert_eq!(info.version, dragent_host::PROTOCOL_VERSION);
    let state = client.runtime_state().expect("state");
    assert_eq!(state.workspaces, 0);
    assert_eq!(state.agents, 0);
    assert_eq!(state.terminals, 0);
    client.heartbeat().expect("heartbeat");
}

#[test]
fn host_survives_ui_disconnect() {
    let iso = Isolated::new("survive");
    let host_id = iso.ensure().host_info().expect("info").host_id;
    // "UI" goes away: drop every client handle, wait past any grace period.
    std::thread::sleep(Duration::from_millis(500));
    let again = HostClient::connect_existing().expect("reconnect");
    assert!(again.ping());
    assert_eq!(again.host_info().expect("info").host_id, host_id);
}

#[test]
fn ui_reconnects_to_existing_host() {
    let iso = Isolated::new("reconnect");
    let first = iso.ensure();
    let first_id = first.host_info().expect("info").host_id;
    drop(first);
    // A fresh UI process would do exactly this: never spawn, only connect.
    let second = HostClient::connect_existing().expect("reconnect");
    assert_eq!(second.host_info().expect("info").host_id, first_id);
    // And ensure() reuses rather than replaces.
    let third = iso.ensure();
    assert_eq!(third.host_info().expect("info").host_id, first_id);
}

#[test]
fn concurrent_ensure_creates_single_authority() {
    let _iso = Isolated::new("single");
    let bin = host_bin();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let bin = bin.clone();
            std::thread::spawn(move || {
                HostClient::ensure_host_with(&bin)
                    .expect("ensure")
                    .host_info()
                    .expect("info")
                    .host_id
            })
        })
        .collect();
    let mut ids = handles
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 1, "exactly one host authority: {ids:?}");
}

#[test]
fn second_service_process_cannot_replace_live_authority() {
    let iso = Isolated::new("service-lock");
    let incumbent = iso.ensure();
    let first = incumbent.host_info().expect("first info");
    let mut competing = Command::new(host_bin())
        .arg(dragent_host::SERVICE_ARG)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch competing service");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = competing.try_wait().expect("poll competing service") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = competing.kill();
            let _ = competing.wait();
            panic!("competing service remained alive");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success(), "competing service should reject startup");
    let current = HostClient::connect_existing().expect("incumbent still available");
    assert_eq!(
        current.host_info().expect("current info").host_id,
        first.host_id
    );
}

#[test]
fn subscribe_receives_host_ready() {
    let iso = Isolated::new("subscribe");
    let client = iso.ensure();
    let (tx, rx) = std::sync::mpsc::channel();
    let _handle = client
        .subscribe(move |event| {
            let _ = tx.send(event);
        })
        .expect("subscribe");
    // HostReady is sent first on every new subscription.
    let event = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("host ready event");
    assert!(matches!(event, Event::HostReady { .. }));
    drop(iso);
}

#[test]
fn shutdown_stops_host() {
    let iso = Isolated::new("shutdown");
    let client = iso.ensure();
    assert!(client.ping());
    client.shutdown().expect("shutdown");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        HostClient::connect_existing().is_err(),
        "dead host must not be connectable"
    );
}
