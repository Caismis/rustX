//! Wire correlation is independent of physical and projected occurrence identity.

use crate::model::types::ModelProtocol;
use serde_json::Value;

pub(crate) mod plan;
pub use plan::Plan;

use crate::model::selected_input::{InputNode, SelectedModelInput};
use std::sync::Arc;

/// The incremental correlation state a request cache carries. The journal and
/// the adapter share the identical allocator; only the carrier differs.
#[derive(Clone, Default)]
pub struct CorrelationSlot {
    /// Latest incremental plan, keyed by request fingerprint.
    pub plan: Option<Plan>,
    /// Monotonic epoch; sibling prefix caches invalidate when it advances.
    pub epoch: u64,
}

/// Shared-input prefix identity of a cached [`Plan`]. Deterministic and
/// contract-level: it only asks whether the new request extends the same
/// immutable input lineage.
#[derive(Clone, Default)]
pub struct Prefix {
    root: Option<Arc<InputNode>>,
}
impl Prefix {
    pub fn capture(input: &SelectedModelInput) -> Self {
        Self {
            root: input.0.root.clone(),
        }
    }
    pub fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |node| node.count)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn matches(&self, input: &SelectedModelInput) -> bool {
        input.shares_prefix(self.root.as_ref())
    }
}

/// Apply the validated request labels directly to typed wire correlation fields.
pub fn encode_fields<S: AsRef<str>>(
    labels: impl IntoIterator<Item = S>,
    mut visit: impl FnMut(&mut dyn FnMut(bool, &mut Value)),
) -> Result<(), &'static str> {
    let mut labels = labels.into_iter();
    let mut pending = std::collections::BTreeMap::new();
    let mut invalid = None;
    let mut missing_label = false;
    // This wire tree is detached. A failed validation discards it before I/O.
    visit(&mut |call, id| {
        let source = id.as_str().unwrap_or_default().to_owned();
        if source.is_empty() {
            invalid = Some("empty provider wire correlation ID");
        }
        if call {
            let label = labels.next().map_or_else(
                || {
                    missing_label = true;
                    source.clone()
                },
                |label| label.as_ref().to_owned(),
            );
            if pending.insert(source, label.clone()).is_some() {
                invalid = Some("ambiguous unresolved provider wire correlation ID");
            }
            *id = Value::String(label);
        } else if let Some(label) = pending.remove(&source) {
            *id = Value::String(label);
        }
    });
    if let Some(error) = invalid {
        return Err(error);
    }
    if missing_label || labels.next().is_some() {
        return Err("wire call count differs from request correlation plan");
    }
    Ok(())
}
/// Explicit labels for each transmitted canonical `ToolCall` coordinate. Stored
/// continuation reserves its boundary's IDs for the results sent without calls.
/// Labels for one request computed without a shared request cache.
///
/// The agent adapter reuses the incremental [`CorrelationSlot`] carried by the
/// selected input's preparation cell; journal readers recompute on a fresh
/// slot and get the identical deterministic assignment.
pub fn compute_labels(
    request: &crate::model::types::ModelRequest,
    boundary: Option<usize>,
) -> Result<std::collections::BTreeMap<(u32, u32), String>, String> {
    plan::prepare(&mut CorrelationSlot::default(), request, boundary).map(|plan| plan.coordinates())
}

pub fn compute_labels_at_boundary(
    request: &crate::model::types::ModelRequest,
) -> Result<std::collections::BTreeMap<(u32, u32), String>, String> {
    compute_labels(request, request.continuation_boundary()?)
}

/// Compute the correlation [`Plan`] for a request on a fresh slot; equivalent
/// to the adapter's incremental cache and deterministic for journal readers.
pub fn compute_plan(
    request: &crate::model::types::ModelRequest,
    boundary: Option<usize>,
) -> Result<Plan, String> {
    plan::prepare(&mut CorrelationSlot::default(), request, boundary)
}

