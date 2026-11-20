//! Shared SQLite store fixtures for the domain test modules.

use crate::model::snapshot::RequestSnapshot;
use crate::observation::EnvelopeNativeObservation;
use crate::sqlite::*;
use crate::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, PublicationStore, RequestStore,
};
use crate::{ConversationStore, InboundDraft};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use rustx_conversation::surface::SurfaceOp;
use rustx_identity::ToolId;
use rustx_publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use rustx_types::context::assembly::ContextGeneration;
use rustx_types::context::contribution::ContributionStart;
use rustx_types::events::types::SubagentTerminalState;
use rustx_types::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx_types::message::content::TextBlock;
use rustx_types::message::types::ToolMessageBlock;
use rustx_types::message::types::{
    AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantMessageBlock, ContextKind,
};
use rustx_types::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContributionEmission, InboundKind, MessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx_types::model::capabilities::ModelCapabilities;
use rustx_types::model::capabilities::ModelCompat;
use rustx_types::model::finish::ModelFinishReason;
use rustx_types::model::invocation_config::ModelInvocationConfig;
use rustx_types::model::request_params::RequestParams;
use rustx_types::model::snapshot::RequestIdentity;
use rustx_types::model::types::ModelProtocol;
use rustx_types::model::types::ModelRequest;
use rustx_types::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, ToolCallId, TurnId,
};
use rustx_types::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use rustx_types::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use std::sync::Arc;

