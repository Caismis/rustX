//! Bounded deterministic selection over compiled typed historical input.
use crate::context::engine::{CompactionBudgets, CompactionConstraints};
use crate::context::{ContextEngine, ContextError, ContextErrorKind};
use crate::model::{ModelInputMessage, input::assemble_model_input, projected::ModelMessageView};
use crate::runtime::CancellationSignal;
use crate::tools::types::ModelToolDefinition;
use std::collections::{BTreeMap, BTreeSet};

/// Arbitrary estimators get a finite exact-query budget, not sampled infeasibility.
pub(crate) const MAX_GENERIC_CANDIDATES: usize = 64;
fn failure(kind: ContextErrorKind, text: &str) -> ContextError {
    ContextError::new(kind, text)
}
async fn cancellation_point(signal: &CancellationSignal) -> Result<(), ContextError> {
    if !signal.is_cancelled() {
        tokio::task::yield_now().await;
    }
    if signal.is_cancelled() {
        Err(failure(
            ContextErrorKind::Cancelled,
            "compaction cancelled during historical planning",
        ))
    } else {
        Ok(())
    }
}

/// Render once, then perform a complete monotone interval search when the
/// estimator proves that contract. Otherwise inspect descending boundaries
/// within a finite budget, reporting incomplete search if preference is unknown.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn summary_prefix(
    engine: &ContextEngine,
    input: &[ModelInputMessage],
    prompt: &str,
    tools: &[ModelToolDefinition],
    budgets: CompactionBudgets,
    constraints: &CompactionConstraints<'_>,
    cancellation: &CancellationSignal,
) -> Result<usize, ContextError> {
    cancellation_point(cancellation).await?;
    let mut limit = engine.soft_input_limit(budgets.primary_output_budget)?;
    if let Some(correction) = constraints.estimate_correction {
        limit = correction.apply(limit);
    }
    let mut positions = BTreeMap::new();
    for (index, message) in input.iter().enumerate() {
        if index % 64 == 0 {
            cancellation_point(cancellation).await?;
        }
        if let Some(id) = message.canonical_id() {
            positions.entry(id).or_insert(index);
        }
    }
    let min = match constraints.must_cover_through {
        Some(id) => positions.get(id).map(|index| index + 1).ok_or_else(|| {
            failure(
                ContextErrorKind::NoProgress,
                "required continuation anchor is absent from historical projection",
            )
        })?,
        None => 1,
    };
    let mut protected: BTreeSet<_> = constraints.protected_context_ids.iter().collect();
    if let Some(fresh) = constraints.fresh_inbound {
        for id in fresh.message_ids() {
            if !positions.contains_key(id) {
                return Err(failure(
                    ContextErrorKind::MalformedHistory,
                    "Fresh Inbound requires its Original fact; a Derived source mention cannot supply it",
                ));
            }
            protected.insert(id);
        }
    }
    if let Some(crate::model::RequestOnlyInsertionAnchor::BeforeMessage(anchor)) =
        constraints.carryover_anchor
    {
        protected.insert(anchor);
    }
    let max = protected
        .iter()
        .filter_map(|id| positions.get(id))
        .copied()
        .min()
        .unwrap_or(input.len());
    let mut pending = BTreeMap::<String, usize>::new();
    let mut pending_count = 0;
    let mut boundaries = Vec::new();
    let mut transcript = String::new();
    for (i, message) in input.iter().enumerate().take(max) {
        if i % 64 == 0 {
            cancellation_point(cancellation).await?;
        }
        match message.view() {
            ModelMessageView::Assistant(content) => {
                for block in content {
                    if let crate::message::types::AssistantContentBlock::ToolCall(call) = block {
                        *pending.entry(call.id.to_string()).or_default() += 1;
                        pending_count += 1;
                    }
                }
            }
            ModelMessageView::Tool(id, _) => {
                let count = pending.get_mut(id.as_str()).ok_or_else(|| {
                    failure(
                        ContextErrorKind::MalformedHistory,
                        "orphan projected ToolResult",
                    )
                })?;
                *count -= 1;
                pending_count -= 1;
                if *count == 0 {
                    pending.remove(id.as_str());
                }
            }
            _ => {}
        }
        super::summarizer::append_summary_message(message, &mut transcript);
        let n = i + 1;
        if n >= min && pending_count == 0 {
            boundaries.push((n, transcript.len()));
        }
    }
    let complete = assemble_model_input(
        input,
        constraints.staged_request_context,
        constraints.carryover,
        constraints.carryover_anchor,
    )
    .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
    #[cfg(test)]
    record_materialized_bytes(&complete);
    let monotone = engine.supports_monotone_compaction(&complete);
    drop(complete);
    let mut search = Search {
        engine,
        input,
        prompt,
        tools,
        constraints,
        cancellation,
        transcript: &transcript,
        boundaries: &boundaries,
        cache: BTreeMap::new(),
    };
    let eligible = boundaries.len();
    let selected = if monotone && eligible > 0 {
        // Summary fits form an initial interval; primary fits a final one.
        let upper = search
            .partition(|cost| cost.summary <= budgets.summary_input_limit)
            .await?;
        let lower = search
            .partition(|cost| {
                cost.primary
                    .saturating_add(u64::from(budgets.summary_output_budget))
                    > limit
            })
            .await?;
        if lower >= upper {
            None
        } else {
            let recent = search
                .partition(|cost| cost.recent >= engine.config().keep_recent_tokens)
                .await?;
            Some(if recent > lower {
                upper.min(recent) - 1
            } else {
                lower
            })
        }
    } else {
        let mut fallback = None;
        let mut preferred = None;
        let examined = eligible.min(MAX_GENERIC_CANDIDATES);
        for index in (0..eligible).rev().take(examined) {
            let cost = search.cost(index).await?;
            if cost.summary <= budgets.summary_input_limit
                && cost
                    .primary
                    .saturating_add(u64::from(budgets.summary_output_budget))
                    <= limit
            {
                fallback = Some(index);
                if cost.recent >= engine.config().keep_recent_tokens {
                    preferred = Some(index);
                    break; // Every larger boundary has been excluded: exact preference.
                }
            }
        }
        if preferred.is_none() && examined < eligible {
            cancellation_point(cancellation).await?;
            return Err(failure(
                ContextErrorKind::SearchIncomplete,
                &format!(
                    "compaction candidate search incomplete: inspected {examined} of {eligible} eligible boundaries; estimator has no monotone accounting guarantee"
                ),
            ));
        }
        preferred.or(fallback)
    };
    cancellation_point(cancellation).await?;
    let Some(index) = selected else {
        return Err(failure(
            if max == 0 {
                ContextErrorKind::NoProgress
            } else {
                ContextErrorKind::CannotFit
            },
            "no complete historical prefix satisfies summary, primary and core-retention budgets",
        ));
    };
    // A proof narrows search, never replaces authoritative whole-candidate pricing.
    let cost = search.cost(index).await?;
    if cost.summary > budgets.summary_input_limit
        || cost
            .primary
            .saturating_add(u64::from(budgets.summary_output_budget))
            > limit
    {
        return Err(failure(
            ContextErrorKind::Internal,
            "estimator violated its monotone compaction contract",
        ));
    }
    cancellation_point(cancellation).await?;
    Ok(boundaries[index].0)
}

