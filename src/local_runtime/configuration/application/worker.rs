//! The finite native preparation loop. One active preparation, latest pending
//! input per scope. Source commits and final publication use the parent's lock.
use super::{
    ApplicationIdentity, ApplyUnit, CapturedApplication, ConfigurationApplications, UnitApplication,
};
use crate::local_runtime::session_runtime_manager::SessionRuntimeManager;
use crate::runtime::identity::SessionId;

impl ConfigurationApplications {
    pub(crate) fn run(&self, manager: SessionRuntimeManager) {
        if !self.lock().start_worker() {
            return;
        }
        let owner = self.clone();
        tokio::spawn(async move {
            loop {
                let Some((scope, identity)) = owner.lock().next() else {
                    break;
                };
                Box::pin(owner.prepare_scope(&manager, &scope, &identity)).await;
                if let Ok(session) = SessionId::parse(&scope)
                    && let Some(runtime) = manager.configuration_runtime(&session)
                {
                    runtime.settle_configuration_resources().await;
                }
            }
        });
    }

    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    async fn prepare_scope(
        &self,
        manager: &SessionRuntimeManager,
        scope: &str,
        identity: &ApplicationIdentity,
    ) {
        if scope.starts_with("source:") {
            let mut state = self.lock();
            if !state.current(scope, identity) {
                return;
            }
            let outcome = match state.source_inputs.get(scope).cloned() {
                Some(Ok(captured)) => {
                    // Process authority is global: an older Workspace scope must
                    // never roll back a newer User publication.
                    let process = state.desired_process.clone().unwrap_or(captured);
                    if let Some((previous, outcome)) = &state.process
                        && previous == &process
                    {
                        let outcome = outcome.clone();
                        state.publish(scope, identity, ApplyUnit::ProcessBindings, || outcome);
                        self.notify(&state);
                        return;
                    }
                    let outcome = if manager.apply_process_limits(&process) {
                        UnitApplication::ProcessRestart
                    } else {
                        UnitApplication::Applied
                    };
                    state.process = Some((process, outcome.clone()));
                    outcome
                }
                Some(Err(diagnostic)) => UnitApplication::Failed { diagnostic },
                None => UnitApplication::Failed {
                    diagnostic: "Source inputs unavailable".into(),
                },
            };
            state.publish(scope, identity, ApplyUnit::ProcessBindings, || outcome);
            self.notify(&state);
            return;
        }
        let Ok(session) = SessionId::parse(scope) else {
            return;
        };
        let capture = {
            let state = self.lock();
            if !state.current(scope, identity) {
                return;
            }
            state.captured_input(scope)
        };
        let CapturedApplication {
            policy,
            process,
            context: capture,
            ..
        } = match capture {
            Ok(capture) => capture,
            Err(diagnostic) => {
                self.fail_units(
                    scope,
                    identity,
                    &[
                        ApplyUnit::ExecutionPolicy,
                        ApplyUnit::SharedCapacity,
                        ApplyUnit::ProcessBindings,
                        ApplyUnit::Capabilities,
                        ApplyUnit::Instructions,
                        ApplyUnit::Provider,
                    ],
                    diagnostic,
                );
                return;
            }
        };
        #[cfg(test)]
        manager
            .configuration
            .test_hooks
            .reach("configuration_policy_prepared");
        // The runtime and the resource preparation it was composed with are
        // taken from one resident composition, never from two lookups.
        let (runtime, resource_loader) = manager
            .configuration_preparation(&session)
            .map_or((None, None), |(runtime, loader)| {
                (Some(runtime), Some(loader))
            });
        {
            let mut state = self.lock();
            if !state.current(scope, identity) {
                return;
            }
            // One process owner performs the change; Session views only project
            // that outcome. Identical process input never re-applies the limits.
            let process = state.desired_process.clone().unwrap_or(process);
            let outcome = match &state.process {
                Some((previous, outcome)) if previous == &process => outcome.clone(),
                _ => {
                    let outcome = if manager.apply_process_limits(&process) {
                        UnitApplication::ProcessRestart
                    } else {
                        UnitApplication::Applied
                    };
                    state.process = Some((process, outcome.clone()));
                    outcome
                }
            };
            state.publish(scope, identity, ApplyUnit::ProcessBindings, || outcome);
            state.publish(scope, identity, ApplyUnit::SharedCapacity, || {
                if let Some(runtime) = &runtime {
                    runtime.apply_shared_capacity(&policy, |base| {
                        policy.compose_runtime_shared_capacity(base)
                    });
                }
                UnitApplication::Applied
            });
            state.publish(
                scope,
                identity,
                ApplyUnit::ExecutionPolicy,
                || match runtime.as_ref().map(|runtime| {
                    policy.timeout_policy().map_err(|e| e.to_string())?;
                    policy.tool_deadline_policy().map_err(|e| e.to_string())?;
                    runtime.apply_execution_policy(|base| {
                        policy.compose_runtime_execution_policy(base)
                    })
                }) {
                    Some(Err(diagnostic)) => UnitApplication::Failed { diagnostic },
                    _ => UnitApplication::Applied,
                },
            );
            for unit in [ApplyUnit::ExecutionPolicy, ApplyUnit::SharedCapacity] {
                if state.scopes[scope].units[&unit] != UnitApplication::Applied {
                    continue;
                }
                let mut bindings = manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings");
                if let Some(retained) = bindings.get_mut(&session) {
                    *retained = match unit {
                        ApplyUnit::ExecutionPolicy => retained.with_execution_policy(&policy),
                        ApplyUnit::SharedCapacity => retained.with_shared_capacity(&policy),
                        _ => unreachable!(),
                    };
                }
                if let Some(source) = state.scope_sources.get(scope).cloned()
                    && state
                        .desired_sources
                        .get(&source)
                        .and_then(|input| input.as_ref().ok())
                        .is_some_and(|input| {
                            Some(&input.revision) == identity.input_revision.as_ref()
                        })
                    && let Some(available) = state.available.get_mut(&source)
                {
                    match unit {
                        ApplyUnit::ExecutionPolicy => available.compose_execution_policy(&policy),
                        ApplyUnit::SharedCapacity => available.compose_shared_capacity(&policy),
                        _ => unreachable!(),
                    }
                }
            }
            self.notify(&state);
        }
        let mut capture = match capture {
            Ok(capture) => capture,
            Err(diagnostic) => {
                self.fail_units(
                    scope,
                    identity,
                    &[
                        ApplyUnit::Capabilities,
                        ApplyUnit::Instructions,
                        ApplyUnit::Provider,
                    ],
                    diagnostic,
                );
                return;
            }
        };
        let retained = || {
            manager
                .sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .get(&session)
                .cloned()
        };
        let mut adopted = retained();
        #[cfg(test)]
        manager.configuration_catalog_gate(&session).await;
        // The Provider unit settles on its own when publishing the catalog
        // changes no invocation the adopted closure would freeze, or when the
        // Session's own selection is one the new catalog no longer admits: the
        // catalog is what the replacement is chosen from, so neither an
        // unrelated Instructions, context or Capability change — including a
        // named Agent the source newly admits, drops or redefines — nor a
        // removed selection may hold it back. A removed selection is never
        // prepared, because there is no invocation to construct from it. Any
        // other unit is then prepared against this published binding, so its
        // candidate can neither restore the older catalog nor adopt over this
        // publication.
        let mut provider_published = false;
        {
            let mut state = self.lock();
            if !state.current(scope, identity) {
                return;
            }
            if let Some(old) = &adopted
                && capture.preserves_adopted_agent_models(old)
                && (capture.same_session_model(old) || capture.selection_unavailable().is_some())
            {
                let others_unchanged = capture.same_capabilities(old) && capture.same_context(old);
                // Capture validated the catalog and its default; publication
                // resolves it. Source availability composes the whole
                // generation, so it waits for any unit still to prepare.
                if others_unchanged
                    && let Err(diagnostic) =
                        state.make_available(scope, identity, &capture, &manager.credentials)
                {
                    drop(state);
                    self.fail_units(
                        scope,
                        identity,
                        &[
                            ApplyUnit::Capabilities,
                            ApplyUnit::Instructions,
                            ApplyUnit::Provider,
                        ],
                        diagnostic,
                    );
                    return;
                }
                // Invocation equivalence settles adoption, not discovery: the
                // valid catalog still reaches this Session's future selection.
                let published = if capture.same_catalog(old) {
                    Ok(true)
                } else {
                    Self::publish_catalog(&mut state, manager, &session, &capture)
                };
                match published {
                    Ok(true) if others_unchanged => {
                        for unit in [
                            ApplyUnit::Capabilities,
                            ApplyUnit::Instructions,
                            ApplyUnit::Provider,
                        ] {
                            state.publish(scope, identity, unit, || UnitApplication::Applied);
                        }
                        self.notify(&state);
                        return;
                    }
                    Ok(true) => {
                        state.publish(scope, identity, ApplyUnit::Provider, || {
                            UnitApplication::Applied
                        });
                        self.notify(&state);
                        provider_published = true;
                    }
                    Ok(false) => {}
                    Err(diagnostic) => {
                        // Nothing was published. The generation's context is
                        // prepared over this catalog, so it fails with it;
                        // unchanged capabilities have nothing to apply.
                        if capture.same_capabilities(old) {
                            state.publish(scope, identity, ApplyUnit::Capabilities, || {
                                UnitApplication::Applied
                            });
                        }
                        drop(state);
                        self.fail_units(
                            scope,
                            identity,
                            &[
                                ApplyUnit::Capabilities,
                                ApplyUnit::Instructions,
                                ApplyUnit::Provider,
                            ],
                            diagnostic,
                        );
                        return;
                    }
                }
            }
        }
        if provider_published {
            // Remaining preparation composes over the binding just published.
            adopted = retained();
        }
        let Some(runtime) = runtime else {
            let mut state = self.lock();
            if state.current(scope, identity) {
                state.deferred.insert(scope.to_owned());
                // A source-only context/provider edit can be prepared against
                // the retained, already available capability definitions. New
                // resource closures still need the allocation publication seam.
                if state
                    .available
                    .get(&capture.input.cwd)
                    .is_some_and(|available| capture.same_capabilities(available))
                    && let Err(diagnostic) =
                        state.make_available(scope, identity, &capture, &manager.credentials)
                {
                    drop(state);
                    self.fail_units(
                        scope,
                        identity,
                        &[ApplyUnit::Instructions, ApplyUnit::Provider],
                        diagnostic,
                    );
                    return;
                }
                // Allocation-specific resources and request-shape comparison
                // wait for natural load. Retained adoption is never changed.
            }
            self.notify(&state);
            return;
        };
        let context_only = adopted
            .as_ref()
            .is_some_and(|old| capture.same_capabilities(old));
        let bindings = |capture: &crate::local_runtime::configuration::ProspectiveSessionConfig| {
            capture
                .models
                .resolve(&manager.credentials)
                .map_err(|error| error.to_string())
                .and_then(|catalog| {
                    crate::model::invocation::ModelBindingRegistry::new(catalog)
                        .map_err(|error| error.to_string())
                })
        };
        #[cfg(test)]
        let injected_failure = manager.configuration_test_gate(&session, false).await;
        #[cfg(not(test))]
        let injected_failure = false;
        let mut capability_failed = false;
        if context_only {
            let mut state = self.lock();
            state.publish(scope, identity, ApplyUnit::Capabilities, || {
                UnitApplication::Applied
            });
            self.notify(&state);
        }
        let prepared = if context_only {
            if injected_failure {
                Err("injected preparation failure".into())
            } else {
                self.lock()
                    .make_available(scope, identity, &capture, &manager.credentials)
                    .and_then(|()| bindings(&capture))
                    .and_then(|models| {
                        runtime.prepare_context_configuration(|base| {
                            capture.compose_context_binding(base, models)
                        })
                    })
            }
        } else {
            let cancellation = crate::runtime::cancellation::CancellationSignal::new();
            let resource_loader = resource_loader
                .as_ref()
                .expect("a resident runtime has its composed resource preparation");
            let prepared_capture = capture.clone();
            let mut preparation = Box::pin(runtime.prepare_configuration(
                |capability| resource_loader.prepare(capability, prepared_capture, &cancellation),
                capture.config.initial_model().clone(),
            ));
            let prepared =
                tokio::time::timeout(std::time::Duration::from_mins(2), &mut preparation).await;
            if prepared.is_err() {
                // Keep the only preparation slot until its physical owners have
                // acknowledged cancellation; dropping a waiter is not settlement.
                cancellation.cancel();
                drop(preparation.await);
            }
            let candidate = match prepared {
                Ok(Ok(mut candidate)) if injected_failure => {
                    if let Some(capability) = candidate.capability.take() {
                        capability.retire_uncommitted().await;
                    }
                    Err(
                        "injected capability preparation failure after resource construction"
                            .into(),
                    )
                }
                Ok(Ok(mut candidate)) => {
                    // Source availability belongs to the captured default's
                    // complete prepared configuration, not an old Session's
                    // selected model. The latter may no longer be in the new
                    // catalog and must not veto new Session availability.
                    let result = self
                        .lock()
                        .make_available(scope, identity, &capture, &manager.credentials)
                        .and_then(|()| bindings(&capture))
                        .and_then(|models| {
                            runtime.complete_candidate_context(
                                &mut candidate,
                                &capture.context_binding(models),
                                capture.session_model().clone(),
                            )
                        });
                    match result {
                        Ok(()) => Ok(candidate),
                        Err(diagnostic) => {
                            if let Some(capability) = candidate.capability.take() {
                                capability.retire_uncommitted().await;
                            }
                            Err(diagnostic)
                        }
                    }
                }
                result => {
                    let diagnostic = match result {
                        Ok(Err(error)) => error,
                        _ => "configuration preparation deadline exceeded".into(),
                    };
                    Err(diagnostic)
                }
            };
            match candidate {
                Ok(candidate) => Ok(candidate),
                Err(diagnostic) => {
                    capability_failed = true;
                    let mut state = self.lock();
                    state.publish(scope, identity, ApplyUnit::Capabilities, || {
                        UnitApplication::Failed { diagnostic }
                    });
                    // Child provider bindings participate in the capability
                    // closure. Independent instructions can still use the
                    // complete committed capability/provider binding, including
                    // its resource leases.
                    let old = adopted.as_ref().expect("retained Session binding");
                    if !provider_published {
                        let provider = if capture.same_provider(old) {
                            UnitApplication::Applied
                        } else {
                            UnitApplication::Failed {
                                diagnostic:
                                    "provider preparation depends on failed capability closure"
                                        .into(),
                            }
                        };
                        state.publish(scope, identity, ApplyUnit::Provider, || provider);
                    }
                    if capture.same_context(old) {
                        state.publish(scope, identity, ApplyUnit::Instructions, || {
                            UnitApplication::Applied
                        });
                        self.notify(&state);
                        return;
                    }
                    self.notify(&state);
                    drop(state);
                    capture = (**old).clone().retaining_context_from(&capture);
                    bindings(&capture).and_then(|models| {
                        runtime.prepare_context_configuration(|base| {
                            capture.compose_context_binding(base, models)
                        })
                    })
                }
            }
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(diagnostic) => {
                let units: &[ApplyUnit] = if capability_failed {
                    &[ApplyUnit::Instructions]
                } else if context_only {
                    &[ApplyUnit::Instructions, ApplyUnit::Provider]
                } else {
                    &[
                        ApplyUnit::Capabilities,
                        ApplyUnit::Instructions,
                        ApplyUnit::Provider,
                    ]
                };
                self.fail_units(scope, identity, units, diagnostic);
                return;
            }
        };
        #[cfg(test)]
        manager.configuration_test_gate(&session, true).await;
        let mut state = self.lock();
        if !state.current(scope, identity) {
            return;
        }
        // Availability composes from source-owned validated units, never from
        // an arbitrary Session's retained capability/provider selection.
        let available = if capability_failed {
            state
                .available
                .get(&capture.input.cwd)
                .map(|old| old.clone().retaining_context_from(&capture))
        } else {
            Some(capture.clone())
        };
        if let Some(available) = available
            && let Err(diagnostic) =
                state.make_available(scope, identity, &available, &manager.credentials)
        {
            drop(state);
            self.fail_units(
                scope,
                identity,
                &[
                    ApplyUnit::Capabilities,
                    ApplyUnit::Instructions,
                    ApplyUnit::Provider,
                ],
                diagnostic,
            );
            return;
        }
        let mut retained = match capture.clone().admit(|| manager.credentials.clone()) {
            Ok(retained) => retained,
            Err(diagnostic) => {
                drop(state);
                self.fail_units(
                    scope,
                    identity,
                    &[
                        ApplyUnit::Capabilities,
                        ApplyUnit::Instructions,
                        ApplyUnit::Provider,
                    ],
                    diagnostic,
                );
                return;
            }
        };
        // The units this candidate commits. Already settled units keep their
        // own outcome; the commit is reported to exactly the remaining ones.
        let units: Vec<ApplyUnit> = [
            ApplyUnit::Capabilities,
            ApplyUnit::Instructions,
            ApplyUnit::Provider,
        ]
        .into_iter()
        .filter(|unit| {
            !((capability_failed && *unit != ApplyUnit::Instructions)
                || (provider_published && *unit == ApplyUnit::Provider)
                || (context_only && *unit == ApplyUnit::Capabilities))
        })
        .collect();
        for &unit in &units {
            state.publish(scope, identity, unit, || UnitApplication::Ready);
        }
        let reason = state.reason(scope);
        let desired = state.desired(scope, identity);
        let bindings_owner = manager.clone();
        let retain_session = session.clone();
        let owner = self.clone();
        let settled_manager = manager.clone();
        let settled_scope = scope.to_owned();
        let settled_identity = identity.clone();
        self.notify(&state);
        // Staging happens under the source fence: no newer desire can be
        // captured between the currency check above and this hand-off. The
        // runtime commits at its own safe boundary, possibly inside this call.
        runtime.stage_configuration(crate::runtime::configuration_commit::StagedConfiguration {
            prepared,
            desired,
            reason,
            retain: Box::new(move |revision| {
                retained.binding_revision = revision;
                bindings_owner
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings")
                    .insert(retain_session, retained);
            }),
            settled: Box::new(move |outcome| {
                Box::pin(async move {
                    owner.staged_settled(
                        &settled_manager,
                        &settled_scope,
                        &settled_identity,
                        &units,
                        outcome,
                    );
                })
            }),
        });
    }

