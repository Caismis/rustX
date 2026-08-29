//! The serializable frozen model authority of an out-of-process runtime
//! (Issue #144).
//!
//! ```text
//! parent process                          child process
//! --------------                          -------------
//! SessionModelConfig
//!   -> ModelBindingRegistry::resolve
//!   -> ResolvedModelInvocation            (semantic authority, decided HERE)
//!   -> FrozenModelSpec  ==== IPC ====>    FrozenModelSpec
//!                                           -> materialize (physical only)
//!                                           -> ResolvedModelInvocation
//!                                           -> SessionModelState::frozen
//! ```
//!
//! # Why this type exists
//!
//! [`SessionModelConfig`] is *desired configuration*, not a resolved model
//! binding. Handing a child a `SessionModelConfig` plus a `models.jsonc`
//! path makes the child re-resolve semantics against a **mutable** file:
//! the catalog can change between the moment the parent admitted the
//! invoking attempt and the moment the child composes, and the child would
//! then silently observe a different provider endpoint, protocol, context
//! window, output budget, reasoning profile, request parameters,
//! compatibility metadata, effective capabilities — or fail to resolve a
//! model that was valid when the parent froze the child.
//!
//! [`FrozenModelSpec`] closes that race by carrying the already-resolved
//! semantics across the process boundary. The child performs only
//! **physical materialization**: it constructs the provider adapter from
//! the frozen provider binding and resolves the declared credential source
//! against its own process environment — exactly the credential boundary
//! rustX already has — and never opens `models.jsonc` again.
//!
//! # What deliberately does not cross
//!
//! - `Arc<dyn ModelAdapter>` — an adapter is a live HTTP client, not data.
//!   It is rebuilt on the child side from the frozen provider binding.
//! - A resolved credential *value*. The frozen binding carries the
//!   declared [`CredentialSource`] (the same bounded two-form syntax the
//!   catalog declares), and the child resolves it through the ordinary
//!   [`CredentialEnvironment`] seam. A literal source is literal in the
//!   catalog too; an `$ENV_VAR` source is read from the child's own
//!   environment, never captured from the parent's.
//! - The rest of the catalog. A frozen spec authorizes exactly one primary
//!   model and at most one explicit summary model — nothing else is
//!   selectable in the child, because a child has no mutable model
//!   authority at all.

use serde::{Deserialize, Serialize};

use crate::model::catalog::{
    CatalogModelView, CredentialEnvironment, CredentialSource, ModelCapabilities, ModelCatalogView,
    ModelCompat, ModelRef, ProviderId, ReasoningProfileId, ReasoningProfileView,
    ResolvedCredential,
};
use crate::model::invocation::{
    ModelBindingRegistry, ModelInvocationError, RequestParams, ResolvedModelInvocation,
};
use crate::model::session::{AttemptSummaryModel, SessionModelConfig};
use crate::model::types::ModelProtocol;

/// The immutable provider binding data one adapter is constructed from.
///
/// This is the *whole* provider input of adapter construction: rustX builds
/// each supported protocol adapter from an endpoint and a credential, so a
/// frozen endpoint plus a frozen credential source reproduces the exact
/// binding the parent resolved without any catalog lookup.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenProviderBinding {
    /// The provider identity of the binding.
    pub provider: ProviderId,
    /// The explicit provider endpoint frozen with the binding.
    pub base_url: String,
    /// The declared credential source, resolved by the consuming process
    /// against its own [`CredentialEnvironment`].
    #[serde(with = "credential_source_wire")]
    pub credential: CredentialSource,
}

impl core::fmt::Debug for FrozenProviderBinding {
    /// Redacted: a literal credential never appears in debug output, which
    /// preserves the [`CredentialSource`] contract across this boundary.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FrozenProviderBinding")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("credential", &self.credential)
            .finish()
    }
}

/// The one serialization of the bounded credential-source syntax.
///
/// [`CredentialSource`] is deliberately not `Serialize` in general: making
/// it so would let a credential leak into any view, event, or snapshot that
/// happens to embed it. This module is the single audited place where a
/// declared source crosses a process boundary, and it emits exactly the
/// catalog's own two-form spelling (`"$ENV_VAR"` or a literal), which
/// [`CredentialSource::parse`] reads back. The round trip is total because
/// the parser is the only constructor: a literal can never begin with `$`,
/// so the two spellings never collide.
mod credential_source_wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::model::catalog::{CredentialSource, ProviderId};

    pub(super) fn serialize<S: Serializer>(
        value: &CredentialSource,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let declared = match value {
            CredentialSource::Literal(literal) => literal.clone(),
            CredentialSource::Environment(variable) => format!("${variable}"),
        };
        declared.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<CredentialSource, D::Error> {
        let declared = String::deserialize(deserializer)?;
        // The provider identity only shapes the rejection diagnostic; the
        // frozen value was already validated by catalog admission.
        let provider = ProviderId::parse("frozen").map_err(serde::de::Error::custom)?;
        CredentialSource::parse(&declared, &provider).map_err(serde::de::Error::custom)
    }
}

