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

/// A native catalog choice at the exact resource generation the client read.
/// Choosing defaults replaces the Session configuration; it never patches old overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogModelSelection {
    pub binding_revision: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub model_position: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub profile_position: Option<u64>,
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
    primary: std::sync::Arc<ResolvedModelInvocation>,
    summary: std::sync::Arc<AttemptSummaryModel>,
}

impl AttemptModelSnapshot {
    /// Creates a snapshot from an already-resolved primary and summary.
    #[must_use]
    pub fn new(primary: ResolvedModelInvocation, summary: AttemptSummaryModel) -> Self {
        Self {
            primary: std::sync::Arc::new(primary),
            summary: std::sync::Arc::new(summary),
        }
    }

    /// The attempt's primary model invocation.
    #[must_use]
    pub fn primary(&self) -> &ResolvedModelInvocation {
        &self.primary
    }

    /// The frozen summary policy.
    #[must_use]
    pub fn summary_policy(&self) -> &AttemptSummaryModel {
        &self.summary
    }

    /// The invocation compaction summaries of this attempt must use.
    ///
    /// In `session` mode this is the primary invocation itself — the same
    /// provider binding, model, protocol, Model Profile, and effective
    /// request parameters.
    #[must_use]
    pub fn summary_invocation(&self) -> &ResolvedModelInvocation {
        match self.summary.as_ref() {
            AttemptSummaryModel::Session => &self.primary,
            AttemptSummaryModel::Explicit(invocation) => invocation,
        }
    }

    /// The redacted client-facing projection of this snapshot.
    #[must_use]
    pub fn view(&self) -> AttemptModelView {
        AttemptModelView {
            primary: self.primary.view(),
            summary: match self.summary.as_ref() {
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
    Frozen(std::sync::Arc<FrozenModelSpec>),
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
    config: std::sync::Arc<SessionModelConfig>,
    /// The invocation last resolved for `config`; absent only when this
    /// state was restored with an already unavailable selection.
    resolved: Option<std::sync::Arc<(ResolvedModelInvocation, AttemptSummaryModel)>>,
    /// Why the authority's catalog does not admit `config`.
    unavailable: Option<std::sync::Arc<ModelInvocationError>>,
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
            config: std::sync::Arc::new(config),
            resolved: Some(std::sync::Arc::new(resolved)),
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
                config: std::sync::Arc::new(config),
                resolved: None,
                unavailable: Some(std::sync::Arc::new(error)),
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
                resolved: (config == *self.config)
                    .then(|| self.resolved.clone())
                    .flatten(),
                config: std::sync::Arc::new(config),
                unavailable: Some(std::sync::Arc::new(error)),
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
            authority: ModelAuthority::Frozen(std::sync::Arc::new(frozen.clone())),
            config: std::sync::Arc::new(frozen.configured.clone()),
            resolved: Some(std::sync::Arc::new(resolved)),
            unavailable: None,
        })
    }

