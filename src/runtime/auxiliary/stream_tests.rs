use super::*;
use crate::message::types::ContentBlockIndex;
use crate::model::{
    GenerationBudgetKind, GenerationChannel, GenerationFailure, ModelStreamProgress,
    ModelTimeoutPhase,
};
use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use crate::runtime::identity::{ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolCallStart};

fn fold() -> AuxiliaryStreamFold {
    AuxiliaryStreamFold::new(
        RequestId::new("one-physical-request"),
        100,
        90,
        1000,
        ModelTimeoutPolicy::new(
            std::time::Duration::from_millis(30),
            std::time::Duration::from_millis(15),
        ),
        GenerationSafetyPolicy {
            max_generated_bytes: 4096,
            max_reasoning_bytes: Some(1024),
        },
    )
}

fn event(event: ModelEvent) -> ModelStreamItem {
    ModelStreamItem::Event(event)
}

fn text(block: u32, value: &str) -> ModelStreamItem {
    event(ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(block),
        text: value.into(),
    })
}

fn usage(tokens: u64) -> ModelUsage {
    ModelUsage {
        input_tokens: 5,
        output_tokens: tokens,
        total_tokens: 5 + tokens,
        details: None,
    }
}

fn completed(usage: Option<ModelUsage>) -> ModelStreamItem {
    event(ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage,
    })
}

#[test]
fn completion_and_eof_have_separate_frontiers() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "bonjour"), 105).unwrap();
    fold.accept(&completed(Some(usage(7))), 110).unwrap();
    assert_eq!(
        fold.deadline_millis(),
        1000,
        "Completed cannot disable the EOF deadline"
    );
    let sealed = fold.finish(115);
    let output = sealed.output.unwrap();
    assert_eq!(output.text, "bonjour");
    assert!(!output.refused);
    assert_eq!(output.finish_reason, ModelFinishReason::Stop);
    assert_eq!(sealed.generation.dispatch_after_start_ms, Some(10));
    assert_eq!(sealed.generation.first_output_ms, Some(5));
    assert_eq!(sealed.generation.last_output_ms, Some(5));
    assert_eq!(
        sealed.generation.terminal_ms, 10,
        "EOF does not replace provider timing"
    );
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Complete
    );
    assert!(!sealed.evidence.remote_execution_uncertainty);
}

#[test]
fn usage_updates_replace_and_missing_final_usage_retains_the_latest() {
    for final_usage in [None, Some(usage(11))] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(3) }), 101)
            .unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 102)
            .unwrap();
        fold.accept(&text(0, "result"), 103).unwrap();
        fold.accept(&completed(final_usage.clone()), 104).unwrap();
        let sealed = fold.finish(105);
        assert!(sealed.output.is_ok());
        assert_eq!(
            sealed.evidence.usage,
            final_usage.clone().or_else(|| Some(usage(7)))
        );
        assert_eq!(
            sealed.evidence.usage_completeness,
            if final_usage.is_some() {
                AuxiliaryUsageCompleteness::Complete
            } else {
                AuxiliaryUsageCompleteness::Partial
            }
        );
    }
}

#[test]
fn prompt_failure_keeps_partial_usage_without_waiting_for_eof() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
        .unwrap();
    let error = local_error(ModelErrorKind::ProviderError, "provider rejected request");
    let failure = fold
        .accept(
            &event(ModelEvent::Failed {
                error: error.clone(),
            }),
            102,
        )
        .unwrap_err();
    assert_eq!(failure, error);
    let sealed = fold.fail(failure, 103);
    assert_eq!(sealed.output.unwrap_err(), error);
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Partial
    );
    assert_eq!(
        sealed.evidence.observed_provider_terminal,
        Some(AuxiliaryObservedProviderTerminal::Failed {
            kind: ModelErrorKind::ProviderError,
        })
    );
    assert_eq!(sealed.generation.terminal_ms, 2);
    assert!(sealed.evidence.remote_execution_uncertainty);
}

#[test]
fn a_bare_failure_is_a_valid_terminal_without_started() {
    let mut fold = fold();
    let error = local_error(ModelErrorKind::Authentication, "authentication failed");
    assert_eq!(
        fold.accept(
            &event(ModelEvent::Failed {
                error: error.clone()
            }),
            101
        ),
        Err(error)
    );
    let sealed = fold.finish(102);
    assert!(sealed.output.is_err());
    assert_eq!(sealed.generation.first_output_ms, None);
    assert_eq!(sealed.generation.terminal_ms, 1);
    assert_eq!(sealed.evidence.usage, None);
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Unreported
    );
    assert!(!sealed.evidence.remote_execution_uncertainty);
}

