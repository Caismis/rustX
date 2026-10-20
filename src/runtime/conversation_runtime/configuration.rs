//! Configuration adoption: off-side candidate preparation, the adoption
//! commit under the coordinator lock, independently published units, and
//! adoption eligibility. Authored-configuration composition is the
//! application owner's concern (`local_runtime::configuration`).

use super::*;

impl RuntimeInner {
    /// Whether this live runtime could adopt a prepared configuration now.
    ///
    /// Runtime-domain advisory state derived by the adoption gate's own idle
    /// rule over its in-memory owners (see [`IdleProbe::InMemory`]); it says
    /// nothing about any candidate, and the gate still evaluates the exact rule
    /// at its commit boundary.
    pub(super) fn adoption_eligibility(&self) -> AdoptionEligibility {
        let state = self.lock_state();
        if !self.lifecycle.is_running() {
            AdoptionEligibility::Unavailable
        } else if self.idle_epoch_probed(&state, IdleProbe::InMemory).is_err()
            || self.tool_runtime.background().configuration_busy()
            || self
                .subagents
                .as_ref()
                .is_some_and(crate::runtime::subagent::SubagentRegistry::configuration_busy)
        {
            AdoptionEligibility::Busy
        } else {
            AdoptionEligibility::Eligible
        }
    }