    /// Record the runtime's outcome for one staged candidate. A candidate the
    /// runtime superseded while it was still the desired application is
    /// prepared again from its exact captured input, never from newer files.
    fn staged_settled(
        &self,
        manager: &SessionRuntimeManager,
        scope: &str,
        identity: &ApplicationIdentity,
        units: &[ApplyUnit],
        outcome: crate::runtime::configuration_commit::StagedOutcome,
    ) {
        use crate::runtime::configuration_commit::StagedOutcome;
        let mut state = self.lock();
        if !state.current(scope, identity) {
            return;
        }
        let unit_outcome = match outcome {
            StagedOutcome::Committed => UnitApplication::Applied,
            StagedOutcome::Failed { diagnostic } => UnitApplication::Failed { diagnostic },
            StagedOutcome::Uncertain { diagnostic } => UnitApplication::Uncertain { diagnostic },
            StagedOutcome::Superseded => {
                let input = state.captured_input(scope);
                state.capture_binding(scope.to_owned(), input);
                self.notify(&state);
                drop(state);
                self.run(manager.clone());
                return;
            }
        };
        let committed = unit_outcome == UnitApplication::Applied;
        for &unit in units {
            state.publish(scope, identity, unit, || unit_outcome.clone());
        }
        self.notify(&state);
        drop(state);
        if committed
            && let Ok(session) = SessionId::parse(scope)
            && let Some(runtime) = manager.configuration_runtime(&session)
        {
            tokio::spawn(async move {
                runtime.settle_configuration_resources().await;
            });
        }
    }

