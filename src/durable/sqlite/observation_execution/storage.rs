//! Fixed-position private storage avoids re-encoding descriptive JSON keys on
//! every native event. All coordinates, optional values and timing flags remain
//! explicit; an absent or extra tuple element is corrupt evidence.
use super::{AttemptExecutionSummary, ConversationStoreError, ExecutionLayout, storage};
use crate::durable::sqlite::observation_codec as codec;
use crate::runtime::identity::{EventId, MessageId, RequestId};

type Failure = (
    Option<crate::runtime_client::event::RuntimeClientAttemptFailure>,
    bool,
    u64,
);
type Terminal = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<EventId>,
    Option<u64>,
    Option<String>,
    Option<crate::model::ModelFinishReason>,
    bool,
    Option<u64>,
);
type Request = (
    Option<MessageId>,
    Option<u64>,
    Option<u64>,
    Option<RequestId>,
    Option<u64>,
    Option<crate::runtime_client::snapshot::JournalFieldRef>,
    u64,
    u64,
    Option<crate::model::ModelUsage>,
    Option<crate::durable::response::CompletedResponseTiming>,
    Vec<String>,
    u64,
    Option<u64>,
);
type Timing = (Option<u64>, Option<u64>, u64, bool, bool, bool, u64, u64);
type Stored = (u64, Failure, Terminal, Request, Timing);

fn fields<'a, T: serde::Serialize + 'a>(
    state: &'a ExecutionLayout,
    timing: T,
) -> impl serde::Serialize + 'a {
    let s = &state.summary;
    (
        state.owner_anchor,
        (&s.failure, s.failure_deferred, s.failure_readable_end),
        (
            &s.started_at,
            &s.ended_at,
            &s.terminal_event_id,
            s.terminal_sequence,
            &s.terminal_kind,
            &s.finish_reason,
            s.finish_reason_deferred,
            s.finish_reason_sequence,
        ),
        (
            &s.closing_message_id,
            s.closing_ledger_position,
            s.closing_identity_sequence,
            &s.last_request_id,
            s.last_request_sequence,
            &s.last_request_id_source,
            s.requests,
            s.reports,
            &s.usage,
            timing,
            &s.models,
            s.models_total,
            s.models_next_offset,
        ),
        (
            state.first_request_sequence,
            state.first_ttft,
            state.terminals,
            state.duration_invalid,
            state.output_invalid,
            state.observed_output,
            state.generation_ms,
            state.output_tokens,
        ),
    )
}

#[derive(serde::Serialize)]
struct JsonTiming {
    #[serde(skip_serializing_if = "Option::is_none")]
    total_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttft_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_ms: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "codec::json_optional_float"
    )]
    output_tokens_per_second: Option<f64>,
}
impl From<&crate::durable::response::CompletedResponseTiming> for JsonTiming {
    fn from(value: &crate::durable::response::CompletedResponseTiming) -> Self {
        Self {
            total_duration_ms: value.total_duration_ms,
            ttft_ms: value.ttft_ms,
            generation_ms: value.generation_ms,
            output_tokens_per_second: value.output_tokens_per_second,
        }
    }
}

