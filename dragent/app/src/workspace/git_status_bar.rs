//! P1 scaffold: status-bar git entry (VS Code editorStatus.ts / statusbar parity).
//!
//! Thin view that reads GitRepoStatusModel (same source as git panel) and renders
//! `$(git-branch) main ↑1 ↓2 +3 ~1` as an ordered, accessible item in the
//! workspace chrome (header/content_header). Behind `git-hunk-stage` flag's status-bar slice.

use crate::theme::Theme;
use warpui::elements::{Container, Element, Text};
use warpui::fonts::FamilyId;

pub fn render_git_status_bar(
    branch: &str,
    ahead: u32,
    behind: u32,
    changed: u32,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    // Scaffold: real impl will add icons, click -> branch picker
    Container::new(
        Text::new(
            format!("{} {} {}/{}", branch, ahead, behind, changed),
            font,
            crate::theme::text_size(10.0),
        )
        .with_color(theme.sub_text())
        .finish(),
    )
    .finish()
}
