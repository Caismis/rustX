//! Session model authority and the immutable attempt model snapshot
//! (Issue #42).
//!
//! ```text
//! SessionModelConfig            (mutable, runtime authority, client-settable)
//!         |  resolve through the ModelBindingRegistry
//!         v
//! ResolvedModelInvocation       (immutable primary) + summary policy
//!         |  frozen at attempt admission
//!         v
//! AttemptModelSnapshot          (immutable for the whole attempt)
//! ```
//!
//! The governing invariant:
//!
//! > A session-model update that linearizes **before** attempt admission is
//! > observed by that attempt. An update that linearizes **after** admission
//! > affects only future attempts.
//!
//! A session composed from an already-frozen specification (a subagent
//! child, Issue #144) short-circuits the first arrow: the parent performed
//! the `SessionModelConfig -> ResolvedModelInvocation` resolution, and the
//! child materializes that decision instead of repeating it against its own
//! view of a mutable catalog file.
//!
//! After admission every model turn of the attempt — every tool→model
//! continuation, every transient or context-overflow retry, every
//! proactive-compaction continuation, and every compaction summary — uses the
//! same immutable snapshot. It never reads live mutable session model state
//! again.
//!
//! # Summary policy
//!
//! Production supports exactly two modes, and both resolve through the same
//! catalog, credential binding, compat handling, Model Profile selection,
//! protected-key validation, and shallow overlay as a primary model:
//!
//! - `session` — the summary uses the attempt's frozen primary invocation,
//!   subject only to the context plane's summary output safety cap, which is
//!   applied through the runtime-owned protected max-output field and never
//!   by mutating a Model Profile or a request-parameter object;
//! - `explicit` — a separately resolved catalog model, frozen at admission
//!   so a later mutation of live session state cannot change the summary
//!   model of an already-admitted attempt.

use serde::{Deserialize, Serialize};

use crate::model::catalog::{CredentialEnvironment, ModelCatalogView, ModelProfileId, ModelRef};
use crate::model::frozen::FrozenModelSpec;
use crate::model::invocation::{
    ModelBindingRegistry, ModelInvocationError, ModelInvocationView, ModelSelection, RequestParams,
    RequestParamsLayer, ResolvedModelInvocation,
};

/// The compaction summary model policy of a session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum SummaryModelPolicy {
    /// Summary generation follows the admitted attempt's primary model.
    #[default]
    Session,
    /// Summary generation uses an explicitly configured catalog model.
    Explicit {
        /// The catalog model reference.
        model: ModelRef,
        /// The selected Model Profile; the model's `default_profile` is used
        /// when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<ModelProfileId>,
        /// The explicit summary request-parameter overrides.
        #[serde(default)]
        request_params: RequestParams,
        /// The explicit summary output-budget override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u32>,
    },
}

/// The authoritative mutable model configuration of one conversation
/// session.
///
/// This one type is the session's state, the `model_get` result, and the
/// `model_set` parameter: an update is a whole-state replacement, never an
/// ambiguous JSON patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SessionModelConfig {
    /// The selected catalog model.
    pub model: ModelRef,
    /// The selected Model Profile; the model's `default_profile` is used
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ModelProfileId>,
    /// The session request-parameter overrides.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The session output-budget override; the selected profile's default,
    /// else the model's hard maximum, is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// The compaction summary model policy.
    #[serde(default)]
    pub summary_model: SummaryModelPolicy,
}

impl SessionModelConfig {
    /// A configuration selecting one model with every default.
    #[must_use]
    pub fn of(model: ModelRef) -> Self {
        Self {
            model,
            profile: None,
            request_params: RequestParams::new(),
            max_output_tokens: None,
            summary_model: SummaryModelPolicy::Session,
        }
    }

    /// The primary model selection this configuration expresses.
    #[must_use]
    pub fn selection(&self) -> ModelSelection {
        ModelSelection {
            model: self.model.clone(),
            profile: self.profile.clone(),
            request_params: self.request_params.clone(),
            max_output_tokens: self.max_output_tokens,
        }
    }

    /// The explicit summary model selection, when the policy is explicit.
    #[must_use]
    pub fn summary_selection(&self) -> Option<ModelSelection> {
        match &self.summary_model {
            SummaryModelPolicy::Session => None,
            SummaryModelPolicy::Explicit {
                model,
                profile,
                request_params,
                max_output_tokens,
            } => Some(ModelSelection {
                model: model.clone(),
                profile: profile.clone(),
                request_params: request_params.clone(),
                max_output_tokens: *max_output_tokens,
            }),
        }
    }
}

