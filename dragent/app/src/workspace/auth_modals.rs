//! Auth-related modals for the workspace: the "Welcome to Dragent" sign-in
//! prompt shown when an action that needs an account (e.g. sharing a session) is
//! taken while signed out. Styled to match the app's own modals.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element,
    EventHandler, Flex, Image, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::theme::{scaled, Theme};

use super::action::WorkspaceAction;

type States = RefCell<HashMap<String, MouseStateHandle>>;

const PANEL_W: f32 = 430.0;
const WHITE: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

const TERMS_URL: &str = "https://dragent.dev/terms";
const PRIVACY_URL: &str = "https://dragent.dev/privacy";

fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

/// The app's brand mark in a rounded square.
pub fn brand_icon(size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Image::new(
            AssetSource::Bundled {
                path: "icons/dragent-icon.png",
            },
            CacheOption::Original,
        )
        .contain()
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

/// "Welcome to Dragent" — shown when a sign-in-required action is taken while
/// signed out. Sign up / Sign in start the browser OAuth flow; Skip dismisses.
pub fn render_login_prompt(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
    pending_code: Option<&str>,
    problem: Option<&str>,
) -> Box<dyn Element> {
    if let Some(problem) = problem {
        return render_sign_in_problem(theme, ui, id, states, problem);
    }
    if let Some(code) = pending_code {
        return render_waiting_for_browser(theme, ui, id, states, code);
    }
    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(18.0)
            .with_child(brand_icon(56.0))
            .with_child(
                Text::new(
                    "Welcome to Dragent!".to_string(),
                    ui,
                    crate::theme::text_size(22.0),
                )
                .with_color(theme.main_text())
                .finish(),
            )
            .with_child(primary_button(
                "Sign up",
                "auth_signup",
                theme,
                ui,
                id,
                states,
            ))
            .with_child(link_line(
                "Already have an account? ",
                "Sign in",
                "signin",
                WorkspaceAction::SignInFromHeader,
                theme,
                ui,
                id,
                states,
            ))
            .with_child(link_line(
                "Don't want to sign in right now? ",
                "Skip for now",
                "skip",
                WorkspaceAction::CloseLoginPrompt,
                theme,
                ui,
                id,
                states,
            ))
            .with_child(footer(theme, ui, id, states))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(26.0)
    .with_padding_right(26.0)
    .with_padding_top(24.0)
    .with_padding_bottom(24.0)
    .finish();

    modal_scrim(panel, WorkspaceAction::CloseLoginPrompt, id)
}

/// A browser sign-in that didn't finish: says why, and stays until the person
/// tries again or cancels.
fn render_sign_in_problem(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
    problem: &str,
) -> Box<dyn Element> {
    let line = |text: String, size: f32, color: ColorU| {
        Text::new(text, ui, crate::theme::text_size(size))
            .with_color(color)
            .finish()
    };
    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(18.0)
            .with_child(brand_icon(56.0))
            .with_child(line(
                "Sign-in didn't finish".to_string(),
                22.0,
                theme.main_text(),
            ))
            .with_child(line(problem.to_string(), 13.0, theme.sub_text()))
            .with_child(primary_button(
                "Try again",
                "auth_retry",
                theme,
                ui,
                id,
                states,
            ))
            .with_child(link_line(
                "Don't want to sign in now? ",
                "Cancel",
                "cancel_failed_signin",
                WorkspaceAction::CloseLoginPrompt,
                theme,
                ui,
                id,
                states,
            ))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(26.0)
    .with_padding_right(26.0)
    .with_padding_top(24.0)
    .with_padding_bottom(24.0)
    .finish();

    modal_scrim_held(panel)
}

/// While the browser sign-in waits: the code to match before approving there.
/// Only Cancel (or the sign-in finishing) closes it: a click elsewhere in the
/// window must not drop a sign-in the person is completing in the browser.
fn render_waiting_for_browser(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
    code: &str,
) -> Box<dyn Element> {
    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(18.0)
            .with_child(brand_icon(56.0))
            .with_child(
                Text::new(
                    "Finish in your browser".to_string(),
                    ui,
                    crate::theme::text_size(22.0),
                )
                .with_color(theme.main_text())
                .finish(),
            )
            .with_child(
                Text::new(
                    "We opened a secure sign-in page. Check the code there matches this one, \
                     then approve. This closes once you're signed in."
                        .to_string(),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(crate::setup::browser_auth_view::render_code(
                theme, ui, code,
            ))
            .with_child(link_line(
                "Didn't mean to sign in? ",
                "Cancel",
                "cancel_signin",
                WorkspaceAction::CloseLoginPrompt,
                theme,
                ui,
                id,
                states,
            ))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(26.0)
    .with_padding_right(26.0)
    .with_padding_top(24.0)
    .with_padding_bottom(24.0)
    .finish();

    modal_scrim_held(panel)
}

fn footer(theme: &Theme, ui: FamilyId, id: EntityId, states: &States) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(link_line(
            "By continuing, you agree to our ",
            "Terms of Service",
            "terms",
            WorkspaceAction::OpenExternalUrl {
                url: TERMS_URL.to_string(),
            },
            theme,
            ui,
            id,
            states,
        ))
        .with_child(link_line(
            "You can adjust your ",
            "Privacy Settings",
            "privacy",
            WorkspaceAction::OpenExternalUrl {
                url: PRIVACY_URL.to_string(),
            },
            theme,
            ui,
            id,
            states,
        ))
        .finish()
}

/// A big full-width filled "Sign up" button.
/// A full-width button that starts the browser sign-in.
fn primary_button(
    label: &'static str,
    key: &str,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(key.to_string())
        .or_default()
        .clone();
    let base = theme.accent;
    let hovered = tint(theme.accent, 220);
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hovered } else { base };
            Container::new(
                Align::new(
                    Text::new(label.to_string(), ui, crate::theme::text_size(14.0))
                        .with_color(WHITE)
                        .finish(),
                )
                .finish(),
            )
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, WorkspaceAction::SignInFromHeader)
        }),
    )
}

