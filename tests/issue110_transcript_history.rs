//! Issue #110 (FND-05) — durable transcript history and Runtime Client paging.
//!
//! The transcript is a derived read model. These tests deliberately build
//! durable prefixes through the existing Message Ledger, Pending Inbound,
//! publication, and Event Journal owners, then read the bounded transcript
//! page instead of introducing a second history fixture. Every test uses
//! fixed data and explicit durable boundaries; none depends on sleeping.

#![allow(clippy::too_many_lines)]

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::conversation::{SurfaceRevision, SurfaceSpan};
use rustx::durable::{
    CompactionCommitInput, ConversationStore, ConversationStoreError, InboundDraft,
    SqliteConversationStore, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT, TranscriptCursor, TranscriptItem,
};
use rustx::events::interaction::{
    InteractionSettlement, InteractionSubject, OptionSpecification, QuestionSpecification,
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse,
    QuestionnaireSpecification, QuestionnaireSubmission, SingleOptionAnswer,
};
use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx::local_runtime::composition::{
    HeadlessConversationRuntime, LocalConversationRuntime, LocalRuntimeDependencies,
    LocalRuntimePaths,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantContentBlock,
    AssistantMessageBlock, CompactionSummaryMetadata, ContextKind, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelFinishReason, ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams,
    RequestSnapshot,
};
use rustx::publication::{
    PublicationAuditBlock, PublicationAuditKind, PublicationFrame, PublicationPayload,
    PublicationStreamStart,
};
use rustx::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, InteractionId, MessageId,
    PublicationStreamId, RequestId, ToolCallId, ToolId, TurnId,
};
use rustx::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use rustx::runtime::{InteractionResponse, RuntimeResourceRevision};
use rustx::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientResult};
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

const CONVERSATION: &str = "conv-fnd05";

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0)
        .single()
        .expect("valid fixed time")
}

fn conversation_id() -> ConversationId {
    ConversationId::new(CONVERSATION)
}

fn attempt() -> AttemptId {
    AttemptId::new("attempt-fnd05")
}

fn user_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(fixed_time()),
    })
}

fn summary_message(id: &str, text: &str) -> UserMessageBlock {
    UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
        timestamp: None,
    }
}

fn assistant_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
    })
}

fn tool_call(call_id: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-read"),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"file_path": ".agents/skills/read/SKILL.md"}),
    }
}

fn assistant_with_call(message_id: &str, call: ToolCall) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(message_id),
        content: vec![AssistantContentBlock::ToolCall(call)],
    })
}

fn tool_result(message_id: &str, call_id: &str, body: &str) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(message_id),
        tool_call_id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-read"),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![ToolResultContent::Text(TextBlock {
                text: body.to_owned(),
            })],
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            managed_output: Some(rustx::tools::types::ManagedOutputContinuation::Complete {
                locator: "/private/tool-output/results/result_1.txt".into(),
            }),
        },
    })
}

fn inbound_draft(id: &str, text: &str) -> InboundDraft {
    InboundDraft {
        message_id: Some(MessageId::new(id)),
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        timestamp: fixed_time(),
        correlation: None,
    }
}

fn message_id(message: &MessageBlock) -> &MessageId {
    match message {
        MessageBlock::User(message) => &message.id,
        MessageBlock::Assistant(message) => &message.id,
        MessageBlock::Tool(message) => &message.id,
    }
}

fn page_message_ids(page: &rustx::durable::TranscriptPage) -> Vec<String> {
    page.entries
        .iter()
        .filter_map(|entry| match &entry.item {
            TranscriptItem::Message { message } => Some(message_id(message).as_str().to_owned()),
            TranscriptItem::PublicationAudit { .. }
            | TranscriptItem::InteractionRequested { .. }
            | TranscriptItem::InteractionSettled { .. } => None,
        })
        .collect()
}

fn client_page_message_ids(
    page: &rustx::runtime_client::RuntimeClientTranscriptPage,
) -> Vec<String> {
    page.entries
        .iter()
        .filter_map(|entry| match &entry.item {
            rustx::runtime_client::RuntimeClientTranscriptItem::Message { message } => {
                Some(message_id(message).as_str().to_owned())
            }
            rustx::runtime_client::RuntimeClientTranscriptItem::PublicationAudit { .. }
            | rustx::runtime_client::RuntimeClientTranscriptItem::InteractionRequested { .. }
            | rustx::runtime_client::RuntimeClientTranscriptItem::InteractionSettled { .. } => None,
        })
        .collect()
}

