//! The deterministic context engine.
//!
//! The engine projects immutable canonical history into bounded model
//! context: it builds [`ContextProjection`] values, plans and applies
//! compaction, and enforces the context-window threshold. Compaction changes
//! the projection, never canonical history.
//!
//! All decisions are deterministic pure functions of (canonical history,
//! latest checkpoint, tool definitions, observed provider usage): the same
//! inputs always produce the same projection, plan, and estimate.
//!
//! The engine owns no provider knowledge: the window/reserve/recent-token
//! configuration is runtime-owned, token estimation is pluggable, and no
//! model name catalog exists.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::checkpoint::{ContextBoundary, ContextCheckpoint, summary_message_id};
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::{ContextProjection, ProjectionItem};
use crate::context::structure::StructuralIndex;
use crate::context::summarizer::{SplitTurnSummaryInput, SummaryInputItem, SummaryRequest};
use crate::context::tokens::{
    ProviderObservedInput, TokenEstimator, TokenMeasurement, TokenMeasurementSource,
};
use crate::message::content::TextBlock;
use crate::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::types::{AgentStatusAttachment, SkillCatalogAttachment};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId};
use crate::runtime::inbound::FreshInboundTurn;
use crate::tools::types::ModelToolDefinition;

/// The static session-owned context policy.
///
/// A conversation session owns the *policy* — the safety reserve, the
/// uncompressed recent-history target, and the summary output safety cap —
/// but it deliberately does **not** own a context window. The context window
/// belongs to the model, and the session model may change between attempts,
/// so the effective [`ContextConfig`] of an attempt is derived from this
/// policy plus that attempt's immutable model snapshot.
///
/// An attempt using model B therefore never makes compaction decisions with
/// model A's window, and no context window captured at process start
/// survives a model change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContextPolicy {
    /// Tokens permanently reserved out of whichever model context window is
    /// in force.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation history kept uncompressed. This is a
    /// token target, never a message count target.
    pub keep_recent_tokens: u64,
    /// The context plane's summary/output safety cap, when it imposes one.
    ///
    /// The cap is applied through the runtime-owned protected max-output
    /// field of the summary invocation; it never mutates a reasoning profile
    /// or a request-parameter object.
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
/// (equality compacts), `max_output_tokens` is the runtime-resolved
/// generation budget, and `reserve_tokens` is an additional safety reserve.
/// Impossible configurations are rejected; no fallback constant is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextConfig {
    /// The model context window in tokens.
    pub context_window_tokens: u64,
    /// Tokens permanently reserved out of the model context window.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation history kept uncompressed. This is a
    /// token target, never a message count target.
    pub keep_recent_tokens: u64,
}

impl ContextConfig {
    /// Derives the soft input limit of one request with the given
    /// runtime-resolved output budget, using checked arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] when the window
    /// leaves no positive effective input budget: `context_window_tokens`
    /// must exceed `reserve_tokens + max_output_tokens`.
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

/// One ordered item of the uncompressed compactable suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuffixItem {
    /// A whole canonical message at this history position.
    Whole(usize),
    /// The retained content slice of the agent message at this history
    /// position, starting at `first_retained_block`.
    RetainedSlice(usize, usize),
}

impl SuffixItem {
    fn position(self) -> usize {
        match self {
            Self::Whole(position) | Self::RetainedSlice(position, _) => position,
        }
    }
}

/// The deterministic plan of one compaction.
///
/// A plan is a pure function of the current state; applying it with a
/// summary produces the next checkpoint ([`ContextEngine::apply_compaction`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionPlan {
    /// The new coverage boundary.
    pub boundary: ContextBoundary,
    /// The canonical material newly retired since the previous checkpoint:
    /// whole messages, or residual content slices of a previously split
    /// agent message that is now fully retired.
    pub newly_retired: Vec<SummaryInputItem>,
    /// The retired prefix of a split turn, when this compaction splits one.
    pub split_turn_prefix: Option<SplitTurnSummaryInput>,
    /// The measured input of the pre-compaction projection, with its
    /// provenance. Preserved for diagnostics and checkpoint metadata; the
    /// anti-loop progress rule never compares measurements of different
    /// provenance.
    pub estimated_before: TokenMeasurement,
    /// The deterministic estimated input of the pre-compaction projection,
    /// computed with the same estimator on both sides of the progress rule.
    /// The anti-loop invariant compares this to the deterministic estimate
    /// of the post-compaction projection.
    pub estimated_before_tokens: u64,
    /// The planned post-compaction estimate: pinned context plus the
    /// retained suffix plus the summary reservation.
    pub planned_estimate_after: u64,
    /// The summary output budget reserved during planning (the runtime
    /// maximum output tokens), a conservative bound for the unknown summary.
    pub summary_reservation: u64,
    /// The exact Agent Status attachment of the request preparation this
    /// plan belongs to, when one exists. The plan is bound to one status
    /// snapshot: hard-fit estimates and the application progress rule use
    /// this same snapshot on both sides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatusAttachment>,
    /// The exact Skill catalog attachment of the attempt's capability
    /// snapshot, when one exists. The plan is bound to one catalog
    /// snapshot: hard-fit estimates and the application progress rule use
    /// this same snapshot on both sides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_catalog: Option<SkillCatalogAttachment>,
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
    /// The continuation constraint: the boundary must retire the
    /// continuation-owning turn completely.
    pub must_cover_through: Option<&'a MessageId>,
    /// The fresh-inbound retention constraint: unobserved fresh inbound
    /// material must remain literal.
    pub fresh_inbound: Option<&'a FreshInboundTurn>,
}

