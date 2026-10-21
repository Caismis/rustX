//! The observation seam bridging authoritative runtime owners into the
//! shared leaf observation queue.

use super::{
    AgentExecutionObserver, AgentStatusObservation, Arc, AttemptId, BackgroundExecutionSnapshot,
    BackgroundObserver, CapabilityObserver, CapabilitySnapshot, ConversationObservation,
    InboundObserver, InteractionObserver, InteractionOutcome, InteractionRef, MessageBlock,
    PublicationAudit, PublicationFrame, PublicationStreamStart, RuntimeEvent, RuntimeInner, Weak,
};

/// The observation seam implementation bridging the authoritative runtime
/// owners into the shared leaf observation queue.
///
/// [`RuntimeObserver::push`] serves the mailbox, the background registry,
/// the capability coordinator, and `AgentExecution`, all of which fire
/// while holding (or being owned by) their authoritative boundary. The
/// observation is appended to the leaf queue and the projection worker is
/// woken — nothing else. These paths never acquire the coordinator lock or
/// the Runtime Client projection lock, and the runtime performs no fold of
/// its own.
///
/// # Lifetime
///
/// The observer is **non-owning**. Authoritative subsystems keep it alive
/// (`Arc<dyn InboundObserver>` and friends are unchanged), but it holds
/// only a `Weak<RuntimeInner>`, so the edge
/// `RuntimeInner -> subsystem -> Arc<RuntimeObserver> -> RuntimeInner` is
/// broken: installing an observation seam never extends a runtime's
/// lifetime.
///
/// Every callback upgrades the weak handle and returns without publishing
/// when the upgrade fails — the observation consumer simply no longer
/// exists. That is never an error for the subsystem: the mailbox, the
/// background registry, and the capability coordinator stay authoritative
/// whether or not a runtime observes them. The upgrade is transient and
/// confined to the callback, so an observer can neither resurrect nor
/// prolong a runtime.
pub(crate) struct RuntimeObserver {
    pub(super) inner: Weak<RuntimeInner>,
}

impl RuntimeObserver {
    pub(super) fn publish_native_batch(
        inner: &RuntimeInner,
        fanout: &crate::runtime::observation::ObservationFanout,
        receipt: &crate::durable::observation::NativeObservationBatch,
        journal_sequence: u64,
    ) {
        // No member releases its ready bit before the complete shared
        // family has installed one native transition.
        if receipt
            .journal_range
            .is_some_and(|(_, last)| last != journal_sequence)
        {
            return;
        }
        if !fanout.native_batch(receipt).is_installed() {
            fanout.source_failed();
            return;
        }
        let release = || -> Result<(), crate::durable::ConversationStoreError> {
            if let Some(canonical) = &receipt.canonical {
                inner.store.mark_canonical_transcript_ready(
                    canonical.first_position,
                    canonical.last_position,
                )?;
            }
            match &receipt.transition {
                crate::durable::observation::NativeObservationTransition::Single(witness)
                | crate::durable::observation::NativeObservationTransition::Adoption(witness)
                | crate::durable::observation::NativeObservationTransition::Compaction(witness) => {
                    if let Some(cursor) = witness.transcript_cursor {
                        inner
                            .store
                            .mark_observation_transcript_ready(cursor.get())?;
                    }
                }
                crate::durable::observation::NativeObservationTransition::WorkflowTerminal {
                    terminal,
                    output,
                } => {
                    for witness in [terminal, output] {
                        if let Some(cursor) = witness.transcript_cursor {
                            inner
                                .store
                                .mark_observation_transcript_ready(cursor.get())?;
                        }
                    }
                }
                _ => {}
            }
            if let Some((first, last)) = receipt.journal_range {
                inner.store.mark_observation_direct(first, last)?;
                fanout.source_ready(last);
            }
            Ok(())
        };
        if release().is_err() {
            fanout.source_failed();
        }
    }

