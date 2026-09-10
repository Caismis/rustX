//! The official-rmcp SEP-2663 Tasks fixture (Issue #243).
//!
//! Every scenario here is a **real** MCP Tasks peer: the fixture answers
//! `tools/call` with a genuine [`CreateTaskResult`], serves genuine
//! `tasks/get` snapshots, accepts genuine `tasks/update` responses, and
//! acknowledges a genuine `tasks/cancel`. Nothing is mocked at the rustX
//! function level, so a passing regression is a statement about the shipped
//! protocol path.
//!
//! # Transitions are counted, never timed
//!
//! A task's next snapshot is a pure function of what the fixture has already
//! answered for it — how many `tasks/get` requests it has served, how many
//! `tasks/update` requests it has accepted, and whether `tasks/cancel`
//! arrived. No scenario sleeps, and no scenario depends on scheduler timing:
//! "the third poll completes the task" is a counter, not a race.
//!
//! # The eventually-consistent scenarios are the point
//!
//! [`TASK_STALE_INPUT_TOOL`] deliberately repeats an input request key that
//! has already been answered, and [`TASK_NEW_INPUT_TOOL`] deliberately mixes
//! an already-answered key with a genuinely new one. Both are legal SEP-2663
//! server behaviour, and both are what the client's answered-key set exists
//! to survive.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolResult, ContentBlock, CreateTaskResult, DetailedTask, GetTaskResult, InputRequests,
    Task, TaskPayload, TaskStatus,
};

/// The environment variable publishing the SEP-2663 Tasks guard tools.
pub const TASK_TOOLS_ENV: &str = "RUSTX_M7_FIXTURE_TASKS";
/// The environment variable that publishes the Tasks guard tools **without**
/// advertising the extension in the server's own capabilities, so a client
/// can prove it refuses an unnegotiated `CreateTaskResult`.
pub const TASK_UNADVERTISED_ENV: &str = "RUSTX_M7_FIXTURE_TASKS_UNADVERTISED";
/// The environment variable naming the JSONL file every task-protocol
/// request is appended to.
pub const TASK_OBSERVATION_FILE_ENV: &str = "RUSTX_M7_FIXTURE_TASK_FILE";

/// `working`, `working`, then `completed`: the plain remote task.
pub const TASK_SIMPLE_TOOL: &str = "task_simple";
/// `input_required`, then — once answered — `working` and `completed`.
pub const TASK_INPUT_TOOL: &str = "task_input";
/// The eventually-consistent repeat: the same `inputRequests` key is served
/// again **after** the client's `tasks/update` was acknowledged.
pub const TASK_STALE_INPUT_TOOL: &str = "task_stale_input";
/// A later snapshot mixing one already-answered key with one genuinely new
/// key.
pub const TASK_NEW_INPUT_TOOL: &str = "task_new_input";
/// `working`, then the protocol's `failed` terminal state.
pub const TASK_FAILED_TOOL: &str = "task_failed";
/// `working`, then the protocol's `cancelled` terminal state, with no rustX
/// cancellation involved.
pub const TASK_CANCELLED_TOOL: &str = "task_cancelled";
/// A snapshot describing a **different** task id: a second remote execution
/// identity trying to appear inside one invocation.
pub const TASK_FOREIGN_TOOL: &str = "task_foreign";
/// `input_required` naming no input request at all.
pub const TASK_EMPTY_INPUT_TOOL: &str = "task_empty_input";
/// A task that never leaves `working`, for the cancellation frontiers.
pub const TASK_FOREVER_TOOL: &str = "task_forever";
/// One synchronous SEP-2322 `input_required` round **before** the task is
/// created, proving a task may be materialized by a continuation.
pub const TASK_MRTR_TOOL: &str = "task_mrtr";
/// A task whose input request is MCP sampling, which rustX refuses.
pub const TASK_SAMPLING_TOOL: &str = "task_sampling";