fn all_entries(
    store: &SqliteConversationStore,
    page_limit: usize,
) -> Vec<rustx::durable::TranscriptEntry> {
    let mut before = None;
    let mut entries = Vec::new();
    loop {
        let page = store
            .load_transcript_page(before, page_limit)
            .expect("transcript page");
        if page.entries.is_empty() {
            break;
        }
        before = page.next_cursor;
        entries.extend(page.entries);
        if before.is_none() {
            break;
        }
    }
    entries
}

fn start_request(store: &SqliteConversationStore, turn: &str) -> RequestId {
    let head = store.load_head().expect("head");
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new(turn),
            retry_number: 0,
        },
        head.revision,
        format!("system prompt for {turn}"),
        Vec::new(),
        RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "model-x".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        64_000,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    let request_id = snapshot.request_id.clone();
    store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .expect("request start");
    request_id
}

fn envelope(event_id: &str, turn: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id(),
        attempt_id: Some(attempt()),
        turn_id: Some(TurnId::new(turn)),
        timestamp: fixed_time(),
        event,
    }
}

fn open_publication(
    store: &SqliteConversationStore,
    turn: &str,
) -> (RequestId, PublicationStreamStart) {
    let request_id = start_request(store, turn);
    let message_id = MessageId::new(format!("{}-agent-{turn}", attempt()));
    let start = PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(&attempt(), &message_id),
        attempt_id: attempt(),
        turn_id: TurnId::new(turn),
        request_id: request_id.clone(),
        message_id,
    };
    store
        .open_publication_stream(&start)
        .expect("open publication");
    (request_id, start)
}

fn frame(
    start: &PublicationStreamStart,
    sequence: u64,
    payload: PublicationPayload,
) -> PublicationFrame {
    PublicationFrame {
        stream_id: start.stream_id.clone(),
        message_id: start.message_id.clone(),
        sequence,
        payload,
    }
}

fn text_frame(start: &PublicationStreamStart, sequence: u64, text: &str) -> PublicationFrame {
    frame(
        start,
        sequence,
        PublicationPayload::TextSuffix {
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            suffix: text.to_owned(),
        },
    )
}

fn proposal_frames(start: &PublicationStreamStart) -> [PublicationFrame; 2] {
    let call = tool_call("call-proposed");
    [
        frame(
            start,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                call: rustx::tools::types::ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        ),
        frame(
            start,
            1,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                call,
            },
        ),
    ]
}

fn requested_interaction(interaction_id: &InteractionId) -> RuntimeEventEnvelope {
    envelope(
        &format!("interaction-requested-event:{interaction_id}"),
        "interaction",
        RuntimeEvent::InteractionRequested {
            interaction_id: interaction_id.clone(),
            subject: InteractionSubject::Questionnaire {
                questionnaire: QuestionnaireSpecification {
                    questions: vec![QuestionSpecification {
                        question: "Which environment?".to_owned(),
                        header: "Environment".to_owned(),
                        options: vec![
                            OptionSpecification {
                                label: "staging".to_owned(),
                                description: "A safe test environment.".to_owned(),
                                preview: None,
                            },
                            OptionSpecification {
                                label: "production".to_owned(),
                                description: "The live environment.".to_owned(),
                                preview: None,
                            },
                        ],
                        multi_select: false,
                    }],
                },
            },
        },
    )
}

fn settled_interaction(interaction_id: &InteractionId) -> RuntimeEventEnvelope {
    envelope(
        &format!("interaction-settled-event:{interaction_id}"),
        "interaction",
        RuntimeEvent::InteractionSettled {
            interaction_id: interaction_id.clone(),
            settlement: InteractionSettlement::QuestionnaireSubmitted {
                submission: QuestionnaireSubmission {
                    answers: vec![QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                            label: "staging".to_owned(),
                        }),
                    }],
                },
            },
        },
    )
}

// ---------------------------------------------------------------------------
// Requirements 1–4: durable acceptance, compaction history, bootstrap, pages
// ---------------------------------------------------------------------------