    /// Creates the non-owning observer of one runtime.
    ///
    /// Installed by [`RuntimeInner::install_observation_bridge`] on behalf
    /// of the Runtime Client adapter (or a headless observation consumer);
    /// a conversation runtime with zero attachments has no subsystem
    /// observation seams and runs the identical admission/execution path.
    pub(crate) fn new(inner: &Arc<RuntimeInner>) -> Self {
        Self {
            inner: Arc::downgrade(inner),
        }
    }

    /// Source and owner receipts already exist at this callback boundary.
    /// Nothing is re-read or retained as a growing lifecycle handle queue.
    pub(super) fn publish_owner(
        &self,
        observation: &ConversationObservation,
        committed: &crate::events::types::RuntimeEventEnvelope,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Some(fanout) = inner.pending.get() else {
            return;
        };
        let batch = committed.observation.as_deref();
        if !fanout.native_owner(observation, batch).is_installed() {
            fanout.source_failed();
            return;
        }
        let Some(batch) = batch else {
            fanout.source_failed();
            return;
        };
        if let Some(canonical) = &batch.canonical
            && inner
                .store
                .mark_canonical_transcript_ready(canonical.first_position, canonical.last_position)
                .is_err()
        {
            fanout.source_failed();
            return;
        }
        if let crate::durable::observation::NativeObservationTransition::Single(witness) =
            &batch.transition
            && let Some(cursor) = witness.transcript_cursor
            && inner
                .store
                .mark_observation_transcript_ready(cursor.get())
                .is_err()
        {
            fanout.source_failed();
            return;
        }
        if let Some((first, last)) = batch.journal_range {
            match inner.store.mark_observation_direct(first, last) {
                Ok(()) => fanout.source_ready(last),
                Err(_) => fanout.source_failed(),
            }
        }
    }

    /// Publishes an immutable owner receipt through the existing native sink.
    /// Disposable activity keeps its finite latest-value lane; exact lifecycle
    /// and capability receipts publish synchronously without owner re-entry.
    pub(crate) fn push(&self, observation: ConversationObservation) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.observe(observation);
    }
}

impl crate::goal::GoalObserver for RuntimeObserver {
    fn changed(&self, receipt: Option<&Arc<crate::durable::inbox::GoalObservationReceipt>>) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Some(fanout) = inner.pending.get() else {
            return;
        };
        let Some(receipt) = receipt else {
            fanout.source_failed();
            return;
        };
        if !fanout.native_goal(receipt).is_installed() {
            fanout.source_failed();
            return;
        }
        let Some(native) = &receipt.native else {
            fanout.source_failed();
            return;
        };
        if let Some(header) = receipt
            .pending
            .as_ref()
            .and_then(|pending| pending.selected.as_ref())
            && let Some(cursor) = header.message.transcript_cursor
            && inner
                .store
                .mark_observation_transcript_ready(cursor.get())
                .is_err()
        {
            fanout.source_failed();
            return;
        }
        if let Some((first, last)) = native.journal_range {
            if inner.store.mark_observation_direct(first, last).is_ok() {
                fanout.source_ready(last);
            } else {
                fanout.source_failed();
            }
        }
    }
}

