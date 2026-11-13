//! Native Workflow file ownership. Presentation never enters the compiler.
use crate::runtime::workflow::{WorkflowBlock, WorkflowDefinition};
use serde::{Deserialize, Serialize};

mod editor;
mod expansion;
#[cfg(test)]
mod tests;
pub use editor::*;

/// Complete authored source, with the executable fields at their native YAML
/// locations. Files without `editor` have deterministic derived presentation.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFileDocument {
    #[serde(flatten)]
    pub definition: WorkflowDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<WorkflowEditorMetadata>,
}

// Explicit wire fields keep duplicate/unknown-field rejection, which serde's
// flatten deserializer cannot guarantee together with deny_unknown_fields.
impl<'de> Deserialize<'de> for WorkflowFileDocument {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            description: String,
            #[serde(default = "default_timeout")]
            timeout_ms: u64,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
            block: WorkflowBlock,
            #[serde(default)]
            editor: Option<WorkflowEditorMetadata>,
        }
        fn default_timeout() -> u64 {
            crate::runtime::workflow::default_workflow_timeout_ms()
        }
        let file = File::deserialize(deserializer)?;
        Ok(Self {
            definition: WorkflowDefinition {
                description: file.description,
                timeout_ms: file.timeout_ms,
                permissions: file.permissions,
                block: file.block,
            },
            editor: file.editor,
        })
    }
}

impl From<WorkflowDefinition> for WorkflowFileDocument {
    fn from(definition: WorkflowDefinition) -> Self {
        Self {
            definition,
            editor: None,
        }
    }
}

impl std::ops::Deref for WorkflowFileDocument {
    type Target = WorkflowDefinition;
    fn deref(&self) -> &Self::Target {
        &self.definition
    }
}
impl std::ops::DerefMut for WorkflowFileDocument {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.definition
    }
}

/// Independent source-file and expanded-document byte bounds, separate from executable budgets.
pub const MAX_WORKFLOW_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WORKFLOW_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

impl WorkflowFileDocument {
    /// Validate file/presentation capacity without requiring a finished graph.
    /// Orphaned addresses are retained and diagnosed by `presentation_diagnostics`.
    /// # Errors
    /// Returns a presentation-specific rejection, or a whole-file size failure.
    pub fn validate(&self) -> Result<(), WorkflowPresentationDiagnostic> {
        if let Some(editor) = &self.editor {
            editor.validate()?;
        }
        if !crate::bounded_file::json_fits(self, MAX_WORKFLOW_DOCUMENT_BYTES) {
            return Err(WorkflowPresentationDiagnostic::new(
                "$",
                "file_size",
                "expanded Workflow document exceeds 16 MiB",
            ));
        }
        Ok(())
    }

    /// The complete canonical candidate. Compilation and executable validation
    /// remain the caller's separate admission step.
    /// # Errors
    /// Rejects invalid presentation or a serialized file exceeding 16 MiB.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkflowPresentationDiagnostic> {
        self.validate()?;
        let failure = || {
            WorkflowPresentationDiagnostic::new(
                "$",
                "file_size",
                "Workflow file cannot be serialized within 16 MiB",
            )
        };
        let mut value = serde_yaml::to_value(self).map_err(|_| failure())?;
        if let Some(block) = value.get_mut("block") {
            omit_ordinary_ports(block);
        }
        let mut encoded =
            crate::bounded_file::LimitedWriter::new(Vec::new(), MAX_WORKFLOW_FILE_BYTES);
        serde_yaml::to_writer(&mut encoded, &value).map_err(|_| failure())?;
        let bytes = encoded.inner;
        if bytes.len() > MAX_WORKFLOW_FILE_BYTES {
            return Err(failure());
        }
        let reread: Self = serde_yaml::from_slice(&bytes).map_err(|_| failure())?;
        if reread != *self {
            return Err(failure());
        }
        Ok(bytes)
    }

    /// Bounded repair diagnostics; no authored content is erased or retargeted.
    #[must_use]
    pub fn presentation_diagnostics(&self) -> Vec<WorkflowPresentationDiagnostic> {
        if let Err(error) = self.validate() {
            return vec![error];
        }
        self.editor
            .as_ref()
            .map_or_else(Vec::new, |editor| editor.diagnostics(&self.definition))
    }
}

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
            if matches!(
                node.get("type").and_then(Value::as_str),
                Some("loop" | "iteration")
            ) && let Some(body) = node.get_mut("body")
            {
                omit_ordinary_ports(body);
            }
        }
    }
}

pub(crate) fn yaml_expansion_within_bound(bytes: &[u8]) -> bool {
    expansion::within_bound(bytes)
}
