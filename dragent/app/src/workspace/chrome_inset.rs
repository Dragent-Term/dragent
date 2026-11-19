//! Layout insets for frameless window chrome (e.g. macOS traffic-light buttons).

use crate::theme::HEADER_BAR_HEIGHT;
use warpui::AppContext;
use warpui_core::windowing::WindowManager;
use warpui_core::WindowId;

/// Horizontal width reserved for the macOS close/minimize/zoom cluster.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_CLUSTER_WIDTH: f32 = 64.0;

/// Extra gap between the traffic lights and the first header control.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_GAP: f32 = 16.0;

/// Size the native titlebar region so macOS traffic lights render at standard scale.
///
/// Warp calls this from `WorkspaceView::update_titlebar_height`; Dragent must do the same
/// on every window (including onboarding) or the buttons appear as tiny pinpricks.
pub fn sync_native_window_chrome(windows: &WindowManager, window_id: WindowId) {
    #[cfg(target_os = "macos")]
    {
        if let Some(platform_window) = windows.platform_window(window_id) {
            platform_window
                .as_ref()
                .set_titlebar_height(f64::from(HEADER_BAR_HEIGHT));
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (windows, window_id);
    }
}

/// Convenience for views that have a [`warpui::AppContext`] reference.
pub fn sync_native_window_chrome_app(app: &AppContext, window_id: WindowId) {
    sync_native_window_chrome(app.windows(), window_id);
}

/// Left padding for the global header so toolbar icons clear native window controls.
/// Takes the window the header is being rendered in — NOT `window_ids().next()`,
/// which is always the first window and gives the wrong fullscreen state (hence
/// wrong padding) in second+ windows.
pub fn header_left_padding(app: &AppContext, window_id: WindowId) -> f32 {
    #[cfg(target_os = "macos")]
    {
        if is_fullscreen(app, window_id) {
            return 12.0;
        }
        TRAFFIC_LIGHT_CLUSTER_WIDTH + TRAFFIC_LIGHT_GAP
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, window_id);
        12.0
    }
}

#[cfg(target_os = "macos")]
fn is_fullscreen(app: &AppContext, window_id: WindowId) -> bool {
    use warpui_core::platform::FullscreenState;

    app.windows()
        .platform_window(window_id)
        .is_some_and(|window| window.fullscreen_state() == FullscreenState::Fullscreen)
}
