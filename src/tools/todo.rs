//! The conversation-owned task list.
//!
//! One conversation owns one [`ConversationTodoList`]: the authoritative
//! set of tasks the model is tracking for that conversation, the id
//! allocator that names them, and the mutation semantics that keep the
//! dependency graph honest. The model-facing `todo` tool is a thin adapter
//! over this authority — it owns the argument contract and the reply
//! prose, never the state machine.
//!
//! ```text
//! native `todo` tool        model-facing contract, reply prose
//! ConversationTodoList      task identity, transitions, dependency graph
//! canonical tool results    the durable record the list is rebuilt from
//! ```
//!
//! # Why the list is not a file
//!
//! Every successful mutation publishes the complete post-mutation
//! [`TodoSnapshot`] as the structured content of its own canonical tool
//! result. That result is an ordinary durable Ledger fact, so the list needs
//! no sidecar file, no separate durability path, and no migration: a
//! restarted process rebuilds the list by reading the last snapshot the
//! conversation ever committed ([`ConversationTodoList::rebuilt`]), and a
//! client renders the same list by reading the same fact from the
//! transcript it already holds.
//!
//! The consequence is that the list follows the conversation. Resuming,
//! forking, or cloning a Session reopens the durable history that carries
//! the snapshot, and a conversation that never called `todo` has no list.
//!
//! # Session isolation
//!
//! The list is conversation-owned state, keyed by [`ConversationId`], and it
//! is reachable only through the tool registration of that conversation's own
//! tool plane. A subagent child composes the read-only `explore` profile and
//! has no `todo` registration at all, so a child can neither read nor
//! overwrite its parent's list — the isolation is structural, not a check.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::message::types::MessageBlock;
use crate::runtime::identity::{ConversationId, ToolId};
use crate::tools::types::{ToolExecutionStatus, ToolResultContent};

/// The durable identity of the native `todo` tool.
///
/// Rebuilding the list and rendering it in a client are both keyed by this
/// runtime identity rather than by a tool name or by the shape of the JSON,
/// so a differently named tool that happens to publish similar structure is
/// never mistaken for the task list.
pub const TODO_TOOL_ID: &str = "tool-todo";

/// The lifecycle status of one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Queued, not started.
    Pending,
    /// Being worked on now.
    InProgress,
    /// Finished.
    Completed,
    /// Tombstoned. Terminal, and kept so historical dependencies still
    /// resolve.
    Deleted,
}

impl TodoStatus {
    /// The model-facing spelling, which is also the serialized spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }

    /// Whether `target` is reachable from this status.
    ///
    /// Completed work may only be tombstoned, and a tombstone is terminal.
    /// A transition to the current status is legal and settles as a no-op.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        match self {
            Self::Deleted => matches!(target, Self::Deleted),
            Self::Completed => matches!(target, Self::Completed | Self::Deleted),
            Self::Pending | Self::InProgress => true,
        }
    }
}

impl core::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One task of the conversation's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoTask {
    /// The conversation-unique task id.
    pub id: u64,
    /// The imperative one-line subject.
    pub subject: String,
    /// Optional long-form detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional present-continuous label, shown while the task is in
    /// progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The lifecycle status.
    pub status: TodoStatus,
    /// The ids this task waits on, ascending and deduplicated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u64>,
    /// Optional free-form owner label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Optional free-form metadata. An empty record is dropped entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

/// The complete state of one conversation's list.
///
/// This is the persistence format: it is what a successful mutation
/// publishes, and it is what [`ConversationTodoList::rebuilt`] reads back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSnapshot {
    /// Every task, tombstones included, in creation order.
    #[serde(default)]
    pub tasks: Vec<TodoTask>,
    /// The id the next created task will receive.
    pub next_id: u64,
}

