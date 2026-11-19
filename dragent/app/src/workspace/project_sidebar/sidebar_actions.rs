//! Sidebar action dispatch.
//!
//! TypedActionView implementation. Moved verbatim; same behavior.

use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};
use super::sidebar_explorer::toggle_project_search_file;
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

use super::sidebar_gitdiff_file::{git_diff_display, git_diff_hunk_offset};
use super::sidebar_review::review_comment_clipboard_text;
use super::view::{ExplorerContextMenuState, GitPanelMode};
use super::view::{ProjectSidebarView, SidebarAction};

impl TypedActionView for ProjectSidebarView {
    type Action = SidebarAction;

    fn handle_action(&mut self, action: &SidebarAction, ctx: &mut ViewContext<Self>) {
        if self.handle_diff_comment_sidebar_action(action, ctx) { return; }
        match action {
            SidebarAction::DiffCommentStart { .. } | SidebarAction::DiffCommentExtend { .. }
            | SidebarAction::DiffCommentFinish | SidebarAction::DiffCommentCancel
            | SidebarAction::DiffCommentMenu(_) | SidebarAction::DiffCommentPickerMove(_) | SidebarAction::DiffCommentPickerAccept | SidebarAction::DiffCommentPickerDismiss | SidebarAction::DiffCommentClearSearch | SidebarAction::DiffCommentChoose(_)
            | SidebarAction::DiffCommentPlacement(_) | SidebarAction::DiffCommentSend => {},
            SidebarAction::OpenExplorerContextMenu {
                relative_path,
                directory,
                position,
            } => {
                let local_position = ctx
                    .element_position_by_id("project_sidebar_context_root")
                    .map(|bounds| *position - bounds.origin())
                    .unwrap_or(*position);
                self.explorer_context_menu = Some(ExplorerContextMenuState {
                    relative_path: relative_path.clone(),
                    directory: *directory,
                    position: local_position,
                });
                ctx.notify();
            }
            SidebarAction::CloseExplorerContextMenu => {
                self.explorer_context_menu = None;
                ctx.notify();
            }
            SidebarAction::ToggleGitViewOptions => {
                self.git_view_options_open = !self.git_view_options_open;
                ctx.notify();
            }
            SidebarAction::ToggleGitDiffStats => {
                self.git_show_diff_stats = !self.git_show_diff_stats;
                ctx.notify();
            }
            SidebarAction::ToggleGitFilter => {
                self.git_filter_open = !self.git_filter_open;
                if !self.git_filter_open {
                    self.git_filter_text.clear();
                    ctx.update_view(&self.git_filter_editor, |editor, editor_ctx| {
                        editor.set_text("", editor_ctx);
                    });
                }
                ctx.notify();
            }
            SidebarAction::ClearGitFilter => {
                self.git_filter_text.clear();
                ctx.update_view(&self.git_filter_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                });
                ctx.notify();
            }
            SidebarAction::ToggleReviewChecks => {
                self.review_checks_open = !self.review_checks_open;
                ctx.notify();
            }
            SidebarAction::ToggleReviewComments => {
                self.review_comments_open = !self.review_comments_open;
                ctx.notify();
            }
            SidebarAction::ToggleReviewResolved => {
                self.review_resolved_open = !self.review_resolved_open;
                ctx.notify();
            }
            SidebarAction::CopyReviewComment { id } => {
                let text = self
                    .review_pr
                    .comments
                    .iter()
                    .find(|comment| comment.id == *id)
                    .map(review_comment_clipboard_text)
                    .unwrap_or_default();
                if !text.is_empty() {
                    ctx.clipboard()
                        .write(warpui_core::clipboard::ClipboardContent::plain_text(text));
                    // Checkmark feedback for ~1.5s, then back to the copy icon.
                    let copied_id = id.clone();
                    self.review_copied_comment = Some(copied_id.clone());
                    ctx.notify();
                    ctx.spawn(
                        Timer::after(std::time::Duration::from_millis(1500)),
                        move |view, _, ctx| {
                            if view.review_copied_comment.as_deref() == Some(&copied_id) {
                                view.review_copied_comment = None;
                            }
                            ctx.notify();
                        },
                    );
                    return;
                }
                ctx.notify();
            }
            SidebarAction::CopyAllReviewComments => {
                let active: Vec<&super::ReviewComment> = self
                    .review_pr
                    .comments
                    .iter()
                    .filter(|comment| !comment.is_resolved)
                    .collect();
                if !active.is_empty() {
                    let text = active
                        .into_iter()
                        .map(review_comment_clipboard_text)
                        .collect::<Vec<_>>()
                        .join("\n\n---\n\n");
                    ctx.clipboard()
                        .write(warpui_core::clipboard::ClipboardContent::plain_text(text));
                    // Checkmark feedback for ~1.5s, then back to the copy icon.
                    self.review_copy_all_copied = true;
                    ctx.notify();
                    ctx.spawn(
                        Timer::after(std::time::Duration::from_millis(1500)),
                        move |view, _, ctx| {
                            view.review_copy_all_copied = false;
                            ctx.notify();
                        },
                    );
                    return;
                }
                ctx.notify();
            }
            SidebarAction::ResolveReviewThread { thread_id, resolve } => {
                let thread_id = thread_id.clone();
                let resolve = *resolve;
                // Pending label on the button until the mutation settles.
                self.review_resolving_thread = Some(thread_id.clone());
                self.review_action_error = None;
                ctx.notify();
                ctx.spawn(
                    async move {
                        // Resolving and unresolving are distinct mutations:
                        // calling resolve on an already-resolved thread is a
                        // silent no-op, which is why Undo appeared to do nothing.
                        let mutation = if resolve {
                            "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
                        } else {
                            "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
                        };
                        let output = crate::process_ext::gh_command()
                            .args([
                                "api",
                                "graphql",
                                "-f",
                                &format!("query={mutation}"),
                                "-f",
                                &format!("id={thread_id}"),
                            ])
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .env("GH_PAGER", "")
                            .no_window()
                            .output()
                            .map_err(|error| format!("Could not start gh: {error}"))?;
                        if output.status.success() {
                            Ok(())
                        } else {
                            Err(String::from_utf8_lossy(&output.stderr)
                                .trim()
                                .chars()
                                .take(140)
                                .collect())
                        }
                    },
                    |view, result: Result<(), String>, ctx| {
                        match result {
                            Ok(()) => {
                                view.review_action_error = None;
                                ctx.dispatch_typed_action_deferred(
                                    WorkspaceAction::RefreshReviewPr,
                                );
                            }
                            Err(error) => {
                                view.review_action_error = None;
                                ctx.dispatch_typed_action_deferred(WorkspaceAction::ShowErrorNotice { title: "Review operation failed".into(), message: error });
                            }
                        }
                        view.review_resolving_thread = None;
                        ctx.notify();
                    },
                );
            }
            SidebarAction::ToggleGitGroup { staged } => {
                if *staged {
                    self.git_staged_collapsed = !self.git_staged_collapsed;
                } else {
                    self.git_changes_collapsed = !self.git_changes_collapsed;
                }
                ctx.notify();
            }
            SidebarAction::SetGitPanelMode(mode) => {
                if self.git_panel_mode != *mode {
                    self.git_panel_mode = *mode;
                    self.git_view_options_open = false;
                    if *mode == GitPanelMode::History {
                        self.load_git_history(ctx);
                    }
                    ctx.notify();
                }
            }
            SidebarAction::ToggleGitDiffLayout => {
                self.git_diff_side_by_side = !self.git_diff_side_by_side;
                // Persist the layout choice to Version Control settings.
                let mut global = crate::settings::AppSettings::load();
                global.git.diff_view = if self.git_diff_side_by_side {
                    crate::settings::DiffView::SideBySide
                } else {
                    crate::settings::DiffView::Inline
                };
                global.save();
                self.git_diff_active_hunk = 0;
                self.git_diff_scroll.scroll_to(Pixels::zero());
                ctx.notify();
            }
            SidebarAction::ToggleGitDiffFullFile => {
                self.git_diff_show_full_file = !self.git_diff_show_full_file;
                self.git_diff_expanded_regions.clear();
                self.git_diff_active_hunk = 0;
                self.git_diff_scroll.scroll_to(Pixels::zero());
                ctx.notify();
            }
            SidebarAction::ExpandGitDiffRegion(region_id) => {
                self.git_diff_expanded_regions.insert(*region_id);
                ctx.notify();
            }
            SidebarAction::ActivateGitMultiFile { path, line, target } => {
                self.git_multi_collapsed.remove(&(path.clone(), *target));
                let already_editing = *target
                    == crate::git_status::diff_head::GitDiffTarget::Unstaged
                    && self.model.read(ctx, |model, _| {
                        model.git_multi_diff.is_some()
                            && model.git_file_diff.as_ref().is_some_and(|active| {
                                active.relative_path == *path && active.target == *target
                            })
                            && model.buffer.is_some()
                    });
                if already_editing {
                    ctx.update_view(&self.document_editor, |_editor, editor_ctx| {
                        editor_ctx.focus_self();
                    });
                    ctx.notify();
                    return;
                }
                if self
                    .pending_git_edit_line
                    .as_ref()
                    .is_some_and(|(pending_path, _)| pending_path == path)
                {
                    return;
                }
                self.pending_git_edit_scroll_anchor =
                    Some(self.git_diff_scroll.scroll_start().as_f32().max(0.0));
                self.pending_git_edit_line = Some((path.clone(), *line));
                if *target == crate::git_status::diff_head::GitDiffTarget::Unstaged {
                    let activated = ctx.update_model(&self.model, |model, model_ctx| {
                        model.activate_git_multi_file(path, *target, model_ctx)
                    });
                    if !activated {
                        self.pending_git_edit_line = None;
                        self.pending_git_edit_scroll_anchor = None;
                    }
                } else {
                    self.pending_git_edit_line = None;
                    self.pending_git_edit_scroll_anchor = None;
                    if let Some(root) = self.git_panel.borrow().cwd.clone() {
                        ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenReadOnlyDocument {
                            // Legacy action name: this opens the real editable
                            // worktree file, never the immutable index snapshot.
                            root,
                            relative_path: path.clone(),
                        });
                    }
                }
            }
            SidebarAction::NavigateGitDiffHunk(direction) => {
                let hunk_count = self.model.read(ctx, |model, _| {
                    model.git_file_diff.as_ref().map_or(0, |file| {
                        git_diff_display(file, self.git_diff_show_full_file)
                            .hunks
                            .len()
                    })
                });
                if hunk_count > 0 {
                    self.git_diff_active_hunk = if *direction < 0 {
                        self.git_diff_active_hunk
                            .checked_sub(1)
                            .unwrap_or(hunk_count - 1)
                    } else {
                        (self.git_diff_active_hunk + 1) % hunk_count
                    };
                    let offset = self.model.read(ctx, |model, _| {
                        model.git_file_diff.as_ref().map_or(0.0, |file| {
                            git_diff_hunk_offset(
                                file,
                                self.git_diff_active_hunk,
                                self.git_diff_side_by_side,
                                self.git_diff_show_full_file,
                            )
                        })
                    });
                    self.git_diff_scroll.scroll_to(Pixels::new(offset));
                    ctx.notify();
                }
            }
            SidebarAction::ToggleGitMultiFile { path, target } => {
                let key = (path.clone(), *target);
                if !self.git_multi_collapsed.remove(&key) {
                    self.git_multi_collapsed.insert(key);
                }
                ctx.notify();
            }
            SidebarAction::ToggleGitMultiHunk { path, target, hunk } => {
                let key = (path.clone(), *target, *hunk);
                if !self.git_multi_hunks_collapsed.remove(&key) {
                    self.git_multi_hunks_collapsed.insert(key);
                }
                ctx.notify();
            }
            SidebarAction::ToggleGlobalSearchReplace => {
                self.global_search_replace_open = !self.global_search_replace_open;
                ctx.notify();
            }
            SidebarAction::ToggleGlobalSearchOption { option } => {
                match option.as_str() {
                    "case_sensitive" => {
                        self.global_search_options.case_sensitive =
                            !self.global_search_options.case_sensitive;
                    }
                    "whole_word" => {
                        self.global_search_options.whole_word =
                            !self.global_search_options.whole_word;
                    }
                    "regex" => {
                        self.global_search_options.regex = !self.global_search_options.regex;
                    }
                    _ => {}
                }
                self.schedule_global_search(ctx);
            }
            SidebarAction::ToggleGlobalSearchFileCollapse { path } => {
                toggle_project_search_file(&mut self.global_search_collapsed, path);
                ctx.notify();
            }
            SidebarAction::ActivateGlobalSearchMatch { path, hit } => {
                self.activate_global_search_match(path.clone(), *hit, true, ctx);
            }
            SidebarAction::SelectGlobalSearchMatch { path, hit } => {
                self.activate_global_search_match(path.clone(), *hit, false, ctx);
            }
            SidebarAction::NavigateGlobalSearchMatch { previous } => {
                self.navigate_global_search_match(*previous, ctx);
            }
            SidebarAction::ReplaceAllGlobalSearchMatches => {
                self.replace_all_global_search(ctx);
            }
            SidebarAction::ToggleAllGlobalSearchFiles => {
                let Some(results) = self.global_search_results.as_ref() else {
                    return;
                };
                let all_collapsed = results
                    .files
                    .iter()
                    .all(|file| self.global_search_collapsed.contains(&file.relative_path));
                if all_collapsed {
                    self.global_search_collapsed.clear();
                } else {
                    self.global_search_collapsed = results
                        .files
                        .iter()
                        .map(|file| file.relative_path.clone())
                        .collect();
                }
                ctx.notify();
            }
            SidebarAction::ToggleGlobalSearchDetails => {
                self.global_search_details_open = !self.global_search_details_open;
                ctx.notify();
            }
        }
    }
}