/// Every SEP-2663 guard tool the fixture publishes when
/// [`TASK_TOOLS_ENV`] is set.
pub const TASK_TOOLS: [&str; 11] = [
    TASK_CANCELLED_TOOL,
    TASK_EMPTY_INPUT_TOOL,
    TASK_FAILED_TOOL,
    TASK_FOREIGN_TOOL,
    TASK_FOREVER_TOOL,
    TASK_INPUT_TOOL,
    TASK_MRTR_TOOL,
    TASK_NEW_INPUT_TOOL,
    TASK_SAMPLING_TOOL,
    TASK_SIMPLE_TOOL,
    TASK_STALE_INPUT_TOOL,
];

/// The method labels the observation records use.
pub const CALL_TOOL: &str = "tools/call";
/// The `tasks/get` observation label.
pub const GET_TASK: &str = "tasks/get";
/// The `tasks/update` observation label.
pub const UPDATE_TASK: &str = "tasks/update";
/// The `tasks/cancel` observation label.
pub const CANCEL_TASK: &str = "tasks/cancel";

/// The polling hint every fixture task publishes.
///
/// It equals rustX's own floor, so a scenario runs at the fastest cadence the
/// client will honour and no test waits on a server-chosen delay.
pub const FIXTURE_POLL_INTERVAL_MS: u64 = 25;

/// The bounded choices a fixture task's elicitation asks about.
pub const TASK_CHOICES: [&str; 2] = ["stable", "beta"];

/// One task-protocol request exactly as the fixture received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TaskObservation {
    /// `tools/call`, `tasks/get`, `tasks/update`, or `tasks/cancel`.
    pub method: String,
    /// The model-facing tool name the task belongs to.
    pub tool: String,
    /// The task id this request addressed. Empty for the creating
    /// `tools/call`, which has none yet.
    pub task_id: String,
    /// Whether this request's own `_meta` advertised the SEP-2663 Tasks
    /// extension (SEP-2575 per-request capabilities).
    pub tasks_advertised: bool,
    /// Whether this request's own `_meta` advertised a client elicitation
    /// capability.
    pub elicitation_advertised: bool,
    /// The exact `inputResponses` map a `tasks/update` carried.
    pub input_responses: Option<serde_json::Value>,
}

/// Reads back every task-protocol request one fixture run observed.
///
/// # Panics
///
/// Panics when the observation file contains a line the fixture did not
/// write.
#[must_use]
pub fn task_observations(path: &Path) -> Vec<TaskObservation> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a fixture task observation line"))
        .collect()
}

/// The per-task server-side state of one fixture instance.
#[derive(Debug, Default, Clone)]
struct FixtureTask {
    /// The unprefixed guard-tool name that created this task.
    scenario: String,
    /// The model-facing tool name, for observations.
    tool: String,
    /// How many `tasks/get` requests the fixture has answered for it.
    gets: usize,
    /// The `inputResponses` maps the fixture has accepted, in order.
    updates: Vec<serde_json::Value>,
    /// Whether a cooperative `tasks/cancel` arrived.
    cancelled: bool,
}

/// The fixture's whole SEP-2663 server state: one entry per created task.
#[derive(Clone, Default)]
pub struct FixtureTaskRegistry {
    tasks: Arc<Mutex<BTreeMap<String, FixtureTask>>>,
}

impl std::fmt::Debug for FixtureTaskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("FixtureTaskRegistry").finish()
    }
}

/// The per-scenario task id, derived from the model-facing tool name so one
/// fixture process can serve several scoped scenarios at once.
#[must_use]
pub fn fixture_task_id(tool: &str) -> String {
    format!("{tool}--task")
}

/// One bounded single-select elicitation request over [`TASK_CHOICES`].
#[must_use]
pub fn task_choice_request(message: &str, property: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": {property: {"type": "string", "enum": TASK_CHOICES}},
                "required": [property],
            },
        },
    })
}

