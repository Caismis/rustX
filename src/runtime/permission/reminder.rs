//! Sparse permission-mode reminders (Issue #544 §10).
//!
//! Emitted only on first context, effective mode change, or successful
//! compaction activation. Eligibility is consumed at successful native
//! model-request-start / context commit — not earlier.

use crate::runtime::types::ApprovalMode;

/// Bounded reminder text for the frozen execution-permission mode.
#[must_use]
pub fn permission_reminder_text(mode: ApprovalMode) -> String {
    match mode {
        ApprovalMode::Plan => {
            "Execution permission mode: plan. All selected tools remain visible. \
Native reads and approved bookkeeping are allowed; Write/Edit may change only \
workspace-root PLAN.md; Bash must be proven read-only. Other defaults require \
elevation_reason for human approval."
                .to_owned()
        }
        ApprovalMode::Automatic => {
            "Execution permission mode: automatic. All selected tools remain visible. \
Plan fast paths stay allowed; other defaults are reviewed by the shared Auxiliary \
model and may be refused. Use elevation_reason to request human approval after a \
review denial or reviewer failure."
                .to_owned()
        }
        ApprovalMode::FullAccess => {
            "Execution permission mode: full_access. All selected tools remain visible. \
Defaults are allowed without Auxiliary or ordinary human review. Effective explicit \
deny and ask resource settings still apply."
                .to_owned()
        }
    }
}

/// Attempt-local reminder eligibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReminderState {
    /// Frozen mode for this admitted execution.
    pub mode: ApprovalMode,
    /// Whether the next successful request-start should include a reminder.
    pub pending: bool,
}

impl PermissionReminderState {
    /// First request of a new admitted execution needs a reminder.
    #[must_use]
    pub const fn for_admission(mode: ApprovalMode) -> Self {
        Self {
            mode,
            pending: true,
        }
    }

    /// Marks a successful compaction activation for the next request.
    pub fn mark_after_compaction(&mut self) {
        self.pending = true;
    }

    /// Text to inject when pending; does not consume eligibility.
    #[must_use]
    pub fn pending_text(&self) -> Option<String> {
        self.pending.then(|| permission_reminder_text(self.mode))
    }

    /// Consumes eligibility after successful request-start commit.
    pub fn consume_after_start_commit(&mut self) {
        self.pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_is_pending_until_consumed() {
        let mut state = PermissionReminderState::for_admission(ApprovalMode::Plan);
        assert!(state.pending_text().unwrap().contains("plan"));
        state.consume_after_start_commit();
        assert!(state.pending_text().is_none());
        state.mark_after_compaction();
        assert!(state.pending_text().unwrap().contains("plan"));
    }
}
