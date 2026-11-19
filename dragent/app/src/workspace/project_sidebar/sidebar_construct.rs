//! Sidebar construction and configuration.
//!
//! Blocks here are impl ProjectSidebarView: same type, same behavior.

use warpui::ReadModel;
use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};
use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::process_ext::NoWindow;
use crate::terminal::render::markdown::render_markdown_document_with_links;
use crate::theme::Theme;
use crate::workspace::{
    editor_render,
    share_sheet::{avatar_sized, avatar_sized_from_source},
    EditorContextMenuCommand, InlineRenameState, InputEditor, InputEditorEvent, InputMode,
    LanguageNavigationCapabilities, LanguageNavigationKind, WorkspaceAction,
};
use dragent_editor::{
    find_buffer_matches, lsp_range_to_char_range, BufferSearchOptions, LineChangeKind, TextRange,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, DraggableState, Element, Empty,
    EventHandler, Expanded, Fill, Flex, Highlight, Image, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Point, Resizable,
    ResizableStateHandle, SavePosition, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::r#async::Timer;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EntityId, EventContext, LayoutContext, ModelHandle,
    PaintContext, SizeConstraint, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId, AsyncAssetType};
use warpui_core::elements::{ui_line_height_ratio, Clipped, Hoverable, MouseStateHandle};
use warpui_core::event::DispatchedEvent;
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;
use warpui_core::units::Pixels;
use web_time::Instant;

use super::sidebar_gitdiff_file::render_git_file_diff;
use super::sidebar_gitdiff_multi::render_git_multi_diff;
use super::sidebar_githistory::{fetch_git_history, render_git_panel_view};
use super::sidebar_panels::with_alpha;
use super::view::ProjectSidebarView;
use super::view::{
    is_markdown_path, source_line_changes_for_model, GitHistoryState, GitPanelMode,
    LanguageLocationItem, SidebarAction, META_SIZE, MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT,
    MULTI_FILE_SEARCH_OVERSCAN_ROWS,
};

