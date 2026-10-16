//! The runtime-owned semantic observation contract (Issue #61).
//!
//! This module owns the vocabulary and the handoff state through which the
//! conversation runtime publishes every semantically meaningful transition
//! of one conversation:
//!
//! ```text
//! ConversationRuntime semantic facts
//!         |
//!         v
//! ConversationObservation (this module, runtime-owned shapes only)
//!         |
//!         v
//! Runtime Client adapter translates
//!         |
//!         v
//! RuntimeClientProjection (snapshot / cursor / replay / subscribers)
//! ```
//!
//! Native pending/settled interactions are live process-owned observations;
//! canonical messages, publication frames and journal facts retain their existing
//! native authorities. This vocabulary carries finite source receipts and cheap
//! immutable source handles to the Runtime Client adapter.
//!
//! # Publication and worker ownership
//!
//! Source owners publish complete finite receipts through the installed CPU-only
//! sink. It captures a transition under the projection lock, encodes its frozen
//! candidate after unlocking, and installs its cursor before releasing the native
//! observation. Producers do no source reads or large serialization in that sink.
//!
//! [`PendingObservations`] is the worker rendezvous for indexed journal readiness,
//! finite pending-head replacement and disposable activity. Native J progress is
//! retained as scalar frontiers; the worker reads bounded pages from the existing
//! journal index. It does not retain every journal body in a FIFO. Native owner
//! receipts establish source cuts; acquiring a projection lock does not itself
//! drain or advance them.
//!
//! Disposable subagent activity and foreground tool progress use latest-value
//! slots keyed by native identities or exact numeric tool occurrences. Reliable
//! lifecycle publication retires the corresponding older disposable slot. These
//! observations never determine execution settlement.
//!
//! Ancillary Session activity and opening-preview publishers receive a finite
//! aggregate watch derived from canonical receipts. They hold no body queue or
//! independent history authority. Closing the rendezvous ends its worker; the
//! execution lifetime remains owned by the native runtime.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::durable::TranscriptCursor;
use crate::events::types::RuntimeEvent;
use crate::events::types::RuntimeEventEnvelope;
#[cfg(test)]
use crate::message::types::MessageBlock;
use crate::publication::{PublicationFrame, PublicationStreamStart};
use crate::runtime::identity::AttemptId;
use crate::runtime::identity::SubagentId;
use crate::runtime::identity::{ToolCallId, ToolId};
#[cfg(test)]
use crate::runtime::inbound::{InboundBatch, InboundItem};
use crate::runtime::interaction::{InteractionOutcome, InteractionRef};
use crate::runtime::subagent::SubagentSnapshot;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::ToolProgress;

/// One runtime-owned semantic observation.
///
/// The observation union carries every external state change the Runtime
/// Client projection folds. It is the single entry point of the projection:
/// no call site folds state directly. Every variant carries runtime-owned
/// source types only — the Runtime Client layer owns the translation into
/// its snapshot/event vocabulary.
#[derive(Debug, Clone)]
// The pending-interaction variant deliberately keeps its live request,
// audit envelope, and transcript cursor inline: the enum is constructed on
// rare interaction boundaries, and boxing an envelope would add an
// allocation to the hot observation path. The size spread is by design.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ConversationObservation {
    /// One complete in-memory native observation transition without a new Journal cut.
    ObservationBatch(Vec<ConversationObservation>),
    /// Exact native collection owner generation paired with its observation.
    CollectionProof {
        owner: &'static str,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
        observation: Box<ConversationObservation>,
    },
    /// One finite canonical terminal fact for a loaded Tool occurrence.
    NativeToolResultCommit(Box<crate::durable::observation::NativeToolResultCommit>),
    /// Exact occurrence from the same native Event append transaction.
    ToolOccurrenceProof {
        occurrence: crate::events::types::ToolObservationOccurrence,
        observation: Box<ConversationObservation>,
    },
    /// One ready semantic publication batch and its represented durable prefix.
    /// The queue cannot release this batch while any earlier Trace-affecting
    /// commit still awaits its owner's infallible native installation.
    JournalBatch {
        through: Option<u64>,
        observations: Vec<ConversationObservation>,
    },
    /// The owner has installed native state for this exact committed receipt.
    Published {
        journal_sequence: u64,
        observation: Box<ConversationObservation>,
    },
    /// A native Workflow cut published after its corresponding durable receipt.
    Workflow(Vec<crate::runtime_client::workflow_source::WorkflowReadSource>),
    /// Bounded authoritative durable replacement, ordered under the Goal
    /// publication boundary. Every Goal change is a durable phase/revision
    /// change; there is no activation-only Goal observation.
    GoalChanged(crate::runtime_client::goal::RuntimeClientGoalView),
    /// One finite witness from the native Journal publication-ready prefix.
    JournalFact(crate::durable::observation::ObservationEventWitness),
    /// Scalar native publication release; body remains solely in the Journal source.
    JournalSourceReady,
    /// Existing native T readiness; selected outside the Projection lock.
    TranscriptSourceReady(crate::durable::observation::TranscriptSourceReady),
    /// One canonical internal runtime fact of an attempt.
    #[cfg(test)]
    Event {
        /// The emitting attempt.
        attempt_id: AttemptId,
        /// The canonical fact.
        event: RuntimeEvent,
    },
    /// One context-compaction lifecycle fact not owned by an Agent attempt.
    ///
    /// Manual compaction is runtime maintenance: completion still names the
    /// atomic durable `RuntimeEvent::CompactionCompleted` fact, but its live
    /// publication is delayed until coordinator ownership is restored. Start
    /// and failure are also persisted before publication. Only compaction
    /// event variants are legal in this lane.
    #[cfg(test)]
    ManualCompactionEvent {
        /// Client correlation, retained only for this runtime incarnation.
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
        /// The compaction lifecycle fact.
        event: RuntimeEvent,
    },
    /// Finite native C evidence; canonical body bytes remain at their source.
    CanonicalCommitted {
        attempt_id: Option<AttemptId>,
        receipt: crate::durable::inbox::TranscriptCommitReceipt,
    },
    /// Finite audit settlement captured in the native publication transaction.
    AuditSettled {
        attempt_id: AttemptId,
        receipt: crate::durable::inbox::PublicationAuditReceipt,
    },
    /// One canonical message commit (the loop's commit observation seam;
    /// the internal committed-message events reference identity only).
    #[cfg(test)]
    Committed {
        /// The committing attempt, when one is active.
        attempt_id: Option<AttemptId>,
        /// The committed canonical message.
        block: MessageBlock,
        /// The durable transcript position of this message, when visible.
        transcript_cursor: Option<TranscriptCursor>,
    },
    /// One composed Agent Status observation.
    Status(crate::runtime_client::snapshot::AgentStatusView),
    /// One publication stream opened (Issue #108). The frozen identity pins
    /// the stream to the exact request generation that started it.
    PublicationOpened {
        /// The streaming attempt.
        attempt_id: AttemptId,
        /// The frozen publication identity.
        start: PublicationStreamStart,
    },
    /// One durably committed publication frame, observed at its release
    /// point. Nothing here reached a client before its own commit.
    Publication {
        /// The streaming attempt.
        attempt_id: AttemptId,
        /// The committed-for-release frame.
        frame: PublicationFrame,
    },
    /// Complete finite pending head from its native transaction.
    PendingReceipt(Arc<crate::durable::inbox::PendingObservationReceipt>),
    /// Test fixture for a native enqueue.
    #[cfg(test)]
    InboundEnqueued(InboundItem),
    /// Test fixture for native adoption.
    #[cfg(test)]
    InboundDrained(InboundBatch),
    /// Test fixture for a native pending head.
    #[cfg(test)]
    PendingInboundChanged(Vec<InboundItem>),
    /// One background registry transition snapshot.
    Background(BackgroundExecutionSnapshot),
    /// One live, not-yet-durable foreground tool progress report (Issue
    /// #178). Disposable: latest-value per tool call, coalesced in the
    /// queue, never durable, never model-facing. The canonical fact commits
    /// at batch settlement as `RuntimeEvent::ToolExecutionProgress`.
    ToolProgress {
        /// The owning attempt.
        #[allow(dead_code)]
        // identity carried for consumers; the in-crate folds key on the call
        attempt_id: AttemptId,
        /// The in-flight tool call.
        tool_call_id: ToolCallId,
        /// The executing tool.
        #[allow(dead_code)]
        // identity carried for consumers; the in-crate folds key on the call
        tool_id: ToolId,
        /// The latest bounded progress notification.
        progress: ToolProgress,
    },
    /// One complete registry owner transition, captured under one registry
    /// lock and released by one durable receipt. Agent lifecycle authority
    /// travels with the finite activation; clients cannot derive it from
    /// activation lifecycle. Reliable, ordered, and non-lossy.
    SubagentLifecycle {
        /// Durable Agent authority, absent for Workflow-owned finite activations.
        agent: Option<Box<crate::runtime::subagent::AgentSnapshot>>,
        snapshot: SubagentSnapshot,
    },
    /// One subagent live-activity snapshot (Issue #178). **Disposable**:
    /// latest-value, coalescing, keyed by subagent identity — a push
    /// overwrites the previous unpublished snapshot of the same subagent,
    /// so this lane is bounded by the number of active subagents and never
    /// consumes the queue capacity or ordering authority of the reliable
    /// lane. A `SubagentLifecycle` snapshot carries the newest observation
    /// projection of its subagent and evicts any queued activity snapshot
    /// for it.
    SubagentActivity(SubagentSnapshot),
    /// One activated authoritative capability snapshot, together with the
    /// authoritative per-source availability state at that commit (Issue
    /// #81). The availability may change without a revision swap (a
    /// diagnostic-only change); the snapshot never changes without one.
    ///
    /// A runtime configuration publication does **not** publish this variant: it
    /// commits a capability generation and a resource generation together
    /// and publishes both as one [`ConversationObservation::Resources`], so
    /// no consumer can fold half a generation.
    Capability {
        /// The activated immutable capability snapshot.
        source: crate::runtime_client::capability_source::CapabilityReadSource,
    },
    /// One published immutable runtime resource generation, **including the
    /// capability generation it was built against**.
    ///
    /// This is the complete result of one configuration publication and the only
    /// observation a capability closure commit emits. The capability half is deliberately not
    /// published separately: the two writes are ordered inside the runtime,
    /// but the consumer folding this queue runs on its own task under its
    /// own lock, so two observations would be two folds and a subscriber
    /// could observe the new capability beside the retired resource
    /// generation — a pairing that never existed. One observation makes the
    /// generation atomic for every consumer by construction rather than by
    /// timing.
    ///
    /// Both halves are still carried explicitly, because the resource
    /// revision and the capability revision move independently: a reload
    /// that only rewrites project instruction files advances the resource
    /// revision and leaves the capability revision exactly where it was.
    Resources {
        model: crate::model::session::SessionModelReadSource,
        approval_mode: crate::runtime::ApprovalMode,
        /// The published immutable resource generation. Its
        /// `capability()` is the capability snapshot committed by the same
        /// reload.
        snapshot: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        /// The authoritative availability of every evaluated optional
        /// capability source at that same commit.
        capability: crate::runtime_client::capability_source::CapabilityReadSource,
        display: crate::runtime_client::resource_source::ResourceReadSource,
    },

    /// One complete frozen admission fact, under the coordinator lock.
    AttemptAdmitted {
        read_image_active: bool,
        attempt_id: AttemptId,
        model: crate::model::session::AttemptModelReadSource,
        resource_revision: crate::runtime::identity::RuntimeResourceRevision,
        approval_mode: ApprovalMode,
    },
    /// The authoritative session model configuration changed.
    SessionModelChanged {
        /// The redacted session model state after the update.
        model: crate::model::session::SessionModelReadSource,
        capability: Option<crate::runtime_client::capability_source::CapabilityReadSource>,
    },
    /// Runtime drain began and new semantic admission closed.
    Shutdown,
    /// Finite head captured at the same native pending mutation boundary.
    PendingHead {
        owner: &'static str,
        cut: crate::runtime::identity::ObservationCollectionCut,
        head: Vec<crate::runtime_client::snapshot::RuntimeClientInteractionHeader>,
    },
    /// One native interaction became pending.
    InteractionPending {
        /// The root-facing routed request. The originating coordinator still
        /// owns the pending waiter and settlement.
        interaction: crate::runtime_client::snapshot::RuntimeClientInteractionHeader,
        /// The originating conversation's requested audit and cursor, when
        /// this is the primary conversation. Child audits stay child-local.
        audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
    },
    /// One native interaction reached its terminal rendezvous outcome.
    InteractionSettled {
        /// The full routed identity.
        interaction: InteractionRef,
        /// The terminal outcome delivered to its semantic owner.
        outcome: InteractionOutcome,
        /// The originating conversation's settled audit, when this is the
        /// primary conversation and its commit succeeded.
        audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
    },
    /// A child process died while owning one or more live interactions.
    ///
    /// This is a projection removal, not a synthetic interaction settlement:
    /// the originating coordinator died with the child and no terminal
    /// response is invented for the historical audit.
    #[cfg(test)]
    InteractionRemoved {
        /// The no-longer-actionable routed identity.
        interaction: InteractionRef,
    },
    /// The durable authority (Pending Inbound Inbox / Message Ledger) failed
    /// a storage operation the coordinator must not silently swallow
    /// (Issue #63, Finding 5). The failure consumes that stage's one bounded
    /// retry allowance for the current finite admission cycle and schedules a
    /// re-kick; the allowance remains consumed if the cycle advances to the
    /// other durable stage. Accepted pending work remains intact.
    DurableFailure {
        /// The human-readable storage failure description.
        #[allow(dead_code)]
        // surfaced by tests; the projection folds it without a client event
        message: String,
    },
    /// The durable authority failed persistently — a second failure of either
    /// transient admission stage in the same finite cycle, or an immediately
    /// non-transient durable
    /// failure (a semantic contract failure, an active attempt's
    /// canonical-write failure, or an exhausted background
    /// terminal-publication budget): the runtime has entered an explicit
    /// degraded state and will not pretend normal operation continues
    /// (Issue #63, Finding 5).
    DurabilityFailed {
        /// The operation that failed persistently.
        operation: String,
        /// The human-readable failure diagnostic.
        diagnostic: String,
    },
    /// The runtime's current configuration-adoption eligibility changed.
    ///
    /// Runtime-domain advisory state, never configuration application state:
    /// it answers whether this live runtime could adopt a prepared
    /// configuration now, derived by the adoption gate's idle rule over its
    /// in-memory owners. The runtime publishes it only when the derived value
    /// differs from its previous publication, so ordinary streaming never
    /// emits it. `session/adoptConfiguration` still revalidates the real gate
    /// at its commit boundary.
    AdoptionEligibility(crate::local_runtime::configuration::application::AdoptionEligibility),
}

