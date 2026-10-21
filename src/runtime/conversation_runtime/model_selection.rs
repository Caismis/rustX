//! Session model selection: the Session-owned selection authority, its
//! prepare/capture/commit protocol, approval mode, and model views.

use super::*;

/// Complete selection resolved from one published authority, with the identities
/// needed to reject any intervening binding or policy/resource publication.
pub(crate) struct PreparedModelSelection {
    pub(super) owner: Arc<()>,
    pub(crate) baseline: u64,
    pub(super) resources: Arc<RuntimeResourceSnapshot>,
    pub(super) model: SessionModelState,
}

impl PreparedModelSelection {
    pub(crate) fn catalog_selection(
        &self,
        choice: &crate::model::session::CatalogModelSelection,
    ) -> Result<SessionModelConfig, ModelUpdateError> {
        if self.baseline != choice.binding_revision {
            return Err(ModelUpdateError::Conflict);
        }
        if choice.model_position > 9_007_199_254_740_991
            || choice
                .profile_position
                .is_some_and(|ordinal| ordinal > 9_007_199_254_740_991)
        {
            return Err(ModelUpdateError::InvalidConfiguration(
                "catalog position exceeds protocol bounds".into(),
            ));
        }
        let source = self.model.read_source().catalog_source();
        let row = source.model_at(choice.model_position).ok_or_else(|| {
            ModelUpdateError::InvalidConfiguration("catalog model position is unavailable".into())
        })?;
        let mut config = SessionModelConfig::of(row.model.clone());
        // Catalog defaults replace primary-owned parameters only. Summary is
        // independently configured and must survive deferred observation.
        config.summary_model = self.model.config().summary_model.clone();
        if let Some(position) = choice.profile_position {
            config.profile = Some(
                source
                    .profile_at(choice.model_position, position)
                    .ok_or_else(|| {
                        ModelUpdateError::InvalidConfiguration(
                            "catalog profile position is unavailable".into(),
                        )
                    })?
                    .id
                    .clone(),
            );
        }
        Ok(config)
    }
}

/// Whether a completed selection changed Session authority. A successful
/// validation of an already explicit selection is not an application change.
pub(crate) struct ModelSelectionCommit {
    pub(crate) view: SessionModelView,
    pub(crate) changed: bool,
}

/// A session model update failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelUpdateError {
    /// A durable Session owns this selection. Runtime-only mutation cannot
    /// update its catalog and retained binding, so it is not a supported control.
    SessionOwned,
    /// The published binding or resource snapshot changed during preparation.
    Conflict,
    /// The runtime has not been activated: a live semantic mutation of an
    /// inert conversation is refused and consumes nothing.
    Inactive,
    /// The configuration cannot be resolved against the catalog or cannot
    /// run under the session context policy.
    InvalidConfiguration(String),
    /// The runtime's durable authority failed persistently: no new durable
    /// mutation may begin until the runtime is reconstructed.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
}

/// A runtime `ApprovalMode` control update failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalModeUpdateError {
    /// The runtime has not been activated.
    Inactive,
    /// The runtime's durable authority failed persistently.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
}

/// Projects a model-resolution failure into a descriptive message.
///
/// Resolution errors never carry credential material: the catalog names an
/// environment variable at most.
pub(super) fn invalid_model(error: &ModelInvocationError) -> ModelUpdateError {
    ModelUpdateError::InvalidConfiguration(error.to_string())
}

/// Whether one model snapshot can run under the session context policy.
///
/// A model whose context window cannot accommodate the policy reserve plus
/// the model output budget can never run an attempt. Rejecting it when the
/// session model is *constructed* or *set* is what keeps the per-attempt
/// context runtime construction infallible at admission, where there is no
/// caller left to report a failure to.
///
/// # Errors
///
/// Returns the engine configuration error.
/// [`validate_context_policy`] for a Session's model state. An unavailable
/// selection resolves no budget and admits no work, so there is nothing to
/// fit; the selection that replaces it is validated when it is applied.
pub(crate) fn validate_model_context(
    policy: &SessionContextPolicy,
    model: &SessionModelState,
) -> Result<(), crate::context::ContextError> {
    model.snapshot().map_or(Ok(()), |snapshot| {
        validate_context_policy(policy, &snapshot)
    })
}

