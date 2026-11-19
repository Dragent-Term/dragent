use super::{view::WorkspaceView, WorkspaceAction};
use warpui::elements::{
    ChildAnchor, DispatchEventResult, Element, EventHandler, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Stack,
};
use warpui::geometry::vector::vec2f;
use warpui::ViewContext;
use warpui_core::clipboard::ClipboardContent;

/// The popup is for errors written for the user: git failures and the
/// input bar's explanations. Anything else is logged, not shown.
const USER_FACING_TITLES: &[&str] = &[
    // Git
    "Git operation failed",
    "Git index failed",
    "Commit failed",
    "Commit & push failed",
    "Branch operation failed",
    "Branch switch failed",
    "Invalid branch name",
    "Review operation failed",
    "Couldn’t delete workspace folder",
    "Couldn’t generate commit message",
    // Input bar
    "Dictation failed",
    "Input failed",
    "Attachment failed",
    "Agent request failed",
    // Explanations
    "Agent account",
    "Pull requests",
    "Review setup",
];

impl WorkspaceView {
    pub(crate) fn show_error_notice(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let notice = (title.into(), message.into());
        if notice.1.trim().is_empty() {
            return;
        }
        if !USER_FACING_TITLES.contains(&notice.0.as_str()) {
            log::warn!("{}: {}", notice.0, notice.1);
            return;
        }
        if !self.error_notices.contains(&notice) {
            if self.error_notices.len() >= 32 {
                self.error_notices.pop_back();
            }
            self.error_notices.push_back(notice);
        }
        ctx.notify();
    }

    fn active_error_notice(&self) -> Option<(&str, &str, &str)> {
        if let Some((title, message)) = self.error_notices.front() {
            return Some(("queued", title, message));
        }
        if let Some(message) = &self.import_project_worktrees_error {
            return Some((
                "import_project_worktrees_error",
                "Couldn’t import worktrees",
                message,
            ));
        }
        if let Some(message) = &self.review_action_error {
            return Some(("review_action_error", "Git operation failed", message));
        }
        if let Some(message) = &self.create_project_error {
            return Some(("create_project_error", "Couldn’t create project", message));
        }
        if let Some(message) = &self.worktree_error {
            return Some(("worktree_error", "Worktree operation failed", message));
        }
        if let Some(message) = &self.clone_url_error {
            return Some(("clone_url_error", "Couldn’t clone repository", message));
        }
        None
    }

    pub(crate) fn handle_error_notice_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ShowErrorNotice { title, message } => {
                self.show_error_notice(title.clone(), message.clone(), ctx)
            }
            WorkspaceAction::DismissErrorNotice => {
                let source = self
                    .active_error_notice()
                    .map(|(source, _, _)| source.to_string());
                match source.as_deref() {
                    Some("queued") => {
                        self.error_notices.pop_front();
                    }
                    Some("import_project_worktrees_error") => {
                        self.import_project_worktrees_error = None
                    }
                    Some("review_action_error") => self.review_action_error = None,
                    Some("create_project_error") => self.create_project_error = None,
                    Some("worktree_error") => self.worktree_error = None,
                    Some("clone_url_error") => self.clone_url_error = None,
                    _ => {}
                }
                self.error_notice_copied = None;
                self.error_notice_scroll = Default::default();
                ctx.notify();
            }
            WorkspaceAction::CopyErrorNotice => {
                if let Some((_, _, message)) = self.active_error_notice() {
                    let text = message.to_string();
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(text.clone()));
                    self.error_notice_copied = Some(text.clone());
                    ctx.notify();
                    ctx.spawn(
                        warpui::r#async::Timer::after(std::time::Duration::from_millis(1500)),
                        move |view, _, ctx| {
                            if view.error_notice_copied.as_ref() == Some(&text) {
                                view.error_notice_copied = None;
                                ctx.notify();
                            }
                        },
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) fn overlay_error_notice(&self, root: Box<dyn Element>) -> Box<dyn Element> {
        let Some((_, title, message)) = self.active_error_notice() else {
            return root;
        };
        let popup = crate::error_notice::render(
            title,
            message,
            &self.theme,
            self.mono_font,
            self.error_notice_scroll.clone(),
            self.error_notice_copied.as_deref() == Some(message),
            self.workspace_view_id,
            WorkspaceAction::DismissErrorNotice,
            WorkspaceAction::CopyErrorNotice,
        );
        Stack::new()
            .with_child(root)
            .with_positioned_overlay_child(
                popup,
                OffsetPositioning::offset_from_parent(
                    vec2f(-12.0, -12.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomRight,
                    ChildAnchor::BottomRight,
                ),
            )
            .finish()
    }
}
