//! The publication gate and the native publication commit: prepared observations
//! are linearized into the projection in commit order.
#[cfg(test)]
use super::ProjectionStage;
use super::sources::activity_domain_sources;
use super::sources::native_actor_transition;
use super::sources::observation_domain_sources;
use super::sources::observation_headers;
#[cfg(test)]
use super::sources::observation_terminal_positions;
use super::sources::prepare_display_sources;
use super::sources::source_value;
use super::sources::weighted_sources;
use super::{
    Arc, ClientState, ConversationObservation, ConversationStore, Mutex, MutexGuard,
    NativePublicationProjection, PendingObservations, PublicationGate, Publishing,
    RuntimeClientProjection,
};

impl PublicationGate {
    #[cfg(test)]
    pub(super) fn worker_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().worker = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    pub(super) fn preparation_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().preparation = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    pub(super) fn publication_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().publication = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    pub(super) fn read_stage(
        &self,
        name: &'static str,
        cut: &super::super::projection::ReadDomainCut,
    ) {
        self.diagnostics.lock().unwrap().read = Some((ProjectionStage::new(name), cut.clone()));
    }

    #[cfg(test)]
    pub(super) fn diagnostic(&self) -> String {
        let Ok(state) = self.diagnostics.try_lock() else {
            return "stage diagnostic busy or poisoned".into();
        };
        let describe =
            |value: &Option<ProjectionStage>| value.as_ref().map(ProjectionStage::describe);
        format!(
            "worker={:?} preparation={:?} publication={:?} physical_read={:?}",
            describe(&state.worker),
            describe(&state.preparation),
            describe(&state.publication),
            state
                .read
                .as_ref()
                .map(|(stage, cut)| (stage.describe(), cut))
        )
    }

