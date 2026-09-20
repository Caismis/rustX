//! Deterministic regressions for the native Trace inspection contract.
//!
//! Every scenario is built from real durable transitions on a real store, so
//! what is asserted is what the authorities actually recorded. Nothing here
//! sleeps or races: ordering is established by committing facts in order, and
//! read cuts are captured explicitly at the point the assertion is about.

use super::bounds::{
    TRACE_DETAIL_TEXT_BYTES, TRACE_PREVIEW_BYTES, TRACE_SUMMARY_CONTEXT,
    TRACE_SUMMARY_CONTEXT_BYTES, TraceText,
};
use super::record::{bound_record, generation_metrics};
use super::*;
use crate::context::assembly::ContextGeneration;
use crate::durable::SqliteConversationStore;
use crate::events::types::RuntimeEvent as E;
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, ContextKind, InboundKind,
    MessageBlock, ToolCallOccurrenceRef, ToolMessageBlock, UserContentBlock, UserMessageBlock,
    UserSource,
};
use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::finish::ModelFinishReason;
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::types::ModelUsage;
use crate::model::{
    ModelCapabilities, ModelCompat, ModelProtocol, RequestIdentity, RequestSnapshot,
};
use crate::runtime::identity::{
    ArtifactId, AttemptId, CapabilityRevision, CertifiedExtensionIdentity, ConversationId, EventId,
    MessageId, RequestId, ToolCallId, ToolId, TurnId,
};
use crate::tools::types::{
    ModelToolDefinition, ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
};
use chrono::{DateTime, TimeZone, Utc};

// ---------------------------------------------------------------------------
// Fixture vocabulary
// ---------------------------------------------------------------------------

/// Values that must never cross the inspection boundary. Each names the
/// infrastructure authority it belongs to, so a failure says what leaked.
const PROVIDER_CREDENTIAL: &str = "sk-live-provider-credential";
const MCP_CREDENTIAL: &str = "mcp-server-bearer-credential";
const EXECUTOR_ENVIRONMENT: &str = "EXECUTOR_SECRET_ENVIRONMENT_VALUE";
const PROVIDER_CONTINUATION: &str = "opaque-provider-continuation-state";
const MANAGED_OUTPUT_LOCATOR: &str = "/var/rustx/managed-output/private-store";

/// Session-owned facts an inspector legitimately needs to see.
const SYSTEM_PROMPT: &str = "You are the historical rustX agent under inspection.";
const TOOL_DESCRIPTION: &str = "Run one non-interactive command inside the workspace.";

fn timestamp(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
}

fn store(id: &str) -> SqliteConversationStore {
    let store = SqliteConversationStore::in_memory(ConversationId::new(id)).unwrap();
    store.initialize(&[]).unwrap();
    store
}

fn event(store: &dyn ConversationStore, kind: E, seconds: i64) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: 1,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(AttemptId::new("attempt-a")),
        turn_id: Some(TurnId::new("1")),
        timestamp: timestamp(seconds),
        event: kind,
    }
}

fn append(store: &dyn ConversationStore, kind: E, seconds: i64) {
    let envelope = event(store, kind, seconds);
    match &envelope.event {
        E::BackgroundExecutionCommitted { .. } => {
            crate::durable::inbox::ConversationInboundCapability::commit_background_ownership(
                store, envelope,
            )
            .unwrap();
        }
        E::BackgroundTerminalPublished {
            message_id,
            execution_id,
            ..
        } => {
            store
                .accept_inbound_with_event(
                    crate::durable::InboundDraft {
                        message_id: Some(message_id.clone()),
                        source: crate::message::types::UserSource::Runtime,
                        kind: crate::message::types::InboundKind::Message,
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "settled".into(),
                        })],
                        timestamp: timestamp(seconds),
                        correlation: Some(format!("background:{execution_id}")),
                    },
                    envelope,
                )
                .unwrap();
        }
        _ => {
            store.append_event(envelope).unwrap();
        }
    }
}

/// Appends one fact inside an explicitly named logical Step, so a regression
/// can open a second Step instead of reusing the fixture's default one.
fn append_in_step(store: &dyn ConversationStore, kind: E, seconds: i64, turn: &str) {
    let mut envelope = event(store, kind, seconds);
    envelope.turn_id = Some(TurnId::new(turn));
    store.append_event(envelope).unwrap();
}

fn start(store: &dyn ConversationStore) {
    append(
        store,
        E::AttemptStarted {
            attempt_id: AttemptId::new("attempt-a"),
        },
        0,
    );
    append(store, E::TurnStarted, 1);
}

/// Commits one actual request whose frozen snapshot carries both legitimate
/// Session-owned input and every infrastructure secret Trace must withhold.
fn request(store: &dyn ConversationStore, retry: u32) -> RequestSnapshot {
    request_with_tools(store, retry, None)
}

fn request_with_tools(
    store: &dyn ConversationStore,
    retry: u32,
    tools: Option<Vec<ModelToolDefinition>>,
) -> RequestSnapshot {
    request_with_identity(
        store,
        retry,
        tools,
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-a"),
            turn: TurnId::new("1"),
            retry_number: retry,
        },
    )
}

fn request_with_identity(
    store: &dyn ConversationStore,
    retry: u32,
    tools: Option<Vec<ModelToolDefinition>>,
    identity: RequestIdentity,
) -> RequestSnapshot {
    commit_request(store, prepared_request(store, retry, tools, identity), &[])
}

/// Builds one frozen snapshot without committing it, so a regression can
/// state the exact immutable authority it is about — the frozen prompt or
/// the frozen request-scoped context identities — before the start commit.
fn prepared_request(
    store: &dyn ConversationStore,
    retry: u32,
    tools: Option<Vec<ModelToolDefinition>>,
    identity: RequestIdentity,
) -> RequestSnapshot {
    let mut snapshot = RequestSnapshot::new(
        identity,
        store.load_head().unwrap().revision,
        SYSTEM_PROMPT.to_owned(),
        vec![],
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: format!("historical-model-{retry}"),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 100,
            request_params: RequestParams::from_iter([
                ("temperature".into(), serde_json::json!(0.25)),
                ("api_key".into(), serde_json::json!(PROVIDER_CREDENTIAL)),
                (
                    "authorization".into(),
                    serde_json::json!(format!("Bearer {MCP_CREDENTIAL}")),
                ),
                (
                    "executor_env".into(),
                    serde_json::json!(EXECUTOR_ENVIRONMENT),
                ),
            ]),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        4096,
        None,
        false,
        vec![ModelToolDefinition {
            id: ToolId::new("tool-bash"),
            name: "bash".into(),
            description: TOOL_DESCRIPTION.to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
            }),
        }],
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: vec![],
        },
        Some(
            crate::runtime::continuation::ProviderContinuationState::Anthropic(
                crate::runtime::continuation::AnthropicContinuation {
                    opaque: serde_json::json!(PROVIDER_CONTINUATION),
                },
            ),
        ),
        vec![],
    );
    if let Some(tools) = tools {
        snapshot.tool_definitions = tools;
    }
    snapshot.continuation = Some(
        crate::runtime::continuation::ProviderContinuationState::Anthropic(
            crate::runtime::continuation::AnthropicContinuation {
                opaque: serde_json::json!(PROVIDER_CONTINUATION),
            },
        ),
    );
    snapshot
}

/// Commits one prepared request through the real durable start transition.
///
/// The store validates that `context` is exactly the ordered request-scoped
/// context the snapshot froze, so a fixture cannot manufacture a disagreement
/// between the two authorities this issue's Context path depends on.
fn commit_request(
    store: &dyn ConversationStore,
    snapshot: RequestSnapshot,
    context: &[MessageBlock],
) -> RequestSnapshot {
    store
        .commit_model_turn_start(
            context,
            &snapshot,
            timestamp(2 + i64::from(snapshot.identity.retry_number) * 2),
        )
        .unwrap();
    snapshot
}

fn failure(store: &dyn ConversationStore, request: &RequestSnapshot, kind: ModelErrorKind) {
    append(
        store,
        E::ModelRequestFailed {
            request_id: request.request_id.clone(),
            usage: None,
            error: ModelError {
                kind,
                message: "the provider rejected the request".into(),
                retry_disposition: ModelRetryDisposition::Transient,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
            generation: None,
        },
        3 + i64::from(request.identity.retry_number) * 2,
    );
}

fn completion(
    store: &dyn ConversationStore,
    request: &RequestSnapshot,
    usage: Option<ModelUsage>,
    generation: Option<GenerationEvidence>,
    seconds: i64,
) {
    let mut completed = event(
        store,
        E::ModelRequestCompleted {
            request_id: request.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage,
            generation,
        },
        seconds,
    );
    completed.attempt_id = Some(request.identity.attempt_id.clone());
    completed.turn_id = Some(request.identity.turn.clone());
    store.append_event(completed).unwrap();
}

fn page(store: &dyn ConversationStore) -> TracePage {
    TraceProjection::new(store)
        .unwrap()
        .page(None, TRACE_PAGE_LIMIT)
        .unwrap()
}

fn detail_of(store: &dyn ConversationStore, id: &str) -> TraceDetail {
    TraceProjection::new(store)
        .unwrap()
        .detail(id)
        .unwrap()
        .expect("detail for a loaded record")
}

fn record_of(page: &TracePage, kind: TraceKind) -> &TraceRecord {
    page.records
        .iter()
        .find(|record| record.kind == kind)
        .unwrap_or_else(|| panic!("a {kind:?} record"))
}

/// Commits one canonical Assistant message proposing one Tool call.
fn propose_tool_call(
    store: &dyn ConversationStore,
    message_id: &str,
    call: &ToolCall,
    seconds: i64,
) {
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(message_id),
                content: vec![AssistantContentBlock::ToolCall(call.clone())],
            }),
            event(
                store,
                E::AssistantMessageCommitted {
                    message_id: MessageId::new(message_id),
                },
                seconds,
            ),
        )
        .unwrap();
}

/// Commits one canonical Tool message answering an exact call.
fn settle_tool_call(
    store: &dyn ConversationStore,
    owner: &str,
    message_id: &str,
    call: &ToolCall,
    result: ToolExecutionResult,
    seconds: i64,
) {
    store
        .append_canonical_with_event(
            &MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(message_id),
                occurrence: ToolCallOccurrenceRef::new(
                    MessageId::new(owner),
                    ContentBlockIndex::new(0),
                ),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result,
            }),
            event(
                store,
                E::ToolMessageCommitted {
                    message_id: MessageId::new(message_id),
                    tool_call_id: call.id.clone(),
                },
                seconds,
            ),
        )
        .unwrap();
}

fn bash_call(call_id: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "ls -la\necho done", "timeout": 30 }),
    }
}

// ---------------------------------------------------------------------------
// Native semantics
// ---------------------------------------------------------------------------

/// Retries are actual requests under one logical Step, ordered by the native
/// ordinal, and reopening the durable store reproduces the page exactly.
#[test]
fn retries_stay_actual_requests_under_one_native_step_and_reopen_identically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let first = request(&store, 0);
    failure(&store, &first, ModelErrorKind::ContextWindowExceeded);
    let second = request(&store, 1);
    failure(&store, &second, ModelErrorKind::MalformedToolProposal);
    let third = request(&store, 2);
    completion(
        &store,
        &third,
        Some(ModelUsage {
            input_tokens: 7,
            output_tokens: 11,
            total_tokens: 18,
            details: None,
        }),
        None,
        7,
    );
    append(&store, E::TurnCompleted, 8);
    append(
        &store,
        E::AttemptCompleted {
            attempt_id: AttemptId::new("attempt-a"),
            finish_reason: ModelFinishReason::Stop,
        },
        9,
    );

    let projected = page(&store);
    assert_eq!(
        projected
            .records
            .iter()
            .filter(|record| record.kind == TraceKind::Step)
            .count(),
        1,
        "three requests share one logical Step"
    );
    let requests: Vec<_> = projected
        .records
        .iter()
        .filter_map(|record| record.request.as_ref())
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.retry_number)
            .collect::<Vec<_>>(),
        [0, 1, 2],
        "the ordinal is native, not inferred from timestamps"
    );
    assert_eq!(
        requests[1].previous_failure_kind,
        Some(ModelErrorKind::ContextWindowExceeded)
    );
    assert_eq!(
        requests[2].previous_failure_kind,
        Some(ModelErrorKind::MalformedToolProposal)
    );
    assert!(requests[0].usage.is_none(), "a failed request has no usage");
    assert_eq!(requests[2].usage.as_ref().unwrap().input_tokens, 7);
    assert_eq!(projected.records[0].timing.duration_ms, Some(9000));
    assert!(
        projected
            .records
            .iter()
            .all(|record| record.location.step_id == Some(TurnId::new("1")))
    );

    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(page(&reopened), projected, "history reopens identically");
}

