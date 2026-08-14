//! Deterministic model-catalog fixtures.
//!
//! This module exists so a suite can build a *real* session model authority —
//! a validated catalog, resolved credentials, real adapter bindings, and a
//! resolved [`SessionModelState`] — without a network and without a second
//! production configuration mode. Everything here goes through the ordinary
//! public catalog/invocation path; nothing bypasses validation. The single
//! substitution is the adapter behind an already-validated binding, through
//! the `cfg(test)`-only
//! [`ScriptedProviderAdapterFactory`](crate::model::invocation::ScriptedProviderAdapterFactory)
//! seam.
//!
//! It is *fixture construction only*: it introduces no runtime behaviour, no
//! alternative validation path, and no production configuration mode. It is
//! compiled only into this crate's test build, so it is not part of the
//! published API and production composition can never reference it.

use std::sync::Arc;

use crate::model::adapter::ModelAdapter;
use crate::model::catalog::{
    MapCredentialEnvironment, ModelCatalog, ModelCatalogDocument, ModelId, ModelRef, ProviderId,
    ResolvedProvider,
};
use crate::model::invocation::{
    ModelBindingRegistry, ModelInvocationError, RequestParams, ScriptedProviderAdapterFactory,
};
use crate::model::session::{SessionModelConfig, SessionModelState};
use crate::model::types::ModelProtocol;

/// An adapter factory that returns one scripted adapter for every provider
/// and protocol.
///
/// Used by tests that drive the agent loop deterministically without a
/// provider: the *binding* is still resolved through the catalog, so
/// endpoint and credential validation are exercised exactly as in
/// production.
pub struct ScriptedAdapterFactory {
    adapter: Arc<dyn ModelAdapter>,
}

impl ScriptedAdapterFactory {
    /// Creates a factory over one scripted adapter.
    #[must_use]
    pub const fn new(adapter: Arc<dyn ModelAdapter>) -> Self {
        Self { adapter }
    }
}

impl ScriptedProviderAdapterFactory for ScriptedAdapterFactory {
    fn adapter(
        &self,
        _provider: &ResolvedProvider,
        _protocol: ModelProtocol,
    ) -> Result<Arc<dyn ModelAdapter>, ModelInvocationError> {
        Ok(Arc::clone(&self.adapter))
    }
}

/// An adapter factory that resolves each provider/protocol pair through an
/// explicit lookup closure.
pub struct MappedAdapterFactory<F> {
    lookup: F,
}

impl<F> MappedAdapterFactory<F>
where
    F: Fn(&ProviderId, ModelProtocol) -> Option<Arc<dyn ModelAdapter>> + Send + Sync,
{
    /// Creates a factory over an explicit lookup.
    pub const fn new(lookup: F) -> Self {
        Self { lookup }
    }
}

impl<F> ScriptedProviderAdapterFactory for MappedAdapterFactory<F>
where
    F: Fn(&ProviderId, ModelProtocol) -> Option<Arc<dyn ModelAdapter>> + Send + Sync,
{
    fn adapter(
        &self,
        provider: &ResolvedProvider,
        protocol: ModelProtocol,
    ) -> Result<Arc<dyn ModelAdapter>, ModelInvocationError> {
        (self.lookup)(provider.id(), protocol).ok_or_else(|| ModelInvocationError::MissingAdapter {
            model: ModelRef::new(provider.id().clone(), ModelId::new("unmapped")),
            protocol,
        })
    }
}

/// An adapter that is never invoked.
///
/// Used by tests that need a *resolved* model binding to exist without ever
/// performing a model turn (for example projection read-model unit tests).
#[derive(Debug, Clone, Copy)]
pub struct NullAdapter;

impl ModelAdapter for NullAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(
        &self,
        _request: crate::model::types::ModelRequest,
        _cancellation: crate::runtime::cancellation::CancellationSignal,
    ) -> crate::model::adapter::ModelEventStream {
        unreachable!("the null fixture adapter is never invoked")
    }
}

/// One model entry of a fixture catalog.
#[derive(Debug, Clone)]
pub struct FixtureModel {
    /// The `provider-id/model-id` reference this model is published under.
    pub reference: String,
    /// The protocol.
    pub protocol: ModelProtocol,
    /// The context window in tokens.
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The model-level default request parameters, as a JSON object.
    pub request_params: serde_json::Value,
    /// The reasoning block, as a JSON object, when the model declares one.
    pub reasoning: Option<serde_json::Value>,
    /// The compat block, as a JSON object.
    pub compat: serde_json::Value,
    /// Whether the model claims tool-call support.
    pub tool_calls: bool,
    /// Whether the model claims reasoning support.
    pub reasoning_capable: bool,
    /// Extra claimed input modalities beyond text.
    pub extra_input_modalities: Vec<&'static str>,
}

impl FixtureModel {
    /// A plain text-only tool-calling model with a large window.
    #[must_use]
    pub fn text(reference: &str, protocol: ModelProtocol) -> Self {
        Self {
            reference: reference.to_owned(),
            protocol,
            context_window: 1_000_000,
            max_output_tokens: 4096,
            request_params: serde_json::json!({}),
            reasoning: None,
            compat: serde_json::json!({}),
            tool_calls: true,
            reasoning_capable: false,
            extra_input_modalities: Vec::new(),
        }
    }

    /// Sets the context window.
    #[must_use]
    pub const fn with_context_window(mut self, tokens: u64) -> Self {
        self.context_window = tokens;
        self
    }

