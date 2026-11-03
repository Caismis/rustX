//! Native automatic configuration commit.
//!
//! Configuration authoring (source capture, merge, provenance, redaction and
//! unit composition) belongs to the application configuration owner. The
//! runtime owns what happens after that: an off-side prepared candidate staged
//! for this runtime, the commit at the runtime's own safe boundary, the frozen
//! configuration facts of an admitted Attempt, and the typed record of what a
//! commit actually changed.
//!
//! A staged candidate is never visible to an admitted Attempt. The runtime
//! commits it under the coordinator lock, before it admits the next
//! independent Attempt, once no real execution owner blocks replacement. No
//! client participates; cache impact is never a reason to wait.
//!
//! None of these values interprets authored documents; a candidate carries
//! the application-composed [`RuntimeConfiguration`](crate::runtime::resources::RuntimeConfiguration)
//! inside its preview generation unchanged.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::session::SessionModelState;
use crate::runtime::resources::RuntimeResourceSnapshot;

/// Compose a live catalog observation over one exact committed resource binding.
/// This synchronous seam cannot read authored sources or start discovery.
pub(crate) type RuntimeCatalogPreparer = dyn Fn(
        crate::capabilities::PreparedCapabilityCandidate,
        &RuntimeResourceSnapshot,
    ) -> Result<crate::runtime::resources::PreparedRuntimeResources, String>
    + Send
    + Sync;

// The configuration facts an admitted Attempt froze at its admission
// linearization point; a later commit never mutates them. (A plain comment:
// this wire type's schema carries no description.)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AdmittedConfiguration {
    pub approval_mode: crate::runtime::ApprovalMode,
    pub model_timeout: Option<crate::local_runtime::config::ModelTimeoutPolicyDocument>,
    pub tool_deadline: Option<crate::local_runtime::config::ToolDeadlinePolicyDocument>,
    pub attempt: crate::runtime::identity::AttemptId,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub model: crate::model::session::SessionModelView,
    pub resources: crate::runtime::capability_inspection::CapabilityInspection,
}

/// One off-side configuration candidate prepared by a specific runtime
/// allocation against one exact Session binding revision.
pub(crate) struct PreparedConfiguration {
    /// The runtime allocation that prepared this candidate. A candidate from
    /// a replaced runtime can never commit into its successor.
    pub(crate) owner: Arc<()>,
    /// The Session binding revision this candidate was prepared against.
    pub(crate) baseline: u64,
    /// The complete candidate generation used for comparison and, without a
    /// capability candidate, for the commit itself.
    pub(crate) preview: Arc<RuntimeResourceSnapshot>,
    pub(crate) model: SessionModelState,
    pub(crate) capability: Option<crate::capabilities::PreparedCapabilityCandidate>,
    pub(crate) resources: Option<crate::runtime::resources::PreparedRuntimeResourceData>,
}

/// The application owner's latest desired application for one scope.
///
/// The owner advances the shared value under its source fence whenever newer
/// input is desired; the runtime reads it under its coordinator lock at the
/// commit point. A candidate whose own desire is no longer current is
/// superseded and can never commit, even when it finished preparing later.
#[derive(Clone, Debug)]
pub(crate) struct DesiredApplication {
    current: Arc<AtomicU64>,
    attempt: u64,
}

impl DesiredApplication {
    pub(crate) fn new(current: Arc<AtomicU64>, attempt: u64) -> Self {
        Self { current, attempt }
    }

    pub(crate) fn is_current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.attempt
    }
}

/// What happened to one staged candidate. Delivered exactly once, outside the
/// coordinator lock, to the application owner that staged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StagedOutcome {
    /// The candidate is the Session's committed configuration.
    Committed,
    /// Newer desired input, a newer Session binding or a replaced runtime
    /// superseded the candidate before it could commit. Nothing changed.
    Superseded,
    /// The commit was refused; the previous configuration remains in use.
    Failed { diagnostic: String },
    /// The resources were published but their change record could not be
    /// made durable. Admission is fenced until native recovery reconciles.
    Uncertain { diagnostic: String },
}

