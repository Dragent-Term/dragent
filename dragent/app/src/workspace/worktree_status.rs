//! Per-worktree sidebar status: agent activity, git diff, and PR state.
//!
//! The sidebar trailing slot composes one glanceable cluster per worktree
//! row: a spinner while any agent works, diff stats for that worktree, and
//! worktree's branch. Every value resolves from live state — the pane's
//! agent controller and command registry, `git` in the worktree itself,
//! and the per-root PR cache — never from global repo state.

use std::path::{Path, PathBuf};

use crate::console::ai_toolbar::AiTool;
use crate::process_ext::NoWindow;

/// Diff summary for exactly one worktree checkout.
#[derive(Clone, Debug, Default)]
pub struct WorktreeGitStatus {
    pub branch: Option<String>,
    /// `files changed` from `git diff --shortstat HEAD` (staged + unstaged).
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    /// Untracked files from `git status --porcelain` (`??` lines).
    pub untracked: usize,
}

impl WorktreeGitStatus {
    pub fn is_clean(&self) -> bool {
        self.files_changed == 0 && self.untracked == 0
    }
}

/// Query git for one worktree root. Runs two cheap read-only commands;
/// callers must invoke this off the UI thread and cache the result.
pub fn query_worktree_git_status(root: &Path) -> Option<WorktreeGitStatus> {
    if !crate::remote_fs::is_remote(root) && !root.is_dir() {
        return None;
    }
    let branch = crate::workspace::project_naming::local_branch_name(root);
    let shortstat = crate::git_cmd::GitCommand::new()
        .args(["diff", "--shortstat", "HEAD", "--"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let untracked = crate::git_cmd::GitCommand::new()
        .args(["status", "--porcelain=v1", "-uall", "--"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| line.starts_with("??"))
                .count()
        })
        .unwrap_or(0);
    let (files_changed, insertions, deletions) = parse_shortstat(&shortstat);
    Some(WorktreeGitStatus {
        branch,
        files_changed,
        insertions,
        deletions,
        untracked,
    })
}

/// Parse `1 file changed, 10 insertions(+), 2 deletions(-)` (any subset).
fn parse_shortstat(output: &str) -> (usize, usize, usize) {
    let mut files = 0;
    let mut insertions = 0;
    let mut deletions = 0;
    for part in output.split(',') {
        let mut tokens = part.split_whitespace();
        let count: usize = tokens.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        match tokens.next().unwrap_or("") {
            token if token.starts_with("file") => files = count,
            token if token.starts_with("insertion") => insertions = count,
            token if token.starts_with("deletion") => deletions = count,
            _ => {}
        }
    }
    (files, insertions, deletions)
}

/// CI state for a worktree's PR badge, mapped from the review service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarPrChecks {
    Passing,
    Failing,
    Pending,
    #[default]
    None,
}

/// PR identity for a worktree's badge. `number == 0` means no PR (renders nothing).
#[derive(Clone, Debug, Default)]
pub struct SidebarPrBadge {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub is_draft: bool,
    pub checks: SidebarPrChecks,
}

pub fn pr_badge_from_cached(
    cached: &super::review_pr_service::CachedReviewPr,
) -> Option<SidebarPrBadge> {
    let pr = cached.pr.as_ref()?;
    let checks = if pr.checks.iter().any(|check| {
        matches!(
            check.status,
            super::project_sidebar::ReviewCheckStatus::Failure
        )
    }) {
        SidebarPrChecks::Failing
    } else if pr.checks.iter().any(|check| {
        matches!(
            check.status,
            super::project_sidebar::ReviewCheckStatus::Pending
        )
    }) {
        SidebarPrChecks::Pending
    } else if pr.checks.iter().any(|check| {
        matches!(
            check.status,
            super::project_sidebar::ReviewCheckStatus::Success
        )
    }) {
        SidebarPrChecks::Passing
    } else {
        SidebarPrChecks::None
    };
    Some(SidebarPrBadge {
        number: pr.number,
        title: pr.title.clone(),
        state: pr.state.clone(),
        is_draft: pr.state.eq_ignore_ascii_case("draft"),
        checks,
    })
}

/// Agent lifecycle status for one pane, resolved from our runtime signals
/// (hook events mapped onto local state):
///
/// ```text
/// Event               Our signal
/// Start → working     controller busy, foreground command, or CLI running
/// PermissionRequest   pane has a pending confirm request
/// Failed → failed     last completed block failed on an agent pane
/// Stop → review/idle  recorded stop newer than last-seen (else idle)
/// (no binding) → idle pane gone, shell-only, or nothing detected
/// ```
///
/// Priority order: permission > failed > working > review > idle. The row
/// aggregates by highest priority across panes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorktreeAgentStatus {
    #[default]
    Idle,
    /// Agent finished and the user hasn't looked at the pane since.
    Review,
    Working {
        label: String,
    },
    Permission,
    Failed,
}

