//! Run the real workspace selection and CLI focus handlers in an isolated
//! process. All terminals have inert input sinks; no host or shell is started.

use super::pane::WorkspacePane;
use super::pane_layout::{PaneLayout, SplitSide};
use super::view::{PaneTab, WorkspaceView};
use crate::realtime::manager::SessionManager;
use crate::terminal::TerminalView;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use warpui::geometry::vector::Vector2F;
use warpui::platform::WindowStyle;
use warpui::{App, Entity, ReadView, TypedActionView, UpdateView};
use warpui_core::WindowInvalidation;

#[test]
fn workspace_command_pane_focus_settles_without_a_feedback_loop() {
    let sandbox = tempfile::tempdir().unwrap();
    let output = sandbox.path().join("test.log");
    let stdout = std::fs::File::create(&output).unwrap();
    let stderr = stdout.try_clone().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::pane_focus_tests::isolated_workspace_command_pane_focus",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DRAGENT_HOME", sandbox.path())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "workspace focus failed to settle: {}",
                std::fs::read_to_string(&output).unwrap()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(
        status.success(),
        "{}",
        std::fs::read_to_string(output).unwrap()
    );
}

#[test]
#[ignore = "launched by workspace_command_pane_focus_settles_without_a_feedback_loop in a private DRAGENT_HOME"]
fn isolated_workspace_command_pane_focus() {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let root = dragent_paths::home().join("fixture-project");
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
        let fixture = dragent_persistence::PersistedSession {
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
                (Some(fixture), dragent_persistence::WriterHandles::noop()),
            )
        });
        let events = Rc::new(Cell::new(0));
        let panes = workspace.update(&mut app, |view, ctx| {
            view.persists_session_snapshot = false;
            view.project_start_visible = false;
            view.work_item_tab_open = false;
            let panes: [_; 2] = std::array::from_fn(|index| {
                let terminal = ctx.add_typed_action_view(|ctx| {
                    TerminalView::new_remote(root.clone(), Arc::new(|_| {}), ctx)
                });
                let id = view.next_tab_id;
                view.next_tab_id += 1;
                // Bound a regression instead of allowing the test process to
                // freeze forever inside the framework's effect queue.
                let events = events.clone();
                ctx.subscribe_to_view(&terminal, move |_, _, _, _| {
                    events.set(events.get() + 1);
                    assert!(
                        events.get() < 100,
                        "workspace entered a terminal focus feedback loop"
                    );
                });
                let pane = WorkspacePane::Terminal(terminal.clone());
                let surface_id = view.register_surface(&pane, ctx);
                super::tabs_panes::subscribe_workspace_pane(
                    id,
                    &pane,
                    view.persistence_writer.sender(),
                    ctx,
                );
                view.tabs.push(PaneTab {
                    id,
                    surface_id,
                    title: format!("terminal {index}"),
                    custom_label: None,
                    color: None,
                    work_item_id: None,
                    project_root: Some(root.clone()),
                    hidden_from_sidebar: index != 0,
                    pane,
                });
                (id, terminal)
            });
            view.active_tab_id = panes[0].0;
            view.active_pane_id = Some(panes[0].1.id());
            let mut layout = PaneLayout::new(panes[0].0);
            layout.split_pane(panes[0].0, panes[1].0, SplitSide::Right);
            layout.focus(panes[0].0);
            view.split_tree = Some(layout);
            panes
        });
        let presenter = app.presenter(window).unwrap();
        app.update(|ctx| {
            let mut presenter = presenter.borrow_mut();
            presenter.invalidate(
                WindowInvalidation {
                    updated: [workspace.id(), panes[0].1.id(), panes[1].1.id()]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(Vector2F::new(1400.0, 900.0), 1.0, None, ctx);
        });
        let original_editor = panes[0].1.read(&app, |view, _| view.command_editor.clone());
        workspace.update(&mut app, |_, ctx| ctx.focus(&original_editor));
        events.set(0);
        let other_editor = panes[1].1.read(&app, |view, _| view.command_editor.clone());
        workspace.update(&mut app, |_, ctx| ctx.focus(&other_editor));
        assert_eq!(
            workspace.read(&app, |view, _| view.active_tab_id),
            panes[1].0
        );
        assert_eq!(
            events.get(),
            1,
            "following native child focus must not refocus its parent terminal"
        );
        workspace.update(&mut app, |_, ctx| ctx.focus(&original_editor));
        events.set(0);
        workspace.update(&mut app, |view, ctx| {
            // Both idle panes redirect parent focus into their input editors.
            // Interleave real CLI focus requests before those child-focus
            // effects have drained, as opening/restoring a split can do.
            for id in [panes[1].0, panes[0].0] {
                let answer = view.pane_tool_for_cli(
                    None,
                    None,
                    "focus",
                    &serde_json::json!({"id": id}),
                    ctx,
                );
                assert!(answer.ok, "{:?}", answer.error);
            }
        });
        events.set(0);
        workspace.update(&mut app, |view, ctx| {
            // The real command API followed by the real CLI/MCP focus-restore
            // handler, including update_active_pane and its view subscriptions.
            panes[1].1.update(ctx, |terminal, ctx| {
                terminal.submit_shell_command("printf pane-command", ctx)
            });
            let answer = view.pane_tool_for_cli(
                None,
                None,
                "focus",
                &serde_json::json!({"id": panes[0].0}),
                ctx,
            );
            assert!(answer.ok, "{:?}", answer.error);
        });
        assert_eq!(
            workspace.read(&app, |view, _| view.active_tab_id),
            panes[0].0
        );
        assert!(panes[0].1.read(&app, |view, app| view.pane_has_focus(app)));
        assert!(!panes[1].1.read(&app, |view, app| view.pane_has_focus(app)));
        assert!(
            events.get() < 20,
            "focus changes must settle without repeated selection"
        );
        assert!(panes[0]
            .1
            .read(&app, |view, _| view.current_running_command.is_none()));
        assert_eq!(
            panes[1]
                .1
                .read(&app, |view, _| view.current_running_command.clone()),
            Some("printf pane-command".into())
        );
    });
}