impl TodoSnapshot {
    /// The empty list of a conversation that has never called `todo`.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    /// The task with `id`, tombstones included.
    #[must_use]
    pub fn task(&self, id: u64) -> Option<&TodoTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// The ids of the tasks that wait on `id`, ascending.
    ///
    /// Derived from the other tasks' `blocked_by` sets rather than stored,
    /// so the forward and reverse edges of the graph cannot disagree.
    #[must_use]
    pub fn blocks(&self, id: u64) -> Vec<u64> {
        self.tasks
            .iter()
            .filter(|task| task.blocked_by.contains(&id))
            .map(|task| task.id)
            .collect()
    }

    /// The tasks that are neither completed nor tombstoned.
    pub fn open(&self) -> impl Iterator<Item = &TodoTask> {
        self.tasks
            .iter()
            .filter(|task| !matches!(task.status, TodoStatus::Completed | TodoStatus::Deleted))
    }

    /// The number of completed tasks and the number of live tasks — the
    /// `done/total` counters, which never count tombstones.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let live = self
            .tasks
            .iter()
            .filter(|task| task.status != TodoStatus::Deleted)
            .count();
        let done = self
            .tasks
            .iter()
            .filter(|task| task.status == TodoStatus::Completed)
            .count();
        (done, live)
    }
}

/// A rejected mutation.
///
/// Every variant is a business-level rejection of the requested change: the
/// list is validated before it is written, so a rejected call leaves the
/// state exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoMutationError {
    /// `create` was called without a non-blank subject.
    BlankSubject,
    /// No task carries the requested id.
    UnknownTask(u64),
    /// A dependency id names no task.
    UnknownDependency {
        /// The field that named it.
        field: &'static str,
        /// The named id.
        id: u64,
    },
    /// A dependency id names a tombstone.
    DeletedDependency {
        /// The field that named it.
        field: &'static str,
        /// The named id.
        id: u64,
    },
    /// A task was blocked on itself.
    SelfBlock(u64),
    /// The requested dependency would close a cycle.
    DependencyCycle,
    /// `update` named a task but requested no change.
    EmptyUpdate,
    /// The requested status is not reachable from the current one.
    IllegalTransition {
        /// The current status.
        from: TodoStatus,
        /// The rejected target status.
        to: TodoStatus,
    },
    /// `delete` was called on a tombstone.
    AlreadyDeleted(u64),
}

impl core::fmt::Display for TodoMutationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BlankSubject => write!(f, "subject required for create"),
            Self::UnknownTask(id) => write!(f, "#{id} not found"),
            Self::UnknownDependency { field, id } => write!(f, "{field}: #{id} not found"),
            Self::DeletedDependency { field, id } => write!(f, "{field}: #{id} is deleted"),
            Self::SelfBlock(id) => write!(f, "cannot block #{id} on itself"),
            Self::DependencyCycle => {
                write!(
                    f,
                    "the requested dependency would create a cycle in the blocked_by graph"
                )
            }
            Self::EmptyUpdate => write!(
                f,
                "update requires at least one mutable field: subject, description, \
                 active_form, status, owner, metadata, add_blocked_by, or remove_blocked_by"
            ),
            Self::IllegalTransition { from, to } => write!(f, "illegal transition {from} → {to}"),
            Self::AlreadyDeleted(id) => write!(f, "#{id} is already deleted"),
        }
    }
}

impl std::error::Error for TodoMutationError {}

