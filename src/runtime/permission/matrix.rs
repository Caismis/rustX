//! Built-in permission matrix for Plan / Automatic / Full Access.

use super::mode::ExecutionPermissionMode;

/// Built-in mode-default action before resource overrides and elevation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinPermissionAction {
    /// Deterministic allow without Auxiliary review or human Ask.
    Allow,
    /// Mode-default refusal that becomes Ask when `elevation_reason` is present.
    Elevate,
    /// Automatic-mode Auxiliary permission review.
    Review,
}

/// Canonical tool/action families covered by the built-in matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinToolKind {
    /// `read`, `read_image`, `glob`, `grep`.
    NativeRead,
    /// `write` / `edit` targeting exactly workspace-root `PLAN.md`.
    PlanFileMutation,
    /// Other `write` / `edit`.
    OtherFileMutation,
    /// Bash proven read-only by the native checker.
    BashReadonly,
    /// Other Bash.
    BashOther,
    /// `ask_user`, `present`.
    Interaction,
    /// Todo create/update/list/get/delete/clear.
    Todo,
    /// `get_goal`.
    GoalRead,
    /// `create_goal`.
    GoalCreate,
    /// `update_goal` Complete / Blocked.
    GoalStatusUpdate,
    /// `get_schedule`, `list_schedules`.
    ScheduleRead,
    /// `schedule_command` mutations.
    ScheduleMutation,
    /// `job_list`, `job_status`, `job_wait`.
    JobObserve,
    /// `job_cancel`, `interrupt_agent`.
    JobControl,
    /// `list_agents`, `wait_agent`.
    AgentObserve,
    /// `subagent`, `send_message`, Workflow invocation, MCP / Managed Python /
    /// Hosted calls, and MCP resource wrappers — resource policy with these
    /// mode defaults when the resource omits permissions.
    ExternalOrDelegated,
}

/// Returns the built-in action for one tool family in one mode.
#[must_use]
pub const fn builtin_action_for(
    kind: BuiltinToolKind,
    mode: ExecutionPermissionMode,
) -> BuiltinPermissionAction {
    use BuiltinPermissionAction::{Allow, Elevate, Review};
    use BuiltinToolKind::*;
    use ExecutionPermissionMode::{Automatic, FullAccess, Plan};

    match (kind, mode) {
        (
            NativeRead | PlanFileMutation | BashReadonly | Interaction | Todo | GoalRead
            | GoalStatusUpdate | ScheduleRead | JobObserve | AgentObserve,
            _,
        ) => Allow,
        (OtherFileMutation | BashOther | GoalCreate | ScheduleMutation | JobControl, Plan) => {
            Elevate
        }
        (
            OtherFileMutation | BashOther | GoalCreate | ScheduleMutation | JobControl,
            Automatic,
        ) => Review,
        (
            OtherFileMutation | BashOther | GoalCreate | ScheduleMutation | JobControl,
            FullAccess,
        ) => Allow,
        (ExternalOrDelegated, Plan) => Elevate,
        (ExternalOrDelegated, Automatic) => Review,
        (ExternalOrDelegated, FullAccess) => Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_always_allowed() {
        for mode in [
            ExecutionPermissionMode::Plan,
            ExecutionPermissionMode::Automatic,
            ExecutionPermissionMode::FullAccess,
        ] {
            assert_eq!(
                builtin_action_for(BuiltinToolKind::NativeRead, mode),
                BuiltinPermissionAction::Allow
            );
        }
    }

    #[test]
    fn other_writes_follow_mode_defaults() {
        assert_eq!(
            builtin_action_for(
                BuiltinToolKind::OtherFileMutation,
                ExecutionPermissionMode::Plan
            ),
            BuiltinPermissionAction::Elevate
        );
        assert_eq!(
            builtin_action_for(
                BuiltinToolKind::OtherFileMutation,
                ExecutionPermissionMode::Automatic
            ),
            BuiltinPermissionAction::Review
        );
        assert_eq!(
            builtin_action_for(
                BuiltinToolKind::OtherFileMutation,
                ExecutionPermissionMode::FullAccess
            ),
            BuiltinPermissionAction::Allow
        );
    }

    #[test]
    fn plan_file_mutation_is_always_allowed() {
        for mode in [
            ExecutionPermissionMode::Plan,
            ExecutionPermissionMode::Automatic,
            ExecutionPermissionMode::FullAccess,
        ] {
            assert_eq!(
                builtin_action_for(BuiltinToolKind::PlanFileMutation, mode),
                BuiltinPermissionAction::Allow
            );
        }
    }
}
