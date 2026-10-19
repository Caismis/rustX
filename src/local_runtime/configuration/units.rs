//! Apply-unit ownership and composition of independently published units.
//!
//! Each composed unit owns a fixed set of configuration keys. Composition
//! moves a unit's values, provenance and component revision together; the
//! same rules serve source availability, retained Session bindings and the
//! runtime generation (see [`super::generation`]).
use super::*;

/// Fixed unit ownership, shared by source and runtime projections. Replacing
/// the owned keys also removes provenance for fields no longer authored.
pub(crate) fn copy_unit_provenance(
    target: &mut BTreeMap<String, Origin>,
    source: &BTreeMap<String, Origin>,
    unit: application::ApplyUnit,
) {
    let prefixes = unit_keys(unit);
    target.retain(|key, _| !owns(prefixes, key));
    target.extend(
        source
            .iter()
            .filter(|(key, _)| owns(prefixes, key))
            .map(|(key, value)| (key.clone(), value.clone())),
    );
}

pub(super) fn unit_keys(unit: application::ApplyUnit) -> &'static [&'static str] {
    use application::ApplyUnit;
    match unit {
        ApplyUnit::ExecutionPolicy => &[
            "approval_mode",
            "model_timeout_policy",
            "tool_deadline_policy",
        ],
        ApplyUnit::SharedCapacity => &["subagents"],
        ApplyUnit::Instructions => &["agent.instructions", "agent.agents_md", "context"],
        ApplyUnit::Provider => &["agent.model", "models", "providers"],
        ApplyUnit::Capabilities | ApplyUnit::ProcessBindings => {
            unreachable!("not a composed unit")
        }
    }
}

pub(super) fn owns(prefixes: &[&str], key: &str) -> bool {
    prefixes.iter().any(|prefix| {
        key == *prefix
            || key
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

impl IndependentPolicy {
    pub(crate) fn compose_execution_policy<P, E>(
        &self,
        config: &mut CurrentRuntimeConfig,
        effective: &mut RuntimeLayer<P, E>,
        provenance: &mut BTreeMap<String, Origin>,
        revisions: &mut BTreeMap<application::ApplyUnit, String>,
    ) {
        let unchanged = config.approval_mode == self.config.approval_mode
            && config.model_timeout_policy == self.config.model_timeout_policy
            && config.tool_deadline_policy == self.config.tool_deadline_policy
            && effective.approval_mode == self.effective.approval_mode
            && effective.model_timeout_policy == self.effective.model_timeout_policy
            && effective.tool_deadline_policy == self.effective.tool_deadline_policy;
        let old_provenance = provenance.clone();
        config.approval_mode = self.config.approval_mode;
        config.model_timeout_policy = self.config.model_timeout_policy;
        config.tool_deadline_policy = self.config.tool_deadline_policy;
        effective.approval_mode = self.effective.approval_mode;
        effective
            .model_timeout_policy
            .clone_from(&self.effective.model_timeout_policy);
        effective
            .tool_deadline_policy
            .clone_from(&self.effective.tool_deadline_policy);
        copy_unit_provenance(
            provenance,
            &self.provenance,
            application::ApplyUnit::ExecutionPolicy,
        );
        if !unchanged || *provenance != old_provenance {
            revisions.insert(
                application::ApplyUnit::ExecutionPolicy,
                self.revision.clone(),
            );
        }
    }
    pub(crate) fn compose_shared_capacity<P, E>(
        &self,
        config: &mut CurrentRuntimeConfig,
        effective: &mut RuntimeLayer<P, E>,
        provenance: &mut BTreeMap<String, Origin>,
        revisions: &mut BTreeMap<application::ApplyUnit, String>,
    ) {
        let unchanged = config.subagents == self.config.subagents
            && effective.subagents == self.effective.subagents;
        let old_provenance = provenance.clone();
        config.subagents = self.config.subagents.clone();
        effective.subagents.clone_from(&self.effective.subagents);
        copy_unit_provenance(
            provenance,
            &self.provenance,
            application::ApplyUnit::SharedCapacity,
        );
        if !unchanged || *provenance != old_provenance {
            revisions.insert(
                application::ApplyUnit::SharedCapacity,
                self.revision.clone(),
            );
        }
    }
}

impl ProspectiveSessionConfig {
    pub(crate) fn compose_execution_policy(&mut self, desired: &IndependentPolicy) {
        desired.compose_execution_policy(
            std::sync::Arc::make_mut(&mut self.config),
            &mut self.effective,
            &mut self.provenance,
            &mut self.component_revisions,
        );
    }

    pub(crate) fn compose_shared_capacity(&mut self, desired: &IndependentPolicy) {
        desired.compose_shared_capacity(
            std::sync::Arc::make_mut(&mut self.config),
            &mut self.effective,
            &mut self.provenance,
            &mut self.component_revisions,
        );
    }
}