impl AgentExecutionObserver for RuntimeObserver {
    fn observe_event(
        &self,
        _attempt_id: &AttemptId,
        event: &RuntimeEvent,
        journal_sequence: u64,
        tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        receipt: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.observe_tool_execution(event, tool_occurrence);
            if let Some(receipt) = receipt {
                Self::publish_native_batch(&inner, fanout, receipt, journal_sequence);
                return;
            }
        }
        self.push(ConversationObservation::Published {
            journal_sequence,
            observation: Box::new(ConversationObservation::JournalSourceReady),
        });
    }

    fn observe_committed(
        &self,
        attempt_id: &AttemptId,
        _block: &MessageBlock,
        receipt: &crate::durable::TranscriptCommitReceipt,
    ) {
        self.push(ConversationObservation::CanonicalCommitted {
            attempt_id: Some(attempt_id.clone()),
            receipt: receipt.clone(),
        });
    }

    fn observe_status(
        &self,
        observation: &AgentStatusObservation,
        source: Option<&crate::durable::StatusObservationSource>,
    ) {
        self.push(ConversationObservation::Status(
            crate::observation::status::prepared_status_view(observation, source),
        ));
    }

    fn observe_contribution_status(
        &self,
        contribution: &crate::context::contribution::ContributionStart,
        _attempt: &AttemptId,
        _turn: u32,
        source: Option<&crate::durable::StatusObservationSource>,
    ) {
        if contribution.presentation.is_none() {
            return;
        }
        // The closed RequestStart receipt publishes this prepared status at
        // its exact C/J cut. Missing receipt evidence fences only observation.
        if source.is_none()
            && let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.source_failed();
        }
    }

    fn observe_publication_opened(&self, attempt_id: &AttemptId, start: &PublicationStreamStart) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.publication_opened(attempt_id, start);
        }
    }

    fn observe_publication(
        &self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
        receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.publication_frame(attempt_id, frame, receipt);
        }
    }

    fn observe_publication_settled(
        &self,
        attempt_id: &AttemptId,
        _audit: &PublicationAudit,
        receipt: &crate::durable::PublicationAuditReceipt,
    ) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.native_audit(attempt_id, receipt);
            // Physical source readiness follows CPU-only display publication.
            // It is not a client ACK, nor release of unrelated Journal rows.
            if inner
                .store
                .mark_observation_transcript_ready(receipt.transcript_cursor.get())
                .is_err()
            {
                fanout.source_failed();
            } else {
                fanout.source_ready(receipt.read_cut.read.journal);
            }
        }
    }

    // The loop fires this while the tool still executes; a leaf push into
    // the queue's disposable live-progress lane, exactly like the other
    // callbacks — no coordinator or projection lock is ever taken here.
    fn observe_tool_progress(
        &self,
        attempt_id: &AttemptId,
        tool_call_id: &crate::runtime::identity::ToolCallId,
        tool_id: &crate::runtime::identity::ToolId,
        progress: &crate::tools::types::ToolProgress,
    ) {
        self.push(ConversationObservation::ToolProgress {
            attempt_id: attempt_id.clone(),
            tool_call_id: tool_call_id.clone(),
            tool_id: tool_id.clone(),
            progress: crate::tools::types::ToolProgress {
                message: progress.message.as_deref().map(|message| {
                    let mut end = message
                        .len()
                        .min(crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES);
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    message[..end].to_owned()
                }),
                completed: progress.completed,
                total: progress.total,
            },
        });
    }
}

// The mailbox fires `on_enqueued`/`on_drained` while the mailbox lock is
// held, and `admit_next_attempt` drains the mailbox under the coordinator
// lock: taking the coordinator lock here would close the cycle. Enqueue
// into the leaf queue only; admission wakeup is the mailbox's own shared
// wake handle (see [`ConversationInboundMailbox::wake`]), which fires at
// the same publication linearization point whether or not any observer is
// installed.
impl InboundObserver for RuntimeObserver {
    fn on_pending(
        &self,
        receipt: Option<&Arc<crate::durable::inbox::PendingObservationReceipt>>,
        cut: &crate::durable::reading::ConversationReadCut,
    ) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(fanout) = inner.pending.get()
        {
            fanout.native_pending(receipt, cut);
            // Removal owns the retired T position too. Marking that exact
            // position wakes source preparation even though its row is gone.
            if let Some(receipt) = receipt
                && let Some(cursor) = receipt
                    .selected
                    .as_ref()
                    .and_then(|header| header.message.transcript_cursor)
                    .or(receipt.removed_transcript)
                && inner
                    .store
                    .mark_observation_transcript_ready(cursor.get())
                    .is_err()
            {
                fanout.source_failed();
            }
        }
    }
}