/// Streaming traversal of the complete native transition. No payload or
/// collection-sized reference buffer is copied while unwrapping envelopes.
pub(crate) struct ObservationFacts<'a> {
    next: Option<&'a ConversationObservation>,
    parents: Vec<std::slice::Iter<'a, ConversationObservation>>,
}
impl ConversationObservation {
    fn source_sequence(&self) -> Option<u64> {
        match self {
            Self::Published {
                journal_sequence, ..
            } => Some(*journal_sequence),
            Self::CanonicalCommitted { receipt, .. } => {
                Some(receipt.observation.read_cut.read.journal)
            }
            Self::AuditSettled { receipt, .. } => Some(receipt.read_cut.read.journal),

            Self::CollectionProof { observation, .. }
            | Self::ToolOccurrenceProof { observation, .. } => observation.source_sequence(),
            _ => None,
        }
    }
    pub(crate) fn publish_journal_source(
        &self,
        store: &dyn crate::durable::ConversationStore,
        fanout: &ObservationFanout,
    ) {
        let result = match self {
            Self::Published {
                journal_sequence,
                observation,
            } => {
                observation.publish_journal_source(store, fanout);
                store
                    .event_transcript_cursor_at_sequence(*journal_sequence)
                    .and_then(|cursor| {
                        cursor.map_or(Ok(()), |cursor| {
                            store.mark_observation_transcript_ready(cursor.get())
                        })
                    })
                    .and_then(|()| {
                        store.mark_observation_ready(*journal_sequence, *journal_sequence)
                    })
                    .map(|()| *journal_sequence)
            }
            Self::CanonicalCommitted { receipt, .. } => store
                .mark_canonical_transcript_ready(
                    receipt.observation.first_position,
                    receipt.observation.last_position,
                )
                .map(|()| receipt.observation.read_cut.read.journal),
            Self::AuditSettled { receipt, .. } => store
                .mark_observation_transcript_ready(receipt.transcript_cursor.get())
                .map(|()| receipt.read_cut.read.journal),
            Self::InteractionPending {
                audit: Some((event, cursor)),
                ..
            }
            | Self::InteractionSettled {
                audit: Some((event, cursor)),
                ..
            } => store
                .mark_observation_transcript_ready(cursor.get())
                .and_then(|()| store.mark_observation_ready(event.sequence, event.sequence))
                .map(|()| event.sequence),
            #[cfg(test)]
            Self::InboundEnqueued(item) => item.transcript_cursor().map_or(Ok(0), |cursor| {
                store
                    .mark_observation_transcript_ready(cursor.get())
                    .map(|()| 0)
            }),
            Self::ObservationBatch(items)
            | Self::JournalBatch {
                observations: items,
                ..
            } => {
                for item in items {
                    item.publish_journal_source(store, fanout);
                }
                return;
            }
            Self::CollectionProof { observation, .. }
            | Self::ToolOccurrenceProof { observation, .. } => {
                observation.publish_journal_source(store, fanout);
                return;
            }
            _ => return,
        };
        match result {
            Ok(through) => fanout.source_ready(through),
            Err(_) => fanout.source_failed(),
        }
    }
    pub(crate) fn facts(&self) -> ObservationFacts<'_> {
        ObservationFacts {
            next: Some(self),
            parents: Vec::new(),
        }
    }
}
impl<'a> Iterator for ObservationFacts<'a> {
    type Item = &'a ConversationObservation;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let observation = match self.next.take() {
                Some(observation) => observation,
                None => loop {
                    let parent = self.parents.last_mut()?;
                    if let Some(observation) = parent.next() {
                        break observation;
                    }
                    self.parents.pop();
                },
            };
            match observation {
                ConversationObservation::ObservationBatch(observations)
                | ConversationObservation::JournalBatch { observations, .. } => {
                    self.parents.push(observations.iter());
                }
                ConversationObservation::Published { observation, .. }
                | ConversationObservation::CollectionProof { observation, .. }
                | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
                    self.next = Some(observation);
                }
                fact => return Some(fact),
            }
        }
    }
}

#[cfg(test)]
struct FixtureJournal {
    recorder: Arc<crate::scripted_suites::support::native_journal::NativeJournalRecorder>,
    cursor: Mutex<u64>,
}

/// The tiny synchronization boundary between the conversation runtime and
/// its observation consumers (the Runtime Client projection).
///
/// This type is the leaf of the lock graph: it owns one mutex over a small
/// state struct plus a `Notify` and calls nothing.
///
/// Two reliable/disposable delivery classes (Issue #178) live side by side
/// behind the one lock: the reliable FIFO of semantic/lifecycle
/// observations, plus two disposable latest-value lanes — subagent activity
/// snapshots keyed by subagent identity, and live (not-yet-durable)
/// foreground tool progress keyed by tool call. Each disposable lane is
/// bounded by the number of active publishers (subagents, in-flight tool
/// calls) — never by the number of updates — so disposable observation
/// traffic provably never consumes queue capacity, synchronization
/// authority, or terminal progress required by the reliable lane.
pub(crate) struct PendingObservations {
    /// The delivery lanes.
    state: Mutex<PendingState>,
    /// Wakes the worker task on every push and on close.
    notify: tokio::sync::Notify,
    journal: std::sync::OnceLock<std::sync::Weak<dyn crate::durable::ConversationStore>>,
    #[cfg(test)]
    fixture_journal: std::sync::OnceLock<FixtureJournal>,
    /// Set by [`close`](PendingObservations::close). Terminal: no further
    /// observation is accepted and the worker exits.
    closed: AtomicBool,
    /// Test-only worker-exit signal, so worker termination is observable
    /// deterministically instead of by timeout.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Test-only park switch. While set, [`drain`](PendingObservations::drain)
    /// yields nothing, so a test can step the queue itself instead of
    /// racing the projection worker. It is read and written only while the
    /// state lock is held, so a worker that has already entered `drain`
    /// either completed before the park or observes it.
    #[cfg(test)]
    parked: AtomicBool,
}

/// The one Runtime Client adapter at native source publication boundaries.
/// Canonical and Journal bodies stay at their existing source; ancillary
/// Session publishers read finite metadata slots in the fan-out below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeReceiptInstallation {
    /// All finite facts from this exact receipt are in working owner heads.
    /// This is neither a published cursor nor a client acknowledgement.
    Installed,
    /// The receipt did not install a complete observation source family.
    Unavailable,
}
impl NativeReceiptInstallation {
    pub(crate) fn is_installed(self) -> bool {
        self == Self::Installed
    }
}

