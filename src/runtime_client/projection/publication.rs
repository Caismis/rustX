//! Publication of client events: staging, encoding, the bounded replay ring and live content frontiers.
#[cfg(test)]
use super::PublicationEncodingProfile;
#[cfg(test)]
use super::ReplayWeight;
use super::prepared_capture;
#[cfg(test)]
use super::views::accumulate_transition;
use super::{
    Arc, AttemptId, ContentBlockIndex, EncodedPublication, PublicationDraft, PublicationFrame,
    RUNTIME_CLIENT_REPLAY_BYTES, RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent,
    RuntimeClientProjection, accumulate_finite_facts, set_checkpoint_transition_cursor,
};

/// Canonical wire literals are independent of hydrated reads and presentation
/// refinements. Only actual page/head/checkpoint carriers replace these bases.
#[derive(Clone)]
pub(super) struct PublicationBasis {
    pub cursor: RuntimeClientCursor,
    pub metadata: crate::runtime_client::checkpoint::ProjectionMetadata,
    pub transcript: Option<Arc<crate::observation::transcript::RuntimeClientTranscriptPage>>,
    pub descriptors:
        Option<Arc<Vec<Arc<crate::observation::transcript::RuntimeClientMessageDescriptor>>>>,
    pub checkpoint: Option<Arc<crate::runtime_client::RuntimeClientSnapshot>>,
    pub weight: usize,
    pub weights: [usize; 3],
}
impl PublicationBasis {
    fn advance(previous: Option<&Self>, event: &RuntimeClientEvent, weight: usize) -> Self {
        let RuntimeClientEvent::ProjectionTransition { metadata, events } = event else {
            unreachable!("complete frozen transition")
        };
        let mut next = Self {
            cursor: RuntimeClientCursor::new(metadata.read_cut.revision),
            metadata: metadata.clone(),
            transcript: previous.and_then(|base| base.transcript.clone()),
            descriptors: previous.and_then(|base| base.descriptors.clone()),
            checkpoint: previous.and_then(|base| base.checkpoint.clone()),
            weight,
            weights: previous.map_or([0; 3], |base| base.weights),
        };
        for event in events {
            match event {
                RuntimeClientEvent::ReadDomainsUpdated { transcript, .. } => {
                    next.weights[0] = weight;
                    next.transcript = Some(Arc::new(*transcript.clone()));
                }
                RuntimeClientEvent::MessageDescriptorCommitted { head, .. } => {
                    next.weights[1] = weight;
                    next.descriptors = Some(Arc::new(head.iter().cloned().collect()));
                }
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                    next.weights = [weight; 3];
                    next.transcript = Some(Arc::new(checkpoint.snapshot.transcript.clone()));
                    next.descriptors =
                        Some(Arc::new(checkpoint.snapshot.message_descriptors.clone()));
                    next.checkpoint = Some(Arc::new(checkpoint.snapshot.clone()));
                }
                _ => {}
            }
        }
        next.weight = next.weights.into_iter().max().unwrap_or(0);
        next
    }
}

/// A subscriber carries a few initialized-domain bits, never an event backlog.
#[derive(Default)]
pub(super) struct DeliveryBases(u8);
impl DeliveryBases {
    pub(super) fn delivery(
        &mut self,
        event: &Arc<RuntimeClientEvent>,
    ) -> Option<Arc<RuntimeClientEvent>> {
        let mut required = 1;
        if let RuntimeClientEvent::ProjectionDelta { events, .. } = event.as_ref() {
            for fact in events {
                match fact {
                    RuntimeClientEvent::ReadDomainsPatched { .. } => required |= 2,
                    RuntimeClientEvent::MessageDescriptorPatched { .. } => required |= 4,
                    RuntimeClientEvent::CheckpointPatched { .. } => required |= 8,
                    _ => {}
                }
            }
        }
        let event = if self.0 & required == required {
            event.clone()
        } else {
            match event.as_ref() {
                RuntimeClientEvent::ProjectionDelta { metadata, .. } => {
                    metadata.native_transition.clone()?
                }
                _ => event.clone(),
            }
        };
        if let RuntimeClientEvent::ProjectionTransition { events, .. }
        | RuntimeClientEvent::ProjectionDelta { events, .. } = event.as_ref()
        {
            self.0 |= 1;
            for fact in events {
                match fact {
                    RuntimeClientEvent::ReadDomainsUpdated { .. } => self.0 |= 2,
                    RuntimeClientEvent::MessageDescriptorCommitted { .. } => self.0 |= 4,
                    RuntimeClientEvent::CheckpointAdvanced { .. } => self.0 |= 0b1110,
                    _ => {}
                }
            }
        }
        Some(event)
    }
}

