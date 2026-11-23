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

/// A terminal beside a text field that isn't the terminal's (a browser pane's
/// address bar, for instance).
struct Beside {
    terminal: ViewHandle<TerminalView>,
    field: ViewHandle<crate::workspace::InputEditor>,
}

impl Entity for Beside {
    type Event = ();
}

impl TypedActionView for Beside {
    type Action = ();
}

impl View for Beside {
    fn ui_name() -> &'static str {
        "TerminalBesideField"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Flex::row()
            .with_child(
                ConstrainedBox::new(ChildView::new(&self.terminal).finish())
                    .with_width(480.0)
                    .with_height(360.0)
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(ChildView::new(&self.field).finish())
                    .with_width(480.0)
                    .with_height(30.0)
                    .finish(),
            )
            .finish()
    }
}

/// Clipboard and palette shortcuts reach every terminal in the window (its
/// key handler sees keys that no binding took). Focused in another pane's
/// field, Cmd+V must not be taken by the terminal laid out before it.
#[test]
fn a_paste_in_another_panes_field_is_not_taken_by_a_terminal() {
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
        let (window, beside) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let terminal = ctx.add_typed_action_view(|ctx| {
                TerminalView::new_remote(directory.path().to_path_buf(), Arc::new(|_| {}), ctx)
            });
            let font = terminal.read(ctx, |view, _| view.mono_family);
            let theme = crate::theme::Theme::dark();
            let field = ctx.add_typed_action_view(|inner| {
                crate::workspace::InputEditor::new(
                    "Search or enter an address",
                    font,
                    12.5,
                    theme.main_text(),
                    theme.sub_text(),
                    theme.accent,
                    theme,
                    crate::workspace::InputMode::Plain {
                        key: "test-address".into(),
                    },
                    inner.focus_handle(),
                )
            });
            Beside { terminal, field }
        });
        let (terminal, field) =
            beside.read(&app, |view, _| (view.terminal.clone(), view.field.clone()));
        // An interactive terminal, as an agent's or a shell's pane is (a
        // remote one starts as a view-only viewer).
        terminal.update(&mut app, |view, ctx| view.set_remote_read_only(false, ctx));
        let presenter = app.presenter(window).unwrap();
        app.update(|ctx| {
            let mut presenter = presenter.borrow_mut();
            presenter.invalidate(
                WindowInvalidation {
                    updated: [beside.id(), terminal.id(), field.id()]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(Vector2F::new(960.0, 360.0), 1.0, None, ctx);
        });
        beside.update(&mut app, |_, ctx| ctx.focus(&field));
        assert!(!terminal.read(&app, |view, app| view.pane_has_focus(app)));
        // Render again so the terminal's key handler sees it isn't focused.
        app.update(|ctx| {
            let mut presenter = presenter.borrow_mut();
            presenter.invalidate(
                WindowInvalidation {
                    updated: [beside.id(), terminal.id(), field.id()]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(Vector2F::new(960.0, 360.0), 1.0, None, ctx);
        });

        let terminal_id = terminal.id();
        for chord in ["cmd-v", "cmd-c", "cmd-r"] {
            let taken = app.update(|ctx| {
                let event = warpui_core::Event::KeyDown {
                    keystroke: warpui_core::keymap::Keystroke::parse(chord).unwrap(),
                    chars: String::new(),
                    details: Default::default(),
                    is_composing: false,
                };
                let result = presenter.borrow_mut().dispatch_event(event, ctx);
                result
                    .actions
                    .iter()
                    .any(|action| action.view_id == terminal_id)
            });
            assert!(
                !taken,
                "{chord} in another pane's field was taken by the terminal"
            );
        }

        // A view-only viewer (shared session) doesn't take them either.
        terminal.update(&mut app, |view, ctx| view.set_remote_read_only(true, ctx));
        app.update(|ctx| {
            let mut presenter = presenter.borrow_mut();
            presenter.invalidate(
                WindowInvalidation {
                    updated: [beside.id(), terminal.id(), field.id()]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(Vector2F::new(960.0, 360.0), 1.0, None, ctx);
        });
        let taken = app.update(|ctx| {
            let event = warpui_core::Event::KeyDown {
                keystroke: warpui_core::keymap::Keystroke::parse("cmd-v").unwrap(),
                chars: String::new(),
                details: Default::default(),
                is_composing: false,
            };
            let result = presenter.borrow_mut().dispatch_event(event, ctx);
            result
                .actions
                .iter()
                .any(|action| action.view_id == terminal_id)
        });
        assert!(
            !taken,
            "a view-only viewer took a paste typed in another pane's field"
        );
    });
}

/// The pop-up terminal over a page (Settings → Agent accounts → sign in),
/// inside the workspace's "click on nothing" handler, which takes the
/// keyboard back (`WorkspaceAction::BlurAll`).
struct Popup {
    terminal: ViewHandle<TerminalView>,
    hovers: std::cell::RefCell<
        std::collections::HashMap<String, warpui_core::elements::MouseStateHandle>,
    >,
}

#[derive(Debug, Clone)]
enum PopupAction {
    BlurAll,
}

impl Entity for Popup {
    type Event = ();
}

impl TypedActionView for Popup {
    type Action = PopupAction;

    fn handle_action(&mut self, action: &PopupAction, ctx: &mut warpui::ViewContext<Self>) {
        match action {
            PopupAction::BlurAll => ctx.focus_self(),
        }
    }
}

impl View for Popup {
    fn ui_name() -> &'static str {
        "PopupTerminalOverPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = crate::theme::Theme::dark();
        let font = self.terminal.read(app, |view, _| view.mono_family);
        let id = app.root_view_id(self.terminal.window_id(app)).unwrap();
        let modal = crate::workspace::render_embedded_terminal_modal(
            id,
            &theme,
            font,
            &self.hovers,
            "Claude account sign-in",
            "",
            "Running on this computer.",
            Some(
                warpui::elements::SavePosition::new(
                    ChildView::new(&self.terminal).finish(),
                    "popup_terminal",
                )
                .finish(),
            ),
            false,
            None,
            false,
            false,
        );
        warpui::elements::EventHandler::new(warpui::elements::Align::new(modal).finish())
            .on_left_mouse_down(|event_ctx, _, _| {
                event_ctx.dispatch_typed_action(PopupAction::BlurAll);
                warpui::elements::DispatchEventResult::PropagateToParent
            })
            .finish()
    }
}

/// A click in the pop-up terminal keeps the keyboard there, so the sign-in
/// code can be typed or pasted into it.
#[test]
fn a_click_in_the_popup_terminal_keeps_the_keyboard_there() {
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
        let (window, popup) = app.add_window(WindowStyle::NotStealFocus, |ctx| Popup {
            terminal: ctx.add_typed_action_view(|ctx| {
                TerminalView::new_remote(directory.path().to_path_buf(), Arc::new(|_| {}), ctx)
            }),
            hovers: Default::default(),
        });
        let terminal = popup.read(&app, |view, _| view.terminal.clone());
        terminal.update(&mut app, |view, ctx| view.set_remote_read_only(false, ctx));
        let presenter = app.presenter(window).unwrap();
        let rebuild = |app: &mut App| {
            app.update(|ctx| {
                let mut presenter = presenter.borrow_mut();
                presenter.invalidate(
                    WindowInvalidation {
                        updated: [popup.id(), terminal.id()].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
                presenter.build_scene(Vector2F::new(1000.0, 700.0), 1.0, None, ctx);
            });
        };
        rebuild(&mut app);
        popup.update(&mut app, |_, ctx| ctx.focus(&terminal));
        assert!(terminal.read(&app, |view, app| view.pane_has_focus(app)));
        rebuild(&mut app);

        let inside = presenter
            .borrow()
            .position_cache()
            .get_position("popup_terminal")
            .unwrap()
            .center();
        app.update(|ctx| {
            let event = warpui_core::Event::LeftMouseDown {
                position: inside,
                modifiers: Default::default(),
                click_count: 1,
                is_first_mouse: false,
            };
            let result = presenter.borrow_mut().dispatch_event(event, ctx);
            for dispatched in result.actions {
                if let warpui_core::presenter::DispatchedActionKind::Typed(action) =
                    dispatched.kind
                {
                    ctx.dispatch_typed_action_for_view(
                        window,
                        dispatched.view_id,
                        action.as_ref(),
                    );
                }
            }
        });
        assert!(
            terminal.read(&app, |view, app| view.pane_has_focus(app)),
            "a click in the pop-up terminal gave the keyboard back to the page"
        );
    });
}

/// The pop-up terminal's shell gets its system-browser statement once, in
/// front of the first command; the block shows the command alone.
#[test]
fn the_popup_terminal_writes_its_shell_setup_once() {
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
        let written = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = written.clone();
        let (_, popup) = app.add_window(WindowStyle::NotStealFocus, |ctx| Popup {
            terminal: ctx.add_typed_action_view(|ctx| {
                TerminalView::new_remote(
                    directory.path().to_path_buf(),
                    Arc::new(move |bytes: &[u8]| sink.lock().unwrap().extend_from_slice(bytes)),
                    ctx,
                )
            }),
            hovers: Default::default(),
        });
        let terminal = popup.read(&app, |view, _| view.terminal.clone());
        terminal.update(&mut app, |view, ctx| {
            view.set_remote_read_only(false, ctx);
            view.use_system_browser(Some("export BROWSER=open; ".into()));
            view.submit_shell_command_preserving_focus("gh auth login", ctx);
        });
        let first = String::from_utf8_lossy(&written.lock().unwrap()).into_owned();
        assert!(first.contains("export BROWSER=open; gh auth login"), "{first:?}");
        terminal.read(&app, |view, _| {
            assert!(view.links_open_in_system_browser);
            assert_eq!(view.pending_shell_setup, None);
            assert_eq!(view.current_running_command.as_deref(), Some("gh auth login"));
        });
        written.lock().unwrap().clear();
        terminal.update(&mut app, |view, ctx| {
            view.submit_shell_command_preserving_focus("echo again", ctx);
        });
        let second = String::from_utf8_lossy(&written.lock().unwrap()).into_owned();
        assert!(second.contains("echo again") && !second.contains("BROWSER"), "{second:?}");
    });
}