/// The chosen boundary shape of a plan.
#[derive(Debug, Clone, Copy)]
enum Chosen {
    /// A whole-message cut retiring suffix items `[0..count)`.
    Whole { count: usize },
    /// A split of the agent message at `agent_position` at block `first`.
    Split {
        agent_position: usize,
        first_retained_block: usize,
    },
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

    /// Builds the current projection of one canonical history.
    ///
    /// The projection is deterministic: pinned system prefix, then the
    /// checkpoint summary (when a checkpoint exists and is not absorbed by
    /// the pinned prefix), then the retained literal suffix. A checkpoint
    /// whose coverage lies fully inside the current pinned system prefix is
    /// *absorbed*: its covered history is literal again, so its summary must
    /// not be injected (that would duplicate covered history next to its
    /// summary). The estimated input is `ProviderReported` only when an
    /// observed provider measurement applies to exactly this projection
    /// (identical fingerprint, including the exact Agent Status attachment);
    /// otherwise it is a deterministic estimate that includes the Agent
    /// Status attachment and the tool definitions. Estimates never become
    /// provider usage.
    ///
    /// `agent_status` is the one status snapshot sampled for this request
    /// preparation: exactly one snapshot per preparation, reused throughout
    /// its proactive compaction planning and application.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] for structurally
    /// invalid canonical history or an inconsistent checkpoint.
    pub fn build_projection(
        &self,
        history: &[MessageBlock],
        checkpoint: Option<&ContextCheckpoint>,
        tool_definitions: &[ModelToolDefinition],
        observed: Option<&ProviderObservedInput>,
        agent_status: Option<&AgentStatusAttachment>,
        skill_catalog: Option<&SkillCatalogAttachment>,
    ) -> Result<ContextProjection, ContextError> {
        let index = StructuralIndex::build(history)?;
        let mut items: Vec<ProjectionItem> = history[..index.pinned_end]
            .iter()
            .cloned()
            .map(ProjectionItem::Message)
            .collect();
        let active_checkpoint =
            checkpoint.filter(|checkpoint| !Self::checkpoint_is_absorbed(&index, checkpoint));
        if let Some(checkpoint) = active_checkpoint {
            items.push(ProjectionItem::Message(MessageBlock::User(
                checkpoint.summary.clone(),
            )));
            items.extend(Self::retained_items(history, checkpoint, &index)?);
        } else {
            items.extend(
                history[index.pinned_end..]
                    .iter()
                    .cloned()
                    .map(ProjectionItem::Message),
            );
        }
        let mut projection = ContextProjection {
            items,
            agent_status: agent_status.cloned(),
            skill_catalog: skill_catalog.cloned(),
            estimated_input: TokenMeasurement {
                input_tokens: 0,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: active_checkpoint.map(|checkpoint| checkpoint.generation),
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
        Ok(projection)
    }

    /// Whether a checkpoint's coverage is fully absorbed by the pinned
    /// system prefix.
    ///
    /// When the current pinned prefix covers the checkpoint boundary (the
    /// boundary message lies inside the pinned region), the checkpoint's
    /// covered history is rendered literally again and the checkpoint must
    /// not contribute its summary to the projection. The checkpoint itself
    /// is untouched; a later compaction establishes a fresh checkpoint.
    fn checkpoint_is_absorbed(index: &StructuralIndex, checkpoint: &ContextCheckpoint) -> bool {
        match &checkpoint.boundary {
            ContextBoundary::AfterMessage { message_id } => index
                .position_of(message_id)
                .is_some_and(|position| position < index.pinned_end),
            ContextBoundary::InsideAgent { message_id, .. } => index
                .position_of(message_id)
                .is_some_and(|position| position < index.pinned_end),
        }
    }

    /// Plans one compaction of the current state.
    ///
    /// The algorithm is deterministic and freezes this priority:
    ///
    /// 1. a whole-turn boundary that satisfies the recent-token target and
    ///    the hard fit;
    /// 2. if no such boundary exists, a hard-fitting whole-turn boundary
    ///    that retains as much useful recent complete-turn context as
    ///    possible (the most-retaining whole cut under the hard fit);
    /// 3. split a turn only when a single oversized turn prevents a viable
    ///    complete-turn projection (no whole cut retains any recent context
    ///    within the hard fit).
    ///
    /// The recent-token target is measured over conversation content only:
    /// tool definitions never count toward satisfying `keep_recent_tokens`,
    /// though they still affect the full request estimate, the threshold,
    /// and the hard fit.
    ///
    /// `must_cover_through` enforces the continuation constraint: the new
    /// boundary must retire the continuation-owning turn completely and may
    /// never split it. When the continuation-owning turn has become part of
    /// the pinned system prefix, no compaction can retire it, and the plan
    /// fails explicitly instead of clearing the continuation while leaving
    /// its boundary literal.
    ///
    /// `fresh_inbound` enforces the fresh-inbound retention constraint: a
    /// fresh inbound turn that has not yet been observed by a successful
    /// model invocation must remain literal in the projection. The planned
    /// boundary must never retire the earliest fresh inbound message or
    /// anything after it. The two constraints serve opposite purposes and
    /// are kept separate:
    ///
    /// ```text
    /// continuation owner   → successful compaction must retire through this
    /// fresh inbound        → successful compaction must not retire this or
    ///                        anything after it
    /// ```
    ///
    /// The planner always uses the exact Agent Status attachment of the
    /// current request preparation (carried by `current_projection`) for its
    /// hard-fit estimates, so the status snapshot itself can change the
    /// compaction decision. If no valid projection can fit while preserving
    /// pinned context, the fresh inbound material, the Agent Status
    /// attachment, the tool definitions, and the required output/reserve
    /// budget, planning fails explicitly with
    /// [`ContextErrorKind::CannotFit`]. The current unobserved user
    /// instruction is never summarized merely to make the request fit.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] for structurally
    /// invalid history, [`ContextErrorKind::NoProgress`] when nothing new
    /// can be retired or the continuation constraint is unsatisfiable, and
    /// [`ContextErrorKind::CannotFit`] when even full compaction cannot fit
    /// pinned context, the fresh inbound material, and the summary
    /// reservation.
    pub fn plan_compaction(
        &self,
        history: &[MessageBlock],
        checkpoint: Option<&ContextCheckpoint>,
        current_projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
        max_output_tokens: u32,
        constraints: &CompactionConstraints<'_>,
    ) -> Result<CompactionPlan, ContextError> {
        let soft_limit = self.soft_input_limit(max_output_tokens)?;
        let index = StructuralIndex::build(history)?;
        let suffix = Self::suffix_items(history, checkpoint, &index)?;
        if suffix.is_empty() {
            return Err(no_progress("the compactable suffix is empty"));
        }
        let reservation = u64::from(max_output_tokens);
        let min_cut = continuation_min_cut(&index, constraints.must_cover_through)?;
        let fresh_cut = fresh_retention_cut(&index, constraints.fresh_inbound)?;
        let scope = PlanScope {
            history,
            suffix: &suffix,
            index: &index,
            tool_definitions,
            estimator: self.estimator.as_ref(),
            soft_limit,
            reservation,
            agent_status: current_projection.agent_status.as_ref(),
            skill_catalog: current_projection.skill_catalog.as_ref(),
        };

        let mut whole_candidates: Vec<(usize, u64, u64)> = Vec::new();
        for count in 1..=suffix.len() {
            let cut = suffix[count - 1].position() + 1;
            if !index.whole_cut_is_valid(cut) || cut < min_cut || cut > fresh_cut {
                continue;
            }
            // The recent-suffix estimate measures conversation content only:
            // tool definitions never satisfy the retention target.
            let retained = scope.estimator.estimate_conversation_input(&projection_of(
                &retained_items_of(&scope, count),
                None,
                None,
            ));
            let planned = estimate_input_of(
                &scope,
                &projection_of(
                    &projection_items_for(&scope, count),
                    scope.agent_status,
                    scope.skill_catalog,
                ),
            )
            .saturating_add(reservation);
            whole_candidates.push((count, retained, planned));
        }

        let target = self.config.keep_recent_tokens;
        let target_cut = whole_candidates
            .iter()
            .filter(|(_, retained, _)| *retained >= target)
            .max_by_key(|(count, _, _)| *count)
            .copied();

        let chosen = match target_cut {
            // Priority 1: the target-satisfying boundary that retires the
            // most, when it fits.
            Some((count, _, planned)) if planned <= soft_limit => Chosen::Whole { count },
            _ => {
                // Priority 2: a hard-fitting whole-turn boundary retaining
                // as much useful recent complete-turn context as possible —
                // the most-retaining whole cut under the hard fit. This must
                // win over splitting whenever it retains any recent context.
                let best_fitting = whole_candidates
                    .iter()
                    .filter(|(_, _, planned)| *planned <= soft_limit)
                    .min_by_key(|(count, _, _)| *count)
                    .copied();
                match best_fitting {
                    Some((count, _, _)) if count < scope.suffix.len() => Chosen::Whole { count },
                    _ => {
                        // Priority 3: no whole cut retains useful recent
                        // context within the hard fit — a single oversized
                        // turn prevents a viable complete-turn projection —
                        // so split the latest turn.
                        match best_split(&scope, min_cut, fresh_cut) {
                            Some((agent_position, first)) => Chosen::Split {
                                agent_position,
                                first_retained_block: first,
                            },
                            None => smallest_fitting_whole(&whole_candidates, soft_limit)
                                .ok_or_else(|| cannot_fit(&self.config))?,
                        }
                    }
                }
            }
        };

        self.build_plan(&scope, chosen, current_projection, max_output_tokens)
    }

    /// The `SummaryRequest` a plan implies, given the previous checkpoint
    /// and the current canonical history.
    ///
    /// The stored checkpoint keeps the generation lineage (the next
    /// checkpoint generation is `previous.generation + 1`), but it is the
    /// active incremental summary source only when it is not absorbed by the
    /// current pinned system prefix. An absorbed checkpoint's coverage is
    /// pinned-literal again, so its old summary must never be fed into the
    /// next summarization: `previous_summary` is `None`, and the newly
    /// retired material starts strictly from the currently compactable
    /// region after the pinned prefix.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] for structurally
    /// invalid history.
    pub fn summary_request(
        &self,
        history: &[MessageBlock],
        checkpoint: Option<&ContextCheckpoint>,
        plan: &CompactionPlan,
    ) -> Result<SummaryRequest, ContextError> {
        let active_previous = match checkpoint {
            Some(checkpoint) => {
                let index = StructuralIndex::build(history)?;
                if Self::checkpoint_is_absorbed(&index, checkpoint) {
                    None
                } else {
                    Some(checkpoint)
                }
            }
            None => None,
        };
        Ok(SummaryRequest {
            previous_summary: active_previous.map(summary_text),
            newly_retired: plan.newly_retired.clone(),
            split_turn_prefix: plan.split_turn_prefix.clone(),
        })
    }

    /// Applies a plan with the generated summary text, producing the next
    /// checkpoint and its projection.
    ///
    /// The mandatory progress rule is enforced here: the new checkpoint must
    /// retire at least one additional compactable canonical unit, and the
    /// deterministic projected estimate must strictly decrease below the
    /// deterministic pre-compaction estimate. Both sides of the comparison
    /// come from the same estimator over the actual projection content —
    /// including the plan's exact Agent Status attachment on both sides — so
    /// the decision never depends on incomparable token provenance; the
    /// provider-reported measurement is preserved only as checkpoint
    /// metadata. If either condition fails the operation errors with
    /// [`ContextErrorKind::NoProgress`] and no checkpoint is produced, so no
    /// retry may follow.
    ///
    /// A summary with no textual content (empty or whitespace-only) is
    /// rejected at this application boundary so no summarizer — including a
    /// custom or scripted one — can erase history through an empty summary.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::NoProgress`] when the plan or the summary
    /// makes no measurable progress, [`ContextErrorKind::SummaryFailed`] for
    /// an empty/whitespace-only summary, and
    /// [`ContextErrorKind::MalformedHistory`] for invalid history.
    pub fn apply_compaction(
        &self,
        conversation_id: &ConversationId,
        history: &[MessageBlock],
        previous: Option<&ContextCheckpoint>,
        plan: &CompactionPlan,
        summary_text: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Result<(ContextCheckpoint, ContextProjection), ContextError> {
        if !self
            .summary_request(history, previous, plan)?
            .advances_coverage()
        {
            return Err(no_progress(
                "the plan retires no additional compactable unit",
            ));
        }
        if summary_text.trim().is_empty() {
            return Err(ContextError::new(
                ContextErrorKind::SummaryFailed,
                "summary generation produced no content",
            ));
        }
        let generation = previous.map_or(1, |checkpoint| checkpoint.generation + 1);
        let mut checkpoint = ContextCheckpoint {
            conversation_id: conversation_id.clone(),
            generation,
            summary: UserMessageBlock {
                id: summary_message_id(conversation_id, generation),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: summary_text.to_owned(),
                })],
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary,
                timestamp: None,
            },
            boundary: plan.boundary.clone(),
            tokens_before: plan.estimated_before,
            estimated_tokens_after: 0,
        };
        let projection = self.build_projection(
            history,
            Some(&checkpoint),
            tool_definitions,
            None,
            plan.agent_status.as_ref(),
            plan.skill_catalog.as_ref(),
        )?;
        let estimated_after = projection.estimated_input.input_tokens;
        if estimated_after >= plan.estimated_before_tokens {
            return Err(no_progress(&format!(
                "projected estimate {} does not strictly decrease from the deterministic estimate {}",
                estimated_after, plan.estimated_before_tokens
            )));
        }
        checkpoint.estimated_tokens_after = estimated_after;
        Ok((checkpoint, projection))
    }

