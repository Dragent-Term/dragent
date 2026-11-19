//! Write-time content snapshots for per-message restore.
//!
//! Unlike a git snapshot (which only sees tracked files and needs a repo),
//! this captures the *prior bytes* of every file an agent turn is about to
//! write, the instant before the write happens. Because every agent mutation
//! flows through a known write tool, we can record the before-state at the
//! source and reverse it exactly on restore — including brand-new files
//! (recorded as "did not exist" → deleted on restore) and edits in folders
//! that aren't git repositories at all.
//!
//! Snapshots are keyed to the **user message** whose turn made the change, so
//! restoring a message rewinds every file that message (and its tool rounds /
//! background continuations) touched, and nothing else.
//!
//! Each pane/conversation store is also written atomically under Dragent's
//! private application-data directory. The source preimages therefore survive
//! process restarts without being mixed across terminal tabs or conversations.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CHECKPOINT_SCHEMA_VERSION: u32 = 1;

/// One file's pre-write state within a message's checkpoint.
#[derive(Clone, Serialize, Deserialize)]
struct FileSnapshot {
    path: PathBuf,
    /// `Some(bytes)` = file existed with these contents before the message
    /// touched it. `None` = the file did not exist (a create → delete on
    /// restore).
    prior: Option<Vec<u8>>,
    /// Monotonic mutation order. A shell command discovers its targets only
    /// after it exits, so an earlier shell snapshot may arrive after a later
    /// structured edit capture. The earliest preimage must still win.
    sequence: u64,
}

/// Per-message checkpoint: the set of files the message changed, each recorded
/// once at its first mutation.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MessageCheckpoint {
    files: Vec<FileSnapshot>,
}

impl MessageCheckpoint {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Record `path`'s current on-disk state if this message hasn't captured
    /// it yet (first-write-wins, so the snapshot is the state before the
    /// message, not before each individual edit).
    fn capture_path(&mut self, path: &Path, sequence: u64) {
        let prior = std::fs::read(path).ok();
        self.capture_preimage(path.to_path_buf(), prior, sequence);
    }

    fn capture_preimage(&mut self, path: PathBuf, prior: Option<Vec<u8>>, sequence: u64) {
        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            if existing.sequence <= sequence {
                return;
            }
            existing.prior = prior;
            existing.sequence = sequence;
            return;
        }
        self.files.push(FileSnapshot {
            path,
            prior,
            sequence,
        });
    }

    /// Restore every captured file to its pre-message state. Returns
    /// (restored, deleted, failed) counts.
    fn restore(&self) -> (usize, usize, usize) {
        let (mut restored, mut deleted, mut failed) = (0, 0, 0);
        // Reverse order so a rename captured as (create dst, delete src) undoes
        // cleanly.
        for snap in self.files.iter().rev() {
            match &snap.prior {
                Some(bytes) => {
                    if let Some(parent) = snap.path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&snap.path, bytes) {
                        Ok(()) => restored += 1,
                        Err(e) => {
                            log::warn!(
                                "[checkpoint] restore write {} failed: {e}",
                                snap.path.display()
                            );
                            failed += 1;
                        }
                    }
                }
                None => {
                    // The message created this path — remove it.
                    if snap.path.is_dir() {
                        let _ = std::fs::remove_dir_all(&snap.path);
                        deleted += 1;
                    } else if snap.path.exists() {
                        match std::fs::remove_file(&snap.path) {
                            Ok(()) => deleted += 1,
                            Err(e) => {
                                log::warn!(
                                    "[checkpoint] restore delete {} failed: {e}",
                                    snap.path.display()
                                );
                                failed += 1;
                            }
                        }
                    }
                }
            }
        }
        (restored, deleted, failed)
    }
}

/// All message checkpoints for a controller, keyed by the UserQuery block id.
pub struct CheckpointStore {
    by_message: HashMap<usize, MessageCheckpoint>,
    /// Insertion order of message ids, for "revert the most recent" and to cap
    /// memory (oldest checkpoints drop first).
    order: Vec<usize>,
    next_sequence: u64,
    persistence_path: Option<PathBuf>,
}

