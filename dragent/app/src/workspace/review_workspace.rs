//! Canonical definition-of-done presentation.
//!
//! This view renders the model's deterministic completion projection. It does
//! not infer success from UI state and does not own a second copy of evidence.

use warpui::color::ColorU;
use warpui::elements::{
    ClippedScrollStateHandle, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty,
    Expanded, Flex, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::theme::Theme;
use crate::workspace::WorkspaceAction;
use dragent_workspace_model::{
    AcceptanceStatus, CompletionReport, CompletionRiskLevel, Evidence, EvidenceKind, WorkStatus,
};

const NAV_WIDTH: f32 = 218.0;
const MINIMUM_MAIN_PANE_WIDTH: f32 = 1110.0;
const MINIMUM_CONTENT_HEIGHT: f32 = 720.0;

#[allow(clippy::too_many_arguments)]
pub fn render(
    theme: &Theme,
    font: FamilyId,
    viewport_width: f32,
    viewport_height: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    work_title: Option<&str>,
    work_status: WorkStatus,
    report: Option<&CompletionReport>,
    evidence: &[Evidence],
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let content = match report {
        Some(report) => report_content(report, evidence, theme, font, workspace_view_id),
        None => empty_review(theme, font),
    };
    let scrolling_content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(super::work_journey::render(
            dragent_workspace_model::BuiltInLayout::Review,
            work_status,
            theme,
            font,
        ))
        .with_child(content)
        .finish();
    let canvas = super::work_canvas::render(
        theme,
        horizontal_scroll,
        vertical_scroll,
        (viewport_width - NAV_WIDTH).max(1.0),
        viewport_height.max(1.0),
        MINIMUM_MAIN_PANE_WIDTH,
        Some(MINIMUM_CONTENT_HEIGHT),
        scrolling_content,
    );
    let root = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            ConstrainedBox::new(super::shape_workspace::work_navigation(
                theme, font, work_title,
            ))
            .with_width(NAV_WIDTH)
            .finish(),
        )
        .with_child(Expanded::new(1.0, canvas).finish())
        .finish();
    Container::new(root)
        .with_background_color(theme.pane_surface())
        .finish()
}

/// Definition-of-done content hosted by the stable work-item editor tab. The
/// tab header owns workflow navigation, so this omits the full arrangement's
/// duplicate work navigator and journey strip.
pub(super) fn render_embedded(
    theme: &Theme,
    font: FamilyId,
    viewport_width: f32,
    viewport_height: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    report: Option<&CompletionReport>,
    evidence: &[Evidence],
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let content = match report {
        Some(report) => report_content(report, evidence, theme, font, workspace_view_id),
        None => empty_review(theme, font),
    };
    super::work_canvas::render(
        theme,
        horizontal_scroll,
        vertical_scroll,
        viewport_width.max(1.0),
        viewport_height.max(1.0),
        MINIMUM_MAIN_PANE_WIDTH,
        Some(MINIMUM_CONTENT_HEIGHT),
        content,
    )
}

fn report_content(
    report: &CompletionReport,
    evidence: &[Evidence],
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let (risk_label, risk_color) = match report.risk {
        CompletionRiskLevel::Clear => ("READY", theme.ui_green()),
        CompletionRiskLevel::NeedsEvidence => ("NEEDS EVIDENCE", theme.ui_yellow()),
        CompletionRiskLevel::Warning => ("READY WITH WAIVERS", theme.ui_yellow()),
        CompletionRiskLevel::Blocked => ("BLOCKED", theme.ui_red()),
    };
    let header = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(5.0)
                    .with_child(label("Definition of done", theme.main_text(), 15.0, font))
                    .with_child(label(&report.summary, theme.sub_text(), 9.4, font))
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(status_badge(risk_label, risk_color, theme, font))
            .finish(),
    )
    .with_uniform_padding(16.0)
    .with_background_color(with_alpha(risk_color, 0.07))
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish();

    let metrics = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(metric(
            "Tasks",
            &format!("{} / {}", report.completed_tasks, report.total_tasks),
            theme,
            font,
        ))
        .with_child(metric(
            "Criteria",
            &format!(
                "{} verified · {} waived",
                report.satisfied_criteria, report.waived_criteria
            ),
            theme,
            font,
        ))
        .with_child(metric(
            "Evidence",
            &report.evidence_count.to_string(),
            theme,
            font,
        ))
        .with_child(metric(
            "Attention",
            &report.open_attention.to_string(),
            theme,
            font,
        ))
        .finish();

    let criteria = criteria_panel(report, theme, font, workspace_view_id);
    let ledger = evidence_panel(evidence, theme, font);
    let columns = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(Expanded::new(1.3, criteria).finish())
        .with_child(Expanded::new(0.8, ledger).finish())
        .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(header)
        .with_child(metrics)
        .with_child(Expanded::new(1.0, columns).finish())
        .finish()
}

