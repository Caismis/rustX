//! Copies keep their meaning across the compaction that retires their facts,
//! and branch where their source branches.

use super::*;

/// A `todo` result committed by the source, exactly as the native tool
/// publishes one: the complete post-mutation list as the result's
/// structured content.
pub(super) fn todo_result(id: &str, call: &str, subject: &str) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            crate::runtime::identity::MessageId::new("source-todo-call"),
            crate::message::types::ContentBlockIndex::new(0),
        ),
        id: MessageId::new(id),
        tool_call_id: ToolCallId::new(call),
        tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
        result: ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: vec![crate::tools::types::ToolResultContent::Json {
                value: serde_json::json!({
                    "tasks": [{
                        "id": 1,
                        "subject": subject,
                        "status": "pending",
                        "blocked_by": [],
                    }],
                    "next_id": 2,
                }),
            }],
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    })
}

/// The assistant turn that called `todo`.
pub(super) fn todo_call(id: &str, call: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::ToolCall(ToolCall {
            id: ToolCallId::new(call),
            tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
            name: "todo".to_owned(),
            arguments: serde_json::json!({"action": "create", "subject": "Write the parser"}),
        })],
    })
}

/// What a lineage's conversation-owned task list rebuilds to, read the
/// way a runtime opening that lineage reads it.
pub(super) fn todo_list_of(store: &SqliteConversationStore) -> crate::tools::todo::TodoSnapshot {
    crate::tools::todo::ConversationTodoList::rebuilt(
        store.conversation_id().clone(),
        &store
            .load_canonical()
            .expect("destination canonical history"),
    )
    .expect("the destination rebuilds a usable list")
    .committed()
}

/// A copy of a conversation means the same thing whether or not the
/// source has been compacted since.
///
/// This is the lineage invariant, stated on the first piece of
/// conversation state that is *derived from canonical history* rather
/// than read off the model projection: the task list. Compaction excludes the
/// `todo` result from model input and leaves it canonical, so the source
/// still has its list — and a copy seeded from the projection alone would
/// not. The two clones below bracket exactly that transition, and the
/// assertion is that they agree with each other and with the source.
///
/// The property is deliberately not "the clone inherits the list". It is
/// that *compaction is invisible to lineage semantics*: it changes the
/// context projection, never what copying the conversation means. Any
/// later conversation-owned state derived from canonical history inherits
/// that guarantee from the seed rather than having to restate it.
#[test]
fn a_clone_means_the_same_before_and_after_the_compaction_that_retires_its_facts() {
    let (_directory, catalog, _config) = open_catalog();
    let history = vec![
        user("source-user-a", "A"),
        todo_call("source-todo-call", "call-todo"),
        todo_result("source-todo-result", "call-todo", "Write the parser"),
        user("source-user-c", "C"),
    ];
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let expected = todo_list_of(&source_store);
    assert_eq!(expected.tasks.len(), 1, "the source committed one task");

    // The clone taken while the `todo` result is still model-visible.
    let before = lineage_at(
        &source_store,
        &source_conversation,
        source_store.load_head().expect("source head").revision,
    );
    let clone_before = catalog
        .prepare_clone_session(&state(), &before)
        .expect("clone before compaction");

    // The compaction retires the whole span the result lives in.
    let _head = source_store.load_head().expect("source head");
    source_store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &source_store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &source_store,
                    "source-compaction-summary",
                    3,
                    "earlier work, summarized",
                ),
                None,
            ),
        )
        .expect("real compaction commit");
    let compacted = source_store.load_head().expect("compacted head");
    assert!(
        !compacted
            .active_message_ids
            .contains(&MessageId::new("source-todo-result")),
        "the fact the list is derived from is no longer model-visible"
    );
    assert_eq!(
        todo_list_of(&source_store),
        expected,
        "the source itself still has the list it committed"
    );

    // The clone taken from the compacted head.
    let after = lineage_at(&source_store, &source_conversation, compacted.revision);
    let clone_after = catalog
        .prepare_clone_session(&state(), &after)
        .expect("clone after compaction");

    let before_store = store_for(
        &catalog,
        &clone_before.session_id,
        &clone_before.conversation_id,
    );
    let after_store = store_for(
        &catalog,
        &clone_after.session_id,
        &clone_after.conversation_id,
    );
    assert_eq!(
        todo_list_of(&before_store),
        todo_list_of(&after_store),
        "a compaction between two clones cannot change what cloning means"
    );
    assert_eq!(
        todo_list_of(&after_store),
        expected,
        "the clone carries the conversation state the source has, not the \
             subset its Surface still shows"
    );

    // The inherited fact is inherited as the source holds it: canonical,
    // and not model-visible. A destination that put the retired result
    // back on its Surface would be re-showing context the source already
    // summarized away.
    let after_head = after_store.load_head().expect("clone head");
    assert_eq!(
        after_head.active_message_ids.len(),
        after_store
            .projected_history()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.as_canonical().is_some())
            .count(),
        "the hot cache contains exactly retained Original facts"
    );
    assert!(
        after_store.load_canonical().expect("clone canonical").len()
            > after_head.active_message_ids.len(),
        "and still carries the canonical facts that Surface no longer shows"
    );
}