/// One completely resolved model invocation in serializable form.
///
/// Every field is a semantic decision the parent already made. Nothing here
/// is re-derived by the consumer: materialization is a pure construction of
/// the provider adapter plus a copy of these values into
/// [`ResolvedModelInvocation`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenModelInvocation {
    /// The immutable provider binding this invocation was resolved against.
    pub binding: FrozenProviderBinding,
    /// The fully qualified model reference.
    pub model: ModelRef,
    /// The protocol the adapter must speak.
    pub protocol: ModelProtocol,
    /// The model's context window in tokens.
    pub context_window: u64,
    /// The model's configured maximum output tokens.
    pub model_max_output_tokens: u32,
    /// The effective output budget of this invocation.
    pub max_output_tokens: u32,
    /// The selected reasoning profile, when the model declares any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// Whether the selected profile semantically enables reasoning.
    pub reasoning_enabled: bool,
    /// The effective opaque provider request parameters, already overlaid.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The effective capabilities of this invocation.
    pub capabilities: ModelCapabilities,
    /// The capabilities the catalog claimed, kept for the redacted view.
    pub declared_capabilities: ModelCapabilities,
    /// The bounded structural translation metadata.
    pub compat: ModelCompat,
}

impl FrozenModelInvocation {
    /// Physically materializes this frozen invocation.
    ///
    /// The only work done here is adapter construction: the credential
    /// source is resolved against `credentials` — the consuming process's
    /// ordinary credential boundary — and the protocol adapter is built
    /// from the frozen endpoint. No catalog is opened, no model is chosen,
    /// and no semantic field is recomputed.
    ///
    /// # Errors
    ///
    /// Returns [`ModelInvocationError::Catalog`] when the declared
    /// credential source cannot be resolved in this process.
    pub fn materialize(
        &self,
        credentials: &dyn CredentialEnvironment,
    ) -> Result<ResolvedModelInvocation, ModelInvocationError> {
        let credential = resolve_frozen_credential(&self.binding, credentials)?;
        let adapter = crate::model::invocation::build_protocol_adapter(
            self.protocol,
            credential.expose(),
            &self.binding.base_url,
        );
        Ok(ResolvedModelInvocation::from_frozen(self, adapter))
    }

    /// The single-model catalog view a frozen authority can honestly serve.
    #[must_use]
    pub fn catalog_model_view(&self) -> CatalogModelView {
        CatalogModelView {
            model: self.model.clone(),
            protocol: self.protocol,
            context_window: self.context_window,
            max_output_tokens: self.model_max_output_tokens,
            declared_capabilities: self.declared_capabilities.clone(),
            effective_capabilities: self.capabilities.clone(),
            // A frozen authority carries only the profile that was
            // selected; the rest of the model's declared profiles are
            // catalog state the child deliberately does not hold.
            reasoning_profiles: self
                .reasoning_profile
                .as_ref()
                .map(|id| {
                    vec![ReasoningProfileView {
                        id: id.clone(),
                        enabled: self.reasoning_enabled,
                    }]
                })
                .unwrap_or_default(),
            default_reasoning_profile: self.reasoning_profile.clone(),
            credential_source: self.binding.credential.view(),
        }
    }
}

/// The frozen summary-model policy of a frozen authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrozenSummaryModel {
    /// Summaries follow the frozen primary invocation.
    Session,
    /// Summaries use this separately frozen explicit invocation.
    Explicit(Box<FrozenModelInvocation>),
}

/// The complete frozen model authority of one out-of-process runtime.
///
/// `configured` is carried alongside the resolved invocations purely as the
/// **descriptive** record of what was asked for — it is what a client sees
/// as `SessionModelView::configured`. It is never re-resolved: the primary
/// and summary invocations are the authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenModelSpec {
    /// The desired configuration this authority was frozen from, kept for
    /// projection only.
    pub configured: SessionModelConfig,
    /// The frozen primary invocation.
    pub primary: FrozenModelInvocation,
    /// The frozen summary policy.
    pub summary: FrozenSummaryModel,
}

