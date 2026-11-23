mod backend;
mod client;
pub mod local_mode;
pub use local_mode::{is_local_mode, NEEDS_ACCOUNT, NEEDS_ACCOUNT_DETAIL};
pub use backend::{backend_url, compiled_service_address, web_origin};
pub use client::{
    companion_pairing_deep_link, format_pairing_countdown, multipart_body, pairing_remaining_secs,
    session_expired_rx, ApiClient, ApiError, CompanionDevice, CompanionPairing, MultipartPart,
    RestoredSession, SessionUser, API_CLIENT,
};
