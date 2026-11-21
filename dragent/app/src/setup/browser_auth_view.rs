//! Browser wait step: the code to match in the browser, and a way back.

use warpui::elements::{
    Border, Container, CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::MouseStateHandle;
use warpui_core::scene::{CornerRadius, Radius};

use crate::theme::Theme;

use super::chrome::{render_naked_button, CAPTION_SIZE, SUBTITLE_SIZE};
use super::onboarding_flow::OnboardingAction;

pub fn render_browser_auth_view(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    auth_error: Option<&str>,
    sign_in_code: Option<&str>,
    cancel_hover: MouseStateHandle,
) -> Box<dyn Element> {
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(16.0)
        .with_child(Box::new(
            Text::new(
                "Finish in your browser".to_string(),
                ui_font,
                crate::theme::text_size(SUBTITLE_SIZE),
            )
            .with_color(theme.foreground),
        ))
        .with_child(Box::new(
            Text::new(
                "We opened a secure sign-in page. This screen will move forward when your account is connected."
                    .to_string(),
                ui_font,
                crate::theme::text_size(CAPTION_SIZE),
            )
            .with_color(theme.sub_text()),
        ));

    if let Some(code) = sign_in_code {
        body = body.with_child(render_code(theme, ui_font, code));
    }

    if let Some(error) = auth_error {
        body = body.with_child(Box::new(
            Text::new(
                error.to_string(),
                ui_font,
                crate::theme::text_size(CAPTION_SIZE),
            )
            .with_color(theme.ansi_red()),
        ));
    }

    body = body.with_child(render_naked_button(
        theme,
        ui_font,
        flow_id,
        cancel_hover,
        "← Back to setup",
        OnboardingAction::CancelBrowserAuth,
    ));

    body.finish()
}

/// The sign-in code, large and spaced out like the browser shows it, so the
/// two are easy to compare before approving.
pub(crate) fn render_code(theme: &Theme, ui_font: FamilyId, code: &str) -> Box<dyn Element> {
    let spaced = code.chars().map(String::from).collect::<Vec<_>>().join(" ");
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(Box::new(
                Text::new(
                    "YOUR CODE".to_string(),
                    ui_font,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.sub_text()),
            ))
            .with_child(Box::new(
                Text::new(spaced, ui_font, crate::theme::text_size(28.0))
                    .with_color(theme.foreground),
            ))
            .with_child(Box::new(
                Text::new(
                    "Approve in your browser only if it shows this same code.".to_string(),
                    ui_font,
                    crate::theme::text_size(CAPTION_SIZE),
                )
                .with_color(theme.sub_text()),
            ))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(20.0)
    .finish()
}
