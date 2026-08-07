//! RustX-owned configuration for the `OpenAI` adapters.
//!
//! No SDK configuration type appears in the public constructor API. The
//! adapter converts this configuration into an SDK client internally.

use reqwest::Client as ReqwestClient;

/// How the `OpenAI` Responses adapter operates with provider storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsesStorageMode {
    /// Provider-side storage enabled (`store: true`); continuation by
    /// `previous_response_id`.
    Stored,
    /// Zero provider retention (`store: false`); continuation by preserved
    /// output items.
    Stateless,
}

/// Configuration for both `OpenAI` adapters (Chat Completions and Responses).
#[derive(Clone)]
pub struct OpenAiAdapterConfig {
    api_key: String,
    api_base: String,
    responses_storage: ResponsesStorageMode,
    /// Optional injected HTTP client, used by tests to talk to a local
    /// fixture server.
    http_client: Option<ReqwestClient>,
}

impl OpenAiAdapterConfig {
    /// Creates a configuration for the default `OpenAI` API base with the
    /// given API key. Responses storage defaults to [`ResponsesStorageMode::Stored`].
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base: "https://api.openai.com/v1".to_owned(),
            responses_storage: ResponsesStorageMode::Stored,
            http_client: None,
        }
    }

    /// Overrides the provider API base URL.
    #[must_use]
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    /// Sets the Responses storage mode.
    #[must_use]
    pub fn with_responses_storage(mut self, mode: ResponsesStorageMode) -> Self {
        self.responses_storage = mode;
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
    pub(crate) fn into_parts(
        self,
    ) -> (String, String, ResponsesStorageMode, Option<ReqwestClient>) {
        (
            self.api_key,
            self.api_base,
            self.responses_storage,
            self.http_client,
        )
    }
}

impl std::fmt::Debug for OpenAiAdapterConfig {
    /// Redacted: the API key never appears in debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiAdapterConfig")
            .field("api_key", &"<redacted>")
            .field("api_base", &self.api_base)
            .field("responses_storage", &self.responses_storage)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "<injected>"),
            )
            .finish()
    }
}
