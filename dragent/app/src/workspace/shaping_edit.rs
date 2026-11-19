//! Shaping record editing methods.
//!
//! Responsible for: shaping record edits (roles, tasks, contracts),
//! plan revisions and structural changes.
//! Blocks here are impl WorkspaceView: same type, same behavior.

use super::share_sheet;
use super::viewer_bar;
use super::{InputEditor, InputEditorEvent, InputMode, LanguageNavigationCapabilities};
use crate::agent::workspace_execution::{
    next_workspace_launch_action, next_workspace_retry_wait_ms, WorkspaceExecutionRuntime,
    WorkspaceLaunchAction, WorkspaceLaunchPolicy, WorkspacePipelineAction,
    WorkspaceResolvedToolAction, WorkspaceToolRequest,
};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::settings::AccountStore;
use crate::settings::{SettingsEvent, SettingsView};
use crate::setup::UiSettings;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{
    Theme, DEFAULT_LEFT_PANEL_WIDTH, DEFAULT_RIGHT_PANEL_WIDTH, HEADER_BAR_HEIGHT, MIN_PANEL_WIDTH,
    UI_FONT_SIZE, WORKSPACE_PADDING,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_session_protocol::{AccessLevel, ParticipantId, Role};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    drag_resize_handle, resizable_state_handle, resizable_state_handle_initialized, Align,
    ChildAnchor, ChildView, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, DragBarSide, DragResizeAxis, DragResizeElement,
    DragResizeHandle, DropTarget, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, ResizableStateHandle,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::Cursor;
use warpui_core::platform::FilePickerConfiguration;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::process_ext::NoWindow;
use dragent_editor::{
    char_index_to_lsp_position, lsp_position_to_char_index, LanguageRequest, LanguageRequestKind,
    TextPosition, TextRange,
};
use dragent_language_services::{
    LanguageServerCatalog, LanguageServerKey, LanguageServiceClient, LanguageServiceClientConfig,
    LanguageServiceEvent, SyncedDocument,
};

use super::pane::WorkspacePane;
use super::pane_layout::{
    LayoutNode, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
use super::project_sidebar::{
    find_git_root, ProjectSidebarModel, ProjectSidebarView, ReviewPrState,
};
// Re-exported so existing `super::view::ReviewShipState` paths keep working
// during the migration; new code should import from `review_pr_service`.
// Re-exported so existing `super::…` and test paths keep working during the
// migration; new code should import from `project_naming`.
// Glob import during migration: shaping helpers moved to `shape_parse`
// verbatim; call sites below are unchanged.
// Glob import during migration: execution delivery helpers moved verbatim.
use super::execution_delivery::*;
// Glob import during migration: language navigation helpers moved verbatim.
use super::language_nav::*;
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_build::*;
use super::shape_parse::*;
use super::sidebar::color_from_hex;
use super::view_overlays::ExplorerPrompt;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus, WorkItemView,
    WorkspaceAction, DEFAULT_SESSION_TITLE,
};
use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessage as WorkspaceAgentMessage,
    AgentMessageId, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, BuiltInLayout, ChargingAuthorization,
    ChargingMode, CommunicationThread, ExecutionAttemptId, LayoutAxis, LayoutChromeState,
    LayoutCollection, LayoutInstance, LayoutPanelState, LayoutScope, MessageDeliveryState, Project,
    ProjectDocumentIndex, SurfaceDescriptor, SurfaceId, SurfaceKind, SurfaceLayoutNode,
    SurfaceRegistry, ThreadId, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
    WorkspaceState, MAX_LAYOUT_FOCUS_HISTORY,
};
use left_panel::LeftPanelView;
use persistence::{PersistedSession, WindowChrome, WriterHandles};
use uuid::Uuid;

use super::view::WorkspaceView;

