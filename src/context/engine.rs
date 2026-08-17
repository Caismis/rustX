//! The deterministic context engine.
//!
//! Since M7.5 (Issue #54) the engine plans from the **current Conversation
//! Surface**, never from append-origin history:
//!
//! ```text
//! Surface @ revision
//!   → finite active MessageIds
//!   → keyed Ledger hydration
//!   → ContextProjection (finite, complete canonical messages only)
//!   → token pressure / retention / compaction planning
//! ```
//!
//! The engine owns token pressure, the soft-limit decision, the retention
//! target, structural compaction planning, summary planning, the fit and
//! progress checks, and the provider-neutral projection. It does **not** own
//! Ledger mutation, Surface authority, attempt cancellation, provider
//! continuation storage, or Runtime Client state: the engine prepares the
//! compaction command, while the durable `ConversationStore` owns the atomic
//! summary Ledger + Surface Replace commit and the hot state installs the
//! already-committed result.
//!
//! All decisions are deterministic pure functions of (Surface revision,
//! hydrated active messages, tool definitions, observed provider usage, and
//! Effective System Prompt): the same inputs always produce the same
//! projection, plan, and estimate.
//!
//! The engine owns no provider knowledge: the window/reserve/recent-token
//! configuration is runtime-owned, token estimation is pluggable, and no
//! model name catalog exists.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::summarizer::SummaryRequest;
use crate::context::tokens::{ProviderObservedInput, TokenEstimator};
use crate::conversation::{
    ConversationError, ConversationState, PreparedCompactionCommit, StructuralIndex,
    SurfaceRevision, SurfaceSpan, summary_message_id,
};
use crate::message::content::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::FreshInboundTurn;
use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use crate::tools::types::ModelToolDefinition;

/// The static session-owned context policy.
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

/// The deterministic plan of one compaction.
///
/// A plan is a pure function of the current Surface state; it names the
/// exact inclusive active span to replace and carries the complete canonical
/// messages of that span for summarization. Applying it produces the one
/// [`PreparedCompactionCommit`] the conversation state linearizes.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPlan {
    /// The Surface revision this plan was made against. A commit against a
    /// different revision is rejected as stale.
    pub surface_revision: SurfaceRevision,
    /// The inclusive active span to replace with the summary.
    pub span: SurfaceSpan,
    /// The complete canonical messages of the span, in Surface order.
    pub retired: Vec<MessageBlock>,
    /// The measured input of the pre-compaction request context, with its
    /// provenance. Preserved for diagnostics; the anti-loop progress rule
    /// never compares measurements of different provenance.
    pub estimated_before: TokenMeasurement,
    /// The deterministic estimate of the pre-compaction request context,
    /// computed with the same estimator and Effective System Prompt used on the other
    /// side of the progress rule.
    pub estimated_before_tokens: u64,
    /// The planned post-compaction estimate: retained context plus the
    /// summary reservation.
    pub planned_estimate_after: u64,
    /// The summary output budget reserved during planning, a conservative
    /// bound for the not-yet-generated summary.
    pub summary_reservation: u64,
    /// The exact token estimate of the assembled summary-model input,
    /// including its instruction, serialized request, and canonical User
    /// wrapper.
    pub summary_input_tokens: u64,
    /// The exact Effective System Prompt of the request preparation this
    /// plan belongs to. It is reused after compaction and is never rebuilt by
    /// a provider adapter.
    pub effective_system_prompt: String,
}

impl CompactionPlan {
    /// The summary request this plan implies.
    #[must_use]
    pub fn summary_request(&self) -> SummaryRequest {
        SummaryRequest {
            retired: self.retired.clone(),
        }
    }
}

/// The structural constraints one compaction plan must satisfy.
///
/// The two constraints serve opposite purposes and are kept separate from
/// the hard-fit decision:
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
}

/// The deterministic context engine.
#[derive(Clone)]
pub struct ContextEngine {
    config: ContextConfig,
    estimator: Arc<dyn TokenEstimator>,
}

impl core::fmt::Debug for ContextEngine {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ContextEngine")
            .field("config", &self.config)
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
        Ok(Self { config, estimator })
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