    /// The retained literal items of one checkpoint boundary.
    fn retained_items(
        history: &[MessageBlock],
        checkpoint: &ContextCheckpoint,
        index: &StructuralIndex,
    ) -> Result<Vec<ProjectionItem>, ContextError> {
        match &checkpoint.boundary {
            ContextBoundary::AfterMessage { message_id } => {
                let position = index.position_of(message_id).ok_or_else(|| {
                    malformed(&format!(
                        "checkpoint boundary references unknown message {message_id}"
                    ))
                })?;
                let covered_end = index.pinned_end.max(position + 1);
                Ok(history[covered_end..]
                    .iter()
                    .cloned()
                    .map(ProjectionItem::Message)
                    .collect())
            }
            ContextBoundary::InsideAgent {
                message_id,
                first_retained_block,
            } => {
                let position = index.position_of(message_id).ok_or_else(|| {
                    malformed(&format!(
                        "checkpoint boundary references unknown message {message_id}"
                    ))
                })?;
                if position < index.pinned_end {
                    return Ok(history[index.pinned_end..]
                        .iter()
                        .cloned()
                        .map(ProjectionItem::Message)
                        .collect());
                }
                let MessageBlock::Agent(agent) = &history[position] else {
                    return Err(malformed(
                        "checkpoint split boundary is not an agent message",
                    ));
                };
                let first = first_retained_block.get() as usize;
                if first > agent.content.len() {
                    return Err(malformed(&format!(
                        "checkpoint split block {first_retained_block} out of range for message {message_id}"
                    )));
                }
                let retired_calls = retired_calls(index, position, first);
                let mut items = vec![ProjectionItem::AgentSlice {
                    source_message_id: agent.id.clone(),
                    content: agent.content[first..].to_vec(),
                }];
                for message in &history[position + 1..] {
                    if let MessageBlock::Tool(tool) = message
                        && retired_calls.contains(&tool.tool_call_id)
                    {
                        continue;
                    }
                    items.push(ProjectionItem::Message(message.clone()));
                }
                Ok(items)
            }
        }
    }