/// The same invariant on the other lineage constructor: a fork cuts the
/// canonical history at the boundary it selected, and inherits the
/// retired facts below that cut.
#[test]
fn a_fork_inherits_the_canonical_state_of_the_boundary_it_cuts_at() {
    let (_directory, catalog, _config) = open_catalog();
    let history = vec![
        user("source-user-a", "A"),
        todo_call("source-todo-call", "call-todo"),
        todo_result("source-todo-result", "call-todo", "Write the parser"),
        user("source-user-c", "C"),
    ];
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let expected = todo_list_of(&source_store);

    let _head = source_store.load_head().expect("source head");
    source_store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &source_store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &source_store,
                    "source-compaction-summary",
                    3,
                    "earlier work, summarized",
                ),
                None,
            ),
        )
        .expect("real compaction commit");

    let compacted = source_store.load_head().expect("compacted head");
    let source = lineage_at(&source_store, &source_conversation, compacted.revision);
    let (prepared, editor_content) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("fork at the surviving boundary");
    assert_eq!(editor_content, vec![text("C")]);

    let fork_store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
    assert_eq!(
        todo_list_of(&fork_store),
        expected,
        "the fork inherits the conversation state in effect at its boundary"
    );
    assert_eq!(
        fork_store
            .load_head()
            .expect("fork head")
            .active_message_ids
            .len(),
        3,
        "the earlier canonical cut precedes checkpoint activation and retains the eligible original prefix"
    );
}

/// A compaction of the span `[A ..= todo_result]`, committed exactly as
/// the runtime commits one.
fn compact_the_retired_span(store: &SqliteConversationStore) -> SurfaceRevision {
    let _head = store.load_head().expect("source head");
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    store,
                    "source-compaction-summary",
                    3,
                    "earlier work, summarized",
                ),
                None,
            ),
        )
        .expect("real compaction commit");
    store.load_head().expect("compacted head").revision
}

