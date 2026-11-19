//! Editor-hosted presentation for one stable work item.
//!
//! Canonical work state remains owned by `WorkspaceState`; this module owns no
//! workflow truth. It is the first migration step away from app-wide Shape and
//! Coordinate screens toward one continuous editor-tab experience.

use super::execution_workspace::{
    action_button, attempt_color, attention_card, evidence_color, evidence_kind_label, format_time,
    message_kind_color, message_kind_label,
};
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

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ProjectMemoryView {
    pub work_item_id: WorkItemId,
    pub title: String,
    pub status: WorkStatus,
    pub outcome: Option<String>,
    pub summary: Option<String>,
    pub resolved_decisions: Vec<(String, String, Option<String>)>,
}

#[derive(Clone, Debug)]
pub(super) struct ShapeComposerModelRow {
    pub id: String,
    pub label: String,
    pub effort_wire: String,
    pub selected: bool,
}

/// One coordination gate with its satisfied state — rendered as a checklist
/// above Start Coordination so a disabled button always explains itself.
#[derive(Clone, Debug)]
pub(super) struct ReadinessCheck {
    pub label: String,
    pub done: bool,
}

/// Unified work-item type scale (base px at the default Typography setting).
/// Everything still flows through `scaled()`, so the user's Typography
/// sliders keep applying on top. Conversation mono text already sat at
/// 13.5–14.5px; the Plan chrome sat at 9.5–11px — this scale lifts the small
/// end and gives every role one step.
pub(super) const WI_TITLE: f32 = 15.0;
pub(super) const WI_HEADING: f32 = 14.0;
pub(super) const WI_SUBHEAD: f32 = 13.0;
pub(super) const WI_BODY: f32 = 12.5;
pub(super) const WI_SMALL: f32 = 11.5;
pub(super) const WI_BUTTON: f32 = 11.0;
pub(super) const WI_CAPTION: f32 = 10.5;