#[test]
fn illegal_tail_cannot_replace_trusted_usage_or_provider_terminal() {
    for tail in [
        event(ModelEvent::UsageUpdate { usage: usage(999) }),
        event(ModelEvent::Failed {
            error: local_error(ModelErrorKind::ProviderError, "late failure"),
        }),
        ModelStreamItem::Progress(ModelStreamProgress::Liveness),
        text(0, "late output"),
        completed(Some(usage(999))),
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&text(0, "result"), 101).unwrap();
        fold.accept(&completed(Some(usage(7))), 102).unwrap();
        assert_eq!(
            fold.accept(&tail, 103).unwrap_err().kind,
            ModelErrorKind::InvalidRequest
        );
        let sealed = fold.finish(104);
        assert_eq!(
            sealed.output.unwrap_err().kind,
            ModelErrorKind::InvalidRequest
        );
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert_eq!(
            sealed.evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Complete
        );
        assert_eq!(
            sealed.evidence.observed_provider_terminal,
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::AuxiliaryFinishReason::Stop,
            })
        );
        assert_eq!(sealed.generation.terminal_ms, 2);
        assert_eq!(sealed.generation.last_output_ms, Some(1));
    }
}

#[test]
fn eof_at_the_absolute_deadline_fails_after_genuine_completion() {
    for eof in [1000, 1001] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&text(0, "result"), 101).unwrap();
        fold.accept(&completed(Some(usage(7))), 102).unwrap();
        let sealed = fold.finish(eof);
        let error = sealed.output.unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Timeout);
        assert_eq!(error.timeout_phase, None);
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert!(sealed.evidence.observed_provider_terminal.is_some());
        assert_eq!(sealed.generation.terminal_ms, 2);
    }
}

#[test]
fn a_late_item_cannot_refresh_an_expired_liveness_deadline() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(
        &ModelStreamItem::Progress(ModelStreamProgress::Liveness),
        129,
    )
    .unwrap();
    assert_eq!(
        fold.deadline_millis(),
        130,
        "liveness cannot start generation"
    );
    let error = fold.accept(&text(0, "late response"), 130).unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::Timeout);
    assert_eq!(error.timeout_phase, Some(ModelTimeoutPhase::ResponseStart));
    assert!(fold.finish(131).output.is_err());
}

#[test]
fn transport_progress_refreshes_idle_but_never_the_absolute_deadline() {
    let mut fold = fold();
    fold.absolute_deadline_millis = 125;
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "result"), 105).unwrap();
    assert_eq!(fold.deadline_millis(), 120);
    fold.accept(
        &ModelStreamItem::Progress(ModelStreamProgress::Liveness),
        119,
    )
    .unwrap();
    assert_eq!(fold.deadline_millis(), 125);
    assert_eq!(fold.expiration(125).unwrap().kind, ModelErrorKind::Timeout);
}

#[test]
fn native_cancellation_keeps_the_observed_completion_separate() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "result"), 101).unwrap();
    fold.accept(&completed(Some(usage(7))), 102).unwrap();
    let sealed = fold.fail(
        local_error(ModelErrorKind::Cancelled, "native cancellation won"),
        105,
    );
    assert_eq!(sealed.output.unwrap_err().kind, ModelErrorKind::Cancelled);
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert!(sealed.evidence.observed_provider_terminal.is_some());
    assert!(!sealed.evidence.remote_execution_uncertainty);
    assert_eq!(sealed.generation.terminal_ms, 2);
}

#[test]
fn eof_without_a_terminal_never_becomes_success() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "unfinished"), 101).unwrap();
    let sealed = fold.finish(102);
    assert_eq!(
        sealed.output.unwrap_err().kind,
        ModelErrorKind::InvalidRequest
    );
    assert!(sealed.evidence.remote_execution_uncertainty);
    assert!(sealed.evidence.observed_provider_terminal.is_none());
}

#[test]
fn continuation_is_validated_then_discarded_without_losing_ordered_text() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "a"), 101).unwrap();
    fold.accept(
        &event(ModelEvent::ContinuationState {
            block_index: ContentBlockIndex::new(1),
            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"signature": "private-continuation"}),
            }),
        }),
        102,
    )
    .unwrap();
    fold.accept(&text(2, "c"), 103).unwrap();
    fold.accept(&text(0, "b"), 104).unwrap();
    fold.accept(&completed(None), 105).unwrap();
    let output = fold.finish(106).output.unwrap();
    assert_eq!(output.text, "abc", "text follows canonical block order");
    assert!(!output.refused);
}

