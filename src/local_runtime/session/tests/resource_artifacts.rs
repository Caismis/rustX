//! Resource attachment identities are projected from remapped runtime references.

use super::*;
use crate::local_runtime::session::LineageSide;
use crate::message::content::FileReference;
use crate::runtime::types::CancellationReason;
use crate::runtime::{CancellationSignal, ExecutionCancellation};
use crate::scripted_suites::common;
use crate::tools::ArtifactStore;
use crate::tools::executor::ToolExecutionContext;
use crate::tools::types::ToolResultContent;

#[tokio::test]
async fn resource_blob_model_references_follow_real_clone_and_fork_remapping() {
    verify_resource_lineage("fixture://ten-blobs");
    verify_resource_lineage("fixture://ten-blobs-spilled");
}

fn verify_resource_lineage(uri: &str) {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let database = catalog.database_path(&session, &conversation);
    let artifacts = ArtifactStore::new(conversation.clone(), database.parent().unwrap()).unwrap();
    let fixture = common::native_fixture();
    let progress = common::NoopProgress;
    let context = ToolExecutionContext::new(
        &conversation,
        None,
        ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &progress,
        &artifacts,
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    let result = crate::tools::mcp::translate_fixture_resource(uri, &context);
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(result.artifacts.len(), 10);
    assert_eq!(result.artifacts[1].artifact_id.as_str(), "artifact_2");
    assert_eq!(result.artifacts[9].artifact_id.as_str(), "artifact_10");
    assert_eq!(result.managed_output.is_some(), uri.ends_with("-spilled"));
    assert_resource_descriptors(&result);
    let mut history = source_history();
    let MessageBlock::Tool(tool) = &mut history[2] else {
        panic!()
    };
    tool.result = result;
    let source_store = store_for(&catalog, &session, &conversation);
    for message in &history {
        source_store.append_canonical(message).unwrap();
    }
    let source = lineage_at(
        &source_store,
        &conversation,
        source_store.load_head().unwrap().revision,
    );
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .unwrap();
    for destination in [&clone, &fork] {
        catalog
            .publish_session(destination, SessionNodeOrigin::New)
            .unwrap();
    }
    drop(catalog);
    let reopened = reopen_catalog(root.path());
    for destination in [&clone, &fork] {
        let copied = store_for(
            &reopened,
            &destination.session_id,
            &destination.conversation_id,
        )
        .load_canonical()
        .unwrap();
        let MessageBlock::Tool(tool) = &copied[2] else {
            panic!()
        };
        verify_copied_references(&tool.result, destination);
        assert_resource_descriptors(&tool.result);
    }
}

fn verify_copied_references(result: &ToolExecutionResult, destination: &PreparedLineage) {
    // Lexical source-ID traversal copies 1, 10, 2, ... into consecutive IDs.
    assert_eq!(result.artifacts[1].artifact_id.as_str(), "artifact_3");
    assert_eq!(result.artifacts[9].artifact_id.as_str(), "artifact_2");
    let owner = ArtifactStore::new(
        destination.conversation_id.clone(),
        destination.database_path.parent().unwrap(),
    )
    .unwrap();
    let projection = result.model_facing_projection().as_text();
    let references: Vec<FileReference> = projection
        .lines()
        .filter_map(|line| line.strip_prefix("Artifact reference: "))
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();
    assert_eq!(references, result.artifacts);
    for (index, reference) in references.iter().enumerate() {
        assert_eq!(
            reference.name.as_deref(),
            Some(format!("fixture://blob-{index}").as_str())
        );
        assert_eq!(
            owner.read_bounded(&reference.artifact_id).unwrap(),
            format!("resource-blob-{index}").as_bytes()
        );
    }
}

fn assert_resource_descriptors(result: &ToolExecutionResult) {
    let value = if let Some(crate::tools::types::ManagedOutputContinuation::Complete { locator }) =
        result.managed_output.as_deref()
    {
        serde_json::from_str(&fs::read_to_string(locator).unwrap()).unwrap()
    } else if let ToolResultContent::Json { value } = &result.content[0] {
        value.clone()
    } else {
        panic!("resource descriptors are JSON or a complete JSON spill")
    };
    for (index, block) in value["contents"].as_array().unwrap().iter().enumerate() {
        assert!(block.get("artifactId").is_none());
        assert_eq!(
            block["_meta"]["artifactId"],
            format!("remote-artifact-{index}")
        );
    }
}
