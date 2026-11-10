//! Wire correlation is independent of physical and projected occurrence identity.
use crate::model::types::ModelProtocol;
use serde_json::Value;

/// Labels from the exact request plan, excluding Stored output already owned by
/// the provider. Native labels stay reserved in the complete plan.
pub(super) fn transmitted_labels(
    request: &crate::model::ModelRequest,
    boundary: Option<usize>,
) -> Result<Vec<String>, String> {
    let stored = is_stored(request);
    Ok(request_labels_at(request, boundary)?
        .into_iter()
        .filter(|((message, _), _)| !stored || boundary != Some(*message as usize))
        .map(|(_, label)| label)
        .collect())
}

/// Apply the validated request labels directly to typed wire correlation fields.
pub(super) fn encode_fields(
    labels: &[String],
    mut visit: impl FnMut(&mut dyn FnMut(bool, &mut Value)),
) -> Result<(), &'static str> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = crate::model::adapter::profile::phase(
        crate::model::adapter::profile::Phase::WireCorrelation,
    );
    let mut raw = Vec::new();
    let mut pending_ids = std::collections::BTreeSet::new();
    let mut invalid = None;
    // Wire strings cannot identify which of two unresolved occurrences owns
    // a result. Reject before editing the detached wire object, not after an
    // overwritten association has reached the provider or frozen metadata.
    visit(&mut |call, id| {
        #[cfg(feature = "issue-387-profile")]
        crate::model::adapter::profile::wire_field();
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
    if raw.len() != labels.len() {
        return Err("wire call count differs from request correlation plan");
    }
    let mut ordinal = 0;
    let mut pending = std::collections::BTreeMap::new();
    visit(&mut |call, id| {
        #[cfg(feature = "issue-387-profile")]
        crate::model::adapter::profile::wire_field();
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
/// Explicit labels for each transmitted canonical `ToolCall` coordinate. Stored
/// continuation reserves its boundary's IDs for the results sent without calls.
pub(crate) fn request_labels(
    request: &crate::model::ModelRequest,
) -> Result<std::collections::BTreeMap<(u32, u32), String>, String> {
    request_labels_at(request, request.continuation_boundary()?)
}

pub(crate) fn request_labels_at(
    request: &crate::model::ModelRequest,
    boundary: Option<usize>,
) -> Result<std::collections::BTreeMap<(u32, u32), String>, String> {
    use crate::message::types::AssistantContentBlock;
    use crate::model::projected::ModelMessageView;
    use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    #[cfg(feature = "issue-387-profile")]
    let _profile = crate::model::adapter::profile::phase(
        crate::model::adapter::profile::Phase::RequestCorrelation,
    );
    let stored = is_stored(request);
    let start = if stored {
        boundary.ok_or("stored continuation lacks accepted output")?
    } else {
        0
    };
    let origin = request.invocation.replay_origin();
    let mut calls = Vec::new();
    let indices = if stored {
        (start..request.messages.len()).collect()
    } else {
        request.messages.tool_indices()
    };
    for message in indices {
        #[cfg(feature = "issue-387-profile")]
        crate::model::adapter::profile::item(message);
        if let ModelMessageView::Assistant(content) = request.messages[message].view() {
            let native = request.protocol() == ModelProtocol::OpenAiResponses && (
                (stored && Some(message) == boundary) || content.iter().any(|block| matches!(block,
                    AssistantContentBlock::Reasoning(reasoning) if reasoning.belongs_to(origin.as_ref())
                        && matches!(reasoning.provider_state, Some(ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless { .. })))
                ))
            );
            for (block, item) in content.iter().enumerate() {
                #[cfg(feature = "issue-387-profile")]
                crate::model::adapter::profile::block();
                if let AssistantContentBlock::ToolCall(call) = item {
                    if call.id.as_str().is_empty() {
                        return Err("empty provider wire correlation ID".into());
                    }
                    calls.push((
                        (
                            u32::try_from(message)
                                .map_err(|_| "request message coordinate overflow")?,
                            u32::try_from(block)
                                .map_err(|_| "request block coordinate overflow")?,
                        ),
                        call.id.to_string(),
                        native,
                    ));
                }
            }
        }
    }
    // The allocator preserves the last occurrence of an ID. Give native calls
    // that priority regardless of their chronological position in the history.
    let order = (0..calls.len())
        .filter(|i| !calls[*i].2)
        .chain((0..calls.len()).filter(|i| calls[*i].2))
        .collect::<Vec<_>>();
    let labels = wire_labels(
        &order
            .iter()
            .map(|i| calls[*i].1.clone())
            .collect::<Vec<_>>(),
        request.protocol(),
    );
    let mut mapped = std::collections::BTreeMap::new();
    for (index, label) in order.into_iter().zip(labels) {
        let (coordinate, raw, native) = &calls[index];
        if *native && &label != raw {
            return Err("native output has conflicting or invalid wire correlation IDs".into());
        }
        mapped.insert(*coordinate, label);
    }
    Ok(mapped)
}

fn is_stored(request: &crate::model::ModelRequest) -> bool {
    request.protocol() == ModelProtocol::OpenAiResponses
        && matches!(
            request.continuation,
            Some(
                crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                    crate::runtime::continuation::OpenAiResponsesContinuation::Stored { .. }
                )
            )
        )
}

pub(crate) fn wire_labels(raw: &[String], protocol: ModelProtocol) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut labels = crate::model::occurrence::wire_labels(raw);
    let limit = if protocol == ModelProtocol::OpenAiChatCompletions {
        40
    } else {
        64
    };
    let valid = |id: &str| {
        !id.is_empty()
            && id.len() <= limit
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    };
    let mut occupied = labels
        .iter()
        .filter(|id| valid(id))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for (index, id) in labels.iter_mut().enumerate() {
        if valid(id) {
            continue;
        }
        let hash = format!("{:x}", Sha256::digest(id.as_bytes()));
        let mut nonce = index;
        loop {
            let label = format!("rustx_{}_{nonce}", &hash[..12]);
            if occupied.insert(label.clone()) {
                *id = label;
                break;
            }
            nonce = nonce
                .checked_add(1)
                .expect("labels are bounded by request memory");
        }
    }
    labels
}

#[cfg(test)]
fn encode_with_labels(
    value: &mut Value,
    protocol: ModelProtocol,
    labels: &[String],
) -> Result<(), &'static str> {
    encode_fields(labels, |field| visit(value, protocol, field))
}