/// Historical request detail comes from the frozen snapshot and the Surface
/// revision it referenced, so later configuration and later history cannot
/// rewrite what an old request shows.
#[test]
fn historical_request_detail_ignores_later_configuration_and_later_history() {
    let store = store("conv_1d0b6c2a-41b1-7a37-9e11-f0b6a9a8c111");
    start(&store);
    let frozen = request(&store, 0);
    completion(&store, &frozen, None, None, 4);
    let record = page(&store);
    let record = record_of(&record, TraceKind::Request);
    let before = detail_of(&store, &record.id);
    let before_request = before.request.as_ref().expect("request detail");
    assert_eq!(before_request.model, "historical-model-0");
    assert_eq!(before_request.effective_system_prompt.text, SYSTEM_PROMPT);
    assert_eq!(before_request.tools.len(), 1);
    assert_eq!(before_request.tools[0].name, "bash");
    assert_eq!(before_request.max_output_tokens, 100);
    assert_eq!(before_request.context_window_tokens, 4096);
    let historical_context = before_request.messages.len();

    // The Session moves on: new canonical history, and a later request that
    // freezes a different model, a different prompt and a different catalog.
    store
        .append_canonical(&MessageBlock::User(
            crate::message::types::UserMessageBlock {
                id: MessageId::new("later-user"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "a later turn the old request never saw".into(),
                })],
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            },
        ))
        .unwrap();
    append(&store, E::TurnStarted, 20);
    let mut later = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-a"),
            turn: TurnId::new("2"),
            retry_number: 0,
        },
        store.load_head().unwrap().revision,
        "A COMPLETELY DIFFERENT PROMPT".to_owned(),
        vec![],
        crate::runtime::RuntimeResourceRevision::new(9),
        ModelInvocationConfig {
            model: "a-completely-different-model".into(),
            protocol: ModelProtocol::AnthropicMessages,
            max_output_tokens: 9999,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        1_000_000,
        None,
        true,
        vec![],
        CapabilityRevision::new(2),
        ContextGeneration {
            id: 2,
            contributors: vec![],
        },
        None,
        vec![],
    );
    later.continuation = None;
    store
        .commit_model_turn_start(&[], &later, timestamp(21))
        .unwrap();

    let after = detail_of(&store, &record.id);
    assert_eq!(after, before, "historical detail is immutable");
    let after_request = after.request.as_ref().expect("request detail");
    assert_eq!(after_request.model, "historical-model-0");
    assert_eq!(after_request.effective_system_prompt.text, SYSTEM_PROMPT);
    assert_eq!(after_request.tools.len(), 1);
    assert_eq!(after_request.messages.len(), historical_context);
    let wire = serde_json::to_string(&after).unwrap();
    assert!(!wire.contains("a later turn the old request never saw"));
    assert!(!wire.contains("A COMPLETELY DIFFERENT PROMPT"));
    assert!(!wire.contains("a-completely-different-model"));
}

/// Tool arguments, schema and result each come from their own authority, and
/// the schema is the one the owning request actually carried.
#[test]
fn tool_detail_joins_arguments_schema_and_result_from_exact_authorities() {
    let store = store("conv_5c27f0d1-93af-7a24-8e01-39d4cb3f1a22");
    start(&store);
    let frozen = request(&store, 0);
    let call = bash_call("call-inspect");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    completion(&store, &frozen, None, None, 4);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        5,
    );
    settle_tool_call(
        &store,
        frozen.provisional_message_id.as_str(),
        "tool-result",
        &call,
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![
                ToolResultContent::Text(TextBlock {
                    text: "total 8\ndrwxr-xr-x".into(),
                }),
                ToolResultContent::Json {
                    value: serde_json::json!({ "files": 2, "hidden": false }),
                },
            ],
            duration_ms: 1234,
            exit_code: Some(0),
            artifacts: vec![],
            truncation: Some(crate::tools::types::TruncationState {
                truncated: true,
                original_bytes: Some(98_765),
            }),
            workflow: None,
            managed_output: Some(crate::tools::types::ManagedOutputContinuation::Complete {
                locator: std::path::PathBuf::from(MANAGED_OUTPUT_LOCATOR),
            }),
        },
        7,
    );

    let projected = page(&store);
    let record = record_of(&projected, TraceKind::Tool);
    let summary = record.tool.as_ref().expect("tool summary");
    assert!(summary.started, "this record is the durable start fact");
    assert_eq!(summary.name.as_deref(), Some("bash"));
    assert_eq!(summary.outcome, Some(TraceToolOutcome::Success));

    let detail = detail_of(&store, &record.id);
    let tool = detail.tool.as_ref().expect("tool detail");
    assert_eq!(tool.lifecycle, TraceToolLifecycle::Settled);
    // Arguments come from the canonical proposal, exactly as recorded.
    assert_eq!(
        tool.arguments.as_ref().unwrap().value,
        serde_json::json!({ "command": "ls -la\necho done", "timeout": 30 })
    );
    // The schema comes from the owning request's own frozen catalog.
    let definition = tool.definition.as_ref().expect("historical definition");
    assert_eq!(definition.name, "bash");
    assert_eq!(definition.description.text, TOOL_DESCRIPTION);
    assert_eq!(
        definition.input_schema.value["properties"]["command"]["type"],
        serde_json::json!("string")
    );
    // The native Bash contract identifies `command` as shell source.
    let source = tool.source.as_ref().expect("native source view");
    assert_eq!(source.field, "command");
    assert_eq!(source.language.as_deref(), Some("shell"));
    assert_eq!(source.text.text, "ls -la\necho done");
    // The result comes from the canonical ToolMessage, not from Trace state.
    let result = tool.result.as_ref().expect("canonical result");
    assert_eq!(result.outcome, TraceToolOutcome::Success);
    assert_eq!(result.duration_ms, 1234);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.blocks.len(), 2);
    assert!(matches!(result.blocks[0], TraceContentBlock::Text { .. }));
    assert!(matches!(result.blocks[1], TraceContentBlock::Json { .. }));
    assert_eq!(
        result.truncation,
        Some(TraceToolTruncation {
            truncated: true,
            original_bytes: Some(98_765),
        })
    );
    let managed = result.managed_output.as_ref().expect("managed metadata");
    assert!(managed.complete && managed.available);
    assert_eq!(
        managed.locator.as_deref(),
        Some(std::path::Path::new(MANAGED_OUTPUT_LOCATOR))
    );
}

/// A proposed call is not a started execution and never acquires a result.
#[test]
fn a_proposed_tool_call_does_not_imply_started_execution() {
    let store = store("conv_9ab3ee6f-1dcb-71b0-8ac3-1c6e2ad77f40");
    start(&store);
    let frozen = request(&store, 0);
    let call = bash_call("call-never-started");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    completion(&store, &frozen, None, None, 4);

    let projected = page(&store);
    assert!(
        !projected
            .records
            .iter()
            .any(|record| record.kind == TraceKind::Tool),
        "a proposal creates no Tool execution record"
    );
    let assistant = record_of(&projected, TraceKind::Assistant);
    assert_eq!(assistant.calls.len(), 1);
    assert_eq!(assistant.calls[0].call_id, call.id);
    assert_eq!(assistant.calls[0].name, "bash");
    assert!(
        assistant.tool.is_none(),
        "an Assistant record carries proposals, never execution"
    );

    // The same call inspected directly reports only what is proven.
    let anchor = TraceProjection::new(&store).unwrap();
    let detail = anchor.detail(&assistant.id).unwrap().unwrap();
    assert_eq!(detail.kind, TraceKind::Assistant);
    assert!(detail.tool.is_none());
}

/// Provider completion is not canonical Assistant acceptance.
#[test]
fn provider_completion_does_not_imply_canonical_assistant_acceptance() {
    let store = store("conv_3f7b41c9-0a2e-7c55-93de-7cb0a2e41b90");
    start(&store);
    let frozen = request(&store, 0);
    completion(&store, &frozen, None, None, 5);
    let projected = page(&store);
    assert_eq!(
        record_of(&projected, TraceKind::Request).state,
        TraceState::Completed
    );
    assert!(
        !projected
            .records
            .iter()
            .any(|record| record.kind == TraceKind::Assistant),
        "no canonical commit, no Assistant record"
    );
    assert!(
        projected
            .records
            .iter()
            .all(|record| record.message_id.is_none())
    );
}

/// Adopted inbound becomes a User record carrying its canonical content.
#[test]
fn adopted_inbound_becomes_a_user_record_with_canonical_content() {
    let store = store("conv_7e11c0a3-55bf-7d44-8bb2-6a41f0d8e5a7");
    // The real acceptance and adoption path, so the anchor is the durable
    // adoption transaction rather than a hand-built event.
    store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: Some(MessageId::new("adopted-user")),
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Please inspect the historical trajectory.".into(),
            })],
            timestamp: timestamp(0),
            correlation: None,
        })
        .unwrap();
    store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: Some(MessageId::new("second-adopted-user")),
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Also inspect this second message.".into(),
            })],
            timestamp: timestamp(1),
            correlation: None,
        })
        .unwrap();
    let batch = store.select_pending_batch().unwrap().unwrap();
    store
        .adopt_pending_batch(batch.watermark, Some(AttemptId::new("attempt-a")))
        .unwrap();

    let projected = page(&store);
    let record = record_of(&projected, TraceKind::User);
    assert_eq!(record.state, TraceState::Completed);
    assert_eq!(
        record.timing.duration_ms, None,
        "adoption is instantaneous, so it has no span"
    );
    assert_eq!(
        record.preview.as_ref().unwrap().text,
        "Please inspect the historical trajectory."
    );
    assert!(record.has_detail);

    let detail = detail_of(&store, &record.id);
    assert_eq!(detail.messages.len(), 2);
    assert_eq!(
        detail.messages[1].message_id,
        MessageId::new("second-adopted-user")
    );
    assert!(
        matches!(&detail.messages[1].blocks[0], TraceContentBlock::Text { text } if text.text == "Also inspect this second message.")
    );
    let message = detail.messages.first().expect("canonical message");
    assert_eq!(message.role, TraceMessageRole::User);
    assert_eq!(message.source.as_deref(), Some("human"));
    assert!(matches!(
        &message.blocks[0],
        TraceContentBlock::Text { text } if text.text == "Please inspect the historical trajectory."
    ));
}

/// A request without a terminal stays incomplete even once its Attempt
/// settled, and keeps no duration.
#[test]
fn a_missing_request_terminal_stays_incomplete_after_attempt_cancellation() {
    let store = store("conv_2374d917-94b7-7f4f-84ec-9587d2cf00ae");
    start(&store);
    request(&store, 0);
    append(
        &store,
        E::AttemptCancelled {
            attempt_id: AttemptId::new("attempt-a"),
            reason: crate::runtime::types::CancellationReason::UserRequested,
        },
        5,
    );
    let records = page(&store).records;
    assert_eq!(records[0].state, TraceState::Cancelled);
    let request = records
        .iter()
        .find(|record| record.kind == TraceKind::Request)
        .unwrap();
    assert_eq!(request.state, TraceState::Incomplete);
    assert_eq!(request.timing.duration_ms, None);
    assert!(request.request.as_ref().unwrap().generation.is_none());
    assert!(
        records
            .iter()
            .all(|record| record.kind != TraceKind::Assistant)
    );
}

/// The terminal vocabulary never invents an outcome it was not given.
#[test]
fn the_terminal_vocabulary_does_not_infer_missing_outcomes() {
    use crate::events::types::{AttemptFailure, AttemptLimit};
    let outcomes = [
        (
            E::AttemptTimedOut {
                attempt_id: AttemptId::new("a"),
            },
            TraceState::TimedOut,
        ),
        (
            E::AttemptLimitExceeded {
                attempt_id: AttemptId::new("a"),
                limit: AttemptLimit::MaxTurns,
            },
            TraceState::Limited,
        ),
        (
            E::AttemptFailed {
                attempt_id: AttemptId::new("a"),
                error: AttemptFailure::Runtime {
                    error: crate::runtime::types::RuntimeError::Internal {
                        message: "diagnostic".into(),
                    },
                },
            },
            TraceState::Failed,
        ),
        (E::TurnStarted, TraceState::Incomplete),
    ];
    for (event, expected) in outcomes {
        assert_eq!(super::record::terminal(&event), expected);
    }
    assert_eq!(
        super::record::tool_state(&ToolExecutionStatus::OutcomeUnknown {
            detail: "unconfirmed".into()
        }),
        TraceState::OutcomeUnknown
    );
}

/// Compaction has no native operation identity, so its boundary is the next
/// durable compaction fact — at any depth of history.
#[test]
fn compaction_uses_the_next_boundary_even_far_back_in_history() {
    let store = store("conv_96847128-a59a-7bfa-8bf9-526873a32546");
    for index in 0..140 {
        append(&store, E::CompactionStarted, index * 2);
        append(
            &store,
            E::CompactionFailed {
                error: "compaction diagnostic".into(),
            },
            index * 2 + 1,
        );
    }
    let read = TraceProjection::new(&store).unwrap();
    let first = read.page(Some(&TraceCursor::at(3)), 1).unwrap();
    assert_eq!(first.records[0].state, TraceState::Failed);
    assert_eq!(first.records[0].timing.duration_ms, Some(1000));
}

