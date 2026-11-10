//! Cross-provider history uses readable content and protocol-safe correlations.
use crate::common::{collect_events, simple_request, sse_fixture};
use rustx::message::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock, ReasoningBlock,
    ToolCallOccurrenceRef, ToolMessageBlock,
};
use rustx::model::{ModelAdapter, ModelEvent, ModelProtocol};
use rustx::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
use rustx::runtime::identity::{MessageId, ToolCallId, ToolId};

const PROTOCOLS: [ModelProtocol; 3] = [
    ModelProtocol::OpenAiChatCompletions,
    ModelProtocol::OpenAiResponses,
    ModelProtocol::AnthropicMessages,
];

/// A real canonical call/result pair, with an optional owned native output unit.
pub(crate) fn tool_history(
    id: &str,
    call_id: &str,
    reasoning: Option<ReasoningBlock>,
) -> [MessageBlock; 2] {
    let mut content = reasoning
        .into_iter()
        .map(AssistantContentBlock::Reasoning)
        .collect::<Vec<_>>();
    let call_block = u32::try_from(content.len()).unwrap();
    content.push(AssistantContentBlock::ToolCall(
        rustx::tools::types::ToolCall {
            id: ToolCallId::new(call_id),
            tool_id: ToolId::new("read-tool"),
            name: "read".into(),
            arguments: serde_json::json!({}),
        },
    ));
    [
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content,
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("{id}-result")),
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new(id),
                ContentBlockIndex::new(call_block),
            ),
            tool_call_id: ToolCallId::new(call_id),
            tool_id: ToolId::new("read-tool"),
            result: rustx::tools::types::ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                    text: format!("{id} tool result"),
                })],
                workflow: None,
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                managed_output: None,
            }
            .into(),
        }),
    ]
}

#[tokio::test]
async fn generic_tool_id_collision_never_rewrites_owned_responses_output() {
    for native_first in [false, true] {
        let protocol = ModelProtocol::OpenAiResponses;
        let server = crate::common::FixtureServer::start(move |_, _| reply(protocol)).await;
        let mut request = simple_request(protocol, "same-model", "follow up");
        let items = vec![
            serde_json::json!({"type":"reasoning", "id":"rs_owned", "summary":[], "encrypted_content":"preserved-cipher"}),
            serde_json::json!({"type":"function_call", "id":"fc_owned", "call_id":"repeat", "name":"read", "arguments":"{}"}),
        ];
        let native = tool_history(
            "native",
            "repeat",
            Some(ReasoningBlock {
                origin: request.invocation.replay_origin(),
                text: None,
                provider_state: Some(ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: items.clone().into(),
                    },
                )),
            }),
        );
        let generic = tool_history("generic", "repeat", None);
        let history = if native_first {
            native.into_iter().chain(generic).collect::<Vec<_>>()
        } else {
            generic.into_iter().chain(native).collect()
        };
        for (index, message) in history.into_iter().enumerate() {
            request.messages.insert(index, message.into());
        }
        let events = collect_events(adapter(&server, protocol).as_ref(), request).await;
        assert!(
            matches!(events.last(), Some(ModelEvent::Completed { .. })),
            "{events:?}"
        );
        let body: serde_json::Value = serde_json::from_str(&server.request_body(0)).unwrap();
        let input = body["input"].as_array().unwrap();
        let at = input
            .iter()
            .position(|item| item["id"] == "rs_owned")
            .unwrap();
        assert_eq!(&input[at..at + 2], &items);
        let (calls, results) = correlations(&body, protocol);
        assert_eq!(calls, results);
        assert_ne!(calls[0], calls[1]);
        assert_eq!(calls[usize::from(!native_first)], "repeat");
    }
}

#[tokio::test]
async fn active_continuation_cannot_borrow_another_binding_or_model() {
    for protocol in [
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        for change_model in [false, true] {
            let server = crate::common::FixtureServer::start(move |_, _| reply(protocol)).await;
            let mut request = simple_request(protocol, "same-model", "follow up");
            let mut origin = request.invocation.replay_origin().unwrap();
            if change_model {
                origin.model = "other-model".into();
            } else {
                origin.binding = "other-binding".into();
            }
            let state = match protocol {
                ModelProtocol::OpenAiResponses => ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stored {
                        previous_response_id: "resp_foreign".into(),
                    },
                ),
                _ => ProviderContinuationState::Anthropic(AnthropicContinuation {
                    opaque: serde_json::json!({"type":"thinking", "thinking":"thought", "signature":"foreign"}).into(),
                }),
            };
            let output = MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("accepted"),
                content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                    origin: Some(origin),
                    text: Some("thought".into()),
                    provider_state: Some(state.clone()),
                })],
            });
            request.continuation_source = Some(rustx::runtime::continuation::AcceptedOutputContinuationSource::from_accepted_output(
                rustx::runtime::identity::RequestId::new("source"), &rustx::runtime::identity::ConversationId::generate(), &rustx::model::SelectedModelInput::default(), &output,
            ).unwrap());
            request.messages.insert(0, output.into());
            request.continuation = Some(state);
            let events = collect_events(adapter(&server, protocol).as_ref(), request).await;
            assert!(
                matches!(events.as_slice(), [ModelEvent::Failed {error}] if error.kind == rustx::model::ModelErrorKind::InvalidRequest),
                "{events:?}"
            );
            assert_eq!(server.attempt_count(), 0);
        }
    }
}