    /// The authoritative desired configuration.
    #[must_use]
    pub fn config(&self) -> &SessionModelConfig {
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
    pub fn unavailable(&self) -> Option<&ModelInvocationError> {
        self.unavailable.as_deref()
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
        match (self.unavailable.as_deref(), self.resolved.as_deref()) {
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
        self.config = std::sync::Arc::new(config);
        self.resolved = Some(std::sync::Arc::new(resolved));
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
            self.unavailable = Some(std::sync::Arc::new(error));
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

    /// Capture immutable native read evidence without copying growing fields.
    pub(crate) fn read_source(&self) -> SessionModelReadSource {
        SessionModelReadSource {
            state: self.clone(),
            inline: std::sync::Arc::default(),
            binding_revision: None,
        }
    }

    /// The redacted client-facing projection of the session model state.
    #[must_use]
    pub fn view(&self) -> SessionModelView {
        SessionModelView {
            configured: (*self.config).clone(),
            effective: self.resolved.as_deref().map(|(primary, _)| primary.view()),
            summary: self.resolved.as_deref().map(|(_, summary)| match summary {
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
#[derive(Debug, Clone)]
pub(crate) struct SessionModelReadSource {
    state: SessionModelState,
    inline: std::sync::Arc<std::sync::OnceLock<Option<SessionModelView>>>,
    // Only the native Runtime owns this revision. Standalone catalog/config
    // sources have no Runtime binding and cannot authorize an ordinal mutation.
    binding_revision: Option<u64>,
}

impl SessionModelReadSource {
    pub(crate) fn at_binding_revision(mut self, revision: u64) -> Self {
        self.binding_revision = Some(revision);
        self
    }
    pub(crate) fn prepare(&self) {
        self.inline.get_or_init(|| {
            serde_json::from_slice(&crate::observation::bounded::bounded_item_bytes(
                self, 16_384,
            )?)
            .ok()
        });
    }
    pub(crate) fn available(&self) -> bool {
        self.state.unavailable.is_none() && self.state.resolved.is_some()
    }
    pub(crate) fn status(&self) -> crate::runtime_client::snapshot::RuntimeClientModelStatus {
        crate::runtime_client::snapshot::RuntimeClientModelStatus {
            available: self.available(),
            body_deferred: self.inline().is_none(),
            binding_revision: self.binding_revision,
        }
    }
    pub(crate) fn inline(&self) -> Option<SessionModelView> {
        self.inline.get().and_then(Clone::clone)
    }
    pub(crate) fn selected_text(&self, path: &[String]) -> Option<&str> {
        match path.first()?.as_str() {
            "configured" => match path.get(1)?.as_str() {
                "model" if path.len() == 2 => Some(self.state.config.model.as_str()),
                "profile" if path.len() == 2 => self
                    .state
                    .config
                    .profile
                    .as_ref()
                    .map(super::catalog::ModelProfileId::as_str),
                "requestParams" => parameter_text(&self.state.config.request_params, &path[2..]),
                _ => None,
            },
            "effective" => {
                let (primary, _) = self.state.resolved.as_deref()?;
                invocation_text(primary, &path[1..])
            }
            "summary" => {
                let (_, summary) = self.state.resolved.as_deref()?;
                match summary {
                    AttemptSummaryModel::Session => None,
                    AttemptSummaryModel::Explicit(invocation) => {
                        invocation_text(invocation, &path[1..])
                    }
                }
            }
            _ => None,
        }
    }
    pub(crate) fn capabilities(&self) -> Option<&crate::model::catalog::ModelCapabilities> {
        self.state
            .resolved
            .as_deref()
            .map(|(primary, _)| primary.capabilities())
    }
    pub(crate) fn catalog_source(&self) -> ModelCatalogReadSource {
        ModelCatalogReadSource(self.state.authority.clone())
    }
}

impl Serialize for SessionModelReadSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut view = serializer.serialize_struct("SessionModelView", 4)?;
        view.serialize_field("configured", self.state.config.as_ref())?;
        if let Some((primary, summary)) = self.state.resolved.as_deref() {
            view.serialize_field("effective", &primary.read_view())?;
            view.serialize_field("summary", &SummaryReadView(summary))?;
        }
        if let Some(error) = self.state.unavailable.as_deref() {
            view.serialize_field("unavailable", &DisplayReadView(error))?;
        }
        view.end()
    }
}

impl schemars::JsonSchema for SessionModelReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SessionModelReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<SessionModelView>()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AttemptModelReadSource {
    snapshot: AttemptModelSnapshot,
    inline: std::sync::Arc<std::sync::OnceLock<Option<AttemptModelView>>>,
}
impl AttemptModelReadSource {
    pub(crate) fn new(snapshot: AttemptModelSnapshot) -> Self {
        Self {
            snapshot,
            inline: std::sync::Arc::default(),
        }
    }
    pub(crate) fn selected_text(&self, path: &[String]) -> Option<&str> {
        match path.first()?.as_str() {
            "primary" => invocation_text(self.snapshot.primary(), &path[1..]),
            "summary" => match self.snapshot.summary_policy() {
                AttemptSummaryModel::Session => None,
                AttemptSummaryModel::Explicit(invocation) => {
                    invocation_text(invocation, &path[1..])
                }
            },
            _ => None,
        }
    }
    pub(crate) fn prepare(&self) {
        self.inline.get_or_init(|| {
            serde_json::from_slice(&crate::observation::bounded::bounded_item_bytes(
                self, 16_384,
            )?)
            .ok()
        });
    }
    pub(crate) fn inline(&self) -> Option<AttemptModelView> {
        self.inline.get().and_then(Clone::clone)
    }
}
impl Serialize for AttemptModelReadSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut view = serializer.serialize_struct("AttemptModelView", 2)?;
        view.serialize_field("primary", &self.snapshot.primary().read_view())?;
        view.serialize_field("summary", &SummaryReadView(self.snapshot.summary_policy()))?;
        view.end()
    }
}
impl schemars::JsonSchema for AttemptModelReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AttemptModelReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<AttemptModelView>()
    }
}
#[derive(Debug, Clone)]
pub(crate) struct ModelCatalogReadSource(ModelAuthority);
impl ModelCatalogReadSource {
    pub(crate) fn selected_text(&self, path: &[String]) -> Option<&str> {
        let [models, model, rest @ ..] = path else {
            return None;
        };
        if models != "models" {
            return None;
        }
        let ordinal = model.strip_prefix('@')?.parse::<u64>().ok()?;
        let row = self.model_at(ordinal)?;
        match rest {
            [key] if key == "model" => Some(row.model.as_str()),
            [key] if key == "defaultProfile" => row
                .default_profile
                .map(super::catalog::ModelProfileId::as_str),
            [key, field] if key == "credentialSource" && field == "variable" => {
                match row.credential_source {
                    super::catalog::CredentialReadView::Environment { variable } => Some(variable),
                    super::catalog::CredentialReadView::Literal => None,
                }
            }
            [profiles, index, key] if profiles == "profiles" && key == "id" => self
                .profile_at(ordinal, index.strip_prefix('@')?.parse::<u64>().ok()?)
                .map(|profile| profile.id.as_str()),
            _ => None,
        }
    }
    pub(crate) fn model_header_at(
        &self,
        ordinal: u64,
    ) -> Option<super::catalog::NativeCatalogModelHeader<'_>> {
        let mut model = self.model_at(ordinal)?;
        let total = self.profile_len(ordinal)?;
        let mut profiles = Vec::with_capacity(total.min(32));
        for offset in 0..total.min(32) {
            let row = self.profile_at(ordinal, offset as u64)?;
            if row.id.as_str().len() <= 256 {
                profiles.push(row);
            }
        }
        let profiles_deferred = profiles.len() != total;
        model.profiles = super::catalog::ProfilesReadView::Header(profiles);
        Some(super::catalog::NativeCatalogModelHeader {
            model,
            profile_count: total as u64,
            profiles_deferred,
        })
    }
    pub(crate) fn model_at(
        &self,
        ordinal: u64,
    ) -> Option<super::catalog::CatalogModelReadView<'_>> {
        let ordinal = usize::try_from(ordinal).ok()?;
        match &self.0 {
            ModelAuthority::Catalog(registry) => registry.catalog().catalog().model_at(ordinal),
            ModelAuthority::Frozen(frozen) => frozen.catalog_model_at(ordinal),
        }
    }
    pub(crate) fn profile_len(&self, model: u64) -> Option<usize> {
        let model = usize::try_from(model).ok()?;
        match &self.0 {
            ModelAuthority::Catalog(registry) => registry.catalog().catalog().profile_count(model),
            ModelAuthority::Frozen(frozen) => frozen
                .catalog_model_at(model)
                .map(|row| usize::from(row.default_profile.is_some())),
        }
    }
    pub(crate) fn profile_at(
        &self,
        model: u64,
        ordinal: u64,
    ) -> Option<super::catalog::CatalogProfileReadView<'_>> {
        let model = usize::try_from(model).ok()?;
        match &self.0 {
            ModelAuthority::Catalog(registry) => registry
                .catalog()
                .catalog()
                .profile_at(model, usize::try_from(ordinal).ok()?),
            ModelAuthority::Frozen(frozen) => {
                if ordinal != 0 {
                    return None;
                }
                let row = frozen.catalog_model_at(model)?;
                let super::catalog::ProfilesReadView::Frozen(Some(id)) = row.profiles else {
                    return None;
                };
                Some(super::catalog::CatalogProfileReadView {
                    id,
                    max_output_tokens: None,
                })
            }
        }
    }
    pub(crate) fn model_count(&self) -> usize {
        match &self.0 {
            ModelAuthority::Catalog(registry) => registry.catalog().catalog().model_count(),
            ModelAuthority::Frozen(frozen) => {
                1 + usize::from(
                    matches!(&frozen.summary,crate::model::frozen::FrozenSummaryModel::Explicit(invocation) if invocation.model!=frozen.primary.model),
                )
            }
        }
    }
}
impl Serialize for ModelCatalogReadSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            ModelAuthority::Catalog(registry) => registry
                .catalog()
                .catalog()
                .read_view()
                .serialize(serializer),
            ModelAuthority::Frozen(frozen) => frozen.read_catalog_view().serialize(serializer),
        }
    }
}
impl schemars::JsonSchema for ModelCatalogReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ModelCatalogReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<ModelCatalogView>()
    }
}

