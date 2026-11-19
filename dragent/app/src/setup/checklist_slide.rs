//! A two-panel setup screen, using Dragent's theme and Stellar artwork.

use warpui::color::ColorU;
use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    Image, MainAxisSize, ParentElement, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{CornerRadius, Radius};

use crate::theme::Theme;

use super::browser_auth_view::render_browser_auth_view;
use super::chrome::render_logo;
use super::git_provider::GitProvider;
use super::onboarding_flow::{GitCliState, OnboardingAction};

pub struct ChecklistViewState<'a> {
    pub account_connected: bool,
    pub browser_auth_active: bool,
    pub auth_error: Option<&'a str>,
    pub provider: GitProvider,
    pub git: &'a GitCliState,
}

pub fn render_checklist(
    theme: &Theme,
    font: FamilyId,
    flow_id: EntityId,
    state: ChecklistViewState<'_>,
    hover_for: impl Fn(&str) -> MouseStateHandle,
    back_hover: MouseStateHandle,
    finish_hover: MouseStateHandle,
) -> Box<dyn Element> {
    let ready =
        u8::from(state.account_connected) + u8::from(matches!(state.git, GitCliState::Connected));
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(Expanded::new(0.9, story_panel(theme, font, ready)).finish())
        .with_child(
            Expanded::new(
                1.3,
                setup_panel(
                    theme,
                    font,
                    flow_id,
                    &state,
                    &hover_for,
                    back_hover,
                    finish_hover,
                    ready,
                ),
            )
            .finish(),
        )
        .finish()
}

