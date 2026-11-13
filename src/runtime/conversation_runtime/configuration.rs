//! Configuration commit: off-side candidate preparation, the staged commit
//! at the runtime's own safe boundary, and independently published units.
//! Authored-configuration composition is the application owner's concern
//! (`local_runtime::configuration`).

use super::{
    Arc, CapabilityCoordinator, ConfigurationCut, ContextBinding, ConversationLifecycleState,
    ConversationObservation, ConversationRuntime, CoordinatorState, DurableOperation,
    PendingRuntimeCatalog, PreparedConfiguration, PreparedRuntimeResources, RuntimeCatalogFailure,
    RuntimeConfiguration, RuntimeInner, RuntimeResourceSnapshot, SessionModelConfig,
    SessionModelState, validate_model_context,
};
use crate::runtime::configuration_commit::{
    ConfigurationChange, ConfigurationContent, StagedConfiguration, StagedOutcome,
};

impl RuntimeInner {
    fn complete_runtime_catalog_observation(&self, state: &mut CoordinatorState, observed: u64) {
        // This candidate's observation cut belongs to its newly adopted
        // physical authority; an older generation's cursor cannot cover it.
        state.catalog_revision = observed;
        if self.capability.catalog_revision() > state.catalog_revision {
            // A committed older capture cannot consume a later live result.
            self.wake.eligibility.notify_one();
        } else if std::mem::take(&mut state.catalog_admission_deferred) {
            // Resume only the admission pass that previously reached this cut.
            self.wake.notify.notify_one();
        }
        #[cfg(test)]
        self.runtime_catalog_processed
            .send_replace(state.catalog_revision);
    }

    /// An older candidate cannot consume a distinct, later successful result.
    /// The existing worker recaptures that ready fact; no source I/O is retried.
    fn defer_newer_catalog_observation(&self, captured: u64) -> bool {
        if self.capability.catalog_revision() > captured {
            self.wake.eligibility.notify_one();
            true
        } else {
            false
        }
    }