/// Parallel physical completion never reorders calls, and outcome certainty
/// survives exactly as the canonical status recorded it.
#[test]
fn parallel_tool_completion_keeps_exact_call_order_and_certainty() {
    let store = store("conv_f9d35d43-770d-7909-8a66-3e665e82ae1d");
    start(&store);
    let owner = AssistantMessageBlock {
        id: MessageId::new("canonical-tool-owner"),
        content: ["call-a", "call-b", "call-c"]
            .into_iter()
            .map(|call| {
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new(call),
                    tool_id: ToolId::new(if call == "call-b" { "tool-b" } else { "tool-a" }),
                    name: "same-name".into(),
                    arguments: serde_json::json!({ "index": call }),
                })
            })
            .collect(),
    };
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(owner),
            event(
                &store,
                E::AssistantMessageCommitted {
                    message_id: MessageId::new("canonical-tool-owner"),
                },
                1,
            ),
        )
        .unwrap();
    for (call, tool, at) in [
        ("call-a", "tool-a", 2),
        ("call-b", "tool-b", 3),
        ("call-c", "tool-a", 4),
    ] {
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new(tool),
            },
            at,
        );
    }
    for (call, tool, status, at) in [
        (
            "call-b",
            "tool-b",
            ToolExecutionStatus::OutcomeUnknown {
                detail: "transport lost after dispatch".into(),
            },
            6,
        ),
        ("call-a", "tool-a", ToolExecutionStatus::Success, 7),
    ] {
        append(
            &store,
            E::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new(tool),
                result: ToolExecutionResult {
                    status,
                    content: vec![],
                    duration_ms: 999_999,
                    exit_code: None,
                    artifacts: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            },
            at,
        );
    }
    let projected = page(&store);
    let tools: Vec<_> = projected
        .records
        .iter()
        .filter(|record| record.kind == TraceKind::Tool)
        .collect();
    assert_eq!(
        tools
            .iter()
            .map(|record| record.tool.as_ref().unwrap().call_id.as_str())
            .collect::<Vec<_>>(),
        ["call-a", "call-b", "call-c"],
        "start order, never physical completion order"
    );
    assert_eq!(
        tools.iter().map(|record| record.state).collect::<Vec<_>>(),
        [
            TraceState::Completed,
            TraceState::OutcomeUnknown,
            TraceState::Incomplete
        ]
    );
    assert_eq!(tools[0].timing.duration_ms, Some(5000));
    assert_eq!(tools[1].timing.duration_ms, Some(3000));
    assert_eq!(tools[2].timing.duration_ms, None);
}

/// Workflow runs join by exact native run identity, never by definition name.
#[test]
fn workflow_runs_join_exact_native_identity_not_definition_name() {
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId};
    let store = store("conv_141ff882-0973-7251-89c7-6389cfe2e736");
    let id = WorkflowId::parse("same-workflow").unwrap();
    let runs: Vec<_> = (1..=2)
        .map(|invocation| WorkflowRunId {
            conversation_id: store.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-a"),
            invocation,
        })
        .collect();
    for (index, run) in runs.iter().enumerate() {
        append(
            &store,
            E::WorkflowStarted {
                tool_call_id: ToolCallId::new("same-call"),
                workflow_id: id.clone(),
                run_id: run.clone(),
            },
            i64::try_from(index).unwrap(),
        );
    }
    append(
        &store,
        E::WorkflowCompleted {
            workflow_id: id,
            run_id: runs[1].clone(),
        },
        10,
    );
    let records = page(&store).records;
    assert_eq!(records[0].state, TraceState::Incomplete);
    assert_eq!(records[0].timing.duration_ms, None);
    assert_eq!(records[1].state, TraceState::Completed);
    assert_eq!(records[1].timing.duration_ms, Some(9000));
    assert_eq!(
        records[1].native_id.as_deref(),
        Some(serde_json::to_string(&runs[1]).unwrap().as_str())
    );
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// Generation metrics use the request's own settled endpoints, and each one
/// is unavailable rather than estimated when its evidence is missing.
#[test]
fn generation_metrics_use_authoritative_endpoints_only() {
    let usage = ModelUsage {
        input_tokens: 100,
        output_tokens: 250,
        total_tokens: 350,
        details: None,
    };
    let complete = generation_metrics(
        GenerationEvidence {
            dispatch_after_start_ms: None,
            first_output_ms: Some(400),
            last_output_ms: Some(2_300),
            terminal_ms: 2_400,
        },
        Some(&usage),
    );
    assert_eq!(
        complete.timeline, None,
        "numeric TTFT never invents a bridge"
    );
    assert_eq!(complete.ttft_ms, Some(400));
    assert_eq!(complete.generation_ms, Some(2_000));
    assert_eq!(complete.terminal_ms, 2_400);
    assert_eq!(complete.output_tokens_per_second, Some(125.0));

    // No model output: TTFT, decode and throughput are all unknown.
    let silent = generation_metrics(
        GenerationEvidence {
            dispatch_after_start_ms: None,
            first_output_ms: None,
            last_output_ms: None,
            terminal_ms: 900,
        },
        Some(&usage),
    );
    assert_eq!(silent.ttft_ms, None);
    assert_eq!(silent.generation_ms, None);
    assert_eq!(silent.output_tokens_per_second, None);
    assert_eq!(silent.terminal_ms, 900);
}

/// Throughput exists only when usage evidence supports it.
#[test]
fn throughput_requires_usage_evidence() {
    let evidence = GenerationEvidence {
        dispatch_after_start_ms: None,
        first_output_ms: Some(100),
        last_output_ms: Some(1_100),
        terminal_ms: 1_100,
    };
    assert_eq!(
        generation_metrics(evidence, None).output_tokens_per_second,
        None,
        "no usage, no rate"
    );
    let measured = generation_metrics(
        evidence,
        Some(&ModelUsage {
            input_tokens: 1,
            output_tokens: 500,
            total_tokens: 501,
            details: None,
        }),
    );
    assert_eq!(measured.output_tokens_per_second, Some(500.0));
}

/// Reopening a Session reproduces the identical historical timing, because
/// the evidence is durable and no wall clock participates.
#[test]
fn reopening_a_session_reproduces_identical_historical_timing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_ce3d1a44-7f22-7bd9-8a01-5d2fa04e9b31");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let frozen = request(&store, 0);
    completion(
        &store,
        &frozen,
        Some(ModelUsage {
            input_tokens: 40,
            output_tokens: 120,
            total_tokens: 160,
            details: None,
        }),
        Some(GenerationEvidence {
            dispatch_after_start_ms: Some(400),
            first_output_ms: Some(320),
            last_output_ms: Some(1_520),
            terminal_ms: 1_600,
        }),
        6,
    );
    let live = page(&store);
    let live_detail = detail_of(&store, &record_of(&live, TraceKind::Request).id);
    drop(store);

    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    let historical = page(&reopened);
    assert_eq!(historical, live);
    let generation = record_of(&historical, TraceKind::Request)
        .request
        .as_ref()
        .unwrap()
        .generation
        .expect("settled generation evidence survives reopen");
    let timeline = generation.timeline.unwrap();
    assert_eq!(timeline.dispatch_ms, 400);
    assert_eq!(timeline.first_output_ms, Some(720));
    assert_eq!(timeline.last_output_ms, Some(1_920));
    assert_eq!(timeline.terminal_ms, 2_000);
    assert_eq!(generation.ttft_ms, Some(320));
    assert_eq!(generation.generation_ms, Some(1_280));
    assert_eq!(generation.terminal_ms, 1_600);
    assert_eq!(
        generation.output_tokens_per_second,
        Some(120.0 / 1.28),
        "throughput derives from the settled decode span and reported usage"
    );
    assert_eq!(
        detail_of(&reopened, &record_of(&historical, TraceKind::Request).id),
        live_detail
    );
}

/// A request settled before generation evidence existed keeps no metrics
/// rather than acquiring invented ones.
#[test]
fn a_request_without_generation_evidence_reports_no_metrics() {
    let store = store("conv_45ab90c1-0e7d-7ff2-8b7c-6d20ae91c334");
    start(&store);
    let frozen = request(&store, 0);
    completion(&store, &frozen, None, None, 5);
    let projected = page(&store);
    let request = record_of(&projected, TraceKind::Request)
        .request
        .as_ref()
        .unwrap();
    assert!(request.generation.is_none());
    assert_eq!(
        record_of(&projected, TraceKind::Request).timing.duration_ms,
        Some(3000),
        "the Journal span remains available independently"
    );
}

// ---------------------------------------------------------------------------
// Summary / detail transport
// ---------------------------------------------------------------------------

/// Summary pages stay bounded and carry no heavy request or Tool payload.
#[test]
fn summary_pages_carry_no_heavy_request_or_tool_payload() {
    let store = store("conv_8b336994-4dd2-73fa-839e-32d1aeb1f763");
    start(&store);
    // A prompt far larger than any summary bound, so "the page carries a
    // bounded preview" and "the page carries the prompt" stay distinguishable.
    let heavy_prompt = format!("{SYSTEM_PROMPT} {}", "P".repeat(40_000));
    let mut prepared = prepared_request(
        &store,
        0,
        None,
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-a"),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
    );
    prepared.effective_system_prompt.clone_from(&heavy_prompt);
    let frozen = commit_request(&store, prepared, &[]);
    let call = bash_call("call-heavy");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    completion(&store, &frozen, None, None, 4);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        5,
    );
    settle_tool_call(
        &store,
        frozen.provisional_message_id.as_str(),
        "heavy-result",
        &call,
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![ToolResultContent::Text(TextBlock {
                text: "R".repeat(400_000),
            })],
            duration_ms: 1,
            exit_code: None,
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
        6,
    );

    let projected = page(&store);
    let wire = serde_json::to_string(&projected).unwrap();
    assert!(
        wire.len() <= TRACE_PAGE_BYTES,
        "a page stays inside its byte bound: {}",
        wire.len()
    );
    assert!(
        !wire.contains(&heavy_prompt),
        "the complete system prompt is detail-only"
    );
    let system = record_of(&projected, TraceKind::Request)
        .request
        .as_ref()
        .unwrap()
        .system_prompt
        .clone();
    let preview = system
        .preview
        .expect("an introduced prompt carries a preview");
    assert!(preview.truncated && preview.text.len() <= TRACE_PREVIEW_BYTES);
    assert!(heavy_prompt.starts_with(&preview.text));
    assert!(
        !wire.contains(TOOL_DESCRIPTION),
        "the Tool catalog is detail-only"
    );
    assert!(!wire.contains("ls -la"), "Tool arguments are detail-only");
    assert!(
        !wire.contains(&"R".repeat(2_000)),
        "Tool results are detail-only"
    );
    for record in &projected.records {
        assert!(
            serde_json::to_vec(record).unwrap().len() <= TRACE_RECORD_BYTES,
            "each summary record stays inside its own bound"
        );
    }
    // The heavy content is reachable, on demand, for the selected record.
    let detail = detail_of(&store, &record_of(&projected, TraceKind::Tool).id);
    let result = detail.tool.unwrap().result.unwrap();
    assert!(matches!(
        &result.blocks[0],
        TraceContentBlock::Text { text } if text.truncated && text.text.len() <= TRACE_DETAIL_TEXT_BYTES
    ));
}

/// Detail is addressed by exact stable identity; an identity that names no
/// record at this cut reports absence rather than a neighbour.
#[test]
fn detail_identity_is_exact_and_absent_records_report_absence() {
    let store = store("conv_20d19a4e-8bcf-7cb0-8b41-1ab2c9370f45");
    start(&store);
    let frozen = request(&store, 0);
    completion(&store, &frozen, None, None, 4);
    let projected = page(&store);
    let read = TraceProjection::new(&store).unwrap();
    for record in &projected.records {
        let detail = read.detail(&record.id).unwrap().expect("loaded record");
        assert_eq!(detail.id, record.id);
        assert_eq!(detail.kind, record.kind);
    }
    // A sequence between two anchors resolves to nothing at all.
    let occupied: std::collections::BTreeSet<_> =
        projected.records.iter().map(|r| r.id.clone()).collect();
    let free = (1..200)
        .map(|n| format!("trace:{n}"))
        .find(|id| !occupied.contains(id))
        .expect("an unoccupied sequence");
    assert!(read.detail(&free).unwrap().is_none());
    for invalid in ["1", "request:1", "transcript:1", "trace:-1", "trace:abc"] {
        assert!(read.detail(invalid).is_err(), "{invalid}");
    }
}