/// Analyze the complete Session configuration without credentials, bindings, or I/O.
/// Primary and explicit summary selections use the same static semantics and
/// request-parameter layers as native binding. An absent summary follows primary.
pub(crate) fn analyze_session_model_config(
    catalog: &crate::model::catalog::ModelCatalog,
    config: &SessionModelConfig,
) -> Result<(ModelInvocationView, Option<ModelInvocationView>), ModelInvocationError> {
    let analyze = |selection: ModelSelection, layer| {
        crate::model::invocation::analyze_selection(
            catalog.model(&selection.model)?,
            &selection,
            layer,
        )
    };
    let primary = analyze(config.selection(), RequestParamsLayer::SessionOverrides)?;
    let summary = config
        .summary_selection()
        .map(|selection| analyze(selection, RequestParamsLayer::SummaryOverrides))
        .transpose()?;
    Ok((primary, summary))
}

/// The frozen summary-model resolution of one attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptSummaryModel {
    /// The summary follows the attempt's primary invocation.
    Session,
    /// The summary uses this already-resolved explicit invocation.
    Explicit(Box<ResolvedModelInvocation>),
}

/// The immutable model ownership object of one admitted attempt.
///
/// Taken at the admission linearization boundary and never re-derived: the
/// whole attempt, including every compaction summary, reads only this value.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptModelSnapshot {
    primary: ResolvedModelInvocation,
    summary: AttemptSummaryModel,
}

impl AttemptModelSnapshot {
    /// Creates a snapshot from an already-resolved primary and summary.
    #[must_use]
    pub const fn new(primary: ResolvedModelInvocation, summary: AttemptSummaryModel) -> Self {
        Self { primary, summary }
    }

    /// The attempt's primary model invocation.
    #[must_use]
    pub const fn primary(&self) -> &ResolvedModelInvocation {
        &self.primary
    }

    /// The frozen summary policy.
    #[must_use]
    pub const fn summary_policy(&self) -> &AttemptSummaryModel {
        &self.summary
    }

    /// The invocation compaction summaries of this attempt must use.
    ///
    /// In `session` mode this is the primary invocation itself — the same
    /// provider binding, model, protocol, Model Profile, and effective
    /// request parameters.
    #[must_use]
    pub const fn summary_invocation(&self) -> &ResolvedModelInvocation {
        match &self.summary {
            AttemptSummaryModel::Session => &self.primary,
            AttemptSummaryModel::Explicit(invocation) => invocation,
        }
    }

    /// The redacted client-facing projection of this snapshot.
    #[must_use]
    pub fn view(&self) -> AttemptModelView {
        AttemptModelView {
            primary: self.primary.view(),
            summary: match &self.summary {
                AttemptSummaryModel::Session => SummaryModelView::Session,
                AttemptSummaryModel::Explicit(invocation) => {
                    SummaryModelView::Explicit(Box::new(invocation.view()))
                }
            },
        }
    }
}

/// Where a session's model semantics come from.
///
/// A normal session owns a mutable catalog authority and can re-resolve a
/// replacement configuration against it. A session composed from an
/// already-frozen specification — a subagent child (Issue #144) — owns no
/// catalog at all: its semantics were decided by the parent, and there is
/// nothing in-process to resolve a different model against.
#[derive(Debug, Clone)]
enum ModelAuthority {
    /// The mutable catalog authority of an ordinary session.
    Catalog(ModelBindingRegistry),
    /// The immutable parent-frozen authority of an out-of-process child.
    Frozen(Box<FrozenModelSpec>),
}

/// The session's model authority plus the current desired configuration and
/// its resolution.
///
/// Updates are transactional: a failed update changes nothing, so a caller
/// can publish a model-change observation exactly when `apply` returns
/// `Ok`.
///
/// The configured selection, the catalog future selections resolve against,
/// and the invocation last resolved for the configuration are distinct facts.
/// A published catalog may no longer admit the configured selection; the
/// state then records why ([`Self::unavailable`]) and refuses every new
/// snapshot until a valid selection is applied. It never substitutes a
/// default, and it keeps the invocation it last resolved, which only display
/// reads.
#[derive(Debug, Clone)]
pub struct SessionModelState {
    authority: ModelAuthority,
    config: SessionModelConfig,
    /// The invocation last resolved for `config`; absent only when this
    /// state was restored with an already unavailable selection.
    resolved: Option<(ResolvedModelInvocation, AttemptSummaryModel)>,
    /// Why the authority's catalog does not admit `config`.
    unavailable: Option<ModelInvocationError>,
}

