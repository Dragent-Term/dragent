//! Settings → Mobile: pair a phone by QR and manage paired phones.
//!
//! The phone never signs in. The signed-in desktop shows a single-use pairing
//! code as a QR beside the steps — a v2 link (§6.2) carrying this desktop's
//! id, its identity key and the pairing secret, with the full link as text
//! for pasting — and lists the paired phones with Remove, plus the security
//! keys section (reset) and the §9 failure banners. Signed out, the page asks
//! for sign-in first.

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element, Empty,
    EventHandler, Expanded, Flex, Image, MainAxisAlignment, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, EntityId};
use warpui::{GetSingletonModelHandle, ReadModel, SingletonEntity};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::api::{format_pairing_countdown, pairing_remaining_secs, CompanionDevice};
use crate::auth::AuthManager;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, Theme};

use super::super::action::SettingsAction;
use super::super::mobile::{
    device_presence, now_epoch_secs, pairing_progress_fraction, pairing_status, DevicePresence,
    MobileAction, MobilePairing, PairingStatus, PAIRING_TTL_SECS,
};
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    alert_banner, card_rows, danger_button, group_label, page_header, primary_button,
};

/// Side of the QR panel. The bitmap carries its own white quiet zone, so the
/// panel only adds a sliver of padding around it.
const QR_PANEL_SIZE: f32 = 224.0;
const QR_PANEL_PADDING: f32 = 6.0;
const QR_ENLARGED_SIZE: f32 = 400.0;
const WHITE: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};
const CLEAR: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

fn act(action: MobileAction) -> SettingsAction {
    SettingsAction::Mobile(action)
}

/// Below this pane width the steps and the QR stop fitting side by side.
fn pair_card_stacks(pane_width: f32) -> bool {
    pane_width > 0.0 && pane_width < 640.0
}

/// `android` → `Android`, `ios` → `iOS`, anything else titled.
pub(crate) fn platform_label(platform: &str) -> String {
    match platform.trim().to_lowercase().as_str() {
        "ios" => "iOS".to_string(),
        "android" => "Android".to_string(),
        "web" => "Web".to_string(),
        other if other.is_empty() => "Phone".to_string(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

/// `2026-11-03T12:00:00Z` → `Nov 3, 2026`; anything unparsable passes through.
pub(crate) fn paired_date(timestamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|at| at.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| timestamp.to_string())
}

/// Groups of four for reading. Copy still gives the phone the raw code.
pub(crate) fn grouped_code(code: &str) -> String {
    code.chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_logged_in(app: &AppContext) -> bool {
    let auth = AuthManager::handle(app);
    app.read_model(&auth, |manager, _| manager.is_logged_in())
}

fn label(theme: &Theme, ui: FamilyId, text: &str, size: f32) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, text_size(size))
        .with_color(theme.main_text())
        .finish()
}

fn muted(theme: &Theme, ui: FamilyId, text: &str, size: f32) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, text_size(size))
        .with_color(theme.sub_text())
        .finish()
}

/// A borderless small button: icon optional, red on hover when it destroys.
#[allow(clippy::too_many_arguments)]
fn quiet_button(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    hover: MouseStateHandle,
    icon: Option<AppIcon>,
    text: &str,
    danger: bool,
    action: MobileAction,
) -> Box<dyn Element> {
    let text = text.to_string();
    let action = act(action);
    Box::new(
        Hoverable::new(hover, move |state| {
            let hovered = state.is_hovered();
            let (color, background) = match (hovered, danger) {
                (false, _) => (theme.sub_text(), CLEAR),
                (true, false) => (theme.main_text(), theme.surface_2()),
                (true, true) => {
                    let red = theme.ui_red();
                    (red, ColorU::new(red.r, red.g, red.b, 22))
                }
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0);
            if let Some(icon) = icon {
                row = row.with_child(render_icon(icon, color, 13.0));
            }
            let row = row.with_child(
                Text::new(text.clone(), ui, text_size(12.5))
                    .with_color(color)
                    .finish(),
            );
            Container::new(row.finish())
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, action.clone());
        }),
    )
}

