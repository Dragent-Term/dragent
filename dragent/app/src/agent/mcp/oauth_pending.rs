/// Pending MCP OAuth PKCE sessions waiting for the deep-link callback.
///
/// When the user clicks "Connect" for an SSE MCP server we open the browser
/// and store the PKCE verifier + metadata here. When the OS delivers the
/// `dragent://anysphere.dragent-mcp/oauth/callback?code=…&state=…` URL we
/// look up the session by state, complete the token exchange, and reconnect.
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;

/// The custom-scheme redirect URI registered with OAuth servers.
/// Custom-scheme OAuth callback for MCP servers.
pub const MCP_REDIRECT_URI: &str = "dragent://anysphere.dragent-mcp/oauth/callback";

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
