//! Browser entry point for the shared-session web viewer.
//!
//! The web viewer is the Dragent desktop app compiled to WebAssembly and drawn
//! on an HTML `<canvas>` via warpui (winit + wgpu WebGL backend + cosmic-text).
//! It runs the app's **real** `RootView` — the same code the desktop runs — so
//! the terminal UI (blocks, agent cards, alt-screen), the input bar, themes, and
//! chrome are literally the desktop's, not a re-implementation. Native execution
//! (PTY, SQLite, ONNX index, MCP, native HTTP/WebSocket) is `#[cfg(wasm)]`-gated
//! out of the app crate; the viewer feeds live session state over a browser
//! WebSocket (wired separately).

use warpui_core::AssetProvider;

/// The bundled asset provider is shared with the desktop app (icons, theme
/// images, fonts) and is wasm-safe (rust-embed bytes, no native deps), so the
/// viewer reuses it rather than duplicating an asset layer.
pub fn assets() -> Box<dyn AssetProvider> {
    Box::new(dragent_assets::DragentAssets)
}

#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

/// Called once when the wasm module loads. Boots the warpui App on the canvas
/// and adds a window backed by the app's real `RootView`. warpui's wasm
/// windowing attaches the canvas to the element `#wasm-container` (falling back
/// to `<body>`), so the host page only needs that mount point.
#[cfg(target_family = "wasm")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_logger::init(wasm_logger::Config::default());
    log::info!("dragent_web_viewer: booting the real RootView on canvas");

    let app = warpui::platform::AppBuilder::new(
        warpui_core::platform::app::AppCallbacks::default(),
        assets(),
        None,
    );
    // On wasm `run` hands off to the browser event loop and returns immediately;
    // the App keeps running via winit's `spawn`.
    //
    // `new_empty` goes straight to a fresh workspace (onboarding is a desktop
    // concern); the shared-session mirror feeds it remote output once the browser
    // WebSocket transport is wired.
    let _ = app.run(|ctx| {
        // App-level singleton models the real views resolve via `get_singleton_*`
        // — the same set the desktop registers before opening its window.
        ctx.add_singleton_model(dragent::auth::AuthManager::new);
        ctx.add_singleton_model(dragent::settings::AccountStore::new);
        ctx.add_singleton_model(dragent::realtime::manager::SessionManager::new);
        // `ViewerRoot` renders the shared session in Tab mode: the mirrored
        // terminal + a presence bar (no sidebar/navbar). `FullApp` mode reuses
        // the real RootView when a whole-desktop share lands.
        ctx.add_window(
            warpui::AddWindowOptions::default(),
            dragent::viewer_view::ViewerRoot::new,
        );
    });
}