fn icon_tile(theme: &Theme, icon: AppIcon, side: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Align::new(render_icon(icon, theme.sub_text(), side * 0.5)).finish())
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(side * 0.25)))
            .finish(),
    )
    .with_width(side)
    .with_height(side)
    .finish()
}

pub fn render_mobile(view: &SettingsView, theme: &Theme, app: &AppContext) -> Box<dyn Element> {
    let ui = view.ui_font;

    let mut page = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(page_header(
            theme,
            ui,
            "Mobile",
            "Reach your terminals and agents from your phone.",
        ));

    // Phone pairing needs the relay; a Dev build's pairing links wouldn't
    // open the phone app either.
    if crate::api::is_local_mode() {
        return page
            .with_child(super::super::widgets::needs_account_state(theme, ui))
            .finish();
    }

    if !is_logged_in(app) {
        return page.with_child(render_signed_out(view, theme, ui)).finish();
    }

    if let Some(notice) = view.mobile.notice.as_deref() {
        page = page.with_child(
            Container::new(alert_banner(theme, ui, notice))
                .with_padding_bottom(16.0)
                .finish(),
        );
    }

    // §9 security banners: paired devices were reset (records unverifiable,
    // keychain item lost, or keys just reset) — with a Dismiss — and phone
    // access off on this computer (no secure store). Both are set on page
    // open; the reset banner survives refreshes until dismissed.
    if let Some(security) = view.mobile.e2ee_notice.as_deref() {
        page = page.with_child(render_dismissible_banner(view, theme, ui, security));
    }

    if view.mobile.e2ee_off.as_deref().is_some() {
        return page
            .with_child(render_access_off(view, theme, ui))
            .with_child(render_devices_header(view, theme, ui))
            .with_child(render_devices(view, theme, ui))
            .with_child(render_security(view, theme, ui))
            .finish();
    }

    page.with_child(render_pair_card(view, theme, ui))
        .with_child(render_devices_header(view, theme, ui))
        .with_child(render_devices(view, theme, ui))
        .with_child(render_security(view, theme, ui))
        .finish()
}

/// A §9 banner with a Dismiss button on its right.
fn render_dismissible_banner(
    view: &SettingsView,
    theme: &Theme,
    ui: FamilyId,
    text: &str,
) -> Box<dyn Element> {
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(Expanded::new(1.0, alert_banner(theme, ui, text)).finish())
        .with_child(quiet_button(
            theme,
            ui,
            view.view_id,
            view.hover("mobile_dismiss_notice"),
            None,
            "Dismiss",
            false,
            MobileAction::DismissNotice,
        ))
        .finish();
    Container::new(row).with_padding_bottom(16.0).finish()
}

/// §9 "No secure key store" / identity unavailable: phone access is off on
/// this computer. The devices list and security section below still work;
/// pairing a new phone does not.
fn render_access_off(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let text = view
        .mobile
        .e2ee_off
        .as_deref()
        .unwrap_or("Phone access is off on this computer.");
    let body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(6.0)
        .with_child(label(theme, ui, "Can't pair a phone here", 14.0))
        .with_child(muted(theme, ui, text, 12.5))
        .finish();
    card_rows(
        theme,
        vec![Container::new(body).with_uniform_padding(24.0).finish()],
    )
}

fn render_signed_out(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(icon_tile(theme, AppIcon::Smartphone, 44.0))
        .with_child(
            Container::new(label(theme, ui, "Sign in to pair a phone", 14.0))
                .with_padding_top(14.0)
                .finish(),
        )
        .with_child(
            Container::new(muted(
                theme,
                ui,
                "Your phone connects through your Dragent account, so sign in on this computer first.",
                12.5,
            ))
            .with_padding_top(4.0)
            .with_padding_bottom(16.0)
            .finish(),
        )
        .with_child(primary_button(
            theme,
            ui,
            view.view_id,
            view.hover("mobile_sign_in"),
            "Sign in",
            SettingsAction::SignIn,
        ))
        .finish();
    card_rows(
        theme,
        vec![Container::new(body).with_uniform_padding(32.0).finish()],
    )
}