/// Paging terminates, every page is disjoint, and reads mutate nothing.
#[test]
fn paging_and_the_read_cut_are_finite_and_mutate_nothing() {
    let store = store("conv_26ca59cb-e63e-7f7f-8903-10861c5839ba");
    start(&store);
    for retry in 0..40 {
        let frozen = request(&store, retry);
        failure(&store, &frozen, ModelErrorKind::Transport);
    }
    let before_head = store.load_head().unwrap();
    let transcript = store.load_transcript_page(None, 64).unwrap();
    let frontier = store.presentation_frontier().unwrap();

    let read = TraceProjection::new(&store).unwrap();
    let newest = read.page(None, 7).unwrap();
    assert_eq!(newest.records.len(), 7);
    let cursor = newest.next_cursor.clone().unwrap();
    let mut all = newest.records.clone();
    let mut next = Some(cursor.clone());
    while let Some(cursor) = next {
        let older = read.page(Some(&cursor), 7).unwrap();
        next = older.next_cursor;
        let mut records = older.records;
        records.extend(all);
        all = records;
    }
    assert_eq!(all.len(), 42);
    assert_eq!(
        all.iter()
            .map(|record| &record.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        all.len(),
        "pages never repeat a record"
    );

    // No read advanced any durable or live position.
    assert_eq!(store.load_head().unwrap(), before_head);
    assert_eq!(store.load_transcript_page(None, 64).unwrap(), transcript);
    assert_eq!(store.presentation_frontier().unwrap(), frontier);
    // Detail reads are equally inert.
    let _ = read.detail(&all[0].id).unwrap();
    assert_eq!(store.load_head().unwrap(), before_head);
    assert_eq!(store.presentation_frontier().unwrap(), frontier);

    // Explicit ordering point: native progress happens after the cut was
    // captured and before the historical read. No sleep is involved.
    let later = request(&store, 40);
    assert_eq!(read.page(None, 7).unwrap(), newest, "the cut is immutable");
    assert!(page(&store).records.iter().any(|record| {
        record
            .request
            .as_ref()
            .is_some_and(|request| request.request_id == later.request_id)
    }));

    assert!(read.page(None, 0).is_err());
    assert!(read.page(None, TRACE_PAGE_LIMIT + 1).is_err());
    for bad in [
        "1",
        "request:1",
        "transcript:1",
        "trace:-1",
        "trace:999_999999_999999_999999",
    ] {
        assert!(read.page(Some(&TraceCursor(bad.into())), 1).is_err());
    }
}

/// Lifecycle refresh stays bounded and repeats no immutable historical input.
#[test]
fn lifecycle_refresh_is_bounded_and_repeats_no_historical_input() {
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::CapabilityView;
    let store = store("conv_08e8fef2-9708-70e8-875e-0815ed73b267");
    start(&store);
    request(&store, 0);
    let projection = TraceProjection::new(&store).unwrap();
    let page = projection.page(None, 32).unwrap();
    let position = record_of(&page, TraceKind::Request).position.clone();
    let snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    let updates = projection
        .refresh(&vec![position.clone(); TRACE_RECORD_LIMIT], Some(&snapshot))
        .unwrap();
    assert_eq!(updates.len(), TRACE_RECORD_LIMIT);
    for update in &updates {
        assert!(serde_json::to_vec(update).unwrap().len() <= 1024);
    }
    let wire = serde_json::to_string(&updates).unwrap();
    for absent in [
        SYSTEM_PROMPT,
        TOOL_DESCRIPTION,
        "effective_system_prompt",
        "input_schema",
        "messages",
    ] {
        assert!(!wire.contains(absent), "refresh repeated {absent}");
    }
    assert!(
        projection
            .refresh(&vec![position; TRACE_RECORD_LIMIT + 1], Some(&snapshot))
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// Safety
// ---------------------------------------------------------------------------

/// The typed allowlist exposes Session-owned model-visible facts and keeps
/// every infrastructure authority on the other side of the boundary.
#[test]
fn the_inspection_allowlist_exposes_session_facts_and_withholds_infrastructure() {
    let store = store("conv_61fd83b0-2f3a-70b6-8e4d-8c9e2a5d7103");
    start(&store);
    let frozen = request(&store, 0);
    let call = bash_call("call-allowlist");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    failure(&store, &frozen, ModelErrorKind::Authentication);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        5,
    );
    settle_tool_call(
        &store,
        frozen.provisional_message_id.as_str(),
        "allowlist-result",
        &call,
        ToolExecutionResult {
            status: ToolExecutionStatus::Failed {
                error: "exit status 2".into(),
            },
            content: vec![ToolResultContent::Text(TextBlock {
                text: "command not found".into(),
            })],
            duration_ms: 12,
            exit_code: Some(2),
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: Some(crate::tools::types::ManagedOutputContinuation::Complete {
                locator: std::path::PathBuf::from(MANAGED_OUTPUT_LOCATOR),
            }),
        },
        7,
    );

    let projected = page(&store);
    let request_detail = detail_of(&store, &record_of(&projected, TraceKind::Request).id);
    let tool_detail = detail_of(&store, &record_of(&projected, TraceKind::Tool).id);
    let exposed = format!(
        "{}{}{}",
        serde_json::to_string(&projected).unwrap(),
        serde_json::to_string(&request_detail).unwrap(),
        serde_json::to_string(&tool_detail).unwrap()
    );

    // Session-owned, model-visible facts an inspector needs.
    for present in [
        SYSTEM_PROMPT,
        TOOL_DESCRIPTION,
        "ls -la",
        "command not found",
        "exit status 2",
        "temperature",
        MANAGED_OUTPUT_LOCATOR,
    ] {
        assert!(exposed.contains(present), "withheld {present}");
    }

    // Infrastructure authority and credentials, on the other side.
    for absent in [
        PROVIDER_CREDENTIAL,
        MCP_CREDENTIAL,
        EXECUTOR_ENVIRONMENT,
        PROVIDER_CONTINUATION,
        "api_key",
        "authorization",
        "executor_env",
        "capability_revision",
        "context_generation",
        "runtime_resource_revision",
        "upload_projection",
        "system_sections",
        "surface_revision",
    ] {
        assert!(!exposed.contains(absent), "leaked {absent}");
    }

    let request = request_detail.request.as_ref().unwrap();
    assert_eq!(request.options.len(), 1, "only the allowlisted option");
    assert_eq!(request.options[0].name, "temperature");
    assert_eq!(
        request.omitted_option_count, 3,
        "the omission is visible without naming what was omitted"
    );
    assert_eq!(
        request.failure.as_ref().unwrap().kind,
        ModelErrorKind::Authentication
    );
}

/// Oversized values are explicitly truncated rather than silently shortened.
#[test]
fn oversized_values_are_explicitly_truncated() {
    let store = store("conv_be417d20-6ab4-70a2-8b23-30c4e8f95cd2");
    start(&store);
    let frozen = request(&store, 0);
    let call = ToolCall {
        id: ToolCallId::new("call-oversized"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "x".repeat(200_000) }),
    };
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    completion(&store, &frozen, None, None, 4);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        5,
    );
    let projected = page(&store);
    let detail = detail_of(&store, &record_of(&projected, TraceKind::Tool).id);
    let tool = detail.tool.as_ref().unwrap();
    assert!(
        tool.arguments.as_ref().unwrap().truncated,
        "oversized arguments say so"
    );
    let source = tool.source.as_ref().unwrap();
    assert!(source.text.truncated);
    assert!(source.text.text.len() <= TRACE_DETAIL_TEXT_BYTES);
    assert!(serde_json::to_vec(&detail).unwrap().len() <= TRACE_DETAIL_BYTES);

    let bounded = TraceText::bounded(&"界".repeat(10_000), TRACE_DETAIL_TEXT_BYTES);
    assert!(bounded.truncated);
    assert!(bounded.text.len() <= TRACE_DETAIL_TEXT_BYTES);
}

/// Record bounds account for JSON escaping and omit oversized identities
/// whole rather than shortening them into different identities.
#[test]
fn record_bounds_account_for_escaping_and_omit_oversized_identities() {
    let store = store("conv_480c9ce8-b0e2-70c4-882d-ae5878fe0801");
    start(&store);
    request(&store, 0);
    let mut record = page(&store).records.pop().unwrap();
    let escaped = "\0".repeat(600);
    record.location.attempt_id = Some(AttemptId::new(escaped.clone()));
    record.location.step_id = Some(TurnId::new(escaped.clone()));
    record.native_id = Some(escaped.clone());
    record.message_id = Some(MessageId::new(escaped.clone()));
    record.calls = (0..8)
        .map(|_| TraceToolCall {
            call_id: ToolCallId::new(escaped.clone()),
            tool_id: ToolId::new(escaped.clone()),
            name: "x".repeat(4096),
        })
        .collect();
    let request = record.request.as_mut().unwrap();
    request.request_id = RequestId::new(escaped.clone());
    request.assistant_message_id = MessageId::new(escaped);
    request.model = "\0".repeat(4096);
    record.attachments.push(TraceArtifact {
        artifact_id: ArtifactId::new("oversized".repeat(1000)),
        image: true,
        name: None,
        mime_type: None,
    });
    bound_record(&mut record);
    assert!(record.truncated);
    assert!(record.attachments.is_empty());
    assert!(record.location.attempt_id.is_none());
    assert!(serde_json::to_vec(&record).unwrap().len() <= TRACE_RECORD_BYTES);
}

/// Request failure preserves the native cancellation and timeout classes.
#[test]
fn request_failure_preserves_native_cancellation_and_timeout_classes() {
    for (kind, state) in [
        (ModelErrorKind::Cancelled, TraceState::Cancelled),
        (ModelErrorKind::Timeout, TraceState::TimedOut),
    ] {
        let store = store("conv_05d689cd-d3ca-7d01-8056-d79aaff8d9da");
        start(&store);
        let frozen = request(&store, 0);
        failure(&store, &frozen, kind);
        assert_eq!(page(&store).records.last().unwrap().state, state);
    }
}

// ---------------------------------------------------------------------------
// Current lifecycle evidence
// ---------------------------------------------------------------------------

/// Current runtime labels require an exact identity match and never supply
/// timing; a durable terminal always wins.
#[test]
fn live_labels_require_exact_runtime_identity_and_never_supply_timing() {
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::{CapabilityView, RuntimeClientBackgroundExecution};
    use crate::tools::background::BackgroundLifecycle;
    let store = store("conv_da5c0fef-42f3-73f9-8f7f-b6878d9b0cbd");
    start(&store);
    let mut snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    let mut record = TraceProjection::new(&store)
        .unwrap()
        .page(None, 32)
        .unwrap()
        .records
        .remove(0);
    record.kind = TraceKind::Background;
    record.native_id = Some("exec_db47f954-a31a-74a3-8706-22baacdc0747".into());
    snapshot.trace.records = vec![record];
    snapshot.background.push(RuntimeClientBackgroundExecution {
        execution_id: crate::runtime::identity::ToolExecutionId::new(
            "exec_68344812-64a3-79bc-815f-6c3b32dfac91",
        ),
        tool_id: ToolId::new("same-tool"),
        tool_name: "same-name".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    });
    repair_live(&mut snapshot);
    assert_eq!(
        snapshot.trace.records[0].state,
        TraceState::Incomplete,
        "a different execution identity proves nothing"
    );
    snapshot.background[0].execution_id =
        crate::runtime::identity::ToolExecutionId::new("exec_db47f954-a31a-74a3-8706-22baacdc0747");
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.records[0].state, TraceState::Running);
    assert_eq!(
        snapshot.trace.records[0].timing.duration_ms, None,
        "running is not a duration"
    );
    snapshot.trace.records[0].state = TraceState::OutcomeUnknown;
    repair_live(&mut snapshot);
    assert_eq!(
        snapshot.trace.records[0].state,
        TraceState::OutcomeUnknown,
        "a durable terminal wins over a current label"
    );
}

/// Records outside the newest window are repaired by identity and settle
/// from their own durable terminals at a later cut.
#[test]
#[allow(clippy::too_many_lines)] // Two independent native lifecycles, one history.
fn older_records_are_repaired_and_settle_by_identity() {
    use crate::runtime::identity::{RuntimeResourceRevision, ToolExecutionId};
    use crate::runtime::workflow::read_model::{WorkflowRunView, WorkflowState};
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId};
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::{CapabilityView, RuntimeClientBackgroundExecution};
    use crate::tools::background::BackgroundLifecycle;
    let store = store("conv_7cc9b826-cbd4-78d6-87f0-53569f654a7b");
    let execution = ToolExecutionId::new("exec_6600d36f-a2f9-7057-8735-85a8c316b8af");
    let workflow = WorkflowId::parse("old-workflow").unwrap();
    let run = WorkflowRunId {
        conversation_id: store.conversation_id().clone(),
        attempt_id: AttemptId::new("attempt-a"),
        invocation: 1,
    };
    append(
        &store,
        E::BackgroundExecutionCommitted {
            execution_id: execution.clone(),
            tool_call_id: ToolCallId::new("background-call"),
            tool_id: ToolId::new("tool"),
            tool_name: "tool".into(),
        },
        0,
    );
    append(
        &store,
        E::WorkflowStarted {
            tool_call_id: ToolCallId::new("workflow-call"),
            workflow_id: workflow.clone(),
            run_id: run.clone(),
        },
        1,
    );
    let positions: Vec<_> = page(&store)
        .records
        .iter()
        .map(|record| record.position.clone())
        .collect();
    for n in 2..70 {
        append(&store, E::TurnStarted, n);
    }
    let mut snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    snapshot.background.push(RuntimeClientBackgroundExecution {
        execution_id: execution.clone(),
        tool_id: ToolId::new("tool"),
        tool_name: "tool".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    });
    snapshot.workflows.runs.push(WorkflowRunView {
        id: run.clone(),
        workflow_id: workflow.clone(),
        program_digest: "digest".into(),
        resource_revision: RuntimeResourceRevision::new(1),
        tool_call_id: ToolCallId::new("workflow-call"),
        state: WorkflowState::Running,
        instances: vec![],
        omitted_instances: 0,
        steps_consumed: 0,
        steps_max: 10,
        agents_consumed: 0,
        candidate: None,
        candidate_users: 0,
        handoff: None,
    });
    let projection = TraceProjection::new(&store).unwrap();
    snapshot.trace = projection.page(None, 32).unwrap();
    assert!(
        snapshot
            .trace
            .records
            .iter()
            .all(|record| !positions.contains(&record.position)),
        "the old records are outside the newest window"
    );

    let inactive = projection.refresh(&positions, None).unwrap();
    assert!(
        inactive
            .iter()
            .all(|update| update.state == TraceState::Incomplete),
        "a durable start alone stays incomplete without a live view"
    );
    let live = projection.refresh(&positions, Some(&snapshot)).unwrap();
    assert!(
        live.iter()
            .all(|update| update.state == TraceState::Running)
    );
    assert!(
        live.iter()
            .all(|update| update.timing.duration_ms.is_none())
    );

    append(
        &store,
        E::BackgroundTerminalPublished {
            execution_id: execution,
            message_id: MessageId::new("terminal"),
            state: crate::events::BackgroundTerminalState::Succeeded,
        },
        100,
    );
    append(
        &store,
        E::WorkflowCompleted {
            workflow_id: workflow,
            run_id: run,
        },
        101,
    );
    assert_eq!(
        projection.refresh(&positions, Some(&snapshot)).unwrap(),
        live,
        "the captured cut cannot see later terminals"
    );
    let settled = TraceProjection::new(&store)
        .unwrap()
        .refresh(&positions, Some(&snapshot))
        .unwrap();
    assert_eq!(settled.len(), 2);
    assert!(
        settled
            .iter()
            .all(|update| update.state == TraceState::Completed
                && update.timing.duration_ms == Some(100_000))
    );
}

