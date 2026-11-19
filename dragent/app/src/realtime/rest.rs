//! REST calls for shared-session invites.
//!
//! A host creates role-scoped invite links: `create_share_link` for a URL to
//! copy/QR, `invite_by_email` to also email it. Each returns a link whose URL
//! carries the capability secret; joining happens over the WebSocket with
//! `{session, secret}`. They go through the shared [`API_CLIENT`], so Bearer auth
//! and 401 rotation are automatic — but the client is blocking, so call these off
//! the UI thread (e.g. from `ctx.spawn`/a worker thread).

use serde::Deserialize;
use serde_json::json;

use crate::api::{ApiError, API_CLIENT};

/// Access a link grants. Mirrors the server's `view_only`/`control`.
pub const ACCESS_VIEW: &str = "view_only";
pub const ACCESS_EDIT: &str = "control";

/// A freshly created invite link.
#[derive(Debug, Clone, Deserialize)]
pub struct ShareLink {
    pub token: String,
    pub join_url: String,
    pub access: String,
    /// Only set by `invite_by_email`: whether the email actually dispatched.
    #[serde(default)]
    pub emailed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DevicePortForward {
    pub id: String,
    pub port: u16,
    pub label: String,
    pub public_url: String,
}

fn parse<T: for<'de> Deserialize<'de>>(v: serde_json::Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|e| ApiError::Parse(e.to_string()))
}

/// Host: create a revocable, role-scoped invite link for a session.
pub fn create_share_link(session_id: &str, access: &str) -> Result<ShareLink, ApiError> {
    let body = json!({ "session_id": session_id, "access": access });
    let resp = API_CLIENT.post("/v1/sessions/share", &body, 15)?;
    parse(resp)
}

/// Host: create a role-scoped invite link and email it to `email`. The link is
/// returned regardless of whether the email dispatched (see `ShareLink::emailed`).
pub fn invite_by_email(session_id: &str, email: &str, access: &str) -> Result<ShareLink, ApiError> {
    let body = json!({ "session_id": session_id, "email": email, "access": access });
    let resp = API_CLIENT.post("/v1/sessions/invite", &body, 15)?;
    parse(resp)
}

/// Host: revoke an invite link by its token.
pub fn revoke_share_link(token: &str) -> Result<(), ApiError> {
    API_CLIENT.post("/v1/sessions/share/revoke", &json!({ "token": token }), 15)?;
    Ok(())
}

pub fn create_port_forward(
    device_id: &str,
    port: u16,
    label: &str,
) -> Result<DevicePortForward, ApiError> {
    let body = json!({ "device_id": device_id, "port": port, "label": label });
    parse(API_CLIENT.post("/v1/forwards", &body, 15)?)
}

pub fn list_port_forwards(device_id: &str) -> Result<Vec<DevicePortForward>, ApiError> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("device_id", device_id)
        .finish();
    parse(API_CLIENT.get(&format!("/v1/forwards?{query}"), 15)?)
}

pub fn close_port_forward(device_id: &str, forward_id: &str) -> Result<(), ApiError> {
    API_CLIENT.post(
        "/v1/forwards/close",
        &json!({ "device_id": device_id, "forward_id": forward_id }),
        15,
    )?;
    Ok(())
}