/// A "{prefix}{link}" row where the link portion is a clickable accent word.
#[allow(clippy::too_many_arguments)]
fn link_line(
    prefix: &str,
    link: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if !prefix.is_empty() {
        row = row.with_child(
            Text::new(prefix.to_string(), ui, crate::theme::text_size(13.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    }
    row.with_child(link_text(link, key, action, theme, ui, id, states))
        .finish()
}

fn link_text(
    label: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("authlink_{key}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let base = theme.accent;
    let hovered = tint(theme.accent, 200);
    Box::new(
        Hoverable::new(hover, move |st| {
            let color = if st.is_hovered() { hovered } else { base };
            Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                .with_color(color)
                .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

/// Wrap a panel in a dismissable centered scrim. Clicking the panel is swallowed;
/// clicking the scrim dispatches `dismiss`.
pub fn modal_scrim(
    panel: Box<dyn Element>,
    dismiss: WorkspaceAction,
    id: EntityId,
) -> Box<dyn Element> {
    let panel = ConstrainedBox::new(panel).with_max_width(PANEL_W).finish();
    modal_scrim_wide(panel, dismiss, id)
}

/// [`modal_scrim`] that only the panel's own controls can close: clicks on the
/// scrim are swallowed.
fn modal_scrim_held(panel: Box<dyn Element>) -> Box<dyn Element> {
    let panel = ConstrainedBox::new(panel).with_max_width(PANEL_W).finish();
    scrim(panel, None)
}

/// [`modal_scrim`] for a panel that sets its own width.
pub fn modal_scrim_wide(
    panel: Box<dyn Element>,
    dismiss: WorkspaceAction,
    id: EntityId,
) -> Box<dyn Element> {
    scrim(panel, Some((dismiss, id)))
}

fn scrim(
    panel: Box<dyn Element>,
    dismiss: Option<(WorkspaceAction, EntityId)>,
) -> Box<dyn Element> {
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_e, _a, _p| DispatchEventResult::StopPropagation)
        .finish();
    EventHandler::new(
        Container::new(Align::new(panel).finish())
            .with_background_color(ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        if let Some((dismiss, id)) = &dismiss {
            ectx.dispatch_typed_action_for(*id, dismiss.clone());
        }
        DispatchEventResult::StopPropagation
    })
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpui::geometry::vector::Vector2F;
    use warpui::platform::WindowStyle;
    use warpui::{App, AppContext, Entity, TypedActionView, View, ViewContext};
    use warpui_core::presenter::DispatchedActionKind;
    use warpui_core::{Event, WindowInvalidation};

    struct Prompt {
        code: Option<&'static str>,
        problem: Option<&'static str>,
        states: States,
    }

    impl Entity for Prompt {
        type Event = ();
    }

    impl TypedActionView for Prompt {
        type Action = WorkspaceAction;
        fn handle_action(&mut self, _: &WorkspaceAction, _: &mut ViewContext<Self>) {}
    }

    impl View for Prompt {
        fn ui_name() -> &'static str {
            "LoginPromptUnderTest"
        }
        fn render(&self, app: &AppContext) -> Box<dyn Element> {
            let id = app.root_view_id(app.window_ids().next().unwrap()).unwrap();
            render_login_prompt(
                &Theme::dark(),
                FamilyId(0),
                id,
                &self.states,
                self.code,
                self.problem,
            )
        }
    }

    /// Press and release the left button at each point; how many clicks asked
    /// to close the prompt.
    fn closes(code: Option<&'static str>, points: Vec<Vector2F>) -> usize {
        asked(code, None, points, "CloseLoginPrompt")
    }

    /// As [`closes`], for any action and a sign-in that failed.
    fn asked(
        code: Option<&'static str>,
        problem: Option<&'static str>,
        points: Vec<Vector2F>,
        wanted: &'static str,
    ) -> usize {
        let (tx, rx) = std::sync::mpsc::channel();
        App::test(dragent_assets::DragentAssets, move |mut app| async move {
            let (window, view) = app.add_window(WindowStyle::NotStealFocus, |_| Prompt {
                code,
                problem,
                states: Default::default(),
            });
            let presenter = app.presenter(window).unwrap();
            let mut closes = 0;
            app.update(|ctx| {
                {
                    let mut presenter = presenter.borrow_mut();
                    presenter.invalidate(
                        WindowInvalidation {
                            updated: [view.id()].into_iter().collect(),
                            ..Default::default()
                        },
                        ctx,
                    );
                    presenter.build_scene(Vector2F::new(1000.0, 700.0), 1.0, None, ctx);
                }
                for position in points {
                    for event in [
                        Event::MouseMoved {
                            position,
                            cmd: false,
                            shift: false,
                            is_synthetic: false,
                        },
                        Event::LeftMouseDown {
                            position,
                            modifiers: Default::default(),
                            click_count: 1,
                            is_first_mouse: false,
                        },
                        Event::LeftMouseUp {
                            position,
                            modifiers: Default::default(),
                        },
                    ] {
                        let result = presenter.borrow_mut().dispatch_event(event, ctx);
                        for dispatched in result.actions {
                            if let DispatchedActionKind::Typed(action) = dispatched.kind {
                                if format!("{action:?}").contains(wanted) {
                                    closes += 1;
                                }
                            }
                        }
                    }
                }
            });
            tx.send(closes).unwrap();
        });
        rx.recv().unwrap()
    }

    fn outside() -> Vec<Vector2F> {
        [(5.0, 5.0), (60.0, 350.0), (940.0, 350.0), (500.0, 690.0)]
            .map(|(x, y)| Vector2F::new(x, y))
            .to_vec()
    }

    #[test]
    fn clicks_outside_never_drop_a_waiting_sign_in() {
        assert_eq!(closes(Some("ABCD-1234"), outside()), 0);
    }

    #[test]
    fn cancel_still_closes_a_waiting_sign_in() {
        // Every click in the panel's 430 px column, in the window's height.
        // Text has no width in tests, so a link is a sliver: step finely.
        let panel = panel_column();
        let hits = closes(Some("ABCD-1234"), panel);
        // Only the Cancel link: the scrim above and below would be hundreds.
        assert!((1..20).contains(&hits), "{hits} clicks closed it");
    }

    #[test]
    fn a_failed_sign_in_stays_until_tried_again_or_cancelled() {
        let problem = Some("This sign-in expired before it was approved.");
        assert_eq!(asked(None, problem, outside(), "CloseLoginPrompt"), 0);
        assert!((1..20).contains(&asked(None, problem, panel_column(), "CloseLoginPrompt")));
        assert!(asked(None, problem, panel_column(), "SignInFromHeader") > 0);
    }

    fn panel_column() -> Vec<Vector2F> {
        (60..640)
            .step_by(4)
            .flat_map(|y| (288..712).step_by(2).map(move |x| (x as f32, y as f32)))
            .map(|(x, y)| Vector2F::new(x, y))
            .collect()
    }

    #[test]
    fn the_welcome_prompt_still_closes_on_an_outside_click() {
        assert_eq!(closes(None, outside()), 4);
    }
}
