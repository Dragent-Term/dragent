//! The header pill for a workspace started from an issue: the issue's key
//! (and how many more were linked), opening it in the browser.

use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, ParentElement, Text,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::view::WorkspaceView;
use super::WorkspaceAction;
use crate::theme::Theme;

impl WorkspaceView {
    pub(crate) fn render_linked_issue_pill(&self, theme: &Theme) -> Option<Box<dyn Element>> {
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id))?;
        let issues = crate::integrations::links::for_workspace(&workspace.root);
        let first = issues.first()?.clone();
        let more = issues.len() - 1;
        let ui = self.ui_font;
        let view_id = self.workspace_view_id;
        let theme = *theme;
        let mouse = self
            .header_mouse_states
            .borrow_mut()
            .entry("linked_issue_pill".to_string())
            .or_default()
            .clone();
        let label = format!("{} {}", first.key, first.title);
        let url = first.url.clone();
        let pill = Hoverable::new(mouse, move |state| {
            let text = if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(crate::integrations::marks::mark(
                    &theme,
                    ui,
                    first.provider,
                    12.0,
                ))
                .with_child(
                    ConstrainedBox::new(
                        Text::new(label.clone(), ui, crate::theme::text_size(11.5))
                            .with_color(text)
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .with_max_width(180.0)
                    .finish(),
                );
            if more > 0 {
                row = row.with_child(
                    Text::new(format!("+{more}"), ui, crate::theme::text_size(11.0))
                        .with_color(theme.disabled_text())
                        .finish(),
                );
            }
            Container::new(row.finish())
                .with_background_color(if state.is_hovered() {
                    theme.surface_3()
                } else {
                    theme.surface_2()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(
                view_id,
                WorkspaceAction::OpenExternalUrl { url: url.clone() },
            )
        })
        .finish();
        Some(pill)
    }
}