impl SessionModelState {
    /// Resolves and validates the initial session model configuration.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure of the primary model or of an
    /// explicit summary model.
    pub fn new(
        registry: ModelBindingRegistry,
        config: SessionModelConfig,
    ) -> Result<Self, ModelInvocationError> {
        let resolved = resolve(&registry, &config)?;
        Ok(Self {
            authority: ModelAuthority::Catalog(registry),
            config,
            resolved: Some(resolved),
            unavailable: None,
        })
    }

    /// Restores a Session's persisted configuration against the catalog it
    /// now publishes.
    ///
    /// Exactly like [`Self::new`], except that a selection the catalog no
    /// longer admits — a removed Model or Profile, or a limit the Model no
    /// longer allows — composes an unavailable state instead of failing: the
    /// Session stays loadable, advertises the catalog, and refuses admission
    /// until a valid selection is applied. Nothing is resolved for it.
    ///
    /// # Errors
    ///
    /// Returns every other resolution failure, such as a credential failure.
    pub fn restore(
        registry: ModelBindingRegistry,
        config: SessionModelConfig,
    ) -> Result<Self, ModelInvocationError> {
        if let Err(error) = analyze_session_model_config(registry.catalog().catalog(), &config) {
            return Ok(Self {
                authority: ModelAuthority::Catalog(registry),
                config,
                resolved: None,
                unavailable: Some(error),
            });
        }
        Self::new(registry, config)
    }

    /// This Session's state for a candidate configuration generation that
    /// publishes `registry` and carries `config` — the Session's own
    /// selection, not a new choice.
    ///
    /// A configuration the catalog admits is resolved exactly like
    /// [`Self::new`]. One it no longer admits is unavailable, keeping the
    /// invocation this state last resolved when `config` is unchanged, so
    /// adopting the candidate publishes the catalog without replacing,
    /// re-resolving or substituting that invocation.
    ///
    /// # Errors
    ///
    /// Returns every other resolution failure.
    pub fn carry(
        &self,
        registry: ModelBindingRegistry,
        config: SessionModelConfig,
    ) -> Result<Self, ModelInvocationError> {
        match analyze_session_model_config(registry.catalog().catalog(), &config) {
            Ok(_) => Self::new(registry, config),
            Err(error) => Ok(Self {
                authority: ModelAuthority::Catalog(registry),
                resolved: (config == self.config)
                    .then(|| self.resolved.clone())
                    .flatten(),
                config,
                unavailable: Some(error),
            }),
        }
    }

    /// Composes the session model state of a runtime whose model semantics
    /// were **already resolved by another process** (Issue #144).
    ///
    /// Nothing is resolved here: the frozen primary and summary invocations
    /// are materialized physically — adapter construction plus credential
    /// resolution through `credentials`, the ordinary process credential
    /// boundary — and become the state verbatim. No model catalog is opened,
    /// so a catalog file that changed after the parent froze this
    /// specification cannot be observed.
    ///
    /// # Errors
    ///
    /// Returns the first credential-resolution failure.
    pub fn frozen(
        frozen: &FrozenModelSpec,
        credentials: &dyn CredentialEnvironment,
    ) -> Result<Self, ModelInvocationError> {
        let resolved = frozen.materialize(credentials)?;
        Ok(Self {
            authority: ModelAuthority::Frozen(Box::new(frozen.clone())),
            config: frozen.configured.clone(),
            resolved: Some(resolved),
            unavailable: None,
        })
    }

    /// The authoritative desired configuration.
    #[must_use]
    pub const fn config(&self) -> &SessionModelConfig {
        &self.config
    }

    /// The binding registry behind this session, when it owns one.
    ///
    /// A frozen (subagent child) authority returns `None`: it holds exactly
    /// the invocations its parent froze and no catalog to resolve others
    /// against.
    #[must_use]
    pub const fn registry(&self) -> Option<&ModelBindingRegistry> {
        match &self.authority {
            ModelAuthority::Catalog(registry) => Some(registry),
            ModelAuthority::Frozen(_) => None,
        }
    }

    /// The safe public catalog view.
    ///
    /// A frozen authority serves exactly the models it froze.
    #[must_use]
    pub fn catalog_view(&self) -> ModelCatalogView {
        match &self.authority {
            ModelAuthority::Catalog(registry) => registry.catalog_view(),
            ModelAuthority::Frozen(frozen) => frozen.catalog_view(),
        }
    }

    /// Why the authority's catalog does not admit the configured selection,
    /// when it does not.
    #[must_use]
    pub const fn unavailable(&self) -> Option<&ModelInvocationError> {
        self.unavailable.as_ref()
    }

