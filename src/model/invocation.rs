//! Opaque provider request parameters, runtime-protected wire keys,
//! effective capabilities, and the immutable resolved model invocation
//! (Issue #42).
//!
//! # Opaque request parameters
//!
//! `requestParams` is an opaque JSON object. rustX deliberately does **not**
//! normalize provider sampling/routing parameters (`temperature`, `top_k`,
//! `min_p`, `repetition_penalty`, `thinking`, `reasoning`,
//! `chat_template_kwargs`, routing objects, future provider extensions) into
//! a universal Rust struct: recognizing a new provider parameter must never
//! require a rustX release.
//!
//! Effective parameters are resolved in exactly this order, and every step
//! is a **top-level shallow overlay** — a nested object or an array is
//! replaced atomically, never deep-merged:
//!
//! ```text
//! model defaults
//!   overlay selected reasoning profile
//!   overlay session overrides
//! ```
//!
//! with the extra rule that the selected reasoning profile **owns** every
//! top-level key it declares: a session override that also declares one of
//! those keys is a deterministic configuration failure, never resolved by
//! merge order.
//!
//! # Final wire placement
//!
//! ```text
//! canonical ModelRequest
//!   -> adapter canonical -> protocol translation
//!   -> final provider request JSON object
//!   -> validate protected-key ownership
//!   -> shallow-overlay effective requestParams
//!   -> HTTP / SDK BYOT
//! ```
//!
//! The effective keys land at the **top level** of the real provider request
//! body. There is no invented `extra_body` nesting level.
//!
//! # Protected wire keys
//!
//! Opaque parameters must never replace runtime semantic authority: target
//! model identity, canonical messages/input/instructions, tool definitions,
//! streaming mode and required stream options, provider continuation
//! identity/state, and the runtime-resolved output budget are owned by the
//! runtime. Each protocol declares its exact protected set here; a collision
//! is an explicit failure at configuration time *and* again at final request
//! construction, so an invalid internal state can never silently overwrite a
//! runtime field.
//!
//! Provider-owned reasoning/sampling fields are deliberately *not* protected
//! merely because rustX recognizes their names: a reasoning profile is
//! expected to own fields such as `thinking`, `reasoning`, or
//! `output_config`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::model::adapter::ModelAdapter;
#[cfg(test)]
use crate::model::catalog::ResolvedProvider;
use crate::model::catalog::{
    CatalogModelView, Modality, ModelCapabilities, ModelCatalogView, ModelCompat, ModelDefinition,
    ModelRef, ProviderId, ReasoningProfileId, ReasoningProfileView, ResolvedModelCatalog,
};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::types::ModelProtocol;

/// An opaque provider request-parameter object.
///
/// Values are preserved exactly as parsed; rustX never rewrites, coerces, or
/// interprets them.
pub type RequestParams = serde_json::Map<String, serde_json::Value>;

/// Which configuration layer a set of request parameters came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestParamsLayer {
    /// The catalog model's default parameters.
    ModelDefaults,
    /// A declared reasoning profile's parameters.
    ReasoningProfile,
    /// The session's request-parameter overrides.
    SessionOverrides,
    /// An explicit summary model's request-parameter overrides.
    SummaryOverrides,
    /// The effective parameters at final wire construction.
    EffectiveRequest,
}

impl fmt::Display for RequestParamsLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ModelDefaults => "model default requestParams",
            Self::ReasoningProfile => "reasoning profile requestParams",
            Self::SessionOverrides => "session requestParams overrides",
            Self::SummaryOverrides => "explicit summary requestParams overrides",
            Self::EffectiveRequest => "effective requestParams",
        })
    }
}

/// One configured collision with a runtime-owned protected wire key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedKeyCollision {
    /// The protocol whose protected set was violated.
    pub protocol: ModelProtocol,
    /// The colliding top-level key.
    pub key: String,
    /// The configuration layer that declared it.
    pub layer: RequestParamsLayer,
}

impl fmt::Display for ProtectedKeyCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} declares runtime-owned protected wire key {:?} of protocol {}",
            self.layer,
            self.key,
            protocol_name(self.protocol)
        )
    }
}

impl std::error::Error for ProtectedKeyCollision {}

