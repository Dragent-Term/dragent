//! Header strip for top-level workspace tabs (Worktree = workspace > Tab > Panes).
//!
//! Rendered above the workspace content in the center column, visually
//! consistent with the editor tabs (same `ide_chrome` geometry: active
//! background, hover states, right divider). A chip selects its surface on
//! click; hover reveals rename + close; `+` parks the current surface and
//! opens a fresh one. All state lives one level above the pane system —
//! switching never touches pane layouts.

use warpui::elements::{
    Align, ChildView, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, EventHandler, Expanded, Flex, Image, MainAxisAlignment,
    MainAxisSize, ParentElement, ScrollbarWidth, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, ReadView};
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{
    AcceptedByDropTarget, Clipped, DragAxis, Draggable, DropTarget, DropTargetData, Hoverable,
};
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::agent::TabSessionKind;
use crate::console::ai_toolbar::{brand_icon, AiTool};
use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::agent_definition::{AgentDefinition, AgentImage, CustomAgentDefinition};
use super::ide_chrome::{self, IdeMetrics};
use super::pane_layout::EDITOR_PANE_ID;
use super::view::{PaneTab, WorkspaceView};
use super::workspace_tab::WorkspaceTab;
use super::WorkspaceAction;

/// A chip in the header strip, where another dragged chip can land.
#[derive(Debug, Clone)]
struct WorkspaceTabDropTarget {
    surface_id: usize,
}

