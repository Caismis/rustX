//! Comparison, composition and admission of one immutable prospective
//! Session capture. Nothing here reads a source document again.
use super::units::{copy_unit_provenance, owns, unit_keys};
use super::*;

impl ProspectiveSessionConfig {
    /// The physical/profile closure admitted by Root selection and Workflow
    /// Agent nodes (including nested nodes and invocation overrides). Unrelated
    /// catalog entries remain inert.
    pub(in crate::local_runtime) fn admitted_agent_dependencies(
        &self,
    ) -> std::collections::BTreeSet<crate::runtime::subagent::SubagentName> {
        let mut names: std::collections::BTreeSet<_> =
            self.config.agent.agents.iter().cloned().collect();
        for id in &self.config.agent.workflows {
            if let Some(entry) = self.workflows.entries().get(id) {
                names.extend(
                    entry
                        .source
                        .agent_nodes()
                        .iter()
                        .map(|node| node.profile.clone()),
                );
            }
        }
        names
    }

    /// Compare independently prepared capability inputs, excluding context and
    /// provider units. Authored revisions and shadowed/unselected metadata are
    /// not effective resource identities.
    pub(crate) fn same_capabilities(&self, other: &Self) -> bool {
        let normalized = |source: &Self| {
            let mut config = source.config.as_ref().clone();
            config.approval_mode = crate::runtime::ApprovalMode::default();
            config.model_timeout_policy =
                super::super::config::ModelTimeoutPolicyDocument::default();
            config.tool_deadline_policy =
                super::super::config::ToolDeadlinePolicyDocument::default();
            config.subagents = super::super::config::SubagentsDocument::default();
            config.context = super::super::config::ContextPolicyDocument::default();
            config.agent.instructions.clear();
            config.agent.model = None;
            config.agent.agents_md =
                super::super::config::AgentProjectInstructionsDocument::default();
            // Inert definitions are not resource demand.
            let demand = super::super::composition::admitted_source_demand(
                &config,
                &source.subagents,
                &source.workflows,
                source.managed_python.clone(),
            );
            config.mcp_servers.retain(|id, _| {
                demand
                    .sources
                    .contains(&crate::capabilities::ToolSourceId::Mcp(id.clone()))
            });
            (config, demand.sources)
        };
        let (a, demand_a) = normalized(self);
        let (b, demand_b) = normalized(other);
        if a != b || demand_a != demand_b {
            return false;
        }
        let agents = self.admitted_agent_dependencies();
        if agents != other.admitted_agent_dependencies()
            || agents
                .iter()
                .any(|name| self.subagents.get(name) != other.subagents.get(name))
        {
            return false;
        }
        if a.agent.workflows.iter().any(|id| {
            let view = |source: &Self| {
                source
                    .workflows
                    .entries()
                    .get(id)
                    .map(|entry| entry.source.tool_identity())
            };
            view(self) != view(other)
        }) {
            return false;
        }
        if demand_a.iter().any(|id| {
            self.managed_python.packages().get(id) != other.managed_python.packages().get(id)
        }) {
            return false;
        }
        // Skill dependencies are materialized as a shared environment, so even
        // non-advertised discovered packages participate in this closure today.
        self.skill_discovery.packages == other.skill_discovery.packages
    }

    pub(crate) fn same_context(&self, other: &Self) -> bool {
        self.config.agent.instructions == other.config.agent.instructions
            && self.config.agent.agents_md == other.config.agent.agents_md
            && self.config.context == other.config.context
            && self.project_context_files == other.project_context_files
            && self.root_agent_project_files == other.root_agent_project_files
    }

