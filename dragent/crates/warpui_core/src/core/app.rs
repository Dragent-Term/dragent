use std::any::{Any, TypeId};
use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::rc::{self, Rc};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Result};
use chrono::Utc;
use futures::prelude::*;
use instant::Instant;
use itertools::Itertools;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use pathfinder_geometry::rect::RectF;
use rustc_hash::FxHashMap;

use super::{
    autotracking, ActionCallback, FocusObservation, GlobalActionCallback, GlobalShortcut,
    InvalidationCallback, Observation, PendingUnsubscribes, RefCounts, Subscription, TaskCallback,
    TypedActionCallback, ViewType,
};
use crate::accessibility::AccessibilityVerbosity;
use crate::actions::StandardAction;
use crate::app_focus_telemetry::AppFocusInfo;
use crate::assets::asset_cache::{AssetHandle, AssetSource};
use crate::assets::AssetProvider;
use crate::core::{ActionType, Window};
use crate::fonts::{self};
use crate::keymap::{CustomTag, Keystroke, Matcher};
use crate::modals::{ModalId, PlatformModalResponseData};
use crate::platform::app::TerminationResult;
use crate::platform::{
    self, FullscreenState, MicrophoneAccessState, SystemTheme, WindowBounds, WindowStyle,
};
use crate::r#async::executor::{self, Background, Foreground, ForegroundTask};
use crate::r#async::{block_on, FutureId};
use crate::windowing::{self, WindowManager};
use crate::{
    assets, rendering, Action, AddSingletonModel, AddWindowOptions, AnyModel, AnyModelHandle,
    AnyView, Effect, Element, Entity, EntityId, Event, GetSingletonModelHandle, ModelAsRef,
    ModelContext, ModelHandle, NextNewWindowsHasThisWindowsBoundsUponClose, Presenter, ReadModel,
    ReadView, SingletonEntity, SpawnedFuture, TaskId, TypedActionView, UpdateModel, UpdateView,
    View, ViewAsRef, ViewContext, ViewHandle, WindowId, WindowInvalidation, ZoomFactor,
};

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

#[derive(Clone)]

pub struct App(Rc<RefCell<AppContext>>);

impl App {
    pub fn test<A: assets::AssetProvider, T: 'static, F: Future<Output = T> + 'static>(
        asset_provider: A,
        f: impl FnOnce(App) -> F,
    ) -> T {
        log::info!("Starting test app...");
        let platform = Box::new(platform::test::AppDelegate::new().unwrap());
        let window_manager = Box::new(platform::test::WindowManager::new());
        let font_cache = Box::new(platform::test::FontDB);
        let executor = Rc::new(executor::Foreground::test());
        let app = Self(Rc::new(RefCell::new(AppContext::with_foreground_executor(
            executor.clone(),
            platform,
            window_manager,
            font_cache,
            Box::new(asset_provider),
            true, /* is_unit_test */
        ))));
        {
            let mut app_mut = app.0.borrow_mut();
            app_mut.weak_self = Rc::downgrade(&app.0);
        }
        log::info!("Executing test...");
        let result = block_on(executor.run(f(app)));
        log::info!("Test complete; terminating application...");
        result
    }

    pub fn new(
        platform_delegate: Box<dyn platform::Delegate>,
        window_manager: Box<dyn platform::WindowManager>,
        font_db: Box<dyn platform::FontDB>,
        asset_provider: Box<dyn AssetProvider>,
    ) -> Result<Self> {
        let app = Self(Rc::new(RefCell::new(AppContext::new(
            platform_delegate,
            window_manager,
            font_db,
            asset_provider,
        )?)));
        app.0.borrow_mut().weak_self = Rc::downgrade(&app.0);
        Ok(app)
    }

    pub(crate) fn can_borrow_mut(&self) -> bool {
        self.0.try_borrow_mut().is_ok()
    }

    pub fn has_window_invalidations(&self, window_id: WindowId) -> bool {
        self.0.borrow().has_window_invalidations(window_id)
    }

    pub fn window_bounds(&self, window_id: &WindowId) -> Option<RectF> {
        self.0.borrow().window_bounds(window_id)
    }

