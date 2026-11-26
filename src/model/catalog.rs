//! The runtime-owned validated model catalog (Issue #42).
//!
//! The catalog is the *only* way a rustX local runtime learns that a model
//! exists, which protocol speaks to it, where its provider endpoint is, and
//! which credential authorizes it. There is deliberately no other path:
//!
//! ```text
//! rustx.toml -> RuntimeLayer -> ModelCatalogDocument -> ModelCatalog
//!               (strict TOML)       (native typed)          (validated)
//! ModelCatalog -> ResolvedModelCatalog (host credentials bound at admission)
//! ```
//!
//! # Explicit provider binding
//!
//! A provider declares its `base_url` and its `api_key` source explicitly. A
//! provider *name* carries no endpoint semantics: naming a provider
//! `openai` or `anthropic` never selects an official endpoint, and no
//! adapter constructor can reach the network without a base URL that came
//! from this catalog. The bounded credential syntax is exactly a literal
//! string or a `$ENV_VAR` reference — no shell commands, no OAuth, no
//! keychain, no credential plugins, and no auth profiles.
//!
//! # Credential redaction
//!
//! A resolved credential lives in [`ResolvedCredential`], which has no
//! `Serialize`, a redacted `Debug`, and a redacted `Display`. The only way
//! to read it is [`ResolvedCredential::expose`], which exists solely for the
//! adapter construction boundary. Every client-facing view carries at most
//! the credential *source kind* and the environment variable *name*
//! ([`CredentialSourceView`]), never the value.
//!
//! # What the catalog does not own
//!
//! Provider wire parameters are opaque ([`crate::model::invocation`] owns
//! the overlay and protected-key contract). A Model may declare named
//! [`ModelProfile`]s: complete invocation presets carrying their own opaque
//! `request_params` and an optional
//! default output budget. The runtime assigns no meaning to a profile name,
//! and a Profile never changes a Model's identity, protocol, context window,
//! hard output maximum, or declared capabilities.
//! Structural translation behaviour lives in the bounded [`ModelCompat`].
//! Built-in API/model/site rules and optional source overrides are resolved
//! before freezing the binding. Historical Chat reasoning replay is a wire
//! translation policy, independent of generation-time reasoning controls.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::invocation::{
    ModelSelection, RequestParams, RequestParamsLayer, analyze_selection,
    validate_request_params_layer, validate_request_params_numbers,
};
use crate::model::types::ModelProtocol;
pub use rustx_types::model::catalog::{
    ModelCatalogError, ModelId, ModelProfileId, ModelRef, ProviderId,
};

/// The only model-catalog schema version this runtime accepts.
pub const MODEL_CATALOG_SCHEMA_VERSION: u32 = 2;

/// The declared source of one provider credential.
///
/// The syntax is exactly two forms and is never extended implicitly: a
/// literal string, or `$ENV_VAR`.
#[derive(Clone, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub enum CredentialSource {
    /// A literal credential written into the catalog file.
    Literal(String),
    /// A credential read from the named process environment variable at
    /// startup.
    Environment(String),
}

impl CredentialSource {
    /// Parses the bounded credential syntax.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::InvalidCredentialSource`] for an empty
    /// value or a malformed environment-variable name.
    pub fn parse(value: &str, provider: &ProviderId) -> Result<Self, ModelCatalogError> {
        if value.is_empty() {
            return Err(ModelCatalogError::InvalidCredentialSource {
                provider: provider.clone(),
                detail: "the api_key source must not be empty".to_owned(),
            });
        }
        let Some(name) = value.strip_prefix('$') else {
            return Ok(Self::Literal(value.to_owned()));
        };
        let valid = crate::credentials::valid_environment_name(name);
        if !valid {
            return Err(ModelCatalogError::InvalidCredentialSource {
                provider: provider.clone(),
                detail: "expected a valid $ENV_VAR credential reference".into(),
            });
        }
        Ok(Self::Environment(name.to_owned()))
    }

    /// The redacted client-facing view of this source.
    #[must_use]
    pub fn view(&self) -> CredentialSourceView {
        match self {
            Self::Literal(_) => CredentialSourceView::Literal,
            Self::Environment(name) => CredentialSourceView::Environment {
                variable: name.clone(),
            },
        }
    }
}

impl fmt::Debug for CredentialSource {
    /// Redacted: a literal credential never appears in debug output, and an
    /// environment reference shows only the variable name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(_) => f.write_str("CredentialSource::Literal(<redacted>)"),
            Self::Environment(name) => write!(f, "CredentialSource::Environment({name})"),
        }
    }
}

impl Serialize for CredentialSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Environment(name) => serializer.serialize_str(&format!("${name}")),
            Self::Literal(_) => serde_json::json!({"redacted": true}).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CredentialSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw, &ProviderId::new("configured")).map_err(serde::de::Error::custom)
    }
}

/// The redacted client-facing description of a credential source.
///
/// This is safe to place in Runtime Client results: it carries the source
/// *kind* and, for an environment reference, the variable *name*. It never
/// carries a credential value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum CredentialSourceView {
    /// The credential is a literal catalog value.
    Literal,
    /// The credential is read from the named environment variable.
    Environment {
        /// The environment variable name (never its value).
        variable: String,
    },
}

/// A resolved provider credential.
///
/// The value is deliberately unreachable except through
/// [`ResolvedCredential::expose`]: the type has no `Serialize`, its `Debug`
/// and `Display` are redacted, and it never appears in an error, an event,
/// a snapshot, or a panic message.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedCredential(String);

impl ResolvedCredential {
    /// Wraps a resolved credential value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Exposes the credential for adapter construction.
    ///
    /// This is the one intentional read boundary. Callers must pass the
    /// value straight into a provider client and must never log, format, or
    /// re-expose it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

pub use rustx_types::model::capabilities::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol, Modality,
    ModelCapabilities, ModelCompat, ResponsesStorageMode,
};

impl From<crate::model::authoring::Compat> for ModelCompat {
    fn from(document: crate::model::authoring::Compat) -> Self {
        let explicit_fields = u8::from(document.chat_max_tokens_field.is_some())
            | (u8::from(document.chat_stream_usage.is_some()) << 1)
            | (u8::from(document.chat_reasoning_replay.is_some()) << 2)
            | (u8::from(document.responses_storage.is_some()) << 3)
            | (u8::from(document.chat_tool_protocol.is_some()) << 4);
        Self {
            chat_max_tokens_field: document.chat_max_tokens_field.unwrap_or_default(),
            chat_stream_usage: document.chat_stream_usage.unwrap_or_default(),
            chat_reasoning_replay: document.chat_reasoning_replay,
            chat_tool_protocol: document.chat_tool_protocol.unwrap_or_default(),
            responses_storage: document.responses_storage.unwrap_or_default(),
            explicit_fields,
        }
    }
}

/// Resolve optional authoring overrides once, before an invocation is
/// frozen. Endpoint/model rules never change an explicit declaration.
/// Kept monolith-side: it consults the catalog-domain `compat` heuristics.
pub(crate) fn compat_for_binding(
    mut compat: ModelCompat,
    protocol: ModelProtocol,
    model: &str,
    base_url: &str,
) -> ModelCompat {
    if protocol == ModelProtocol::OpenAiChatCompletions {
        if !compat.chat_max_tokens_field_is_explicit() {
            compat.chat_max_tokens_field =
                crate::model::compat::chat_max_tokens_field(model, base_url);
        }
        compat.chat_reasoning_replay = Some(compat.chat_reasoning_replay.unwrap_or_default());
    }
    if protocol == ModelProtocol::OpenAiResponses && !compat.responses_storage_is_explicit() {
        compat.responses_storage = crate::model::compat::responses_storage(base_url);
    }
    compat
}

/// One validated Model Profile: a complete, independent invocation preset.
///
/// A Profile never inherits request parameters from its Model or from
/// another Profile: its `request_params` are the complete native object of
/// an invocation that selects it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelProfile {
    /// The default output budget of an invocation selecting this profile;
    /// the Model's hard maximum when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// The complete provider-native request parameters of this profile.
    ///
    /// The profile owns every top-level key it declares: an explicit
    /// selection override may not also declare one of them.
    #[serde(default)]
    pub request_params: RequestParams,
}

/// One declared Model Profile of the programmatic catalog document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelProfileDocument {
    /// The declared default output budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// The complete provider-native request parameters.
    #[serde(default)]
    pub request_params: RequestParams,
}

/// One validated catalog model definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDefinition {
    /// Explicit provider binding, independent of the catalog name.
    pub provider: ProviderId,
    /// The model identity within its provider.
    pub id: ModelId,
    /// The protocol an adapter must speak to this model.
    pub protocol: ModelProtocol,
    /// The model context window in tokens.
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The capabilities the catalog claims for this model.
    pub capabilities: ModelCapabilities,
    /// The model-level native request parameters of a Model without
    /// profiles. Always empty when the Model declares profiles.
    pub request_params: RequestParams,
    /// The profile selected when an invocation does not choose one; present
    /// exactly when `profiles` is nonempty.
    pub default_profile: Option<ModelProfileId>,
    /// Optional complete Profile order, lowest declared reasoning strength first.
    pub reasoning_profile_order: Option<Vec<ModelProfileId>>,
    /// The declared profiles; empty when the Model declares none.
    pub profiles: BTreeMap<ModelProfileId, ModelProfile>,
    /// The bounded structural translation metadata.
    pub compat: ModelCompat,
}

impl ModelDefinition {
    /// Resolves one declared profile by identity.
    #[must_use]
    pub fn profile(&self, id: &ModelProfileId) -> Option<&ModelProfile> {
        self.profiles.get(id)
    }
}

/// One validated catalog provider (before credential resolution).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDefinition {
    /// The API protocol shared by every model of this provider.
    pub protocol: ModelProtocol,
    /// The provider identity.
    pub id: ProviderId,
    /// The mandatory explicit provider endpoint.
    pub base_url: String,
    /// The declared credential source.
    pub api_key: CredentialSource,
}

