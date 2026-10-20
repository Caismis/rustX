//! The Runtime Client projection: the one linearization owner of externally
//! visible Runtime Client state.
//!
//! [`RuntimeClientProjection`] owns:
//!
//! ```text
//! current snapshot read model
//! RuntimeClientCursor allocation
//! published RuntimeClientEvent sequence
//! bounded in-memory replay/resume state
//! subscriber registration and delivery
//! ```
//!
//! Every externally meaningful runtime transition follows one path:
//!
//! ```text
//! authoritative runtime transition commits
//!         |
//!         v
//! projection fold (update the snapshot read model)
//!         |
//!         +--> allocate the next RuntimeClientCursor
//!         +--> publish the corresponding RuntimeClientEvent
//!         +--> deliver to every subscriber
//! ```
//!
//! The host captures a finite working transition under its projection lock,
//! encodes that frozen candidate after unlocking, then installs its cursor and
//! replay record under the same lock. Readers retain the previous published
//! seed until the complete source proof and encoded transition are installed:
//!
//!
//! > The returned snapshot describes authoritative Runtime Client state at
//! > cursor C. A subscription/resume after C observes every subsequently
//! > published Runtime Client event in that stream, or fails explicitly
//! > with `resync_required`.
//!
//! # Projection replay cache and the one retained backlog
//!
//! Replay is a bounded in-memory ring (`cursor -> RuntimeClientEvent`)
//! with an explicit retention limit. That ring is the **sole** retained
//! Runtime Client event backlog: a subscription is a consumed cursor into
//! it, never a second queue.
//!
//! ```text
//! publish  -> allocate cursor -> push into the bounded ring
//!                             -> evict beyond the retention limit
//!                             -> wake every subscriber (edge-triggered)
//!
//! consume  -> subscriber reads the next retained entry after its cursor
//!          -> advances its own cursor
//!          -> falling behind retention yields resync_required
//! ```
//!
//! Consequences, which Issue #38 transport backpressure depends on:
//!
//! - a slow consumer costs one `RuntimeClientCursor`, never buffered
//!   events, so subscriber memory is O(1) and total retained memory is
//!   bounded by `replay_limit`;
//! - a consumer that falls behind retention can never silently skip
//!   events while still looking cursor-contiguous — the next poll is the
//!   explicit [`RuntimeClientError::ResyncRequired`] terminal result;
//! - wakeups are edge-triggered notifications only and carry no payload,
//!   so notification can never grow without bound and never blocks the
//!   publisher.
//!
//! The ring is not durable persistence and is never a recovery input. Durable
//! Event Journal reads and current Surface bootstrap remain `ConversationStore`
//! responsibilities.
//!
//! Live native interactions follow the same fold. A pending Approval or
//! Questionnaire is snapshot state and a terminal interaction observation
//! removes it; neither becomes canonical conversation history. The projection
//! never answers an interaction or infers an outcome from attachment loss.
//!
//! # `RuntimeEvent` mapping policy
//!
//! Every internal [`RuntimeEvent`] variant is classified explicitly:
//!
//! ```text
//! PROJECT                         -> one RuntimeClientEvent
//! FOLD INTO CLIENT STATE ONLY     -> snapshot update, no client event
//! INTERNAL / NOT EXPOSED          -> no snapshot change, no client event
//! ```
//!
//! Internal provider/request mechanics ([`ModelRequestStarted`],
//! [`ModelRequestFailed`], [`ModelRetryScheduled`]) and compaction
//! mechanics stay internal unless they express a client-relevant semantic
//! fact. The mapping is defined here, in one place, so internal
//! `RuntimeEvent` evolution cannot silently break the Runtime Client protocol.
//! Goal is seeded at the same runtime bootstrap cut and all live Goal changes
//! fold through this owner. Goal journal facts stay audit-only. Every folded
//! Goal change is a durable phase/revision change: there is no activation-only
//! Goal event, and no snapshot or event can represent an Active-but-inert Goal.
//!
//! # Module structure
//!
//! One owner, [`RuntimeClientProjection`], with its inherent implementation
//! grouped by responsibility. No child owns a lock, task or cursor of its own.
//!
//! - lifecycle: construction, bootstrap from the native durable cut, activation;
//! - observe → fold / journal / canonical: admission and folding of native
//!   observations into the read model and its client events;
//! - publication: event staging and encoding, the bounded replay ring;
//! - subscription: subscriber cursors and wakeups over that ring;
//! - read / `read_domains` / `checkpoint_seed`: the read interface over source-owned
//!   cuts, durable read domains, and the off-lock checkpoint seed;
//! - `in_flight` / views: pure in-flight block helpers and native snapshot views.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::Notify;