    /// A live result and next Attempt capture share this native lock. A result
    /// observed during an Attempt stays pending until that whole Attempt settles.
    #[allow(clippy::too_many_lines)] // One native capture, validation and capability commit boundary.
    pub(super) fn publish_runtime_catalogs_locked(&self, state: &mut CoordinatorState) {
        state.catalog_commit_blocked = false;
        let Some(prepare) = self.runtime_catalog_preparer.get() else {
            return;
        };
        let observed = self.capability.catalog_revision();
        if observed == state.catalog_revision && state.pending_runtime_catalog.is_none() {
            if state.catalog_admission_deferred {
                self.complete_runtime_catalog_observation(state, observed);
            }
            return;
        }
        // Detached executions and children retain their frozen tools/specs and
        // physical leases. They do not own publication of future runtime facts.
        if !self.lifecycle.is_running()
            || state.current_attempt.is_some()
            || state.manual_compaction.is_some()
        {
            return;
        }
        let (captured, authority, refresh_sequence, candidate) =
            match state.pending_runtime_catalog.take() {
                Some(pending) => (
                    pending.catalog_revision,
                    pending.authority,
                    pending.refresh_sequence,
                    Ok(Some(pending.candidate)),
                ),
                None => (
                    observed,
                    self.capability.runtime_refresh_authority(),
                    None,
                    self.capability
                        .capture_runtime_catalogs()
                        .map_err(|error| error.to_string()),
                ),
            };
        let candidate = match candidate {
            Ok(Some(mut candidate)) => match self
                .capability
                .rebase_runtime_refresh(&authority, &mut candidate)
            {
                Ok(true) => Ok(Some(candidate)),
                Ok(false) => {
                    // The old source authority cannot consume observations
                    // completed under the current adopted binding.
                    self.wake.eligibility.notify_one();
                    return;
                }
                Err(error) => Err(error.to_string()),
            },
            other => other,
        };
        let prepared = match candidate.and_then(|candidate| {
            candidate
                .map(|candidate| prepare(candidate, &state.resources))
                .transpose()
        }) {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                self.complete_runtime_catalog_observation(state, observed);
                return;
            }
            Err(diagnostic) => {
                self.publish_runtime_catalog_failure(
                    state, &authority, captured, observed, diagnostic,
                );
                return;
            }
        };
        let (candidate, resources) = prepared.into_parts();
        let mut candidate = Some(candidate);
        let committed = match self
            .capability
            .try_commit_runtime(&self.capability_publication, &mut candidate)
        {
            Ok(committed) => committed,
            Err(crate::capabilities::CapabilityCommitError::Busy) => {
                state.pending_runtime_catalog = candidate.map(|candidate| PendingRuntimeCatalog {
                    catalog_revision: captured,
                    authority,
                    refresh_sequence,
                    candidate,
                });
                state.catalog_commit_blocked = true;
                #[cfg(test)]
                self.runtime_catalog_blocked.notify_one();
                return;
            }
            Err(error) => {
                self.publish_runtime_catalog_failure(
                    state,
                    &authority,
                    captured,
                    observed,
                    error.to_string(),
                );
                return;
            }
        };
        self.complete_runtime_catalog_observation(state, captured);
        self.publish_committed_runtime_catalog(
            state,
            resources,
            committed.snapshot,
            committed.availability,
            refresh_sequence,
        );
    }

    fn publish_runtime_catalog_failure(
        &self,
        state: &mut CoordinatorState,
        authority: &crate::capabilities::RuntimeRefreshAuthority,
        captured: u64,
        observed: u64,
        diagnostic: String,
    ) {
        if self.defer_newer_catalog_observation(captured) {
            return;
        }
        self.complete_runtime_catalog_observation(state, observed);
        if state
            .runtime_catalog_failure
            .as_ref()
            .is_none_or(|failure| failure.refresh_sequence.is_none())
        {
            state.runtime_catalog_failure = Some(RuntimeCatalogFailure {
                authority: authority.clone(),
                refresh_sequence: None,
            });
            state.resources = Arc::new(state.resources.with_runtime_catalog_failure(diagnostic));
            self.observe_resources(state, state.resources.capability_availability().clone());
        }
    }

    fn publish_committed_runtime_catalog(
        &self,
        state: &mut CoordinatorState,
        resources: crate::runtime::resources::PreparedRuntimeResourceData,
        snapshot: Arc<crate::capabilities::CapabilitySnapshot>,
        availability: crate::capabilities::CapabilityAvailability,
        refresh_sequence: Option<u64>,
    ) {
        let failure_reason = state
            .resources
            .inspection()
            .resource_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.field == "runtime.catalog")
            .map(|diagnostic| diagnostic.reason.clone());
        let may_clear_failure = state
            .runtime_catalog_failure
            .as_ref()
            .is_none_or(|failure| {
                failure.refresh_sequence.is_none_or(|failed_sequence| {
                    refresh_sequence.is_some_and(|sequence| sequence > failed_sequence)
                })
            });
        let clears_failure = failure_reason.is_some() && may_clear_failure;
        if may_clear_failure {
            state.runtime_catalog_failure = None;
        } else if let Some(failure) = &mut state.runtime_catalog_failure {
            failure.authority = self
                .capability
                .reanchor_runtime_refresh_authority(&failure.authority);
        }
        if snapshot.revision() == state.resources.capability().revision()
            && availability == *state.resources.capability_availability()
            && !clears_failure
        {
            return;
        }
        let resources = RuntimeResourceSnapshot::from_prepared(
            state.resources.revision().next(),
            resources,
            snapshot,
        );
        let resources = if !may_clear_failure && let Some(reason) = failure_reason {
            resources.with_runtime_catalog_failure(reason)
        } else {
            resources
        };
        state.resources = Arc::new(resources);
        state.binding_revision = state
            .binding_revision
            .checked_add(1)
            .expect("Session binding revision exhausted");
        self.observe_resources(state, availability);
        self.runtime_catalog_publication
            .send_replace(state.binding_revision);
    }

    /// Publishes the adopted generation, Session model and approval mode of
    /// `state` as one resource observation. Called inside the coordinator
    /// critical section that installed them, so observers see the same cut.
    pub(super) fn observe_resources(
        &self,
        state: &CoordinatorState,
        availability: crate::capabilities::CapabilityAvailability,
    ) {
        let resources = &state.resources;
        let model = state
            .model
            .read_source()
            .at_binding_revision(state.binding_revision);
        self.observe(ConversationObservation::Resources {
            capability: crate::observation::capability_source::CapabilityReadSource::new(
                Arc::clone(resources.capability()),
                availability,
                model.capabilities().cloned(),
            ),
            model,
            approval_mode: state.effective_approval_mode,
            display: crate::observation::resource_source::ResourceReadSource::new(Arc::clone(
                resources,
            )),
            snapshot: Arc::clone(resources),
        });
    }
    /// Whether a real execution owner currently blocks replacing the
    /// committed configuration.
    ///
    /// This is configuration commit safety, deliberately narrower than the
    /// residency idle rule: queued human input, a recovered continuation and
    /// a Goal's authorization for future rounds own no committed resource, so
    /// they never starve a ready candidate. The next Attempt they produce is
    /// admitted after the commit and captures it.
    fn configuration_commit_blocked(&self, state: &CoordinatorState) -> bool {
        self.live_execution_blocks_configuration(state)
            || self.tool_runtime.background().configuration_busy()
            || self
                .subagents
                .as_ref()
                .is_some_and(crate::runtime::subagent::SubagentRegistry::configuration_busy)
    }

    /// Foreground owners that must settle before a staged resource/context
    /// commit publishes. Pure `ExecutionPolicy` / `approval_mode` publication
    /// does not use this predicate: it applies under the coordinator lock
    /// without waiting on background or subagent residency (Issue #544 P13).
    fn live_execution_blocks_configuration(&self, state: &CoordinatorState) -> bool {
        state.current_attempt.is_some()
            || state.scheduled.is_some()
            || state.manual_compaction.is_some()
            || self
                .manual_tail
                .lock()
                .expect("manual tail owner")
                .is_some()
            || self.durability_gate.is_failed()
    }

    /// Report one staged candidate's outcome outside the coordinator lock. A
    /// candidate that did not commit retires its uncommitted physical owners.
    pub(super) fn settle_staged_configuration(
        &self,
        staged: StagedConfiguration,
        outcome: StagedOutcome,
    ) {
        self.report_staged_outcome(staged.prepared.capability, staged.settled, outcome);
    }

    fn report_staged_outcome(
        &self,
        uncommitted: Option<crate::capabilities::PreparedCapabilityCandidate>,
        settled: Box<
            dyn FnOnce(StagedOutcome) -> futures_util::future::BoxFuture<'static, ()> + Send,
        >,
        outcome: StagedOutcome,
    ) {
        let report = settled(outcome);
        self.executor.spawn(async move {
            if let Some(capability) = uncommitted {
                capability.retire_uncommitted().await;
            }
            report.await;
        });
    }

    /// Commit the staged candidate when it is still current and no real owner
    /// blocks it. Called under the coordinator lock at every boundary pass and
    /// before Attempt admission, so commit and admission have one order.
    #[allow(clippy::too_many_lines)] // One validation, publication and evidence boundary.
    pub(super) fn commit_staged_configuration_locked(&self, state: &mut CoordinatorState) {
        state.configuration_commit_waiting = false;
        let Some(staged) = state.staged_configuration.take() else {
            return;
        };
        if !self.lifecycle.is_running()
            || !staged.desired.is_current()
            || state.binding_revision != staged.prepared.baseline
            || !Arc::ptr_eq(&staged.prepared.owner, &self.configuration_owner)
        {
            self.settle_staged_configuration(staged, StagedOutcome::Superseded);
            return;
        }
        if self.configuration_commit_blocked(state) {
            state.staged_configuration = Some(staged);
            return;
        }
        let StagedConfiguration {
            mut prepared,
            desired,
            reason,
            retain,
            settled,
        } = staged;
        let failure_reason = state
            .resources
            .inspection()
            .resource_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.field == "runtime.catalog")
            .map(|diagnostic| diagnostic.reason.clone());
        let revision = state.resources.revision().next();
        let context_only = prepared.capability.is_none();
        let (resources, availability) = if prepared.capability.is_some() {
            let committed = match self
                .capability
                .try_commit_runtime(&self.capability_publication, &mut prepared.capability)
            {
                Ok(committed) => committed,
                Err(crate::capabilities::CapabilityCommitError::Busy) => {
                    // A lifecycle admission is crossing the capability plane;
                    // the worker waits for that exact owner to release it.
                    state.configuration_commit_waiting = true;
                    state.staged_configuration = Some(StagedConfiguration {
                        prepared,
                        desired,
                        reason,
                        retain,
                        settled,
                    });
                    return;
                }
                Err(error) => {
                    self.report_staged_outcome(
                        prepared.capability,
                        settled,
                        StagedOutcome::Failed {
                            diagnostic: error.to_string(),
                        },
                    );
                    return;
                }
            };
            (
                Arc::new(RuntimeResourceSnapshot::from_prepared(
                    revision,
                    prepared
                        .resources
                        .take()
                        .expect("capability resource closure"),
                    committed.snapshot,
                )),
                committed.availability,
            )
        } else {
            match self.capability.try_publish_context(
                &self.capability_publication,
                prepared.preview.capability(),
                &prepared
                    .preview
                    .configuration()
                    .expect("context configuration")
                    .config
                    .agent,
            ) {
                Ok(()) => {}
                Err(crate::capabilities::CapabilityCommitError::Busy) => {
                    state.configuration_commit_waiting = true;
                    state.staged_configuration = Some(StagedConfiguration {
                        prepared,
                        desired,
                        reason,
                        retain,
                        settled,
                    });
                    return;
                }
                Err(error) => {
                    self.report_staged_outcome(
                        None,
                        settled,
                        StagedOutcome::Failed {
                            diagnostic: error.to_string(),
                        },
                    );
                    return;
                }
            }
            (
                Arc::new(prepared.preview.as_ref().clone().with_revision(revision)),
                prepared.preview.capability_availability().clone(),
            )
        };
        let preserves_failure = context_only
            || state
                .runtime_catalog_failure
                .as_ref()
                .is_some_and(|failure| {
                    self.capability
                        .runtime_refresh_is_current(&failure.authority)
                });
        let resources = if preserves_failure && let Some(reason) = failure_reason {
            Arc::new(resources.with_runtime_catalog_failure(reason))
        } else {
            resources
        };
        let previous_binding = state.binding_revision;
        let binding_revision = previous_binding
            .checked_add(1)
            .expect("Session binding revision exhausted");
        let change = ConfigurationChange::between(
            reason,
            (previous_binding, binding_revision),
            (&state.resources, &state.model),
            (&resources, &prepared.model),
        );
        retain(binding_revision);
        if !preserves_failure {
            state.runtime_catalog_failure = None;
        }
        let reopens = state.model.unavailable().is_some() && prepared.model.unavailable().is_none();
        state.model = prepared.model;
        state.effective_approval_mode = resources
            .configuration()
            .map_or(state.effective_approval_mode, |generation| {
                generation.config.approval_mode
            });
        state.resources = resources;
        state.binding_revision = binding_revision;
        // The change record and the generation it describes share this
        // critical section: no Attempt can be admitted between them. A record
        // that cannot be made durable fences admission instead of letting new
        // work run on an unrecorded generation.
        let outcome = self.record_effective_change(state, change);
        self.observe_resources(state, availability);
        if reopens {
            // Work accepted before the selection became unavailable was held,
            // not dropped; this commit is what admits it.
            self.mailbox.wake().notify_one();
        }
        self.report_staged_outcome(None, settled, outcome);
    }

    /// Record the change fact of a generation just installed under the
    /// coordinator lock. No effective change records nothing; a record that
    /// cannot be made durable fences admission and leaves the commit
    /// uncertain, since the generation is already published.
    fn record_effective_change(
        &self,
        state: &mut CoordinatorState,
        change: Option<(ConfigurationChange, ConfigurationContent)>,
    ) -> StagedOutcome {
        let Some((change, content)) = change else {
            return StagedOutcome::Committed;
        };
        match self.record_configuration_change(change, &content) {
            Ok(()) => StagedOutcome::Committed,
            Err(diagnostic) => {
                let _ = self.unrecorded_configuration.set(diagnostic.clone());
                self.record_durability_failure(
                    state,
                    DurableOperation::RecordConfigurationChange,
                    diagnostic.clone(),
                );
                StagedOutcome::Uncertain { diagnostic }
            }
        }
    }

    /// Append the Session-level change fact with its transcript position and
    /// release it to observers in Journal order. It belongs to no Attempt.
    fn record_configuration_change(
        &self,
        change: ConfigurationChange,
        content: &ConfigurationContent,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_mut()
            .is_some_and(|probe| std::mem::take(&mut probe.fail_configuration_record))
        {
            return Err("fault injected: configuration change record".into());
        }
        let event = crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: self.clock.now(),
            event: crate::events::types::RuntimeEvent::ConfigurationApplied { change },
        };
        let (event, cursor) = self
            .store
            .append_configuration_change(event, &content.instructions, &content.catalog)
            .map_err(|error| error.to_string())?;
        // The wrapper releases the fact's transcript position and Journal
        // sequence in order; the inner fact becomes the clients' live row.
        self.observe(ConversationObservation::Published {
            journal_sequence: event.sequence,
            observation: Box::new(ConversationObservation::ConfigurationApplied {
                event: Box::new(event),
                cursor,
            }),
        });
        Ok(())
    }
}

