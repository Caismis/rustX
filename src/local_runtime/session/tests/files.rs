//! Session file history and managed image regressions across clone, fork and
//! branch cuts, including source deletion and mapping loss.

use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real read/copy/publication/deletion transaction across all lineage cuts.
async fn managed_images_materialize_exact_clone_fork_branch_cuts() {
    use super::super::{LineageSide, deletion::SessionDeleteResult};
    use crate::model::{catalog::ModelRef, invocation::ModelSelection, types::ModelProtocol};
    use crate::scripted_suites::{common, support::model};
    use crate::tools::{
        ArtifactStore,
        executor::{PreflightOutcome, ToolExecutionContext},
        types::*,
    };
    use std::sync::Arc;
    let (_root, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &[]);
    let database = catalog.database_path(&session, &conversation);
    let artifacts = ArtifactStore::new(conversation.clone(), database.parent().unwrap()).unwrap();
    artifacts.put_bounded(b"irrelevant").unwrap();
    let fixture = common::native_fixture();
    let path = fixture.runtime.workspace().root().join("snapshot.png");
    let mut bytes = Vec::new();
    png::Encoder::new(&mut bytes, 1, 1)
        .write_header()
        .unwrap()
        .write_image_data(&[128])
        .unwrap();
    fs::write(&path, &bytes).unwrap();
    let mut definition =
        model::FixtureModel::text("fixture/image", ModelProtocol::AnthropicMessages);
    definition.extra_input_modalities.push("image");
    let invocation = model::fixture_registry(
        &[definition],
        &model::ScriptedAdapterFactory::new(Arc::new(model::NullAdapter)),
    )
    .resolve(&ModelSelection::of(
        ModelRef::parse("fixture/image").unwrap(),
    ))
    .unwrap();
    let call = ToolCall {
        id: crate::runtime::identity::ToolCallId::new("source-call"),
        tool_id: crate::runtime::identity::ToolId::new("tool-read-image"),
        name: "read_image".into(),
        arguments: serde_json::json!({"path":path}),
    };
    let PreflightOutcome::Ready(prepared) = fixture.registry.preflight(&call).unwrap() else {
        panic!()
    };
    let progress = common::NoopProgress;
    let mut context = ToolExecutionContext::new(
        &conversation,
        None,
        crate::runtime::ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &progress,
        &artifacts,
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    context.model_invocation = Some(&invocation);
    let result = fixture
        .registry
        .executor(&call.tool_id)
        .start(prepared.invocation, context)
        .completion
        .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let ToolResultContent::Image(original) = &result.content[1] else {
        panic!()
    };
    assert_eq!(original.artifact_id.as_str(), "artifact_2");
    let mut history = source_history();
    let MessageBlock::Assistant(assistant) = &mut history[1] else {
        panic!()
    };
    let crate::message::types::AssistantContentBlock::ToolCall(tool_call) =
        &mut assistant.content[1]
    else {
        panic!()
    };
    *tool_call = call.clone();
    let MessageBlock::Tool(tool) = &mut history[2] else {
        panic!()
    };
    tool.tool_id = call.tool_id;
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
    fs::write(&path, b"replaced").unwrap();
    fs::remove_file(&path).unwrap();
    let before = fs::read_dir(&catalog.root).unwrap().count();
    let mut missing = source.clone();
    let MessageBlock::Tool(tool) = &mut missing.canonical[2] else {
        panic!()
    };
    tool.result.content.push(ToolResultContent::Image(
        crate::message::content::ImageReference {
            artifact_id: crate::runtime::ArtifactId::new("artifact_999"),
            alt: None,
        },
    ));
    assert!(catalog.prepare_clone_session(&state(), &missing).is_err());
    assert_eq!(
        fs::read_dir(&catalog.root).unwrap().count(),
        before,
        "failed copy removes its private allocation"
    );
    let rejected = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog.arm_write_fault_before_rename();
    assert!(
        catalog
            .publish_session(&rejected, SessionNodeOrigin::New)
            .is_err()
    );
    assert!(catalog.snapshot(&rejected.session_id).is_err());
    catalog.discard_prepared_session(&rejected).unwrap();
    assert!(!rejected.database_path.parent().unwrap().exists());
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .unwrap();
    let (branch, _) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .unwrap();
    let (empty, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-a"),
            LineageSide::Before,
        )
        .unwrap();
    assert!(
        ArtifactStore::archive_lengths(empty.database_path.parent().unwrap(), || Ok(()))
            .unwrap()
            .is_empty()
    );
    catalog.discard_prepared_session(&empty).unwrap();
    catalog
        .publish_session(&clone, SessionNodeOrigin::New)
        .unwrap();
    catalog
        .publish_session(&fork, SessionNodeOrigin::New)
        .unwrap();
    catalog
        .publish_node(&session, &branch, node, SessionNodeOrigin::New)
        .unwrap();
    let verify = |destination: &PreparedLineage| {
        let reopened = store_for(
            &catalog,
            &destination.session_id,
            &destination.conversation_id,
        );
        let history = reopened.load_canonical().unwrap();
        let mut messages = crate::model::input::canonical_input(&history);
        crate::model::images::project(&mut messages, true);
        let owner = ArtifactStore::new(
            destination.conversation_id.clone(),
            destination.database_path.parent().unwrap(),
        )
        .unwrap();
        let resolved = crate::model::images::resolve(&messages, |id| {
            owner.read_bounded(id).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved.values().next().unwrap(), &bytes);
        assert_eq!(resolved.keys().next().unwrap().as_str(), "artifact_1");
        assert_eq!(
            ArtifactStore::archive_lengths(owner.root(), || Ok(()))
                .unwrap()
                .len(),
            1
        );
    };
    verify(&clone);
    verify(&fork);
    verify(&branch);
    drop(source_store);
    drop(artifacts);
    // Delete through the ordinary Session owner; independent destinations survive.
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&session) else {
        panic!()
    };
    let work = catalog
        .commit_delete(&session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    for destination in [&clone, &fork] {
        let owner = ArtifactStore::new(
            destination.conversation_id.clone(),
            destination.database_path.parent().unwrap(),
        )
        .unwrap();
        assert_eq!(
            owner
                .read_bounded(&crate::runtime::ArtifactId::new("artifact_1"))
                .unwrap(),
            bytes
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real canonical copy/deletion contract across all lineage cuts.
async fn session_file_history_copies_preserve_original_scope_and_deleted_source_is_unavailable() {
    use super::super::{LineageSide, deletion};
    use crate::tools::session_files::{declare, read_authorized};
    let (catalog_root, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &[]);
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    fs::write(cwd.join("报告 file.md"), b"ORIGINAL").unwrap();
    let reference = declare(&cwd, &conversation, "报告 file.md", None).unwrap();
    let mut history = source_history();
    let MessageBlock::Tool(tool) = &mut history[2] else {
        panic!()
    };
    tool.result.deliveries = vec![reference.clone()];
    let store = store_for(&catalog, &session, &conversation);
    for message in &history {
        store.append_canonical(message).unwrap();
    }
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .unwrap();
    let (branch, _) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .unwrap();
    catalog
        .publish_session(&clone, SessionNodeOrigin::New)
        .unwrap();
    catalog
        .publish_session(&fork, SessionNodeOrigin::New)
        .unwrap();
    catalog
        .publish_node(&session, &branch, node.clone(), SessionNodeOrigin::New)
        .unwrap();
    let reopened = reopen_catalog(catalog_root.path());
    assert_eq!(
        reopened.file_source(&conversation).unwrap(),
        (session.clone(), node.clone())
    );
    for destination in [&clone, &fork, &branch] {
        let copied = store_for(
            &reopened,
            &destination.session_id,
            &destination.conversation_id,
        )
        .load_canonical()
        .unwrap();
        let deliveries: Vec<_> = copied
            .iter()
            .filter_map(|block| match block {
                MessageBlock::Tool(tool) => Some(&tool.result.deliveries),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(deliveries, vec![&reference]);
        assert_ne!(destination.conversation_id, reference.scope.conversation_id);
        assert!(
            crate::tools::ArtifactStore::archive_lengths(
                destination.database_path.parent().unwrap(),
                || Ok(())
            )
            .unwrap()
            .is_empty()
        );
    }
    assert_eq!(
        catalog.file_source(&conversation).unwrap(),
        (session.clone(), node)
    );
    let unrelated = tempfile::tempdir().unwrap();
    fs::write(unrelated.path().join(&reference.path), b"UNRELATED").unwrap();
    assert!(
        read_authorized(
            &unrelated.path().canonicalize().unwrap(),
            &reference,
            || Ok(())
        )
        .is_err()
    );
    fs::write(cwd.join(&reference.path), b"CURRENT").unwrap();
    assert_eq!(
        read_authorized(&cwd, &reference, || Ok(())).unwrap(),
        b"CURRENT"
    );
    let deletion::SessionDeleteResult::Preview { preview } = catalog.delete_preview(&session)
    else {
        panic!()
    };
    let work = catalog
        .commit_delete(&session, &preview.target_revision)
        .unwrap()
        .unwrap();
    assert!(catalog.file_source(&conversation).is_err());
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        deletion::SessionDeleteResult::Deleted { .. }
    ));
    // The copied canonical facts remain truthful references, without bytes or fallback.
    for destination in [&clone, &fork] {
        assert!(catalog.snapshot(&destination.session_id).is_ok());
        let copied = store_for(
            &catalog,
            &destination.session_id,
            &destination.conversation_id,
        )
        .load_canonical()
        .unwrap();
        assert!(copied.iter().any(|block| matches!(block, MessageBlock::Tool(tool) if tool.result.deliveries == [reference.clone()])));
    }
    assert!(
        catalog
            .file_source(&reference.scope.conversation_id)
            .is_err()
    );
}

#[test]
fn session_file_mapping_loss_between_open_and_read_returns_no_bytes() {
    use crate::tools::session_files::{declare, read_authorized, read_failure, unavailable};
    let (_catalog_root, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &[]);
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    fs::write(cwd.join("file.txt"), b"ORIGINAL").unwrap();
    let reference = declare(&cwd, &conversation, "file.txt", None).unwrap();
    let mut original = state();
    original.cwd = cwd.clone();
    let revision = catalog.settings_revision(&session).unwrap();
    let revision = catalog
        .replace_settings(&session, revision, original)
        .unwrap();
    assert!(catalog.file_mapping_matches(&conversation, &session, &node, &cwd));
    let unrelated = tempfile::tempdir().unwrap();
    fs::write(unrelated.path().join("file.txt"), b"UNRELATED").unwrap();
    let mut replacement = state();
    replacement.cwd = unrelated.path().canonicalize().unwrap();
    let catalog = std::cell::RefCell::new(catalog);
    let checks = std::cell::Cell::new(0);
    let result = read_authorized(&cwd, &reference, || {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            // The descriptor is open, but no bytes have been read yet.
            catalog
                .borrow_mut()
                .replace_settings(&session, revision, replacement.clone())
                .unwrap();
        }
        if catalog
            .borrow()
            .file_mapping_matches(&conversation, &session, &node, &cwd)
        {
            Ok(())
        } else {
            Err(unavailable())
        }
    });
    assert_eq!(checks.get(), 2);
    assert_eq!(
        read_failure(&result.unwrap_err()),
        crate::tools::session_files::SessionFileReadFailure::Unavailable
    );
    assert_eq!(
        catalog.borrow().file_source(&conversation).unwrap(),
        (session, node)
    );
}