pub(crate) trait PublicationProjectionCommit: Send + Sync {
    fn native_actor(
        &self,
        observation: &ConversationObservation,
    ) -> Option<NativeReceiptInstallation>;
    fn native_owner(
        &self,
        observation: &ConversationObservation,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> NativeReceiptInstallation;
    fn native_batch(
        &self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) -> NativeReceiptInstallation;
    fn native_manual_compaction(
        &self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> NativeReceiptInstallation;
    fn native_goal(
        &self,
        receipt: &crate::durable::inbox::GoalObservationReceipt,
    ) -> NativeReceiptInstallation;
    fn native_audit(
        &self,
        attempt: &AttemptId,
        receipt: &crate::durable::inbox::PublicationAuditReceipt,
    );
    fn native_pending(
        &self,
        receipt: Option<&crate::durable::inbox::PendingObservationReceipt>,
        cut: &crate::durable::reading::ConversationReadCut,
    );
    fn opened(&self, attempt: &AttemptId, start: &PublicationStreamStart);
    fn frame(
        &self,
        attempt: &AttemptId,
        frame: &PublicationFrame,
        receipt: &crate::publication::PublicationFrameReceipt,
    );
}

/// Finite source-owned Session metadata, folded at the native C publication.
/// The first ordinary User boundary and the maximum Human timestamp have
/// independent aggregation rules: replacing a later receipt cannot erase the
/// opening boundary, including a settled boundary with no renderable text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum OpeningUserPreview {
    #[default]
    Unobserved,
    Observed {
        preview: Option<String>,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionObservationMetadata {
    pub(crate) human_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) opening_preview: OpeningUserPreview,
    pub(crate) closed: bool,
}
impl SessionObservationMetadata {
    fn fold_canonical(&mut self, batch: &crate::durable::inbox::CanonicalObservationBatch) {
        self.fold(
            batch.human_activity_at,
            batch.has_opening_user,
            batch.opening_preview.as_deref(),
        );
    }
    fn fold(
        &mut self,
        activity: Option<chrono::DateTime<chrono::Utc>>,
        opening: bool,
        preview: Option<&str>,
    ) {
        if let Some(activity) = activity {
            self.human_activity_at = Some(
                self.human_activity_at
                    .map_or(activity, |previous| previous.max(activity)),
            );
        }
        if opening && self.opening_preview == OpeningUserPreview::Unobserved {
            self.opening_preview = OpeningUserPreview::Observed {
                preview: preview.map(str::to_owned),
            };
        }
    }
}

pub(crate) struct ObservationFanout {
    /// The primary queue owned by the Runtime Client projection.
    primary: Arc<PendingObservations>,
    /// Serializes publication and close so every consumer sees the same
    /// observation order and no subscriber is closed halfway through a
    /// publication.
    dispatch: Mutex<()>,
    session_metadata: tokio::sync::watch::Sender<SessionObservationMetadata>,

    publication_projection: std::sync::OnceLock<Arc<dyn PublicationProjectionCommit>>,
    child_activity: Mutex<(
        crate::runtime::subagent::activity::SubagentObservationProjector,
        tokio::sync::watch::Sender<crate::runtime::subagent::activity::SubagentObservation>,
    )>,
}

impl ObservationFanout {
    /// Creates a fan-out with the Runtime Client queue as its primary sink.
    pub(crate) fn new(primary: Arc<PendingObservations>) -> Self {
        Self {
            primary,
            dispatch: Mutex::new(()),
            session_metadata: tokio::sync::watch::channel(SessionObservationMetadata::default()).0,

            publication_projection: std::sync::OnceLock::new(),
            child_activity: Mutex::new((
                crate::runtime::subagent::activity::SubagentObservationProjector::default(),
                tokio::sync::watch::channel(
                    crate::runtime::subagent::activity::SubagentObservation::default(),
                )
                .0,
            )),
        }
    }

    pub(crate) fn session_metadata(
        &self,
    ) -> tokio::sync::watch::Receiver<SessionObservationMetadata> {
        self.session_metadata.subscribe()
    }

    fn canonical_metadata(&self, batch: &crate::durable::inbox::CanonicalObservationBatch) {
        self.session_metadata.send_if_modified(|metadata| {
            let previous = metadata.clone();
            metadata.fold_canonical(batch);
            *metadata != previous
        });
    }

    pub(crate) fn child_activity(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::runtime::subagent::activity::SubagentObservation> {
        self.child_activity
            .lock()
            .expect("native activity slot")
            .1
            .subscribe()
    }

    pub(crate) fn child_settled(&self) {
        let mut activity = self.child_activity.lock().expect("native activity slot");
        activity.0.native_settlement();
        let value = activity.0.observation().clone();
        activity.1.send_replace(value);
    }

    fn fold_child_activity(&self, observation: &ConversationObservation) {
        let mut activity = self.child_activity.lock().expect("native activity slot");
        for fact in observation.facts() {
            if activity.0.fold(fact, chrono::Utc::now()) {
                let value = activity.0.observation().clone();
                activity.1.send_replace(value);
            }
        }
    }

    pub(crate) fn install_publication_projection(
        &self,
        projection: Arc<dyn PublicationProjectionCommit>,
    ) {
        assert!(
            self.publication_projection.set(projection).is_ok(),
            "one native publication projection owner"
        );
    }

    /// Returns the primary Runtime Client queue.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn primary(&self) -> Arc<PendingObservations> {
        Arc::clone(&self.primary)
    }

    /// Publishes to the one Runtime Client owner and finite ancillary slots.
    pub(crate) fn push(
        &self,
        observation: &ConversationObservation,
    ) -> Option<NativeReceiptInstallation> {
        let dispatch = self
            .dispatch
            .lock()
            .expect("observation fan-out dispatch lock poisoned");
        for fact in observation.facts() {
            if let ConversationObservation::CanonicalCommitted { receipt, .. } = fact
                && receipt.ordinal.checked_add(1) == Some(receipt.observation.count)
            {
                self.canonical_metadata(&receipt.observation);
            }
        }
        self.fold_child_activity(observation);
        let installed = self
            .publication_projection
            .get()
            .and_then(|projection| projection.native_actor(observation));
        if installed.is_some() {
            self.primary.retire_lifecycle(observation);
        } else {
            self.primary.push(observation.clone());
        }
        drop(dispatch);
        installed
    }

    pub(crate) fn observe_tool_execution(
        &self,
        event: &RuntimeEvent,
        occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
    ) {
        {
            let mut activity = self.child_activity.lock().expect("native activity slot");
            if activity.0.native_event(event) {
                let value = activity.0.observation().clone();
                activity.1.send_replace(value);
            }
        }
        let apply = |queue: &PendingObservations| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => {
                if let Some(occurrence) = occurrence {
                    queue.tool_started(tool_call_id, occurrence);
                }
            }
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
            | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
                queue.tool_retired(tool_call_id, occurrence);
            }
            _ => {}
        };
        apply(&self.primary);
    }

    pub(crate) fn source_ready(&self, through: u64) {
        use crate::durable::presentation::JournalObserver;
        self.primary.ready(through);
    }
    pub(crate) fn source_failed(&self) {
        use crate::durable::presentation::JournalObserver;
        self.primary.committed(None);
    }

    pub(crate) fn native_pending(
        &self,
        receipt: Option<&Arc<crate::durable::inbox::PendingObservationReceipt>>,
        cut: &crate::durable::reading::ConversationReadCut,
    ) {
        if let Some(projection) = self.publication_projection.get() {
            projection.native_pending(receipt.map(Arc::as_ref), cut);
        } else if let Some(receipt) = receipt {
            self.primary
                .push(ConversationObservation::PendingReceipt(Arc::clone(receipt)));
        }
        if receipt.is_none() {
            self.source_failed();
        }
    }

    pub(crate) fn native_goal(
        &self,
        receipt: &crate::durable::inbox::GoalObservationReceipt,
    ) -> NativeReceiptInstallation {
        if let Some(projection) = self.publication_projection.get() {
            projection.native_goal(receipt)
        } else {
            NativeReceiptInstallation::Unavailable
        }
    }

    pub(crate) fn native_audit(
        &self,
        attempt: &AttemptId,
        receipt: &crate::durable::inbox::PublicationAuditReceipt,
    ) {
        if let Some(projection) = self.publication_projection.get() {
            projection.native_audit(attempt, receipt);
        }
    }

    /// Exact native Registry commit: owner header and its same-append source
    /// proof cross one display boundary, without a reliable handle queue.
    pub(crate) fn native_owner(
        &self,
        observation: &ConversationObservation,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        let _dispatch = self.dispatch.lock().expect("native owner dispatch");
        self.fold_child_activity(observation);
        if let Some(canonical) = batch.and_then(|batch| batch.canonical.as_ref()) {
            self.canonical_metadata(canonical);
        }
        let installed = self
            .publication_projection
            .get()
            .map_or(NativeReceiptInstallation::Unavailable, |projection| {
                projection.native_owner(observation, batch)
            });
        self.primary.retire_lifecycle(observation);
        installed
    }

    pub(crate) fn native_manual_compaction(
        &self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        let _dispatch = self.dispatch.lock().expect("manual compaction publication");
        if let Some(canonical) = batch.and_then(|batch| batch.canonical.as_deref()) {
            self.canonical_metadata(canonical);
        }
        if let Some(projection) = self.publication_projection.get() {
            projection.native_manual_compaction(request, batch)
        } else {
            NativeReceiptInstallation::Unavailable
        }
    }

    pub(crate) fn native_batch(
        &self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) -> NativeReceiptInstallation {
        if let Some(canonical) = &batch.canonical {
            self.canonical_metadata(canonical);
        }
        if let Some(projection) = self.publication_projection.get() {
            projection.native_batch(batch)
        } else {
            NativeReceiptInstallation::Unavailable
        }
    }

    pub(crate) fn publication_opened(&self, attempt: &AttemptId, start: &PublicationStreamStart) {
        if let Some(projection) = self.publication_projection.get() {
            projection.opened(attempt, start);
        }
    }
    pub(crate) fn publication_frame(
        &self,
        attempt: &AttemptId,
        frame: &PublicationFrame,
        receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        if let Some(projection) = self.publication_projection.get() {
            projection.frame(attempt, frame, receipt);
        }
    }

    /// Closes the primary queue and wakes finite ancillary metadata readers.
    pub(crate) fn close(&self) {
        let _dispatch = self
            .dispatch
            .lock()
            .expect("observation fan-out dispatch lock poisoned");
        self.session_metadata
            .send_modify(|metadata| metadata.closed = true);
        self.primary.close();
    }
}

/// The delivery lanes behind the one queue lock.
struct PendingState {
    // Client-only publication synchronization, not execution state. Each key
    // exists only from durable COMMIT to the owner's synchronous publication.
    unpublished: std::collections::BTreeSet<u64>,
    // Latest Trace-affecting receipt; ignored Journal kinds cannot advance it.
    committed_through: Option<u64>,
    journal_read: u64,
    transcript_read: Option<crate::durable::observation::TranscriptSourceReady>,
    journal_failed: bool,
    /// The reliable lane: ordered, non-lossy semantic/lifecycle
    /// observations.
    reliable: VecDeque<ConversationObservation>,
    pending_head: Option<Arc<crate::durable::inbox::PendingObservationReceipt>>,
    /// The disposable subagent-activity lane: the latest unpublished
    /// activity snapshot of each subagent that reported one, keyed by
    /// subagent identity.
    latest_activity: BTreeMap<SubagentId, ConversationObservation>,
    /// Latest metadata proof for activity outside the native loaded head.
    latest_offhead_activity: Option<ConversationObservation>,
    activity_cut: Option<crate::runtime::identity::ObservationCollectionCut>,
    /// The disposable live-tool-progress lane (Issue #178): the latest
    /// unpublished live progress report of each in-flight foreground tool
    /// call, keyed by tool call. Entries are whole
    /// [`ConversationObservation::ToolProgress`] values so the key never
    /// duplicates the payload's identity fields.
    latest_progress: BTreeMap<(u64, u32), ConversationObservation>,
    progress_owners: BTreeMap<usize, (ToolCallId, crate::events::types::ToolObservationOccurrence)>,
    #[cfg(test)]
    fixture_progress: BTreeMap<ToolCallId, ConversationObservation>,
    #[cfg(test)]
    fixture_progress_counts: (u64, u64),
}