/// Individually validated catalog entries, before reference closure.
type CatalogEntries = (
    BTreeMap<ProviderId, ProviderDefinition>,
    BTreeMap<ModelRef, Arc<ModelDefinition>>,
);

/// The validated model catalog.
///
/// Validation is complete at this point: every provider has an explicit
/// endpoint and credential source, every model has a known protocol, sane
/// limits, coherent capabilities, valid profiles, and request parameters that collide with no
/// runtime-protected wire key.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCatalog {
    providers: BTreeMap<ProviderId, ProviderDefinition>,
    models: BTreeMap<ModelRef, Arc<ModelDefinition>>,
    // Native immutable ordinal indexes share identity bytes with the authoritative maps.
    model_ordinals: Arc<[ModelRef]>,
    profile_ordinals: Arc<[Arc<[ModelProfileId]>]>,
}

impl ModelCatalog {
    /// Credential-free model choices in deterministic presentation order.
    #[must_use]
    pub fn view(&self) -> ModelCatalogView {
        let mut models = Vec::new();
        for reference in self.model_ordinals.iter().cloned() {
            let model = &self.models[&reference];
            let provider = &self.providers[&model.provider];
            models.push(CatalogModelView {
                model: reference,
                protocol: model.protocol,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                declared_capabilities: model.capabilities.clone(),
                effective_capabilities: super::invocation::effective_capabilities(
                    &model.capabilities,
                    model.protocol,
                ),
                profiles: model
                    .profiles
                    .iter()
                    .map(|(id, profile)| ModelProfileView {
                        id: id.clone(),
                        max_output_tokens: profile.max_output_tokens,
                    })
                    .collect(),
                default_profile: model.default_profile.clone(),
                reasoning_profile_order: model.reasoning_profile_order.clone(),
                credential_source: provider.api_key.view(),
            });
        }
        ModelCatalogView { models }
    }

    /// Parses and validates a catalog from TOML bytes.
    ///
    /// Strict `snake_case` authoring types resolve into native configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::Syntax`] for malformed TOML (including
    /// unknown fields and duplicate provider identities) and a specific
    /// validation error otherwise.
    pub fn from_toml_slice(bytes: &[u8]) -> Result<Self, ModelCatalogError> {
        let document: super::authoring::Catalog = crate::toml_authoring::parse(bytes)
            .map_err(|detail| ModelCatalogError::Syntax { detail })?;
        let catalog = Self::from_authoring(document)?;
        if catalog.models.is_empty() {
            return Err(ModelCatalogError::EmptyCatalog);
        }
        Ok(catalog)
    }

    /// Validate the authored catalog, then index only visible model choices.
    /// Source settings may publish an empty catalog to existing Sessions.
    pub(crate) fn from_authoring(
        document: super::authoring::Catalog,
    ) -> Result<Self, ModelCatalogError> {
        let hidden = document
            .models
            .iter()
            .filter(|(_, model)| {
                model.hidden
                    || document
                        .providers
                        .get(&model.provider)
                        .is_some_and(|provider| provider.hidden)
            })
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        Self::from_document_with_choices(document.into(), |reference| {
            !hidden.contains(reference.as_str())
        })
    }

    /// Validates an already-parsed catalog document.
    ///
    /// # Errors
    ///
    /// Returns the first validation failure.
    pub fn from_document(document: ModelCatalogDocument) -> Result<Self, ModelCatalogError> {
        let catalog = Self::from_document_with_choices(document, |_| true)?;
        if catalog.models.is_empty() {
            return Err(ModelCatalogError::EmptyCatalog);
        }
        Ok(catalog)
    }

    fn from_document_with_choices(
        document: ModelCatalogDocument,
        visible: impl Fn(&ModelRef) -> bool,
    ) -> Result<Self, ModelCatalogError> {
        let (providers, models) = Self::validated_entries(document)?;
        let model_ordinals: Arc<[_]> = models
            .keys()
            .filter(|reference| visible(reference))
            .cloned()
            .collect();
        let profile_ordinals = model_ordinals
            .iter()
            .map(|reference| models[reference].profiles.keys().cloned().collect())
            .collect();
        Ok(Self {
            providers,
            models,
            model_ordinals,
            profile_ordinals,
        })
    }

    /// Validates every authored Provider and Model on its own terms.
    ///
    /// This is exactly the part of [`Self::from_document`] one entry decides
    /// alone. Whether a Model's Provider exists and whether the catalog is
    /// nonempty are facts of the complete catalog: a configuration source may
    /// leave either to the other scope it is composed with.
    ///
    /// # Errors
    ///
    /// Returns the first entry validation failure.
    pub fn validate_entries(document: ModelCatalogDocument) -> Result<(), ModelCatalogError> {
        validate_schema_version(document.schema_version)?;
        let mut protocols = BTreeMap::new();
        for (raw_id, provider) in document.providers {
            let id = ProviderId::parse(raw_id)?;
            let definition = validate_provider(&id, provider)?;
            protocols.insert(id, definition.protocol);
        }
        for (name, model) in document.models {
            let reference = ModelRef::parse(&name)?;
            let provider = ProviderId::parse(model.provider.clone())?;
            validate_model_fields(&reference, &model, protocols.get(&provider).copied())?;
        }
        Ok(())
    }

    fn validated_entries(
        document: ModelCatalogDocument,
    ) -> Result<CatalogEntries, ModelCatalogError> {
        validate_schema_version(document.schema_version)?;
        let mut providers = BTreeMap::new();
        for (raw_id, provider) in document.providers {
            let id = ProviderId::parse(raw_id)?;
            let definition = validate_provider(&id, provider)?;
            providers.insert(id, definition);
        }
        let mut models = BTreeMap::new();
        for (name, document) in document.models {
            let reference = ModelRef::parse(&name)?;
            let provider = ProviderId::parse(document.provider.clone())?;
            let protocol = providers
                .get(&provider)
                .ok_or(ModelCatalogError::UnknownProvider { provider })?
                .protocol;
            models.insert(
                reference.clone(),
                Arc::new(resolve_model(&reference, document, protocol)?),
            );
        }
        Ok((providers, models))
    }

    /// The providers in deterministic identity order.
    pub fn providers(&self) -> impl Iterator<Item = &ProviderDefinition> {
        self.providers.values()
    }

    /// Resolves one model reference.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`] or
    /// [`ModelCatalogError::UnknownModel`].
    pub fn model(&self, reference: &ModelRef) -> Result<&Arc<ModelDefinition>, ModelCatalogError> {
        self.models
            .get(reference)
            .ok_or_else(|| ModelCatalogError::UnknownModel {
                model: reference.clone(),
            })
    }

    /// Whether `selection` resolved against this catalog is exactly the
    /// invocation `other_selection` resolves to against `other`.
    ///
    /// This is the effective request semantics an Attempt would freeze — the
    /// resolved Profile, output budget, opaque parameters and
    /// capabilities ([`analyze_selection`]) — together with the binding they
    /// travel over: provider endpoint and credential source, wire model
    /// identity, protocol, limits and compat. How a selection is authored
    /// (an omitted versus an explicit default Profile) and catalog entries it
    /// does not resolve through (other Models, unselected Profiles) are not
    /// part of it. A selection that fails to resolve on either side is never
    /// the same.
    pub(crate) fn same_invocation(
        &self,
        selection: &ModelSelection,
        other: &Self,
        other_selection: &ModelSelection,
        layer: RequestParamsLayer,
    ) -> bool {
        let effective = |catalog: &Self, selection: &ModelSelection| {
            let model = catalog.models.get(&selection.model)?;
            let provider = catalog.providers.get(&model.provider)?;
            let view = analyze_selection(model, selection, layer).ok()?;
            Some((
                view,
                model.id.clone(),
                compat_for_binding(
                    model.compat,
                    model.protocol,
                    model.id.as_str(),
                    &provider.base_url,
                ),
                provider.clone(),
            ))
        };
        matches!(
            (effective(self, selection), effective(other, other_selection)),
            (Some(left), Some(right)) if left == right
        )
    }

    /// Every model reference in deterministic presentation order.
    pub fn model_refs(&self) -> impl Iterator<Item = ModelRef> + '_ {
        self.models.keys().cloned()
    }

    /// Visible choices in the same order as the published catalog.
    pub(crate) fn selectable_model_refs(&self) -> impl Iterator<Item = &ModelRef> {
        self.model_ordinals.iter()
    }

    /// Captures credential inputs for every provider without requiring unused keys.
    ///
    /// # Errors
    ///
    /// Kept as a fallible binding boundary; missing references are diagnosed
    /// lazily by `ResolvedProvider::credential` when a provider is admitted.
    pub fn resolve(
        &self,
        environment: &dyn CredentialEnvironment,
    ) -> Result<ResolvedModelCatalog, ModelCatalogError> {
        let mut providers = BTreeMap::new();
        let captured = environment.snapshot();
        for (id, provider) in &self.providers {
            providers.insert(
                id.clone(),
                ResolvedProvider {
                    id: id.clone(),
                    base_url: provider.base_url.clone(),
                    credential: std::sync::Arc::new(std::sync::OnceLock::new()),
                    captured: captured.clone(),
                    source: provider.api_key.clone(),
                },
            );
        }
        Ok(ResolvedModelCatalog {
            catalog: self.clone(),
            providers,
        })
    }
}

/// One provider with its credential bound.
#[derive(Clone)]
pub struct ResolvedProvider {
    id: ProviderId,
    base_url: String,
    credential: std::sync::Arc<std::sync::OnceLock<Result<ResolvedCredential, ModelCatalogError>>>,
    captured: crate::credentials::CredentialSnapshot,
    source: CredentialSource,
}

impl ResolvedProvider {
    /// The provider identity.
    #[must_use]
    pub const fn id(&self) -> &ProviderId {
        &self.id
    }

