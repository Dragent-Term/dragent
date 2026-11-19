//! `--idempotency-key KEY`: run a command at most once per key, so an agent
//! retrying after a timeout or a lost connection can't post the same comment,
//! create the same workspace or send the same message twice.
//!
//! The first run with a key is recorded before it starts and its result
//! after it ends. Running again with the same key and arguments then:
//! - replays a finished run's output;
//! - reports a run still under way as `accepted`;
//! - refuses, as `timeout`/`outcome_unknown`, when the earlier run's outcome
//!   is unknown — the caller checks what happened (the request id helps)
//!   and uses a new key if it really should run again;
//! - runs again when the earlier run failed before anything happened.
//!
//! Reusing a key for different arguments is refused as `invalid_input`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dragent_host::outcome::{Failure, Status};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub(super) const FLAG: &str = "--idempotency-key";
/// Records older than this are forgotten.
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A run that started this long ago and never finished is presumed lost.
const UNDER_WAY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Started,
    Done,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    /// Hash of the command and arguments the key was first used with.
    command: String,
    stage: Stage,
    started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<Failure>,
}

fn ledger() -> PathBuf {
    dragent_paths::organisation_dir().join("cli-idempotency")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Remove `--idempotency-key KEY` (or `=KEY`) from `args`; the key if given.
pub(super) fn take_key(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let invalid = |message: &str| Failure::new(Status::InvalidInput, "invalid_idempotency_key", message).encode();
    let mut key = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == FLAG {
            let value = args.get(index + 1).cloned().ok_or_else(|| invalid("--idempotency-key needs a value"))?;
            args.drain(index..index + 2);
            key = Some(value);
        } else if let Some(value) = args[index].strip_prefix(&format!("{FLAG}=")) {
            key = Some(value.to_string());
            args.remove(index);
        } else {
            index += 1;
        }
    }
    match key {
        Some(key) if key.trim().is_empty() || key.chars().count() > 200 => {
            Err(invalid("an idempotency key is 1 to 200 characters"))
        }
        other => Ok(other),
    }
}

/// Run `command` at most once for `key`.
pub(super) fn run(
    key: &str,
    args: &[String],
    command: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    run_in(&ledger(), key, args, command)
}

fn run_in(
    dir: &Path,
    key: &str,
    args: &[String],
    command: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("couldn't record the idempotency key: {error}"))?;
    prune(dir);
    let path = dir.join(format!("{}.json", digest(key)));
    let identity = digest(&args.join("\0"));
    let started = Entry {
        command: identity.clone(),
        stage: Stage::Started,
        started_at_ms: now_ms(),
        output: None,
        failure: None,
    };
    let first = std::fs::OpenOptions::new().write(true).create_new(true).open(&path);
    if first.is_err() {
        let earlier: Option<Entry> = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match earlier {
            Some(entry) if entry.command != identity => {
                return Err(Failure::new(
                    Status::InvalidInput,
                    "idempotency_key_reused",
                    "This idempotency key was already used for a different command.",
                )
                .next("Use a new key for a different operation.")
                .encode());
            }
            Some(Entry { stage: Stage::Done, output, .. }) => {
                eprintln!("dragent: replayed the result of the earlier run with this idempotency key");
                return Ok(output.unwrap_or_default());
            }
            Some(Entry { stage: Stage::Failed, failure: Some(failure), .. }) if failure.retry_safe => {}
            Some(Entry { stage: Stage::Started, started_at_ms, .. })
                if now_ms().saturating_sub(started_at_ms) < UNDER_WAY.as_millis() as u64 =>
            {
                return Ok(json!({
                    "status": "accepted",
                    "code": "in_progress",
                    "message": "A run with this idempotency key is still under way.",
                    "idempotency_key": key,
                })
                .to_string());
            }
            Some(entry) => {
                let mut failure = Failure::new(
                    Status::Timeout,
                    "outcome_unknown",
                    "An earlier run with this idempotency key didn't confirm its outcome, so it isn't run again.",
                )
                .next("Check whether the earlier run happened; if it didn't and it should run, use a new key.");
                failure.request_id = entry.failure.and_then(|earlier| earlier.request_id);
                return Err(failure.encode());
            }
            // Unreadable record: treat the key as fresh.
            None => {}
        }
    }
    write(&path, &started);
    let result = command();
    let finished = match &result {
        Ok(output) => Entry {
            stage: Stage::Done,
            output: Some(output.clone()),
            ..started
        },
        Err(error) => Entry {
            stage: Stage::Failed,
            failure: Some(Failure::from_error(error)),
            ..started
        },
    };
    write(&path, &finished);
    result
}

