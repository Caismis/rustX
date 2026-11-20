//! The conversation-owned Todo vocabulary: task identity, status transition
//! law, bounded status presentation, the published snapshot contract, and the
//! deterministic rebuild of the latest snapshot from canonical history.
//! Shared by the tool implementation (mutation), the journal (snapshot
//! projection and `is_publication` tagging), and the Agent Status composition.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::message::types::MessageBlock;
use crate::tools::types::{ToolExecutionStatus, ToolResultContent};
use std::collections::HashSet;

pub const TODO_TOOL_ID: &str = "tool-todo";

/// The maximum number of Todo tasks included in one bounded status
/// presentation. The current in-progress task, when any, uses one slot.
pub const MAX_TODO_STATUS_TASKS: usize = 6;

/// The maximum byte length of one Todo task label included in the bounded
/// status presentation.
pub const MAX_TODO_STATUS_TEXT_BYTES: usize = 256;

/// One bounded Todo task of the read-only status presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoStatusTask {
    /// The conversation-owned task id.
    pub id: u64,
    /// The bounded task subject.
    pub subject: String,
    /// The bounded in-progress label, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The authoritative committed lifecycle status.
    pub status: TodoStatus,
    /// Whether an active dependency still blocks this task.
    pub blocked: bool,
}

/// The bounded, **read-only** semantic Todo presentation (Issue #259).
///
/// This is the whole interface the Todo extension offers another extension.
/// It is derived here, by the list's owner, from the *committed* snapshot —
/// the conversation's durable truth — and it is a value, not a handle: a
/// consumer can render it and fingerprint it, and can neither mutate the
/// list, drive its recovery, nor observe a mutation some batch has staged but
/// not committed.
///
/// Agent Status is the only consumer today. It decides *whether and how often*
/// to show a reminder; it never decides what the list is.
///
/// Tasks are in conversation creation order. `current` is the first committed
/// `InProgress` task, when any; `tasks` contains the remaining committed active
/// tasks up to the explicit bound. Counts cover the complete committed
/// snapshot, while `omitted_count` is the number of active tasks not shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoStatusPresentation {
    /// The first committed `InProgress` task, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<TodoStatusTask>,
    /// Remaining active tasks in deterministic creation order.
    #[serde(default)]
    pub tasks: Vec<TodoStatusTask>,
    /// Number of committed Pending or `InProgress` tasks.
    pub active_count: usize,
    /// Number of active tasks with an unresolved active dependency.
    pub blocked_count: usize,
    /// Number of committed Completed tasks.
    pub completed_count: usize,
    /// Number of committed Deleted tasks.
    pub deleted_count: usize,
    /// Number of active tasks omitted by the bounded presentation.
    pub omitted_count: usize,
}

impl TodoStatusPresentation {
    /// Whether this presentation contains work a reminder could be about.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        self.active_count > 0
    }

    /// The stable content fingerprint of this presentation.
    ///
    /// Owned here because the presentation is owned here: a consumer that
    /// computed its own fingerprint over its own rendering would be deciding
    /// when two Todo states are "the same", which is the list's question.
    ///
    /// # Panics
    ///
    /// Only if this value stops being serializable, which its own derive
    /// makes a compile-time property rather than a runtime one.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("Todo status presentation is serializable");
        let digest = <sha2::Sha256 as sha2::Digest>::digest(encoded);
        format!("{digest:x}")
    }
}

