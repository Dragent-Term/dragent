use std::any::{Any, TypeId};
use std::sync::atomic::AtomicI64;

use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;

use super::{GlobalShortcut, Observation, Subscription, ViewType};
use crate::accessibility::ActionAccessibilityContent;
use crate::assets::AssetProvider;
use crate::core::ActionType;
use crate::keymap::Keystroke;
use crate::{
    Action, AnyModel, AnyView, Entity, EntityId, Event, ModelHandle, TypedActionView, View,
    ViewContext, ViewHandle, WindowId, ZoomFactor,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;
#[cfg(feature = "test-util")]
use crate::r#async::FutureId;

impl AppContext {
    /// Observes a [`ModelHandle`] for changes, calling `callback` whenever the model is invalidated.
    pub fn observe_model<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        F: 'static + FnMut(ModelHandle<S>, &mut AppContext),
    {
        self.observations
            .entry(handle.id())
            .or_default()
            .push(Observation::FromApp {
                callback: Box::new(move |observed_id, app| {
                    let model = ModelHandle::new(observed_id, &app.ref_counts);
                    callback(model, app)
                }),
            });
    }

    /// Subscribes to a [`ModelHandle`] for changes, calling `callback` with the emitted event whenever the model is invalidated.
    pub fn subscribe_to_model<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        S::Event: 'static,
        F: 'static + FnMut(ModelHandle<S>, &S::Event, &mut AppContext),
    {
        self.subscriptions
            .entry(handle.id())
            .or_default()
            .push(Subscription::FromApp {
                callback: Box::new(move |payload, app, entity_id| {
                    let model = ModelHandle::new(entity_id, &app.ref_counts);
                    let payload: &<S as Entity>::Event =
                        payload.downcast_ref().expect("downcast is type safe");
                    callback(model, payload, app);
                }),
            });
    }

    /// Subscribes to a [`ViewHandle`] for changes, calling `callback` with the emitted event whenever the view is invalidated.
    pub fn subscribe_to_view<S, F>(&mut self, handle: &ViewHandle<S>, mut callback: F)
    where
        S: View,
        S::Event: 'static,
        F: 'static + FnMut(ViewHandle<S>, &S::Event, &mut AppContext),
    {
        self.subscriptions
            .entry(handle.id())
            .or_default()
            .push(Subscription::FromApp {
                callback: Box::new(move |payload, app, entity_id| {
                    let Some(current_window_id) = app.view_to_window.get(&entity_id).copied()
                    else {
                        log::warn!("subscribe_to_view callback: view {entity_id:?} not found in view_to_window");
                        return;
                    };
                    if !app.windows.contains_key(&current_window_id) {
                        log::warn!("subscribe_to_view callback: window {current_window_id:?} not found for view {entity_id:?}");
                        return;
                    }

                    let view = ViewHandle::new(current_window_id, entity_id, &app.ref_counts);
                    let payload: &<S as Entity>::Event =
                        payload.downcast_ref().expect("downcast is type safe");
                    callback(view, payload, app);
                }),
            });
    }

    /// Returns a future that can be awaited to ensure the spawned background future identified by
    /// `future_id` has finished. If no spawned future exists with `future_id` a future that
    /// immediately resolves to the unit type is returned.
    ///
    /// This is useful for tests to ensure that calls to `ctx#spawn` have
    /// completed before asserting correct state.
    #[cfg(feature = "test-util")]
    pub fn await_spawned_future(&mut self, future_id: FutureId) -> future::BoxFuture<'static, ()> {
        match self.spawned_futures.remove(&future_id) {
            None => futures::future::ready(()).boxed(),
            Some(future) => future,
        }
    }

    /// This callback will be invoked immediately after the very first frame in the app is drawn.
    /// After that, the callback will be removed.
    pub fn on_first_frame_drawn<F: 'static + Fn(&mut AppContext)>(&mut self, callback: F) {
        self.first_frame_callback = Some(Box::new(callback));
    }

    ///  Callback that is called whenever a frame is successfully drawn.
    pub fn on_frame_drawn<F: 'static + Fn(&mut AppContext, WindowId)>(&mut self, callback: F) {
        self.frame_drawn_callback = Some(Box::new(callback));
    }

    /// Callback invoked whenever a frame fails to render in a given [`WindowId`].
    pub fn on_draw_frame_error<F: 'static + Fn(&mut AppContext, WindowId)>(&mut self, callback: F) {
        self.on_draw_frame_error_callback = Some(Box::new(callback));
    }

    pub fn unregister_global_shortcut(&mut self, shortcut: &Keystroke) {
        if self.is_wayland() {
            return;
        }
        self.global_shortcuts.remove(shortcut);
        self.platform_delegate.unregister_global_shortcut(shortcut);
    }

    pub fn register_global_shortcut<T: 'static + Any>(
        &mut self,
        mut shortcut: Keystroke,
        action: &'static str,
        arg: T,
    ) {
        if self.is_wayland() {
            return;
        }
        // Note that for global hotkey we don't support registering the meta key so
        // we will treat meta key as alt.
        if shortcut.meta {
            shortcut.meta = false;
            shortcut.alt = true;
        }

        self.global_shortcuts.insert(
            shortcut.clone(),
            GlobalShortcut {
                action,
                args: Box::new(arg),
            },
        );

        self.platform_delegate.register_global_shortcut(shortcut);
    }

    pub(super) fn dispatch_draw_frame_error_callback(&mut self, window_id: WindowId) {
        let callback = self.on_draw_frame_error_callback.take();
        if let Some(callback) = &callback {
            callback(self, window_id);
        }

        self.on_draw_frame_error_callback = callback;
    }

    pub(super) fn trigger_on_frame_drawn_callbacks(&mut self, window_id: WindowId) {
        if let Some(callback) = self.first_frame_callback.take() {
            callback(self);
        }

        let frame_drawn_callback = self.frame_drawn_callback.take();
        if let Some(callback) = &frame_drawn_callback {
            callback(self, window_id);
        }
        self.frame_drawn_callback = frame_drawn_callback;

        if let Some(callbacks) = self.next_frame_callbacks.remove(&window_id) {
            for callback in callbacks {
                callback();
            }
        }
    }

    /// Installs the callback so that it'll be invoked after the next frame for the window is
    /// drawn.
    pub fn on_next_frame_drawn<F: 'static + Fn()>(&mut self, window_id: WindowId, callback: F) {
        let entry = self.next_frame_callbacks.entry(window_id).or_default();
        entry.push(Box::new(callback));
    }

    /// Sets a function which gets to inspect and modify events before they are dispatched.
    pub fn set_event_munger<F>(&mut self, handler: F)
    where
        F: 'static + Fn(&mut Event, &mut AppContext),
    {
        self.event_munger = Box::new(handler)
    }

    /// Sets the zoom factor for the application. Changing the zoom factor adjusts
    /// the magnification of every element rendered within the application.
    ///
    /// All views in every window are invalidated when this is invoked.
    ///
    /// ## Validation
    /// The zoom factor is clamped to the range [0.5, 4.0].
    pub fn set_zoom_factor(&mut self, zoom_factor: f32) {
        let zoom_factor = ZoomFactor::new(zoom_factor.clamp(0.5, 4.0));
        self.zoom_factor = zoom_factor;
        self.invalidate_all_views();
    }

    /// Sets the callback invoked before opening a URL.
    pub fn set_before_open_url<F>(&mut self, handler: F)
    where
        F: 'static + Fn(&str, &AppContext) -> String,
    {
        self.before_open_url_callback = Box::new(handler);
    }

    /// Internal helper method to store the handler for a `TypedActionView` being registered
    ///
    /// Creates a handler which will dispatch to `TypedActionView::handle_action` for the given
    /// View + Action combination.
    pub(super) fn add_typed_action<V>(&mut self)
    where
        V: TypedActionView + View,
    {
        let handler = Box::new(
            |view: &mut dyn AnyView,
             action: &dyn Any,
             app: &mut AppContext,
             window_id: WindowId,
             view_id: EntityId| {
                let is_screen_reader_enabled = app
                    .platform_delegate
                    .is_screen_reader_enabled()
                    .unwrap_or(false);
                // Safety: The handler is stored in a map keyed on both the ActionType and the
                // ViewType, so we will only call it if both match, making the downcasts safe
                let action = action
                    .downcast_ref()
                    .expect("Handlers are hashed by action type");
                let view = view
                    .as_any_mut()
                    .downcast_mut()
                    .expect("Handlers are hashed by view type");
                let mut ctx = ViewContext::new(app, window_id, view_id);
                V::handle_action(view, action, &mut ctx);
                if is_screen_reader_enabled {
                    match V::action_accessibility_contents(view, action, &mut ctx) {
                        ActionAccessibilityContent::CustomFn(f) => {
                            app.platform_delegate.set_accessibility_contents(
                                f(action).with_verbosity(app.a11y_verbosity),
                            );
                        }
                        ActionAccessibilityContent::Custom(content) => {
                            app.platform_delegate.set_accessibility_contents(
                                content.with_verbosity(app.a11y_verbosity),
                            );
                        }
                        ActionAccessibilityContent::Empty => {}
                    };
                }
            },
        );

        // Insert the action handler for this view into the `typed_actions` hash
        // We only need to do this once per View type, since the handler is the same for every
        // instance.
        self.typed_actions
            .entry(ActionType::of::<V::Action>())
            .or_default()
            .entry(ViewType::of::<V>())
            .or_insert(handler);
    }

    pub fn add_action<S, V, T, F>(&mut self, name: S, mut handler: F)
    where
        S: Into<String>,
        V: View,
        T: Any,
        F: 'static + FnMut(&mut V, &T, &mut ViewContext<V>) -> bool,
    {
        let name = name.into();
        let name_clone = name.clone();
        let handler = Box::new(
            move |view: &mut dyn AnyView,
                  arg: &dyn Any,
                  app: &mut AppContext,
                  window_id: WindowId,
                  view_id: EntityId| {
                match arg.downcast_ref() {
                    Some(arg) => {
                        let mut ctx = ViewContext::new(app, window_id, view_id);
                        handler(
                            view.as_any_mut()
                                .downcast_mut()
                                .expect("downcast is type safe"),
                            arg,
                            &mut ctx,
                        )
                    }
                    None => {
                        log::error!("Could not downcast argument for action {name_clone}");
                        false
                    }
                }
            },
        );

        self.actions
            .entry(TypeId::of::<V>())
            .or_default()
            .entry(name)
            .or_default()
            .push(handler);
    }

    pub fn add_global_action<S, T, F>(&mut self, name: S, mut handler: F)
    where
        S: Into<String>,
        T: 'static + Any,
        F: 'static + FnMut(&T, &mut AppContext),
    {
        let name = name.into();
        let name_clone = name.clone();
        let handler = Box::new(
            move |arg: &dyn Any,
                  location: &'static std::panic::Location<'static>,
                  app: &mut AppContext| {
                if let Some(arg) = arg.downcast_ref() {
                    handler(arg, app);
                } else {
                    debug_assert!(
                        false,
                        "Could not downcast argument for action {name_clone}: {location:?}"
                    );
                    log::error!("Could not downcast argument for action {name_clone}");
                }
            },
        );

        self.global_actions.entry(name).or_default().push(handler);
    }

    pub fn pending_flushes(&self) -> usize {
        self.pending_flushes
    }

    pub fn models_of_type<M: Entity>(&self) -> Vec<ModelHandle<M>> {
        let ref_counts = &self.ref_counts;
        self.models
            .iter()
            .filter(|(_, m)| (*m).as_any().type_id() == TypeId::of::<M>())
            .map(|(model_id, _)| ModelHandle::new(*model_id, ref_counts))
            .collect::<Vec<ModelHandle<M>>>()
    }
}