fn retire_interaction_receipts(
    observation: &ConversationObservation,
    unpublished: &mut std::collections::BTreeSet<u64>,
) {
    match observation {
        ConversationObservation::InteractionPending {
            audit: Some((event, _)),
            ..
        }
        | ConversationObservation::InteractionSettled {
            audit: Some((event, _)),
            ..
        } => {
            unpublished.remove(&event.sequence);
        }
        ConversationObservation::ObservationBatch(observations) => {
            for observation in observations {
                retire_interaction_receipts(observation, unpublished);
            }
        }
        _ => {}
    }
}

impl PendingObservations {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(PendingState {
                unpublished: std::collections::BTreeSet::new(),
                committed_through: None,
                journal_read: 0,
                transcript_read: None,
                journal_failed: false,
                reliable: VecDeque::new(),
                pending_head: None,
                latest_activity: BTreeMap::new(),
                latest_offhead_activity: None,
                activity_cut: None,
                latest_progress: BTreeMap::new(),
                progress_owners: BTreeMap::new(),
                #[cfg(test)]
                fixture_progress: BTreeMap::new(),
                #[cfg(test)]
                fixture_progress_counts: (0, 0),
            }),
            notify: tokio::sync::Notify::new(),
            journal: std::sync::OnceLock::new(),
            #[cfg(test)]
            fixture_journal: std::sync::OnceLock::new(),
            closed: AtomicBool::new(false),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
            #[cfg(test)]
            parked: AtomicBool::new(false),
        }
    }

    pub(crate) fn bind_journal(
        &self,
        store: &Arc<dyn crate::durable::ConversationStore>,
        through: u64,
    ) {
        if self.journal.set(Arc::downgrade(store)).is_ok() {
            self.state
                .lock()
                .expect("native pending observations")
                .journal_read = through;
        }
    }

    pub(crate) fn retire_lifecycle(&self, observation: &ConversationObservation) {
        let mut state = self.state.lock().expect("native disposable activity owner");
        if let ConversationObservation::CollectionProof {
            owner: "agents",
            cut,
            ordinal,
            ..
        } = observation
        {
            if state
                .activity_cut
                .is_none_or(|previous| cut.generation >= previous.generation)
            {
                state.activity_cut = Some(*cut);
                state.latest_offhead_activity = None;
            }
            if let Some(position) =
                ordinal.and_then(|position| cut.total.checked_sub(position.saturating_add(1)))
            {
                state.latest_activity.retain(|_, previous| {
                    if let ConversationObservation::CollectionProof {
                        cut: previous_cut,
                        ordinal: Some(previous_ordinal),
                        ..
                    } = previous
                    {
                        previous_cut
                            .total
                            .checked_sub(previous_ordinal.saturating_add(1))
                            != Some(position)
                    } else {
                        true
                    }
                });
            }
        }
        for fact in observation.facts() {
            if let ConversationObservation::SubagentLifecycle { snapshot, .. } = fact {
                state.latest_activity.remove(&snapshot.subagent_id);
            }
        }
    }

    pub(crate) fn push(&self, observation: ConversationObservation) {
        if self.closed.load(Ordering::Acquire) {
            // A closed observation queue is terminal: never queue an
            // observation that nothing will ever fold.
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        if let ConversationObservation::PendingReceipt(receipt) = observation {
            state.pending_head = Some(receipt);
            drop(state);
            self.notify.notify_one();
            return;
        }
        let observation = if let ConversationObservation::Published {
            journal_sequence,
            observation,
        } = observation
        {
            state.unpublished.remove(&journal_sequence);
            if self.journal.get().is_some() {
                state.committed_through = Some(
                    state
                        .committed_through
                        .map_or(journal_sequence, |previous| previous.max(journal_sequence)),
                );
                if matches!(
                    observation.as_ref(),
                    ConversationObservation::JournalSourceReady
                ) {
                    drop(state);
                    self.notify.notify_one();
                    return;
                }
                ConversationObservation::Published {
                    journal_sequence,
                    observation,
                }
            } else {
                *observation
            }
        } else {
            observation
        };
        retire_interaction_receipts(&observation, &mut state.unpublished);
        for fact in observation.facts() {
            if let ConversationObservation::SubagentLifecycle { snapshot, .. } = fact {
                state.latest_activity.remove(&snapshot.subagent_id);
            }
        }
        if let ConversationObservation::CollectionProof {
            owner: "agents",
            cut,
            ordinal,
            observation: inner,
        } = &observation
        {
            if let ConversationObservation::SubagentActivity(snapshot) = inner.as_ref() {
                Self::coalesce_native_activity(
                    &mut state,
                    *cut,
                    *ordinal,
                    snapshot.subagent_id.clone(),
                    observation,
                );
                drop(state);
                self.notify.notify_one();
                return;
            }
            if let ConversationObservation::SubagentLifecycle { snapshot, .. } = inner.as_ref() {
                state.latest_activity.remove(&snapshot.subagent_id);
            }
        }
        Self::enqueue(&mut state, observation, self.journal.get().is_some());
        drop(state);
        self.notify.notify_one();
    }

    fn coalesce_native_activity(
        state: &mut PendingState,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
        identity: SubagentId,
        observation: ConversationObservation,
    ) {
        if state
            .activity_cut
            .is_some_and(|previous| cut.generation < previous.generation)
        {
            return;
        }
        state.activity_cut = Some(cut);
        state.latest_activity.retain(|_, observation| {
            if let ConversationObservation::CollectionProof {
                cut: previous,
                ordinal: Some(position),
                ..
            } = observation
            {
                cut.total
                    .checked_sub(previous.total)
                    .and_then(|added| position.checked_add(added))
                    .is_some_and(|position| position < 32)
            } else {
                false
            }
        });
        if let Some(position) = ordinal.filter(|position| *position < 32) {
            let native_position = cut.total.checked_sub(position.saturating_add(1));
            state.latest_activity.retain(|_, previous| {
                if let ConversationObservation::CollectionProof {
                    cut: previous_cut,
                    ordinal: Some(previous_ordinal),
                    ..
                } = previous
                {
                    previous_cut
                        .total
                        .checked_sub(previous_ordinal.saturating_add(1))
                        != native_position
                } else {
                    false
                }
            });
            state.latest_activity.insert(identity, observation);
        } else {
            state.latest_offhead_activity = Some(observation);
        }
    }

    fn enqueue(
        state: &mut PendingState,
        observation: ConversationObservation,
        journal_bound: bool,
    ) {
        #[cfg(not(test))]
        let _ = journal_bound;
        match observation {
            // Disposable: overwrite in place — the queue holds only the
            // latest unpublished activity snapshot per subagent.
            ConversationObservation::SubagentActivity(snapshot) => {
                state.latest_activity.insert(
                    snapshot.subagent_id.clone(),
                    ConversationObservation::SubagentActivity(snapshot),
                );
            }
            // Disposable: overwrite in place — the queue holds only the
            // latest unpublished live progress report per tool call.
            ConversationObservation::ToolProgress {
                ref tool_call_id, ..
            } => {
                #[cfg(test)]
                {
                    state.fixture_progress_counts.0 =
                        state.fixture_progress_counts.0.saturating_add(1);
                }
                if let Some(occurrence) = state
                    .progress_owners
                    .get(&tool_call_id.shared_source_token())
                    .map(|(_, occurrence)| occurrence.clone())
                {
                    #[cfg(test)]
                    {
                        state.fixture_progress_counts.1 =
                            state.fixture_progress_counts.1.saturating_add(1);
                    }
                    let key = (occurrence.ledger_position, occurrence.block_index.get());
                    state.latest_progress.insert(
                        key,
                        ConversationObservation::ToolOccurrenceProof {
                            occurrence,
                            observation: Box::new(observation),
                        },
                    );
                } else {
                    #[cfg(test)]
                    if !journal_bound {
                        state
                            .fixture_progress
                            .insert(tool_call_id.clone(), observation);
                    }
                }
            }
            // Reliable, and authoritative over activity: a lifecycle
            // snapshot carries the newest observation projection of its
            // subagent, so it evicts any queued activity snapshot of that
            // subagent. No consumer ever folds an activity snapshot older
            // than the lifecycle snapshot it already folded.
            ConversationObservation::SubagentLifecycle { agent, snapshot } => {
                state.latest_activity.remove(&snapshot.subagent_id);
                state
                    .reliable
                    .push_back(ConversationObservation::SubagentLifecycle { agent, snapshot });
            }
            // Reliable; a tool settlement fact retires the call's pending
            // live progress: a settled call leaves no stale live report
            // behind.
            #[cfg(test)]
            ConversationObservation::Event { ref event, .. }
                if matches!(
                    event,
                    RuntimeEvent::ToolExecutionCompleted { .. }
                        | RuntimeEvent::ToolExecutionFailed { .. }
                ) =>
            {
                let (RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. }) = event
                else {
                    unreachable!("the match guard admits exactly the two settlement facts")
                };
                if let Some((_, occurrence)) = state
                    .progress_owners
                    .remove(&tool_call_id.shared_source_token())
                {
                    state
                        .latest_progress
                        .remove(&(occurrence.ledger_position, occurrence.block_index.get()));
                }
                #[cfg(test)]
                state.fixture_progress.remove(tool_call_id);
                state.reliable.push_back(observation);
            }
            other => state.reliable.push_back(other),
        }
    }

    fn tool_started(
        &self,
        call_id: &ToolCallId,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) {
        // Only native loaded foreground ordinals need disposable progress.
        // Off-head tools retain their exact canonical/J read domains.
        if occurrence.tool_ordinal >= 32 {
            return;
        }
        let mut state = self.state.lock().expect("native tool progress source");
        let token = call_id.shared_source_token();
        if state.progress_owners.len() < 32 || state.progress_owners.contains_key(&token) {
            // The source Arc stays alive until retirement, so allocation reuse
            // cannot associate another execution with this occurrence.
            state
                .progress_owners
                .insert(token, (call_id.clone(), occurrence.clone()));
        }
    }
    fn tool_retired(
        &self,
        call_id: &ToolCallId,
        occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
    ) {
        let mut state = self.state.lock().expect("native tool progress source");
        if state
            .progress_owners
            .get(&call_id.shared_source_token())
            .is_some_and(|(_, current)| occurrence.is_none_or(|native| native == current))
            && let Some((_, current)) = state.progress_owners.remove(&call_id.shared_source_token())
        {
            state
                .latest_progress
                .remove(&(current.ledger_position, current.block_index.get()));
        }
    }

    /// Drains everything currently queued, in fold order: the reliable
    /// entries in push order first, then the disposable lanes — the latest
    /// live progress report of each in-flight tool call (in tool-call
    /// identity order), then the latest activity snapshot of each subagent
    /// (in subagent-identity order).
    ///
    /// This ordering is regression-free by construction: every queued
    /// activity entry is strictly newer than any queued lifecycle snapshot
    /// of the same subagent (a lifecycle push evicts it), and every queued
    /// live progress entry belongs to a tool call whose settlement fact is
    /// not queued (a settlement push evicts it).
    pub(crate) fn drain(&self) -> Vec<ConversationObservation> {
        if let Some(store) = self.journal.get().and_then(std::sync::Weak::upgrade) {
            return self.drain_journal(store.as_ref());
        }
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        #[cfg(test)]
        if self.parked.load(Ordering::Acquire) {
            // Parked under the state lock: whatever the projection worker
            // was about to fold, it folds nothing from here on.
            return Vec::new();
        }
        if !state.unpublished.is_empty() && !state.journal_failed {
            return Vec::new();
        }
        let mut drained: Vec<ConversationObservation> = state.reliable.drain(..).collect();
        if let Some(receipt) = state.pending_head.take() {
            drained.push(ConversationObservation::PendingReceipt(receipt));
        }
        drained.extend(std::mem::take(&mut state.latest_progress).into_values());
        #[cfg(test)]
        drained.extend(std::mem::take(&mut state.fixture_progress).into_values());
        drained.extend(std::mem::take(&mut state.latest_activity).into_values());
        drained.extend(state.latest_offhead_activity.take());
        if state.committed_through.is_some() || state.journal_failed {
            let committed = state.committed_through.take();
            let through = if std::mem::take(&mut state.journal_failed) {
                state.unpublished.clear();
                None
            } else {
                committed
            };
            vec![ConversationObservation::JournalBatch {
                through,
                observations: drained,
            }]
        } else if drained.is_empty() {
            Vec::new()
        } else {
            vec![ConversationObservation::ObservationBatch(drained)]
        }
    }

    fn journal_source_window(&self) -> Result<Option<(u64, u64)>, ()> {
        let state = self.state.lock().expect("native source cursor");
        #[cfg(test)]
        if self.parked.load(Ordering::Acquire) {
            return Ok(None);
        }
        if state.journal_failed {
            return Err(());
        }
        Ok(Some((
            state.journal_read,
            state.committed_through.unwrap_or(state.journal_read),
        )))
    }

    fn journal_witness_page(
        store: &dyn crate::durable::ConversationStore,
        after: u64,
        ready: u64,
    ) -> Option<Vec<crate::durable::observation::ObservationEventWitness>> {
        let end = ready.min(after.saturating_add(32));
        (after.saturating_add(1)..=end)
            .map(|sequence| {
                store
                    .observation_event_witness(sequence, ready)
                    .ok()
                    .flatten()
            })
            .collect()
    }

    fn drain_journal(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Vec<ConversationObservation> {
        let (after, through) = match self.journal_source_window() {
            Ok(Some(window)) => window,
            Ok(None) => return Vec::new(),
            Err(()) => {
                return vec![ConversationObservation::JournalBatch {
                    through: None,
                    observations: Vec::new(),
                }];
            }
        };
        let Ok(transcript) = store.transcript_source_ready() else {
            self.source_read_failed();
            return Vec::new();
        };
        let Ok(ready) = store.observation_ready_frontier(after, through) else {
            self.source_read_failed();
            return vec![ConversationObservation::JournalBatch {
                through: None,
                observations: Vec::new(),
            }];
        };
        let Some(facts) = Self::journal_witness_page(store, after, ready) else {
            self.source_read_failed();
            return Vec::new();
        };
        let mut state = self.state.lock().expect("native source cursor");
        if state.journal_read != after {
            return Vec::new();
        }
        let mut transitions = Vec::with_capacity(32);
        for witness in facts {
            let sequence = witness.sequence;
            while transitions.len() < 32
                && state
                    .reliable
                    .front()
                    .is_some_and(|item| item.source_sequence().is_none_or(|owned| owned < sequence))
            {
                transitions.push(
                    state
                        .reliable
                        .pop_front()
                        .expect("native independently owned transition"),
                );
            }
            if transitions.len() == 32 {
                break;
            }
            let mut observations = Vec::with_capacity(33);
            while observations.len() < 32
                && state
                    .reliable
                    .front()
                    .is_some_and(|item| item.source_sequence() == Some(sequence))
            {
                observations.push(state.reliable.pop_front().expect("native queued proof"));
            }
            observations.push(ConversationObservation::JournalFact(witness));
            state.journal_read = sequence;
            transitions.push(ConversationObservation::JournalBatch {
                through: Some(sequence),
                observations,
            });
        }
        if transitions.is_empty() && ready == after {
            while transitions.len() < 32
                && state
                    .reliable
                    .front()
                    .is_some_and(|item| item.source_sequence().is_none_or(|owned| owned <= after))
            {
                transitions.push(state.reliable.pop_front().expect("native finite proof"));
            }
        }
        Self::drain_current_sources(&mut state, through, &mut transitions);
        let transcript_changed = transcript.prefix > 0
            && (state.journal_read > after || state.transcript_read.as_ref() != Some(&transcript));
        let transcript_deferred = transcript_changed && transitions.len() >= 32;
        if transcript_changed && !transcript_deferred {
            state.transcript_read = Some(transcript.clone());
            transitions.push(ConversationObservation::TranscriptSourceReady(transcript));
        }
        let more = transcript_deferred
            || state.pending_head.is_some()
            || state.journal_read < ready
            || state.reliable.front().is_some_and(|item| {
                item.source_sequence()
                    .is_none_or(|owned| owned <= state.journal_read)
            })
            || (state.journal_read == through
                && (!state.latest_progress.is_empty()
                    || !state.latest_activity.is_empty()
                    || state.latest_offhead_activity.is_some()));
        drop(state);
        if more {
            self.notify.notify_one();
        }
        transitions
    }
    fn drain_current_sources(
        state: &mut PendingState,
        through: u64,
        transitions: &mut Vec<ConversationObservation>,
    ) {
        if transitions.len() < 32
            && let Some(receipt) = state.pending_head.take()
        {
            transitions.push(ConversationObservation::PendingReceipt(receipt));
        }
        if state.journal_read == through && state.reliable.is_empty() {
            if transitions.len() < 32
                && let Some((_, observation)) = state.latest_progress.pop_first()
            {
                transitions.push(observation);
            }
            if transitions.len() < 32
                && let Some((_, observation)) = state.latest_activity.pop_first()
            {
                transitions.push(observation);
            }
            if transitions.len() < 32
                && let Some(observation) = state.latest_offhead_activity.take()
            {
                transitions.push(observation);
            }
        }
    }

    fn source_read_failed(&self) {
        self.state
            .lock()
            .expect("native source read state")
            .journal_failed = true;
    }

    /// Waits for the next push or for close.
    ///
    /// `Notify::notify_one` stores one permit even with no waiter, so a
    /// push or a close between two waits is never missed.
    pub(crate) async fn wait(&self) {
        self.notify.notified().await;
    }

    pub(crate) fn has_unpublished(&self) -> bool {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        !state.unpublished.is_empty()
            || (self.journal.get().is_some()
                && state
                    .committed_through
                    .is_some_and(|through| state.journal_read < through))
    }

    pub(crate) fn can_publish_prepared_reads(&self) -> bool {
        #[cfg(test)]
        if self.parked.load(Ordering::Acquire) {
            return false;
        }
        !self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The terminal close, performed when either owner is destroyed.
    ///
    /// Idempotent: the second close is a no-op. No concurrent producer can
    /// exist after the last owner drops: every producer reaches this queue
    /// through a live owner handle.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.reliable.clear();
        state.unpublished.clear();
        state.committed_through = None;
        state.journal_failed = false;
        state.pending_head = None;
        state.latest_activity.clear();
        state.latest_offhead_activity = None;
        state.activity_cut = None;
        state.latest_progress.clear();
        state.progress_owners.clear();
        #[cfg(test)]
        state.fixture_progress.clear();
        drop(state);
        self.notify.notify_one();
    }

    /// Test-only: removes and returns the single oldest queued
    /// observation, so a test can stop between two enqueues and inspect the
    /// consumer's state at exactly that cut. The reliable lane's oldest
    /// entry wins; an empty reliable lane pops the disposable lanes in the
    /// documented drain order (live tool progress in tool-call order, then
    /// subagent activity in subagent-identity order, the latter wrapped as
    /// [`ConversationObservation::SubagentActivity`]).
    #[cfg(test)]
    pub(crate) fn pop_one(&self) -> Option<ConversationObservation> {
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        if !state.unpublished.is_empty() {
            return None;
        }
        if state.committed_through.is_some() || state.journal_failed {
            let through = state
                .committed_through
                .take()
                .filter(|_| !state.journal_failed);
            state.journal_failed = false;
            let mut observations: Vec<_> = state.reliable.drain(..).collect();
            if let Some(receipt) = state.pending_head.take() {
                observations.push(ConversationObservation::PendingReceipt(receipt));
            }
            observations.extend(std::mem::take(&mut state.latest_progress).into_values());
            observations.extend(std::mem::take(&mut state.fixture_progress).into_values());
            observations.extend(std::mem::take(&mut state.latest_activity).into_values());
            observations.extend(state.latest_offhead_activity.take());
            return Some(ConversationObservation::JournalBatch {
                through,
                observations,
            });
        }
        if let Some(receipt) = state.pending_head.take() {
            return Some(ConversationObservation::PendingReceipt(receipt));
        }
        if let Some(observation) = state.reliable.pop_front() {
            return Some(observation);
        }
        if let Some((_, observation)) = state.latest_progress.pop_first() {
            return Some(observation);
        }
        if let Some((_, observation)) = state.fixture_progress.pop_first() {
            return Some(observation);
        }
        state
            .latest_activity
            .pop_first()
            .map(|(_, observation)| observation)
            .or_else(|| state.latest_offhead_activity.take())
    }

    /// Test-only: stops the projection worker from folding anything, so a
    /// test owns the fold schedule and can inspect every cut of the
    /// observation stream deterministically.
    ///
    /// Parking takes the state lock, so it is ordered against every
    /// concurrent `drain`: a worker either drained before the park or
    /// drains nothing after it. [`pop_one`](PendingObservations::pop_one)
    /// and [`queued`](PendingObservations::queued) deliberately ignore the
    /// park — they are the test's own hands on the queue.
    #[cfg(test)]
    pub(crate) fn park(&self) {
        let _state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        self.parked.store(true, Ordering::Release);
    }

    /// Test-only: lifts the park and wakes the projection worker, so a
    /// backlog that accumulated (coalesced) while parked folds on the
    /// worker's next drain.
    #[cfg(test)]
    pub(crate) fn unpark(&self) {
        {
            let _state = self
                .state
                .lock()
                .expect("pending observation queue lock poisoned");
            self.parked.store(false, Ordering::Release);
        }
        self.notify.notify_one();
    }

    /// Test-only: whether nothing is left to fold now or once the journal
    /// commits — no queued observation in any lane, none awaiting its
    /// journal commit, and no committed cut not yet released as a batch.
    #[cfg(test)]
    pub(crate) fn settled(&self) -> bool {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.reliable.is_empty()
            && state.pending_head.is_none()
            && state.latest_activity.is_empty()
            && state.latest_offhead_activity.is_none()
            && state.latest_progress.is_empty()
            && state.fixture_progress.is_empty()
            && state.unpublished.is_empty()
            && state
                .committed_through
                .is_none_or(|through| state.journal_read >= through)
            && !state.journal_failed
    }

    /// Test-only: the number of observations waiting to be folded, across
    /// all delivery lanes.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.reliable.len()
            + state.latest_activity.len()
            + usize::from(state.latest_offhead_activity.is_some())
            + state.latest_progress.len()
            + state.fixture_progress.len()
    }

    /// Exact native lifecycle debt, excluding the independent latest Activity slot.
    #[cfg(test)]
    pub(crate) fn queued_lifecycle_records(&self) -> usize {
        self.state
            .lock()
            .expect("native lifecycle debt")
            .reliable
            .iter()
            .filter(|observation| {
                observation.facts().any(|fact| {
                    matches!(
                        fact,
                        ConversationObservation::SubagentLifecycle { .. }
                            | ConversationObservation::Background(_)
                    )
                })
            })
            .count()
    }

    /// Test-only: pending disposable subagent activity, excluding reliable
    /// lifecycle and configuration observations that must not be coalesced.
    #[cfg(test)]
    pub(crate) fn queued_activity(&self) -> usize {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.latest_activity.len() + usize::from(state.latest_offhead_activity.is_some())
    }

    /// Test-only: separate runtime adoption authority from the child activity
    /// backpressure lane. A child admission can publish Busy asynchronously.
    #[cfg(test)]
    pub(crate) fn queued_without_adoption(&self) -> usize {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state
            .reliable
            .iter()
            .filter(|entry| !matches!(entry, ConversationObservation::AdoptionEligibility(_)))
            .count()
            + state.latest_activity.len()
            + usize::from(state.latest_offhead_activity.is_some())
            + state.latest_progress.len()
    }

    /// Installs the test-only worker-exit signal.
    #[cfg(test)]
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        *self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned") = Some(sender);
    }

    /// Fires the test-only worker-exit signal, once.
    #[cfg(test)]
    pub(crate) fn signal_worker_exit(&self) {
        if let Some(sender) = self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned")
            .take()
        {
            let _ = sender.send(());
        }
    }
}