impl FrozenModelSpec {
    /// Freezes one desired configuration against an already-admitted
    /// binding registry.
    ///
    /// This is the parent-side semantic decision point: after this call the
    /// configuration has been resolved exactly once, and every consumer of
    /// the result observes that one resolution.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure of the primary model or of an
    /// explicit summary model.
    pub fn freeze(
        registry: &ModelBindingRegistry,
        configured: &SessionModelConfig,
    ) -> Result<Self, ModelInvocationError> {
        let primary = registry.freeze(&configured.selection())?;
        let summary = match configured.summary_selection() {
            None => FrozenSummaryModel::Session,
            Some(selection) => FrozenSummaryModel::Explicit(Box::new(registry.freeze_with_layer(
                &selection,
                crate::model::invocation::RequestParamsLayer::SummaryOverrides,
            )?)),
        };
        Ok(Self {
            configured: configured.clone(),
            primary,
            summary,
        })
    }

    /// Physically materializes the frozen primary and summary invocations.
    ///
    /// # Errors
    ///
    /// Returns the first credential-resolution failure.
    pub fn materialize(
        &self,
        credentials: &dyn CredentialEnvironment,
    ) -> Result<(ResolvedModelInvocation, AttemptSummaryModel), ModelInvocationError> {
        let primary = self.primary.materialize(credentials)?;
        let summary = match &self.summary {
            FrozenSummaryModel::Session => AttemptSummaryModel::Session,
            FrozenSummaryModel::Explicit(invocation) => {
                AttemptSummaryModel::Explicit(Box::new(invocation.materialize(credentials)?))
            }
        };
        Ok((primary, summary))
    }

    /// The catalog view a frozen authority serves: exactly the models it
    /// froze, in primary-then-explicit-summary order.
    #[must_use]
    pub fn catalog_view(&self) -> ModelCatalogView {
        let mut models = vec![self.primary.catalog_model_view()];
        if let FrozenSummaryModel::Explicit(invocation) = &self.summary
            && invocation.model != self.primary.model
        {
            models.push(invocation.catalog_model_view());
        }
        ModelCatalogView { models }
    }
}

/// A minimal frozen authority for in-crate tests that need the *shape* of a
/// frozen model specification without composing a catalog.
///
/// The binding names a literal credential and an unroutable endpoint: a test
/// that only composes a runtime never issues a provider request, and a test
/// that does would fail loudly rather than reach a real service.
#[cfg(test)]
#[must_use]
pub(crate) fn test_frozen_model_spec(model: ModelRef) -> FrozenModelSpec {
    FrozenModelSpec {
        configured: SessionModelConfig::of(model.clone()),
        primary: FrozenModelInvocation {
            binding: FrozenProviderBinding {
                provider: model.provider().clone(),
                base_url: "http://127.0.0.1:9/v1".to_owned(),
                credential: CredentialSource::Literal("test-only-secret".to_owned()),
            },
            model,
            protocol: ModelProtocol::OpenAiChatCompletions,
            context_window: 128_000,
            model_max_output_tokens: 512,
            max_output_tokens: 512,
            reasoning_profile: None,
            reasoning_enabled: false,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, false),
            declared_capabilities: ModelCapabilities::text_only(true, false),
            compat: ModelCompat::default(),
        },
        summary: FrozenSummaryModel::Session,
    }
}

