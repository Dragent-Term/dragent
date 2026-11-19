//! Comment on diff lines and hand the comment to an agent.
//!
//! Hovering a diff line shows a + in its gutter; clicking it (or dragging
//! across lines) opens a composer card under the selection. The card sends
//! the comment, with the file and line range, to an agent already running in
//! this workspace, or starts a new agent session in this workspace: beside
//! the diff or in a new tab.

use super::view::{ProjectSidebarView, SidebarAction};
use crate::git_status::diff_head::GitDiffTarget;
use crate::workspace::{InputEditor, WorkspaceAction};
use crate::{
    icons::{render_icon, AppIcon},
    theme::Theme,
};
use std::path::{Path, PathBuf};
use warpui::elements::{
    ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Fill, Flex,
    Image, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    ScrollbarWidth, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{Entity, EntityId, ReadModel, TypedActionView, UpdateView, ViewContext, ViewHandle};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, DropShadow, Radius};
use warpui_core::{
    assets::asset_cache::AssetSource, image_cache::CacheOption, text_layout::ClipConfig,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffCommentAnchor {
    pub path: PathBuf,
    pub target: GitDiffTarget,
    pub start: usize,
    pub end: usize,
    pub start_deleted: bool,
    pub end_deleted: bool,
}

/// Something a comment can be sent to: an agent already running in this
/// workspace, or an installed agent to start. Shared with browser design
/// notes, which pick from the same menu.
pub(crate) type DiffCommentAgent = crate::workspace::AgentTarget;

/// Where a new agent session opens. Always in this workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Beside what you're looking at.
    Split,
    /// A new tab.
    Tab,
}

impl Placement {
    pub const ALL: [Placement; 2] = [Placement::Split, Placement::Tab];

    pub fn from_settings(settings: &crate::setup::UiSettings) -> Self {
        match settings.diff_comment_placement.as_deref() {
            Some("tab") => Placement::Tab,
            Some("split") => Placement::Split,
            _ if settings.diff_comment_new_tab => Placement::Tab,
            _ => Placement::Split,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Placement::Split => "split",
            Placement::Tab => "tab",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Placement::Split => "Split pane",
            Placement::Tab => "New tab",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Placement::Split => "Opens beside this diff",
            Placement::Tab => "Opens as another tab in this workspace",
        }
    }

