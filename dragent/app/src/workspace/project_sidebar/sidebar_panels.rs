//! Sidebar panels: quick actions, errors, diagnostics, sections.
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

use super::view::ProjectSidebarView;
use super::view::{
    BADGE_SIZE, META_SIZE, NAME_SIZE, PATH_SIZE, ROW_SIZE, SCRIPT_CMD_SIZE, SECTION_SIZE,
};

pub(crate) fn render_quick_action_chip(
    action: &QuickAction,
    workspace_view_id: EntityId,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let command = action.command.clone();
    let hover_state = view
        .quick_action_mouse_states
        .borrow_mut()
        .entry(command.clone())
        .or_insert_with(MouseStateHandle::default)
        .clone();

    Hoverable::new(hover_state, move |state| {
        let bg = if state.is_hovered() {
            with_alpha(theme.accent, 0.10)
        } else {
            theme.block_background()
        };
        let border = if state.is_hovered() {
            with_alpha(theme.accent, 0.35)
        } else {
            theme.border()
        };

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(render_icon(AppIcon::Terminal, theme.accent, ICON_SIZE_SM))
                        .with_child(
                            Text::new(action.label.clone(), ui, crate::theme::text_size(ROW_SIZE))
                                .with_color(theme.main_text())
                                .finish(),
                        )
                        .finish(),
                )
                .with_child(
                    Text::new(truncate(&action.command, 16), mono, crate::theme::text_size(META_SIZE))
                        .with_color(theme.disabled_text())
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .finish()
    })
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::RunActivePaneCommand {
                command: command.clone(),
            },
        );
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

pub(crate) fn render_errors_section(
    m: &ProjectSidebarModel,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let error_count = m
        .errors
        .iter()
        .filter(|e| e.level == ErrorLevel::Error)
        .count();

    let errors_hdr = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(section_title_inline("Errors", ui, theme))
        .with_child(if error_count > 0 {
            count_badge(error_count.to_string(), theme.ui_red(), ui)
        } else {
            Empty::new().finish()
        })
        .finish();

    let mut error_items: Vec<Box<dyn Element>> = Vec::new();
    for (idx, err) in m.errors.iter().rev().take(10).enumerate() {
        error_items.push(render_error_row(idx, err, ui, mono, theme, view));
    }

    if error_items.is_empty() {
        error_items.push(empty_label("No errors", ui, theme));
    }

    let mut errors_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(7.0);
    for item in error_items {
        errors_col = errors_col.with_child(item);
    }

    panel_section(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(Container::new(errors_hdr).with_padding_bottom(7.0).finish())
            .with_child(errors_col.finish())
            .finish(),
        None,
    )
}

pub(crate) fn render_language_diagnostics_section(
    diagnostics: &[dragent_editor::Diagnostic],
    root: PathBuf,
    document_path: Option<PathBuf>,
    workspace_view_id: EntityId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0);
    for (index, diagnostic) in diagnostics.iter().take(100).enumerate() {
        let color = match diagnostic.severity {
            dragent_editor::DiagnosticSeverity::Error => theme.ui_red(),
            dragent_editor::DiagnosticSeverity::Warning => theme.ui_yellow(),
            dragent_editor::DiagnosticSeverity::Information => theme.accent,
            dragent_editor::DiagnosticSeverity::Hint => theme.sub_text(),
        };
        let source = diagnostic.source.as_deref().unwrap_or("language server");
        let position_label = format!(
            "{}:{}",
            diagnostic.range.start.line + 1,
            diagnostic.range.start.character + 1
        );

        let mouse = mouse_states
            .borrow_mut()
            .entry(format!("language_diagnostic_{index}"))
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let message = diagnostic.message.clone();
        let severity_color = color;
        let hover_bg = theme.accent_overlay();
        let idle_bg = ColorU::new(0, 0, 0, 0);
        let source_owned = source.to_string();

        // The whole row is one navigation target: the diagnostic carries the
        // exact LSP range, and clicking reuses the SHARED language-location
        // navigation (same system as Go to Definition / References) to
        // open/reuse the file, move the cursor, reveal the exact range and
        // focus the editor.
        let row: Box<dyn Element> = match document_path.as_ref() {
            Some(document_path) => {
                let nav_path = document_path.clone();
                let nav_range = diagnostic.range;
                let nav_root = root.clone();
                Hoverable::new(mouse, move |state| {
                    let row = Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(7.0)
                        .with_child(
                            Text::new(position_label.clone(), mono, crate::theme::text_size(META_SIZE))
                                .with_color(severity_color)
                                .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.0,
                                Flex::column()
                                    .with_main_axis_size(MainAxisSize::Min)
                                    .with_child(
                                        Text::new(message.clone(), ui, crate::theme::text_size(META_SIZE))
                                            .with_color(theme.main_text())
                                            .soft_wrap(false)
                                            .finish(),
                                    )
                                    .with_child(
                                        Text::new(
                                            source_owned.clone(),
                                            ui,
                                            crate::theme::text_size((META_SIZE - 0.5).max(8.0)),
                                        )
                                        .with_color(theme.disabled_text())
                                        .soft_wrap(false)
                                        .finish(),
                                    )
                                    .finish(),
                            )
                            .finish(),
                        )
                        .finish();
                    Container::new(row)
                        .with_background_color(if state.is_hovered() {
                            hover_bg
                        } else {
                            idle_bg
                        })
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .finish()
                })
                .on_click(move |ctx, _app, _pos| {
                    log::info!(
                        "[problems] navigate file={} range={:?}",
                        nav_path.display(),
                        nav_range
                    );
                    ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::OpenLanguageLocation {
                            root: nav_root.clone(),
                            relative_path: nav_path.clone(),
                            range: nav_range,
                        },
                    );
                })
                .with_cursor(Cursor::PointingHand)
                .finish()
            }
            None => {
                // No active document to navigate within — read-only row.
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(7.0)
                        .with_child(
                            Text::new(position_label.clone(), mono, crate::theme::text_size(META_SIZE))
                                .with_color(severity_color)
                                .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(message.clone(), ui, crate::theme::text_size(META_SIZE))
                                    .with_color(theme.main_text())
                                    .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .finish()
            }
        };
        rows = rows.with_child(row);
    }

    panel_section(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_child(section_title_inline("Problems", ui, theme))
                        .with_child(count_badge(
                            diagnostics.len().to_string(),
                            theme.ui_red(),
                            ui,
                        ))
                        .finish(),
                )
                .with_padding_bottom(7.0)
                .finish(),
            )
            .with_child(rows.finish())
            .finish(),
        None,
    )
}

