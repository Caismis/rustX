//! Read-only Trace inspection over native durable authorities.
//!
//! Nothing in this module is history, execution authority, settlement
//! authority, or recovery input. Reads select indexed Journal anchors and
//! join immutable Request Snapshots and canonical Ledger messages by exact
//! identity, under one captured read cut that excludes every later fact. The
//! browser never folds Journal events; it receives resolved records.
//!
//! Public pages and selected reads use finite native headers and indexed ranges.
//! The full semantic detail reader exists only as a test oracle. Paging never
//! hydrates original Request snapshots, Journal bodies or canonical messages.
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
//! Idle adoption        the first Attempt started after it (turn prompt rule)
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
pub(crate) use bounds::identity_fits;
mod content;
#[cfg(test)]
mod detail;
pub(crate) mod frozen;
#[cfg(test)]
mod oracle_summary;
pub(crate) use frozen::{
    request_detail as frozen_request_detail, request_outcome as frozen_request_outcome,
};
mod inspection;
mod lifecycle;
pub use inspection::{
    TraceInspectionContent, TraceInspectionHead, TraceInspectionMessage, TraceInspectionMessages,
    TraceToolDefinitionSelection,
};
mod live;
mod locate;
#[cfg(test)]
mod provenance_tests;
mod record;
pub(crate) mod request;
mod summary;
#[cfg(test)]
mod tool;
mod types;

pub use bounds::{
    TRACE_DETAIL_BYTES, TRACE_PAGE_BYTES, TRACE_RECORD_BYTES, TRACE_SUMMARY_CONTEXT,
    TRACE_SUMMARY_CONTEXT_BYTES, TraceJson, TracePreview, TraceText,
};
pub use locate::{TraceToolLocation, TraceToolLocator};
pub use types::{
    TraceArtifact, TraceContentBlock, TraceContextKind, TraceContextPresentation,
    TraceContextSource, TraceCursor, TraceDetail, TraceFieldKind, TraceFieldSource,
    TraceGeneration, TraceGenerationTimeline, TraceInputProvenance, TraceKind, TraceLifecycle,
    TraceLocation, TraceManagedOutput, TraceMessageDetail, TraceMessageRole, TracePage,
    TraceRecord, TraceRequestDetail, TraceRequestFailure, TraceRequestMessage, TraceRequestOption,
    TraceRequestOutcome, TraceRequestPredecessor, TraceRequestSummary, TraceState,
    TraceSystemPromptPresentation, TraceSystemPromptState, TraceTiming, TraceToolCall,
    TraceToolCatalogState, TraceToolDefinition, TraceToolDetail, TraceToolLifecycle,
    TraceToolOutcome, TraceToolOutcomeUpdate, TraceToolResult, TraceToolSource, TraceToolSummary,
    TraceToolTruncation,
};

use record::bound_record;

