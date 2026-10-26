//! Live catalog publication from the exact connections the adopted generation owns.
//! Discovery and retry belong to the connection owner; this capture performs no I/O.

use super::{
    CapabilityCoordinator, CapabilityPreparationError, CapabilitySourceState,
    PreparedCapabilityCandidate, ToolSourceId, select_tools,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The exact adopted source/physical-owner cut used by an explicit recovery.
/// The immutable baseline distinguishes a later sibling catalog from the
/// fresh owners this recovery has prepared but not yet published.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeRefreshAuthority {
    sources: std::collections::BTreeSet<ToolSourceId>,
    identities: BTreeMap<ToolSourceId, Arc<()>>,
    inputs: super::CapabilityResourceInputs,
    snapshot: Arc<super::CapabilitySnapshot>,
    mcp_epochs: BTreeMap<crate::runtime::identity::McpServerId, u64>,
    owners: BTreeMap<crate::runtime::identity::McpServerId, crate::tools::mcp::McpRuntimeBinding>,
}

impl RuntimeRefreshAuthority {
    fn capture(state: &super::CoordinatorState, inputs: super::CapabilityResourceInputs) -> Self {
        Self {
            sources: inputs.source_demand.sources.clone(),
            identities: state.runtime_source_authorities.clone(),
            inputs,
            snapshot: state.snapshot.clone(),
            mcp_epochs: state.mcp_epochs.clone(),
            owners: state
                .mcp_runtimes
                .iter()
                .map(|generation| (generation.server_id().clone(), generation.binding()))
                .collect(),
        }
    }

    fn source_is_current(
        &self,
        source: &ToolSourceId,
        state: &super::CoordinatorState,
        inputs: &super::CapabilityResourceInputs,
    ) -> bool {
        let id = crate::tools::mcp::source_server_id(source);
        if !matches!(
            (self.identities.get(source), state.runtime_source_authorities.get(source)),
            (Some(previous), Some(current)) if Arc::ptr_eq(previous, current)
        ) || !same_source_authorization(
            source,
            self.snapshot.mcp_servers(),
            &self.inputs,
            state.snapshot.mcp_servers(),
            inputs,
        ) {
            return false;
        }
        match (
            self.owners.get(&id),
            state
                .mcp_runtimes
                .iter()
                .find(|generation| generation.server_id() == &id),
        ) {
            (None, None) => true,
            (Some(previous), Some(current)) => {
                Arc::ptr_eq(previous.connection(), current.connection())
            }
            _ => false,
        }
    }
}

