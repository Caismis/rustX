//! Event journal regressions of the SQLite store.

use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore, JournalStore, ObservationStore, ProjectionStore};
use crate::durable::{ConversationStore, ConversationStoreError, InboundDraft};
use crate::events::types::RuntimeEvent;
use crate::events::types::SubagentTerminalState;
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use crate::message::types::{AssistantMessageBlock, ToolMessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, MessageId, RequestId, ToolCallId, ToolId, TurnId,
};
use crate::runtime::types::CancellationReason;
use crate::tools::types::{
    ToolCall, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
};
use chrono::TimeZone;
use chrono::Utc;
use rusqlite::{TransactionBehavior, params};
use std::sync::{Arc, Mutex};

#[test]
fn background_terminal_publication_is_idempotent_and_terminal_unique() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let execution_id =
        crate::runtime::identity::ToolExecutionId::new("exec_63093cff-69b8-7efb-82db-20a75f993612");
    let message_id = MessageId::new("background-notification-1");
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
    let notification = UserMessageBlock {
        id: message_id.clone(),
        content: draft("background").content,
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    };
    let event = envelope(
        &conversation_id,
        "background-event-1",
        None,
        RuntimeEvent::BackgroundTerminalPublished {
            execution_id: execution_id.clone(),
            message_id: message_id.clone(),
            state: crate::events::types::BackgroundTerminalState::Succeeded,
        },
    );
    let draft_for_store = || InboundDraft {
        message_id: Some(message_id.clone()),
        source: notification.source.clone(),
        kind: notification.kind.clone(),
        content: notification.content.clone(),
        timestamp,
        correlation: Some("background:execution-1".to_owned()),
    };
    let (accepted, persisted) = store
        .accept_inbound_with_event(draft_for_store(), event.clone())
        .unwrap();
    assert!(!accepted.retried);
    assert_eq!(persisted.sequence, 1);

    let (retried, persisted_retry) = store
        .accept_inbound_with_event(draft_for_store(), event)
        .unwrap();
    assert!(retried.retried);
    assert_eq!(persisted_retry.sequence, persisted.sequence);

    let second_message_id = MessageId::new("background-notification-2");
    let second_event = envelope(
        &conversation_id,
        "background-event-2",
        None,
        RuntimeEvent::BackgroundTerminalPublished {
            execution_id,
            message_id: second_message_id.clone(),
            state: crate::events::types::BackgroundTerminalState::Succeeded,
        },
    );
    let second_draft = InboundDraft {
        message_id: Some(second_message_id),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: "second".to_owned(),
        })],
        timestamp,
        correlation: Some("background:execution-2".to_owned()),
    };
    assert!(matches!(
        store.accept_inbound_with_event(second_draft, second_event),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
}

#[test]
#[allow(clippy::too_many_lines)] // Full admission, private capture, reopen and tamper boundaries.
fn agent_authorities_reopen_from_private_capture_without_journal_secrets() {
    use crate::credentials::{CredentialSnapshot, SourceCredentials};
    use crate::model::catalog::{CredentialSource, ResolvedCredential};
    use crate::runtime::identity::SubagentId;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::generate();
    let child_id = ConversationId::generate();
    let agent_id = AgentId::new("private-agent");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let (event, mut authority) =
        crate::local_runtime::session::tests::deletion_tests::admit_agent(envelope(
            &id,
            crate::runtime::subagent::subagent_ownership_event_id(&SubagentId::for_conversation(
                &id, 1,
            ))
            .as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                parent_agent_id: AgentId::new("parent-agent"),
                subagent_id: SubagentId::for_conversation(&id, 1),
                child_agent_id: agent_id.clone(),
                child_conversation_id: child_id,
                admitted_authority: None,
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: ToolCallId::new("private-call"),
                },
                agent: "explore".into(),
                definition_digest: "sha256:definition".into(),
                profile_digest: "sha256:profile".into(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: directory.path().to_path_buf(),
            },
        ));
    authority.resolved.model.primary.binding.credential =
        CredentialSource::Literal("private-literal".into());
    authority.resolved.model.primary.binding.resolved_credential = None;
    let mut summary = authority.resolved.model.primary.clone();
    summary.binding.credential = CredentialSource::Environment("AUTHORED_PROVIDER".into());
    summary.binding.resolved_credential = Some(ResolvedCredential::new("captured-provider"));
    authority.resolved.model.summary =
        crate::model::frozen::FrozenSummaryModel::Explicit(Box::new(summary));
    let mut source: SourceCredentials = serde_json::from_value(serde_json::json!({
        "environment": {"TOKEN":"$AUTHORED_SOURCE"}, "headers":{"Authorization":"$AUTHORED_SOURCE"}
    }))
    .unwrap();
    source.capture(CredentialSnapshot::new([(
        "AUTHORED_SOURCE".into(),
        "captured-source".into(),
    )]));
    authority.resolved.materialization.sources.insert(
        crate::capabilities::ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new(
            "private-source",
        )),
        crate::tools::mcp::McpServerBinding {
            credentials: source,
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: "test-server".into(),
                args: vec!["private-mcp-arg".into()],
                cwd: None,
                environment: [("TOKEN".into(), "private-mcp-env".into())].into(),
            },
            policy: crate::tools::ToolInvocationPolicy::default(),
        },
    );
    authority
        .resolved
        .environment
        .push(("TOOL_TOKEN".into(), "private-tool-env".into()));
    authority.resolved.instructions = "private-instructions".into();
    authority.resolved.materialization.sources.insert(
        crate::capabilities::ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new("private-http")),
        crate::tools::mcp::McpServerBinding {
            credentials: SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::StreamableHttp {
                endpoint: "https://private-url-user:private-url-password@example.test/api?token=private-url-query".into(),
                headers: [("Authorization".into(), "private-http-auth".into()), ("X-Private".into(), "private-http-header".into())].into(),
            },
            policy: crate::tools::ToolInvocationPolicy::default(),
        },
    );
    let canaries = [
        "private-literal",
        "captured-provider",
        "captured-source",
        "private-mcp-arg",
        "private-mcp-env",
        "private-tool-env",
        "private-instructions",
        "private-url-user",
        "private-url-password",
        "private-url-query",
        "private-http-auth",
        "private-http-header",
    ];
    let public = serde_json::to_string(&event).unwrap();
    for secret in canaries {
        assert!(!public.contains(secret), "public DTO contains {secret}");
    }
    assert!(
        store.append_event(event.clone()).is_err(),
        "public event alone cannot admit private authority"
    );
    let mut invalid = event.clone();
    invalid.schema_version = 0;
    assert!(store.append_agent_admission(invalid, &authority).is_err());
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM agent_authorities", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        0,
        "failed public event commit rolls back the private authority row"
    );
    assert!(store.read_events(None, 10).unwrap().events.is_empty());
    store
        .append_agent_admission(event.clone(), &authority)
        .unwrap();
    assert!(
        store.append_agent_admission(event, &authority).is_err(),
        "duplicate admission rolls back"
    );
    let journal: String = store
        .lock()
        .unwrap()
        .query_row("SELECT event_json FROM events", [], |row| row.get(0))
        .unwrap();
    for secret in canaries {
        assert!(
            !journal.contains(secret),
            "Event Journal contains private material"
        );
    }
    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    let restored = reopened.load_agent_authority(&agent_id).unwrap();
    assert_eq!(
        restored.resolved.environment,
        authority.resolved.environment
    );
    assert_eq!(
        restored.resolved.instructions,
        authority.resolved.instructions
    );
    for (name, binding) in &authority.resolved.materialization.sources {
        assert_eq!(
            restored.resolved.materialization.sources[name].transport,
            binding.transport
        );
    }
    let events = reopened.read_events(None, 10).unwrap().events;
    assert!(
        matches!(&events[0].event, RuntimeEvent::SubagentOwnershipCommitted {
        admitted_authority: Some(reference), ..
    } if reference == &agent_id)
    );
    let authority = restored;
    assert_eq!(
        authority
            .resolved
            .model
            .primary
            .binding
            .resolved_credential
            .as_ref()
            .unwrap()
            .expose(),
        "private-literal"
    );
    let crate::model::frozen::FrozenSummaryModel::Explicit(summary) =
        &authority.resolved.model.summary
    else {
        panic!("summary")
    };
    assert_eq!(
        summary
            .binding
            .resolved_credential
            .as_ref()
            .unwrap()
            .expose(),
        "captured-provider"
    );
    for source in authority
        .resolved
        .materialization
        .sources
        .values()
        .filter(|source| {
            matches!(
                source.transport,
                crate::tools::mcp::McpTransportConfig::Stdio { .. }
            )
        })
    {
        let resolved = source.credentials.resolve().unwrap();
        assert_eq!(resolved.environment["TOKEN"].expose(), "captured-source");
        assert_eq!(
            resolved.headers["Authorization"].expose(),
            "captured-source"
        );
    }
    reopened
        .lock()
        .unwrap()
        .execute("DELETE FROM agent_authorities", [])
        .unwrap();
    assert!(
        reopened.load_agent_authority(&agent_id).is_err(),
        "missing private capture must fail closed"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Real compaction, later Tool admission, and client admission.
fn resumed_agent_origin_survives_creation_call_compaction() {
    use crate::runtime::identity::SubagentId;
    use crate::runtime::subagent::AgentActivationOrigin;
    let store = store();
    let conversation = store.conversation_id().clone();
    let agent = AgentId::new("continuable-agent");
    let child = ConversationId::generate();
    let call_message = |message: &str, call: &str, name: &str| {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(message),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new(call),
                tool_id: ToolId::new(name),
                name: name.into(),
                arguments: serde_json::json!({}),
            })],
        })
    };
    store
        .initialize(&[
            call_message("creation", "create-call", "subagent"),
            MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("creation-result"),
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    MessageId::new("creation"),
                    ContentBlockIndex::new(0),
                ),
                tool_call_id: ToolCallId::new("create-call"),
                tool_id: ToolId::new("subagent"),
                result: ToolExecutionResult {
                    deliveries: vec![],
                    status: ToolExecutionStatus::Success,
                    content: vec![],
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            }),
        ])
        .unwrap();
    let admission = |sequence, origin| {
        let activation = SubagentId::for_conversation(&conversation, sequence);
        envelope(
            &conversation,
            crate::runtime::subagent::subagent_ownership_event_id(&activation).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                parent_agent_id: AgentId::new("parent"),
                subagent_id: activation,
                child_agent_id: agent.clone(),
                child_conversation_id: child.clone(),
                admitted_authority: None,
                origin,
                agent: "explore".into(),
                definition_digest: "sha256:definition".into(),
                profile_digest: "sha256:profile".into(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        )
    };
    let (creation, authority) =
        crate::local_runtime::session::tests::deletion_tests::admit_agent(admission(
            1,
            AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("create-call"),
            },
        ));
    store.append_agent_admission(creation, &authority).unwrap();
    let settle = |sequence| {
        let (notice, report, event) = success_publication(
            &store,
            &SubagentId::for_conversation(&conversation, sequence),
            &agent,
            "done",
        );
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .unwrap();
    };
    settle(1);
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "creation-checkpoint",
                    2,
                    "creation retired",
                ),
                None,
            ),
        )
        .unwrap();
    assert!(
        !store
            .load_head()
            .unwrap()
            .active_message_ids
            .contains(&MessageId::new("creation"))
    );
    store
        .append_canonical(&call_message("followup", "message-call", "send_message"))
        .unwrap();
    let resume = |sequence, origin: AgentActivationOrigin| {
        store
            .append_event(crate::runtime::subagent::admission_event(
                &conversation,
                &agent,
                &SubagentId::for_conversation(&conversation, sequence),
                &origin,
                crate::events::types::AgentActivationAdmissionPhase::Reserved,
                Utc::now(),
            ))
            .unwrap();
        store.append_event(admission(sequence, origin)).unwrap();
    };
    resume(
        2,
        AgentActivationOrigin::MessageTool {
            tool_call_id: ToolCallId::new("message-call"),
        },
    );
    settle(2);
    resume(3, AgentActivationOrigin::ClientControl);
    let origins = store
        .read_events(None, 100)
        .unwrap()
        .events
        .into_iter()
        .filter_map(|event| match event.event {
            RuntimeEvent::SubagentOwnershipCommitted {
                child_agent_id,
                child_conversation_id,
                origin,
                ..
            } => {
                assert_eq!(child_agent_id, agent);
                assert_eq!(child_conversation_id, child);
                Some(origin)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        origins,
        vec![
            AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("create-call")
            },
            AgentActivationOrigin::MessageTool {
                tool_call_id: ToolCallId::new("message-call")
            },
            AgentActivationOrigin::ClientControl,
        ]
    );
    assert!(store.load_agent_authority(&agent).is_ok());
}