impl DropTargetData for WorkspaceTabDropTarget {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Leading icon for a workspace-tab chip: an app glyph, bundled artwork,
/// or a user-supplied image file. All reuse existing assets — nothing
/// tab-specific.
pub(crate) enum SurfaceChipIcon {
    App(AppIcon),
    Asset(&'static str),
    File(String),
}

/// Resolved chip content for one workspace tab. Resolved from live pane /
/// session / editor state on every render, never stored, so it cannot go
/// stale. A user rename overrides the label only; the icon stays dynamic.
pub(crate) struct SurfaceChip {
    pub label: String,
    pub icon: SurfaceChipIcon,
}

/// Chip content for one agent definition: registry name plus registry
/// artwork (bundled or user file), falling back to the shared brand glyph.
fn definition_chip(definition: &AgentDefinition) -> SurfaceChip {
    let icon = match &definition.image {
        Some(AgentImage::Bundled(path)) => SurfaceChipIcon::Asset(path),
        Some(AgentImage::File(path)) => SurfaceChipIcon::File(path.clone()),
        None => SurfaceChipIcon::App(AppIcon::Agent),
    };
    SurfaceChip {
        label: definition.name.clone(),
        icon,
    }
}

/// Chip content for one live-detected CLI tool: registry artwork when the
/// tool maps to a known binary, otherwise the shared brand glyph. Used for
/// both the running tool and the last-command fallback so the two agree.
fn live_tool_chip(tool: AiTool, is_dark: bool) -> SurfaceChip {
    use super::agent_definition::builtin_image_for_cli;
    let icon = match builtin_image_for_cli(tool.cli(), is_dark) {
        Some(AgentImage::Bundled(path)) => SurfaceChipIcon::Asset(path),
        Some(AgentImage::File(path)) => SurfaceChipIcon::File(path),
        None => SurfaceChipIcon::App(brand_icon(tool)),
    };
    SurfaceChip {
        label: tool.label().to_string(),
        icon,
    }
}

/// Chip content for one pane-tab, resolved live-first so tabs and pane
/// headers follow whatever agent is actually running: live tool → last
/// command (registry for customs, detector spellings for the rest) →
/// stored launch identity (restore/idle) → agent-kind tab → terminal.
/// Stored identity never shadows a live or recently-run agent; it only
/// fills in when detection has nothing to say.
///
/// Login suffix rule (see `login_suffix_for_session`):
/// - Dragent never shows a login (`Dragent`, not `Dragent · System login`).
/// - No stored identity → bare name (`Claude`).
/// - Single identity for that provider (no extra profiles) → bare name.
/// - Multiple identities → `Name · <login>` to separate them
///   (`Claude · System login`, `Claude · Work`).
pub(crate) fn chip_for_pane<C: ReadView>(tab: &PaneTab, ctx: &C, customs: &[CustomAgentDefinition], is_dark: bool) -> SurfaceChip {
    let mut chip = unlabelled_chip_for_pane(tab, ctx, customs, is_dark);
    #[cfg(not(target_family = "wasm"))]
    if let Some(record) = dragent_host::agent_sessions::load(tab.pane.pane_uuid(ctx)) {
        if let Some(suffix) = login_suffix_for_session(&record) {
            chip.label = format!("{} · {}", chip.label, suffix);
        }
    }
    chip
}

/// Conditional login suffix: `None` means bare agent name.
///
/// - `dragent` provider → always `None` (its terminal never shows a login).
/// - `record.account == None` (identity never set) → `None`.
/// - Provider has no extra saved profiles (only System exists) → `None`,
///   except a non-`System login` label still shows (registry unreadable or
///   stale binding — it already disambiguates).
/// - Provider has ≥1 saved profile → `Some(label)` (`System login` or the
///   profile label) so panes separate.
pub(crate) fn login_suffix_for_session(
    record: &dragent_host::agent_sessions::AgentSession,
) -> Option<String> {
    if record.provider == "dragent" {
        return None;
    }
    let binding = record.account.as_ref()?;
    if provider_has_multiple_identities(&record.provider) {
        return Some(binding.label.clone());
    }
    if binding.label == "System login" {
        None
    } else {
        Some(binding.label.clone())
    }
}

/// True when a provider has ≥1 saved extra profile (System + profile, or
/// profile + profile) so panes need the login to tell sessions apart.
/// `dragent` never counts — it has no account adapter.
pub(crate) fn provider_has_multiple_identities(provider: &str) -> bool {
    if provider == "dragent" {
        return false;
    }
    match dragent_host::agent_accounts::Registry::load() {
        Ok(registry) => registry
            .profiles
            .iter()
            .any(|profile| profile.provider == provider),
        Err(_) => false,
    }
}

/// Truncate a header segment to `max_chars` with an ellipsis. Headers have
/// fixed widths — a long task or command must never push split/share/close
/// off the edge.
pub(crate) fn truncate_header_segment(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Short display name for a running command (`npm test`, `cargo build`),
/// never file/dir arguments. Mirrors `TerminalView::command_display_name`
/// so headers stay one short line without depending on the terminal view.
pub(crate) fn short_command_name(command: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for token in command.split_whitespace().take(4) {
        if token.contains('/') || token.contains('\\') || token.starts_with('-') {
            break;
        }
        out.push(token);
        if out.len() == 2 {
            break;
        }
    }
    let mut name = out.join(" ");
    if name.is_empty() {
        name = command
            .split_whitespace()
            .next()
            .unwrap_or("command")
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("command")
            .to_string();
    }
    truncate_header_segment(&name, 32)
}

fn unlabelled_chip_for_pane<C: ReadView>(
    tab: &PaneTab,
    ctx: &C,
    customs: &[CustomAgentDefinition],
    is_dark: bool,
) -> SurfaceChip {
    use super::agent_definition::{definition_by_cli, definition_by_id};
    if tab.pane.is_settings() {
        return SurfaceChip {
            label: "Settings".to_string(),
            icon: SurfaceChipIcon::App(AppIcon::Settings),
        };
    }
    if tab.pane.is_browser() {
        return SurfaceChip {
            label: tab.pane.display_title(ctx),
            icon: tab
                .pane
                .browser_icon(ctx)
                .map(SurfaceChipIcon::File)
                .unwrap_or(SurfaceChipIcon::App(AppIcon::Globe)),
        };
    }
    if let Some(tool) = tab.pane.active_ai_tool(ctx) {
        return live_tool_chip(tool, is_dark);
    }
    if let Some(command) = tab.pane.last_command(ctx) {
        if let Some(binary) = command.split_whitespace().next() {
            if let Some(definition) = definition_by_cli(binary, customs, is_dark) {
                return definition_chip(&definition);
            }
        }
        if let Some(tool) = AiTool::from_command(&command) {
            return live_tool_chip(tool, is_dark);
        }
    }
    if let Some(agent_id) = tab.pane.agent_id(ctx) {
        if let Some(definition) = definition_by_id(&agent_id, customs, is_dark) {
            return definition_chip(&definition);
        }
    }
    if tab.pane.tab_session_kind(ctx) == TabSessionKind::Agent {
        return SurfaceChip {
            label: "Agent".to_string(),
            icon: SurfaceChipIcon::App(AppIcon::Agent),
        };
    }
    SurfaceChip {
        label: "Terminal".to_string(),
        icon: SurfaceChipIcon::App(AppIcon::Terminal),
    }
}

/// Live Dragent context for the relay catalog: active work title, latest
/// live step (principal coordination activity while a coordination turn
/// is in flight, else the newest non-finished pipeline entry), and recent
/// pipeline entries oldest-first. `None` for non-Dragent panes or while
/// idle — the phone falls back to PTY bytes then. Read-only; never
/// touches creds or prompts. Bounds mirror the relay catalog caps
/// (`MAX_AGENT_ACTIVITY_ENTRIES`, text truncated to the catalog title /
/// activity bounds); the aggregator re-truncates defensively.
pub(crate) struct DragentRelayContext {
    pub work_title: Option<String>,
    pub step: Option<String>,
    pub activity: Vec<crate::agent::execution_pipeline::WorkspaceExecutionActivity>,
}

impl WorkspaceView {
    /// Resolve one surface's chip from live state. The active surface may
    /// additionally reflect the editor (its document state is workspace-
    /// global in slice 1); parked surfaces resolve from their own panes.
    pub(crate) fn surface_chip<C: ReadView>(&self, ctx: &C, surface_id: usize) -> SurfaceChip {
        if surface_id == self.active_workspace_tab_id {
            if self.editor_shows_active_file() {
                if let Some(path) = self
                    .document_requested_path
                    .clone()
                    .or_else(|| self.document_open_paths.last().cloned())
                {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Editor")
                        .to_string();
                    let asset = dragent_editor::FileKind::from_path(&path).icon_asset();
                    return self.apply_custom_chip_name(
                        surface_id,
                        SurfaceChip {
                            label: name,
                            icon: SurfaceChipIcon::Asset(asset),
                        },
                    );
                }
            }
            let chip = self
                .tabs
                .iter()
                .find(|tab| tab.id == self.active_tab_id)
                .map(|tab| self.full_pane_chip(tab, ctx))
                .unwrap_or(SurfaceChip {
                    label: "Empty".to_string(),
                    icon: SurfaceChipIcon::App(AppIcon::Terminal),
                });
            return self.apply_custom_chip_name(surface_id, chip);
        }
        if let Some(tab) = self
            .parked_workspace_tabs
            .iter()
            .find(|tab| tab.id == surface_id)
        {
            let chip = if tab.active_tab_id == EDITOR_PANE_ID {
                SurfaceChip {
                    label: "Editor".to_string(),
                    icon: SurfaceChipIcon::App(AppIcon::Code),
                }
            } else {
                tab.panes
                    .iter()
                    .find(|pane| pane.id == tab.active_tab_id)
                    .map(|pane| self.full_pane_chip(pane, ctx))
                    .unwrap_or(SurfaceChip {
                        label: "Empty".to_string(),
                        icon: SurfaceChipIcon::App(AppIcon::Terminal),
                    })
            };
            return self.apply_custom_chip_name(surface_id, chip);
        }
        SurfaceChip {
            label: String::new(),
            icon: SurfaceChipIcon::App(AppIcon::Terminal),
        }
    }

    /// A user rename overrides the resolved label only; empty means dynamic.
    fn apply_custom_chip_name(&self, surface_id: usize, mut chip: SurfaceChip) -> SurfaceChip {
        let custom = if surface_id == self.active_workspace_tab_id {
            Some(self.active_workspace_tab_name.as_str())
        } else {
            self.parked_workspace_tabs
                .iter()
                .find(|tab| tab.id == surface_id)
                .map(|tab| tab.name.as_str())
        };
        if let Some(name) = custom {
            if !name.trim().is_empty() {
                chip.label = name.to_string();
            }
        }
        chip
    }

    /// Dragent's own agent runs in the workspace runtime (principal /
    /// execution pipeline), not in the terminal controller that
    /// `pane_agent_status` reads. Without this, a working Dragent would
    /// stay `Idle` and never glow. Any active pipeline session or pending
    /// principal coordination counts as Dragent working — including the
    /// Dragent terminal.
    pub(crate) fn is_dragent_working(&self) -> bool {
        if self.workspace_principal_coordination_pending {
            return true;
        }
        if self.workspace_execution_runtime.active_count() > 0 {
            return true;
        }
        false
    }

    pub(crate) fn is_dragent_pane<C: ReadView>(&self, tab: &PaneTab, ctx: &C) -> bool {
        tab.pane.agent_id(ctx).as_deref() == Some("dragent")
    }

    pub(crate) fn dragent_relay_context<C: ReadView>(
        &self,
        tab: &PaneTab,
        ctx: &C,
    ) -> Option<DragentRelayContext> {
        if !self.is_dragent_pane(tab, ctx) {
            return None;
        }
        if !self.is_dragent_working() {
            return None;
        }
        let work_title = self
            .workspace_state
            .active_work()
            .filter(|work| !super::project_naming::is_default_work_label(&work.title))
            .map(|work| work.title.clone())
            .filter(|title| !title.trim().is_empty());
        // One collection for both the latest step and the recent list:
        // newest non-finished entry is the live step, the tail is the list.
        let mut all = Vec::new();
        for id in self
            .workspace_execution_runtime
            .active_attempt_ids()
            .into_iter()
            .take(8)
        {
            for entry in self.workspace_execution_runtime.activity(id) {
                all.push(entry);
            }
        }
        all.sort_by_key(|entry| entry.sequence);
        let mut step = if self.workspace_principal_coordination_pending {
            Some(
                self.workspace_principal_coordination_activity
                    .clone()
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "Working…".to_string()),
            )
        } else {
            all.iter()
                .rev()
                .find(|entry| entry.kind != "finished" && !entry.text.trim().is_empty())
                .map(|entry| entry.text.clone())
        };
        let mut activity: Vec<_> = all
            .into_iter()
            .rev()
            .take(
                crate::realtime::desktop_catalog::MAX_AGENT_ACTIVITY_ENTRIES,
            )
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // Keep per-render clones small: head-truncate long texts here; the
        // catalog aggregator enforces the final bounds.
        let truncate = |text: String, max: usize| {
            if text.chars().count() <= max {
                text
            } else {
                text.chars().take(max).collect()
            }
        };
        step = step
            .map(|text| {
                truncate(
                    text,
                    crate::realtime::desktop_catalog::MAX_TITLE_CHARS,
                )
            })
            .filter(|text| !text.trim().is_empty());
        for entry in activity.iter_mut() {
            entry.text = truncate(
                std::mem::take(&mut entry.text),
                crate::realtime::desktop_catalog::MAX_AGENT_ACTIVITY_TEXT_CHARS,
            );
            entry.kind = truncate(
                std::mem::take(&mut entry.kind),
                crate::realtime::desktop_catalog::MAX_AGENT_ACTIVITY_KIND_CHARS,
            );
        }
        activity.retain(|entry| !entry.text.trim().is_empty());
        if step.is_none() && work_title.is_none() && activity.is_empty() {
            // Working but nothing titled yet — keep a generic live marker so
            // the phone renders something instead of a blank idle tab.
            step = Some("Working…".to_string());
        }
        Some(DragentRelayContext {
            work_title,
            step,
            activity,
        })
    }

    /// Effective lifecycle for one pane: terminal signals first, CLI hook
    /// activity (the same source the sidebar/workspace rows use) second,
    /// Dragent runtime third. Covers internal, external CLI and
    /// Dragent-terminal agents so every header updates together.
    pub(crate) fn effective_pane_status<C: ReadView>(
        &self,
        tab: &PaneTab,
        ctx: &C,
    ) -> super::worktree_status::WorktreeAgentStatus {
        let pane_uuid = tab.pane.pane_uuid(ctx);
        let stop = self.agent_stop_state.get(&pane_uuid);
        let mut status = super::worktree_status::pane_agent_status(&tab.pane, stop, ctx);
        // Same idea as the workspace/sidebar rows: CLI hook activity
        // (Start/PermissionRequest/Failed/Stop) polled off-thread into
        // LeftPanelView. Without this, external CLIs (Claude, Codex, …)
        // stay Idle in the header while they work — only Dragent lit up
        // via its runtime fallback below.
        let cli_status: Option<super::worktree_status::WorktreeAgentStatus> =
            self.left_panel_view.read(ctx, |lp, _| {
                lp.tabs
                    .iter()
                    .flat_map(|t| t.agents.iter())
                    .find(|a| a.pane_id == pane_uuid)
                    .map(|a| a.status.clone())
            });
        if let Some(cli) = cli_status {
            status = super::worktree_status::WorktreeAgentStatus::pick_higher(status, cli);
        }
        if !matches!(
            status,
            super::worktree_status::WorktreeAgentStatus::Idle
        ) {
            return status;
        }
        if self.is_dragent_pane(tab, ctx) && self.is_dragent_working() {
            return super::worktree_status::WorktreeAgentStatus::Working {
                label: "Dragent".to_string(),
            };
        }
        status
    }

    /// Full header label: base agent chip + conditional login + live work
    /// context. Work context only appends when the pane is not idle, so
    /// idle tabs stay bare (`Claude`, `Dragent`) while working tabs read
    /// `Claude · Login authentication` or
    /// `Claude · Work · Login authentication · Dispatched: edit`.
    pub(crate) fn full_pane_chip<C: ReadView>(
        &self,
        tab: &PaneTab,
        ctx: &C,
    ) -> SurfaceChip {
        let is_dark = self.theme.is_dark;
        let mut chip = chip_for_pane(tab, ctx, &self.cached_custom_agents, is_dark);
        if super::agent_sleep::any_asleep()
            && super::agent_sleep::is_pane_asleep(tab.pane.pane_uuid(ctx))
        {
            chip.label = format!("{} · Sleeping", chip.label);
            return chip;
        }
        let status = self.effective_pane_status(tab, ctx);
        if matches!(
            status,
            super::worktree_status::WorktreeAgentStatus::Idle
        ) {
            return chip;
        }
        if let Some(work) = self.pane_work_context(tab, ctx) {
            chip.label = format!("{} · {}", chip.label, work);
        }
        chip
    }

    /// Live work context for the trailing header segment. Updates as the
    /// agent works (task title + latest live step), never stored.
    ///
    /// Priority:
    /// 1. Team CLI worker (`cli_workers` by pane uuid): its task title
    ///    (`Login authentication`) plus the latest live step (running
    ///    command or pipeline activity like `Dispatched: edit`).
    /// 2. Any running agent command in this pane (external/adhoc agents).
    /// 3. Active work title (Dragent / internal agent fallback).
    /// 4. `None` — header stays bare + glow, no stale guess.
    pub(crate) fn pane_work_context<C: ReadView>(
        &self,
        tab: &PaneTab,
        ctx: &C,
    ) -> Option<String> {
        let pane_uuid = tab.pane.pane_uuid(ctx);
        if let Some(worker) = self
            .cli_workers
            .values()
            .find(|worker| worker.pane == pane_uuid && !worker.done)
        {
            let base = truncate_header_segment(&worker.task, 40);
            if base.is_empty() {
                return None;
            }
            if let Some(live) = self.cli_worker_live_step(worker.attempt_id, pane_uuid) {
                if live.is_empty() || live.eq_ignore_ascii_case(&base) {
                    return Some(base);
                }
                let live = truncate_header_segment(&live, 32);
                return Some(truncate_header_segment(
                    &format!("{base} · {live}"),
                    64,
                ));
            }
            return Some(base);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let runs = crate::agent::local_tools::running_agent_commands_for(pane_uuid);
            if let Some(run) = runs.first() {
                let name = short_command_name(&run.command);
                if !name.is_empty() {
                    // Prefer pipeline activity when it says more than the
                    // bare command (e.g. `Dispatched: edit` vs `npm`).
                    if let Some(activity) = self.latest_pipeline_step(None, Some(pane_uuid)) {
                        if !activity.is_empty() && !activity.eq_ignore_ascii_case(&name) {
                            let combined =
                                format!("{} · {}", truncate_header_segment(&name, 24), truncate_header_segment(&activity, 28));
                            return Some(truncate_header_segment(&combined, 64));
                        }
                    }
                    return Some(truncate_header_segment(&name, 40));
                }
            }
        }
        // Dragent's own work: active work title + live principal step.
        // This is what makes `Dragent · Login authentication · Thinking…`
        // update while the principal coordinates, even with no terminal
        // command running.
        if self.is_dragent_pane(tab, ctx) && self.is_dragent_working() {
            let work_title = self
                .workspace_state
                .active_work()
                .filter(|work| !super::project_naming::is_default_work_label(&work.title))
                .map(|work| truncate_header_segment(&work.title, 40));
            let principal_step = if self.workspace_principal_coordination_pending {
                self.workspace_principal_coordination_activity
                    .as_deref()
                    .map(|text| truncate_header_segment(text, 32))
                    .filter(|text| !text.is_empty())
                    .or_else(|| Some("Working…".to_string()))
            } else {
                self.latest_pipeline_step(None, Some(pane_uuid))
            };
            match (work_title, principal_step) {
                (Some(title), Some(step)) if !step.eq_ignore_ascii_case(&title) => {
                    return Some(truncate_header_segment(
                        &format!("{title} · {step}"),
                        64,
                    ));
                }
                (Some(title), _) => return Some(title),
                (None, Some(step)) => return Some(step),
                (None, None) => {
                    // Still working but no title yet — keep the glow + a
                    // generic live marker so the header updates.
                    return Some("Working…".to_string());
                }
            }
        }
        if let Some(work) = self.workspace_state.active_work() {
            if !super::project_naming::is_default_work_label(&work.title) {
                // A live pipeline step keeps the trailing segment updating
                // even when the work title itself is stable.
                if let Some(step) = self.latest_pipeline_step(None, Some(pane_uuid)) {
                    let title = truncate_header_segment(&work.title, 36);
                    let step = truncate_header_segment(&step, 28);
                    if !step.is_empty() {
                        return Some(truncate_header_segment(
                            &format!("{title} · {step}"),
                            64,
                        ));
                    }
                }
                return Some(truncate_header_segment(&work.title, 48));
            }
        }
        None
    }

    /// Latest live step for a team attempt (preferred) or any pane:
    /// pipeline activity (`Thinking…`, `Dispatched: <tool>`) or the running
    /// command's short name. `None` when finished/idle so the header falls
    /// back to the stable task/work title.
    fn cli_worker_live_step(
        &self,
        attempt_id: dragent_workspace_model::ExecutionAttemptId,
        pane_uuid: uuid::Uuid,
    ) -> Option<String> {
        if let Some(step) = self.latest_pipeline_step(Some(attempt_id), Some(pane_uuid)) {
            return Some(step);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let runs = crate::agent::local_tools::running_agent_commands_for(pane_uuid);
            if let Some(run) = runs.first() {
                let name = short_command_name(&run.command);
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
        None
    }

    /// Newest pipeline activity text for an attempt, or the most recent
    /// activity across attempts when `attempt_id` is `None`. Finished
    /// entries resolve to `None` so callers keep the stable title.
    fn latest_pipeline_step(
        &self,
        attempt_id: Option<dragent_workspace_model::ExecutionAttemptId>,
        _pane_uuid: Option<uuid::Uuid>,
    ) -> Option<String> {
        let texts: Vec<(u64, String, String)> = if let Some(id) = attempt_id {
            self.workspace_execution_runtime
                .activity(id)
                .into_iter()
                .map(|entry| (entry.sequence, entry.kind, entry.text))
                .collect()
        } else {
            // Adhoc panes have no attempt: scan recent activity across the
            // runtime and take the newest non-finished entry. Bounded — the
            // runtime keeps per-attempt logs small.
            let mut all = Vec::new();
            for id in self
                .workspace_execution_runtime
                .active_attempt_ids()
                .into_iter()
                .take(8)
            {
                for entry in self.workspace_execution_runtime.activity(id) {
                    all.push((entry.sequence, entry.kind.clone(), entry.text.clone()));
                }
            }
            all.sort_by_key(|(seq, _, _)| *seq);
            all
        };
        for (_seq, kind, text) in texts.into_iter().rev() {
            if kind == "finished" {
                return None;
            }
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Raw narration can be a full sentence — keep the header short.
            return Some(truncate_header_segment(trimmed, 32));
        }
        None
    }

    /// The editor owns the active context when its pane is focused and a
    /// document is actually shown (fullscreen center or a loaded file).
    fn editor_shows_active_file(&self) -> bool {
        if self.active_tab_id == EDITOR_PANE_ID {
            return true;
        }
        self.editor_pane_focused
            && (self.document_center_open
                || self.document_requested_path.is_some()
                || !self.document_open_paths.is_empty())
    }
    /// Agent lifecycle for a top-level surface: highest-priority status
    /// across its panes (active surface reads live panes, parked reads its
    /// stored panes best-effort). Drives the header glow + dot so the tab
    /// bar always reflects internal, external and Dragent-terminal agents.
    pub(crate) fn surface_agent_status<C: ReadView>(
        &self,
        ctx: &C,
        surface_id: usize,
    ) -> super::worktree_status::WorktreeAgentStatus {
        use super::worktree_status::WorktreeAgentStatus;
        let mut status = WorktreeAgentStatus::Idle;
        if surface_id == self.active_workspace_tab_id {
            // Active surface: fold every split pane so a background split
            // still lights the tab. Label stays per-active-tab (see
            // `surface_chip`); glow reflects any working pane. Uses the
            // effective status so Dragent-runtime work lights the tab even
            // when no terminal controller is busy.
            for tab in self.tabs.iter() {
                status = WorktreeAgentStatus::pick_higher(
                    status,
                    self.effective_pane_status(tab, ctx),
                );
            }
            return status;
        }
        if let Some(parked) = self
            .parked_workspace_tabs
            .iter()
            .find(|tab| tab.id == surface_id)
        {
            for pane in parked.panes.iter() {
                // Parked panes are not mounted — read best-effort. A uuid
                // that no longer resolves stays Idle rather than sticking.
                status = WorktreeAgentStatus::pick_higher(
                    status,
                    self.effective_pane_status(pane, ctx),
                );
            }
        }
        status
    }

    /// The header tab chips: one per surface in stable strip order. This is
    /// bare content for the main header row (no bar of its own) — the header
    /// embeds it where its empty spacer used to be, so tabs start right
    /// after the sidebar controls and scroll beneath the right cluster. A
    /// right-edge fade overlaps the scroller so tabs visibly slide under the
    /// header controls instead of clipping against them.
    pub(crate) fn render_workspace_tab_strip(
        &self,
        app: &AppContext,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let ui_font: FamilyId = self.ui_font;
        let view_id = self.workspace_view_id;
        let theme_owned = *theme;
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(0.0);

        for (surface_id, _name, active) in self.ordered_workspace_tab_rows() {
            let key = format!("workspace_tab_{surface_id}");
            let mouse = self
                .header_mouse_states
                .borrow_mut()
                .entry(key)
                .or_default()
                .clone();
            let renaming = self.renaming_workspace_tab == Some(surface_id);
            let rename_editor = &self.workspace_tab_rename_editor;
            let close_key = format!("workspace_tab_close_{surface_id}");
            let close_mouse = self
                .header_mouse_states
                .borrow_mut()
                .entry(close_key)
                .or_default()
                .clone();
            let rename_key = format!("workspace_tab_rename_{surface_id}");
            let rename_mouse = self
                .header_mouse_states
                .borrow_mut()
                .entry(rename_key)
                .or_default()
                .clone();
            let chip = self.surface_chip(app, surface_id);
            let label = chip.label.clone();
            let chip_icon = chip.icon;
            // Agent lifecycle for this surface — internal, external and
            // Dragent-terminal agents all fold in here, so the tab bar
            // always reflects whoever is working.
            let agent_status = self.surface_agent_status(app, surface_id);
            let status_idle =
                matches!(agent_status, super::worktree_status::WorktreeAgentStatus::Idle);
            let status_color = agent_status.color(&theme_owned);
            let is_working = matches!(
                agent_status,
                super::worktree_status::WorktreeAgentStatus::Working { .. }
            );
            let chip: Box<dyn Element> = if renaming {
                // Inline rename: the strip editor commits on Enter, cancels
                // on Escape via `InputMode::RenameWorkspaceTab`. Same full
                // header height as a chip so the row never jumps.
                Box::new(
                    ConstrainedBox::new(
                        Container::new(
                            Flex::column()
                                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                                .with_main_axis_size(MainAxisSize::Max)
                                .with_main_axis_alignment(MainAxisAlignment::Center)
                                .with_child(
                                    Container::new(ChildView::new(rename_editor).finish())
                                        .with_padding_left(IdeMetrics::TAB_PAD_LEFT)
                                        .with_padding_right(IdeMetrics::TAB_PAD_RIGHT)
                                        .with_background_color(theme_owned.surface_2())
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_background_color(theme_owned.surface_2())
                        .with_border(
                            Border::top(IdeMetrics::TAB_ACTIVE_RAIL)
                                .with_border_color(theme_owned.accent),
                        )
                        .finish(),
                    )
                    .with_width(IdeMetrics::TAB_WIDTH)
                    .with_height(crate::theme::HEADER_BAR_HEIGHT),
                )
            } else {
                Box::new(
                    Hoverable::new(mouse, move |state| {
                        let hovered = state.is_hovered();
                        let color = if active || hovered {
                            theme_owned.main_text()
                        } else {
                            theme_owned.sub_text()
                        };
                        let trailing: Box<dyn Element> = if hovered {
                            let rename = EventHandler::new(
                                ConstrainedBox::new(
                                    Align::new(render_icon(AppIcon::Edit, color, 12.0)).finish(),
                                )
                                .with_width(IdeMetrics::TAB_TRAILING)
                                .with_height(IdeMetrics::TAB_TRAILING)
                                .finish(),
                            )
                            .on_left_mouse_down(move |ctx, _, _| {
                                ctx.dispatch_typed_action_for(
                                    view_id,
                                    WorkspaceAction::StartRenameWorkspaceTab {
                                        workspace_tab_id: surface_id,
                                    },
                                );
                                DispatchEventResult::StopPropagation
                            })
                            .finish();
                            let close = EventHandler::new(
                                ConstrainedBox::new(
                                    Align::new(render_icon(AppIcon::XClose, color, 12.0)).finish(),
                                )
                                .with_width(IdeMetrics::TAB_TRAILING)
                                .with_height(IdeMetrics::TAB_TRAILING)
                                .finish(),
                            )
                            .on_left_mouse_down(move |ctx, _, _| {
                                ctx.dispatch_typed_action_for(
                                    view_id,
                                    WorkspaceAction::CloseWorkspaceTab {
                                        workspace_tab_id: surface_id,
                                    },
                                );
                                DispatchEventResult::StopPropagation
                            })
                            .finish();
                            Box::new(
                                Flex::row()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_main_axis_size(MainAxisSize::Min)
                                    .with_spacing(0.0)
                                    .with_child(rename)
                                    .with_child(close),
                            )
                        } else if !status_idle {
                            // Working (or needs input / failed / review): the
                            // trailing slot always shows the live status dot,
                            // so the tab bar updates without hovering.
                            let glow = status_color;
                            let dot: Box<dyn Element> = if is_working {
                                Container::new(
                                    Container::new(
                                        ConstrainedBox::new(
                                            warpui::elements::Empty::new().finish(),
                                        )
                                        .with_width(5.0)
                                        .with_height(5.0)
                                        .finish(),
                                    )
                                    .with_background_color(glow)
                                    .with_corner_radius(CornerRadius::with_all(
                                        Radius::Pixels(3.0),
                                    ))
                                    .finish(),
                                )
                                .with_background_color(warpui::color::ColorU::new(
                                    glow.r, glow.g, glow.b, 40,
                                ))
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                                .with_padding_left(2.5)
                                .with_padding_right(2.5)
                                .with_padding_top(2.5)
                                .with_padding_bottom(2.5)
                                .finish()
                            } else {
                                Container::new(
                                    ConstrainedBox::new(
                                        warpui::elements::Empty::new().finish(),
                                    )
                                    .with_width(6.0)
                                    .with_height(6.0)
                                    .finish(),
                                )
                                .with_background_color(glow)
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                                .finish()
                            };
                            Container::new(Align::new(dot).finish())
                                .with_padding_left(20.0)
                                .with_padding_right(8.0)
                                .finish()
                        } else {
                            // Same 40px trailing slot as the hover buttons: a
                            // non-hovered chip keeps the shared bar height.
                            Box::new(
                                ConstrainedBox::new(warpui::elements::Empty::new().finish())
                                    .with_width(IdeMetrics::TAB_TRAILING * 2.0)
                                    .with_height(IdeMetrics::TAB_TRAILING),
                            )
                        };
                        let icon_tint = if !status_idle { status_color } else { color };
                        let leading: Box<dyn Element> = match chip_icon {
                            SurfaceChipIcon::App(icon) => {
                                ConstrainedBox::new(render_icon(icon, icon_tint, 13.0))
                                    .with_width(IdeMetrics::TAB_ICON)
                                    .with_height(IdeMetrics::TAB_ICON)
                                    .finish()
                            }
                            SurfaceChipIcon::Asset(path) => ConstrainedBox::new(
                                Image::new(AssetSource::Bundled { path }, CacheOption::BySize)
                                    .contain()
                                    .finish(),
                            )
                            .with_width(IdeMetrics::TAB_ICON)
                            .with_height(IdeMetrics::TAB_ICON)
                            .finish(),
                            SurfaceChipIcon::File(path) => ConstrainedBox::new(
                                Image::new(
                                    AssetSource::LocalFile { path: path.clone() },
                                    CacheOption::BySize,
                                )
                                .contain()
                                .finish(),
                            )
                            .with_width(IdeMetrics::TAB_ICON)
                            .with_height(IdeMetrics::TAB_ICON)
                            .finish(),
                        };
                        let content = Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_spacing(IdeMetrics::TAB_GAP)
                            .with_child(leading)
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    Clipped::new(
                                        Text::new(label.clone(), ui_font, crate::theme::text_size(12.0))
                                            .with_color(color)
                                            .soft_wrap(false)
                                            .with_clip(
                                                warpui_core::text_layout::ClipConfig::ellipsis(),
                                            )
                                            .finish(),
                                    )
                                    .finish(),
                                )
                                .finish(),
                            )
                            .with_child(trailing)
                            .finish();
                        // Full header height with no vertical inset: the
                        // active chip's bottom edge lands directly on the
                        // header's border line instead of floating as a
                        // separate card. Content stays vertically centered.
                        let background = if active {
                            theme_owned.pane_surface()
                        } else if hovered {
                            theme_owned.surface_2()
                        } else {
                            warpui::color::ColorU::new(0, 0, 0, 0)
                        };
                        // Live agent status overrides the active rail so a
                        // working tab glows even when it is not selected.
                        let rail = if !status_idle {
                            status_color
                        } else {
                            ide_chrome::tab_active_rail(&theme_owned, active)
                        };
                        let centered = Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_main_axis_alignment(MainAxisAlignment::Center)
                            .with_child(
                                Container::new(content)
                                    .with_padding_left(IdeMetrics::TAB_PAD_LEFT)
                                    .with_padding_right(IdeMetrics::TAB_PAD_RIGHT)
                                    .with_background_color(background)
                                    .finish(),
                            )
                            .finish();
                        let body = Container::new(centered)
                            .with_background_color(background)
                            .with_border(
                                Border::top(IdeMetrics::TAB_ACTIVE_RAIL).with_border_color(rail),
                            )
                            .finish();
                        ConstrainedBox::new(
                            Container::new(body)
                                .with_background_color(background)
                                .with_border(
                                    Border::right(1.0)
                                        .with_border_color(ide_chrome::tab_divider(&theme_owned)),
                                )
                                .finish(),
                        )
                        .with_width(IdeMetrics::TAB_WIDTH)
                        .with_height(crate::theme::HEADER_BAR_HEIGHT)
                        .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(
                            view_id,
                            WorkspaceAction::SelectWorkspaceTab {
                                workspace_tab_id: surface_id,
                            },
                        )
                    }),
                )
            };
            // Drag a chip sideways onto another to move it there; a click
            // still selects it. Not while it's being renamed.
            let chip: Box<dyn Element> = if renaming {
                chip
            } else {
                let drag_state = self
                    .workspace_tab_drag_states
                    .borrow_mut()
                    .entry(surface_id)
                    .or_default()
                    .clone();
                let draggable = Draggable::new(drag_state, chip)
                    .with_drag_axis(DragAxis::HorizontalOnly)
                    .with_keep_original_visible(true)
                    .with_drop_target_at_pointer()
                    .with_accepted_by_drop_target_fn(move |data, _| {
                        match data.as_any().downcast_ref::<WorkspaceTabDropTarget>() {
                            Some(target) if target.surface_id != surface_id => {
                                AcceptedByDropTarget::Yes
                            }
                            _ => AcceptedByDropTarget::No,
                        }
                    })
                    .on_drop(move |ctx, _, _, data| {
                        if let Some(target) = data
                            .and_then(|data| data.as_any().downcast_ref::<WorkspaceTabDropTarget>())
                        {
                            ctx.dispatch_typed_action_for(
                                view_id,
                                WorkspaceAction::MoveWorkspaceTabTo {
                                    workspace_tab_id: surface_id,
                                    target_workspace_tab_id: target.surface_id,
                                },
                            );
                        }
                    })
                    .finish();
                DropTarget::new(draggable, WorkspaceTabDropTarget { surface_id }).finish()
            };
            row = row.with_child(
                warpui::elements::SavePosition::new(
                    ConstrainedBox::new(chip)
                        .with_width(IdeMetrics::TAB_WIDTH)
                        .finish(),
                    &workspace_tab_position_id(surface_id),
                )
                .finish(),
            );
        }

        let clear = warpui::color::ColorU::new(0, 0, 0, 0);
        let scroller = ClippedScrollable::horizontal(
            self.workspace_tab_strip_scroll.clone(),
            row.finish(),
            ScrollbarWidth::None,
            clear.into(),
            clear.into(),
            clear.into(),
        )
        .with_overlayed_scrollbar()
        .finish();
        // The scroller clips at the fixed add button. A decorative overlay
        // here covers the last tab's close/rename hit targets, so leave the
        // edge unobstructed for both painting and pointer events.
        scroller
    }

    /// Close-confirmation modal for a top-level workspace tab. Closing stops
    /// every pane session on that surface and removes worktrees no other
    /// surface references; the last tab resets instead of disappearing.
    pub(crate) fn render_close_workspace_tab_modal(
        &self,
        theme: &Theme,
        surface_id: usize,
        summary: &str,
        display_name: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let name = display_name.to_string();
        let workspace = self
            .pending_close_project_workspace
            .and_then(|id| self.workspaces.iter().find(|workspace| workspace.id == id));
        let surface_ids = workspace
            .map(|workspace| workspace.tab_ids.clone())
            .unwrap_or_else(|| vec![surface_id]);
        let panes: Vec<&super::view::PaneTab> = surface_ids
            .iter()
            .flat_map(|id| {
                if *id == self.active_workspace_tab_id {
                    self.tabs.iter().collect::<Vec<_>>()
                } else {
                    self.parked_workspace_tabs
                        .iter()
                        .find(|tab| tab.id == *id)
                        .map(|tab| tab.panes.iter().collect())
                        .unwrap_or_default()
                }
            })
            .collect();
        let session_count = panes.iter().filter(|tab| !tab.pane.is_settings()).count();
        let mut short = summary.to_string();
        if short.chars().count() > 64 {
            short = short.chars().take(64).collect::<String>();
            short.push('\u{2026}');
        }

        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: &'static str,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("closeworkspacetab_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |st| {
                    let bg = if st.is_hovered() {
                        warpui::color::ColorU {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                            a: 28,
                        }
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    };
                    Container::new(
                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
            )
        };

        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Cancel".to_string(),
                theme.sub_text(),
                theme.outline(),
                "cancel",
                WorkspaceAction::CancelCloseWorkspaceTab,
            ))
            .with_child(button(
                if workspace.is_some() {
                    "Close workspace"
                } else {
                    "Close tab"
                }
                .to_string(),
                theme.ui_red(),
                theme.ui_red(),
                "confirm",
                WorkspaceAction::CloseWorkspaceTabConfirmed {
                    workspace_tab_id: surface_id,
                },
            ))
            .finish();

        let mut body = if session_count == 1 {
            format!("This will close \u{201c}{name}\u{201d} and stop its session.")
        } else {
            format!("This will close \u{201c}{name}\u{201d} and stop its {session_count} sessions.")
        };
        if !summary.is_empty() {
            body.push_str(&format!(
                " \u{201c}{short}\u{201d} is still running and will be stopped."
            ));
        }

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        format!("Close \u{201c}{name}\u{201d}?"),
                        ui,
                        crate::theme::text_size(15.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(body, ui, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(Container::new(buttons).with_padding_top(4.0).finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(20.0)
        .with_padding_right(20.0)
        .with_padding_top(20.0)
        .with_padding_bottom(20.0)
        .finish();

        let centered = ConstrainedBox::new(dialog).with_max_width(440.0).finish();
        Container::new(Align::new(centered).finish())
            .with_background_color(warpui::color::ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish()
    }
}

impl SurfaceChipIcon {
    pub(crate) fn render(self, color: warpui::color::ColorU, size: f32) -> Box<dyn Element> {
        let content = match self {
            Self::App(icon) => render_icon(icon, color, size),
            Self::Asset(path) => Image::new(AssetSource::Bundled { path }, CacheOption::BySize)
                .contain()
                .finish(),
            Self::File(path) => Image::new(AssetSource::LocalFile { path }, CacheOption::BySize)
                .contain()
                .finish(),
        };
        ConstrainedBox::new(content)
            .with_width(size)
            .with_height(size)
            .finish()
    }
}

pub(crate) fn workspace_tab_position_id(tab_id: usize) -> String {
    format!("workspace_header_tab_{tab_id}")
}

impl WorkspaceView {
    /// Resolve the tab's bounds after layout, so this works when adding a
    /// tab changes the strip from fitting to overflowing in the same frame.
    pub(crate) fn reveal_workspace_tab(&self, tab_id: usize) {
        self.workspace_tab_strip_scroll
            .scroll_to_position(warpui::elements::ScrollTarget {
                position_id: workspace_tab_position_id(tab_id),
                mode: warpui::elements::ScrollToPositionMode::FullyIntoView,
            });
    }
}

#[cfg(test)]
mod header_tests {
    use super::{login_suffix_for_session, provider_has_multiple_identities, truncate_header_segment};

    fn test_session(provider: &str, label: Option<&str>) -> dragent_host::agent_sessions::AgentSession {
        dragent_host::agent_sessions::AgentSession {
            organisation_id: "test-org".to_string(),
            pane_uuid: uuid::Uuid::new_v4(),
            generation: uuid::Uuid::new_v4(),
            provider: provider.to_string(),
            account: label.map(|label| dragent_host::agent_accounts::Binding {
                selection: dragent_host::agent_accounts::Selection::System,
                provider: provider.to_string(),
                machine_id: None,
                label: label.to_string(),
            }),
            cwd: std::path::PathBuf::from("/tmp"),
            executable: provider.to_string(),
            resume_options: None,
            conversation_id: None,
            active: true,
            resumed: false,
            last_exit_code: None,
            workspace_id: "test".to_string(),
            terminal_id: "test".to_string(),
            recovery: None,
            used: true,
        }
    }

    #[test]
    fn dragent_never_shows_login() {
        assert_eq!(provider_has_multiple_identities("dragent"), false);
        let session = test_session("dragent", Some("System login"));
        assert_eq!(login_suffix_for_session(&session), None);
    }

    #[test]
    fn missing_identity_stays_bare() {
        let session = test_session("claude", None);
        assert_eq!(login_suffix_for_session(&session), None);
    }

    #[test]
    fn header_segments_truncate_with_ellipsis() {
        assert_eq!(truncate_header_segment("Login authentication", 40), "Login authentication");
        assert_eq!(truncate_header_segment("Login authentication", 10), "Login aut…");
        assert_eq!(truncate_header_segment("  padded  ", 10), "padded");
    }

    #[test]
    fn short_command_names_stay_short() {
        assert_eq!(super::short_command_name("npm test --watch"), "npm test");
        assert_eq!(super::short_command_name("/usr/local/bin/npm test"), "npm");
        assert_eq!(
            super::short_command_name("cargo test -- --nocapture"),
            "cargo test"
        );
    }
}
