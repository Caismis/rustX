//! Request-owned context occupancy. A provider measurement describes one exact
//! prepared request, never tokenized visible history. Compaction invalidates the
//! last request reading until a new request reports input again.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;

/// The last provider-measured request context for this Conversation.
/// It is explicitly a request reading, not an estimate of unsent composer text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOccupancy {
    /// Exact normalized effective request input, including cache reads.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub input_tokens: u64,
    /// Capacity frozen in the same request's native model snapshot.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub context_window_tokens: u64,
    /// Provider-facing historical model, never substituted from current config.
    pub model: String,
}

/// Project the latest prepared request's provider reading at a finite cut.
/// A newer unfinished request, or compaction, makes the prior reading absent.
/// # Errors
/// Durable read failures are not treated as missing evidence.
pub(crate) fn read(
    store: &dyn ConversationStore,
    through: u64,
) -> Result<Option<ContextOccupancy>, ConversationStoreError> {
    let boundary = store.read_presentation_events(&FactQuery {
        scope: FactScope::All,
        kinds: vec![
            "model_request_started",
            "compaction_started",
            "compaction_completed",
        ],
        before: None,
        after: 0,
        ascending: false,
        through,
        limit: 1,
    })?;
    let Some(event) = boundary.first() else {
        return Ok(None);
    };
    let RuntimeEvent::ModelRequestStarted { request_id, .. } = &event.event else {
        return Ok(None);
    };
    let terminal = store.read_presentation_events(&FactQuery {
        scope: FactScope::Request(request_id.to_string()),
        kinds: vec!["model_request_completed", "model_request_failed"],
        before: None,
        after: event.sequence,
        ascending: true,
        through,
        limit: 1,
    })?;
    let Some(
        RuntimeEvent::ModelRequestCompleted {
            usage: Some(usage), ..
        }
        | RuntimeEvent::ModelRequestFailed {
            usage: Some(usage), ..
        },
    ) = terminal.first().map(|event| &event.event)
    else {
        return Ok(None);
    };
    let snapshot = store.load_request_snapshot(request_id)?;
    if snapshot.context_window_tokens == 0 {
        return Ok(None);
    }
    Ok(Some(ContextOccupancy {
        input_tokens: usage.input_tokens,
        context_window_tokens: snapshot.context_window_tokens,
        model: snapshot.invocation.model,
    }))
}