#[test]
fn switching_back_from_an_opened_worker_settles_without_a_feedback_loop() {
    run_isolated(
        "workspace::pane_focus_tests::isolated_switch_back_from_a_worker_surface",
        "focus failed to settle switching between workspace surfaces",
    );
}

/// Run one ignored test in a child process with a private DRAGENT_HOME,
/// failing (instead of hanging) if it never finishes.
fn run_isolated(test: &str, what: &str) {
    let sandbox = tempfile::tempdir().unwrap();
    let output = sandbox.path().join("test.log");
    let stdout = std::fs::File::create(&output).unwrap();
    let stderr = stdout.try_clone().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture", "--test-threads=1"])
        .env("DRAGENT_HOME", sandbox.path())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what}: {}", std::fs::read_to_string(&output).unwrap());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(status.success(), "{}", std::fs::read_to_string(output).unwrap());
}

/// A lead's workspace on screen, a worker's workspace in the background:
/// open the worker (as Agent Coordination does), then go back to the lead.
#[test]
#[ignore = "launched by switching_back_from_an_opened_worker_settles_without_a_feedback_loop in a private DRAGENT_HOME"]
fn isolated_switch_back_from_a_worker_surface() {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let root = dragent_paths::home().join("lead-project");
        let worker_root = dragent_paths::home().join("worker-project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&worker_root).unwrap();
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
        let fixture = dragent_persistence::PersistedSession {
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
                (Some(fixture), dragent_persistence::WriterHandles::noop()),
            )
        });
        let events = Rc::new(Cell::new(0));
        let (lead, worker, lead_workspace) = workspace.update(&mut app, |view, ctx| {
            view.persists_session_snapshot = false;
            view.project_start_visible = false;
            view.work_item_tab_open = false;
            let mut make = |view: &mut WorkspaceView, ctx: &mut warpui::ViewContext<WorkspaceView>, root: &std::path::Path, title: &str| {
                let terminal = ctx.add_typed_action_view(|ctx| {
                    TerminalView::new_remote(root.to_path_buf(), Arc::new(|_| {}), ctx)
                });
                let id = view.next_tab_id;
                view.next_tab_id += 1;
                let events = events.clone();
                ctx.subscribe_to_view(&terminal, move |_, _, _, _| {
                    events.set(events.get() + 1);
                    assert!(
                        events.get() < 200,
                        "workspace entered a terminal focus feedback loop"
                    );
                });
                let pane = WorkspacePane::Terminal(terminal.clone());
                let surface_id = view.register_surface(&pane, ctx);
                super::tabs_panes::subscribe_workspace_pane(
                    id,
                    &pane,
                    view.persistence_writer.sender(),
                    ctx,
                );
                (
                    PaneTab {
                        id,
                        surface_id,
                        title: title.into(),
                        custom_label: None,
                        color: None,
                        work_item_id: None,
                        project_root: Some(root.to_path_buf()),
                        hidden_from_sidebar: false,
                        pane,
                    },
                    terminal,
                )
            };
            let (lead_tab, lead) = make(view, ctx, &root, "lead");
            let lead_id = lead_tab.id;
            view.active_tab_id = lead_id;
            view.active_pane_id = Some(lead.id());
            view.tabs.push(lead_tab);
            view.split_tree = None;
            view.assign_surface_to_workspace(root.clone(), "lead".into());
            let lead_workspace = view
                .workspaces
                .iter()
                .find(|workspace| workspace.tab_ids.contains(&view.active_workspace_tab_id))
                .map(|workspace| workspace.id)
                .unwrap();
            let (worker_tab, worker) = make(view, ctx, &worker_root, "t4 · worker");
            let worker_id = worker_tab.id;
            view.park_new_surface(worker_root.clone(), "worker".into(), worker_tab);
            ((lead_id, lead), (worker_id, worker), lead_workspace)
        });
        let presenter = app.presenter(window).unwrap();
        let draw = |app: &mut warpui::App| {
            app.update(|ctx| {
                let mut presenter = presenter.borrow_mut();
                presenter.invalidate(
                    WindowInvalidation {
                        updated: [workspace.id(), lead.1.id(), worker.1.id()]
                            .into_iter()
                            .collect(),
                        ..Default::default()
                    },
                    ctx,
                );
                presenter.build_scene(Vector2F::new(1400.0, 900.0), 1.0, None, ctx);
            });
        };
        // Both run Claude Code: full screen, reporting focus changes.
        for terminal in [&lead.1, &worker.1] {
            terminal.read(&app, |view, app| {
                view.model.read(app, |model, _| {
                    model.feed_for_test(
                        b"\x1b]7979;precmd;0;/tmp\x07\x1b]7979;preexec;claude\x07\x1b[?1049h\x1b[?1004h\x1b[?2004h",
                    )
                })
            });
        }
        draw(&mut app);
        let lead_editor = lead.1.read(&app, |view, _| view.command_editor.clone());
        workspace.update(&mut app, |_, ctx| ctx.focus(&lead_editor));
        draw(&mut app);

        // Open the worker from Agent Coordination: its tab, in its own surface.
        events.set(0);
        workspace.update(&mut app, |view, ctx| view.open_teams(None, ctx));
        draw(&mut app);
        workspace.update(&mut app, |view, ctx| {
            // What clicking a worker on the page does, in one update: the
            // page closes (focusing the pane on screen, the lead) and the
            // worker's tab is selected, which queues its focus behind it.
            view.close_teams(ctx);
            view.handle_action(&super::action::WorkspaceAction::SelectTab { tab_id: worker.0 }, ctx);
        });
        draw(&mut app);
        assert!(events.get() < 50, "opening the worker kept refocusing: {}", events.get());
        assert!(worker.1.read(&app, |view, app| view.pane_has_focus(app)));
        assert!(!lead.1.read(&app, |view, app| view.pane_has_focus(app)));
        // The page is opened again on top, as happened before the switch.
        workspace.update(&mut app, |view, ctx| view.open_teams(None, ctx));
        draw(&mut app);

        // Back to the lead's workspace from the sidebar.
        events.set(0);
        workspace.update(&mut app, |view, ctx| {
            view.handle_action(
                &super::action::WorkspaceAction::SelectProjectWorkspace {
                    workspace_id: lead_workspace,
                },
                ctx,
            );
        });
        draw(&mut app);
        draw(&mut app);
        assert!(events.get() < 50, "switching back kept refocusing: {}", events.get());
        assert_eq!(workspace.read(&app, |view, _| view.active_tab_id), lead.0);
        assert!(lead.1.read(&app, |view, app| view.pane_has_focus(app)));
        assert!(!worker.1.read(&app, |view, app| view.pane_has_focus(app)));
    });
}