fn adapter(
    server: &crate::common::FixtureServer,
    protocol: ModelProtocol,
) -> Box<dyn ModelAdapter> {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            Box::new(rustx::model::OpenAiChatCompletionsAdapter::new(
                rustx::model::OpenAiAdapterConfig::new("test-key", server.url("/v1")),
            ))
        }
        ModelProtocol::OpenAiResponses => Box::new(rustx::model::OpenAiResponsesAdapter::new(
            rustx::model::OpenAiAdapterConfig::new("test-key", server.url("/v1")),
        )),
        ModelProtocol::AnthropicMessages => Box::new(rustx::model::AnthropicMessagesAdapter::new(
            rustx::model::AnthropicAdapterConfig::new("test-key", server.url("")),
        )),
    }
}

fn reply(protocol: ModelProtocol) -> crate::common::FixtureReply {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => sse_fixture("openai_chat", "plain_text.sse"),
        ModelProtocol::OpenAiResponses => sse_fixture("openai_responses", "plain_text.sse"),
        ModelProtocol::AnthropicMessages => sse_fixture("anthropic", "text.sse"),
    }
}

fn correlations(body: &serde_json::Value, protocol: ModelProtocol) -> (Vec<String>, Vec<String>) {
    let mut calls = Vec::new();
    let mut results = Vec::new();
    match protocol {
        ModelProtocol::OpenAiResponses => {
            for item in body["input"].as_array().unwrap() {
                match item["type"].as_str() {
                    Some("function_call") => {
                        calls.push(item["call_id"].as_str().unwrap().to_owned());
                    }
                    Some("function_call_output") => {
                        results.push(item["call_id"].as_str().unwrap().to_owned());
                    }
                    _ => {}
                }
            }
        }
        ModelProtocol::AnthropicMessages => {
            for message in body["messages"].as_array().unwrap() {
                for part in message["content"].as_array().unwrap() {
                    match part["type"].as_str() {
                        Some("tool_use") => calls.push(part["id"].as_str().unwrap().to_owned()),
                        Some("tool_result") => {
                            results.push(part["tool_use_id"].as_str().unwrap().to_owned());
                        }
                        _ => {}
                    }
                }
            }
        }
        ModelProtocol::OpenAiChatCompletions => {
            for message in body["messages"].as_array().unwrap() {
                if let Some(items) = message["tool_calls"].as_array() {
                    calls.extend(
                        items
                            .iter()
                            .map(|call| call["id"].as_str().unwrap().to_owned()),
                    );
                }
                if message["role"] == "tool" {
                    results.push(message["tool_call_id"].as_str().unwrap().to_owned());
                }
            }
        }
    }
    (calls, results)
}

