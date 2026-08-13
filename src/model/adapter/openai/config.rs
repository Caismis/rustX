//! RustX-owned configuration for the `OpenAI` adapters.
//!
//! No SDK configuration type appears in the public constructor API. The
//! adapter converts this configuration into an SDK client internally.
//!
//! # The endpoint is mandatory
//!
//! There is deliberately **no** default `https://api.openai.com/v1`. A
//! provider binding is constructible only with an explicit base URL, which
//! always originates from the validated model catalog
//! ([`crate::model::catalog`]). No provider *name* can therefore select an
//! official network endpoint.
//!
//! # Storage mode is not adapter configuration
//!
//! Responses storage/continuation mode is a per-model structural translation
//! behaviour and lives in
//! [`ModelCompat`](crate::model::catalog::ModelCompat), carried by each
//! request's invocation configuration. One adapter therefore serves every
//! model of its provider.

use reqwest::Client as ReqwestClient;

/// Configuration for both `OpenAI` adapters (Chat Completions and Responses).
#[derive(Clone)]
pub struct OpenAiAdapterConfig {
    api_key: String,
    api_base: String,
    /// Optional injected HTTP client, used by tests to talk to a local
    /// fixture server.
    http_client: Option<ReqwestClient>,
}

impl OpenAiAdapterConfig {
    /// Creates a configuration from an explicit credential and endpoint.
    ///
    /// Both arguments are mandatory: there is no implicit official endpoint
    /// and no credential discovery.
    #[must_use]
    pub fn new(api_key: impl Into<String>, api_base: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base: api_base.into(),
            http_client: None,
        }
    }

    /// Injects an HTTP client instead of building a default one.
    ///
    /// This is the narrow test seam used by deterministic tests that point
    /// the adapter at a local fixture server. It changes transport only: the
    /// endpoint and credential remain explicit, so it is not a second
    /// production configuration mode.
    #[must_use]
    pub fn with_http_client(mut self, client: ReqwestClient) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Consumes the configuration into its parts.
    pub(crate) fn into_parts(self) -> (String, String, Option<ReqwestClient>) {
        (self.api_key, self.api_base, self.http_client)
    }
}

impl std::fmt::Debug for OpenAiAdapterConfig {
    /// Redacted: the API key never appears in debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiAdapterConfig")
            .field("api_key", &"<redacted>")
            .field("api_base", &self.api_base)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "<injected>"),
            )
            .finish()
    }
}
