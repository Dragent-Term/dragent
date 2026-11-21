//! Dragent's open-source repository and X account: the Star button and the
//! social links shown in the sidebar, Settings → About and the new
//! workspace screen.
//!
//! Clicking Star stars the repository in place through the person's own
//! `gh` sign-in ("Starring…" → "Starred on GitHub"); nothing is starred
//! without that click. Only without a usable `gh` (missing, signed out)
//! does it open the repository on GitHub instead.

use std::sync::Mutex;

use warpui::elements::{
    Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize, ParentElement,
    Text,
};
use warpui::fonts::FamilyId;
use warpui::EventContext;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, Theme};

pub const GITHUB_REPO: &str = "Dragent-Term/dragent";
pub const GITHUB_URL: &str = "https://github.com/Dragent-Term/dragent";
pub const X_HANDLE: &str = "@dragentai";
pub const X_URL: &str = "https://x.com/dragentai";

/// What is known about the repository and the person's star.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StarState {
    /// The repository's star count, once read.
    pub stars: Option<u64>,
    /// Whether the person's `gh` account has starred it; `None` when `gh`
    /// is missing or signed out (Star then opens the repository).
    pub starred: Option<bool>,
    /// A star request is under way.
    pub starring: bool,
}

static STATE: Mutex<StarState> = Mutex::new(StarState {
    stars: None,
    starred: None,
    starring: false,
});

/// The latest state, for rendering.
pub fn state() -> StarState {
    STATE.lock().map(|state| *state).unwrap_or_default()
}

fn update(change: impl FnOnce(&mut StarState)) -> StarState {
    match STATE.lock() {
        Ok(mut state) => {
            change(&mut state);
            *state
        }
        Err(_) => StarState::default(),
    }
}

/// Mark a star request as started; false when one already runs.
pub fn begin_starring() -> bool {
    let mut began = false;
    update(|state| {
        if !state.starring {
            state.starring = true;
            began = true;
        }
    });
    began
}

#[cfg(not(target_family = "wasm"))]
fn gh(args: &[&str]) -> Option<std::process::Output> {
    use crate::process_ext::{probe_output, NoWindow};
    let mut command = crate::process_ext::gh_command();
    command.args(args).no_window();
    probe_output(&mut command, std::time::Duration::from_secs(10)).ok()
}

/// Read the star count and whether the person starred it. Blocking: run it
/// off the UI thread.
#[cfg(not(target_family = "wasm"))]
pub fn refresh() -> StarState {
    let starred =
        gh(&["api", &format!("user/starred/{GITHUB_REPO}"), "--silent"]).and_then(|output| {
            // 204 = starred, 404 = not; anything else (no gh, signed out) is unknown.
            if output.status.success() {
                Some(true)
            } else if String::from_utf8_lossy(&output.stderr).contains("404") {
                Some(false)
            } else {
                None
            }
        });
    let stars = stars_from_gh().or_else(stars_from_api);
    update(|state| {
        state.starred = starred;
        if stars.is_some() {
            state.stars = stars;
        }
    })
}

