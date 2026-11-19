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
