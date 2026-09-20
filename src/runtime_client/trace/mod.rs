//! Read-only Trace inspection over native durable authorities.
//!
//! Nothing in this module is history, execution authority, settlement
//! authority, or recovery input. Reads select indexed Journal anchors and
//! join immutable Request Snapshots and canonical Ledger messages by exact
//! identity, under one captured read cut that excludes every later fact. The
//! browser never folds Journal events; it receives resolved records.
//!
//! ## The two levels
//!
//! ```text
//! page()    bounded summary records   ledger rows, timeline spans, paging
//! detail()  one record's heavy facts  historical request, Tool, message
//! ```
//!
//! A page carries no request context, no Tool schema and no Tool result, so
//! paging cost stays independent of how large the inspected content is.
//! `detail` is fetched for the one record a reader selected, and — like
//! every other read here — mutates nothing: no cursor advances, no pending
//! work is consumed, and no lifecycle settles.
//!
//! ## Where each fact comes from
//!
//! | Authority | Facts |
//! | --- | --- |
//! | Event Journal | identities, ordering, starts, proven terminals |
//! | Immutable Request Snapshots | the exact historical request and its Tool catalog |
//! | Message Ledger | canonical accepted User / Assistant / Tool content |
//! | Canonical `ToolMessage` | the one Tool result authority |
//! | Request-owned generation evidence | settled TTFT / decode timing |
//! | Current runtime projections | positive current lifecycle evidence only |
//!
//! ## Resolved presentation relationships
//!
//! Some facts a reader needs are relationships between records rather than
//! facts about one record. They are resolved here, by native authority, and
//! never left for a client to infer from adjacency, names, timestamps or the
//! window it happens to have loaded:
//!
//! ```text
//! System Prompt state  nearest preceding actual request, in durable order
//! Context introduction RequestSnapshot.request_context_ids + Ledger reads
//! Tool-owned domains   the outer ToolCallId frozen in the native start fact
//! ```
//!
//! ## The two read responsibilities
//!
//! Immutable historical presentation and mutable lifecycle repair are
//! separate responsibilities, and neither depends on the other:
//!
//! ```text
//! anchor    native identity, grouping, own durable terminal   shared
//!   record    + bounded preview and the relationships above   page()
//!   lifecycle + current runtime evidence                      refresh()
//! ```
//!
//! A refresh therefore resolves no relationship a `TraceLifecycle` does not
//! carry. That is a correctness rule, not a performance note: an error
//! reached only while resolving a request's Context presentation must not
//! make that record's lifecycle repair unavailable.

mod anchor;
mod bounds;
mod content;
mod detail;
mod lifecycle;
mod live;
mod record;
mod request;
mod summary;
mod tool;
mod types;

pub use bounds::{
    TRACE_DETAIL_BYTES, TRACE_PAGE_BYTES, TRACE_RECORD_BYTES, TRACE_SUMMARY_CONTEXT,
    TRACE_SUMMARY_CONTEXT_BYTES, TraceJson, TracePreview, TraceText,
};
pub use types::{
    TraceArtifact, TraceContentBlock, TraceContextKind, TraceContextPresentation,
    TraceContextSource, TraceCursor, TraceDetail, TraceGeneration, TraceGenerationTimeline,
    TraceKind, TraceLifecycle, TraceLocation, TraceManagedOutput, TraceMessageDetail,
    TraceMessageRole, TracePage, TraceRecord, TraceRequestDetail, TraceRequestFailure,
    TraceRequestMessage, TraceRequestOption, TraceRequestOutcome, TraceRequestSummary, TraceState,
    TraceSystemPromptPresentation, TraceSystemPromptState, TraceTiming, TraceToolCall,
    TraceToolDefinition, TraceToolDetail, TraceToolLifecycle, TraceToolOutcome,
    TraceToolOutcomeUpdate, TraceToolResult, TraceToolSource, TraceToolSummary,
    TraceToolTruncation,
};

use record::bound_record;

use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEventEnvelope;

/// Largest page a client may request.
pub const TRACE_PAGE_LIMIT: usize = 32;
/// Most loaded records a client may ask the server to refresh at one cut.
pub const TRACE_RECORD_LIMIT: usize = 512;

/// The closed anchor vocabulary. One anchor becomes one ledger record.
const ANCHORS: &[&str] = &[
    "attempt_started",
    "turn_started",
    "inbound_turn_adopted",
    "model_request_started",
    "assistant_message_committed",
    "tool_execution_started",
    "compaction_started",
    "background_execution_committed",
    "subagent_ownership_committed",
    "workflow_started",
    "interaction_requested",
];

/// Most canonical messages projected for one adopted inbound turn.
const ADOPTED_MESSAGE_LIMIT: usize = 8;
/// Most Step-scoped joins performed while resolving one Tool's owning request.
const STEP_JOIN_LIMIT: usize = 16;

/// Stateless read-only inspection owner. Dropping it changes no native state.
pub struct TraceProjection<'a> {
    store: &'a dyn ConversationStore,
    through: u64,
}

impl<'a> TraceProjection<'a> {
    /// Materializes the exact prefix captured by the native observation cut.
    pub(crate) fn through(store: &'a dyn ConversationStore, through: u64) -> Self {
        Self { store, through }
    }

    /// Captures one durable read cut. Every later join excludes newer facts.
    ///
    /// # Errors
    ///
    /// Returns the durable read error without interpreting it as absent
    /// history: an unreadable store is not an empty conversation.
    pub fn new(store: &'a dyn ConversationStore) -> Result<Self, ConversationStoreError> {
        Ok(Self {
            store,
            through: store.presentation_frontier()?,
        })
    }