/// Compute the [`Plan`] against the caller-owned incremental [`CorrelationSlot`];
/// the adapter's request cache carries the slot between requests so unchanged
/// prefixes are not re-planned. Deterministically identical to a fresh slot.
pub fn prepare_plan(
    slot: &mut CorrelationSlot,
    request: &crate::model::types::ModelRequest,
    boundary: Option<usize>,
) -> Result<Plan, String> {
    plan::prepare(slot, request, boundary)
}

pub fn is_stored(request: &crate::model::types::ModelRequest) -> bool {
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

#[cfg(test)]
fn encode_with_labels(
    value: &mut Value,
    protocol: ModelProtocol,
    labels: &[String],
) -> Result<(), &'static str> {
    encode_fields(labels, |field| visit(value, protocol, field))
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
                            visit_anthropic_block(block, &mut field);
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

pub fn visit_anthropic_block(block: &mut Value, mut field: impl FnMut(bool, &mut Value)) {
    match block["type"].as_str() {
        Some("tool_use") => field(true, &mut block["id"]),
        Some("tool_result") => field(false, &mut block["tool_use_id"]),
        _ => {}
    }
}
pub fn observed_correlations(value: &mut Value, protocol: ModelProtocol) -> Vec<(bool, String)> {
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
        encode_with_labels(value, protocol, &resolve_wire_labels(&raw, protocol))
    }
    #[test]
    fn anthropic_ids_stay_bounded_and_unique_even_when_aliases_are_occupied() {
        let invalid = "call/foreign|with:punctuation".to_owned();
        let hash = format!("{:x}", sha2::Sha256::digest(invalid.as_bytes()));
        let mut raw = vec!["repeat".into(), "repeat".into()];
        raw.extend((0..70).map(|i| format!("rustx_occurrence_0{}", "_".repeat(i))));
        raw.push(format!("rustx_{}_{}", &hash[..12], raw.len() + 1));
        raw.push(invalid);
        let labels = resolve_wire_labels(&raw, ModelProtocol::AnthropicMessages);
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
        assert_eq!(
            labels,
            resolve_wire_labels(&raw, ModelProtocol::AnthropicMessages)
        );
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

pub fn resolve_wire_labels(raw: &[String], protocol: ModelProtocol) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut labels = wire_labels(raw);
    let valid = |id: &str| valid_wire_id(id, protocol);
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

pub fn wire_labels(raw: &[String]) -> Vec<String> {
    let mut remaining = std::collections::BTreeMap::<&str, usize>::new();
    for id in raw {
        *remaining.entry(id).or_default() += 1;
    }
    let mut occupied = raw
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    raw.iter()
        .enumerate()
        .map(|(index, id)| {
            let count = remaining.get_mut(id.as_str()).expect("counted wire ID");
            *count -= 1;
            if *count == 0 {
                return id.clone();
            }
            let mut label = format!("rustx_occurrence_{index}");
            while occupied.contains(&label) {
                label.push('_');
            }
            occupied.insert(label.clone());
            label
        })
        .collect()
}

pub fn valid_wire_id(id: &str, protocol: ModelProtocol) -> bool {
    let limit = if protocol == ModelProtocol::OpenAiChatCompletions {
        40
    } else {
        64
    };
    !id.is_empty()
        && id.len() <= limit
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationWork {
    pub correlation_items: usize,
}
#[cfg(any(test, feature = "test-fixtures"))]
thread_local! { static WORK: std::cell::Cell<CorrelationWork> = const {std::cell::Cell::new(CorrelationWork {correlation_items:0})}; }
#[cfg(any(test, feature = "test-fixtures"))]
pub(crate) fn work(f: impl FnOnce(&mut CorrelationWork)) {
    WORK.with(|w| {
        let mut v = w.get();
        f(&mut v);
        w.set(v);
    });
}

#[cfg(any(test, feature = "test-fixtures"))]
pub fn take_correlation_work() -> CorrelationWork {
    WORK.with(|w| w.replace(CorrelationWork::default()))
}