/// A normal subagent terminal publication commits through
/// `accept_subagent_terminal` — the one durable authority for
/// `SubagentTerminalPublished` (Issue #192). The retry of a notice-free
/// non-success publication is an idempotent no-op, a semantically
/// different retry of the same child conflicts deterministically, and
/// the obsolete generic `accept_inbound_with_event` path rejects the
/// payload so it can never bypass the terminal-publication contract.
#[test]
#[allow(clippy::too_many_lines)]
fn subagent_terminal_publication_is_idempotent_and_terminal_unique() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);

    // A failed terminal: one runtime-authored message, no notice.
    let (report, event) = non_success_publication(
        &store,
        &subagent_id,
        &child_agent_id,
        SubagentTerminalState::Failed,
        "subagent failed: boom",
    );

    // The obsolete generic path can no longer publish a normal subagent
    // terminal fact — for either a success-shaped or a failure-shaped
    // payload.
    assert!(matches!(
        store.accept_inbound_with_event(report.clone(), event.clone()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store.load_pending().unwrap().len(),
        0,
        "the rejected generic transition accepted nothing"
    );

    let (notice, accepted, persisted) = store
        .accept_subagent_terminal(None, report.clone(), event.clone())
        .unwrap();
    assert!(
        notice.is_none(),
        "a non-success terminal carries no successful-terminal notice"
    );
    assert!(!accepted.retried);
    assert_eq!(persisted.sequence, 2, "the ownership commit is sequence 1");

    // The notice-free retry is an idempotent no-op.
    let (notice, retried, persisted_retry) = store
        .accept_subagent_terminal(None, report.clone(), event.clone())
        .unwrap();
    assert!(notice.is_none());
    assert!(retried.retried);
    assert_eq!(persisted_retry.sequence, persisted.sequence);

    // A semantically different retry of the same child's terminal —
    // same canonical identities, changed content — is a deterministic
    // conflict, never a second publication.
    let (conflicting_report, _) = non_success_publication(
        &store,
        &subagent_id,
        &child_agent_id,
        SubagentTerminalState::Failed,
        "a different diagnostic",
    );
    assert!(matches!(
        store.accept_subagent_terminal(None, conflicting_report, event),
        Err(ConversationStoreError::CorrelationConflict { .. })
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1, "exactly once");

    // Provenance: a failed terminal must be Runtime-authored — an
    // Agent-authored one is rejected before anything is accepted.
    let other = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
    let other_child = crate::runtime::identity::AgentId::new("agent-other");
    own_normal_subagent(&store, &other, &other_child);
    let (mut wrong_report, wrong_event) = non_success_publication(
        &store,
        &other,
        &other_child,
        SubagentTerminalState::Failed,
        "subagent failed: boom",
    );
    wrong_report.source = UserSource::Agent {
        agent_id: other_child,
    };
    assert!(matches!(
        store.accept_subagent_terminal(None, wrong_report, wrong_event),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
}

/// The Issue #192 pair publication: a successful terminal commits the
/// runtime-authored terminal notice (the parent-model execution
/// correlation) and the child-authored report in one transaction —
/// notice first, report (the terminal result) last, the event
/// committed once.
#[test]
#[allow(clippy::too_many_lines)]
fn subagent_terminal_pair_publication_is_atomic_idempotent_and_ordered() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);
    let notice_id = crate::runtime::subagent::terminal_notice_message_id(&subagent_id);
    let report_id = crate::runtime::subagent::terminal_message_id(&subagent_id);
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "the final report");

    let (notice, report, persisted) = store
        .accept_subagent_terminal(Some(notice), report, event)
        .unwrap();
    let notice = notice.expect("the notice was accepted");
    assert!(!notice.retried && !report.retried);
    assert!(
        notice.sequence < report.sequence,
        "the notice is ordered strictly before the terminal report"
    );
    let pending = store.load_pending().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].message.id, notice_id);
    assert_eq!(pending[1].message.id, report_id);
    let notice_text = match &pending[0].message.content[0] {
        UserContentBlock::Text(text) => text.text.clone(),
        other => panic!("the notice is text: {other:?}"),
    };
    assert!(
        notice_text.contains(&format!("Agent {child_agent_id} activation {subagent_id}")),
        "the notice separately names the durable Agent and finite activation: {notice_text}"
    );

    // The retry of the identical pair is an idempotent no-op on both
    // sides and never re-commits the event.
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "the final report");
    let (notice_retry, report_retry, persisted_retry) = store
        .accept_subagent_terminal(Some(notice), report, event)
        .unwrap();
    assert!(notice_retry.expect("notice").retried);
    assert!(report_retry.retried);
    assert_eq!(persisted_retry.sequence, persisted.sequence);
    assert_eq!(store.load_pending().unwrap().len(), 2);

    // A notice that is not runtime-authored is rejected before anything
    // is accepted.
    let other = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
    let other_child = crate::runtime::identity::AgentId::new("agent-other");
    own_normal_subagent(&store, &other, &other_child);
    let (mut child_authored_notice, other_report, other_event) =
        success_publication(&store, &other, &other_child, "the other report");
    child_authored_notice.source = UserSource::Agent {
        agent_id: other_child.clone(),
    };
    assert!(matches!(
        store.accept_subagent_terminal(
            Some(child_authored_notice),
            other_report.clone(),
            other_event.clone()
        ),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    // A non-terminal event payload is rejected just as strictly.
    let wrong_event = envelope(
        &conversation_id,
        "not-a-terminal",
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: other.clone(),
            child_agent_id: other_child,
            child_conversation_id: crate::scripted_suites::common::identity::child_conversation_id(
                other.as_str(),
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-other"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace: std::path::PathBuf::from("/shared-workspace"),
        },
    );
    assert!(matches!(
        store.accept_subagent_terminal(None, other_report, wrong_event),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store.load_pending().unwrap().len(),
        2,
        "rejections accept nothing"
    );
}

/// A durable failure between the pair's acceptances and the event commit
/// commits nothing: the bounded retry then publishes the pair and the
/// fact cleanly and exactly once.
#[test]
fn subagent_terminal_pair_publication_retries_atomically_after_a_fault() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);
    let publication =
        || success_publication(&store, &subagent_id, &child_agent_id, "the final report");

    store.arm_fail_event_times(1);
    let (notice, report, event) = publication();
    assert!(
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .is_err(),
        "the injected journal fault fails the publication"
    );
    assert_eq!(
        store.load_pending().unwrap().len(),
        0,
        "the failed transaction committed neither the notice nor the report"
    );

    let (notice, report, event) = publication();
    let (notice, report, persisted) = store
        .accept_subagent_terminal(Some(notice), report, event)
        .expect("the bounded retry commits the pair");
    assert!(!notice.expect("notice").retried);
    assert!(!report.retried);
    assert_eq!(persisted.sequence, 2);
    assert_eq!(store.load_pending().unwrap().len(), 2);
}

