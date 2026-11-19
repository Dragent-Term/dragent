// Sidebar navigation, projects header, and menu primitives.
//
// Moved verbatim; same behavior.

/// Which full-screen surface the sidebar nav points at, if any. The matching
/// row keeps its hover background while its screen owns the center column.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SidebarNavActive {
    #[default]
    None,
    Automations,
    Teams,
    Tasks,
    PullRequests,
}

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::{
    AppContext, Entity, EntityId, TypedActionView, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::elements::{
    AcceptedByDropTarget, DispatchEventResult, Draggable, DraggableState, DropTargetData,
    EventHandler, Hoverable, MouseStateHandle,
};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::theme::Theme;

use super::left_panel::{
    sidebar_mouse, tab_drag, LeftPanelAction, ProjectData, SidebarSortMode, TabData, FONT_MENU,
    FONT_MENU_SECTION, MENU_ICON_SIZE, MENU_ICON_SLOT,
};
use super::panel_tabs::render_tab_menu;
use super::sidebar::{tab_color_options, SidebarFocus, SidebarViewMode};
use super::{PaneDropZone, WorkspaceAction};

#[derive(Debug, Clone)]
pub(crate) struct SidebarGroupDropTarget {
    pub project_key: String,
    pub group: Option<String>,
}

impl DropTargetData for SidebarGroupDropTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarTabDropTarget {
    pub project_key: String,
    pub tab_id: usize,
    pub group: Option<String>,
}

impl DropTargetData for SidebarTabDropTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A whole project section, where a dragged project heading can land.
#[derive(Debug, Clone)]
pub(crate) struct SidebarProjectDropTarget {
    pub project_key: String,
}

impl DropTargetData for SidebarProjectDropTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Where a sidebar row's leading glyph starts at tree `depth` (0: no
/// project, 1: in a project, 2: in one of its folders). Kept tight: the
/// project header's chevron sits at 8 (4 margin + 4 padding) and each level
/// steps in 12pt, so nested rows keep their room for titles.
pub(crate) fn tree_lead(depth: usize) -> f32 {
    match depth {
        0 => 9.0,
        depth => 8.0 + 12.0 * depth as f32,
    }
}

pub(crate) fn render_workspace_group_header(
    workspace_id: EntityId,
    ui_font: FamilyId,
    theme: &Theme,
    project_key: &str,
    group: &str,
    is_collapsed: bool,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let hover = sidebar_mouse(
        sidebar_mouse_states,
        format!("workspace_group:{project_key}:{group}"),
    );
    let label = group.to_string();
    let action_key = project_key.to_string();
    let action_group = group.to_string();
    let remove_mouse = sidebar_mouse(
        sidebar_mouse_states,
        format!("remove_group:{project_key}:{group}"),
    );
    let remove_root = std::path::PathBuf::from(project_key);
    let remove_group = group.to_string();
    Hoverable::new(hover, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(render_icon(
                    if is_collapsed {
                        AppIcon::ChevronRight
                    } else {
                        AppIcon::ChevronDown
                    },
                    theme.disabled_text(),
                    10.0,
                ))
                .with_child(render_icon(
                    if is_collapsed {
                        AppIcon::Folder
                    } else {
                        AppIcon::FolderOpen
                    },
                    theme.sub_text(),
                    12.0,
                ))
                .with_child(
                    Expanded::new(
                        1.0,
                        // Muted, so a folder reads as part of its project
                        // rather than another project.
                        Text::new(label.clone(), ui_font, crate::theme::text_size(12.0))
                            .with_color(theme.sub_text())
                            .with_style(Properties::default().weight(Weight::Medium))
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(if state.is_hovered() {
                    Hoverable::new(remove_mouse.clone(), move |mouse| {
                        let button = Container::new(render_icon(
                            AppIcon::XClose,
                            if mouse.is_hovered() {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            },
                            12.0,
                        ))
                        .with_padding_left(5.0)
                        .with_padding_right(5.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .finish();
                        crate::machine_ui::with_tooltip(
                            theme,
                            ui_font,
                            button,
                            mouse.is_hovered(),
                            "Delete group and its workspaces…",
                        )
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click({
                        let root = remove_root.clone();
                        let group = remove_group.clone();
                        move |ctx, _, _| {
                            ctx.dispatch_typed_action_for(
                                workspace_id,
                                WorkspaceAction::RemoveProjectGroup {
                                    root: root.clone(),
                                    group: group.clone(),
                                },
                            )
                        }
                    })
                    .finish()
                } else {
                    Empty::new().finish()
                })
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
            theme.sidebar_row_hover()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_padding_left(tree_lead(1))
        .with_padding_right(10.0)
        .with_padding_top(9.0)
        .with_padding_bottom(7.0)
        .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(LeftPanelAction::ToggleWorkspaceGroup {
            project_key: action_key.clone(),
            group: action_group.clone(),
        });
    })
    .finish()
}


/// A project's picture in its heading: a small rounded square holding its
/// icon (or its owner's avatar), else its first letter.
fn project_thumbnail(
    icon: Option<std::path::PathBuf>,
    letter: &str,
    ui_font: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    const SIDE: f32 = 16.0;
    // The letter shows until the picture has loaded, and stays if it can't
    // be read.
    let letter_tile = || -> Box<dyn Element> {
        Align::new(
            Text::new(letter.to_string(), ui_font, crate::theme::text_size(9.5))
                .with_color(theme.sub_text())
                .with_style(Properties::default().weight(Weight::Semibold))
                .finish(),
        )
        .finish()
    };
    let inner: Box<dyn Element> = match icon {
        Some(path) => {
            use warpui_core::assets::asset_cache::AssetSource;
            use warpui_core::image_cache::CacheOption;
            warpui::elements::Image::new(
                AssetSource::LocalFile { path: path.to_string_lossy().into_owned() },
                CacheOption::BySize,
            )
            .contain()
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PROJECT_ICON_RADIUS)))
            .before_load(letter_tile())
            .on_load_failure(letter_tile())
            .finish()
        }
        None => letter_tile(),
    };
    Container::new(ConstrainedBox::new(inner).with_width(SIDE).with_height(SIDE).finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PROJECT_ICON_RADIUS)))
        .finish()
}

