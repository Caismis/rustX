//! Settled generation display metrics derived from request-owned evidence.
use super::types::{TraceGeneration, TraceGenerationTimeline};
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::types::ModelUsage;

/// Derives display metrics from settled evidence and this request's usage.
pub(crate) fn generation_metrics(
    evidence: GenerationEvidence,
    usage: Option<&ModelUsage>,
) -> TraceGeneration {
    TraceGeneration {
        timeline: generation_timeline(evidence),
        ttft_ms: evidence.time_to_first_output_ms(),
        generation_ms: evidence.generation_ms(),
        terminal_ms: evidence.terminal_ms,
        // Throughput needs both a measured decode span and reported output
        // tokens. Missing usage leaves it unavailable rather than zero.
        output_tokens_per_second: usage
            .and_then(|usage| evidence.throughput_tokens_per_second(usage.output_tokens)),
    }
}
/// Positions require the runtime's bridge; numeric TTFT alone is insufficient.
fn generation_timeline(evidence: GenerationEvidence) -> Option<TraceGenerationTimeline> {
    let dispatch_ms = evidence.dispatch_after_start_ms?;
    let first_output_ms = match evidence.first_output_ms {
        Some(v) => Some(dispatch_ms.checked_add(v)?),
        None => None,
    };
    let last_output_ms = match evidence.last_output_ms {
        Some(v) => Some(dispatch_ms.checked_add(v)?),
        None => None,
    };
    Some(TraceGenerationTimeline {
        dispatch_ms,
        first_output_ms,
        last_output_ms,
        terminal_ms: dispatch_ms.checked_add(evidence.terminal_ms)?,
    })
}
