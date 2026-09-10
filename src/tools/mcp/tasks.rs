//! The MCP Tasks extension (`io.modelcontextprotocol/tasks`, SEP-2663)
//! translation layer (Issue #243).
//!
//! # One invocation, one remote task, one settlement
//!
//! MCP `2026-07-28` lets a server answer `tools/call` with a
//! [`CreateTaskResult`] (`resultType: "task"`) instead of a `CallToolResult`.
//! That answer is **the remote execution state of the already-admitted rustX
//! `ToolInvocation`** moving into a task lifecycle. It is never a terminal
//! `ToolResult`, never a second rustX execution identity, never a rustX
//! background record, Scheduler job, `WorkflowRun`, Subagent, Goal, or Todo,
//! and never durable state:
//!
//! ```text
//! model `ToolCall` A
//!     |
//!     v
//! rustX `ToolInvocation` A ── approval evaluated once ──┐
//!     |                                                |
//!     +--> tools/call ------------------------------+  |
//!     |        |                                    |  |
//!     |        +--> `CallToolResult` ---------------+--+--> ONE terminal
//!     |        |                                    |  |    `ToolExecutionResult`
//!     |        +--> `InputRequiredResult` (SEP-2322)|  |
//!     |        |        -> one Questionnaire        |  |
//!     |        |        -> tools/call round N+1 ----+  |
//!     |        |                                    |  |
//!     |        +--> `CreateTaskResult`              |  |
//!     |                 |                           |  |
//!     |                 v                           |  |
//!     |          RemoteTaskActive                   |  |
//!     |                 |                           |  |
//!     |                 +--> tasks/get   (poll)     |  |
//!     |                 +--> tasks/update (answers) |  |
//!     |                 +--> tasks/cancel (best     |  |
//!     |                 |     effort, on rustX      |  |
//!     |                 |     abandonment)          |  |
//!     |                 v                           |  |
//!     |          terminal task state ---------------+--+
//! ```
//!
//! There is **no `tools/call` after task creation**: a materialized task is
//! the one remote execution of this invocation, so the round loop that owns
//! MRTR continuations is left behind for good the moment a task exists.
//!
//! # What this module owns, and what it deliberately does not
//!
//! This module owns the *translation and validation* of the task protocol:
//! what a task id is allowed to be, which snapshots are self-consistent, how
//! a server's polling hint becomes a cancellation-aware wait, which input
//! requests are still outstanding, and how a terminal task state becomes evidence the
//! existing MCP result projection can settle.
//!
//! It owns **no** dispatch. The physical requests, the external-effect
//! frontier, cancellation arbitration, local Streamable HTTP request
//! ownership, connection generations, and terminal settlement all stay in
//! [`super`], where the existing MCP execution semantics already live — the
//! polling driver composes them exactly as the MRTR round driver composes
//! them, and re-implements none of them.
//!
//! Human input is likewise not owned here. A task's `input_required` snapshot
//! carries the very [`InputRequests`] an `InputRequiredResult` carries, so the
//! whole typed-Questionnaire translation of [`super::mrtr`] is reused
//! unchanged: one runtime-owned `QuestionnaireRequester`, one bounded
//! Questionnaire, one typed response, one `inputResponses` map. There is no
//! task-specific interaction subsystem and no task-specific question
//! vocabulary.
//!
//! # Eventual consistency is a protocol fact, not an error
//!
//! SEP-2663 acknowledges `tasks/update` *before* the task's observable state
//! must reflect it, so a later `tasks/get` may legitimately report an input
//! request rustX has already answered:
//!
//! ```text
//! tasks/get     -> input_required { A }
//! Questionnaire -> answer A
//! tasks/update  { A } -> ACK
//! tasks/get     -> input_required { A }      <-- stale, and legal
//! tasks/get     -> working
//! ```
//!
//! rustX answers a request key **once per task**. [`RemoteTask`] remembers
//! every key it has answered for the lifetime of the task, so a repeated key
//! publishes no second interaction and sends no second update, while a
//! genuinely new key in a later snapshot is still processed. The set is
//! bounded: a server cannot grow this invocation's execution-local state
//! without limit by inventing keys.
//!
//! # `ttlMs` is retention metadata, not a deadline
//!
//! A task's `ttlMs` says how long the *server* may retain it. rustX already
//! has exactly one deadline authority — the generic Issue #204 execution
//! deadline the Agent Loop owns — and this module deliberately introduces no
//! second one. `pollIntervalMs` has a 25 ms minimum floor and no maximum
//! clamp. Every poll, including the first, waits for the server hint; local
//! cancellation and the outer deadline can interrupt that wait.