    pub fn next_window_bounds_and_style(&self) -> (WindowBounds, WindowStyle) {
        self.0.borrow().next_window_bounds_and_style()
    }

    pub fn focused_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.0.borrow().focused_view_id(window_id)
    }

    pub fn presenter(&self, window_id: WindowId) -> Option<Rc<RefCell<Presenter>>> {
        self.0.borrow().presenter(window_id)
    }

    pub fn is_window_open(&self, window_id: WindowId) -> bool {
        self.0.borrow().is_window_open(window_id)
    }

    pub fn foreground_executor(&self) -> Rc<Foreground> {
        self.0.borrow().foreground.clone()
    }

    pub fn background_executor(&self) -> Arc<Background> {
        self.0.borrow().background.clone()
    }

    pub fn set_a11y_verbosity(&mut self, verbosity: AccessibilityVerbosity) {
        self.0.borrow_mut().set_a11y_verbosity(verbosity);
    }

    pub fn on_window_invalidated<F: 'static + FnMut(WindowId, &mut AppContext)>(
        &self,
        window_id: WindowId,
        callback: F,
    ) {
        self.0
            .borrow_mut()
            .on_window_invalidated(window_id, callback);
    }

    /// Adds an action with a given handler that is executed when the action is dispatched. The
    /// action handler should return whether the action was handled. If it returns false this means
    /// a parent view that listens on the same action name will receive the action.
    pub fn add_action<S, V, T, F>(&self, name: S, handler: F)
    where
        S: Into<String>,
        V: View,
        T: Any,
        F: 'static + FnMut(&mut V, &T, &mut ViewContext<V>) -> bool,
    {
        self.0.borrow_mut().add_action(name, handler);
    }

    pub fn add_global_action<S, T, F>(&self, name: S, handler: F)
    where
        S: Into<String>,
        T: 'static + Any,
        F: 'static + FnMut(&T, &mut AppContext),
    {
        self.0.borrow_mut().add_global_action(name, handler);
    }

    pub fn dispatch_action<T: 'static + Any>(
        &self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        name: &str,
        arg: T,
    ) {
        self.0.borrow_mut().dispatch_action(
            window_id,
            responder_chain,
            name,
            &arg,
            log::Level::Info,
        );
    }

    pub fn dispatch_typed_action(
        &self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        action: &dyn Action,
    ) {
        self.0.borrow_mut().dispatch_typed_action(
            window_id,
            responder_chain,
            action,
            // One per key press and per mouse move over a terminal: at Info
            // this formatted and wrote a line on the main thread each time.
            log::Level::Debug,
        );
    }

    pub fn last_active_timestamp() -> i64 {
        LAST_USER_ACTION_UNIX_TIMESTAMP.load(Ordering::SeqCst)
    }

    pub fn record_last_active_timestamp() {
        LAST_USER_ACTION_UNIX_TIMESTAMP.fetch_max(Utc::now().timestamp(), Ordering::SeqCst);
    }

    pub fn dispatch_global_action<T: 'static + Any>(&self, name: &str, arg: T) {
        self.0.borrow_mut().dispatch_global_action(name, &arg);
    }

    pub fn dispatch_standard_action(&mut self, window_id: WindowId, action: StandardAction) {
        log::info!("Dispatching standard action {action:?}");
        self.update(move |ctx| {
            let responder_chain = ctx.get_responder_chain(window_id);
            let res = ctx.dispatch_standard_action(action, window_id, &responder_chain);
            if let Err(error) = res {
                log::error!("error dispatching standard action: {error}");
            }
        });
    }

    pub fn dispatch_custom_action<Action>(&mut self, action: Action, window_id: WindowId)
    where
        Action: Into<CustomTag> + Debug + Copy,
    {
        self.0
            .borrow_mut()
            .dispatch_custom_action(action, window_id);
    }

    pub fn key_bindings_dispatching_enabled(&mut self, window_id: WindowId) -> bool {
        self.0.borrow_mut().key_bindings_enabled(window_id)
    }

    pub fn dispatch_keystroke(
        &self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        keystroke: &Keystroke,
        is_composing: bool,
    ) -> Result<bool> {
        let mut state = self.0.borrow_mut();
        state.dispatch_keystroke(window_id, responder_chain, keystroke, is_composing)
    }

    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        let mut state = self.0.borrow_mut();
        state.pending_flushes += 1;
        let handle = state.add_model(build_model);
        state.flush_effects();
        handle
    }

    pub fn add_singleton_model<T, F>(&self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        let mut state = self.0.borrow_mut();
        state.pending_flushes += 1;
        let handle = state.add_singleton_model(build_model);
        state.flush_effects();
        handle
    }

    pub fn get_singleton_model_handle<T>(&self) -> ModelHandle<T>
    where
        T: SingletonEntity,
    {
        self.0.borrow().get_singleton_model_handle()
    }

    pub fn add_window_with_bounds<T, F>(
        &mut self,
        style: WindowStyle,
        bounds: WindowBounds,
        build_root_view: F,
    ) -> (WindowId, ViewHandle<T>)
    where
        T: View + TypedActionView,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.0.borrow_mut().add_window(
            AddWindowOptions {
                window_style: style,
                window_bounds: bounds,
                ..Default::default()
            },
            build_root_view,
        )
    }

    pub fn add_window<T, F>(
        &mut self,
        style: WindowStyle,
        build_root_view: F,
    ) -> (WindowId, ViewHandle<T>)
    where
        T: View + TypedActionView,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.add_window_with_bounds(style, WindowBounds::Default, build_root_view)
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.0.borrow().window_ids().collect()
    }

    pub fn root_view<T: View>(&self, window_id: WindowId) -> Option<ViewHandle<T>> {
        self.0.borrow().root_view(window_id)
    }

    pub fn root_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.0.borrow().root_view_id(window_id)
    }

    pub fn add_view<T, F>(&mut self, window_id: WindowId, build_view: F) -> ViewHandle<T>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        let mut state = self.0.borrow_mut();
        state.pending_flushes += 1;
        let handle = state.add_view(window_id, build_view);
        state.flush_effects();
        handle
    }

    pub fn add_typed_action_view<V, F>(
        &mut self,
        window_id: WindowId,
        build_view: F,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.0
            .borrow_mut()
            .add_typed_action_view(window_id, build_view)
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
        let mut state = self.0.borrow_mut();
        state.pending_flushes += 1;
        let handle = state.add_option_view(window_id, build_view);
        state.flush_effects();
        handle
    }

    pub fn read<T, F: FnOnce(&AppContext) -> T>(&self, callback: F) -> T {
        callback(&self.0.borrow())
    }

    pub fn update<T, F: FnOnce(&mut AppContext) -> T>(&mut self, callback: F) -> T {
        let mut state = self.0.borrow_mut();
        state.pending_flushes += 1;
        let result = callback(&mut state);
        state.flush_effects();
        result
    }

    // Returns objects in the order they were added
    pub fn views_of_type<T: View>(&self, window_id: WindowId) -> Option<Vec<ViewHandle<T>>> {
        let views = self.0.borrow().views_of_type(window_id);
        // Since iter yields arbitrary order, sort by id
        match views {
            Some(mut views) => {
                views.sort_by_key(|view| view.id());
                Some(views)
            }
            None => None,
        }
    }

    // Returns objects in the order they were added
    pub fn models_of_type<M: Entity>(&self) -> Vec<ModelHandle<M>> {
        let mut models = self.0.borrow().models_of_type();
        // Since iter yields arbitrary order, sort by id
        models.sort_by_key(|models| models.id());
        models
    }

    #[cfg(test)]
    pub fn finish_pending_tasks(&self) -> impl Future<Output = ()> {
        self.0.borrow().finish_pending_tasks()
    }

    pub(crate) fn as_mut(&mut self) -> AppContextRefMut<'_> {
        AppContextRefMut::new(self.0.as_ref().borrow_mut())
    }

    pub fn termination_result(self) -> Option<TerminationResult> {
        self.0.borrow_mut().termination_result.take()
    }
}