    /// Whether every invocation an Attempt of this Session would freeze is
    /// effectively unchanged: the primary and explicit Summary selection, and
    /// those of every admitted named Agent. Compared as resolved invocations
    /// ([`ModelCatalog::same_invocation`]), so authored spelling and
    /// unselected catalog entries never force a preparation, while any
    /// provider binding, wire model, limit, Profile, reasoning or parameter
    /// change does. Catalog edits reach future selections separately, through
    /// catalog publication ([`Self::same_catalog`]).
    pub(crate) fn same_provider(&self, other: &Self) -> bool {
        self.same_session_model(other) && self.same_agent_models(other)
    }

    /// Whether the Session's own primary and explicit Summary selections
    /// resolve to effectively the same invocations here as in `other`.
    pub(crate) fn same_session_model(&self, other: &Self) -> bool {
        self.same_model(self.session_model(), other, other.session_model())
    }

    /// Whether two complete closures admit the same named Agents with
    /// effectively the same model invocations.
    fn same_agent_models(&self, other: &Self) -> bool {
        let agents = self.admitted_agent_dependencies();
        agents == other.admitted_agent_dependencies()
            && agents.iter().all(|name| {
                let model = |source: &Self| {
                    source
                        .subagents
                        .get(name)
                        .and_then(|definition| definition.profile().model.clone())
                };
                match (model(self), model(other)) {
                    (None, None) => true,
                    (Some(left), Some(right)) => self.same_model(&left, other, &right),
                    _ => false,
                }
            })
    }

    /// Whether publishing this capture's catalog leaves every named Agent
    /// invocation of the `adopted` closure unchanged. Which Agents are admitted
    /// and how they are defined stays the adopted closure's until Capability
    /// adoption, so an Agent this source newly admits, drops or redefines is
    /// that unit's change, never one to an adopted invocation. An adopted Agent
    /// naming no model inherits the invoking Attempt's frozen model; one naming
    /// a selection resolves it against the catalog the Session publishes, so
    /// that selection must resolve to the same invocation here.
    pub(crate) fn preserves_adopted_agent_models(&self, adopted: &Self) -> bool {
        adopted.admitted_agent_dependencies().iter().all(|name| {
            adopted
                .subagents
                .get(name)
                .and_then(|definition| definition.profile().model.clone())
                .is_none_or(|model| adopted.same_model(&model, self, &model))
        })
    }

    fn same_model(
        &self,
        left: &crate::model::session::SessionModelConfig,
        other: &Self,
        right: &crate::model::session::SessionModelConfig,
    ) -> bool {
        use crate::model::invocation::RequestParamsLayer;
        self.models.same_invocation(
            &left.selection(),
            &other.models,
            &right.selection(),
            RequestParamsLayer::SessionOverrides,
        ) && match (left.summary_selection(), right.summary_selection()) {
            (None, None) => true,
            (Some(left), Some(right)) => self.models.same_invocation(
                &left,
                &other.models,
                &right,
                RequestParamsLayer::SummaryOverrides,
            ),
            _ => false,
        }
    }

    /// Why this capture's catalog does not admit the Session's own selection,
    /// when it does not. Such a capture is still a valid source generation:
    /// the selection is the Session's to correct against this catalog, so it
    /// is published and the selection reported unavailable, never re-chosen.
    pub(crate) fn selection_unavailable(&self) -> Option<String> {
        crate::model::session::analyze_session_model_config(&self.models, self.session_model())
            .err()
            .map(|error| error.to_string())
    }

    /// Whether this capture publishes the same Model Catalog as `other`: the
    /// Provider unit — the selectable Models with their Profiles, the
    /// providers they bind, and the `agent.model` default the catalog is
    /// validated with — regardless of what any Session selection resolves
    /// to. A difference while every Session invocation is unchanged (or the
    /// Session's selection is no longer admitted) is a catalog-only
    /// publication: future selection changes, the current invocation does not.
    pub(crate) fn same_catalog(&self, other: &Self) -> bool {
        let provenance = |source: &BTreeMap<String, Origin>| {
            source
                .iter()
                .filter(|(key, _)| owns(unit_keys(application::ApplyUnit::Provider), key))
                .map(|(key, origin)| (key.clone(), origin.clone()))
                .collect::<Vec<_>>()
        };
        let default = |source: &Self| {
            source
                .effective
                .agent
                .as_ref()
                .and_then(|agent| agent.model.clone())
        };
        self.models == other.models
            && self.config.agent.model == other.config.agent.model
            && self.effective.models == other.effective.models
            && self.effective.providers == other.effective.providers
            && default(self) == default(other)
            && provenance(&self.provenance) == provenance(&other.provenance)
    }