use std::collections::BTreeSet;
use std::time::Duration;

use rmcp::model::{
    CallToolResult, CreateTaskResult, ErrorData, GetTaskResult, InputRequests, TaskPayload,
    TaskStatus,
};

/// The most bytes of server-assigned task id rustX will retain and echo.
///
/// The id is protocol-owned and never parsed, so the only defensible bound is
/// a size bound: it is carried in every `tasks/get`, `tasks/update`, and
/// `tasks/cancel` this invocation sends.
pub(super) const MCP_TASK_ID_MAX_BYTES: usize = 512;

/// The most distinct input-request keys rustX will answer over the lifetime
/// of one remote task.
///
/// Answered keys are remembered so an eventually-consistent repeat is not
/// answered twice, which makes the set the one piece of task state a server
/// could otherwise grow without limit.
pub(super) const MCP_TASK_MAX_ANSWERED_REQUESTS: usize = 32;

/// The most bytes of `statusMessage` rustX will retain from a task snapshot.
pub(super) const MCP_TASK_MAX_STATUS_MESSAGE_BYTES: usize = 1024;

/// The wait between two `tasks/get` polls when the server offers no hint.
pub(super) const MCP_TASK_POLL_INTERVAL_DEFAULT: Duration = Duration::from_millis(500);

/// The shortest wait rustX will honour, whatever the server hints.
///
/// A server asking for `0` is asking for a busy loop over a network
/// transport; the floor refuses that without refusing the call.
pub(crate) const MCP_TASK_POLL_INTERVAL_MIN: Duration = Duration::from_millis(25);

/// Why one remote task cannot be driven by rustX.
///
/// Every violation is a **deterministic terminal diagnostic** of the owning
/// invocation, produced from a correlated remote answer. It fails that one
/// invocation and never poisons the connection generation: a server that
/// answers `tasks/get` with a self-contradictory snapshot has not corrupted
/// the transport, and unrelated healthy calls on the same peer keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct McpTaskViolation {
    /// The bounded model-facing diagnostic.
    pub(super) diagnostic: String,
}

impl McpTaskViolation {
    fn new(diagnostic: impl Into<String>) -> Self {
        Self {
            diagnostic: diagnostic.into(),
        }
    }
}

/// What one validated `tasks/get` snapshot means for the owning invocation.
///
/// The variants are the whole task contract: only the last three settle the
/// invocation, and `Working` is the only state that schedules another poll.
#[derive(Debug)]
pub(super) enum RemoteTaskObservation {
    /// The task is still executing — or is waiting only on input requests
    /// rustX has already answered, which is the protocol's own
    /// eventually-consistent view of an accepted `tasks/update`.
    Working,
    /// The task is waiting on input requests rustX has **not** answered yet.
    /// Only the outstanding subset is carried.
    InputRequired(Box<InputRequests>),
    /// The task finished and carries the final result of the original
    /// `tools/call`. A `completed` task may still carry `isError: true`:
    /// task completion is not tool business success.
    Completed(Box<CallToolResult>),
    /// The task failed with the JSON-RPC error the protocol requires.
    Failed(Box<ErrorData>),
    /// The task reached the protocol's `cancelled` terminal state.
    Cancelled,
}

/// The adapter-local remote-execution state of one rustX `ToolInvocation`.
///
/// It lives on the executor's stack for the lifetime of one invocation and is
/// dropped at terminal settlement. It never enters the Event Journal, the
/// Message Ledger, the Conversation Surface, Workflow/Goal/Subagent durable
/// state, the background registry, `SQLite`, the Runtime Client protocol, or a
/// TUI projection, and it mints no rustX identity of its own: the only
/// identity here is the server's own task id.
#[derive(Debug)]
pub(super) struct RemoteTask {
    /// The server-assigned task id, echoed verbatim in every follow-up.
    task_id: String,
    /// The last status the server reported, seeded from `CreateTaskResult`.
    status: TaskStatus,
    /// The current wait between polls, refreshed from every snapshot.
    poll_interval: Duration,
    /// Every input-request key this task has already been answered for.
    answered: BTreeSet<String>,
}

