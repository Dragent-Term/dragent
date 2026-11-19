//! Shared, authenticated local service owning the heavy code-index engine.

use anyhow::{Context, Result};
use dragent_index_protocol::{
    PROTOCOL_VERSION, RepoIndexState, RepoIndexStatus, Request, RequestEnvelope, Response,
    ResponseEnvelope, SERVICE_ARG, SearchResult, ServiceEndpoint,
};
use rand::RngCore;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_REQUEST_BYTES: u64 = 2 * 1024 * 1024;
const CLIENT_LEASE_TIMEOUT: Duration = Duration::from_secs(90);
const STARTUP_GRACE: Duration = Duration::from_secs(10);

/// Run the service when the hidden service argument is present. Returns true
/// when the caller must exit instead of starting the graphical application.
pub fn run_if_requested() -> Result<bool> {
    if !std::env::args().any(|arg| arg == SERVICE_ARG) {
        return Ok(false);
    }
    run_service()?;
    Ok(true)
}

/// Embedding inference uses every core it is given. On a machine without an
/// NPU/GPU that starves the user's shells and UI, so the indexer only gets
/// time the interactive processes leave over.
fn lower_own_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
        };
        // SAFETY: the current-process pseudo handle needs no cleanup.
        if unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) } == 0 {
            log::warn!("[index] could not lower service priority");
        }
    }
    #[cfg(unix)]
    // SAFETY: nice() only adjusts this process's scheduling priority.
    unsafe {
        libc::nice(10);
    }
}