/// The one pairing card: steps beside the QR, and the paste fallback along the
/// bottom while a code is live.
fn render_pair_card(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let stacked = view.mobile.preview_stacked || pair_card_stacks(view.content_pane_width);
    let steps = render_steps(theme, ui);
    let qr = ConstrainedBox::new(render_qr_column(view, theme, ui))
        .with_width(QR_PANEL_SIZE)
        .finish();
    let hero = if stacked {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(24.0)
            .with_child(qr)
            .with_child(steps)
            .finish()
    } else {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(32.0)
            .with_child(Expanded::new(1.0, steps).finish())
            .with_child(qr)
            .finish()
    };

    let mut rows = vec![Container::new(hero).with_uniform_padding(24.0).finish()];
    if let PairingStatus::Live(_) = pairing_status(&view.mobile.pairing, now_epoch_secs()) {
        if let Some(pairing) = view.mobile.pairing.as_ref() {
            // A v2 QR carries the full link (§6.2: no bare-code entry), so
            // the fallback row shows the link; v1 keeps the code as text.
            rows.push(if pairing.is_v2() {
                render_link_row(view, theme, ui, pairing, stacked)
            } else {
                render_code_row(view, theme, ui, pairing, stacked)
            });
        }
    }
    card_rows(theme, rows)
}

fn render_steps(theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(label(theme, ui, "Pair your phone", 15.0))
        .with_child(render_step(
            theme,
            ui,
            "1",
            "Get the Dragent app",
            "Install it on your phone.",
        ))
        .with_child(render_step(
            theme,
            ui,
            "2",
            "Tap Scan QR code",
            "It's on the app's home screen. Point your camera at the code here.",
        ))
        .with_child(render_step(
            theme,
            ui,
            "3",
            "You're connected",
            "No sign-in on the phone. Keep Dragent open on this computer to reach it.",
        ))
        .finish()
}

fn render_step(
    theme: &Theme,
    ui: FamilyId,
    number: &str,
    title: &str,
    body: &str,
) -> Box<dyn Element> {
    let badge = ConstrainedBox::new(
        Container::new(Align::new(muted(theme, ui, number, 12.0)).finish())
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
            .finish(),
    )
    .with_width(22.0)
    .with_height(22.0)
    .finish();
    let text = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(3.0)
        .with_child(label(theme, ui, title, 13.5))
        .with_child(muted(theme, ui, body, 12.5))
        .finish();
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(12.0)
        .with_child(badge)
        .with_child(Expanded::new(1.0, text).finish())
        .finish()
}

/// The right side of the card: the QR while a code is live, otherwise an empty
/// frame holding the next thing to do. An expired code is removed rather than
/// dimmed, so nobody tries to scan a dead one.
fn render_qr_column(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let state = &view.mobile;
    let id = view.view_id;
    let column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0);

    let frame_action = |title: Option<&str>, button: &str, key: &str| {
        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(render_icon(AppIcon::QrCode, theme.disabled_text(), 32.0));
        if let Some(title) = title {
            content = content.with_child(
                Container::new(muted(theme, ui, title, 12.5))
                    .with_padding_top(10.0)
                    .finish(),
            );
        }
        content
            .with_child(
                Container::new(primary_button(
                    theme,
                    ui,
                    id,
                    view.hover(key),
                    button,
                    act(MobileAction::CreatePairing),
                ))
                .with_padding_top(14.0)
                .finish(),
            )
            .finish()
    };
    let validity = format!("Single use, valid for {} minutes", PAIRING_TTL_SECS / 60);

    if state.creating {
        return column
            .with_child(qr_frame(theme, muted(theme, ui, "Creating a code…", 12.5)))
            .with_child(muted(theme, ui, &validity, 12.0))
            .finish();
    }

    match pairing_status(&state.pairing, now_epoch_secs()) {
        PairingStatus::Idle => column
            .with_child(qr_frame(
                theme,
                frame_action(None, "Show QR code", "mobile_generate"),
            ))
            .with_child(muted(theme, ui, &validity, 12.0))
            .finish(),
        PairingStatus::Expired => column
            .with_child(qr_frame(
                theme,
                frame_action(
                    Some("This code expired"),
                    "Show a new code",
                    "mobile_regenerate",
                ),
            ))
            .with_child(muted(theme, ui, &validity, 12.0))
            .finish(),
        PairingStatus::Live(remaining) => {
            let qr = state
                .pairing
                .as_ref()
                .and_then(|pairing| pairing.qr_asset_id.as_deref());
            let mut column = column;
            column = match qr {
                Some(asset_id) => column.with_child(render_qr_button(view, theme, asset_id)),
                None => column.with_child(qr_frame(
                    theme,
                    muted(
                        theme,
                        ui,
                        "Couldn't draw this QR. Paste the code instead.",
                        12.5,
                    ),
                )),
            };
            let meter = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(muted(
                    theme,
                    ui,
                    &format!("Expires in {}", format_pairing_countdown(remaining)),
                    12.0,
                ))
                .with_child(
                    ConstrainedBox::new(expiry_bar(theme, pairing_progress_fraction(remaining)))
                        .with_width(QR_PANEL_SIZE)
                        .finish(),
                )
                .finish();
            let mut actions = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(4.0);
            if qr.is_some() {
                actions = actions.with_child(quiet_button(
                    theme,
                    ui,
                    id,
                    view.hover("mobile_enlarge"),
                    Some(AppIcon::Maximize2),
                    "Enlarge",
                    false,
                    MobileAction::EnlargeQr,
                ));
            }
            let actions = actions.with_child(quiet_button(
                theme,
                ui,
                id,
                view.hover("mobile_new_code"),
                Some(AppIcon::RefreshCw),
                "New code",
                false,
                MobileAction::CreatePairing,
            ));
            column
                .with_child(meter)
                .with_child(actions.finish())
                .finish()
        }
    }
}