use crate::durable::observation::{TraceFactHeader, TraceFactKind, TraceFactQuery, TraceFactScope};
#[cfg(test)]
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
#[cfg(test)]
thread_local! {
    static PAGE_READ_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub(crate) fn take_page_read_work() -> usize {
    PAGE_READ_WORK.with(|work| work.replace(0))
}
#[cfg(test)]
use crate::events::types::RuntimeEventEnvelope;

/// Largest page a client may request.
pub const TRACE_PAGE_LIMIT: usize = 32;
/// Most records resolved together during checkpoint or Agent enrichment.
pub const TRACE_RECORD_LIMIT: usize = 8;
/// Most loaded identities admitted by one standalone Root Trace repair.
pub const TRACE_REFRESH_LIMIT: usize = 128;

/// The closed anchor vocabulary. One anchor becomes one ledger record.
#[cfg(test)]
pub(crate) const ANCHORS: &[&str] = &[
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
#[cfg(test)]
const ADOPTED_MESSAGE_LIMIT: usize = 8;
/// Most Step-scoped joins performed while resolving one Tool's owning request.
#[cfg(test)]
const STEP_JOIN_LIMIT: usize = 16;

/// Stateless read-only inspection owner. Dropping it changes no native state.
pub struct TraceProjection<'a> {
    store: &'a dyn ConversationStore,
    through: u64,
    source_cut: Option<crate::durable::observation::ObservationReadCut>,
}

impl<'a> TraceProjection<'a> {
    /// Materializes the exact prefix captured by the native observation cut.
    #[cfg(test)]
    pub(crate) fn through(store: &'a dyn ConversationStore, through: u64) -> Self {
        let source_cut = store
            .observation_read_cut()
            .ok()
            .filter(|cut| cut.read.journal == through);
        Self {
            store,
            through,
            source_cut,
        }
    }

    pub(crate) fn at_cut(
        store: &'a dyn ConversationStore,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Self, ConversationStoreError> {
        if &cut.read.conversation_id != store.conversation_id() {
            return Err(ConversationStoreError::InvalidReference(
                "foreign Trace source cut".into(),
            ));
        }
        Ok(Self {
            store,
            through: cut.read.journal,
            source_cut: Some(cut.clone()),
        })
    }

    pub(crate) fn history_cut(
        &self,
    ) -> Result<super::checkpoint::CheckpointReadCut, ConversationStoreError> {
        let native = self.cut()?.clone();
        Ok(super::checkpoint::CheckpointReadCut {
            conversation_id: native.read.conversation_id.clone(),
            activation_id: None,
            revision: 0,
            through: native.read.journal,
            source_cut: Some(native),
            canonical_foreground: None,
            live_publication: None,
            domain_revisions: std::collections::BTreeMap::new(),
            collection_cuts: std::collections::BTreeMap::new(),
        })
    }

    fn cut(
        &self,
    ) -> Result<&crate::durable::observation::ObservationReadCut, ConversationStoreError> {
        self.source_cut.as_ref().ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "Trace canonical joins require the original complete source cut".into(),
            )
        })
    }

    /// Captures one durable read cut. Every later join excludes newer facts.
    ///
    /// # Errors
    ///
    /// Returns the durable read error without interpreting it as absent
    /// history: an unreadable store is not an empty conversation.
    pub fn new(store: &'a dyn ConversationStore) -> Result<Self, ConversationStoreError> {
        let source_cut = store.observation_read_cut()?;
        Ok(Self {
            store,
            through: source_cut.read.journal,
            source_cut: Some(source_cut),
        })
    }

    fn facts(
        &self,
        scope: TraceFactScope,
        kinds: &[TraceFactKind],
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TraceFactHeader>, ConversationStoreError> {
        self.store.observation_trace_facts(
            self.cut()?,
            &TraceFactQuery {
                scope,
                kinds: kinds.to_vec(),
                before,
                after: 0,
                ascending: false,
                limit,
            },
        )
    }
    fn ending(
        &self,
        anchor_sequence: u64,
        kinds: &[TraceFactKind],
    ) -> Result<Option<TraceFactHeader>, ConversationStoreError> {
        Ok(self
            .facts(TraceFactScope::Owner { anchor_sequence }, kinds, None, 1)?
            .pop())
    }
    fn next_attempt(&self, after: u64) -> Result<Option<TraceFactHeader>, ConversationStoreError> {
        Ok(self
            .store
            .observation_trace_facts(
                self.cut()?,
                &TraceFactQuery {
                    scope: TraceFactScope::All,
                    kinds: vec![TraceFactKind::AttemptStarted],
                    before: None,
                    after,
                    ascending: true,
                    limit: 1,
                },
            )?
            .pop())
    }

    #[cfg(test)]
    fn original_facts(
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

    #[cfg(test)]
    fn original_ending(
        &self,
        scope: FactScope,
        kinds: &[&'static str],
    ) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
        Ok(self.original_facts(scope, kinds, None, 1)?.pop())
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
        #[cfg(test)]
        PAGE_READ_WORK.with(|work| work.set(work.get() + 1));
        if limit == 0 || limit > TRACE_PAGE_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "Trace limit must be 1..=32".into(),
            ));
        }
        let mut anchors = self.facts(
            TraceFactScope::All,
            &anchor_kinds(),
            before.map(TraceCursor::sequence).transpose()?,
            limit,
        )?;
        let more = if anchors.len() == limit {
            let before = anchors.last().map(|anchor| anchor.sequence);
            !self
                .facts(TraceFactScope::All, &anchor_kinds(), before, 1)?
                .is_empty()
        } else {
            false
        };
        anchors.reverse();
        let mut next_cursor = more.then(|| TraceCursor::at(anchors[0].sequence));
        let mut records: Vec<TraceRecord> = anchors
            .iter()
            .map(|anchor| self.record(anchor))
            .collect::<Result<_, _>>()?;
        for record in &mut records {
            bound_record(record);
            if bounds::encoded_len(record) > bounds::TRACE_RECORD_BYTES {
                return Err(ConversationStoreError::ObservationCapacityExceeded);
            }
        }
        // Bound encoded bytes, including JSON escaping, not just counts. A
        // page that cannot fit drops its oldest rows and moves its cursor
        // back to them, so paging still terminates and nothing is skipped.
        while records.len() > 1 && bounds::encoded_len(&records) > TRACE_PAGE_BYTES {
            records.remove(0);
            next_cursor = Some(records[0].position.clone());
        }
        let page = TracePage {
            read_cut: Some(self.history_cut()?),
            updates: Vec::new(),
            records,
            next_cursor,
        };
        ensure_page_bound(&page)?;
        Ok(page)
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
    #[cfg(test)]
    pub fn detail(&self, id: &str) -> Result<Option<TraceDetail>, ConversationStoreError> {
        let Some(anchor) = self.original_anchor_at(&TraceCursor(id.to_owned()))? else {
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
    ) -> Result<Option<TraceFactHeader>, ConversationStoreError> {
        let sequence = cursor.sequence()?;
        Ok(self
            .store
            .observation_trace_facts(
                self.cut()?,
                &TraceFactQuery {
                    scope: TraceFactScope::All,
                    kinds: anchor_kinds(),
                    before: Some(sequence.saturating_add(1)),
                    after: sequence.saturating_sub(1),
                    ascending: false,
                    limit: 1,
                },
            )?
            .pop()
            .filter(|event| event.sequence == sequence))
    }

    #[cfg(test)]
    fn original_anchor_at(
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

    /// Child refresh resolves only lifecycle facts, then exact live activity.
    pub(crate) fn refresh_agent(
        &self,
        records: &[TraceCursor],
        agent: &super::snapshot::RuntimeClientAgent,
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
                live::repair_agent_anchor(&mut facts, agent);
                Ok(Some(facts.lifecycle()?))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|updates| updates.into_iter().flatten().collect())
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
                Ok(Some(facts.lifecycle()?))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|updates| updates.into_iter().flatten().collect())
    }

    /// Fill one bounded reply with a complete prefix of the supplied interests.
    /// Missing anchors still consume their input position. Every lookup uses
    /// this projection's captured Source cut; no publication is produced.
    pub(crate) fn refresh_page(
        &self,
        page: &mut TracePage,
        records: &[TraceCursor],
        snapshot: Option<&super::snapshot::RuntimeClientSnapshot>,
    ) -> Result<usize, ConversationStoreError> {
        if records.len() > TRACE_REFRESH_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "too many Trace refresh interests".into(),
            ));
        }
        let mut refreshed = 0;
        for batch in records.chunks(TRACE_RECORD_LIMIT) {
            let previous = page.updates.len();
            // A lookup failure is an explicit read failure, even when an
            // earlier batch would have fitted. It is never hidden by paging.
            page.updates.extend(self.refresh(batch, snapshot)?);
            if let Err(error) = ensure_page_bound(page) {
                page.updates.truncate(previous);
                if refreshed == 0 {
                    return Err(error);
                }
                break;
            }
            refreshed += batch.len();
        }
        Ok(refreshed)
    }
}