fn criteria_panel(
    report: &CompletionReport,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(section_title(
            "Acceptance criteria",
            &format!("{} total", report.criteria.len()),
            theme,
            font,
        ));
    for (index, criterion) in report.criteria.iter().enumerate() {
        let (status, color) = match criterion.status {
            AcceptanceStatus::Pending => ("PENDING", theme.ui_yellow()),
            AcceptanceStatus::Satisfied => ("VERIFIED", theme.ui_green()),
            AcceptanceStatus::Failed => ("FAILED", theme.ui_red()),
            AcceptanceStatus::Waived => ("WAIVED", theme.ui_yellow()),
        };
        let mut card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(label(
                        &format!("{:02}", index + 1),
                        theme.disabled_text(),
                        8.4,
                        font,
                    ))
                    .with_child(
                        Container::new(label(&criterion.description, theme.main_text(), 9.5, font))
                            .with_padding_left(8.0)
                            .finish(),
                    )
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(status_badge(status, color, theme, font))
                    .finish(),
            );
        if !criterion.evidence_summaries.is_empty() {
            for summary in &criterion.evidence_summaries {
                card = card.with_child(label(
                    &format!("Evidence · {summary}"),
                    theme.sub_text(),
                    8.4,
                    font,
                ));
            }
        } else if let Some(reason) = criterion.waiver_reason.as_deref() {
            card = card.with_child(label(
                &format!("Waiver · {reason}"),
                theme.sub_text(),
                8.4,
                font,
            ));
        } else {
            card = card.with_child(label(
                "No accepted evidence is linked yet.",
                theme.disabled_text(),
                8.4,
                font,
            ));
        }
        if matches!(
            criterion.status,
            AcceptanceStatus::Pending | AcceptanceStatus::Failed
        ) {
            card = card.with_child(waive_button(
                criterion.criterion_id,
                theme,
                font,
                workspace_view_id,
            ));
        }
        rows = rows.with_child(
            Container::new(card.finish())
                .with_uniform_padding(10.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::left(2.0).with_border_color(color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish(),
        );
    }
    panel(rows.finish(), theme)
}

fn evidence_panel(evidence: &[Evidence], theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(section_title(
            "Evidence ledger",
            &format!("{} records", evidence.len()),
            theme,
            font,
        ));
    if evidence.is_empty() {
        rows = rows.with_child(label(
            "Accepted changes, tests, reviews, and artifacts will appear here.",
            theme.disabled_text(),
            8.8,
            font,
        ));
    }
    for item in evidence.iter().rev().take(12) {
        rows = rows.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(4.0)
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(label(
                                evidence_kind(item.kind),
                                evidence_color(item.kind, theme),
                                8.1,
                                font,
                            ))
                            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                            .with_child(label("CANONICAL", theme.disabled_text(), 7.8, font))
                            .finish(),
                    )
                    .with_child(label(&item.summary, theme.main_text(), 8.9, font))
                    .with_child(label(&item.source, theme.disabled_text(), 7.9, font))
                    .finish(),
            )
            .with_uniform_padding(9.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish(),
        );
    }
    panel(rows.finish(), theme)
}

fn waive_button(
    criterion_id: dragent_workspace_model::AcceptanceCriterionId,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let theme_copy = *theme;
    Hoverable::new(
        super::execution_workspace::stable_mouse(format!("review:waive:{criterion_id:?}")),
        move |state| {
            Container::new(label(
                "Record explicit waiver",
                theme_copy.ui_yellow(),
                8.3,
                font,
            ))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_background_color(if state.is_hovered() {
                with_alpha(theme_copy.ui_yellow(), 0.10)
            } else {
                theme_copy.background
            })
            .with_border(Border::all(1.0).with_border_color(theme_copy.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::WaiveWorkspaceCriterion {
                acceptance_criterion_id: criterion_id,
            },
        );
    })
    .finish()
}

fn metric(title: &str, value: &str, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Expanded::new(
        1.0,
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(3.0)
                .with_child(label(title, theme.disabled_text(), 8.2, font))
                .with_child(label(value, theme.main_text(), 10.0, font))
                .finish(),
        )
        .with_uniform_padding(10.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish(),
    )
    .finish()
}

fn panel(child: Box<dyn Element>, theme: &Theme) -> Box<dyn Element> {
    Container::new(child)
        .with_uniform_padding(12.0)
        .with_background_color(theme.sidebar_background())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

fn section_title(title: &str, count: &str, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(label(title, theme.main_text(), 10.5, font))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(label(count, theme.sub_text(), 8.5, font))
        .finish()
}

fn empty_review(theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(7.0)
            .with_child(label(
                "Nothing to verify yet",
                theme.main_text(),
                14.0,
                font,
            ))
            .with_child(label(
                "Approve a shaped plan and let Coordinate produce accepted evidence.",
                theme.sub_text(),
                9.2,
                font,
            ))
            .finish(),
    )
    .with_uniform_padding(18.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish()
}

fn evidence_kind(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::FileChange => "FILE CHANGE",
        EvidenceKind::Command => "COMMAND",
        EvidenceKind::Test => "TEST",
        EvidenceKind::Review => "REVIEW",
        EvidenceKind::Artifact => "ARTIFACT",
        EvidenceKind::UserConfirmation => "CONFIRMATION",
        EvidenceKind::ExternalReference => "REFERENCE",
    }
}

fn evidence_color(kind: EvidenceKind, theme: &Theme) -> ColorU {
    match kind {
        EvidenceKind::FileChange | EvidenceKind::Test | EvidenceKind::Review => theme.ui_green(),
        EvidenceKind::Command | EvidenceKind::Artifact => theme.accent,
        EvidenceKind::UserConfirmation | EvidenceKind::ExternalReference => theme.ui_yellow(),
    }
}

fn status_badge(text: &str, color: ColorU, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Container::new(label(text, color, 8.0, font))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(color))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

fn label(text: &str, color: ColorU, size: f32, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}
