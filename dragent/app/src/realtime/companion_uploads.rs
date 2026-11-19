//! Files a phone attaches to a chat (photos, screenshots, documents),
//! received in pieces inside the encrypted channel and saved on this desktop.
//! The chat then names the saved file's path, as the desktop's own "+"
//! attach does, and the agent reads it from there.
//!
//! Methods:
//! - `files.upload_begin {name, size}` -> `{upload}`
//! - `files.upload_chunk {upload, offset, data}` -> `{received}` (`data`:
//!   base64; pieces may arrive in any order, a few at a time)
//! - `files.upload_end {upload}` -> `{path, name, size}`
//!
//! Saved under `<Dragent home>/phone-uploads/<day>/`; anything older than
//! a week is removed when the next upload begins.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;

pub(crate) const METHODS: &[&str] = &["files.upload_begin", "files.upload_chunk", "files.upload_end"];

/// The largest file a phone may attach.
pub(crate) const MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;
/// One piece, decoded (the request stays well under the 64 KB limit).
const MAX_CHUNK_BYTES: usize = 46 * 1024;
/// An upload nobody finished is dropped after this.
const ABANDONED_AFTER: Duration = Duration::from_secs(10 * 60);
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_OPEN: usize = 8;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

struct Upload {
    account: AccountKey,
    name: String,
    size: u64,
    received: u64,
    /// Pieces written so far: offset -> length (never overlapping).
    pieces: BTreeMap<u64, u64>,
    partial: PathBuf,
    file: File,
    touched: Instant,
}

fn uploads() -> &'static Mutex<HashMap<String, Upload>> {
    static UPLOADS: OnceLock<Mutex<HashMap<String, Upload>>> = OnceLock::new();
    UPLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn folder() -> PathBuf {
    dragent_paths::home().join("phone-uploads")
}

/// A file name safe on every platform: the last path part, ordinary
/// characters only, never empty or hidden.
pub(crate) fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let mut name: String = if trimmed.is_empty() { "attachment".into() } else { trimmed.into() };
    if name.chars().count() > 120 {
        // Keep the extension.
        let ext = Path::new(&name).extension().and_then(|e| e.to_str()).map(str::to_string);
        let stem: String = name.chars().take(100).collect();
        name = match ext {
            Some(ext) if ext.len() <= 10 => format!("{stem}.{ext}"),
            _ => stem,
        };
    }
    name
}

/// A path under `dir` named `name`, not taken yet.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("attachment");
    let ext = path.extension().and_then(|e| e.to_str());
    (2..1000)
        .map(|n| dir.join(match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        }))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| dir.join(format!("{}-{name}", uuid::Uuid::new_v4().simple())))
}

/// Remove day folders older than a week (best effort).
fn sweep(root: &Path) {
    let Ok(days) = std::fs::read_dir(root) else { return };
    let cutoff = SystemTime::now().checked_sub(KEEP_FOR);
    for day in days.flatten() {
        let old = day
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .zip(cutoff)
            .is_some_and(|(modified, cutoff)| modified < cutoff);
        if old && day.path().is_dir() {
            let _ = std::fs::remove_dir_all(day.path());
        }
    }
}

fn text<'a>(object: &'a Map<String, Value>, key: &str, max: usize) -> Result<&'a str, Failure> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max && !value.contains('\0'))
        .ok_or(Failure::Invalid)
}

fn only(object: &Map<String, Value>, keys: &[&str]) -> Result<(), Failure> {
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    Ok(())
}

fn begin(account: AccountKey, name: &str, size: u64) -> Result<Value, Failure> {
    let root = folder();
    sweep(&root);
    let partial_dir = root.join(".partial");
    std::fs::create_dir_all(&partial_dir).map_err(|_| Failure::Unavailable)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let partial = partial_dir.join(&id);
    let file = File::create(&partial).map_err(|_| Failure::Unavailable)?;
    let mut open = uploads().lock().unwrap_or_else(|e| e.into_inner());
    open.retain(|_, upload| {
        let keep = upload.touched.elapsed() < ABANDONED_AFTER;
        if !keep {
            let _ = std::fs::remove_file(&upload.partial);
        }
        keep
    });
    if open.len() >= MAX_OPEN {
        let _ = std::fs::remove_file(&partial);
        return Err(Failure::Unavailable);
    }
    open.insert(
        id.clone(),
        Upload { account, name: safe_name(name), size, received: 0, pieces: BTreeMap::new(), partial, file, touched: Instant::now() },
    );
    Ok(json!({"upload": id}))
}