// Registry -> fan-out dispatch -> native publication gate -> raw Projection.
// Immutable headers and same-append receipts need no Registry re-entry, Store
// reads, socket readiness or await inside the native display boundary.
impl BackgroundObserver for RuntimeObserver {
    fn on_committed(
        &self,
        snapshot: &BackgroundExecutionSnapshot,
        committed: &crate::events::types::RuntimeEventEnvelope,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.publish_owner(
            &ConversationObservation::CollectionProof {
                owner: "jobs",
                cut,
                ordinal,
                observation: Box::new(ConversationObservation::Background(snapshot.clone())),
            },
            committed,
        );
    }
    fn on_snapshot(
        &self,
        snapshot: &BackgroundExecutionSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.push(ConversationObservation::CollectionProof {
            owner: "jobs",
            cut,
            ordinal,
            observation: Box::new(ConversationObservation::Background(snapshot.clone())),
        });
    }
}

// The same native owner lock order applies to Child lifecycle receipts.
// Lifecycle facts use the synchronous owner sink; disposable Activity keeps
// its existing finite coalescing lane and cannot retire execution authority.
impl crate::runtime::subagent::SubagentObserver for RuntimeObserver {
    fn on_committed(
        &self,
        agent: Option<&crate::runtime::subagent::AgentSnapshot>,
        snapshot: &crate::runtime::subagent::SubagentSnapshot,
        committed: &crate::events::types::RuntimeEventEnvelope,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.publish_owner(
            &ConversationObservation::CollectionProof {
                owner: "agents",
                cut,
                ordinal,
                observation: Box::new(ConversationObservation::SubagentLifecycle {
                    agent: agent.cloned().map(Box::new),
                    snapshot: snapshot.clone(),
                }),
            },
            committed,
        );
    }
    fn on_snapshot(
        &self,
        agent: Option<&crate::runtime::subagent::AgentSnapshot>,
        snapshot: &crate::runtime::subagent::SubagentSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.push(ConversationObservation::CollectionProof {
            owner: "agents",
            cut,
            ordinal,
            observation: Box::new(ConversationObservation::SubagentLifecycle {
                agent: agent.cloned().map(Box::new),
                snapshot: snapshot.clone(),
            }),
        });
    }

    fn on_activity(
        &self,
        snapshot: &crate::runtime::subagent::SubagentSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.push(ConversationObservation::CollectionProof {
            owner: "agents",
            cut,
            ordinal,
            observation: Box::new(ConversationObservation::SubagentActivity(snapshot.clone())),
        });
    }

    fn on_interaction_pending(
        &self,
        interaction: &crate::observation::interaction::RuntimeClientInteractionHeader,
        proof: &crate::runtime::subagent::ChildPendingHeadProof,
    ) {
        self.push(ConversationObservation::ObservationBatch(vec![
            ConversationObservation::InteractionPending {
                interaction: interaction.clone(),
                audit: None,
            },
            child_pending_head_observation(proof),
        ]));
    }
    fn on_interaction_settled(
        &self,
        interaction: &InteractionRef,
        outcome: &InteractionOutcome,
        proof: &crate::runtime::subagent::ChildPendingHeadProof,
    ) {
        self.push(ConversationObservation::ObservationBatch(vec![
            ConversationObservation::InteractionSettled {
                interaction: interaction.clone(),
                outcome: outcome.clone(),
                audit: None,
            },
            child_pending_head_observation(proof),
        ]));
    }
    fn on_interaction_head(&self, proof: &crate::runtime::subagent::ChildPendingHeadProof) {
        self.push(child_pending_head_observation(proof));
    }
}

// The coordinator fires `on_snapshot` while the capability state lock is
// held, with an attempt commit blocked behind it. Push only, so an
// authoritative capability commit never waits on the coordinator lock or
// the Runtime Client projection lock. The observation carries the
// authoritative `CapabilitySnapshot`; the Runtime Client projection owns
// the translation into its capability view.
impl CapabilityObserver for RuntimeObserver {
    fn on_snapshot(
        &self,
        snapshot: &Arc<CapabilitySnapshot>,
        availability: &crate::capabilities::CapabilityAvailability,
    ) {
        self.push(ConversationObservation::Capability {
            source: crate::observation::capability_source::CapabilityReadSource::new(
                Arc::clone(snapshot),
                availability.clone(),
                None,
            ),
        });
    }
}