#[test]
fn malformed_envelopes_are_absorbing_and_cannot_add_evidence() {
    for invalid in [
        text(0, "before started"),
        completed(Some(usage(99))),
        event(ModelEvent::UsageUpdate { usage: usage(99) }),
        ModelStreamItem::Progress(ModelStreamProgress::Generation),
    ] {
        let mut fold = fold();
        assert!(fold.accept(&invalid, 101).is_err());
        assert!(fold.accept(&event(ModelEvent::Started), 102).is_err());
        let sealed = fold.finish(103);
        assert!(sealed.output.is_err());
        assert!(sealed.evidence.usage.is_none());
        assert!(sealed.evidence.observed_provider_terminal.is_none());
    }
    for invalid in [
        event(ModelEvent::Started),
        text(2, "skipped block"),
        event(ModelEvent::ContinuationState {
            block_index: ContentBlockIndex::new(0),
            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({}),
            }),
        }),
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&text(0, "text block"), 101).unwrap();
        assert!(fold.accept(&invalid, 102).is_err());
        assert!(fold.finish(103).output.is_err());
    }
}

#[test]
fn every_tool_proposal_form_is_refused_without_execution_authority() {
    for tool in [
        event(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: ToolCallId::new("call"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
            },
        }),
        event(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call"),
            arguments_delta: "{}".into(),
        }),
        event(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCall {
                id: ToolCallId::new("call"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            },
        }),
        event(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        let error = fold.accept(&tool, 101).unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Unsupported);
        assert!(fold.finish(102).output.is_err());
    }
}

#[test]
fn provider_length_is_local_failure_with_genuine_terminal_evidence() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(&text(0, "truncated"), 101).unwrap();
    let error = fold
        .accept(
            &event(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Length,
                usage: Some(usage(7)),
            }),
            102,
        )
        .unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::GenerationBudgetExceeded);
    let sealed = fold.finish(103);
    assert!(sealed.output.is_err());
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.observed_provider_terminal,
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::AuxiliaryFinishReason::Length,
        })
    );
}

#[test]
fn generation_byte_limit_is_enforced_before_accepting_large_output() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    let error = fold.accept(&text(0, &"a".repeat(4097)), 101).unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::GenerationBudgetExceeded);
    let sealed = fold.finish(102);
    assert!(sealed.output.is_err());
    assert!(sealed.evidence.remote_execution_uncertainty);
    assert!(sealed.evidence.observed_provider_terminal.is_none());
}

#[test]
fn empty_deltas_cannot_allocate_an_unbounded_number_of_blocks() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    for block in 0..4096 {
        fold.accept(&text(block, ""), 101).unwrap();
    }
    assert_eq!(
        fold.accept(&text(4096, ""), 102).unwrap_err().kind,
        ModelErrorKind::InvalidRequest
    );
    assert!(fold.finish(103).output.is_err());
}

#[test]
fn local_adapter_failure_is_not_remote_termination_proof() {
    for kind in [
        ModelErrorKind::Cancelled,
        ModelErrorKind::Timeout,
        ModelErrorKind::Transport,
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
            .unwrap();
        assert!(
            fold.accept(
                &event(ModelEvent::Failed {
                    error: local_error(kind, "local stream failure")
                }),
                102
            )
            .is_err()
        );
        let sealed = fold.finish(103);
        assert!(sealed.evidence.observed_provider_terminal.is_none());
        assert!(sealed.evidence.remote_execution_uncertainty);
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert_eq!(
            sealed.evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
    }
}

#[test]
fn empty_and_refused_output_remain_context_acceptance_decisions() {
    let mut empty = fold();
    empty.accept(&event(ModelEvent::Started), 100).unwrap();
    empty.accept(&completed(None), 101).unwrap();
    assert!(empty.finish(102).output.unwrap().text.is_empty());
    let mut refused = fold();
    refused.accept(&event(ModelEvent::Started), 100).unwrap();
    refused.accept(&text(0, "provisional"), 101).unwrap();
    refused
        .accept(
            &event(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(1),
                text: "cannot comply".into(),
            }),
            102,
        )
        .unwrap();
    refused
        .accept(
            &event(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Refusal,
                usage: None,
            }),
            103,
        )
        .unwrap();
    let output = refused.finish(104).output.unwrap();
    assert!(output.refused);
    assert!(
        output.text.is_empty(),
        "a refusal rolls back provisional text"
    );
}