impl CapabilityCoordinator {
    pub(crate) fn catalog_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.mcp_invalidation.subscribe_catalog_changes()
    }

    pub(crate) fn catalog_revision(&self) -> u64 {
        *self.catalog_changes().borrow()
    }

    /// An explicit host refresh may retry an unavailable or invalidated source.
    pub(crate) fn needs_mcp_refresh(&self) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        let invalidation = self.inner.mcp_invalidation.lock();
        state.mcp_epochs.iter().any(|(id, epoch)| invalidation.epoch(id) != *epoch)
            || state.availability.iter().any(|(source, availability)| {
                matches!(source, ToolSourceId::Mcp(id) if state.snapshot.mcp_servers().contains_key(id))
                    && matches!(availability, CapabilitySourceState::Unavailable { .. })
            })
    }

    /// Capture source authority without I/O while the native admission owner is held.
    pub(crate) fn runtime_refresh_authority(&self) -> RuntimeRefreshAuthority {
        let state = self.inner.state.lock().expect("capability state");
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability inputs")
            .clone();
        RuntimeRefreshAuthority::capture(&state, inputs)
    }

    /// All scoped facts, including a failed-source diagnostic, still belong to
    /// their exact adopted bindings, authorization and physical owners.
    pub(crate) fn runtime_refresh_is_current(&self, authority: &RuntimeRefreshAuthority) -> bool {
        let state = self.inner.state.lock().expect("capability state");
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability inputs");
        authority
            .sources
            .iter()
            .all(|source| authority.source_is_current(source, &state, &inputs))
    }

    /// A native runtime-only publication can replace an older physical owner
    /// while retaining a newer explicit failure. Preserve that failure's source
    /// scope while anchoring it to the exact resulting adopted owner cut.
    pub(crate) fn reanchor_runtime_refresh_authority(
        &self,
        authority: &RuntimeRefreshAuthority,
    ) -> RuntimeRefreshAuthority {
        let mut current = self.runtime_refresh_authority();
        current.sources.clone_from(&authority.sources);
        current
    }

    /// Explicit runtime recovery freezes the already-admitted source and Skill
    /// inputs. Both successful and failed completion carry that same authority;
    /// a completion cannot install declarations or diagnostics under new intent.
    pub(crate) async fn prepare_runtime_refresh(
        &self,
    ) -> (
        RuntimeRefreshAuthority,
        Result<PreparedCapabilityCandidate, CapabilityPreparationError>,
    ) {
        let (inputs, mut authority, discovered) = {
            let state = self.inner.state.lock().expect("capability state");
            let inputs = self
                .inner
                .resource_inputs
                .lock()
                .expect("capability inputs")
                .clone();
            let authority = RuntimeRefreshAuthority::capture(&state, inputs.clone());
            let skills = state.snapshot.skills();
            let discovered = crate::skills::SkillDiscoveryOutcome {
                invalid: Vec::new(),
                roots: skills.roots().to_vec(),
                packages: skills
                    .packages()
                    .iter()
                    .map(|package| (**package).clone())
                    .collect(),
                provenance: skills.provenance().to_vec(),
                diagnostics: skills.diagnostics().to_vec(),
            };
            (inputs, authority, discovered)
        };
        let result = async {
            let candidate = self
                .prepare_candidate_from_inputs(
                    inputs,
                    true,
                    authority.snapshot.revision(),
                    Some(discovered),
                    None,
                )
                .await?;
            let failed = candidate
                .availability()
                .iter()
                .filter_map(|(source, state)| match state {
                    CapabilitySourceState::Unavailable { reason } => {
                        Some((source.clone(), reason.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !failed.is_empty() {
                authority.sources = failed.iter().map(|(source, _)| source.clone()).collect();
                let reason = failed[0].1.clone();
                candidate.retire_uncommitted().await;
                return Err(CapabilityPreparationError::Mcp(reason));
            }
            Ok(candidate)
        }
        .await;
        (authority, result)
    }

    /// Rebase runtime facts at the native admission cut without repeating I/O.
    /// Non-MCP resources always come from the current adopted generation.
    /// Fresh physical owners overlay only their still-exact source authority;
    /// sibling catalog progress and unrelated authored adoption remain intact.
    #[allow(clippy::too_many_lines)] // One source-authority merge and selection at the native cut.
    pub(crate) fn rebase_runtime_refresh(
        &self,
        authority: &RuntimeRefreshAuthority,
        candidate: &mut PreparedCapabilityCandidate,
    ) -> Result<bool, CapabilityPreparationError> {
        let state = self.inner.state.lock().expect("capability state");
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability inputs")
            .clone();
        let eligible = authority
            .sources
            .iter()
            .filter(|source| authority.source_is_current(source, &state, &inputs))
            .map(crate::tools::mcp::source_server_id)
            .collect::<std::collections::BTreeSet<_>>();
        if eligible.is_empty() && !authority.sources.is_empty() {
            return Ok(false);
        }
        // The ordinary catalog omits profile-composed dispatchers. Preserve
        // the adopted base as well, so runtime facts cannot erase Agent
        // controls when no Workflow admission happens to recompose them.
        let mut registrations = inputs.base_tool_registry.registrations();
        for registration in state.snapshot.available_tools().registrations() {
            if let Some(existing) = registrations
                .iter_mut()
                .find(|entry| entry.definition.id == registration.definition.id)
            {
                existing.clone_from(registration);
            } else {
                registrations.push(registration.clone());
            }
        }
        let mut availability = state.availability.clone();
        let mut epochs = state.mcp_epochs.clone();
        let mut bindings = state.snapshot.mcp_servers().clone();
        let mut retained = BTreeMap::new();
        let mut resource_bindings = BTreeMap::new();
        let mut advanced = std::collections::BTreeSet::new();
        for generation in &state.mcp_runtimes {
            let id = generation.server_id();
            let Some(source) = inputs
                .source_demand
                .sources
                .iter()
                .find(|source| crate::tools::mcp::source_server_id(source) == *id)
            else {
                continue;
            };
            retained.insert(id.clone(), generation.binding());
            if state.snapshot.resource_sources().contains(source) {
                resource_bindings.insert(source.clone(), generation.binding());
            }
            if let Some((epoch, Ok(discovery))) = generation.connection().catalog_snapshot() {
                let resources_changed = state.snapshot.resource_sources().contains(source)
                    != discovery.capabilities.resources;
                if discovery.capabilities.resources {
                    resource_bindings.insert(source.clone(), generation.binding());
                } else {
                    resource_bindings.remove(source);
                }
                let live = live_registrations(source, generation, &bindings[id], discovery.tools);
                if authority.mcp_epochs.get(id) != Some(&epoch)
                    || resources_changed
                    || source_definitions(&authority.snapshot, source)
                        != live
                            .iter()
                            .map(|entry| &entry.definition)
                            .collect::<Vec<_>>()
                {
                    advanced.insert(id.clone());
                }
                registrations
                    .retain(|entry| entry.definition.origin.source().as_ref() != Some(source));
                registrations.extend(live);
                epochs.insert(id.clone(), epoch);
                availability.insert(source.clone(), CapabilitySourceState::Ready);
            }
        }
        let fresh = candidate
            .mcp_runtimes
            .iter()
            .filter(|generation| {
                eligible.contains(generation.server_id())
                    && !advanced.contains(generation.server_id())
            })
            .map(|generation| generation.server_id().clone())
            .collect::<std::collections::BTreeSet<_>>();
        for id in &fresh {
            let source = authority
                .sources
                .iter()
                .find(|source| crate::tools::mcp::source_server_id(source) == *id)
                .expect("a fresh owner has captured source authority");
            registrations.retain(|entry| entry.definition.origin.source().as_ref() != Some(source));
            registrations.extend(
                candidate
                    .available_tools
                    .registrations()
                    .iter()
                    .filter(|entry| entry.definition.origin.source().as_ref() == Some(source))
                    .cloned(),
            );
            epochs.insert(id.clone(), candidate.mcp_epochs[id]);
            bindings.insert(id.clone(), candidate.effective_mcp_servers[id].clone());
            if let Some(binding) = candidate.resource_bindings.get(source) {
                resource_bindings.insert(source.clone(), binding.clone());
            } else {
                resource_bindings.remove(source);
            }
            availability.insert(source.clone(), CapabilitySourceState::Ready);
            retained.remove(id);
        }
        let (available, registry, profile) = select_tools(
            &registrations,
            self.inner.extension_tools.registrations(),
            &resource_bindings,
            &inputs.agent_activation,
            state.snapshot.skills(),
            &availability,
        )
        .map_err(CapabilityPreparationError::ToolActivation)?;
        candidate
            .mcp_runtimes
            .retain(|generation| fresh.contains(generation.server_id()));
        candidate.retained_mcp = retained;
        candidate.force_publish = !candidate.mcp_runtimes.is_empty();
        candidate.base_revision = state.revision;
        candidate.resource_inputs = inputs;
        candidate.skills = state.snapshot.skills().clone();
        candidate.python = state.snapshot.python_environment().cloned();
        candidate.node = state.snapshot.node_environment().cloned();
        candidate.effective_environment = state.snapshot.effective_environment().clone();
        candidate.mcp_epochs = epochs;
        candidate.effective_mcp_servers = bindings;
        candidate.availability = availability;
        candidate.available_tools = Arc::new(available);
        candidate.resource_bindings = resource_bindings;
        candidate.candidate_registry = Arc::new(registry);
        candidate.resolved_profile = Some(Arc::new(profile));
        Ok(true)
    }

    /// Capture successful live observations only from adopted connection owners.
    /// A candidate for a different authored source cannot contribute declarations.
    /// Failed discovery leaves the complete last-good generation authoritative.
    #[allow(
        clippy::too_many_lines,
        reason = "one atomic capture of tool and resource facts under the publication lock"
    )]
    pub(crate) fn capture_runtime_catalogs(
        &self,
    ) -> Result<Option<PreparedCapabilityCandidate>, CapabilityPreparationError> {
        let state = self.inner.state.lock().expect("capability state");
        let inputs = self
            .inner
            .resource_inputs
            .lock()
            .expect("capability inputs")
            .clone();
        let snapshot = &state.snapshot;
        let mut availability = state.availability.clone();
        let mut registrations = inputs.base_tool_registry.registrations();
        let mut retained_mcp = BTreeMap::new();
        let mut resource_bindings = BTreeMap::new();
        let mut mcp_epochs = state.mcp_epochs.clone();
        let mut changed = false;
        for generation in &state.mcp_runtimes {
            let id = generation.server_id();
            let source = if inputs.mcp_servers.contains_key(id) {
                ToolSourceId::Mcp(id.clone())
            } else {
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
            let previous = snapshot
                .available_tools()
                .registrations()
                .iter()
                .filter(|entry| entry.definition.origin.source().as_ref() == Some(&source))
                .cloned()
                .collect::<Vec<_>>();
            if snapshot.resource_sources().contains(&source) {
                resource_bindings.insert(source.clone(), generation.binding());
            }
            if let Some((epoch, Ok(discovery))) = generation.connection().catalog_snapshot() {
                changed |= snapshot.resource_sources().contains(&source)
                    != discovery.capabilities.resources;
                if discovery.capabilities.resources {
                    resource_bindings.insert(source.clone(), generation.binding());
                } else {
                    resource_bindings.remove(&source);
                }
                let binding = &snapshot.mcp_servers()[id];
                let live = crate::tools::mcp::definitions_owned_for_source(
                    &source,
                    id,
                    binding.policy,
                    &generation.binding(),
                    discovery.tools,
                )
                .into_iter()
                .map(|(definition, executor)| {
                    crate::tools::executor::ToolRegistration::plain(definition, executor)
                })
                .collect::<Vec<_>>();
                changed |= previous
                    .iter()
                    .map(|tool| &tool.definition)
                    .collect::<Vec<_>>()
                    != live.iter().map(|tool| &tool.definition).collect::<Vec<_>>()
                    || state.mcp_epochs.get(id) != Some(&epoch)
                    || availability.get(&source) != Some(&CapabilitySourceState::Ready);
                mcp_epochs.insert(id.clone(), epoch);
                availability.insert(source, CapabilitySourceState::Ready);
                registrations.extend(live);
            } else {
                registrations.extend(previous);
            }
            retained_mcp.insert(id.clone(), generation.binding());
        }
        if !changed {
            return Ok(None);
        }
        let (available_tools, registry, profile) = select_tools(
            &registrations,
            self.inner.extension_tools.registrations(),
            &resource_bindings,
            &inputs.agent_activation,
            snapshot.skills(),
            &availability,
        )
        .map_err(CapabilityPreparationError::ToolActivation)?;
        Ok(Some(PreparedCapabilityCandidate {
            resolved_profile: Some(Arc::new(profile)),
            base_revision: state.revision,
            skills: snapshot.skills().clone(),
            python: snapshot.python_environment().cloned(),
            node: snapshot.node_environment().cloned(),
            effective_environment: snapshot.effective_environment().clone(),
            candidate_registry: Arc::new(registry),
            available_tools: Arc::new(available_tools),
            resource_bindings,
            mcp_epochs,
            retained_mcp,
            availability,
            mcp_runtimes: Vec::new(),
            effective_mcp_servers: snapshot.mcp_servers().clone(),
            resource_inputs: inputs,
            force_publish: false,
        }))
    }
}

fn source_definitions<'a>(
    snapshot: &'a super::CapabilitySnapshot,
    source: &ToolSourceId,
) -> Vec<&'a crate::tools::types::ToolDefinition> {
    snapshot
        .available_tools()
        .registrations()
        .iter()
        .filter(|entry| entry.definition.origin.source().as_ref() == Some(source))
        .map(|entry| &entry.definition)
        .collect()
}

fn live_registrations(
    source: &ToolSourceId,
    generation: &crate::tools::mcp::McpRuntimeGeneration,
    binding: &crate::tools::mcp::McpServerBinding,
    tools: Vec<crate::tools::mcp::CanonicalMcpTool>,
) -> Vec<crate::tools::executor::ToolRegistration> {
    crate::tools::mcp::definitions_owned_for_source(
        source,
        generation.server_id(),
        binding.policy,
        &generation.binding(),
        tools,
    )
    .into_iter()
    .map(|(definition, executor)| {
        crate::tools::executor::ToolRegistration::plain(definition, executor)
    })
    .collect()
}

fn same_source_authorization(
    source: &ToolSourceId,
    previous_bindings: &crate::tools::mcp::McpServerBindings,
    previous: &super::CapabilityResourceInputs,
    current_bindings: &crate::tools::mcp::McpServerBindings,
    current: &super::CapabilityResourceInputs,
) -> bool {
    let id = crate::tools::mcp::source_server_id(source);
    previous.source_demand.sources.contains(source)
        && current.source_demand.sources.contains(source)
        && previous_bindings.get(&id) == current_bindings.get(&id)
        && previous.agent_activation.profile.tools.sources.get(source)
            == current.agent_activation.profile.tools.sources.get(source)
        && previous.source_demand.managed_python.packages().get(source)
            == current.source_demand.managed_python.packages().get(source)
}

pub(super) fn next_source_authorities(
    state: &super::CoordinatorState,
    previous_inputs: &super::CapabilityResourceInputs,
    candidate: &PreparedCapabilityCandidate,
    installs_owners: bool,
) -> BTreeMap<ToolSourceId, Arc<()>> {
    candidate
        .resource_inputs
        .source_demand
        .sources
        .iter()
        .map(|source| {
            let id = crate::tools::mcp::source_server_id(source);
            let previous = state
                .mcp_runtimes
                .iter()
                .find(|generation| generation.server_id() == &id)
                .map(crate::tools::mcp::McpRuntimeGeneration::connection);
            let current = if installs_owners {
                candidate
                    .mcp_runtimes
                    .iter()
                    .find(|generation| generation.server_id() == &id)
                    .map(crate::tools::mcp::McpRuntimeGeneration::connection)
                    .or_else(|| {
                        candidate
                            .retained_mcp
                            .get(&id)
                            .map(crate::tools::mcp::McpRuntimeBinding::connection)
                    })
            } else {
                previous
            };
            let same_owner = match (previous, current) {
                (None, None) => true,
                (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
                _ => false,
            };
            let identity = if same_owner
                && same_source_authorization(
                    source,
                    state.snapshot.mcp_servers(),
                    previous_inputs,
                    &candidate.effective_mcp_servers,
                    &candidate.resource_inputs,
                ) {
                state
                    .runtime_source_authorities
                    .get(source)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(()))
            } else {
                Arc::new(())
            };
            (source.clone(), identity)
        })
        .collect()
}
