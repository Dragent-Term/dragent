use std::sync::Arc;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image, MainAxisAlignment,
    MainAxisSize, ParentElement, Shrinkable, Text, Wrap,
};
use warpui::AppContext;
use warpui::{GetSingletonModelHandle, ReadModel, SingletonEntity};
use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId, AsyncAssetType};
use warpui_core::elements::WrapFill;
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::auth::{AuthManager, AuthStatus};
use crate::theme::Theme;

use super::super::account_store::AccountStore;
use super::super::action::{SettingsAction, SettingsSection};
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    accent_outline_button, alert_banner, card_rows, danger_button, group_label, page_header,
    primary_button, secondary_button, setting_row, toggle_row, usage_percent_row,
};

pub fn render_account(view: &SettingsView, theme: &Theme, app: &AppContext) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;

    let auth = AuthManager::handle(app);
    let (logged_in, auth_user) = app.read_model(&auth, |a, _| {
        let user = match a.status() {
            AuthStatus::LoggedIn(u) => Some(u.clone()),
            _ => None,
        };
        (a.is_logged_in(), user)
    });

    // Prefer server-fetched account data; fall back to auth session fields.
    let account = AccountStore::handle(app);
    let (account_data, account_loading) =
        app.read_model(&account, |s, _| (s.data.clone(), s.loading));

    let display_name = account_data
        .as_ref()
        .map(|d| d.user.display_name.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            auth_user.as_ref().and_then(|u| {
                match (u.first_name.as_deref(), u.last_name.as_deref()) {
                    (Some(f), Some(l)) if !f.is_empty() => Some(format!("{f} {l}")),
                    (Some(f), _) if !f.is_empty() => Some(f.to_string()),
                    (_, Some(l)) if !l.is_empty() => Some(l.to_string()),
                    _ => u.user_id.clone(),
                }
            })
        })
        .unwrap_or_else(|| "Dragent User".to_string());

    let initials: String = display_name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    let sub_text = if logged_in {
        account_data
            .as_ref()
            .map(|d| d.user.email.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                auth_user
                    .as_ref()
                    .and_then(|u| u.user_id.as_deref())
                    .map(|uid| format!("@{uid}"))
            })
            .unwrap_or_else(|| "dragent.dev".to_string())
    } else {
        "Not signed in".to_string()
    };

    let tier = account_data
        .as_ref()
        .map(|d| d.user.tier.as_str())
        .unwrap_or("free")
        .to_string();
    let tier_label = match tier.as_str() {
        "pro" => "Pro Plan",
        "ultra" => "Ultra Plan",
        "mega" => "Mega Plan",
        _ => "Free Plan",
    };

    // ── Profile card ──────────────────────────────────────────────────────────
    let avatar_initials = if initials.is_empty() {
        "D".to_string()
    } else {
        initials
    };

    // Profile photo URL: server-fetched account data first, then the auth
    // session (populated by the OAuth providers). Empty string = no photo.
    let avatar_url = account_data
        .as_ref()
        .map(|d| d.user.avatar_url.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            auth_user
                .as_ref()
                .and_then(|u| u.avatar_url.clone())
                .filter(|s| !s.is_empty())
        });

    let initials_avatar = |text: &str| -> Box<dyn Element> {
        Container::new(
            Align::new(
                Text::new(text.to_string(), ui, crate::theme::text_size(20.0))
                    .with_color(theme.accent_text())
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.accent)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(26.0)))
        .finish()
    };

    // Photo when we have a URL — initials while it loads or if it fails.
    let avatar_inner: Box<dyn Element> = match &avatar_url {
        Some(url) => Image::new(avatar_photo_source(url), CacheOption::BySize)
            .cover()
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(26.0)))
            .before_load(initials_avatar(&avatar_initials))
            .on_load_failure(initials_avatar(&avatar_initials))
            .finish(),
        None => initials_avatar(&avatar_initials),
    };

    let avatar = ConstrainedBox::new(avatar_inner)
        .with_width(52.0)
        .with_height(52.0)
        .finish();

    let plan_badge = Container::new(
        Text::new(tier_label.to_string(), ui, crate::theme::text_size(11.5))
            .with_color(theme.accent)
            .finish(),
    )
    .with_background_color(theme.accent_overlay())
    .with_border(Border::all(1.0).with_border_color(ColorU::new(
        theme.accent.r,
        theme.accent.g,
        theme.accent.b,
        60,
    )))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish();

    // Name column: display name with email/sub-text below.
    let name_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(display_name, ui, crate::theme::text_size(15.5))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(sub_text, ui, crate::theme::text_size(13.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        )
        .finish();

    // Left cluster: [avatar] [name col]. The plan badge sits at the far right of
    // the card; using Wrap + WrapFill (same idiom as `setting_row`) lets the
    // badge drop to its own run when the window is too narrow, instead of
    // overlapping the name column.
    let identity = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(16.0)
        .with_child(avatar)
        .with_child(Shrinkable::new(1.0, name_col).finish())
        .finish();

    let profile_card = Container::new(
        Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_run_spacing(12.0)
            .with_child(WrapFill::new(200.0, identity).finish())
            .with_child(plan_badge)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_padding_left(18.0)
    .with_padding_right(18.0)
    .with_padding_top(18.0)
    .with_padding_bottom(18.0)
    .finish();

    // ── Usage bars ─────────────────────────────────────────────────────────────
    // Every user has a free MONTHLY allowance, usable at any pace. Paid users
    // ALSO have a plan allowance,
    // which is spent first (then the free one, then credits). So paid users get
    // two meters; free users just the free one. Values are micro-cents shown as
    // credits (1¢ each). Both renew monthly.
    let is_paid = tier != "free";
    let mut usage_rows: Vec<Box<dyn Element>> = Vec::new();

    // Builds one "<name>  ·  resets <date>" allowance meter row.
    let meter_label = |name: &str, resets_at: i64| -> String {
        if resets_at > 0 {
            format!(
                "{name}  \u{00B7}  resets {}",
                format_unix_datetime_local(resets_at)
            )
        } else {
            name.to_string()
        }
    };

    // Plan allowance (paid tiers only) — spent before the free allowance.
    if is_paid {
        let p = account_data.as_ref().map(|d| &d.usage.paid);
        let used = p.map(|p| p.used_mc).unwrap_or(0);
        let cap = p.map(|p| p.cap_mc.max(1)).unwrap_or(1);
        let resets = p.map(|p| p.resets_at).unwrap_or(0);
        usage_rows.push(usage_percent_row(
            theme,
            ui,
            &meter_label("Plan allowance", resets),
            used,
            cap,
            theme.accent,
        ));
    }

    // Free allowance — every user has one.
    {
        let f = account_data.as_ref().map(|d| &d.usage.free);
        let used = f.map(|f| f.used_mc).unwrap_or(0);
        let cap = f.map(|f| f.cap_mc.max(1)).unwrap_or(1);
        let resets = f.map(|f| f.resets_at).unwrap_or(0);
        usage_rows.push(usage_percent_row(
            theme,
            ui,
            &meter_label("Free allowance", resets),
            used,
            cap,
            theme.accent,
        ));
    }

    // Credit balance + the opt-in that governs it. Both belong beside the
    // usage meters: this is exactly where a user reads "I'm at my limit" and
    // decides what to do about it.
    let deposit_mc = account_data
        .as_ref()
        .map(|d| d.usage.deposit_balance_mc)
        .unwrap_or(0);
    let credits_on = account_data
        .as_ref()
        .map(|d| d.usage.allow_credit_usage)
        .unwrap_or(false);
    if logged_in {
        // Through the shared helper — 1 credit = 1 cent, defined once so this
        // and every allowance meter uses the same conversion.
        let credits = crate::settings::account_store::mc_to_credits(deposit_mc);
        usage_rows.push(setting_row(
            theme,
            ui,
            "Top-up credits",
            Some("Purchased credits never expire and are separate from your plan."),
            Box::new(
                Text::new(
                    format!("{credits} credits"),
                    ui,
                    crate::theme::text_size(12.5),
                )
                .with_color(if deposit_mc > 0 {
                    theme.main_text()
                } else {
                    theme.sub_text()
                }),
            ),
        ));
        usage_rows.push(toggle_row(
            theme,
            ui,
            id,
            view.hover("credit_usage"),
            "Use my credits when limits are reached",
            Some(
                "Turns on automatically when you buy credits. When off, requests are refused once your monthly allowance is used up, instead of spending credits. Your monthly allowance is always used before purchased credits.",
            ),
            credits_on,
            SettingsAction::ToggleCreditUsage,
        ));
    }

    let usage_card = card_rows(theme, usage_rows);

    // ── Session section ───────────────────────────────────────────────────────
    let session_card = if logged_in {
        card_rows(
            theme,
            vec![
                setting_row(
                    theme,
                    ui,
                    "Billing & credits",
                    Some("Your organisation's plan, credit packs, and usage."),
                    accent_outline_button(
                        theme,
                        ui,
                        id,
                        view.hover("billing"),
                        "Open Billing & Usage",
                        SettingsAction::SelectSection(SettingsSection::Billing),
                    ),
                ),
                setting_row(
                    theme,
                    ui,
                    "Sign out",
                    Some("Sign out of your Dragent account on this device."),
                    danger_button(
                        theme,
                        ui,
                        id,
                        view.hover("signout"),
                        "Sign out",
                        SettingsAction::SignOut,
                    ),
                ),
            ],
        )
    } else {
        // While a browser sign-in waits, show the code the browser asks the
        // user to match before approving, large enough to compare.
        let pending_code = app.read_model(&auth, |a, _| a.pending_sign_in_code());
        let sign_in_desc = if pending_code.is_some() {
            "Waiting for the browser. Approve there only if it shows the code below."
        } else {
            "Connect your Dragent account to unlock AI features."
        };
        let rows = card_rows(
            theme,
            vec![setting_row(
                theme,
                ui,
                "Sign in",
                Some(sign_in_desc),
                primary_button(
                    theme,
                    ui,
                    id,
                    view.hover("signin"),
                    "Sign in with browser",
                    SettingsAction::SignIn,
                ),
            )],
        );
        match pending_code {
            Some(code) => Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(rows)
                .with_child(crate::setup::browser_auth_view::render_code(
                    theme, ui, &code,
                ))
                .finish(),
            None => rows,
        }
    };

    let refresh_label = if account_loading {
        "Refreshing…"
    } else {
        "Refresh"
    };
    let usage_header = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(group_label(theme, ui, "Usage"))
            .with_child(secondary_button(
                theme,
                ui,
                id,
                view.hover("refresh_account"),
                refresh_label,
                SettingsAction::RefreshAccount,
            ))
            .finish(),
    )
    .with_padding_bottom(4.0)
    .finish();

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0)
        .with_child(page_header(
            theme,
            ui,
            "Account",
            "Manage your Dragent profile, plan, and usage.",
        ))
        .with_child(profile_card)
        .with_child(usage_header)
        .with_child(usage_card)
        .with_child(group_label(theme, ui, "Session"))
        .with_child(session_card);

    if let Some(msg) = &view.auth_status_message {
        col = col.with_child(
            Container::new(alert_banner(theme, ui, msg))
                .with_padding_top(16.0)
                .finish(),
        );
    }

    col.finish()
}