pub(super) fn child_pending_head_observation(
    proof: &crate::runtime::subagent::ChildPendingHeadProof,
) -> ConversationObservation {
    ConversationObservation::PendingHead {
        owner: "pending_children",
        cut: proof.cut,
        head: proof.head.clone(),
    }
}

pub(super) fn pending_head_observation(
    proof: &crate::runtime::interaction::PendingHeadProof,
) -> ConversationObservation {
    let head = proof
        .head
        .iter()
        .map(|(sequence, source)| {
            let mut header = source.header().clone();
            header.source_offset = sequence.checked_mul(2).expect("pending source offset");
            header
        })
        .collect();
    ConversationObservation::PendingHead {
        owner: "pending_primary",
        cut: proof.cut,
        head,
    }
}

// Interaction pending publication fires while the coordinator's pending-state
// lock is held; terminal publication fires only after the waiter releases its
// callback authority and while the coordinator's counted settlement guard is
// held. Both callbacks are leaves: push only into the queue, never into the
// Runtime Client projection lock or conversation coordinator lock.
impl InteractionObserver for RuntimeObserver {
    fn on_pending(
        &self,
        source: &Arc<crate::runtime::interaction::source::PendingInteractionSource>,
        audit: &crate::events::types::RuntimeEventEnvelope,
        transcript_cursor: crate::durable::TranscriptCursor,
        proof: &crate::runtime::interaction::PendingHeadProof,
    ) {
        self.push(ConversationObservation::ObservationBatch(vec![
            ConversationObservation::InteractionPending {
                interaction: source.header().clone(),
                audit: Some((audit.clone(), transcript_cursor)),
            },
            pending_head_observation(proof),
        ]));
    }

    fn on_settled(
        &self,
        interaction_id: &crate::runtime::identity::InteractionId,
        outcome: &InteractionOutcome,
        audit: Option<&(
            crate::events::types::RuntimeEventEnvelope,
            crate::durable::TranscriptCursor,
        )>,
        proof: &crate::runtime::interaction::PendingHeadProof,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        self.push(ConversationObservation::ObservationBatch(vec![
            ConversationObservation::InteractionSettled {
                interaction: InteractionRef::new(
                    inner.conversation_id.clone(),
                    interaction_id.clone(),
                ),
                outcome: outcome.clone(),
                audit: audit.cloned(),
            },
            pending_head_observation(proof),
        ]));
    }
}

impl RuntimeInner {
    /// Publishes one semantic observation into the shared observation
    /// fan-out, when a projection consumer exists and the fan-out is open.
    ///
    /// This is a leaf publication: the runtime keeps no second fold of the
    /// observation vocabulary. Because the queue is installed while the
    /// runtime is still inactive (see
    /// [`RuntimeInner::install_observation_bridge`]) and an inactive
    /// runtime publishes nothing, an installed consumer observes every
    /// observation this runtime ever emits.
    #[allow(clippy::needless_pass_by_value)] // observer callers construct owned observations
    pub(super) fn observe(&self, observation: ConversationObservation) {
        if let ConversationObservation::CanonicalCommitted { receipt, .. } = &observation
            && (receipt.ordinal.checked_add(1) != Some(receipt.observation.count)
                || receipt.observation.journal_range.is_some())
        {
            // J-backed canonical writes belong to the same-commit native batch.
            // Its callback installs C, pending and lifecycle facts together and
            // releases their source readiness only after that complete CPU fold.
            return;
        }
        if let Some(pending) = self.pending.get() {
            match pending.push(&observation) {
                Some(crate::runtime::observation::NativeReceiptInstallation::Unavailable) => {
                    pending.source_failed();
                }
                _ => observation.publish_journal_source(self.store.as_ref(), pending),
            }
        }
    }
}
