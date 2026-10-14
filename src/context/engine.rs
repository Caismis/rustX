//! Deterministic compilation, token accounting and compaction planning.
//!
//! The engine consumes bounded typed historical input: immutable Original
//! references and identity-free Derived messages selected by the durable
//! Projection Head, followed by eligible canonical Tail strictly after its base.
//! It owns structural validation, retention, token pressure and fit/progress
//! checks. The durable store owns checkpoint/head/receipt/completion atomicity;
//! the Agent Loop owns current core input, cancellation and actual request starts.
//! Provider adapters alone own native wire translation and continuation rules.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::tokens::{ProviderObservedInput, TokenEstimator};
use crate::conversation::SurfaceRevision;
use crate::message::types::MessageBlock;
use crate::model::input::{
    ModelInputMessage, RequestOnlyInsertionAnchor, assemble_model_input, canonical_input,
};
use crate::runtime::identity::MessageId;
use crate::runtime::inbound::FreshInboundTurn;
use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use crate::tools::types::ModelToolDefinition;

/// The static current-runtime context policy.
///
/// A conversation session owns the *policy* — the safety reserve, the
/// uncompressed recent-history target, and the summary output safety cap —
/// but it deliberately does **not** own a context window. The context window
/// belongs to the model, and the session model may change between attempts,
/// so the effective [`ContextConfig`] of an attempt is derived from this
/// policy plus that attempt's immutable model snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContextPolicy {
    /// Tokens permanently reserved out of whichever model context window is
    /// in force.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation content kept uncompressed. This is a
    /// token target, never a message count target.
    pub keep_recent_tokens: u64,
    /// The context plane's summary/output safety cap, when it imposes one.
    pub summary_output_cap: Option<u32>,
}

impl SessionContextPolicy {
    /// Validate primary and summary budgets without binding a provider.
    ///
    /// # Errors
    /// Rejects impossible input budgets and a zero summary cap.
    pub fn validate_budgets(
        &self,
        primary: (u64, u32),
        summary: (u64, u32),
    ) -> Result<(), ContextError> {
        if self.summary_output_cap == Some(0) {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                "summary_output_cap must be positive when present",
            ));
        }
        self.config_for_window(primary.0)
            .soft_input_limit(primary.1)?;
        let summary_output = self
            .summary_output_cap
            .map_or(summary.1, |cap| cap.min(summary.1));
        self.config_for_window(summary.0)
            .soft_input_limit(summary_output)?;
        Ok(())
    }

    /// Derives the attempt context configuration for one model context
    /// window.
    #[must_use]
    pub const fn config_for_window(&self, context_window_tokens: u64) -> ContextConfig {
        ContextConfig {
            context_window_tokens,
            reserve_tokens: self.reserve_tokens,
            keep_recent_tokens: self.keep_recent_tokens,
        }
    }
}

/// The runtime-owned context configuration of one attempt.
///
/// The soft input limit of one request is derived explicitly and checked:
///
/// ```text
/// soft_input_limit = context_window_tokens - reserve_tokens - max_output_tokens
/// ```
///
/// Automatic compaction triggers at `estimated_input >= soft_input_limit`
/// (equality compacts). Impossible configurations are rejected; no fallback
/// constant is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextConfig {
    /// The model context window in tokens.
    pub context_window_tokens: u64,
    /// Tokens permanently reserved out of the model context window.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation content kept uncompressed. This is a
    /// token target, never a message count target.
    pub keep_recent_tokens: u64,
}

/// The budgets compaction planning must keep distinct.
///
/// The primary model's output budget determines how much input the next
/// primary request may carry. The frozen summary invocation's output budget
/// is a reservation for the summary that will be generated while applying
/// the plan, and its **input** limit bounds how large a Surface span may be
/// serialized into one summary request: an arbitrarily large span can never
/// become an impossible summary-model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionBudgets {
    /// The primary invocation's effective maximum output tokens.
    pub primary_output_budget: u32,
    /// The frozen/capped summary invocation's effective maximum output
    /// tokens.
    pub summary_output_budget: u32,
    /// The frozen summary invocation's effective input limit: the largest
    /// selected span estimate one summary request may carry.
    ///
    /// This budget is never scaled by an [`EstimateCorrection`]. A
    /// correction is the ratio between one *primary* request's estimate and
    /// what the provider counted for that same request — a measurement of
    /// that request, not a calibration of a tokenizer. The two requests do
    /// not carry the same material: the summary request has no tools, no
    /// Agent Status, no Skill catalog, and no provider continuation, and a
    /// stored continuation alone can put six figures of provider-counted
    /// input behind the primary request that the summary request will never
    /// send. Reusing the ratio would shrink a summary budget on evidence
    /// about a different request — possibly to `CannotFit` — even when both
    /// requests go to the same model. A summary request that really is too
    /// large is rejected by the summary model, and the bounded shrink loop
    /// around this plan replans against *that* measurement.
    pub summary_input_limit: u64,
}

