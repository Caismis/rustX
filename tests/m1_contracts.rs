//! M1 contract tests: deserialize representative JSON fixtures, assert typed
//! semantic values, re-serialize, and verify round-trip equality with
//! deterministic serialization. These tests require no network access.

use std::fs;
use std::path::PathBuf;

use rustx::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use rustx::message::types::{
    AgentContentBlock, InboundKind, MessageBlock, UserMessageBlock, UserSource,
};
use rustx::protocol::manifest::RuntimeManifest;
use rustx::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
use rustx::runtime::identity::{CapabilityRevision, EventId, MessageId};
use rustx::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Reads a fixture file from the M1 fixture directory.
fn read_fixture(name: &str) -> String {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "m1", name]
        .iter()
        .collect();
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("failed to read fixture {name}: {error}");
    })
}

/// Round-trips a value through JSON, checking deterministic serialization.
fn round_trip<T>(value: &T) -> T
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let first = serde_json::to_string(value).expect("serialize value");
    let second = serde_json::to_string(value).expect("serialize value again");
    assert_eq!(first, second, "serialization must be deterministic");
    let decoded: T = serde_json::from_str(&first).expect("deserialize value");
    assert_eq!(decoded, *value, "round trip must preserve the value");
    decoded
}

/// Fixture A: human inbound input is a `UserMessageBlock` with human source.
#[test]
fn human_input_round_trip() {
    let block: MessageBlock =
        serde_json::from_str(&read_fixture("a_human_input.json")).expect("parse fixture");
    let MessageBlock::User(user) = &block else {
        panic!("fixture A must deserialize as a User message");
    };
    assert_eq!(user.id, MessageId::new("msg-user-1"));
    assert_eq!(user.source, UserSource::Human);
    assert_eq!(user.kind, InboundKind::Message);
    assert!(
        matches!(&user.content[0], rustx::message::types::UserContentBlock::Text(t) if t.text.contains("Summarize"))
    );
    let _ = round_trip(&block);
}

/// Fixture B: an inbound message from another agent remains a
/// `UserMessageBlock` with agent provenance — never an `AgentMessageBlock`.
#[test]
fn agent_to_agent_inbound_remains_user() {
    let block: MessageBlock = serde_json::from_str(&read_fixture("b_agent_to_agent_inbound.json"))
        .expect("parse fixture");
    let MessageBlock::User(user) = &block else {
        panic!("fixture B must deserialize as a User message");
    };
    assert!(matches!(
        user.source,
        UserSource::Agent { ref agent_id } if agent_id.as_str() == "agent-b"
    ));
    assert_eq!(user.kind, InboundKind::Message);
    assert!(!matches!(block, MessageBlock::Agent(_)));
    assert!(!matches!(block, MessageBlock::Tool(_)));
    let _ = round_trip(&block);
}

/// Fixture C: one completed generation with text, reasoning (including
/// stateful `OpenAI` Responses continuation state), and a tool call.
#[test]
fn agent_generation_round_trip() {
    let block: MessageBlock =
        serde_json::from_str(&read_fixture("c_agent_generation.json")).expect("parse fixture");
    let MessageBlock::Agent(agent) = &block else {
        panic!("fixture C must deserialize as an Agent message");
    };
    assert_eq!(agent.id, MessageId::new("msg-agent-a-gen-3"));
    let mut saw_text = false;
    let mut saw_reasoning_with_state = false;
    let mut saw_tool_call = false;
    for content in &agent.content {
        match content {
            AgentContentBlock::Text(text) => {
                saw_text = true;
                assert!(text.text.contains("Cargo manifest"));
            }
            AgentContentBlock::Reasoning(reasoning) => {
                assert_eq!(
                    reasoning.text.as_deref(),
                    Some(
                        "The user asked for the repository layout, so the most direct action is to list the top-level directory contents before summarizing."
                    )
                );
                let Some(ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stored {
                        previous_response_id,
                    },
                )) = &reasoning.provider_state
                else {
                    panic!("fixture C reasoning must carry stored OpenAI continuation state");
                };
                assert_eq!(previous_response_id, "resp_abc123");
                saw_reasoning_with_state = true;
            }
            AgentContentBlock::ToolCall(call) => {
                saw_tool_call = true;
                assert_eq!(call.id.as_str(), "call_01");
                assert_eq!(call.name, "list_directory");
                assert_eq!(call.arguments, serde_json::json!({"path": "."}));
            }
            _ => {}
        }
    }
    assert!(saw_text && saw_reasoning_with_state && saw_tool_call);
    let _ = round_trip(&block);
}