    /// Builds the current projection of one conversation state.
    ///
    /// The read path is finite by construction: the Surface answers *which*
    /// identities are active, and only those bodies are hydrated by keyed
    /// Ledger lookup. Retired Ledger history is never iterated and never
    /// hydrated, so the cost is a function of the active Surface alone.
    ///
    /// The estimated input is `ProviderReported` only when an observed
    /// provider measurement applies to exactly this request context
    /// (identical fingerprint, including the Surface revision and the exact
    /// Effective System Prompt); otherwise it is a deterministic estimate
    /// that includes the Effective System Prompt and tool definitions. Estimates
    /// never become provider usage.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] for a structurally
    /// invalid active conversation or a dangling Surface identity.
    pub fn build_projection(
        &self,
        state: &ConversationState,
        tool_definitions: &[ModelToolDefinition],
        observed: Option<&ProviderObservedInput>,
        effective_system_prompt: &str,
    ) -> Result<ContextProjection, ContextError> {
        let (messages, _) = state
            .structure()
            .map_err(|error| conversation_failed(&error))?;
        Ok(self.measured_projection(
            state.revision(),
            messages,
            effective_system_prompt,
            tool_definitions,
            observed,
        ))
    }

    /// Plans one compaction of the current Surface.
    ///
    /// The compactable region is the earliest contiguous run of non-`System`
    /// active messages. Candidate spans are the inclusive prefixes of that
    /// run; every candidate must contain complete canonical messages only,
    /// must never separate a tool call from its result, must satisfy the
    /// continuation and fresh-inbound constraints, and must fit the summary
    /// model's input limit.
    ///
    /// The deterministic priority is frozen:
    ///
    /// 1. the largest candidate whose retained recent conversation content
    ///    still meets `keep_recent_tokens`, when the resulting request fits;
    /// 2. otherwise the most-retaining candidate that fits the hard limit.
    ///
    /// The recent-token target is measured over conversation content only:
    /// tool definitions and the Effective System Prompt never count toward satisfying
    /// `keep_recent_tokens`, though they still affect the full request
    /// estimate, the threshold, and the hard fit. A single oversized message
    /// never produces a half-message Surface node: if no complete-message
    /// span fits, planning fails with [`ContextErrorKind::CannotFit`].
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] for structurally
    /// invalid active conversation, [`ContextErrorKind::NoProgress`] when
    /// nothing can be retired or the continuation constraint is
    /// unsatisfiable, and [`ContextErrorKind::CannotFit`] when no complete
    /// message span produces a fitting request.
    ///
    /// # Panics
    ///
    /// Panics only if the chosen span indices fall outside the Surface the
    /// candidates were derived from, which is unreachable by construction.
    pub fn plan_compaction(
        &self,
        state: &ConversationState,
        current_projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
        budgets: CompactionBudgets,
        constraints: &CompactionConstraints<'_>,
    ) -> Result<CompactionPlan, ContextError> {
        let soft_limit = self.soft_input_limit(budgets.primary_output_budget)?;
        let (active, index) = state
            .structure()
            .map_err(|error| conversation_failed(&error))?;
        let reservation = u64::from(budgets.summary_output_budget);
        let (first, run_end) = compactable_run(&index)?;
        let min_end = continuation_min_end(&index, constraints.must_cover_through, first, run_end)?;
        let Some(max_end) = fresh_retention_max_end(&index, constraints.fresh_inbound)? else {
            return Err(no_progress(
                "unobserved fresh inbound material must stay active, so nothing can be retired",
            ));
        };

        let mut candidates: Vec<Candidate> = Vec::new();
        for end in first..=run_end {
            if end < min_end || end > max_end || index.validate_span(first, end).is_err() {
                continue;
            }
            let span_messages = active[first..=end].to_vec();
            let summary_input_tokens = self.estimate_summary_input(state.revision(), span_messages);
            if summary_input_tokens > budgets.summary_input_limit {
                // The selected span would not fit the summary model's own
                // request budget: this candidate is impossible.
                continue;
            }
            let retained_recent = self.estimator.estimate_conversation_input(&bare_projection(
                state.revision(),
                &active[end + 1..],
            ));
            let planned_items = retained_items(&active, first, end);
            let planned = self
                .estimator
                .estimate_input(
                    &projection_of(
                        state.revision(),
                        &planned_items,
                        &current_projection.effective_system_prompt,
                    ),
                    tool_definitions,
                )
                .saturating_add(reservation);
            candidates.push(Candidate {
                end,
                retained_recent,
                planned,
                summary_input_tokens,
            });
        }
        if candidates.is_empty() {
            if min_end > max_end || min_end > run_end {
                return Err(no_progress(
                    "the compaction constraints leave no retirable surface span",
                ));
            }
            return Err(cannot_fit(&self.config));
        }

        let target = self.config.keep_recent_tokens;
        let chosen = candidates
            .iter()
            .filter(|candidate| candidate.retained_recent >= target)
            .max_by_key(|candidate| candidate.end)
            .filter(|candidate| candidate.planned <= soft_limit)
            .or_else(|| {
                candidates
                    .iter()
                    .filter(|candidate| candidate.planned <= soft_limit)
                    .min_by_key(|candidate| candidate.end)
            })
            .ok_or_else(|| cannot_fit(&self.config))?;

        let span = SurfaceSpan::new(
            index
                .id_at(first)
                .expect("the compactable run starts inside the surface")
                .clone(),
            index
                .id_at(chosen.end)
                .expect("the chosen span end is inside the surface")
                .clone(),
        );
        Ok(CompactionPlan {
            surface_revision: state.revision(),
            span,
            retired: active[first..=chosen.end].to_vec(),
            estimated_before: current_projection.estimated_input,
            // The deterministic estimate of the pre-compaction request
            // context, provenance-free and including the same system prompt:
            // the anti-loop progress rule compares this to the deterministic
            // estimate of the post-compaction context and never mixes a
            // provider-reported measurement with an estimate.
            estimated_before_tokens: self.estimator.estimate_input(
                &projection_of(
                    current_projection.surface_revision,
                    &current_projection.messages,
                    &current_projection.effective_system_prompt,
                ),
                tool_definitions,
            ),
            planned_estimate_after: chosen.planned,
            summary_reservation: reservation,
            summary_input_tokens: chosen.summary_input_tokens,
            effective_system_prompt: current_projection.effective_system_prompt.clone(),
        })
    }

    /// Estimates the exact provider-neutral input assembled for one summary
    /// request. This is the planner's view of the same
    /// [`SummaryRequest::model_input`] contract the production summarizer
    /// sends.
    fn estimate_summary_input(&self, revision: SurfaceRevision, retired: Vec<MessageBlock>) -> u64 {
        let input = SummaryRequest { retired }.model_input();
        self.estimator
            .estimate_conversation_input(&bare_projection(revision, &input.messages))
    }

    /// Prepares the semantic commit of one compaction: the canonical summary
    /// message, the validated Surface replacement, and the projection the
    /// commit will establish.
    ///
    /// Nothing is mutated here. The mandatory progress rule is enforced at
    /// this boundary: the plan must retire at least one canonical message,
    /// the summary must carry textual content, and the deterministic
    /// post-compaction estimate must strictly decrease below the
    /// deterministic pre-compaction estimate. Both sides of the comparison
    /// come from the same estimator over the actual request context —
    /// including the plan's exact Effective System Prompt — so the decision never
    /// depends on incomparable token provenance.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::NoProgress`] when the plan or the summary
    /// makes no measurable progress, [`ContextErrorKind::SummaryFailed`] for
    /// an empty/whitespace-only summary, and
    /// [`ContextErrorKind::MalformedHistory`] when the Surface rejects the
    /// replacement.
    ///
    /// # Panics
    ///
    /// Panics only if the compaction generation overflows `u64`, which is
    /// unreachable for an in-process conversation.
    pub fn prepare_compaction(
        &self,
        state: &ConversationState,
        conversation_id: &ConversationId,
        plan: &CompactionPlan,
        summary_text: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Result<(PreparedCompactionCommit, ContextProjection), ContextError> {
        if plan.retired.is_empty() {
            return Err(no_progress("the plan retires no canonical message"));
        }
        if plan.surface_revision != state.revision() {
            return Err(no_progress(&format!(
                "the plan was made against surface revision {} but the current revision is {}",
                plan.surface_revision,
                state.revision()
            )));
        }
        if summary_text.trim().is_empty() {
            return Err(ContextError::new(
                ContextErrorKind::SummaryFailed,
                "summary generation produced no content",
            ));
        }
        let generation = state
            .surface()
            .compaction_generation()
            .checked_add(1)
            .expect("the compaction generation cannot overflow");
        let summary = UserMessageBlock {
            id: summary_message_id(conversation_id, generation),
            content: vec![UserContentBlock::Text(TextBlock {
                text: summary_text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary,
            timestamp: None,
        };
        let commit = state
            .prepare_compaction(summary.clone(), plan.span.clone())
            .map_err(|error| conversation_failed(&error))?;
        let projected_messages = Self::projected_after(state, plan, &summary)?;
        let projection = self.measured_projection(
            state.revision().next(),
            projected_messages,
            &plan.effective_system_prompt,
            tool_definitions,
            None,
        );
        let estimated_after = projection.estimated_input.input_tokens;
        if estimated_after >= plan.estimated_before_tokens {
            return Err(no_progress(&format!(
                "projected estimate {} does not strictly decrease from the deterministic estimate {}",
                estimated_after, plan.estimated_before_tokens
            )));
        }
        Ok((commit, projection))
    }

    /// The active messages the commit of `plan` will establish.
    fn projected_after(
        state: &ConversationState,
        plan: &CompactionPlan,
        summary: &UserMessageBlock,
    ) -> Result<Vec<MessageBlock>, ContextError> {
        let (active, index) = state
            .structure()
            .map_err(|error| conversation_failed(&error))?;
        let start = index
            .position_of(&plan.span.start)
            .ok_or_else(|| malformed("the plan's span start is not active"))?;
        let end = index
            .position_of(&plan.span.end)
            .ok_or_else(|| malformed("the plan's span end is not active"))?;
        let mut messages = active[..start].to_vec();
        messages.push(MessageBlock::User(summary.clone()));
        messages.extend_from_slice(&active[end + 1..]);
        Ok(messages)
    }

    /// Wraps hydrated messages into a measured projection.
    fn measured_projection(
        &self,
        surface_revision: SurfaceRevision,
        messages: Vec<MessageBlock>,
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
        observed: Option<&ProviderObservedInput>,
    ) -> ContextProjection {
        let mut projection = ContextProjection {
            surface_revision,
            messages,
            effective_system_prompt: effective_system_prompt.to_owned(),
            estimated_input: TokenMeasurement {
                input_tokens: 0,
                source: TokenMeasurementSource::Estimated,
            },
        };
        projection.estimated_input = match observed {
            Some(observed) if observed.fingerprint == projection.fingerprint() => {
                TokenMeasurement {
                    input_tokens: observed.input_tokens,
                    source: TokenMeasurementSource::ProviderReported,
                }
            }
            _ => TokenMeasurement {
                input_tokens: self.estimator.estimate_input(&projection, tool_definitions),
                source: TokenMeasurementSource::Estimated,
            },
        };
        projection
    }
}

/// One evaluated candidate span `[first ..= end]`.
struct Candidate {
    end: usize,
    retained_recent: u64,
    planned: u64,
    summary_input_tokens: u64,
}

/// The compactable region: the earliest contiguous run of non-`System`
/// active messages, as an inclusive `(first, last)` index pair.
///
/// The narrow interim rule of Issue #54 is that trusted `System` content is
/// never replaced by a runtime summary. A later `System` message therefore
/// bounds the *current* compactable run at that point, but it never pins
/// older conversational messages (they are compactable in the run before it)
/// and it never resurrects retired Surface history. The request-time
/// Effective System Prompt is assembled before this engine and is carried
/// through compaction unchanged.
fn compactable_run(index: &StructuralIndex) -> Result<(usize, usize), ContextError> {
    let systems = index.system_positions();
    let first = (0..index.len())
        .find(|position| !systems.contains(position))
        .ok_or_else(|| no_progress("the active surface holds no compactable message"))?;
    let run_end = systems
        .iter()
        .copied()
        .find(|position| *position > first)
        .map_or(index.len() - 1, |position| position - 1);
    Ok((first, run_end))
}

/// The smallest span end the continuation constraint requires.
///
/// A continuation can be retired only by actually covering its owning turn
/// completely. When the continuation-owning turn lies outside the current
/// compactable run, the constraint is unsatisfiable and planning fails
/// explicitly rather than clearing the continuation while leaving its
/// message active.
fn continuation_min_end(
    index: &StructuralIndex,
    must_cover_through: Option<&MessageId>,
    first: usize,
    run_end: usize,
) -> Result<usize, ContextError> {
    let Some(owner) = must_cover_through else {
        return Ok(first);
    };
    let Some(owner_position) = index.position_of(owner) else {
        return Err(malformed(&format!(
            "continuation-owning message {owner} is not active on the surface"
        )));
    };
    if !index.assistant_positions().contains(&owner_position) {
        return Err(malformed(&format!(
            "continuation-owning message {owner} is not an Assistant message"
        )));
    }
    let turn_end = index.turn_end_of(owner_position);
    if owner_position < first || turn_end > run_end {
        return Err(no_progress(
            "the continuation-owning turn lies outside the compactable region \
             and cannot be retired by compaction",
        ));
    }
    Ok(turn_end)
}

/// The largest span end the fresh-inbound retention constraint permits, or
/// `None` when nothing at all may be retired.
///
/// A fresh inbound turn that has not yet been observed by a successfully
/// completed model invocation must remain active, so the span must end
/// strictly before the earliest fresh inbound message.
///
/// # Errors
///
/// Returns [`ContextErrorKind::MalformedHistory`] when a fresh inbound
/// message is not active; a pending fresh trigger always references active
/// messages.
fn fresh_retention_max_end(
    index: &StructuralIndex,
    fresh_inbound: Option<&FreshInboundTurn>,
) -> Result<Option<usize>, ContextError> {
    let Some(fresh) = fresh_inbound else {
        return Ok(Some(index.len().saturating_sub(1)));
    };
    let earliest = fresh
        .message_ids()
        .iter()
        .map(|id| {
            index.position_of(id).ok_or_else(|| {
                malformed(&format!(
                    "fresh inbound message {id} is not active on the surface"
                ))
            })
        })
        .collect::<Result<Vec<usize>, ContextError>>()?
        .into_iter()
        .min()
        .expect("a fresh inbound turn is never empty");
    Ok(earliest.checked_sub(1))
}

/// The retained active messages of a span `[first ..= end]` replacement,
/// excluding the not-yet-generated summary.
fn retained_items(active: &[MessageBlock], first: usize, end: usize) -> Vec<MessageBlock> {
    let mut items = active[..first].to_vec();
    items.extend_from_slice(&active[end + 1..]);
    items
}

/// Wraps a message list into a projection for the conversation-content
/// estimate, without a request-time system prompt.
fn bare_projection(revision: SurfaceRevision, messages: &[MessageBlock]) -> ContextProjection {
    projection_of(revision, messages, "")
}

/// Wraps a message list into a projection for estimation.
fn projection_of(
    revision: SurfaceRevision,
    messages: &[MessageBlock],
    effective_system_prompt: &str,
) -> ContextProjection {
    ContextProjection {
        surface_revision: revision,
        messages: messages.to_vec(),
        effective_system_prompt: effective_system_prompt.to_owned(),
        estimated_input: TokenMeasurement {
            input_tokens: 0,
            source: TokenMeasurementSource::Estimated,
        },
    }
}

fn no_progress(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::NoProgress, message)
}

fn malformed(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::MalformedHistory, message)
}

/// Projects a conversation-domain failure into the context error model.
fn conversation_failed(error: &ConversationError) -> ContextError {
    ContextError::new(ContextErrorKind::MalformedHistory, error.to_string())
}

fn cannot_fit(config: &ContextConfig) -> ContextError {
    ContextError::new(
        ContextErrorKind::CannotFit,
        format!(
            "no complete-message surface span fits under window {} with reserve {}",
            config.context_window_tokens, config.reserve_tokens
        ),
    )
}
