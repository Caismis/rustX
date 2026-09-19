//! Request-owned durable generation timing evidence.
//!
//! One actual model request produces at most one [`GenerationEvidence`]
//! value, settled together with that request's provider terminal fact. It is
//! **execution evidence**, not canonical history: no Assistant message, no
//! Message Ledger row, and no Conversation Surface revision ever contains
//! it, and nothing recovers, resumes, or settles execution from it.
//!
//! ## Why offsets rather than timestamps
//!
//! Output and terminal offsets use the adapter dispatch monotonic origin.
//! `dispatch_after_start_ms` bridges that origin to the monotonic reading
//! paired with the exact UTC timestamp supplied to the durable start transaction.
//! The pair is captured under start arbitration; it is retained only after a
//! fresh successful commit. Commit linearizes the fact's existence, while its
//! supplied timestamp defines its timeline origin. No later wall clock repairs
//! or estimates this relationship. Missing bridge evidence means no phase positions.
//!
//! Four bounded scalars are retained. No provider deltas or per-token series
//! are journalled, and Trace only projects this request-owned evidence.
//!
//! ## What "model output" means
//!
//! [`GenerationEvidence::first_output_ms`] is the offset of the first
//! *provider-independent* model output: text, reasoning, or refusal with
//! non-empty content, or the assembly of a tool call. Provider protocol
//! frames that carry no generated content — stream start, usage updates,
//! continuation state — are deliberately not output, so TTFT measures
//! generation rather than connection setup.

use serde::{Deserialize, Serialize};

use crate::model::event::ModelEvent;

/// Settled provider-independent timing evidence for one actual request.
///
/// An optional boundary or clock relationship is absent when it was not
/// observed. Readers must never fill missing evidence: a request that failed
/// before producing output has no first output, and a historical request
/// without a measured start/dispatch bridge has no request-relative phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationEvidence {
    /// Monotonic elapsed time from the paired durable-start origin to dispatch.
    /// Absent when that relationship was not captured; never infer it later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub dispatch_after_start_ms: Option<u64>,
    /// Offset of the first non-empty provider-independent model output.
    ///
    /// `None` when this request produced no model output at all. Time to
    /// first token is exactly this value; it is never derived another way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub first_output_ms: Option<u64>,
    /// Offset of the last non-empty provider-independent model output.
    ///
    /// Equal to [`Self::first_output_ms`] for a single-output generation.
    /// `None` exactly when `first_output_ms` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub last_output_ms: Option<u64>,
    /// Offset of the provider terminal — completion or normalized failure.
    ///
    /// This is always present: the evidence is settled by that exact
    /// terminal, so the terminal's own offset is always observed.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub terminal_ms: u64,
}

impl GenerationEvidence {
    /// Time to first model output, in milliseconds.
    ///
    /// Unknown — and therefore `None` — when the generation produced no
    /// output. A failed request that streamed text before failing still has
    /// a truthful TTFT; a request that failed during connection does not.
    #[must_use]
    pub const fn time_to_first_output_ms(&self) -> Option<u64> {
        self.first_output_ms
    }

    /// Decode duration: first model output through the provider terminal.
    ///
    /// `None` without a first output. The terminal is the end of the
    /// generation the provider actually ran, so this remains correct for a
    /// generation that ended in failure after producing output.
    #[must_use]
    pub fn generation_ms(&self) -> Option<u64> {
        self.first_output_ms
            .and_then(|first| self.terminal_ms.checked_sub(first))
    }

    /// Output throughput in tokens per second, given this request's usage.
    ///
    /// `None` whenever any input is missing or the decode span is zero: a
    /// rate over an unmeasurable interval is not a slow rate, it is an
    /// unknown one.
    #[must_use]
    pub fn throughput_tokens_per_second(&self, output_tokens: u64) -> Option<f64> {
        let generation = self.generation_ms()?;
        if generation == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)] // Display metric; exactness is not claimed.
        Some(output_tokens as f64 / (generation as f64 / 1000.0))
    }
}

/// Request-local accumulator that observes one physical generation.
///
/// Created at the dispatch frontier and dropped with the request. It owns no
/// durable state, decides nothing, and cannot outlive its request: a retry
/// creates its own accumulator, exactly like the request deadline and the
/// generation guard, so it can never inherit a replaced generation's spans.
#[derive(Debug, Clone, Copy)]
pub struct GenerationTiming {
    /// The monotonic reading of this request's dispatch frontier.
    origin: u64,
    dispatch_after_start_ms: Option<u64>,
    terminal: Option<u64>,
    /// Offset of the first observed model output.
    first: Option<u64>,
    /// Offset of the most recent observed model output.
    last: Option<u64>,
}

impl GenerationTiming {
    /// Starts the accumulator at this request's dispatch frontier.
    /// `request_start_ms` must be the monotonic reading paired with the exact
    /// timestamp supplied to this request's freshly committed durable start.
    #[must_use]
    pub fn started_at(origin_ms: u64, request_start_ms: Option<u64>) -> Self {
        Self {
            origin: origin_ms,
            dispatch_after_start_ms: request_start_ms
                .and_then(|start| origin_ms.checked_sub(start)),
            terminal: None,
            first: None,
            last: None,
        }
    }

