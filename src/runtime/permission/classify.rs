//! Classify a prepared tool invocation into a built-in matrix family.

use std::path::{Component, Path, PathBuf};

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
        ToolOrigin::Mcp { .. }
        | ToolOrigin::ManagedPython { .. }
        | ToolOrigin::Host { .. } => {
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
            let class = hints
                .bash_command
                .map(analyze_bash_readonly)
                .map(|analysis| analysis.class)
                .unwrap_or(BashReadonlyClass::Unproven {
                    reason: "missing Bash command".to_owned(),
                });
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
        "subagent" | "send_message" | "workflow" => (BuiltinToolKind::ExternalOrDelegated, None),
        _ => (BuiltinToolKind::ExternalOrDelegated, None),
    }
}

/// True only when the mutation target is exactly `<workspace>/PLAN.md`.
#[must_use]
pub fn is_workspace_plan_file(
    workspace_root: Option<&Path>,
    mutation_target: Option<&Path>,
) -> bool {
    let (Some(root), Some(target)) = (workspace_root, mutation_target) else {
        return false;
    };
    let expected = root.join("PLAN.md");
    let Ok(canonical_root) = normalize_existing_or_lexical(root) else {
        return false;
    };
    let Ok(canonical_target) = normalize_existing_or_lexical(target) else {
        return false;
    };
    let Ok(canonical_expected) = normalize_existing_or_lexical(&expected) else {
        return false;
    };
    canonical_target == canonical_expected
        && canonical_target.starts_with(&canonical_root)
        && canonical_target
            .file_name()
            .is_some_and(|name| name == "PLAN.md")
}

fn normalize_existing_or_lexical(path: &Path) -> Result<PathBuf, ()> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_exact_workspace_plan_file_counts() {
        let root = PathBuf::from("/workspace");
        assert!(is_workspace_plan_file(
            Some(&root),
            Some(Path::new("/workspace/PLAN.md"))
        ));
        assert!(is_workspace_plan_file(
            Some(&root),
            Some(&root.join("PLAN.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(&root),
            Some(Path::new("/workspace/docs/PLAN.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(&root),
            Some(Path::new("/tmp/PLAN.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(&root),
            Some(Path::new("/workspace/plan.md"))
        ));
        assert!(!is_workspace_plan_file(
            Some(&root),
            Some(Path::new("PLAN.md"))
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