    fn icon(self) -> AppIcon {
        match self {
            Placement::Split => AppIcon::SplitVertical,
            Placement::Tab => AppIcon::Plus,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMenu {
    Agents,
    Placement,
}

#[derive(Clone)]
pub(crate) struct DiffComment {
    pub anchor: DiffCommentAnchor,
    pub workspace: Option<(String, usize)>,
    pub context_hunk: Option<dragent_document::DiffHunk>,
    pub root: PathBuf,
    pub text: String,
    pub agents: Vec<DiffCommentAgent>,
    pub selected: Option<String>,
    pub selection_override: bool,
    pub menu: Option<ComposerMenu>,
    pub menu_active: usize,
    pub search: String,
    pub placement: Placement,
    pub sending: bool,
    pub error: Option<String>,
    pub generation: uuid::Uuid,
    pub scroll: ClippedScrollStateHandle,
}

/// Fixed card metrics: the diff lists reserve exactly this much room.
const CARD_MARGIN: f32 = 6.0;
const CARD_PADDING: f32 = 12.0;
const HEADER_HEIGHT: f32 = 16.0;
const EDITOR_HEIGHT: f32 = 88.0;
const ROW_HEIGHT: f32 = 28.0;
const GAP: f32 = 8.0;

impl DiffComment {
    pub fn matches(&self, path: &Path, target: GitDiffTarget) -> bool {
        self.anchor.path == path && self.anchor.target == target
    }

    /// The height the card takes in the diff, margins included.
    pub fn height(&self) -> f32 {
        crate::theme::scaled(
            CARD_MARGIN * 2.0
                + CARD_PADDING * 2.0
                + HEADER_HEIGHT
                + EDITOR_HEIGHT
                + ROW_HEIGHT * 2.0
                + GAP * 3.0,
        )
    }

    pub fn chosen(&self) -> Option<&DiffCommentAgent> {
        let key = self.selected.as_ref()?;
        self.agents.iter().find(|agent| agent.key == *key)
    }

    fn range_label(&self) -> String {
        let a = &self.anchor;
        let (first, last) = (a.start.min(a.end), a.start.max(a.end));
        if first == last {
            format!("Line {first}")
        } else {
            format!("Lines {first}–{last}")
        }
    }

    /// The text the agent receives: where the lines are, then the comment.
    pub fn prompt(&self) -> String {
        let a = &self.anchor;
        let range = if a.start == a.end {
            format!("L{}", a.start)
        } else {
            format!("L{}-L{}", a.start.min(a.end), a.start.max(a.end))
        };
        let side = if a.start_deleted && a.end_deleted {
            " (deleted lines)"
        } else if a.start_deleted != a.end_deleted {
            " (across deletions and additions)"
        } else {
            ""
        };
        format!(
            "In {}:{range}{side}: {}",
            a.path.display(),
            self.text.trim()
        )
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect()
    }

    /// One line under the controls saying what Send will do.
    fn outcome(&self) -> String {
        if self.sending {
            return match self.chosen() {
                Some(agent) if !agent.existing => format!("Starting {}…", agent.name),
                _ => "Delivering…".into(),
            };
        }
        match self.chosen() {
            None => "Pick who should get this comment".into(),
            Some(agent) if agent.existing => format!("Goes to {} as your next message", agent.title),
            Some(agent) => match self.placement {
                Placement::Split => format!("Starts a new {} session beside this diff", agent.name),
                Placement::Tab => format!("Starts a new {} session in a new tab", agent.name),
            },
        }
    }

    fn can_send(&self) -> bool {
        !self.sending && !self.text.trim().is_empty() && self.chosen().is_some()
    }
}

impl ProjectSidebarView {
    /// Repaint the card wherever it is drawn. The code editor's diff is
    /// drawn by the workspace, which only redraws when this view emits.
    pub(crate) fn diff_comment_changed(ctx: &mut ViewContext<Self>) {
        ctx.notify();
        ctx.emit(());
    }

    /// A hover handle that survives redraws.
    fn comment_mouse(&self, key: &str) -> MouseStateHandle {
        let mut handles = self.diff_comment_mouse.borrow_mut();
        if handles.len() > 4096 {
            handles.clear();
        }
        handles.entry(key.to_string()).or_default().clone()
    }

    pub(crate) fn begin_diff_comment(
        &mut self,
        path: PathBuf,
        target: GitDiffTarget,
        start: usize,
        end: usize,
        start_deleted: bool,
        end_deleted: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let (start, end, start_deleted, end_deleted) = if start <= end {
            (start, end, start_deleted, end_deleted)
        } else {
            (end, start, end_deleted, start_deleted)
        };
        let root = ctx.read_model(&self.model, |m, _| {
            m.git_review_root()
                .unwrap_or(m.workspace_root())
                .to_path_buf()
        });
        let context_hunk = ctx.read_model(&self.model, |model, _| {
            let file = model
                .git_file_diff
                .as_ref()
                .filter(|f| f.relative_path == path && f.target == target)?;
            let regions = file.diff.unchanged_regions(
                file.old_text.lines().count(),
                file.new_text.lines().count(),
                1,
            );
            for region in &regions {
                let first = if end_deleted {
                    region.old_start
                } else {
                    region.new_start
                };
                if end >= first && end < first + region.line_count {
                    self.git_diff_expanded_regions.insert(region.id);
                }
            }
            if end_deleted
                || file
                    .diff
                    .hunks
                    .iter()
                    .any(|h| h.lines.iter().any(|line| line.new_line == Some(end)))
            {
                return None;
            }
            let region = regions.iter().find(|region| {
                end >= region.new_start && end < region.new_start + region.line_count
            })?;
            let first = start.max(region.new_start).max(end.saturating_sub(80));
            let lines: Vec<_> = file
                .new_text
                .lines()
                .enumerate()
                .skip(first.saturating_sub(1))
                .take(end.saturating_sub(first) + 1)
                .map(|(index, text)| dragent_document::DiffLine {
                    kind: dragent_document::DiffLineKind::Context,
                    old_line: Some(region.old_start + index + 1 - region.new_start),
                    new_line: Some(index + 1),
                    text: text.to_string(),
                })
                .collect();
            Some(dragent_document::DiffHunk {
                lines,
                ..Default::default()
            })
        });
        let settings = crate::setup::UiSettings::load();
        self.git_multi_collapsed.remove(&(path.clone(), target));
        self.git_multi_hunks_collapsed
            .retain(|(p, t, _)| *p != path || *t != target);
        self.diff_comment = Some(DiffComment {
            anchor: DiffCommentAnchor {
                path,
                target,
                start,
                end,
                start_deleted,
                end_deleted,
            },
            workspace: None,
            context_hunk,
            root,
            text: String::new(),
            agents: vec![],
            selected: None,
            selection_override: false,
            menu: None,
            menu_active: 0,
            search: String::new(),
            placement: Placement::from_settings(&settings),
            sending: false,
            error: None,
            generation: uuid::Uuid::new_v4(),
            scroll: ClippedScrollStateHandle::default(),
        });
        ctx.update_view(&self.diff_comment_editor, |editor, ec| {
            editor.set_text("", ec);
            ec.focus_self();
        });
        ctx.update_view(&self.diff_comment_search, |editor, ec| {
            editor.set_text("", ec)
        });
        ctx.dispatch_typed_action_deferred(WorkspaceAction::RefreshDiffCommentAgents);
        let generation = self.diff_comment.as_ref().unwrap().generation;
        self.refresh_diff_comment_while_open(generation, ctx);
        Self::diff_comment_changed(ctx);
    }

    /// Keep the agent list current while the card is open: sessions start
    /// and end around it.
    fn refresh_diff_comment_while_open(&self, generation: uuid::Uuid, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(2)),
            move |view, _, ctx| {
                if view
                    .diff_comment
                    .as_ref()
                    .is_some_and(|c| c.generation == generation)
                {
                    if view.diff_comment.as_ref().is_some_and(|c| !c.sending) {
                        ctx.dispatch_typed_action_deferred(
                            WorkspaceAction::RefreshDiffCommentAgents,
                        );
                    }
                    view.refresh_diff_comment_while_open(generation, ctx);
                }
            },
        );
    }

    /// Agents matching the menu's search, in menu order (running first).
    fn visible_agents(c: &DiffComment) -> Vec<&DiffCommentAgent> {
        crate::workspace::visible_agent_targets(&c.agents, &c.search)
    }

    pub(crate) fn handle_diff_comment_sidebar_action(
        &mut self,
        action: &SidebarAction,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        match action {
            SidebarAction::DiffCommentStart {
                path,
                target,
                line,
                deleted,
            } => {
                self.diff_comment_drag = Some(DiffCommentAnchor {
                    path: path.clone(),
                    target: *target,
                    start: *line,
                    end: *line,
                    start_deleted: *deleted,
                    end_deleted: *deleted,
                });
            }
            SidebarAction::DiffCommentExtend {
                path,
                target,
                line,
                deleted,
            } => {
                if let Some(a) = &mut self.diff_comment_drag {
                    if a.path == *path && a.target == *target {
                        a.end = *line;
                        a.end_deleted = *deleted;
                    }
                } else {
                    return true;
                }
            }
            SidebarAction::DiffCommentFinish => {
                let Some(a) = self.diff_comment_drag.take() else {
                    return true;
                };
                self.begin_diff_comment(
                    a.path,
                    a.target,
                    a.start,
                    a.end,
                    a.start_deleted,
                    a.end_deleted,
                    ctx,
                );
            }
            SidebarAction::DiffCommentCancel => {
                if let Some(c) = &mut self.diff_comment {
                    if c.sending {
                        return true;
                    }
                    // Esc closes an open menu before the card.
                    if c.menu.take().is_some() {
                        ctx.update_view(&self.diff_comment_editor, |_, ec| ec.focus_self());
                        Self::diff_comment_changed(ctx);
                        return true;
                    }
                }
                self.diff_comment = None;
                self.diff_comment_drag = None;
            }
            SidebarAction::DiffCommentMenu(menu) => {
                if let Some(c) = &mut self.diff_comment {
                    c.menu = (c.menu != Some(*menu)).then_some(*menu);
                    c.menu_active = 0;
                }
                if *menu == ComposerMenu::Agents {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::RefreshDiffCommentAgents);
                }
                if self.diff_comment.as_ref().is_some_and(|c| c.menu == Some(ComposerMenu::Agents)) {
                    ctx.update_view(&self.diff_comment_search, |editor, ec| {
                        editor.set_text("", ec);
                        ec.focus_self();
                    });
                } else {
                    ctx.update_view(&self.diff_comment_editor, |_, ec| ec.focus_self());
                }
            }
            SidebarAction::DiffCommentPickerMove(direction) => {
                if let Some(c) = &mut self.diff_comment {
                    let count = match c.menu {
                        Some(ComposerMenu::Agents) => Self::visible_agents(c).len(),
                        Some(ComposerMenu::Placement) => Placement::ALL.len(),
                        None => 0,
                    };
                    if count > 0 {
                        c.menu_active =
                            (c.menu_active as i32 + direction).rem_euclid(count as i32) as usize;
                    }
                }
            }
            SidebarAction::DiffCommentPickerAccept => {
                let choice = self.diff_comment.as_ref().and_then(|c| match c.menu {
                    Some(ComposerMenu::Agents) => Self::visible_agents(c)
                        .get(c.menu_active)
                        .map(|agent| SidebarAction::DiffCommentChoose(agent.key.clone())),
                    Some(ComposerMenu::Placement) => Placement::ALL
                        .get(c.menu_active)
                        .map(|placement| SidebarAction::DiffCommentPlacement(*placement)),
                    None => None,
                });
                if let Some(choice) = choice {
                    self.handle_diff_comment_sidebar_action(&choice, ctx);
                }
            }
            SidebarAction::DiffCommentClearSearch => {
                ctx.update_view(&self.diff_comment_search, |editor, ec| {
                    editor.set_text("", ec);
                    ec.focus_self();
                });
            }
            SidebarAction::DiffCommentPickerDismiss => {
                if let Some(c) = &mut self.diff_comment {
                    c.menu = None;
                }
                ctx.update_view(&self.diff_comment_editor, |_, ec| ec.focus_self());
            }
            SidebarAction::DiffCommentChoose(key) => {
                if let Some(c) = &mut self.diff_comment {
                    if c.agents.iter().any(|a| a.key == *key) {
                        c.selected = Some(key.clone());
                        c.selection_override = true;
                        c.menu = None;
                    }
                }
                crate::workspace::remember_agent_target(key);
                ctx.update_view(&self.diff_comment_editor, |_, ec| ec.focus_self());
            }
            SidebarAction::DiffCommentPlacement(placement) => {
                if let Some(c) = &mut self.diff_comment {
                    c.placement = *placement;
                    c.menu = None;
                }
                let mut settings = crate::setup::UiSettings::load();
                settings.diff_comment_placement = Some(placement.key().into());
                settings.diff_comment_new_tab = *placement == Placement::Tab;
                settings.save();
                ctx.update_view(&self.diff_comment_editor, |_, ec| ec.focus_self());
            }
            SidebarAction::DiffCommentSend => {
                if self.diff_comment.as_ref().is_some_and(DiffComment::can_send) {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::SendDiffComment);
                }
            }
            _ => return false,
        }
        Self::diff_comment_changed(ctx);
        true
    }
}