fn manual_clock() -> crate::runtime::monotonic::ManualMonotonicClock {
    let clock = crate::runtime::monotonic::ManualMonotonicClock::new();
    clock.advance(100);
    clock
}

#[tokio::test(flavor = "current_thread")]
async fn completed_without_eof_remains_bounded_by_the_absolute_deadline() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let stream = Box::pin(
        futures_util::stream::iter([
            event(ModelEvent::Started),
            text(0, "result"),
            completed(Some(usage(7))),
        ])
        .chain(futures_util::stream::pending()),
    );
    let mut pending = Box::pin(poll_stream(stream, fold(), &cancellation, &clock));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    clock.advance(900);
    let AuxiliaryStreamExit::Failed(sealed) = pending.await else {
        panic!("Completed without EOF cannot return a success candidate")
    };
    assert_eq!(sealed.output.unwrap_err().kind, ModelErrorKind::Timeout);
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert!(sealed.evidence.observed_provider_terminal.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn failed_terminal_returns_without_polling_a_stalled_tail() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let stream = Box::pin(
        futures_util::stream::iter([
            event(ModelEvent::Started),
            event(ModelEvent::UsageUpdate { usage: usage(7) }),
            event(ModelEvent::Failed {
                error: local_error(ModelErrorKind::ProviderError, "provider failed"),
            }),
        ])
        .chain(futures_util::stream::pending()),
    );
    let mut pending = Box::pin(poll_stream(stream, fold(), &cancellation, &clock));
    let std::task::Poll::Ready(AuxiliaryStreamExit::Failed(sealed)) =
        futures_util::poll!(pending.as_mut())
    else {
        panic!("a genuine failure must not wait for EOF")
    };
    assert_eq!(
        sealed.output.unwrap_err().kind,
        ModelErrorKind::ProviderError
    );
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Partial
    );
}

struct TrackedStream {
    inner: ModelStream,
    dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl futures_util::Stream for TrackedStream {
    type Item = ModelStreamItem;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(context)
    }
}

impl Drop for TrackedStream {
    fn drop(&mut self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_drops_the_owned_provider_stream_before_returning() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream = Box::pin(TrackedStream {
        inner: Box::pin(
            futures_util::stream::iter([event(ModelEvent::Started)])
                .chain(futures_util::stream::pending()),
        ),
        dropped: dropped.clone(),
    });
    let mut pending = Box::pin(poll_stream(stream, fold(), &cancellation, &clock));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    cancellation.cancel();
    let AuxiliaryStreamExit::Failed(sealed) = pending.await else {
        panic!("cancellation cannot produce EOF success")
    };
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(sealed.output.unwrap_err().kind, ModelErrorKind::Cancelled);
    assert!(sealed.evidence.remote_execution_uncertainty);
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_response_uses_the_existing_transport_timeout_phase() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let stream = Box::pin(
        futures_util::stream::iter([event(ModelEvent::Started)])
            .chain(futures_util::stream::pending()),
    );
    let mut pending = Box::pin(poll_stream(stream, fold(), &cancellation, &clock));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    clock.advance(30);
    let AuxiliaryStreamExit::Failed(sealed) = pending.await else {
        panic!("a stalled response must time out")
    };
    let error = sealed.output.unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::Timeout);
    assert_eq!(error.timeout_phase, Some(ModelTimeoutPhase::ResponseStart));
    assert!(sealed.evidence.observed_provider_terminal.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn eof_returns_a_candidate_for_the_native_authorization_boundary() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let stream = Box::pin(futures_util::stream::iter([
        event(ModelEvent::Started),
        text(0, "result"),
        completed(None),
    ]));
    let AuxiliaryStreamExit::Eof(candidate) =
        poll_stream(stream, fold(), &cancellation, &clock).await
    else {
        panic!("valid terminal plus EOF produces an authorization candidate")
    };
    // Simulate the native cancellation winner at the grant after stream exit.
    // EOF by itself did not seal a successful outcome.
    let sealed = (*candidate).fail(
        local_error(
            ModelErrorKind::Cancelled,
            "parent closed before success grant",
        ),
        clock.now_millis(),
    );
    assert_eq!(sealed.output.unwrap_err().kind, ModelErrorKind::Cancelled);
    assert!(sealed.evidence.observed_provider_terminal.is_some());
}

#[test]
fn late_terminals_preserve_evidence_without_reviving_expired_requests() {
    for (had_output, absolute, now, phase) in [
        (false, 1000, 130, Some(ModelTimeoutPhase::ResponseStart)),
        (true, 1000, 116, Some(ModelTimeoutPhase::StreamIdle)),
        (true, 110, 110, None),
    ] {
        for terminal in [
            completed(Some(usage(13))),
            event(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Length,
                usage: Some(usage(13)),
            }),
            event(ModelEvent::Failed {
                error: local_error(ModelErrorKind::ProviderError, "late provider failure"),
            }),
        ] {
            let mut fold = fold();
            fold.absolute_deadline_millis = absolute;
            fold.accept(&event(ModelEvent::Started), 100).unwrap();
            if had_output {
                fold.accept(&text(0, "partial"), 101).unwrap();
            }
            fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
                .unwrap();
            let error = fold.accept(&terminal, now).unwrap_err();
            assert_eq!(error.kind, ModelErrorKind::Timeout);
            assert_eq!(error.timeout_phase, phase);
            assert_eq!(error.generation, None, "expiry precedes the length guard");
            let sealed = fold.finish(1001);
            assert_eq!(sealed.output, Err(error), "sealing cannot relabel expiry");
            assert_eq!(sealed.generation.terminal_ms, now - 100);
            assert_eq!(sealed.generation.first_output_ms, had_output.then_some(1));
            assert_eq!(sealed.generation.last_output_ms, had_output.then_some(1));
            let ModelStreamItem::Event(terminal) = terminal else {
                unreachable!()
            };
            let (provider, expected_usage, completeness, uncertain) = match terminal {
                ModelEvent::Completed { finish_reason, .. } => (
                    AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: (&finish_reason).into(),
                    },
                    usage(13),
                    AuxiliaryUsageCompleteness::Complete,
                    false,
                ),
                ModelEvent::Failed { error } => (
                    AuxiliaryObservedProviderTerminal::Failed { kind: error.kind },
                    usage(7),
                    AuxiliaryUsageCompleteness::Partial,
                    true,
                ),
                _ => unreachable!(),
            };
            assert_eq!(sealed.evidence.observed_provider_terminal, Some(provider));
            assert_eq!(sealed.evidence.usage, Some(expected_usage));
            assert_eq!(sealed.evidence.usage_completeness, completeness);
            assert_eq!(sealed.evidence.remote_execution_uncertainty, uncertain);
        }
    }
}