impl CompactionBudgets {
    /// Creates the budgets for one admitted attempt.
    #[must_use]
    pub const fn new(
        primary_output_budget: u32,
        summary_output_budget: u32,
        summary_input_limit: u64,
    ) -> Self {
        Self {
            primary_output_budget,
            summary_output_budget,
            summary_input_limit,
        }
    }
}

impl ContextConfig {
    /// Derives the soft input limit of one request with the given
    /// runtime-resolved output budget, using checked arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] when the window
    /// leaves no positive effective input budget.
    pub fn soft_input_limit(&self, max_output_tokens: u32) -> Result<u64, ContextError> {
        let output = u64::from(max_output_tokens);
        if self.context_window_tokens <= self.reserve_tokens {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                format!(
                    "context_window_tokens {} must exceed reserve_tokens {}",
                    self.context_window_tokens, self.reserve_tokens
                ),
            ));
        }
        let remaining = self.context_window_tokens - self.reserve_tokens;
        if remaining <= output {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                format!(
                    "context_window_tokens {} must exceed reserve_tokens {} + max_output_tokens {}",
                    self.context_window_tokens, self.reserve_tokens, output
                ),
            ));
        }
        Ok(remaining - output)
    }
}

/// The structural constraints one compaction plan must satisfy.
///
/// The two reference constraints serve opposite purposes and are kept
/// separate from the hard-fit decision:
///
/// ```text
/// must_cover_through → successful compaction must retire through this
/// fresh_inbound      → successful compaction must not retire this or
///                      anything after it
/// ```
#[derive(Debug, Clone, Default)]
pub struct CompactionConstraints<'a> {
    /// The continuation constraint: the span must retire the
    /// continuation-owning turn completely.
    pub must_cover_through: Option<&'a MessageId>,
    /// The fresh-inbound retention constraint: unobserved fresh inbound
    /// material must remain active.
    pub fresh_inbound: Option<&'a FreshInboundTurn>,
    /// Core-owned current generation Context; an algorithm cannot summarize it away.
    pub protected_context_ids: &'a [MessageId],
    /// The **staged, not-yet-committed** request-scoped context blocks the
    /// upcoming request will carry in addition to the post-compaction
    /// historical projection (Issue #12, M9b).
    ///
    /// Request-scoped context commits only inside the durable model-turn
    /// start transaction, so a compaction decided while such context is
    /// staged plans over a historical projection that does not contain it. Every candidate
    /// is therefore evaluated against the exact hypothetical post-compaction
    /// request — the retained projection plus this overlay plus the Effective
    /// System Prompt plus tools — through the same [`TokenEstimator`], never
    /// as a scalar token delta: the estimator has no additive or monotonic
    /// contract, so a staged-context token reservation is not a reusable
    /// quantity across different candidate projections.
    ///
    /// The staged overlay is neither canonical nor retirable: it is not on
    /// the history the planner selects, and it exists only for exact planning
    /// of the upcoming request.
    pub staged_request_context: &'a [MessageBlock],
    /// The frozen bounded request-only carryover admitted to the upcoming
    /// request, if any. It is fit-tested as the exact provider-neutral item
    /// rather than as a guessed scalar reservation.
    pub carryover: Option<&'a crate::model::RenderedUnresolvedOutputCarryover>,
    /// The frozen canonical anchor of that request-only item.
    pub carryover_anchor: Option<&'a RequestOnlyInsertionAnchor>,
    /// The measured error of this runtime's deterministic token estimate,
    /// when a provider has just proven the estimate too optimistic.
    ///
    /// A compaction that recovers from a context overflow must not plan
    /// against the very budget the provider just rejected: the estimator
    /// said the failed request fit, and it did not. When present, every
    /// runtime-owned budget of this plan — the soft input limit and the
    /// summary model's input limit — is scaled into the estimate space the
    /// observed error implies, so the post-compaction request targets a
    /// genuinely smaller provider request rather than the same one again.
    pub estimate_correction: Option<EstimateCorrection>,
}