/// The subagent terminal transition is semantically closed (Issue #192):
/// every combination of event, report, and notice that no legal runtime
/// decision sequence could produce is rejected deterministically, and a
/// rejection accepts nothing.
#[test]
#[allow(clippy::too_many_lines)]
fn subagent_terminal_transition_rejects_semantically_unreachable_states() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let owned = |ordinal: u64| {
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        (subagent_id, child_agent_id)
    };
    let reject = |result: Result<_, ConversationStoreError>, pending: usize| {
        assert!(
            matches!(result, Err(ConversationStoreError::InvalidReference(_))),
            "the unreachable state is rejected"
        );
        assert_eq!(
            store.load_pending().unwrap().len(),
            pending,
            "a rejection accepts nothing"
        );
    };

    // A success without its runtime-authored terminal notice.
    let (subagent_id, child) = owned(1);
    let (_, report, event) = success_publication(&store, &subagent_id, &child, "report");
    reject(store.accept_subagent_terminal(None, report, event), 0);

    // A success whose notice names a different subagent.
    let (subagent_id, child) = owned(2);
    let (other_id, other_child) = owned(22);
    let (wrong_notice, _, _) = success_publication(&store, &other_id, &other_child, "other");
    let (_, report, event) = success_publication(&store, &subagent_id, &child, "report");
    reject(
        store.accept_subagent_terminal(Some(wrong_notice), report, event),
        0,
    );
    let _ = other_child;

    // A success with a non-canonical report message identity.
    let (subagent_id, child) = owned(3);
    let (notice, mut report, event) = success_publication(&store, &subagent_id, &child, "report");
    report.message_id = Some(MessageId::new("subagent-3-not-the-terminal"));
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // A success with the wrong report producer correlation.
    let (subagent_id, child) = owned(4);
    let (notice, mut report, event) = success_publication(&store, &subagent_id, &child, "report");
    report.correlation = Some("subagent-terminal:someone-else".to_owned());
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // A success whose notice carries the wrong producer correlation.
    let (subagent_id, child) = owned(5);
    let (mut notice, report, event) = success_publication(&store, &subagent_id, &child, "report");
    notice.correlation = Some("subagent-terminal-notice:someone-else".to_owned());
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // A success whose notice has an invalid inbound kind.
    let (subagent_id, child) = owned(6);
    let (mut notice, report, event) = success_publication(&store, &subagent_id, &child, "report");
    notice.kind = InboundKind::Context(crate::message::types::ContextKind::RuntimeToolObservation);
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // A terminal event referencing the wrong report message.
    let (subagent_id, child) = owned(7);
    let (notice, report, mut event) = success_publication(&store, &subagent_id, &child, "report");
    let RuntimeEvent::SubagentTerminalPublished { message_id, .. } = &mut event.event else {
        panic!("terminal event");
    };
    *message_id = MessageId::new("some-other-message");
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // A non-success terminal must not carry the successful-terminal
    // notice.
    for (ordinal, state) in [
        (9, SubagentTerminalState::Failed),
        (10, SubagentTerminalState::Cancelled),
        (11, SubagentTerminalState::Interrupted),
    ] {
        let (subagent_id, child) = owned(ordinal);
        let (report, event) =
            non_success_publication(&store, &subagent_id, &child, state, "runtime terminal");
        let notice = crate::runtime::subagent::terminal_notice(
            &child,
            &subagent_id,
            &crate::runtime::subagent::SubagentName::parse("explore").expect("canonical name"),
            Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
        );
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );
    }

    // A non-canonical event identity.
    let (subagent_id, child) = owned(12);
    let (notice, report, mut event) = success_publication(&store, &subagent_id, &child, "report");
    event.event_id = EventId::new("not-the-canonical-terminal-event");
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );

    // An attempt/turn envelope is not a publication fact's shape.
    let (subagent_id, child) = owned(13);
    let (notice, report, mut event) = success_publication(&store, &subagent_id, &child, "report");
    event.attempt_id = Some(AttemptId::new("attempt-13"));
    reject(
        store.accept_subagent_terminal(Some(notice), report, event),
        0,
    );
}