impl ModelAsRef for App {
    // Unsupported for the same reasons as [`ViewAsRef`] being
    // unsupported for [`App`].
    fn model<T: Entity>(&self, _: &ModelHandle<T>) -> &T {
        unimplemented!("Read from [`App::read_model`] instead");
    }
}

impl ReadModel for App {
    fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
    where
        T: Entity,
        F: FnOnce(&T, &AppContext) -> S,
    {
        let state = self.0.borrow();
        state.read_model(handle, read)
    }
}

impl UpdateModel for App {
    fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        self.as_mut().update_model(handle, update)
    }
}

impl UpdateView for App {
    fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
    where
        T: View,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        self.as_mut().update_view(handle, update)
    }
}

impl ReadView for App {
    fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
    where
        T: View,
        F: FnOnce(&T, &AppContext) -> S,
    {
        let state = self.0.borrow();
        state.read_view(handle, read)
    }
}

impl ViewAsRef for App {
    // This is unimplemented because we would need to do
    // some borrow-gymnastics, like returning a wrapper type
    // that internally holds the reference, to get around a
    // "returned reference to temporary value" error.
    //
    // That effort is currently unjustified because we want
    // to ideally strip the *AsRef, Read* and Update* implementations
    // from [`App`].
    fn view<T: View>(&self, _handle: &ViewHandle<T>) -> &T {
        unimplemented!("Read from [`App::read_view`] instead");
    }