    /// Records one normalized model event observed at `now_ms`.
    ///
    /// Only generated model content advances the output span. The order of
    /// the two assignments matters: the first output also becomes the last,
    /// so a single-output generation retains both output endpoints.
    pub fn observe(&mut self, event: &ModelEvent, now_ms: u64) {
        if matches!(
            event,
            ModelEvent::Completed { .. } | ModelEvent::Failed { .. }
        ) {
            self.terminal
                .get_or_insert(now_ms.saturating_sub(self.origin));
        }
        if self.terminal.is_some() || !carries_model_output(event) {
            return;
        }
        let offset = now_ms.saturating_sub(self.origin);
        if self.first.is_none() {
            self.first = Some(offset);
        }
        self.last = Some(offset);
    }

    /// Settles this request's evidence at its provider terminal.
    #[must_use]
    pub fn settle(self, terminal_ms: u64) -> GenerationEvidence {
        GenerationEvidence {
            dispatch_after_start_ms: self.dispatch_after_start_ms,
            first_output_ms: self.first,
            last_output_ms: self.last,
            terminal_ms: self
                .terminal
                .unwrap_or_else(|| terminal_ms.saturating_sub(self.origin)),
        }
    }
}

/// Whether one normalized model event carries generated model output.
///
/// Empty text deltas are explicitly excluded: some providers open a content
/// block with an empty delta, and treating that as the first token would
/// report a TTFT that measures the provider's framing rather than its
/// generation.
fn carries_model_output(event: &ModelEvent) -> bool {
    match event {
        ModelEvent::TextDelta { text, .. }
        | ModelEvent::ReasoningDelta { text, .. }
        | ModelEvent::RefusalDelta { text, .. } => !text.is_empty(),
        ModelEvent::ToolCallStarted { .. } | ModelEvent::ToolCallCompleted { .. } => true,
        ModelEvent::ToolCallArgumentsDelta {
            arguments_delta, ..
        } => !arguments_delta.is_empty(),
        ModelEvent::Started
        | ModelEvent::UsageUpdate { .. }
        | ModelEvent::ContinuationState { .. }
        | ModelEvent::Completed { .. }
        | ModelEvent::Failed { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{GenerationEvidence, GenerationTiming};
    use crate::message::types::ContentBlockIndex;
    use crate::model::event::ModelEvent;

    fn text(text: &str) -> ModelEvent {
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        }
    }

    /// Offsets are measured from the dispatch frontier, not from zero.
    #[test]
    fn offsets_are_relative_to_the_dispatch_frontier() {
        let mut timing = GenerationTiming::started_at(10_000, Some(10_000));
        timing.observe(&text("hello"), 10_250);
        timing.observe(&text(" world"), 10_900);
        let evidence = timing.settle(11_000);
        assert_eq!(evidence.first_output_ms, Some(250));
        assert_eq!(evidence.last_output_ms, Some(900));
        assert_eq!(evidence.terminal_ms, 1_000);
    }

    /// A provider frame that carries no generated content is not output.
    #[test]
    fn framing_events_do_not_start_the_output_span() {
        let mut timing = GenerationTiming::started_at(0, Some(0));
        timing.observe(&ModelEvent::Started, 100);
        timing.observe(
            &ModelEvent::UsageUpdate {
                usage: crate::model::types::ModelUsage {
                    input_tokens: 1,
                    output_tokens: 0,
                    total_tokens: 1,
                    details: None,
                },
            },
            150,
        );
        timing.observe(&text(""), 200);
        timing.observe(&text("x"), 400);
        assert_eq!(timing.settle(500).first_output_ms, Some(400));
    }

    /// A generation with no output keeps every output span unavailable.
    #[test]
    fn a_generation_without_output_has_no_spans() {
        let evidence = GenerationTiming::started_at(0, Some(0)).settle(750);
        assert_eq!(evidence.first_output_ms, None);
        assert_eq!(evidence.last_output_ms, None);
        assert_eq!(evidence.terminal_ms, 750);
        assert_eq!(evidence.time_to_first_output_ms(), None);
        assert_eq!(evidence.generation_ms(), None);
        assert_eq!(evidence.throughput_tokens_per_second(100), None);
    }

    /// Derived metrics use only the authoritative endpoints.
    #[test]
    fn derived_metrics_use_authoritative_endpoints_only() {
        let evidence = GenerationEvidence {
            dispatch_after_start_ms: None,
            first_output_ms: Some(200),
            last_output_ms: Some(1_100),
            terminal_ms: 1_200,
        };
        assert_eq!(evidence.time_to_first_output_ms(), Some(200));
        assert_eq!(evidence.generation_ms(), Some(1_000));
        let throughput = evidence
            .throughput_tokens_per_second(500)
            .expect("throughput over a measured decode span");
        assert!((throughput - 500.0).abs() < f64::EPSILON);
    }

    /// An unmeasurable decode span yields no rate rather than a huge one.
    #[test]
    fn a_zero_length_decode_span_has_no_throughput() {
        let evidence = GenerationEvidence {
            dispatch_after_start_ms: None,
            first_output_ms: Some(400),
            last_output_ms: Some(400),
            terminal_ms: 400,
        };
        assert_eq!(evidence.generation_ms(), Some(0));
        assert_eq!(evidence.throughput_tokens_per_second(10), None);
    }

    /// A single output is both the first and the last one.
    #[test]
    fn a_single_output_settles_both_endpoints() {
        let mut timing = GenerationTiming::started_at(0, Some(0));
        timing.observe(&text("only"), 300);
        let evidence = timing.settle(310);
        assert_eq!(evidence.first_output_ms, Some(300));
        assert_eq!(evidence.last_output_ms, Some(300));
    }
}