/// Requirement 1: a user item becomes transcript-visible at durable
/// acceptance, never at a client-local display operation.
#[test]
fn requirement_01_acceptance_is_the_transcript_visibility_frontier() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    assert!(
        store
            .load_transcript_page(None, 10)
            .expect("empty page")
            .entries
            .is_empty()
    );

    let accepted = store
        .accept_inbound(inbound_draft("user-1", "durable first"))
        .expect("accept inbound");
    let page = store
        .load_transcript_page(None, 10)
        .expect("page after acceptance");
    assert_eq!(page_message_ids(&page), vec![accepted.message_id.as_str()]);
    assert_eq!(store.load_pending().expect("pending").len(), 1);

    let rejected = store.accept_inbound(InboundDraft {
        content: Vec::new(),
        ..inbound_draft("user-rejected", "never visible")
    });
    assert!(matches!(
        rejected,
        Err(ConversationStoreError::EmptyContent)
    ));
    assert_eq!(
        page_message_ids(&store.load_transcript_page(None, 10).expect("page")),
        vec!["user-1".to_owned()]
    );
}

/// Requirement 2: compaction retires messages from the Surface but does not
/// delete their durable transcript entries.
#[test]
fn requirement_02_compacted_history_remains_pageable_after_surface_replacement() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store
        .initialize(&[user_message("user-a", "a"), user_message("user-b", "b")])
        .expect("initialize");
    let summary = summary_message("summary-1", "summary");
    store
        .commit_compaction(CompactionCommitInput {
            summary: summary.clone(),
            span: SurfaceSpan::new(MessageId::new("user-a"), MessageId::new("user-b")),
            expected_revision: SurfaceRevision::new(2),
            tokens_before: TokenMeasurement {
                input_tokens: 20,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 4,
            attempt_id: None,
            turn_id: None,
            timestamp: fixed_time(),
        })
        .expect("compaction");

    assert_eq!(
        store.load_head().expect("head").active_message_ids,
        vec![MessageId::new("summary-1")]
    );
    assert_eq!(
        page_message_ids(&store.load_transcript_page(None, 10).expect("transcript")),
        vec!["user-a", "user-b", "summary-1"]
    );
    assert_eq!(
        store
            .load_canonical_page(None, 10)
            .expect("canonical page")
            .messages
            .len(),
        3
    );
}

/// Requirement 3: the no-cursor read is a bounded bootstrap page, not a
/// complete conversation materialization.
#[test]
fn requirement_03_bootstrap_page_is_bounded() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    let messages = (0..(TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT + 6))
        .map(|index| user_message(&format!("user-{index:02}"), "history"))
        .collect::<Vec<_>>();
    store.initialize(&messages).expect("initialize");

    let page = store
        .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
        .expect("bootstrap page");
    assert_eq!(page.entries.len(), TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT);
    assert_eq!(page_message_ids(&page).first(), Some(&"user-06".to_owned()));
    assert_eq!(page_message_ids(&page).last(), Some(&"user-69".to_owned()));
    assert!(page.next_cursor.is_some(), "older history needs a cursor");
}

/// Requirement 4: the exclusive cursor walks older rows in stable order and
/// appends after the first read cannot create duplicates or gaps.
#[test]
fn requirement_04_older_pages_have_stable_order_without_duplicates_or_gaps() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    let messages = (0..9)
        .map(|index| user_message(&format!("user-{index}"), "history"))
        .collect::<Vec<_>>();
    store.initialize(&messages).expect("initialize");

    let newest = store.load_transcript_page(None, 3).expect("newest page");
    let before = newest.next_cursor.expect("older cursor");
    store
        .append_canonical(&user_message("user-9", "new append"))
        .expect("append after page");
    let older = store
        .load_transcript_page(Some(before), 3)
        .expect("older page");
    let oldest = store
        .load_transcript_page(older.next_cursor, 3)
        .expect("oldest page");

    let mut ids = page_message_ids(&oldest);
    ids.extend(page_message_ids(&older));
    ids.extend(page_message_ids(&newest));
    assert_eq!(
        ids,
        (0..9)
            .map(|index| format!("user-{index}"))
            .collect::<Vec<_>>()
    );
    let unique = ids.iter().collect::<HashSet<_>>();
    assert_eq!(unique.len(), ids.len());
}

// ---------------------------------------------------------------------------
// Requirements 5–6 and 12–14: Runtime Client boundaries and resources
// ---------------------------------------------------------------------------

const MODELS_JSON: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "https://local.issue110.invalid/v1",
      "apiKey": "$RUSTX_ISSUE110_KEY",
      "models": [{
        "id": "issue110-model",
        "protocol": "openai_chat_completions",
        "contextWindow": 128000,
        "maxOutputTokens": 4096,
        "capabilities": {
          "inputModalities": ["text"],
          "outputModalities": ["text"],
          "toolCalls": true,
          "reasoning": false
        },
        "compat": {"chatReasoningReplay": "omit"}
      }]
    }
  }
}"#;