    fn try_view<T: View>(&self, _handle: &ViewHandle<T>) -> Option<&T> {
        unimplemented!("Read from [`App::read_view`] instead");
    }
}

impl AddSingletonModel for App {
    fn add_singleton_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut super::ModelContext<T>) -> T,
    {
        self.0.borrow_mut().add_singleton_model(build_model)
    }
}

impl GetSingletonModelHandle for App {
    fn get_singleton_model_handle<T: SingletonEntity>(&self) -> ModelHandle<T> {
        self.0.borrow_mut().get_singleton_model_handle()
    }
}

/// A wrapper around a mutably-borrowed app context.
///
/// This is necessary in order to ensure that all pending effects are flushed
/// before we return the mutable borrow.
pub(crate) struct AppContextRefMut<'a>(RefMut<'a, AppContext>);

impl<'a> AppContextRefMut<'a> {
    fn new(mut inner: RefMut<'a, AppContext>) -> Self {
        inner.pending_flushes += 1;
        Self(inner)
    }
}

impl std::ops::Drop for AppContextRefMut<'_> {
    fn drop(&mut self) {
        self.0.flush_effects();
    }
}

impl<'a> std::ops::Deref for AppContextRefMut<'a> {
    type Target = RefMut<'a, AppContext>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for AppContextRefMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct RepaintTask {
    pub task: ForegroundTask,

    /// The window from which this comes
    pub window_id: WindowId,

    pub repaint_trigger: RepaintTrigger,
}

pub enum RepaintTrigger {
    Timer { instant: Instant },
    AssetLoaded { asset_handle: AssetHandle },
}

pub(super) type EventMunger = dyn Fn(&mut Event, &mut AppContext);

pub type DrawFrameErrorCallback = dyn Fn(&mut AppContext, WindowId);

pub type FrameDrawnCallback = dyn Fn(&mut AppContext, WindowId);

pub type BeforeOpenUrlCallback = dyn Fn(&str, &AppContext) -> String;

pub struct AppContext {
    /////////////////////////
    // Fields from AppContext
    /////////////////////////
    pub(super) models: HashMap<EntityId, Box<dyn AnyModel>>,
    /// A mapping from type ID -> handle to the single global model of that
    /// type.  The handle is a strong reference, ensuring the model will not be
    /// dropped during the lifetime of the application.
    ///
    /// We use an FxHashMap here because `TypeId` hashes as a u64, and FxHasher
    /// is the fastest commonly-used hasher for this type.
    pub(super) singleton_models: FxHashMap<TypeId, AnyModelHandle>,
    pub(super) last_frame_position_cache: HashMap<WindowId, crate::presenter::PositionCache>,
    pub(super) windows: HashMap<WindowId, Window>,
    pub(super) ref_counts: Arc<Mutex<RefCounts>>,
    pub(super) platform_delegate: Box<dyn platform::Delegate>,

