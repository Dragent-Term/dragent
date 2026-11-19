/// Pre-registered OAuth client credentials for MCP servers that do not support
/// open Dynamic Client Registration (RFC 7591).
///
/// These are obtained by registering Dragent as an OAuth app with each provider.
/// Keyed by the **OAuth issuer URL** (the `authorization_servers[0]` value from
/// the provider's `/.well-known/oauth-protected-resource` document).
///
/// Add entries here as we register with additional providers.

pub struct StaticOAuthClient {
    /// OAuth authorization server issuer URL (origin only, no trailing slash).
    pub issuer: &'static str,
    /// Pre-registered client ID issued by the provider.
    pub client_id: &'static str,
    /// Client secret, if the app was registered as a confidential client.
    /// Public PKCE clients leave this `None`.
    pub client_secret: Option<&'static str>,
}

/// Pre-registered clients. Populated as Dragent is approved by each provider.
///
/// Figma: apply at https://www.figma.com/developers to join the MCP partner program.
/// GitHub: apply at https://github.com/settings/applications/new for an OAuth App.
pub static STATIC_CLIENTS: &[StaticOAuthClient] = &[
    // ── Figma ─────────────────────────────────────────────────────────────────
    // Uncomment and fill in once approved by Figma's MCP partner program:
    // StaticOAuthClient {
    //     issuer: "https://www.figma.com",
    //     client_id: "YOUR_FIGMA_CLIENT_ID",
    //     client_secret: None,
    // },
];

/// Look up pre-registered credentials by OAuth issuer URL.
/// The issuer is compared after stripping any trailing slash.
pub fn find_by_issuer(issuer: &str) -> Option<&'static StaticOAuthClient> {
    let issuer = issuer.trim_end_matches('/');
    STATIC_CLIENTS
        .iter()
        .find(|c| c.issuer.trim_end_matches('/') == issuer)
}
