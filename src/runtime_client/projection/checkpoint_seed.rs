//! The immutable checkpoint seed captured under the projection boundary and
//! materialized into a bounded checkpoint after the lock is released.
use super::{
    Arc, CheckpointSeed, ForegroundToolExecution, ForegroundToolState, InFlightBlock, MessageBlock,
    RuntimeClientContextView, RuntimeClientCursor, RuntimeClientError, RuntimeClientSnapshot,
    bounded_diagnostic, bounded_outcome, defer_result_content, deferred_live_block,
    inline_block_bytes,
};

impl CheckpointSeed {
    pub(super) fn checkpoint_read_cut(&self) -> crate::observation::cut::CheckpointReadCut {
        self.read_cut.clone()
    }
    pub(crate) fn retain_foreground_progress(
        &self,
        page: &mut super::super::checkpoint::CheckpointPage,
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
            let mut source: ForegroundToolExecution =
                crate::observation::bounded::decode_domain_value(value).map_err(|_| {
                    RuntimeClientError::ObservationUnavailable {
                        message: "native foreground source header unavailable".into(),
                    }
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
            item.value = Some(
                crate::observation::bounded::structural_value(&source).ok_or_else(|| {
                    RuntimeClientError::ObservationUnavailable {
                        message: "finite foreground source header unavailable".into(),
                    }
                })?,
            );
        }
        Ok(())
    }

    pub(super) fn domain_revision(&self, domain: &str) -> u64 {
        let owner = domain.split('.').next().unwrap_or(domain);
        self.domain_revisions.get(owner).copied().unwrap_or(0)
    }
    pub(super) fn capture_context(
        &self,
        capture: &mut crate::observation::capture::Capture,
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
    pub(super) fn capture_attempt(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> Option<super::super::snapshot::RuntimeClientAttempt> {
        self.snapshot
            .attempt
            .as_ref()
            .map(|attempt| super::super::snapshot::RuntimeClientAttempt {
                attempt_id: attempt.attempt_id.clone(),
                attempt_id_source: attempt.attempt_id_source.clone(),
                phase: match &attempt.phase {
                    super::super::snapshot::RuntimeClientAttemptPhase::Incomplete => {
                        super::super::snapshot::RuntimeClientAttemptPhase::Incomplete
                    }
                    super::super::snapshot::RuntimeClientAttemptPhase::Admitted => {
                        super::super::snapshot::RuntimeClientAttemptPhase::Admitted
                    }
                    super::super::snapshot::RuntimeClientAttemptPhase::Running => {
                        super::super::snapshot::RuntimeClientAttemptPhase::Running
                    }
                    super::super::snapshot::RuntimeClientAttemptPhase::Settled { outcome } => {
                        super::super::snapshot::RuntimeClientAttemptPhase::Settled {
                            outcome: bounded_outcome(outcome),
                        }
                    }
                },
                turn: attempt.turn,
                last_usage: attempt.last_usage.clone(),
                in_flight: attempt.in_flight.as_ref().map(|flight| {
                    super::super::snapshot::InFlightAssistantMessage {
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

    /// Finite capture at the native semantic cut. No Store work or awaiting is
    /// permitted here; every inline clone is independently byte/count capped.
    pub(crate) fn checkpoint_candidate(
        &self,
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let (mut checkpoint, _) = self.checkpoint_unfitted()?;
        super::super::checkpoint::fit_checkpoint(&mut checkpoint)?;
        Ok(checkpoint)
    }

    pub(super) fn checkpoint_unfitted(
        &self,
    ) -> Result<(super::super::checkpoint::RuntimeClientCheckpoint, bool), RuntimeClientError> {
        use super::super::checkpoint::RuntimeClientCheckpoint;
        use crate::observation::capture::Capture;
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
                super::super::snapshot::RuntimeDurabilityFailure {
                    operation: bounded_diagnostic(&failure.operation),
                    diagnostic: bounded_diagnostic(&failure.diagnostic),
                }
            }),
            messages: capture.message_page("messages", &source.messages, &source.conversation_id),
            message_descriptors: self.capture_message_descriptors(&mut capture),
            publication_settlements: source.publication_settlements.clone(),
            transcript: self.capture_transcript(&mut capture),
            trace: crate::observation::trace::TracePage {
                read_cut: source.trace.read_cut.clone(),
                records: self.trace_head.capture(&source.trace.records, &mut capture),
                updates: capture.page("trace.updates", &source.trace.updates),
                next_cursor: source.trace.next_cursor.clone(),
            },
            trace_updates: capture.page("trace_updates", &source.trace_updates),
            attempt,
            inbound: super::super::snapshot::InboundDiagnostics {
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
            resources: crate::observation::resource_source::RuntimeClientResourcesView {
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
        self.capture_evidence(&snapshot, &mut capture);
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

    pub(super) fn capture_capabilities(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> crate::observation::capability_source::CapabilityView {
        self.capability_head.capture(capture)
    }

    pub(super) fn capture_message_descriptors(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> Vec<crate::observation::transcript::RuntimeClientMessageDescriptor> {
        self.snapshot
            .message_descriptors
            .iter()
            .map(|source| {
                let mut descriptor = source.clone();
                while crate::observation::bounded::bounded_bytes(&descriptor, 16_384).is_none() {
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

    pub(super) fn capture_transcript(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> crate::observation::transcript::RuntimeClientTranscriptPage {
        let source = &self.snapshot;
        crate::observation::transcript::RuntimeClientTranscriptPage {
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

    pub(super) fn capture_evidence(
        &self,
        snapshot: &RuntimeClientSnapshot,
        capture: &mut crate::observation::capture::Capture,
    ) {
        for (index, message) in snapshot.messages.iter().enumerate() {
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
        for binding in &snapshot.transcript.content {
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

    pub(super) fn capture_collection_frontiers(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) {
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

    pub(super) fn capture_foreground(
        &self,
        capture: &mut crate::observation::capture::Capture,
        foreground: &[ForegroundToolExecution],
    ) -> Vec<ForegroundToolExecution> {
        let head: Vec<_> = foreground
            .iter()
            .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
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

    pub(super) fn capture_foreground_evidence(
        &self,
        capture: &mut crate::observation::capture::Capture,
        head: &[ForegroundToolExecution],
    ) {
        for (index, tool) in head.iter().enumerate() {
            let live_arguments = self.publication_start.as_ref().and_then(|start| {
                self.live_frontiers.get(&(
                    start.stream_id.clone(),
                    tool.block_index,
                    crate::observation::content::ContentRepresentation::RawJsonUtf8.key(),
                    "live",
                ))
            });
            let canonical_arguments = self
                .content_metadata
                .get(&(
                    "canonical",
                    tool.occurrence.locator.key().into_owned(),
                    tool.block_index.get(),
                    crate::observation::content::ContentRepresentation::RawJsonUtf8.key(),
                ))
                .map(Arc::as_ref);
            if let Some(reference) = live_arguments.or(canonical_arguments) {
                let arguments = match &tool.state {
                    ForegroundToolState::Assembled { arguments }
                    | ForegroundToolState::Running { arguments, .. }
                    | ForegroundToolState::Settled { arguments, .. } => arguments,
                };
                capture
                    .content
                    .push(crate::observation::cut::ContentBinding {
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
                capture
                    .content
                    .push(crate::observation::cut::ContentBinding {
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

    pub(super) fn capture_live_blocks(
        &self,
        capture: &mut crate::observation::capture::Capture,
        blocks: &[InFlightBlock],
    ) -> Vec<InFlightBlock> {
        let mut result = Vec::new();
        for (index, block) in blocks
            .iter()
            .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
            .enumerate()
        {
            let (block_index, representation, field) = match block {
                InFlightBlock::Text { block_index, .. }
                | InFlightBlock::Reasoning { block_index, .. }
                | InFlightBlock::Refusal { block_index, .. } => (
                    *block_index,
                    crate::observation::content::ContentRepresentation::Utf8Text,
                    "text",
                ),
                InFlightBlock::ToolCall { block_index, .. } => (
                    *block_index,
                    crate::observation::content::ContentRepresentation::RawJsonUtf8,
                    "arguments",
                ),
            };
            let copied = crate::observation::bounded::bounded_clone(
                block,
                crate::observation::bounded::CHECKPOINT_ITEM_BYTES,
            );
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
                                    crate::observation::content::ContentRepresentation::CanonicalJsonV1.key(),
                                    "live",
                                ))
                            })
                            .flatten()
                    });
                if let Some(reference) = reference {
                    capture
                        .content
                        .push(crate::observation::cut::ContentBinding {
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
                    crate::observation::content::ContentRepresentation::CanonicalJsonV1.key(),
                    "live_block_metadata",
                )) {
                    capture
                        .content
                        .push(crate::observation::cut::ContentBinding {
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

    pub(super) fn resolve_captured_content(
        &self,
        bindings: &mut [crate::observation::cut::ContentBinding],
    ) {
        for binding in bindings {
            if let crate::observation::content::ContentSource::CanonicalMessageBlock {
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
                        crate::observation::content::ContentRepresentation::RawJsonUtf8.key(),
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
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        use super::super::checkpoint::page;
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
                |_item: &super::super::snapshot::RuntimeClientContextView| "context".into()
            ),
            "capabilities" => list!(
                std::slice::from_ref(&source.capabilities),
                |_item: &crate::observation::capability_source::CapabilityView| "capabilities"
                    .into()
            ),
            "goal" => list!(
                &source.goal.iter().collect::<Vec<_>>(),
                |_item: &&crate::observation::goal::RuntimeClientGoalView| "goal".into()
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
                |entry: &crate::observation::transcript::RuntimeClientTranscriptEntry| entry
                    .cursor
                    .get()
                    .to_string()
            ),
            "pending_interactions" => self.checkpoint_pending_page(cut, domain, offset, limit),
            "agents" => list!(
                &source.agents,
                |agent: &super::super::snapshot::RuntimeClientAgent| agent.agent_id.to_string()
            ),
            "jobs" => list!(
                &source.jobs,
                |job: &super::super::snapshot::RuntimeClientJob| job.job_id.to_string()
            ),
            "statuses" => list!(
                &source.statuses,
                |status: &crate::observation::status::AgentStatusView| status
                    .source_locator
                    .as_ref()
                    .map(|locator| locator.key().into_owned())
                    .or_else(|| status.status_message_id.as_ref().map(ToString::to_string))
                    .unwrap_or_default()
            ),
            "inbound.pending" => list!(
                &source.inbound.pending,
                |item: &super::super::snapshot::InboundItemView| item.message.id.to_string()
            ),
            "attempt.in_flight.blocks" | "attempt.foreground" => {
                self.checkpoint_attempt_page(cut, domain, offset, limit)
            }
            _ => self.checkpoint_auxiliary_page(cut, domain, offset, limit),
        }
    }
    pub(super) fn checkpoint_attempt_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        macro_rules! list {
            ($items:expr, $identity:expr) => {
                super::super::checkpoint::page(
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

    pub(super) fn checkpoint_messages_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        let mut result = super::super::checkpoint::page(
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
                let mut capture = crate::observation::capture::Capture::new(self.cursor.get());
                let message = capture.message(
                    &source.messages[usize::try_from(item.offset).expect("validated page offset")],
                    &source.conversation_id,
                    &[],
                );
                item.value = crate::observation::bounded::bounded_bytes(
                    &message,
                    crate::observation::bounded::CHECKPOINT_ITEM_BYTES,
                )
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                item.content = capture.content;
            }
        }
        Ok(result)
    }

    pub(super) fn checkpoint_pending_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let source = &self.snapshot;
        let mut result = super::super::checkpoint::page(
            domain,
            cut.clone(),
            RuntimeClientCursor::new(cut.revision),
            &source.pending_interactions,
            offset,
            limit,
            |interaction: &crate::observation::interaction::RuntimeClientInteractionHeader| {
                interaction.interaction.to_string()
            },
        )?;
        for item in &mut result.items {
            let header = &source.pending_interactions
                [usize::try_from(item.offset).expect("validated interaction offset")];
            item.body_complete = Some(header.body_complete);
            item.deferred = !header.body_complete;
            if header.kind
                == crate::observation::interaction::RuntimeClientInteractionKind::Approval
            {
                result
                    .collections
                    .push(crate::observation::cut::CollectionFrontier {
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

    pub(super) fn checkpoint_auxiliary_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        use super::super::checkpoint::page;
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
                |tool: &crate::observation::capability_source::RuntimeClientTool| tool
                    .id
                    .to_string()
            ),
            "capabilities.configured_tools" => list!(
                &source.capabilities.configured_tools,
                |tool: &crate::observation::capability_source::RuntimeClientTool| tool
                    .id
                    .to_string()
            ),
            "capabilities.available_tools" => list!(
                &source.capabilities.available_tools,
                |tool: &crate::observation::capability_source::RuntimeClientTool| tool
                    .id
                    .to_string()
            ),
            "capabilities.skills" => list!(
                &source.capabilities.skills,
                |skill: &crate::observation::capability_source::RuntimeClientSkill| skill
                    .id
                    .to_string()
            ),
            "capabilities.sources" => list!(
                &source.capabilities.sources,
                |_: &crate::observation::capability_source::CapabilitySourceView| String::from(
                    "source"
                )
            ),
            "resources.context_files" => list!(
                &source.resources.context_files,
                |file: &crate::observation::resource_source::RuntimeClientContextFile| {
                    bounded_diagnostic(&file.path)
                }
            ),
            "workflows.runs" => list!(
                &source.workflows.runs,
                |run: &crate::runtime::workflow::read_model::WorkflowRunView| format!(
                    "{}:{}:{}",
                    run.id.conversation_id, run.id.origin, run.id.invocation
                )
            ),
            _ => Err(RuntimeClientError::InvalidRequest {
                message: "unknown checkpoint collection domain".into(),
            }),
        }
    }
}
