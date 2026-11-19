//! Desktop work in a helper process. A call into an app that stopped
//! answering, or a crash inside a platform API, then costs only the helper:
//! Dragent ends it and starts a fresh one for the next request, and Dragent
//! itself never waits on the desktop. The app turns this on with
//! [`use_helper`]; without it (the helper itself, tests) work stays
//! in-process.
//!
//! One JSON message per line each way over the helper's stdin and stdout.
//! Requests go one at a time, so two desktop actions never interleave.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Permissions, Preview, Request};

/// The flag that starts the app's executable as the helper.
pub const SERVICE_ARG: &str = "--dragent-computer-service";
/// Tells the helper which process it works for, so that app is left out of
/// the apps it can see, as it is in-process.
pub(crate) const OWNER_ENV: &str = "DRAGENT_COMPUTER_OWNER";

/// A helper with nothing to do for this long exits; the next request starts
/// another.
const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// A started helper must answer its greeting within this.
const START_WITHIN: Duration = Duration::from_secs(20);
/// Starts that failed within the last minute; at this many, wait.
const FAILED_STARTS: usize = 3;

/// An action plus the fresh snapshot that follows it.
pub(crate) const RUN_WITHIN: Duration = Duration::from_secs(75);
pub(crate) const PREVIEW_WITHIN: Duration = Duration::from_secs(30);
pub(crate) const CHECK_WITHIN: Duration = Duration::from_secs(20);

const TIMED_OUT: &str = "action_timeout: The app didn't answer within the time limit, so computer use was \
restarted. An action may have been partly done: look at the app and take a new snapshot before going on.";
const STOPPED: &str =
    "accessibility_error: Computer use stopped unexpectedly and restarts for the next \
request. Take a new snapshot before going on.";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Call {
    Hello,
    Preview {
        scope: String,
        request: Request,
    },
    Run {
        scope: String,
        request: Request,
        approved: Option<Preview>,
    },
    Permissions,
}

#[derive(Debug, Serialize, Deserialize)]
struct Envelope<C = Call> {
    id: u64,
    call: C,
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    id: u64,
    #[serde(default)]
    value: Value,
    #[serde(default)]
    error: Option<String>,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
}

