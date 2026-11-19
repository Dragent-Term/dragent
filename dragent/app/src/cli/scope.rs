//! Which machine a project, workspace or path is on, for command results:
//! this computer, or a machine Dragent reaches over SSH (whose files appear
//! here under a stand-in folder that holds nothing locally).

use std::path::Path;

use serde_json::{json, Value};

/// The machine `path` belongs to: `{"kind": "desktop", …}` for this
/// computer, or `{"kind": "ssh", "id", "name", "path"}` with the path as
/// that machine sees it.
pub(crate) fn machine_of(path: &Path) -> Value {
    match crate::remote_fs::resolve(path) {
        Some(remote) => json!({
            "kind": "ssh",
            "id": remote.machine_id,
            "name": crate::remote_fs::machine_name(path).unwrap_or_else(|| remote.machine_id.clone()),
            "path": remote.path,
        }),
        None => dragent_host::scope::this_machine(),
    }
}

/// Whether `path` is on a remote machine, so nothing here may probe it on
/// the local disk (its stand-in folder is empty) or act on it locally.
pub(crate) fn is_remote(path: &Path) -> bool {
    crate::remote_fs::is_remote(path)
}

/// Refuse to act on `path` from this computer when it's on a remote
/// machine: `code` names the case, `what` says what must run there.
pub(crate) fn refuse_remote(path: &Path, code: &str, what: &str, next: &str) -> Result<(), String> {
    if !is_remote(path) {
        return Ok(());
    }
    let machine = machine_of(path);
    Err(dragent_host::outcome::Failure::new(
        dragent_host::outcome::Status::Unsupported,
        code,
        format!(
            "{} is on the machine {}: {what}.",
            machine["path"].as_str().unwrap_or_default(),
            machine["name"].as_str().unwrap_or_default()
        ),
    )
    .next(next)
    .encode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_host::outcome::{Failure, Status};

    #[test]
    fn results_name_the_machine_a_path_is_on() {
        let remote = crate::remote_fs::alias("build-box", "/home/dev/app");
        let machine = machine_of(&remote);
        assert_eq!(
            (machine["kind"].as_str(), machine["id"].as_str(), machine["path"].as_str()),
            (Some("ssh"), Some("build-box"), Some("/home/dev/app"))
        );
        assert!(is_remote(&remote));
        let local = machine_of(Path::new("/tmp/project"));
        assert_eq!(local["kind"], "desktop");
        assert!(local["name"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(!is_remote(Path::new("/tmp/project")));
    }

    #[test]
    fn work_for_a_remote_project_is_refused_here_never_done_locally() {
        let remote = crate::remote_fs::alias("build-box", "/home/dev/app");
        let refused = Failure::from_error(
            &refuse_remote(&remote, "remote_project", "its team has to run there", "Run it there.").unwrap_err(),
        );
        assert_eq!((refused.status, refused.code.as_str()), (Status::Unsupported, "remote_project"));
        assert!(refused.message.starts_with("/home/dev/app is on the machine "), "{}", refused.message);
        assert!(refuse_remote(Path::new("/tmp/project"), "remote_project", "x", "y").is_ok());
    }
}