impl ProjectSidebarView {
    pub fn set_explorer_can_paste(&mut self, can_paste: bool, ctx: &mut ViewContext<Self>) {
        if self.explorer_can_paste != can_paste {
            self.explorer_can_paste = can_paste;
            ctx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: ModelHandle<ProjectSidebarModel>,
        workspace_view_id: EntityId,
        theme: Theme,
        ui_font: FamilyId,
        mono_font: FamilyId,
        git_panel: std::rc::Rc<std::cell::RefCell<super::GitPanelState>>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let diff_comment_editor = ctx.add_typed_action_view(|ec| InputEditor::new(
            "What should the agent do with these lines?", ui_font, crate::theme::text_size(13.0), theme.main_text(),
            theme.disabled_text(), theme.accent, theme,
            InputMode::MultiLine { key: "diff_agent_comment".into(), max_height: 88 }, ec.focus_handle()));
        let diff_comment_search = ctx.add_typed_action_view(|ec| InputEditor::new(
            "Find an agent or session", ui_font, crate::theme::text_size(12.0), theme.main_text(),
            theme.disabled_text(), theme.accent, theme,
            InputMode::Plain { key: "diff_agent_search".into() }, ec.focus_handle()));
        ctx.subscribe_to_view(&diff_comment_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                let Some(comment) = &mut view.diff_comment else { return };
                // Only the Send button depends on the text.
                let was_empty = comment.text.trim().is_empty();
                comment.text = text.clone();
                if was_empty != comment.text.trim().is_empty() {
                    Self::diff_comment_changed(ctx);
                }
            }
        });
        ctx.subscribe_to_view(&diff_comment_search, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if let Some(comment) = &mut view.diff_comment { comment.search = text.clone(); comment.menu_active = 0; }
                Self::diff_comment_changed(ctx);
            }
        });
        // Source-control panel editors: commit message + new-branch name.
        let git_commit_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Commit message…",
                mono_font,
                crate::theme::text_size(11.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "git_commit_msg".to_string(),
                    max_height: 96,
                },
                editor_ctx.focus_handle(),
            )
        });
        let git_filter_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Search files…",
                ui_font,
                crate::theme::text_size(11.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "git_filter".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        // The workspace resolves the selected IDE family once and passes the
        // resulting ID to every editor surface. Resolving again here allowed
        // the initial document editor to disagree with tabs, Markdown, and
        // later live-settings updates.
        let editor_font = mono_font;
        // Source files use the editor's own compact metric, not the terminal
        // preference. Coupling these made code spacing change when users only
        // intended to resize their terminal.
        let editor_font_size = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE);
        let document_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "Select a file to edit",
                editor_font,
                editor_font_size,
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "project_document_editor".to_string(),
                    max_height: MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT as u32,
                },
                editor_ctx.focus_handle(),
            )
        });
        ctx.update_view(&document_editor, |editor, editor_ctx| {
            editor.set_menu_font(ui_font, editor_ctx);
        });
        let buffer_search_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "Find in file",
                editor_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "project_buffer_search".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let language_rename_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "New symbol name",
                editor_font,
                crate::theme::text_size(13.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::LanguageRename { cursor: 0 },
                editor_ctx.focus_handle(),
            )
        });
        let buffer_replace_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "Replace in file",
                editor_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "project_buffer_replace".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        // Global project search editors
        let global_search_query_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "Search across project…",
                editor_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "global_search_query".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let global_search_replace_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "Replace",
                editor_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "global_search_replace".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let global_search_include_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "files to include",
                editor_font,
                crate::theme::text_size(11.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "global_search_include".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let global_search_exclude_editor = ctx.add_typed_action_view(move |editor_ctx| {
            InputEditor::new(
                "files to exclude",
                editor_font,
                crate::theme::text_size(11.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "global_search_exclude".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let model_for_editor = model.clone();
        ctx.subscribe_to_view(&document_editor, move |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, cursor } = event {
                let model_started = std::time::Instant::now();
                let (changed, line_changes) =
                    ctx.update_model(&model_for_editor, |model, model_ctx| {
                        let changed = model.replace_document_text(text.clone(), model_ctx);
                        (changed, source_line_changes_for_model(model))
                    });
                let model_elapsed = model_started.elapsed();
                if model_elapsed > std::time::Duration::from_millis(50) {
                    log::warn!(
                        "[editor-perf] slow document sync: {:?} bytes={}",
                        model_elapsed,
                        text.len()
                    );
                }
                if changed {
                    ctx.dispatch_typed_action_deferred(
                        WorkspaceAction::RequestLanguageCompletion { cursor: *cursor },
                    );
                }
                ctx.update_view(&view.document_editor, |editor, editor_ctx| {
                    editor.set_source_line_changes(line_changes, editor_ctx);
                });
                if view.buffer_search_open {
                    view.refresh_buffer_search_for_text(text, false, ctx);
                }
                view.schedule_autosave(ctx);
            } else if let InputEditorEvent::DiffCommentSelection { start, end, start_deleted, end_deleted } = event {
                let file = ctx.read_model(&view.model, |model, _| model.git_file_diff.as_ref().map(|file| (file.relative_path.clone(), file.target)));
                if let Some((path, target)) = file {
                    view.begin_diff_comment(path, target, *start, *end,
                        *start_deleted, *end_deleted, ctx);
                }
            } else if matches!(event, InputEditorEvent::GitDiffProjectionRowsChanged { .. }) {
                // The child projection and the aggregate card must resize in
                // the same frame so the outer scrollbar remains authoritative.
                ctx.notify();
            } else if matches!(event, InputEditorEvent::AcceptGhost) {
                ctx.update_view(&view.document_editor, |editor, editor_ctx| {
                    if let Some(suffix) = editor.ghost_suffix().map(str::to_owned) {
                        editor.set_ghost_suffix(None, editor_ctx);
                        editor.insert_text_at_cursor(&suffix, editor_ctx);
                    }
                });
            } else if let InputEditorEvent::ContextMenuCommand { command, cursor } = event {
                let action = match command {
                    EditorContextMenuCommand::GoToDefinition
                    | EditorContextMenuCommand::GoToSourceDefinition => {
                        Some(WorkspaceAction::RequestLanguageNavigation {
                            cursor: *cursor,
                            kind: LanguageNavigationKind::Definition,
                        })
                    }
                    EditorContextMenuCommand::GoToTypeDefinition => {
                        Some(WorkspaceAction::RequestLanguageNavigation {
                            cursor: *cursor,
                            kind: LanguageNavigationKind::TypeDefinition,
                        })
                    }
                    EditorContextMenuCommand::GoToImplementation => {
                        Some(WorkspaceAction::RequestLanguageNavigation {
                            cursor: *cursor,
                            kind: LanguageNavigationKind::Implementation,
                        })
                    }
                    EditorContextMenuCommand::GoToReferences
                    | EditorContextMenuCommand::FindAllReferences => {
                        Some(WorkspaceAction::RequestLanguageNavigation {
                            cursor: *cursor,
                            kind: LanguageNavigationKind::References,
                        })
                    }
                    EditorContextMenuCommand::RenameSymbol => {
                        Some(WorkspaceAction::StartLanguageRename { cursor: *cursor })
                    }
                    EditorContextMenuCommand::FormatDocument => {
                        Some(WorkspaceAction::RequestLanguageFormatting)
                    }
                    EditorContextMenuCommand::ShowCodeActions => {
                        Some(WorkspaceAction::RequestLanguageCodeActions { cursor: *cursor })
                    }
                    EditorContextMenuCommand::ToggleMarkdownPreview => {
                        Some(WorkspaceAction::ToggleMarkdownPreview)
                    }
                    EditorContextMenuCommand::ToggleDocumentTerminal => {
                        Some(WorkspaceAction::ToggleDocumentTerminal)
                    }
                    EditorContextMenuCommand::CommandPalette => {
                        Some(WorkspaceAction::OpenCommandPalette)
                    }
                    EditorContextMenuCommand::AddSymbolToChat
                    | EditorContextMenuCommand::AddSymbolToNewChat => {
                        let new_chat = matches!(
                            command,
                            EditorContextMenuCommand::AddSymbolToNewChat
                        );
                        // Resolve file + 1-based line range + symbol text from
                        // the document editor at the menu cursor. Selection
                        // wins; else the word under the cursor; else the
                        // line itself.
                        let resolved =
                            view.document_editor.read(ctx, |editor, _| {
                                let path = editor.source_path.clone()?;
                                let buffer = editor.get_text().to_string();
                                let chars: Vec<char> = buffer.chars().collect();
                                let len = chars.len();
                                let cursor = (*cursor).min(len);
                                let (symbol, start, end) =
                                    match editor.active_selection_range() {
                                        Some((s, e)) if s != e => {
                                            let s = s.min(len);
                                            let e = e.min(len).max(s);
                                            let text: String =
                                                chars[s..e].iter().collect();
                                            (text, s, e)
                                        }
                                        _ => {
                                            let (s, e) = editor.word_range_at(cursor);
                                            if s != e {
                                                let text: String =
                                                    chars[s..e].iter().collect();
                                                (text, s, e)
                                            } else {
                                                (String::new(), cursor, cursor)
                                            }
                                        }
                                    };
                                let line = crate::workspace::input_editor::text::source_line_number(
                                    &chars, start,
                                )
                                as u32;
                                let end_line =
                                    crate::workspace::input_editor::text::source_line_number(
                                        &chars, end.max(start),
                                    ) as u32;
                                let symbol_name = symbol
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                let symbol_name = symbol_name.chars().take(80).collect::<String>();
                                Some((path, line.max(1), end_line.max(line.max(1)), symbol_name))
                            });
                        resolved.map(|(path, line, end_line, symbol_name)| {
                            if new_chat {
                                WorkspaceAction::AttachEditorSymbolToNewAgent {
                                    path,
                                    line,
                                    end_line,
                                    symbol_name,
                                }
                            } else {
                                WorkspaceAction::AttachEditorSymbolToAgent {
                                    path,
                                    line,
                                    end_line,
                                    symbol_name,
                                }
                            }
                        })
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    ctx.dispatch_typed_action_deferred(action);
                }
            } else if let InputEditorEvent::DefinitionHoverRequested {
                request_id,
                cursor,
                symbol_range,
                string_link,
            } = event
            {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::RequestDefinitionHover {
                    request_id: *request_id,
                    cursor: *cursor,
                    symbol_range: symbol_range.clone(),
                    string_link: string_link.clone(),
                });
            } else if let InputEditorEvent::DefinitionHoverClicked { request_id, cursor } = event {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenDefinitionHover {
                    request_id: *request_id,
                    cursor: *cursor,
                });
            }
        });
        ctx.subscribe_to_view(&git_commit_editor, |_, _, event, ctx| {
            if let InputEditorEvent::Changed { .. } = event {
                // The buttons capture both eligibility and the message when
                // this sidebar renders. Invalidating only the child editor
                // leaves those closures stale, especially with a browser
                // active and no terminal output to trigger another render.
                ctx.notify();
            }
        });
        let git_filter_editor = git_filter_editor.clone();
        ctx.subscribe_to_view(&git_filter_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.git_filter_text = text.clone();
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&buffer_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { .. } = event {
                // Anchor the active match to the caret instead of jumping back
                // to the first match, so next/previous navigation continues
                // from where the user is instead of restarting at the top.
                view.refresh_buffer_search_at_caret(ctx);
            }
        });
        // Query and path filters share one debounced, generation-checked search
        // pipeline. A slow older request can therefore never replace newer
        // results after the user has continued typing.
        ctx.subscribe_to_view(&global_search_query_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { .. } = event {
                view.schedule_global_search(ctx);
            }
        });
        ctx.subscribe_to_view(&global_search_include_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { .. } = event {
                view.schedule_global_search(ctx);
            }
        });
        ctx.subscribe_to_view(&global_search_exclude_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { .. } = event {
                view.schedule_global_search(ctx);
            }
        });
        ctx.subscribe_to_model(&model, |view, _, event, ctx| {
            let root = view.model.read(ctx, |m, _| m.git_review_root().unwrap_or(m.workspace_root()).to_path_buf());
            if view.diff_comment.as_ref().is_some_and(|c| c.root != root) { view.diff_comment = None; view.diff_comment_drag = None; }
            if matches!(
                event,
                SidebarEvent::Changed | SidebarEvent::DocumentPresentationChanged
            ) {
                let path = view.model.read(ctx, |model, _| {
                    model
                        .buffer
                        .as_ref()
                        .map(|buffer| buffer.document().relative_path.clone())
                });
                let capabilities = view.model.read(ctx, |model, _| {
                    model
                        .buffer
                        .as_ref()
                        .map(|buffer| buffer.capabilities())
                        .unwrap_or_default()
                });
                let line_changes = view
                    .model
                    .read(ctx, |model, _| source_line_changes_for_model(model));
                let diagnostic_lines = view
                    .model
                    .read(ctx, |model, _| model.active_diagnostic_lines());
                let diagnostic_ranges = view
                    .model
                    .read(ctx, |model, _| model.active_diagnostic_ranges());
                let (git_projection, git_projection_is_current) =
                    view.model.read(ctx, |model, _| {
                        let projection = model.git_file_diff.as_ref().and_then(|file| {
                            (!file.binary).then(|| {
                                (
                                    file.diff.clone(),
                                    file.old_text.lines().count(),
                                    file.new_text.lines().count(),
                                    file.target
                                        == crate::git_status::diff_head::GitDiffTarget::Staged,
                                )
                            })
                        });
                        (projection, !model.document_diff.truncated)
                    });
                let multi_diff_active = view
                    .model
                    .read(ctx, |model, _| model.git_multi_diff.is_some());
                let external_horizontal_scroll =
                    multi_diff_active.then(|| view.git_multi_horizontal_scroll.clone());
                let document_key = path.as_ref().map(|path| {
                    view.model
                        .read(ctx, |model, _| model.workspace_root().join(path))
                });
                let path_changed = document_key != view.editor_document_key;
                if path_changed {
                    if let Some(key) = &view.editor_document_key {
                        let state = view
                            .document_editor
                            .read(ctx, |editor, _| editor.capture_view_state());
                        view.editor_view_states.insert(key.clone(), state);
                    }
                }
                let editor_has_projection = view
                    .document_editor
                    .read(ctx, |editor, _| editor.has_git_diff_projection());
                let refresh_projection = path_changed
                    || matches!(event, SidebarEvent::DocumentPresentationChanged)
                    || (git_projection.is_some() && git_projection_is_current)
                    || editor_has_projection != git_projection.is_some();
                ctx.update_view(&view.document_editor, |editor, editor_ctx| {
                    editor.set_external_horizontal_scroll_state(
                        external_horizontal_scroll,
                        editor_ctx,
                    );
                    editor.set_document_capabilities(capabilities, editor_ctx);
                    editor.set_source_line_changes(line_changes, editor_ctx);
                    editor.set_source_diagnostic_lines(diagnostic_lines, editor_ctx);
                    editor.set_source_diagnostics(diagnostic_ranges, editor_ctx);
                    if refresh_projection {
                        editor.set_git_diff_projection(git_projection, editor_ctx);
                    }
                });
                let text = (path_changed || view.pending_git_edit_line.is_some())
                    .then(|| {
                        view.model.read(ctx, |model, _| {
                            model
                                .buffer
                                .as_ref()
                                .map(|buffer| buffer.text().to_string())
                        })
                    })
                    .flatten();
                let pending_cursor = view
                    .pending_git_edit_line
                    .as_ref()
                    .filter(|(pending_path, _)| path.as_ref() == Some(pending_path))
                    .and_then(|(_, line)| {
                        text.as_ref().map(|text| {
                            text.lines()
                                .take(line.saturating_sub(1))
                                .map(|source| source.chars().count() + 1)
                                .sum::<usize>()
                        })
                    });
                if path_changed {
                    view.markdown_preview_open = false;
                    // References/code-action results belong to the document
                    // they were asked for — drop them when switching files so
                    // the peek panel can never linger over unrelated content.
                    view.clear_language_results(ctx);
                    view.editor_document_path = path.clone();
                    view.editor_document_key = document_key.clone();
                    if view.buffer_search_open {
                        view.refresh_buffer_search(true, ctx);
                    }
                }
                if path_changed || pending_cursor.is_some() {
                    ctx.update_view(&view.document_editor, |editor, editor_ctx| {
                        if path_changed {
                            editor.set_document_path(path.as_deref(), editor_ctx);
                            editor.set_text(text.as_deref().unwrap_or(""), editor_ctx);
                            if let Some(state) = document_key
                                .as_ref()
                                .and_then(|key| view.editor_view_states.get(key))
                            {
                                editor.restore_view_state(state, editor_ctx);
                            }
                        }
                        if let Some(cursor) = pending_cursor {
                            editor.set_cursor_char_index(cursor, editor_ctx);
                            // The aggregate diff owns vertical positioning.
                            // Revealing inside the child would move the page
                            // away from the row that initiated activation.
                            if !multi_diff_active {
                                editor.reveal_cursor(editor_ctx);
                            }
                            editor_ctx.focus_self();
                        }
                    });
                }
                if pending_cursor.is_some() {
                    view.pending_git_edit_line = None;
                    if let Some(anchor) = view.pending_git_edit_scroll_anchor.take() {
                        view.git_diff_scroll.scroll_to(Pixels::new(anchor));
                    }
                }
                ctx.notify();
            }
        });

        Self {
            model,
            workspace_view_id,
            diff_comment: None,
            diff_comment_drag: None,
            diff_comment_editor,
            diff_comment_search,
            diff_comment_mouse: Default::default(),
            theme,
            ui_font,
            mono_font,
            scroll: ClippedScrollStateHandle::new(),
            bottom_panel_scroll: ClippedScrollStateHandle::new(),
            markdown_preview_scroll: ClippedScrollStateHandle::new(),
            git_diff_scroll: ClippedScrollStateHandle::new(),
            git_multi_horizontal_scroll: ClippedScrollStateHandle::new(),
            git_diff_side_by_side: crate::settings::git_consumers::diff_side_by_side(),
            git_diff_show_full_file: false,
            git_diff_expanded_regions: std::collections::HashSet::new(),
            git_diff_active_hunk: 0,
            git_multi_collapsed: std::collections::HashSet::new(),
            git_multi_hunks_collapsed: std::collections::HashSet::new(),
            pending_git_edit_line: None,
            pending_git_edit_scroll_anchor: None,
            language_locations_scroll: ClippedScrollStateHandle::new(),
            language_locations_panel_state: warpui::elements::resizable_state_handle(260.0),
            // The initial 30% size is resolved against the actual sidebar
            // height by Resizable; this value is only a safe pre-layout seed.
            error_panel_state: warpui::elements::resizable_state_handle(100.0),
            quick_action_mouse_states: RefCell::new(Default::default()),
            error_mouse_states: RefCell::new(Default::default()),
            document_mouse_states: RefCell::new(Default::default()),
            document_action_mouse_states: RefCell::new(Default::default()),
            explorer_menu_mouse_states: RefCell::new(Default::default()),
            explorer_context_menu: None,
            explorer_can_paste: false,
            document_tab_drag_states: RefCell::new(Default::default()),
            explorer_file_drag_states: RefCell::new(Default::default()),
            document_editor,
            language_rename_editor,
            sidebar_tab: super::SidebarTab::default(),
            git_commit_editor,
            git_filter_editor,
            git_filter_text: String::new(),
            git_filter_open: false,
            git_changes_scroll: ClippedScrollStateHandle::new(),
            git_history_scroll: ClippedScrollStateHandle::new(),
            git_panel_mode: GitPanelMode::Changes,
            git_view_options_open: false,
            git_show_diff_stats: true,
            git_staged_collapsed: false,
            git_changes_collapsed: false,
            git_history: GitHistoryState::Loading,
            review_pr: super::ReviewPrState::default(),
            review_checks_open: true,
            review_comments_open: true,
            review_resolved_open: false,
            review_action_error: None,
            review_copy_all_copied: false,
            review_copied_comment: None,
            review_resolving_thread: None,
            git_commit_generating: false,
            git_commit_generation_error: None,
            git_panel,
            // Sidebar-local actions (Changes/History and view options) must
            // target this view, not the enclosing WorkspaceView.
            view_id: ctx.view_id(),
            sidebar_tab_mouse_states: RefCell::new(Default::default()),
            git_panel_mouse_states: RefCell::new(Default::default()),
            buffer_search_editor,
            buffer_replace_editor,
            buffer_search_open: false,
            buffer_replace_open: false,
            buffer_search_options: BufferSearchOptions::default(),
            buffer_search_matches: Vec::new(),
            buffer_search_active: None,
            buffer_search_error: None,
            // Global project search (VS Code style)
            global_search_query_editor,
            global_search_replace_editor,
            global_search_include_editor,
            global_search_exclude_editor,
            global_search_results: None,
            global_search_in_progress: false,
            global_search_replacing: false,
            global_search_scroll: ClippedScrollStateHandle::new(),
            global_search_editor_scroll: ClippedScrollStateHandle::new(),
            global_search_editor_horizontal_scroll: ClippedScrollStateHandle::new(),
            global_search_collapsed: std::collections::HashSet::new(),
            global_search_error: None,
            global_search_epoch: 0,
            global_search_generation: Arc::new(AtomicU64::new(0)),
            global_search_context_root: None,
            global_search_results_root: None,
            global_search_active: None,
            global_search_options: crate::workspace::search_panel::SearchOptions::default(),
            global_search_replace_open: false,
            global_search_details_open: false,
            editor_document_path: None,
            editor_document_key: None,
            editor_view_states: HashMap::new(),
            markdown_preview_open: false,
            language_rename_open: false,
            language_locations_title: None,
            language_locations_root: None,
            language_locations: Vec::new(),
            language_code_actions: Vec::new(),
            autosave_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_language_locations(
        &mut self,
        root: PathBuf,
        title: impl Into<String>,
        locations: Vec<LanguageLocationItem>,
        ctx: &mut ViewContext<Self>,
    ) {
        let title = title.into();
        log::info!(
            "[references] peek panel set title={title} items={} with_previews={}",
            locations.len(),
            locations
                .iter()
                .filter(|item| item.line_text.is_some())
                .count(),
        );
        self.language_locations_title = Some(title);
        self.language_locations_root = Some(root);
        self.language_locations = locations;
        self.language_code_actions.clear();
        ctx.notify();
    }

    pub fn set_language_code_actions(
        &mut self,
        actions: Vec<(String, serde_json::Value)>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.language_locations_title = None;
        self.language_locations_root = None;
        self.language_code_actions = actions;
        self.language_locations.clear();
        ctx.notify();
    }

    pub fn clear_language_results(&mut self, ctx: &mut ViewContext<Self>) {
        self.language_locations_title = None;
        self.language_locations_root = None;
        self.language_locations.clear();
        self.language_code_actions.clear();
        ctx.notify();
    }

    pub fn language_results_open(&self) -> bool {
        !self.language_locations.is_empty() || !self.language_code_actions.is_empty()
    }

    pub fn start_language_rename(&mut self, cursor: usize, ctx: &mut ViewContext<Self>) {
        let Some((text, _)) = self.document_text_and_cursor(ctx) else {
            return;
        };
        let chars: Vec<char> = text.chars().collect();
        let cursor = cursor.min(chars.len());
        let start = chars[..cursor]
            .iter()
            .rposition(|ch| !ch.is_alphanumeric() && *ch != '_')
            .map_or(0, |index| index + 1);
        let end = chars[cursor..]
            .iter()
            .position(|ch| !ch.is_alphanumeric() && *ch != '_')
            .map_or(chars.len(), |index| cursor + index);
        if start == end {
            return;
        }
        let name: String = chars[start..end].iter().collect();
        ctx.update_view(&self.language_rename_editor, |editor, editor_ctx| {
            editor.set_text(&name, editor_ctx);
            editor.set_mode(InputMode::LanguageRename { cursor });
            // Match VS Code: the current name starts fully selected, so typing
            // replaces it and the selection makes the text readable over the
            // document.
            editor.select_all(editor_ctx);
            editor_ctx.focus_self();
        });
        // Anchor the VS Code-style rename input directly under the symbol in
        // the document editor instead of showing a detached dialog.
        let rename_editor = self.language_rename_editor.clone();
        let anchor_char = start;
        let end_char = end;
        ctx.update_view(&self.document_editor, move |editor, editor_ctx| {
            editor.begin_inline_rename(
                InlineRenameState {
                    editor: rename_editor,
                    anchor_char,
                    end_char,
                },
                editor_ctx,
            );
        });
        self.language_rename_open = true;
        ctx.notify();
    }

    pub fn take_language_rename(&mut self, ctx: &mut ViewContext<Self>) -> Option<(usize, String)> {
        if !self.language_rename_open {
            return None;
        }
        let result = self.language_rename_editor.read(ctx, |editor, _| {
            let cursor = match editor.mode() {
                InputMode::LanguageRename { cursor } => *cursor,
                _ => 0,
            };
            (cursor, editor.get_text().trim().to_string())
        });
        self.close_language_rename(ctx);
        Some(result)
    }

    pub fn cancel_language_rename(&mut self, ctx: &mut ViewContext<Self>) {
        self.close_language_rename(ctx);
    }

    pub(crate) fn close_language_rename(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.language_rename_open {
            return;
        }
        self.language_rename_open = false;
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.end_inline_rename(editor_ctx);
            editor_ctx.focus_self();
        });
        ctx.notify();
    }

    pub fn toggle_markdown_preview(&mut self, ctx: &mut ViewContext<Self>) {
        let is_markdown = self.model.read(ctx, |model, _| {
            model
                .preview
                .as_ref()
                .is_some_and(|preview| is_markdown_path(&preview.relative_path))
        });
        if is_markdown {
            self.markdown_preview_open = !self.markdown_preview_open;
            if self.markdown_preview_open && self.buffer_search_open {
                self.close_buffer_search(ctx);
            }
            self.markdown_preview_scroll.scroll_to(Pixels::zero());
            if !self.markdown_preview_open {
                // Returning from the full-document preview immediately puts
                // the caret back in the editable Markdown surface.
                ctx.update_view(&self.document_editor, |_editor, editor_ctx| {
                    editor_ctx.focus_self();
                });
            }
            ctx.notify();
        }
    }

    pub fn markdown_document_open(&self, app: &AppContext) -> bool {
        self.model.read(app, |model, _| {
            model
                .preview
                .as_ref()
                .is_some_and(|preview| is_markdown_path(&preview.relative_path))
        })
    }

    pub fn markdown_preview_open(&self) -> bool {
        self.markdown_preview_open
    }

    pub fn prepare_git_diff(&mut self, ctx: &mut ViewContext<Self>) {
        self.git_diff_active_hunk = 0;
        self.git_diff_show_full_file = false;
        self.git_diff_expanded_regions.clear();
        self.git_diff_scroll.scroll_to(Pixels::zero());
        ctx.notify();
    }

    pub fn set_sidebar_tab(&mut self, tab: super::SidebarTab, ctx: &mut ViewContext<Self>) {
        if self.sidebar_tab != tab {
            self.sidebar_tab = tab;
            ctx.notify();
        }
    }

    /// Apply completion of a Git operation to panel-owned UI immediately.
    /// Repository contents arrive through the model's subsequent Updated
    /// event; no timer or polling loop is needed.
    pub fn git_operation_completed(&mut self, succeeded: bool, ctx: &mut ViewContext<Self>) {
        if succeeded {
            let editor = self.git_commit_editor.clone();
            ctx.update_view(&editor, |editor, editor_ctx| {
                editor.set_text("", editor_ctx);
            });
            if self.git_panel_mode == GitPanelMode::History
                || matches!(self.git_history, GitHistoryState::Loaded(_))
            {
                self.load_git_history(ctx);
            }
        }
        ctx.notify();
    }

    pub fn begin_git_commit_generation(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.git_commit_generating {
            return false;
        }
        self.git_commit_generating = true;
        self.git_commit_generation_error = None;
        ctx.notify();
        true
    }

    pub fn finish_git_commit_generation(
        &mut self,
        result: Result<String, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.git_commit_generating = false;
        match result {
            Ok(message) => {
                self.git_commit_generation_error = None;
                let editor = self.git_commit_editor.clone();
                ctx.update_view(&editor, move |editor, editor_ctx| {
                    editor.set_text(&message, editor_ctx);
                    editor_ctx.focus_self();
                });
            }
            Err(error) => {
                self.git_commit_generation_error = None;
                ctx.dispatch_typed_action_deferred(WorkspaceAction::ShowErrorNotice { title: "Couldn’t generate commit message".into(), message: error });
            },
        }
        ctx.notify();
    }

    /// Load commit history off the UI thread. Like a standard history tab, the
    /// panel owns a small summary list and opens the expensive commit patch
    /// only when the user selects an entry.
    pub fn load_git_history(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(cwd) = self.git_panel.borrow().cwd.clone() else {
            self.git_history = GitHistoryState::Error("No Git repository found".to_string());
            ctx.notify();
            return;
        };
        let expected_cwd = cwd.clone();
        self.git_history = GitHistoryState::Loading;
        ctx.notify();
        ctx.spawn(
            async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let _ = tx.send(fetch_git_history(&cwd));
                });
                rx.await
                    .map_err(|_| "Git history worker stopped unexpectedly".to_string())?
            },
            move |view, result, ctx| {
                if view.git_panel.borrow().cwd.as_ref() != Some(&expected_cwd) {
                    log::debug!(
                        "[git-history] ignored stale result cwd={}",
                        expected_cwd.display()
                    );
                    return;
                }
                view.git_history = match result {
                    Ok(entries) => GitHistoryState::Loaded(entries),
                    Err(error) => GitHistoryState::Error(error),
                };
                ctx.notify();
            },
        );
    }

    pub fn sidebar_tab(&self) -> super::SidebarTab {
        self.sidebar_tab
    }

    /// Debounced save after document edits.
    /// Called on every document `Changed` event; when the pref is off this is
    /// a no-op. Generation-checked so rapid typing saves once.
    pub(crate) fn schedule_autosave(&mut self, ctx: &mut ViewContext<Self>) {
        let prefs = crate::console::editor_settings::EditorSettings::global();
        if !prefs.editor.auto_save {
            return;
        }
        let delay_ms = prefs.editor.clamped_auto_save_delay_ms();
        let generation = self.autosave_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let autosave_gen = self.autosave_generation.clone();
        ctx.spawn(
            Timer::after(std::time::Duration::from_millis(delay_ms as u64)),
            move |view: &mut Self, _, ctx| {
                if autosave_gen.load(Ordering::SeqCst) != generation {
                    return;
                }
                // Re-check the pref at fire time (user may have toggled off).
                if !crate::console::editor_settings::EditorSettings::global()
                    .editor
                    .auto_save
                {
                    return;
                }
                // Skip when nothing is dirty — avoids pointless writes.
                let dirty = view.model.read(ctx, |m, _| {
                    m.document_snapshot().is_some() && !m.document_diff.is_empty()
                });
                if !dirty {
                    return;
                }
                log::info!("[autosave] saving dirty document after {delay_ms}ms debounce");
                ctx.dispatch_typed_action_deferred(WorkspaceAction::SaveOpenDocument);
            },
        );
    }

    /// Source-control panel (right sidebar, Source Control tab).
    pub(crate) fn render_git_panel(
        &self,
        app: &AppContext,
        theme: &Theme,
        ui: FamilyId,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let state = self.git_panel.borrow();
        render_git_panel_view(
            &state,
            &self.git_commit_editor,
            &self.git_filter_editor,
            &self.git_filter_text,
            self.git_filter_open,
            self.workspace_view_id,
            self.view_id,
            &self.git_panel_mouse_states,
            self.git_changes_scroll.clone(),
            self.git_history_scroll.clone(),
            self.git_panel_mode,
            self.git_view_options_open,
            self.git_show_diff_stats,
            self.git_staged_collapsed,
            self.git_changes_collapsed,
            &self.git_history,
            self.git_commit_generating,
            self.git_commit_generation_error.as_deref(),
            ui,
            mono,
            theme,
            app,
        )
    }
}