/// The MCP sampling request rustX must refuse without calling any model.
#[must_use]
pub fn task_sampling_request() -> serde_json::Value {
    serde_json::json!({
        "method": "sampling/createMessage",
        "params": {
            "messages": [{"role": "user", "content": {"type": "text", "text": "capital?"}}],
            "maxTokens": 64,
        },
    })
}

fn input_requests(entries: serde_json::Value) -> Result<InputRequests, rmcp::ErrorData> {
    serde_json::from_value(entries).map_err(|error| {
        rmcp::ErrorData::internal_error(format!("invalid fixture input requests: {error}"), None)
    })
}

fn base_task(task_id: &str, status: TaskStatus) -> Task {
    Task::new(
        task_id,
        status,
        "2026-07-28T10:00:00Z",
        "2026-07-28T10:00:00Z",
    )
    .with_poll_interval_ms(FIXTURE_POLL_INTERVAL_MS)
}

/// The answer one accepted `inputResponses` map gave for `key`/`property`.
fn answered(update: &serde_json::Value, key: &str, property: &str) -> Option<String> {
    update
        .get(key)?
        .get("content")?
        .get(property)?
        .as_str()
        .map(str::to_owned)
}

fn completed(text: String) -> Result<TaskPayload, rmcp::ErrorData> {
    let result = CallToolResult::success(vec![ContentBlock::text(text)]);
    let value = serde_json::to_value(&result).map_err(|error| {
        rmcp::ErrorData::internal_error(format!("cannot encode fixture result: {error}"), None)
    })?;
    let serde_json::Value::Object(result) = value else {
        return Err(rmcp::ErrorData::internal_error(
            "a fixture tool result is not a JSON object",
            None,
        ));
    };
    Ok(TaskPayload::Completed { result })
}

impl FixtureTaskRegistry {
    /// Materializes one `tools/call` as a remote task.
    #[must_use]
    pub fn create(&self, scenario: &str, tool: &str) -> CreateTaskResult {
        let task_id = fixture_task_id(tool);
        let mut tasks = self.tasks.lock().expect("fixture task registry");
        tasks.entry(task_id.clone()).or_insert_with(|| FixtureTask {
            scenario: scenario.to_owned(),
            tool: tool.to_owned(),
            ..FixtureTask::default()
        });
        CreateTaskResult::new(base_task(&task_id, TaskStatus::Working))
    }

    /// The model-facing tool name one task belongs to, for observations.
    #[must_use]
    pub fn tool_of(&self, task_id: &str) -> String {
        self.tasks
            .lock()
            .expect("fixture task registry")
            .get(task_id)
            .map_or_else(String::new, |task| task.tool.clone())
    }