const RUNTIME_CONFIG_JSON: &str = r#"{
  "agentId": "agent-issue110",
  "model": {"model": "local/issue110-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192}
}"#;

fn startup(root: &Path) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let models = root.join("models.jsonc");
    let config = root.join("rustx.jsonc");
    std::fs::write(&models, MODELS_JSON).expect("models.jsonc");
    std::fs::write(&config, RUNTIME_CONFIG_JSON).expect("rustx.jsonc");
    LocalRuntimePaths {
        models,
        config,
        skill_paths: Vec::new(),
        no_skills: false,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.join("private"),
    }
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE110_KEY".to_owned(),
            "issue110-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

fn seed_composed_store(paths: &LocalRuntimePaths, messages: &[MessageBlock]) {
    let artifacts = paths.artifacts_root();
    std::fs::create_dir_all(&artifacts).expect("artifact root");
    let store = SqliteConversationStore::open(
        ConversationId::new("conversation-standalone"),
        &artifacts.join("conversation.sqlite"),
    )
    .expect("seed store");
    store.initialize(messages).expect("seed history");
}

fn initialized_snapshot(
    result: RuntimeClientResult,
) -> rustx::runtime_client::RuntimeClientSnapshot {
    let RuntimeClientResult::Initialized { snapshot, .. } = result else {
        panic!("expected initialized result, got {result:?}");
    };
    snapshot
}

/// Requirement 5: detaching and reattaching the Runtime Client does not
/// replace or reset the durable transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_05_detach_and_reattach_reads_the_same_durable_transcript() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    seed_composed_store(&paths, &[user_message("seed-user", "persisted")]);
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("interactive composition");

    let (first, first_result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("first attach");
    let first_snapshot = initialized_snapshot(first_result);
    runtime.host().detach(first.attachment_id());
    let (_second, second_result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("reattach");
    let second_snapshot = initialized_snapshot(second_result);
    assert_eq!(first_snapshot.transcript, second_snapshot.transcript);
    assert_eq!(
        page_message_ids(
            &runtime
                .runtime()
                .transcript_page(None, 64)
                .expect("runtime page")
        ),
        vec!["seed-user".to_owned()]
    );
}

/// Requirement 6: a headless runtime can commit/read history with zero
/// clients, and a later interactive composition reads the same durable page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_06_headless_history_is_available_to_a_later_client() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    seed_composed_store(&paths, &[]);
    {
        let headless = HeadlessConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect("headless composition");
        assert!(!headless.tool_runtime().is_runtime_client_bound());
        let accepted = headless
            .runtime()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: "accepted while headless".to_owned(),
            })])
            .expect("headless inbound acceptance");
        let page = headless
            .runtime()
            .transcript_page(None, 64)
            .expect("headless page");
        assert_eq!(page_message_ids(&page), vec![accepted.message_id.as_str()]);
    }

    let interactive = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("interactive reopen");
    let (_, result) = interactive
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("later attach");
    assert_eq!(
        client_page_message_ids(&initialized_snapshot(result).transcript),
        vec!["conversation-standalone-inbound-1"]
    );
}

/// Requirement 7: Agent Status remains durable model context but is absent
/// from the normal transcript visibility class.
#[test]
fn requirement_07_agent_status_is_excluded_from_normal_transcript() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let accepted_context = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("status-1")),
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::AgentStatus(
                AgentStatusGenerationMetadata::new(fixed_time(), vec![AgentStatusModuleId::Time])
                    .expect("valid Agent Status metadata"),
            )),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "internal status".to_owned(),
            })],
            timestamp: fixed_time(),
            correlation: None,
        })
        .expect("status context acceptance");
    assert!(
        store
            .load_transcript_page(None, 10)
            .expect("page before status adoption")
            .entries
            .is_empty()
    );
    store
        .adopt_pending_batch(
            accepted_context.sequence,
            adoption_of(&store, accepted_context.sequence),
        )
        .expect("status context adoption");
    store
        .append_canonical(&user_message("visible-1", "visible"))
        .expect("visible message");
    let canonical = store.load_canonical().expect("canonical");
    assert!(
        canonical
            .iter()
            .any(|message| message_id(message).as_str() == "status-1")
    );
    assert_eq!(
        page_message_ids(&store.load_transcript_page(None, 10).expect("page")),
        vec!["visible-1".to_owned()]
    );
}

