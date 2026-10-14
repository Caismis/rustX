//! Bounded deterministic selection over compiled typed historical input.
use crate::context::engine::{CompactionBudgets, CompactionConstraints};
use crate::context::{ContextEngine, ContextError, ContextErrorKind};
use crate::model::{ModelInputMessage, input::assemble_model_input, projected::ModelMessageView};
use crate::runtime::CancellationSignal;
use crate::tools::types::ModelToolDefinition;
use std::collections::{BTreeMap, BTreeSet};

/// Non-additive estimators require complete candidate queries. Cap them independently
/// of history length; never use interpolation or additive prices as proof of fit.
pub(crate) const MAX_SUMMARY_CANDIDATES: usize = 64;
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

/// One structural/rendering pass and at most 64 exact full-candidate evaluations.
/// For longer histories, examine evenly spaced complete boundaries (including both
/// endpoints). Apply keep-recent preference among proven fits; fail closed if none
/// fits. No monotonic/additive estimator contract is assumed.
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
    let count = boundaries.len().min(MAX_SUMMARY_CANDIDATES);
    let mut preferred = None;
    let mut first_fit = None;
    for probe in 0..count {
        cancellation_point(cancellation).await?;
        let index = if count == 1 {
            0
        } else {
            probe * (boundaries.len() - 1) / (count - 1)
        };
        let (n, end) = boundaries[index];
        let summary = super::summarizer::summary_model_input(&transcript[..end]);
        let summary_tokens = engine.estimate_model_input(&summary.messages, "", &[]);
        cancellation_point(cancellation).await?;
        if summary_tokens > budgets.summary_input_limit {
            continue;
        }
        let planned = assemble_model_input(
            &input[n..],
            constraints.staged_request_context,
            constraints.carryover,
            constraints.carryover_anchor,
        )
        .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
        let primary_tokens = engine.estimate_model_input(&planned, prompt, tools);
        cancellation_point(cancellation).await?;
        if primary_tokens.saturating_add(u64::from(budgets.summary_output_budget)) <= limit {
            let recent = engine.estimate_model_input(&input[n..], "", &[]);
            first_fit.get_or_insert(n);
            if recent >= engine.config().keep_recent_tokens {
                preferred = Some(n);
            }
        }
    }
    cancellation_point(cancellation).await?;
    preferred.or(first_fit).ok_or_else(|| failure(
        if max == 0 { ContextErrorKind::NoProgress } else { ContextErrorKind::CannotFit },
        "no complete historical prefix examined satisfies summary, primary and core-retention budgets"))
}
