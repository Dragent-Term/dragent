//! Lightweight app-facing client for the shared code-index service.
//!
//! Keeping this facade separate means desktop and WASM consumers do not own an
//! ONNX session, SQLite connection, or filesystem watcher. Native calls cross a
//! versioned authenticated loopback channel; WASM retains the existing no-op
//! behavior.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use dragent_index_protocol::{HybridResult, RepoIndexState, RepoIndexStatus, SearchResult};

pub struct CodeIndexManager {
    enabled: AtomicBool,
    auto_index: AtomicBool,
    respect_gitignore: AtomicBool,
    repos: Mutex<Vec<(PathBuf, usize)>>,
    path_cache: Mutex<Vec<PathCacheEntry>>,
}

struct PathCacheEntry {
    repo_root: PathBuf,
    value: Option<(PathBuf, Vec<PathBuf>)>,
    cached_at: Instant,
}

impl CodeIndexManager {
    const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            auto_index: AtomicBool::new(true),
            respect_gitignore: AtomicBool::new(true),
            repos: Mutex::new(Vec::new()),
            path_cache: Mutex::new(Vec::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        native::send_ack(dragent_index_protocol::Request::SetEnabled(on));
    }

    pub fn auto_index(&self) -> bool {
        self.auto_index.load(Ordering::Relaxed)
    }

    pub fn set_auto_index(&self, on: bool) {
        self.auto_index.store(on, Ordering::Relaxed);
        native::send_ack(dragent_index_protocol::Request::SetAutoIndex(on));
    }

    /// Kept for API compatibility. Normal startup is repository-demand driven.
    pub fn init_embedder_background(&self) {}

    pub fn set_respect_gitignore(&self, respect: bool) {
        self.respect_gitignore.store(respect, Ordering::Relaxed);
        native::send_ack(dragent_index_protocol::Request::SetRespectGitignore(
            respect,
        ));
    }

    pub fn reindex(&self, root: &Path) {
        self.invalidate_path_cache(root);
        native::send_ack(dragent_index_protocol::Request::Reindex {
            root: root.to_path_buf(),
        });
    }

    pub fn remove_index(&self, root: &Path) {
        self.invalidate_path_cache(root);
        self.repos
            .lock()
            .unwrap()
            .retain(|(known, _)| known != root);
        native::send_ack(dragent_index_protocol::Request::RemoveIndex {
            root: root.to_path_buf(),
        });
    }

    pub fn reindex_all(&self) {
        self.path_cache.lock().unwrap().clear();
        native::send_ack(dragent_index_protocol::Request::ReindexAll);
    }

    pub fn register_repo(&self, root: PathBuf, max_files: usize) {
        if std::env::var_os("DRAGENT_INDEX_DISABLE").is_some() {
            return;
        }
        {
            let mut repos = self.repos.lock().unwrap();
            if let Some((_, budget)) = repos.iter_mut().find(|(known, _)| *known == root) {
                *budget = max_files;
            } else {
                repos.push((root.clone(), max_files));
            }
        }
        self.invalidate_path_cache(&root);
        // The service parks registrations while disabled and restores them on
        // enable, matching the former in-process manager behavior.
        native::send_ack(dragent_index_protocol::Request::RegisterRepo { root, max_files });
    }

    pub fn index_status(&self) -> Vec<RepoIndexStatus> {
        match native::request(dragent_index_protocol::Request::IndexStatus) {
            Some(dragent_index_protocol::Response::IndexStatus(statuses)) => statuses,
            _ => Vec::new(),
        }
    }

    pub fn search(&self, repo_root: &Path, query: &str, top_k: usize) -> Vec<SearchResult> {
        match native::request(dragent_index_protocol::Request::Search {
            repo_root: repo_root.to_path_buf(),
            query: query.to_string(),
            top_k,
        }) {
            Some(dragent_index_protocol::Response::Search(results)) => results,
            _ => Vec::new(),
        }
    }

    pub fn hybrid_search(&self, repo_root: &Path, query: &str, top_k: usize) -> Vec<HybridResult> {
        match native::request(dragent_index_protocol::Request::HybridSearch {
            repo_root: repo_root.to_path_buf(),
            query: query.to_string(),
            top_k,
        }) {
            Some(dragent_index_protocol::Response::HybridSearch(results)) => results,
            _ => Vec::new(),
        }
    }

