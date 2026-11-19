//! Local, provider-ready agent checkpoints for desktop-owned execution.
//!
//! These files may contain user prompts, tool arguments, and tool output. They
//! therefore live only in the user's Dragent data directory and are removed on
//! completion/cancellation or pruned after seven days. API keys are never included.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 2;
const STALE_CHECKPOINT_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedToolResult {
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalRunCheckpoint {
    pub schema_version: u32,
    pub run_id: String,
    /// Unique identity for this execution of a stable run id. A late result
    /// from an older turn must never be allowed to mutate its successor.
    #[serde(default)]
    pub execution_id: String,
    pub request: serde_json::Value,
    pub round: u32,
    pub total_input_tokens: i64,
    pub total_cached_input_tokens: i64,
    pub total_output_tokens: i64,
    pub context_tokens: i64,
    pub phase: String,
    #[serde(default)]
    pub read_cache: HashMap<String, CachedToolResult>,
    #[serde(default)]
    pub read_only_streak: u32,
    /// Tool results are committed as they arrive while a round is waiting.
    /// On restart this distinguishes completed calls from calls that were
    /// parked before execution, without replaying an uncertain side effect.
    #[serde(default)]
    pub pending_tool_results: HashMap<String, CachedToolResult>,
}

impl LocalRunCheckpoint {
    pub fn new(run_id: String, request: serde_json::Value) -> Self {
        prune_stale_checkpoints();
        Self {
            schema_version: SCHEMA_VERSION,
            run_id,
            execution_id: uuid::Uuid::new_v4().simple().to_string(),
            request,
            round: 0,
            total_input_tokens: 0,
            total_cached_input_tokens: 0,
            total_output_tokens: 0,
            context_tokens: 0,
            phase: "ready_model".to_string(),
            read_cache: HashMap::new(),
            read_only_streak: 0,
            pending_tool_results: HashMap::new(),
        }
    }

    /// Install a brand-new execution. Existing state for the stable run id is
    /// never overwritten; recovery must explicitly load and continue it.
    pub fn install(&self) -> Result<(), String> {
        with_run_lock(&self.run_id, || {
            let path = checkpoint_path(&self.run_id)?;
            if path.exists() {
                return Err(format!(
                    "local run `{}` already has a recoverable checkpoint",
                    self.run_id
                ));
            }
            self.write_unlocked()
        })
    }

    /// Atomically update the latest checkpoint for one execution. Every writer
    /// reloads under the same per-run lock, so no caller can save a stale full
    /// snapshot over a result or provider phase written by another caller.
    pub fn update<F>(run_id: &str, execution_id: &str, update: F) -> Result<Self, String>
    where
        F: FnOnce(&mut Self) -> Result<(), String>,
    {
        with_run_lock(run_id, || {
            let mut checkpoint = Self::load_unlocked(run_id)?;
            if checkpoint.execution_id != execution_id {
                return Err("local run execution changed before checkpoint update".into());
            }
            update(&mut checkpoint)?;
            checkpoint.write_unlocked()?;
            Ok(checkpoint)
        })
    }

    fn write_unlocked(&self) -> Result<(), String> {
        let path = checkpoint_path(&self.run_id)?;
        let parent = path
            .parent()
            .ok_or_else(|| "local checkpoint path has no parent".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create local checkpoint directory: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("secure local checkpoint directory: {error}"))?;
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("encode local checkpoint: {error}"))?;
        let temporary = path.with_extension(format!(
            "json.{}.{}.tmp",
            self.execution_id,
            uuid::Uuid::new_v4().simple()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("open local checkpoint: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("write local checkpoint: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync local checkpoint: {error}"))?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("commit local checkpoint: {error}"));
        }
        Ok(())
    }

    pub fn load(run_id: &str) -> Result<Self, String> {
        with_run_lock(run_id, || Self::load_unlocked(run_id))
    }

    fn load_unlocked(run_id: &str) -> Result<Self, String> {
        let path = checkpoint_path(run_id)?;
        let bytes =
            std::fs::read(path).map_err(|error| format!("read local checkpoint: {error}"))?;
        let mut checkpoint: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("decode local checkpoint: {error}"))?;
        if checkpoint.run_id != run_id || !matches!(checkpoint.schema_version, 1 | SCHEMA_VERSION) {
            return Err("local checkpoint is incompatible".to_string());
        }
        if checkpoint.execution_id.is_empty() {
            checkpoint.execution_id = uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!(
                    "{}:{}:legacy-local-run",
                    checkpoint.run_id, checkpoint.round
                )
                .as_bytes(),
            )
            .simple()
            .to_string();
        }
        checkpoint.schema_version = SCHEMA_VERSION;
        Ok(checkpoint)
    }

    /// Delete only the execution the caller owns. A cancelled old run cannot
    /// remove a newer turn that reused the same stable run id.
    pub fn delete(run_id: &str, execution_id: &str) -> Result<bool, String> {
        with_run_lock(run_id, || {
            let path = checkpoint_path(run_id)?;
            if path.exists() {
                let checkpoint = Self::load_unlocked(run_id)?;
                if checkpoint.execution_id != execution_id {
                    return Ok(false);
                }
            }
            match std::fs::remove_file(path) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(format!("delete local checkpoint {run_id}: {error}")),
            }
        })
    }
}