    /// Freezes the current configuration into an attempt model snapshot.
    ///
    /// This is a cheap clone of values resolved when the configuration was
    /// last accepted, so it is safe to call under the admission
    /// linearization lock.
    ///
    /// # Errors
    ///
    /// Returns why the selection is unavailable: no new model work may be
    /// admitted with a selection the published catalog does not admit, and
    /// the invocation last resolved for it is never a substitute.
    pub fn snapshot(&self) -> Result<AttemptModelSnapshot, ModelInvocationError> {
        match (&self.unavailable, &self.resolved) {
            (None, Some((primary, summary))) => {
                Ok(AttemptModelSnapshot::new(primary.clone(), summary.clone()))
            }
            (Some(error), _) => Err(error.clone()),
            (None, None) => unreachable!("an admitted selection is always resolved"),
        }
    }

    /// Applies a whole-state configuration replacement transactionally.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure; on failure this state is
    /// completely unchanged and no model-change observation may be
    /// published.
    pub fn apply(&mut self, config: SessionModelConfig) -> Result<(), ModelInvocationError> {
        let ModelAuthority::Catalog(registry) = &self.authority else {
            return Err(ModelInvocationError::ImmutableModelAuthority {
                model: self.config.model.clone(),
            });
        };
        let resolved = resolve(registry, &config)?;
        self.config = config;
        self.resolved = Some(resolved);
        self.unavailable = None;
        Ok(())
    }

    /// Publishes a newly available catalog as the authority future
    /// selections resolve against, without touching the current invocation.
    ///
    /// Catalog publication and invocation adoption are distinct: the current
    /// configuration keeps the primary and Summary invocations it already
    /// resolved, so no adapter, credential or parameter is re-resolved, and
    /// the configured selection is never changed.
    ///
    /// - When `registry` no longer admits the configuration, it is published
    ///   anyway and the selection becomes unavailable: the catalog is what a
    ///   valid replacement is chosen from, and no new snapshot is admitted
    ///   until one is applied.
    /// - When the configuration still resolves, publication is sound only
    ///   while it resolves to effectively the same invocations
    ///   ([`ModelCatalog::same_invocation`]). Otherwise — including a
    ///   selection that becomes admissible again — nothing changes and
    ///   `false` is returned, because that change needs invocation
    ///   preparation and adoption.
    ///
    /// A frozen authority owns no catalog and always refuses.
    ///
    /// [`ModelCatalog::same_invocation`]: crate::model::catalog::ModelCatalog::same_invocation
    #[must_use]
    pub fn publish_catalog(&mut self, registry: ModelBindingRegistry) -> bool {
        let ModelAuthority::Catalog(current) = &self.authority else {
            return false;
        };
        let (old, new) = (current.catalog().catalog(), registry.catalog().catalog());
        if let Err(error) = analyze_session_model_config(new, &self.config) {
            self.authority = ModelAuthority::Catalog(registry);
            self.unavailable = Some(error);
            return true;
        }
        let same = |selection: ModelSelection, layer| {
            old.same_invocation(&selection, new, &selection, layer)
        };
        if self.unavailable.is_some()
            || !same(
                self.config.selection(),
                RequestParamsLayer::SessionOverrides,
            )
            || self
                .config
                .summary_selection()
                .is_some_and(|summary| !same(summary, RequestParamsLayer::SummaryOverrides))
        {
            return false;
        }
        self.authority = ModelAuthority::Catalog(registry);
        true
    }

    /// The redacted client-facing projection of the session model state.
    #[must_use]
    pub fn view(&self) -> SessionModelView {
        SessionModelView {
            configured: self.config.clone(),
            effective: self.resolved.as_ref().map(|(primary, _)| primary.view()),
            summary: self.resolved.as_ref().map(|(_, summary)| match summary {
                AttemptSummaryModel::Session => SummaryModelView::Session,
                AttemptSummaryModel::Explicit(invocation) => {
                    SummaryModelView::Explicit(Box::new(invocation.view()))
                }
            }),
            unavailable: self.unavailable.as_ref().map(ToString::to_string),
        }
    }
}

/// Resolves one configuration into its primary invocation and summary
/// policy without touching any existing state.
fn resolve(
    registry: &ModelBindingRegistry,
    config: &SessionModelConfig,
) -> Result<(ResolvedModelInvocation, AttemptSummaryModel), ModelInvocationError> {
    let primary = registry.resolve(&config.selection())?;
    let summary = match config.summary_selection() {
        None => AttemptSummaryModel::Session,
        Some(selection) => AttemptSummaryModel::Explicit(Box::new(
            registry.resolve_with_layer(&selection, RequestParamsLayer::SummaryOverrides)?,
        )),
    };
    Ok((primary, summary))
}

