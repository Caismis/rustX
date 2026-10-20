// A tiny Child delivery has no dependency on a complete historical Host.
use rustx::durable::{CanonicalStore, JournalStore, ObservationStore};
use crate::runtime_client::content::CanonicalContentLocator;

fn giant_selected_delivery(
    child: &ChildDelivery,
    file: SessionFileReference,
) -> (CanonicalContentLocator, MessageBlock) {
    let proposal = MessageId::new("selected-child-proposal");
    let call = ToolCall {
        id: ToolCallId::new("selected-child-call"),
        tool_id: ToolId::new("tool-present"),
        name: "present".into(),
        arguments: serde_json::json!({}),
    };
    child
        .store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: proposal.clone(),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        }))
        .unwrap();
    let original = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("giant-child-original-id🙂".repeat(80_000)),
        occurrence: ToolCallOccurrenceRef::new(proposal, ContentBlockIndex::new(0)),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![crate::tools::types::ToolResultContent::Text(
                crate::message::content::TextBlock {
                    text: "giant-child-original-result🙂".repeat(80_000),
                },
            )],
            duration_ms: u64::MAX,
            exit_code: None,
            artifacts: vec![],
            deliveries: vec![file],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    });
    assert!(original.id().as_str().len() > 1_048_576);
    assert!(serde_json::to_vec(&original).unwrap().len() > 2 * 1_048_576);
    let receipt = child.store.append_canonical(&original).unwrap();
    (
        CanonicalContentLocator::ByLedgerPosition {
            position: receipt.observation.last_position,
        },
        original,
    )
}

fn assert_selected_child_span(
    host: &crate::runtime_client::RuntimeClientHost,
    child: &ChildDelivery,
    locator: &CanonicalContentLocator,
    file: &SessionFileReference,
    foreign_message: &MessageId,
) {
    crate::durable::SqliteConversationStore::trace_read_work();
    let opens = crate::durable::sqlite::conversation_store_opens_on_this_thread();
    let selected = host
        .inner
        .agent_file_reference(&child.agent, locator, 0)
        .unwrap();
    assert_eq!(
        crate::durable::sqlite::conversation_store_opens_on_this_thread() - opens,
        1,
        "only Registry's authorized original Store is opened"
    );
    assert_eq!(
        crate::durable::SqliteConversationStore::trace_read_work(),
        (0, 0)
    );
    let (rows, bytes) = crate::durable::SqliteConversationStore::content_read_work();
    assert!(rows > 0 && rows <= 2 && bytes > 0 && bytes <= 16_384);
    assert_eq!(&selected, file);
    assert_eq!(
        crate::tools::session_files::read_authorized(&child.workspace, &selected, || Ok(()))
            .unwrap(),
        DELIVERED
    );
    let denied = crate::tools::session_files::read_authorized(&child.workspace, &selected, || {
        Err(std::io::ErrorKind::PermissionDenied.into())
    })
    .unwrap_err();
    assert_eq!(
        crate::tools::session_files::read_failure(&denied),
        SessionFileReadFailure::Unauthorized
    );
    assert!(matches!(
        host.inner
            .agent_file_reference(&AgentId::new("foreign-selected-child"), locator, 0),
        Err(crate::runtime_client::RuntimeClientError::UnknownAgent { .. })
    ));
    assert!(
        host.inner
            .agent_file_reference(&child.agent, locator, 1)
            .is_err()
    );
    assert!(
        host.inner
            .agent_file_reference(
                &child.agent,
                &CanonicalContentLocator::ByLedgerPosition { position: 0 },
                0
            )
            .is_err()
    );
    assert!(matches!(
        host.inner.agent_file_reference(
            &child.agent,
            &CanonicalContentLocator::ByMessageId {
                message_id: foreign_message.clone()
            },
            0
        ),
        Err(crate::runtime_client::RuntimeClientError::InvalidState { .. })
    ));
}

async fn selected_child_delivery_with_trace_fault() {
    let child = ChildDelivery::with_delivery(false).await;
    let file = crate::tools::session_files::declare(
        &child.workspace,
        child.store.conversation_id(),
        "child-report.md",
        None,
    )
    .unwrap();
    let mut foreign_file = file.clone();
    foreign_file.scope.conversation_id = crate::runtime::identity::ConversationId::generate();
    // Commit the ordinary semantic negative before the Source-only giant row.
    // Its detached Tool event authorizer intentionally uses the bounded native Surface.
    let foreign_message = commit_delivery(&child.store, "foreign-selected-scope", foreign_file);
    let (locator, original) = giant_selected_delivery(&child, file.clone());
    let host = child
        .fixture
        .manager
        .configuration_host(&child.fixture.sessions[0].id)
        .unwrap();
    let runtime = child
        .fixture
        .manager
        .configuration_runtime(&child.fixture.sessions[0].id)
        .unwrap();
    let registry = runtime.subagent_registry().unwrap();
    let (agent, activation) = registry
        .agent_snapshot_with_activation(&child.agent)
        .unwrap();
    assert_eq!(agent.conversation_id, *child.store.conversation_id());
    let cut = child.store.observation_read_cut().unwrap();
    let original_events = child.store.read_events(Some(0), 64).unwrap().events;
    assert_eq!(
        original_events.last().map(|event| event.sequence),
        Some(cut.read.journal)
    );
    let journal = serde_json::to_vec(&original_events).unwrap();
    let native = child.evidence();
    let path = registry
        .agent_artifact_root(&child.agent)
        .unwrap()
        .join("conversation.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (sequence, header): (u64, Vec<u8>) = connection.query_row("SELECT sequence,observation_trace_fact FROM events WHERE observation_kind='attempt_started' ORDER BY sequence LIMIT 1", [], |row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    std::str::from_utf8(&header).unwrap();
    serde_json::from_slice::<serde_json::Value>(&header).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET observation_trace_fact=NULL WHERE sequence=?1",
                [sequence]
            )
            .unwrap(),
        1
    );
    let broken = registry.transcript_store(&activation.subagent_id).unwrap();
    assert!(
        crate::runtime_client::RuntimeClientHost::new_durable(std::sync::Arc::new(broken), None)
            .is_err(),
        "the unrelated own Trace header really prevents whole historical Host construction"
    );

    assert_selected_child_span(&host, &child, &locator, &file, &foreign_message);
    assert_eq!(
        registry
            .agent_snapshot_with_activation(&child.agent)
            .unwrap()
            .0
            .latest_activation,
        agent.latest_activation
    );
    assert_eq!(child.store.observation_read_cut().unwrap(), cut);
    assert_eq!(
        serde_json::to_vec(&child.store.read_events(Some(0), 64).unwrap().events).unwrap(),
        journal
    );
    assert_eq!(
        child
            .store
            .load_messages(std::slice::from_ref(original.id()))
            .unwrap(),
        vec![original]
    );
    assert_eq!(
        child.evidence(),
        native,
        "selection does not activate, execute, request models or commit native facts"
    );
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET observation_trace_fact=?1 WHERE sequence=?2",
                rusqlite::params![header, sequence]
            )
            .unwrap(),
        1
    );
    drop(connection);
    child.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t12_t14_t19_child_delivery_selects_only_its_native_span_with_unrelated_trace_fault() {
    Box::pin(bounded(selected_child_delivery_with_trace_fault())).await;
}