/// Corner radius of a project's picture in its heading.
const PROJECT_ICON_RADIUS: f32 = 5.0;

pub(crate) fn render_project_group_header(
    workspace_id: EntityId,
    ui_font: FamilyId,
    theme: &Theme,
    project_key: &str,
    label: &str,
    item_count: usize,
    is_collapsed: bool,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    machine: Option<(String, crate::settings::machines_store::MachineStatus)>,
    drag_state: Option<DraggableState>,
    icon: Option<std::path::PathBuf>,
) -> Box<dyn Element> {
    let action_key = project_key.to_string();
    let letter = label.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    // Remote projects show a server glyph and their machine's connection
    // dot after the name; everything else is identical.
    let remote = machine.is_some() || crate::remote_fs::is_remote(std::path::Path::new(project_key));
    let dot_hover = sidebar_mouse(sidebar_mouse_states, format!("project_machine_dot:{project_key}"));
    let mouse_state = sidebar_mouse(sidebar_mouse_states, format!("project_group:{project_key}"));
    let header = Hoverable::new(mouse_state, move |state| {
        let background = if state.is_hovered() {
            theme.sidebar_row_hover()
        } else {
            ColorU::transparent_black()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                // No chevron: clicking the heading folds the project.
                .with_child(if remote {
                    render_icon(AppIcon::Server, theme.accent, 12.0)
                } else {
                    project_thumbnail(icon.clone(), &letter, ui_font, theme)
                })
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(7.0)
                            .with_child(
                                Shrinkable::new(
                                    1.0,
                                    Text::new(label.to_string(), ui_font, crate::theme::text_size(14.0))
                                        .with_color(theme.main_text())
                                        .with_style(Properties::default().weight(Weight::Medium))
                                        .soft_wrap(false)
                                        .with_clip(ClipConfig::ellipsis())
                                        .finish(),
                                )
                                .finish(),
                            )
                            .with_children(machine.clone().map(|(name, status)| {
                                let tip = crate::machine_ui::status_tooltip(&name, status);
                                Hoverable::new(dot_hover.clone(), move |dot_state| {
                                    crate::machine_ui::with_tooltip(
                                        theme,
                                        ui_font,
                                        crate::machine_ui::sized(
                                            crate::machine_ui::status_dot(theme, status, 7.0),
                                            12.0,
                                            12.0,
                                        ),
                                        dot_state.is_hovered(),
                                        &tip,
                                    )
                                })
                                .finish()
                            }))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Hoverable::new(
                        sidebar_mouse(sidebar_mouse_states, format!("project_add:{project_key}")),
                        move |state| {
                            Container::new(render_icon(
                                AppIcon::Plus,
                                if state.is_hovered() {
                                    theme.main_text()
                                } else {
                                    theme.sub_text()
                                },
                                12.0,
                            ))
                            .with_padding_left(5.0)
                            .with_padding_right(5.0)
                            .with_padding_top(5.0)
                            .with_padding_bottom(5.0)
                            .with_background_color(if state.is_hovered() {
                                theme.surface_2()
                            } else {
                                ColorU::transparent_black()
                            })
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                            .finish()
                        },
                    )
                    .with_cursor(Cursor::PointingHand)
                    .on_click({
                        let root = project_key.to_string();
                        move |event_ctx, _app, _pos| {
                            event_ctx.dispatch_typed_action_for(
                                workspace_id,
                                WorkspaceAction::OpenProjectWorkspacePrompt {
                                    root: root.clone().into(),
                                },
                            );
                        }
                    })
                    .finish(),
                )
                .with_child(
                    Text::new(item_count.to_string(), ui_font, crate::theme::text_size(9.5))
                        .with_color(theme.disabled_text())
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_padding_left(4.0)
        .with_padding_right(10.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_margin_left(4.0)
        .with_margin_right(4.0)
        .with_margin_top(4.0)
        .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(LeftPanelAction::ToggleProjectGroup {
            project_key: action_key.clone(),
        });
    })
    .finish();
    let root = project_key.to_string();
    let header = EventHandler::new(header)
        .on_right_mouse_down(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action_for(
                workspace_id,
                WorkspaceAction::ToggleProjectContextMenu {
                    root: root.clone().into(),
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish();
    // Drag the heading onto another project to move the whole project
    // there; a click still folds it.
    let header = match drag_state {
        Some(drag_state) => {
            let own = project_key.to_string();
            let source = project_key.to_string();
            let committed = project_key.to_string();
            let target_of = |data: Option<&dyn DropTargetData>| {
                data.and_then(|data| data.as_any().downcast_ref::<SidebarProjectDropTarget>())
                    .map(|target| target.project_key.clone())
            };
            Draggable::new(drag_state, header)
                .with_drag_axis(warpui_core::elements::DragAxis::VerticalOnly)
                .with_keep_original_visible(true)
                .with_drop_target_at_pointer()
                .with_accepted_by_drop_target_fn(move |data, _app| {
                    match data.as_any().downcast_ref::<SidebarProjectDropTarget>() {
                        Some(target) if target.project_key != own => AcceptedByDropTarget::Yes,
                        _ => AcceptedByDropTarget::No,
                    }
                })
                .on_drag(move |event_ctx, _app, _rect, data| {
                    event_ctx.dispatch_typed_action(LeftPanelAction::PreviewProjectDrop(
                        target_of(data).map(|target| super::left_panel::ProjectDragPreview {
                            source: source.clone(),
                            target,
                        }),
                    ));
                })
                .on_drop(move |event_ctx, _app, _rect, data| {
                    event_ctx.dispatch_typed_action(LeftPanelAction::CommitProjectDrop {
                        source: committed.clone(),
                        target: target_of(data),
                    });
                })
                .finish()
        }
        None => header,
    };
    SavePosition::new(header, &format!("project_header_{project_key}")).finish()
}

/// Builds the floating "..." tab context menu overlay (menu + click-outside
/// scrim), anchored to the tab's overflow button via `SavePosition` tag
/// `tab_menu_btn_{tab_id}`. Rendered from `WorkspaceView`'s own top-level
/// window Stack (not nested inside `LeftPanelView`'s render tree) so it isn't
/// clipped/composited within the sidebar's own bounded sub-view.
pub fn render_tab_menu_overlay(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    tab: &TabData,
    groups: &[String],
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    panel_width: f32,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let tab_id = tab.id;
    let menu = render_tab_menu(
        ui_font,
        theme,
        workspace_id,
        tab,
        groups,
        sidebar_mouse_states,
    );
    let menu = ConstrainedBox::new(menu).with_width(panel_width).finish();
    let menu = EventHandler::new(menu)
        .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
        .finish();
    let scrim = Container::new(Align::new(Empty::new().finish()).finish())
        .with_background_color(ColorU::new(0, 0, 0, 0))
        .finish();
    let scrim = EventHandler::new(scrim)
        .on_left_mouse_down(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::CloseTabMenu);
            DispatchEventResult::StopPropagation
        })
        .finish();
    let offset = OffsetPositioning::offset_from_save_position_element(
        &format!("tab_menu_btn_{tab_id}"),
        vec2f(0.0, 2.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomLeft,
        ChildAnchor::TopLeft,
    );
    (menu, scrim, offset)
}

/// Top navigation: New Workspace and Automations, plus Pull requests as a
/// disabled "Soon" row while that surface is being built. The workspaces
/// are the list below, so they have no row of their own.
pub(crate) fn render_sidebar_nav(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    automation_failures: usize,
    teams_needing_you: usize,
    active: SidebarNavActive,
) -> Box<dyn Element> {
    let mut nav = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0)
        .with_child(sidebar_nav_row(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "nav-new"),
            AppIcon::Plus,
            "New Workspace",
            true,
            true,
            false,
            Some(SidebarNavClick::Workspace(
                WorkspaceAction::ToggleNewTabPickerFromSidebar,
            )),
        ))
        .with_child(sidebar_nav_row(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "nav-tasks"),
            AppIcon::TaskList,
            "Tasks",
            false,
            cfg!(not(target_family = "wasm")),
            active == SidebarNavActive::Tasks,
            Some(SidebarNavClick::Workspace(WorkspaceAction::OpenTasks)),
        ))
        .with_child(sidebar_nav_row_badged(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "nav-automations"),
            AppIcon::Clock,
            "Automations",
            cfg!(not(target_family = "wasm")),
            Some(SidebarNavClick::Workspace(WorkspaceAction::OpenAutomations)),
            automation_failures,
            active == SidebarNavActive::Automations,
        ))
        .with_child(sidebar_nav_row_badged(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "nav-teams"),
            AppIcon::Layers,
            "Agent Coordination",
            cfg!(not(target_family = "wasm")),
            Some(SidebarNavClick::Workspace(WorkspaceAction::OpenTeams { team: None })),
            teams_needing_you,
            active == SidebarNavActive::Teams,
        ))
        .with_child(sidebar_nav_row(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "nav-pull-requests"),
            AppIcon::PullRequest,
            "Code reviews",
            false,
            cfg!(not(target_family = "wasm")),
            active == SidebarNavActive::PullRequests,
            Some(SidebarNavClick::Workspace(WorkspaceAction::OpenPullRequests)),
        ));
    Container::new(nav.finish())
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(8.0)
        .with_padding_bottom(6.0)
        .finish()
}

