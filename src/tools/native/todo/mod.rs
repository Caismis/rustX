//! The native `todo` tool: the model's own task list.
//!
//! ```json
//! { "action": "create", "subject": "Write the parser" }
//! { "action": "update", "id": 1, "status": "in_progress" }
//! { "action": "list" }
//! ```
//!
//! The tool is a thin adapter over the conversation-owned
//! [`ConversationTodoList`]: it owns the model-facing argument contract and
//! the prose of the reply, while task identity, the status machine, and the
//! dependency graph belong to that authority.
//!
//! # Every result carries the whole list
//!
//! A successful call replies with two content blocks: a human-readable
//! summary of what the call did, and the complete post-call
//! [`TodoSnapshot`] as structured JSON. The snapshot is why the list needs
//! no separate persistence — a restarted runtime rebuilds it from the last
//! such result, and a client renders the panel from the same fact in the
//! transcript it already holds.
//!
//! # A rejected call is an ordinary failed result
//!
//! Dependency and transition rules are validated before the list is
//! written, so a rejected call leaves the list untouched and settles as a
//! normal failed tool result carrying the specific reason. It is never an
//! attempt-level runtime failure, and it never publishes a snapshot — a
//! snapshot exists only where a mutation actually happened.

mod input;

use core::fmt::Write as _;

use futures_util::future::BoxFuture;

use crate::message::content::TextBlock;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::native::support::failed_result;
use crate::tools::todo::{
    ConversationTodoList, TODO_TOOL_ID, TodoSnapshot, TodoStatus, TodoTask, TodoUpdate,
};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
    ToolResultContent,
};

use input::{TodoAction, TodoInput};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "todo";

/// The tool-owned registration of the native `todo` tool.
///
/// The list is conversation state rather than a filesystem or process
/// effect, so the tool keeps the fixed foreground-only, sequential,
/// approval-never policy of the runtime intrinsics: two concurrent
/// mutations of one list would make the published snapshots race, and there
/// is nothing here for a human to approve.
#[must_use]
pub(super) fn registration(todos: ConversationTodoList) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), std::sync::Arc::new(TodoExecutor { todos }))
}

fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new(TODO_TOOL_ID),
        name: NAME.to_owned(),
        description: concat!(
            "Track multi-step work as a visible task list the user can see. ",
            "Open a list before starting anything that takes several steps, and keep it ",
            "current as you work: create one task per discrete step, mark exactly one task ",
            "in_progress at a time, and mark a task completed immediately when it is done ",
            "rather than in batches at the end. Never complete a task whose tests fail or ",
            "whose work is unfinished — leave it in_progress and add a task for the ",
            "remaining work. Change a task's status with {\"action\": \"update\", \"id\": N, ",
            "\"status\": \"...\"}. Use blocked_by/add_blocked_by when one task genuinely ",
            "cannot start before another finishes. Skip the list entirely for single-step ",
            "work — it is overhead there. Every reply contains the complete list, so a ",
            "list action is only needed to re-read it."
        )
        .to_owned(),
        input_schema: input_schema::<TodoInput>(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

struct TodoExecutor {
    todos: ConversationTodoList,
}

impl ToolExecutor for TodoExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            match TodoInput::parse(&invocation.arguments) {
                Ok(input) => self.dispatch(input),
                Err(error) => failed_result(error),
            }
        })
    }
}

impl TodoExecutor {
    /// Performs one action and renders its reply.
    fn dispatch(&self, input: TodoInput) -> ToolExecutionResult {
        match input.action {
            TodoAction::Create => self.create(input),
            TodoAction::Update => self.update(input),
            TodoAction::List => self.list(&input),
            TodoAction::Get => self.get(&input),
            TodoAction::Delete => self.delete(&input),
            TodoAction::Clear => self.clear(),
        }
    }

    fn create(&self, input: TodoInput) -> ToolExecutionResult {
        if input
            .subject
            .as_ref()
            .is_none_or(|subject| subject.trim().is_empty())
        {
            return failed_result("subject required for create");
        }
        match self.todos.create(input.create()) {
            Ok((task, snapshot)) => settled(
                format!("Created #{}: {} ({})", task.id, task.subject, task.status),
                &snapshot,
            ),
            Err(error) => failed_result(error.to_string()),
        }
    }