/// Fixture D: tool result with status, content, duration, exit code,
/// artifact reference, and truncation metadata.
#[test]
fn tool_result_round_trip() {
    let block: MessageBlock =
        serde_json::from_str(&read_fixture("d_tool_result.json")).expect("parse fixture");
    let MessageBlock::Tool(tool) = &block else {
        panic!("fixture D must deserialize as a Tool message");
    };
    assert_eq!(tool.tool_call_id.as_str(), "call_01");
    assert_eq!(tool.tool_id.as_str(), "tool-list");
    assert_eq!(tool.result.status, ToolExecutionStatus::Success);
    assert_eq!(tool.result.duration_ms, 12);
    assert_eq!(tool.result.exit_code, Some(0));
    let artifact = tool
        .result
        .artifacts
        .first()
        .expect("fixture D must reference an artifact");
    assert_eq!(artifact.artifact_id.as_str(), "artifact-1");
    let truncation = tool
        .result
        .truncation
        .as_ref()
        .expect("truncation metadata");
    assert!(truncation.truncated);
    assert_eq!(truncation.original_bytes, Some(51_200));
    let _ = round_trip(&block);
}

/// Fixture D-interrupted: interrupted/unknown tool execution stays a
/// distinct status and round-trips.
#[test]
fn tool_interrupted_round_trip() {
    let block: MessageBlock =
        serde_json::from_str(&read_fixture("d_tool_interrupted.json")).expect("parse fixture");
    let MessageBlock::Tool(tool) = &block else {
        panic!("fixture D-interrupted must deserialize as a Tool message");
    };
    assert_eq!(tool.result.status, ToolExecutionStatus::Interrupted);
    assert_eq!(tool.result.duration_ms, 0);
    assert_eq!(tool.result.exit_code, None);
    let _ = round_trip(&block);
}

/// Fixture E: a representative runtime manifest.
#[test]
fn manifest_round_trip() {
    let manifest: RuntimeManifest =
        serde_json::from_str(&read_fixture("e_manifest.json")).expect("parse fixture");
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(manifest.runtime_version, "0.1.0");
    assert_eq!(manifest.agent.id.as_str(), "agent-a");
    assert_eq!(manifest.agent.version_id.as_str(), "agent-v1");
    assert_eq!(
        manifest.model.protocol,
        rustx::model::types::ModelProtocol::OpenAiResponses
    );
    assert_eq!(manifest.model.model.to_string(), "provider-a/gpt-5-mini");
    assert_eq!(
        manifest.model.reasoning_profile,
        Some(rustx::model::ReasoningProfileId::new("high"))
    );
    assert!(manifest.model.reasoning_enabled);
    assert_eq!(manifest.capabilities.revision, CapabilityRevision::new(42));
    assert_eq!(manifest.capabilities.skills.len(), 1);
    assert_eq!(manifest.capabilities.tools.len(), 2);
    assert_eq!(manifest.capabilities.mcp.len(), 1);
    assert_eq!(manifest.context.context_window_tokens, 131_072);
    assert_eq!(manifest.context.reserve_tokens, 16_384);
    assert_eq!(manifest.context.keep_recent_tokens, 20_000);
    assert_eq!(manifest.limits.max_turns, 64);
    assert_eq!(manifest.limits.max_tool_calls, 128);
    assert_eq!(manifest.limits.max_runtime_seconds, 1_800);
    let _ = round_trip(&manifest);
}

/// Fixture G: stateless `OpenAI` Responses continuation preserves opaque
/// reasoning/output items for zero-data-retention operation.
#[test]
fn openai_stateless_continuation_round_trip() {
    let state: ProviderContinuationState =
        serde_json::from_str(&read_fixture("g_openai_stateless_continuation.json"))
            .expect("parse fixture");
    let ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
        items,
    }) = &state
    else {
        panic!("fixture G must deserialize as stateless OpenAI continuation");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(
        items[0]["encrypted_content"], "opaque-encrypted-reasoning",
        "reasoning items carry top-level opaque encrypted content"
    );
    assert_eq!(items[1]["type"], "output_text");
    let _ = round_trip(&state);
}

/// Fixtures F: runtime event envelopes.
#[test]
fn attempt_started_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_attempt_started.json")).expect("parse fixture");
    assert_eq!(envelope.schema_version, 1);
    assert_eq!(envelope.sequence, 1);
    assert_eq!(envelope.event_id, EventId::new("evt-1"));
    assert_eq!(envelope.conversation_id.as_str(), "conv-1");
    assert_eq!(
        envelope
            .attempt_id
            .as_ref()
            .map(rustx::runtime::identity::AttemptId::as_str),
        Some("attempt-1")
    );
    assert!(matches!(
        envelope.event,
        RuntimeEvent::AttemptStarted { ref attempt_id } if attempt_id.as_str() == "attempt-1"
    ));
    let _ = round_trip(&envelope);
}