    /// The uncompressed compactable suffix as an ordered virtual item list.
    fn suffix_items(
        history: &[MessageBlock],
        checkpoint: Option<&ContextCheckpoint>,
        index: &StructuralIndex,
    ) -> Result<Vec<SuffixItem>, ContextError> {
        // An absorbed checkpoint contributes nothing to the compactable
        // suffix: its covered history is pinned-literal again.
        let Some(checkpoint) =
            checkpoint.filter(|checkpoint| !Self::checkpoint_is_absorbed(index, checkpoint))
        else {
            return Ok((index.pinned_end..history.len())
                .map(SuffixItem::Whole)
                .collect());
        };
        match &checkpoint.boundary {
            ContextBoundary::AfterMessage { message_id } => {
                let position = index.position_of(message_id).ok_or_else(|| {
                    malformed(&format!(
                        "checkpoint boundary references unknown message {message_id}"
                    ))
                })?;
                let covered_end = index.pinned_end.max(position + 1);
                Ok((covered_end..history.len())
                    .map(SuffixItem::Whole)
                    .collect())
            }
            ContextBoundary::InsideAgent {
                message_id,
                first_retained_block,
            } => {
                let position = index.position_of(message_id).ok_or_else(|| {
                    malformed(&format!(
                        "checkpoint boundary references unknown message {message_id}"
                    ))
                })?;
                if position < index.pinned_end {
                    return Ok((index.pinned_end..history.len())
                        .map(SuffixItem::Whole)
                        .collect());
                }
                let first = first_retained_block.get() as usize;
                let Some(content_len) = index.content_len_of(position) else {
                    return Err(malformed(
                        "checkpoint split boundary is not an agent message",
                    ));
                };
                if first > content_len {
                    return Err(malformed(&format!(
                        "checkpoint split block {first_retained_block} out of range"
                    )));
                }
                let retired_calls = retired_calls(index, position, first);
                let mut items = vec![SuffixItem::RetainedSlice(position, first)];
                for (p, message) in history.iter().enumerate().skip(position + 1) {
                    if let MessageBlock::Tool(tool) = message
                        && retired_calls.contains(&tool.tool_call_id)
                    {
                        continue;
                    }
                    items.push(SuffixItem::Whole(p));
                }
                Ok(items)
            }
        }
    }