    /// Publishes the current adoption eligibility when it differs from the
    /// last publication. Derivation and publication happen under one lock, so
    /// publications are totally ordered, the latest always reflects the latest
    /// derivation, and an unchanged value is never republished.
    pub(super) fn publish_adoption_eligibility(&self) {
        let mut published = self
            .published_eligibility
            .lock()
            .expect("published eligibility lock poisoned");
        let eligibility = self.adoption_eligibility();
        if published.as_ref() != Some(&eligibility) {
            *published = Some(eligibility.clone());
            self.observe(ConversationObservation::AdoptionEligibility(eligibility));
        }
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
}

impl ConversationRuntime {
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
        let (baseline, old, old_model) = {
            let state = self.inner.lock_state();
            (
                state.binding_revision,
                state.resources.clone(),
                state.model.clone(),
            )
        };
        let prepared = prepare(&self.inner.capability)
            .await
            .map_err(|error| error.message)?;
        let (capability, resources) = prepared.into_parts();
        let generation = resources
            .configuration
            .as_ref()
            .ok_or("candidate has no configuration")?;
        let model = SessionModelState::new(generation.models.clone(), selection)
            .map_err(|error| error.to_string())?;
        validate_model_context(&generation.config.context_policy(), &model)
            .map_err(|error| error.message)?;
        let preview = Arc::new(RuntimeResourceSnapshot::from_prepared(
            old.revision().next(),
            resources.clone(),
            capability.configuration_snapshot(old.capability()),
        ));
        let impact = configuration_cache_impact(&old, &old_model, &preview, &model);
        Ok(PreparedConfiguration {
            owner: self.inner.configuration_owner.clone(),
            baseline,
            preview,
            model,
            capability: Some(capability),
            resources: Some(resources),
            impact,
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
        validate_model_context(&binding.configuration.config.context_policy(), &model)
            .map_err(|error| error.message)?;
        if let Some(capability) = &mut candidate.capability {
            capability.set_context_profile(
                &binding.configuration.config.agent,
                &binding.root_agent_project_files,
            );
        }
        if let Some(resources) = &mut candidate.resources {
            resources.set_context_binding(binding);
        }
        candidate.preview = Arc::new(candidate.preview.with_context_binding(binding)?);
        candidate.model = model;
        let state = self.inner.lock_state();
        candidate.impact = configuration_cache_impact(
            &state.resources,
            &state.model,
            &candidate.preview,
            &candidate.model,
        );
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
        validate_model_context(&binding.configuration.config.context_policy(), &model)
            .map_err(|error| error.message)?;
        let preview = Arc::new(old.with_context_binding(&binding)?);
        let impact = configuration_cache_impact(&old, &old_model, &preview, &model);
        Ok(PreparedConfiguration {
            owner: self.inner.configuration_owner.clone(),
            baseline,
            preview,
            model,
            capability: None,
            resources: None,
            impact,
        })
    }

    pub(crate) fn configuration_allocation_matches(
        &self,
        candidate: &PreparedConfiguration,
    ) -> bool {
        Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
    }

    /// Publish readiness while holding the same binding fence as model changes
    /// and Attempt admission. The caller holds the source/application fence.
    pub(crate) fn with_configuration_baseline(
        &self,
        candidate: &PreparedConfiguration,
        publish: impl FnOnce(),
    ) -> bool {
        let state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running()
            || state.binding_revision != candidate.baseline
            || !Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
        {
            return false;
        }
        publish();
        true
    }

    /// The live runtime's current configuration-adoption eligibility,
    /// derived from its native owners. Advisory only: adoption revalidates
    /// the real gate at its commit boundary.
    #[cfg(test)]
    pub(crate) fn configuration_adoption_eligibility(&self) -> AdoptionEligibility {
        self.inner.adoption_eligibility()
    }

    /// Final commit primitive used by the native configuration owner. It holds
    /// the source fence around this call; this gate is also Attempt admission's
    /// gate. A refusal leaves the ready candidate available to inspect/retry.
    pub(crate) fn adopt_configuration(
        &self,
        prepared: &mut Option<PreparedConfiguration>,
        expected_binding: u64,
        explicit: bool,
        retain: impl FnOnce() -> Result<(), AdoptionError>,
    ) -> Result<u64, AdoptionError> {
        let mut state = self.inner.lock_state();
        if explicit
            && (self.inner.idle_epoch_locked(&state).is_err()
                || self.inner.tool_runtime.background().configuration_busy()
                || self
                    .inner
                    .subagents
                    .as_ref()
                    .is_some_and(crate::runtime::subagent::SubagentRegistry::configuration_busy))
        {
            return Err(AdoptionError::Busy);
        }
        let candidate = prepared.as_ref().ok_or(AdoptionError::NotReady)?;
        if !self.inner.lifecycle.is_running()
            || state.binding_revision != expected_binding
            || candidate.baseline != expected_binding
            || !Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
            || (!explicit && !candidate.adopts_implicitly())
        {
            return Err(AdoptionError::Conflict);
        }
        let candidate = prepared.take().expect("checked candidate");
        let revision = state.resources.revision().next();
        let (resources, availability) = if let Some(capability) = candidate.capability {
            let committed = self
                .inner
                .capability
                .commit_runtime(&self.inner.capability_publication, capability)
                .map_err(|error| AdoptionError::Failed {
                    diagnostic: error.to_string(),
                })?;
            (
                Arc::new(RuntimeResourceSnapshot::from_prepared(
                    revision,
                    candidate.resources.expect("capability resource closure"),
                    committed.snapshot,
                )),
                committed.availability,
            )
        } else {
            (
                Arc::new(candidate.preview.as_ref().clone().with_revision(revision)),
                candidate.preview.capability_availability().clone(),
            )
        };
        retain()?;
        let reopens =
            state.model.unavailable().is_some() && candidate.model.unavailable().is_none();
        state.model = candidate.model;
        state.effective_approval_mode = resources
            .configuration()
            .map_or(state.effective_approval_mode, |generation| {
                generation.config.approval_mode
            });
        state.resources = resources;
        state.binding_revision = state
            .binding_revision
            .checked_add(1)
            .expect("Session binding revision exhausted");
        self.inner.observe_resources(&state, availability);
        if reopens {
            // Work accepted before the selection became unavailable was held,
            // not dropped; this commit is what admits it.
            self.inner.mailbox.wake().notify_one();
        }
        Ok(state.binding_revision)
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
    ) -> Result<bool, String> {
        let mut state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running() {
            return Err("runtime is not running".into());
        }
        let Some(configuration) = state.resources.configuration().and_then(|c| compose(c)) else {
            return Ok(false);
        };
        let approval_mode = configuration.config.approval_mode;
        let resources = Arc::new(
            state
                .resources
                .with_configuration(configuration)
                .with_revision(state.resources.revision().next()),
        );
        state.effective_approval_mode = approval_mode;
        let availability = resources.capability_availability().clone();
        state.resources = resources;
        self.inner.observe_resources(&state, availability);
        Ok(true)
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