fn run_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_run_lock<T>(
    run_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = run_locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(run_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    operation()
}

fn prune_stale_checkpoints() {
    let directory = checkpoint_directory();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_CHECKPOINT_AGE);
        if stale {
            let Some(run_id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if let Err(error) = with_run_lock(run_id, || {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("remove stale checkpoint: {error}"))
            }) {
                log::warn!(
                    "[local-agent] prune stale checkpoint {}: {error}",
                    path.display()
                );
            }
        }
    }
}

fn checkpoint_path(run_id: &str) -> Result<PathBuf, String> {
    if !run_id.starts_with("local-")
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid local run id".to_string());
    }
    Ok(checkpoint_directory().join(format!("{run_id}.json")))
}

fn checkpoint_directory() -> PathBuf {
    #[cfg(test)]
    {
        return std::env::temp_dir()
            .join(format!("dragent-agent-run-tests-{}", std::process::id()));
    }
    #[cfg(not(test))]
    {
        dragent_paths::home().join("agent-runs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn awaiting_checkpoint(run_id: String) -> LocalRunCheckpoint {
        let mut checkpoint = LocalRunCheckpoint::new(
            run_id,
            serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [
                        {"id": "call-a", "name": "record_knowledge", "args": "{}"},
                        {"id": "call-b", "name": "record_knowledge", "args": "{}"}
                    ]
                }]
            }),
        );
        checkpoint.round = 1;
        checkpoint.phase = "awaiting_tools".into();
        checkpoint
    }

    #[test]
    fn rejects_paths_disguised_as_run_ids() {
        assert!(checkpoint_path("../../secret").is_err());
        assert!(checkpoint_path("server-task").is_err());
        assert!(checkpoint_path("local-1234-abcd").is_ok());
    }

    #[test]
    fn older_checkpoints_upgrade_execution_identity_and_default_pending_results() {
        let decoded: LocalRunCheckpoint = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": "local-legacy",
            "request": {},
            "round": 1,
            "total_input_tokens": 2,
            "total_cached_input_tokens": 0,
            "total_output_tokens": 3,
            "context_tokens": 5,
            "phase": "awaiting_tools",
            "read_cache": {},
            "read_only_streak": 0
        }))
        .unwrap();
        assert!(decoded.pending_tool_results.is_empty());
        let run_id = decoded.run_id.clone();
        let mut legacy = decoded;
        legacy.execution_id.clear();
        legacy.schema_version = 1;
        let path = checkpoint_path(&run_id).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let upgraded = LocalRunCheckpoint::load(&run_id).unwrap();
        assert_eq!(upgraded.schema_version, SCHEMA_VERSION);
        assert!(!upgraded.execution_id.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn concurrent_tool_result_updates_both_survive_disk_reload() {
        let run_id = format!("local-test-{}", uuid::Uuid::new_v4().simple());
        let checkpoint = awaiting_checkpoint(run_id.clone());
        let execution_id = checkpoint.execution_id.clone();
        checkpoint.install().unwrap();

        let first_run = run_id.clone();
        let first_execution = execution_id.clone();
        let first = std::thread::spawn(move || {
            LocalRunCheckpoint::update(&first_run, &first_execution, |checkpoint| {
                checkpoint.pending_tool_results.insert(
                    "call-a".into(),
                    CachedToolResult {
                        output: "first".into(),
                        is_error: false,
                    },
                );
                Ok(())
            })
            .unwrap();
        });
        let second_run = run_id.clone();
        let second_execution = execution_id.clone();
        let second = std::thread::spawn(move || {
            LocalRunCheckpoint::update(&second_run, &second_execution, |checkpoint| {
                checkpoint.pending_tool_results.insert(
                    "call-b".into(),
                    CachedToolResult {
                        output: "second".into(),
                        is_error: false,
                    },
                );
                Ok(())
            })
            .unwrap();
        });
        first.join().unwrap();
        second.join().unwrap();

        let reloaded = LocalRunCheckpoint::load(&run_id).unwrap();
        assert_eq!(reloaded.pending_tool_results.len(), 2);
        assert_eq!(reloaded.pending_tool_results["call-a"].output, "first");
        assert_eq!(reloaded.pending_tool_results["call-b"].output, "second");
        LocalRunCheckpoint::delete(&run_id, &execution_id).unwrap();
    }

    #[test]
    fn provider_update_and_tool_result_share_latest_checkpoint() {
        let run_id = format!("local-test-{}", uuid::Uuid::new_v4().simple());
        let checkpoint = awaiting_checkpoint(run_id.clone());
        let execution_id = checkpoint.execution_id.clone();
        checkpoint.install().unwrap();

        let provider_run = run_id.clone();
        let provider_execution = execution_id.clone();
        let provider = std::thread::spawn(move || {
            LocalRunCheckpoint::update(&provider_run, &provider_execution, |checkpoint| {
                checkpoint.context_tokens = 321;
                checkpoint.read_only_streak = 4;
                Ok(())
            })
            .unwrap();
        });
        let result_run = run_id.clone();
        let result_execution = execution_id.clone();
        let result = std::thread::spawn(move || {
            LocalRunCheckpoint::update(&result_run, &result_execution, |checkpoint| {
                checkpoint.pending_tool_results.insert(
                    "call-a".into(),
                    CachedToolResult {
                        output: "accepted".into(),
                        is_error: false,
                    },
                );
                Ok(())
            })
            .unwrap();
        });
        provider.join().unwrap();
        result.join().unwrap();

        let reloaded = LocalRunCheckpoint::load(&run_id).unwrap();
        assert_eq!(reloaded.phase, "awaiting_tools");
        assert_eq!(reloaded.context_tokens, 321);
        assert_eq!(reloaded.read_only_streak, 4);
        assert_eq!(reloaded.pending_tool_results["call-a"].output, "accepted");
        LocalRunCheckpoint::delete(&run_id, &execution_id).unwrap();
    }

    #[test]
    fn old_execution_cannot_update_or_delete_reused_run_id() {
        let run_id = format!("local-test-{}", uuid::Uuid::new_v4().simple());
        let old = awaiting_checkpoint(run_id.clone());
        let old_execution = old.execution_id.clone();
        old.install().unwrap();
        LocalRunCheckpoint::delete(&run_id, &old_execution).unwrap();

        let new = awaiting_checkpoint(run_id.clone());
        let new_execution = new.execution_id.clone();
        new.install().unwrap();
        assert!(LocalRunCheckpoint::update(&run_id, &old_execution, |_| Ok(())).is_err());
        assert!(!LocalRunCheckpoint::delete(&run_id, &old_execution).unwrap());
        assert_eq!(
            LocalRunCheckpoint::load(&run_id).unwrap().execution_id,
            new_execution
        );
        LocalRunCheckpoint::delete(&run_id, &new_execution).unwrap();
    }
}