/// The runtime-owned protected wire keys of one protocol.
///
/// These are derived from what the adapters actually construct:
///
/// - **Chat Completions**: `model`, `messages`, `tools`, `stream`,
///   `stream_options`, and *both* max-token spellings (`max_tokens` and
///   `max_completion_tokens`), so a second contradictory maximum can never
///   be injected next to the one the compat metadata selected.
/// - **Responses**: `model`, `input`, `instructions`, `tools`, `stream`,
///   `max_output_tokens`, plus the continuation-semantics trio `store`,
///   `previous_response_id`, and `include` (the encrypted-reasoning
///   `include` value is what makes stateless replay possible).
/// - **Anthropic Messages**: `model`, `messages`, `system`, `tools`,
///   `stream`, `max_tokens`.
#[must_use]
pub const fn protected_keys(protocol: ModelProtocol) -> &'static [&'static str] {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => &[
            "max_completion_tokens",
            "max_tokens",
            "messages",
            "model",
            "stream",
            "stream_options",
            "tools",
        ],
        ModelProtocol::OpenAiResponses => &[
            "include",
            "input",
            "instructions",
            "max_output_tokens",
            "model",
            "previous_response_id",
            "store",
            "stream",
            "tools",
        ],
        ModelProtocol::AnthropicMessages => &[
            "max_tokens",
            "messages",
            "model",
            "stream",
            "system",
            "tools",
        ],
    }
}

/// Validates that one configuration layer declares no protected wire key.
///
/// # Errors
///
/// Returns the first [`ProtectedKeyCollision`] in deterministic key order.
pub fn validate_request_params_layer(
    params: &RequestParams,
    protocol: ModelProtocol,
    layer: RequestParamsLayer,
) -> Result<(), ProtectedKeyCollision> {
    let protected = protected_keys(protocol);
    for key in params.keys() {
        if protected.contains(&key.as_str()) {
            return Err(ProtectedKeyCollision {
                protocol,
                key: key.clone(),
                layer,
            });
        }
    }
    Ok(())
}

/// Applies one top-level shallow overlay.
///
/// A nested object or array value replaces the previous value atomically;
/// rustX never deep-merges configuration.
pub fn overlay_shallow(base: &mut RequestParams, overlay: &RequestParams) {
    for (key, value) in overlay {
        base.insert(key.clone(), value.clone());
    }
}

/// Overlays the effective request parameters onto a final provider request
/// body.
///
/// This is the last defence of the protected-key contract: it runs after the
/// adapter has placed every runtime-owned structural field, so an invalid
/// internal state cannot silently overwrite one.
///
/// # Errors
///
/// Returns [`ModelErrorKind::InvalidRequest`] when the translated request is
/// not a JSON object or when the effective parameters declare a protected
/// key.
pub fn finalize_provider_request(
    request: serde_json::Value,
    params: &RequestParams,
    protocol: ModelProtocol,
) -> Result<serde_json::Value, ModelError> {
    let serde_json::Value::Object(mut object) = request else {
        return Err(ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "a translated provider request must be a JSON object".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        });
    };
    validate_request_params_layer(params, protocol, RequestParamsLayer::EffectiveRequest).map_err(
        |collision| ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: collision.to_string(),
            retry_after_ms: None,
            provider_code: None,
        },
    )?;
    overlay_shallow(&mut object, params);
    Ok(serde_json::Value::Object(object))
}

/// The capabilities one protocol adapter can actually represent today.
///
/// All three adapters reject canonical image and file references because
/// artifact/media resolution does not exist yet, so no protocol advertises
/// image or file input regardless of what a catalog claims.
#[must_use]
pub fn adapter_capabilities(protocol: ModelProtocol) -> ModelCapabilities {
    match protocol {
        ModelProtocol::OpenAiChatCompletions
        | ModelProtocol::OpenAiResponses
        | ModelProtocol::AnthropicMessages => ModelCapabilities::text_only(true, true),
    }
}

/// The capabilities the current rustX runtime can carry end to end.
#[must_use]
pub fn runtime_capabilities() -> ModelCapabilities {
    ModelCapabilities::text_only(true, true)
}

/// Computes the effective client-visible capability of one model.
///
/// ```text
/// model-declared  ∩  adapter/protocol  ∩  current runtime  =  effective
/// ```
#[must_use]
pub fn effective_capabilities(
    declared: &ModelCapabilities,
    protocol: ModelProtocol,
) -> ModelCapabilities {
    declared
        .intersect(&adapter_capabilities(protocol))
        .intersect(&runtime_capabilities())
}

/// The provider-neutral invocation configuration one model request carries
/// to its adapter.
///
/// This is the only provider-configuration channel of a [`ModelRequest`]
/// ([`crate::model::types::ModelRequest`]): it is immutable for the whole
/// attempt, carries no credential and no adapter object, and never leaks
/// into canonical conversation facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvocationConfig {
    /// The provider-facing model identifier.
    pub model: String,
    /// The protocol the adapter must speak.
    pub protocol: ModelProtocol,
    /// The runtime-resolved effective maximum output tokens.
    ///
    /// The runtime resolves this before the adapter boundary: no adapter
    /// invents a generation limit, and providers that require an explicit
    /// maximum (Anthropic) are always satisfiable.
    pub max_output_tokens: u32,
    /// The effective opaque provider request parameters.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The effective capabilities of this invocation.
    pub capabilities: ModelCapabilities,
    /// The bounded structural translation metadata.
    #[serde(default)]
    pub compat: ModelCompat,
}