pub fn draft(text: &str) -> InboundDraft {
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

pub fn store() -> SqliteConversationStore {
    SqliteConversationStore::in_memory(ConversationId::new(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
    ))
    .unwrap()
}

pub fn user_message(id: &str, text: &str) -> MessageBlock {
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

pub fn request_context_message(id: &str, text: &str) -> MessageBlock {
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

pub fn agent_status_message(
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

pub fn assistant_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
    })
}

pub fn envelope(
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

pub fn invocation() -> ModelInvocationConfig {
    ModelInvocationConfig {
        replay_binding: Some("test-binding".into()),
        model: "model-before-restart".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        max_output_tokens: 128,
        request_params: RequestParams::new(),
        capabilities: ModelCapabilities::text_only(),
        compat: ModelCompat::default(),
    }
}

/// Freeze once before a test starts a request. Receipt replay uses this exact value.
pub fn freeze_test_input(
    store: &SqliteConversationStore,
    snapshot: &mut RequestSnapshot,
    context: &[MessageBlock],
) {
    let history = store.projected_history().unwrap();
    let mut messages = history.messages.to_vec();
    messages.extend(rustx_types::model::input::canonical_input(context));
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            store.conversation_id(),
            &ModelRequest {
                response_schema: None,
                response_byte_limit: None,
                continuation_source: None,
                images: rustx_types::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                messages: messages.into(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
            },
        )
        .unwrap();
}

pub fn test_request_snapshot(
    revision: rustx_conversation::SurfaceRevision,
    invocation: ModelInvocationConfig,
) -> RequestSnapshot {
    RequestSnapshot::new(
        rustx_types::runtime::types::ApprovalMode::FullAccess,
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-1"),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        rustx_types::context::resources::LedgerFrontier((revision).get()),
        "system frozen before restart".to_owned(),
        Vec::new(),
        rustx_types::runtime::identity::RuntimeResourceRevision::new(1),
        invocation,
        4096,
        None,
        Vec::new(),
        rustx_types::runtime::identity::CapabilityRevision::new(7),
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
pub fn progress_request_snapshot(
    attempt_id: &str,
    turn: &str,
    retry_number: u32,
    revision: rustx_conversation::SurfaceRevision,
    request_context_ids: Vec<MessageId>,
) -> RequestSnapshot {
    RequestSnapshot::new(
        rustx_types::runtime::types::ApprovalMode::FullAccess,
        RequestIdentity {
            attempt_id: AttemptId::new(attempt_id),
            turn: TurnId::new(turn),
            retry_number,
        },
        rustx_types::context::resources::LedgerFrontier((revision).get()),
        "frozen".to_owned(),
        Vec::new(),
        rustx_types::runtime::identity::RuntimeResourceRevision::new(1),
        invocation(),
        1024,
        None,
        Vec::new(),
        rustx_types::runtime::identity::CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        request_context_ids,
    )
}

pub fn hidden_context_audit_fixture() -> HiddenContextAuditFixture {
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
    store
        .stage_publication_frames(
            &start,
            &[PublicationFrame {
                stream_id: start.stream_id.clone(),
                message_id: start.message_id.clone(),
                sequence: 0,
                payload: PublicationPayload::TextSuffix {
                    block_index: ContentBlockIndex::new(0),
                    suffix: "original audit prefix".into(),
                },
            }],
        )
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    let context = store
        .append_canonical(&request_context_message(
            "hidden-before-audit",
            "future context",
        ))
        .unwrap();
    assert!(context.transcript_cursor.is_none());
    let (_, audit) = store.terminalize_publication_audit(&start, time).unwrap();
    assert_eq!(audit.read_cut.ledger_through, context.ledger_position);
    HiddenContextAuditFixture {
        store,
        before,
        context,
        audit,
    }
}

pub fn status_start_snapshot_for(
    attempt_id: &str,
    turn: &str,
    revision: rustx_conversation::SurfaceRevision,
    message_id: MessageId,
    emissions: Vec<ContributionEmission>,
    request_context_ids: Vec<MessageId>,
) -> RequestSnapshot {
    let mut snapshot =
        progress_request_snapshot(attempt_id, turn, 0, revision, request_context_ids);
    let producer = rustx_types::runtime::identity::ContextContributorIdentity::Native(
        rustx_types::runtime::identity::NativeContextContributor::AgentStatus,
    );
    snapshot.context_generation.contributors.push(
        rustx_types::context::assembly::ContributorGeneration {
            identity: producer.clone(),
            attestation: None,
        },
    );
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
        opportunities: rustx_types::context::contribution::ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    });
    snapshot
}

/// A request-start snapshot with a caller-chosen ordered context id set.
pub fn context_start_snapshot(
    revision: rustx_conversation::SurfaceRevision,
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

pub fn test_bind_contribution(
    snapshot: &mut RequestSnapshot,
    message_id: MessageId,
    metadata: ContextKind,
    emissions: Vec<ContributionEmission>,
) {
    let producer = rustx_types::runtime::identity::ContextContributorIdentity::Native(
        metadata.native_contribution_owner().unwrap().0,
    );
    if !snapshot
        .context_generation
        .contributors
        .iter()
        .any(|entry| entry.identity == producer)
    {
        snapshot.context_generation.contributors.push(
            rustx_types::context::assembly::ContributorGeneration {
                identity: producer.clone(),
                attestation: None,
            },
        );
    }
    snapshot.contributions.push(ContributionStart {
        message_id,
        producer,
        metadata,
        presentation: None,
        emissions,
        opportunities: rustx_types::context::contribution::ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    });
}

pub fn status_start_snapshot(
    revision: rustx_conversation::SurfaceRevision,
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
pub fn own_normal_subagent(
    store: &SqliteConversationStore,
    subagent_id: &rustx_types::runtime::identity::SubagentId,
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
pub fn own_normal_subagent_with_workspace(
    store: &SqliteConversationStore,
    subagent_id: &rustx_types::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    workspace: std::path::PathBuf,
) {
    let mut event = envelope(
        store.conversation_id(),
        crate::subagent_facts::subagent_ownership_event_id(subagent_id).as_ref(),
        None,
        RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: rustx_types::runtime::identity::AgentId::new("agent-parent"),
            admitted_authority: None,
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: child_conversation_id(subagent_id.as_str()),
            origin: rustx_types::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("call-sub"),
            },
            agent: "explore".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: rustx_types::events::types::SubagentOwnershipKind::Normal,
            workspace,
        },
    );
    if store.load_agent_authority(child_agent_id).is_ok() {
        let RuntimeEvent::SubagentOwnershipCommitted { origin, .. } = &event.event else {
            unreachable!("ownership fixture")
        };
        store
            .append_event(crate::subagent_facts::admission_event(
                store.conversation_id(),
                child_agent_id,
                subagent_id,
                origin,
                rustx_types::events::types::AgentActivationAdmissionPhase::Reserved,
                Utc::now(),
            ))
            .unwrap();
        store.append_event(event).expect("resumed ownership fact");
    } else {
        let authority = admit_authority_doc();
        if let RuntimeEvent::SubagentOwnershipCommitted {
            admitted_authority,
            child_agent_id,
            ..
        } = &mut event.event
        {
            *admitted_authority = Some(child_agent_id.clone());
        }
        store
            .append_agent_admission(event, &authority)
            .expect("ownership fact");
    }
}

/// The canonical successful terminal publication triple (notice,
/// report, event) exactly as the registry builds it (Issue #192).
pub fn success_publication(
    store: &SqliteConversationStore,
    subagent_id: &rustx_types::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    report_text: &str,
) -> (InboundDraft, InboundDraft, RuntimeEventEnvelope) {
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
    let (report, event) = crate::subagent_facts::terminal_publication(
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
    let notice = crate::subagent_facts::terminal_notice(
        child_agent_id,
        subagent_id,
        &rustx_types::runtime::subagent::SubagentName::parse("explore").expect("canonical name"),
        timestamp,
    );
    (notice, report, event)
}

/// The canonical non-success terminal publication pair (report, event):
/// one runtime-authored terminal message and no notice.
pub fn non_success_publication(
    store: &SqliteConversationStore,
    subagent_id: &rustx_types::runtime::identity::SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    text: &str,
) -> (InboundDraft, RuntimeEventEnvelope) {
    crate::subagent_facts::terminal_publication(
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

pub fn workflow_terminal_fixture(
    node_instance: &rustx_types::runtime::workflow::WorkflowNodeInstance,
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
    let subagent_id =
        rustx_types::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
    let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
    if normal {
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
    } else {
        store
            .append_event(envelope(
                &conversation_id,
                crate::subagent_facts::subagent_ownership_event_id(&subagent_id).as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id: rustx_types::runtime::identity::AgentId::new("agent-parent"),
                    admitted_authority: None,
                    subagent_id: subagent_id.clone(),
                    child_agent_id: child_agent_id.clone(),
                    child_conversation_id: child_conversation_id(subagent_id.as_str()),
                    origin: rustx_types::runtime::subagent::AgentActivationOrigin::Workflow {
                        node_id: Box::new(node_instance.clone()),
                    },
                    agent: "reviewer".to_owned(),
                    definition_digest: "sha256:definition".to_owned(),
                    profile_digest: "sha256:profile".to_owned(),
                    ownership: rustx_types::events::types::SubagentOwnershipKind::Workflow,
                    workspace: std::path::PathBuf::from("/shared-workspace"),
                },
            ))
            .expect("ownership fact");
    }

    let workflow_id = node_instance.block.definition.workflow_id.clone();
    let run_id = node_instance.block.run.clone();
    let terminal = crate::subagent_facts::terminal_settlement(
        &conversation_id,
        &subagent_id,
        &child_agent_id,
        rustx_types::events::types::SubagentTerminalState::Succeeded,
        true,
        timestamp,
    );
    let output = crate::subagent_facts::workflow_output_event(
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
pub const V30_QUESTIONNAIRE_REQUESTED: &str = r#"{"schema_version":1,"event_id":"interaction-requested-v31","sequence":1,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:00Z","event":{"type":"interaction_requested","interaction_id":"interaction-v31","subject":{"type":"questionnaire","invocation_id":{"caller":"agent","call_id":"legacy-call"},"questionnaire":{"questions":[{"question":"Which direction?","header":"Direction","options":[{"label":"First","description":"The first authored option."},{"label":"Second","description":"The second authored option."}],"multi_select":false}]}}}}"#;

/// The matching schema-30 settlement: the answer names the authored
/// **label**, which the typed vocabulary replaced with an option index.
pub const V30_QUESTIONNAIRE_SETTLED: &str = r#"{"schema_version":1,"event_id":"interaction-settled-v31","sequence":2,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:01Z","event":{"type":"interaction_settled","interaction_id":"interaction-v31","settlement":{"type":"questionnaire_submitted","submission":{"answers":[{"question_index":0,"answer":{"type":"single_option","value":{"label":"First"}}}]}}}}"#;

/// A schema-31 subagent ownership fact: `(agent, definition_digest)` and
/// no effective execution-profile identity at all.
pub const V31_SUBAGENT_OWNERSHIP: &str = r#"{"schema_version":1,"event_id":"subagent-committed-event:conv-v31-subagent-1","sequence":1,"conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd","timestamp":"2026-01-01T00:00:00Z","event":{"type":"subagent_ownership_committed","subagent_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","child_agent_id":"agent-child","child_conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","tool_call_id":"call-sub","agent":"explore","definition_digest":"sha256:d1","ownership":"normal","workspace":{"logical_workspace":"<shared-workspace>","isolation":{"mode":"shared"}}}}"#;

pub fn assert_status_original_ranges(
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
    pub fn fail_auxiliary_request_insert_once_for_test(
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
    pub fn park_auxiliary_request_insert_until_interrupted_for_test(
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

/// A private-authority document fixture: the journal stores the owner's
/// opaque serialization plus its credential escrow verbatim.
pub fn admit_authority_doc() -> crate::store::journal::PrivateAgentAuthority {
    crate::store::journal::PrivateAgentAuthority {
        authority: serde_json::json!({
            "title": "fixture-agent",
            "resolved": {
                "instructions": "private-instructions",
                "environment": [["PRIVATE_KEY", "private-literal"]],
                "materialization": {"sources": {}},
            },
        }),
        credentials: vec![
            ("PROVIDER_TOKEN".to_owned(), "captured-provider".to_owned()),
            ("SOURCE_TOKEN".to_owned(), "captured-source".to_owned()),
        ],
    }
}

pub fn child_conversation_id(key: &str) -> rustx_types::runtime::identity::ConversationId {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(key.as_bytes());
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    rustx_types::runtime::identity::ConversationId::from_uuid(uuid::Uuid::from_bytes(bytes))
        .unwrap()
}
pub fn response_start(store: &SqliteConversationStore, name: &str) -> AttemptId {
    let attempt = AttemptId::new(format!("response-{name}"));
    store
        .append_event(envelope(
            store.conversation_id(),
            &format!("{name}-start"),
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    attempt
}
pub fn response_message(store: &SqliteConversationStore, attempt: &AttemptId, id: &str) {
    store
        .append_canonical_with_event(
            &assistant_message(id, "original exact body 水"),
            envelope(
                store.conversation_id(),
                &format!("event-{id}"),
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new(id),
                },
            ),
        )
        .unwrap();
}
pub fn response_complete(store: &SqliteConversationStore, attempt: &AttemptId) {
    store
        .append_event(envelope(
            store.conversation_id(),
            &format!("{}-end", attempt.as_str()),
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
}
pub fn append_completed_turns(
    store: &SqliteConversationStore,
    first: usize,
    turns: usize,
    tool: bool,
) {
    use crate::InboxStore;
    use crate::sqlite::test_support::{response_complete, response_message, response_start};
    use rustx_types::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, ToolMessageBlock,
    };
    use rustx_types::runtime::identity::{MessageId, ToolCallId, ToolId};
    use rustx_types::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

    fn model_step(
        store: &SqliteConversationStore,
        attempt: &AttemptId,
        turn: &str,
        tools: bool,
    ) -> RequestSnapshot {
        let phase = super::query_work::phase;
        phase("read:request");
        let mut snapshot = progress_request_snapshot(
            attempt.as_str(),
            turn,
            0,
            store.load_head().unwrap().revision,
            vec![],
        );
        let history = store.projected_history().unwrap();
        let request = rustx_types::model::types::ModelRequest {
            response_schema: None,
            response_byte_limit: None,
            continuation_source: None,
            images: snapshot.images.clone(),
            invocation: snapshot.invocation.clone(),
            messages: history.messages.clone(),
            tools: snapshot.tool_definitions.clone(),
            effective_system_prompt: snapshot.effective_system_prompt.clone(),
            continuation: snapshot.continuation.clone(),
        };
        snapshot.history_cut = history.cut.clone();
        snapshot.tool_associations = crate::model::occurrence::freeze_associations(
            &history,
            &snapshot.request_id,
            &request,
            None,
        )
        .unwrap();
        snapshot
            .freeze_selected_input(store.conversation_id(), &request, &history.input_manifest)
            .unwrap();
        phase("commit_model_turn_start");
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        phase("event:model_request_completed");
        let mut event = envelope(
            store.conversation_id(),
            "",
            Some(attempt.clone()),
            RuntimeEvent::ModelRequestCompleted {
                owner: crate::model::snapshot::ModelRequestOwner::Agent {
                    identity: snapshot.identity.clone(),
                },
                request_id: snapshot.request_id.clone(),
                finish_reason: if tools {
                    rustx_types::model::finish::ModelFinishReason::ToolCalls
                } else {
                    rustx_types::model::finish::ModelFinishReason::Stop
                },
                usage: None,
                generation: None,
            },
        );
        event.turn_id = Some(snapshot.identity.turn.clone());
        store.append_event(event).unwrap();
        snapshot
    }

    let phase = super::query_work::phase;
    for ordinal in first..first + turns {
        phase("accept_inbound");
        let accepted = store.accept_inbound(draft("turn growth")).unwrap();
        phase("adopt_pending_batch");
        store.adopt_pending_batch(accepted.sequence, None).unwrap();
        phase("event:attempt_started");
        let attempt = response_start(store, &format!("frontier-{ordinal}"));
        let first_request = model_step(store, &attempt, "1", tool);
        let final_request = if tool {
            let call = ToolCall {
                id: ToolCallId::new(format!("call-{ordinal}")),
                tool_id: ToolId::new("read"),
                name: "read".into(),
                arguments: serde_json::json!({"path":"fixture.txt"}),
            };
            phase("canonical:tool_call");
            let proposal = MessageBlock::Assistant(AssistantMessageBlock {
                id: first_request.provisional_message_id.clone(),
                content: vec![AssistantContentBlock::ToolCall(call.clone())],
            });
            store
                .append_canonical_with_event(
                    &proposal,
                    envelope(
                        store.conversation_id(),
                        "",
                        Some(attempt.clone()),
                        RuntimeEvent::AssistantMessageCommitted {
                            message_id: proposal.id().clone(),
                        },
                    ),
                )
                .unwrap();
            phase("event:tool_started");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "",
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionStarted {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                    },
                ))
                .unwrap();
            let result = ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            };
            phase("event:tool_completed");
            store
                .append_event(envelope(
                    store.conversation_id(),
                    "",
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionCompleted {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        result: result.clone().into(),
                    },
                ))
                .unwrap();
            phase("canonical:tool_result");
            let message = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!("result-{ordinal}")),
                occurrence: rustx_types::message::types::ToolCallOccurrenceRef::new(
                    proposal.id().clone(),
                    ContentBlockIndex::new(0),
                ),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id,
                result: result.into(),
            });
            store
                .append_canonical_with_event(
                    &message,
                    envelope(
                        store.conversation_id(),
                        "",
                        Some(attempt.clone()),
                        RuntimeEvent::ToolMessageCommitted {
                            message_id: message.id().clone(),
                            tool_call_id: call.id,
                        },
                    ),
                )
                .unwrap();
            model_step(store, &attempt, "2", false)
        } else {
            first_request
        };
        phase("canonical:assistant");
        response_message(
            store,
            &attempt,
            final_request.provisional_message_id.as_str(),
        );
        phase("event:attempt_completed");
        response_complete(store, &attempt);
        phase("");
    }
}
// Scripted-suite vocabulary fixtures shared with the monolith test tree. The
// canonical implementation lives here (durable owns the journaled shapes the
// fixtures produce); `tests/{support,common}` delegates to these.

/// A normalized successful tool result with fixed deterministic fields.
#[must_use]
pub fn success_result(text: &str) -> rustx_types::tools::types::ToolExecutionResult {
    rustx_types::tools::types::ToolExecutionResult {
        deliveries: Vec::new(),
        status: rustx_types::tools::types::ToolExecutionStatus::Success,
        content: vec![rustx_types::tools::types::ToolResultContent::Text(
            rustx_types::message::content::TextBlock {
                text: text.to_owned(),
            },
        )],
        duration_ms: 7,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

/// The canonical text-only invocation configuration of an adapter test.
#[must_use]
pub fn invocation_for(
    protocol: rustx_types::model::types::ModelProtocol,
    model: &str,
) -> rustx_types::model::invocation_config::ModelInvocationConfig {
    let chat_reasoning_replay = (protocol
        == rustx_types::model::types::ModelProtocol::OpenAiChatCompletions)
        .then_some(rustx_types::model::capabilities::ChatReasoningReplay::Omit);
    rustx_types::model::invocation_config::ModelInvocationConfig {
        replay_binding: Some("test-binding".into()),
        model: model.to_owned(),
        protocol,
        max_output_tokens: 512,
        request_params: rustx_types::model::request_params::RequestParams::new(),
        capabilities: rustx_types::model::capabilities::ModelCapabilities::text_only(),
        compat: rustx_types::model::capabilities::ModelCompat {
            chat_reasoning_replay,
            ..rustx_types::model::capabilities::ModelCompat::default()
        },
    }
}

/// A canonical user-only request for the given protocol.
#[must_use]
pub fn simple_request(
    protocol: rustx_types::model::types::ModelProtocol,
    model: &str,
    prompt: &str,
) -> rustx_types::model::types::ModelRequest {
    use rustx_types::message::content::TextBlock;
    use rustx_types::message::types::{
        MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx_types::runtime::identity::MessageId;
    rustx_types::model::types::ModelRequest {
        response_schema: None,
        response_byte_limit: None,
        continuation_source: None,
        images: std::collections::BTreeMap::new(),
        invocation: invocation_for(protocol, model),
        messages: vec![rustx_types::model::input::ModelInputMessage::Canonical(
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: prompt.to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx_types::message::types::InboundKind::Message,
                timestamp: None,
            }),
        )]
        .into(),
        tools: Vec::new(),
        effective_system_prompt: String::new(),
        continuation: None,
    }
}

/// Native durable checkpoint fixture. Selection is explicit in model-input
/// indices; the fixture never inserts a summary into the canonical Ledger.
pub fn checkpoint_fixture(
    store: &dyn crate::ConversationStore,
    id: &str,
    retired: usize,
    text: &str,
) -> rustx_types::context::checkpoint::CheckpointCandidate {
    use rustx_types::context::checkpoint::{
        CheckpointCandidate, CheckpointItem, CheckpointMessage,
    };
    use rustx_types::model::input::ModelInputMessage;
    use rustx_types::model::projected::{
        CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
        SemanticTransformation,
    };
    let history = store.projected_history().expect("fixture historical input");
    assert!(retired > 0 && retired <= history.messages.len());
    let parent = store.projection_head().unwrap();
    let originals: Vec<_> = history
        .messages
        .iter()
        .take(retired)
        .filter_map(|m| m.as_canonical().cloned())
        .collect();
    let physical = rustx_types::context::compaction_metadata::retired_span_metadata(&originals);
    let uses_parent = history.messages.iter().take(retired).any(|m| {
        matches!(m, ModelInputMessage::Derived(d) if d.transformations.contains(&SemanticTransformation::Summary))
    });
    let metadata = rustx_types::message::types::CompactionSummaryMetadata::accumulate(
        parent
            .as_ref()
            .filter(|_| uses_parent)
            .and_then(|p| p.candidate.summary_metadata.as_ref()),
        physical.read_files().to_vec(),
        physical.modified_files().to_vec(),
    );
    let checkpoint_id = rustx_types::runtime::identity::CheckpointId::new(id);
    let mut items = vec![CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![rustx_types::message::types::UserContentBlock::Text(
                    rustx_types::message::content::TextBlock {
                        text: rustx_types::context::compaction_metadata::render_summary_text(
                            text, &metadata,
                        ),
                    },
                )],
                source: rustx_types::message::types::UserSource::Runtime,
                kind: rustx_types::model::projected::ProjectedUserKind::Message,
                timestamp: None,
            },
            sources: originals
                .iter()
                .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
                .collect(),
            checkpoint_sources: parent
                .as_ref()
                .filter(|_| uses_parent)
                .map(|p| CheckpointSourceRef {
                    conversation_id: store.conversation_id().clone(),
                    checkpoint_id: p.candidate.checkpoint_id.clone(),
                    content_digest: p.fingerprint.clone(),
                })
                .into_iter()
                .collect(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: vec![],
        result_call: None,
    }];
    for mut item in history
        .checkpoint_items(parent.as_ref())
        .expect("fixture checkpoint items")
        .into_iter()
        .skip(retired)
    {
        if let Some(call) = &mut item.result_call {
            call.message.checkpoint_id = checkpoint_id.clone();
            call.message.item_index = call
                .message
                .item_index
                .checked_sub(u32::try_from(retired).expect("bounded fixture prefix"))
                .unwrap()
                + 1;
        }
        items.push(item);
    }
    CheckpointCandidate {
        checkpoint_id,
        expected_head: history.cut.checkpoint_id,
        read_cut: store.compaction_resource_cut().unwrap(),
        base_frontier: history.cut.ledger,
        items,
        summary_metadata: Some(metadata),
        summary_generation: None,
    }
}

/// Fixture constructs an owned commit receipt input.
#[allow(clippy::needless_pass_by_value)]
pub fn checkpoint_commit_fixture(
    store: &dyn crate::ConversationStore,
    candidate: rustx_types::context::checkpoint::CheckpointCandidate,
    attempt: Option<rustx_types::runtime::identity::AttemptId>,
) -> crate::projection::ProjectionCommitInput {
    let operation_id = rustx_types::runtime::identity::CompactionOperationId::new(format!(
        "operation:{}",
        candidate.checkpoint_id
    ));
    let signal = rustx_types::runtime::cancellation::CancellationSignal::new();
    crate::projection::ProjectionCommitInput {
        grant: signal
            .authorize_compaction_commit(operation_id.clone())
            .unwrap(),
        operation_id,
        candidate,
        owner: store.projection_owner(attempt.as_ref()).unwrap(),
        tokens_before: rustx_types::runtime::types::TokenMeasurement {
            input_tokens: 100,
            source: rustx_types::runtime::types::TokenMeasurementSource::Estimated,
        },
        estimated_tokens_after: 10,
        occupancy: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
    }
}

// ---- shared observation test builders (moved from observation/tests/*) ----

pub fn derived_enrichment_proposal() -> (MessageBlock, ToolCall) {
    let call = ToolCall {
        id: ToolCallId::new("derived-tool-call"),
        tool_id: ToolId::new("derived-tool"),
        name: "Original native tool".into(),
        arguments: serde_json::json!({"original":true}),
    };
    (
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("derived-native-proposal"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        }),
        call,
    )
}

pub fn original_success_tool_result() -> ToolExecutionResult {
    use rustx_types::tools::types::ToolResultContent;
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![
            ToolResultContent::Text(TextBlock {
                text: "original🙂".repeat(10_000),
            }),
            ToolResultContent::Json {
                value: serde_json::json!({"content":[{"not":"a ContentBinding"}],"collections":[{"not":"a frontier"}],"integer":u64::MAX}),
            },
        ],
        duration_ms: 7,
        exit_code: None,
        artifacts: vec![],
        deliveries: vec![],
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

pub fn original_tool_before_result() -> (SqliteConversationStore, AttemptId, MessageBlock, ToolCall)
{
    let store = store();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("successful-original-owner");
    let (proposal, call) = derived_enrichment_proposal();
    store
        .append_canonical_with_event(
            &proposal,
            envelope(
                store.conversation_id(),
                "original-proposal",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: proposal.id().clone(),
                },
            ),
        )
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "original-start",
            Some(attempt.clone()),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    (store, attempt, proposal, call)
}

pub fn member_completed_lineage(original: &str) -> LineageSeed {
    use crate::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    use crate::response::{CompletedResponseProvenance, ResponseOrigin};
    let id = MessageId::new(original);
    let conversation = ConversationId::generate();
    let attempt = AttemptId::new("inherited-giant-final");
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
    LineageSeed::history(vec![
        user_message("giant-member-input", "native input"),
        assistant_message(original, "original final"),
    ])
    .with_turns(vec![TurnReadingProvenance {
        id: ConversationTurnId {
            conversation_id: conversation.clone(),
            attempt_id: attempt.clone(),
        },
        process_message_ids: vec![id.clone()],
        preceding_message_id: None,
        prompt_message_id: Some(MessageId::new("giant-member-input")),
        outcome: InheritedTurnOutcome::Completed,
        started_at: Some(at),
        ended_at: Some(at),
        execution: Some(TurnExecution {
            completed_response: true,
            ..Default::default()
        }),
    }])
    .unwrap()
    .with_completed_responses(vec![CompletedResponseProvenance {
        process_message_ids: vec![id.clone()],
        closing_message_id: id.clone(),
        origin: ResponseOrigin {
            conversation_id: conversation,
            attempt_id: attempt,
            closing_message_id: id,
        },
        completed_at: at,
        retry_message_id: None,
        usage: None,
        timing: None,
        models: Vec::new(),
    }])
    .unwrap()
}

pub fn member_source_page(
    store: &SqliteConversationStore,
    cut: &crate::observation::ObservationReadCut,
) -> crate::observation::transcript::RuntimeClientTranscriptPage {
    let mut page = crate::observation::transcript::transcript_header_view(
        store.transcript_header_page(cut, None, 32).unwrap(),
    );
    crate::response_completed::decorate_window(store, &mut page, cut.read.journal).unwrap();
    page
}

pub fn commit_original_tool_result(
    store: &SqliteConversationStore,
    attempt: &AttemptId,
    proposal: &MessageBlock,
    call: &ToolCall,
    result: &ToolExecutionResult,
) {
    let message = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("original-success-c"),
        occurrence: rustx_types::message::types::ToolCallOccurrenceRef::new(
            proposal.id().clone(),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: result.clone().into(),
    });
    let (committed, receipt) = store
        .append_canonical_with_event(
            &message,
            envelope(
                store.conversation_id(),
                "original-result-commit",
                Some(attempt.clone()),
                RuntimeEvent::ToolMessageCommitted {
                    tool_call_id: call.id.clone(),
                    message_id: message.id().clone(),
                },
            ),
        )
        .unwrap();
    assert_eq!(receipt.ledger_position, 2);
    assert!(
        !committed
            .observation
            .native_batch()
            .unwrap()
            .source_unavailable
    );
    assert_eq!(
        store.load_messages(&[message.id().clone()]).unwrap(),
        vec![message]
    );
}

pub fn assert_projection_receipt_head(receipt: &crate::projection::ProjectionCommitReceipt) {
    let witness = projection_receipt_witness(receipt);
    assert!(!witness.source_unavailable);
    let reading = witness.occupancy.as_ref().unwrap();
    assert_eq!(
        reading.projection_head.transcript_anchor.get(),
        receipt
            .completed
            .observation
            .native_batch()
            .unwrap()
            .read_cut
            .read
            .transcript
    );
    assert_eq!(
        reading.projection_head.activation_id,
        receipt.checkpoint.activation.activation
    );
    assert_eq!(
        reading.projection_head.checkpoint_id,
        receipt.checkpoint.candidate.checkpoint_id
    );
    assert_eq!(
        reading.projection_head.origin.completed_journal.0,
        receipt.completed.sequence
    );
    assert_eq!(
        reading.compaction.activation_id,
        reading.projection_head.activation_id
    );
}

pub fn observation_projection_input(
    store: &SqliteConversationStore,
    id: &str,
) -> crate::projection::ProjectionCommitInput {
    let candidate =
        crate::sqlite::test_support::checkpoint_fixture(store, id, 1, &"\u{1}".repeat(130_000));
    let mut input = crate::sqlite::test_support::checkpoint_commit_fixture(store, candidate, None);
    // Exercise a large opaque model field while remaining within the native
    // checkpoint occupancy record's 1 MiB admission/read budget.
    input.occupancy = Some(occupancy_origin_native(
        &"original-model".repeat(20_000),
        200,
    ));
    input
}

pub fn occupancy_origin_compact(
    store: &SqliteConversationStore,
    predecessor: &str,
    summary: &str,
    occupancy: &rustx_types::context::occupancy::ContextOccupancy,
) -> RuntimeEventEnvelope {
    let _ = predecessor;
    let history = store.projected_history().unwrap();
    let candidate = crate::sqlite::test_support::checkpoint_fixture(
        store,
        summary,
        history.messages.len(),
        "native summary",
    );
    let mut input = crate::sqlite::test_support::checkpoint_commit_fixture(store, candidate, None);
    input.tokens_before = TokenMeasurement {
        input_tokens: 400,
        source: TokenMeasurementSource::Estimated,
    };
    input.estimated_tokens_after = occupancy.input_tokens;
    input.occupancy = Some(occupancy.clone());
    store.commit_projection(input).unwrap().completed
}

pub fn occupancy_origin_native(
    model: &str,
    input_tokens: u64,
) -> rustx_types::context::occupancy::ContextOccupancy {
    rustx_types::context::occupancy::ContextOccupancy {
        estimated: true,
        input_tokens,
        context_window_tokens: 8_192,
        model: model.into(),
        breakdown: rustx_types::context::occupancy::ContextBreakdown {
            system_tokens: 7,
            tool_tokens: 11,
            message_tokens: input_tokens - 18,
        },
    }
}

// Exact immutable native occupancy origins, independent of newer display readings.
pub fn occupancy_origin_cut(
    store: &SqliteConversationStore,
) -> crate::observation::cut::CheckpointReadCut {
    let source = store.observation_read_cut().unwrap();
    crate::observation::cut::CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        conversation_id: store.conversation_id().clone(),
        activation_id: None,
        revision: source.read.journal,
        through: source.read.journal,
        source_cut: Some(source),
        domain_revisions: crate::observation::metadata::MetadataFragment::default(),
        collection_cuts: crate::observation::metadata::MetadataFragment::default(),
    }
}

pub fn statistics_inherited_seed() -> LineageSeed {
    let inherited = crate::reading::TurnReadingProvenance {
        id: crate::reading::ConversationTurnId {
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000021"),
            attempt_id: AttemptId::new("inherited-statistics"),
        },
        process_message_ids: vec![MessageId::new("inherited-stats-answer")],
        preceding_message_id: None,
        prompt_message_id: Some(MessageId::new("inherited-stats-input")),
        outcome: crate::reading::InheritedTurnOutcome::Completed,
        started_at: None,
        ended_at: None,
        execution: Some(crate::reading::TurnExecution {
            model_requests: 9,
            ..Default::default()
        }),
    };
    LineageSeed::history(vec![
        user_message("inherited-stats-input", "original"),
        assistant_message("inherited-stats-answer", "answer"),
    ])
    .with_turns(vec![inherited])
    .unwrap()
}

pub fn inherited_seed(count: usize, measured_last: bool) -> LineageSeed {
    use crate::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    let mut canonical = Vec::new();
    let mut history = Vec::new();
    let mut turns = Vec::new();
    for index in 0..count {
        let id = MessageId::new(format!("inherited-{index}"));
        canonical.push(assistant_message(id.as_str(), "historical content"));
        history.push(SurfaceOp::Append {
            message_id: id.clone(),
        });
        turns.push(TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: ConversationId::new("conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73"),
                attempt_id: AttemptId::new(format!("attempt-{index}")),
            },
            process_message_ids: vec![id.clone()],
            preceding_message_id: None,
            prompt_message_id: None,
            outcome: InheritedTurnOutcome::Completed,
            started_at: None,
            ended_at: None,
            execution: Some(TurnExecution {
                model_requests: 1,
                occupancy: (index + 1 < count || measured_last).then(|| {
                    rustx_types::context::occupancy::ContextOccupancy {
                        estimated: true,
                        input_tokens: 12,
                        context_window_tokens: 4096,
                        model: "historical/model".into(),
                        breakdown: rustx_types::context::occupancy::ContextBreakdown {
                            system_tokens: 0,
                            tool_tokens: 0,
                            message_tokens: 12,
                        },
                    }
                }),
                ..Default::default()
            }),
        });
    }
    LineageSeed::replayed(canonical, history)
        .unwrap()
        .with_turns(turns)
        .unwrap()
}

// Fixed native read evidence does not replace original paired membership.
pub fn deny_selected_original_columns(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
        } if matches!(
            (table_name, column_name),
            ("message_ledger", "message_id" | "message_json")
                | ("inherited_message_membership", "assistant_message_id")
                | (
                    "inherited_turn_provenance" | "inherited_response_provenance",
                    "payload"
                )
        ) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

pub fn deny_trace_original_columns(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
        } if matches!(
            (table_name, column_name),
            (
                "events",
                "event_json" | "attempt_id" | "turn_id" | "event_id"
            ) | ("request_snapshots", "previous_started_sequence")
                | ("request_snapshot_payloads", "snapshot_json")
                | ("request_owners", "request_id")
                | ("message_ledger", "message_id" | "message_json")
        ) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

pub fn trace_arguments_reference(
    fields: &[crate::observation::trace::TraceFieldSource],
) -> rustx_types::observation::content::ContentRef {
    use crate::observation::trace::{TraceFieldKind, TraceFieldSource};
    (fields
        .iter()
        .find_map(|field| match field {
            TraceFieldSource::Content {
                field: TraceFieldKind::Arguments,
                binding,
            } => Some(binding.content_ref.clone()),
            _ => None,
        })
        .expect("original arguments field"))
    .as_ref()
    .clone()
}

pub fn projection_receipt_witness(
    receipt: &crate::projection::ProjectionCommitReceipt,
) -> &crate::observation::ObservationEventWitness {
    let batch = receipt.completed.observation.native_batch().unwrap();
    assert_eq!(batch.read_cut.read.journal, receipt.completed.sequence);
    let crate::observation::NativeObservationTransition::Single(witness) = &batch.transition else {
        panic!("original completion must carry one native witness");
    };
    assert_eq!(witness.sequence, receipt.completed.sequence);
    witness
}
