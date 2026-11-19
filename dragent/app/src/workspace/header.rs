//! Global workspace title bar — toolbar, search, settings, and account controls.

use dragent_workspace_model::{AttentionItemId, BuiltInLayout};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Flex, MainAxisAlignment, MainAxisSize, OffsetPositioning, ParentElement,
    PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Clipped, DispatchEventResult, EventHandler};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::theme::{
    Theme, HEADER_BAR_HEIGHT, HEADER_ICON_PADDING, HEADER_SEARCH_MAX_WIDTH, TAB_BAR_BORDER_HEIGHT,
};

use super::execution_workspace::AttentionOverlayEntry;
/// Re-exported so existing `header::…` paths keep working.
pub(crate) use super::header_menus::*;
use super::header_menus::{render_account_avatar_button, render_attention_bell};
use super::{InputEditor, WorkspaceAction};

/// Left navbar segment: sidebar toggle only. Rendered above the left
/// panel so the sidebar's right demarcation line passes straight through
/// the navbar instead of stopping below it.
pub fn render_left_segment(
    theme: &Theme,
    left_inset: f32,
    workspace_view_id: EntityId,
    left_panel_visible: bool,
    header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    navbar_segment(
        theme,
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(2.0)
            .with_child(toolbar_button(
                theme,
                workspace_view_id,
                header_mouse_state("sidebar"),
                AppIcon::LayoutLeft,
                left_panel_visible,
                WorkspaceAction::ToggleLeftPanel,
            ))
            .finish(),
        left_inset,
        0.0,
        Some(Side::Right),
    )
}

/// Center navbar segment: Review split-button plus the global controls.
/// Rendered above the terminal/editor column.
///
/// `minimal` hides every control except the Windows caption cluster (which
/// renders whenever the right strip is gone): the empty start screen keeps
/// its sidebar header but no other header content. Borderless Windows
/// windows have no native caption buttons, so those must survive.
#[allow(clippy::too_many_arguments)]
pub fn render_center_segment(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    left_panel_visible: bool,
    // Traffic-light inset, used only while the sidebar is collapsed and the
    // toggle moves into this segment (see `render_left_segment`).
    left_inset: f32,
    header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    settings_active: bool,
    account_logged_in: bool,
    // Signed-in identity for the header avatar: (display name, avatar URL).
    account_identity: (String, String),
    attention_count: usize,
    unread_automation_results: usize,
    attention_bell_open: bool,
    window_maximized: bool,
    right_panel_shown: bool,
    minimal: bool,
    // Top-level workspace tabs for the header row. `None` (and `minimal`)
    // keeps the old empty spacer: no workspace, no tabs.
    workspace_tabs: Option<Box<dyn Element>>,
    // Marks a workspace with the issue it's linked to.
    automation_pill: Option<Box<dyn Element>>,
    // Agent plan usage, beside the attention bell.
    usage_chip: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(6.0);
    if !minimal {
        if !left_panel_visible {
            // The left segment is gone with the panel, so its toggle moves here —
            // drawn exactly as it is with the sidebar open (same chip, same spot
            // right after the traffic lights) so collapsing never moves or
            // restyles the control the user just clicked.
            row = row.with_child(toolbar_button(
                theme,
                workspace_view_id,
                header_mouse_state("sidebar"),
                AppIcon::LayoutLeft,
                true,
                WorkspaceAction::ToggleLeftPanel,
            ));
        }
        if let Some(tabs) = workspace_tabs {
            // Reserve the toolbar first, then give the tab area all remaining
            // width. Inside it, only the strip flexes: it takes its intrinsic
            // width until it reaches the space left by the fixed add button.
            // Do not put an Expanded spacer beside the strip; WarpUI divides
            // flex space between them and would start scrolling at half width.
            let tab_area = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(6.0)
                .with_child(Shrinkable::new(1.0, tabs).finish())
                .with_child(
                    SavePosition::new(
                        new_workspace_tab_button(
                            theme,
                            workspace_view_id,
                            header_mouse_state("workspace_tab_new"),
                        ),
                        NEW_TAB_BUTTON_POSITION,
                    )
                    .finish(),
                )
                .finish();
            row = row.with_child(Expanded::new(1.0, tab_area).finish());
        } else {
            row = row.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
        }
        if let Some(pill) = automation_pill {
            row = row.with_child(pill);
        }
        if let Some(chip) = usage_chip {
            row = row.with_child(chip);
        }
        row = row.with_child(render_attention_bell(
            ui_font,
            theme,
            workspace_view_id,
            header_mouse_state("attention_bell"),
            attention_count,
            unread_automation_results,
            attention_bell_open,
        ));
    }

    // Signed out → "Sign in". Signed in → the account avatar, which opens the
    // account menu (documentation / upgrade / sign out).
    if !minimal {
        row = row.with_child(if account_logged_in {
            render_account_avatar_button(
                ui_font,
                theme,
                workspace_view_id,
                header_mouse_state("account"),
                &account_identity.0,
                &account_identity.1,
            )
        } else {
            render_sign_in_button(
                ui_font,
                theme,
                workspace_view_id,
                header_mouse_state("signin"),
            )
        });

        // A staged release is actionable immediately from the global header. The
        // account menu keeps the same action for users who discover it there, but
        // the primary control should not be hidden behind another menu.
        if let crate::autoupdate::UpdateState::Ready(version) = crate::autoupdate::update_state() {
            row = row.with_child(render_restart_update_button(
                ui_font,
                theme,
                workspace_view_id,
                header_mouse_state("restart_update"),
                version,
            ));
        }

        row = row.with_child(toolbar_button(
            theme,
            workspace_view_id,
            header_mouse_state("settings"),
            AppIcon::Settings,
            settings_active,
            WorkspaceAction::OpenSettings,
        ));

        if !right_panel_shown {
            // The right strip (and its collapse toggle) is gone with the panel:
            // reopen from here, mirroring the left sidebar fallback.
            row = row.with_child(toolbar_button(
                theme,
                workspace_view_id,
                header_mouse_state("expand_right"),
                AppIcon::LayoutRight,
                false,
                WorkspaceAction::ToggleRightPanel,
            ));
        }
    }

    if minimal {
        // Nothing else is in the row to push the caption buttons to the
        // right edge, where they belong on every screen.
        row = row.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    }

    if !right_panel_shown {
        #[cfg(windows)]
        {
            row = row.with_child(render_caption_cluster(
                theme,
                workspace_view_id,
                header_mouse_state,
                window_maximized,
            ));
        }
    }

    let padding_left = if left_panel_visible { 0.0 } else { left_inset };
    navbar_segment(theme, row.finish(), padding_left, 12.0, None)
}