/// The fields one `create` supplies.
#[derive(Debug, Clone, Default)]
pub struct TodoCreate {
    /// The imperative one-line subject. Required and non-blank.
    pub subject: String,
    /// Optional long-form detail.
    pub description: Option<String>,
    /// Optional present-continuous label.
    pub active_form: Option<String>,
    /// Optional owner label.
    pub owner: Option<String>,
    /// Optional initial dependencies.
    pub blocked_by: Vec<u64>,
    /// Optional initial metadata.
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

/// The fields one `update` may change.
///
/// Every field is optional and absent means *unchanged*: an update is a
/// patch, never a whole-record replacement, so two updates of different
/// fields cannot undo each other.
#[derive(Debug, Clone, Default)]
pub struct TodoChange {
    /// A new subject.
    pub subject: Option<String>,
    /// A new long-form detail.
    pub description: Option<String>,
    /// A new present-continuous label.
    pub active_form: Option<String>,
    /// A new owner label.
    pub owner: Option<String>,
    /// The requested status.
    pub status: Option<TodoStatus>,
    /// Dependencies to add.
    pub add_blocked_by: Vec<u64>,
    /// Dependencies to remove.
    pub remove_blocked_by: Vec<u64>,
    /// A metadata patch merged key by key. A JSON `null` value removes that
    /// key, and emptying the record drops the field.
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

impl TodoChange {
    /// Whether this change requests anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subject.is_none()
            && self.description.is_none()
            && self.active_form.is_none()
            && self.owner.is_none()
            && self.status.is_none()
            && self.add_blocked_by.is_empty()
            && self.remove_blocked_by.is_empty()
            && self.metadata.is_none()
    }
}

/// What one accepted `update` actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoUpdate {
    /// The task after the update.
    pub task: TodoTask,
    /// The status the task held before the update, when it changed.
    pub previous_status: Option<TodoStatus>,
    /// Whether the update changed nothing. A model that re-issues an
    /// identical update is told it was a no-op instead of being told the
    /// task was updated again.
    pub unchanged: bool,
}

/// The authoritative task list of one conversation.
///
/// Cloning shares the list: the clone handed to the tool registration and
/// the handle the composition keeps are the same authority, exactly as with
/// the conversation's background registry.
#[derive(Clone)]
pub struct ConversationTodoList {
    conversation_id: ConversationId,
    inner: Arc<Mutex<TodoSnapshot>>,
}

impl core::fmt::Debug for ConversationTodoList {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConversationTodoList")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl ConversationTodoList {
    /// Creates the empty list of one conversation.
    #[must_use]
    pub fn new(conversation_id: ConversationId) -> Self {
        Self {
            conversation_id,
            inner: Arc::new(Mutex::new(TodoSnapshot::empty())),
        }
    }

    /// Rebuilds one conversation's list from its canonical history.
    ///
    /// The last snapshot a successful `todo` result ever committed *is* the
    /// list: later results supersede earlier ones completely, so replaying
    /// the whole history is unnecessary and a partially reconstructed list
    /// is impossible. History that contains no such result yields the empty
    /// list, which is exactly what a conversation that never tracked tasks
    /// should have.
    #[must_use]
    pub fn rebuilt(conversation_id: ConversationId, history: &[MessageBlock]) -> Self {
        let snapshot = last_snapshot(history).unwrap_or_else(TodoSnapshot::empty);
        Self {
            conversation_id,
            inner: Arc::new(Mutex::new(snapshot)),
        }
    }

    /// The conversation this list belongs to.
    #[must_use]
    pub const fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The current complete state.
    #[must_use]
    pub fn snapshot(&self) -> TodoSnapshot {
        self.state().clone()
    }

    /// Adds one task in `pending` and returns it with the new snapshot.
    ///
    /// # Errors
    ///
    /// Returns the rejection of a blank subject or of an initial dependency
    /// that is unknown or tombstoned. Nothing is written when the call is
    /// rejected, so the id allocator does not advance either.
    pub fn create(&self, spec: TodoCreate) -> Result<(TodoTask, TodoSnapshot), TodoMutationError> {
        let subject = spec.subject.trim().to_owned();
        if subject.is_empty() {
            return Err(TodoMutationError::BlankSubject);
        }
        let mut state = self.state();
        let blocked_by = normalize_ids(spec.blocked_by);
        for id in &blocked_by {
            check_dependency(&state, "blocked_by", *id)?;
        }
        let task = TodoTask {
            id: state.next_id,
            subject,
            description: spec.description,
            active_form: spec.active_form,
            status: TodoStatus::Pending,
            blocked_by,
            owner: spec.owner,
            metadata: spec.metadata.filter(|metadata| !metadata.is_empty()),
        };
        state.next_id = state.next_id.saturating_add(1);
        state.tasks.push(task.clone());
        Ok((task, state.clone()))
    }

