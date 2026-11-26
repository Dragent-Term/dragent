//! Agents panel: host agent runs across open projects + modal sheet.
//!
//! Rows are resolved off-thread (`action_agents`); this module owns the row
//! shape, the alive/exit state text, and the overlay modal, following the
//! `render_close_tab_modal` overlay patterns (centered dialog, hover buttons,
//! typed-action dispatch).

use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex, Hoverable,
    MainAxisAlignment, MainAxisSize, ParentElement, Text,
};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::action::WorkspaceAction;
use super::view::WorkspaceView;
use crate::theme::Theme;

/// One host agent run row in the agents panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunRow {
    pub run_id: String,
    pub label: String,
    pub state: String,
    pub branch: String,
    pub home_path: String,
}

/// A host run's state in words, with what it holds when known.
pub fn run_state_text(state: &dragent_host::AgentRunState, memory_bytes: Option<u64>) -> String {
    use dragent_host::AgentRunState;
    let state = match state {
        AgentRunState::Waiting => "Waiting for memory".to_string(),
        AgentRunState::Paused => "Paused".to_string(),
        AgentRunState::Running => "Running".to_string(),
        AgentRunState::Exited { code: 0 } => return "Done".to_string(),
        AgentRunState::Exited { code } => return format!("Exited ({code})"),
    };
    match memory_bytes {
        Some(bytes) => format!("{state} · {}", super::agent_sleep::size_label(bytes)),
        None => state,
    }
}

impl WorkspaceView {
    /// Centered agents modal over a dimmed backdrop: one row per host run
    /// with Focus/Stop buttons, plus Refresh/Close. Empty, busy, and error
    /// states render in place of the row list.
    pub(crate) fn render_agents_modal(&self, theme: &Theme) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;

        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: String,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states.borrow_mut().entry(key).or_default().clone();
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
                        Text::new(label.clone(), ui, crate::theme::text_size(12.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
            )
        };

        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0);
        if self.agents_busy {
            list = list.with_child(
                Text::new(
                    "Loading agent runs…".to_string(),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        } else if self.agent_runs.is_empty() {
            list = list.with_child(
                Text::new(
                    "No agent runs on the host.".to_string(),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        }
        for row in &self.agent_runs {
            let focus = button(
                "Focus".to_string(),
                theme.accent,
                theme.accent,
                format!("agents_focus_{}", row.run_id),
                WorkspaceAction::FocusAgentRun {
                    run_id: row.run_id.clone(),
                },
            );
            let stop = button(
                "Stop".to_string(),
                theme.ui_red(),
                theme.ui_red(),
                format!("agents_stop_{}", row.run_id),
                WorkspaceAction::StopAgentRun {
                    run_id: row.run_id.clone(),
                },
            );
            let subtitle = if row.branch.is_empty() {
                row.home_path.clone()
            } else {
                format!("{} · {}", row.branch, row.home_path)
            };
            let texts = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(2.0)
                .with_child(
                    Text::new(
                        format!("{} · {}", row.label, row.state),
                        ui,
                        crate::theme::text_size(13.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(subtitle, ui, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish();
            let actions = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(8.0)
                .with_child(focus)
                .with_child(stop)
                .finish();
            let entry = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(Expanded::new(1.0, texts).finish())
                .with_child(actions)
                .finish();
            list = list.with_child(entry);
        }

        let footer = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Refresh".to_string(),
                theme.sub_text(),
                theme.outline(),
                "agents_refresh".to_string(),
                WorkspaceAction::RefreshAgentRuns,
            ))
            .with_child(button(
                "Close".to_string(),
                theme.sub_text(),
                theme.outline(),
                "agents_close".to_string(),
                WorkspaceAction::CloseAgentsPanel,
            ))
            .finish();

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new("Agent runs".to_string(), ui, crate::theme::text_size(15.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(list.finish())
                .with_child(Container::new(footer).with_padding_top(4.0).finish())
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
        // Narrow the dialog like the close-tab modal; the sheet stack centers it.
        let centered = ConstrainedBox::new(dialog).with_max_width(520.0).finish();
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

#[cfg(test)]
mod tests {
    use super::run_state_text;

    #[test]
    fn run_state_text_covers_alive_and_exit_shapes() {
        use dragent_host::AgentRunState::*;
        assert_eq!(run_state_text(&Running, None), "Running");
        assert_eq!(run_state_text(&Running, Some(512 << 20)), "Running · 512 MB");
        assert_eq!(run_state_text(&Paused, Some(5 << 30)), "Paused · 5.0 GB");
        assert_eq!(run_state_text(&Waiting, None), "Waiting for memory");
        assert_eq!(run_state_text(&Exited { code: 0 }, Some(1)), "Done");
        assert_eq!(run_state_text(&Exited { code: 2 }, None), "Exited (2)");
    }
}
