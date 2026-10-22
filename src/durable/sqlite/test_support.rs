//! Shared SQLite store fixtures for the domain test modules.

use crate::context::assembly::ContextGeneration;
use crate::durable::sqlite::*;
use crate::durable::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, PublicationStore, RequestStore,
};
use crate::durable::{ConversationStore, InboundDraft};
use crate::events::types::SubagentTerminalState;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::content::TextBlock;
use crate::message::types::{
    AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantMessageBlock, ContextKind,
};
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContributionEmission, InboundKind, MessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::catalog::ModelCapabilities;
use crate::model::catalog::ModelCompat;
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::snapshot::RequestSnapshot;
use crate::model::snapshot::{ContributionStart, RequestIdentity};
use crate::model::types::ModelProtocol;
use crate::model::types::ModelRequest;
use crate::publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, ToolCallId, TurnId,
};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub(super) fn draft(text: &str) -> InboundDraft {
    InboundDraft {
        message_id: None,
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
        correlation: None,
    }
}

pub(super) fn store() -> SqliteConversationStore {
    SqliteConversationStore::in_memory(ConversationId::new(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
    ))
    .unwrap()
}

pub(super) fn user_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap()),
    })
}

pub(super) fn request_context_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::RuntimeToolObservation),
        timestamp: None,
    })
}

pub(super) fn agent_status_message(
    id: &str,
    generated_at: DateTime<Utc>,
    modules: &[AgentStatusModuleId],
) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "<system-reminder>bounded status</system-reminder>".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(generated_at, modules.to_vec())
                .expect("valid Agent Status metadata"),
        )),
        timestamp: None,
    })
}

pub(super) fn assistant_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
    })
}

pub(super) fn envelope(
    conversation_id: &ConversationId,
    event_id: &str,
    attempt_id: Option<AttemptId>,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id,
        turn_id: None,
        timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
        event,
    }
}

pub(super) fn invocation() -> ModelInvocationConfig {
    ModelInvocationConfig {
        model: "model-before-restart".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        max_output_tokens: 128,
        request_params: RequestParams::new(),
        capabilities: ModelCapabilities::text_only(),
        compat: ModelCompat::default(),
    }
}

/// Freeze once before a test starts a request. Receipt replay uses this exact value.
pub(super) fn freeze_test_input(
    store: &SqliteConversationStore,
    snapshot: &mut RequestSnapshot,
    context: &[MessageBlock],
) {
    let history = store.projected_history().unwrap();
    let mut messages = history.messages.to_vec();
    messages.extend(crate::model::input::canonical_input(context));
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            store.conversation_id(),
            &ModelRequest {
                continuation_source: None,
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                messages: messages.into(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
            },
        )
        .unwrap();
}

pub(super) fn test_request_snapshot(
    revision: crate::conversation::SurfaceRevision,
    invocation: ModelInvocationConfig,
) -> RequestSnapshot {
    RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-1"),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier((revision).get()),
        "system frozen before restart".to_owned(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        invocation,
        4096,
        None,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(7),
        ContextGeneration {
            id: 9,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}

/// Builds a fresh model-turn snapshot with an identity that is unique to
/// the progress test which owns it. The request identity is intentionally
/// the only progress input: the store, not this helper, assigns the
/// durable Todo origin at commit time.
pub(super) fn progress_request_snapshot(
    attempt_id: &str,
    turn: &str,
    retry_number: u32,
    revision: crate::conversation::SurfaceRevision,
    request_context_ids: Vec<MessageId>,
) -> RequestSnapshot {
    RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new(attempt_id),
            turn: TurnId::new(turn),
            retry_number,
        },
        crate::context::resources::LedgerFrontier((revision).get()),
        "frozen".to_owned(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        invocation(),
        1024,
        None,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        request_context_ids,
    )
}

pub(crate) fn hidden_context_audit_fixture() -> HiddenContextAuditFixture {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936190");
    let store = Arc::new(SqliteConversationStore::in_memory(id).unwrap());
    store.initialize(&[]).unwrap();
    let snapshot = context_start_snapshot(store.load_head().unwrap().revision, Vec::new());
    let time = Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap();
    let started = store.commit_model_turn_start(&[], &snapshot, time).unwrap();
    store
        .mark_observation_ready(started.started.sequence, started.started.sequence)
        .unwrap();
    let start = PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(
            &snapshot.identity.attempt_id,
            &snapshot.provisional_message_id,
        ),
        message_id: snapshot.provisional_message_id.clone(),
        attempt_id: snapshot.identity.attempt_id.clone(),
        turn_id: snapshot.identity.turn.clone(),
        request_id: snapshot.request_id.clone(),
    };
    store.open_publication_stream(&start).unwrap();
    store
        .stage_publication_frames(&[PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id,
            sequence: 0,
            payload: PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "original audit prefix".into(),
            },
        }])
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    let context = store
        .append_canonical(&request_context_message(
            "hidden-before-audit",
            "future context",
        ))
        .unwrap();
    assert!(context.transcript_cursor.is_none());
    let (_, audit) = store
        .terminalize_publication_audit(&start.stream_id, time)
        .unwrap();
    assert_eq!(audit.read_cut.ledger_through, context.ledger_position);
    HiddenContextAuditFixture {
        store,
        before,
        context,
        audit,
    }
}