/// The lifecycle status of one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
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
#[derive(schemars::JsonSchema)]
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
///   bidi controls can repaint colours, move the cursor, retitle the
///   window, or reverse the reading order of text around them.
///
/// The bidi half is Unicode's `Bidi_Control` property, and it is refused
/// *whole*: U+061C, U+200E, U+200F, U+202A–U+202E, and U+2066–U+2069 — twelve
/// code points, not the eleven that look alike. U+061C ARABIC LETTER MARK is
/// the one that hides: it is `Cf`, not a control character, so
/// [`char::is_control`] does not see it, and a set assembled by eye from the
/// `LRM`/`RLM`/embedding/isolate families leaves it out. It reverses reading
/// order exactly as its neighbours do.
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
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}')
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
/// publishes, and it is what `ConversationTodoList::rebuilt` reads back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
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

    /// The bounded read-only [`TodoStatusPresentation`] of this list.
    ///
    /// Deriving it is the list's own business: the bounds, the choice of the
    /// current task, the blocked predicate, and the truncation rule are all
    /// facts about the task model, not about any consumer's rendering.
    #[must_use]
    pub fn status_presentation(&self) -> TodoStatusPresentation {
        let states = self
            .tasks
            .iter()
            .map(|task| (task.id, task.status))
            .collect::<BTreeMap<_, _>>();
        let blocked = |task: &TodoTask| {
            task.blocked_by.iter().any(|blocker| {
                states.get(blocker).is_some_and(|status| {
                    matches!(status, TodoStatus::Pending | TodoStatus::InProgress)
                })
            })
        };
        let active = self
            .tasks
            .iter()
            .filter(|task| matches!(task.status, TodoStatus::Pending | TodoStatus::InProgress));
        let active_count = active.clone().count();
        let blocked_count = active.clone().filter(|task| blocked(task)).count();
        let completed_count = self
            .tasks
            .iter()
            .filter(|task| task.status == TodoStatus::Completed)
            .count();
        let deleted_count = self
            .tasks
            .iter()
            .filter(|task| task.status == TodoStatus::Deleted)
            .count();

        let current_id = self
            .tasks
            .iter()
            .find(|task| task.status == TodoStatus::InProgress)
            .map(|task| task.id);
        let mut current = None;
        let mut tasks = Vec::new();
        let task_limit = if current_id.is_some() {
            MAX_TODO_STATUS_TASKS.saturating_sub(1)
        } else {
            MAX_TODO_STATUS_TASKS
        };
        for task in active {
            let bounded = TodoStatusTask {
                id: task.id,
                subject: bound_status_text(&task.subject),
                active_form: task.active_form.as_deref().map(bound_status_text),
                status: task.status,
                blocked: blocked(task),
            };
            if Some(task.id) == current_id {
                current = Some(bounded);
            } else if tasks.len() < task_limit {
                tasks.push(bounded);
            }
        }
        let displayed = usize::from(current.is_some()) + tasks.len();
        TodoStatusPresentation {
            current,
            tasks,
            active_count,
            blocked_count,
            completed_count,
            deleted_count,
            omitted_count: active_count.saturating_sub(displayed),
        }
    }
}

/// Bounds one presentation field to [`MAX_TODO_STATUS_TEXT_BYTES`] on a UTF-8
/// character boundary.
fn bound_status_text(text: &str) -> String {
    if text.len() <= MAX_TODO_STATUS_TEXT_BYTES {
        return text.to_owned();
    }
    let marker = "…";
    let limit = MAX_TODO_STATUS_TEXT_BYTES.saturating_sub(marker.len());
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = text[..end].to_owned();
    bounded.push_str(marker);
    bounded
}
pub fn published_snapshot(
    message: &MessageBlock,
) -> Option<Result<TodoSnapshot, TodoRebuildError>> {
    if !is_publication(message) {
        return None;
    }
    let MessageBlock::Tool(tool) = message else {
        return None;
    };
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

/// Whether `message` is a successful `todo` result: the messages whose
/// newest instance is the list (see [`published_snapshot`]).
#[must_use]
pub fn is_publication(message: &MessageBlock) -> bool {
    matches!(message, MessageBlock::Tool(tool)
        if tool.tool_id.as_str() == TODO_TOOL_ID
            && tool.result.status == ToolExecutionStatus::Success)
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
pub fn last_snapshot(history: &[MessageBlock]) -> Option<Result<TodoSnapshot, TodoRebuildError>> {
    history.iter().rev().find_map(published_snapshot)
}

/// Whether `candidate` can reach itself through the graph it would create.
pub fn closes_cycle(snapshot: &TodoSnapshot, candidate: &TodoTask) -> bool {
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
