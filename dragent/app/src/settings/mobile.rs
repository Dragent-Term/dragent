//! Settings → Mobile: pair a phone by QR and manage paired phones.
//!
//! The phone never signs in: the signed-in desktop creates a single-use
//! pairing code (POST /v1/companion/pairings) and shows it as a QR.
//!
//! With the `e2ee` feature the QR carries the v2 link (§6.2 step 3 of
//! `docs/design/e2ee.md`):
//! `<scheme>://pair?v=2&c=<code>&g=<gateway urlencoded>&d=<desktop id>&k=<b64url identity pub>&p=<b64url P>`,
//! where `<scheme>` is this edition's `dragent_paths::url_scheme()`.
//! `P` is generated and registered by the live companion host
//! (`realtime::companion_host::e2ee_live::begin_pairing`), which holds it for
//! the pairing handshake and wipes it when pairing commits, is cancelled or
//! replaced, fails three times or lapses. The page keeps only the link it
//! shows ([`PairingSecret`]: never persisted, never logged, wiped when the
//! code lapses or is replaced), and the person pastes the full link when they
//! have no camera — there is no bare-code entry. Removing a phone and
//! resetting keys also go through the live host, so open channels close at
//! once (§6.4, §6.6). Without the feature the QR carries this edition's v1
//! `<scheme>://pair?c=<code>&g=<gateway urlencoded>` link plus the code as text.
//!
//! Every network call runs on a background thread via [`crate::bg::detach`] —
//! the UI thread never blocks.

use warpui::ViewContext;
use warpui::{SingletonEntity, UpdateModel};
use warpui_core::clipboard::ClipboardContent;

use super::settings_view::SettingsView;
use crate::api::{self, ApiError, CompanionDevice};

const REQUEST_TIMEOUT_SECS: u64 = 15;

#[derive(Clone, Debug)]
pub enum MobileAction {
    /// Create a new single-use pairing code (background POST).
    CreatePairing,
    /// Copy the shown code to the clipboard (paste fallback for the phone).
    CopyCode,
    /// Copy the full pairing link (the no-camera fallback for a v2 QR).
    CopyLink,
    /// Enlarge the shown QR code in an overlay (click/Esc closes it).
    EnlargeQr,
    /// Close the enlarged-QR overlay.
    CloseQrOverlay,
    /// Re-list the paired phones (background GET).
    RefreshDevices,
    /// Arm the inline remove confirm for one paired phone.
    RequestRemove(String),
    /// Cancel the armed inline remove confirm.
    CancelRemove,
    /// Remove one paired phone (local tombstone first, then background
    /// DELETE), after confirm.
    RemoveDevice(String),
    /// Arm the inline "Reset security keys" confirm (§6.6).
    RequestReset,
    /// Cancel the armed reset confirm.
    CancelReset,
    /// Reset this desktop's security keys (background), after confirm.
    ResetKeys,
    /// Dismiss the security notice banner.
    DismissNotice,
}

/// A live pairing code shown as a QR.
#[derive(Clone, Debug, Default)]
pub struct MobilePairing {
    pub code: String,
    pub deep_link: String,
    pub expires_at_epoch: i64,
    /// Raw-image asset id the QR was registered under (for the `Image` element).
    pub qr_asset_id: Option<String>,
    /// The v2 pairing secret `P` (§6.2 step 2). `Some` exactly when the shown
    /// link is a v2 link; `None` for the v1 fallback. Held in memory only and
    /// wiped on drop — it is never persisted and never logged.
    #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
    pub(crate) secret: Option<PairingSecret>,
}

/// The v2 pairing secret `P` (§6.2 step 2): 32 random bytes. Clones carry the
/// same bytes (each copy wipes itself on drop); the `Debug` impl prints
/// nothing.
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
#[derive(Clone, Default)]
pub(crate) struct PairingSecret(Vec<u8>);

#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
impl PairingSecret {
    /// Fresh random bytes from the OS generator. `None` when randomness is
    /// unavailable — pairing stays off rather than reusing a secret.
    #[cfg(test)]
    pub(crate) fn generate() -> Option<Self> {
        let mut bytes = vec![0u8; 32];
        dragent_e2ee::fill_random(&mut bytes).ok()?;
        Some(PairingSecret(bytes))
    }

    /// The secret the live host registered for this code (`p`, base64url).
    fn registered(link: &PairingLink) -> Option<Self> {
        let bytes = dragent_e2ee::codec::b64::decode(link.secret_b64()).ok()?;
        (bytes.len() == 32).then_some(PairingSecret(bytes))
    }

    /// `p` for the v2 link: base64url without padding.
    pub(crate) fn b64(&self) -> String {
        dragent_e2ee::codec::b64::encode(&self.0)
    }
}

#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
impl Drop for PairingSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
impl std::fmt::Debug for PairingSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingSecret(Redacted)")
    }
}