    /// The explicit provider endpoint.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The bound credential.
    ///
    /// # Errors
    /// Returns a provider-specific missing-reference error on first admitted use.
    pub fn credential(&self) -> Result<&ResolvedCredential, ModelCatalogError> {
        self.credential
            .get_or_init(|| match &self.source {
                CredentialSource::Literal(value) => Ok(ResolvedCredential::new(value.clone())),
                CredentialSource::Environment(name) => self
                    .captured
                    .var(name)
                    .filter(|value| !value.is_empty())
                    .map(ResolvedCredential::new)
                    .ok_or_else(|| ModelCatalogError::MissingEnvironmentCredential {
                        provider: self.id.clone(),
                        variable: name.clone(),
                    }),
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The redacted credential source view.
    #[must_use]
    pub fn credential_source(&self) -> CredentialSourceView {
        self.source.view()
    }

    /// The **declared** credential source of this provider.
    ///
    /// This is catalog configuration, not a resolved secret: it is the same
    /// bounded two-form value the catalog file declared. It exists so a
    /// frozen provider binding can carry the declaration to another process
    /// that resolves it through its own [`CredentialEnvironment`], instead
    /// of that process reopening the mutable catalog. Use
    /// [`ResolvedProvider::credential_source`] for anything client-facing.
    #[must_use]
    pub const fn credential_declaration(&self) -> &CredentialSource {
        &self.source
    }
}

impl fmt::Debug for ResolvedProvider {
    /// Redacted: the bound credential never appears in debug output.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedProvider")
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .field("credential", &"<redacted>")
            .field("captured", &self.captured)
            .field("credential_source", &self.source)
            .finish()
    }
}

/// The catalog with every provider credential bound.
#[derive(Clone, Debug)]
pub struct ResolvedModelCatalog {
    catalog: ModelCatalog,
    providers: BTreeMap<ProviderId, ResolvedProvider>,
}

impl ResolvedModelCatalog {
    /// The underlying validated catalog.
    #[must_use]
    pub const fn catalog(&self) -> &ModelCatalog {
        &self.catalog
    }

    /// Resolves one provider binding.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`].
    pub fn provider(&self, id: &ProviderId) -> Result<&ResolvedProvider, ModelCatalogError> {
        self.providers
            .get(id)
            .ok_or_else(|| ModelCatalogError::UnknownProvider {
                provider: id.clone(),
            })
    }

    /// Resolves one model reference to its provider binding and definition.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`] or
    /// [`ModelCatalogError::UnknownModel`].
    pub fn binding(
        &self,
        reference: &ModelRef,
    ) -> Result<(&ResolvedProvider, &Arc<ModelDefinition>), ModelCatalogError> {
        let model = self.catalog.model(reference)?;
        let provider = self.provider(&model.provider)?;
        Ok((provider, model))
    }

    /// Every model reference in deterministic order.
    pub fn model_refs(&self) -> impl Iterator<Item = ModelRef> + '_ {
        self.catalog.model_refs()
    }
}

/// The process-environment lookup used to resolve `$ENV_VAR` credentials.
pub trait CredentialEnvironment: Send + Sync {
    /// Capture a stable host input without resolving a particular reference.
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot;
    /// Reads one environment variable.
    fn var(&self, name: &str) -> Option<String>;
}

/// The real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessCredentialEnvironment;

impl CredentialEnvironment for ProcessCredentialEnvironment {
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot {
        crate::credentials::CredentialSnapshot::capture()
    }
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// An explicit in-memory environment, used by deterministic tests and by
/// callers that resolve credentials from an already-collected map.
#[derive(Clone, Default)]
pub struct MapCredentialEnvironment {
    variables: BTreeMap<String, String>,
}

impl fmt::Debug for MapCredentialEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MapCredentialEnvironment(<redacted>)")
    }
}

impl MapCredentialEnvironment {
    /// Creates an environment from name/value pairs.
    #[must_use]
    pub fn new(variables: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            variables: variables.into_iter().collect(),
        }
    }
}

impl CredentialEnvironment for MapCredentialEnvironment {
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot {
        crate::credentials::CredentialSnapshot::new(self.variables.clone())
    }
    fn var(&self, name: &str) -> Option<String> {
        self.variables.get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// Document (wire) shapes
// ---------------------------------------------------------------------------

/// The native programmatic catalog document. TOML authoring is owned by
/// [`super::authoring::Catalog`] and resolves into this typed representation.
///
/// Unknown fields are rejected everywhere: a typo must fail loudly rather
/// than silently changing runtime semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCatalogDocument {
    /// The catalog schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The providers, keyed by provider identity. Duplicate keys are
    /// rejected rather than silently resolved by last-write-wins.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub providers: BTreeMap<String, ProviderDocument>,
    /// Independently named complete Model definitions.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub models: BTreeMap<String, ModelDocument>,
}

const fn default_schema_version() -> u32 {
    MODEL_CATALOG_SCHEMA_VERSION
}

/// One provider entry of the catalog document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ProviderDocument {
    /// The API protocol shared by every model of this provider.
    pub protocol: ModelProtocol,
    /// The mandatory explicit provider endpoint.
    pub base_url: String,
    /// The mandatory credential source: a literal or `$ENV_VAR`.
    pub api_key: CredentialSource,
}

/// One model entry of the catalog document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelDocument {
    /// Explicit Provider identity.
    pub provider: String,
    /// The model identity within its provider.
    pub id: String,
    /// The model context window in tokens.
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The claimed capabilities.
    pub capabilities: ModelCapabilities,
    /// The model-level native request parameters. Presence is tracked: a
    /// Model with profiles must not declare them, not even as `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_params: Option<RequestParams>,
    /// The profile selected when an invocation does not choose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<ModelProfileId>,
    /// Optional complete Profile order, lowest declared reasoning strength first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile_order: Option<Vec<ModelProfileId>>,
    /// The declared profiles. Presence is tracked: a declared collection
    /// must be nonempty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<BTreeMap<ModelProfileId, ModelProfileDocument>>,
    /// The bounded structural translation metadata.
    #[serde(default)]
    pub compat: ModelCompat,
}

/// Deserializes a map, rejecting duplicate keys instead of keeping the last.
fn deserialize_unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMap<V>(std::marker::PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueMap<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map with unique keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, V>()? {
                if out.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                }
                out.insert(key, value);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_map(UniqueMap(std::marker::PhantomData))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_schema_version(found: u32) -> Result<(), ModelCatalogError> {
    if found != MODEL_CATALOG_SCHEMA_VERSION {
        return Err(ModelCatalogError::UnsupportedSchemaVersion {
            supported: MODEL_CATALOG_SCHEMA_VERSION,
            found,
        });
    }
    Ok(())
}

fn validate_provider(
    id: &ProviderId,
    document: ProviderDocument,
) -> Result<ProviderDefinition, ModelCatalogError> {
    validate_base_url(id, &document.base_url)?;
    let api_key = document.api_key.clone();
    Ok(ProviderDefinition {
        id: id.clone(),
        protocol: document.protocol,
        base_url: document.base_url,
        api_key,
    })
}

/// Validates that the endpoint is an explicit absolute HTTP(S) URL.
///
/// The check is deliberately structural only: nothing about a hostname ever
/// changes runtime behaviour.
fn validate_base_url(provider: &ProviderId, base_url: &str) -> Result<(), ModelCatalogError> {
    let valid = url::Url::parse(base_url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && !url.cannot_be_a_base()
    });
    if !valid {
        return Err(ModelCatalogError::InvalidBaseUrl {
            provider: provider.clone(),
            base_url: base_url.to_owned(),
        });
    }
    Ok(())
}

// A source may inherit its Provider from the other scope. In that case only
// protocol-independent constraints are decidable here; composition supplies
// the Provider's protocol before any Model can become an invocation binding.
fn validate_model_fields(
    reference: &ModelRef,
    document: &ModelDocument,
    protocol: Option<ModelProtocol>,
) -> Result<ValidatedProfiles, ModelCatalogError> {
    ModelId::parse(document.id.clone())?;
    let reference = reference.clone();

    if document.context_window == 0 || document.max_output_tokens == 0 {
        return Err(ModelCatalogError::InvalidLimits {
            model: reference,
            detail: "context_window and max_output_tokens must both be positive".to_owned(),
        });
    }
    if u64::from(document.max_output_tokens) >= document.context_window {
        return Err(ModelCatalogError::InvalidLimits {
            model: reference,
            detail: format!(
                "max_output_tokens {} must be smaller than context_window {}",
                document.max_output_tokens, document.context_window
            ),
        });
    }
    if document.capabilities.input_modalities.is_empty()
        || document.capabilities.output_modalities.is_empty()
    {
        return Err(ModelCatalogError::InvalidCapabilities {
            model: reference,
            detail:
                "input_modalities and output_modalities must each declare at least one modality"
                    .to_owned(),
        });
    }
    if !document.capabilities.supports_text_conversation() {
        return Err(ModelCatalogError::InvalidCapabilities {
            model: reference,
            detail: "a rustX conversation model must declare text input and text output".to_owned(),
        });
    }

    if let Some(protocol) = protocol {
        validate_compat(&reference, protocol, document.compat)?;
    }

    let (default_profile, profiles) = validate_profiles(&reference, document, protocol)?;
    validate_reasoning_profile_order(&reference, document, &profiles)?;
    let empty_params = RequestParams::default();
    let request_params = document.request_params.as_ref().unwrap_or(&empty_params);
    if let Some(protocol) = protocol {
        validate_request_params_layer(request_params, protocol, RequestParamsLayer::ModelParams)
            .map_err(|collision| ModelCatalogError::ProtectedKey {
                model: reference.clone(),
                key: collision.key,
                layer: collision.layer,
            })?;
    }
    validate_request_params_numbers(request_params, RequestParamsLayer::ModelParams).map_err(
        |inexact| ModelCatalogError::InexactNumber {
            model: reference.clone(),
            layer: inexact.layer,
        },
    )?;

    Ok((default_profile, profiles))
}