impl crate::durable::presentation::JournalObserver for PendingObservations {
    fn committed(&self, through: Option<u64>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        if let Some(through) = through {
            state.committed_through = Some(
                state
                    .committed_through
                    .map_or(through, |previous| previous.max(through)),
            );
        } else {
            state.journal_failed = true;
        }
        drop(state);
        self.notify.notify_one();
    }
    fn ready(&self, _through: u64) {
        self.state
            .lock()
            .expect("native source publication")
            .journal_failed = false;
        self.notify.notify_one();
    }
}
#[cfg(test)]
impl PendingObservations {
    pub(crate) fn source_progress_debug(&self) -> String {
        let state = self.state.lock().expect("fixture progress diagnostic");
        format!(
            "owners={} progress={} reliable={} journal_read={} through={:?} failed={} reports={} matches={} pending_head={} unpublished={} activity={} offhead={}",
            state.progress_owners.len(),
            state.latest_progress.len(),
            state.reliable.len(),
            state.journal_read,
            state.committed_through,
            state.journal_failed,
            state.fixture_progress_counts.0,
            state.fixture_progress_counts.1,
            state.pending_head.is_some(),
            state.unpublished.len(),
            state.latest_activity.len(),
            state.latest_offhead_activity.is_some(),
        )
    }