/// The redacted client-facing projection of the session model state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SessionModelView {
    /// The authoritative desired configuration, exactly as a client would
    /// send it back through `model_set`.
    pub configured: SessionModelConfig,
    /// The effective primary invocation last resolved for `configured`.
    /// Absent only when the Session was loaded with an already unavailable
    /// selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<ModelInvocationView>,
    /// The summary policy last resolved for `configured`, absent exactly
    /// when `effective` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryModelView>,
    /// Why the Session's published Model Catalog does not admit
    /// `configured`. While present, `effective` and `summary` are display
    /// facts only: no new Attempt is admitted until a valid selection is
    /// committed, and nothing falls back to a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// The redacted client-facing projection of a summary policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum SummaryModelView {
    /// The summary follows the primary model.
    Session,
    /// The summary uses this resolved explicit invocation.
    Explicit(Box<ModelInvocationView>),
}

/// The redacted client-facing projection of one attempt's frozen model
/// snapshot.
///
/// This is what makes "session desired model = B, running attempt model = A"
/// unambiguous without a client inferring anything from event ordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct AttemptModelView {
    /// The attempt's frozen primary invocation.
    pub primary: ModelInvocationView,
    /// The attempt's frozen summary policy.
    pub summary: SummaryModelView,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::ModelCatalog;

    #[test]
    fn analyze_session_model_config_validates_primary_and_explicit_summary() {
        let document: crate::local_runtime::authoring::RuntimeLayer = crate::toml_authoring::parse(
            include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
        )
        .unwrap();
        let catalog = ModelCatalog::from_document(
            crate::model::authoring::Catalog {
                schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
                providers: document.providers.unwrap(),
                models: document.models.unwrap(),
            }
            .into(),
        )
        .unwrap();
        let mut config = SessionModelConfig::of(ModelRef::parse("example/demo-model").unwrap());
        config.max_output_tokens = Some(2048);
        config.summary_model = SummaryModelPolicy::Explicit {
            model: config.model.clone(),
            profile: None,
            request_params: RequestParams::new(),
            max_output_tokens: Some(1024),
        };
        let (primary, summary) = analyze_session_model_config(&catalog, &config).unwrap();
        assert_eq!(primary.max_output_tokens, 2048);
        assert_eq!(summary.unwrap().max_output_tokens, 1024);

        config
            .request_params
            .insert("messages".into(), serde_json::json!([]));
        let ModelInvocationError::ProtectedKey(error) =
            analyze_session_model_config(&catalog, &config).unwrap_err()
        else {
            panic!("primary protected key must fail");
        };
        assert_eq!(error.layer, RequestParamsLayer::SessionOverrides);
        config.request_params.clear();
        let SummaryModelPolicy::Explicit { request_params, .. } = &mut config.summary_model else {
            unreachable!()
        };
        request_params.insert("messages".into(), serde_json::json!([]));
        let ModelInvocationError::ProtectedKey(error) =
            analyze_session_model_config(&catalog, &config).unwrap_err()
        else {
            panic!("summary protected key must fail");
        };
        assert_eq!(error.layer, RequestParamsLayer::SummaryOverrides);
    }

    #[test]
    fn analyze_session_model_config_validates_complete_summary_selection() {
        let document: crate::local_runtime::authoring::RuntimeLayer = crate::toml_authoring::parse(
            include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
        )
        .unwrap();
        let catalog = ModelCatalog::from_document(
            crate::model::authoring::Catalog {
                schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
                providers: document.providers.unwrap(),
                models: document.models.unwrap(),
            }
            .into(),
        )
        .unwrap();
        let mut config = SessionModelConfig::of(ModelRef::parse("example/demo-model").unwrap());
        for (model, profile, budget) in [
            ("example/missing", None, None),
            ("example/demo-model", Some("missing"), None),
            ("example/demo-model", None, Some(0)),
            ("example/demo-model", None, Some(4097)),
        ] {
            config.summary_model = SummaryModelPolicy::Explicit {
                model: ModelRef::parse(model).unwrap(),
                profile: profile.map(|p| ModelProfileId::parse(p).unwrap()),
                request_params: RequestParams::new(),
                max_output_tokens: budget,
            };
            assert!(analyze_session_model_config(&catalog, &config).is_err());
        }
    }
}
