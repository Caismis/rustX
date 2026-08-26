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
//!
//! # A mutation is provisional until its own result is durable
//!
//! Publishing the snapshot as a canonical tool result is what makes the
//! result the list's only durable record — which is only true if the
//! in-memory list never runs ahead of it. A mutation therefore writes a
//! *staged* list, not the authority:
//!
//! ```text
//! todo call        -> staged   (later calls of the same batch read it)
//! batch committed  -> staged becomes committed
//! batch failed     -> staged is dropped, committed is untouched
//! ```
//!
//! The Agent Loop owns both endings: it installs the staged list exactly
//! where the whole `ToolResult` batch became canonical, and discards it on
//! every other exit. So a batch that fails to commit — a durable failure, a
//! rejected append, a poisoned attempt — leaves the conversation's list
//! exactly as canonical history describes it.
//!
//! Provisional state is owned, not ambient. [`ConversationTodoList::open_batch`]
//! hands out one [`TodoBatch`] at a time and refuses to take the list away
//! from a batch that already holds it; every mutation goes through the
//! [`TodoWriter`] that batch hands its own executors, and a writer whose
//! batch is closed writes nothing. So an executor driven outside the Agent
//! Loop has no writer and mutates nothing, and one driven *beside* a running
//! batch cannot slip a task into a list that batch is about to publish as
//! its own.

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
#[serde(deny_unknown_fields)]
pub struct TodoTask {
    /// The task id, unique within the current list generation.
    ///
    /// `clear` resets the allocator, so an id names one task for as long as
    /// the list it belongs to lives — not for as long as the conversation
    /// does.
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

impl TodoTask {
    /// Every model-written text field, with whether it may span lines.
    ///
    /// The single-line fields are the ones a client draws as one row of a
    /// bounded panel; `description` is long-form prose that only the
    /// model-facing `get` reply ever renders.
    fn text_fields(&self) -> impl Iterator<Item = (&'static str, &str, bool)> {
        [
            Some(("subject", self.subject.as_str(), false)),
            self.active_form
                .as_deref()
                .map(|value| ("active_form", value, false)),
            self.owner.as_deref().map(|value| ("owner", value, false)),
            self.description
                .as_deref()
                .map(|value| ("description", value, true)),
        ]
        .into_iter()
        .flatten()
    }
}

/// The first character of `value` that a task text field may not carry, if
/// any.
///
/// The list is written by a model and drawn by a terminal client, so the
/// text of a task is the one place where model output reaches a terminal
/// unescaped. Two distinct hazards live in the same character classes and
/// are refused together:
///
/// - **layout**: a newline, a carriage return, or a tab makes one task
///   occupy more than the one physical row a bounded panel budgeted for it,
///   so enough of them push the conversation off the screen no matter what
///   the panel's row budget says;
/// - **control**: `ESC`-introduced CSI/OSC sequences, the C1 range, and the
///   bidi overrides can repaint colours, move the cursor, retitle the
///   window, or reverse the reading order of text around them.
///
/// `multiline` keeps `\n` legal for the one field whose whole purpose is
/// long-form prose, and it never reaches a bounded panel row.
#[must_use]
pub fn forbidden_control(value: &str, multiline: bool) -> Option<char> {
    value.chars().find(|character| {
        if multiline && *character == '\n' {
            return false;
        }
        character.is_control()
            || matches!(character,
                '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

/// The first character a task's metadata may not carry, key or value.
///
/// Metadata is model-written and the `get` reply renders it, so it obeys the
/// same rule as every other task text — it is simply nested, so keys and
/// every string anywhere inside a value are checked.
#[must_use]
pub fn metadata_control(metadata: &BTreeMap<String, serde_json::Value>) -> Option<char> {
    metadata
        .iter()
        .find_map(|(key, value)| forbidden_control(key, false).or_else(|| json_control(value)))
}

/// The first forbidden character in any string `value` contains.
fn json_control(value: &serde_json::Value) -> Option<char> {
    match value {
        serde_json::Value::String(text) => forbidden_control(text, false),
        serde_json::Value::Array(items) => items.iter().find_map(json_control),
        serde_json::Value::Object(fields) => fields.iter().find_map(|(key, nested)| {
            forbidden_control(key, false).or_else(|| json_control(nested))
        }),
        _ => None,
    }
}

/// A published list that no sequence of mutations could have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoSnapshotError {
    /// The id allocator cannot name a task.
    UnusableAllocator,
    /// Two tasks share one id.
    DuplicateId(u64),
    /// A task carries an id the allocator never handed out.
    UnallocatedId {
        /// The task's id.
        id: u64,
        /// The allocator's next id.
        next_id: u64,
    },
    /// The tasks are not the dense, ordered generation every sequence of
    /// mutations produces.
    OutOfCreationOrder {
        /// Where the task sits in the list.
        position: usize,
        /// The id it carries.
        id: u64,
        /// The id creation order requires there.
        expected: u64,
    },
    /// The allocator is ahead of the tasks it allocated: ids were handed out
    /// that the list does not carry, which no mutation can do.
    AllocatorAhead {
        /// The allocator's next id.
        next_id: u64,
        /// How many tasks the list carries.
        tasks: usize,
    },
    /// A task has no subject.
    BlankSubject(u64),
    /// A text field carries a character a client may not draw.
    ControlCharacter {
        /// The task that carries it.
        id: u64,
        /// The field that carries it.
        field: &'static str,
        /// The offending scalar value.
        codepoint: u32,
    },
    /// A task waits on itself.
    SelfBlock(u64),
    /// A task waits on an id no task carries.
    UnknownDependency {
        /// The waiting task.
        id: u64,
        /// The id it waits on.
        blocker: u64,
    },
    /// A dependency set is not the ascending, deduplicated form every
    /// mutation writes.
    UnnormalizedDependencies(u64),
    /// The dependency graph contains a cycle through this task.
    DependencyCycle(u64),
}

impl core::fmt::Display for TodoSnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnusableAllocator => write!(f, "next_id 0 can never name a task"),
            Self::DuplicateId(id) => write!(f, "#{id} appears twice"),
            Self::UnallocatedId { id, next_id } => {
                write!(f, "#{id} was never allocated (next_id is {next_id})")
            }
            Self::OutOfCreationOrder {
                position,
                id,
                expected,
            } => write!(
                f,
                "#{id} sits at position {position}, where creation order requires #{expected}"
            ),
            Self::AllocatorAhead { next_id, tasks } => write!(
                f,
                "next_id {next_id} is ahead of the {tasks} task(s) the list carries"
            ),
            Self::BlankSubject(id) => write!(f, "#{id} has no subject"),
            Self::ControlCharacter {
                id,
                field,
                codepoint,
            } => write!(
                f,
                "#{id} {field} carries the control character U+{codepoint:04X}"
            ),
            Self::SelfBlock(id) => write!(f, "#{id} waits on itself"),
            Self::UnknownDependency { id, blocker } => {
                write!(f, "#{id} waits on #{blocker}, which no task carries")
            }
            Self::UnnormalizedDependencies(id) => {
                write!(f, "#{id} blocked_by is not ascending and deduplicated")
            }
            Self::DependencyCycle(id) => write!(f, "#{id} closes a cycle in the blocked_by graph"),
        }
    }
}

impl std::error::Error for TodoSnapshotError {}

/// Why a conversation's list could not be rebuilt from its own history.
///
/// Every variant describes the **newest** successful `todo` result, which
/// is the only record that can be the list. Rebuilding never falls back to
/// an older snapshot: an older one is a list the conversation has already
/// moved on from, so adopting it would resurrect tasks that were completed,
/// tombstoned, or cleared. Refusing is the only answer that cannot silently
/// serve a false list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoRebuildError {
    /// The result settled successfully but published no structured list.
    Missing,
    /// The published list could not be decoded.
    Undecodable(String),
    /// The published list decoded but violates the list's own invariants.
    Invalid(TodoSnapshotError),
}

