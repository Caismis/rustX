//! Native context readings: a provider-measured request or the deterministic
//! rebuilt context committed by compaction. Neither uses browser-visible history.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::tokens::bytes_to_tokens;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
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
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub input_tokens: u64,
    /// Capacity frozen in the request or compaction's native model snapshot.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub context_window_tokens: u64,
    /// Provider-facing historical model, never substituted from current config.
    pub model: String,
    /// Heuristic composition of the same request's input.
    pub breakdown: ContextBreakdown,
}

/// Where one request's measured input went, priced by the provider-neutral
/// `ceil(bytes / 4)` estimate. System prompt and Tool definitions are priced
/// heuristically; messages are the remainder of the measured or estimated total.
/// The parts sum to that total unless the component estimates alone exceed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextBreakdown {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub system_tokens: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub tool_tokens: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub message_tokens: u64,
}

/// Project the latest native context reading at a finite cut.
/// New requests and compaction in progress preserve the last reading. A committed
/// checkpoint replaces it with its native estimate; failed maintenance preserves it.
/// # Errors
/// Durable read failures are not treated as missing evidence.
pub(crate) fn read(
    store: &dyn ConversationStore,
    through: u64,
) -> Result<Option<ContextOccupancy>, ConversationStoreError> {
    let mut before = None;
    loop {
        let events = store.read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec![
                "model_request_completed",
                "model_request_failed",
                "compaction_completed",
            ],
            before,
            after: 0,
            ascending: false,
            through,
            limit: 64,
        })?;
        for event in &events {
            match &event.event {
                RuntimeEvent::ModelRequestCompleted {
                    request_id,
                    usage: Some(usage),
                    ..
                }
                | RuntimeEvent::ModelRequestFailed {
                    request_id,
                    usage: Some(usage),
                    ..
                } => return measure(usage, &store.load_request_snapshot(request_id)?),
                RuntimeEvent::CompactionCompleted { occupancy, .. } => return Ok(occupancy.clone()),
                _ => {}
            }
        }
        if events.len() < 64 {
            return inherited(store);
        }
        before = events.last().map(|event| event.sequence);
    }
}

/// Before its own first measurement, a lineage child reads the context its newest
/// inherited request measured, as a Harness fork's copied prefix does, unless
/// its retained Surface compacted after that turn's content.
fn inherited(
    store: &dyn ConversationStore,
) -> Result<Option<ContextOccupancy>, ConversationStoreError> {
    use crate::conversation::SurfaceOp;
    let Some(turn) = store
        .load_inherited_turns()?
        .into_iter()
        .rev()
        .find(|turn| {
            turn.execution
                .as_ref()
                .is_some_and(|execution| execution.model_requests > 0)
        })
    else {
        return Ok(None);
    };
    let history = store.load_surface_history(store.load_head()?.revision)?;
    let appended = history.iter().rposition(|op| {
        matches!(op, SurfaceOp::Append { message_id } if turn.process_message_ids.contains(message_id))
    });
    let compacted = history
        .iter()
        .rposition(|op| matches!(op, SurfaceOp::Replace { .. }));
    if compacted > appended {
        return Ok(None);
    }
    Ok(turn.execution.and_then(|execution| execution.occupancy))
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