/// Namespace marker for profile photos fetched by URL.
struct AvatarAsset;
impl AsyncAssetType for AvatarAsset {}

/// Async image source for the profile photo. The asset cache calls `fetch` at
/// most once per URL and keeps the decoded image, so re-renders are free.
pub(crate) fn avatar_photo_source(url: &str) -> AssetSource {
    let fetch_url = url.to_string();
    AssetSource::Async {
        id: AsyncAssetId::new::<AvatarAsset>(url),
        fetch: Arc::new(move || {
            let url = fetch_url.clone();
            Box::pin(fetch_avatar_async(url))
        }),
    }
}

/// Native: `ureq` on a background thread so the shared async executor never
/// stalls; the future only awaits a channel.
#[cfg(not(target_family = "wasm"))]
async fn fetch_avatar_async(url: String) -> anyhow::Result<bytes::Bytes> {
    let (tx, rx) = futures::channel::oneshot::channel();
    crate::bg::detach(move || {
        let _ = tx.send(fetch_avatar_bytes(&url));
    });
    rx.await
        .map_err(|_| anyhow::anyhow!("avatar fetch thread dropped"))?
}

/// Web viewer: no threads or `ureq`. Fetch through the host page's same-origin
/// `/api/avatar` proxy — participant avatars are cross-origin OAuth-CDN URLs
/// that the browser won't let a canvas read directly (no CORS), so the proxy
/// re-serves the bytes from our own origin.
#[cfg(target_family = "wasm")]
async fn fetch_avatar_async(url: String) -> anyhow::Result<bytes::Bytes> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let proxied = format!(
        "/api/avatar?url={}",
        js_sys::encode_uri_component(&url)
            .as_string()
            .unwrap_or(url)
    );
    let win = web_sys::window().ok_or_else(|| anyhow::anyhow!("no window"))?;
    let resp_val = JsFuture::from(win.fetch_with_str(&proxied))
        .await
        .map_err(|e| anyhow::anyhow!("avatar fetch failed: {e:?}"))?;
    let resp: web_sys::Response = resp_val
        .dyn_into()
        .map_err(|_| anyhow::anyhow!("avatar fetch: not a Response"))?;
    if !resp.ok() {
        anyhow::bail!("avatar fetch HTTP {}", resp.status());
    }
    let buf = JsFuture::from(
        resp.array_buffer()
            .map_err(|e| anyhow::anyhow!("array_buffer: {e:?}"))?,
    )
    .await
    .map_err(|e| anyhow::anyhow!("array_buffer await: {e:?}"))?;
    let array = js_sys::Uint8Array::new(&buf);
    Ok(bytes::Bytes::from(array.to_vec()))
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn fetch_avatar_bytes(url: &str) -> anyhow::Result<bytes::Bytes> {
    use std::io::Read;
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(20))
        .call()?;
    let mut buf = Vec::new();
    // Avatars are small; cap the read so a hostile URL can't balloon memory.
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(bytes::Bytes::from(buf))
}

