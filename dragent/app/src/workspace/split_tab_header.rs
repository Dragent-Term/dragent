//! Per-tab header shown at the top of each pane in a split-tab horizontal split.

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, SavePosition, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::theme::{Theme, PANEL_HEADER_HEIGHT, UI_FONT_SIZE};

use super::WorkspaceAction;

/// `SavePosition` tag prefix anchoring a split pane's ⋮ share menu.
pub fn split_share_menu_tag(tab_id: usize) -> String {
    format!("split_share_menu_{tab_id}")
}

pub fn render_split_tab_header(
    ui_font: FamilyId,
    theme: &Theme,
    icon: Box<dyn Element>,
    runtime_status: Option<&str>,
    agent_status: &super::worktree_status::WorktreeAgentStatus,
    title: &str,
    workspace_view_id: EntityId,
    tab_id: usize,
    is_active: bool,
    close_mouse_state: MouseStateHandle,
    split_mouse_state: MouseStateHandle,
    share_mouse_state: MouseStateHandle,
    split_direction: super::pane_layout::SplitDirection,
    show_terminal_actions: bool,
) -> Box<dyn Element> {
    let title_color = if is_active {
        theme.main_text()
    } else {
        theme.sub_text()
    };
    let status_idle = matches!(
        agent_status,
        super::worktree_status::WorktreeAgentStatus::Idle
    );
    let is_working = matches!(
        agent_status,
        super::worktree_status::WorktreeAgentStatus::Working { .. }
    );
    let status_color = agent_status.color(theme);

    let mut title_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.0)
        .with_child(icon)
        .with_child(
            Expanded::new(
                1.0,
                Text::new(
                    title.to_string(),
                    ui_font,
                    crate::theme::text_size(UI_FONT_SIZE),
                )
                .with_color(title_color)
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
            )
            .finish(),
        );
    // Live agent dot: glowing ring while working, solid dot for
    // needs-input / failed / review. Always visible (not hover-gated) so
    // the right-hand split/share/close cluster sits beside a live status.
    if !status_idle {
        let dot: Box<dyn Element> = if is_working {
            Container::new(
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(5.0)
                        .with_height(5.0)
                        .finish(),
                )
                .with_background_color(status_color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .finish(),
            )
            .with_background_color(ColorU::new(
                status_color.r,
                status_color.g,
                status_color.b,
                40,
            ))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(2.5)
            .with_padding_right(2.5)
            .with_padding_top(2.5)
            .with_padding_bottom(2.5)
            .finish()
        } else {
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(status_color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish()
        };
        title_row = title_row.with_child(dot);
    }
    if let Some(status) = runtime_status {
        let text_color = if status_idle {
            theme.sub_text()
        } else {
            status_color
        };
        title_row = title_row.with_child(
            Text::new(status.to_string(), ui_font, crate::theme::text_size(11.0))
                .with_color(text_color)
                .finish(),
        );
    }
    let title_row = title_row.finish();

    let close_btn = Hoverable::new(close_mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        Container::new(render_icon(
            AppIcon::XClose,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            ICON_SIZE_MD,
        ))
        .with_uniform_padding(4.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::CloseTab { tab_id });
    })
    .finish();

    // One split button per pane: divides this pane along the given
    // direction. The caller passes the opposite of the pane's parent split,
    // so layouts alternate as they nest. Horizontal arranges left|right,
    // vertical stacks top/bottom.
    let (split_icon, split_action) = match split_direction {
        super::pane_layout::SplitDirection::Horizontal => (
            AppIcon::SplitHorizontal,
            WorkspaceAction::SplitTabHorizontal { tab_id },
        ),
        super::pane_layout::SplitDirection::Vertical => (
            AppIcon::SplitVertical,
            WorkspaceAction::SplitTabVertical { tab_id },
        ),
    };
    let split_btn = Hoverable::new(split_mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        Container::new(render_icon(
            split_icon,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            ICON_SIZE_MD,
        ))
        .with_uniform_padding(4.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(workspace_view_id, split_action.clone());
    })
    .finish();

    // ⋮ share menu per pane, so split terminals share independently.
    let share_btn = Hoverable::new(share_mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        Container::new(render_icon(
            AppIcon::DotsVertical,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            ICON_SIZE_MD,
        ))
        .with_uniform_padding(4.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::ToggleSplitShareMenu { tab_id },
        );
    })
    .finish();
    let share_btn = SavePosition::new(share_btn, &split_share_menu_tag(tab_id)).finish();

    let controls = if show_terminal_actions {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(split_btn)
            .with_child(share_btn)
            .with_child(close_btn)
            .finish()
    } else {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(close_btn)
            .finish()
    };

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(Shrinkable::new(1.0, title_row).finish())
        .with_child(controls)
        .finish();

    let mut header = Container::new(
        Container::new(row)
            .with_padding_left(10.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .finish();

    if !is_active {
        header = Container::new(header)
            .with_background_color(ColorU::new(0, 0, 0, 30))
            .finish();
    }

    ConstrainedBox::new(header)
        .with_height(PANEL_HEADER_HEIGHT)
        .finish()
}