fn story_panel(theme: &Theme, font: FamilyId, ready: u8) -> Box<dyn Element> {
    let image = Image::new(
        AssetSource::Bundled {
            path: "dragent/stellar_bg.jpg",
        },
        CacheOption::Original,
    )
    .cover()
    .finish();
    let tint = Container::new(Empty::new().finish())
        .with_background_color(ColorU::new(
            theme.background.r,
            theme.background.g,
            theme.background.b,
            170,
        ))
        .finish();
    let content = Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(render_logo(52.0))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(label("Everything", font, 35.0, theme.foreground))
            .with_child(label("starts here.", font, 35.0, theme.foreground))
            .with_child(
                Container::new(label(
                    "Connect the essentials, then make this workspace yours.",
                    font,
                    14.0,
                    theme.sub_text(),
                ))
                .with_padding_top(14.0)
                .finish(),
            )
            .with_child(
                Container::new(label(
                    &format!("0{ready} / 02  CONNECTIONS READY"),
                    font,
                    11.0,
                    theme.accent,
                ))
                .with_padding_top(44.0)
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(52.0)
    .with_padding_right(40.0)
    .with_padding_top(52.0)
    .with_padding_bottom(50.0)
    .finish();
    Container::new(
        Clipped::new(
            Stack::new()
                .with_child(image)
                .with_child(tint)
                .with_child(content)
                .finish(),
        )
        .finish(),
    )
    .with_background_color(theme.solid_bg())
    .with_border(Border::right(1.0).with_border_color(theme.outline()))
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn setup_panel(
    theme: &Theme,
    font: FamilyId,
    flow_id: EntityId,
    state: &ChecklistViewState<'_>,
    hover_for: &impl Fn(&str) -> MouseStateHandle,
    back_hover: MouseStateHandle,
    finish_hover: MouseStateHandle,
    ready: u8,
) -> Box<dyn Element> {
    let header = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(10.0)
        .with_child(label("DRAGENT  /  SETUP", font, 11.0, theme.accent))
        .with_child(label("Ready your workspace", font, 27.0, theme.foreground))
        .with_child(label(
            "Connect Dragent and choose the Git provider you use.",
            font,
            13.0,
            theme.sub_text(),
        ))
        .finish();
    let cards = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(account_card(theme, font, flow_id, state, hover_for))
        .with_child(provider_card(
            theme,
            font,
            flow_id,
            state.provider,
            state.git,
            hover_for,
        ))
        .finish();
    let footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(action_button(
            theme,
            font,
            flow_id,
            back_hover,
            "← Back",
            OnboardingAction::Back,
            false,
        ))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(if ready == 2 {
            action_button(
                theme,
                font,
                flow_id,
                finish_hover,
                "Open Dragent  →",
                OnboardingAction::Finish,
                true,
            )
        } else {
            Container::new(label(
                "Connect your account and chosen provider",
                font,
                12.0,
                theme.sub_text(),
            ))
            .with_background_color(theme.surface_2())
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(11.0)
            .with_padding_bottom(11.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .finish();
    let content = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header)
        .with_child(Container::new(cards).with_padding_top(28.0).finish())
        .with_child(
            Container::new(label(
                &format!("{ready} of 2 connections complete"),
                font,
                12.0,
                theme.sub_text(),
            ))
            .with_padding_top(26.0)
            .with_padding_bottom(12.0)
            .finish(),
        )
        .with_child(footer)
        .finish();
    Container::new(Align::new(ConstrainedBox::new(content).with_max_width(570.0).finish()).finish())
        .with_background_color(theme.solid_bg())
        .with_padding_left(36.0)
        .with_padding_right(36.0)
        .with_padding_top(36.0)
        .with_padding_bottom(36.0)
        .finish()
}

fn account_card(
    theme: &Theme,
    font: FamilyId,
    flow_id: EntityId,
    state: &ChecklistViewState<'_>,
    hover_for: &impl Fn(&str) -> MouseStateHandle,
) -> Box<dyn Element> {
    let description = if state.account_connected {
        "Your Dragent account is connected."
    } else if state.browser_auth_active {
        "Finish signing in through your browser."
    } else {
        "Sign in to connect your work and cloud agents."
    };
    let action = if state.account_connected {
        status_pill(theme, font, "Connected", true)
    } else if state.browser_auth_active {
        status_pill(theme, font, "Waiting", false)
    } else {
        action_button(
            theme,
            font,
            flow_id,
            hover_for("signin"),
            "Sign in  ↗",
            OnboardingAction::StartBrowserAuth,
            true,
        )
    };
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(card_top(
            theme,
            font,
            "01",
            "Dragent account",
            description,
            action,
        ));
    if state.browser_auth_active {
        body = body.with_child(render_browser_auth_view(
            theme,
            font,
            flow_id,
            state.auth_error,
            hover_for("copy_link"),
            hover_for("paste_callback"),
            hover_for("cancel_browser"),
        ));
    } else if let Some(error) = state.auth_error {
        body = body.with_child(label(error, font, 12.0, theme.ansi_red()));
    }
    card(theme, body.finish())
}

fn provider_card(
    theme: &Theme,
    font: FamilyId,
    flow_id: EntityId,
    provider: GitProvider,
    state: &GitCliState,
    hover_for: &impl Fn(&str) -> MouseStateHandle,
) -> Box<dyn Element> {
    let name = provider.label();
    let (description, action) = match state {
        GitCliState::Checking => (
            format!("Checking for {name} CLI on this machine."),
            status_pill(theme, font, "Checking", false),
        ),
        GitCliState::Missing => (
            format!(
                "Install {} to connect your {name} repositories.",
                provider.cli()
            ),
            action_button(
                theme,
                font,
                flow_id,
                hover_for("github_action"),
                "Install",
                OnboardingAction::InstallGitCli,
                true,
            ),
        ),
        GitCliState::Installing => (
            format!("Installing {name} CLI. This may take a moment."),
            status_pill(theme, font, "Installing", false),
        ),
        GitCliState::ReadyToConnect => (
            "Sign in through the built-in terminal on this machine.".into(),
            action_button(
                theme,
                font,
                flow_id,
                hover_for("github_action"),
                "Connect",
                OnboardingAction::ConnectGitCli,
                true,
            ),
        ),
        GitCliState::AwaitingAuthentication => (
            format!("Finish {name} sign-in, then check your connection."),
            action_button(
                theme,
                font,
                flow_id,
                hover_for("github_action"),
                "Verify",
                OnboardingAction::CheckGitCli,
                true,
            ),
        ),
        GitCliState::Connected => (
            format!("Your {name} repositories are connected on this machine."),
            status_pill(theme, font, "Connected", true),
        ),
        GitCliState::Error(_) => (
            format!("{name} setup needs attention."),
            action_button(
                theme,
                font,
                flow_id,
                hover_for("github_action"),
                "Retry",
                OnboardingAction::CheckGitCli,
                true,
            ),
        ),
    };
    let mut choices = Flex::row().with_spacing(10.0);
    for choice in GitProvider::ALL {
        choices = choices.with_child(action_button(
            theme,
            font,
            flow_id,
            hover_for(if choice == GitProvider::Github {
                "provider_github"
            } else {
                "provider_gitlab"
            }),
            choice.label(),
            OnboardingAction::SelectGitProvider(choice),
            choice == provider,
        ));
    }
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(card_top(
            theme,
            font,
            "02",
            "Git provider",
            "Choose one to continue. You can connect both later.",
            choices.finish(),
        ))
        .with_child(card_top(
            theme,
            font,
            "",
            &format!("{name} CLI"),
            &description,
            action,
        ));
    if provider == GitProvider::Gitlab {
        body = body.with_child(label(
            "Supports GitLab.com and self-managed GitLab. Choose your host in the sign-in prompts.",
            font,
            12.0,
            theme.sub_text(),
        ));
    }
    if let GitCliState::Error(error) = state {
        body = body.with_child(label(error, font, 12.0, theme.ansi_red()));
    }
    card(theme, body.finish())
}

fn card_top(
    theme: &Theme,
    font: FamilyId,
    number: &str,
    title: &str,
    description: &str,
    action: Box<dyn Element>,
) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(17.0)
        .with_child(if number.is_empty() {
            Empty::new().finish()
        } else {
            Container::new(label(number, font, 13.0, theme.accent))
                .with_background_color(theme.accent_bg())
                .with_padding_left(11.0)
                .with_padding_right(11.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish()
        })
        .with_child(
            Shrinkable::new(
                1.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(5.0)
                    .with_child(label(title, font, 16.0, theme.foreground))
                    .with_child(label(description, font, 12.0, theme.sub_text()))
                    .finish(),
            )
            .finish(),
        )
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(action)
        .finish()
}

fn card(theme: &Theme, content: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .with_padding_left(17.0)
        .with_padding_right(17.0)
        .with_padding_top(17.0)
        .with_padding_bottom(17.0)
        .finish()
}

fn status_pill(theme: &Theme, font: FamilyId, text: &str, done: bool) -> Box<dyn Element> {
    let color = if done {
        theme.ansi_green()
    } else {
        theme.ansi(3)
    };
    Container::new(label(text, font, 12.0, color))
        .with_background_color(theme.surface_2())
        .with_padding_left(11.0)
        .with_padding_right(11.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

fn label(text: &str, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Box::new(Text::new(text.to_string(), font, crate::theme::text_size(size)).with_color(color))
}

#[allow(clippy::too_many_arguments)]
fn action_button(
    theme: &Theme,
    font: FamilyId,
    flow_id: EntityId,
    hover: MouseStateHandle,
    text: &str,
    action: OnboardingAction,
    primary: bool,
) -> Box<dyn Element> {
    let text = text.to_string();
    let theme = *theme;
    Box::new(
        Hoverable::new(hover, move |state| {
            let background = if primary {
                if state.is_hovered() {
                    theme.foreground
                } else {
                    theme.accent
                }
            } else {
                theme.surface_1()
            };
            let foreground = if primary {
                theme.accent_text()
            } else if state.is_hovered() {
                theme.foreground
            } else {
                theme.sub_text()
            };
            Container::new(label(&text, font, 13.0, foreground))
                .with_background_color(background)
                .with_padding_left(if primary { 16.0 } else { 4.0 })
                .with_padding_right(if primary { 16.0 } else { 4.0 })
                .with_padding_top(10.0)
                .with_padding_bottom(10.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish()
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(flow_id, action.clone())),
    )
}