/// What a lineage carries, in a form two different lineages can be
/// compared in: identities are lineage-owned and deliberately differ, so
/// the comparable fact is the content.
pub(super) fn shape(message: &MessageBlock) -> String {
    match message {
        MessageBlock::User(user) => {
            let text = user
                .content
                .iter()
                .map(|block| match block {
                    UserContentBlock::Text(block) => block.text.clone(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("user({:?}, {text})", user.kind)
        }
        MessageBlock::Assistant(assistant) => format!("assistant({})", assistant.content.len()),
        MessageBlock::Tool(tool) => format!("tool({})", tool.tool_id),
    }
}

fn shapes(messages: &[MessageBlock]) -> Vec<String> {
    messages.iter().map(shape).collect()
}

/// What a lineage durably carries, retired facts included.
fn canonical_shapes(store: &SqliteConversationStore) -> Vec<String> {
    shapes(&store.load_canonical().expect("canonical history"))
}

/// What a lineage currently shows the model.
fn surface_shapes(store: &SqliteConversationStore) -> Vec<String> {
    let head = store.load_head().expect("head");
    shapes(
        &store
            .load_surface_snapshot(head.revision)
            .expect("Surface snapshot"),
    )
}

/// The shape of the user message both lineages are branched at.
const SELECTED_BOUNDARY: &str = "user(Message, C)";

/// The branch points a lineage reports for its own current head, in the
/// form two different lineages can be compared in.
fn reported_boundaries(
    store: &SqliteConversationStore,
) -> Vec<(SurfaceRevision, String, MessageId)> {
    let revision = store.load_head().expect("head").revision;
    store
        .load_user_message_boundaries(revision)
        .expect("historical boundaries")
        .into_iter()
        .map(|boundary| {
            (
                boundary.surface_revision,
                shape(&MessageBlock::User(boundary.message.clone())),
                boundary.message.id,
            )
        })
        .collect()
}

/// The clone of a compacted source that carries a `todo` result its
/// Surface no longer shows: the shape every test below branches.
fn compacted_source_and_its_clone(
    catalog: &SessionCatalog,
) -> (
    SqliteConversationStore,
    ConversationId,
    SqliteConversationStore,
    PreparedLineage,
    crate::tools::todo::TodoSnapshot,
) {
    let history = vec![
        user("source-user-a", "A"),
        todo_call("source-todo-call", "call-todo"),
        todo_result("source-todo-result", "call-todo", "Write the parser"),
        user("source-user-c", "C"),
    ];
    let (source_conversation, source_session, _source_node) = append_history(catalog, &history);
    let source_store = store_for(catalog, &source_session, &source_conversation);
    let expected = todo_list_of(&source_store);
    let compacted = compact_the_retired_span(&source_store);
    let clone = catalog
        .prepare_clone_session(
            &state(),
            &lineage_at(&source_store, &source_conversation, compacted),
        )
        .expect("clone the compacted source");
    let clone_store = store_for(catalog, &clone.session_id, &clone.conversation_id);
    let source_facts = source_store
        .read_lineage_cut(compacted)
        .unwrap()
        .projections;
    let copied_facts = clone_store
        .read_lineage_cut(clone_store.load_head().unwrap().revision)
        .unwrap()
        .projections;
    assert_eq!(copied_facts.len(), source_facts.len());
    for (source, copied) in source_facts.iter().zip(&copied_facts) {
        assert_eq!(copied.activation.origin, source.activation.origin);
        assert!(copied.activation.destination_installation.is_some());
        assert_eq!(
            copied.candidate.summary_metadata,
            source.candidate.summary_metadata
        );
        assert_ne!(
            copied.candidate.checkpoint_id,
            source.candidate.checkpoint_id
        );
        clone_store
            .projection_checkpoint(&copied.candidate.checkpoint_id)
            .unwrap();
    }
    assert!(
        clone_store.read_events(None, 64).unwrap().events.is_empty(),
        "inherited checkpoint installation owns no destination execution fact"
    );
    (
        source_store,
        source_conversation,
        clone_store,
        clone,
        expected,
    )
}

/// Copying a lineage is closed under the lineage operations that follow
/// the copy.
///
/// The two earlier lineage invariants say a copy keeps what the source
/// currently *means* and currently *shows*. Neither is enough on its own,
/// because a copy is not a terminal object: the user forks and branches
/// the copy afterwards, and those operations read the copy's *history*,
/// not its current state.
///
/// A copy must retain canonical boundaries and independent projection
/// activations. Rebuilding only the current projection would lose historical
/// choices needed by subsequent forks, including two activations at one frontier.
///
/// So the invariant is stated on the composition, not on the copy: a
/// fork taken on a copy, at a boundary the copy itself reports, means
/// what the same fork taken on the source means.
#[test]
fn a_copy_branches_where_its_source_branches() {
    let (_directory, catalog, _config) = open_catalog();
    let (source_store, source_conversation, clone_store, clone, expected) =
        compacted_source_and_its_clone(&catalog);

    // The copy reports the source's branch points, at the source's
    // revisions. A copy rebuilt from the final projection reports `C` at
    // the revision it was re-appended in, which is a branch point the
    // source never had.
    let source_boundaries = reported_boundaries(&source_store);
    let clone_boundaries = reported_boundaries(&clone_store);
    let positions = |boundaries: &[(SurfaceRevision, String, MessageId)]| {
        boundaries
            .iter()
            .map(|(revision, shape, _)| (*revision, shape.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        positions(&clone_boundaries),
        positions(&source_boundaries),
        "the copy offers the branch points the source offers, in the same \
             historical positions"
    );

    // Each lineage is forked at its own reported boundary, through the
    // ordinary path: no revision is hand-constructed here.
    let fork = |store: &SqliteConversationStore,
                conversation: &ConversationId,
                boundaries: &[(SurfaceRevision, String, MessageId)]| {
        let (revision, _, message_id) = boundaries
            .iter()
            .find(|(_, shape, _)| shape == SELECTED_BOUNDARY)
            .expect("the selected boundary");
        catalog
            .prepare_fork_session(
                &state(),
                &lineage_at(store, conversation, *revision),
                message_id,
                crate::local_runtime::session::LineageSide::Before,
            )
            .expect("fork at the reported boundary")
    };
    let (source_fork, source_editor) =
        fork(&source_store, &source_conversation, &source_boundaries);
    let (clone_fork, clone_editor) = fork(&clone_store, &clone.conversation_id, &clone_boundaries);
    let source_fork_store = store_for(
        &catalog,
        &source_fork.session_id,
        &source_fork.conversation_id,
    );
    let clone_fork_store = store_for(
        &catalog,
        &clone_fork.session_id,
        &clone_fork.conversation_id,
    );
    assert_eq!(
        clone_fork_store
            .load_pending_unresolved_output_stream_id()
            .expect("fork carryover state"),
        None,
        "fork destinations never inherit execution-recovery carryover"
    );

    assert_eq!(clone_editor, vec![text("C")]);
    assert_eq!(
        clone_editor, source_editor,
        "the same boundary is restored to the editor from either lineage"
    );
    assert_eq!(
        canonical_shapes(&clone_fork_store),
        canonical_shapes(&source_fork_store),
        "a fork of the copy inherits the canonical prefix a fork of the source \
             inherits"
    );
    assert_eq!(
        surface_shapes(&clone_fork_store),
        surface_shapes(&source_fork_store),
        "and shows what that fork shows"
    );

    // The specific failure the composition exists to catch: the boundary
    // is what the fork cuts *before*, so it can never also be a canonical
    // fact the fork inherits — otherwise the same message is at once
    // committed history and an uncommitted prompt.
    assert!(
        !canonical_shapes(&clone_fork_store)
            .iter()
            .any(|shape| shape == SELECTED_BOUNDARY),
        "a fork cannot inherit the very message it hands back to the editor"
    );
    assert_eq!(
        todo_list_of(&clone_fork_store),
        expected,
        "and still inherits the conversation state in effect at that boundary"
    );
    assert_eq!(
        todo_list_of(&clone_fork_store),
        todo_list_of(&source_fork_store)
    );
}

/// A tree node shares the fork's boundary semantics, so it shares the
/// closure property: branching a copy at a copied boundary means what
/// branching the source at that boundary means.
#[test]
fn a_tree_node_of_a_copy_branches_where_its_source_branches() {
    let (_directory, mut catalog, _config) = open_catalog();
    let (source_store, source_conversation, clone_store, clone, expected) =
        compacted_source_and_its_clone(&catalog);

    let (source_session, _, _) = catalog
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("source lineage");
    catalog
        .publish_session(&clone, SessionNodeOrigin::New)
        .expect("publish independent clone before branching");
    let branch =
        |store: &SqliteConversationStore, session_id: &SessionId, conversation: &ConversationId| {
            let (revision, _, message_id) = reported_boundaries(store)
                .into_iter()
                .find(|(_, shape, _)| shape == SELECTED_BOUNDARY)
                .expect("the selected boundary");
            catalog
                .prepare_tree_node(
                    session_id,
                    &state(),
                    &lineage_at(store, conversation, revision),
                    &message_id,
                    crate::local_runtime::session::LineageSide::Before,
                )
                .expect("branch at the reported boundary")
        };
    let (source_node, source_editor) = branch(&source_store, &source_session, &source_conversation);
    let (clone_node, clone_editor) =
        branch(&clone_store, &clone.session_id, &clone.conversation_id);
    assert_eq!(clone_editor, vec![text("C")]);
    assert_eq!(clone_editor, source_editor);

    let source_node_store = store_for(
        &catalog,
        &source_node.session_id,
        &source_node.conversation_id,
    );
    let clone_node_store = store_for(
        &catalog,
        &clone_node.session_id,
        &clone_node.conversation_id,
    );
    assert_eq!(
        clone_node_store
            .load_pending_unresolved_output_stream_id()
            .expect("tree carryover state"),
        None,
        "tree destinations never inherit execution-recovery carryover"
    );
    assert_eq!(
        canonical_shapes(&clone_node_store),
        canonical_shapes(&source_node_store),
        "a branch of the copy inherits the canonical prefix a branch of the \
             source inherits"
    );
    assert!(
        !canonical_shapes(&clone_node_store)
            .iter()
            .any(|shape| shape == SELECTED_BOUNDARY),
        "a branch cannot inherit the very message it hands back to the editor"
    );
    assert_eq!(
        todo_list_of(&clone_node_store),
        expected,
        "and still inherits the conversation state in effect at that boundary"
    );
}
