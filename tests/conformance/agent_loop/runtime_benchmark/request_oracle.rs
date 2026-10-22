//! Independent fixed-workload expectations, not selected from captured history.
use serde_json::{Value, json};

const TIME_PREFIX: &str = "<system-reminder>\nTimezone: UTC\nCurrent time: ";
const TIME_SUFFIX: &str = "\n</system-reminder>";
const NOTE: &str = "deterministic-note-payload-6d41";

fn text(role: &str, value: &str) -> Value {
    json!({"role":role,"content":[{"type":"text","text":value}],"tool_calls":null})
}

pub(super) fn expected_body(case: &str, index: usize) -> Value {
    let mut messages = vec![
        text("user", "benchmark turn 0000"),
        text("user", &format!("{TIME_PREFIX}<TIME>{TIME_SUFFIX}")),
    ];
    if case.starts_with("short-") {
        for turn in 0..index {
            messages.push(text("assistant", &format!("reply {turn:04}")));
            messages.push(text("user", &format!("benchmark turn {:04}", turn + 1)));
        }
    } else if case == "tool" && index == 1 {
        messages.push(json!({"role":"assistant","content":[],"tool_calls":[{
            "type":"function","id":"benchmark-read","function":{"name":"read",
            "arguments":{"path":"<WORKSPACE>/note.txt"}}}]}));
        messages.push(json!({"role":"tool","tool_call_id":"benchmark-read",
            "content":[{"type":"text","text":NOTE}],"tool_calls":null}));
    } else {
        assert_eq!(index, 0);
    }
    json!({"messages":messages,"model":"chat-model","stream":true,
        "stream_options":{"include_usage":true},"max_completion_tokens":1024,
        "tools":serde_json::from_str::<Value>(include_str!("expected_tools.json")).unwrap()})
}

/// JSON argument spelling is transport-only; timestamp/path normalization is
/// applied only for the independent expectation, never wire/frozen equivalence.
fn normalize(body: &Value, workspace: &str, dynamic: bool) -> Value {
    let mut body = body.clone();
    for message in body["messages"].as_array_mut().unwrap() {
        for block in message["content"].as_array_mut().unwrap() {
            if let Some(value) = block["text"].as_str()
                && value.starts_with(TIME_PREFIX)
                && dynamic
            {
                let timestamp = value
                    .strip_prefix(TIME_PREFIX)
                    .unwrap()
                    .strip_suffix(TIME_SUFFIX)
                    .expect("complete time context");
                let parsed = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S")
                    .expect("valid generated UTC timestamp");
                assert_eq!(parsed.format("%Y-%m-%d %H:%M:%S").to_string(), timestamp);
                block["text"] = json!(format!("{TIME_PREFIX}<TIME>{TIME_SUFFIX}"));
            }
        }
        if let Some(calls) = message["tool_calls"].as_array_mut() {
            for call in calls {
                let mut arguments: Value = serde_json::from_str(
                    call["function"]["arguments"]
                        .as_str()
                        .expect("wire JSON arguments"),
                )
                .unwrap();
                if dynamic && arguments["path"] == json!(format!("{workspace}/note.txt")) {
                    arguments["path"] = json!("<WORKSPACE>/note.txt");
                }
                call["function"]["arguments"] = arguments;
            }
        }
    }
    body
}

/// Deliberately small independent projection of this fixture's supported
/// semantic vocabulary. It does not call the production provider translator.
pub(super) fn reconstructed_body(request: &Value) -> Value {
    assert_eq!(
        request["invocation"],
        json!({"model":"chat-model",
        "protocol":"openai_chat_completions","max_output_tokens":1024,
        "request_params":{},"capabilities":{"inputModalities":["text"],
        "outputModalities":["text"],"toolCalls":true},
        "compat":{"chatReasoningReplay":"omit"}})
    );
    assert_eq!(request["effective_system_prompt"], "");
    assert!(request["continuation"].is_null());
    assert!(request["continuation_source"].is_null());
    let tools: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            assert_eq!(
                tool["id"],
                json!(format!("tool-{}", tool["name"].as_str().unwrap()))
            );
            json!({"type":"function","function":{"name":tool["name"],
            "description":tool["description"],"parameters":tool["input_schema"]}})
        })
        .collect();
    let messages: Vec<_> = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            assert_eq!(message["input_kind"], "canonical");
            match message["role"].as_str().unwrap() {
                "user" => json!({"role":"user","content":message["content"],"tool_calls":null}),
                "assistant" => {
                    let mut content = Vec::new();
                    let mut calls = Vec::new();
                    for block in message["content"].as_array().unwrap() {
                        match block["type"].as_str().unwrap() {
                            "text" => content.push(block.clone()),
                            "tool_call" => {
                                assert_eq!(block["tool_id"], "tool-read");
                                calls.push(json!({"type":"function","id":block["id"],
                                "function":{"name":block["name"],
                                "arguments":serde_json::to_string(&block["arguments"]).unwrap()}}));
                            }
                            other => panic!("unexpected assistant block {other}"),
                        }
                    }
                    json!({"role":"assistant","content":content,
                    "tool_calls":if calls.is_empty() { Value::Null } else { json!(calls) }})
                }
                "tool" => {
                    assert_eq!(message["tool_id"], "tool-read");
                    assert_eq!(message["result"]["status"], json!({"type":"success"}));
                    assert_eq!(message["result"]["artifacts"], json!([]));
                    json!({"role":"tool","tool_call_id":message["tool_call_id"],
                    "content":message["result"]["content"],"tool_calls":null})
                }
                other => panic!("unexpected semantic role {other}"),
            }
        })
        .collect();
    json!({"messages":messages,"model":"chat-model","stream":true,
        "stream_options":{"include_usage":true},"max_completion_tokens":1024,"tools":tools})
}

