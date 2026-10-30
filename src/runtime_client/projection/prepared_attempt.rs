//! Only the latest immutable attempt owns these finite item preparations.
//! Admission is replayed against the current shared budget; source references
//! and deferred evidence are always assembled from the current seed.
use super::{ForegroundToolExecution, ForegroundToolState, InFlightBlock, RuntimeClientAttempt};
use crate::model::session::{AttemptModelView, SummaryModelView};
use crate::runtime_client::checkpoint::{CHECKPOINT_PAGE_ITEMS, PreparedCapture};
use crate::runtime_client::settings::AdmittedSettings;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

type Item<T> = Arc<OnceLock<PreparedCapture<T>>>;

#[derive(Default)]
pub(super) struct Items {
    pub model: Item<Option<Box<AttemptModelView>>>,
    pub settings: Item<Option<AdmittedSettings>>,
    pub blocks: Vec<Item<Option<InFlightBlock>>>,
    // Reserve only: deferred source selection and the public Tool copy must
    // still use the current row, rather than a cached decorated result.
    pub tools: Vec<Item<bool>>,
}

#[derive(Default)]
struct Latest {
    source: Arc<Option<RuntimeClientAttempt>>,
    items: Arc<Items>,
}

#[derive(Clone, Default)]
pub(super) struct PreparedAttemptItems(Arc<Mutex<Latest>>);

impl PreparedAttemptItems {
    pub(super) fn select(&self, source: &Arc<Option<RuntimeClientAttempt>>) -> Arc<Items> {
        let mut latest = self.0.lock().expect("immutable attempt preparation");
        if Arc::ptr_eq(source, &latest.source) {
            return Arc::clone(&latest.items);
        }
        let before = latest.source.as_ref().as_ref();
        let after = source.as_ref().as_ref();
        let old_blocks = before.and_then(|a| a.in_flight.as_ref()).map(|f| &f.blocks);
        let blocks = after.and_then(|a| a.in_flight.as_ref()).map(|f| &f.blocks);
        let mut items = Items::default();
        if same_model(
            before.and_then(|a| a.model.as_deref()),
            after.and_then(|a| a.model.as_deref()),
        ) {
            items.model = Arc::clone(&latest.items.model);
        }
        if before.and_then(|a| a.execution_settings.as_ref())
            == after.and_then(|a| a.execution_settings.as_ref())
        {
            items.settings = Arc::clone(&latest.items.settings);
        }
        if let Some(blocks) = blocks {
            items.blocks = blocks
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(i, block)| {
                    // InFlightBlock contains strings, identities and integer
                    // coordinates only: native equality is exact wire equality.
                    reused(
                        latest.items.blocks.get(i),
                        old_blocks.and_then(|v| v.get(i)) == Some(block),
                    )
                })
                .collect();
        }
        if let Some(after) = after {
            items.tools = after
                .foreground
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(i, tool)| {
                    let same = before
                        .and_then(|a| a.foreground.get(i))
                        .is_some_and(|old| same_tool(old, tool));
                    reused(latest.items.tools.get(i), same)
                })
                .collect();
        }
        let items = Arc::new(items);
        *latest = Latest {
            source: Arc::clone(source),
            items: Arc::clone(&items),
        };
        items
    }
}

fn reused<T>(previous: Option<&Item<T>>, same: bool) -> Item<T> {
    previous
        .filter(|_| same)
        .map_or_else(Arc::default, Arc::clone)
}

fn same_model(left: Option<&AttemptModelView>, right: Option<&AttemptModelView>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            same_invocation(&left.primary, &right.primary)
                && match (&left.summary, &right.summary) {
                    (SummaryModelView::Session, SummaryModelView::Session) => true,
                    (SummaryModelView::Explicit(left), SummaryModelView::Explicit(right)) => {
                        same_invocation(left, right)
                    }
                    _ => false,
                }
        }
        _ => false,
    }
}

fn same_invocation(
    left: &crate::model::invocation::ModelInvocationView,
    right: &crate::model::invocation::ModelInvocationView,
) -> bool {
    // Exhaustive destructuring makes additions to this DTO require a deliberate
    // equality decision. Only request_params contains ordered maps/floats.
    let crate::model::invocation::ModelInvocationView {
        model,
        protocol,
        context_window,
        model_max_output_tokens,
        max_output_tokens,
        profile,
        reasoning_enabled,
        request_params,
        capabilities,
        declared_capabilities,
    } = left;
    model == &right.model
        && protocol == &right.protocol
        && context_window == &right.context_window
        && model_max_output_tokens == &right.model_max_output_tokens
        && max_output_tokens == &right.max_output_tokens
        && profile == &right.profile
        && reasoning_enabled == &right.reasoning_enabled
        && capabilities == &right.capabilities
        && declared_capabilities == &right.declared_capabilities
        && exact_map(request_params, &right.request_params, 0, &mut 16_384)
}

fn exact_map(
    left: &serde_json::Map<String, serde_json::Value>,
    right: &serde_json::Map<String, serde_json::Value>,
    depth: usize,
    remaining: &mut usize,
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((lk, lv), (rk, rv))| lk == rk && exact_json(lv, rv, depth + 1, remaining))
}

fn exact_json(
    left: &serde_json::Value,
    right: &serde_json::Value,
    depth: usize,
    remaining: &mut usize,
) -> bool {
    if depth > 128 || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match (left, right) {
        (Value::Number(left), Value::Number(right)) if left.is_f64() && right.is_f64() => {
            left.as_f64().map(f64::to_bits) == right.as_f64().map(f64::to_bits)
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(l, r)| exact_json(l, r, depth + 1, remaining))
        }
        (Value::Object(left), Value::Object(right)) => exact_map(left, right, depth, remaining),
        _ => left == right,
    }
}

fn same_tool(left: &ForegroundToolExecution, right: &ForegroundToolExecution) -> bool {
    fn result_json(
        left: &crate::tools::types::ToolExecutionResult,
        right: &crate::tools::types::ToolExecutionResult,
    ) -> bool {
        use crate::tools::types::ToolResultContent;
        let mut remaining = 16_384;
        left.content.len() == right.content.len()
            && left
                .content
                .iter()
                .zip(&right.content)
                .all(|(l, r)| match (l, r) {
                    (
                        ToolResultContent::Json { value: l },
                        ToolResultContent::Json { value: r },
                    ) => exact_json(l, r, 0, &mut remaining),
                    (ToolResultContent::Json { .. }, _) | (_, ToolResultContent::Json { .. }) => {
                        false
                    }
                    _ => true,
                })
    }
    let committed = match (&left.committed_result, &right.committed_result) {
        (Some(left), Some(right)) => result_json(left, right),
        (None, None) => true,
        _ => false,
    };
    let state = match (&left.state, &right.state) {
        (
            ForegroundToolState::Running { progress: l, .. },
            ForegroundToolState::Running { progress: r, .. },
        ) => match (l, r) {
            (Some(l), Some(r)) => {
                l.completed.map(f64::to_bits) == r.completed.map(f64::to_bits)
                    && l.total.map(f64::to_bits) == r.total.map(f64::to_bits)
            }
            (None, None) => true,
            _ => false,
        },
        (
            ForegroundToolState::Settled { result: l, .. },
            ForegroundToolState::Settled { result: r, .. },
        ) => result_json(l, r),
        (ForegroundToolState::Assembled { .. }, ForegroundToolState::Assembled { .. }) => true,
        _ => false,
    };
    // Reject pathological JSON depth before PartialEq descends into it. Native
    // equality then covers every remaining field, including authoritative refs.
    committed && state && left == right
}

#[cfg(test)]
mod tests;
