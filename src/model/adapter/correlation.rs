//! Wire correlation is independent of physical and projected occurrence identity.
use crate::model::types::ModelProtocol;
use serde_json::Value;

/// Disambiguate completed historical batches while retaining the newest native
/// call label (required by stored continuation). Provider signatures and opaque
/// replay values are untouched; only explicit wire correlation fields change.
pub(crate) fn encode(value: &mut Value, protocol: ModelProtocol) -> Result<(), &'static str> {
    let mut raw = Vec::new();
    let mut pending_ids = std::collections::BTreeSet::new();
    let mut invalid = None;
    // Wire strings cannot identify which of two unresolved occurrences owns
    // a result. Reject before editing the detached wire object, not after an
    // overwritten association has reached the provider or frozen metadata.
    visit(value, protocol, |call, id| {
        let source = id.as_str().unwrap_or_default();
        if source.is_empty() {
            invalid = Some("empty provider wire correlation ID");
        }
        if call {
            if !pending_ids.insert(source.to_owned()) {
                invalid = Some("ambiguous unresolved provider wire correlation ID");
            }
            raw.push(source.to_owned());
        } else {
            pending_ids.remove(source);
        }
    });
    if let Some(error) = invalid {
        return Err(error);
    }
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
    Ok(())
}
/// Actual explicit wire labels for neutral request coordinates. Covered
/// Responses input has no wire fields in this request. The accepted stored
/// output supplies native correlation for its explicitly transmitted results.
pub(crate) fn request_labels(
    request: &crate::model::ModelRequest,
) -> Result<std::collections::BTreeMap<(u32, u32), String>, String> {
    use crate::message::types::AssistantContentBlock;
    use crate::model::projected::ModelMessageView;
    use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    let boundary = request.continuation_boundary()?;
    let stored = request.protocol() == ModelProtocol::OpenAiResponses
        && matches!(
            request.continuation,
            Some(ProviderContinuationState::OpenAiResponses(
                OpenAiResponsesContinuation::Stored { .. }
            ))
        );
    let start = if request.protocol() == ModelProtocol::OpenAiResponses {
        boundary.map_or(0, |at| at + usize::from(stored))
    } else {
        0
    };
    let mut calls = Vec::new();
    for (message, item) in request.messages.iter().enumerate().skip(start) {
        if let ModelMessageView::Assistant(content) = item.view() {
            for (block, item) in content.iter().enumerate() {
                if let AssistantContentBlock::ToolCall(call) = item {
                    calls.push((
                        (
                            u32::try_from(message)
                                .map_err(|_| "request message coordinate overflow")?,
                            u32::try_from(block)
                                .map_err(|_| "request block coordinate overflow")?,
                        ),
                        call.id.to_string(),
                    ));
                }
            }
        }
    }
    let labels = crate::model::occurrence::wire_labels(
        &calls.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
    );
    let mut mapped: std::collections::BTreeMap<_, _> = calls
        .into_iter()
        .zip(labels)
        .map(|((coordinate, _), label)| (coordinate, label))
        .collect();
    if stored {
        let at = boundary.ok_or("stored continuation lacks accepted output")?;
        if let ModelMessageView::Assistant(content) = request.messages[at].view() {
            let mut native_ids = std::collections::BTreeSet::new();
            for (block, item) in content.iter().enumerate() {
                if let AssistantContentBlock::ToolCall(call) = item {
                    if call.id.as_str().is_empty() || !native_ids.insert(call.id.as_str()) {
                        return Err("ambiguous stored native wire correlation ID".into());
                    }
                    mapped.insert(
                        (
                            u32::try_from(at).map_err(|_| "request message coordinate overflow")?,
                            u32::try_from(block)
                                .map_err(|_| "request block coordinate overflow")?,
                        ),
                        call.id.to_string(),
                    );
                }
            }
        }
    }
    Ok(mapped)
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
pub(crate) fn observed_correlations(
    value: &mut Value,
    protocol: ModelProtocol,
) -> Vec<(bool, String)> {
    let mut observed = Vec::new();
    visit(value, protocol, |call, id| {
        observed.push((
            call,
            id.as_str().expect("wire correlation string").to_owned(),
        ));
    });
    observed
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
            encode(&mut value, protocol).unwrap();
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
            encode(&mut retry, protocol).unwrap();
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