/// The notice/report pair is one atomic publication: a retry that finds
/// exactly one side already durable — a state only a bypass could reach —
/// is rejected, and the ambiguous conflicting retry of the same
/// correlation is a deterministic conflict.
#[test]
fn subagent_terminal_pair_retry_must_be_unambiguous() {
    let store = store();
    let conversation_id = store.conversation_id().clone();

    // Only the notice side is durable (committed through the plain
    // acceptance seam): the pair retry must not complete.
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "report");
    store.accept_inbound(notice.clone()).expect("notice alone");
    assert!(matches!(
        store.accept_subagent_terminal(Some(notice), report, event),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store.load_pending().unwrap().len(),
        1,
        "the mismatched pair retry accepted nothing further"
    );

    // Only the report side is durable: symmetric rejection.
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "report");
    store.accept_inbound(report.clone()).expect("report alone");
    assert!(matches!(
        store.accept_subagent_terminal(Some(notice), report, event),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.load_pending().unwrap().len(), 2);

    // A committed pair retried with the same correlations but changed
    // content is a deterministic correlation conflict.
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 3);
    let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
    own_normal_subagent(&store, &subagent_id, &child_agent_id);
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "report");
    store
        .accept_subagent_terminal(Some(notice), report, event)
        .expect("pair commits");
    let (notice, mut report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "report");
    report.content = vec![UserContentBlock::Text(TextBlock {
        text: "a conflicting report body".to_owned(),
    })];
    assert!(matches!(
        store.accept_subagent_terminal(Some(notice), report, event),
        Err(ConversationStoreError::CorrelationConflict { .. })
    ));
    assert_eq!(store.load_pending().unwrap().len(), 4, "exactly once");
}

