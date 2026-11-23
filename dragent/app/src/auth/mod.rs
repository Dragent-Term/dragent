//! Cross-platform sign-in: polling and paste work everywhere; deep links are optional.
//!
//! | Platform | Token storage        | Deep link registration              |
//! |----------|----------------------|-------------------------------------|
//! | macOS    | Keychain             | `Dragent-Info.plist` URL types      |
//! | Windows  | Credential Manager   | Registry at startup (`deep_link`)   |
//! | Linux    | Secret Service       | `.desktop` `x-scheme-handler`       |

mod browser;
mod callback;
mod clipboard_text;
mod code;
mod completion;
mod deep_link;
mod manager;
mod pkce;
mod poll;
mod session_store;
mod token_exchange;
pub(crate) use token_exchange::exchange as exchange_token;
mod types;

pub use browser::browser_sign_in_url;
pub use callback::{is_auth_callback_url, parse_auth_callback, AuthCallback};
pub use clipboard_text::extract_auth_paste;
pub use code::sign_in_code;
pub use completion::{parse_auth_completion, AuthCompletion};
pub use deep_link::{
    custom_scheme, get_deep_link_receiver, register_scheme_handler, set_deep_link_receiver,
};
pub use manager::{handle_open_url, handle_pasted_auth, AuthManager};
pub use poll::{auth_polling_enabled, poll_session_url};
pub use session_store::{
    clear_refresh_token, has_saved_session, load_refresh_token, refresh_token_source,
    save_refresh_token,
};
pub use types::{AuthManagerEvent, AuthStatus, AuthUser};
