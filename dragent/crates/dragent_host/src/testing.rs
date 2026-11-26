//! For tests that start real host/terminal services: stop them when the test
//! ends, pass or fail.
//!
//! [`ServiceGuard::new`] marks every service this process starts from then on
//! with the test's process id, so the services close by themselves once the
//! test process is gone — even when it was killed and no destructor ran.
//! Dropping the guard (also during a panic's unwinding) stops the services
//! named by the test's records at once: a polite shutdown first, then a kill
//! of the verified service process, waiting until it is gone.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::protocol::{DaemonRequest, PTY_SERVICE_ARG, Request, SERVICE_ARG, ServiceEndpoint};

/// How long a polite shutdown may take before the process is killed.
const POLITE_WAIT: Duration = Duration::from_secs(3);

pub struct ServiceGuard {
    endpoint: PathBuf,
    org_dir: PathBuf,
    stopped: bool,
}

impl ServiceGuard {
    /// Call after pointing this process at its private storage (the host
    /// endpoint override / `DRAGENT_HOME`) and before starting any service.
    pub fn new() -> Self {
        mark_test_services();
        let endpoint = crate::discovery::endpoint_path();
        let org_dir = endpoint
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            endpoint,
            org_dir,
            stopped: false,
        }
    }

    /// The services' data folder this guard watches.
    pub fn org_dir(&self) -> &Path {
        &self.org_dir
    }

    /// Stop every service the test's records name and wait until they are
    /// gone. Safe to call more than once; dropping the guard calls it.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        stop_services(&self.endpoint, &self.org_dir);
    }
}

impl Default for ServiceGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Make every service this process starts from now on close once this
/// process is gone (killed included). [`ServiceGuard::new`] does this; call
/// it directly when a test manages several data folders itself.
pub fn mark_test_services() {
    // SAFETY: test harnesses set their process environment up front, before
    // starting the services that read it.
    unsafe {
        std::env::set_var(
            crate::service_lifecycle::EXIT_WITH_PID_ENV,
            std::process::id().to_string(),
        );
        // A test's agents never wait because the machine it runs on happens
        // to be short of memory.
        if std::env::var_os(crate::agent_sleep::TEST_PRESSURE_ENV).is_none() {
            std::env::set_var(crate::agent_sleep::TEST_PRESSURE_ENV, "normal");
        }
    }
}

/// Stop the host named by `endpoint` and the terminal service recorded in
/// `org_dir`. Only processes verified to be Dragent services are ever
/// signalled. Returns the pids still alive afterwards (none, normally).
pub fn stop_services(endpoint: &Path, org_dir: &Path) -> Vec<u32> {
    let mut left = Vec::new();
    let host = std::fs::read_to_string(endpoint)
        .ok()
        .and_then(|text| serde_json::from_str::<ServiceEndpoint>(&text).ok());
    // Read before the host's shutdown removes the terminal service's record.
    let pty = crate::pty_supervisor::read_manifest(org_dir);
    if let Some(host) = host {
        let _ = crate::discovery::rpc_with_timeout(
            &host,
            "test-guard",
            Request::Shutdown,
            Duration::from_secs(2),
        );
        if !wait_or_kill(host.pid, SERVICE_ARG) {
            left.push(host.pid);
        }
    }
    if let Some(pty) = pty {
        let conn = crate::pty_supervisor::DaemonConn::from(&pty);
        let _ = conn.rpc(DaemonRequest::Shutdown);
        if !wait_or_kill(pty.pid, PTY_SERVICE_ARG) {
            left.push(pty.pid);
        }
    }
    left
}

/// Wait for a service to exit; kill it (verified by its service flag) if it
/// does not. True when it is gone.
fn wait_or_kill(pid: u32, service_arg: &str) -> bool {
    let deadline = Instant::now() + POLITE_WAIT;
    while Instant::now() < deadline {
        if !crate::stale::is_service_process(pid, service_arg) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::stale::terminate_outdated(pid, service_arg)
        || !crate::stale::is_service_process(pid, service_arg)
}