impl ConversationRuntime {
    pub(crate) fn install_runtime_catalog_preparer(
        &self,
        prepare: Arc<crate::runtime::configuration_commit::RuntimeCatalogPreparer>,
    ) {
        assert!(self.inner.runtime_catalog_preparer.set(prepare).is_ok());
    }

    /// A later successful catalog cannot erase a newer explicit failure.
    /// Reconciliation may retry it only under its still-adopted authority.
    pub(crate) fn needs_mcp_refresh(&self) -> bool {
        let state = self.inner.lock_state();
        self.inner.capability.needs_mcp_refresh()
            || state
                .runtime_catalog_failure
                .as_ref()
                .is_some_and(|failure| {
                    failure.refresh_sequence.is_some()
                        && self
                            .inner
                            .capability
                            .runtime_refresh_is_current(&failure.authority)
                })
    }

    /// An explicit retry uses the committed source closure and queues only its
    /// capability result. The eventual idle commit retains whatever model and
    /// context the runtime has adopted by then.
    pub(crate) async fn refresh_runtime_catalogs(&self) -> Result<(), String> {
        let sequence = {
            let mut state = self.inner.lock_state();
            if !self.inner.lifecycle.is_running() {
                return Err("runtime is not running".into());
            }
            state.runtime_refresh_sequence = state
                .runtime_refresh_sequence
                .checked_add(1)
                .expect("runtime refresh sequence exhausted");
            state.runtime_refresh_sequence
        };
        let captured = self.inner.capability.catalog_revision();
        let (authority, outcome) = self.inner.capability.prepare_runtime_refresh().await;
        #[cfg(test)]
        let gate = self
            .inner
            .runtime_refresh_completed_gate
            .lock()
            .expect("refresh gate")
            .take();
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.enter();
        }
        // Completion validation is wholly synchronous. Rejected physical
        // owners are returned out of the native lock before they are retired.
        let completion = (|| {
            let mut state = self.inner.lock_state();
            if !self.inner.lifecycle.is_running() || sequence != state.runtime_refresh_sequence {
                return Err((
                    "runtime refresh was superseded".to_owned(),
                    outcome.ok().map(Box::new),
                ));
            }
            let candidate = match outcome {
                Ok(mut candidate) => {
                    match self
                        .inner
                        .capability
                        .rebase_runtime_refresh(&authority, &mut candidate)
                    {
                        Ok(true) => candidate,
                        result => {
                            return Err((
                                match result {
                                    Err(error) => error.to_string(),
                                    _ => "runtime refresh was superseded".into(),
                                },
                                Some(Box::new(candidate)),
                            ));
                        }
                    }
                }
                Err(error) => {
                    if !self.inner.capability.runtime_refresh_is_current(&authority) {
                        return Err(("runtime refresh was superseded".to_owned(), None));
                    }
                    let diagnostic = format!("last explicit MCP refresh failed: {error}");
                    state.runtime_catalog_failure = Some(RuntimeCatalogFailure {
                        authority: authority.clone(),
                        refresh_sequence: Some(sequence),
                    });
                    state.resources = Arc::new(
                        state
                            .resources
                            .with_runtime_catalog_failure(diagnostic.clone()),
                    );
                    self.inner.observe_resources(
                        &state,
                        state.resources.capability_availability().clone(),
                    );
                    return Err((diagnostic, None));
                }
            };
            let previous = state
                .pending_runtime_catalog
                .replace(PendingRuntimeCatalog {
                    catalog_revision: captured,
                    authority,
                    refresh_sequence: Some(sequence),
                    candidate,
                });
            self.inner.publish_runtime_catalogs_locked(&mut state);
            Ok(previous)
        })();
        let previous = match completion {
            Ok(previous) => previous,
            Err((diagnostic, candidate)) => {
                if let Some(candidate) = candidate {
                    (*candidate).retire_uncommitted().await;
                }
                return Err(diagnostic);
            }
        };
        if let Some(previous) = previous {
            previous.candidate.retire_uncommitted().await;
        }
        self.inner.wake.eligibility.notify_one();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn install_runtime_refresh_completed_gate(&self, gate: Arc<super::Gate>) {
        *self
            .inner
            .runtime_refresh_completed_gate
            .lock()
            .expect("refresh gate") = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn runtime_catalog_blocked(&self) -> tokio::sync::futures::Notified<'_> {
        self.inner.runtime_catalog_blocked.notified()
    }