impl PublicationDraft {
    /// Freeze typed edits and meter only their JSON outside native locks.
    #[allow(clippy::too_many_lines)] // Admission and rare complete replacement share one cut.
    pub(crate) fn encode(mut self) -> Result<EncodedPublication, RuntimeClientError> {
        use crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
        use crate::runtime_client::checkpoint::{
            ProjectionMetadata, RuntimeClientCheckpoint, certified_checkpoint_fit,
        };
        use prepared_capture::CapturedCheckpoint;
        #[cfg(test)]
        let started = std::time::Instant::now();
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
        self.seed.snapshot.trace.stamp_cut(&self.seed.read_cut);
        for event in &mut self.events {
            stamp_event_trace_cut(event, &self.seed.read_cut);
        }
        let previous = self
            .previous
            .as_ref()
            .and_then(|seed| seed.display.as_deref());
        let (event, encoded, display) = {
            let CapturedCheckpoint {
                snapshot,
                mut metadata,
                cursor,
                evicted,
                fit_budget,
                mut metadata_budget,
                prepared_metadata,
            } = self.seed.capture_checkpoint()?;
            let mut arrays = Some(prepared_metadata);
            let mut snapshot = Some(snapshot);
            let mut owned = None;
            let mut fitted = false;
            if !certified_checkpoint_fit(fit_budget) {
                let mut checkpoint = RuntimeClientCheckpoint {
                    snapshot: snapshot.take().expect("one materialization")(),
                    cursor,
                    read_cut: metadata.read_cut,
                    content: metadata.content,
                    collections: metadata.collections,
                };
                fitted =
                    super::checkpoint::fit_checkpoint_with_budget(&mut checkpoint, fit_budget)?;
                if fitted {
                    arrays = None;
                    metadata_budget = None;
                }
                metadata = ProjectionMetadata {
                    read_cut: checkpoint.read_cut.clone(),
                    content: std::mem::take(&mut checkpoint.content),
                    collections: std::mem::take(&mut checkpoint.collections),
                };
                owned = Some(checkpoint);
            }
            let mut complete = RuntimeClientEvent::ProjectionTransition {
                events: self.events,
                metadata: metadata.clone(),
            };
            set_checkpoint_transition_cursor(&mut complete, cursor);
            let replace = fitted || evicted || self.replacement;
            if replace {
                complete = replacement(
                    &metadata,
                    cursor,
                    owned.take().unwrap_or_else(|| RuntimeClientCheckpoint {
                        snapshot: snapshot.take().expect("one materialization")(),
                        cursor,
                        read_cut: metadata.read_cut.clone(),
                        content: Arc::default(),
                        collections: Arc::default(),
                    }),
                );
            }
            let reserve = fit_budget
                .unwrap_or(CHECKPOINT_TARGET_BYTES)
                .max(previous.map_or(0, |base| base.weight));
            let native_weight = |event: &RuntimeClientEvent| {
                let heavy = event
                    .facts()
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            RuntimeClientEvent::ReadDomainsUpdated { .. }
                                | RuntimeClientEvent::MessageDescriptorCommitted { .. }
                                | RuntimeClientEvent::CheckpointAdvanced { .. }
                        )
                    })
                    .count();
                metadata_budget
                    .unwrap_or(CHECKPOINT_TARGET_BYTES)
                    .saturating_add(reserve.saturating_mul(heavy))
            };
            let mut weight = native_weight(&complete);
            let mut event = previous.map_or_else(
                || complete.clone(),
                |base| incremental_event(complete.clone(), base, weight),
            );
            let mut encoded = meter_with_metadata(&event, metadata_budget, arrays.as_ref());
            // The first delivery of any domain uses the exact original carrier.
            // Large uncertain admissions retain the original bounded fallback.
            if !replace
                && (encoded.is_none()
                    || weight.saturating_add(encoded.unwrap_or(0)) > CHECKPOINT_TARGET_BYTES
                        && meter_with_metadata(&complete, metadata_budget, arrays.as_ref())
                            .is_none())
            {
                complete = replacement(
                    &metadata,
                    cursor,
                    owned.take().unwrap_or_else(|| RuntimeClientCheckpoint {
                        snapshot: snapshot.take().expect("one materialization")(),
                        cursor,
                        read_cut: metadata.read_cut.clone(),
                        content: Arc::default(),
                        collections: Arc::default(),
                    }),
                );
                weight = native_weight(&complete);
                event = previous.map_or_else(
                    || complete.clone(),
                    |base| incremental_event(complete.clone(), base, weight),
                );
                encoded = meter_with_metadata(&event, metadata_budget, arrays.as_ref());
            }
            let encoded = encoded.ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                message: "finite display transition exceeds its encoded budget".into(),
            })?;
            let display = PublicationBasis::advance(
                previous,
                &complete,
                fit_budget.unwrap_or(CHECKPOINT_TARGET_BYTES),
            );
            (event, encoded, Arc::new(display))
        };
        self.seed.display = Some(display);
        #[cfg(test)]
        if let Some(profile) = &self.encoding_profile {
            let mut profile = profile.lock().unwrap();
            profile.records += 1;
            profile.bytes += encoded;
            profile.encode_us += started.elapsed().as_micros();
        }
        Ok(EncodedPublication {
            seed: Arc::new(self.seed),
            event,
            bytes: encoded,
            sources: self.sources,
        })
    }
}

