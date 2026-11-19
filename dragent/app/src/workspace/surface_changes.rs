//! Work-item changes and agent timeline views.
//!
//! Moved verbatim; same behavior.

//! Editor-hosted presentation for one stable work item.
//!
//! Canonical work state remains owned by `WorkspaceState`; this module owns no
//! workflow truth. It is the first migration step away from app-wide Shape and
//! Coordinate screens toward one continuous editor-tab experience.

use super::execution_workspace::{
    action_button, attempt_color, attention_card, evidence_color, evidence_kind_label, format_time,
    message_kind_color, message_kind_label,
};
use super::work_item_surface::*;
use dragent_workspace_model::{
    AgentMessage, AgentMessageKind, AgentMessageRouting, ArtifactRef, AttentionItem,
    AttentionStatus, CommunicationThreadSummary, CompletionReport, Evidence, EvidenceKind,
    ExecutionAttempt, ExecutionAttemptStatus, ExecutionFileChangeKind, Plan, ShapingProposal,
    ShapingRecords, WorkBrief, WorkItemId, WorkStatus,
};
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisSize, ParentElement, ResizableStateHandle, ScrollbarWidth, SelectableArea,
    SelectionUpdateArgs, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::shimmering_text::{
    ShimmerConfig, ShimmeringTextElement, ShimmeringTextStateHandle,
};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::agent::controller::block::TodoListItem;
use crate::agent::workspace_execution::WorkspaceCheckpointSummary;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::shape_workspace::ProposedRoleView;
use super::{InputEditor, ShapeRecordEditTarget, WorkItemView, WorkspaceAction};