/// The measured error of one deterministic token estimate.
///
/// The runtime's [`TokenEstimator`] is deliberately provider-neutral and
/// therefore approximate. A provider rejection for context size is the one
/// authoritative measurement of that approximation error, and this value
/// carries it as an exact integer ratio — never a float, never a fabricated
/// provider usage measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstimateCorrection {
    /// What the runtime estimated for the rejected request.
    pub estimated_tokens: u64,
    /// What the provider counted for that same request.
    pub observed_tokens: u64,
}

impl EstimateCorrection {
    /// The correction applied when a provider rejected a request as too
    /// large but reported no token count: plan for a request a quarter
    /// smaller than the estimate that was already proven wrong.
    pub const UNQUANTIFIED: Self = Self {
        estimated_tokens: 3,
        observed_tokens: 4,
    };

    /// The correction implied by one provider-reported measurement, or
    /// `None` when the measurement carries no usable error (a zero estimate,
    /// or an observation that does not exceed the estimate).
    #[must_use]
    pub const fn new(estimated_tokens: u64, observed_tokens: u64) -> Option<Self> {
        if estimated_tokens == 0 || observed_tokens <= estimated_tokens {
            return None;
        }
        Some(Self {
            estimated_tokens,
            observed_tokens,
        })
    }

    /// Scales one runtime-owned token budget into the estimate space this
    /// correction implies. The result is never zero: a corrected budget
    /// still has to admit a plan.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "estimated_tokens < observed_tokens by construction, so the scaled value \
                  never exceeds `budget` and always fits `u64`"
    )]
    pub const fn apply(self, budget: u64) -> u64 {
        let scaled =
            (budget as u128 * self.estimated_tokens as u128) / self.observed_tokens as u128;
        if scaled == 0 { 1 } else { scaled as u64 }
    }
}

/// The deterministic context engine.
#[derive(Clone)]
pub struct ContextEngine {
    image_input: Option<bool>,
    uploads: Option<Arc<dyn crate::model::uploads::UploadProjectionResolver>>,
    config: ContextConfig,
    estimator: Arc<dyn TokenEstimator>,
}

impl core::fmt::Debug for ContextEngine {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ContextEngine")
            .field("config", &self.config)
            .field("uploads", &self.uploads)
            .field("image_input", &self.image_input)
            .field("estimator", &"<opaque token estimator>")
            .finish()
    }
}

