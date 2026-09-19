use super::*;
use crate::durable::response::CompletedResponseTiming;
use crate::model::generation_evidence::GenerationEvidence;

fn timing_case(
    requests: &[(Option<GenerationEvidence>, Option<ModelUsage>, bool)],
) -> CompletedResponseTiming {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    user(&store, "input");
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    for (index, (generation, usage, failed)) in requests.iter().enumerate() {
        request_timed(
            &store,
            "a",
            u32::try_from(index).unwrap(),
            usage.clone(),
            *generation,
            *failed,
        );
    }
    assistant(&store, "a", "closing");
    let mut completed = event(
        &store,
        "a",
        RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new("a"),
            finish_reason: ModelFinishReason::Stop,
        },
    );
    completed.timestamp += chrono::Duration::milliseconds(19_000);
    store.append_event(completed).unwrap();
    let view = page(&store, None, 64);
    assert_eq!(tails(&view).len(), 1);
    tails(&view)[0].timing.clone().unwrap()
}

#[test]
fn single_request_uses_native_offsets_not_scaled_wall_time() {
    let timing = timing_case(&[(Some(sample_generation()), Some(usage(None)), false)]);
    assert_eq!(
        timing,
        CompletedResponseTiming {
            total_duration_ms: Some(19_000),
            ttft_ms: Some(320),
            generation_ms: Some(1280),
            output_tokens_per_second: Some(15.625)
        }
    );
    // The 400ms durable-start bridge is not added to dispatch-relative TTFT.
    let no_bridge = GenerationEvidence {
        dispatch_after_start_ms: None,
        ..sample_generation()
    };
    assert_eq!(
        timing_case(&[(Some(no_bridge), Some(usage(None)), false)]),
        timing
    );
}

#[test]
fn failed_requests_and_retries_contribute_real_model_work() {
    let second = GenerationEvidence {
        first_output_ms: Some(100),
        terminal_ms: 820,
        last_output_ms: Some(700),
        ..sample_generation()
    };
    let timing = timing_case(&[
        (Some(sample_generation()), Some(usage(None)), true),
        (Some(second), Some(usage(None)), false),
    ]);
    assert_eq!(
        timing,
        CompletedResponseTiming {
            total_duration_ms: Some(19_000),
            ttft_ms: Some(320),
            generation_ms: Some(2000),
            output_tokens_per_second: Some(20.0)
        }
    );
}

#[test]
fn missing_zero_and_no_output_are_distinct() {
    let known = (Some(sample_generation()), Some(usage(None)), false);
    let absent = timing_case(&[(None, Some(usage(None)), true), known.clone()]);
    assert_eq!(absent.ttft_ms, None); // Never substitute the second request's TTFT.
    assert_eq!(absent.generation_ms, None);
    assert_eq!(absent.output_tokens_per_second, None);
    assert_eq!(absent.total_duration_ms, Some(19_000));
    let no_usage = timing_case(&[(Some(sample_generation()), None, false)]);
    assert_eq!(no_usage.generation_ms, Some(1280));
    assert_eq!(no_usage.output_tokens_per_second, None);
    let zero = GenerationEvidence {
        terminal_ms: 320,
        last_output_ms: Some(320),
        ..sample_generation()
    };
    let zero = timing_case(&[(Some(zero), Some(usage(None)), false)]);
    assert_eq!(zero.generation_ms, Some(0));
    assert_eq!(zero.output_tokens_per_second, None);
    let no_output = GenerationEvidence {
        first_output_ms: None,
        last_output_ms: None,
        ..sample_generation()
    };
    let mut empty_usage = usage(None);
    empty_usage.output_tokens = 0;
    let empty = timing_case(&[(Some(no_output), Some(empty_usage.clone()), true)]);
    assert_eq!(empty.ttft_ms, None);
    assert_eq!(empty.generation_ms, None);
    let mixed = timing_case(&[(Some(no_output), Some(empty_usage), true), known]);
    assert_eq!(mixed.ttft_ms, None);
    assert_eq!(mixed.generation_ms, Some(1280));
    assert_eq!(mixed.output_tokens_per_second, Some(15.625));
}

#[test]
fn absent_or_reversed_attempt_endpoints_do_not_invent_runtime() {
    let mut fold = super::super::timing::TimingFold::default();
    let id = RequestId::new("request");
    fold.start(id.clone());
    fold.terminal(&id, Some(sample_generation()), Some(&usage(None)));
    let end = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    assert_eq!(fold.summary(None, end).unwrap().total_duration_ms, None);
    assert_eq!(
        fold.summary(Some(end + chrono::Duration::seconds(1)), end)
            .unwrap()
            .total_duration_ms,
        None
    );
}