/// Canonical Tool artifacts merge by identity, first canonical occurrence
/// owning image typing, and display metadata never decides type.
#[test]
fn canonical_tool_artifacts_merge_by_identity_with_first_occurrence_typing() {
    use crate::message::content::{FileReference, ImageReference};
    let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
    start(&store);
    let file = |id: &str| FileReference {
        artifact_id: ArtifactId::new(id),
        name: Some("report.png".into()),
        mime_type: Some("application/octet-stream".into()),
        description: Some("producer metadata".into()),
    };
    for (index, content) in [
        vec![],
        vec![ToolResultContent::Image(ImageReference {
            artifact_id: ArtifactId::new("artifact-one"),
            alt: None,
        })],
    ]
    .into_iter()
    .enumerate()
    {
        let call = ToolCall {
            id: ToolCallId::new(format!("artifact-call-{index}")),
            tool_id: ToolId::new("artifact-tool"),
            name: "files".into(),
            arguments: serde_json::json!({}),
        };
        let owner = format!("artifact-owner-{index}");
        propose_tool_call(&store, &owner, &call, 1);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            2,
        );
        settle_tool_call(
            &store,
            &owner,
            &format!("artifact-result-{index}"),
            &call,
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content,
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![file("artifact-one"), file("artifact-two")],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
            3,
        );
        let projected = page(&store);
        let record = projected
            .records
            .iter()
            .find(|record| {
                record
                    .tool
                    .as_ref()
                    .is_some_and(|view| view.call_id == call.id)
            })
            .unwrap();
        assert_eq!(
            record
                .attachments
                .iter()
                .map(|artifact| artifact.artifact_id.as_str())
                .collect::<Vec<_>>(),
            ["artifact-one", "artifact-two"]
        );
        assert_eq!(
            record.attachments[0].image,
            index == 1,
            "canonical block type owns image typing, not the filename"
        );
        assert!(!record.attachments[1].image);
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One durable scenario checks all bounded canonical categories.
fn identity_bounds_make_canonical_trace_details_explicitly_partial() {
    use crate::message::content::{FileReference, ImageReference};
    for oversized in [false, true] {
        let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
        let identity = if oversized {
            "x".repeat(super::bounds::TRACE_IDENTITY_BYTES + 1)
        } else {
            "valid-identity".to_owned()
        };
        let image = ImageReference {
            artifact_id: ArtifactId::new(&identity),
            alt: None,
        };
        let file = FileReference {
            artifact_id: ArtifactId::new(&identity),
            name: None,
            mime_type: None,
            description: None,
        };
        store
            .accept_inbound(crate::durable::InboundDraft {
                message_id: Some(MessageId::new("bounded-user")),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                content: vec![
                    UserContentBlock::Text(TextBlock {
                        text: "retained".into(),
                    }),
                    UserContentBlock::Image(image.clone()),
                    UserContentBlock::File(file.clone()),
                ],
                timestamp: timestamp(0),
                correlation: None,
            })
            .unwrap();
        let batch = store.select_pending_batch().unwrap().unwrap();
        store
            .adopt_pending_batch(batch.watermark, Some(AttemptId::new("attempt-a")))
            .unwrap();
        append(&store, E::TurnStarted, 1);
        let frozen = request_with_tools(
            &store,
            0,
            Some(vec![ModelToolDefinition {
                id: ToolId::new(&identity),
                name: "historical".into(),
                description: "frozen".into(),
                input_schema: serde_json::json!({}),
            }]),
        );
        let call = bash_call("bounded-call");
        propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 3);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            4,
        );
        settle_tool_call(
            &store,
            frozen.provisional_message_id.as_str(),
            "bounded-result",
            &call,
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![
                    ToolResultContent::Image(image.clone()),
                    ToolResultContent::File(file),
                ],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
            5,
        );
        let assistant_id = MessageId::new("bounded-assistant");
        let mut bounded_call = bash_call(&identity);
        bounded_call.tool_id = ToolId::new(&identity);
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(AssistantMessageBlock {
                    id: assistant_id.clone(),
                    content: vec![
                        AssistantContentBlock::ToolCall(bounded_call),
                        AssistantContentBlock::Image(image),
                    ],
                }),
                event(
                    &store,
                    E::AssistantMessageCommitted {
                        message_id: assistant_id.clone(),
                    },
                    6,
                ),
            )
            .unwrap();
        let projected = page(&store);
        let user = detail_of(&store, &record_of(&projected, TraceKind::User).id);
        assert_eq!(user.messages[0].truncated, oversized);
        assert_eq!(user.messages[0].blocks.len(), if oversized { 1 } else { 3 });
        let request = detail_of(&store, &record_of(&projected, TraceKind::Request).id)
            .request
            .unwrap();
        assert_eq!(request.tools_truncated, oversized);
        assert_eq!(request.tools.len(), usize::from(!oversized));
        assert_eq!(request.messages[0].truncated, oversized);
        let tool_detail = detail_of(&store, &record_of(&projected, TraceKind::Tool).id);
        assert!(!tool_detail.truncated);
        let tool = tool_detail.tool.unwrap();
        let tool = tool.result.unwrap();
        assert_eq!(tool.blocks_truncated, oversized);
        assert_eq!(tool.blocks.len(), if oversized { 0 } else { 2 });
        let record = projected
            .records
            .iter()
            .find(|record| record.message_id.as_ref() == Some(&assistant_id))
            .unwrap();
        assert_eq!(record.truncated, oversized);
        let assistant = detail_of(&store, &record.id);
        assert_eq!(assistant.messages[0].truncated, oversized);
        assert_eq!(
            assistant.messages[0].blocks.len(),
            if oversized { 0 } else { 2 }
        );
    }
}

#[test]
fn managed_storage_failure_diagnostics_preserve_native_execution_facts() {
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::output::{ForegroundOutputCapture, continuation_for_capture};
    let private_path = "/private/rustx-managed-output/example/tasks/result.output";
    for available in [false, true] {
        let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
        let directory = tempfile::tempdir().unwrap();
        let output = ManagedToolOutput::new(
            store.conversation_id().clone(),
            directory.path().join(private_path.trim_start_matches('/')),
        )
        .unwrap();
        if available {
            output.fail_writes_after(0);
        } else {
            // A real allocation error includes the private results directory.
            let results = output.root().join("results");
            std::fs::remove_dir(&results).unwrap();
            std::fs::write(&results, "not a directory").unwrap();
            assert!(
                output
                    .open_spill()
                    .unwrap_err()
                    .to_string()
                    .contains(private_path)
            );
        }
        let mut capture = ForegroundOutputCapture::with_limit(1);
        let diagnostic = capture.push("retained output", &output).unwrap_err();
        let captured = capture.finish(false);
        start(&store);
        let call = bash_call("storage-failure");
        propose_tool_call(&store, "storage-owner", &call, 2);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            3,
        );
        settle_tool_call(
            &store,
            "storage-owner",
            "storage-result",
            &call,
            ToolExecutionResult {
                status: ToolExecutionStatus::Failed {
                    error: diagnostic.clone(),
                },
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: continuation_for_capture(&captured, available, Some(&diagnostic)),
            },
            4,
        );
        let detail = detail_of(&store, &record_of(&page(&store), TraceKind::Tool).id);
        let result = detail.tool.as_ref().unwrap().result.as_ref().unwrap();
        assert_eq!(result.outcome, TraceToolOutcome::Failed);
        assert_eq!(result.detail.as_ref().unwrap().text, diagnostic);
        let managed = result.managed_output.as_ref().unwrap();
        assert!(!managed.complete);
        assert_eq!(managed.available, available);
        assert_eq!(managed.diagnostic.as_ref().unwrap().text, diagnostic);
        assert_eq!(managed.locator, captured.output_locator);
        if available {
            assert!(diagnostic.contains("test-forced output write failure"));
        } else {
            assert!(diagnostic.contains(private_path));
            assert!(
                serde_json::to_string(&detail)
                    .unwrap()
                    .contains(private_path)
            );
        }
    }
}

#[test]
fn mandatory_tool_identities_bound_whole_details_and_historical_result_blocks() {
    for oversized in [None, Some("call"), Some("tool")] {
        let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
        start(&store);
        let sentinel = "opaque-identity-".repeat(super::bounds::TRACE_IDENTITY_BYTES);
        let mut call = bash_call("ordinary-call");
        if oversized == Some("call") {
            call.id = ToolCallId::new(&sentinel);
        }
        if oversized == Some("tool") {
            call.tool_id = ToolId::new(&sentinel);
        }
        let frozen = request(&store, 0);
        propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 3);
        completion(&store, &frozen, None, None, 4);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            5,
        );
        settle_tool_call(
            &store,
            frozen.provisional_message_id.as_str(),
            "identity-result",
            &call,
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 1,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
            6,
        );
        let next = request(&store, 1);
        let projected = page(&store);
        let detail = detail_of(&store, &record_of(&projected, TraceKind::Tool).id);
        assert_eq!(detail.kind, TraceKind::Tool);
        assert_eq!(detail.truncated, oversized.is_some());
        assert_eq!(detail.tool.is_none(), oversized.is_some());
        assert!(!serde_json::to_string(&detail).unwrap().contains(&sentinel));
        let record = projected
            .records
            .iter()
            .find(|r| {
                r.request
                    .as_ref()
                    .is_some_and(|r| r.request_id == next.request_id)
            })
            .unwrap();
        let detail = detail_of(&store, &record.id);
        let message = detail
            .request
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .find(|m| m.role == TraceMessageRole::Tool)
            .unwrap();
        assert_eq!(message.truncated, oversized.is_some());
        assert_eq!(message.blocks.len(), usize::from(oversized.is_none()));
        assert!(!serde_json::to_string(&detail).unwrap().contains(&sentinel));
        if let Some(tool) = detail_of(&store, &record_of(&projected, TraceKind::Tool).id).tool {
            assert_eq!(tool.call_id, call.id);
            assert_eq!(tool.tool_id, call.tool_id);
        }
    }
}

#[test]
fn managed_output_locators_and_diagnostics_are_exact_recorded_facts() {
    use crate::tools::types::ManagedOutputContinuation as M;
    let locator =
        std::path::PathBuf::from("/private/rustx-managed-output/example/tasks/result.output");
    let diagnostic = format!(
        "cannot append {}: recorded storage failure",
        locator.display()
    );
    for continuation in [
        M::Complete {
            locator: locator.clone(),
        },
        M::Partial {
            locator: locator.clone(),
            diagnostic: diagnostic.clone(),
        },
        M::Unavailable {
            diagnostic: diagnostic.clone(),
        },
    ] {
        let complete = matches!(continuation, M::Complete { .. });
        let available = !matches!(continuation, M::Unavailable { .. });
        let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
        start(&store);
        let call = bash_call("locator-call");
        propose_tool_call(&store, "locator-owner", &call, 2);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            3,
        );
        settle_tool_call(
            &store,
            "locator-owner",
            "locator-result",
            &call,
            ToolExecutionResult {
                status: ToolExecutionStatus::Failed {
                    error: diagnostic.clone(),
                },
                content: vec![],
                duration_ms: 1,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: Some(continuation),
            },
            4,
        );
        let detail = detail_of(&store, &record_of(&page(&store), TraceKind::Tool).id);
        let result = detail.tool.as_ref().unwrap().result.as_ref().unwrap();
        let output = result.managed_output.as_ref().unwrap();
        assert_eq!(output.complete, complete);
        assert_eq!(output.available, available);
        assert_eq!(output.locator.as_ref(), available.then_some(&locator));
        assert_eq!(
            output.diagnostic.as_ref().map(|d| d.text.as_str()),
            (!complete).then_some(diagnostic.as_str())
        );
        assert_eq!(result.detail.as_ref().unwrap().text, diagnostic);
        let wire = serde_json::to_value(&detail).unwrap();
        assert_eq!(
            wire["tool"]["result"]["managed_output"]["locator"],
            if available {
                serde_json::json!(locator)
            } else {
                serde_json::Value::Null
            }
        );
    }
}

#[test]
fn mandatory_request_identities_bound_the_entire_request_detail() {
    let bound = super::bounds::TRACE_IDENTITY_BYTES;
    // Includes derived Request/Assistant identities that exceed the bound
    // even though their individual Attempt and Step components fit.
    for (attempt_len, step_len) in [(bound + 1, 1), (1, bound + 1), (bound / 2, bound / 2)] {
        let store = store("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38");
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("a".repeat(attempt_len)),
            turn: TurnId::new("s".repeat(step_len)),
            retry_number: 0,
        };
        let snapshot = request_with_identity(&store, 0, None, identity);
        let record = record_of(&page(&store), TraceKind::Request).clone();
        let detail = detail_of(&store, &record.id);
        assert!(detail.truncated);
        assert!(detail.request.is_none());
        assert!(
            !serde_json::to_string(&detail)
                .unwrap()
                .contains(snapshot.request_id.as_str())
        );
    }
}

