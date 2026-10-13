//! Issue #460: native preflight rejection survives the actual serialized
//! request for every supported protocol. Only loopback fixture servers run;
//! native registration supplies the schema and diagnostic without execution.

use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
    ToolCallOccurrenceRef, ToolMessageBlock,
};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelEvent, ModelInputMessage,
    ModelProtocol, ModelRequest, OpenAiAdapterConfig, OpenAiChatCompletionsAdapter,
    OpenAiResponsesAdapter,
};
use rustx::runtime::identity::{MessageId, ToolCallId};
use rustx::tools::PreflightOutcome;
use rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES;
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent};
use serde_json::{Value, json};

const CALL_IDS: [&str; 3] = ["call-before", "call-rejected-ask-user", "call-after"];

fn result(status: ToolExecutionStatus, content: Vec<ToolResultContent>) -> ToolExecutionResult {
    ToolExecutionResult {
        status,
        content,
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        deliveries: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn rejection_history(protocol: ModelProtocol) -> ModelRequest {
    // This fixture registers the real native normalizer. It does not run a
    // tool or Agent Loop; canonical settlement is covered by the Agent suite.
    let fixture = crate::common::native_fixture_without_extensions();
    let definitions = fixture.registry.model_definitions();
    let ask_user = definitions
        .iter()
        .find(|tool| tool.name == "ask_user")
        .expect("registered native ask_user")
        .clone();
    let read = definitions
        .into_iter()
        .find(|tool| tool.name == "read")
        .expect("registered native read");
    let rejected_call = ToolCall {
        id: ToolCallId::new(CALL_IDS[1]),
        tool_id: ask_user.id.clone(),
        name: ask_user.name.clone(),
        arguments: json!({"questions": [{
            "question": "Choose the implementation approach.",
            "header": "Approach",
            "options": [
                {"label": "PRIVATE_DUPLICATE_LABEL", "description": "First approach."},
                {"label": "Independent", "description": "Second approach."},
                {"label": "PRIVATE_DUPLICATE_LABEL", "description": "Third approach."}
            ]
        }]}),
    };
    let PreflightOutcome::Rejected { tool_id, error, .. } = fixture
        .registry
        .preflight(&rejected_call)
        .expect("native identity resolves")
    else {
        panic!("duplicate option labels must fail native preflight");
    };
    assert_eq!(tool_id, rejected_call.tool_id);
    assert!(error.contains("questions[0].options[2].label"));
    assert!(error.contains("questions[0].options[0].label"));
    assert!(error.contains("unique within each question"));
    assert!(error.contains("Change one of the conflicting labels"));
    assert!(!error.contains("PRIVATE_DUPLICATE_LABEL"));
    assert!(
        error.len() <= 1024,
        "native diagnostic is explicitly bounded"
    );

    let calls = [
        ToolCall {
            id: ToolCallId::new(CALL_IDS[0]),
            tool_id: read.id.clone(),
            name: read.name.clone(),
            arguments: json!({"path": "before.txt"}),
        },
        rejected_call,
        ToolCall {
            id: ToolCallId::new(CALL_IDS[2]),
            tool_id: read.id.clone(),
            name: read.name.clone(),
            arguments: json!({"path": "after.txt"}),
        },
    ];
    let results = [
        result(
            ToolExecutionStatus::Success,
            vec![ToolResultContent::Text(TextBlock {
                text: "Content before rejection.".to_owned(),
            })],
        ),
        result(ToolExecutionStatus::Failed { error }, Vec::new()),
        result(
            ToolExecutionStatus::Success,
            vec![ToolResultContent::Text(TextBlock {
                text: "Content after rejection.".to_owned(),
            })],
        ),
    ];
    assert!(
        results[1].content.is_empty(),
        "failure lives only in status"
    );
    let mut request = crate::common::simple_request(protocol, "wire-test", "Help me choose.");
    request.tools = vec![ask_user, read];
    let assistant_id = MessageId::new("assistant-mixed-batch");
    request
        .messages
        .push(ModelInputMessage::Canonical(MessageBlock::Assistant(
            AssistantMessageBlock {
                id: assistant_id.clone(),
                content: calls
                    .iter()
                    .cloned()
                    .map(AssistantContentBlock::ToolCall)
                    .collect(),
            },
        )));
    for (index, (call, result)) in calls.into_iter().zip(results).enumerate() {
        request
            .messages
            .push(ModelInputMessage::Canonical(MessageBlock::Tool(
                ToolMessageBlock {
                    id: MessageId::new(format!("result-{index}")),
                    occurrence: ToolCallOccurrenceRef::new(
                        assistant_id.clone(),
                        ContentBlockIndex::new(u32::try_from(index).unwrap()),
                    ),
                    tool_call_id: call.id,
                    tool_id: call.tool_id,
                    result,
                },
            )));
    }
    request
}

fn assert_no_pattern(value: &Value) {
    match value {
        Value::Object(fields) => {
            assert!(!fields.contains_key("pattern"), "hazardous schema: {value}");
            fields.values().for_each(assert_no_pattern);
        }
        Value::Array(items) => items.iter().for_each(assert_no_pattern),
        _ => {}
    }
}

fn text_parts(content: &Value) -> String {
    content
        .as_array()
        .expect("protocol text-part array")
        .iter()
        .map(|part| {
            assert_eq!(part["type"], "text");
            part["text"].as_str().expect("non-null text part")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn adapter_for(
    protocol: ModelProtocol,
    server: &crate::common::FixtureServer,
) -> Box<dyn ModelAdapter> {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => Box::new(OpenAiChatCompletionsAdapter::new(
            OpenAiAdapterConfig::new("fixture-key", server.url("/v1")),
        )),
        ModelProtocol::OpenAiResponses => Box::new(OpenAiResponsesAdapter::new(
            OpenAiAdapterConfig::new("fixture-key", server.url("/v1")),
        )),
        ModelProtocol::AnthropicMessages => Box::new(AnthropicMessagesAdapter::new(
            AnthropicAdapterConfig::new("fixture-key", server.url("")),
        )),
    }
}

async fn assert_rejection_wire(protocol: ModelProtocol) {
    let (fixture_dir, fixture_name) = match protocol {
        ModelProtocol::OpenAiChatCompletions => ("openai_chat", "plain_text.sse"),
        ModelProtocol::OpenAiResponses => ("openai_responses", "plain_text.sse"),
        ModelProtocol::AnthropicMessages => ("anthropic", "text.sse"),
    };
    let server = crate::common::FixtureServer::start(move |_, _| {
        crate::common::sse_fixture(fixture_dir, fixture_name)
    })
    .await;
    let adapter = adapter_for(protocol, &server);
    let request = rejection_history(protocol);
    let expected: Vec<_> = request
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelInputMessage::Canonical(MessageBlock::Tool(tool)) => Some((
                tool.tool_call_id.as_str().to_owned(),
                tool.result.model_facing_projection().as_text(),
            )),
            _ => None,
        })
        .collect();
    assert!(
        expected[1]
            .1
            .starts_with("Tool call failed: invalid ask_user arguments:")
    );
    let schema = request.tools[0].input_schema.clone();
    assert_no_pattern(&schema);
    let events = crate::common::collect_events(adapter.as_ref(), request).await;
    assert!(
        matches!(events.last(), Some(ModelEvent::Completed { .. })),
        "{protocol:?}: {events:?}"
    );
    assert_eq!(server.attempt_count(), 1);
    let body: Value = serde_json::from_str(&server.request_body(0)).expect("serialized request");
    let (wire_schema, wire_results): (&Value, Vec<(String, String)>) = match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            let messages = body["messages"].as_array().expect("chat messages");
            let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
            let calls = assistant["tool_calls"].as_array().expect("assistant calls");
            assert_eq!(
                calls
                    .iter()
                    .map(|c| c["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                CALL_IDS
            );
            let results = messages
                .iter()
                .filter(|m| m["role"] == "tool")
                .map(|tool| {
                    (
                        tool["tool_call_id"].as_str().unwrap().to_owned(),
                        text_parts(&tool["content"]),
                    )
                })
                .collect();
            (&body["tools"][0]["function"]["parameters"], results)
        }
        ModelProtocol::OpenAiResponses => {
            let items = body["input"].as_array().expect("Responses input items");
            let calls = items.iter().filter(|m| m["type"] == "function_call");
            assert_eq!(
                calls
                    .map(|c| c["call_id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                CALL_IDS
            );
            let results = items
                .iter()
                .filter(|m| m["type"] == "function_call_output")
                .map(|tool| {
                    (
                        tool["call_id"].as_str().unwrap().to_owned(),
                        tool["output"].as_str().expect("text output").to_owned(),
                    )
                })
                .collect();
            (&body["tools"][0]["parameters"], results)
        }
        ModelProtocol::AnthropicMessages => {
            let messages = body["messages"].as_array().expect("Anthropic messages");
            let assistant_index = messages
                .iter()
                .position(|m| m["role"] == "assistant")
                .unwrap();
            let calls = messages[assistant_index]["content"].as_array().unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| c["type"] == "tool_use")
                    .map(|c| c["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                CALL_IDS
            );
            assert_eq!(messages[assistant_index + 1]["role"], "user");
            let next_content = messages[assistant_index + 1]["content"].as_array().unwrap();
            assert_eq!(
                next_content.len(),
                CALL_IDS.len(),
                "one ordered tool-result batch immediately follows the assistant"
            );
            let results = messages
                .iter()
                .flat_map(|message| {
                    let content = message["content"].as_array().expect("content blocks");
                    content
                        .iter()
                        .filter(|part| part["type"] == "tool_result")
                        .map(move |tool| {
                            assert_eq!(message["role"], "user");
                            (
                                tool["tool_use_id"].as_str().unwrap().to_owned(),
                                text_parts(&tool["content"]),
                            )
                        })
                })
                .collect();
            (&body["tools"][0]["input_schema"], results)
        }
    };
    assert_eq!(
        wire_schema, &schema,
        "compiled native schema reaches the adapter unchanged"
    );
    assert_no_pattern(wire_schema);
    assert_eq!(
        wire_results, expected,
        "all result identities, ordering, and complete projections survive without drops or duplicates"
    );
    for (_, text) in &wire_results {
        assert!(!text.is_empty());
        assert!(text.len() <= MAX_MODEL_TOOL_RESULT_BYTES);
    }
}

#[tokio::test]
async fn chat_serializes_actionable_ask_user_rejection_and_compiled_schema() {
    assert_rejection_wire(ModelProtocol::OpenAiChatCompletions).await;
}

#[tokio::test]
async fn responses_serializes_actionable_ask_user_rejection_and_compiled_schema() {
    assert_rejection_wire(ModelProtocol::OpenAiResponses).await;
}

#[tokio::test]
async fn anthropic_serializes_actionable_ask_user_rejection_and_compiled_schema() {
    assert_rejection_wire(ModelProtocol::AnthropicMessages).await;
}

fn unicode_questionnaire() -> Value {
    json!({"questions": [{
        "question": "您希望首页采用哪一种视觉风格？",
        "header": "首页风格",
        "options": [
            {
                "label": "清晰简洁",
                "description": "突出核心信息，减少视觉干扰。",
                "preview": "## 首页预览\n欢迎探索我们的产品。"
            },
            {"label": "生动叙事", "description": "以丰富的图像和故事展示产品特色。"},
            {"label": "专业可信", "description": "通过案例与数据建立专业形象。"}
        ]
    }]})
}

fn argument_stream(protocol: ModelProtocol, fragments: &[String]) -> String {
    let call_id = "call-unicode-ask-user";
    let events = match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            let chunk = |delta: Value, finish_reason: Value| {
                json!({"id":"chat-unicode", "object":"chat.completion.chunk", "created":1,
                    "model":"wire-test", "choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]})
            };
            let mut events = vec![chunk(
                json!({"role":"assistant","tool_calls":[{"index":0,"id":call_id,"type":"function",
                    "function":{"name":"ask_user","arguments":""}}]}),
                Value::Null,
            )];
            events.extend(fragments.iter().map(|fragment| {
                chunk(
                    json!({"tool_calls":[{"index":0,"function":{"arguments":fragment}}]}),
                    Value::Null,
                )
            }));
            events.push(chunk(json!({}), json!("tool_calls")));
            events
        }
        ModelProtocol::OpenAiResponses => {
            let completed_item = json!({"id":"fc-unicode", "type":"function_call", "status":"completed",
                "call_id":call_id, "name":"ask_user", "arguments":fragments.concat()});
            let mut events = vec![
                json!({"type":"response.output_item.added", "output_index":0,
                "item":{"id":"fc-unicode", "type":"function_call", "status":"in_progress",
                    "call_id":call_id, "name":"ask_user", "arguments":""}}),
            ];
            events.extend(fragments.iter().map(|fragment| {
                json!({"type":"response.function_call_arguments.delta", "item_id":"fc-unicode",
                    "output_index":0, "delta":fragment})
            }));
            events.push(
                json!({"type":"response.function_call_arguments.done", "item_id":"fc-unicode",
                "output_index":0, "arguments":fragments.concat()}),
            );
            events.push(json!({"type":"response.output_item.done", "output_index":0,
                "item":completed_item}));
            events.push(json!({"type":"response.completed", "response":{"id":"resp-unicode",
                "object":"response", "status":"completed", "model":"wire-test", "output":[completed_item],
                "usage":{"input_tokens":20,"output_tokens":80,"total_tokens":100}}}));
            for (index, event) in events.iter_mut().enumerate() {
                event["sequence_number"] = json!(index + 1);
            }
            events
        }
        ModelProtocol::AnthropicMessages => {
            let mut events = vec![
                json!({"type":"message_start", "message":{"id":"msg-unicode", "type":"message",
                    "role":"assistant", "model":"wire-test", "content":[], "stop_reason":null,
                    "stop_sequence":null, "usage":{"input_tokens":20,"output_tokens":1}}}),
                json!({"type":"content_block_start", "index":0,
                    "content_block":{"type":"tool_use","id":call_id,"name":"ask_user","input":{}}}),
            ];
            events.extend(fragments.iter().map(|fragment| {
                json!({"type":"content_block_delta", "index":0,
                    "delta":{"type":"input_json_delta", "partial_json":fragment}})
            }));
            events.push(json!({"type":"content_block_stop", "index":0}));
            events.push(
                json!({"type":"message_delta", "delta":{"stop_reason":"tool_use",
                "stop_sequence":null}, "usage":{"input_tokens":20,"output_tokens":80}}),
            );
            events.push(json!({"type":"message_stop"}));
            events
        }
    };
    let mut stream = events
        .into_iter()
        .map(|event| {
            if protocol == ModelProtocol::AnthropicMessages {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            } else {
                format!("data: {event}\n\n")
            }
        })
        .collect::<String>();
    if protocol == ModelProtocol::OpenAiChatCompletions {
        stream.push_str("data: [DONE]\n\n");
    }
    stream
}

async fn assert_unicode_stream(protocol: ModelProtocol) {
    let original = unicode_questionnaire();
    let raw = serde_json::to_string(&original).unwrap();
    // Single-character fragments deliberately split every multi-character
    // Chinese field value and JSON escape; adapters must assemble them exactly.
    let fragments: Vec<String> = raw.chars().map(|character| character.to_string()).collect();
    let stream = argument_stream(protocol, &fragments);
    let server = crate::common::FixtureServer::start(move |_, _| {
        crate::common::FixtureReply::body(200, "OK", "text/event-stream", stream.clone())
    })
    .await;
    let fixture = crate::common::native_fixture_without_extensions();
    let definition = fixture
        .registry
        .model_definitions()
        .into_iter()
        .find(|tool| tool.name == "ask_user")
        .unwrap();
    let mut request =
        crate::common::simple_request(protocol, "wire-test", "Ask about the homepage.");
    request.tools = vec![definition.clone()];
    let events =
        crate::common::collect_events(adapter_for(protocol, &server).as_ref(), request).await;
    assert!(
        matches!(
            events.last(),
            Some(ModelEvent::Completed {
                finish_reason: rustx::model::finish::ModelFinishReason::ToolCalls,
                ..
            })
        ),
        "{protocol:?}: {events:?}"
    );
    let observed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallArgumentsDelta {
                arguments_delta, ..
            } => Some(arguments_delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        observed, fragments,
        "every raw Unicode argument fragment survives"
    );
    assert_eq!(observed.concat(), raw);
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallCompleted { call, .. } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 1);
    let call = completed[0];
    assert_eq!(call.id, ToolCallId::new("call-unicode-ask-user"));
    assert_eq!(call.tool_id, definition.id);
    assert_eq!(call.name, "ask_user");
    assert_eq!(
        call.arguments, original,
        "deserialized field values remain exact"
    );
    let PreflightOutcome::Ready(prepared) = fixture.registry.preflight(call).unwrap() else {
        panic!("the assembled Unicode questionnaire must pass native preflight");
    };
    let mut normalized = original;
    normalized["questions"][0]["multi_select"] = json!(false);
    assert_eq!(
        prepared.invocation.arguments, normalized,
        "normalization adds only the documented default"
    );
    assert_eq!(server.attempt_count(), 1);
}

#[tokio::test]
async fn chat_assembles_exact_unicode_ask_user_arguments() {
    assert_unicode_stream(ModelProtocol::OpenAiChatCompletions).await;
}

#[tokio::test]
async fn responses_assembles_exact_unicode_ask_user_arguments() {
    assert_unicode_stream(ModelProtocol::OpenAiResponses).await;
}

#[tokio::test]
async fn anthropic_assembles_exact_unicode_ask_user_arguments() {
    assert_unicode_stream(ModelProtocol::AnthropicMessages).await;
}