fn chunk(account: &AccountKey, id: &str, offset: u64, data: &str) -> Result<Value, Failure> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| Failure::Invalid)?;
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
        return Err(Failure::Invalid);
    }
    let mut open = uploads().lock().unwrap_or_else(|e| e.into_inner());
    let upload = open.get_mut(id).filter(|u| &u.account == account).ok_or(Failure::Gone)?;
    let length = bytes.len() as u64;
    let end = offset.checked_add(length).ok_or(Failure::Invalid)?;
    if end > upload.size {
        return Err(Failure::Invalid);
    }
    // A piece sent twice (a retry) is acknowledged again; any other overlap
    // is refused.
    if upload.pieces.get(&offset) == Some(&length) {
        return Ok(json!({"received": upload.received}));
    }
    let before = upload.pieces.range(..=offset).next_back().is_some_and(|(at, len)| at + len > offset);
    let after = upload.pieces.range(offset..).next().is_some_and(|(at, _)| *at < end);
    if before || after {
        return Err(Failure::Invalid);
    }
    upload.file.seek(SeekFrom::Start(offset)).map_err(|_| Failure::Unavailable)?;
    upload.file.write_all(&bytes).map_err(|_| Failure::Unavailable)?;
    upload.pieces.insert(offset, length);
    upload.received += length;
    upload.touched = Instant::now();
    Ok(json!({"received": upload.received}))
}

fn end(account: &AccountKey, id: &str) -> Result<Value, Failure> {
    let upload = {
        let mut open = uploads().lock().unwrap_or_else(|e| e.into_inner());
        match open.get(id) {
            Some(upload) if &upload.account == account => open.remove(id),
            _ => None,
        }
    }
    .ok_or(Failure::Gone)?;
    if upload.received != upload.size {
        let _ = std::fs::remove_file(&upload.partial);
        return Err(Failure::Invalid);
    }
    upload.file.sync_all().map_err(|_| Failure::Unavailable)?;
    drop(upload.file);
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dir = folder().join(day);
    std::fs::create_dir_all(&dir).map_err(|_| Failure::Unavailable)?;
    let path = free_path(&dir, &upload.name);
    std::fs::rename(&upload.partial, &path).map_err(|_| Failure::Unavailable)?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(upload.name),
        "size": upload.size,
    }))
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(account: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    let account = account.clone();
    match method {
        "files.upload_begin" => {
            only(object, &["name", "size"])?;
            let name = text(object, "name", 512)?.to_string();
            let size = object
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| *size > 0 && *size <= MAX_UPLOAD_BYTES)
                .ok_or(Failure::Invalid)?;
            Ok(Box::new(move || begin(account, &name, size)))
        }
        "files.upload_chunk" => {
            only(object, &["upload", "offset", "data"])?;
            let id = text(object, "upload", 64)?.to_string();
            let offset = object.get("offset").and_then(Value::as_u64).ok_or(Failure::Invalid)?;
            let data = text(object, "data", MAX_CHUNK_BYTES * 2)?.to_string();
            Ok(Box::new(move || chunk(&account, &id, offset, &data)))
        }
        "files.upload_end" => {
            only(object, &["upload"])?;
            let id = text(object, "upload", 64)?.to_string();
            Ok(Box::new(move || end(&account, &id)))
        }
        _ => Err(Failure::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_plain_and_inside_the_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\me\\photo.jpg"), "photo.jpg");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("   "), "attachment");
        assert_eq!(safe_name("a<b>:c.png"), "a_b__c.png");
        let long = format!("{}.png", "x".repeat(300));
        assert!(safe_name(&long).ends_with(".png") && safe_name(&long).chars().count() <= 104);
    }

    #[test]
    fn a_file_arrives_in_pieces() {
        let account = AccountKey::new("u", "o");
        let started = begin(account.clone(), "shot.png", 6).unwrap();
        let id = started["upload"].as_str().unwrap().to_string();
        let piece = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(chunk(&account, &id, 0, &piece(b"abc")).unwrap()["received"], 3);
        // A retried piece is acknowledged, an overlap refused, and pieces
        // may come out of order.
        assert_eq!(chunk(&account, &id, 0, &piece(b"abc")).unwrap()["received"], 3);
        assert!(chunk(&account, &id, 2, &piece(b"cd")).is_err());
        assert_eq!(chunk(&account, &id, 5, &piece(b"f")).unwrap()["received"], 4);
        // Another account can't touch it.
        assert!(chunk(&AccountKey::new("x", "o"), &id, 3, &piece(b"def")).is_err());
        chunk(&account, &id, 3, &piece(b"de")).unwrap();
        let done = end(&account, &id).unwrap();
        let path = PathBuf::from(done["path"].as_str().unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn requests_are_checked() {
        let key = AccountKey::new("u", "o");
        assert!(job(&key, "files.upload_begin", &json!({"name": "a.png", "size": 0})).is_err());
        assert!(job(&key, "files.upload_begin", &json!({"name": "a.png", "size": MAX_UPLOAD_BYTES + 1})).is_err());
        assert!(job(&key, "files.upload_begin", &json!({"name": "a.png", "size": 10, "path": "/etc"})).is_err());
        assert!(job(&key, "files.upload_begin", &json!({"name": "a.png", "size": 10})).is_ok());
    }
}