    /// This binding with `published`'s Model Catalog (the whole Provider unit
    /// and its component revision) and every other input, including the
    /// Session's own selection and the adopted-source baseline
    /// (`source_revisions`), retained.
    pub(crate) fn with_published_catalog(mut self, published: &Self) -> Self {
        self.models = published.models.clone();
        std::sync::Arc::make_mut(&mut self.config)
            .agent
            .model
            .clone_from(&published.config.agent.model);
        self.effective
            .models
            .clone_from(&published.effective.models);
        self.effective
            .providers
            .clone_from(&published.effective.providers);
        self.effective
            .agent
            .get_or_insert_with(Default::default)
            .model = published
            .effective
            .agent
            .as_ref()
            .and_then(|agent| agent.model.clone());
        copy_unit_provenance(
            &mut self.provenance,
            &published.provenance,
            application::ApplyUnit::Provider,
        );
        self.component_revisions.insert(
            application::ApplyUnit::Provider,
            published.component_revisions[&application::ApplyUnit::Provider].clone(),
        );
        self
    }

    pub(crate) fn retaining_context_from(mut self, adopted: &Self) -> Self {
        let config = std::sync::Arc::make_mut(&mut self.config);
        config
            .agent
            .instructions
            .clone_from(&adopted.config.agent.instructions);
        config
            .agent
            .agents_md
            .clone_from(&adopted.config.agent.agents_md);
        config.context = adopted.config.context;
        self.project_context_files
            .clone_from(&adopted.project_context_files);
        self.root_agent_project_files
            .clone_from(&adopted.root_agent_project_files);
        // These paths are exclusively the Workspace-authored agents_md inputs
        // retained for physical-authority validation after path rebasing.
        self.project_resources
            .clone_from(&adopted.project_resources);
        let context = self.effective.agent.get_or_insert_with(Default::default);
        context.instructions = adopted
            .effective
            .agent
            .as_ref()
            .and_then(|agent| agent.instructions.clone());
        context.agents_md = adopted
            .effective
            .agent
            .as_ref()
            .and_then(|agent| agent.agents_md.clone());
        self.effective
            .context
            .clone_from(&adopted.effective.context);
        copy_unit_provenance(
            &mut self.provenance,
            &adopted.provenance,
            application::ApplyUnit::Instructions,
        );
        self.component_revisions.insert(
            application::ApplyUnit::Instructions,
            adopted.component_revisions[&application::ApplyUnit::Instructions].clone(),
        );
        self
    }