    /// Sets the configured maximum output tokens.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = tokens;
        self
    }

    /// Sets the model-level default request parameters.
    #[must_use]
    pub fn with_request_params(mut self, params: serde_json::Value) -> Self {
        self.request_params = params;
        self
    }

    /// Declares a reasoning block and marks the model reasoning-capable.
    #[must_use]
    pub fn with_reasoning(mut self, reasoning: serde_json::Value) -> Self {
        self.reasoning = Some(reasoning);
        self.reasoning_capable = true;
        self
    }

    /// Claims provider-default reasoning that is always enabled without
    /// exposing selectable reasoning profiles.
    #[must_use]
    pub const fn always_on_reasoning(mut self) -> Self {
        self.reasoning_capable = true;
        self
    }

    /// Sets the compat block.
    #[must_use]
    pub fn with_compat(mut self, compat: serde_json::Value) -> Self {
        self.compat = compat;
        self
    }

    /// Sets whether the model claims tool-call support.
    #[must_use]
    pub const fn with_tool_calls(mut self, tool_calls: bool) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Claims an additional input modality (used to prove the effective
    /// capability intersection actually narrows a raw catalog claim).
    #[must_use]
    pub fn claiming_input(mut self, modality: &'static str) -> Self {
        self.extra_input_modalities.push(modality);
        self
    }

    fn parts(&self) -> (String, serde_json::Value) {
        let (provider, model) = self
            .reference
            .split_once('/')
            .expect("a fixture model reference is provider/model");
        let mut inputs = vec![serde_json::json!("text")];
        for modality in &self.extra_input_modalities {
            inputs.push(serde_json::json!(modality));
        }
        let mut document = serde_json::json!({
            "id": model,
            "protocol": self.protocol,
            "contextWindow": self.context_window,
            "maxOutputTokens": self.max_output_tokens,
            "capabilities": {
                "inputModalities": inputs,
                "outputModalities": ["text"],
                "toolCalls": self.tool_calls,
                "reasoning": self.reasoning_capable,
            },
            "requestParams": self.request_params,
            "compat": self.compat,
        });
        if let Some(reasoning) = &self.reasoning {
            document["reasoning"] = reasoning.clone();
        }
        (provider.to_owned(), document)
    }
}

/// Builds a validated catalog document from fixture models.
///
/// Every provider gets an explicit local base URL and a literal credential:
/// there is no implicit endpoint anywhere, exactly as in production.
///
/// # Panics
///
/// Panics when the fixture models do not form a well-formed document, which
/// always means the fixture itself is wrong.
#[must_use]
pub fn fixture_catalog_document(models: &[FixtureModel]) -> ModelCatalogDocument {
    let mut providers = serde_json::Map::new();
    for model in models {
        let (provider, document) = model.parts();
        let entry = providers.entry(provider.clone()).or_insert_with(|| {
            serde_json::json!({
                "baseUrl": format!("https://{provider}.fixture.invalid/v1"),
                "apiKey": format!("fixture-key-{provider}"),
                "models": [],
            })
        });
        entry["models"]
            .as_array_mut()
            .expect("models array")
            .push(document);
    }
    serde_json::from_value(serde_json::json!({"providers": providers}))
        .expect("the fixture catalog document is well formed")
}

/// Builds a validated catalog from fixture models.
///
/// # Panics
///
/// Panics when the fixture models do not form a valid catalog, which always
/// means the fixture itself is wrong.
#[must_use]
pub fn fixture_catalog(models: &[FixtureModel]) -> ModelCatalog {
    ModelCatalog::from_document(fixture_catalog_document(models))
        .expect("the fixture catalog validates")
}

/// Builds a binding registry over fixture models and one adapter factory.
///
/// # Panics
///
/// Panics when the fixture catalog or its bindings fail to resolve.
#[must_use]
pub fn fixture_registry(
    models: &[FixtureModel],
    factory: &dyn ScriptedProviderAdapterFactory,
) -> ModelBindingRegistry {
    let resolved = fixture_catalog(models)
        .resolve(&MapCredentialEnvironment::default())
        .expect("literal fixture credentials resolve");
    ModelBindingRegistry::new_with_scripted_adapters(resolved, factory)
        .expect("fixture bindings resolve")
}

/// Builds a session model state selecting one fixture model.
///
/// # Panics
///
/// Panics when the selection does not resolve.
#[must_use]
pub fn fixture_session_model(
    models: &[FixtureModel],
    selected: &str,
    factory: &dyn ScriptedProviderAdapterFactory,
) -> SessionModelState {
    let registry = fixture_registry(models, factory);
    SessionModelState::new(
        registry,
        SessionModelConfig::of(ModelRef::parse(selected).expect("valid fixture reference")),
    )
    .expect("the fixture session model resolves")
}

/// The single scripted-model session state used by most runtime tests: one
/// Chat Completions model with a very large window and no reasoning block.
///
/// # Panics
///
/// Panics when the fixture does not resolve.
#[must_use]
pub fn scripted_session_model(adapter: Arc<dyn ModelAdapter>) -> SessionModelState {
    fixture_session_model(
        &[
            FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions)
                .with_max_output_tokens(512),
        ],
        "scripted/scripted",
        &ScriptedAdapterFactory::new(adapter),
    )
}

/// A `requestParams` object literal.
///
/// # Panics
///
/// Panics when the value is not a JSON object.
#[must_use]
pub fn request_params(value: serde_json::Value) -> RequestParams {
    match value {
        serde_json::Value::Object(map) => map,
        other => panic!("requestParams must be a JSON object, got {other}"),
    }
}
