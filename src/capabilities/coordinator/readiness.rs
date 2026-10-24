//! Native configuration candidates from already-owned startup observations.
//! No source I/O is restarted and a pending sibling never delays publication.

use super::{
    CapabilityCoordinator, CapabilityPreparationError, CapabilityResourceInputs,
    CapabilitySourceState, PreparedCapabilityCandidate, ToolSourceId, select_tools,
};
use std::collections::BTreeMap;
use std::sync::Arc;

impl CapabilityCoordinator {
    pub(crate) fn has_invalidated_mcp_catalogs(&self) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        let invalidation = self.inner.mcp_invalidation.lock();
        state
            .mcp_epochs
            .iter()
            .any(|(id, epoch)| invalidation.epoch(id) != *epoch)
    }

    pub(crate) fn has_connecting_mcp_sources(&self) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        state.availability.iter().any(|(source, availability)| {
            matches!(source, ToolSourceId::Mcp(_))
                && matches!(availability, CapabilitySourceState::Connecting { .. })
        })
    }

    /// An explicit host refresh may reconsider invalidated published catalogs
    /// or retry unavailable configured MCP sources without authored changes.
    pub(crate) fn needs_mcp_refresh(&self) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        let invalidation = self.inner.mcp_invalidation.lock();
        state
            .mcp_epochs
            .iter()
            .any(|(id, epoch)| invalidation.epoch(id) != *epoch)
            || state.availability.iter().any(|(source, availability)| {
                matches!(source, ToolSourceId::Mcp(id) if state.snapshot.mcp_servers().contains_key(id))
                    && matches!(availability, CapabilitySourceState::Unavailable { .. })
            })
    }

    pub(crate) fn has_startup_updates(&self) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        state.mcp_runtimes.iter().any(|generation| {
            matches!(
                state
                    .availability
                    .get(&ToolSourceId::Mcp(generation.server_id().clone())),
                Some(CapabilitySourceState::Connecting { .. })
            ) && generation.connection().catalog_snapshot().is_some()
        })
    }

    /// Captures terminal observations without borrowing physical ownership from
    /// the published generation. The ordinary revision/epoch commit fences
    /// decide whether those same owners can transfer to this candidate.
    pub(crate) async fn prepare_ready_captured_inputs(
        &self,
        inputs: CapabilityResourceInputs,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        self.prepare_owned(
            Some(cancellation),
            move |coordinator, cancellation| async move {
                if cancellation.is_cancelled() {
                    return Err(CapabilityPreparationError::ConversationInactive);
                }
                coordinator.capture_startup_readiness(inputs)
            },
        )
        .await
    }

    #[allow(clippy::too_many_lines)] // one immutable candidate assembled under its base-revision fence
    fn capture_startup_readiness(
        &self,
        inputs: CapabilityResourceInputs,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        let state = self.inner.state.lock().expect("capability state");
        let previous = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability inputs");
        if inputs.source_demand.sources != previous.source_demand.sources
            || inputs
                .source_demand
                .sources
                .iter()
                .any(|source| match source {
                    ToolSourceId::Mcp(id) => {
                        inputs.mcp_servers.get(id) != previous.mcp_servers.get(id)
                    }
                    ToolSourceId::ManagedPython(_) => false,
                })
        {
            return Err(CapabilityPreparationError::Mcp(
                "startup readiness no longer matches the admitted source configuration".into(),
            ));
        }
        let snapshot = &state.snapshot;
        let mut availability = state.availability.clone();
        let mut registrations = inputs.base_tool_registry.registrations();
        let mut retained_mcp = BTreeMap::new();
        let mut mcp_epochs = BTreeMap::new();
        for generation in &state.mcp_runtimes {
            let id = generation.server_id();
            let source = if previous.mcp_servers.contains_key(id) {
                ToolSourceId::Mcp(id.clone())
            } else {
                // Managed packages were fully materialized before composition.
                let Some(source) = inputs
                    .source_demand
                    .sources
                    .iter()
                    .find(|source| crate::tools::mcp::source_server_id(source) == *id)
                else {
                    continue;
                };
                source.clone()
            };
            let epoch = state.mcp_epochs[id];
            let terminal = matches!(
                availability.get(&source),
                Some(CapabilitySourceState::Connecting { .. })
            )
            .then(|| generation.connection().catalog_snapshot())
            .flatten();
            mcp_epochs.insert(
                id.clone(),
                terminal.as_ref().map_or(epoch, |(captured, _)| *captured),
            );
            match terminal {
                Some((_, Ok(tools))) => {
                    let binding = &snapshot.mcp_servers()[id];
                    registrations.extend(
                        crate::tools::mcp::definitions_owned_for_source(
                            &source,
                            id,
                            binding.policy,
                            &generation.binding(),
                            tools,
                        )
                        .into_iter()
                        .map(|(definition, executor)| {
                            crate::tools::executor::ToolRegistration::plain(definition, executor)
                        }),
                    );
                    availability.insert(source, CapabilitySourceState::Ready);
                }
                Some((_, Err(reason))) => {
                    // The current Attempt keeps its frozen definitions. The
                    // candidate reports failure and removes these declarations
                    // only when the normal native adoption boundary accepts it.
                    availability.insert(source, CapabilitySourceState::unavailable(reason));
                    continue;
                }
                None => {
                    registrations.extend(
                        snapshot
                            .available_tools()
                            .registrations()
                            .iter()
                            .filter(|entry| {
                                entry.definition.origin.source().as_ref() == Some(&source)
                            })
                            .cloned(),
                    );
                }
            }
            retained_mcp.insert(id.clone(), generation.binding());
        }
        let (available_tools, registry, profile) = select_tools(
            &registrations,
            self.inner.extension_tools.registrations(),
            &inputs.agent_activation,
            snapshot.skills(),
            &availability,
        )
        .map_err(CapabilityPreparationError::ToolActivation)?;
        Ok(PreparedCapabilityCandidate {
            resolved_profile: Some(Arc::new(profile)),
            base_revision: state.revision,
            skills: snapshot.skills().clone(),
            python: snapshot.python_environment().cloned(),
            node: snapshot.node_environment().cloned(),
            effective_environment: snapshot.effective_environment().clone(),
            candidate_registry: Arc::new(registry),
            available_tools: Arc::new(available_tools),
            mcp_epochs,
            mcp_hints: BTreeMap::new(),
            retained_mcp,
            availability,
            mcp_runtimes: Vec::new(),
            effective_mcp_servers: snapshot.mcp_servers().clone(),
            resource_inputs: inputs,
            force_publish: true,
        })
    }
}