    fn facts(
        &self,
        scope: FactScope,
        kinds: &[&'static str],
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        self.store.read_presentation_events(&FactQuery {
            scope,
            kinds: kinds.to_vec(),
            before,
            after: 0,
            ascending: false,
            through: self.through,
            limit,
        })
    }

    fn ending(
        &self,
        scope: FactScope,
        kinds: &[&'static str],
    ) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
        Ok(self.facts(scope, kinds, None, 1)?.pop())
    }

    /// Newest or next-older finite page of bounded summary records.
    ///
    /// # Errors
    ///
    /// Rejects invalid cursors and limits, and propagates failed
    /// authoritative reads.
    pub fn page(
        &self,
        before: Option<&TraceCursor>,
        limit: usize,
    ) -> Result<TracePage, ConversationStoreError> {
        if limit == 0 || limit > TRACE_PAGE_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "Trace limit must be 1..=32".into(),
            ));
        }
        let mut anchors = self.facts(
            FactScope::All,
            ANCHORS,
            before.map(TraceCursor::sequence).transpose()?,
            limit + 1,
        )?;
        let more = anchors.len() > limit;
        anchors.truncate(limit);
        anchors.reverse();
        let mut next_cursor = more.then(|| TraceCursor::at(anchors[0].sequence));
        let mut records: Vec<TraceRecord> = anchors
            .iter()
            .map(|anchor| self.record(anchor))
            .collect::<Result<_, _>>()?;
        for record in &mut records {
            bound_record(record);
        }
        // Bound encoded bytes, including JSON escaping, not just counts. A
        // page that cannot fit drops its oldest rows and moves its cursor
        // back to them, so paging still terminates and nothing is skipped.
        while records.len() > 1 && bounds::encoded_len(&records) > TRACE_PAGE_BYTES {
            records.remove(0);
            next_cursor = Some(records[0].position.clone());
        }
        Ok(TracePage {
            records,
            next_cursor,
        })
    }

    /// Heavy inspection detail for one exact stable record identity.
    ///
    /// Returns `None` when the identity names no anchor at this read cut, so
    /// a record that has left the captured prefix is reported as absent
    /// rather than approximated by a neighbour.
    ///
    /// # Errors
    ///
    /// Rejects an unparseable identity and propagates failed authoritative
    /// reads.
    pub fn detail(&self, id: &str) -> Result<Option<TraceDetail>, ConversationStoreError> {
        let Some(anchor) = self.anchor_at(&TraceCursor(id.to_owned()))? else {
            return Ok(None);
        };
        let mut detail = self.build_detail(&anchor)?;
        // A detail response must fit the transport with room for the
        // envelope. Heavy sections are released in the order that keeps the
        // most inspection value: the reconstructed request context is the
        // largest and the most reconstructible from neighbouring records.
        if bounds::encoded_len(&detail) > TRACE_DETAIL_BYTES
            && let Some(request) = detail.request.as_mut()
        {
            request.messages.clear();
            request.messages_truncated = true;
            detail.truncated = true;
        }
        if bounds::encoded_len(&detail) > TRACE_DETAIL_BYTES
            && let Some(request) = detail.request.as_mut()
        {
            request.tools.clear();
            request.tools_truncated = true;
        }
        if bounds::encoded_len(&detail) > TRACE_DETAIL_BYTES
            && let Some(tool) = detail.tool.as_mut()
            && let Some(result) = tool.result.as_mut()
        {
            result.blocks.clear();
            result.blocks_truncated = true;
        }
        if bounds::encoded_len(&detail) > TRACE_DETAIL_BYTES {
            detail.messages.clear();
            detail.tool = None;
            detail.request = None;
            detail.truncated = true;
        }
        Ok(Some(detail))
    }

    /// Resolves one stable record identity to its exact anchor.
    ///
    /// The bounded query brackets the requested sequence and the result is
    /// checked for equality, so a neighbouring anchor can never stand in for
    /// a record that is absent at this cut.
    fn anchor_at(
        &self,
        cursor: &TraceCursor,
    ) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
        let sequence = cursor.sequence()?;
        Ok(self
            .store
            .read_presentation_events(&FactQuery {
                scope: FactScope::All,
                kinds: ANCHORS.to_vec(),
                before: Some(sequence.saturating_add(1)),
                after: sequence.saturating_sub(1),
                ascending: false,
                through: self.through,
                limit: 1,
            })?
            .pop()
            .filter(|event| event.sequence == sequence))
    }

    /// Refreshes loaded records' mutable lifecycle at this cut.
    ///
    /// Only the facts a [`TraceLifecycle`] transmits are resolved. The
    /// immutable historical relationships a summary row carries — the System
    /// Prompt predecessor, the canonical Context introduction and the
    /// recorded Tool name — are not projected, not read, and therefore
    /// cannot make lifecycle repair slow or unavailable.
    ///
    /// # Errors
    ///
    /// Rejects an interest set larger than [`TRACE_RECORD_LIMIT`], rejects
    /// an unparseable cursor, and propagates failed lifecycle reads.
    pub(crate) fn refresh(
        &self,
        records: &[TraceCursor],
        snapshot: Option<&super::snapshot::RuntimeClientSnapshot>,
    ) -> Result<Vec<TraceLifecycle>, ConversationStoreError> {
        if records.len() > TRACE_RECORD_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "too many Trace records".into(),
            ));
        }
        records
            .iter()
            .map(|cursor| {
                let Some(anchor) = self.anchor_at(cursor)? else {
                    return Ok(None);
                };
                let mut facts = self.anchor_facts(&anchor)?;
                if let Some(snapshot) = snapshot {
                    live::repair_anchor(&mut facts, snapshot);
                }
                Ok(Some(facts.lifecycle()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|updates| updates.into_iter().flatten().collect())
    }
}

pub(crate) use live::{repair_live, repair_records};

#[cfg(test)]
mod tests;