/// A nav row with a red count of things needing attention (hidden at 0).
#[allow(clippy::too_many_arguments)]
fn sidebar_nav_row_badged(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    label: &'static str,
    enabled: bool,
    action: Option<SidebarNavClick>,
    count: usize,
    active: bool,
) -> Box<dyn Element> {
    let row = sidebar_nav_row(ui_font, theme, workspace_id, mouse, icon, label, false, enabled, active, action);
    if count == 0 {
        return row;
    }
    let badge = Container::new(
        Text::new(count.min(99).to_string(), ui_font, crate::theme::text_size(10.0))
            .with_color(ColorU::new(255, 255, 255, 255))
            .finish(),
    )
    .with_background_color(theme.ui_red())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(1.0)
    .with_padding_bottom(1.0)
    .finish();
    let mut stack = warpui::elements::Stack::new().with_child(row);
    stack.add_positioned_overlay_child(
        badge,
        OffsetPositioning::offset_from_parent(
            vec2f(-10.0, 0.0),
            warpui::elements::ParentOffsetBounds::WindowByPosition,
            warpui::elements::ParentAnchor::MiddleRight,
            ChildAnchor::MiddleRight,
        ),
    );
    stack.finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sidebar_nav_row(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    label: &'static str,
    highlighted: bool,
    enabled: bool,
    active: bool,
    action: Option<SidebarNavClick>,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let hoverable = Hoverable::new(mouse, move |state| {
        let icon_el: Box<dyn Element> = if highlighted {
            Container::new(render_icon(AppIcon::Plus, theme_owned.background, 12.0))
                .with_background_color(theme_owned.accent)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish()
        } else {
            render_icon(
                icon,
                if enabled {
                    theme_owned.sub_text()
                } else {
                    theme_owned.disabled_text()
                },
                15.0,
            )
        };

        let label_el = Expanded::new(
            1.0,
            Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                .with_color(if enabled {
                    theme_owned.main_text()
                } else {
                    theme_owned.disabled_text()
                })
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
        )
        .finish();

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(icon_el)
            .with_child(label_el);
        if !enabled {
            row = row.with_child(
                Text::new("Soon".to_string(), ui_font, crate::theme::text_size(10.0))
                    .with_color(theme_owned.disabled_text())
                    .finish(),
            );
        }

        Container::new(row.finish())
            .with_background_color(if highlighted {
                theme_owned.accent_overlay()
            } else if enabled && (state.is_hovered() || active) {
                theme_owned.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish()
    });
    if !enabled {
        return hoverable.finish();
    }
    let action = action.expect("enabled sidebar nav rows must carry an action");
    hoverable
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| match action.clone() {
            SidebarNavClick::Workspace(action) => {
                event_ctx.dispatch_typed_action_for(workspace_id, action);
            }
        })
        .finish()
}

#[derive(Clone)]
pub(crate) enum SidebarNavClick {
    Workspace(WorkspaceAction),
}

/// Projects section header: collapse toggle, expanding
/// search, sort toggle (manual / name), and add-project. Sits between the
/// top nav and the project groups, replacing the old always-visible search
/// bar — the same InputEditor is reused, so existing filter wiring carries
/// over unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_projects_header(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    collapsed: bool,
    search_expanded: bool,
    sort_mode: SidebarSortMode,
    search_editor: &ViewHandle<crate::workspace::InputEditor>,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let collapse = Hoverable::new(
        sidebar_mouse(sidebar_mouse_states, "projects-header"),
        move |state| {
            let background = if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(
                        if collapsed {
                            AppIcon::ChevronRight
                        } else {
                            AppIcon::ChevronDown
                        },
                        theme.sub_text(),
                        10.0,
                    ))
                    .with_child(
                        Text::new("Projects".to_string(), ui_font, crate::theme::text_size(11.0))
                            .with_color(theme.main_text())
                            .with_style(Properties::default().weight(Weight::Medium))
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(LeftPanelAction::ToggleProjects);
    })
    .finish();

    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0)
        .with_child(collapse)
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());

    let row = row
        .with_child(projects_icon_button(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "projects-search"),
            AppIcon::Search,
            search_expanded,
            "Search projects",
            ProjectsHeaderClick::Panel(LeftPanelAction::ToggleProjectsSearch),
        ))
        .with_child(projects_icon_button(
            ui_font,
            theme,
            workspace_id,
            sidebar_mouse(sidebar_mouse_states, "projects-sort"),
            AppIcon::Sort,
            sort_mode == SidebarSortMode::Name,
            if sort_mode == SidebarSortMode::Name {
                "Sorted by name — switch to manual order"
            } else {
                "Manual order — sort by name"
            },
            ProjectsHeaderClick::Panel(LeftPanelAction::CycleSortMode),
        ))
        .with_child(
            SavePosition::new(
                projects_icon_button(
                    ui_font,
                    theme,
                    workspace_id,
                    sidebar_mouse(sidebar_mouse_states, "projects-add"),
                    AppIcon::FolderPlus,
                    false,
                    "Add project",
                    ProjectsHeaderClick::Workspace(WorkspaceAction::ToggleAddProjectMenu),
                ),
                "projects_add_btn",
            )
            .finish(),
        );

    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(row.finish())
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        );
    if search_expanded {
        let editor = search_editor.clone();
        column = column.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(crate::theme::scaled(6.0))
                    .with_child(Expanded::new(1.0, ChildView::new(&editor).finish()).finish())
                    .with_child(
                        Hoverable::new(
                            sidebar_mouse(sidebar_mouse_states, "projects-search-clear"),
                            move |state| {
                                Container::new(render_icon(
                                    AppIcon::XClose,
                                    if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
                                    crate::theme::scaled(11.0),
                                ))
                                .with_uniform_padding(crate::theme::scaled(4.0))
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                                    crate::theme::scaled(4.0),
                                )))
                                .finish()
                            },
                        )
                        .with_cursor(Cursor::PointingHand)
                        .on_click(move |event_ctx, _app, _pos| {
                            event_ctx.dispatch_typed_action(LeftPanelAction::ClearProjectsSearch);
                        })
                        .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(10.0))
            .with_padding_right(crate::theme::scaled(8.0))
            .with_padding_top(crate::theme::scaled(6.0))
            .with_padding_bottom(crate::theme::scaled(6.0))
            .with_background_color(theme.pane_surface())
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish(),
        );
    }
    Container::new(column.finish())
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
}