// ---------------------------------------------------------------------------
// Request-relative presentation facts (#372)
//
// Three relationships that a browser must never discover for itself. Every
// scenario below establishes the relationship through the same durable
// transitions production uses, then asserts what the server resolved — never
// what a client could have inferred from the rows it happened to load.
// ---------------------------------------------------------------------------

/// Builds one canonical admitted Context fact, as Context Assembly commits
/// it: the provenance is stated explicitly, because provenance is exactly
/// what the Context presentation contract is about. A fixture that stamped
/// every fact `Runtime` could not tell a native fact apart from an
/// extension's, which is the distinction Trace has to preserve.
fn context_message(id: &str, source: UserSource, kind: ContextKind, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
        source,
        kind: InboundKind::Context(kind),
        timestamp: None,
    })
}

/// One native runtime-owned Context fact. Context Assembly assigns
/// `UserSource::Runtime` to every native contributor lane.
fn runtime_context(id: &str, kind: ContextKind, text: &str) -> MessageBlock {
    context_message(id, UserSource::Runtime, kind, text)
}

/// One certified extension's Context fact, with the exact contributor
/// identity rustX assigns at admission. The extension lane always produces
/// `ContextKind::ExtensionEnvironment`, so the family alone can never
/// identify which extension produced the fact.
fn extension_context(id: &str, extension: &str, text: &str) -> MessageBlock {
    context_message(
        id,
        UserSource::Extension {
            contributor: CertifiedExtensionIdentity::new(extension).expect("extension identity"),
        },
        ContextKind::ExtensionEnvironment,
        text,
    )
}

/// The exact certified-extension provenance of one projected Context fact.
fn extension_source(extension: &str) -> TraceContextSource {
    TraceContextSource::CertifiedExtension {
        contributor: CertifiedExtensionIdentity::new(extension).expect("extension identity"),
    }
}

fn goal_status(objective: &str) -> ContextKind {
    ContextKind::GoalStatus(Box::new(crate::goal::GoalSnapshot {
        reference: crate::goal::GoalRef {
            id: "goal-1".into(),
            revision: 3,
        },
        objective: objective.to_owned(),
        phase: crate::goal::GoalPhase::Active,
        blocked_reason: None,
        autonomous_round_budget: 8,
        autonomous_rounds_consumed: 2,
        origin: crate::goal::GoalOrigin::RuntimeControl,
        last_round_message_id: None,
    }))
}

fn identity_of(turn: &str, retry: u32) -> RequestIdentity {
    RequestIdentity {
        attempt_id: AttemptId::new("attempt-a"),
        turn: TurnId::new(turn),
        retry_number: retry,
    }
}

/// Commits one actual request with an exact frozen prompt and exact frozen
/// request-scoped context identities.
fn request_with(
    store: &dyn ConversationStore,
    turn: &str,
    retry: u32,
    prompt: &str,
    context: &[MessageBlock],
) -> RequestSnapshot {
    let mut prepared = prepared_request(store, retry, None, identity_of(turn, retry));
    prompt.clone_into(&mut prepared.effective_system_prompt);
    prepared.request_context_ids = context.iter().map(|message| message.id().clone()).collect();
    prepared.contributions = context
        .iter()
        .filter_map(|message| {
            let MessageBlock::User(user) = message else {
                return None;
            };
            let InboundKind::Context(metadata) = &user.kind else {
                return None;
            };
            let producer = match &user.source {
                UserSource::Runtime => {
                    crate::runtime::identity::ContextContributorIdentity::Native(
                        metadata.native_contribution_owner()?.0,
                    )
                }
                UserSource::Extension { contributor } => {
                    crate::runtime::identity::ContextContributorIdentity::CertifiedExtension(
                        contributor.clone(),
                    )
                }
                _ => return None,
            };
            prepared
                .context_generation
                .contributors
                .push(crate::context::ContributorGeneration {
                    identity: producer.clone(),
                    attestation: None,
                });
            Some(crate::model::ContributionStart {
                message_id: user.id.clone(),
                producer,
                metadata: metadata.clone(),
                presentation: None,
                emissions: vec![],
                opportunities: crate::context::ContributionOpportunities::default(),
                post_tool_batch_anchor: None,
            })
        })
        .collect();
    commit_request(store, prepared, context)
}

fn system_of(page: &TracePage, index: usize) -> TraceSystemPromptPresentation {
    page.records
        .iter()
        .filter(|record| record.kind == TraceKind::Request)
        .nth(index)
        .expect("a request record")
        .request
        .as_ref()
        .expect("a request summary")
        .system_prompt
        .clone()
}

fn context_of(page: &TracePage, index: usize) -> Vec<TraceContextPresentation> {
    page.records
        .iter()
        .filter(|record| record.kind == TraceKind::Request)
        .nth(index)
        .expect("a request record")
        .request
        .as_ref()
        .expect("a request summary")
        .context_additions
        .clone()
}

/// §13.1–3: the classification is resolved against the nearest preceding
/// actual request, by exact historical value equality of two frozen prompts.
#[test]
fn system_prompt_state_follows_the_previous_actual_request() {
    let store = store("conv_5f1d0f60-2f4e-7a11-9b02-6b8cf1a0d301");
    start(&store);
    let first = request_with(&store, "1", 0, "prompt-A", &[]);
    completion(&store, &first, None, None, 4);
    let second = request_with(&store, "1", 1, "prompt-A", &[]);
    completion(&store, &second, None, None, 6);
    let third = request_with(&store, "1", 2, "prompt-B", &[]);
    completion(&store, &third, None, None, 8);

    let projected = page(&store);
    assert_eq!(
        system_of(&projected, 0).state,
        TraceSystemPromptState::Initial
    );
    assert_eq!(
        system_of(&projected, 1).state,
        TraceSystemPromptState::Unchanged
    );
    assert_eq!(
        system_of(&projected, 2).state,
        TraceSystemPromptState::Changed
    );
    // `Unchanged` does not repeat a preview the preceding row already carries.
    assert!(system_of(&projected, 1).preview.is_none());
    assert_eq!(
        system_of(&projected, 0).preview.unwrap().text,
        "prompt-A",
        "the request that introduced the prompt previews it"
    );
    assert_eq!(system_of(&projected, 2).preview.unwrap().text, "prompt-B");
}

/// §13.7: a fresh logical Step opens at `retry_number == 0`, which is not
/// evidence that no earlier actual request exists.
#[test]
fn a_fresh_step_at_retry_zero_is_still_compared_with_the_previous_request() {
    let store = store("conv_61ab8f2c-9d7a-7c41-8f13-2a9c0e5f1b22");
    start(&store);
    let first = request_with(&store, "1", 0, "prompt-A", &[]);
    completion(&store, &first, None, None, 4);
    append(&store, E::TurnCompleted, 5);
    append_in_step(&store, E::TurnStarted, 6, "2");
    let second = request_with(&store, "2", 0, "prompt-B", &[]);
    append_in_step(
        &store,
        E::ModelRequestCompleted {
            request_id: second.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
        9,
        "2",
    );

    let projected = page(&store);
    assert_eq!(second.identity.retry_number, 0);
    assert_eq!(
        system_of(&projected, 1).state,
        TraceSystemPromptState::Changed,
        "a new Step's initial request still has a previous actual request"
    );
}

/// §13.4: page boundaries are a client's view, not native authority. A page
/// that starts after the predecessor reports exactly what a whole page does.
#[test]
fn a_page_boundary_cannot_turn_changed_into_initial() {
    let store = store("conv_7c4f2b80-1e55-7d20-8a64-0cb4e2d7f911");
    start(&store);
    let first = request_with(&store, "1", 0, "prompt-A", &[]);
    completion(&store, &first, None, None, 4);
    let second = request_with(&store, "1", 1, "prompt-B", &[]);
    completion(&store, &second, None, None, 6);

    let whole = page(&store);
    let read = TraceProjection::new(&store).unwrap();
    // One record at a time, so the newest page contains no predecessor row.
    let newest = read.page(None, 1).unwrap();
    assert_eq!(newest.records.len(), 1);
    let only = newest.records[0]
        .request
        .as_ref()
        .expect("the newest row is the second request");
    assert_eq!(only.request_id, second.request_id);
    assert_eq!(
        only.system_prompt.state,
        TraceSystemPromptState::Changed,
        "the predecessor is outside this page, not outside history"
    );
    assert_eq!(only.system_prompt, system_of(&whole, 1));
}

/// §13.5–6: the classification is a historical fact. Reopening the durable
/// store reproduces it, and later requests with other prompts — the shape a
/// reconfigured Session produces — never rewrite an older row.
#[test]
fn historical_system_classification_survives_reopen_and_later_requests() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_9b2e77c4-53a0-7ef1-8d26-41ba7c0e5d38");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let first = request_with(&store, "1", 0, "prompt-A", &[]);
    completion(&store, &first, None, None, 4);
    let second = request_with(&store, "1", 1, "prompt-B", &[]);
    completion(&store, &second, None, None, 6);
    let historical = page(&store);

    // A later request froze a third prompt, exactly as a reconfigured System
    // contributor generation would. It is a new row, not a rewrite.
    let third = request_with(&store, "1", 2, "prompt-C", &[]);
    completion(&store, &third, None, None, 8);
    let extended = page(&store);
    assert_eq!(system_of(&extended, 0), system_of(&historical, 0));
    assert_eq!(system_of(&extended, 1), system_of(&historical, 1));
    assert_eq!(
        system_of(&extended, 2).state,
        TraceSystemPromptState::Changed
    );

    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(page(&reopened).records, extended.records);
}

/// §13.8, 13.10–12: identity, order, provenance and semantic family all come
/// from the immutable snapshot and keyed Ledger reads.
#[test]
fn canonical_context_is_projected_from_the_frozen_request_identities() {
    let store = store("conv_2d90c1af-6b34-7a05-8e77-9f1c4b6a2e50");
    start(&store);
    let context = [
        runtime_context("ctx-goal", goal_status("ship the release"), "Goal: active"),
        runtime_context(
            "ctx-observation",
            ContextKind::RuntimeToolObservation,
            "The tool batch settled.",
        ),
        extension_context(
            "ctx-environment",
            "vendor.observability",
            "Environment facts.",
        ),
    ];
    let frozen = request_with(&store, "1", 0, "prompt-A", &context);
    completion(&store, &frozen, None, None, 8);

    let additions = context_of(&page(&store), 0);
    assert_eq!(
        additions
            .iter()
            .map(|addition| addition.message_id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["ctx-goal", "ctx-observation", "ctx-environment"],
        "the frozen snapshot order is preserved exactly"
    );
    assert_eq!(
        additions
            .iter()
            .map(|addition| addition.context_kind)
            .collect::<Vec<_>>(),
        vec![
            TraceContextKind::GoalStatus,
            TraceContextKind::RuntimeToolObservation,
            TraceContextKind::ExtensionEnvironment,
        ]
    );
    assert_eq!(
        additions
            .iter()
            .map(|addition| addition.source.clone())
            .collect::<Vec<_>>(),
        vec![
            TraceContextSource::Runtime,
            TraceContextSource::Runtime,
            extension_source("vendor.observability"),
        ],
        "provenance is the canonical message's own UserSource, not the family"
    );
    for addition in &additions {
        assert!(!addition.truncated);
        assert!(addition.attachments.is_empty());
    }
    assert_eq!(additions[0].preview.as_ref().unwrap().text, "Goal: active");
    // The internal ContextKind payload stays inside the runtime: a family
    // name crosses, a complete GoalSnapshot does not.
    let wire = serde_json::to_string(&page(&store)).unwrap();
    assert!(!wire.contains("ship the release"));
    assert!(!wire.contains("autonomous_round_budget"));
}

/// §13.9: the Context Engine admits request context once, at the first
/// successful start. Retry and recovery requests reuse it, so their own
/// snapshots list none and Trace re-emits nothing. No Trace-local "already
/// displayed" state exists to get this right or wrong.
#[test]
fn retry_and_recovery_requests_introduce_no_duplicate_context() {
    let store = store("conv_4e77b013-8c2a-7f39-9d54-3b6a1c8e70df");
    start(&store);
    let introduced = [runtime_context(
        "ctx-observation",
        ContextKind::RuntimeToolObservation,
        "The tool batch settled.",
    )];
    let first = request_with(&store, "1", 0, "prompt-A", &introduced);
    failure(&store, &first, ModelErrorKind::Transport);
    let retry = request_with(&store, "1", 1, "prompt-A", &[]);
    failure(&store, &retry, ModelErrorKind::ContextWindowExceeded);
    let recovery = request_with(&store, "1", 2, "prompt-A", &[]);
    completion(&store, &recovery, None, None, 10);

    let projected = page(&store);
    assert_eq!(context_of(&projected, 0).len(), 1);
    assert!(context_of(&projected, 1).is_empty());
    assert!(context_of(&projected, 2).is_empty());
    assert_eq!(
        serde_json::to_string(&projected)
            .unwrap()
            .matches("ctx-observation")
            .count(),
        1,
        "the identity appears at exactly one request boundary"
    );
}

/// §13.13–14: Trace states what the authorities recorded. Compaction is not
/// a Context-removal fact, and request-only input has no canonical identity
/// to promote into one.
#[test]
fn compaction_and_request_only_input_never_become_context_facts() {
    use crate::model::input::{
        CarryoverBlockKind, CarryoverOmissionCounts, RenderedCarryoverRecord,
        RenderedCarryoverText, RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor,
        UnresolvedOutputSettlement,
    };
    let store = store("conv_8a15d3e9-70cb-7c62-8b90-5e24f7c1a063");
    start(&store);
    let introduced = [extension_context(
        "ctx-environment",
        "vendor.observability",
        "Environment facts.",
    )];
    let first = request_with(&store, "1", 0, "prompt-A", &introduced);
    completion(&store, &first, None, None, 4);
    append(&store, E::CompactionStarted, 5);

    // A request whose only extra input is request-only carryover: it has no
    // canonical MessageId, so it is request detail and nothing else.
    let stream = crate::runtime::identity::PublicationStreamId::new("stream-1");
    let mut prepared = prepared_request(&store, 1, None, identity_of("1", 1));
    "prompt-A".clone_into(&mut prepared.effective_system_prompt);
    prepared.unresolved_output_carryover_source = Some(stream.clone());
    prepared.unresolved_output_carryover = Some(RenderedUnresolvedOutputCarryover {
        source_stream_id: stream,
        source_settlement: UnresolvedOutputSettlement::Incomplete,
        records: vec![RenderedCarryoverRecord::Text(RenderedCarryoverText {
            kind: CarryoverBlockKind::Text,
            text: Some("unresolved-output-carryover".to_owned()),
            omitted_prefix_bytes: 0,
            omitted_detail_bytes: 0,
        })],
        omitted_blocks: CarryoverOmissionCounts::default(),
    });
    prepared.unresolved_output_carryover_anchor = Some(RequestOnlyInsertionAnchor::AfterCanonical);
    let second = commit_request(&store, prepared, &[]);
    completion(&store, &second, None, None, 9);

    let projected = page(&store);
    assert_eq!(context_of(&projected, 0).len(), 1);
    assert!(
        context_of(&projected, 1).is_empty(),
        "request-only carryover is never canonical Context presentation"
    );
    // It is still reachable where it belongs: as a request-only item of the
    // heavy request detail, with no canonical identity attached to it.
    let detail = detail_of(
        &store,
        &projected
            .records
            .iter()
            .filter(|record| record.kind == TraceKind::Request)
            .nth(1)
            .unwrap()
            .id,
    );
    let carryover = detail
        .request
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|message| message.role == TraceMessageRole::RequestOnly)
        .expect("the request-only item remains request detail");
    assert!(carryover.message_id.is_none());
    // Compaction contributes a record, never a Context fact of its own.
    let compaction = record_of(&projected, TraceKind::Compaction);
    assert!(compaction.request.is_none());
    assert_eq!(
        serde_json::to_string(&projected)
            .unwrap()
            .matches("ctx-environment")
            .count(),
        1
    );
}