pub(crate) fn ensure_page_bound(page: &TracePage) -> Result<(), ConversationStoreError> {
    let schema = crate::runtime_client::structural_json::root::<TracePage>();
    let encoded = crate::runtime_client::structural_json::Encoded {
        value: page,
        schema: &schema,
        root: &schema,
    };
    let limit = crate::runtime_client::checkpoint::CHECKPOINT_TARGET_BYTES
        - crate::runtime_client::checkpoint::CHECKPOINT_ITEM_BYTES;
    if crate::runtime_client::preflight::bounded_fit(&encoded, limit) == Some(true) {
        return Ok(());
    }
    if crate::runtime_client::checkpoint::bounded_bytes(&encoded, limit).is_none() {
        return Err(ConversationStoreError::ObservationCapacityExceeded);
    }
    Ok(())
}

/// Native Trace pages use their indexed J cursor as an exclusive boundary;
/// numeric offsets never infer a Trace identity from a loaded row rank.
pub(crate) fn checkpoint_frontier(
    total: u64,
    loaded: u64,
    next: Option<&TraceCursor>,
) -> super::checkpoint::CollectionFrontier {
    super::checkpoint::CollectionFrontier {
        domain: "trace.records".into(),
        revision: 0,
        total,
        loaded,
        next_offset: next.and_then(|cursor| cursor.sequence().ok()),
    }
}