    fn estimate_items(
        &self,
        items: &[ProjectionItem],
        tools: &[ModelToolDefinition],
        agent_status: Option<&AgentStatusAttachment>,
        skill_catalog: Option<&SkillCatalogAttachment>,
    ) -> u64 {
        let projection = ContextProjection {
            items: items.to_vec(),
            agent_status: agent_status.cloned(),
            skill_catalog: skill_catalog.cloned(),
            estimated_input: TokenMeasurement {
                input_tokens: 0,
                source: TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: None,
        };
        self.estimator.estimate_input(&projection, tools)
    }

    /// Assembles the plan for one chosen boundary.
    fn build_plan(
        &self,
        scope: &PlanScope<'_>,
        chosen: Chosen,
        current_projection: &ContextProjection,
        max_output_tokens: u32,
    ) -> Result<CompactionPlan, ContextError> {
        let (shape, split_turn_prefix) = match chosen {
            Chosen::Whole { count } => (whole_plan_shape(scope, count), None),
            Chosen::Split {
                agent_position,
                first_retained_block,
            } => {
                let shape = split_plan_shape(scope, agent_position, first_retained_block)?;
                let split = shape.split_turn_prefix.clone();
                (shape, split)
            }
        };
        let planned_estimate_after = self
            .estimate_items(
                &shape.planned_items,
                scope.tool_definitions,
                scope.agent_status,
                scope.skill_catalog,
            )
            .saturating_add(scope.reservation);
        if planned_estimate_after > self.soft_input_limit(max_output_tokens)? {
            return Err(cannot_fit(&self.config));
        }
        Ok(CompactionPlan {
            boundary: shape.boundary,
            newly_retired: shape.newly_retired,
            split_turn_prefix,
            estimated_before: current_projection.estimated_input,
            // The deterministic estimate of the pre-compaction projection,
            // provenance-free and including the same exact Agent Status
            // attachment: the anti-loop progress rule compares this to the
            // deterministic estimate of the post-compaction projection (also
            // computed with the plan's attachment) and never mixes a
            // provider-reported measurement with an estimate.
            estimated_before_tokens: estimate_input_of(
                scope,
                &projection_of(
                    &current_projection.items,
                    scope.agent_status,
                    scope.skill_catalog,
                ),
            ),
            planned_estimate_after,
            summary_reservation: scope.reservation,
            agent_status: scope.agent_status.cloned(),
            skill_catalog: scope.skill_catalog.cloned(),
        })
    }
}

/// The deterministic planning scope of one compaction.
struct PlanScope<'a> {
    history: &'a [MessageBlock],
    suffix: &'a [SuffixItem],
    index: &'a StructuralIndex,
    tool_definitions: &'a [ModelToolDefinition],
    estimator: &'a dyn TokenEstimator,
    soft_limit: u64,
    reservation: u64,
    /// The exact Agent Status attachment of the current request preparation;
    /// hard-fit estimates include it so the status itself can change the
    /// compaction decision.
    agent_status: Option<&'a AgentStatusAttachment>,
    /// The exact Skill catalog attachment of the attempt's capability
    /// snapshot; hard-fit estimates include it so a large catalog can
    /// contribute to `CannotFit`.
    skill_catalog: Option<&'a SkillCatalogAttachment>,
}

