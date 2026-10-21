//! Publication of client events: staging, encoding, the bounded replay ring and live content frontiers.
#[cfg(test)]
use super::PublicationEncodingProfile;
#[cfg(test)]
use super::ReplayWeight;
#[cfg(test)]
use super::views::accumulate_transition;
use super::{
    Arc, AttemptId, ContentBlockIndex, EncodedPublication, PublicationDraft, PublicationFrame,
    RUNTIME_CLIENT_REPLAY_BYTES, RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent,
    RuntimeClientProjection, accumulate_finite_facts, set_checkpoint_transition_cursor,
};

impl PublicationDraft {
    /// CPU-only encoding of a frozen transition, outside every native state lock.
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
        stamp_trace_cut(&mut self.seed.snapshot.trace, &self.seed.read_cut);
        for event in &mut self.events {
            stamp_event_trace_cut(event, &self.seed.read_cut);
        }
        let (mut checkpoint, captured_eviction) = self.seed.checkpoint_unfitted()?;
        #[cfg(test)]
        encoding_phase(&mut timing, |profile, elapsed| {
            profile.capture_us += elapsed;
        });
        let fitted = super::super::checkpoint::fit_checkpoint(&mut checkpoint)?;
        #[cfg(test)]
        encoding_phase(&mut timing, |profile, elapsed| {
            profile.fit_us += elapsed;
        });
        let metadata = super::super::checkpoint::ProjectionMetadata {
            read_cut: checkpoint.read_cut.clone(),
            collections: checkpoint.collections.clone(),
            content: checkpoint.content.clone(),
        };
        let mut event = RuntimeClientEvent::ProjectionTransition {
            events: self.events,
            metadata: metadata.clone(),
        };
        set_checkpoint_transition_cursor(&mut event, self.seed.cursor);
        let encoded = if fitted || captured_eviction || self.replacement {
            None
        } else {
            encoded_event_bytes(&event, crate::observation::bounded::CHECKPOINT_TARGET_BYTES)
        };
        if encoded.is_none() {
            let transition = event_kind(&event).to_owned();
            let mut replacement = checkpoint;
            replacement.content.clear();
            replacement.collections.clear();
            event = RuntimeClientEvent::ProjectionTransition {
                events: vec![RuntimeClientEvent::CheckpointAdvanced {
                    transition,
                    checkpoint: Box::new(replacement),
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
                encoded_event_bytes(&event, crate::observation::bounded::CHECKPOINT_TARGET_BYTES)
            })
            .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                message: "finite display transition exceeds its encoded budget".into(),
            })?;
        #[cfg(test)]
        if let Some((mut measured, started)) = timing {
            measured.encode_us += started.elapsed().as_micros();
            let mut total = self.encoding_profile.as_ref().unwrap().lock().unwrap();
            total.records += 1;
            total.bytes += encoded.len();
            total.capture_us += measured.capture_us;
            total.fit_us += measured.fit_us;
            total.probe_us += measured.probe_us;
            total.encode_us += measured.encode_us;
        }
        Ok(EncodedPublication {
            seed: Arc::new(self.seed),
            event,
            bytes: encoded.len(),
            sources: self.sources,
        })
    }
}
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
        RuntimeClientEvent::ReadDomainsUpdated { trace, .. } => stamp_trace_cut(trace, cut),
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
#[cfg(test)]
pub(super) fn encoding_phase(
    timing: &mut Option<(PublicationEncodingProfile, std::time::Instant)>,
    record: fn(&mut PublicationEncodingProfile, u128),
) {
    if let Some((profile, start)) = timing {
        record(profile, start.elapsed().as_micros());
        *start = std::time::Instant::now();
    }
}
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
        let (events, _, replacement) = self
            .staged_publication
            .take()
            .expect("native publication reservation");
        if self.native_source_pending() {
            for event in events {
                accumulate_finite_facts(&mut self.deferred_events, event);
            }
            self.deferred_events.2 |= replacement;
            return None;
        }
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

    pub(super) fn projection_metadata(&self) -> super::super::checkpoint::ProjectionMetadata {
        if self.staged_publication.is_some() {
            return super::super::checkpoint::ProjectionMetadata {
                read_cut: self.working_read_cut(),
                collections: Vec::new(),
                content: Vec::new(),
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
                collections: Vec::new(),
                content: Vec::new(),
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
        stamp_trace_cut(&mut self.snapshot.trace, &cut);
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

    pub(super) fn retained_event_cut(
        event: &RuntimeClientEvent,
    ) -> Option<&crate::observation::cut::CheckpointReadCut> {
        match event {
            RuntimeClientEvent::ProjectionTransition { metadata, .. } => Some(&metadata.read_cut),
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => Some(&checkpoint.read_cut),
            _ => None,
        }
    }

    pub(super) fn retain_replay(&mut self, event: RuntimeClientEvent, bytes: usize) {
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
