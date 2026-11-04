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
pub const CONFIGURATION_CHANGE_NAME_LIMIT: usize = 16;

/// Most JSON-encoded bytes the recorded names of one category occupy. Names
/// are not bounded at their source (an MCP server chooses its own), so the
/// six categories of one change stay within the 16 KiB transcript and Trace
/// headers by this budget, never by an assumed name length.
pub const CONFIGURATION_CHANGE_NAME_BYTES: usize = 1024;

/// A bounded, ordered list of names with its exact total.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct BoundedNames {
    /// Sorted names: at most [`CONFIGURATION_CHANGE_NAME_LIMIT`] of them,
    /// within [`CONFIGURATION_CHANGE_NAME_BYTES`] encoded bytes.
    pub names: Vec<String>,
    /// The exact number of names in this category.
    pub total: u32,
}

impl BoundedNames {
    fn from_names(mut names: Vec<String>) -> Self {
        names.sort();
        let total = u32::try_from(names.len()).unwrap_or(u32::MAX);
        // A name that alone would overflow the budget is counted, not
        // recorded; the shorter names after it still are.
        let mut bytes = 0;
        names.retain(|name| {
            let encoded = serde_json::to_string(name).map_or(usize::MAX, |encoded| encoded.len());
            let fits = bytes + encoded < CONFIGURATION_CHANGE_NAME_BYTES;
            if fits {
                bytes += encoded + 1;
            }
            fits
        });
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
    pub(crate) fn between<T: PartialEq>(
        before: &BTreeMap<String, T>,
        after: &BTreeMap<String, T>,
    ) -> Self {
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
    /// The byte length of the effective system instructions this commit
    /// installed. The fact retains them exactly, readable in bounded ranges
    /// even when no request ever used them.
    pub instructions_bytes: u64,
}

impl ConfigurationChange {
    /// The effective differences between two committed generations with the
    /// successor's effective instructions, or `None` when nothing future work
    /// observes changed. An independently published unit changes the
    /// generation without rebinding the Session, so both revisions are then
    /// the same.
    pub(crate) fn between(
        reason: ConfigurationChangeReason,
        (previous_binding_revision, binding_revision): (u64, u64),
        before: (&RuntimeResourceSnapshot, &SessionModelState),
        after: (&RuntimeResourceSnapshot, &SessionModelState),
    ) -> Option<(Self, String)> {
        let (old, old_model) = before;
        let (new, new_model) = after;
        // A Tool is its whole definition and the executable binding of the
        // server it runs on; a same-name implementation change is `changed`
        // even when the model-visible declaration is byte-identical.
        let tools = |snapshot: &RuntimeResourceSnapshot| {
            let capability = snapshot.capability();
            let registry = capability.tool_registry();
            registry
                .definitions()
                .into_iter()
                .zip(registry.model_definitions())
                .map(|(definition, model)| {
                    let server = match &definition.origin {
                        crate::tools::types::ToolOrigin::Builtin => None,
                        crate::tools::types::ToolOrigin::Mcp { server_id } => {
                            Some(server_id.clone())
                        }
                        crate::tools::types::ToolOrigin::ManagedPython { package } => {
                            Some(crate::tools::python::python_server_id(package))
                        }
                    };
                    let binding = server.and_then(|id| capability.mcp_servers().get(&id).cloned());
                    (definition.name.clone(), (model, definition, binding))
                })
                .collect::<BTreeMap<_, _>>()
        };
        // A Skill is its catalog entry and its content-derived version, so a
        // body or script edit under unchanged frontmatter is `changed`.
        let skills = |snapshot: &RuntimeResourceSnapshot| {
            let capability = snapshot.capability();
            let versions = capability
                .skills()
                .bindings()
                .iter()
                .map(|binding| (binding.skill_id.as_str(), &binding.version_id))
                .collect::<BTreeMap<_, _>>();
            capability
                .model_skill_entries()
                .iter()
                .map(|entry| {
                    let version = versions.get(entry.name.as_str()).copied().cloned();
                    (entry.name.clone(), (entry.clone(), version))
                })
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
        let instructions = prompt(new);
        if prompt(old) != instructions {
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
        let instructions = instructions.unwrap_or_default();
        (!domains.is_empty()).then_some((
            Self {
                reason,
                previous_binding_revision,
                binding_revision,
                domains,
                tools,
                skills,
                instructions_bytes: instructions.len() as u64,
            },
            instructions,
        ))
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
    fn name_lists_are_bounded_by_encoded_bytes() {
        let long = "x".repeat(20 * 1024);
        let escaped = "\"\u{1}水".repeat(200);
        let after = BTreeMap::from([
            (long.clone(), 0_u8),
            (escaped.clone(), 0),
            ("a".to_owned(), 0),
        ]);
        let changes = NamedChanges::between(&BTreeMap::new(), &after);
        assert_eq!(changes.added.total, 3);
        // Sorted, so the short name is kept and the oversized ones are counted.
        assert_eq!(changes.added.names, ["a"]);
        let change = ConfigurationChange {
            reason: ConfigurationChangeReason::Source,
            previous_binding_revision: 1,
            binding_revision: 2,
            domains: vec![ConfigurationDomain::Tools, ConfigurationDomain::Skills],
            tools: NamedChanges {
                added: changes.added.clone(),
                removed: NamedChanges::between(&after, &BTreeMap::new()).removed,
                changed: changes.added.clone(),
            },
            skills: NamedChanges {
                added: changes.added.clone(),
                removed: changes.added.clone(),
                changed: changes.added,
            },
            instructions_bytes: 0,
        };
        let worst = (0..CONFIGURATION_CHANGE_NAME_LIMIT)
            .map(|index| (format!("{index:02}{}", "\"".repeat(200)), 0_u8))
            .collect::<BTreeMap<_, _>>();
        let worst = NamedChanges::between(&BTreeMap::new(), &worst).added;
        assert!(
            serde_json::to_string(&worst.names).unwrap().len()
                <= CONFIGURATION_CHANGE_NAME_BYTES + 2
        );
        let full = ConfigurationChange {
            tools: NamedChanges {
                added: worst.clone(),
                removed: worst.clone(),
                changed: worst.clone(),
            },
            skills: NamedChanges {
                added: worst.clone(),
                removed: worst.clone(),
                changed: worst,
            },
            ..change.clone()
        };
        for change in [change, full] {
            assert!(serde_json::to_vec(&change).unwrap().len() < 8 * 1024);
        }
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