impl WorkspaceView {
    pub(crate) fn shaping_proposed_role_views(
        &self,
        artifacts: &[dragent_workspace_model::ArtifactRef],
        records: Option<&dragent_workspace_model::ShapingRecords>,
    ) -> Vec<super::shape_workspace::ProposedRoleView> {
        self.workspace_state
            .latest_proposed_team()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        let definition = self
                            .workspace_state
                            .role_definition(role.role_definition_id)?;
                        let contract = self.workspace_state.work_contract(role.work_contract_id)?;
                        Some(super::shape_workspace::ProposedRoleView {
                            work_contract_id: contract.id,
                            version: contract.version,
                            kind: definition.kind,
                            name: definition.name.clone(),
                            mission: contract.mission.clone(),
                            rationale: role.rationale.clone(),
                            capabilities: contract
                                .capabilities
                                .iter()
                                .map(|capability| capability.capability.clone())
                                .collect(),
                            tools: contract.tools.clone(),
                            write_scope: contract
                                .write_scope
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect(),
                            approval_required: contract
                                .permissions
                                .iter()
                                .any(|permission| permission.requires_approval),
                            permissions: contract.permissions.clone(),
                            execution_location: contract.execution_location,
                            status: contract.status,
                            maximum_input_tokens: contract.budget.maximum_input_tokens,
                            maximum_output_tokens: contract.budget.maximum_output_tokens,
                            maximum_cost_micros: contract.budget.maximum_cost_micros,
                            maximum_duration_ms: contract.budget.maximum_duration_ms,
                            maximum_tool_calls: contract.budget.maximum_tool_calls,
                            expected_outputs: contract.expected_outputs.clone(),
                            input_artifacts: contract
                                .input_artifact_ids
                                .iter()
                                .filter_map(|id| {
                                    artifacts.iter().find(|artifact| artifact.id == *id).map(
                                        |artifact| {
                                            artifact.relative_path.to_string_lossy().into_owned()
                                        },
                                    )
                                })
                                .collect(),
                            success_criteria: contract
                                .success_criteria
                                .iter()
                                .filter_map(|id| {
                                    records?
                                        .acceptance_criteria
                                        .iter()
                                        .find(|criterion| criterion.id == *id)
                                        .map(|criterion| criterion.description.clone())
                                })
                                .collect(),
                            escalation_rules: contract.escalation_rules.clone(),
                            preferred_model_route: contract.preferred_model_route.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn start_shape_record_edit(
        &mut self,
        target: super::ShapeRecordEditTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending {
            return;
        }
        let records = self.workspace_state.shaping_records_projection();
        let artifacts = self.workspace_state.artifacts();
        let plan = self.workspace_state.latest_structured_plan();
        let roles = self
            .workspace_state
            .latest_proposed_team()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .role_definition(role.role_definition_id)
                            .map(|definition| (definition.name.clone(), role.work_contract_id))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let text = match target {
            super::ShapeRecordEditTarget::WorkBriefOutcome => self
                .workspace_state
                .latest_work_brief()
                .map(|brief| brief.outcome.clone())
                .or_else(|| {
                    self.workspace_state
                        .latest_shaping_proposal()
                        .map(|proposal| proposal.outcome.clone())
                }),
            super::ShapeRecordEditTarget::WorkBriefSummary => self
                .workspace_state
                .latest_work_brief()
                .map(|brief| brief.summary.clone())
                .or_else(|| {
                    self.workspace_state
                        .latest_shaping_proposal()
                        .map(|proposal| proposal.plan_summary.clone())
                }),
            super::ShapeRecordEditTarget::NewRequirement
            | super::ShapeRecordEditTarget::NewDecision
            | super::ShapeRecordEditTarget::NewAcceptanceCriterion => Some(String::new()),
            super::ShapeRecordEditTarget::Requirement(id) => records.as_ref().and_then(|records| {
                records
                    .requirements
                    .iter()
                    .find(|record| record.id == id)
                    .map(|record| record.summary.clone())
            }),
            super::ShapeRecordEditTarget::Decision(id) => records.as_ref().and_then(|records| {
                records
                    .decisions
                    .iter()
                    .find(|record| record.id == id)
                    .map(|record| record.question.clone())
            }),
            super::ShapeRecordEditTarget::DecisionResolution(id) => {
                records.as_ref().and_then(|records| {
                    records
                        .decisions
                        .iter()
                        .find(|record| record.id == id)
                        .map(|record| record.resolution.clone().unwrap_or_default())
                })
            }
            super::ShapeRecordEditTarget::AcceptanceCriterion(id) => {
                records.as_ref().and_then(|records| {
                    records
                        .acceptance_criteria
                        .iter()
                        .find(|record| record.id == id)
                        .map(|record| record.description.clone())
                })
            }
            super::ShapeRecordEditTarget::NewArtifact => Some("path=\nkind=Other".into()),
            super::ShapeRecordEditTarget::Artifact(id) => artifacts
                .iter()
                .find(|artifact| artifact.id == id)
                .map(|artifact| {
                    format!(
                        "path={}\nkind={:?}",
                        artifact.relative_path.to_string_lossy(),
                        artifact.kind
                    )
                }),
            super::ShapeRecordEditTarget::NewPlanTask => roles.first().and_then(|(role, _)| {
                records
                    .as_ref()?
                    .acceptance_criteria
                    .first()
                    .map(|criterion| {
                        format!(
                            "title=\ndescription=\nrole={role}\ncriterion={}",
                            criterion.description
                        )
                    })
            }),
            super::ShapeRecordEditTarget::PlanTask(id) => plan.and_then(|plan| {
                plan.tasks.iter().find(|task| task.id == id).map(|task| {
                    format_plan_task_edit(
                        task,
                        plan,
                        &roles,
                        records
                            .as_ref()
                            .map(|records| records.acceptance_criteria.as_slice())
                            .unwrap_or_default(),
                    )
                })
            }),
            super::ShapeRecordEditTarget::RoleMission(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| contract.mission.clone()),
            super::ShapeRecordEditTarget::RoleCapabilities(id) => {
                self.workspace_state.work_contract(id).map(|contract| {
                    contract
                        .capabilities
                        .iter()
                        .map(|capability| capability.capability.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            super::ShapeRecordEditTarget::RoleTools(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| contract.tools.join("\n")),
            super::ShapeRecordEditTarget::RoleWriteScope(id) => {
                self.workspace_state.work_contract(id).map(|contract| {
                    contract
                        .write_scope
                        .iter()
                        .map(|path| path.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            super::ShapeRecordEditTarget::RolePermissions(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| format_role_permissions(&contract.permissions)),
            super::ShapeRecordEditTarget::RoleExpectedOutputs(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| contract.expected_outputs.join("\n")),
            super::ShapeRecordEditTarget::RoleInputArtifacts(id) => {
                self.workspace_state.work_contract(id).map(|contract| {
                    contract
                        .input_artifact_ids
                        .iter()
                        .filter_map(|id| {
                            artifacts
                                .iter()
                                .find(|artifact| artifact.id == *id)
                                .map(|artifact| artifact.relative_path.to_string_lossy())
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            super::ShapeRecordEditTarget::RoleSuccessCriteria(id) => {
                self.workspace_state.work_contract(id).map(|contract| {
                    records
                        .as_ref()
                        .map(|records| {
                            contract
                                .success_criteria
                                .iter()
                                .filter_map(|id| {
                                    records
                                        .acceptance_criteria
                                        .iter()
                                        .find(|criterion| criterion.id == *id)
                                        .map(|criterion| criterion.description.as_str())
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                })
            }
            super::ShapeRecordEditTarget::RoleEscalationRules(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| contract.escalation_rules.join("\n")),
            super::ShapeRecordEditTarget::RoleExecutionLocation(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| match contract.execution_location {
                    dragent_workspace_model::ExecutionLocation::Local => "Local".into(),
                    dragent_workspace_model::ExecutionLocation::Cloud => "Cloud".into(),
                    dragent_workspace_model::ExecutionLocation::Hybrid => "Hybrid".into(),
                }),
            super::ShapeRecordEditTarget::RoleModelRoute(id) => {
                self.workspace_state.work_contract(id).map(|contract| {
                    contract
                        .preferred_model_route
                        .clone()
                        .unwrap_or_else(|| "Auto".into())
                })
            }
            super::ShapeRecordEditTarget::RoleBudget(id) => self
                .workspace_state
                .work_contract(id)
                .map(|contract| format_shape_budget(&contract.budget)),
        };
        let Some(text) = text else {
            return;
        };
        self.shape_record_edit_target = Some(target);
        self.shape_record_edit_text = text.clone();
        self.shape_record_edit_error = None;
        self.shape_remove_armed = None;
        let editor = self.shape_record_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text(&text, editor_ctx);
            editor.request_focus(editor_ctx);
        });
        ctx.notify();
    }

    pub(crate) fn cancel_shape_record_edit(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_record_edit_pending {
            return;
        }
        self.shape_record_edit_target = None;
        self.shape_record_edit_text.clear();
        self.shape_record_edit_error = None;
        self.shape_remove_armed = None;
        ctx.notify();
    }

    pub(crate) fn save_shape_record_edit(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(target) = self.shape_record_edit_target else {
            return;
        };
        let text = self.shape_record_edit_text.trim().to_string();
        let may_be_empty = matches!(
            target,
            super::ShapeRecordEditTarget::RoleCapabilities(_)
                | super::ShapeRecordEditTarget::RoleTools(_)
                | super::ShapeRecordEditTarget::RoleWriteScope(_)
                | super::ShapeRecordEditTarget::RoleExpectedOutputs(_)
                | super::ShapeRecordEditTarget::RoleInputArtifacts(_)
                | super::ShapeRecordEditTarget::RoleEscalationRules(_)
                | super::ShapeRecordEditTarget::RoleModelRoute(_)
                | super::ShapeRecordEditTarget::RoleBudget(_)
        );
        if (!may_be_empty && text.is_empty()) || self.shape_record_edit_pending {
            return;
        }
        self.shape_remove_armed = None;
        let Some(work) = self.workspace_state.active_work().cloned() else {
            return;
        };
        let records = self.workspace_state.shaping_records_projection();
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        let now = chrono::Utc::now().timestamp_millis();
        let provenance = dragent_workspace_model::Provenance {
            source: dragent_workspace_model::ProvenanceSource::ManualEdit,
            recorded_by: actor.id,
            recorded_at_unix_ms: now,
        };
        let payload = match target {
            super::ShapeRecordEditTarget::WorkBriefOutcome
            | super::ShapeRecordEditTarget::WorkBriefSummary => {
                let Some(proposal) = self.workspace_state.latest_shaping_proposal() else {
                    return;
                };
                let mut brief = self
                    .workspace_state
                    .latest_work_brief()
                    .cloned()
                    .unwrap_or_else(|| {
                        dragent_workspace_model::WorkBrief::from_shaping_proposal(
                            work.id, proposal, actor.id, now,
                        )
                    });
                if self.workspace_state.latest_work_brief().is_some() {
                    brief.version = brief.version.saturating_add(1);
                }
                match target {
                    super::ShapeRecordEditTarget::WorkBriefOutcome => brief.outcome = text,
                    super::ShapeRecordEditTarget::WorkBriefSummary => brief.summary = text,
                    _ => unreachable!(),
                }
                brief.updated_at_unix_ms = now;
                brief.updated_by = actor.id;
                WorkspaceCommandPayload::UpsertWorkBrief { work_brief: brief }
            }
            super::ShapeRecordEditTarget::NewRequirement => {
                let Some(mut records) = records.clone() else {
                    return;
                };
                records
                    .requirements
                    .push(dragent_workspace_model::Requirement {
                        id: dragent_workspace_model::RequirementId::new(),
                        work_item_id: work.id,
                        summary: text,
                        details: String::new(),
                        priority: dragent_workspace_model::RequirementPriority::Normal,
                        status: dragent_workspace_model::StructuredRecordStatus::Draft,
                        provenance: vec![provenance],
                        created_at_unix_ms: now,
                        updated_at_unix_ms: now,
                    });
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::Requirement(id) => {
                let Some(mut record) = records
                    .as_ref()
                    .and_then(|records| records.requirements.iter().find(|record| record.id == id))
                    .cloned()
                else {
                    return;
                };
                record.summary = text;
                record.updated_at_unix_ms = now;
                record.provenance.push(provenance);
                WorkspaceCommandPayload::UpsertRequirement {
                    requirement: record,
                }
            }
            super::ShapeRecordEditTarget::NewDecision => {
                let Some(mut records) = records.clone() else {
                    return;
                };
                records.decisions.push(dragent_workspace_model::Decision {
                    id: dragent_workspace_model::DecisionId::new(),
                    work_item_id: work.id,
                    question: text,
                    resolution: None,
                    rationale: None,
                    status: dragent_workspace_model::DecisionStatus::Open,
                    provenance: vec![provenance],
                    decided_by: None,
                    created_at_unix_ms: now,
                    updated_at_unix_ms: now,
                });
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::Decision(id) => {
                let Some(mut record) = records
                    .as_ref()
                    .and_then(|records| records.decisions.iter().find(|record| record.id == id))
                    .cloned()
                else {
                    return;
                };
                record.question = text;
                record.updated_at_unix_ms = now;
                record.provenance.push(provenance);
                WorkspaceCommandPayload::UpsertDecision { decision: record }
            }
            super::ShapeRecordEditTarget::DecisionResolution(id) => {
                let Some(mut record) = records
                    .as_ref()
                    .and_then(|records| records.decisions.iter().find(|record| record.id == id))
                    .cloned()
                else {
                    return;
                };
                record.resolution = Some(text);
                record.status = dragent_workspace_model::DecisionStatus::Resolved;
                record.decided_by = Some(actor.id);
                record.updated_at_unix_ms = now;
                record.provenance.push(provenance);
                WorkspaceCommandPayload::UpsertDecision { decision: record }
            }
            super::ShapeRecordEditTarget::NewAcceptanceCriterion => {
                let Some(mut records) = records.clone() else {
                    return;
                };
                records
                    .acceptance_criteria
                    .push(dragent_workspace_model::AcceptanceCriterion {
                        id: dragent_workspace_model::AcceptanceCriterionId::new(),
                        work_item_id: work.id,
                        description: text,
                        status: dragent_workspace_model::AcceptanceStatus::Pending,
                        evidence_ids: Vec::new(),
                        waiver_reason: None,
                        waived_by: None,
                        provenance: vec![provenance],
                        created_at_unix_ms: now,
                        updated_at_unix_ms: now,
                    });
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::AcceptanceCriterion(id) => {
                let Some(mut record) = records
                    .as_ref()
                    .and_then(|records| {
                        records
                            .acceptance_criteria
                            .iter()
                            .find(|record| record.id == id)
                    })
                    .cloned()
                else {
                    return;
                };
                record.description = text;
                record.updated_at_unix_ms = now;
                record.provenance.push(provenance);
                WorkspaceCommandPayload::UpsertAcceptanceCriterion {
                    acceptance_criterion: record,
                }
            }
            super::ShapeRecordEditTarget::NewArtifact
            | super::ShapeRecordEditTarget::Artifact(_) => {
                let (relative_path, kind) = match parse_shape_artifact(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        self.shape_record_edit_error = Some(error);
                        ctx.notify();
                        return;
                    }
                };
                let mut artifacts = self.workspace_state.artifacts();
                let id = match target {
                    super::ShapeRecordEditTarget::Artifact(id) => id,
                    _ => dragent_workspace_model::ArtifactId::new(),
                };
                let artifact = dragent_workspace_model::ArtifactRef {
                    id,
                    work_item_id: work.id,
                    relative_path,
                    kind,
                };
                if let Some(existing) = artifacts.iter_mut().find(|item| item.id == id) {
                    *existing = artifact;
                } else {
                    artifacts.push(artifact);
                }
                WorkspaceCommandPayload::ReplaceArtifacts { artifacts }
            }
            super::ShapeRecordEditTarget::NewPlanTask => {
                let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
                    return;
                };
                let Some(records) = records.as_ref() else {
                    self.shape_record_edit_error =
                        Some("Acceptance criteria are not available".into());
                    ctx.notify();
                    return;
                };
                let Some(team) = self.workspace_state.latest_proposed_team() else {
                    self.shape_record_edit_error = Some("A proposed team is required".into());
                    ctx.notify();
                    return;
                };
                let roles = team
                    .roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .role_definition(role.role_definition_id)
                            .map(|definition| (definition.name.clone(), role.work_contract_id))
                    })
                    .collect::<Vec<_>>();
                let task_id = dragent_workspace_model::PlanTaskId::new();
                plan.tasks.push(dragent_workspace_model::PlanTask {
                    id: task_id,
                    plan_id: plan.id,
                    title: String::new(),
                    description: String::new(),
                    status: dragent_workspace_model::PlanTaskStatus::Proposed,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: None,
                });
                let revised_task = match parse_plan_task_edit(
                    &text,
                    task_id,
                    &plan,
                    &roles,
                    &records.acceptance_criteria,
                ) {
                    Ok(task) => task,
                    Err(error) => {
                        self.shape_record_edit_error = Some(error);
                        ctx.notify();
                        return;
                    }
                };
                *plan.tasks.last_mut().expect("new plan task is present") = revised_task;
                let result = if work_status_is_post_coordination(work.status) {
                    append_plan_task_payload(&self.workspace_state, plan, now)
                } else {
                    build_plan_revision_payload(&self.workspace_state, plan, actor.id, now)
                };
                match result {
                    Ok(payload) => payload,
                    Err(error) => {
                        self.shape_record_edit_error = Some(error);
                        ctx.notify();
                        return;
                    }
                }
            }
            super::ShapeRecordEditTarget::PlanTask(id) => {
                let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
                    return;
                };
                let Some(records) = records.as_ref() else {
                    self.shape_record_edit_error =
                        Some("Acceptance criteria are not available".into());
                    ctx.notify();
                    return;
                };
                let Some(team) = self.workspace_state.latest_proposed_team() else {
                    self.shape_record_edit_error = Some("A proposed team is required".into());
                    ctx.notify();
                    return;
                };
                let roles = team
                    .roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .role_definition(role.role_definition_id)
                            .map(|definition| (definition.name.clone(), role.work_contract_id))
                    })
                    .collect::<Vec<_>>();
                let revised_task = match parse_plan_task_edit(
                    &text,
                    id,
                    &plan,
                    &roles,
                    &records.acceptance_criteria,
                ) {
                    Ok(task) => task,
                    Err(error) => {
                        self.shape_record_edit_error = Some(error);
                        ctx.notify();
                        return;
                    }
                };
                let Some(task) = plan.tasks.iter_mut().find(|task| task.id == id) else {
                    return;
                };
                *task = revised_task;
                match build_plan_revision_payload(&self.workspace_state, plan, actor.id, now) {
                    Ok(payload) => payload,
                    Err(error) => {
                        self.shape_record_edit_error = Some(error);
                        ctx.notify();
                        return;
                    }
                }
            }
            super::ShapeRecordEditTarget::RoleMission(id)
            | super::ShapeRecordEditTarget::RoleCapabilities(id)
            | super::ShapeRecordEditTarget::RoleTools(id)
            | super::ShapeRecordEditTarget::RoleWriteScope(id)
            | super::ShapeRecordEditTarget::RolePermissions(id)
            | super::ShapeRecordEditTarget::RoleExpectedOutputs(id)
            | super::ShapeRecordEditTarget::RoleInputArtifacts(id)
            | super::ShapeRecordEditTarget::RoleSuccessCriteria(id)
            | super::ShapeRecordEditTarget::RoleEscalationRules(id)
            | super::ShapeRecordEditTarget::RoleExecutionLocation(id)
            | super::ShapeRecordEditTarget::RoleModelRoute(id)
            | super::ShapeRecordEditTarget::RoleBudget(id) => {
                let Some(mut contract) = self.workspace_state.work_contract(id).cloned() else {
                    return;
                };
                match target {
                    super::ShapeRecordEditTarget::RoleMission(_) => contract.mission = text,
                    super::ShapeRecordEditTarget::RoleCapabilities(_) => {
                        contract.capabilities = split_shape_list(&text)
                            .into_iter()
                            .map(|capability| dragent_workspace_model::CapabilityGrant {
                                capability,
                                scope: None,
                            })
                            .collect();
                    }
                    super::ShapeRecordEditTarget::RoleTools(_) => {
                        contract.tools = split_shape_list(&text);
                    }
                    super::ShapeRecordEditTarget::RoleWriteScope(_) => {
                        let scopes = split_shape_list(&text);
                        contract.write_scope = scopes.iter().map(PathBuf::from).collect();
                        contract.permissions = if scopes.is_empty() {
                            vec![dragent_workspace_model::PermissionGrant {
                                permission: "project_read".into(),
                                scope: None,
                                requires_approval: false,
                            }]
                        } else {
                            let mut permissions = vec![dragent_workspace_model::PermissionGrant {
                                permission: "project_read".into(),
                                scope: None,
                                requires_approval: false,
                            }];
                            permissions.extend(scopes.into_iter().map(|scope| {
                                dragent_workspace_model::PermissionGrant {
                                    permission: "project_write".into(),
                                    scope: Some(scope),
                                    requires_approval: true,
                                }
                            }));
                            permissions
                        };
                    }
                    super::ShapeRecordEditTarget::RolePermissions(_) => {
                        contract.permissions =
                            match parse_role_permissions(&text, &contract.write_scope) {
                                Ok(permissions) => permissions,
                                Err(error) => {
                                    self.shape_record_edit_error = Some(error);
                                    ctx.notify();
                                    return;
                                }
                            };
                    }
                    super::ShapeRecordEditTarget::RoleExpectedOutputs(_) => {
                        contract.expected_outputs = split_shape_list(&text);
                    }
                    super::ShapeRecordEditTarget::RoleInputArtifacts(_) => {
                        contract.input_artifact_ids =
                            match parse_input_artifacts(&text, &self.workspace_state.artifacts()) {
                                Ok(artifacts) => artifacts,
                                Err(error) => {
                                    self.shape_record_edit_error = Some(error);
                                    ctx.notify();
                                    return;
                                }
                            };
                    }
                    super::ShapeRecordEditTarget::RoleSuccessCriteria(_) => {
                        let Some(records) = records.as_ref() else {
                            self.shape_record_edit_error =
                                Some("Acceptance criteria are not available".into());
                            ctx.notify();
                            return;
                        };
                        contract.success_criteria =
                            match parse_success_criteria(&text, &records.acceptance_criteria) {
                                Ok(criteria) => criteria,
                                Err(error) => {
                                    self.shape_record_edit_error = Some(error);
                                    ctx.notify();
                                    return;
                                }
                            };
                    }
                    super::ShapeRecordEditTarget::RoleEscalationRules(_) => {
                        contract.escalation_rules = split_shape_list(&text);
                    }
                    super::ShapeRecordEditTarget::RoleExecutionLocation(_) => {
                        contract.execution_location = match parse_execution_location(&text) {
                            Ok(location) => location,
                            Err(error) => {
                                self.shape_record_edit_error = Some(error);
                                ctx.notify();
                                return;
                            }
                        };
                    }
                    super::ShapeRecordEditTarget::RoleModelRoute(_) => {
                        let old_route = contract.preferred_model_route.clone();
                        contract.preferred_model_route = match parse_preferred_model_route(&text) {
                            Ok(route) => route,
                            Err(error) => {
                                self.shape_record_edit_error = Some(error);
                                ctx.notify();
                                return;
                            }
                        };
                        // A route change alters execution inputs. If the plan
                        // is already approved and coordination has not started,
                        // the approval must be invalidated through the
                        // authority (plan back to Draft) once this save lands.
                        // Auto<->Auto saves are no-ops and keep approval.
                        self.shape_plan_reset_after_edit = self
                            .workspace_state
                            .latest_structured_plan()
                            .is_some_and(|plan| {
                                plan.status == dragent_workspace_model::PlanStatus::Approved
                            })
                            && self.workspace_state.active_work().is_some_and(|work| {
                                work.status == dragent_workspace_model::WorkStatus::Planning
                            })
                            && dragent_workspace_model::contract_route_change_invalidates_approval(
                                old_route.as_deref(),
                                contract.preferred_model_route.as_deref(),
                            );
                    }
                    super::ShapeRecordEditTarget::RoleBudget(_) => {
                        contract.budget = match parse_shape_budget(&text) {
                            Ok(budget) => budget,
                            Err(error) => {
                                self.shape_record_edit_error = Some(error);
                                ctx.notify();
                                return;
                            }
                        };
                    }
                    _ => unreachable!(),
                }
                contract.version = contract.version.saturating_add(1);
                contract.status = dragent_workspace_model::WorkContractStatus::Proposed;
                contract.updated_at_unix_ms = now;
                contract.updated_by = actor.id;
                WorkspaceCommandPayload::UpsertWorkContract {
                    work_contract: contract,
                }
            }
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: self.workspace_state.project().id,
            work_item_id: work.id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload,
        };
        self.submit_shape_revision_command(command, Some(target), ctx);
    }

    pub(crate) fn submit_shape_revision_command(
        &mut self,
        command: WorkspaceCommandEnvelope,
        expected_target: Option<super::ShapeRecordEditTarget>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.shape_record_edit_pending = true;
        self.shape_record_edit_error = None;
        self.shape_record_edit_epoch = self.shape_record_edit_epoch.wrapping_add(1);
        let epoch = self.shape_record_edit_epoch;
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .accept_workspace_command(command)
                .map_err(|error| format!("Could not save this edit: {error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                if view.shape_record_edit_epoch != epoch
                    || view.shape_record_edit_target != expected_target
                {
                    return;
                }
                view.shape_record_edit_pending = false;
                match result {
                    Ok(events) if !events.is_empty() => {
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        let mut projected_all = true;
                        for event in events {
                            view.workspace_canonical_revision =
                                view.workspace_canonical_revision.max(event.sequence);
                            if let Some(projected) =
                                dragent_session_protocol::workspace::project_envelope_event(
                                    event,
                                    principal_id,
                                )
                            {
                                view.workspace_state
                                    .record(projected.timestamp_unix_ms, projected.kind);
                            } else {
                                projected_all = false;
                            }
                        }
                        if projected_all {
                            view.shape_record_edit_target = None;
                            view.shape_record_edit_text.clear();
                            view.shape_record_edit_error = None;
                            view.shape_coordination_error = None;
                            if view.shape_plan_reset_after_edit {
                                view.shape_plan_reset_after_edit = false;
                                view.reset_approved_plan_to_draft(ctx);
                            }
                        } else {
                            view.shape_plan_reset_after_edit = false;
                            let error = "The authority returned an unexpected edit event".into();
                            view.shape_record_edit_error = Some(error);
                        }
                    }
                    Ok(_) => {
                        view.shape_plan_reset_after_edit = false;
                        let error = "The authority returned no edit events".to_string();
                        view.shape_record_edit_error = Some(error.clone());
                        if expected_target.is_none() {
                            view.shape_coordination_error = Some(error);
                        }
                    }
                    Err(error) => {
                        view.shape_plan_reset_after_edit = false;
                        view.shape_record_edit_error = Some(error.clone());
                        if expected_target.is_none() {
                            view.shape_coordination_error = Some(error);
                        }
                    }
                }
                ctx.notify();
            },
            |_, _| {},
        );
        ctx.notify();
    }

    pub(crate) fn move_shape_plan_task(
        &mut self,
        task_id: dragent_workspace_model::PlanTaskId,
        direction: super::ShapePlanTaskMove,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending {
            return;
        }
        let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
            return;
        };
        if !move_plan_task(&mut plan, task_id, direction) {
            return;
        }
        self.submit_plan_structure_revision(plan, ctx);
    }

    pub(crate) fn remove_shape_plan_task(
        &mut self,
        task_id: dragent_workspace_model::PlanTaskId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending {
            return;
        }
        let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
            return;
        };
        if let Err(error) = remove_plan_task(&mut plan, task_id) {
            self.shape_coordination_error = Some(error);
            ctx.notify();
            return;
        }
        // Two-step remove: first press arms ("Confirm remove"), second press
        // executes.
        let armed = super::ShapeRecordEditTarget::PlanTask(task_id);
        if self.shape_remove_armed != Some(armed) {
            self.shape_remove_armed = Some(armed);
            ctx.notify();
            return;
        }
        self.shape_remove_armed = None;
        self.submit_plan_structure_revision(plan, ctx);
    }

    pub(crate) fn remove_shape_record(
        &mut self,
        target: super::ShapeRecordEditTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending {
            return;
        }
        let Some(work) = self.workspace_state.active_work().cloned() else {
            return;
        };
        let payload = match target {
            super::ShapeRecordEditTarget::Requirement(id) => {
                let Some(mut records) = self.workspace_state.shaping_records_projection() else {
                    return;
                };
                records.requirements.retain(|record| record.id != id);
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::Decision(id) => {
                let Some(mut records) = self.workspace_state.shaping_records_projection() else {
                    return;
                };
                records.decisions.retain(|record| record.id != id);
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::AcceptanceCriterion(id) => {
                let Some(mut records) = self.workspace_state.shaping_records_projection() else {
                    return;
                };
                records.acceptance_criteria.retain(|record| record.id != id);
                WorkspaceCommandPayload::ReplaceShapingRecords { records }
            }
            super::ShapeRecordEditTarget::Artifact(id) => {
                let mut artifacts = self.workspace_state.artifacts();
                artifacts.retain(|artifact| artifact.id != id);
                WorkspaceCommandPayload::ReplaceArtifacts { artifacts }
            }
            _ => return,
        };
        // Two-step remove: first press arms ("Confirm remove"), second press
        // executes. Anything else disarms (starting/cancelling/saving edits
        // clears the armed slot).
        if self.shape_remove_armed != Some(target) {
            self.shape_remove_armed = Some(target);
            ctx.notify();
            return;
        }
        self.shape_remove_armed = None;
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        self.shape_record_edit_target = None;
        self.shape_record_edit_text.clear();
        self.submit_shape_revision_command(
            WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: self.workspace_state.project().id,
                work_item_id: work.id,
                actor_id: actor.id,
                device_id: actor.device_id,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                expected_revision: Some(self.workspace_canonical_revision),
                payload,
            },
            None,
            ctx,
        );
    }

    pub(crate) fn submit_plan_structure_revision(
        &mut self,
        plan: dragent_workspace_model::Plan,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(work) = self.workspace_state.active_work().cloned() else {
            return;
        };
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        let now = chrono::Utc::now().timestamp_millis();
        let payload = match build_plan_revision_payload(&self.workspace_state, plan, actor.id, now)
        {
            Ok(payload) => payload,
            Err(error) => {
                self.shape_record_edit_error = Some(error.clone());
                self.shape_coordination_error = Some(error);
                ctx.notify();
                return;
            }
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: self.workspace_state.project().id,
            work_item_id: work.id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload,
        };
        self.shape_record_edit_target = None;
        self.shape_record_edit_text.clear();
        self.submit_shape_revision_command(command, None, ctx);
    }
}
