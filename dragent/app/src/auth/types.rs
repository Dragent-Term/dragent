//! Auth session types.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthUser {
    pub user_id: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl AuthUser {
    /// The local-mode identity (a Dev build with no server). It has no user
    /// id: nothing it signs in to exists on any server.
    pub fn local_demo() -> Self {
        Self {
            user_id: None,
            first_name: Some(crate::api::local_mode::DEMO_FIRST_NAME.to_string()),
            last_name: Some(crate::api::local_mode::DEMO_LAST_NAME.to_string()),
            avatar_url: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    LoggedOut,
    /// Stored refresh token found; silently validating in background.
    Restoring,
    SigningIn,
    LoggedIn(AuthUser),
    NeedsReauth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthManagerEvent {
    AuthComplete,
    /// Silent session restore on startup completed successfully.
    SessionRestored,
    AuthFailed {
        message: String,
    },
    /// The user signed out deliberately.
    LoggedOut,
    /// The session died on its own — refresh token expired or revoked. The user
    /// is now signed out and needs to be told why (they did not ask for this).
    SessionExpired,
}