    ////////////////////////////////
    // Fields from MutableAppContext
    ////////////////////////////////
    pub(super) actions: HashMap<TypeId, HashMap<String, Vec<Box<ActionCallback>>>>,
    /// Map of typed actions to their internal handler functions
    ///
    /// We use a nested HashMap to key on both the ActionType and the ViewType, which allows us to
    /// efficiently look up the appropriate handler while iterating the responder_chain on dispatch
    ///
    /// Safety Note: The `TypedActionCallback` must only be called with parameters that match the
    /// type keys, as it requires the values to appropriately downcast.
    pub(super) typed_actions: HashMap<ActionType, HashMap<ViewType, Box<TypedActionCallback>>>,
    pub(super) presenters: HashMap<WindowId, Rc<RefCell<Presenter>>>,
    /// Configuration options related to rendering of the application.
    pub(super) rendering_config: rendering::Config,
    pub(super) global_actions: HashMap<String, Vec<Box<GlobalActionCallback>>>,
    pub(super) keystroke_matcher: Matcher,
    pub(super) next_task_id: usize,
    pub(super) weak_self: rc::Weak<RefCell<Self>>,
    pub(super) subscriptions: HashMap<EntityId, Vec<Subscription>>,
    pub(super) observations: HashMap<EntityId, Vec<Observation>>,
    /// Tracks pending unsubscribes during event emission.
    /// When `emit_event` is processing callbacks, unsubscribes are deferred here to avoid
    /// O(N²) tombstone scanning. The unsubscribes are processed at the end of event emission.
    pub(super) pending_unsubscribes: Option<PendingUnsubscribes>,
    pub(super) window_invalidations: HashMap<WindowId, WindowInvalidation>,
    pub(super) invalidation_callbacks: HashMap<WindowId, Box<InvalidationCallback>>,
    pub(super) disabled_key_bindings_windows: HashSet<WindowId>,
    pub(super) window_bounds: HashMap<WindowId, Option<RectF>>,
    pub(super) next_window_bounds_map:
        HashMap<WindowId, NextNewWindowsHasThisWindowsBoundsUponClose>,

    /// The bounds of the next window to open.  Typically this is set
    /// using the position of the last window that was closed.
    pub(super) next_window_bounds: Option<RectF>,

    /// Map of window id to the last mouse moved event on that window.  Used
    /// for dispatch of synthetic mouse moved events to trigger hover behavior.
    pub(super) window_last_mouse_moved_event: HashMap<WindowId, Rc<RefCell<Option<Event>>>>,
    pub(super) foreground: Rc<executor::Foreground>,
    pub(super) background: Arc<executor::Background>,
    pub(super) task_callbacks: HashMap<usize, TaskCallback>,
    /// Callbacks that trigger a view redraw on a delay
    /// We store references in case we need to cancel them.
    /// The map key is the timer id returned by EventContext.notify_after
    pub(super) notify_tasks: HashMap<TaskId, ForegroundTask>,
    pub(super) repaint_tasks: HashMap<TaskId, RepaintTask>,

    /// A channel used in tests that receives messages whenever a task
    /// completes.
    #[cfg(test)]
    pub(super) task_done: (async_channel::Sender<usize>, async_channel::Receiver<usize>),
    pub(super) pending_effects: VecDeque<Effect>,
    pub(super) pending_flushes: usize,
    pub(super) flushing_effects: bool,
    pub(super) app_focus_info: AppFocusInfo,
    #[allow(clippy::type_complexity)]
    pub(super) first_frame_callback: Option<Box<dyn Fn(&mut AppContext)>>,
    pub(super) frame_drawn_callback: Option<Box<FrameDrawnCallback>>,
    pub(super) global_shortcuts: HashMap<Keystroke, GlobalShortcut>,
    pub(super) next_frame_callbacks: HashMap<WindowId, Vec<Box<dyn Fn()>>>,
    pub(super) on_draw_frame_error_callback: Option<Box<DrawFrameErrorCallback>>,