/// Requirement 8: an incomplete publication is a typed noncanonical item,
/// not a canonical Assistant message.
#[test]
fn requirement_08_incomplete_publication_is_distinct_from_canonical_assistant() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (_request_id, start) = open_publication(&store, "incomplete");
    store
        .stage_publication_frames(&[text_frame(&start, 0, "partial")])
        .expect("stage");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("incomplete audit");
    assert_eq!(audit.0.kind, PublicationAuditKind::Incomplete);
    store
        .append_canonical(&assistant_message("canonical-assistant", "canonical"))
        .expect("canonical Assistant");

    let entries = all_entries(&store, 10);
    assert!(entries.iter().any(|entry| {
        matches!(
            &entry.item,
            TranscriptItem::PublicationAudit { audit } if audit.kind == PublicationAuditKind::Incomplete
        )
    }));
    assert!(entries.iter().any(|entry| {
        matches!(
            &entry.item,
            TranscriptItem::Message { message: MessageBlock::Assistant(assistant) }
                if assistant.id.as_str() == "canonical-assistant"
        )
    }));
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry.item, TranscriptItem::Message { .. }))
            .count(),
        1
    );
}

/// Requirement 9: complete-but-unaccepted and incomplete publications remain
/// distinct audit settlements and neither becomes a canonical Assistant.
#[test]
fn requirement_09_unaccepted_and_incomplete_publications_stay_distinct() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");

    let (_request_id, incomplete) = open_publication(&store, "incomplete-2");
    store
        .stage_publication_frames(&[text_frame(&incomplete, 0, "partial")])
        .expect("stage incomplete");
    store
        .terminalize_publication_audit(&incomplete.stream_id, fixed_time())
        .expect("incomplete audit");

    let (request_id, unaccepted) = open_publication(&store, "unaccepted");
    store
        .stage_publication_frames(&[text_frame(&unaccepted, 0, "complete")])
        .expect("stage unaccepted");
    store
        .append_event(envelope(
            "provider-complete-unaccepted",
            "unaccepted",
            RuntimeEvent::ModelRequestCompleted {
                request_id,
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ))
        .expect("provider outcome");
    store
        .commit_publication_terminal(
            &unaccepted.stream_id,
            &[frame(&unaccepted, 1, PublicationPayload::TerminalOnly)],
        )
        .expect("publication terminal");
    store
        .terminalize_publication_audit(&unaccepted.stream_id, fixed_time())
        .expect("unaccepted audit");

    let audits = all_entries(&store, 10)
        .into_iter()
        .filter_map(|entry| match entry.item {
            TranscriptItem::PublicationAudit { audit } => Some(audit),
            TranscriptItem::Message { .. }
            | TranscriptItem::InteractionRequested { .. }
            | TranscriptItem::InteractionSettled { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        audits.iter().map(|audit| audit.kind).collect::<Vec<_>>(),
        vec![
            PublicationAuditKind::Incomplete,
            PublicationAuditKind::Unaccepted,
        ]
    );
    assert!(store.load_canonical().expect("canonical").is_empty());
}

/// Requirement 10: a publication-audit tool proposal is typed audit content
/// and has no Tool Plane execution fact or canonical `ToolResult`.
#[test]
fn requirement_10_audited_tool_proposal_is_unaccepted_and_unexecuted() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (_request_id, start) = open_publication(&store, "proposal-audit");
    store
        .stage_publication_frames(&proposal_frames(&start))
        .expect("proposal frames");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("incomplete proposal audit");
    assert_eq!(audit.0.kind, PublicationAuditKind::Incomplete);
    assert!(matches!(
        audit.0.content.as_slice(),
        [PublicationAuditBlock::ProposedToolCall { complete: true, .. }]
    ));
    assert!(
        store
            .read_events(None, 128)
            .expect("events")
            .events
            .iter()
            .all(|event| !matches!(event.event, RuntimeEvent::ToolExecutionStarted { .. }))
    );
    assert!(store.load_canonical().expect("canonical").is_empty());
}

/// Requirement 11: requested and settled interaction facts are historical
/// transcript entries, while no pending waiter is reconstructed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_11_interaction_audits_page_without_recovering_a_waiter() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    seed_composed_store(&paths, &[]);
    let store = SqliteConversationStore::open(
        ConversationId::new("conversation-standalone"),
        &paths.artifacts_root().join("conversation.sqlite"),
    )
    .expect("reopen seeded store");
    let interaction_id = InteractionId::for_attempt(&attempt(), 1);
    let mut requested = requested_interaction(&interaction_id);
    requested.conversation_id = ConversationId::new("conversation-standalone");
    store
        .append_interaction_audit(requested)
        .expect("requested audit");
    let mut settled = settled_interaction(&interaction_id);
    settled.conversation_id = ConversationId::new("conversation-standalone");
    store
        .append_interaction_audit(settled)
        .expect("settled audit");

    drop(store);

    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("cold reopen");
    let (attachment, result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after audit recovery");
    let snapshot = initialized_snapshot(result);
    assert!(snapshot.pending_interactions.is_empty());
    assert!(snapshot.inbound.pending.is_empty());
    let page = runtime
        .host()
        .transcript_page(None, 10)
        .expect("historical audit page");
    assert!(matches!(
        page.entries[0].item,
        rustx::runtime_client::RuntimeClientTranscriptItem::InteractionRequested { .. }
    ));
    assert!(matches!(
        page.entries[1].item,
        rustx::runtime_client::RuntimeClientTranscriptItem::InteractionSettled { .. }
    ));
    assert!(matches!(
        runtime.host().respond_interaction(
            &interaction_id,
            InteractionResponse::Questionnaire {
                response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                    answers: vec![QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                            label: "staging".to_owned(),
                        }),
                    }],
                }),
            },
        ),
        Err(rustx::runtime_client::RuntimeClientError::InteractionNotPending { .. })
    ));
    drop(attachment);
}