pub(crate) fn validate_context_policy(
    policy: &SessionContextPolicy,
    model: &AttemptModelSnapshot,
) -> Result<(), crate::context::ContextError> {
    policy.validate_budgets(
        (
            model.primary().context_window(),
            model.primary().max_output_tokens(),
        ),
        (
            model.summary_invocation().context_window(),
            model.summary_invocation().max_output_tokens(),
        ),
    )
}

impl ConversationRuntime {
    /// Reads the authoritative session model catalog.
    #[must_use]
    pub fn model_catalog(&self) -> ModelCatalogView {
        self.inner.lock_state().model.catalog_view()
    }

    /// Reads the authoritative effective/desired `ApprovalMode` control state.
    #[must_use]
    pub fn approval_mode(&self) -> ApprovalMode {
        self.inner.lock_state().effective_approval_mode
    }

    /// Replaces a standalone conversation's in-memory model configuration.
    ///
    /// Durable local Sessions must select through their native Session owner,
    /// which commits catalog settings, retained binding, and runtime authority
    /// together. Every runtime clone retains that construction-time ownership.
    ///
    /// # Lifecycle
    ///
    /// A model update is a live semantic mutation: it is refused with the
    /// typed [`ModelUpdateError::Inactive`] while the runtime is inactive
    /// and consumes nothing. Construction-time model configuration belongs
    /// in the [`SessionModelState`] supplied to
    /// [`ConversationRuntime::new`](Self::new), not in a live mutation of a
    /// runtime that has not started.
    ///
    /// # Linearization
    ///
    /// Resolution and validation use an immutable published capture off the
    /// coordinator lock. Revision/resource validation and replacement happen
    /// under the coordinator lock that also owns attempt admission. An update
    /// therefore either linearizes before an admission
    /// (and that attempt observes it) or after it (and only later attempts
    /// observe it). There is no third possibility and no timing assumption.
    ///
    /// # Transactionality
    ///
    /// A rejected update changes nothing: the session keeps its previous
    /// configuration and no observation is published.
    ///
    /// # Errors
    ///
    /// Returns [`ModelUpdateError::SessionOwned`] for a durable Session runtime,
    /// [`ModelUpdateError::Inactive`] before activation,
    /// [`ModelUpdateError::DurabilityFailed`] while the runtime's durable
    /// authority is in the explicit failed state, and
    /// [`ModelUpdateError::InvalidConfiguration`] when the configuration
    /// cannot be resolved against the catalog or cannot run under the
    /// session context policy.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<SessionModelView, ModelUpdateError> {
        if self.inner.model_selection_owner == ModelSelectionOwner::Session {
            return Err(ModelUpdateError::SessionOwned);
        }
        let candidate = self.prepare_model_selection(config)?;
        self.commit_model_selection(candidate, |_| Ok(false))
            .map(|commit| commit.view)
    }

    /// Resolve against the exact published authority, off the admission lock.
    /// Resources are retained, never prepared or replaced by model selection.
    pub(crate) fn prepare_model_selection(
        &self,
        config: SessionModelConfig,
    ) -> Result<PreparedModelSelection, ModelUpdateError> {
        self.resolve_model_selection(self.capture_model_selection()?, config)
    }

    /// Cheap immutable capture; the Session owner calls this under its source fence.
    pub(crate) fn capture_model_selection(
        &self,
    ) -> Result<PreparedModelSelection, ModelUpdateError> {
        let state = self.inner.lock_state();
        self.check_model_selection_lifecycle()?;
        Ok(PreparedModelSelection {
            owner: self.inner.configuration_owner.clone(),
            baseline: state.binding_revision,
            resources: state.resources.clone(),
            model: state.model.clone(),
        })
    }

    pub(crate) fn resolve_model_selection(
        &self,
        mut candidate: PreparedModelSelection,
        config: SessionModelConfig,
    ) -> Result<PreparedModelSelection, ModelUpdateError> {
        candidate
            .model
            .apply(config)
            .map_err(|error| invalid_model(&error))?;
        let policy = candidate
            .resources
            .configuration()
            .map_or(self.inner.context.policy, |generation| {
                generation.config.context_policy()
            });
        validate_model_context(&policy, &candidate.model)
            .map_err(|error| ModelUpdateError::InvalidConfiguration(error.message))?;
        Ok(candidate)
    }

    pub(super) fn check_model_selection_lifecycle(&self) -> Result<(), ModelUpdateError> {
        if !self.inner.lifecycle.is_running() {
            return Err(ModelUpdateError::Inactive);
        }
        if let Some(failure) = self.inner.durability_gate.admission_failure() {
            return Err(ModelUpdateError::DurabilityFailed {
                message: failure.diagnostic,
            });
        }
        Ok(())
    }

    /// Selection's sole publication boundary, shared with independent Attempt
    /// admission and retirement. The native Session owner holds its source and
    /// settings fences and persists/retains in `commit` before this infallible
    /// publication. An already-admitted Attempt owns its old snapshot and leases.
    /// No idle or physical-resource replacement is involved.
    /// `commit` returns whether durable authority changed; the outcome reports
    /// either that change or a changed runtime selection, never mere success.
    pub(crate) fn commit_model_selection(
        &self,
        candidate: PreparedModelSelection,
        commit: impl FnOnce(u64) -> Result<bool, ModelUpdateError>,
    ) -> Result<ModelSelectionCommit, ModelUpdateError> {
        #[cfg(test)]
        let (contended, locked_gate) = {
            let mut probe = self
                .inner
                .probe
                .lock()
                .expect("coordinator probe lock poisoned");
            probe.as_mut().map_or((None, None), |probe| {
                (
                    probe.selection_contended.take(),
                    probe.selection_locked_gate.clone(),
                )
            })
        };
        #[cfg(test)]
        if let Some(contended) = contended {
            assert!(matches!(
                self.inner.state.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            contended.notify_one();
        }
        let mut state = self.inner.lock_state();
        #[cfg(test)]
        if let Some(gate) = locked_gate {
            gate.enter();
        }
        self.check_model_selection_lifecycle()?;
        let _permission = self
            .inner
            .durability_gate
            .enter_coordinator_commit()
            .map_err(|failure| ModelUpdateError::DurabilityFailed {
                message: failure.diagnostic,
            })?;
        #[cfg(test)]
        let commit_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.selection_commit_gate.clone());
        #[cfg(test)]
        if let Some(gate) = commit_gate {
            gate.enter();
        }
        if state.binding_revision != candidate.baseline
            || !Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
            || !Arc::ptr_eq(&candidate.resources, &state.resources)
        {
            return Err(ModelUpdateError::Conflict);
        }
        let unchanged = state.explicit_model && state.model.view() == candidate.model.view();
        let revision = if unchanged {
            state.binding_revision
        } else {
            state
                .binding_revision
                .checked_add(1)
                .expect("Session binding revision exhausted")
        };
        // Even an unchanged live invocation must visit its durable owner:
        // a loaded default may not yet be an explicit persisted selection.
        let persisted = commit(revision)?;
        if unchanged {
            return Ok(ModelSelectionCommit {
                view: state.model.view(),
                changed: persisted,
            });
        }
        let reopens = state.model.unavailable().is_some();
        state.model = candidate.model;
        state.explicit_model = true;
        state.binding_revision = revision;
        let view = state.model.view();
        self.inner
            .observe(ConversationObservation::SessionModelChanged {
                model: state
                    .model
                    .read_source()
                    .at_binding_revision(state.binding_revision),
                capability: None,
            });
        if reopens {
            self.inner.mailbox.wake().notify_one();
        }
        Ok(ModelSelectionCommit {
            view,
            changed: true,
        })
    }

    pub(crate) fn model_is_frozen(&self) -> bool {
        self.inner.lock_state().model.registry().is_none()
    }

    /// Immutable native read evidence for transport boundary tests.
    #[cfg(test)]
    pub(crate) fn model_read_source(&self) -> crate::model::session::SessionModelReadSource {
        let state = self.inner.lock_state();
        state
            .model
            .read_source()
            .at_binding_revision(state.binding_revision)
    }

    /// The authoritative session model view.
    #[must_use]
    pub fn model_view(&self) -> SessionModelView {
        self.inner.lock_state().model.view()
    }

    /// The authoritative session model configuration (test convenience).
    #[cfg(test)]
    pub(crate) fn model_config(&self) -> SessionModelConfig {
        self.inner.lock_state().model.config().clone()
    }

    /// The resolved invocations the next Attempt would freeze, adapter
    /// bindings included (test convenience).
    #[cfg(test)]
    pub(crate) fn model_snapshot(&self) -> AttemptModelSnapshot {
        self.inner
            .lock_state()
            .model
            .snapshot()
            .expect("an admissible selection")
    }
}