fn meter_with_metadata(
    event: &RuntimeClientEvent,
    metadata_budget: Option<usize>,
    arrays: Option<&crate::runtime_client::checkpoint::metadata_fragment::PreparedMetadata>,
) -> Option<usize> {
    let limit = crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
    if let RuntimeClientEvent::ProjectionTransition { events, .. } = event {
        let events_budget =
            crate::observation::preflight::required_safe_integer_budget(events, limit);
        return crate::runtime_client::checkpoint::metadata_fragment::transition_length(
            event,
            events_budget,
            prepared_capture::FitBudget::transition(events_budget, metadata_budget),
            limit,
            arrays,
        );
    }
    let budget = crate::observation::preflight::required_safe_integer_budget(event, limit);
    crate::observation::bounded::bounded_structural_len_with_budget(event, limit, budget)
}

fn replacement(
    metadata: &crate::runtime_client::checkpoint::ProjectionMetadata,
    cursor: RuntimeClientCursor,
    mut checkpoint: crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
) -> RuntimeClientEvent {
    checkpoint.cursor = cursor;
    checkpoint.content = Arc::default();
    checkpoint.collections = Arc::default();
    RuntimeClientEvent::ProjectionTransition {
        events: vec![RuntimeClientEvent::CheckpointAdvanced {
            transition: "projection_transition".into(),
            checkpoint: Box::new(checkpoint),
        }],
        metadata: metadata.clone(),
    }
}