/// Format a Unix timestamp as "Jan 2" for the reset date label.
/// Format a unix timestamp as a LOCAL wall-clock time like "5:30 PM" — the
/// form the user expects for a same-day session reset.
pub(crate) fn format_clock_local(unix_secs: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(unix_secs, 0).single() {
        Some(dt) => dt.format("%-I:%M %p").to_string(),
        None => "soon".to_string(),
    }
}

fn format_unix_date(unix_secs: i64) -> String {
    // chrono, in the user's local timezone. The previous hand-rolled version
    // ignored leap years (`days % 365`), which put every date 14 days late —
    // a Jul 17 reset displayed as Jul 31.
    chrono::DateTime::from_timestamp(unix_secs, 0)
        .map(|dt: chrono::DateTime<chrono::Utc>| {
            dt.with_timezone(&chrono::Local)
                .format("%b %-d")
                .to_string()
        })
        .unwrap_or_default()
}

/// Date AND local time, e.g. "Jul 20, 10:30 PM".
///
/// The monthly allowance resets at whatever time of day the cycle began, not at
/// midnight. Showing only the date implies midnight and can be up to 24 hours
/// wrong about when you can work again, which is the entire question the user is
/// asking.
pub(crate) fn format_unix_datetime_local(unix_secs: i64) -> String {
    chrono::DateTime::from_timestamp(unix_secs, 0)
        .map(|dt: chrono::DateTime<chrono::Utc>| {
            dt.with_timezone(&chrono::Local)
                .format("%b %-d, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod date_tests {
    use super::{format_unix_date, format_unix_datetime_local};

    #[test]
    fn leap_years_are_respected() {
        // 2026-07-17 12:00 UTC. The old formatter rendered this as "Jul 31".
        // Local-timezone conversion can shift ±1 day around midnight; noon UTC
        // stays Jul 17 for any offset within ±11h.
        assert_eq!(format_unix_date(1_784_289_600), "Jul 17");
    }

    /// The monthly allowance resets at the time of day the cycle began, not at
    /// midnight, so the reset label must carry the time of day — a date alone
    /// can be up to 24 hours wrong about when work can resume.
    #[test]
    fn reset_label_includes_the_time() {
        let s = format_unix_datetime_local(1_784_289_600);
        assert!(
            s.contains("AM") || s.contains("PM"),
            "expected a clock time in {s:?}"
        );
        assert!(
            s.contains(','),
            "expected 'Mon D, H:MM AM/PM' shape, got {s:?}"
        );
        // Still leads with the same date the date-only formatter produces.
        assert!(s.starts_with(&format_unix_date(1_784_289_600)), "got {s:?}");
    }
}