/// Right navbar segment. Rendered above the right panel so its left
/// demarcation line passes through the navbar; hosts the Review
/// split-button plus the Windows caption buttons (macOS gets native
/// traffic lights, so the strip is otherwise quiet).
pub fn render_right_segment(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    window_maximized: bool,
    review_active: bool,
    ship: &super::view::ReviewShipState,
    open_count: usize,
    panel_width: f32,
) -> Box<dyn Element> {
    // Collapse stays inset from the sidebar edge. Review is right-anchored
    // with no trailing margin, so optional stats can only grow toward the
    // center instead of escaping past the window edge. Its left hairline
    // separates the expanding control from the rest of the panel header.
    let review = ConstrainedBox::new(
        Container::new(
            Align::new(render_review_button(
                ui_font,
                theme,
                workspace_view_id,
                header_mouse_state("review_main"),
                header_mouse_state("review_chevron"),
                header_mouse_state("review_stats"),
                review_active,
                ship,
                open_count,
            ))
            .finish(),
        )
        .with_border(Border::left(1.0).with_border_color(theme.border()))
        .finish(),
    )
    .with_height(HEADER_BAR_HEIGHT)
    .finish();
    let mut group = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(0.0)
        .with_child(review);
    // Windows draws no native caption buttons on a borderless window, so the
    // header supplies its own minimize/maximize/close cluster, flush against
    // the top-right corner per platform convention. (Never an Empty
    // placeholder on other platforms: Empty takes all available space and
    // would shove Review off the edge.)
    #[cfg(windows)]
    {
        group = group.with_child(render_caption_cluster(
            theme,
            workspace_view_id,
            header_mouse_state,
            window_maximized,
        ));
    }
    // Max row capped at the live panel width: fills the strip so
    // SpaceBetween docks Review at the far right, while the cap keeps the
    // Max finite under unbounded measure (a bare Max would abort there).
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(
            Container::new(toolbar_button(
                theme,
                workspace_view_id,
                header_mouse_state("collapse_right"),
                AppIcon::LayoutRight,
                true,
                WorkspaceAction::ToggleRightPanel,
            ))
            .with_margin_left(15.0)
            .with_margin_right(15.0)
            .finish(),
        )
        .with_child(group.finish());
    let row = ConstrainedBox::new(row.finish())
        .with_max_width(panel_width.max(1.0))
        .finish();
    // Item margins carry all the spacing now; the segment adds none.
    navbar_segment(theme, row, 0.0, 0.0, Some(Side::Left))
}

