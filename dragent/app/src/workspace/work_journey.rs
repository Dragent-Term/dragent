//! Shared work lifecycle shown across agent-workspace presentations.
//!
//! Layouts are different lenses over one work item. This strip makes the
//! transition from intent to evidence visible instead of presenting Shape,
//! Coordinate, and Develop as unrelated screens.

use dragent_workspace_model::{BuiltInLayout, WorkStatus};
use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StageState {
    Complete,
    Current,
    Running,
    Queued,
}

pub(super) fn render(
    active_layout: BuiltInLayout,
    work_status: WorkStatus,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let approved = matches!(
        work_status,
        WorkStatus::ReadyToExecute
            | WorkStatus::Executing
            | WorkStatus::Reviewing
            | WorkStatus::Verifying
            | WorkStatus::NeedsUserApproval
            | WorkStatus::Completed
    );
    let executing = work_status == WorkStatus::Executing;
    let reviewing = matches!(
        work_status,
        WorkStatus::Reviewing | WorkStatus::Verifying | WorkStatus::NeedsUserApproval
    );
    let completed = work_status == WorkStatus::Completed;
    let stages = [
        (
            "Shape",
            if approved {
                "Outcome defined"
            } else {
                "Define outcome"
            },
            if approved {
                StageState::Complete
            } else {
                StageState::Current
            },
        ),
        (
            "Approve",
            if approved {
                "Plan approved"
            } else {
                "Lock the plan"
            },
            if approved {
                StageState::Complete
            } else {
                StageState::Queued
            },
        ),
        (
            "Coordinate",
            if approved {
                "Assign & launch"
            } else {
                "Assign roles"
            },
            if executing || reviewing || completed {
                StageState::Complete
            } else if approved || active_layout == BuiltInLayout::Coordinate {
                StageState::Current
            } else {
                StageState::Queued
            },
        ),
        (
            "Develop",
            if executing {
                "Agents working"
            } else {
                "Execute & test"
            },
            if reviewing || completed {
                StageState::Complete
            } else if executing {
                StageState::Running
            } else {
                StageState::Queued
            },
        ),
        (
            "Review",
            if completed {
                "Evidence accepted"
            } else {
                "Evidence & ship"
            },
            if completed {
                StageState::Complete
            } else if reviewing {
                StageState::Current
            } else {
                StageState::Queued
            },
        ),
    ];

    let mut flow = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0);
    for (index, (name, description, state)) in stages.into_iter().enumerate() {
        if index > 0 {
            flow = flow.with_child(render_icon(
                AppIcon::ArrowRight,
                theme.disabled_text(),
                11.0,
            ));
        }
        flow = flow
            .with_child(Expanded::new(1.0, stage(name, description, state, theme, font)).finish());
    }

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(7.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(label("WORK JOURNEY", theme.disabled_text(), 9.1, font))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(label(
                        "One work item · layouts change the lens, not the running work",
                        theme.sub_text(),
                        9.4,
                        font,
                    ))
                    .finish(),
            )
            .with_child(flow.finish())
            .finish(),
    )
    .with_padding_left(14.0)
    .with_padding_right(14.0)
    .with_padding_top(9.0)
    .with_padding_bottom(10.0)
    .with_background_color(theme.sidebar_background())
    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
    .finish()
}

fn stage(
    name: &str,
    description: &str,
    state: StageState,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let (color, state_label, icon) = match state {
        StageState::Complete => (theme.ui_green(), "Complete", AppIcon::Check),
        StageState::Current => (theme.accent, "Current", AppIcon::Agent),
        StageState::Running => (theme.link(), "Running", AppIcon::TerminalPanel),
        StageState::Queued => (theme.disabled_text(), "Queued", AppIcon::ChevronRight),
    };

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(3.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(icon, color, 11.0))
                    .with_child(label(name, theme.main_text(), 10.2, font))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(label(state_label, color, 8.8, font))
                    .finish(),
            )
            .with_child(label(description, theme.sub_text(), 9.0, font))
            .finish(),
    )
    .with_padding_left(9.0)
    .with_padding_right(9.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_background_color(if matches!(state, StageState::Current) {
        theme.accent_overlay()
    } else {
        theme.surface_1()
    })
    .with_border(
        Border::all(1.0).with_border_color(if matches!(state, StageState::Current) {
            color
        } else {
            theme.outline()
        }),
    )
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .finish()
}

fn label(text: &str, color: ColorU, size: f32, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}