impl Default for CheckpointStore {
    fn default() -> Self {
        Self {
            by_message: HashMap::new(),
            order: Vec::new(),
            next_sequence: 0,
            persistence_path: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedCheckpointStore {
    schema_version: u32,
    by_message: HashMap<usize, MessageCheckpoint>,
    order: Vec<usize>,
    next_sequence: u64,
}

/// How many messages' checkpoints to retain (older ones are dropped).
const MAX_CHECKPOINTS: usize = 20;

impl CheckpointStore {
    /// Bind this in-memory store to one pane conversation and load any durable
    /// snapshots left by an earlier app process.
    pub fn bind(&mut self, pane_uuid: Uuid, conversation_id: Uuid) {
        self.bind_path(checkpoint_path(pane_uuid, conversation_id));
    }

    fn bind_path(&mut self, path: PathBuf) {
        if self.persistence_path.as_ref() == Some(&path) {
            return;
        }
        self.reset_memory();
        self.persistence_path = Some(path.clone());

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                log::warn!("[checkpoint] read {} failed: {error}", path.display());
                return;
            }
        };
        match serde_json::from_slice::<PersistedCheckpointStore>(&bytes) {
            Ok(saved) if saved.schema_version == CHECKPOINT_SCHEMA_VERSION => {
                self.by_message = saved.by_message;
                self.order = saved.order;
                self.next_sequence = saved.next_sequence;
                self.sanitize_order();
            }
            Ok(_) => log::warn!(
                "[checkpoint] ignoring incompatible checkpoint {}",
                path.display()
            ),
            Err(error) => log::warn!("[checkpoint] decode {} failed: {error}", path.display()),
        }
    }

    /// Re-key snapshots after persisted agent blocks are assigned fresh runtime
    /// IDs. The marker stored on each UserQuery supplies the old→new mapping.
    pub fn remap_message_ids(&mut self, remap: &HashMap<usize, usize>) {
        if remap.is_empty() {
            return;
        }
        let mut remapped = HashMap::with_capacity(self.by_message.len());
        for (old_id, checkpoint) in std::mem::take(&mut self.by_message) {
            remapped.insert(remap.get(&old_id).copied().unwrap_or(old_id), checkpoint);
        }
        self.by_message = remapped;
        for id in &mut self.order {
            if let Some(new_id) = remap.get(id) {
                *id = *new_id;
            }
        }
        self.sanitize_order();
        self.persist();
    }

    /// Capture `path`'s pre-write state under `message_id`. Called for every
    /// target of every write tool, before the write runs.
    pub fn capture(&mut self, message_id: usize, path: &Path) {
        let sequence = self.reserve_sequence();
        let entry = self.by_message.entry(message_id).or_insert_with(|| {
            self.order.push(message_id);
            MessageCheckpoint::default()
        });
        entry.capture_path(path, sequence);

        if self.order.len() > MAX_CHECKPOINTS {
            let oldest = self.order.remove(0);
            self.by_message.remove(&oldest);
        }
        self.persist();
    }

    fn reserve_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }

    /// Register an Agent-run shell command before it starts. The filesystem
    /// scan itself runs on the command's worker thread via
    /// [`begin_shell_checkpoint`], never in render/model code.
    pub fn register_shell_command(&mut self, call_id: &str, message_id: usize, root: &Path) {
        let registration = ShellCheckpointRegistration {
            message_id,
            sequence: self.reserve_sequence(),
            root: root.to_path_buf(),
        };
        if let Ok(mut registrations) = SHELL_REGISTRATIONS.lock() {
            registrations.insert(call_id.to_string(), registration);
        }
    }