    pub(crate) fn bind_fixture_journal(
        &self,
        recorder: Arc<crate::scripted_suites::support::native_journal::NativeJournalRecorder>,
        through: u64,
    ) {
        assert!(
            self.fixture_journal
                .set(FixtureJournal {
                    recorder,
                    cursor: Mutex::new(through)
                })
                .is_ok()
        );
    }

    /// Consume one original fact for sequential assertion waits. The scalar
    /// cursor leaves later original rows unread; finite actor tails stay in the
    /// existing queue rather than a copied Journal/body cache.
    pub(crate) fn next_fixture_fact(&self) -> Option<ConversationObservation> {
        let fixture = self.fixture_journal.get().expect("fixture Journal bound");
        let recorder = &fixture.recorder;
        let mut cursor = fixture
            .cursor
            .lock()
            .expect("fixture original Journal cursor");
        {
            let mut state = self.state.lock().expect("fixture finite observations");
            state.journal_failed = false;
            state.committed_through = None;
            state.unpublished.clear();
        }
        for envelope in recorder.read_after(*cursor) {
            *cursor = envelope.sequence;
            if let Some(attempt_id) = envelope.attempt_id {
                if *cursor < recorder.admitted_through() {
                    self.notify.notify_one();
                }
                return Some(ConversationObservation::Event {
                    attempt_id,
                    event: envelope.event,
                });
            }
        }
        if *cursor < recorder.admitted_through() {
            self.notify.notify_one();
            return None;
        }
        drop(cursor);
        let mut state = self.state.lock().expect("fixture finite observations");
        while let Some(observation) = state.reliable.pop_front() {
            let mut facts = observation.facts().cloned();
            let Some(first) = facts.next() else { continue };
            let tail = facts.collect::<Vec<_>>();
            if !tail.is_empty() {
                state
                    .reliable
                    .push_front(ConversationObservation::ObservationBatch(tail));
            }
            return Some(first);
        }
        if let Some(receipt) = state.pending_head.take() {
            return Some(ConversationObservation::PendingReceipt(receipt));
        }
        state
            .latest_progress
            .pop_first()
            .map(|(_, fact)| fact)
            .or_else(|| state.fixture_progress.pop_first().map(|(_, fact)| fact))
            .or_else(|| state.latest_activity.pop_first().map(|(_, fact)| fact))
            .or_else(|| state.latest_offhead_activity.take())
    }

    /// Headless execution fixtures read original Journal facts after native
    /// receipt notice. No body is queued by a callback, and this is not a
    /// Runtime Client read, publication ACK, or production observation path.
    pub(crate) fn drain_fixture_facts(&self) -> Vec<ConversationObservation> {
        let fixture = self.fixture_journal.get().expect("fixture Journal bound");
        let recorder = &fixture.recorder;
        // This is the fixture reader's cursor, not a native owner/queue lock.
        // Serialize its finite read so concurrent assertion tasks cannot
        // consume the same original Journal page twice.
        let mut cursor = fixture
            .cursor
            .lock()
            .expect("fixture original Journal cursor");
        let events = recorder.read_after(*cursor);
        if let Some(last) = events.last() {
            *cursor = last.sequence;
            if last.sequence < recorder.admitted_through() {
                self.notify.notify_one();
            }
        }
        drop(cursor);
        let mut facts = events
            .into_iter()
            .filter_map(|envelope| {
                envelope
                    .attempt_id
                    .map(|attempt_id| ConversationObservation::Event {
                        attempt_id,
                        event: envelope.event,
                    })
            })
            .collect::<Vec<_>>();
        let queued = {
            let mut state = self.state.lock().expect("fixture finite observations");
            // Consume the unavailable-display notice used only to wake this
            // original-source fixture reader. It is never an installation ACK.
            state.journal_failed = false;
            state.committed_through = None;
            state.unpublished.clear();
            let mut queued = state.reliable.drain(..).collect::<Vec<_>>();
            queued.extend(
                state
                    .pending_head
                    .take()
                    .map(ConversationObservation::PendingReceipt),
            );
            queued.extend(std::mem::take(&mut state.latest_progress).into_values());
            queued.extend(std::mem::take(&mut state.fixture_progress).into_values());
            queued.extend(std::mem::take(&mut state.latest_activity).into_values());
            queued.extend(state.latest_offhead_activity.take());
            queued
        };
        for observation in queued {
            facts.extend(observation.facts().cloned());
        }
        facts
    }

    /// Synthetic queue fixture receipts; production receives only scalar native J notice.
    pub(crate) fn committed_fixture(&self, events: Option<Vec<RuntimeEventEnvelope>>) {
        use crate::durable::presentation::JournalObserver;
        let Some(events) = events else {
            self.committed(None);
            return;
        };
        for event in events {
            if trace_fact_requires_publication(&event.event) {
                self.state
                    .lock()
                    .unwrap()
                    .unpublished
                    .insert(event.sequence);
                self.committed(Some(event.sequence));
            }
        }
    }
}