    #[cfg(test)]
    pub(super) fn settlement_lock<'a>(&self, boundary: &'a Mutex<()>) -> MutexGuard<'a, ()> {
        match boundary.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                let hook = self.settlement_contention_hook.lock().unwrap().take();
                if let Some(hook) = hook {
                    hook();
                }
                boundary
                    .lock()
                    .expect("test settlement publication boundary")
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                panic!("test settlement publication boundary poisoned");
            }
        }
    }

    pub(super) fn generation(&self) -> u64 {
        self.progress
            .0
            .lock()
            .expect("native publication progress")
            .generation
    }
    /// Marks a transition in flight; the caller holds the publication order.
    pub(super) fn publishing(&self) -> Publishing<'_> {
        self.progress
            .0
            .lock()
            .expect("native publication progress")
            .publishing = true;
        Publishing(self)
    }
    pub(super) fn completed(&self) {
        let mut progress = self.progress.0.lock().expect("native publication progress");
        progress.generation = progress.generation.saturating_add(1);
        progress.publishing = false;
        self.progress.1.notify_all();
    }
    /// Waits until no transition is in flight, or the deadline passes
    /// (false). A transition that folded before this call is installed when
    /// it returns true.
    pub(super) fn wait_installed(&self, deadline: std::time::Instant) -> bool {
        let mut progress = self.progress.0.lock().expect("native publication progress");
        #[cfg(test)]
        if progress.publishing
            && let Some(hook) = self.repair_settle_hook.lock().unwrap().take()
        {
            hook();
        }
        while progress.publishing {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return false;
            };
            progress = self
                .progress
                .1
                .wait_timeout(progress, remaining)
                .expect("native publication progress wait")
                .0;
        }
        true
    }
    pub(super) fn wait_after(&self, generation: u64, deadline: std::time::Instant) {
        let mut current = self.progress.0.lock().expect("native publication progress");
        while current.generation == generation {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                break;
            };
            let (next, elapsed) = self
                .progress
                .1
                .wait_timeout(current, remaining)
                .expect("native publication progress wait");
            current = next;
            if elapsed.timed_out() {
                break;
            }
        }
    }
}
impl Drop for Publishing<'_> {
    fn drop(&mut self) {
        self.0.completed();
    }
}
/// One complete native display transition. Encoding cannot hold the raw state
/// lock, and a visible cursor is installed only after its frozen bytes exist.
pub(super) fn commit_display<T>(
    state: &Mutex<ClientState>,
    publication: &PublicationGate,
    fold: impl FnOnce(&mut RuntimeClientProjection) -> T,
) -> T {
    let _publication = publication
        .order
        .lock()
        .expect("native display publication gate");
    // A source failure may resolve a pending reader without creating a cursor.
    // Completing wakes readers after every CPU fold, including that failure
    // path, and only once the fold's publication is installed.
    let _publishing = publication.publishing();
    #[cfg(test)]
    publication.publication_stage("fold");
    let (draft, installed) = {
        let mut state = state.lock().expect("native display capture");
        state.projection.begin_publication();
        let installed = fold(&mut state.projection);
        (state.projection.finish_publication(), installed)
    };
    if let Some(draft) = draft {
        #[cfg(test)]
        publication.publication_stage("encode");
        #[cfg(test)]
        if let Some(hook) = publication.encoding_hook.lock().unwrap().take() {
            hook();
        }
        let encoded = draft.encode();
        #[cfg(test)]
        publication.publication_stage("install");
        let mut state = state.lock().expect("native display install");
        match encoded {
            Ok(encoded) => state.projection.install_publication(encoded),
            Err(error) => state.projection.publication_failed(&error),
        }
        drop(state);
    }
    #[cfg(test)]
    publication.publication_stage("idle");
    installed
}
pub(super) fn prepared_lock<'a>(
    state: &'a Mutex<ClientState>,
    preparation: &Mutex<()>,
    publication: &PublicationGate,
    pending: &PendingObservations,
    store: &dyn ConversationStore,
    workflows: Option<&crate::runtime::workflow::read_model::WorkflowReadModel>,
) -> MutexGuard<'a, ClientState> {
    let _preparation = preparation.lock().expect("projection preparation mutex");
    #[cfg(test)]
    publication.preparation_stage("drain");
    let observations = pending.drain();
    #[cfg(test)]
    if !observations.is_empty() {
        let hook = publication.preparation_hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    apply_prepared_observations(state, publication, observations, store, false, true);
    #[cfg(test)]
    publication.preparation_stage("workflow");
    if let Some(workflows) = workflows
        && !pending.has_unpublished()
    {
        let revision = state
            .lock()
            .expect("native Workflow descriptor")
            .projection
            .snapshot_ref()
            .workflows
            .revision;
        let cuts = workflows
            .published_source_cuts_after(revision)
            .into_iter()
            .filter(|cut| cut.revision > revision)
            .map(crate::observation::workflow_source::WorkflowReadSource::new)
            .collect::<Vec<_>>();
        if !cuts.is_empty() {
            apply_prepared_observations(
                state,
                publication,
                vec![ConversationObservation::Workflow(cuts)],
                store,
                false,
                true,
            );
        }
    }
    #[cfg(test)]
    publication.preparation_stage("idle");
    state.lock().expect("runtime client host lock poisoned")
}
pub(super) fn journal_preparation(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
) -> Vec<(
    u64,
    Option<crate::observation::failure::RuntimeClientAttemptFailure>,
    Option<crate::observation::tool::JournalFieldRef>,
)> {
    observation
        .facts()
        .filter_map(|fact| {
            let ConversationObservation::JournalFact(witness) = fact else {
                return None;
            };
            let failure = if witness.event_kind == "attempt_failed" {
                witness
                    .attempt_id
                    .as_ref()
                    .and_then(|attempt| {
                        store
                            .observation_attempt_summary(attempt, witness.sequence)
                            .ok()
                    })
                    .and_then(|summary| summary.failure)
            } else {
                None
            };
            let field = match witness.event_kind.as_str() {
                "tool_execution_completed" => Some("result"),
                "tool_execution_failed" => Some("error"),
                _ => None,
            };
            let source = field.and_then(|field| {
                let path = vec!["event".into(), field.into()];
                store
                    .event_value_read(witness.sequence, witness.sequence, &path, 0, 1)
                    .ok()
                    .map(|chunk| crate::observation::tool::JournalFieldRef {
                        sequence: witness.sequence,
                        path,
                        encoded_bytes: chunk.readable_end,
                    })
            });
            Some((witness.sequence, failure, source))
        })
        .take(32)
        .collect()
}
pub(super) fn apply_prepared_observations(
    state: &Mutex<ClientState>,
    publication_gate: &PublicationGate,
    observations: Vec<ConversationObservation>,
    store: &dyn ConversationStore,
    publication: bool,
    complete: bool,
) {
    for mut observation in observations {
        #[cfg(test)]
        publication_gate.preparation_stage("metadata");
        let mut model_capabilities = state
            .lock()
            .expect("source header state")
            .projection
            .current_model_capabilities();
        let mut capability_source = state
            .lock()
            .expect("source header state")
            .projection
            .current_capability_source();
        prepare_display_sources(
            &mut observation,
            &mut model_capabilities,
            &mut capability_source,
        );
        let publication_content =
            super::super::checkpoint::publication_content_evidence(&observation);
        let references = super::super::checkpoint::observation_metadata(store, &observation);
        let headers = observation_headers(store, &observation);
        #[cfg(test)]
        let terminals = observation_terminal_positions(store, &observation, None);
        let journal = journal_preparation(store, &observation);
        let mut sources = observation_domain_sources(&observation);
        sources.extend(activity_domain_sources(state, &observation, &sources));
        let sources = weighted_sources(sources);
        super::super::checkpoint::bound_publication_payload(&mut observation);
        #[cfg(test)]
        publication_gate.preparation_stage("publish");
        commit_display(state, publication_gate, |projection| {
            projection.prepare_publication_content(publication_content);
            projection.install_content_metadata(references);
            projection.prepare_message_headers(headers);
            #[cfg(test)]
            projection.prepare_terminal_positions(terminals);
            for (sequence, failure, source) in journal {
                projection.prepare_journal_fact(sequence, failure, source);
            }
            projection.publication_commit_scope(publication, complete);
            projection.apply(observation);
            projection.publication_commit_scope(false, complete);
            projection.install_domain_sources(sources);
        });
    }
}
impl NativePublicationProjection {
    pub(super) fn commit_observation(
        &self,
        observation: ConversationObservation,
        content: Vec<super::super::checkpoint::PreparedPublicationContent>,
        receipt: Option<&crate::publication::PublicationFrameReceipt>,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let complete = true;
        #[cfg(test)]
        let mut hook = matches!(&observation, ConversationObservation::Publication { .. })
            .then(|| inner.publication_commit_hook.lock().unwrap().take())
            .flatten();
        #[cfg(test)]
        if let Some(hook) = &mut hook {
            hook(&inner, std::slice::from_ref(&observation), false);
        }
        #[cfg(test)]
        let evidence = hook.as_ref().map(|_| observation.clone());
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.prepare_publication_content(content);
            if let Some(receipt) = receipt {
                projection.prepare_publication_frontier(receipt);
            }
            projection.publication_commit_scope(true, complete);
            projection.apply(observation);
            projection.publication_commit_scope(false, complete);
        });
        #[cfg(test)]
        if let Some(hook) = &mut hook {
            hook(
                &inner,
                std::slice::from_ref(evidence.as_ref().unwrap()),
                true,
            );
        }
    }
}
impl crate::runtime::observation::PublicationProjectionCommit for NativePublicationProjection {
    fn native_actor(
        &self,
        observation: &ConversationObservation,
    ) -> Option<crate::runtime::observation::NativeReceiptInstallation> {
        if !native_actor_transition(observation) {
            return None;
        }
        let Some(inner) = self.inner.upgrade() else {
            return Some(crate::runtime::observation::NativeReceiptInstallation::Unavailable);
        };
        let (mut model_capabilities, mut capability_source) = {
            let state = inner.state.lock().expect("native actor source descriptors");
            (
                state.projection.current_model_capabilities(),
                state.projection.current_capability_source(),
            )
        };
        let mut observation = observation.clone();
        // Immutable source headers are prepared without a Projection lock,
        // storage reads, or waiting for the asynchronous display worker.
        prepare_display_sources(
            &mut observation,
            &mut model_capabilities,
            &mut capability_source,
        );
        let audit_batches = observation
            .facts()
            .filter_map(|fact| match fact {
                ConversationObservation::InteractionPending {
                    audit: Some((event, _)),
                    ..
                }
                | ConversationObservation::InteractionSettled {
                    audit: Some((event, _)),
                    ..
                } => Some(event.observation.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let sources = weighted_sources(observation_domain_sources(&observation));
        Some(commit_display(
            &inner.state,
            &inner.publication_gate,
            |projection| {
                projection.publication_commit_scope(true, true);
                projection.apply(observation);
                for batch in audit_batches {
                    if let Some(batch) = batch {
                        projection.apply_native_batch(&batch);
                    } else {
                        projection.mark_observation_unavailable(
                            "native Interaction audit source unavailable".into(),
                        );
                    }
                }
                projection.install_domain_sources(sources);
                projection.publication_commit_scope(false, true);
                projection.native_receipt_installation()
            },
        ))
    }
    fn native_owner(
        &self,
        observation: &ConversationObservation,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let sources = weighted_sources(observation_domain_sources(observation));
        let complete = batch.is_some();
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, complete);
            projection.apply(observation.clone());
            if let Some(batch) = batch {
                projection.apply_native_batch(batch);
            } else {
                projection.mark_observation_unavailable(
                    "native Registry commit source unavailable".into(),
                );
            }
            projection.install_domain_sources(sources);
            projection.publication_commit_scope(false, complete);
            projection.native_receipt_installation()
        })
    }
    fn native_goal(
        &self,
        receipt: &crate::durable::inbox::GoalObservationReceipt,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let sources = weighted_sources(vec![(
            "goal".into(),
            Arc::new(super::super::domain::DomainSource::Goal(source_value(
                receipt.view.clone(),
            ))),
        )]);
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply(ConversationObservation::GoalChanged(receipt.view.clone()));
            if let Some(pending) = &receipt.pending {
                projection.apply_pending_receipt(Some(pending));
            }
            if let Some(native) = &receipt.native {
                projection.apply_native_batch(native);
            } else {
                projection.mark_observation_unavailable(
                    "native Goal observation source unavailable".into(),
                );
            }
            projection.install_domain_sources(sources);
            projection.publication_commit_scope(false, receipt.native.is_some());
            projection.native_receipt_installation()
        })
    }
    fn native_audit(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        receipt: &crate::durable::inbox::PublicationAuditReceipt,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply(ConversationObservation::AuditSettled {
                attempt_id: attempt.clone(),
                receipt: receipt.clone(),
            });
            projection.publication_commit_scope(false, true);
        });
        #[cfg(test)]
        if let Some(hook) = inner.audit_commit_hook.lock().unwrap().take() {
            hook(&inner);
        }
    }
    fn native_pending(
        &self,
        receipt: Option<&crate::durable::inbox::PendingObservationReceipt>,
        _cut: &crate::durable::reading::ConversationReadCut,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply_pending_receipt(receipt);
            projection.publication_commit_scope(false, true);
        });
    }
    fn native_manual_compaction(
        &self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.apply_manual_compaction(request, batch);
            projection.native_receipt_installation()
        })
    }

    fn native_batch(
        &self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let complete = true;
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, complete);
            projection.apply_native_batch(batch);
            projection.publication_commit_scope(false, complete);
            projection.native_receipt_installation()
        })
    }
    fn opened(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        start: &crate::publication::PublicationStreamStart,
    ) {
        self.commit_observation(
            ConversationObservation::PublicationOpened {
                attempt_id: attempt.clone(),
                start: start.clone(),
            },
            Vec::new(),
            None,
        );
    }
    fn frame(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        frame: &crate::publication::PublicationFrame,
        receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        if receipt.unavailable {
            if let Some(inner) = self.inner.upgrade() {
                inner
                    .state
                    .lock()
                    .expect("native publication source fence")
                    .projection
                    .mark_observation_unavailable(
                        "native publication display source unavailable".into(),
                    );
            }
            return;
        }
        assert_eq!(
            frame.stream_id, receipt.stream_id,
            "frozen native staging stream"
        );
        assert_eq!(
            frame.sequence, receipt.sequence,
            "frozen native staging sequence"
        );
        let content = receipt
            .content
            .iter()
            .filter_map(|reference| {
                let crate::observation::content::ContentSource::LivePublicationPrefix {
                    component,
                    block_index,
                    ..
                } = &reference.source
                else {
                    return None;
                };
                Some(super::super::checkpoint::PreparedPublicationContent {
                    suffix: receipt.suffix_bytes.is_some()
                        && *component
                            == crate::observation::content::PublicationContentComponent::Payload,
                    stream_id: receipt.stream_id.clone(),
                    block_index: *block_index,
                    revision: reference.content_revision,
                    component: *component,
                    representation: reference.representation,
                    base_length: receipt
                        .suffix_bytes
                        .filter(|_| {
                            *component
                                == crate::observation::content::PublicationContentComponent::Payload
                        })
                        .map(|bytes| {
                            reference
                                .readable_end
                                .checked_sub(bytes)
                                .expect("frozen staged suffix frontier")
                        }),
                    readable_end: reference.readable_end,
                })
            })
            .collect();
        let frame = super::super::checkpoint::bounded_publication_frame(frame);
        self.commit_observation(
            ConversationObservation::Publication {
                attempt_id: attempt.clone(),
                frame,
            },
            content,
            Some(receipt),
        );
    }
}
