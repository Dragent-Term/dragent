use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicI64;

use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;
use pathfinder_geometry::rect::RectF;

use crate::assets::AssetProvider;
use crate::core::Window;
use crate::event::KeyState;
use crate::keymap::{CustomTag, Keystroke};
use crate::notification::{NotificationSendError, RequestPermissionsOutcome, UserNotification};
use crate::platform::keyboard::KeyCode;
use crate::platform::{Cursor, WindowBounds, WindowContext, WindowOptions, WindowStyle};
use crate::windowing::{WindowCallbacks, WindowManager};
use crate::{
    AddWindowOptions, AnyModel, AnyView, Clipboard, CursorInfo, Element, Entity, EntityId, Event,
    ModelContext, ModelHandle, Presenter, Scene, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle, WindowId,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;
use crate::ClosedWindowData;

impl AppContext {
    /// Dispatches `self_or_child_interacted_with` on every view in the responder chain,
    /// and should only be called if the event was handled by a view in the chain.
    pub(super) fn dispatch_self_or_child_interacted_with(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
    ) {
        for view_id in responder_chain {
            if let Some(view) = self
                .windows
                .get_mut(&window_id)
                .and_then(|w| w.views.remove(view_id))
            {
                view.self_or_child_interacted_with(self, window_id, *view_id);

                // Reinsert the view before moving on to the next link in the responder chain
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.views.insert(*view_id, view);
                }
            }
        }
    }

    pub fn default_keystroke_trigger_for_custom_action(
        &self,
        custom_tag: CustomTag,
    ) -> Option<Keystroke> {
        self.keystroke_matcher
            .default_keystroke_trigger_for_custom_action(custom_tag)
    }

    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.pending_flushes += 1;
        let model_id = EntityId::new();
        let mut ctx = ModelContext::new(self, model_id);
        let model = build_model(&mut ctx);
        self.models.insert(model_id, Box::new(model));
        self.flush_effects();
        ModelHandle::new(model_id, &self.ref_counts)
    }

    pub fn add_singleton_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        let model_handle = self.add_model(build_model);
        let prev_value = self
            .singleton_models
            .insert(std::any::TypeId::of::<T>(), model_handle.clone().into());
        // Panic in debug mode if this is the second time a singleton model was
        // registered for type T.
        debug_assert!(
            prev_value.is_none(),
            "add_singleton_model() was called twice for {:?}",
            std::any::type_name::<T>()
        );
        model_handle
    }

    /// Delegates to the OS to request the user attention within the app. For mac this bounces the
    /// icon in the dock.
    pub(super) fn request_user_attention(&self, window_id: WindowId) {
        self.platform_delegate.request_user_attention(window_id);
    }

    /// Delegates to the OS to show the system character palette.
    pub fn open_character_palette(&mut self) {
        self.platform_delegate.open_character_palette();
    }

    /// Delegates to the OS to request permissions for sending desktop notifications.
    ///
    /// ## Platform-Specific
    /// * Linux: Always calls the `on_completion_callback` with a value of [`RequestPermissionsOutcome::Accepted`].
    pub(super) fn request_desktop_notification_permissions<F, T>(
        &mut self,
        view_id: EntityId,
        window_id: WindowId,
        on_completion_callback: F,
    ) where
        F: 'static + Send + Sync + FnOnce(&mut T, RequestPermissionsOutcome, &mut ViewContext<T>),
        T: View,
    {
        self.platform_delegate
            .request_desktop_notification_permissions(Box::new(
                move |request_permissions_outcome, ctx| {
                    if let Some(mut view) = ctx
                        .windows
                        .get_mut(&window_id)
                        .and_then(|w| w.views.remove(&view_id))
                    {
                        let mut view_context = ViewContext::new(ctx, window_id, view_id);
                        on_completion_callback(
                            view.as_mut()
                                .as_any_mut()
                                .downcast_mut()
                                .expect("Should be able to downcast to mutable view."),
                            request_permissions_outcome,
                            &mut view_context,
                        );
                        ctx.windows
                            .get_mut(&window_id)
                            .expect("Should be able to retrieve window.")
                            .views
                            .insert(view_id, view);
                    }
                },
            ))
    }

    pub fn reset_cursor(&mut self) {
        self.cursor_updated_for_view = None;
        self.platform_delegate.set_cursor_shape(Cursor::Arrow)
    }

    #[cfg(test)]
    pub fn get_cursor_shape(&self) -> Cursor {
        self.platform_delegate.get_cursor_shape()
    }

    /// Delegates to the OS to send a desktop notification.
    pub(super) fn send_desktop_notification<F, T>(
        &mut self,
        content: UserNotification,
        view_id: EntityId,
        window_id: WindowId,
        on_error_callback: F,
    ) where
        F: 'static + Send + Sync + FnOnce(&mut T, NotificationSendError, &mut ViewContext<T>),
        T: View,
    {
        self.platform_delegate.send_desktop_notification(
            content,
            window_id,
            Box::new(move |notification_error, ctx| {
                if let Some(mut view) = ctx
                    .windows
                    .get_mut(&window_id)
                    .and_then(|w| w.views.remove(&view_id))
                {
                    let mut view_context = ViewContext::new(ctx, window_id, view_id);
                    on_error_callback(
                        view.as_mut()
                            .as_any_mut()
                            .downcast_mut()
                            .expect("Should be able to downcast to mutable view."),
                        notification_error,
                        &mut view_context,
                    );
                    ctx.windows
                        .get_mut(&window_id)
                        .expect("Should be able to retrieve window.")
                        .views
                        .insert(view_id, view);
                }
            }),
        )
    }

    pub(super) fn active_cursor_position(&mut self, window_id: WindowId) -> Option<CursorInfo> {
        let focused_view_id = self.focused_view_id(window_id)?;
        let view = self
            .windows
            .get_mut(&window_id)?
            .views
            .remove(&focused_view_id)?;
        let position = view.active_cursor_position(self, window_id, focused_view_id);

        if let Some(window) = self.windows.get_mut(&window_id) {
            window.views.insert(focused_view_id, view);
        }

        position
    }

    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        self.platform_delegate.clipboard()
    }

    pub fn set_last_mouse_move_event(&mut self, window_id: WindowId, event: Event) {
        match event {
            Event::MouseMoved { .. } => {
                let last_mouse_moved_event = self.get_last_mouse_moved_event(window_id);
                *last_mouse_moved_event.borrow_mut() = Some(event);
            }
            _ => {
                panic!("not a mouse move event")
            }
        }
    }

    pub(super) fn get_last_mouse_moved_event(
        &mut self,
        window_id: WindowId,
    ) -> Rc<RefCell<Option<Event>>> {
        self.window_last_mouse_moved_event
            .entry(window_id)
            .or_default()
            .clone()
    }

    /// Creates a new window with the view returned by the `build_root_view` function as its root
    /// view.
    pub fn add_window<T, F>(
        &mut self,
        options: AddWindowOptions,
        build_root_view: F,
    ) -> (WindowId, ViewHandle<T>)
    where
        T: View + TypedActionView,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.insert_window(options, build_root_view)
    }

    pub(super) fn insert_window<T, F>(
        &mut self,
        add_window_options: AddWindowOptions,
        build_root_view: F,
    ) -> (WindowId, ViewHandle<T>)
    where
        T: View + TypedActionView,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        let (window_id, _root_view_id) =
            self.insert_window_internal(None, add_window_options, |window_id, ctx| {
                ctx.windows.insert(window_id, Window::default());
                let root_handle = ctx.add_typed_action_view(window_id, build_root_view);
                let root_view_id = root_handle.id();
                ctx.windows
                    .get_mut(&window_id)
                    .expect("this window was just inserted and should still exist")
                    .root_view = Some(root_handle.into());
                root_view_id
            });
        (
            window_id,
            self.root_view(window_id)
                .expect("should have just inserted a window and root view"),
        )
    }

    pub(super) fn insert_window_internal<F>(
        &mut self,
        window_id: Option<WindowId>,
        add_window_options: AddWindowOptions,
        build_window_data: F,
    ) -> (WindowId, EntityId)
    where
        F: FnOnce(WindowId, &mut AppContext) -> EntityId,
    {
        let AddWindowOptions {
            window_style,
            window_bounds,
            title,
            fullscreen_state,
            background_blur_radius_pixels,
            background_blur_texture,
            anchor_new_windows_from_closed_position,
            on_gpu_driver_selected: on_gpu_driver_reported,
            window_instance,
        } = add_window_options;

        let window_id = window_id.unwrap_or_else(WindowId::new);

        // Make sure we store the window bounds before we create the root view,
        // in case it uses this value.
        self.window_bounds.insert(window_id, window_bounds.bounds());
        self.next_window_bounds_map
            .insert(window_id, anchor_new_windows_from_closed_position);

        // Clear the next window bounds if they were set - we don't want to start
        // from the last closed position after a new window has been created.
        self.next_window_bounds = None;

        self.presenters
            .insert(window_id, Rc::new(RefCell::new(Presenter::new(window_id))));

        let window_options = WindowOptions {
            bounds: window_bounds,
            fullscreen_state,
            hide_title_bar: true,
            title,
            style: window_style,
            background_blur_radius_pixels,
            background_blur_texture,
            gpu_power_preference: self.rendering_config.gpu_power_preference,
            backend_preference: self.rendering_config.backend_preference,
            on_gpu_device_info_reported: on_gpu_driver_reported.unwrap_or(Box::new(|_| {})),
            window_instance,
        };

        let callbacks = WindowCallbacks {
            standard_action_callback: Box::new(move |action, ctx| {
                log::info!("Dispatching standard action {action:?}");
                let responder_chain = ctx.get_responder_chain(window_id);
                let res = ctx.dispatch_standard_action(action, window_id, &responder_chain);
                if let Err(error) = res {
                    log::error!("error dispatching standard action: {error}");
                }
            }),
            event_callback: Box::new(move |event, ctx| {
                let last_mouse_moved_event: Rc<RefCell<Option<Event>>> =
                    ctx.get_last_mouse_moved_event(window_id);
                match event {
                    Event::MouseMoved { .. } => {
                        *last_mouse_moved_event.borrow_mut() = Some(event.clone())
                    }
                    // Update the last mouse moved event with the current state of the modifiers
                    // so we don't dispatch a synthetic moused moved event with an incorrect modifier state.
                    Event::ModifierStateChanged {
                        modifiers,
                        key_code,
                        ..
                    } => {
                        if let Some(Event::MouseMoved { cmd, shift, .. }) =
                            &mut (*last_mouse_moved_event.borrow_mut())
                        {
                            *cmd = modifiers.cmd;
                            *shift = modifiers.shift;
                        }

                        if let Some(presenter) = ctx.presenter(window_id) {
                            if let Some(key_code) = key_code {
                                // Based on the key code in question and the new state of the modifier key,
                                // we can infer whether it was pressed or released.
                                let key_pressed = match key_code {
                                    KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                                        Some(modifiers.shift)
                                    }
                                    KeyCode::ControlLeft | KeyCode::ControlRight => {
                                        Some(modifiers.ctrl)
                                    }
                                    KeyCode::AltLeft | KeyCode::AltRight => Some(modifiers.alt),
                                    KeyCode::SuperLeft | KeyCode::SuperRight => Some(modifiers.cmd),
                                    KeyCode::Fn => Some(modifiers.func),
                                    _ => None,
                                };
                                if let Some(key_pressed) = key_pressed {
                                    // Note: this can be slightly incorrect in a particular edge case where the user
                                    // uses 2 physical keys corresponding to the same logical modifer. For example:
                                    // 1. The user holds down right-alt - we fire the right-alt pressed event
                                    // 2. The user then holds down left-alt - we fire the left-alt pressed event
                                    // 3. The user lets go of left-alt - we would incorrectly fire the left-alt pressed event (since the logical state is still true)
                                    // 4. The user lets go of right-alt - we correctly fire the right-alt released event
                                    // This is a known limitation due to the underlying APIs being limited (we must use lower-level Apple
                                    // APIs to get the exact physical key states, which we currently don't do).
                                    let key_state = if key_pressed {
                                        KeyState::Pressed
                                    } else {
                                        KeyState::Released
                                    };

                                    ctx.handle_window_event(
                                        Event::ModifierKeyChanged {
                                            key_code,
                                            state: key_state,
                                        },
                                        window_id,
                                        presenter.clone(),
                                    );
                                }
                            }
                        }
                    }
                    // Update the last mouse moved event on mouse up with the new position.
                    // This makes sure that hoverables are updated to the proper mouse
                    // position after a click-and-drag, as we don't update the cached event
                    // on a LeftMouseDragged event.
                    Event::LeftMouseUp {
                        position: new_position,
                        modifiers,
                    } => {
                        if let Some(Event::MouseMoved {
                            cmd,
                            shift,
                            position,
                            is_synthetic: _,
                        }) = &mut (*last_mouse_moved_event.borrow_mut())
                        {
                            *position = new_position;
                            *cmd = modifiers.cmd;
                            *shift = modifiers.shift;
                        }
                    }
                    _ => (),
                };

                if let Some(presenter) = ctx.presenter(window_id) {
                    ctx.handle_window_event(event, window_id, presenter)
                } else {
                    crate::windowing::EventDispatchResult::default()
                }
            }),
            resize_callback: Box::new(move |window, ctx| {
                let origin = window.origin();
                let size = window.size();
                ctx.window_bounds
                    .insert(window_id, Some(RectF::new(origin, size)));

                window.request_redraw();

                // On Linux and Windows, we don't have a direct way to react to
                // window fullscreen state changes, so instead we're using a
                // resize event as a signal that the fullscreen state _may_ have
                // changed.
                #[cfg(any(target_os = "linux", target_os = "freebsd", windows))]
                crate::windowing::WindowManager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.update_is_active_window_fullscreen(ctx);
                });

                ctx.report_active_cursor_position_update();
            }),
            build_scene_callback: Box::new(move |window, ctx| ctx.build_scene(window_id, window)),
            frame_callback: Box::new(move |ctx| {
                ctx.trigger_on_frame_drawn_callbacks(window_id);
            }),
            draw_frame_error_callback: Box::new(move |ctx| {
                ctx.dispatch_draw_frame_error_callback(window_id);
            }),
            move_callback: Box::new(move |bound, ctx| {
                ctx.window_bounds.insert(window_id, Some(bound));
                ctx.report_active_cursor_position_update();
            }),
            active_cursor_position_callback: Box::new(move |ctx| {
                ctx.active_cursor_position(window_id)
            }),
        };

        let window_result = WindowManager::handle(self).update(self, |windowing_state, _ctx| {
            windowing_state.open_window(window_id, window_options, callbacks)
        });

        // Create the root view after adding the window but before registering an invalidation callback.
        // This ensures that a platform window is always available to user view code.
        let root_view_id = build_window_data(window_id, self);
        self.focus(window_id, root_view_id);

        match window_result {
            Err(err) => {
                log::error!("error opening window: {err}");
            }
            Ok(_) => {
                self.on_window_invalidated(window_id, move |window_id, ctx| {
                    if let Some(window) = ctx.windows().platform_window(window_id) {
                        // All we need to do when a window is invalidated is
                        // request from the OS that it be redrawn.  We'll do all
                        // of the actual work later.
                        window.request_redraw();

                        // In tests, however, there's no real event loop, so we need to do the work now.
                        // While this _shouldn't_ be necessary in integration tests, it currently is.
                        if ctx.is_unit_test || cfg!(feature = "integration_tests") {
                            ctx.build_scene(window_id, window.as_ctx());
                        }
                    }
                });
            }
        };

        (window_id, root_view_id)
    }

    pub fn reopen_closed_window(&mut self, data: ClosedWindowData) {
        let ClosedWindowData {
            window_id,
            window,
            subscriptions,
            observations,
            view_to_window,
            bounds,
            fullscreen_state,
        } = data;

        let Some(bounds) = bounds else {
            log::error!("Had no bounds for cached closed window!");
            return;
        };

        for (entity_id, subs) in subscriptions {
            self.subscriptions.insert(entity_id, subs);
        }
        for (entity_id, obs) in observations {
            self.observations.insert(entity_id, obs);
        }
        for (entity_id, win_id) in view_to_window {
            self.view_to_window.insert(entity_id, win_id);
        }

        let add_window_options = AddWindowOptions {
            // TODO(vorporeal): what's the right value here?
            background_blur_radius_pixels: None,
            background_blur_texture: false,
            window_bounds: WindowBounds::ExactPosition(bounds),
            // TODO(alokedesai): Determine if, and how, we want to pass the on_gpu_driver_reported
            // callback from the original window back to this window.
            on_gpu_driver_selected: None,
            fullscreen_state,
            ..Default::default()
        };
        let root_view_id = window
            .root_view
            .as_ref()
            .expect("should have root view")
            .id();
        self.insert_window_internal(
            Some(window_id),
            add_window_options,
            move |window_id, ctx| {
                ctx.windows.insert(window_id, window);
                root_view_id
            },
        );

        self.invalidate_all_views_for_window(window_id);
    }

    /// Returns the bounds and window style for the next window to create.
    /// This is a function of what window was last closed plus the positions
    /// of all current windows.
    pub fn next_window_bounds_and_style(&self) -> (WindowBounds, WindowStyle) {
        let active_window_id = self.windows().active_window();
        match (
            self.next_window_bounds,
            active_window_id.and_then(|id| self.window_bounds(&id)),
        ) {
            // If the last closed window position exactly overlays any current
            // window, then do a cascade from the active window instead of
            // using the last closed window position
            (Some(last_closed_rect), Some(active_window_rect))
                if self.matches_any_window_bounds(last_closed_rect) =>
            {
                (
                    WindowBounds::ExactPosition(active_window_rect),
                    WindowStyle::Cascade,
                )
            }
            // Otherwise, use the last closed window position and size as the spot
            // for launching the new window.
            (Some(last_closed_rect), _) => (
                WindowBounds::ExactPosition(last_closed_rect),
                WindowStyle::Normal,
            ),
            // If there is no last closed window but there is an active window,
            // cascade from the active window.
            (_, Some(active_window_rect)) => (
                WindowBounds::ExactPosition(active_window_rect),
                WindowStyle::Cascade,
            ),
            // And finally fall back to using the default window position.
            (_, _) => (WindowBounds::Default, WindowStyle::Normal),
        }
    }

    /// Builds a new scene for the given window.
    pub(super) fn build_scene(
        &mut self,
        window_id: WindowId,
        window: &dyn WindowContext,
    ) -> Rc<Scene> {
        let mut scene = Rc::new(Scene::new(
            window.backing_scale_factor(),
            self.rendering_config(),
        ));
        let Some(presenter) = self.presenter(window_id) else {
            return scene;
        };

        // This outer loop exists because after redrawing a scene, we will sometimes emit an
        // artificial `MouseMoved` event to ensure that `Hoverable` elements are properly updated
        // if the layout changes. However, to prevent an infinite loop hanging the app, we limit
        // to a maximum of three iterations. If more invalidations are created, then those will be
        // handled on the next call to `update_windows`.
        for iter in 1..=3 {
            let invalidation = self.take_all_invalidations_for_window(window_id);

            // Always build the scene at least once, even if there
            // are no updated views.
            if invalidation.updated.is_empty() && !invalidation.redraw_requested && iter > 1 {
                break;
            }

            {
                let mut presenter = presenter.borrow_mut();
                presenter.invalidate(invalidation, self);

                // Skip rendering if a dimension is 0. This must happen after
                // invalidation to ensure the proper views are still invalidated.
                let size = window.size();
                if size.x() == 0. || size.y() == 0. {
                    log::debug!(
                        "Received window_id={window_id:?} with window size {size:?}. Skipping render"
                    );
                    break;
                }

                // Build the scene.  As part of this process, we compute element
                // origins and bounding boxes.
                //
                // In the future, we should separate out position computation from
                // scene building, as we don't need to do the latter on each
                // iteration of this loop.
                scene = presenter.build_scene(
                    size,
                    window.backing_scale_factor(),
                    window.max_texture_dimension_2d(),
                    self,
                );

                // Cache the last position cache after rendering.
                self.last_frame_position_cache
                    .insert(window_id, presenter.position_cache().clone());
            }

            // Synthesize a MouseMoved event in case any elements
            // are now hovered due to being in a different location.
            let last_mouse_moved_event: Rc<RefCell<Option<Event>>> =
                self.get_last_mouse_moved_event(window_id);
            let event = last_mouse_moved_event.borrow();
            if let Some(event) = event
                .as_ref()
                .and_then(Event::to_synthetic_mouse_move_event)
            {
                self.handle_window_event(event, window_id, presenter.clone());
            }
        }

        scene
    }
    /// Add a view that implements the `TypedAction` trait, including the default parent view
    ///
    /// This will create the view as normal as well as register it's `handle_action` method in the
    /// typed_actions hash.
    ///
    /// Note: This is intended to be the replacement for `add_view` with the conversion to typed
    /// actions (and will subsequently be renamed to `add_view` once that is complete)
    pub(super) fn add_typed_action_view_with_parent<V, F>(
        &mut self,
        window_id: WindowId,
        build_view: F,
        parent_view_id: EntityId,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.add_typed_action_view_internal(window_id, build_view, Some(parent_view_id))
    }
}