    /// Applies one patch to one task.
    ///
    /// # Errors
    ///
    /// Returns the rejection of an unknown task, an empty patch, an illegal
    /// status transition, or a dependency edge that is unknown, tombstoned,
    /// self-referential, or cyclic. Validation completes before anything is
    /// written.
    pub fn update(
        &self,
        id: u64,
        change: TodoChange,
    ) -> Result<(TodoUpdate, TodoSnapshot), TodoMutationError> {
        if change.is_empty() {
            return Err(TodoMutationError::EmptyUpdate);
        }
        let mut state = self.state();
        let position = state
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or(TodoMutationError::UnknownTask(id))?;
        let current = state.tasks[position].clone();

        let mut updated = current.clone();
        if let Some(status) = change.status {
            if !current.status.can_transition_to(status) {
                return Err(TodoMutationError::IllegalTransition {
                    from: current.status,
                    to: status,
                });
            }
            updated.status = status;
        }
        if let Some(subject) = change.subject {
            updated.subject = subject;
        }
        if let Some(description) = change.description {
            updated.description = Some(description);
        }
        if let Some(active_form) = change.active_form {
            updated.active_form = Some(active_form);
        }
        if let Some(owner) = change.owner {
            updated.owner = Some(owner);
        }
        if let Some(patch) = change.metadata {
            updated.metadata = merge_metadata(updated.metadata.take(), patch);
        }
        for added in &change.add_blocked_by {
            if *added == id {
                return Err(TodoMutationError::SelfBlock(id));
            }
            check_dependency(&state, "add_blocked_by", *added)?;
        }
        let mut blocked_by: Vec<u64> = updated
            .blocked_by
            .iter()
            .copied()
            .filter(|existing| !change.remove_blocked_by.contains(existing))
            .chain(change.add_blocked_by.iter().copied())
            .collect();
        blocked_by = normalize_ids(blocked_by);
        updated.blocked_by = blocked_by;
        if !change.add_blocked_by.is_empty() && closes_cycle(&state, &updated) {
            return Err(TodoMutationError::DependencyCycle);
        }

        let unchanged = updated == current;
        let previous_status = (updated.status != current.status).then_some(current.status);
        state.tasks[position] = updated.clone();
        Ok((
            TodoUpdate {
                task: updated,
                previous_status,
                unchanged,
            },
            state.clone(),
        ))
    }

    /// Tombstones one task.
    ///
    /// The task is kept as a `deleted` record so historical dependency
    /// references still resolve; it is never removed from the list.
    ///
    /// # Errors
    ///
    /// Returns the rejection of an unknown task or of a task that is
    /// already tombstoned.
    pub fn delete(&self, id: u64) -> Result<(TodoTask, TodoSnapshot), TodoMutationError> {
        let mut state = self.state();
        let position = state
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or(TodoMutationError::UnknownTask(id))?;
        if state.tasks[position].status == TodoStatus::Deleted {
            return Err(TodoMutationError::AlreadyDeleted(id));
        }
        state.tasks[position].status = TodoStatus::Deleted;
        let task = state.tasks[position].clone();
        Ok((task, state.clone()))
    }

    /// Drops every task and resets the id allocator.
    ///
    /// Returns how many tasks were dropped together with the emptied
    /// snapshot.
    #[must_use]
    pub fn clear(&self) -> (usize, TodoSnapshot) {
        let mut state = self.state();
        let dropped = state.tasks.len();
        *state = TodoSnapshot::empty();
        (dropped, state.clone())
    }

    /// Replaces the whole list with `snapshot`.
    ///
    /// The one writer that does not go through the mutation semantics: it
    /// exists so a runtime that already holds a committed snapshot can adopt
    /// it verbatim.
    pub fn adopt(&self, snapshot: TodoSnapshot) {
        *self.state() = snapshot;
    }