fn invocation_text<'a>(
    invocation: &'a ResolvedModelInvocation,
    path: &[String],
) -> Option<&'a str> {
    match path.first()?.as_str() {
        "modelRef" if path.len() == 1 => Some(invocation.model_ref().as_str()),
        "profile" if path.len() == 1 => invocation
            .profile()
            .map(super::catalog::ModelProfileId::as_str),
        "requestParams" => parameter_text(invocation.request_params(), &path[1..]),
        _ => None,
    }
}
fn parameter_text<'a>(params: &'a RequestParams, path: &[String]) -> Option<&'a str> {
    // Ordinal object selection is handled by the immutable indexed source.
    // Named paths borrow the native map in logarithmic work, then return exact bytes.
    let first = path.first()?;
    if first.starts_with('@') {
        return None;
    }
    let mut value = params.get(first)?;
    for key in &path[1..] {
        value = match value {
            serde_json::Value::Object(map) if !key.starts_with('@') => map.get(key)?,
            serde_json::Value::Array(items) => {
                items.get(key.strip_prefix('@').unwrap_or(key).parse::<usize>().ok()?)?
            }
            _ => return None,
        };
    }
    value.as_str()
}

struct SummaryReadView<'a>(&'a AttemptSummaryModel);
#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum BorrowedSummary<'a> {
    Session,
    Explicit(super::invocation::ModelInvocationReadView<'a>),
}
impl Serialize for SummaryReadView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            AttemptSummaryModel::Session => BorrowedSummary::Session.serialize(serializer),
            AttemptSummaryModel::Explicit(invocation) => {
                BorrowedSummary::Explicit(invocation.read_view()).serialize(serializer)
            }
        }
    }
}
struct DisplayReadView<'a>(&'a dyn std::fmt::Display);
impl Serialize for DisplayReadView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self.0)
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
    fn t08_t15_model_source_keeps_original_bytes_and_defers_large_native_values() {
        let mut frozen = crate::model::frozen::test_frozen_model_spec(
            ModelRef::parse("test/source-model").unwrap(),
        );
        let original = "🦀\nquoted \" bytes ".repeat(180_000);
        frozen.primary.request_params.insert(
            "native_payload".into(),
            serde_json::Value::String(original.clone()),
        );
        frozen.configured.request_params.insert(
            "native_payload".into(),
            serde_json::Value::String(original.clone()),
        );
        let state = SessionModelState::frozen(
            &frozen,
            &crate::model::catalog::MapCredentialEnvironment::new([]),
        )
        .unwrap();
        let source = state.read_source();
        let cloned = source.clone();
        assert!(std::sync::Arc::ptr_eq(&state.config, &cloned.state.config));
        assert!(std::sync::Arc::ptr_eq(
            state.resolved.as_ref().unwrap(),
            cloned.state.resolved.as_ref().unwrap()
        ));
        source.prepare();
        assert!(
            source.inline().is_none(),
            "large desired configuration has an explicit deferred model header"
        );
        assert_eq!(
            source.status(),
            crate::runtime_client::snapshot::RuntimeClientModelStatus {
                available: true,
                body_deferred: true,
                binding_revision: None
            }
        );
        let path = vec![
            "configured".into(),
            "requestParams".into(),
            "native_payload".into(),
        ];
        let borrowed = source.selected_text(&path).unwrap();
        assert_eq!(borrowed, original);
        let first = crate::runtime::read_layout::text_chunk(borrowed, 1, 65_536).unwrap();
        assert_eq!(first.bytes, &original.as_bytes()[1..65_537]);
        assert_eq!(first.next_offset, Some(65_537));
        assert_eq!(first.readable_end, original.len() as u64);
        assert!(
            crate::runtime::read_layout::text_chunk(borrowed, original.len() as u64 + 1, 1)
                .is_err()
        );
        assert_eq!(source.catalog_source().model_count(), 1);
    }

    #[test]
    fn t11_t19_model_source_defers_opaque_wide_parameters_without_rewriting_original_defaults() {
        let mut frozen = crate::model::frozen::test_frozen_model_spec(
            ModelRef::parse("test/source-model").unwrap(),
        );
        let original = serde_json::json!({"opaque_integer": u64::MAX});
        frozen
            .primary
            .request_params
            .insert("opaque".into(), original.clone());
        frozen
            .configured
            .request_params
            .insert("opaque".into(), original.clone());
        let state = SessionModelState::frozen(
            &frozen,
            &crate::model::catalog::MapCredentialEnvironment::new([]),
        )
        .unwrap();
        let source = state.read_source();
        source.prepare();
        assert!(source.available());
        assert!(
            source.inline().is_none(),
            "opaque wide integers need an exact byte view, not a rounded editable object"
        );
        assert_eq!(state.config().request_params.get("opaque"), Some(&original));
        assert_eq!(serde_json::to_value(&source).unwrap()["configured"]["requestParams"]["opaque"]["opaque_integer"].as_u64(), Some(u64::MAX));
        let attempt = state.snapshot().unwrap();
        let source = AttemptModelReadSource::new(attempt);
        source.prepare();
        assert!(source.inline().is_none());
        assert_eq!(serde_json::to_value(source).unwrap()["primary"]["requestParams"]["opaque"]["opaque_integer"].as_u64(), Some(u64::MAX));
    }

    #[test]
    fn t14_borrowed_model_and_catalog_encoding_matches_native_redacted_view() {
        let frozen = crate::model::frozen::test_frozen_model_spec(
            ModelRef::parse("test/source-model").unwrap(),
        );
        let state = SessionModelState::frozen(
            &frozen,
            &crate::model::catalog::MapCredentialEnvironment::new([]),
        )
        .unwrap();
        let source = state.read_source();
        source.prepare();
        assert_eq!(source.inline(), Some(state.view()));
        assert_eq!(
            serde_json::to_value(&source).unwrap(),
            serde_json::to_value(state.view()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(source.catalog_source()).unwrap(),
            serde_json::to_value(state.catalog_view()).unwrap()
        );
        let attempt = state.snapshot().unwrap();
        let source = AttemptModelReadSource::new(attempt.clone());
        source.prepare();
        assert_eq!(source.inline(), Some(attempt.view()));
        assert_eq!(
            serde_json::to_value(source).unwrap(),
            serde_json::to_value(attempt.view()).unwrap()
        );
    }

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
