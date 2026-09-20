//! The finite native preparation loop. One active preparation, latest pending
//! input per scope. Source commits and final publication use the parent's lock.
use super::{
    AdoptionError, ApplicationIdentity, ApplyUnit, AvailableConfiguration, CacheImpact,
    CapturedApplication, ConfigurationApplication, ConfigurationApplications, ReadyConfiguration,
    UnitApplication,
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
        let runtime = manager.configuration_runtime(&session);
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
                    runtime.apply_shared_capacity(&policy);
                }
                UnitApplication::Applied
            });
            state.publish(
                scope,
                identity,
                ApplyUnit::ExecutionPolicy,
                || match runtime
                    .as_ref()
                    .map(|runtime| runtime.apply_execution_policy(&policy))
                {
                    Some(Err(diagnostic)) => UnitApplication::Failed { diagnostic },
                    _ => UnitApplication::Applied,
                },
            );
            if state.scopes[scope].units[&ApplyUnit::ExecutionPolicy] == UnitApplication::Applied {
                let mut bindings = manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings");
                if let Some(retained) = bindings.get_mut(&session) {
                    *retained = retained.with_execution_policy(&policy);
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
                    available.apply_execution_policy(&policy);
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
        let adopted = manager
            .sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings")
            .get(&session)
            .cloned();
        {
            let mut state = self.lock();
            if let Some(adopted) = &adopted
                && capture.same_capabilities(adopted)
                && capture.same_context(adopted)
                && capture.same_provider(adopted)
            {
                if let Err(diagnostic) =
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
        let mut context_only = adopted
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
                        runtime.prepare_context_configuration(&capture, models, None)
                    })
            }
        } else {
            let capability_capture = adopted.as_ref().map_or_else(
                || capture.clone(),
                |old| capture.clone().retaining_context_from(old),
            );
            let cancellation = crate::runtime::cancellation::CancellationSignal::new();
            let mut preparation = Box::pin(runtime.prepare_configuration(
                capture.clone(),
                capture.config.initial_model().clone(),
                &cancellation,
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
                        .and_then(|()| bindings(&capability_capture))
                        .and_then(|models| {
                            runtime.complete_candidate_context(
                                &mut candidate,
                                &capability_capture,
                                models,
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
            if let Err(diagnostic) = candidate {
                capability_failed = true;
                let mut state = self.lock();
                state.publish(scope, identity, ApplyUnit::Capabilities, || {
                    UnitApplication::Failed { diagnostic }
                });
                // Child provider bindings participate in the capability closure.
                // Independent instructions can still use the complete adopted
                // capability/provider binding, including its resource leases.
                let old = adopted.as_ref().expect("retained Session binding");
                let provider = if capture.same_provider(old) {
                    UnitApplication::Applied
                } else {
                    UnitApplication::Failed {
                        diagnostic: "provider preparation depends on failed capability closure"
                            .into(),
                    }
                };
                state.publish(scope, identity, ApplyUnit::Provider, || provider);
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
                    runtime.prepare_context_configuration(&capture, models, None)
                })
            } else {
                let mut candidate = candidate.expect("successful capability candidate");
                if candidate.impact == CacheImpact::Preserved {
                    let mut state = self.lock();
                    if !state.current(scope, identity) {
                        return;
                    }
                    let context_unchanged = capture.same_context(&capability_capture)
                        && capture.same_provider(&capability_capture);
                    let mut retained =
                        match capability_capture.admit(|| manager.credentials.clone()) {
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
                    let baseline = candidate.baseline;
                    retained.binding_revision = baseline + 1;
                    let mut ready = Some(candidate);
                    let outcome = runtime.adopt_configuration(&mut ready, baseline, false, || {
                        manager
                            .sessions
                            .configuration_bindings
                            .lock()
                            .expect("Session configuration bindings")
                            .insert(session.clone(), retained);
                        Ok(())
                    });
                    if let Err(error) = outcome {
                        drop(state);
                        self.fail_units(
                            scope,
                            identity,
                            &[
                                ApplyUnit::Capabilities,
                                ApplyUnit::Instructions,
                                ApplyUnit::Provider,
                            ],
                            format!("configuration commit refused: {error:?}"),
                        );
                        return;
                    }
                    state.publish(scope, identity, ApplyUnit::Capabilities, || {
                        UnitApplication::Applied
                    });
                    if context_unchanged {
                        if let Err(diagnostic) =
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
                        for unit in [ApplyUnit::Instructions, ApplyUnit::Provider] {
                            state.publish(scope, identity, unit, || UnitApplication::Applied);
                        }
                        self.notify(&state);
                        return;
                    }
                    self.notify(&state);
                    drop(state);
                    context_only = true;
                    bindings(&capture).and_then(|models| {
                        runtime.prepare_context_configuration(&capture, models, None)
                    })
                } else {
                    match bindings(&capture).and_then(|models| {
                        runtime.complete_candidate_context(&mut candidate, &capture, models)
                    }) {
                        Ok(()) => Ok(candidate),
                        Err(error) => Err(error),
                    }
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
        if !capability_failed
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
        let impact = prepared.impact;
        let baseline = prepared.baseline;
        let mut prepared = Some(prepared);
        let outcome = if impact == CacheImpact::Preserved {
            let retained = capture.clone().admit(|| manager.credentials.clone());
            match retained {
                Err(diagnostic) => UnitApplication::Failed { diagnostic },
                Ok(mut retained) => {
                    retained.binding_revision = baseline + 1;
                    match runtime.adopt_configuration(&mut prepared, baseline, false, || {
                        manager
                            .sessions
                            .configuration_bindings
                            .lock()
                            .expect("Session configuration bindings")
                            .insert(session.clone(), retained);
                        Ok(())
                    }) {
                        Ok(_) => UnitApplication::Applied,
                        Err(error) => UnitApplication::Failed {
                            diagnostic: format!("configuration commit refused: {error:?}"),
                        },
                    }
                }
            }
        } else {
            let current = runtime.with_configuration_baseline(
                prepared.as_ref().expect("prepared candidate"),
                || {
                    state
                        .scopes
                        .get_mut(scope)
                        .expect("current scope")
                        .candidate = Some(AvailableConfiguration {
                        identity: identity.clone(),
                        expected_binding: baseline,
                        impact,
                    });
                },
            );
            if current {
                state
                    .ready
                    .insert(scope.to_owned(), ReadyConfiguration { capture, prepared });
                UnitApplication::Ready { impact }
            } else {
                UnitApplication::Failed {
                    diagnostic: "Session binding changed during configuration preparation".into(),
                }
            }
        };
        for unit in [
            ApplyUnit::Capabilities,
            ApplyUnit::Instructions,
            ApplyUnit::Provider,
        ] {
            if capability_failed && unit != ApplyUnit::Instructions {
                continue;
            }
            let unit_outcome = if context_only && unit == ApplyUnit::Capabilities {
                UnitApplication::Applied
            } else {
                outcome.clone()
            };
            state.publish(scope, identity, unit, || unit_outcome);
        }
        self.notify(&state);
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

    pub(crate) fn adopt(
        &self,
        manager: &SessionRuntimeManager,
        session: &SessionId,
        identity: &ApplicationIdentity,
        expected_binding: u64,
    ) -> Result<ConfigurationApplication, AdoptionError> {
        let scope = session.to_string();
        let mut state = self.lock();
        if !state.current(&scope, identity) {
            return Err(AdoptionError::Conflict);
        }
        let ready = state.ready.get_mut(&scope).ok_or(AdoptionError::NotReady)?;
        let runtime = manager
            .configuration_runtime(session)
            .ok_or(AdoptionError::NotReady)?;
        let mut retained = ready
            .capture
            .clone()
            .admit(|| manager.credentials.clone())
            .map_err(|diagnostic| AdoptionError::Failed { diagnostic })?;
        retained.binding_revision = expected_binding + 1;
        let outcome =
            runtime.adopt_configuration(&mut ready.prepared, expected_binding, true, || {
                manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings")
                    .insert(session.clone(), retained);
                Ok(())
            });
        if let Err(error) = outcome {
            if ready.prepared.is_none() {
                state.ready.remove(&scope);
                state
                    .scopes
                    .get_mut(&scope)
                    .expect("current scope")
                    .candidate = None;
                for unit in [
                    ApplyUnit::Capabilities,
                    ApplyUnit::Instructions,
                    ApplyUnit::Provider,
                ] {
                    if matches!(
                        state.scopes[&scope].units[&unit],
                        UnitApplication::Ready { .. }
                    ) {
                        state.publish(&scope, identity, unit, || UnitApplication::Failed {
                            diagnostic: format!("configuration commit refused: {error:?}"),
                        });
                    }
                }
                self.notify(&state);
            }
            return Err(error);
        }
        state.ready.remove(&scope);
        for unit in [
            ApplyUnit::Capabilities,
            ApplyUnit::Instructions,
            ApplyUnit::Provider,
        ] {
            if matches!(
                state.scopes[&scope].units[&unit],
                UnitApplication::Ready { .. }
            ) {
                state.publish(&scope, identity, unit, || UnitApplication::Applied);
            }
        }
        state
            .scopes
            .get_mut(&scope)
            .expect("current scope")
            .candidate = None;
        self.notify(&state);
        let view = state.view(&scope).expect("current scope");
        drop(state);
        tokio::spawn(async move {
            runtime.settle_configuration_resources().await;
        });
        Ok(view)
    }
}
