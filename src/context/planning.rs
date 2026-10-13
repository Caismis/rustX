//! Deterministic summary selection over compiled typed historical input.
use crate::context::engine::{CompactionBudgets, CompactionConstraints};
use crate::context::{ContextEngine, ContextError, ContextErrorKind};
use crate::model::{ModelInputMessage, input::assemble_model_input, projected::ModelMessageView};
use crate::tools::types::ModelToolDefinition;
use std::collections::BTreeMap;

fn failure(kind: ContextErrorKind, text: &str) -> ContextError {
    ContextError::new(kind, text)
}

/// Choose a whole-message prefix, with complete Tool batches and all core anchors retained.
#[allow(clippy::too_many_lines)] // One complete prefix fold over Tool batches, core anchors and both model budgets.
pub(crate) fn summary_prefix(
    engine: &ContextEngine,
    input: &[ModelInputMessage],
    prompt: &str,
    tools: &[ModelToolDefinition],
    budgets: CompactionBudgets,
    constraints: &CompactionConstraints<'_>,
) -> Result<usize, ContextError> {
    let mut limit = engine.soft_input_limit(budgets.primary_output_budget)?;
    if let Some(correction) = constraints.estimate_correction {
        limit = correction.apply(limit);
    }
    let min = match constraints.must_cover_through {
        Some(id) => input
            .iter()
            .position(|message| message.canonical_id() == Some(id))
            .map(|index| index + 1)
            .ok_or_else(|| {
                failure(
                    ContextErrorKind::NoProgress,
                    "required continuation anchor is absent from historical projection",
                )
            })?,
        None => 1,
    };
    let fresh = constraints
        .fresh_inbound
        .map(super::super::runtime::inbound::FreshInboundTurn::message_ids);
    if fresh.is_some_and(|ids| {
        ids.iter()
            .any(|id| !input.iter().any(|m| m.canonical_id() == Some(id)))
    }) {
        return Err(failure(
            ContextErrorKind::MalformedHistory,
            "Fresh Inbound requires its Original fact; a Derived source mention cannot supply it",
        ));
    }
    let max = input
        .iter()
        .position(|message| {
            message.canonical_id().is_some_and(|id| {
                constraints.protected_context_ids.contains(id)
                    || fresh.is_some_and(|ids| ids.contains(id))
                    || matches!(
                        constraints.carryover_anchor,
                        Some(crate::model::RequestOnlyInsertionAnchor::BeforeMessage(anchor))
                            if anchor == id
                    )
            })
        })
        .unwrap_or(input.len());
    let mut pending = BTreeMap::<String, usize>::new();
    let mut fits = Vec::new();
    for (i, message) in input.iter().enumerate().take(max) {
        match message.view() {
            ModelMessageView::Assistant(content) => {
                for block in content {
                    if let crate::message::types::AssistantContentBlock::ToolCall(call) = block {
                        *pending.entry(call.id.to_string()).or_default() += 1;
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
                *count = count.checked_sub(1).ok_or_else(|| {
                    failure(
                        ContextErrorKind::MalformedHistory,
                        "duplicate projected ToolResult",
                    )
                })?;
            }
            _ => {}
        }
        let n = i + 1;
        if n < min || pending.values().any(|count| *count != 0) {
            continue;
        }
        let summary = super::summarizer::SummaryRequest {
            retired: input[..n].to_vec(),
        }
        .model_input();
        if engine.estimate_model_input(&summary.messages, "", &[]) > budgets.summary_input_limit {
            continue;
        }
        let planned = assemble_model_input(
            &input[n..],
            constraints.staged_request_context,
            constraints.carryover,
            constraints.carryover_anchor,
        )
        .map_err(|e| failure(ContextErrorKind::MalformedHistory, &e))?;
        if engine
            .estimate_model_input(&planned, prompt, tools)
            .saturating_add(u64::from(budgets.summary_output_budget))
            <= limit
        {
            let recent = engine.estimate_model_input(&input[n..], "", &[]);
            fits.push((n, recent));
        }
    }
    fits.iter()
        .filter(|(_, recent)| *recent >= engine.config().keep_recent_tokens)
        .map(|(n, _)| *n)
        .max()
        .or_else(|| fits.first().map(|(n, _)| *n))
        .ok_or_else(|| {
            failure(
                if max == 0 {
                    ContextErrorKind::NoProgress
                } else {
                    ContextErrorKind::CannotFit
                },
                "no complete historical prefix satisfies summary, primary and core-retention budgets",
            )
        })
}
