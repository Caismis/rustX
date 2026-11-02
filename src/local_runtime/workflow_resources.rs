//! Bounded two-scope Workflow discovery, shared by prospective analysis and immutable input capture.
use crate::runtime::resources::RuntimeResourceLoadError;
use crate::runtime::resources::SourceScope;
use crate::runtime::workflow::{
    WorkflowCatalog, WorkflowCompileError, WorkflowDefinition, WorkflowId, WorkflowProgram,
};
use crate::workflow_file::{MAX_WORKFLOW_FILE_BYTES, WorkflowFileDocument};
use std::path::Path;

/// Discover and compile canonical Workflow files before resource publication.
/// A missing Workspace means User-only discovery, with no cwd-derived authority.
pub(crate) fn load(
    workspace: Option<&Path>,
    user_root: &Path,
) -> Result<WorkflowCatalog, RuntimeResourceLoadError> {
    let mut programs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut candidates = std::collections::BTreeMap::new();
    let mut locations = std::collections::BTreeMap::new();
    for (boundary, root, scope) in
        std::iter::once((user_root, user_root.join("workflows"), SourceScope::User)).chain(
            workspace.map(|workspace| {
                (
                    workspace,
                    workspace.join(".agents/workflows"),
                    SourceScope::Workspace,
                )
            }),
        )
    {
        let paths = match super::resource_directory::files(boundary, &root, "yaml") {
            Ok(paths) => paths,
            Err(error) => {
                candidates.clear();
                locations.clear();
                diagnostics.push(error);
                continue;
            }
        };
        for path in paths {
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| crate::runtime::workflow::WorkflowId::parse(name).ok());
            let Some(id) = id else {
                diagnostics.push(
                    RuntimeResourceLoadError::new("invalid Workflow filename identity")
                        .at(&path, "workflows"),
                );
                continue;
            };
            let shadowed = locations
                .remove(&id)
                .map(|lower: crate::runtime::resources::ResourceLocation| lower.path);
            locations.insert(
                id.clone(),
                crate::runtime::resources::ResourceLocation {
                    scope,
                    path: path.clone(),
                    shadowed,
                },
            );
            candidates.insert(id, (path, boundary.to_path_buf()));
        }
    }
    let mut invalid = std::collections::BTreeMap::new();
    let mut presentation = std::collections::BTreeMap::new();
    if candidates.len() > crate::runtime::workflow::MAX_WORKFLOW_DEFINITIONS {
        diagnostics.push(
            RuntimeResourceLoadError::new("Workflow catalog exceeds its definition bound").at(
                &workspace.map_or_else(
                    || user_root.join("workflows"),
                    |workspace| workspace.join(".agents/workflows"),
                ),
                "workflows",
            ),
        );
        candidates.clear();
        locations.clear();
    }
    for (id, (path, boundary)) in candidates {
        let result = super::resource_directory::read_resource(&boundary, &path)
            .and_then(|bytes| parse_source(&id, &path, &bytes))
            .and_then(|document| {
                let warnings: Vec<_> = document.presentation_diagnostics().into_iter().map(|diagnostic| {
                    let mut error = RuntimeResourceLoadError::new(diagnostic.reason).at(&path, diagnostic.path)
                        .because("presentation target no longer exists; authored content is retained");
                    error.inspection.category = Some("workflow_presentation");
                    error
                }).collect();
                if !warnings.is_empty() { presentation.insert(id.clone(), warnings); }
                compile_definition(&id, &path, document.definition)
            });
        match result {
            Ok(program) => programs.push(program),
            Err(error) => {
                invalid.insert(id, error);
            }
        }
    }
    let mut catalog = WorkflowCatalog::new(programs).map_err(|error| {
        RuntimeResourceLoadError::new(format!("cannot admit Workflow catalog: {error}"))
    })?;
    catalog.set_invalid(invalid);
    catalog.locations = locations;
    catalog.discovery_diagnostics = diagnostics;
    catalog.presentation_diagnostics = presentation;
    Ok(catalog)
}

/// Parse one Workflow file's bytes with the native authoring type. The
/// diagnostic carries only native structural coordinates and static reasons.
pub(super) fn parse_source(
    id: &WorkflowId,
    path: &Path,
    bytes: &[u8],
) -> Result<WorkflowFileDocument, RuntimeResourceLoadError> {
    if bytes.len() > MAX_WORKFLOW_FILE_BYTES {
        return Err(RuntimeResourceLoadError::new(format!(
            "workflow {id} at {} exceeds the {MAX_WORKFLOW_FILE_BYTES}-byte bound",
            path.display()
        ))
        .at(path, format!("workflows.{id}")));
    }
    let document: WorkflowFileDocument = serde_path_to_error::deserialize(serde_yaml::Deserializer::from_slice(bytes)).map_err(
        |error| {
            let mut failure = RuntimeResourceLoadError::new(format!(
                "cannot deserialize discovered workflow {id} at {}: {error}",
                path.display()
            ))
            .at(path, safe_parser_path(error.path()))
            .because("invalid Workflow YAML syntax, shape, duplicate key, or unknown field");
            if let Some(location) = error.inner().location() {
                failure.inspection.line = Some(location.line());
                failure.inspection.column = Some(location.column());
            }
            failure.inspection.category = Some(if error.path().to_string().starts_with("editor") { "workflow_presentation" } else { "workflow_language" });
            failure.inspection.correction = Some(
                "correct this YAML using schemas/workflow.schema.json; fields and mapping keys must be unique",
            );
            failure
        },
    )?;
    document.validate().map_err(|error| {
        let mut failure = RuntimeResourceLoadError::new(error.reason.clone())
            .at(path, error.path)
            .because("invalid Workflow file or presentation metadata");
        failure.inspection.category = Some(if error.category == "file_size" {
            "workflow_file"
        } else {
            "workflow_presentation"
        });
        failure
    })?;
    Ok(document)
}

