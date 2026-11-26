//! The pop-up terminal over Settings, in the real workspace with a real shell
//! and a program running in it: what is typed must reach that program.

use super::view::WorkspaceView;
use crate::realtime::manager::SessionManager;
use std::time::{Duration, Instant};
use warpui::geometry::vector::Vector2F;
use warpui::platform::WindowStyle;
use warpui::r#async::Timer;
use warpui::{App, ReadView, TypedActionView, UpdateView};
use warpui_core::{Event, WindowInvalidation};

/// A private data folder and shell environment, as a test of its own.
#[test]
fn typing_reaches_a_running_program_in_the_popup_terminal() {
    let sandbox = tempfile::tempdir().unwrap();
    let log = sandbox.path().join("test.log");
    let out = std::fs::File::create(&log).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::popup_terminal_tests::isolated_popup_typing",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DRAGENT_HOME", sandbox.path())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timed out: {}", std::fs::read_to_string(&log).unwrap());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = std::fs::read_to_string(&log).unwrap();
    println!("{text}");
    assert!(status.success(), "{text}");
}

#[test]
#[ignore = "launched by the parent test in a private DRAGENT_HOME"]
fn isolated_popup_typing() {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let root = dragent_paths::home().join("project");
        std::fs::create_dir_all(&root).unwrap();
        app.update(|ctx| {
            ctx.add_singleton_model(crate::auth::AuthManager::preview);
            ctx.add_singleton_model(crate::settings::AccountStore::new);
            ctx.add_singleton_model(crate::settings::OrgStore::new);
            ctx.add_singleton_model(crate::organisation_context::OrganisationContext::preview);
            ctx.add_singleton_model(crate::settings::machines_store::MachinesStore::preview);
            ctx.add_singleton_model(crate::automations::AutomationRuntime::preview);
            ctx.add_singleton_model(crate::cli::inbox::CliInbox::preview);
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
        let persisted = dragent_persistence::PersistedSession {
            session: dragent_persistence::SerializedSession {
                tabs: Vec::new(),
                active_tab_id: 0,
                active_pane_uuid: None,
                chrome: Default::default(),
                layouts: Default::default(),
                document: dragent_persistence::SerializedDocumentSession {
                    root: root.clone(),
                    ..Default::default()
                },
            },
            blocks: Default::default(),
            agent_blocks: Default::default(),
            working_copies: Vec::new(),
            workspace_work_items: Vec::new(),
            workspace_events: Vec::new(),
            canonical_workspace_events: Vec::new(),
        };
        let (window, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            WorkspaceView::build_prepared(
                ctx,
                true,
                (Some(persisted), dragent_persistence::WriterHandles::noop()),
            )
        });
        workspace.update(&mut app, |view, ctx| {
            view.persists_session_snapshot = false;
            view.open_settings(ctx);
        });
        let presenter = app.presenter(window).unwrap();
        let mut failures = Vec::new();
        for (program, body) in [
            ("line", "read -r x"),
            ("no-echo", "stty -echo; read -r x; stty echo"),
            (
                "raw",
                "stty raw -echo; x=$(dd bs=1 count=2 2>/dev/null); stty sane",
            ),
            (
                "alt-screen",
                r"printf '\033[?1049h'; read -r x; printf '\033[?1049l'",
            ),
        ] {
            for scenario in ["untouched", "click", "blur-click", "blur-heading", "paste"] {
                let ready = root.join(format!("{program}-{scenario}.ready"));
                let got = root.join(format!("{program}-{scenario}.got"));
                let script = root.join(format!("{program}-{scenario}.sh"));
                std::fs::write(
                    &script,
                    format!(
                        "touch {}; {body}; printf %s \"$x\" > {}\n",
                        ready.display(),
                        got.display()
                    ),
                )
                .unwrap();
                workspace.update(&mut app, |view, ctx| {
                    view.open_page_command_terminal(
                        "Sign-in",
                        Some(root.clone()),
                        &format!("bash {}", script.display()),
                        ctx,
                    );
                });
                let terminal = workspace
                    .read(&app, |view, _| view.prompt_install_terminal.clone())
                    .unwrap();
                let editor = terminal.read(&app, |view, _| view.command_editor.clone());
                let settings = workspace.read(&app, |view, _| view.settings_view.clone());
                let rebuild = |app: &mut App| {
                    app.update(|ctx| {
                        let mut presenter = presenter.borrow_mut();
                        presenter.invalidate(
                            WindowInvalidation {
                                updated: [
                                    workspace.id(),
                                    terminal.id(),
                                    editor.id(),
                                    settings.id(),
                                ]
                                .into_iter()
                                .collect(),
                                ..Default::default()
                            },
                            ctx,
                        );
                        presenter.build_scene(Vector2F::new(1400.0, 900.0), 1.0, None, ctx);
                    });
                };
                let started = Instant::now();
                while !ready.exists() && started.elapsed() < Duration::from_secs(8) {
                    rebuild(&mut app);
                    Timer::after(Duration::from_millis(50)).await;
                }
                Timer::after(Duration::from_millis(300)).await;
                rebuild(&mut app);
                // As the window does: keybindings on the focused view's chain
                // first, then the elements, then the actions they dispatched.
                let send = |app: &mut App, event: Event| {
                    if let Event::KeyDown { keystroke, .. } = &event {
                        if let Some(id) = app.focused_view_id(window) {
                            let chain = presenter.borrow().ancestors(id);
                            if app
                                .dispatch_keystroke(window, &chain, keystroke, false)
                                .unwrap_or(false)
                            {
                                return;
                            }
                        }
                    }
                    app.update(|ctx| {
                        let result = presenter.borrow_mut().dispatch_event(event, ctx);
                        for dispatched in result.actions {
                            use warpui_core::presenter::DispatchedActionKind;
                            if let DispatchedActionKind::Typed(action) = dispatched.kind {
                                ctx.dispatch_typed_action_for_view(
                                    window,
                                    dispatched.view_id,
                                    action.as_ref(),
                                );
                            }
                        }
                    });
                };
                if scenario.starts_with("blur") {
                    workspace.update(&mut app, |_, ctx| ctx.focus_self());
                    rebuild(&mut app);
                }
                if scenario.contains("click") || scenario == "blur-heading" {
                    let body = presenter
                        .borrow()
                        .position_cache()
                        .get_position("popup_terminal_body")
                        .expect("the sheet is on screen");
                    let place = match scenario {
                        // The sheet's description, just above its terminal.
                        "blur-heading" => {
                            Vector2F::new(body.center().x(), body.origin().y() - 20.0)
                        }
                        _ => body.center(),
                    };
                    send(
                        &mut app,
                        Event::LeftMouseDown {
                            position: place,
                            modifiers: Default::default(),
                            click_count: 1,
                            is_first_mouse: false,
                        },
                    );
                    send(
                        &mut app,
                        Event::LeftMouseUp {
                            position: place,
                            modifiers: Default::default(),
                        },
                    );
                    rebuild(&mut app);
                }
                let (running, alt, focus_flag, notice) = terminal.read(&app, |view, app| {
                    let model = view.terminal_model();
                    let (running, alt) =
                        model.read(app, |m, _| (m.is_command_running(), m.alt_screen_active()));
                    (
                        running,
                        alt,
                        view.pane_has_focus(app),
                        view.host_notice_active,
                    )
                });
                let focused = app.focused_view_id(window);
                assert!(
                    workspace.read(&app, |view, _| view.prompt_install_terminal.is_some()),
                    "{program}/{scenario}: the click closed the pop-up"
                );
                if scenario == "paste" {
                    app.update(|ctx| {
                        ctx.clipboard()
                            .write(warpui_core::clipboard::ClipboardContent::plain_text(
                                "hi".into(),
                            ))
                    });
                }
                let keys: &[&str] = match scenario {
                    "paste" => &["cmd-v", "enter"],
                    _ => &["h", "i", "enter"],
                };
                for &key in keys {
                    send(
                        &mut app,
                        Event::KeyDown {
                            keystroke: warpui_core::keymap::Keystroke::parse(key).unwrap(),
                            chars: if key == "enter" {
                                "\r".into()
                            } else {
                                key.into()
                            },
                            details: Default::default(),
                            is_composing: false,
                        },
                    );
                    rebuild(&mut app);
                }
                let started = Instant::now();
                while !got.exists() && started.elapsed() < Duration::from_secs(2) {
                    Timer::after(Duration::from_millis(50)).await;
                }
                Timer::after(Duration::from_millis(100)).await;
                let text = std::fs::read_to_string(&got).unwrap_or_default();
                let line = format!(
                    "{program}/{scenario}: ready={} got={text:?} running={running} alt={alt} \
                     in_terminal={focus_flag} notice={notice} focused={focused:?} \
                     terminal={:?} editor={:?} settings={:?} workspace={:?}",
                    ready.exists(),
                    terminal.id(),
                    editor.id(),
                    settings.id(),
                    workspace.id()
                );
                println!("{line}");
                if text != "hi" {
                    failures.push(line);
                }
            }
        }
        // Closing the pop-up gives the keyboard back to Settings.
        workspace.update(&mut app, |view, ctx| {
            view.handle_action(&super::WorkspaceAction::ClosePromptAgentInstall, ctx)
        });
        let settings = workspace.read(&app, |view, _| view.settings_view.id());
        assert_eq!(app.focused_view_id(window), Some(settings));
        assert!(failures.is_empty(), "keys lost:\n{}", failures.join("\n"));
    });
}
