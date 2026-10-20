//! Native context readings: a provider-measured request or the deterministic
//! rebuilt context committed by compaction. Neither uses browser-visible history.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::tokens::bytes_to_tokens;
#[cfg(test)]
use crate::durable::ConversationStore;
use crate::durable::ConversationStoreError;
#[cfg(test)]
use crate::durable::presentation::{FactQuery, FactScope};
#[cfg(test)]
use crate::events::types::RuntimeEvent;

/// Latest native context reading for this Conversation, with explicit provenance.
/// Unsent composer text is never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOccupancy {
    /// True for the native rebuilt-context estimate after compaction.
    #[serde(default)]
    pub estimated: bool,
    /// Normalized request input, or the committed rebuilt-context estimate.
    pub input_tokens: u64,
    /// Capacity frozen in the request or compaction's native model snapshot.
    pub context_window_tokens: u64,
    /// Provider-facing historical model, never substituted from current config.
    pub model: String,
    /// Heuristic composition of the same request's input.
    pub breakdown: ContextBreakdown,
}

impl ContextOccupancy {
    pub(crate) fn validate(&self) -> Result<(), ConversationStoreError> {
        if self.context_window_tokens == 0 {
            return Err(ConversationStoreError::InvalidReference(
                "an inherited occupancy reading has no valid context window".into(),
            ));
        }
        Ok(())
    }
}

/// Where one request's measured input went, priced by the provider-neutral
/// `ceil(bytes / 4)` estimate. System prompt and Tool definitions are priced
/// heuristically; messages are the remainder of the measured or estimated total.
/// The parts sum to that total unless the component estimates alone exceed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextBreakdown {
    pub system_tokens: u64,
    pub tool_tokens: u64,
    pub message_tokens: u64,
}

/// Project the latest native context reading at a finite cut.
/// New requests and compaction in progress preserve the last reading. A committed
/// checkpoint replaces it with its native estimate; failed maintenance preserves it.
/// # Errors
/// Durable read failures are not treated as missing evidence.
#[cfg(test)]
pub(crate) fn read(
    store: &dyn ConversationStore,
    through: u64,
) -> Result<Option<ContextOccupancy>, ConversationStoreError> {
    let events = store.read_presentation_events(&FactQuery {
        scope: FactScope::ContextReading,
        kinds: vec![
            "model_request_completed",
            "model_request_failed",
            "compaction_completed",
        ],
        before: None,
        after: 0,
        ascending: false,
        through,
        limit: 1,
    })?;
    match events.first().map(|event| &event.event) {
        Some(
            RuntimeEvent::ModelRequestCompleted {
                owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
                request_id,
                usage: Some(usage),
                ..
            }
            | RuntimeEvent::ModelRequestFailed {
                owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
                request_id,
                usage: Some(usage),
                ..
            },
        ) => measure(usage, &store.load_request_snapshot(request_id)?),
        Some(RuntimeEvent::CompactionCompleted { occupancy, .. }) => Ok(occupancy.clone()),
        None => store.load_inherited_occupancy(),
        Some(_) => Err(ConversationStoreError::InvalidReference(
            "context reading index returned non-primary Journal evidence".into(),
        )),
    }
}

/// Inherited occupancy follows projection activation independently of execution.
/// An activation covering the newest retained completed turn supersedes its
/// old request measurement; source/destination Journal domains are never compared.
pub(crate) fn inherit(
    turns: &[crate::durable::reading::TurnReadingProvenance],
    canonical: &[crate::message::types::MessageBlock],
    projections: &[crate::durable::projection::ProjectionCheckpoint],
) -> Option<ContextOccupancy> {
    let turn = turns.iter().rev().find(|turn| {
        turn.execution
            .as_ref()
            .is_some_and(|execution| execution.model_requests > 0)
    });
    if let Some(checkpoint) = projections.last() {
        let last_turn_position = turn.and_then(|turn| {
            turn.process_message_ids
                .iter()
                .filter_map(|id| {
                    canonical
                        .iter()
                        .position(|message| message.id() == id)
                        .map(|i| i as u64 + 1)
                })
                .max()
        });
        if last_turn_position
            .is_none_or(|position| position <= checkpoint.candidate.base_frontier.0)
        {
            return checkpoint.occupancy.clone();
        }
    }
    turn.and_then(|turn| turn.execution.as_ref())
        .and_then(|execution| execution.occupancy.clone())
}

/// The occupancy one terminal reading measures for its own frozen request.
///
/// # Errors
///
/// Fails only when the frozen request cannot be serialized for pricing.
pub(crate) fn measure(
    usage: &crate::model::types::ModelUsage,
    snapshot: &crate::model::snapshot::RequestSnapshot,
) -> Result<Option<ContextOccupancy>, ConversationStoreError> {
    if snapshot.context_window_tokens == 0 {
        return Ok(None);
    }
    // The same serialized forms the request estimate prices; an absent part costs nothing.
    let estimate = |bytes: usize| bytes_to_tokens(bytes as u64);
    let system_tokens = if snapshot.effective_system_prompt.is_empty() {
        0
    } else {
        estimate(
            serde_json::to_vec(&snapshot.effective_system_prompt)
                .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))?
                .len(),
        )
    };
    let tool_tokens = if snapshot.tool_definitions.is_empty() {
        0
    } else {
        estimate(
            serde_json::to_vec(&snapshot.tool_definitions)
                .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))?
                .len(),
        )
    };
    Ok(Some(ContextOccupancy {
        estimated: false,
        input_tokens: usage.input_tokens,
        context_window_tokens: snapshot.context_window_tokens,
        model: snapshot.invocation.model.clone(),
        breakdown: ContextBreakdown {
            system_tokens,
            tool_tokens,
            message_tokens: usage
                .input_tokens
                .saturating_sub(system_tokens.saturating_add(tool_tokens)),
        },
    }))
}

/// Prices the same primary request context validated and committed by compaction.
/// The reading never contributes to cumulative provider usage.
pub(crate) fn estimate(
    input_tokens: u64,
    context_window_tokens: u64,
    model: &str,
    system: &str,
    tools: &[crate::tools::types::ModelToolDefinition],
) -> ContextOccupancy {
    use crate::context::TokenEstimator as _;
    let estimator = crate::context::DefaultTokenEstimator;
    let base = estimator.estimate_input(&[], "", &[]);
    let system_tokens = estimator
        .estimate_input(&[], system, &[])
        .saturating_sub(base);
    let tool_tokens = estimator
        .estimate_input(&[], "", tools)
        .saturating_sub(base);
    ContextOccupancy {
        estimated: true,
        input_tokens,
        context_window_tokens,
        model: model.to_owned(),
        breakdown: ContextBreakdown {
            system_tokens,
            tool_tokens,
            message_tokens: input_tokens.saturating_sub(system_tokens.saturating_add(tool_tokens)),
        },
    }
}
