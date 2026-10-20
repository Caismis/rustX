//! Clone, fork and tree-node publication: identity domains, retained surface
//! revisions, todo state, failed publication invisibility, and node switching.

use super::super::{SESSION_LIST_PAGE_LIMIT, SESSION_TREE_PAGE_LIMIT};
use super::compaction_lineage::todo_call;
use super::compaction_lineage::todo_list_of;
use super::compaction_lineage::todo_result;
use super::*;

/// Provenance is user work even when the destination conversation is
/// legitimately empty. A fork at the very first user message cuts to the
/// empty lineage, and a clone of an untouched lineage carries nothing —
/// yet both are used and resume-visible, because the unused
/// classification only ever fits the untouched root-`New` shell.
#[test]
fn clone_and_fork_with_empty_destinations_are_still_resume_visible() {
    let (_directory, mut catalog, _config) = open_catalog();
    let visible = |catalog: &SessionCatalog| {
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("resume page")
            .sessions
    };

    // Clone the still-unused root lineage: an empty copy of an empty
    // conversation, but an explicit user lineage operation.
    let (root_session, root_node, _) = catalog
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("root lineage");
    let root_store = store_for(&catalog, &root_session, &root_node.conversation_id);
    let root_revision = root_store.load_head().expect("root head").revision;
    let source = lineage_at(&root_store, &root_node.conversation_id, root_revision);
    let clone = catalog
        .prepare_clone_session(&state(), &source)
        .expect("prepare empty clone");
    let clone_id = clone.session_id.clone();
    catalog
        .publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session: root_session,
                source_node: root_node.id.clone(),
                source_surface_revision: root_revision,
            },
        )
        .expect("publish empty clone");
    assert!(
        !catalog.is_session_unused(&clone_id).expect("empty clone"),
        "an empty clone destination is used by provenance"
    );
    let page = visible(&catalog);
    assert_eq!(page.len(), 2, "both Sessions are visible");
    assert_eq!(page[0].id, clone_id);
    assert_eq!(
        page[0].preview, None,
        "an empty destination has no first-message line, yet the row exists"
    );

    // Fork the clone at its very first user message: the destination
    // seed is the empty prefix before that boundary.
    let (clone_session, clone_node, _) = catalog
        .lineage(&clone_id, None)
        .map(|(node, state)| (clone_id.clone(), node, state))
        .expect("clone lineage");
    let clone_store = store_for(&catalog, &clone_session, &clone_node.conversation_id);
    clone_store
        .append_canonical(&user("only-prompt", "the first prompt"))
        .unwrap();
    let revision = clone_store.load_head().expect("clone head").revision;
    let source = lineage_at(&clone_store, &clone_node.conversation_id, revision);
    let (fork, _editor) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("only-prompt"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare fork at the first user message");
    let fork_id = fork.session_id.clone();
    let fork_conversation = fork.conversation_id.clone();
    catalog
        .publish_session(
            &fork,
            SessionNodeOrigin::Fork {
                source_session: clone_session,
                source_node: clone_node.id.clone(),
                source_surface_revision: revision,
                source_message: MessageId::new("only-prompt"),
                source_locator: fixture_source_locator(&clone_store, "only-prompt"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .expect("publish empty fork");

    // The fork destination is genuinely empty — initial Surface, no
    // canonical message, no Pending Inbound — and used anyway.
    let fork_store = store_for(&catalog, &fork_id, &fork_conversation);
    assert!(
        fork_store
            .load_canonical()
            .expect("fork canonical")
            .is_empty()
    );
    assert_eq!(
        fork_store.load_head().expect("fork head").revision,
        SurfaceRevision::INITIAL
    );
    assert!(
        !catalog.is_session_unused(&fork_id).expect("empty fork"),
        "an empty fork destination is used by provenance"
    );
    let ids = visible(&catalog)
        .into_iter()
        .map(|summary| summary.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![fork_id, clone_id, first_session(&catalog)],
        "empty clone and empty fork are both resume-visible; the root shell is not"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One shared source exercises every product lineage cut.
fn lineage_product_paths_preserve_reused_provider_ids_without_compaction() {
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
    let (_directory, mut catalog, _config) = open_catalog();
    let assistant = |id: &str, index: usize| {
        let mut content = vec![
            AssistantContentBlock::Text(TextBlock {
                text: "reason".into()
            });
            index
        ];
        content.push(AssistantContentBlock::ToolCall(ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "pwd"}),
        }));
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content,
        })
    };
    let result = |id: &str, owner: &str, index: u32, text: &str| {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(id),
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new(owner),
                ContentBlockIndex::new(index),
            ),
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Text(TextBlock {
                    text: text.into(),
                })],
                duration_ms: 1,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        })
    };
    let history = vec![
        user("user-A", "A"),
        assistant("assistant-A", 1),
        result("result-A", "assistant-A", 1, "old-result"),
        user("user-B", "B"),
        assistant("assistant-B", 2),
        result("result-B", "assistant-B", 2, "new-result"),
        user("after-B", "continue"),
    ];
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().unwrap().revision;
    // Exactly the canonical/Surface/history materialization used by SessionController.
    let source = lineage_at(&source_store, &source_conversation, revision);
    assert_eq!(source.messages, source.canonical);
    assert!(
        source
            .surface_history
            .iter()
            .all(|op| matches!(op, crate::conversation::SurfaceOp::Append { .. }))
    );
    source_store.projected_history().unwrap();
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (early, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("user-B"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .unwrap();
    let (late, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("after-B"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .unwrap();
    let (tree, _) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("after-B"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .unwrap();
    for (destination, expected) in [(&clone, 2), (&early, 1), (&late, 2), (&tree, 2)] {
        // prepare_* has already called the real remap_seed -> initialize_lineage.
        let store = store_for(
            &catalog,
            &destination.session_id,
            &destination.conversation_id,
        );
        let copied = store.load_canonical().unwrap();
        store.projected_history().unwrap();
        let results: Vec<_> = copied
            .iter()
            .filter_map(|m| match m {
                MessageBlock::Tool(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), expected);
        for (index, tool) in results.iter().enumerate() {
            assert_eq!(tool.tool_call_id, ToolCallId::new("call-1"));
            assert_eq!(tool.occurrence.block_index.get() as usize, index + 1);
            assert_ne!(
                tool.occurrence.assistant_message_id.as_str(),
                ["assistant-A", "assistant-B"][index]
            );
            let owner = copied
                .iter()
                .find(|m| m.id() == &tool.occurrence.assistant_message_id)
                .unwrap();
            let MessageBlock::Assistant(owner) = owner else {
                panic!("Assistant owner")
            };
            let AssistantContentBlock::ToolCall(call) = &owner.content[index + 1] else {
                panic!("Tool block")
            };
            assert_eq!(call.id, tool.tool_call_id);
            assert_eq!(call.tool_id, tool.tool_id);
            assert_eq!(
                tool.result.content,
                vec![crate::tools::types::ToolResultContent::Text(TextBlock {
                    text: ["old-result", "new-result"][index].into()
                })]
            );
        }
        if expected == 1 {
            assert!(
                !serde_json::to_string(&copied)
                    .unwrap()
                    .contains("new-result")
            );
        }
        drop(store);
        let reopened = store_for(
            &catalog,
            &destination.session_id,
            &destination.conversation_id,
        );
        let mut before = None;
        let mut observed = 0;
        loop {
            let page = reopened.load_transcript_page(before, 1).unwrap();
            for entry in page.entries {
                for tool in entry.tool_calls {
                    let canonical_result = results
                        .iter()
                        .find(|result| {
                            result.occurrence.assistant_message_id == tool.message_id
                                && result.occurrence.block_index == tool.block_index
                        })
                        .expect("indexed result has its exact canonical owner after reopen");
                    assert_eq!(tool.result.as_ref(), Some(&canonical_result.result));
                    assert_eq!(tool.call.id, ToolCallId::new("call-1"));
                    observed += 1;
                }
            }
            before = page.next_cursor;
            if before.is_none() {
                break;
            }
        }
        assert_eq!(observed, expected);
    }
    catalog
        .publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: revision,
            },
        )
        .unwrap();
    catalog
        .publish_node(
            &source_session,
            &tree,
            source_node.clone(),
            SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node,
                source_surface_revision: revision,
                source_message: MessageId::new("after-B"),
                source_locator: fixture_source_locator(&source_store, "after-B"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .unwrap();
}

#[test]
fn clone_uses_exact_revision_and_isolates_execution_identity_domains() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let selected_revision = source_store.load_head().expect("source head").revision;
    let source_snapshot = lineage_at(&source_store, &source_conversation, selected_revision);

    let prepared = catalog
        .prepare_clone_session(&state(), &source_snapshot)
        .expect("prepare exact clone");
    let destination_session = prepared.session_id.clone();
    let destination_conversation = prepared.conversation_id.clone();
    let destination_store = store_for(&catalog, &destination_session, &destination_conversation);
    assert_eq!(
        destination_store
            .load_pending_unresolved_output_stream_id()
            .expect("clone carryover state"),
        None,
        "clone destinations never inherit execution-recovery carryover"
    );
    let cloned_before_source_mutation = destination_store
        .load_canonical()
        .expect("clone canonical history");

    source_store
        .append_canonical(&user("source-late", "later source work"))
        .expect("mutate source after selected revision");
    assert_eq!(
        destination_store
            .load_canonical()
            .expect("clone remains independent"),
        cloned_before_source_mutation
    );

    let assistant = cloned_before_source_mutation
        .iter()
        .find_map(|message| match message {
            MessageBlock::Assistant(message) => Some(message),
            _ => None,
        })
        .expect("cloned assistant");
    let call = assistant
        .content
        .iter()
        .find_map(|content| match content {
            AssistantContentBlock::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("cloned tool call");
    assert_ne!(assistant.id, MessageId::new("source-assistant"));
    assert_eq!(call.id, ToolCallId::new("source-call"));
    let result = cloned_before_source_mutation
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(message) => Some(message),
            _ => None,
        })
        .expect("cloned tool result");
    assert_eq!(result.tool_call_id, call.id);

    assert!(
        destination_store
            .read_events(None, 64)
            .expect("destination events")
            .events
            .is_empty()
    );
    assert!(
        destination_store
            .read_request_snapshots(None, 64)
            .expect("destination request snapshots")
            .snapshots
            .is_empty()
    );

    let destination = catalog
        .publish_session(
            &prepared,
            SessionNodeOrigin::Clone {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: selected_revision,
            },
        )
        .expect("publish clone");
    assert_eq!(destination.id, destination_session);
    assert_eq!(destination.active_conversation_id, destination_conversation);
    assert_ne!(source_conversation, destination.active_conversation_id);
    assert_eq!(
        catalog
            .summary(&destination_session)
            .expect("clone summary")
            .preview
            .as_deref(),
        Some("A"),
        "the clone is born with the projection derived from its frozen seed, \
             without opening its store"
    );
}

#[test]
fn clone_preserves_todo_state_but_starts_a_fresh_execution_epoch() {
    let (_directory, catalog, _config) = open_catalog();
    let history = vec![
        user("source-user-a", "A"),
        todo_call("source-todo-call", "call-todo"),
        todo_result("source-todo-result", "call-todo", "Write the parser"),
    ];
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let expected_todo = todo_list_of(&source_store);
    let status_id = MessageId::new("source-todo-status");
    let status = todo_status(status_id.as_str());
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "source-todo-fingerprint".to_owned(),
    };
    let mut snapshot = todo_status_start_snapshot(
        source_store
            .load_head()
            .expect("source head")
            .revision
            .next(),
        status_id.clone(),
        emission,
    );
    freeze_session_test_input(&source_store, &mut snapshot, std::slice::from_ref(&status));
    source_store
        .commit_model_turn_start(
            &[status],
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        )
        .expect("source status start");
    assert_eq!(source_store.current_logical_step_progress().unwrap(), 1);
    assert!(
        source_store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .is_some()
    );

    let source_revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, source_revision);
    let prepared = catalog
        .prepare_clone_session(&state(), &source)
        .expect("prepare clone");
    let destination_store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
    assert_eq!(
        destination_store
            .load_pending_unresolved_output_stream_id()
            .expect("clone carryover state"),
        None,
        "clone destinations never inherit execution-recovery carryover"
    );

    assert_eq!(todo_list_of(&destination_store), expected_todo);
    assert_eq!(
        destination_store.current_logical_step_progress().unwrap(),
        0
    );
    assert!(
        destination_store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .is_none()
    );
    assert!(
        destination_store
            .read_events(None, 64)
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        destination_store
            .read_request_snapshots(None, 64)
            .unwrap()
            .snapshots
            .is_empty()
    );
}

#[test]
fn failed_clone_and_fork_publication_is_not_visible() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);

    let clone = catalog
        .prepare_clone_session(&state(), &source)
        .expect("prepare clone");
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: revision,
            },
        ),
        Err(SessionError::CatalogCommit {
            error: CatalogCommitError::NotCommitted { .. }
        })
    ));
    assert_eq!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("visible Session page")
            .sessions
            .len(),
        1,
        "failed clone publication does not expose a half-created Session"
    );

    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare fork");
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.publish_session(
            &fork,
            SessionNodeOrigin::Fork {
                source_session,
                source_node,
                source_surface_revision: revision,
                source_message: MessageId::new("source-user-c"),
                source_locator: fixture_source_locator(&source_store, "source-user-c"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        ),
        Err(SessionError::CatalogCommit {
            error: CatalogCommitError::NotCommitted { .. }
        })
    ));
    assert_eq!(
        catalog
            .snapshot(&first_session(&catalog))
            .expect("source remains active")
            .node_count,
        1,
        "failed fork publication does not expose a half-created node"
    );
}

