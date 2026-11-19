//! Agent conversations have a measured document viewport, independent of the
//! shell emulator, shell block height estimates and terminal scroll position.
use super::terminal_view::TerminalView;
use crate::agent::controller::block::AgentBlockKind;
use crate::terminal::render::build_agent_block_element;
use warpui::elements::{
    ClippedScrollable, Container, CrossAxisAlignment, Element, Flex, ParentElement,
    ScrollbarWidth, SelectableArea, Text,
};
use warpui::{AppContext, ReadModel};

impl TerminalView {
    pub(crate) fn is_builtin_agent_tab(&self) -> bool {
        self.tab_session_kind == crate::agent::TabSessionKind::Agent
            && self.agent_id.as_deref().is_none_or(|id| id == "dragent")
    }

    pub(crate) fn uses_agent_transcript(&self) -> bool {
        self.is_builtin_agent_tab()
            && self.input_config.input_type == crate::agent::DetectedInputType::Agent
    }

    pub(crate) fn render_agent_transcript(&self, app: &AppContext) -> Box<dyn Element> {
        let offset = self.agent_transcript_scroll.scroll_start().as_f32();
        let (follow, last) = follow_transition(
            offset,
            self.agent_transcript_last_scroll.get(),
            self.agent_transcript_scroll.max_scroll().as_f32(),
            self.agent_transcript_follow.get(),
        );
        self.agent_transcript_follow.set(follow);
        self.agent_transcript_last_scroll.set(last);
        let theme = self.theme;
        let mut context = self.block_render_context(app);
        context.content_width_px =
            (self.output_viewport_width_px - 2.0 * crate::theme::TERMINAL_PADDING_LEFT).max(80.0);
        let (blocks, checkpoints) = self.agent_controller.read(app, |controller, _| {
            let blocks = controller.agent_blocks().to_vec();
            let checkpoints: std::collections::HashSet<usize> = blocks
                .iter()
                .filter(|b| {
                    matches!(b.kind, AgentBlockKind::UserQuery { .. })
                        && controller.has_live_checkpoint(b.id)
                })
                .map(|b| b.id)
                .collect();
            (blocks, checkpoints)
        });
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(crate::terminal::render::agent_transcript::MESSAGE_GAP);
        if blocks.is_empty() {
            column = column.with_child(Container::new(
                Flex::column().with_spacing(10.0)
                    .with_child(Text::new("What would you like to work on?".to_string(), self.mono_family, crate::theme::text_size(18.0)).with_color(theme.main_text()).finish())
                    .with_child(Text::new("Ask Dragent to explore, build, or fix something in this workspace.".to_string(), self.mono_family, crate::theme::text_size(13.0)).with_color(theme.sub_text()).finish())
                    .finish()).with_padding_top(40.0).with_padding_bottom(24.0).finish());
        }
        if !blocks.is_empty() {
            // The shared chat layout: your message, the turn timer, replies,
            // steps on the guide line, grouped steps, changed files. Cards
            // that need you keep their own renderer and buttons.
            let working = self.agent_controller.read(app, |controller, _| controller.is_busy());
            let entries = crate::chat::dragent::entries_from_blocks(&blocks);
            let lines = crate::chat::layout::layout(&entries, working, crate::chat::dragent::waiting_on_you(&blocks));
            let ui = crate::fonts::ui_role(self.mono_family);
            let extras = |id: &str| -> Option<Box<dyn Element>> {
                let id: usize = id.parse().ok()?;
                let block = blocks.iter().find(|b| b.id == id)?;
                you_extras(block, checkpoints.contains(&id), &context, theme, ui)
            };
            let empty_open = std::collections::HashSet::new();
            let view = crate::chat::view::ChatView {
                ctx: &context,
                theme,
                ui,
                mono: self.mono_family,
                now_ms: chrono::Utc::now().timestamp_millis(),
                open: if self.chat_open.is_empty() { &empty_open } else { &self.chat_open },
                you_extras: Some(&extras),
            };
            let card = |block: &crate::agent::controller::block::AgentBlock| {
                build_agent_block_element(block, &context, theme, self.mono_family, &checkpoints)
            };
            column = column.with_child(crate::chat::view::render(&lines, &view, &card));
        }
        let document = Container::new(column.finish())
            .with_padding_left(crate::theme::TERMINAL_PADDING_LEFT)
            .with_padding_right(crate::theme::TERMINAL_PADDING_LEFT)
            .with_padding_top(16.0)
            .with_padding_bottom(24.0)
            .finish();
        let selected = self.block_selected_text.clone();
        let selectable = SelectableArea::new(
            self.block_selectable_handle.clone(),
            move |args: warpui::elements::SelectionUpdateArgs,
                  _: &mut warpui::EventContext,
                  _: &AppContext| {
                *selected.write() = args.selection;
            },
            document,
        )
        .finish();
        if self.agent_transcript_follow.get() {
            self.agent_transcript_scroll
                .scroll_to(warpui_core::units::Pixels::new(f32::MAX));
        }
        ClippedScrollable::vertical(
            self.agent_transcript_scroll.clone(),
            selectable,
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.pane_surface().into(),
        )
        .with_overlayed_scrollbar()
        .finish()
    }
}

