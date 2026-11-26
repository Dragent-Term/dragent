//! Fire-and-forget background work.
//!
//! `std::thread::spawn` is unsupported on wasm32 (`operation not supported on
//! this platform`) and panics at runtime, aborting the module. The web viewer
//! never runs this background work (command execution, HTTP posts, watchers are
//! all gated), so on wasm `detach` simply drops the closure. On native it's a
//! detached thread, exactly as before.
pub fn detach<F: FnOnce() + Send + 'static>(f: F) {
    #[cfg(not(target_family = "wasm"))]
    {
        std::thread::spawn(f);
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = f;
    }
}

/// Run blocking work (host RPCs, file system scans) on its own thread and
/// await its result. View and model `spawn` futures run on a small shared
/// async pool, the same one that drives timers and every other background
/// follow-up; a blocking call made inside one of those futures holds a pool
/// thread for as long as it blocks. At launch, re-attaching a few dozen
/// restored terminals to a slow host used to occupy the whole pool that way,
/// and everything else waited behind it.
///
/// A worker that stops without answering (a panic) reports an error.
pub fn run_result<T, F>(
    f: F,
) -> impl std::future::Future<Output = Result<T, String>> + Send + 'static
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    detach(move || {
        let _ = tx.send(f());
    });
    async move {
        rx.await
            .unwrap_or_else(|_| Err("background worker stopped".to_string()))
    }
}