pub(super) fn status_start_snapshot_for(
    attempt_id: &str,
    turn: &str,
    revision: crate::conversation::SurfaceRevision,
    message_id: MessageId,
    emissions: Vec<ContributionEmission>,
    request_context_ids: Vec<MessageId>,
) -> RequestSnapshot {
    let mut snapshot =
        progress_request_snapshot(attempt_id, turn, 0, revision, request_context_ids);
    let producer = crate::runtime::identity::ContextContributorIdentity::Native(
        crate::runtime::identity::NativeContextContributor::AgentStatus,
    );
    snapshot
        .context_generation
        .contributors
        .push(crate::context::ContributorGeneration {
            identity: producer.clone(),
            attestation: None,
        });
    for id in snapshot.request_context_ids.clone() {
        if id != message_id {
            test_bind_contribution(
                &mut snapshot,
                id,
                ContextKind::RuntimeToolObservation,
                vec![],
            );
        }
    }
    snapshot.contributions.push(ContributionStart {
        message_id,
        producer,
        metadata: ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                vec![AgentStatusModuleId::Todo],
            )
            .unwrap(),
        ),
        presentation: None,
        emissions,
        opportunities: crate::context::ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    });
    snapshot
}

/// A request-start snapshot with a caller-chosen ordered context id set.
pub(super) fn context_start_snapshot(
    revision: crate::conversation::SurfaceRevision,
    request_context_ids: Vec<MessageId>,
) -> RequestSnapshot {
    let mut snapshot =
        progress_request_snapshot("attempt-1", "1", 0, revision, request_context_ids.clone());
    for message_id in request_context_ids {
        test_bind_contribution(
            &mut snapshot,
            message_id,
            ContextKind::RuntimeToolObservation,
            vec![],
        );
    }
    snapshot
}

pub(super) fn test_bind_contribution(
    snapshot: &mut RequestSnapshot,
    message_id: MessageId,
    metadata: ContextKind,
    emissions: Vec<ContributionEmission>,
) {
    let producer = crate::runtime::identity::ContextContributorIdentity::Native(
        metadata.native_contribution_owner().unwrap().0,
    );
    if !snapshot
        .context_generation
        .contributors
        .iter()
        .any(|entry| entry.identity == producer)
    {
        snapshot
            .context_generation
            .contributors
            .push(crate::context::ContributorGeneration {
                identity: producer.clone(),
                attestation: None,
            });
    }
    snapshot.contributions.push(ContributionStart {
        message_id,
        producer,
        metadata,
        presentation: None,
        emissions,
        opportunities: crate::context::ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    });
}

pub(super) fn status_start_snapshot(
    revision: crate::conversation::SurfaceRevision,
    message_id: MessageId,
    emissions: Vec<ContributionEmission>,
) -> RequestSnapshot {
    status_start_snapshot_for(
        "attempt-1",
        "1",
        revision,
        message_id.clone(),
        emissions,
        vec![message_id],
    )
}

/// Commits the canonical Normal-ownership fact of one subagent child
/// over a shared workspace (agent name `explore`).
pub(super) fn own_normal_subagent(
    store: &SqliteConversationStore,
    subagent_id: &crate::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
) {
    own_normal_subagent_with_workspace(
        store,
        subagent_id,
        child_agent_id,
        std::path::PathBuf::from("/shared-workspace"),
    );
}

