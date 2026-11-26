//! Connected-service calls from SSH agents go through the same local MCP
//! process, auth checks and automation policy as local agents. Credentials
//! never leave the desktop. Every child is bound to the original account.
use dragent_host::HostClient;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

struct Job {
    binding: Arc<Binding>,
    connection: usize,
}
fn jobs() -> &'static Mutex<HashMap<String, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(Mutex::default)
}
pub fn stop(machine: &str) {
    if let Some(job) = jobs()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(machine)
    {
        job.binding.stop.store(true, Ordering::Release);
    }
}
/// The account and organisation whose runtime the connection reached. The
/// remote runtime is chosen per organisation at connect time, so the bridge
/// never serves it under another one.
struct Binding {
    machine: String,
    context: String,
    user: Option<String>,
    stop: Arc<AtomicBool>,
}
impl Binding {
    fn active(&self) -> bool {
        self.matches(
                &dragent_paths::organisation_id(),
                &crate::organisation_context::cached_active().map(|active| active.user_id),
            )
    }
    fn matches(&self, context: &str, user: &Option<String>) -> bool {
        !self.stop.load(Ordering::Acquire) && self.context == context && &self.user == user
    }
}
/// One claimed call: valid only while no switch happened since its claim.
#[derive(Clone)]
struct Scope {
    binding: Arc<Binding>,
    generation: u64,
}
impl Scope {
    fn current(&self) -> bool {
        self.binding.active() && self.same_generation(dragent_paths::organisation_selection())
    }
    fn same_generation(&self, generation: u64) -> bool {
        self.generation == generation
    }
}
/// Serve the machine's connected-service calls for as long as it stays
/// connected. While another organisation is selected or agent tools are off
/// it stops polling, so the remote side reports the desktop offline; it picks
/// up again when the original account returns.
pub fn start(machine: &str, client: HostClient) {
    let binding = Arc::new(Binding {
        machine: machine.into(),
        context: dragent_paths::organisation_id(),
        user: crate::organisation_context::cached_active().map(|active| active.user_id),
        stop: Arc::new(AtomicBool::new(false)),
    });
    let connection = client
        .remote_connection()
        .map(|remote| Arc::as_ptr(remote) as usize)
        .unwrap_or(0);
    {
        let mut jobs = jobs().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(job) = jobs.get(machine) {
            if !job.binding.stop.load(Ordering::Acquire)
                && job.connection == connection
                && job.binding.context == binding.context
                && job.binding.user == binding.user
            {
                return;
            }
            job.binding.stop.store(true, Ordering::Release);
        }
        jobs.insert(
            machine.into(),
            Job {
                binding: binding.clone(),
                connection,
            },
        );
    }
    std::thread::spawn(move || {
        // A new owner after any pause: the host fails calls the previous
        // owner claimed instead of handing them out again.
        let mut owner: Option<(String, u64)> = None;
        let busy = Arc::new(AtomicUsize::new(0));
        let catalog: Vec<_> = super::bridge::catalog()
            .into_iter()
            .filter(|tool| {
                dragent_host::service_mcp::tool(tool["name"].as_str().unwrap_or_default()).is_some()
            })
            .collect();
        let person = serde_json::to_value(crate::cli::team::Caller::person()).unwrap();
        while !binding.stop.load(Ordering::Acquire) {
            if !binding.active() {
                owner = None;
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let generation = dragent_paths::organisation_selection();
            if owner.as_ref().is_none_or(|(_, seen)| *seen != generation) {
                owner = Some((uuid::Uuid::new_v4().to_string(), generation));
            }
            let owner = owner.as_ref().map(|(id, _)| id.clone()).unwrap_or_default();
            let scope = Scope {
                binding: binding.clone(),
                generation,
            };
            let free = 4usize.saturating_sub(busy.load(Ordering::Acquire));
            match client.coordination(
                "host_services_poll",
                json!({"owner":owner,"catalog":catalog,"limit":free}),
                person.clone(),
            ) {
                Ok(value) => {
                    for call in value["calls"].as_array().into_iter().flatten() {
                        let Some(id) = call["id"].as_str().map(str::to_string) else {
                            continue;
                        };
                        let request = call["request"].clone();
                        let scope = scope.clone();
                        let client = client.clone();
                        let busy = busy.clone();
                        let owner = owner.clone();
                        let person = person.clone();
                        busy.fetch_add(1, Ordering::AcqRel);
                        std::thread::spawn(move || {
                            let result = execute(&scope, &client, &request);
                            let (text, error) = match result {
                                Ok(text) => (text, false),
                                Err(error) => (error, true),
                            };
                            // No retry after a claimed operation, including a lost answer.
                            let _ = client.coordination(
                                "host_services_finish",
                                json!({"owner":owner,"id":id,"text":text,"error":error}),
                                person,
                            );
                            busy.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                }
                Err(error) => {
                    log::debug!("[ssh-mcp] desktop service bridge: {error}");
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        binding.stop.store(true, Ordering::Release);
    });
}
fn execute(scope: &Scope, client: &HostClient, request: &Value) -> Result<String, String> {
    if !scope.current() || scope.binding.user.is_none() {
        return Err(
            "Sign in to the original organisation in Dragent to use connected services".into(),
        );
    }
    let name = request["name"].as_str().ok_or("Missing service tool")?;
    if dragent_host::service_mcp::tool(name).is_none() {
        return Err("Unknown service tool".into());
    }
    let mut arguments = request["arguments"].clone();
    if !arguments.is_object() {
        return Err("Service arguments must be an object".into());
    }
    let cwd = request["cwd"]
        .as_str()
        .ok_or("Missing remote working directory")?;
    let requested = arguments["project"]
        .as_str()
        .filter(|path| !path.is_empty())
        .unwrap_or(cwd);
    let native = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        Path::new(cwd).join(requested)
    };
    let alias = crate::remote_fs::checked_alias(&scope.binding.machine, &native.to_string_lossy())
        .ok_or("Invalid remote project path; use its absolute path on that machine")?;
    crate::remote_fs::ensure_stand_in(&alias);
    arguments["project"] = json!(alias);
    let origin = client
        .run_git(&native.to_string_lossy(), &["remote", "get-url", "origin"])
        .ok()
        .filter(|(code, _, _)| *code == 0)
        .and_then(|(_, bytes, _)| origin_hint(&String::from_utf8_lossy(&bytes)));
    if !scope.current() {
        return Err("The active account or organisation changed".into());
    }
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .args([
            format!("--dragent-organisation={}", scope.binding.context),
            crate::cli::CLI_ARG.into(),
            "mcp".into(),
        ])
        .current_dir(&alias)
        .env(
            "DRAGENT_MCP_BOUND_USER",
            scope.binding.user.as_deref().unwrap(),
        )
        .env(
            "DRAGENT_HOST_TERMINAL_ID",
            request["terminal"].as_str().unwrap_or_default(),
        )
        .env_remove("GH_REPO")
        .env_remove("DRAGENT_REMOTE_GIT_ORIGIN")
        .env_remove("DRAGENT_REMOTE_PROJECT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(origin) = origin {
        command
            .env("DRAGENT_REMOTE_PROJECT", &alias)
            .env("DRAGENT_REMOTE_GIT_ORIGIN", &origin);
        if let Ok(url) = url::Url::parse(&origin) {
            if let Some(host) = url.host_str() {
                command.env(
                    "GH_REPO",
                    format!(
                        "{host}/{}",
                        url.path().trim_matches('/').trim_end_matches(".git")
                    ),
                );
            }
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Couldn't start the desktop MCP bridge: {e}"))?;
    if let Some(mut input) = child.stdin.take() {
        let line=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string()+"\n";
        if let Err(error) = input.write_all(line.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
    }
    let output = child.stdout.take().ok_or("No MCP output pipe")?;
    let errors = child.stderr.take().ok_or("No MCP error pipe")?;
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = output.take(1024 * 1024 + 1).read_to_end(&mut bytes);
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = errors.take(64 * 1024).read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let mut cancelled = false;
    loop {
        if !scope.current() || started.elapsed() > Duration::from_secs(50) {
            cancelled = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let bytes = out.join().unwrap_or_default();
    let _ = err.join();
    if cancelled || !scope.current() {
        return Err("The desktop connection/account changed or the service timed out. Check the service before retrying a write.".into());
    }
    if bytes.len() > 1024 * 1024 {
        return Err("Service output exceeded 1 MiB".into());
    }
    let reply: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The desktop MCP server returned an invalid answer")?;
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .ok_or("The desktop MCP server returned no result")?
        .to_string();
    if reply["result"]["isError"].as_bool() == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}
/// Only repository identity crosses into the local process environment.
/// Remove any credential, query or fragment present in Git's remote URL.
fn origin_hint(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let url = if raw.contains("://") {
        url::Url::parse(raw).ok()?
    } else {
        let location = raw
            .rsplit_once('@')
            .map(|(_, location)| location)
            .unwrap_or(raw);
        let (host, path) = location.split_once(':')?;
        url::Url::parse(&format!("https://{host}/{path}")).ok()?
    };
    if !matches!(url.scheme(), "http" | "https" | "ssh" | "git") {
        return None;
    }
    let host = url.host()?;
    // Changing ssh to https in place is disallowed by the URL parser.
    // Reconstruct metadata with no credentials, port, query or fragment.
    let url = url::Url::parse(&format!("https://{host}{}", url.path())).ok()?;
    Some(url.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claimed_calls_cannot_survive_account_switches_or_return_to_the_same_org() {
        let binding = Arc::new(Binding {
            machine: "host".into(),
            context: "org".into(),
            user: Some("user".into()),
            stop: Arc::new(AtomicBool::new(false)),
        });
        let scope = Scope {
            binding: binding.clone(),
            generation: 7,
        };
        assert!(binding.matches("org", &Some("user".into())));
        assert!(!binding.matches("other", &Some("user".into())));
        assert!(!binding.matches("org", &Some("other".into())));
        assert!(!binding.matches("org", &None));
        assert!(scope.same_generation(7));
        assert!(!scope.same_generation(9));
        binding.stop.store(true, Ordering::Release);
        assert!(!binding.matches("org", &Some("user".into())));
    }
    #[test]
    fn origin_metadata_never_copies_a_password_or_token() {
        assert_eq!(
            origin_hint("https://oauth:secret@gitlab.com/team/app.git?token=secret#secret")
                .unwrap(),
            "https://gitlab.com/team/app.git"
        );
        assert_eq!(
            origin_hint("git@github.com:org/repo.git").unwrap(),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            origin_hint("ssh://git:secret@gitlab.com:2222/team/app.git?token=secret").unwrap(),
            "https://gitlab.com/team/app.git"
        );
        assert!(origin_hint("file:///a/local/repository").is_none());
        assert!(origin_hint("/a/local/repository").is_none());
    }
}