#[test]
fn workflow_ownership_requires_matching_kind_origin_and_parent_conversation() {
    use crate::events::types::SubagentOwnershipKind;
    use crate::runtime::subagent::AgentActivationOrigin;
    for case in [
        "foreign_run",
        "foreign_envelope",
        "ordinary_origin",
        "normal_kind",
    ] {
        let (source, terminal, output) = workflow_terminal_fixture(
            &crate::runtime::workflow::test_instance("review_pr", "review"),
            false,
        );
        let target = store();
        let mut ownership = source.read_events(None, 10).unwrap().events[0].clone();
        ownership.sequence = 0;
        assert_eq!(&ownership.conversation_id, target.conversation_id());
        let mut invalid = ownership.clone();
        let RuntimeEvent::SubagentOwnershipCommitted {
            origin,
            ownership: kind,
            child_agent_id,
            ..
        } = &mut invalid.event
        else {
            unreachable!()
        };
        let child = child_agent_id.clone();
        match case {
            "foreign_run" | "foreign_envelope" => {
                let AgentActivationOrigin::Workflow { node_id } = origin else {
                    unreachable!()
                };
                node_id.block.run.conversation_id = ConversationId::generate();
                if case == "foreign_envelope" {
                    invalid.conversation_id = node_id.block.run.conversation_id.clone();
                }
            }
            "ordinary_origin" => *origin = AgentActivationOrigin::ClientControl,
            "normal_kind" => *kind = SubagentOwnershipKind::Normal,
            _ => unreachable!(),
        }
        let result = if case == "normal_kind" {
            // Supply legitimate private authority so this tests the kind/origin
            // boundary, not an unrelated missing-authority rejection.
            let (event, authority) =
                crate::local_runtime::session::tests::deletion_tests::admit_agent(invalid);
            target.append_agent_admission(event, &authority)
        } else {
            target.append_event(invalid)
        };
        if case == "foreign_envelope" {
            assert!(
                matches!(
                    result,
                    Err(ConversationStoreError::ConversationIdMismatch { .. })
                ),
                "{case}: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(ConversationStoreError::InvalidReference(_))),
                "{case}: {result:?}"
            );
        }
        assert!(
            target.read_events(None, 10).unwrap().events.is_empty(),
            "{case}"
        );
        assert!(
            target.load_agent_authority(&child).is_err(),
            "no partial private authority: {case}"
        );
        let lifecycle_count: i64 = target
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM lifecycle_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(lifecycle_count, 0, "no partial lifecycle: {case}");
        // The same exact identity is still admissible after rejection.
        target.append_event(ownership).unwrap();
        let committed = target
            .commit_workflow_agent_terminal(terminal.clone(), output.clone())
            .unwrap();
        assert_eq!(
            target
                .commit_workflow_agent_terminal(terminal, output)
                .unwrap(),
            committed
        );
        assert_eq!(target.read_events(None, 10).unwrap().events.len(), 3);
    }
}

#[test]
fn normal_agent_creation_and_resume_ownership_keep_their_native_admission_paths() {
    let store = store();
    let first = crate::runtime::identity::SubagentId::for_conversation(store.conversation_id(), 1);
    let second = crate::runtime::identity::SubagentId::for_conversation(store.conversation_id(), 2);
    let child = AgentId::new("durable-ordinary-agent");
    own_normal_subagent(&store, &first, &child);
    let (notice, report, event) = success_publication(&store, &first, &child, "first result");
    store
        .accept_subagent_terminal(Some(notice), report, event)
        .unwrap();
    own_normal_subagent(&store, &second, &child);
    let (notice, report, event) = success_publication(&store, &second, &child, "resumed result");
    store
        .accept_subagent_terminal(Some(notice), report, event)
        .unwrap();
    assert!(store.load_agent_authority(&child).is_ok());
    let events = store.read_events(None, 10).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::SubagentOwnershipCommitted { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::AgentActivationAdmission {
                    phase: crate::events::types::AgentActivationAdmissionPhase::Reserved,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn workflow_agent_value_and_child_terminal_are_one_atomic_transition() {
    let (store, terminal, output) = workflow_terminal_fixture(
        &crate::runtime::workflow::test_instance("review_pr", "review"),
        false,
    );
    let mut unproven = terminal.clone();
    if let RuntimeEvent::SubagentTerminalSettled {
        physical_settlement_proven,
        ..
    } = &mut unproven.event
    {
        *physical_settlement_proven = false;
    }
    assert!(
        matches!(
            store.commit_workflow_agent_terminal(unproven, output.clone()),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "a valid value cannot commit Workflow success without physical proof"
    );
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 1);

    // A fault after both event rows are staged must roll the pair back;
    // the ownership fact is the only durable state left.
    store.arm_fail_event_times(1);
    assert!(
        store
            .commit_workflow_agent_terminal(terminal.clone(), output.clone())
            .is_err()
    );
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 1);

    let committed = store
        .commit_workflow_agent_terminal(terminal.clone(), output.clone())
        .expect("compound Workflow terminal");
    assert_eq!(committed.0.sequence, 2);
    assert_eq!(committed.1.sequence, 3);
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 3);

    // An ambiguous/idempotent retry returns the same two durable facts and
    // cannot allocate another lifecycle terminal or value event.
    let retried = store
        .commit_workflow_agent_terminal(terminal, output)
        .expect("idempotent compound retry");
    assert_eq!(retried, committed);
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 3);
}