    fn update(&self, input: TodoInput) -> ToolExecutionResult {
        let Some(id) = input.id else {
            return failed_result("id required for update");
        };
        match self.todos.update(id, input.change()) {
            Ok((update, snapshot)) => settled(describe_update(&update), &snapshot),
            Err(error) => failed_result(error.to_string()),
        }
    }

    fn list(&self, input: &TodoInput) -> ToolExecutionResult {
        let snapshot = self.todos.snapshot();
        let filter = input.status.map(TodoStatus::from);
        let rows: Vec<String> = snapshot
            .tasks
            .iter()
            .filter(|task| match filter {
                Some(status) => task.status == status,
                None => input.include_deleted || task.status != TodoStatus::Deleted,
            })
            .map(render_row)
            .collect();
        let text = if rows.is_empty() {
            "No tasks".to_owned()
        } else {
            rows.join("\n")
        };
        settled(text, &snapshot)
    }

    fn get(&self, input: &TodoInput) -> ToolExecutionResult {
        let Some(id) = input.id else {
            return failed_result("id required for get");
        };
        let snapshot = self.todos.snapshot();
        let Some(task) = snapshot.task(id) else {
            return failed_result(
                crate::tools::todo::TodoMutationError::UnknownTask(id).to_string(),
            );
        };
        settled(render_detail(task, &snapshot.blocks(id)), &snapshot)
    }

    fn delete(&self, input: &TodoInput) -> ToolExecutionResult {
        let Some(id) = input.id else {
            return failed_result("id required for delete");
        };
        match self.todos.delete(id) {
            Ok((task, snapshot)) => {
                settled(format!("Deleted #{}: {}", task.id, task.subject), &snapshot)
            }
            Err(error) => failed_result(error.to_string()),
        }
    }

    fn clear(&self) -> ToolExecutionResult {
        let (dropped, snapshot) = self.todos.clear();
        let plural = if dropped == 1 { "task" } else { "tasks" };
        settled(format!("Cleared {dropped} {plural}"), &snapshot)
    }
}

/// One settled `todo` result: the summary, then the complete list.
fn settled(summary: String, snapshot: &TodoSnapshot) -> ToolExecutionResult {
    let value = serde_json::to_value(snapshot)
        .unwrap_or_else(|_| serde_json::json!({ "tasks": [], "next_id": 1 }));
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![
            ToolResultContent::Text(TextBlock { text: summary }),
            ToolResultContent::Json { value },
        ],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// What an accepted update actually did, in one line.
///
/// A model that re-issues an identical update is told the call changed
/// nothing, so it can stop repeating it instead of reading a second
/// `Updated #N` as evidence of progress.
fn describe_update(update: &TodoUpdate) -> String {
    if update.unchanged {
        return format!(
            "No change: #{} already matches the requested values (status: {})",
            update.task.id, update.task.status
        );
    }
    match update.previous_status {
        Some(previous) => format!(
            "Updated #{} ({previous} → {})",
            update.task.id, update.task.status
        ),
        None => format!("Updated #{}", update.task.id),
    }
}

/// One task as a list row.
fn render_row(task: &TodoTask) -> String {
    let mut row = format!("[{}] #{} {}", task.status, task.id, task.subject);
    if let (TodoStatus::InProgress, Some(active_form)) = (task.status, &task.active_form) {
        let _ = write!(row, " ({active_form})");
    }
    if !task.blocked_by.is_empty() {
        let _ = write!(row, " ⛓ {}", render_ids(&task.blocked_by));
    }
    row
}

/// One task in full, with both directions of its dependency edges.
fn render_detail(task: &TodoTask, blocks: &[u64]) -> String {
    let mut lines = vec![render_row(task)];
    if let Some(description) = &task.description {
        lines.push(format!("description: {description}"));
    }
    if let Some(active_form) = &task.active_form {
        lines.push(format!("active_form: {active_form}"));
    }
    if let Some(owner) = &task.owner {
        lines.push(format!("owner: {owner}"));
    }
    if !task.blocked_by.is_empty() {
        lines.push(format!("blocked_by: {}", render_ids(&task.blocked_by)));
    }
    if !blocks.is_empty() {
        lines.push(format!("blocks: {}", render_ids(blocks)));
    }
    if let Some(metadata) = &task.metadata {
        for (key, value) in metadata {
            lines.push(format!("metadata.{key}: {value}"));
        }
    }
    lines.join("\n")
}

fn render_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(",")
}
