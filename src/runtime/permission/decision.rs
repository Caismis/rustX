//! Deterministic permission evaluation for one frozen prepared invocation.

use super::bash::BashReadonlyClass;
use super::matrix::{BuiltinPermissionAction, BuiltinToolKind, builtin_action_for};
use super::mode::ExecutionPermissionMode;
use super::resource::ResourcePermissionValue;

/// Bounded runtime-owned elevation reason extracted from tool metadata.
///
/// A nonblank reason requests human escalation. It is never approval evidence
/// and never reaches MCP/Python/Hosted/native business executors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevationReason(String);

impl ElevationReason {
    /// Creates a reason after trimming; blank input is rejected.
    pub fn new(raw: impl AsRef<str>) -> Option<Self> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_owned()))
        }
    }

    /// Borrowed reason text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Immutable permission facts for one prepared invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionFacts {
    /// Frozen execution-permission mode for this admitted execution.
    pub mode: ExecutionPermissionMode,
    /// Built-in tool/action family after path/Bash classification.
    pub kind: BuiltinToolKind,
    /// Effective resource override for the active mode, if any.
    pub resource: ResourcePermissionValue,
    /// Optional elevation reason from runtime metadata.
    pub elevation_reason: Option<ElevationReason>,
    /// Structured Bash analysis when the tool is Bash.
    pub bash: Option<BashReadonlyClass>,
}

/// Outcome kind of one permission evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecisionKind {
    /// Continue to the cancellation / tool-start frontier.
    Allow,
    /// Hard or mode-default denial; settle a typed denied `ToolResult`.
    Deny,
    /// Require Auxiliary permission review before continuing.
    Review,
    /// Require exact human approval for this frozen invocation.
    Ask,
}

/// One permission decision with a bounded diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDecision {
    /// Decision kind.
    pub kind: PermissionDecisionKind,
    /// Bounded explanation for the model/UI/audit.
    pub reason: String,
}

impl PermissionDecision {
    fn allow() -> Self {
        Self {
            kind: PermissionDecisionKind::Allow,
            reason: String::new(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            kind: PermissionDecisionKind::Deny,
            reason: reason.into(),
        }
    }

    fn ask(reason: impl Into<String>) -> Self {
        Self {
            kind: PermissionDecisionKind::Ask,
            reason: reason.into(),
        }
    }

    fn review(reason: impl Into<String>) -> Self {
        Self {
            kind: PermissionDecisionKind::Review,
            reason: reason.into(),
        }
    }
}

/// Evaluates one frozen permission decision.
///
/// Precedence: effective resource `deny` / `allow` / `ask`, then built-in
/// matrix action, then elevation reason. Explicit deny is never elevatable.
#[must_use]
pub fn evaluate_permission(facts: &PermissionFacts) -> PermissionDecision {
    match facts.resource {
        ResourcePermissionValue::Deny => {
            return PermissionDecision::deny(format!(
                "tool is denied by an effective resource permission in {} mode; elevation cannot override deny",
                facts.mode
            ));
        }
        ResourcePermissionValue::Allow => return PermissionDecision::allow(),
        ResourcePermissionValue::Ask => {
            return PermissionDecision::ask(format!(
                "tool requires human approval under an effective resource ask permission in {} mode",
                facts.mode
            ));
        }
        ResourcePermissionValue::Default => {}
    }

    match builtin_action_for(facts.kind, facts.mode) {
        BuiltinPermissionAction::Allow => PermissionDecision::allow(),
        BuiltinPermissionAction::Elevate => match &facts.elevation_reason {
            Some(reason) => PermissionDecision::ask(format!(
                "{} mode refuses this default operation until a human approves the elevation request: {}",
                facts.mode,
                reason.as_str()
            )),
            None => PermissionDecision::deny(plan_or_mode_denial(facts)),
        },
        BuiltinPermissionAction::Review => PermissionDecision::review(format!(
            "{} mode sends this default operation to the Auxiliary permission reviewer",
            facts.mode
        )),
    }
}

fn plan_or_mode_denial(facts: &PermissionFacts) -> String {
    match facts.kind {
        BuiltinToolKind::OtherFileMutation => format!(
            "{} mode allows Write/Edit only for workspace-root PLAN.md without elevation; provide elevation_reason to request human approval",
            facts.mode
        ),
        BuiltinToolKind::BashOther => match facts.bash {
            Some(BashReadonlyClass::Readonly) => unreachable!("readonly Bash uses Allow"),
            Some(BashReadonlyClass::Unproven { .. }) | None => format!(
                "{} mode allows only Bash commands proven read-only by the native checker; provide elevation_reason to request human approval",
                facts.mode
            ),
        },
        _ => format!(
            "{} mode refuses this default operation without elevation_reason; provide elevation_reason to request human approval",
            facts.mode
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::permission::BashReadonlyClass;

    fn facts(
        mode: ExecutionPermissionMode,
        kind: BuiltinToolKind,
        resource: ResourcePermissionValue,
        elevation: Option<&str>,
    ) -> PermissionFacts {
        PermissionFacts {
            mode,
            kind,
            resource,
            elevation_reason: elevation.and_then(ElevationReason::new),
            bash: None,
        }
    }

    #[test]
    fn hard_deny_survives_full_access_and_elevation() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::FullAccess,
            BuiltinToolKind::NativeRead,
            ResourcePermissionValue::Deny,
            Some("please"),
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Deny);
        assert!(decision.reason.contains("cannot override deny"));
    }

    #[test]
    fn plan_write_without_reason_is_denied() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::Plan,
            BuiltinToolKind::OtherFileMutation,
            ResourcePermissionValue::Default,
            None,
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Deny);
        assert!(decision.reason.contains("PLAN.md"));
    }

    #[test]
    fn plan_write_with_reason_asks() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::Plan,
            BuiltinToolKind::OtherFileMutation,
            ResourcePermissionValue::Default,
            Some("need to update config"),
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Ask);
    }

    #[test]
    fn automatic_default_reviews() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::Automatic,
            BuiltinToolKind::OtherFileMutation,
            ResourcePermissionValue::Default,
            None,
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Review);
    }

    #[test]
    fn full_access_allows_defaults() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::FullAccess,
            BuiltinToolKind::BashOther,
            ResourcePermissionValue::Default,
            None,
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Allow);
    }

    #[test]
    fn blank_elevation_reason_is_ignored() {
        assert!(ElevationReason::new("   ").is_none());
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::Plan,
            BuiltinToolKind::GoalCreate,
            ResourcePermissionValue::Default,
            Some("  "),
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Deny);
    }

    #[test]
    fn resource_ask_requires_human_without_review() {
        let decision = evaluate_permission(&facts(
            ExecutionPermissionMode::Automatic,
            BuiltinToolKind::NativeRead,
            ResourcePermissionValue::Ask,
            None,
        ));
        assert_eq!(decision.kind, PermissionDecisionKind::Ask);
    }

    #[test]
    fn readonly_bash_is_allowed_in_plan() {
        let mut f = facts(
            ExecutionPermissionMode::Plan,
            BuiltinToolKind::BashReadonly,
            ResourcePermissionValue::Default,
            None,
        );
        f.bash = Some(BashReadonlyClass::Readonly);
        assert_eq!(evaluate_permission(&f).kind, PermissionDecisionKind::Allow);
    }
}
