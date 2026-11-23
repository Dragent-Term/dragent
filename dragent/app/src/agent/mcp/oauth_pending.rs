/// Pending MCP OAuth PKCE sessions waiting for the deep-link callback.
///
/// When the user clicks "Connect" for an SSE MCP server we open the browser
/// and store the PKCE verifier + metadata here. When the OS delivers this
/// edition's deep-link callback
/// (`<scheme>://anysphere.dragent-mcp/oauth/callback?code=…&state=…`, where
/// `<scheme>` is `dragent_paths::url_scheme()`) we look up the session by
/// state, complete the token exchange, and reconnect.
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;

/// This build's custom-scheme redirect URI registered with OAuth servers.
pub fn mcp_redirect_uri() -> String {
    format!(
        "{}://{MCP_OAUTH_CALLBACK_HOST}{MCP_OAUTH_CALLBACK_PATH}",
        dragent_paths::url_scheme()
    )
}

pub const MCP_OAUTH_CALLBACK_HOST: &str = "anysphere.dragent-mcp";
pub const MCP_OAUTH_CALLBACK_PATH: &str = "/oauth/callback";

pub struct McpPkceSession {
    pub server_name: String,
    pub verifier: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub token_endpoint: String,
}

static SESSIONS: Lazy<Mutex<HashMap<String, McpPkceSession>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn store(state: String, session: McpPkceSession) {
    SESSIONS.lock().insert(state, session);
}

pub fn take(state: &str) -> Option<McpPkceSession> {
    SESSIONS.lock().remove(state)
}