/// §13.15–16: a Context presentation is reproducible and bounded. An
/// adversarial payload shortens the list deterministically, in native order,
/// and never invents or shortens an identity.
#[test]
fn context_presentation_is_reproducible_and_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_ce31f6a8-2b74-7d09-8a15-6c0fb3e94d27");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let context: Vec<MessageBlock> = (0..(TRACE_SUMMARY_CONTEXT + 8))
        .map(|index| {
            extension_context(
                &format!("ctx-{index:03}"),
                "vendor.observability",
                &"E".repeat(8_000),
            )
        })
        .collect();
    let frozen = request_with(&store, "1", 0, "prompt-A", &context);
    completion(&store, &frozen, None, None, 8);

    let projected = page(&store);
    let record = record_of(&projected, TraceKind::Request);
    let summary = record.request.as_ref().unwrap();
    assert!(summary.context_truncated);
    assert!(!summary.context_additions.is_empty());
    assert!(
        serde_json::to_vec(&summary.context_additions)
            .unwrap()
            .len()
            <= TRACE_SUMMARY_CONTEXT_BYTES
    );
    assert!(serde_json::to_vec(record).unwrap().len() <= TRACE_RECORD_BYTES);
    assert!(serde_json::to_vec(&projected).unwrap().len() <= TRACE_PAGE_BYTES);
    // A deterministic prefix in frozen order: never a sampled or reordered
    // subset, and never a synthetic replacement identity.
    for (index, addition) in summary.context_additions.iter().enumerate() {
        assert_eq!(addition.message_id.as_str(), format!("ctx-{index:03}"));
    }
    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(page(&reopened).records, projected.records);
}

/// An identity longer than the Trace identity bound is omitted whole. It is
/// never shortened into a different identity that refers to nothing.
#[test]
fn an_oversized_context_identity_is_omitted_whole() {
    let store = store("conv_0f6b52d7-9a48-7bc3-8e01-74d3fa26c519");
    start(&store);
    let oversized = "x".repeat(super::bounds::TRACE_IDENTITY_BYTES + 1);
    let context = [
        extension_context(&oversized, "vendor.observability", "Oversized."),
        extension_context("ctx-kept", "vendor.observability", "Kept."),
    ];
    let frozen = request_with(&store, "1", 0, "prompt-A", &context);
    completion(&store, &frozen, None, None, 8);

    let projected = page(&store);
    let summary = record_of(&projected, TraceKind::Request)
        .request
        .as_ref()
        .unwrap()
        .clone();
    assert!(summary.context_truncated);
    assert_eq!(summary.context_additions.len(), 1);
    assert_eq!(summary.context_additions[0].message_id.as_str(), "ctx-kept");
    assert!(
        !serde_json::to_string(&projected)
            .unwrap()
            .contains(&oversized[..64])
    );
}

/// Request-scoped context that is not an admitted Context fact never becomes
/// durable in the first place: the start transition rejects it, so no Trace
/// read can encounter one. Trace keeps its own typed guard for the same
/// invariant rather than downgrading such a message into an ordinary User
/// message, but the durable authority is what makes the case unreachable.
#[test]
fn request_scoped_context_that_is_not_an_admitted_context_fact_never_commits() {
    let store = store("conv_36c0a4e1-8f27-7d5a-9b34-1e78cd05f2a6");
    start(&store);
    let ordinary = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("not-context"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "An ordinary inbound message.".into(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    let mut prepared = prepared_request(&store, 0, None, identity_of("1", 0));
    prepared.request_context_ids = vec![MessageId::new("not-context")];
    let error = store
        .commit_model_turn_start(std::slice::from_ref(&ordinary), &prepared, timestamp(2))
        .expect_err("the durable start transition rejects it");
    assert!(matches!(
        error,
        ConversationStoreError::InvalidReference(ref detail)
            if detail.contains("canonical context has no accepted contribution record")
    ));
    // Nothing committed, so the conversation still has no request at all.
    assert!(
        !page(&store)
            .records
            .iter()
            .any(|record| record.kind == TraceKind::Request)
    );
}

/// §13.17–20: Background, Subagent and Workflow records carry the exact
/// outer `ToolCallId` their own native start fact froze. Reused Tool, Agent
/// and Workflow names cannot cross-correlate them.
#[test]
fn tool_owned_domains_carry_their_exact_originating_tool_call() {
    let store = store("conv_a7f34b28-5c91-7e06-8d42-3fb8c0e17d54");
    start(&store);
    append(
        &store,
        E::BackgroundExecutionCommitted {
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_6600d36f-a2f9-7057-8735-85a8c316b8af",
            ),
            tool_call_id: ToolCallId::new("call-background"),
            tool_id: ToolId::new("tool-shared"),
            tool_name: "shared".into(),
        },
        2,
    );
    let subagent =
        crate::runtime::identity::SubagentId::new("sub_1f0a7d4c-5b28-7e19-8a63-90cf2d8b4e17");
    let mut ownership = event(
        &store,
        E::SubagentOwnershipCommitted {
            subagent_id: subagent.clone(),
            child_agent_id: crate::runtime::identity::AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new("conv_1d5e2a90-7b41-7c38-8a02-64f0be93c175"),
            tool_call_id: ToolCallId::new("call-subagent"),
            agent: "shared".into(),
            definition_digest: "digest".into(),
            profile_digest: "profile".into(),
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            workspace: crate::runtime::workspace::WorkspaceSnapshot {
                borrowed_from: None,
                logical_workspace: std::path::PathBuf::from("/workspace"),
                isolation: crate::runtime::workspace::WorkspaceIsolation::Shared,
            },
        },
        3,
    );
    // The durable contract derives this event's canonical identity from the
    // subagent it opens; ownership is not an ordinary standalone fact.
    ownership.event_id = EventId::new(format!("subagent-committed-event:{subagent}"));
    store.append_event(ownership).unwrap();
    append(
        &store,
        E::WorkflowStarted {
            tool_call_id: ToolCallId::new("call-workflow"),
            workflow_id: crate::runtime::workflow::WorkflowId::parse("shared").unwrap(),
            run_id: crate::runtime::workflow::WorkflowRunId {
                conversation_id: store.conversation_id().clone(),
                attempt_id: AttemptId::new("attempt-a"),
                invocation: 1,
            },
        },
        4,
    );

    let projected = page(&store);
    let correlated = |kind| {
        record_of(&projected, kind)
            .originating_tool_call_id
            .clone()
            .map(|id| id.as_str().to_owned())
    };
    assert_eq!(
        correlated(TraceKind::Background).as_deref(),
        Some("call-background")
    );
    assert_eq!(
        correlated(TraceKind::Subagent).as_deref(),
        Some("call-subagent")
    );
    assert_eq!(
        correlated(TraceKind::Workflow).as_deref(),
        Some("call-workflow")
    );
    // Every other kind carries none: the relation exists only where a native
    // start fact recorded it.
    for record in &projected.records {
        if !matches!(
            record.kind,
            TraceKind::Background | TraceKind::Subagent | TraceKind::Workflow
        ) {
            assert!(record.originating_tool_call_id.is_none());
        }
    }
}

/// §13.21–22: the correlation is a frozen native fact, so it survives both a
/// page that excludes the parent Tool row and a lifecycle refresh — and a
/// lifecycle refresh never carries immutable presentation data at all.
#[test]
fn tool_correlation_survives_paging_and_lifecycle_refresh() {
    let store = store("conv_b0925de4-4a13-7f87-8c56-2d7e9a0b1f64");
    start(&store);
    let frozen = request_with(
        &store,
        "1",
        0,
        "prompt-A",
        &[extension_context(
            "ctx-environment",
            "vendor.observability",
            "Environment facts.",
        )],
    );
    let call = bash_call("call-parent");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 4);
    completion(&store, &frozen, None, None, 4);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        5,
    );
    append(
        &store,
        E::BackgroundExecutionCommitted {
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_6600d36f-a2f9-7057-8735-85a8c316b8af",
            ),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            tool_name: "bash".into(),
        },
        6,
    );

    let read = TraceProjection::new(&store).unwrap();
    let newest = read.page(None, 1).unwrap();
    let background = &newest.records[0];
    assert_eq!(background.kind, TraceKind::Background);
    assert!(
        !newest
            .records
            .iter()
            .any(|record| record.kind == TraceKind::Tool),
        "the parent Tool row is outside this page"
    );
    assert_eq!(
        background
            .originating_tool_call_id
            .as_ref()
            .map(ToString::to_string),
        Some("call-parent".to_owned())
    );

    // Lifecycle refresh carries mutable lifecycle facts only.
    let updates = read
        .refresh(
            &page(&store)
                .records
                .iter()
                .map(|record| record.position.clone())
                .collect::<Vec<_>>(),
            None,
        )
        .unwrap();
    let wire = serde_json::to_string(&updates).unwrap();
    for immutable in [
        "originating_tool_call_id",
        "system_prompt",
        "context_additions",
        "ctx-environment",
    ] {
        assert!(
            !wire.contains(immutable),
            "a lifecycle update must not repeat {immutable}"
        );
    }
    assert_eq!(
        page(&store)
            .records
            .iter()
            .find(|record| record.kind == TraceKind::Background)
            .and_then(|record| record.originating_tool_call_id.clone())
            .map(|id| id.as_str().to_owned()),
        Some("call-parent".to_owned()),
        "a refresh changes no immutable relation"
    );
}

// ---------------------------------------------------------------------------
// Read-responsibility ownership (#372 revision)
//
// Immutable historical presentation and mutable lifecycle repair are separate
// responsibilities. The regressions below prove that as an implementation
// property — deterministic counters around the two immutable relationship
// paths — rather than inferring it from how long a refresh took.
// ---------------------------------------------------------------------------

/// A current-runtime snapshot with no running attempt, so live repair is
/// exercised without refining any durable state.
fn quiet_snapshot(
    store: &dyn ConversationStore,
) -> crate::runtime_client::snapshot::RuntimeClientSnapshot {
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::CapabilityView;
    RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0
}

/// Commits one actual request that owns both immutable relationships: a
/// System Prompt to classify against its predecessor, and canonical Context
/// of its own.
fn related_request(store: &dyn ConversationStore, retry: u32, prompt: &str) -> RequestSnapshot {
    let context = [
        runtime_context(
            &format!("ctx-{retry}-observation"),
            ContextKind::RuntimeToolObservation,
            "The tool batch settled.",
        ),
        extension_context(
            &format!("ctx-{retry}-environment"),
            "vendor.observability",
            "Environment facts.",
        ),
    ];
    let frozen = request_with(store, &(retry + 1).to_string(), 0, prompt, &context);
    completion(store, &frozen, None, None, 3 + i64::from(retry) * 2);
    frozen
}