impl RemoteTask {
    /// Accepts one `CreateTaskResult` as this invocation's remote execution.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when the seed task is not one rustX can
    /// address: an empty or oversized id is not a usable protocol identity.
    pub(super) fn create(result: &CreateTaskResult) -> Result<Self, McpTaskViolation> {
        let task_id = result.task.task_id.clone();
        if task_id.is_empty() {
            return Err(McpTaskViolation::new(
                "the MCP server created a task with an empty task id",
            ));
        }
        if task_id.len() > MCP_TASK_ID_MAX_BYTES {
            return Err(McpTaskViolation::new(format!(
                "the MCP task id is {} bytes, above the {MCP_TASK_ID_MAX_BYTES}-byte rustX bound",
                task_id.len()
            )));
        }
        Ok(Self {
            task_id,
            status: result.task.status,
            poll_interval: poll_interval(result.task.poll_interval_ms),
            answered: BTreeSet::new(),
        })
    }

    /// Validate creation metadata after accepting the independently usable id.
    pub(super) fn validate_creation(result: &CreateTaskResult) -> Result<(), McpTaskViolation> {
        if result
            .task
            .status_message
            .as_ref()
            .is_some_and(|message| message.len() > MCP_TASK_MAX_STATUS_MESSAGE_BYTES)
        {
            return Err(McpTaskViolation::new(
                "the MCP task status message exceeds the rustX retention bound",
            ));
        }
        Ok(())
    }

    /// The server-assigned task id.
    pub(super) fn task_id(&self) -> &str {
        &self.task_id
    }