/// A model delta event is an execution fact, never a conversation message.
#[test]
fn agent_text_delta_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_agent_text_delta.json")).expect("parse fixture");
    assert!(matches!(
        envelope.event,
        RuntimeEvent::AgentTextDelta {
            ref message_id,
            block_index,
            ref delta
        } if message_id.as_str() == "msg-agent-a-gen-3"
            && block_index.get() == 0
            && delta.contains("Cargo manifest")
    ));
    assert_eq!(
        envelope
            .turn_id
            .as_ref()
            .map(rustx::runtime::identity::TurnId::as_str),
        Some("turn-1")
    );
    let json = serde_json::to_string(&envelope).expect("serialize envelope");
    assert!(
        serde_json::from_str::<MessageBlock>(&json).is_err(),
        "an event envelope must never deserialize as a MessageBlock"
    );
    let _ = round_trip(&envelope);
}

/// A refusal delta event preserves refusal semantics as an execution fact,
/// never flattening it into text.
#[test]
fn agent_refusal_delta_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_agent_refusal_delta.json")).expect("parse fixture");
    let RuntimeEvent::AgentRefusalDelta {
        message_id,
        block_index,
        delta,
    } = &envelope.event
    else {
        panic!("fixture F-refusal must deserialize as AgentRefusalDelta");
    };
    assert_eq!(message_id.as_str(), "msg-agent-a-gen-4");
    assert_eq!(block_index.get(), 1);
    assert_eq!(delta, "I cannot comply with that request.");
    let value = serde_json::to_value(&envelope.event).expect("serialize event");
    assert_eq!(value["type"], "agent_refusal_delta");
    let _ = round_trip(&envelope);
}

/// Tool execution completion is an event that stays attributable to its
/// originating tool call and carries the normalized result.
#[test]
fn tool_execution_completed_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_tool_execution_completed.json"))
            .expect("parse fixture");
    let RuntimeEvent::ToolExecutionCompleted {
        tool_call_id,
        tool_id,
        result,
    } = &envelope.event
    else {
        panic!("fixture F-tool must deserialize as ToolExecutionCompleted");
    };
    assert_eq!(tool_call_id.as_str(), "call_01");
    assert_eq!(tool_id.as_str(), "tool-list");
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(result.duration_ms, 12);
    let _ = round_trip(&envelope);
}

/// A committed agent message is an execution fact referencing the message by
/// identity; the event never embeds message content.
#[test]
fn agent_message_committed_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_agent_message_committed.json"))
            .expect("parse fixture");
    let RuntimeEvent::AgentMessageCommitted { message_id } = &envelope.event else {
        panic!("fixture F-commit must deserialize as AgentMessageCommitted");
    };
    assert_eq!(message_id, &MessageId::new("msg-agent-a-gen-3"));
    let value = serde_json::to_value(&envelope.event).expect("serialize event");
    assert!(
        value.get("message").is_none(),
        "committed-message events must not embed message content"
    );
    let _ = round_trip(&envelope);
}

/// A completed attempt carries its finish reason directly; no outcome
/// payload exists on the terminal event.
#[test]
fn attempt_completed_envelope_round_trip() {
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(&read_fixture("f_attempt_completed.json")).expect("parse fixture");
    let RuntimeEvent::AttemptCompleted {
        attempt_id,
        finish_reason,
    } = &envelope.event
    else {
        panic!("fixture F-complete must deserialize as AttemptCompleted");
    };
    assert_eq!(attempt_id.as_str(), "attempt-1");
    assert_eq!(
        finish_reason,
        &rustx::model::finish::ModelFinishReason::Stop
    );
    assert_eq!(
        rustx::events::types::AttemptOutcome::from_terminal_event(&envelope.event),
        Some(rustx::events::types::AttemptOutcome::Completed {
            finish_reason: rustx::model::finish::ModelFinishReason::Stop,
        })
    );
    let _ = round_trip(&envelope);
}