/// One fully prepared candidate waiting for the runtime's safe boundary.
pub(crate) struct StagedConfiguration {
    pub(crate) prepared: PreparedConfiguration,
    pub(crate) desired: DesiredApplication,
    /// Why this commit happens; recorded with its change fact.
    pub(crate) reason: ConfigurationChangeReason,
    /// Installs the retained Session binding at the committed revision. It
    /// runs inside the commit critical section and takes only leaf locks.
    pub(crate) retain: Box<dyn FnOnce(u64) + Send>,
    /// Reports the outcome to the staging owner; the runtime spawns the future
    /// after releasing its lock, so the owner may take its own fence.
    pub(crate) settled:
        Box<dyn FnOnce(StagedOutcome) -> futures_util::future::BoxFuture<'static, ()> + Send>,
}

/// Why the Session's configuration changed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationChangeReason {
    /// A saved or externally edited authored source.
    Source,
    /// An explicit Session model selection reconsidered the source.
    ModelSelection,
    /// The runtime allocation was rebuilt and reprepared its sources.
    Reallocation,
}

/// One configuration domain a commit changed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationDomain {
    /// The effective system instructions, project guidance or Agent profile.
    Instructions,
    /// The context budgeting/projection policy.
    ContextPolicy,
    /// The model-facing Tool catalog.
    Tools,
    /// The Skill catalog.
    Skills,
    /// The effective model of future work.
    Model,
    /// The approval mode future Attempts freeze.
    ApprovalMode,
}

/// Most names one change category records; the total stays exact.
/// Tool and Skill names are themselves bounded, so one complete change fits
/// the bounded transcript and Trace headers.
pub const CONFIGURATION_CHANGE_NAME_LIMIT: usize = 16;

/// A bounded, ordered list of names with its exact total.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct BoundedNames {
    /// At most [`CONFIGURATION_CHANGE_NAME_LIMIT`] names in sorted order.
    pub names: Vec<String>,
    /// The exact number of names in this category.
    pub total: u32,
}