impl Running {
    fn end(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Helper {
    command: Vec<OsString>,
    envs: Vec<(String, String)>,
    running: Option<Running>,
    last_used: Instant,
    next_id: u64,
    failed_starts: Vec<Instant>,
}

static HELPER: OnceLock<Mutex<Helper>> = OnceLock::new();

/// Send this process's desktop work to a helper started as `command`
/// (program first): the app's own executable with [`SERVICE_ARG`]. Call once,
/// at startup.
pub fn use_helper(command: Vec<OsString>) {
    if !command.is_empty() {
        let _ = HELPER.set(Mutex::new(Helper::new(command, Vec::new())));
    }
}

pub(crate) fn active() -> bool {
    HELPER.get().is_some()
}

/// End the helper now, if one runs.
pub fn stop_helper() {
    if let Some(helper) = HELPER.get() {
        helper.lock().unwrap_or_else(|p| p.into_inner()).stop();
    }
}

/// Make `call` in the helper. `before_send` runs once nothing else can
/// reach the desktop, right before the request goes out: the app's last
/// check that it is still allowed.
pub(crate) fn request<T: DeserializeOwned>(
    call: Call,
    within: Duration,
    before_send: impl FnOnce() -> Result<(), String>,
) -> Result<T, String> {
    let helper = HELPER.get().ok_or("Computer use has no helper.")?;
    let mut helper = helper.lock().unwrap_or_else(|p| p.into_inner());
    before_send()?;
    let value = helper.send(&call, within)?;
    serde_json::from_value(value)
        .map_err(|e| format!("accessibility_error: Computer use sent an unreadable answer: {e}"))
}

/// Why an exchange failed.
enum Failure {
    /// The helper answered with an error.
    Answered(String),
    TimedOut,
    /// It stopped after taking the request.
    Gone,
    /// It was gone before the request reached it.
    NotSent,
}

impl Helper {
    fn new(command: Vec<OsString>, envs: Vec<(String, String)>) -> Self {
        Self {
            command,
            envs,
            running: None,
            last_used: Instant::now(),
            next_id: 1,
            failed_starts: Vec::new(),
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.end();
        }
    }

    fn start(&mut self) -> Result<(), String> {
        self.failed_starts
            .retain(|at| at.elapsed() < Duration::from_secs(60));
        if self.failed_starts.len() >= FAILED_STARTS {
            return Err("accessibility_error: Computer use couldn't start several times in a row. Try again in a minute.".into());
        }
        let mut running = match spawn(&self.command, &self.envs) {
            Ok(running) => running,
            Err(error) => {
                self.failed_starts.push(Instant::now());
                return Err(format!(
                    "accessibility_error: Computer use couldn't start: {error}"
                ));
            }
        };
        let id = self.take_id();
        if exchange(&mut running, id, &Call::Hello, START_WITHIN).is_ok() {
            self.running = Some(running);
            return Ok(());
        }
        running.end();
        self.failed_starts.push(Instant::now());
        Err("accessibility_error: Computer use didn't start. Try again.".into())
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, call: &Call, within: Duration) -> Result<Value, String> {
        // A helper that exited, or is about to exit for being idle, is
        // replaced before anything is sent, so no request is lost half way.
        let stale = self.last_used.elapsed() + Duration::from_secs(60) >= IDLE_EXIT;
        let exited = self
            .running
            .as_mut()
            .is_some_and(|running| !matches!(running.child.try_wait(), Ok(None)));
        if stale || exited {
            self.stop();
        }
        if self.running.is_none() {
            self.start()?;
        }
        let result = match self.exchange(call, within) {
            // It went away between the check and the write: nothing reached
            // it, so a fresh helper may take the request.
            Err(Failure::NotSent) => {
                self.stop();
                self.start()?;
                self.exchange(call, within)
            }
            other => other,
        };
        self.last_used = Instant::now();
        result.map_err(|failure| match failure {
            Failure::Answered(error) => error,
            failure => {
                // Whatever it was doing can't be trusted to finish; the next
                // request gets a fresh helper.
                self.stop();
                match failure {
                    Failure::TimedOut => TIMED_OUT.into(),
                    _ => STOPPED.into(),
                }
            }
        })
    }

    fn exchange(&mut self, call: &Call, within: Duration) -> Result<Value, Failure> {
        let id = self.take_id();
        let running = self.running.as_mut().ok_or(Failure::NotSent)?;
        exchange(running, id, call, within)
    }
}

fn spawn(command: &[OsString], envs: &[(String, String)]) -> std::io::Result<Running> {
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .envs(envs.iter().map(|(k, v)| (k, v)))
        .env(OWNER_ENV, std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = process.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let (tx, replies) = mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("dragent-computer-replies".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(Running {
        child,
        stdin,
        replies,
    })
}

fn exchange(
    running: &mut Running,
    id: u64,
    call: &Call,
    within: Duration,
) -> Result<Value, Failure> {
    let line = serde_json::to_string(&Envelope { id, call })
        .map_err(|e| Failure::Answered(format!("invalid_argument: {e}")))?;
    if writeln!(running.stdin, "{line}")
        .and_then(|_| running.stdin.flush())
        .is_err()
    {
        return Err(Failure::NotSent);
    }
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match running.replies.recv_timeout(left) {
            // Anything else on its output (a stray log line, a late answer to
            // an earlier request) isn't this answer.
            Ok(line) => match serde_json::from_str::<Reply>(&line) {
                Ok(reply) if reply.id == id => {
                    return match reply.error {
                        Some(error) => Err(Failure::Answered(error)),
                        None => Ok(reply.value),
                    };
                }
                _ => continue,
            },
            Err(RecvTimeoutError::Timeout) => return Err(Failure::TimedOut),
            Err(RecvTimeoutError::Disconnected) => return Err(Failure::Gone),
        }
    }
}

/// The helper's side: answer requests from stdin until it closes (Dragent
/// quit or ended it), or until nothing has come for a while.
pub fn serve() {
    // On macOS the list of running apps only refreshes while the main run
    // loop runs, so requests are read on another thread and the main thread
    // keeps the run loop going.
    #[cfg(target_os = "macos")]
    {
        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
        let reader = std::thread::Builder::new()
            .name("dragent-computer-requests".into())
            .spawn(|| {
                answer_requests();
                std::process::exit(0);
            });
        if reader.is_err() {
            return answer_requests();
        }
        loop {
            // With nothing attached yet the run loop returns at once; wait a
            // little rather than spin.
            let ran = CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                Duration::from_secs(1),
                false,
            );
            if ran == CFRunLoopRunResult::Finished {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    answer_requests();
}

fn answer_requests() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let started = Instant::now();
    let now = move || started.elapsed().as_secs();
    let _ = std::thread::Builder::new()
        .name("dragent-computer-idle".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            if now().saturating_sub(LAST.load(Ordering::Relaxed)) >= IDLE_EXIT.as_secs() {
                std::process::exit(0);
            }
        });
    let mut stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        LAST.store(now(), Ordering::Relaxed);
        let reply = match serde_json::from_str::<Envelope>(&line) {
            Ok(envelope) => answer(envelope),
            Err(error) => Reply {
                id: 0,
                value: Value::Null,
                error: Some(format!("invalid_argument: {error}")),
            },
        };
        let Ok(text) = serde_json::to_string(&reply) else {
            continue;
        };
        if writeln!(stdout, "{text}")
            .and_then(|_| stdout.flush())
            .is_err()
        {
            break;
        }
        LAST.store(now(), Ordering::Relaxed);
    }
}

fn answer(envelope: Envelope) -> Reply {
    fn value<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
        result.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
    }
    let result = match envelope.call {
        Call::Hello => Ok(json!({ "pid": std::process::id() })),
        Call::Preview { scope, request } => value(crate::preview_here(&scope, request)),
        Call::Run {
            scope,
            request,
            approved,
        } => value(crate::run_here(&scope, request, approved, || Ok(()))),
        Call::Permissions => value(Ok::<Permissions, String>(crate::permissions_here())),
    };
    match result {
        Ok(value) => Reply {
            id: envelope.id,
            value,
            error: None,
        },
        Err(error) => Reply {
            id: envelope.id,
            value: Value::Null,
            error: Some(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODE: &str = "DRAGENT_COMPUTER_TEST_HELPER";

    /// Not a test of its own: the other tests start this test binary again
    /// running only this, as their helper, behaving as `MODE` says.
    #[test]
    #[ignore = "started by the helper tests"]
    fn helper_process() {
        let mode = std::env::var(MODE).unwrap_or_default();
        if mode == "serve" {
            return answer_requests();
        }
        let mut stdout = std::io::stdout();
        for line in std::io::stdin().lock().lines() {
            let Ok(envelope) = serde_json::from_str::<Envelope>(&line.unwrap_or_default()) else {
                continue;
            };
            if !matches!(envelope.call, Call::Hello) {
                match mode.as_str() {
                    "hang" => std::thread::sleep(Duration::from_secs(120)),
                    _ => std::process::exit(3),
                }
            }
            let reply = json!({ "id": envelope.id, "value": {} });
            writeln!(stdout, "{reply}").unwrap();
            stdout.flush().unwrap();
        }
    }

    fn helper(mode: &str) -> Helper {
        let command = vec![
            std::env::current_exe().unwrap().into_os_string(),
            "helper::tests::helper_process".into(),
            "--exact".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ];
        Helper::new(command, vec![(MODE.into(), mode.into())])
    }

    fn snapshot() -> Call {
        Call::Run {
            scope: "test".into(),
            request: Request::from_tool(
                "snapshot",
                &json!({ "app": "Test App", "screenshot": false }),
            )
            .unwrap(),
            approved: None,
        }
    }

    #[test]
    fn messages_are_single_lines_that_read_back() {
        let request =
            Request::from_tool("click", &json!({ "app": "Notes", "element": 3 })).unwrap();
        let call = Call::Run {
            scope: "s".into(),
            request: request.clone(),
            approved: None,
        };
        let line = serde_json::to_string(&Envelope { id: 9, call: &call }).unwrap();
        assert!(!line.contains('\n'));
        let back: Envelope = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 9);
        assert!(matches!(back.call, Call::Run { request: r, .. } if r == request));
        let hello = answer(Envelope {
            id: 1,
            call: Call::Hello,
        });
        assert_eq!(hello.id, 1);
        assert_eq!(hello.value["pid"], std::process::id());
    }

    #[test]
    fn requests_are_answered_by_the_helper_process() {
        let mut helper = helper("serve");
        let outcome = helper.send(&snapshot(), RUN_WITHIN).unwrap();
        let text = outcome["text"].as_str().unwrap();
        assert!(text.contains("Editor"), "{text}");
        let permissions: Permissions =
            serde_json::from_value(helper.send(&Call::Permissions, CHECK_WITHIN).unwrap()).unwrap();
        assert_eq!(permissions.platform, "test");
        // The snapshot is kept in the helper, so element numbers still work.
        let click = Call::Run {
            scope: "test".into(),
            request: Request::from_tool("click", &json!({ "app": "Test App", "element": 1 }))
                .unwrap(),
            approved: None,
        };
        helper.send(&click, RUN_WITHIN).unwrap();
        // Errors from the desktop come back as they are.
        let missing = Call::Run {
            scope: "test".into(),
            request: Request::from_tool("snapshot", &json!({ "app": "Nothing Here" })).unwrap(),
            approved: None,
        };
        let error = helper.send(&missing, RUN_WITHIN).unwrap_err();
        assert_eq!(crate::errors::details(&error)["code"], "app_not_found");
        helper.stop();
    }

    #[test]
    fn a_helper_that_went_away_is_replaced_before_sending() {
        let mut helper = helper("serve");
        helper.send(&Call::Permissions, CHECK_WITHIN).unwrap();
        let first = helper.running.as_ref().unwrap().child.id();
        helper.running.as_mut().unwrap().child.kill().unwrap();
        let _ = helper.running.as_mut().unwrap().child.wait();
        helper.send(&Call::Permissions, CHECK_WITHIN).unwrap();
        assert_ne!(helper.running.as_ref().unwrap().child.id(), first);
        helper.stop();
    }

    #[test]
    fn a_hung_helper_is_ended_and_the_next_request_gets_a_fresh_one() {
        let mut helper = helper("hang");
        let error = helper
            .send(&snapshot(), Duration::from_secs(2))
            .unwrap_err();
        assert_eq!(crate::errors::details(&error)["code"], "action_timeout");
        assert!(helper.running.is_none());
        // A fresh helper starts (and hangs again, being this one).
        let error = helper
            .send(&snapshot(), Duration::from_millis(500))
            .unwrap_err();
        assert_eq!(crate::errors::details(&error)["code"], "action_timeout");
    }

    #[test]
    fn a_crash_is_reported_and_the_next_request_gets_a_fresh_one() {
        let mut helper = helper("crash");
        let error = helper.send(&snapshot(), RUN_WITHIN).unwrap_err();
        assert!(error.contains("stopped unexpectedly"), "{error}");
        assert!(helper.running.is_none());
        let error = helper.send(&snapshot(), RUN_WITHIN).unwrap_err();
        assert!(error.contains("stopped unexpectedly"), "{error}");
    }

    #[test]
    fn repeated_failed_starts_wait_before_trying_again() {
        let mut helper = Helper::new(vec!["/no/such/dragent-helper".into()], Vec::new());
        for _ in 0..FAILED_STARTS {
            let error = helper.send(&Call::Hello, CHECK_WITHIN).unwrap_err();
            assert!(error.contains("couldn't start:"), "{error}");
        }
        let error = helper.send(&Call::Hello, CHECK_WITHIN).unwrap_err();
        assert!(error.contains("several times in a row"), "{error}");
    }
}