impl ProjectSidebarView {
    /// The E2E automation bridge's handle on the comment card: `start:<path>:<line>`
    /// opens a card on a working-tree line, `text:<comment>`, `agents` (menu),
    /// `choose:<key>`, `send`; anything else only reads the card's state.
    pub(crate) fn diff_comment_automation(
        &mut self,
        action: &str,
        ctx: &mut ViewContext<Self>,
    ) -> serde_json::Value {
        let (verb, rest) = action.split_once(':').unwrap_or((action, ""));
        match verb {
            "start" => {
                let Some((path, line)) = rest.rsplit_once(':') else {
                    return serde_json::json!({"ok": false, "error": "start:<path>:<line>"});
                };
                self.diff_comment_drag = Some(DiffCommentAnchor {
                    path: path.into(),
                    target: GitDiffTarget::Unstaged,
                    start: line.parse().unwrap_or(1),
                    end: line.parse().unwrap_or(1),
                    start_deleted: false,
                    end_deleted: false,
                });
                self.handle_action(&SidebarAction::DiffCommentFinish, ctx);
            }
            "text" => {
                let text = rest.to_string();
                ctx.update_view(&self.diff_comment_editor, |editor, ec| {
                    editor.set_text("", ec);
                    editor.insert_text_at_cursor(&text, ec);
                });
            }
            "agents" => self.handle_action(&SidebarAction::DiffCommentMenu(ComposerMenu::Agents), ctx),
            "choose" => self.handle_action(&SidebarAction::DiffCommentChoose(rest.to_string()), ctx),
            "send" => self.handle_action(&SidebarAction::DiffCommentSend, ctx),
            _ => {}
        }
        let comment = self.diff_comment.as_ref();
        serde_json::json!({
            "ok": true,
            "open": comment.is_some(),
            "text": comment.map(|c| c.text.clone()),
            "selected": comment.and_then(|c| c.selected.clone()),
            "menu": comment.is_some_and(|c| c.menu.is_some()),
            "sending": comment.is_some_and(|c| c.sending),
            "error": comment.and_then(|c| c.error.clone()),
            "agents": comment.map_or_else(Vec::new, |c| {
                c.agents
                    .iter()
                    .map(|a| serde_json::json!({"key": a.key, "title": a.title, "existing": a.existing}))
                    .collect()
            }),
        })
    }
}

