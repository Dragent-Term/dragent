//! Native pointer events through the real explorer rows and agent composers.
//! The workspace uses private storage and inert terminals, with no agent runs.

use super::{project_sidebar, view::WorkspaceView, WorkspaceAction};
use crate::realtime::manager::SessionManager;
use crate::terminal::TerminalView;
use crate::theme::Theme;
use std::{cell::RefCell, collections::HashMap, path::PathBuf, sync::Arc};
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Element, Flex,
    ParentElement, SavePosition, ScrollbarWidth,
};
use warpui::geometry::vector::vec2f;
use warpui::platform::WindowStyle;
use warpui::{
    App, AppContext, Entity, ReadModel, ReadView, TypedActionView, UpdateModel, UpdateView, View,
    ViewContext, ViewHandle,
};
use warpui_core::{Event, WindowInvalidation};

#[derive(Clone, Copy)]
enum Composer {
    Project,
    Shape,
    Terminal,
}

struct Fixture {
    workspace: ViewHandle<WorkspaceView>,
    terminal: ViewHandle<TerminalView>,
    composer: Composer,
    sent: usize,
    opened: usize,
    scroll: ClippedScrollStateHandle,
    hovers: RefCell<HashMap<String, warpui_core::elements::MouseStateHandle>>,
}

impl Entity for Fixture {
    type Event = ();
}

impl TypedActionView for Fixture {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match action {
            WorkspaceAction::SubmitPrompt => {
                self.sent += 1;
                // Hold the asynchronous operation in flight without creating
                // a worktree or sending a real request to an agent.
                ctx.update_view(&self.workspace, |view, ctx| {
                    view.project_start_busy = true;
                    ctx.notify();
                });
            }
            WorkspaceAction::OpenReadOnlyDocument { .. } => self.opened += 1,
            _ => ctx.update_view(&self.workspace, |view, ctx| view.handle_action(action, ctx)),
        }
        ctx.notify();
    }
}

impl View for Fixture {
    fn ui_name() -> &'static str {
        "ComposerInputRegression"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let content = self.workspace.read(app, |view, _| {
            let theme = Theme::dark();
            let id = app.root_view_id(view.window_id).unwrap();
            match self.composer {
                Composer::Project => super::project_prompt::prompt_box(
                    &theme,
                    view.mono_font,
                    id,
                    &view.prompt_editor,
                    super::action::PromptAgent::Dragent,
                    (false, &[]),
                    None,
                    false,
                    view.project_start_busy,
                    crate::voice::VoiceUiState::Idle,
                    0,
                    &self.hovers,
                ),
                Composer::Shape => super::shape_view_composer::composer(
                    &theme,
                    view.ui_font,
                    id,
                    Default::default(),
                    Default::default(),
                    view.shape_composer_editor.clone(),
                    false,
                    false,
                    false,
                    "",
                    None,
                    false,
                    false,
                    &view.shape_composer_attachments,
                    None,
                    crate::voice::VoiceUiState::Idle,
                    0,
                    false,
                    &[],
                    "Current model",
                    false,
                    Default::default(),
                    |_| Default::default(),
                    false,
                    false,
                    Default::default(),
                    false,
                ),
                Composer::Terminal => ChildView::new(&self.terminal).finish(),
            }
        });
        let source = self.workspace.read(app, |view, app| {
            view.project_sidebar_view.read(app, |sidebar, app| {
                sidebar.model.read(app, |model, _| {
                    project_sidebar::render_documents_section(
                        model,
                        sidebar.ui_font,
                        sidebar.mono_font,
                        &Theme::dark(),
                        sidebar,
                    )
                })
            })
        });
        let theme = Theme::dark();
        let source = ClippedScrollable::vertical(
            self.scroll.clone(),
            source,
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.main_text().into(),
            theme.background.into(),
        )
        .finish();
        Flex::row()
            .with_child(
                ConstrainedBox::new(
                    Align::new(SavePosition::new(content, "test_composer").finish()).finish(),
                )
                .with_width(650.0)
                .with_height(480.0)
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(SavePosition::new(source, "test_explorer").finish())
                    .with_width(300.0)
                    .with_height(480.0)
                    .finish(),
            )
            .finish()
    }
}

