use std::any::Any;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::atomic::AtomicI64;

use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;

use super::ViewType;
use crate::assets::AssetProvider;
use crate::core::ActionType;
use crate::keymap::{BindingLens, EditableBinding, FixedBinding, IsBindingValid, Trigger};
use crate::platform::{Cursor, WindowContext};
use crate::r#async::SpawnableOutput;
use crate::{
    AccessibilityData, Action, AddSingletonModel, AnyModel, AnyView, Clipboard, Element, Entity,
    EntityId, GetSingletonModelHandle, ModelAsRef, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewAsRef, ViewHandle, WindowId,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;

impl AppContext {
    pub fn root_view<T: View>(&self, window_id: WindowId) -> Option<ViewHandle<T>> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.root_view.as_ref())
            .and_then(|root_view| root_view.clone().downcast::<T>())
    }

    /// Returns the [`AccessibilityData`] of the focused view, or a parent of that view in its
    /// responder chain.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn focused_view_accessibility_data(
        &mut self,
        window_id: WindowId,
    ) -> Option<AccessibilityData> {
        let responder_chain = self.get_responder_chain(window_id);
        for view_id in responder_chain {
            let window = self.windows.get_mut(&window_id)?;
            let view = window.views.remove(&view_id)?;
            let accessibility_data = view.accessibility_data(self, window_id, view_id);

            if let Some(window) = self.windows.get_mut(&window_id) {
                window.views.insert(view_id, view);
            }

            if let Some(accessibility_data) = accessibility_data {
                return Some(accessibility_data);
            }
        }
        None
    }

    pub fn dispatch_action_for_view(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        name: &str,
        arg: &dyn Any,
    ) -> bool {
        if let Some(presenter) = self.presenter(window_id) {
            let responder_chain = presenter.borrow().ancestors(view_id);
            self.dispatch_action(window_id, &responder_chain, name, arg, log::Level::Info)
        } else {
            false
        }
    }

    /// Dispatch a typed action using the given view as the base of the responder_chain
    pub fn dispatch_typed_action_for_view(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        action: &dyn Action,
    ) {
        if let Some(presenter) = self.presenter(window_id) {
            let responder_chain = presenter.borrow().ancestors(view_id);
            self.dispatch_typed_action(window_id, &responder_chain, action, log::Level::Debug);
        }
    }

    pub fn dispatch_action(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        name: &str,
        arg: &dyn Any,
        log_level: log::Level,
    ) -> bool {
        log::log!(log_level, "dispatching action for {name:?}");
        self.pending_flushes += 1;

        let mut any_action_handled = false;
        let mut dispatched_actions = HashSet::<String>::new();

        for view_id in responder_chain.iter().rev() {
            if let Some(mut view) = self
                .windows
                .get_mut(&window_id)
                .and_then(|w| w.views.remove(view_id))
            {
                let type_id = view.as_any().type_id();

                if let Some((name, mut handlers)) = self
                    .actions
                    .get_mut(&type_id)
                    .and_then(|h| h.remove_entry(name))
                {
                    // Only dispatch the action if a previous view hasn't already handled it or the
                    // child view determined it should be propagated to the parent view.
                    if !dispatched_actions.contains(name.as_str()) {
                        for handler in handlers.iter_mut().rev() {
                            let handled = handler(view.as_mut(), arg, self, window_id, *view_id);
                            any_action_handled |= handled;
                            if handled {
                                dispatched_actions.insert(name.clone());
                            }
                        }
                    } else {
                        log::log!(log_level, "not propagating action {name:?} to parent view");
                    }
                    self.actions
                        .get_mut(&type_id)
                        .unwrap()
                        .insert(name, handlers);
                }

                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.views.insert(*view_id, view);
                }
            }
        }

        if !any_action_handled {
            log::warn!("Action {name:?} was dispatched, but no view handled it");
        }

        self.dispatch_global_action(name, arg);

        self.flush_effects();
        any_action_handled
    }

    /// Dispatch a typed action to a view that was registered with `add_typed_action_view`
    ///
    /// The action will be dispatched to deepest view in the `responder_chain` that registered a
    /// handler for the action's type
    pub fn dispatch_typed_action(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        action: &dyn Action,
        log_level: log::Level,
    ) -> bool {
        log::log!(
            log_level,
            "dispatching typed action: {}::{action:?}",
            action.type_name()
        );

        let action_type: ActionType = action.into();
        // If there are no handlers registered for the given action, then we can return early
        // without needing to look at the responder chain at all, since no views will handle it
        let Some(mut handlers) = self.typed_actions.remove(&action_type) else {
            log::warn!("Dispatched action has no handlers: {:?}", &action);
            return false;
        };

        // Increment pending flushes only after we know there is a handler for the action.
        self.pending_flushes += 1;

        // Traverse the responder chain from the leaf view to see if any views handle the action
        // We stop on the first View that handles the action, we explicitly do not propagate to
        // parent views
        let handled = responder_chain.iter().rev().any(|view_id| {
            let mut view = match self
                .windows
                .get_mut(&window_id)
                .and_then(|w| w.views.remove(view_id))
            {
                Some(view) => view,
                None => return false,
            };

            // Check to see if we have a handler for this action on the view
            let view_type = ViewType(view.as_any().type_id());
            let found = match handlers.get_mut(&view_type) {
                Some(handler) => {
                    handler(view.as_mut(), action.as_any(), self, window_id, *view_id);
                    true
                }
                None => false,
            };

            // Reinsert the view before moving on to the next link in the responder chain
            if let Some(window) = self.windows.get_mut(&window_id) {
                window.views.insert(*view_id, view);
            }

            found
        });

        // Reinsert the action handlers for this action type
        self.typed_actions.insert(action_type, handlers);

        if !handled {
            log::warn!("Action {action:?} was dispatched, but no view handled it");
        }

        self.flush_effects();
        handled
    }

    /// Global actions are being phased out. Prefer dispatching typed actions instead of global actions.
    #[track_caller]
    pub fn dispatch_global_action(&mut self, name: &str, arg: &dyn Any) {
        let location = std::panic::Location::caller();
        self.dispatch_global_action_internal(name, location, arg);
    }

    pub(super) fn dispatch_global_action_internal(
        &mut self,
        name: &str,
        location: &'static std::panic::Location<'static>,
        arg: &dyn Any,
    ) {
        if let Some((name, mut handlers)) = self.global_actions.remove_entry(name) {
            log::info!("dispatching global action for {}", &name);
            self.pending_flushes += 1;
            for handler in handlers.iter_mut().rev() {
                handler(arg, location, self);
            }
            self.global_actions.insert(name, handlers);
            self.flush_effects();
        }
    }
    /// Registers a validator that validates every binding that matches the given view's default
    /// [`Context`].
    /// After the app is initialized, the provided `binding_validator` function is called for every
    /// binding that matches the View's default context. If the binding is invalid (indicated by
    /// [`IsBindingValid::No`]), the app will panic if `debug_assertions` are enabled.
    #[cfg(debug_assertions)]
    pub fn register_binding_validator<T: View>(
        &mut self,
        binding_validator: impl Fn(BindingLens) -> IsBindingValid + 'static,
    ) {
        let context = T::default_keymap_context();
        self.keystroke_matcher
            .register_binding_validator(context, binding_validator)
    }

    #[cfg(not(debug_assertions))]
    pub fn register_binding_validator<T: View>(
        &mut self,
        binding_validator: impl Fn(BindingLens) -> IsBindingValid + 'static,
    ) {
    }

    /// Sets a default binding validator that runs on _every_ binding that is registered by the
    /// application.
    /// Noops if `debug_assertions` are disabled.
    #[cfg(debug_assertions)]
    pub fn set_default_binding_validator(
        &mut self,
        binding_validator: impl Fn(BindingLens) -> IsBindingValid + 'static,
    ) {
        self.keystroke_matcher
            .set_default_binding_validator(binding_validator)
    }

    /// Sets a default binding validator that runs on _every_ binding that is registered by the
    /// application.
    /// Noops if `debug_assertions` are disabled.
    #[cfg(not(debug_assertions))]
    pub fn set_default_binding_validator(
        &mut self,
        binding_validator: impl Fn(BindingLens) -> IsBindingValid + 'static,
    ) {
    }

    /// Runs through each registered binding validator, asserting that each matching binding is
    /// valid. Noop if debug assertions are not enabled.
    #[cfg(debug_assertions)]
    pub(crate) fn validate_bindings(&mut self) {
        self.keystroke_matcher.validate_bindings();
    }

    /// Runs through each registered binding validator, asserting that each matching binding is
    /// valid. Noop if debug assertions are not enabled.
    #[cfg(not(debug_assertions))]
    pub(crate) fn validate_bindings(&mut self) {}

    /// Add new fixed (immutable) key bindings to the app
    pub fn register_fixed_bindings<T: IntoIterator<Item = FixedBinding>>(&mut self, bindings: T) {
        self.keystroke_matcher.register_fixed_bindings(bindings);
    }

    /// Register new actions with the app
    ///
    /// Editable Bindings have a name identifier which can be used to override their key bindings
    /// via the `set_custom_trigger` method.
    pub fn register_editable_bindings<A: IntoIterator<Item = EditableBinding>>(
        &mut self,
        actions: A,
    ) {
        self.keystroke_matcher.register_editable_bindings(actions);
    }

    /// Set a custom trigger for a given editable binding name
    ///
    /// This will override the default trigger for that action
    pub fn set_custom_trigger(&mut self, name: String, trigger: Trigger) {
        self.keystroke_matcher.set_custom_trigger(name, trigger);
    }

    pub(super) fn disable_key_bindings(&mut self, window_id: WindowId) {
        self.disabled_key_bindings_windows.insert(window_id);
    }

    pub(super) fn enable_key_bindings(&mut self, window_id: WindowId) {
        self.disabled_key_bindings_windows.remove(&window_id);
    }

    pub fn key_bindings_enabled(&self, window_id: WindowId) -> bool {
        !self.disabled_key_bindings_windows.contains(&window_id)
    }

    /// Remove any custom trigger associated with a given action
    ///
    /// This will return the trigger to its default state (if any)
    pub fn remove_custom_trigger<N>(&mut self, name: N)
    where
        N: AsRef<str>,
    {
        self.keystroke_matcher.remove_custom_trigger(name);
    }

    /// Fetch the key bindings that apply to the given window / view
    ///
    /// This will look at key bindings in precedence order (closest to the view) first and only
    /// return the _first_ binding for a given trigger condition. That binding is the one that
    /// would run if the keys were pressed, so it matches what bindings are "available" from that
    /// view.
    pub fn key_bindings_for_view(
        &self,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Vec<BindingLens<'_>> {
        let contexts = self.contexts_for_window_and_view(window_id, view_id);
        let mut triggers = HashSet::with_capacity(contexts.len());
        let mut results = Vec::with_capacity(contexts.len());

        for binding in contexts
            .into_iter()
            // The contexts are ordered top-down (from the root view down to the current view),
            // however the precedence order is bottom-up, so we need to reverse the iteration order
            .rev()
            .flat_map(|c| self.keystroke_matcher.bindings_for_context(c))
        {
            // Include all empty triggers, since they can't "overlap"
            if binding.trigger.is_empty() || triggers.insert(binding.trigger) {
                results.push(binding);
            }
        }

        results
    }
    pub(super) fn set_cursor_shape(
        &mut self,
        cursor: Cursor,
        window_id: WindowId,
        view_id: EntityId,
    ) {
        self.cursor_updated_for_view = Some((window_id, view_id));
        self.platform_delegate.set_cursor_shape(cursor)
    }
}