fn agent_icon(agent: &DiffCommentAgent, theme: &Theme, size: f32) -> Box<dyn Element> {
    crate::workspace::agent_target_image(Some(agent), theme, size)
}

fn one_line(text: String, ui: FamilyId, size: f32, color: warpui::color::ColorU) -> Box<dyn Element> {
    Text::new(text, ui, crate::theme::text_size(size))
        .with_color(color)
        .soft_wrap(false)
        .with_clip(ClipConfig::ellipsis())
        .finish()
}

impl ProjectSidebarView {
    /// A compact control in the card's footer: icon, label, chevron. It is
    /// never wider than `max_width`; a long label ends in an ellipsis.
    fn composer_chip(
        &self,
        key: &str,
        icon: Box<dyn Element>,
        label: String,
        open: bool,
        max_width: f32,
        action: SidebarAction,
    ) -> Box<dyn Element> {
        let (theme, ui, id) = (self.theme, self.ui_font, self.view_id);
        let chip = Hoverable::new(self.comment_mouse(key), move |state| {
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(icon)
                    .with_child(
                        warpui::elements::Shrinkable::new(
                            1.0,
                            one_line(label, ui, 12.0, theme.main_text()),
                        )
                        .finish(),
                    )
                    .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 10.0))
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(7.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if open || state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
        .finish();
        // The label shrinks, so the chip needs a bounded width to shrink in.
        ConstrainedBox::new(chip)
            .with_max_width(crate::theme::scaled(max_width))
            .finish()
    }

    fn composer_button(&self, key: &str, label: &str, primary: bool, enabled: bool, action: SidebarAction) -> Box<dyn Element> {
        let (theme, ui, id) = (self.theme, self.ui_font, self.view_id);
        let label = label.to_string();
        let shortcut = if cfg!(target_os = "macos") { "⌘↵" } else { "Ctrl↵" };
        let button = Hoverable::new(self.comment_mouse(key), move |state| {
            let (background, foreground) = match (primary, enabled) {
                (true, true) => (
                    if state.is_hovered() {
                        crate::workspace::panel_nav::with_alpha(theme.accent, 0.85)
                    } else {
                        theme.accent
                    },
                    theme.background,
                ),
                (true, false) => (theme.surface_2(), theme.disabled_text()),
                (false, _) => (
                    if state.is_hovered() {
                        theme.surface_2()
                    } else {
                        warpui::color::ColorU::transparent_black()
                    },
                    theme.sub_text(),
                ),
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    Text::new(label.clone(), ui, crate::theme::text_size(12.0))
                        .with_color(foreground)
                        .finish(),
                );
            if primary {
                row = row.with_child(
                    Text::new(shortcut.to_string(), ui, crate::theme::text_size(10.0))
                        .with_color(crate::workspace::panel_nav::with_alpha(foreground, 0.7))
                        .finish(),
                );
            }
            Container::new(row.finish())
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish()
        });
        if enabled {
            button
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
                .finish()
        } else {
            button.finish()
        }
    }

    /// One row in a composer menu: icon, title, detail, and a check when chosen.
    fn menu_row(
        &self,
        key: &str,
        icon: Box<dyn Element>,
        title: String,
        detail: String,
        chosen: bool,
        active: bool,
        action: SidebarAction,
    ) -> Box<dyn Element> {
        let (theme, ui, id) = (self.theme, self.ui_font, self.view_id);
        Hoverable::new(self.comment_mouse(key), move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(icon)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Flex::column()
                                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                                .with_spacing(1.0)
                                .with_child(one_line(title, ui, 12.5, theme.main_text()))
                                .with_child(one_line(detail, ui, 11.0, theme.sub_text()))
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(if chosen {
                        render_icon(AppIcon::Check, theme.accent, 12.0)
                    } else {
                        Empty::new().finish()
                    })
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(if active || state.is_hovered() {
                theme.surface_2()
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
        .finish()
    }

    fn menu_heading(&self, text: &str) -> Box<dyn Element> {
        Container::new(one_line(text.to_string(), self.ui_font, 10.5, self.theme.disabled_text()))
            .with_padding_left(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(3.0)
            .finish()
    }

    /// A floating menu under a chip; arrow keys, Enter and Esc work in it.
    fn menu_panel(&self, content: Box<dyn Element>, width: f32) -> Box<dyn Element> {
        let id = self.view_id;
        let panel = ConstrainedBox::new(
            Container::new(content)
                .with_uniform_padding(6.0)
                .with_background_color(self.theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(self.theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                    warpui::color::ColorU::new(0, 0, 0, 90),
                ))
                .finish(),
        )
        .with_width(crate::theme::scaled(width))
        .finish();
        EventHandler::new(panel)
            .with_always_handle()
            .on_keydown(move |ctx, _, key| {
                let action = match key.key.as_str() {
                    "up" => SidebarAction::DiffCommentPickerMove(-1),
                    "down" => SidebarAction::DiffCommentPickerMove(1),
                    "enter" if !key.cmd && !key.ctrl => SidebarAction::DiffCommentPickerAccept,
                    "escape" => SidebarAction::DiffCommentPickerDismiss,
                    _ => return DispatchEventResult::PropagateToParent,
                };
                ctx.dispatch_typed_action_for(id, action);
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// The shared agent menu (running sessions, then new agents), with this
    /// card's arrow keys, Enter and Esc.
    fn agent_menu(&self, c: &DiffComment) -> Box<dyn Element> {
        let id = self.view_id;
        let menu = crate::workspace::render_agent_target_menu(crate::workspace::AgentTargetMenuProps {
            theme: &self.theme,
            ui: self.ui_font,
            states: &self.diff_comment_mouse,
            scroll: &c.scroll,
            search_editor: &self.diff_comment_search,
            search: &c.search,
            targets: &c.agents,
            selected: c.selected.as_deref(),
            active: Some(c.menu_active),
            pick: std::rc::Rc::new(move |ctx, key| {
                ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentChoose(key))
            }),
            clear_search: std::rc::Rc::new(move |ctx| {
                ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentClearSearch)
            }),
        });
        EventHandler::new(menu)
            .with_always_handle()
            .on_keydown(move |ctx, _, key| {
                let action = match key.key.as_str() {
                    "up" => SidebarAction::DiffCommentPickerMove(-1),
                    "down" => SidebarAction::DiffCommentPickerMove(1),
                    "enter" if !key.cmd && !key.ctrl => SidebarAction::DiffCommentPickerAccept,
                    "escape" => SidebarAction::DiffCommentPickerDismiss,
                    _ => return DispatchEventResult::PropagateToParent,
                };
                ctx.dispatch_typed_action_for(id, action);
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    fn placement_menu(&self, c: &DiffComment) -> Box<dyn Element> {
        let theme = self.theme;
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(self.menu_heading("Open the new session in"));
        for (index, placement) in Placement::ALL.into_iter().enumerate() {
            list = list.with_child(self.menu_row(
                &format!("diff_comment_place_{}", placement.key()),
                render_icon(placement.icon(), theme.sub_text(), crate::theme::scaled(15.0)),
                placement.label().into(),
                placement.detail().into(),
                c.placement == placement,
                index == c.menu_active,
                SidebarAction::DiffCommentPlacement(placement),
            ));
        }
        self.menu_panel(list.finish(), 290.0)
    }

    fn with_menu(&self, anchor: Box<dyn Element>, menu: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(anchor)
            .with_positioned_overlay_child(
                menu,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, 4.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            )
            .finish()
    }

    fn render_composer(&self, c: &DiffComment) -> Box<dyn Element> {
        let (theme, ui, id) = (self.theme, self.ui_font, self.view_id);
        let file = c
            .anchor
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let close = Hoverable::new(self.comment_mouse("diff_comment_close"), move |state| {
            Container::new(render_icon(
                AppIcon::XClose,
                if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                11.0,
            ))
            .with_uniform_padding(3.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentCancel))
        .finish();
        let header = ConstrainedBox::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::MessageSquare, theme.accent, 12.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        one_line(
                            format!("{} · {file}", c.range_label()),
                            ui,
                            11.5,
                            theme.sub_text(),
                        ),
                    )
                    .finish(),
                )
                .with_child(one_line("Esc".into(), ui, 10.0, theme.disabled_text()))
                .with_child(close)
                .finish(),
        )
        .with_height(crate::theme::scaled(HEADER_HEIGHT))
        .finish();

        let editor = ConstrainedBox::new(ChildView::new(&self.diff_comment_editor).finish())
            .with_height(crate::theme::scaled(EDITOR_HEIGHT))
            .finish();

        // Who gets it, and (for a new agent) where it opens.
        let chosen = c.chosen();
        let agent_label = chosen
            .map(|agent| {
                if agent.existing {
                    agent.title.clone()
                } else {
                    format!("New {}", agent.name)
                }
            })
            .unwrap_or_else(|| "Pick an agent".into());
        let agent_chip_icon = match chosen {
            Some(agent) => agent_icon(agent, &theme, crate::theme::scaled(14.0)),
            None => render_icon(AppIcon::Agent, theme.sub_text(), crate::theme::scaled(14.0)),
        };
        let agent_open = c.menu == Some(ComposerMenu::Agents);
        let mut agent_chip = self.composer_chip(
            "diff_comment_agent_chip",
            agent_chip_icon,
            agent_label,
            agent_open,
            220.0,
            SidebarAction::DiffCommentMenu(ComposerMenu::Agents),
        );
        if agent_open {
            agent_chip = self.with_menu(agent_chip, self.agent_menu(c));
        }
        let mut choices = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(warpui::elements::Shrinkable::new(1.0, agent_chip).finish());
        if chosen.is_some_and(|agent| !agent.existing) {
            let placement_open = c.menu == Some(ComposerMenu::Placement);
            let mut placement_chip = self.composer_chip(
                "diff_comment_place_chip",
                render_icon(c.placement.icon(), theme.sub_text(), crate::theme::scaled(13.0)),
                c.placement.label().into(),
                placement_open,
                140.0,
                SidebarAction::DiffCommentMenu(ComposerMenu::Placement),
            );
            if placement_open {
                placement_chip = self.with_menu(placement_chip, self.placement_menu(c));
            }
            choices = choices.with_child(placement_chip);
        }
        let choices = ConstrainedBox::new(choices.finish())
            .with_height(crate::theme::scaled(ROW_HEIGHT))
            .finish();

        let actions = ConstrainedBox::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    Expanded::new(1.0, one_line(c.outcome(), ui, 11.0, theme.sub_text())).finish(),
                )
                .with_child(self.composer_button(
                    "diff_comment_cancel",
                    "Cancel",
                    false,
                    !c.sending,
                    SidebarAction::DiffCommentCancel,
                ))
                .with_child(self.composer_button(
                    "diff_comment_send",
                    if c.sending { "Sending" } else { "Send" },
                    true,
                    c.can_send(),
                    SidebarAction::DiffCommentSend,
                ))
                .finish(),
        )
        .with_height(crate::theme::scaled(ROW_HEIGHT))
        .finish();

        let mut body = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(crate::theme::scaled(GAP))
            .with_child(header)
            .with_child(editor)
            .with_child(choices)
            .with_child(actions);
        let card = Container::new(body.finish())
            .with_uniform_padding(crate::theme::scaled(CARD_PADDING))
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(
                warpui::color::ColorU::new(0, 0, 0, 70),
            ))
            .finish();
        let card = ConstrainedBox::new(
            Container::new(card)
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(crate::theme::scaled(CARD_MARGIN))
                .with_padding_bottom(crate::theme::scaled(CARD_MARGIN))
                .finish(),
        )
        .with_height(c.height())
        .finish();
        EventHandler::new(card)
            .on_keydown(move |ctx, _, key| {
                if key.key == "escape" {
                    ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentCancel);
                    return DispatchEventResult::StopPropagation;
                }
                if key.key == "enter" && (key.cmd || key.ctrl) {
                    ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentSend);
                    return DispatchEventResult::StopPropagation;
                }
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }
}

pub(crate) fn comment_gutter(
    content: Box<dyn Element>,
    path: &Path,
    target: GitDiffTarget,
    line: usize,
    deleted: bool,
    id: EntityId,
) -> Box<dyn Element> {
    let start_path = path.to_path_buf();
    EventHandler::new(content)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                id,
                SidebarAction::DiffCommentStart {
                    path: start_path.clone(),
                    target,
                    line,
                    deleted,
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish()
}

pub(crate) fn comment_row(
    content: Box<dyn Element>,
    path: &Path,
    target: GitDiffTarget,
    line: usize,
    deleted: bool,
    id: EntityId,
) -> Box<dyn Element> {
    let drag_path = path.to_path_buf();
    let path = path.to_path_buf();
    EventHandler::new(content)
        .on_mouse_dragged(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                id,
                SidebarAction::DiffCommentExtend {
                    path: drag_path.clone(),
                    target,
                    line,
                    deleted,
                },
            );
            DispatchEventResult::PropagateToParent
        })
        .on_mouse_in(
            move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    id,
                    SidebarAction::DiffCommentExtend {
                        path: path.clone(),
                        target,
                        line,
                        deleted,
                    },
                );
                DispatchEventResult::PropagateToParent
            },
            None,
        )
        .on_left_mouse_up(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, SidebarAction::DiffCommentFinish);
            DispatchEventResult::PropagateToParent
        })
        .finish()
}