/// Commits the canonical Normal-ownership fact over an explicit frozen
/// Session workspace binding.
pub(super) fn own_normal_subagent_with_workspace(
    store: &SqliteConversationStore,
    subagent_id: &crate::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    workspace: std::path::PathBuf,
) {
    let event = envelope(
        store.conversation_id(),
        crate::runtime::subagent::subagent_ownership_event_id(subagent_id).as_ref(),
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: crate::scripted_suites::common::identity::child_conversation_id(
                subagent_id.as_str(),
            ),
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-sub"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace,
        },
    );
    if store.load_agent_authority(child_agent_id).is_ok() {
        let RuntimeEvent::SubagentOwnershipCommitted { origin, .. } = &event.event else {
            unreachable!("ownership fixture")
        };
        store
            .append_event(crate::runtime::subagent::admission_event(
                store.conversation_id(),
                child_agent_id,
                subagent_id,
                origin,
                crate::events::types::AgentActivationAdmissionPhase::Reserved,
                Utc::now(),
            ))
            .unwrap();
        store.append_event(event).expect("resumed ownership fact");
    } else {
        let (event, authority) =
            crate::local_runtime::session::tests::deletion_tests::admit_agent(event);
        store
            .append_agent_admission(event, &authority)
            .expect("ownership fact");
    }
}

/// The canonical successful terminal publication triple (notice,
/// report, event) exactly as the registry builds it (Issue #192).
pub(super) fn success_publication(
    store: &SqliteConversationStore,
    subagent_id: &crate::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    report_text: &str,
) -> (InboundDraft, InboundDraft, RuntimeEventEnvelope) {
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
    let (report, event) = crate::runtime::subagent::terminal_publication(
        store.conversation_id(),
        subagent_id,
        child_agent_id,
        SubagentTerminalState::Succeeded,
        vec![UserContentBlock::Text(TextBlock {
            text: report_text.to_owned(),
        })],
        true,
        timestamp,
    );
    let notice = crate::runtime::subagent::terminal_notice(
        child_agent_id,
        subagent_id,
        &crate::runtime::subagent::SubagentName::parse("explore").expect("canonical name"),
        timestamp,
    );
    (notice, report, event)
}

/// The canonical non-success terminal publication pair (report, event):
/// one runtime-authored terminal message and no notice.
pub(super) fn non_success_publication(
    store: &SqliteConversationStore,
    subagent_id: &crate::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    text: &str,
) -> (InboundDraft, RuntimeEventEnvelope) {
    crate::runtime::subagent::terminal_publication(
        store.conversation_id(),
        subagent_id,
        child_agent_id,
        state,
        vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        true,
        Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
    )
}

pub(super) fn workflow_terminal_fixture(
    node_instance: &crate::runtime::workflow::WorkflowNodeInstance,
    normal: bool,
) -> (
    SqliteConversationStore,
    RuntimeEventEnvelope,
    RuntimeEventEnvelope,
) {
    let store = store();
    let conversation_id = store.conversation_id().clone();
    let mut node_instance = node_instance.clone();
    node_instance.block.run.conversation_id = conversation_id.clone();
    let subagent_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
    if normal {
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
    } else {
        store
            .append_event(envelope(
                &conversation_id,
                crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                    admitted_authority: None,
                    subagent_id: subagent_id.clone(),
                    child_agent_id: child_agent_id.clone(),
                    child_conversation_id:
                        crate::scripted_suites::common::identity::child_conversation_id(
                            subagent_id.as_str(),
                        ),
                    origin: crate::runtime::subagent::AgentActivationOrigin::Workflow {
                        node_id: Box::new(node_instance.clone()),
                    },
                    agent: "reviewer".to_owned(),
                    definition_digest: "sha256:definition".to_owned(),
                    profile_digest: "sha256:profile".to_owned(),
                    ownership: crate::events::types::SubagentOwnershipKind::Workflow,
                    workspace: std::path::PathBuf::from("/shared-workspace"),
                },
            ))
            .expect("ownership fact");
    }

    let workflow_id = node_instance.block.definition.workflow_id.clone();
    let run_id = node_instance.block.run.clone();
    let terminal = crate::runtime::subagent::terminal_settlement(
        &conversation_id,
        &subagent_id,
        &child_agent_id,
        crate::events::types::SubagentTerminalState::Succeeded,
        true,
        timestamp,
    );
    let output = crate::runtime::subagent::workflow_output_event(
        &conversation_id,
        &subagent_id,
        &workflow_id,
        &run_id,
        &node_instance,
        serde_json::json!({"passed": true}),
        timestamp,
    );

    (store, terminal, output)
}

/// The exact `events.event_json` a schema-30 runtime writes for a
/// Questionnaire request, produced by `origin/main`'s own serializer.
///
/// It is the obsolete choice-only shape: each question carries `options`
/// and `multi_select` instead of a typed `answer`, and the subject records
/// no requester identity at all.
pub(super) const V30_QUESTIONNAIRE_REQUESTED: &str = r#"{"schema_version":1,"event_id":"interaction-requested-v31","sequence":1,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:00Z","event":{"type":"interaction_requested","interaction_id":"interaction-v31","subject":{"type":"questionnaire","invocation_id":{"caller":"agent","call_id":"legacy-call"},"questionnaire":{"questions":[{"question":"Which direction?","header":"Direction","options":[{"label":"First","description":"The first authored option."},{"label":"Second","description":"The second authored option."}],"multi_select":false}]}}}}"#;

