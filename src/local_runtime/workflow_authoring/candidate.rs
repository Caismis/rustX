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
use crate::runtime::workflow::WorkflowId;
use crate::workflow_file::WorkflowFileDocument;
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
    /// `description` is for models; `display` is how people see the file.
    Create {
        description: String,
        #[serde(default)]
        display: Option<crate::workflow_file::WorkflowDisplay>,
    },
    /// Replace the whole file with one native-validated definition.
    Replace { document: Box<WorkflowFileDocument> },
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
fn minimal(
    description: &str,
    display: Option<crate::workflow_file::WorkflowDisplay>,
) -> Result<WorkflowFileDocument, SettingsError> {
    let empty =
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false});
    // Omitting `timeout_ms` takes the native default exactly as a file would.
    let mut document: WorkflowFileDocument = serde_json::from_value(serde_json::json!({
        "description": description,
        "block": {
            "input": empty,
            "output": empty,
            "nodes": {"start":{"type":"start"},"done": {"type": "return", "output": {"type": "literal", "value": {}}}},
            "edges": [{"from":"start","to":"done"}],
        },
    }))
    .map_err(|_| SettingsError::Invalid)?;
    document.editor = display.map(|display| crate::workflow_file::WorkflowEditorMetadata {
        display: Some(display),
        ..Default::default()
    });
    Ok(document)
}

/// Compile `definition` under `id` and serialize it canonically. The
/// serialized bytes must parse back to exactly the compiled definition.
/// These are exactly the bytes a `replace` of `definition` commits, so
/// their revision is the candidate's native identity.
pub(in crate::local_runtime) fn encode(
    id: &WorkflowId,
    path: &Path,
    definition: &WorkflowFileDocument,
) -> Result<Vec<u8>, SettingsError> {
    definition
        .validate()
        .map_err(|error| invalid(super::presentation_diagnostic(&error)))?;
    crate::local_runtime::workflow_resources::compile_definition(
        id,
        path,
        definition.definition.clone(),
    )
    .map_err(|error| invalid(WorkflowDiagnostic::from_resource(&error)))?;
    definition
        .canonical_bytes()
        .map_err(|error| invalid(super::presentation_diagnostic(&error)))
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
        WorkflowEdit::Create {
            description,
            display,
        } => {
            if original.is_some() {
                return Err(SettingsError::Invalid);
            }
            encode(id, path, &minimal(&description, display)?).map(Some)
        }
        WorkflowEdit::Replace { document } => {
            if original.is_none() {
                return Err(SettingsError::Invalid);
            }
            // A browser-authored document: it must not carry a number the
            // browser could not have preserved (see `unrepresentable_number`).
            if let Some(diagnostic) = super::unrepresentable_number(&document) {
                return Err(invalid(diagnostic));
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