#[cfg(not(target_family = "wasm"))]
fn stars_from_gh() -> Option<u64> {
    let output = gh(&[
        "api",
        &format!("repos/{GITHUB_REPO}"),
        "--jq",
        ".stargazers_count",
    ])?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

/// GitHub's public API, for people without `gh`.
#[cfg(not(target_family = "wasm"))]
fn stars_from_api() -> Option<u64> {
    let repo: serde_json::Value = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Dragent")
        .build()
        .ok()?
        .get(format!("https://api.github.com/repos/{GITHUB_REPO}"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    repo.get("stargazers_count")?.as_u64()
}

/// Star the repository with the person's `gh` account. Blocking. Err means
/// it couldn't (the caller opens the repository instead).
#[cfg(not(target_family = "wasm"))]
pub fn star() -> Result<StarState, String> {
    let output = gh(&[
        "api",
        "--method",
        "PUT",
        &format!("user/starred/{GITHUB_REPO}"),
        "--silent",
    ]);
    let ok = output
        .as_ref()
        .is_some_and(|output| output.status.success());
    // The count as GitHub now has it, the new star included.
    let stars = if ok { stars_from_gh() } else { None };
    let state = update(|state| {
        state.starring = false;
        if ok {
            state.starred = Some(true);
            if stars.is_some() {
                state.stars = stars;
            }
        }
    });
    if ok {
        Ok(state)
    } else {
        Err(output
            .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_string())
            .unwrap_or_else(|| "gh is not available".into()))
    }
}

#[cfg(target_family = "wasm")]
pub fn refresh() -> StarState {
    state()
}

#[cfg(target_family = "wasm")]
pub fn star() -> Result<StarState, String> {
    update(|state| state.starring = false);
    Err("not available here".into())
}

fn format_stars(stars: u64) -> String {
    if stars >= 1000 {
        let tenths = (stars as f64 / 100.0).round() / 10.0;
        let text = format!("{tenths:.1}");
        format!("{}k", text.trim_end_matches(".0"))
    } else {
        stars.to_string()
    }
}

/// "Star on GitHub": GitHub mark and label, with the star count at the
/// right. Starred, it reads "Starred on GitHub" with a filled gold star.
/// `fill` stretches it to the width it's given (the sidebar).
pub fn render_star_button(
    theme: &Theme,
    ui: FamilyId,
    state: StarState,
    fill: bool,
    mouse: MouseStateHandle,
    mut on_click: impl FnMut(&mut EventContext) + 'static,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse, move |hover| {
        let hovered = hover.is_hovered();
        let starred = state.starred == Some(true);
        let text_color = if hovered {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let gold = theme.ui_yellow();
        let label = if state.starring {
            "Starring…"
        } else if starred {
            "Starred on GitHub"
        } else {
            "Star on GitHub"
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(render_icon(
                AppIcon::Asset("icons/github.svg"),
                text_color,
                13.0,
            ));
        let text = Text::new(label.to_string(), ui, text_size(12.0))
            .with_color(text_color)
            .finish();
        row = row.with_child(text);
        // The star, and the count once there is one, at the end.
        let mut tail = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(render_icon(
                AppIcon::Asset(if starred {
                    "icons/star-filled.svg"
                } else {
                    "icons/star.svg"
                }),
                if starred || hovered { gold } else { text_color },
                12.0,
            ));
        if let Some(stars) = state.stars.filter(|stars| *stars > 0) {
            tail = tail.with_child(
                Text::new(format_stars(stars), ui, text_size(12.0))
                    .with_color(text_color)
                    .finish(),
            );
        }
        row = row.with_child(
            Container::new(tail.finish())
                .with_background_color(theme.fg_overlay_1())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .finish(),
        );
        if fill {
            // Full width (the sidebar): the content sits in the middle.
            row = row
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::Center);
        }
        Container::new(row.finish())
            .with_background_color(if hovered {
                theme.sidebar_row_hover()
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .with_border(Border::all(1.0).with_border_color(if hovered {
                theme.sub_text()
            } else {
                theme.border()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(if fill { 8.0 } else { 10.0 })
            .with_padding_right(if fill { 8.0 } else { 5.0 })
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| on_click(ectx))
    .finish()
}

/// The X mark, as an icon link or with a label beside it.
pub fn render_x_button(
    theme: &Theme,
    ui: FamilyId,
    label: Option<&'static str>,
    mouse: MouseStateHandle,
    mut on_click: impl FnMut(&mut EventContext) + 'static,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse, move |hover| {
        let color = if hover.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(AppIcon::Asset("icons/x-logo.svg"), color, 12.0));
        if let Some(label) = label {
            row = row.with_child(
                Text::new(label.to_string(), ui, text_size(12.0))
                    .with_color(color)
                    .finish(),
            );
        }
        Container::new(row.finish())
            .with_uniform_padding(5.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_background_color(if hover.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| on_click(ectx))
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_counts_read_like_githubs() {
        assert_eq!(format_stars(0), "0");
        assert_eq!(format_stars(999), "999");
        assert_eq!(format_stars(1000), "1k");
        assert_eq!(format_stars(1240), "1.2k");
        assert_eq!(format_stars(15_960), "16k");
    }
}