/// The matching schema-30 settlement: the answer names the authored
/// **label**, which the typed vocabulary replaced with an option index.
pub(super) const V30_QUESTIONNAIRE_SETTLED: &str = r#"{"schema_version":1,"event_id":"interaction-settled-v31","sequence":2,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:01Z","event":{"type":"interaction_settled","interaction_id":"interaction-v31","settlement":{"type":"questionnaire_submitted","submission":{"answers":[{"question_index":0,"answer":{"type":"single_option","value":{"label":"First"}}}]}}}}"#;

/// A schema-31 subagent ownership fact: `(agent, definition_digest)` and
/// no effective execution-profile identity at all.
pub(super) const V31_SUBAGENT_OWNERSHIP: &str = r#"{"schema_version":1,"event_id":"subagent-committed-event:conv-v31-subagent-1","sequence":1,"conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd","timestamp":"2026-01-01T00:00:00Z","event":{"type":"subagent_ownership_committed","subagent_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","child_agent_id":"agent-child","child_conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","tool_call_id":"call-sub","agent":"explore","definition_digest":"sha256:d1","ownership":"normal","workspace":{"logical_workspace":"<shared-workspace>","isolation":{"mode":"shared"}}}}"#;

pub(super) fn assert_status_original_ranges(
    store: &SqliteConversationStore,
    view: &crate::observation::status::AgentStatusView,
    rendered: &str,
    target: &MessageId,
) {
    use base64::Engine as _;
    let bytes = store
        .content_read(view.rendered_ref.as_ref().unwrap(), 0, 65_536)
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(bytes.data_base64)
            .unwrap(),
        rendered.as_bytes()
    );
    let reference = view
        .opportunities
        .fresh_inbound
        .as_ref()
        .unwrap()
        .target_metadata_ref
        .as_ref()
        .unwrap();
    let chunk = store.content_read(reference, 0, 65_536).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk.data_base64)
        .unwrap();
    let expected_end = reference.readable_end.min(65_536);
    assert_eq!(bytes.len() as u64, expected_end);
    assert_eq!(chunk.offset, 0);
    assert_eq!(chunk.end_offset, expected_end);
    assert_eq!(chunk.readable_end, reference.readable_end);
    assert_eq!(chunk.complete, expected_end == reference.readable_end);
    assert_eq!(
        chunk.next_offset,
        (expected_end < reference.readable_end).then_some(expected_end)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"],
        target.as_str()
    );
    let (rows, materialized) = SqliteConversationStore::content_read_work();
    assert!(rows <= 2 && materialized <= 131_072);
}

impl SqliteConversationStore {
    pub(crate) fn fail_auxiliary_request_insert_once_for_test(
        &self,
        automatic_rollback: bool,
    ) -> Arc<std::sync::atomic::AtomicUsize> {
        use rusqlite::functions::FunctionFlags;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        let connection = self.lock().unwrap();
        connection
            .create_scalar_function(
                "auxiliary_request_fault_once",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| Ok(i64::from(observed.fetch_add(1, Ordering::SeqCst) == 0)),
            )
            .unwrap();
        let raise = if automatic_rollback {
            "ROLLBACK"
        } else {
            "FAIL"
        };
        connection
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER auxiliary_request_fault
            AFTER INSERT ON auxiliary_requests
            WHEN auxiliary_request_fault_once() = 1
            BEGIN SELECT RAISE({raise}, 'consumed required RequestStart storage fault'); END;"
            ))
            .unwrap();
        seen
    }
}

impl SqliteConversationStore {
    pub(crate) fn park_auxiliary_request_insert_until_interrupted_for_test(
        &self,
    ) -> tokio::sync::oneshot::Receiver<()> {
        use rusqlite::functions::FunctionFlags;
        let (entered, running) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let connection = self.lock().unwrap();
        connection
            .create_scalar_function(
                "auxiliary_request_insert_entered",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| {
                    if let Some(entered) = entered.lock().unwrap().take() {
                        let _ = entered.send(());
                    }
                    Ok(1_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TEMP TRIGGER auxiliary_request_interrupt
            AFTER INSERT ON auxiliary_requests BEGIN
                SELECT auxiliary_request_insert_entered();
                SELECT (WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n);
            END;").unwrap();
        running
    }
}