impl BoundedNames {
    fn from_names(mut names: Vec<String>) -> Self {
        names.sort();
        let total = u32::try_from(names.len()).unwrap_or(u32::MAX);
        names.truncate(CONFIGURATION_CHANGE_NAME_LIMIT);
        Self { names, total }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Named-item differences of one catalog. A same-name item whose definition
/// changed is `changed`, never a removal plus an addition.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct NamedChanges {
    pub added: BoundedNames,
    pub removed: BoundedNames,
    pub changed: BoundedNames,
}

impl NamedChanges {
    fn between<T: PartialEq>(before: &BTreeMap<String, T>, after: &BTreeMap<String, T>) -> Self {
        let added = after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .cloned()
            .collect();
        let removed = before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .cloned()
            .collect();
        let changed = after
            .iter()
            .filter(|(name, value)| before.get(*name).is_some_and(|old| old != *value))
            .map(|(name, _)| name.clone())
            .collect();
        Self {
            added: BoundedNames::from_names(added),
            removed: BoundedNames::from_names(removed),
            changed: BoundedNames::from_names(changed),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The typed semantic summary of one actual configuration commit.
///
/// Recorded once per commit that changed something; never for preparation,
/// a file notification or a commit with no effective semantic change. It
/// names what changed, not the bodies: the exact prompt and Tool schemas a
/// request used remain in that request's own immutable snapshot.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ConfigurationChange {
    pub reason: ConfigurationChangeReason,
    /// The Session binding revision this commit replaced.
    pub previous_binding_revision: u64,
    /// The Session binding revision this commit installed.
    pub binding_revision: u64,
    /// The changed domains, in their stable order.
    pub domains: Vec<ConfigurationDomain>,
    pub tools: NamedChanges,
    pub skills: NamedChanges,
}

impl ConfigurationChange {
    /// The effective differences between two committed generations, or `None`
    /// when nothing future work observes changed.
    pub(crate) fn between(
        reason: ConfigurationChangeReason,
        previous_binding_revision: u64,
        before: (&RuntimeResourceSnapshot, &SessionModelState),
        after: (&RuntimeResourceSnapshot, &SessionModelState),
    ) -> Option<Self> {
        let (old, old_model) = before;
        let (new, new_model) = after;
        let tools = |snapshot: &RuntimeResourceSnapshot| {
            snapshot
                .capability()
                .tool_registry()
                .model_definitions()
                .into_iter()
                .map(|definition| (definition.name.clone(), definition))
                .collect::<BTreeMap<_, _>>()
        };
        let skills = |snapshot: &RuntimeResourceSnapshot| {
            snapshot
                .capability()
                .model_skill_entries()
                .iter()
                .map(|entry| (entry.name.clone(), entry.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let prompt = |snapshot: &RuntimeResourceSnapshot| {
            snapshot
                .context_assembly()
                .system_sections(&snapshot.native_system_input())
                .map(|sections| crate::context::render_effective_system_prompt(&sections))
                .ok()
        };
        let configuration = |snapshot: &RuntimeResourceSnapshot| {
            snapshot.configuration().map(|configuration| {
                (
                    configuration.config.context,
                    configuration.config.approval_mode,
                )
            })
        };
        let tools = NamedChanges::between(&tools(old), &tools(new));
        let skills = NamedChanges::between(&skills(old), &skills(new));
        let mut domains = Vec::new();
        if prompt(old) != prompt(new) {
            domains.push(ConfigurationDomain::Instructions);
        }
        let (old_configuration, new_configuration) = (configuration(old), configuration(new));
        if old_configuration.map(|c| c.0) != new_configuration.map(|c| c.0) {
            domains.push(ConfigurationDomain::ContextPolicy);
        }
        if !tools.is_empty() {
            domains.push(ConfigurationDomain::Tools);
        }
        if !skills.is_empty() {
            domains.push(ConfigurationDomain::Skills);
        }
        if old_model.view() != new_model.view() {
            domains.push(ConfigurationDomain::Model);
        }
        if old_configuration.map(|c| c.1) != new_configuration.map(|c| c.1) {
            domains.push(ConfigurationDomain::ApprovalMode);
        }
        (!domains.is_empty()).then(|| Self {
            reason,
            previous_binding_revision,
            binding_revision: previous_binding_revision + 1,
            domains,
            tools,
            skills,
        })
    }
}

/// One coherent cut of a runtime's committed configuration: the generation
/// future Attempts acquire, the Session binding and model intent, the
/// effective approval mode, and the configuration the current Attempt froze.
pub(crate) struct ConfigurationCut {
    pub(crate) binding_revision: u64,
    pub(crate) resources: Arc<RuntimeResourceSnapshot>,
    pub(crate) approval_mode: crate::runtime::ApprovalMode,
    pub(crate) session_model: Option<crate::model::session::SessionModelConfig>,
    pub(crate) effective_model: crate::model::session::SessionModelView,
    pub(crate) admitted_attempt: Option<AdmittedConfiguration>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_definition_change_is_changed_not_removed_and_added() {
        let before = BTreeMap::from([("read".to_owned(), 1), ("write".to_owned(), 1)]);
        let after = BTreeMap::from([("read".to_owned(), 2), ("grep".to_owned(), 1)]);
        let changes = NamedChanges::between(&before, &after);
        assert_eq!(changes.added.names, ["grep"]);
        assert_eq!(changes.removed.names, ["write"]);
        assert_eq!(changes.changed.names, ["read"]);
        assert!(NamedChanges::between(&after, &after).is_empty());
    }

    #[test]
    fn name_lists_are_bounded_with_exact_totals() {
        let after = (0..100)
            .map(|index| (format!("tool_{index:03}"), 0_u8))
            .collect::<BTreeMap<_, _>>();
        let changes = NamedChanges::between(&BTreeMap::new(), &after);
        assert_eq!(changes.added.total, 100);
        assert_eq!(changes.added.names.len(), CONFIGURATION_CHANGE_NAME_LIMIT);
        assert_eq!(changes.added.names[0], "tool_000");
    }

    #[test]
    fn a_superseded_desire_is_never_current_again() {
        let current = Arc::new(AtomicU64::new(2));
        let v2 = DesiredApplication::new(current.clone(), 2);
        assert!(v2.is_current());
        current.store(3, Ordering::SeqCst);
        assert!(!v2.is_current());
        assert!(DesiredApplication::new(current, 3).is_current());
    }
}
