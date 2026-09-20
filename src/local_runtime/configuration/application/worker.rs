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
                self.fail(scope, identity, diagnostic);
                return;
            }
        };
        if manager.configuration_runtime(&session).is_none()
            && let Err(error) = manager.load(&session, None).await
        {
            self.fail(
                scope,
                identity,
                format!("Session configuration preparation: {error}"),
            );
            return;
        }
        let Some(runtime) = manager.configuration_runtime(&session) else {
            self.fail(
                scope,
                identity,
                "Session retired during configuration preparation".into(),
            );
            return;
        };
        {
            let mut state = self.lock();
            state.publish(scope, identity, ApplyUnit::ProcessBindings, || {
                if manager.apply_process_limits(&process) {
                    UnitApplication::ProcessRestart
                } else {
                    UnitApplication::Applied
                }
            });
            state.publish(scope, identity, ApplyUnit::SharedCapacity, || {
                runtime.apply_shared_capacity(&policy);
                UnitApplication::Applied
            });
            state.publish(
                scope,
                identity,
                ApplyUnit::ExecutionPolicy,
                || match runtime.apply_execution_policy(&policy) {
                    Ok(_) => UnitApplication::Applied,
                    Err(diagnostic) => UnitApplication::Failed { diagnostic },
                },
            );
            if state.current(scope, identity)
                && state.scopes[scope].units[&ApplyUnit::ExecutionPolicy]
                    == UnitApplication::Applied
            {
                let mut bindings = manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings");
                if let Some(retained) = bindings.get_mut(&session) {
                    *retained = retained.with_execution_policy(&policy);
                }
            }
            self.notify(&state);
            if !state.current(scope, identity) {
                return;
            }
        }
        let capture = match capture {
            Ok(capture) => capture,
            Err(diagnostic) => {
                self.fail(scope, identity, diagnostic);
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
        let mut context_only = adopted
            .as_ref()
            .is_some_and(|old| capture.same_capabilities(old));
        let bindings = || {
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
        if manager.configuration_test_gate(&session, false).await {
            self.fail(scope, identity, "injected preparation failure".into());
            return;
        }
        let prepared = if context_only {
            bindings()
                .and_then(|models| runtime.prepare_context_configuration(&capture, models, None))
        } else {
            let capability_capture = adopted.as_ref().map_or_else(
                || capture.clone(),
                |old| capture.clone().retaining_context_from(old),
            );
            let cancellation = crate::runtime::cancellation::CancellationSignal::new();
            let mut preparation =
                Box::pin(runtime.prepare_configuration(capability_capture.clone(), &cancellation));
            let prepared =
                tokio::time::timeout(std::time::Duration::from_mins(2), &mut preparation).await;
            if prepared.is_err() {
                // Keep the only preparation slot until its physical owners have
                // acknowledged cancellation; dropping a waiter is not settlement.
                cancellation.cancel();
                drop(preparation.await);
            }
            let mut candidate = match prepared {
                Ok(Ok(candidate)) => candidate,
                result => {
                    let diagnostic = match result {
                        Ok(Err(error)) => error,
                        _ => "configuration preparation deadline exceeded".into(),
                    };
                    self.fail(scope, identity, diagnostic);
                    return;
                }
            };
            if candidate.impact == CacheImpact::Preserved {
                let mut state = self.lock();
                if !state.current(scope, identity) {
                    return;
                }
                let context_unchanged = capture.same_context(&capability_capture)
                    && capture.same_provider(&capability_capture);
                let mut retained = match capability_capture.admit(|| manager.credentials.clone()) {
                    Ok(retained) => retained,
                    Err(diagnostic) => {
                        drop(state);
                        self.fail(scope, identity, diagnostic);
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
                    self.fail(
                        scope,
                        identity,
                        format!("configuration commit refused: {error:?}"),
                    );
                    return;
                }
                state.publish(scope, identity, ApplyUnit::Capabilities, || {
                    UnitApplication::Applied
                });
                if context_unchanged {
                    for unit in [ApplyUnit::Instructions, ApplyUnit::Provider] {
                        state.publish(scope, identity, unit, || UnitApplication::Applied);
                    }
                    self.notify(&state);
                    return;
                }
                self.notify(&state);
                drop(state);
                context_only = true;
                bindings().and_then(|models| {
                    runtime.prepare_context_configuration(&capture, models, None)
                })
            } else {
                match bindings().and_then(|models| {
                    runtime.complete_candidate_context(&mut candidate, &capture, models)
                }) {
                    Ok(()) => Ok(candidate),
                    Err(error) => Err(error),
                }
            }
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(diagnostic) => {
                self.fail(scope, identity, diagnostic);
                return;
            }
        };
        #[cfg(test)]
        manager.configuration_test_gate(&session, true).await;
        let mut state = self.lock();
        if !state.current(scope, identity) {
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
    fn fail(&self, scope: &str, identity: &ApplicationIdentity, diagnostic: String) {
        let mut state = self.lock();
        let units: Vec<_> = state
            .view(scope)
            .into_iter()
            .flat_map(|view| view.units)
            .filter_map(|(unit, result)| {
                matches!(result, UnitApplication::Preparing).then_some(unit)
            })
            .collect();
        for unit in units {
            state.publish(scope, identity, unit, || UnitApplication::Failed {
                diagnostic: diagnostic.clone(),
            });
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
                    state.publish(&scope, identity, unit, || UnitApplication::Failed {
                        diagnostic: format!("configuration commit refused: {error:?}"),
                    });
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
            state.publish(&scope, identity, unit, || UnitApplication::Applied);
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