impl core::fmt::Display for TodoRebuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "the newest committed todo result does not carry a usable list: "
        )?;
        match self {
            Self::Missing => write!(f, "it published no structured list"),
            Self::Undecodable(error) => write!(f, "{error}"),
            Self::Invalid(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TodoRebuildError {}

/// The complete state of one conversation's list.
///
/// This is the persistence format: it is what a successful mutation
/// publishes, and it is what [`ConversationTodoList::rebuilt`] reads back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoSnapshot {
    /// Every task, tombstones included, in creation order.
    #[serde(default)]
    pub tasks: Vec<TodoTask>,
    /// The id the next created task will receive.
    pub next_id: u64,
}

/// The default list is the *empty* list, allocator included.
///
/// Deriving this would produce `next_id: 0`, which is not a list any
/// mutation could ever have produced: the first created task would take id
/// `0` and the second would take `1`, so a defaulted list and a real one
/// would not even name tasks the same way.
impl Default for TodoSnapshot {
    fn default() -> Self {
        Self::empty()
    }
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

    /// Checks every invariant a list produced by these mutations holds.
    ///
    /// Decoding proves the *shape* of a published snapshot; this proves its
    /// *meaning*. It is what a rebuild runs before adopting a snapshot from
    /// canonical history, so a record that no sequence of mutations could
    /// have produced is refused rather than served as the conversation's
    /// list.
    ///
    /// # Errors
    ///
    /// Returns the first violated invariant.
    pub fn validate(&self) -> Result<(), TodoSnapshotError> {
        if self.next_id == 0 {
            return Err(TodoSnapshotError::UnusableAllocator);
        }
        let mut seen = HashSet::with_capacity(self.tasks.len());
        for (position, task) in self.tasks.iter().enumerate() {
            if !seen.insert(task.id) {
                return Err(TodoSnapshotError::DuplicateId(task.id));
            }
            if task.id == 0 || task.id >= self.next_id {
                return Err(TodoSnapshotError::UnallocatedId {
                    id: task.id,
                    next_id: self.next_id,
                });
            }
            // The generation is dense and ordered, and there is no mutation
            // that could make it otherwise: ids are handed out one at a time
            // in creation order, `delete` tombstones in place rather than
            // removing, and `clear` starts a whole new generation at 1. A
            // gap, a reordering, or an allocator ahead of the tasks it
            // allocated is therefore a record no history could have produced.
            let expected = position as u64 + 1;
            if task.id != expected {
                return Err(TodoSnapshotError::OutOfCreationOrder {
                    position,
                    id: task.id,
                    expected,
                });
            }
            if task.subject.trim().is_empty() {
                return Err(TodoSnapshotError::BlankSubject(task.id));
            }
            for (field, value, multiline) in task.text_fields() {
                if let Some(character) = forbidden_control(value, multiline) {
                    return Err(TodoSnapshotError::ControlCharacter {
                        id: task.id,
                        field,
                        codepoint: character as u32,
                    });
                }
            }
            if let Some(metadata) = &task.metadata
                && let Some(character) = metadata_control(metadata)
            {
                return Err(TodoSnapshotError::ControlCharacter {
                    id: task.id,
                    field: "metadata",
                    codepoint: character as u32,
                });
            }
        }
        if self.next_id != self.tasks.len() as u64 + 1 {
            return Err(TodoSnapshotError::AllocatorAhead {
                next_id: self.next_id,
                tasks: self.tasks.len(),
            });
        }
        for task in &self.tasks {
            for blocker in &task.blocked_by {
                if *blocker == task.id {
                    return Err(TodoSnapshotError::SelfBlock(task.id));
                }
                if !seen.contains(blocker) {
                    return Err(TodoSnapshotError::UnknownDependency {
                        id: task.id,
                        blocker: *blocker,
                    });
                }
            }
            if task.blocked_by.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(TodoSnapshotError::UnnormalizedDependencies(task.id));
            }
            if closes_cycle(self, task) {
                return Err(TodoSnapshotError::DependencyCycle(task.id));
            }
        }
        Ok(())
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
    /// A task was left without a non-blank subject.
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
    /// The id allocator has no id left to hand out.
    AllocatorExhausted,
    /// The mutation would produce a list that could not be published and
    /// read back — the authority refuses to stage what a later rebuild
    /// would refuse to adopt.
    Unpublishable(TodoSnapshotError),
    /// The writing batch no longer holds the list: it has already settled,
    /// was discarded, or never opened. Provisional state belongs to the
    /// batch that will publish it, so a mutation with no such batch behind
    /// it writes nothing at all.
    BatchClosed,
}

impl core::fmt::Display for TodoMutationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BlankSubject => write!(f, "a task subject cannot be blank"),
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
            Self::AllocatorExhausted => write!(f, "the task id allocator is exhausted"),
            Self::Unpublishable(error) => write!(
                f,
                "the resulting list could not be read back after a restart: {error}"
            ),
            Self::BatchClosed => write!(
                f,
                "the task list is not open for this call: a task list mutation belongs to the \
                 tool-result batch that publishes it"
            ),
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
    inner: Arc<Mutex<TodoListState>>,
}