#[test]
fn definite_rejection_only_closes_remote_uncertainty_before_started() {
    for kind in [
        ModelErrorKind::InvalidRequest,
        ModelErrorKind::Authentication,
        ModelErrorKind::RateLimit,
        ModelErrorKind::ContextWindowExceeded,
        ModelErrorKind::Unsupported,
        ModelErrorKind::ProviderError,
        ModelErrorKind::MalformedToolProposal,
        ModelErrorKind::GenerationDegenerated,
        ModelErrorKind::GenerationBudgetExceeded,
        ModelErrorKind::Cancelled,
        ModelErrorKind::Timeout,
        ModelErrorKind::Transport,
    ] {
        for started in [false, true] {
            let mut fold = fold();
            if started {
                fold.accept(&event(ModelEvent::Started), 100).unwrap();
                fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
                    .unwrap();
            }
            let mut error = local_error(kind.clone(), "normalized provider failure");
            error.provider_code = Some("provider_code".into());
            error.retry_after_ms = Some(321);
            error.retry_disposition = ModelRetryDisposition::Transient;
            assert_eq!(
                fold.accept(
                    &event(ModelEvent::Failed {
                        error: error.clone()
                    }),
                    102
                ),
                Err(error.clone())
            );
            let sealed = fold.finish(1001);
            assert_eq!(sealed.output, Err(error));
            assert_eq!(sealed.generation.terminal_ms, 2);
            assert_eq!(sealed.generation.first_output_ms, None);
            assert_eq!(sealed.generation.last_output_ms, None);
            assert_eq!(
                sealed.evidence.request_id,
                RequestId::new("one-physical-request")
            );
            assert_eq!(sealed.evidence.usage, started.then(|| usage(7)));
            assert_eq!(
                sealed.evidence.usage_completeness,
                if started {
                    AuxiliaryUsageCompleteness::Partial
                } else {
                    AuxiliaryUsageCompleteness::Unreported
                }
            );
            let local = matches!(
                kind,
                ModelErrorKind::Cancelled | ModelErrorKind::Timeout | ModelErrorKind::Transport
            );
            assert_eq!(
                sealed.evidence.observed_provider_terminal,
                (!local).then(|| AuxiliaryObservedProviderTerminal::Failed { kind: kind.clone() })
            );
            let definite_rejection = matches!(
                kind,
                ModelErrorKind::InvalidRequest
                    | ModelErrorKind::Authentication
                    | ModelErrorKind::RateLimit
                    | ModelErrorKind::ContextWindowExceeded
                    | ModelErrorKind::Unsupported
            );
            assert_eq!(
                sealed.evidence.remote_execution_uncertainty,
                started || !definite_rejection,
                "Started={started}, kind={kind:?}"
            );
        }
    }
}