#[derive(Clone)]
pub(crate) enum ProjectsHeaderClick {
    Panel(LeftPanelAction),
    Workspace(WorkspaceAction),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn projects_icon_button(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    active: bool,
    tooltip: &'static str,
    click: ProjectsHeaderClick,
) -> Box<dyn Element> {
    Hoverable::new(mouse, move |state| {
        let color = if active {
            theme.accent
        } else if state.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let button = Container::new(render_icon(icon, color, 13.0))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish();
        super::ide_chrome::tooltip_below(button, state.is_hovered(), tooltip, theme, ui_font)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| match click.clone() {
        ProjectsHeaderClick::Panel(action) => {
            event_ctx.dispatch_typed_action(action);
        }
        ProjectsHeaderClick::Workspace(action) => {
            event_ctx.dispatch_typed_action_for(workspace_id, action);
        }
    })
    .finish()
}

pub(crate) fn render_color_strip(
    theme: &Theme,
    workspace_id: EntityId,
    tab: &TabData,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let tab_id = tab.id;
    let mut row = Flex::row().with_spacing(6.0);
    for (ci, option) in tab_color_options().iter().enumerate() {
        let color = *option;
        let selected = tab.color == color;
        let size = if color.is_some() { 12.0 } else { 10.0 };
        let bg = color.unwrap_or(theme.surface_3());
        let outline = if selected {
            theme.main_text()
        } else {
            ColorU::transparent_black()
        };
        let swatch_key = format!("swatch_{tab_id}_{ci}");
        let swatch_mouse = sidebar_mouse(sidebar_mouse_states, swatch_key);

        row = row.with_child(
            Hoverable::new(swatch_mouse, move |_| {
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(size)
                        .with_height(size)
                        .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size / 2.0)))
                .with_border(Border::all(2.0).with_border_color(outline))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(
                    workspace_id,
                    WorkspaceAction::SetTabColor { tab_id, color },
                );
            })
            .finish(),
        );
    }

    Container::new(row.finish())
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_bottom(6.0)
        .finish()
}

