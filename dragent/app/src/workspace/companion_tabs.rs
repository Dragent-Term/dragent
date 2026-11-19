//! Tabs a paired phone asks this window to open (its "+ New tab" menu).
//! Each opens in the named workspace as a tab of its own, in the
//! background: what the person is looking at and the keyboard stay put.

use warpui::{SingletonEntity, UpdateView, ViewContext};

use super::pane::WorkspacePane;
use super::view::WorkspaceView;
use crate::realtime::companion_host::{Failure, NewTabKind, NewTabRequest};
use crate::realtime::desktop_catalog::AccountKey;

/// What the desktop's header menu runs for its "Dragent CLI" item.
const DRAGENT_CLI_LABEL: &str = "Dragent CLI";
const DRAGENT_CLI_COMMAND: &str = "dragent --help";

impl WorkspaceView {
    /// Start taking this window's requests (once, when the window is built).
    pub(super) fn listen_for_companion_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        let requests = crate::realtime::companion_host::new_tab_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            requests,
            |view, request, ctx| {
                let result = view.open_tab_for_companion(&request, ctx);
                request.answer(result);
            },
            |_, _| {},
        );
        // A phone opened a tab whose agent is asleep.
        let wakes = crate::realtime::companion_host::wake_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            wakes,
            |view, tab_id, ctx| {
                view.wake_agent_for_phone(tab_id, ctx);
            },
            |_, _| {},
        );
        self.keep_catalog_fresh(ctx);
        // A phone looked at one of this window's agents.
        let seen = crate::realtime::companion_host::seen_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            seen,
            |view, pane, ctx| view.phone_saw_pane(pane, ctx),
            |_, _| {},
        );
        // A phone deleting one of this window's workspaces.
        let deletes = crate::realtime::companion_host::delete_workspace_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            deletes,
            |view, request, ctx| {
                let result = view.delete_workspace_for_companion(&request, ctx);
                request.answer(result);
            },
            |_, _| {},
        );
        // A phone's actions on this window's browser tabs.
        let browser = crate::realtime::companion_host::browser_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            browser,
            |view, request, ctx| view.run_browser_for_companion(request, ctx),
            |_, _| {},
        );
    }

    /// A phone opened this agent's tab: what it finished or failed is seen,
    /// as when the tab is selected here. Clears the stop record and the
    /// sidebar row now, and redraws so the catalog the phone reads agrees.
    fn phone_saw_pane(&mut self, saw: crate::realtime::companion_host::PhoneSaw, ctx: &mut ViewContext<Self>) {
        use crate::realtime::companion_host::PhoneSaw;
        log::info!("[phone-seen] window heard {saw:?}");
        let pane = match saw {
            PhoneSaw::Pane(pane) => {
                let ours = self
                    .surfaces()
                    .any(|(_, panes)| panes.iter().any(|tab| tab.pane.pane_uuid(ctx) == pane));
                if !ours {
                    return;
                }
                pane
            }
            PhoneSaw::Terminal(session) => {
                let Some(tab_id) = self
                    .host_owned_tabs
                    .iter()
                    .find(|(_, tab)| tab.session_id.as_deref() == Some(session.as_str()))
                    .map(|(tab_id, _)| *tab_id)
                else {
                    log::info!("[phone-seen] no host tab with that terminal here");
                    return;
                };
                let Some(pane) = self.surfaces().find_map(|(_, panes)| {
                    panes.iter().find(|tab| tab.id == tab_id).map(|tab| tab.pane.pane_uuid(ctx))
                }) else {
                    return;
                };
                pane
            }
        };
        let changed = self.mark_pane_seen(pane);
        log::info!("[phone-seen] pane {pane}: stop record changed={changed}");
        self.left_panel_view.update(ctx, |panel, panel_ctx| {
            for row in panel.tabs.iter_mut() {
                row.acknowledge_pane_activity(pane);
            }
            panel_ctx.notify();
        });
        self.sync_left_panel(ctx);
        ctx.notify();
        // Tell the phone now: a window nobody is looking at (the screen
        // locked, the desktop asleep) may not draw again for a long while.
        super::desktop_catalog::note_rendered(self, ctx);
        if let Some(tab) = self.surfaces().find_map(|(_, panes)| panes.iter().find(|tab| tab.pane.pane_uuid(ctx) == pane)) {
            log::info!("[phone-seen] pane {pane} now {:?}", self.effective_pane_status(tab, ctx));
        }
    }

    /// What the phone sees is noted as the window draws. A window that
    /// doesn't draw (hidden, the screen locked, the display asleep) would
    /// leave the phone on old statuses, so note it every few seconds too;
    /// unchanged catalogs aren't sent again.
    fn keep_catalog_fresh(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_secs(2)), |view, _, ctx| {
            super::desktop_catalog::note_rendered(view, ctx);
            view.keep_catalog_fresh(ctx);
        });
    }

    /// Carry out a phone's browser action in the browser tab it named, with
    /// the same control agents use. Only for the account this window
    /// published its catalog under, and only for a tab that is a browser.
    fn run_browser_for_companion(
        &mut self,
        request: crate::realtime::companion_host::BrowserRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        let reply = request.replier();
        if self.current_account(ctx).as_ref() != Some(&request.account) {
            return reply(Err("This desktop is signed in to another account.".into()));
        }
        let found = self.surfaces().find_map(|(surface, panes)| {
            panes.iter().find_map(|tab| match &tab.pane {
                WorkspacePane::Browser(browser) if tab.id as u64 == request.tab_id => {
                    Some((surface, tab.id, browser.clone()))
                }
                _ => None,
            })
        });
        let Some((surface, tab_id, browser)) = found else {
            return reply(Err("That browser tab isn't open any more.".into()));
        };
        if request.tool == "release" {
            browser.update(ctx, |view, view_ctx| view.phone_release(view_ctx));
            return reply(Ok("released".into()));
        }
        // The tab shows who is using it; after "Take back control" the
        // phone is refused until it opens the tab again.
        let allowed = request.cleanup
            || browser.update(ctx, |view, view_ctx| {
                view.phone_touch(&request.phone_name, request.claim, request.viewport, view_ctx)
            });
        if !allowed {
            return reply(Err(crate::realtime::phone_browser::TAKEN_BACK.into()));
        }
        let command = match crate::browser::agent::AgentCommand::parse(&request.tool, &request.arguments) {
            Ok(command) => command,
            Err(error) => return reply(Err(error)),
        };
        // The page only runs while its tab is the one shown: bring it into
        // view when the phone opens it (not on every step, which would fight
        // whoever is at the desktop).
        if request.claim {
            self.reveal_browser(surface, tab_id, ctx);
        }
        browser.update(ctx, |view, view_ctx| view.agent_now(command, reply, view_ctx));
    }

    /// Delete the workspace a phone named, as confirming the close dialog
    /// does. Only for the account this window published its catalog under,
    /// and only while the workspace is still the folder the phone's checks
    /// looked at.
    fn delete_workspace_for_companion(
        &mut self,
        request: &crate::realtime::companion_host::DeleteWorkspaceRequest,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), Failure> {
        if self.current_account(ctx).as_ref() != Some(&request.account) {
            return Err(Failure::Gone);
        }
        let root = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == request.workspace_id)
            .map(|workspace| workspace.root.clone())
            .ok_or(Failure::Gone)?;
        if root != request.root {
            return Err(Failure::Gone);
        }
        if self.delete_workspace_for_cli(&root, ctx).ok {
            Ok(())
        } else {
            Err(Failure::Unavailable)
        }
    }

    /// The new pane-tab's id in this window.
    fn open_tab_for_companion(
        &mut self,
        request: &NewTabRequest,
        ctx: &mut ViewContext<Self>,
    ) -> Result<u64, Failure> {
        // Only for the account this window published its catalog under.
        if self.current_account(ctx).as_ref() != Some(&request.account) {
            return Err(Failure::Gone);
        }
        let root = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == request.workspace_id)
            .map(|workspace| workspace.root.clone())
            .ok_or(Failure::Gone)?;
        let tab = match request.kind {
            NewTabKind::Terminal => {
                self.create_tab_with_cwd_focus(ctx, None, None, root.clone(), Some(root), false)
            }
            NewTabKind::Browser => self.create_browser_tab_focus(None, Some(root), false, ctx),
            NewTabKind::DragentCli => {
                let tab = self.create_tab_with_cwd_focus(
                    ctx,
                    Some(DRAGENT_CLI_LABEL.to_string()),
                    None,
                    root.clone(),
                    Some(root.clone()),
                    false,
                );
                if let WorkspacePane::Terminal(terminal) = &tab.pane {
                    // As the header's item does: WSL wraps local commands.
                    let command = if crate::remote_fs::resolve(&root).is_some() {
                        DRAGENT_CLI_COMMAND.to_string()
                    } else {
                        crate::settings::settings_tasks_hosts::wrap_local_command(
                            DRAGENT_CLI_COMMAND,
                        )
                    };
                    ctx.update_view(terminal, |terminal, terminal_ctx| {
                        terminal.submit_shell_command_preserving_focus(&command, terminal_ctx)
                    });
                }
                tab
            }
        };
        let tab_id = tab.id as u64;
        self.add_parked_surface(request.workspace_id, tab);
        if request.kind == NewTabKind::DragentCli {
            if let Some(surface) = self.parked_workspace_tabs.last_mut() {
                surface.name = DRAGENT_CLI_LABEL.to_string();
            }
        }
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        // The next render publishes the tab, which is what the phone waits for.
        ctx.notify();
        Ok(tab_id)
    }

    pub(super) fn current_account(&self, ctx: &ViewContext<Self>) -> Option<AccountKey> {
        use crate::auth::{AuthManager, AuthStatus};
        if self.organisation_id != dragent_paths::organisation_id() {
            return None;
        }
        match AuthManager::as_ref(ctx).status() {
            AuthStatus::LoggedIn(user) => Some(AccountKey::new(
                user.user_id.clone().unwrap_or_default(),
                self.organisation_id.clone(),
            )),
            _ => None,
        }
    }
}