    /// Serves one `tasks/get`.
    ///
    /// # Errors
    ///
    /// Returns the protocol's `-32602` for a task id the fixture never
    /// created, exactly as SEP-2663 requires.
    pub fn get(&self, task_id: &str) -> Result<GetTaskResult, rmcp::ErrorData> {
        let state = {
            let mut tasks = self.tasks.lock().expect("fixture task registry");
            let Some(task) = tasks.get_mut(task_id) else {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("unknown task {task_id}"),
                    None,
                ));
            };
            task.gets += 1;
            task.clone()
        };
        // A cooperative cancellation is honoured on the next observation, so
        // a client that asked for one and then polls sees the protocol's own
        // terminal `cancelled` — which still proves nothing about whatever
        // side effect the scenario would have had.
        if state.cancelled {
            return Ok(GetTaskResult::new(DetailedTask::new(
                base_task(task_id, TaskStatus::Cancelled),
                TaskPayload::Cancelled,
            )));
        }
        let payload = Self::payload(&state)?;
        // `DetailedTask::new` forces the base status to agree with the
        // payload, so the fixture cannot emit a self-contradictory snapshot
        // by accident — the deliberately malformed scenarios are explicit.
        let snapshot = DetailedTask::new(base_task(task_id, TaskStatus::Working), payload);
        Ok(GetTaskResult::new(snapshot))
    }

    /// The scenario's next payload, as a pure function of what it has served.
    #[allow(
        clippy::too_many_lines,
        reason = "the guard-scenario family is one transition table"
    )]
    fn payload(state: &FixtureTask) -> Result<TaskPayload, rmcp::ErrorData> {
        let updates = state.updates.len();
        let choice = |index: usize, key: &str, property: &str| {
            state
                .updates
                .get(index)
                .and_then(|update| answered(update, key, property))
                .unwrap_or_else(|| "<none>".to_owned())
        };
        match state.scenario.as_str() {
            TASK_SIMPLE_TOOL => {
                if state.gets < 3 {
                    Ok(TaskPayload::Working)
                } else {
                    completed(format!("task simple done after {} polls", state.gets))
                }
            }

            TASK_FAILED_TOOL => {
                if state.gets < 2 {
                    return Ok(TaskPayload::Working);
                }
                let error = serde_json::from_value(serde_json::json!({
                    "code": -32001,
                    "message": "the fixture task failed remotely",
                }))
                .map_err(|error| {
                    rmcp::ErrorData::internal_error(
                        format!("cannot encode fixture task error: {error}"),
                        None,
                    )
                })?;
                Ok(TaskPayload::Failed { error })
            }
            TASK_CANCELLED_TOOL => {
                if state.gets < 2 {
                    Ok(TaskPayload::Working)
                } else {
                    Ok(TaskPayload::Cancelled)
                }
            }
            TASK_EMPTY_INPUT_TOOL => Ok(TaskPayload::InputRequired {
                input_requests: InputRequests::new(),
            }),
            TASK_SAMPLING_TOOL => Ok(TaskPayload::InputRequired {
                input_requests: input_requests(
                    serde_json::json!({"ask": task_sampling_request()}),
                )?,
            }),
            TASK_INPUT_TOOL | TASK_MRTR_TOOL => {
                if updates == 0 {
                    return Ok(TaskPayload::InputRequired {
                        input_requests: input_requests(serde_json::json!({
                            "ask": task_choice_request("Which channel?", "channel"),
                        }))?,
                    });
                }
                // One `working` observation between the accepted update and
                // the result, so the poll loop genuinely continues after an
                // interaction rather than settling immediately.
                if state.gets < 3 {
                    return Ok(TaskPayload::Working);
                }
                completed(format!("channel={}", choice(0, "ask", "channel")))
            }
            TASK_STALE_INPUT_TOOL => {
                if updates == 0 {
                    return Ok(TaskPayload::InputRequired {
                        input_requests: input_requests(serde_json::json!({
                            "ask": task_choice_request("Which channel?", "channel"),
                        }))?,
                    });
                }
                // The eventually-consistent repeat: the update was
                // acknowledged, and this snapshot still shows the key.
                if state.gets == 2 {
                    return Ok(TaskPayload::InputRequired {
                        input_requests: input_requests(serde_json::json!({
                            "ask": task_choice_request("Which channel?", "channel"),
                        }))?,
                    });
                }
                if state.gets == 3 {
                    return Ok(TaskPayload::Working);
                }
                completed(format!("channel={}", choice(0, "ask", "channel")))
            }
            TASK_NEW_INPUT_TOOL => {
                if updates == 0 {
                    return Ok(TaskPayload::InputRequired {
                        input_requests: input_requests(serde_json::json!({
                            "first": task_choice_request("Which channel?", "channel"),
                        }))?,
                    });
                }
                if updates == 1 {
                    // One already-answered key beside one genuinely new key.
                    return Ok(TaskPayload::InputRequired {
                        input_requests: input_requests(serde_json::json!({
                            "first": task_choice_request("Which channel?", "channel"),
                            "second": task_choice_request("Which fallback?", "fallback"),
                        }))?,
                    });
                }
                completed(format!(
                    "channel={} fallback={}",
                    choice(0, "first", "channel"),
                    choice(1, "second", "fallback")
                ))
            }
            // `TASK_FOREVER_TOOL` never leaves `working`, for the
            // cancellation frontiers. `TASK_FOREIGN_TOOL` is deliberately
            // malformed: the snapshot the handler actually returns for it
            // describes *another* task id entirely, so the payload it would
            // otherwise carry is never observed.
            TASK_FOREVER_TOOL | TASK_FOREIGN_TOOL => Ok(TaskPayload::Working),
            other => Err(rmcp::ErrorData::internal_error(
                format!("unknown fixture task scenario {other}"),
                None,
            )),
        }
    }

    /// Whether this task's scenario answers with a foreign task id.
    #[must_use]
    pub fn answers_foreign_id(&self, task_id: &str) -> bool {
        self.tasks
            .lock()
            .expect("fixture task registry")
            .get(task_id)
            .is_some_and(|task| task.scenario == TASK_FOREIGN_TOOL)
    }

    /// Accepts one `tasks/update`.
    ///
    /// # Errors
    ///
    /// Returns the protocol's `-32602` for an unknown task id.
    pub fn update(
        &self,
        task_id: &str,
        responses: &rmcp::model::InputResponses,
    ) -> Result<(), rmcp::ErrorData> {
        let mut tasks = self.tasks.lock().expect("fixture task registry");
        let Some(task) = tasks.get_mut(task_id) else {
            return Err(rmcp::ErrorData::invalid_params(
                format!("unknown task {task_id}"),
                None,
            ));
        };
        let encoded = serde_json::to_value(responses).map_err(|error| {
            rmcp::ErrorData::internal_error(
                format!("cannot encode fixture input responses: {error}"),
                None,
            )
        })?;
        task.updates.push(encoded);
        Ok(())
    }

    /// Accepts one cooperative `tasks/cancel`.
    ///
    /// # Errors
    ///
    /// Returns the protocol's `-32602` for an unknown task id.
    pub fn cancel(&self, task_id: &str) -> Result<(), rmcp::ErrorData> {
        let mut tasks = self.tasks.lock().expect("fixture task registry");
        let Some(task) = tasks.get_mut(task_id) else {
            return Err(rmcp::ErrorData::invalid_params(
                format!("unknown task {task_id}"),
                None,
            ));
        };
        task.cancelled = true;
        Ok(())
    }
}

