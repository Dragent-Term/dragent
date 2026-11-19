use std::io::{BufRead, BufReader};
use std::sync::mpsc;

#[cfg(not(target_family = "wasm"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

use super::events::{AgentEventKind, CreateTaskResponse, PipelineEvent};
use crate::api::API_CLIENT;

#[cfg(not(target_family = "wasm"))]
#[derive(Clone)]
pub(crate) struct LocalToolResult {
    pub(crate) call_id: String,
    pub(crate) output: String,
    pub(crate) is_error: bool,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) struct LocalRunControl {
    pub(crate) results: mpsc::Sender<LocalToolResult>,
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) execution_id: String,
    pub(crate) delivered_tool_results: HashSet<String>,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn local_runs() -> &'static Mutex<HashMap<String, LocalRunControl>> {
    static RUNS: OnceLock<Mutex<HashMap<String, LocalRunControl>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Parse a Go-style duration string ("2h56m37s", "45m", "30s") into total seconds.
fn parse_wait_secs(s: &str) -> u64 {
    let mut total = 0u64;
    let mut num = 0u64;
    for c in s.chars() {
        match c {
            '0'..='9' => num = num * 10 + (c as u64 - '0' as u64),
            'h' => {
                total += num * 3600;
                num = 0;
            }
            'm' => {
                total += num * 60;
                num = 0;
            }
            's' => {
                total += num;
                num = 0;
            }
            _ => {}
        }
    }
    total
}

/// Convert a wait-duration string into a human-readable "3:45 PM" retry time label.
/// Falls back to an empty string if the duration cannot be parsed.
pub(crate) fn parse_wait_to_retry_label(wait_str: &str) -> String {
    let secs = parse_wait_secs(wait_str);
    if secs == 0 {
        return String::new();
    }
    let retry_at = chrono::Local::now() + chrono::Duration::seconds(secs as i64);
    // Format as "3:45 PM" — strip the leading zero from single-digit hours.
    retry_at.format("%-I:%M %p").to_string()
}

/// Stream failure modes — single definition in `dragent_agent::pipeline`
/// shared by the engine core, the Host, and the UI.
pub use dragent_agent::pipeline::StreamError;

pub use dragent_agent::pipeline::HostToolDef;

pub(crate) static HOST_TOOL_DEFS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, Vec<HostToolDef>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