#[test]
fn usage_snapshots_replace_details_and_distinguish_zero_from_unreported() {
    let zero = ModelUsage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        details: None,
    };
    for (latest, final_usage, completeness) in [
        (None, None, AuxiliaryUsageCompleteness::Unreported),
        (
            Some(zero.clone()),
            None,
            AuxiliaryUsageCompleteness::Partial,
        ),
        (
            None,
            Some(zero.clone()),
            AuxiliaryUsageCompleteness::Complete,
        ),
        (
            Some(usage(3)),
            Some(zero.clone()),
            AuxiliaryUsageCompleteness::Complete,
        ),
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        if let Some(latest) = &latest {
            let mut previous = usage(999);
            previous.details = Some(crate::model::UsageDetails {
                reasoning_tokens: Some(888),
                cached_input_tokens: Some(4),
            });
            fold.accept(&event(ModelEvent::UsageUpdate { usage: previous }), 101)
                .unwrap();
            fold.accept(
                &event(ModelEvent::UsageUpdate {
                    usage: latest.clone(),
                }),
                102,
            )
            .unwrap();
        }
        fold.accept(&completed(final_usage.clone()), 103).unwrap();
        let sealed = fold.finish(104);
        assert_eq!(sealed.output.unwrap().text, "");
        assert_eq!(sealed.evidence.usage, final_usage.or(latest));
        assert_eq!(sealed.evidence.usage_completeness, completeness);
        assert!(!sealed.evidence.remote_execution_uncertainty);
        assert_eq!(sealed.generation.first_output_ms, None);
        assert_eq!(sealed.generation.terminal_ms, 3);
    }
}

#[test]
fn discarded_reasoning_and_refusal_still_charge_cumulative_utf8_budgets() {
    for reasoning_limit in [Some(3), None] {
        let mut fold = fold();
        fold.guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 8,
            max_reasoning_bytes: reasoning_limit,
        });
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
            .unwrap();
        fold.accept(
            &event(ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "界".into(),
            }),
            102,
        )
        .unwrap();
        fold.accept(&text(1, "é"), 103).unwrap();
        fold.accept(
            &event(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(2),
                text: "否".into(),
            }),
            104,
        )
        .unwrap();
        // Exactly 8 generated UTF-8 bytes, including content that is not
        // returned as auxiliary text, are legal. The next byte must fail.
        let rejected = event(ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "!".into(),
        });
        let error = fold.accept(&rejected, 105).unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::GenerationBudgetExceeded);
        assert_eq!(
            error.generation,
            Some(GenerationFailure::RuntimeBudgetExceeded {
                budget: if reasoning_limit.is_some() {
                    GenerationBudgetKind::Reasoning
                } else {
                    GenerationBudgetKind::TotalOutput
                },
                limit_bytes: if reasoning_limit.is_some() { 3 } else { 8 },
                observed_bytes: if reasoning_limit.is_some() { 4 } else { 9 },
            })
        );
        assert_eq!(
            fold.accept(&completed(Some(usage(999))), 106),
            Err(error.clone())
        );
        let sealed = fold.finish(1001);
        assert_eq!(sealed.output, Err(error));
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert_eq!(
            sealed.evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
        assert_eq!(sealed.evidence.observed_provider_terminal, None);
        assert!(sealed.evidence.remote_execution_uncertainty);
        assert_eq!(sealed.generation.first_output_ms, Some(2));
        assert_eq!(
            sealed.generation.last_output_ms,
            Some(4),
            "rejected bytes are not trusted output"
        );
    }
}

