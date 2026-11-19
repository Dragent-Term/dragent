//! Route browser feedback through the same workspace-scoped agents as diff comments.
use super::{pane::WorkspacePane, view::WorkspaceView};
use crate::browser::BrowserView;
use std::path::PathBuf;
use warpui::{ReadView, UpdateView, ViewContext, ViewHandle};

impl WorkspaceView {
    pub(super) fn request_browser_design_targets(
        &mut self,
        tab_id: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.prompt_agent_checking {
            self.refresh_prompt_agents(ctx);
        }
        self.refresh_browser_design_targets(tab_id, ctx);
        self.wait_browser_design_targets(tab_id, 30, ctx);
    }

    fn wait_browser_design_targets(
        &mut self,
        tab_id: usize,
        attempts: u8,
        ctx: &mut ViewContext<Self>,
    ) {
        if attempts == 0 {
            return;
        }
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_millis(200)),
            move |view, _, ctx| {
                if view.prompt_agent_checking {
                    view.wait_browser_design_targets(tab_id, attempts - 1, ctx);
                } else {
                    view.refresh_browser_design_targets(tab_id, ctx);
                }
            },
        );
    }

    pub(super) fn refresh_browser_design_targets(
        &mut self,
        tab_id: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some((browser, root)) = self.browser_design_context(tab_id) else {
            return;
        };
        let targets = self.feedback_agents(&root, ctx);
        ctx.update_view(&browser, |view, vc| view.set_design_targets(targets, vc));
    }

    fn browser_design_context(&self, tab_id: usize) -> Option<(ViewHandle<BrowserView>, PathBuf)> {
        self.tabs.iter().find_map(|tab| match &tab.pane {
            WorkspacePane::Browser(browser) if tab.id == tab_id => {
                let root = tab
                    .project_root
                    .clone()
                    .or_else(|| self.selected_project_root())?;
                Some((browser.clone(), root))
            }
            _ => None,
        })
    }

    pub(super) fn send_browser_design(
        &mut self,
        tab_id: usize,
        target: &str,
        prompt: &str,
        files: &[PathBuf],
        ctx: &mut ViewContext<Self>,
    ) {
        let Some((browser, root)) = self.browser_design_context(tab_id) else {
            let browser = self
                .parked_workspace_tabs
                .iter()
                .flat_map(|tab| &tab.panes)
                .find_map(|tab| match &tab.pane {
                    WorkspacePane::Browser(browser) if tab.id == tab_id => Some(browser.clone()),
                    _ => None,
                });
            if let Some(browser) = browser {
                ctx.update_view(&browser, |view, vc| {
                    view.finish_design_send(
                        Err("Return to this browser's workspace before sending feedback.".into()),
                        vc,
                    )
                });
            }
            return;
        };
        let target = self
            .feedback_agents(&root, ctx)
            .into_iter()
            .find(|agent| agent.key == target);
        let Some(target) = target else {
            ctx.update_view(&browser, |view, vc| {
                view.finish_design_send(
                    Err("That agent session has ended. Choose another agent.".into()),
                    vc,
                )
            });
            self.refresh_browser_design_targets(tab_id, ctx);
            return;
        };
        if !target.existing {
            // A new session opens where diff comments open theirs.
            let placement = super::project_sidebar::sidebar_diff_comments::Placement::from_settings(
                &crate::setup::UiSettings::load(),
            );
            let result = self.start_feedback_agent(
                &root,
                placement,
                &target.agent,
                prompt,
                files,
                ctx,
            );
            ctx.update_view(&browser, |view, vc| view.finish_design_send(result, vc));
            return;
        }
        // A session put to sleep wakes first; the feedback goes in once it's up.
        if let Some((sleeper, pane)) = self.asleep_target(&root, &target.key, ctx) {
            let (key, prompt, files) = (target.key.clone(), prompt.to_string(), files.to_vec());
            let woken: Box<dyn FnOnce(&mut Self, &mut ViewContext<Self>)> =
                Box::new(move |view, ctx| view.send_browser_design(tab_id, &key, &prompt, &files, ctx));
            let failed_browser = browser.clone();
            let failed: Box<dyn FnOnce(&mut Self, String, &mut ViewContext<Self>)> = Box::new(move |_, reason, ctx| {
                ctx.update_view(&failed_browser, |view, vc| view.finish_design_send(Err(reason), vc));
            });
            self.wake_then(sleeper, pane, woken, failed, ctx);
            return;
        }
        let terminal = self
            .feedback_terminals(&root)
            .into_iter()
            .find_map(|(tab, _)| match &tab.pane {
                WorkspacePane::Terminal(terminal)
                    if format!("existing:{}", tab.pane.pane_uuid(ctx)) == target.key =>
                {
                    Some(terminal.clone())
                }
                _ => None,
            });
        let Some(terminal) = terminal else {
            ctx.update_view(&browser, |view, vc| {
                view.finish_design_send(Err("The agent terminal is no longer open.".into()), vc)
            });
            return;
        };
        if target.agent == "dragent" {
            ctx.update_view(&terminal, |view, vc| {
                view.receive_context_files(files.iter().take(5).cloned().collect(), vc);
                view.submit_agent_query(prompt.to_string(), vc);
            });
            ctx.update_view(&browser, |view, vc| view.finish_design_send(Ok(()), vc));
            return;
        }
        // Delivered like a diff comment: pasted into the session, then Enter.
        let (writer, delay) = terminal.read(ctx, |view, vc| {
            (
                view.manager.writer().clone(),
                view.active_ai_tool(vc)
                    .map(|tool| tool.submit_cr_delay_ms())
                    .unwrap_or(100),
            )
        });
        let text: String = prompt
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
            .collect();
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result = (|| -> Result<(), String> {
                writer.write_confirmed(format!("\x1b[200~{text}\x1b[201~").as_bytes())?;
                std::thread::sleep(std::time::Duration::from_millis(delay.max(50)));
                writer.write_confirmed(b"\r")
            })();
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |_, result, ctx| {
            ctx.update_view(&browser, |view, vc| {
                view.finish_design_send(
                    result.unwrap_or_else(|_| Err("Feedback delivery was interrupted.".into())),
                    vc,
                )
            });
        });
    }
}
