//! Deterministic regressions for the native Trace inspection contract.
//!
//! Every scenario is built from real durable transitions on a real store, so
//! what is asserted is what the authorities actually recorded. Nothing here
//! sleeps or races: ordering is established by committing facts in order, and
//! read cuts are captured explicitly at the point the assertion is about.

use super::bounds::{TRACE_DETAIL_TEXT_BYTES, TraceText};
use super::record::{bound_record, generation_metrics};
use super::*;
use crate::context::assembly::ContextGeneration;
use crate::durable::SqliteConversationStore;
use crate::events::types::RuntimeEvent as E;
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
    ToolCallOccurrenceRef, ToolMessageBlock, UserContentBlock,
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
    ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId, MessageId, RequestId,
    ToolCallId, ToolId, TurnId,
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
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-a"),
            turn: TurnId::new("1"),
            retry_number: retry,
        },
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
    store
        .commit_model_turn_start(&[], &snapshot, timestamp(2 + i64::from(retry) * 2))
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
    append(
        store,
        E::ModelRequestCompleted {
            request_id: request.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage,
            generation,
        },
        seconds,
    );
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
    assert!(
        !serde_json::to_string(&detail)
            .unwrap()
            .contains(MANAGED_OUTPUT_LOCATOR),
        "the managed-output locator is a host path owned by the output store"
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
    let frozen = request(&store, 0);
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
        !wire.contains(SYSTEM_PROMPT),
        "the system prompt is detail-only"
    );
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
    ] {
        assert!(exposed.contains(present), "withheld {present}");
    }

    // Infrastructure authority and credentials, on the other side.
    for absent in [
        PROVIDER_CREDENTIAL,
        MCP_CREDENTIAL,
        EXECUTOR_ENVIRONMENT,
        PROVIDER_CONTINUATION,
        MANAGED_OUTPUT_LOCATOR,
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
        let mut call = bash_call("bounded-call");
        call.tool_id = ToolId::new(&identity);
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
        assert_eq!(tool_detail.truncated, oversized);
        let tool = tool_detail.tool.unwrap();
        assert_eq!(tool.definition.is_none(), oversized);
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
fn managed_storage_failure_diagnostics_never_cross_trace_with_private_paths() {
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::output::{ForegroundOutputCapture, continuation_for_capture};
    let private_path = "/private/rustx-managed-output/secret/tasks/result.output";
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
        assert!(!result.detail.as_ref().unwrap().text.contains(private_path));
        let managed = result.managed_output.as_ref().unwrap();
        assert!(!managed.complete);
        assert_eq!(managed.available, available);
        assert_eq!(managed.diagnostic.as_ref().unwrap().text, diagnostic);
        assert!(
            !managed
                .diagnostic
                .as_ref()
                .unwrap()
                .text
                .contains(private_path)
        );
        assert!(
            !serde_json::to_string(&detail)
                .unwrap()
                .contains(private_path)
        );
    }
}