    pub fn list_indexed_paths(&self, repo_root: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
        const PATH_CACHE_TTL: Duration = Duration::from_secs(2);
        if let Some(value) = self
            .path_cache
            .lock()
            .unwrap()
            .iter()
            .find(|entry| {
                entry.repo_root == repo_root && entry.cached_at.elapsed() < PATH_CACHE_TTL
            })
            .map(|entry| entry.value.clone())
        {
            return value;
        }

        let value = match native::request(dragent_index_protocol::Request::ListIndexedPaths {
            repo_root: repo_root.to_path_buf(),
        }) {
            Some(dragent_index_protocol::Response::IndexedPaths(paths)) => paths,
            _ => None,
        };
        let mut cache = self.path_cache.lock().unwrap();
        cache.retain(|entry| entry.repo_root != repo_root);
        cache.push(PathCacheEntry {
            repo_root: repo_root.to_path_buf(),
            value: value.clone(),
            cached_at: Instant::now(),
        });
        value
    }

    pub fn is_ready(&self) -> bool {
        matches!(
            native::request(dragent_index_protocol::Request::IsReady),
            Some(dragent_index_protocol::Response::Ready(true))
        )
    }

    fn invalidate_path_cache(&self, root: &Path) {
        self.path_cache.lock().unwrap().retain(|entry| {
            !entry.repo_root.starts_with(root) && !root.starts_with(&entry.repo_root)
        });
    }
}

static CODE_INDEX_MANAGER: CodeIndexManager = CodeIndexManager::new();

pub fn get_manager() -> &'static CodeIndexManager {
    &CODE_INDEX_MANAGER
}

pub mod walk {
    use std::path::PathBuf;

    pub fn global_ignore_path() -> Option<PathBuf> {
        Some(dragent_paths::home().join("index.ignore"))
    }
}