/// The v2 pairing link for the QR (§6.2 step 3):
/// `<scheme>://pair?v=2&c=<code>&g=<gateway>&d=<desktop id>&k=<b64url identity pub>&p=<b64url P>`,
/// where `<scheme>` is this edition's `dragent_paths::url_scheme()`.
/// `c`, `k` and `p` are already URL-safe; the gateway and desktop id are
/// form-encoded.
pub(crate) fn pairing_v2_link(
    code: &str,
    gateway_url: &str,
    desktop_id: &str,
    key_b64: &str,
    secret_b64: &str,
) -> String {
    let gateway: String = url::form_urlencoded::byte_serialize(gateway_url.as_bytes()).collect();
    let desktop: String = url::form_urlencoded::byte_serialize(desktop_id.as_bytes()).collect();
    let scheme = dragent_paths::url_scheme();
    format!("{scheme}://pair?v=2&c={code}&g={gateway}&d={desktop}&k={key_b64}&p={secret_b64}")
}

#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
impl MobilePairing {
    /// Whether the shown link is a v2 link (it carries `P`).
    pub(crate) fn is_v2(&self) -> bool {
        self.secret.is_some()
    }
}

#[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
impl MobilePairing {
    /// Without the `e2ee` feature every link is v1.
    pub(crate) fn is_v2(&self) -> bool {
        false
    }
}

/// What the live host registered for a new code (§6.2 step 3): this
/// desktop's id, its identity public key and the secret `P`.
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub(crate) use crate::realtime::companion_host::e2ee_live::{PairingLink, PhoneAccessError};

/// Without the `e2ee` feature there is no pairing secret to register.
#[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
pub(crate) struct PairingLink;

/// The §9 text when phone access is off on this computer.
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub(crate) fn phone_access_off_message() -> String {
    match crate::realtime::e2e_store::desktop() {
        Ok(_) => "Phone access is off on this computer.".to_string(),
        Err(error) => format!("Phone access is off on this computer: {error}"),
    }
}

/// Without the `e2ee` feature there is no identity: pairing is unavailable.
#[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
pub(crate) fn phone_access_off_message() -> String {
    "Phone access needs the encrypted phone relay.".to_string()
}