/// An empty frame the size of the QR, so the card keeps its shape.
fn qr_frame(theme: &Theme, content: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Align::new(content).finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_uniform_padding(12.0)
            .finish(),
    )
    .with_width(QR_PANEL_SIZE)
    .with_height(QR_PANEL_SIZE)
    .finish()
}

fn qr_panel(theme: &Theme, asset_id: &str, side: f32) -> Box<dyn Element> {
    let image = ConstrainedBox::new(
        Image::new(
            warpui_core::assets::asset_cache::AssetSource::Raw {
                id: asset_id.to_string(),
            },
            warpui_core::image_cache::CacheOption::BySize,
        )
        .contain()
        .finish(),
    )
    .with_width(side - 2.0 * QR_PANEL_PADDING)
    .with_height(side - 2.0 * QR_PANEL_PADDING)
    .finish();
    Container::new(image)
        .with_background_color(WHITE)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_uniform_padding(QR_PANEL_PADDING)
        .finish()
}

fn render_qr_button(view: &SettingsView, theme: &Theme, asset_id: &str) -> Box<dyn Element> {
    let id = view.view_id;
    Box::new(
        Hoverable::new(view.hover("mobile_qr"), move |_| {
            qr_panel(theme, asset_id, QR_PANEL_SIZE)
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, act(MobileAction::EnlargeQr));
        }),
    )
}

/// Thin meter under the countdown: full when the code is fresh, empty when it
/// lapses.
fn expiry_bar(theme: &Theme, fraction: f32) -> Box<dyn Element> {
    let fill = fraction.clamp(0.0, 1.0).max(0.002);
    let track = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Expanded::new(
                fill,
                Container::new(Empty::new().finish())
                    .with_background_color(theme.accent)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                    .finish(),
            )
            .finish(),
        )
        .with_child(Expanded::new((1.0 - fill).max(0.0), Empty::new().finish()).finish())
        .finish();
    ConstrainedBox::new(
        Container::new(track)
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
            .finish(),
    )
    .with_height(3.0)
    .finish()
}

