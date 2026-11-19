//! Sidebar explorer and search-file renders.
//!
//! Moved verbatim; same behavior.

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
    find_buffer_matches, lsp_range_to_char_range, BufferSearchOptions, FileKind, LineChangeKind,
    TextRange,
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

use super::sidebar_gitdiff_file::render_git_diff_source;
use super::sidebar_panels::{section_title_inline, with_alpha};
use super::view::ProjectSidebarView;
use super::view::{
    ExplorerContextMenuState, SidebarAction, FILE_TREE_SIZE, MULTI_FILE_SEARCH_OVERSCAN_ROWS,
};

pub(crate) fn render_documents_section(
    model: &ProjectSidebarModel,
    ui: FamilyId,
    file_font: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let document_root = model
        .active_cwd
        .clone()
        .unwrap_or_else(|| model.workspace_root().to_path_buf());
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    if let Some((parent, directory, editor)) = &model.explorer_creation {
        if parent.as_os_str().is_empty() {
            rows = rows.with_child(render_inline_creation(
                parent, *directory, editor, model.explorer_creation_error.as_deref(), ui, theme, view,
            ));
        }
    }
    for entry in model
        .documents
        .entries()
        .iter()
        .filter(|entry| model.is_document_visible(&entry.document.relative_path))
        .take(400)
    {
        let relative_path = entry.document.relative_path.clone();
        let is_directory = entry.is_directory;
        let is_ignored = entry.is_ignored;
        let has_git_changes = model.git_path_changed(&relative_path, is_directory);
        let selected = model
            .explorer_selection
            .as_ref()
            .is_some_and(|(path, _)| path == &relative_path);
        let expanded = is_directory && model.is_directory_expanded(&relative_path);
        let depth = relative_path.components().count().saturating_sub(1) as f32;
        let label = relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("?")
            .to_string();
        let key = format!(
            "{}:{}",
            if is_directory { "dir" } else { "file" },
            relative_path.display()
        );
        let mouse = view
            .document_mouse_states
            .borrow_mut()
            .entry(key)
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let workspace_view_id = view.workspace_view_id;
        let display_path = relative_path.clone();
        let action = if is_directory {
            WorkspaceAction::ToggleProjectDirectory {
                relative_path: relative_path.clone(),
            }
        } else {
            WorkspaceAction::OpenReadOnlyDocument {
                root: document_root.clone(),
                relative_path: relative_path.clone(),
            }
        };
        let row = Hoverable::new(mouse, move |state| {
                let disclosure = if is_directory {
                    render_icon(
                        if expanded {
                            AppIcon::ChevronDown
                        } else {
                            AppIcon::ChevronRight
                        },
                        theme.sub_text(),
                        10.0,
                    )
                } else {
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(10.0)
                        .finish()
                };
                let git_marker = if has_git_changes {
                    Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(4.0)
                            .with_height(16.0)
                            .finish(),
                    )
                    .with_background_color(theme.ui_yellow())
                    .finish()
                } else {
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(4.0)
                        .with_height(16.0)
                        .finish()
                };
                let row = Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(4.0)
                        .with_child(disclosure)
                        .with_child(if is_directory {
                            render_icon(
                                if expanded {
                                    AppIcon::FolderOpen
                                } else {
                                    AppIcon::Folder
                                },
                                theme.sub_text(),
                                14.0,
                            )
                        } else {
                            render_language_icon(&display_path, theme)
                        })
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(
                                    label.clone(),
                                    file_font,
                                    crate::theme::text_size(FILE_TREE_SIZE),
                                )
                                .with_style(Properties::default().weight(Weight::Normal))
                                .with_color(if has_git_changes {
                                    theme.ui_yellow()
                                } else if is_ignored {
                                    theme.disabled_text()
                                } else {
                                    theme.main_text()
                                })
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(8.0 + depth * 12.0)
                .with_padding_right(8.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if selected {
                    with_alpha(theme.accent, 0.16)
                } else if state.is_hovered() {
                    with_alpha(theme.accent, 0.08)
                } else {
                    theme.panel_background()
                })
                .finish();
                Stack::new()
                    .with_child(row)
                    .with_positioned_child(
                        git_marker,
                        OffsetPositioning::offset_from_parent(
                            vec2f(-2.0, 4.0),
                            ParentOffsetBounds::Unbounded,
                            ParentAnchor::TopRight,
                            ChildAnchor::TopRight,
                        ),
                    )
                    .finish()
            })
            .on_click(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            })
            .on_right_click({
                let relative_path = relative_path.clone();
                let view_id = view.view_id;
                move |ctx, _app, position| {
                    ctx.dispatch_typed_action_for(
                        view_id,
                        SidebarAction::OpenExplorerContextMenu {
                            relative_path: relative_path.clone(),
                            directory: is_directory,
                            position,
                        },
                    );
                }
            })
            .with_cursor(Cursor::PointingHand)
            .finish();
        let row = if is_directory {
            row
        } else {
            let path = document_root.join(&relative_path);
            let drag = view
                .explorer_file_drag_states
                .borrow_mut()
                .entry(path.clone())
                .or_default()
                .clone();
            crate::agent::file_drag::draggable_file(row, drag, path)
        };
        rows = rows.with_child(row);
        if let Some((parent, directory, editor)) = &model.explorer_creation {
            if is_directory && parent == &relative_path {
                rows = rows.with_child(render_inline_creation(
                    parent, *directory, editor, model.explorer_creation_error.as_deref(), ui, theme, view,
                ));
            }
        }
    }
    Container::new(rows.finish())
        .with_padding_bottom(11.0)
        .finish()
}