#[tokio::test]
async fn every_protocol_pair_scopes_reasoning_and_preserves_tool_pairs() {
    for source in PROTOCOLS {
        for target in PROTOCOLS {
            // Same owner, another model, another provider/endpoint, unattributed history.
            for difference in 0..6 {
                let server = crate::common::FixtureServer::start(move |_, _| reply(target)).await;
                let mut request = simple_request(target, "same-model", "follow up");
                if target == ModelProtocol::OpenAiChatCompletions {
                    request.invocation.compat.chat_reasoning_replay =
                        Some(rustx::model::ChatReasoningReplay::ReasoningContent);
                }
                let mut origin = request.invocation.replay_origin().unwrap();
                origin.protocol = source;
                if difference == 1 {
                    origin.model = "other-model".into();
                }
                if difference == 2 {
                    origin.binding = "other-binding".into();
                }
                let native = source == target && difference == 0;
                if difference >= 4 {
                    request.invocation.replay_binding = None;
                }
                let id = match source {
                    ModelProtocol::AnthropicMessages => "toolu_native".into(),
                    ModelProtocol::OpenAiResponses => "call_native".into(),
                    ModelProtocol::OpenAiChatCompletions => {
                        format!("foreign/call|{}", "long".repeat(40))
                    }
                };
                let state = match source {
                    ModelProtocol::OpenAiChatCompletions => None,
                    ModelProtocol::AnthropicMessages => Some(ProviderContinuationState::Anthropic(
                        AnthropicContinuation {
                            opaque: serde_json::json!({"type":"thinking", "thinking":"source thought", "signature":"native-anthropic-signature"}).into(),
                        },
                    )),
                    ModelProtocol::OpenAiResponses => {
                        Some(ProviderContinuationState::OpenAiResponses(
                            OpenAiResponsesContinuation::Stateless {
                                items: vec![
                                    serde_json::json!({"type":"reasoning", "id":"rs_original", "summary":[{"type":"summary_text", "text":"source thought"}], "encrypted_content":"native-responses-cipher"}),
                                    serde_json::json!({"type":"message", "role":"assistant", "id":"msg_original", "content":[{"type":"output_text", "text":"previous answer", "annotations":[]}]}),
                                    serde_json::json!({"type":"function_call", "id":"fc_original", "call_id":id, "name":"read", "arguments":"{}"}),
                                ].into(),
                            },
                        ))
                    }
                };
                request.messages.insert(
                    0,
                    MessageBlock::Assistant(AssistantMessageBlock {
                        id: MessageId::new("old-assistant"),
                        content: vec![
                            AssistantContentBlock::Reasoning(ReasoningBlock {
                                origin: (!matches!(difference, 3 | 5)).then_some(origin),
                                text: Some("source thought".into()),
                                provider_state: state,
                            }),
                            AssistantContentBlock::Text(TextBlock {
                                text: "previous answer".into(),
                            }),
                            AssistantContentBlock::ToolCall(rustx::tools::types::ToolCall {
                                id: ToolCallId::new(&id),
                                tool_id: ToolId::new("read-tool"),
                                name: "read".into(),
                                arguments: serde_json::json!({}),
                            }),
                        ],
                    })
                    .into(),
                );
                request.messages.insert(
                    1,
                    MessageBlock::Tool(ToolMessageBlock {
                        id: MessageId::new("old-result"),
                        occurrence: ToolCallOccurrenceRef::new(
                            MessageId::new("old-assistant"),
                            ContentBlockIndex::new(2),
                        ),
                        tool_call_id: ToolCallId::new(&id),
                        tool_id: ToolId::new("read-tool"),
                        result: rustx::tools::types::ToolExecutionResult {
                            status: rustx::tools::types::ToolExecutionStatus::Success,
                            content: vec![rustx::tools::types::ToolResultContent::Text(
                                TextBlock {
                                    text: "actual tool result".into(),
                                },
                            )],
                            workflow: None,
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: vec![],
                            deliveries: vec![],
                            truncation: None,
                            managed_output: None,
                        }
                        .into(),
                    })
                    .into(),
                );
                let events = collect_events(adapter(&server, target).as_ref(), request).await;
                assert!(
                    matches!(events.last(), Some(ModelEvent::Completed { .. })),
                    "{source:?} -> {target:?}, identity {difference}: {events:?}"
                );
                let body: serde_json::Value =
                    serde_json::from_str(&server.request_body(0)).unwrap();
                let text = serde_json::to_string(&body).unwrap();
                assert!(
                    text.contains("source thought")
                        && text.contains("previous answer")
                        && text.contains("actual tool result")
                );
                assert_eq!(
                    text.contains("native-anthropic-signature"),
                    native && source == ModelProtocol::AnthropicMessages
                );
                assert_eq!(
                    text.contains("native-responses-cipher"),
                    native && source == ModelProtocol::OpenAiResponses
                );
                if target == ModelProtocol::OpenAiChatCompletions {
                    assert_eq!(
                        body["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|m| m.get("reasoning_content").is_some()),
                        native
                    );
                }
                let (calls, results) = correlations(&body, target);
                assert_eq!(calls.len(), 1);
                assert_eq!(calls, results);
                let limit = if target == ModelProtocol::OpenAiChatCompletions {
                    40
                } else {
                    64
                };
                assert!(
                    calls[0].len() <= limit
                        && calls[0]
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                );
            }
        }
    }
}

#[tokio::test]
async fn foreign_redacted_thinking_leaves_no_empty_assistant_and_refusal_is_readable() {
    for target in PROTOCOLS {
        let server = crate::common::FixtureServer::start(move |_, _| reply(target)).await;
        let mut request = simple_request(target, "model", "follow up");
        request.messages.insert(0, MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("opaque-history"),
            content: vec![AssistantContentBlock::Reasoning(ReasoningBlock { origin: None, text: None, provider_state: Some(ProviderContinuationState::Anthropic(AnthropicContinuation { opaque:serde_json::json!({"type":"redacted_thinking", "data":"must-not-leak"}).into() })) })],
        }).into());
        request.messages.insert(
            1,
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("prior-refusal"),
                content: vec![AssistantContentBlock::Refusal(
                    rustx::message::types::RefusalBlock {
                        text: "previous refusal".into(),
                    },
                )],
            })
            .into(),
        );
        let events = collect_events(adapter(&server, target).as_ref(), request).await;
        assert!(
            matches!(events.last(), Some(ModelEvent::Completed { .. })),
            "{events:?}"
        );
        let body: serde_json::Value = serde_json::from_str(&server.request_body(0)).unwrap();
        let text = serde_json::to_string(&body).unwrap();
        assert!(!text.contains("must-not-leak"));
        assert!(text.contains("previous refusal"));
        let messages = if target == ModelProtocol::OpenAiResponses {
            &body["input"]
        } else {
            &body["messages"]
        };
        assert_eq!(
            messages.as_array().unwrap().len(),
            2,
            "opaque-only history produces no empty assistant message"
        );
    }
}
