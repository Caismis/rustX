//! Bounded trusted Workflow source resolution, shared by prospective analysis and reload.
use super::config::WorkflowsDocument;
use crate::runtime::resources::RuntimeResourceLoadError;
use crate::runtime::workflow::{
    MAX_WORKFLOW_BYTES, WorkflowCatalog, WorkflowCompileError, WorkflowDefinition, WorkflowProgram,
};
use std::path::{Path, PathBuf};
const AGENT_RESOURCES_DIRECTORY: &str = ".agents";
const WORKFLOW_RESOURCES_DIRECTORY: &str = "workflows";

/// Loads and compiles exactly the configured Workflow definitions.
///
/// The configured id is the only filesystem identity: a registered `id` is
/// read from `.agents/workflows/{id}.yaml`. Directory contents are never
/// scanned, so an unregistered YAML file cannot become model-visible by
/// accident. Compilation happens before the candidate reaches the runtime
/// resource publication boundary.
pub(crate) fn load(
    workspace: &Path,
    document: &WorkflowsDocument,
    profiles: &super::config::SubagentsDocument,
) -> Result<WorkflowCatalog, RuntimeResourceLoadError> {
    let mut programs = Vec::with_capacity(document.definitions.len());
    for id in &document.definitions {
        let path = workspace_workflow_path(workspace, id);
        crate::runtime::resources::validate_project_resource_path(workspace, &path)
            .map_err(|error| error.at(&path, format!("workflows.definitions.{id}")))?;
        let bytes = crate::bounded_file::read_bounded(&path).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "cannot read registered workflow {id} at {}: {error}",
                path.display()
            ))
            .at(&path, format!("workflows.definitions.{id}"))
        })?;
        if bytes.len() > MAX_WORKFLOW_BYTES {
            return Err(RuntimeResourceLoadError::new(format!(
                "workflow {id} at {} exceeds the {MAX_WORKFLOW_BYTES}-byte bound",
                path.display()
            ))
            .at(&path, format!("workflows.definitions.{id}")));
        }
        let definition: WorkflowDefinition =
            serde_path_to_error::deserialize(serde_yaml::Deserializer::from_slice(&bytes))
                .map_err(|error| {
                    let mut failure = RuntimeResourceLoadError::new(format!(
                        "cannot deserialize registered workflow {id} at {}: {error}",
                        path.display()
                    ))
                    .at(&path, safe_parser_path(error.path()))
                    .because(
                        "invalid Workflow YAML syntax, shape, duplicate key, or unknown field",
                    );
                    if let Some(location) = error.inner().location() {
                        failure.inspection.line = Some(location.line());
                        failure.inspection.column = Some(location.column());
                    }
                    failure.inspection.category = Some("workflow_language");
                    failure.inspection.correction = Some("correct this YAML using schemas/workflow.schema.json; fields and mapping keys must be unique");
                    failure
                })?;
        let program = WorkflowProgram::compile(
            id.clone(),
            definition,
            &profiles.workflow.iter().cloned().collect(),
        )
        .map_err(|error| {
            let mut failure = RuntimeResourceLoadError::new(format!(
                "cannot compile registered workflow {id} at {}: {error}",
                path.display()
            ))
            .at(&path, error.path())
            .because(match error.cause() {
                crate::runtime::workflow::WorkflowCompileError::ProfileNotAdmitted {
                    profile,
                    ..
                } if !profiles.definitions.contains(profile) => {
                    "named role is missing from subagents.definitions"
                }
                crate::runtime::workflow::WorkflowCompileError::ProfileNotAdmitted { .. } => {
                    "named role exists but is not admitted by subagents.workflow"
                }
                crate::runtime::workflow::WorkflowCompileError::InvalidReference(_) => {
                    "binding cannot resolve in this lexical scope on every incoming control path"
                }
                crate::runtime::workflow::WorkflowCompileError::IncompatibleReference(_) => {
                    "binding type does not satisfy the declared destination schema"
                }
                crate::runtime::workflow::WorkflowCompileError::InvalidSchema(_) => {
                    "schema violates the closed Workflow schema contract"
                }
                _ => "Workflow graph, control structure, or configured bound is invalid",
            });
            failure.inspection.category = Some(match error.cause() {
                WorkflowCompileError::ProfileNotAdmitted { profile, .. }
                    if !profiles.definitions.contains(profile) =>
                {
                    "resource_missing"
                }
                WorkflowCompileError::ProfileNotAdmitted { .. } => "resource_not_admitted",
                _ => "workflow_language",
            });
            failure.inspection.correction = Some(match error.cause() {
                WorkflowCompileError::ProfileNotAdmitted { .. } => "register the canonical .agents/subagents/<name>.md resource and explicitly admit its name in subagents.workflow",
                WorkflowCompileError::InvalidReference(_) => "bind args or earlier values guaranteed on every incoming path; nested blocks only see their explicitly projected input",
                WorkflowCompileError::IncompatibleReference(_) => "make the binding type, required fields and finite values satisfy the destination schema",
                WorkflowCompileError::InvalidSchema(_) => "use only the closed Workflow schema vocabulary: type, properties, required, additionalProperties, items, const and enum",
                _ => "use existing node ids and deterministic control ports; end every path with Return and respect the reported configured/native bound",
            });
            if !matches!(error.cause(), WorkflowCompileError::InvalidSchema(_)) {
                failure.inspection.detail = Some(format!(
                    "{}: {}",
                    failure.diagnostic_reason.unwrap_or("invalid Workflow"),
                    error.cause()
                ));
            }
            failure
        })?;
        programs.push(program);
    }
    WorkflowCatalog::new(programs, document.main.clone()).map_err(|error| {
        RuntimeResourceLoadError::new(format!("cannot admit Workflow catalog: {error}"))
    })
}

/// Maps one explicitly registered Workflow identity to its one source file.
/// This is intentionally not a discovery helper: the caller must provide the
/// id from `workflows.definitions`.
fn workspace_workflow_path(workspace: &Path, id: &crate::runtime::workflow::WorkflowId) -> PathBuf {
    workspace
        .join(AGENT_RESOURCES_DIRECTORY)
        .join(WORKFLOW_RESOURCES_DIRECTORY)
        .join(format!("{}.yaml", id.as_str()))
}

// Unknown failing field names are untrusted content (and may themselves contain
// credentials). Keep their typed container and parser coordinates, not that text.
// The structural field vocabulary comes from the authoring type, not a second schema.
fn safe_parser_path(path: &serde_path_to_error::Path) -> String {
    fn fields(schema: &serde_json::Value, names: &mut std::collections::BTreeSet<String>) {
        match schema {
            serde_json::Value::Object(object) => {
                if let Some(properties) = object
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                {
                    names.extend(properties.keys().cloned());
                }
                for value in object.values() {
                    fields(value, names);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    fields(value, names);
                }
            }
            _ => {}
        }
    }
    let schema = serde_json::to_value(schemars::schema_for!(WorkflowDefinition))
        .expect("native schema serializes");
    let mut names = std::collections::BTreeSet::new();
    fields(&schema, &mut names);
    let segments: Vec<_> = path.iter().collect();
    let mut output = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 && !matches!(segment, serde_path_to_error::Segment::Seq { .. }) {
            output.push('.');
        }
        if index + 1 == segments.len()
            && matches!(segment, serde_path_to_error::Segment::Map { key } if !names.contains(key))
        {
            output.push_str("<redacted_field>");
        } else {
            output.push_str(&segment.to_string());
        }
    }
    if output.is_empty() {
        "$".into()
    } else {
        output
    }
}