/// Under one of your messages: the restore button when its checkpoint can
/// still be restored, and the names of what you attached.
fn you_extras(
    block: &crate::agent::controller::block::AgentBlock,
    restorable: bool,
    ctx: &crate::terminal::render::BlockRenderContext<'_>,
    theme: crate::theme::Theme,
    ui: warpui::fonts::FamilyId,
) -> Option<Box<dyn Element>> {
    use crate::terminal::view::actions::TerminalAction;
    use warpui::elements::{Hoverable, SavePosition};
    let AgentBlockKind::UserQuery { attachments, .. } = &block.kind else {
        return None;
    };
    if !restorable && attachments.is_empty() {
        return None;
    }
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
    for attachment in attachments {
        row = row.with_child(
            Container::new(
                Text::new(attachment.name.clone(), ui, crate::theme::text_size(11.5)).with_color(theme.sub_text()).finish(),
            )
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .with_background_color(theme.surface_2())
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(warpui_core::scene::Radius::Pixels(5.0)))
            .finish(),
        );
    }
    if restorable {
        let block_id = block.id;
        let button = Hoverable::new(ctx.mouse_state(&format!("restore-msg-{block_id}")), move |state| {
            Container::new(crate::icons::render_icon(
                crate::icons::AppIcon::Undo2,
                if state.is_hovered() { theme.accent } else { theme.sub_text() },
                crate::icons::ICON_SIZE_SM,
            ))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
        })
        .with_cursor(warpui::platform::Cursor::PointingHand)
        .on_click(move |event, _, _| event.dispatch_typed_action(TerminalAction::ToggleCheckpointPreview { block_id }));
        // The restore preview anchors to this position.
        row = row.with_child(SavePosition::new(Box::new(button), &format!("checkpoint_btn_{block_id}")).finish());
    }
    Some(row.finish())
}

pub(crate) fn follow_transition(offset: f32, previous: f32, maximum: f32, following: bool) -> (bool, f32) {
    if offset >= 1e30 {
        return (following, previous);
    }
    let next = if offset >= maximum - 2.0 {
        true
    } else if offset < previous - 2.0 {
        false
    } else {
        following
    };
    (next, offset)
}

#[cfg(test)]
mod tests {
    use super::follow_transition;
    #[test]
    fn reading_history_does_not_snap_to_bottom_when_scrolling_down_in_the_middle() {
        assert_eq!(follow_transition(400.0, 950.0, 950.0, true), (false, 400.0));
        assert_eq!(
            follow_transition(600.0, 400.0, 950.0, false),
            (false, 600.0)
        );
        assert_eq!(follow_transition(949.0, 600.0, 950.0, false), (true, 949.0));
        assert_eq!(
            follow_transition(f32::MAX, 600.0, 950.0, false),
            (false, 600.0)
        );
    }
}