#[test]
fn explorer_files_drop_into_agent_composers_and_busy_send_does_not_repeat() {
    let sandbox = tempfile::tempdir().unwrap();
    let output = sandbox.path().join("test.log");
    let stdout = std::fs::File::create(&output).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::composer_input_tests::isolated_composer_input",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DRAGENT_HOME", sandbox.path())
        .stdout(stdout.try_clone().unwrap())
        .stderr(stdout)
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(40) {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "composer input did not settle: {}",
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
#[ignore = "launched by the parent test in a private DRAGENT_HOME"]
fn isolated_composer_input() {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let root = dragent_paths::home().join("fixture-project");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("context's notes.txt");
        std::fs::write(&path, "file context for the agent").unwrap();
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
        let (window, fixture) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let fixture_id = ctx.view_id();
            let workspace = ctx.add_typed_action_view(|ctx| {
                WorkspaceView::build_prepared(
                    ctx,
                    true,
                    (Some(persisted), dragent_persistence::WriterHandles::noop()),
                )
            });
            ctx.update_view(&workspace, |view, ctx| {
                view.persists_session_snapshot = false;
                ctx.update_view(&view.project_sidebar_view, |sidebar, _| {
                    sidebar.workspace_view_id = fixture_id;
                });
                ctx.update_model(&view.project_sidebar_model, |model, ctx| {
                    model.set_workspace_root(root.clone());
                    model.active_cwd = Some(root.clone());
                    let mut index = dragent_workspace_model::ProjectDocumentIndex::default();
                    index.replace(vec![dragent_workspace_model::DocumentEntry {
                        document: dragent_workspace_model::DocumentRef::from_relative_path(
                            PathBuf::from("context's notes.txt"),
                        )
                        .unwrap(),
                        is_directory: false,
                        byte_size: Some(26),
                        is_ignored: false,
                    }]);
                    model.set_documents(index, ctx);
                });
            });
            let terminal = ctx.add_typed_action_view(|ctx| {
                TerminalView::new_remote(root.clone(), Arc::new(|_| {}), ctx)
            });
            ctx.update_view(&terminal, |view, _| view.remote_read_only = false);
            Fixture {
                workspace,
                terminal,
                composer: Composer::Project,
                sent: 0,
                opened: 0,
                scroll: ClippedScrollStateHandle::new(),
                hovers: Default::default(),
            }
        });
        let presenter = app.presenter(window).unwrap();
        let rebuild = |app: &mut App| {
            app.update(|ctx| {
                let mut presenter = presenter.borrow_mut();
                presenter.invalidate(
                    WindowInvalidation {
                        updated: [fixture.id()].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
                presenter.build_scene(vec2f(950.0, 480.0), 1.0, None, ctx);
            });
        };
        let event = |app: &mut App, event| {
            app.update(|ctx| {
                let result = presenter.borrow_mut().dispatch_event(event, ctx);
                for dispatched in result.actions {
                    match dispatched.kind {
                        warpui_core::presenter::DispatchedActionKind::Typed(action) => {
                            ctx.dispatch_typed_action_for_view(
                                window,
                                dispatched.view_id,
                                action.as_ref(),
                            );
                        }
                        warpui_core::presenter::DispatchedActionKind::Legacy { name, arg } => {
                            ctx.dispatch_action_for_view(
                                window,
                                dispatched.view_id,
                                name,
                                arg.as_ref(),
                            );
                        }
                    }
                }
                result.handled
            })
        };
        let drag = |app: &mut App, destination| {
            rebuild(app);
            let source = presenter
                .borrow()
                .position_cache()
                .get_position("test_explorer")
                .unwrap()
                .origin()
                + vec2f(50.0, 12.0);
            event(
                app,
                Event::LeftMouseDown {
                    position: source,
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
            );
            event(
                app,
                Event::LeftMouseDragged {
                    position: source + vec2f(-15.0, 0.0),
                    modifiers: Default::default(),
                },
            );
            rebuild(app);
            event(
                app,
                Event::LeftMouseDragged {
                    position: destination,
                    modifiers: Default::default(),
                },
            );
            rebuild(app);
            event(
                app,
                Event::LeftMouseUp {
                    position: destination,
                    modifiers: Default::default(),
                },
            );
        };
        let workspace = fixture.read(&app, |view, _| view.workspace.clone());
        let terminal = fixture.read(&app, |view, _| view.terminal.clone());
        rebuild(&mut app);
        let source = presenter
            .borrow()
            .position_cache()
            .get_position("test_explorer")
            .unwrap()
            .origin()
            + vec2f(50.0, 12.0);
        event(
            &mut app,
            Event::LeftMouseDown {
                position: source,
                modifiers: Default::default(),
                click_count: 1,
                is_first_mouse: false,
            },
        );
        event(
            &mut app,
            Event::LeftMouseUp {
                position: source,
                modifiers: Default::default(),
            },
        );
        assert_eq!(
            fixture.read(&app, |view, _| view.opened),
            1,
            "a click still opens the file"
        );
        fixture.update(&mut app, |view, _| view.opened = 0);
        let destination = presenter
            .borrow()
            .position_cache()
            .get_position("test_composer")
            .unwrap()
            .center();
        drag(&mut app, destination);
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments.len()),
            1
        );
        assert_eq!(
            fixture.read(&app, |view, _| view.opened),
            0,
            "dragging must not open the file"
        );
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments[0]
                .any_path()
                .unwrap()
                .to_path_buf()),
            path
        );

        fixture.update(&mut app, |view, ctx| {
            view.composer = Composer::Shape;
            ctx.notify();
        });
        rebuild(&mut app);
        let destination = presenter
            .borrow()
            .position_cache()
            .get_position("test_composer")
            .unwrap()
            .center();
        drag(&mut app, destination);
        assert_eq!(
            workspace.read(&app, |view, _| view.shape_composer_attachments.len()),
            1
        );
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments.len()),
            1
        );

        fixture.update(&mut app, |view, ctx| {
            view.composer = Composer::Terminal;
            ctx.notify();
        });
        drag(&mut app, vec2f(200.0, 240.0));
        assert_eq!(
            terminal.read(&app, |view, _| view.pending_attachments.len()),
            1
        );

        terminal.update(&mut app, |view, ctx| {
            view.remote_ai_tool = Some(crate::terminal::model::input_mode::AiTool::Codex);
            ctx.update_view(&view.rich_input_editor, |editor, ctx| {
                editor.set_text("Inspect ", ctx)
            });
            ctx.notify();
        });
        drag(&mut app, vec2f(200.0, 240.0));
        assert_eq!(
            terminal.read(&app, |view, ctx| view
                .rich_input_editor
                .read(ctx, |editor, _| editor.get_text().to_string())),
            format!(
                "Inspect {}",
                crate::terminal::view::terminal_helpers::shell_quote(&path.to_string_lossy())
            )
        );
        assert_eq!(
            terminal.read(&app, |view, _| view.pending_attachments.len()),
            1,
            "CLI agents receive paths rather than built-in agent attachments"
        );

        terminal.update(&mut app, |view, ctx| {
            view.remote_read_only = true;
            ctx.notify();
        });
        drag(&mut app, vec2f(200.0, 240.0));
        assert_eq!(
            terminal.read(&app, |view, _| view.pending_attachments.len()),
            1
        );
        terminal.update(&mut app, |view, ctx| {
            view.remote_read_only = false;
            ctx.notify();
        });

        // A drop outside an input must not attach anything or move the file.
        drag(&mut app, vec2f(940.0, 470.0));
        assert_eq!(
            terminal.read(&app, |view, _| view.pending_attachments.len()),
            1
        );
        assert!(path.exists());

        fixture.update(&mut app, |view, ctx| {
            view.composer = Composer::Project;
            ctx.notify();
        });
        rebuild(&mut app);
        let submit = presenter
            .borrow()
            .position_cache()
            .get_position("prompt_submit")
            .unwrap();
        let click = |app: &mut App| {
            event(
                app,
                Event::LeftMouseDown {
                    position: submit.center(),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
            );
            event(
                app,
                Event::LeftMouseUp {
                    position: submit.center(),
                    modifiers: Default::default(),
                },
            );
        };
        click(&mut app);
        assert_eq!(fixture.read(&app, |view, _| view.sent), 1);
        rebuild(&mut app);
        let busy_bounds = presenter
            .borrow()
            .position_cache()
            .get_position("prompt_submit")
            .unwrap();
        assert_eq!(
            submit.size(),
            busy_bounds.size(),
            "busy state must not shift button size"
        );
        assert!(
            presenter
                .borrow()
                .scene()
                .unwrap()
                .layers()
                .flat_map(|layer| &layer.rects)
                .any(|rect| rect.bounds.size() == vec2f(12.0, 12.0)
                    && busy_bounds.contains_point(rect.bounds.center())
                    && rect.background == warpui::elements::Fill::Solid(Theme::dark().background)),
            "busy button must paint the filled stop square"
        );
        click(&mut app);
        assert_eq!(
            fixture.read(&app, |view, _| view.sent),
            1,
            "busy button must not submit again"
        );
        workspace.update(&mut app, |view, ctx| {
            view.handle_action(
                &WorkspaceAction::AttachPromptFiles {
                    paths: vec![path.clone()],
                },
                ctx,
            )
        });
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments.len()),
            1,
            "files must not be added to a prompt already being sent"
        );
        workspace.update(&mut app, |view, ctx| {
            view.project_start_busy = false;
            ctx.notify();
        });
        rebuild(&mut app);
        click(&mut app);
        assert_eq!(
            fixture.read(&app, |view, _| view.sent),
            2,
            "idle state must allow retry"
        );

        workspace.update(&mut app, |view, ctx| {
            view.project_start_busy = false;
            view.receive_prompt_context_files(vec![path.clone(); 6], ctx);
        });
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments.len()),
            5
        );
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_notice.clone()),
            Some("Maximum 5 attachments per message.".into())
        );
        let binary = root.join("unsupported.bin");
        std::fs::write(&binary, [0xff, 0xfe]).unwrap();
        workspace.update(&mut app, |view, ctx| {
            view.prompt_attachments.clear();
            view.receive_prompt_context_files(vec![binary, path.clone()], ctx);
        });
        assert_eq!(
            workspace.read(&app, |view, _| view.prompt_attachments.len()),
            1
        );
        assert!(workspace
            .read(&app, |view, _| view.prompt_notice.as_deref().is_some_and(
                |notice| notice.contains("not an image or text file")
            )));
    });
}
