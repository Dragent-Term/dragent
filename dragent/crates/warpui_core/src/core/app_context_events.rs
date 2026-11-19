use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::AtomicI64;

use futures::future::join_all;
use futures::prelude::*;
use instant::Instant;
use itertools::Itertools;
use lazy_static::lazy_static;

use super::{autotracking, FocusObservation, PendingUnsubscribes, Subscription};
use crate::assets::asset_cache::{AssetCache, AssetHandle, AssetSource, AssetState};
use crate::assets::AssetProvider;
use crate::fonts::{self, ExternalFontFamily, FallbackFontModel, RequestedFallbackFontSource};
use crate::presenter::{CursorUpdate, DispatchedActionKind};
use crate::r#async::Timer;
use crate::{
    AnyModel, AnyView, Effect, Element, EntityId, Event, Presenter, SingletonEntity, TaskId,
    TypedActionView, View, ViewContext, ViewHandle, WindowId, WindowInvalidation,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;
use crate::core::EventMunger;
use crate::App;
use crate::RepaintTask;
use crate::RepaintTrigger;

impl AppContext {
    /// Simulates rendering a frame for the given window.
    #[cfg(test)]
    pub fn simulate_render_frame(&mut self, window_id: WindowId) {
        let Some(window) = self.windows().platform_window(window_id) else {
            return;
        };
        self.build_scene(window_id, window.as_ctx());
    }

    pub fn add_view<T, F>(&mut self, window_id: WindowId, build_view: F) -> ViewHandle<T>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.add_option_view(window_id, |ctx| Some(build_view(ctx)))
            .unwrap()
    }

    pub fn add_option_view<T, F>(
        &mut self,
        window_id: WindowId,
        build_view: F,
    ) -> Option<ViewHandle<T>>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> Option<T>,
    {
        let view_id = EntityId::new();
        self.pending_flushes += 1;
        let mut ctx = ViewContext::new(self, window_id, view_id);
        let handle = if let Some(view) = build_view(&mut ctx) {
            if let Some(window) = self.windows.get_mut(&window_id) {
                window.views.insert(view_id, Box::new(view));
            } else {
                panic!("Window does not exist");
            }
            self.view_to_window.insert(view_id, window_id);
            self.window_invalidations
                .entry(window_id)
                .or_default()
                .updated
                .insert(view_id);
            Some(ViewHandle::new(window_id, view_id, &self.ref_counts))
        } else {
            None
        };
        self.flush_effects();
        handle
    }

    /// Add a view that implements the `TypedAction` trait
    ///
    /// This will create the view as normal as well as register it's `handle_action` method in the
    /// typed_actions hash.
    ///
    /// Note: This is intended to be the replacement for `add_view` with the conversion to typed
    /// actions (and will subsequently be renamed to `add_view` once that is complete)
    pub fn add_typed_action_view<V, F>(
        &mut self,
        window_id: WindowId,
        build_view: F,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.add_typed_action_view_internal(window_id, build_view, None)
    }

    pub(super) fn add_typed_action_view_internal<V, F>(
        &mut self,
        window_id: WindowId,
        build_view: F,
        parent_view_id: Option<EntityId>,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.pending_flushes += 1;

        // Build the view and insert it into the window `views` map
        let view_id = EntityId::new();
        let mut ctx = ViewContext::new(self, window_id, view_id);
        let view = build_view(&mut ctx);
        let window = self
            .windows
            .get_mut(&window_id)
            .expect("Window does not exist");
        window.views.insert(view_id, Box::new(view));

        // Register in view_to_window mapping
        self.view_to_window.insert(view_id, window_id);

        // If a parent view ID was provided, add the view as a child of the parent
        if let Some(parent_view_id) = parent_view_id {
            if let Some(presenter) = self.presenter(window_id) {
                presenter.borrow_mut().set_parent(view_id, parent_view_id);
            }
            self.structural_child_to_parent
                .insert(view_id, parent_view_id);
            self.structural_parent_to_children
                .entry(parent_view_id)
                .or_default()
                .insert(view_id);
        }

        // Register the action handler for this view type (if it hasn't already been added)
        self.add_typed_action::<V>();
        // Mark the view as needing to be drawn
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .updated
            .insert(view_id);

        // Create the handle for managing the view lifetime
        let handle = ViewHandle::new(window_id, view_id, &self.ref_counts);
        self.flush_effects();
        handle
    }

    /// Transfers a single view from one window to another.
    ///
    /// This moves the view to the target window and updates all internal mappings.
    /// The view's subscriptions and observations will continue to work correctly
    /// because they now use dynamic window lookup.
    ///
    /// Returns `true` if the transfer was successful, `false` if the view doesn't exist.
    pub fn transfer_view_to_window(
        &mut self,
        view_id: EntityId,
        source_window_id: WindowId,
        target_window_id: WindowId,
    ) -> bool {
        if source_window_id == target_window_id {
            return true;
        }

        let Some(source_window) = self.windows.get_mut(&source_window_id) else {
            return false;
        };

        let Some(view) = source_window.views.remove(&view_id) else {
            return false;
        };

        // Mark the view as removed from the source window's invalidation set.
        // This tells the renderer to stop tracking this view in the source window.
        self.window_invalidations
            .entry(source_window_id)
            .or_default()
            .removed
            .insert(view_id);

        let Some(target_window) = self.windows.get_mut(&target_window_id) else {
            // Target window doesn't exist - roll back by putting the view back in source window
            if let Some(source_window) = self.windows.get_mut(&source_window_id) {
                source_window.views.insert(view_id, view);
            }
            return false;
        };

        target_window.views.insert(view_id, view);
        self.view_to_window.insert(view_id, target_window_id);

        self.window_invalidations
            .entry(target_window_id)
            .or_default()
            .updated
            .insert(view_id);

        // Remove from autotracking in the old window. The view will be automatically
        // added to autotracking in the new window when it renders, since render_view()
        // tracks dependencies based on the current window_id.
        autotracking::remove_view(source_window_id, view_id);

        if let Some(mut view) = self
            .windows
            .get_mut(&target_window_id)
            .and_then(|w| w.views.remove(&view_id))
        {
            view.on_window_transferred(source_window_id, target_window_id, self, view_id);
            if let Some(window) = self.windows.get_mut(&target_window_id) {
                window.views.insert(view_id, view);
            }
        }

        true
    }

    /// Transfers a view and all its descendant views from one window to another.
    ///
    /// This is useful when transferring a component like a tab that contains
    /// multiple nested views. The view tree is determined by the presenter's
    /// parent-child relationships.
    ///
    /// Returns the list of view IDs that were transferred.
    pub fn transfer_view_tree_to_window(
        &mut self,
        root_view_id: EntityId,
        source_window_id: WindowId,
        target_window_id: WindowId,
    ) -> Vec<EntityId> {
        if source_window_id == target_window_id {
            return vec![root_view_id];
        }

        let descendants = self
            .presenter(source_window_id)
            .map(|presenter| presenter.borrow().descendants(root_view_id))
            .unwrap_or_default();

        let mut transferred = Vec::with_capacity(descendants.len() + 1);

        if self.transfer_view_to_window(root_view_id, source_window_id, target_window_id) {
            transferred.push(root_view_id);
        }

        for view_id in descendants {
            if self.transfer_view_to_window(view_id, source_window_id, target_window_id) {
                transferred.push(view_id);
            }
        }

        self.transfer_structural_children(source_window_id, target_window_id, &mut transferred);

        transferred
    }

    /// Transfers structural children of already-transferred views.
    ///
    /// Uses `structural_parent_to_children` to walk from transferred parents
    /// to their children, avoiding iteration over all views in the source window.
    pub(super) fn transfer_structural_children(
        &mut self,
        source_window_id: WindowId,
        target_window_id: WindowId,
        transferred: &mut Vec<EntityId>,
    ) {
        let mut transferred_set: HashSet<EntityId> = transferred.iter().copied().collect();
        let mut to_process: Vec<EntityId> = transferred.clone();

        while let Some(parent_id) = to_process.pop() {
            let children: Vec<EntityId> = self
                .structural_parent_to_children
                .get(&parent_id)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();

            for child_id in children {
                if transferred_set.contains(&child_id) {
                    continue;
                }
                if self.transfer_view_to_window(child_id, source_window_id, target_window_id) {
                    transferred.push(child_id);
                    transferred_set.insert(child_id);
                    to_process.push(child_id);
                }
            }
        }
    }

    pub(super) fn remove_dropped_items(&mut self) {
        loop {
            let dropped_items = self.ref_counts.lock().take_dropped();
            if dropped_items.is_empty() {
                break;
            }

            for model_id in dropped_items.models {
                self.models.remove(&model_id);
                self.subscriptions.remove(&model_id);
                self.observations.remove(&model_id);
            }

            for (handle_window_id, view_id) in dropped_items.views {
                // Look up the current window from view_to_window mapping, which may differ
                // from handle_window_id if the view was transferred between windows.
                //
                // The entry may be missing when handle_window_closed() eagerly removes
                // view_to_window entries while the views are still alive in ClosedWindowData.
                // When those views' handles are later dropped, we correctly fall back to
                // handle_window_id.
                let current_window_id = self
                    .view_to_window
                    .remove(&view_id)
                    .unwrap_or(handle_window_id);

                // Focus the root view if the view being removed is focused
                if let Some(focused_view_id) = self.focused_view_id(current_window_id) {
                    if view_id == focused_view_id {
                        if let Some(root_view_id) = self.root_view_id(current_window_id) {
                            self.focus(current_window_id, root_view_id);
                        }
                    }
                }

                self.subscriptions.remove(&view_id);
                self.observations.remove(&view_id);
                if let Some(parent_id) = self.structural_child_to_parent.remove(&view_id) {
                    if let Some(children) = self.structural_parent_to_children.get_mut(&parent_id) {
                        children.remove(&view_id);
                        if children.is_empty() {
                            self.structural_parent_to_children.remove(&parent_id);
                        }
                    }
                }
                self.structural_parent_to_children.remove(&view_id);

                if let Some(window) = self.windows.get_mut(&current_window_id) {
                    self.window_invalidations
                        .entry(current_window_id)
                        .or_default()
                        .removed
                        .insert(view_id);
                    window.views.remove(&view_id);
                }

                autotracking::remove_view(current_window_id, view_id);
            }
        }
    }

    /// Fire all `observe_focus` callbacks registered for `observed_id`.
    ///
    /// `focused` is `true` when the view gained focus, `false` when it lost it.
    /// Follows the same remove-fire-re-insert pattern as `notify_model_observers`
    /// so that callbacks added during firing are picked up on the next flush.
    pub(super) fn fire_focus_observations(&mut self, observed_id: EntityId, focused: bool) {
        if let Some(observations) = self.focus_observations.remove(&observed_id) {
            for mut obs in observations {
                let alive = match &mut obs {
                    FocusObservation::FromView {
                        window_id: stored_window_id,
                        view_id,
                        callback,
                    } => {
                        // Honour window transfers — use the current window for this view.
                        let current_window_id = self
                            .view_to_window
                            .get(view_id)
                            .copied()
                            .unwrap_or(*stored_window_id);
                        if let Some(mut view) = self
                            .windows
                            .get_mut(&current_window_id)
                            .and_then(|w| w.views.remove(view_id))
                        {
                            callback(
                                view.as_any_mut(),
                                focused,
                                self,
                                current_window_id,
                                *view_id,
                            );
                            if let Some(window) = self.windows.get_mut(&current_window_id) {
                                window.views.insert(*view_id, view);
                            }
                            true
                        } else {
                            false // observer view was dropped — discard
                        }
                    }
                };
                if alive {
                    self.focus_observations
                        .entry(observed_id)
                        .or_default()
                        .push(obs);
                }
            }
        }
    }

    pub(super) fn flush_effects(&mut self) {
        self.pending_flushes -= 1;

        if !self.flushing_effects && self.pending_flushes == 0 {
            self.flushing_effects = true;

            self.remove_dropped_items();

            while let Some(effect) = self.pending_effects.pop_front() {
                match effect {
                    Effect::Event { entity_id, payload } => self.emit_event(entity_id, payload),
                    Effect::ModelNotification { model_id } => self.notify_model_observers(model_id),
                    Effect::ViewNotification { window_id, view_id } => {
                        self.notify_view_observers(window_id, view_id)
                    }
                    Effect::Focus { window_id, view_id } => {
                        self.focus(window_id, view_id);
                    }
                    Effect::TypedAction {
                        window_id,
                        view_id,
                        action,
                    } => {
                        self.dispatch_typed_action_for_view(window_id, view_id, action.as_ref());
                    }
                    Effect::GlobalAction {
                        name,
                        location,
                        arg,
                    } => {
                        self.dispatch_global_action_internal(name, location, arg.as_ref());
                    }
                }

                self.remove_dropped_items();
            }

            self.flushing_effects = false;
            self.update_windows();
        }
    }

    pub(super) fn update_windows(&mut self) {
        let invalidated_window_ids = self
            .window_invalidations
            .keys()
            .chain(autotracking::windows_with_invalidations().iter())
            .unique()
            .cloned()
            .collect_vec();
        for window_id in invalidated_window_ids {
            if let Some(mut callback) = self.invalidation_callbacks.remove(&window_id) {
                callback(window_id, self);
                self.invalidation_callbacks.insert(window_id, callback);
            }
        }
    }

    /// Collects invalidations for the given window from all sources: manual
    /// and autotracking.
    ///
    /// This operation is destructive: It will clear the caches for both manual and autotracked
    /// invalidations.
    pub(super) fn take_all_invalidations_for_window(
        &mut self,
        window_id: WindowId,
    ) -> WindowInvalidation {
        let mut invalidations = self
            .window_invalidations
            .remove(&window_id)
            .unwrap_or_default();
        invalidations
            .updated
            .extend(autotracking::take_invalidations_for_window(window_id));
        invalidations
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn simulate_window_event(
        &mut self,
        event: Event,
        window_id: WindowId,
        presenter: Rc<RefCell<Presenter>>,
    ) -> bool {
        self.handle_window_event(event, window_id, presenter)
            .handled
    }

    pub(super) fn handle_window_event(
        &mut self,
        mut event: Event,
        window_id: WindowId,
        presenter: Rc<RefCell<Presenter>>,
    ) -> crate::windowing::EventDispatchResult {
        let mut event_munger: Box<EventMunger> = Box::new(|_, _| {});
        std::mem::swap(&mut event_munger, &mut self.event_munger);
        // Give the app a chance to modify the event first. The closure had to be moved out of
        // `self.event_munger` to avoid a double-borrow error, so swapped it into `event_munger`
        // local variable, then swap it back after.
        event_munger(&mut event, self);
        std::mem::swap(&mut event_munger, &mut self.event_munger);

        let mut keystroke_handled = false;
        if self.key_bindings_enabled(window_id) {
            // Checks (and possibly dispatches) for actions with a matching keybinding
            if let Event::KeyDown {
                keystroke,
                is_composing,
                ..
            } = &event
            {
                if let Some(focused_view_id) = self.focused_view_id(window_id) {
                    let responder_chain = presenter.borrow().ancestors(focused_view_id);
                    match self.dispatch_keystroke(
                        window_id,
                        &responder_chain,
                        keystroke,
                        *is_composing,
                    ) {
                        Ok(handled) => {
                            keystroke_handled = handled;
                        }
                        Err(error) => {
                            log::error!("error dispatching keystroke: {error}");
                        }
                    }
                }
            }
        }

        let mut result = crate::windowing::EventDispatchResult::default();
        if !keystroke_handled {
            result = self.handle_non_keybound_event(event.clone(), window_id, presenter.clone());
        }

        let handled = keystroke_handled || result.handled;

        // Only dispatch `self_or_child_interacted_with` if:
        // (1) the event was handled by a view in the responder chain, and
        // (2) the event is a valid interaction (we exclude mouse and scroll movements to reduce noise)
        if handled && !matches!(event, Event::MouseMoved { .. } | Event::ScrollWheel { .. }) {
            if let Some(focused_view_id) = self.focused_view_id(window_id) {
                let responder_chain = presenter.borrow().ancestors(focused_view_id);
                self.dispatch_self_or_child_interacted_with(window_id, &responder_chain);
            }
        }

        crate::windowing::EventDispatchResult {
            handled,
            soft_keyboard_requested: result.soft_keyboard_requested,
        }
    }

    /// Schedules an asynchronous task for each delayed notify. Removes any existing notify jobs that've been cancelled.
    pub fn manage_delayed_repaint_timers(&mut self, window_id: WindowId, repaint_at: Instant) {
        // Avoid creating new timers if a timer with a closer repaint time for
        // the same window already exists.
        if self.repaint_tasks.iter().any(|(_, task)| {
            task.window_id == window_id &&
            matches!(task.repaint_trigger, RepaintTrigger::Timer { instant } if instant <= repaint_at)
        }) {
            return;
        }

        let weak_app = self.weak_self.clone();

        let task_id = TaskId::new();
        let task = self.foreground.spawn(async move {
            Timer::after(repaint_at.saturating_duration_since(Instant::now())).await;
            if let Some(app) = weak_app.upgrade() {
                let mut app = app.borrow_mut();

                // If the timer is no longer in repaint_tasks, it was cancelled.
                if app.repaint_tasks.remove(&task_id).is_some() {
                    app.window_invalidations
                        .entry(window_id)
                        .or_default()
                        .redraw_requested = true;
                    app.update_windows();
                }
            }
        });

        self.repaint_tasks.insert(
            task_id,
            RepaintTask {
                task,
                window_id,
                repaint_trigger: RepaintTrigger::Timer {
                    instant: repaint_at,
                },
            },
        );
    }

    /// Schedules an asynchronous task for each delayed notify. Removes any existing notify jobs that've been cancelled.
    pub fn manage_pending_assets(
        &mut self,
        window_id: WindowId,
        pending_assets: HashSet<AssetHandle>,
    ) {
        pending_assets.into_iter().for_each(|pending_asset| {
            // Avoid creating new repaint tasks if this (window, asset) pairing already has a future.
            if self.repaint_tasks.iter().any(|(_, task)| {
                task.window_id == window_id && matches!(&task.repaint_trigger, RepaintTrigger::AssetLoaded { asset_handle } if asset_handle == &pending_asset)
            }) {
                return;
            }

            let asset_cache = AssetCache::as_ref(self);
            let Some(asset_loaded_future) = pending_asset.when_loaded(asset_cache) else {
                return;
            };
            let weak_app = self
                .weak_self
                .clone();

            let task_id = TaskId::new();

            let task = self.foreground.spawn(async move {
                asset_loaded_future.await;

                let Some(app) = weak_app.upgrade() else {
                    return;
                };
                let mut app = app.borrow_mut();

                // If the timer is no longer in repaint_tasks, it was cancelled.
                if app.repaint_tasks.remove(&task_id).is_some() {
                    app.window_invalidations
                        .entry(window_id)
                        .or_default()
                        .redraw_requested = true;
                    app.update_windows();
                }
            });

            self.repaint_tasks.insert(
                task_id,
                RepaintTask {
                    task,
                    window_id,
                    repaint_trigger: RepaintTrigger::AssetLoaded {
                        asset_handle: pending_asset,
                    },
                },
            );
        })
    }

    pub(super) fn load_fallback_family_and_redraw(
        &mut self,
        window_id: WindowId,
        fallback_family: ExternalFontFamily,
        request_sources: Vec<RequestedFallbackFontSource>,
        asset_sources: Vec<AssetSource>,
    ) {
        let asset_cache = AssetCache::as_ref(self);

        let mut font_family_bytes: Vec<Vec<u8>> = Vec::new();
        // Get the raw font bytes from the loaded assets.
        for asset in asset_sources
            .into_iter()
            .map(|source| asset_cache.load_asset::<fonts::FontBytes>(source))
        {
            match asset {
                AssetState::Loaded { data } => {
                    // TODO(PLAT-746): Update API for loading a font family to
                    // take an `Rc`, so we don't have to clone the font data.
                    font_family_bytes.push(data.0.clone());
                }
                AssetState::Evicted => {
                    log::warn!("Unable to load requested fallback font because it was evicted");
                }
                AssetState::FailedToLoad(e) => {
                    log::warn!("Unable to load requested fallback font: {e:?}");
                }
                AssetState::Loading { .. } => {
                    log::error!("Fallback font asset should not be in a loading state");
                }
            }
        }

        // Early return if we were not able to load any of the font assets for
        // this family.
        // TODO(PLAT-760): Implement a retry mechanism if we load some but not
        // all of the font assets for the family. Currently we will load the
        // partial font family into the cache and will not try loading again.
        if font_family_bytes.is_empty() {
            log::warn!(
                "Failed to load any fonts for the family {}",
                &fallback_family.name
            );
            return;
        }

        // Insert the family into the font cache.
        if !self
            .font_cache()
            .is_fallback_family_loaded(fallback_family.name)
        {
            if let Err(e) = fonts::Cache::handle(self).update(self, |cache, _| {
                cache.load_fallback_family_from_bytes(fallback_family, font_family_bytes)
            }) {
                log::warn!("Unable to load fallback family from bytes: {e:?}");
                return;
            }

            // TODO(PLAT-747): Ideally the font cache itself is a model and can
            // handle emitting these events.
            FallbackFontModel::handle(self).update(self, |model, ctx| {
                model.loaded_fallback_font(ctx);
            });
        }

        // Clear the glyph/layout caches before redrawing. These caches must
        // be cleared even if the fallback family has previously been loaded.
        for source in request_sources {
            match source {
                RequestedFallbackFontSource::GlyphForChar(key) => {
                    fonts::Cache::handle(self).update(self, |cache, _| {
                        cache.remove_glyphs_by_char_entry(key);
                    });
                }
                RequestedFallbackFontSource::Line(key) => {
                    if let Some(presenter) = self.presenter(window_id) {
                        presenter.borrow().text_layout_cache().remove_line(&key);
                    }
                }
                RequestedFallbackFontSource::TextFrame(key) => {
                    if let Some(presenter) = self.presenter(window_id) {
                        presenter
                            .borrow()
                            .text_layout_cache()
                            .remove_text_frame(&key);
                    }
                }
            }
        }

        // Trigger a redraw on the window.
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .redraw_requested = true;
        self.update_windows();
    }

    pub(crate) fn load_requested_fallback_families(&mut self, window_id: WindowId) {
        let Some(fallback_font_source_provider) = self.fallback_font_source_provider.as_ref()
        else {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                log::warn!(
                    "No fallback_font_source_provider registered; cannot load fallback fonts"
                );
            });

            return;
        };

        let requested_fallback_families =
            fonts::Cache::as_ref(self).take_requested_fallback_families();
        let asset_cache = AssetCache::as_ref(self);

        for (fallback_family, request_sources) in requested_fallback_families {
            let mut asset_sources = Vec::with_capacity(fallback_family.font_urls.len());
            let mut futures = Vec::with_capacity(fallback_family.font_urls.len());
            for fallback_font in fallback_family.font_urls.as_ref() {
                let asset_source = fallback_font_source_provider(fallback_font);
                let asset = asset_cache.load_asset::<fonts::FontBytes>(asset_source.clone());

                // If the font is loading, collect the future so we can wait
                // for it to resolve.
                if let AssetState::Loading { ref handle } = asset {
                    if let Some(future) = handle.when_loaded(asset_cache) {
                        futures.push(future);
                    }
                }
                // We need to load the asset again once the future has resolved,
                // so collect the asset source.
                asset_sources.push(asset_source);
            }

            let weak_self_clone = self.weak_self.clone();
            self.foreground
                .spawn(async move {
                    join_all(futures).await;

                    let Some(app) = weak_self_clone.upgrade() else {
                        return;
                    };
                    let mut app = app.borrow_mut();

                    app.load_fallback_family_and_redraw(
                        window_id,
                        fallback_family,
                        request_sources,
                        asset_sources,
                    );
                })
                .detach();
        }
    }

    pub(super) fn handle_non_keybound_event(
        &mut self,
        event: Event,
        window_id: WindowId,
        presenter: Rc<RefCell<Presenter>>,
    ) -> crate::windowing::EventDispatchResult {
        let log_level = match &event {
            // If the action comes from the MouseMoved or ScrollWheel events,
            // dispatch it at the `trace` log level so it doesn't clutter the
            // logs by default.
            Event::MouseMoved { .. } | Event::ScrollWheel { .. } => log::Level::Trace,
            _ => {
                // Update last user action timestamp for non-hover events
                App::record_last_active_timestamp();
                log::Level::Info
            }
        };
        let dispatch_result = presenter.borrow_mut().dispatch_event(event, self);

        // Iterate through all timers, and put a task onto the main thread for each
        // to call back and notify the view when the timer triggers.
        for (timer_id, view_to_notify) in dispatch_result.notify_timers_to_set.iter() {
            let (timer_id, view_to_notify) = (*timer_id, *view_to_notify);
            let weak_app = self.weak_self.clone();
            let task = self.foreground.spawn(async move {
                Timer::after(view_to_notify.notify_at - Instant::now()).await;
                if let Some(app) = weak_app.upgrade() {
                    let mut app = app.borrow_mut();
                    if app.notify_tasks.remove(&timer_id).is_some() {
                        log::info!(
                            "notifying view observers and updating windows for timer id {timer_id}"
                        );
                        app.notify_view_observers(window_id, view_to_notify.view_id);
                        // Note that for the hoverable delay this triggers the appropriate
                        // behavior because the window stores the last mouse moved event and
                        // dispatches it on every window redraw.
                        // See the on_window_invalidated callback
                        app.update_windows();
                    }
                }
            });
            self.notify_tasks.insert(timer_id, task);
        }

        for timer_id in dispatch_result.notify_timers_to_clear {
            // Dropping the task should be sufficient to cancel it.
            self.notify_tasks.remove(&timer_id);
        }

        for view_id in dispatch_result.notified {
            self.notify_view_observers(window_id, view_id)
        }

        for action in dispatch_result.actions.into_iter().rev() {
            let responder_chain = presenter.borrow().ancestors(action.view_id);
            match action.kind {
                DispatchedActionKind::Legacy { name, arg } => {
                    self.dispatch_action(
                        window_id,
                        &responder_chain,
                        name,
                        arg.as_ref(),
                        log_level,
                    );
                }
                DispatchedActionKind::Typed(action) => {
                    self.dispatch_typed_action(
                        window_id,
                        &responder_chain,
                        action.as_ref(),
                        log_level,
                    );
                }
            }
        }

        match dispatch_result.cursor_update {
            Some(CursorUpdate::Set {
                cursor, view_id, ..
            }) => self.set_cursor_shape(cursor, window_id, view_id),
            Some(CursorUpdate::Reset) => self.reset_cursor(),
            _ => {}
        }

        crate::windowing::EventDispatchResult {
            handled: dispatch_result.handled,
            soft_keyboard_requested: dispatch_result.soft_keyboard_requested,
        }
    }

    pub(super) fn emit_event(&mut self, entity_id: EntityId, payload: Box<dyn Any>) {
        if let Some(subscriptions) = self.subscriptions.remove(&entity_id) {
            // Start tracking unsubscribes for this entity. Unsubscribes called from inside
            // callbacks are deferred and processed at the end, avoiding O(N²) tombstone scanning.
            //
            // Note: All callbacks that existed when the event started processing will be called.
            // Unsubscribe only prevents re-insertion (i.e., it affects future events, not the
            // current one).
            debug_assert!(
                self.pending_unsubscribes.is_none(),
                "pending_unsubscribes should be None at start of emit_event"
            );
            self.pending_unsubscribes = Some(PendingUnsubscribes {
                entity_id,
                keys: HashSet::new(),
            });

            let mut to_reinsert = Vec::new();

            for mut subscription in subscriptions {
                let alive = match &mut subscription {
                    Subscription::FromModel { model_id, callback } => {
                        if let Some(mut model) = self.models.remove(model_id) {
                            callback(model.as_any_mut(), payload.as_ref(), self, *model_id);
                            self.models.insert(*model_id, model);
                            true
                        } else {
                            false
                        }
                    }
                    Subscription::FromView {
                        window_id: stored_window_id,
                        view_id,
                        callback,
                    } => {
                        let current_window_id = self
                            .view_to_window
                            .get(view_id)
                            .copied()
                            .unwrap_or(*stored_window_id);
                        if let Some(mut view) = self
                            .windows
                            .get_mut(&current_window_id)
                            .and_then(|window| window.views.remove(view_id))
                        {
                            callback(
                                view.as_any_mut(),
                                payload.as_ref(),
                                self,
                                current_window_id,
                                *view_id,
                            );

                            // XXX We need to check whether window is None
                            // once again because callback could
                            // potentially erase the window (i.e. if we
                            // handle the Terminal exit event)
                            if let Some(window) = self.windows.get_mut(&current_window_id) {
                                window.views.insert(*view_id, view);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    Subscription::FromApp { callback } => {
                        callback(payload.as_ref(), self, entity_id);
                        true
                    }
                };

                if alive {
                    to_reinsert.push(subscription);
                }
            }

            // Process any pending unsubscribes collected during callback execution.
            let pending = self.pending_unsubscribes.take().unwrap();

            // Remove unsubscribed entries from the subscriptions that existed when this event started.
            if !pending.keys.is_empty() {
                to_reinsert.retain(|sub| {
                    sub.subscription_key()
                        .is_none_or(|key| !pending.keys.contains(&key))
                });
            }

            // Collect any new subscriptions added during callback execution (e.g., a callback
            // that subscribes a new handler to this same entity).
            let mut final_subs = self.subscriptions.remove(&entity_id).unwrap_or_default();
            final_subs.extend(to_reinsert);

            // Re-insert surviving subscriptions.
            if !final_subs.is_empty() {
                self.subscriptions.insert(entity_id, final_subs);
            }
        }
    }
}