/// Windows caption cluster (minimize / maximize / close). Windows-only:
/// other platforms draw native controls, and an Empty placeholder would
/// expand to fill the row and displace its siblings.
#[cfg(windows)]
fn render_caption_cluster(
    theme: &Theme,
    workspace_view_id: EntityId,
    header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    window_maximized: bool,
) -> Box<dyn Element> {
    Container::new(render_caption_buttons(
        theme,
        workspace_view_id,
        header_mouse_state,
        window_maximized,
    ))
    .with_padding_left(8.0)
    .finish()
}

enum Side {
    Left,
    Right,
}

/// One strip of the segmented navbar: fixed header height, sidebar chrome
/// background, bottom border, and an optional side border continuing the
/// adjacent panel's demarcation line through the navbar.
fn navbar_segment(
    theme: &Theme,
    content: Box<dyn Element>,
    padding_left: f32,
    padding_right: f32,
    side: Option<Side>,
) -> Box<dyn Element> {
    let mut container = Container::new(content)
        .with_padding_left(padding_left)
        .with_padding_right(padding_right);
    container = match side {
        Some(Side::Left) => {
            container.with_border(Border::left(1.0).with_border_color(theme.border()))
        }
        Some(Side::Right) => {
            container.with_border(Border::right(1.0).with_border_color(theme.border()))
        }
        None => container,
    };
    // NOTE: no separate hairline strip here (the old header had one): a
    // background Container sizes to its available width, which is
    // unbounded while the Resizable measures us — it would report (and
    // paint) infinite width and trip the scene validator. The bottom
    // border below draws the divider on our own finite rect instead.
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                ConstrainedBox::new(container.finish())
                    .with_height(HEADER_BAR_HEIGHT)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
    .finish()
}

/// Review split-button: optional dirty
/// stats face, Review face, chevron — joined by visible hairline dividers.
/// Stats open Source Control, Review opens the Review tab, chevron opens
/// the Push / Create PR menu.
fn render_review_button(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    main_mouse: MouseStateHandle,
    chevron_mouse: MouseStateHandle,
    stats_mouse: MouseStateHandle,
    review_active: bool,
    ship: &super::view::ReviewShipState,
    open_count: usize,
) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

    if ship.shows_stats() {
        let additions = ship.additions;
        let deletions = ship.deletions;
        let files = ship.dirty_files;
        let stats = Hoverable::new(stats_mouse, move |state| {
            let mut stats_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(render_icon(AppIcon::GitCompare, theme.accent, 13.0));
            if additions == 0 && deletions == 0 {
                stats_row = stats_row.with_child(
                    Text::new(format!("{files}"), ui_font, crate::theme::text_size(11.5))
                        .with_color(if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                );
            } else {
                stats_row = stats_row
                    .with_child(
                        Text::new(format!("+{additions}"), ui_font, crate::theme::text_size(11.5))
                            .with_color(theme.ui_green())
                            .finish(),
                    )
                    .with_child(
                        Text::new(
                            format!("\u{2212}{deletions}"),
                            ui_font,
                            crate::theme::text_size(11.5),
                        )
                        .with_color(theme.ui_red())
                        .finish(),
                    );
            }
            Container::new(stats_row.finish())
                .with_padding_left(10.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::SetProjectSidebarTab {
                    tab: super::project_sidebar::SidebarTab::SourceControl,
                },
            );
        })
        .finish();
        row = row.with_child(stats).with_child(divider(theme));
    }

    let main = Hoverable::new(main_mouse, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(
                    AppIcon::PullRequest,
                    if review_active || state.is_hovered() {
                        theme.accent
                    } else {
                        theme.sub_text()
                    },
                    13.0,
                ))
                .with_child(review_count_badge(
                    ui_font,
                    theme,
                    open_count,
                    review_active,
                ))
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(6.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        // SetProjectSidebarTab is an "open this destination" action: it
        // selects the Review tab and reverses a closed right panel.
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::SetProjectSidebarTab {
                tab: super::project_sidebar::SidebarTab::Review,
            },
        );
    })
    .finish();
    let chevron = Hoverable::new(chevron_mouse, move |state| {
        Container::new(render_icon(
            AppIcon::ChevronDown,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            11.0,
        ))
        .with_padding_left(4.0)
        .with_padding_right(9.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ToggleReviewMenu);
    })
    .finish();
    SavePosition::new(
        row.with_child(main)
            .with_child(divider(theme))
            .with_child(chevron)
            .finish(),
        REVIEW_MENU_TAG,
    )
    .finish()
}

