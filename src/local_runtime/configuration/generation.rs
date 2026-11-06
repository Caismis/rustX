//! Composition of the configuration record a runtime resource generation
//! carries.
//!
//! The native runtime adopts complete [`RuntimeConfiguration`] values; it never
//! reads authored documents, redacts them, or decides which fields an apply
//! unit owns. Every such rule lives here, on the application side, and is
//! handed to the runtime either as a finished value or as a composition over
//! the exact base generation the runtime selects under its own lock.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::application::ApplyUnit;
use super::settings::{EffectiveConfiguration, SourceDocumentView, redact};
use super::units::copy_unit_provenance;
use super::{IndependentPolicy, Origin, ProspectiveSessionConfig};
use crate::model::invocation::ModelBindingRegistry;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::resources::{ContextBinding, RuntimeConfiguration};

/// The authored half of a generation's configuration: source revisions,
/// component revisions, field provenance and the redacted effective document.
///
/// It describes where the executed configuration came from. The runtime
/// carries it inert inside the same immutable generation as the execution
/// bindings, so one read cut always pairs a configuration with its own
/// provenance; it is never consulted by execution.
#[derive(Clone, PartialEq)]
pub struct AuthoredConfiguration {
    pub(crate) component_revisions: BTreeMap<ApplyUnit, String>,
    pub source_revisions: BTreeMap<PathBuf, String>,
    pub effective: SourceDocumentView,
    pub provenance: BTreeMap<String, Origin>,
}

impl ProspectiveSessionConfig {
    /// The redacted authored record of this complete capture.
    pub(crate) fn authored(&self) -> AuthoredConfiguration {
        AuthoredConfiguration {
            component_revisions: self.component_revisions.clone(),
            source_revisions: self.source_revisions.clone(),
            effective: redact(self.effective.clone()),
            provenance: self.provenance.clone(),
        }
    }

    /// The complete runtime configuration of this capture over `models`.
    pub(crate) fn runtime_configuration(
        &self,
        models: ModelBindingRegistry,
    ) -> RuntimeConfiguration {
        RuntimeConfiguration {
            config: self.config.clone(),
            models,
            resource_definitions: self.inspection.definitions.clone(),
            resource_diagnostics: self.inspection.resource_diagnostics.clone(),
            authored: self.authored(),
        }
    }

    /// Binds this complete capture's context as a candidate's own context.
    pub(crate) fn context_binding(&self, models: ModelBindingRegistry) -> ContextBinding {
        ContextBinding {
            configuration: self.runtime_configuration(models),
            project_context_files: self.project_context_files.clone(),
            root_agent_project_files: self.root_agent_project_files.clone(),
        }
    }

    /// Composes this capture's Instructions and Provider units over `base`,
    /// an adopted generation. The units' values, provenance and component
    /// revisions move; every other unit of `base` is retained. The capture may
    /// itself be composed (C1+I2): its diagnostic source baseline is carried,
    /// never the newest authored source manifest.
    pub(crate) fn compose_context_binding(
        &self,
        base: &RuntimeConfiguration,
        models: ModelBindingRegistry,
    ) -> ContextBinding {
        let mut configuration = base.clone();
        let config = Arc::make_mut(&mut configuration.config);
        config
            .agent
            .instructions
            .clone_from(&self.config.agent.instructions);
        config
            .agent
            .agents_md
            .clone_from(&self.config.agent.agents_md);
        config.context = self.config.context;
        config.agent.model.clone_from(&self.config.agent.model);
        configuration.models = models;
        let authored = &mut configuration.authored;
        for unit in [ApplyUnit::Instructions, ApplyUnit::Provider] {
            authored
                .component_revisions
                .insert(unit, self.component_revisions[&unit].clone());
            copy_unit_provenance(&mut authored.provenance, &self.provenance, unit);
        }
        let agent = self.effective.agent.as_ref();
        let context = authored
            .effective
            .agent
            .get_or_insert_with(Default::default);
        context.instructions = agent.and_then(|agent| agent.instructions.clone());
        context.agents_md = agent.and_then(|agent| agent.agents_md.clone());
        context.model = agent.and_then(|agent| agent.model.clone());
        authored
            .effective
            .context
            .clone_from(&self.effective.context);
        authored.effective.models.clone_from(&self.effective.models);
        authored.effective.providers = redact(self.effective.clone()).providers;
        authored.source_revisions.clone_from(&self.source_revisions);
        ContextBinding {
            configuration,
            project_context_files: self.project_context_files.clone(),
            root_agent_project_files: self.root_agent_project_files.clone(),
        }
    }