/// Programmatic variants not covered by fixtures also round-trip: a
/// cancelled attempt and an interrupted tool result inside an envelope.
#[test]
fn additional_event_variants_round_trip() {
    use rustx::runtime::identity::{AttemptId, ConversationId, EventId};
    use rustx::runtime::types::CancellationReason;

    let cancelled = RuntimeEventEnvelope {
        schema_version: 1,
        event_id: EventId::new("evt-6"),
        sequence: 6,
        conversation_id: ConversationId::new("conv-1"),
        attempt_id: Some(AttemptId::new("attempt-1")),
        turn_id: None,
        timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:05Z")
            .expect("parse timestamp")
            .with_timezone(&chrono::Utc),
        event: RuntimeEvent::AttemptCancelled {
            attempt_id: AttemptId::new("attempt-1"),
            reason: CancellationReason::UserRequested,
        },
    };
    let decoded: RuntimeEventEnvelope =
        serde_json::from_str(&serde_json::to_string(&cancelled).expect("serialize"))
            .expect("deserialize");
    assert_eq!(decoded, cancelled);

    let interrupted = RuntimeEvent::ToolExecutionCompleted {
        tool_call_id: rustx::runtime::identity::ToolCallId::new("call_07"),
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Interrupted,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
        },
    };
    let decoded: RuntimeEvent =
        serde_json::from_str(&serde_json::to_string(&interrupted).expect("serialize"))
            .expect("deserialize");
    assert_eq!(decoded, interrupted);

    let compaction = RuntimeEvent::CompactionCompleted {
        generation: 3,
        tokens_before: TokenMeasurement {
            input_tokens: 4800,
            source: TokenMeasurementSource::ProviderReported,
        },
        estimated_tokens_after: 1700,
    };
    let encoded = serde_json::to_value(&compaction).expect("serialize compaction");
    assert_eq!(
        encoded["tokens_before"],
        serde_json::json!({
            "input_tokens": 4800,
            "source": "provider_reported"
        })
    );
    let estimated = serde_json::to_value(TokenMeasurement {
        input_tokens: 4800,
        source: TokenMeasurementSource::Estimated,
    })
    .expect("serialize estimated measurement");
    assert_eq!(
        estimated,
        serde_json::json!({
            "input_tokens": 4800,
            "source": "estimated"
        })
    );
    let decoded: RuntimeEvent =
        serde_json::from_str(&serde_json::to_string(&compaction).expect("serialize"))
            .expect("deserialize");
    assert_eq!(decoded, compaction);
}

/// A `ToolMessageBlock` composes `ToolExecutionResult` as the single source
/// of truth, while the committed-message event references the message by
/// identity only.
#[test]
fn tool_message_composes_execution_result() {
    let block: MessageBlock =
        serde_json::from_str(&read_fixture("d_tool_result.json")).expect("parse fixture");
    let MessageBlock::Tool(tool_message) = &block else {
        panic!("fixture D must deserialize as a Tool message");
    };
    assert_eq!(tool_message.result.status, ToolExecutionStatus::Success);

    let committed = RuntimeEvent::ToolMessageCommitted {
        message_id: tool_message.id.clone(),
        tool_call_id: tool_message.tool_call_id.clone(),
    };
    let value = serde_json::to_value(&committed).expect("serialize event");
    assert!(
        value.get("message").is_none(),
        "committed-message events must not embed message content"
    );
    let decoded: RuntimeEvent =
        serde_json::from_str(&serde_json::to_string(&committed).expect("serialize"))
            .expect("deserialize");
    assert_eq!(decoded, committed);
}

/// A `UserMessageBlock` with a fixed persisted UTC timestamp round-trips
/// through the canonical encoding.
#[test]
fn user_message_timestamp_round_trips() {
    let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
        .expect("parse timestamp")
        .with_timezone(&chrono::Utc);
    let block = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-user-ts"),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: "deploy it".to_owned(),
            },
        )],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    });
    let json = serde_json::to_string(&block).expect("serialize block");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert_eq!(
        value["timestamp"], "2026-08-07T12:00:00Z",
        "the timestamp is part of the canonical encoding"
    );
    let decoded: MessageBlock = serde_json::from_str(&json).expect("deserialize block");
    assert_eq!(decoded, block, "the timestamp survives the round trip");
}

/// A `UserMessageBlock` without a timestamp remains compatible with the
/// existing canonical encoding: the field is absent while `None` and
/// defaults to `None` on deserialization.
#[test]
fn user_message_without_timestamp_stays_compatible() {
    let block = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-user-no-ts"),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: "continue".to_owned(),
            },
        )],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    let json = serde_json::to_string(&block).expect("serialize block");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert!(
        value.get("timestamp").is_none(),
        "a None timestamp is omitted from the canonical encoding"
    );
    let decoded: MessageBlock = serde_json::from_str(&json).expect("deserialize block");
    assert_eq!(decoded, block, "round trip preserves the absent timestamp");
    // The pre-timestamp canonical encoding remains representable.
    let legacy = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-user-legacy"),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: "legacy".to_owned(),
            },
        )],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    let legacy_json = r#"{"role":"user","id":"msg-user-legacy","content":[{"type":"text","text":"legacy"}],"source":"human","kind":"message"}"#;
    let decoded_legacy: MessageBlock =
        serde_json::from_str(legacy_json).expect("legacy encoding deserializes");
    assert_eq!(
        decoded_legacy, legacy,
        "the pre-timestamp canonical encoding remains representable"
    );
}
