//! Offline presentation of the Workflow owner's candidate admission facts.
use super::diagnostics::{Report, Validity};
use super::launch::{HostEnvironment, LaunchRequest};
use crate::runtime::workflow::WorkflowId;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct WorkflowProjection {
    pub id: WorkflowId,
    pub source: PathBuf,
    pub admission: crate::runtime::capability_inspection::WorkflowInspection,
    pub program: Option<crate::runtime::workflow::inspection::WorkflowInspection>,
}

pub(super) fn inspect(
    id: &WorkflowId,
    explain: bool,
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Report {
    let operation = if explain {
        "workflow_explain"
    } else {
        "workflow_check"
    };
    let (mut report, launch) = super::diagnostics::inspect(operation, request, host);
    report.launch = None;
    report.capabilities = None;
    let Some(launch) = launch else {
        return report;
    };

    // Inspecting a particular identity selects its diagnostic even when the
    // malformed resource was harmless to unrelated offline composition.
    if let Some(error) = launch.workflows.invalid().get(id) {
        let failure = super::diagnostics::LaunchFailure::resource(error.clone());
        report.validity = Validity::Invalid;
        report.diagnostics.insert(0, *failure.diagnostic);
        return report;
    }

    let source = launch.workflows.locations.get(id).map_or_else(
        || {
            launch
                .locations
                .workspace
                .join(".agents/workflows")
                .join(format!("{id}.yaml"))
        },
        |location| location.path.clone(),
    );
    let Some(admission) = launch.inspection.workflows.get(id) else {
        return Report::failure(
            operation,
            Some(source),
            "workflows",
            "Workflow id is not discovered",
            "create its canonical .agents/workflows/<name>.yaml resource before inspection",
        );
    };
    if matches!(
        admission,
        crate::runtime::capability_inspection::WorkflowInspection::Disabled(_)
    ) {
        report.validity = Validity::Incomplete;
    }
    report.workflow = Some(WorkflowProjection {
        id: id.clone(),
        source,
        admission: admission.clone(),
        program: explain.then(|| {
            launch
                .workflows
                .get(id)
                .expect("admission belongs to discovered source")
                .inspect()
        }),
    });
    report
}
