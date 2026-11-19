//! Per-session disk history for cold restore (Phase 1).
//!
//! One directory per terminal session under `<org_dir>/pty-history/`:
//! `<session_id>/meta.json` ([`HistoryRecord`]) plus `<session_id>/output.bin`
//! (raw PTY bytes, capped at [`MAX_HISTORY_BYTES`]). All writes are
//! best-effort: errors are swallowed so history never breaks the live path.
//! A dead daemon's sessions survive on disk as tombstones (`ended_at` stamped
//! by [`HistoryStore::close_session`], `None` while the session is live).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use super::protocol::HistoryRecord;

pub const MAX_HISTORY_BYTES: u64 = 1024 * 1024;
const META_NAME: &str = "meta.json";
const DATA_NAME: &str = "output.bin";
const DIR_NAME: &str = "pty-history";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reject path-traversal / separator payloads for session ids.
fn safe_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && !session_id.contains('/')
        && !session_id.contains('\\')
        && !session_id.contains('\0')
        && session_id != "."
        && session_id != ".."
}

#[cfg(unix)]
fn restrict_600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_600(_path: &Path) {}

/// Disk-backed per-session terminal history, rooted at `<org_dir>/pty-history`.
#[derive(Debug, Clone)]
pub struct HistoryStore {
    base: PathBuf,
}

/// One bounded mailbox per live history file. The disk worker never holds
/// this lock during I/O, and the PTY producer never waits for disk progress.
struct PendingHistory {
    bytes: Vec<u8>,
    remaining: usize,
    closed: bool,
}

pub(crate) struct HistoryWriter {
    pending: Arc<(Mutex<PendingHistory>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HistoryWriter {
    pub(crate) fn append(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let (lock, ready) = &*self.pending;
        let mut pending = lock.lock().unwrap_or_else(|p| p.into_inner());
        if pending.closed || pending.remaining == 0 {
            return;
        }
        let count = bytes.len().min(pending.remaining);
        pending.bytes.extend_from_slice(&bytes[..count]);
        pending.remaining -= count;
        ready.notify_one();
    }
}

impl Drop for HistoryWriter {
    fn drop(&mut self) {
        let (lock, ready) = &*self.pending;
        lock.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        ready.notify_one();
        // Detach: the worker drains accepted bytes before exiting. Dropping a
        // PTY callback must not wait on a slow or unavailable filesystem.
        let _ = self.worker.take();
    }
}

impl HistoryStore {
    /// Open (creating) `<org_dir>/pty-history/`.
    pub fn open(org_dir: &Path) -> std::io::Result<Self> {
        let base = org_dir.join(DIR_NAME);
        std::fs::create_dir_all(&base)?;
        Ok(Self { base })
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base.join(session_id)
    }

    /// Open once during session setup, before starting the PTY. Pending plus
    /// written bytes never exceed the existing history cap. A worker exits
    /// after reaching the cap or draining a closed producer.
    pub(crate) fn writer(&self, session_id: &str) -> std::io::Result<HistoryWriter> {
        use std::io::{Error, ErrorKind, Write};
        if !safe_id(session_id) {
            return Err(Error::new(ErrorKind::InvalidInput, "invalid session id"));
        }
        let dir = self.session_dir(session_id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(DATA_NAME);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        restrict_600(&path);
        let remaining = MAX_HISTORY_BYTES.saturating_sub(file.metadata()?.len()) as usize;
        let pending = Arc::new((
            Mutex::new(PendingHistory {
                bytes: Vec::new(),
                remaining,
                closed: false,
            }),
            Condvar::new(),
        ));
        let shared = pending.clone();
        let worker = if remaining == 0 {
            None
        } else {
            Some(
                std::thread::Builder::new()
                    .name("pty-history".into())
                    .spawn(move || {
                        let (lock, ready) = &*shared;
                        loop {
                            let state = lock.lock().unwrap_or_else(|p| p.into_inner());
                            let mut state = ready
                                .wait_while(state, |s| {
                                    s.bytes.is_empty() && !s.closed && s.remaining > 0
                                })
                                .unwrap_or_else(|p| p.into_inner());
                            if state.bytes.is_empty() {
                                break;
                            }
                            let batch = std::mem::take(&mut state.bytes);
                            drop(state);
                            if let Err(error) = file.write_all(&batch) {
                                log::warn!("terminal history write failed: {error}");
                                let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
                                state.closed = true;
                                state.bytes.clear();
                                break;
                            }
                        }
                    })?,
            )
        };
        Ok(HistoryWriter { pending, worker })
    }

    /// Append raw PTY bytes, capped at [`MAX_HISTORY_BYTES`]. Writes the
    /// partial prefix that fits; swallows all errors.
    pub fn append(&self, session_id: &str, data: &[u8]) {
        if data.is_empty() || !safe_id(session_id) {
            return;
        }
        let dir = self.session_dir(session_id);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(DATA_NAME);
        let current = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if current >= MAX_HISTORY_BYTES {
            return;
        }
        let budget = (MAX_HISTORY_BYTES - current) as usize;
        let chunk = &data[..data.len().min(budget)];
        if chunk.is_empty() {
            return;
        }
        let existed = path.exists();
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(chunk)
            });
        if result.is_err() {
            return;
        }
        if !existed {
            restrict_600(&path);
        }
    }

    /// Persist (or overwrite) the session meta record. Swallows all errors.
    pub fn write_meta(&self, record: &HistoryRecord) {
        if !safe_id(&record.session_id) {
            return;
        }
        let dir = self.session_dir(&record.session_id);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(META_NAME);
        let tmp = dir.join(format!("{META_NAME}.tmp-{}", std::process::id()));
        let Ok(raw) = serde_json::to_vec(record) else {
            return;
        };
        let Ok(_) = std::fs::write(&tmp, &raw) else {
            return;
        };
        restrict_600(&tmp);
        let _ = std::fs::rename(&tmp, &path);
        restrict_600(&path);
    }

    /// Load one session's meta record. `None` when missing or corrupt.
    pub fn read_meta(&self, session_id: &str) -> Option<HistoryRecord> {
        if !safe_id(session_id) {
            return None;
        }
        let raw = std::fs::read(self.session_dir(session_id).join(META_NAME)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Stamp `ended_at` (+ `exit_code`) on a session. When meta is missing
    /// (crash between spawn and meta write), a minimal fresh record is
    /// created so the tombstone still lists.
    pub fn close_session(&self, session_id: &str, exit_code: Option<i32>) {
        if !safe_id(session_id) {
            return;
        }
        let now = now_ms();
        match self.read_meta(session_id) {
            Some(mut record) => {
                record.ended_at_unix_ms = Some(now);
                record.exit_code = exit_code;
                self.write_meta(&record);
            }
            None => {
                self.write_meta(&HistoryRecord {
                    session_id: session_id.to_string(),
                    workspace_id: String::new(),
                    home_id: String::new(),
                    cols: 0,
                    rows: 0,
                    cwd: String::new(),
                    started_at_unix_ms: now,
                    ended_at_unix_ms: Some(now),
                    exit_code,
                });
            }
        }
    }

    /// Last `max_bytes` of a session's output plus its meta record.
    /// `None` when the session has no meta record.
    pub fn read_tail(
        &self,
        session_id: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, HistoryRecord)> {
        let record = self.read_meta(session_id)?;
        let raw = std::fs::read(self.session_dir(session_id).join(DATA_NAME)).unwrap_or_default();
        let tail = if raw.len() > max_bytes {
            raw[raw.len() - max_bytes..].to_vec()
        } else {
            raw
        };
        Some((tail, record))
    }

    /// All meta records, oldest-first. Garbage dirs (missing/corrupt meta)
    /// are skipped.
    pub fn list(&self) -> Vec<HistoryRecord> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.base) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !safe_id(&name) {
                continue;
            }
            if let Some(record) = self.read_meta(&name) {
                out.push(record);
            }
        }
        out.sort_by_key(|r| r.started_at_unix_ms);
        out
    }