fn line_key(path: &Path, target: GitDiffTarget, line: usize, deleted: bool) -> String {
    format!("diff_line:{}:{target:?}:{line}:{deleted}", path.display())
}

impl ProjectSidebarView {
    pub(crate) fn comment_for_file(
        &self,
        path: &Path,
        target: GitDiffTarget,
    ) -> Option<&DiffComment> {
        self.diff_comment
            .as_ref()
            .filter(|c| c.matches(path, target))
    }

    pub(crate) fn render_comment_after(
        &self,
        path: &Path,
        target: GitDiffTarget,
        line: usize,
        deleted: bool,
    ) -> Option<Box<dyn Element>> {
        let c = self.comment_for_file(path, target)?;
        if c.anchor.end != line || c.anchor.end_deleted != deleted {
            return None;
        }
        Some(self.render_composer(c))
    }

    pub(crate) fn diff_comment_row(
        &self,
        row: Box<dyn Element>,
        path: &Path,
        target: GitDiffTarget,
        line: usize,
        deleted: bool,
    ) -> Box<dyn Element> {
        let anchor = self
            .diff_comment_drag
            .as_ref()
            .or_else(|| self.diff_comment.as_ref().map(|c| &c.anchor));
        let selected = anchor.is_some_and(|a| {
            a.path == path
                && a.target == target
                && line >= a.start.min(a.end)
                && line <= a.start.max(a.end)
                && (a.start_deleted != a.end_deleted || deleted == a.start_deleted)
        });
        let row = if selected {
            Container::new(row)
                .with_background_color(crate::workspace::panel_nav::with_alpha(
                    self.theme.accent,
                    0.16,
                ))
                .finish()
        } else {
            row
        };
        // Hovering anywhere on the line reveals its gutter +.
        let row = Hoverable::new(
            self.comment_mouse(&line_key(path, target, line, deleted)),
            move |_| row,
        )
        .finish();
        comment_row(row, path, target, line, deleted, self.view_id)
    }