/// The committed list and the provisional one built on top of it.
///
/// `staged` is `None` exactly when nothing is in flight, so the common case
/// costs no clone and the authority is never a copy of itself.
///
/// Provisional state is never anonymous. `open` names the one batch that may
/// write, and the stage itself carries the batch that wrote it, so the two
/// questions a stage has to answer — *may this caller write?* and *whose list
/// is this?* — are both answered by data rather than by timing.
#[derive(Debug)]
struct TodoListState {
    committed: TodoSnapshot,
    staged: Option<StagedTodos>,
    open: Option<u64>,
    next_batch: u64,
}

/// One batch's provisional list, tagged with the batch that wrote it.
#[derive(Debug)]
struct StagedTodos {
    batch: u64,
    snapshot: TodoSnapshot,
}

impl TodoListState {
    /// The list `batch` reads and writes: its own stage, or the committed
    /// authority when it has staged nothing yet.
    ///
    /// A stage that belongs to another batch is invisible here rather than
    /// inherited, so no caller can read — or build on — provisional state it
    /// does not own.
    fn working_for(&self, batch: u64) -> &TodoSnapshot {
        match &self.staged {
            Some(staged) if staged.batch == batch => &staged.snapshot,
            _ => &self.committed,
        }
    }

    /// The list an observer sees: the in-flight one when a batch is
    /// mid-flight, the committed one otherwise. Never a write path.
    fn visible(&self) -> &TodoSnapshot {
        self.staged
            .as_ref()
            .map_or(&self.committed, |staged| &staged.snapshot)
    }
}

/// The right to write and settle one `ToolResult` batch's provisional list.
///
/// Opened by the Agent Loop before a batch runs and consumed when that batch
/// becomes canonical. Three properties make the token worth its existence:
///
/// - it is **exclusive**: a batch cannot be opened while another one is open,
///   so a second caller can never silently replace the batch that is running
///   and leave that batch unable to settle what it committed;
/// - it is the **only** way to mutate the list: every mutation goes through
///   the [`TodoWriter`] this token hands out, and a writer whose batch is no
///   longer open is refused. A caller with no batch identity cannot write
///   provisional state that a batch would then publish as its own;
/// - settling installs what the batch's own committed results published, not
///   whatever happens to be staged, so a batch that committed no `todo`
///   result moves nothing.
///
/// Dropping the token without settling discards the batch's provisional list,
/// which makes every early return of the Agent Loop — a durable failure, a
/// rejected append, a panic — leave the authority exactly as canonical
/// history describes it.
#[must_use = "an opened batch settles the list or discards it"]
#[derive(Debug)]
pub struct TodoBatch {
    list: ConversationTodoList,
    id: u64,
}

/// What settling one batch did to the authority.
///
/// Returned rather than swallowed: the whole point of the token is that the
/// in-memory list and canonical history cannot disagree, and a settlement
/// that quietly did nothing is exactly how they would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoSettlement {
    /// The batch's own canonical results published a list, and it is now the
    /// authority.
    Installed,
    /// The batch committed no `todo` result, so the authority did not move.
    Unchanged,
    /// The batch no longer held the list when it settled. The canonical list
    /// its own results published was still installed — canonical history is
    /// the authority, and refusing to install it is what would leave the
    /// process behind the Ledger — but something had already taken the list
    /// away from this batch, which the exclusivity of [`TodoBatch`] is meant
    /// to make impossible.
    Superseded,
}

impl TodoBatch {
    /// The batch-scoped mutation authority handed to this batch's executors.
    ///
    /// Cloning a writer shares the batch: two calls of one batch write one
    /// provisional list, which is exactly what makes a later call of the
    /// batch read what an earlier one did.
    #[must_use]
    pub fn writer(&self) -> TodoWriter {
        TodoWriter {
            list: self.list.clone(),
            batch: self.id,
        }
    }

    /// Installs the list this batch's own canonical results published.
    ///
    /// `blocks` is the exact `ToolResult` batch that just became canonical.
    /// The newest `todo` result in it *is* the list now; a batch that
    /// committed no `todo` result moves nothing, whatever was staged.
    pub fn settle(self, blocks: &[MessageBlock]) -> TodoSettlement {
        let list = self.list.clone();
        let mut state = list.state();
        let owned = state.open == Some(self.id);
        if owned {
            state.open = None;
            state.staged = None;
        }
        // Canonical history is the authority, so what these committed blocks
        // published is installed whether or not the token still owned the
        // stage. A settlement that skipped this step would leave the process
        // holding a list the Ledger has already superseded.
        let moved = if let Some(Ok(published)) = blocks.iter().rev().find_map(published_snapshot) {
            state.committed = published;
            true
        } else {
            false
        };
        drop(state);
        match (owned, moved) {
            (false, _) => TodoSettlement::Superseded,
            (true, true) => TodoSettlement::Installed,
            (true, false) => TodoSettlement::Unchanged,
        }
    }

    /// Drops this batch's provisional list, leaving the authority untouched.
    pub fn discard(self) {}
}

impl Drop for TodoBatch {
    fn drop(&mut self) {
        let mut state = self.list.state();
        if state.open == Some(self.id) {
            state.open = None;
            state.staged = None;
        }
    }
}

/// The mutation authority of one `ToolResult` batch.
///
/// Every `todo` mutation needs one, and a writer names the batch it belongs
/// to, so provisional state is written by an identified owner and read back
/// only by that owner. A writer whose batch has settled, been discarded, or
/// been dropped mutates nothing: it is refused with
/// [`TodoMutationError::BatchClosed`] rather than falling back to the
/// authority.
///
/// The Agent Loop hands one to the executors of the batch it opened
/// ([`crate::tools::executor::ToolExecutionContext::with_todos`]); an
/// executor driven outside that loop receives none and therefore cannot
/// write.
#[derive(Debug, Clone)]
pub struct TodoWriter {
    list: ConversationTodoList,
    batch: u64,
}