    /// The "event munger" allows for clients to hook into events and modify them
    /// before they are dispatched.
    pub(super) event_munger: Box<EventMunger>,

    /// Callback invoked before opening any URL.
    pub(super) before_open_url_callback: Box<BeforeOpenUrlCallback>,

    pub(super) a11y_verbosity: AccessibilityVerbosity,

    #[allow(dead_code)]
    pub(super) spawned_futures: HashMap<FutureId, SpawnedFuture>,

    /// This maps modal IDs to the struct of data needed to send the response back to the context
    /// which requested a platform native modal. This is needed because these modals are opened
    /// asynchronously in platform code. Only the ID is propagated to/from platform code. Once we
    /// have the modal response, we only have its ID, and so we look up the data in this map from
    /// that ID.
    pub(super) platform_modal_data_map: HashMap<ModalId, PlatformModalResponseData>,

    /// If the cursor shape was changed by a view, keep track of that so that we can reset the
    /// cursor if that view goes away.
    pub(crate) cursor_updated_for_view: Option<(WindowId, EntityId)>,
    pub(super) is_unit_test: bool,

    pub(super) termination_result: OnceLock<TerminationResult>,

    /// The current zoom (magnification) factor of the application.
    pub(super) zoom_factor: ZoomFactor,

    /// Maps view entity IDs to their containing window.
    /// This is the source of truth for which window a view belongs to,
    /// enabling views to be transferred between windows.
    ///
    /// Visibility is `pub(super)` because the `view::handle` module needs to access
    /// this field for dynamic window lookup in `ViewHandle::window_id()`,
    /// `WeakViewHandle::upgrade()`, and `WeakViewHandle::window_id()`.
    pub(super) view_to_window: HashMap<EntityId, WindowId>,

    /// Maps child view → parent view for views created via `add_typed_action_view_with_parent`.
    /// Unlike the presenter's layout-time parent map, this persists across renders and
    /// includes views that are conditionally rendered. Used by `transfer_view_tree_to_window`
    /// to discover non-rendered child views that must move with their parent.
    ///
    /// Populated by `add_typed_action_view_internal` when a parent_view_id is provided,
    /// and cleaned up in `remove_dropped_items` when views are dropped.
    pub(super) structural_child_to_parent: HashMap<EntityId, EntityId>,

    /// Reverse of `structural_child_to_parent`: maps parent view → set of child views.
    /// Enables efficient traversal in `transfer_structural_children` without iterating
    /// all views in the source window.
    pub(super) structural_parent_to_children: HashMap<EntityId, HashSet<EntityId>>,

    /// When set, all focus changes to this window are suppressed.
    /// Used during tab drag to prevent the new window from stealing focus.
    pub(super) suppress_focus_for_window: Option<WindowId>,

    /// An optional provider that creates an [`AssetSource`] for loading a
    /// fallback font from a URL string. Injected by the application layer so
    /// that `warpui_core` does not depend on `reqwest`.
    #[allow(clippy::type_complexity)]
    pub(super) fallback_font_source_provider: Option<Box<dyn Fn(&str) -> AssetSource>>,

    /// Per-view callbacks registered via [`ViewContext::observe_focus`].
    /// Fired whenever the observed view gains (`true`) or loses (`false`) focus.
    pub(super) focus_observations: HashMap<EntityId, Vec<FocusObservation>>,
}

impl UpdateModel for AppContext {
    fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        if let Some(mut model) = self.models.remove(&handle.id()) {
            self.pending_flushes += 1;
            let mut ctx = ModelContext::new(self, handle.id());
            let result = update(
                model
                    .as_any_mut()
                    .downcast_mut()
                    .expect("Downcast is type safe"),
                &mut ctx,
            );
            self.models.insert(handle.id(), model);
            self.flush_effects();
            result
        } else {
            panic!("Circular model update");
        }
    }
}