fn incremental_event(
    complete: RuntimeClientEvent,
    base: &PublicationBasis,
    weight: usize,
) -> RuntimeClientEvent {
    use crate::runtime_client::delta::{
        CheckpointDelta, ListDelta, MetadataDelta, TranscriptDelta,
    };
    let RuntimeClientEvent::ProjectionTransition { events, metadata } = &complete else {
        unreachable!("frozen transition")
    };
    let events = events
        .iter()
        .map(|event| match event {
            RuntimeClientEvent::ReadDomainsUpdated {
                projection_head,
                transcript,
                trace,
                occupancy,
                todos,
            } if base.transcript.is_some() => RuntimeClientEvent::ReadDomainsPatched {
                projection_head: projection_head.clone(),
                transcript: Box::new(TranscriptDelta::between(
                    base.transcript.as_ref().unwrap(),
                    transcript,
                )),
                trace: trace.clone(),
                occupancy: occupancy.clone(),
                todos: todos.clone(),
            },
            RuntimeClientEvent::MessageDescriptorCommitted {
                attempt_id,
                descriptor,
                head,
                transcript_cursor,
                evicted_message_id,
            } if base.descriptors.is_some() => {
                let mut prior = base.descriptors.as_ref().unwrap().as_ref().clone();
                prior.push(descriptor.clone());
                RuntimeClientEvent::MessageDescriptorPatched {
                    attempt_id: attempt_id.clone(),
                    descriptor: descriptor.clone(),
                    head: ListDelta::between(&prior, &head.iter().cloned().collect::<Vec<_>>()),
                    transcript_cursor: *transcript_cursor,
                    evicted_message_id: evicted_message_id.clone(),
                }
            }
            RuntimeClientEvent::CheckpointAdvanced {
                transition,
                checkpoint,
            } if base.checkpoint.is_some() => RuntimeClientEvent::CheckpointPatched {
                transition: transition.clone(),
                checkpoint: Box::new({
                    let mut prior = base.checkpoint.as_ref().unwrap().as_ref().clone();
                    prior.transcript = base.transcript.as_ref().unwrap().as_ref().clone();
                    prior
                        .message_descriptors
                        .clone_from(base.descriptors.as_ref().unwrap().as_ref());
                    CheckpointDelta::between(&prior, checkpoint)
                }),
            },
            event => event.clone(),
        })
        .collect();
    let mut delta = MetadataDelta::from_parts(
        base.cursor,
        &base.metadata.content,
        &base.metadata.collections,
        &base.metadata.read_cut,
        metadata,
    );
    delta.native_bytes = weight;
    delta.native_transition = Some(Arc::new(complete));
    RuntimeClientEvent::ProjectionDelta {
        events,
        metadata: Box::new(delta),
    }
}