/// Open-review count pill on the Review face (unresolved review threads;
/// hidden at zero). Short text, so it never affects the strip width.
fn review_count_badge(
    ui_font: FamilyId,
    theme: &Theme,
    open_count: usize,
    review_active: bool,
) -> Box<dyn Element> {
    if open_count == 0 {
        return Empty::new().finish();
    }
    let label = if open_count > 99 {
        "99+".to_string()
    } else {
        open_count.to_string()
    };
    Container::new(
        Text::new(label, ui_font, crate::theme::text_size(10.0))
            .with_color(if review_active {
                theme.background
            } else {
                theme.accent
            })
            .finish(),
    )
    .with_background_color(if review_active {
        theme.accent
    } else {
        theme.accent_overlay()
    })
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(1.0)
    .with_padding_bottom(1.0)
    .finish()
}

/// Full-navbar-height vertical hairline joining split-button segments.
fn divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(1.0)
            .with_height(HEADER_BAR_HEIGHT)
            .finish(),
    )
    .with_background_color(theme.outline())
    .finish()
}

pub const LAYOUT_MENU_TAG: &str = "workspace_layout_menu";
pub const REVIEW_MENU_TAG: &str = "review_menu_btn";

pub(crate) fn account_menu_row(
    label: &str,
    icon: AppIcon,
    action: WorkspaceAction,
    accent: bool,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let label = label.to_string();
    Hoverable::new(mouse_state, move |state| {
        let fg = if accent {
            theme_owned.accent
        } else {
            theme_owned.main_text()
        };
        let bg = if state.is_hovered() {
            theme_owned.surface_3()
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(icon, fg, 14.0))
                .with_child(Box::new(
                    Text::new(label.clone(), ui_font, crate::theme::text_size(12.0)).with_color(fg),
                ))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

fn render_sign_in_button(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    Hoverable::new(mouse_state, move |state| {
        let border = if state.is_hovered() {
            theme.accent
        } else {
            theme.outline()
        };
        let color = if state.is_hovered() {
            theme.accent
        } else {
            theme.main_text()
        };
        Container::new(Box::new(
            Text::new("Sign in".to_string(), ui_font, crate::theme::text_size(12.0)).with_color(color),
        ))
        .with_border(Border::all(1.0).with_border_color(border))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::SignInFromHeader);
    })
    .finish()
}

fn render_restart_update_button(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    version: String,
) -> Box<dyn Element> {
    Hoverable::new(mouse_state, move |state| {
        let background = if state.is_hovered() {
            theme.accent
        } else {
            theme.surface_2()
        };
        let foreground = if state.is_hovered() {
            theme.background
        } else {
            theme.accent
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(AppIcon::ArrowUp, foreground, 13.0))
                .with_child(Box::new(
                    Text::new(
                        "Restart to update".to_string(),
                        ui_font,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(foreground),
                ))
                .finish(),
        )
        .with_background_color(background)
        .with_border(Border::all(1.0).with_border_color(theme.accent))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        log::info!("[autoupdate] restart requested from header version={version}");
        event_ctx
            .dispatch_typed_action_for(workspace_view_id, WorkspaceAction::UpdateAppFromHeader);
    })
    .finish()
}

/// Bordered `+` action pinned directly after the workspace-tab strip: a
/// fixed header control outside the tab scroll state, so creating a tab
/// never requires scrolling. Full header height like the chips, with its
/// own outline so it reads as an action rather than another tab. It opens
/// the new-tab menu (a terminal, or the Dragent command line), anchored
/// under it by its saved position.
fn new_workspace_tab_button(
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    Box::new(
        Hoverable::new(mouse_state, move |state| {
            let hovered = state.is_hovered();
            let color = if hovered {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            ConstrainedBox::new(
                Container::new(Align::new(render_icon(AppIcon::Plus, color, 13.0)).finish())
                    .with_background_color(if hovered {
                        theme.surface_2()
                    } else {
                        ColorU::transparent_black()
                    })
                    .with_border(
                        Border::all(1.0).with_border_color(if hovered {
                            theme.accent
                        } else {
                            theme.outline()
                        }),
                    )
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .finish(),
            )
            // Square: as wide as it is tall.
            .with_width(HEADER_BAR_HEIGHT - 12.0)
            .with_height(HEADER_BAR_HEIGHT - 12.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::ToggleNewTabPicker,
            );
        }),
    )
}

/// Where the new-tab menu hangs from.
pub(crate) const NEW_TAB_BUTTON_POSITION: &str = "header_new_tab_btn";

fn toolbar_button(
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    icon: AppIcon,
    active: bool,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let color = if active || hovered {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let bg = if active {
            theme.fg_overlay_1()
        } else if hovered {
            theme.surface_1()
        } else {
            ColorU::transparent_black()
        };

        Container::new(icon_chip(icon, color))
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

/// The minimize / maximize-restore / close cluster for the custom Windows
/// titlebar, replicating the native caption controls of a decorated window.
#[cfg(windows)]
fn render_caption_buttons(
    theme: &Theme,
    workspace_view_id: EntityId,
    header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    window_maximized: bool,
) -> Box<dyn Element> {
    let maximize_icon = if window_maximized {
        AppIcon::Restore
    } else {
        AppIcon::Maximize
    };

    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(caption_button(
            theme,
            workspace_view_id,
            header_mouse_state("caption-minimize"),
            AppIcon::Minimize,
            WorkspaceAction::MinimizeWindow,
            false,
        ))
        .with_child(caption_button(
            theme,
            workspace_view_id,
            header_mouse_state("caption-maximize"),
            maximize_icon,
            WorkspaceAction::ToggleMaximizeWindow,
            false,
        ))
        .with_child(caption_button(
            theme,
            workspace_view_id,
            header_mouse_state("caption-close"),
            AppIcon::CaptionClose,
            WorkspaceAction::CloseWindow,
            true,
        ))
        .finish()
}

/// A single Windows caption button: flat, full header height, subtle hover —
/// and the close button turns the conventional red. Uses the default arrow
/// cursor (not a pointing hand), matching native caption buttons.
#[cfg(windows)]
fn caption_button(
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    icon: AppIcon,
    action: WorkspaceAction,
    is_close: bool,
) -> Box<dyn Element> {
    /// Native Windows 11 caption buttons are 46px wide.
    const CAPTION_BUTTON_WIDTH: f32 = 46.0;

    Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let (bg, fg) = if is_close && hovered {
            // Hover red of the native close button (Windows 11).
            (
                ColorU::new(196, 43, 28, 255),
                ColorU::new(255, 255, 255, 255),
            )
        } else if hovered {
            (theme.surface_1(), theme.main_text())
        } else {
            (ColorU::transparent_black(), theme.sub_text())
        };

        // 16px render of a caption glyph drawn at 15/24 of its viewBox ≈ the
        // native 10px glyph with a 1px hairline stroke (Windows 11 standard).
        ConstrainedBox::new(
            Container::new(Align::new(render_icon(icon, fg, ICON_SIZE_MD)).finish())
                .with_background_color(bg)
                .finish(),
        )
        .with_width(CAPTION_BUTTON_WIDTH)
        .with_height(HEADER_BAR_HEIGHT)
        .finish()
    })
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

fn icon_chip(icon: AppIcon, color: ColorU) -> Box<dyn Element> {
    Container::new(render_icon(icon, color, ICON_SIZE_MD))
        .with_padding_left(HEADER_ICON_PADDING)
        .with_padding_right(HEADER_ICON_PADDING)
        .with_padding_top(HEADER_ICON_PADDING)
        .with_padding_bottom(HEADER_ICON_PADDING)
        .finish()
}
