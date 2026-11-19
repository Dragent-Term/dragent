use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::AtomicI64;

use anyhow::{anyhow, Result};
use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;

use super::autotracking;
use crate::actions::StandardAction;
use crate::assets::AssetProvider;
use crate::keymap::{
    BindingLens, Context, CustomTag, DescriptionContext, EditableBindingLens, Keystroke,
    MatchResult, Trigger,
};
use crate::platform::WindowContext;
use crate::r#async::SpawnableOutput;
use crate::windowing::WindowManager;
use crate::{
    Action, AddSingletonModel, AnyModel, AnyView, Clipboard, Element, Entity, EntityId, Event,
    GetSingletonModelHandle, ModelAsRef, NextNewWindowsHasThisWindowsBoundsUponClose, ReadModel,
    ReadView, SingletonEntity, TypedActionView, UpdateModel, UpdateView, View, ViewAsRef, WindowId,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;
use crate::ClosedWindowData;

impl AppContext {
    /// Fetch an iterator of `BindingLens` objects, with the editable key bindings
    /// modified by the custom bindings, where appropriate.
    ///
    /// Editable bindings will be returned first, followed by any fixed bindings in the reverse
    /// order they were added.
    pub fn get_key_bindings(&self) -> impl Iterator<Item = BindingLens<'_>> {
        self.keystroke_matcher.get_bindings()
    }

    /// Returns the first registered binding with the given name, if one exists.
    pub fn get_binding_by_name(&self, name: &str) -> Option<BindingLens<'_>> {
        self.keystroke_matcher.get_binding_by_name(name)
    }

    /// Executes an updater callback against the current binding for
    /// a custom action.  A typical use of an updater is to update menu
    /// state based on the current contextual binding for a given custom action.
    pub fn update_custom_action_binding<F>(&self, custom_tag: CustomTag, mut updater: F)
    where
        F: FnMut(Option<BindingLens<'_>>),
    {
        updater(self.active_binding_for_custom_action(custom_tag));
    }

    /// Returns an optional binding for a custom action in the context of
    /// the currently focused window and view, if there is one.
    pub(super) fn active_binding_for_custom_action(
        &self,
        custom_tag: CustomTag,
    ) -> Option<BindingLens<'_>> {
        let window_id = self.windows().active_window()?;
        let view_id = self.focused_view_id(window_id)?;
        let contexts = self.contexts_for_window_and_view(window_id, view_id);
        self.binding_for_custom_action(custom_tag, contexts)
    }

    pub fn default_binding_for_custom_action(
        &self,
        custom_tag: CustomTag,
    ) -> Option<BindingLens<'_>> {
        self.keystroke_matcher
            .default_binding_for_custom_action(custom_tag)
    }

    pub fn description_for_custom_action(
        &self,
        custom_tag: CustomTag,
        description_for: DescriptionContext,
    ) -> Option<String> {
        // Resolve via the dynamic override (if any) so menu-bar callers pick
        // up state-dependent labels automatically. `in_context` would skip
        // the override and return the static fallback.
        self.default_binding_for_custom_action(custom_tag)
            .and_then(|binding| binding.description)
            .map(|s| s.resolve(self, description_for).into_owned())
    }

    /// Returns an optional binding for a custom action in any of the given contexts.
    /// Expects contexts to be ordered from more general (root view) to more specific (leafs)
    /// and uses the first context that matches.
    pub fn binding_for_custom_action(
        &self,
        custom_tag: CustomTag,
        contexts: Vec<Context>,
    ) -> Option<BindingLens<'_>> {
        contexts
            .into_iter()
            // The contexts are ordered top-down (from the root view down to the current view),
            // however the precedence order is bottom-up, so we need to reverse the iteration order
            .rev()
            .find_map(|context| {
                self.keystroke_matcher
                    .binding_for_custom_action_in_context(custom_tag, &context)
            })
    }

    pub(super) fn contexts_for_window_and_view(
        &self,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Vec<Context> {
        let responder_chain = self
            .presenter(window_id)
            .expect("Invalid window id")
            .borrow()
            .ancestors(view_id);
        match self.contexts_from_responder_chain(window_id, &responder_chain) {
            Ok(ctxs) => ctxs,
            Err(error) => {
                log::error!("Unable to fetch Key Bindings for View: {error}");
                Vec::new()
            }
        }
    }

    /// Fetch an iterator of editable bindings
    ///
    /// The triggers for those actions will be overwritten by any custom triggers
    ///
    /// Items will be returned in the reverse order they were registered, the most recently
    /// registered editable binding will have the highest precedence
    pub fn editable_bindings(&self) -> impl Iterator<Item = EditableBindingLens<'_>> {
        self.keystroke_matcher.editable_bindings()
    }

    /// Overrides any registered binding with has a [`Trigger::Custom`] to one that is keystroke
    /// based ([`Trigger::Keystrokes`]) using the provided `custom_to_keystroke` fn.
    pub fn convert_custom_triggers_to_keystroke_triggers(
        &mut self,
        custom_to_keystroke_fn: impl Fn(CustomTag) -> Option<Keystroke> + 'static,
    ) {
        self.keystroke_matcher
            .convert_custom_triggers_to_keystroke_triggers(custom_to_keystroke_fn);
    }

    pub fn register_default_keystroke_triggers_for_custom_actions(
        &mut self,
        custom_to_keystroke_fn: impl Fn(CustomTag) -> Option<Keystroke> + 'static,
    ) {
        self.keystroke_matcher
            .register_default_keystroke_triggers_for_custom_actions(custom_to_keystroke_fn);
    }

    /// Fetch an iterator of editable bindings that apply in a given window / view
    ///
    /// This will return all editable bindings, regardless of whether or not they have
    /// associated key bindings
    pub fn editable_bindings_for_view(
        &self,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Vec<EditableBindingLens<'_>> {
        let responder_chain = self
            .presenter(window_id)
            .expect("Invalid window id")
            .borrow()
            .ancestors(view_id);
        let contexts = match self.contexts_from_responder_chain(window_id, &responder_chain) {
            Ok(ctxs) => ctxs,
            Err(error) => {
                log::error!("Unable to fetch Key Bindings for View: {error}");
                return Vec::new();
            }
        };

        self.keystroke_matcher
            .editable_bindings()
            .filter(|action| contexts.iter().any(|ctx| action.in_context(ctx)))
            .collect()
    }

    /// return a list of contexts corresponding to a responder chain.
    pub(super) fn contexts_from_responder_chain(
        &self,
        window_id: WindowId,
        responder_chain: &[EntityId],
    ) -> Result<Vec<Context>> {
        let mut context_chain = Vec::new();
        for view_id in responder_chain {
            if let Some(view) = self
                .windows
                .get(&window_id)
                .and_then(|w| w.views.get(view_id))
            {
                let mut context = view.keymap_context(self);
                if self.platform_delegate.is_ime_open() {
                    context.set.insert("IMEOpen");
                }
                context_chain.push(context);
            } else {
                return Err(anyhow!(
                    "View {} in responder chain does not exist",
                    view_id
                ));
            }
        }
        Ok(context_chain)
    }

    pub(super) fn dispatch_standard_action(
        &mut self,
        action: StandardAction,
        window_id: WindowId,
        responder_chain: &[EntityId],
    ) -> Result<bool> {
        let mut context_chain = self.contexts_from_responder_chain(window_id, responder_chain)?;
        for (i, ctx) in context_chain.iter_mut().enumerate().rev() {
            let handled = match self.keystroke_matcher.match_standard(action, ctx) {
                MatchResult::Action(action) => self.dispatch_typed_action(
                    window_id,
                    &responder_chain[0..=i],
                    action.as_ref(),
                    log::Level::Info,
                ),
                _ => false,
            };
            // In this case, paste is the only valid interaction
            if handled && matches!(action, StandardAction::Paste) {
                self.dispatch_self_or_child_interacted_with(window_id, responder_chain);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Returns the responder chain for a given context and window ID.
    ///
    /// The "responder chain" is the view hierarchy to match against with bindings.
    /// This prefers the focused view and its ancestors; if no view is focused it
    /// dispatches to the root view.
    pub(super) fn get_responder_chain(&self, window_id: WindowId) -> Vec<EntityId> {
        if let Some(focused) = self.focused_view_id(window_id) {
            if let Some(presenter) = self.presenter(window_id) {
                presenter.borrow().ancestors(focused)
            } else {
                vec![]
            }
        } else if let Some(root) = self.root_view_id(window_id) {
            vec![root]
        } else {
            vec![]
        }
    }

    pub fn custom_action_bindings(&self) -> impl Iterator<Item = BindingLens<'_>> {
        self.keystroke_matcher.custom_action_bindings()
    }

    pub fn dispatch_custom_action<Action>(&mut self, action: Action, window_id: WindowId)
    where
        Action: Into<CustomTag> + Debug + Copy,
    {
        if self.key_bindings_enabled(window_id) {
            log::info!("Dispatching custom action {action:?}");
            let responder_chain = self.get_responder_chain(window_id);
            let res =
                self.dispatch_custom_action_internal(action.into(), window_id, &responder_chain);

            if let Ok(true) = res {
                self.dispatch_self_or_child_interacted_with(window_id, &responder_chain);
            }

            if let Err(error) = res {
                log::error!("error dispatching custom action: {error}");
            }
        } else {
            // We hit this case when the user is in the course of editing their keybindings.
            self.dispatch_custom_action_keystroke(action, window_id);
        }
    }

    /// Figures out what keystroke (if any) is bound to a custom action and dispatches it as a key event.
    /// This is used when the user is in the course of editing their keybindings and we need
    /// to get the raw keystroke for something that is currently bound to a custom action.
    ///
    /// This happens commonly with actions that are defined through Mac menu items (e.g. cmd-p for
    /// the command palette). If we want to map another keybinding to cmd-p while cmd-p is
    /// currently mapped to the command palette through a mac menu item, the only way to do it
    /// is by first handling the custom action, and then looking up the keystroke for the
    /// action, and then handling that as a raw key event.
    pub(super) fn dispatch_custom_action_keystroke<Action>(
        &mut self,
        action: Action,
        window_id: WindowId,
    ) -> bool
    where
        Action: Into<CustomTag> + Debug + Clone + Copy,
    {
        self.contexts_from_responder_chain(window_id, &self.get_responder_chain(window_id))
            .ok()
            .and_then(|contexts| self.binding_for_custom_action(action.into(), contexts))
            .and_then(|binding| match binding.trigger {
                Trigger::Keystrokes(keys) => keys.first().cloned(),
                Trigger::Custom(custom_tag) => self
                    .keystroke_matcher
                    .default_keystroke_trigger_for_custom_action(*custom_tag),
                _ => None,
            })
            .map(|keystroke| Event::KeyDown {
                keystroke: keystroke.clone(),
                chars: keystroke.key,
                details: Default::default(),
                is_composing: false,
            })
            .and_then(|key_event| {
                self.presenter(window_id).map(|presenter| {
                    log::info!("Dispatching key event {key_event:?} for custom action {action:?}");
                    self.handle_non_keybound_event(key_event, window_id, presenter.clone())
                        .handled
                })
            })
            .unwrap_or_default()
    }

    pub(super) fn dispatch_custom_action_internal(
        &mut self,
        action: CustomTag,
        window_id: WindowId,
        responder_chain: &[EntityId],
    ) -> Result<bool> {
        let mut context_chain = self.contexts_from_responder_chain(window_id, responder_chain)?;
        for (i, ctx) in context_chain.iter_mut().enumerate().rev() {
            let handled = match self.keystroke_matcher.match_custom(action, ctx) {
                MatchResult::Action(action) => self.dispatch_typed_action(
                    window_id,
                    &responder_chain[0..=i],
                    action.as_ref(),
                    log::Level::Info,
                ),
                _ => false,
            };
            if handled {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn dispatch_keystroke(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        keystroke: &Keystroke,
        is_composing: bool,
    ) -> Result<bool> {
        let mut context_chain = self.contexts_from_responder_chain(window_id, responder_chain)?;
        let mut pending = false;
        for (i, ctx) in context_chain.iter_mut().enumerate().rev() {
            if is_composing {
                ctx.set.insert("IMEOpen");
            }
            let handled = match self.keystroke_matcher.push_keystroke(
                keystroke.clone(),
                responder_chain[i],
                ctx,
            ) {
                MatchResult::None => false,
                MatchResult::Pending => {
                    pending = true;
                    false
                }
                MatchResult::Action(action) => self.dispatch_typed_action(
                    window_id,
                    &responder_chain[0..=i],
                    action.as_ref(),
                    log::Level::Info,
                ),
            };

            if handled {
                return Ok(true);
            }
        }
        Ok(pending)
    }
    #[allow(clippy::unwrap_in_result)]
    pub(crate) fn handle_window_closed(&mut self, window_id: WindowId) -> Option<ClosedWindowData> {
        // Defer ALL effect flushes until the window is fully removed from
        // self.windows. Without this guard, the flush that used to happen during
        // build_scene (in the test-only code path in on_window_invalidated) would
        // fire cascading effects (save_app, active_window_changed,
        // ClearMarkedText, etc.) while the window is still registered. Those
        // effects can trigger nested update_view calls on views in the
        // half-dead window, causing "circular view reference" panics.
        self.pending_flushes += 1;

        let view_ids = self
            .windows
            .get(&window_id)
            .map(|window| window.views.keys().copied().collect_vec());

        if let Some(view_ids) = &view_ids {
            for view_id in view_ids {
                // We're using .expect here even though the function returns an Option because we
                // know that the window exists at this point.
                if let Some(mut view) = self
                    .windows
                    .get_mut(&window_id)
                    .expect("Window exists")
                    .views
                    .remove(view_id)
                {
                    view.on_window_closed(self, window_id, *view_id);

                    self.windows
                        .get_mut(&window_id)
                        .expect("Window exists")
                        .views
                        .insert(*view_id, view);
                }
            }
        }

        let fullscreen_state = self
            .windows()
            .platform_window(window_id)
            .map(|window| window.fullscreen_state())
            .unwrap_or_default();

        if let (Some(bounds), Some(NextNewWindowsHasThisWindowsBoundsUponClose::Yes)) = (
            self.window_bounds.get(&window_id),
            self.next_window_bounds_map.get(&window_id),
        ) {
            // Store the bounds of the window that was closed so that the next window
            // we reopen is positioned there.
            self.next_window_bounds = *bounds;
        }
        WindowManager::handle(self).update(self, |windowing_state, ctx| {
            windowing_state.remove_window(window_id, ctx);
        });
        self.presenters.remove(&window_id);
        self.invalidation_callbacks.remove(&window_id);
        self.window_invalidations.remove(&window_id);
        autotracking::close_window(window_id);

        let mut subscriptions = HashMap::new();
        let mut observations = HashMap::new();
        // Back up view_to_window mappings so they can be restored if the window is reopened
        // via reopen_closed_window(). This preserves the view-to-window associations.
        let mut view_to_window_backup = HashMap::new();
        for view_id in view_ids.into_iter().flatten() {
            if let Some(subs) = self.subscriptions.remove(&view_id) {
                subscriptions.insert(view_id, subs);
            }
            if let Some(obs) = self.observations.remove(&view_id) {
                observations.insert(view_id, obs);
            }
            if let Some(view_window_id) = self.view_to_window.remove(&view_id) {
                debug_assert_eq!(
                    view_window_id, window_id,
                    "View {view_id:?} was in window {view_window_id:?} but expected {window_id:?} - was it transferred during close?"
                );
                view_to_window_backup.insert(view_id, view_window_id);
            }
        }

        if !self.window_bounds.contains_key(&window_id) {
            log::info!("missing window bounds!");
        }
        if !self.windows.contains_key(&window_id) {
            log::info!("missing core window data!");
        }
        let (Some(window), Some(bounds)) = (
            self.windows.remove(&window_id),
            self.window_bounds.remove(&window_id),
        ) else {
            log::error!("Closed a window that was missing underlying window data!");
            self.flush_effects();
            return None;
        };

        let result = Some(ClosedWindowData {
            window_id,
            window,
            subscriptions,
            observations,
            view_to_window: view_to_window_backup,
            bounds,
            fullscreen_state,
        });

        self.flush_effects();
        result
    }
}