impl ContextEngine {
    /// Creates an engine over one runtime-owned configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] when the
    /// configuration leaves no positive effective input budget even before
    /// any output budget.
    pub fn new(
        config: ContextConfig,
        estimator: Arc<dyn TokenEstimator>,
    ) -> Result<Self, ContextError> {
        if config.context_window_tokens <= config.reserve_tokens {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                format!(
                    "context_window_tokens {} must exceed reserve_tokens {}",
                    config.context_window_tokens, config.reserve_tokens
                ),
            ));
        }
        Ok(Self {
            config,
            estimator,
            uploads: None,
            image_input: None,
        })
    }

    pub(crate) fn set_image_projection(
        &mut self,
        artifacts: crate::tools::artifacts::ArtifactStore,
        enabled: bool,
    ) {
        self.estimator = Arc::new(super::images::ImageEstimator {
            inner: self.estimator.clone(),
            artifacts,
            enabled,
        });
        self.image_input = Some(enabled);
    }

    pub(crate) fn set_upload_resolver(
        &mut self,
        owner: Arc<dyn crate::model::uploads::UploadProjectionResolver>,
    ) {
        self.estimator = Arc::new(UploadEstimator {
            inner: self.estimator.clone(),
            owner: owner.clone(),
        });
        self.uploads = Some(owner);
    }
    pub(crate) fn project_uploads(
        &self,
        messages: &mut [ModelInputMessage],
    ) -> Result<crate::model::uploads::UploadProjection, ContextError> {
        let projection = match &self.uploads {
            Some(owner) => owner.resolve(messages).map_err(|e| malformed(&e))?,
            None => crate::model::uploads::UploadProjection::default(),
        };
        projection.apply(messages).map_err(|e| malformed(&e))?;
        Ok(projection)
    }

    /// The engine configuration.
    #[must_use]
    pub fn config(&self) -> &ContextConfig {
        &self.config
    }

    /// The soft input limit of one request with the given output budget.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] for an impossible
    /// effective input budget.
    pub fn soft_input_limit(&self, max_output_tokens: u32) -> Result<u64, ContextError> {
        self.config.soft_input_limit(max_output_tokens)
    }

    /// Whether the given projection requires automatic compaction for a
    /// request with the given output budget: `estimated >= soft limit`.
    /// Equality is deterministic: at the threshold, compact.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] for an impossible
    /// effective input budget.
    pub fn should_compact(
        &self,
        projection: &ContextProjection,
        max_output_tokens: u32,
    ) -> Result<bool, ContextError> {
        Ok(projection.estimated_input.input_tokens >= self.soft_input_limit(max_output_tokens)?)
    }

    /// Whether the projection fits under the soft input limit of a request
    /// with the given output budget.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] for an impossible
    /// effective input budget.
    pub fn fits_under_soft_limit(
        &self,
        projection: &ContextProjection,
        max_output_tokens: u32,
    ) -> Result<bool, ContextError> {
        Ok(projection.estimated_input.input_tokens < self.soft_input_limit(max_output_tokens)?)
    }

    /// The exact estimated input of one hypothetical post-compaction
    /// request: the given post-compaction projection's messages plus the
    /// non-retirable staged request-scoped context, measured by the same
    /// estimator over the exact Effective System Prompt and tool definitions
    /// (Issue #12, M9b).
    ///
    /// The staged overlay is never folded into the projection canonically:
    /// it is transient planning input evaluated here only to decide whether
    /// the upcoming request fits after a candidate compaction. The result is
    /// a full request estimate, never a scalar token delta, so the fit
    /// decision respects the estimator's exact (possibly non-additive,
    /// non-monotonic) behavior. The estimator sees only the provider-visible
    /// messages plus the exact Effective System Prompt and tools; the
    /// projection's `SurfaceRevision` and measurement provenance are outside
    /// the estimation boundary.
    #[must_use]
    pub fn estimate_with_staged_context(
        &self,
        projection: &ContextProjection,
        staged_request_context: &[MessageBlock],
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        self.estimate_with_staged_context_and_carryover(
            projection,
            staged_request_context,
            tool_definitions,
            None,
            None,
        )
    }

    /// Estimates the exact post-compaction request including its frozen
    /// request-only carryover, when one is admitted.
    ///
    /// # Panics
    ///
    /// Panics only when a supplied frozen anchor is absent from the assembled
    /// request; request preparation validates anchors before calling this
    /// method.
    #[must_use]
    pub fn estimate_with_staged_context_and_carryover(
        &self,
        projection: &ContextProjection,
        staged_request_context: &[MessageBlock],
        tool_definitions: &[ModelToolDefinition],
        carryover: Option<&crate::model::RenderedUnresolvedOutputCarryover>,
        carryover_anchor: Option<&RequestOnlyInsertionAnchor>,
    ) -> u64 {
        let messages = assemble_model_input(
            &projection.messages,
            staged_request_context,
            carryover,
            carryover_anchor,
        )
        .expect("the context engine receives a valid frozen carryover anchor");
        self.estimator.estimate_input(
            &messages,
            &projection.effective_system_prompt,
            tool_definitions,
        )
    }

    /// Estimates an already assembled provider-neutral request input. This
    /// boundary is used for the actual mixed canonical/request-only request;
    /// publication carryover is therefore charged at the exact detail level
    /// that the request admits.
    #[must_use]
    pub fn estimate_model_input(
        &self,
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        self.estimator
            .estimate_input(messages, effective_system_prompt, tool_definitions)
    }

    pub(crate) fn supports_monotone_compaction(&self, messages: &[ModelInputMessage]) -> bool {
        self.estimator.supports_monotone_compaction(messages)
    }

    /// Price the frozen provider-neutral request after dynamic materialization.
    #[must_use]
    pub fn estimate_frozen_request(&self, request: &crate::model::ModelRequest) -> u64 {
        self.estimator.estimate_frozen_request(request)
    }

    /// Measure an already structurally compiled historical projection and core overlay.
    #[must_use]
    pub fn build_projection_input(
        &self,
        revision: SurfaceRevision,
        messages: Vec<ModelInputMessage>,
        tools: &[ModelToolDefinition],
        observed: Option<&ProviderObservedInput>,
        prompt: &str,
    ) -> ContextProjection {
        let mut projection = ContextProjection {
            surface_revision: revision,
            messages,
            effective_system_prompt: prompt.to_owned(),
            estimated_input: TokenMeasurement {
                input_tokens: 0,
                source: TokenMeasurementSource::Estimated,
            },
        };
        projection.estimated_input = observed
            .and_then(|o| self.reuse_measurement(o, &projection, tools))
            .unwrap_or_else(|| TokenMeasurement {
                input_tokens: self
                    .estimator
                    .estimate_input(&projection.messages, prompt, tools),
                source: TokenMeasurementSource::Estimated,
            });
        projection
    }

    /// Reuses one provider measurement for this projection, exactly or as a
    /// prefix anchor, or refuses it.
    ///
    /// The exact match is unchanged: an identical request context keeps the
    /// provider's own number with `ProviderReported` provenance. The anchored
    /// case is the one that matters over a long conversation. The provider
    /// measured the cost of a prefix of this context — including its
    /// Effective System Prompt and tool definitions — so that part needs no
    /// estimating at all, and only the canonical messages appended since are
    /// estimated. Estimator error therefore applies to one turn's tail rather
    /// than compounding across the entire history, which is what makes the
    /// soft-limit decision trustworthy exactly when the conversation is long
    /// enough for it to matter.
    ///
    /// The anchor is refused outright — never patched with a guessed delta —
    /// when the Effective System Prompt or the tool definitions changed, or
    /// when the measured identities are not an ordered prefix of this
    /// projection (which a changed checkpoint can invalidate).
    fn reuse_measurement(
        &self,
        observed: &ProviderObservedInput,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> Option<TokenMeasurement> {
        let mut rendered = projection.messages.clone();
        self.project_uploads(&mut rendered).ok()?;
        if let Some(enabled) = self.image_input {
            crate::model::images::project(&mut rendered, enabled);
        }
        let mut projected = projection.clone();
        projected.messages.clone_from(&rendered);
        if observed.fingerprint == projected.fingerprint()
            || observed.fingerprint
                == crate::context::tokens::model_input_fingerprint(
                    projection.surface_revision,
                    &rendered,
                    &projection.effective_system_prompt,
                )
        {
            return Some(TokenMeasurement {
                input_tokens: observed.input_tokens,
                source: TokenMeasurementSource::ProviderReported,
            });
        }
        let covered = observed.anchor.as_ref()?.covered_prefix_model_input(
            &rendered,
            &projection.effective_system_prompt,
            tool_definitions,
        )?;
        let appended = self
            .estimator
            .estimate_input(&projected.messages[covered..], "", &[]);
        Some(TokenMeasurement {
            input_tokens: observed.input_tokens.saturating_add(appended),
            source: TokenMeasurementSource::ProviderAnchored,
        })
    }
}