/// The immutable resolved model invocation of one attempt (or one explicit
/// summary model).
///
/// Everything an attempt needs to talk to a provider is frozen here: the
/// provider binding and its adapter, the model identity and protocol, the
/// context window, the output budget, the selected reasoning profile and its
/// semantic enabled state, the effective request parameters, and the
/// effective capabilities.
///
/// The type is deliberately **not** `Serialize`: it owns an adapter handle.
/// [`ResolvedModelInvocation::view`] produces the redacted client-facing
/// projection.
#[derive(Clone)]
pub struct ResolvedModelInvocation {
    provider: ProviderId,
    model_ref: ModelRef,
    adapter: Arc<dyn ModelAdapter>,
    protocol: ModelProtocol,
    context_window: u64,
    model_max_output_tokens: u32,
    effective_output_tokens: u32,
    reasoning_profile: Option<ReasoningProfileId>,
    reasoning_enabled: bool,
    request_params: RequestParams,
    capabilities: ModelCapabilities,
    declared_capabilities: ModelCapabilities,
    compat: ModelCompat,
}

impl fmt::Debug for ResolvedModelInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedModelInvocation")
            .field("model", &self.model_ref)
            .field("protocol", &self.protocol)
            .field("context_window", &self.context_window)
            .field("effective_output_tokens", &self.effective_output_tokens)
            .field("reasoning_profile", &self.reasoning_profile)
            .field("reasoning_enabled", &self.reasoning_enabled)
            .field("request_params", &self.request_params)
            .field("capabilities", &self.capabilities)
            .field("compat", &self.compat)
            .field("declared_capabilities", &self.declared_capabilities)
            .field("model_max_output_tokens", &self.model_max_output_tokens)
            .field("provider", &self.provider)
            .field("adapter", &"<provider adapter>")
            .finish()
    }
}

impl PartialEq for ResolvedModelInvocation {
    /// Two invocations are equal when their semantic configuration is equal
    /// and they share the same adapter binding.
    fn eq(&self, other: &Self) -> bool {
        self.model_ref == other.model_ref
            && self.protocol == other.protocol
            && self.context_window == other.context_window
            && self.effective_output_tokens == other.effective_output_tokens
            && self.reasoning_profile == other.reasoning_profile
            && self.reasoning_enabled == other.reasoning_enabled
            && self.request_params == other.request_params
            && self.capabilities == other.capabilities
            && self.compat == other.compat
            && Arc::ptr_eq(&self.adapter, &other.adapter)
    }
}

impl ResolvedModelInvocation {
    /// The provider identity of the binding.
    #[must_use]
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// The fully qualified model reference.
    #[must_use]
    pub const fn model_ref(&self) -> &ModelRef {
        &self.model_ref
    }

    /// The provider adapter of the binding.
    #[must_use]
    pub fn adapter(&self) -> &Arc<dyn ModelAdapter> {
        &self.adapter
    }

    /// The protocol of the binding.
    #[must_use]
    pub const fn protocol(&self) -> ModelProtocol {
        self.protocol
    }

    /// The model's context window in tokens.
    #[must_use]
    pub const fn context_window(&self) -> u64 {
        self.context_window
    }

    /// The effective output budget of this invocation.
    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        self.effective_output_tokens
    }

    /// The selected reasoning profile, when the model declares any.
    #[must_use]
    pub const fn reasoning_profile(&self) -> Option<&ReasoningProfileId> {
        self.reasoning_profile.as_ref()
    }

    /// Whether the selected profile semantically enables reasoning.
    #[must_use]
    pub const fn reasoning_enabled(&self) -> bool {
        self.reasoning_enabled
    }

    /// The effective opaque provider request parameters.
    #[must_use]
    pub const fn request_params(&self) -> &RequestParams {
        &self.request_params
    }

    /// The effective capabilities of this invocation.
    #[must_use]
    pub const fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    /// The bounded structural translation metadata.
    #[must_use]
    pub const fn compat(&self) -> ModelCompat {
        self.compat
    }

    /// The provider-neutral invocation configuration of one model request.
    #[must_use]
    pub fn invocation_config(&self) -> ModelInvocationConfig {
        ModelInvocationConfig {
            model: self.model_ref.model().as_str().to_owned(),
            protocol: self.protocol,
            max_output_tokens: self.effective_output_tokens,
            request_params: self.request_params.clone(),
            capabilities: self.capabilities.clone(),
            compat: self.compat,
        }
    }

    /// The same invocation with a stricter output budget.
    ///
    /// This is how the context plane expresses its summary/output safety cap:
    /// the cap flows through the runtime-owned protected max-output field and
    /// never mutates the reasoning profile or the request parameters.
    #[must_use]
    pub(crate) fn with_output_cap(&self, cap: u32) -> Self {
        let mut capped = self.clone();
        capped.effective_output_tokens = self.effective_output_tokens.min(cap);
        capped
    }

    /// The redacted client-facing projection of this invocation.
    #[must_use]
    pub fn view(&self) -> ModelInvocationView {
        ModelInvocationView {
            model: self.model_ref.clone(),
            protocol: self.protocol,
            context_window: self.context_window,
            model_max_output_tokens: self.model_max_output_tokens,
            max_output_tokens: self.effective_output_tokens,
            reasoning_profile: self.reasoning_profile.clone(),
            reasoning_enabled: self.reasoning_enabled,
            request_params: self.request_params.clone(),
            capabilities: self.capabilities.clone(),
            declared_capabilities: self.declared_capabilities.clone(),
        }
    }
}