/// One entry per pane, including duplicate providers in separate sessions.
#[derive(Clone, Debug)]
pub struct SidebarAgent {
    pub image: Option<super::AgentImage>,
    pub pane_id: uuid::Uuid,
    pub tool: Option<AiTool>,
    pub status: WorktreeAgentStatus,
    pub base_status: WorktreeAgentStatus,
    pub cli: bool,
    pub activity_known: bool,
}

/// Durable stop record for one pane: when its agent last finished, when the
/// user last looked at it, and whether it failed. Backs the review dot.
#[derive(Clone, Debug)]
pub struct AgentStopState {
    pub stopped_at_ms: i64,
    pub seen_at_ms: i64,
    pub failed: bool,
}

/// The least contrast a small status mark keeps against its background.
pub(crate) const STATUS_MARK_CONTRAST: f32 = 4.5;

/// "Needs input": the orange the tab's header shows, everywhere an agent
/// waits on the person (failures stay red).
/// A fixed orange: a theme's yellow can be a pale cream that reads as
/// neither orange nor urgent.
pub(crate) fn needs_input_color(_theme: &crate::theme::Theme) -> warpui::color::ColorU {
    NEEDS_INPUT_ORANGE
}

pub(crate) const NEEDS_INPUT_ORANGE: warpui::color::ColorU = warpui::color::ColorU {
    r: 0xf0,
    g: 0x8a,
    b: 0x3c,
    a: 0xff,
};

/// The working spinner's amber, as the phone draws it.
pub(crate) const WORKING_AMBER: warpui::color::ColorU = warpui::color::ColorU {
    r: 0xd9,
    g: 0xa4,
    b: 0x41,
    a: 0xff,
};