#[cfg(all(not(target_family = "wasm"), feature = "service-client"))]
mod native {
    use dragent_index_protocol::{
        PROTOCOL_VERSION, Request, RequestEnvelope, Response, ResponseEnvelope, SERVICE_ARG,
        ServiceEndpoint,
    };
    use std::io::{BufRead, BufReader, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    use std::process::{Command, Stdio};
    use std::sync::atomic::Ordering;
    use std::sync::{Mutex, Once, OnceLock};
    use std::time::{Duration, Instant};

    static START_LOCK: Mutex<()> = Mutex::new(());
    /// When the last attempt to start the service failed. Callers in that
    /// window get an answer at once instead of each waiting for a new start.
    static LAST_FAILED_START: Mutex<Option<Instant>> = Mutex::new(None);
    static HEARTBEAT: Once = Once::new();
    static CLIENT_ID: OnceLock<String> = OnceLock::new();
    const CONNECT_TIMEOUT: Duration = Duration::from_millis(350);
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
    /// Status questions and settings changes are answered at once; only a
    /// search may legitimately take long.
    const QUICK_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
    const RETRY_START_AFTER: Duration = Duration::from_secs(30);

    /// Settings changes and repo registrations don't wait for the service:
    /// they go out in order from one background thread, so a caller on the
    /// UI thread never stalls on a slow or restarting service.
    pub(super) fn send_ack(request: Request) {
        static QUEUE: OnceLock<std::sync::mpsc::Sender<Request>> = OnceLock::new();
        let queue = QUEUE.get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::channel::<Request>();
            let _ = std::thread::Builder::new()
                .name("index-client-acks".into())
                .spawn(move || {
                    for request in receiver {
                        if !matches!(self::request(request), Some(Response::Ack)) {
                            log::warn!(
                                "[index-client] shared index service did not acknowledge request"
                            );
                        }
                    }
                });
            sender
        });
        let _ = queue.send(request);
    }

    pub(super) fn request(request: Request) -> Option<Response> {
        start_heartbeat();
        if let Ok(response) = request_once(request.clone()) {
            return Some(response);
        }
        if let Err(error) = ensure_service() {
            log::warn!("[index-client] shared service unavailable: {error}");
            return None;
        }
        match request_once(request) {
            Ok(response) => Some(response),
            Err(error) => {
                log::warn!("[index-client] request failed: {error}");
                None
            }
        }
    }

    fn ensure_service() -> Result<(), String> {
        let _guard = START_LOCK.lock().unwrap();
        if matches!(request_once(Request::Ping), Ok(Response::Pong)) {
            return Ok(());
        }
        let last_failure = *LAST_FAILED_START.lock().unwrap();
        if last_failure.is_some_and(|at| at.elapsed() < RETRY_START_AFTER) {
            return Err("the service failed to start moments ago; not retrying yet".into());
        }
        let started = start_service();
        *LAST_FAILED_START.lock().unwrap() = started.is_err().then(Instant::now);
        started
    }

    fn start_service() -> Result<(), String> {
        // A service from an older build may still own the port. Shutdown is
        // accepted across protocol versions when its endpoint token matches.
        let _ = request_once(Request::Shutdown);
        std::thread::sleep(Duration::from_millis(80));

        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg(SERVICE_ARG)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut command);
        command.spawn().map_err(|e| e.to_string())?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(request_once(Request::Ping), Ok(Response::Pong)) {
                return restore_desired_state();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("service did not become ready within 5 seconds".into())
    }

    fn restore_desired_state() -> Result<(), String> {
        let manager = super::get_manager();
        expect_ack(request_once(Request::SetAutoIndex(
            manager.auto_index.load(Ordering::Relaxed),
        ))?)?;
        expect_ack(request_once(Request::SetRespectGitignore(
            manager.respect_gitignore.load(Ordering::Relaxed),
        ))?)?;
        expect_ack(request_once(Request::SetEnabled(
            manager.enabled.load(Ordering::Relaxed),
        ))?)?;
        for (root, max_files) in manager.repos.lock().unwrap().clone() {
            expect_ack(request_once(Request::RegisterRepo { root, max_files })?)?;
        }
        Ok(())
    }

    fn expect_ack(response: Response) -> Result<(), String> {
        if matches!(response, Response::Ack) {
            Ok(())
        } else {
            Err("unexpected response while restoring index service state".into())
        }
    }

    fn request_once(request: Request) -> Result<Response, String> {
        let endpoint = read_endpoint()?;
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), endpoint.port);
        let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
            .map_err(|e| format!("connect: {e}"))?;
        let read_timeout = match request {
            Request::Search { .. } | Request::HybridSearch { .. } => REQUEST_TIMEOUT,
            _ => QUICK_REQUEST_TIMEOUT,
        };
        stream
            .set_read_timeout(Some(read_timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;

        let envelope = RequestEnvelope {
            version: PROTOCOL_VERSION,
            token: endpoint.token,
            client_id: client_id().to_string(),
            request,
        };
        serde_json::to_writer(&mut stream, &envelope).map_err(|e| e.to_string())?;
        stream.write_all(b"\n").map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if line.len() > 32 * 1024 * 1024 {
            return Err("service response exceeded 32 MiB".into());
        }
        let response: ResponseEnvelope = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if response.version != PROTOCOL_VERSION {
            return Err(format!(
                "protocol mismatch: client={} service={}",
                PROTOCOL_VERSION, response.version
            ));
        }
        response.result
    }

    fn read_endpoint() -> Result<ServiceEndpoint, String> {
        let path = endpoint_path().ok_or("no config directory")?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    fn endpoint_path() -> Option<std::path::PathBuf> {
        Some(dragent_paths::home().join("index-service-v1.json"))
    }

    fn client_id() -> &'static str {
        CLIENT_ID.get_or_init(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            format!("{}-{nanos:x}", std::process::id())
        })
    }

    fn start_heartbeat() {
        HEARTBEAT.call_once(|| {
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(30));
                    // Use the normal recovery path so an index service killed
                    // by the OS is restored while the desktop remains alive.
                    let _ = request(Request::Heartbeat);
                }
            });
        });
    }

    fn no_window(command: &mut Command) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000 | 0x0000_0008); // NO_WINDOW | DETACHED_PROCESS
        }
        #[cfg(not(windows))]
        let _ = command;
    }

}

#[cfg(any(target_family = "wasm", not(feature = "service-client")))]
mod native {
    use dragent_index_protocol::{Request, Response};

    pub(super) fn send_ack(_request: Request) {}
    pub(super) fn request(_request: Request) -> Option<Response> {
        None
    }
}