pub(crate) fn changes_view(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let changed_attempts = props
        .attempts
        .iter()
        .filter(|attempt| attempt.change_summary.is_some())
        .collect::<Vec<_>>();
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(4.0)
                        .with_child(heading("Agent changes", theme, font))
                        .with_child(body_text(
                            "Canonical change sets grouped by the agent attempt and plan task that produced them.",
                            theme.sub_text(),
                            font,
                        ))
                        .finish(),
                )
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(status_pill(
                    &format!("{} change sets", changed_attempts.len()),
                    theme.accent,
                    theme,
                    font,
                ))
                .finish(),
        );
    if let Some(notice) = props.execution_notice {
        content = content.with_child(
            Container::new(body_text(notice, theme.ui_yellow(), font))
                .with_uniform_padding(12.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.ui_yellow()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish(),
        );
    }
    if changed_attempts.is_empty() {
        content = content.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(6.0)
                    .with_child(heading("No agent changes yet", theme, font))
                    .with_child(body_text(
                        "Change sets appear after a writable execution finishes and its isolated Git delta is accepted.",
                        theme.disabled_text(),
                        font,
                    ))
                    .finish(),
            )
            .with_uniform_padding(18.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    }
    for attempt in changed_attempts {
        content = content.with_child(change_attempt_card(
            attempt,
            props.plan,
            props.proposed_roles,
            theme,
            font,
            workspace_view_id,
        ));
    }
    ClippedScrollable::vertical(
        props.changes_scroll.clone(),
        Container::new(content.finish())
            .with_uniform_padding(24.0)
            .finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.pane_surface().into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

/// Inspectable runtime agent screen: read-only audit view over the durable
/// canonical event log for one execution attempt.
///
/// Stable identities (work item, task, attempt) select the content; every row
/// renders attributed canonical state — assigned task, execution status,
/// messages, evidence, attention, checkpoints, and file changes — never a UI
/// guess. Verbose bodies stay fully visible (bounded canonical records) while
/// the item list caps at a recent window with an explicit expander.
/// Opening or closing this tab touches no execution state.
pub(crate) fn agent_timeline_view(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    const TIMELINE_WINDOW: usize = 30;
    let attempt = props
        .selected_agent_attempt
        .and_then(|id| props.attempts.iter().find(|attempt| attempt.id == id))
        .or_else(|| {
            if props.selected_agent_attempt.is_some() {
                return None;
            }
            props.selected_agent_task.and_then(|task_id| {
                props
                    .attempts
                    .iter()
                    .filter(|attempt| attempt.task_id == task_id)
                    .max_by_key(|attempt| attempt.attempt_number)
            })
        });
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0);
    let Some(attempt) = attempt else {
        // Either nothing is selected, or a task with no attempts yet. The
        // task case still renders its assigned context so the click that led
        // here always lands somewhere meaningful.
        let waiting_task = props.selected_agent_task.and_then(|task_id| {
            props
                .plan
                .iter()
                .flat_map(|plan| plan.tasks.iter())
                .find(|task| task.id == task_id)
        });
        content = content.with_child(heading("Agent", theme, font));
        if let Some(task) = waiting_task {
            let role_name = task
                .assigned_contract_id
                .and_then(|contract_id| {
                    props
                        .proposed_roles
                        .iter()
                        .find(|role| role.work_contract_id == contract_id)
                })
                .map(|role| role.name.as_str())
                .unwrap_or("Unassigned");
            content = content
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(heading(role_name, theme, font))
                        .with_child(status_pill(
                            &format!("{:?}", task.status),
                            theme.disabled_text(),
                            theme,
                            font,
                        ))
                        .finish(),
                )
                .with_child(body_text(
                    &format!("Task: {}", task.title),
                    theme.sub_text(),
                    font,
                ))
                .with_child(
                    Container::new(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_spacing(6.0)
                            .with_child(heading("No execution attempts yet", theme, font))
                            .with_child(body_text(
                                "The scheduler has not dispatched this task. Its timeline — tool calls, messages, evidence, and checkpoints — appears here once the first attempt starts.",
                                theme.sub_text(),
                                font,
                            ))
                            .finish(),
                    )
                    .with_uniform_padding(16.0)
                    .with_background_color(theme.surface_1())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                    .finish(),
                );
        } else {
            content = content.with_child(
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(6.0)
                        .with_child(heading("No runtime agent selected", theme, font))
                        .with_child(body_text(
                            "Open an agent from a role card or an activity row in Coordinate. The timeline renders that agent's durable history; selecting it never starts or alters execution.",
                            theme.sub_text(),
                            font,
                        ))
                        .finish(),
                )
                .with_uniform_padding(16.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            );
        }
        return ClippedScrollable::vertical(
            props.agent_timeline_scroll.clone(),
            Container::new(content.finish())
                .with_uniform_padding(24.0)
                .finish(),
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.pane_surface().into(),
        )
        .with_overlayed_scrollbar()
        .finish();
    };

    let task = props
        .plan
        .iter()
        .flat_map(|plan| plan.tasks.iter())
        .find(|task| task.id == attempt.task_id);
    let role_name = task
        .and_then(|task| task.assigned_contract_id)
        .and_then(|contract_id| {
            props
                .proposed_roles
                .iter()
                .find(|role| role.work_contract_id == contract_id)
        })
        .map(|role| role.name.as_str())
        .unwrap_or("Unassigned");
    let task_title = task
        .map(|task| task.title.as_str())
        .unwrap_or("Unknown task");
    let live = attempt.status == ExecutionAttemptStatus::Running;
    content = content
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(heading(role_name, theme, font))
                .with_child(status_pill(
                    &format!("{:?}", attempt.status),
                    attempt_color(attempt.status, theme),
                    theme,
                    font,
                ))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(body_text(
                    if live {
                        "Live execution"
                    } else {
                        "Durable history"
                    },
                    if live {
                        theme.link()
                    } else {
                        theme.disabled_text()
                    },
                    font,
                ))
                .finish(),
        )
        .with_child(body_text(
            &format!(
                "Task: {task_title} · Attempt {} · {} · started {}",
                attempt.attempt_number,
                attempt.model_route,
                format_time(attempt.started_at_unix_ms),
            ),
            theme.sub_text(),
            font,
        ));
    if let Some(summary) = attempt.finished_at_unix_ms.map(format_time) {
        content = content.with_child(body_text(
            &format!("Finished {summary}"),
            theme.disabled_text(),
            font,
        ));
    }
    if let Some(resolution) = &attempt.review_resolution {
        content = content.with_child(body_text(
            &format!("Review resolution: {resolution:?}"),
            theme.sub_text(),
            font,
        ));
    }
    if let Some(summary) = &attempt.change_summary {
        let mut files = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(body_text(
                &format!("{} changed file(s)", summary.files.len()),
                theme.main_text(),
                font,
            ));
        for file in &summary.files {
            let change_glyph = match file.kind {
                ExecutionFileChangeKind::Added => "+",
                ExecutionFileChangeKind::Deleted => "-",
                ExecutionFileChangeKind::Modified => "~",
                ExecutionFileChangeKind::Renamed => "»",
                ExecutionFileChangeKind::Binary => "●",
            };
            files = files.with_child(body_text(
                &format!(
                    "{change_glyph} {} (+{} -{})",
                    file.relative_path.to_string_lossy(),
                    file.added_lines.unwrap_or(0),
                    file.deleted_lines.unwrap_or(0),
                ),
                theme.sub_text(),
                font,
            ));
        }
        content = content.with_child(
            Container::new(files.finish())
                .with_uniform_padding(12.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
        );
    }

    // Controller-run attempt: the agent's own conversation IS the timeline —
    // the same block list (tool steps, thinking, approvals, questions, error
    // cards, final reply) a terminal agent shows, live while it runs and
    // restored from SQLite afterwards. Header stays fixed; the pane scrolls.
    if let Some(agent) = props.attempt_agent_view.as_ref() {
        let header = Container::new(content.finish())
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_top(20.0)
            .with_padding_bottom(12.0)
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish();
        return Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header)
                .with_child(Expanded::new(1.0, ChildView::new(agent).finish()).finish())
                .finish(),
        )
        .with_background_color(theme.pane_surface())
        .finish();
    }

    // Agent CLI attempt: the agent works in its own terminal. Point there;
    // its messages to the team follow in the durable history below.
    if let Some((agent, workspace)) = props.cli_worker.as_ref() {
        let mut card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.0)
            .with_child(body_text(
                &format!(
                    "{agent} works on this task in a terminal of its own, in the task's checkout. \
                     It reaches the team with `dragent work`; its messages and findings are below."
                ),
                theme.sub_text(),
                font,
            ));
        if let Some(workspace_id) = workspace {
            card = card.with_child(action_button(
                "Open its workspace",
                theme.accent,
                WorkspaceAction::SelectProjectWorkspace {
                    workspace_id: *workspace_id,
                },
                *theme,
                font,
                workspace_view_id,
            ));
        }
        content = content.with_child(
            Container::new(card.finish())
                .with_uniform_padding(12.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
        );
    }

    let mut activity = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(heading("Runtime activity", theme, font))
        .with_child(body_text(
            "Recent output and tool progress from this existing execution (last 256 entries). Durable records follow below.",
            theme.disabled_text(),
            font,
        ));
    if props.agent_activity.is_empty() {
        activity = activity.with_child(body_text(
            if live {
                "Waiting for runtime output."
            } else {
                "No runtime activity retained in this session. See durable history below."
            },
            theme.sub_text(),
            font,
        ));
    }
    for entry in props.agent_activity {
        activity = activity.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(4.0)
                    .with_child(body_text(
                        &format!("#{} · {}", entry.sequence, entry.kind),
                        theme.link(),
                        font,
                    ))
                    .with_child(body_text(&entry.text, theme.main_text(), props.mono))
                    .finish(),
            )
            .with_uniform_padding(12.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    }
    content = content.with_child(activity.finish());

    // Chronological timeline merged from durable canonical records only:
    // messages addressed to this attempt, task evidence, attempt attention,
    // and checkpoints. No hidden reasoning is rendered; verbose records are
    // bounded canonical bodies shown in full.
    struct TimelineRow {
        at: i64,
        sort: u8,
        title: String,
        body: String,
        color: ColorU,
    }
    let mut rows: Vec<TimelineRow> = Vec::new();
    rows.push(TimelineRow {
        at: attempt.started_at_unix_ms,
        sort: 0,
        title: "Execution started".to_string(),
        body: format!(
            "Attempt {} dispatched on {}",
            attempt.attempt_number, attempt.model_route
        ),
        color: theme.link(),
    });
    for (message, routing) in props.agent_messages {
        let belongs = match routing.execution_attempt_id {
            Some(id) => id == attempt.id,
            None => routing.task_id == Some(attempt.task_id),
        };
        if !belongs {
            continue;
        }
        let mut body = message.body.clone();
        if let Some(finding) = &message.review_finding {
            body = format!(
                "{}\nVerdict: {:?} · {} finding(s)",
                finding.summary,
                finding.verdict,
                finding.findings.len()
            );
        }
        let scoped = if routing.execution_attempt_id == Some(attempt.id) {
            ""
        } else {
            " (task-scoped)"
        };
        rows.push(TimelineRow {
            at: message.created_at_unix_ms,
            sort: 1,
            title: format!("{}{}", message_kind_label(message.kind), scoped),
            body,
            color: message_kind_color(message.kind, theme),
        });
    }
    for evidence in props.evidence {
        if evidence.task_id != Some(attempt.task_id) {
            continue;
        }
        rows.push(TimelineRow {
            at: evidence.created_at_unix_ms,
            sort: 2,
            title: format!("Evidence · {}", evidence_kind_label(evidence.kind)),
            body: format!("{} — {}", evidence.summary, evidence.source),
            color: evidence_color(evidence.kind, theme),
        });
    }
    for item in props.attention {
        if item.execution_attempt_id != Some(attempt.id) {
            continue;
        }
        let state = match item.status {
            AttentionStatus::Open | AttentionStatus::Acknowledged => "open",
            AttentionStatus::Resolved => "resolved",
            AttentionStatus::Dismissed => "dismissed",
        };
        let mut body = format!("[{state}] {}", item.summary);
        if let Some(resolution) = &item.resolution {
            body = format!("{body}\nResolution: {resolution}");
        }
        rows.push(TimelineRow {
            at: item.created_at_unix_ms,
            sort: 3,
            title: "Attention".to_string(),
            body,
            color: theme.ui_yellow(),
        });
    }
    for checkpoint in props.checkpoints {
        if checkpoint.attempt_id != attempt.id {
            continue;
        }
        rows.push(TimelineRow {
            at: attempt.updated_at_unix_ms,
            sort: 4,
            title: "Checkpoint".to_string(),
            body: format!(
                "{} · {} file(s) · +{} -{}",
                checkpoint.task_title,
                checkpoint.changed_files,
                checkpoint.added_lines,
                checkpoint.deleted_lines,
            ),
            color: theme.ui_green(),
        });
    }
    if let Some(finished_at) = attempt.finished_at_unix_ms {
        rows.push(TimelineRow {
            at: finished_at,
            sort: 5,
            title: format!("Execution {:?}", attempt.status),
            body: String::new(),
            color: attempt_color(attempt.status, theme),
        });
    }
    rows.sort_by(|left, right| (left.at, left.sort).cmp(&(right.at, right.sort)));
    let total = rows.len();
    let windowed: &[TimelineRow] = if props.agent_timeline_expanded || total <= TIMELINE_WINDOW {
        &rows
    } else {
        &rows[total - TIMELINE_WINDOW..]
    };
    let mut timeline = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(heading("Timeline", theme, font))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(body_text(
                    &format!("showing {} of {total} events", windowed.len()),
                    theme.disabled_text(),
                    font,
                ))
                .finish(),
        );
    for row in windowed {
        timeline = timeline.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(4.0)
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(body_text(
                                &format_time(row.at),
                                theme.disabled_text(),
                                font,
                            ))
                            .with_child(body_text(&row.title, row.color, font))
                            .finish(),
                    )
                    .with_child(body_text(&row.body, theme.main_text(), font))
                    .finish(),
            )
            .with_uniform_padding(12.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    }
    if total > TIMELINE_WINDOW {
        timeline = timeline.with_child(action_button(
            if props.agent_timeline_expanded {
                "Show recent"
            } else {
                "Show full history"
            },
            theme.accent,
            WorkspaceAction::ToggleWorkspaceAgentTimelineExpanded,
            *theme,
            font,
            workspace_view_id,
        ));
    }
    content = content.with_child(timeline.finish()).with_child(body_text(
        "Read-only inspection: this tab renders durable canonical history and live status. Opening or closing it never restarts or alters execution.",
        theme.disabled_text(),
        font,
    ));
    ClippedScrollable::vertical(
        props.agent_timeline_scroll.clone(),
        Container::new(content.finish())
            .with_uniform_padding(24.0)
            .finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.pane_surface().into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

pub(crate) fn change_attempt_card(
    attempt: &ExecutionAttempt,
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let summary = attempt
        .change_summary
        .as_ref()
        .expect("caller filters attempts without a change summary");
    let role_name = roles
        .iter()
        .find(|role| role.work_contract_id == attempt.contract_id)
        .map(|role| role.name.as_str())
        .unwrap_or("Worker agent");
    let task_title = plan
        .and_then(|plan| plan.tasks.iter().find(|task| task.id == attempt.task_id))
        .map(|task| task.title.as_str())
        .unwrap_or("Unknown task");
    let additions = summary
        .files
        .iter()
        .filter_map(|file| file.added_lines)
        .sum::<u64>();
    let deletions = summary
        .files
        .iter()
        .filter_map(|file| file.deleted_lines)
        .sum::<u64>();
    let review =
        attempt
            .review_resolution
            .map_or("Awaiting review", |resolution| match resolution {
                dragent_workspace_model::ExecutionReviewResolution::Accepted => "Accepted",
                dragent_workspace_model::ExecutionReviewResolution::Integrated => "Integrated",
                dragent_workspace_model::ExecutionReviewResolution::RevisionRequested => {
                    "Revision requested"
                }
                dragent_workspace_model::ExecutionReviewResolution::Discarded => "Discarded",
            });
    let attempt_id = attempt.id;
    let theme_copy = *theme;
    let open = Hoverable::new(
        super::execution_workspace::stable_mouse(format!("changes:open:{attempt_id:?}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(render_icon(AppIcon::Eye, theme_copy.accent_text(), 11.0))
                    .with_child(
                        Text::new("Open structured diff".to_string(), font, crate::theme::text_size(WI_BUTTON))
                            .with_color(theme_copy.accent_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_right(9.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(if state.is_hovered() {
                theme_copy.link()
            } else {
                theme_copy.accent
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenWorkspaceAttemptChanges { attempt_id },
        );
    })
    .finish();
    let mut files = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0);
    for file in &summary.files {
        let kind = match file.kind {
            ExecutionFileChangeKind::Added => "A",
            ExecutionFileChangeKind::Modified => "M",
            ExecutionFileChangeKind::Deleted => "D",
            ExecutionFileChangeKind::Renamed => "R",
            ExecutionFileChangeKind::Binary => "B",
        };
        files = files.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(body_text(kind, theme.accent, font))
                .with_child(
                    Expanded::new(
                        1.0,
                        body_text(
                            &file.relative_path.to_string_lossy(),
                            theme.sub_text(),
                            font,
                        ),
                    )
                    .finish(),
                )
                .with_child(body_text(
                    &format!(
                        "+{} −{}",
                        file.added_lines.unwrap_or(0),
                        file.deleted_lines.unwrap_or(0)
                    ),
                    theme.disabled_text(),
                    font,
                ))
                .finish(),
        );
    }
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_spacing(3.0)
                            .with_child(heading(task_title, theme, font))
                            .with_child(body_text(
                                &format!(
                                    "{role_name} · attempt {} · {} · {review}",
                                    attempt.attempt_number, attempt.model_route
                                ),
                                theme.sub_text(),
                                font,
                            ))
                            .finish(),
                    )
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(body_text(
                        &format!("{} files · +{additions} −{deletions}", summary.files.len()),
                        theme.sub_text(),
                        font,
                    ))
                    .with_child(Container::new(open).with_padding_left(10.0).finish())
                    .finish(),
            )
            .with_child(files.finish())
            .finish(),
    )
    .with_uniform_padding(14.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish()
}