#[test]
fn clone_and_fork_use_retained_surface_revision_after_real_compaction() {
    let (_directory, catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let retained_revision = source_store.load_head().expect("source head").revision;
    source_store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &source_store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &source_store,
                    "compaction-summary",
                    1,
                    "compacted A",
                ),
                None,
            ),
        )
        .expect("real compaction commit");
    let retained = lineage_at(&source_store, &source_conversation, retained_revision);
    assert_eq!(retained.messages.len(), history.len());

    let prepared_clone = catalog
        .prepare_clone_session(&state(), &retained)
        .expect("clone retained revision");
    assert_eq!(
        prepared_clone.display_preview.as_deref(),
        Some("A"),
        "the retained pre-compaction seed still opens with the first user message"
    );
    let clone_store = store_for(
        &catalog,
        &prepared_clone.session_id,
        &prepared_clone.conversation_id,
    );
    assert_eq!(
        clone_store.load_canonical().expect("clone history").len(),
        history.len(),
        "clone reads the retained pre-compaction revision rather than the compacted head"
    );

    let (prepared_fork, editor_content) = catalog
        .prepare_fork_session(
            &state(),
            &retained,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("fork retained revision");
    assert_eq!(editor_content, vec![text("C")]);
    assert_eq!(
        prepared_fork.display_preview.as_deref(),
        Some("A"),
        "the fork's retained prefix opens with the same first user message"
    );
    let fork_store = store_for(
        &catalog,
        &prepared_fork.session_id,
        &prepared_fork.conversation_id,
    );
    assert_eq!(
        fork_store
            .load_pending_unresolved_output_stream_id()
            .expect("fork carryover state"),
        None,
        "fork destinations never inherit execution-recovery carryover"
    );
    assert_eq!(
        fork_store.load_canonical().expect("fork history").len(),
        history.len() - 1,
        "fork stops immediately before the selected retained user boundary"
    );

    // Compaction changes typed model input but never rewrites the canonical append
    // log: a seed taken from after the compaction still opens with the
    // lineage's first ordinary user append, so the projection subject is
    // stable across compaction.
    let compacted_revision = source_store.load_head().expect("compacted head").revision;
    let compacted = lineage_at(&source_store, &source_conversation, compacted_revision);
    let post_compaction_clone = catalog
        .prepare_clone_session(&state(), &compacted)
        .expect("clone compacted head");
    assert_eq!(
        post_compaction_clone.display_preview.as_deref(),
        Some("A"),
        "the retained operation history still opens with the first user message"
    );
}