fn resolve_model(
    reference: &ModelRef,
    document: ModelDocument,
    protocol: ModelProtocol,
) -> Result<ModelDefinition, ModelCatalogError> {
    let (default_profile, profiles) = validate_model_fields(reference, &document, Some(protocol))?;
    let id = ModelId::parse(document.id)?;
    Ok(ModelDefinition {
        provider: ProviderId::parse(document.provider)?,
        id,
        protocol,
        context_window: document.context_window,
        max_output_tokens: document.max_output_tokens,
        capabilities: document.capabilities,
        request_params: document.request_params.unwrap_or_default(),
        default_profile,
        reasoning_profile_order: document.reasoning_profile_order,
        profiles,
        compat: document.compat,
    })
}

type ValidatedProfiles = (
    Option<ModelProfileId>,
    BTreeMap<ModelProfileId, ModelProfile>,
);

/// Validates the Profile contract of one Model.
///
/// With profiles: a nonempty collection, a declared `default_profile`, no
/// model-level `request_params` (not even `{}`), and
/// positive profile budgets within the Model hard maximum. Without profiles:
/// no `default_profile`.
fn validate_profiles(
    reference: &ModelRef,
    document: &ModelDocument,
    protocol: Option<ModelProtocol>,
) -> Result<ValidatedProfiles, ModelCatalogError> {
    let invalid = |detail: String| ModelCatalogError::InvalidProfiles {
        model: reference.clone(),
        detail,
    };
    let Some(declared) = &document.profiles else {
        return match &document.default_profile {
            Some(default) => Err(invalid(format!(
                "default_profile {:?} is declared but the model declares no profiles",
                default.as_str()
            ))),
            None => Ok((None, BTreeMap::new())),
        };
    };
    if declared.is_empty() {
        return Err(invalid(
            "a declared profiles collection must declare at least one profile".to_owned(),
        ));
    }
    if document.request_params.is_some() {
        return Err(invalid(
            "a model with profiles must not declare model-level request_params; \
             each profile declares its complete request_params"
                .to_owned(),
        ));
    }
    let default = document
        .default_profile
        .clone()
        .ok_or_else(|| invalid("a model with profiles must declare default_profile".to_owned()))?;
    if !declared.contains_key(&default) {
        return Err(invalid(format!(
            "default_profile {:?} is not declared in profiles",
            default.as_str()
        )));
    }
    let mut profiles = BTreeMap::new();
    for (id, profile) in declared {
        if let Some(budget) = profile.max_output_tokens
            && (budget == 0 || budget > document.max_output_tokens)
        {
            return Err(invalid(format!(
                "profile {:?} max_output_tokens {budget} must be positive and at most the model max_output_tokens {}",
                id.as_str(),
                document.max_output_tokens
            )));
        }
        if let Some(protocol) = protocol {
            validate_request_params_layer(
                &profile.request_params,
                protocol,
                RequestParamsLayer::ModelProfile,
            )
            .map_err(|collision| ModelCatalogError::ProtectedKey {
                model: reference.clone(),
                key: collision.key,
                layer: collision.layer,
            })?;
        }
        validate_request_params_numbers(&profile.request_params, RequestParamsLayer::ModelProfile)
            .map_err(|inexact| ModelCatalogError::InexactNumber {
                model: reference.clone(),
                layer: inexact.layer,
            })?;
        profiles.insert(
            id.clone(),
            ModelProfile {
                max_output_tokens: profile.max_output_tokens,
                request_params: profile.request_params.clone(),
            },
        );
    }
    Ok((Some(default), profiles))
}

/// A declaration is a complete permutation, independent of default selection
/// and of the catalog's lexical ordinal indexes. Never infer or repair it.
fn validate_reasoning_profile_order(
    reference: &ModelRef,
    document: &ModelDocument,
    profiles: &BTreeMap<ModelProfileId, ModelProfile>,
) -> Result<(), ModelCatalogError> {
    let Some(order) = &document.reasoning_profile_order else {
        return Ok(());
    };
    let invalid = |detail| ModelCatalogError::InvalidReasoningProfileOrder {
        model: reference.clone(),
        detail,
    };
    if order.is_empty() || profiles.is_empty() {
        return Err(invalid(
            "reasoning_profile_order requires Profiles and a nonempty complete order".to_owned(),
        ));
    }
    let mut seen = BTreeSet::new();
    for id in order {
        ModelProfileId::parse(id.as_str())?;
        if !profiles.contains_key(id) {
            return Err(invalid(format!(
                "reasoning_profile_order names unknown Profile {:?}; use only this Model's Profiles",
                id.as_str()
            )));
        }
        if !seen.insert(id) {
            return Err(invalid(format!(
                "reasoning_profile_order repeats Profile {:?}; list each Profile exactly once",
                id.as_str()
            )));
        }
    }
    let missing: Vec<_> = profiles
        .keys()
        .filter(|id| !seen.contains(id))
        .map(ModelProfileId::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "reasoning_profile_order is missing Profiles {missing:?}; list every Profile"
        )));
    }
    Ok(())
}

fn validate_compat(
    reference: &ModelRef,
    protocol: ModelProtocol,
    compat: ModelCompat,
) -> Result<(), ModelCatalogError> {
    let invalid_detail = match protocol {
        ModelProtocol::OpenAiChatCompletions if compat.responses_storage_is_explicit() => Some(
            format!("compat declares fields that do not apply to protocol {protocol:?}"),
        ),
        ModelProtocol::OpenAiResponses
            if compat.chat_max_tokens_field_is_explicit()
                || compat.chat_stream_usage_is_explicit()
                || compat.chat_reasoning_replay_is_explicit()
                || compat.chat_tool_protocol_is_explicit() =>
        {
            Some(format!(
                "compat declares fields that do not apply to protocol {protocol:?}"
            ))
        }
        ModelProtocol::AnthropicMessages
            if compat.chat_max_tokens_field_is_explicit()
                || compat.chat_stream_usage_is_explicit()
                || compat.chat_reasoning_replay_is_explicit()
                || compat.chat_tool_protocol_is_explicit()
                || compat.responses_storage_is_explicit() =>
        {
            Some(format!(
                "compat declares fields that do not apply to protocol {protocol:?}"
            ))
        }
        ModelProtocol::OpenAiChatCompletions
        | ModelProtocol::OpenAiResponses
        | ModelProtocol::AnthropicMessages => None,
    };
    if let Some(detail) = invalid_detail {
        return Err(ModelCatalogError::InvalidCompat {
            model: reference.clone(),
            detail,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Borrowed credential-free catalog encoding. The authoritative immutable
/// catalog remains the only owner of growing profile/model identities.
pub(crate) struct ModelCatalogReadView<'a>(&'a ModelCatalog);
impl ModelCatalog {
    pub(crate) fn model_count(&self) -> usize {
        self.model_ordinals.len()
    }
    pub(crate) fn model_at(&self, ordinal: usize) -> Option<CatalogModelReadView<'_>> {
        let reference = self.model_ordinals.get(ordinal)?;
        let model = self.models.get(reference)?;
        let provider = self.providers.get(&model.provider)?;
        Some(CatalogModelReadView {
            model: reference,
            protocol: model.protocol,
            context_window: model.context_window,
            max_output_tokens: model.max_output_tokens,
            declared_capabilities: &model.capabilities,
            effective_capabilities: super::invocation::effective_capabilities(
                &model.capabilities,
                model.protocol,
            ),
            profiles: ProfilesReadView::Native(&model.profiles),
            default_profile: model.default_profile.as_ref(),
            reasoning_profile_order: model.reasoning_profile_order.as_deref(),
            credential_source: provider.api_key.read_view(),
        })
    }
    pub(crate) fn profile_count(&self, ordinal: usize) -> Option<usize> {
        self.profile_ordinals.get(ordinal).map(|keys| keys.len())
    }
    pub(crate) fn profile_at(
        &self,
        model_ordinal: usize,
        ordinal: usize,
    ) -> Option<CatalogProfileReadView<'_>> {
        let id = self.profile_ordinals.get(model_ordinal)?.get(ordinal)?;
        let model = self.models.get(self.model_ordinals.get(model_ordinal)?)?;
        let profile = model.profiles.get(id)?;
        Some(CatalogProfileReadView {
            id,
            max_output_tokens: profile.max_output_tokens,
        })
    }
    pub(crate) fn read_view(&self) -> ModelCatalogReadView<'_> {
        ModelCatalogReadView(self)
    }
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CredentialReadView<'a> {
    Literal,
    Environment { variable: &'a str },
}
impl CredentialSource {
    pub(crate) fn read_view(&self) -> CredentialReadView<'_> {
        match self {
            Self::Literal(_) => CredentialReadView::Literal,
            Self::Environment(variable) => CredentialReadView::Environment { variable },
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogProfileReadView<'a> {
    pub id: &'a ModelProfileId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}
pub(crate) enum ProfilesReadView<'a> {
    Header(Vec<CatalogProfileReadView<'a>>),
    Native(&'a BTreeMap<ModelProfileId, ModelProfile>),
    Frozen(Option<&'a ModelProfileId>),
}
impl Serialize for ProfilesReadView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Profile<'a> {
            id: &'a ModelProfileId,
            #[serde(skip_serializing_if = "Option::is_none")]
            max_output_tokens: Option<u32>,
        }
        match self {
            Self::Header(profiles) => profiles.serialize(serializer),
            Self::Native(profiles) => {
                let mut seq = serializer.serialize_seq(Some(profiles.len()))?;
                for (id, profile) in *profiles {
                    seq.serialize_element(&Profile {
                        id,
                        max_output_tokens: profile.max_output_tokens,
                    })?;
                }
                seq.end()
            }
            Self::Frozen(profile) => {
                let mut seq = serializer.serialize_seq(Some(usize::from(profile.is_some())))?;
                if let Some(id) = profile {
                    seq.serialize_element(&Profile {
                        id,
                        max_output_tokens: None,
                    })?;
                }
                seq.end()
            }
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogModelReadView<'a> {
    pub model: &'a ModelRef,
    pub protocol: ModelProtocol,
    pub context_window: u64,
    pub max_output_tokens: u32,
    pub declared_capabilities: &'a ModelCapabilities,
    pub effective_capabilities: ModelCapabilities,
    pub profiles: ProfilesReadView<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<&'a ModelProfileId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_profile_order: Option<&'a [ModelProfileId]>,
    pub credential_source: CredentialReadView<'a>,
}
/// Finite native collection header. Deferred profiles are explicitly identified.
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeCatalogModelHeader<'a> {
    #[serde(flatten)]
    pub model: CatalogModelReadView<'a>,
    pub profile_count: u64,
    pub profiles_deferred: bool,
}
impl schemars::JsonSchema for CatalogModelReadView<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CatalogModelReadView".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<CatalogModelView>()
    }
}
impl schemars::JsonSchema for CatalogProfileReadView<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CatalogProfileReadView".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<ModelProfileView>()
    }
}
impl Serialize for ModelCatalogReadView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeSeq, SerializeStruct};
        struct Models<'a>(&'a ModelCatalog);
        impl Serialize for Models<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(Some(self.0.models.len()))?;
                for (reference, model) in &self.0.models {
                    let provider = &self.0.providers[&model.provider];
                    seq.serialize_element(&CatalogModelReadView {
                        model: reference,
                        protocol: model.protocol,
                        context_window: model.context_window,
                        max_output_tokens: model.max_output_tokens,
                        declared_capabilities: &model.capabilities,
                        effective_capabilities: super::invocation::effective_capabilities(
                            &model.capabilities,
                            model.protocol,
                        ),
                        profiles: ProfilesReadView::Native(&model.profiles),
                        default_profile: model.default_profile.as_ref(),
                        reasoning_profile_order: model.reasoning_profile_order.as_deref(),
                        credential_source: provider.api_key.read_view(),
                    })?;
                }
                seq.end()
            }
        }
        let mut view = serializer.serialize_struct("ModelCatalogView", 1)?;
        view.serialize_field("models", &Models(self.0))?;
        view.end()
    }
}

