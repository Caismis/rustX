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
//! After admission every model turn of the attempt — every tool→model
//! continuation, every context-overflow retry, every proactive-compaction
//! continuation, and every compaction summary — uses the same immutable
//! snapshot. It never reads live mutable session model state again.
//!
//! # Summary policy
//!
//! Production supports exactly two modes, and both resolve through the same
//! catalog, credential binding, compat handling, reasoning-profile
//! validation, protected-key validation, and shallow overlay as a primary
//! model:
//!
//! - `session` — the summary uses the attempt's frozen primary invocation,
//!   subject only to the context plane's summary output safety cap, which is
//!   applied through the runtime-owned protected max-output field and never
//!   by mutating a reasoning profile or a request-parameter object;
//! - `explicit` — a separately resolved catalog model, frozen at admission
//!   so a later mutation of live session state cannot change the summary
//!   model of an already-admitted attempt.

use serde::{Deserialize, Serialize};

use crate::model::catalog::{ModelCatalogView, ModelRef, ReasoningProfileId};
use crate::model::invocation::{
    ModelBindingRegistry, ModelInvocationError, ModelInvocationView, ModelSelection, RequestParams,
    RequestParamsLayer, ResolvedModelInvocation,
};

/// The compaction summary model policy of a session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SummaryModelPolicy {
    /// Summary generation follows the admitted attempt's primary model.
    #[default]
    Session,
    /// Summary generation uses an explicitly configured catalog model.
    Explicit {
        /// The catalog model reference.
        model: ModelRef,
        /// The selected reasoning profile; the model default is used when
        /// absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_profile: Option<ReasoningProfileId>,
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
pub struct SessionModelConfig {
    /// The selected catalog model.
    pub model: ModelRef,
    /// The selected reasoning profile; the model default is used when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// The session request-parameter overrides.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The session output-budget override; the model's configured maximum is
    /// used when absent.
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
            reasoning_profile: None,
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
            reasoning_profile: self.reasoning_profile.clone(),
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
                reasoning_profile,
                request_params,
                max_output_tokens,
            } => Some(ModelSelection {
                model: model.clone(),
                reasoning_profile: reasoning_profile.clone(),
                request_params: request_params.clone(),
                max_output_tokens: *max_output_tokens,
            }),
        }
    }
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
    /// provider binding, model, protocol, reasoning profile, and effective
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

/// The session's model authority: the binding registry plus the current
/// desired configuration and its resolution.
///
/// Updates are transactional: a failed update changes nothing, so a caller
/// can publish a model-change observation exactly when `apply` returns
/// `Ok`.
#[derive(Debug, Clone)]
pub struct SessionModelState {
    registry: ModelBindingRegistry,
    config: SessionModelConfig,
    primary: ResolvedModelInvocation,
    summary: AttemptSummaryModel,
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
        let (primary, summary) = resolve(&registry, &config)?;
        Ok(Self {
            registry,
            config,
            primary,
            summary,
        })
    }

    /// The authoritative desired configuration.
    #[must_use]
    pub const fn config(&self) -> &SessionModelConfig {
        &self.config
    }

    /// The binding registry behind this session.
    #[must_use]
    pub const fn registry(&self) -> &ModelBindingRegistry {
        &self.registry
    }

    /// The safe public catalog view.
    #[must_use]
    pub fn catalog_view(&self) -> ModelCatalogView {
        self.registry.catalog_view()
    }

    /// Freezes the current configuration into an attempt model snapshot.
    ///
    /// This is a cheap clone of values resolved when the configuration was
    /// last accepted, so it is safe to call under the admission
    /// linearization lock.
    #[must_use]
    pub fn snapshot(&self) -> AttemptModelSnapshot {
        AttemptModelSnapshot::new(self.primary.clone(), self.summary.clone())
    }

    /// Applies a whole-state configuration replacement transactionally.
    ///
    /// # Errors
    ///
    /// Returns the first resolution failure; on failure this state is
    /// completely unchanged and no model-change observation may be
    /// published.
    pub fn apply(&mut self, config: SessionModelConfig) -> Result<(), ModelInvocationError> {
        let (primary, summary) = resolve(&self.registry, &config)?;
        self.config = config;
        self.primary = primary;
        self.summary = summary;
        Ok(())
    }

    /// The redacted client-facing projection of the session model state.
    #[must_use]
    pub fn view(&self) -> SessionModelView {
        SessionModelView {
            configured: self.config.clone(),
            effective: self.primary.view(),
            summary: match &self.summary {
                AttemptSummaryModel::Session => SummaryModelView::Session,
                AttemptSummaryModel::Explicit(invocation) => {
                    SummaryModelView::Explicit(Box::new(invocation.view()))
                }
            },
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
pub struct SessionModelView {
    /// The authoritative desired configuration, exactly as a client would
    /// send it back through `model_set`.
    pub configured: SessionModelConfig,
    /// The resolved effective primary invocation.
    pub effective: ModelInvocationView,
    /// The resolved summary policy.
    pub summary: SummaryModelView,
}

impl SessionModelView {
    /// The attempt view an attempt admitted with exactly this session state
    /// would freeze.
    #[must_use]
    pub fn to_attempt_view(&self) -> AttemptModelView {
        AttemptModelView {
            primary: self.effective.clone(),
            summary: self.summary.clone(),
        }
    }
}

/// The redacted client-facing projection of a summary policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
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
pub struct AttemptModelView {
    /// The attempt's frozen primary invocation.
    pub primary: ModelInvocationView,
    /// The attempt's frozen summary policy.
    pub summary: SummaryModelView,
}