/// The assembled shape of one chosen boundary.
struct PlanShape {
    boundary: ContextBoundary,
    newly_retired: Vec<SummaryInputItem>,
    split_turn_prefix: Option<SplitTurnSummaryInput>,
    planned_items: Vec<ProjectionItem>,
}

/// The tool calls of the agent message at `agent_position` whose content
/// blocks lie strictly before `first_retained_block`.
fn retired_calls(
    index: &StructuralIndex,
    agent_position: usize,
    first_retained_block: usize,
) -> Vec<ToolCallId> {
    index
        .calls_of(agent_position)
        .iter()
        .filter(|(block, _)| *block < first_retained_block)
        .map(|(_, call)| call.clone())
        .collect()
}

/// The projection item of one suffix item.
fn suffix_item_projection(item: SuffixItem, history: &[MessageBlock]) -> ProjectionItem {
    match item {
        SuffixItem::Whole(position) => ProjectionItem::Message(history[position].clone()),
        SuffixItem::RetainedSlice(position, first) => {
            let MessageBlock::Agent(agent) = &history[position] else {
                unreachable!("retained slices only reference agent messages");
            };
            ProjectionItem::AgentSlice {
                source_message_id: agent.id.clone(),
                content: agent.content[first..].to_vec(),
            }
        }
    }
}

/// The retained literal items of a suffix starting at item `from`.
fn retained_items_of(scope: &PlanScope<'_>, from: usize) -> Vec<ProjectionItem> {
    scope.suffix[from..]
        .iter()
        .map(|item| suffix_item_projection(*item, scope.history))
        .collect()
}

/// The projection items of a whole cut retiring `[0..count)` suffix items:
/// pinned prefix plus retained suffix.
fn projection_items_for(scope: &PlanScope<'_>, count: usize) -> Vec<ProjectionItem> {
    let mut items: Vec<ProjectionItem> = scope.history[..scope.index.pinned_end]
        .iter()
        .cloned()
        .map(ProjectionItem::Message)
        .collect();
    items.extend(retained_items_of(scope, count));
    items
}

/// The projection items of a split boundary: pinned prefix plus the retained
/// slice and the retained suffix after the split.
fn split_projection_items(
    scope: &PlanScope<'_>,
    agent_position: usize,
    first_retained_block: usize,
) -> Vec<ProjectionItem> {
    let mut items: Vec<ProjectionItem> = scope.history[..scope.index.pinned_end]
        .iter()
        .cloned()
        .map(ProjectionItem::Message)
        .collect();
    let Some(agent_item_index) = scope
        .suffix
        .iter()
        .position(|item| item.position() == agent_position)
    else {
        return items;
    };
    items.push(suffix_item_projection(
        SuffixItem::RetainedSlice(agent_position, first_retained_block),
        scope.history,
    ));
    let retired_calls = retired_calls(scope.index, agent_position, first_retained_block);
    for item in &scope.suffix[agent_item_index + 1..] {
        match *item {
            SuffixItem::Whole(position) => {
                if let MessageBlock::Tool(tool) = &scope.history[position]
                    && retired_calls.contains(&tool.tool_call_id)
                {
                    continue;
                }
            }
            SuffixItem::RetainedSlice(_, _) => {}
        }
        items.push(suffix_item_projection(*item, scope.history));
    }
    items
}