/// The redacted client-facing projection of one resolved model invocation.
///
/// It carries no credential, no adapter object, no provider HTTP client, and
/// no synchronization identity. The effective request parameters *are*
/// exposed: they are provider-owned configuration a model-control client
/// needs, and they can never contain credential material because a
/// credential is never a request parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInvocationView {
    /// The fully qualified model reference.
    pub model: ModelRef,
    /// The protocol of the binding.
    pub protocol: ModelProtocol,
    /// The model's context window in tokens.
    pub context_window: u64,
    /// The model's configured maximum output tokens.
    pub model_max_output_tokens: u32,
    /// The effective output budget.
    pub max_output_tokens: u32,
    /// The selected reasoning profile, when the model declares any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// Whether reasoning is semantically enabled.
    pub reasoning_enabled: bool,
    /// The effective opaque provider request parameters.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The effective capabilities.
    pub capabilities: ModelCapabilities,
    /// The raw capabilities the catalog claims, for clients that want to
    /// explain why an effective capability is absent.
    pub declared_capabilities: ModelCapabilities,
}

/// One desired model selection: the mutable configuration a session owns and
/// a Runtime Client sets as a whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    /// The selected catalog model.
    pub model: ModelRef,
    /// The selected reasoning profile; the model default is used when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// The session's request-parameter overrides.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The session's output-budget override; the model's configured maximum
    /// is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl ModelSelection {
    /// A selection of one model with every default.
    #[must_use]
    pub fn of(model: ModelRef) -> Self {
        Self {
            model,
            reasoning_profile: None,
            request_params: RequestParams::new(),
            max_output_tokens: None,
        }
    }
}

/// A model-invocation resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInvocationError {
    /// The catalog could not resolve the reference or its credential.
    Catalog(crate::model::catalog::ModelCatalogError),
    /// The selected reasoning profile is not declared by the model.
    UnknownReasoningProfile {
        /// The model.
        model: ModelRef,
        /// The requested profile.
        profile: ReasoningProfileId,
    },
    /// A reasoning profile was selected for a model that declares none.
    ModelDeclaresNoReasoningProfiles {
        /// The model.
        model: ModelRef,
        /// The requested profile.
        profile: ReasoningProfileId,
    },
    /// The session override declares a key the selected reasoning profile
    /// owns.
    ReasoningProfileKeyOwnership {
        /// The model.
        model: ModelRef,
        /// The selected profile.
        profile: ReasoningProfileId,
        /// The contested key.
        key: String,
    },
    /// A configured request-parameter layer collides with a protected wire
    /// key.
    ProtectedKey(ProtectedKeyCollision),
    /// The requested output budget is impossible for the model.
    InvalidOutputBudget {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// The model's effective capabilities cannot carry a rustX conversation.
    UnusableCapabilities {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// No adapter binding exists for the model's provider and protocol.
    MissingAdapter {
        /// The model.
        model: ModelRef,
        /// The protocol.
        protocol: ModelProtocol,
    },
}

impl fmt::Display for ModelInvocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(error) => write!(f, "{error}"),
            Self::UnknownReasoningProfile { model, profile } => write!(
                f,
                "model {model} declares no reasoning profile {:?}",
                profile.as_str()
            ),
            Self::ModelDeclaresNoReasoningProfiles { model, profile } => write!(
                f,
                "model {model} declares no reasoning profiles; {:?} cannot be selected",
                profile.as_str()
            ),
            Self::ReasoningProfileKeyOwnership {
                model,
                profile,
                key,
            } => write!(
                f,
                "model {model}: reasoning profile {:?} owns request key {key:?}; \
                 a session override may not also declare it",
                profile.as_str()
            ),
            Self::ProtectedKey(collision) => write!(f, "{collision}"),
            Self::InvalidOutputBudget { model, detail } => {
                write!(f, "model {model} output budget is invalid: {detail}")
            }
            Self::UnusableCapabilities { model, detail } => {
                write!(
                    f,
                    "model {model} effective capabilities are unusable: {detail}"
                )
            }
            Self::MissingAdapter { model, protocol } => write!(
                f,
                "no adapter binding for model {model} protocol {}",
                protocol_name(*protocol)
            ),
        }
    }
}

