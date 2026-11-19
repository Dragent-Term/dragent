//! Exercise focus through the real terminal view and its child input editors,
//! without starting a shell, host daemon, or network connection.

use super::TerminalView;
use crate::realtime::manager::SessionManager;
use std::sync::Arc;
use warpui::elements::{ChildView, ConstrainedBox, Element, Flex, ParentElement};
use warpui::geometry::vector::Vector2F;
use warpui::platform::WindowStyle;
use warpui::{App, AppContext, Entity, ReadView, TypedActionView, UpdateView, View, ViewHandle};
use warpui_core::WindowInvalidation;

struct Pair {
    panes: [ViewHandle<TerminalView>; 2],
    active: usize,
    selections: usize,
}

impl Entity for Pair {
    type Event = ();
}

impl TypedActionView for Pair {
    type Action = ();
}

impl View for Pair {
    fn ui_name() -> &'static str {
        "TerminalFocusRegression"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let mut row = Flex::row();
        for pane in &self.panes {
            row = row.with_child(
                ConstrainedBox::new(ChildView::new(pane).finish())
                    .with_width(480.0)
                    .with_height(360.0)
                    .finish(),
            );
        }
        row.finish()
    }
}

fn with_pair(
    test: impl FnOnce(&mut App, &ViewHandle<Pair>, &[ViewHandle<TerminalView>; 2]) + 'static,
) {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let directory = tempfile::tempdir().unwrap();
        app.update(|ctx| {
            ctx.add_singleton_model(crate::auth::AuthManager::preview);
            ctx.add_singleton_model(crate::settings::AccountStore::new);
            ctx.add_singleton_model(|_| SessionManager {
                session: None,
                forwards: Vec::new(),
                forward_error: None,
                forward_busy: false,
                #[cfg(not(target_family = "wasm"))]
                _forward_tunnel: None,
                idle_epoch: 0,
                share_busy: None,
                viewer_theme_id: None,
            });
        });
        let (window, pair) = app.add_window(WindowStyle::NotStealFocus, |ctx| Pair {
            panes: std::array::from_fn(|_| {
                ctx.add_typed_action_view(|ctx| {
                    TerminalView::new_remote(directory.path().to_path_buf(), Arc::new(|_| {}), ctx)
                })
            }),
            active: 0,
            selections: 0,
        });
        let panes = pair.read(&app, |view, _| view.panes.clone());
        // Build the actual view hierarchy so focus reaches a terminal when
        // its command editor receives the keyboard.
        let presenter = app.presenter(window).unwrap();
        app.update(|ctx| {
            let mut presenter = presenter.borrow_mut();
            presenter.invalidate(
                WindowInvalidation {
                    updated: [pair.id(), panes[0].id(), panes[1].id()]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(Vector2F::new(960.0, 360.0), 1.0, None, ctx);
        });
        test(&mut app, &pair, &panes);
    });
}

#[test]
fn switching_between_terminal_inputs_clears_the_previous_panes_focus() {
    with_pair(|app, pair, panes| {
        let editors = panes
            .each_ref()
            .map(|pane| pane.read(app, |view, _| view.command_editor.clone()));
        pair.update(app, |_, ctx| ctx.focus(&editors[0]));
        assert!(panes[0].read(app, |view, _| view.pane_has_focus));
        assert!(!panes[1].read(app, |view, _| view.pane_has_focus));

        pair.update(app, |_, ctx| ctx.focus(&editors[1]));
        assert!(panes[1].read(app, |view, _| view.pane_has_focus));
        assert!(
            !panes[0].read(app, |view, _| view.pane_has_focus),
            "a terminal must lose focus when its child editor hands the keyboard to another pane"
        );

        pair.update(app, |_, ctx| ctx.focus(&editors[0]));
        assert!(panes[0].read(app, |view, _| view.pane_has_focus));
        assert!(!panes[1].read(app, |view, _| view.pane_has_focus));
    });
}

#[test]
fn queued_focus_events_cannot_start_a_pane_selection_loop() {
    with_pair(|app, pair, panes| {
        for (index, pane) in panes.iter().enumerate() {
            let pane = pane.clone();
            pair.update(app, |_, ctx| {
                let handle = pane.clone();
                ctx.subscribe_to_view(&pane, move |pair, _, _, ctx| {
                    // The workspace follows the same terminal focus accessor
                    // and selects/refocuses a different pane when it owns input.
                    if pair.active != index
                        && handle.read(ctx, |view, app| view.pane_has_focus(app))
                    {
                        pair.active = index;
                        pair.selections += 1;
                        assert!(
                            pair.selections <= 8,
                            "terminal focus events entered a selection loop"
                        );
                        ctx.focus(&handle);
                    }
                });
            });
        }
        let editors = panes
            .each_ref()
            .map(|pane| pane.read(app, |view, _| view.command_editor.clone()));
        pair.update(app, |_, ctx| ctx.focus(&editors[0]));
        // Command launch briefly focuses the new pane before the CLI restores
        // focus. Its earlier event must not select it after focus was returned.
        pair.update(app, |_, ctx| {
            ctx.focus(&editors[1]);
            ctx.focus(&editors[0]);
        });
        assert_eq!(pair.read(app, |view, _| view.active), 0);

        // Even an outdated cached flag plus a late view event cannot reclaim
        // the keyboard. This was the feedback path in the captured hang stack.
        panes[1].update(app, |view, ctx| {
            view.pane_has_focus = true;
            ctx.emit(());
        });
        assert_eq!(
            pair.read(app, |view, _| (view.active, view.selections)),
            (0, 0)
        );
        assert!(!panes[1].read(app, |view, ctx| view.pane_has_focus(ctx)));

        // A real focus change still activates its pane and settles once.
        pair.update(app, |_, ctx| ctx.focus(&editors[1]));
        assert_eq!(
            pair.read(app, |view, _| (view.active, view.selections)),
            (1, 1)
        );
        assert!(!panes[0].read(app, |view, ctx| view.pane_has_focus(ctx)));
        assert!(panes[1].read(app, |view, ctx| view.pane_has_focus(ctx)));
    });
}

#[test]
fn background_command_submission_preserves_the_callers_keyboard_focus() {
    with_pair(|app, pair, panes| {
        let editor = panes[0].read(app, |view, _| view.command_editor.clone());
        pair.update(app, |_, ctx| ctx.focus(&editor));
        panes[1].update(app, |view, ctx| {
            view.submit_shell_command_preserving_focus("printf pane-command", ctx);
        });
        assert!(panes[0].read(app, |view, app| view.pane_has_focus(app)));
        assert!(!panes[1].read(app, |view, app| view.pane_has_focus(app)));
        assert!(panes[0].read(app, |view, _| view.current_running_command.is_none()));
        assert_eq!(
            panes[1].read(app, |view, _| view.current_running_command.as_deref()
                == Some("printf pane-command")),
            true
        );
    });
}
