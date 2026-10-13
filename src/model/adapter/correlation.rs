//! Wire correlation is independent of physical and projected occurrence identity.
use crate::model::types::ModelProtocol;
use serde_json::Value;

/// Disambiguate completed historical batches while retaining the newest native
/// call label (required by stored continuation). Provider signatures and opaque
/// replay values are untouched; only explicit wire correlation fields change.
pub(crate) fn encode(value: &mut Value, protocol: ModelProtocol) {
    let mut raw = Vec::new();
    visit(value, protocol, |call, id| {
        if call {
            raw.push(id.as_str().unwrap_or_default().to_owned());
        }
    });
    let labels = crate::model::occurrence::wire_labels(&raw);
    let mut ordinal = 0;
    let mut pending = std::collections::BTreeMap::new();
    visit(value, protocol, |call, id| {
        let source = id.as_str().unwrap_or_default().to_owned();
        if call {
            let label = labels[ordinal].clone();
            ordinal += 1;
            pending.insert(source, label.clone());
            *id = Value::String(label);
        } else if let Some(label) = pending.remove(&source) {
            *id = Value::String(label);
        }
    });
}
fn visit(value: &mut Value, protocol: ModelProtocol, mut field: impl FnMut(bool, &mut Value)) {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            if let Some(messages) = value["messages"].as_array_mut() {
                for message in messages {
                    if let Some(calls) = message["tool_calls"].as_array_mut() {
                        for call in calls {
                            field(true, &mut call["id"]);
                        }
                    }
                    if message["role"] == "tool" {
                        field(false, &mut message["tool_call_id"]);
                    }
                }
            }
        }
        ModelProtocol::AnthropicMessages => {
            if let Some(messages) = value["messages"].as_array_mut() {
                for message in messages {
                    if let Some(blocks) = message["content"].as_array_mut() {
                        for block in blocks {
                            match block["type"].as_str() {
                                Some("tool_use") => field(true, &mut block["id"]),
                                Some("tool_result") => field(false, &mut block["tool_use_id"]),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        ModelProtocol::OpenAiResponses => {
            if let Some(items) = value["input"].as_array_mut() {
                for item in items {
                    match item["type"].as_str() {
                        Some("function_call") => field(true, &mut item["call_id"]),
                        Some("function_call_output") => field(false, &mut item["call_id"]),
                        _ => {}
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_wire_ids_are_paired_by_occurrence_in_each_explicit_protocol() {
        let fixtures = [
            (
                ModelProtocol::OpenAiChatCompletions,
                serde_json::json!({"messages":[{"role":"assistant","tool_calls":[{"id":"repeat"}]},{"role":"tool","tool_call_id":"repeat"},{"role":"assistant","tool_calls":[{"id":"repeat"}]},{"role":"tool","tool_call_id":"repeat"}]}),
            ),
            (
                ModelProtocol::AnthropicMessages,
                serde_json::json!({"messages":[{"content":[{"type":"thinking","signature":"untouched","thinking":"native"},{"type":"tool_use","id":"repeat"}]},{"content":[{"type":"tool_result","tool_use_id":"repeat"}]},{"content":[{"type":"tool_use","id":"repeat"}]},{"content":[{"type":"tool_result","tool_use_id":"repeat"}]}]}),
            ),
            (
                ModelProtocol::OpenAiResponses,
                serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"untouched"},{"type":"function_call","call_id":"repeat"},{"type":"function_call_output","call_id":"repeat"},{"type":"function_call","call_id":"repeat"},{"type":"function_call_output","call_id":"repeat"}]}),
            ),
        ];
        for (protocol, mut value) in fixtures {
            let source = value.clone();
            encode(&mut value, protocol);
            let mut associations = Vec::new();
            visit(&mut value, protocol, |call, id| {
                associations.push((call, id.clone()));
            });
            assert_eq!(
                associations,
                vec![
                    (true, Value::from("rustx_occurrence_0")),
                    (false, Value::from("rustx_occurrence_0")),
                    (true, Value::from("repeat")),
                    (false, Value::from("repeat"))
                ]
            );
            let mut retry = source;
            encode(&mut retry, protocol);
            assert_eq!(value, retry);
            if protocol == ModelProtocol::AnthropicMessages {
                assert_eq!(value["messages"][0]["content"][0]["signature"], "untouched");
            }
            if protocol == ModelProtocol::OpenAiResponses {
                assert_eq!(value["input"][0]["encrypted_content"], "untouched");
            }
        }
    }
}
