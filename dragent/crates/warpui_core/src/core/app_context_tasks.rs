use std::any::Any;
use std::path::Path;
use std::pin::pin;
use std::sync::atomic::AtomicI64;

use anyhow::{anyhow, Result};
use futures::prelude::*;
use itertools::Itertools;
use lazy_static::lazy_static;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;

use super::{BlurContext, FocusContext, Observation, TaskCallback};
use crate::accessibility::AccessibilityVerbosity;
use crate::assets::AssetProvider;
use crate::fonts::{self};
use crate::platform::app::TerminationResult;
use crate::platform::file_picker::{FilePickerConfiguration, FilePickerError};
use crate::platform::{SaveFilePickerConfiguration, TerminationMode, WindowBounds, WindowContext};
#[cfg(test)]
use crate::r#async::block_on;
use crate::r#async::SpawnableOutput;
use crate::util::post_inc;
use crate::{
    AddWindowOptions, AnyModel, AnyView, ApplicationBundleInfo, Element, EntityId,
    NextNewWindowsHasThisWindowsBoundsUponClose, SingletonEntity, View, WindowId,
};
#[cfg(test)]
use std::collections::HashSet;

lazy_static! {
    static ref LAST_USER_ACTION_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
}

use super::app::AppContext;

impl AppContext {
    pub(super) fn notify_model_observers(&mut self, observed_id: EntityId) {
        // TODO: Apply the same deferred unsubscribe pattern used in `emit_event` to support
        // unobserving from inside an observation callback.
        if let Some(observations) = self.observations.remove(&observed_id) {
            if self.models.contains_key(&observed_id) {
                for mut observation in observations {
                    let alive = match &mut observation {
                        Observation::FromModel { model_id, callback } => {
                            if let Some(mut model) = self.models.remove(model_id) {
                                callback(model.as_any_mut(), observed_id, self, *model_id);
                                self.models.insert(*model_id, model);
                                true
                            } else {
                                false
                            }
                        }
                        Observation::FromView {
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
                                .and_then(|w| w.views.remove(view_id))
                            {
                                callback(
                                    view.as_any_mut(),
                                    observed_id,
                                    self,
                                    current_window_id,
                                    *view_id,
                                );
                                if let Some(window) = self.windows.get_mut(&current_window_id) {
                                    window.views.insert(*view_id, view);
                                }
                                true
                            } else {
                                false
                            }
                        }
                        Observation::FromApp { callback } => {
                            callback(observed_id, self);
                            true
                        }
                    };

                    if alive {
                        self.observations
                            .entry(observed_id)
                            .or_default()
                            .push(observation);
                    }
                }
            }
        }
    }

    pub(super) fn notify_view_observers(&mut self, window_id: WindowId, view_id: EntityId) {
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .updated
            .insert(view_id);
    }

