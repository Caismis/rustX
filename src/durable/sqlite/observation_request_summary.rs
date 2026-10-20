//! Fixed request reading scalars captured from the original frozen input.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::model::RequestSnapshot;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::durable::observation::RequestReadSummary;

pub(super) fn serialized_bytes(value: &impl Serialize) -> Result<u64, ConversationStoreError> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| std::io::Error::other("request byte count overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| storage("request byte count unavailable"))?;
    Ok(counter.0)
}

fn text_digest(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

/// Mirrors `serde_json::Value`'s `PartialEq`, including equal signed float zero.
fn value_digest(hash: &mut Sha256, value: &serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Null => hash.update([0]),
        Value::Bool(value) => hash.update([1, u8::from(*value)]),
        Value::Number(value) if value.is_f64() => {
            hash.update([2]);
            let value = value.as_f64().expect("floating native number");
            hash.update(
                (if value == 0.0 { 0.0 } else { value })
                    .to_bits()
                    .to_le_bytes(),
            );
        }
        Value::Number(value) => {
            hash.update([3]);
            text_digest(hash, &value.to_string());
        }
        Value::String(value) => {
            hash.update([4]);
            text_digest(hash, value);
        }
        Value::Array(values) => {
            hash.update([5]);
            hash.update((values.len() as u64).to_le_bytes());
            for value in values {
                value_digest(hash, value);
            }
        }
        Value::Object(values) => {
            hash.update([6]);
            hash.update((values.len() as u64).to_le_bytes());
            // Native object equality ignores insertion order, including when
            // serde_json's preserve_order feature is enabled. Sum independently
            // framed entry hashes modulo 2^256; only a fixed accumulator is
            // retained, without sorting or copying original keys/values.
            let mut entries = [0_u8; 32];
            for (key, value) in values {
                let mut entry = Sha256::new();
                entry.update([7]);
                text_digest(&mut entry, key);
                value_digest(&mut entry, value);
                let mut carry = 0_u16;
                for (sum, byte) in entries.iter_mut().zip(entry.finalize()) {
                    let total = u16::from(*sum) + u16::from(byte) + carry;
                    *sum = total.to_le_bytes()[0];
                    carry = total >> 8;
                }
            }
            hash.update(entries);
        }
    }
}

pub(super) fn tools_digest(tools: &[crate::tools::types::ModelToolDefinition]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update((tools.len() as u64).to_le_bytes());
    for tool in tools {
        text_digest(&mut hash, tool.id.as_str());
        text_digest(&mut hash, &tool.name);
        text_digest(&mut hash, &tool.description);
        value_digest(&mut hash, &tool.input_schema);
    }
    hash.finalize().into()
}

pub(super) fn index(
    connection: &Connection,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let fits = crate::observation::trace::identity_fits;
    let summary = RequestReadSummary {
        request_id: fits(snapshot.request_id.as_str()).then(|| snapshot.request_id.clone()),
        attempt_id: fits(snapshot.identity.attempt_id.as_str())
            .then(|| snapshot.identity.attempt_id.clone()),
        step_id: fits(snapshot.identity.turn.as_str()).then(|| snapshot.identity.turn.clone()),
        assistant_message_id: fits(snapshot.provisional_message_id.as_str())
            .then(|| snapshot.provisional_message_id.clone()),
        retry_number: snapshot.identity.retry_number,
        model: fits(&snapshot.invocation.model).then(|| snapshot.invocation.model.clone()),
        model_preview: crate::observation::trace::TracePreview::of(&snapshot.invocation.model),
        context_window_tokens: snapshot.context_window_tokens,
        prompt_preview: crate::observation::trace::TracePreview::of(
            &snapshot.effective_system_prompt,
        ),
        prompt_digest: Sha256::digest(snapshot.effective_system_prompt.as_bytes()).into(),
        tools_digest: tools_digest(&snapshot.tool_definitions),
        prompt_json_bytes: if snapshot.effective_system_prompt.is_empty() {
            0
        } else {
            serialized_bytes(&snapshot.effective_system_prompt)?
        },
        tools_json_bytes: if snapshot.tool_definitions.is_empty() {
            0
        } else {
            serialized_bytes(&snapshot.tool_definitions)?
        },
    };
    let bytes = crate::observation::bounded::bounded_bytes(&summary, 16_384)
        .ok_or_else(|| storage("native request reading header exceeds bound"))?;
    connection
        .execute(
            "UPDATE request_snapshots SET observation_summary=?1 WHERE request_id=?2",
            params![bytes, snapshot.request_id.as_str()],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn read(
    connection: &Connection,
    request: &crate::runtime::identity::RequestId,
    through: u64,
) -> Result<RequestReadSummary, ConversationStoreError> {
    let bytes: Option<Option<Vec<u8>>> = connection.query_row(
        "SELECT observation_summary FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",
        params![request.as_str(),through], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    let bytes = bytes
        .flatten()
        .ok_or_else(|| storage("native request reading header unavailable"))?;
    serde_json::from_slice(&bytes).map_err(|_| storage("native request reading header is corrupt"))
}

pub(super) fn occupancy(
    connection: &Connection,
    request: &crate::runtime::identity::RequestId,
    usage: &crate::model::types::ModelUsage,
    through: u64,
) -> Result<
    Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
    ConversationStoreError,
> {
    let summary = read(connection, request, through)?;
    if summary.context_window_tokens == 0 {
        return Ok(None);
    }
    let sequence: u64 = connection.query_row("SELECT started_sequence FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let system_tokens = crate::context::tokens::bytes_to_tokens(summary.prompt_json_bytes);
    let tool_tokens = crate::context::tokens::bytes_to_tokens(summary.tools_json_bytes);
    let deferred = summary.model.is_none();
    Ok(Some(
        crate::observation::occupancy::RuntimeClientContextOccupancy {
            estimated: false,
            input_tokens: usage.input_tokens,
            context_window_tokens: summary.context_window_tokens,
            model: summary.model,
            model_deferred: deferred,
            model_source: deferred
                .then(|| format!("trace.records.{sequence}.detail.request.model")),
            breakdown: crate::context::occupancy::ContextBreakdown {
                system_tokens,
                tool_tokens,
                message_tokens: usage
                    .input_tokens
                    .saturating_sub(system_tokens.saturating_add(tool_tokens)),
            },
        },
    ))
}
