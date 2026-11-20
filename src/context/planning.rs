//! Bounded deterministic selection over compiled typed historical input.
use super::engine::assemble_context_input;
use crate::context::engine::{CompactionBudgets, CompactionConstraints};
use crate::context::{ContextEngine, ContextError, ContextErrorKind};
use crate::model::ModelInputMessage;
use crate::model::{SelectedModelInput, projected::ModelMessageView};
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
    input: &SelectedModelInput,
    prompt: &str,
    tools: &[ModelToolDefinition],
    budgets: CompactionBudgets,
    constraints: &CompactionConstraints<'_>,
    cancellation: &CancellationSignal,
) -> Result<usize, ContextError> {
    cancellation_point(cancellation).await?;
    let mut limit = engine.admissible_input_limit(budgets.primary_output_budget)?;
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
    let mut min = match constraints.must_cover_through {
        Some(id) => positions.get(id).map(|index| index + 1).ok_or_else(|| {
            failure(
                ContextErrorKind::MalformedHistory,
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
    if max == 0 && !protected.is_empty() {
        return Err(failure(
            ContextErrorKind::NoEligibleHistory,
            "protected Fresh Inbound or current Context begins before any eligible historical source",
        ));
    }
    let first_new = input.iter().take(max).position(|message| !matches!(
        message,
        ModelInputMessage::Derived(derived)
            if derived.transformations.contains(&crate::model::projected::SemanticTransformation::Summary)
    ));
    // Prefer retiring new source when available. A Summary alone can still
    // need reduction after a smaller primary is selected or a provider
    // reports overflow.
    if let Some(first_new) = first_new {
        min = min.max(first_new + 1);
    }
    let mut pending = BTreeMap::<String, usize>::new();
    let mut pending_count = 0;
    let mut boundaries = Vec::new();
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
        let n = i + 1;
        if n >= min && pending_count == 0 {
            boundaries.push(n);
        }
    }
    let complete = assemble_context_input(
        input,
        constraints.staged_request_context,
        constraints.carryover,
        constraints.carryover_anchor,
    )
    .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
    let monotone = engine.supports_monotone_compaction(&complete);
    drop(complete);
    // Historical progress is judged on the whole projected history, without
    // request-only or staged inputs. The candidate uses the same accounting
    // path as the final checkpoint validation.
    let history_before = engine.estimate_model_input(input, "", &[]);
    if first_new.is_none() && max == 1 && engine.supports_summary_floor() {
        let mut minimum = input.range(1..input.len());
        minimum.insert(0, minimum_summary());
        if engine.estimate_model_input(&minimum, "", &[]) >= history_before {
            return Err(failure(
                ContextErrorKind::NoEligibleHistory,
                "the existing Summary is already minimal and no new eligible source remains",
            ));
        }
    }
    let mut search = Search {
        engine,
        input,
        prompt,
        tools,
        constraints,
        cancellation,
        boundaries: &boundaries,
        cache: BTreeMap::new(),
    };
    let eligible = boundaries.len();
    if eligible == 0 {
        return Err(failure(
            ContextErrorKind::NoEligibleHistory,
            "no complete eligible historical source remains before protected input or the continuation constraint",
        ));
    }
    let selected = if monotone && eligible > 0 {
        // Primary fit chooses retirement; summary capacity chooses chunks later.
        let upper = eligible;
        let lower = search
            .partition(|cost| cost.primary > limit || cost.history >= history_before)
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
            if cost.primary <= limit && cost.history < history_before {
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
        let best = search
            .cache
            .values()
            .map(|cost| cost.primary)
            .min()
            .unwrap_or(u64::MAX);
        // A synthetic price is proof only when the estimator certifies this
        // Summary shape as its lower bound. Monotone prefix deletion alone
        // does not establish that property for arbitrary estimators.
        let fit_witness = search.cache.values().any(|cost| cost.primary <= limit);
        let kind = if !engine.supports_summary_floor() {
            ContextErrorKind::SearchIncomplete
        } else if fit_witness {
            ContextErrorKind::NoProgress
        } else {
            ContextErrorKind::CannotFit
        };
        return Err(failure(
            kind,
            &format!(
                "no synthetic Summary candidate both satisfies primary fit and demonstrates historical reduction; {}",
                primary_fit_evidence(engine, input, prompt, tools, constraints, best, limit)
            ),
        ));
    };
    // A proof narrows search, never replaces authoritative whole-candidate pricing.
    let cost = search.cost(index).await?;
    if cost.primary > limit || cost.history >= history_before {
        return Err(failure(
            ContextErrorKind::Internal,
            "estimator violated its monotone compaction contract",
        ));
    }
    cancellation_point(cancellation).await?;
    Ok(boundaries[index])
}

/// Independent labeled estimates for diagnostics only. They are never added
/// together or used to infer feasibility for an arbitrary estimator.
#[allow(clippy::too_many_arguments)] // Each value names a separate frozen request authority.
pub(super) fn primary_fit_evidence(
    engine: &ContextEngine,
    input: &SelectedModelInput,
    prompt: &str,
    tools: &[ModelToolDefinition],
    constraints: &CompactionConstraints<'_>,
    complete: u64,
    limit: u64,
) -> String {
    let mut protected_ids: BTreeSet<_> =
        constraints.protected_context_ids.iter().cloned().collect();
    if let Some(fresh) = constraints.fresh_inbound {
        protected_ids.extend(fresh.message_ids().iter().cloned());
    }
    let mut protected = SelectedModelInput::default();
    for message in input {
        if message
            .canonical_id()
            .is_some_and(|id| protected_ids.contains(id))
        {
            protected.push(message.clone());
        }
    }
    for message in constraints.staged_request_context {
        protected.push(ModelInputMessage::Canonical(message.clone()));
    }
    let fixed = (!prompt.is_empty() || !tools.is_empty())
        .then(|| engine.estimate_model_input(&SelectedModelInput::default(), prompt, tools));
    let protected =
        (!protected.is_empty()).then(|| engine.estimate_model_input(&protected, "", &[]));
    let carryover = constraints.carryover.map(|value| {
        let mut selected = SelectedModelInput::default();
        selected.push(ModelInputMessage::RequestOnly(
            crate::model::RequestOnlyModelContext::UnresolvedOutputCarryover(value.clone()),
        ));
        engine.estimate_model_input(&selected, "", &[])
    });
    let describe = |value: Option<u64>| match value {
        None => "not present".to_owned(),
        Some(u64::MAX) => "unavailable".to_owned(),
        Some(value) => value.to_string(),
    };
    format!(
        "complete candidate estimate={}, primary input limit estimate={limit}; independent estimates: fixed System Prompt/Tools={}, protected Fresh Inbound/current Context={}, request-only carryover={}; these estimates are not additive or provider usage",
        describe(Some(complete)),
        describe(fixed),
        describe(protected),
        describe(carryover)
    )
}

#[derive(Clone, Copy)]
struct Cost {
    primary: u64,
    recent: u64,
    history: u64,
}
struct Search<'a, 'c> {
    engine: &'a ContextEngine,
    input: &'a SelectedModelInput,
    prompt: &'a str,
    tools: &'a [ModelToolDefinition],
    constraints: &'a CompactionConstraints<'c>,
    cancellation: &'a CancellationSignal,
    boundaries: &'a [usize],
    cache: BTreeMap<usize, Cost>,
}
impl Search<'_, '_> {
    async fn cost(&mut self, index: usize) -> Result<Cost, ContextError> {
        cancellation_point(self.cancellation).await?;
        if let Some(cost) = self.cache.get(&index) {
            return Ok(*cost);
        }
        let n = self.boundaries[index];
        let mut remaining = self.input.range(n..self.input.len());
        // Price the shortest rendered Summary shape in the primary accounting
        // path, including its message wrapper. It is a candidate witness, not
        // a lower bound on an arbitrary estimator's price for other Summaries.
        // Summary output tokens belong to another model and cannot be added
        // as primary input tokens.
        remaining.insert(0, minimum_summary());
        let history = self.engine.estimate_model_input(&remaining, "", &[]);
        cancellation_point(self.cancellation).await?;
        let planned = assemble_context_input(
            &remaining,
            self.constraints.staged_request_context,
            self.constraints.carryover,
            self.constraints.carryover_anchor,
        )
        .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
        let primary = self
            .engine
            .estimate_model_input(&planned, self.prompt, self.tools);
        cancellation_point(self.cancellation).await?;
        let recent =
            self.engine
                .estimate_model_input(&self.input.range(n..self.input.len()), "", &[]);
        cancellation_point(self.cancellation).await?;
        let cost = Cost {
            primary,
            recent,
            history,
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

pub(crate) fn minimum_summary() -> ModelInputMessage {
    let text = super::summary_content::render(serde_json::json!({
        "goal": "x",
        "constraints_and_preferences": ["x"],
        "progress": {"done": ["x"], "in_progress": ["x"], "blocked": ["x"]},
        "key_decisions": ["x"],
        "next_steps": ["x"],
        "critical_context": ["x"]
    }))
    .expect("the minimum Summary satisfies the internal domain contract")
    .trim_end()
    .to_owned();
    ModelInputMessage::Derived(crate::model::projected::DerivedModelMessage {
        message: crate::model::projected::ProjectedMessage::User {
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock { text },
            )],
            source: crate::message::types::UserSource::Runtime,
            kind: crate::model::projected::ProjectedUserKind::CompactionContext,
            timestamp: None,
        },
        sources: Vec::new(),
        checkpoint_sources: Vec::new(),
        transformations: vec![crate::model::projected::SemanticTransformation::Summary],
    })
}
#[cfg(test)]
thread_local! { static COPIED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_copied_bytes() -> u64 {
    COPIED_BYTES.with(|total| total.replace(0))
}

#[cfg(test)]
pub(super) fn record_materialized_message(message: &ModelInputMessage) {
    // Only newly cloned staged/carryover bodies; historical roots are shared.
    COPIED_BYTES.with(|total| {
        total.set(
            total.get().saturating_add(
                crate::context::resources::bounded_encoded_len(message, usize::MAX)
                    .expect("test accounting") as u64,
            ),
        );
    });
}