    pub(super) fn focus(&mut self, window_id: WindowId, focused_id: EntityId) {
        if self.windows.get(&window_id).and_then(|w| w.focused_view) == Some(focused_id) {
            return;
        }

        if self.suppress_focus_for_window == Some(window_id) {
            return;
        }
        self.pending_flushes += 1;

        if let Some((blurred_id, mut blurred)) = self.windows.get_mut(&window_id).and_then(|w| {
            let blurred_view = w.focused_view;
            w.focused_view = Some(focused_id);
            blurred_view.and_then(|id| w.views.remove(&id).map(|view| (id, view)))
        }) {
            blurred.on_blur(&BlurContext::SelfBlurred, self, window_id, blurred_id);
            self.windows
                .get_mut(&window_id)
                .unwrap()
                .views
                .insert(blurred_id, blurred);

            // Notify any `observe_focus` subscribers that this view lost focus.
            self.fire_focus_observations(blurred_id, false);

            if let Some(presenter) = self.presenter(window_id) {
                let blur_ctx = BlurContext::DescendentBlurred(blurred_id);
                // Skip the last entry, it is the blurred view itself.
                for view_id in presenter
                    .borrow()
                    .ancestors(blurred_id)
                    .into_iter()
                    .rev()
                    .skip(1)
                {
                    if let Some(mut view) = self
                        .windows
                        .get_mut(&window_id)
                        .and_then(|w| w.views.remove(&view_id))
                    {
                        view.on_blur(&blur_ctx, self, window_id, view_id);
                        self.windows
                            .get_mut(&window_id)
                            .and_then(|w| w.views.insert(view_id, view));
                    }
                }
            }
        }

        // Close the IME if it was open since the view that was focused has changed.
        // It's important for us to do this asynchronously: since we are in a method
        // that's called from UI framework code, we don't want to trigger platform
        // side effects that could cause the AppContext to be borrowed in
        // the same callstack.
        self.platform_delegate.close_ime_async(window_id);

        if let Some(mut focused) = self
            .windows
            .get_mut(&window_id)
            .and_then(|w| w.views.remove(&focused_id))
        {
            focused.on_focus(&FocusContext::SelfFocused, self, window_id, focused_id);
            self.windows
                .get_mut(&window_id)
                .unwrap()
                .views
                .insert(focused_id, focused);

            // Notify any `observe_focus` subscribers that this view gained focus.
            self.fire_focus_observations(focused_id, true);

            if let Some(presenter) = self.presenter(window_id) {
                let focus_ctx = FocusContext::DescendentFocused(focused_id);
                // Skip the last entry, it is the focused view itself.
                for view_id in presenter
                    .borrow()
                    .ancestors(focused_id)
                    .into_iter()
                    .rev()
                    .skip(1)
                {
                    if let Some(mut view) = self
                        .windows
                        .get_mut(&window_id)
                        .and_then(|w| w.views.remove(&view_id))
                    {
                        view.on_focus(&focus_ctx, self, window_id, view_id);
                        self.windows
                            .get_mut(&window_id)
                            .and_then(|w| w.views.insert(view_id, view));
                    }
                }
            }
        }

        self.flush_effects();
    }

    pub(super) fn spawn_local<F>(&mut self, future: F) -> usize
    where
        F: 'static + Future,
    {
        let task_id = post_inc(&mut self.next_task_id);
        let app = self.weak_self.clone();
        self.foreground
            .spawn_boxed(
                async move {
                    let output = future.await;
                    if let Some(app) = app.upgrade() {
                        // Ignore any errors that may occur when relaying task output,
                        // as there's nothing we can do about the entity no longer
                        // existing.
                        let _ = app
                            .borrow_mut()
                            .relay_task_output(task_id, Box::new(output));
                    }
                }
                .boxed_local(),
            )
            .detach();
        task_id
    }

    pub(super) fn spawn_stream_local<F>(
        &mut self,
        stream: F,
        done_tx: futures::channel::oneshot::Sender<()>,
    ) -> usize
    where
        F: 'static + crate::r#async::Stream,
        F::Item: SpawnableOutput,
    {
        // Spawn a background task to poll the stream, and forward items to the
        // main thread (foreground executor).
        let (tx, rx) = async_channel::unbounded();
        self.background
            .spawn(async move {
                let mut stream = pin!(stream);
                while let Some(item) = stream.next().await {
                    // If we fail to send the item, the foreground task has dropped the receiver,
                    // meaning the entity that spawned the stream no longer exists, and we can
                    // stop polling the stream.
                    if tx.send(item).await.is_err() {
                        break;
                    }
                }
            })
            .detach();

        self.relay_stream_on_main_thread(rx, done_tx)
    }