impl std::error::Error for ModelInvocationError {}

impl From<crate::model::catalog::ModelCatalogError> for ModelInvocationError {
    fn from(error: crate::model::catalog::ModelCatalogError) -> Self {
        Self::Catalog(error)
    }
}

/// The runtime's model binding registry: the resolved catalog plus exactly
/// one adapter per provider/protocol pair it uses.
///
/// One registry exists per local runtime process. Resolving a selection is a
/// pure lookup, so admission never constructs an HTTP client.
#[derive(Clone)]
pub struct ModelBindingRegistry {
    catalog: ResolvedModelCatalog,
    adapters: BTreeMap<(ProviderId, ModelProtocol), Arc<dyn ModelAdapter>>,
}

impl fmt::Debug for ModelBindingRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelBindingRegistry")
            .field("catalog", &self.catalog)
            .field("adapters", &self.adapters.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ModelBindingRegistry {
    /// Builds every supported adapter binding directly from the resolved
    /// provider endpoint and credential.
    ///
    /// # Errors
    ///
    /// Returns the first adapter construction failure.
    pub fn new(catalog: ResolvedModelCatalog) -> Result<Self, ModelInvocationError> {
        let mut adapters: BTreeMap<(ProviderId, ModelProtocol), Arc<dyn ModelAdapter>> =
            BTreeMap::new();
        for reference in catalog.model_refs().collect::<Vec<_>>() {
            let (provider, model) = catalog.binding(&reference)?;
            let key = (provider.id().clone(), model.protocol);
            if let std::collections::btree_map::Entry::Vacant(slot) = adapters.entry(key) {
                let credential = provider.credential().expose();
                let base_url = provider.base_url();
                let adapter: Arc<dyn ModelAdapter> = match model.protocol {
                    ModelProtocol::OpenAiChatCompletions => {
                        use crate::model::adapter::openai::{
                            OpenAiAdapterConfig, OpenAiChatCompletionsAdapter,
                        };
                        Arc::new(OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
                            credential, base_url,
                        )))
                    }
                    ModelProtocol::OpenAiResponses => {
                        use crate::model::adapter::openai::{
                            OpenAiAdapterConfig, OpenAiResponsesAdapter,
                        };
                        Arc::new(OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(
                            credential, base_url,
                        )))
                    }
                    ModelProtocol::AnthropicMessages => {
                        use crate::model::adapter::anthropic::{
                            AnthropicAdapterConfig, AnthropicMessagesAdapter,
                        };
                        Arc::new(AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new(
                            credential, base_url,
                        )))
                    }
                };
                slot.insert(adapter);
            }
        }
        Ok(Self { catalog, adapters })
    }

    /// The in-crate deterministic binding seam.
    ///
    /// It exists only under `cfg(test)` and is `pub(crate)`, so it is not
    /// part of the published API: [`ModelBindingRegistry::new`] is the one
    /// binding path a consumer of this library can call, and it constructs
    /// the three supported protocol adapters directly.
    #[cfg(test)]
    pub(crate) fn new_with_scripted_adapters(
        catalog: ResolvedModelCatalog,
        factory: &dyn ScriptedProviderAdapterFactory,
    ) -> Result<Self, ModelInvocationError> {
        let mut adapters: BTreeMap<(ProviderId, ModelProtocol), Arc<dyn ModelAdapter>> =
            BTreeMap::new();
        for reference in catalog.model_refs().collect::<Vec<_>>() {
            let (provider, model) = catalog.binding(&reference)?;
            let key = (provider.id().clone(), model.protocol);
            if let std::collections::btree_map::Entry::Vacant(slot) = adapters.entry(key) {
                slot.insert(factory.adapter(provider, model.protocol)?);
            }
        }
        Ok(Self { catalog, adapters })
    }

    /// The resolved catalog behind this registry.
    #[must_use]
    pub const fn catalog(&self) -> &ResolvedModelCatalog {
        &self.catalog
    }

    /// Resolves one selection into an immutable invocation.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure; resolution is transactional in
    /// the sense that a failure produces no partially-resolved value.
    pub fn resolve(
        &self,
        selection: &ModelSelection,
    ) -> Result<ResolvedModelInvocation, ModelInvocationError> {
        self.resolve_with_layer(selection, RequestParamsLayer::SessionOverrides)
    }

    /// Resolves one selection, attributing override validation failures to
    /// the given configuration layer.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure.
    pub fn resolve_with_layer(
        &self,
        selection: &ModelSelection,
        layer: RequestParamsLayer,
    ) -> Result<ResolvedModelInvocation, ModelInvocationError> {
        let (provider, model) = self.catalog.binding(&selection.model)?;
        let protocol = model.protocol;

        let (profile_id, profile) = select_reasoning_profile(&selection.model, model, selection)?;

        validate_request_params_layer(&selection.request_params, protocol, layer)
            .map_err(ModelInvocationError::ProtectedKey)?;

        // The selected profile owns every top-level key it declares.
        if let (Some(profile_id), Some(profile)) = (profile_id.as_ref(), profile) {
            for key in selection.request_params.keys() {
                if profile.request_params.contains_key(key) {
                    return Err(ModelInvocationError::ReasoningProfileKeyOwnership {
                        model: selection.model.clone(),
                        profile: profile_id.clone(),
                        key: key.clone(),
                    });
                }
            }
        }

        let mut request_params = model.request_params.clone();
        if let Some(profile) = profile {
            overlay_shallow(&mut request_params, &profile.request_params);
        }
        overlay_shallow(&mut request_params, &selection.request_params);

        let effective_output_tokens = match selection.max_output_tokens {
            None => model.max_output_tokens,
            Some(0) => {
                return Err(ModelInvocationError::InvalidOutputBudget {
                    model: selection.model.clone(),
                    detail: "the output budget must be positive".to_owned(),
                });
            }
            Some(requested) if requested > model.max_output_tokens => {
                return Err(ModelInvocationError::InvalidOutputBudget {
                    model: selection.model.clone(),
                    detail: format!(
                        "requested {requested} exceeds the model maximum {}",
                        model.max_output_tokens
                    ),
                });
            }
            Some(requested) => requested,
        };

        let capabilities = effective_capabilities(&model.capabilities, protocol);
        if !capabilities.supports_text_conversation() {
            return Err(ModelInvocationError::UnusableCapabilities {
                model: selection.model.clone(),
                detail: "the effective capabilities cannot carry text input and text output"
                    .to_owned(),
            });
        }

        let adapter = self
            .adapters
            .get(&(provider.id().clone(), protocol))
            .ok_or_else(|| ModelInvocationError::MissingAdapter {
                model: selection.model.clone(),
                protocol,
            })?
            .clone();

        Ok(ResolvedModelInvocation {
            provider: provider.id().clone(),
            model_ref: selection.model.clone(),
            adapter,
            protocol,
            context_window: model.context_window,
            model_max_output_tokens: model.max_output_tokens,
            effective_output_tokens,
            reasoning_profile: profile_id,
            // A reasoning-capable model without a profile block has
            // provider-default reasoning semantics: reasoning is always on,
            // but there is no selectable profile and no synthetic wire field.
            reasoning_enabled: if model.capabilities.reasoning {
                profile.is_none_or(|profile| profile.enabled)
            } else {
                false
            },
            request_params,
            capabilities,
            declared_capabilities: model.capabilities.clone(),
            compat: model.compat,
        })
    }

    /// The safe public catalog view served to Runtime Clients.
    #[must_use]
    pub fn catalog_view(&self) -> ModelCatalogView {
        let mut models = Vec::new();
        for reference in self.catalog.model_refs() {
            let Ok((provider, model)) = self.catalog.binding(&reference) else {
                continue;
            };
            models.push(CatalogModelView {
                model: reference,
                protocol: model.protocol,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                declared_capabilities: model.capabilities.clone(),
                effective_capabilities: effective_capabilities(&model.capabilities, model.protocol),
                reasoning_profiles: model
                    .reasoning
                    .as_ref()
                    .map(|reasoning| {
                        reasoning
                            .profiles
                            .iter()
                            .map(|(id, profile)| ReasoningProfileView {
                                id: id.clone(),
                                enabled: profile.enabled,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                default_reasoning_profile: model.default_reasoning_profile().cloned(),
                credential_source: provider.credential_source(),
            });
        }
        ModelCatalogView { models }
    }
}

/// The adapter substitution used by the crate's own deterministic suites.
///
/// It exists only under `cfg(test)`, so no consumer of this library can
/// substitute an arbitrary [`ModelAdapter`] for a validated catalog binding.
/// The one binding path is [`ModelBindingRegistry::new`], which constructs
/// the three supported protocol adapters explicitly.
#[cfg(test)]
pub(crate) trait ScriptedProviderAdapterFactory: Send + Sync {
    fn adapter(
        &self,
        provider: &ResolvedProvider,
        protocol: ModelProtocol,
    ) -> Result<Arc<dyn ModelAdapter>, ModelInvocationError>;
}

type SelectedProfile<'a> = (
    Option<ReasoningProfileId>,
    Option<&'a crate::model::catalog::ReasoningProfile>,
);

/// Selects the reasoning profile of one resolution: the explicit selection
/// when present, otherwise the model default. No profile is ever
/// synthesized.
fn select_reasoning_profile<'a>(
    reference: &ModelRef,
    model: &'a ModelDefinition,
    selection: &ModelSelection,
) -> Result<SelectedProfile<'a>, ModelInvocationError> {
    let Some(requested) = selection.reasoning_profile.as_ref() else {
        let Some(default) = model.default_reasoning_profile() else {
            return Ok((None, None));
        };
        let profile = model
            .reasoning_profile(default)
            .expect("catalog validation guarantees the default profile exists");
        return Ok((Some(default.clone()), Some(profile)));
    };
    if model.reasoning.is_none() {
        return Err(ModelInvocationError::ModelDeclaresNoReasoningProfiles {
            model: reference.clone(),
            profile: requested.clone(),
        });
    }
    let profile = model.reasoning_profile(requested).ok_or_else(|| {
        ModelInvocationError::UnknownReasoningProfile {
            model: reference.clone(),
            profile: requested.clone(),
        }
    })?;
    Ok((Some(requested.clone()), Some(profile)))
}

