//! Bounded detached reads of immutable compaction inputs. No execution authority.

//! Bounded detached reads of immutable compaction inputs. No execution authority.

use crate::{ConversationStoreError, store::canonical::TranscriptCursor};
use rustx_types::context::occupancy::ContextBreakdown;
use rustx_types::events::types::RuntimeEventEnvelope;
use rustx_types::message::types::MessageBlock;

pub use rustx_types::context::resources::{JournalFrontier, LedgerFrontier, ResourceReadCut};

/// Hard logical materialization bounds, including the sum of all pages in an operation.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub page_items: usize,
    pub record_bytes: usize,
    pub page_bytes: usize,
    pub total_items: usize,
    pub total_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            page_items: 64,
            record_bytes: 1024 * 1024,
            page_bytes: 4 * 1024 * 1024,
            total_items: 8192,
            total_bytes: 16 * 1024 * 1024,
        }
    }
}

impl ResourceLimits {
    pub fn validate(self) -> Result<(), ConversationStoreError> {
        let hard = Self::default();
        if self.page_items == 0
            || self.page_items > hard.page_items
            || self.record_bytes == 0
            || self.record_bytes > hard.record_bytes
            || self.page_bytes == 0
            || self.page_bytes > hard.page_bytes
            || self.total_items == 0
            || self.total_items > hard.total_items
            || self.total_bytes == 0
            || self.total_bytes > hard.total_bytes
        {
            return Err(bound("invalid compaction resource limits"));
        }
        Ok(())
    }
}

/// Detached owned rows and their byte charge; the position is in the source domain.
#[derive(Debug)]
pub struct ResourcePage<T> {
    pub rows: Vec<(u64, T)>,
    pub bytes: usize,
}

/// One canonical typed working record, with source positions and real facts.
#[derive(Debug, serde::Serialize)]
pub struct TrajectoryMessage {
    pub message: MessageBlock,
    pub transcript_position: Option<TranscriptCursor>,
    pub committed: Option<RuntimeEventEnvelope>,
    pub tools: Vec<TrajectoryTool>,
}
#[derive(Debug, serde::Serialize)]
pub struct TrajectoryTool {
    pub occurrence: rustx_types::message::types::ToolCallOccurrenceRef,
    /// Real Started/terminal facts as of the cut; empty is unknown/inherited.
    pub facts: Vec<RuntimeEventEnvelope>,
}

/// Reject a compaction read that exceeded its declared bound.
#[must_use]
pub fn bound(message: &str) -> ConversationStoreError {
    ConversationStoreError::ResourceBoundExceeded(message.to_owned())
}

/// Count encoding directly into a bounded sink; never allocate another source-sized buffer.
pub fn bounded_encoded_len(
    value: &impl serde::Serialize,
    limit: usize,
) -> Result<usize, ConversationStoreError> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("materialization byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| bound("cumulative compaction bytes exceeded"))?;
    Ok(counter.bytes)
}

/// old request measurement; source/destination Journal domains are never compared.
#[must_use]
pub fn inherit(
    turns: &[crate::reading::TurnReadingProvenance],
    canonical: &[rustx_types::message::types::MessageBlock],
    projections: &[crate::projection::ProjectionCheckpoint],
) -> Option<rustx_types::context::occupancy::ContextOccupancy> {
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

/// Fails only when the frozen request cannot be serialized for pricing.
pub fn measure(
    usage: &rustx_types::model::auxiliary::ModelUsage,
    snapshot: &crate::model::snapshot::RequestSnapshot,
) -> Result<Option<rustx_types::context::occupancy::ContextOccupancy>, crate::ConversationStoreError>
{
    let Some(input_tokens) = usage.input_tokens else {
        return Ok(None);
    };
    if snapshot.context_window_tokens == 0 {
        return Ok(None);
    }
    // The same serialized forms the request estimate prices; an absent part costs nothing.
    let estimate = |bytes: usize| crate::context_resources::bytes_to_tokens(bytes as u64);
    let system_tokens = if snapshot.effective_system_prompt.is_empty() {
        0
    } else {
        estimate(
            serde_json::to_vec(&snapshot.effective_system_prompt)
                .map_err(|error| {
                    crate::ConversationStoreError::InvalidReference(error.to_string())
                })?
                .len(),
        )
    };
    let tool_tokens = if snapshot.tool_definitions.is_empty() {
        0
    } else {
        estimate(
            serde_json::to_vec(&snapshot.tool_definitions)
                .map_err(|error| {
                    crate::ConversationStoreError::InvalidReference(error.to_string())
                })?
                .len(),
        )
    };
    Ok(Some(rustx_types::context::occupancy::ContextOccupancy {
        estimated: false,
        input_tokens,
        context_window_tokens: snapshot.context_window_tokens,
        model: snapshot.invocation.model.clone(),
        breakdown: ContextBreakdown {
            system_tokens,
            tool_tokens,
            message_tokens: input_tokens.saturating_sub(system_tokens.saturating_add(tool_tokens)),
        },
    }))
}

/// as one estimated token, with any remainder counting as one more.
#[must_use]
pub const fn bytes_to_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}