impl UpdateView for AppContext {
    fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
    where
        T: View,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        self.pending_flushes += 1;
        let window_id = handle.window_id(self);
        let mut view = if let Some(window) = self.windows.get_mut(&window_id) {
            if let Some(view) = window.views.remove(&handle.id()) {
                view
            } else {
                panic!("Circular view update");
            }
        } else {
            panic!("Window does not exist");
        };

        let mut ctx = ViewContext::new(self, window_id, handle.id());
        let result = update(
            view.as_any_mut()
                .downcast_mut()
                .expect("Downcast is type safe"),
            &mut ctx,
        );
        if let Some(window) = self.windows.get_mut(&window_id) {
            window.views.insert(handle.id(), view);
        }
        self.flush_effects();
        result
    }
}

impl AddSingletonModel for AppContext {
    fn add_singleton_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut super::ModelContext<T>) -> T,
    {
        AppContext::add_singleton_model(self, build_model)
    }
}

pub struct ClosedWindowData {
    pub window_id: WindowId,
    pub(super) window: Window,
    pub(super) subscriptions: HashMap<EntityId, Vec<Subscription>>,
    pub(super) observations: HashMap<EntityId, Vec<Observation>>,
    pub(super) view_to_window: HashMap<EntityId, WindowId>,
    // TODO(vorporeal): why is AppContext.window_bounds holding an option?
    pub(super) bounds: Option<RectF>,
    pub(super) fullscreen_state: FullscreenState,
}

impl ClosedWindowData {
    /// The window's frame at the moment it closed.
    pub fn bounds(&self) -> Option<RectF> {
        self.bounds
    }

    pub fn fullscreen_state(&self) -> FullscreenState {
        self.fullscreen_state
    }
}

impl AppContext {
    pub fn font_cache(&self) -> &fonts::Cache {
        fonts::Cache::as_ref(self)
    }

    pub fn root_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.root_view.as_ref().map(|v| v.id()))
    }

    pub fn focused_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.focused_view)
    }

    pub fn view_name(&self, window_id: WindowId, view_id: EntityId) -> Option<&str> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.views.get(&view_id))
            .map(|view| view.ui_name())
    }

    /// Returns all the views of type `T` within `window_id`.
    pub fn views_of_type<T: View>(&self, window_id: WindowId) -> Option<Vec<ViewHandle<T>>> {
        let ref_counts = &self.ref_counts;
        self.windows.get(&window_id).map(|window| {
            window
                .views
                .iter()
                .filter(|(_, v)| (*v).as_any().type_id() == TypeId::of::<T>())
                .map(|(view_id, _)| ViewHandle::new(window_id, *view_id, ref_counts))
                .collect::<Vec<ViewHandle<T>>>()
        })
    }

    /// Returns the view of type `T` within `window_id` with the given `entity_id`.
    pub fn view_with_id<T: View>(
        &self,
        window_id: WindowId,
        entity_id: EntityId,
    ) -> Option<ViewHandle<T>> {
        let ref_counts = &self.ref_counts;
        self.windows.get(&window_id).and_then(|window| {
            window
                .views
                .get(&entity_id)
                .filter(|view| (*view).as_any().type_id() == TypeId::of::<T>())
                .map(|_| ViewHandle::new(window_id, entity_id, ref_counts))
        })
    }

    /// Opens the given URL in the default application configured to handle the URL.
    pub fn open_url(&self, url: &str) {
        let effective_url = (self.before_open_url_callback)(url, self);
        self.platform_delegate.open_url(&effective_url);
    }

    pub fn system_theme(&self) -> SystemTheme {
        self.platform_delegate.system_theme()
    }

    pub fn is_headless(&self) -> bool {
        self.platform_delegate.is_headless()
    }

    pub fn microphone_access_state(&self) -> MicrophoneAccessState {
        self.platform_delegate.microphone_access_state()
    }

    pub fn windows(&self) -> &WindowManager {
        WindowManager::as_ref(self)
    }

    pub fn window_ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().cloned()
    }

    pub fn is_wayland(&self) -> bool {
        matches!(
            self.windows().windowing_system(),
            Some(windowing::System::Wayland)
        )
    }

    /// Returns all view IDs registered in the given window.
    pub fn view_ids_for_window(&self, window_id: WindowId) -> Vec<EntityId> {
        self.windows
            .get(&window_id)
            .map(|window| window.views.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn render_view(&self, window_id: WindowId, view_id: EntityId) -> Result<Box<dyn Element>> {
        // surfacing the error of a missing window earlier
        let window = self
            .windows
            .get(&window_id)
            .ok_or_else(|| anyhow!("window not found"))?;
        window
            .views
            .get(&view_id)
            .map(|view| autotracking::render_view(window_id, view_id, || view.render(self)))
            .ok_or_else(|| anyhow!("view not found"))
    }

    pub fn render_views(&self, window_id: WindowId) -> Result<HashMap<EntityId, Box<dyn Element>>> {
        self.windows
            .get(&window_id)
            .map(|w| {
                w.views
                    .iter()
                    .map(|(id, view)| (*id, view.render(self)))
                    .collect::<HashMap<_, _>>()
            })
            .ok_or_else(|| anyhow!("window not found"))
    }

    /// Returns the cached element position from the last rendered frame, if there is one.
    pub fn element_position_by_id_at_last_frame<S>(
        &self,
        window_id: WindowId,
        id: S,
    ) -> Option<RectF>
    where
        S: AsRef<str>,
    {
        self.last_frame_position_cache
            .get(&window_id)
            .and_then(|position_cache| position_cache.get_position(id))
    }
}

impl AsRef<AppContext> for AppContext {
    fn as_ref(&self) -> &AppContext {
        self
    }
}

impl ModelAsRef for AppContext {
    fn model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        if let Some(model) = self.models.get(&handle.id()) {
            model
                .as_any()
                .downcast_ref()
                .expect("downcast should be type safe")
        } else {
            panic!(
                "circular model reference for model type {}",
                std::any::type_name::<T>()
            );
        }
    }
}