/// Bottom strip of the card: the code as text for when the camera can't read
/// the QR.
fn render_code_row(
    view: &SettingsView,
    theme: &Theme,
    ui: FamilyId,
    pairing: &MobilePairing,
    stacked: bool,
) -> Box<dyn Element> {
    let hint = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0)
        .with_child(label(theme, ui, "Can't scan it?", 13.0))
        .with_child(muted(
            theme,
            ui,
            "Paste this code into the app instead.",
            12.0,
        ))
        .finish();
    let chip = Container::new(label(theme, ui, &grouped_code(&pairing.code), 12.5))
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish();
    let copied = view.mobile.copied;
    let code = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(6.0)
        .with_child(chip)
        .with_child(quiet_button(
            theme,
            ui,
            view.view_id,
            view.hover("mobile_copy"),
            Some(if copied {
                AppIcon::Check
            } else {
                AppIcon::Copy
            }),
            if copied { "Copied" } else { "Copy" },
            false,
            MobileAction::CopyCode,
        ))
        .finish();
    let row = if stacked {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(hint)
            .with_child(code)
            .finish()
    } else {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(hint)
            .with_child(code)
            .finish()
    };
    Container::new(row)
        .with_padding_left(24.0)
        .with_padding_right(18.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

/// Bottom strip of the card for a v2 QR: the full pairing link, for when the
/// camera can't read the QR (§6.2: without a camera, the person pastes the
/// full link — there is no bare-code entry).
fn render_link_row(
    view: &SettingsView,
    theme: &Theme,
    ui: FamilyId,
    pairing: &MobilePairing,
    _stacked: bool,
) -> Box<dyn Element> {
    let hint = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0)
        .with_child(label(theme, ui, "Can't scan it?", 13.0))
        .with_child(muted(
            theme,
            ui,
            "Paste this link into the app instead.",
            12.0,
        ))
        .finish();
    let chip = Container::new(muted(theme, ui, &pairing.deep_link, 11.5))
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish();
    let copied = view.mobile.copied;
    let code = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(6.0)
        .with_child(Expanded::new(1.0, chip).finish())
        .with_child(quiet_button(
            theme,
            ui,
            view.view_id,
            view.hover("mobile_copy_link"),
            Some(if copied {
                AppIcon::Check
            } else {
                AppIcon::Copy
            }),
            if copied { "Copied" } else { "Copy link" },
            false,
            MobileAction::CopyLink,
        ))
        .finish();
    // The link is long, so it always gets its own line under the hint.
    let row = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(hint)
        .with_child(code)
        .finish();
    Container::new(row)
        .with_padding_left(24.0)
        .with_padding_right(18.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

fn render_devices_header(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let refreshing = view.mobile.devices_loading;
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(group_label(theme, ui, "Paired phones"))
        .with_child(
            Container::new(quiet_button(
                theme,
                ui,
                view.view_id,
                view.hover("mobile_refresh_devices"),
                Some(AppIcon::RefreshCw),
                if refreshing {
                    "Refreshing…"
                } else {
                    "Refresh"
                },
                false,
                MobileAction::RefreshDevices,
            ))
            .with_padding_bottom(3.0)
            .finish(),
        )
        .finish()
}

fn render_devices(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let state = &view.mobile;
    let padded = |element: Box<dyn Element>| {
        Container::new(element)
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(14.0)
            .with_padding_bottom(14.0)
            .finish()
    };

    if state.devices.is_empty() {
        let row = if let Some(error) = state.devices_error.as_deref() {
            Text::new(error.to_string(), ui, text_size(12.5))
                .with_color(theme.ui_red())
                .finish()
        } else if state.devices_loading {
            muted(theme, ui, "Loading…", 12.5)
        } else {
            muted(
                theme,
                ui,
                "No phones yet. Pair one above and it shows up here.",
                12.5,
            )
        };
        return card_rows(theme, vec![padded(row)]);
    }

    let now = now_epoch_secs();
    let rows = state
        .devices
        .iter()
        .map(|device| padded(render_device_row(view, theme, ui, device, now)))
        .collect();
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(8.0)
        .with_child(card_rows(theme, rows));
    column = match state.devices_error.as_deref() {
        Some(error) => column.with_child(
            Text::new(error.to_string(), ui, text_size(12.0))
                .with_color(theme.ui_red())
                .finish(),
        ),
        None => column.with_child(
            Text::new(
                "A removed phone loses access right away.".to_string(),
                ui,
                text_size(12.0),
            )
            .with_color(theme.disabled_text())
            .finish(),
        ),
    };
    column.finish()
}

/// The security keys section (§6.6): this computer's key id and the
/// compromise-recovery reset. Resetting deletes every phone pin, makes a new
/// key, and every phone must pair again.
fn render_security(view: &SettingsView, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let state = &view.mobile;
    let id = view.view_id;
    let key_line = match state.e2ee_key_id.as_deref() {
        Some(key_id) => format!("This computer's key: {key_id}"),
        None => "This computer's key is unavailable.".to_string(),
    };
    let details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0)
        .with_child(label(theme, ui, "Security keys", 13.5))
        .with_child(muted(theme, ui, &key_line, 12.0))
        .with_child(muted(
            theme,
            ui,
            "Pairing is per phone. If this computer is ever compromised, reset its keys and pair each phone again.",
            12.0,
        ))
        .finish();

    let controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min);
    let controls = if state.resetting {
        controls.with_child(muted(theme, ui, "Resetting…", 12.5))
    } else if state.confirm_reset {
        controls
            .with_spacing(8.0)
            .with_child(quiet_button(
                theme,
                ui,
                id,
                view.hover("mobile_reset_cancel"),
                None,
                "Cancel",
                false,
                MobileAction::CancelReset,
            ))
            .with_child(danger_button(
                theme,
                ui,
                id,
                view.hover("mobile_reset_confirm"),
                "Reset keys now",
                act(MobileAction::ResetKeys),
            ))
    } else {
        controls.with_child(quiet_button(
            theme,
            ui,
            id,
            view.hover("mobile_reset"),
            None,
            "Reset security keys",
            true,
            MobileAction::RequestReset,
        ))
    };

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(icon_tile(theme, AppIcon::Key, 36.0))
        .with_child(Expanded::new(1.0, details).finish())
        .with_child(controls.finish())
        .finish();
    let padded = Container::new(row)
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish();
    let warning = "Pair each phone again now. Until you do, someone holding the old key could pretend to be this computer to them.";
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(8.0)
        .with_child(group_label(theme, ui, "Security keys"))
        .with_child(card_rows(theme, vec![padded]));
    if state.confirm_reset || state.resetting {
        column = column.with_child(
            Text::new(warning.to_string(), ui, text_size(12.0))
                .with_color(theme.ui_red())
                .finish(),
        );
    }
    column.finish()
}