    /// Attach a completed shell command's discovered file preimages to its
    /// originating message checkpoint. Returns `(message_id, changed_files)`.
    pub fn ingest_shell_command(&mut self, call_id: &str) -> Option<(usize, usize)> {
        let completed = SHELL_COMPLETED.lock().ok()?.remove(call_id)?;
        let changed = completed.files.len();
        if changed == 0 {
            return Some((completed.message_id, 0));
        }
        let entry = self
            .by_message
            .entry(completed.message_id)
            .or_insert_with(|| {
                self.order.push(completed.message_id);
                MessageCheckpoint::default()
            });
        for file in completed.files {
            entry.capture_preimage(file.path, file.prior, completed.sequence);
        }
        if self.order.len() > MAX_CHECKPOINTS {
            let oldest = self.order.remove(0);
            self.by_message.remove(&oldest);
        }
        self.persist();
        Some((completed.message_id, changed))
    }

    /// Whether `message_id` has any captured files (→ show a restore control).
    pub fn has_checkpoint(&self, message_id: usize) -> bool {
        self.by_message
            .get(&message_id)
            .is_some_and(|c| !c.is_empty())
    }

    /// Per-file path plus added/deleted line counts for `message_id`'s
    /// checkpoint (computed against each file's current on-disk content), for
    /// a restore preview. Does not mutate anything. `None` if there's no live
    /// checkpoint.
    pub fn preview_stats(&self, message_id: usize) -> Option<Vec<(PathBuf, u32, u32)>> {
        let cp = self.by_message.get(&message_id)?;
        if cp.is_empty() {
            return None;
        }
        Some(
            cp.files
                .iter()
                .map(|f| {
                    let current = std::fs::read_to_string(&f.path).unwrap_or_default();
                    let prior = f
                        .prior
                        .as_ref()
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    let (added, deleted) = diff_line_counts(&prior, &current);
                    (f.path.clone(), added, deleted)
                })
                .collect(),
        )
    }

    /// The newest message id that has a checkpoint (for "revert last change").
    pub fn latest_message(&self) -> Option<usize> {
        self.order
            .iter()
            .rev()
            .find(|id| self.has_checkpoint(**id))
            .copied()
    }

    /// Restore files to their state before `message_id`. Returns a human
    /// summary, or None if that message has no checkpoint.
    pub fn restore(&self, message_id: usize) -> Option<String> {
        let cp = self.by_message.get(&message_id)?;
        if cp.is_empty() {
            return None;
        }
        let (restored, deleted, failed) = cp.restore();
        let mut parts = Vec::new();
        if restored > 0 {
            parts.push(format!("{restored} file{} restored", plural(restored)));
        }
        if deleted > 0 {
            parts.push(format!("{deleted} created file{} removed", plural(deleted)));
        }
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        Some(if parts.is_empty() {
            "Nothing to restore.".to_string()
        } else {
            format!("Restored checkpoint: {}.", parts.join(", "))
        })
    }

    pub fn clear(&mut self) {
        self.reset_memory();
        self.persistence_path = None;
    }

    /// Forget checkpoints belonging to discarded later turns and update the
    /// durable copy. The selected message remains restorable.
    pub fn discard_after(&mut self, message_id: usize) {
        let Some(position) = self.order.iter().position(|id| *id == message_id) else {
            return;
        };
        let discarded = self.order.split_off(position + 1);
        for id in discarded {
            self.by_message.remove(&id);
        }
        self.persist();
    }

    fn reset_memory(&mut self) {
        self.by_message.clear();
        self.order.clear();
        self.next_sequence = 0;
    }

    fn sanitize_order(&mut self) {
        self.order.retain(|id| self.by_message.contains_key(id));
        self.order.dedup();
        while self.order.len() > MAX_CHECKPOINTS {
            let oldest = self.order.remove(0);
            self.by_message.remove(&oldest);
        }
    }

    fn persist(&self) {
        let Some(path) = self.persistence_path.as_ref() else {
            return;
        };
        if let Err(error) = persist_store(path, self) {
            log::warn!("[checkpoint] persist {} failed: {error}", path.display());
        }
    }
}