fn run_service() -> Result<()> {
    // One service per Dragent home. The lock lasts as long as this process;
    // the system drops it if the process dies.
    let Some(_instance) = claim_instance(&dragent_paths::home().join("index-service.lock"))?
    else {
        // Another concurrently launched desktop won the service race.
        return Ok(());
    };
    lower_own_priority();
    // Any free loopback port, published in the endpoint file. A fixed port
    // sat in the range the system hands out to other programs, and once one
    // of them held it the service could never start again.
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;

    let token = generate_token();
    write_endpoint(&ServiceEndpoint {
        version: PROTOCOL_VERSION,
        port,
        token: token.clone(),
        pid: std::process::id(),
    })?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let clients = Arc::new(Mutex::new(HashMap::<String, Instant>::new()));
    let started = Instant::now();
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let token = token.clone();
                let shutdown = shutdown.clone();
                let clients = clients.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, &token, &shutdown, &clients) {
                        log::debug!("[index-service] request failed: {error:#}");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.into()),
        }
        if started.elapsed() >= STARTUP_GRACE {
            let mut clients = clients.lock().unwrap();
            clients.retain(|_, seen| seen.elapsed() < CLIENT_LEASE_TIMEOUT);
            if clients.is_empty() {
                log::info!("[index-service] no live desktop clients — exiting");
                break;
            }
        }
    }
    remove_owned_endpoint(&token);
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    expected_token: &str,
    shutdown: &AtomicBool,
    clients: &Mutex<HashMap<String, Instant>>,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut line = String::new();
    BufReader::new(stream.try_clone()?)
        .take(MAX_REQUEST_BYTES)
        .read_line(&mut line)?;
    let envelope: RequestEnvelope = serde_json::from_str(&line).context("decode request")?;

    let result = if envelope.token != expected_token {
        Err("unauthorized".to_string())
    } else if matches!(envelope.request, Request::Shutdown) {
        // Allow an authenticated newer client to retire an old protocol
        // service so an application update cannot strand the fixed port.
        shutdown.store(true, Ordering::Release);
        Ok(Response::Ack)
    } else if envelope.version != PROTOCOL_VERSION {
        Err(format!(
            "protocol mismatch: client={} service={PROTOCOL_VERSION}",
            envelope.version
        ))
    } else {
        clients
            .lock()
            .unwrap()
            .insert(envelope.client_id, Instant::now());
        dispatch(envelope.request)
    };

    let response = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        result,
    };
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn dispatch(request: Request) -> Result<Response, String> {
    let manager = dragent_index_engine::get_manager();
    match request {
        Request::Ping => Ok(Response::Pong),
        Request::Heartbeat => Ok(Response::Ack),
        Request::SetEnabled(on) => {
            manager.set_enabled(on);
            Ok(Response::Ack)
        }
        Request::SetAutoIndex(on) => {
            manager.set_auto_index(on);
            Ok(Response::Ack)
        }
        Request::SetRespectGitignore(respect) => {
            manager.set_respect_gitignore(respect);
            Ok(Response::Ack)
        }
        Request::RegisterRepo { root, max_files } => {
            manager.register_repo(root, max_files);
            Ok(Response::Ack)
        }
        Request::Reindex { root } => {
            manager.reindex(&root);
            Ok(Response::Ack)
        }
        Request::RemoveIndex { root } => {
            manager.remove_index(&root);
            Ok(Response::Ack)
        }
        Request::ReindexAll => {
            manager.reindex_all();
            Ok(Response::Ack)
        }
        Request::IndexStatus => Ok(Response::IndexStatus(
            manager
                .index_status()
                .into_iter()
                .map(|status| RepoIndexStatus {
                    root: status.root,
                    state: match status.state {
                        dragent_index_engine::RepoIndexState::Indexing => RepoIndexState::Indexing,
                        dragent_index_engine::RepoIndexState::Watching => RepoIndexState::Watching,
                        dragent_index_engine::RepoIndexState::Paused => RepoIndexState::Paused,
                    },
                    pending: status.pending,
                    files: status.files,
                    chunks: status.chunks,
                    capped: status.capped,
                    synced_secs_ago: status.synced_secs_ago,
                })
                .collect(),
        )),
        Request::Search {
            repo_root,
            query,
            top_k,
        } => Ok(Response::Search(
            manager
                .search(&repo_root, &query, top_k)
                .into_iter()
                .map(|result| SearchResult {
                    path: result.path,
                    byte_start: result.byte_start,
                    byte_end: result.byte_end,
                    text: result.text,
                    score: result.score,
                })
                .collect(),
        )),
        Request::HybridSearch {
            repo_root,
            query,
            top_k,
        } => Ok(Response::HybridSearch(
            manager
                .hybrid_search(&repo_root, &query, top_k)
                .into_iter()
                .map(|hit| dragent_index_protocol::HybridResult {
                    path: hit.path,
                    byte_start: hit.byte_start,
                    byte_end: hit.byte_end,
                    text: hit.text,
                    vector_score: hit.vector_score,
                    bm25_score: hit.bm25_score,
                    fused_score: hit.fused_score,
                    matched_symbol: hit.matched_symbol,
                })
                .collect(),
        )),
        Request::ListIndexedPaths { repo_root } => Ok(Response::IndexedPaths(
            manager.list_indexed_paths(&repo_root),
        )),
        Request::IsReady => Ok(Response::Ready(manager.is_ready())),
        Request::Shutdown => unreachable!("shutdown handled before dispatch"),
    }
}

/// The lock that makes this the only index service, or `None` when another
/// one already holds it.
fn claim_instance(path: &std::path::Path) -> Result<Option<std::fs::File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn endpoint_path() -> Result<PathBuf> {
    Ok(dragent_paths::home().join("index-service-v1.json"))
}

fn write_endpoint(endpoint: &ServiceEndpoint) -> Result<()> {
    let path = endpoint_path()?;
    let parent = path.parent().context("endpoint has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".index-service-{}.tmp", endpoint.pid));
    let bytes = serde_json::to_vec(endpoint)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&temporary, &bytes)?;

    std::fs::rename(temporary, path)?;
    Ok(())
}

fn remove_owned_endpoint(token: &str) {
    let Ok(path) = endpoint_path() else {
        return;
    };
    let owned = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ServiceEndpoint>(&bytes).ok())
        .is_some_and(|endpoint| endpoint.token == token);
    if owned {
        let _ = std::fs::remove_file(path);
    }
}

fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_service_holds_the_instance_lock() {
        let dir = std::env::temp_dir().join(format!("dragent-index-lock-{}", generate_token()));
        let lock = dir.join("index-service.lock");
        let first = claim_instance(&lock).unwrap();
        assert!(first.is_some());
        assert!(claim_instance(&lock).unwrap().is_none(), "a second service must stand down");
        drop(first);
        assert!(claim_instance(&lock).unwrap().is_some(), "the lock frees when its holder exits");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tokens_are_random_and_256_bits() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }
}
