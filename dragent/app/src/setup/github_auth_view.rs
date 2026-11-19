//! Git provider authentication inside Dragent's own terminal view.

use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex,
    MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::MouseStateHandle;
use warpui_core::scene::{CornerRadius, Radius};

use crate::terminal::TerminalView;
use crate::theme::Theme;

use super::chrome::{render_naked_button, render_primary_button, render_secondary_button};
use super::git_provider::GitProvider;
use super::onboarding_flow::OnboardingAction;

pub fn render_git_auth_view(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    provider: GitProvider,
    terminal: &ViewHandle<TerminalView>,
    device_code: Option<&str>,
    message: Option<&str>,
    cancel_hover: MouseStateHandle,
    retry_hover: MouseStateHandle,
    copy_hover: MouseStateHandle,
    verify_hover: MouseStateHandle,
) -> Box<dyn Element> {
    let heading = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(Box::new(
            Text::new(format!("Sign in to {}", provider.label()), ui_font, crate::theme::text_size(24.0))
                .with_color(theme.foreground),
        ))
        .with_child(Box::new(
            Text::new(
                if provider == GitProvider::Gitlab {
                    "Choose GitLab.com or your own GitLab host below, then follow its sign-in prompts. Credentials stay on this machine."
                } else {
                    "Follow the prompts below. GitHub will open in your browser to finish sign-in."
                }
                    .to_string(),
                ui_font,
                crate::theme::text_size(13.0),
            )
            .with_color(theme.sub_text()),
        ))
        .finish();

    let status = if let Some(code) = device_code {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(12.0)
            .with_child(Box::new(
                Text::new(code.to_string(), ui_font, crate::theme::text_size(18.0))
                    .with_color(theme.accent),
            ))
            .with_child(Box::new(
                Text::new(
                    "Copied · paste on GitHub".to_string(),
                    ui_font,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text()),
            ))
            .with_child(render_secondary_button(
                theme,
                ui_font,
                flow_id,
                copy_hover,
                "Copy again",
                OnboardingAction::CopyGitCode,
                false,
            ))
            .finish()
    } else {
        Box::new(
            Text::new(
                message
                    .unwrap_or(if provider == GitProvider::Gitlab {
                        "Waiting for the GitLab CLI prompt…"
                    } else {
                        "Waiting for the GitHub CLI prompt…"
                    })
                    .to_string(),
                ui_font,
                crate::theme::text_size(12.0),
            )
            .with_color(if message.is_some() {
                theme.ansi(3)
            } else {
                theme.sub_text()
            }),
        )
    };

    let terminal_frame = Container::new(
        ConstrainedBox::new(ChildView::new(terminal).finish())
            .with_height(300.0)
            .finish(),
    )
    .with_background_color(theme.solid_bg())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .finish();

    let controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(render_naked_button(
            theme,
            ui_font,
            flow_id,
            cancel_hover,
            "Cancel",
            OnboardingAction::CancelGitAuth,
        ))
        .with_child(render_secondary_button(
            theme,
            ui_font,
            flow_id,
            retry_hover,
            "Restart sign-in",
            OnboardingAction::RetryGitAuth,
            false,
        ))
        .with_child(render_primary_button(
            theme,
            ui_font,
            flow_id,
            verify_hover,
            "Check connection",
            OnboardingAction::CheckGitCli,
            false,
        ))
        .finish();

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(18.0)
        .with_child(heading)
        .with_child(status);
    if device_code.is_some() {
        if let Some(message) = message {
            body = body.with_child(Box::new(
                Text::new(message.to_string(), ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.ansi(3)),
            ));
        }
    }
    let panel = Container::new(
        ConstrainedBox::new(
            body.with_child(terminal_frame)
                .with_child(controls)
                .finish(),
        )
        .with_max_width(760.0)
        .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_uniform_padding(24.0)
    .finish();

    Container::new(Align::new(panel).finish())
        .with_background_color(theme.solid_bg())
        .finish()
}
