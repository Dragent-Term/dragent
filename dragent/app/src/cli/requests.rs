//! Requests from the command line to the open app.
//!
//! The command line writes `<id>.json` into the organisation's
//! `cli-requests` folder; the app claims it by renaming it to
//! `<id>.claimed`, does the work, and answers in `<id>.done.json`. Only
//! processes of this user can write there, so (unlike a deep link,
//! which any web page can open) a request can't come from a web page.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A request the app hasn't claimed in this long is dropped: it was meant
/// for a window that isn't open.
pub const UNCLAIMED_TTL: Duration = Duration::from_secs(60);

pub use dragent_host::cli_requests::{Request, Response};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Envelope {
    created_at_ms: u64,
    request: Request,
}

pub fn folder() -> PathBuf {
    dragent_paths::organisation_dir().join("cli-requests")
}

/// Identity discovery only. Commands still go through the selected
/// organisation's inbox and its live authentication checks.
pub fn identity_folder() -> PathBuf {
    dragent_paths::home().join("cli-identity")
}

pub fn scoped_identity_folder(context: &str) -> PathBuf {
    dragent_paths::organisation_dir_for(context).join("cli-identity")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, path)
}

/// Answers nobody collected are kept this long for `dragent requests show`.
pub const ANSWER_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Queue `request` in `folder`; returns its id.
pub fn submit(folder: &Path, request: Request) -> Result<String, String> {
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("couldn't create {}: {error}", folder.display()))?;
    prune_answers(folder, ANSWER_TTL);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let envelope = Envelope {
        created_at_ms: now_ms(),
        request,
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|error| error.to_string())?;
    write_private(&folder.join(format!("{id}.json")), &bytes)
        .map_err(|error| format!("couldn't queue the request: {error}"))?;
    Ok(id)
}

/// Whether the app has claimed request `id`.
pub fn claimed(folder: &Path, id: &str) -> bool {
    folder.join(format!("{id}.claimed")).exists() || folder.join(format!("{id}.done.json")).exists()
}

/// Withdraw an unclaimed request. False when the app claimed it meanwhile.
pub fn withdraw(folder: &Path, id: &str) -> bool {
    std::fs::remove_file(folder.join(format!("{id}.json"))).is_ok()
}

/// Where a request stands, for inspecting one whose caller stopped waiting.
#[derive(Debug, PartialEq)]
pub enum State {
    /// Written, not yet taken by the app.
    Queued,
    /// The app took it and is working on it.
    InProgress,
    /// The app answered.
    Done(Response),
    /// Never existed, already collected, or expired.
    Unknown,
}

/// Look at request `id` without collecting its answer.
pub fn inspect(folder: &Path, id: &str) -> State {
    // Ids are uuids; anything else could reach outside the folder.
    if uuid::Uuid::parse_str(id).is_err() {
        return State::Unknown;
    }
    if let Some(response) = std::fs::read(folder.join(format!("{id}.done.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        return State::Done(response);
    }
    if folder.join(format!("{id}.claimed")).exists() {
        return State::InProgress;
    }
    if folder.join(format!("{id}.json")).exists() {
        return State::Queued;
    }
    State::Unknown
}

/// Remove answers older than `ttl` that nobody collected.
fn prune_answers(folder: &Path, ttl: Duration) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".done.json") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ttl);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The app's answer to request `id`, once it has one (read once).
pub fn take_response(folder: &Path, id: &str) -> Option<Response> {
    let path = folder.join(format!("{id}.done.json"));
    let bytes = std::fs::read(&path).ok()?;
    let response = serde_json::from_slice(&bytes).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(response)
}

/// App side: claim every fresh request in `folder`, oldest first. Stale
/// ones are deleted unanswered.
pub fn claim_all(folder: &Path) -> Vec<(String, Request)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut claimed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|id| !id.ends_with(".done"))
            .map(str::to_string)
        else {
            continue;
        };
        let envelope = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Envelope>(&bytes).ok());
        let Some(envelope) = envelope else {
            // From a newer command line, most likely: say so rather than
            // leave it waiting.
            if std::fs::rename(&path, folder.join(format!("{id}.claimed"))).is_ok() {
                respond(
                    folder,
                    &id,
                    &Response::failure(
                        dragent_host::outcome::Failure::new(
                            dragent_host::outcome::Status::Unsupported,
                            "app_too_old",
                            "This version of Dragent doesn't know that command. Update Dragent and try again.",
                        )
                        .next("Update the Dragent app, then run the command again."),
                    ),
                );
            }
            continue;
        };
        if now_ms().saturating_sub(envelope.created_at_ms) > UNCLAIMED_TTL.as_millis() as u64 {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        // The rename is the claim: whoever renames first owns it.
        if std::fs::rename(&path, folder.join(format!("{id}.claimed"))).is_ok() {
            claimed.push((envelope.created_at_ms, id, envelope.request));
        }
    }
    claimed.sort_by_key(|(created, ..)| *created);
    claimed.into_iter().map(|(_, id, request)| (id, request)).collect()
}

