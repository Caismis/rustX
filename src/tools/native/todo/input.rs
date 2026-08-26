//! The typed model-facing input contract of the native `todo` tool.
//!
//! One flat object carries every action, exactly as the model writes it.
//! The alternative — one variant per action — would give the provider a
//! tagged union whose spelling differs per provider, and the actions share
//! most of their fields anyway.
//!
//! Which fields an action actually requires is *semantic* validation and
//! lives in the tool, not in the schema: a rejected `update` must be able to
//! explain that it needed a mutable field, and a schema-level rejection
//! cannot say that.
//!
//! # This is the trust boundary for the text of a task
//!
//! A task's text is written by a model and later drawn by a terminal
//! client, so this module is where that text stops being arbitrary bytes. A
//! length bound alone does not make a task one row: a single `\n` in a
//! subject makes one task occupy two physical lines, and enough of them
//! overflow a panel whose whole contract is that it is bounded. `ESC`,
//! the C1 range, and the bidi overrides are worse than layout — they
//! repaint, move the cursor, retitle the window, or reverse the reading
//! order of the text around them.
//!
//! Every text field is therefore checked here, before any list state is
//! touched, and a violation is an ordinary rejected call naming the exact
//! character. Nothing downstream has to escape what can never be stored.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;
use crate::tools::todo::{TodoChange, TodoCreate, TodoStatus, forbidden_control};

/// The requested operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TodoAction {
    /// Add one task in `pending`.
    Create,
    /// Change one task's fields, status, or dependencies.
    Update,
    /// Show the list.
    List,
    /// Show one task with its dependency edges.
    Get,
    /// Tombstone one task.
    Delete,
    /// Drop every task.
    Clear,
}

/// The model-facing status spelling of the input contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TodoStatusInput {
    /// Queued, not started.
    Pending,
    /// Being worked on now.
    InProgress,
    /// Finished.
    Completed,
    /// Tombstoned.
    Deleted,
}

impl From<TodoStatusInput> for TodoStatus {
    fn from(status: TodoStatusInput) -> Self {
        match status {
            TodoStatusInput::Pending => Self::Pending,
            TodoStatusInput::InProgress => Self::InProgress,
            TodoStatusInput::Completed => Self::Completed,
            TodoStatusInput::Deleted => Self::Deleted,
        }
    }
}

/// The canonical input contract of the `todo` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TodoInput {
    /// The operation to perform.
    pub action: TodoAction,
    /// The task to act on. Required by `update`, `get`, and `delete`.
    pub id: Option<u64>,
    /// The imperative one-line subject. Required by `create`.
    #[schemars(length(min = 1, max = 200))]
    pub subject: Option<String>,
    /// Optional long-form detail of the task.
    #[schemars(length(min = 1, max = 4096))]
    pub description: Option<String>,
    /// The present-continuous label shown while the task is in progress,
    /// for example `writing the parser`.
    #[schemars(length(min = 1, max = 200))]
    pub active_form: Option<String>,
    /// Who is doing the task. Free-form.
    #[schemars(length(min = 1, max = 100))]
    pub owner: Option<String>,
    /// On `update`, the requested status. On `list`, the status to filter
    /// by.
    pub status: Option<TodoStatusInput>,
    /// On `create`, the ids this new task waits on.
    pub blocked_by: Option<Vec<u64>>,
    /// On `update`, ids to add to this task's dependencies. Additive — send
    /// only what changes, never the whole set.
    pub add_blocked_by: Option<Vec<u64>>,
    /// On `update`, ids to remove from this task's dependencies. Additive.
    pub remove_blocked_by: Option<Vec<u64>>,
    /// Free-form metadata. On `update` the record is merged key by key and
    /// a `null` value removes that key.
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
    /// On `list`, include tombstoned tasks. Omitted means they are hidden.
    #[serde(default)]
    pub include_deleted: bool,
}

impl TodoInput {
    /// Deserializes one `todo` invocation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        input.validate()?;
        Ok(input)
    }

    /// Rejects text a bounded client row could not draw as itself.
    ///
    /// `description` is the one long-form field and is never a panel row,
    /// so line breaks stay legal there and nowhere else.
    fn validate(&self) -> Result<(), String> {
        for (field, value, multiline) in [
            ("subject", self.subject.as_deref(), false),
            ("active_form", self.active_form.as_deref(), false),
            ("owner", self.owner.as_deref(), false),
            ("description", self.description.as_deref(), true),
        ] {
            let Some(value) = value else {
                continue;
            };
            if let Some(character) = forbidden_control(value, multiline) {
                let allowance = if multiline {
                    " (a line break is the only one allowed there)"
                } else {
                    ", and it is one line"
                };
                return Err(format!(
                    "{field} may not contain the control character U+{:04X}{allowance}",
                    character as u32
                ));
            }
        }
        Ok(())
    }

    /// The `create` specification this invocation describes.
    pub(super) fn create(self) -> TodoCreate {
        TodoCreate {
            subject: self.subject.unwrap_or_default(),
            description: self.description,
            active_form: self.active_form,
            owner: self.owner,
            blocked_by: self.blocked_by.unwrap_or_default(),
            metadata: self.metadata,
        }
    }

    /// The `update` patch this invocation describes.
    pub(super) fn change(self) -> TodoChange {
        TodoChange {
            subject: self.subject,
            description: self.description,
            active_form: self.active_form,
            owner: self.owner,
            status: self.status.map(TodoStatus::from),
            add_blocked_by: self.add_blocked_by.unwrap_or_default(),
            remove_blocked_by: self.remove_blocked_by.unwrap_or_default(),
            metadata: self.metadata,
        }
    }
}