impl ReadModel for AppContext {
    fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
    where
        T: Entity,
        F: FnOnce(&T, &AppContext) -> S,
    {
        read(self.model(handle), self)
    }
}

impl ViewAsRef for AppContext {
    fn view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        let window_id = handle.window_id(self);
        if let Some(window) = self.windows.get(&window_id) {
            if let Some(view) = window.views.get(&handle.id()) {
                view.as_any()
                    .downcast_ref()
                    .expect("downcast should be type safe")
            } else {
                panic!(
                    "circular view reference for view type {}",
                    std::any::type_name::<T>()
                );
            }
        } else {
            panic!("window does not exist");
        }
    }

    /// Returns the backing view, or None if materializing the view would
    /// otherwise produce a circular view reference.
    fn try_view<T: View>(&self, handle: &ViewHandle<T>) -> Option<&T> {
        let window_id = handle.window_id(self);
        self.windows
            .get(&window_id)?
            .views
            .get(&handle.id())?
            .as_any()
            .downcast_ref()
    }
}

impl ReadView for AppContext {
    fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
    where
        T: View,
        F: FnOnce(&T, &AppContext) -> S,
    {
        read(self.view(handle), self)
    }
}

impl GetSingletonModelHandle for AppContext {
    fn get_singleton_model_handle<T: SingletonEntity>(&self) -> ModelHandle<T> {
        match self.singleton_models.get(&std::any::TypeId::of::<T>()) {
            Some(model_handle) => model_handle
                .clone()
                .downcast()
                .expect("a registered singleton model should never have a refcount of 0"),
            None => {
                panic!(
                    "Cannot get singleton model of type {:?} that was never registered",
                    std::any::type_name::<T>()
                );
            }
        }
    }
}

impl AppContext {
    pub(super) fn get_singleton_model_as_ref<T: SingletonEntity>(&self) -> &T {
        match self.singleton_models.get(&std::any::TypeId::of::<T>()) {
            Some(model_handle) => model_handle
                .downcast_ref(self)
                .expect("downcast should be type safe"),
            None => {
                panic!(
                    "Cannot get singleton model of type {:?} that was never registered",
                    std::any::type_name::<T>()
                );
            }
        }
    }
}