#[cfg(test)]
fn visit(value: &mut Value, protocol: ModelProtocol, mut field: impl FnMut(bool, &mut Value)) {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            if let Some(messages) = value["messages"].as_array_mut() {
                for message in messages {
                    #[cfg(feature = "issue-387-profile")]
                    crate::model::adapter::profile::wire_item();
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
                    #[cfg(feature = "issue-387-profile")]
                    crate::model::adapter::profile::wire_item();
                    if let Some(blocks) = message["content"].as_array_mut() {
                        for block in blocks {
                            visit_anthropic_block(block, &mut field);
                        }
                    }
                }
            }
        }
        ModelProtocol::OpenAiResponses => {
            if let Some(items) = value["input"].as_array_mut() {
                for item in items {
                    #[cfg(feature = "issue-387-profile")]
                    crate::model::adapter::profile::wire_item();
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

pub(super) fn visit_anthropic_block(block: &mut Value, mut field: impl FnMut(bool, &mut Value)) {
    match block["type"].as_str() {
        Some("tool_use") => field(true, &mut block["id"]),
        Some("tool_result") => field(false, &mut block["tool_use_id"]),
        _ => {}
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
    use sha2::Digest;

    fn encode_fixture(value: &mut Value, protocol: ModelProtocol) -> Result<(), &'static str> {
        let mut raw = Vec::new();
        visit(value, protocol, |call, id| {
            if call {
                raw.push(id.as_str().unwrap().to_owned());
            }
        });
        encode_with_labels(value, protocol, &wire_labels(&raw, protocol))
    }
    #[test]
    fn anthropic_ids_stay_bounded_and_unique_even_when_aliases_are_occupied() {
        let invalid = "call/foreign|with:punctuation".to_owned();
        let hash = format!("{:x}", sha2::Sha256::digest(invalid.as_bytes()));
        let mut raw = vec!["repeat".into(), "repeat".into()];
        raw.extend((0..70).map(|i| format!("rustx_occurrence_0{}", "_".repeat(i))));
        raw.push(format!("rustx_{}_{}", &hash[..12], raw.len() + 1));
        raw.push(invalid);
        let labels = wire_labels(&raw, ModelProtocol::AnthropicMessages);
        assert_eq!(
            labels
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            raw.len()
        );
        assert!(labels.iter().all(|id| {
            id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        }));
        assert_eq!(labels, wire_labels(&raw, ModelProtocol::AnthropicMessages));
    }
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
            encode_fixture(&mut value, protocol).unwrap();
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
            encode_fixture(&mut retry, protocol).unwrap();
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
