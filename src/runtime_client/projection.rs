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

mod canonical_tools;
mod prepared_capture;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::Notify;

use super::event::{RuntimeClientAttemptFailure, RuntimeClientEvent, RuntimeClientOutcome};
use super::snapshot::{
    AGENT_STATUS_WINDOW, AgentStatusOpportunityView, AgentStatusView, CapabilityView,
    ForegroundToolExecution, ForegroundToolState, FreshInboundOpportunityView,
    InFlightAssistantMessage, InFlightBlock, InboundDiagnostics, InboundDrainView, InboundItemView,
    PostToolBatchOpportunityView, RuntimeClientAttempt, RuntimeClientAttemptPhase,
    RuntimeClientCompactionView, RuntimeClientContextView, RuntimeClientJob, RuntimeClientSnapshot,
    RuntimeClientStatusSection, RuntimeClientTodoStatusTask, RuntimeClientTranscriptCursor,
};
use super::types::{RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent};
use crate::agent::observer::AgentStatusObservation;
use crate::context::status::{AgentStatusSectionData, render_agent_status};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::publication::{PublicationFrame, PublicationPayload};
use crate::runtime::identity::{AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime::inbound::InboundItem;
use crate::runtime::observation::ConversationObservation;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

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

impl Drop for Subscriber {
    fn drop(&mut self) {
        // Wake a parked receiver on removal or destruction of the projection.
        // The receiver holds only a weak host reference and observes Closed.
        self.notify.notify_waiters();
        self.notify.notify_one();
    }
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
type ContentMetadata =
    imbl::OrdMap<(&'static str, String, u32, &'static str), Arc<super::content::ContentRef>>;

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
    head_bindings: std::collections::BTreeMap<MessageId, Vec<super::checkpoint::ContentBinding>>,
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
    canonical_foreground: Option<super::checkpoint::CanonicalForegroundReadCut>,
    native_tool_occurrences:
        std::collections::BTreeMap<ToolCallId, super::snapshot::RuntimeClientToolOccurrence>,
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
            Option<super::snapshot::JournalFieldRef>,
            bool,
            Option<super::snapshot::JournalFieldRef>,
        ),
    >,
    prepared_journal: std::collections::BTreeMap<
        u64,
        (
            Option<RuntimeClientAttemptFailure>,
            Option<super::snapshot::JournalFieldRef>,
        ),
    >,
    live_frontiers: std::collections::BTreeMap<
        (
            crate::runtime::identity::PublicationStreamId,
            ContentBlockIndex,
            &'static str,
            &'static str,
        ),
        super::content::ContentRef,
    >,
    read_domains_dirty: bool,
    /// One explicit checkpoint demand, coalesced until its bounded read completes.
    read_domains_requested: bool,
    /// Failure of the current read-domain fence; cleared by matching success
    /// or a new invalidating revision, never by an obsolete read.
    read_failure: Option<String>,
    read_revision: u64,
    domain_sources: DomainSources,
    domain_source_bytes: usize,
    source_frontiers: std::collections::BTreeMap<String, super::checkpoint::CollectionFrontier>,
    collection_cuts:
        std::collections::BTreeMap<String, crate::runtime::identity::ObservationCollectionCut>,
    pending_heads:
        std::collections::BTreeMap<String, Vec<super::snapshot::RuntimeClientInteractionHeader>>,
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
    capability_head: super::capability_source::PreparedCapabilityHead,
    trace_head: super::checkpoint::PreparedTraceHead,
    /// The bounded projection replay ring, oldest first.
    replay: VecDeque<(RuntimeClientCursor, RuntimeClientEvent)>,
    /// Scalar cut-to-cursor lookup, bounded by the same replay retention.
    replay_cut_cursors: HashMap<u64, RuntimeClientCursor>,
    /// Genuine constructor cut, retained only while replay after cursor zero is serviceable.
    initial_read_cut: Option<super::checkpoint::CheckpointReadCut>,
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

fn canonical_locator_position(locator: &super::content::CanonicalContentLocator) -> u64 {
    match locator {
        super::content::CanonicalContentLocator::ByLedgerPosition { position } => *position,
        super::content::CanonicalContentLocator::ByMessageId { .. } => u64::MAX,
    }
}

fn canonical_header_position(header: &crate::durable::observation::MessageHeader) -> Option<u64> {
    header.ledger_position
}

/// One constructor-prepared finite Tool reading with its immutable sources.
pub(crate) struct PreparedDurableToolReading {
    pub occurrence: super::snapshot::RuntimeClientToolOccurrence,
    pub content: Vec<super::checkpoint::ContentBinding>,
    pub collections: Vec<super::checkpoint::CollectionFrontier>,
}

#[derive(Clone)]
struct ForegroundResultContent {
    occurrence: super::snapshot::RuntimeClientToolOccurrence,
    metadata: super::content::ContentRef,
    content: Vec<super::checkpoint::ContentBinding>,
    frontier: super::checkpoint::CollectionFrontier,
}

#[derive(Clone)]
pub(crate) struct CheckpointSeed {
    prepared: prepared_capture::PreparedCaptures,
    read_domains: Option<ReadDomainCut>,
    snapshot: RuntimeClientSnapshot,
    capability_head: super::capability_source::PreparedCapabilityHead,
    trace_head: super::checkpoint::PreparedTraceHead,
    cursor: RuntimeClientCursor,
    read_cut: super::checkpoint::CheckpointReadCut,
    exhausted: bool,
    journal_failure: bool,
    source_pending: bool,
    source_cut: Option<crate::durable::observation::ObservationReadCut>,
    head_bindings: std::collections::BTreeMap<MessageId, Vec<super::checkpoint::ContentBinding>>,
    foreground_content: Vec<ForegroundResultContent>,
    source_frontiers: std::collections::BTreeMap<String, super::checkpoint::CollectionFrontier>,
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
        super::content::ContentRef,
    >,
    live_block_total: u64,
    live_tool_total: u64,
    canonical_foreground: Option<super::checkpoint::CanonicalForegroundReadCut>,
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

#[cfg(test)]
fn encoding_phase(
    timing: &mut Option<(PublicationEncodingProfile, std::time::Instant)>,
    record: fn(&mut PublicationEncodingProfile, u128),
) {
    if let Some((profile, start)) = timing {
        record(profile, start.elapsed().as_micros());
        *start = std::time::Instant::now();
    }
}

pub(crate) struct EncodedPublication {
    seed: Arc<CheckpointSeed>,
    event: RuntimeClientEvent,
    bytes: usize,
    sources: DomainSources,
}

#[cfg(test)]
fn encoded_event_bytes(event: &RuntimeClientEvent, limit: usize) -> Option<Vec<u8>> {
    let schema = super::structural_json::root::<RuntimeClientEvent>();
    super::checkpoint::bounded_bytes(
        &super::structural_json::Encoded {
            value: event,
            schema: &schema,
            root: &schema,
        },
        limit,
    )
}

fn stamp_trace_cut(
    trace: &mut super::trace::TracePage,
    cut: &super::checkpoint::CheckpointReadCut,
) {
    if trace.read_cut.is_none() && (!trace.records.is_empty() || !trace.updates.is_empty()) {
        trace.read_cut = Some(cut.clone());
    }
}

fn stamp_event_trace_cut(
    event: &mut RuntimeClientEvent,
    cut: &super::checkpoint::CheckpointReadCut,
) {
    match event {
        RuntimeClientEvent::ReadDomainsUpdated {
            trace: Some(trace), ..
        } => stamp_trace_cut(trace, cut),
        RuntimeClientEvent::ProjectionTransition { events, .. } => {
            for event in events {
                stamp_event_trace_cut(event, cut);
            }
        }
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            stamp_trace_cut(&mut checkpoint.snapshot.trace, &checkpoint.read_cut);
        }
        _ => {}
    }
}

impl PublicationDraft {
    /// CPU-only encoding of a frozen transition, outside every native state lock.
    #[allow(clippy::too_many_lines)] // Keep capture, admission, and replacement ordering explicit.
    pub(crate) fn encode(mut self) -> Result<EncodedPublication, RuntimeClientError> {
        #[cfg(test)]
        let mut timing = self.encoding_profile.as_ref().map(|_| {
            (
                PublicationEncodingProfile::default(),
                std::time::Instant::now(),
            )
        });
        let next = self
            .seed
            .cursor
            .get()
            .checked_add(1)
            .ok_or(RuntimeClientError::ProjectionExhausted)?;
        self.seed.cursor = RuntimeClientCursor::new(next);
        self.seed.read_cut.revision = next;
        if let Some(cut) = &mut self.seed.read_domains {
            cut.revision = next;
        }
        stamp_trace_cut(&mut self.seed.snapshot.trace, &self.seed.read_cut);
        for event in &mut self.events {
            stamp_event_trace_cut(event, &self.seed.read_cut);
        }
        let (event, encoded) = {
            use super::checkpoint::{
                CHECKPOINT_TARGET_BYTES, ProjectionMetadata, RuntimeClientCheckpoint,
                bounded_structural_len_with_budget, certified_checkpoint_fit, sort_metadata,
            };
            use prepared_capture::{CapturedCheckpoint, FitBudget};
            let CapturedCheckpoint {
                snapshot,
                mut metadata,
                cursor,
                evicted,
                fit_budget,
                mut metadata_budget,
            } = self.seed.capture_checkpoint()?;
            let mut snapshot = Some(snapshot);
            #[cfg(test)]
            encoding_phase(&mut timing, |profile, elapsed| {
                profile.capture_us += elapsed;
            });
            let mut checkpoint = None;
            let fitted = if certified_checkpoint_fit(fit_budget) {
                sort_metadata(&mut metadata.collections, &mut metadata.content);
                false
            } else {
                let mut owned = RuntimeClientCheckpoint {
                    snapshot: snapshot.take().expect("capture materialized once")(),
                    cursor,
                    read_cut: metadata.read_cut,
                    content: metadata.content,
                    collections: metadata.collections,
                };
                let fitted = super::checkpoint::fit_checkpoint_with_budget(&mut owned, fit_budget)?;
                metadata = ProjectionMetadata {
                    read_cut: owned.read_cut.clone(),
                    content: std::mem::take(&mut owned.content),
                    collections: std::mem::take(&mut owned.collections),
                };
                // Eviction may change frontier option tags/charges. The original
                // certificate describes the unmodified capture only.
                metadata_budget = None;
                checkpoint = Some(owned);
                fitted
            };
            #[cfg(test)]
            encoding_phase(&mut timing, |profile, elapsed| {
                profile.fit_us += elapsed;
            });
            let mut event = RuntimeClientEvent::ProjectionTransition {
                events: self.events,
                metadata,
            };
            set_checkpoint_transition_cursor(&mut event, cursor);
            let encoded = if fitted || evicted || self.replacement {
                None
            } else {
                let RuntimeClientEvent::ProjectionTransition { events, .. } = &event else {
                    unreachable!("publication draft is a projection transition");
                };
                let events_budget = metadata_budget.and_then(|_| {
                    super::preflight::required_safe_integer_budget(events, CHECKPOINT_TARGET_BYTES)
                });
                bounded_structural_len_with_budget(
                    &event,
                    CHECKPOINT_TARGET_BYTES,
                    FitBudget::transition(events_budget, metadata_budget),
                )
            };
            if encoded.is_none() {
                let transition = event_kind(&event).to_owned();
                // Descriptors belong to the outer transition in both paths.
                let RuntimeClientEvent::ProjectionTransition { metadata, .. } = event else {
                    unreachable!("publication draft is a projection transition");
                };
                let checkpoint = checkpoint.unwrap_or_else(|| RuntimeClientCheckpoint {
                    snapshot: snapshot.take().expect("capture materialized once")(),
                    cursor,
                    read_cut: metadata.read_cut.clone(),
                    content: Vec::new(),
                    collections: Vec::new(),
                });
                event = RuntimeClientEvent::ProjectionTransition {
                    events: vec![RuntimeClientEvent::CheckpointAdvanced {
                        transition,
                        checkpoint: Box::new(checkpoint),
                    }],
                    metadata,
                };
            }
            #[cfg(test)]
            encoding_phase(&mut timing, |profile, elapsed| {
                profile.probe_us += elapsed;
            });
            let encoded = encoded
                .or_else(|| {
                    bounded_structural_len_with_budget(
                        &event,
                        CHECKPOINT_TARGET_BYTES,
                        FitBudget::replacement(fit_budget, metadata_budget),
                    )
                })
                .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                    message: "finite display transition exceeds its encoded budget".into(),
                })?;
            (event, encoded)
        };
        #[cfg(test)]
        if let Some((mut measured, started)) = timing {
            measured.encode_us += started.elapsed().as_micros();
            let mut total = self.encoding_profile.as_ref().unwrap().lock().unwrap();
            total.records += 1;
            total.bytes += encoded;
            total.capture_us += measured.capture_us;
            total.fit_us += measured.fit_us;
            total.probe_us += measured.probe_us;
            total.encode_us += measured.encode_us;
        }
        Ok(EncodedPublication {
            seed: Arc::new(self.seed),
            event,
            bytes: encoded,
            sources: self.sources,
        })
    }
}

impl CheckpointSeed {
    fn checkpoint_read_cut(&self) -> super::checkpoint::CheckpointReadCut {
        self.read_cut.clone()
    }
    pub(crate) fn retain_foreground_progress(
        &self,
        page: &mut super::checkpoint::CheckpointPage,
    ) -> Result<(), RuntimeClientError> {
        // Disposable progress belongs only to this exact published live cut.
        // Retained historical pages reconstruct their durable Journal facts.
        if page.read_cut != self.read_cut {
            return Ok(());
        }
        let Some(attempt) = &self.snapshot.attempt else {
            return Ok(());
        };
        for item in &mut page.items {
            let Some(value) = item.value.take() else {
                continue;
            };
            let mut source: ForegroundToolExecution = super::checkpoint::decode_domain_value(value)
                .map_err(|_| RuntimeClientError::ObservationUnavailable {
                    message: "native foreground source header unavailable".into(),
                })?;
            if let ForegroundToolState::Running { progress, .. } = &mut source.state
                && let Some(current) = attempt
                    .foreground
                    .iter()
                    .find(|current| current.occurrence == source.occurrence)
                && let ForegroundToolState::Running { progress: live, .. } = &current.state
            {
                progress.clone_from(live);
            }
            item.value = Some(super::checkpoint::structural_value(&source).ok_or_else(|| {
                RuntimeClientError::ObservationUnavailable {
                    message: "finite foreground source header unavailable".into(),
                }
            })?);
        }
        Ok(())
    }

    fn domain_revision(&self, domain: &str) -> u64 {
        let owner = domain.split('.').next().unwrap_or(domain);
        self.domain_revisions.get(owner).copied().unwrap_or(0)
    }
    fn capture_context(
        &self,
        capture: &mut super::checkpoint::Capture,
    ) -> RuntimeClientContextView {
        let mut context = RuntimeClientContextView {
            projection_head: self.snapshot.context.projection_head.clone(),
            compaction_in_progress: self.snapshot.context.compaction_in_progress,
            compaction_count: self.snapshot.context.compaction_count,
            latest_compaction: capture
                .item(
                    "context.latest_compaction",
                    &self.snapshot.context.latest_compaction,
                )
                .flatten(),
            manual_compaction: capture
                .item(
                    "context.manual_compaction",
                    &self.snapshot.context.manual_compaction,
                )
                .flatten(),
            compaction_error: capture
                .item(
                    "context.compaction_error",
                    &self.snapshot.context.compaction_error,
                )
                .flatten(),
            occupancy: capture
                .item("context.occupancy", &self.snapshot.context.occupancy)
                .flatten(),
        };
        context.compaction_error = self
            .snapshot
            .context
            .compaction_error
            .as_ref()
            .map(|diagnostic| bounded_diagnostic(diagnostic));
        context
    }
    fn capture_attempt(
        &self,
        capture: &mut super::checkpoint::Capture,
    ) -> Option<super::snapshot::RuntimeClientAttempt> {
        self.snapshot
            .attempt
            .as_ref()
            .map(|attempt| super::snapshot::RuntimeClientAttempt {
                attempt_id: attempt.attempt_id.clone(),
                attempt_id_source: attempt.attempt_id_source.clone(),
                phase: match &attempt.phase {
                    super::snapshot::RuntimeClientAttemptPhase::Incomplete => {
                        super::snapshot::RuntimeClientAttemptPhase::Incomplete
                    }
                    super::snapshot::RuntimeClientAttemptPhase::Admitted => {
                        super::snapshot::RuntimeClientAttemptPhase::Admitted
                    }
                    super::snapshot::RuntimeClientAttemptPhase::Running => {
                        super::snapshot::RuntimeClientAttemptPhase::Running
                    }
                    super::snapshot::RuntimeClientAttemptPhase::Settled { outcome } => {
                        super::snapshot::RuntimeClientAttemptPhase::Settled {
                            outcome: bounded_outcome(outcome),
                        }
                    }
                },
                turn: attempt.turn,
                last_usage: attempt.last_usage.clone(),
                in_flight: attempt.in_flight.as_ref().map(|flight| {
                    super::snapshot::InFlightAssistantMessage {
                        message_id: flight.message_id.clone(),
                        blocks: self.capture_live_blocks(capture, &flight.blocks),
                    }
                }),
                foreground: self.capture_foreground(capture, &attempt.foreground),
                model: capture.item("attempt.model", &attempt.model).flatten(),
                execution_settings: capture
                    .item("attempt.execution_settings", &attempt.execution_settings)
                    .flatten(),
            })
    }

    pub(crate) fn native_read_cut(
        &self,
    ) -> Option<&crate::durable::observation::ObservationReadCut> {
        self.read_cut.source_cut.as_ref()
    }

    /// Hydrate only this request's retained published cut. Concurrent native
    /// publication may retire the live head, but cannot change this seed's
    /// dependencies. Inspection neither installs domains nor publishes a repair.
    pub(crate) fn checkpoint_with_read_domains(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let Some(cut) = self
            .read_domains
            .as_ref()
            .filter(|_| !self.exhausted && !self.journal_failure)
        else {
            return self.checkpoint_candidate();
        };
        let ReadDomainValues {
            projection_head,
            transcript,
            occupancy,
            trace,
            trace_total,
        } = cut
            .read(store)
            .map_err(RuntimeClientError::observation_store_error)?;
        let mut trace = trace.ok_or_else(|| RuntimeClientError::ObservationUnavailable {
            message: "complete checkpoint Trace head unavailable".into(),
        })?;
        let mut seed = self.clone();
        super::trace::repair_records(&mut trace.records, &seed.snapshot);
        stamp_trace_cut(&mut trace, &seed.read_cut);
        seed.source_frontiers.insert(
            "trace.records".into(),
            super::trace::checkpoint_frontier(
                trace_total,
                trace.records.len() as u64,
                trace.next_cursor.as_ref(),
            ),
        );
        seed.snapshot.transcript = transcript;
        seed.snapshot.context.occupancy = occupancy;
        seed.snapshot.context.projection_head = projection_head;
        seed.snapshot.trace = trace;
        seed.trace_head = super::checkpoint::PreparedTraceHead::default();
        seed.prepared = prepared_capture::PreparedCaptures::default();
        seed.checkpoint_candidate()
    }

    /// Finite capture at the native semantic cut. No Store work or awaiting is
    /// permitted here; every inline clone is independently byte/count capped.
    pub(crate) fn checkpoint_candidate(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let captured = self.capture_checkpoint()?;
        let fit_budget = captured.fit_budget;
        let mut checkpoint = captured.materialize();
        super::checkpoint::fit_checkpoint_with_budget(&mut checkpoint, fit_budget)?;
        Ok(checkpoint)
    }

    #[cfg(test)]
    fn checkpoint_prepared(
        &self,
    ) -> Result<
        (
            super::checkpoint::RuntimeClientCheckpoint,
            bool,
            Option<usize>,
        ),
        RuntimeClientError,
    > {
        let captured = self.capture_checkpoint()?;
        let evicted = captured.evicted;
        let budget = captured.fit_budget;
        Ok((captured.materialize(), evicted, budget))
    }

    #[allow(clippy::too_many_lines)] // Keep the shared admission order explicit.
    fn capture_checkpoint(
        &self,
    ) -> Result<
        prepared_capture::CapturedCheckpoint<impl FnOnce() -> RuntimeClientSnapshot + '_>,
        RuntimeClientError,
    > {
        use super::checkpoint::{Capture, ProjectionMetadata};
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if self.journal_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native Journal observation source unavailable".into(),
            });
        }
        let source = &self.snapshot;
        let mut capture = Capture::new(self.cursor.get());
        let mut budget = prepared_capture::FitBudget::new();
        macro_rules! cached {
            ($field:ident, $build:expr) => {{
                let (value, charge) =
                    self.prepared
                        .capture(stringify!($field), &mut capture, $build);
                budget.field(stringify!($field), charge);
                value
            }};
        }
        // Preserve the shared optional-body admission order, but borrow the
        // normalized bodies until a replacement or a reader actually needs them.
        let context = cached!(context, |capture| self.capture_context(capture));
        let attempt = self.capture_attempt(&mut capture);
        budget.value("attempt", &attempt);
        budget.value("settings_evidence", &source.settings_evidence);
        let effective_plugins = cached!(effective_plugins, |capture| capture
            .item("effective_plugins", &source.effective_plugins)
            .flatten());
        let workflows = cached!(workflows, |capture| {
            crate::runtime::workflow::read_model::WorkflowSnapshot {
                revision: source.workflows.revision,
                runs: capture.page("workflows.runs", &source.workflows.runs),
                omitted_runs: source.workflows.omitted_runs,
            }
        });
        budget.value("conversation_id", &source.conversation_id);
        budget.value("shutting_down", &source.shutting_down);
        budget.value("effective_approval_mode", &source.effective_approval_mode);
        budget.value(
            "configuration_adoption_eligibility",
            &source.configuration_adoption_eligibility,
        );
        let durability_failure = source.durability_failure.as_ref().map(|failure| {
            super::snapshot::RuntimeDurabilityFailure {
                operation: bounded_diagnostic(&failure.operation),
                diagnostic: bounded_diagnostic(&failure.diagnostic),
            }
        });
        budget.value("durability_failure", &durability_failure);
        let messages = cached!(messages, |capture| capture.message_page(
            "messages",
            &source.messages,
            &source.conversation_id
        ));
        let message_descriptors = cached!(message_descriptors, |capture| self
            .capture_message_descriptors(capture));
        budget.value("publication_settlements", &source.publication_settlements);
        let transcript = cached!(transcript, |capture| self.capture_transcript(capture));
        let mut trace_budget = prepared_capture::FitBudget::new();
        trace_budget.value("read_cut", &source.trace.read_cut);
        let (trace_records, charge) =
            self.prepared
                .capture("trace_records", &mut capture, |capture| {
                    self.trace_head.capture(&source.trace.records, capture)
                });
        trace_budget.field("records", charge);
        let (trace_page_updates, charge) =
            self.prepared
                .capture("trace_page_updates", &mut capture, |capture| {
                    capture.page("trace.updates", &source.trace.updates)
                });
        trace_budget.field("updates", charge);
        trace_budget.value("next_cursor", &source.trace.next_cursor);
        budget.field("trace", trace_budget.finish());
        let trace_updates = cached!(trace_updates, |capture| capture
            .page("trace_updates", &source.trace_updates));
        let inbound = cached!(inbound, |capture| super::snapshot::InboundDiagnostics {
            pending: capture.page("inbound.pending", &source.inbound.pending),
            last_drain: capture
                .item("inbound.last_drain", &source.inbound.last_drain)
                .flatten(),
        });
        let pending_interactions = cached!(pending_interactions, |capture| capture
            .interaction_page(&source.pending_interactions));
        let jobs = cached!(jobs, |capture| capture.page("jobs", &source.jobs));
        let agents = cached!(agents, |capture| capture.page("agents", &source.agents));
        let statuses = cached!(statuses, |capture| capture
            .page("statuses", &source.statuses));
        let capabilities = cached!(capabilities, |capture| self.capture_capabilities(capture));
        let resources = cached!(resources, |capture| {
            super::snapshot::RuntimeClientResourcesView {
                revision: source.resources.revision,
                inspection: capture
                    .item("resources.inspection", &source.resources.inspection)
                    .unwrap_or_default(),
                context_files: capture
                    .page("resources.context_files", &source.resources.context_files),
                agent_profile: source.resources.agent_profile,
            }
        });
        let model = cached!(model, |capture| capture
            .item("model", &source.model)
            .flatten());
        budget.value("model_status", &source.model_status);
        let todos = cached!(todos, |capture| source.todos.as_ref().map(|todos| {
            crate::tools::todo::TodoSnapshot {
                tasks: capture.page("todos.tasks", &todos.tasks),
                next_id: todos.next_id,
            }
        }));
        let goal = cached!(goal, |capture| capture.item("goal", &source.goal).flatten());
        self.capture_evidence(&messages, &transcript.content, &mut capture);
        let metadata = ProjectionMetadata {
            read_cut: self.checkpoint_read_cut(),
            content: capture.content,
            collections: capture.collections,
        };
        let mut complete_budget = prepared_capture::FitBudget::new();
        complete_budget.value("read_cut", &metadata.read_cut);
        complete_budget.value("content", &metadata.content);
        complete_budget.value("collections", &metadata.collections);
        let metadata_budget = complete_budget.finish();
        complete_budget.field("snapshot", budget.finish());
        complete_budget.value("cursor", &self.cursor);
        Ok(prepared_capture::CapturedCheckpoint {
            snapshot: move || {
                #[cfg(test)]
                super::checkpoint::record_capture_work(4);
                RuntimeClientSnapshot {
                    settings_evidence: source.settings_evidence,
                    effective_plugins: effective_plugins.into_owned(),
                    workflows: workflows.into_owned(),
                    conversation_id: source.conversation_id.clone(),
                    shutting_down: source.shutting_down,
                    effective_approval_mode: source.effective_approval_mode,
                    configuration_adoption_eligibility: source
                        .configuration_adoption_eligibility
                        .clone(),
                    durability_failure,
                    messages: messages.into_owned(),
                    message_descriptors: message_descriptors.into_owned(),
                    publication_settlements: source.publication_settlements.clone(),
                    transcript: transcript.into_owned(),
                    trace: super::trace::TracePage {
                        read_cut: source.trace.read_cut.clone(),
                        records: trace_records.into_owned(),
                        updates: trace_page_updates.into_owned(),
                        next_cursor: source.trace.next_cursor.clone(),
                    },
                    trace_updates: trace_updates.into_owned(),
                    attempt,
                    inbound: inbound.into_owned(),
                    pending_interactions: pending_interactions.into_owned(),
                    jobs: jobs.into_owned(),
                    agents: agents.into_owned(),
                    statuses: statuses.into_owned(),
                    context: context.into_owned(),
                    capabilities: capabilities.into_owned(),
                    resources: resources.into_owned(),
                    model: model.into_owned(),
                    model_status: source.model_status,
                    todos: todos.into_owned(),
                    goal: goal.into_owned(),
                }
            },
            metadata,
            cursor: self.cursor,
            evicted: capture.evicted,
            fit_budget: complete_budget.finish(),
            metadata_budget,
        })
    }

    #[cfg(test)]
    fn checkpoint_unfitted(
        &self,
    ) -> Result<(super::checkpoint::RuntimeClientCheckpoint, bool), RuntimeClientError> {
        use super::checkpoint::{Capture, RuntimeClientCheckpoint};
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if self.journal_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native Journal observation source unavailable".into(),
            });
        }
        let source = &self.snapshot;
        let mut capture = Capture::new(self.cursor.get());
        let context = self.capture_context(&mut capture);
        let attempt = self.capture_attempt(&mut capture);
        let snapshot = RuntimeClientSnapshot {
            settings_evidence: source.settings_evidence,
            effective_plugins: capture
                .item("effective_plugins", &source.effective_plugins)
                .flatten(),
            workflows: crate::runtime::workflow::read_model::WorkflowSnapshot {
                revision: source.workflows.revision,
                runs: capture.page("workflows.runs", &source.workflows.runs),
                omitted_runs: source.workflows.omitted_runs,
            },
            conversation_id: source.conversation_id.clone(),
            shutting_down: source.shutting_down,
            effective_approval_mode: source.effective_approval_mode,
            configuration_adoption_eligibility: source.configuration_adoption_eligibility.clone(),
            durability_failure: source.durability_failure.as_ref().map(|failure| {
                super::snapshot::RuntimeDurabilityFailure {
                    operation: bounded_diagnostic(&failure.operation),
                    diagnostic: bounded_diagnostic(&failure.diagnostic),
                }
            }),
            messages: capture.message_page("messages", &source.messages, &source.conversation_id),
            message_descriptors: self.capture_message_descriptors(&mut capture),
            publication_settlements: source.publication_settlements.clone(),
            transcript: self.capture_transcript(&mut capture),
            trace: super::trace::TracePage {
                read_cut: source.trace.read_cut.clone(),
                records: self.trace_head.capture(&source.trace.records, &mut capture),
                updates: capture.page("trace.updates", &source.trace.updates),
                next_cursor: source.trace.next_cursor.clone(),
            },
            trace_updates: capture.page("trace_updates", &source.trace_updates),
            attempt,
            inbound: super::snapshot::InboundDiagnostics {
                pending: capture.page("inbound.pending", &source.inbound.pending),
                last_drain: capture
                    .item("inbound.last_drain", &source.inbound.last_drain)
                    .flatten(),
            },
            pending_interactions: capture.interaction_page(&source.pending_interactions),
            jobs: capture.page("jobs", &source.jobs),
            agents: capture.page("agents", &source.agents),
            statuses: capture.page("statuses", &source.statuses),
            context,
            capabilities: self.capture_capabilities(&mut capture),
            resources: super::snapshot::RuntimeClientResourcesView {
                revision: source.resources.revision,
                inspection: capture
                    .item("resources.inspection", &source.resources.inspection)
                    .unwrap_or_default(),
                context_files: capture
                    .page("resources.context_files", &source.resources.context_files),
                agent_profile: source.resources.agent_profile,
            },
            model: capture.item("model", &source.model).flatten(),
            model_status: source.model_status,
            todos: source
                .todos
                .as_ref()
                .map(|todos| crate::tools::todo::TodoSnapshot {
                    tasks: capture.page("todos.tasks", &todos.tasks),
                    next_id: todos.next_id,
                }),
            goal: capture.item("goal", &source.goal).flatten(),
        };
        self.capture_evidence(
            &snapshot.messages,
            &snapshot.transcript.content,
            &mut capture,
        );
        Ok((
            RuntimeClientCheckpoint {
                snapshot,
                cursor: self.cursor,
                read_cut: self.checkpoint_read_cut(),
                content: capture.content,
                collections: capture.collections,
            },
            capture.evicted,
        ))
    }

    fn capture_capabilities(
        &self,
        capture: &mut super::checkpoint::Capture,
    ) -> super::snapshot::CapabilityView {
        self.capability_head.capture(capture)
    }

    fn capture_message_descriptors(
        &self,
        capture: &mut super::checkpoint::Capture,
    ) -> Vec<super::snapshot::RuntimeClientMessageDescriptor> {
        self.snapshot
            .message_descriptors
            .iter()
            .map(|source| {
                let mut descriptor = source.clone();
                while super::checkpoint::bounded_len(&descriptor, 16_384).is_none() {
                    let Some(removed) = descriptor.tool_calls.pop() else {
                        break;
                    };
                    capture.evicted = true;
                    let loaded = descriptor.tool_calls.len();
                    descriptor.content.retain(|binding| {
                        binding
                            .path
                            .first()
                            .is_none_or(|field| field != "tool_calls")
                            || binding
                                .path
                                .get(1)
                                .and_then(|value| value.parse::<usize>().ok())
                                .is_some_and(|index| index < loaded)
                    });
                    let domain = format!("messages.{}.tools", descriptor.locator.key());
                    if let Some(frontier) = descriptor
                        .collections
                        .iter_mut()
                        .find(|frontier| frontier.domain == domain)
                    {
                        frontier.loaded = loaded as u64;
                        frontier.next_offset = Some(u64::from(removed.block_index.get()));
                    }
                }
                descriptor
            })
            .collect()
    }

    fn capture_transcript(
        &self,
        capture: &mut super::checkpoint::Capture,
    ) -> super::snapshot::RuntimeClientTranscriptPage {
        let source = &self.snapshot;
        super::snapshot::RuntimeClientTranscriptPage {
            source_cut: source.transcript.source_cut.clone(),
            content: source.transcript.content.clone(),
            collections: source.transcript.collections.clone(),
            deferred_entries: capture.page(
                "transcript.deferred_entries",
                &source.transcript.deferred_entries,
            ),
            inherited_through: source.transcript.inherited_through,
            statistics: capture
                .item("transcript.statistics", &source.transcript.statistics)
                .flatten(),
            entries: capture.transcript_page(&source.transcript.entries, &source.conversation_id),
            next_cursor: source.transcript.next_cursor,
        }
    }

    fn capture_evidence(
        &self,
        messages: &[MessageBlock],
        transcript_content: &[super::checkpoint::ContentBinding],
        capture: &mut super::checkpoint::Capture,
    ) {
        for (index, message) in messages.iter().enumerate() {
            if let Some(bindings) = self.head_bindings.get(message.id()) {
                for binding in bindings {
                    let mut binding = binding.clone();
                    if binding.path.first().is_some_and(|field| field == "display") {
                        binding.path.remove(0);
                    }
                    let mut path = vec!["messages".to_owned(), index.to_string()];
                    path.append(&mut binding.path);
                    binding.path = path;
                    if !capture
                        .content
                        .iter()
                        .any(|existing| existing.path == binding.path)
                    {
                        capture.content.push(binding);
                    }
                }
            }
        }
        for binding in transcript_content {
            let mut binding = binding.clone();
            binding.path.insert(0, "transcript".into());
            if !capture
                .content
                .iter()
                .any(|existing| existing.path == binding.path)
            {
                capture.content.push(binding);
            }
        }
        for original in self.source_frontiers.values() {
            if let Some(frontier) = capture
                .collections
                .iter_mut()
                .find(|frontier| frontier.domain == original.domain)
            {
                frontier.total = original.total;
                frontier.next_offset = if frontier.loaded < original.loaded {
                    Some(0)
                } else {
                    original.next_offset
                };
            } else {
                capture.collections.push(original.clone());
            }
        }
        self.capture_collection_frontiers(capture);
    }

    fn capture_collection_frontiers(&self, capture: &mut super::checkpoint::Capture) {
        self.resolve_captured_content(&mut capture.content);
        for frontier in &mut capture.collections {
            frontier.revision = self.domain_revision(&frontier.domain);
        }
        for frontier in &mut capture.collections {
            if let Some(cut) = self.collection_cuts.get(&frontier.domain) {
                frontier.total = cut.total;
                frontier.next_offset =
                    (frontier.loaded < frontier.total).then_some(frontier.loaded);
            }
        }
        if let Some(frontier) = capture
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == "pending_interactions")
        {
            frontier.total = ["pending_primary", "pending_children"]
                .iter()
                .filter_map(|owner| self.collection_cuts.get(*owner))
                .map(|cut| cut.total)
                .sum();
            frontier.next_offset = (frontier.loaded < frontier.total).then(|| {
                self.snapshot
                    .pending_interactions
                    .last()
                    .map_or(0, |header| header.source_offset.saturating_add(1))
            });
        }
        for frontier in &mut capture.collections {
            if frontier.domain == "attempt.in_flight.blocks" {
                frontier.total = self.live_block_total;
                frontier.next_offset =
                    (frontier.loaded < frontier.total).then_some(frontier.loaded);
            }
            if frontier.domain == "attempt.foreground" {
                frontier.total = self
                    .canonical_foreground
                    .as_ref()
                    .map_or(self.live_tool_total, |cut| cut.total);
                frontier.next_offset =
                    (frontier.loaded < frontier.total).then_some(frontier.loaded);
            }
        }
        if let Some(cut) = &self.source_cut {
            for domain in &mut capture.collections {
                match domain.domain.as_str() {
                    "messages" => {
                        domain.total = cut.ledger_through;
                        domain.next_offset = (domain.total > domain.loaded).then_some(0);
                    }
                    "inbound.pending" => {
                        domain.total = cut.read.pending_total;
                        domain.next_offset = (domain.total > domain.loaded).then_some(0);
                    }
                    "transcript.entries" => {
                        domain.total = cut.transcript_total;
                        domain.next_offset = (domain.total > domain.loaded).then_some(0);
                    }
                    _ => {}
                }
            }
        }
    }

    fn capture_foreground(
        &self,
        capture: &mut super::checkpoint::Capture,
        foreground: &[ForegroundToolExecution],
    ) -> Vec<ForegroundToolExecution> {
        let head: Vec<_> = foreground
            .iter()
            .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
            .map(|tool| {
                let mut bounded = tool.clone();
                if !capture.reserve(&bounded) {
                    match &mut bounded.state {
                        ForegroundToolState::Assembled { arguments }
                        | ForegroundToolState::Running { arguments, .. }
                        | ForegroundToolState::Settled { arguments, .. } => arguments.clear(),
                    }
                    if let Some(source) = self
                        .foreground_content
                        .iter()
                        .find(|source| source.occurrence == tool.occurrence)
                    {
                        bounded.result_metadata_ref = Some(source.metadata.clone());
                        bounded.committed_result_deferred = bounded.committed_result.is_some();
                        if let Some(result) = &mut bounded.committed_result {
                            defer_result_content(result);
                        }
                        if let ForegroundToolState::Settled { result, .. } = &mut bounded.state {
                            defer_result_content(result);
                        }
                    }
                }
                bounded
            })
            .collect();
        capture.frontier(
            "attempt.foreground",
            foreground.len() as u64,
            head.len() as u64,
            (head.len() < foreground.len()).then_some(head.len() as u64),
        );
        self.capture_foreground_evidence(capture, &head);
        head
    }

    fn capture_foreground_evidence(
        &self,
        capture: &mut super::checkpoint::Capture,
        head: &[ForegroundToolExecution],
    ) {
        for (index, tool) in head.iter().enumerate() {
            let live_arguments = self.publication_start.as_ref().and_then(|start| {
                self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    tool.block_index,
                    super::content::ContentRepresentation::RawJsonUtf8.key(),
                    "live",
                ))
            });
            let canonical_arguments = self
                .content_metadata
                .get(&(
                    "canonical",
                    tool.occurrence.locator.key().into_owned(),
                    tool.block_index.get(),
                    super::content::ContentRepresentation::RawJsonUtf8.key(),
                ))
                .map(Arc::as_ref);
            if let Some(reference) = live_arguments.or(canonical_arguments) {
                let arguments = match &tool.state {
                    ForegroundToolState::Assembled { arguments }
                    | ForegroundToolState::Running { arguments, .. }
                    | ForegroundToolState::Settled { arguments, .. } => arguments,
                };
                capture.content.push(super::checkpoint::ContentBinding {
                    source_path: None,
                    path: vec![
                        "attempt".into(),
                        "foreground".into(),
                        index.to_string(),
                        "state".into(),
                        "arguments".into(),
                    ],
                    content_ref: reference.clone(),
                    deferred: arguments.len() as u64 != reference.readable_end,
                });
            }
            if let Some(source) = self
                .foreground_content
                .iter()
                .find(|source| source.occurrence == tool.occurrence)
            {
                capture.collections.push(source.frontier.clone());
                for binding in source
                    .content
                    .iter()
                    .filter(|binding| binding.path.get(1).is_some_and(|field| field == "result"))
                {
                    let mut binding = binding.clone();
                    let mut path = vec![
                        "attempt".into(),
                        "foreground".into(),
                        index.to_string(),
                        "committed_result".into(),
                    ];
                    path.extend(binding.path.into_iter().skip(2));
                    binding.path = path;
                    capture.content.push(binding);
                }
            }
            if let Some(reference) = &tool.result_metadata_ref {
                capture.content.push(super::checkpoint::ContentBinding {
                    source_path: Some(vec!["result".into()]),
                    path: vec![
                        "attempt".into(),
                        "foreground".into(),
                        index.to_string(),
                        "committed_result".into(),
                    ],
                    content_ref: reference.clone(),
                    deferred: tool.committed_result_deferred,
                });
            }
        }
    }

    fn capture_live_blocks(
        &self,
        capture: &mut super::checkpoint::Capture,
        blocks: &[InFlightBlock],
    ) -> Vec<InFlightBlock> {
        let mut result = Vec::new();
        for (index, block) in blocks
            .iter()
            .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
            .enumerate()
        {
            let (block_index, representation, field) = match block {
                InFlightBlock::Text { block_index, .. }
                | InFlightBlock::Reasoning { block_index, .. }
                | InFlightBlock::Refusal { block_index, .. } => (
                    *block_index,
                    super::content::ContentRepresentation::Utf8Text,
                    "text",
                ),
                InFlightBlock::ToolCall { block_index, .. } => (
                    *block_index,
                    super::content::ContentRepresentation::RawJsonUtf8,
                    "arguments",
                ),
            };
            let copied =
                super::checkpoint::bounded_clone(block, super::checkpoint::CHECKPOINT_ITEM_BYTES);
            let copied = copied.filter(|value| capture.reserve(value));
            let deferred = copied.is_none();
            let bounded = copied.unwrap_or_else(|| deferred_live_block(block));
            // Optional bytes share a budget; the first 32 structural descriptors
            // and their exact source references always survive its exhaustion.
            if let Some(start) = &self.publication_start {
                let reference = self
                    .live_frontiers
                    .get(&(
                        start.stream_id.clone(),
                        block_index,
                        representation.key(),
                        "live",
                    ))
                    .filter(|reference| field != "arguments" || reference.readable_end > 0)
                    .or_else(|| {
                        (field == "arguments")
                            .then(|| {
                                self.live_frontiers.get(&(
                                    start.stream_id.clone(),
                                    block_index,
                                    super::content::ContentRepresentation::CanonicalJsonV1.key(),
                                    "live",
                                ))
                            })
                            .flatten()
                    });
                if let Some(reference) = reference {
                    capture.content.push(super::checkpoint::ContentBinding {
                        source_path: None,
                        path: vec![
                            "attempt".into(),
                            "in_flight".into(),
                            "blocks".into(),
                            index.to_string(),
                            field.into(),
                        ],
                        content_ref: reference.clone(),
                        deferred: deferred
                            || reference.readable_end > inline_block_bytes(&bounded, field),
                    });
                }
                if let Some(reference) = self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    block_index,
                    super::content::ContentRepresentation::CanonicalJsonV1.key(),
                    "live_block_metadata",
                )) {
                    capture.content.push(super::checkpoint::ContentBinding {
                        source_path: Some(vec!["name".into()]),
                        path: vec![
                            "attempt".into(),
                            "in_flight".into(),
                            "blocks".into(),
                            index.to_string(),
                        ],
                        content_ref: reference.clone(),
                        deferred: true,
                    });
                }
            }
            result.push(bounded);
        }
        capture.frontier(
            "attempt.in_flight.blocks",
            blocks.len() as u64,
            result.len() as u64,
            (result.len() < blocks.len()).then_some(result.len() as u64),
        );
        result
    }

    fn resolve_captured_content(&self, bindings: &mut [super::checkpoint::ContentBinding]) {
        for binding in bindings {
            if let super::content::ContentSource::CanonicalMessageBlock {
                component,
                locator,
                block_index,
            } = &binding.content_ref.source
            {
                let raw = self
                    .content_metadata
                    .get(&(
                        component.key(),
                        locator.key().into_owned(),
                        block_index.get(),
                        super::content::ContentRepresentation::RawJsonUtf8.key(),
                    ))
                    .map(Arc::as_ref);
                let native = self
                    .content_metadata
                    .get(&(
                        component.key(),
                        locator.key().into_owned(),
                        block_index.get(),
                        binding.content_ref.representation.key(),
                    ))
                    .map(Arc::as_ref);
                let reference = if binding
                    .path
                    .last()
                    .is_some_and(|field| field == "arguments")
                {
                    raw.or(native)
                } else {
                    native
                };
                if let Some(reference) = reference {
                    binding.content_ref.clone_from(reference);
                }
            }
        }
    }

    pub(crate) fn checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        use super::checkpoint::page;
        if cut.conversation_id != self.snapshot.conversation_id
            || cut.activation_id != self.read_cut.activation_id
            || cut
                .domain_revisions
                .get(domain.split('.').next().unwrap_or(domain))
                .copied()
                .unwrap_or(0)
                != self.domain_revision(domain)
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        let source = &self.snapshot;
        macro_rules! list {
            ($items:expr,$identity:expr) => {
                page(
                    domain,
                    cut.clone(),
                    RuntimeClientCursor::new(cut.revision),
                    $items,
                    offset,
                    limit,
                    $identity,
                )
            };
        }
        match domain {
            "model" => list!(
                &source.model.iter().collect::<Vec<_>>(),
                |_item: &&crate::model::session::SessionModelView| "model".into()
            ),
            "context" => list!(
                std::slice::from_ref(&source.context),
                |_item: &super::snapshot::RuntimeClientContextView| "context".into()
            ),
            "capabilities" => list!(
                std::slice::from_ref(&source.capabilities),
                |_item: &super::snapshot::CapabilityView| "capabilities".into()
            ),
            "goal" => list!(
                &source.goal.iter().collect::<Vec<_>>(),
                |_item: &&crate::runtime_client::goal::RuntimeClientGoalView| "goal".into()
            ),
            "todos.tasks" => list!(
                source
                    .todos
                    .as_ref()
                    .map_or(&[][..], |todos| todos.tasks.as_slice()),
                |item: &crate::tools::todo::TodoTask| item.id.to_string()
            ),
            "messages" => self.checkpoint_messages_page(cut, domain, offset, limit),
            "transcript.entries" => list!(
                &source.transcript.entries,
                |entry: &super::snapshot::RuntimeClientTranscriptEntry| entry
                    .cursor
                    .get()
                    .to_string()
            ),
            "pending_interactions" => self.checkpoint_pending_page(cut, domain, offset, limit),
            "agents" => list!(
                &source.agents,
                |agent: &super::snapshot::RuntimeClientAgent| agent.agent_id.to_string()
            ),
            "jobs" => list!(&source.jobs, |job: &super::snapshot::RuntimeClientJob| job
                .job_id
                .to_string()),
            "statuses" => list!(
                &source.statuses,
                |status: &super::snapshot::AgentStatusView| status
                    .source_locator
                    .as_ref()
                    .map(|locator| locator.key().into_owned())
                    .or_else(|| status.status_message_id.as_ref().map(ToString::to_string))
                    .unwrap_or_default()
            ),
            "inbound.pending" => list!(
                &source.inbound.pending,
                |item: &super::snapshot::InboundItemView| item.message.id.to_string()
            ),
            "attempt.in_flight.blocks" | "attempt.foreground" => {
                self.checkpoint_attempt_page(cut, domain, offset, limit)
            }
            _ => self.checkpoint_auxiliary_page(cut, domain, offset, limit),
        }
    }
    fn checkpoint_attempt_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        macro_rules! list {
            ($items:expr, $identity:expr) => {
                super::checkpoint::page(
                    domain,
                    cut.clone(),
                    RuntimeClientCursor::new(cut.revision),
                    $items,
                    offset,
                    limit,
                    $identity,
                )
            };
        }
        match domain {
            "attempt.in_flight.blocks" => list!(
                source
                    .attempt
                    .as_ref()
                    .and_then(|attempt| attempt.in_flight.as_ref())
                    .map_or(&[][..], |flight| &flight.blocks[..]),
                |block: &InFlightBlock| match block {
                    InFlightBlock::Text { block_index, .. }
                    | InFlightBlock::Reasoning { block_index, .. }
                    | InFlightBlock::Refusal { block_index, .. }
                    | InFlightBlock::ToolCall { block_index, .. } => block_index.to_string(),
                }
            ),
            "attempt.foreground" => list!(
                source
                    .attempt
                    .as_ref()
                    .map_or(&[][..], |attempt| &attempt.foreground[..]),
                |tool: &ForegroundToolExecution| format!(
                    "{}:{}",
                    tool.occurrence.locator.key(),
                    tool.block_index
                )
            ),
            _ => unreachable!("native attempt domain"),
        }
    }

    fn checkpoint_messages_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        let mut result = super::checkpoint::page(
            domain,
            cut.clone(),
            RuntimeClientCursor::new(cut.revision),
            &source.messages,
            offset,
            limit,
            |message: &MessageBlock| message.id().to_string(),
        )?;
        for item in &mut result.items {
            if item.deferred {
                let mut capture = super::checkpoint::Capture::new(self.cursor.get());
                let message = capture.message(
                    &source.messages[usize::try_from(item.offset).expect("validated page offset")],
                    &source.conversation_id,
                    &[],
                );
                item.value = super::checkpoint::bounded_bytes(
                    &message,
                    super::checkpoint::CHECKPOINT_ITEM_BYTES,
                )
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                item.content = capture.content;
            }
        }
        Ok(result)
    }

    fn checkpoint_pending_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        let mut result = super::checkpoint::page(
            domain,
            cut.clone(),
            RuntimeClientCursor::new(cut.revision),
            &source.pending_interactions,
            offset,
            limit,
            |interaction: &super::snapshot::RuntimeClientInteractionHeader| {
                interaction.interaction.to_string()
            },
        )?;
        for item in &mut result.items {
            let header = &source.pending_interactions
                [usize::try_from(item.offset).expect("validated interaction offset")];
            item.body_complete = Some(header.body_complete);
            item.deferred = !header.body_complete;
            if header.kind == super::snapshot::RuntimeClientInteractionKind::Approval {
                result
                    .collections
                    .push(super::checkpoint::CollectionFrontier {
                        domain: format!(
                            "pending_interactions.{}.request.kind.arguments",
                            item.offset
                        ),
                        revision: self.domain_revision("pending_interactions"),
                        total: 1,
                        loaded: 0,
                        next_offset: Some(0),
                    });
            }
        }
        Ok(result)
    }

    fn checkpoint_auxiliary_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        use super::checkpoint::page;
        let source = &self.snapshot;
        macro_rules! list {
            ($items:expr,$identity:expr) => {
                page(
                    domain,
                    cut.clone(),
                    RuntimeClientCursor::new(cut.revision),
                    $items,
                    offset,
                    limit,
                    $identity,
                )
            };
        }
        match domain {
            "capabilities.tools" => list!(
                &source.capabilities.tools,
                |tool: &super::snapshot::RuntimeClientTool| tool.id.to_string()
            ),
            "capabilities.configured_tools" => list!(
                &source.capabilities.configured_tools,
                |tool: &super::snapshot::RuntimeClientTool| tool.id.to_string()
            ),
            "capabilities.available_tools" => list!(
                &source.capabilities.available_tools,
                |tool: &super::snapshot::RuntimeClientTool| tool.id.to_string()
            ),
            "capabilities.skills" => list!(
                &source.capabilities.skills,
                |skill: &super::snapshot::RuntimeClientSkill| skill.id.to_string()
            ),
            "capabilities.sources" => list!(
                &source.capabilities.sources,
                |_: &super::snapshot::CapabilitySourceView| String::from("source")
            ),
            "resources.context_files" => list!(
                &source.resources.context_files,
                |file: &super::snapshot::RuntimeClientContextFile| bounded_diagnostic(&file.path)
            ),
            "workflows.runs" => list!(
                &source.workflows.runs,
                |run: &crate::runtime::workflow::read_model::WorkflowRunView| format!(
                    "{}:{}:{}",
                    run.id.conversation_id, run.id.attempt_id, run.id.invocation
                )
            ),
            _ => Err(RuntimeClientError::InvalidRequest {
                message: "unknown checkpoint collection domain".into(),
            }),
        }
    }
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
    pub(crate) projection_head: Option<super::snapshot::RuntimeClientProjectionHeadView>,
    pub(crate) transcript: super::snapshot::RuntimeClientTranscriptPage,
    pub(crate) occupancy: Option<super::occupancy::RuntimeClientContextOccupancy>,
    pub(crate) trace: Option<super::trace::TracePage>,
    pub(crate) trace_total: u64,
}

type ReadDomains = Result<ReadDomainValues, crate::durable::ConversationStoreError>;

fn projection_head_view(
    store: &dyn crate::durable::ConversationStore,
    through: u64,
) -> Result<
    Option<super::snapshot::RuntimeClientProjectionHeadView>,
    crate::durable::ConversationStoreError,
> {
    store.observation_projection_head(through)
}

pub(crate) fn checkpoint_view(
    checkpoint: crate::durable::projection::ProjectionCheckpoint,
) -> super::snapshot::RuntimeClientProjectionHeadView {
    let original_items = checkpoint
        .candidate
        .items
        .iter()
        .filter(|item| {
            matches!(
                item.message,
                crate::context::checkpoint::CheckpointMessage::Original(_)
            )
        })
        .count();
    let original_items = u32::try_from(original_items).expect("bounded checkpoint item count");
    let summary_preview = checkpoint
        .candidate
        .items
        .iter()
        .find_map(|item| match &item.message {
            crate::context::checkpoint::CheckpointMessage::Derived(derived)
                if derived
                    .transformations
                    .contains(&crate::model::projected::SemanticTransformation::Summary) =>
            {
                match &derived.message {
                    crate::model::projected::ProjectedMessage::User { content, .. } => {
                        content.iter().find_map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) => {
                                Some(super::trace::TraceText::detail(&text.text))
                            }
                            _ => None,
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        });
    super::snapshot::RuntimeClientProjectionHeadView {
        checkpoint_id: checkpoint.candidate.checkpoint_id,
        activation_id: checkpoint.activation.activation,
        base_frontier: checkpoint.candidate.base_frontier,
        activation_ledger: checkpoint.activation.ledger,
        origin: checkpoint.activation.origin,
        destination_installation: checkpoint.activation.destination_installation,
        original_items,
        derived_items: u32::try_from(checkpoint.candidate.items.len())
            .expect("bounded checkpoint item count")
            - original_items,
        summary_preview,
    }
}

impl ReadDomainCut {
    pub(crate) fn read(&self, store: &dyn crate::durable::ConversationStore) -> ReadDomains {
        self.read_with_trace(store, true)
    }

    pub(crate) fn read_live(&self, store: &dyn crate::durable::ConversationStore) -> ReadDomains {
        self.read_with_trace(store, false)
    }

    fn read_with_trace(
        &self,
        store: &dyn crate::durable::ConversationStore,
        materialize_trace: bool,
    ) -> ReadDomains {
        if !materialize_trace {
            let cut = self.source_cut.as_ref().ok_or_else(|| {
                crate::durable::ConversationStoreError::InvalidReference(
                    "native read-domain source cut unavailable".into(),
                )
            })?;
            let head = store.observation_checkpoint_head(
                cut,
                self.through,
                super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS,
            )?;
            return Ok(ReadDomainValues {
                projection_head: head.projection_head,
                transcript: head.transcript,
                occupancy: head.occupancy,
                trace: None,
                trace_total: head.trace_total,
            });
        }
        let mut page = if let Some(cut) = &self.source_cut {
            super::snapshot::transcript_header_view(store.transcript_header_page(
                cut,
                None,
                super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS,
            )?)
        } else {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "native read-domain source cut unavailable".into(),
            ));
        };
        super::response::decorate_through(store, &mut page, self.through)?;
        let occupancy = super::occupancy::read(store, self.through)?;
        let trace_cut = self
            .source_cut
            .as_ref()
            .ok_or(crate::durable::ConversationStoreError::ObservationCutExpired)?;
        let trace = if materialize_trace {
            let mut trace = super::trace::TraceProjection::at_cut(store, trace_cut)?
                .page(None, super::trace::TRACE_PAGE_LIMIT)?;
            trace.read_cut = None;
            Some(trace)
        } else {
            None
        };
        let trace_total = store.observation_trace_total(self.through)?;
        Ok(ReadDomainValues {
            projection_head: projection_head_view(store, self.through)?,
            transcript: page,
            occupancy,
            trace,
            trace_total,
        })
    }
}

/// Request-owned copy, independent of the live materializer's installation fence.
#[cfg(test)]
pub(crate) struct SnapshotCandidate {
    pub(crate) snapshot: RuntimeClientSnapshot,
    pub(crate) cursor: RuntimeClientCursor,
    pub(crate) trace_through: u64,
    pub(crate) read_cut: ReadDomainCut,
}
#[cfg(test)]
impl SnapshotCandidate {
    pub(crate) fn read_domains(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> ReadDomains {
        self.read_cut.read(store)
    }

    pub(crate) fn materialize(
        mut self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Result<Self, RuntimeClientError> {
        let domains = self
            .read_domains(store)
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })?;
        self.snapshot.transcript = domains.transcript;
        self.snapshot.context.occupancy = domains.occupancy;
        self.snapshot.context.projection_head = domains.projection_head;
        self.snapshot.trace =
            domains
                .trace
                .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                    message: "complete snapshot Trace head unavailable".into(),
                })?;
        Ok(self)
    }
}

fn event_kind(event: &RuntimeClientEvent) -> &'static str {
    match event {
        RuntimeClientEvent::MessageCommitted { .. } => "message_committed",
        RuntimeClientEvent::MessageDescriptorCommitted { .. } => "message_descriptor_committed",
        RuntimeClientEvent::AssistantPublicationSettled { .. } => "assistant_publication_settled",
        RuntimeClientEvent::AssistantPublicationDeferredSettled { .. } => {
            "assistant_publication_deferred_settled"
        }
        RuntimeClientEvent::ToolCallAssembled { .. } => "tool_call_assembled",
        RuntimeClientEvent::ToolOccurrenceRebound { .. } => "tool_occurrence_rebound",
        RuntimeClientEvent::PendingHeadChanged { .. } => "pending_head_changed",
        RuntimeClientEvent::InteractionPending { .. } => "interaction_pending",
        RuntimeClientEvent::InteractionSettled { .. } => "interaction_settled",
        RuntimeClientEvent::AttemptSettled { .. } => "attempt_settled",
        RuntimeClientEvent::ReadDomainsUpdated { .. } => "read_domains_updated",
        RuntimeClientEvent::ProjectionTransition { .. } => "projection_transition",
        RuntimeClientEvent::PendingInboundChanged { .. } => "pending_inbound_changed",
        RuntimeClientEvent::InboundEnqueued { .. } => "inbound_enqueued",
        RuntimeClientEvent::AgentUpdated { .. } => "agent_updated",
        RuntimeClientEvent::JobUpdated { .. } => "job_updated",
        RuntimeClientEvent::CollectionChanged { .. } => "collection_changed",
        RuntimeClientEvent::CapabilityUpdated { .. } => "capability_updated",
        RuntimeClientEvent::ResourceGenerationUpdated { .. } => "resource_generation_updated",
        RuntimeClientEvent::SessionModelChanged { .. } => "session_model_changed",
        _ => "projection_updated",
    }
}

fn defer_result_content(result: &mut ToolExecutionResult) {
    result.content.clear();
    result.artifacts.clear();
    result.deliveries.clear();
    result.workflow = None;
}

fn inline_label(value: &str) -> String {
    let mut end = value.len().min(256);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn inline_prefix(value: &str) -> String {
    let mut end = value.len().min(super::checkpoint::CHECKPOINT_ITEM_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn append_inline_prefix(prefix: &mut String, suffix: &str) {
    if suffix.len() <= super::checkpoint::CHECKPOINT_ITEM_BYTES.saturating_sub(prefix.len()) {
        prefix.push_str(suffix);
    }
}

fn bounded_diagnostic(value: &str) -> String {
    let mut end = value.len().min(1024);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn bounded_outcome(outcome: &RuntimeClientOutcome) -> RuntimeClientOutcome {
    use crate::runtime::types::RuntimeError as Error;
    match outcome {
        RuntimeClientOutcome::Failed {
            error:
                RuntimeClientAttemptFailure::Model {
                    kind,
                    message,
                    retry_after_ms,
                },
            error_deferred,
            error_source,
        } => RuntimeClientOutcome::Failed {
            error_deferred: *error_deferred,
            error_source: error_source.clone(),
            error: RuntimeClientAttemptFailure::Model {
                kind: kind.clone(),
                message: bounded_diagnostic(message),
                retry_after_ms: *retry_after_ms,
            },
        },
        RuntimeClientOutcome::Failed {
            error: RuntimeClientAttemptFailure::Runtime { error },
            error_deferred,
            error_source,
        } => RuntimeClientOutcome::Failed {
            error_deferred: *error_deferred,
            error_source: error_source.clone(),
            error: RuntimeClientAttemptFailure::Runtime {
                error: match error {
                    Error::Internal { message } => Error::Internal {
                        message: bounded_diagnostic(message),
                    },
                    Error::InvalidState { message } => Error::InvalidState {
                        message: bounded_diagnostic(message),
                    },
                    Error::Unsupported { message } => Error::Unsupported {
                        message: bounded_diagnostic(message),
                    },
                    Error::UnknownTool { name } => Error::UnknownTool {
                        name: bounded_diagnostic(name),
                    },
                    Error::DurableStore { message } => Error::DurableStore {
                        message: bounded_diagnostic(message),
                    },
                    Error::ContractViolation { message } => Error::ContractViolation {
                        message: bounded_diagnostic(message),
                    },
                    Error::ContextPreparationFailed { message } => {
                        Error::ContextPreparationFailed {
                            message: bounded_diagnostic(message),
                        }
                    }
                    Error::ContextCompactionFailed { message } => Error::ContextCompactionFailed {
                        message: bounded_diagnostic(message),
                    },
                    Error::PreStepRejected { reason } => Error::PreStepRejected {
                        reason: bounded_diagnostic(reason),
                    },
                    Error::PreStepPolicyFailed { message } => Error::PreStepPolicyFailed {
                        message: bounded_diagnostic(message),
                    },
                    Error::ToolResultObservationFailed { message } => {
                        Error::ToolResultObservationFailed {
                            message: bounded_diagnostic(message),
                        }
                    }
                    Error::RestartInterrupted { message } => Error::RestartInterrupted {
                        message: bounded_diagnostic(message),
                    },
                    Error::DeferredContextRejected { message } => Error::DeferredContextRejected {
                        message: bounded_diagnostic(message),
                    },
                },
            },
        },
        RuntimeClientOutcome::Completed {
            finish_reason: crate::model::ModelFinishReason::Other { reason },
            finish_reason_deferred,
            finish_reason_source,
        } => RuntimeClientOutcome::Completed {
            finish_reason: crate::model::ModelFinishReason::Other {
                reason: bounded_diagnostic(reason),
            },
            finish_reason_deferred: *finish_reason_deferred || reason.len() > 1024,
            finish_reason_source: finish_reason_source.clone(),
        },
        other => other.clone(),
    }
}

/// Closed observation classification. Trace invalidation belongs separately to
/// `JournalBatch`, whose source vocabulary is owned by `PendingObservations`.
fn invalidates_read_domains(observation: &ConversationObservation) -> bool {
    match observation {
        ConversationObservation::JournalFact(_) => true,
        ConversationObservation::JournalSourceReady
        | ConversationObservation::TranscriptSourceReady(_) => false,
        ConversationObservation::CanonicalCommitted { .. }
        | ConversationObservation::AuditSettled { .. }
        | ConversationObservation::PendingReceipt(_) => true,
        #[cfg(test)]
        ConversationObservation::InboundEnqueued(_)
        | ConversationObservation::InboundDrained(_)
        | ConversationObservation::PendingInboundChanged(_) => true,
        #[cfg(test)]
        ConversationObservation::Committed { .. } => true,
        ConversationObservation::InteractionPending { audit, .. }
        | ConversationObservation::InteractionSettled { audit, .. } => audit.is_some(),
        #[cfg(test)]
        ConversationObservation::Event { event, .. } => invalidates_event(event),
        #[cfg(test)]
        ConversationObservation::ManualCompactionEvent { event, .. } => invalidates_event(event),
        ConversationObservation::ObservationBatch(_)
        | ConversationObservation::JournalBatch { .. }
        | ConversationObservation::CollectionProof { .. }
        | ConversationObservation::ToolOccurrenceProof { .. }
        | ConversationObservation::Published { .. } => {
            unreachable!("publication envelopes are classified through their contents")
        }
        #[cfg(test)]
        ConversationObservation::InteractionRemoved { .. } => false,
        ConversationObservation::PendingHead { .. }
        | ConversationObservation::Workflow(_)
        | ConversationObservation::NativeToolResultCommit(_)
        | ConversationObservation::GoalChanged(_)
        | ConversationObservation::Status(_)
        | ConversationObservation::PublicationOpened { .. }
        | ConversationObservation::Publication { .. }
        | ConversationObservation::Background(_)
        | ConversationObservation::ToolProgress { .. }
        | ConversationObservation::SubagentLifecycle { .. }
        | ConversationObservation::SubagentActivity(_)
        | ConversationObservation::Capability { .. }
        | ConversationObservation::Resources { .. }
        | ConversationObservation::AttemptAdmitted { .. }
        | ConversationObservation::SessionModelChanged { .. }
        | ConversationObservation::Shutdown
        | ConversationObservation::DurableFailure { .. }
        | ConversationObservation::DurabilityFailed { .. }
        | ConversationObservation::AdoptionEligibility(_) => false,
    }
}

#[cfg(test)]
fn invalidates_event(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptStarted { .. }
            | RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
            | RuntimeEvent::TurnStarted
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
            | RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
    )
}

fn initial_client_snapshot(
    conversation_id: ConversationId,
    initial_messages: Vec<MessageBlock>,
    initial_capabilities: CapabilityView,
    initial_model: Option<SessionModelView>,
) -> RuntimeClientSnapshot {
    RuntimeClientSnapshot {
        settings_evidence: super::settings::SettingsEvidence::LiveSession,
        // Absent until the host installs the attached runtime's own
        // frozen composition. A projection with no runtime behind it
        // — durable historical inspection — keeps it absent rather
        // than reconstructing one.
        effective_plugins: None,
        workflows: crate::runtime::workflow::read_model::WorkflowSnapshot::default(),
        conversation_id,
        shutting_down: false,
        effective_approval_mode: ApprovalMode::Policy,
        // A host binds before activation, and an inspection projection
        // has no live runtime: neither can adopt until the runtime
        // publishes otherwise.
        configuration_adoption_eligibility:
            crate::local_runtime::configuration::application::AdoptionEligibility::Unavailable,
        durability_failure: None,
        messages: initial_messages,
        message_descriptors: Vec::new(),
        publication_settlements: Vec::new(),
        transcript: super::snapshot::RuntimeClientTranscriptPage::default(),
        trace: super::trace::TracePage::default(),
        trace_updates: Vec::new(),
        attempt: None,
        inbound: InboundDiagnostics {
            pending: Vec::new(),
            last_drain: None,
        },
        pending_interactions: Vec::new(),
        jobs: Vec::new(),
        agents: Vec::new(),
        statuses: Vec::new(),
        context: RuntimeClientContextView::default(),
        capabilities: initial_capabilities,
        resources: super::snapshot::RuntimeClientResourcesView::default(),
        model: initial_model,
        model_status: None,
        todos: None,
        goal: None,
    }
}

impl RuntimeClientProjection {
    /// Creates the projection over one conversation with the current Surface
    /// working set and the initial capability view.
    pub(crate) fn new(
        conversation_id: ConversationId,
        initial_messages: Vec<MessageBlock>,
        initial_capabilities: CapabilityView,
        initial_model: Option<SessionModelView>,
        replay_limit: usize,
    ) -> Self {
        Self {
            #[cfg(test)]
            encoding_profile: None,
            activation_id: None,
            staged_publication: None,
            published_seed: None,
            published_sources: None,
            seen_activations: std::collections::HashSet::new(),
            journal_through: 0,
            native_journal_highwater: 0,
            native_canonical_highwater: 0,
            canonical_observed_through: 0,
            publication_state: NativePublicationState::default(),
            read_through: 0,
            source_cut: None,
            head_bindings: std::collections::BTreeMap::new(),
            foreground_content: Vec::new(),
            prepared_messages: std::collections::BTreeMap::new(),
            bootstrap_transcript_through: 0,
            #[cfg(test)]
            pending_transcript: std::collections::BTreeMap::new(),
            publication_start: None,
            content_metadata: ContentMetadata::new(),
            prepared_publication_content: Vec::new(),
            batch_events: None,
            transcript_prefix: 0,
            #[cfg(test)]
            prepared_terminals: std::collections::BTreeMap::new(),
            #[cfg(test)]
            prepared_pending: None,
            deferred_events: (Vec::new(), 0, false),
            canonical_foreground: None,
            native_tool_occurrences: std::collections::BTreeMap::new(),
            live_revision: 0,
            live_block_total: 0,
            live_tool_total: 0,
            live_block_ordinal: None,
            live_tool_ordinal: None,
            pending_epoch_wait: None,
            pending_mutation_revision: 0,
            source_state: ObservationSourceState::default(),
            prepared_outcomes: std::collections::BTreeMap::new(),
            prepared_journal: std::collections::BTreeMap::new(),
            live_frontiers: std::collections::BTreeMap::new(),
            read_domains_dirty: false,
            read_domains_requested: false,
            read_failure: None,
            read_revision: 0,
            domain_revisions: std::collections::BTreeMap::new(),
            domain_sources: DomainSources::new(),
            domain_source_bytes: 0,
            source_frontiers: std::collections::BTreeMap::default(),
            collection_cuts: std::collections::BTreeMap::new(),
            pending_heads: std::collections::BTreeMap::new(),
            collection_ordinal: None,
            tool_occurrence: None,
            cursor: RuntimeClientCursor::new(0),
            exhausted: false,
            trace_head: super::checkpoint::PreparedTraceHead::default(),
            capability_head: super::capability_source::PreparedCapabilityHead::new(
                &initial_capabilities,
                Vec::new(),
            ),
            snapshot: initial_client_snapshot(
                conversation_id,
                initial_messages,
                initial_capabilities,
                initial_model,
            ),
            replay: VecDeque::new(),
            replay_cut_cursors: HashMap::new(),
            initial_read_cut: None,
            replay_sizes: VecDeque::new(),
            replay_bytes: 0,
            replay_limit,
            subscribers: Vec::new(),
            next_subscriber_id: 1,
            #[cfg(test)]
            probe: None,
        }
    }

    /// Installs the test-only linearization hooks. Only available under
    /// `#[cfg(test)]`; never used by production code.
    #[cfg(test)]
    pub(crate) fn install_probe(
        &mut self,
        probe: crate::runtime_client::test_sync::ProjectionProbe,
    ) {
        self.probe = Some(probe);
    }

    /// Seeds the projection from the runtime-owned bootstrap snapshot.
    ///
    /// The conversation runtime captured every fact of this snapshot at
    /// one global cut `R` while it was still inactive (see
    /// `ConversationRuntime::install_observation_bridge`); this method
    /// owns the translation of the semantic source types into the client
    /// snapshot read model.
    ///
    /// **Nothing here publishes and nothing here allocates a cursor.**
    /// The seed *is* the state at cursor 0 — current Surface, session
    /// model, the startup capability snapshot, and pending inbound — all
    /// installed as snapshot state rather than replayed through
    /// [`RuntimeClientProjection::apply`], so state that existed before
    /// the bootstrap cut can never fabricate a live event. The background
    /// seed is provably empty by the ownership-transfer invariant (a
    /// `ConversationRuntime` is constructed only over a pristine
    /// tool-runtime background plane, and the transfer then refuses
    /// dispatch commits while its mailbox is bound inactive) and is still
    /// installed from the same seed for one coherent cut. Every
    /// transition after `R` arrives through the live observation stream
    /// and gets the first real cursor.
    pub(crate) fn set_settings_evidence(&mut self, evidence: super::settings::SettingsEvidence) {
        self.snapshot.settings_evidence = evidence;
    }

    /// Installs the frozen effective native Agent Extension composition of
    /// the attached Agent runtime (Issue #256).
    ///
    /// The host calls this once, at construction, from
    /// [`ConversationRuntime::native_extensions`](crate::runtime::ConversationRuntime::native_extensions).
    /// There is no later mutation seam: the projected composition changes
    /// only when the runtime behind the projection is a different one.
    pub(crate) fn set_effective_plugins(&mut self, extensions: super::settings::EffectivePlugins) {
        self.snapshot.effective_plugins = Some(extensions);
    }

    fn bootstrap_agents(
        &mut self,
        agents: &[(
            crate::runtime::subagent::AgentSnapshot,
            crate::runtime::subagent::SubagentSnapshot,
        )],
    ) {
        self.snapshot.agents.clear();
        self.seen_activations.clear();
        for (agent, activation) in agents {
            self.seen_activations
                .insert(agent.latest_activation.clone());
            self.snapshot.agents.push(agent_view(agent, activation));
        }
    }

    pub(crate) fn bootstrap(
        &mut self,
        seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    ) {
        self.source_cut.clone_from(&seed.observation_cut);
        self.pending_mutation_revision = seed
            .observation_cut
            .as_ref()
            .map_or(0, |cut| cut.read.mutation_revision);
        self.source_state.source_unavailable = seed.source_failure.is_some();
        self.source_state.source_layout_failure = seed.source_failure.is_some();
        self.read_failure.clone_from(&seed.source_failure);
        self.journal_through = seed.journal_through;
        self.native_journal_highwater = seed.journal_through;
        self.read_through = seed.journal_through;
        self.snapshot.transcript = super::snapshot::transcript_page_view(seed.transcript.clone())
            .expect("runtime bootstrap transcript is valid");
        self.bootstrap_transcript_through =
            seed.transcript.entries.last().map_or(0, |e| e.cursor.get());
        #[cfg(test)]
        {
            self.pending_transcript = seed
                .inbound_pending
                .iter()
                .filter_map(|item| {
                    item.transcript_cursor()
                        .map(|cursor| (item.message().id.clone(), cursor))
                })
                .collect();
        }
        self.snapshot.shutting_down = seed.shutting_down;
        self.collection_cuts
            .insert("jobs".into(), seed.background_cut);
        self.collection_cuts
            .insert("agents".into(), seed.agents_cut);
        self.snapshot.effective_approval_mode = seed.approval_mode;
        self.snapshot.inbound.pending =
            seed.inbound_pending.iter().map(inbound_item_view).collect();
        self.collection_cuts
            .insert("pending_primary".into(), seed.pending_primary_cut);
        self.collection_cuts
            .insert("pending_children".into(), seed.pending_children_cut);
        for owner in ["pending_primary", "pending_children"] {
            let head: Vec<_> = seed
                .pending_interactions
                .iter()
                .filter(|header| {
                    (owner == "pending_primary")
                        == matches!(
                            header.source,
                            crate::runtime::interaction::InteractionSource::Primary
                        )
                })
                .take(32)
                .cloned()
                .collect();
            self.pending_heads.insert(owner.into(), head);
        }
        self.rebuild_pending_head();
        for existing in &seed.background {
            upsert_background(&mut self.snapshot.jobs, background_view(existing));
        }
        self.bootstrap_agents(&seed.agents);
        // Resource header is installed by the off-lock bootstrap preparer.
        // The bootstrap cut is the *only* place the Todo composition fact
        // enters this projection: `None` here means the attached runtime
        // composes no Todo extension, and the live fold below deliberately
        // cannot turn that back into a list.
        self.snapshot.todos.clone_from(&seed.todos);
        self.snapshot.goal.clone_from(&seed.goal);
        self.snapshot.model_status = Some(seed.model.status());
        // An inactive runtime has never admitted an attempt, composed an
        // Agent Status, or compacted, so `attempt`, `statuses`, and
        // `context` keep their empty initial values by construction.
    }

    /// Installs constructor-prepared finite historical readings without a live cursor.
    pub(crate) fn bootstrap_durable_reading(
        &mut self,
        attempt: Option<RuntimeClientAttempt>,
        context: super::snapshot::RuntimeClientContextView,
        tools: Vec<PreparedDurableToolReading>,
    ) {
        self.snapshot.attempt = attempt;
        self.snapshot.context = context;
        for tool in tools {
            let PreparedDurableToolReading {
                occurrence,
                content: bindings,
                collections,
            } = tool;
            self.install_content_metadata(
                bindings
                    .iter()
                    .map(|binding| binding.content_ref.clone())
                    .collect(),
            );
            let slot = self.snapshot.attempt.as_ref().and_then(|attempt| {
                attempt
                    .foreground
                    .iter()
                    .find(|slot| slot.occurrence == occurrence)
            });
            if let Some(metadata) = slot.and_then(|slot| slot.result_metadata_ref.clone())
                && let Some(frontier) = collections.into_iter().next()
            {
                self.foreground_content.push(ForegroundResultContent {
                    occurrence,
                    metadata,
                    content: bindings
                        .into_iter()
                        .filter(|binding| {
                            binding.path.get(1).is_some_and(|field| field == "result")
                        })
                        .collect(),
                    frontier,
                });
            }
        }
    }

    /// Seeds the normal foreground-tool read model from one canonical child
    /// message while replaying the child's durable Event Journal. The
    /// assistant message remains in the Message Ledger; this only gives the
    /// projection the same call slots that live publication frames create
    /// before `ToolExecutionStarted`/`ToolExecutionCompleted` facts arrive.
    ///
    /// No message is copied into another durable authority and no client event
    /// is published by this bootstrap-only operation.
    #[cfg(test)]
    pub(crate) fn bootstrap_durable_message(
        &mut self,
        attempt_id: &AttemptId,
        message: &MessageBlock,
    ) {
        let MessageBlock::Assistant(assistant) = message else {
            if let MessageBlock::Tool(tool) = message
                && let Some(attempt) = self
                    .snapshot
                    .attempt
                    .as_mut()
                    .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
                && let Some(slot) = attempt.foreground.iter_mut().find(|slot| {
                    slot.message_id.as_ref() == Some(&tool.occurrence.assistant_message_id)
                        && slot.block_index == tool.occurrence.block_index
                })
            {
                slot.committed_result = Some(tool.result.clone());
            }
            return;
        };
        self.ensure_attempt(attempt_id);
        let attempt = self
            .snapshot
            .attempt
            .as_mut()
            .expect("durable message bootstrap creates the attempt view");
        for (index, block) in assistant.content.iter().enumerate() {
            let crate::message::types::AssistantContentBlock::ToolCall(call) = block else {
                continue;
            };
            if attempt
                .foreground
                .iter()
                .any(|slot| slot.call_id.as_ref() == Some(&call.id))
            {
                continue;
            }
            if attempt.foreground.len() >= 32 {
                break;
            }
            attempt.foreground.push(ForegroundToolExecution {
                committed_result: None,
                committed_result_deferred: false,
                result_source: None,
                result_metadata_ref: None,
                occurrence: super::snapshot::RuntimeClientToolOccurrence {
                    locator: super::content::CanonicalContentLocator::ByMessageId {
                        message_id: assistant.id.clone(),
                    },
                    block_index: ContentBlockIndex::new(
                        u32::try_from(index).expect("canonical block index"),
                    ),
                },
                message_id: super::snapshot::inline_message_id(&assistant.id),
                block_index: ContentBlockIndex::new(
                    u32::try_from(index).expect("canonical block index"),
                ),
                call_id: super::snapshot::inline_call_id(&call.id),
                metadata_ref: None,
                assembly_complete: true,
                tool_id: super::snapshot::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                state: ForegroundToolState::Assembled {
                    arguments: String::new(),
                },
            });
        }
    }

    fn rebuild_pending_head(&mut self) {
        let mut head: Vec<_> = self.pending_heads.values().flatten().cloned().collect();
        head.sort_by_key(|header| header.source_offset);
        head.truncate(32);
        self.snapshot.pending_interactions = head;
    }

    /// Applies one authoritative observation: fold the snapshot read
    /// model, publish the resulting events, and deliver to subscribers.
    ///
    /// This is the one projection application path. Fold, cursor
    /// allocation, replay retention, and delivery share the caller's
    /// synchronization boundary, so no caller may partially apply an
    /// observation.
    pub(crate) fn apply(&mut self, observation: ConversationObservation) {
        if let ConversationObservation::TranscriptSourceReady(proof) = observation {
            if self.observe_transcript_source_prefix(proof.prefix, &proof.read_cut) {
                self.read_domains_dirty = true;
                if let Some(staged) = &mut self.staged_publication {
                    staged.2 = true;
                }
                self.publish(RuntimeClientEvent::ProjectionTransition {
                    events: Vec::new(),
                    metadata: self.projection_metadata(),
                });
            }
            return;
        }

        let observation = match observation {
            ConversationObservation::Workflow(mut cuts) => {
                cuts.retain(|source| source.revision() > self.snapshot.workflows.revision);
                if cuts.is_empty() {
                    return;
                }
                if cuts.len() > 1 {
                    for source in cuts {
                        self.apply(ConversationObservation::Workflow(vec![source]));
                    }
                    return;
                }
                ConversationObservation::Workflow(cuts)
            }
            other => other,
        };
        if let ConversationObservation::PendingReceipt(receipt) = observation {
            self.apply_pending_receipt(Some(receipt.as_ref()));
            return;
        }
        if let ConversationObservation::ToolOccurrenceProof {
            occurrence,
            observation,
        } = observation
        {
            let previous = self.tool_occurrence.replace(occurrence);
            self.apply(*observation);
            self.tool_occurrence = previous;
            return;
        }
        if let ConversationObservation::CollectionProof {
            owner,
            cut,
            ordinal,
            observation,
        } = observation
        {
            self.apply_collection_proof(owner, cut, ordinal, observation);
            return;
        }
        if let ConversationObservation::ObservationBatch(observations) = observation {
            self.apply_observation_batch(observations);
            return;
        }
        if self.exhausted {
            return;
        }
        if let ConversationObservation::Published { observation, .. } = observation {
            self.apply(*observation);
            return;
        }
        if let ConversationObservation::JournalBatch {
            through,
            observations,
        } = observation
        {
            self.apply_journal_batch(through, observations);
            return;
        }
        if let ConversationObservation::JournalFact(witness) = &observation
            && ((!self.publication_state.installing_native_batch
                && witness.native_direct
                && !witness.source_unavailable)
                || self.already_installed_tool_start(witness))
        {
            self.prepared_journal.remove(&witness.sequence);
            self.prepared_outcomes.remove(&witness.sequence);
            return;
        }
        if !self.prepare_observation_revision(&observation) {
            return;
        }
        let published = self.fold(observation);
        #[cfg(test)]
        self.probe_publish_enter();
        match published.len() {
            0 => {}
            1 => self.publish(published.into_iter().next().expect("one transition")),
            _ => {
                let metadata = self.projection_metadata();
                self.publish(RuntimeClientEvent::ProjectionTransition {
                    events: published,
                    metadata,
                });
            }
        }
    }

    // A worker may prepare this witness before the native owner marks its row
    // direct, then deliver it after that exact occurrence has started. Only
    // that repeated Started fact is redundant; terminal and other Journal facts
    // still fold in their native order.
    fn already_installed_tool_start(
        &self,
        witness: &crate::durable::observation::ObservationEventWitness,
    ) -> bool {
        if self.publication_state.installing_native_batch
            || witness.source_unavailable
            || witness.event_kind != "tool_execution_started"
            || witness.sequence > self.journal_through
        {
            return false;
        }
        let Some(native) = &witness.tool_occurrence else {
            return false;
        };
        self.snapshot.attempt.as_ref().is_some_and(|attempt| {
            attempt.attempt_id.as_ref() == witness.attempt_id.as_ref()
                && attempt.foreground.iter().any(|slot| {
                    slot.occurrence.locator
                        == super::content::CanonicalContentLocator::ByLedgerPosition {
                            position: native.ledger_position,
                        }
                        && slot.occurrence.block_index == native.block_index
                        && matches!(
                            slot.state,
                            ForegroundToolState::Running { .. }
                                | ForegroundToolState::Settled { .. }
                        )
                })
        })
    }

    fn prepare_observation_revision(&mut self, observation: &ConversationObservation) -> bool {
        if invalidates_read_domains(observation) {
            self.read_through = self.journal_through;
            self.read_domains_dirty = true;
            let Some(revision) = self.read_revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return false;
            };
            self.read_revision = revision;
        }
        self.advance_domain_revisions(observation);
        #[cfg(test)]
        if let ConversationObservation::Event { attempt_id, event } = observation
            && is_attempt_terminal(event)
            && let Some(cursor) = self.prepared_terminals.remove(attempt_id)
        {
            self.observe_transcript_position(cursor);
        }
        match observation {
            #[cfg(test)]
            ConversationObservation::InboundEnqueued(item) => {
                if let Some(cursor) = item.transcript_cursor() {
                    self.observe_transcript_position(cursor);
                    self.pending_transcript
                        .insert(item.message().id.clone(), cursor);
                }
            }
            #[cfg(test)]
            ConversationObservation::InboundDrained(batch) => {
                for item in batch.items() {
                    self.pending_transcript.remove(&item.message().id);
                }
            }
            #[cfg(test)]
            ConversationObservation::PendingInboundChanged(items) => {
                self.pending_transcript = items
                    .iter()
                    .filter_map(|item| {
                        item.transcript_cursor()
                            .map(|cursor| (item.message().id.clone(), cursor))
                    })
                    .collect();
            }

            ConversationObservation::InteractionPending {
                audit: Some((_, cursor)),
                ..
            }
            | ConversationObservation::InteractionSettled {
                audit: Some((_, cursor)),
                ..
            } => {
                self.observe_transcript_position(*cursor);
            }
            _ => {}
        }
        true
    }

    fn apply_collection_proof(
        &mut self,
        owner: &'static str,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
        observation: Box<ConversationObservation>,
    ) {
        let before_cursor = self.cursor;
        let before_count = self.batch_events.as_ref().map(|batch| batch.0.len());
        let changed = self
            .collection_cuts
            .get(owner)
            .is_none_or(|current| cut.generation > current.generation);
        let observed = self.collection_cuts.entry(owner.into()).or_insert(cut);
        if cut.generation > observed.generation {
            *observed = cut;
        }
        let adjusted = ordinal
            .and_then(|ordinal| ordinal.checked_add(observed.total.saturating_sub(cut.total)))
            .unwrap_or(u64::MAX);
        let previous = self.collection_ordinal.replace(adjusted);
        self.apply(*observation);
        self.collection_ordinal = previous;
        if changed
            && self.cursor == before_cursor
            && self.batch_events.as_ref().map(|batch| batch.0.len()) == before_count
        {
            self.publish(RuntimeClientEvent::CollectionChanged {
                domain: owner.into(),
                cut,
            });
        }
    }

    fn apply_observation_batch(&mut self, observations: Vec<ConversationObservation>) {
        if self.batch_events.is_some() {
            for observation in observations {
                self.apply(observation);
            }
            return;
        }
        self.batch_events = Some((Vec::new(), 0, false));
        for observation in observations {
            self.apply(observation);
        }
        let (events, _, overflow) = self.batch_events.take().expect("native transition buffer");
        if overflow && let Some(staged) = &mut self.staged_publication {
            staged.2 = true;
        } else if overflow {
            self.publish(RuntimeClientEvent::CheckpointAdvanced {
                transition: "native_observation_batch".into(),
                checkpoint: Box::new(if let Ok(checkpoint) = self.checkpoint_candidate() {
                    checkpoint
                } else {
                    self.mark_observation_unavailable("native batch display unavailable".into());
                    return;
                }),
            });
        } else if !events.is_empty() {
            self.publish(RuntimeClientEvent::ProjectionTransition {
                events,
                metadata: self.projection_metadata(),
            });
        }
    }

    fn apply_journal_batch(
        &mut self,
        through: Option<u64>,
        observations: Vec<ConversationObservation>,
    ) {
        assert!(
            self.batch_events.is_none(),
            "native Journal batches cannot nest"
        );
        self.batch_events = Some((Vec::new(), 0, false));
        for observation in observations {
            self.apply(observation);
        }
        if let Some(through) = through {
            let duplicate = self
                .batch_events
                .as_ref()
                .is_some_and(|batch| batch.0.is_empty())
                && through <= self.journal_through;
            if duplicate {
                self.batch_events = None;
                return;
            }
            self.journal_through = self.journal_through.max(through);
            self.read_through = self.read_through.max(through);
            self.source_state.source_unavailable = false;
            self.source_state.journal_failure = false;
            self.publish(RuntimeClientEvent::TraceChanged);
            let (events, _, overflow) = self.batch_events.take().expect("batch buffer");
            if overflow && let Some(staged) = &mut self.staged_publication {
                staged.2 = true;
                return;
            }
            let event = if overflow {
                RuntimeClientEvent::CheckpointAdvanced {
                    transition: "native_journal_batch".into(),
                    checkpoint: Box::new(if let Ok(checkpoint) = self.checkpoint_candidate() {
                        checkpoint
                    } else {
                        self.mark_observation_unavailable(
                            "native Journal display unavailable".into(),
                        );
                        return;
                    }),
                }
            } else {
                RuntimeClientEvent::ProjectionTransition {
                    events,
                    metadata: self.projection_metadata(),
                }
            };
            self.publish(event);
        } else {
            self.batch_events = None;
            self.source_state.source_unavailable = true;
            self.source_state.journal_failure = true;
            self.read_failure = Some("native Journal observation source unavailable".into());
            self.wake_subscribers();
        }
    }

    /// Folds one observation into the read model and returns the external
    /// events the observation publishes.
    #[allow(clippy::too_many_lines)]
    fn fold(&mut self, observation: ConversationObservation) -> Vec<RuntimeClientEvent> {
        match observation {
            ConversationObservation::ObservationBatch(_)
            | ConversationObservation::JournalBatch { .. } => {
                unreachable!("batches are applied at the synchronization boundary")
            }
            ConversationObservation::Published { observation, .. }
            | ConversationObservation::CollectionProof { observation, .. }
            | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
                self.fold(*observation)
            }
            ConversationObservation::Workflow(cuts) => {
                for source in cuts {
                    let header = source.header();
                    self.domain_revisions.insert("workflows", header.revision.0);
                    self.source_frontiers
                        .retain(|domain, _| !domain.starts_with("workflows."));
                    self.install_source_frontiers(source.frontiers());
                    let value = Arc::new(super::domain::SourceValue::new(Arc::new(source)));
                    let source = Arc::new(super::domain::DomainSource::Workflow(value));
                    let weight = source.encoded_weight();
                    self.install_domain_sources(vec![("workflows".into(), source, weight)]);
                    self.fold_workflows(header);
                }
                Vec::new()
            }
            ConversationObservation::GoalChanged(view) => {
                self.snapshot.goal = Some(view.clone());
                vec![RuntimeClientEvent::GoalChanged { view }]
            }
            ConversationObservation::PendingReceipt(_) => {
                unreachable!("pending receipt has a finite source fold")
            }
            ConversationObservation::NativeToolResultCommit(tool) => {
                let occurrence = super::snapshot::RuntimeClientToolOccurrence {
                    locator: super::content::CanonicalContentLocator::ByLedgerPosition {
                        position: tool.occurrence.ledger_position,
                    },
                    block_index: tool.occurrence.block_index,
                };
                update_stored_tool_slots(&mut self.snapshot, &occurrence, |slot| {
                    let mut result = tool.result.clone();
                    super::tool_observation::defer_result(&mut result);
                    slot.committed_result = Some(result.clone());
                    slot.committed_result_deferred = tool.result_deferred
                        || super::tool_observation::has_result_body(&tool.result);
                    slot.result_metadata_ref = Some(tool.metadata.clone());
                    if let ForegroundToolState::Settled {
                        result: settled, ..
                    } = &mut slot.state
                    {
                        settled.content.clear();
                        settled.artifacts.clear();
                        settled.deliveries.clear();
                        settled.workflow = None;
                        settled.managed_output = None;
                    }
                });
                let Some(attempt) = &mut self.snapshot.attempt else {
                    return Vec::new();
                };
                self.foreground_content.retain(|known| {
                    attempt
                        .foreground
                        .iter()
                        .any(|slot| slot.occurrence == known.occurrence)
                });
                let Some(slot) = attempt
                    .foreground
                    .iter_mut()
                    .find(|slot| slot.occurrence == occurrence)
                else {
                    return Vec::new();
                };
                slot.result_metadata_ref = Some(tool.metadata.clone());
                slot.committed_result_deferred = tool.result_deferred;
                let result = tool.result.clone();
                let source = ForegroundResultContent {
                    occurrence: occurrence.clone(),
                    metadata: tool.metadata.clone(),
                    content: tool.content.iter().take(4).cloned().collect(),
                    frontier: super::checkpoint::CollectionFrontier {
                        domain: format!("messages.ledger:{}.blocks", tool.result_ledger_position),
                        revision: 0,
                        total: tool.content_total,
                        loaded: 0,
                        next_offset: (tool.content_total > 0).then_some(0),
                    },
                };
                if let Some(current) = self
                    .foreground_content
                    .iter_mut()
                    .find(|known| known.occurrence == occurrence)
                {
                    *current = source;
                } else if self.foreground_content.len() < 32 {
                    self.foreground_content.push(source);
                }
                slot.committed_result = Some(result.clone());
                vec![RuntimeClientEvent::ToolResultCommitted {
                    attempt_id: attempt
                        .attempt_id
                        .clone()
                        .expect("live frame has a native attempt identity"),
                    occurrence,
                    result,
                    result_deferred: tool.result_deferred,
                    result_metadata_ref: slot.result_metadata_ref.clone(),
                }]
            }
            ConversationObservation::JournalFact(witness) => self.fold_journal_fact(witness),
            ConversationObservation::JournalSourceReady => Vec::new(),
            ConversationObservation::TranscriptSourceReady(_) => {
                unreachable!("T readiness is applied before revision changes")
            }
            #[cfg(test)]
            ConversationObservation::Event { attempt_id, event } => {
                self.fold_event(&attempt_id, &event)
            }
            #[cfg(test)]
            ConversationObservation::ManualCompactionEvent { event, request_id } => {
                self.snapshot.context.manual_compaction =
                    Some(super::snapshot::RuntimeClientManualCompaction {
                        request_id,
                        released: !matches!(event, RuntimeEvent::CompactionStarted { .. }),
                        error: if let RuntimeEvent::CompactionFailed { error, .. } = &event {
                            Some(error.clone())
                        } else {
                            None
                        },
                    });
                self.fold_compaction_event(None, &event)
            }
            ConversationObservation::CanonicalCommitted {
                attempt_id,
                receipt,
            } => self.fold_canonical_receipt(attempt_id.as_ref(), receipt),
            ConversationObservation::AuditSettled {
                attempt_id,
                receipt,
            } => self.fold_audit_receipt(attempt_id, receipt),
            #[cfg(test)]
            ConversationObservation::Committed {
                attempt_id,
                block,
                transcript_cursor,
            } => {
                // A canonical ToolMessage is the authoritative result fact.
                // It repairs an accepted foreground slot that has not seen a
                // live execution settlement (notably BeforeStart, which must
                // never fabricate started/completed facts). A slot transitions
                // to Settled at most once: the live event path and this commit
                // path share the same idempotent helper, and only the winner
                // publishes ToolExecutionSettled.
                let projected_block = self
                    .prepared_messages
                    .get(block.id())
                    .and_then(|(header, _)| header.display.clone())
                    .unwrap_or_else(|| {
                        let mut capture = super::checkpoint::Capture::new(self.cursor.get());
                        capture.message(&block, &self.snapshot.conversation_id, &[])
                    });
                let mut events = Vec::with_capacity(2);
                let settled_witness = if let MessageBlock::Tool(tool) = &projected_block {
                    self.snapshot
                        .attempt
                        .as_ref()
                        .and_then(|attempt| {
                            attempt.foreground.iter().find(|slot| {
                                slot.message_id.as_ref()
                                    == Some(&tool.occurrence.assistant_message_id)
                                    && slot.block_index == tool.occurrence.block_index
                            })
                        })
                        .map(|slot| (slot.occurrence.clone(), slot.metadata_ref.clone()))
                } else {
                    None
                };
                if let (Some(attempt_id), MessageBlock::Tool(tool)) =
                    (attempt_id.as_ref(), &projected_block)
                    && self.settle_foreground(
                        attempt_id,
                        &tool.tool_call_id,
                        Some(&tool.occurrence),
                        tool.result.clone(),
                    ) == ForegroundSettlement::Applied
                {
                    events.push(RuntimeClientEvent::ToolExecutionSettled {
                        result_source: None,
                        result_metadata_ref: None,
                        attempt_id: attempt_id.clone(),
                        occurrence: settled_witness
                            .as_ref()
                            .map(|(occurrence, _)| occurrence.clone()),
                        metadata_ref: settled_witness.and_then(|(_, reference)| reference),
                        tool_call_id: super::snapshot::inline_call_id(&tool.tool_call_id),
                        tool_id: super::snapshot::inline_tool_id(&tool.tool_id),
                        result: tool.result.clone(),
                    });
                }
                if matches!(block, MessageBlock::Assistant(_))
                    && self
                        .publication_start
                        .as_ref()
                        .is_some_and(|start| &start.message_id == block.id())
                    && let Some(attempt) = &mut self.snapshot.attempt
                {
                    attempt.in_flight = None;
                    self.publication_start = None;
                    self.live_frontiers.clear();
                }
                // The task list is derived from canonical results, exactly
                // as the runtime's own list is: a committed `todo` result
                // *is* the list moving. A result whose payload does not
                // decode is left out rather than allowed to replace a good
                // list with a broken one — the runtime writes these, so an
                // undecodable one is a defect, not a list.
                //
                // The fold is guarded on the composition fact rather than on
                // the message: a runtime that composes no Todo extension must
                // present no current list at all, and canonical history it
                // inherited — which still legitimately contains `todo`
                // results, and still renders as transcript history — must
                // never be able to manufacture one.
                if let Some(current) = self.snapshot.todos.as_mut()
                    && let Some(Ok(todos)) = crate::tools::todo::published_snapshot(&block)
                {
                    *current = todos;
                }
                let header = self
                    .prepared_messages
                    .remove(block.id())
                    .map(|(header, revision)| {
                        if let (Some(cut), Some(revision)) = (&mut self.source_cut, revision) {
                            cut.surface_revision = cut.surface_revision.max(revision);
                        }
                        header
                    });
                if let Some(header) = &header {
                    if let Some(position) = header.ledger_position {
                        if let MessageBlock::Assistant(assistant) = &block {
                            self.canonical_foreground =
                                Some(super::checkpoint::CanonicalForegroundReadCut {
                                    locator:
                                        super::content::CanonicalContentLocator::ByLedgerPosition {
                                            position,
                                        },
                                    total: header.tool_call_count,
                                });
                            self.native_tool_occurrences.clear();
                            if let Some(attempt) = &self.snapshot.attempt {
                                for slot in &attempt.foreground {
                                    if slot.message_id.as_ref() != Some(&assistant.id) {
                                        continue;
                                    }
                                    if let Some(
                                        crate::message::types::AssistantContentBlock::ToolCall(
                                            call,
                                        ),
                                    ) = assistant.content.get(slot.block_index.get() as usize)
                                    {
                                        self.native_tool_occurrences.insert(call.id.clone(),super::snapshot::RuntimeClientToolOccurrence {locator:super::content::CanonicalContentLocator::ByLedgerPosition {position},block_index:slot.block_index});
                                    }
                                }
                            }
                        }
                        if let Some(attempt) = self.snapshot.attempt.as_mut() {
                            for slot in attempt
                                .foreground
                                .iter_mut()
                                .filter(|slot| slot.message_id.as_ref() == Some(block.id()))
                            {
                                let previous_locator = slot.occurrence.locator.clone();
                                slot.occurrence.locator =
                                    super::content::CanonicalContentLocator::ByLedgerPosition {
                                        position,
                                    };
                                slot.metadata_ref=self.content_metadata.get(&("canonical_block_metadata",format!("ledger:{position}"),slot.block_index.get(),super::content::ContentRepresentation::CanonicalJsonV1.key())).map(Arc::as_ref).cloned().or_else(||header.content.iter().find_map(|binding|match &binding.content_ref.source {
                                    super::content::ContentSource::CanonicalMessageBlock {component:super::content::CanonicalContentComponent::BlockMetadata,block_index,..} if *block_index==slot.block_index=>Some(binding.content_ref.clone()),
                                    _=>None,
                                }));
                                events.push(RuntimeClientEvent::ToolOccurrenceRebound {
                                    attempt_id: attempt
                                        .attempt_id
                                        .clone()
                                        .expect("live frame has a native attempt identity"),
                                    previous_locator,
                                    occurrence: slot.occurrence.clone(),
                                    metadata_ref: slot.metadata_ref.clone(),
                                });
                            }
                        }
                    }
                    self.head_bindings
                        .insert(block.id().clone(), header.content.clone());
                }

                let descriptor = header
                    .as_ref()
                    .and_then(super::snapshot::RuntimeClientMessageDescriptor::from_header);
                let descriptor_only = header
                    .as_ref()
                    .is_some_and(|header| header.display.is_none());
                if let Some(descriptor) = descriptor.as_ref() {
                    self.snapshot.message_descriptors.push(descriptor.clone());
                    if self.snapshot.message_descriptors.len()
                        > super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
                    {
                        self.snapshot.message_descriptors.remove(0);
                    }
                }
                let mut evicted_message_id = None;
                if let Some(cursor) = transcript_cursor {
                    self.observe_transcript_position(cursor);
                }
                if let Some(cut) = self.source_cut.as_mut() {
                    if let Some(position) =
                        header.as_ref().and_then(|header| header.ledger_position)
                    {
                        cut.ledger_through = cut.ledger_through.max(position);
                    }
                    if self.snapshot.messages.len()
                        >= super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
                    {
                        let removed = self.snapshot.messages.remove(0);
                        evicted_message_id = Some(removed.id().clone());
                        self.head_bindings.remove(removed.id());
                    }
                    if let Some(display) = header.and_then(|header| header.display) {
                        self.snapshot.messages.push(display);
                    }
                } else {
                    self.snapshot.messages.push(block.clone());
                }
                let transcript_cursor = transcript_cursor.map(RuntimeClientTranscriptCursor::from);
                if descriptor_only {
                    if let Some(descriptor) = descriptor {
                        events.push(RuntimeClientEvent::MessageDescriptorCommitted {
                            attempt_id,
                            descriptor,
                            head: self.snapshot.message_descriptors.clone(),
                            transcript_cursor,
                            evicted_message_id,
                        });
                    }
                } else {
                    if let Some(descriptor) = descriptor {
                        events.push(RuntimeClientEvent::MessageDescriptorCommitted {
                            attempt_id: attempt_id.clone(),
                            descriptor,
                            head: self.snapshot.message_descriptors.clone(),
                            transcript_cursor,
                            evicted_message_id: evicted_message_id.clone(),
                        });
                    }
                    events.push(RuntimeClientEvent::MessageCommitted {
                        evicted_message_id,
                        attempt_id,
                        message: projected_block,
                        transcript_cursor,
                    });
                }
                events
            }
            // The three publication observations replace what used to be
            // per-delta Event Journal facts (Issue #108). The client-facing
            // vocabulary is unchanged: a frame folds into exactly the
            // streaming event a delta used to publish, and every one of them
            // is already durably committed for release before it arrives.
            ConversationObservation::PublicationOpened { attempt_id, start } => {
                self.publication_start = Some(start.clone());
                self.canonical_foreground = None;
                self.native_tool_occurrences.clear();
                self.live_revision = 0;
                self.live_block_total = 0;
                self.live_tool_total = 0;
                self.live_block_ordinal = None;
                self.live_tool_ordinal = None;
                self.live_frontiers.clear();
                self.ensure_attempt(&attempt_id);
                self.snapshot
                    .attempt
                    .as_mut()
                    .expect("attempt view exists")
                    .foreground
                    .clear();
                self.snapshot
                    .attempt
                    .as_mut()
                    .expect("attempt view exists")
                    .in_flight = Some(InFlightAssistantMessage {
                    message_id: start.message_id.clone(),
                    blocks: Vec::new(),
                });
                vec![RuntimeClientEvent::AssistantMessageStarted {
                    attempt_id,
                    message_id: start.message_id,
                }]
            }
            ConversationObservation::Publication { attempt_id, frame } => {
                self.fold_publication_frame(&attempt_id, &frame)
            }
            // An audit is a derived transcript item, but it is not a
            // canonical Message Ledger message and never becomes an
            // in-flight message. It only closes the in-flight read model of a
            // stream that will never be canonically accepted.

            // A composed status joins the bounded window in composition
            // order. It replaces nothing: an earlier composition is a
            // historical fact of this conversation and stays exactly where
            // the runtime put it.
            //
            // Placement is projected, never reconstructed: the observation
            // already carries the durable identity its opportunity froze, so
            // nothing here consults the conversation's current transcript
            // position. Doing that would sample a frontier that unrelated
            // durable activity may have advanced since the composition was
            // determined.
            //
            // Retention is owned here and published as part of the
            // transition. The event carries the exact eviction this
            // admission caused, so an incremental fold reproduces this
            // window instead of applying a second retention policy of its
            // own — which is what makes a live fold and a snapshot repair at
            // the same cursor agree past the window bound.
            ConversationObservation::Status(view) => {
                let evicted = admit_status(&mut self.snapshot.statuses, view.clone());
                let evicted_status_message_id = evicted
                    .as_ref()
                    .and_then(|status| status.status_message_id.clone());
                let evicted_status_locator = evicted.and_then(|status| status.source_locator);
                vec![RuntimeClientEvent::AgentStatusComposed {
                    attempt_id: view.attempt_id.clone(),
                    turn: view.turn,
                    status: view,
                    evicted_status_message_id,
                    evicted_status_locator,
                }]
            }
            #[cfg(test)]
            ConversationObservation::PendingInboundChanged(items) => {
                self.snapshot.inbound.pending = self.prepared_pending.take().unwrap_or_else(|| {
                    items
                        .iter()
                        .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
                        .map(inbound_item_view)
                        .collect()
                });
                vec![RuntimeClientEvent::PendingInboundChanged {
                    pending: self.snapshot.inbound.pending.clone(),
                }]
            }
            #[cfg(test)]
            ConversationObservation::InboundEnqueued(item) => {
                if let Some(pending) = self.prepared_pending.take() {
                    self.snapshot.inbound.pending = pending;
                    vec![RuntimeClientEvent::PendingInboundChanged {
                        pending: self.snapshot.inbound.pending.clone(),
                    }]
                } else {
                    let view = inbound_item_view(&item);
                    self.snapshot.inbound.pending.push(view.clone());
                    self.snapshot
                        .inbound
                        .pending
                        .truncate(super::checkpoint::CHECKPOINT_PAGE_ITEMS);
                    vec![RuntimeClientEvent::InboundEnqueued {
                        sequence: item.sequence(),
                        message: view.message,
                        transcript_cursor: item
                            .transcript_cursor()
                            .map(RuntimeClientTranscriptCursor::from),
                    }]
                }
            }
            #[cfg(test)]
            ConversationObservation::InboundDrained(batch) => {
                if let Some(pending) = self.prepared_pending.take() {
                    self.snapshot.inbound.pending = pending;
                } else {
                    self.snapshot
                        .inbound
                        .pending
                        .retain(|item| item.sequence > batch.watermark());
                }
                self.snapshot.inbound.last_drain = Some(InboundDrainView {
                    watermark: batch.watermark(),
                    count: batch.items().len(),
                });
                vec![RuntimeClientEvent::InboundDrained {
                    watermark: batch.watermark(),
                    count: batch.items().len(),
                    message_ids: batch
                        .items()
                        .iter()
                        .map(|item| item.message().id.clone())
                        .collect(),
                }]
            }
            ConversationObservation::PendingHead { owner, cut, head } => {
                if self
                    .collection_cuts
                    .get(owner)
                    .is_some_and(|current| current.generation > cut.generation)
                {
                    return Vec::new();
                }
                self.collection_cuts.insert(owner.into(), cut);
                self.pending_heads.insert(owner.into(), head.clone());
                self.rebuild_pending_head();
                vec![RuntimeClientEvent::PendingHeadChanged {
                    owner: owner.into(),
                    cut,
                    head: self.snapshot.pending_interactions.clone(),
                }]
            }
            ConversationObservation::InteractionPending { interaction, audit } => {
                let header = interaction;
                upsert_interaction(&mut self.snapshot.pending_interactions, header.clone());
                let mut events = vec![RuntimeClientEvent::InteractionPending {
                    interaction: header,
                }];
                if let Some((audit, transcript_cursor)) = audit {
                    let audit_view = super::snapshot::interaction_requested_view(audit)
                        .expect("runtime interaction requested audit is valid");
                    let transcript_cursor = RuntimeClientTranscriptCursor::from(transcript_cursor);
                    events.push(RuntimeClientEvent::InteractionAuditRequested {
                        audit: Box::new(audit_view),
                        transcript_cursor,
                    });
                }
                events
            }
            ConversationObservation::InteractionSettled {
                interaction,
                outcome,
                audit,
            } => {
                self.snapshot
                    .pending_interactions
                    .retain(|pending| pending.interaction != interaction);
                let mut events = vec![RuntimeClientEvent::InteractionSettled {
                    interaction,
                    outcome,
                }];
                if let Some(audit) = audit {
                    let (audit, transcript_cursor) = audit;
                    let audit_view = super::snapshot::interaction_settled_view(audit)
                        .expect("runtime interaction settled audit is valid");
                    let transcript_cursor = RuntimeClientTranscriptCursor::from(transcript_cursor);
                    events.push(RuntimeClientEvent::InteractionAuditSettled {
                        audit: Box::new(audit_view),
                        transcript_cursor,
                    });
                }
                events
            }
            #[cfg(test)]
            ConversationObservation::InteractionRemoved { interaction } => {
                self.snapshot
                    .pending_interactions
                    .retain(|pending| pending.interaction != interaction);
                vec![RuntimeClientEvent::InteractionRemoved { interaction }]
            }
            ConversationObservation::Background(snapshot) => {
                let view = background_view(&snapshot);
                let position = self.collection_ordinal.unwrap_or(0);
                let evicted_job_id = if position < 32 {
                    upsert_background(&mut self.snapshot.jobs, view.clone())
                } else {
                    None
                };
                vec![RuntimeClientEvent::JobUpdated {
                    job: view,
                    position,
                    evicted_job_id,
                }]
            }
            ConversationObservation::ToolProgress {
                attempt_id,
                tool_call_id,
                tool_id,
                progress,
            } => {
                // Live-only, latest-value (Issue #178): the report is folded
                // directly into the running foreground slot owned by this
                // projection. It is never written to the Event Journal. A
                // read-only attachment therefore sees the same disposable
                // state as the child runtime's ordinary Runtime Client
                // projection, while durable bootstrap still reconstructs
                // only committed progress facts.
                let occurrence = self
                    .tool_display_occurrence()
                    .or_else(|| self.native_tool_occurrences.get(&tool_call_id).cloned());
                let metadata_ref = self
                    .snapshot
                    .attempt
                    .as_ref()
                    .and_then(|attempt| {
                        attempt
                            .foreground
                            .iter()
                            .find(|slot| Some(&slot.occurrence) == occurrence.as_ref())
                    })
                    .and_then(|slot| slot.metadata_ref.clone());
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(&attempt_id)
                    && let Some(slot) = attempt
                        .foreground
                        .iter_mut()
                        .find(|slot| Some(&slot.occurrence) == occurrence.as_ref())
                    && let ForegroundToolState::Running { arguments, .. } = &slot.state
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionProgress {
                    occurrence,
                    metadata_ref,
                    attempt_id,
                    tool_call_id: super::snapshot::inline_call_id(&tool_call_id),
                    tool_id: super::snapshot::inline_tool_id(&tool_id),
                    execution_id: None,
                    progress,
                }]
            }
            ConversationObservation::SubagentLifecycle { agent, snapshot } => {
                let Some(agent) = agent else {
                    return Vec::new();
                };
                if !matches!(
                    snapshot.ownership,
                    crate::events::types::SubagentOwnershipKind::Normal
                ) {
                    return Vec::new();
                }
                // The registry owns both identities and lifecycle. One receipt
                // installs this complete pair; disposable activity may only
                // enrich this published owner state afterward.
                let view = agent_view(&agent, &snapshot);
                let position = self.collection_ordinal.unwrap_or(0);
                let evicted_agent_id = (position < 32
                    && self.snapshot.agents.len() >= 32
                    && !self
                        .snapshot
                        .agents
                        .iter()
                        .any(|item| item.agent_id == view.agent_id))
                .then(|| self.snapshot.agents.last().unwrap().agent_id.clone());
                if position < 32
                    && !upsert_subagent(
                        &mut self.snapshot.agents,
                        &mut self.seen_activations,
                        view.clone(),
                    )
                {
                    return Vec::new();
                }
                let mut events = vec![RuntimeClientEvent::AgentUpdated {
                    agent: Box::new(view),
                    position,
                    evicted_agent_id,
                }];
                if snapshot.state.is_terminal() {
                    let removed: Vec<_> = self
                        .snapshot
                        .pending_interactions
                        .iter()
                        .filter(|pending| {
                            pending.interaction.conversation_id == snapshot.child_conversation_id
                        })
                        .map(|pending| pending.interaction.clone())
                        .collect();
                    self.snapshot.pending_interactions.retain(|pending| {
                        pending.interaction.conversation_id != snapshot.child_conversation_id
                    });
                    events.extend(
                        removed.into_iter().map(|interaction| {
                            RuntimeClientEvent::InteractionRemoved { interaction }
                        }),
                    );
                }
                events
            }
            ConversationObservation::SubagentActivity(snapshot) => {
                if !matches!(
                    snapshot.ownership,
                    crate::events::types::SubagentOwnershipKind::Normal
                ) {
                    return Vec::new();
                }
                let Some(existing) = self.snapshot.agents.iter().find(|agent| {
                    agent.agent_id == snapshot.child_agent_id
                        && agent.activation_id == snapshot.subagent_id
                }) else {
                    return Vec::new();
                };
                let view = activation_view(
                    &snapshot,
                    existing.state,
                    existing.current_activation.clone(),
                    &existing.title,
                );
                if !upsert_subagent(
                    &mut self.snapshot.agents,
                    &mut self.seen_activations,
                    view.clone(),
                ) {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::AgentUpdated {
                    agent: Box::new(view),
                    position: self.collection_ordinal.unwrap_or(0),
                    evicted_agent_id: None,
                }]
            }
            ConversationObservation::Capability { source } => {
                let capabilities = source.header();
                self.capability_head = source.prepared_head();
                self.install_source_frontiers(source.frontiers());
                self.snapshot.capabilities.clone_from(&capabilities);
                vec![RuntimeClientEvent::CapabilityUpdated { capabilities }]
            }
            ConversationObservation::Resources {
                model,
                approval_mode,
                snapshot,
                capability,
                display,
            } => {
                // One configuration commit, one fold, one event. The runtime publishes the
                // resource generation and the capability generation it was
                // built against as a single observation precisely so both
                // views move together: this arm updates the snapshot
                // completely before returning, so no `snapshot()` call can
                // land between the two halves and read a pairing that never
                // existed.
                //
                // The published event carries both halves for the same
                // reason. Two events would be two cursors, and a client
                // that maintains its projection from the event stream would
                // sit at the first one holding the new capability
                // generation beside the retired resource generation.
                // Adjacent is not atomic; one event is.
                self.install_source_frontiers(vec![super::checkpoint::CollectionFrontier {
                    domain: "model_catalog.models".into(),
                    revision: 0,
                    total: model.catalog_source().model_count() as u64,
                    loaded: 0,
                    next_offset: (model.catalog_source().model_count() > 0).then_some(0),
                }]);
                let capabilities = capability.header();
                self.capability_head = capability.prepared_head();
                self.install_source_frontiers(capability.frontiers());
                let model_status = model.status();
                let model = model.inline().map(Box::new);
                self.install_model_frontier(model.is_some());
                let resources = display.header();
                self.install_source_frontiers(display.frontiers());
                let plugins = snapshot
                    .root_profile()
                    .map(|profile| super::settings::EffectivePlugins::project(&profile.extensions));
                self.snapshot.effective_plugins.clone_from(&plugins);
                self.snapshot.capabilities.clone_from(&capabilities);
                self.snapshot.resources.clone_from(&resources);
                self.snapshot.model = model.as_deref().cloned();
                self.snapshot.model_status = Some(model_status);
                self.snapshot.effective_approval_mode = approval_mode;
                vec![RuntimeClientEvent::ResourceGenerationUpdated {
                    model_status,
                    plugins,
                    model,
                    approval_mode,
                    capabilities,
                    resources,
                }]
            }
            ConversationObservation::AttemptAdmitted {
                read_image_active,
                attempt_id,
                model,
                resource_revision,
                approval_mode,
            } => {
                if self
                    .snapshot
                    .attempt
                    .as_ref()
                    .is_none_or(|attempt| attempt.attempt_id.as_ref() != Some(&attempt_id))
                {
                    self.foreground_content.clear();
                }
                self.install_source_frontiers(vec![super::checkpoint::CollectionFrontier {
                    domain: "attempt.model".into(),
                    revision: 0,
                    total: 1,
                    loaded: u64::from(model.inline().is_some()),
                    next_offset: model.inline().is_none().then_some(0),
                }]);
                let retained = self
                    .snapshot
                    .attempt
                    .take()
                    .filter(|current| current.attempt_id.as_ref() == Some(&attempt_id));
                self.snapshot.attempt = Some(RuntimeClientAttempt {
                    attempt_id: Some(attempt_id),
                    attempt_id_source: None,
                    phase: retained
                        .as_ref()
                        .map_or(RuntimeClientAttemptPhase::Admitted, |current| {
                            current.phase.clone()
                        }),
                    turn: 0,
                    last_usage: None,
                    in_flight: retained
                        .as_ref()
                        .and_then(|current| current.in_flight.clone()),
                    foreground: retained.map_or_else(Vec::new, |current| current.foreground),
                    model: model.inline().map(Box::new),
                    execution_settings: Some(super::settings::AdmittedSettings {
                        read_image_active: Some(read_image_active),
                        resource_revision,
                        approval_mode,
                    }),
                });
                // Admission changes frozen attempt/model authority before
                // execution starts. Publish that complete state at its own cut.
                if let Some(staged) = &mut self.staged_publication {
                    staged.2 = true;
                    Vec::new()
                } else {
                    #[cfg(test)]
                    {
                        vec![RuntimeClientEvent::CheckpointAdvanced {
                            transition: "attempt_admitted".into(),
                            checkpoint: Box::new(match self.checkpoint_candidate() {
                                Ok(checkpoint) => checkpoint,
                                Err(error) => {
                                    self.publication_failed(&error);
                                    return Vec::new();
                                }
                            }),
                        }]
                    }
                    #[cfg(not(test))]
                    {
                        self.mark_observation_unavailable(
                            "native admission lacks publication scope".into(),
                        );
                        Vec::new()
                    }
                }
            }
            ConversationObservation::SessionModelChanged { model, capability } => {
                let capability =
                    capability.expect("model observation capability prepared outside Projection");
                self.snapshot.capabilities = capability.header();
                self.capability_head = capability.prepared_head();
                self.install_source_frontiers(capability.frontiers());
                let model_status = model.status();
                let model = model.inline().map(Box::new);
                self.install_model_frontier(model.is_some());
                self.snapshot.model = model.as_deref().cloned();
                self.snapshot.model_status = Some(model_status);
                vec![
                    RuntimeClientEvent::SessionModelChanged {
                        model,
                        model_status,
                    },
                    RuntimeClientEvent::CapabilityUpdated {
                        capabilities: self.snapshot.capabilities.clone(),
                    },
                ]
            }
            ConversationObservation::Shutdown => {
                self.snapshot.shutting_down = true;
                vec![RuntimeClientEvent::RuntimeShutdown]
            }
            ConversationObservation::DurableFailure { .. } => {
                // A transient durable-authority failure is recorded and
                // re-kicked; it does not yet change the externally visible
                // runtime health.
                Vec::new()
            }
            ConversationObservation::DurabilityFailed {
                operation,
                diagnostic,
            } => {
                self.snapshot.durability_failure =
                    Some(super::snapshot::RuntimeDurabilityFailure {
                        operation: operation.clone(),
                        diagnostic: diagnostic.clone(),
                    });
                vec![RuntimeClientEvent::RuntimeDurabilityFailed {
                    operation,
                    diagnostic,
                }]
            }
            ConversationObservation::AdoptionEligibility(eligibility) => {
                if self.snapshot.configuration_adoption_eligibility == eligibility {
                    return Vec::new();
                }
                self.snapshot
                    .configuration_adoption_eligibility
                    .clone_from(&eligibility);
                vec![RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged { eligibility }]
            }
        }
    }

    /// Folds one durably committed publication frame into the in-flight read
    /// model and publishes its client event.
    ///
    /// The tool-call frames are model **proposals**: they update the
    /// in-flight assembly view, and the `Assembled` foreground state records
    /// exactly that — a call the model proposed, not one the Tool Plane
    /// started. Only `ToolExecutionStarted` moves a slot to `Running`.
    fn fold_publication_frame(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
    ) -> Vec<RuntimeClientEvent> {
        let Some(revision) = frame.sequence.checked_add(1) else {
            self.publication_failed(&RuntimeClientError::ProjectionExhausted);
            return Vec::new();
        };
        self.live_revision = self.live_revision.max(revision);

        match &frame.payload {
            PublicationPayload::TextSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Text);
                vec![self.content_advanced(
                    attempt_id,
                    frame,
                    *block_index,
                    suffix,
                    "text",
                    super::content::ContentRepresentation::Utf8Text,
                )]
            }
            PublicationPayload::ReasoningSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Reasoning);
                vec![self.content_advanced(
                    attempt_id,
                    frame,
                    *block_index,
                    suffix,
                    "reasoning",
                    super::content::ContentRepresentation::Utf8Text,
                )]
            }
            PublicationPayload::RefusalSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Refusal);
                vec![self.content_advanced(
                    attempt_id,
                    frame,
                    *block_index,
                    suffix,
                    "refusal",
                    super::content::ContentRepresentation::Utf8Text,
                )]
            }
            PublicationPayload::ProposedToolCallStarted { block_index, call } => {
                self.fold_proposed_tool_start(attempt_id, frame, *block_index, call)
            }
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index,
                call_id: _,
                suffix,
            } => {
                self.append_arguments(*block_index, suffix);
                vec![self.content_advanced(
                    attempt_id,
                    frame,
                    *block_index,
                    suffix,
                    "tool_call_arguments",
                    super::content::ContentRepresentation::RawJsonUtf8,
                )]
            }
            PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
                self.fold_proposed_tool_completed(attempt_id, frame, *block_index, call)
            }
            // The terminal-only frame carries the publication terminal
            // transition and no visible payload, so it publishes nothing.
            PublicationPayload::TerminalOnly => Vec::new(),
        }
    }

    fn fold_proposed_tool_start(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        call: &crate::tools::types::ToolCallStart,
    ) -> Vec<RuntimeClientEvent> {
        let message_id = frame.message_id.clone();
        let metadata_ref = self.observed_structural_content(
            attempt_id,
            frame,
            block_index,
            super::content::PublicationContentComponent::BlockMetadata,
        );
        self.ensure_attempt(attempt_id);
        let attempt = self.snapshot.attempt.as_mut().expect("attempt view exists");
        if self.live_tool_ordinal.is_none_or(|position| position < 32)
            && attempt.foreground.len() < 32
        {
            attempt.foreground.push(ForegroundToolExecution {
                committed_result: None,
                committed_result_deferred: false,
                result_source: None,
                result_metadata_ref: None,
                occurrence: super::snapshot::RuntimeClientToolOccurrence {
                    locator: super::content::CanonicalContentLocator::ByMessageId {
                        message_id: message_id.clone(),
                    },
                    block_index,
                },
                message_id: Some(message_id.clone()),
                block_index,
                call_id: super::snapshot::inline_call_id(&call.id),
                metadata_ref: metadata_ref.clone(),
                assembly_complete: false,
                tool_id: super::snapshot::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                state: ForegroundToolState::Assembled {
                    arguments: String::new(),
                },
            });
        }
        if self.live_block_ordinal.is_none_or(|position| position < 32) {
            push_in_flight_block(
                &mut attempt.in_flight,
                InFlightBlock::ToolCall {
                    block_index,
                    call_id: super::snapshot::inline_call_id(&call.id),
                    metadata_ref: metadata_ref.clone(),
                    tool_id: super::snapshot::inline_tool_id(&call.tool_id),
                    name: inline_label(&call.name),
                    arguments: String::new(),
                },
            );
        }
        vec![RuntimeClientEvent::ToolCallStarted {
            position: self
                .live_block_ordinal
                .unwrap_or(u64::from(block_index.get())),
            foreground_position: self.live_tool_ordinal.unwrap_or(0),
            attempt_id: attempt_id.clone(),
            message_id,
            block_index,
            call: super::snapshot::RuntimeClientToolCall {
                id: super::snapshot::inline_call_id(&call.id),
                tool_id: super::snapshot::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                arguments: None,
                metadata_ref,
            },
        }]
    }

    fn fold_proposed_tool_completed(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        call: &crate::tools::types::ToolCall,
    ) -> Vec<RuntimeClientEvent> {
        let message_id = frame.message_id.clone();
        let canonical_ref = self.observed_structural_content(
            attempt_id,
            frame,
            block_index,
            super::content::PublicationContentComponent::Payload,
        );
        let metadata_ref = self.observed_structural_content(
            attempt_id,
            frame,
            block_index,
            super::content::PublicationContentComponent::BlockMetadata,
        );
        let arguments_json = self.live_arguments_prefix(frame, block_index);
        let arguments_ref = self
            .live_frontiers
            .get(&(
                frame.stream_id.clone(),
                block_index,
                super::content::ContentRepresentation::RawJsonUtf8.key(),
                "live",
            ))
            .cloned();
        let canonical_length = canonical_ref
            .as_ref()
            .map(|reference| reference.readable_end);
        let arguments_ref = arguments_ref
            .filter(|reference| reference.readable_end > 0)
            .or(canonical_ref);
        let arguments_json = if arguments_ref
            .as_ref()
            .is_some_and(|reference| reference.readable_end != arguments_json.len() as u64)
        {
            String::new()
        } else {
            arguments_json
        };
        self.set_assembled(block_index);
        vec![RuntimeClientEvent::ToolCallAssembled {
            position: self
                .live_block_ordinal
                .unwrap_or(u64::from(block_index.get())),
            foreground_position: self.live_tool_ordinal.unwrap_or(0),
            arguments_json,
            arguments_ref,
            attempt_id: attempt_id.clone(),
            message_id,
            block_index,
            call: super::snapshot::RuntimeClientToolCall {
                id: super::snapshot::inline_call_id(&call.id),
                tool_id: super::snapshot::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                arguments: super::checkpoint::bounded_json_value(&call.arguments, 1024).filter(
                    |value| !value.is_null() || canonical_length.is_none_or(|length| length == 4),
                ),
                metadata_ref,
            },
        }]
    }

    fn live_arguments_prefix(
        &self,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
    ) -> String {
        self.snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| {
                attempt
                    .in_flight
                    .as_ref()
                    .and_then(|flight| {
                        flight.blocks.iter().find_map(|block| match block {
                            InFlightBlock::ToolCall {
                                block_index: index,
                                arguments,
                                ..
                            } if *index == block_index => Some(arguments.clone()),
                            _ => None,
                        })
                    })
                    .or_else(|| {
                        attempt
                            .foreground
                            .iter()
                            .find(|slot| {
                                slot.message_id.as_ref() == Some(&frame.message_id)
                                    && slot.block_index == block_index
                            })
                            .map(|slot| arguments_of(&slot.state))
                    })
            })
            .unwrap_or_default()
    }

    pub(crate) fn prepare_publication_frontier(
        &mut self,
        receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        let Some(revision) = receipt.sequence.checked_add(1) else {
            self.mark_observation_unavailable("native publication revision unavailable".into());
            return;
        };
        self.live_revision = revision;
        self.live_block_total = receipt.block_total;
        self.live_tool_total = receipt.tool_total;
        self.live_block_ordinal = receipt.block_ordinal;
        self.live_tool_ordinal = receipt.tool_ordinal;
    }
    fn current_live_cut(&self) -> Option<super::checkpoint::LivePublicationReadCut> {
        let start = self.publication_start.as_ref()?;
        Some(super::checkpoint::LivePublicationReadCut {
            source: super::content::ContentSource::LivePublicationPrefix {
                component: super::content::PublicationContentComponent::Payload,
                stream_id: start.stream_id.clone(),
                message_id: start.message_id.clone(),
                attempt_id: start.attempt_id.clone(),
                turn_id: start.turn_id.clone(),
                publication_request_id: start.request_id.clone(),
                block_index: ContentBlockIndex::new(0),
            },
            revision: self.live_revision,
            block_total: self.live_block_total,
            tool_total: self.live_tool_total,
        })
    }

    pub(crate) fn prepare_publication_content(
        &mut self,
        evidence: Vec<super::checkpoint::PreparedPublicationContent>,
    ) {
        self.prepared_publication_content = evidence;
    }
    fn observed_structural_content(
        &mut self,
        attempt: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        component: super::content::PublicationContentComponent,
    ) -> Option<super::content::ContentRef> {
        let index = self
            .prepared_publication_content
            .iter()
            .position(|evidence| {
                !evidence.suffix
                    && evidence.stream_id == frame.stream_id
                    && evidence.block_index == block_index
                    && evidence.revision == frame.sequence + 1
                    && evidence.component == component
            })?;
        let evidence = self.prepared_publication_content.remove(index);
        let start = self.publication_start.as_ref()?;
        let reference = super::content::ContentRef {
            conversation_id: self.snapshot.conversation_id.clone(),
            source: super::content::ContentSource::LivePublicationPrefix {
                component,
                stream_id: frame.stream_id.clone(),
                message_id: frame.message_id.clone(),
                attempt_id: attempt.clone(),
                turn_id: start.turn_id.clone(),
                publication_request_id: start.request_id.clone(),
                block_index,
            },
            representation: evidence.representation,
            version: 1,
            content_revision: evidence.revision,
            readable_end: evidence.readable_end,
        };
        let key = (
            frame.stream_id.clone(),
            block_index,
            reference.representation.key(),
            reference.source.key().0,
        );
        if (self.live_block_ordinal.is_none_or(|position| position < 32)
            || self.live_tool_ordinal.is_some_and(|position| position < 32))
            && (self.live_frontiers.contains_key(&key) || self.live_frontiers.len() < 192)
        {
            self.live_frontiers.insert(key, reference.clone());
        }
        Some(reference)
    }

    fn content_advanced(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        suffix: &str,
        kind: &str,
        representation: super::content::ContentRepresentation,
    ) -> RuntimeClientEvent {
        let start = self
            .publication_start
            .as_ref()
            .expect("committed frame belongs to an opened stream");
        assert_eq!(
            start.stream_id, frame.stream_id,
            "publication source identity"
        );
        let key = (
            frame.stream_id.clone(),
            block_index,
            representation.key(),
            "live",
        );
        let prepared = self
            .prepared_publication_content
            .iter()
            .position(|evidence| {
                evidence.suffix
                    && evidence.stream_id == frame.stream_id
                    && evidence.block_index == block_index
                    && evidence.revision == frame.sequence + 1
                    && evidence.representation == representation
            })
            .map(|index| self.prepared_publication_content.remove(index));
        let previous = self
            .live_frontiers
            .get(&key)
            .map_or(0, |frontier| frontier.readable_end);
        let (base_length, end_length) = match prepared {
            Some(evidence) if evidence.base_length.is_some() => {
                (evidence.base_length.unwrap(), evidence.readable_end)
            }
            Some(evidence) => (
                previous,
                previous
                    .checked_add(evidence.readable_end)
                    .expect("publication frontier"),
            ),
            None => (
                previous,
                previous
                    .checked_add(suffix.len() as u64)
                    .expect("publication frontier"),
            ),
        };
        let suffix_bytes = end_length
            .checked_sub(base_length)
            .expect("publication suffix range");
        let content_ref = super::content::ContentRef {
            conversation_id: self.snapshot.conversation_id.clone(),
            source: super::content::ContentSource::LivePublicationPrefix {
                component: crate::runtime_client::content::PublicationContentComponent::Payload,
                stream_id: frame.stream_id.clone(),
                message_id: frame.message_id.clone(),
                attempt_id: attempt_id.clone(),
                turn_id: start.turn_id.clone(),
                publication_request_id: start.request_id.clone(),
                block_index,
            },
            representation,
            version: 1,
            content_revision: frame
                .sequence
                .checked_add(1)
                .expect("frame revision overflow"),
            readable_end: end_length,
        };
        if (self.live_block_ordinal.is_none_or(|position| position < 32)
            || self.live_tool_ordinal.is_some_and(|position| position < 32))
            && (self.live_frontiers.contains_key(&key) || self.live_frontiers.len() < 192)
        {
            self.live_frontiers.insert(key, content_ref.clone());
        }
        RuntimeClientEvent::BlockContentAdvanced {
            position: self
                .live_block_ordinal
                .unwrap_or(u64::from(block_index.get())),
            attempt_id: attempt_id.clone(),
            message_id: frame.message_id.clone(),
            block_index,
            kind: kind.into(),
            base_length,
            end_length,
            content_revision: content_ref.content_revision,
            inline_delta: (suffix_bytes == suffix.len() as u64
                && suffix.len() <= super::checkpoint::CHECKPOINT_ITEM_BYTES)
                .then(|| suffix.to_owned()),
            content_ref,
        }
    }

    pub(crate) fn apply_manual_compaction(
        &mut self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        let Some(batch) = batch else {
            self.mark_observation_unavailable("native manual compaction source unavailable".into());
            return;
        };
        let (crate::durable::observation::NativeObservationTransition::Single(witness)
        | crate::durable::observation::NativeObservationTransition::Compaction(witness)) =
            &batch.transition
        else {
            self.mark_observation_unavailable(
                "invalid native manual compaction source family".into(),
            );
            return;
        };
        self.snapshot.context.manual_compaction =
            Some(super::snapshot::RuntimeClientManualCompaction {
                request_id: request.cloned(),
                released: witness.event_kind != "compaction_started",
                error: (witness.event_kind == "compaction_failed")
                    .then(|| {
                        witness
                            .value
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .map(bounded_diagnostic)
                    })
                    .flatten(),
            });
        self.apply_native_batch(batch);
    }

    /// Installs only finite source-owned receipts. Source I/O and original body
    /// materialization happened at the native write owner, before this boundary.
    #[allow(clippy::too_many_lines)] // Fold every closed native family before one shared cut/publication update.
    pub(crate) fn apply_native_batch(
        &mut self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) {
        use crate::durable::observation::NativeObservationTransition;
        self.observe_released_journal_prefix(batch.ready_before);
        let mut observations = Vec::with_capacity(34);
        self.install_native_trace_total(batch.trace_total);
        let model_turn = matches!(
            batch.transition,
            NativeObservationTransition::ModelTurnSettlement { .. }
        );
        if !model_turn {
            if let Some(pending) = &batch.pending {
                self.apply_pending_receipt(Some(pending));
            }
            self.prepare_native_canonical(batch, &mut observations);
        }
        match &batch.transition {
            NativeObservationTransition::Single(value)
            | NativeObservationTransition::Adoption(value)
            | NativeObservationTransition::Compaction(value) => {
                self.prepare_native_witness(batch, value, &mut observations);
            }
            NativeObservationTransition::RequestStart {
                started,
                contributions,
                statuses,
                ..
            } => {
                self.prepare_native_witness(batch, started, &mut observations);
                for contribution in contributions {
                    self.prepare_native_witness(batch, contribution, &mut observations);
                }
                observations.extend(
                    statuses
                        .iter()
                        .cloned()
                        .map(ConversationObservation::Status),
                );
            }
            NativeObservationTransition::WorkflowTerminal { terminal, output } => {
                self.prepare_native_witness(batch, terminal, &mut observations);
                self.prepare_native_witness(batch, output, &mut observations);
            }
            NativeObservationTransition::ModelTurnSettlement {
                provider,
                canonical,
                assistant,
                boundary,
            } => {
                use crate::durable::observation::ModelTurnObservationBoundary;
                self.prepare_native_witness(batch, provider, &mut observations);
                self.prepare_native_canonical_headers(
                    assistant,
                    canonical.attempt_id.clone(),
                    &mut observations,
                );
                self.prepare_native_witness(batch, canonical, &mut observations);
                match boundary {
                    Some(ModelTurnObservationBoundary::Completed { turn, attempt }) => {
                        self.prepare_native_witness(batch, turn, &mut observations);
                        self.prepare_native_witness(batch, attempt, &mut observations);
                    }
                    Some(ModelTurnObservationBoundary::Adopted {
                        turn,
                        adoption,
                        canonical,
                    }) => {
                        self.prepare_native_witness(batch, turn, &mut observations);
                        if let Some(pending) = &batch.pending {
                            observations
                                .push(ConversationObservation::PendingReceipt(pending.clone()));
                        }
                        self.prepare_native_canonical_headers(
                            canonical,
                            adoption.attempt_id.clone(),
                            &mut observations,
                        );
                        self.prepare_native_witness(batch, adoption, &mut observations);
                    }
                    None => {}
                }
            }
            NativeObservationTransition::ToolResults { loaded_results, .. } => {
                observations.extend(
                    loaded_results.iter().cloned().map(|tool| {
                        ConversationObservation::NativeToolResultCommit(Box::new(tool))
                    }),
                );
            }
        }
        if let Some((first, last)) = batch.journal_range {
            self.native_journal_highwater = self.native_journal_highwater.max(last);
            // Native owners may finish their independent callbacks out of J
            // order. Their bounded working heads can install immediately, but
            // only the contiguous prefix authorizes historical source bytes.
            if first <= self.journal_through.saturating_add(1) {
                self.journal_through = self.journal_through.max(last);
                self.read_through = self.journal_through;
            }
        }
        self.record_native_cut(batch.read_cut.clone());
        self.source_state.source_unavailable = false;
        self.publication_state.installing_native_batch = true;
        self.apply(ConversationObservation::ObservationBatch(observations));
        self.publication_state.installing_native_batch = false;
        self.observe_released_transcript_prefix(batch.ready_t_through, &batch.read_cut);
        if batch.source_unavailable || batch.ready_before.is_none() {
            self.mark_observation_unavailable("native Journal display family unavailable".into());
        }
    }

    fn prepare_native_witness(
        &mut self,
        batch: &crate::durable::observation::NativeObservationBatch,
        witness: &crate::durable::observation::ObservationEventWitness,
        observations: &mut Vec<ConversationObservation>,
    ) {
        let field = match witness.event_kind.as_str() {
            "tool_execution_completed" => Some("result"),
            "tool_execution_failed" => Some("error"),
            _ => None,
        }
        .and_then(|field| {
            witness
                .fields
                .iter()
                .find(|entry| entry.path == ["event", field])
                .map(|entry| super::snapshot::JournalFieldRef {
                    sequence: witness.sequence,
                    path: entry.path.clone(),
                    encoded_bytes: entry.encoded_bytes,
                })
        });
        self.prepare_journal_fact(witness.sequence, batch.failure.clone(), field);
        self.prepared_outcomes.insert(
            witness.sequence,
            (
                batch.failure_deferred,
                batch
                    .failure_sequence
                    .filter(|_| batch.failure_deferred && batch.failure_readable_end > 0)
                    .map(|sequence| super::snapshot::JournalFieldRef {
                        sequence,
                        path: vec!["normalized_failure".into()],
                        encoded_bytes: batch.failure_readable_end,
                    }),
                batch.finish_reason_deferred,
                batch
                    .finish_reason
                    .as_ref()
                    .map(|source| super::snapshot::JournalFieldRef {
                        sequence: source.sequence,
                        path: source.path.clone(),
                        encoded_bytes: source.encoded_bytes,
                    }),
            ),
        );
        observations.push(ConversationObservation::JournalFact(witness.clone()));
    }

    fn prepare_native_canonical(
        &mut self,
        batch: &crate::durable::observation::NativeObservationBatch,
        observations: &mut Vec<ConversationObservation>,
    ) {
        use crate::durable::observation::NativeObservationTransition;
        if let Some(canonical) = &batch.canonical {
            let attempt_id = match &batch.transition {
                NativeObservationTransition::Single(witness)
                | NativeObservationTransition::Adoption(witness)
                | NativeObservationTransition::Compaction(witness) => witness.attempt_id.clone(),
                NativeObservationTransition::RequestStart { started, .. } => {
                    started.attempt_id.clone()
                }
                _ => None,
            };
            self.prepare_native_canonical_headers(canonical, attempt_id, observations);
        }
    }

    fn prepare_native_canonical_headers(
        &mut self,
        canonical: &Arc<crate::durable::inbox::CanonicalObservationBatch>,
        attempt_id: Option<AttemptId>,
        observations: &mut Vec<ConversationObservation>,
    ) {
        self.install_content_metadata(
            canonical
                .head
                .iter()
                .flat_map(|header| {
                    header
                        .content
                        .iter()
                        .map(|binding| binding.content_ref.clone())
                })
                .collect(),
        );
        self.prepare_message_headers(
            canonical
                .head
                .iter()
                .filter_map(|header| {
                    header.identity.as_ref().map(|identity| {
                        (
                            identity.clone(),
                            header.clone(),
                            Some(canonical.read_cut.surface_revision),
                        )
                    })
                })
                .collect(),
        );
        observations.push(ConversationObservation::CanonicalCommitted {
            attempt_id,
            receipt: crate::durable::inbox::TranscriptCommitReceipt {
                transcript_cursor: canonical.last_transcript,
                ledger_position: canonical.last_position,
                ordinal: canonical.count.saturating_sub(1),
                observation: canonical.clone(),
            },
        });
    }

    fn install_native_trace_total(&mut self, total: Option<u64>) {
        let Some(total) = total else {
            self.mark_observation_unavailable("native Trace count source unavailable".into());
            return;
        };
        let loaded = self.snapshot.trace.records.len() as u64;
        let previous = self
            .source_frontiers
            .get("trace.records")
            .map_or(0, |frontier| frontier.total);
        let total = total.max(previous);
        let mut frontier = super::trace::checkpoint_frontier(
            total,
            loaded,
            self.snapshot.trace.next_cursor.as_ref(),
        );
        if loaded < total {
            frontier.next_offset = Some(0);
        }
        self.source_frontiers
            .insert("trace.records".into(), frontier);
    }

    pub(crate) fn prepare_journal_fact(
        &mut self,
        sequence: u64,
        failure: Option<RuntimeClientAttemptFailure>,
        result: Option<super::snapshot::JournalFieldRef>,
    ) {
        self.prepared_journal.insert(sequence, (failure, result));
    }

    fn fold_journal_fact(
        &mut self,
        witness: crate::durable::observation::ObservationEventWitness,
    ) -> Vec<RuntimeClientEvent> {
        if witness.source_unavailable {
            self.mark_observation_unavailable("native Journal display family unavailable".into());
        }
        if !self.publication_state.installing_native_batch && witness.native_direct {
            self.prepared_journal.remove(&witness.sequence);
            self.prepared_outcomes.remove(&witness.sequence);
            return Vec::new();
        }
        self.install_native_trace_total(witness.trace_total);
        let outcome_source = self
            .prepared_outcomes
            .remove(&witness.sequence)
            .unwrap_or_default();
        if let Some(cursor) = witness.transcript_cursor {
            self.observe_transcript_position(cursor);
        }
        let prepared = self
            .prepared_journal
            .remove(&witness.sequence)
            .unwrap_or_default();
        if let Some(reading) = &witness.occupancy {
            return self.fold_compaction_reading(witness.attempt_id.as_ref(), reading);
        }
        if witness.event_kind == "compaction_completed" {
            self.mark_observation_unavailable(
                "native compaction reading source unavailable".into(),
            );
            return Vec::new();
        }
        let Some(attempt) = witness.attempt_id.as_ref() else {
            if let Ok(event) = super::structural_json::decode::<RuntimeEvent>(witness.value)
                && matches!(
                    event,
                    RuntimeEvent::CompactionStarted { .. }
                        | RuntimeEvent::CompactionCompleted { .. }
                        | RuntimeEvent::CompactionFailed { .. }
                )
            {
                return self.fold_compaction_event(None, &event);
            }
            return Vec::new();
        };
        self.tool_occurrence.clone_from(&witness.tool_occurrence);
        if witness.event_kind == "attempt_failed" {
            return prepared.0.map_or_else(Vec::new, |error| {
                self.settle_attempt(
                    attempt,
                    RuntimeClientOutcome::Failed {
                        error,
                        error_deferred: outcome_source.0,
                        error_source: outcome_source.1,
                    },
                )
            });
        }
        if matches!(
            witness.event_kind.as_str(),
            "tool_execution_started" | "tool_execution_completed" | "tool_execution_failed"
        ) {
            return self.fold_tool_witness(attempt, &witness, prepared.1);
        }
        if let Ok(event) = super::structural_json::decode::<RuntimeEvent>(witness.value) {
            if matches!(
                event,
                RuntimeEvent::CompactionStarted { .. }
                    | RuntimeEvent::CompactionCompleted { .. }
                    | RuntimeEvent::CompactionFailed { .. }
            ) {
                return self.fold_compaction_event(Some(attempt), &event);
            }
            if let RuntimeEvent::AttemptCompleted { finish_reason, .. } = event {
                return self.settle_attempt(
                    attempt,
                    RuntimeClientOutcome::Completed {
                        finish_reason,
                        finish_reason_deferred: outcome_source.2,
                        finish_reason_source: outcome_source.3,
                    },
                );
            }
            self.fold_event(attempt, &event)
        } else {
            Vec::new()
        }
    }

    fn start_tool_witness(
        &mut self,
        attempt: &AttemptId,
        occurrence: Option<&super::snapshot::RuntimeClientToolOccurrence>,
    ) {
        if let Some(occurrence) = occurrence {
            update_stored_tool_slots(&mut self.snapshot, occurrence, |slot| {
                if matches!(slot.state, ForegroundToolState::Assembled { .. }) {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
            });
        }
        if let Some(slot) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|current| current.attempt_id.as_ref() == Some(attempt))
            .and_then(|current| {
                current
                    .foreground
                    .iter_mut()
                    .find(|slot| Some(&slot.occurrence) == occurrence)
            })
            && matches!(slot.state, ForegroundToolState::Assembled { .. })
        {
            slot.state = ForegroundToolState::Running {
                arguments: arguments_of(&slot.state),
                progress: None,
            };
        }
    }

    fn fold_tool_witness(
        &mut self,
        attempt: &AttemptId,
        witness: &crate::durable::observation::ObservationEventWitness,
        source: Option<super::snapshot::JournalFieldRef>,
    ) -> Vec<RuntimeClientEvent> {
        let value = &witness.value;
        let tool_call_id = value
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| id.len() <= 256)
            .map(ToolCallId::new);
        let tool_id = value
            .get("tool_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| id.len() <= 256)
            .map(ToolId::new);
        let occurrence = self.tool_display_occurrence();
        let metadata_ref = self.tool_display_metadata();
        if witness.event_kind == "tool_execution_started" {
            self.start_tool_witness(attempt, occurrence.as_ref());
            return vec![RuntimeClientEvent::ToolExecutionStarted {
                attempt_id: attempt.clone(),
                occurrence,
                metadata_ref,
                tool_call_id,
                tool_id,
            }];
        }
        let Some(mut result) = tool_witness_result(witness) else {
            return Vec::new();
        };
        let mut result_metadata_ref = None;
        let result_source = source;
        if let Some(slot) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|current| current.attempt_id.as_ref() == Some(attempt))
            .and_then(|current| {
                current
                    .foreground
                    .iter_mut()
                    .find(|slot| Some(&slot.occurrence) == occurrence.as_ref())
            })
        {
            if matches!(slot.state, ForegroundToolState::Settled { .. }) {
                return Vec::new();
            }
            if let Some(committed) = &slot.committed_result {
                result.content.clone_from(&committed.content);
                result.artifacts.clone_from(&committed.artifacts);
                result.deliveries.clone_from(&committed.deliveries);
                result.workflow.clone_from(&committed.workflow);
                result.managed_output.clone_from(&committed.managed_output);
                result_metadata_ref.clone_from(&slot.result_metadata_ref);
            }
            slot.state = ForegroundToolState::Settled {
                arguments: arguments_of(&slot.state),
                result: result.clone(),
            };
            slot.result_source.clone_from(&result_source);
        }
        if let Some(occurrence) = occurrence.as_ref() {
            update_stored_tool_slots(&mut self.snapshot, occurrence, |slot| {
                let mut settled = result.clone();
                if let Some(committed) = &slot.committed_result {
                    settled.content.clone_from(&committed.content);
                    settled.artifacts.clone_from(&committed.artifacts);
                    settled.deliveries.clone_from(&committed.deliveries);
                    settled.workflow.clone_from(&committed.workflow);
                    settled.managed_output.clone_from(&committed.managed_output);
                }
                super::tool_observation::defer_result(&mut settled);
                slot.state = ForegroundToolState::Settled {
                    arguments: arguments_of(&slot.state),
                    result: settled,
                };
                slot.result_source.clone_from(&result_source);
            });
        }
        vec![RuntimeClientEvent::ToolExecutionSettled {
            attempt_id: attempt.clone(),
            occurrence,
            metadata_ref,
            tool_call_id,
            tool_id,
            result,
            result_source,
            result_metadata_ref,
        }]
    }

    /// The explicit `RuntimeEvent` mapping policy of the Runtime Client
    /// protocol.
    ///
    /// Classification (see the module documentation):
    ///
    /// - PROJECT: attempt lifecycle/settlement, foreground tool lifecycle,
    ///   and progress. Streaming assistant output and tool-call assembly are
    ///   no longer `RuntimeEvent` facts at all: they arrive as durably
    ///   committed publication frames (Issue #108) and fold through
    ///   [`RuntimeClientProjection::fold_publication_frame`];
    /// - PROJECT: turn counting and final request usage, carrying the exact
    ///   values folded into the attempt view;
    /// - TRACE: request boundaries and retries remain internal; `JournalBatch`
    ///   independently invalidates the represented durable Trace cut;
    /// - PROJECT: compaction start/failure and committed completion, carrying
    ///   attempt attribution when automatic and no attempt identity when
    ///   manual.
    // The mapping table is one explicit classification policy; identical
    // `Vec::new()` bodies mark intentionally distinct classes (the remaining
    // fold-only/internal observations) that must remain separately
    // documented.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn fold_event(
        &mut self,
        attempt_id: &AttemptId,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::AttemptStarted { .. } => {
                let model = self.frozen_attempt_model(attempt_id);
                let execution_settings = self.frozen_execution_settings(attempt_id);
                self.snapshot.attempt = Some(RuntimeClientAttempt {
                    attempt_id: Some(attempt_id.clone()),
                    attempt_id_source: None,
                    phase: RuntimeClientAttemptPhase::Running,
                    turn: 0,
                    last_usage: None,
                    in_flight: None,
                    foreground: Vec::new(),
                    model: model.clone(),
                    execution_settings: execution_settings.clone(),
                });
                // The Agent Status window is deliberately untouched: a status
                // composed by an earlier attempt is a historical fact of this
                // conversation, not attempt-scoped live state, so a new
                // attempt neither retracts nor relocates it.
                //
                // The published event carries the same frozen model the
                // attempt read model carries, so an incremental subscriber
                // and a snapshot reader agree without inference.
                vec![RuntimeClientEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                    model,
                    execution_settings,
                }]
            }
            RuntimeEvent::AttemptCompleted { finish_reason, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Completed {
                    finish_reason_deferred: false,
                    finish_reason_source: None,
                    finish_reason: finish_reason.clone(),
                },
            ),
            RuntimeEvent::AttemptCancelled { reason, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Cancelled { reason: *reason },
            ),
            RuntimeEvent::AttemptTimedOut { .. } => {
                self.settle_attempt(attempt_id, RuntimeClientOutcome::TimedOut)
            }
            RuntimeEvent::AttemptLimitExceeded { limit, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::LimitExceeded { limit: *limit },
            ),
            RuntimeEvent::AttemptFailed { error, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Failed {
                    error_deferred: false,
                    error_source: None,
                    error: client_failure(error),
                },
            ),
            RuntimeEvent::TurnStarted => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                {
                    attempt.turn = attempt.turn.saturating_add(1);
                    return vec![RuntimeClientEvent::AttemptTurnUpdated {
                        attempt_id: attempt_id.clone(),
                        turn: attempt.turn,
                    }];
                }
                Vec::new()
            }
            // Request mechanics are private. The JournalBatch owner publishes
            // one independent Trace invalidation for the represented cut.
            RuntimeEvent::TurnCompleted
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::ModelRetryScheduled { .. } => Vec::new(),
            RuntimeEvent::ContextContributionEmitted { .. } => Vec::new(),
            RuntimeEvent::ModelRequestCompleted { usage, .. } => {
                if let Some(usage) = usage
                    && let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                {
                    attempt.last_usage = Some(usage.clone());
                    return vec![RuntimeClientEvent::AttemptUsageUpdated {
                        attempt_id: attempt_id.clone(),
                        usage: usage.clone(),
                    }];
                }
                Vec::new()
            }
            // The durable Journal event carries identity only; the projection
            // already receives the canonical body from the commit
            // observation and never copies body content into the Journal.
            RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. } => Vec::new(),
            // The adopted turn reaches a client as the committed `UserMessage`
            // it names. The obligation itself is durable recovery authority,
            // never a client-facing execution fact.
            RuntimeEvent::InboundTurnAdopted { .. } => Vec::new(),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                    && let Some(slot) = foreground_slot_for_event(
                        &mut attempt.foreground,
                        tool_call_id,
                        self.tool_occurrence.as_ref(),
                    )
                    && matches!(&slot.state, ForegroundToolState::Assembled { .. })
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionStarted {
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: super::snapshot::inline_call_id(tool_call_id),
                    tool_id: super::snapshot::inline_tool_id(tool_id),
                }]
            }
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                tool_id,
                execution_id,
                progress,
            } => {
                if execution_id.is_some() {
                    return Vec::new();
                }
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                    && let Some(slot) = foreground_slot_for_event(
                        &mut attempt.foreground,
                        tool_call_id,
                        self.tool_occurrence.as_ref(),
                    )
                    && let ForegroundToolState::Running { arguments, .. } = &slot.state
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionProgress {
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: super::snapshot::inline_call_id(tool_call_id),
                    tool_id: super::snapshot::inline_tool_id(tool_id),
                    execution_id: execution_id.clone(),
                    progress: progress.clone(),
                }]
            }
            // A deadline fact is durable liveness *intent* evidence only
            // (Issue #204): the terminal `ToolExecutionCompleted` carries the
            // client-facing settlement, so the projection has nothing to add.
            // The cancellation-request, settlement-observation, and
            // settlement-control-plane-failure facts are the same kind of
            // durable evidence.
            RuntimeEvent::ToolExecutionDeadlineFired { .. }
            | RuntimeEvent::ToolExecutionCancellationRequested { .. }
            | RuntimeEvent::ToolExecutionSettlementObserved { .. }
            | RuntimeEvent::ToolExecutionSettlementControlFailed { .. } => Vec::new(),
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } => {
                if self.settle_foreground(attempt_id, tool_call_id, None, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    result_source: None,
                    result_metadata_ref: None,
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: super::snapshot::inline_call_id(tool_call_id),
                    tool_id: super::snapshot::inline_tool_id(tool_id),
                    result: result.clone(),
                }]
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                error,
            } => {
                let result = ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Failed {
                        error: error.clone(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                };
                if self.settle_foreground(attempt_id, tool_call_id, None, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    result_source: None,
                    result_metadata_ref: None,
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: super::snapshot::inline_call_id(tool_call_id),
                    tool_id: super::snapshot::inline_tool_id(tool_id),
                    result,
                }]
            }
            RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::CompactionFailed { .. } => {
                self.fold_compaction_event(Some(attempt_id), event)
            }
            // The background/subagent ownership/terminal publication facts
            // are durable execution facts; the client projection learns the
            // resulting snapshot and inbound message through its native
            // background/subagent/mailbox/message projections.
            RuntimeEvent::BackgroundExecutionCommitted { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::AgentActivationAdmission { .. }
            | RuntimeEvent::SubagentOwnershipCommitted { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::SubagentPhysicalSettlementProven { .. } => Vec::new(),
            // The interaction requested/settled facts are durable audit
            // evidence (Issue #109). The client already learns the live
            // pending/settled transitions from the coordinator's own
            // observation seam, so replaying the Journal fact here would
            // publish the same prompt twice under two different vocabularies.
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. } => {
                Vec::new()
            }
            // Goal journal facts are audit-only. The authoritative bounded
            // view enters through GoalChanged, never replay.
            RuntimeEvent::Goal { .. } => Vec::new(),
            // Workflow lifecycle/join facts are best-effort observability;
            // the successful child value and native terminal pair are the
            // separate durable authority. The Runtime Client continues to
            // project the bounded parent Tool call/result and does not expose
            // workflow-local values or child transcripts as a second
            // conversation surface.
            RuntimeEvent::WorkflowStarted { .. }
            | RuntimeEvent::WorkflowBlockStarted { .. }
            | RuntimeEvent::WorkflowBlockSettled { .. }
            | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
            | RuntimeEvent::WorkflowLoopIterationSettled { .. }
            | RuntimeEvent::WorkflowLoopExited { .. }
            | RuntimeEvent::NativeToolInvocation { .. }
            | RuntimeEvent::WorkflowNodeStarted { .. }
            | RuntimeEvent::WorkflowNodeSettled { .. }
            | RuntimeEvent::WorkflowAgentAdmitted { .. }
            | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            | RuntimeEvent::WorkflowBranchSelected { .. }
            | RuntimeEvent::WorkflowParallelAdmitted { .. }
            | RuntimeEvent::WorkflowParallelSettled { .. }
            | RuntimeEvent::WorkflowCompleted { .. }
            | RuntimeEvent::WorkflowFailed { .. }
            | RuntimeEvent::WorkflowCancelled { .. } => Vec::new(),
        }
    }

    fn tool_display_occurrence(&self) -> Option<super::snapshot::RuntimeClientToolOccurrence> {
        let native = self.tool_occurrence.as_ref()?;
        Some(super::snapshot::RuntimeClientToolOccurrence {
            locator: super::content::CanonicalContentLocator::ByLedgerPosition {
                position: native.ledger_position,
            },
            block_index: native.block_index,
        })
    }
    fn tool_display_metadata(&self) -> Option<super::content::ContentRef> {
        let occurrence = self.tool_display_occurrence()?;
        self.snapshot
            .attempt
            .as_ref()?
            .foreground
            .iter()
            .find(|slot| slot.occurrence == occurrence)
            .and_then(|slot| slot.metadata_ref.clone())
    }

    /// Folds the shared automatic/manual compaction lifecycle. Manual
    /// maintenance carries no attempt identity; both paths update the exact
    /// same context read model and publish the same client vocabulary.
    fn fold_compaction_event(
        &mut self,
        attempt_id: Option<&AttemptId>,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::CompactionStarted { .. } => {
                self.snapshot.context.compaction_error = None;
                self.snapshot.context.compaction_in_progress = true;
                vec![RuntimeClientEvent::ContextCompactionStarted {
                    context: self.snapshot.context.clone(),
                    attempt_id: attempt_id.cloned(),
                }]
            }
            RuntimeEvent::CompactionFailed { error, .. } => {
                self.snapshot.context.compaction_error = Some(error.clone());
                self.snapshot.context.compaction_in_progress = false;
                vec![RuntimeClientEvent::ContextCompactionFailed {
                    context: self.snapshot.context.clone(),
                    attempt_id: attempt_id.cloned(),
                    error: error.clone(),
                }]
            }
            #[cfg(test)]
            RuntimeEvent::CompactionCompleted {
                generation,
                checkpoint_id,
                activation_id,
                tokens_before,
                estimated_tokens_after,
                occupancy,
                ..
            } => self.fold_compaction_completed(
                attempt_id,
                &RuntimeClientCompactionView {
                    generation: *generation,
                    checkpoint_id: checkpoint_id.clone(),
                    activation_id: *activation_id,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                },
                occupancy
                    .as_ref()
                    .map(|value| {
                        super::occupancy::RuntimeClientContextOccupancy::from_native(value, None)
                    })
                    .as_ref(),
            ),
            _ => unreachable!("only compaction events enter the compaction fold"),
        }
    }

    fn fold_compaction_reading(
        &mut self,
        attempt_id: Option<&AttemptId>,
        reading: &crate::durable::observation::OccupancyObservation,
    ) -> Vec<RuntimeClientEvent> {
        // The original completion owns both the generation and its eligible
        // projection head. A later asynchronous read may refine other domains,
        // but cannot be required to make this published completion coherent.
        self.snapshot.context.projection_head = Some(reading.projection_head.clone());
        self.fold_compaction_completed(attempt_id, &reading.compaction, reading.value.as_ref())
    }

    fn fold_compaction_completed(
        &mut self,
        attempt_id: Option<&AttemptId>,
        compaction: &RuntimeClientCompactionView,
        occupancy: Option<&super::occupancy::RuntimeClientContextOccupancy>,
    ) -> Vec<RuntimeClientEvent> {
        self.snapshot.context.compaction_in_progress = false;
        self.snapshot.context.occupancy = occupancy.cloned();
        self.snapshot.context.compaction_count = self
            .snapshot
            .context
            .compaction_count
            .checked_add(1)
            .expect("Runtime Client compaction count cannot overflow");
        self.snapshot.context.latest_compaction = Some(compaction.clone());
        vec![RuntimeClientEvent::ContextCompacted {
            attempt_id: attempt_id.cloned(),
            context: self.snapshot.context.clone(),
        }]
    }

    /// Folds one terminal attempt settlement: the attempt view settles and
    /// exactly one terminal client event is published.
    fn settle_attempt(
        &mut self,
        attempt_id: &AttemptId,
        outcome: RuntimeClientOutcome,
    ) -> Vec<RuntimeClientEvent> {
        if let Some(attempt) = &mut self.snapshot.attempt
            && attempt.attempt_id.as_ref() == Some(attempt_id)
        {
            attempt.phase = RuntimeClientAttemptPhase::Settled {
                outcome: outcome.clone(),
            };
            self.snapshot.context.compaction_in_progress = false;
        }
        vec![RuntimeClientEvent::AttemptSettled {
            attempt_id: attempt_id.clone(),
            outcome,
        }]
    }

    /// Ensures the attempt view exists with a running phase (used by unit
    /// tests applying raw event sequences without prior admission).
    fn ensure_attempt(&mut self, attempt_id: &AttemptId) {
        if self
            .snapshot
            .attempt
            .as_ref()
            .is_none_or(|attempt| attempt.attempt_id.as_ref() != Some(attempt_id))
        {
            let model = self.frozen_attempt_model(attempt_id);
            self.snapshot.attempt = Some(RuntimeClientAttempt {
                attempt_id: Some(attempt_id.clone()),
                attempt_id_source: None,
                phase: RuntimeClientAttemptPhase::Running,
                turn: 0,
                last_usage: None,
                in_flight: None,
                foreground: Vec::new(),
                model,
                execution_settings: None,
            });
        }
    }

    /// The model view an attempt froze at admission.
    ///
    /// Rebuilding the attempt view (for example when the loop's
    /// `AttemptStarted` arrives after admission) must never lose the frozen
    /// model: while the same attempt is described, the already-frozen view
    /// wins over live session state.
    fn frozen_execution_settings(
        &self,
        attempt_id: &AttemptId,
    ) -> Option<super::settings::AdmittedSettings> {
        self.snapshot
            .attempt
            .as_ref()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
            .and_then(|attempt| attempt.execution_settings.clone())
    }

    fn frozen_attempt_model(&self, attempt_id: &AttemptId) -> Option<Box<AttemptModelView>> {
        match self
            .snapshot
            .attempt
            .as_ref()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
        {
            Some(attempt) => attempt.model.clone(),
            None => None,
        }
    }

    fn install_todo_head(&mut self, todos: &crate::durable::inbox::NativeTodoObservation) {
        self.snapshot.todos = Some(todos.snapshot.clone());
        self.source_frontiers.insert(
            "todos.tasks".into(),
            super::checkpoint::CollectionFrontier {
                domain: "todos.tasks".into(),
                revision: todos.source_ledger_position,
                total: todos.total,
                loaded: todos.loaded,
                next_offset: todos.next_offset,
            },
        );
    }

    fn fold_canonical_receipt(
        &mut self,
        attempt_id: Option<&AttemptId>,
        receipt: crate::durable::inbox::TranscriptCommitReceipt,
    ) -> Vec<RuntimeClientEvent> {
        if receipt.ordinal.checked_add(1) != Some(receipt.observation.count) {
            return Vec::new();
        }
        let batch = receipt.observation;
        self.observe_released_journal_prefix(batch.ready_through);
        self.observe_released_transcript_prefix(batch.ready_t_through, &batch.read_cut);
        self.native_canonical_highwater = self
            .native_canonical_highwater
            .max(batch.read_cut.ledger_through);
        if let (Some(first), Some(last)) = (batch.first_transcript, batch.last_transcript) {
            if last
                .get()
                .checked_sub(first.get())
                .and_then(|span| span.checked_add(1))
                == Some(batch.transcript_count)
            {
                self.observe_transcript_range(first.get(), last.get());
            } else {
                // The receipt cannot authorize interleaved non-canonical T.
                for header in &batch.head {
                    if canonical_header_position(header).is_some_and(|position| {
                        position >= batch.first_position && position <= batch.last_position
                    }) && let Some(cursor) = header.transcript_cursor
                    {
                        self.observe_transcript_position(cursor);
                    }
                }
            }
        }
        let mut events = Vec::new();
        for header in &batch.head {
            if canonical_header_position(header).is_some_and(|position| {
                position >= batch.first_position && position <= batch.last_position
            }) {
                if batch.read_cut.ledger_through > self.canonical_observed_through {
                    self.install_canonical_tool_proposals(attempt_id, header, &mut events);
                }
                self.rebind_canonical_foreground(header, &mut events);
            }
        }
        if batch.read_cut.ledger_through <= self.canonical_observed_through {
            return events;
        }
        self.canonical_observed_through = batch.read_cut.ledger_through;
        self.source_state.source_layout_failure =
            batch.source_unavailable || batch.ready_through.is_none();
        self.source_state.source_unavailable = self.source_state.source_layout_failure;
        if batch.source_unavailable {
            self.mark_observation_unavailable("native canonical display source unavailable".into());
        }
        self.record_native_cut(batch.read_cut.clone());
        for header in &batch.head {
            let cursor = header.transcript_cursor;
            let owns_header = canonical_header_position(header).is_some_and(|position| {
                position >= batch.first_position && position <= batch.last_position
            });
            events.extend(self.fold_canonical_header(
                attempt_id.cloned(),
                header.clone(),
                cursor,
                owns_header,
            ));
        }
        if let Some(todos) = &batch.todos
            && self.snapshot.todos.is_some()
        {
            self.install_todo_head(todos);
            events.push(RuntimeClientEvent::ReadDomainsUpdated {
                projection_head: self.snapshot.context.projection_head.clone(),
                transcript: self.snapshot.transcript.clone(),
                trace: None,
                occupancy: self.snapshot.context.occupancy.clone(),
                todos: self.snapshot.todos.clone(),
            });
        }
        events
    }

    fn retain_descriptor_execution(
        &self,
        descriptor: &mut super::snapshot::RuntimeClientMessageDescriptor,
    ) {
        if let Some(existing) = self
            .snapshot
            .message_descriptors
            .iter()
            .find(|existing| existing.locator == descriptor.locator)
        {
            for slot in &mut descriptor.tool_calls {
                if let Some(current) = existing
                    .tool_calls
                    .iter()
                    .find(|current| current.occurrence == slot.occurrence)
                {
                    retain_tool_execution(slot, current);
                }
            }
        }
    }

    fn canonical_header_unchanged(
        &self,
        header: &crate::durable::observation::MessageHeader,
        descriptor: &super::snapshot::RuntimeClientMessageDescriptor,
    ) -> bool {
        self.snapshot
            .message_descriptors
            .iter()
            .any(|current| current == descriptor)
            && match &header.display {
                Some(message) => {
                    self.snapshot
                        .messages
                        .iter()
                        .any(|current| current == message)
                        && self.head_bindings.get(message.id()) == Some(&header.content)
                }
                None => !self
                    .snapshot
                    .messages
                    .iter()
                    .any(|current| header.identity.as_ref() == Some(current.id())),
            }
    }

    fn install_canonical_descriptor(
        &mut self,
        descriptor: &super::snapshot::RuntimeClientMessageDescriptor,
    ) -> Option<MessageId> {
        if let Some(existing) = self
            .snapshot
            .message_descriptors
            .iter_mut()
            .find(|existing| existing.locator == descriptor.locator)
        {
            existing.clone_from(descriptor);
        } else {
            self.snapshot.message_descriptors.push(descriptor.clone());
        }
        self.snapshot
            .message_descriptors
            .sort_by_key(|descriptor| canonical_locator_position(&descriptor.locator));
        let evicted = if self.snapshot.message_descriptors.len()
            > super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
        {
            self.snapshot.message_descriptors.remove(0).identity
        } else {
            None
        };
        if let Some(identity) = &evicted {
            self.snapshot
                .messages
                .retain(|message| message.id() != identity);
            self.head_bindings.remove(identity);
        }
        evicted
    }

    fn fold_canonical_header(
        &mut self,
        attempt_id: Option<AttemptId>,
        header: crate::durable::observation::MessageHeader,
        transcript_cursor: Option<crate::durable::TranscriptCursor>,
        owns_header: bool,
    ) -> Vec<RuntimeClientEvent> {
        let mut events = Vec::new();
        let Some(mut descriptor) =
            super::snapshot::RuntimeClientMessageDescriptor::from_header(&header)
        else {
            return events;
        };
        self.retain_descriptor_execution(&mut descriptor);
        // The complete source head may include already installed earlier C rows.
        // Only exact unchanged evidence is redundant; this transaction's own
        // committed rows and every body/Tool/source refinement remain facts.
        if !owns_header && self.canonical_header_unchanged(&header, &descriptor) {
            return events;
        }
        let evicted_message_id = self.install_canonical_descriptor(&descriptor);
        let transcript_cursor = transcript_cursor.map(RuntimeClientTranscriptCursor::from);
        events.push(RuntimeClientEvent::MessageDescriptorCommitted {
            attempt_id: attempt_id.clone(),
            descriptor,
            head: self.snapshot.message_descriptors.clone(),
            transcript_cursor,
            evicted_message_id: evicted_message_id.clone(),
        });
        if let Some(message) = header.display {
            self.head_bindings
                .insert(message.id().clone(), header.content);
            if let Some(existing) = self
                .snapshot
                .messages
                .iter_mut()
                .find(|existing| existing.id() == message.id())
            {
                existing.clone_from(&message);
            } else {
                self.snapshot.messages.push(message.clone());
            }
            let order: std::collections::BTreeMap<_, _> = self
                .snapshot
                .message_descriptors
                .iter()
                .filter_map(|descriptor| {
                    descriptor.identity.as_ref().map(|identity| {
                        (
                            identity.clone(),
                            canonical_locator_position(&descriptor.locator),
                        )
                    })
                })
                .collect();
            self.snapshot
                .messages
                .sort_by_key(|message| order.get(message.id()).copied().unwrap_or(u64::MAX));
            events.push(RuntimeClientEvent::MessageCommitted {
                attempt_id,
                message,
                transcript_cursor,
                evicted_message_id,
            });
        }
        events
    }

    fn rebind_canonical_foreground(
        &mut self,
        header: &crate::durable::observation::MessageHeader,
        events: &mut Vec<RuntimeClientEvent>,
    ) {
        if header.role == "assistant" {
            if let Some(locator) = header.locator.clone() {
                self.canonical_foreground = Some(super::checkpoint::CanonicalForegroundReadCut {
                    locator: locator.clone(),
                    total: header.tool_call_count,
                });
            }
            if let Some(attempt) = self.snapshot.attempt.as_mut() {
                for slot in &mut attempt.foreground {
                    let same_message = header.locator.as_ref() == Some(&slot.occurrence.locator)
                        || header
                            .identity
                            .as_ref()
                            .is_some_and(|identity| slot.message_id.as_ref() == Some(identity));
                    if !same_message {
                        continue;
                    }
                    let Some(locator) = header.locator.clone() else {
                        continue;
                    };
                    if slot.occurrence.locator == locator {
                        continue;
                    }
                    let previous_locator =
                        std::mem::replace(&mut slot.occurrence.locator, locator.clone());
                    slot.metadata_ref = header
                        .content
                        .iter()
                        .find_map(|binding| match &binding.content_ref.source {
                            super::content::ContentSource::CanonicalMessageBlock {
                                component: super::content::CanonicalContentComponent::BlockMetadata,
                                block_index,
                                ..
                            } if *block_index == slot.block_index => {
                                Some(binding.content_ref.clone())
                            }
                            _ => None,
                        })
                        .or_else(|| {
                            self.content_metadata
                                .get(&(
                                    "canonical_block_metadata",
                                    locator.key().into_owned(),
                                    slot.block_index.get(),
                                    super::content::ContentRepresentation::CanonicalJsonV1.key(),
                                ))
                                .map(Arc::as_ref)
                                .cloned()
                        });
                    events.push(RuntimeClientEvent::ToolOccurrenceRebound {
                        attempt_id: attempt
                            .attempt_id
                            .clone()
                            .expect("live frame has a native attempt identity"),
                        previous_locator,
                        occurrence: slot.occurrence.clone(),
                        metadata_ref: slot.metadata_ref.clone(),
                    });
                }
                if attempt
                    .in_flight
                    .as_ref()
                    .is_some_and(|flight| header.identity.as_ref() == Some(&flight.message_id))
                {
                    attempt.in_flight = None;
                    self.publication_start = None;
                    self.live_frontiers.clear();
                }
            }
        }
    }

    fn observe_audit_dependencies(
        &mut self,
        receipt: &crate::durable::inbox::PublicationAuditReceipt,
    ) {
        self.observe_released_journal_prefix(receipt.ready_through);
        self.observe_released_transcript_prefix(receipt.ready_t_through, &receipt.read_cut);
        self.native_journal_highwater = self
            .native_journal_highwater
            .max(receipt.read_cut.read.journal);
        self.native_canonical_highwater = self
            .native_canonical_highwater
            .max(receipt.read_cut.ledger_through);
    }

    fn fold_audit_receipt(
        &mut self,
        attempt_id: AttemptId,
        receipt: crate::durable::inbox::PublicationAuditReceipt,
    ) -> Vec<RuntimeClientEvent> {
        // Only the captured audit T is this owner's new fact. An unrelated
        // concurrent C/J/Pending transaction must retain its publication owner.
        self.observe_audit_dependencies(&receipt);
        // Preserve the actual captured tuple as a fenced candidate; its L/J/M/T
        // requirements do not authorize reads before their native owners install.
        self.record_native_cut(receipt.read_cut.clone());
        let covered = self.source_cut.as_ref().is_some_and(|cut| {
            self.journal_through >= receipt.read_cut.read.journal
                && self.canonical_observed_through >= receipt.read_cut.ledger_through
                && cut.ledger_through >= receipt.read_cut.ledger_through
                && cut.surface_revision >= receipt.read_cut.surface_revision
                && cut.read.mutation_revision >= receipt.read_cut.read.mutation_revision
        });
        if self.pending_mutation_revision < receipt.read_cut.read.mutation_revision {
            self.pending_epoch_wait = Some(receipt.read_cut.read.mutation_revision);
        }
        self.observe_transcript_position(receipt.transcript_cursor);
        if covered {
            if let Some(cut) = &mut self.source_cut
                && cut.read.transcript == receipt.read_cut.read.transcript
            {
                cut.transcript_total = receipt.read_cut.transcript_total;
                cut.read.transcript_total = cut.transcript_total;
            }
            self.source_state.source_unavailable = false;
        } else {
            self.source_state.source_unavailable = true;
        }
        let summary = &receipt.summary;
        let matches_live = self.publication_start.as_ref().is_some_and(|start| {
            summary.stream_id.as_ref() == Some(&start.stream_id)
                && summary.message_id.as_ref() == Some(&start.message_id)
                && summary.attempt_id.as_ref() == Some(&start.attempt_id)
                && summary.request_id.as_ref() == Some(&start.request_id)
                && summary.turn_id.as_ref() == Some(&start.turn_id)
                && start.attempt_id == attempt_id
        });
        let settlement_record = super::snapshot::RuntimeClientPublicationSettlementRecord {
            settlement: receipt.summary.clone().into(),
            transcript_cursor: receipt.transcript_cursor.into(),
            read_cut: receipt.read_cut.clone(),
        };
        self.snapshot
            .publication_settlements
            .retain(|record| record.transcript_cursor != settlement_record.transcript_cursor);
        self.snapshot
            .publication_settlements
            .push(settlement_record);
        self.snapshot
            .publication_settlements
            .sort_by_key(|record| record.transcript_cursor);
        let excess = self
            .snapshot
            .publication_settlements
            .len()
            .saturating_sub(crate::durable::observation::HEADER_PAGE_MAX);
        self.snapshot.publication_settlements.drain(..excess);
        if matches_live {
            self.publication_start = None;
            self.live_frontiers.clear();
            if let Some(attempt) = self
                .snapshot
                .attempt
                .as_mut()
                .filter(|attempt| attempt.attempt_id.as_ref() == Some(&attempt_id))
                && attempt
                    .in_flight
                    .as_ref()
                    .is_some_and(|flight| summary.message_id.as_ref() == Some(&flight.message_id))
            {
                attempt.in_flight = None;
            }
        }
        let audit = receipt.header.display.map(|entry| {
            let super::snapshot::RuntimeClientTranscriptItem::PublicationAudit { audit } =
                entry.item
            else {
                unreachable!("native audit receipt source")
            };
            audit
        });
        if receipt.header.deferred || audit.is_none() {
            return vec![RuntimeClientEvent::AssistantPublicationDeferredSettled {
                attempt_id,
                settlement: receipt.summary.into(),
                settlement_head: self.snapshot.publication_settlements.clone(),
                audit: audit.map(Box::new),
                transcript_cursor: receipt.transcript_cursor.into(),
                read_cut: receipt.read_cut,
                content: receipt.header.content,
                collections: receipt.header.collections,
            }];
        }
        let audit = audit.expect("complete native audit header");
        vec![RuntimeClientEvent::AssistantPublicationSettled {
            attempt_id,
            audit: Box::new(audit),
            transcript_cursor: receipt.transcript_cursor.into(),
            settlement_head: self.snapshot.publication_settlements.clone(),
        }]
    }

    /// Appends one text/reasoning/refusal delta to the in-flight block.
    fn append_text(&mut self, block_index: ContentBlockIndex, delta: &str, kind: TextKind) {
        if self
            .live_block_ordinal
            .is_some_and(|position| position >= 32)
        {
            return;
        }

        let expected = self
            .publication_start
            .as_ref()
            .and_then(|start| {
                self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    block_index,
                    super::content::ContentRepresentation::Utf8Text.key(),
                    "live",
                ))
            })
            .map_or(0, |reference| reference.readable_end);
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
            return;
        };
        let Some(in_flight) = attempt.in_flight.as_mut() else {
            return;
        };
        let Some(existing) = in_flight
            .blocks
            .iter_mut()
            .find(|block| block_index_of(block) == block_index)
        else {
            if in_flight.blocks.len() >= 32 {
                return;
            }
            in_flight.blocks.push(match kind {
                TextKind::Text => InFlightBlock::Text {
                    block_index,
                    text: inline_prefix(delta),
                },
                TextKind::Reasoning => InFlightBlock::Reasoning {
                    block_index,
                    text: inline_prefix(delta),
                },
                TextKind::Refusal => InFlightBlock::Refusal {
                    block_index,
                    text: inline_prefix(delta),
                },
            });
            return;
        };
        match existing {
            InFlightBlock::Text { text, .. }
            | InFlightBlock::Reasoning { text, .. }
            | InFlightBlock::Refusal { text, .. } => {
                if text.len() as u64 == expected {
                    append_inline_prefix(text, delta);
                }
            }
            InFlightBlock::ToolCall { .. } => {}
        }
    }

    /// Appends one JSON argument fragment to the in-flight tool-call block
    /// and the foreground slot.
    fn append_arguments(&mut self, block_index: ContentBlockIndex, delta: &str) {
        let expected = self
            .publication_start
            .as_ref()
            .and_then(|start| {
                self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    block_index,
                    super::content::ContentRepresentation::RawJsonUtf8.key(),
                    "live",
                ))
            })
            .map_or(0, |reference| reference.readable_end);
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
            return;
        };
        let message_id = attempt
            .in_flight
            .as_ref()
            .map(|flight| flight.message_id.clone());
        if let Some(in_flight) = attempt.in_flight.as_mut()
            && let Some(InFlightBlock::ToolCall { arguments, .. }) =
                in_flight.blocks.iter_mut().find(|block| match block {
                    InFlightBlock::ToolCall {
                        block_index: index, ..
                    }
                    | InFlightBlock::Text {
                        block_index: index, ..
                    }
                    | InFlightBlock::Reasoning {
                        block_index: index, ..
                    }
                    | InFlightBlock::Refusal {
                        block_index: index, ..
                    } => *index == block_index,
                })
            && arguments.len() as u64 == expected
        {
            append_inline_prefix(arguments, delta);
        }
        if let Some(slot) = attempt.foreground.iter_mut().find(|slot| {
            message_id.is_some()
                && slot.message_id.as_ref() == message_id.as_ref()
                && slot.block_index == block_index
        }) {
            match &mut slot.state {
                ForegroundToolState::Assembled { arguments }
                | ForegroundToolState::Running { arguments, .. }
                | ForegroundToolState::Settled { arguments, .. } => {
                    if arguments.len() as u64 == expected {
                        append_inline_prefix(arguments, delta);
                    }
                }
            }
        }
    }

    fn set_assembled(&mut self, block_index: ContentBlockIndex) {
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
            return;
        };
        let Some(message_id) = attempt
            .in_flight
            .as_ref()
            .map(|flight| flight.message_id.clone())
        else {
            return;
        };
        if let Some(slot) = attempt.foreground.iter_mut().find(|slot| {
            slot.message_id.as_ref() == Some(&message_id) && slot.block_index == block_index
        }) {
            slot.assembly_complete = true;
        }
    }

    /// Settles one foreground slot with its normalized result.
    fn settle_foreground(
        &mut self,
        attempt_id: &AttemptId,
        call_id: &ToolCallId,
        occurrence: Option<&crate::message::types::ToolCallOccurrenceRef>,
        result: ToolExecutionResult,
    ) -> ForegroundSettlement {
        let Some(attempt) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
        else {
            return ForegroundSettlement::Missing;
        };
        let slot = if let Some(native) = occurrence {
            attempt.foreground.iter_mut().find(|slot| {
                slot.message_id.as_ref() == Some(&native.assistant_message_id)
                    && slot.block_index == native.block_index
            })
        } else {
            foreground_slot_for_event(
                &mut attempt.foreground,
                call_id,
                self.tool_occurrence.as_ref(),
            )
        };
        let Some(slot) = slot else {
            return ForegroundSettlement::Missing;
        };
        if occurrence.is_some_and(|owner| {
            Some(&owner.assistant_message_id) != slot.message_id.as_ref()
                || owner.block_index != slot.block_index
        }) {
            return ForegroundSettlement::Missing;
        }
        if matches!(&slot.state, ForegroundToolState::Settled { .. }) {
            return ForegroundSettlement::AlreadySettled;
        }
        let arguments = arguments_of(&slot.state);
        slot.state = ForegroundToolState::Settled { arguments, result };
        ForegroundSettlement::Applied
    }

    /// Called under the host lock after reading one coherent native cut.
    /// A complete replacement advances one Runtime cursor even when the
    /// independent native Workflow revision has skipped intermediate cuts.
    pub(crate) fn fold_workflows(
        &mut self,
        mut workflows: crate::runtime::workflow::read_model::WorkflowSnapshot,
    ) {
        for pending in &self.snapshot.pending_interactions {
            let Some(node) = pending.workflow_node.as_deref() else {
                continue;
            };
            let reason = match pending.kind {
                super::snapshot::RuntimeClientInteractionKind::Review => {
                    crate::runtime::workflow::read_model::WorkflowWait::Review
                }
                super::snapshot::RuntimeClientInteractionKind::Approval => {
                    crate::runtime::workflow::read_model::WorkflowWait::Approval
                }
                super::snapshot::RuntimeClientInteractionKind::Questionnaire => {
                    crate::runtime::workflow::read_model::WorkflowWait::Questionnaire
                }
            };
            if let Some(run) = workflows
                .runs
                .iter_mut()
                .find(|run| run.id == node.block.run)
                && let Some(instance) = run.instances.iter_mut().find(|instance| {
                    instance.block == node.block
                        && instance.node.as_ref() == Some(&node.node)
                        && instance.visit == Some(node.visit)
                })
            {
                // Correlation never interprets or changes native execution
                // state. A queued older request cannot turn a settled node
                // back into an actionable wait.
                if instance.state
                    == (crate::runtime::workflow::read_model::WorkflowState::Waiting { reason })
                {
                    instance.interaction = Some(pending.interaction.clone());
                }
            }
        }
        let previous = self.snapshot.workflows.revision.0;
        if workflows.revision.0 < previous || workflows == self.snapshot.workflows {
            return;
        }
        self.snapshot.workflows.clone_from(&workflows);
        self.publish(RuntimeClientEvent::WorkflowsUpdated { workflows });
    }

    /// Allocate the client cursor, retain the bounded replay entry and wake
    /// subscribers. No subscriber owns an event queue or blocks publication.
    fn domain_revision(&self, domain: &str) -> u64 {
        let owner = domain.split('.').next().unwrap_or(domain);
        self.domain_revisions.get(owner).copied().unwrap_or(0)
    }

    fn advance_domain_revisions(&mut self, observation: &ConversationObservation) {
        let owners: &[&'static str] = match observation {
            ConversationObservation::CanonicalCommitted { .. } => {
                &["messages", "transcript", "attempt", "todos"]
            }
            #[cfg(test)]
            ConversationObservation::Committed { .. } => {
                &["messages", "transcript", "attempt", "todos"]
            }
            ConversationObservation::PublicationOpened { .. }
            | ConversationObservation::Publication { .. } => &["attempt"],
            ConversationObservation::AuditSettled { .. } => &["attempt", "transcript"],
            ConversationObservation::PendingReceipt(_) => &["inbound", "transcript"],
            #[cfg(test)]
            ConversationObservation::InboundEnqueued(_)
            | ConversationObservation::InboundDrained(_)
            | ConversationObservation::PendingInboundChanged(_) => &["inbound", "transcript"],
            ConversationObservation::PendingHead { .. }
            | ConversationObservation::InteractionPending { .. }
            | ConversationObservation::InteractionSettled { .. } => {
                &["pending_interactions", "transcript"]
            }
            #[cfg(test)]
            ConversationObservation::InteractionRemoved { .. } => {
                &["pending_interactions", "transcript"]
            }
            ConversationObservation::NativeToolResultCommit(_) => &["attempt.foreground"],
            ConversationObservation::GoalChanged(_) => &["goal"],
            ConversationObservation::Background(_) => &["jobs"],
            ConversationObservation::SubagentLifecycle { .. }
            | ConversationObservation::SubagentActivity(_) => &["agents"],
            ConversationObservation::Capability { .. } => &["capabilities"],
            ConversationObservation::Resources { .. } => &[
                "resources",
                "capabilities",
                "model",
                "effective_plugins",
                "model_catalog",
            ],
            ConversationObservation::SessionModelChanged { .. } => &["model", "capabilities"],
            ConversationObservation::Status(_) => &["statuses"],
            #[cfg(test)]
            ConversationObservation::ManualCompactionEvent { .. } => &["context"],
            ConversationObservation::ToolProgress { .. }
            | ConversationObservation::AttemptAdmitted { .. } => &["attempt"],
            _ => &["attempt", "context", "trace"],
        };
        for owner in owners {
            let revision = self.domain_revisions.entry(owner).or_default();
            let Some(next) = revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return;
            };
            *revision = next;
            if *owner == "jobs" || *owner == "agents" {
                let keys: Vec<_> = self
                    .domain_sources
                    .range(owner.to_string()..format!("{owner}/"))
                    .filter(|(key, _)| key.split('.').next() == Some(*owner))
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in keys {
                    self.domain_sources.get_mut(&key).unwrap().2 = *revision;
                }
            }
        }
    }

    pub(crate) fn begin_publication(&mut self) {
        assert!(
            self.staged_publication.is_none(),
            "one native publication owner"
        );
        self.staged_publication = Some((Vec::new(), 0, false));
    }

    pub(crate) fn finish_publication(&mut self) -> Option<PublicationDraft> {
        let mut publication = self
            .staged_publication
            .take()
            .expect("native publication reservation");
        if self.native_source_pending() {
            for event in publication.0 {
                accumulate_finite_facts(&mut self.deferred_events, event);
            }
            self.deferred_events.2 |= publication.2;
            return None;
        }
        // Native receipts can advance J or C without the worker's JournalBatch.
        // Invalidate independent Trace readers in this same encoded publication,
        // only after the complete represented Source prefix is readable.
        if self.source_cut.is_some()
            && self.validate_source_prefix().is_ok()
            && self.published_seed.as_ref().is_some_and(|published| {
                published.read_cut.source_cut.is_none()
                    || published.read_cut.through != self.read_through
                    || published.source_cut != self.source_cut
            })
            && !publication
                .0
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::TraceChanged))
        {
            accumulate_finite_facts(&mut publication, RuntimeClientEvent::TraceChanged);
            // Invalidation belongs to this same native cut, before its terminal
            // settlement. Consumers may stop after that terminal fact.
            if let Some(terminal) = publication
                .0
                .iter()
                .position(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
            {
                publication.0[terminal..].rotate_right(1);
            }
        }
        let (events, _, replacement) = publication;
        if events.is_empty() && !replacement {
            return None;
        }
        #[cfg(test)]
        let seed_started = self
            .encoding_profile
            .as_ref()
            .map(|_| std::time::Instant::now());
        let seed = self.capture_working_seed();
        #[cfg(test)]
        if let Some(started) = seed_started {
            let mut profile = self.encoding_profile.as_ref().unwrap().lock().unwrap();
            profile.seed_us += started.elapsed().as_micros();
            profile.content_metadata_max = profile
                .content_metadata_max
                .max(self.content_metadata.len());
        }
        Some(PublicationDraft {
            #[cfg(test)]
            encoding_profile: self.encoding_profile.as_ref().map(Arc::clone),
            seed,
            events,
            replacement,
            sources: self.domain_sources.clone(),
        })
    }

    pub(crate) fn publication_failed(&mut self, error: &RuntimeClientError) {
        if matches!(error, RuntimeClientError::ProjectionExhausted) {
            self.exhausted = true;
            self.wake_subscribers();
        } else {
            self.mark_observation_unavailable(format!("{error:?}"));
        }
    }

    pub(crate) fn install_publication(&mut self, encoded: EncodedPublication) {
        assert_eq!(
            encoded.seed.cursor.get(),
            self.cursor.get().checked_add(1).expect("reserved cursor")
        );
        self.cursor = encoded.seed.cursor;
        if self.snapshot.trace.read_cut.is_none() {
            self.snapshot
                .trace
                .read_cut
                .clone_from(&encoded.seed.snapshot.trace.read_cut);
        }
        self.published_seed = Some(encoded.seed);
        self.published_sources = Some(encoded.sources);
        self.retain_replay(encoded.event, encoded.bytes);
        self.wake_subscribers();
    }

    fn projection_metadata(&self) -> super::checkpoint::ProjectionMetadata {
        if self.staged_publication.is_some() {
            return super::checkpoint::ProjectionMetadata {
                read_cut: self.working_read_cut(),
                collections: Vec::new(),
                content: Vec::new(),
            };
        }
        if let Ok(checkpoint) = self.checkpoint_candidate() {
            super::checkpoint::ProjectionMetadata {
                read_cut: checkpoint.read_cut,
                collections: checkpoint.collections,
                content: checkpoint.content,
            }
        } else {
            let mut read_cut = self.checkpoint_read_cut();
            read_cut.source_cut = None;
            super::checkpoint::ProjectionMetadata {
                read_cut,
                collections: Vec::new(),
                content: Vec::new(),
            }
        }
    }

    fn publish(&mut self, event: RuntimeClientEvent) {
        if self.staged_publication.is_some() {
            if let Some(buffer) = &mut self.batch_events {
                accumulate_finite_facts(buffer, event);
            } else if !self.publication_state.publication_commit
                && self.validate_source_prefix().is_err()
            {
                accumulate_finite_facts(&mut self.deferred_events, event);
            } else {
                if !self.deferred_events.0.is_empty() || self.deferred_events.2 {
                    let deferred = std::mem::take(&mut self.deferred_events);
                    let Some(staged) = &mut self.staged_publication else {
                        return;
                    };
                    staged.2 |= deferred.2;
                    for event in deferred.0 {
                        accumulate_finite_facts(staged, event);
                    }
                }
                if let Some(staged) = &mut self.staged_publication {
                    accumulate_finite_facts(staged, event);
                }
            }
            return;
        }
        #[cfg(test)]
        self.publish_fixture(event);
        #[cfg(not(test))]
        self.mark_observation_unavailable(
            "native display transition lacks publication scope".into(),
        );
    }

    #[cfg(test)]
    fn fixture_batch_event(&mut self, event: RuntimeClientEvent) -> Option<RuntimeClientEvent> {
        if let Some((events, bytes, overflow)) = &mut self.batch_events {
            if !*overflow {
                let encoded = super::checkpoint::bounded_bytes(
                    &event,
                    super::checkpoint::CHECKPOINT_TARGET_BYTES / 2,
                );
                if let Some(encoded) = encoded
                    && events.len() < super::checkpoint::CHECKPOINT_PAGE_ITEMS
                    && *bytes + encoded.len() <= super::checkpoint::CHECKPOINT_TARGET_BYTES / 2
                {
                    *bytes += encoded.len();
                    match event {
                        RuntimeClientEvent::ProjectionTransition { events: inner, .. } => {
                            events.extend(inner);
                        }
                        event => events.push(event),
                    }
                } else {
                    events.clear();
                    *overflow = true;
                }
            }
            return None;
        }
        Some(event)
    }

    #[cfg(test)]
    fn fixture_event_weight(event: &RuntimeClientEvent) -> usize {
        let mut weight = ReplayWeight(0);
        if serde_json::to_writer(&mut weight, event).is_ok() {
            weight.0
        } else {
            RUNTIME_CLIENT_REPLAY_BYTES + 1
        }
    }

    #[cfg(test)]
    fn publish_fixture(&mut self, event: RuntimeClientEvent) {
        let Some(event) = self.fixture_batch_event(event) else {
            return;
        };
        if !self.publication_state.publication_commit && self.validate_source_prefix().is_err() {
            accumulate_transition(&mut self.deferred_events, event);
            return;
        }
        let event = if !self.deferred_events.0.is_empty() || self.deferred_events.2 {
            accumulate_transition(&mut self.deferred_events, event);
            let (events, _, overflow) = std::mem::take(&mut self.deferred_events);
            if overflow {
                RuntimeClientEvent::CheckpointAdvanced {
                    transition: "native_transcript_prefix".into(),
                    checkpoint: Box::new(self.checkpoint_candidate().expect("finite observed cut")),
                }
            } else {
                RuntimeClientEvent::ProjectionTransition {
                    events,
                    metadata: self.projection_metadata(),
                }
            }
        } else {
            event
        };
        let next = self.cursor.get().checked_add(1);
        let Some(next_value) = next else {
            // Explicit exhaustion: the cursor never wraps, publication
            // stops, and every read path fails with `projection_exhausted`
            // instead of silently dropping the observation.
            self.exhausted = true;
            self.wake_subscribers();
            return;
        };
        let mut cut = self.working_read_cut();
        cut.revision = next_value;
        stamp_trace_cut(&mut self.snapshot.trace, &cut);
        let mut event = event;
        stamp_event_trace_cut(&mut event, &cut);
        let mut event = if super::checkpoint::bounded_bytes(
            &event,
            super::checkpoint::CHECKPOINT_TARGET_BYTES / 2,
        )
        .is_some()
        {
            event
        } else {
            let transition = event_kind(&event).to_owned();
            let Ok(mut checkpoint) = self.checkpoint_candidate() else {
                self.mark_observation_unavailable("display replacement source unavailable".into());
                return;
            };
            checkpoint.cursor = RuntimeClientCursor::new(next_value);
            checkpoint.read_cut.revision = next_value;
            RuntimeClientEvent::CheckpointAdvanced {
                transition,
                checkpoint: Box::new(checkpoint),
            }
        };
        set_checkpoint_transition_cursor(&mut event, RuntimeClientCursor::new(next_value));
        let mut metadata = self.projection_metadata();
        metadata.read_cut.revision = next_value;
        let event = match event {
            RuntimeClientEvent::ProjectionTransition { events, .. } => {
                RuntimeClientEvent::ProjectionTransition { events, metadata }
            }
            event => RuntimeClientEvent::ProjectionTransition {
                events: vec![event],
                metadata,
            },
        };
        let event =
            if super::checkpoint::bounded_bytes(&event, super::checkpoint::CHECKPOINT_TARGET_BYTES)
                .is_some()
            {
                event
            } else {
                let transition = event_kind(&event).to_owned();
                let Ok(mut checkpoint) = self.checkpoint_candidate() else {
                    self.mark_observation_unavailable(
                        "native display replacement unavailable".into(),
                    );
                    return;
                };
                checkpoint.cursor = RuntimeClientCursor::new(next_value);
                checkpoint.read_cut.revision = next_value;
                let mut metadata = self.projection_metadata();
                metadata.read_cut.revision = next_value;
                // The complete transition carries this evidence once, in its metadata.
                checkpoint.content.clear();
                checkpoint.collections.clear();
                RuntimeClientEvent::ProjectionTransition {
                    events: vec![RuntimeClientEvent::CheckpointAdvanced {
                        transition,
                        checkpoint: Box::new(checkpoint),
                    }],
                    metadata,
                }
            };
        self.cursor = RuntimeClientCursor::new(next_value);
        // Count without allocating a second serialized copy. Stop at one byte
        // beyond the ring budget: such an entry is unserviceable and evicts the
        // ring through its cursor, so a lagging subscriber gets explicit resync.
        let bytes = Self::fixture_event_weight(&event);
        self.retain_replay(event, bytes);
        self.wake_subscribers();
    }

    fn retained_event_cut(
        event: &RuntimeClientEvent,
    ) -> Option<&super::checkpoint::CheckpointReadCut> {
        match event {
            RuntimeClientEvent::ProjectionTransition { metadata, .. } => Some(&metadata.read_cut),
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => Some(&checkpoint.read_cut),
            _ => None,
        }
    }

    fn retain_replay(&mut self, event: RuntimeClientEvent, bytes: usize) {
        if let Some(cut) = Self::retained_event_cut(&event) {
            self.replay_cut_cursors.insert(cut.revision, self.cursor);
        }
        self.replay_sizes.push_back(bytes);
        self.replay_bytes += bytes;
        self.replay.push_back((self.cursor, event));
        while self.replay.len() > self.replay_limit
            || self.replay_bytes > RUNTIME_CLIENT_REPLAY_BYTES
        {
            let (cursor, event) = self.replay.pop_front().expect("retained replay entry");
            if let Some(cut) = Self::retained_event_cut(&event)
                && self.replay_cut_cursors.get(&cut.revision) == Some(&cursor)
            {
                self.replay_cut_cursors.remove(&cut.revision);
            }
            self.replay_bytes -= self.replay_sizes.pop_front().expect("retained event size");
            if self.earliest_serviceable().get() > 0 {
                self.retire_initial_read_cut();
            }
        }
    }

    fn retire_initial_read_cut(&mut self) {
        if let Some(cut) = self.initial_read_cut.take() {
            if self.replay_cut_cursors.get(&cut.revision) == Some(&RuntimeClientCursor::new(0)) {
                self.replay_cut_cursors.remove(&cut.revision);
            }
            self.replay_bytes -= super::checkpoint::CHECKPOINT_TARGET_BYTES;
        }
    }

    /// Wakes every registered subscriber. Notification carries no payload
    /// and never blocks the publisher.
    fn wake_subscribers(&self) {
        for subscriber in &self.subscribers {
            subscriber.notify.notify_one();
        }
    }

    pub(crate) fn set_inspection_frontier(&mut self, through: u64) {
        self.journal_through = through;
        self.native_journal_highwater = through;
        self.read_through = through;
    }

    pub(crate) fn checkpoint_seed(&self) -> Arc<CheckpointSeed> {
        self.published_seed
            .as_ref()
            .map_or_else(|| Arc::new(self.capture_working_seed()), Arc::clone)
    }

    #[cfg(test)]
    pub(crate) fn profile_encoding(
        &mut self,
        profile: Arc<std::sync::Mutex<PublicationEncodingProfile>>,
    ) {
        self.encoding_profile = Some(profile);
    }

    pub(crate) fn bind_activation(
        &mut self,
        activation: Option<crate::runtime::identity::SubagentId>,
    ) {
        // Host construction owns this scope before exposing the first seed or cursor.
        self.activation_id = activation;
    }

    pub(crate) fn activation_id(&self) -> Option<&crate::runtime::identity::SubagentId> {
        self.activation_id.as_ref()
    }

    pub(crate) fn initialize_publication_view(&mut self) {
        self.retire_initial_read_cut();
        let cut = self.working_read_cut();
        stamp_trace_cut(&mut self.snapshot.trace, &cut);
        let seed = Arc::new(self.capture_working_seed());
        if self.cursor.get() == 0 && self.replay.is_empty() {
            self.initial_read_cut = Some(seed.read_cut.clone());
            self.replay_cut_cursors
                .insert(seed.read_cut.revision, self.cursor);
            // Reserve the complete bounded checkpoint allowance for its cut metadata.
            // Immutable source handles already belong to the charged published workset;
            // unchanged owner revisions permit reuse, while changed owners still expire.
            self.replay_bytes += super::checkpoint::CHECKPOINT_TARGET_BYTES;
        }
        self.published_seed = Some(seed);
        self.published_sources = Some(self.domain_sources.clone());
    }

    fn capture_working_seed(&self) -> CheckpointSeed {
        let read_cut = self.working_read_cut();
        let mut seed = CheckpointSeed {
            prepared: prepared_capture::PreparedCaptures::empty(),
            read_domains: (self.read_domains_dirty
                || self.snapshot.trace.read_cut.as_ref().is_none_or(|trace| {
                    trace.through != read_cut.through || trace.source_cut != read_cut.source_cut
                }))
            .then(|| ReadDomainCut {
                revision: read_cut.revision,
                through: read_cut.through,
                source_cut: read_cut.source_cut.clone(),
            }),
            snapshot: self.snapshot.clone(),
            capability_head: self.capability_head.clone(),
            trace_head: self.trace_head.clone(),
            cursor: self.cursor,
            read_cut,
            exhausted: self.exhausted,
            journal_failure: self.source_state.journal_failure,
            source_pending: !self.source_state.source_layout_failure
                && !self.source_state.journal_failure
                && (self.native_source_pending()
                    || (self.source_state.source_unavailable && self.source_cut.is_some())),
            source_cut: self.source_cut.clone(),
            head_bindings: self.head_bindings.clone(),
            foreground_content: self.foreground_content.clone(),
            source_frontiers: self.source_frontiers.clone(),
            collection_cuts: self.collection_cuts.clone(),
            domain_revisions: self.domain_revisions.clone(),
            content_metadata: self.content_metadata.clone(),
            publication_start: self.publication_start.clone(),
            live_frontiers: self.live_frontiers.clone(),
            live_block_total: self.live_block_total,
            live_tool_total: self.live_tool_total,
            canonical_foreground: self.canonical_foreground.clone(),
        };
        seed.prepared =
            prepared_capture::PreparedCaptures::inherit(&seed, self.published_seed.as_deref());
        seed
    }

    pub(crate) fn checkpoint_candidate(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.checkpoint_seed().checkpoint_candidate()
    }

    pub(crate) fn checkpoint_read_cut(&self) -> super::checkpoint::CheckpointReadCut {
        self.published_seed
            .as_ref()
            .map_or_else(|| self.working_read_cut(), |seed| seed.read_cut.clone())
    }

    fn working_read_cut(&self) -> super::checkpoint::CheckpointReadCut {
        super::checkpoint::CheckpointReadCut {
            canonical_foreground: self.canonical_foreground.clone(),
            live_publication: self.current_live_cut(),
            conversation_id: self.snapshot.conversation_id.clone(),
            activation_id: self.activation_id.clone(),
            revision: self.cursor.get(),
            through: self.read_through,
            collection_cuts: self.collection_cuts.clone(),
            domain_revisions: self
                .domain_revisions
                .iter()
                .map(|(key, revision)| ((*key).to_owned(), *revision))
                .collect(),
            source_cut: (!self.source_state.source_unavailable
                && self.validate_source_prefix().is_ok())
            .then_some(self.source_cut.as_ref())
            .flatten()
            .map(|cut| {
                let mut cut = cut.clone();
                cut.read.journal = self.read_through;
                cut.read.ledger_through = cut.ledger_through;
                cut.read.surface_revision = cut.surface_revision;
                cut.read.transcript_total = cut.transcript_total;
                cut
            }),
        }
    }

    pub(crate) fn install_content_metadata(&mut self, references: Vec<super::content::ContentRef>) {
        for reference in references {
            if let super::content::ContentSource::CanonicalMessageBlock {
                component,
                locator,
                block_index,
            } = &reference.source
            {
                let key = (
                    component.key(),
                    locator.key().into_owned(),
                    block_index.get(),
                    reference.representation.key(),
                );
                self.content_metadata.insert(key, Arc::new(reference));
            }
        }
        // This cache contains finite read evidence only. The immutable Store is
        // the authority, and omitted indexes are materialized outside this lock.
        while self.content_metadata.len() > 4096 {
            let unneeded = self
                .content_metadata
                .keys()
                .find(|(_, message, _, _)| {
                    !self
                        .snapshot
                        .messages
                        .iter()
                        .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
                        .any(|block| block.id().as_str() == message)
                })
                .cloned();
            match unneeded {
                Some(key) => {
                    self.content_metadata.remove(&key);
                }
                None => break,
            }
        }
    }
    /// Installs a finite native descriptor head at the frozen bootstrap cut.
    /// Called during construction, before the Projection synchronization boundary exists.
    pub(crate) fn initialize_descriptor_heads(
        &mut self,
        store: &dyn crate::durable::ConversationStore,
        cut: crate::durable::observation::ObservationReadCut,
    ) -> Result<(), RuntimeClientError> {
        let messages = store
            .message_header_page(&cut, None, super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS)
            .map_err(|error| RuntimeClientError::ObservationUnavailable {
                message: error.to_string(),
            })?;
        let transcript = store
            .transcript_header_page(&cut, None, super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS)
            .map_err(|error| RuntimeClientError::ObservationUnavailable {
                message: error.to_string(),
            })?;
        self.head_bindings = messages
            .headers
            .iter()
            .filter_map(|header| {
                header
                    .identity
                    .as_ref()
                    .map(|identity| (identity.clone(), header.content.clone()))
            })
            .collect();
        self.snapshot.message_descriptors = messages
            .headers
            .iter()
            .filter_map(super::snapshot::RuntimeClientMessageDescriptor::from_header)
            .collect();
        self.snapshot.messages = messages
            .headers
            .iter()
            .filter_map(|header| header.display.clone())
            .collect();
        self.snapshot.transcript = super::snapshot::transcript_header_view(transcript);
        self.snapshot.publication_settlements = store
            .publication_settlement_head(&cut)
            .map_err(RuntimeClientError::observation_store_error)?;
        let references = messages
            .headers
            .iter()
            .flat_map(|header| {
                header
                    .content
                    .iter()
                    .map(|binding| binding.content_ref.clone())
            })
            .collect();
        self.install_content_metadata(references);
        if self.snapshot.todos.is_some()
            && let Some(todos) = store
                .todo_observation_head(&cut)
                .map_err(RuntimeClientError::observation_store_error)?
        {
            self.install_todo_head(&todos);
        }
        self.transcript_prefix = cut.read.transcript;
        self.pending_mutation_revision = cut.read.mutation_revision;
        self.canonical_observed_through = cut.ledger_through;
        self.native_canonical_highwater = cut.ledger_through;
        self.source_cut = Some(cut);
        Ok(())
    }

    fn observe_transcript_position(&mut self, cursor: crate::durable::TranscriptCursor) {
        self.observe_transcript_range(cursor.get(), cursor.get());
    }

    fn observe_transcript_range(&mut self, first: u64, last: u64) {
        if first > last || last <= self.transcript_prefix {
            return;
        }
        if let Some(cut) = &mut self.source_cut {
            if last > cut.read.transcript {
                cut.transcript_total = cut
                    .transcript_total
                    .saturating_add(last - cut.read.transcript);
            }
            cut.read.transcript = cut.read.transcript.max(last);
        }
        if first <= self.transcript_prefix.saturating_add(1) {
            self.transcript_prefix = self.transcript_prefix.max(last);
        }
    }

    fn observe_transcript_source_prefix(
        &mut self,
        prefix: u64,
        proof: &crate::durable::observation::ObservationReadCut,
    ) -> bool {
        if prefix <= self.transcript_prefix || prefix > proof.read.transcript {
            return false;
        }
        let Some(current) = &self.source_cut else {
            return false;
        };
        if proof.read.conversation_id != self.snapshot.conversation_id
            || proof.read.mutation_revision != self.pending_mutation_revision
            || current.read.mutation_revision != proof.read.mutation_revision
            || self.canonical_observed_through < proof.ledger_through
            || self.journal_through < proof.read.journal
            || current.surface_revision < proof.surface_revision
            || current.read.transcript < prefix
        {
            return false;
        }
        self.transcript_prefix = prefix;
        true
    }

    fn observe_released_transcript_prefix(
        &mut self,
        ready: Option<u64>,
        proof: &crate::durable::observation::ObservationReadCut,
    ) {
        match ready {
            Some(prefix) => {
                self.observe_transcript_source_prefix(prefix, proof);
            }
            None => self.mark_observation_unavailable(
                "native transcript release prefix unavailable".into(),
            ),
        }
    }

    fn install_pending_receipt_head(
        &mut self,
        receipt: &crate::durable::inbox::PendingObservationReceipt,
    ) {
        let mut pending = Vec::with_capacity(receipt.head.len());
        for header in &receipt.head {
            if let Some(MessageBlock::User(message)) = &header.message.display {
                pending.push(InboundItemView {
                    revision: header.revision,
                    sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                    deferred: header.message.deferred,
                    message: message.clone(),
                });
            }
        }
        self.snapshot.inbound.pending = pending;
        if let Some(header) = &receipt.selected
            && let Some(cursor) = header.message.transcript_cursor
        {
            self.observe_transcript_position(cursor);
        }
        if let Some(cursor) = receipt.removed_transcript {
            self.observe_transcript_position(cursor);
        }
    }

    pub(crate) fn apply_pending_receipt(
        &mut self,
        receipt: Option<&crate::durable::inbox::PendingObservationReceipt>,
    ) {
        let Some(receipt) = receipt else {
            self.mark_observation_unavailable("native pending display source unavailable".into());
            return;
        };
        self.observe_released_journal_prefix(receipt.ready_through);
        if receipt.read_cut.mutation_revision < self.pending_mutation_revision {
            return;
        }
        self.pending_mutation_revision = receipt.read_cut.mutation_revision;
        if let Some(expected) = self.pending_epoch_wait {
            if receipt.read_cut.mutation_revision < expected {
                return;
            }
            self.pending_epoch_wait = None;
        }
        self.install_pending_receipt_head(receipt);
        self.native_journal_highwater = self.native_journal_highwater.max(receipt.read_cut.journal);
        if let Some(mut cut) = self.source_cut.clone() {
            cut.read.clone_from(&receipt.read_cut);
            cut.transcript_total = receipt.read_cut.transcript_total;
            cut.surface_revision = receipt.read_cut.surface_revision;
            cut.ledger_through = receipt.read_cut.ledger_through;
            self.record_native_cut(cut);
        }
        if let Some(cut) = self.source_cut.clone() {
            self.observe_released_transcript_prefix(receipt.ready_t_through, &cut);
        }
        self.source_state.source_unavailable = self.source_cut.is_none();
        self.read_domains_dirty = true;
        let Some(revision) = self.read_revision.checked_add(1) else {
            self.publication_failed(&RuntimeClientError::ProjectionExhausted);
            return;
        };
        self.read_revision = revision;
        for owner in ["inbound", "transcript"] {
            let revision = self.domain_revisions.entry(owner).or_default();
            let Some(next) = revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return;
            };
            *revision = next;
        }
        let loaded = self.snapshot.inbound.pending.len() as u64;
        self.source_frontiers.insert(
            "inbound.pending".into(),
            super::checkpoint::CollectionFrontier {
                domain: "inbound.pending".into(),
                revision: self.read_revision,
                total: receipt.total,
                loaded,
                next_offset: (loaded < receipt.total).then(|| {
                    if loaded == receipt.head.len() as u64 {
                        receipt.head.last().map_or(0, |item| item.sequence)
                    } else {
                        0
                    }
                }),
            },
        );
        let mut events = vec![RuntimeClientEvent::PendingInboundChanged {
            pending: self.snapshot.inbound.pending.clone(),
        }];
        if let Some(header) = &receipt.selected
            && receipt
                .head
                .iter()
                .any(|item| item.sequence == header.sequence)
            && let Some(MessageBlock::User(message)) = &header.message.display
        {
            events.push(RuntimeClientEvent::InboundEnqueued {
                sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                message: message.clone(),
                transcript_cursor: header
                    .message
                    .transcript_cursor
                    .map(RuntimeClientTranscriptCursor::from),
            });
        }
        if let (Some(watermark), Some(count)) = (receipt.adopted_through, receipt.adopted_count) {
            let watermark = crate::runtime::inbound::InboundSequence::new(watermark);
            let count = usize::try_from(count).expect("native adoption count fits usize");
            self.snapshot.inbound.last_drain = Some(InboundDrainView { watermark, count });
            events.push(RuntimeClientEvent::InboundDrained {
                watermark,
                count,
                message_ids: Vec::new(),
            });
        }
        self.publish(RuntimeClientEvent::ProjectionTransition {
            events,
            metadata: self.projection_metadata(),
        });
    }

    pub(crate) fn mark_observation_unavailable(&mut self, mut message: String) {
        self.source_state.source_layout_failure = true;
        self.source_state.source_unavailable = true;
        let mut end = message.len().min(1024);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        self.read_failure = Some(message);
        self.wake_subscribers();
    }

    pub(crate) fn publication_commit_scope(&mut self, active: bool, _complete: bool) {
        // Queue occupancy cannot establish or invalidate a native source cut.
        // The frozen C/J/T receipts and their contiguous owner prefix do that.
        self.publication_state.publication_commit = active;
    }

    #[cfg(test)]
    pub(crate) fn source_prefix_debug(&self) -> String {
        format!(
            "cursor={} J={}/{} L={} T={} M={} wait={:?} source={:?} published={:?} unavailable={} layout={} journal={} read_failure={:?} working_progress={}",
            self.cursor.get(),
            self.journal_through,
            self.native_journal_highwater,
            self.canonical_observed_through,
            self.transcript_prefix,
            self.pending_mutation_revision,
            self.pending_epoch_wait,
            self.source_cut,
            self.published_seed.as_ref().map(|seed| &seed.read_cut),
            self.source_state.source_unavailable,
            self.source_state.source_layout_failure,
            self.source_state.journal_failure,
            self.read_failure.as_ref().map(|message| {
                let mut end = message.len().min(256);
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                &message[..end]
            }),
            self.snapshot.attempt.as_ref().map_or(0, |attempt| attempt
                .foreground
                .iter()
                .filter(|slot| matches!(
                    slot.state,
                    ForegroundToolState::Running {
                        progress: Some(_),
                        ..
                    }
                ))
                .count()),
        )
    }

    pub(crate) fn validate_published_source(&self) -> Result<(), RuntimeClientError> {
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        if let Some(seed) = &self.published_seed {
            if seed.read_cut.source_cut.is_some() && !seed.journal_failure {
                return Ok(());
            }
            if seed.source_pending
                && !self.source_state.source_layout_failure
                && !self.source_state.journal_failure
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "published native source cut unavailable".into(),
            });
        }
        self.validate_source_prefix()
    }

    /// Completion of this exact finite CPU receipt, independent of source
    /// prefix readiness and external cursor publication.
    pub(crate) fn native_receipt_installation(
        &self,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        if self.exhausted
            || self.source_state.source_layout_failure
            || self.source_state.journal_failure
        {
            crate::runtime::observation::NativeReceiptInstallation::Unavailable
        } else {
            crate::runtime::observation::NativeReceiptInstallation::Installed
        }
    }

    /// Native append captured this prefix from its publication-readiness
    /// index. It includes intrinsically ready diagnostics, but stops at every
    /// earlier owner or missing index row. It does not grant P byte access.
    fn observe_released_journal_prefix(&mut self, ready: Option<u64>) {
        let Some(ready) = ready else {
            self.mark_observation_unavailable("native source release prefix unavailable".into());
            return;
        };
        self.journal_through = self.journal_through.max(ready);
        self.read_through = self.journal_through;
    }

    /// A delayed owner fact cannot roll back a newer native transaction cut.
    fn record_native_cut(&mut self, cut: crate::durable::observation::ObservationReadCut) {
        self.native_journal_highwater = self.native_journal_highwater.max(cut.read.journal);
        self.native_canonical_highwater = self.native_canonical_highwater.max(cut.ledger_through);
        if cut.read.mutation_revision > self.pending_mutation_revision {
            self.pending_epoch_wait = Some(cut.read.mutation_revision);
        }
        let advances = self.source_cut.as_ref().is_none_or(|previous| {
            cut.read.journal >= previous.read.journal
                && cut.ledger_through >= previous.ledger_through
                && cut.surface_revision >= previous.surface_revision
                && cut.read.mutation_revision >= previous.read.mutation_revision
                && cut.read.transcript >= previous.read.transcript
        });
        if advances {
            self.source_cut = Some(cut);
        }
    }

    fn native_source_pending(&self) -> bool {
        self.journal_through < self.native_journal_highwater
            || self.canonical_observed_through < self.native_canonical_highwater
            || self.pending_epoch_wait.is_some()
            || self.source_cut.as_ref().is_some_and(|cut| {
                cut.ledger_through > self.canonical_observed_through
                    || cut.read.transcript != self.transcript_prefix
            })
    }

    pub(crate) fn validate_source_prefix(&self) -> Result<(), RuntimeClientError> {
        if self.native_source_pending() {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        if self.source_state.source_unavailable
            || self.source_state.source_layout_failure
            || self
                .source_cut
                .as_ref()
                .is_some_and(|cut| cut.read.transcript != self.transcript_prefix)
        {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native transcript commit awaits its observation owner".into(),
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn prepare_terminal_positions(
        &mut self,
        positions: Vec<(AttemptId, crate::durable::TranscriptCursor)>,
    ) {
        self.prepared_terminals.extend(positions);
    }

    pub(crate) fn prepare_message_headers(
        &mut self,
        headers: Vec<(
            MessageId,
            crate::durable::observation::MessageHeader,
            Option<crate::conversation::SurfaceRevision>,
        )>,
    ) {
        for (identity, header, revision) in headers {
            self.prepared_messages.insert(identity, (header, revision));
        }
    }

    pub(crate) fn install_domain_sources(
        &mut self,
        sources: Vec<(String, Arc<super::domain::DomainSource>, usize)>,
    ) {
        for (key, source, weight) in sources {
            let revision = self.domain_revision(&key);
            if let Some((_, old, _)) = self.domain_sources.insert(key, (source, weight, revision)) {
                self.domain_source_bytes -= old;
            }
            self.domain_source_bytes += weight;
        }
        while self.domain_sources.len() > 4096 || self.domain_source_bytes > 16 * 1024 * 1024 {
            let key = self
                .domain_sources
                .keys()
                .find(|key| key.starts_with("jobs.") || key.starts_with("agents."))
                .cloned()
                .or_else(|| self.domain_sources.keys().next().cloned());
            if let Some(key) = key {
                if let Some((_, weight, _)) = self.domain_sources.remove(&key) {
                    self.domain_source_bytes -= weight;
                }
            } else {
                break;
            }
        }
    }
    pub(crate) fn current_capability_source(
        &self,
    ) -> Option<super::capability_source::CapabilityReadSource> {
        self.domain_sources
            .get("capabilities")
            .and_then(|(source, _, _)| match source.as_ref() {
                super::domain::DomainSource::Capability(value) => Some(value.value().clone()),
                _ => None,
            })
    }
    pub(crate) fn current_model_capabilities(
        &self,
    ) -> Option<crate::model::catalog::ModelCapabilities> {
        self.domain_sources
            .get("model")
            .and_then(|(source, _, _)| match source.as_ref() {
                super::domain::DomainSource::Model(value) => value.value().capabilities().cloned(),
                _ => None,
            })
    }
    pub(crate) fn install_resource_header(
        &mut self,
        header: super::snapshot::RuntimeClientResourcesView,
    ) {
        self.snapshot.resources = header;
    }
    pub(crate) fn install_source_frontiers(
        &mut self,
        frontiers: Vec<super::checkpoint::CollectionFrontier>,
    ) {
        for frontier in frontiers {
            assert!(
                matches!(
                    frontier.domain.as_str(),
                    "model"
                        | "attempt.model"
                        | "model_catalog.models"
                        | "capabilities.configured_tools"
                        | "capabilities.tools"
                        | "capabilities.available_tools"
                        | "capabilities.skills"
                        | "capabilities.sources"
                        | "resources.inspection"
                        | "resources.context_files"
                ) || frontier.domain == "workflows.runs"
                    || frontier.domain.starts_with("workflows.runs.@")
            );
            self.source_frontiers
                .insert(frontier.domain.clone(), frontier);
        }
    }
    fn install_model_frontier(&mut self, inline: bool) {
        self.install_source_frontiers(vec![super::checkpoint::CollectionFrontier {
            domain: "model".into(),
            revision: 0,
            total: 1,
            loaded: u64::from(inline),
            next_offset: (!inline).then_some(0),
        }]);
    }
    pub(crate) fn validate_domain_version(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
    ) -> Result<(), RuntimeClientError> {
        let owner = domain.split('.').next().unwrap_or(domain);
        if cut.domain_revisions.get(owner).copied().unwrap_or(0)
            != self.published_seed.as_ref().map_or_else(
                || self.domain_revision(domain),
                |seed| seed.domain_revision(domain),
            )
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(())
    }
    pub(crate) fn domain_source(
        &self,
        domain: &str,
        item_offset: u64,
    ) -> Option<Arc<super::domain::DomainSource>> {
        let snapshot = self.snapshot_ref();
        let key = match domain {
            "pending_interactions" => snapshot
                .pending_interactions
                .get(usize::try_from(item_offset).ok()?)
                .map(|interaction| format!("pending_interactions.{}", interaction.interaction)),
            "jobs" => snapshot
                .jobs
                .get(usize::try_from(item_offset).ok()?)
                .map(|job| format!("jobs.{}", job.job_id)),
            "agents" => snapshot
                .agents
                .get(usize::try_from(item_offset).ok()?)
                .map(|agent| format!("agents.{}", agent.agent_id)),
            domain if domain.starts_with("model_catalog.") => Some("model_catalog".to_owned()),
            domain if domain.starts_with("capabilities.") => Some("capabilities".to_owned()),
            domain if domain.starts_with("resources.") => Some("resources".to_owned()),
            domain if domain.starts_with("resources.") => Some("resources".to_owned()),
            domain if domain.starts_with("model.") => Some("model".to_owned()),
            domain if domain.starts_with("attempt.model.") => Some("attempt.model".to_owned()),
            domain if domain.starts_with("workflows.") => Some("workflows".to_owned()),
            "todos.tasks" => Some("todos".to_owned()),
            _ => Some(domain.to_owned()),
        }?;
        self.published_sources
            .as_ref()
            .unwrap_or(&self.domain_sources)
            .get(&key)
            .and_then(|(source, _, revision)| {
                (*revision
                    == self.published_seed.as_ref().map_or_else(
                        || self.domain_revision(domain),
                        |seed| seed.domain_revision(domain),
                    ))
                .then(|| Arc::clone(source))
            })
    }

    pub(crate) fn validate_checkpoint_cut(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
    ) -> Result<(), RuntimeClientError> {
        if self.checkpoint_read_cut() == *cut {
            return Ok(());
        }
        // The current bounded Trace cache owns its issued cut independently
        // of replay eviction. Retiring that cache also retires this evidence.
        if self
            .published_seed
            .as_ref()
            .filter(|seed| {
                !seed.snapshot.trace.records.is_empty() || !seed.snapshot.trace.updates.is_empty()
            })
            .and_then(|seed| seed.snapshot.trace.read_cut.as_ref())
            == Some(cut)
        {
            return Ok(());
        }
        if self.replay_cut_cursors.get(&cut.revision) == Some(&RuntimeClientCursor::new(0))
            && self.earliest_serviceable().get() == 0
            && self.initial_read_cut.as_ref() == Some(cut)
        {
            return Ok(());
        }
        if let Some(cursor) = self.replay_cut_cursors.get(&cut.revision)
            && let Some((first, _)) = self.replay.front()
            && let Some(position) = cursor
                .get()
                .checked_sub(first.get())
                .and_then(|position| usize::try_from(position).ok())
            && let Some((retained_cursor, event)) = self.replay.get(position)
            && retained_cursor == cursor
            && Self::retained_event_cut(event) == Some(cut)
        {
            return Ok(());
        }
        Err(RuntimeClientError::ReadCutExpired)
    }

    #[cfg(test)]
    pub(crate) fn publication_source(
        &self,
        block_index: ContentBlockIndex,
    ) -> super::content::ContentSource {
        let start = self
            .publication_start
            .as_ref()
            .expect("opened native stream");
        super::content::ContentSource::LivePublicationPrefix {
            component: super::content::PublicationContentComponent::Payload,
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            attempt_id: start.attempt_id.clone(),
            turn_id: start.turn_id.clone(),
            publication_request_id: start.request_id.clone(),
            block_index,
        }
    }

    #[cfg(test)]
    pub(crate) fn checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        self.checkpoint_seed()
            .checkpoint_page(cut, domain, offset, limit)
    }

    pub(crate) fn validate_live_content_ref(
        &self,
        reference: &super::content::ContentRef,
    ) -> Result<(), RuntimeClientError> {
        use super::content::ContentSource;
        let ContentSource::LivePublicationPrefix { .. } = &reference.source else {
            return Ok(());
        };
        let Some(scope) = self.checkpoint_read_cut().live_publication else {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        };
        let ContentSource::LivePublicationPrefix {
            component,
            stream_id,
            message_id,
            attempt_id,
            turn_id,
            publication_request_id,
            ..
        } = &reference.source
        else {
            unreachable!()
        };
        let ContentSource::LivePublicationPrefix {
            stream_id: expected_stream,
            message_id: expected_message,
            attempt_id: expected_attempt,
            turn_id: expected_turn,
            publication_request_id: expected_request,
            ..
        } = &scope.source
        else {
            unreachable!()
        };
        let _ = component;
        if reference.conversation_id != self.snapshot.conversation_id
            || stream_id != expected_stream
            || message_id != expected_message
            || attempt_id != expected_attempt
            || turn_id != expected_turn
            || publication_request_id != expected_request
            || reference.version != 1
            || reference.content_revision > scope.revision
        {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        }
        Ok(())
    }

    /// Capture one semantic cut. Completion never consults the moving live head.
    #[cfg(test)]
    pub(crate) fn snapshot_candidate(&self) -> Result<SnapshotCandidate, RuntimeClientError> {
        #[cfg(test)]
        self.probe_snapshot_enter();
        self.capture_candidate()
    }

    pub(crate) fn read_domain_candidate(&self) -> Option<ReadDomainCut> {
        if self.subscribers.is_empty() && !self.read_domains_requested {
            return None;
        }
        self.read_domain_cut()
    }

    /// A published Source failure requires a successful worker-owned repair.
    /// An ordinary dirty view is materialized from the checkpoint's own seed.
    pub(crate) fn request_failed_read_domains(&mut self) -> Option<(ReadDomainCut, bool)> {
        self.read_failure.as_ref()?;
        self.request_read_domains()
    }

    /// Request a worker-owned hydration without requiring a subscriber.
    /// The worker still owns physical reads and their publication fence.
    pub(crate) fn request_read_domains(&mut self) -> Option<(ReadDomainCut, bool)> {
        if self.exhausted
            || (!self.read_domains_dirty && self.read_failure.is_none())
            || self.validate_source_prefix().is_err()
        {
            return None;
        }
        let newly_requested = !self.read_domains_requested;
        self.read_domains_requested = true;
        self.read_domains_dirty = true;
        self.read_domain_cut().map(|cut| (cut, newly_requested))
    }

    #[cfg(test)]
    fn capture_candidate(&self) -> Result<SnapshotCandidate, RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        Ok(SnapshotCandidate {
            snapshot: self.checkpoint_candidate()?.snapshot,
            cursor: self.cursor,
            trace_through: self.journal_through,
            read_cut: self.read_domain_fence(),
        })
    }

    /// Capture a finite derived cut; Store work must happen outside the host lock.
    pub(crate) fn read_domain_fence(&self) -> ReadDomainCut {
        ReadDomainCut {
            revision: self.read_revision,
            through: self.read_through,
            source_cut: self.working_read_cut().source_cut,
        }
    }

    #[cfg(test)]
    pub(crate) fn request_read_domains_for_test(&mut self) {
        self.read_domains_requested = true;
        self.read_domains_dirty = true;
    }

    pub(crate) fn read_domain_cut(&self) -> Option<ReadDomainCut> {
        (self.read_domains_dirty && !self.exhausted && !self.native_source_pending())
            .then(|| self.read_domain_fence())
    }

    /// Installation is the linearization point. A newer semantic cut retires
    /// both successful and failed reads, leaving the newest cut dirty for repair.
    pub(crate) fn install_read_domains(
        &mut self,
        cut: &ReadDomainCut,
        read: ReadDomains,
        publish: bool,
    ) -> bool {
        if self.exhausted
            || self.read_domain_fence() != *cut
            || (!self.read_domains_dirty && self.read_failure.is_none())
        {
            return false;
        }
        self.read_domains_requested = false;
        let ReadDomainValues {
            projection_head,
            transcript,
            occupancy,
            mut trace,
            trace_total,
        } = match read {
            Ok(value) => value,
            Err(error) => {
                self.read_domains_dirty = false;
                let mut message = error.to_string();
                if message.len() > 1024 {
                    let mut end = 1024;
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    message.truncate(end);
                }
                self.read_failure = Some(message);
                self.wake_subscribers();
                return true;
            }
        };
        let recovered_failure = self.read_failure.take().is_some();
        if recovered_failure {
            self.wake_subscribers();
        }
        self.read_domains_dirty = false;
        let trace_changed = trace.as_mut().is_some_and(|trace| {
            super::trace::repair_records(&mut trace.records, &self.snapshot);
            // This cache owns its original issued cut. A live refresh without
            // Trace never clears, retags or retires its physical Source pins.
            trace.read_cut.clone_from(&self.snapshot.trace.read_cut);
            self.snapshot.trace != *trace
        });
        if let Some(trace) = &trace {
            self.source_frontiers.insert(
                "trace.records".into(),
                super::trace::checkpoint_frontier(
                    trace_total,
                    trace.records.len() as u64,
                    trace.next_cursor.as_ref(),
                ),
            );
        }
        if recovered_failure
            || self.snapshot.transcript != transcript
            || self.snapshot.context.occupancy != occupancy
            || self.snapshot.context.projection_head != projection_head
            || trace_changed
        {
            self.snapshot.transcript.clone_from(&transcript);
            if let Some(trace) = &mut trace {
                if trace_changed {
                    trace.read_cut = None;
                }
                if self.snapshot.trace.records != trace.records {
                    self.trace_head = super::checkpoint::PreparedTraceHead::default();
                }
                self.snapshot.trace.clone_from(trace);
            }
            self.snapshot.context.occupancy.clone_from(&occupancy);
            self.snapshot
                .context
                .projection_head
                .clone_from(&projection_head);
            if publish {
                self.publish(RuntimeClientEvent::ReadDomainsUpdated {
                    projection_head,
                    transcript,
                    trace: trace_changed.then_some(trace).flatten(),
                    occupancy,
                    todos: self.snapshot.todos.clone(),
                });
            }
        }
        true
    }

    pub(crate) fn initialize_read_domains(
        &mut self,
        store: &dyn crate::durable::ConversationStore,
    ) {
        self.read_domains_dirty = true;
        if let Some(cut) = self.read_domain_cut() {
            self.install_read_domains(&cut, cut.read(store), false);
        }
    }

    /// The snapshot and its cursor, linearized together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted: after that point the projection can no longer
    /// fold authoritative transitions, so returning a stale read model
    /// would silently hide dropped observations.
    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        #[cfg(test)]
        self.probe_snapshot_enter();
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        Ok((self.snapshot_ref().clone(), self.cursor))
    }

    /// Captured under the same host lock as the snapshot and subscription.
    #[cfg(test)]
    pub(crate) fn snapshot_cut(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor, u64), RuntimeClientError> {
        let (snapshot, cursor) = self.snapshot()?;
        Ok((snapshot, cursor, self.journal_through))
    }

    /// Repairs missed committed pending notifications from durable authority.
    #[cfg(test)]
    pub(crate) fn repair_pending(&mut self, items: Vec<InboundItem>) {
        let pending: Vec<_> = items.iter().map(inbound_item_view).collect();
        if self.snapshot.inbound.pending != pending {
            self.apply(ConversationObservation::PendingInboundChanged(items));
        }
    }

    /// The oldest cursor a subscription can still serve: one before the
    /// oldest retained replay entry, or the current cursor when the ring
    /// is empty.
    fn earliest_serviceable(&self) -> RuntimeClientCursor {
        match self.replay.front() {
            Some((cursor, _)) => RuntimeClientCursor::new(cursor.get().saturating_sub(1)),
            None => self.cursor,
        }
    }

    /// Registers a subscriber consuming from `after_cursor` and returns its
    /// registration identity and wakeup handle.
    ///
    /// No events are copied into the subscriber: the bounded replay ring is
    /// the one retained backlog and the subscriber is a cursor into it.
    /// Serviceability is checked at registration under the caller's
    /// synchronization boundary, so a resume after a serviceable cursor has
    /// no gap and an expired cursor fails explicitly with
    /// `resync_required`.
    pub(crate) fn subscribe(
        &mut self,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(u64, Arc<Notify>), RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        let earliest = self.earliest_serviceable();
        if after_cursor > self.cursor || after_cursor < earliest {
            return Err(RuntimeClientError::ResyncRequired {
                after_cursor,
                earliest_serviceable: earliest,
            });
        }
        let subscriber_id = self.next_subscriber_id;
        self.next_subscriber_id = self
            .next_subscriber_id
            .checked_add(1)
            .ok_or(RuntimeClientError::ProjectionExhausted)?;
        let notify = Arc::new(Notify::new());
        self.subscribers.push(Subscriber {
            id: subscriber_id,
            consumed: after_cursor,
            notify: notify.clone(),
        });
        // A subscriber registered behind the current cursor already has
        // retained work: arm its first wakeup so a parked consumer never
        // waits for the next publication to observe the existing gap.
        if after_cursor < self.cursor {
            notify.notify_one();
        }
        Ok((subscriber_id, notify))
    }

    /// Polls one registered subscriber for the next retained event after
    /// its consumed cursor, advancing that cursor on delivery.
    ///
    /// This is the one delivery path: exactly one retained ring entry is
    /// handed out per call, so a consumer can never observe a
    /// non-contiguous cursor sequence. If the events the subscriber still
    /// needs were evicted, the poll reports [`SubscriberPoll::Lagged`]
    /// without advancing the cursor — the condition is stable and terminal
    /// until the client re-subscribes.
    pub(crate) fn poll_subscriber(&mut self, subscriber_id: u64) -> SubscriberPoll {
        if self.exhausted {
            return SubscriberPoll::Exhausted;
        }
        let earliest = self.earliest_serviceable();
        let Some(subscriber) = self
            .subscribers
            .iter_mut()
            .find(|subscriber| subscriber.id == subscriber_id)
        else {
            return SubscriberPoll::Closed;
        };
        let consumed = subscriber.consumed;
        if let Some(message) = &self.read_failure {
            return SubscriberPoll::Unavailable {
                message: message.clone(),
            };
        }
        if consumed >= self.cursor {
            return SubscriberPoll::Pending;
        }
        if consumed < earliest {
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        }
        let Some((cursor, event)) = self
            .replay
            .iter()
            .find(|(cursor, _)| *cursor > consumed)
            .map(|(cursor, event)| (*cursor, event.clone()))
        else {
            // Unreachable while `consumed < self.cursor` and
            // `consumed >= earliest_serviceable`; treated as lag rather
            // than as a silent skip.
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        };
        subscriber.consumed = cursor;
        SubscriberPoll::Event(RuntimeClientProtocolEvent { cursor, event })
    }

    /// Removes one registered subscriber (attachment detach, re-subscribe,
    /// or a dropped subscription handle) and wakes it so a parked consumer
    /// observes the closure instead of waiting forever.
    pub(crate) fn remove_subscriber(&mut self, subscriber_id: u64) {
        if let Some(index) = self
            .subscribers
            .iter()
            .position(|subscriber| subscriber.id == subscriber_id)
        {
            let subscriber = self.subscribers.remove(index);
            subscriber.notify.notify_one();
        }
    }

    /// The current cursor (host observability/tests).
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> RuntimeClientCursor {
        self.cursor
    }

    /// Forces the cursor near exhaustion (overflow tests).
    #[cfg(test)]
    pub(crate) fn force_cursor_for_test(&mut self, value: u64) {
        self.cursor = RuntimeClientCursor::new(value);
    }

    /// The current snapshot read model reference (host/tests).
    pub(crate) fn snapshot_ref(&self) -> &RuntimeClientSnapshot {
        self.published_seed
            .as_ref()
            .map_or(&self.snapshot, |seed| &seed.snapshot)
    }

    /// The current snapshot read model reference, failing explicitly once
    /// the observation stream is exhausted.
    pub(crate) fn snapshot_ref_checked(
        &self,
    ) -> Result<&RuntimeClientSnapshot, RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        Ok(self.snapshot_ref())
    }

    /// The retained replay ring (tests).
    #[cfg(test)]
    #[allow(dead_code)] // asserted by the bounded-replay regression tests
    pub(crate) fn replay_len(&self) -> usize {
        self.replay.len()
    }

    #[cfg(test)]
    fn probe_publish_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.publish_enter();
        }
    }

    #[cfg(test)]
    pub(crate) fn probe_snapshot_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.snapshot_enter();
        }
    }
}

/// The text block kinds folded into the in-flight message.
#[derive(Debug, Clone, Copy)]
enum TextKind {
    Text,
    Reasoning,
    Refusal,
}

/// The block index of one in-flight block.
fn block_index_of(block: &InFlightBlock) -> ContentBlockIndex {
    match block {
        InFlightBlock::Text { block_index, .. }
        | InFlightBlock::Reasoning { block_index, .. }
        | InFlightBlock::Refusal { block_index, .. }
        | InFlightBlock::ToolCall { block_index, .. } => *block_index,
    }
}

/// Appends one block in its native publication creation order.
fn push_in_flight_block(in_flight: &mut Option<InFlightAssistantMessage>, block: InFlightBlock) {
    let Some(message) = in_flight.as_mut() else {
        return;
    };
    let index = block_index_of(&block);
    if !message
        .blocks
        .iter()
        .any(|existing| block_index_of(existing) == index)
    {
        if message.blocks.len() >= 32 {
            return;
        }
        message.blocks.push(block);
    }
}

/// The mutable foreground slot of one logical call.
fn foreground_slot_for_event<'a>(
    slots: &'a mut [ForegroundToolExecution],
    call_id: &ToolCallId,
    occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
) -> Option<&'a mut ForegroundToolExecution> {
    if let Some(native) = occurrence {
        return slots.iter_mut().find(|slot| {
            slot.occurrence.block_index == native.block_index
                && slot.occurrence.locator
                    == super::content::CanonicalContentLocator::ByLedgerPosition {
                        position: native.ledger_position,
                    }
        });
    }
    foreground_slot_mut(slots, call_id)
}

fn foreground_slot_mut<'a>(
    foreground: &'a mut [ForegroundToolExecution],
    call_id: &ToolCallId,
) -> Option<&'a mut ForegroundToolExecution> {
    foreground
        .iter_mut()
        .find(|slot| slot.call_id.as_ref() == Some(call_id))
}

fn update_stored_tool_slots(
    snapshot: &mut RuntimeClientSnapshot,
    occurrence: &super::snapshot::RuntimeClientToolOccurrence,
    mut update: impl FnMut(&mut super::snapshot::ForegroundToolExecution),
) {
    for slot in snapshot
        .message_descriptors
        .iter_mut()
        .flat_map(|descriptor| &mut descriptor.tool_calls)
        .chain(
            snapshot
                .transcript
                .entries
                .iter_mut()
                .flat_map(|entry| &mut entry.tool_calls),
        )
    {
        if &slot.occurrence == occurrence {
            update(slot);
        }
    }
}

pub(crate) fn tool_witness_result(
    witness: &crate::durable::observation::ObservationEventWitness,
) -> Option<ToolExecutionResult> {
    let value = &witness.value;
    let status = if witness.event_kind == "tool_execution_failed" {
        ToolExecutionStatus::Failed {
            error: value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("deferred tool diagnostic")
                .to_owned(),
        }
    } else {
        value
            .get("result")
            .and_then(|result| result.get("status"))
            .and_then(|status| {
                super::structural_json::decode::<ToolExecutionStatus>(status.clone()).ok()
            })?
    };
    let result_value = value.get("result");
    let scalar_u64 = |field: &str| {
        result_value
            .and_then(|result| result.get(field))
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    };
    Some(ToolExecutionResult {
        status,
        content: Vec::new(),
        duration_ms: scalar_u64("duration_ms").unwrap_or(0),
        exit_code: result_value
            .and_then(|result| result.get("exit_code"))
            .and_then(serde_json::Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        artifacts: Vec::new(),
        deliveries: Vec::new(),
        truncation: result_value
            .and_then(|result| result.get("truncation"))
            .and_then(|value| super::structural_json::decode(value.clone()).ok()),
        workflow: None,
        managed_output: result_value
            .and_then(|result| result.get("managed_output"))
            .and_then(|value| super::structural_json::decode(value.clone()).ok()),
    })
}

fn deferred_live_block(block: &InFlightBlock) -> InFlightBlock {
    match block {
        InFlightBlock::Text { block_index, .. } => InFlightBlock::Text {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::Reasoning { block_index, .. } => InFlightBlock::Reasoning {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::Refusal { block_index, .. } => InFlightBlock::Refusal {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::ToolCall {
            block_index,
            call_id,
            metadata_ref,
            tool_id,
            name,
            ..
        } => InFlightBlock::ToolCall {
            block_index: *block_index,
            call_id: call_id.clone(),
            metadata_ref: metadata_ref.clone(),
            tool_id: tool_id.clone(),
            name: name.clone(),
            arguments: String::new(),
        },
    }
}

/// The arguments string of one foreground state.
fn arguments_of(state: &ForegroundToolState) -> String {
    match state {
        ForegroundToolState::Assembled { arguments }
        | ForegroundToolState::Running { arguments, .. }
        | ForegroundToolState::Settled { arguments, .. } => arguments.clone(),
    }
}

/// Whether the native event settles its attempt.
#[cfg(test)]
pub(crate) fn is_attempt_terminal(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    )
}

fn accumulate_finite_facts(
    buffer: &mut (Vec<RuntimeClientEvent>, usize, bool),
    event: RuntimeClientEvent,
) {
    if buffer.2 {
        return;
    }
    let events = match event {
        RuntimeClientEvent::ProjectionTransition { events, .. } => events,
        event => vec![event],
    };
    if buffer.0.len() + events.len() > super::checkpoint::CHECKPOINT_PAGE_ITEMS {
        buffer.0.clear();
        buffer.2 = true;
    } else {
        buffer.0.extend(events);
    }
}

#[cfg(test)]
fn accumulate_transition(
    buffer: &mut (Vec<RuntimeClientEvent>, usize, bool),
    event: RuntimeClientEvent,
) {
    if buffer.2 {
        return;
    }
    let count = match &event {
        RuntimeClientEvent::ProjectionTransition { events, .. } => events.len(),
        _ => 1,
    };
    let bytes =
        super::checkpoint::bounded_bytes(&event, super::checkpoint::CHECKPOINT_TARGET_BYTES / 2);
    if let Some(bytes) = bytes
        && buffer.0.len() + count <= super::checkpoint::CHECKPOINT_PAGE_ITEMS
        && buffer.1 + bytes.len() <= super::checkpoint::CHECKPOINT_TARGET_BYTES / 2
    {
        buffer.1 += bytes.len();
        match event {
            RuntimeClientEvent::ProjectionTransition { events, .. } => buffer.0.extend(events),
            event => buffer.0.push(event),
        }
    } else {
        buffer.0.clear();
        buffer.2 = true;
    }
}

fn set_checkpoint_transition_cursor(event: &mut RuntimeClientEvent, cursor: RuntimeClientCursor) {
    match event {
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            checkpoint.cursor = cursor;
            checkpoint.read_cut.revision = cursor.get();
        }
        RuntimeClientEvent::ProjectionTransition { events, .. } => {
            for event in events {
                set_checkpoint_transition_cursor(event, cursor);
            }
        }
        _ => {}
    }
}

fn inline_block_bytes(block: &InFlightBlock, field: &str) -> u64 {
    let value = match block {
        InFlightBlock::Text { text, .. }
        | InFlightBlock::Reasoning { text, .. }
        | InFlightBlock::Refusal { text, .. } => text,
        InFlightBlock::ToolCall { arguments, .. } if field == "arguments" => arguments,
        InFlightBlock::ToolCall { .. } => return 0,
    };
    value.len() as u64
}

pub(crate) fn client_failure(failure: &AttemptFailure) -> RuntimeClientAttemptFailure {
    super::event::finite_attempt_failure(failure).0
}

/// One pending inbound item of the diagnostics view.
fn inbound_item_view(item: &InboundItem) -> InboundItemView {
    let copied =
        super::checkpoint::bounded_clone(item.message(), super::checkpoint::CHECKPOINT_ITEM_BYTES);
    let deferred = copied.is_none();
    let message = copied.unwrap_or_else(|| {
        let mut capture = super::checkpoint::Capture::new(0);
        let MessageBlock::User(message) = capture.user_message(
            item.message(),
            &ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            &[],
        ) else {
            unreachable!("native inbound is User")
        };
        message
    });
    InboundItemView {
        revision: item.revision(),
        sequence: item.sequence(),
        deferred,
        message,
    }
}

/// Projects one authoritative background registry snapshot into the
/// external Runtime Client shape.
pub(crate) fn background_view(snapshot: &BackgroundExecutionSnapshot) -> RuntimeClientJob {
    let bash = snapshot
        .bash
        .as_deref()
        .and_then(|value| super::checkpoint::bounded_clone(value, 1024));
    let result = snapshot
        .result
        .as_deref()
        .and_then(|value| super::checkpoint::bounded_clone(value, 4096));
    let mut deferred_fields = Vec::new();
    if snapshot.bash.is_some() && bash.is_none() {
        deferred_fields.push(vec!["bash".into()]);
    }
    if snapshot.result.is_some() && result.is_none() {
        deferred_fields.push(vec!["result".into()]);
    }
    if snapshot.tool_name.len() > 256 {
        deferred_fields.push(vec!["tool_name".into()]);
    }
    let tool_id = super::snapshot::inline_tool_id(&snapshot.tool_id);
    if tool_id.is_none() {
        deferred_fields.push(vec!["tool_id".into()]);
    }
    RuntimeClientJob {
        deferred_fields,
        bash,
        job_id: snapshot.execution_id.clone(),
        tool_id,
        tool_name: snapshot.tool_name.chars().take(256).collect(),
        state: snapshot.state,
        progress: snapshot.progress.clone(),
        result,
    }
}

pub(crate) fn background_source_view(
    value: &crate::tools::background::BackgroundObservationValue<'_>,
) -> RuntimeClientJob {
    let bash = value
        .bash
        .and_then(|value| super::checkpoint::bounded_clone(value, 1024));
    let result = value
        .result
        .and_then(|value| super::checkpoint::bounded_clone(value, 4096));
    let progress = value
        .progress
        .and_then(|value| super::checkpoint::bounded_clone(value, 1024));
    let mut deferred_fields = Vec::new();
    if value.bash.is_some() && bash.is_none() {
        deferred_fields.push(vec!["bash".into()]);
    }
    if value.result.is_some() && result.is_none() {
        deferred_fields.push(vec!["result".into()]);
    }
    if value.progress.is_some() && progress.is_none() {
        deferred_fields.push(vec!["progress".into()]);
    }
    if value.tool_name.len() > 256 {
        deferred_fields.push(vec!["tool_name".into()]);
    }
    let tool_id = super::snapshot::inline_tool_id(value.tool_id);
    if tool_id.is_none() {
        deferred_fields.push(vec!["tool_id".into()]);
    }
    RuntimeClientJob {
        deferred_fields,
        bash,
        result,
        progress,
        job_id: value.job_id.clone(),
        tool_id,
        tool_name: value.tool_name.chars().take(256).collect(),
        state: value.state,
    }
}

pub(crate) fn agent_source_view(
    value: &crate::runtime::subagent::AgentObservationValue<'_>,
) -> super::snapshot::RuntimeClientAgent {
    let profile = value
        .execution_profile
        .and_then(|profile| super::checkpoint::bounded_clone(profile, 2048));
    let observation = super::checkpoint::bounded_clone(value.observation, 1024);
    let mut deferred_fields = Vec::new();
    if value.title.len() > 256 {
        deferred_fields.push(vec!["title".into()]);
    }
    if value.detail.is_some_and(|detail| detail.len() > 256) {
        deferred_fields.push(vec!["detail".into()]);
    }
    if value.execution_profile.is_some() && profile.is_none() {
        deferred_fields.push(vec!["execution_profile".into()]);
    }
    if observation.is_none() {
        deferred_fields.push(vec!["observation".into()]);
    }
    super::snapshot::RuntimeClientAgent {
        deferred_fields,
        title: value.title.chars().take(256).collect(),
        parent_agent_id: value.parent_agent_id.clone(),
        activation_id: value.activation_id.clone(),
        agent_id: value.agent_id.clone(),
        child_conversation_id: value.child_conversation_id.clone(),
        agent: value.agent.into(),
        definition_digest: value.definition_digest.into(),
        profile_digest: value.profile_digest.into(),
        state: value.state,
        current_activation: value.current_activation.cloned(),
        activation_state: value.activation_state,
        detail: value.detail.map(bounded_diagnostic),
        observation: observation.unwrap_or_default(),
        execution_profile: profile,
        started_at: value.started_at,
        workspace: value.workspace.into(),
    }
}

/// Builds the deterministic active capability projection from the
/// authoritative capability snapshot and the coordinator-owned
/// availability state (Issue #81).
///
/// The tool catalog preserves registry order; the Skill catalog is
/// ordered by Skill name (the two snapshot lists derive from one sorted
/// package order); the source list preserves the deterministic
/// source-identity order of the authoritative availability map. No
/// executors, environment paths, or dependency internals ever appear.
#[cfg(test)]
pub(crate) fn capability_view(
    snapshot: &crate::capabilities::CapabilitySnapshot,
    availability: &crate::capabilities::CapabilityAvailability,
    model: Option<&crate::model::catalog::ModelCapabilities>,
) -> CapabilityView {
    let source = super::capability_source::CapabilityReadSource::new(
        Arc::new(snapshot.clone()),
        availability.clone(),
        model.cloned(),
    );
    source.prepare();
    source.header()
}

/// Inserts or replaces one background view, preserving execution
/// allocation order.
fn upsert_background(
    background: &mut Vec<RuntimeClientJob>,
    view: RuntimeClientJob,
) -> Option<crate::runtime::identity::ToolExecutionId> {
    if let Some(existing) = background
        .iter_mut()
        .find(|entry| entry.job_id == view.job_id)
    {
        *existing = view;
        return None;
    }
    background.insert(0, view);
    (background.len() > 32).then(|| background.pop().unwrap().job_id)
}

/// Projects one authoritative subagent registry snapshot into the external
/// Runtime Client shape (Issue #60).
pub(crate) fn agent_view(
    agent: &crate::runtime::subagent::AgentSnapshot,
    activation: &crate::runtime::subagent::SubagentSnapshot,
) -> super::snapshot::RuntimeClientAgent {
    activation_view(
        activation,
        agent.state,
        agent.current_activation.clone(),
        &agent.title,
    )
}

/// Activation data enriches a view only with explicitly supplied Agent authority.
pub(crate) fn activation_view(
    snapshot: &crate::runtime::subagent::SubagentSnapshot,
    state: crate::runtime::subagent::AgentState,
    current_activation: Option<crate::runtime::identity::SubagentId>,
    title: &str,
) -> super::snapshot::RuntimeClientAgent {
    super::snapshot::RuntimeClientAgent {
        deferred_fields: {
            let mut fields = Vec::new();
            if title.len() > 256 {
                fields.push(vec!["title".into()]);
            }
            if snapshot
                .detail
                .as_deref()
                .is_some_and(|detail| detail.len() > 256)
            {
                fields.push(vec!["detail".into()]);
            }
            if snapshot
                .profile
                .as_deref()
                .is_some_and(|profile| super::checkpoint::bounded_bytes(profile, 2048).is_none())
            {
                fields.push(vec!["execution_profile".into()]);
            }
            fields
        },
        title: title.chars().take(256).collect(),
        parent_agent_id: snapshot.parent_agent_id.clone(),
        activation_id: snapshot.subagent_id.clone(),
        agent_id: snapshot.child_agent_id.clone(),
        child_conversation_id: snapshot.child_conversation_id.clone(),
        agent: snapshot.agent.clone(),
        definition_digest: snapshot.definition_digest.clone(),
        profile_digest: snapshot.profile_digest.clone(),
        state,
        current_activation,
        activation_state: snapshot.state,
        detail: snapshot.detail.as_deref().map(bounded_diagnostic),
        observation: snapshot.observation.clone(),
        execution_profile: snapshot
            .profile
            .as_deref()
            .and_then(|profile| super::checkpoint::bounded_clone(profile, 2048)),
        started_at: snapshot.started_at,
        workspace: snapshot.workspace.clone(),
    }
}

fn upsert_subagent(
    subagents: &mut Vec<super::snapshot::RuntimeClientAgent>,
    seen: &mut std::collections::HashSet<crate::runtime::identity::SubagentId>,
    view: super::snapshot::RuntimeClientAgent,
) -> bool {
    if let Some(existing) = subagents
        .iter_mut()
        .find(|entry| entry.agent_id == view.agent_id)
    {
        if existing.activation_id != view.activation_id && seen.contains(&view.activation_id) {
            return false;
        }
        seen.insert(view.activation_id.clone());
        *existing = view;
    } else {
        seen.insert(view.activation_id.clone());
        subagents.insert(0, view);
        subagents.truncate(32);
    }
    seen.retain(|activation| {
        subagents
            .iter()
            .any(|agent| &agent.activation_id == activation)
    });
    true
}

/// Inserts or replaces one live native interaction projection, preserving
/// coordinator publication order. Identity never determines presentation order.
fn upsert_interaction(
    interactions: &mut Vec<super::snapshot::RuntimeClientInteractionHeader>,
    request: super::snapshot::RuntimeClientInteractionHeader,
) {
    if let Some(existing) = interactions
        .iter_mut()
        .find(|existing| existing.interaction == request.interaction)
    {
        *existing = request;
    } else {
        interactions.push(request);
    }
}

/// Builds the structured external status view from the exact composed
/// status observation; the rendered representation derives from the same
/// composition through the one canonical renderer.
#[cfg(test)]
pub(crate) fn status_view(observation: &AgentStatusObservation) -> AgentStatusView {
    prepared_status_view(observation, None)
}
/// Prepare finite display metadata before entering the projection synchronization boundary.
pub(crate) fn prepared_status_view(
    observation: &AgentStatusObservation,
    source: Option<&crate::durable::inbox::StatusObservationSource>,
) -> AgentStatusView {
    prepared_status(
        &observation.attempt_id,
        observation.turn,
        &observation.status_message_id,
        &observation.opportunities,
        observation.post_tool_batch_anchor,
        &observation.status,
        source,
    )
}
pub(crate) fn prepared_status(
    attempt_id: &crate::runtime::identity::AttemptId,
    turn: u32,
    status_message_id: &crate::runtime::identity::MessageId,
    opportunities: &crate::context::ContributionOpportunities,
    post_tool_batch_anchor: Option<crate::durable::TranscriptCursor>,
    status: &crate::context::AgentStatus,
    source: Option<&crate::durable::inbox::StatusObservationSource>,
) -> AgentStatusView {
    let sections = status
        .sections
        .iter()
        .map(|section| match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
            } => RuntimeClientStatusSection::Temporal {
                current_time: *current_time,
                timezone: *timezone,
            },
            AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            } => RuntimeClientStatusSection::BackgroundExecutions {
                executions: executions
                    .iter()
                    .map(
                        |execution| super::snapshot::RuntimeClientStatusBackgroundExecution {
                            job_id: execution.execution_id.clone(),
                            tool_name: execution.tool_name.to_string(),
                            state: execution.state,
                            progress: execution.progress.clone(),
                        },
                    )
                    .collect(),
                omitted_count: *omitted_count,
            },
            AgentStatusSectionData::Todo { presentation } => RuntimeClientStatusSection::Todo {
                current: presentation.current.as_ref().map(todo_status_task_view),
                tasks: presentation
                    .tasks
                    .iter()
                    .map(todo_status_task_view)
                    .collect(),
                active_count: presentation.active_count,
                blocked_count: presentation.blocked_count,
                completed_count: presentation.completed_count,
                deleted_count: presentation.deleted_count,
                omitted_count: presentation.omitted_count,
            },
        })
        .collect();
    let fresh_inbound =
        opportunities
            .fresh_inbound
            .as_ref()
            .map(|fresh| FreshInboundOpportunityView {
                target_message_id: (fresh.target_message_id.as_str().len() <= 256)
                    .then(|| fresh.target_message_id.clone()),
                target_locator: source.and_then(|source| source.target_locator.clone()),
                transcript_anchor: source
                    .and_then(|source| source.target_transcript)
                    .map(Into::into),
                target_metadata_ref: source.and_then(|source| source.target_metadata_ref.clone()),
            });
    // The placement fact of a `PostToolBatch` composition is carried by the
    // observation, frozen by the Agent Loop at the tool batch's own commit.
    // It is copied, never derived.
    let post_tool_batch = opportunities
        .post_tool_batch
        .map(|_| PostToolBatchOpportunityView {
            transcript_anchor: post_tool_batch_anchor.map(RuntimeClientTranscriptCursor::from),
        });
    AgentStatusView {
        attempt_id: attempt_id.clone(),
        turn,
        status_message_id: (status_message_id.as_str().len() <= 256)
            .then(|| status_message_id.clone()),
        source_locator: source.map(|source| source.locator.clone()),
        request_id: source.map(|source| source.request_id.clone()),
        rendered_ref: source.and_then(|source| source.rendered_ref.clone()),
        opportunities: AgentStatusOpportunityView {
            fresh_inbound,
            post_tool_batch,
        },
        rendered: render_agent_status(status),
        sections,
    }
}

/// Admits one composed status into the bounded projection window and returns
/// the composition that admission pushed out, when the window was full.
///
/// Identity is the composed status message: re-observing the same
/// composition is idempotent rather than a second historical fact, and an
/// admission that does nothing evicts nothing.
///
/// This is the **only** retention decision in the system. The returned
/// eviction is published with the composition as one transition, so a client
/// folding incrementally reproduces this window without ever knowing
/// [`AGENT_STATUS_WINDOW`]. Because the window is only ever grown by one
/// before it is trimmed, one admission can retire at most one composition,
/// and the transition is fully described by a single optional identity.
fn admit_status(
    statuses: &mut Vec<AgentStatusView>,
    view: AgentStatusView,
) -> Option<AgentStatusView> {
    if statuses.iter().any(|existing| {
        if let Some(locator) = &view.source_locator {
            existing.source_locator.as_ref() == Some(locator)
        } else {
            view.status_message_id.is_some() && existing.status_message_id == view.status_message_id
        }
    }) {
        return None;
    }
    statuses.push(view);
    if statuses.len() > AGENT_STATUS_WINDOW {
        return Some(statuses.remove(0));
    }
    None
}

fn todo_status_task_view(task: &crate::tools::todo::TodoStatusTask) -> RuntimeClientTodoStatusTask {
    RuntimeClientTodoStatusTask {
        id: task.id,
        subject: task.subject.clone(),
        active_form: task.active_form.clone(),
        status: task.status,
        blocked: task.blocked,
    }
}

/// A complete canonical view refines C independently of the installed J state.
fn retain_tool_execution(source: &mut ForegroundToolExecution, current: &ForegroundToolExecution) {
    let committed = source.committed_result.take().map(|result| {
        (
            result,
            source.committed_result_deferred,
            source.result_metadata_ref.take(),
        )
    });
    source.clone_from(current);
    if let Some((result, deferred, metadata)) = committed {
        source.committed_result = Some(result);
        source.committed_result_deferred = deferred;
        source.result_metadata_ref = metadata;
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeClientError, RuntimeClientProjection, SubscriberPoll};
    use crate::agent::{
        AgentExecution, AgentExecutionObserver, AgentExecutionRequest, AgentStatusObservation,
    };
    use crate::context::{
        AgentStatus, AgentStatusEngine, CompactionBudgets, ContextEngine, ContextError,
        ContextErrorKind, ContextRuntime, ContributionOpportunities, FreshInboundOpportunity,
        PostToolBatchOpportunity,
    };
    use crate::conversation::ConversationState;
    use crate::events::interaction::{InteractionSettlement, InteractionSubject};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::ModelAdapter;
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::publication::{PublicationFrame, PublicationPayload};
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, EventId, InteractionId, MessageId, RequestId,
        ToolCallId, ToolId, TurnId,
    };
    use crate::runtime::inbound::{ConversationInboundMailbox, InitialTurnTrigger};
    use crate::runtime::interaction::{
        ApprovalDecision, InteractionKind, InteractionOutcome, InteractionRef, InteractionRequest,
        InteractionResponse, InteractionSource, RoutedInteraction,
    };
    use crate::runtime::observation::ConversationObservation;
    use crate::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
    use crate::runtime_client::event::{RuntimeClientEvent, RuntimeClientOutcome};
    use crate::runtime_client::snapshot::{
        AGENT_STATUS_WINDOW, AgentStatusOpportunityView, AgentStatusView, ForegroundToolState,
        InFlightBlock, RuntimeClientAttemptPhase, RuntimeClientStatusSection,
        RuntimeClientTranscriptCursor,
    };
    use crate::runtime_client::types::RuntimeClientCursor;
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
        ToolProgress,
    };
    use std::sync::{Arc, Mutex};

    fn attempt() -> AttemptId {
        AttemptId::new("attempt-1")
    }

    fn interaction_audit(
        request: &InteractionRequest,
        event_id: &str,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 1,
            conversation_id: request.conversation_id.clone(),
            attempt_id: Some(request.attempt_id.clone()),
            turn_id: Some(TurnId::new(request.turn.to_string())),
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc),
            event,
        }
    }

    /// A deterministic session model view for projection unit tests.
    fn model_view() -> crate::model::session::SessionModelView {
        crate::scripted_suites::support::model::scripted_session_model(std::sync::Arc::new(
            crate::scripted_suites::support::model::NullAdapter,
        ))
        .view()
    }

    fn projection() -> RuntimeClientProjection {
        RuntimeClientProjection::new(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                configured_tools: Vec::new(),
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            Some(model_view()),
            64,
        )
    }

    /// Adoption eligibility is runtime-domain projection state: a published
    /// change folds into the snapshot and publishes one event, and an
    /// unchanged value folds into nothing. It invalidates no derived read
    /// domain, because it is not conversation history.
    #[test]
    fn adoption_eligibility_publishes_only_changes() {
        use crate::local_runtime::configuration::application::AdoptionEligibility;
        let mut projection = projection();
        let (snapshot, cursor) = projection.snapshot().unwrap();
        assert_eq!(
            snapshot.configuration_adoption_eligibility,
            AdoptionEligibility::Unavailable
        );
        let (subscriber, _) = projection.subscribe(cursor).unwrap();
        for eligibility in [
            AdoptionEligibility::Unavailable,
            AdoptionEligibility::Eligible,
            AdoptionEligibility::Eligible,
            AdoptionEligibility::Busy,
            AdoptionEligibility::Busy,
            AdoptionEligibility::Eligible,
        ] {
            projection.apply(ConversationObservation::AdoptionEligibility(eligibility));
        }
        assert!(projection.read_domain_cut().is_none());
        let mut published = Vec::new();
        while let SubscriberPoll::Event(event) = projection.poll_subscriber(subscriber) {
            let crate::runtime_client::RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged {
                eligibility,
            } = single_fact(&event)
            else {
                panic!("only eligibility changes are published: {:?}", event.event);
            };
            published.push(eligibility.clone());
        }
        assert_eq!(
            published,
            vec![
                AdoptionEligibility::Eligible,
                AdoptionEligibility::Busy,
                AdoptionEligibility::Eligible
            ]
        );
        assert_eq!(
            projection
                .snapshot()
                .unwrap()
                .0
                .configuration_adoption_eligibility,
            AdoptionEligibility::Eligible
        );
    }

    #[test]
    fn t14_t20_workflow_native_revision_gaps_preserve_complete_replay_and_checkpoint() {
        let event: RuntimeClientEvent = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/workflow-v22.json"
        ))
        .unwrap();
        let RuntimeClientEvent::WorkflowsUpdated { mut workflows } = event else {
            panic!("original native Workflow fixture");
        };
        let (mut projection, _store) = initialized_source_projection();
        let mut folded = projection.checkpoint_candidate().unwrap();
        let initial = folded.clone();
        let (subscriber, _) = projection.subscribe(initial.cursor).unwrap();
        for revision in [37, 41] {
            workflows.revision.0 = revision;
            let source =
                super::super::workflow_source::WorkflowReadSource::new(Arc::new(workflows.clone()));
            source.prepare();
            publish_source_fact(&mut projection, |projection| {
                projection.apply(ConversationObservation::Workflow(vec![source]));
            });
            let SubscriberPoll::Event(record) = projection.poll_subscriber(subscriber) else {
                panic!("a complete native replacement remains replayable");
            };
            assert_eq!(record.cursor.get(), folded.cursor.get() + 1);
            assert!(
                projection
                    .replay
                    .iter()
                    .any(|(cursor, event)| { *cursor == record.cursor && event == &record.event })
            );
            let RuntimeClientEvent::ProjectionTransition { events, metadata } = &record.event
            else {
                panic!("one complete Runtime transition");
            };
            let [
                RuntimeClientEvent::WorkflowsUpdated {
                    workflows: replacement,
                },
            ] = events.as_slice()
            else {
                panic!("one positive Workflow replacement");
            };
            assert_eq!(replacement.revision.0, revision);
            folded.snapshot.workflows.clone_from(replacement);
            folded.cursor = record.cursor;
            folded.read_cut.clone_from(&metadata.read_cut);
            folded.collections.clone_from(&metadata.collections);
            folded.content.clone_from(&metadata.content);
            assert_eq!(folded, projection.checkpoint_candidate().unwrap());
            projection
                .validate_checkpoint_cut(&initial.read_cut)
                .unwrap();
            assert!(projection.initial_read_cut.is_some());
            assert_eq!(
                projection.replay.len(),
                usize::try_from(record.cursor.get()).unwrap()
            );
        }
        assert_eq!(projection.replay.len(), 2);
        assert_eq!(projection.snapshot_ref().workflows.revision.0, 41);
        assert!(
            projection
                .validate_domain_version(&initial.read_cut, "workflows.runs")
                .is_err()
        );
    }

    #[test]
    fn t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut() {
        use base64::Engine as _;
        let event: RuntimeClientEvent = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/workflow-v22.json"
        ))
        .unwrap();
        let RuntimeClientEvent::WorkflowsUpdated {
            workflows: original,
        } = event
        else {
            panic!("original native Workflow fixture");
        };
        assert!(!original.runs.is_empty());
        let source =
            super::super::workflow_source::WorkflowReadSource::new(Arc::new(original.clone()));
        source.prepare();
        let (mut projection, _store) = initialized_source_projection();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::Workflow(vec![source]));
        });
        let cut = projection.checkpoint_candidate().unwrap().read_cut;
        projection.validate_checkpoint_cut(&cut).unwrap();
        projection
            .validate_domain_version(&cut, "workflows.runs")
            .unwrap();
        let selected = projection
            .domain_source("workflows.runs", 0)
            .expect("subdomain selects the existing native Workflow owner");
        assert!(Arc::ptr_eq(
            &selected,
            &projection.domain_source("workflows", 0).unwrap()
        ));
        let page = selected
            .page(&cut, "workflows.runs", &["runs".into()], 0, 32, false)
            .unwrap();
        assert_eq!(page.read_cut, cut);
        assert_eq!(page.cursor.get(), cut.revision);
        assert_eq!(page.total, u64::try_from(original.runs.len()).unwrap());
        assert_eq!(page.items[0].offset, 0);
        let chunk = selected
            .read(
                &cut,
                "workflows.runs",
                0,
                &["runs".into(), "@0".into()],
                0,
                65_536,
            )
            .unwrap();
        assert_eq!(chunk.cut, cut);
        assert_eq!(chunk.offset, 0);
        assert_eq!(chunk.next_offset, None);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert_eq!(u64::try_from(bytes.len()).unwrap(), chunk.readable_end);
        let run: crate::runtime::workflow::read_model::WorkflowRunView =
            super::super::checkpoint::decode_domain_value(serde_json::from_slice(&bytes).unwrap())
                .unwrap();
        assert_eq!(run, original.runs[0]);

        let mut later = original;
        later.revision.0 += 1;
        let source = super::super::workflow_source::WorkflowReadSource::new(Arc::new(later));
        source.prepare();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::Workflow(vec![source]));
        });
        assert!(matches!(
            projection.validate_domain_version(&cut, "workflows.runs"),
            Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
        ));
    }

    #[test]
    fn queued_old_review_cannot_resurrect_a_settled_workflow_node() {
        let mut client = projection();
        let request: crate::runtime::interaction::InteractionRequest = serde_json::from_str(
            include_str!("../../tests/fixtures/runtime-client/review-v21.json"),
        )
        .unwrap();
        let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind
        else {
            panic!("Review fixture");
        };
        let node = (*review.instance).clone();
        let event: crate::runtime_client::RuntimeClientEvent = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/workflow-v22.json"
        ))
        .unwrap();
        let crate::runtime_client::RuntimeClientEvent::WorkflowsUpdated { mut workflows } = event
        else {
            panic!("Workflow fixture");
        };
        workflows.runs[0].id.clone_from(&node.block.run);
        workflows.runs[0].instances[0].block = node.block;
        workflows.runs[0].instances[0].node = Some(node.node);
        client
            .snapshot
            .pending_interactions
            .push(super::super::checkpoint::interaction_header(
                &crate::runtime::interaction::RoutedInteraction::primary(request),
            ));
        client.fold_workflows(workflows);
        let row = &client.snapshot.workflows.runs[0].instances[0];
        assert!(matches!(
            row.state,
            crate::runtime::workflow::read_model::WorkflowState::Settled { .. }
        ));
        assert!(row.interaction.is_none());
    }

    fn apply_event(projection: &mut RuntimeClientProjection, event: RuntimeEvent) {
        projection.apply(event_observation(event));
    }

    #[test]
    fn loop_facts_remain_journal_observations_without_client_execution_or_history() {
        use crate::runtime::workflow::{WorkflowExecutionOutcome, WorkflowLoopExit, test_instance};
        let mut projection = projection();
        let (before, before_cursor) = projection.snapshot().unwrap();
        let node = test_instance("feedback", "loop");
        let body = node.block.clone();
        for event in [
            RuntimeEvent::WorkflowLoopIterationAdmitted {
                node: node.clone(),
                body: body.clone(),
                iteration: 1,
            },
            RuntimeEvent::WorkflowLoopIterationSettled {
                node: node.clone(),
                body,
                iteration: 1,
                outcome: WorkflowExecutionOutcome::Completed,
            },
            RuntimeEvent::WorkflowLoopExited {
                node,
                iterations: 1,
                status: WorkflowLoopExit::Exhausted,
            },
        ] {
            projection.apply(event_observation(event));
        }
        let (after, after_cursor) = projection.snapshot().unwrap();
        assert_eq!(before_cursor, after_cursor);
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
    }

    /// Opens the representative publication stream on a projection.
    fn apply_publication_open(projection: &mut RuntimeClientProjection) {
        projection.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt(),
            start: stream_start(),
        });
    }

    /// Releases one already-committed publication frame into a projection.
    fn apply_frame(
        projection: &mut RuntimeClientProjection,
        sequence: u64,
        payload: PublicationPayload,
    ) {
        if projection.publication_start.is_none() {
            apply_publication_open(projection);
        }
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: frame(sequence, payload),
        });
    }

    fn text_frame(text: &str) -> PublicationPayload {
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: text.to_owned(),
        }
    }

    fn event_observation(event: RuntimeEvent) -> ConversationObservation {
        ConversationObservation::Event {
            attempt_id: attempt(),
            event,
        }
    }

    /// One composed status observation for the placement suites.
    ///
    /// `post_tool_batch_anchor` is the placement fact the Agent Loop froze at
    /// the tool batch commit; it is supplied by the caller exactly as the
    /// loop supplies it, never derived from projection state.
    fn status_observation(
        status_message_id: &str,
        turn: u32,
        opportunities: ContributionOpportunities,
        post_tool_batch_anchor: Option<crate::durable::TranscriptCursor>,
    ) -> AgentStatusObservation {
        AgentStatusObservation {
            attempt_id: attempt(),
            turn,
            status_message_id: MessageId::new(status_message_id),
            opportunities,
            post_tool_batch_anchor,
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        }
    }

    /// One `FreshInbound`-only composition: placed by the exact inbound
    /// message identity, which needs no transcript position.
    fn fresh_inbound_status(
        status_message_id: &str,
        turn: u32,
        target: &str,
    ) -> AgentStatusObservation {
        status_observation(
            status_message_id,
            turn,
            ContributionOpportunities {
                fresh_inbound: Some(FreshInboundOpportunity {
                    target_message_id: MessageId::new(target),
                }),
                post_tool_batch: None,
            },
            None,
        )
    }

    /// One `PostToolBatch`-only composition, carrying the durable position
    /// its settled tool batch froze.
    fn post_tool_batch_status(
        status_message_id: &str,
        turn: u32,
        batch_cursor: u64,
    ) -> AgentStatusObservation {
        status_observation(
            status_message_id,
            turn,
            ContributionOpportunities {
                fresh_inbound: None,
                post_tool_batch: Some(PostToolBatchOpportunity),
            },
            Some(crate::durable::TranscriptCursor::new(batch_cursor)),
        )
    }

    /// The transcript-position placement one composition publishes, if any.
    fn published_anchor(view: &AgentStatusView) -> Option<RuntimeClientTranscriptCursor> {
        view.opportunities
            .post_tool_batch
            .and_then(|batch| batch.transcript_anchor)
    }

    /// Commits one visible canonical message at a durable transcript
    /// position.
    fn commit_at(projection: &mut RuntimeClientProjection, id: &str, cursor: u64) {
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: MessageBlock::User(UserMessageBlock {
                id: MessageId::new(id),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "hello".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(cursor)),
        });
    }

    /// A `FreshInbound` composition is placed by the exact inbound message
    /// identity its opportunity carries, and publishes no transcript
    /// position: a message identity is the stronger fact, and nothing later
    /// can move it.
    #[test]
    fn fresh_inbound_status_is_placed_by_message_identity() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "user-1", 7);
        projection.apply(ConversationObservation::Status(super::status_view(
            &(fresh_inbound_status("status-1", 1, "user-1")),
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), 1);
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            Some(MessageId::new("user-1"))
        );
        assert_eq!(
            published_anchor(&snapshot.statuses[0]),
            None,
            "a FreshInbound composition needs no transcript position"
        );

        // A later commit changes nothing about an already-composed status.
        commit_at(&mut projection, "user-2", 9);
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            Some(MessageId::new("user-1"))
        );
    }

    /// A `PostToolBatch` composition publishes exactly the placement the
    /// Agent Loop froze at its tool batch commit — **not** whatever durable
    /// position the conversation happens to have reached when the projection
    /// folds the observation.
    ///
    /// This is the invariant that a fold-time frontier violated. Inbound
    /// acceptance is an independent durable boundary and may legally commit
    /// between the composition and its observation; the interleaving below is
    /// exactly that case, folded in the order it can really occur.
    #[test]
    fn post_tool_batch_status_keeps_the_anchor_its_batch_froze() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "user-1", 3);
        commit_at(&mut projection, "tool-result", 5);

        // Unrelated durable transcript activity lands before the status
        // observation reaches the projection.
        commit_at(&mut projection, "unrelated-inbound", 6);

        projection.apply(ConversationObservation::Status(super::status_view(
            &(post_tool_batch_status("status-post-batch", 2, 5)),
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let status = snapshot.statuses.last().expect("composed status");
        assert!(status.opportunities.fresh_inbound.is_none());
        assert_eq!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::new(5)),
            "the anchor is the tool batch's own frozen position, not the newest durable one"
        );
    }

    /// A new attempt is not a retraction: an earlier composition stays in the
    /// window, at its own placement, and a later one is added beside it.
    #[test]
    fn a_later_attempt_neither_removes_nor_relocates_an_earlier_status() {
        let mut projection = projection();
        commit_at(&mut projection, "user-1", 1);
        projection.apply(ConversationObservation::Status(super::status_view(
            &(fresh_inbound_status("status-1", 1, "user-1")),
        )));
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "tool-result", 4);
        projection.apply(ConversationObservation::Status(super::status_view(
            &(post_tool_batch_status("status-2", 1, 4)),
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot
                .statuses
                .iter()
                .map(|status| status
                    .status_message_id
                    .as_ref()
                    .expect("inline status identity")
                    .as_str()
                    .to_owned())
                .collect::<Vec<_>>(),
            vec!["status-1".to_owned(), "status-2".to_owned()]
        );
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            Some(MessageId::new("user-1"))
        );
        assert_eq!(
            published_anchor(&snapshot.statuses[1]),
            Some(RuntimeClientTranscriptCursor::new(4))
        );
    }

    /// Identity is the composed status message: observing one composition
    /// twice describes one historical fact, evicts nothing, and the window
    /// stays bounded.
    #[test]
    fn the_agent_status_window_is_identity_keyed_and_bounded() {
        let mut projection = projection();
        let mut client = LiveStatusClient::attach(&mut projection);
        let observation = post_tool_batch_status("status-1", 1, 1);
        projection.apply(ConversationObservation::Status(super::status_view(
            &(observation.clone()),
        )));
        projection.apply(ConversationObservation::Status(super::status_view(
            &(observation),
        )));
        client.drain(&mut projection);
        assert_eq!(
            client.evictions,
            vec![None, None],
            "neither the first admission nor its replay evicts anything"
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), 1);

        for index in 0..AGENT_STATUS_WINDOW + 5 {
            projection.apply(ConversationObservation::Status(super::status_view(
                &(post_tool_batch_status(
                    &format!("status-fill-{index}"),
                    1,
                    u64::try_from(index).expect("test index fits a cursor") + 2,
                )),
            )));
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), AGENT_STATUS_WINDOW);
        assert_eq!(
            snapshot.statuses.last().expect("newest").status_message_id,
            Some(MessageId::new(format!(
                "status-fill-{}",
                AGENT_STATUS_WINDOW + 4
            )))
        );
    }

    /// A continuously subscribed client that folds status-window
    /// transitions exactly as the TUI reducer does.
    ///
    /// It is deliberately dumb: it removes the identity the runtime says its
    /// own admission evicted, then appends by identity. It does not know
    /// `AGENT_STATUS_WINDOW`, does not count entries, and makes no retention
    /// decision of its own.
    struct LiveStatusClient {
        subscriber: u64,
        statuses: Vec<AgentStatusView>,
        evictions: Vec<Option<MessageId>>,
    }

    impl LiveStatusClient {
        fn attach(projection: &mut RuntimeClientProjection) -> Self {
            let (subscriber, _notify) = projection
                .subscribe(RuntimeClientCursor::new(0))
                .expect("serviceable cursor");
            Self {
                subscriber,
                statuses: Vec::new(),
                evictions: Vec::new(),
            }
        }

        /// Drains everything published so far and folds it.
        fn drain(&mut self, projection: &mut RuntimeClientProjection) {
            loop {
                match projection.poll_subscriber(self.subscriber) {
                    SubscriberPoll::Event(event) => {
                        for fact in event.event.facts() {
                            let RuntimeClientEvent::AgentStatusComposed {
                                status,
                                evicted_status_message_id,
                                ..
                            } = fact
                            else {
                                continue;
                            };
                            self.evictions.push(evicted_status_message_id.clone());
                            if let Some(evicted) = evicted_status_message_id {
                                self.statuses.retain(|existing| {
                                    existing.status_message_id.as_ref() != Some(evicted)
                                });
                            }
                            if !self.statuses.iter().any(|existing| {
                                existing.status_message_id == status.status_message_id
                            }) {
                                self.statuses.push(status.clone());
                            }
                        }
                    }
                    SubscriberPoll::Pending => return,
                    other => panic!("the status client lost its subscription: {other:?}"),
                }
            }
        }
    }

    /// **The bounded-window convergence invariant.**
    ///
    /// Folding every live event through cursor `C` and replacing state from
    /// the authoritative snapshot at cursor `C` must reconstruct the same
    /// semantic presentation state — past the retention bound as well as
    /// below it.
    ///
    /// The client here is deliberately dumb: it applies the eviction each
    /// event carries and appends by identity. It does not know
    /// `AGENT_STATUS_WINDOW`, does not count, and makes no retention
    /// decision, which is the entire point — with two retention owners this
    /// fold and the snapshot would diverge from the 65th distinct
    /// composition onwards.
    #[test]
    fn the_status_window_converges_between_live_fold_and_snapshot_repair() {
        let mut projection = projection();
        let mut live = LiveStatusClient::attach(&mut projection);
        let overshoot = 7;

        for index in 0..AGENT_STATUS_WINDOW + overshoot {
            let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
            projection.apply(ConversationObservation::Status(super::status_view(
                &(post_tool_batch_status(&format!("status-{index}"), 1, cursor)),
            )));
            live.drain(&mut projection);
        }
        let client = live.statuses;

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.statuses.len(),
            AGENT_STATUS_WINDOW,
            "the runtime window stays bounded"
        );
        assert_eq!(
            client, snapshot.statuses,
            "the live fold reconstructs the authoritative window exactly"
        );
        assert_eq!(
            client
                .iter()
                .map(|status| status
                    .status_message_id
                    .as_ref()
                    .expect("inline status identity")
                    .as_str()
                    .to_owned())
                .collect::<Vec<_>>(),
            (overshoot..AGENT_STATUS_WINDOW + overshoot)
                .map(|index| format!("status-{index}"))
                .collect::<Vec<_>>(),
            "retention and order are the runtime's, applied verbatim"
        );
        assert_eq!(
            client.last().map(|status| status.status_message_id.clone()),
            snapshot
                .statuses
                .last()
                .map(|status| status.status_message_id.clone()),
            "the newest composition agrees on both paths"
        );
        for (folded, authoritative) in client.iter().zip(&snapshot.statuses) {
            assert_eq!(
                published_anchor(folded),
                published_anchor(authoritative),
                "placement survives the fold unchanged"
            );
        }
    }

    /// Replay combined with eviction: re-observing a composition the window
    /// already holds must not displace another one.
    ///
    /// This is the failure a client-side retention rule invites — a second
    /// "append then trim" would drop the oldest entry on every replay, while
    /// the runtime's own window would not move at all.
    #[test]
    fn replaying_a_composition_evicts_nothing_from_a_full_window() {
        let mut projection = projection();
        let mut live = LiveStatusClient::attach(&mut projection);
        let mut observations = Vec::new();

        for index in 0..AGENT_STATUS_WINDOW + 3 {
            let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
            let observation = post_tool_batch_status(&format!("status-{index}"), 1, cursor);
            observations.push(observation.clone());
            projection.apply(ConversationObservation::Status(super::status_view(
                &(observation),
            )));
            live.drain(&mut projection);
        }
        let (before, _) = projection.snapshot().expect("snapshot");
        let evictions_before = live.evictions.len();

        // Replay the newest composition and one that is still retained.
        for observation in [
            observations.last().expect("newest").clone(),
            observations[AGENT_STATUS_WINDOW].clone(),
        ] {
            projection.apply(ConversationObservation::Status(super::status_view(
                &(observation),
            )));
            live.drain(&mut projection);
        }
        assert_eq!(
            live.evictions[evictions_before..],
            [None, None],
            "a replay is not an admission and evicts nothing"
        );

        let (after, _) = projection.snapshot().expect("snapshot");
        assert_eq!(after.statuses, before.statuses, "replay moves no window");
        assert_eq!(
            live.statuses, after.statuses,
            "the client fold agrees after replay"
        );
    }

    // -----------------------------------------------------------------------
    // The PostToolBatch anchor race.
    //
    // The Agent Loop's status linearization is:
    //
    // ```text
    // canonical ToolResult batch commits          <- placement determined here
    //   -> pending PostToolBatch opportunity
    //   -> next primary-step preparation
    //   -> Agent Status composed
    //   -> durable model-turn-start commit
    //   -> observe_status                          <- placement observed here
    // ```
    //
    // Inbound acceptance is an independent durable boundary and may commit
    // anywhere in that window. The test below parks the loop at
    // `observe_status`, durably accepts an unrelated inbound message while it
    // is parked, folds that acceptance into the Runtime Client projection, and
    // only then releases the status. Anything that derived placement at the
    // fold would produce the inbound's cursor; the frozen fact does not.
    // -----------------------------------------------------------------------

    /// An observer that folds every execution fact into a Runtime Client
    /// projection, but parks the first `PostToolBatch` status observation
    /// before it reaches the projection.
    ///
    /// The park is a real rendezvous: `observe_status` is a synchronous
    /// Agent Loop callback, so blocking in it blocks the loop itself. The
    /// controlling task therefore provably runs *between* the composition and
    /// its publication — not merely at some convenient moment.
    struct StatusParkObserver {
        projection: Arc<Mutex<RuntimeClientProjection>>,
        /// Every observed canonical commit with a durable position.
        commits: Mutex<Vec<(MessageId, crate::durable::TranscriptCursor)>>,
        /// Set when the loop is parked inside `observe_status`.
        parked: tokio::sync::watch::Sender<bool>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        released: Mutex<bool>,
    }

    impl StatusParkObserver {
        fn install(
            projection: Arc<Mutex<RuntimeClientProjection>>,
        ) -> (
            Arc<Self>,
            tokio::sync::watch::Receiver<bool>,
            std::sync::mpsc::Sender<()>,
        ) {
            let (parked, parked_rx) = tokio::sync::watch::channel(false);
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            (
                Arc::new(Self {
                    projection,
                    commits: Mutex::new(Vec::new()),
                    parked,
                    release: Mutex::new(release_rx),
                    released: Mutex::new(false),
                }),
                parked_rx,
                release_tx,
            )
        }

        fn apply(&self, observation: ConversationObservation) {
            self.projection
                .lock()
                .expect("projection lock")
                .apply(observation);
        }

        /// The durable position of the last committed tool message: the
        /// canonical `ToolResult` batch's own transcript frontier.
        fn tool_batch_cursor(&self) -> crate::durable::TranscriptCursor {
            *self
                .commits
                .lock()
                .expect("commit lock")
                .iter()
                .filter(|(id, _)| id.as_str().contains("-tool-"))
                .map(|(_, cursor)| cursor)
                .max()
                .expect("the attempt committed a canonical ToolResult batch")
        }
    }

    impl AgentExecutionObserver for StatusParkObserver {
        fn observe_event(
            &self,
            attempt_id: &AttemptId,
            event: &RuntimeEvent,
            _journal_sequence: u64,
            _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
            _receipt: Option<&crate::durable::observation::NativeObservationBatch>,
        ) {
            self.apply(ConversationObservation::Event {
                attempt_id: attempt_id.clone(),
                event: event.clone(),
            });
        }

        fn observe_committed(
            &self,
            attempt_id: &AttemptId,
            block: &MessageBlock,
            receipt: &crate::durable::inbox::TranscriptCommitReceipt,
        ) {
            if let Some(cursor) = receipt.transcript_cursor {
                self.commits
                    .lock()
                    .expect("commit lock")
                    .push((crate::conversation::message_id_of(block), cursor));
            }
            self.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: Some(attempt_id.clone()),
                receipt: receipt.clone(),
            });
        }

        fn observe_status(
            &self,
            observation: &AgentStatusObservation,
            _source: Option<&crate::durable::inbox::StatusObservationSource>,
        ) {
            let park = observation.opportunities.post_tool_batch.is_some()
                && observation.opportunities.fresh_inbound.is_none()
                && !*self.released.lock().expect("release flag lock");
            if park {
                self.parked.send_replace(true);
                // Blocks the Agent Loop here, before the Runtime Client can
                // fold anything about this composition.
                let _ = self.release.lock().expect("release lock").recv();
                *self.released.lock().expect("release flag lock") = true;
            }
            self.apply(ConversationObservation::Status(super::status_view(
                &(observation.clone()),
            )));
        }

        fn observe_publication_opened(
            &self,
            attempt_id: &AttemptId,
            start: &crate::publication::PublicationStreamStart,
        ) {
            self.apply(ConversationObservation::PublicationOpened {
                attempt_id: attempt_id.clone(),
                start: start.clone(),
            });
        }

        fn observe_publication(
            &self,
            attempt_id: &AttemptId,
            frame: &crate::publication::PublicationFrame,
            _receipt: &crate::publication::PublicationFrameReceipt,
        ) {
            self.apply(ConversationObservation::Publication {
                attempt_id: attempt_id.clone(),
                frame: frame.clone(),
            });
        }

        fn observe_publication_settled(
            &self,
            attempt_id: &AttemptId,
            _audit: &crate::publication::PublicationAudit,
            receipt: &crate::durable::inbox::PublicationAuditReceipt,
        ) {
            self.apply(ConversationObservation::AuditSettled {
                attempt_id: attempt_id.clone(),
                receipt: receipt.clone(),
            });
        }
    }

    /// The controlling half of the anchor race.
    ///
    /// Everything here runs strictly inside the park: the Agent Loop is
    /// blocked in `observe_status`, between the composition and its
    /// publication, for the whole body of this function. Returns the tool
    /// batch's own durable position and the later position the unrelated
    /// inbound was accepted at.
    async fn drive_anchor_race(
        observer: Arc<StatusParkObserver>,
        projection: Arc<Mutex<RuntimeClientProjection>>,
        mailbox: ConversationInboundMailbox,
        mut parked: tokio::sync::watch::Receiver<bool>,
        release: std::sync::mpsc::Sender<()>,
    ) -> (
        crate::durable::TranscriptCursor,
        crate::durable::TranscriptCursor,
    ) {
        parked
            .wait_for(|reached| *reached)
            .await
            .expect("the status park stays open");
        let batch_cursor = observer.tool_batch_cursor();

        // An independent durable boundary: a new inbound message is accepted,
        // and the durable transcript advances past the batch.
        mailbox
            .enqueue(UserMessageBlock {
                id: MessageId::new("unrelated-inbound"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "and another thing".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: Some(chrono::Utc::now()),
            })
            .expect("durable inbound acceptance");
        let batch = mailbox
            .select_pending_batch()
            .expect("select pending")
            .expect("the accepted inbound is pending");
        let item = batch
            .items()
            .iter()
            .find(|item| item.message().id == MessageId::new("unrelated-inbound"))
            .expect("the accepted inbound is in the pending batch")
            .clone();
        let inbound_cursor = item
            .transcript_cursor()
            .expect("a visible inbound message is a durable transcript item");
        assert!(
            inbound_cursor > batch_cursor,
            "the unrelated inbound is durably later than the tool batch"
        );

        // Fold the acceptance into the Runtime Client *before* the status.
        projection
            .lock()
            .expect("projection lock")
            .apply(ConversationObservation::InboundEnqueued(item));
        let (mid_flight, _) = projection
            .lock()
            .expect("projection lock")
            .snapshot()
            .expect("snapshot");
        assert!(
            mid_flight.statuses.is_empty(),
            "the parked composition has not been published yet"
        );
        assert_eq!(
            mid_flight
                .inbound
                .pending
                .last()
                .expect("the unrelated inbound is folded")
                .message
                .id,
            MessageId::new("unrelated-inbound"),
            "the projection has already folded the later durable position"
        );

        release.send(()).expect("release the parked status");
        (batch_cursor, inbound_cursor)
    }

    /// **The `PostToolBatch` placement invariant.**
    ///
    /// A `PostToolBatch` Agent Status is anchored to the durable transcript
    /// frontier of the tool batch that caused the composition. Later
    /// unrelated durable transcript activity cannot move that anchor.
    ///
    /// The interleaving is forced, not hoped for:
    ///
    /// ```text
    /// 1. canonical ToolResult batch commits at cursor N
    /// 2. the PostToolBatch opportunity freezes N
    /// 3. the loop PARKS in observe_status, before the projection sees it
    /// 4. an unrelated inbound is durably accepted at cursor N+1
    /// 5. its InboundEnqueued observation is folded
    /// 6. the park is RELEASED and the status is folded
    /// 7. the published anchor is still N
    /// ```
    ///
    /// Step 5 is asserted before step 6 runs, so the projection provably held
    /// the later position when it folded the composition. There are no
    /// sleeps and no timing assumptions anywhere: every ordering above is a
    /// rendezvous.
    /// The scripted provider of the anchor race: one turn that calls `todo`,
    /// then plain answers.
    ///
    /// The tool is `todo` on purpose. The Todo module is the one status module
    /// eligible for a `PostToolBatch` opportunity, so this batch both settles
    /// a canonical `ToolResult` batch and makes the next primary step compose
    /// a `PostToolBatch`-only status.
    fn anchor_race_model() -> Arc<FakeModel> {
        let call = crate::tools::types::ToolCall {
            id: ToolCallId::new("call-anchor-1"),
            tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
            name: "todo".to_owned(),
            arguments: serde_json::json!({ "action": "create", "subject": "Anchor the status" }),
        };
        let arguments_json =
            serde_json::to_string(&call.arguments).expect("serialize todo arguments");
        let stop_turn = || {
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "done".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]
        };
        Arc::new(FakeModel::new(vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::ToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: crate::tools::types::ToolCallStart {
                        id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                    },
                }),
                FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                    block_index: ContentBlockIndex::new(0),
                    call_id: call.id.clone(),
                    arguments_delta: arguments_json,
                }),
                FakeStep::Emit(ModelEvent::ToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call,
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::ToolCalls,
                    usage: None,
                }),
            ],
            stop_turn(),
            stop_turn(),
        ]))
    }

    /// A context runtime with a real Agent Status engine and no compaction
    /// pressure, so the only status decision under test is placement.
    fn anchor_race_context_runtime() -> ContextRuntime {
        ContextRuntime::with_scripted_summarizer(
            ContextEngine::new(
                crate::context::ContextConfig {
                    context_window_tokens: 200_000,
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                },
                Arc::new(ScriptedEstimator::new(1, 0, 0)),
            )
            .expect("valid test context engine"),
            Arc::new(FakeContextSummarizer::new(Vec::new())),
            Some(AgentStatusEngine::default()),
            CompactionBudgets::new(1, 1, 1_000_000),
        )
    }

    /// The attempt request of the anchor race: a pure continuation, so the
    /// only status opportunity this attempt can produce is `PostToolBatch`.
    fn anchor_race_request(
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
    ) -> AgentExecutionRequest {
        let adapter: Arc<dyn ModelAdapter> = model;
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id,
            attempt_id: AttemptId::new("attempt-anchor"),
            conversation: ConversationState::from_messages(vec![MessageBlock::User(
                UserMessageBlock {
                    id: MessageId::new("user-1"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "track the work".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    timestamp: None,
                },
            )])
            .expect("bootstrap conversation"),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            model: crate::scripted_suites::support::attempt_model_with_window(
                adapter,
                "status-anchor",
                200_000,
                64,
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn a_later_inbound_cannot_relocate_a_prepared_post_tool_batch_status() {
        let fixture = crate::scripted_suites::common::native_fixture();
        let capability = crate::scripted_suites::common::capability_lease(
            fixture.ordinary_registry.clone(),
            &fixture.runtime,
        )
        .await;
        let request = anchor_race_request(
            fixture.runtime.conversation_id().clone(),
            anchor_race_model(),
        );
        let projection = Arc::new(Mutex::new(projection()));
        let (observer, parked, release) = StatusParkObserver::install(projection.clone());

        let controller = tokio::spawn(drive_anchor_race(
            observer.clone(),
            projection.clone(),
            fixture.mailbox.clone(),
            parked,
            release,
        ));

        let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
        let mut execution = AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            anchor_race_context_runtime(),
            &fixture.runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.observe(observer.as_ref());
        let _result = execution.run().await;
        let (batch_cursor, inbound_cursor) = controller.await.expect("controller task");

        let (snapshot, _) = projection
            .lock()
            .expect("projection lock")
            .snapshot()
            .expect("snapshot");
        let status = snapshot
            .statuses
            .iter()
            .find(|status| status.opportunities.post_tool_batch.is_some())
            .expect("the attempt composed a PostToolBatch status");
        assert_eq!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::from(batch_cursor)),
            "the status keeps the position its own tool batch froze"
        );
        assert_ne!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::from(inbound_cursor)),
            "an unrelated inbound accepted before the fold cannot relocate the status"
        );
    }

    #[test]
    fn agent_status_opportunity_view_allows_absent_fresh_inbound() {
        let observation = AgentStatusObservation {
            attempt_id: attempt(),
            turn: 1,
            status_message_id: MessageId::new("status-1"),
            opportunities: ContributionOpportunities::default(),
            post_tool_batch_anchor: None,
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        };

        let view = super::status_view(&observation);
        assert!(view.opportunities.fresh_inbound.is_none());

        let encoded = serde_json::to_value(&view.opportunities).expect("serialize opportunity");
        assert_eq!(encoded, serde_json::json!({}));
        let decoded: AgentStatusOpportunityView =
            serde_json::from_value(encoded).expect("deserialize opportunity");
        assert_eq!(decoded, view.opportunities);
    }

    #[test]
    fn agent_status_opportunity_view_publishes_one_combined_observation() {
        let status_message_id = MessageId::new("status-combined");
        let observation = AgentStatusObservation {
            attempt_id: attempt(),
            turn: 2,
            status_message_id: status_message_id.clone(),
            opportunities: ContributionOpportunities {
                fresh_inbound: Some(FreshInboundOpportunity {
                    target_message_id: MessageId::new("fresh-inbound"),
                }),
                post_tool_batch: Some(PostToolBatchOpportunity),
            },
            post_tool_batch_anchor: Some(crate::durable::TranscriptCursor::new(6)),
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        };

        let view = super::status_view(&observation);
        assert_eq!(view.status_message_id, Some(status_message_id));
        assert_eq!(
            view.opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id
                .clone(),
            Some(MessageId::new("fresh-inbound"))
        );
        assert_eq!(
            published_anchor(&view),
            Some(RuntimeClientTranscriptCursor::new(6)),
            "the PostToolBatch half carries the placement its batch froze"
        );
        let encoded = serde_json::to_value(&view).expect("serialize combined status view");
        assert_eq!(
            encoded["opportunities"]["post_tool_batch"],
            serde_json::json!({ "transcript_anchor": 6 })
        );
        assert!(
            encoded.get("transcript_anchor").is_none(),
            "placement lives on the opportunity that froze it, never on the view itself"
        );
        assert_eq!(
            encoded["status_message_id"],
            serde_json::json!("status-combined")
        );
    }

    #[test]
    fn todo_status_section_round_trips_the_wire_shape() {
        let wire = serde_json::json!({
            "type": "todo",
            "current": {
                "id": 7,
                "subject": "Review the boundary",
                "active_form": "Reviewing the boundary",
                "status": "in_progress",
                "blocked": false
            },
            "tasks": [{
                "id": 8,
                "subject": "Write the regression",
                "status": "pending",
                "blocked": true
            }],
            "active_count": 2,
            "blocked_count": 1,
            "completed_count": 3,
            "deleted_count": 1,
            "omitted_count": 0
        });
        let section: RuntimeClientStatusSection =
            serde_json::from_value(wire.clone()).expect("decode the Runtime Client Todo section");
        assert_eq!(
            serde_json::to_value(section).expect("encode the Runtime Client Todo section"),
            wire
        );
    }

    fn single_fact(
        event: &crate::runtime_client::types::RuntimeClientProtocolEvent,
    ) -> &RuntimeClientEvent {
        let RuntimeClientEvent::ProjectionTransition { events, metadata } = &event.event else {
            panic!("complete native transition: {:?}", event.event);
        };
        assert_eq!(metadata.read_cut.revision, event.cursor.get());
        let [fact] = events.as_slice() else {
            panic!("one complete fact at this cursor: {events:?}");
        };
        fact
    }

    fn collect(
        projection: &mut RuntimeClientProjection,
        after: RuntimeClientCursor,
    ) -> Vec<crate::runtime_client::types::RuntimeClientProtocolEvent> {
        let (id, _notify) = projection.subscribe(after).expect("serviceable cursor");
        let mut events = Vec::new();
        loop {
            match projection.poll_subscriber(id) {
                SubscriberPoll::Event(event) => events.push(event),
                SubscriberPoll::Pending => break,
                other => panic!("unexpected subscriber poll: {other:?}"),
            }
        }
        projection.remove_subscriber(id);
        events
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CompactionOrderFact {
        /// The typed checkpoint and local completion fact committed atomically.
        RuntimeCompactionCompleted,
        ClientContextCompacted,
        ClientAttemptSettled,
    }

    struct EventPathObserver {
        store: Arc<dyn crate::durable::ConversationStore>,
        projection: Mutex<RuntimeClientProjection>,
        subscriber: u64,
        facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
    }

    impl EventPathObserver {
        fn new(
            initial_messages: Vec<MessageBlock>,
            facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
            store: Arc<dyn crate::durable::ConversationStore>,
        ) -> Self {
            let mut projection = RuntimeClientProjection::new(
                ConversationId::new("conv_d8b669b8-1b38-797b-8db2-c56430c1741d"),
                initial_messages,
                crate::runtime_client::snapshot::CapabilityView {
                    configured_tools: Vec::new(),
                    revision: crate::runtime::identity::CapabilityRevision::new(1),
                    tools: Vec::new(),
                    available_tools: Vec::new(),
                    skills: Vec::new(),
                    sources: Vec::new(),
                },
                Some(model_view()),
                64,
            );
            projection
                .initialize_descriptor_heads(store.as_ref(), store.observation_read_cut().unwrap())
                .unwrap();
            let (subscriber, _notify) = projection
                .subscribe(RuntimeClientCursor::new(0))
                .expect("subscribe projection observer");
            Self {
                store,
                projection: Mutex::new(projection),
                subscriber,
                facts,
            }
        }

        fn release_native_batch(
            &self,
            receipt: &crate::durable::observation::NativeObservationBatch,
        ) {
            if let Some(canonical) = &receipt.canonical {
                self.store
                    .mark_canonical_transcript_ready(
                        canonical.first_position,
                        canonical.last_position,
                    )
                    .unwrap();
            }
            if let Some((first, last)) = receipt.journal_range {
                for sequence in first..=last {
                    if let Some(cursor) = self
                        .store
                        .event_transcript_cursor_at_sequence(sequence)
                        .unwrap()
                    {
                        self.store
                            .mark_observation_transcript_ready(cursor.get())
                            .unwrap();
                    }
                }
                self.store.mark_observation_direct(first, last).unwrap();
            }
            let ready = self.store.transcript_source_ready().unwrap();
            let mut projection = self.projection.lock().unwrap();
            projection.apply(ConversationObservation::TranscriptSourceReady(ready));
            self.drain_client_events(&mut projection);
        }

        fn drain_client_events(&self, projection: &mut RuntimeClientProjection) {
            loop {
                match projection.poll_subscriber(self.subscriber) {
                    SubscriberPoll::Event(event) => {
                        for fact in event.event.facts() {
                            match fact {
                                RuntimeClientEvent::ContextCompacted { .. } => self
                                    .facts
                                    .lock()
                                    .expect("order facts lock")
                                    .push(CompactionOrderFact::ClientContextCompacted),
                                RuntimeClientEvent::AttemptSettled { .. } => self
                                    .facts
                                    .lock()
                                    .expect("order facts lock")
                                    .push(CompactionOrderFact::ClientAttemptSettled),
                                _ => {}
                            }
                        }
                    }
                    SubscriberPoll::Pending => return,
                    other => panic!("projection observer lost its subscription: {other:?}"),
                }
            }
        }

        fn snapshot(&self) -> crate::runtime_client::snapshot::RuntimeClientSnapshot {
            self.projection
                .lock()
                .expect("projection lock")
                .snapshot()
                .expect("projection snapshot")
                .0
        }
    }

    impl AgentExecutionObserver for EventPathObserver {
        fn observe_event(
            &self,
            _attempt_id: &AttemptId,
            event: &RuntimeEvent,
            journal_sequence: u64,
            _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
            receipt: Option<&crate::durable::observation::NativeObservationBatch>,
        ) {
            if matches!(event, RuntimeEvent::CompactionCompleted { .. }) {
                self.facts
                    .lock()
                    .expect("order facts lock")
                    .push(CompactionOrderFact::RuntimeCompactionCompleted);
            }
            let receipt = receipt.expect("the actual native Journal receipt");
            if receipt
                .journal_range
                .is_some_and(|(_, last)| journal_sequence < last)
            {
                return;
            }
            {
                let mut projection = self.projection.lock().expect("projection lock");
                projection.apply_native_batch(receipt);
                self.drain_client_events(&mut projection);
            }
            self.release_native_batch(receipt);
        }

        fn observe_committed(
            &self,
            attempt_id: &AttemptId,
            _block: &MessageBlock,
            receipt: &crate::durable::inbox::TranscriptCommitReceipt,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: Some(attempt_id.clone()),
                receipt: receipt.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_status(
            &self,
            observation: &AgentStatusObservation,
            source: Option<&crate::durable::inbox::StatusObservationSource>,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::Status(
                super::prepared_status_view(observation, source),
            ));
            self.drain_client_events(&mut projection);
        }

        fn observe_publication_opened(
            &self,
            attempt_id: &AttemptId,
            start: &crate::publication::PublicationStreamStart,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::PublicationOpened {
                attempt_id: attempt_id.clone(),
                start: start.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_publication(
            &self,
            attempt_id: &AttemptId,
            frame: &crate::publication::PublicationFrame,
            _receipt: &crate::publication::PublicationFrameReceipt,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::Publication {
                attempt_id: attempt_id.clone(),
                frame: frame.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_publication_settled(
            &self,
            attempt_id: &AttemptId,
            _audit: &crate::publication::PublicationAudit,
            receipt: &crate::durable::inbox::PublicationAuditReceipt,
        ) {
            {
                let mut projection = self.projection.lock().expect("projection lock");
                projection.apply(ConversationObservation::AuditSettled {
                    attempt_id: attempt_id.clone(),
                    receipt: receipt.clone(),
                });
                self.drain_client_events(&mut projection);
            }
            self.store
                .mark_observation_transcript_ready(receipt.transcript_cursor.get())
                .unwrap();
            let ready = self.store.transcript_source_ready().unwrap();
            let mut projection = self.projection.lock().unwrap();
            projection.apply(ConversationObservation::TranscriptSourceReady(ready));
            self.drain_client_events(&mut projection);
        }
    }

    fn compactable_user(id: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "history".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    async fn run_compaction_order_case(
        fail_summary: bool,
    ) -> (
        crate::scripted_suites::common::DurableExecutionAudit,
        Arc<EventPathObserver>,
        Arc<Mutex<Vec<CompactionOrderFact>>>,
    ) {
        let facts = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(FakeModel::new(vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]]));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let model_snapshot = crate::scripted_suites::support::attempt_model_with_window(
            adapter,
            "projection-order",
            200,
            1,
        );
        let initial_messages = vec![compactable_user("old-1"), compactable_user("old-2")];
        let engine = ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 200,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(ScriptedEstimator::new(120, 0, 0)),
        )
        .expect("valid test context engine");
        let summary_step = if fail_summary {
            FakeSummaryStep::Fail(ContextError::new(
                ContextErrorKind::SummaryFailed,
                "scripted summary failure",
            ))
        } else {
            FakeSummaryStep::Return("summary".to_owned())
        };
        let runtime = ContextRuntime::with_scripted_summarizer(
            engine,
            Arc::new(FakeContextSummarizer::new(vec![summary_step])),
            Some(AgentStatusEngine::default()),
            CompactionBudgets::new(1, 1, 1_000_000),
        );
        let tool_runtime = crate::scripted_suites::common::tool_runtime(
            "conv_d8b669b8-1b38-797b-8db2-c56430c1741d",
        );
        let store = tool_runtime.durable_store();
        let capability =
            crate::scripted_suites::common::capability_lease(ToolRegistry::new(), &tool_runtime)
                .await;
        let request = AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("conv_d8b669b8-1b38-797b-8db2-c56430c1741d"),
            attempt_id: AttemptId::new("attempt-order"),
            conversation: ConversationState::from_messages(initial_messages.clone())
                .expect("bootstrap conversation"),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            model: model_snapshot,
        };
        let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
        let mut execution = AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        let observer = Arc::new(EventPathObserver::new(
            initial_messages,
            facts.clone(),
            store.clone(),
        ));
        execution.observe(observer.as_ref());
        let result = execution.run().await;
        let result = crate::scripted_suites::common::durable_agent_result(result, store.as_ref());
        (result, observer, facts)
    }

    /// A successful committed compaction has one ordered fact path: the
    /// immutable checkpoint activation/receipt, the actual local completion event, the
    /// Runtime Client event, and terminal settlement.
    ///
    /// The ordering is the client-visible invariant: no `ContextCompacted`
    /// may imply success before immutable content and the durable Projection Head exist.
    #[tokio::test]
    async fn committed_compaction_order_is_canonical_and_terminal_last() {
        let (result, observer, facts) = run_compaction_order_case(false).await;
        assert_eq!(
            *facts.lock().expect("order facts lock"),
            vec![
                CompactionOrderFact::RuntimeCompactionCompleted,
                CompactionOrderFact::ClientContextCompacted,
                CompactionOrderFact::ClientAttemptSettled,
            ]
        );
        assert!(matches!(
            result.event_history.last(),
            Some(RuntimeEvent::AttemptCompleted { .. })
        ));
        assert_eq!(result.conversation.surface().compaction_generation(), 1);
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.context.compaction_count, 1);
        let latest = snapshot
            .context
            .latest_compaction
            .expect("latest compaction");
        assert_eq!(latest.generation, 1);
        assert_eq!(
            latest.activation_id,
            result
                .event_history
                .iter()
                .find_map(|event| match event {
                    RuntimeEvent::CompactionCompleted { activation_id, .. } => Some(*activation_id),
                    _ => None,
                })
                .expect("one compaction completion"),
            "the client view carries the revision the rewrite established"
        );
        assert_eq!(
            snapshot.messages.len(),
            result.canonical_history.len(),
            "client transcript contains only canonical execution facts"
        );
    }

    /// A failed compaction publishes its live start/failure lifecycle and
    /// terminal settlement, but never commits a canonical summary and never
    /// emits canonical or client completion.
    #[tokio::test]
    async fn failed_compaction_has_no_summary_commit_or_client_event() {
        let (result, observer, facts) = run_compaction_order_case(true).await;
        assert_eq!(
            *facts.lock().expect("order facts lock"),
            vec![CompactionOrderFact::ClientAttemptSettled]
        );
        assert!(
            result
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
        );
        assert!(
            result
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
        );
        assert!(
            result
                .event_history
                .iter()
                .all(|event| { !matches!(event, RuntimeEvent::CompactionCompleted { .. }) })
        );
        assert!(matches!(
            result.event_history.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
        assert_eq!(result.conversation.surface().compaction_generation(), 0);
        assert!(
            result.projections.is_empty(),
            "failed maintenance installs no projection"
        );
        let snapshot = observer.snapshot();
        assert!(!snapshot.context.compaction_in_progress);
        assert_eq!(snapshot.context.compaction_count, 0);
        assert!(snapshot.context.latest_compaction.is_none());
    }

    /// Canonical completion events advance the external context view exactly
    /// once per committed generation and preserve token-measurement
    /// provenance.
    #[test]
    fn committed_compactions_are_visible_without_exposing_summary_content() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::CompactionCompleted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
                generation: 1,
                checkpoint_id: crate::runtime::identity::CheckpointId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
                ),
                activation_id: crate::context::checkpoint::ProjectionActivationId(3),
                tokens_before: TokenMeasurement {
                    input_tokens: 4800,
                    source: TokenMeasurementSource::ProviderReported,
                },
                estimated_tokens_after: 1700,
                occupancy: None,
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::CompactionCompleted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
                generation: 2,
                checkpoint_id: crate::runtime::identity::CheckpointId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-2",
                ),
                activation_id: crate::context::checkpoint::ProjectionActivationId(6),
                tokens_before: TokenMeasurement {
                    input_tokens: 4700,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1800,
                occupancy: None,
            },
        );

        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.messages, Vec::new());
        assert_eq!(snapshot.context.compaction_count, 2);
        let latest = snapshot
            .context
            .latest_compaction
            .expect("latest compaction");
        assert_eq!(latest.generation, 2);
        assert_eq!(
            latest.tokens_before.source,
            TokenMeasurementSource::Estimated
        );
        assert_eq!(latest.estimated_tokens_after, 1800);

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 2);
        assert!(events[0].event.facts().iter().any(
            |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { attempt_id, context }
                if attempt_id.as_ref() == Some(&attempt())
                    && context.compaction_count == 1
                    && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 1))
        ));
        assert!(events[1].event.facts().iter().any(
            |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { context, .. }
                if context.compaction_count == 2
                    && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 2))
        ));
        assert_eq!(cursor, RuntimeClientCursor::new(2));
    }

    fn success_result() -> ToolExecutionResult {
        ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        }
    }

    fn cancelled_result(
        reason: CancellationReason,
        phase: ToolCancellationPhase,
    ) -> ToolExecutionResult {
        ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Cancelled { reason, phase },
            content: Vec::new(),
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        }
    }

    fn apply_assembled_call(projection: &mut RuntimeClientProjection, call: &ToolCall) {
        apply_frame(
            projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
    }

    #[test]
    fn journal_batch_invalidates_trace_even_when_session_events_publish() {
        use crate::events::types::RuntimeEventEnvelope;
        use crate::runtime::identity::EventId;
        use crate::runtime::observation::PendingObservations;
        let mut projection = projection();
        let queue = PendingObservations::new();
        let facts = [
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
            RuntimeEvent::TurnStarted,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call"),
                tool_id: ToolId::new("tool"),
            },
        ];
        for (index, event) in facts.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            queue.committed_fixture(Some(vec![RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                event_id: EventId::new(format!("trace-{sequence}")),
                sequence,
                conversation_id: projection.snapshot_ref().conversation_id.clone(),
                attempt_id: Some(attempt()),
                turn_id: None,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                event: event.clone(),
            }]));
            queue.push(ConversationObservation::Published {
                journal_sequence: sequence,
                observation: Box::new(event_observation(event)),
            });
        }
        let batches = queue.drain();
        assert!(matches!(
            batches.as_slice(),
            [ConversationObservation::JournalBatch {
                through: Some(3),
                ..
            }]
        ));
        for batch in batches {
            projection.apply(batch);
        }
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert!(events.iter().any(|e| {
            e.event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::ToolExecutionStarted { .. }))
        }));
        assert!(events.iter().any(|e| {
            e.event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        }));
        assert!(events.iter().any(|e| {
            e.event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { .. }))
        }));
        assert_eq!(
            events
                .iter()
                .filter(|e| e
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::TraceChanged)))
                .count(),
            1
        );
    }

    #[test]
    fn failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut() {
        use futures_util::FutureExt as _;
        let mut projection = projection();
        let (subscriber, wake) = projection.subscribe(RuntimeClientCursor::new(0)).unwrap();
        assert!(wake.notified().now_or_never().is_none());
        projection.apply(ConversationObservation::JournalBatch {
            through: None,
            observations: Vec::new(),
        });
        assert!(wake.notified().now_or_never().is_some());
        assert_eq!(
            projection.poll_subscriber(subscriber),
            SubscriberPoll::Unavailable {
                message: "native Journal observation source unavailable".into(),
            }
        );
        assert!(matches!(
            projection.subscribe(RuntimeClientCursor::new(0)),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        assert!(matches!(
            projection.validate_published_source(),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        assert_eq!(projection.cursor(), RuntimeClientCursor::new(0));
    }

    #[test]
    fn trace_progress_retains_dependency_revision_but_retires_the_older_journal_cut() {
        let mut projection = projection();
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        let cut = projection.read_domain_cut().unwrap();
        projection.apply(ConversationObservation::JournalBatch {
            through: Some(9),
            observations: vec![event_observation(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("trace-only-call"),
                tool_id: ToolId::new("tool"),
            })],
        });
        let current = projection.read_domain_cut().unwrap();
        assert_eq!(current.revision, cut.revision);
        assert_eq!(current.through, 9);
        assert_eq!(projection.snapshot_cut().unwrap().2, 9);
        assert!(!projection.install_read_domains(
            &cut,
            Ok(super::ReadDomainValues {
                projection_head: None,
                transcript: super::super::snapshot::RuntimeClientTranscriptPage::default(),
                occupancy: None,
                trace: Some(super::super::trace::TracePage::default()),
                trace_total: 0,
            }),
            true
        ));
    }

    #[test]
    fn t14_t19_worker_read_fence_keeps_one_native_cut_before_publication() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let initial = projection.checkpoint_candidate().unwrap();
        let first = store
            .append_event(source_tool_event(
                &store,
                RuntimeEvent::TurnCompleted,
                "worker-cut-first",
            ))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(first.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(first.sequence, first.sequence)
            .unwrap();
        let first_published = projection.checkpoint_candidate().unwrap();
        assert_eq!(first_published.cursor.get(), initial.cursor.get() + 1);
        assert_eq!(first_published.read_cut.through, first.sequence);
        assert_eq!(first_published.snapshot.trace, initial.snapshot.trace);
        assert!(
            projection
                .replay
                .back()
                .unwrap()
                .1
                .facts()
                .iter()
                .any(|fact| { matches!(fact, RuntimeClientEvent::TraceChanged) })
        );
        let old_fence = projection.read_domain_cut().unwrap();
        assert_eq!(old_fence.through, first.sequence);
        assert_eq!(
            old_fence.source_cut,
            Some(store.observation_read_cut().unwrap())
        );
        let old_read = old_fence.read(&store).unwrap();
        assert_eq!(
            old_read
                .transcript
                .source_cut
                .as_ref()
                .unwrap()
                .read
                .journal,
            old_fence.through
        );
        let mut second_event =
            source_tool_event(&store, RuntimeEvent::TurnCompleted, "worker-cut-second");
        second_event.turn_id = Some(TurnId::new("2"));
        let second = store.append_event(second_event).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(second.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(second.sequence, second.sequence)
            .unwrap();
        let current = projection.read_domain_cut().unwrap();
        assert_eq!(current.through, second.sequence);
        assert_eq!(
            current.source_cut,
            Some(store.observation_read_cut().unwrap())
        );
        assert!(
            !projection.install_read_domains(&old_fence, Ok(old_read), true),
            "a coherent older worker read still cannot install at a newer native fence"
        );
        let second_published = projection.checkpoint_candidate().unwrap();
        assert_eq!(second_published.cursor.get(), initial.cursor.get() + 2);
        assert_eq!(second_published.read_cut.through, second.sequence);
        assert_eq!(second_published.snapshot.trace, initial.snapshot.trace);
        let read = current.read(&store).unwrap();
        publish_source_fact(&mut projection, |projection| {
            assert!(projection.install_read_domains(&current, Ok(read), true));
        });
        let published = projection.checkpoint_candidate().unwrap();
        assert_eq!(published.cursor.get(), initial.cursor.get() + 3);
        assert_eq!(published.read_cut.through, second.sequence);
        assert_eq!(published.read_cut.source_cut, current.source_cut);
        assert_eq!(
            published
                .snapshot
                .transcript
                .source_cut
                .as_ref()
                .unwrap()
                .read
                .journal,
            second.sequence
        );
    }

    #[test]
    fn display_reads_follow_observers_and_explicit_checkpoint_demand() {
        let mut projection = projection();
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        let cut = projection.read_domain_cut().unwrap();
        assert!(projection.read_domain_candidate().is_none());
        let cursor = projection.cursor();
        let (subscriber, _) = projection.subscribe(cursor).unwrap();
        assert_eq!(projection.read_domain_candidate(), Some(cut.clone()));
        projection.remove_subscriber(subscriber);
        assert!(projection.read_domain_candidate().is_none());
        assert_eq!(projection.read_domain_cut(), Some(cut.clone()));
        assert_eq!(projection.request_read_domains(), Some((cut.clone(), true)));
        assert_eq!(
            projection.request_read_domains(),
            Some((cut.clone(), false))
        );
        assert_eq!(projection.read_domain_candidate(), Some(cut.clone()));
        assert_eq!(
            projection.cursor(),
            cursor,
            "demand itself publishes nothing"
        );
        let domains = super::ReadDomainValues {
            projection_head: projection.snapshot.context.projection_head.clone(),
            transcript: projection.snapshot.transcript.clone(),
            occupancy: projection.snapshot.context.occupancy.clone(),
            trace: Some(projection.snapshot.trace.clone()),
            trace_total: 0,
        };
        assert!(projection.install_read_domains(&cut, Ok(domains), false));
        assert!(!projection.read_domains_requested);
        assert!(projection.read_domain_candidate().is_none());
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        assert!(projection.read_domain_cut().is_some());
        assert!(
            projection.read_domain_candidate().is_none(),
            "a completed request is not a permanent observer"
        );
    }

    #[test]
    fn dormant_display_preserves_serviceable_semantic_replay() {
        let mut projection = projection();
        let cursor = projection.cursor();
        projection.apply(event_observation(RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        }));
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        projection.apply(event_observation(RuntimeEvent::TurnCompleted));
        assert!(projection.read_domain_candidate().is_none());
        let published = projection.cursor();
        assert!(published > cursor);
        let (subscriber, _) = projection.subscribe(cursor).unwrap();
        let mut consumed = cursor;
        while consumed < published {
            let super::SubscriberPoll::Event(event) = projection.poll_subscriber(subscriber) else {
                panic!("every retained semantic event must remain replayable");
            };
            assert_eq!(event.cursor.get(), consumed.get() + 1);
            consumed = event.cursor;
        }
        assert_eq!(
            projection.poll_subscriber(subscriber),
            super::SubscriberPoll::Pending
        );
        assert!(projection.read_domain_candidate().is_some());
    }

    #[test]
    fn read_failure_requires_successful_source_read_at_the_current_dependency_revision() {
        let mut projection = projection();
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        let failed = projection.read_domain_cut().unwrap();
        assert!(projection.install_read_domains(
            &failed,
            Err(crate::durable::ConversationStoreError::InvalidReference(
                "transient".into()
            )),
            true
        ));
        assert!(projection.read_failure.is_some());
        assert!(projection.read_domain_cut().is_none());
        let cursor = projection.cursor();
        assert_eq!(
            projection.request_read_domains(),
            Some((failed.clone(), true))
        );
        assert_eq!(
            projection.request_read_domains(),
            Some((failed.clone(), false))
        );
        assert_eq!(
            projection.cursor(),
            cursor,
            "concurrent requests coalesce at one native fence"
        );
        assert!(projection.install_read_domains(
            &failed,
            Err(crate::durable::ConversationStoreError::InvalidReference(
                "still unavailable".into()
            )),
            true,
        ));
        projection.apply(ConversationObservation::JournalSourceReady);
        assert!(
            projection.read_domain_cut().is_none(),
            "a late wake cannot restart a completed failed read"
        );
        assert_eq!(projection.cursor(), cursor);
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        let next = projection.read_domain_cut().unwrap();
        assert_ne!(next, failed);
        assert!(
            projection.read_failure.is_some(),
            "a new dependency is not proof that its Source read succeeded"
        );
        let domains = super::ReadDomainValues {
            projection_head: projection.snapshot.context.projection_head.clone(),
            transcript: projection.snapshot.transcript.clone(),
            occupancy: None,
            trace: Some(super::super::trace::TracePage::default()),
            trace_total: 0,
        };
        assert!(!projection.install_read_domains(&failed, Ok(domains.clone()), true));
        assert_eq!(projection.read_domain_cut(), Some(next.clone()));
        assert!(projection.install_read_domains(&next, Ok(domains), true));
        assert!(projection.read_failure.is_none());
        assert!(projection.read_domain_cut().is_none());
        // An older in-flight read for the same fence cannot poison a cut which
        // another successful owner has already established.
        assert!(!projection.install_read_domains(
            &next,
            Err(crate::durable::ConversationStoreError::InvalidReference(
                "late failure".into()
            )),
            true
        ));
        assert!(projection.snapshot().is_ok());
    }

    #[test]
    fn durable_read_cut_retires_stale_results_without_overwriting_semantics() {
        let mut projection = projection();
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        let old = projection.read_domain_cut().unwrap();
        projection.apply(ConversationObservation::Shutdown);
        assert_eq!(
            projection.read_domain_cut(),
            Some(old.clone()),
            "shutdown does not invalidate durable domains"
        );
        projection.apply(event_observation(RuntimeEvent::TurnStarted));
        assert!(!projection.install_read_domains(
            &old,
            Ok(super::ReadDomainValues {
                projection_head: None,
                transcript: super::super::snapshot::RuntimeClientTranscriptPage::default(),
                occupancy: None,
                trace: Some(super::super::trace::TracePage::default()),
                trace_total: 0,
            }),
            true
        ));
        assert!(projection.snapshot_ref().shutting_down);
        assert!(projection.read_domain_cut().is_some());
    }

    /// The receipt fold is the CPU linearization point. A late audit retains
    /// its positive fact but cannot clear a replacement with the same message.
    #[test]
    fn t14_t19_audit_receipt_fences_exact_stream_and_preserves_deferred_settlement_head() {
        use crate::durable::ConversationStore;
        let mut projection = projection();
        projection.apply(event_observation(RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        }));
        apply_publication_open(&mut projection);
        let original = stream_start();
        let store = crate::durable::SqliteConversationStore::in_memory(
            projection.snapshot.conversation_id.clone(),
        )
        .unwrap();
        store.initialize(&[]).unwrap();
        let cut = store.observation_read_cut().unwrap();
        projection.source_cut = Some(cut.clone());
        let receipt = |start: &crate::publication::PublicationStreamStart, position: u64| {
            crate::durable::inbox::PublicationAuditReceipt {
                ready_through: Some(0),
                ready_t_through: Some(0),
                summary: crate::durable::inbox::PublicationAuditSummary {
                    stream_id: Some(start.stream_id.clone()),
                    message_id: Some(start.message_id.clone()),
                    attempt_id: Some(start.attempt_id.clone()),
                    turn_id: Some(start.turn_id.clone()),
                    request_id: Some(start.request_id.clone()),
                    kind: crate::publication::PublicationAuditKind::Incomplete,
                    settled_at: chrono::Utc::now(),
                    block_count: 73,
                },
                transcript_cursor: crate::durable::TranscriptCursor::new(position),
                read_cut: cut.clone(),
                header: crate::durable::observation::TranscriptHeader {
                    tool_calls: Vec::new(),
                    publication_settlement: None,
                    cursor: crate::durable::TranscriptCursor::new(position),
                    reference_kind: "publication_audit".into(),
                    reference_id: format!("transcript:{position}"),
                    message: None,
                    display: None,
                    content: Vec::new(),
                    collections: Vec::new(),
                    deferred: true,
                },
            }
        };
        let mut replacement = original.clone();
        replacement.stream_id =
            crate::runtime::identity::PublicationStreamId::new("replacement-same-message");
        replacement.request_id = RequestId::new("replacement-request");
        projection.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt(),
            start: replacement.clone(),
        });
        let execution_phase = projection.snapshot.attempt.as_ref().unwrap().phase.clone();
        let events = projection.fold_audit_receipt(attempt(), receipt(&original, 1));
        assert_eq!(projection.publication_start.as_ref(), Some(&replacement));
        assert!(
            projection
                .snapshot
                .attempt
                .as_ref()
                .unwrap()
                .in_flight
                .is_some()
        );
        assert!(
            matches!(&events[0],RuntimeClientEvent::AssistantPublicationDeferredSettled {settlement,settlement_head,..} if settlement.block_count==73 && settlement_head.len()==1)
        );
        let events = projection.fold_audit_receipt(attempt(), receipt(&replacement, 2));
        assert!(projection.publication_start.is_none());
        assert!(
            projection
                .snapshot
                .attempt
                .as_ref()
                .unwrap()
                .in_flight
                .is_none()
        );
        assert!(
            matches!(&events[0],RuntimeClientEvent::AssistantPublicationDeferredSettled {settlement_head,..} if settlement_head==&projection.snapshot.publication_settlements)
        );
        assert_eq!(projection.snapshot.publication_settlements.len(), 2);
        assert_eq!(
            projection.snapshot.attempt.as_ref().unwrap().phase,
            execution_phase,
            "Publication audit is not execution settlement"
        );
    }

    fn stream_start() -> crate::publication::PublicationStreamStart {
        crate::publication::PublicationStreamStart {
            stream_id: crate::runtime::identity::PublicationStreamId::new("attempt-1-pub-1"),
            attempt_id: attempt(),
            turn_id: crate::runtime::identity::TurnId::new("1"),
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            message_id: MessageId::new("msg-1"),
        }
    }

    fn frame(sequence: u64, payload: PublicationPayload) -> PublicationFrame {
        PublicationFrame {
            stream_id: stream_start().stream_id,
            message_id: MessageId::new("msg-1"),
            sequence,
            payload,
        }
    }

    /// The representative attempt sequence: streaming publication, a tool
    /// call, execution, and terminal settlement.
    ///
    /// Streaming assembly arrives as durably committed publication frames
    /// (Issue #108); only the surrounding execution facts are Event Journal
    /// observations.
    fn representative_sequence() -> Vec<ConversationObservation> {
        let mut observations = vec![
            event_observation(RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            }),
            event_observation(RuntimeEvent::TurnStarted),
            event_observation(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                model: "scripted".to_owned(),
            }),
            ConversationObservation::PublicationOpened {
                attempt_id: attempt(),
                start: stream_start(),
            },
        ];
        for (sequence, payload) in [
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "hello ".to_owned(),
            },
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "world".to_owned(),
            },
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(1),
                call: ToolCallStart {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(1),
                call_id: ToolCallId::new("call_1"),
                suffix: "{}".to_owned(),
            },
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(1),
                call: ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
        ]
        .into_iter()
        .enumerate()
        {
            observations.push(ConversationObservation::Publication {
                attempt_id: attempt(),
                frame: frame(sequence as u64, payload),
            });
        }
        observations.extend([
            event_observation(RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
                generation: None,
            }),
            event_observation(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
            }),
            event_observation(RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                execution_id: None,
                progress: ToolProgress {
                    message: Some("half way".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                },
            }),
            event_observation(RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                result: success_result(),
            }),
            event_observation(RuntimeEvent::TurnCompleted),
            event_observation(RuntimeEvent::AttemptCompleted {
                attempt_id: attempt(),
                finish_reason: ModelFinishReason::Stop,
            }),
        ]);
        observations
    }

    /// The projection is deterministic: applying the same representative
    /// sequence twice produces identical event sequences and identical
    /// snapshots.
    #[test]
    fn representative_sequence_projects_deterministically() {
        let mut first = projection();
        for observation in representative_sequence() {
            first.apply(observation);
        }
        let mut second = projection();
        for observation in representative_sequence() {
            second.apply(observation);
        }
        let first_events = collect(&mut first, RuntimeClientCursor::new(0));
        let second_events = collect(&mut second, RuntimeClientCursor::new(0));
        assert_eq!(first_events, second_events);
        assert!(!first_events.is_empty(), "the sequence publishes events");
        let (first_snapshot, first_cursor) = first.snapshot().expect("snapshot");
        let (second_snapshot, second_cursor) = second.snapshot().expect("snapshot");
        assert_eq!(first_snapshot, second_snapshot);
        assert_eq!(first_cursor, second_cursor);
    }

    /// Request payloads stay internal; invalidation and compaction lifecycle and
    /// committed completion are projected as runtime-owned lifecycle facts.
    #[test]
    fn request_payloads_stay_internal_and_trace_is_owned_by_journal_batch() {
        let mut projection = projection();
        for event in [
            RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                model: "m".to_owned(),
            },
            RuntimeEvent::ModelRequestFailed {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                error: ModelError {
                    kind: ModelErrorKind::RateLimit,
                    message: "retry".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                    retry_after_ms: Some(10),
                    provider_code: Some("rate_limit_exceeded".to_owned()),
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
                usage: None,
                generation: None,
            },
            RuntimeEvent::ModelRetryScheduled {
                failed_request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                retry_number: 1,
                retry_delay_ms: Some(5),
            },
            RuntimeEvent::CompactionStarted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
            },
            RuntimeEvent::CompactionCompleted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
                generation: 1,
                checkpoint_id: crate::runtime::identity::CheckpointId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
                ),
                activation_id: crate::context::checkpoint::ProjectionActivationId(3),
                tokens_before: TokenMeasurement {
                    input_tokens: 1,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1,
                occupancy: None,
            },
            RuntimeEvent::CompactionFailed {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
                error: "boom".to_owned(),
            },
            RuntimeEvent::TurnCompleted,
        ] {
            apply_event(&mut projection, event);
        }
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 3);
        assert!(
            events[0]
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::ContextCompactionStarted { .. }))
        );
        assert!(events[1].event.facts().iter().any(
            |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { context, .. }
                if context.compaction_count == 1)
        ));
        assert!(events[2].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::ContextCompactionFailed { error, .. } if error == "boom")));
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(3));
        assert!(!snapshot.context.compaction_in_progress);
        assert_eq!(snapshot.context.compaction_count, 1);
        assert!(snapshot.attempt.is_none());
    }

    #[test]
    fn manual_compaction_projects_without_fabricating_an_attempt_identity() {
        let mut projection = projection();
        projection.apply(ConversationObservation::ManualCompactionEvent {
            request_id: Some("test-request".to_owned().try_into().unwrap()),
            event: RuntimeEvent::CompactionStarted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
            },
        });
        let admitted = projection
            .snapshot_ref()
            .context
            .manual_compaction
            .as_ref()
            .unwrap();
        assert_eq!(
            admitted
                .request_id
                .as_ref()
                .map(crate::runtime::identity::ManualCompactionRequestId::as_str),
            Some("test-request")
        );
        assert!(!admitted.released);
        projection.apply(ConversationObservation::ManualCompactionEvent {
            request_id: Some("test-request".to_owned().try_into().unwrap()),
            event: RuntimeEvent::CompactionCompleted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
                generation: 1,
                checkpoint_id: crate::runtime::identity::CheckpointId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
                ),
                activation_id: crate::context::checkpoint::ProjectionActivationId(2),
                tokens_before: TokenMeasurement {
                    input_tokens: 8_400,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1_900,
                occupancy: None,
            },
        });

        let released = projection
            .snapshot_ref()
            .context
            .manual_compaction
            .as_ref()
            .unwrap();
        assert_eq!(
            released
                .request_id
                .as_ref()
                .map(crate::runtime::identity::ManualCompactionRequestId::as_str),
            Some("test-request")
        );
        assert!(released.released);
        assert!(released.error.is_none());
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert!(events[0].event.facts().iter().any(|fact| matches!(
            fact,
            RuntimeClientEvent::ContextCompactionStarted {
                attempt_id: None,
                ..
            }
        )));
        assert!(events[1].event.facts().iter().any(
            |fact| matches!(fact, RuntimeClientEvent::ContextCompacted {
                attempt_id: None,
                context,
            } if !context.compaction_in_progress && context.compaction_count == 1)
        ));
    }

    /// Turn and usage observations publish the exact values folded into the
    /// attempt view, so incremental subscribers stay synchronized with a
    /// snapshot without polling.
    #[test]
    fn turn_and_usage_events_agree_with_the_attempt_view() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(&mut projection, RuntimeEvent::TurnStarted);
        apply_event(&mut projection, RuntimeEvent::TurnCompleted);
        apply_event(
            &mut projection,
            RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(ModelUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    details: None,
                }),
                generation: None,
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        assert_eq!(events.len(), 2);
        assert_eq!(
            *single_fact(&events[0]),
            RuntimeClientEvent::AttemptTurnUpdated {
                attempt_id: attempt(),
                turn: 1,
            }
        );
        assert_eq!(
            *single_fact(&events[1]),
            RuntimeClientEvent::AttemptUsageUpdated {
                attempt_id: attempt(),
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    details: None,
                },
            }
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let attempt_view = snapshot.attempt.expect("attempt view exists");
        assert_eq!(attempt_view.turn, 1);
        assert_eq!(
            attempt_view
                .last_usage
                .as_ref()
                .map(|usage| usage.input_tokens),
            Some(10)
        );
    }

    #[test]
    fn shutdown_folds_into_the_snapshot_and_publishes_the_runtime_fact() {
        let mut projection = projection();
        projection.apply(ConversationObservation::Shutdown);

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 1);
        assert_eq!(
            *single_fact(&events[0]),
            RuntimeClientEvent::RuntimeShutdown
        );

        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert!(snapshot.shutting_down);
        assert_eq!(cursor, RuntimeClientCursor::new(1));
    }

    /// Exactly one terminal settlement exists per attempt, and the
    /// terminal client event carries the platform outcome.
    #[test]
    fn terminal_settlement_is_unique() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt(),
                reason: CancellationReason::UserRequested,
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        assert_eq!(
            events
                .iter()
                .filter(|event| event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. })))
                .count(),
            1,
            "exactly one AttemptSettled"
        );
        assert_eq!(
            *single_fact(events.last().expect("terminal event")),
            RuntimeClientEvent::AttemptSettled {
                attempt_id: attempt(),
                outcome: RuntimeClientOutcome::Cancelled {
                    reason: CancellationReason::UserRequested,
                },
            }
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
    }

    /// Provider-specific failure fields never leak: the projected failure
    /// drops the provider error code.
    #[test]
    fn provider_specific_fields_do_not_leak() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt(),
                error: crate::events::types::AttemptFailure::Model {
                    error: ModelError {
                        kind: ModelErrorKind::RateLimit,
                        message: "retries exhausted".to_owned(),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                        retry_after_ms: Some(5_000),
                        provider_code: Some("rate_limit_exceeded".to_owned()),
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    },
                },
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        let settled = events
            .iter()
            .flat_map(|protocol| protocol.event.facts())
            .find_map(|fact| match fact {
                RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
                _ => None,
            })
            .expect("terminal settlement");
        let value = serde_json::to_value(settled).expect("serialize outcome");
        assert_eq!(value["type"], "failed");
        let text = serde_json::to_string(settled).expect("serialize");
        assert!(
            !text.contains("provider_code"),
            "provider-specific fields never leak: {text}"
        );
        assert!(text.contains("rate_limit"));
        assert!(text.contains("retries exhausted"));
        assert!(text.contains("5000"));
    }

    /// Streamed message state folds correctly into the in-flight repair
    /// view, and committing the message clears the in-flight state.
    #[test]
    fn streamed_message_state_folds_correctly() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_publication_open(&mut projection);
        apply_frame(&mut projection, 0, text_frame("hello "));
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        let in_flight = snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .expect("in-flight message");
        assert_eq!(
            in_flight.blocks,
            vec![InFlightBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "hello ".to_owned(),
            }]
        );
        // Resume after C delivers exactly the remaining delta, never a
        // duplicate of the accumulated text.
        apply_frame(&mut projection, 1, text_frame("world"));
        let resumed = collect(&mut projection, cursor);
        assert_eq!(resumed.len(), 1, "exactly one delta is published after C");
        assert!(resumed[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, inline_delta:Some(delta), .. } if kind == "text" && delta == "world")));
        // The canonical commit clears the in-flight repair state.
        let committed = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-1"),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: "hello world".to_owned(),
            })],
        });
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: committed.clone(),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.messages, vec![committed]);
        assert!(
            snapshot
                .attempt
                .as_ref()
                .and_then(|attempt| attempt.in_flight.as_ref())
                .is_none()
        );
    }

    /// A durable commit cursor travels with the live observation even when
    /// observations arrive in the opposite order. The Runtime Client event
    /// cursor records that delivery order only; it is deliberately not the
    /// transcript order input.
    #[test]
    fn live_committed_observations_carry_durable_cursors_across_reordering() {
        let mut projection = projection();
        let message_a = compactable_user("message-a");
        let message_b = compactable_user("message-b");

        projection.apply(ConversationObservation::Committed {
            attempt_id: None,
            block: message_b,
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(11)),
        });
        projection.apply(ConversationObservation::Committed {
            attempt_id: None,
            block: message_a,
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(10)),
        });

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(
            events
                .iter()
                .map(|event| event.cursor.get())
                .collect::<Vec<_>>(),
            vec![1, 2],
            "Runtime Client cursors follow observation delivery"
        );
        assert_eq!(
            events
                .iter()
                .flat_map(|event| event.event.facts())
                .filter_map(|fact| match fact {
                    RuntimeClientEvent::MessageCommitted {
                        transcript_cursor, ..
                    } => transcript_cursor.map(RuntimeClientTranscriptCursor::get),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![11, 10],
            "each live fact carries its own durable transcript cursor"
        );
    }

    /// Foreground tool state keeps canonical logical identities even when
    /// physical completion order is reversed.
    #[test]
    fn foreground_tool_identity_survives_reversed_completion() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        for (sequence, call) in [
            ToolCallStart {
                id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
            },
            ToolCallStart {
                id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
                name: "beta".to_owned(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            apply_frame(
                &mut projection,
                sequence as u64,
                PublicationPayload::ProposedToolCallStarted {
                    block_index: ContentBlockIndex::new(u32::try_from(sequence).unwrap()),
                    call,
                },
            );
        }
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
            },
        );
        // B settles first physically.
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
                result: success_result(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
                result: success_result(),
            },
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        assert_eq!(foreground.len(), 2);
        assert_eq!(foreground[0].block_index, ContentBlockIndex::new(0));
        assert_eq!(foreground[1].block_index, ContentBlockIndex::new(1));
        assert_eq!(foreground[0].message_id, foreground[1].message_id);
        assert_eq!(foreground[0].call_id, Some(ToolCallId::new("call_a")));
        assert_eq!(foreground[1].call_id, Some(ToolCallId::new("call_b")));
        assert!(matches!(
            foreground[0].state,
            ForegroundToolState::Settled { .. }
        ));
        assert!(matches!(
            foreground[1].state,
            ForegroundToolState::Settled { .. }
        ));
    }

    /// The Runtime Client projection carries the canonical cancellation phase
    /// verbatim and preserves it through its wire round trip.
    #[test]
    fn issue136_foreground_cancellation_phase_round_trips_through_projection() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_cancelled"),
            tool_id: ToolId::new("tool-cancelled"),
            name: "cancelled".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        apply_frame(
            &mut projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            &mut projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            &mut projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                phase: ToolCancellationPhase::DuringExecution,
            },
            content: Vec::new(),
            duration_ms: 3,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        };
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        let ForegroundToolState::Settled {
            result: projected, ..
        } = &foreground[0].state
        else {
            panic!("the cancelled call must be settled");
        };
        assert_eq!(projected, &result);
        let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
        assert_eq!(
            encoded["attempt"]["foreground"][0]["state"]["result"]["status"]["phase"],
            "during_execution"
        );
        let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
            serde_json::from_value(encoded).expect("snapshot round trip");
        assert_eq!(decoded, snapshot);
    }

    /// The Runtime Client projection carries the canonical `OutcomeUnknown`
    /// result verbatim — bounded `detail` included — and preserves it through
    /// its wire round trip, never flattening it into a known `failed`.
    #[test]
    fn issue202_outcome_unknown_flows_through_projection_unchanged() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_outcome_unknown"),
            tool_id: ToolId::new("tool-outcome-unknown"),
            name: "outcome_unknown".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        apply_frame(
            &mut projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            &mut projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            &mut projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "executor lost contact after dispatch".to_owned(),
            },
            content: Vec::new(),
            duration_ms: 3,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        };
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        let ForegroundToolState::Settled {
            result: projected, ..
        } = &foreground[0].state
        else {
            panic!("the outcome-unknown call must be settled");
        };
        assert_eq!(projected, &result);
        let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
        let status = &encoded["attempt"]["foreground"][0]["state"]["result"]["status"];
        assert_eq!(status["type"], "outcome_unknown");
        assert_eq!(status["detail"], "executor lost contact after dispatch");
        let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
            serde_json::from_value(encoded).expect("snapshot round trip");
        assert_eq!(decoded, snapshot);
    }

    /// Canonical result commitment does not create an execution terminal fact.
    #[test]
    fn issue136_before_start_commit_preserves_execution_distinction() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let mut projection = projection();
        let store = SqliteConversationStore::in_memory(projection.snapshot.conversation_id.clone())
            .unwrap();
        let envelope = |event| RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt()),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            tool_occurrence: None,
            observation: None,
            event,
        };
        let started = store
            .append_event(envelope(RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            }))
            .unwrap();
        projection.apply_native_batch(started.observation.as_ref().unwrap());
        let call = ToolCall {
            id: ToolCallId::new("call_before_start"),
            tool_id: ToolId::new("tool-before-start"),
            name: "before_start".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-1"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        });
        let (assistant_event, _) = store
            .append_canonical_with_event(
                &assistant,
                envelope(RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.id().clone(),
                }),
            )
            .unwrap();
        projection.bootstrap_durable_message(&attempt(), &assistant);
        projection.apply_native_batch(assistant_event.observation.as_ref().unwrap());
        let result = cancelled_result(
            CancellationReason::RuntimeShutdown,
            ToolCancellationPhase::BeforeStart,
        );
        let committed = MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                assistant.id().clone(),
                ContentBlockIndex::new(0),
            ),
            id: MessageId::new("message-before-start"),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        });
        let (_, before_commit) = projection.snapshot().unwrap();
        let (events, _) = store
            .append_canonical_batch_with_events(
                std::slice::from_ref(&committed),
                &[envelope(RuntimeEvent::ToolMessageCommitted {
                    message_id: committed.id().clone(),
                    tool_call_id: call.id,
                })],
            )
            .unwrap();
        projection.apply_native_batch(events[0].observation.as_ref().unwrap());
        let events = collect(&mut projection, before_commit);
        let facts: Vec<_> = events
            .iter()
            .flat_map(|event| event.event.facts())
            .collect();
        assert_eq!(facts.iter().filter(|fact| matches!(fact,
            RuntimeClientEvent::ToolResultCommitted { result: projected, .. } if projected == &result
        )).count(), 1);
        assert!(!facts.iter().any(|fact| matches!(
            fact,
            RuntimeClientEvent::ToolExecutionStarted { .. }
                | RuntimeClientEvent::ToolExecutionSettled { .. }
        )));
        let (snapshot, _) = projection.snapshot().unwrap();
        let foreground = &snapshot.attempt.as_ref().unwrap().foreground;
        assert_eq!(foreground.len(), 1);
        assert!(matches!(
            foreground[0].state,
            ForegroundToolState::Assembled { .. }
        ));
        assert_eq!(foreground[0].committed_result.as_ref(), Some(&result));
    }

    /// A live `DuringExecution` settlement already closes the slot; its later
    /// canonical `ToolMessage` commit is history only and emits no duplicate
    /// foreground settlement.
    #[test]
    fn issue136_live_settlement_is_not_duplicated_by_tool_message_commit() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_during_execution"),
            tool_id: ToolId::new("tool-during-execution"),
            name: "during_execution".to_owned(),
            arguments: serde_json::json!({}),
        };
        apply_assembled_call(&mut projection, &call);
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = cancelled_result(
            CancellationReason::ParentCancelled,
            ToolCancellationPhase::DuringExecution,
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );
        let (_, before_commit) = projection.snapshot().expect("snapshot before commit");
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new("msg-1"),
                    crate::message::types::ContentBlockIndex::new(0),
                ),
                id: MessageId::new("message-during-execution"),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            }),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
        });

        let commit_events = collect(&mut projection, before_commit);
        assert_eq!(commit_events.len(), 1);
        assert!(
            commit_events[0]
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::MessageCommitted { .. }))
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot after commit");
        assert!(matches!(
            &snapshot.attempt.as_ref().expect("attempt view").foreground[0].state,
            ForegroundToolState::Settled { result: projected, .. } if projected == &result
        ));
    }

    /// A resume after a serviceable cursor replays exactly the retained
    /// gap, in order, without duplicates or gaps.
    #[test]
    fn resume_after_a_serviceable_cursor_has_no_gap() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_publication_open(&mut projection);
        let (_, opened_cursor) = projection.snapshot().unwrap();
        assert_eq!(
            opened_cursor,
            RuntimeClientCursor::new(2),
            "Attempt start and genuine stream opening precede suffixes"
        );
        for index in 0..10u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
        }
        let (_, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(12));
        let events = collect(&mut projection, RuntimeClientCursor::new(3));
        let cursors: Vec<u64> = events.iter().map(|event| event.cursor.get()).collect();
        assert_eq!(cursors, vec![4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].cursor < pair[1].cursor)
        );
    }

    /// An expired cursor (evicted from the bounded ring) and a cursor
    /// ahead of the stream both fail explicitly with `resync_required`;
    /// a fresh snapshot still repairs all state.
    #[test]
    fn unserviceable_cursors_fail_with_resync_required() {
        let mut projection = RuntimeClientProjection::new(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                configured_tools: Vec::new(),
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            Some(model_view()),
            4,
        );
        apply_publication_open(&mut projection);
        for index in 0..20u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
        }
        let error = projection
            .subscribe(RuntimeClientCursor::new(1))
            .expect_err("the cursor was evicted from the bounded ring");
        match error {
            crate::runtime_client::types::RuntimeClientError::ResyncRequired {
                after_cursor,
                earliest_serviceable,
            } => {
                assert_eq!(after_cursor, RuntimeClientCursor::new(1));
                assert_eq!(earliest_serviceable, RuntimeClientCursor::new(17));
            }
            other => panic!("expected resync_required, got {other:?}"),
        }
        let error = projection
            .subscribe(RuntimeClientCursor::new(100))
            .expect_err("a cursor ahead of the stream is unserviceable");
        assert!(matches!(
            error,
            crate::runtime_client::types::RuntimeClientError::ResyncRequired { .. }
        ));
        // A fresh snapshot repairs every externally visible fact.
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(21));
        let in_flight = snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .expect("in-flight view repaired");
        assert!(matches!(
            &in_flight.blocks[0],
            InFlightBlock::Text { text, .. } if text == "012345678910111213141516171819"
        ));
    }

    /// A stalled subscriber introduces no second backlog: the bounded
    /// replay ring is the only retained storage, its size never exceeds the
    /// retention limit no matter how far behind the consumer is, and the
    /// consumer is told explicitly that it fell behind rather than being
    /// handed a silently non-contiguous stream.
    #[test]
    fn a_stalled_subscriber_is_bounded_and_never_silently_skips() {
        let limit = 4;
        let mut projection = RuntimeClientProjection::new(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                configured_tools: Vec::new(),
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            Some(model_view()),
            limit,
        );
        apply_publication_open(&mut projection);
        let (stalled, _notify) = projection
            .subscribe(RuntimeClientCursor::new(1))
            .expect("the current cursor is serviceable");

        // The subscriber never polls while 200 events are published.
        for index in 0..200u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
            assert!(
                projection.replay_len() <= limit,
                "the one retained backlog stays within its bound at every publication"
            );
        }
        assert_eq!(
            projection.replay_len(),
            limit,
            "retention is the only storage that grew, and only to its bound"
        );

        // Publication cost per subscriber is one wakeup, so the stalled
        // subscriber holds nothing: it is still sitting at its original
        // cursor.
        let poll = projection.poll_subscriber(stalled);
        let SubscriberPoll::Lagged {
            after_cursor,
            earliest_serviceable,
        } = poll
        else {
            panic!("a subscriber past retention must lag explicitly, got {poll:?}");
        };
        assert_eq!(after_cursor, RuntimeClientCursor::new(1));
        assert_eq!(earliest_serviceable, RuntimeClientCursor::new(197));
        assert_eq!(
            projection.poll_subscriber(stalled),
            poll,
            "the lag verdict is stable and never partially advances the cursor"
        );

        // A consumer that keeps up observes strictly contiguous cursors,
        // one event per poll, and then parks.
        let (live, _notify) = projection
            .subscribe(projection.cursor())
            .expect("the current cursor is serviceable");
        let base = projection.cursor().get();
        for step in 1..=3u64 {
            apply_frame(&mut projection, 199 + step, text_frame("x"));
            let poll = projection.poll_subscriber(live);
            let SubscriberPoll::Event(event) = poll else {
                panic!("the live subscriber must receive, got {poll:?}");
            };
            assert_eq!(event.cursor.get(), base + step);
        }
        assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Pending);

        // Removal is observable, never an indefinite park.
        projection.remove_subscriber(live);
        assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Closed);
    }

    /// Cursor overflow fails explicitly and never wraps: publishing stops,
    /// subscriptions fail with a typed error, and reads no longer hand back
    /// a read model that silently stopped folding transitions.
    #[test]
    fn cursor_overflow_fails_explicitly() {
        let mut projection = projection();
        projection.force_cursor_for_test(u64::MAX - 1);
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        assert_eq!(projection.cursor(), RuntimeClientCursor::new(u64::MAX));
        apply_event(&mut projection, RuntimeEvent::TurnStarted);
        apply_frame(&mut projection, 0, text_frame("x"));
        assert_eq!(
            projection.cursor(),
            RuntimeClientCursor::new(u64::MAX),
            "the cursor never wraps"
        );
        assert!(matches!(
            projection.subscribe(RuntimeClientCursor::new(0)),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
        // Reads fail explicitly too: after exhaustion the projection can no
        // longer fold authoritative transitions, and returning the stale
        // read model would hide exactly that.
        assert!(matches!(
            projection.snapshot(),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
        assert!(matches!(
            projection.snapshot_ref_checked(),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
    }

    /// Mailbox observations fold into the inbound diagnostics view:
    /// enqueues appear as pending in inbound-sequence order and a finite
    /// drain clears them and records the watermark.
    #[test]
    fn inbound_diagnostics_fold_from_mailbox_observations() {
        let mailbox = ConversationInboundMailbox::new(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ));
        let item = |id: &str| UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: id.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
            ),
        };
        let first = mailbox.enqueue(item("m1")).expect("enqueue");
        let second = mailbox.enqueue(item("m2")).expect("enqueue");

        let mut projection = projection();
        let drained = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("batch");
        let _ = (first, second);
        // The authoritative items fold through the enqueue observations.
        for entry in drained.items() {
            projection.apply(ConversationObservation::InboundEnqueued(entry.clone()));
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.inbound.pending.len(), 2);
        assert_eq!(
            snapshot.inbound.pending[0].sequence.get(),
            1,
            "pending items follow inbound sequence order"
        );
        assert_eq!(snapshot.inbound.pending[1].sequence.get(), 2);

        // The finite drain of the same batch clears the pending view and
        // records the watermark.
        projection.apply(ConversationObservation::InboundDrained(drained));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert!(snapshot.inbound.pending.is_empty());
        let last_drain = snapshot.inbound.last_drain.expect("drain recorded");
        assert_eq!(last_drain.watermark.get(), 2);
        assert_eq!(last_drain.count, 2);
    }

    /// Native interactions are live Runtime Client projection facts: a
    /// pending event adds exactly one deterministic snapshot entry, and its
    /// terminal event removes that entry without touching canonical messages.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn interaction_pending_and_settled_fold_into_snapshot_and_events() {
        let mut projection = projection();
        let request = InteractionRequest {
            id: InteractionId::new("attempt-1-interaction-1"),
            conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            attempt_id: attempt(),
            turn: 2,
            kind: InteractionKind::Approval {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-1"),
                },
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".into(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({"path": "original"}),
                reason: "native policy".to_owned(),
            },
        };
        let requested_audit = interaction_audit(
            &request,
            "event-requested",
            RuntimeEvent::InteractionRequested {
                interaction_id: request.id.clone(),
                subject: InteractionSubject::Approval {
                    invocation_id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new("call-1"),
                    },
                    tool_id: ToolId::new("tool-alpha"),
                    tool_name: "alpha".into(),
                    arguments_digest: "0".repeat(64),
                    reason: "native policy".to_owned(),
                },
            },
        );
        projection.apply(ConversationObservation::InteractionPending {
            interaction: super::super::checkpoint::interaction_request_header(
                &request,
                InteractionSource::Primary,
            ),
            audit: Some((requested_audit, crate::durable::TranscriptCursor::new(1))),
        });
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.pending_interactions,
            vec![super::super::checkpoint::interaction_request_header(
                &request,
                InteractionSource::Primary
            )]
        );
        assert_eq!(cursor, RuntimeClientCursor::new(1));
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert!(matches!(
            events.first().and_then(|event| event.event.facts().iter().find(|fact| matches!(fact, RuntimeClientEvent::InteractionPending { .. }))),
            Some(RuntimeClientEvent::InteractionPending { interaction })
                if interaction.request_id == request.id && interaction.attempt_id == request.attempt_id && !interaction.body_complete
        ));

        let outcome = InteractionOutcome::Responded {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        };
        let settled_audit = interaction_audit(
            &request,
            "event-settled",
            RuntimeEvent::InteractionSettled {
                interaction_id: request.id.clone(),
                settlement: InteractionSettlement::Approved,
            },
        );
        projection.apply(ConversationObservation::InteractionSettled {
            interaction: InteractionRef::new(request.conversation_id.clone(), request.id.clone()),
            outcome: outcome.clone(),
            audit: Some((settled_audit, crate::durable::TranscriptCursor::new(2))),
        });
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert!(snapshot.pending_interactions.is_empty());
        assert_eq!(cursor, RuntimeClientCursor::new(2));
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        assert!(matches!(
            events.first().and_then(|event| event.event.facts().iter().find(|fact| matches!(fact, RuntimeClientEvent::InteractionSettled { .. }))),
            Some(RuntimeClientEvent::InteractionSettled {
                interaction,
                outcome: event_outcome,
            }) if interaction
                == &InteractionRef::new(request.conversation_id.clone(), request.id.clone())
                && event_outcome == &outcome
        ));

        // The projection never turns a live prompt into a historical audit;
        // only the durable requested/settled audit observations do that.
        assert!(
            projection
                .snapshot()
                .expect("snapshot")
                .0
                .messages
                .is_empty()
        );
    }

    /// The root projection is a set of independently addressed routed
    /// interactions. Primary and child prompts can coexist, and removing or
    /// settling one pair never selects or changes another pair.
    #[test]
    fn routed_interactions_keep_multiple_sources_and_removals_independent() {
        let mut projection = projection();
        let approval = |conversation: &str, id: &str| InteractionRequest {
            id: InteractionId::new(id),
            conversation_id: ConversationId::new(conversation),
            attempt_id: attempt(),
            turn: 1,
            kind: InteractionKind::Approval {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new(format!("{id}-call")),
                },
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".into(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({"id": id}),
                reason: "independent prompt".to_owned(),
            },
        };
        let primary = RoutedInteraction::primary(approval(
            "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89",
            "primary-a",
        ));
        let child_a_request = approval("conv_8e708397-e71d-7840-8b5a-5074c7b2d0ed", "child-a");
        let child_a = RoutedInteraction::subagent(
            crate::runtime::identity::SubagentId::new(
                "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89-subagent-1",
            ),
            child_a_request.conversation_id.clone(),
            crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
            child_a_request,
        );
        let other_child_request = approval("conv_143069cc-2a88-7514-b43a-0dfdaa9f1170", "child-b");
        let child_b = RoutedInteraction::subagent(
            crate::runtime::identity::SubagentId::new(
                "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89-subagent-2",
            ),
            other_child_request.conversation_id.clone(),
            crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
            other_child_request,
        );

        for interaction in [primary.clone(), child_a.clone(), child_b.clone()] {
            projection.apply(ConversationObservation::InteractionPending {
                interaction: super::super::checkpoint::interaction_header(&interaction),
                audit: None,
            });
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.pending_interactions.len(), 3);
        assert_eq!(
            snapshot
                .pending_interactions
                .iter()
                .map(|interaction| interaction.interaction.clone())
                .collect::<Vec<_>>(),
            vec![
                primary.interaction.clone(),
                child_a.interaction.clone(),
                child_b.interaction.clone(),
            ]
        );

        projection.apply(ConversationObservation::InteractionRemoved {
            interaction: child_a.interaction.clone(),
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot after child removal");
        assert_eq!(
            snapshot
                .pending_interactions
                .iter()
                .map(|interaction| interaction.interaction.clone())
                .collect::<Vec<_>>(),
            vec![primary.interaction.clone(), child_b.interaction.clone()]
        );

        projection.apply(ConversationObservation::InteractionSettled {
            interaction: child_b.interaction.clone(),
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            },
            audit: None,
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot after settlement");
        assert_eq!(
            snapshot.pending_interactions,
            vec![super::super::checkpoint::interaction_header(&primary)]
        );
        assert!(snapshot.messages.is_empty());
    }

    /// The snapshot and its cursor linearize: the returned cursor is the
    /// cursor of the returned snapshot.
    #[test]
    fn snapshot_and_cursor_are_linearized() {
        let mut projection = projection();
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(snapshot.messages.is_empty());
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(1));
        assert!(snapshot.attempt.is_some());
    }

    /// Background registry observations fold into the background section
    /// preserving newest-first allocation order and terminal retention.
    #[test]
    fn background_observations_fold_in_allocation_order() {
        use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
        let mut projection = projection();
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                execution_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
                ),
                tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
                tool_name: "bg".into(),
                state: BackgroundLifecycle::Running,
                progress: None,
                result: None,
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                execution_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
                ),
                tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
                tool_name: "bg".into(),
                state: BackgroundLifecycle::Succeeded,
                progress: None,
                result: Some(std::sync::Arc::new(success_result())),
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                execution_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_20eb7fc0-b69d-7476-8553-c156fdc879c3",
                ),
                tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
                tool_name: "bg".into(),
                state: BackgroundLifecycle::Starting,
                progress: None,
                result: None,
            },
        ));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.jobs.len(), 2);
        assert_eq!(
            snapshot.jobs[1].job_id.as_str(),
            "exec_215a03ee-2332-70b6-8e2d-634da8066f98"
        );
        assert_eq!(snapshot.jobs[1].state, BackgroundLifecycle::Succeeded);
        assert!(snapshot.jobs[1].result.is_some());
        assert_eq!(
            snapshot.jobs[0].job_id.as_str(),
            "exec_20eb7fc0-b69d-7476-8553-c156fdc879c3"
        );
    }

    /// Issue #202: the projection carries the honest background terminal
    /// lifecycle vocabulary verbatim. `OutcomeUnknown` and `TimedOut` are
    /// first-class terminal states and are never collapsed into `Failed`,
    /// in neither the folded view nor the published event, on or off the
    /// wire.
    #[test]
    fn issue202_background_terminal_states_project_verbatim() {
        use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
        let mut projection = projection();
        let outcome_unknown_result = ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "cancellation raced the executor after dispatch".to_owned(),
            },
            ..success_result()
        };
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                execution_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_60002c8f-aeff-7c09-8709-610f0ed8d415",
                ),
                tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
                tool_name: "bg".into(),
                state: BackgroundLifecycle::OutcomeUnknown,
                progress: None,
                result: Some(std::sync::Arc::new(outcome_unknown_result)),
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                execution_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_d013aabc-70aa-72fc-8b80-81767d9a64a1",
                ),
                tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
                tool_name: "bg".into(),
                state: BackgroundLifecycle::TimedOut,
                progress: None,
                result: Some(std::sync::Arc::new(ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::TimedOut,
                    ..success_result()
                })),
            },
        ));

        // The folded view keeps the typed terminal states verbatim.
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.jobs.len(), 2);
        assert_eq!(snapshot.jobs[1].state, BackgroundLifecycle::OutcomeUnknown);
        assert_eq!(snapshot.jobs[0].state, BackgroundLifecycle::TimedOut);

        // The published events carry the same verbatim states.
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        let [unknown_event, timed_out_event] = events.as_slice() else {
            panic!("two JobUpdated events: {events:?}");
        };
        let RuntimeClientEvent::JobUpdated {
            job: unknown_view, ..
        } = single_fact(unknown_event)
        else {
            panic!("a JobUpdated event: {:?}", unknown_event.event);
        };
        let RuntimeClientEvent::JobUpdated {
            job: timed_out_view,
            ..
        } = single_fact(timed_out_event)
        else {
            panic!("a JobUpdated event: {:?}", timed_out_event.event);
        };
        assert_eq!(unknown_view.state, BackgroundLifecycle::OutcomeUnknown);
        assert_eq!(timed_out_view.state, BackgroundLifecycle::TimedOut);

        // On the wire the states serialize as their typed snake_case names,
        // never "failed".
        let unknown_json = serde_json::to_value(unknown_view).expect("execution JSON");
        assert_eq!(unknown_json["state"], "outcome_unknown");
        let timed_out_json = serde_json::to_value(timed_out_view).expect("execution JSON");
        assert_eq!(timed_out_json["state"], "timed_out");
    }

    /// Issue #178: a subagent registry snapshot folds into the whole-view
    /// `AgentUpdated` event carrying the live activity projection and
    /// the redacted execution profile, and the snapshot repair path serves
    /// the same enriched view.
    #[test]
    fn subagent_observations_fold_the_activity_projection_into_the_view() {
        use crate::runtime::subagent::{
            SubagentActivity, SubagentActivityCounters, SubagentExecutionProfile,
            SubagentObservation,
        };

        let mut projection = projection();
        let observation = SubagentObservation {
            revision: 2,
            activity: SubagentActivity::ToolGroup {
                active: 1,
                representative: Some(
                    crate::runtime::subagent::activity::SubagentToolRepresentative {
                        tool_call_id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-bash"),
                        progress: None,
                    },
                ),
            },
            counters: SubagentActivityCounters {
                tool_executions: 1,
                ..SubagentActivityCounters::default()
            },
            ..SubagentObservation::default()
        };
        projection.apply(owner_observation(
            agent_activation_snapshot(observation.clone()),
            crate::runtime::subagent::AgentState::Active,
        ));
        // The disposable activity lane folds identically (unconditional
        // last-write-wins whole-view upsert).
        let newer = SubagentObservation {
            revision: 3,
            ..observation.clone()
        };
        projection.apply(ConversationObservation::SubagentActivity(
            agent_activation_snapshot(newer.clone()),
        ));

        // Both events carry the enriched whole view; the latest wins.
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        let [_, event] = events.as_slice() else {
            panic!("two AgentUpdated events: {events:?}");
        };
        let RuntimeClientEvent::AgentUpdated {
            agent: subagent, ..
        } = single_fact(event)
        else {
            panic!("a AgentUpdated event: {:?}", event.event);
        };
        assert_eq!(subagent.observation, newer);
        assert_eq!(
            subagent.execution_profile,
            Some(SubagentExecutionProfile {
                model: "local/model".to_owned(),
                profile: None,
                reasoning_enabled: false,
            })
        );
        assert_eq!(
            subagent.started_at,
            chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc)
        );

        // The snapshot repair path re-reads the same enriched view.
        let (repaired, _) = projection.snapshot().expect("snapshot");
        assert_eq!(repaired.agents.len(), 1);
        assert_eq!(repaired.agents[0].observation, newer);
        assert_eq!(
            repaired.agents[0].execution_profile,
            subagent.execution_profile
        );
    }
    fn committed_settlement() -> crate::runtime::subagent::SubagentSettlement {
        crate::runtime::subagent::SubagentSettlement {
            publication: crate::runtime::subagent::SubagentPublication::Committed,
            physical: crate::runtime::subagent::SubagentPhysicalSettlement::Proven,
        }
    }

    fn owner_snapshot(
        activation: &crate::runtime::subagent::SubagentSnapshot,
        state: crate::runtime::subagent::AgentState,
    ) -> crate::runtime::subagent::AgentSnapshot {
        crate::runtime::subagent::AgentSnapshot {
            title: "Explore task".into(),
            agent_id: activation.child_agent_id.clone(),
            conversation_id: activation.child_conversation_id.clone(),
            parent_agent_id: activation.parent_agent_id.clone(),
            agent: activation.agent.clone(),
            state,
            current_activation: (state != crate::runtime::subagent::AgentState::Inactive)
                .then(|| activation.subagent_id.clone()),
            latest_activation: activation.subagent_id.clone(),
            observation: activation.observation.clone(),
        }
    }

    fn owner_observation(
        snapshot: crate::runtime::subagent::SubagentSnapshot,
        state: crate::runtime::subagent::AgentState,
    ) -> ConversationObservation {
        ConversationObservation::SubagentLifecycle {
            agent: Some(Box::new(owner_snapshot(&snapshot, state))),
            snapshot,
        }
    }

    /// Folding exactly one owner observation is the publication cut. No client
    /// lifecycle inference or second owner observation can repair this result.
    #[test]
    fn owner_transition_projects_starting_and_terminal_unproven_at_one_cut() {
        use crate::runtime::subagent::{AgentState, SubagentPublication, SubagentState};
        for (activation_state, owner_state) in [
            (SubagentState::Stopping, AgentState::Admitting),
            (SubagentState::Interrupted, AgentState::Unavailable),
        ] {
            let mut projection = projection();
            let mut activation =
                agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
            activation.state = activation_state;
            if activation_state.is_terminal() {
                activation.settlement.publication = SubagentPublication::Committed;
            }
            let owner = owner_snapshot(&activation, owner_state);
            let fresh = super::agent_view(&owner, &activation);
            projection.apply(ConversationObservation::SubagentLifecycle {
                agent: Some(Box::new(owner)),
                snapshot: activation.clone(),
            });
            let (snapshot, _) = projection.snapshot().unwrap();
            assert_eq!(snapshot.agents, vec![fresh.clone()]);
            let events = collect(&mut projection, RuntimeClientCursor::new(0));
            assert_eq!(events.len(), 1, "one complete owner transition");
            assert!(
                events[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AgentUpdated { agent, .. } if **agent == fresh))
            );
            // Disposable activation data can never replace owner lifecycle.
            activation.observation.revision += 1;
            projection.apply(ConversationObservation::SubagentActivity(activation));
            assert_eq!(
                projection.snapshot().unwrap().0.agents[0].state,
                owner_state
            );
        }
    }

    #[test]
    fn agents_and_jobs_share_incremental_cursor_and_captured_snapshot_cut() {
        use crate::runtime::subagent::{AgentState, SubagentObservation};
        use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
        let mut projection = projection();
        let fence = projection.read_domain_fence();
        let mut activation = agent_activation_snapshot(SubagentObservation::default());
        let job = BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("bash")),
            tool_name: "bash".into(),
            state: BackgroundLifecycle::Running,
            progress: None,
            result: None,
        };
        projection.apply(owner_observation(activation.clone(), AgentState::Active));
        projection.apply(ConversationObservation::Background(job.clone()));
        let candidate = projection.snapshot_candidate().unwrap();
        assert_eq!(candidate.cursor, RuntimeClientCursor::new(2));
        let captured_activation = candidate.snapshot.agents[0].activation_id.clone();
        activation.subagent_id =
            crate::runtime::identity::SubagentId::new("conv_57d68983-5497-771e-8aaa-5f1356061698");
        projection.apply(owner_observation(activation.clone(), AgentState::Active));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                bash: None,
                state: BackgroundLifecycle::Succeeded,
                result: Some(std::sync::Arc::new(success_result())),
                ..job
            },
        ));
        assert_eq!(
            projection.read_domain_fence(),
            fence,
            "domain observations never request a durable Session reread"
        );
        assert_eq!(
            candidate.snapshot.agents[0].activation_id,
            captured_activation
        );
        assert_eq!(
            candidate.snapshot.jobs[0].state,
            BackgroundLifecycle::Running
        );
        let events = collect(&mut projection, candidate.cursor);
        assert_eq!(
            events.len(),
            2,
            "no unrelated Trace/read-domain invalidation"
        );
        assert_eq!(events[0].cursor, RuntimeClientCursor::new(3));
        assert_eq!(events[1].cursor, RuntimeClientCursor::new(4));
        assert!(
            events[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AgentUpdated { agent, .. } if agent.activation_id == activation.subagent_id))
        );
        assert!(
            events[1].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::JobUpdated { job, .. } if job.state == BackgroundLifecycle::Succeeded))
        );
        assert_eq!(
            candidate.snapshot.agents[0].activation_id, captured_activation,
            "replay cannot mutate an earlier captured activation"
        );
    }

    fn agent_activation_snapshot(
        observation: crate::runtime::subagent::SubagentObservation,
    ) -> crate::runtime::subagent::SubagentSnapshot {
        use crate::runtime::subagent::{SubagentExecutionProfile, SubagentSnapshot, SubagentState};
        SubagentSnapshot {
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            parent_agent_id: AgentId::new("agent-parent"),
            subagent_id: crate::runtime::identity::SubagentId::new(
                "conv_57d68983-5497-771e-8aaa-5f1356061697",
            ),
            child_agent_id: AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new("conv_57d68983-5497-771e-8aaa-5f1356061697"),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-1"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            profile_digest: "sha256:p1".to_owned(),
            workspace: std::path::PathBuf::from("/shared-workspace"),
            state: SubagentState::Running,
            cancel_reason: None,
            detail: None,
            observation,
            profile: Some(std::sync::Arc::new(SubagentExecutionProfile {
                model: "local/model".to_owned(),
                profile: None,
                reasoning_enabled: false,
            })),
            settlement: crate::runtime::subagent::SubagentSettlement::default(),
            started_at: chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc),
        }
    }
    #[test]
    fn durable_agent_projection_preserves_identity_across_resume_and_rejects_old_activity() {
        use crate::runtime::subagent::{AgentState, SubagentState};
        let mut projection = projection();
        let first =
            agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
        projection.apply(owner_observation(first.clone(), AgentState::Active));
        let mut completed = first.clone();
        completed.state = SubagentState::Succeeded;
        completed.settlement = committed_settlement();
        projection.apply(owner_observation(completed.clone(), AgentState::Inactive));
        let (inactive, _) = projection.snapshot().unwrap();
        assert_eq!(inactive.agents.len(), 1);
        assert_eq!(inactive.agents[0].state, AgentState::Inactive);
        assert_eq!(inactive.agents[0].current_activation, None);
        projection.apply(ConversationObservation::SubagentLifecycle {
            snapshot: completed,
            agent: Some(Box::new(crate::runtime::subagent::AgentSnapshot {
                title: "Explore task".into(),
                agent_id: first.child_agent_id.clone(),
                conversation_id: first.child_conversation_id.clone(),
                parent_agent_id: first.parent_agent_id.clone(),
                agent: first.agent.clone(),
                state: AgentState::Admitting,
                current_activation: Some(crate::runtime::identity::SubagentId::new(
                    "activation-next",
                )),
                latest_activation: first.subagent_id.clone(),
                observation: first.observation.clone(),
            })),
        });
        let (reserving, _) = projection.snapshot().unwrap();
        assert_eq!(reserving.agents[0].state, AgentState::Admitting);
        assert_eq!(
            reserving.agents[0].current_activation,
            Some(crate::runtime::identity::SubagentId::new("activation-next"))
        );
        let mut resumed = first.clone();
        resumed.subagent_id = crate::runtime::identity::SubagentId::new("activation-next");
        projection.apply(owner_observation(resumed.clone(), AgentState::Active));
        let (_, resume_cursor) = projection.snapshot().unwrap();
        projection.apply(ConversationObservation::SubagentActivity(first.clone()));
        let (reconnected, cursor) = projection.snapshot().unwrap();
        assert_eq!(
            cursor, resume_cursor,
            "stale old activity publishes no update"
        );
        assert_eq!(reconnected.agents.len(), 1);
        let agent = &reconnected.agents[0];
        assert_eq!(agent.agent_id, first.child_agent_id);
        assert_eq!(agent.child_conversation_id, first.child_conversation_id);
        assert_eq!(agent.state, AgentState::Active);
        assert_eq!(agent.current_activation, Some(resumed.subagent_id.clone()));
        assert_eq!(agent.activation_id, resumed.subagent_id);
        let replay = collect(&mut projection, RuntimeClientCursor::new(0));
        let states: Vec<_> = replay
            .iter()
            .flat_map(|event| event.event.facts())
            .filter_map(|fact| match fact {
                RuntimeClientEvent::AgentUpdated { agent, .. } => {
                    Some((agent.agent_id.clone(), agent.state))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![
                (first.child_agent_id.clone(), AgentState::Active),
                (first.child_agent_id.clone(), AgentState::Inactive),
                (first.child_agent_id.clone(), AgentState::Admitting),
                (first.child_agent_id, AgentState::Active)
            ]
        );
    }

    #[test]
    fn authoritative_agent_bootstrap_keeps_latest_activation_then_accepts_live_resume() {
        use crate::runtime::subagent::{AgentSnapshot, AgentState, SubagentState};
        let mut projection = projection();
        let mut latest =
            agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
        latest.subagent_id = crate::runtime::identity::SubagentId::new("activation-3");
        latest.state = SubagentState::Succeeded;
        latest.settlement = committed_settlement();
        let owner = AgentSnapshot {
            title: "Explore task".into(),
            agent_id: latest.child_agent_id.clone(),
            conversation_id: latest.child_conversation_id.clone(),
            parent_agent_id: latest.parent_agent_id.clone(),
            agent: latest.agent.clone(),
            state: AgentState::Inactive,
            current_activation: None,
            latest_activation: latest.subagent_id.clone(),
            observation: latest.observation.clone(),
        };
        projection.bootstrap_agents(&[(owner, latest.clone())]);
        let (snapshot, cursor) = projection.snapshot().unwrap();
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert_eq!(snapshot.agents.len(), 1);
        assert_eq!(snapshot.agents[0].activation_id, latest.subagent_id);
        assert_eq!(snapshot.agents[0].state, AgentState::Inactive);
        let mut next = latest.clone();
        next.subagent_id = crate::runtime::identity::SubagentId::new("activation-4");
        next.state = SubagentState::Running;
        next.settlement = crate::runtime::subagent::SubagentSettlement::default();
        projection.apply(owner_observation(next.clone(), AgentState::Active));
        let (resumed, _) = projection.snapshot().unwrap();
        assert_eq!(resumed.agents.len(), 1);
        assert_eq!(resumed.agents[0].agent_id, latest.child_agent_id);
        assert_eq!(resumed.agents[0].current_activation, Some(next.subagent_id));
    }

    #[test]
    fn workflow_activations_do_not_become_continuable_agent_rows() {
        let mut projection = projection();
        let mut workflow =
            agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
        workflow.ownership = crate::events::types::SubagentOwnershipKind::Workflow;
        projection.apply(ConversationObservation::SubagentLifecycle {
            agent: None,
            snapshot: workflow.clone(),
        });
        projection.apply(ConversationObservation::SubagentActivity(workflow));
        let (snapshot, cursor) = projection.snapshot().unwrap();
        assert!(snapshot.agents.is_empty());
        assert_eq!(cursor, RuntimeClientCursor::new(0));
    }

    #[test]
    fn t20_replay_byte_budget_covers_every_event_and_evicts_in_cursor_order() {
        use super::RUNTIME_CLIENT_REPLAY_BYTES;
        let mut projection = projection();
        projection.replay_limit = super::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT;
        let event = |bytes: usize| RuntimeClientEvent::BlockContentAdvanced {
            position: 0,
            attempt_id: attempt(),
            message_id: MessageId::new("message"),
            block_index: ContentBlockIndex::new(0),
            kind: "text".into(),
            base_length: 0,
            end_length: bytes as u64,
            content_revision: 1,
            inline_delta: Some("x".repeat(bytes)),
            content_ref: crate::runtime_client::content::ContentRef {
                conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
                source: crate::runtime_client::content::ContentSource::LivePublicationPrefix {
                    component: crate::runtime_client::content::PublicationContentComponent::Payload,
                    stream_id: crate::runtime::identity::PublicationStreamId::new("stream"),
                    message_id: MessageId::new("message"),
                    attempt_id: attempt(),
                    turn_id: crate::runtime::identity::TurnId::new("turn"),
                    publication_request_id: crate::runtime::identity::RequestId::new("request"),
                    block_index: ContentBlockIndex::new(0),
                },
                representation: crate::runtime_client::content::ContentRepresentation::Utf8Text,
                version: 1,
                content_revision: 1,
                readable_end: bytes as u64,
            },
        };
        for _ in 0..600 {
            projection.publish(event(32 * 1024));
            assert!(projection.replay_bytes <= RUNTIME_CLIENT_REPLAY_BYTES);
            assert_eq!(projection.replay.len(), projection.replay_sizes.len());
        }
        assert!(
            projection.replay.len() < 600,
            "text events must consume the byte budget"
        );
        let first = projection.replay.front().unwrap().0.get();
        for (index, (cursor, _)) in projection.replay.iter().enumerate() {
            assert_eq!(cursor.get(), first + u64::try_from(index).unwrap());
        }
        let actual: usize = projection
            .replay
            .iter()
            .map(|(_, event)| serde_json::to_vec(event).unwrap().len())
            .sum();
        assert_eq!(actual, projection.replay_bytes);
        projection.publish(event(RUNTIME_CLIENT_REPLAY_BYTES + 1));
        assert!(
            projection
                .replay
                .back()
                .unwrap()
                .1
                .facts()
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::CheckpointAdvanced { .. }))
        );
        assert!(projection.replay_bytes <= RUNTIME_CLIENT_REPLAY_BYTES);
        assert!(projection.replay.iter().all(|(_, event)| {
            super::super::checkpoint::bounded_bytes(
                event,
                super::super::checkpoint::CHECKPOINT_TARGET_BYTES,
            )
            .is_some()
        }));
    }
    #[test]
    fn t14_t20_retained_cut_lookup_uses_exact_metadata_and_bounded_cursor_index() {
        let mut projection = projection();
        projection.replay_limit = 1;
        let mut cut = projection.checkpoint_read_cut();
        cut.revision = 9_001;
        projection.cursor = RuntimeClientCursor::new(1);
        projection.retain_replay(
            RuntimeClientEvent::ProjectionTransition {
                events: Vec::new(),
                metadata: super::super::checkpoint::ProjectionMetadata {
                    read_cut: cut.clone(),
                    collections: Vec::new(),
                    content: Vec::new(),
                },
            },
            1,
        );
        assert!(projection.validate_checkpoint_cut(&cut).is_ok());
        let mut tampered = cut.clone();
        tampered.through += 1;
        assert!(matches!(
            projection.validate_checkpoint_cut(&tampered),
            Err(super::RuntimeClientError::ReadCutExpired)
        ));
        let mut checkpoint = projection.checkpoint_candidate().unwrap();
        checkpoint.read_cut = cut.clone();
        checkpoint.read_cut.revision = 9_002;
        let replacement_cut = checkpoint.read_cut.clone();
        projection.cursor = RuntimeClientCursor::new(2);
        projection.retain_replay(
            RuntimeClientEvent::CheckpointAdvanced {
                transition: "retained replacement".into(),
                checkpoint: Box::new(checkpoint),
            },
            1,
        );
        assert!(projection.validate_checkpoint_cut(&replacement_cut).is_ok());
        assert!(matches!(
            projection.validate_checkpoint_cut(&cut),
            Err(super::RuntimeClientError::ReadCutExpired)
        ));
        assert_eq!(projection.replay_cut_cursors.len(), 1);
        assert_eq!(
            projection.replay_cut_cursors.get(&9_002),
            Some(&RuntimeClientCursor::new(2))
        );
    }

    #[test]
    fn t06_t14_oversized_transition_is_one_foldable_checkpoint_and_converges() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let initial = projection.checkpoint_candidate().unwrap();
        let message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: MessageId::new("large-canonical"),
            content: vec![AssistantContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "🙂\"".repeat(400_000),
                },
            )],
        });
        let receipt = store.append_canonical(&message).unwrap();
        projection.begin_publication();
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: receipt.clone(),
        });
        let mut draft = projection.finish_publication().unwrap();
        // Inject the actual original C value only into its encoded candidate.
        // Normal producers already defer this body; the source cut, finite
        // state and reserved cursor still come from that exact native commit.
        draft.events = vec![RuntimeClientEvent::MessageCommitted {
            evicted_message_id: None,
            attempt_id: None,
            message,
            transcript_cursor: receipt.transcript_cursor.map(Into::into),
        }];
        let encoded = draft.encode().unwrap();
        assert!(encoded.bytes <= super::super::checkpoint::CHECKPOINT_TARGET_BYTES);
        projection.install_publication(encoded);
        assert_eq!(projection.cursor.get(), initial.cursor.get() + 1);
        let (cursor, event) = projection.replay.back().unwrap();
        let RuntimeClientEvent::ProjectionTransition { events, metadata } = event else {
            panic!("one complete native transition");
        };
        let [
            RuntimeClientEvent::CheckpointAdvanced {
                transition,
                checkpoint,
            },
        ] = events.as_slice()
        else {
            panic!("one bounded authoritative replacement");
        };
        assert_eq!(transition, "projection_transition");
        assert_eq!(checkpoint.cursor, *cursor);
        let mut folded = checkpoint.as_ref().clone();
        folded.read_cut.clone_from(&metadata.read_cut);
        folded.content.clone_from(&metadata.content);
        folded.collections.clone_from(&metadata.collections);
        assert_eq!(
            folded,
            projection.checkpoint_candidate().unwrap(),
            "complete replacement preserves all cut, domain, identity, frontier, and settlement fields"
        );
        assert!(
            super::super::checkpoint::bounded_bytes(
                event,
                super::super::checkpoint::CHECKPOINT_TARGET_BYTES
            )
            .is_some()
        );
        let payload = metadata
            .content
            .iter()
            .find(|binding| {
                matches!(
                    binding.content_ref.source,
                    super::super::content::ContentSource::CanonicalMessageBlock {
                        component: super::super::content::CanonicalContentComponent::Payload,
                        ..
                    }
                )
            })
            .expect("original native canonical payload remains readable");
        assert_eq!(payload.content_ref.readable_end, 2_000_000);
        let cursor = *cursor;
        let (subscriber, _) = projection.subscribe(initial.cursor).unwrap();
        let SubscriberPoll::Event(observed) = projection.poll_subscriber(subscriber) else {
            panic!("event remains retained and serviceable")
        };
        assert_eq!(observed.cursor, cursor);
    }

    #[test]
    fn t19_atomic_canonical_batch_advances_its_exact_transcript_interval() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let mut projection = projection();
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            projection.snapshot.conversation_id.clone(),
            &directory.path().join("history.sqlite"),
        )
        .unwrap();
        projection
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        let messages = (0..73)
            .map(|index| {
                MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
                    id: MessageId::new(format!("canonical-batch-{index}")),
                    content: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        let receipts = store.append_canonical_batch(&messages).unwrap();
        assert_eq!(receipts[0].observation.transcript_count, 73);
        let cursor = projection.cursor;
        for receipt in receipts {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        }
        assert_eq!(projection.cursor.get(), cursor.get() + 1);
        assert_eq!(projection.transcript_prefix, 73);
        projection.validate_source_prefix().unwrap();
        assert_eq!(projection.snapshot.message_descriptors.len(), 8);
        let read = projection.checkpoint_read_cut().source_cut.unwrap();
        assert_eq!(read.read.transcript, 73);
        assert_eq!(read.transcript_total, 73);
    }

    fn publish_source_fact(
        projection: &mut RuntimeClientProjection,
        fold: impl FnOnce(&mut RuntimeClientProjection),
    ) {
        projection.begin_publication();
        projection.publication_commit_scope(true, true);
        fold(projection);
        projection.publication_commit_scope(false, true);
        if let Some(draft) = projection.finish_publication() {
            projection.install_publication(draft.encode().unwrap());
        }
    }

    mod metadata_ownership_tests {
        use super::*;
        include!("projection/metadata_ownership_tests.rs");
    }

    mod metadata_root_tests {
        use super::*;
        include!("projection/metadata_root_tests.rs");
    }

    mod incremental_checkpoint_tests {
        use super::*;
        include!("projection/incremental_checkpoint_tests.rs");
    }

    mod model_turn_settlement_tests {
        use super::*;
        include!("projection/model_turn_settlement_tests.rs");
    }

    mod prepared_trace_tests {
        use super::*;
        include!("projection/prepared_trace_tests.rs");
    }

    mod cached_trace_tests {
        use super::*;
        include!("projection/cached_trace_tests.rs");
    }

    fn initialized_source_projection() -> (
        RuntimeClientProjection,
        crate::durable::SqliteConversationStore,
    ) {
        use crate::durable::ConversationStore;
        let mut projection = projection();
        let store = crate::durable::SqliteConversationStore::in_memory(
            projection.snapshot.conversation_id.clone(),
        )
        .unwrap();
        projection
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        projection.initialize_publication_view();
        (projection, store)
    }

    #[test]
    fn worker_read_candidate_keeps_native_cut_without_capturing_checkpoint() {
        let (mut projection, store) = initialized_source_projection();
        let published = projection.checkpoint_seed();
        assert!(!published.trace_head.prepared());
        projection.request_read_domains_for_test();
        let candidate = projection.read_domain_candidate().unwrap();
        assert_eq!(candidate, projection.read_domain_fence());
        assert!(
            !published.trace_head.prepared(),
            "worker admission must not capture or fit a published checkpoint"
        );
        let read = candidate.read(&store).unwrap();
        assert_eq!(
            read.transcript.source_cut.as_ref(),
            candidate.source_cut.as_ref()
        );
        assert!(projection.install_read_domains(&candidate, Ok(read), false));
        assert!(!published.trace_head.prepared());
        assert!(projection.read_domain_candidate().is_none());
        // The explicit snapshot test API still captures its own published view.
        let snapshot = projection.snapshot_candidate().unwrap();
        assert!(published.trace_head.prepared());
        assert_eq!(snapshot.cursor, published.cursor);
        assert_eq!(snapshot.read_cut, candidate);
    }

    #[test]
    fn t14_t20_initial_cut_expires_with_its_replay_cursor() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        projection.replay_limit = 1;
        let initial = projection.checkpoint_candidate().unwrap().read_cut;
        assert_eq!(
            projection.replay_bytes,
            super::super::checkpoint::CHECKPOINT_TARGET_BYTES
        );
        for ordinal in 1..=2 {
            let receipt = store
                .append_canonical(&compactable_user(&format!("initial-cut-{ordinal}")))
                .unwrap();
            publish_source_fact(&mut projection, |projection| {
                projection.apply(ConversationObservation::CanonicalCommitted {
                    attempt_id: None,
                    receipt,
                });
            });
            assert!(projection.replay_bytes <= super::RUNTIME_CLIENT_REPLAY_BYTES);
            if ordinal == 1 {
                assert!(projection.validate_checkpoint_cut(&initial).is_ok());
                let mut tampered = initial.clone();
                tampered.through += 1;
                assert!(projection.validate_checkpoint_cut(&tampered).is_err());
            }
        }
        assert!(projection.initial_read_cut.is_none());
        assert!(
            !projection
                .replay_cut_cursors
                .contains_key(&initial.revision)
        );
        assert_eq!(
            projection.replay_bytes,
            projection.replay_sizes.iter().sum::<usize>()
        );
        assert!(matches!(
            projection.validate_checkpoint_cut(&initial),
            Err(super::RuntimeClientError::ReadCutExpired)
        ));
    }

    #[test]
    fn t14_t20_initial_cut_metadata_is_charged_to_the_replay_byte_budget() {
        let (mut projection, _store) = initialized_source_projection();
        let initial = projection.checkpoint_candidate().unwrap().read_cut;
        for cursor in 1..=1_000 {
            // Stress real schema-aware record weights at the existing replay
            // boundary; no source or execution state is changed by this probe.
            let mut seed = projection.capture_working_seed();
            seed.cursor = RuntimeClientCursor::new(cursor);
            seed.read_cut.revision = cursor;
            let event = RuntimeClientEvent::CheckpointAdvanced {
                transition: "bounded replay weight ".repeat(1_500),
                checkpoint: Box::new(seed.checkpoint_candidate().unwrap()),
            };
            let bytes = super::encoded_event_bytes(
                &event,
                super::super::checkpoint::CHECKPOINT_TARGET_BYTES,
            )
            .unwrap()
            .len();
            projection.install_publication(super::EncodedPublication {
                seed: Arc::new(seed),
                event,
                bytes,
                sources: projection.domain_sources.clone(),
            });
            assert!(projection.replay_bytes <= super::RUNTIME_CLIENT_REPLAY_BYTES);
            let reservation = usize::from(projection.initial_read_cut.is_some())
                * super::super::checkpoint::CHECKPOINT_TARGET_BYTES;
            assert_eq!(
                projection.replay_bytes,
                projection.replay_sizes.iter().sum::<usize>() + reservation
            );
            if projection.initial_read_cut.is_none() {
                assert!(projection.earliest_serviceable().get() > 0);
                assert!(projection.validate_checkpoint_cut(&initial).is_err());
                return;
            }
        }
        panic!("encoded byte pressure must retire the initial cut before the count limit");
    }

    #[test]
    fn t14_native_admission_publishes_frozen_model_before_execution_starts() {
        let (mut projection, _store) = initialized_source_projection();
        let initial = projection.checkpoint_candidate().unwrap();
        let session = crate::scripted_suites::support::model::scripted_session_model(Arc::new(
            crate::scripted_suites::support::model::NullAdapter,
        ));
        let model = crate::model::session::AttemptModelReadSource::new(session.snapshot().unwrap());
        model.prepare();
        let frozen = model.inline();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::AttemptAdmitted {
                read_image_active: false,
                attempt_id: attempt(),
                model,
                resource_revision: crate::runtime::identity::RuntimeResourceRevision::new(7),
                approval_mode: crate::runtime::types::ApprovalMode::Policy,
            });
        });
        let current = projection.checkpoint_candidate().unwrap();
        assert_eq!(current.cursor.get(), initial.cursor.get() + 1);
        assert_eq!(current.read_cut.through, initial.read_cut.through);
        let admitted = current.snapshot.attempt.as_ref().unwrap();
        assert_eq!(admitted.phase, RuntimeClientAttemptPhase::Admitted);
        assert_eq!(admitted.model.as_deref(), frozen.as_ref());
        assert_eq!(admitted.turn, 0);
        assert!(admitted.foreground.is_empty());
        let event = &projection.replay.back().unwrap().1;
        assert!(
            !event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        );
        assert!(event.facts().iter().any(|fact| matches!(fact,
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. }
                if checkpoint.snapshot.attempt == current.snapshot.attempt)));
    }

    fn source_tool_event(
        store: &dyn crate::durable::ConversationStore,
        event: RuntimeEvent,
        identity: &str,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(identity),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt()),
            turn_id: Some(TurnId::new("1")),
            timestamp: chrono::DateTime::UNIX_EPOCH,
            tool_occurrence: None,
            observation: None,
            event,
        }
    }

    fn native_running_tool_fixture() -> (
        RuntimeClientProjection,
        crate::durable::SqliteConversationStore,
        ToolCall,
        crate::durable::observation::ObservationEventWitness,
    ) {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let started = append_source_attempt_start(&store, "progress-attempt-start");
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(started.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(started.sequence, started.sequence)
            .unwrap();
        let call = ToolCall {
            id: ToolCallId::new("progress-call"),
            tool_id: ToolId::new("progress-tool"),
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        };
        let message = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("progress-proposal"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        });
        let (committed, _) = store
            .append_canonical_with_event(
                &message,
                source_tool_event(
                    &store,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: message.id().clone(),
                    },
                    "progress-proposal-commit",
                ),
            )
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(committed.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(committed.sequence, committed.sequence)
            .unwrap();
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        // The display baseline is the actual canonical assembled occurrence;
        // execution state below comes exclusively from its native Started J.
        projection.snapshot.attempt.as_mut().unwrap().foreground =
            projection.snapshot.message_descriptors[0]
                .tool_calls
                .clone();
        let started = store
            .append_event(source_tool_event(
                &store,
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
                "progress-tool-start",
            ))
            .unwrap();
        let prepared = store
            .observation_event_witness(started.sequence, started.sequence)
            .unwrap()
            .unwrap();
        assert!(!prepared.native_direct);
        assert!(prepared.tool_occurrence.is_some());
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(started.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(started.sequence, started.sequence)
            .unwrap();
        (projection, store, call, prepared)
    }

    #[test]
    fn t14_t19_prepared_started_witness_preserves_exact_live_progress_and_terminal_facts() {
        use crate::durable::ConversationStore;
        let (mut projection, store, call, prepared) = native_running_tool_fixture();
        let historical = projection.checkpoint_seed();
        let progress = ToolProgress {
            message: Some("phase 1".into()),
            ..ToolProgress::default()
        };
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::ToolOccurrenceProof {
                occurrence: prepared.tool_occurrence.clone().unwrap(),
                observation: Box::new(ConversationObservation::ToolProgress {
                    attempt_id: attempt(),
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    progress: progress.clone(),
                }),
            });
        });
        let before = projection.checkpoint_candidate().unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::JournalBatch {
                through: Some(prepared.sequence),
                observations: vec![ConversationObservation::JournalFact(prepared.clone())],
            });
        });
        assert_eq!(projection.checkpoint_candidate().unwrap(), before);
        let seed = projection.checkpoint_seed();
        let slot = &before.snapshot.attempt.as_ref().unwrap().foreground[0];
        assert!(
            matches!(&slot.state, ForegroundToolState::Running { progress: Some(value), .. } if value == &progress)
        );
        let mut page = super::super::tool_observation::page(
            &store,
            &before.read_cut,
            "attempt.foreground",
            &slot.occurrence.locator,
            0,
            32,
        )
        .unwrap();
        historical.retain_foreground_progress(&mut page).unwrap();
        let durable: super::super::snapshot::ForegroundToolExecution =
            super::super::checkpoint::decode_domain_value(page.items[0].value.clone().unwrap())
                .unwrap();
        assert!(matches!(
            durable.state,
            ForegroundToolState::Running { progress: None, .. }
        ));
        seed.retain_foreground_progress(&mut page).unwrap();
        let live: super::super::snapshot::ForegroundToolExecution =
            super::super::checkpoint::decode_domain_value(page.items[0].value.clone().unwrap())
                .unwrap();
        assert_eq!(live, *slot);
        let terminal = store
            .append_event(source_tool_event(
                &store,
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call.id,
                    tool_id: call.tool_id,
                    result: success_result(),
                },
                "progress-tool-completed",
            ))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(terminal.observation.as_deref().unwrap());
        });
        let settled = projection.checkpoint_candidate().unwrap();
        assert!(settled.cursor > before.cursor);
        assert!(matches!(
            settled.snapshot.attempt.as_ref().unwrap().foreground[0].state,
            ForegroundToolState::Settled { .. }
        ));
        assert!(
            projection
                .replay
                .back()
                .unwrap()
                .1
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::ToolExecutionSettled { .. }))
        );
    }

    fn append_source_attempt_start(
        store: &dyn crate::durable::ConversationStore,
        identity: &str,
    ) -> RuntimeEventEnvelope {
        store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new(identity),
                sequence: 0,
                conversation_id: store.conversation_id().clone(),
                attempt_id: Some(attempt()),
                turn_id: None,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                tool_occurrence: None,
                observation: None,
                event: RuntimeEvent::AttemptStarted {
                    attempt_id: attempt(),
                },
            })
            .unwrap()
    }

    /// An earlier native owner can delay release indefinitely without creating
    /// a sparse skip map or granting future J/C/pending source access.
    #[test]
    fn t14_t20_native_direct_receipts_wait_for_the_exact_earlier_source_prefix() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let initial = projection.checkpoint_read_cut();
        let earlier = append_source_attempt_start(&store, "held-native-owner");
        let mut goal = None;
        for index in 0..73 {
            let write = goal.map_or_else(
                || crate::goal::GoalWrite::Create {
                    objective: "native goal 0".into(),
                    budget: 3,
                    origin: crate::goal::GoalOrigin::RuntimeControl,
                },
                |expected| crate::goal::GoalWrite::Mutate {
                    expected,
                    mutation: crate::goal::GoalMutation::Edit {
                        objective: format!("native goal {index}"),
                    },
                },
            );
            let (result, receipt) = store.write_goal(write).unwrap();
            goal = Some(result.unwrap().reference);
            let receipt = receipt.unwrap();
            let native = receipt.native.as_ref().unwrap();
            publish_source_fact(&mut projection, |projection| {
                projection.apply(ConversationObservation::GoalChanged(receipt.view.clone()));
                projection.apply_native_batch(native);
            });
            assert!(projection.native_receipt_installation().is_installed());
            let (first, last) = native.journal_range.unwrap();
            store.mark_observation_direct(first, last).unwrap();
            assert_eq!(store.observation_ready_frontier(0, last).unwrap(), 0);
            assert_eq!(projection.checkpoint_read_cut().through, 0);
            assert_eq!(projection.checkpoint_read_cut(), initial);
            assert!(projection.working_read_cut().source_cut.is_none());
            assert!(
                projection.snapshot_ref().goal.is_none(),
                "published C retains its earlier Goal state"
            );
            assert_eq!(
                projection
                    .snapshot
                    .goal
                    .as_ref()
                    .unwrap()
                    .current
                    .as_ref()
                    .unwrap()
                    .reference,
                goal.clone().unwrap()
            );
        }
        let latest = projection.source_cut.clone().unwrap();
        assert_eq!(projection.native_journal_highwater, 74);
        assert_eq!(
            projection
                .snapshot
                .goal
                .as_ref()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .objective,
            "native goal 72"
        );
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(earlier.observation.as_ref().unwrap());
        });
        assert_eq!(
            projection.source_cut.as_ref().unwrap(),
            &latest,
            "the delayed owner's cut cannot roll back the later installed source"
        );
        store
            .mark_observation_direct(earlier.sequence, earlier.sequence)
            .unwrap();
        assert_eq!(store.observation_ready_frontier(0, 74).unwrap(), 74);
        for sequence in 1..=74 {
            let witness = store
                .observation_event_witness(sequence, 74)
                .unwrap()
                .unwrap();
            assert!(witness.native_direct);
            publish_source_fact(&mut projection, |projection| {
                projection.apply(ConversationObservation::JournalBatch {
                    through: Some(sequence),
                    observations: vec![ConversationObservation::JournalFact(witness)],
                });
            });
        }
        assert_released_goal_prefix(&projection, &goal.unwrap());
    }

    fn assert_released_goal_prefix(
        projection: &RuntimeClientProjection,
        goal: &crate::goal::GoalRef,
    ) {
        assert_eq!(projection.checkpoint_read_cut().through, 74);
        assert_eq!(
            projection
                .checkpoint_read_cut()
                .source_cut
                .as_ref()
                .unwrap()
                .read
                .journal,
            74
        );
        assert_eq!(
            projection
                .snapshot_ref()
                .goal
                .as_ref()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .reference,
            *goal
        );
        assert_eq!(
            projection
                .snapshot_ref()
                .attempt
                .as_ref()
                .unwrap()
                .attempt_id,
            Some(attempt())
        );
        assert!(projection.prepared_journal.is_empty());
        assert!(projection.prepared_outcomes.is_empty());
    }

    #[test]
    fn t14_t20_no_j_canonical_and_pending_cuts_do_not_authorize_a_held_journal_owner() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let published = projection.checkpoint_read_cut();
        let earlier = append_source_attempt_start(&store, "held-before-no-j-c");
        let message = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("native-no-j-c"),
            content: Vec::new(),
        });
        let canonical = store.append_canonical(&message).unwrap();
        assert!(canonical.observation.journal_range.is_none());
        let pending = store
            .accept_inbound(crate::durable::inbox::InboundDraft {
                message_id: None,
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "later pending owner".into(),
                    },
                )],
                timestamp: chrono::DateTime::UNIX_EPOCH,
                correlation: None,
            })
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_pending_receipt(pending.pending_observation.as_deref());
        });
        let latest = projection.source_cut.clone().unwrap();
        assert_eq!(
            latest.read.mutation_revision,
            pending.read_cut.mutation_revision
        );
        assert_eq!(projection.native_journal_highwater, earlier.sequence);
        assert_eq!(projection.checkpoint_read_cut().through, 0);
        assert_eq!(projection.checkpoint_read_cut(), published);
        assert!(projection.working_read_cut().source_cut.is_none());
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(earlier.observation.as_ref().unwrap());
        });
        assert_eq!(projection.source_cut.as_ref().unwrap(), &latest);
        assert_eq!(
            projection.checkpoint_read_cut(),
            published,
            "uninstalled C cannot mutate the published seed"
        );
        assert!(
            projection.working_read_cut().source_cut.is_none(),
            "uninstalled C still has its own owner"
        );
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: canonical,
            });
        });
        store
            .mark_observation_direct(earlier.sequence, earlier.sequence)
            .unwrap();
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        store
            .mark_observation_transcript_ready(pending.read_cut.transcript)
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::TranscriptSourceReady(
                store.transcript_source_ready().unwrap(),
            ));
        });
        let cut = projection.checkpoint_read_cut();
        let source = cut.source_cut.unwrap();
        assert_eq!(source, store.observation_read_cut().unwrap());
        assert_eq!(
            source.read.mutation_revision,
            pending.read_cut.mutation_revision
        );
        assert_eq!(source.ledger_through, 1);
        assert_eq!(source.read.transcript, 2);
        assert_eq!(projection.snapshot_ref().inbound.pending.len(), 1);
        assert_eq!(projection.snapshot_ref().message_descriptors.len(), 1);
        assert_eq!(
            projection
                .snapshot_ref()
                .attempt
                .as_ref()
                .unwrap()
                .attempt_id,
            Some(attempt())
        );
    }

    #[test]
    fn t14_t19_complete_canonical_head_survives_an_earlier_delayed_callback() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let initial = projection.checkpoint_read_cut();
        let first = store
            .append_canonical(&MessageBlock::User(
                crate::message::types::UserMessageBlock {
                    id: MessageId::new("first-native-user"),
                    source: crate::message::types::UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    timestamp: Some(chrono::DateTime::UNIX_EPOCH),
                    content: vec![crate::message::types::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: "first native preview".into(),
                        },
                    )],
                },
            ))
            .unwrap();
        let second = store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("second-native-c"),
                content: Vec::new(),
            }))
            .unwrap();
        assert_eq!(
            second.observation.head.len(),
            2,
            "later native transaction captures complete global canonical head"
        );
        assert_eq!(
            second.observation.opening_preview.as_deref(),
            Some("first native preview")
        );
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: second,
            });
        });
        assert_eq!(
            projection.checkpoint_read_cut(),
            initial,
            "a global head does not release the earlier owned T"
        );
        assert_eq!(projection.snapshot.message_descriptors.len(), 2);
        let latest = projection.source_cut.clone();
        store.mark_canonical_transcript_ready(2, 2).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: first,
            });
        });
        assert_eq!(
            projection.source_cut, latest,
            "earlier complete view cannot roll back newer cut"
        );
        assert_eq!(
            projection.snapshot.message_descriptors.len(),
            2,
            "earlier callback does not duplicate the complete head"
        );
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::TranscriptSourceReady(
                store.transcript_source_ready().unwrap(),
            ));
        });
        assert_eq!(projection.cursor.get(), initial.revision + 1);
        assert_eq!(
            projection.checkpoint_read_cut().source_cut.unwrap(),
            store.observation_read_cut().unwrap()
        );
        assert_eq!(
            projection
                .snapshot
                .messages
                .iter()
                .map(|message| message.id().as_str())
                .collect::<Vec<_>>(),
            ["first-native-user", "second-native-c"]
        );
    }

    fn assert_canonical_header_fact(
        projection: &mut RuntimeClientProjection,
        header: &crate::durable::observation::MessageHeader,
        owns_header: bool,
    ) {
        assert!(
            !projection
                .fold_canonical_header(None, header.clone(), header.transcript_cursor, owns_header)
                .is_empty()
        );
    }

    #[test]
    fn t14_t19_complete_canonical_head_omits_only_exact_unchanged_rows() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let [first, second] = canonical_capture_receipts(&store);
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: first,
            });
        });
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        let cursor = projection.cursor;
        let earlier = second.observation.head[0].clone();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: second,
            });
        });
        let facts = projection
            .replay
            .iter()
            .filter(|(at, _)| *at > cursor)
            .flat_map(|(_, event)| event.facts())
            .collect::<Vec<_>>();
        assert_eq!(
            facts
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeClientEvent::MessageDescriptorCommitted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            facts
                .iter()
                .filter(|event| matches!(event, RuntimeClientEvent::MessageCommitted { .. }))
                .count(),
            1
        );
        assert_eq!(projection.snapshot.message_descriptors.len(), 2);
        assert!(
            projection
                .fold_canonical_header(None, earlier.clone(), earlier.transcript_cursor, false)
                .is_empty()
        );
        let mut changed_body = earlier.clone();
        let Some(MessageBlock::User(body)) = &mut changed_body.display else {
            panic!("native User source header")
        };
        let UserContentBlock::Text(text) = &mut body.content[0] else {
            panic!("native User text")
        };
        text.text.push_str(" changed");
        assert_canonical_header_fact(&mut projection, &changed_body, false);
        assert_canonical_header_fact(&mut projection, &earlier, false);
        let mut refined = earlier.clone();
        refined.content[0].content_ref.readable_end += 1;
        assert_canonical_header_fact(&mut projection, &refined, false);
        assert_canonical_header_fact(&mut projection, &earlier, false);
        assert!(
            projection
                .fold_canonical_header(None, earlier.clone(), earlier.transcript_cursor, false)
                .is_empty()
        );
        assert_canonical_header_fact(&mut projection, &earlier, true);
    }

    fn canonical_capture_wire<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
    ) -> serde_json::Value {
        let mut bytes = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut bytes, value).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn canonical_capture_case(
        name: &str,
        initial: crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
        projection: &RuntimeClientProjection,
    ) -> serde_json::Value {
        use crate::app_server::protocol::{
            AttachmentTarget, JsonRpcVersion, MethodResult, Notification, NotificationMethod,
            RequestId as WireId, Response, Success,
        };
        let expected = projection.checkpoint_candidate().unwrap();
        let target = AttachmentTarget {
            session_id: crate::local_runtime::session::SessionId::new(
                "ses_00000000-0000-7000-8000-000000000421",
            ),
            conversation_id: initial.snapshot.conversation_id.clone(),
            runtime_incarnation: serde_json::from_str("1").unwrap(),
            attachment_id: crate::runtime_client::AttachmentId::new("canonical-capture"),
        };
        let events = projection
            .replay
            .iter()
            .filter(|(cursor, _)| *cursor > initial.cursor)
            .map(|(cursor, event)| {
                assert!(
                    !event
                        .facts()
                        .iter()
                        .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
                );
                canonical_capture_wire(&Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification: NotificationMethod::Event {
                        target: target.clone(),
                        registration_id: "canonical-capture".into(),
                        cursor: *cursor,
                        event: Box::new(event.clone()),
                    },
                })
            })
            .collect::<Vec<_>>();
        assert!(!events.is_empty());
        let checkpoint = |value| {
            canonical_capture_wire(&Response::Success(Box::new(Success {
                jsonrpc: JsonRpcVersion::V2,
                id: WireId::String("canonical-capture".into()),
                result: MethodResult::Checkpoint {
                    checkpoint: Box::new(value),
                },
            })))
        };
        serde_json::json!({
            "name": name, "initial": checkpoint(initial),
            "events": events, "expected": checkpoint(expected),
        })
    }

    fn canonical_capture_receipts(
        store: &dyn crate::durable::ConversationStore,
    ) -> [crate::durable::inbox::TranscriptCommitReceipt; 2] {
        let first = store
            .append_canonical(&MessageBlock::User(UserMessageBlock {
                id: MessageId::new("boundary-user"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: Some(chrono::DateTime::UNIX_EPOCH),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "boundary preview".into(),
                })],
            }))
            .unwrap();
        let second = store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("boundary-assistant"),
                content: Vec::new(),
            }))
            .unwrap();
        [first, second]
    }

    fn canonical_tool_result_capture_case() -> serde_json::Value {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let call = ToolCall {
            id: ToolCallId::new("canonical-boundary-call"),
            tool_id: ToolId::new("canonical-boundary-tool"),
            name: "boundary_tool".into(),
            arguments: serde_json::json!({}),
        };
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("canonical-boundary-proposal"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        });
        let receipt = store.append_canonical(&assistant).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        });
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        let initial = projection.checkpoint_candidate().unwrap();
        let result = cancelled_result(
            CancellationReason::RuntimeShutdown,
            ToolCancellationPhase::BeforeStart,
        );
        let receipt = store
            .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    assistant.id().clone(),
                    ContentBlockIndex::new(0),
                ),
                id: MessageId::new("canonical-boundary-result"),
                tool_call_id: call.id,
                tool_id: call.tool_id,
                result: result.clone(),
            }))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        });
        let descriptor = &projection.snapshot.message_descriptors[0];
        let slot = &descriptor.tool_calls[0];
        assert_eq!(slot.committed_result.as_ref(), Some(&result));
        assert!(matches!(slot.state, ForegroundToolState::Assembled { .. }));
        assert!(slot.result_metadata_ref.is_some());
        assert!(
            descriptor.collections.iter().any(|frontier| frontier.domain
                == "messages.ledger:2.blocks"
                && frontier.total == 0)
        );
        canonical_capture_case(
            "tool result C metadata before terminal J",
            initial,
            &projection,
        )
    }

    /// App-encoded native checkpoints isolate canonical publication from the
    /// asynchronous transcript read domain; no client fold produces the oracle.
    #[test]
    fn t14_native_canonical_boundary_independent_checkpoint_capture() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let initial = projection.checkpoint_candidate().unwrap();
        let [first, second] = canonical_capture_receipts(&store);
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: first,
            });
        });
        let first_case = canonical_capture_case(
            "first canonical C before transcript refresh",
            initial,
            &projection,
        );
        let initial = projection.checkpoint_candidate().unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: second,
            });
        });
        let second_case = canonical_capture_case(
            "later canonical C with empty block domain",
            initial,
            &projection,
        );
        let (mut delayed, store) = initialized_source_projection();
        let initial = delayed.checkpoint_candidate().unwrap();
        let [first, second] = canonical_capture_receipts(&store);
        publish_source_fact(&mut delayed, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: second,
            });
        });
        assert_eq!(delayed.checkpoint_candidate().unwrap(), initial);
        store.mark_canonical_transcript_ready(2, 2).unwrap();
        publish_source_fact(&mut delayed, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: first,
            });
        });
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        publish_source_fact(&mut delayed, |projection| {
            projection.apply(ConversationObservation::TranscriptSourceReady(
                store.transcript_source_ready().unwrap(),
            ));
        });
        let delayed_case =
            canonical_capture_case("complete C2 head after delayed C1 owner", initial, &delayed);
        let tool_case = canonical_tool_result_capture_case();
        if let Ok(path) = std::env::var("RUSTX_CANONICAL_CAPTURE") {
            std::fs::write(
                path,
                serde_json::to_string_pretty(&serde_json::json!({
                    "cases": [first_case, second_case, delayed_case, tool_case],
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn t14_t19_hidden_canonical_dependency_fences_a_native_audit_without_a_t_gap() {
        use crate::durable::ConversationStore;
        let fixture = crate::durable::SqliteConversationStore::hidden_context_audit_fixture();
        let mut projection = projection();
        projection.snapshot.conversation_id = fixture.before.read.conversation_id.clone();
        projection
            .initialize_descriptor_heads(fixture.store.as_ref(), fixture.before.clone())
            .unwrap();
        projection.set_inspection_frontier(fixture.before.read.journal);
        projection.initialize_publication_view();
        let initial = projection.checkpoint_read_cut();
        assert!(fixture.context.transcript_cursor.is_none());
        assert_eq!(fixture.audit.transcript_cursor.get(), 1);
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::AuditSettled {
                attempt_id: fixture.audit.summary.attempt_id.clone().unwrap(),
                receipt: fixture.audit.clone(),
            });
        });
        assert_eq!(
            projection.transcript_prefix, 1,
            "there is no hidden T hole masking the C dependency"
        );
        assert_eq!(projection.native_canonical_highwater, 1);
        assert_eq!(projection.canonical_observed_through, 0);
        assert_eq!(
            projection.checkpoint_read_cut(),
            initial,
            "audit cannot publish SourceCutNone across an uninstalled hidden C"
        );
        assert!(matches!(
            projection.validate_source_prefix(),
            Err(super::RuntimeClientError::ReadCutExpired)
        ));
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: fixture.context,
            });
        });
        assert_eq!(projection.cursor.get(), initial.revision + 1);
        assert_eq!(projection.snapshot.publication_settlements.len(), 1);
        assert_eq!(
            projection.checkpoint_read_cut().source_cut.unwrap(),
            fixture.store.observation_read_cut().unwrap()
        );
    }

    #[test]
    fn t19_published_checkpoint_is_independent_of_pending_native_owner_work() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let mut projection = projection();
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            projection.snapshot.conversation_id.clone(),
            &directory.path().join("history.sqlite"),
        )
        .unwrap();
        projection
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        projection.initialize_publication_view();
        let published = projection.checkpoint_seed();
        let cut = published.read_cut.clone();
        projection.observe_transcript_position(crate::durable::TranscriptCursor::new(2));
        assert!(
            projection.validate_source_prefix().is_err(),
            "working T has an unresolved owner"
        );
        projection.validate_published_source().unwrap();
        assert_eq!(projection.checkpoint_read_cut(), cut);
        assert_eq!(
            projection.checkpoint_seed().checkpoint_candidate().unwrap(),
            published.checkpoint_candidate().unwrap()
        );
        projection.publication_commit_scope(true, false);
        projection.publication_commit_scope(false, false);
        assert!(
            !projection.source_state.source_unavailable,
            "queued observations cannot invalidate the native source proof"
        );
        projection.validate_published_source().unwrap();
    }

    #[test]
    fn t19_unobserved_transcript_position_fences_a_later_transition_until_its_owner_installs() {
        use crate::durable::ConversationStore;
        let (mut projection, store) = initialized_source_projection();
        let started = append_source_attempt_start(&store, "t-prefix-start");
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(started.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(started.sequence, started.sequence)
            .unwrap();
        let earlier = store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new("held-terminal-t"),
                sequence: 0,
                conversation_id: store.conversation_id().clone(),
                attempt_id: Some(attempt()),
                turn_id: None,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                tool_occurrence: None,
                observation: None,
                event: RuntimeEvent::AttemptFailed {
                    attempt_id: attempt(),
                    error: crate::events::types::AttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "native terminal".into(),
                        },
                    },
                },
            })
            .unwrap();
        let later = store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("later-native-c"),
                content: Vec::new(),
            }))
            .unwrap();
        let cursor = projection.cursor;
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: later,
            });
        });
        store.mark_canonical_transcript_ready(1, 1).unwrap();
        assert_eq!(store.transcript_source_ready().unwrap().prefix, 0);
        assert_eq!(
            projection.cursor, cursor,
            "unreleased T=1 fences the later native C"
        );
        assert!(matches!(
            projection.validate_source_prefix(),
            Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
        ));
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(earlier.observation.as_deref().unwrap());
        });
        store
            .mark_observation_direct(earlier.sequence, earlier.sequence)
            .unwrap();
        store.mark_observation_transcript_ready(1).unwrap();
        let proof = store.transcript_source_ready().unwrap();
        assert_eq!(proof.prefix, 2);
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::TranscriptSourceReady(proof));
        });
        assert_eq!(
            projection.cursor.get(),
            cursor.get() + 1,
            "the exact complete source prefix publishes once"
        );
        projection.validate_source_prefix().unwrap();
        let source = projection.checkpoint_read_cut().source_cut.unwrap();
        assert_eq!(source, store.observation_read_cut().unwrap());
        assert_eq!(source.read.transcript, 2);
        assert_eq!(source.read.journal, 2);
        assert_eq!(source.ledger_through, 1);
    }

    #[test]
    fn t16_pending_adoption_receipt_releases_only_its_matching_mutation_fence() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let mut projection = projection();
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            projection.snapshot.conversation_id.clone(),
            &directory.path().join("history.sqlite"),
        )
        .unwrap();
        projection
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        let mut pending_first = self::projection();
        pending_first
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        let accepted = store
            .accept_inbound(crate::durable::inbox::InboundDraft {
                message_id: None,
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "native pending adoption".into(),
                    },
                )],
                timestamp: chrono::Utc::now(),
                correlation: None,
            })
            .unwrap();
        projection.apply_pending_receipt(accepted.pending_observation.as_deref());
        pending_first.apply_pending_receipt(accepted.pending_observation.as_deref());
        let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
        pending_first.apply_pending_receipt(adopted.pending_observation.as_deref());
        for receipt in &adopted.canonical_receipts {
            pending_first.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: receipt.clone(),
            });
        }
        assert!(
            pending_first.pending_epoch_wait.is_none(),
            "native mailbox publishes its pending receipt before Agent canonical callbacks; that order cannot restore an already represented mutation fence"
        );
        for receipt in &adopted.canonical_receipts {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt: receipt.clone(),
            });
        }
        assert_eq!(
            projection.pending_epoch_wait,
            Some(adopted.read_cut.mutation_revision)
        );
        projection.apply_pending_receipt(accepted.pending_observation.as_deref());
        assert_eq!(
            projection.pending_epoch_wait,
            Some(adopted.read_cut.mutation_revision),
            "an older pending receipt cannot release the adoption fence"
        );
        projection.apply_pending_receipt(adopted.pending_observation.as_deref());
        assert!(
            projection.pending_epoch_wait.is_none(),
            "the exact native adoption receipt releases its pending revision fence"
        );
        let native = adopted
            .adoption
            .as_ref()
            .unwrap()
            .observation
            .as_deref()
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(native);
        });
        publish_source_fact(&mut pending_first, |projection| {
            projection.apply_native_batch(native);
        });
        assert!(!matches!(
            projection.validate_source_prefix(),
            Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
        ));
    }

    #[test]
    fn t15_many_small_message_pages_advance_under_both_native_budgets() {
        let mut projection = projection();
        projection.snapshot.messages = (0..1000)
            .map(|index| {
                MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
                    id: MessageId::new(format!("message-{index}")),
                    content: vec![AssistantContentBlock::Text(
                        crate::message::content::TextBlock { text: "x".into() },
                    )],
                })
            })
            .collect();
        let checkpoint = projection.checkpoint_candidate().unwrap();
        assert!(checkpoint.snapshot.messages.len() <= 32);
        assert!(
            super::super::checkpoint::bounded_bytes(
                &checkpoint,
                super::super::checkpoint::CHECKPOINT_TARGET_BYTES
            )
            .is_some()
        );
        let mut offset = 0;
        let mut identities = Vec::new();
        loop {
            let page = projection
                .checkpoint_page(&checkpoint.read_cut, "messages", offset, 32)
                .unwrap();
            assert!(page.items.len() <= 32);
            assert!(
                super::super::checkpoint::bounded_bytes(
                    &page,
                    super::super::checkpoint::CHECKPOINT_TARGET_BYTES
                )
                .is_some()
            );
            identities.extend(page.items.iter().map(|item| item.identity.clone()));
            match page.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(identities.len(), 1000);
        assert_eq!(
            identities
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1000
        );
    }

    fn raw_publication_snapshot(
        frontier: crate::context::resources::LedgerFrontier,
    ) -> crate::model::RequestSnapshot {
        crate::model::RequestSnapshot::new(
            crate::model::RequestIdentity {
                attempt_id: attempt(),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            frontier,
            "system".into(),
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
            crate::model::invocation::ModelInvocationConfig {
                model: "scripted".into(),
                protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
                max_output_tokens: 100,
                request_params: crate::model::invocation::RequestParams::new(),
                capabilities: crate::model::ModelCapabilities::text_only(true, true),
                compat: crate::model::ModelCompat::default(),
            },
            4096,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            crate::context::assembly::ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            Vec::new(),
        )
    }

    fn raw_publication_store(
        conversation_id: ConversationId,
    ) -> (
        crate::durable::SqliteConversationStore,
        crate::publication::PublicationStreamStart,
    ) {
        use crate::durable::ConversationStore;
        let store = crate::durable::SqliteConversationStore::in_memory(conversation_id).unwrap();
        let snapshot = raw_publication_snapshot(crate::context::resources::LedgerFrontier(
            store.observation_read_cut().unwrap().ledger_through,
        ));
        store
            .commit_model_turn_start(&[], &snapshot, chrono::Utc::now())
            .unwrap();
        let start = crate::publication::PublicationStreamStart {
            stream_id: crate::runtime::identity::PublicationStreamId::for_request(
                &snapshot.identity.attempt_id,
                &snapshot.provisional_message_id,
            ),
            attempt_id: snapshot.identity.attempt_id.clone(),
            turn_id: snapshot.identity.turn.clone(),
            request_id: snapshot.request_id.clone(),
            message_id: snapshot.provisional_message_id.clone(),
        };
        store.open_publication_stream(&start).unwrap();
        (store, start)
    }

    fn assert_raw_publication_range(
        store: &dyn crate::durable::ConversationStore,
        reference: &crate::runtime_client::content::ContentRef,
        forged: &crate::runtime_client::content::ContentRef,
        raw: &str,
    ) {
        use base64::Engine as _;
        let chunk = store.content_read(reference, 0, 65_536).unwrap();
        assert_eq!(chunk.readable_end, reference.readable_end);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            raw.as_bytes()
        );
        let rejected = store
            .content_read(forged, 0, 65_536)
            .expect_err("a forged frontier must return no bytes");
        assert!(
            matches!(
                rejected,
                crate::durable::ConversationStoreError::InvalidReference(_)
            ),
            "exact native source failure: {rejected:?}"
        );
        assert_eq!(
            crate::durable::SqliteConversationStore::content_read_work(),
            (0, 0),
            "frontier authorization rejects before any body range read"
        );
    }

    #[test]
    fn t19_raw_assembled_arguments_and_observed_live_frontier_are_exact() {
        use crate::durable::ConversationStore;
        let mut projection = projection();
        let (store, start) = raw_publication_store(projection.snapshot.conversation_id.clone());
        let staged_frame = |sequence, payload| {
            let frame = PublicationFrame {
                stream_id: start.stream_id.clone(),
                message_id: start.message_id.clone(),
                sequence,
                payload,
            };
            store
                .stage_publication_frames(std::slice::from_ref(&frame))
                .unwrap();
            frame
        };
        projection.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt(),
            start: start.clone(),
        });
        let call = ToolCall {
            id: ToolCallId::new("raw-call"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".into(),
            arguments: serde_json::json!({"n":1000.0}),
        };
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: staged_frame(
                0,
                PublicationPayload::ProposedToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: ToolCallStart {
                        id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                    },
                },
            ),
        });
        let raw = " { \"n\": 1e3 } ";
        let raw = raw.to_owned();
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: staged_frame(
                1,
                PublicationPayload::ProposedToolCallArgumentsSuffix {
                    block_index: ContentBlockIndex::new(0),
                    call_id: call.id.clone(),
                    suffix: raw.clone(),
                },
            ),
        });
        let RuntimeClientEvent::BlockContentAdvanced { content_ref, .. } =
            projection.replay.back().unwrap().1.facts().last().unwrap()
        else {
            panic!("bounded native advancement")
        };
        let reference = content_ref.clone();
        projection.validate_live_content_ref(&reference).unwrap();
        let mut forged = reference.clone();
        forged.readable_end += 1;
        // Scope validation does not grant bytes beyond the original native frontier.
        projection.validate_live_content_ref(&forged).unwrap();
        assert_raw_publication_range(&store, &reference, &forged, &raw);
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: staged_frame(
                2,
                PublicationPayload::ProposedToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call,
                },
            ),
        });
        let RuntimeClientEvent::ToolCallAssembled { arguments_json, .. } =
            projection.replay.back().unwrap().1.facts().last().unwrap()
        else {
            panic!("assembly remains distinct")
        };
        assert_eq!(arguments_json, &raw);
    }

    /// T14/T20: Status carries its own native presentation, without examining
    /// unrelated immutable Job body metadata held by the native registry snapshot.
    #[test]
    fn t14_t20_status_background_never_clones_or_serializes_hidden_job_body() {
        let result = std::sync::Arc::new(crate::scripted_suites::support::fake::success_result(
            &"hidden result ".repeat(700_000),
        ));
        let status = crate::context::AgentStatus {
            generated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            sections: vec![crate::context::AgentStatusSection {
                id: crate::context::AgentStatusSectionId::new("background_execution"),
                data: crate::context::AgentStatusSectionData::BackgroundExecution {
                    executions: vec![crate::tools::background::BackgroundExecutionSnapshot {
                        bash: None,
                        execution_id: crate::runtime::identity::ToolExecutionId::new(
                            "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
                        ),
                        tool_id: std::sync::Arc::new(crate::runtime::identity::ToolId::new(
                            "private hidden identity ".repeat(100_000),
                        )),
                        tool_name: "visible native name".into(),
                        state: crate::tools::background::BackgroundLifecycle::PublishingTerminal,
                        progress: None,
                        result: Some(std::sync::Arc::clone(&result)),
                    }],
                    omitted_count: 73,
                },
            }],
        };
        let count = std::sync::Arc::strong_count(&result);
        let view = super::prepared_status(
            &attempt(),
            1,
            &MessageId::new("status-source"),
            &crate::context::ContributionOpportunities::default(),
            None,
            &status,
            None,
        );
        assert_eq!(std::sync::Arc::strong_count(&result), count);
        let encoded = serde_json::to_vec(&view).unwrap();
        assert!(encoded.len() < 4096);
        assert!(
            !String::from_utf8(encoded)
                .unwrap()
                .contains("private hidden")
        );
        let crate::runtime_client::snapshot::RuntimeClientStatusSection::BackgroundExecutions {
            executions,
            omitted_count,
        } = &view.sections[0]
        else {
            panic!("native Background section")
        };
        assert_eq!(*omitted_count, 73);
        assert_eq!(executions.len(), 1);
        assert_eq!(
            executions[0].state,
            crate::tools::background::BackgroundLifecycle::PublishingTerminal
        );
        assert!(view.rendered.contains("publishing_terminal"));
    }
}