pub(crate) fn menu_item(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    label: &str,
    icon: AppIcon,
    danger: bool,
    action: WorkspaceAction,
    mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let label = label.to_string();
    let text_color = if danger {
        theme.ui_red()
    } else {
        theme.main_text()
    };
    let icon_color = if danger {
        theme.ui_red()
    } else {
        theme.sub_text()
    };

    Hoverable::new(mouse_state, move |state| {
        let bg = if state.is_hovered() {
            if danger {
                with_alpha(theme.ui_red(), 0.12)
            } else {
                theme.sidebar_menu_hover()
            }
        } else {
            ColorU::transparent_black()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(
                    ConstrainedBox::new(
                        Align::new(render_icon(icon, icon_color, MENU_ICON_SIZE)).finish(),
                    )
                    .with_width(MENU_ICON_SLOT)
                    .finish(),
                )
                .with_child(
                    Text::new(label.clone(), ui_font, crate::theme::text_size(FONT_MENU))
                        .with_color(text_color)
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_id, action.clone());
    })
    .finish()
}

pub(crate) fn menu_separator(theme: &Theme) -> Box<dyn Element> {
    Container::new(Empty::new().finish())
        .with_background_color(theme.border())
        .with_padding_top(1.0)
        .with_margin_left(6.0)
        .with_margin_right(6.0)
        .with_margin_top(2.0)
        .with_margin_bottom(2.0)
        .finish()
}

pub(crate) fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}