/// The generic Event Journal append is not allowed to create an interaction
/// transcript fact without returning its durable cursor. The dedicated audit
/// transition owns both the Journal write and the transcript-order allocation.
#[test]
fn interaction_audit_transition_is_the_only_cursor_returning_interaction_path() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let interaction_id = InteractionId::for_attempt(&attempt(), 1);
    let requested = requested_interaction(&interaction_id);
    let requested_event_id = requested.event_id.clone();

    assert!(matches!(
        store.append_event(requested.clone()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(
        store
            .read_events(None, 10)
            .expect("Journal after rejected request")
            .events
            .is_empty()
    );
    assert!(
        store
            .load_transcript_page(None, 10)
            .expect("transcript after rejected request")
            .entries
            .is_empty()
    );

    let (persisted_requested, requested_cursor) = store
        .append_interaction_audit(requested)
        .expect("specialized request transition");
    assert_eq!(persisted_requested.event_id, requested_event_id);
    let requested_page = store
        .load_transcript_page(None, 10)
        .expect("requested transcript page");
    assert_eq!(requested_page.entries.len(), 1);
    assert_eq!(requested_page.entries[0].cursor, requested_cursor);
    assert!(matches!(
        requested_page.entries[0].item,
        TranscriptItem::InteractionRequested { .. }
    ));

    let settled = settled_interaction(&interaction_id);
    assert!(matches!(
        store.append_event(settled.clone()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store
            .read_events(None, 10)
            .expect("Journal after rejected settlement")
            .events
            .len(),
        1
    );
    assert_eq!(
        store
            .load_transcript_page(None, 10)
            .expect("transcript after rejected settlement")
            .entries
            .len(),
        1
    );

    let (persisted_settled, settled_cursor) = store
        .append_interaction_audit(settled)
        .expect("specialized settlement transition");
    assert!(persisted_settled.event_id.as_str().contains("settled"));
    assert!(requested_cursor < settled_cursor);
    let page = store
        .load_transcript_page(None, 10)
        .expect("settled transcript page");
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.cursor)
            .collect::<Vec<_>>(),
        vec![requested_cursor, settled_cursor]
    );
    assert!(matches!(
        page.entries[1].item,
        TranscriptItem::InteractionSettled { .. }
    ));

    assert!(matches!(
        store.append_interaction_audit(requested_interaction(&interaction_id)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(matches!(
        store.append_interaction_audit(settled_interaction(&interaction_id)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert_eq!(
        store
            .read_events(None, 10)
            .expect("final Journal")
            .events
            .len(),
        2,
        "the specialized lifecycle remains exactly once"
    );
}

/// Requirement 12: transcript cursors are independent from the live Runtime
/// Client event cursor, and paging does not move the event cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_12_transcript_paging_preserves_runtime_client_cursor_invariants() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    seed_composed_store(
        &paths,
        &[
            user_message("cursor-1", "one"),
            user_message("cursor-2", "two"),
            user_message("cursor-3", "three"),
        ],
    );
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("interactive composition");
    let (attachment, _result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let live_before = runtime.host().snapshot().expect("snapshot").1;
    let newest_page = runtime
        .host()
        .transcript_page(None, 2)
        .expect("bounded newest transcript page");
    assert_eq!(
        client_page_message_ids(&newest_page),
        vec!["cursor-2", "cursor-3"]
    );
    let oldest_cursor = newest_page.next_cursor.expect("older page cursor");
    let older = runtime
        .host()
        .transcript_page(Some(oldest_cursor), 2)
        .expect("older transcript page");
    assert_eq!(client_page_message_ids(&older), vec!["cursor-1"]);
    let live_after = runtime.host().snapshot().expect("snapshot").1;
    assert_eq!(live_before, live_after);
    attachment.detach();
}

/// The ordering spine uses an explicit `(kind, id)` identity. The same
/// opaque string is therefore legal in the independent `MessageId`, `EventId`,
/// and `PublicationStreamId` domains, while Pending -> Ledger adoption keeps
/// one message reference and one cursor.
#[test]
fn transcript_reference_identity_is_typed_and_collision_free() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");

    let shared_id = "interaction-requested-event:collision";
    let accepted = store
        .accept_inbound(inbound_draft(shared_id, "same opaque id"))
        .expect("accepted message");
    let accepted_cursor = accepted.transcript_cursor.expect("accepted cursor");
    let batch = store
        .select_pending_batch()
        .expect("select pending")
        .expect("pending batch");
    store
        .adopt_pending_batch(batch.watermark, adoption_of(&store, batch.watermark))
        .expect("adopt message");

    let interaction = requested_interaction(&InteractionId::new("collision"));
    store
        .append_interaction_audit(interaction)
        .expect("EventId collision is scoped");

    let publication_turn = "typed-collision";
    let publication_message_id = MessageId::new(format!("{}-agent-{publication_turn}", attempt()));
    let publication_stream_id =
        PublicationStreamId::for_request(&attempt(), &publication_message_id);
    let publication_message = store
        .accept_inbound(inbound_draft(
            publication_stream_id.as_str(),
            "PublicationStreamId shares this opaque string",
        ))
        .expect("MessageId and PublicationStreamId collision is scoped");

    let request_id = start_request(&store, publication_turn);
    let start = PublicationStreamStart {
        stream_id: publication_stream_id.clone(),
        attempt_id: attempt(),
        turn_id: TurnId::new(publication_turn),
        request_id,
        message_id: publication_message_id,
    };
    store
        .open_publication_stream(&start)
        .expect("PublicationStreamId collision is scoped");
    let (_, publication_cursor) = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("publication audit");

    let entries = all_entries(&store, 10);
    let cursors = entries
        .iter()
        .map(|entry| entry.cursor.get())
        .collect::<Vec<_>>();
    assert_eq!(
        cursors,
        vec![
            accepted_cursor.get(),
            2,
            publication_message
                .transcript_cursor
                .expect("publication collision message cursor")
                .get(),
            publication_cursor.get(),
        ]
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry.item, TranscriptItem::Message { .. }))
            .count(),
        2
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.item, TranscriptItem::InteractionRequested { .. }))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.item, TranscriptItem::PublicationAudit { .. }))
    );
    assert_eq!(
        store
            .load_transcript_page(None, 10)
            .expect("typed collision page")
            .entries
            .iter()
            .filter(|entry| entry.cursor == accepted_cursor)
            .count(),
        1,
        "Pending -> Ledger adoption reuses the acceptance reference"
    );
    assert!(matches!(
        store.append_canonical(&user_message(shared_id, "duplicate")),
        Err(ConversationStoreError::DuplicateMessageId(_))
    ));
}