/// Every rejection leaves the exact committed prefix untouched; the valid
/// pair can then commit and retry, including repeated/nested instances.
#[test]
fn workflow_terminal_rejects_inconsistent_and_substituted_typed_identities() {
    use crate::runtime::workflow::{WorkflowId, test_instance};
    for case in [
        "run_field",
        "workflow_field",
        "owned_run",
        "owned_node",
        "visit",
        "loop",
        "parallel",
    ] {
        let mut owned = test_instance("review_pr", "review");
        if case == "loop" {
            owned.block.definition.blocks = vec!["retry".into(), "body".into()];
            owned.block.invocations = vec![0, 1];
        } else if case == "parallel" {
            owned.block.definition.blocks = vec!["checks".into(), "left".into()];
            owned.block.invocations = vec![0, 0];
        }
        let (store, terminal, output) = workflow_terminal_fixture(&owned, false);
        let before = store.read_events(None, 100).unwrap().events;
        let mut invalid = output.clone();
        let RuntimeEvent::WorkflowAgentOutputCommitted {
            workflow_id,
            run_id,
            node_id,
            ..
        } = &mut invalid.event
        else {
            unreachable!()
        };
        match case {
            "run_field" => run_id.invocation += 1,
            "workflow_field" => *workflow_id = WorkflowId::parse("other_workflow").unwrap(),
            "owned_run" => {
                run_id.invocation += 1;
                node_id.block.run = run_id.clone();
            }
            "owned_node" => node_id.node = "another_node".into(),
            "visit" => node_id.visit += 1,
            "loop" => node_id.block.invocations[1] += 1,
            "parallel" => node_id.block.definition.blocks[1] = "right".into(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                store.commit_workflow_agent_terminal(terminal.clone(), invalid.clone()),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "{case}"
        );
        assert_eq!(
            store.read_events(None, 100).unwrap().events,
            before,
            "{case}"
        );
        let committed = store
            .commit_workflow_agent_terminal(terminal.clone(), output.clone())
            .unwrap();
        assert_eq!(
            store
                .commit_workflow_agent_terminal(terminal.clone(), output)
                .unwrap(),
            committed
        );
        let after = store.read_events(None, 100).unwrap().events;
        assert_eq!(after.len(), before.len() + 2);
        // The retry fast path must not bypass identity validation either.
        assert!(
            matches!(
                store.commit_workflow_agent_terminal(terminal, invalid),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "{case}"
        );
        assert_eq!(store.read_events(None, 100).unwrap().events, after);
    }
}

/// The frozen Session execution binding in an ownership fact is one
/// absolute directory; a relative binding is refused before any write.
#[test]
fn subagent_ownership_rejects_a_relative_session_workspace_binding() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let workspace = std::path::PathBuf::from("relative/workspace");
    let error = store
        .append_event(envelope(
            &conversation_id,
            crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: subagent_id.clone(),
                child_agent_id: AgentId::new(format!("agent-{subagent_id}")),
                child_conversation_id:
                    crate::scripted_suites::common::identity::child_conversation_id(
                        subagent_id.as_str(),
                    ),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-sub"),
                },
                agent: "worker".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace,
            },
        ))
        .expect_err("durable ownership must reject a relative workspace binding");
    assert!(matches!(error, ConversationStoreError::InvalidReference(_)));
    assert!(store.read_events(None, 8).unwrap().events.is_empty());
}

