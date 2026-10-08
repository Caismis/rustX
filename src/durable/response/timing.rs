//! Response aggregation is a sibling of Trace over the same request evidence.
use crate::durable::reading::TurnExecution;
use crate::durable::response::{CompletedResponseTiming, ConversationTiming};
use crate::model::{ModelUsage, generation_evidence::GenerationEvidence};
use crate::runtime::identity::{AttemptId, RequestId, ToolCallId};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

struct RequestReading {
    generation: Option<GenerationEvidence>,
    output_tokens: Option<u64>,
}

#[derive(Default)]
pub(crate) struct TimingFold {
    first: Option<RequestId>,
    requests: BTreeMap<RequestId, Option<RequestReading>>,
}
impl TimingFold {
    pub(crate) fn start(&mut self, id: RequestId) {
        self.first.get_or_insert_with(|| id.clone());
        self.requests.insert(id, None);
    }
    pub(crate) fn terminal(
        &mut self,
        id: &RequestId,
        generation: Option<GenerationEvidence>,
        usage: Option<&ModelUsage>,
    ) {
        if let Some(request) = self.requests.get_mut(id) {
            *request = Some(RequestReading {
                generation,
                output_tokens: usage.map(|usage| usage.output_tokens),
            });
        }
    }
    pub(crate) fn summary(
        &self,
        start: Option<DateTime<Utc>>,
        end: DateTime<Utc>,
    ) -> Option<CompletedResponseTiming> {
        let total_duration_ms = start.filter(|start| *start <= end).and_then(|start| {
            end.signed_duration_since(start)
                .num_milliseconds()
                .try_into()
                .ok()
        });
        let ttft_ms = self
            .first
            .as_ref()
            .and_then(|id| self.requests.get(id))
            .and_then(|request| request.as_ref())
            .and_then(|request| request.generation.as_ref())
            .and_then(GenerationEvidence::time_to_first_output_ms);
        let work = self.work();
        let generation_ms = work.map(|(duration, _)| duration);
        #[allow(clippy::cast_precision_loss)]
        // Display ratio, as in GenerationEvidence's request throughput helper.
        let output_tokens_per_second = work.and_then(|(duration, output)| {
            output
                .filter(|_| duration > 0)
                .map(|output| output as f64 * 1000.0 / duration as f64)
        });
        let summary = CompletedResponseTiming {
            total_duration_ms,
            ttft_ms,
            generation_ms,
            output_tokens_per_second,
        };
        (summary != CompletedResponseTiming::default()).then_some(summary)
    }
    // Every actual request must have terminal evidence. Known no-output requests
    // add no decode span; unknown evidence invalidates the exact aggregate.
    // Rates additionally require all usage, and positive spans for every request
    // with output. A measured zero span remains valid duration but has no rate.
    fn work(&self) -> Option<(u64, Option<u64>)> {
        let mut duration = 0_u64;
        let mut output = Some(0_u64);
        let mut observed_output = false;
        for request in self.requests.values() {
            let request = request.as_ref()?;
            let generation = request.generation.as_ref()?;
            let tokens = request.output_tokens;
            if generation.first_output_ms.is_some() {
                observed_output = true;
                let span = generation.generation_ms()?;
                duration = duration.checked_add(span)?;
                output = output
                    .zip(tokens)
                    .filter(|_| span > 0)
                    .and_then(|(a, b)| a.checked_add(b));
            } else if generation.last_output_ms.is_some() {
                return None;
            } else if tokens != Some(0) {
                output = None;
            }
        }
        observed_output.then_some((duration, output))
    }
}

/// Whole-conversation work totals: Harness session statistics over native
/// request generation evidence and foreground Tool lifecycle timestamps.
#[derive(Default)]
pub(crate) struct ActivityFold {
    model_ms: Option<u64>,
    ttft_ms: u64,
    ttft_requests: u64,
    decode_ms: u64,
    decode_tokens: u64,
    tool_ms: Option<u64>,
    tools: BTreeMap<(AttemptId, ToolCallId), DateTime<Utc>>,
}
impl ActivityFold {
    pub(crate) fn request(
        &mut self,
        generation: Option<&GenerationEvidence>,
        usage: Option<&ModelUsage>,
    ) {
        let Some(generation) = generation else {
            return;
        };
        self.model_ms = Some(
            self.model_ms
                .unwrap_or(0)
                .saturating_add(generation.terminal_ms),
        );
        if let Some(first) = generation.time_to_first_output_ms() {
            self.ttft_ms = self.ttft_ms.saturating_add(first);
            self.ttft_requests += 1;
        }
        if let (Some(span), Some(usage)) = (generation.generation_ms(), usage)
            && span > 0
        {
            self.decode_ms = self.decode_ms.saturating_add(span);
            self.decode_tokens = self.decode_tokens.saturating_add(usage.output_tokens);
        }
    }
    pub(crate) fn tool_started(&mut self, attempt: AttemptId, call: ToolCallId, at: DateTime<Utc>) {
        self.tools.insert((attempt, call), at);
    }
    /// A terminal without its recorded start, or a reversed clock, adds nothing.
    pub(crate) fn tool_settled(&mut self, attempt: AttemptId, call: ToolCallId, at: DateTime<Utc>) {
        let Some(started) = self.tools.remove(&(attempt, call)) else {
            return;
        };
        let span = u64::try_from(at.signed_duration_since(started).num_milliseconds()).unwrap_or(0);
        self.tool_ms = Some(self.tool_ms.unwrap_or(0).saturating_add(span));
    }
    /// Writes this fold's additive work into one inherited turn's evidence.
    pub(crate) fn record(&self, execution: &mut TurnExecution) {
        execution.model_ms = self.model_ms;
        execution.ttft_ms = self.ttft_ms;
        execution.ttft_requests = self.ttft_requests;
        execution.decode_ms = self.decode_ms;
        execution.decode_tokens = self.decode_tokens;
        execution.tool_ms = self.tool_ms;
    }
    /// Adds one inherited turn's recorded work, as if its facts were folded here.
    pub(crate) fn absorb(&mut self, execution: &TurnExecution) {
        let add = |total: Option<u64>, part: Option<u64>| match (total, part) {
            (total, None) => total,
            (total, Some(part)) => Some(total.unwrap_or(0).saturating_add(part)),
        };
        self.model_ms = add(self.model_ms, execution.model_ms);
        self.tool_ms = add(self.tool_ms, execution.tool_ms);
        self.ttft_ms = self.ttft_ms.saturating_add(execution.ttft_ms);
        self.ttft_requests += execution.ttft_requests;
        self.decode_ms = self.decode_ms.saturating_add(execution.decode_ms);
        self.decode_tokens = self.decode_tokens.saturating_add(execution.decode_tokens);
    }
    pub(crate) fn summary(&self) -> Option<ConversationTiming> {
        #[allow(clippy::cast_precision_loss)] // Display ratio, as in the per-response rate.
        let output_tokens_per_second = (self.decode_ms > 0)
            .then(|| self.decode_tokens as f64 * 1000.0 / self.decode_ms as f64);
        let summary = ConversationTiming {
            model_ms: self.model_ms,
            tool_ms: self.tool_ms,
            mean_ttft_ms: (self.ttft_requests > 0).then(|| self.ttft_ms / self.ttft_requests),
            output_tokens_per_second,
        };
        (summary != ConversationTiming::default()).then_some(summary)
    }
}
