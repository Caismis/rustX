//! One deterministic dual-model compaction policy, resolved from admitted models.
use super::{CompactionBudgets, ContextConfig, ContextError, ContextErrorKind};

/// The resolved policy belongs to Context; it is not authored configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompactionPolicy {
    pub primary: ContextConfig,
    pub budgets: CompactionBudgets,
    pub minimum_summary_output: u32,
    pub maximum_summary_output: u32,
}

/// Visible JSON must have room in addition to an explicit reasoning budget.
const VISIBLE_SUMMARY_OUTPUT: u32 = 1_024;

/// Five percent, rounded upward. The cap keeps very large advertised windows
/// from reserving an unbounded amount of otherwise useful primary context.
pub(crate) fn automatic_margin(window: u64) -> u64 {
    window.div_ceil(20).clamp(1, 65_536)
}

impl CompactionPolicy {
    pub(crate) fn resolve(
        primary_window: u64,
        primary_output: u32,
        summary_window: u64,
        summary_output_capability: u32,
    ) -> Result<Self, ContextError> {
        Self::resolve_with_reasoning(
            primary_window,
            primary_output,
            summary_window,
            summary_output_capability,
            None,
        )
    }

    pub(crate) fn for_models(
        model: &crate::model::session::AttemptModelSnapshot,
    ) -> Result<Self, ContextError> {
        let summary = model.summary_invocation();
        let reasoning = summary
            .reasoning_output_minimum()
            .map_err(|error| ContextError::new(ContextErrorKind::InvalidConfiguration, error))?;
        Self::resolve_with_reasoning(
            model.primary().context_window(),
            model.primary().max_output_tokens(),
            summary.context_window(),
            summary.output_capability(),
            reasoning,
        )
    }