/// Keep explorer actions outside the file tree's scroll viewport.
pub(crate) fn render_explorer_header(
    model: &ProjectSidebarModel,
    ui: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let parent = model
        .explorer_selection
        .as_ref()
        .map(|(path, directory)| {
            if *directory {
                path.clone()
            } else {
                path.parent().unwrap_or(Path::new("")).to_path_buf()
            }
        })
        .unwrap_or_default();
    let mut actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0);
    for (icon, label, key, action) in [
        (
            AppIcon::FilePlus,
            "New file",
            "explorer_new_file",
            WorkspaceAction::ExplorerStartCreate {
                relative_parent: parent.clone(),
                directory: false,
            },
        ),
        (
            AppIcon::FolderPlus,
            "New folder",
            "explorer_new_folder",
            WorkspaceAction::ExplorerStartCreate {
                relative_parent: parent.clone(),
                directory: true,
            },
        ),
        (
            AppIcon::RefreshCw,
            "Refresh explorer",
            "explorer_refresh",
            WorkspaceAction::ExplorerRefresh,
        ),
    ] {
        actions = actions.with_child(buffer_search_icon_button(
            icon, label, key, action, theme, view,
        ));
    }
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, section_title_inline("Files", ui, theme)).finish())
            .with_child(actions.finish())
            .finish(),
    )
    .with_padding_left(13.0)
    .with_padding_right(8.0)
    .with_padding_top(5.0)
    .with_padding_bottom(5.0)
    .with_background_color(theme.panel_background())
    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
    .finish()
}