/// App side: answer a claimed request.
pub fn respond(folder: &Path, id: &str, response: &Response) {
    if let Ok(bytes) = serde_json::to_vec(response) {
        if let Err(error) = write_private(&folder.join(format!("{id}.done.json")), &bytes) {
            log::warn!("[cli] couldn't answer request {id}: {error}");
        }
    }
    let _ = std::fs::remove_file(folder.join(format!("{id}.claimed")));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(prompt: &str) -> Request {
        Request::CreateWorkspace {
            project: "/repo".into(),
            agent: Some("claude".into()),
            prompt: prompt.into(),
            branch: None,
            base_branch: None,
            name: None,
            pull_request: None,
            local: false,
            command: None,
            tags: Vec::new(),
            model: None,
            background: false,
        }
    }

    #[test]
    fn a_request_is_claimed_once_and_answered() {
        let folder = tempfile::tempdir().unwrap();
        let id = submit(folder.path(), create("fix the tests")).unwrap();
        assert!(!claimed(folder.path(), &id));

        let first = claim_all(folder.path());
        assert_eq!(first, vec![(id.clone(), create("fix the tests"))]);
        assert!(claim_all(folder.path()).is_empty(), "claimed only once");
        assert!(claimed(folder.path(), &id));
        assert!(!withdraw(folder.path(), &id), "a claimed request can't be withdrawn");

        respond(
            folder.path(),
            &id,
            &Response {
                ok: true,
                branch: Some("fix-tests".into()),
                ..Response::default()
            },
        );
        let response = take_response(folder.path(), &id).unwrap();
        assert!(response.ok);
        assert_eq!(response.branch.as_deref(), Some("fix-tests"));
        assert!(take_response(folder.path(), &id).is_none(), "read once");
    }

    #[test]
    fn stale_and_unreadable_requests_are_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let stale = Envelope {
            created_at_ms: now_ms() - UNCLAIMED_TTL.as_millis() as u64 - 1_000,
            request: create("old"),
        };
        std::fs::write(folder.path().join("old.json"), serde_json::to_vec(&stale).unwrap()).unwrap();
        assert!(claim_all(folder.path()).is_empty());
        assert!(std::fs::read_dir(folder.path()).unwrap().next().is_none());
    }

    #[test]
    fn an_unknown_request_gets_an_answer() {
        let folder = tempfile::tempdir().unwrap();
        let unknown = format!(r#"{{"created_at_ms":{},"request":{{"kind":"teleport"}}}}"#, now_ms());
        std::fs::write(folder.path().join("new.json"), unknown).unwrap();
        assert!(claim_all(folder.path()).is_empty());
        let response = take_response(folder.path(), "new").unwrap();
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("Update Dragent"));
        let failure = response.failure.unwrap();
        assert_eq!(failure.status, dragent_host::outcome::Status::Unsupported);
        assert_eq!(failure.code, "app_too_old");
    }

    #[test]
    fn a_request_can_be_inspected_without_collecting_its_answer() {
        let folder = tempfile::tempdir().unwrap();
        let id = submit(folder.path(), create("late")).unwrap();
        assert_eq!(inspect(folder.path(), &id), State::Queued);
        claim_all(folder.path());
        assert_eq!(inspect(folder.path(), &id), State::InProgress);
        respond(folder.path(), &id, &Response::done());
        assert_eq!(inspect(folder.path(), &id), State::Done(Response::done()));
        assert_eq!(inspect(folder.path(), &id), State::Done(Response::done()), "inspecting leaves it");
        assert!(take_response(folder.path(), &id).is_some());
        assert_eq!(inspect(folder.path(), &id), State::Unknown);
        assert_eq!(inspect(folder.path(), "../../etc/passwd"), State::Unknown);
        // Uncollected answers expire.
        let other = submit(folder.path(), create("forgotten")).unwrap();
        claim_all(folder.path());
        respond(folder.path(), &other, &Response::done());
        prune_answers(folder.path(), Duration::ZERO);
        assert_eq!(inspect(folder.path(), &other), State::Unknown);
    }
}
