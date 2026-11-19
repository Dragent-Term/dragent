use std::time::Duration;

use warpui::elements::shimmering_text::{
    ShimmerConfig, ShimmeringTextElement, ShimmeringTextStateHandle,
};
use warpui::elements::{
    Align, ChildView, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Entity, GetSingletonModelHandle, SingletonEntity, TypedActionView, UpdateModel,
    View, ViewContext, ViewHandle, WindowId,
};
use warpui_core::fonts::Cache as FontCache;

use crate::setup::{OnboardingEvent, OnboardingFlow, UiSettings};
use crate::theme::Theme;
use crate::workspace::{prewarm, sync_native_window_chrome, WorkspaceView};

/// Which workspace to construct after background session preparation.
enum PendingWorkspace {
    Restore,
    Empty,
    Secondary,
}

enum RootState {
    Onboarding(ViewHandle<OnboardingFlow>),
    /// Remains responsive while disk recovery runs on a background worker.
    Transitioning {
        theme: Theme,
        ui_font: FamilyId,
        pending: PendingWorkspace,
        /// Kept across re-renders so the wordmark's sweep doesn't restart.
        shimmer: ShimmeringTextStateHandle,
    },
    Workspace(ViewHandle<WorkspaceView>),
}

pub struct RootView {
    state: RootState,
    /// The launch window restores the saved session; New Window ones start empty.
    primary: bool,
    ui_font: FamilyId,
    /// This window's id: the relay-catalog contributor key. Retired on drop
    /// so a closed window's workspaces/tabs leave the shared aggregate.
    window_id: WindowId,
    /// Bumped whenever an in-flight workspace load is superseded (an
    /// organisation switch while loading), so its result is discarded.
    load_generation: u64,
}

impl RootView {
    /// This window's workspace, once it has finished loading.
    pub(crate) fn workspace(&self) -> Option<ViewHandle<WorkspaceView>> {
        match &self.state {
            RootState::Workspace(workspace) => Some(workspace.clone()),
            _ => None,
        }
    }

    pub(crate) fn persist_workspace(&self, ctx: &mut ViewContext<Self>) {
        if let RootState::Workspace(workspace) = &self.state {
            use warpui::UpdateView;
            ctx.update_view(workspace, |view, view_ctx| view.save_session(view_ctx));
        }
    }

    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        sync_native_window_chrome(ctx.windows(), ctx.window_id());
        Self::follow_organisation(ctx);

        let font_cache = ctx.get_singleton_model_handle::<FontCache>();
        let ui_font = ctx.update_model(&font_cache, |cache, _| prewarm::load_ui_font(cache));

        ctx.update_model(&font_cache, |cache, _| {
            prewarm::load_mono_font(cache);
        });

        let settings = UiSettings::load();