fn render_device_row(
    view: &SettingsView,
    theme: &Theme,
    ui: FamilyId,
    device: &CompanionDevice,
    now: i64,
) -> Box<dyn Element> {
    let id = view.view_id;
    let state = &view.mobile;
    let device_id = device.device_id.as_str();
    let platform = platform_label(&device.platform);
    let description = if device.created_at.is_empty() {
        platform
    } else {
        format!("{platform} · Paired {}", paired_date(&device.created_at))
    };
    let details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0)
        .with_child(label(theme, ui, &device.name, 13.5))
        .with_child(muted(theme, ui, &description, 12.0))
        .finish();

    let controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min);
    let controls = if state.revoking.as_deref() == Some(device_id) {
        controls.with_child(muted(theme, ui, "Removing…", 12.5))
    } else if state.confirm_remove.as_deref() == Some(device_id) {
        controls
            .with_spacing(8.0)
            .with_child(quiet_button(
                theme,
                ui,
                id,
                view.hover(&format!("mobile_cancel_{device_id}")),
                None,
                "Cancel",
                false,
                MobileAction::CancelRemove,
            ))
            .with_child(danger_button(
                theme,
                ui,
                id,
                view.hover(&format!("mobile_confirm_{device_id}")),
                "Remove phone",
                act(MobileAction::RemoveDevice(device.device_id.clone())),
            ))
    } else {
        controls
            .with_spacing(14.0)
            .with_child(presence_row(
                theme,
                ui,
                device_presence(device.last_seen_at.as_deref(), now),
            ))
            .with_child(quiet_button(
                theme,
                ui,
                id,
                view.hover(&format!("mobile_remove_{device_id}")),
                None,
                "Remove",
                true,
                MobileAction::RequestRemove(device.device_id.clone()),
            ))
    };

    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(icon_tile(
            theme,
            match device.platform.trim().to_lowercase().as_str() {
                "web" => AppIcon::Globe,
                _ => AppIcon::Smartphone,
            },
            36.0,
        ))
        .with_child(Expanded::new(1.0, details).finish())
        .with_child(controls.finish())
        .finish()
}