/// A lifecycle refresh resolves no immutable relationship.
///
/// Building a summary page runs both relationship paths; refreshing the same
/// record runs neither. This is not a performance note: a refresh may cover
/// `TRACE_RECORD_LIMIT` records, and an error reached only while resolving a
/// request's Context presentation must not be able to make that record's
/// lifecycle repair unavailable. A path that is never entered cannot fail.
#[test]
fn a_lifecycle_refresh_resolves_no_immutable_presentation_relationship() {
    use super::summary::probe;
    let store = store("conv_7c1a0b52-3d68-7e41-9a07-2f5b8d6e04c3");
    start(&store);
    related_request(&store, 0, "prompt-A");
    let frozen = related_request(&store, 1, "prompt-B");
    let projection = TraceProjection::new(&store).unwrap();

    // 1. A summary page resolves both relationships, exactly as it must.
    probe::reset();
    let page = projection.page(None, TRACE_PAGE_LIMIT).unwrap();
    let (system, context) = probe::counts();
    assert_eq!(system, 2, "each request row classifies its own predecessor");
    assert_eq!(context, 2, "each request row joins its own frozen Context");
    let record = page
        .records
        .iter()
        .filter(|record| record.kind == TraceKind::Request)
        .nth(1)
        .expect("the second request row");
    let summary = record.request.as_ref().expect("a request summary");
    assert_eq!(summary.system_prompt.state, TraceSystemPromptState::Changed);
    assert_eq!(summary.context_additions.len(), 2);

    // 2. The same record's lifecycle resolves neither.
    probe::reset();
    let updates = projection
        .refresh(
            std::slice::from_ref(&record.position),
            Some(&quiet_snapshot(&store)),
        )
        .unwrap();
    assert_eq!(
        probe::counts(),
        (0, 0),
        "lifecycle refresh entered an immutable presentation path"
    );

    // 3. And the lifecycle it produced is still correct.
    assert_eq!(updates.len(), 1);
    let update = &updates[0];
    assert_eq!(update.id, record.id);
    assert_eq!(update.state, TraceState::Completed);
    assert_eq!(update.timing, record.timing);
    assert!(
        update
            .request
            .as_ref()
            .expect("a request outcome")
            .failure_kind
            .is_none()
    );
    let wire = serde_json::to_string(update).unwrap();
    for immutable in [
        "system_prompt",
        "context_additions",
        "certified_extension",
        frozen.effective_system_prompt.as_str(),
    ] {
        assert!(!wire.contains(immutable), "lifecycle carried {immutable}");
    }
}

/// The relationship paths stay unentered for every retained cursor, so a
/// larger interest set cannot reintroduce O(N) immutable reconstruction.
#[test]
fn refreshing_many_request_cursors_reconstructs_no_immutable_presentation() {
    use super::summary::probe;
    let store = store("conv_3b8e46d1-5f70-7c29-8d63-4a1e9c70b528");
    start(&store);
    for retry in 0..12 {
        related_request(&store, retry, &format!("prompt-{retry}"));
    }
    let projection = TraceProjection::new(&store).unwrap();
    let cursors: Vec<TraceCursor> = projection
        .page(None, TRACE_PAGE_LIMIT)
        .unwrap()
        .records
        .iter()
        .filter(|record| record.kind == TraceKind::Request)
        .map(|record| record.position.clone())
        .collect();
    assert_eq!(cursors.len(), 12);

    probe::reset();
    let updates = projection
        .refresh(&cursors, Some(&quiet_snapshot(&store)))
        .unwrap();
    assert_eq!(updates.len(), 12);
    assert_eq!(
        probe::counts(),
        (0, 0),
        "a wider interest set reintroduced immutable reconstruction"
    );
    // The 512-record interest bound still governs the refreshed set.
    assert!(
        projection
            .refresh(
                &vec![cursors[0].clone(); TRACE_RECORD_LIMIT + 1],
                Some(&quiet_snapshot(&store))
            )
            .is_err()
    );
}

/// A lifecycle update states exactly the mutable facts of the summary row it
/// refreshes — no more, and nothing different.
#[test]
fn a_lifecycle_update_restates_its_summary_rows_mutable_facts_exactly() {
    let store = store("conv_9d47f0ba-1c35-7b84-8e20-6f3a5d1c9e74");
    start(&store);
    let frozen = related_request(&store, 0, "prompt-A");
    let call = bash_call("call-lifecycle");
    propose_tool_call(&store, frozen.provisional_message_id.as_str(), &call, 5);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        6,
    );
    settle_tool_call(
        &store,
        frozen.provisional_message_id.as_str(),
        "lifecycle-result",
        &call,
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![ToolResultContent::Text(TextBlock {
                text: "done".into(),
            })],
            duration_ms: 12,
            exit_code: Some(0),
            artifacts: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
        7,
    );

    let projection = TraceProjection::new(&store).unwrap();
    let page = projection.page(None, TRACE_PAGE_LIMIT).unwrap();
    let cursors: Vec<TraceCursor> = page
        .records
        .iter()
        .map(|record| record.position.clone())
        .collect();
    let updates = projection
        .refresh(&cursors, Some(&quiet_snapshot(&store)))
        .unwrap();

    assert_eq!(updates.len(), page.records.len());
    for (record, update) in page.records.iter().zip(&updates) {
        assert_eq!(update.id, record.id);
        assert_eq!(update.state, record.state);
        assert_eq!(update.timing, record.timing);
        assert_eq!(update.message_id, record.message_id);
        assert_eq!(update.attachments, record.attachments);
        assert_eq!(update.truncated, record.truncated);
        assert_eq!(
            update.request.as_ref().map(|outcome| (
                outcome.failure_kind.clone(),
                outcome.usage.clone(),
                outcome.generation
            )),
            record.request.as_ref().map(|summary| (
                summary.failure_kind.clone(),
                summary.usage.clone(),
                summary.generation
            ))
        );
        assert_eq!(
            update.tool.as_ref().map(|outcome| (
                outcome.started,
                outcome.outcome,
                outcome.detail.clone()
            )),
            record.tool.as_ref().map(|summary| (
                summary.started,
                summary.outcome,
                summary.detail.clone()
            ))
        );
    }
    assert!(
        updates
            .iter()
            .any(|update| update.tool.is_some() && update.request.is_none()),
        "the Tool row's own outcome still travels"
    );
}

// ---------------------------------------------------------------------------
// Exact canonical Context provenance (#372 revision)
// ---------------------------------------------------------------------------

/// Provenance is the canonical message's own `UserSource`, kept exact.
///
/// Two certified extensions contribute the same context family in the same
/// request. The family, the assembly generation, the contributor list and the
/// message order are all identical between them, so nothing but the frozen
/// `UserSource` can tell them apart — and Trace must.
#[test]
fn two_certified_extensions_stay_distinguishable_by_exact_contributor_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_5a2c7e93-4b16-7d80-9f35-8e07b2c4d169");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let context = [
        runtime_context(
            "ctx-observation",
            ContextKind::RuntimeToolObservation,
            "The tool batch settled.",
        ),
        extension_context("ctx-extension-a", "vendor-a.environment", "Facts from A."),
        extension_context("ctx-extension-b", "vendor-b.environment", "Facts from B."),
    ];
    let frozen = request_with(&store, "1", 0, "prompt-A", &context);
    completion(&store, &frozen, None, None, 8);

    let additions = context_of(&page(&store), 0);
    assert_eq!(
        additions
            .iter()
            .map(|addition| addition.message_id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["ctx-observation", "ctx-extension-a", "ctx-extension-b"],
        "native order is exactly request_context_ids"
    );
    assert_eq!(
        additions
            .iter()
            .map(|addition| addition.context_kind)
            .collect::<Vec<_>>(),
        vec![
            TraceContextKind::RuntimeToolObservation,
            TraceContextKind::ExtensionEnvironment,
            TraceContextKind::ExtensionEnvironment,
        ]
    );
    assert_eq!(additions[0].source, TraceContextSource::Runtime);
    assert_eq!(
        additions[1].source,
        extension_source("vendor-a.environment")
    );
    assert_eq!(
        additions[2].source,
        extension_source("vendor-b.environment")
    );
    assert_ne!(
        additions[1].source, additions[2].source,
        "two extensions of one family must not collapse to one provenance"
    );
    // The exact identity is on the wire, not a coarse namespace standing in
    // for it, and the internal Context payload still never crosses.
    let wire = serde_json::to_string(&page(&store)).unwrap();
    assert!(wire.contains("vendor-a.environment"));
    assert!(wire.contains("vendor-b.environment"));
    assert!(!wire.contains("agent_status_metadata"));
    // Exact extension identity is bounded by its own contract, well inside
    // the Trace identity and summary byte bounds.
    let projected = page(&store);
    let record = record_of(&projected, TraceKind::Request);
    assert!(
        serde_json::to_vec(&record.request.as_ref().unwrap().context_additions)
            .unwrap()
            .len()
            <= TRACE_SUMMARY_CONTEXT_BYTES
    );
    assert!(serde_json::to_vec(record).unwrap().len() <= TRACE_RECORD_BYTES);

    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(
        context_of(&page(&reopened), 0),
        additions,
        "reopening the durable store reproduces the same typed provenance"
    );
}

fn agent_status_context_kind() -> ContextKind {
    ContextKind::AgentStatus(
        crate::message::types::AgentStatusGenerationMetadata::new(
            timestamp(1),
            [crate::message::types::AgentStatusModuleId::Time],
        )
        .unwrap(),
    )
}

#[test]
fn all_context_assembly_semantic_pairs_project_from_durable_request_start() {
    let store = store("conv_5a2c7e93-4b16-7d80-9f35-8e07b2c4d169");
    start(&store);
    let context = [
        runtime_context("goal", goal_status("ship"), "goal"),
        runtime_context("tool", ContextKind::RuntimeToolObservation, "tool"),
        runtime_context("status", agent_status_context_kind(), "status"),
        extension_context("extension", "vendor-a.environment", "extension"),
    ];
    let frozen = request_with(&store, "1", 0, "prompt", &context);
    let additions = context_of(&page(&store), 0);
    assert_eq!(
        additions
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<Vec<_>>(),
        frozen.request_context_ids
    );
    assert_eq!(
        additions
            .iter()
            .map(|item| (item.source.clone(), item.context_kind))
            .collect::<Vec<_>>(),
        vec![
            (TraceContextSource::Runtime, TraceContextKind::GoalStatus),
            (
                TraceContextSource::Runtime,
                TraceContextKind::RuntimeToolObservation
            ),
            (TraceContextSource::Runtime, TraceContextKind::AgentStatus),
            (
                extension_source("vendor-a.environment"),
                TraceContextKind::ExtensionEnvironment
            ),
        ]
    );
}

fn assert_context_pair_rejected(source: UserSource, kind: ContextKind) {
    let store = store("conv_5a2c7e93-4b16-7d80-9f35-8e07b2c4d169");
    start(&store);
    let message = context_message(
        "contradictory-context",
        source.clone(),
        kind.clone(),
        "context",
    );
    let mut snapshot = prepared_request(&store, 0, None, identity_of("1", 0));
    snapshot.request_context_ids = vec![message.id().clone()];
    let producer = match source {
        UserSource::Extension { contributor } => {
            crate::runtime::identity::ContextContributorIdentity::CertifiedExtension(contributor)
        }
        _ => crate::runtime::identity::ContextContributorIdentity::Native(
            crate::runtime::identity::NativeContextContributor::RuntimeToolObservation,
        ),
    };
    snapshot
        .context_generation
        .contributors
        .push(crate::context::ContributorGeneration {
            identity: producer.clone(),
            attestation: None,
        });
    snapshot
        .contributions
        .push(crate::model::ContributionStart {
            message_id: message.id().clone(),
            producer,
            metadata: kind,
            presentation: None,
            emissions: vec![],
            opportunities: crate::context::ContributionOpportunities::default(),
            post_tool_batch_anchor: None,
        });
    assert!(matches!(
        store.commit_model_turn_start(&[message], &snapshot, timestamp(2)),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(
        store
            .read_request_snapshots(None, 10)
            .unwrap()
            .snapshots
            .is_empty()
    );
}

#[test]
fn runtime_extension_environment_is_rejected_after_durable_request_start() {
    assert_context_pair_rejected(UserSource::Runtime, ContextKind::ExtensionEnvironment);
}

fn certified_source() -> UserSource {
    UserSource::Extension {
        contributor: CertifiedExtensionIdentity::new("vendor-a.environment").unwrap(),
    }
}

#[test]
fn extension_runtime_tool_observation_is_rejected_after_durable_request_start() {
    assert_context_pair_rejected(certified_source(), ContextKind::RuntimeToolObservation);
}

#[test]
fn extension_goal_status_is_rejected_after_durable_request_start() {
    assert_context_pair_rejected(certified_source(), goal_status("ship"));
}

#[test]
fn extension_agent_status_is_rejected_after_durable_request_start() {
    assert_context_pair_rejected(certified_source(), agent_status_context_kind());
}

#[test]
fn non_admitted_context_provenance_is_rejected_after_durable_request_start() {
    for source in [
        UserSource::Human,
        UserSource::Agent {
            agent_id: crate::runtime::identity::AgentId::new("agent-a"),
        },
        UserSource::Fleet,
        UserSource::ExternalSystem,
    ] {
        assert_context_pair_rejected(source, ContextKind::RuntimeToolObservation);
    }
}
