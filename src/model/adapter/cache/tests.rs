use std::sync::Arc;

use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
    ToolCallOccurrenceRef, ToolMessageBlock,
};
use crate::model::{ModelInputMessage, ModelProtocol, ModelRequest};
use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
use crate::tools::types::{ModelToolDefinition, ToolCall};

pub(crate) fn request() -> ModelRequest {
    let mut request = crate::scripted_suites::common::simple_request(
        ModelProtocol::OpenAiChatCompletions,
        "fixture",
        "turn growth",
    );
    request.messages.clear();
    request.tools.push(ModelToolDefinition {
        id: ToolId::new("read"),
        name: "read".into(),
        description: "description".repeat(32),
        input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}})
            .into(),
    });
    request
}

pub(crate) fn append(request: &mut ModelRequest, turn: usize, tool: bool, repeated: bool) {
    let user = crate::scripted_suites::common::simple_request(
        ModelProtocol::OpenAiChatCompletions,
        "fixture",
        "turn growth",
    )
    .messages[0]
        .clone();
    request.messages.push(user);
    if tool {
        let assistant = MessageId::new(format!("call-{turn:08}"));
        let id = ToolCallId::new(if repeated {
            "repeat".into()
        } else {
            format!("read-{turn:08}")
        });
        request.messages.push(
            MessageBlock::Assistant(AssistantMessageBlock {
                id: assistant.clone(),
                content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: id.clone(),
                    tool_id: ToolId::new("read"),
                    name: "read".into(),
                    arguments: serde_json::json!({"path":"fixture.txt"}),
                })],
            })
            .into(),
        );
        request.messages.push(
            MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!("result-{turn:08}")),
                tool_call_id: id,
                tool_id: ToolId::new("read"),
                occurrence: ToolCallOccurrenceRef::new(assistant, ContentBlockIndex::new(0)),
                result: crate::scripted_suites::support::fake::success_result("fixed tool result")
                    .into(),
            })
            .into(),
        );
    }
    request.messages.push(
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(format!("reply-{turn:08}")),
            content: vec![AssistantContentBlock::Text(crate::message::TextBlock {
                text: "ok".into(),
            })],
        })
        .into(),
    );
}

pub(crate) fn prepared(request: &ModelRequest) -> Vec<u8> {
    super::super::validation::prepare_request(request.clone(), request.protocol())
        .unwrap()
        .body
        .to_vec()
}
pub(crate) fn oracle(request: &ModelRequest) -> Vec<u8> {
    let tools = super::super::validation::validate_request(request, request.protocol()).unwrap();
    match request.protocol() {
        ModelProtocol::OpenAiChatCompletions => serde_json::to_vec(
            &super::super::openai::chat_completions::uncached_request(request).unwrap(),
        )
        .unwrap(),
        ModelProtocol::OpenAiResponses => serde_json::to_vec(
            &super::super::openai::responses::uncached_request(
                request,
                &tools,
                request.invocation.compat.responses_storage,
            )
            .unwrap(),
        )
        .unwrap(),
        ModelProtocol::AnthropicMessages => serde_json::to_vec(
            &super::super::anthropic::mapping::uncached_request(request, &tools).unwrap(),
        )
        .unwrap(),
    }
}

#[test]
fn incremental_preparation_encodes_only_delta_at_8_64_512_1024_turns() {
    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        for tool in [false, true] {
            for repeated in [false, true] {
                let mut sizes = Vec::new();
                for history in [8, 64, 512, 1024] {
                    let mut request = request();
                    request.invocation.protocol = protocol;
                    for turn in 0..history {
                        append(&mut request, turn, tool, repeated);
                    }
                    assert_eq!(prepared(&request), oracle(&request));
                    append(&mut request, history, tool, repeated);
                    super::take_work();
                    let body = prepared(&request);
                    let work = super::take_work();
                    assert_eq!(body, oracle(&request));
                    let delta = if tool { 4 } else { 2 };
                    assert_eq!(work.admission_items, delta);
                    assert_eq!(work.correlation_items, delta);
                    assert_eq!(
                        work.encoded_items,
                        delta + if tool && repeated { 2 } else { 0 }
                    );
                    assert_eq!(work.catalog_bytes, 0);
                    assert!(work.encoded_bytes < 2048, "{work:?}");
                    sizes.push(work.encoded_bytes);
                    println!(
                        "protocol={protocol:?} history={history} tool={tool} repeated={repeated} {work:?}"
                    );
                }
                if protocol != ModelProtocol::OpenAiResponses && (!tool || !repeated) {
                    assert!(sizes.windows(2).all(|pair| pair[0] == pair[1]));
                }
                assert!(sizes.iter().max().unwrap() - sizes.iter().min().unwrap() < 32);
            }
        }
    }
}