pub(crate) fn render_project_section(
    project: &ProjectInfo,
    is_active: bool,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let letter = project
        .name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    let avatar = ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(letter, mono, crate::theme::text_size(11.0))
                    .with_style(Properties::default().weight(Weight::Bold))
                    .with_color(theme.accent)
                    .finish(),
            )
            .finish(),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_background_color(blend_accent_bg(theme.accent))
        .finish(),
    )
    .with_width(26.0)
    .with_height(26.0)
    .finish();

    let name_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(avatar)
        .with_child(
            Expanded::new(
                1.0,
                Flex::column()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_child(
                        Text::new(project.name.clone(), ui, crate::theme::text_size(NAME_SIZE))
                            .with_style(Properties::default().weight(Weight::Semibold))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .with_child(
                        Text::new(path_label(&project.path), ui, crate::theme::text_size(PATH_SIZE))
                            .with_color(theme.sub_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
            )
            .finish(),
        )
        .finish();

    let mut info_rows: Vec<Box<dyn Element>> = Vec::new();
    if let Some(main) = super::super::project_naming::worktree_main_checkout(&project.git_root) {
        if main != project.git_root {
            if let Some(name) = project.git_root.file_name().and_then(|name| name.to_str()) {
                info_rows.push(kv_row("Worktree", name, theme.sub_text(), ui, mono, theme));
            }
        }
    }
    if let Some(rt) = project.runtimes.first() {
        info_rows.push(kv_row(
            "Runtime",
            &format!("{} {}", rt.kind.label(), rt.version),
            theme.ui_green(),
            ui,
            mono,
            theme,
        ));
    }
    if project.runtimes.len() > 1 {
        let extra: Vec<String> = project.runtimes[1..]
            .iter()
            .map(|r| format!("{} {}", r.kind.label(), r.version))
            .collect();
        info_rows.push(kv_row(
            "Also",
            &extra.join(", "),
            theme.sub_text(),
            ui,
            mono,
            theme,
        ));
    }
    if let Some(branch) = &project.branch {
        info_rows.push(kv_row_with_icon(
            "Branch",
            branch,
            theme.ansi(5),
            ui,
            mono,
            theme,
            AppIcon::GitBranch,
        ));
    }
    if let Some(status) = &project.status_label {
        info_rows.push(kv_row("Status", status, theme.accent, ui, mono, theme));
    }
    if project.is_monorepo {
        info_rows.push(kv_row(
            "Type",
            "monorepo",
            theme.sub_text(),
            ui,
            mono,
            theme,
        ));
    }

    let mut info_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0);
    for row in info_rows {
        info_col = info_col.with_child(row);
    }

    let border_color = if is_active {
        theme.accent
    } else {
        theme.border()
    };

    panel_section(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(section_title("Project", ui, theme))
            .with_child(Container::new(name_row).with_padding_bottom(9.0).finish())
            .with_child(info_col.finish())
            .finish(),
        Some(border_color),
    )
}

fn render_error_row(
    idx: usize,
    err: &super::model::ProcessError,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let accent = if err.level == ErrorLevel::Error {
        theme.ui_red()
    } else {
        theme.ui_yellow()
    };
    let elapsed = Instant::now().duration_since(err.recorded_at).as_secs();
    let time_label = fmt_elapsed(elapsed);

    let hover_state = view
        .error_mouse_states
        .borrow_mut()
        .entry(idx)
        .or_insert_with(MouseStateHandle::default)
        .clone();

    let content = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Text::new(truncate(&err.message, 32), mono, crate::theme::text_size(SCRIPT_CMD_SIZE))
                    .with_color(accent)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(time_label, ui, crate::theme::text_size(BADGE_SIZE))
                        .with_color(theme.disabled_text())
                        .finish(),
                )
                .with_padding_top(1.0)
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(7.0)
    .with_border(Border::left(2.0).with_border_color(accent))
    .finish();

    Hoverable::new(hover_state, move |state| {
        Container::new(content)
            .with_background_color(if state.is_hovered() {
                with_alpha(accent, 0.04)
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

// ── primitives ───────────────────────────────────────────────────────────────

pub(crate) fn panel_section(
    child: Box<dyn Element>,
    bottom_border: Option<ColorU>,
) -> Box<dyn Element> {
    let mut section = Container::new(child)
        .with_padding_left(13.0)
        .with_padding_right(13.0)
        .with_padding_top(12.0)
        .with_padding_bottom(11.0);
    if let Some(color) = bottom_border {
        section = section.with_border(Border::bottom(1.0).with_border_color(color));
    }
    section.finish()
}

pub(crate) fn section_title(label: &str, ui: FamilyId, theme: &Theme) -> Box<dyn Element> {
    Container::new(section_title_inline(label, ui, theme))
        .with_padding_bottom(7.0)
        .finish()
}

pub(crate) fn section_title_inline(label: &str, ui: FamilyId, theme: &Theme) -> Box<dyn Element> {
    Text::new(label.to_uppercase(), ui, crate::theme::text_size(SECTION_SIZE))
        .with_style(Properties::default().weight(Weight::Bold))
        .with_color(theme.disabled_text())
        .finish()
}

fn empty_label(text: &str, ui: FamilyId, theme: &Theme) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, crate::theme::text_size(ROW_SIZE))
        .with_color(theme.disabled_text())
        .finish()
}

fn count_badge(label: String, color: ColorU, ui: FamilyId) -> Box<dyn Element> {
    Container::new(Text::new(label, ui, crate::theme::text_size(BADGE_SIZE)).with_color(color).finish())
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .with_background_color(with_alpha(color, 0.08))
        .with_border(Border::all(1.0).with_border_color(with_alpha(color, 0.15)))
        .finish()
}

fn kv_row(
    label: &str,
    value: &str,
    value_color: ColorU,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(
            Text::new(label.to_string(), ui, crate::theme::text_size(ROW_SIZE))
                .with_color(theme.sub_text())
                .finish(),
        )
        .with_child(
            Expanded::new(
                1.0,
                Text::new(value.to_string(), mono, crate::theme::text_size(ROW_SIZE))
                    .with_color(value_color)
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish(),
        )
        .finish()
}

fn kv_row_with_icon(
    label: &str,
    value: &str,
    value_color: ColorU,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    icon: AppIcon,
) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(
            Text::new(label.to_string(), ui, crate::theme::text_size(ROW_SIZE))
                .with_color(theme.sub_text())
                .finish(),
        )
        .with_child(
            Expanded::new(
                1.0,
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(render_icon(icon, value_color, ICON_SIZE_SM))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(value.to_string(), mono, crate::theme::text_size(ROW_SIZE))
                                .with_color(value_color)
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .finish(),
        )
        .finish()
}

fn path_label(path: &std::path::Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let p = path.to_string_lossy();
    let h = home.to_string_lossy();
    if p.starts_with(h.as_ref()) {
        format!("~{}", &p[h.len()..])
    } else {
        p.into_owned()
    }
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            chars[..max.saturating_sub(1)].iter().collect::<String>()
        )
    }
}

fn fmt_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

fn accent_magenta() -> ColorU {
    ColorU::new(180, 100, 220, 255)
}

pub(crate) fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

fn blend_accent_bg(accent: ColorU) -> ColorU {
    ColorU::new(accent.r / 10 + 8, accent.g / 10 + 8, accent.b / 10 + 8, 255)
}