    pub(crate) fn resolve_with_reasoning(
        primary_window: u64,
        primary_output: u32,
        summary_window: u64,
        summary_output_capability: u32,
        reasoning_budget: Option<u32>,
    ) -> Result<Self, ContextError> {
        let invalid = |text| ContextError::new(ContextErrorKind::InvalidConfiguration, text);
        if primary_window == 0 || summary_window == 0 || summary_output_capability == 0 {
            return Err(invalid(
                "primary and summary capacities must be known and positive",
            ));
        }
        let reserve = automatic_margin(primary_window);
        let hard = primary_window
            .checked_sub(reserve)
            .and_then(|n| n.checked_sub(u64::from(primary_output)))
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                invalid("primary window cannot accommodate output and automatic safety margin")
            })?;
        let keep_recent = u64::try_from((u128::from(hard) * 15).div_ceil(100))
            .expect("a fraction of u64 fits u64")
            .min(20_000);
        // Reserve total summary output, including possible reasoning. Profile
        // controls remain frozen; this is only a valid per-request output cap.
        let automatic_output = u64::from(summary_output_capability)
            .min((summary_window / 4).max(1))
            .min((hard / 4).max(1));
        let minimum_output = match reasoning_budget {
            Some(tokens) => tokens.checked_add(VISIBLE_SUMMARY_OUTPUT).ok_or_else(|| {
                invalid("summary reasoning budget leaves no representable visible JSON allowance")
            })?,
            None => 1,
        };
        let available_output = summary_window
            .checked_sub(automatic_margin(summary_window))
            .and_then(|n| n.checked_sub(1))
            .unwrap_or(0)
            .min(u64::from(summary_output_capability));
        if u64::from(minimum_output) > available_output {
            return Err(invalid(
                "summary reasoning budget and visible JSON allowance cannot fit the admitted model capability and window",
            ));
        }
        let summary_output = automatic_output.max(u64::from(minimum_output));
        let summary_input = summary_window
            .checked_sub(automatic_margin(summary_window))
            .and_then(|n| n.checked_sub(summary_output))
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                invalid("summary window cannot accommodate output and automatic safety margin")
            })?;
        let automatic_ceiling = u64::from(summary_output_capability)
            .min(summary_window / 2)
            .max(summary_output);
        let reasoning_headroom = if reasoning_budget.is_some() {
            u64::from(minimum_output).saturating_add(u64::from(VISIBLE_SUMMARY_OUTPUT))
        } else {
            0
        };
        let maximum_output = automatic_ceiling
            .max(reasoning_headroom)
            .min(available_output);
        Ok(Self {
            primary: ContextConfig {
                context_window_tokens: primary_window,
                reserve_tokens: reserve,
                keep_recent_tokens: keep_recent,
            },
            budgets: CompactionBudgets::new(
                primary_output,
                u32::try_from(summary_output).expect("bounded by admitted output capability"),
                summary_input,
            ),
            minimum_summary_output: minimum_output,
            maximum_summary_output: u32::try_from(maximum_output)
                .expect("bounded by admitted output capability"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smaller_summary_model_does_not_shrink_primary_admission() {
        let p = CompactionPolicy::resolve(128_000, 8_000, 16_000, 8_000).unwrap();
        assert_eq!(p.primary.admissible_input_limit(8_000).unwrap(), 113_600);
        assert_eq!(p.primary.keep_recent_tokens, 17_040);
        assert_eq!(p.budgets.summary_output_budget, 4_000);
        assert_eq!(p.budgets.summary_input_limit, 11_200);
    }

    #[test]
    fn thinking_model_reserves_total_output_without_changing_primary_output() {
        let p = CompactionPolicy::resolve(256_000, 16_000, 128_000, 64_000).unwrap();
        assert_eq!(p.primary.admissible_input_limit(16_000).unwrap(), 227_200);
        assert_eq!(p.primary.keep_recent_tokens, 20_000);
        assert_eq!(p.budgets.primary_output_budget, 16_000);
        assert_eq!(p.budgets.summary_output_budget, 32_000);
        assert_eq!(p.budgets.summary_input_limit, 89_600);
    }

    #[test]
    fn explicit_reasoning_budget_has_visible_output_and_can_be_impossible() {
        let p =
            CompactionPolicy::resolve_with_reasoning(128_000, 8_000, 16_000, 8_000, Some(5_000))
                .unwrap();
        assert_eq!(p.budgets.primary_output_budget, 8_000);
        assert_eq!(p.minimum_summary_output, 6_024);
        assert_eq!(p.budgets.summary_output_budget, 6_024);
        assert_eq!(p.budgets.summary_input_limit, 9_176);
        assert!(p.maximum_summary_output >= p.budgets.summary_output_budget);
        assert!(
            CompactionPolicy::resolve_with_reasoning(128_000, 8_000, 16_000, 8_000, Some(7_500),)
                .is_err()
        );
        assert!(CompactionPolicy::resolve_with_reasoning(
            128_000, 8_000, 16_000, 8_000, Some(u32::MAX),
        )
        .is_err());
    }

    #[test]
    fn tiny_unknown_and_impossible_windows_are_checked() {
        assert!(CompactionPolicy::resolve(0, 1, 16, 1).is_err());
        assert!(CompactionPolicy::resolve(16, 1, 0, 1).is_err());
        assert!(CompactionPolicy::resolve(16, 16, 16, 1).is_err());
        assert!(CompactionPolicy::resolve(16, 1, 1, 1).is_err());
        let p = CompactionPolicy::resolve(16, 1, 16, 1).unwrap();
        assert_eq!(p.primary.reserve_tokens, 1);
        assert_eq!(p.primary.admissible_input_limit(1).unwrap(), 14);
        assert_eq!(p.budgets.summary_output_budget, 1);
    }

    #[test]
    fn recovery_output_ceiling_uses_summary_capacity_instead_of_primary_input() {
        let p = CompactionPolicy::resolve(16_000, 1_000, 128_000, 64_000).unwrap();
        assert_eq!(p.budgets.summary_output_budget, 3_550);
        assert_eq!(p.maximum_summary_output, 64_000);
        let same_model = CompactionPolicy::resolve(16_000, 1_000, 16_000, 8_000).unwrap();
        assert_eq!(same_model.budgets.summary_output_budget, 3_550);
        assert_eq!(same_model.maximum_summary_output, 8_000);
    }

    #[test]
    fn integer_arithmetic_is_bounded_at_u64_capacity() {
        let p = CompactionPolicy::resolve(u64::MAX, 1, u64::MAX, u32::MAX).unwrap();
        assert_eq!(p.primary.reserve_tokens, 65_536);
        assert_eq!(p.primary.keep_recent_tokens, 20_000);
        assert_eq!(p.budgets.summary_output_budget, u32::MAX);
    }
}
