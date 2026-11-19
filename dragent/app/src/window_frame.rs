//! Main window geometry: saved when the window closes or the app quits, and
//! reused at launch so the layout (and every PTY size) comes back unchanged.

use std::sync::Mutex;

use dragent_persistence::SerializedWindowFrame;
use warpui_core::geometry::rect::RectF;
use warpui_core::geometry::vector::Vector2F;
use warpui_core::platform::{FullscreenState, WindowBounds};
use warpui_core::{AddWindowOptions, AppContext, ClosedWindowData, DisplayIdx, WindowId};

static MAIN_WINDOW: Mutex<Option<WindowId>> = Mutex::new(None);

const MIN_WIDTH: f32 = 400.0;
const MIN_HEIGHT: f32 = 300.0;
/// A restored frame must overlap a connected display by this much so its
/// title bar can still be grabbed.
const MIN_VISIBLE: f32 = 100.0;

pub(crate) fn set_main_window(window_id: WindowId) {
    *MAIN_WINDOW.lock().unwrap() = Some(window_id);
}

/// Options for the main window: the saved frame when it still fits on a
/// connected display, otherwise the maximized default.
pub(crate) fn launch_options(ctx: &AppContext) -> AddWindowOptions {
    let maximized = AddWindowOptions {
        fullscreen_state: FullscreenState::Maximized,
        ..Default::default()
    };
    let Some(frame) = dragent_persistence::launch_window_frame() else {
        return maximized;
    };
    let bounds = RectF::new(
        Vector2F::new(frame.x, frame.y),
        Vector2F::new(frame.width, frame.height),
    );
    if frame.width < MIN_WIDTH || frame.height < MIN_HEIGHT || !visible_on_a_display(ctx, bounds) {
        return maximized;
    }
    AddWindowOptions {
        window_bounds: WindowBounds::ExactPosition(bounds),
        fullscreen_state: match frame.fullscreen_state {
            1 => FullscreenState::Fullscreen,
            2 => FullscreenState::Maximized,
            _ => FullscreenState::Normal,
        },
        ..Default::default()
    }
}

/// Save the main window's frame from a window that is closing.
pub(crate) fn persist_closed(closed: Option<&ClosedWindowData>) {
    let Some(closed) = closed else { return };
    if *MAIN_WINDOW.lock().unwrap() != Some(closed.window_id) {
        return;
    }
    if let Some(bounds) = closed.bounds() {
        save(bounds, closed.fullscreen_state());
    }
}

/// Save the main window's frame while it is still open (app quit).
pub(crate) fn persist_live(ctx: &AppContext) {
    let Some(window_id) = *MAIN_WINDOW.lock().unwrap() else {
        return;
    };
    let Some(bounds) = ctx.window_bounds(&window_id) else {
        return;
    };
    let state = ctx
        .windows()
        .platform_window(window_id)
        .map_or(FullscreenState::Normal, |window| window.fullscreen_state());
    save(bounds, state);
}

fn save(bounds: RectF, state: FullscreenState) {
    if bounds.width() < MIN_WIDTH || bounds.height() < MIN_HEIGHT {
        return;
    }
    dragent_persistence::shared_writer().save_window_frame(SerializedWindowFrame {
        x: bounds.origin_x(),
        y: bounds.origin_y(),
        width: bounds.width(),
        height: bounds.height(),
        fullscreen_state: state as u8,
    });
}

fn visible_on_a_display(ctx: &AppContext, bounds: RectF) -> bool {
    let windows = ctx.windows();
    let displays = std::iter::once(DisplayIdx::Primary)
        .chain((0..windows.display_count().saturating_sub(1)).map(DisplayIdx::External));
    displays
        .filter_map(|display| windows.bounds_for_display_idx(display))
        .any(|display| {
            display.intersection(bounds).is_some_and(|overlap| {
                overlap.width() >= MIN_VISIBLE && overlap.height() >= MIN_VISIBLE
            })
        })
}