    /// The last status the server reported for this task, as the protocol
    /// spells it. It is diagnostic evidence: an invocation that cannot reach
    /// a terminal task state reports what it last saw.
    pub(super) const fn status(&self) -> &'static str {
        match self.status {
            TaskStatus::Working => "working",
            TaskStatus::InputRequired => "input_required",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
            _ => "unknown",
        }
    }

    /// The wait before the next poll.
    pub(super) const fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Validates one `tasks/get` snapshot and classifies it for the driver.
    ///
    /// Validation is rustX's own ownership contract, layered on rmcp's typed
    /// [`DetailedTask`](rmcp::model::DetailedTask) invariants rather than
    /// duplicating them: rmcp already refuses a snapshot whose status and
    /// payload disagree, so what is checked here is that the snapshot is
    /// about **this** task, that it stays within rustX's retention bounds,
    /// and that a terminal state actually carries the payload the original
    /// `tools/call` needs.
    ///
    /// Timestamps and status *transitions* are deliberately not validated:
    /// the specification leaves both flexible, and a task legitimately moves
    /// between `working` and `input_required` in either direction.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when the snapshot is not one rustX can
    /// act on.
    pub(super) fn observe(
        &mut self,
        snapshot: &GetTaskResult,
    ) -> Result<RemoteTaskObservation, McpTaskViolation> {
        let task = &snapshot.task.task;
        // One invocation has exactly one remote task. A snapshot about a
        // different id is a second remote execution identity trying to
        // appear inside this invocation, whatever the server meant by it.
        if task.task_id != self.task_id {
            return Err(McpTaskViolation::new(format!(
                "the MCP server answered tasks/get for task {:?} with a snapshot of task {:?}",
                bounded_id(&self.task_id),
                bounded_id(&task.task_id)
            )));
        }
        if let Some(message) = &task.status_message
            && message.len() > MCP_TASK_MAX_STATUS_MESSAGE_BYTES
        {
            return Err(McpTaskViolation::new(format!(
                "the MCP task status message is {} bytes, above the \
                 {MCP_TASK_MAX_STATUS_MESSAGE_BYTES}-byte rustX bound",
                message.len()
            )));
        }
        self.status = task.status;
        self.poll_interval = poll_interval(task.poll_interval_ms);
        match &snapshot.task.payload {
            TaskPayload::Working => Ok(RemoteTaskObservation::Working),
            TaskPayload::InputRequired { input_requests } => self.outstanding(input_requests),
            TaskPayload::Completed { result } => {
                let value = serde_json::Value::Object(result.clone());
                serde_json::from_value::<CallToolResult>(value).map_or_else(
                    |error| {
                        Err(McpTaskViolation::new(format!(
                            "the completed MCP task carries a result that is not a tools/call \
                             result: {error}"
                        )))
                    },
                    |result| Ok(RemoteTaskObservation::Completed(Box::new(result))),
                )
            }
            TaskPayload::Failed { error } => {
                let value = serde_json::Value::Object(error.clone());
                serde_json::from_value::<ErrorData>(value).map_or_else(
                    |error| {
                        Err(McpTaskViolation::new(format!(
                            "the failed MCP task carries an error that is not a JSON-RPC error \
                             object: {error}"
                        )))
                    },
                    |error| Ok(RemoteTaskObservation::Failed(Box::new(error))),
                )
            }
            TaskPayload::Cancelled => Ok(RemoteTaskObservation::Cancelled),
            // `TaskPayload` is `#[non_exhaustive]`: a future SEP-2663 status
            // rustX has never seen must fail this invocation deterministically
            // rather than panic or be silently treated as progress.
            _ => Err(McpTaskViolation::new(format!(
                "the MCP task reported status {:?}, which rustX does not implement",
                task.status
            ))),
        }
    }

    /// The subset of one `input_required` snapshot rustX has not answered.
    fn outstanding(
        &self,
        requests: &InputRequests,
    ) -> Result<RemoteTaskObservation, McpTaskViolation> {
        if requests.is_empty() {
            // rmcp proves the field is present; an *empty* map is the
            // contradiction it cannot catch — the task says it is blocked on
            // client input and names none.
            return Err(McpTaskViolation::new(
                "the MCP task reports status \"input_required\" and carries no input requests",
            ));
        }
        let outstanding: InputRequests = requests
            .iter()
            .filter(|(key, _)| !self.answered.contains(*key))
            .map(|(key, request)| (key.clone(), request.clone()))
            .collect();
        if outstanding.is_empty() {
            // Every key was already answered through `tasks/update`. The
            // acknowledgement is eventually consistent, so this is the
            // server's stale view of work rustX has done: keep polling, ask
            // nobody, and send nothing.
            return Ok(RemoteTaskObservation::Working);
        }
        Ok(RemoteTaskObservation::InputRequired(Box::new(outstanding)))
    }

    /// Records the keys one accepted `tasks/update` answers.
    ///
    /// Called at the update **dispatch frontier**, so a repeat can never be
    /// answered twice even if the acknowledgement itself is never observed:
    /// a second answer to a key rustX already sent would be a duplicate
    /// human interaction and a duplicate protocol effect, and neither is
    /// recoverable by retrying.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when the task has asked for more distinct
    /// input requests than rustX will retain.
    pub(super) fn record_answered<'a>(
        &mut self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), McpTaskViolation> {
        for key in keys {
            self.answered.insert(key.to_owned());
        }
        if self.answered.len() > MCP_TASK_MAX_ANSWERED_REQUESTS {
            return Err(McpTaskViolation::new(format!(
                "the MCP task asked for {} distinct input requests, above the \
                 {MCP_TASK_MAX_ANSWERED_REQUESTS} rustX bound",
                self.answered.len()
            )));
        }
        Ok(())
    }

    /// How many distinct input requests this task has been answered for.
    #[cfg(test)]
    pub(super) fn answered_count(&self) -> usize {
        self.answered.len()
    }
}

/// The cancellation-aware rustX wait one server polling hint asks for.
///
/// The local floor prevents busy polling. No upper clamp may shorten the
/// server interval; the invocation cancellation/deadline interrupts the wait.
pub(super) fn poll_interval(hint_ms: Option<u64>) -> Duration {
    let Some(hint_ms) = hint_ms else {
        return MCP_TASK_POLL_INTERVAL_DEFAULT;
    };
    Duration::from_millis(hint_ms).max(MCP_TASK_POLL_INTERVAL_MIN)
}

/// Method-specific decoding after the shared physical request owner.
pub(super) trait TaskResponse: Sized {
    fn decode(result: rmcp::model::ServerResult) -> Result<Self, String>;
}