#[test]
fn fork_seeds_before_user_and_returns_uncommitted_prompt() {
    let (_directory, catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);

    let mut current_session_intent = state();
    current_session_intent.model.as_mut().unwrap().model =
        serde_json::from_value(serde_json::json!("provider/current-session-intent"))
            .expect("current model");
    let (prepared, editor_content) = catalog
        .prepare_fork_session(
            &current_session_intent,
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare fork");
    assert_eq!(
        prepared.state, current_session_intent,
        "lineage history selection cannot restore old node-local control state"
    );
    assert_eq!(editor_content, vec![text("C")]);
    let destination_store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
    let prefix = destination_store
        .load_canonical()
        .expect("fork canonical prefix");
    assert_eq!(prefix.len(), 3);
    assert!(prefix.iter().all(|message| {
        !matches!(message, MessageBlock::User(user) if user.id == MessageId::new("source-user-c"))
    }));

    let source_after_prepare = source
        .messages
        .iter()
        .map(crate::conversation::message_id_of)
        .collect::<Vec<_>>();
    assert_eq!(source_after_prepare.len(), 4);
    assert_eq!(
        source_store
            .load_canonical()
            .expect("source remains untouched")
            .len(),
        4
    );
}

#[test]
fn fork_preserves_earlier_status_and_excludes_selected_turn_status() {
    let (_directory, catalog, _config) = open_catalog();
    let history = vec![
        user("source-user-a", "A"),
        status("status-before", "Status1"),
        user("source-user-c", "C"),
        status("status-after", "Status2"),
    ];
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);

    let (prepared, editor_content) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare fork");
    assert_eq!(editor_content, vec![text("C")]);
    let prefix = store_for(&catalog, &prepared.session_id, &prepared.conversation_id)
        .load_canonical()
        .expect("fork prefix");
    assert_eq!(prefix.len(), 2);
    assert!(matches!(
        &prefix[1],
        MessageBlock::User(message)
            if matches!(
                &message.kind,
                InboundKind::Context(ContextKind::AgentStatus(_))
            )
                && message.content == vec![text("Status1")]
    ));
    assert!(prefix.iter().all(|message| {
        !matches!(message, MessageBlock::User(message)
            if message.content == vec![text("C")] || message.content == vec![text("Status2")])
    }));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tree_branch_is_a_distinct_linear_node_and_failed_publication_is_invisible() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);

    let before_failed = catalog.snapshot(&source_session).expect("source snapshot");
    let failed = catalog
        .prepare_session(&state(), &[])
        .expect("prepare private destination");
    std::fs::remove_file(&failed.database_path).expect("remove private seed");
    assert!(matches!(
        catalog.publish_session(&failed, SessionNodeOrigin::New),
        Err(SessionError::Catalog { .. })
    ));
    assert_eq!(
        catalog
            .snapshot(&source_session)
            .expect("after failed publication"),
        before_failed
    );

    let (prepared, _editor) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare tree node");
    let branch_conversation = prepared.conversation_id.clone();
    let snapshot = catalog
        .publish_node(
            &source_session,
            &prepared,
            source_node.clone(),
            SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node,
                source_surface_revision: revision,
                source_message: MessageId::new("source-user-c"),
                source_locator: fixture_source_locator(&source_store, "source-user-c"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .expect("publish tree node");
    assert_eq!(snapshot.node_count, 2);
    assert_eq!(snapshot.active_conversation_id, branch_conversation);
    let (attached_node, _) = catalog
        .conversation_lineage(&source_session, &source_conversation)
        .expect("attached source survives a different default node");
    assert_eq!(attached_node.conversation_id, source_conversation);
    assert_ne!(attached_node.id, snapshot.active_node);
    // Explicit historical node routing reads the graph without moving
    // the durable default node.
    let (selected, _) = catalog
        .lineage(&source_session, Some(&attached_node.id))
        .expect("explicit historical node routing");
    assert_eq!(selected.id, attached_node.id);
    assert_eq!(
        catalog.snapshot(&source_session).unwrap().active_node,
        snapshot.active_node
    );

    let nodes = catalog
        .node_page(&source_session, 0, SESSION_TREE_PAGE_LIMIT)
        .expect("tree node page")
        .nodes;
    assert_eq!(snapshot.active_node, nodes[1].id);
    assert_ne!(nodes[0].conversation_id, branch_conversation);
    assert!(
        nodes
            .iter()
            .all(|node| node.conversation_id != source_conversation
                || node.origin == SessionNodeOrigin::New)
    );
    assert_eq!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("list page")
            .sessions
            .len(),
        1,
        "tree branch stays in one Session"
    );

    // Session and node ordinals are independent domains. A new Session
    // after a tree branch must not reuse the branch's globally unique
    // SessionNodeId.
    let new_session = catalog
        .prepare_session(&state(), &[])
        .expect("prepare new session after tree branch");
    assert_ne!(new_session.node_id, snapshot.active_node);
    catalog
        .publish_session(&new_session, SessionNodeOrigin::New)
        .expect("publish new session after tree branch");
    assert_eq!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("list page")
            .sessions
            .len(),
        2,
        "the new Session is independently visible"
    );
    assert_eq!(
        catalog.persisted_session_ids().len(),
        2,
        "both Sessions remain durable catalog state"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn tree_nodes_switch_between_independent_lineages_without_rewind() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);

    let (prepared_a, _) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("source-user-a"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare branch A");
    catalog
        .publish_node(
            &source_session,
            &prepared_a,
            source_node.clone(),
            SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: revision,
                source_message: MessageId::new("source-user-a"),
                source_locator: fixture_source_locator(&source_store, "source-user-a"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .expect("publish branch A");
    let (prepared_b, _) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare branch B");
    catalog
        .publish_node(
            &source_session,
            &prepared_b,
            source_node.clone(),
            SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: revision,
                source_message: MessageId::new("source-user-c"),
                source_locator: fixture_source_locator(&source_store, "source-user-c"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .expect("publish branch B");

    let nodes = catalog
        .node_page(&source_session, 0, SESSION_TREE_PAGE_LIMIT)
        .expect("all branch nodes")
        .nodes;
    assert_eq!(nodes.len(), 3);
    let branch_a = &nodes[1];
    let branch_b = &nodes[2];
    assert_ne!(branch_a.conversation_id, branch_b.conversation_id);
    let destination_store_a = store_for(&catalog, &source_session, &branch_a.conversation_id);
    let destination_store_b = store_for(&catalog, &source_session, &branch_b.conversation_id);
    destination_store_a
        .append_canonical(&user("branch-a-late", "A later"))
        .expect("mutate branch A");
    destination_store_b
        .append_canonical(&user("branch-b-late", "B later"))
        .expect("mutate branch B");

    let selected_a = catalog
        .set_current_node(&source_session, Some(&branch_a.id))
        .expect("select branch A");
    assert_eq!(selected_a.active_conversation_id, branch_a.conversation_id);
    let selected_b = catalog
        .set_current_node(&source_session, Some(&branch_b.id))
        .expect("select branch B");
    assert_eq!(selected_b.active_conversation_id, branch_b.conversation_id);
    let selected_a_again = catalog
        .set_current_node(&source_session, Some(&branch_a.id))
        .expect("select branch A again");
    assert_eq!(
        selected_a_again.active_conversation_id,
        branch_a.conversation_id
    );

    let ids_a = destination_store_a
        .load_canonical()
        .expect("branch A history")
        .into_iter()
        .map(|message| crate::conversation::message_id_of(&message))
        .collect::<BTreeSet<_>>();
    let ids_b = destination_store_b
        .load_canonical()
        .expect("branch B history")
        .into_iter()
        .map(|message| crate::conversation::message_id_of(&message))
        .collect::<BTreeSet<_>>();
    assert!(ids_a.contains(&MessageId::new("branch-a-late")));
    assert!(!ids_a.contains(&MessageId::new("branch-b-late")));
    assert!(ids_b.contains(&MessageId::new("branch-b-late")));
    assert!(!ids_b.contains(&MessageId::new("branch-a-late")));
}

#[test]
fn surface_revision_is_a_linear_history_boundary() {
    let (_directory, catalog, _config) = open_catalog();
    let (conversation, session, _node) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &session, &conversation);
    let first = store
        .load_surface_snapshot(SurfaceRevision::new(2))
        .expect("historical revision");
    let head = store.load_head().expect("head");
    assert_eq!(first.len(), 2);
    assert!(head.revision > SurfaceRevision::new(2));
}
