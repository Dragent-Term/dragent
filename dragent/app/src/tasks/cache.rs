//! The last list per organisation on disk, so the Tasks page opens
//! instantly and reads while offline (changes still need the server).

use std::path::{Path, PathBuf};

use dragent_tasks::Task;

/// What's remembered: the tasks and the organisation's key prefix.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct CachedList {
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub fetched_at_ms: i64,
}

fn path_for(dir: &Path) -> PathBuf {
    dir.join("tasks").join("cache.json")
}

/// The cache file for `org`. Scoped per organisation like the rest of the
/// local data.
pub(crate) fn path(org: &str) -> PathBuf {
    path_for(&dragent_paths::organisation_dir_for(org))
}

/// The last saved list, if any. A broken file reads as no cache rather than
/// an error: the next refresh writes a good one.
pub(crate) fn load(org: &str) -> Option<CachedList> {
    let bytes = std::fs::read(path(org)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Remember `tasks`, atomically: a crash mid-write must never leave half a
/// file behind.
pub(crate) fn save(org: &str, cached: &CachedList) -> Result<(), String> {
    let path = path(org);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(cached).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, &path).map_err(|error| error.to_string())?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

pub(crate) fn stamped(tasks: Vec<Task>, prefix: String) -> CachedList {
    CachedList {
        tasks,
        prefix,
        fetched_at_ms: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_tasks::{Priority, Status};

    fn task(key: &str) -> Task {
        Task {
            id: format!("id-{key}"),
            key: key.into(),
            number: 1,
            title: format!("Task {key}"),
            description: String::new(),
            status: Status::Todo,
            priority: Priority::None,
            assignee: None,
            creator: None,
            project: None,
            labels: Vec::new(),
            due_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 0,
        }
    }

    #[test]
    fn a_saved_list_comes_back_and_a_broken_file_reads_as_no_cache() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.scratch")
            .join(format!("tasks-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = path_for(&dir);
        assert_eq!(path, dir.join("tasks").join("cache.json"));

        // Save through the real helper shape: write then read back.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cached = stamped(vec![task("DRA-1")], "DRA".into());
        let bytes = serde_json::to_vec(&cached).unwrap();
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, &bytes).unwrap();
        assert!(!path.exists(), "nothing readable until the rename");
        std::fs::rename(&temp, &path).unwrap();
        let back: CachedList = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, cached);

        std::fs::write(&path, b"{not json").unwrap();
        assert!(serde_json::from_slice::<CachedList>(&std::fs::read(&path).unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