impl MobilePairing {
    /// Assemble the shown pairing from a fresh server code and the secret the
    /// live host registered for it. With `e2ee` this is the v2 link (§6.2
    /// step 3); `None` when nothing was registered (phone access off, §6.1:
    /// fail closed, no file fallback) so the page shows the §9 state instead
    /// of a QR. Without `e2ee` this is the v1 link and never fails.
    pub(crate) fn from_server(
        code: String,
        gateway_url: &str,
        expires_at_epoch: i64,
        link: Option<&PairingLink>,
    ) -> Option<Self> {
        #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
        {
            let link = link?;
            let secret = PairingSecret::registered(link)?;
            let deep_link = pairing_v2_link(
                &code,
                gateway_url,
                &link.desktop_id,
                &link.key_b64,
                link.secret_b64(),
            );
            Some(Self {
                code,
                deep_link,
                expires_at_epoch,
                qr_asset_id: None,
                secret: Some(secret),
            })
        }
        #[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
        {
            let _ = link;
            let deep_link = api::companion_pairing_deep_link(code.as_str(), gateway_url);
            Some(Self {
                code,
                deep_link,
                expires_at_epoch,
                qr_asset_id: None,
            })
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MobileState {
    pub pairing: Option<MobilePairing>,
    pub creating: bool,
    /// The enlarged-QR overlay is open (closes on click/Esc, expiry, pairing).
    pub qr_enlarged: bool,
    /// The code was just copied — the Copy button reads "Copied" briefly.
    pub copied: bool,
    /// `ui_preview` only: draw the narrow-window layout at any width.
    pub preview_stacked: bool,
    /// Device ids known when the live code was created — a new id appearing
    /// while the code is shown means the phone just paired.
    pub pair_snapshot: Vec<String>,
    /// Inline remove confirm armed for this device id (second click removes).
    pub confirm_remove: Option<String>,
    pub devices: Vec<CompanionDevice>,
    pub devices_loading: bool,
    pub devices_error: Option<String>,
    pub revoking: Option<String>,
    pub notice: Option<String>,
    /// The §9 security banner: paired devices were reset (pin store failed its
    /// check, the keychain item was lost, or keys were just reset). Shown
    /// until dismissed. Set from the desktop's reset notice on page open and
    /// by the reset flow.
    pub e2ee_notice: Option<String>,
    /// Phone access is off on this computer (§6.1/§9: no secure store, or the
    /// identity is unavailable). Set on page open; `None` means ready.
    pub e2ee_off: Option<String>,
    /// Short id of this desktop's identity key (first bytes of `k`), so the
    /// person can tell their computers apart. Set on page open.
    pub e2ee_key_id: Option<String>,
    /// The inline "Reset security keys" confirm (§6.6) is armed.
    pub confirm_reset: bool,
    /// A key reset is running on a background thread.
    pub resetting: bool,
    pairing_generation: u64,
    devices_generation: u64,
    tick_armed: bool,
    poll_armed: bool,
}

/// Refresh state of the shown pairing code: the 1 s countdown repaint, the
/// expiry auto-refresh, and the renderer's countdown label all read this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairingStatus {
    /// No code on screen.
    Idle,
    /// A code is shown with this many seconds left.
    Live(u64),
    /// The shown code lapsed — fetch a replacement.
    Expired,
}

pub(crate) fn pairing_status(pairing: &Option<MobilePairing>, now_epoch: i64) -> PairingStatus {
    match pairing {
        None => PairingStatus::Idle,
        Some(pairing) => {
            let remaining = api::pairing_remaining_secs(pairing.expires_at_epoch, now_epoch);
            if remaining == 0 {
                PairingStatus::Expired
            } else {
                PairingStatus::Live(remaining)
            }
        }
    }
}

pub(crate) fn now_epoch_secs() -> i64 {
    web_time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Server TTL of a pairing code (10 min). The expiry progress bar reads
/// `remaining / PAIRING_TTL_SECS`.
pub(crate) const PAIRING_TTL_SECS: u64 = 600;

/// Fill fraction of the expiry progress bar: 1.0 fresh, 0.0 lapsed.
pub(crate) fn pairing_progress_fraction(remaining_secs: u64) -> f32 {
    (remaining_secs as f32 / PAIRING_TTL_SECS as f32).clamp(0.0, 1.0)
}

/// Last-seen presence of a paired phone, from its `last_seen_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevicePresence {
    Connected,
    Reconnecting,
    Offline,
}

/// Recently seen (≤ 2 min) → Connected; seen within 15 min → Reconnecting
/// (the phone may be waking up); anything older, missing or unparsable →
/// Offline. Future timestamps (clock skew) count as Connected.
pub(crate) fn device_presence(last_seen_at: Option<&str>, now_epoch: i64) -> DevicePresence {
    let seen = last_seen_at.and_then(parse_seen_epoch);
    match seen {
        Some(seen) if now_epoch.saturating_sub(seen) <= 120 => DevicePresence::Connected,
        Some(seen) if now_epoch.saturating_sub(seen) <= 900 => DevicePresence::Reconnecting,
        _ => DevicePresence::Offline,
    }
}

/// `last_seen_at` arrives as RFC 3339; accept plain epoch seconds (or millis)
/// too so a shape change degrades to Offline rather than a parse crash.
fn parse_seen_epoch(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(epoch) = value.parse::<i64>() {
        return Some(if epoch > 1_000_000_000_000 {
            epoch / 1000
        } else {
            epoch
        });
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Short, human-readable id of an identity key: the first 8 characters of its
/// base64url form, so the person can tell their computers apart when the
/// phone says a desktop's key changed (§9).
pub(crate) fn short_key_id(key_b64: &str) -> String {
    key_b64.chars().take(8).collect()
}

/// Friendly one-liner for a pairing/devices request failure.
pub(crate) fn mobile_error_message(error: &ApiError) -> String {
    match error {
        ApiError::LocalMode => crate::api::NEEDS_ACCOUNT.to_string(),
        ApiError::NotAuthenticated => "Sign in to pair a phone.".to_string(),
        ApiError::Network(_) => {
            "Couldn't reach Dragent. Check your connection and try again.".to_string()
        }
        ApiError::Server { status: 410, .. } => {
            "That code already expired or was used. Get a new one.".to_string()
        }
        ApiError::Server { status: 429, .. } => {
            "Too many tries. Wait a bit and try again.".to_string()
        }
        other => other.user_message(),
    }
}

/// Register a fresh pairing secret with the live host (background thread
/// only: it waits for the host's state lock). `Err` carries the §9 text.
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
fn register_pairing_secret() -> Result<PairingLink, String> {
    crate::realtime::companion_host::e2ee_live::begin_pairing().map_err(|e| e.to_string())
}

/// Without the `e2ee` feature the v1 link carries no secret.
#[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
fn register_pairing_secret() -> Result<PairingLink, String> {
    Ok(PairingLink)
}

/// Encode the deep link as a QR bitmap and register it with the asset cache
/// so the page can render it with an `Image` (same as the share sheet).
fn register_pairing_qr(ctx: &mut ViewContext<SettingsView>, deep_link: &str) -> Option<String> {
    let qr = crate::realtime::qr::encode(deep_link)?;
    let bytes = warpui_core::image_cache::CustomImageHeader::prepend_custom_header(
        qr.rgba,
        qr.size,
        qr.size,
        warpui_core::image_cache::CustomImageFormat::Rgba,
    )
    .ok()?;
    let cache = warpui_core::assets::asset_cache::AssetCache::handle(ctx);
    let id = qr.id.clone();
    ctx.update_model(&cache, |c, cctx| {
        c.insert_raw_asset_bytes::<warpui_core::image_cache::ImageType>(id, &bytes, cctx)
    });
    Some(qr.id)
}

impl SettingsView {
    /// List the paired phones (off the UI thread). Called when the Mobile
    /// page opens, on manual refresh, and after a revoke lands.
    pub(crate) fn refresh_mobile_devices(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_mobile_devices_inner(ctx, false);
    }

    /// The 3 s poll while a code is live: same fetch, but without flipping
    /// the card into its "Loading…" state on every pass.
    fn refresh_mobile_devices_quiet(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_mobile_devices_inner(ctx, true);
    }

    fn refresh_mobile_devices_inner(&mut self, ctx: &mut ViewContext<Self>, quiet: bool) {
        if !quiet {
            self.mobile.devices_loading = true;
            // Page open or manual refresh: re-read the security state too, so
            // a reset notice (§9) surfaces as its banner exactly once.
            self.refresh_mobile_security();
        }
        self.mobile.devices_error = None;
        self.mobile.devices_generation = self.mobile.devices_generation.wrapping_add(1);
        let generation = self.mobile.devices_generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result = crate::api::API_CLIENT.list_companion_devices(REQUEST_TIMEOUT_SECS);
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |view, result, ctx| {
            if view.mobile.devices_generation != generation {
                return;
            }
            view.mobile.devices_loading = false;
            match result {
                Ok(Ok(devices)) => {
                    // A phone scanned the live code: a device id nobody had
                    // when the code was created. Celebrate, then clear the
                    // code — its single use is spent.
                    let live = matches!(
                        pairing_status(&view.mobile.pairing, now_epoch_secs()),
                        PairingStatus::Live(_)
                    );
                    if live
                        && devices
                            .iter()
                            .any(|d| !view.mobile.pair_snapshot.contains(&d.device_id))
                    {
                        view.mobile.notice = Some("Phone paired.".to_string());
                        view.mobile.pairing = None;
                        view.mobile.qr_enlarged = false;
                        view.mobile.copied = false;
                    }
                    view.mobile.devices = devices;
                    view.mobile.devices_error = None;
                }
                Ok(Err(error)) => {
                    view.mobile.devices_error = Some(mobile_error_message(&error));
                }
                Err(_) => {
                    view.mobile.devices_error = Some("Pairing request was cancelled.".to_string());
                }
            }
            ctx.notify();
        });
    }

    /// Re-read the E2EE security state for the page: whether phone access is
    /// off on this computer, and any reset notice (§9) the desktop recorded
    /// when its keys or records were lost. Taking the notice shows it once.
    pub(crate) fn refresh_mobile_security(&mut self) {
        #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
        match crate::realtime::e2e_store::desktop() {
            Ok(desktop) => {
                self.mobile.e2ee_off = None;
                self.mobile.e2ee_key_id = Some(short_key_id(&dragent_e2ee::codec::b64::encode(
                    desktop.public().as_bytes(),
                )));
                if let Some(notice) = desktop.take_notice() {
                    self.mobile.e2ee_notice = Some(notice.message().to_string());
                }
            }
            Err(error) => {
                self.mobile.e2ee_off =
                    Some(format!("Phone access is off on this computer: {error}"));
                self.mobile.e2ee_key_id = None;
            }
        }
        #[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
        {
            self.mobile.e2ee_off = None;
        }
    }

    /// Create a single-use pairing code (off the UI thread), then show its QR.
    fn create_mobile_pairing(&mut self, ctx: &mut ViewContext<Self>) {
        if self.mobile.creating {
            return;
        }
        self.mobile.creating = true;
        self.mobile.notice = None;
        self.mobile.pairing = None;
        self.mobile.qr_enlarged = false;
        self.mobile.copied = false;
        self.mobile.confirm_remove = None;
        // Phones known before this code existed — anything else that shows
        // up while it is live just scanned it.
        self.mobile.pair_snapshot = self
            .mobile
            .devices
            .iter()
            .map(|d| d.device_id.clone())
            .collect();
        self.mobile.pairing_generation = self.mobile.pairing_generation.wrapping_add(1);
        let generation = self.mobile.pairing_generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result = crate::api::API_CLIENT
                .create_companion_pairing(REQUEST_TIMEOUT_SECS)
                .map(|pairing| {
                    // §6.2 step 2: register `P` with the live host for this
                    // code (it replaces any earlier offer).
                    let link = register_pairing_secret();
                    (pairing, link)
                });
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |view, result, ctx| {
            if view.mobile.pairing_generation != generation {
                return;
            }
            view.mobile.creating = false;
            match result {
                Ok(Ok((pairing, link))) => {
                    let (link, refused) = match link {
                        Ok(link) => (Some(link), None),
                        Err(message) => (None, Some(message)),
                    };
                    match MobilePairing::from_server(
                        pairing.code,
                        &pairing.gateway_url,
                        pairing.expires_at_epoch,
                        link.as_ref(),
                    ) {
                        Some(mut shown) => {
                            shown.qr_asset_id = register_pairing_qr(ctx, &shown.deep_link);
                            view.mobile.pairing = Some(shown);
                            view.arm_mobile_tick(ctx);
                            view.arm_mobile_poll(ctx);
                        }
                        // No identity (no secure store): fail closed with the
                        // §9 text instead of a QR the phone could never use.
                        None => {
                            view.mobile.notice =
                                Some(refused.unwrap_or_else(phone_access_off_message));
                        }
                    }
                }
                Ok(Err(error)) => {
                    view.mobile.notice = Some(mobile_error_message(&error));
                }
                Err(_) => {
                    view.mobile.notice = Some("Pairing request was cancelled.".to_string());
                }
            }
            ctx.notify();
        });
    }

    /// Remove one phone (§6.4): the local pin and tombstone first, then the
    /// server DELETE (availability only) on a background thread, then refresh.
    fn remove_mobile_device(&mut self, device_id: String, ctx: &mut ViewContext<Self>) {
        if self.mobile.revoking.is_some() {
            return;
        }
        self.mobile.revoking = Some(device_id.clone());
        self.mobile.notice = None;
        self.mobile.devices_generation = self.mobile.devices_generation.wrapping_add(1);
        let generation = self.mobile.devices_generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            // §6.4: the local pin and tombstone first, under the live host's
            // state lock — its channels close, unwritten input is discarded
            // and the catalog is re-sealed without it — so the phone is
            // refused from now on even if the server call below fails. The
            // removal holds in memory even when the write itself fails.
            #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
            if let Err(error) = crate::realtime::companion_host::e2ee_live::revoke_phone(&device_id) {
                log::warn!("[mobile] local phone removal incomplete: {error}");
            }
            let result =
                crate::api::API_CLIENT.revoke_companion_device(&device_id, REQUEST_TIMEOUT_SECS);
            let _ = tx.send(result.map(|_| device_id));
        });
        ctx.spawn(rx, move |view, result, ctx| {
            if view.mobile.devices_generation != generation {
                return;
            }
            view.mobile.revoking = None;
            match result {
                Ok(Ok(_)) => {
                    view.mobile.notice = Some("Phone removed.".to_string());
                    view.refresh_mobile_devices(ctx);
                }
                Ok(Err(error)) => {
                    view.mobile.notice = Some(mobile_error_message(&error));
                    ctx.notify();
                }
                Err(_) => {
                    view.mobile.notice = Some("Pairing request was cancelled.".to_string());
                    ctx.notify();
                }
            }
        });
    }