impl TaskResponse for GetTaskResult {
    fn decode(result: rmcp::model::ServerResult) -> Result<Self, String> {
        match result {
            rmcp::model::ServerResult::GetTaskResult(snapshot) => Ok(snapshot),
            _ => Err("expected GetTaskResult task snapshot".to_owned()),
        }
    }
}

impl TaskResponse for rmcp::model::TaskAckResult {
    fn decode(result: rmcp::model::ServerResult) -> Result<Self, String> {
        match result {
            rmcp::model::ServerResult::TaskAckResult(ack) => Ok(ack),
            _ => Err("expected TaskAckResult acknowledgement".to_owned()),
        }
    }
}

/// A task id trimmed for a diagnostic, so a hostile id cannot inflate one.
fn bounded_id(id: &str) -> String {
    const LIMIT: usize = 64;
    if id.chars().count() <= LIMIT {
        return id.to_owned();
    }
    id.chars().take(LIMIT).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use rmcp::model::{DetailedTask, Task};

    use super::*;

    fn seed(task_id: &str) -> CreateTaskResult {
        CreateTaskResult::new(Task::new(
            task_id,
            TaskStatus::Working,
            "2026-07-28T10:00:00Z",
            "2026-07-28T10:00:00Z",
        ))
    }

    fn snapshot(task_id: &str, payload: TaskPayload) -> GetTaskResult {
        GetTaskResult::new(DetailedTask::new(
            Task::new(
                task_id,
                TaskStatus::Working,
                "2026-07-28T10:00:00Z",
                "2026-07-28T10:00:01Z",
            ),
            payload,
        ))
    }

    fn requests(value: serde_json::Value) -> InputRequests {
        serde_json::from_value(value).expect("input requests")
    }

    fn elicitation(property: &str) -> serde_json::Value {
        serde_json::json!({
            "method": "elicitation/create",
            "params": {
                "message": "which channel?",
                "requestedSchema": {
                    "type": "object",
                    "properties": {property: {"type": "string", "enum": ["stable", "beta"]}},
                    "required": [property],
                },
            },
        })
    }

    #[test]
    fn a_task_id_must_be_a_usable_bounded_protocol_identity() {
        assert!(RemoteTask::create(&seed("")).is_err());
        let oversized = "t".repeat(MCP_TASK_ID_MAX_BYTES + 1);
        assert!(RemoteTask::create(&seed(&oversized)).is_err());
        assert_eq!(
            RemoteTask::create(&seed("task-a"))
                .expect("a bounded id")
                .task_id(),
            "task-a"
        );
    }

    #[test]
    fn a_snapshot_of_another_task_is_a_second_identity_and_is_refused() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        let error = task
            .observe(&snapshot("task-b", TaskPayload::Working))
            .expect_err("a foreign snapshot is refused");
        assert!(
            error.diagnostic.contains("task-a") && error.diagnostic.contains("task-b"),
            "{}",
            error.diagnostic
        );
    }

    #[test]
    fn an_already_answered_request_key_reads_as_working_rather_than_a_new_ask() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        let payload = TaskPayload::InputRequired {
            input_requests: requests(serde_json::json!({"a": elicitation("channel")})),
        };
        assert!(matches!(
            task.observe(&snapshot("task-a", payload.clone()))
                .expect("an outstanding ask"),
            RemoteTaskObservation::InputRequired(_)
        ));
        task.record_answered(["a"]).expect("one answered key");
        // The stale repeat: the same key, after a successful update.
        assert!(matches!(
            task.observe(&snapshot("task-a", payload))
                .expect("a stale repeat"),
            RemoteTaskObservation::Working
        ));
        assert_eq!(task.answered_count(), 1);
    }

    #[test]
    fn a_genuinely_new_key_alongside_an_answered_one_is_still_processed() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        task.record_answered(["a"]).expect("one answered key");
        let payload = TaskPayload::InputRequired {
            input_requests: requests(serde_json::json!({
                "a": elicitation("channel"),
                "b": elicitation("fallback"),
            })),
        };
        let RemoteTaskObservation::InputRequired(outstanding) = task
            .observe(&snapshot("task-a", payload))
            .expect("a mixed snapshot")
        else {
            panic!("a genuinely new key is outstanding work");
        };
        assert_eq!(outstanding.keys().collect::<Vec<_>>(), vec!["b"]);
    }

    #[test]
    fn an_input_required_task_naming_no_request_is_contradictory() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        let error = task
            .observe(&snapshot(
                "task-a",
                TaskPayload::InputRequired {
                    input_requests: InputRequests::new(),
                },
            ))
            .expect_err("a contradictory snapshot is refused");
        assert!(error.diagnostic.contains("input_required"), "{error:?}");
    }

    #[test]
    fn a_completed_task_must_carry_a_tools_call_result() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        let malformed = serde_json::from_value(serde_json::json!({"content": 7})).expect("object");
        assert!(
            task.observe(&snapshot(
                "task-a",
                TaskPayload::Completed { result: malformed }
            ))
            .is_err()
        );
        let valid = serde_json::from_value(serde_json::json!({
            "content": [{"type": "text", "text": "done"}],
            "isError": true,
        }))
        .expect("object");
        let RemoteTaskObservation::Completed(result) = task
            .observe(&snapshot(
                "task-a",
                TaskPayload::Completed { result: valid },
            ))
            .expect("a completed task")
        else {
            panic!("a completed task carries its result");
        };
        // Task completion is not tool business success: `isError` survives.
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn a_failed_task_must_carry_a_json_rpc_error() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        let malformed =
            serde_json::from_value(serde_json::json!({"code": "boom"})).expect("object");
        assert!(
            task.observe(&snapshot(
                "task-a",
                TaskPayload::Failed { error: malformed }
            ))
            .is_err()
        );
        let valid =
            serde_json::from_value(serde_json::json!({"code": -32603, "message": "remote boom"}))
                .expect("object");
        let RemoteTaskObservation::Failed(error) = task
            .observe(&snapshot("task-a", TaskPayload::Failed { error: valid }))
            .expect("a failed task")
        else {
            panic!("a failed task carries its error");
        };
        assert_eq!(error.message.as_ref(), "remote boom");
    }

    #[test]
    fn the_answered_key_set_is_bounded() {
        let mut task = RemoteTask::create(&seed("task-a")).expect("task");
        for index in 0..MCP_TASK_MAX_ANSWERED_REQUESTS {
            task.record_answered([format!("key-{index}").as_str()])
                .expect("within the bound");
        }
        assert!(task.record_answered(["one-too-many"]).is_err());
    }

    #[test]
    fn mcp_tasks_polling_hints_are_never_shortened() {
        assert_eq!(poll_interval(None), MCP_TASK_POLL_INTERVAL_DEFAULT);
        assert_eq!(poll_interval(Some(0)), MCP_TASK_POLL_INTERVAL_MIN);
        assert_eq!(
            poll_interval(Some(u64::MAX)),
            Duration::from_millis(u64::MAX)
        );
        assert_eq!(poll_interval(Some(60_000)), Duration::from_mins(1));
        assert_eq!(poll_interval(Some(1_000)), Duration::from_secs(1));
    }

    #[test]
    fn mcp_tasks_ack_decoder_accepts_only_task_ack() {
        use rmcp::model::{ServerResult, TaskAckResult};
        assert!(
            TaskAckResult::decode(ServerResult::TaskAckResult(TaskAckResult::default())).is_ok()
        );
        for unrelated in [
            ServerResult::CallToolResult(CallToolResult::success(vec![])),
            ServerResult::CreateTaskResult(seed("task-a")),
            ServerResult::GetTaskResult(snapshot("task-a", TaskPayload::Working)),
            ServerResult::InputRequiredResult(
                serde_json::from_value(
                    serde_json::json!({"resultType": "input_required", "inputRequests": {}}),
                )
                .expect("input result"),
            ),
            ServerResult::EmptyResult(rmcp::model::EmptyObject {}),
        ] {
            assert!(TaskAckResult::decode(unrelated).is_err());
        }
    }

    #[test]
    fn a_status_message_above_the_retention_bound_is_refused() {
        let mut seed = seed("task-a");
        seed.task.status_message = Some("m".repeat(MCP_TASK_MAX_STATUS_MESSAGE_BYTES + 1));
        let mut task = RemoteTask::create(&seed).expect("identity remains addressable");
        assert!(RemoteTask::validate_creation(&seed).is_err());
        let snapshot = GetTaskResult::new(DetailedTask::new(seed.task, TaskPayload::Working));
        assert!(task.observe(&snapshot).is_err());
    }
}
