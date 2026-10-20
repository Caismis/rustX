//! Projection construction, bootstrap from the native durable cut, and
//! activation binding. Bootstrap seeds the read model once; it never replays history.
#[cfg(test)]
use super::AttemptId;
#[cfg(test)]
use super::ContentBlockIndex;
#[cfg(test)]
use super::ForegroundToolExecution;
#[cfg(test)]
use super::ForegroundToolState;
#[cfg(test)]
use super::in_flight::inline_label;
use super::{
    ApprovalMode, Arc, CapabilityView, ContentMetadata, ConversationId, DomainSources,
    ForegroundResultContent, HashMap, InboundDiagnostics, MessageBlock, NativePublicationState,
    ObservationSourceState, PreparedDurableToolReading, RuntimeClientAttempt,
    RuntimeClientContextView, RuntimeClientCursor, RuntimeClientProjection, RuntimeClientSnapshot,
    SessionModelView, VecDeque, agent_view, background_view, inbound_item_view, stamp_trace_cut,
    upsert_background,
};

pub(super) fn initial_client_snapshot(
    conversation_id: ConversationId,
    initial_messages: Vec<MessageBlock>,
    initial_capabilities: CapabilityView,
    initial_model: Option<SessionModelView>,
) -> RuntimeClientSnapshot {
    RuntimeClientSnapshot {
        settings_evidence: super::super::settings::SettingsEvidence::LiveSession,
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
            crate::runtime::adoption::AdoptionEligibility::Unavailable,
        durability_failure: None,
        messages: initial_messages,
        message_descriptors: Vec::new(),
        publication_settlements: Vec::new(),
        transcript: crate::observation::transcript::RuntimeClientTranscriptPage::default(),
        trace: crate::observation::trace::TracePage::default(),
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
        resources: crate::observation::resource_source::RuntimeClientResourcesView::default(),
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
            trace_head: super::super::checkpoint::PreparedTraceHead::default(),
            capability_head: crate::observation::capability_source::PreparedCapabilityHead::new(
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
    pub(crate) fn set_settings_evidence(
        &mut self,
        evidence: super::super::settings::SettingsEvidence,
    ) {
        self.snapshot.settings_evidence = evidence;
    }

    /// Installs the frozen effective native Agent Extension composition of
    /// the attached Agent runtime (Issue #256).
    ///
    /// The host calls this once, at construction, from
    /// [`ConversationRuntime::native_extensions`](crate::runtime::ConversationRuntime::native_extensions).
    /// There is no later mutation seam: the projected composition changes
    /// only when the runtime behind the projection is a different one.
    pub(crate) fn set_effective_plugins(
        &mut self,
        extensions: super::super::settings::EffectivePlugins,
    ) {
        self.snapshot.effective_plugins = Some(extensions);
    }

    pub(super) fn bootstrap_agents(
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
        self.snapshot.transcript =
            crate::observation::transcript::transcript_page_view(seed.transcript.clone())
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
        context: super::super::snapshot::RuntimeClientContextView,
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
                occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: assistant.id.clone(),
                    },
                    block_index: ContentBlockIndex::new(
                        u32::try_from(index).expect("canonical block index"),
                    ),
                },
                message_id: crate::observation::tool::inline_message_id(&assistant.id),
                block_index: ContentBlockIndex::new(
                    u32::try_from(index).expect("canonical block index"),
                ),
                call_id: crate::observation::tool::inline_call_id(&call.id),
                metadata_ref: None,
                assembly_complete: true,
                tool_id: crate::observation::tool::inline_tool_id(&call.tool_id),
                name: inline_label(&call.name),
                state: ForegroundToolState::Assembled {
                    arguments: String::new(),
                },
            });
        }
    }

    pub(super) fn rebuild_pending_head(&mut self) {
        let mut head: Vec<_> = self.pending_heads.values().flatten().cloned().collect();
        head.sort_by_key(|header| header.source_offset);
        head.truncate(32);
        self.snapshot.pending_interactions = head;
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
            self.replay_bytes += crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
        }
        self.published_seed = Some(seed);
        self.published_sources = Some(self.domain_sources.clone());
    }
}
