//! Each agent's models as the agent itself reports them, so every model
//! picker (new workspace, coordination, the phone) offers what the person can
//! actually use: their OpenCode providers, their Codex account's models, and
//! so on, not a list fixed at build time.
//!
//! Asking is slow (it starts the agent), so it happens once per run in the
//! background, the first time an agent's models are wanted. Until the answer
//! is in, the last answer saved on disk is used, and before any answer the
//! bundled catalog. An agent that can't be asked keeps the catalog.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// (id, label) pairs, as the pickers use them.
pub(crate) type Models = Vec<(String, String)>;

#[derive(Default)]
struct State {
    /// Agent key → its models, from this run or the saved file.
    models: HashMap<String, Models>,
    /// Agents asked (or being asked) this run.
    asked: HashSet<String>,
    loaded: bool,
}

fn shared() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn cache_file() -> PathBuf {
    dragent_paths::home().join("agent_models.json")
}

/// The agent's own model list, when known. The first call in a run starts
/// asking the agent in the background.
pub(crate) fn models(key: &str) -> Option<Models> {
    if !can_ask(key) || cfg!(test) {
        return None;
    }
    let mut state = shared().lock().ok()?;
    if !state.loaded {
        state.loaded = true;
        if let Some(saved) = std::fs::read(cache_file()).ok().and_then(|bytes| serde_json::from_slice::<HashMap<String, Models>>(&bytes).ok()) {
            state.models = saved;
        }
    }
    if state.asked.insert(key.to_string()) {
        let key = key.to_string();
        let _ = std::thread::Builder::new().name(format!("{key}-models")).spawn(move || {
            let Some(found) = ask(&key).filter(|models| !models.is_empty()) else {
                return;
            };
            let Ok(mut state) = shared().lock() else { return };
            state.models.insert(key, found);
            if let Ok(bytes) = serde_json::to_vec_pretty(&state.models) {
                let _ = std::fs::write(cache_file(), bytes);
            }
        });
    }
    state.models.get(key).cloned()
}

/// Agents that can list their models: by a `models` command, or by opening
/// a chat session (no message sent) and reading the model picker it reports.
/// Copilot and Qwen report no models that way (and Copilot records an empty
/// session), so they keep the catalog.
fn can_ask(key: &str) -> bool {
    matches!(key, "opencode" | "mimo" | "cursor-agent" | "claude" | "codex" | "gemini")
}

fn ask(key: &str) -> Option<Models> {
    match key {
        "opencode" | "mimo" => parse_provider_models(&run(key, &["models"])?),
        "cursor-agent" => parse_cursor_models(&run(key, &["models"])?),
        _ => ask_session(key),
    }
}

/// Run the agent's own command (as found on the person's PATH) and return
/// what it printed, when it succeeded.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let mut command = dragent_host::host_tools::command(program);
    command.args(args).stdin(std::process::Stdio::null()).current_dir(scratch_dir());
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Open a chat session in a scratch folder without sending anything, and
/// read the model picker the agent offers.
fn ask_session(key: &str) -> Option<Models> {
    let (_, program, _) = dragent_acp::driver_for(key)?;
    let command = dragent_host::host_tools::command(program);
    let path = PathBuf::from(command.get_program());
    let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    let env: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
        .collect();
    let dir = scratch_dir();
    let driver = dragent_acp::start(key, &path, &args, &dir, &env).ok()?;
    let picker = driver.open(&dir, None).ok().and_then(|session| session.options.model);
    driver.shutdown();
    // Every picker already offers the agent's default; don't list it twice.
    Some(
        picker?
            .choices
            .into_iter()
            .filter(|choice| choice.id != "default")
            .map(|choice| (choice.id, choice.name))
            .collect(),
    )
}

/// A folder of our own for these runs, so nothing lands in a project.
fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("dragent-agent-models");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `opencode models` / `mimo models`: one `provider/model` per line, mimo
/// adding ` — window …` details after the id.
fn parse_provider_models(text: &str) -> Option<Models> {
    let models: Models = text
        .lines()
        .filter_map(|line| {
            let id = line.split(" — ").next()?.trim();
            (id.contains('/') && !id.contains(' ')).then(|| (id.to_string(), id.to_string()))
        })
        .collect();
    (!models.is_empty()).then_some(models)
}

/// `cursor-agent models`: `id - Label` lines under a heading, the current
/// one marked `(current, default)`.
fn parse_cursor_models(text: &str) -> Option<Models> {
    let models: Models = text
        .lines()
        .filter_map(|line| {
            let (id, label) = line.split_once(" - ")?;
            let id = id.trim();
            if id.is_empty() || id.contains(' ') {
                return None;
            }
            let label = label.split(" (").next().unwrap_or(label).trim();
            Some((id.to_string(), label.to_string()))
        })
        .collect();
    (!models.is_empty()).then_some(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_lists_read_ids_and_drop_details() {
        let opencode = "openai/gpt-5.5\nopenrouter/anthropic/claude-sonnet-4.5\n\n";
        assert_eq!(
            parse_provider_models(opencode).unwrap(),
            vec![
                ("openai/gpt-5.5".into(), "openai/gpt-5.5".into()),
                ("openrouter/anthropic/claude-sonnet-4.5".into(), "openrouter/anthropic/claude-sonnet-4.5".into()),
            ]
        );
        let mimo = "amazon-bedrock/amazon.nova-lite-v1:0 — window 300K, compacts at 270K\nnot a model line";
        assert_eq!(parse_provider_models(mimo).unwrap()[0].0, "amazon-bedrock/amazon.nova-lite-v1:0");
        assert_eq!(parse_provider_models(mimo).unwrap().len(), 1);
        assert!(parse_provider_models("Error: not signed in").is_none());
    }

    #[test]
    fn cursor_list_reads_ids_and_labels() {
        let text = "Available models\n\nauto - Auto (current, default)\ngpt-5.3-codex-low - Codex 5.3 Low\n";
        assert_eq!(
            parse_cursor_models(text).unwrap(),
            vec![("auto".into(), "Auto".into()), ("gpt-5.3-codex-low".into(), "Codex 5.3 Low".into())]
        );
    }

    #[test]
    fn only_agents_that_can_list_are_asked() {
        assert!(can_ask("opencode") && can_ask("codex") && can_ask("claude") && can_ask("cursor-agent"));
        assert!(!can_ask("droid") && !can_ask("copilot"));
        // Tests never start agents.
        assert!(models("opencode").is_none());
    }
}

/// Asks the agents installed on this machine; run by hand:
/// `cargo test -p dragent --lib live_models::probe -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn probe() {
    for key in ["opencode", "mimo", "cursor-agent", "claude", "codex", "gemini"] {
        let started = std::time::Instant::now();
        let found = ask(key);
        println!(
            "{key}: {} models in {:?}, first: {:?}",
            found.as_ref().map_or(0, Vec::len),
            started.elapsed(),
            found.as_ref().and_then(|m| m.first())
        );
    }
}