#[test]
fn degeneration_on_each_text_channel_stops_before_claiming_a_terminal() {
    for channel in 0..3 {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 101)
            .unwrap();
        let repeated = "abcd".repeat(256);
        let block_index = ContentBlockIndex::new(0);
        let delta = match channel {
            0 => ModelEvent::TextDelta {
                block_index,
                text: repeated,
            },
            1 => ModelEvent::ReasoningDelta {
                block_index,
                text: repeated,
            },
            _ => ModelEvent::RefusalDelta {
                block_index,
                text: repeated,
            },
        };
        let error = fold.accept(&event(delta), 102).unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::GenerationDegenerated);
        assert_eq!(
            error.generation,
            Some(GenerationFailure::Degenerated {
                channel: if channel == 1 {
                    GenerationChannel::Reasoning
                } else {
                    GenerationChannel::Content
                },
                period_bytes: 4,
                repetitions: 256,
                span_bytes: 1024,
            })
        );
        assert_eq!(
            fold.accept(&completed(Some(usage(999))), 103),
            Err(error.clone())
        );
        let sealed = fold.finish(104);
        assert_eq!(sealed.output, Err(error));
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert_eq!(
            sealed.evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
        assert_eq!(sealed.evidence.observed_provider_terminal, None);
        assert!(sealed.evidence.remote_execution_uncertainty);
        assert_eq!(sealed.generation.first_output_ms, None);
        assert_eq!(sealed.generation.last_output_ms, None);
        assert_eq!(sealed.generation.terminal_ms, 4);
    }
}

#[test]
fn malformed_block_kind_does_not_poison_trusted_evidence_with_its_tail() {
    for invalid in [
        event(ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "foreign reasoning".into(),
        }),
        event(ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(0),
            text: "foreign refusal".into(),
        }),
        event(ModelEvent::ContinuationState {
            block_index: ContentBlockIndex::new(0),
            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"signature": "foreign-continuation"}),
            }),
        }),
        text(u32::MAX, "sparse allocation attack"),
    ] {
        let mut fold = fold();
        fold.accept(&event(ModelEvent::Started), 100).unwrap();
        fold.accept(&text(0, "trusted"), 101).unwrap();
        fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 102)
            .unwrap();
        let error = fold.accept(&invalid, 103).unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        assert_eq!(
            fold.accept(&completed(Some(usage(999))), 104),
            Err(error.clone())
        );
        let sealed = fold.finish(1001);
        assert_eq!(sealed.output, Err(error));
        assert_eq!(sealed.evidence.usage, Some(usage(7)));
        assert_eq!(
            sealed.evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
        assert_eq!(sealed.evidence.observed_provider_terminal, None);
        assert!(sealed.evidence.remote_execution_uncertainty);
        assert_eq!(sealed.generation.first_output_ms, Some(1));
        assert_eq!(sealed.generation.last_output_ms, Some(1));
    }
}

#[test]
fn generation_progress_changes_liveness_without_inventing_output_timing() {
    let mut fold = fold();
    fold.accept(&event(ModelEvent::Started), 100).unwrap();
    fold.accept(
        &ModelStreamItem::Progress(ModelStreamProgress::Generation),
        129,
    )
    .unwrap();
    assert_eq!(fold.deadline_millis(), 144);
    fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 143)
        .unwrap();
    assert_eq!(fold.deadline_millis(), 158);
    fold.accept(
        &event(ModelEvent::ContinuationState {
            block_index: ContentBlockIndex::new(0),
            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"signature": "must-not-leak"}),
            }),
        }),
        157,
    )
    .unwrap();
    assert_eq!(fold.deadline_millis(), 172);
    fold.accept(&text(1, ""), 171).unwrap();
    assert_eq!(fold.deadline_millis(), 186);
    fold.accept(
        &event(ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "private reasoning".into(),
        }),
        185,
    )
    .unwrap();
    fold.accept(&completed(None), 199).unwrap();
    let sealed = fold.finish(999);
    assert_eq!(sealed.output.unwrap().text, "");
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Partial
    );
    assert_eq!(sealed.generation.first_output_ms, Some(85));
    assert_eq!(sealed.generation.last_output_ms, Some(85));
    assert_eq!(sealed.generation.terminal_ms, 99);
}