/// The stable serialized name of a protocol, for diagnostics.
fn protocol_name(protocol: ModelProtocol) -> String {
    serde_json::to_value(protocol)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{protocol:?}"))
}

/// Whether every canonical modality present in a message list is supported.
///
/// This is the early rejection boundary: the runtime refuses content its
/// effective capabilities cannot represent *before* a provider request is
/// opened, so the provider is never the first validator of a known rustX
/// capability mismatch.
///
/// # Errors
///
/// Returns [`ModelErrorKind::Unsupported`] naming the unsupported modality.
pub fn validate_content_modalities(
    messages: &[crate::message::types::MessageBlock],
    capabilities: &ModelCapabilities,
) -> Result<(), ModelError> {
    use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
    use crate::tools::types::ToolResultContent;

    let mut required: Vec<Modality> = Vec::new();
    let mut require = |modality: Modality| {
        if !required.contains(&modality) {
            required.push(modality);
        }
    };
    for message in messages {
        match message {
            MessageBlock::System(_) => require(Modality::Text),
            MessageBlock::User(user) => {
                for content in &user.content {
                    match content {
                        UserContentBlock::Text(_) => require(Modality::Text),
                        UserContentBlock::Image(_) => require(Modality::Image),
                        UserContentBlock::File(_) => require(Modality::File),
                    }
                }
            }
            MessageBlock::Assistant(assistant) => {
                for content in &assistant.content {
                    match content {
                        AssistantContentBlock::Image(_) => require(Modality::Image),
                        AssistantContentBlock::Text(_)
                        | AssistantContentBlock::Refusal(_)
                        | AssistantContentBlock::Reasoning(_)
                        | AssistantContentBlock::ToolCall(_) => require(Modality::Text),
                    }
                }
            }
            MessageBlock::Tool(tool) => {
                for content in &tool.result.content {
                    match content {
                        ToolResultContent::Image(_) => require(Modality::Image),
                        ToolResultContent::File(_) => require(Modality::File),
                        ToolResultContent::Text(_) | ToolResultContent::Json { .. } => {
                            require(Modality::Text);
                        }
                    }
                }
            }
        }
    }
    for modality in required {
        if !capabilities.input_modalities.contains(&modality) {
            return Err(ModelError {
                kind: ModelErrorKind::Unsupported,
                message: format!(
                    "the effective model capabilities do not support {modality:?} input; \
                     the request is rejected before any provider request"
                ),
                retry_after_ms: None,
                provider_code: None,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ModelInvocationError, RequestParams, RequestParamsLayer, finalize_provider_request,
        overlay_shallow, protected_keys, validate_request_params_layer,
    };
    use crate::model::types::ModelProtocol;

    fn params(json: serde_json::Value) -> RequestParams {
        match json {
            serde_json::Value::Object(map) => map,
            other => panic!("requestParams must be an object, got {other}"),
        }
    }

    /// A nested object value is replaced atomically: rustX never deep-merges.
    #[test]
    fn nested_object_replacement_is_shallow() {
        let mut base = params(serde_json::json!({"routing": {"a": 1, "b": 2}, "keep": true}));
        overlay_shallow(&mut base, &params(serde_json::json!({"routing": {"c": 3}})));
        assert_eq!(
            serde_json::Value::Object(base),
            serde_json::json!({"routing": {"c": 3}, "keep": true}),
            "the nested object is replaced whole, never merged key by key"
        );
    }

    /// Array values are atomic too.
    #[test]
    fn array_replacement_is_atomic() {
        let mut base = params(serde_json::json!({"stop": ["a", "b", "c"]}));
        overlay_shallow(&mut base, &params(serde_json::json!({"stop": ["z"]})));
        assert_eq!(
            serde_json::Value::Object(base),
            serde_json::json!({"stop": ["z"]})
        );
    }

    /// Unknown, non-protected keys are valid and survive verbatim.
    #[test]
    fn unknown_unprotected_keys_are_accepted() {
        let extension = params(serde_json::json!({
            "top_k": 40,
            "min_p": 0.05,
            "repetition_penalty": 1.1,
            "chat_template_kwargs": {"enable_thinking": false},
            "provider": {"order": ["a", "b"]}
        }));
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            validate_request_params_layer(
                &extension,
                protocol,
                RequestParamsLayer::SessionOverrides,
            )
            .expect("extension keys are not protected");
        }
    }

    /// Every protocol protects its own runtime-owned structural fields, and
    /// both Chat Completions max-token spellings are protected together.
    #[test]
    fn protected_sets_are_protocol_specific() {
        assert!(protected_keys(ModelProtocol::OpenAiChatCompletions).contains(&"max_tokens"));
        assert!(
            protected_keys(ModelProtocol::OpenAiChatCompletions).contains(&"max_completion_tokens")
        );
        assert!(protected_keys(ModelProtocol::OpenAiResponses).contains(&"store"));
        assert!(protected_keys(ModelProtocol::OpenAiResponses).contains(&"previous_response_id"));
        assert!(protected_keys(ModelProtocol::OpenAiResponses).contains(&"include"));
        assert!(protected_keys(ModelProtocol::AnthropicMessages).contains(&"system"));
        // Provider-owned reasoning/sampling fields are deliberately not
        // protected: a reasoning profile must be able to own them.
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            for key in ["thinking", "reasoning", "output_config", "temperature"] {
                assert!(
                    !protected_keys(protocol).contains(&key),
                    "{key} must stay provider-owned for {protocol:?}"
                );
            }
        }
    }

    /// The final overlay lands at the provider request top level and refuses
    /// to overwrite a runtime-owned field.
    #[test]
    fn final_overlay_is_top_level_and_protected() {
        let translated = serde_json::json!({
            "model": "m",
            "messages": [],
            "stream": true,
            "max_completion_tokens": 128
        });
        let final_request = finalize_provider_request(
            translated.clone(),
            &params(serde_json::json!({"temperature": 0.7, "top_k": 40})),
            ModelProtocol::OpenAiChatCompletions,
        )
        .expect("overlay applies");
        assert_eq!(final_request["temperature"], 0.7);
        assert_eq!(final_request["top_k"], 40);
        assert_eq!(final_request["max_completion_tokens"], 128);
        assert!(
            final_request.get("extra_body").is_none(),
            "there is no invented extra_body nesting level"
        );

        let rejected = finalize_provider_request(
            translated,
            &params(serde_json::json!({"messages": ["hijack"]})),
            ModelProtocol::OpenAiChatCompletions,
        )
        .expect_err("protected key");
        assert_eq!(
            rejected.kind,
            crate::model::error::ModelErrorKind::InvalidRequest
        );
        assert!(rejected.message.contains("messages"));
    }

    /// A non-object translated request can never be overlaid.
    #[test]
    fn non_object_requests_are_rejected() {
        let error = finalize_provider_request(
            serde_json::json!([1, 2, 3]),
            &RequestParams::new(),
            ModelProtocol::OpenAiResponses,
        )
        .expect_err("must fail");
        assert_eq!(
            error.kind,
            crate::model::error::ModelErrorKind::InvalidRequest
        );
    }

    /// The protected-key error keeps its layer attribution.
    #[test]
    fn protected_collisions_name_their_layer() {
        let error = validate_request_params_layer(
            &params(serde_json::json!({"store": false})),
            ModelProtocol::OpenAiResponses,
            RequestParamsLayer::SummaryOverrides,
        )
        .expect_err("must fail");
        assert_eq!(error.key, "store");
        assert_eq!(error.layer, RequestParamsLayer::SummaryOverrides);
        let wrapped = ModelInvocationError::ProtectedKey(error);
        assert!(wrapped.to_string().contains("summary"));
    }
}
