use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::rc::{self, Rc};
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

use anyhow::Result;
use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use pathfinder_geometry::rect::RectF;

use super::RefCounts;
use crate::app_focus_telemetry::AppFocusInfo;
use crate::assets::asset_cache::{AssetCache, AssetSource};
use crate::assets::AssetProvider;
use crate::fonts::{self, FallbackFontModel};
use crate::image_cache::{self, ImageCache};
use crate::keymap::Keystroke;
use crate::modals::{
    AlertDialog, AlertDialogWithCallbacks, AppModalCallback, ModalId, PlatformModalResponseData,
};
use crate::platform::{self, WindowContext};
use crate::r#async::executor::{self};
use crate::r#async::{FutureId, SpawnableOutput};
use crate::windowing::WindowManager;
use crate::{
    assets, rendering, Action, AddSingletonModel, AnyModel, AnyView, Clipboard, Element, Entity,
    GetSingletonModelHandle, ModelAsRef, Presenter, ReadModel, ReadView, SingletonEntity,
    SpawnedFuture, TypedActionView, UpdateModel, UpdateView, View, ViewAsRef, WindowId, ZoomFactor,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;

impl AppContext {
    pub fn is_window_open(&self, window_id: WindowId) -> bool {
        self.windows.contains_key(&window_id)
    }

    /// Sets or clears the window for which focus changes should be suppressed.
    /// When set, all `ctx.focus()` calls targeting this window will be ignored.
    pub fn set_suppress_focus_for_window(&mut self, window_id: Option<WindowId>) {
        self.suppress_focus_for_window = window_id;
    }

    pub(super) fn with_foreground_executor(
        foreground: Rc<executor::Foreground>,
        platform_delegate: Box<dyn platform::Delegate>,
        window_manager: Box<dyn platform::WindowManager>,
        font_db: Box<dyn platform::FontDB>,
        asset_provider: Box<dyn assets::AssetProvider>,
        is_unit_test: bool,
    ) -> Self {
        let mut ctx = Self {
            // AppContext fields
            models: Default::default(),
            singleton_models: Default::default(),
            windows: Default::default(),
            ref_counts: Arc::new(Mutex::new(RefCounts::default())),
            last_frame_position_cache: Default::default(),
            platform_delegate,
            // AppContext fields
            actions: Default::default(),
            typed_actions: Default::default(),
            global_actions: Default::default(),
            presenters: Default::default(),
            rendering_config: Default::default(),
            keystroke_matcher: Default::default(),
            disabled_key_bindings_windows: Default::default(),
            next_task_id: 0,
            weak_self: rc::Weak::default(),
            subscriptions: Default::default(),
            next_window_bounds: None,
            next_window_bounds_map: Default::default(),
            observations: Default::default(),
            pending_unsubscribes: None,
            window_invalidations: Default::default(),
            invalidation_callbacks: Default::default(),
            window_bounds: Default::default(),
            window_last_mouse_moved_event: Default::default(),
            foreground: foreground.clone(),
            background: Default::default(),
            task_callbacks: Default::default(),
            notify_tasks: Default::default(),
            repaint_tasks: Default::default(),
            spawned_futures: Default::default(),
            #[cfg(test)]
            task_done: async_channel::unbounded(),
            pending_effects: VecDeque::new(),
            pending_flushes: 0,
            flushing_effects: false,
            app_focus_info: AppFocusInfo::new(),
            first_frame_callback: None,
            frame_drawn_callback: None,
            global_shortcuts: Default::default(),
            next_frame_callbacks: Default::default(),
            event_munger: Box::new(|_evt, _ctx| {}),
            before_open_url_callback: Box::new(|url, _ctx| url.to_owned()),
            a11y_verbosity: Default::default(),
            platform_modal_data_map: Default::default(),
            on_draw_frame_error_callback: None,
            cursor_updated_for_view: None,
            is_unit_test,
            termination_result: Default::default(),
            zoom_factor: ZoomFactor::default(),
            view_to_window: Default::default(),
            structural_child_to_parent: Default::default(),
            structural_parent_to_children: Default::default(),
            suppress_focus_for_window: None,
            fallback_font_source_provider: None,
            focus_observations: Default::default(),
        };

        // Register a variety of required/core singleton models.
        ctx.add_singleton_model(|_| fonts::Cache::new(font_db));
        ctx.add_singleton_model(|_| WindowManager::new(window_manager));
        ctx.add_singleton_model(|ctx| {
            AssetCache::new(asset_provider, foreground, ctx.background_executor())
        });
        ctx.add_singleton_model(|_| ImageCache::new());
        ctx.add_singleton_model(|_| FallbackFontModel::new());

        if !is_unit_test {
            ctx.background_executor()
                .spawn(async {
                    image_cache::prewarm_svg_font_db();
                })
                .detach();
        }

        ctx
    }

    /// Registers a provider that creates an [`AssetSource`] for a given URL.
    /// Used to load fallback fonts without pulling `reqwest` into `warpui_core`.
    pub fn set_fallback_font_source_provider(
        &mut self,
        provider: impl Fn(&str) -> AssetSource + 'static,
    ) {
        self.fallback_font_source_provider = Some(Box::new(provider));
    }

    pub fn foreground_executor(&self) -> &Rc<executor::Foreground> {
        &self.foreground
    }

    pub fn background_executor(&self) -> &Arc<executor::Background> {
        &self.background
    }

    pub fn window_bounds(&self, window_id: &WindowId) -> Option<RectF> {
        *self.window_bounds.get(window_id)?
    }

    pub fn update_window_bounds(&mut self, window_id: WindowId, bounds: RectF) {
        self.window_bounds.insert(window_id, Some(bounds));
    }

    /// Moves the OS window to `bounds` and immediately updates the local cache.
    /// Unlike `update_window_bounds` (which only updates the cache when the
    /// platform reports a move), this also commands the platform — so
    /// `window_bounds()` returns the new rect on the same frame, without
    /// waiting for a move-event callback.
    pub fn set_and_cache_window_bounds(&mut self, window_id: WindowId, bounds: RectF) {
        self.windows().set_window_bounds(window_id, bounds);
        self.window_bounds.insert(window_id, Some(bounds));
    }

    pub(super) fn matches_any_window_bounds(&self, r: RectF) -> bool {
        self.window_bounds.values().any(|b| *b == Some(r))
    }

    /// Create a window showing a modal dialog native to the platform. The modal will synchronously
    /// block all other interactions with the app until dismissed. Each button can have a callback
    /// attached to it in the [`crate::modals::ModalButton`] struct.
    pub fn show_native_platform_modal(
        &mut self,
        alert_data: AlertDialogWithCallbacks<AppModalCallback>,
    ) {
        let id = ModalId::new();
        let (button_titles, button_callbacks) = alert_data
            .button_data
            .into_iter()
            .map(|button| (button.title, button.on_click))
            .unzip();
        let dialog = AlertDialog {
            message_text: alert_data.message_text,
            info_text: alert_data.info_text,
            buttons: button_titles,
        };

        let response_data = PlatformModalResponseData {
            button_callbacks,
            disable_callback: alert_data.on_disable,
        };
        self.platform_modal_data_map.insert(id, response_data);
        self.platform_delegate
            .show_native_platform_modal(id, dialog);
    }

    /// When a native platform modal which was requested by an app/view context returns a response,
    /// this method handles dispatching the right handler for the button clicked. The response is
    /// encoded as a 0-based index into the list of buttons on the modal, and the callback will be
    /// at the same index in the Vec of callbacks.
    /// TODO(CORE-2323): Implement native Windows OS modal
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn process_platform_modal_response(
        &mut self,
        modal_id: ModalId,
        response_button_index: usize,
        disable_modal: bool,
    ) {
        if let Some(mut response_data) = self.platform_modal_data_map.remove(&modal_id) {
            if disable_modal {
                (response_data.disable_callback)(self);
            }
            response_data.button_callbacks.remove(response_button_index)(self);
        }
    }

    pub fn zoom_factor(&self) -> ZoomFactor {
        self.zoom_factor
    }

    pub fn report_active_cursor_position_update(&self) {
        self.windows().active_cursor_position_updated();
    }

    pub fn rendering_config(&self) -> rendering::Config {
        self.rendering_config
    }

    pub fn update_rendering_config<U>(&mut self, update_fn: U)
    where
        U: FnOnce(&mut rendering::Config),
    {
        update_fn(&mut self.rendering_config)
    }

    pub fn has_window_invalidations(&self, window_id: WindowId) -> bool {
        self.window_invalidations
            .get(&window_id)
            .is_some_and(|invalidation| {
                !invalidation.updated.is_empty() || !invalidation.removed.is_empty()
            })
    }

    pub fn presenter(&self, window_id: WindowId) -> Option<Rc<RefCell<Presenter>>> {
        // The presenter may not exist if there is a race condition where a window event comes in
        // after the window is closed (for example, if a fullscreen window is closed, a resize event
        // comes in after the window is closed.
        self.presenters.get(&window_id).cloned()
    }

    pub(super) fn invalidate_all_views_for_window(&mut self, window_id: WindowId) {
        let Some(window) = self.windows.get(&window_id) else {
            return;
        };
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .updated = window.views.keys().cloned().collect();
    }

    pub fn invalidate_all_views(&mut self) {
        // Mark all views in all windows as invalid.
        for window_id in self.windows.keys().cloned().collect_vec() {
            self.invalidate_all_views_for_window(window_id)
        }
    }

    pub(crate) fn on_global_shortcut_triggered(&mut self, shortcut: Keystroke) {
        if let Some(global_action) = self.global_shortcuts.remove(&shortcut) {
            self.dispatch_global_action(global_action.action, global_action.args.as_ref());
            self.global_shortcuts.insert(shortcut, global_action);
        }
    }

    pub fn on_window_invalidated<F: 'static + FnMut(WindowId, &mut AppContext)>(
        &mut self,
        window_id: WindowId,
        callback: F,
    ) {
        self.invalidation_callbacks
            .insert(window_id, Box::new(callback));
    }
    pub(super) fn new(
        platform_delegate: Box<dyn platform::Delegate>,
        window_manager: Box<dyn platform::WindowManager>,
        font_db: Box<dyn platform::FontDB>,
        asset_provider: Box<dyn AssetProvider>,
    ) -> Result<Self> {
        Ok(Self::with_foreground_executor(
            Rc::new(executor::Foreground::platform(
                platform_delegate.dispatch_delegate(),
            )?),
            platform_delegate,
            window_manager,
            font_db,
            asset_provider,
            false, /* is_unit_test */
        ))
    }

    #[cfg(feature = "test-util")]
    pub(super) fn register_spawned_future(&mut self, future: SpawnedFuture) -> FutureId {
        let future_id = FutureId::new();
        self.spawned_futures.insert(future_id, future);

        future_id
    }

    #[cfg(not(feature = "test-util"))]
    pub(super) fn register_spawned_future(&mut self, _future: SpawnedFuture) -> FutureId {
        FutureId::new()
    }
}