    /// Publish a validated catalog to one Session under the application
    /// fence: every invocation it would freeze is unchanged, or its own
    /// selection is one the catalog no longer admits. A resident runtime
    /// commits its selection authority and the retained binding in one
    /// critical section, all or nothing. A cold Session defers to residency:
    /// its rebind takes this fence after the runtime is resident and
    /// republishes against it, so a load that composed from the older binding
    /// never keeps that catalog. Returns `Ok(false)` when the live invocation
    /// would change; that change needs preparation and adoption.
    fn publish_catalog(
        state: &mut super::ApplicationState,
        manager: &SessionRuntimeManager,
        session: &SessionId,
        capture: &crate::local_runtime::configuration::ProspectiveSessionConfig,
    ) -> Result<bool, String> {
        let models = capture
            .models
            .resolve(&manager.credentials)
            .map_err(|error| error.to_string())
            .and_then(|catalog| {
                crate::model::invocation::ModelBindingRegistry::new(catalog)
                    .map_err(|error| error.to_string())
            })?;
        let Some(runtime) = manager.configuration_runtime(session) else {
            state.deferred.insert(session.to_string());
            return Ok(true);
        };
        let catalog = models.clone();
        runtime.publish_model_catalog(
            catalog,
            |base| capture.compose_model_catalog(base, models),
            |revision| {
                let mut bindings = manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings");
                let retained = bindings
                    .get_mut(session)
                    .ok_or("the resident Session has no retained configuration binding")?;
                *retained = retained.with_published_catalog(capture, revision);
                Ok(())
            },
        )
    }

    #[allow(clippy::needless_pass_by_value)] // consumes diagnostics from fallible preparation branches
    fn fail_units(
        &self,
        scope: &str,
        identity: &ApplicationIdentity,
        units: &[ApplyUnit],
        diagnostic: String,
    ) {
        let mut state = self.lock();
        if !state.current(scope, identity) {
            return;
        }
        for &unit in units {
            if state.scopes[scope].units[&unit] == UnitApplication::Preparing {
                state.publish(scope, identity, unit, || UnitApplication::Failed {
                    diagnostic: diagnostic.clone(),
                });
            }
        }
        self.notify(&state);
    }
}