fn relative_luminance(color: warpui::color::ColorU) -> f32 {
    let channel = |value: u8| {
        let v = value as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast between two opaque colours.
pub(crate) fn contrast_ratio(a: warpui::color::ColorU, b: warpui::color::ColorU) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    let (high, low) = if a >= b { (a, b) } else { (b, a) };
    (high + 0.05) / (low + 0.05)
}

/// `color` in its own family, darkened on a light background (lightened on
/// a dark one) just enough to reach `target` contrast. Blending toward black
/// or white keeps the hue, so a theme's green stays its green.
pub(crate) fn with_contrast(
    color: warpui::color::ColorU,
    background: warpui::color::ColorU,
    target: f32,
) -> warpui::color::ColorU {
    use warpui::color::ColorU;
    let opaque = ColorU::new(color.r, color.g, color.b, 255);
    if contrast_ratio(opaque, background) >= target {
        return opaque;
    }
    let toward = if relative_luminance(background) < 0.18 { 255.0 } else { 0.0 };
    let mix = |from: u8, t: f32| (from as f32 + (toward - from as f32) * t).round() as u8;
    for step in 1..=20 {
        let t = step as f32 / 20.0;
        let candidate = ColorU::new(mix(opaque.r, t), mix(opaque.g, t), mix(opaque.b, t), 255);
        if contrast_ratio(candidate, background) >= target {
            return candidate;
        }
    }
    let end = toward as u8;
    ColorU::new(end, end, end, 255)
}

/// Wall-clock milliseconds for stop/seen bookkeeping.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

impl WorktreeAgentStatus {
    pub(crate) fn color(&self, theme: &crate::theme::Theme) -> warpui::color::ColorU {
        match self {
            Self::Working { .. } => theme.accent,
            Self::Review => theme.ui_green(),
            Self::Permission => needs_input_color(theme),
            Self::Failed => theme.ui_red(),
            Self::Idle => warpui::color::ColorU::transparent_black(),
        }
    }

    /// The colour of the status mark on a workspace's icon, as the phone
    /// shows it: amber while working, green when ready for review, orange when
    /// it needs input, red when it failed. The theme's own colour, sharpened to stand
    /// out on its background. None when idle.
    pub(crate) fn mark_color(&self, theme: &crate::theme::Theme) -> Option<warpui::color::ColorU> {
        let base = match self {
            // The phone's amber: a terminal palette's yellow is often a pale
            // cream that doesn't read as "working".
            Self::Working { .. } => WORKING_AMBER,
            Self::Review => theme.ui_green(),
            Self::Permission => needs_input_color(theme),
            Self::Failed => theme.ui_red(),
            Self::Idle => return None,
        };
        Some(with_contrast(base, theme.solid_background(), STATUS_MARK_CONTRAST))
    }

    pub(crate) fn after_cli_event(&self, event: Option<&str>, new_event: bool) -> Self {
        match event {
            Some("Start") => Self::Working {
                label: String::new(),
            },
            Some("PermissionRequest") => Self::Permission,
            // Like a finish: news until the person has looked at it.
            Some("Failed") if new_event || matches!(self, Self::Failed) => Self::Failed,
            Some("Stop") if new_event || matches!(self, Self::Review) => Self::Review,
            _ => Self::Idle,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, WorktreeAgentStatus::Idle)
    }

    /// Status snapshots do not need polling for animation. The working icon
    /// schedules its own paints; other lifecycle states stay still.
    pub fn needs_tick(&self) -> bool {
        false
    }

    fn priority(&self) -> u8 {
        match self {
            WorktreeAgentStatus::Idle => 0,
            WorktreeAgentStatus::Review => 1,
            WorktreeAgentStatus::Working { .. } => 2,
            WorktreeAgentStatus::Failed => 3,
            WorktreeAgentStatus::Permission => 4,
        }
    }

    /// Highest-priority status wins.
    pub fn pick_higher(a: Self, b: Self) -> Self {
        if b.priority() > a.priority() {
            b
        } else {
            a
        }
    }
}

/// A phone view after the stop counts like a desktop view of the pane: the
/// finished agent is seen, so its Review clears on mobile too. Render-safe
/// (in-memory phone-seen instants, never blocks).
fn phone_saw_stop(pane: uuid::Uuid, stopped_at_ms: i64) -> bool {
    crate::realtime::phone_agent::seen_at(pane)
        .is_some_and(|seen| seen >= stopped_at_ms.max(0) as u64)
}

/// Resolve one pane's lifecycle status from live runtime state. Stopped
/// agents always resolve to `Idle` or `Review` — nothing latches `Working`,
/// so the indicator can never stick on after the underlying state goes
/// quiet. `stop` is the pane's recorded completion (if any); a successful
/// stop newer than its last-seen moment renders `Review`.
pub fn pane_agent_status<C: warpui::ReadView>(
    pane: &super::pane::WorkspacePane,
    stop: Option<&AgentStopState>,
    ctx: &C,
) -> WorktreeAgentStatus {
    use crate::agent::TabSessionKind;
    let super::pane::WorkspacePane::Terminal(handle) = pane else {
        return WorktreeAgentStatus::Idle;
    };
    // Permission first: a pending confirm blocks the agent until resolved.
    if handle.read(ctx, |view, _| view.pending_confirm_request().is_some()) {
        return WorktreeAgentStatus::Permission;
    }
    let tool = pane.active_ai_tool(ctx);
    let busy = handle.read(ctx, |view, view_ctx| {
        view.agent_controller
            .read(view_ctx, |controller, _| controller.is_busy())
    });
    let pane_uuid = pane.pane_uuid(ctx);
    let foreground_blocked =
        !crate::agent::local_tools::foreground_agent_commands_for(pane_uuid).is_empty();
    if busy || foreground_blocked {
        return WorktreeAgentStatus::Working {
            label: tool
                .map(|tool| tool.label().to_string())
                .unwrap_or_else(|| "Dragent".to_string()),
        };
    }
    // Explicit completion first: `record_agent_stop` fires on every turn
    // boundary (`BlockCompleted` for terminal agents, `TurnCompleted` /
    // `Error` for the internal AgentController incl. Dragent). An unseen
    // stop is authoritative — a successful Dragent finish must show Review
    // even when an older terminal block failed. Checking the stale block
    // latch first is what stuck finished Dragent panes on Failed.
    // A phone view counts like a desktop view: opening the tab on the phone
    // marks the stop seen, so Review clears there too.
    if let Some(stop) = stop {
        if stop.stopped_at_ms > stop.seen_at_ms && !phone_saw_stop(pane_uuid, stop.stopped_at_ms) {
            if stop.failed {
                return WorktreeAgentStatus::Failed;
            } else {
                return WorktreeAgentStatus::Review;
            }
        }
    }
    // Failed latch fallback: the most recent completed block failed on a
    // pane that belongs to an agent, with no unseen stop to override it.
    // Cleared automatically by the next run (which flips to Working).
    let is_agent_pane = pane.agent_id(ctx).is_some()
        || pane.tab_session_kind(ctx) == TabSessionKind::Agent
        || tool.is_some()
        || pane
            .last_command(ctx)
            .as_deref()
            .and_then(AiTool::from_command)
            .is_some();
    // Only before any finish was recorded: once there is one, the person
    // has seen it (an unseen one returned above), and a failure they've
    // looked at no longer shows.
    if is_agent_pane && stop.is_none() {
        let last_failed = handle.read(ctx, |view, view_ctx| {
            view.terminal_model().read(view_ctx, |model, _| {
                model.with_core(|core| {
                    core.blocks()
                        .blocks()
                        .iter()
                        .rev()
                        .find(|block| block.is_completed())
                        .is_some_and(|block| {
                            block.completion.as_ref().is_some_and(|done| done.failed)
                        })
                })
            })
        });
        if last_failed {
            return WorktreeAgentStatus::Failed;
        }
    }
    WorktreeAgentStatus::Idle
}

/// Whether a hook report is news. While the app runs, any change is. On the
/// first look at a session (after a launch), only a finish the person hasn't
/// seen is: one that came after they last looked at it.
pub(crate) fn is_new_activity(
    previous: Option<&(uuid::Uuid, u64, String)>,
    current: &(uuid::Uuid, u64, String),
    seen_ms: Option<u64>,
) -> bool {
    match previous {
        Some(previous) => previous != current,
        None => seen_ms.is_some_and(|seen| current.1 > seen),
    }
}

/// The person looked at an agent after its last report (at `event_ms`).
pub(crate) fn seen_since(event_ms: Option<u64>, seen_ms: Option<u64>) -> bool {
    matches!((event_ms, seen_ms), (Some(event), Some(seen)) if seen >= event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_marks_are_sharp_on_every_theme_in_their_own_family() {
        let hue = |c: warpui::color::ColorU| {
            let (r, g, b) = (c.r as f32, c.g as f32, c.b as f32);
            let (max, min) = (r.max(g).max(b), r.min(g).min(b));
            if max == min {
                return 0.0;
            }
            let d = max - min;
            let h = if max == r {
                ((g - b) / d).rem_euclid(6.0)
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            h * 60.0
        };
        for info in crate::theme::Theme::all_infos() {
            let theme = &info.theme;
            for (status, base) in [
                (WorktreeAgentStatus::Working { label: String::new() }, WORKING_AMBER),
                (WorktreeAgentStatus::Review, theme.ui_green()),
                (WorktreeAgentStatus::Permission, NEEDS_INPUT_ORANGE),
                (WorktreeAgentStatus::Failed, theme.ui_red()),
            ] {
                let mark = status.mark_color(theme).unwrap();
                let contrast = contrast_ratio(mark, theme.solid_background());
                assert!(contrast >= STATUS_MARK_CONTRAST - 0.01, "{} {status:?}: {contrast}", info.id);
                let drift = (hue(mark) - hue(base)).abs();
                assert!(
                    drift.min(360.0 - drift) <= 8.0 || base.r.abs_diff(base.g) < 12 && base.g.abs_diff(base.b) < 12,
                    "{} {status:?} left its family: {base:?} → {mark:?}",
                    info.id
                );
            }
            assert_eq!(WorktreeAgentStatus::Idle.mark_color(theme), None);
        }
    }

    #[test]
    fn a_finish_looked_at_elsewhere_counts_as_seen() {
        assert!(seen_since(Some(1_000), Some(1_000)));
        assert!(seen_since(Some(1_000), Some(5_000)));
        // Finished again after the last look: still to review.
        assert!(!seen_since(Some(5_000), Some(1_000)));
        assert!(!seen_since(Some(1_000), None));
        assert!(!seen_since(None, Some(1_000)));
    }

    #[test]
    fn a_finish_already_viewed_stays_viewed_after_a_restart() {
        let generation = uuid::Uuid::from_u128(1);
        let stop = (generation, 1_000, "Stop".to_string());
        let after_launch = |seen| {
            WorktreeAgentStatus::Idle
                .after_cli_event(Some("Stop"), is_new_activity(None, &stop, seen))
        };
        // Viewed after it finished: it stays idle.
        assert_eq!(after_launch(Some(2_000)), WorktreeAgentStatus::Idle);
        // First time this session is met: its state counts as known.
        assert_eq!(after_launch(Some(1_000)), WorktreeAgentStatus::Idle);
        // Finished after the last look, the app closed or not: still to review.
        assert_eq!(after_launch(Some(500)), WorktreeAgentStatus::Review);
    }

    #[test]
    fn while_the_app_runs_any_change_is_news() {
        let generation = uuid::Uuid::from_u128(1);
        let earlier = (generation, 1_000, "Start".to_string());
        let stop = (generation, 2_000, "Stop".to_string());
        assert!(is_new_activity(Some(&earlier), &stop, None));
        assert!(!is_new_activity(Some(&stop), &stop, None));
    }

    #[test]
    fn an_agent_that_went_quiet_is_idle_not_working_or_ready_for_review() {
        let working = WorktreeAgentStatus::Idle.after_cli_event(Some("Start"), true);
        assert!(matches!(working, WorktreeAgentStatus::Working { .. }));
        let quiet = dragent_host::agent_sessions::ACTIVITY_QUIET;
        assert_eq!(
            working.after_cli_event(Some(quiet), true),
            WorktreeAgentStatus::Idle
        );
    }

    #[test]
    fn cli_completion_stays_green_until_acknowledged_and_does_not_reappear() {
        let working = WorktreeAgentStatus::Idle.after_cli_event(Some("Start"), true);
        assert!(matches!(working, WorktreeAgentStatus::Working { .. }));
        let completed = working.after_cli_event(Some("Stop"), true);
        assert_eq!(completed, WorktreeAgentStatus::Review);
        assert_eq!(completed.after_cli_event(Some("Stop"), false), completed);
        let acknowledged = WorktreeAgentStatus::Idle;
        assert_eq!(
            acknowledged.after_cli_event(Some("Stop"), false),
            WorktreeAgentStatus::Idle
        );
        assert_eq!(
            acknowledged.after_cli_event(Some("Stop"), true),
            WorktreeAgentStatus::Review
        );
    }

    #[test]
    fn a_failure_shows_until_seen_and_does_not_come_back() {
        let failed = WorktreeAgentStatus::Idle.after_cli_event(Some("Failed"), true);
        assert_eq!(failed, WorktreeAgentStatus::Failed);
        // Unseen, it stays.
        assert_eq!(failed.after_cli_event(Some("Failed"), false), WorktreeAgentStatus::Failed);
        // Once looked at (desktop or phone), the same old report is no news.
        assert_eq!(
            WorktreeAgentStatus::Idle.after_cli_event(Some("Failed"), false),
            WorktreeAgentStatus::Idle
        );
        // A new message starts it working again.
        assert!(matches!(
            failed.after_cli_event(Some("Start"), true),
            WorktreeAgentStatus::Working { .. }
        ));
    }

    #[test]
    fn activity_colors_follow_the_selected_accent_and_success_state() {
        let mut theme = crate::theme::Theme::dark();
        theme.accent = warpui::color::ColorU::new(100, 50, 210, 255);
        assert_eq!(
            WorktreeAgentStatus::Working {
                label: String::new()
            }
            .color(&theme),
            theme.accent
        );
        assert_eq!(WorktreeAgentStatus::Review.color(&theme), theme.ui_green());
        assert_eq!(
            WorktreeAgentStatus::Permission.color(&theme),
            NEEDS_INPUT_ORANGE
        );
        assert_eq!(WorktreeAgentStatus::Failed.color(&theme), theme.ui_red());
        assert_eq!(WorktreeAgentStatus::Idle.color(&theme).a, 0);
    }

    #[test]
    fn failed_paused_and_missing_activity_do_not_report_success() {
        for (event, expected) in [
            (Some("Failed"), WorktreeAgentStatus::Failed),
            (Some("PermissionRequest"), WorktreeAgentStatus::Permission),
            (None, WorktreeAgentStatus::Idle),
        ] {
            assert_eq!(
                WorktreeAgentStatus::Review.after_cli_event(event, true),
                expected
            );
        }
    }

    #[test]
    fn shortstat_parses_full_summary() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)"),
            (3, 10, 2)
        );
    }

    #[test]
    fn shortstat_parses_partial_summaries() {
        assert_eq!(
            parse_shortstat(" 1 file changed, 5 insertions(+)"),
            (1, 5, 0)
        );
        assert_eq!(
            parse_shortstat(" 2 files changed, 1 deletion(-)"),
            (2, 0, 1)
        );
        assert_eq!(parse_shortstat(""), (0, 0, 0));
    }

    #[test]
    fn clean_status_has_no_changes() {
        assert!(WorktreeGitStatus::default().is_clean());
        assert!(!WorktreeGitStatus {
            untracked: 1,
            ..Default::default()
        }
        .is_clean());
    }
}
