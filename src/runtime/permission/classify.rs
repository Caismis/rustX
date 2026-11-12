//! Classify a prepared tool invocation into a built-in matrix family.

use std::path::Path;

use super::bash::{BashReadonlyClass, analyze_bash_readonly};
use super::matrix::BuiltinToolKind;
use crate::tools::types::ToolOrigin;

/// Extra facts needed to classify Write/Edit and Bash invocations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationHints<'a> {
    /// Workspace root used for PLAN.md authorization.
    pub workspace_root: Option<&'a Path>,
    /// Absolute or workspace-relative mutation target for write/edit.
    pub mutation_target: Option<&'a Path>,
    /// Bash command text when the tool is Bash.
    pub bash_command: Option<&'a str>,
}

/// Classifies one tool into the built-in permission matrix family.
#[must_use]
pub fn classify_tool_kind(
    tool_name: &str,
    origin: &ToolOrigin,
    hints: &ClassificationHints<'_>,
) -> (BuiltinToolKind, Option<BashReadonlyClass>) {
    match origin {
        ToolOrigin::Mcp { .. } | ToolOrigin::ManagedPython { .. } | ToolOrigin::Host { .. } => {
            return (BuiltinToolKind::ExternalOrDelegated, None);
        }
        ToolOrigin::Builtin => {}
    }

    match tool_name {
        "read" | "read_image" | "glob" | "grep" => (BuiltinToolKind::NativeRead, None),
        "write" | "edit" => {
            if is_workspace_plan_file(hints.workspace_root, hints.mutation_target) {
                (BuiltinToolKind::PlanFileMutation, None)
            } else {
                (BuiltinToolKind::OtherFileMutation, None)
            }
        }
        "bash" => {
            let class = hints.bash_command.map_or(
                BashReadonlyClass::Unproven {
                    reason: "missing Bash command".to_owned(),
                },
                |command| analyze_bash_readonly(command).class,
            );
            let kind = match &class {
                BashReadonlyClass::Readonly => BuiltinToolKind::BashReadonly,
                BashReadonlyClass::Unproven { .. } => BuiltinToolKind::BashOther,
            };
            (kind, Some(class))
        }
        "ask_user" | "present" => (BuiltinToolKind::Interaction, None),
        "todo" | "todo_create" | "todo_update" | "todo_list" | "todo_get" | "todo_delete"
        | "todo_clear" => (BuiltinToolKind::Todo, None),
        "get_goal" => (BuiltinToolKind::GoalRead, None),
        "create_goal" => (BuiltinToolKind::GoalCreate, None),
        "update_goal" => (BuiltinToolKind::GoalStatusUpdate, None),
        "get_schedule" | "list_schedules" => (BuiltinToolKind::ScheduleRead, None),
        "schedule_command" => (BuiltinToolKind::ScheduleMutation, None),
        "job_list" | "job_status" | "job_wait" => (BuiltinToolKind::JobObserve, None),
        "job_cancel" | "interrupt_agent" => (BuiltinToolKind::JobControl, None),
        "list_agents" | "wait_agent" => (BuiltinToolKind::AgentObserve, None),
        _ => (BuiltinToolKind::ExternalOrDelegated, None),
    }
}

/// True only when the mutation target is exactly `<workspace>/PLAN.md`.
///
/// `mutation_target` must already be the Write/Edit effective path (symlink
/// ancestors and dangling final links resolved). A dangling `PLAN.md` symlink
/// that resolves outside the workspace therefore never matches.
#[must_use]
pub fn is_workspace_plan_file(
    workspace_root: Option<&Path>,
    mutation_target: Option<&Path>,
) -> bool {
    let (Some(root), Some(target)) = (workspace_root, mutation_target) else {
        return false;
    };
    let Ok(canonical_root) = root.canonicalize().or_else(|_| {
        crate::tools::native::support::resolve_effective_path(root).map_err(|_| ())
    }) else {
        return false;
    };
    let expected = canonical_root.join("PLAN.md");
    // The expected plan path is a real workspace file name, not a symlink we
    // follow: PLAN.md itself must be the mutation target, never a link hop.
    target == expected
        && target.starts_with(&canonical_root)
        && target.file_name().is_some_and(|name| name == "PLAN.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_workspace_plan_file_counts() {
        let root = tempfile::tempdir().expect("workspace");
        let plan = root.path().join("PLAN.md");
        std::fs::write(&plan, "plan").expect("plan file");
        assert!(is_workspace_plan_file(Some(root.path()), Some(&plan)));
        assert!(!is_workspace_plan_file(
            Some(root.path()),
            Some(&root.path().join("docs").join("PLAN.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(root.path()),
            Some(Path::new("/tmp/PLAN.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(root.path()),
            Some(&root.path().join("plan.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(root.path()),
            Some(Path::new("PLAN.md"))
        ));
    }

    #[test]
    fn dangling_plan_symlink_to_outside_is_not_the_plan_file() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let dangling = outside.path().join("new.txt");
        let plan_link = workspace.path().join("PLAN.md");
        std::os::unix::fs::symlink(&dangling, &plan_link).expect("symlink");
        let effective =
            crate::tools::native::support::resolve_effective_path(&plan_link).expect("resolve");
        assert_eq!(effective, dangling);
        assert!(!is_workspace_plan_file(
            Some(workspace.path()),
            Some(&effective)
        ));
    }

    #[test]
    fn classifies_native_reads_and_bash() {
        let hints = ClassificationHints {
            workspace_root: None,
            mutation_target: None,
            bash_command: Some("ls -la"),
        };
        let (kind, bash) = classify_tool_kind("bash", &ToolOrigin::Builtin, &hints);
        assert_eq!(kind, BuiltinToolKind::BashReadonly);
        assert!(matches!(bash, Some(BashReadonlyClass::Readonly)));
        let (kind, _) = classify_tool_kind("read", &ToolOrigin::Builtin, &hints);
        assert_eq!(kind, BuiltinToolKind::NativeRead);
    }
}