/// Compile one parsed definition under its filename identity, with the
/// discovery diagnostic contract.
pub(super) fn compile_definition(
    id: &WorkflowId,
    path: &Path,
    definition: WorkflowDefinition,
) -> Result<WorkflowProgram, RuntimeResourceLoadError> {
    WorkflowProgram::compile(id.clone(), definition).map_err(|error| {
        let mut failure = RuntimeResourceLoadError::new(format!(
            "cannot compile discovered workflow {id} at {}: {error}",
            path.display()
        ))
        .at(path, error.path())
        .because(match error.cause() {
            WorkflowCompileError::InvalidReference(_) => {
                "binding cannot resolve in this lexical scope on every incoming control path"
            }
            WorkflowCompileError::IncompatibleReference(_) => {
                "binding type does not satisfy the declared destination schema"
            }
            WorkflowCompileError::InvalidSchema(_) => {
                "schema violates the closed Workflow schema contract"
            }
            _ => "Workflow graph, control structure, or configured bound is invalid",
        });
        failure.inspection.category = Some("workflow_language");
        failure.inspection.correction = Some(match error.cause() {
            WorkflowCompileError::InvalidReference(_) => {
                "bind args or earlier values guaranteed on every incoming path; nested blocks only see their explicitly projected input"
            }
            WorkflowCompileError::IncompatibleReference(_) => {
                "make the binding type, required fields and finite values satisfy the destination schema"
            }
            WorkflowCompileError::InvalidSchema(_) => {
                "use only the closed Workflow schema vocabulary: type, properties, required, additionalProperties, items, const, enum, string maxLength, and bounded description and default annotations"
            }
            _ => {
                "use existing node ids and deterministic control ports; end every path with Return and respect the reported configured/native bound"
            }
        });
        if !matches!(error.cause(), WorkflowCompileError::InvalidSchema(_)) {
            failure.inspection.detail = Some(format!(
                "{}: {}",
                failure.diagnostic_reason.unwrap_or("invalid Workflow"),
                error.cause()
            ));
        }
        failure
    })
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
    let schema = serde_json::to_value(schemars::schema_for!(WorkflowFileDocument))
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

#[cfg(test)]
mod tests {
    use super::*;
    const PROGRAM: &str = "description: Return a literal\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  edges:\n  - from: workflow_start\n    to: done\n  nodes:\n    workflow_start:\n      type: start\n    done:\n      type: return\n      outputs: []\n";
    #[test]
    fn user_discovery_does_not_read_workspace_overrides() {
        let dir = tempfile::tempdir().unwrap();
        // The workspace argument is canonical authority; model the production
        // binding point instead of passing an aliased temporary root.
        let root = dir.path().canonicalize().unwrap();
        let user = root.join("user");
        std::fs::create_dir_all(user.join("workflows")).unwrap();
        std::fs::create_dir_all(root.join(".agents/workflows")).unwrap();
        std::fs::write(user.join("workflows/literal.yaml"), PROGRAM).unwrap();
        std::fs::write(root.join(".agents/workflows/literal.yaml"), "invalid: [").unwrap();
        let (catalog, effects) =
            super::super::static_effects::measure(|| load(None, &user).unwrap());
        let id = crate::runtime::workflow::WorkflowId::parse("literal").unwrap();
        assert!(catalog.entries().contains_key(&id));
        assert!(catalog.invalid().is_empty());
        assert_eq!(effects, [0; 11]);
        assert!(
            load(Some(&root), &user)
                .unwrap()
                .invalid()
                .contains_key(&id)
        );
    }

    #[test]
    fn workflow_discovery_and_errors_are_independent_of_creation_order() {
        for names in [["zeta", "alpha"], ["alpha", "zeta"]] {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().canonicalize().unwrap();
            let root = workspace.join(".agents/workflows");
            std::fs::create_dir_all(&root).unwrap();
            for name in names {
                std::fs::write(root.join(format!("{name}.yaml")), PROGRAM).unwrap();
            }
            std::fs::write(root.join("incidental.txt"), "invalid YAML: [").unwrap();
            let catalog = load(Some(&workspace), &workspace.join("user/.agents")).unwrap();
            assert_eq!(
                catalog
                    .entries()
                    .keys()
                    .map(crate::runtime::workflow::WorkflowId::as_str)
                    .collect::<Vec<_>>(),
                ["alpha", "zeta"]
            );
            assert!(catalog.enabled_ids().is_empty());
            for name in names {
                std::fs::write(root.join(format!("{name}.yaml")), "invalid: [").unwrap();
            }
            let invalid = load(Some(&workspace), &workspace.join("user/.agents")).unwrap();
            let error = invalid.invalid().values().next().unwrap();
            assert_eq!(error.source_file, Some(root.join("alpha.yaml")));
            assert_eq!(
                invalid.invalid(),
                load(Some(&workspace), &workspace.join("user/.agents"))
                    .unwrap()
                    .invalid()
            );
            assert_eq!(catalog.entries().len(), 2);
        }
    }
}