/// The history cut the continuation constraint requires, or 0 when no
/// constraint applies.
///
/// A continuation can be retired only by actually covering its owning turn.
/// When a new `SystemMessage` has pinned the continuation-owning message
/// into the literal prefix, no compaction can retire it; the constraint is
/// unsatisfiable and the plan fails explicitly rather than clearing the
/// continuation while leaving its boundary literal.
fn continuation_min_cut(
    index: &StructuralIndex,
    must_cover_through: Option<&MessageId>,
) -> Result<usize, ContextError> {
    let Some(owner) = must_cover_through else {
        return Ok(0);
    };
    let Some(owner_position) = index.position_of(owner) else {
        return Err(malformed(&format!(
            "continuation-owning message {owner} is not in canonical history"
        )));
    };
    if !index.agent_positions().contains(&owner_position) {
        return Err(malformed(&format!(
            "continuation-owning message {owner} is not an agent message"
        )));
    }
    if owner_position < index.pinned_end {
        return Err(no_progress(
            "the continuation-owning turn is pinned by system context and \
             cannot be retired by compaction",
        ));
    }
    Ok(index.turn_end_of(owner_position) + 1)
}

/// The maximum history cut the fresh-inbound retention constraint permits,
/// or the full history length when no fresh inbound turn is pending.
///
/// A fresh inbound turn that has not yet been observed by a successfully
/// completed model invocation must remain literal in the projection, so the
/// retirement boundary must never pass the earliest fresh inbound message:
/// `cut <= p` for the earliest fresh message position `p`. When the fresh
/// message lies inside the pinned system prefix it is literal regardless,
/// and the constraint is vacuous.
///
/// # Errors
///
/// Returns [`ContextErrorKind::MalformedHistory`] when a fresh inbound
/// message is not present in canonical history; a pending fresh trigger must
/// always reference committed messages.
fn fresh_retention_cut(
    index: &StructuralIndex,
    fresh_inbound: Option<&FreshInboundTurn>,
) -> Result<usize, ContextError> {
    let Some(fresh) = fresh_inbound else {
        return Ok(index.len());
    };
    let earliest = fresh
        .message_ids()
        .iter()
        .map(|id| {
            index.position_of(id).ok_or_else(|| {
                malformed(&format!(
                    "fresh inbound message {id} is not in canonical history"
                ))
            })
        })
        .collect::<Result<Vec<usize>, ContextError>>()?
        .into_iter()
        .min()
        .expect("a fresh inbound turn is never empty");
    if earliest < index.pinned_end {
        return Ok(index.len());
    }
    Ok(earliest)
}

/// The smallest split of the latest turn that fits, preserving as much of
/// the turn's tail as possible.
///
/// The split is additionally subject to the fresh-inbound retention
/// constraint: the boundary may not retire the earliest fresh inbound
/// message, so the split agent message must lie strictly before it.
fn best_split(scope: &PlanScope<'_>, min_cut: usize, fresh_cut: usize) -> Option<(usize, usize)> {
    let (agent_position, current_first) = last_agent_item(scope)?;
    if min_cut > agent_position {
        // The continuation constraint cannot be satisfied by a split of this
        // turn: its message may not remain partly literal.
        return None;
    }
    if agent_position + 1 > fresh_cut {
        // The fresh-inbound retention constraint cannot be satisfied by a
        // split of this turn: retiring it would also retire the earliest
        // fresh inbound message.
        return None;
    }
    let content_len = scope.index.content_len_of(agent_position)?;
    for first in (current_first + 1)..content_len {
        let items = split_projection_items(scope, agent_position, first);
        let planned = estimate_input_of(
            scope,
            &projection_of(&items, scope.agent_status, scope.skill_catalog),
        )
        .saturating_add(scope.reservation);
        if planned <= scope.soft_limit {
            return Some((agent_position, first));
        }
    }
    None
}

/// The last agent-message suffix item: its history position and its current
/// first retained block (0 when the message is a whole item).
fn last_agent_item(scope: &PlanScope<'_>) -> Option<(usize, usize)> {
    let mut last = None;
    for item in scope.suffix {
        let position = item.position();
        if scope.index.agent_positions().contains(&position) {
            let current_first = match item {
                SuffixItem::Whole(_) => 0,
                SuffixItem::RetainedSlice(_, first) => *first,
            };
            last = Some((position, current_first));
        }
    }
    last
}

/// Wraps one item list into a projection for estimation.
fn projection_of(
    items: &[ProjectionItem],
    agent_status: Option<&AgentStatusAttachment>,
    skill_catalog: Option<&SkillCatalogAttachment>,
) -> ContextProjection {
    ContextProjection {
        items: items.to_vec(),
        agent_status: agent_status.cloned(),
        skill_catalog: skill_catalog.cloned(),
        estimated_input: TokenMeasurement {
            input_tokens: 0,
            source: TokenMeasurementSource::Estimated,
        },
        checkpoint_generation: None,
    }
}