    /// Like [`Self::spawn_stream_local`], but the stream is polled on the main
    /// thread rather than by a background task. For cheap-to-poll streams
    /// (channel receivers) whose delivery must not wait behind background
    /// work: when every background thread is busy, a background-polled stream
    /// stalls until one frees up.
    pub(super) fn spawn_stream_on_main_thread<F>(
        &mut self,
        stream: F,
        done_tx: futures::channel::oneshot::Sender<()>,
    ) -> usize
    where
        F: 'static + crate::r#async::Stream,
        F::Item: SpawnableOutput,
    {
        self.relay_stream_on_main_thread(stream, done_tx)
    }

    /// Poll `stream` on the foreground executor, handing each item to the
    /// task's callbacks.
    fn relay_stream_on_main_thread<F>(
        &mut self,
        rx: F,
        done_tx: futures::channel::oneshot::Sender<()>,
    ) -> usize
    where
        F: 'static + crate::r#async::Stream,
        F::Item: SpawnableOutput,
    {
        // Spawn a task on the foreground executor to invoke the provided on_item and on_done
        // callbacks on the main thread.
        let task_id = post_inc(&mut self.next_task_id);
        let app = self.weak_self.clone();
        self.foreground
            .spawn(async move {
                let mut stream = pin!(rx);
                loop {
                    match stream.next().await {
                        Some(item) => {
                            if let Some(app) = app.upgrade() {
                                let mut app = app.borrow_mut();

                                // If the entity that spawned the stream no longer exists, terminate
                                // the stream.
                                if app.relay_task_output(task_id, Box::new(item)).is_err() {
                                    app.stream_completed(task_id);
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        None => {
                            if let Some(app) = app.upgrade() {
                                let mut app = app.borrow_mut();
                                app.stream_completed(task_id);
                            }
                            let _ = done_tx.send(());
                            break;
                        }
                    }
                }
            })
            .detach();
        task_id
    }

    /// Opens the file path using the default application configured to handle the given filetype.
    pub fn open_file_path(&mut self, path: &Path) {
        self.platform_delegate.open_file_path(path);
    }

    /// Opens the given file path in an explorer view. On MacOS this will open the file in finder.
    pub fn open_file_path_in_explorer(&mut self, path: &Path) {
        self.platform_delegate.open_file_path_in_explorer(path);
    }

    /// Prompt the user to pick file path(s) in the OS native file picker.
    pub fn open_file_picker(
        &mut self,
        callback: impl FnOnce(Result<Vec<String>, FilePickerError>, &mut AppContext)
            + Send
            + Sync
            + 'static,
        config: FilePickerConfiguration,
    ) {
        self.platform_delegate
            .open_file_picker(Box::new(callback), config)
    }

    /// Prompt the user to save a file with the OS native save file dialog.
    pub fn open_save_file_picker(
        &mut self,
        callback: impl FnOnce(Option<String>, &mut AppContext) + Send + Sync + 'static,
        config: SaveFilePickerConfiguration,
    ) {
        self.platform_delegate
            .open_save_file_picker(Box::new(callback), config)
    }

    pub fn application_bundle_info(
        &self,
        bundle_identifier: &str,
    ) -> Option<ApplicationBundleInfo<'_>> {
        self.platform_delegate
            .application_bundle_info(bundle_identifier)
    }

    /// Triggers termination of the app and optionally takes in a TerminationResult, which will be
    /// printed when the program exits.
    pub fn terminate_app(
        &mut self,
        termination_mode: TerminationMode,
        termination_result: Option<TerminationResult>,
    ) {
        if let Some(termination_result) = termination_result {
            #[cfg(debug_assertions)]
            self.termination_result
                .set(termination_result)
                .expect("Termination result should not have been set already");
            #[cfg(not(debug_assertions))]
            let _ = self.termination_result.set(termination_result);
        }
        self.platform_delegate.terminate_app(termination_mode);
    }

    pub fn check_view_focused(&self, window_id: WindowId, view_id: &EntityId) -> bool {
        let focused_view_id = match self.focused_view_id(window_id) {
            Some(id) => id,
            None => return false,
        };
        focused_view_id == *view_id
    }

    pub fn check_view_or_child_focused(&self, window_id: WindowId, view_id: &EntityId) -> bool {
        let focused_view_id = match self.focused_view_id(window_id) {
            Some(id) => id,
            None => return false,
        };
        let presenter = match self.presenter(window_id) {
            Some(p) => p,
            None => return false,
        };

        let borrowed_presenter = presenter.borrow();
        borrowed_presenter
            .ancestors(focused_view_id)
            .contains(view_id)
    }

    pub(super) fn relay_task_output(&mut self, task_id: usize, output: Box<dyn Any>) -> Result<()> {
        self.pending_flushes += 1;
        let Some(task_callback) = self.task_callbacks.remove(&task_id) else {
            return Err(anyhow!("Unable to retrieve task callback."));
        };

        let mut result = Ok(());

        match task_callback {
            TaskCallback::ModelFromFuture { model_id, callback } => {
                if let Some(mut model) = self.models.remove(&model_id) {
                    callback(model.as_any_mut(), output, self, model_id);
                    self.models.insert(model_id, model);
                }
                self.task_done(task_id);
            }
            TaskCallback::ModelFromStream {
                model_id,
                mut on_item,
                on_done,
            } => {
                if let Some(mut model) = self.models.remove(&model_id) {
                    on_item(model.as_any_mut(), output, self, model_id);
                    self.models.insert(model_id, model);
                } else {
                    result = Err(anyhow!(
                        "Unable to retrieve model when relaying task output from stream"
                    ));
                }
                // Streams go through different code paths compared to Futures.
                // Even if the stream halts after this call, we still need to
                // refer to the task callback in stream completed.
                self.task_callbacks.insert(
                    task_id,
                    TaskCallback::ModelFromStream {
                        model_id,
                        on_item,
                        on_done,
                    },
                );
            }
            TaskCallback::ViewFromFuture {
                window_id,
                view_id,
                callback,
            } => {
                if let Some(mut view) = self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|w| w.views.remove(&view_id))
                {
                    callback(view.as_mut(), output, self, window_id, view_id);
                    self.windows
                        .get_mut(&window_id)
                        .ok_or_else(|| anyhow!("Unable to retrieve window for view"))?
                        .views
                        .insert(view_id, view);
                }
                self.task_done(task_id);
            }
            TaskCallback::ViewFromStream {
                window_id,
                view_id,
                mut on_item,
                on_done,
            } => {
                if let Some(mut view) = self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|w| w.views.remove(&view_id))
                {
                    on_item(view.as_mut(), output, self, window_id, view_id);
                    self.windows
                        .get_mut(&window_id)
                        .ok_or_else(|| anyhow!("Unable to retrieve window for view"))?
                        .views
                        .insert(view_id, view);
                } else {
                    result = Err(anyhow!(
                        "Unable to retrieve view when relaying task output from stream"
                    ));
                }
                // Streams go through different code paths compared to Futures.
                // Even if the stream halts after this call, we still need to
                // refer to the task callback in stream completed.
                self.task_callbacks.insert(
                    task_id,
                    TaskCallback::ViewFromStream {
                        window_id,
                        view_id,
                        on_item,
                        on_done,
                    },
                );
            }
        };
        self.flush_effects();
        result
    }

