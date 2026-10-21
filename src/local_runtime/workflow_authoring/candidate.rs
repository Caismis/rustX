//! Canonical candidate bytes of one Workflow source mutation.
//!
//! Called by the native source commit transaction while it holds the
//! destination's writer lock and has verified the expected revision. Every
//! written definition is compiled under the destination identity first and
//! serialized by the native YAML serializer; nothing invalid is staged, so a
//! rejected candidate leaves the original file untouched.
use super::{SourceDirectory, WorkflowDiagnostic, file_name};
use crate::local_runtime::configuration::settings::SettingsError;
use crate::runtime::resources::SourceScope;
use crate::runtime::workflow::{MAX_WORKFLOW_BYTES, WorkflowDefinition, WorkflowId};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::Path;

/// One whole-file Workflow mutation of the exact file the mutation names.
///
/// The surrounding source write carries the destination's expected
/// revision: the absent revision for `create` and `copy`, the exact current
/// revision for `replace` and `delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowEdit {
    /// Create the native minimal valid program: an entry Return of an empty
    /// object, with matching empty object input and output contracts.
    Create { description: String },
    /// Replace the whole file with one native-validated definition.
    Replace { document: Box<WorkflowDefinition> },
    /// Copy another file of the same scope at exactly `source_revision`.
    Copy {
        source: WorkflowId,
        source_revision: String,
    },
    /// Remove exactly this file.
    Delete,
}

fn invalid(diagnostic: WorkflowDiagnostic) -> SettingsError {
    SettingsError::InvalidWorkflow(Box::new(diagnostic))
}

/// The minimal valid program a new file starts from.
fn minimal(description: &str) -> Result<WorkflowDefinition, SettingsError> {
    let empty =
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false});
    // Omitting `timeout_ms` takes the native default exactly as a file would.
    serde_json::from_value(serde_json::json!({
        "description": description,
        "block": {
            "input": empty,
            "output": empty,
            "entry": "done",
            "nodes": {"done": {"type": "return", "output": {"type": "literal", "value": {}}}},
        },
    }))
    .map_err(|_| SettingsError::Invalid)
}

/// Compile `definition` under `id` and serialize it canonically. The
/// serialized bytes must parse back to exactly the compiled definition.
pub(in crate::local_runtime) fn encode(
    id: &WorkflowId,
    path: &Path,
    definition: &WorkflowDefinition,
) -> Result<Vec<u8>, SettingsError> {
    crate::local_runtime::workflow_resources::compile_definition(id, path, definition.clone())
        .map_err(|error| invalid(WorkflowDiagnostic::from_resource(&error)))?;
    let mut document = serde_yaml::to_value(definition).map_err(|_| SettingsError::Invalid)?;
    if let Some(block) = document.get_mut("block") {
        omit_ordinary_ports(block);
    }
    let text = serde_yaml::to_string(&document).map_err(|_| SettingsError::Invalid)?;
    let reread: WorkflowDefinition =
        serde_yaml::from_str(&text).map_err(|_| SettingsError::Invalid)?;
    if reread != *definition {
        return Err(SettingsError::Invalid);
    }
    if text.len() > MAX_WORKFLOW_BYTES {
        return Err(invalid(WorkflowDiagnostic::new(
            "$",
            "size",
            "the serialized Workflow file exceeds the native program bound",
        )));
    }
    Ok(text.into_bytes())
}

/// An ordinary edge's absent port serializes as `port: null`; omit it, as
/// authored files do. Only the `edges` of each lexical block are touched,
/// found structurally — never authored literal or schema data. The caller
/// proves the result parses back to exactly the compiled definition.
fn omit_ordinary_ports(block: &mut serde_yaml::Value) {
    use serde_yaml::Value;
    if let Some(Value::Sequence(edges)) = block.get_mut("edges") {
        for edge in edges {
            if let Value::Mapping(edge) = edge
                && edge.get("port") == Some(&Value::Null)
            {
                edge.remove("port");
            }
        }
    }
    if let Some(Value::Mapping(nodes)) = block.get_mut("nodes") {
        for node in nodes.values_mut() {
            match node.get("type").and_then(Value::as_str) {
                Some("loop") => {
                    if let Some(body) = node.get_mut("body") {
                        omit_ordinary_ports(body);
                    }
                }
                Some("parallel") => {
                    if let Some(Value::Mapping(branches)) = node.get_mut("branches") {
                        for branch in branches.values_mut() {
                            if let Some(child) = branch.get_mut("block") {
                                omit_ordinary_ports(child);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The candidate bytes of `edit` (`None` removes the file).
///
/// `original` is the destination's current content, already verified
/// against the expected revision. A copy source is read through the same
/// admitted directory handle, so it is the same scope's file and is never
/// reached through a redirected path.
pub(in crate::local_runtime) fn candidate(
    id: &WorkflowId,
    edit: WorkflowEdit,
    original: Option<&[u8]>,
    directory: &SourceDirectory,
    path: &Path,
    scope: SourceScope,
) -> Result<Option<Vec<u8>>, SettingsError> {
    match edit {
        WorkflowEdit::Create { description } => {
            if original.is_some() {
                return Err(SettingsError::Invalid);
            }
            encode(id, path, &minimal(&description)?).map(Some)
        }
        WorkflowEdit::Replace { document } => {
            if original.is_none() {
                return Err(SettingsError::Invalid);
            }
            encode(id, path, &document).map(Some)
        }
        WorkflowEdit::Copy {
            source,
            source_revision,
        } => {
            if original.is_some() || source == *id {
                return Err(SettingsError::Invalid);
            }
            let source_name = file_name(&source);
            let bytes = directory
                .read(OsStr::new(&source_name))
                .map_err(|_| SettingsError::Io)?;
            let actual = crate::local_runtime::settings::revision(bytes.as_deref());
            if actual != source_revision {
                return Err(SettingsError::Conflict {
                    scope,
                    expected: source_revision,
                    actual,
                });
            }
            let source_path = path.with_file_name(&source_name);
            let definition = crate::local_runtime::workflow_resources::parse_source(
                &source,
                &source_path,
                bytes.as_deref().unwrap_or_default(),
            )
            .map_err(|error| invalid(WorkflowDiagnostic::from_resource(&error)))?;
            encode(id, path, &definition).map(Some)
        }
        WorkflowEdit::Delete => {
            if original.is_none() {
                return Err(SettingsError::Invalid);
            }
            Ok(None)
        }
    }
}
