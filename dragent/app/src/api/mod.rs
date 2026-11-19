mod backend;
mod client;
pub use backend::{backend_url, web_origin};
pub use client::{
    companion_pairing_deep_link, format_pairing_countdown, multipart_body, pairing_remaining_secs,
    session_expired_rx, ApiClient, ApiError, CompanionDevice, CompanionPairing, MultipartPart,
    RestoredSession, SessionUser, API_CLIENT,
};