/// The durable identity of a `SubagentOwnershipCommitted` fact is
/// canonical: the `EventId` must be the deterministic
/// `subagent-committed-event:{id}` derived from the very `SubagentId`
/// embedded in the payload. A mismatched pair is malformed and must
/// never enter durable authority — no Event Journal row and no
/// lifecycle opening.
#[test]
fn subagent_ownership_rejects_a_mismatched_event_identity() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let s1 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let s2 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
    // The body names S1 but the EventId is the canonical identity of S2:
    // the pair must be rejected by the write-side validation.
    let malformed = envelope(
        &conversation_id,
        crate::runtime::subagent::subagent_ownership_event_id(&s2).as_ref(),
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: s1.clone(),
            child_agent_id: AgentId::new("agent-a"),
            child_conversation_id: crate::runtime::identity::ConversationId::new(
                "conv_4761f81f-3cff-7c88-84c5-21410a0b63f3",
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace: std::path::PathBuf::from("/shared-workspace"),
        },
    );
    let (malformed, authority) =
        crate::local_runtime::session::tests::deletion_tests::admit_agent(malformed);
    assert!(matches!(
        store.append_agent_admission(malformed, &authority),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(
        store
            .read_events(None, 64)
            .expect("events")
            .events
            .is_empty(),
        "no Event Journal row was committed"
    );
    // The correct canonical binding for the same body succeeds.
    let canonical = envelope(
        &conversation_id,
        crate::runtime::subagent::subagent_ownership_event_id(&s1).as_ref(),
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: s1.clone(),
            child_agent_id: AgentId::new("agent-a"),
            child_conversation_id: crate::runtime::identity::ConversationId::new(
                "conv_4761f81f-3cff-7c88-84c5-21410a0b63f3",
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace: std::path::PathBuf::from("/shared-workspace"),
        },
    );
    let (canonical, authority) =
        crate::local_runtime::session::tests::deletion_tests::admit_agent(canonical);
    store
        .append_agent_admission(canonical, &authority)
        .expect("canonical binding succeeds");
}

/// Even when a malformed ownership fact exists (reachable only through
/// a raw database row, since the write path rejects it), the
/// deterministic terminal-provenance lookup defensively revalidates the
/// embedded `SubagentId` before trusting `child_agent_id`: the located
/// fact must actually belong to the requested child.
#[test]
fn subagent_terminal_provenance_revalidates_the_embedded_subagent_identity() {
    let store = store();
    store.initialize(&[]).expect("initialize");
    let conversation_id = store.conversation_id().clone();
    let s1 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let s2 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
    // Raw-insert a fact whose EventId is canonical for S1 but whose
    // embedded SubagentId is S2 — the state only a bypass of the write
    // path could produce.
    let malformed = envelope(
        &conversation_id,
        crate::runtime::subagent::subagent_ownership_event_id(&s1).as_ref(),
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: s2.clone(),
            child_agent_id: AgentId::new("agent-b"),
            child_conversation_id: crate::runtime::identity::ConversationId::new(
                "conv_027fd54f-460d-7e47-84f4-ffff3ee8af4a",
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-b"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace: std::path::PathBuf::from("/shared-workspace"),
        },
    );
    {
        let mut connection = store.lock().expect("store lock");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("transaction");
        transaction
            .execute(
                "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                params![
                    1i64,
                    malformed.event_id.as_str(),
                    i64::from(malformed.schema_version),
                    conversation_id.as_str(),
                    encode(&malformed, "raw subagent ownership").expect("encode")
                ],
            )
            .expect("raw ownership row");
        transaction
            .execute(
                "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,NULL)",
                [format!("subagent:{s1}")],
            )
            .expect("raw lifecycle opening");
        transaction
            .execute(
                "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
                [1i64],
            )
            .expect("bump event sequence");
        transaction.commit().expect("commit raw row");
    }

    // A terminal for S1 resolves the ownership fact by S1's canonical
    // EventId and must reject the embedded-S2 mismatch.
    let (notice, report, event) =
        success_publication(&store, &s1, &AgentId::new("agent-b"), "terminal");
    assert!(
        matches!(
            store.accept_subagent_terminal(Some(notice), report, event),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "the ownership fact located by S1's canonical EventId belongs to S2 and must be rejected"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn event_journal_enforces_dependencies_order_and_terminal_absorption() {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let attempt_id = AttemptId::new("attempt-events");
    let started = store
        .append_event(envelope(
            &conversation_id,
            "attempt-started",
            Some(attempt_id.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
        ))
        .unwrap();
    assert_eq!(started.sequence, 1);

    assert!(matches!(
        store.append_event(envelope(
            &conversation_id,
            "attempt-identity-conflict",
            Some(attempt_id.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("different-attempt"),
            },
        )),
        Err(ConversationStoreError::InvalidReference(detail))
            if detail.contains("attempt identity")
    ));

    let mut turn_started = envelope(
        &conversation_id,
        "turn-started",
        Some(attempt_id.clone()),
        RuntimeEvent::TurnStarted,
    );
    turn_started.turn_id = Some(TurnId::new("turn-events"));
    assert_eq!(store.append_event(turn_started).unwrap().sequence, 2);

    let missing = envelope(
        &conversation_id,
        "missing-assistant",
        Some(attempt_id.clone()),
        RuntimeEvent::AssistantMessageCommitted {
            message_id: MessageId::new("assistant-missing"),
        },
    );
    assert!(matches!(
        store.append_event(missing),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    let assistant = assistant_message("assistant-1", "provider body");
    let committed = store
        .append_canonical_with_event(
            &assistant,
            envelope(
                &conversation_id,
                "assistant-committed",
                Some(attempt_id.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("assistant-1"),
                },
            ),
        )
        .unwrap();
    assert_eq!(committed.0.sequence, 3);
    assert_eq!(store.load_canonical().unwrap().len(), 1);

    let mut turn_completed = envelope(
        &conversation_id,
        "turn-completed",
        Some(attempt_id.clone()),
        RuntimeEvent::TurnCompleted,
    );
    turn_completed.turn_id = Some(TurnId::new("turn-events"));
    assert_eq!(store.append_event(turn_completed).unwrap().sequence, 4);
    assert!(matches!(
        store.append_event({
            let mut duplicate = envelope(
                &conversation_id,
                "turn-terminal-duplicate",
                Some(attempt_id.clone()),
                RuntimeEvent::TurnCompleted,
            );
            duplicate.turn_id = Some(TurnId::new("turn-events"));
            duplicate
        }),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(matches!(
        store.append_event({
            let mut late = envelope(
                &conversation_id,
                "after-turn-terminal",
                Some(attempt_id.clone()),
                RuntimeEvent::ModelRetryScheduled {
                    failed_request_id: RequestId::new("request:attempt-events:turn-events:0"),
                    retry_number: 1,
                    retry_delay_ms: None,
                },
            );
            late.turn_id = Some(TurnId::new("turn-events"));
            late
        }),
        // The event is rejected at the new request-correlation boundary
        // before lifecycle absorption can be checked: this fixture has
        // no durable failed request with that identity.
        Err(ConversationStoreError::InvalidReference(_))
    ));

    let terminal = store
        .append_event(envelope(
            &conversation_id,
            "attempt-terminal",
            Some(attempt_id.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt_id.clone(),
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    assert_eq!(terminal.sequence, 5);
    assert!(matches!(
        store.append_event({
            let mut after = envelope(
                &conversation_id,
                "after-terminal",
                Some(attempt_id),
                RuntimeEvent::TurnCompleted,
            );
            after.turn_id = Some(TurnId::new("turn-events"));
            after
        }),
        Err(ConversationStoreError::TerminalViolation(_))
    ));

    store.arm_fail_event_times(1);
    assert!(
        store
            .append_event(envelope(
                &conversation_id,
                "standalone-event",
                None,
                RuntimeEvent::CompactionStarted {
                    operation_id: crate::runtime::identity::CompactionOperationId::new(
                        "fixture-compaction"
                    )
                },
            ))
            .is_err()
    );
    let standalone = store
        .append_event(envelope(
            &conversation_id,
            "standalone-event",
            None,
            RuntimeEvent::CompactionStarted {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "fixture-compaction",
                ),
            },
        ))
        .unwrap();
    assert_eq!(standalone.sequence, 6);
    assert!(
        store
            .append_event(envelope(
                &conversation_id,
                "standalone-event",
                None,
                RuntimeEvent::CompactionStarted {
                    operation_id: crate::runtime::identity::CompactionOperationId::new(
                        "fixture-compaction"
                    )
                },
            ))
            .is_err()
    );
    let page = store.read_events(None, 2).unwrap();
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].sequence, 1);
    assert_eq!(page.next_sequence, Some(2));
    let rest = store.read_events(page.next_sequence, 10).unwrap();
    assert_eq!(
        rest.events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![3, 4, 5, 6]
    );
    let journal_json = serde_json::to_string(&rest.events).unwrap();
    assert!(!journal_json.contains("provider body"));
}

#[test]
fn journal_cut_observer_reports_commits_but_never_reads_or_rollbacks() {
    #[derive(Default)]
    struct Observer(Mutex<Vec<Option<u64>>>);
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, through: Option<u64>) {
            self.0.lock().unwrap().push(through);
        }
    }
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936178");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let observer = Arc::new(Observer::default());
    assert_eq!(store.observe_journal(observer.clone()).unwrap(), 0);
    let mut event = envelope(
        &id,
        "committed",
        Some(AttemptId::new("attempt")),
        RuntimeEvent::TurnStarted,
    );
    event.turn_id = Some(TurnId::new("1"));
    let committed = store.append_event(event.clone()).unwrap();
    assert_eq!(*observer.0.lock().unwrap(), vec![Some(committed.sequence)]);
    assert!(store.append_event(event).is_err()); // Rejected duplicate is not a new cut.
    store.presentation_frontier().unwrap();
    store.load_head().unwrap();
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "UPDATE rustx_store SET next_event_sequence = next_event_sequence + 1 WHERE id = 1",
                [],
            )
            .unwrap();
        transaction.rollback().unwrap();
    }
    assert_eq!(*observer.0.lock().unwrap(), vec![Some(committed.sequence)]);
    assert_eq!(
        journal_prefix(&store.lock().unwrap()).unwrap(),
        committed.sequence
    );
}

#[test]
fn t10_t19_derived_journal_body_failure_preserves_native_terminal_witness() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936179");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let attempt = AttemptId::new("terminal-layout-failure");
    store
        .append_event(envelope(
            &id,
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_display_body BEFORE INSERT ON content_segments WHEN NEW.source_kind='observation_journal' BEGIN SELECT RAISE(ABORT,'derived source unavailable'); END").unwrap();
    let event = store
        .append_event(envelope(
            &id,
            "settled",
            Some(attempt.clone()),
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: crate::events::types::AttemptFailure::Runtime {
                    error: crate::runtime::types::RuntimeError::Internal {
                        message: "native diagnostic".repeat(10000),
                    },
                },
            },
        ))
        .unwrap();
    assert_eq!(event.sequence, 2);
    store.mark_observation_ready(1, 2).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
    let witness = store.observation_event_witness(2, 2).unwrap().unwrap();
    assert_eq!(witness.event_kind, "attempt_failed");
    assert_eq!(witness.value["error"]["type"], "runtime");
    assert_eq!(witness.value["error"]["error"]["type"], "internal");
    assert!(witness.deferred);
    assert!(
        store
            .event_value_read(2, 2, &["event".into(), "error".into()], 0, 65536)
            .is_err()
    );
    let terminal = store.observation_attempt_summary(&attempt, 2).unwrap();
    assert_eq!(terminal.terminal_kind.as_deref(), Some("attempt_failed"));
    assert!(terminal.failure.is_some());
    assert_eq!(store.read_events(None, 32).unwrap().events.len(), 2);
}

#[test]
fn loop_lifecycle_facts_round_trip_without_executable_state() {
    use crate::runtime::workflow::{WorkflowExecutionOutcome, WorkflowLoopExit, test_instance};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("loop-facts.sqlite");
    let conversation = ConversationId::new("conv_701197a0-a688-77da-8a0f-ce405d769eb8");
    let mut node = test_instance("feedback", "loop");
    node.block.run.conversation_id = conversation.clone();
    node.block.invocations = vec![0, 2];
    let mut body = node.block.clone();
    body.definition
        .blocks
        .extend(["loop".into(), "body".into()]);
    body.invocations.push(3);
    let facts = vec![
        RuntimeEvent::WorkflowLoopIterationAdmitted {
            node: node.clone(),
            body: body.clone(),
            iteration: 3,
        },
        RuntimeEvent::WorkflowLoopIterationSettled {
            node: node.clone(),
            body,
            iteration: 3,
            outcome: WorkflowExecutionOutcome::Completed,
        },
        RuntimeEvent::WorkflowLoopExited {
            node: node.clone(),
            iterations: 3,
            status: WorkflowLoopExit::LoopBreak,
        },
        RuntimeEvent::WorkflowLoopExited {
            node,
            iterations: 3,
            status: WorkflowLoopExit::LoopCompleted,
        },
    ];
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        for (index, fact) in facts.iter().enumerate() {
            store
                .append_event(envelope(
                    &conversation,
                    &format!("loop-fact-{index}"),
                    None,
                    fact.clone(),
                ))
                .unwrap();
        }
    }
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let read = store
        .read_events(None, 16)
        .unwrap()
        .events
        .into_iter()
        .map(|e| e.event)
        .collect::<Vec<_>>();
    assert_eq!(read, facts);
    assert!(store.load_canonical().unwrap().is_empty());
}

/// Both typed cancellation phases are durable canonical facts. They must
/// survive a close/reopen round trip without a compatibility default or
/// prose-based reconstruction.
#[test]
fn issue136_cancellation_phases_survive_durable_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cancellation-phases.sqlite");
    let conversation_id = ConversationId::new("conv_d865e314-7198-7c52-8c86-68581b290333");
    let base = user_message("user-issue-136", "Run the tools");
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("assistant-issue-136"),
        content: vec![
            AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("call-before-start"),
                tool_id: ToolId::new("tool-before-start"),
                name: "before_start".to_owned(),
                arguments: serde_json::json!({}),
            }),
            AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("call-during-execution"),
                tool_id: ToolId::new("tool-during-execution"),
                name: "during_execution".to_owned(),
                arguments: serde_json::json!({}),
            }),
        ],
    });
    let cancelled = |index: u32,
                     message_id: &str,
                     call_id: &str,
                     tool_id: &str,
                     reason: CancellationReason,
                     phase: ToolCancellationPhase| {
        MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("assistant-issue-136"),
                crate::message::types::ContentBlockIndex::new(index),
            ),
            id: MessageId::new(message_id),
            tool_call_id: ToolCallId::new(call_id),
            tool_id: ToolId::new(tool_id),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Cancelled { reason, phase },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        })
    };
    let before_start = cancelled(
        0,
        "result-before-start",
        "call-before-start",
        "tool-before-start",
        CancellationReason::UserRequested,
        ToolCancellationPhase::BeforeStart,
    );
    let during_execution = cancelled(
        1,
        "result-during-execution",
        "call-during-execution",
        "tool-during-execution",
        CancellationReason::RuntimeShutdown,
        ToolCancellationPhase::DuringExecution,
    );
    let expected = vec![
        base.clone(),
        assistant.clone(),
        before_start.clone(),
        during_execution.clone(),
    ];

    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store.initialize(std::slice::from_ref(&base)).unwrap();
        store.append_canonical(&assistant).unwrap();
        store
            .append_canonical_batch(&[before_start.clone(), during_execution.clone()])
            .unwrap();
        assert_eq!(store.load_canonical().unwrap(), expected);
    }

    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    assert_eq!(reopened.load_canonical().unwrap(), expected);
}