    /// Composes a newly published Model Catalog — the whole Provider unit —
    /// over `base`, the way independent policy units are composed: the unit's
    /// values, provenance and component revision move, while
    /// `source_revisions` stays the baseline the adopted context was resolved
    /// from.
    pub(crate) fn compose_model_catalog(
        &self,
        base: &RuntimeConfiguration,
        models: ModelBindingRegistry,
    ) -> RuntimeConfiguration {
        let mut configuration = base.clone();
        configuration.models = models;
        Arc::make_mut(&mut configuration.config)
            .agent
            .model
            .clone_from(&self.config.agent.model);
        let effective = redact(self.effective.clone());
        let authored = &mut configuration.authored;
        authored.effective.models = effective.models;
        authored.effective.providers = effective.providers;
        authored
            .effective
            .agent
            .get_or_insert_with(Default::default)
            .model = effective.agent.and_then(|agent| agent.model);
        copy_unit_provenance(
            &mut authored.provenance,
            &self.provenance,
            ApplyUnit::Provider,
        );
        authored.component_revisions.insert(
            ApplyUnit::Provider,
            self.component_revisions[&ApplyUnit::Provider].clone(),
        );
        configuration
    }
}

impl IndependentPolicy {
    /// Composes the shared child capacity unit over `base`; `None` when that
    /// changes nothing the generation carries.
    pub(crate) fn compose_runtime_shared_capacity(
        &self,
        base: &RuntimeConfiguration,
    ) -> Option<RuntimeConfiguration> {
        let mut configuration = base.clone();
        let authored = &mut configuration.authored;
        self.compose_shared_capacity(
            Arc::make_mut(&mut configuration.config),
            &mut authored.effective,
            &mut authored.provenance,
            &mut authored.component_revisions,
        );
        changed(base, configuration)
    }

    /// Composes the execution policy unit over `base`; `None` when that
    /// changes nothing the generation carries.
    pub(crate) fn compose_runtime_execution_policy(
        &self,
        base: &RuntimeConfiguration,
    ) -> Option<RuntimeConfiguration> {
        let mut configuration = base.clone();
        let authored = &mut configuration.authored;
        self.compose_execution_policy(
            Arc::make_mut(&mut configuration.config),
            &mut authored.effective,
            &mut authored.provenance,
            &mut authored.component_revisions,
        );
        changed(base, configuration)
    }
}

/// A composed independent unit is published only when it changes the
/// executed configuration, the authored document or its provenance; a
/// revision-only difference is not a new generation.
fn changed(
    base: &RuntimeConfiguration,
    configuration: RuntimeConfiguration,
) -> Option<RuntimeConfiguration> {
    (configuration.config != base.config
        || configuration.authored.effective != base.authored.effective
        || configuration.authored.provenance != base.authored.provenance)
        .then_some(configuration)
}

impl EffectiveConfiguration {
    /// Reads configuration, Session intent and admitted work of a live
    /// runtime in one coherent native cut.
    #[must_use]
    pub fn adopted(runtime: &ConversationRuntime) -> Option<Self> {
        let cut = runtime.configuration_cut();
        let resources = &cut.resources;
        let generation = resources.configuration()?;
        let config = &generation.config;
        let authored = &generation.authored;
        Some(Self {
            process_bindings: None,
            application: None,
            adopted_binding: cut.binding_revision,
            source_revisions: authored.source_revisions.clone(),
            generation: resources.revision(),
            document: authored.effective.clone(),
            root_agent: config.agent.clone(),
            context: config.context,
            model_timeout: config.model_timeout_policy,
            tool_deadline: config.tool_deadline_policy,
            child_capacity: config.subagents.clone(),
            approval_mode: cut.approval_mode,
            provenance: authored.provenance.clone(),
            resources: resources.inspection().clone(),
            available_tools: resources
                .capability()
                .available_tools()
                .tools()
                .iter()
                .map(|tool| tool.definition.clone())
                .collect(),
            session_model: cut.session_model,
            effective_model: cut.effective_model,
            admitted_attempt: cut.admitted_attempt,
        })
    }
}
