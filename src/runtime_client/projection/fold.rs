//! Folding of one admitted observation into the read model and its client events.
#[cfg(test)]
use super::ForegroundSettlement;
#[cfg(test)]
use super::InboundDrainView;
#[cfg(test)]
use super::MessageBlock;
#[cfg(test)]
use super::RuntimeEvent;
#[cfg(test)]
use super::views::inbound_item_view;
use super::{
    Arc, AttemptId, ContentBlockIndex, ConversationObservation, ForegroundResultContent,
    ForegroundToolExecution, ForegroundToolState, InFlightAssistantMessage, InFlightBlock,
    PublicationFrame, PublicationPayload, RuntimeClientAttempt, RuntimeClientAttemptPhase,
    RuntimeClientError, RuntimeClientEvent, RuntimeClientProjection, RuntimeClientTranscriptCursor,
    TextKind, activation_view, admit_status, agent_view, arguments_of, background_view,
    inline_label, push_in_flight_block, update_stored_tool_slots, upsert_background,
    upsert_interaction, upsert_subagent,
};

impl RuntimeClientProjection {
    /// Folds one observation into the read model and returns the external
    /// events the observation publishes.
    #[allow(clippy::too_many_lines)]
    pub(super) fn fold(&mut self, observation: ConversationObservation) -> Vec<RuntimeClientEvent> {
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
                    let value = Arc::new(super::super::domain::SourceValue::new(Arc::new(source)));
                    let source = Arc::new(super::super::domain::DomainSource::Workflow(value));
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
                let occurrence = crate::observation::tool::RuntimeClientToolOccurrence {
                    locator:
                        crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                            position: tool.occurrence.ledger_position,
                        },
                    block_index: tool.occurrence.block_index,
                };
                update_stored_tool_slots(&mut self.snapshot, &occurrence, |slot| {
                    let mut result = tool.result.clone();
                    crate::observation::tool::defer_result(&mut result);
                    slot.committed_result = Some(result.clone());
                    slot.committed_result_deferred = tool.result_deferred
                        || crate::observation::tool::has_result_body(&tool.result);
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
                    frontier: crate::observation::cut::CollectionFrontier {
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
                    Some(super::super::snapshot::RuntimeClientManualCompaction {
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
                        let mut capture =
                            crate::observation::capture::Capture::new(self.cursor.get());
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
                        tool_call_id: crate::observation::tool::inline_call_id(&tool.tool_call_id),
                        tool_id: crate::observation::tool::inline_tool_id(&tool.tool_id),
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
                                Some(crate::observation::cut::CanonicalForegroundReadCut {
                                    locator:
                                        crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
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
                                        self.native_tool_occurrences.insert(call.id.clone(),crate::observation::tool::RuntimeClientToolOccurrence {locator:crate::observation::content::CanonicalContentLocator::ByLedgerPosition {position},block_index:slot.block_index});
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
                                    crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                                        position,
                                    };
                                slot.metadata_ref=self.content_metadata.get(&("canonical_block_metadata",format!("ledger:{position}"),slot.block_index.get(),crate::observation::content::ContentRepresentation::CanonicalJsonV1.key())).map(Arc::as_ref).cloned().or_else(||header.content.iter().find_map(|binding|match &binding.content_ref.source {
                                    crate::observation::content::ContentSource::CanonicalMessageBlock {component:crate::observation::content::CanonicalContentComponent::BlockMetadata,block_index,..} if *block_index==slot.block_index=>Some(binding.content_ref.clone()),
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

                let descriptor = header.as_ref().and_then(
                    crate::observation::transcript::RuntimeClientMessageDescriptor::from_header,
                );
                let descriptor_only = header
                    .as_ref()
                    .is_some_and(|header| header.display.is_none());
                if let Some(descriptor) = descriptor.as_ref() {
                    self.snapshot.message_descriptors.push(descriptor.clone());
                    if self.snapshot.message_descriptors.len()
                        > super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
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
                        >= super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS
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
                        .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
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
                        .truncate(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS);
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
                    let audit_view =
                        crate::observation::transcript::interaction_requested_view(audit)
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
                    let audit_view =
                        crate::observation::transcript::interaction_settled_view(audit)
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
                    tool_call_id: crate::observation::tool::inline_call_id(&tool_call_id),
                    tool_id: crate::observation::tool::inline_tool_id(&tool_id),
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
                self.install_source_frontiers(vec![crate::observation::cut::CollectionFrontier {
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
                let plugins = snapshot.root_profile().map(|profile| {
                    super::super::settings::EffectivePlugins::project(&profile.extensions)
                });
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
                self.install_source_frontiers(vec![crate::observation::cut::CollectionFrontier {
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
                    execution_settings: Some(super::super::settings::AdmittedSettings {
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
                    Some(super::super::snapshot::RuntimeDurabilityFailure {
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
    pub(super) fn fold_publication_frame(
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
                    crate::observation::content::ContentRepresentation::Utf8Text,
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
                    crate::observation::content::ContentRepresentation::Utf8Text,
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
                    crate::observation::content::ContentRepresentation::Utf8Text,
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
                    crate::observation::content::ContentRepresentation::RawJsonUtf8,
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

    pub(super) fn fold_proposed_tool_start(
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
            crate::observation::content::PublicationContentComponent::BlockMetadata,
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
                occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: message_id.clone(),
                    },
                    block_index,
                },
                message_id: Some(message_id.clone()),
                block_index,
                call_id: crate::observation::tool::inline_call_id(&call.id),
                metadata_ref: metadata_ref.clone(),
                assembly_complete: false,
                tool_id: crate::observation::tool::inline_tool_id(&call.tool_id),
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
                    call_id: crate::observation::tool::inline_call_id(&call.id),
                    metadata_ref: metadata_ref.clone(),
                    tool_id: crate::observation::tool::inline_tool_id(&call.tool_id),
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
            call: super::super::snapshot::RuntimeClientToolCall {
                id: crate::observation::tool::inline_call_id(&call.id),
                tool_id: crate::observation::tool::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                arguments: None,
                metadata_ref,
            },
        }]
    }

    pub(super) fn fold_proposed_tool_completed(
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
            crate::observation::content::PublicationContentComponent::Payload,
        );
        let metadata_ref = self.observed_structural_content(
            attempt_id,
            frame,
            block_index,
            crate::observation::content::PublicationContentComponent::BlockMetadata,
        );
        let arguments_json = self.live_arguments_prefix(frame, block_index);
        let arguments_ref = self
            .live_frontiers
            .get(&(
                frame.stream_id.clone(),
                block_index,
                crate::observation::content::ContentRepresentation::RawJsonUtf8.key(),
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
            call: super::super::snapshot::RuntimeClientToolCall {
                id: crate::observation::tool::inline_call_id(&call.id),
                tool_id: crate::observation::tool::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                arguments: crate::observation::bounded::bounded_json_value(&call.arguments, 1024)
                    .filter(|value| {
                        !value.is_null() || canonical_length.is_none_or(|length| length == 4)
                    }),
                metadata_ref,
            },
        }]
    }

    pub(super) fn live_arguments_prefix(
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
}