fn malformed(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::MalformedHistory, message)
}

/// Token estimation with the current upload renderer, used only during preparation.
struct UploadEstimator {
    inner: Arc<dyn TokenEstimator>,
    owner: Arc<dyn crate::model::uploads::UploadProjectionResolver>,
}
fn has_uploads(messages: &[ModelInputMessage]) -> bool {
    messages.iter().any(|message| match message.view() {
        crate::model::projected::ModelMessageView::User(content) => content.iter().any(|block| {
            matches!(
                block,
                crate::message::types::UserContentBlock::UploadedFile(_)
            )
        }),
        _ => false,
    })
}
impl TokenEstimator for UploadEstimator {
    fn supports_monotone_compaction(&self, messages: &[ModelInputMessage]) -> bool {
        // No typed upload means apply() is identity. Dynamic upload resolution
        // has no monotonicity promise; do not infer one from the inner formula.
        !has_uploads(messages) && self.inner.supports_monotone_compaction(messages)
    }

    fn estimate_input(
        &self,
        messages: &[ModelInputMessage],
        prompt: &str,
        tools: &[ModelToolDefinition],
    ) -> u64 {
        if !has_uploads(messages) {
            return self.inner.estimate_input(messages, prompt, tools);
        }
        let mut rendered = messages.to_vec();
        let Ok(projection) = self.owner.resolve(messages) else {
            return u64::MAX;
        };
        if projection.apply(&mut rendered).is_err() {
            return u64::MAX;
        }
        self.inner.estimate_input(&rendered, prompt, tools)
    }
    fn estimate_frozen_request(&self, request: &crate::model::ModelRequest) -> u64 {
        // Upload-bearing content was already rendered and frozen by the caller.
        self.inner.estimate_frozen_request(request)
    }
    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        let mut rendered = canonical_input(messages);
        let Ok(projection) = self.owner.resolve(&rendered) else {
            return u64::MAX;
        };
        if projection.apply(&mut rendered).is_err() {
            return u64::MAX;
        }
        let canonical = rendered
            .into_iter()
            .filter_map(|m| match m {
                ModelInputMessage::Canonical(m) => Some(m),
                ModelInputMessage::RequestOnly(_) | ModelInputMessage::Derived(_) => None,
            })
            .collect::<Vec<_>>();
        self.inner.estimate_conversation_input(&canonical)
    }
}
