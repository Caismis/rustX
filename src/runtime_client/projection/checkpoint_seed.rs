//! The immutable checkpoint seed captured under the projection boundary and
//! materialized into a bounded checkpoint after the lock is released.
use super::attempt_view;
use super::prepared_attempt;
use super::prepared_capture;
use super::{
    Arc, CheckpointSeed, ForegroundToolExecution, ForegroundToolState, InFlightBlock, MessageBlock,
    RuntimeClientContextView, RuntimeClientCursor, RuntimeClientError, RuntimeClientSnapshot,
    bounded_diagnostic, bounded_outcome, deferred_live_block, inline_block_bytes,
};
use super::{ReadDomainValues, deferred_foreground_tool, merge_source_frontiers, stamp_trace_cut};

impl CheckpointSeed {
    /// Positive Workflow identities are meaningful only at this published cut.
    /// Selected historic sources never borrow a later seed's activity labels.
    pub(crate) fn live_workflows_at(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
    ) -> Option<&crate::observation::workflow_source::WorkflowReadSource> {
        // The display header can omit instances by count or byte size. Keep
        // the original immutable source captured with this publication instead.
        (self.read_cut == *cut)
            .then_some(self.workflow_source.as_ref())
            .flatten()
    }

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
        let Some(attempt) = self.snapshot.attempt.as_ref() else {
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
    pub(super) fn capture_attempt_view(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> Option<attempt_view::AttemptView<'_>> {
        let prepared = self.prepared_attempt.select(self.snapshot.attempt.owner());
        let mut bindings = Vec::new();
        let result = self.snapshot.attempt.as_ref().map(|attempt| {
            let in_flight = attempt.in_flight.as_ref().map(|flight| {
                let (blocks, fit) = self.capture_live_blocks_view(
                    capture,
                    flight.blocks.iter(),
                    &mut bindings,
                    Some(&prepared),
                );
                attempt_view::InFlightView {
                    message_id: &flight.message_id,
                    blocks,
                    fit,
                }
            });
            let (foreground, foreground_budget) = self.capture_foreground_view(
                capture,
                &attempt.foreground,
                &mut bindings,
                Some(&prepared),
            );
            let mut view = attempt_view::AttemptView {
                fit: None,
                attempt_id: &attempt.attempt_id,
                attempt_id_source: &attempt.attempt_id_source,
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
                last_usage: &attempt.last_usage,
                in_flight,
                foreground,
                model: prepared_attempt::capture_shared(capture, &prepared.model, |capture| {
                    capture
                        .item("attempt.model", attempt.model.value())
                        .flatten()
                }),
                execution_settings: prepared_attempt::capture_shared(
                    capture,
                    &prepared.settings,
                    |capture| {
                        capture
                            .item(
                                "attempt.execution_settings",
                                attempt.execution_settings.value(),
                            )
                            .flatten()
                    },
                ),
            };
            view.fit = view.measure(&prepared, foreground_budget);
            view
        });
        self.attempt_bindings.append(bindings, &mut capture.content);
        result
    }

    #[cfg(test)]
    pub(super) fn capture_attempt(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> Option<super::snapshot::RuntimeClientAttempt> {
        self.capture_attempt_view(capture)
            .map(attempt_view::AttemptView::into_owned)
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
            transcript_rows,
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
        seed.transcript_bindings = transcript.content.clone().into();
        seed.snapshot.transcript = (transcript).into();
        seed.transcript_head = transcript_rows;
        seed.snapshot.context.occupancy = occupancy;
        seed.snapshot.context.projection_head = projection_head;
        seed.snapshot.trace = (trace).into();
        seed.trace_head = super::checkpoint::PreparedTraceHead::default();
        seed.prepared = prepared_capture::PreparedCaptures::default();
        seed.checkpoint_candidate()
    }

    #[cfg(test)]
    pub(super) fn checkpoint_prepared(
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
    pub(super) fn capture_checkpoint(
        &self,
    ) -> Result<
        prepared_capture::CapturedCheckpoint<impl FnOnce() -> RuntimeClientSnapshot + '_>,
        RuntimeClientError,
    > {
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
        // Keep live Goal CAS controls ahead of optional history and display bodies.
        let goal = cached!(goal, |capture| capture
            .item("goal", source.goal.value())
            .flatten());
        // Preserve the shared optional-body admission order, but borrow the
        // normalized bodies until a replacement or a reader actually needs them.
        let context = cached!(context, |capture| self.capture_context(capture));
        let attempt = self.capture_attempt_view(&mut capture);
        budget.field(
            "attempt",
            attempt.as_ref().map_or(Some(4), |value| {
                value.fit.and_then(|budget| budget.checked_add(1))
            }),
        );
        budget.value("settings_evidence", &source.settings_evidence);
        let effective_plugins = cached!(effective_plugins, |capture| capture
            .item("effective_plugins", source.effective_plugins.value())
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
        // A new durable cut can retain exactly the same transcript rows. Keep
        // their normalized admission with the rows, not with the changing page
        // envelope. Preserve the original deferred/statistics/entries order.
        let (transcript_deferred_entries, deferred_budget) =
            self.prepared
                .capture("transcript_deferred_entries", &mut capture, |capture| {
                    capture.page(
                        "transcript.deferred_entries",
                        &source.transcript.deferred_entries,
                    )
                });
        let (transcript_statistics, statistics_budget) =
            self.prepared
                .capture("transcript_statistics", &mut capture, |capture| {
                    capture
                        .item(
                            "transcript.statistics",
                            source.transcript.statistics.value(),
                        )
                        .flatten()
                });
        let (transcript_entries, entries_budget) = self.transcript_head.capture(&mut capture);
        let mut transcript_budget = prepared_capture::FitBudget::new();
        transcript_budget.value("source_cut", &source.transcript.source_cut);
        transcript_budget.field("content", self.transcript_bindings.budget());
        transcript_budget.value("collections", &source.transcript.collections);
        transcript_budget.field("deferred_entries", deferred_budget);
        transcript_budget.value("inherited_through", &source.transcript.inherited_through);
        transcript_budget.field("statistics", statistics_budget);
        transcript_budget.field("entries", entries_budget);
        transcript_budget.value("next_cursor", &source.transcript.next_cursor);
        budget.field("transcript", transcript_budget.finish());
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
        // The captured capability generation is immutable. Retained checkpoint
        // events must share it instead of cloning all tool schemas per cursor.
        let capabilities = cached!(capabilities, |capture| Arc::new(
            self.capture_capabilities(capture)
        ));
        let resources = cached!(resources, |capture| {
            crate::observation::resource_source::RuntimeClientResourcesView {
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
            .item("model", source.model.value())
            .flatten());
        let mut model_status = source.model_status;
        if let Some(status) = &mut model_status {
            status.body_deferred = model.is_none();
        }
        budget.value("model_status", &model_status);
        let todos = cached!(todos, |capture| source.todos.as_ref().map(|todos| {
            crate::tools::todo::TodoSnapshot {
                tasks: capture.page("todos.tasks", &todos.tasks),
                next_id: todos.next_id,
            }
        }));
        self.capture_evidence(&messages, &mut capture);
        let read_cut = self.checkpoint_read_cut();
        let prepared_metadata = self.metadata_arrays.prepare_capture(
            &read_cut,
            capture.content,
            self.metadata_frontiers.prepare(capture.collections),
        );
        let metadata = prepared_metadata.metadata(read_cut);
        let metadata_budget = prepared_metadata.fragment_budget();
        let mut complete_budget = prepared_capture::FitBudget::from_budget(metadata_budget);
        complete_budget.field("snapshot", budget.finish());
        complete_budget.value("cursor", &self.cursor);
        Ok(prepared_capture::CapturedCheckpoint {
            prepared_metadata,
            snapshot: move || {
                #[cfg(test)]
                crate::observation::capture::record_capture_work(4);
                RuntimeClientSnapshot {
                    settings_evidence: source.settings_evidence,
                    effective_plugins: effective_plugins.into_owned(),
                    workflows: workflows.into_owned(),
                    conversation_id: source.conversation_id.clone(),
                    shutting_down: source.shutting_down,
                    effective_approval_mode: source.effective_approval_mode,
                    durability_failure,
                    messages: messages.into_owned(),
                    message_descriptors: message_descriptors.into_owned(),
                    publication_settlements: source.publication_settlements.owned(),
                    transcript: crate::observation::transcript::RuntimeClientTranscriptPage {
                        source_cut: source.transcript.source_cut.clone(),
                        content: source.transcript.content.clone(),
                        collections: source.transcript.collections.clone(),
                        deferred_entries: transcript_deferred_entries.into_owned().into(),
                        inherited_through: source.transcript.inherited_through,
                        statistics: transcript_statistics.into_owned(),
                        entries: transcript_entries.into_entries().into(),
                        next_cursor: source.transcript.next_cursor,
                    },
                    trace: crate::observation::trace::TracePage {
                        read_cut: source.trace.read_cut.clone(),
                        records: trace_records.into_owned(),
                        updates: trace_page_updates.into_owned(),
                        next_cursor: source.trace.next_cursor.clone(),
                    },
                    trace_updates: trace_updates.into_owned(),
                    attempt: attempt.map(attempt_view::AttemptView::into_owned),
                    inbound: inbound.into_owned(),
                    pending_interactions: pending_interactions.into_owned(),
                    jobs: jobs.into_owned(),
                    agents: agents.into_owned(),
                    statuses: statuses.into_owned(),
                    context: context.into_owned(),
                    capabilities: capabilities.into_owned(),
                    resources: resources.into_owned(),
                    model: model.into_owned(),
                    model_status,
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

    /// Finite capture at the native semantic cut. No Store work or awaiting is
    /// permitted here; every inline clone is independently byte/count capped.
    pub(crate) fn checkpoint_candidate(
        &self,
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let captured = self.capture_checkpoint()?;
        let fit_budget = captured.fit_budget;
        let mut checkpoint = captured.materialize();
        super::super::checkpoint::fit_checkpoint_with_budget(&mut checkpoint, fit_budget)?;
        Ok(checkpoint)
    }

    #[cfg(test)]
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
        // Keep the bounded Goal CAS controls ahead of history/display bodies.
        // Shared inline pressure must not make a live Goal appear absent.
        let goal = capture.item("goal", source.goal.value()).flatten();
        let context = self.capture_context(&mut capture);
        let attempt = self.capture_attempt(&mut capture);
        let mut snapshot = RuntimeClientSnapshot {
            settings_evidence: source.settings_evidence,
            effective_plugins: capture
                .item("effective_plugins", source.effective_plugins.value())
                .flatten(),
            workflows: crate::runtime::workflow::read_model::WorkflowSnapshot {
                revision: source.workflows.revision,
                runs: capture.page("workflows.runs", &source.workflows.runs),
                omitted_runs: source.workflows.omitted_runs,
            },
            conversation_id: source.conversation_id.clone(),
            shutting_down: source.shutting_down,
            effective_approval_mode: source.effective_approval_mode,
            durability_failure: source.durability_failure.as_ref().map(|failure| {
                super::super::snapshot::RuntimeDurabilityFailure {
                    operation: bounded_diagnostic(&failure.operation),
                    diagnostic: bounded_diagnostic(&failure.diagnostic),
                }
            }),
            messages: capture.message_page("messages", &source.messages, &source.conversation_id),
            message_descriptors: self.capture_message_descriptors(&mut capture),
            publication_settlements: source.publication_settlements.owned(),
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
            capabilities: Arc::new(self.capture_capabilities(&mut capture)),
            resources: crate::observation::resource_source::RuntimeClientResourcesView {
                revision: source.resources.revision,
                inspection: capture
                    .item("resources.inspection", &source.resources.inspection)
                    .unwrap_or_default(),
                context_files: capture
                    .page("resources.context_files", &source.resources.context_files),
                agent_profile: source.resources.agent_profile,
            },
            model: capture.item("model", source.model.value()).flatten(),
            model_status: source.model_status,
            todos: source
                .todos
                .as_ref()
                .map(|todos| crate::tools::todo::TodoSnapshot {
                    tasks: capture.page("todos.tasks", &todos.tasks),
                    next_id: todos.next_id,
                }),
            goal,
        };
        // The source body may fit on its own but be evicted by this cut's
        // shared capture budget. Advertise the body actually sent, so clients
        // read the model domain before confirming a selection.
        if let Some(status) = &mut snapshot.model_status {
            status.body_deferred = snapshot.model.is_none();
        }
        self.capture_evidence(&snapshot.messages, &mut capture);
        Ok((
            RuntimeClientCheckpoint {
                snapshot,
                cursor: self.cursor,
                read_cut: self.checkpoint_read_cut(),
                content: capture.content.into(),
                collections: Arc::new(capture.collections.into_iter().map(Into::into).collect()),
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
    ) -> Vec<Arc<crate::observation::transcript::RuntimeClientMessageDescriptor>> {
        self.snapshot
            .message_descriptors
            .iter()
            .map(|source| {
                let mut descriptor = Arc::clone(source);
                while crate::observation::bounded::bounded_len(&descriptor, 16_384).is_none() {
                    let Some(removed) = Arc::make_mut(&mut descriptor).tool_calls.pop() else {
                        break;
                    };
                    capture.evicted = true;
                    let loaded = descriptor.tool_calls.len();
                    Arc::make_mut(&mut descriptor).content.retain(|binding| {
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
                    if let Some(frontier) = Arc::make_mut(&mut descriptor)
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

    #[cfg(test)]
    pub(super) fn capture_transcript(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) -> crate::observation::transcript::RuntimeClientTranscriptPage {
        let source = &self.snapshot;
        crate::observation::transcript::RuntimeClientTranscriptPage {
            source_cut: source.transcript.source_cut.clone(),
            content: source.transcript.content.clone(),
            collections: source.transcript.collections.clone(),
            deferred_entries: capture
                .page(
                    "transcript.deferred_entries",
                    &source.transcript.deferred_entries,
                )
                .into(),
            inherited_through: source.transcript.inherited_through,
            statistics: capture
                .item(
                    "transcript.statistics",
                    source.transcript.statistics.value(),
                )
                .flatten(),
            entries: capture
                .transcript_page(
                    &source
                        .transcript
                        .entries
                        .iter()
                        .map(|row| row.as_ref().clone())
                        .collect::<Vec<_>>(),
                    &source.conversation_id,
                )
                .into_iter()
                .map(Arc::new)
                .collect::<Vec<_>>()
                .into(),
            next_cursor: source.transcript.next_cursor,
        }
    }

    pub(super) fn capture_evidence(
        &self,
        messages: &[MessageBlock],
        capture: &mut crate::observation::capture::Capture,
    ) {
        // Index only borrowed paths. Preserve existing duplicates and the first
        // incoming owner at each path; sorting happens after shared admission.
        let heads = messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                self.head_bindings
                    .get(message.id())
                    .map(|bindings| bindings.message(index))
            })
            .chain(std::iter::once(self.transcript_bindings.transcript()))
            .collect::<Vec<_>>();
        if heads.iter().any(|head| !head.is_empty()) {
            self.metadata_arrays
                .append_evidence(&mut capture.content, heads);
        }
        merge_source_frontiers(&self.source_frontiers, &mut capture.collections);
        self.capture_collection_frontiers(capture);
    }

    pub(super) fn capture_collection_frontiers(
        &self,
        capture: &mut crate::observation::capture::Capture,
    ) {
        self.resolve_captured_content(&mut capture.content);
        for frontier in &mut capture.collections {
            frontier.revision = self.domain_revision(&frontier.domain);
            if let Some(cut) = self.collection_cuts.get(frontier.domain.as_str()) {
                frontier.total = cut.total;
                frontier.next_offset =
                    (frontier.loaded < frontier.total).then_some(frontier.loaded);
            }
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
            if let Some(cut) = &self.source_cut {
                match frontier.domain.as_str() {
                    "messages" => {
                        frontier.total = cut.ledger_through;
                        frontier.next_offset = (frontier.total > frontier.loaded).then_some(0);
                    }
                    "inbound.pending" => {
                        frontier.total = cut.read.pending_total;
                        frontier.next_offset = (frontier.total > frontier.loaded).then_some(0);
                    }
                    "transcript.entries" => {
                        frontier.total = cut.transcript_total;
                        frontier.next_offset = (frontier.total > frontier.loaded).then_some(0);
                    }
                    _ => {}
                }
            }
        }
        // Only the first pending domain is overridden, including deliberately
        // duplicated input frontiers. Other per-domain passes affect all rows.
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
    }

    #[cfg(test)]
    pub(super) fn capture_foreground_prepared(
        &self,
        capture: &mut crate::observation::capture::Capture,
        foreground: &[ForegroundToolExecution],
        bindings: &mut Vec<crate::observation::cut::ContentBinding>,
        prepared: Option<&prepared_attempt::Items>,
    ) -> Vec<ForegroundToolExecution> {
        self.capture_foreground_view(capture, foreground, bindings, prepared)
            .0
            .into_iter()
            .map(std::borrow::Cow::into_owned)
            .collect()
    }

    fn capture_foreground_view<'a>(
        &self,
        capture: &mut crate::observation::capture::Capture,
        foreground: &'a [ForegroundToolExecution],
        bindings: &mut Vec<crate::observation::cut::ContentBinding>,
        prepared: Option<&prepared_attempt::Items>,
    ) -> (
        Vec<std::borrow::Cow<'a, ForegroundToolExecution>>,
        Option<usize>,
    ) {
        let head: Vec<_> = foreground
            .iter()
            .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
            .enumerate()
            .map(|(index, tool)| {
                let admitted = if let Some(owner) = prepared.and_then(|p| p.tools.get(index)) {
                    *prepared_attempt::capture(capture, owner, |capture| capture.reserve(tool))
                } else {
                    capture.reserve(tool)
                };
                if admitted {
                    std::borrow::Cow::Borrowed(tool)
                } else {
                    let source = self
                        .foreground_content
                        .iter()
                        .find(|source| source.occurrence == tool.occurrence);
                    std::borrow::Cow::Owned(deferred_foreground_tool(tool, source))
                }
            })
            .collect();
        capture.frontier(
            "attempt.foreground",
            foreground.len() as u64,
            head.len() as u64,
            (head.len() < foreground.len()).then_some(head.len() as u64),
        );
        let mut budget = Some(2usize);
        for (index, value) in head.iter().enumerate() {
            let item = prepared
                .and_then(|items| items.tools.get(index))
                .map(Arc::as_ref);
            budget = budget
                .zip(prepared_attempt::tool_budget(
                    item,
                    value,
                    matches!(value, std::borrow::Cow::Borrowed(_)),
                ))
                .and_then(|(total, part)| total.checked_add(2)?.checked_add(part));
        }
        self.capture_foreground_evidence(
            capture,
            head.iter().map(std::borrow::Cow::as_ref),
            bindings,
        );
        (head, budget)
    }

    #[cfg(test)]
    pub(super) fn capture_foreground(
        &self,
        capture: &mut crate::observation::capture::Capture,
        foreground: &[ForegroundToolExecution],
        bindings: &mut Vec<crate::observation::cut::ContentBinding>,
    ) -> Vec<ForegroundToolExecution> {
        self.capture_foreground_prepared(capture, foreground, bindings, None)
    }

    fn capture_live_blocks_view<'a>(
        &self,
        capture: &mut crate::observation::capture::Capture,
        blocks: impl ExactSizeIterator<Item = &'a InFlightBlock>,
        bindings: &mut Vec<crate::observation::cut::ContentBinding>,
        prepared: Option<&prepared_attempt::Items>,
    ) -> (Vec<std::borrow::Cow<'a, InFlightBlock>>, Option<usize>) {
        let mut result = Vec::new();
        let mut budget = Some(2usize);
        let total = blocks.len();
        for (index, block) in blocks
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
            let copied = if let Some(owner) = prepared.and_then(|p| p.blocks.get(index)) {
                prepared_attempt::live_block_view(capture, owner, block)
            } else {
                capture.live_block(block).map(std::borrow::Cow::Owned)
            };
            let deferred = copied.is_none();
            let bounded =
                copied.unwrap_or_else(|| std::borrow::Cow::Owned(deferred_live_block(block)));
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
                    bindings.push(crate::observation::cut::ContentBinding {
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
                    bindings.push(crate::observation::cut::ContentBinding {
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
            let owner = prepared
                .and_then(|items| items.blocks.get(index))
                .map(Arc::as_ref);
            budget = budget
                .zip(prepared_attempt::block_budget(owner, &bounded, !deferred))
                .and_then(|(total, part)| total.checked_add(2)?.checked_add(part));
            result.push(bounded);
        }
        capture.frontier(
            "attempt.in_flight.blocks",
            total as u64,
            result.len() as u64,
            (result.len() < total).then_some(result.len() as u64),
        );
        (result, budget)
    }

    pub(super) fn capture_foreground_evidence<'a>(
        &self,
        capture: &mut crate::observation::capture::Capture,
        head: impl IntoIterator<Item = &'a ForegroundToolExecution>,
        bindings: &mut Vec<crate::observation::cut::ContentBinding>,
    ) {
        for (index, tool) in head.into_iter().enumerate() {
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
                bindings.push(crate::observation::cut::ContentBinding {
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
                    bindings.push(binding);
                }
            }
            if let Some(reference) = &tool.result_metadata_ref {
                bindings.push(crate::observation::cut::ContentBinding {
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

    pub(super) fn resolve_captured_content(
        &self,
        bindings: &mut [crate::observation::metadata::MetadataFragment<
            crate::observation::cut::ContentBinding,
        >],
    ) {
        for binding in bindings {
            if let crate::observation::content::ContentSource::CanonicalMessageBlock {
                component,
                locator,
                block_index,
            } = &binding.content_ref.source
            {
                let representation = binding.content_ref.representation.key();
                let raw = crate::observation::content::ContentRepresentation::RawJsonUtf8.key();
                let prefer_raw = binding
                    .path
                    .last()
                    .is_some_and(|field| field == "arguments");
                let mut key = (
                    component.key(),
                    locator.key().into_owned(),
                    block_index.get(),
                    if prefer_raw { raw } else { representation },
                );
                let reference = self.content_metadata.get(&key).or_else(|| {
                    if prefer_raw && representation != raw {
                        key.3 = representation;
                        self.content_metadata.get(&key)
                    } else {
                        None
                    }
                });
                if let Some(reference) = reference {
                    *binding = binding.resolve(reference);
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
                std::slice::from_ref(source.context.value()),
                |_item: &super::snapshot::RuntimeClientContextView| "context".into()
            ),
            "capabilities" => list!(
                std::slice::from_ref(source.capabilities.as_ref()),
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
            "transcript.entries" => list!(&source.transcript.entries, |entry: &Arc<
                crate::observation::transcript::RuntimeClientTranscriptEntry,
            >| entry
                .cursor
                .get()
                .to_string()),
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
                    .map_or_else(Vec::new, |flight| flight.blocks.iter().collect::<Vec<_>>())
                    .as_slice(),
                |block: &&InFlightBlock| match *block {
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
                item.content = capture
                    .content
                    .into_iter()
                    .map(crate::observation::metadata::MetadataFragment::into_owned)
                    .collect();
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
