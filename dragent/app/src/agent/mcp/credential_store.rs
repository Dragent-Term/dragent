/// Minimal credential store stub for MCP OAuth tokens.
/// Full OAuth is a follow-up; for now this is a placeholder.
pub struct KeyringCredentialStore {
    pub service_name: String,
}

impl KeyringCredentialStore {
    pub fn new(server_name: &str) -> Self {
        Self {
            service_name: format!("dragent-mcp-{server_name}"),
        }
    }

    /// Store a token (stub — logs only, does not persist).
    pub fn store_token(&self, token: &str) {
        log::debug!(
            "[mcp/credential_store] would store token for '{}' (stub)",
            self.service_name
        );
        let _ = token;
    }

    /// Retrieve a token (stub — always returns None).
    pub fn get_token(&self) -> Option<String> {
        log::debug!(
            "[mcp/credential_store] no token stored for '{}' (stub)",
            self.service_name
        );
        None
    }
}