use crate::observation::failure::RuntimeClientAttemptFailure;

use super::event::{RuntimeClientEvent, RuntimeClientOutcome};
use super::snapshot::{
    AGENT_STATUS_WINDOW, InFlightAssistantMessage, InFlightBlock, InboundDiagnostics,
    InboundDrainView, InboundItemView, RuntimeClientAttempt, RuntimeClientAttemptPhase,
    RuntimeClientContextView, RuntimeClientJob, RuntimeClientSnapshot,
};
use super::types::{RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::observation::capability_source::CapabilityView;
use crate::observation::head::RuntimeClientCompactionView;
use crate::observation::status::AgentStatusView;
use crate::observation::tool::{ForegroundToolExecution, ForegroundToolState};
use crate::observation::transcript::RuntimeClientTranscriptCursor;
use crate::publication::{PublicationFrame, PublicationPayload};
use crate::runtime::identity::{AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime::inbound::InboundItem;
use crate::runtime::observation::ConversationObservation;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

mod canonical;
mod checkpoint_seed;
mod fold;
mod in_flight;
mod journal;
mod lifecycle;
mod observe;
mod publication;
mod read;
mod read_domains;
mod subscription;
pub(crate) mod views;

use in_flight::{
    append_inline_prefix, arguments_of, block_index_of, bounded_diagnostic, bounded_outcome,
    defer_result_content, deferred_live_block, foreground_slot_for_event, inline_block_bytes,
    inline_label, inline_prefix, push_in_flight_block, retain_tool_execution,
    update_stored_tool_slots,
};
#[cfg(test)]
use publication::encoded_event_bytes;
use publication::stamp_trace_cut;
use views::{
    accumulate_finite_facts, activation_view, admit_status, agent_view, background_view,
    client_failure, inbound_item_view, set_checkpoint_transition_cursor, upsert_background,
    upsert_interaction, upsert_subagent,
};

/// The default bounded replay retention of the projection observation stream.
///
/// The ring retains at most this many published events; a resume after an
/// expired cursor fails with `resync_required` and the client repairs with
/// a fresh snapshot. This is an in-memory bound, never a durability claim.
pub const RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT: usize = 4096;
/// Encoded byte ceiling for every event, in addition to the entry-count limit.
pub const RUNTIME_CLIENT_REPLAY_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
struct ReplayWeight(usize);
#[cfg(test)]
impl std::io::Write for ReplayWeight {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .saturating_add(bytes.len())
            .min(RUNTIME_CLIENT_REPLAY_BYTES + 1);
        if self.0 > RUNTIME_CLIENT_REPLAY_BYTES {
            return Err(std::io::Error::other("event exceeds replay byte budget"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One registered subscriber of the observation stream.
///
/// A subscriber owns **no** event storage: it is a consumed cursor into
/// the one bounded replay ring plus an edge-triggered wakeup handle. A
/// stalled subscriber therefore cannot grow memory and cannot make the
/// publisher block.
struct Subscriber {
    /// The opaque registration identity (attachment-scoped).
    id: u64,
    /// The cursor through which this subscriber already consumed events.
    consumed: RuntimeClientCursor,
    /// The edge-triggered wakeup handle. Carries no payload.
    notify: Arc<Notify>,
}

/// The result of one subscriber poll against the bounded replay ring.
// The event variant is the overwhelmingly common one and is produced once
// per delivered event; boxing it would add an allocation to every delivery
// to shrink a short-lived stack value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SubscriberPoll {
    /// The next retained event after the subscriber's consumed cursor.
    /// The subscriber's cursor advanced to this event's cursor.
    Event(RuntimeClientProtocolEvent),
    /// The subscriber is caught up: nothing published after its cursor.
    Pending,
    /// The subscription is no longer registered (detach, re-subscribe, or
    /// a dropped handle).
    Closed,
    /// The subscriber fell behind the bounded retention: the events it
    /// still needs were evicted. This is terminal for the subscription and
    /// is reported explicitly instead of silently skipping the gap.
    Lagged {
        /// The cursor the subscriber consumed through.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The native live source failed; retained cursors have not been skipped.
    Unavailable { message: String },
    /// The cursor space is exhausted; the observation stream is over.
    Exhausted,
}

/// The result of applying one foreground settlement to the projection.
///
/// Only an execution-terminal Journal fact settles the lifecycle. Canonical
/// result commitment is retained separately. A missing slot never invents an
/// execution or tool occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundSettlement {
    Applied,
    AlreadySettled,
    Missing,
}

// The working owner and an issued cut share immutable map nodes. Updating a
// single source copies only its tree path; published roots remain unchanged.
type DomainSources = imbl::OrdMap<String, (Arc<super::domain::DomainSource>, usize, u64)>;
type ContentMetadata = imbl::OrdMap<
    (&'static str, String, u32, &'static str),
    Arc<crate::observation::content::ContentRef>,
>;

#[derive(Default)]
struct ObservationSourceState {
    source_unavailable: bool,
    source_layout_failure: bool,
    journal_failure: bool,
}

#[derive(Default)]
struct NativePublicationState {
    installing_native_batch: bool,
    publication_commit: bool,
}

/// Finite working state and the published seed guarded by the host's projection
/// lock. Native owners install complete receipts through the CPU publication
/// sink; the worker prepares indexed source facts outside this lock. Acquiring
/// the lock alone never drains source work or advances the published cursor.
pub(crate) struct RuntimeClientProjection {
    #[cfg(test)]
    encoding_profile: Option<Arc<std::sync::Mutex<PublicationEncodingProfile>>>,
    activation_id: Option<crate::runtime::identity::SubagentId>,
    staged_publication: Option<(Vec<RuntimeClientEvent>, usize, bool)>,
    published_seed: Option<Arc<CheckpointSeed>>,
    published_sources: Option<DomainSources>,

    seen_activations: std::collections::HashSet<crate::runtime::identity::SubagentId>,
    journal_through: u64,
    /// Greatest installed native receipt; this never authorizes an earlier hole.
    native_journal_highwater: u64,
    /// Canonical dependency named by an installed native receipt, not permission to read it.
    native_canonical_highwater: u64,
    canonical_observed_through: u64,
    publication_state: NativePublicationState,
    /// Latest represented prefix that changed a durable read dependency.
    /// Trace-only progress must not starve a Session snapshot candidate.
    read_through: u64,
    source_cut: Option<crate::durable::observation::ObservationReadCut>,
    head_bindings:
        std::collections::BTreeMap<MessageId, Vec<crate::observation::cut::ContentBinding>>,
    foreground_content: Vec<ForegroundResultContent>,
    prepared_messages: std::collections::BTreeMap<
        MessageId,
        (
            crate::durable::observation::MessageHeader,
            Option<crate::conversation::SurfaceRevision>,
        ),
    >,
    bootstrap_transcript_through: u64,
    #[cfg(test)]
    pending_transcript: std::collections::BTreeMap<MessageId, crate::durable::TranscriptCursor>,
    publication_start: Option<crate::publication::PublicationStreamStart>,
    content_metadata: ContentMetadata,
    prepared_publication_content: Vec<super::checkpoint::PreparedPublicationContent>,
    batch_events: Option<(Vec<RuntimeClientEvent>, usize, bool)>,
    transcript_prefix: u64,
    #[cfg(test)]
    prepared_terminals: std::collections::BTreeMap<AttemptId, crate::durable::TranscriptCursor>,
    #[cfg(test)]
    prepared_pending: Option<Vec<InboundItemView>>,
    deferred_events: (Vec<RuntimeClientEvent>, usize, bool),
    canonical_foreground: Option<crate::observation::cut::CanonicalForegroundReadCut>,
    native_tool_occurrences: std::collections::BTreeMap<
        ToolCallId,
        crate::observation::tool::RuntimeClientToolOccurrence,
    >,
    live_revision: u64,
    live_block_total: u64,
    live_tool_total: u64,
    live_block_ordinal: Option<u64>,
    live_tool_ordinal: Option<u64>,
    pending_epoch_wait: Option<u64>,
    pending_mutation_revision: u64,
    source_state: ObservationSourceState,
    prepared_outcomes: std::collections::BTreeMap<
        u64,
        (
            bool,
            Option<crate::observation::tool::JournalFieldRef>,
            bool,
            Option<crate::observation::tool::JournalFieldRef>,
        ),
    >,
    prepared_journal: std::collections::BTreeMap<
        u64,
        (
            Option<RuntimeClientAttemptFailure>,
            Option<crate::observation::tool::JournalFieldRef>,
        ),
    >,
    live_frontiers: std::collections::BTreeMap<
        (
            crate::runtime::identity::PublicationStreamId,
            ContentBlockIndex,
            &'static str,
            &'static str,
        ),
        crate::observation::content::ContentRef,
    >,
    read_domains_dirty: bool,
    /// Failure of the current read-domain fence; cleared by matching success
    /// or a new invalidating revision, never by an obsolete read.
    read_failure: Option<String>,
    read_revision: u64,
    domain_sources: DomainSources,
    domain_source_bytes: usize,
    source_frontiers:
        std::collections::BTreeMap<String, crate::observation::cut::CollectionFrontier>,
    collection_cuts:
        std::collections::BTreeMap<String, crate::runtime::identity::ObservationCollectionCut>,
    pending_heads: std::collections::BTreeMap<
        String,
        Vec<crate::observation::interaction::RuntimeClientInteractionHeader>,
    >,
    collection_ordinal: Option<u64>,
    tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
    domain_revisions: std::collections::BTreeMap<&'static str, u64>,
    /// The cursor of the last published event (0 = nothing published yet).
    cursor: RuntimeClientCursor,
    /// Set when the cursor space is exhausted: publishing stops and
    /// snapshot/subscribe fail explicitly. Never wraps.
    exhausted: bool,
    /// The deterministic snapshot read model.
    snapshot: RuntimeClientSnapshot,
    capability_head: crate::observation::capability_source::PreparedCapabilityHead,
    trace_head: super::checkpoint::PreparedTraceHead,
    /// The bounded projection replay ring, oldest first.
    replay: VecDeque<(RuntimeClientCursor, RuntimeClientEvent)>,
    /// Scalar cut-to-cursor lookup, bounded by the same replay retention.
    replay_cut_cursors: HashMap<u64, RuntimeClientCursor>,
    /// Genuine constructor cut, retained only while replay after cursor zero is serviceable.
    initial_read_cut: Option<crate::observation::cut::CheckpointReadCut>,
    replay_sizes: VecDeque<usize>,
    replay_bytes: usize,
    /// The explicit bounded retention limit.
    replay_limit: usize,
    /// The registered subscribers in registration order.
    subscribers: Vec<Subscriber>,
    /// The next opaque subscriber registration identity.
    next_subscriber_id: u64,
    /// Test-only linearization hooks for controlled race tests.
    #[cfg(test)]
    probe: Option<crate::runtime_client::test_sync::ProjectionProbe>,
}

fn canonical_locator_position(
    locator: &crate::observation::content::CanonicalContentLocator,
) -> u64 {
    match locator {
        crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position } => {
            *position
        }
        crate::observation::content::CanonicalContentLocator::ByMessageId { .. } => u64::MAX,
    }
}

fn canonical_header_position(header: &crate::durable::observation::MessageHeader) -> Option<u64> {
    header.ledger_position
}

/// One constructor-prepared finite Tool reading with its immutable sources.
pub(crate) struct PreparedDurableToolReading {
    pub occurrence: crate::observation::tool::RuntimeClientToolOccurrence,
    pub content: Vec<crate::observation::cut::ContentBinding>,
    pub collections: Vec<crate::observation::cut::CollectionFrontier>,
}

#[derive(Clone)]
struct ForegroundResultContent {
    occurrence: crate::observation::tool::RuntimeClientToolOccurrence,
    metadata: crate::observation::content::ContentRef,
    content: Vec<crate::observation::cut::ContentBinding>,
    frontier: crate::observation::cut::CollectionFrontier,
}

#[derive(Clone)]
pub(crate) struct CheckpointSeed {
    snapshot: RuntimeClientSnapshot,
    capability_head: crate::observation::capability_source::PreparedCapabilityHead,
    trace_head: super::checkpoint::PreparedTraceHead,
    cursor: RuntimeClientCursor,
    read_cut: crate::observation::cut::CheckpointReadCut,
    exhausted: bool,
    journal_failure: bool,
    source_pending: bool,
    source_cut: Option<crate::durable::observation::ObservationReadCut>,
    head_bindings:
        std::collections::BTreeMap<MessageId, Vec<crate::observation::cut::ContentBinding>>,
    foreground_content: Vec<ForegroundResultContent>,
    source_frontiers:
        std::collections::BTreeMap<String, crate::observation::cut::CollectionFrontier>,
    collection_cuts:
        std::collections::BTreeMap<String, crate::runtime::identity::ObservationCollectionCut>,
    domain_revisions: std::collections::BTreeMap<&'static str, u64>,
    content_metadata: ContentMetadata,
    publication_start: Option<crate::publication::PublicationStreamStart>,
    live_frontiers: std::collections::BTreeMap<
        (
            crate::runtime::identity::PublicationStreamId,
            ContentBlockIndex,
            &'static str,
            &'static str,
        ),
        crate::observation::content::ContentRef,
    >,
    live_block_total: u64,
    live_tool_total: u64,
    canonical_foreground: Option<crate::observation::cut::CanonicalForegroundReadCut>,
}

pub(crate) struct PublicationDraft {
    #[cfg(test)]
    encoding_profile: Option<Arc<std::sync::Mutex<PublicationEncodingProfile>>>,
    seed: CheckpointSeed,
    events: Vec<RuntimeClientEvent>,
    replacement: bool,
    sources: DomainSources,
}

#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub(crate) struct PublicationEncodingProfile {
    pub records: u64,
    pub bytes: usize,
    pub seed_us: u128,
    pub content_metadata_max: usize,
    pub capture_us: u128,
    pub fit_us: u128,
    pub probe_us: u128,
    pub encode_us: u128,
}

pub(crate) struct EncodedPublication {
    seed: Arc<CheckpointSeed>,
    event: RuntimeClientEvent,
    bytes: usize,
    sources: DomainSources,
}

/// Host identity is fixed for the lifetime of a live projection. Revision
/// fences every durable-domain observation, independently of publication cursors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadDomainCut {
    revision: u64,
    through: u64,
    source_cut: Option<crate::durable::observation::ObservationReadCut>,
}

#[derive(Clone)]
pub(crate) struct ReadDomainValues {
    pub(crate) projection_head: Option<crate::observation::head::RuntimeClientProjectionHeadView>,
    pub(crate) transcript: crate::observation::transcript::RuntimeClientTranscriptPage,
    pub(crate) occupancy: Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
    pub(crate) trace: crate::observation::trace::TracePage,
    pub(crate) trace_total: u64,
}

type ReadDomains = Result<ReadDomainValues, crate::durable::ConversationStoreError>;

/// Request-owned copy, independent of the live materializer's installation fence.
#[cfg(test)]
pub(crate) struct SnapshotCandidate {
    pub(crate) snapshot: RuntimeClientSnapshot,
    pub(crate) cursor: RuntimeClientCursor,
    pub(crate) trace_through: u64,
    pub(crate) read_cut: ReadDomainCut,
}

/// The text block kinds folded into the in-flight message.
#[derive(Debug, Clone, Copy)]
enum TextKind {
    Text,
    Reasoning,
    Refusal,
}

#[cfg(test)]
mod tests;