    pub(super) fn stream_completed(&mut self, task_id: usize) {
        self.pending_flushes += 1;
        let Some(task_callback) = self.task_callbacks.remove(&task_id) else {
            return;
        };
        match task_callback {
            TaskCallback::ModelFromStream {
                model_id, on_done, ..
            } => {
                if let Some(mut model) = self.models.remove(&model_id) {
                    on_done(model.as_any_mut(), self, model_id);
                    self.models.insert(model_id, model);
                }
            }
            TaskCallback::ViewFromStream {
                window_id,
                view_id,
                on_done: callback,
                ..
            } => {
                if let Some(mut view) = self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|w| w.views.remove(&view_id))
                {
                    callback(view.as_mut(), self, window_id, view_id);
                    self.windows
                        .get_mut(&window_id)
                        .expect("Window should exist.")
                        .views
                        .insert(view_id, view);
                }
            }
            _ => {}
        };
        self.flush_effects();
        self.task_done(task_id);
    }

    pub(super) fn task_done(&self, _task_id: usize) {
        // If the receiver has been dropped, the app is likely terminating,
        // so ignore the error.  Additionally, we only ever consume items
        // from this queue in tests, so if this isn't a test, don't stick
        // things into the channel, otherwise it will grow forever without
        // bound.
        #[cfg(test)]
        let _ = block_on(self.task_done.0.send(_task_id));
    }

    pub fn set_a11y_verbosity(&mut self, verbosity: AccessibilityVerbosity) {
        self.a11y_verbosity = verbosity;
    }

    #[cfg(test)]
    pub fn finish_pending_tasks(&self) -> impl Future<Output = ()> {
        let mut pending_tasks = self.task_callbacks.keys().cloned().collect::<HashSet<_>>();
        let task_done = self.task_done.1.clone();

        async move {
            while !pending_tasks.is_empty() {
                if let Ok(task_id) = task_done.recv().await {
                    pending_tasks.remove(&task_id);
                } else {
                    break;
                }
            }
        }
    }

    pub fn open_view_tree_debug_window(&mut self, target_window_id: WindowId) {
        let Some(presenter) = self.presenter(target_window_id) else {
            return;
        };
        let Some(root_view_id) = self.root_view_id(target_window_id) else {
            return;
        };

        let Some(current_bounds) = self.window_bounds(&target_window_id) else {
            return;
        };
        let size = Vector2F::new(340., 540.);
        let origin = Vector2F::new(
            current_bounds.origin().x() + current_bounds.width() - size.x() - 20.,
            current_bounds.origin().y() + 20.,
        );

        let options = AddWindowOptions {
            window_bounds: WindowBounds::ExactPosition(RectF::new(origin, size)),
            anchor_new_windows_from_closed_position:
                NextNewWindowsHasThisWindowsBoundsUponClose::No,
            window_instance: Some("dev.dragent.ui-debug".to_owned()),
            title: Some("View Tree Debugger".to_owned()),
            ..Default::default()
        };
        self.add_window(options, |ctx| {
            crate::debug::DebugRootView::new(
                target_window_id,
                presenter.borrow().parents(),
                root_view_id,
                ctx,
            )
        });
    }

    pub fn record_app_focus(&mut self, user_id: Option<String>, anonymous_id: String) {
        self.app_focus_info.record_app_focus(user_id, anonymous_id);
    }

    pub fn record_app_blur(&mut self, user_id: Option<String>, anonymous_id: String) {
        self.app_focus_info.record_app_blur(user_id, anonymous_id);
    }

    pub fn try_record_daily_app_focus_duration(
        &mut self,
        user_id: Option<String>,
        anonymous_id: String,
    ) {
        self.app_focus_info
            .try_record_daily_app_focus_duration(user_id, anonymous_id);
    }

    pub fn is_screen_reader_enabled(&self) -> Option<bool> {
        self.platform_delegate.is_screen_reader_enabled()
    }

    /// A way for applications to specify custom fallback fonts.
    ///
    /// Takes a function that maps characters to external font families, each
    /// containing URLs to the font files for that family.
    ///
    /// If a character cannot be rendered using the existing loaded fonts, and
    /// the fallback font function specifies a fallback font family, the font
    /// family will be lazy-loaded and the character will be re-rendered with
    /// the specified font.
    pub fn set_fallback_font_fn(
        &mut self,
        f: impl Fn(char) -> Option<fonts::ExternalFontFamily> + Send + Sync + 'static,
    ) {
        fonts::Cache::handle(self).update(self, |cache, _| {
            cache.set_fallback_font_fn(Box::new(f));
        });
    }
}