pub(super) struct WorkItemSurfaceProps<'a> {
    pub title: &'a str,
    pub project_scope: &'a str,
    pub status: WorkStatus,
    pub active_view: WorkItemView,
    pub mono: FamilyId,
    pub proposal: Option<&'a ShapingProposal>,
    pub work_brief: Option<&'a WorkBrief>,
    pub records: Option<&'a ShapingRecords>,
    pub artifacts: &'a [ArtifactRef],
    pub project_memory: &'a [ProjectMemoryView],
    /// This work item's shared knowledge ledger (decisions, discoveries,
    /// root causes, handoffs) — written by the Principal and every worker,
    /// read by all of them. Newest first.
    pub knowledge: &'a [dragent_workspace_model::ShapingMemoryNote],
    pub plan: Option<&'a Plan>,
    pub proposed_roles: &'a [ProposedRoleView],
    pub composer: ViewHandle<InputEditor>,
    pub composer_has_text: bool,
    pub composer_attachments: &'a [crate::agent::attachments::LoadedAttachment],
    pub composer_notice: Option<&'a str>,
    pub voice_state: crate::voice::VoiceUiState,
    pub voice_spin: usize,
    pub principal_pending: bool,
    /// Composer send/stop busy state: true for a live Shaping turn OR a live
    /// coordination turn. `principal_pending` above stays Shaping-only (it
    /// gates the Approve Shape row); this is the wider signal the composer
    /// itself uses, since the input stays usable either way.
    pub composer_busy: bool,
    /// Live status while the Principal handles an agent's request during
    /// coordination — informational only; the composer stays sendable.
    pub principal_activity: Option<String>,
    /// Principal composer model picker open state — same global-model picker
    /// as the terminal agent input. Branch UI is deliberately omitted.
    pub shape_model_menu_open: bool,
    pub shape_models: Vec<ShapeComposerModelRow>,
    pub shape_selected_label: String,
    pub shape_model_locked: bool,
    /// Work Item Flow step 3 gate: true once the user clicked Approve Shape.
    pub shape_approved: bool,
    /// Work Item Flow: true once a Gather turn declared requirements complete.
    /// Unlocks the Approve Shape button; reset on every new submit.
    pub requirements_complete: bool,
    /// Whether a shape proposal already exists (refinement stays permitted).
    pub has_proposal: bool,
    /// The work item's agent conversation (terminal agent controller +
    /// block renderer). `None` only before the item's first turn.
    pub agent_view: Option<ViewHandle<crate::agent::conversation_view::AgentConversationView>>,
    pub coordination_ready: bool,
    /// A coordination or launch request is in flight. The Start button
    /// renders Working and ignores taps until the authority answers.
    pub coordination_pending: bool,
    /// Latest coordination failure (approve/launch), rendered inline under
    /// the Start Coordination button so Plan-view failures are never silent.
    pub coordination_error: Option<&'a str>,
    /// Startup recovery notice (interrupted sessions found at launch).
    /// Informational only — rendered neutrally, never as a failure.
    pub recovery_notice: Option<&'a str>,
    pub record_edit_target: Option<&'a ShapeRecordEditTarget>,
    /// Armed (confirm-stage) remove target for two-step deletion.
    pub remove_armed: Option<ShapeRecordEditTarget>,
    pub record_editor: ViewHandle<InputEditor>,
    pub record_edit_has_text: bool,
    pub record_edit_pending: bool,
    pub record_edit_error: Option<&'a str>,
    /// In-flight per-role Approve action (contract id) and its error — the
    /// Plan view Team section approves contracts inline without leaving.
    pub role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    pub role_action_error: Option<&'a str>,
    /// Per-task model catalog for the Plan task cards. Same source as the
    /// Shape role cards — never a second list. Locked (free tier) renders
    /// Auto as server-resolved and non-editable.
    pub task_model_routes: &'a [super::shape_workspace::ModelRouteOption],
    pub task_model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    pub task_model_picker_locked: bool,
    pub vertical_scroll: ClippedScrollStateHandle,
    /// Horizontal scroll for the view-route strip, so a pane narrower than
    /// the six routes scrolls them instead of forcing the header wider.
    pub view_tabs_scroll: ClippedScrollStateHandle,
    pub coordinate_viewport_width: f32,
    pub coordinate_viewport_height: f32,
    pub coordinate_horizontal_scroll: ClippedScrollStateHandle,
    pub coordinate_vertical_scroll: ClippedScrollStateHandle,
    pub activity_scroll: ClippedScrollStateHandle,
    pub activity_dock_state: ResizableStateHandle,
    pub active_execution_count: usize,
    pub attempts: &'a [ExecutionAttempt],
    pub agent_messages: &'a [(AgentMessage, AgentMessageRouting)],
    pub communication_threads: &'a [CommunicationThreadSummary],
    pub evidence: &'a [Evidence],
    pub attention: &'a [AttentionItem],
    pub checkpoints: &'a [WorkspaceCheckpointSummary],
    pub checkpoint_notice: Option<&'a str>,
    pub execution_notice: Option<&'a str>,
    pub changes_scroll: ClippedScrollStateHandle,
    pub review_horizontal_scroll: ClippedScrollStateHandle,
    pub review_vertical_scroll: ClippedScrollStateHandle,
    pub completion_report: Option<&'a CompletionReport>,
    /// Selected runtime agent for the Agent tab. Stable identities resolved
    /// by the view from canonical state; None renders the empty state.
    pub selected_agent_task: Option<dragent_workspace_model::PlanTaskId>,
    pub selected_agent_attempt: Option<dragent_workspace_model::ExecutionAttemptId>,
    pub agent_activity: &'a [crate::agent::workspace_execution::WorkspaceExecutionActivity],
    /// The selected attempt's live (or SQLite-restored) agent conversation —
    /// the same terminal-agent block list the Principal uses. `None` only for
    /// attempts that predate controller-run execution.
    pub attempt_agent_view:
        Option<ViewHandle<crate::agent::conversation_view::AgentConversationView>>,
    /// The selected attempt runs on an agent CLI in a terminal of its own:
    /// its name, and its sidebar workspace while that's open.
    pub cli_worker: Option<(String, Option<usize>)>,
    /// Full-history expansion for the agent timeline. Presentation only.
    pub agent_timeline_expanded: bool,
    pub agent_timeline_scroll: ClippedScrollStateHandle,
    /// Coordination gates with satisfied states. Empty when coordination is
    /// not applicable; the Plan view renders these as a checklist so a
    /// disabled Start Coordination always explains what remains.
    pub readiness_checks: &'a [ReadinessCheck],
}