    /// Remove one session's history (meta + output). Swallows all errors.
    pub fn purge_session(&self, session_id: &str) {
        if !safe_id(session_id) {
            return;
        }
        let _ = std::fs::remove_dir_all(self.session_dir(session_id));
    }

    /// Remove recordings of terminals whose directory no longer exists (its
    /// project or worktree was deleted): nothing can be restored there.
    /// Returns how many were purged.
    pub fn purge_missing_cwds(&self) -> usize {
        let mut count = 0;
        for record in self.list() {
            if !record.cwd.is_empty() && !Path::new(&record.cwd).exists() {
                self.purge_session(&record.session_id);
                count += 1;
            }
        }
        count
    }

    /// Remove every session belonging to `home_id`. Returns how many were purged.
    pub fn purge_home(&self, home_id: &str) -> usize {
        let mut count = 0;
        for record in self.list() {
            if record.home_id == home_id {
                self.purge_session(&record.session_id);
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dragent-history-test-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn record(session_id: &str, home_id: &str, started: u64) -> HistoryRecord {
        HistoryRecord {
            session_id: session_id.to_string(),
            workspace_id: "ws-1".to_string(),
            home_id: home_id.to_string(),
            cols: 80,
            rows: 24,
            cwd: "/tmp".to_string(),
            started_at_unix_ms: started,
            ended_at_unix_ms: None,
            exit_code: None,
        }
    }

    #[test]
    fn recordings_of_terminals_in_deleted_folders_are_purged() {
        let dir = scratch("missing");
        let store = HistoryStore::open(&dir).expect("open");
        let existing = dir.join("project");
        std::fs::create_dir_all(&existing).unwrap();
        let mut kept = record("term-kept", "home-1", now_ms());
        kept.cwd = existing.to_string_lossy().into_owned();
        let mut gone = record("term-gone", "home-2", now_ms());
        gone.cwd = dir.join("deleted-worktree").to_string_lossy().into_owned();
        store.write_meta(&kept);
        store.write_meta(&gone);

        assert_eq!(store.purge_missing_cwds(), 1);
        assert!(store.read_meta("term-kept").is_some());
        assert!(store.read_meta("term-gone").is_none());
    }

    #[test]
    fn append_read_close_stamps_ended_at() {
        let dir = scratch("arc");
        let store = HistoryStore::open(&dir).expect("open");
        let mut rec = record("term-a", "home-1", now_ms());
        store.write_meta(&rec);
        store.append("term-a", b"hello ");
        store.append("term-a", b"world");
        let (tail, back) = store.read_tail("term-a", 1024).expect("tail");
        assert_eq!(tail, b"hello world");
        assert_eq!(back.session_id, "term-a");
        assert!(back.ended_at_unix_ms.is_none());

        store.close_session("term-a", Some(3));
        rec = store.read_meta("term-a").expect("meta after close");
        assert!(rec.ended_at_unix_ms.is_some());
        assert_eq!(rec.exit_code, Some(3));

        // Missing meta still stamps a minimal tombstone.
        store.close_session("term-missing", None);
        let tomb = store.read_meta("term-missing").expect("minimal record");
        assert_eq!(tomb.session_id, "term-missing");
        assert!(tomb.ended_at_unix_ms.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_enforces_one_mb_cap() {
        let dir = scratch("cap");
        let store = HistoryStore::open(&dir).expect("open");
        store.write_meta(&record("term-cap", "home-1", now_ms()));
        let big = vec![b'x'; MAX_HISTORY_BYTES as usize + 4096];
        store.append("term-cap", &big);
        let (tail, _) = store.read_tail("term-cap", usize::MAX).expect("tail");
        assert_eq!(tail.len(), MAX_HISTORY_BYTES as usize);
        // Further appends are no-ops past the cap.
        store.append("term-cap", b"more");
        let (tail2, _) = store.read_tail("term-cap", usize::MAX).expect("tail2");
        assert_eq!(tail2.len(), MAX_HISTORY_BYTES as usize);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn drain_writer(mut writer: HistoryWriter) {
        let worker = writer.worker.take();
        drop(writer);
        if let Some(worker) = worker {
            worker.join().expect("history worker");
        }
    }

    #[test]
    fn async_writer_preserves_order_and_drains_on_close() {
        let dir = scratch("async-order");
        let store = HistoryStore::open(&dir).unwrap();
        store.write_meta(&record("term-a", "home-1", now_ms()));
        let writer = store.writer("term-a").unwrap();
        let mut expected = Vec::new();
        for i in 0..10_000u32 {
            let bytes = i.to_le_bytes();
            writer.append(&bytes);
            expected.extend_from_slice(&bytes);
        }
        drain_writer(writer);
        assert_eq!(store.read_tail("term-a", usize::MAX).unwrap().0, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn async_writer_respects_existing_file_budget() {
        let dir = scratch("async-cap");
        let store = HistoryStore::open(&dir).unwrap();
        store.write_meta(&record("term-a", "home-1", now_ms()));
        store.append("term-a", b"prefix");
        let writer = store.writer("term-a").unwrap();
        writer.append(&vec![b'x'; MAX_HISTORY_BYTES as usize + 4096]);
        writer.append(b"must not fit");
        drain_writer(writer);
        let output = store.read_tail("term-a", usize::MAX).unwrap().0;
        assert_eq!(output.len(), MAX_HISTORY_BYTES as usize);
        assert_eq!(&output[..6], b"prefix");
        assert!(output[6..].iter().all(|b| *b == b'x'));
        let writer = store.writer("term-a").unwrap();
        assert!(writer.worker.is_none(), "full history needs no worker");
        drain_writer(writer);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_mailbox_is_bounded_even_without_a_disk_consumer() {
        let pending = Arc::new((
            Mutex::new(PendingHistory {
                bytes: Vec::new(),
                remaining: MAX_HISTORY_BYTES as usize,
                closed: false,
            }),
            Condvar::new(),
        ));
        let writer = HistoryWriter {
            pending: pending.clone(),
            worker: None,
        };
        for _ in 0..100 {
            writer.append(&[b'a'; 64 * 1024]);
        }
        let state = pending.0.lock().unwrap();
        assert_eq!(state.bytes.len(), MAX_HISTORY_BYTES as usize);
        assert_eq!(state.remaining, 0);
    }

    #[test]
    fn list_skips_garbage_and_purge_home_clears() {
        let dir = scratch("list");
        let store = HistoryStore::open(&dir).expect("open");
        store.write_meta(&record("term-1", "home-a", 10));
        store.write_meta(&record("term-2", "home-a", 20));
        store.write_meta(&record("term-3", "home-b", 30));
        // Garbage dirs: no meta, corrupt meta.
        std::fs::create_dir_all(dir.join("pty-history").join("junk-no-meta")).unwrap();
        std::fs::create_dir_all(dir.join("pty-history").join("junk-bad")).unwrap();
        std::fs::write(
            dir.join("pty-history").join("junk-bad").join("meta.json"),
            b"not json",
        )
        .unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].session_id, "term-1");
        assert_eq!(listed[2].session_id, "term-3");

        assert_eq!(store.purge_home("home-a"), 2);
        let rest = store.list();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].session_id, "term-3");
        assert!(store.read_tail("term-1", 64).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