pub(crate) fn checkpoint_page(
    store: &dyn ConversationStore,
    cut: &super::checkpoint::CheckpointReadCut,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, super::types::RuntimeClientError> {
    let unavailable = super::types::RuntimeClientError::observation_store_error;
    let before = (offset != 0).then(|| TraceCursor::at(offset));
    let source_cut = cut
        .source_cut
        .as_ref()
        .ok_or(super::types::RuntimeClientError::ReadCutExpired)?;
    let page = TraceProjection::at_cut(store, source_cut)
        .and_then(|projection| projection.page(before.as_ref(), limit))
        .map_err(unavailable)?;
    let total = store
        .observation_trace_total(cut.through)
        .map_err(unavailable)?;
    let mut frontier =
        checkpoint_frontier(total, page.records.len() as u64, page.next_cursor.as_ref());
    frontier.revision = cut.domain_revisions.get("trace").copied().unwrap_or(0);
    let items = page
        .records
        .iter()
        .map(|record| {
            let value = super::checkpoint::structural_value(record);
            Ok(super::checkpoint::CheckpointPageItem {
                offset: record.position.sequence().map_err(unavailable)?,
                identity: record.id.clone(),
                deferred: value.is_none(),
                value,
                content: Vec::new(),
                key: None,
                body_complete: None,
            })
        })
        .collect::<Result<Vec<_>, super::types::RuntimeClientError>>()?;
    Ok(super::checkpoint::CheckpointPage {
        cursor: super::types::RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: "trace.records".into(),
        total,
        items,
        next_offset: frontier.next_offset,
        collections: vec![frontier],
    })
}

#[cfg(test)]
pub(crate) use live::repair_live;
pub(crate) use live::{repair_agent_records, repair_records};

#[cfg(test)]
mod tests;

fn anchor_kinds() -> Vec<TraceFactKind> {
    use TraceFactKind as K;
    vec![
        K::AttemptStarted,
        K::TurnStarted,
        K::InboundTurnAdopted,
        K::ModelRequestStarted,
        K::AssistantMessageCommitted,
        K::ToolExecutionStarted,
        K::CompactionStarted,
        K::BackgroundExecutionCommitted,
        K::SubagentOwnershipCommitted,
        K::WorkflowStarted,
        K::InteractionRequested,
    ]
}
