//! Remote projects seen from this Mac.
//!
//! Each saved machine's filesystem appears under one stand-in folder,
//! `<organisation dir>/remote/<machine id>/`, so `/home/dev/app` on machine
//! `m` is the local path `…/remote/m/home/dev/app`. A remote project is then
//! an ordinary project rooted at that path: the sidebar, workspace tabs,
//! panes, Files, Source Control and editor are the same components as for a
//! local project. Only their I/O differs — every path under a stand-in is
//! served by that machine's host (files, git, search, terminals), never by
//! the local disk.
use dragent_host::HostClient;
use dragent_host::development::{
    DevelopmentRequest, DevelopmentResponse, FileEntry, FsRequest, FsStat,
};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// A path on a remote machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePath {
    pub machine_id: String,
    /// Absolute POSIX path on the machine.
    pub path: String,
}

struct Connection {
    client: HostClient,
    home: String,
    shell: String,
    name: String,
}

static CONNECTIONS: LazyLock<RwLock<HashMap<String, Connection>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Parent of every machine's stand-in folder.
pub fn mounts_root() -> PathBuf {
    dragent_paths::organisation_dir().join("remote")
}

fn valid_machine_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Validate identities before constructing local stand-ins. Remote data must
/// never turn into local parent traversal, a drive prefix or a separator.
pub(crate) fn checked_alias(machine_id: &str, remote: &str) -> Option<PathBuf> {
    if !valid_machine_id(machine_id) || !remote.starts_with('/') || remote.contains('\0') {
        return None;
    }
    let mut path = mounts_root().join(machine_id);
    for part in remote
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if part == ".." || part.contains('\\') || part.contains(':') {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

/// The local stand-in for `remote` on `machine_id`. Malformed host output is
/// quarantined under an invalid remote identity so I/O fails closed.
pub fn alias(machine_id: &str, remote: &str) -> PathBuf {
    checked_alias(machine_id, remote).unwrap_or_else(|| {
        log::warn!("Rejected an invalid remote path identity");
        mounts_root()
            .join(".invalid")
            .join(uuid::Uuid::new_v4().to_string())
    })
}

/// The machine path behind a local stand-in path, if `path` is one.
pub fn resolve(path: &Path) -> Option<RemotePath> {
    let root = mounts_root();
    let rest: PathBuf = match path.strip_prefix(&root) {
        Ok(rest) => rest.to_path_buf(),
        // Callers often canonicalize; the stand-in root may sit behind a
        // symlink (e.g. /var → /private/var).
        Err(_) => path
            .strip_prefix(root.canonicalize().ok()?)
            .ok()?
            .to_path_buf(),
    };
    // A malformed mounted path must still route to remote I/O and fail, never
    // fall through into local disk access. No connection can have this id.
    let invalid = || {
        Some(RemotePath {
            machine_id: String::new(),
            path: "/".into(),
        })
    };
    let mut components = rest.components();
    let machine_id = match components.next() {
        Some(Component::Normal(name)) => match name.to_str() {
            Some(name) => name.to_string(),
            None => return invalid(),
        },
        _ => return invalid(),
    };
    if !valid_machine_id(&machine_id) {
        return invalid();
    }
    let mut remote = String::new();
    for component in components {
        match component {
            Component::Normal(part) => {
                remote.push('/');
                let Some(part) = part.to_str() else {
                    return invalid();
                };
                if part.contains(['\\', ':', '\0']) {
                    return invalid();
                }
                remote.push_str(part);
            }
            Component::CurDir => {}
            // Reject ambiguous stand-ins rather than clamping into another path.
            _ => return invalid(),
        }
    }
    if remote.is_empty() {
        remote.push('/');
    }
    Some(RemotePath {
        machine_id,
        path: remote,
    })
}

pub fn is_remote(path: &Path) -> bool {
    resolve(path).is_some()
}

/// Remote folders that become project or workspace roots get an empty
/// local stand-in directory, so local `is_dir` checks treat them like any
/// other project folder. Nothing is ever written inside it.
pub fn ensure_stand_in(path: &Path) {
    if is_remote(path) {
        let _ = std::fs::create_dir_all(path);
    }
}

/// `path.exists()` for local paths, the machine's answer for remote ones.
pub fn path_exists(path: &Path) -> bool {
    if is_remote(path) {
        exists(path)
    } else {
        path.exists()
    }
}

/// Read a small text file from wherever `path` lives.
pub fn read_to_string(path: &Path) -> Option<String> {
    if is_remote(path) {
        read(path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    } else {
        std::fs::read_to_string(path).ok()
    }
}

/// `read_to_string` for metadata that UI code consults on every refresh
/// (`.git` pointer files, HEAD): remote answers are kept for a minute so
/// the sidebar never waits on the network. Local reads are never cached.
pub fn read_to_string_cached(path: &Path) -> Option<String> {
    if !is_remote(path) {
        return std::fs::read_to_string(path).ok();
    }
    cached(&TEXT_CACHE, path, || read_to_string(path))
}

/// The repository root containing remote `path`, as a stand-in path. UI code
/// asks this on every refresh, so answers are kept like
/// [`read_to_string_cached`].
pub fn git_root(path: &Path) -> Option<PathBuf> {
    cached(&ROOT_CACHE, path, || {
        // `path` may be a file; git runs in its folder then.
        [Some(path), path.parent()]
            .into_iter()
            .flatten()
            .find_map(|dir| {
                let output = crate::git_cmd::GitCommand::new()
                    .args(["rev-parse", "--show-toplevel"])
                    .current_dir(dir)
                    .output()
                    .ok()?;
                let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (output.status.success() && !root.is_empty()).then(|| PathBuf::from(root))
            })
    })
}

type Cache<T> = LazyLock<std::sync::Mutex<HashMap<PathBuf, (std::time::Instant, T)>>>;

static TEXT_CACHE: Cache<Option<String>> = LazyLock::new(Default::default);
static ROOT_CACHE: Cache<Option<PathBuf>> = LazyLock::new(Default::default);

/// Forget every remembered remote answer. Called when a terminal command
/// finishes (it may have switched branches or worktrees) and when a machine
/// connects or goes away.
pub fn forget_cached() {
    if let Ok(mut cache) = TEXT_CACHE.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = ROOT_CACHE.lock() {
        cache.clear();
    }
}

/// Answer for remote `path` from `cache` when under a minute old, else from
/// `load`. Unreachable machines are not cached: the answer may change on
/// connect.
fn cached<T: Clone + Default>(cache: &Cache<T>, path: &Path, load: impl FnOnce() -> T) -> T {
    if let Some((at, value)) = cache.lock().ok().and_then(|c| c.get(path).cloned()) {
        if at.elapsed() < std::time::Duration::from_secs(60) {
            return value;
        }
    }
    if !resolve(path).is_some_and(|remote| is_connected(&remote.machine_id)) {
        return T::default();
    }
    let value = load();
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            path.to_path_buf(),
            (std::time::Instant::now(), value.clone()),
        );
    }
    value
}

/// Map a path a remote shell reported back into the machine's stand-in.
pub fn alias_remote_path(machine_id: &str, reported: &Path) -> PathBuf {
    if resolve(reported).is_some() || !reported.is_absolute() {
        return reported.to_path_buf();
    }
    alias(machine_id, &reported.to_string_lossy())
}

/// The machine is connected: its paths become reachable.
pub fn connect(machine_id: &str, client: HostClient, home: String, shell: String, name: String) {
    if !valid_machine_id(machine_id) {
        log::warn!("Rejected an invalid remote machine identity");
        return;
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let client = client.clone();
        let enabled = crate::integrations::agents::enabled();
        std::thread::spawn(move || {
            if let Err(error) = client.coordination(
                "host_mcp_configure",
                serde_json::json!({"enabled": enabled}),
                serde_json::to_value(crate::cli::team::Caller::person()).unwrap(),
            ) {
                log::warn!("[remote] MCP registration: {error}");
            }
        });
    }
    #[cfg(not(target_family = "wasm"))]
    crate::integrations::ssh_bridge::start(machine_id, client.clone());
    forget_cached();
    if let Ok(mut connections) = CONNECTIONS.write() {
        connections.insert(
            machine_id.to_string(),
            Connection {
                client,
                home,
                shell,
                name,
            },
        );
    }
}

pub fn disconnect(machine_id: &str) {
    #[cfg(not(target_family = "wasm"))]
    crate::integrations::ssh_bridge::stop(machine_id);
    if let Ok(mut connections) = CONNECTIONS.write() {
        connections.remove(machine_id);
    }
    forget_cached();
}

/// Why a remote path can't be reached: its machine isn't connected (yet).
pub const NOT_CONNECTED: &str = "This machine is not connected. Connect it from Settings → Machines.";

pub fn client(machine_id: &str) -> Result<HostClient, String> {
    CONNECTIONS
        .read()
        .ok()
        .and_then(|connections| connections.get(machine_id).map(|c| c.client.clone()))
        .ok_or_else(|| NOT_CONNECTED.to_string())
}

/// Ids of the machines whose paths are reachable now, sorted.
pub fn connected_machines() -> Vec<String> {
    let mut ids: Vec<String> = CONNECTIONS
        .read()
        .map(|connections| connections.keys().cloned().collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

pub fn is_connected(machine_id: &str) -> bool {
    CONNECTIONS
        .read()
        .is_ok_and(|connections| connections.contains_key(machine_id))
}

/// The machine's login shell, for new remote terminals.
pub fn shell(machine_id: &str) -> Option<String> {
    CONNECTIONS
        .read()
        .ok()?
        .get(machine_id)
        .map(|c| c.shell.clone())
}

pub fn home(machine_id: &str) -> Option<String> {
    CONNECTIONS
        .read()
        .ok()?
        .get(machine_id)
        .map(|c| c.home.clone())
}

/// Display name of the machine behind `path` ("This Mac" chips, tooltips).
pub fn machine_name(path: &Path) -> Option<String> {
    let remote = resolve(path)?;
    let connected = CONNECTIONS
        .read()
        .ok()
        .and_then(|connections| connections.get(&remote.machine_id).map(|c| c.name.clone()));
    connected.or_else(|| {
        dragent_host::machine_registry::MachineRegistry::load()
            .ok()?
            .machines
            .into_iter()
            .find(|m| m.id == remote.machine_id)
            .map(|m| m.name())
    })
}

/// The host that owns `path` and the path as that host sees it: the
/// machine for stand-in paths, this Mac's host for everything else.
pub fn host_for(path: &Path) -> Result<(HostClient, String), String> {
    match resolve(path) {
        Some(remote) => Ok((client(&remote.machine_id)?, remote.path)),
        None => Ok((
            HostClient::ensure_host()?,
            path.to_string_lossy().into_owned(),
        )),
    }
}

/// Rewrite stand-in paths inside a command argument (`-C <dir>`, worktree
/// destinations, `path:line` forms) into the machine's own paths.
pub fn translate_arg(machine_id: &str, arg: &str) -> String {
    let prefix = mounts_root().join(machine_id);
    let prefix = prefix.to_string_lossy();
    if !arg.contains(prefix.as_ref()) {
        return arg.to_string();
    }
    let replaced = arg.replace(prefix.as_ref(), "");
    if replaced.is_empty() {
        "/".into()
    } else {
        replaced
    }
}

/// Map machine paths in host output back to their stand-ins (worktree paths,
/// `git worktree list`), so the desktop keeps seeing one path space.
pub fn alias_output(machine_id: &str, text: &str, roots: &[&str]) -> String {
    let mut out = text.to_string();
    for root in roots {
        if root.len() > 1 {
            out = out.replace(root, &alias(machine_id, root).to_string_lossy());
        }
    }
    out
}

fn fs(
    path: &Path,
    request: impl FnOnce(String) -> FsRequest,
) -> Result<DevelopmentResponse, String> {
    let remote = resolve(path).ok_or("Not a remote path")?;
    client(&remote.machine_id)?.development(DevelopmentRequest::Fs(request(remote.path)))
}

pub fn stat(path: &Path) -> Result<FsStat, String> {
    match fs(path, |path| FsRequest::Stat { path })? {
        DevelopmentResponse::Stat(stat) => Ok(stat),
        other => Err(format!("Unexpected response: {other:?}")),
    }
}

pub fn exists(path: &Path) -> bool {
    stat(path).is_ok_and(|s| s.exists)
}

pub fn is_dir(path: &Path) -> bool {
    stat(path).is_ok_and(|s| s.exists && s.directory)
}

pub fn list(path: &Path) -> Result<Vec<FileEntry>, String> {
    match fs(path, |path| FsRequest::List { path })? {
        DevelopmentResponse::Entries(entries) => Ok(entries),
        other => Err(format!("Unexpected response: {other:?}")),
    }
}

pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    match fs(path, |path| FsRequest::Read { path })? {
        DevelopmentResponse::File { data, .. } => Ok(data),
        other => Err(format!("Unexpected response: {other:?}")),
    }
}

pub fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    let data = data.to_vec();
    fs(path, move |path| FsRequest::Write { path, data }).map(|_| ())
}

pub fn create_dir_all(path: &Path) -> Result<(), String> {
    fs(path, |path| FsRequest::CreateDir { path }).map(|_| ())
}

pub fn create_file(path: &Path) -> Result<(), String> {
    fs(path, |path| FsRequest::CreateFile { path }).map(|_| ())
}

pub fn rename(from: &Path, to: &Path) -> Result<(), String> {
    let to = resolve(to)
        .ok_or("Destination is not on the same machine")?
        .path;
    fs(from, move |from| FsRequest::Rename { from, to }).map(|_| ())
}

pub fn copy(from: &Path, to: &Path) -> Result<(), String> {
    let to = resolve(to)
        .ok_or("Destination is not on the same machine")?
        .path;
    fs(from, move |from| FsRequest::Copy { from, to }).map(|_| ())
}

pub fn remove(path: &Path) -> Result<(), String> {
    fs(path, |path| FsRequest::Remove { path }).map(|_| ())
}

/// E2EE phone file serving inside the KK channel (docs/design/e2ee.md §2.1
/// F2, §4.5). The desktop serves its local filesystem directly; there is no
/// mirror. The caller (E3b `companion_host`) enforces the sidebar-project
/// allowlist before calling here, so these helpers only enforce path hygiene,
/// caps and truncation. Symlinks are never followed.
pub mod e2ee {
    use super::*;
    use serde_json::{json, Value};
    use std::path::Path;

    /// Largest `files.open` response body (§4.5: truncated with explicit flag).
    pub const MAX_OPEN_BYTES: usize = 64 * 1024;
    /// Longest `files.search` query.
    pub const MAX_QUERY_BYTES: usize = 256;
    /// RPC list pagination (§4.5: `limit` default 50, max 200).
    pub const DEFAULT_LIMIT: usize = 50;
    pub const MAX_LIMIT: usize = 200;
    /// Hard cap for `files.list` / `files.search` entries (mobile readers cap
    /// at 2048; the RPC limit caps first).
    pub const MAX_ENTRIES: usize = 2048;
    /// Deepest directory descent for `files.search`.
    pub const MAX_SEARCH_DEPTH: usize = 6;
    /// Most directories visited in one `files.search`.
    pub const MAX_SEARCH_DIRS: usize = 256;

    /// Normalise an RPC `limit`: default 50, max 200, `0` means default.
    pub fn normalise_limit(limit: Option<u64>) -> Result<usize, &'static str> {
        match limit {
            None | Some(0) => Ok(DEFAULT_LIMIT),
            Some(n) if n <= MAX_LIMIT as u64 => Ok(n as usize),
            Some(_) => Err("invalid limit"),
        }
    }

    /// Path hygiene for a phone-supplied absolute path: must start with `/`,
    /// contain no NUL, and already be canonical (no `.`, `..`, `//`, trailing
    /// `/` except root). Returns the cleaned path on success.
    pub fn check_absolute(path: &str) -> Result<String, &'static str> {
        if path.is_empty() || !path.starts_with('/') || path.contains('\0') {
            return Err("invalid path");
        }
        if path.len() > 4096 {
            return Err("invalid path");
        }
        if path != "/" && path.ends_with('/') {
            return Err("invalid path");
        }
        if path.contains("//") {
            return Err("invalid path");
        }
        for part in path.split('/').skip(1) {
            if part.is_empty() || part == "." || part == ".." {
                return Err("invalid path");
            }
            if part.contains('\\') || part.contains(':') {
                return Err("invalid path");
            }
        }
        Ok(path.to_string())
    }

    /// Whether `path` sits under one of the allowlisted project roots (equal
    /// or descendant). The caller supplies the sidebar allowlist.
    pub fn under_allowlist(path: &str, allowlist: &std::collections::HashSet<String>) -> bool {
        allowlist.iter().any(|root| {
            path == root || (path.starts_with(root.as_str()) && path[root.len()..].starts_with('/'))
        })
    }

    fn entry_kind(file_type: &std::fs::FileType) -> Option<&'static str> {
        if file_type.is_dir() {
            Some("directory")
        } else if file_type.is_file() {
            Some("file")
        } else {
            None
        }
    }

    /// `files.list {path, limit?}` → `{entries: [{name, kind}]}`. Dotfiles are
    /// included verbatim; symlinks are never followed (symlink entries are
    /// skipped).
    pub fn list(path: &str, limit: Option<u64>) -> Result<Value, &'static str> {
        let take = normalise_limit(limit)?;
        list_up_to(path, take)
    }

    /// The whole directory (up to [`MAX_ENTRIES`]), for a file browser.
    pub fn list_all(path: &str) -> Result<Value, &'static str> {
        list_up_to(path, MAX_ENTRIES)
    }

    fn list_up_to(path: &str, take: usize) -> Result<Value, &'static str> {
        let clean = check_absolute(path)?;
        let dir = Path::new(&clean);
        let read = std::fs::read_dir(dir).map_err(|_| "unavailable")?;
        let mut entries = Vec::new();
        for entry in read {
            let Ok(entry) = entry else { continue };
            let Ok(ft) = entry.file_type() else { continue };
            // Never follow symlinks: `file_type` here does not follow, and
            // symlink entries have neither `is_dir` nor `is_file`.
            let Some(kind) = entry_kind(&ft) else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name.len() > 1024 || name.contains('/') || name.contains('\0') {
                continue;
            }
            entries.push(json!({"name": name, "kind": kind}));
            // Read past `take` so the names kept are the first in order,
            // not whichever the disk listed first.
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
        entries.sort_by(|a, b| {
            a["name"].as_str().cmp(&b["name"].as_str())
        });
        entries.truncate(take.min(MAX_ENTRIES));
        Ok(json!({"entries": entries}))
    }

    /// `files.search {query, limit?}` under `base` (one allowlisted root the
    /// caller chose) → `{entries: [{name, kind, path}]}`. Bounded substring
    /// match over entry names and full paths (case-insensitive), like the old
    /// mirror. No symlink following, bounded depth and dir count.
    pub fn search(base: &str, query: &str, limit: Option<u64>) -> Result<Value, &'static str> {
        let base = check_absolute(base)?;
        let q = query.trim();
        if q.is_empty() || q.len() > MAX_QUERY_BYTES {
            return Err("invalid query");
        }
        let take = normalise_limit(limit)?;
        let folded = q.to_lowercase();
        let mut hits = Vec::new();
        let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(Path::new(&base).to_path_buf(), 0)];
        let mut visited = 0usize;
        while let Some((dir, depth)) = stack.pop() {
            if visited >= MAX_SEARCH_DIRS {
                break;
            }
            visited += 1;
            if depth > MAX_SEARCH_DEPTH {
                continue;
            }
            let Ok(read) = std::fs::read_dir(&dir) else { continue };
            for entry in read {
                let Ok(entry) = entry else { continue };
                let Ok(ft) = entry.file_type() else { continue };
                let Some(kind) = entry_kind(&ft) else { continue };
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.is_empty() || name.len() > 1024 {
                    continue;
                }
                let full = format!("{}/{}", dir.to_string_lossy().trim_end_matches('/'), name);
                if name.to_lowercase().contains(&folded) || full.to_lowercase().contains(&folded) {
                    hits.push(json!({"name": name, "kind": kind, "path": full}));
                    if hits.len() >= take.min(MAX_ENTRIES) {
                        return Ok(json!({"entries": hits}));
                    }
                }
                // Dependency and build folders would use up the whole budget.
                let heavy = matches!(name.as_str(), ".git" | "node_modules" | "target" | ".next" | "Pods" | ".venv");
                if kind == "directory" && !heavy {
                    stack.push((dir.join(&name), depth + 1));
                }
            }
        }
        Ok(json!({"entries": hits}))
    }

    /// `files.open {path}` → `{content, truncated}` (text only, capped at
    /// 64 KiB with an explicit flag). Binary (non-UTF-8) reads are
    /// unavailable, never truncated silently.
    /// Largest image `image` sends (it travels base64 in one end-to-end
    /// message of at most 1 MiB).
    pub const MAX_IMAGE_BYTES: u64 = 700 * 1024;

    /// The image types a phone can show, by extension.
    pub fn image_mime(path: &str) -> Option<&'static str> {
        let lower = path.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
        Some(match ext {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return None,
        })
    }

    /// `files.image {path}`: an image file's bytes (base64) for the phone to
    /// show; `too_large` with its size past [`MAX_IMAGE_BYTES`].
    pub fn image(path: &str) -> Result<Value, &'static str> {
        use base64::Engine as _;
        let clean = check_absolute(path)?;
        let mime = image_mime(&clean).ok_or("invalid")?;
        let meta = std::fs::symlink_metadata(&clean).map_err(|_| "unavailable")?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("unavailable");
        }
        if meta.len() > MAX_IMAGE_BYTES {
            return Ok(json!({"mime": mime, "too_large": true, "size": meta.len(), "data": null}));
        }
        let bytes = std::fs::read(&clean).map_err(|_| "unavailable")?;
        Ok(json!({"mime": mime, "too_large": false, "size": bytes.len(),
            "data": base64::engine::general_purpose::STANDARD.encode(&bytes)}))
    }

    pub fn open(path: &str) -> Result<Value, &'static str> {
        let clean = check_absolute(path)?;
        // Never follow a trailing symlink: the final component must not be one.
        let meta = std::fs::symlink_metadata(&clean).map_err(|_| "unavailable")?;
        if meta.file_type().is_symlink() {
            return Err("unavailable");
        }
        if !meta.is_file() {
            return Err("unavailable");
        }
        let bytes = std::fs::read(&clean).map_err(|_| "unavailable")?;
        let (content, truncated) = if bytes.len() > MAX_OPEN_BYTES {
            let cut = String::from_utf8_lossy(&bytes[..MAX_OPEN_BYTES]).into_owned();
            (cut, true)
        } else {
            let text = String::from_utf8(bytes).map_err(|_| "unavailable")?;
            (text, false)
        };
        if content.len() > 4 * 1024 * 1024 {
            return Err("unavailable");
        }
        Ok(json!({"content": content, "truncated": truncated}))
    }

    /// Largest file a phone can page through with [`open_at`].
    pub const MAX_PAGED_BYTES: u64 = 8 * 1024 * 1024;

    /// `files.open {path, offset}` → one page of a text file:
    /// `{content, truncated, size, offset, next_offset}`. Pages end on a
    /// character boundary; `next_offset` is null on the last page. Binary
    /// files and files over [`MAX_PAGED_BYTES`] are unavailable.
    pub fn open_at(path: &str, offset: u64) -> Result<Value, &'static str> {
        use std::io::{Read, Seek, SeekFrom};
        let clean = check_absolute(path)?;
        let meta = std::fs::symlink_metadata(&clean).map_err(|_| "unavailable")?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("unavailable");
        }
        let size = meta.len();
        if size > MAX_PAGED_BYTES || offset > size {
            return Err("unavailable");
        }
        let mut file = std::fs::File::open(&clean).map_err(|_| "unavailable")?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| "unavailable")?;
        let mut bytes = Vec::with_capacity(MAX_OPEN_BYTES + 4);
        file.take(MAX_OPEN_BYTES as u64 + 4)
            .read_to_end(&mut bytes)
            .map_err(|_| "unavailable")?;
        if bytes.contains(&0) {
            return Err("unavailable");
        }
        let last = offset + bytes.len() as u64 >= size && bytes.len() <= MAX_OPEN_BYTES;
        let mut end = bytes.len().min(MAX_OPEN_BYTES);
        if !last {
            // Step back to a character boundary so no page splits one.
            while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
                end -= 1;
            }
        }
        let content = std::str::from_utf8(&bytes[..end]).map_err(|_| "unavailable")?;
        let next = offset + end as u64;
        Ok(json!({"content": content, "truncated": !last, "size": size, "offset": offset,
            "next_offset": if last { Value::Null } else { json!(next) }}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_file_is_read_a_page_at_a_time_without_splitting_characters() {
        let dir = std::env::temp_dir().join(format!("dragent-open-at-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.txt");
        // Three-byte characters straddle every page boundary.
        let text = "€".repeat(e2ee::MAX_OPEN_BYTES);
        std::fs::write(&path, &text).unwrap();
        let path = path.to_string_lossy().to_string();
        let mut offset = 0u64;
        let mut read = String::new();
        let mut pages = 0;
        loop {
            let page = e2ee::open_at(&path, offset).unwrap();
            read.push_str(page["content"].as_str().unwrap());
            assert_eq!(page["size"].as_u64().unwrap(), text.len() as u64);
            pages += 1;
            match page["next_offset"].as_u64() {
                Some(next) => {
                    assert_eq!(page["truncated"], true);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(pages, 4);
        assert_eq!(read, text);
        std::fs::write(dir.join("bin"), [0u8, 1, 2]).unwrap();
        assert!(e2ee::open_at(&dir.join("bin").to_string_lossy(), 0).is_err(), "binary files are not text");
        assert!(e2ee::open_at(&path, text.len() as u64 + 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgetting_drops_remembered_remote_answers() {
        let head = alias("m-forget", "/srv/app/.git/HEAD");
        TEXT_CACHE.lock().unwrap().insert(
            head.clone(),
            (std::time::Instant::now(), Some("ref: refs/heads/old\n".into())),
        );
        ROOT_CACHE
            .lock()
            .unwrap()
            .insert(head.clone(), (std::time::Instant::now(), None));
        forget_cached();
        assert!(!TEXT_CACHE.lock().unwrap().contains_key(&head));
        assert!(!ROOT_CACHE.lock().unwrap().contains_key(&head));
    }

    #[test]
    fn stand_in_paths_round_trip() {
        let root = alias("m1", "/home/dev/app");
        assert_eq!(root, mounts_root().join("m1/home/dev/app"));
        let resolved = resolve(&root.join("src/main.rs")).unwrap();
        assert_eq!(resolved.machine_id, "m1");
        assert_eq!(resolved.path, "/home/dev/app/src/main.rs");
        assert_eq!(resolve(&mounts_root().join("m1")).unwrap().path, "/");
        assert!(resolve(Path::new("/home/dev/app")).is_none());
    }

    #[test]
    fn arguments_and_reported_paths_translate() {
        let root = alias("m1", "/srv/app");
        assert_eq!(translate_arg("m1", &root.to_string_lossy()), "/srv/app");
        assert_eq!(translate_arg("m1", "--porcelain"), "--porcelain");
        assert_eq!(
            alias_remote_path("m1", Path::new("/srv/app/src")),
            root.join("src")
        );
        assert_eq!(alias_remote_path("m1", &root), root);
    }
}

#[cfg(test)]
mod image_tests {
    use super::e2ee::{image, image_mime, MAX_IMAGE_BYTES};

    #[test]
    fn images_come_back_base64_and_big_ones_say_so() {
        use base64::Engine as _;
        let dir = std::env::temp_dir().join(format!("dragent-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("logo.png");
        std::fs::write(&small, [0x89, b'P', b'N', b'G', 1, 2, 3]).unwrap();
        let v = image(&small.to_string_lossy()).unwrap();
        assert_eq!(v["mime"], "image/png");
        assert_eq!(v["too_large"], false);
        let data = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap();
        assert_eq!(data, vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
        let big = dir.join("photo.jpg");
        std::fs::write(&big, vec![0u8; (MAX_IMAGE_BYTES + 1) as usize]).unwrap();
        let v = image(&big.to_string_lossy()).unwrap();
        assert_eq!((v["too_large"].as_bool(), v["data"].is_null()), (Some(true), true));
        assert!(image(&dir.join("notes.txt").to_string_lossy()).is_err(), "only images");
        assert_eq!(image_mime("A.JPEG"), Some("image/jpeg"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