fn checkpoint_path(pane_uuid: Uuid, conversation_id: Uuid) -> PathBuf {
    dragent_paths::home()
        .join("restore-checkpoints")
        .join(pane_uuid.to_string())
        .join(format!("{conversation_id}.json"))
}

fn persist_store(path: &Path, store: &CheckpointStore) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "checkpoint path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create checkpoint directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure checkpoint directory: {error}"))?;
    }

    let saved = PersistedCheckpointStore {
        schema_version: CHECKPOINT_SCHEMA_VERSION,
        by_message: store.by_message.clone(),
        order: store.order.clone(),
        next_sequence: store.next_sequence,
    };
    let bytes =
        serde_json::to_vec(&saved).map_err(|error| format!("encode checkpoint: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("open temporary checkpoint: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("write checkpoint: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync checkpoint: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("commit checkpoint: {error}"))?;
    Ok(())
}

#[derive(Clone)]
struct ShellCheckpointRegistration {
    message_id: usize,
    sequence: u64,
    root: PathBuf,
}

struct WorkspaceSnapshot {
    registration: ShellCheckpointRegistration,
    files: HashMap<PathBuf, Vec<u8>>,
}

struct CompletedShellCheckpoint {
    message_id: usize,
    sequence: u64,
    files: Vec<FileSnapshot>,
}

static SHELL_REGISTRATIONS: Lazy<std::sync::Mutex<HashMap<String, ShellCheckpointRegistration>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));
static SHELL_ACTIVE: Lazy<std::sync::Mutex<HashMap<String, WorkspaceSnapshot>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));
static SHELL_COMPLETED: Lazy<std::sync::Mutex<HashMap<String, CompletedShellCheckpoint>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Capture the workspace immediately before a registered Agent shell command.
/// Git/ignore rules are honored, so dependency caches and build output do not
/// turn a checkpoint into an unbounded copy of `node_modules`/`target`.
pub fn begin_shell_checkpoint(call_id: &str) {
    let registration = SHELL_REGISTRATIONS
        .lock()
        .ok()
        .and_then(|mut registrations| registrations.remove(call_id));
    let Some(registration) = registration else {
        return;
    };
    let files = scan_workspace_files(&registration.root);
    if let Ok(mut active) = SHELL_ACTIVE.lock() {
        active.insert(
            call_id.to_string(),
            WorkspaceSnapshot {
                registration,
                files,
            },
        );
    }
}

/// Compare the workspace after an Agent shell command with its pre-command
/// snapshot and retain preimages only for files the command actually changed.
pub fn finish_shell_checkpoint(call_id: &str) {
    let before = SHELL_ACTIVE
        .lock()
        .ok()
        .and_then(|mut active| active.remove(call_id));
    let Some(before) = before else {
        return;
    };
    let mut after = scan_workspace_files(&before.registration.root);
    let mut changed = Vec::new();
    for (path, prior) in before.files {
        match after.remove(&path) {
            Some(current) if current == prior => {}
            _ => changed.push(FileSnapshot {
                path,
                prior: Some(prior),
                sequence: before.registration.sequence,
            }),
        }
    }
    for path in after.into_keys() {
        changed.push(FileSnapshot {
            path,
            prior: None,
            sequence: before.registration.sequence,
        });
    }
    if let Ok(mut completed) = SHELL_COMPLETED.lock() {
        completed.insert(
            call_id.to_string(),
            CompletedShellCheckpoint {
                message_id: before.registration.message_id,
                sequence: before.registration.sequence,
                files: changed,
            },
        );
    }
}

fn scan_workspace_files(root: &Path) -> HashMap<PathBuf, Vec<u8>> {
    if !root.is_dir() {
        return HashMap::new();
    }
    let mut files = HashMap::new();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .standard_filters(true)
        .follow_links(false);
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        match std::fs::read(&path) {
            Ok(bytes) => {
                files.insert(path, bytes);
            }
            Err(error) => log::debug!(
                "[checkpoint] shell snapshot skipped {}: {error}",
                path.display()
            ),
        }
    }
    files
}