#[tokio::test(flavor = "current_thread")]
async fn ready_control_wins_without_polling_a_ready_provider_tail() {
    for provider_completed in [false, true] {
        for cancel in [false, true] {
            for tail in [Some(completed(Some(usage(999)))), None] {
                let clock = manual_clock();
                let cancellation = CancellationSignal::new();
                let mut fold = fold();
                fold.accept(&event(ModelEvent::Started), 100).unwrap();
                fold.accept(&text(0, "trusted"), 100).unwrap();
                fold.accept(&event(ModelEvent::UsageUpdate { usage: usage(7) }), 100)
                    .unwrap();
                if provider_completed {
                    fold.accept(&completed(Some(usage(9))), 100).unwrap();
                }
                clock.advance(900);
                if cancel {
                    cancellation.cancel();
                }
                let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let count = polls.clone();
                let mut tail = tail;
                let stream = Box::pin(futures_util::stream::poll_fn(move |_| {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::task::Poll::Ready(tail.take())
                }));
                let AuxiliaryStreamExit::Failed(sealed) =
                    poll_stream(stream, fold, &cancellation, &clock).await
                else {
                    panic!("ready control cannot return an EOF success candidate")
                };
                let error = sealed.output.unwrap_err();
                assert_eq!(
                    error.kind,
                    if cancel {
                        ModelErrorKind::Cancelled
                    } else {
                        ModelErrorKind::Timeout
                    }
                );
                assert_eq!(error.timeout_phase, None);
                assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 0);
                assert_eq!(
                    sealed.evidence.usage,
                    Some(usage(if provider_completed { 9 } else { 7 }))
                );
                assert_eq!(
                    sealed.evidence.usage_completeness,
                    if provider_completed {
                        AuxiliaryUsageCompleteness::Complete
                    } else {
                        AuxiliaryUsageCompleteness::Partial
                    }
                );
                assert_eq!(
                    sealed.evidence.observed_provider_terminal,
                    provider_completed.then_some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::AuxiliaryFinishReason::Stop
                    })
                );
                assert_eq!(
                    sealed.evidence.remote_execution_uncertainty,
                    !provider_completed
                );
                assert_eq!(
                    sealed.generation.terminal_ms,
                    if provider_completed { 0 } else { 900 }
                );
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_completion_poll_prevents_the_next_eof_poll() {
    let clock = manual_clock();
    let cancellation = CancellationSignal::new();
    let cancel = cancellation.clone();
    let stream = futures_util::stream::iter([
        event(ModelEvent::Started),
        text(0, "provisional result"),
        completed(Some(usage(7))),
    ])
    .inspect(move |item| {
        if matches!(item, ModelStreamItem::Event(ModelEvent::Completed { .. })) {
            cancel.cancel();
        }
    })
    .chain(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<ModelStreamItem>> {
            panic!("cancellation must win before another provider/EOF poll")
        },
    ));
    let AuxiliaryStreamExit::Failed(sealed) =
        poll_stream(Box::pin(stream), fold(), &cancellation, &clock).await
    else {
        panic!("cancellation observed before sealing cannot produce success")
    };
    assert_eq!(sealed.output.unwrap_err().kind, ModelErrorKind::Cancelled);
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Complete
    );
    assert_eq!(
        sealed.evidence.observed_provider_terminal,
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::AuxiliaryFinishReason::Stop
        })
    );
    assert!(!sealed.evidence.remote_execution_uncertainty);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_returned_by_a_poll_crossing_the_deadline_is_evidence_not_success() {
    let clock = std::sync::Arc::new(manual_clock());
    let during_poll = clock.clone();
    let cancellation = CancellationSignal::new();
    let stream = futures_util::stream::iter([
        event(ModelEvent::Started),
        text(0, "result"),
        completed(Some(usage(7))),
    ])
    .inspect(move |item| {
        if matches!(item, ModelStreamItem::Event(ModelEvent::Completed { .. })) {
            // The timer branch was pending before the provider was polled.
            // Reaching the idle boundary inside that poll must not bypass it.
            during_poll.advance(15);
        }
    })
    .chain(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<ModelStreamItem>> {
            panic!("the late terminal must fail promptly, without a tail poll")
        },
    ));
    let AuxiliaryStreamExit::Failed(sealed) =
        poll_stream(Box::pin(stream), fold(), &cancellation, clock.as_ref()).await
    else {
        panic!("an in-poll deadline crossing cannot produce success")
    };
    let error = sealed.output.unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::Timeout);
    assert_eq!(error.timeout_phase, Some(ModelTimeoutPhase::StreamIdle));
    assert_eq!(sealed.evidence.usage, Some(usage(7)));
    assert_eq!(
        sealed.evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Complete
    );
    assert_eq!(
        sealed.evidence.observed_provider_terminal,
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::AuxiliaryFinishReason::Stop
        })
    );
    assert!(!sealed.evidence.remote_execution_uncertainty);
    assert_eq!(sealed.generation.first_output_ms, Some(0));
    assert_eq!(sealed.generation.last_output_ms, Some(0));
    assert_eq!(sealed.generation.terminal_ms, 15);
}
