use std::sync::atomic::Ordering;
use std::time::Duration;

use dragent_resource_lifecycle::ResourceLease;

use super::embedder::Embedder;
use super::manager::set_background_qos;
use crate::CodeIndexManager;
use crate::index_batch::{model_idle_timeout, repo_idle_timeout};
use crate::index_watcher::MODEL_REAPER_INTERVAL;

// ── Thread priority ───────────────────────────────────────────────────────────

/// Drop the calling thread to background (Utility) scheduling so indexing —
/// walking, chunking, and especially ONNX embedding — never competes with the
/// UI thread for cores. On Apple platforms the ONNX Runtime's intra-op pool
/// inherits the QoS of the thread that creates the session, so calling this
/// before `Embedder::load` covers inference threads too. The terminal must
/// never be the reason the app stutters.

// ── Global static ─────────────────────────────────────────────────────────────

/// Default per-repo file budget, used when a parked/unknown root has to be
/// re-registered without its original budget.

impl CodeIndexManager {
    /// Download (if needed) and load the embedding model in the background.
    /// Safe to call multiple times — subsequent calls are no-ops.
    pub fn init_embedder_background(&'static self) {
        self.start_model_reaper();
        if self.embedder.is_ready() || self.embedder.is_loading() {
            return;
        }
        std::thread::spawn(move || {
            set_background_qos();
            // `acquire_embedder` owns load/ready/failure logging so concurrent
            // callers still produce one coherent lifecycle event.
            drop(self.acquire_embedder(Duration::from_secs(60)));
        });
    }

    pub(crate) fn start_model_reaper(&'static self) {
        if self.model_reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        std::thread::spawn(move || {
            set_background_qos();
            loop {
                std::thread::sleep(MODEL_REAPER_INTERVAL);
                let mut released = self.embedder.evict_if_idle(model_idle_timeout());
                if released {
                    log::info!("[index] released idle embedding model");
                }
                released |= self.release_idle_repos(repo_idle_timeout()) > 0;
                if released {
                    return_freed_memory();
                }
            }
        });
    }

    pub(crate) fn acquire_embedder(&self, timeout: Duration) -> Option<ResourceLease<Embedder>> {
        // catch_unwind: ort panics (rather than erroring) when the ONNX Runtime
        // library is incompatible. Degrade to an unavailable index rather than
        // killing the worker thread.
        match self.embedder.acquire_with(timeout, || {
            log::info!("[index] loading embedding model on demand…");
            match std::panic::catch_unwind(Embedder::load) {
                Ok(Ok(embedder)) => {
                    log::info!("[index] embedding model ready");
                    Ok(embedder)
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    Err(format!("onnxruntime panic: {message}"))
                }
            }
        }) {
            Ok(lease) => Some(lease),
            Err(error) => {
                log::warn!("[index] embedding model unavailable: {error}");
                None
            }
        }
    }
}

/// macOS keeps freed pages charged to the process until asked for them, so the
/// model and repositories we just dropped would still count against it.
#[cfg(target_os = "macos")]
fn return_freed_memory() {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: a null zone means all zones and a goal of 0 means as much as possible.
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn return_freed_memory() {}
