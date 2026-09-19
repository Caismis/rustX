//! Response aggregation is a sibling of Trace over the same request evidence.
use crate::durable::response::CompletedResponseTiming;
use crate::model::{ModelUsage, generation_evidence::GenerationEvidence};
use crate::runtime::identity::RequestId;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

struct RequestReading {
    generation: Option<GenerationEvidence>,
    output_tokens: Option<u64>,
}

#[derive(Default)]
pub(super) struct TimingFold {
    first: Option<RequestId>,
    requests: BTreeMap<RequestId, Option<RequestReading>>,
}
impl TimingFold {
    pub(super) fn start(&mut self, id: RequestId) {
        self.first.get_or_insert_with(|| id.clone());
        self.requests.insert(id, None);
    }
    pub(super) fn terminal(
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
    pub(super) fn summary(
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
