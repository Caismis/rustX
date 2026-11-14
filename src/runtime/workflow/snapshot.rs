//! Immutable admission-bound Workflow run definition snapshots (Issue #552).
//!
//! Captured at program admission/compilation from the exact executable
//! definition and same-revision presentation metadata, never by rereading the
//! mutable source file after a run starts or substituting another revision's
//! layout.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{WorkflowId, WorkflowRunId};
use crate::workflow_file::{MAX_WORKFLOW_DOCUMENT_BYTES, WorkflowFileDocument};

/// Maximum bytes transferred in one snapshot chunk read, matching native `ContentRead`.
pub const WORKFLOW_SNAPSHOT_CHUNK_BYTES: usize = 65_536;

/// Complete declarative Workflow definition + same-revision presentation
/// snapshot bound to an admitted [`WorkflowProgram`](super::WorkflowProgram).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunDefinitionSnapshot {
    pub workflow_id: WorkflowId,
    pub program_digest: String,
    pub source_revision: String,
    pub has_source_presentation: bool,
    pub document: WorkflowFileDocument,
}

/// Compact header describing an immutable snapshot bound to one Workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunSnapshotHeader {
    pub run_id: WorkflowRunId,
    pub workflow_id: WorkflowId,
    pub program_digest: String,
    pub source_revision: String,
    pub snapshot_digest: String,
    pub has_source_presentation: bool,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub byte_length: u64,
    pub sha256: String,
}

/// Bounded chunk of an immutable run snapshot document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunSnapshotChunk {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    pub data: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}

/// Canonical SHA-256 hex digest of an arbitrary byte slice.
#[must_use]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Encodes and validates the snapshot's declarative `WorkflowFileDocument` for
/// durable SQLite persistence, returning canonical JSON bytes, their SHA-256,
/// and the snapshot's content+identity digest.
///
/// # Errors
/// Returns a message when validation fails or normalized bytes exceed
/// `MAX_WORKFLOW_DOCUMENT_BYTES`.
pub(crate) fn encode_snapshot_document(
    snapshot: &WorkflowRunDefinitionSnapshot,
) -> Result<(Vec<u8>, String, String), String> {
    snapshot.document.validate().map_err(|error| error.reason)?;
    let mut writer =
        crate::bounded_file::LimitedWriter::new(Vec::new(), MAX_WORKFLOW_DOCUMENT_BYTES);
    serde_json::to_writer(&mut writer, &snapshot.document).map_err(|error| error.to_string())?;
    let bytes = writer.inner;
    let payload_sha256 = sha256_hex(&bytes);
    let snapshot_digest = compute_snapshot_digest(
        &snapshot.workflow_id,
        &snapshot.program_digest,
        &snapshot.source_revision,
        snapshot.has_source_presentation,
        &payload_sha256,
    );
    Ok((bytes, payload_sha256, snapshot_digest))
}

#[must_use]
pub(crate) fn compute_snapshot_digest(
    workflow_id: &WorkflowId,
    program_digest: &str,
    source_revision: &str,
    has_source_presentation: bool,
    payload_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workflow_id.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(program_digest.as_bytes());
    hasher.update([0]);
    hasher.update(source_revision.as_bytes());
    hasher.update([0]);
    hasher.update([u8::from(has_source_presentation)]);
    hasher.update([0]);
    hasher.update(payload_sha256.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
pub(crate) fn test_run_snapshot(workflow_id: WorkflowId) -> WorkflowRunDefinitionSnapshot {
    let document: WorkflowFileDocument = serde_json::from_value(serde_json::json!({
        "description": "test workflow",
        "block": {
            "input": { "type": "object", "properties": {}, "additionalProperties": false },
            "nodes": {
                "start": { "type": "start" },
                "done": { "type": "return", "outputs": [] }
            },
            "edges": [{ "from": "start", "to": "done" }]
        }
    }))
    .expect("valid minimal test workflow document");
    super::WorkflowProgram::compile_document(workflow_id, "sha256:test-run-snapshot", document)
        .expect("compile minimal test workflow")
        .run_snapshot()
        .as_ref()
        .clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn inspection_bounds_fixture_is_admissible() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../web-console/test/fixtures/workflow-run-bounds.json"
        ))
        .unwrap();
        for key in ["maximum", "long_history"] {
            let document: crate::workflow_file::WorkflowFileDocument =
                serde_json::from_value(fixtures[key].clone()).unwrap();
            let program = super::super::WorkflowProgram::compile_document(
                super::super::WorkflowId::parse(key).unwrap(),
                "sha256:bounds-fixture",
                document,
            )
            .unwrap_or_else(|error| panic!("{key}: {error}"));
            assert!(program.inspect().conservative_steps <= super::super::MAX_WORKFLOW_STEPS);
        }
    }
}