/// Closed dependency list for Trace's durable facts. Other native facts are
/// deliberately excluded from Trace; they do not create a Trace-only cursor
/// transition. Their owners continue publishing ordinary observations.
pub(crate) fn trace_fact_requires_publication(event: &RuntimeEvent) -> bool {
    match event {
        RuntimeEvent::AttemptStarted { .. }
        | RuntimeEvent::AttemptCompleted { .. }
        | RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. }
        | RuntimeEvent::AttemptFailed { .. }
        | RuntimeEvent::TurnStarted
        | RuntimeEvent::TurnCompleted
        | RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ContextContributionEmitted { .. }
        | RuntimeEvent::ModelRequestCompleted { .. }
        | RuntimeEvent::ModelRequestFailed { .. }
        | RuntimeEvent::ModelRetryScheduled { .. }
        // Adoption is the canonical linearization point at which inbound
        // became a turn this conversation owes an answer for. Trace projects
        // it as the ledger's User record, so its receipt must be published
        // before a later owner may advance the represented prefix past it.
        | RuntimeEvent::InboundTurnAdopted { .. }
        | RuntimeEvent::AssistantMessageCommitted { .. }
        | RuntimeEvent::ToolExecutionStarted { .. }
        | RuntimeEvent::ToolExecutionCompleted { .. }
        | RuntimeEvent::ToolExecutionFailed { .. }
        | RuntimeEvent::ToolMessageCommitted { .. }
        | RuntimeEvent::CompactionStarted { .. }
        | RuntimeEvent::CompactionCompleted { .. }
        | RuntimeEvent::CompactionFailed { .. }
        | RuntimeEvent::BackgroundExecutionCommitted { .. }
        | RuntimeEvent::BackgroundTerminalPublished { .. }
        | RuntimeEvent::AgentActivationAdmission { .. }
        | RuntimeEvent::SubagentOwnershipCommitted { .. }
        | RuntimeEvent::SubagentTerminalPublished { .. }
        | RuntimeEvent::SubagentTerminalSettled { .. }
        | RuntimeEvent::SubagentPhysicalSettlementProven { .. }
        | RuntimeEvent::WorkflowStarted { .. }
        | RuntimeEvent::WorkflowCompleted { .. }
        | RuntimeEvent::WorkflowFailed { .. }
        | RuntimeEvent::WorkflowCancelled { .. }
        | RuntimeEvent::InteractionRequested { .. }
        | RuntimeEvent::InteractionSettled { .. }
        | RuntimeEvent::Goal { .. } => true,
        RuntimeEvent::NativeToolInvocation { .. }
        | RuntimeEvent::ToolExecutionProgress { .. }
        | RuntimeEvent::ToolExecutionDeadlineFired { .. }
        | RuntimeEvent::ToolExecutionCancellationRequested { .. }
        | RuntimeEvent::ToolExecutionSettlementObserved { .. }
        | RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        | RuntimeEvent::WorkflowBlockStarted { .. }
        | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
        | RuntimeEvent::WorkflowLoopIterationSettled { .. }
        | RuntimeEvent::WorkflowLoopExited { .. }
        | RuntimeEvent::WorkflowBlockSettled { .. }
        | RuntimeEvent::WorkflowNodeStarted { .. }
        | RuntimeEvent::WorkflowNodeSettled { .. }
        | RuntimeEvent::WorkflowAgentAdmitted { .. }
        | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
        | RuntimeEvent::WorkflowBranchSelected { .. }
        | RuntimeEvent::WorkflowParallelAdmitted { .. }
        | RuntimeEvent::WorkflowParallelSettled { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn t20_session_metadata_keeps_first_boundary_and_maximum_activity_without_fifo() {
        let first = chrono::DateTime::parse_from_rfc3339("2026-10-07T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut metadata = super::SessionObservationMetadata::default();
        metadata.fold(None, true, None);
        for ordinal in 0..100_000 {
            metadata.fold(
                Some(first + chrono::Duration::seconds(ordinal)),
                true,
                Some("later user"),
            );
        }
        assert_eq!(
            metadata.opening_preview,
            super::OpeningUserPreview::Observed { preview: None },
            "an unrenderable first User boundary is settled"
        );
        assert_eq!(
            metadata.human_activity_at,
            Some(first + chrono::Duration::seconds(99_999))
        );
        metadata.fold(Some(first), false, Some("ignored preview"));
        assert_eq!(
            metadata.human_activity_at,
            Some(first + chrono::Duration::seconds(99_999))
        );
        let mut text = super::SessionObservationMetadata::default();
        text.fold(None, true, Some("opening"));
        text.fold(None, true, Some("later"));
        assert_eq!(
            text.opening_preview,
            super::OpeningUserPreview::Observed {
                preview: Some("opening".into())
            }
        );
    }

    use super::*;
    use crate::runtime::identity::{AgentId, ConversationId, ToolCallId};
    use crate::runtime::subagent::{SubagentObservation, SubagentState};

    /// T20: the publication-ready J highwater owns no committed body buffers.
    /// The tiny readiness COMMIT is the source release linearization point.
    #[test]
    fn t20_native_journal_workset_retains_scalar_highwater_and_bounded_pages() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::runtime::identity::EventId;
        let conversation = ConversationId::new("conv_c8899c61-9925-71a2-b69d-b36bbca550c5");
        let store: Arc<dyn ConversationStore> =
            Arc::new(SqliteConversationStore::in_memory(conversation.clone()).unwrap());
        let queue = Arc::new(PendingObservations::new());
        queue.bind_journal(&store, 0);
        assert_eq!(store.observe_journal(queue.clone()).unwrap(), 0);
        queue.park();
        for ordinal in 0..73 {
            let started = store
                .append_event(RuntimeEventEnvelope {
                    schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                    tool_occurrence: None,
                    observation: None,
                    event_id: EventId::new(format!("started-{ordinal}")),
                    sequence: 0,
                    conversation_id: conversation.clone(),
                    attempt_id: Some(AttemptId::new(format!("attempt-{ordinal}"))),
                    turn_id: None,
                    timestamp: chrono::DateTime::UNIX_EPOCH,
                    event: RuntimeEvent::AttemptStarted {
                        attempt_id: AttemptId::new(format!("attempt-{ordinal}")),
                    },
                })
                .unwrap();
            store
                .mark_observation_ready(started.sequence, started.sequence)
                .unwrap();
            let event = store
                .append_event(RuntimeEventEnvelope {
                    schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                    tool_occurrence: None,
                    observation: None,
                    event_id: EventId::new(format!("source-{ordinal}")),
                    sequence: 0,
                    conversation_id: conversation.clone(),
                    attempt_id: Some(AttemptId::new(format!("attempt-{ordinal}"))),
                    turn_id: None,
                    timestamp: chrono::DateTime::UNIX_EPOCH,
                    event: RuntimeEvent::AttemptFailed {
                        attempt_id: AttemptId::new(format!("attempt-{ordinal}")),
                        error: crate::events::types::AttemptFailure::Runtime {
                            error: crate::runtime::types::RuntimeError::Internal {
                                message: "native diagnostic ".repeat(4096),
                            },
                        },
                    },
                })
                .unwrap();
            queue.push(ConversationObservation::Published {
                journal_sequence: event.sequence,
                observation: Box::new(ConversationObservation::JournalSourceReady),
            });
            assert_eq!(queue.queued(), 0, "committed body stays at native J source");
            store
                .mark_observation_ready(event.sequence, event.sequence)
                .unwrap();
        }
        assert!(
            queue.drain().is_empty(),
            "manual source reader owns this schedule"
        );
        queue.unpark();
        let mut sequences = Vec::new();
        for expected in [32, 32, 32, 32, 18] {
            let transitions = queue.drain();
            assert_eq!(transitions.len(), expected);
            for transition in transitions {
                let ConversationObservation::JournalBatch {
                    through: Some(sequence),
                    observations,
                } = transition
                else {
                    panic!("complete native J transition");
                };
                assert_eq!(observations.len(), 1);
                let ConversationObservation::JournalFact(witness) = &observations[0] else {
                    panic!("finite source witness");
                };
                assert_eq!(witness.sequence, sequence);
                assert!(crate::runtime_client::checkpoint::bounded_bytes(witness, 16384).is_some());
                sequences.push(sequence);
            }
        }
        assert_eq!(sequences, (1..=146).collect::<Vec<_>>());
        assert!(!queue.has_unpublished());
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn unpublished_earlier_commit_cannot_be_overtaken_by_a_later_owner() {
        use crate::runtime::identity::EventId;
        let queue = PendingObservations::new();
        let receipt = |sequence| RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: 1,
            event_id: EventId::new(format!("receipt-{sequence}")),
            sequence,
            conversation_id: ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
            attempt_id: Some(AttemptId::new("attempt")),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt"),
            },
        };
        queue.committed_fixture(Some(vec![receipt(1)]));
        queue.committed_fixture(Some(vec![receipt(2)]));
        let published = |sequence| ConversationObservation::Published {
            journal_sequence: sequence,
            observation: Box::new(ConversationObservation::Event {
                attempt_id: AttemptId::new("attempt"),
                event: receipt(sequence).event,
            }),
        };
        queue.push(published(2));
        assert!(queue.drain().is_empty());
        assert!(queue.pop_one().is_none());
        queue.push(published(1));
        let batch = queue.drain();
        assert!(
            matches!(batch.as_slice(), [ConversationObservation::JournalBatch { through: Some(2), observations }] if observations.len() == 2)
        );
        assert!(queue.drain().is_empty());
        // A fact excluded from Trace cannot independently publish a cursor.
        let mut ignored = receipt(3);
        // Tool execution progress is native execution state that Trace never
        // consumes, so it allocates no Trace-only cursor transition.
        ignored.event = RuntimeEvent::ToolExecutionProgress {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: crate::runtime::identity::ToolId::new("tool-1"),
            execution_id: None,
            progress: crate::tools::types::ToolProgress::default(),
        };
        queue.committed_fixture(Some(vec![ignored]));
        assert!(queue.drain().is_empty());
    }

    /// A minimal subagent snapshot carrying only the identity and the
    /// activity revision this suite distinguishes.
    fn subagent_snapshot(subagent_id: &str, revision: u64) -> SubagentSnapshot {
        SubagentSnapshot {
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            subagent_id: SubagentId::new(subagent_id),
            child_agent_id: AgentId::new("agent-child"),
            child_conversation_id: crate::scripted_suites::common::identity::child_conversation_id(
                subagent_id,
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-1"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            profile_digest: "sha256:p1".to_owned(),
            workspace: std::path::PathBuf::from("<shared>"),
            state: SubagentState::Running,
            cancel_reason: None,
            detail: None,
            observation: SubagentObservation {
                revision,
                ..SubagentObservation::default()
            },
            profile: None,
            settlement: crate::runtime::subagent::SubagentSettlement::default(),
            started_at: chrono::Utc::now(),
        }
    }

    /// One live tool progress report of `call`, carrying `message`.
    fn live_progress(call: &str, message: &str) -> ConversationObservation {
        ConversationObservation::ToolProgress {
            attempt_id: AttemptId::new("attempt-1"),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-bash"),
            progress: ToolProgress {
                message: Some(message.to_owned()),
                ..ToolProgress::default()
            },
        }
    }

    #[test]
    fn t19_t20_native_progress_uses_retained_original_source_without_scanning_provider_id() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
        use crate::runtime::identity::EventId;
        let conversation = ConversationId::new("conv_c8899c61-9925-71a2-b69d-b36bbca550c5");
        let store: Arc<dyn ConversationStore> =
            Arc::new(SqliteConversationStore::in_memory(conversation.clone()).unwrap());
        let attempt = AttemptId::new("attempt-original-source");
        let envelope = |event| RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: Some(attempt.clone()),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event,
        };
        store
            .append_event(envelope(RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            }))
            .unwrap();
        let call = ToolCallId::new("original-provider-id".repeat(65536));
        let message = crate::runtime::identity::MessageId::new("native-assistant");
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(AssistantMessageBlock {
                    id: message.clone(),
                    content: vec![AssistantContentBlock::ToolCall(
                        crate::tools::types::ToolCall {
                            id: call.clone(),
                            tool_id: ToolId::new("tool-echo"),
                            name: "echo".into(),
                            arguments: serde_json::json!({}),
                        },
                    )],
                }),
                envelope(RuntimeEvent::AssistantMessageCommitted {
                    message_id: message,
                }),
            )
            .unwrap();
        let started = store
            .append_event(envelope(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.clone(),
                tool_id: ToolId::new("tool-echo"),
            }))
            .unwrap();
        let RuntimeEvent::ToolExecutionStarted {
            tool_call_id: original,
            ..
        } = &started.event
        else {
            panic!("native start")
        };
        assert_eq!(
            original.shared_source_token(),
            call.shared_source_token(),
            "the native append returns the original shared execution source"
        );
        let occurrence = started.tool_occurrence.as_ref().unwrap();
        let queue = PendingObservations::new();
        queue.bind_journal(&store, started.sequence);
        queue.tool_started(original, occurrence);
        let progress = |id| ConversationObservation::ToolProgress {
            attempt_id: attempt.clone(),
            tool_call_id: id,
            tool_id: ToolId::new("tool-echo"),
            progress: ToolProgress {
                message: Some("native progress".into()),
                ..ToolProgress::default()
            },
        };
        queue.push(progress(original.clone()));
        assert_eq!(queue.state.lock().unwrap().latest_progress.len(), 1);
        let detached = ToolCallId::new(call.as_str());
        queue.push(progress(detached));
        assert_eq!(
            queue.state.lock().unwrap().latest_progress.len(),
            1,
            "separately allocated text cannot replace original-source progress"
        );
        assert!(queue.state.lock().unwrap().fixture_progress.is_empty());
        queue.tool_retired(original, Some(occurrence));
        assert!(queue.state.lock().unwrap().latest_progress.is_empty());
        assert!(queue.state.lock().unwrap().progress_owners.is_empty());
    }

    /// N activity pushes of one subagent leave exactly one queued entry,
    /// and the drain yields only the latest snapshot.
    #[test]
    fn activity_pushes_of_one_subagent_coalesce_to_the_latest() {
        let queue = PendingObservations::new();
        for revision in 1..=5 {
            queue.push(ConversationObservation::SubagentActivity(
                subagent_snapshot(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1",
                    revision,
                ),
            ));
        }
        assert_eq!(queue.queued(), 1, "the activity lane holds the latest only");
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 1);
        match &drained[0] {
            ConversationObservation::SubagentActivity(snapshot) => {
                assert_eq!(
                    snapshot.subagent_id,
                    SubagentId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1")
                );
                assert_eq!(snapshot.observation.revision, 5, "only the latest survives");
            }
            other => panic!("expected an activity observation, got {other:?}"),
        }
        assert_eq!(queue.queued(), 0);
    }

    /// Reliable updates can arrive after a stalled consumer's initial cut;
    /// they stay lossless while activity retains exactly its latest value.
    #[test]
    fn reliable_updates_do_not_change_the_activity_backlog_bound() {
        let queue = PendingObservations::new();
        let id = "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1";
        queue.park();
        queue.push(ConversationObservation::SubagentLifecycle {
            agent: None,
            snapshot: subagent_snapshot(id, 1),
        });
        let baseline = queue.queued();
        queue.push(ConversationObservation::SubagentLifecycle {
            agent: None,
            snapshot: subagent_snapshot(id, 2),
        });
        for revision in 3..=202 {
            queue.push(ConversationObservation::SubagentActivity(
                subagent_snapshot(id, revision),
            ));
        }
        assert!(
            queue.queued() > baseline + 1,
            "reliable updates also remain pending"
        );
        assert_eq!(queue.queued_activity(), 1);
        assert!(queue.drain().is_empty(), "the consumer remains parked");
        queue.unpark();
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert!(matches!(drained.as_slice(), [
            ConversationObservation::SubagentLifecycle { snapshot: first, .. },
            ConversationObservation::SubagentLifecycle { snapshot: second, .. },
            ConversationObservation::SubagentActivity(activity),
        ] if first.observation.revision == 1 && second.observation.revision == 2
            && activity.observation.revision == 202));
    }

    /// Native ordinal proofs, rather than the number of historical agents,
    /// bound the disposable lane while source preparation is parked.
    #[test]
    fn t20_native_activity_head_and_offhead_metadata_have_finite_workset() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let queue = PendingObservations::new();
        let store: Arc<dyn ConversationStore> = Arc::new(
            SqliteConversationStore::in_memory(ConversationId::new(
                "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b",
            ))
            .unwrap(),
        );
        queue.bind_journal(&store, 0);
        queue.park();
        for total in 1..=1000 {
            queue.push(ConversationObservation::CollectionProof {
                owner: "agents",
                cut: crate::runtime::identity::ObservationCollectionCut {
                    generation: total,
                    total,
                },
                ordinal: Some(0),
                observation: Box::new(ConversationObservation::SubagentActivity(
                    subagent_snapshot(&format!("conv-1-subagent-{total}"), total),
                )),
            });
            assert!(queue.queued_activity() <= 32);
            assert_eq!(queue.queued_lifecycle_records(), 0);
        }
        for generation in 1001..=2000 {
            queue.push(ConversationObservation::CollectionProof {
                owner: "agents",
                cut: crate::runtime::identity::ObservationCollectionCut {
                    generation,
                    total: 1000,
                },
                ordinal: Some(999),
                observation: Box::new(ConversationObservation::SubagentActivity(
                    subagent_snapshot("conv-1-subagent-1", generation),
                )),
            });
            assert_eq!(queue.queued_activity(), 33);
        }
        queue.unpark();
        let mut drained = Vec::new();
        loop {
            let batch = queue.drain();
            if batch.is_empty() {
                break;
            }
            assert!(batch.len() <= 32, "one finite physical drain");
            drained.extend(batch);
        }
        assert_eq!(drained.len(), 33);
        assert_eq!(
            drained
                .iter()
                .filter(|observation| matches!(
                    observation,
                    ConversationObservation::CollectionProof {
                        cut: crate::runtime::identity::ObservationCollectionCut {
                            generation: 2000,
                            total: 1000
                        },
                        ordinal: Some(999),
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(queue.drain().is_empty());
    }

    /// Activity entries of K subagents coexist: the disposable lane is
    /// bounded by the number of active subagents, not by the number of
    /// activity updates.
    #[test]
    fn activity_entries_coexist_per_subagent() {
        let queue = PendingObservations::new();
        for index in 1..=3 {
            let subagent_id = format!("conv-1-subagent-{index}");
            for revision in 1..=2 {
                queue.push(ConversationObservation::SubagentActivity(
                    subagent_snapshot(&subagent_id, revision),
                ));
            }
        }
        assert_eq!(queue.queued(), 3, "one entry per active subagent");
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 3);
        for (index, observation) in drained.iter().enumerate() {
            match observation {
                ConversationObservation::SubagentActivity(snapshot) => {
                    let expected = format!("conv-1-subagent-{}", index + 1);
                    assert_eq!(snapshot.subagent_id, SubagentId::new(&expected));
                    assert_eq!(snapshot.observation.revision, 2);
                }
                other => panic!("expected an activity observation, got {other:?}"),
            }
        }
    }

    /// A lifecycle push evicts the queued activity entry of its subagent,
    /// and a newer activity push then queues again: the drain folds the
    /// lifecycle snapshot (reliable, ordered) first and the newer activity
    /// after it — never a stale activity on top of a lifecycle transition.
    #[test]
    fn a_lifecycle_snapshot_evicts_the_queued_activity() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1", 3),
        ));
        queue.push(ConversationObservation::SubagentLifecycle {
            agent: None,
            snapshot: subagent_snapshot("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1", 4),
        });
        assert_eq!(
            queue.queued(),
            1,
            "the lifecycle push evicted the stale activity"
        );
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1", 5),
        ));
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 2);
        match (&drained[0], &drained[1]) {
            (
                ConversationObservation::SubagentLifecycle {
                    snapshot: lifecycle,
                    ..
                },
                ConversationObservation::SubagentActivity(activity),
            ) => {
                assert_eq!(lifecycle.observation.revision, 4);
                assert_eq!(activity.observation.revision, 5);
            }
            other => panic!("expected lifecycle then activity, got {other:?}"),
        }
    }

    /// Parked, pushed observations accumulate coalesced; the unpark hands
    /// the backlog to the consumer in one drain.
    #[test]
    fn a_parked_queue_coalesces_and_unpark_releases_the_backlog() {
        let queue = PendingObservations::new();
        queue.park();
        for revision in 1..=4 {
            queue.push(ConversationObservation::SubagentActivity(
                subagent_snapshot(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1",
                    revision,
                ),
            ));
        }
        queue.push(ConversationObservation::Shutdown);
        assert!(queue.drain().is_empty(), "parked: the worker folds nothing");
        assert_eq!(queue.queued(), 2, "the backlog accumulated coalesced");
        queue.unpark();
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], ConversationObservation::Shutdown));
        match &drained[1] {
            ConversationObservation::SubagentActivity(snapshot) => {
                assert_eq!(snapshot.observation.revision, 4);
            }
            other => panic!("expected the coalesced activity, got {other:?}"),
        }
    }

    /// Close clears both lanes and refuses further pushes.
    #[test]
    fn close_clears_both_lanes_terminally() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::Shutdown);
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1", 1),
        ));
        queue.close();
        assert_eq!(queue.queued(), 0);
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1", 2),
        ));
        assert_eq!(queue.queued(), 0, "a closed queue accepts nothing");
        assert!(queue.is_closed());
    }

    /// N live progress reports of one tool call coalesce to exactly one
    /// queued entry, and the drain yields only the latest report (after the
    /// reliable entries, in the documented fold order).
    #[test]
    fn live_progress_reports_of_one_call_coalesce_to_the_latest() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::Shutdown);
        queue.push(live_progress("call-1", "first"));
        queue.push(live_progress("call-1", "second"));
        queue.push(live_progress("call-1", "third"));
        assert_eq!(queue.queued(), 2, "the progress lane holds the latest only");
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], ConversationObservation::Shutdown));
        match &drained[1] {
            ConversationObservation::ToolProgress {
                tool_call_id,
                progress,
                ..
            } => {
                assert_eq!(*tool_call_id, ToolCallId::new("call-1"));
                assert_eq!(progress.message.as_deref(), Some("third"));
            }
            other => panic!("expected the coalesced live progress, got {other:?}"),
        }
    }

    /// A settled call leaves no stale live progress behind: the durable
    /// settlement fact (reliable) evicts the call's pending live entry.
    #[test]
    fn a_tool_settlement_fact_evicts_the_pending_live_progress() {
        let queue = PendingObservations::new();
        queue.push(live_progress("call-1", "halfway"));
        queue.push(ConversationObservation::Event {
            attempt_id: AttemptId::new("attempt-1"),
            event: RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                result: crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            },
        });
        assert_eq!(queue.queued(), 1, "the settlement evicted the live report");
        let batches = queue.drain();
        let [ConversationObservation::ObservationBatch(drained)] = batches.as_slice() else {
            panic!("one complete observation batch: {batches:?}");
        };
        assert_eq!(drained.len(), 1);
        assert!(
            matches!(
                drained[0],
                ConversationObservation::Event {
                    event: RuntimeEvent::ToolExecutionCompleted { .. },
                    ..
                }
            ),
            "only the reliable settlement fact folds"
        );
    }
}