fn write(path: &Path, entry: &Entry) {
    if let Ok(bytes) = serde_json::to_vec(entry) {
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, path);
        }
    }
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_key_is_taken_out_of_the_arguments() {
        let mut a = args(&["integrations", "comment", "--idempotency-key", "k1", "X-1", "done"]);
        assert_eq!(take_key(&mut a).unwrap().as_deref(), Some("k1"));
        assert_eq!(a, args(&["integrations", "comment", "X-1", "done"]));
        let mut b = args(&["panes", "open", "--idempotency-key=k2"]);
        assert_eq!(take_key(&mut b).unwrap().as_deref(), Some("k2"));
        assert_eq!(b, args(&["panes", "open"]));
        assert!(take_key(&mut args(&["x", "--idempotency-key"])).is_err());
        assert!(take_key(&mut args(&["x", "--idempotency-key", " "])).is_err());
        assert_eq!(take_key(&mut args(&["x"])).unwrap(), None);
    }

    #[test]
    fn a_finished_run_is_replayed_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let runs = Cell::new(0);
        let command = args(&["integrations", "comment", "X-1", "hi"]);
        let run = || {
            run_in(dir.path(), "k", &command, || {
                runs.set(runs.get() + 1);
                Ok("{\"id\":\"c1\"}".to_string())
            })
        };
        assert_eq!(run().unwrap(), "{\"id\":\"c1\"}");
        assert_eq!(run().unwrap(), "{\"id\":\"c1\"}");
        assert_eq!(runs.get(), 1);
        // The same key for something else is refused.
        let other = run_in(dir.path(), "k", &args(&["integrations", "comment", "X-2", "hi"]), || Ok(String::new()));
        assert_eq!(Failure::from_error(&other.unwrap_err()).code, "idempotency_key_reused");
    }

    #[test]
    fn an_unknown_outcome_is_never_run_again() {
        let dir = tempfile::tempdir().unwrap();
        let command = args(&["workspaces", "create"]);
        let timeout = Failure::new(Status::Timeout, "no_answer", "late").request("req-1").encode();
        assert!(run_in(dir.path(), "k", &command, || Err(timeout.clone())).is_err());
        let again = run_in(dir.path(), "k", &command, || panic!("must not run again"));
        let failure = Failure::from_error(&again.unwrap_err());
        assert_eq!((failure.status, failure.code.as_str()), (Status::Timeout, "outcome_unknown"));
        assert_eq!(failure.request_id.as_deref(), Some("req-1"), "points at what to inspect");
    }

    #[test]
    fn a_run_that_did_nothing_can_run_again() {
        let dir = tempfile::tempdir().unwrap();
        let command = args(&["panes", "open"]);
        let unavailable = Failure::new(Status::Unavailable, "app_not_open", "closed").encode();
        assert!(run_in(dir.path(), "k", &command, || Err(unavailable.clone())).is_err());
        assert_eq!(run_in(dir.path(), "k", &command, || Ok("opened".into())).unwrap(), "opened");
    }

    #[test]
    fn a_run_under_way_is_reported_as_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let command = args(&["team", "create", "goal"]);
        let path = dir.path().join(format!("{}.json", digest("k")));
        write(&path, &Entry {
            command: digest(&command.join("\0")),
            stage: Stage::Started,
            started_at_ms: now_ms(),
            output: None,
            failure: None,
        });
        let output = run_in(dir.path(), "k", &command, || panic!("must not run twice")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["status"], "accepted");
        assert_eq!(value["code"], "in_progress");
    }
}