pub(super) fn encode(state: &ExecutionLayout) -> Option<Vec<u8>> {
    // The public-shaped preflight and original tuple admission are distinct.
    // Neither may use the smaller binary form or normalized nonfinite timings.
    if !crate::runtime_client::preflight::fits(
        state,
        crate::durable::observation::HEADER_ENCODED_MAX,
    ) {
        return None;
    }
    codec::encode(
        &fields(state, &state.summary.timing),
        &fields(state, state.summary.timing.as_ref().map(JsonTiming::from)),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok()
}
pub(super) fn decode(bytes: &[u8]) -> Result<ExecutionLayout, ConversationStoreError> {
    let (owner_anchor, failure, terminal, request, timing): Stored =
        codec::decode_json_tuple(bytes, crate::durable::observation::HEADER_ENCODED_MAX)
            .map_err(|_| storage("native execution header corrupt"))?;
    let (failure, failure_deferred, failure_readable_end) = failure;
    let (
        started_at,
        ended_at,
        terminal_event_id,
        terminal_sequence,
        terminal_kind,
        finish_reason,
        finish_reason_deferred,
        finish_reason_sequence,
    ) = terminal;
    let (
        closing_message_id,
        closing_ledger_position,
        closing_identity_sequence,
        last_request_id,
        last_request_sequence,
        last_request_id_source,
        requests,
        reports,
        usage,
        response_timing,
        models,
        models_total,
        models_next_offset,
    ) = request;
    let (
        first_request_sequence,
        first_ttft,
        terminals,
        duration_invalid,
        output_invalid,
        observed_output,
        generation_ms,
        output_tokens,
    ) = timing;
    Ok(ExecutionLayout {
        owner_anchor,
        summary: AttemptExecutionSummary {
            failure,
            failure_deferred,
            failure_readable_end,
            started_at,
            ended_at,
            terminal_event_id,
            terminal_sequence,
            terminal_kind,
            finish_reason,
            finish_reason_deferred,
            finish_reason_sequence,
            closing_message_id,
            closing_ledger_position,
            closing_identity_sequence,
            last_request_id,
            last_request_sequence,
            last_request_id_source,
            requests,
            reports,
            usage,
            timing: response_timing,
            models,
            models_total,
            models_next_offset,
        },
        first_request_sequence,
        first_ttft,
        terminals,
        duration_invalid,
        output_invalid,
        observed_output,
        generation_ms,
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original_encode(state: &ExecutionLayout) -> Option<Vec<u8>> {
        crate::runtime_client::preflight::fits(state, 16_384)
            .then(|| {
                crate::runtime_client::checkpoint::bounded_bytes(
                    &fields(state, &state.summary.timing),
                    16_384,
                )
            })
            .flatten()
    }

    fn populated() -> ExecutionLayout {
        let timestamp = chrono::DateTime::from_timestamp(1_700_000_000, 123).unwrap();
        ExecutionLayout {
            owner_anchor: 1,
            summary: AttemptExecutionSummary {
                failure: Some(
                    crate::runtime_client::event::RuntimeClientAttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "diagnostic 水".into(),
                        },
                    },
                ),
                failure_deferred: true,
                failure_readable_end: 2,
                started_at: Some(timestamp),
                ended_at: Some(timestamp + chrono::Duration::seconds(3)),
                terminal_event_id: Some(EventId::new("terminal")),
                terminal_sequence: Some(4),
                terminal_kind: Some("attempt_failed".into()),
                finish_reason: Some(crate::model::ModelFinishReason::Stop),
                finish_reason_deferred: true,
                finish_reason_sequence: Some(5),
                closing_message_id: Some(MessageId::new("closing")),
                closing_ledger_position: Some(6),
                closing_identity_sequence: Some(7),
                last_request_id: Some(RequestId::new("request")),
                last_request_sequence: Some(8),
                last_request_id_source: Some(crate::runtime_client::snapshot::JournalFieldRef {
                    sequence: 9,
                    path: vec!["event".into(), "request_id".into()],
                    encoded_bytes: 10,
                }),
                requests: 11,
                reports: 12,
                usage: Some(crate::model::ModelUsage {
                    input_tokens: u64::MAX,
                    output_tokens: 13,
                    total_tokens: u64::MAX,
                    details: None,
                }),
                timing: Some(crate::durable::response::CompletedResponseTiming {
                    total_duration_ms: Some(14),
                    ttft_ms: Some(15),
                    generation_ms: Some(16),
                    output_tokens_per_second: Some(17.5),
                }),
                models: vec!["model 水".into()],
                models_total: 18,
                models_next_offset: Some(19),
            },
            first_request_sequence: Some(20),
            first_ttft: Some(21),
            terminals: 22,
            duration_invalid: true,
            output_invalid: true,
            observed_output: true,
            generation_ms: 23,
            output_tokens: 24,
        }
    }

    #[test]
    fn compact_execution_preserves_every_field_and_rejects_missing_positions() {
        for state in [ExecutionLayout::default(), populated()] {
            let original = serde_json::to_value(&state).unwrap();
            let bytes = encode(&state).unwrap();
            assert!(bytes.len() * 2 < serde_json::to_vec(&state).unwrap().len());
            assert_eq!(
                serde_json::to_value(decode(&bytes).unwrap()).unwrap(),
                original
            );
            assert!(decode(&serde_json::to_vec(&state).unwrap()).is_err());
            let value: serde_json::Value = codec::decode(&bytes).unwrap();
            for group in 1..5 {
                let mut missing = value.clone();
                missing[group].as_array_mut().unwrap().pop();
                assert!(decode(&codec::encode_admitted(&missing).unwrap()).is_err());
            }
            assert!(
                decode(&serde_json::to_vec(&fields(&state, &state.summary.timing)).unwrap())
                    .is_err()
            );
        }
    }

    #[test]
    fn compact_execution_keeps_original_bounded_admission() {
        for bytes in [0, 128, 8192, 14000, 15000, 16000, 16384, 20000] {
            let mut state = populated();
            state.summary.models = vec!["水\n".repeat(bytes / 4)];
            let original = crate::runtime_client::checkpoint::bounded_bytes(
                &state,
                crate::durable::observation::HEADER_ENCODED_MAX,
            );
            let compact = encode(&state);
            assert_eq!(compact.is_some(), original.is_some(), "model bytes {bytes}");
            if let Some(compact) = compact {
                assert_eq!(
                    serde_json::to_value(decode(&compact).unwrap()).unwrap(),
                    serde_json::to_value(&state).unwrap()
                );
            }
        }
    }

    #[test]
    fn binary_execution_preserves_both_admission_stages_and_json_timing() {
        for rate in [
            0.0,
            -0.0,
            1.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let mut state = populated();
            state
                .summary
                .timing
                .as_mut()
                .unwrap()
                .output_tokens_per_second = Some(rate);
            let json = original_encode(&state).unwrap();
            let bytes = encode(&state).unwrap();
            let expected: Stored = serde_json::from_slice(&json).unwrap();
            let actual: Stored = codec::decode_json_tuple(&bytes, 16_384).unwrap();
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            let read_rate = decode(&bytes)
                .unwrap()
                .summary
                .timing
                .unwrap()
                .output_tokens_per_second;
            if rate.is_finite() {
                assert_eq!(read_rate.unwrap().to_bits(), rate.to_bits());
            } else {
                assert_eq!(read_rate, None);
            }
        }
        let mut state = populated();
        let mut lower = 0;
        let mut upper = 20_000;
        while lower + 1 < upper {
            let size = usize::midpoint(lower, upper);
            state.summary.models = vec!["x".repeat(size)];
            if original_encode(&state).is_some() {
                lower = size;
            } else {
                upper = size;
            }
        }
        for size in [lower, upper] {
            state.summary.models = vec!["x".repeat(size)];
            assert_eq!(encode(&state).is_some(), original_encode(&state).is_some());
        }
    }

    #[test]
    fn binary_execution_read_preserves_tuple_budget_without_writer_preflight() {
        let mut state = populated();
        state.summary.models = vec![String::new()];
        let base = serde_json::to_vec(&fields(&state, &state.summary.timing))
            .unwrap()
            .len();
        for length in [16_383, 16_384, 16_385] {
            state.summary.models = vec!["x".repeat(length - base)];
            let tuple = fields(&state, &state.summary.timing);
            assert_eq!(serde_json::to_vec(&tuple).unwrap().len(), length);
            let bytes = codec::encode_admitted(&tuple).unwrap();
            assert_eq!(decode(&bytes).is_ok(), length <= 16_384);
            assert!(
                encode(&state).is_none(),
                "reader must not reapply stricter writer admission"
            );
        }
        let state = populated();
        let mut value = serde_json::to_value(fields(&state, &state.summary.timing)).unwrap();
        value[3][8]["ignored"] = serde_json::json!(vec![1.0_f64; 3700]);
        let json = serde_json::to_vec(&value).unwrap();
        assert!(json.len() < 16_384);
        let bytes = codec::encode_admitted(&value).unwrap();
        assert!(bytes.len() > 16_384 && bytes.len() <= codec::MAX_BYTES);
        assert_eq!(
            serde_json::to_value(decode(&bytes).unwrap()).unwrap(),
            serde_json::to_value(&state).unwrap()
        );
        let store = crate::durable::SqliteConversationStore::in_memory(
            crate::runtime::identity::ConversationId::generate(),
        )
        .unwrap();
        let connection = store.lock().unwrap();
        connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,observation_execution) VALUES(1,'wide-execution',1,'test','time','{}',?1)", [&bytes]).unwrap();
        let retained: Vec<u8> = connection
            .query_row(
                "SELECT observation_execution FROM events WHERE sequence=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(decode(&retained).unwrap()).unwrap(),
            serde_json::to_value(&state).unwrap()
        );
        value[3][8]["ignored"] = serde_json::json!("\u{0000}".repeat(4000));
        let bytes = codec::encode_admitted(&value).unwrap();
        assert!(
            bytes.len() < 16_384,
            "small physical cell can exceed its JSON byte allowance"
        );
        assert!(decode(&bytes).is_err());
    }
}