/// The safe public catalog view served to Runtime Clients.
///
/// A client selects a model and a Model Profile from this view; it never
/// reads `rustx.toml` itself and never sees a credential, an adapter, or a
/// provider HTTP client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCatalogView {
    /// Every selectable model in deterministic reference order.
    #[serde(default)]
    pub models: Vec<CatalogModelView>,
}

/// One selectable model of the public catalog view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct CatalogModelView {
    /// The fully qualified model reference.
    pub model: ModelRef,
    /// The protocol an adapter speaks to this model.
    pub protocol: ModelProtocol,
    /// The model context window in tokens.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The capabilities the catalog claims.
    pub declared_capabilities: ModelCapabilities,
    /// The capabilities the runtime can actually deliver today.
    pub effective_capabilities: ModelCapabilities,
    /// The declared Model Profiles in deterministic order.
    #[serde(default)]
    pub profiles: Vec<ModelProfileView>,
    /// The profile selected when an invocation does not choose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<ModelProfileId>,
    /// Optional complete Profile order, lowest declared reasoning strength first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile_order: Option<Vec<ModelProfileId>>,
    /// The redacted credential source of the model's provider.
    pub credential_source: CredentialSourceView,
}

/// One selectable Model Profile of the public catalog view.
///
/// The profile's provider request parameters are provider-owned wire
/// configuration a client never needs to select a profile; the runtime
/// semantics it carries are exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelProfileView {
    /// The profile identity.
    pub id: ModelProfileId,
    /// The profile's default output budget, when it declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn document() -> Value {
        json!({
            "providers": {"p": {"protocol":"openai_chat_completions", "baseUrl":"https://gateway.example/v1", "apiKey":"$RUSTX_KEY"}},
            "models": {"chosen": {
                "provider":"p", "id":"wire/model",
                "contextWindow":1000, "maxOutputTokens":100,
                "capabilities":{"inputModalities":["text"],"outputModalities":["text"]},
                "compat":{"chatReasoningReplay":"omit"}
            }}
        })
    }
    fn catalog(value: Value) -> Result<ModelCatalog, ModelCatalogError> {
        let doc = serde_json::from_value(value).map_err(|e| ModelCatalogError::Syntax {
            detail: e.to_string(),
        })?;
        ModelCatalog::from_document(doc)
    }
    fn selected() -> ModelRef {
        ModelRef::parse("chosen").unwrap()
    }
    fn registry(value: Value) -> crate::model::invocation::ModelBindingRegistry {
        crate::model::invocation::ModelBindingRegistry::new(
            catalog(value)
                .unwrap()
                .resolve(&MapCredentialEnvironment::new([(
                    "RUSTX_KEY".into(),
                    "fixture-key".into(),
                )]))
                .unwrap(),
        )
        .unwrap()
    }
    fn native_catalog_with_large_profile() -> (ModelCatalog, String) {
        let mut value = document();
        let model = value["models"]["chosen"].clone();
        let rows = value["models"].as_object_mut().unwrap();
        rows.clear();
        for ordinal in 0..73 {
            rows.insert(format!("model-{ordinal:03}"), model.clone());
        }
        let enormous = format!("a{}", "x".repeat(2_100_000));
        let mut profiles = serde_json::Map::new();
        profiles.insert(enormous.clone(), json!({}));
        for ordinal in 0..72 {
            profiles.insert(format!("profile-{ordinal:03}"), json!({}));
        }
        value["models"]["model-000"]["profiles"] = Value::Object(profiles);
        value["models"]["model-000"]["defaultProfile"] = enormous.clone().into();
        let catalog = catalog(value).unwrap();
        (catalog, enormous)
    }

    #[test]
    fn t15_native_catalog_ordinals_preserve_identity_and_advance_past_large_profiles() {
        let (catalog, enormous) = native_catalog_with_large_profile();
        assert_eq!(catalog.model_count(), 73);
        assert_eq!(catalog.model_at(72).unwrap().model.as_str(), "model-072");
        assert!(catalog.model_at(73).is_none());
        assert_eq!(catalog.profile_count(0), Some(73));
        assert_eq!(catalog.profile_at(0, 0).unwrap().id.as_str(), enormous);
        assert_eq!(
            catalog.profile_at(0, 72).unwrap().id.as_str(),
            "profile-071"
        );
        let key = catalog.models.keys().next().unwrap();
        assert!(Arc::ptr_eq(&key.0, &catalog.model_ordinals[0].0));
        let profile_key = catalog.models[key].profiles.keys().next().unwrap();
        assert!(Arc::ptr_eq(
            &profile_key.0,
            &catalog.profile_ordinals[0][0].0
        ));
        assert!(
            crate::observation::bounded::bounded_item_bytes(&catalog.model_at(0).unwrap(), 16_384)
                .is_none()
        );
        assert!(
            crate::observation::bounded::bounded_item_bytes(
                &catalog.profile_at(0, 72).unwrap(),
                16_384
            )
            .is_some()
        );
        let state = crate::model::session::SessionModelState::new(
            crate::model::invocation::ModelBindingRegistry::new(
                catalog
                    .resolve(&MapCredentialEnvironment::new([(
                        "RUSTX_KEY".into(),
                        "fixture-key".into(),
                    )]))
                    .unwrap(),
            )
            .unwrap(),
            crate::model::session::SessionModelConfig::of(ModelRef::parse("model-001").unwrap()),
        )
        .unwrap();
        let source = crate::runtime_client::domain::DomainSource::Catalog(Arc::new(
            crate::runtime_client::domain::SourceValue::new(Arc::new(
                state.read_source().catalog_source(),
            )),
        ));
        let cut = crate::observation::cut::CheckpointReadCut {
            canonical_foreground: None,
            live_publication: None,
            conversation_id: crate::runtime::identity::ConversationId::new(
                "conv_01900000-0000-7000-8000-000000000011",
            ),
            activation_id: None,
            revision: 0,
            through: 0,
            source_cut: None,
            domain_revisions: crate::observation::metadata::MetadataFragment::default(),
            collection_cuts: crate::observation::metadata::MetadataFragment::default(),
        };
        for (offset, next) in [(0, Some(32)), (32, Some(64)), (64, None)] {
            let page = source
                .page(
                    &cut,
                    "model_catalog.models",
                    &["models".into()],
                    offset,
                    32,
                    false,
                )
                .unwrap();
            assert_eq!(page.total, 73);
            assert_eq!(page.next_offset, next);
            assert_eq!(page.items.first().unwrap().offset, offset);
            assert!(rustx_types::observation::bounded::bounded_bytes(&page, 786_432).is_some());
            if offset == 0 {
                assert!(page.items[0].deferred);
                assert!(page.items[1].value.is_some());
            }
        }
        let page = source
            .page(
                &cut,
                "model_catalog.models.@0.profiles",
                &["models".into(), "@0".into(), "profiles".into()],
                0,
                32,
                false,
            )
            .unwrap();
        assert_eq!(page.total, 73);
        assert_eq!(page.next_offset, Some(32));
        assert!(page.items[0].deferred);
        assert!(page.items[1].value.is_some());
    }

    #[test]
    fn provider_and_model_names_never_supply_semantics() {
        let mut value = document();
        let p = value["providers"]
            .as_object_mut()
            .unwrap()
            .remove("p")
            .unwrap();
        value["providers"]["openai"] = p;
        value["models"]["chosen"]["provider"] = "openai".into();
        let catalog = catalog(value).unwrap();
        assert_eq!(
            catalog.providers().next().unwrap().base_url,
            "https://gateway.example/v1"
        );
        assert_eq!(
            catalog.model(&selected()).unwrap().id.as_str(),
            "wire/model"
        );
        assert!(matches!(
            catalog.model(&ModelRef::parse("openai/wire/model").unwrap()),
            Err(ModelCatalogError::UnknownModel { .. })
        ));
    }
    #[test]
    fn provider_required_fields_and_unknown_fields_fail() {
        for field in ["protocol", "baseUrl", "apiKey"] {
            let mut value = document();
            value["providers"]["p"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::Syntax { .. })
            ));
        }
        let mut value = document();
        value["models"]["chosen"]["protocol"] = "openai_responses".into();
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
        for pointer in [
            "/providers/p",
            "/models/chosen",
            "/models/chosen/capabilities",
            "/models/chosen/compat",
        ] {
            let mut value = document();
            value.pointer_mut(pointer).unwrap()["unknown"] = true.into();
            assert!(catalog(value).is_err(), "{pointer}");
        }
        let mut value = document();
        value["providers"]["p"]["models"] = json!([]);
        assert!(
            matches!(catalog(value), Err(ModelCatalogError::Syntax { .. })),
            "Providers cannot contain Models"
        );
    }

    #[test]
    fn every_model_inherits_its_providers_protocol() {
        let mut value = document();
        value["models"]["chosen"]["compat"] = json!({});
        value["models"]["second"] = value["models"]["chosen"].clone();
        value["models"]["second"]["id"] = "another-model".into();
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            value["providers"]["p"]["protocol"] = json!(protocol);
            let resolved = catalog(value.clone()).unwrap();
            assert_eq!(resolved.providers().next().unwrap().protocol, protocol);
            for model in resolved.model_refs() {
                assert_eq!(resolved.model(&model).unwrap().protocol, protocol);
            }
        }
    }
    #[test]
    fn invalid_base_url_fails() {
        for url in [
            "",
            "gateway.example",
            "ftp://x/y",
            "https://",
            "https://u:p@example.com/v1",
            "https://example.com/v1?x=1",
            "https://example.com/v1#fragment",
            "https:// example.com",
        ] {
            let mut value = document();
            value["providers"]["p"]["baseUrl"] = url.into();
            assert!(
                matches!(
                    catalog(value),
                    Err(ModelCatalogError::InvalidBaseUrl { .. })
                ),
                "{url}"
            );
        }
    }
    #[test]
    fn independent_model_provider_reference_is_required() {
        let mut value = document();
        value["models"]["chosen"]["provider"] = "missing".into();
        assert!(catalog(value).is_err());
        let mut value = document();
        value["models"]["chosen"]
            .as_object_mut()
            .unwrap()
            .remove("provider");
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
    }
    #[test]
    fn environment_credentials_resolve_lazily_or_fail_without_disclosing_values() {
        let catalog = catalog(document()).unwrap();
        let empty = catalog
            .resolve(&MapCredentialEnvironment::default())
            .unwrap();
        let error = empty
            .provider(&ProviderId::new("p"))
            .unwrap()
            .credential()
            .unwrap_err();
        assert!(matches!(
            error,
            ModelCatalogError::MissingEnvironmentCredential { .. }
        ));
        assert!(error.to_string().contains("RUSTX_KEY"));
        let resolved = catalog
            .resolve(&MapCredentialEnvironment::new([(
                "RUSTX_KEY".into(),
                "sk-do-not-print".into(),
            )]))
            .unwrap();
        let provider = resolved.provider(&ProviderId::new("p")).unwrap();
        assert_eq!(provider.credential().unwrap().expose(), "sk-do-not-print");
        assert_eq!(
            provider.credential_source(),
            CredentialSourceView::Environment {
                variable: "RUSTX_KEY".into()
            }
        );
        assert!(!format!("{resolved:?}").contains("sk-do-not-print"));
        let secret = ResolvedCredential::new("sk-do-not-print");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.to_string(), "<redacted>");
        assert!(
            !format!("{:?}", CredentialSource::Literal("sk-do-not-print".into()))
                .contains("sk-do-not-print")
        );
    }
    #[test]
    fn invalid_protocol_and_limits_fail() {
        let mut value = document();
        value["providers"]["p"]["protocol"] = "guessed".into();
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
        for (window, output) in [(0, 100), (100, 0), (100, 101)] {
            let mut value = document();
            value["models"]["chosen"]["contextWindow"] = window.into();
            value["models"]["chosen"]["maxOutputTokens"] = output.into();
            assert!(catalog(value).is_err());
        }
    }
    #[test]
    fn malformed_capabilities_fail() {
        for capabilities in [
            json!({"inputModalities":[],"outputModalities":["text"]}),
            json!({"inputModalities":["text"],"outputModalities":[]}),
        ] {
            let mut value = document();
            value["models"]["chosen"]["capabilities"] = capabilities;
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::InvalidCapabilities { .. })
            ));
        }
    }
    #[test]
    fn model_references_are_independent_names_with_nonempty_segments() {
        for name in ["chosen", "a/b", "a/b/c"] {
            assert!(ModelRef::parse(name).is_ok());
        }
        for name in ["", "a/", "/b", "a//b", "a/b/", "white space"] {
            assert!(ModelRef::parse(name).is_err());
        }
        assert!(catalog(document()).unwrap().model(&selected()).is_ok());
        assert!(matches!(
            catalog(document())
                .unwrap()
                .model(&ModelRef::parse("missing").unwrap()),
            Err(ModelCatalogError::UnknownModel { .. })
        ));
    }
    #[test]
    fn chat_reasoning_replay_is_optional_and_overrides_round_trip() {
        let mut value = document();
        value["models"]["chosen"]["compat"] = json!({});
        assert!(catalog(value).is_ok());
        for replay in ["reasoning", "reasoning_content", "omit"] {
            let mut value = document();
            value["models"]["chosen"]["compat"]["chatReasoningReplay"] = replay.into();
            let doc: ModelCatalogDocument = serde_json::from_value(value).unwrap();
            assert!(catalog(serde_json::to_value(doc).unwrap()).is_ok());
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // endpoint matrix also verifies frozen overrides
    fn built_in_chat_policy_is_frozen_and_explicit_overrides_win() {
        use crate::model::invocation::ModelSelection;
        let cases = [
            ("https://api.deepseek.com", "deepseek-flash", "max_tokens"),
            (
                "https://api.deepseek.com/v1/",
                "deepseek-v4-pro",
                "max_tokens",
            ),
            ("https://api.z.ai/api/paas/v4", "GLM-5.3", "max_tokens"),
            (
                "https://api.z.ai/api/coding/paas/v4/",
                "glm-5.3",
                "max_tokens",
            ),
            (
                "https://open.bigmodel.cn/api/paas/v4",
                "glm-5.3",
                "max_tokens",
            ),
            ("https://api.moonshot.cn/v1", "kimi-k3", "max_tokens"),
            ("https://api.kimi.ai/coding/v1", "k3", "max_tokens"),
            (
                "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
                "qwen3.8-max",
                "max_tokens",
            ),
            (
                "https://coding.dashscope.aliyuncs.com/v1",
                "qwen3.7-plus",
                "max_tokens",
            ),
            (
                "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/",
                "qwen4-max",
                "max_tokens",
            ),
            (
                "https://api.deepseek.com.evil.example/v1",
                "deepseek-flash",
                "max_completion_tokens",
            ),
            (
                "https://api.z.ai/unrelated/v1",
                "glm-5.3",
                "max_completion_tokens",
            ),
            (
                "http://localhost:8000/v1",
                "vllm-model",
                "max_completion_tokens",
            ),
            (
                "https://service.example/v1",
                "deepseek-v4-flash",
                "max_completion_tokens",
            ),
            (
                "https://opencode.ai/zen/go/v1",
                "deepseek-v4-flash",
                "max_tokens",
            ),
            (
                "https://opencode.ai/zen/go/v1/",
                "glm-5.3-flash",
                "max_tokens",
            ),
            (
                "https://opencode.ai/zen/go/v1",
                "unlisted-model",
                "max_completion_tokens",
            ),
            (
                "https://opencode.ai/zen/v1",
                "deepseek-v4-flash",
                "max_completion_tokens",
            ),
            (
                "https://opencode.ai.evil.example/zen/go/v1",
                "deepseek-v4-flash",
                "max_completion_tokens",
            ),
        ];
        for (base_url, model, field) in cases {
            let mut value = document();
            value["providers"]["p"]["baseUrl"] = base_url.into();
            value["models"]["chosen"]["id"] = model.into();
            value["models"]["chosen"]
                .as_object_mut()
                .unwrap()
                .remove("compat");
            let frozen = registry(value.clone())
                .freeze(&ModelSelection::of(selected()))
                .unwrap();
            assert_eq!(
                frozen.compat.chat_max_tokens_field.wire_name(),
                field,
                "{base_url} {model}"
            );
            assert_eq!(
                frozen.compat.chat_reasoning_replay,
                Some(ChatReasoningReplay::ReasoningContent)
            );
            assert_eq!(frozen.compat.chat_stream_usage, ChatStreamUsage::Supported);
            assert_eq!(frozen.compat.chat_tool_protocol, ChatToolProtocol::Native);
            let restored: crate::model::frozen::FrozenModelInvocation =
                serde_json::from_value(serde_json::to_value(&frozen).unwrap()).unwrap();
            assert_eq!(restored.compat, frozen.compat);

            value["models"]["chosen"]["compat"] = json!({
                "chatMaxTokensField":"max_completion_tokens",
                "chatReasoningReplay":"omit",
                "chatStreamUsage":"unsupported",
                "chatToolProtocol":"qwen_xml"
            });
            let explicit = registry(value)
                .freeze(&ModelSelection::of(selected()))
                .unwrap();
            assert_eq!(
                explicit.compat.chat_max_tokens_field,
                ChatMaxTokensField::MaxCompletionTokens
            );
            assert_eq!(
                explicit.compat.chat_reasoning_replay,
                Some(ChatReasoningReplay::Omit)
            );
            assert_eq!(
                explicit.compat.chat_stream_usage,
                ChatStreamUsage::Unsupported
            );
            assert_eq!(
                explicit.compat.chat_tool_protocol,
                ChatToolProtocol::QwenXml
            );
        }
    }

    #[test]
    fn responses_storage_defaults_follow_endpoint_and_preserve_overrides() {
        use crate::model::invocation::ModelSelection;
        for (endpoint, expected) in [
            ("https://api.deepseek.com", ResponsesStorageMode::Stateless),
            (
                "https://api.deepseek.com/v1/",
                ResponsesStorageMode::Stateless,
            ),
            ("https://api.x.ai/v1", ResponsesStorageMode::Stored),
            ("http://localhost:8000/v1", ResponsesStorageMode::Stored),
            (
                "https://api.deepseek.com.evil.example",
                ResponsesStorageMode::Stored,
            ),
        ] {
            let mut value = document();
            value["providers"]["p"]["protocol"] = "openai_responses".into();
            value["providers"]["p"]["baseUrl"] = endpoint.into();
            value["models"]["chosen"]
                .as_object_mut()
                .unwrap()
                .remove("compat");
            let frozen = registry(value.clone())
                .freeze(&ModelSelection::of(selected()))
                .unwrap();
            assert_eq!(frozen.compat.responses_storage, expected, "{endpoint}");
            let restored: crate::model::frozen::FrozenModelInvocation =
                serde_json::from_value(serde_json::to_value(&frozen).unwrap()).unwrap();
            assert_eq!(restored.compat, frozen.compat);
            for (mode, expected) in [
                ("stored", ResponsesStorageMode::Stored),
                ("stateless", ResponsesStorageMode::Stateless),
            ] {
                value["models"]["chosen"]["compat"] = json!({"responsesStorage": mode});
                let frozen = registry(value.clone())
                    .freeze(&ModelSelection::of(selected()))
                    .unwrap();
                assert_eq!(frozen.compat.responses_storage, expected);
            }
        }
    }

    #[test]
    fn authored_and_omitted_protocol_defaults_have_the_same_invocation() {
        let mut implicit = document();
        implicit["models"]["chosen"]
            .as_object_mut()
            .unwrap()
            .remove("compat");
        let mut explicit = implicit.clone();
        explicit["models"]["chosen"]["compat"] = json!({
            "chatMaxTokensField":"max_completion_tokens",
            "chatReasoningReplay":"reasoning_content",
            "chatStreamUsage":"supported",
            "chatToolProtocol":"native"
        });
        let selection = ModelSelection::of(selected());
        assert!(catalog(implicit).unwrap().same_invocation(
            &selection,
            &catalog(explicit).unwrap(),
            &selection,
            RequestParamsLayer::SessionOverrides
        ));
    }
    #[test]
    fn protocol_specific_compat_rejects_foreign_even_default_valued_members() {
        for protocol in ["openai_responses", "anthropic_messages"] {
            for (field, setting) in [
                ("chatReasoningReplay", "omit"),
                ("chatToolProtocol", "native"),
                ("chatMaxTokensField", "max_completion_tokens"),
            ] {
                let mut value = document();
                value["providers"]["p"]["protocol"] = protocol.into();
                value["models"]["chosen"]["compat"] = json!({field:setting});
                assert!(
                    matches!(catalog(value), Err(ModelCatalogError::InvalidCompat { .. })),
                    "{protocol} {field}"
                );
            }
        }
        let mut value = document();
        value["models"]["chosen"]["compat"]["responsesStorage"] = "stored".into();
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::InvalidCompat { .. })
        ));
    }
    #[test]
    fn absent_compat_never_serializes_foreign_defaults() {
        let mut value = document();
        value["providers"]["p"]["protocol"] = "anthropic_messages".into();
        value["models"]["chosen"]["compat"] = json!({});
        let doc: ModelCatalogDocument = serde_json::from_value(value).unwrap();
        let value = serde_json::to_value(doc).unwrap();
        assert_eq!(value["models"]["chosen"]["compat"], json!({}));
        assert!(catalog(value).is_ok());
    }
    #[test]
    fn chat_tool_protocol_defaults_and_explicit_variants() {
        let default = catalog(document()).unwrap();
        assert_eq!(
            default
                .model(&selected())
                .unwrap()
                .compat
                .chat_tool_protocol,
            ChatToolProtocol::Native
        );
        for (name, expected) in [
            ("native", ChatToolProtocol::Native),
            ("qwen_xml", ChatToolProtocol::QwenXml),
        ] {
            let mut value = document();
            value["models"]["chosen"]["compat"]["chatToolProtocol"] = name.into();
            assert_eq!(
                catalog(value)
                    .unwrap()
                    .model(&selected())
                    .unwrap()
                    .compat
                    .chat_tool_protocol,
                expected
            );
        }
    }
    fn profiled(reasoning: bool) -> Value {
        let mut value = document();
        let model = &mut value["models"]["chosen"];
        model["defaultProfile"] = "balanced".into();
        model["profiles"] = if reasoning {
            json!({
                "fast": {"maxOutputTokens": 20, "requestParams": {"reasoning_effort": "low"}},
                "balanced": {"requestParams": {"reasoning_effort": "medium", "opaque": [null]}},
                "off": {"requestParams": {"thinking": {"type": "disabled"}}}
            })
        } else {
            json!({
                "precise": {"maxOutputTokens": 40, "requestParams": {"temperature": 0.1, "top_p": 0.5}},
                "balanced": {"requestParams": {"temperature": 0.7}},
                "creative": {"requestParams": {"temperature": 1.2, "stop": ["\n\n", null]}}
            })
        };
        value
    }
    fn invalid_profiles(value: Value) -> String {
        match catalog(value) {
            Err(ModelCatalogError::InvalidProfiles { detail, .. }) => detail,
            other => panic!("expected InvalidProfiles, got {other:?}"),
        }
    }
    #[test]
    #[allow(clippy::too_many_lines)] // complete accepted/rejected permutation matrix
    fn reasoning_order_is_optional_or_a_complete_model_local_permutation() {
        for value in [document(), profiled(true), profiled(false)] {
            let catalog = catalog(value).unwrap();
            assert!(
                catalog
                    .model(&selected())
                    .unwrap()
                    .reasoning_profile_order
                    .is_none()
            );
            assert!(
                serde_json::to_value(catalog.view()).unwrap()["models"][0]
                    .get("reasoningProfileOrder")
                    .is_none()
            );
        }
        for order in [
            json!(["off", "fast", "balanced"]),
            json!(["balanced", "off", "fast"]),
        ] {
            let mut value = profiled(true);
            value["models"]["chosen"]["reasoningProfileOrder"] = order.clone();
            let catalog = catalog(value).unwrap();
            assert_eq!(
                serde_json::to_value(catalog.view()).unwrap()["models"][0]["reasoningProfileOrder"],
                order
            );
            assert_eq!(
                serde_json::to_value(catalog.read_view()).unwrap(),
                serde_json::to_value(catalog.view()).unwrap()
            );
            assert_eq!(
                serde_json::to_value(catalog.model_at(0).unwrap()).unwrap(),
                serde_json::to_value(&catalog.view().models[0]).unwrap()
            );
            assert_eq!(
                (0..3)
                    .map(|i| catalog.profile_at(0, i).unwrap().id.as_str())
                    .collect::<Vec<_>>(),
                ["balanced", "fast", "off"]
            );
            assert_eq!(
                catalog
                    .model(&selected())
                    .unwrap()
                    .default_profile
                    .as_ref()
                    .unwrap()
                    .as_str(),
                "balanced"
            );
        }
        let mut singleton = profiled(true);
        singleton["models"]["chosen"]["profiles"] = json!({"balanced": {}});
        singleton["models"]["chosen"]["reasoningProfileOrder"] = json!(["balanced"]);
        assert!(catalog(singleton).is_ok());
        for (order, expected) in [
            (json!([]), "nonempty"),
            (json!(["off", "fast"]), "missing Profiles [\"balanced\"]"),
            (json!(["off", "off", "balanced"]), "repeats Profile \"off\""),
            (
                json!(["off", "foreign", "balanced"]),
                "unknown Profile \"foreign\"",
            ),
        ] {
            let mut value = profiled(true);
            // An identity in a different Model does not belong to this one.
            let mut other = value["models"]["chosen"].clone();
            other["profiles"] = json!({"foreign": {}});
            other["defaultProfile"] = json!("foreign");
            value["models"]["other"] = other;
            value["models"]["chosen"]["reasoningProfileOrder"] = order;
            let error = catalog(value).unwrap_err();
            assert!(matches!(
                error,
                ModelCatalogError::InvalidReasoningProfileOrder { .. }
            ));
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(
                error.classification(),
                (
                    "models.*.reasoning_profile_order",
                    "invalid Model reasoning Profile order"
                )
            );
        }
        for order in [json!([]), json!(["off"])] {
            let mut value = document();
            value["models"]["chosen"]["reasoningProfileOrder"] = order;
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::InvalidReasoningProfileOrder { .. })
            ));
        }
        let mut doc: ModelCatalogDocument = serde_json::from_value(profiled(true)).unwrap();
        doc.models
            .get_mut("chosen")
            .unwrap()
            .reasoning_profile_order = Some(vec![ModelProfileId::new("bad/id")]);
        assert!(matches!(
            ModelCatalog::from_document(doc),
            Err(ModelCatalogError::InvalidIdentity { .. })
        ));
    }
    #[test]
    fn profiles_are_validated_complete_presets() {
        for reasoning in [true, false] {
            let catalog = catalog(profiled(reasoning)).unwrap();
            let model = catalog.model(&selected()).unwrap();
            assert_eq!(model.default_profile.as_ref().unwrap().as_str(), "balanced");
            assert_eq!(model.profiles.len(), 3);
            assert!(model.request_params.is_empty());
            let view = &catalog.view().models[0];
            assert_eq!(view.default_profile, model.default_profile);
            assert_eq!(
                view.profiles
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>(),
                if reasoning {
                    ["balanced", "fast", "off"]
                } else {
                    ["balanced", "creative", "precise"]
                }
            );
        }
        let catalog = catalog(profiled(false)).unwrap();
        let model = catalog.model(&selected()).unwrap();
        // No profile reads another's parameters.
        assert_eq!(
            Value::Object(
                model
                    .profile(&ModelProfileId::new("creative"))
                    .unwrap()
                    .request_params
                    .clone()
            ),
            json!({"temperature": 1.2, "stop": ["\n\n", null]})
        );
        assert_eq!(
            model
                .profile(&ModelProfileId::new("precise"))
                .unwrap()
                .max_output_tokens,
            Some(40)
        );
    }
    #[test]
    fn profile_structure_violations_fail_deterministically() {
        type Mutation = fn(&mut Value);
        let cases: [(&str, Mutation); 8] = [
            ("default_profile \"missing\" is not declared", |m| {
                m["defaultProfile"] = "missing".into();
            }),
            ("must declare default_profile", |m| {
                m.as_object_mut().unwrap().remove("defaultProfile");
            }),
            ("at least one profile", |m| m["profiles"] = json!({})),
            ("must not declare model-level request_params", |m| {
                m["requestParams"] = json!({"temperature": 0.2});
            }),
            ("must not declare model-level request_params", |m| {
                m["requestParams"] = json!({});
            }),
            ("max_output_tokens 0 must be positive", |m| {
                m["profiles"]["fast"]["maxOutputTokens"] = 0.into();
            }),
            (
                "max_output_tokens 101 must be positive and at most the model max_output_tokens 100",
                |m| {
                    m["profiles"]["fast"]["maxOutputTokens"] = 101.into();
                },
            ),
            ("declares no profiles", |m| {
                m.as_object_mut().unwrap().remove("profiles");
            }),
        ];
        for (expected, mutate) in cases {
            let mut value = profiled(true);
            mutate(&mut value["models"]["chosen"]);
            let detail = invalid_profiles(value);
            assert!(detail.contains(expected), "{expected}: {detail}");
        }
        // Profile identities are validated opaque names.
        for name in ["", "a b", "a/b"] {
            let mut value = profiled(false);
            value["models"]["chosen"]["profiles"][name] = json!({});
            assert!(
                matches!(catalog(value), Err(ModelCatalogError::Syntax { .. })),
                "{name:?}"
            );
        }
        // The obsolete reasoning block is an unknown field.
        let mut value = document();
        value["models"]["chosen"]["reasoning"] =
            json!({"defaultProfile":"off","profiles":{"off":{"enabled":false}}});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
    }
    #[test]
    fn model_params_and_profile_params_cannot_replace_protected_structure() {
        let mut value = document();
        value["models"]["chosen"]["requestParams"] = json!({"messages":[]});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::ProtectedKey {
                layer: RequestParamsLayer::ModelParams,
                ..
            })
        ));
        for key in [
            "messages",
            "max_tokens",
            "max_completion_tokens",
            "stream",
            "model",
        ] {
            let mut value = profiled(true);
            value["models"]["chosen"]["profiles"]["off"]["requestParams"] = json!({key: 1});
            assert!(
                matches!(
                    catalog(value),
                    Err(ModelCatalogError::ProtectedKey {
                        layer: RequestParamsLayer::ModelProfile,
                        ..
                    })
                ),
                "{key}"
            );
        }
    }
    #[test]
    fn reasoning_parameters_need_no_capability_declaration() {
        let mut value = document();
        value["models"]["chosen"]["requestParams"] =
            json!({"provider_reasoning":{"mode":"default"}, "opaque":[null, true, {"future":42}]});
        let bindings = registry(value);
        let invocation = bindings
            .resolve(&crate::model::invocation::ModelSelection::of(selected()))
            .unwrap();
        assert_eq!(invocation.profile(), None);
        assert_eq!(invocation.request_params()["opaque"][2]["future"], 42);
        assert_eq!(invocation.request_params()["opaque"][0], Value::Null);
        assert_eq!(
            invocation.request_params()["provider_reasoning"]["mode"],
            "default"
        );
    }
    #[test]
    fn model_and_profile_numbers_are_held_to_the_binary64_domain() {
        let mut value = document();
        value["models"]["chosen"]["requestParams"] = json!({"seed": 9_007_199_254_740_993_u64});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::InexactNumber {
                layer: RequestParamsLayer::ModelParams,
                ..
            })
        ));
        let mut value = profiled(false);
        value["models"]["chosen"]["profiles"]["creative"]["requestParams"] =
            json!({"nested": [{"seed": -9_007_199_254_740_993_i64}]});
        let error = catalog(value).unwrap_err();
        assert!(matches!(
            error,
            ModelCatalogError::InexactNumber {
                layer: RequestParamsLayer::ModelProfile,
                ..
            }
        ));
        assert!(!error.to_string().contains("9007199254740993"), "{error}");
        let mut value = profiled(false);
        value["models"]["chosen"]["profiles"]["creative"]["requestParams"] =
            json!({"seed": 9_007_199_254_740_992_u64, "t": 0.1, "big": 1e300});
        catalog(value).unwrap();
    }
    #[test]
    fn same_invocation_compares_effective_semantics_not_authoring() {
        use crate::model::invocation::ModelSelection;
        type Mutation = fn(&mut Value);
        let layer = RequestParamsLayer::SessionOverrides;
        let implicit = ModelSelection::of(selected());
        let explicit = |profile: &str| ModelSelection {
            profile: Some(ModelProfileId::new(profile)),
            ..ModelSelection::of(selected())
        };
        let same = |a: &Value, sa: &ModelSelection, b: &Value, sb: &ModelSelection| {
            catalog(a.clone())
                .unwrap()
                .same_invocation(sa, &catalog(b.clone()).unwrap(), sb, layer)
        };
        for reasoning in [true, false] {
            let base = profiled(reasoning);
            // An omitted Profile and an explicit default Profile are one invocation.
            assert!(same(&base, &implicit, &base, &explicit("balanced")));
            assert!(same(&base, &explicit("balanced"), &base, &implicit));
            let other = if reasoning { "fast" } else { "creative" };
            assert!(!same(&base, &implicit, &base, &explicit(other)));
            // Editing an unselected Profile, or adding one, is no change.
            let mut edited = base.clone();
            edited["models"]["chosen"]["profiles"][other]["requestParams"] =
                json!({"edited": true});
            edited["models"]["chosen"]["profiles"]["added"] = json!({"requestParams": {}});
            assert!(same(&base, &implicit, &edited, &implicit));
            assert!(same(
                &base,
                &explicit("balanced"),
                &edited,
                &explicit("balanced")
            ));
            assert!(!same(&base, &explicit(other), &edited, &explicit(other)));
            // Moving the default changes the implicit selection only.
            let mut moved = base.clone();
            moved["models"]["chosen"]["defaultProfile"] = other.into();
            assert!(!same(&base, &implicit, &moved, &implicit));
            assert!(same(
                &base,
                &explicit("balanced"),
                &moved,
                &explicit("balanced")
            ));
            assert!(same(&base, &implicit, &moved, &explicit("balanced")));
        }
        // Every effective property of the selected Profile counts.
        let base = profiled(true);
        let profile_edits: [Mutation; 3] = [
            |v| {
                v["models"]["chosen"]["profiles"]["balanced"]["requestParams"]["opaque"] =
                    json!([false]);
            },
            |v| v["models"]["chosen"]["profiles"]["balanced"]["maxOutputTokens"] = 50.into(),
            |v| {
                v["models"]["chosen"]["profiles"]["balanced"]["requestParams"]["extra"] =
                    json!(null);
            },
        ];
        // And so does every property of the binding it travels over.
        let bindings: [Mutation; 9] = [
            |v| v["providers"]["p"]["baseUrl"] = "https://other.example/v1".into(),
            |v| v["providers"]["p"]["apiKey"] = "$OTHER_KEY".into(),
            |v| v["models"]["chosen"]["id"] = "wire/other".into(),
            |v| v["models"]["chosen"]["contextWindow"] = 2000.into(),
            |v| v["models"]["chosen"]["maxOutputTokens"] = 200.into(),
            |v| v["models"]["chosen"]["capabilities"]["inputModalities"] = json!(["text", "image"]),
            |v| v["models"]["chosen"]["compat"]["chatReasoningReplay"] = "reasoning_content".into(),
            |v| {
                v["providers"]["p"]["protocol"] = "openai_responses".into();
                v["models"]["chosen"]["compat"] = json!({});
            },
            |v| v["models"]["chosen"]["capabilities"]["jsonSchemaOutput"] = true.into(),
        ];
        for (index, change) in profile_edits.iter().chain(&bindings).enumerate() {
            let mut changed = base.clone();
            change(&mut changed);
            assert!(
                !same(&base, &implicit, &changed, &implicit),
                "change {index}"
            );
        }
        // A selection that no longer resolves is never the same.
        let mut removed = base.clone();
        removed["models"]["chosen"]["profiles"]
            .as_object_mut()
            .unwrap()
            .remove("fast");
        assert!(!same(&base, &explicit("fast"), &removed, &explicit("fast")));
        // Explicit overrides are compared after resolution.
        let overriding = ModelSelection {
            request_params: json!({"seed": 1}).as_object().unwrap().clone(),
            ..explicit("balanced")
        };
        assert!(!same(&base, &implicit, &base, &overriding));
        let inherited = ModelSelection {
            profile: None,
            ..overriding.clone()
        };
        assert!(same(&base, &overriding, &base, &inherited));
    }
}
