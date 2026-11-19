//! A bounded, ephemeral queue for desktop-owned service calls. SSH already
//! authenticates the desktop/host link. No provider credentials cross it.
//! Claimed operations are never replayed after a disconnect.
use crate::team_commands::Caller;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
const OFFLINE: &str = "Connect the Dragent desktop to this SSH machine to use connected services. Remote coordination remains available.";
const LEASE: Duration = Duration::from_secs(6);
const WAIT: Duration = Duration::from_secs(55);
#[derive(Default)]
struct State {
    owner: Option<String>,
    seen: Option<Instant>,
    catalog: Vec<Value>,
    calls: HashMap<String, Call>,
}
struct Call {
    value: Value,
    claimed: bool,
    answer: Option<Result<String, String>>,
}
#[derive(Default)]
struct Relay {
    state: Mutex<State>,
    changed: Condvar,
}
impl Relay {
    fn poll(&self, owner: &str, catalog: Vec<Value>, limit: usize) -> Value {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.owner.as_deref() != Some(owner) {
            for call in state.calls.values_mut() {
                call.answer=Some(Err("The desktop connection changed. Check the service before retrying a claimed operation.".into()));
            }
        }
        state.owner = Some(owner.into());
        state.seen = Some(Instant::now());
        state.catalog = catalog;
        let calls: Vec<_> = state
            .calls
            .iter_mut()
            .filter(|(_, call)| !call.claimed && call.answer.is_none())
            .take(limit)
            .map(|(id, call)| {
                call.claimed = true;
                json!({"id":id,"request":call.value})
            })
            .collect();
        self.changed.notify_all();
        json!({"calls":calls})
    }
    fn finish(&self, owner: &str, id: &str, answer: Result<String, String>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.owner.as_deref() != Some(owner) {
            return Err("A different desktop owns this call".into());
        }
        let call = state
            .calls
            .get_mut(id)
            .ok_or("The service call expired; it must not be replayed")?;
        if !call.claimed || call.answer.is_some() {
            return Err("The service call already completed or wasn't claimed".into());
        }
        call.answer = Some(answer);
        self.changed.notify_all();
        Ok(())
    }
    fn call(&self, value: Value, timeout: Duration) -> Result<String, String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.seen.is_none_or(|seen| seen.elapsed() > LEASE) {
            return Err(OFFLINE.into());
        }
        if state.calls.len() >= 32 {
            return Err("Too many pending service calls; try again after they finish".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.calls.insert(
            id.clone(),
            Call {
                value,
                claimed: false,
                answer: None,
            },
        );
        self.changed.notify_all();
        let started = Instant::now();
        loop {
            if let Some(answer) = state.calls.get_mut(&id).and_then(|call| call.answer.take()) {
                state.calls.remove(&id);
                return answer;
            }
            if started.elapsed() > timeout || state.seen.is_none_or(|seen| seen.elapsed() > LEASE) {
                let claimed = state.calls.remove(&id).is_some_and(|call| call.claimed);
                return Err(if claimed {
                    "The desktop stopped answering after claiming this operation. Check the service before retrying.".into()
                } else {
                    OFFLINE.into()
                });
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(200))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
}
fn relay() -> &'static Relay {
    static RELAY: OnceLock<Relay> = OnceLock::new();
    RELAY.get_or_init(Relay::default)
}
/// Intercepted inside the host process, before the normal coordination engine.
pub fn dispatch(tool: &str, args: &Value, caller: &Caller) -> Option<Result<Value, String>> {
    match tool {
        "host_services_catalog" => {
            let state = relay().state.lock().unwrap_or_else(|p| p.into_inner());
            Some(Ok(json!(if state.catalog.is_empty() {
                crate::service_mcp::catalog(&[])
            } else {
                state.catalog.clone()
            })))
        }
        "host_services_poll" => Some((|| {
            if caller.agent {
                return Err("Only the desktop can serve connected-service tools".into());
            }
            let owner = args["owner"]
                .as_str()
                .filter(|owner| owner.len() <= 128)
                .ok_or("Missing desktop owner")?;
            let catalog = args["catalog"]
                .as_array()
                .ok_or("Missing tool catalog")?
                .iter()
                .filter(|tool| {
                    crate::service_mcp::tool(tool["name"].as_str().unwrap_or_default()).is_some()
                })
                .cloned()
                .collect();
            Ok(relay().poll(
                owner,
                catalog,
                args["limit"].as_u64().unwrap_or(4).min(4) as usize,
            ))
        })()),
        "host_services_finish" => Some((|| {
            if caller.agent {
                return Err("Only the desktop can answer service calls".into());
            }
            let owner = args["owner"].as_str().ok_or("Missing desktop owner")?;
            let id = args["id"].as_str().ok_or("Missing call ID")?;
            let text = args["text"].as_str().ok_or("Missing service answer")?;
            if text.len() > 1024 * 1024 {
                return Err("Service answer exceeds 1 MiB".into());
            }
            relay().finish(
                owner,
                id,
                if args["error"].as_bool() == Some(true) {
                    Err(text.into())
                } else {
                    Ok(text.into())
                },
            )?;
            Ok(json!({"done":true}))
        })()),
        "host_services_call" => Some((|| {
            let name = args["name"].as_str().ok_or("Missing tool name")?;
            if crate::service_mcp::tool(name).is_none() {
                return Err("Unknown connected-service tool".into());
            }
            if args.to_string().len() > 256 * 1024 {
                return Err("Service arguments exceed 256 KiB".into());
            }
            if !crate::remote_mcp::enabled() {
                return Err("Agent tools are disabled in Dragent settings".into());
            }
            let client = crate::HostClient::connect_existing()?;
            let (pane, workspace, terminal, cwd) = if let Some(pane) = caller.pane.as_deref() {
                let record = pane
                    .parse()
                    .ok()
                    .and_then(crate::agent_sessions::load)
                    .filter(|record| record.active)
                    .ok_or("This agent session is no longer active")?;
                (
                    Some(pane.to_string()),
                    record.workspace_id,
                    record.terminal_id,
                    record.cwd.to_string_lossy().into_owned(),
                )
            } else if !caller.agent {
                let terminal = caller
                    .terminal
                    .as_deref()
                    .ok_or("Run this command in its Dragent SSH terminal")?;
                let mut found = None;
                for workspace in client.list_workspaces()? {
                    if let Ok(session) = client.terminal(&workspace, terminal) {
                        found = Some((workspace, session));
                        break;
                    }
                }
                let (workspace, session) =
                    found.ok_or("This terminal no longer belongs to the remote host")?;
                (None, workspace, session.id, session.cwd)
            } else {
                return Err("Run this MCP server inside a Dragent agent session".into());
            };
            if !client
                .terminal(&workspace, &terminal)
                .is_ok_and(|terminal| terminal.state == crate::protocol::TerminalState::Running)
            {
                return Err("This terminal is no longer running".into());
            }
            let text=relay().call(json!({"name":name,"arguments":args["arguments"],"cwd":cwd,"terminal":terminal,"pane":pane}),WAIT)?;
            Ok(json!({"text":text}))
        })()),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn requests_are_claimed_once_and_answers_are_bound_to_the_desktop_owner() {
        let relay = Arc::new(Relay::default());
        assert!(
            relay
                .call(json!({}), Duration::ZERO)
                .unwrap_err()
                .contains("Connect")
        );
        relay.poll("a", Vec::new(), 4);
        let worker = relay.clone();
        let call =
            std::thread::spawn(move || worker.call(json!({"name":"read"}), Duration::from_secs(2)));
        let started = Instant::now();
        let claimed = loop {
            let rows = relay.poll("a", Vec::new(), 4);
            if !rows["calls"].as_array().unwrap().is_empty() {
                break rows;
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        };
        let id = claimed["calls"][0]["id"].as_str().unwrap();
        assert!(
            relay.poll("a", Vec::new(), 4)["calls"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(relay.finish("b", id, Ok("wrong account".into())).is_err());
        relay.finish("a", id, Ok("result".into())).unwrap();
        assert_eq!(call.join().unwrap().unwrap(), "result");
        assert!(relay.finish("a", id, Ok("replay".into())).is_err());
    }
    #[test]
    fn changing_connections_cancels_claimed_calls_without_replaying_them() {
        let relay = Arc::new(Relay::default());
        relay.poll("old", Vec::new(), 4);
        let worker = relay.clone();
        let call = std::thread::spawn(move || {
            worker.call(json!({"name":"comment"}), Duration::from_secs(2))
        });
        let started = Instant::now();
        loop {
            if !relay.poll("old", Vec::new(), 4)["calls"]
                .as_array()
                .unwrap()
                .is_empty()
            {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert!(
            relay.poll("new", Vec::new(), 4)["calls"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            call.join()
                .unwrap()
                .unwrap_err()
                .contains("connection changed")
        );
    }
}