fn presence_row(theme: &Theme, ui: FamilyId, presence: DevicePresence) -> Box<dyn Element> {
    let (text, color) = match presence {
        DevicePresence::Connected => ("Connected", theme.ui_green()),
        DevicePresence::Reconnecting => ("Reconnecting", theme.ui_yellow()),
        DevicePresence::Offline => ("Offline", theme.disabled_text()),
    };
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(7.0)
            .with_height(7.0)
            .finish(),
    )
    .with_background_color(color)
    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
    .finish();
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(6.0)
        .with_child(dot)
        .with_child(muted(theme, ui, text, 12.0))
        .finish()
}

/// Full-surface overlay with the enlarged QR. `None` unless the user asked for
/// it while a renderable code is on screen; the settings view stacks it over
/// the page. Clicking outside, the Close button, or Esc closes it.
pub fn render_mobile_overlay(view: &SettingsView, theme: &Theme) -> Option<Box<dyn Element>> {
    if !view.mobile.qr_enlarged {
        return None;
    }
    let ui = view.ui_font;
    let id = view.view_id;
    let pairing = view.mobile.pairing.as_ref()?;
    let asset_id = pairing.qr_asset_id.as_deref()?;
    let remaining = pairing_remaining_secs(pairing.expires_at_epoch, now_epoch_secs());

    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(12.0)
            .with_child(label(theme, ui, "Scan with the Dragent app", 14.0))
            .with_child(qr_panel(theme, asset_id, QR_ENLARGED_SIZE))
            .with_child(muted(
                theme,
                ui,
                &format!("Expires in {}", format_pairing_countdown(remaining)),
                12.5,
            ))
            .with_child(quiet_button(
                theme,
                ui,
                id,
                view.hover("mobile_qr_close"),
                None,
                "Close",
                false,
                MobileAction::CloseQrOverlay,
            ))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(16.0)))
    .with_uniform_padding(24.0)
    .finish();
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();

    let scrim = Container::new(Align::new(panel).finish())
        .with_background_color(ColorU::new(0, 0, 0, 160))
        .finish();
    Some(
        EventHandler::new(scrim)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, act(MobileAction::CloseQrOverlay));
                DispatchEventResult::StopPropagation
            })
            .on_keydown(move |ctx, _, key| match key.key.as_str() {
                "escape" => {
                    ctx.dispatch_typed_action_for(id, act(MobileAction::CloseQrOverlay));
                    DispatchEventResult::StopPropagation
                }
                _ => DispatchEventResult::PropagateToParent,
            })
            .finish(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_labels() {
        assert_eq!(platform_label("ios"), "iOS");
        assert_eq!(platform_label("iOS"), "iOS");
        assert_eq!(platform_label("android"), "Android");
        assert_eq!(platform_label("web"), "Web");
        assert_eq!(platform_label(""), "Phone");
        assert_eq!(platform_label("windows"), "Windows");
    }

    #[test]
    fn dates_read_as_a_day() {
        assert_eq!(paired_date("2026-11-03T12:00:00Z"), "Nov 3, 2026");
        assert_eq!(paired_date(""), "");
        assert_eq!(paired_date("n/a"), "n/a");
    }

    #[test]
    fn codes_group_in_fours() {
        assert_eq!(
            grouped_code("K7MQ2XRB9TGH4VNC8WPD"),
            "K7MQ 2XRB 9TGH 4VNC 8WPD"
        );
        assert_eq!(grouped_code("ABCDE"), "ABCD E");
        assert_eq!(grouped_code(""), "");
    }

    #[test]
    fn card_stacks_only_when_the_pane_is_narrow() {
        assert!(pair_card_stacks(560.0));
        assert!(!pair_card_stacks(900.0));
        // Before the first layout pass the width is unknown.
        assert!(!pair_card_stacks(0.0));
    }
}