/// Requirement 13: resource reload changes future resource state only; it
/// cannot append a transcript item or rewrite an existing one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_13_resource_reload_has_no_transcript_item_or_diff() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    seed_composed_store(&paths, &[user_message("reload-user", "history")]);
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("interactive composition");
    let before = runtime
        .host()
        .transcript_page(None, 64)
        .expect("before page");
    runtime
        .host()
        .reload_resources()
        .await
        .expect("reload resources");
    let after = runtime
        .host()
        .transcript_page(None, 64)
        .expect("after page");
    assert_eq!(before, after);
}

/// Requirement 14: a cold reopen retains transcript pages while startup
/// resource discovery observes the new workspace generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requirement_14_cold_reopen_keeps_history_and_refreshes_resources() {
    let root = tempfile::tempdir().expect("root");
    let paths = startup(root.path());
    let skill = paths.workspace.join(".agents/skills/reopened");
    std::fs::create_dir_all(&skill).expect("skill directory");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: reopened\ndescription: first\n---\nfirst\n",
    )
    .expect("first skill");
    seed_composed_store(&paths, &[user_message("cold-user", "history")]);
    let first = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("first composition");
    let first_page = first.host().transcript_page(None, 64).expect("first page");
    drop(first);

    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: reopened\ndescription: second\n---\nsecond\n",
    )
    .expect("updated skill");
    let second = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("cold reopen");
    let second_page = second
        .host()
        .transcript_page(None, 64)
        .expect("reopened page");
    assert_eq!(first_page, second_page);
    let reopened_capabilities = second.capability().current_snapshot().skills().clone();
    let reopened_skill = reopened_capabilities
        .catalog_entries()
        .iter()
        .find(|entry| entry.name == "reopened")
        .expect("reopened Skill is discovered");
    assert_eq!(reopened_skill.description, "second");
}