        if settings.onboarding_completed {
            // "Sync with OS": resolve the persisted theme against the live
            // OS mode at startup so a system change while away applies.
            let os_dark = matches!(ctx.system_theme(), warpui_core::platform::SystemTheme::Dark);
            let theme_id = settings
                .os_matched_id(os_dark)
                .unwrap_or_else(|| settings.theme_id.clone());
            let theme = Theme::from_id(&theme_id);
            // Restore once background preparation completes. An absent saved
            // session naturally lands on the workspace creation screen.
            Self::spawn_workspace_pending(ctx, theme, ui_font, PendingWorkspace::Restore)
        } else {
            let flow = ctx.add_typed_action_view(OnboardingFlow::new);
            ctx.subscribe_to_view(&flow, move |this: &mut RootView, _handle, event, ctx| {
                let OnboardingEvent::Completed { theme_id } = event;
                let theme = Theme::from_id(theme_id);
                *this =
                    Self::spawn_workspace_pending(ctx, theme, ui_font, PendingWorkspace::Restore);
            });
            Self {
                state: RootState::Onboarding(flow),
                primary: true,
                ui_font,
                window_id: ctx.window_id(),
                load_generation: 0,
            }
        }
    }

    /// Root view for an additional (New Window) window. Onboarding is already done
    /// by the time this is reachable, so go straight to a fresh, empty workspace.
    pub fn new_empty(ctx: &mut ViewContext<Self>) -> Self {
        sync_native_window_chrome(ctx.windows(), ctx.window_id());
        Self::follow_organisation(ctx);

        let font_cache = ctx.get_singleton_model_handle::<FontCache>();
        let ui_font = ctx.update_model(&font_cache, |cache, _| prewarm::load_ui_font(cache));
        ctx.update_model(&font_cache, |cache, _| {
            prewarm::load_mono_font(cache);
        });

        let settings = UiSettings::load();
        let os_dark = matches!(ctx.system_theme(), warpui_core::platform::SystemTheme::Dark);
        let theme_id = settings
            .os_matched_id(os_dark)
            .unwrap_or_else(|| settings.theme_id.clone());
        let theme = Theme::from_id(&theme_id);
        Self::spawn_workspace_pending(ctx, theme, ui_font, PendingWorkspace::Secondary)
    }

    fn spawn_workspace_pending(
        ctx: &mut ViewContext<Self>,
        theme: Theme,
        ui_font: FamilyId,
        pending: PendingWorkspace,
    ) -> Self {
        let primary = !matches!(pending, PendingWorkspace::Secondary);
        let this = Self {
            state: RootState::Transitioning {
                theme,
                ui_font,
                pending,
                shimmer: ShimmeringTextStateHandle::new(),
            },
            primary,
            ui_font,
            window_id: ctx.window_id(),
            load_generation: 0,
        };
        ctx.notify();
        Self::load_workspace(ctx, 0);
        this
    }

    /// Rebuild this window's workspace when the selected organisation changes:
    /// save and detach while the old one is selected, restore from the new one.
    fn follow_organisation(ctx: &mut ViewContext<Self>) {
        use crate::organisation_context::{OrganisationContext, OrganisationContextEvent};
        let context = OrganisationContext::handle(ctx);
        ctx.subscribe_to_model(&context, |root, _, event, ctx| match event {
            OrganisationContextEvent::WillSwitch => root.detach_workspace(ctx),
            OrganisationContextEvent::Switched => {
                if matches!(root.state, RootState::Transitioning { .. }) {
                    root.load_generation += 1;
                    Self::load_workspace(ctx, root.load_generation);
                }
            }
        });
    }

    fn detach_workspace(&mut self, ctx: &mut ViewContext<Self>) {
        match &self.state {
            RootState::Onboarding(_) => return,
            RootState::Workspace(_) => self.persist_workspace(ctx),
            RootState::Transitioning { .. } => {}
        }
        // Supersede any in-flight load: it read the previous organisation.
        self.load_generation += 1;
        let settings = UiSettings::load();
        let os_dark = matches!(ctx.system_theme(), warpui_core::platform::SystemTheme::Dark);
        let theme_id = settings
            .os_matched_id(os_dark)
            .unwrap_or_else(|| settings.theme_id.clone());
        self.state = RootState::Transitioning {
            theme: Theme::from_id(&theme_id),
            ui_font: self.ui_font,
            pending: if self.primary {
                PendingWorkspace::Restore
            } else {
                PendingWorkspace::Secondary
            },
            shimmer: ShimmeringTextStateHandle::new(),
        };
        ctx.notify();
    }

    fn load_workspace(ctx: &mut ViewContext<Self>, generation: u64) {
        // A selection can change while disk recovery runs, including before
        // this window receives WillSwitch. Never publish that stale result.
        let selection = dragent_paths::organisation_selection();
        ctx.spawn(
            async {
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let started = std::time::Instant::now();
                    let prepared = prewarm::load_persistence();
                    log::info!(
                        "[startup] session recovery prepared in {}ms",
                        started.elapsed().as_millis()
                    );
                    let _ = tx.send(prepared);
                });
                rx.await
            },
            move |root, prepared, ctx| {
                let Ok(prepared) = prepared else {
                    log::error!(
                        "[startup] session preparation worker stopped; preserving saved session"
                    );
                    return;
                };
                if root.load_generation != generation
                    || dragent_paths::organisation_selection() != selection
                {
                    return;
                }
                let pending = match &mut root.state {
                    RootState::Transitioning { pending, .. } => {
                        std::mem::replace(pending, PendingWorkspace::Empty)
                    }
                    _ => return,
                };
                // Settings → General → Restore sessions on startup.
                let restore = !matches!(pending, PendingWorkspace::Secondary)
                    && crate::settings::git_consumers::restore_sessions();
                let started = std::time::Instant::now();
                let workspace = ctx.add_typed_action_view(move |ctx| {
                    WorkspaceView::build_prepared(ctx, restore, prepared)
                });
                log::info!(
                    "[startup] workspace views constructed in {}ms",
                    started.elapsed().as_millis()
                );
                root.state = RootState::Workspace(workspace);
                ctx.notify();
            },
        );
    }

    /// Keep the window painting while session metadata and worktrees load.
    fn render_transitioning(
        theme: &Theme,
        ui_font: FamilyId,
        shimmer: &ShimmeringTextStateHandle,
    ) -> Box<dyn Element> {
        let wordmark = ShimmeringTextElement::new(
            "Dragent",
            ui_font,
            crate::theme::text_size(30.0),
            theme.sub_text(),
            theme.main_text(),
            ShimmerConfig {
                period: Duration::from_millis(1600),
                shimmer_radius: 3,
                padding: 3,
            },
            shimmer.clone(),
        )
        .finish();
        // `Align` fills the whole window and centers the wordmark; a Flex
        // column would size its width to the text, leaving the rest of the
        // window unpainted.
        Container::new(Align::new(wordmark).finish())
            .with_background_color(theme.background)
            .finish()
    }
}

impl Entity for RootView {
    type Event = ();
}

impl Drop for RootView {
    fn drop(&mut self) {
        // Window closed: retire this window's relay-catalog contribution so
        // its workspaces/tabs leave the shared aggregate. Other windows are
        // untouched. Open windows retain their contribution while idle.
        #[cfg(not(target_family = "wasm"))]
        crate::workspace::desktop_catalog::note_window_closed(self.window_id);
    }
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "RootView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        match &self.state {
            RootState::Onboarding(flow) => ChildView::new(flow).finish(),
            RootState::Transitioning {
                theme,
                ui_font,
                shimmer,
                ..
            } => Self::render_transitioning(theme, *ui_font, shimmer),
            RootState::Workspace(ws) => ChildView::new(ws).finish(),
        }
    }
}

impl TypedActionView for RootView {
    type Action = ();
}
