//! Workflow discovery catalog and dependency-admission types.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{MAX_WORKFLOW_DEFINITIONS, WorkflowId, WorkflowProgram, WorkflowRunError};

/// A discovered source and its generation-owned admission decision.
#[derive(Debug, Clone)]
pub struct WorkflowEntry {
    pub source: Arc<WorkflowProgram>,
    pub admission: WorkflowAdmission,
}

/// Only Enabled carries a frozen executable program. Discovery alone is inert.
#[derive(Debug, Clone)]
pub enum WorkflowAdmission {
    Enabled(Arc<WorkflowProgram>),
    Disabled(Vec<WorkflowAdmissionDiagnostic>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowAdmissionDiagnostic {
    pub path: String,
    pub reason: WorkflowDependencyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum WorkflowDependencyFailure {
    NotAdmitted,
    /// Bounded static model error classification, without provider details.
    Model {
        reason: String,
    },
    Materialization {
        #[serde(skip)]
        detail: String,
    },
    Agent(crate::runtime::agent_profile::AgentProfileDiagnostic),
    Tool(crate::capabilities::selection::ToolSelectionError),
    IneligibleTool(crate::capabilities::selection::ExactToolSelector),
}

impl WorkflowAdmissionDiagnostic {
    pub(crate) fn redacted(&self) -> Self {
        let mut fact = self.clone();
        match &mut fact.reason {
            WorkflowDependencyFailure::Agent(reason) => *reason = reason.redacted(),
            WorkflowDependencyFailure::Tool(reason) => *reason = reason.redacted(),
            WorkflowDependencyFailure::Materialization { detail } => detail.clear(),
            _ => {}
        }
        fact
    }
}

/// One immutable catalog; enabled identities are derived, never separately authored.
#[derive(Debug, Clone, Default)]
pub struct WorkflowCatalog {
    pub(crate) locations: BTreeMap<WorkflowId, crate::runtime::resources::ResourceLocation>,
    pub(crate) discovery_diagnostics: Vec<crate::runtime::resources::RuntimeResourceLoadError>,
    /// Source diagnostics only: never part of a program, Tool schema or run.
    pub(crate) presentation_diagnostics:
        BTreeMap<WorkflowId, Vec<crate::runtime::resources::RuntimeResourceLoadError>>,
    pub(super) entries: BTreeMap<WorkflowId, WorkflowEntry>,
    invalid: BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError>,
}

impl WorkflowCatalog {
    pub(crate) fn set_invalid(
        &mut self,
        invalid: BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError>,
    ) {
        self.invalid = invalid;
    }
    #[must_use]
    pub fn invalid(
        &self,
    ) -> &BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError> {
        &self.invalid
    }

    /// Discovers structurally compiled sources. Admission is a separate candidate phase.
    ///
    /// # Errors
    /// Rejects duplicate identities or an oversized catalog.
    pub fn new(
        programs: impl IntoIterator<Item = WorkflowProgram>,
    ) -> Result<Self, WorkflowCatalogError> {
        let mut entries = BTreeMap::new();
        for program in programs {
            if entries
                .insert(
                    program.id.clone(),
                    WorkflowEntry {
                        source: Arc::new(program),
                        admission: WorkflowAdmission::Disabled(vec![WorkflowAdmissionDiagnostic {
                            path: "block".into(),
                            reason: WorkflowDependencyFailure::NotAdmitted,
                        }]),
                    },
                )
                .is_some()
            {
                return Err(WorkflowCatalogError::DuplicateDefinition);
            }
            if entries.len() > MAX_WORKFLOW_DEFINITIONS {
                return Err(WorkflowCatalogError::TooManyDefinitions);
            }
        }
        Ok(Self {
            entries,
            locations: BTreeMap::new(),
            invalid: BTreeMap::new(),
            discovery_diagnostics: Vec::new(),
            presentation_diagnostics: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Discovered source, including disabled programs, for inert inspection.
    #[must_use]
    pub fn get(&self, id: &WorkflowId) -> Option<&Arc<WorkflowProgram>> {
        self.entries.get(id).map(|entry| &entry.source)
    }

    /// The winning file is enabled only by the raw selection of the scope that
    /// owns it. A Global entry never enables the Workspace file that shadows
    /// it, and a Workspace entry never enables a Global file.
    #[must_use]
    pub(crate) fn enabled_by_owning_scope(
        &self,
        id: &WorkflowId,
        user_selected: &[WorkflowId],
        workspace_selected: &[WorkflowId],
    ) -> bool {
        self.locations.get(id).is_some_and(|location| {
            let selected = if location.scope == crate::runtime::resources::SourceScope::Workspace {
                workspace_selected
            } else {
                user_selected
            };
            selected.contains(id)
        })
    }

    #[must_use]
    pub fn entries(&self) -> &BTreeMap<WorkflowId, WorkflowEntry> {
        &self.entries
    }

    #[must_use]
    pub fn enabled_ids(&self) -> BTreeSet<WorkflowId> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| {
                matches!(entry.admission, WorkflowAdmission::Enabled(_)).then_some(id.clone())
            })
            .collect()
    }

    /// The common direct/native execution gate; it never rediscovers dependencies.
    ///
    /// # Errors
    /// Returns the generation's retained disabled reason or an unknown identity.
    pub fn executable(&self, id: &WorkflowId) -> Result<&Arc<WorkflowProgram>, WorkflowRunError> {
        match self.entries.get(id) {
            Some(WorkflowEntry {
                admission: WorkflowAdmission::Enabled(program),
                ..
            }) => Ok(program),
            Some(WorkflowEntry {
                admission: WorkflowAdmission::Disabled(diagnostics),
                ..
            }) => Err(WorkflowRunError::Disabled {
                workflow: id.clone(),
                diagnostics: diagnostics.clone(),
            }),
            None => Err(WorkflowRunError::InvalidProgram(format!(
                "Workflow {id} is not discovered"
            ))),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCatalogError {
    DuplicateDefinition,
    TooManyDefinitions,
}
impl fmt::Display for WorkflowCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DuplicateDefinition => "duplicate workflow definition id",
            Self::TooManyDefinitions => "too many Workflow definitions",
        })
    }
}
impl std::error::Error for WorkflowCatalogError {}