/// A `tasks/get` snapshot that deliberately describes a **different** task.
#[must_use]
pub fn foreign_snapshot(task_id: &str) -> GetTaskResult {
    GetTaskResult::new(DetailedTask::new(
        base_task(&format!("{task_id}--foreign"), TaskStatus::Working),
        TaskPayload::Working,
    ))
}

/// Appends one task-protocol observation, when a file is configured.
///
/// # Errors
///
/// Returns an internal error when the observation cannot be recorded, so a
/// silently unobserved scenario fails loudly instead of passing.
pub fn record(path: Option<&Path>, observation: &TaskObservation) -> Result<(), rmcp::ErrorData> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            rmcp::ErrorData::internal_error(
                format!("cannot record fixture task request: {error}"),
                None,
            )
        })?;
    let line = serde_json::to_string(observation).map_err(|error| {
        rmcp::ErrorData::internal_error(
            format!("cannot encode fixture task request: {error}"),
            None,
        )
    })?;
    writeln!(file, "{line}").map_err(|error| {
        rmcp::ErrorData::internal_error(
            format!("cannot record fixture task request: {error}"),
            None,
        )
    })
}

/// The per-request client capabilities one task-protocol request carried, as
/// `(tasks, elicitation)`.
#[must_use]
pub fn advertised(capabilities: Option<rmcp::model::ClientCapabilities>) -> (bool, bool) {
    capabilities.map_or((false, false), |capabilities| {
        (
            capabilities.supports_tasks(),
            capabilities.elicitation.is_some(),
        )
    })
}

/// The observation file one fixture instance records into.
#[must_use]
pub fn observation_file() -> Option<PathBuf> {
    std::env::var_os(TASK_OBSERVATION_FILE_ENV).map(PathBuf::from)
}