    #[cfg(test)]
    pub(crate) fn has_pending_runtime_catalog(&self) -> bool {
        self.inner.lock_state().pending_runtime_catalog.is_some()
    }

    #[cfg(test)]
    pub(crate) fn runtime_catalog_observations(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.runtime_catalog_processed.subscribe()
    }

    pub(crate) fn runtime_catalog_publications(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.runtime_catalog_publication.subscribe()
    }

    /// Prepare a source binding for its explicit selection without holding
    /// execution admission. Availability uses the captured default; rebinding
    /// to an existing Session is a separate preparation step.
    ///
    /// The Session binding revision, current generation and model are read
    /// before `prepare` runs, so a candidate prepared across a concurrent
    /// adoption is stale at its own commit boundary. `prepare` is the
    /// application's off-side resource preparation over this runtime's
    /// capability coordinator; it owns its own cancellation.
    pub(crate) async fn prepare_configuration<'c>(
        &'c self,
        prepare: impl FnOnce(
            &'c CapabilityCoordinator,
        ) -> futures_util::future::BoxFuture<
            'c,
            Result<PreparedRuntimeResources, crate::runtime::resources::RuntimeResourceLoadError>,
        >,
        selection: SessionModelConfig,
    ) -> Result<PreparedConfiguration, String> {
        let (baseline, old) = {
            let state = self.inner.lock_state();
            (state.binding_revision, state.resources.clone())
        };
        let prepared = prepare(&self.inner.capability)
            .await
            .map_err(|error| error.message)?;
        let (capability, resources) = prepared.into_parts();
        let generation = resources
            .configuration
            .as_ref()
            .ok_or("candidate has no configuration")?;
        let model = SessionModelState::restore(generation.models.clone(), selection)
            .map_err(|error| error.to_string())?;
        validate_model_context(&model).map_err(|error| error.message)?;
        let preview = Arc::new(RuntimeResourceSnapshot::from_prepared(
            old.revision().next(),
            resources.clone(),
            capability.configuration_snapshot(old.capability()),
        ));
        Ok(PreparedConfiguration {
            owner: self.inner.configuration_owner.clone(),
            baseline,
            preview,
            model,
            capability: Some(capability),
            resources: Some(resources),
        })
    }

    /// Binds a prepared candidate to its complete context. The Session's own
    /// `selection` is carried, never re-chosen: a catalog that no longer
    /// admits it yields an unavailable candidate.
    pub(crate) fn complete_candidate_context(
        &self,
        candidate: &mut PreparedConfiguration,
        binding: &ContextBinding,
        selection: SessionModelConfig,
    ) -> Result<(), String> {
        let current = self.inner.lock_state().model.clone();
        let model = current
            .carry(binding.configuration.models.clone(), selection)
            .map_err(|error| error.to_string())?;
        validate_model_context(&model).map_err(|error| error.message)?;
        if let Some(capability) = &mut candidate.capability {
            capability.set_context_profile(&binding.configuration.config.agent);
        }
        if let Some(resources) = &mut candidate.resources {
            resources.set_context_binding(binding);
        }
        candidate.preview = Arc::new(candidate.preview.with_context_binding(binding)?);
        candidate.model = model;
        Ok(())
    }

    /// Prepares a context-only candidate over the exact adopted generation
    /// this call selects. `compose` is the application's composition of its
    /// context units over that generation's configuration. Context adoption
    /// carries Session intent; it cannot choose a model.
    pub(crate) fn prepare_context_configuration(
        &self,
        compose: impl FnOnce(&RuntimeConfiguration) -> ContextBinding,
    ) -> Result<PreparedConfiguration, String> {
        let (baseline, old, old_model) = {
            let state = self.inner.lock_state();
            (
                state.binding_revision,
                state.resources.clone(),
                state.model.clone(),
            )
        };
        let binding = compose(old.configuration().ok_or("missing configuration")?);
        let model = old_model
            .carry(
                binding.configuration.models.clone(),
                old_model.config().clone(),
            )
            .map_err(|error| error.to_string())?;
        validate_model_context(&model).map_err(|error| error.message)?;
        let preview = Arc::new(old.with_context_binding(&binding)?);
        Ok(PreparedConfiguration {
            owner: self.inner.configuration_owner.clone(),
            baseline,
            preview,
            model,
            capability: None,
            resources: None,
        })
    }

    /// Stage one fully prepared candidate for this runtime's safe boundary.
    ///
    /// The candidate replaces any earlier staged one, which is superseded.
    /// When no real execution owner blocks replacement the commit happens
    /// here, inside this call; otherwise the runtime commits it before it
    /// admits its next independent Attempt, or when the last blocker settles.
    /// The caller holds its source fence, so no newer desire can be captured
    /// between its currency check and this staging.
    pub(crate) fn stage_configuration(&self, staged: StagedConfiguration) {
        let mut state = self.inner.lock_state();
        if let Some(previous) = state.staged_configuration.replace(staged) {
            self.inner
                .settle_staged_configuration(previous, StagedOutcome::Superseded);
        }
        self.inner.commit_staged_configuration_locked(&mut state);
        drop(state);
        // A blocked candidate is retried at the worker's next boundary pass.
        self.inner.wake.eligibility.notify_one();
    }

    /// The identity of this runtime allocation, shared with every candidate
    /// it prepares.
    pub(crate) fn configuration_owner(&self) -> &Arc<()> {
        &self.inner.configuration_owner
    }

    /// This allocation's native authority over unrecorded configuration
    /// commits.
    pub(crate) fn configuration_health(&self) -> ConfigurationHealth {
        ConfigurationHealth(self.inner.unrecorded_configuration.clone())
    }

    /// The Session model state configuration changes are compared against.
    #[cfg(test)]
    pub(crate) fn model_state_for_test(&self) -> crate::model::session::SessionModelState {
        self.inner.lock_state().model.clone()
    }

    /// Whether a candidate is staged and waiting for the safe boundary.
    #[cfg(test)]
    pub(crate) fn has_staged_configuration(&self) -> bool {
        self.inner.lock_state().staged_configuration.is_some()
    }

    /// Publish the shared child capacity unit. `capacity` is the desired
    /// configuration whose child limit the subagent registry adopts; `compose`
    /// is the application's composition of that unit over the adopted
    /// generation's configuration (`None` when nothing it carries changes).
    pub(crate) fn apply_shared_capacity(
        &self,
        capacity: &crate::local_runtime::config::CurrentRuntimeConfig,
        compose: impl FnOnce(&RuntimeConfiguration) -> Option<RuntimeConfiguration>,
    ) {
        if let Some(subagents) = &self.inner.subagents {
            subagents.publish_configuration(capacity);
        }
        let mut state = self.inner.lock_state();
        if let Some(configuration) = state.resources.configuration().and_then(|c| compose(c)) {
            state.resources = Arc::new(state.resources.with_configuration(configuration));
        }
    }

    pub(crate) async fn settle_configuration_resources(&self) {
        // Retirement only collects generations whose immutable leases are gone.
        // Physical failure is reported by the existing native health owner.
        let _ = self.inner.capability.settle_ready_mcp_runtimes().await;
    }

    pub(crate) fn restore_configuration_binding(&self, revision: u64) {
        let mut state = self.inner.lock_state();
        assert_eq!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Inactive
        );
        state.binding_revision = revision;
    }

    /// Publish a newly available Model Catalog to this Session for future
    /// selection without adopting anything.
    ///
    /// Under the lock Attempt admission and model changes take, the Session's
    /// selection authority ([`SessionModelState::publish_catalog`]), the
    /// catalog its configuration generation carries and the retained binding
    /// are committed together, so `model_catalog`, `model_set` and every
    /// later admission observe one catalog. The current invocations, their
    /// adapters and the capability snapshot are retained verbatim; an
    /// admitted Attempt keeps the snapshot and registry it froze. A configured
    /// selection the catalog no longer admits becomes unavailable rather than
    /// blocking publication. The binding revision advances so no candidate
    /// prepared against the older catalog can adopt over this one, and
    /// `retain` commits the retained binding at that revision inside the same
    /// critical section. `compose` is the application's composition of the
    /// Provider unit over the adopted generation's configuration. The caller
    /// holds the source/application fence.
    ///
    /// Returns `Ok(false)`, changing nothing, when the current configuration
    /// would resolve to different invocations against `models`: that change
    /// needs preparation and adoption.
    ///
    /// # Errors
    ///
    /// Returns `retain`'s refusal, or a generation that carries no
    /// configuration, with nothing committed.
    pub(crate) fn publish_model_catalog(
        &self,
        models: crate::model::invocation::ModelBindingRegistry,
        compose: impl FnOnce(&RuntimeConfiguration) -> RuntimeConfiguration,
        retain: impl FnOnce(u64) -> Result<(), String>,
    ) -> Result<bool, String> {
        let mut state = self.inner.lock_state();
        let mut model = state.model.clone();
        if !model.publish_catalog(models) {
            return Ok(false);
        }
        let configuration = compose(
            state
                .resources
                .configuration()
                .ok_or("the adopted generation carries no configuration")?,
        );
        let resources = Arc::new(
            state
                .resources
                .with_configuration(configuration)
                .with_revision(state.resources.revision().next()),
        );
        let revision = state
            .binding_revision
            .checked_add(1)
            .expect("Session binding revision exhausted");
        retain(revision)?;
        state.binding_revision = revision;
        state.model = model;
        let availability = resources.capability_availability().clone();
        state.resources = resources;
        self.inner.observe_resources(&state, availability);
        Ok(true)
    }

    /// Publish independent execution policy without changing the adopted
    /// context or any admitted Attempt. `compose` is the application's
    /// composition of that unit over the adopted generation's configuration
    /// (`None` when nothing it carries changes). The native configuration owner
    /// holds its input fence across this call; this lock orders the swap
    /// against Attempt capture.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic when the runtime is not running.
    pub(crate) fn apply_execution_policy(
        &self,
        compose: impl FnOnce(&RuntimeConfiguration) -> Option<RuntimeConfiguration>,
    ) -> Result<StagedOutcome, String> {
        let mut state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running() {
            return Err("runtime is not running".into());
        }
        // Approval / ExecutionPolicy publishes immediately under this lock.
        // Live Attempts keep the mode frozen at their admission; background and
        // subagent residency never delay the session-level mode swap (P13).
        let Some(configuration) = state.resources.configuration().and_then(|c| compose(c)) else {
            return Ok(StagedOutcome::Committed);
        };
        let approval_mode = configuration.config.approval_mode;
        let resources = Arc::new(
            state
                .resources
                .with_configuration(configuration)
                .with_revision(state.resources.revision().next()),
        );
        // The unit keeps its own commit identity: it changes the generation
        // future Attempts freeze, never the Session binding.
        let change = ConfigurationChange::between(
            crate::runtime::configuration_commit::ConfigurationChangeReason::Source,
            (state.binding_revision, state.binding_revision),
            (&state.resources, &state.model),
            (&resources, &state.model),
        );
        state.effective_approval_mode = approval_mode;
        let availability = resources.capability_availability().clone();
        state.resources = resources;
        let outcome = self.inner.record_effective_change(&mut state, change);
        self.inner.observe_resources(&state, availability);
        Ok(outcome)
    }

    /// Reads the adopted generation, Session intent and admitted work in one
    /// coherent cut of the coordinator state.
    #[must_use]
    pub(crate) fn configuration_cut(&self) -> ConfigurationCut {
        let state = self.inner.lock_state();
        ConfigurationCut {
            binding_revision: state.binding_revision,
            resources: state.resources.clone(),
            approval_mode: state.effective_approval_mode,
            session_model: state.explicit_model.then(|| state.model.view().configured),
            effective_model: state.model.view(),
            admitted_attempt: state
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.configuration.clone()),
        }
    }
}

#[cfg(test)]
mod tests;

/// One runtime allocation's answer to whether it published a configuration
/// commit whose change fact it could not record. The failure is absorbing, so
/// only a replacement allocation recovers from it.
#[derive(Clone)]
pub(crate) struct ConfigurationHealth(Arc<std::sync::OnceLock<String>>);

impl ConfigurationHealth {
    /// The diagnostic of the first unrecorded commit, if any.
    pub(crate) fn unrecorded(&self) -> Option<String> {
        self.0.get().cloned()
    }

    pub(crate) fn same_allocation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ConfigurationHealth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ConfigurationHealth")
            .field(&self.unrecorded())
            .finish()
    }
}