#[test]
fn old_cuts_edits_and_signed_zero_never_reuse_stale_wire() {
    let mut request = request();
    append(&mut request, 0, true, false);
    if let ModelInputMessage::Canonical(MessageBlock::Assistant(message)) = &mut request.messages[1]
        && let AssistantContentBlock::ToolCall(call) = &mut message.content[0]
    {
        call.arguments = serde_json::from_str("{\"x\":-0.0}").unwrap();
    }
    let old = request.clone();
    let old_body = prepared(&old);
    if let ModelInputMessage::Canonical(MessageBlock::Assistant(message)) = &mut request.messages[1]
        && let AssistantContentBlock::ToolCall(call) = &mut message.content[0]
    {
        call.arguments = serde_json::from_str("{\"x\":0.0}").unwrap();
    }
    let body = prepared(&request);
    assert_ne!(old_body, body);
    assert_eq!(body, oracle(&request));
    append(&mut request, 1, true, false);
    assert_eq!(prepared(&request), oracle(&request));
    assert_eq!(prepared(&old), old_body);
    assert_eq!(prepared(&request), oracle(&request));
    request.messages.insert(1, request.messages[0].clone());
    assert_eq!(prepared(&request), oracle(&request));
}

#[test]
fn tool_schema_owner_capabilities_and_cache_lifetime_are_checked() {
    let mut request = request();
    append(&mut request, 0, false, false);
    request.tools[0].input_schema = Arc::new(serde_json::from_str("{\"default\":-0.0}").unwrap());
    let old = prepared(&request);
    request.tools[0].input_schema = Arc::new(serde_json::from_str("{\"default\":0.0}").unwrap());
    assert_ne!(old, prepared(&request));
    assert_eq!(prepared(&request), oracle(&request));
    request.invocation.capabilities.input_modalities.clear();
    assert!(
        super::super::validation::prepare_request(request.clone(), request.protocol()).is_err()
    );
    let frozen = request
        .messages
        .freeze(&crate::runtime::identity::ConversationId::generate());
    let cache = Arc::downgrade(request.messages.preparation());
    drop(request);
    assert!(
        cache.upgrade().is_none(),
        "prefix certificates must not form an ownership cycle"
    );
    assert!(
        !frozen.is_empty(),
        "frozen facts survive without a transport cache owner"
    );
}

#[test]
fn semantic_prefix_hashing_never_rehashes_old_owners() {
    for history in [8, 64, 512, 1024] {
        let mut request = request();
        for turn in 0..history {
            append(&mut request, turn, false, false);
        }
        let before = request.messages.len();
        super::prefix_fingerprint(&request.messages, before);
        append(&mut request, history, false, false);
        crate::model::selected_input::take_hash_work();
        let actual = super::prefix_fingerprint(&request.messages, request.messages.len());
        let work = crate::model::selected_input::take_hash_work();
        assert_eq!(
            actual,
            crate::model::types::semantic_messages_fingerprint(&request.messages)
        );
        assert!(work.1 < 512, "history={history}, {work:?}");
        println!("semantic history={history} hashed_bytes={}", work.1);
    }
}

#[test]
fn responses_new_native_ids_relabel_only_the_conflicting_old_message() {
    use crate::message::types::ReasoningBlock;
    use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    let mut request = request();
    request.invocation.protocol = ModelProtocol::OpenAiResponses;
    append(&mut request, 0, false, false);
    prepared(&request);
    request.messages.push(MessageBlock::Assistant(AssistantMessageBlock {id:MessageId::new("native"),content:vec![AssistantContentBlock::Reasoning(ReasoningBlock {origin:request.invocation.replay_origin(),text:None,provider_state:Some(ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {items:vec![serde_json::json!({"type":"reasoning","id":"msg_rustx_1","encrypted_content":"opaque"})].into()}))})]}).into());
    super::take_work();
    let body = prepared(&request);
    let work = super::take_work();
    assert_eq!(work.encoded_items, 2);
    assert_eq!(body, oracle(&request));
    assert!(String::from_utf8(body).unwrap().contains("msg_rustx_1_1"));
}

#[test]
fn anthropic_tool_groups_span_requests_and_empty_assistants_end_groups() {
    let mut request = request();
    request.invocation.protocol = ModelProtocol::AnthropicMessages;
    append(&mut request, 0, true, false);
    let result = request.messages[2].clone();
    request.messages = request.messages.range(0..3);
    prepared(&request);
    request.messages.push(result.clone());
    super::take_work();
    let body = prepared(&request);
    let work = super::take_work();
    assert_eq!(work.encoded_items, 1);
    assert_eq!(body, oracle(&request));
    request.messages.push(
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("empty"),
            content: vec![],
        })
        .into(),
    );
    request.messages.push(result);
    assert_eq!(prepared(&request), oracle(&request));
    let body: serde_json::Value = serde_json::from_slice(&prepared(&request)).unwrap();
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
}

#[test]
fn rejected_byte_budget_never_encodes_an_oversized_fragment() {
    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        let mut request = request();
        request.invocation.protocol = protocol;
        request.tools.clear();
        request.messages = crate::scripted_suites::common::simple_request(
            protocol,
            "fixture",
            &"x".repeat(1024 * 1024),
        )
        .messages;
        super::take_work();
        let error = super::super::validation::prepare_request_with_limit(
            request.clone(),
            protocol,
            Some(1024),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.kind,
            crate::model::ModelErrorKind::ContextWindowExceeded
        );
        let work = super::take_work();
        assert!(work.encoded_bytes <= 1024);
        assert_eq!(prepared(&request), oracle(&request));
    }
}