// ---------------------------------------------------------------------------
// Requirements 15–16: independent durable owners and value paging
// ---------------------------------------------------------------------------

/// Requirement 15: an old Request Snapshot retains its System/resource bytes
/// even after the transcript has advanced and the current resource generation
/// would be different.
#[test]
fn requirement_15_old_request_snapshot_reconstructs_old_system_bytes() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store
        .initialize(&[user_message("snapshot-user", "history")])
        .expect("initialize");
    let head = store.load_head().expect("head");
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new("old-request"),
            retry_number: 0,
        },
        head.revision,
        "old frozen System bytes".to_owned(),
        Vec::new(),
        RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "old-model".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 64,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, false),
            compat: ModelCompat::default(),
        },
        1024,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .expect("old request start");
    store
        .append_canonical(&user_message("new-history", "later"))
        .expect("later history");

    let loaded = store
        .load_request_snapshot(&snapshot.request_id)
        .expect("snapshot");
    assert_eq!(loaded.effective_system_prompt, "old frozen System bytes");
    assert_eq!(
        loaded.runtime_resource_revision,
        RuntimeResourceRevision::new(1)
    );
    assert!(
        !store
            .load_transcript_page(None, 10)
            .expect("transcript")
            .entries
            .iter()
            .any(|entry| serde_json::to_string(&entry.item)
                .expect("item JSON")
                .contains("old frozen System bytes"))
    );
}

/// Requirement 16: a Read/SKILL.md `ToolResult` remains pageable by value after
/// the source file and its managed-output locator disappear.
#[test]
fn requirement_16_read_tool_result_is_pageable_after_source_disappears() {
    let directory = tempfile::tempdir().expect("workspace");
    let skill = directory.path().join(".agents/skills/read/SKILL.md");
    std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("skill directory");
    std::fs::write(&skill, "old skill body").expect("skill body");

    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    store
        .append_canonical(&assistant_with_call(
            "assistant-read",
            tool_call("call-read"),
        ))
        .expect("Assistant call");
    store
        .append_canonical(&tool_result(
            "tool-result-read",
            "call-read",
            "old skill body",
        ))
        .expect("ToolResult");
    std::fs::remove_file(skill).expect("remove source file");

    let page = store.load_transcript_page(None, 10).expect("page by value");
    let result = page.entries.iter().find_map(|entry| match &entry.item {
        TranscriptItem::Message {
            message: MessageBlock::Tool(tool),
        } => Some(tool),
        TranscriptItem::Message { .. }
        | TranscriptItem::PublicationAudit { .. }
        | TranscriptItem::InteractionRequested { .. }
        | TranscriptItem::InteractionSettled { .. } => None,
    });
    let result = result.expect("ToolResult remains in transcript");
    assert!(result.result.content.iter().any(|content| {
        matches!(content, ToolResultContent::Text(text) if text.text == "old skill body")
    }));
    assert!(result.result.managed_output.is_some());
}

/// The explicit cursor type is an exclusive durable position, not an offset
/// and not an Event Journal sequence. This small store-level assertion keeps
/// that contract visible beside the numbered paging regressions.
#[test]
fn transcript_cursor_is_a_stable_exclusive_position() {
    let cursor = TranscriptCursor::new(42);
    assert_eq!(cursor.get(), 42);
    assert_eq!(cursor.to_string(), "42");
}

/// The durable answer obligation of one adoption, built from exactly the
/// pending items the adoption transaction will consume.
fn adoption_of(
    store: &SqliteConversationStore,
    watermark: rustx::runtime::inbound::InboundSequence,
) -> rustx::events::types::RuntimeEventEnvelope {
    rustx::durable::inbox::inbound_adoption_event(
        store.conversation_id(),
        None,
        store
            .load_pending()
            .expect("pending")
            .into_iter()
            .filter(|item| item.sequence <= watermark)
            .map(|item| item.message_id)
            .collect(),
    )
}
