//! Host discovery: endpoint file, spawn lock, lazy spawn-or-connect.
//!
//! Adapted from the index-service pattern with two deliberate changes:
//! an ephemeral port chosen by the service (never hardcoded) and an
//! exclusive lock file so two racing spawners cannot create two authorities.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use super::protocol::{
    ENDPOINT_FILE_ENV, ENDPOINT_FILENAME, PROTOCOL_VERSION, Request, RequestEnvelope, Response,
    ResponseEnvelope, ServiceEndpoint,
};

pub fn endpoint_path() -> PathBuf {
    if let Ok(path) = std::env::var(ENDPOINT_FILE_ENV) {
        return PathBuf::from(path);
    }
    dragent_paths::organisation_dir().join(ENDPOINT_FILENAME)
}

fn lock_path() -> PathBuf {
    let mut path = endpoint_path();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("host-v1.json");
    path.set_file_name(format!("{name}.lock"));
    path
}

pub fn read_endpoint() -> Option<ServiceEndpoint> {
    let raw = fs::read_to_string(endpoint_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_endpoint(endpoint: &ServiceEndpoint) -> std::io::Result<()> {
    let path = endpoint_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    serde_json::to_writer(&file, endpoint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    file.write_all(b"\n")?;
    drop(file);
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the endpoint file only if it still describes our token (never
/// clobber a successor that already replaced it).
pub fn remove_owned_endpoint(token: &str) {
    if read_endpoint().map(|e| e.token) == Some(token.to_string()) {
        let _ = fs::remove_file(endpoint_path());
    }
}

fn rpc_once(
    endpoint: &ServiceEndpoint,
    client_id: &str,
    request: Request,
    read_timeout: Duration,
) -> Result<Response, String> {
    let stream = super::transport::Stream::connect(&endpoint.address(), Duration::from_millis(350))
        .map_err(|e| format!("connect: {e}"))?;
    let token = endpoint.token.as_str();
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|e| format!("{e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("{e}"))?;
    let envelope = RequestEnvelope {
        version: PROTOCOL_VERSION,
        token: token.to_string(),
        client_id: client_id.to_string(),
        request,
    };
    super::protocol::write_message(&mut &stream, &envelope).map_err(|e| format!("write: {e}"))?;
    let env: ResponseEnvelope =
        super::protocol::read_message(&mut &stream).map_err(|e| format!("read: {e}"))?;
    if env.version != PROTOCOL_VERSION {
        return Err(format!(
            "protocol mismatch (host {}, client {})",
            env.version, PROTOCOL_VERSION
        ));
    }
    env.result
}

/// How long a health ping waits for the answer. A live host answers in
/// milliseconds; a hung one used to hold every caller for 10 s, one after
/// another behind the client's start lock (one per restored terminal at
/// launch).
pub const PING_TIMEOUT: Duration = Duration::from_secs(3);

pub fn ping(endpoint: &ServiceEndpoint, client_id: &str) -> bool {
    matches!(
        rpc_once(endpoint, client_id, Request::Ping, PING_TIMEOUT),
        Ok(Response::Pong)
    )
}

pub fn rpc(
    endpoint: &ServiceEndpoint,
    client_id: &str,
    request: Request,
) -> Result<Response, String> {
    rpc_once(endpoint, client_id, request, Duration::from_secs(10))
}

/// [`rpc`] for requests that can legitimately run long (git network ops).
pub fn rpc_with_timeout(
    endpoint: &ServiceEndpoint,
    client_id: &str,
    request: Request,
    read_timeout: Duration,
) -> Result<Response, String> {
    rpc_once(endpoint, client_id, request, read_timeout)
}

/// Held for the entire host lifetime. The OS releases the lock if the
/// process dies, so a stale lock file can never block recovery.
pub struct HostAuthorityGuard {
    _file: fs::File,
}

/// Take the single-host authority lock, or `None` when another live host
/// holds it. A real OS lock on every platform (`flock` on macOS/Linux,
/// `LockFileEx` on Windows, via `File::try_lock`): the lock file itself is
/// never meaningful, only who holds the lock on it.
pub fn acquire_host_authority() -> std::io::Result<Option<HostAuthorityGuard>> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    match file.try_lock() {
        Ok(()) => Ok(Some(HostAuthorityGuard { _file: file })),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

/// Ask the host to release its authority while leaving every PTY alive.
/// Old builds without this frozen handshake refuse without shutting down.
pub fn restart_for_update(endpoint: &ServiceEndpoint) -> Result<(), String> {
    let stream = super::transport::Stream::connect(&endpoint.address(), Duration::from_millis(350))
        .map_err(|e| format!("connect: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut payload = super::protocol::HOST_UPDATE_MAGIC.to_vec();
    payload.extend_from_slice(endpoint.token.as_bytes());
    super::protocol::write_frame(&mut &stream, &payload).map_err(|e| e.to_string())?;
    let reply = super::protocol::read_frame(&mut &stream).map_err(|e| e.to_string())?;
    if reply == b"OK" {
        Ok(())
    } else {
        Err("host declined safe update".into())
    }
}
