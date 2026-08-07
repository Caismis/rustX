//! RustX-owned configuration for the Anthropic Messages adapter.

use reqwest::Client as ReqwestClient;

/// Configuration for the Anthropic Messages adapter.
#[derive(Clone)]
pub struct AnthropicAdapterConfig {
    api_key: String,
    api_base: String,
    anthropic_version: String,
    /// Optional injected HTTP client, used by tests to talk to a local
    /// fixture server.
    http_client: Option<ReqwestClient>,
}

impl AnthropicAdapterConfig {
    /// Creates a configuration for the default Anthropic API with the given
    /// API key.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base: "https://api.anthropic.com".to_owned(),
            anthropic_version: "2023-06-01".to_owned(),
            http_client: None,
        }
    }

    /// Overrides the provider API base URL.
    #[must_use]
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    /// Overrides the `anthropic-version` request header.
    #[must_use]
    pub fn with_anthropic_version(mut self, version: impl Into<String>) -> Self {
        self.anthropic_version = version.into();
        self
    }

    /// Injects an HTTP client instead of building a default one.
    ///
    /// This is the test seam used by deterministic tests that point the
    /// adapter at a local fixture server; production callers do not need it.
    #[must_use]
    pub fn with_http_client(mut self, client: ReqwestClient) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Consumes the configuration into its parts.
    pub(crate) fn into_parts(self) -> (String, String, String, Option<ReqwestClient>) {
        (
            self.api_key,
            self.api_base,
            self.anthropic_version,
            self.http_client,
        )
    }
}

impl std::fmt::Debug for AnthropicAdapterConfig {
    /// Redacted: the API key never appears in debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicAdapterConfig")
            .field("api_key", &"<redacted>")
            .field("api_base", &self.api_base)
            .field("anthropic_version", &self.anthropic_version)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "<injected>"),
            )
            .finish()
    }
}
