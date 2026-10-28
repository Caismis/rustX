//! Exact-target inert analysis of the current complete authoring document.
use super::{UserConfigManager, WorkflowDiagnostic, presentation_diagnostic};
use crate::local_runtime::configuration::settings::{SettingsError, SourceTarget};
use crate::runtime::workflow::{
    WorkflowId,
    authoring::{WorkflowAnalysisQuery, WorkflowCandidateAnalysis},
};
use crate::workflow_file::{WorkflowFileDocument, WorkflowPresentationDiagnostic};
use serde::{Deserialize, Serialize};

/// Complete response, including correlation and presentation diagnostics.
pub const MAX_WORKFLOW_ANALYSIS_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAnalysisReport {
    pub target: SourceTarget,
    pub id: WorkflowId,
    /// Identity of complete canonical candidate bytes, including presentation.
    pub candidate_revision: String,
    /// Opaque browser edit generation; independent of file and connection identity.
    pub generation: String,
    pub query: Option<WorkflowAnalysisQuery>,
    pub analysis: WorkflowCandidateAnalysis,
    pub presentation: Vec<WorkflowPresentationDiagnostic>,
}

impl UserConfigManager {
    /// Analyze without discovering dependencies or constructing a `WorkflowProgram`.
    /// # Errors
    /// Rejects redirected targets, invalid file metadata and capacity violations.
    pub fn analyze_workflow(
        &self,
        target: &SourceTarget,
        id: &WorkflowId,
        document: &WorkflowFileDocument,
        generation: String,
        query: Option<WorkflowAnalysisQuery>,
    ) -> Result<WorkflowAnalysisReport, SettingsError> {
        target.validate()?;
        let invalid = |diagnostic| SettingsError::InvalidWorkflow(Box::new(diagnostic));
        if generation.is_empty() || generation.len() > 64 {
            return Err(SettingsError::Invalid);
        }
        if let Some(diagnostic) = super::unrepresentable_number(document) {
            return Err(invalid(diagnostic));
        }
        let bytes = document
            .canonical_bytes()
            .map_err(|error| invalid(presentation_diagnostic(&error)))?;
        let analysis =
            crate::runtime::workflow::authoring::analyze(&document.definition, query.as_ref())
                .map_err(|reason| {
                    invalid(WorkflowDiagnostic::new("$", "analysis_capacity", reason))
                })?;
        let report = WorkflowAnalysisReport {
            target: target.clone(),
            id: id.clone(),
            generation,
            query,
            analysis,
            candidate_revision: crate::local_runtime::settings::revision(Some(&bytes)),
            presentation: document.presentation_diagnostics(),
        };
        if serde_json::to_vec(&report).map_or(true, |bytes| {
            bytes.len() > MAX_WORKFLOW_ANALYSIS_RESPONSE_BYTES
        }) {
            return Err(invalid(WorkflowDiagnostic::new(
                "$",
                "analysis_capacity",
                "complete analysis response exceeds 512 KiB",
            )));
        }
        Ok(report)
    }
}
