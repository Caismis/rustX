//! Offline check and explanation of one candidate Workflow document.
//!
//! Structural validity (the native compiler) and dependency admission
//! (named Agents and Tools of one explicit Workspace composition) are
//! separate facts. A structurally valid candidate may have unresolved
//! dependencies, and dependencies are never checked against a context the
//! caller did not name: a User (Global) file has none unless one is given.
//! External sources stay `Unprepared`; nothing is prepared, connected,
//! executed or credentialed.
use super::{UserConfigManager, WorkflowDiagnostic, bounded, file_path};
use crate::local_runtime::configuration::SessionConfigInput;
use crate::local_runtime::configuration::settings::{SettingsError, SourceTarget};
use crate::runtime::workflow::inspection::WorkflowInspection;
use crate::runtime::workflow::{
    MAX_WORKFLOW_DIAGNOSTIC_BYTES, WorkflowAdmission, WorkflowAdmissionDiagnostic, WorkflowCatalog,
    WorkflowDefinition, WorkflowId, WorkflowProgram,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Whether the native compiler admits the candidate's structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStructure {
    /// Compiled. `outputs` are the compiled output contracts of every node
    /// by authored path, including derived Parallel and Loop results.
    Valid { outputs: BTreeMap<String, Value> },
    /// The first native compiler rejection.
    Invalid { diagnostic: WorkflowDiagnostic },
}

/// Dependency admission of a structurally valid candidate in one explicit
/// Workspace composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDependencies {
    /// No context was named, or the structure is invalid.
    NotChecked,
    /// Every named Agent and Tool admits in `context`.
    Admitted { context: PathBuf },
    /// Native admission diagnostics in `context`.
    Blocked {
        context: PathBuf,
        diagnostics: Vec<WorkflowAdmissionDiagnostic>,
    },
    /// `context` cannot be composed offline; readiness stays unknown.
    Unavailable { context: PathBuf, reason: String },
}

/// One offline check of one exact candidate document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCheckReport {
    pub structure: WorkflowStructure,
    pub dependencies: WorkflowDependencies,
    /// The redacted static explanation, when requested of a valid candidate.
    pub explanation: Option<Box<WorkflowInspection>>,
}

impl UserConfigManager {
    /// Check one candidate document for `id` in `target`, optionally in an
    /// explicit Workspace dependency context. A Workspace file's dependency
    /// context is always its own Workspace.
    /// # Errors
    /// Rejects a redirected target or context, and a Workspace file checked
    /// in a different Workspace.
    pub fn check_workflow(
        &self,
        target: &SourceTarget,
        id: &WorkflowId,
        document: WorkflowDefinition,
        context: Option<&Path>,
        explain: bool,
    ) -> Result<WorkflowCheckReport, SettingsError> {
        target.validate()?;
        let context = match (target, context) {
            (SourceTarget::Workspace { directory }, None) => Some(directory.clone()),
            (SourceTarget::Workspace { directory }, Some(context)) if context == directory => {
                Some(directory.clone())
            }
            (SourceTarget::Workspace { .. }, Some(_)) => return Err(SettingsError::Invalid),
            (SourceTarget::User, context) => context.map(Path::to_path_buf),
        };
        if let Some(directory) = &context {
            SourceTarget::Workspace {
                directory: directory.clone(),
            }
            .validate()?;
        }
        let path = file_path(self, target, id);
        let program =
            match crate::local_runtime::workflow_resources::compile_definition(id, &path, document)
            {
                Ok(program) => program,
                Err(error) => {
                    return Ok(WorkflowCheckReport {
                        structure: WorkflowStructure::Invalid {
                            diagnostic: WorkflowDiagnostic::from_resource(&error),
                        },
                        dependencies: WorkflowDependencies::NotChecked,
                        explanation: None,
                    });
                }
            };
        Ok(WorkflowCheckReport {
            structure: WorkflowStructure::Valid {
                outputs: program.node_outputs(),
            },
            explanation: explain.then(|| Box::new(program.inspect())),
            dependencies: context.map_or(WorkflowDependencies::NotChecked, |context| {
                self.candidate_dependencies(context, program)
            }),
        })
    }

    /// Admit `program` in place of its identity in the offline composition
    /// of `workspace`, with exactly the metadata capture admits with.
    fn candidate_dependencies(
        &self,
        workspace: PathBuf,
        program: WorkflowProgram,
    ) -> WorkflowDependencies {
        let unavailable = |workspace: PathBuf, reason: &str| WorkflowDependencies::Unavailable {
            context: workspace,
            reason: bounded(reason, MAX_WORKFLOW_DIAGNOSTIC_BYTES),
        };
        let prospective = match self
            .capture_application(&SessionConfigInput::new(workspace.clone()))
            .and_then(|captured| captured.context)
        {
            Ok(prospective) => prospective,
            Err(reason) => return unavailable(workspace, &reason),
        };
        let id = program.id().clone();
        let programs = prospective
            .workflows
            .entries()
            .iter()
            .filter(|(other, _)| **other != id)
            .map(|(_, entry)| (*entry.source).clone())
            .chain(std::iter::once(program));
        let mut catalog = match WorkflowCatalog::new(programs) {
            Ok(catalog) => catalog,
            Err(error) => return unavailable(workspace, &error.to_string()),
        };
        let inputs = crate::local_runtime::configuration::capture::MetadataAdmission::compose(
            &prospective.config,
            &prospective.subagents,
            &prospective.workflows,
            &prospective.managed_python,
        );
        let skills =
            crate::skills::SkillSnapshot::from_discovery(prospective.skill_discovery.clone());
        catalog.admit_metadata(
            &inputs.catalog,
            &inputs.availability,
            &skills,
            &prospective.subagents,
            &inputs.native_leaves,
        );
        match &catalog.entries()[&id].admission {
            WorkflowAdmission::Enabled(_) => WorkflowDependencies::Admitted { context: workspace },
            WorkflowAdmission::Disabled(diagnostics) => WorkflowDependencies::Blocked {
                context: workspace,
                diagnostics: diagnostics
                    .iter()
                    .map(WorkflowAdmissionDiagnostic::redacted)
                    .collect(),
            },
        }
    }
}