#[derive(Clone, Copy)]
struct Cost {
    summary: u64,
    primary: u64,
    recent: u64,
}
struct Search<'a, 'c> {
    engine: &'a ContextEngine,
    input: &'a [ModelInputMessage],
    prompt: &'a str,
    tools: &'a [ModelToolDefinition],
    constraints: &'a CompactionConstraints<'c>,
    cancellation: &'a CancellationSignal,
    transcript: &'a str,
    boundaries: &'a [(usize, usize)],
    cache: BTreeMap<usize, Cost>,
}
impl Search<'_, '_> {
    async fn cost(&mut self, index: usize) -> Result<Cost, ContextError> {
        cancellation_point(self.cancellation).await?;
        if let Some(cost) = self.cache.get(&index) {
            return Ok(*cost);
        }
        let (n, end) = self.boundaries[index];
        let summary = super::summarizer::summary_model_input(&self.transcript[..end]);
        let summary_tokens = self.engine.estimate_model_input(&summary.messages, "", &[]);
        cancellation_point(self.cancellation).await?;
        let planned = assemble_model_input(
            &self.input[n..],
            self.constraints.staged_request_context,
            self.constraints.carryover,
            self.constraints.carryover_anchor,
        )
        .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
        #[cfg(test)]
        record_materialized_bytes(&planned);
        let primary = self
            .engine
            .estimate_model_input(&planned, self.prompt, self.tools);
        cancellation_point(self.cancellation).await?;
        let recent = self.engine.estimate_model_input(&self.input[n..], "", &[]);
        cancellation_point(self.cancellation).await?;
        let cost = Cost {
            summary: summary_tokens,
            primary,
            recent,
        };
        self.cache.insert(index, cost);
        Ok(cost)
    }
    // The supplied predicate must be an initial true interval, proven by the
    // estimator's capability, not inferred from any sampled neighbor.
    async fn partition(&mut self, predicate: impl Fn(Cost) -> bool) -> Result<usize, ContextError> {
        let (mut low, mut high) = (0, self.boundaries.len());
        while low < high {
            let mid = low + (high - low) / 2;
            if predicate(self.cost(mid).await?) {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        Ok(low)
    }
}
#[cfg(test)]
thread_local! { static COPIED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_copied_bytes() -> u64 {
    COPIED_BYTES.with(|total| total.replace(0))
}

#[cfg(test)]
fn record_materialized_bytes(messages: &[ModelInputMessage]) {
    // Logical serialized bytes of bodies actually cloned by assembly, not RSS.
    COPIED_BYTES.with(|total| {
        total.set(
            total.get().saturating_add(
                crate::context::resources::bounded_encoded_len(&messages, usize::MAX)
                    .expect("test accounting") as u64,
            ),
        );
    });
}