    fn state(&self) -> std::sync::MutexGuard<'_, TodoSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The last complete snapshot committed by a successful `todo` result.
fn last_snapshot(history: &[MessageBlock]) -> Option<TodoSnapshot> {
    let todo = ToolId::new(TODO_TOOL_ID);
    history
        .iter()
        .rev()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_id == todo => Some(&tool.result),
            _ => None,
        })
        .filter(|result| result.status == ToolExecutionStatus::Success)
        .find_map(|result| {
            result.content.iter().find_map(|content| match content {
                ToolResultContent::Json { value } => {
                    serde_json::from_value::<TodoSnapshot>(value.clone()).ok()
                }
                _ => None,
            })
        })
}

/// Rejects a dependency that names no task or names a tombstone.
fn check_dependency(
    snapshot: &TodoSnapshot,
    field: &'static str,
    id: u64,
) -> Result<(), TodoMutationError> {
    match snapshot.task(id) {
        None => Err(TodoMutationError::UnknownDependency { field, id }),
        Some(task) if task.status == TodoStatus::Deleted => {
            Err(TodoMutationError::DeletedDependency { field, id })
        }
        Some(_) => Ok(()),
    }
}

/// Whether `candidate` can reach itself through the graph it would create.
fn closes_cycle(snapshot: &TodoSnapshot, candidate: &TodoTask) -> bool {
    let mut seen = HashSet::new();
    let mut frontier = candidate.blocked_by.clone();
    while let Some(id) = frontier.pop() {
        if id == candidate.id {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(task) = snapshot.task(id) {
            frontier.extend(task.blocked_by.iter().copied());
        }
    }
    false
}

/// Sorts and deduplicates a dependency set so equal graphs compare equal.
fn normalize_ids(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Merges one metadata patch, where a JSON `null` removes its key.
fn merge_metadata(
    current: Option<BTreeMap<String, serde_json::Value>>,
    patch: BTreeMap<String, serde_json::Value>,
) -> Option<BTreeMap<String, serde_json::Value>> {
    let mut merged = current.unwrap_or_default();
    for (key, value) in patch {
        if value.is_null() {
            merged.remove(&key);
        } else {
            merged.insert(key, value);
        }
    }
    (!merged.is_empty()).then_some(merged)
}

#[cfg(test)]
mod tests {
    use super::{
        ConversationTodoList, TODO_TOOL_ID, TodoChange, TodoCreate, TodoMutationError,
        TodoSnapshot, TodoStatus,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, ToolMessageBlock};
    use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

    fn list() -> ConversationTodoList {
        ConversationTodoList::new(ConversationId::new("conv-todo"))
    }

    fn subject(subject: &str) -> TodoCreate {
        TodoCreate {
            subject: subject.to_owned(),
            ..TodoCreate::default()
        }
    }

    fn status(status: TodoStatus) -> TodoChange {
        TodoChange {
            status: Some(status),
            ..TodoChange::default()
        }
    }

    #[test]
    fn ids_are_assigned_in_creation_order_and_start_pending() {
        let todos = list();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, snapshot) = todos.create(subject("Write the tests")).expect("create");
        assert_eq!((first.id, second.id), (1, 2));
        assert_eq!(first.status, TodoStatus::Pending);
        assert_eq!(snapshot.next_id, 3);
    }

    #[test]
    fn a_blank_subject_creates_nothing() {
        let todos = list();
        assert_eq!(
            todos.create(subject("   ")).expect_err("blank subject"),
            TodoMutationError::BlankSubject
        );
        assert_eq!(todos.snapshot(), TodoSnapshot::empty());
    }

    #[test]
    fn completed_work_may_only_be_tombstoned() {
        let todos = list();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        todos
            .update(task.id, status(TodoStatus::Completed))
            .expect("complete");
        assert_eq!(
            todos
                .update(task.id, status(TodoStatus::InProgress))
                .expect_err("illegal transition"),
            TodoMutationError::IllegalTransition {
                from: TodoStatus::Completed,
                to: TodoStatus::InProgress,
            }
        );
        let (update, _) = todos
            .update(task.id, status(TodoStatus::Deleted))
            .expect("tombstone");
        assert_eq!(update.task.status, TodoStatus::Deleted);
    }

    #[test]
    fn re_issuing_the_same_update_is_reported_as_a_no_op() {
        let todos = list();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        let (first, _) = todos
            .update(task.id, status(TodoStatus::InProgress))
            .expect("start");
        assert!(!first.unchanged);
        assert_eq!(first.previous_status, Some(TodoStatus::Pending));
        let (again, _) = todos
            .update(task.id, status(TodoStatus::InProgress))
            .expect("start again");
        assert!(again.unchanged);
        assert_eq!(again.previous_status, None);
    }

    #[test]
    fn an_update_without_a_mutable_field_is_rejected() {
        let todos = list();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        assert_eq!(
            todos
                .update(task.id, TodoChange::default())
                .expect_err("empty update"),
            TodoMutationError::EmptyUpdate
        );
    }

    #[test]
    fn dependency_edges_are_validated_before_anything_is_written() {
        let todos = list();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, _) = todos.create(subject("Write the tests")).expect("create");

        assert_eq!(
            todos
                .create(TodoCreate {
                    subject: "Ship".to_owned(),
                    blocked_by: vec![99],
                    ..TodoCreate::default()
                })
                .expect_err("unknown dependency"),
            TodoMutationError::UnknownDependency {
                field: "blocked_by",
                id: 99,
            }
        );
        assert_eq!(
            todos.snapshot().next_id,
            3,
            "a rejected create does not consume an id"
        );

        assert_eq!(
            todos
                .update(
                    second.id,
                    TodoChange {
                        add_blocked_by: vec![second.id],
                        ..TodoChange::default()
                    },
                )
                .expect_err("self block"),
            TodoMutationError::SelfBlock(second.id)
        );

        todos
            .update(
                second.id,
                TodoChange {
                    add_blocked_by: vec![first.id],
                    ..TodoChange::default()
                },
            )
            .expect("second waits on first");
        assert_eq!(
            todos
                .update(
                    first.id,
                    TodoChange {
                        add_blocked_by: vec![second.id],
                        ..TodoChange::default()
                    },
                )
                .expect_err("cycle"),
            TodoMutationError::DependencyCycle
        );
        assert_eq!(
            todos.snapshot().task(first.id).expect("first").blocked_by,
            Vec::<u64>::new(),
            "the rejected edge was never written"
        );
    }

    #[test]
    fn a_tombstone_stays_referenceable_but_cannot_be_a_new_dependency() {
        let todos = list();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, _) = todos.create(subject("Write the tests")).expect("create");
        todos
            .update(
                second.id,
                TodoChange {
                    add_blocked_by: vec![first.id],
                    ..TodoChange::default()
                },
            )
            .expect("second waits on first");
        todos.delete(first.id).expect("tombstone");

        let snapshot = todos.snapshot();
        assert_eq!(
            snapshot.task(second.id).expect("second").blocked_by,
            vec![first.id],
            "the historical edge still resolves"
        );
        assert_eq!(snapshot.blocks(first.id), vec![second.id]);
        assert_eq!(
            todos.delete(first.id).expect_err("already deleted"),
            TodoMutationError::AlreadyDeleted(first.id)
        );

        let (third, _) = todos.create(subject("Ship")).expect("create");
        assert_eq!(
            todos
                .update(
                    third.id,
                    TodoChange {
                        add_blocked_by: vec![first.id],
                        ..TodoChange::default()
                    },
                )
                .expect_err("deleted dependency"),
            TodoMutationError::DeletedDependency {
                field: "add_blocked_by",
                id: first.id,
            }
        );
    }

    #[test]
    fn metadata_merges_key_by_key_and_null_removes_a_key() {
        let todos = list();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        let patch = |pairs: &[(&str, serde_json::Value)]| TodoChange {
            metadata: Some(
                pairs
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.clone()))
                    .collect(),
            ),
            ..TodoChange::default()
        };
        todos
            .update(task.id, patch(&[("pr", serde_json::json!(12))]))
            .expect("first patch");
        let (update, _) = todos
            .update(task.id, patch(&[("branch", serde_json::json!("main"))]))
            .expect("second patch");
        let metadata = update.task.metadata.clone().expect("metadata");
        assert_eq!(metadata["pr"], serde_json::json!(12));
        assert_eq!(metadata["branch"], serde_json::json!("main"));

        let (update, _) = todos
            .update(
                task.id,
                patch(&[
                    ("pr", serde_json::Value::Null),
                    ("branch", serde_json::Value::Null),
                ]),
            )
            .expect("removal");
        assert_eq!(
            update.task.metadata, None,
            "an emptied record drops the field entirely"
        );
    }

    #[test]
    fn clear_drops_every_task_and_restarts_the_id_allocator() {
        let todos = list();
        todos.create(subject("Write the parser")).expect("create");
        todos.create(subject("Write the tests")).expect("create");
        let (dropped, snapshot) = todos.clear();
        assert_eq!(dropped, 2);
        assert_eq!(snapshot, TodoSnapshot::empty());
        let (task, _) = todos.create(subject("Start over")).expect("create");
        assert_eq!(task.id, 1);
    }

    #[test]
    fn progress_counts_live_tasks_only() {
        let todos = list();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, _) = todos.create(subject("Write the tests")).expect("create");
        todos.create(subject("Ship")).expect("create");
        todos
            .update(first.id, status(TodoStatus::Completed))
            .expect("complete");
        todos.delete(second.id).expect("tombstone");
        assert_eq!(todos.snapshot().progress(), (1, 2));
    }

    fn todo_result(snapshot: &TodoSnapshot) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("message-todo"),
            tool_call_id: ToolCallId::new("call-todo"),
            tool_id: ToolId::new(TODO_TOOL_ID),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![
                    ToolResultContent::Text(TextBlock {
                        text: "Created #1: Write the parser (pending)".to_owned(),
                    }),
                    ToolResultContent::Json {
                        value: serde_json::to_value(snapshot).expect("snapshot"),
                    },
                ],
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        })
    }

    #[test]
    fn the_list_is_rebuilt_from_the_last_committed_snapshot() {
        let todos = list();
        todos.create(subject("Write the parser")).expect("create");
        let first = todos.snapshot();
        let (task, _) = todos.create(subject("Write the tests")).expect("create");
        todos
            .update(task.id, status(TodoStatus::InProgress))
            .expect("start");
        let latest = todos.snapshot();

        let history = vec![todo_result(&first), todo_result(&latest)];
        let rebuilt = ConversationTodoList::rebuilt(ConversationId::new("conv-todo"), &history);
        assert_eq!(rebuilt.snapshot(), latest);

        // The rebuilt list keeps allocating where the conversation left off.
        let (next, _) = rebuilt.create(subject("Ship")).expect("create");
        assert_eq!(next.id, 3);
    }

    #[test]
    fn history_without_a_todo_result_rebuilds_an_empty_list() {
        let rebuilt = ConversationTodoList::rebuilt(
            ConversationId::new("conv-todo"),
            &[MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("message-bash"),
                tool_call_id: ToolCallId::new("call-bash"),
                tool_id: ToolId::new("tool-bash"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: vec![ToolResultContent::Json {
                        value: serde_json::json!({ "tasks": [], "next_id": 9 }),
                    }],
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            })],
        );
        assert_eq!(rebuilt.snapshot(), TodoSnapshot::empty());
    }
}