    /// Capture process credentials after coherent User and Workspace resolution.
    /// # Errors
    /// Rejects changed physical resource authority before credential capture.
    pub fn admit(
        self,
        credentials: impl FnOnce() -> crate::credentials::CredentialSnapshot,
    ) -> Result<AdmittedSessionConfig, String> {
        self.validate_resource_authority()?;
        Ok(AdmittedSessionConfig {
            binding_revision: 1,
            credentials: credentials(),
            prospective: Box::new(self),
        })
    }
    /// Recheck physical targets immediately before resource preparation. This
    /// never rereads launch documents and is not an execution filesystem sandbox.
    pub(crate) fn validate_resource_authority(&self) -> Result<(), String> {
        if std::fs::canonicalize(&self.workspace).as_ref().ok() != Some(&self.workspace) {
            return Err("Workspace physical binding changed after capture".into());
        }
        let agents = self.admitted_agent_dependencies();
        for role in self
            .role_sources
            .values()
            .filter(|role| agents.contains(&role.identity))
        {
            let boundary = if role.layer == "workspace" {
                &self.workspace
            } else {
                &self.agent_root
            };
            crate::runtime::resources::validate_project_resource_path(boundary, &role.selected)
                .map_err(|e| e.to_string())?;
        }
        for path in &self.project_resources {
            crate::runtime::resources::validate_project_resource_path(&self.workspace, path)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// The immutable validated settings this launch resolved.
    #[must_use]
    pub fn config(&self) -> &CurrentRuntimeConfig {
        &self.config
    }

    /// Prospective Session model after validating deliberate Session intent.
    /// The Root default and its authored provenance remain separate facts.
    #[must_use]
    pub fn session_model(&self) -> &crate::model::session::SessionModelConfig {
        self.input
            .model
            .as_ref()
            .unwrap_or_else(|| self.config.initial_model())
    }

    /// Native current-file analysis. This is prospective and has no authority
    /// over an already-published runtime generation.
    #[must_use]
    pub const fn resource_inspection(
        &self,
    ) -> &crate::runtime::capability_inspection::CapabilityInspection {
        &self.inspection
    }

    /// The effective source provenance of every Skill identity this launch
    /// admitted, including what each one shadowed (Issue #280).
    #[must_use]
    pub fn skill_provenance(&self) -> &[crate::skills::SkillProvenance] {
        &self.inspection.skills
    }

    /// The typed, canonically ordered Skill discovery facts of this launch.
    ///
    /// Unused malformed packages warn. Selecting an unavailable identity in
    /// an Agent profile fails at that profile's resolution or admission boundary.
    #[must_use]
    pub fn skill_diagnostics(&self) -> &[crate::skills::SkillDiagnostic] {
        &self.inspection.skill_diagnostics
    }

    /// Safe field origins, without configuration values.
    #[must_use]
    pub const fn provenance(&self) -> &BTreeMap<String, Origin> {
        &self.provenance
    }

    /// Stable identity of the canonical workspace path.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

impl AdmittedSessionConfig {
    /// Change only Session intent, retaining the exact published resources,
    /// catalog and credential capture. No source or physical resource is reopened.
    pub(crate) fn with_model_selection(
        &self,
        selection: crate::model::session::SessionModelConfig,
        revision: u64,
    ) -> Self {
        let mut retained = self.clone();
        retained.prospective.input.model = Some(selection);
        retained.binding_revision = revision;
        retained
    }

    /// This binding at `binding_revision` with `published`'s catalog and its
    /// own selection, context and capabilities retained.
    pub(crate) fn with_published_catalog(
        &self,
        published: &ProspectiveSessionConfig,
        binding_revision: u64,
    ) -> Self {
        Self {
            binding_revision,
            credentials: self.credentials.clone(),
            prospective: Box::new(
                self.prospective
                    .as_ref()
                    .clone()
                    .with_published_catalog(published),
            ),
        }
    }

    pub(crate) fn with_execution_policy(&self, desired: &IndependentPolicy) -> Self {
        let mut retained = self.clone();
        retained.prospective.compose_execution_policy(desired);
        retained
    }
    pub(crate) fn with_shared_capacity(&self, desired: &IndependentPolicy) -> Self {
        let mut retained = self.clone();
        retained.prospective.compose_shared_capacity(desired);
        retained
    }
}

impl std::ops::Deref for AdmittedSessionConfig {
    type Target = ProspectiveSessionConfig;
    fn deref(&self) -> &Self::Target {
        &self.prospective
    }
}

impl std::ops::Deref for ProspectiveSessionConfig {
    type Target = SessionLocations;
    fn deref(&self) -> &Self::Target {
        &self.locations
    }
}

impl std::fmt::Debug for ProspectiveSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProspectiveSessionConfig")
            .field("configuration", &"<redacted>")
            .finish_non_exhaustive()
    }
}