/// Re-exported area modules; existing `super::work_item_surface::…` paths
/// keep working. Canonical homes are the surface_* modules.
pub(crate) use super::surface_changes::*;
pub(crate) use super::surface_composer::*;
pub(crate) use super::surface_header::*;
pub(crate) use super::surface_plan::*;
pub(crate) use super::surface_text::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordination_notice_prefers_error_then_blocker_then_nothing() {
        use dragent_workspace_model::WorkStatus;
        let checks = vec![
            ReadinessCheck {
                label: "Shaped proposal".into(),
                done: true,
            },
            ReadinessCheck {
                label: "Approve role contracts (0/2 approved)".into(),
                done: false,
            },
        ];
        assert_eq!(
            coordination_notice_content(
                Some("billing preflight failed"),
                false,
                WorkStatus::Planning,
                &checks,
                None,
            ),
            Some((
                "Coordination failed",
                "billing preflight failed".to_string(),
                CoordinationNoticeTone::Error
            ))
        );
        assert_eq!(
            coordination_notice_content(None, false, WorkStatus::Planning, &checks, None),
            Some((
                "Waiting to coordinate",
                "Next: Approve role contracts (0/2 approved)".to_string(),
                CoordinationNoticeTone::Waiting
            ))
        );
        assert_eq!(
            coordination_notice_content(None, true, WorkStatus::Planning, &checks, None),
            None
        );
        assert_eq!(
            coordination_notice_content(None, false, WorkStatus::ReadyToExecute, &checks, None),
            Some((
                "Waiting to coordinate",
                "Next: Approve role contracts (0/2 approved)".to_string(),
                CoordinationNoticeTone::Waiting
            )),
            "blockers stay visible even after coordination"
        );
        // Green checks but not plannable: the lifecycle stage is the blocker.
        let done_checks = vec![ReadinessCheck {
            label: "Shaped proposal".into(),
            done: true,
        }];
        let (title, body, tone) =
            coordination_notice_content(None, false, WorkStatus::Understanding, &done_checks, None)
                .unwrap();
        assert_eq!(title, "Waiting to coordinate");
        assert!(body.contains("still being understood"));
        assert!(matches!(tone, CoordinationNoticeTone::Waiting));
        // Planning with no listed blockers: nothing to say (unreachable in
        // practice — an empty issue list with a team and plan is approvable).
        assert_eq!(
            coordination_notice_content(None, false, WorkStatus::Planning, &[], None),
            None
        );
        // Recovery surfaces only when nothing more urgent does.
        assert_eq!(
            coordination_notice_content(
                None,
                true,
                WorkStatus::Planning,
                &done_checks,
                Some("1 interrupted execution was preserved"),
            ),
            Some((
                "Previous session recovered",
                "1 interrupted execution was preserved".to_string(),
                CoordinationNoticeTone::Recovery
            ))
        );
        assert_eq!(
            coordination_notice_content(
                Some("boom"),
                true,
                WorkStatus::Planning,
                &done_checks,
                Some("recovery"),
            ),
            Some((
                "Coordination failed",
                "boom".to_string(),
                CoordinationNoticeTone::Error
            )),
            "a fresh failure outranks the recovery note"
        );
        // Executing work tells the user where to follow it.
        assert_eq!(
            coordination_notice_content(None, true, WorkStatus::Executing, &done_checks, None),
            Some((
                "Coordination is underway",
                "Open the Coordinate view to follow progress.".to_string(),
                CoordinationNoticeTone::Waiting
            ))
        );
    }
}