impl TodoWriter {
    /// The conversation this list belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        self.list.conversation_id()
    }

    /// The list this batch sees: the committed list plus whatever this batch
    /// has staged on top of it.
    ///
    /// Deliberately the *working* list rather than the authority, so a second
    /// `todo` call in one batch reads what the first one did — and never what
    /// some other batch did.
    ///
    /// # Errors
    ///
    /// Returns [`TodoMutationError::BatchClosed`] when this batch no longer
    /// holds the list. A closed batch reads nothing rather than reading the
    /// authority, so a stale writer cannot publish a list as if it were its
    /// own.
    pub fn snapshot(&self) -> Result<TodoSnapshot, TodoMutationError> {
        let state = self.list.state();
        self.guard(&state)?;
        Ok(state.working_for(self.batch).clone())
    }

    /// Adds one task in `pending` and returns it with the new snapshot.
    ///
    /// # Errors
    ///
    /// Returns the rejection of a closed batch, of a blank subject, or of an
    /// initial dependency that is unknown or tombstoned. Nothing is written
    /// when the call is rejected, so the id allocator does not advance
    /// either.
    pub fn create(&self, spec: TodoCreate) -> Result<(TodoTask, TodoSnapshot), TodoMutationError> {
        let subject = spec.subject.trim().to_owned();
        if subject.is_empty() {
            return Err(TodoMutationError::BlankSubject);
        }
        let mut state = self.list.state();
        self.guard(&state)?;
        let mut next = state.working_for(self.batch).clone();
        let blocked_by = normalize_ids(spec.blocked_by);
        for id in &blocked_by {
            check_dependency(&next, "blocked_by", *id)?;
        }
        let task = TodoTask {
            id: next.next_id,
            subject,
            description: spec.description,
            active_form: spec.active_form,
            status: TodoStatus::Pending,
            blocked_by,
            owner: spec.owner,
            metadata: spec.metadata.filter(|metadata| !metadata.is_empty()),
        };
        next.next_id = next
            .next_id
            .checked_add(1)
            .ok_or(TodoMutationError::AllocatorExhausted)?;
        next.tasks.push(task.clone());
        Ok((task, stage(&mut state, self.batch, next)?))
    }

    /// Applies one patch to one task.
    ///
    /// # Errors
    ///
    /// Returns the rejection of a closed batch, an unknown task, an empty
    /// patch, an illegal status transition, or a dependency edge that is
    /// unknown, tombstoned, self-referential, or cyclic. Validation completes
    /// before anything is written.
    pub fn update(
        &self,
        id: u64,
        change: TodoChange,
    ) -> Result<(TodoUpdate, TodoSnapshot), TodoMutationError> {
        if change.is_empty() {
            return Err(TodoMutationError::EmptyUpdate);
        }
        let mut state = self.list.state();
        self.guard(&state)?;
        let mut next = state.working_for(self.batch).clone();
        let position = next
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or(TodoMutationError::UnknownTask(id))?;
        let current = next.tasks[position].clone();

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
            // The same rule `create` applies, for the same reason: a task
            // with no subject is a row a client cannot draw and a snapshot a
            // rebuild refuses, so it must never become a published list.
            let subject = subject.trim().to_owned();
            if subject.is_empty() {
                return Err(TodoMutationError::BlankSubject);
            }
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
            check_dependency(&next, "add_blocked_by", *added)?;
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
        if !change.add_blocked_by.is_empty() && closes_cycle(&next, &updated) {
            return Err(TodoMutationError::DependencyCycle);
        }

        let unchanged = updated == current;
        let previous_status = (updated.status != current.status).then_some(current.status);
        next.tasks[position] = updated.clone();
        Ok((
            TodoUpdate {
                task: updated,
                previous_status,
                unchanged,
            },
            stage(&mut state, self.batch, next)?,
        ))
    }

    /// Tombstones one task.
    ///
    /// The task is kept as a `deleted` record so historical dependency
    /// references still resolve; it is never removed from the list.
    ///
    /// # Errors
    ///
    /// Returns the rejection of a closed batch, of an unknown task, or of a
    /// task that is already tombstoned.
    pub fn delete(&self, id: u64) -> Result<(TodoTask, TodoSnapshot), TodoMutationError> {
        let mut state = self.list.state();
        self.guard(&state)?;
        let mut next = state.working_for(self.batch).clone();
        let position = next
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or(TodoMutationError::UnknownTask(id))?;
        if next.tasks[position].status == TodoStatus::Deleted {
            return Err(TodoMutationError::AlreadyDeleted(id));
        }
        next.tasks[position].status = TodoStatus::Deleted;
        let task = next.tasks[position].clone();
        Ok((task, stage(&mut state, self.batch, next)?))
    }

    /// Drops every task and resets the id allocator.
    ///
    /// Returns how many tasks were dropped together with the emptied
    /// snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`TodoMutationError::BatchClosed`] when this batch no longer
    /// holds the list.
    pub fn clear(&self) -> Result<(usize, TodoSnapshot), TodoMutationError> {
        let mut state = self.list.state();
        self.guard(&state)?;
        let dropped = state.working_for(self.batch).tasks.len();
        let emptied = TodoSnapshot::empty();
        Ok((dropped, stage(&mut state, self.batch, emptied)?))
    }

    /// Refuses every write and every read of a batch that no longer holds
    /// the list.
    fn guard(&self, state: &TodoListState) -> Result<(), TodoMutationError> {
        if state.open == Some(self.batch) {
            Ok(())
        } else {
            Err(TodoMutationError::BatchClosed)
        }
    }
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
        Self::over(conversation_id, TodoSnapshot::empty())
    }

    /// Creates the list of one conversation over an already committed
    /// snapshot, with nothing staged.
    ///
    /// Deliberately private. `committed` becomes the authority without
    /// passing [`TodoSnapshot::validate`], so the only two callers are the
    /// ones that cannot carry an unusable list: [`Self::new`], whose
    /// snapshot is empty, and [`Self::rebuilt`], which validates what it
    /// read from canonical history first.
    fn over(conversation_id: ConversationId, committed: TodoSnapshot) -> Self {
        Self {
            conversation_id,
            inner: Arc::new(Mutex::new(TodoListState {
                committed,
                staged: None,
                open: None,
                next_batch: 1,
            })),
        }
    }

    /// Rebuilds one conversation's list from its canonical history.
    ///
    /// The snapshot the **newest** successful `todo` result published *is*
    /// the list: later results supersede earlier ones completely, so
    /// replaying the whole history is unnecessary and a partially
    /// reconstructed list is impossible. History that contains no such
    /// result yields the empty list, which is exactly what a conversation
    /// that never tracked tasks should have.
    ///
    /// # Errors
    ///
    /// Returns [`TodoRebuildError`] when that newest result carries no
    /// usable list. The rebuild fails closed rather than reaching further
    /// back: an older snapshot is a list this conversation has already
    /// superseded, and adopting it would revive tasks that were completed,
    /// tombstoned, or cleared.
    pub fn rebuilt(
        conversation_id: ConversationId,
        history: &[MessageBlock],
    ) -> Result<Self, TodoRebuildError> {
        let committed = match last_snapshot(history) {
            Some(result) => result?,
            None => TodoSnapshot::empty(),
        };
        Ok(Self::over(conversation_id, committed))
    }

    /// The conversation this list belongs to.
    #[must_use]
    pub const fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// What an observer of this list currently sees: the in-flight list
    /// while a batch is running, the committed list otherwise.
    ///
    /// This is a read, not a write path, and it is nobody's authority: a
    /// caller that needs the conversation's durable truth — a client
    /// projection, a bootstrap seed — uses [`Self::committed`], and a caller
    /// that needs to *mutate* uses the [`TodoWriter`] of its own batch.
    #[must_use]
    pub fn snapshot(&self) -> TodoSnapshot {
        self.state().visible().clone()
    }

    /// The authoritative list: exactly what canonical history committed.
    #[must_use]
    pub fn committed(&self) -> TodoSnapshot {
        self.state().committed.clone()
    }

    /// Whether a batch has staged mutations that have not committed yet.
    #[must_use]
    pub fn has_staged(&self) -> bool {
        self.state().staged.is_some()
    }

    /// Opens one `ToolResult` batch over this list, when one may be opened.
    ///
    /// The new batch starts from the committed authority: any provisional
    /// state left behind by something that never committed is dropped here,
    /// so a batch can neither read nor inherit it. See [`TodoBatch`].
    ///
    /// Returns `None` rather than taking the list away from a batch that
    /// already holds it. Silently replacing the open batch is the one way
    /// this type could let canonical history and the in-memory list
    /// disagree: the replaced batch would still commit its results, and its
    /// settlement would then find a list it no longer owns. A caller that
    /// cannot open a batch runs without one — its `todo` calls are refused,
    /// and the authority is untouched — which is the failure this list is
    /// designed to have.
    ///
    /// `None` is also returned in the unreachable case of an exhausted batch
    /// allocator, for the same reason: a reused batch id is an identity that
    /// no longer identifies.
    #[must_use]
    pub fn open_batch(&self) -> Option<TodoBatch> {
        let mut state = self.state();
        if state.open.is_some() {
            return None;
        }
        let id = state.next_batch;
        state.next_batch = state.next_batch.checked_add(1)?;
        state.open = Some(id);
        state.staged = None;
        drop(state);
        Some(TodoBatch {
            list: self.clone(),
            id,
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, TodoListState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Stages one candidate list, but only if it is one a rebuild could adopt.
///
/// Every mutation goes through here, so the authority never holds — and
/// never publishes — a list that [`TodoSnapshot::validate`] would refuse.
/// Without this the two halves of the durability story could drift: a call
/// could succeed, commit its snapshot, and leave the next restart unable to
/// rebuild the very list it just wrote.
fn stage(
    state: &mut TodoListState,
    batch: u64,
    next: TodoSnapshot,
) -> Result<TodoSnapshot, TodoMutationError> {
    next.validate().map_err(TodoMutationError::Unpublishable)?;
    state.staged = Some(StagedTodos {
        batch,
        snapshot: next.clone(),
    });
    Ok(next)
}

/// The list one canonical message publishes, when it publishes one.
///
/// `None` means "this message is not a settled `todo` result": another
/// tool, or a rejected call, which publishes nothing and therefore never
/// replaces the list. `Some(Err(..))` means it *is* the record the list
/// must come from and that record is unusable — never a reason to look at
/// an older one.
#[must_use]
pub fn published_snapshot(
    message: &MessageBlock,
) -> Option<Result<TodoSnapshot, TodoRebuildError>> {
    let MessageBlock::Tool(tool) = message else {
        return None;
    };
    if tool.tool_id != ToolId::new(TODO_TOOL_ID)
        || tool.result.status != ToolExecutionStatus::Success
    {
        return None;
    }
    let Some(value) = tool
        .result
        .content
        .iter()
        .find_map(|content| match content {
            ToolResultContent::Json { value } => Some(value),
            _ => None,
        })
    else {
        return Some(Err(TodoRebuildError::Missing));
    };
    Some(decode_snapshot(value))
}

/// Decodes and validates one published list, strictly.
fn decode_snapshot(value: &serde_json::Value) -> Result<TodoSnapshot, TodoRebuildError> {
    let snapshot: TodoSnapshot = serde_json::from_value(value.clone())
        .map_err(|error| TodoRebuildError::Undecodable(error.to_string()))?;
    snapshot.validate().map_err(TodoRebuildError::Invalid)?;
    Ok(snapshot)
}

/// The list the newest successful `todo` result of `history` published.
///
/// The search stops at that result whatever it contains: it is the only
/// record that can be the list, so an unusable one is an error rather than
/// a reason to keep walking backwards into a superseded list.
fn last_snapshot(history: &[MessageBlock]) -> Option<Result<TodoSnapshot, TodoRebuildError>> {
    history.iter().rev().find_map(published_snapshot)
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
        ConversationTodoList, TODO_TOOL_ID, TodoBatch, TodoChange, TodoCreate, TodoMutationError,
        TodoRebuildError, TodoSettlement, TodoSnapshot, TodoSnapshotError, TodoStatus, TodoTask,
        TodoWriter, forbidden_control,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, ToolMessageBlock};
    use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

    fn list() -> ConversationTodoList {
        ConversationTodoList::new(ConversationId::new("conv-todo"))
    }

    /// A list, the one batch open over it, and that batch's writer.
    ///
    /// Every mutation below goes through a writer because in the runtime
    /// every mutation does: provisional state belongs to the batch that will
    /// publish it, and there is no other way to write it.
    fn open() -> (ConversationTodoList, TodoBatch, TodoWriter) {
        let list = list();
        let batch = list.open_batch().expect("a fresh list opens one batch");
        let writer = batch.writer();
        (list, batch, writer)
    }

    /// The batch-scoped working list, for the many tests that only care what
    /// the mutation did.
    fn working(todos: &TodoWriter) -> TodoSnapshot {
        todos.snapshot().expect("the batch is open")
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
        let (_list, _batch, todos) = open();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, snapshot) = todos.create(subject("Write the tests")).expect("create");
        assert_eq!((first.id, second.id), (1, 2));
        assert_eq!(first.status, TodoStatus::Pending);
        assert_eq!(snapshot.next_id, 3);
    }

    #[test]
    fn a_blank_subject_creates_nothing() {
        let (_list, _batch, todos) = open();
        assert_eq!(
            todos.create(subject("   ")).expect_err("blank subject"),
            TodoMutationError::BlankSubject
        );
        assert_eq!(working(&todos), TodoSnapshot::empty());
    }

    #[test]
    fn completed_work_may_only_be_tombstoned() {
        let (_list, _batch, todos) = open();
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
        let (_list, _batch, todos) = open();
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
        let (_list, _batch, todos) = open();
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
        let (_list, _batch, todos) = open();
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
            working(&todos).next_id,
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
            working(&todos).task(first.id).expect("first").blocked_by,
            Vec::<u64>::new(),
            "the rejected edge was never written"
        );
    }

    #[test]
    fn a_tombstone_stays_referenceable_but_cannot_be_a_new_dependency() {
        let (_list, _batch, todos) = open();
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

        let snapshot = working(&todos);
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
        let (_list, _batch, todos) = open();
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
        let (_list, _batch, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        todos.create(subject("Write the tests")).expect("create");
        let (dropped, snapshot) = todos.clear().expect("clear");
        assert_eq!(dropped, 2);
        assert_eq!(snapshot, TodoSnapshot::empty());
        let (task, _) = todos.create(subject("Start over")).expect("create");
        assert_eq!(task.id, 1);
    }

    #[test]
    fn progress_counts_live_tasks_only() {
        let (_list, _batch, todos) = open();
        let (first, _) = todos.create(subject("Write the parser")).expect("create");
        let (second, _) = todos.create(subject("Write the tests")).expect("create");
        todos.create(subject("Ship")).expect("create");
        todos
            .update(first.id, status(TodoStatus::Completed))
            .expect("complete");
        todos.delete(second.id).expect("tombstone");
        assert_eq!(working(&todos).progress(), (1, 2));
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
        let (_list, _batch, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        let first = working(&todos);
        let (task, _) = todos.create(subject("Write the tests")).expect("create");
        todos
            .update(task.id, status(TodoStatus::InProgress))
            .expect("start");
        let latest = working(&todos);

        let history = vec![todo_result(&first), todo_result(&latest)];
        let rebuilt = ConversationTodoList::rebuilt(ConversationId::new("conv-todo"), &history)
            .expect("rebuild");
        assert_eq!(rebuilt.committed(), latest);

        // The rebuilt list keeps allocating where the conversation left off.
        let batch = rebuilt.open_batch().expect("a rebuilt list opens a batch");
        let (next, _) = batch.writer().create(subject("Ship")).expect("create");
        assert_eq!(next.id, 3);
    }

    #[test]
    fn a_mutation_is_staged_until_the_batch_that_carries_it_commits() {
        let (list, batch, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        assert!(list.has_staged());
        assert_eq!(
            list.committed(),
            TodoSnapshot::empty(),
            "the authority does not move before the result is durable"
        );
        assert_eq!(
            working(&todos).tasks.len(),
            1,
            "the batch itself reads what it has staged"
        );

        // A second call of the same batch composes on the staged list.
        let (second, _) = todos.create(subject("Write the tests")).expect("create");
        assert_eq!(second.id, 2);

        let staged = working(&todos);
        assert_eq!(staged.tasks.len(), 2);
        assert_eq!(
            batch.settle(&[todo_result(&staged)]),
            TodoSettlement::Installed
        );
        assert_eq!(list.committed(), staged);
        assert!(!list.has_staged());
        assert_eq!(
            todos
                .create(subject("Too late"))
                .expect_err("settled batch"),
            TodoMutationError::BatchClosed,
            "a writer whose batch has settled writes nothing"
        );
    }

    /// One batch at a time, and the running one keeps its list.
    ///
    /// Overlapping opens are the one way this type could let canonical
    /// history and the in-memory list disagree: the displaced batch would
    /// still commit its own results, and its settlement would then find a
    /// list it no longer owned.
    #[test]
    fn a_second_batch_cannot_take_the_list_from_the_one_that_holds_it() {
        let (list, first, todos) = open();
        todos.create(subject("Write the parser")).expect("create");

        assert!(
            list.open_batch().is_none(),
            "a batch already holds the list"
        );
        assert_eq!(
            working(&todos).tasks.len(),
            1,
            "the refused open left the running batch's list exactly as it was"
        );

        let staged = working(&todos);
        assert_eq!(
            first.settle(&[todo_result(&staged)]),
            TodoSettlement::Installed,
            "the batch that committed the results is still the batch that owns the list"
        );
        assert_eq!(list.committed(), staged);

        let second = list
            .open_batch()
            .expect("the settled batch released the list");
        let (next, _) = second
            .writer()
            .create(subject("Write the tests"))
            .expect("create");
        assert_eq!(next.id, 2, "the next batch composes on the committed list");
    }

    /// A stranded stage is neither readable nor writable by anyone else.
    ///
    /// This is the executor-driven case: something holding the public tool
    /// registry runs `todo` outside any batch. It has no writer, so there is
    /// nothing to strand — and the batch that runs next commits a list it
    /// built itself.
    #[test]
    fn a_call_that_owns_no_batch_writes_nothing_at_all() {
        let list = list();
        let orphan = {
            let batch = list.open_batch().expect("open");
            let writer = batch.writer();
            batch.discard();
            writer
        };
        assert_eq!(
            orphan
                .create(subject("Written outside a batch"))
                .expect_err("no batch"),
            TodoMutationError::BatchClosed
        );
        assert!(!list.has_staged());

        let batch = list.open_batch().expect("open");
        assert_eq!(
            orphan
                .create(subject("Written beside a running batch"))
                .expect_err("not this batch"),
            TodoMutationError::BatchClosed,
            "a stale writer cannot insert a task into the batch that is running"
        );
        assert_eq!(
            orphan.snapshot().expect_err("not this batch"),
            TodoMutationError::BatchClosed,
            "nor read the list of the batch that is running"
        );
        let todos = batch.writer();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        assert_eq!(task.id, 1, "no stranded write moved the allocator");
        assert_eq!(
            working(&todos).tasks.len(),
            1,
            "the running batch publishes only what the running batch wrote"
        );

        // The batch commits an ordinary non-`todo` result: nothing it
        // committed published a list, so the authority does not move.
        assert_eq!(
            batch.settle(&[MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("message-bash"),
                tool_call_id: ToolCallId::new("call-bash"),
                tool_id: ToolId::new("tool-bash"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: vec![ToolResultContent::Text(TextBlock {
                        text: "ok".to_owned(),
                    })],
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            })]),
            TodoSettlement::Unchanged
        );
        assert_eq!(
            list.committed(),
            TodoSnapshot::empty(),
            "no committed result published a list, so no list was installed"
        );
        assert!(!list.has_staged());
    }

    /// A settled batch installs its newest published list even if a later
    /// mutation staged something else after it.
    #[test]
    fn a_batch_installs_the_newest_list_its_own_results_published() {
        let (list, batch, todos) = open();
        todos.create(subject("First")).expect("create");
        let first = working(&todos);
        todos.create(subject("Second")).expect("create");
        let second = working(&todos);
        assert_eq!(
            batch.settle(&[todo_result(&first), todo_result(&second)]),
            TodoSettlement::Installed
        );
        assert_eq!(list.committed(), second);
    }

    #[test]
    fn a_dropped_batch_leaves_the_committed_list_exactly_as_it_was() {
        let (list, first, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        first.settle(&[todo_result(&working(&todos))]);
        let committed = list.committed();

        let doomed = list.open_batch().expect("open");
        let doomed_writer = doomed.writer();
        doomed_writer
            .create(subject("Write the tests"))
            .expect("create");
        doomed_writer.delete(1).expect("tombstone");
        let (_, cleared) = doomed_writer.clear().expect("clear");
        assert_eq!(cleared, TodoSnapshot::empty());

        // The Agent Loop's every non-commit exit, in one line.
        drop(doomed);
        assert!(!list.has_staged());
        assert_eq!(list.committed(), committed);
        assert_eq!(
            list.snapshot(),
            committed,
            "with nothing staged, the observable list is the committed one"
        );
        assert_eq!(
            doomed_writer.snapshot().expect_err("the batch is gone"),
            TodoMutationError::BatchClosed,
            "the writer of a dropped batch reads nothing, least of all the authority"
        );
    }

    #[test]
    fn a_rejected_mutation_stages_nothing() {
        let (list, batch, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        batch.settle(&[todo_result(&working(&todos))]);
        assert!(!list.has_staged());

        let batch = list.open_batch().expect("open");
        let todos = batch.writer();
        todos
            .update(9, status(TodoStatus::Completed))
            .expect_err("unknown task");
        assert!(
            !list.has_staged(),
            "a rejected call leaves no provisional list to commit or discard"
        );
    }

    /// `update` obeys the same subject rule `create` does.
    ///
    /// It did not, and the gap was publishable: a blank subject passed the
    /// schema's `minLength` (whitespace is characters) and the control-
    /// character rule, so the call succeeded and committed a snapshot that
    /// the next restart refused to rebuild.
    #[test]
    fn an_update_may_not_blank_out_a_subject() {
        let (_list, _batch, todos) = open();
        let (task, _) = todos.create(subject("Write the parser")).expect("create");
        assert_eq!(
            todos
                .update(
                    task.id,
                    TodoChange {
                        subject: Some("   ".to_owned()),
                        ..TodoChange::default()
                    },
                )
                .expect_err("a blank subject"),
            TodoMutationError::BlankSubject
        );
        assert_eq!(
            working(&todos).task(1).expect("kept").subject,
            "Write the parser"
        );

        let (updated, snapshot) = todos
            .update(
                task.id,
                TodoChange {
                    subject: Some("  Write the lexer  ".to_owned()),
                    ..TodoChange::default()
                },
            )
            .expect("update");
        assert_eq!(
            updated.task.subject, "Write the lexer",
            "trimmed exactly like create"
        );
        snapshot
            .validate()
            .expect("a published list is always one a rebuild can adopt");
    }

    /// Every mutation is checked against the rule a rebuild applies, so the
    /// authority can never publish a list it could not read back.
    #[test]
    fn a_mutation_that_could_not_be_read_back_is_refused() {
        let (_list, _batch, todos) = open();
        let (task, snapshot) = todos.create(subject("Write the parser")).expect("create");
        snapshot.validate().expect("valid");
        for blank in ["", "  ", "\u{2028}  "] {
            todos
                .update(
                    task.id,
                    TodoChange {
                        subject: Some(blank.to_owned()),
                        ..TodoChange::default()
                    },
                )
                .expect_err("unpublishable");
        }
        assert_eq!(working(&todos), snapshot, "and nothing was written");
    }

    #[test]
    fn every_character_a_terminal_row_cannot_hold_is_named() {
        for character in [
            '\n', '\r', '\t', '\u{1b}', '\u{7f}', '\u{9b}', '\u{202e}', '\u{2066}',
        ] {
            assert_eq!(
                forbidden_control(&format!("ship{character}now"), false),
                Some(character),
                "U+{:04X} is not a character one panel row may carry",
                character as u32
            );
        }
        assert_eq!(forbidden_control("ship it — now", false), None);
        assert_eq!(
            forbidden_control("first\nsecond", true),
            None,
            "the long-form field keeps its line breaks"
        );
        assert_eq!(
            forbidden_control("first\tsecond", true),
            Some('\t'),
            "and nothing else"
        );
    }

    #[test]
    fn the_default_list_is_the_empty_list_allocator_included() {
        assert_eq!(TodoSnapshot::default(), TodoSnapshot::empty());
        assert_eq!(TodoSnapshot::default().next_id, 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one table of rejected records
    fn a_published_list_that_no_mutation_could_have_produced_is_refused() {
        let task = |id: u64, subject: &str| TodoTask {
            id,
            subject: subject.to_owned(),
            description: None,
            active_form: None,
            status: TodoStatus::Pending,
            blocked_by: Vec::new(),
            owner: None,
            metadata: None,
        };
        let invalid = [
            (
                TodoSnapshot {
                    tasks: vec![task(1, "a"), task(1, "b")],
                    next_id: 2,
                },
                TodoSnapshotError::DuplicateId(1),
            ),
            (
                TodoSnapshot {
                    tasks: vec![task(7, "a")],
                    next_id: 2,
                },
                TodoSnapshotError::UnallocatedId { id: 7, next_id: 2 },
            ),
            (
                TodoSnapshot {
                    tasks: vec![task(1, "   ")],
                    next_id: 2,
                },
                TodoSnapshotError::BlankSubject(1),
            ),
            (
                TodoSnapshot {
                    tasks: vec![TodoTask {
                        blocked_by: vec![1],
                        ..task(1, "a")
                    }],
                    next_id: 2,
                },
                TodoSnapshotError::SelfBlock(1),
            ),
            (
                TodoSnapshot {
                    tasks: vec![TodoTask {
                        blocked_by: vec![4],
                        ..task(1, "a")
                    }],
                    next_id: 2,
                },
                TodoSnapshotError::UnknownDependency { id: 1, blocker: 4 },
            ),
            (
                TodoSnapshot {
                    tasks: vec![TodoTask {
                        subject: "safe\nspoofed".to_owned(),
                        ..task(1, "a")
                    }],
                    next_id: 2,
                },
                TodoSnapshotError::ControlCharacter {
                    id: 1,
                    field: "subject",
                    codepoint: 0x000a,
                },
            ),
            (
                TodoSnapshot {
                    tasks: vec![TodoTask {
                        metadata: Some(
                            [("owner\u{1b}]0;owned".to_owned(), serde_json::json!("me"))]
                                .into_iter()
                                .collect(),
                        ),
                        ..task(1, "a")
                    }],
                    next_id: 2,
                },
                TodoSnapshotError::ControlCharacter {
                    id: 1,
                    field: "metadata",
                    codepoint: 0x001b,
                },
            ),
            // The generation is dense and ordered. None of these is a list
            // any sequence of mutations could have left behind, and each one
            // would hand a rebuilt conversation ids that name nothing.
            (
                TodoSnapshot {
                    tasks: Vec::new(),
                    next_id: 9,
                },
                TodoSnapshotError::AllocatorAhead {
                    next_id: 9,
                    tasks: 0,
                },
            ),
            (
                TodoSnapshot {
                    tasks: vec![task(1, "a"), task(3, "c")],
                    next_id: 4,
                },
                TodoSnapshotError::OutOfCreationOrder {
                    position: 1,
                    id: 3,
                    expected: 2,
                },
            ),
            (
                TodoSnapshot {
                    tasks: vec![task(2, "b"), task(1, "a")],
                    next_id: 3,
                },
                TodoSnapshotError::OutOfCreationOrder {
                    position: 0,
                    id: 2,
                    expected: 1,
                },
            ),
            (
                TodoSnapshot {
                    tasks: vec![task(1, "a"), task(2, "b")],
                    next_id: 9,
                },
                TodoSnapshotError::AllocatorAhead {
                    next_id: 9,
                    tasks: 2,
                },
            ),
        ];
        for (snapshot, expected) in invalid {
            assert_eq!(snapshot.validate().expect_err("invalid"), expected);
        }
        assert!(
            TodoSnapshot {
                tasks: vec![
                    task(1, "a"),
                    TodoTask {
                        blocked_by: vec![1],
                        ..task(2, "b")
                    },
                ],
                next_id: 3,
            }
            .validate()
            .is_ok()
        );
    }

    /// A rebuild never reaches past an unusable record into a list the
    /// conversation has already superseded — that would revive tasks the
    /// model completed, tombstoned, or cleared.
    #[test]
    fn an_unusable_newest_snapshot_fails_the_rebuild_instead_of_reviving_an_older_one() {
        let (_list, _batch, todos) = open();
        todos.create(subject("Write the parser")).expect("create");
        let superseded = working(&todos);

        let mut malformed = todo_result(&TodoSnapshot::empty());
        let MessageBlock::Tool(tool) = &mut malformed else {
            panic!("a tool result");
        };
        tool.result.content = vec![ToolResultContent::Json {
            value: serde_json::json!({ "tasks": [{ "id": 1 }], "next_id": 2 }),
        }];

        let history = vec![todo_result(&superseded), malformed];
        let error = ConversationTodoList::rebuilt(ConversationId::new("conv-todo"), &history)
            .expect_err("the newest record is unusable");
        assert!(
            matches!(error, TodoRebuildError::Undecodable(_)),
            "{error:?}"
        );
    }

    #[test]
    fn a_successful_result_without_a_list_is_a_rebuild_failure_not_an_empty_list() {
        let mut missing = todo_result(&TodoSnapshot::empty());
        let MessageBlock::Tool(tool) = &mut missing else {
            panic!("a tool result");
        };
        tool.result.content = vec![ToolResultContent::Text(TextBlock {
            text: "Created #1".to_owned(),
        })];
        assert_eq!(
            ConversationTodoList::rebuilt(ConversationId::new("conv-todo"), &[missing])
                .expect_err("no list"),
            TodoRebuildError::Missing
        );
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
        )
        .expect("rebuild");
        assert_eq!(rebuilt.snapshot(), TodoSnapshot::empty());
    }
}