    /// Reset this desktop's security keys (§6.6 compromise recovery): new keys
    /// now, every phone pin deleted and tombstoned, the shown QR discarded
    /// (it names the old key). Every phone must pair again. Off the UI thread.
    fn reset_mobile_keys(&mut self, ctx: &mut ViewContext<Self>) {
        if self.mobile.resetting {
            return;
        }
        self.mobile.resetting = true;
        self.mobile.confirm_reset = false;
        self.mobile.notice = None;
        self.mobile.devices_generation = self.mobile.devices_generation.wrapping_add(1);
        let generation = self.mobile.devices_generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            // §6.6 through the live host: new keys, every pin tombstoned,
            // every channel closed and zeroised, the old `P` discarded.
            #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
            let result = crate::realtime::companion_host::e2ee_live::reset_keys()
                .map_err(|e| e.to_string());
            #[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
            let result: Result<usize, String> =
                Err("Phone access needs the encrypted phone relay.".to_string());
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |view, result, ctx| {
            if view.mobile.devices_generation != generation {
                return;
            }
            view.mobile.resetting = false;
            match result {
                Ok(Ok(_)) => {
                    // The old QR names the old key: it must go (its secret is
                    // wiped on drop).
                    view.mobile.pairing = None;
                    view.mobile.qr_enlarged = false;
                    view.mobile.copied = false;
                    #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
                    {
                        view.mobile.e2ee_notice = Some(
                            crate::realtime::e2e_store::ResetNotice::KeysReset
                                .message()
                                .to_string(),
                        );
                    }
                    #[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
                    {
                        view.mobile.e2ee_notice = None;
                    }
                    view.refresh_mobile_devices(ctx);
                }
                Ok(Err(error)) => {
                    view.mobile.notice = Some(format!(
                        "Couldn't reset this computer's security keys: {error}"
                    ));
                    ctx.notify();
                }
                Err(_) => {
                    view.mobile.notice = Some("Pairing request was cancelled.".to_string());
                    ctx.notify();
                }
            }
        });
    }

    /// Repaint the countdown every second while a code is live. A lapsed
    /// code stays on screen as an expired card (with Regenerate) instead of
    /// being silently replaced — the user may still be reading it.
    pub(crate) fn arm_mobile_tick(&mut self, ctx: &mut ViewContext<Self>) {
        if self.mobile.tick_armed {
            return;
        }
        self.mobile.tick_armed = true;
        Self::spawn_mobile_tick(ctx);
    }

    fn spawn_mobile_tick(ctx: &mut ViewContext<SettingsView>) {
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(1)),
            |view: &mut SettingsView, _, ctx| {
                if view.section != super::action::SettingsSection::Mobile {
                    view.mobile.tick_armed = false;
                    return;
                }
                match pairing_status(&view.mobile.pairing, now_epoch_secs()) {
                    PairingStatus::Idle => {
                        view.mobile.tick_armed = false;
                    }
                    PairingStatus::Live(_) => {
                        ctx.notify();
                        Self::spawn_mobile_tick(ctx);
                    }
                    PairingStatus::Expired => {
                        // The shown code is dead — leave the expired card in
                        // place (with its Regenerate button) and stop ticking.
                        // `P`'s 10 minutes are over too: wipe it even though
                        // the card stays.
                        view.mobile.tick_armed = false;
                        view.mobile.qr_enlarged = false;
                        #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
                        if let Some(pairing) = view.mobile.pairing.as_mut() {
                            pairing.secret = None;
                            // The live host wipes `P` too (it also lapses on
                            // its own after 10 minutes).
                            crate::bg::detach(
                                crate::realtime::companion_host::e2ee_live::cancel_pairing,
                            );
                        }
                        ctx.notify();
                    }
                }
            },
        );
    }

    /// Re-list the phones every 3 s while a code is live, so the page
    /// notices the moment the phone scans it (see the refresh callback).
    /// Stops on its own when the code lapses, pairs, or the page closes.
    pub(crate) fn arm_mobile_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.mobile.poll_armed {
            return;
        }
        self.mobile.poll_armed = true;
        Self::spawn_mobile_poll(ctx);
    }

    fn spawn_mobile_poll(ctx: &mut ViewContext<SettingsView>) {
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(3)),
            |view: &mut SettingsView, _, ctx| {
                let live = matches!(
                    pairing_status(&view.mobile.pairing, now_epoch_secs()),
                    PairingStatus::Live(_)
                );
                if view.section != super::action::SettingsSection::Mobile || !live {
                    view.mobile.poll_armed = false;
                    return;
                }
                view.refresh_mobile_devices_quiet(ctx);
                Self::spawn_mobile_poll(ctx);
            },
        );
    }

    /// Sample data for a named `ui_preview` state. Nothing here calls the server.
    pub(crate) fn preview_mobile_state(&mut self, state: &str, ctx: &mut ViewContext<Self>) {
        let now = now_epoch_secs();
        let seen =
            |ago: i64| chrono::DateTime::from_timestamp(now - ago, 0).map(|at| at.to_rfc3339());
        let device =
            |id: &str, name: &str, platform: &str, last_seen: Option<String>| CompanionDevice {
                device_id: id.to_string(),
                name: name.to_string(),
                platform: platform.to_string(),
                created_at: "2026-11-03T12:00:00Z".to_string(),
                last_seen_at: last_seen,
            };
        let code = "K7MQ2XRB9TGH4VNC8WPD";
        #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
        let deep_link = pairing_v2_link(
            code,
            "https://api.example.test",
            "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            &"A".repeat(43),
            &"B".repeat(43),
        );
        #[cfg(not(all(feature = "e2ee", not(target_family = "wasm"))))]
        let deep_link = api::companion_pairing_deep_link(code, "https://api.example.test");
        let expires_at_epoch = match state {
            "expired" => now - 1,
            _ => now + 582,
        };
        let shows_code = matches!(
            state,
            "live" | "live_light" | "enlarged" | "expired" | "revoke" | "narrow"
        );
        self.mobile = MobileState::default();
        if shows_code {
            let qr_asset_id = register_pairing_qr(ctx, &deep_link);
            self.mobile.pairing = Some(MobilePairing {
                code: code.to_string(),
                deep_link,
                expires_at_epoch,
                qr_asset_id,
                #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
                secret: Some(PairingSecret(vec![7u8; 32])),
            });
        }
        if matches!(
            state,
            "paired" | "revoke" | "narrow" | "reset" | "no_store" | "records_reset"
        ) {
            self.mobile.devices = vec![
                device("phone-1", "Jordan's iPhone", "ios", seen(30)),
                device("phone-2", "Pixel 9", "android", seen(86_400)),
            ];
            self.mobile.e2ee_key_id = Some("AbC123Xy".to_string());
        }
        if state == "paired" {
            self.mobile.notice = Some("Phone paired.".to_string());
        }
        if state == "reset" {
            self.mobile.confirm_reset = true;
        }
        if state == "no_store" {
            self.mobile.e2ee_off = Some(
                "Phone access is off on this computer: secure key store unavailable: no storage access"
                    .to_string(),
            );
        }
        if state == "records_reset" {
            self.mobile.e2ee_notice = Some(
                "Dragent's security records on this device couldn't be verified, so paired devices were reset. Pair again."
                    .to_string(),
            );
        }
        self.mobile.qr_enlarged = state == "enlarged";
        if state == "revoke" {
            self.mobile.confirm_remove = Some("phone-1".to_string());
        }
        self.mobile.preview_stacked = state == "narrow";
        self.theme_id = if state == "live_light" {
            "light"
        } else {
            "dark"
        }
        .to_string();
    }

    pub(crate) fn handle_mobile_action(
        &mut self,
        action: &MobileAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            MobileAction::CreatePairing => self.create_mobile_pairing(ctx),
            MobileAction::CopyCode => {
                let code = self
                    .mobile
                    .pairing
                    .as_ref()
                    .map(|pairing| pairing.code.clone());
                self.copy_mobile_text(ctx, code);
            }
            MobileAction::CopyLink => {
                let link = self
                    .mobile
                    .pairing
                    .as_ref()
                    .map(|pairing| pairing.deep_link.clone());
                self.copy_mobile_text(ctx, link);
            }
            MobileAction::EnlargeQr => {
                if self.mobile.pairing.is_some() {
                    self.mobile.qr_enlarged = true;
                }
                ctx.notify();
            }
            MobileAction::CloseQrOverlay => {
                self.mobile.qr_enlarged = false;
                ctx.notify();
            }
            MobileAction::RefreshDevices => self.refresh_mobile_devices(ctx),
            MobileAction::RequestRemove(device_id) => {
                self.mobile.confirm_remove = Some(device_id.clone());
                ctx.notify();
            }
            MobileAction::CancelRemove => {
                self.mobile.confirm_remove = None;
                ctx.notify();
            }
            MobileAction::RemoveDevice(device_id) => {
                self.mobile.confirm_remove = None;
                self.remove_mobile_device(device_id.clone(), ctx)
            }
            MobileAction::RequestReset => {
                self.mobile.confirm_reset = true;
                ctx.notify();
            }
            MobileAction::CancelReset => {
                self.mobile.confirm_reset = false;
                ctx.notify();
            }
            MobileAction::ResetKeys => self.reset_mobile_keys(ctx),
            MobileAction::DismissNotice => {
                self.mobile.e2ee_notice = None;
                self.mobile.notice = None;
                ctx.notify();
            }
        }
    }

    /// Copy text to the clipboard and flip the button to "Copied" briefly.
    /// `None` (nothing shown) just repaints.
    fn copy_mobile_text(&mut self, ctx: &mut ViewContext<Self>, text: Option<String>) {
        if let Some(text) = text {
            ctx.clipboard().write(ClipboardContent::plain_text(text));
            self.mobile.copied = true;
            // Flip the button back to "Copy" after a beat.
            ctx.spawn(
                warpui::r#async::Timer::after(std::time::Duration::from_secs(2)),
                |view, _, ctx| {
                    view.mobile.copied = false;
                    ctx.notify();
                },
            );
        }
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_tracks_liveness() {
        assert_eq!(pairing_status(&None, 1_000), PairingStatus::Idle);
        let live = Some(MobilePairing {
            code: "c".to_string(),
            deep_link: "d".to_string(),
            expires_at_epoch: 1_600,
            qr_asset_id: None,
            #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
            secret: None,
        });
        assert_eq!(pairing_status(&live, 1_000), PairingStatus::Live(600));
        assert_eq!(pairing_status(&live, 1_600), PairingStatus::Expired);
        assert_eq!(pairing_status(&live, 9_999), PairingStatus::Expired);
    }

    #[test]
    fn error_messages_stay_friendly() {
        assert_eq!(
            mobile_error_message(&ApiError::NotAuthenticated),
            "Sign in to pair a phone."
        );
        assert!(mobile_error_message(&ApiError::Network("down".to_string()))
            .contains("Check your connection"));
        assert!(mobile_error_message(&ApiError::Server {
            status: 410,
            body: "gone".to_string(),
        })
        .contains("expired or was used"));
        assert!(mobile_error_message(&ApiError::Server {
            status: 429,
            body: "slow down".to_string(),
        })
        .contains("Too many tries"));
        // Anything else falls back to the server's own message.
        assert_eq!(
            mobile_error_message(&ApiError::Server {
                status: 500,
                body: r#"{"error": "boom"}"#.to_string(),
            }),
            "Boom"
        );
    }

    #[test]
    fn presence_grades_recency() {
        use super::DevicePresence::*;
        let now = 1_800_000_000;
        let rfc = |epoch: i64| {
            chrono::DateTime::from_timestamp(epoch, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap()
        };
        assert_eq!(device_presence(Some(&rfc(now - 30)), now), Connected);
        assert_eq!(device_presence(Some(&rfc(now - 120)), now), Connected);
        assert_eq!(device_presence(Some(&rfc(now - 300)), now), Reconnecting);
        assert_eq!(device_presence(Some(&rfc(now - 900)), now), Reconnecting);
        assert_eq!(device_presence(Some(&rfc(now - 901)), now), Offline);
        // Missing, empty and garbage timestamps read as Offline.
        assert_eq!(device_presence(None, now), Offline);
        assert_eq!(device_presence(Some(""), now), Offline);
        assert_eq!(device_presence(Some("not-a-date"), now), Offline);
        // Clock skew (phone claims the future) still counts as Connected.
        assert_eq!(device_presence(Some(&rfc(now + 60)), now), Connected);
        // Plain epoch seconds degrade gracefully too.
        assert_eq!(
            device_presence(Some(&(now - 10).to_string()), now),
            Connected
        );
        assert_eq!(
            device_presence(Some(&((now - 10_000) * 1000).to_string()), now),
            Offline
        );
    }

    #[test]
    fn progress_bar_runs_full_to_empty() {
        assert!((pairing_progress_fraction(600) - 1.0).abs() < f32::EPSILON);
        assert!((pairing_progress_fraction(300) - 0.5).abs() < 0.001);
        assert!((pairing_progress_fraction(0) - 0.0).abs() < f32::EPSILON);
        assert!((pairing_progress_fraction(9999) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clock_moves_forward() {
        assert!(now_epoch_secs() > 1_700_000_000);
    }

    #[test]
    fn key_ids_show_eight_chars() {
        assert_eq!(short_key_id(&"A".repeat(43)), "AAAAAAAA");
        assert_eq!(short_key_id("abc"), "abc");
        assert_eq!(short_key_id(""), "");
    }

    #[test]
    fn v2_link_carries_every_param() {
        let link = pairing_v2_link(
            "K7MQ2XRB9TGH4VNC8WPD",
            "https://api.example.test",
            "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            &"A".repeat(43),
            &"B".repeat(43),
        );
        let parsed = url::Url::parse(&link).expect("deep link must parse");
        assert_eq!(parsed.scheme(), dragent_paths::url_scheme());
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(pairs.get("v").map(|s| s.as_ref()), Some("2"));
        assert_eq!(
            pairs.get("c").map(|s| s.as_ref()),
            Some("K7MQ2XRB9TGH4VNC8WPD")
        );
        assert_eq!(
            pairs.get("g").map(|s| s.as_ref()),
            Some("https://api.example.test")
        );
        assert_eq!(
            pairs.get("d").map(|s| s.as_ref()),
            Some("3fa85f64-5717-4562-b3fc-2c963f66afa6")
        );
        assert_eq!(pairs.get("k").map(|s| s.len()), Some(43));
        assert_eq!(pairs.get("p").map(|s| s.len()), Some(43));
    }

    #[test]
    fn v2_link_encodes_gateway() {
        let link = pairing_v2_link(
            "CODE",
            "https://relay.example.com:443/x?y=z&a=b c",
            "desktop-id",
            "key",
            "secret",
        );
        assert!(
            !link.contains("://relay"),
            "gateway must be urlencoded: {link}"
        );
        let parsed = url::Url::parse(&link).expect("deep link must parse");
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(
            pairs.get("g").map(|s| s.as_ref()),
            Some("https://relay.example.com:443/x?y=z&a=b c")
        );
    }

    #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
    #[test]
    fn pairing_secret_is_random_b64_and_redacted() {
        let secret = PairingSecret::generate().expect("randomness available");
        let encoded = secret.b64();
        // 32 bytes → 43 base64url characters, no padding.
        assert_eq!(encoded.len(), 43);
        assert!(!encoded.contains('='));
        assert!(encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // Two secrets differ.
        let other = PairingSecret::generate().expect("randomness available");
        assert_ne!(secret.b64(), other.b64());
        // Nothing leaks through Debug — neither the secret nor the pairing.
        assert_eq!(format!("{secret:?}"), "PairingSecret(Redacted)");
        let pairing = MobilePairing {
            code: "c".to_string(),
            deep_link: "d".to_string(),
            expires_at_epoch: 0,
            qr_asset_id: None,
            secret: Some(secret),
        };
        assert!(pairing.is_v2());
        let printed = format!("{pairing:?}");
        assert!(
            !printed.contains(&encoded),
            "the secret must not leak through Debug: {printed}"
        );
        assert!(printed.contains("Redacted"));
    }

    #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
    #[test]
    fn v1_pairings_are_not_v2() {
        assert!(!MobilePairing::default().is_v2());
    }
}