    pub(crate) fn diff_comment_gutter(
        &self,
        path: &Path,
        target: GitDiffTarget,
        line: usize,
        deleted: bool,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.ui_font;
        let line_hovered = self
            .comment_mouse(&line_key(path, target, line, deleted))
            .lock()
            .map(|state| state.is_hovered())
            .unwrap_or(false);
        let plus_key = format!("{}:plus", line_key(path, target, line, deleted));
        let button = Hoverable::new(self.comment_mouse(&plus_key), move |state| {
            let shown = line_hovered || state.is_hovered();
            let mark = if shown {
                Container::new(render_icon(
                    AppIcon::Plus,
                    if state.is_hovered() {
                        theme.background
                    } else {
                        theme.accent
                    },
                    crate::theme::scaled(10.0),
                ))
                .with_uniform_padding(2.0)
                .with_background_color(if state.is_hovered() {
                    theme.accent
                } else {
                    crate::workspace::panel_nav::with_alpha(theme.accent, 0.18)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            } else {
                Empty::new().finish()
            };
            let button = ConstrainedBox::new(warpui::elements::Align::new(mark).finish())
                .with_width(crate::theme::scaled(18.0))
                .with_height(crate::theme::scaled(18.0))
                .finish();
            super::sidebar_explorer::buffer_search_tooltip(
                button,
                state.is_hovered(),
                "Comment for an agent · drag to cover more lines",
                &theme,
                font,
            )
        })
        .with_cursor(Cursor::PointingHand)
        .finish();
        comment_gutter(button, path, target, line, deleted, self.view_id)
    }

    pub(crate) fn diff_comment_context_row(
        &self,
        row: Box<dyn Element>,
        file: &crate::git_status::diff_head::GitFileDiff,
        line: usize,
    ) -> Box<dyn Element> {
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.diff_comment_gutter(&file.relative_path, file.target, line, false))
            .with_child(Expanded::new(1.0, row).finish())
            .finish();
        let row = self.diff_comment_row(row, &file.relative_path, file.target, line, false);
        if let Some(composer) =
            self.render_comment_after(&file.relative_path, file.target, line, false)
        {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(row)
                .with_child(composer)
                .finish()
        } else {
            row
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(key: &str, title: &str, existing: bool) -> DiffCommentAgent {
        DiffCommentAgent {
            key: key.into(),
            name: "Claude Code".into(),
            title: title.into(),
            detail: if existing { "Claude Code · running".into() } else { "New session".into() },
            agent: "claude".into(),
            existing,
            image: None,
        }
    }

    #[test]
    fn placement_follows_saved_choice_and_old_setting() {
        let mut settings = crate::setup::UiSettings::default();
        assert_eq!(Placement::from_settings(&settings), Placement::Split);
        settings.diff_comment_new_tab = true;
        assert_eq!(Placement::from_settings(&settings), Placement::Tab);
        settings.diff_comment_placement = Some("split".into());
        assert_eq!(Placement::from_settings(&settings), Placement::Split);
    }

    #[test]
    fn running_sessions_list_before_new_agents_and_search_narrows() {
        let comment = DiffComment {
            anchor: DiffCommentAnchor {
                path: "src/a.rs".into(),
                target: GitDiffTarget::Unstaged,
                start: 3,
                end: 7,
                start_deleted: false,
                end_deleted: false,
            },
            workspace: None,
            context_hunk: None,
            root: "/repo".into(),
            text: "Rename this".into(),
            agents: vec![
                agent("new:claude", "Claude Code", false),
                agent("existing:1", "fix-login", true),
            ],
            selected: Some("new:claude".into()),
            selection_override: false,
            menu: None,
            menu_active: 0,
            search: String::new(),
            placement: Placement::Tab,
            sending: false,
            error: None,
            generation: uuid::Uuid::new_v4(),
            scroll: ClippedScrollStateHandle::default(),
        };
        let order: Vec<_> = ProjectSidebarView::visible_agents(&comment)
            .iter()
            .map(|a| a.key.as_str())
            .collect();
        assert_eq!(order, ["existing:1", "new:claude"]);
        assert_eq!(comment.prompt(), "In src/a.rs:L3-L7: Rename this");
        assert_eq!(comment.outcome(), "Starts a new Claude Code session in a new tab");
        assert!(comment.can_send());

        let narrowed = DiffComment { search: "login".into(), ..comment };
        assert_eq!(ProjectSidebarView::visible_agents(&narrowed).len(), 1);
    }
}

#[cfg(test)]
mod layout_tests {
    use std::collections::HashSet;

    use warpui::geometry::vector::Vector2F;
    use warpui::{AppContext, View, ViewHandle};
    use warpui_core::platform::WindowStyle;
    use warpui_core::{App, Presenter, TypedActionView, WindowInvalidation};

    use super::*;
    use crate::workspace::project_sidebar::model::ProjectSidebarModel;

    /// Draws the real sidebar view's composer card at a given width.
    struct Harness {
        sidebar: ViewHandle<ProjectSidebarView>,
        width: f32,
    }

    impl Entity for Harness {
        type Event = ();
    }

    impl View for Harness {
        fn ui_name() -> &'static str {
            "DiffCommentHarness"
        }

        fn render(&self, app: &AppContext) -> Box<dyn Element> {
            let sidebar = self.sidebar.as_ref(app);
            let Some(comment) = sidebar.diff_comment.as_ref() else {
                return Empty::new().finish();
            };
            ConstrainedBox::new(sidebar.render_composer(comment))
                .with_width(self.width)
                .finish()
        }
    }

    impl TypedActionView for Harness {
        type Action = ();
    }

    fn agent(key: &str, existing: bool) -> DiffCommentAgent {
        DiffCommentAgent {
            key: key.into(),
            name: "Claude Code".into(),
            title: if existing { "fix-login-flow-for-sso-users".into() } else { "Claude Code".into() },
            detail: if existing { "Claude Code · running here".into() } else { "Start a new session".into() },
            agent: "claude".into(),
            existing,
            image: None,
        }
    }

    fn comment(selected: Option<&str>, menu: Option<ComposerMenu>, error: bool) -> DiffComment {
        DiffComment {
            anchor: DiffCommentAnchor {
                path: "src/very/long/path/to/a/module_with_a_long_name.rs".into(),
                target: GitDiffTarget::Unstaged,
                start: 12,
                end: 18,
                start_deleted: false,
                end_deleted: false,
            },
            workspace: None,
            context_hunk: None,
            root: "/repo".into(),
            text: "Please rename this and add a test".into(),
            agents: vec![agent("existing:1", true), agent("new:claude", false)],
            selected: selected.map(str::to_string),
            selection_override: false,
            menu,
            menu_active: 0,
            search: String::new(),
            placement: Placement::Split,
            sending: false,
            error: error.then(|| "That session has ended; pick another".into()),
            generation: uuid::Uuid::new_v4(),
            scroll: ClippedScrollStateHandle::default(),
        }
    }

    /// Lay the card out in every state; a layout mistake (such as an
    /// unbounded flexible child) panics here instead of in the app.
    #[test]
    fn the_card_lays_out_in_every_state_and_width() {
        let states = [
            comment(None, None, false),
            comment(Some("new:claude"), None, false),
            comment(Some("new:claude"), Some(ComposerMenu::Placement), false),
            comment(Some("existing:1"), Some(ComposerMenu::Agents), true),
        ];
        for width in [420.0_f32, 240.0] {
            for state in states.clone() {
                App::test((), move |mut app| async move {
                    let app = &mut app;
                    let (window_id, harness) =
                        app.add_window(WindowStyle::NotStealFocus, move |ctx| {
                            let model = ctx.add_model(|_| ProjectSidebarModel::new());
                            let view_id = ctx.view_id();
                            let sidebar = ctx.add_typed_action_view(|c| {
                                ProjectSidebarView::new(
                                    model,
                                    view_id,
                                    Theme::dark(),
                                    FamilyId(0),
                                    FamilyId(0),
                                    Default::default(),
                                    c,
                                )
                            });
                            Harness { sidebar, width }
                        });
                    let sidebar = harness.read(app, |h, _| h.sidebar.clone());
                    sidebar.update(app, |view, _| view.diff_comment = Some(state));
                    let mut presenter = Presenter::new(window_id);
                    let invalidation = WindowInvalidation {
                        updated: HashSet::from([app.root_view_id(window_id).unwrap()]),
                        ..Default::default()
                    };
                    app.update(move |ctx| {
                        presenter.invalidate(invalidation, ctx);
                        presenter.build_scene(Vector2F::new(800.0, 600.0), 1.0, None, ctx);
                    });
                });
            }
        }
    }

    /// The workspace draws the code editor's diff (and the card in it) from
    /// this view's state, so opening a comment must tell it to repaint.
    #[test]
    fn opening_a_comment_signals_views_that_draw_it() {
        App::test((), |mut app| async move {
            let app = &mut app;
            let (_, harness) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                let model = ctx.add_model(|_| ProjectSidebarModel::new());
                let view_id = ctx.view_id();
                let sidebar = ctx.add_typed_action_view(|c| {
                    ProjectSidebarView::new(
                        model,
                        view_id,
                        Theme::dark(),
                        FamilyId(0),
                        FamilyId(0),
                        Default::default(),
                        c,
                    )
                });
                Harness { sidebar, width: 420.0 }
            });
            let signals = std::rc::Rc::new(std::cell::Cell::new(0));
            let sidebar = harness.read(app, |h, _| h.sidebar.clone());
            harness.update(app, |_, ctx| {
                let signals = signals.clone();
                ctx.subscribe_to_view(&sidebar, move |_, _, _, _| signals.set(signals.get() + 1));
            });
            sidebar.update(app, |view, ctx| {
                view.begin_diff_comment(
                    "src/lib.rs".into(),
                    GitDiffTarget::Unstaged,
                    3,
                    5,
                    false,
                    false,
                    ctx,
                )
            });
            assert!(signals.get() > 0, "opening the card didn't signal");
            let before = signals.get();
            sidebar.update(app, |view, ctx| {
                view.handle_diff_comment_sidebar_action(
                    &SidebarAction::DiffCommentMenu(ComposerMenu::Placement),
                    ctx,
                );
            });
            assert!(signals.get() > before, "opening a menu didn't signal");
        });
    }
}