/// Freeze only owned vector storage. Shared row bodies and every encoded
/// value/cut/budget remain unchanged; no published owner is mutated.
pub(super) fn compact_replay_storage(event: &mut RuntimeClientEvent) {
    match event {
        RuntimeClientEvent::ProjectionDelta { events, .. } => {
            for event in events.iter_mut() {
                compact_replay_storage(event);
            }
            events.shrink_to_fit();
        }
        RuntimeClientEvent::ProjectionTransition { events, metadata } => {
            for event in events.iter_mut() {
                compact_replay_storage(event);
            }
            events.shrink_to_fit();
            if let Some(values) = Arc::get_mut(&mut metadata.content) {
                values.shrink_to_fit();
            }
            if let Some(values) = Arc::get_mut(&mut metadata.collections) {
                values.shrink_to_fit();
            }
        }
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            if let Some(values) = Arc::get_mut(&mut checkpoint.content) {
                values.shrink_to_fit();
            }
            if let Some(values) = Arc::get_mut(&mut checkpoint.collections) {
                values.shrink_to_fit();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(super) fn encoded_event_bytes(event: &RuntimeClientEvent, limit: usize) -> Option<Vec<u8>> {
    let schema = crate::observation::structural_json::root::<RuntimeClientEvent>();
    crate::observation::bounded::bounded_bytes(
        &crate::observation::structural_json::Encoded {
            value: event,
            schema: &schema,
            root: &schema,
        },
        limit,
    )
}
pub(super) fn stamp_trace_cut(
    trace: &mut crate::observation::trace::TracePage,
    cut: &crate::observation::cut::CheckpointReadCut,
) {
    if trace.read_cut.is_none() && (!trace.records.is_empty() || !trace.updates.is_empty()) {
        trace.read_cut = Some(cut.clone());
    }
}
pub(super) fn stamp_event_trace_cut(
    event: &mut RuntimeClientEvent,
    cut: &crate::observation::cut::CheckpointReadCut,
) {
    match event {
        RuntimeClientEvent::ReadDomainsUpdated {
            trace: Some(trace), ..
        }
        | RuntimeClientEvent::ReadDomainsPatched {
            trace: Some(trace), ..
        } => stamp_trace_cut(trace, cut),
        RuntimeClientEvent::ProjectionTransition { events, .. }
        | RuntimeClientEvent::ProjectionDelta { events, .. } => {
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
#[cfg(test)]
pub(super) fn event_kind(event: &RuntimeClientEvent) -> &'static str {
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

impl RuntimeClientProjection {
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

    pub(super) fn current_live_cut(
        &self,
    ) -> Option<crate::observation::cut::LivePublicationReadCut> {
        let start = self.publication_start.as_ref()?;
        Some(crate::observation::cut::LivePublicationReadCut {
            source: crate::observation::content::ContentSource::LivePublicationPrefix {
                component: crate::observation::content::PublicationContentComponent::Payload,
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
        evidence: Vec<super::super::checkpoint::PreparedPublicationContent>,
    ) {
        self.prepared_publication_content = evidence;
    }

    pub(super) fn observed_structural_content(
        &mut self,
        attempt: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        component: crate::observation::content::PublicationContentComponent,
    ) -> Option<crate::observation::content::ContentRef> {
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
        let reference = crate::observation::content::ContentRef {
            conversation_id: self.snapshot.conversation_id.clone(),
            source: crate::observation::content::ContentSource::LivePublicationPrefix {
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

    pub(super) fn content_advanced(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
        block_index: ContentBlockIndex,
        suffix: &str,
        kind: &str,
        representation: crate::observation::content::ContentRepresentation,
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
        let content_ref = crate::observation::content::ContentRef {
            conversation_id: self.snapshot.conversation_id.clone(),
            source: crate::observation::content::ContentSource::LivePublicationPrefix {
                component: crate::observation::content::PublicationContentComponent::Payload,
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
                && suffix.len() <= crate::observation::bounded::CHECKPOINT_ITEM_BYTES)
                .then(|| suffix.to_owned()),
            content_ref,
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
            previous: Some(self.checkpoint_seed()),
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
        let bytes = encoded.bytes.saturating_add(match &encoded.event {
            RuntimeClientEvent::ProjectionDelta { metadata, .. } => metadata.native_bytes,
            _ => 0,
        });
        self.retain_replay(encoded.event, bytes);
        self.wake_subscribers();
    }

    pub(super) fn projection_metadata(&self) -> super::super::checkpoint::ProjectionMetadata {
        if self.staged_publication.is_some() {
            return super::super::checkpoint::ProjectionMetadata {
                read_cut: self.working_read_cut(),
                collections: Arc::default(),
                content: Arc::default(),
            };
        }
        if let Ok(checkpoint) = self.checkpoint_candidate() {
            super::super::checkpoint::ProjectionMetadata {
                read_cut: checkpoint.read_cut,
                collections: checkpoint.collections,
                content: checkpoint.content,
            }
        } else {
            let mut read_cut = self.checkpoint_read_cut();
            read_cut.source_cut = None;
            super::super::checkpoint::ProjectionMetadata {
                read_cut,
                collections: Arc::default(),
                content: Arc::default(),
            }
        }
    }

    /// Allocate the client cursor, retain the bounded replay entry and wake
    /// subscribers. No subscriber owns an event queue or blocks publication.
    pub(super) fn publish(&mut self, event: RuntimeClientEvent) {
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
    pub(super) fn fixture_batch_event(
        &mut self,
        event: RuntimeClientEvent,
    ) -> Option<RuntimeClientEvent> {
        if let Some((events, bytes, overflow)) = &mut self.batch_events {
            if !*overflow {
                let encoded = crate::observation::bounded::bounded_bytes(
                    &event,
                    crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 2,
                );
                if let Some(encoded) = encoded
                    && events.len() < crate::observation::bounded::CHECKPOINT_PAGE_ITEMS
                    && *bytes + encoded.len()
                        <= crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 2
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
    pub(super) fn fixture_event_weight(event: &RuntimeClientEvent) -> usize {
        let mut weight = ReplayWeight(0);
        if serde_json::to_writer(&mut weight, event).is_ok() {
            weight.0
        } else {
            RUNTIME_CLIENT_REPLAY_BYTES + 1
        }
    }

    #[cfg(test)]
    pub(super) fn publish_fixture(&mut self, event: RuntimeClientEvent) {
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
        self.snapshot.trace.stamp_cut(&cut);
        let mut event = event;
        stamp_event_trace_cut(&mut event, &cut);
        let mut event = if crate::observation::bounded::bounded_bytes(
            &event,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 2,
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
        let event = if crate::observation::bounded::bounded_bytes(
            &event,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
        )
        .is_some()
        {
            event
        } else {
            let transition = event_kind(&event).to_owned();
            let Ok(mut checkpoint) = self.checkpoint_candidate() else {
                self.mark_observation_unavailable("native display replacement unavailable".into());
                return;
            };
            checkpoint.cursor = RuntimeClientCursor::new(next_value);
            checkpoint.read_cut.revision = next_value;
            let mut metadata = self.projection_metadata();
            metadata.read_cut.revision = next_value;
            // The complete transition carries this evidence once, in its metadata.
            checkpoint.content = Arc::default();
            checkpoint.collections = Arc::default();
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

    pub(super) fn retained_event_cut(
        event: &RuntimeClientEvent,
    ) -> Option<&crate::observation::cut::CheckpointReadCut> {
        match event {
            RuntimeClientEvent::ProjectionTransition { metadata, .. } => Some(&metadata.read_cut),
            RuntimeClientEvent::ProjectionDelta { metadata, .. } => metadata.native_cut.as_ref(),
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => Some(&checkpoint.read_cut),
            _ => None,
        }
    }

    pub(super) fn retain_replay(&mut self, mut event: RuntimeClientEvent, bytes: usize) {
        debug_assert!(self.replay.back().is_none_or(|(previous, _)| {
            previous.get().checked_add(1) == Some(self.cursor.get())
        }));
        compact_replay_storage(&mut event);
        if let Some(cut) = Self::retained_event_cut(&event) {
            self.replay_cut_cursors.insert(cut.revision, self.cursor);
        }
        self.replay_sizes.push_back(bytes);
        self.replay_bytes += bytes;
        self.replay.push_back((self.cursor, Arc::new(event)));
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

    pub(super) fn retire_initial_read_cut(&mut self) {
        if let Some(cut) = self.initial_read_cut.take() {
            if self.replay_cut_cursors.get(&cut.revision) == Some(&RuntimeClientCursor::new(0)) {
                self.replay_cut_cursors.remove(&cut.revision);
            }
            self.replay_bytes -= crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
        }
    }

    /// Wakes every registered subscriber. Notification carries no payload
    /// and never blocks the publisher.
    pub(super) fn wake_subscribers(&self) {
        for subscriber in &self.subscribers {
            subscriber.notify.notify_one();
        }
    }

    #[cfg(test)]
    pub(crate) fn profile_encoding(
        &mut self,
        profile: Arc<std::sync::Mutex<PublicationEncodingProfile>>,
    ) {
        self.encoding_profile = Some(profile);
    }

    #[cfg(test)]
    pub(super) fn probe_publish_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.publish_enter();
        }
    }
}