/// Resolves one frozen credential source in the consuming process.
fn resolve_frozen_credential(
    binding: &FrozenProviderBinding,
    credentials: &dyn CredentialEnvironment,
) -> Result<ResolvedCredential, ModelInvocationError> {
    match &binding.credential {
        CredentialSource::Literal(value) => Ok(ResolvedCredential::new(value.clone())),
        CredentialSource::Environment(name) => credentials
            .var(name)
            .filter(|value| !value.is_empty())
            .map(ResolvedCredential::new)
            .ok_or_else(|| {
                ModelInvocationError::Catalog(
                    crate::model::catalog::ModelCatalogError::MissingEnvironmentCredential {
                        provider: binding.provider.clone(),
                        variable: name.clone(),
                    },
                )
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::{FrozenModelSpec, FrozenSummaryModel};
    use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
    use crate::model::invocation::ModelBindingRegistry;
    use crate::model::session::SessionModelConfig;
    use crate::model::types::ModelProtocol;

    const M1: &str = r#"{
      "providers": {
        "local": {
          "baseUrl": "http://127.0.0.1:9/v1",
          "apiKey": "$RUSTX_FROZEN_KEY",
          "models": [{
            "id": "m",
            "protocol": "openai_chat_completions",
            "contextWindow": 128000,
            "maxOutputTokens": 512,
            "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
            "requestParams": {"temperature": 0.25},
            "compat": {"chatReasoningReplay": "omit"}
          }]
        }
      }
    }"#;

    fn environment() -> MapCredentialEnvironment {
        MapCredentialEnvironment::new([(
            "RUSTX_FROZEN_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )])
    }

    fn registry(document: &str) -> ModelBindingRegistry {
        let catalog = ModelCatalog::from_jsonc_slice(document.as_bytes()).expect("catalog");
        ModelBindingRegistry::new(catalog.resolve(&environment()).expect("resolved"))
            .expect("registry")
    }

    fn config() -> SessionModelConfig {
        SessionModelConfig::of(ModelRef::parse("local/m").expect("reference"))
    }

    /// The freeze carries the exact resolved semantics, and materializing
    /// it reproduces the same invocation without any catalog.
    #[test]
    fn a_freeze_round_trips_the_exact_resolved_semantics() {
        let registry = registry(M1);
        let resolved = registry.resolve(&config().selection()).expect("resolve");
        let frozen = FrozenModelSpec::freeze(&registry, &config()).expect("freeze");
        let encoded = serde_json::to_vec(&frozen).expect("encode");
        let decoded: FrozenModelSpec = serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded, frozen);

        let (primary, summary) = decoded.materialize(&environment()).expect("materialize");
        assert_eq!(primary.protocol(), resolved.protocol());
        assert_eq!(primary.context_window(), resolved.context_window());
        assert_eq!(primary.max_output_tokens(), resolved.max_output_tokens());
        assert_eq!(primary.request_params(), resolved.request_params());
        assert_eq!(primary.capabilities(), resolved.capabilities());
        assert_eq!(primary.compat(), resolved.compat());
        assert_eq!(primary.provider(), resolved.provider());
        assert_eq!(primary.model_ref(), resolved.model_ref());
        assert!(matches!(
            summary,
            crate::model::AttemptSummaryModel::Session
        ));
        assert!(matches!(decoded.summary, FrozenSummaryModel::Session));
    }

    /// The declared credential source crosses as a *source*, never as a
    /// resolved value: an environment source is read from the consuming
    /// process, so a different environment is a materialization failure
    /// rather than a silent parent-credential capture.
    #[test]
    fn an_environment_credential_is_resolved_by_the_consumer() {
        let frozen = FrozenModelSpec::freeze(&registry(M1), &config()).expect("freeze");
        assert!(
            !serde_json::to_string(&frozen)
                .expect("encode")
                .contains("test-only-secret"),
            "an environment-sourced credential value never crosses the boundary"
        );
        assert!(
            frozen
                .materialize(&MapCredentialEnvironment::default())
                .is_err(),
            "the consumer resolves the declared source in its own environment"
        );
    }

    /// Both spellings of the bounded credential syntax survive the wire
    /// unchanged, so a frozen binding names the same source the catalog
    /// declared.
    #[test]
    fn both_credential_source_spellings_round_trip() {
        use crate::model::catalog::{CredentialSource, CredentialSourceView};

        for (declared, expected) in [
            (
                "$RUSTX_FROZEN_KEY",
                CredentialSourceView::Environment {
                    variable: "RUSTX_FROZEN_KEY".to_owned(),
                },
            ),
            ("a-literal-value", CredentialSourceView::Literal),
        ] {
            let mut frozen = FrozenModelSpec::freeze(&registry(M1), &config()).expect("freeze");
            frozen.primary.binding.credential =
                CredentialSource::parse(declared, &frozen.primary.binding.provider)
                    .expect("declared source parses");
            let decoded: FrozenModelSpec =
                serde_json::from_slice(&serde_json::to_vec(&frozen).expect("encode"))
                    .expect("decode");
            assert_eq!(
                decoded.primary.binding.credential,
                frozen.primary.binding.credential
            );
            assert_eq!(decoded.primary.binding.credential.view(), expected);
        }
    }

    /// A frozen authority serves only the model it froze.
    #[test]
    fn the_frozen_catalog_view_contains_only_the_frozen_model() {
        let frozen = FrozenModelSpec::freeze(&registry(M1), &config()).expect("freeze");
        let view = frozen.catalog_view();
        assert_eq!(view.models.len(), 1);
        assert_eq!(view.models[0].model, ModelRef::parse("local/m").expect("m"));
        assert_eq!(
            view.models[0].protocol,
            ModelProtocol::OpenAiChatCompletions
        );
        assert_eq!(view.models[0].context_window, 128_000);
    }
}