fn render_inline_creation(
    parent: &Path,
    directory: bool,
    editor: &ViewHandle<InputEditor>,
    _error: Option<&str>,
    ui: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(render_icon(
            if directory {
                AppIcon::Folder
            } else {
                AppIcon::File
            },
            theme.sub_text(),
            14.0,
        ))
        .with_child(
            Expanded::new(
                1.0,
                Container::new(ChildView::new(editor).finish())
                    .with_padding_left(4.0)
                    .with_padding_right(4.0)
                    .finish(),
            )
            .finish(),
        )
        .with_child(buffer_search_icon_button(
            AppIcon::Check,
            "Create (Enter)",
            "explorer_create_confirm",
            WorkspaceAction::ExplorerCommitPrompt,
            theme,
            view,
        ))
        .with_child(buffer_search_icon_button(
            AppIcon::XClose,
            "Cancel (Escape)",
            "explorer_create_cancel",
            WorkspaceAction::ExplorerCancelPrompt,
            theme,
            view,
        ))
        .finish();
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(row);

    Container::new(content.finish())
        .with_padding_left(22.0 + parent.components().count() as f32 * 12.0)
        .with_padding_right(8.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_background_color(with_alpha(theme.accent, 0.16))
        .finish()
}

pub(crate) fn render_explorer_context_menu(
    target: &ExplorerContextMenuState,
    can_paste: bool,
    root: PathBuf,
    sidebar_view_id: EntityId,
    workspace_view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let path = target.relative_path.clone();
    let parent = if target.directory {
        path.clone()
    } else {
        path.parent().unwrap_or(Path::new("")).to_path_buf()
    };
    let mut menu = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);

    if !target.directory {
        menu = menu.with_child(explorer_menu_item(
            "Open",
            None,
            AppIcon::File,
            false,
            true,
            WorkspaceAction::OpenReadOnlyDocument {
                root,
                relative_path: path.clone(),
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "open",
        ));
    }
    menu = menu
        .with_child(explorer_menu_item(
            "New File…",
            None,
            AppIcon::File,
            false,
            true,
            WorkspaceAction::ExplorerStartCreate {
                relative_parent: parent.clone(),
                directory: false,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "new_file",
        ))
        .with_child(explorer_menu_item(
            "New Folder…",
            None,
            AppIcon::Folder,
            false,
            true,
            WorkspaceAction::ExplorerStartCreate {
                relative_parent: parent.clone(),
                directory: true,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "new_folder",
        ))
        .with_child(explorer_menu_separator(theme))
        .with_child(explorer_menu_item(
            "Cut",
            Some(if cfg!(target_os = "macos") {
                "⌘X"
            } else {
                "Ctrl+X"
            }),
            AppIcon::Replace,
            false,
            true,
            WorkspaceAction::ExplorerCutEntry {
                relative_path: path.clone(),
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "cut",
        ))
        .with_child(explorer_menu_item(
            "Copy",
            Some(if cfg!(target_os = "macos") {
                "⌘C"
            } else {
                "Ctrl+C"
            }),
            AppIcon::Copy,
            false,
            true,
            WorkspaceAction::ExplorerCopyEntry {
                relative_path: path.clone(),
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "copy",
        ))
        .with_child(explorer_menu_item(
            "Paste",
            Some(if cfg!(target_os = "macos") {
                "⌘V"
            } else {
                "Ctrl+V"
            }),
            AppIcon::Copy,
            false,
            can_paste,
            WorkspaceAction::ExplorerPasteInto {
                relative_directory: parent,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "paste",
        ))
        .with_child(explorer_menu_item(
            "Duplicate",
            None,
            AppIcon::Copy,
            false,
            true,
            WorkspaceAction::ExplorerDuplicateEntry {
                relative_path: path.clone(),
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "duplicate",
        ))
        .with_child(explorer_menu_separator(theme))
        .with_child(explorer_menu_item(
            "Copy Path",
            None,
            AppIcon::Copy,
            false,
            true,
            WorkspaceAction::ExplorerCopyPath {
                relative_path: path.clone(),
                absolute: true,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "copy_path",
        ))
        .with_child(explorer_menu_item(
            "Copy Relative Path",
            None,
            AppIcon::Copy,
            false,
            true,
            WorkspaceAction::ExplorerCopyPath {
                relative_path: path.clone(),
                absolute: false,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "copy_relative_path",
        ))
        .with_child(explorer_menu_separator(theme))
        .with_child(explorer_menu_item(
            "Rename…",
            Some("F2"),
            AppIcon::Edit,
            false,
            true,
            WorkspaceAction::ExplorerStartRename {
                relative_path: path.clone(),
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "rename",
        ))
        .with_child(explorer_menu_item(
            "Delete…",
            None,
            AppIcon::XClose,
            true,
            true,
            WorkspaceAction::ExplorerRequestDelete {
                relative_path: path,
                directory: target.directory,
            },
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "delete",
        ))
        .with_child(explorer_menu_separator(theme))
        .with_child(explorer_menu_item(
            "Refresh Explorer",
            None,
            AppIcon::History,
            false,
            true,
            WorkspaceAction::ExplorerRefresh,
            sidebar_view_id,
            workspace_view_id,
            ui,
            theme,
            mouse_states,
            "refresh",
        ));

    ConstrainedBox::new(
        Container::new(
            Container::new(menu.finish())
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .finish(),
        )
        .with_background_color(theme.sidebar_menu_background())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish(),
    )
    .with_width(250.0)
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn explorer_menu_item(
    label: &str,
    shortcut: Option<&str>,
    icon: AppIcon,
    danger: bool,
    enabled: bool,
    action: WorkspaceAction,
    sidebar_view_id: EntityId,
    workspace_view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
) -> Box<dyn Element> {
    let mouse = mouse_states
        .borrow_mut()
        .entry(key.to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let label = label.to_string();
    let shortcut = shortcut.unwrap_or_default().to_string();
    let text_color = if !enabled {
        theme.disabled_text()
    } else if danger {
        theme.ui_red()
    } else {
        theme.main_text()
    };
    let icon_color = if !enabled {
        theme.disabled_text()
    } else if danger {
        theme.ui_red()
    } else {
        theme.sub_text()
    };
    Hoverable::new(mouse, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(render_icon(icon, icon_color, 13.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                            .with_color(text_color)
                            .soft_wrap(false)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Text::new(shortcut.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .soft_wrap(false)
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(if enabled && state.is_hovered() {
            if danger {
                with_alpha(theme.ui_red(), 0.12)
            } else {
                theme.sidebar_menu_hover()
            }
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .on_click(move |event_ctx, _app, _position| {
        if enabled {
            event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            event_ctx.dispatch_typed_action_for(
                sidebar_view_id,
                SidebarAction::CloseExplorerContextMenu,
            );
        }
    })
    .with_cursor(if enabled {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .finish()
}

pub(crate) fn explorer_menu_separator(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.0)
            .finish(),
    )
    .with_margin_top(4.0)
    .with_margin_bottom(4.0)
    .with_margin_left(8.0)
    .with_margin_right(8.0)
    .with_background_color(theme.border())
    .finish()
}

/// Render a compact file marker for navigation results.
///
/// These rows can be created before the bundled file-kind image has been
/// decoded. Using the existing vector icon path here keeps the reference
/// sheet populated immediately instead of leaving an empty image box while
/// the asset cache catches up.
/// The file's real ecosystem icon (Rust gear, React atom, TypeScript "TS",
/// Markdown "M", …) — the same bundled Material Icon Theme SVGs and the same
/// `FileKind` classification already used for editor tabs (`editor_tabs.rs`),
/// so a file's icon matches wherever it's shown. `theme` is unused now that
/// this renders a full-color bundled asset rather than a theme-tinted glyph,
/// kept only so callers don't need to change; a future icon-less extension
/// still falls back to a generic (currently theme-neutral) file glyph via
/// `FileKind`'s own `icons/file.svg` default.
pub(crate) fn render_language_icon(path: &Path, _theme: &Theme) -> Box<dyn Element> {
    let asset = FileKind::from_path(path).icon_asset();
    ConstrainedBox::new(
        Image::new(AssetSource::Bundled { path: asset }, CacheOption::BySize)
            .contain()
            .finish(),
    )
    .with_width(16.0)
    .with_height(16.0)
    .finish()
}

pub(crate) fn document_action_button(
    icon: AppIcon,
    key: &str,
    action: WorkspaceAction,
    color: ColorU,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let mouse = view
        .document_action_mouse_states
        .borrow_mut()
        .entry(key.to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let workspace_view_id = view.workspace_view_id;
    // Borderless icon button: transparent at rest, tinted on hover. Matches
    // the terminal pane header buttons so the breadcrumb bar keeps the
    // shared 34px height (16px icon + 5px padding, no border).
    Hoverable::new(mouse, move |state| {
        Container::new(render_icon(icon, color, 16.0))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_background_color(if state.is_hovered() {
                with_alpha(color, 0.12)
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .finish()
    })
    .on_click(move |ctx, _app, _position| {
        ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn buffer_search_option_button(
    label: &str,
    tooltip: &str,
    key: &str,
    action: WorkspaceAction,
    active: bool,
    theme: &Theme,
    font: FamilyId,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let mouse = view
        .document_action_mouse_states
        .borrow_mut()
        .entry(key.to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let workspace_view_id = view.workspace_view_id;
    let label = label.to_string();
    let tooltip = tooltip.to_string();
    ConstrainedBox::new(
        Hoverable::new(mouse, move |state| {
            let button = Container::new(
                Align::new(
                    Text::new(label.clone(), font, crate::theme::text_size(10.5))
                        .with_color(if active {
                            theme.accent
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_background_color(if active || state.is_hovered() {
                with_alpha(theme.accent, if active { 0.18 } else { 0.09 })
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .finish();
            buffer_search_tooltip(button, state.is_hovered(), &tooltip, theme, font)
        })
        .on_click(move |ctx, _app, _position| {
            ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        })
        .with_cursor(Cursor::PointingHand)
        .finish(),
    )
    .with_width(27.0)
    .with_height(26.0)
    .finish()
}

pub(crate) fn buffer_search_icon_button(
    icon: AppIcon,
    tooltip: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let mouse = view
        .document_action_mouse_states
        .borrow_mut()
        .entry(key.to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let workspace_view_id = view.workspace_view_id;
    let tooltip = tooltip.to_string();
    let tooltip_font = view.ui_font;
    ConstrainedBox::new(
        Hoverable::new(mouse, move |state| {
            let button = Container::new(render_icon(icon, theme.sub_text(), 13.0))
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_background_color(if state.is_hovered() {
                    with_alpha(theme.accent, 0.09)
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .finish();
            buffer_search_tooltip(button, state.is_hovered(), &tooltip, theme, tooltip_font)
        })
        .on_click(move |ctx, _app, _position| {
            ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        })
        .with_cursor(Cursor::PointingHand)
        .finish(),
    )
    .with_width(26.0)
    .with_height(26.0)
    .finish()
}

pub(crate) fn global_search_icon_button(
    icon: AppIcon,
    tooltip: &str,
    action: SidebarAction,
    enabled: bool,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let mouse = MouseStateHandle::default();
    let tooltip = tooltip.to_string();
    let button = ConstrainedBox::new(
        Hoverable::new(mouse, move |state| {
            let color = if enabled {
                theme.sub_text()
            } else {
                theme.disabled_text()
            };
            let button = Container::new(render_icon(icon, color, 12.0))
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_background_color(if enabled && state.is_hovered() {
                    theme.surface_1()
                } else {
                    ColorU::transparent_black()
                })
                .finish();
            buffer_search_tooltip(button, state.is_hovered(), &tooltip, theme, font)
        })
        .with_cursor(if enabled {
            Cursor::PointingHand
        } else {
            Cursor::Arrow
        })
        .finish(),
    )
    .with_width(24.0)
    .with_height(24.0)
    .finish();
    EventHandler::new(button)
        .on_left_mouse_down(move |ctx, _app, _position| {
            if enabled {
                ctx.dispatch_typed_action_for(view_id, action.clone());
            }
            DispatchEventResult::StopPropagation
        })
        .finish()
}

pub(crate) fn project_search_sidebar_sticky_file<'a>(
    results: &'a crate::workspace::search_panel::SearchResults,
    collapsed: &std::collections::HashSet<PathBuf>,
    scroll_top: f32,
) -> Option<&'a crate::workspace::search_panel::SearchFileResult> {
    if scroll_top <= 0.0 {
        return None;
    }
    let mut top = 0.0;
    let mut active = None;
    for file in &results.files {
        if scroll_top < top {
            break;
        }
        active = Some(file);
        top += 28.0;
        if !collapsed.contains(&file.relative_path) {
            top += file.hits.len() as f32 * 24.0;
        }
    }
    active
}

/// Toggle one file without disturbing the expansion state of any sibling.
/// Returns the file's new collapsed state.
pub(crate) fn toggle_project_search_file(
    collapsed_files: &mut std::collections::HashSet<PathBuf>,
    path: &Path,
) -> bool {
    if collapsed_files.remove(path) {
        false
    } else {
        collapsed_files.insert(path.to_path_buf());
        true
    }
}

/// Convert one search hit into the same UTF-16 selection range used by LSP
/// navigation. Search stores byte offsets so highlighted Unicode text must be
/// translated before it is handed to the editor.
pub(crate) fn project_search_hit_range(
    hit: &crate::workspace::search_panel::SearchHit,
) -> dragent_editor::TextRange {
    let (start_character, end_character) = hit.match_range.map_or(
        (hit.col.saturating_sub(1), hit.col.saturating_sub(1)),
        |(start, end)| {
            let start_character = hit.line_text[..start].encode_utf16().count();
            let matched = hit.line_text[start..end].encode_utf16().count();
            (start_character, start_character + matched)
        },
    );
    dragent_editor::TextRange {
        start: dragent_editor::TextPosition {
            line: hit.line.saturating_sub(1) as u32,
            character: start_character as u32,
        },
        end: dragent_editor::TextPosition {
            line: hit.line.saturating_sub(1) as u32,
            character: end_character as u32,
        },
    }
}

pub(crate) fn render_sidebar_search_file_header(
    file: &crate::workspace::search_panel::SearchFileResult,
    collapsed: bool,
    view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
    sticky: bool,
) -> Box<dyn Element> {
    let file_name = file
        .relative_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.relative_path.to_string_lossy().into_owned());
    let parent_path = file
        .relative_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().into_owned());
    let hit_count = file.hits.len();
    let path = file.relative_path.clone();
    let row = Hoverable::new(MouseStateHandle::default(), move |state| {
        let identity = render_search_file_identity(
            file_name.clone(),
            parent_path.clone(),
            ui,
            crate::theme::text_size(11.5),
            theme,
        );
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(
                    Text::new(
                        (if collapsed { "▸ " } else { "▾ " }).to_string(),
                        ui,
                        crate::theme::text_size(10.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_child(identity)
                .with_child(
                    Container::new(
                        Text::new(hit_count.to_string(), ui, crate::theme::text_size(9.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .with_padding_left(crate::theme::scaled(6.0))
                    .with_padding_right(crate::theme::scaled(6.0))
                    .with_padding_top(crate::theme::scaled(1.0))
                    .with_padding_bottom(crate::theme::scaled(1.0))
                    .with_background_color(theme.surface_1())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish(),
                )
                .finish(),
        )
        .with_padding_left(crate::theme::scaled(10.0))
        .with_padding_right(crate::theme::scaled(8.0))
        .with_background_color(if state.is_hovered() {
            theme.surface_1()
        } else {
            theme.panel_background()
        })
        .with_border(if sticky {
            Border::bottom(1.0).with_border_color(theme.border())
        } else {
            Border::default()
        })
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    // Search rows are virtualized and can be rebuilt between press/release.
    // Toggle on press so the full-width header remains a reliable target.
    .on_mouse_down(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(
            view_id,
            SidebarAction::ToggleGlobalSearchFileCollapse { path: path.clone() },
        );
    })
    .finish();
    ConstrainedBox::new(row).with_height(28.0).finish()
}

pub(crate) fn render_search_file_identity(
    filename: String,
    parent: Option<String>,
    font: FamilyId,
    size: f32,
    theme: &Theme,
) -> Box<dyn Element> {
    let filename_chars = filename.chars().count();
    let label = parent.map_or_else(
        || filename.clone(),
        |parent| format!("{filename} — {parent}"),
    );
    Shrinkable::new(
        1.0,
        Text::new(label, font, size)
            .with_color(theme.sub_text())
            .with_single_highlight(
                Highlight::new()
                    .with_properties(Properties::default().weight(Weight::Medium))
                    .with_foreground_color(theme.main_text()),
                (0..filename_chars).collect(),
            )
            .soft_wrap(false)
            .with_clip(ClipConfig::ellipsis())
            .finish(),
    )
    .finish()
}

pub(crate) fn buffer_search_tooltip(
    child: Box<dyn Element>,
    hovered: bool,
    label: &str,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    if !hovered {
        return child;
    }
    let styles = UiComponentStyles {
        font_family_id: Some(font),
        font_size: Some(crate::theme::text_size(10.5)),
        font_color: Some(theme.main_text()),
        background: Some(Fill::Solid(theme.surface_2())),
        border_color: Some(Fill::Solid(theme.outline())),
        border_width: Some(1.0),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(5.0))),
        padding: Some(Coords {
            top: 4.0,
            bottom: 4.0,
            left: 7.0,
            right: 7.0,
        }),
        ..Default::default()
    };
    let tooltip = Tooltip::new(label.to_string(), styles).build().finish();
    Stack::new()
        .with_child(child)
        .with_positioned_overlay_child(
            tooltip,
            OffsetPositioning::offset_from_parent(
                vec2f(0.0, 5.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomMiddle,
                ChildAnchor::TopMiddle,
            ),
        )
        .finish()
}