/// Count added/deleted lines between `old` and `new` via the same line-level
/// diff the write/edit tools use for their own change summaries.
fn diff_line_counts(old: &str, new: &str) -> (u32, u32) {
    let hunks = super::diff::line_diff(old, new);
    let (mut added, mut deleted) = (0u32, 0u32);
    for hunk in &hunks {
        for line in &hunk.lines {
            match line {
                super::diff::DiffLine::Added(_) => added += 1,
                super::diff::DiffLine::Removed(_) => deleted += 1,
                super::diff::DiffLine::Context(_) => {}
            }
        }
    }
    (added, deleted)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Extract the filesystem targets a write tool will mutate, resolved against
/// `cwd`. Covers the structured tools directly and best-effort-parses
/// apply_patch's file markers.
pub fn write_target_paths(tool: &str, args_json: &str, cwd: &Path) -> Vec<PathBuf> {
    let resolve = |p: &str| -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    };
    let v: serde_json::Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    match tool {
        "write_file" | "edit_file" | "create_directory" | "delete_file" => {
            if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
                push(resolve(p));
            }
        }
        "move_file" => {
            for key in ["source", "destination", "from", "to"] {
                if let Some(p) = v.get(key).and_then(|x| x.as_str()) {
                    push(resolve(p));
                }
            }
        }
        "batch_edit" => {
            if let Some(edits) = v.get("edits").and_then(|e| e.as_array()) {
                for edit in edits {
                    if let Some(p) = edit.get("path").and_then(|x| x.as_str()) {
                        push(resolve(p));
                    }
                }
            }
        }
        "apply_patch" => {
            // Patch text lives under "patch"/"input"/"content"; scan for the
            // path markers of the common patch dialects.
            let patch = v
                .get("patch")
                .or_else(|| v.get("input"))
                .or_else(|| v.get("content"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            for line in patch.lines() {
                let l = line.trim();
                for prefix in [
                    "*** Update File: ",
                    "*** Add File: ",
                    "*** Delete File: ",
                    "+++ b/",
                    "--- a/",
                ] {
                    if let Some(rest) = l.strip_prefix(prefix) {
                        let p = rest.trim();
                        if !p.is_empty() && p != "/dev/null" {
                            push(resolve(p));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_and_restore_modified_file() {
        let dir = std::env::temp_dir().join(format!("ckpt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"original").unwrap();

        let mut store = CheckpointStore::default();
        store.capture(1, &f); // before edit
        std::fs::write(&f, b"agent edit").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"agent edit");

        assert!(store.has_checkpoint(1));
        store.restore(1).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"original");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn created_file_is_removed_on_restore() {
        let dir = std::env::temp_dir().join(format!("ckpt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("new.txt");

        let mut store = CheckpointStore::default();
        store.capture(2, &f); // captured as "did not exist"
        std::fs::write(&f, b"created by agent").unwrap();
        assert!(f.exists());

        store.restore(2).unwrap();
        assert!(!f.exists(), "created file must be deleted on restore");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_write_wins_across_a_message() {
        let dir = std::env::temp_dir().join(format!("ckpt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"v0").unwrap();

        let mut store = CheckpointStore::default();
        store.capture(3, &f);
        std::fs::write(&f, b"v1").unwrap();
        store.capture(3, &f); // second capture in same message is a no-op
        std::fs::write(&f, b"v2").unwrap();

        store.restore(3).unwrap();
        assert_eq!(
            std::fs::read(&f).unwrap(),
            b"v0",
            "restores to before the message"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persisted_checkpoint_survives_store_restart() {
        let dir = std::env::temp_dir().join(format!("durable-ckpt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("source.rs");
        let store_path = dir.join("state/checkpoint.json");
        std::fs::write(&file, b"before agent").unwrap();

        {
            let mut store = CheckpointStore::default();
            store.bind_path(store_path.clone());
            store.capture(12, &file);
            std::fs::write(&file, b"after agent").unwrap();
        }

        let mut restored_store = CheckpointStore::default();
        restored_store.bind_path(store_path);
        assert!(restored_store.has_checkpoint(12));
        restored_store.restore(12).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"before agent");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persisted_checkpoint_ids_remap_to_restored_block_ids() {
        let dir = std::env::temp_dir().join(format!("remap-ckpt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("source.rs");
        let store_path = dir.join("checkpoint.json");
        std::fs::write(&file, b"before").unwrap();

        let mut store = CheckpointStore::default();
        store.bind_path(store_path.clone());
        store.capture(41, &file);
        drop(store);

        let mut loaded = CheckpointStore::default();
        loaded.bind_path(store_path);
        loaded.remap_message_ids(&HashMap::from([(41, 3)]));
        assert!(!loaded.has_checkpoint(41));
        assert!(loaded.has_checkpoint(3));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shell_checkpoint_restores_modified_created_and_deleted_files() {
        let dir = std::env::temp_dir().join(format!("shell-ckpt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let modified = dir.join("modified.txt");
        let deleted = dir.join("deleted.txt");
        let created = dir.join("created.txt");
        std::fs::write(&modified, b"before").unwrap();
        std::fs::write(&deleted, b"keep me").unwrap();

        let call_id = uuid::Uuid::new_v4().to_string();
        let mut store = CheckpointStore::default();
        store.register_shell_command(&call_id, 7, &dir);
        begin_shell_checkpoint(&call_id);
        std::fs::write(&modified, b"after").unwrap();
        std::fs::remove_file(&deleted).unwrap();
        std::fs::write(&created, b"new").unwrap();
        finish_shell_checkpoint(&call_id);

        assert_eq!(store.ingest_shell_command(&call_id), Some((7, 3)));
        store.restore(7).unwrap();
        assert_eq!(std::fs::read(&modified).unwrap(), b"before");
        assert_eq!(std::fs::read(&deleted).unwrap(), b"keep me");
        assert!(!created.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn earlier_shell_preimage_wins_if_ingested_after_structured_capture() {
        let dir = std::env::temp_dir().join(format!("shell-order-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ordered.txt");
        std::fs::write(&file, b"v0").unwrap();

        let call_id = uuid::Uuid::new_v4().to_string();
        let mut store = CheckpointStore::default();
        store.register_shell_command(&call_id, 8, &dir);
        begin_shell_checkpoint(&call_id);
        std::fs::write(&file, b"v1-shell").unwrap();

        // A later structured edit is captured before the background shell
        // completion is ingested into the model.
        store.capture(8, &file);
        std::fs::write(&file, b"v2-structured").unwrap();
        finish_shell_checkpoint(&call_id);
        store.ingest_shell_command(&call_id).unwrap();

        store.restore(8).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"v0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extract_paths_from_write_tools() {
        let cwd = Path::new("/proj");
        assert_eq!(
            write_target_paths("write_file", r#"{"path":"src/a.rs"}"#, cwd),
            vec![PathBuf::from("/proj/src/a.rs")]
        );
        assert_eq!(
            write_target_paths(
                "batch_edit",
                r#"{"edits":[{"path":"a"},{"path":"b"}]}"#,
                cwd
            ),
            vec![PathBuf::from("/proj/a"), PathBuf::from("/proj/b")]
        );
        let mv = write_target_paths("move_file", r#"{"source":"a","destination":"b"}"#, cwd);
        assert!(mv.contains(&PathBuf::from("/proj/a")) && mv.contains(&PathBuf::from("/proj/b")));
        let patch = write_target_paths(
            "apply_patch",
            r#"{"patch":"*** Update File: src/x.rs\n@@\n-old\n+new"}"#,
            cwd,
        );
        assert_eq!(patch, vec![PathBuf::from("/proj/src/x.rs")]);
    }
}