/// The estimate of one projection under the scope's estimator.
fn estimate_input_of(scope: &PlanScope<'_>, projection: &ContextProjection) -> u64 {
    scope
        .estimator
        .estimate_input(projection, scope.tool_definitions)
}
/// The smallest whole cut that fits, retaining the most content while
/// respecting the hard fit.
fn smallest_fitting_whole(candidates: &[(usize, u64, u64)], soft_limit: u64) -> Option<Chosen> {
    candidates
        .iter()
        .filter(|(_, _, planned)| *planned <= soft_limit)
        .min_by_key(|(count, _, _)| *count)
        .map(|(count, _, _)| Chosen::Whole { count: *count })
}

/// The whole-cut shape of one chosen boundary: the retired items and the
/// planned projection items.
fn whole_plan_shape(scope: &PlanScope<'_>, count: usize) -> PlanShape {
    let mut newly_retired = Vec::new();
    for item in &scope.suffix[..count] {
        match *item {
            SuffixItem::Whole(position) => {
                newly_retired.push(SummaryInputItem::Message(scope.history[position].clone()));
            }
            SuffixItem::RetainedSlice(position, first) => {
                let MessageBlock::Agent(agent) = &scope.history[position] else {
                    unreachable!("retained slices only reference agent messages");
                };
                newly_retired.push(SummaryInputItem::AgentSlice {
                    message_id: agent.id.clone(),
                    content: agent.content[first..].to_vec(),
                });
            }
        }
    }
    let boundary_message_id =
        crate::context::structure::message_id(&scope.history[scope.suffix[count - 1].position()]);
    PlanShape {
        boundary: ContextBoundary::AfterMessage {
            message_id: boundary_message_id,
        },
        newly_retired,
        split_turn_prefix: None,
        planned_items: projection_items_for(scope, count),
    }
}

/// The split shape of one chosen boundary.
fn split_plan_shape(
    scope: &PlanScope<'_>,
    agent_position: usize,
    first_retained_block: usize,
) -> Result<PlanShape, ContextError> {
    let MessageBlock::Agent(agent) = &scope.history[agent_position] else {
        return Err(malformed("split boundary is not an agent message"));
    };
    let current_first = scope
        .suffix
        .iter()
        .find(|item| item.position() == agent_position)
        .map_or(0, |item| match item {
            SuffixItem::Whole(_) => 0,
            SuffixItem::RetainedSlice(_, first) => *first,
        });
    let newly_retired_calls: Vec<ToolCallId> = scope
        .index
        .calls_of(agent_position)
        .iter()
        .filter(|(block, _)| *block >= current_first && *block < first_retained_block)
        .map(|(_, call)| call.clone())
        .collect();
    let mut newly_retired = Vec::new();
    let mut split = SplitTurnSummaryInput {
        message_id: agent.id.clone(),
        retired_prefix: agent.content[current_first..first_retained_block].to_vec(),
        retired_tool_messages: Vec::new(),
    };
    for item in scope.suffix {
        match *item {
            SuffixItem::Whole(position) if position < agent_position => {
                newly_retired.push(SummaryInputItem::Message(scope.history[position].clone()));
            }
            SuffixItem::RetainedSlice(position, first) if position < agent_position => {
                let MessageBlock::Agent(previous) = &scope.history[position] else {
                    unreachable!("retained slices only reference agent messages");
                };
                newly_retired.push(SummaryInputItem::AgentSlice {
                    message_id: previous.id.clone(),
                    content: previous.content[first..].to_vec(),
                });
            }
            SuffixItem::Whole(position) if position > agent_position => {
                if let MessageBlock::Tool(tool) = &scope.history[position]
                    && newly_retired_calls.contains(&tool.tool_call_id)
                {
                    split.retired_tool_messages.push(tool.clone());
                }
            }
            _ => {}
        }
    }
    let first =
        u32::try_from(first_retained_block).expect("content block indices always fit a u32");
    Ok(PlanShape {
        boundary: ContextBoundary::InsideAgent {
            message_id: agent.id.clone(),
            first_retained_block: ContentBlockIndex::new(first),
        },
        newly_retired,
        split_turn_prefix: Some(split),
        planned_items: split_projection_items(scope, agent_position, first_retained_block),
    })
}

fn summary_text(checkpoint: &ContextCheckpoint) -> String {
    checkpoint
        .summary
        .content
        .iter()
        .filter_map(|block| match block {
            UserContentBlock::Text(text) => Some(text.text.clone()),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn no_progress(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::NoProgress, message)
}

fn malformed(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::MalformedHistory, message)
}

fn cannot_fit(config: &ContextConfig) -> ContextError {
    ContextError::new(
        ContextErrorKind::CannotFit,
        format!(
            "pinned context, summary, and retained suffix cannot fit under window {} with reserve {}",
            config.context_window_tokens, config.reserve_tokens
        ),
    )
}
