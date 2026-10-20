//! Canonical receipt, descriptor and audit folding, and in-flight foreground text/argument assembly.
use super::{
    Arc, AttemptId, ContentBlockIndex, ForegroundSettlement, ForegroundToolState, InFlightBlock,
    MessageId, RuntimeClientEvent, RuntimeClientProjection, RuntimeClientTranscriptCursor,
    TextKind, ToolCallId, ToolExecutionResult, append_inline_prefix, arguments_of, block_index_of,
    canonical_header_position, canonical_locator_position, foreground_slot_for_event,
    inline_prefix, retain_tool_execution,
};

impl RuntimeClientProjection {
    pub(super) fn fold_canonical_receipt(
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
                trace: self.snapshot.trace.clone(),
                occupancy: self.snapshot.context.occupancy.clone(),
                todos: self.snapshot.todos.clone(),
            });
        }
        events
    }

    pub(super) fn retain_descriptor_execution(
        &self,
        descriptor: &mut crate::observation::transcript::RuntimeClientMessageDescriptor,
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

    pub(super) fn canonical_header_unchanged(
        &self,
        header: &crate::durable::observation::MessageHeader,
        descriptor: &crate::observation::transcript::RuntimeClientMessageDescriptor,
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

    pub(super) fn install_canonical_descriptor(
        &mut self,
        descriptor: &crate::observation::transcript::RuntimeClientMessageDescriptor,
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
            > super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
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

    pub(super) fn fold_canonical_header(
        &mut self,
        attempt_id: Option<AttemptId>,
        header: crate::durable::observation::MessageHeader,
        transcript_cursor: Option<crate::durable::TranscriptCursor>,
        owns_header: bool,
    ) -> Vec<RuntimeClientEvent> {
        let mut events = Vec::new();
        let Some(mut descriptor) =
            crate::observation::transcript::RuntimeClientMessageDescriptor::from_header(&header)
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

    pub(super) fn rebind_canonical_foreground(
        &mut self,
        header: &crate::durable::observation::MessageHeader,
        events: &mut Vec<RuntimeClientEvent>,
    ) {
        if header.role == "assistant" {
            if let Some(locator) = header.locator.clone() {
                self.canonical_foreground =
                    Some(crate::observation::cut::CanonicalForegroundReadCut {
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
                            crate::observation::content::ContentSource::CanonicalMessageBlock {
                                component: crate::observation::content::CanonicalContentComponent::BlockMetadata,
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
                                    crate::observation::content::ContentRepresentation::CanonicalJsonV1.key(),
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

    pub(super) fn observe_audit_dependencies(
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

    pub(super) fn fold_audit_receipt(
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
        let settlement_record =
            crate::observation::transcript::RuntimeClientPublicationSettlementRecord {
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
            let crate::observation::transcript::RuntimeClientTranscriptItem::PublicationAudit {
                audit,
            } = entry.item
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
    pub(super) fn append_text(
        &mut self,
        block_index: ContentBlockIndex,
        delta: &str,
        kind: TextKind,
    ) {
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
                    crate::observation::content::ContentRepresentation::Utf8Text.key(),
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
    pub(super) fn append_arguments(&mut self, block_index: ContentBlockIndex, delta: &str) {
        let expected = self
            .publication_start
            .as_ref()
            .and_then(|start| {
                self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    block_index,
                    crate::observation::content::ContentRepresentation::RawJsonUtf8.key(),
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

    pub(super) fn set_assembled(&mut self, block_index: ContentBlockIndex) {
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
    pub(super) fn settle_foreground(
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
}