pub(super) fn assert_bodies(
    case: &str,
    index: usize,
    workspace: &str,
    wire: &Value,
    frozen: &Value,
) -> Value {
    assert_eq!(
        normalize(wire, workspace, false),
        normalize(frozen, workspace, false),
        "complete wire/frozen input differs, including generated timestamp and path"
    );
    let expected = expected_body(case, index);
    assert_eq!(
        normalize(wire, workspace, true),
        expected,
        "complete ordered wire input differs from independent workload"
    );
    assert_eq!(
        normalize(frozen, workspace, true),
        expected,
        "complete ordered frozen input differs from independent workload"
    );
    expected
}

pub(super) fn assert_identity(
    snapshot: &Value,
    request: &Value,
    start: &Value,
    conversation: &str,
) {
    assert_eq!(start["event"]["request_id"], snapshot["request_id"]);
    assert_eq!(start["event"]["model"], "chat-model");
    assert_eq!(start["conversation_id"], conversation);
    assert_eq!(start["attempt_id"], snapshot["identity"]["attempt_id"]);
    assert_eq!(start["turn_id"], snapshot["identity"]["turn"]);
    assert_eq!(snapshot["identity"]["retry_number"], 0);
    let inputs = snapshot["input"].as_array().unwrap();
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(inputs.len(), messages.len());
    for (input, message) in inputs.iter().zip(messages) {
        assert_eq!(input["input_kind"], "original");
        assert_eq!(input["conversation_id"], conversation);
        assert_eq!(input["message_id"], message["id"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wire(index: usize) -> Value {
        let mut body = expected_body("short-large", index);
        body["messages"][1]["content"][0]["text"] =
            json!(format!("{TIME_PREFIX}2026-10-17 00:00:00{TIME_SUFFIX}"));
        body
    }
    #[test]
    fn complete_oracle_rejects_omitted_older_history_even_when_wire_and_frozen_agree() {
        let good = wire(64);
        assert_bodies("short-large", 64, "/fixture", &good, &good);
        let mut bad = good;
        bad["messages"].as_array_mut().unwrap().remove(2);
        assert!(
            std::panic::catch_unwind(|| assert_bodies("short-large", 64, "/fixture", &bad, &bad))
                .is_err()
        );
    }
    #[test]
    fn complete_oracle_rejects_reordered_older_history_even_when_wire_and_frozen_agree() {
        let good = wire(64);
        assert_bodies("short-large", 64, "/fixture", &good, &good);
        let mut bad = good;
        bad["messages"].as_array_mut().unwrap().swap(2, 4);
        assert!(
            std::panic::catch_unwind(|| assert_bodies("short-large", 64, "/fixture", &bad, &bad))
                .is_err()
        );
    }
    #[test]
    fn complete_oracle_rejects_mismatched_frozen_content_and_generated_time() {
        let good = wire(64);
        assert_bodies("short-large", 64, "/fixture", &good, &good);
        for index in [1, 2] {
            let mut bad = good.clone();
            bad["messages"][index]["content"][0]["text"] = json!("different frozen input");
            assert!(
                std::panic::catch_unwind(|| assert_bodies(
                    "short-large",
                    64,
                    "/fixture",
                    &good,
                    &bad
                ))
                .is_err()
            );
        }
    }
    #[test]
    fn complete_oracle_rejects_changed_tool_schema() {
        let good = wire(0);
        assert_bodies("short-small", 0, "/fixture", &good, &good);
        let mut bad = good;
        bad["tools"].as_array_mut().unwrap().remove(0);
        assert!(
            std::panic::catch_unwind(|| assert_bodies("short-small", 0, "/fixture", &bad, &bad))
                .is_err()
        );
    }
    #[test]
    fn complete_oracle_rejects_frozen_identity_and_original_reference_mismatch() {
        let start = json!({"event":{"request_id":"request-1","model":"chat-model"},
            "conversation_id":"conversation","attempt_id":"attempt","turn_id":"1"});
        let snapshot = json!({"request_id":"request-1",
            "identity":{"attempt_id":"attempt","turn":"1","retry_number":0},
            "input":[{"input_kind":"original","conversation_id":"conversation","message_id":"message"}]});
        let request = json!({"messages":[{"id":"message"}]});
        assert_identity(&snapshot, &request, &start, "conversation");
        let mut wrong_identity = snapshot.clone();
        wrong_identity["identity"]["attempt_id"] = json!("another attempt");
        assert!(
            std::panic::catch_unwind(|| assert_identity(
                &wrong_identity,
                &request,
                &start,
                "conversation"
            ))
            .is_err()
        );
        let mut wrong_reference = snapshot;
        wrong_reference["input"][0]["message_id"] = json!("another original");
        assert!(
            std::panic::catch_unwind(|| assert_identity(
                &wrong_reference,
                &request,
                &start,
                "conversation"
            ))
            .is_err()
        );
    }
}
