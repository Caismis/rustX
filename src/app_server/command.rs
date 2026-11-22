//! Connection-owned standalone commands dispatched through the Session's
//! admitted physical Execution Client.
//!
//! `command/exec` registers one managed process operation on the
//! Session's execution endpoint and drives it — Register, Start, ordered
//! output paging, stdin/resize admission and correlated input receipts,
//! and the final `ReleaseOutput` — through typed exchanges on the data
//! lane. No Server-local spawn, PTY, or drain path exists here: the
//! physical actor lives in the Execution Client. The UI connection owns
//! only the `process_id` → operation map; cancellation rides the
//! dedicated control lane and never targets another Session's or UI
//! connection's operation.
mod output;

use self::output::OutputAccounting;
use super::protocol::{ErrorData, NotificationMethod, RequestId, RpcError};
use super::rpc_error::domain;
use base64::Engine as _;
use rustx_execution_protocol::DecimalU64;
use rustx_execution_protocol::process::{
    InputOutcome, OperationId, OperationState, ProcessOperationKey, ProcessProgram, ProcessReply,
    ProcessRequest, ProcessSpec, ProcessStdin, ProcessStream, PtySpec, SettledDisposition,
    SettledSnapshot, SpecRefusal,
};
use rustx_execution_server::ProcessOutcomeUnknown;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecParams {
    /// The Session whose admitted Execution Client owns the process.
    pub session_id: crate::local_runtime::session::SessionId,
    #[schemars(length(min = 1, max = 256))]
    pub command: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub env: BTreeMap<String, Option<String>>,
    pub process_id: Option<String>,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub stream_stdin: bool,
    #[serde(default)]
    pub stream_stdout_stderr: bool,
    #[schemars(range(min = 1, max = 262_144))]
    pub output_bytes_cap: Option<usize>,
    #[schemars(range(min = 1, max = 3_600_000))]
    pub timeout_ms: Option<u32>,
    pub size: Option<TerminalSize>,
}
impl std::fmt::Debug for ExecParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecParams")
            .field("session_id", &self.session_id)
            .field("process_id", &self.process_id)
            .field("tty", &self.tty)
            .field("environment_keys", &self.env.keys())
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSize {
    #[schemars(range(min = 1, max = 1000))]
    pub rows: u16,
    #[schemars(range(min = 1, max = 1000))]
    pub cols: u16,
}
impl TerminalSize {
    fn validate(self) -> Result<Self, RpcError> {
        if self.rows == 0 || self.cols == 0 || self.rows > 1000 || self.cols > 1000 {
            return Err(domain(ErrorData::InvalidParams));
        }
        Ok(self)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteParams {
    pub process_id: String,
    pub delta_base64: Option<String>,
    #[serde(default)]
    pub close_stdin: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub status: ExecStatus,
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecStatus {
    Completed,
    TimedOut,
    Terminated,
    ProcessControlFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputDelta {
    pub process_id: String,
    pub sequence: u64,
    pub stream: OutputStream,
    pub delta_base64: String,
    pub cap_reached: bool,
}

/// The outcome of one typed exchange with the Execution Client.
#[derive(Debug)]
enum Exchange {
    /// The verified typed reply.
    Reply(ProcessReply),
    /// The connection's bounded in-flight budget is busy — the caller
    /// retries on its paced cycle; never a queue, never a failure.
    Busy,
    /// A transport or protocol failure mapped to the closed public error.
    Failed(RpcError),
}

/// One UI-accepted operation: the Server-assigned complete identity and
/// the endpoint/control route it belongs to. The map keys only this UI
/// connection's own `process_id`s — controls can never target another
/// connection's operation.
struct Entry {
    key: ProcessOperationKey,
    process_id: String,
    endpoint: Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    cancellation: crate::runtime::CancellationSignal,
    /// Set by an authoritative registration refusal or terminal snapshot.
    /// Either proves this driver has no outstanding physical cancellation.
    reconciled: std::sync::atomic::AtomicBool,
    /// The strict input sequence — serialized so concurrent writes can
    /// never double-assign one sequence position.
    next_input: tokio::sync::Mutex<u64>,
}
#[derive(Default)]
struct State {
    closed: bool,
    active: BTreeMap<String, Arc<Entry>>,
}
pub(super) struct Commands {
    state: Mutex<State>,
    revisions: tokio::sync::watch::Sender<u64>,
}
impl Default for Commands {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            revisions: tokio::sync::watch::channel(0).0,
        }
    }
}
impl Commands {
    #[cfg(test)]
    pub(crate) async fn admitted_for_test(&self, id: &str) {
        let mut revisions = self.revisions.subscribe();
        loop {
            drop(revisions.borrow_and_update());
            if self.state.lock().expect("commands").active.contains_key(id) {
                return;
            }
            revisions
                .changed()
                .await
                .expect("commands watch sender retained while observing admission");
        }
    }
    /// UI detach: cancels only this connection's own operations — the
    /// per-operation task forwards the cancel on the control lane to its
    /// exact key. Session-wide and Agent operations are untouched.
    pub fn close(&self) {
        let mut state = self.state.lock().expect("connection commands");
        state.closed = true;
        for entry in state.active.values() {
            entry.cancellation.cancel();
        }
    }
    #[allow(clippy::too_many_lines)] // One complete admission: gate, spec, identity, task.
    pub fn start(
        self: &Arc<Self>,
        params: &ExecParams,
        manager: crate::local_runtime::session_runtime_manager::SessionRuntimeManager,
        events: Arc<super::service_events::Events>,
        owner: super::host::ServerOperation,
    ) -> Result<oneshot::Receiver<Result<ExecResult, RpcError>>, RpcError> {
        validate(params)?;
        // Non-creating lookup: no admitted Client means unavailability,
        // never a Server-local fallback.
        let endpoint = manager
            .execution_endpoint(&params.session_id)
            .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
        // Logical authorization only: the admitted workspace must sit
        // under a configured environment root — before dispatch and again
        // inside the operation task's gate.
        let authorized = || {
            manager
                .process_policy()
                .environment_roots
                .iter()
                .any(|root| endpoint.workspace().starts_with(root))
        };
        if !authorized() {
            return Err(domain(ErrorData::EnvironmentUnauthorized));
        }
        // The live root retirement signal from the authorized logical
        // policy — a removal races the operation's START grant through
        // the control-lane cancel the task forwards.
        let root_retired = manager
            .environment
            .retirement(endpoint.workspace())
            .ok_or_else(|| domain(ErrorData::EnvironmentUnauthorized))?;
        let admission = endpoint
            .admission()
            .ok_or_else(|| domain(ErrorData::ExecutionUnavailable))?
            .clone();
        endpoint
            .control()
            .ok_or_else(|| domain(ErrorData::ExecutionUnavailable))?;
        let relative = crate::local_runtime::session_runtime_manager::workspace_entry(
            endpoint.workspace(),
            std::path::Path::new(&params.cwd),
        )
        .map_err(|_| domain(ErrorData::EnvironmentUnauthorized))?;
        let spec = ProcessSpec {
            cwd: relative,
            program: ProcessProgram::Argv(params.command.clone()),
            env: params
                .env
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_ref()
                        .map(|value| (key.clone(), Some(value.clone())))
                })
                .collect(),
            timeout_ms: u64::from(params.timeout_ms.unwrap_or(60_000)),
            pty: params.tty.then(|| {
                params
                    .size
                    .unwrap_or(TerminalSize { rows: 24, cols: 80 })
                    .into()
            }),
            stdin: if params.tty || params.stream_stdin {
                ProcessStdin::Streamed
            } else {
                ProcessStdin::Closed
            },
            capture: true,
            output_cap: u32::try_from(params.output_bytes_cap.unwrap_or(64 * 1024))
                .map_err(|_| domain(ErrorData::InvalidParams))?,
        };
        let key = ProcessOperationKey {
            incarnation: admission.server_incarnation,
            session: admission.session,
            instance: admission.instance,
            generation: admission.generation,
            operation: OperationId::new(),
        };
        let process_id = params
            .process_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let cancellation = events.closed.child();
        let mut state = self.state.lock().expect("connection commands");
        if state.closed || state.active.len() >= 4 || state.active.contains_key(&process_id) {
            return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
        }
        let entry = Arc::new(Entry {
            key: key.clone(),
            process_id: process_id.clone(),
            endpoint: endpoint.clone(),
            cancellation: cancellation.clone(),
            reconciled: std::sync::atomic::AtomicBool::new(false),
            next_input: tokio::sync::Mutex::new(1),
        });
        state.active.insert(process_id.clone(), entry.clone());
        drop(state);
        self.revisions.send_modify(|revision| *revision += 1);
        let (sender, received) = oneshot::channel();
        let commands = self.clone();
        let streaming = params.tty || params.stream_stdout_stderr;
        let (disarm, disarmed) = oneshot::channel::<()>();
        let watcher = watch_cancellation(
            entry.clone(),
            cancellation.clone(),
            root_retired.clone(),
            events.closed.clone(),
            disarmed,
        );
        #[cfg(test)]
        let watcher = record_cancellation_watcher(&process_id, watcher);
        drop(watcher);
        tokio::spawn(async move {
            let _owner = owner;
            let result = run(
                entry.clone(),
                key,
                spec,
                process_id.clone(),
                streaming,
                manager,
                events,
                cancellation.clone(),
                root_retired,
            )
            .await;
            // A failure with an unresolved registration or operation
            // hands it to the cancellation watcher.
            if result.is_err() && !entry.reconciled.load(std::sync::atomic::Ordering::Acquire) {
                cancellation.cancel();
            } else {
                let _ = disarm.send(());
            }
            commands
                .state
                .lock()
                .expect("connection commands")
                .active
                .remove(&process_id);
            let _ = sender.send(result);
        });
        Ok(received)
    }
    fn entry(&self, process_id: &str) -> Result<Arc<Entry>, RpcError> {
        let state = self.state.lock().expect("connection commands");
        if state.closed {
            return Err(domain(ErrorData::ConnectionClosed));
        }
        state
            .active
            .get(process_id)
            .cloned()
            .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))
    }
    /// Whether this exact admission is still live on the connection —
    /// the per-operation task removes it when the operation ends, so a
    /// missing entry means the `Unknown` is final rather than an
    /// admission that outraced its own `Register` dispatch.
    fn still_active(&self, entry: &Arc<Entry>) -> bool {
        self.state
            .lock()
            .expect("connection commands")
            .active
            .get(&entry.process_id)
            .is_some_and(|live| Arc::ptr_eq(live, entry))
    }
    /// One admitted input control — write or close — followed by the
    /// correlated physical receipt query. The sequence is committed only
    /// on `InputAccepted`, so a refused admission never leaves a gap.
    async fn input(
        &self,
        entry: &Arc<Entry>,
        make: impl FnOnce(u64) -> ProcessRequest,
    ) -> Result<(), RpcError> {
        let mut next = entry.next_input.lock().await;
        let sequence = *next;
        let request = make(sequence);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match exchange(&entry.endpoint, request.clone()).await {
                Exchange::Reply(ProcessReply::InputAccepted { .. }) => {
                    *next = sequence + 1;
                    break;
                }
                Exchange::Reply(ProcessReply::Unknown { .. }) => {
                    // An admission can outrace its own `Register`
                    // dispatch — while the entry is still live the
                    // record may simply not exist yet. A removed entry
                    // means the operation ended: the miss is final.
                    if !self.still_active(entry) || std::time::Instant::now() >= deadline {
                        return Err(domain(ErrorData::EnvironmentResourceNotFound));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                }
                Exchange::Reply(ProcessReply::Conflict { .. }) => {
                    return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
                }
                Exchange::Reply(_) => return Err(domain(ErrorData::OperationFailed)),
                // The lane's bounded in-flight budget is full — `WouldBlock`
                // means the request never dispatched, so a paced retry of
                // the same sequence is never a replay.
                Exchange::Busy => {
                    if std::time::Instant::now() >= deadline {
                        return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                }
                Exchange::Failed(error) => return Err(error),
            }
        }
        drop(next);
        collect_input(entry, sequence).await
    }
    pub async fn write(&self, params: WriteParams) -> Result<(), RpcError> {
        if params
            .delta_base64
            .as_ref()
            .is_some_and(|data| data.len() > 87_384)
            || (params.delta_base64.is_none() && !params.close_stdin)
        {
            return Err(domain(ErrorData::InvalidParams));
        }
        let entry = self.entry(&params.process_id)?;
        match (params.delta_base64.is_some(), params.close_stdin) {
            // One atomic admission: the Client vetoes the whole batch
            // when a byte-delivered EOF is impossible under the live
            // termios policy — no partial write, no forged close.
            (true, _) => {
                let delta = params.delta_base64.clone().expect("checked");
                let close = params.close_stdin;
                self.input(&entry, |sequence| ProcessRequest::WriteStdin {
                    key: entry.key.clone(),
                    sequence: DecimalU64::new(sequence),
                    delta_base64: delta,
                    close,
                })
                .await
            }
            (false, true) => {
                self.input(&entry, |sequence| ProcessRequest::CloseStdin {
                    key: entry.key.clone(),
                    sequence: DecimalU64::new(sequence),
                })
                .await
            }
            (false, false) => unreachable!("validated"),
        }
    }
    pub async fn resize(&self, process_id: &str, size: TerminalSize) -> Result<(), RpcError> {
        let entry = self.entry(process_id)?;
        let size = size.validate()?;
        self.input(&entry, |sequence| ProcessRequest::Resize {
            key: entry.key.clone(),
            sequence: DecimalU64::new(sequence),
            size: size.into(),
        })
        .await
    }
    /// Cancel this connection's one operation: the control lane carries
    /// the cancellation to the exact key — nonblocking enqueue, the
    /// operation's watcher delivers the keyed cancel on the control lane.
    pub fn terminate(&self, process_id: &str) -> Result<(), RpcError> {
        let entry = self.entry(process_id)?;
        // The enqueue-only path: a full queue or dead lane is not an
        // error the caller can act on — the driver already observes its
        // own cancellation and the lane's health is reported elsewhere.
        entry.cancellation.cancel();
        Ok(())
    }
}

/// An accepted input keeps its receipt slot until collection or explicit
/// abandonment is proven. A local failure hands it to binding teardown.
struct InputReceiptOwner<'a>(
    Option<&'a crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
);

impl InputReceiptOwner<'_> {
    fn confirm(mut self) {
        self.0 = None;
    }
}

impl Drop for InputReceiptOwner<'_> {
    fn drop(&mut self) {
        if let Some(endpoint) = self.0 {
            endpoint.cut_pair_if_open();
        }
    }
}

async fn collect_input(entry: &Entry, sequence: u64) -> Result<(), RpcError> {
    use std::time::{Duration, Instant};
    let owner = InputReceiptOwner(Some(&entry.endpoint));
    let budgets = (
        Duration::from_secs(10),
        rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT,
    );
    #[cfg(test)]
    let hold = tests::pause_input_receipt(&entry.process_id).await;
    #[cfg(test)]
    let budgets = if hold == tests::InputHoldMode::ExpireCleanup {
        (Duration::ZERO, Duration::ZERO)
    } else {
        budgets
    };
    let (observation_budget, exchange_budget) = budgets;
    let deadline = Instant::now() + observation_budget;
    let mut finish_after_error = false;
    loop {
        let abandon_pending =
            finish_after_error || entry.cancellation.is_cancelled() || Instant::now() >= deadline;
        let exchange_deadline = Instant::now() + exchange_budget;
        #[cfg(test)]
        let exchange_deadline = if hold == tests::InputHoldMode::ExpireFirstPoll && !abandon_pending
        {
            Instant::now()
        } else {
            exchange_deadline
        };
        match exchange_until(
            &entry.endpoint,
            ProcessRequest::InputReceipt {
                key: entry.key.clone(),
                sequence: sequence.into(),
                abandon_pending,
            },
            exchange_deadline,
        )
        .await
        {
            Exchange::Reply(ProcessReply::InputReceipt {
                status: InputOutcome::Pending,
                ..
            })
            | Exchange::Busy
                if !abandon_pending => {}
            Exchange::Reply(ProcessReply::InputReceipt { status, .. }) => {
                owner.confirm();
                return match status {
                    InputOutcome::Applied => Ok(()),
                    InputOutcome::Rejected => Err(domain(ErrorData::InvalidParams)),
                    // A completed receipt releases the slot. Failed can
                    // still represent a write that applied a prefix.
                    InputOutcome::Failed | InputOutcome::Pending | InputOutcome::Unknown => {
                        Err(domain(ErrorData::OutcomeUnknown))
                    }
                };
            }
            Exchange::Reply(ProcessReply::Unknown { .. }) => {
                owner.confirm();
                return Err(domain(ErrorData::OutcomeUnknown));
            }
            _ if !abandon_pending => {
                // Even an ordinary pre-wire timeout leaves the accepted
                // slot owned. Make a bounded final claim or abandonment.
                finish_after_error = true;
                continue;
            }
            // The input crossed admission. A failure observing its final
            // receipt cannot prove what the physical input did.
            _ => return Err(domain(ErrorData::OutcomeUnknown)),
        }
        tokio::select! {
            () = entry.cancellation.cancelled() => {}
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

fn validate(params: &ExecParams) -> Result<(), RpcError> {
    if params.command.is_empty()
        || params.command.len() > 256
        || params.command[0].is_empty()
        || params.command.iter().any(|part| part.contains('\0'))
        || params.command.iter().map(String::len).sum::<usize>() > 64 * 1024
        || params.env.len() > 128
        || params.env.iter().any(|(key, value)| {
            key.len() > 256
                || value
                    .as_ref()
                    .is_some_and(|value| value.len() > 8192 || value.contains('\0'))
        })
        || params
            .output_bytes_cap
            .is_some_and(|cap| cap == 0 || cap > 262_144)
        || params
            .timeout_ms
            .is_some_and(|timeout| timeout == 0 || timeout > 3_600_000)
        || params.size.is_some() && !params.tty
        || (params.tty || params.stream_stdin || params.stream_stdout_stderr)
            && params.process_id.is_none()
        || params
            .process_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || !RequestId::is_valid_string(id))
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    if let Some(size) = params.size {
        size.validate()?;
    }
    crate::tools::environment::ToolEnvironment::from_authorized(
        params
            .env
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone()))),
    )
    .map_err(|_| domain(ErrorData::InvalidParams))?;
    Ok(())
}

impl From<TerminalSize> for PtySpec {
    fn from(size: TerminalSize) -> Self {
        PtySpec {
            rows: size.rows,
            cols: size.cols,
        }
    }
}

/// One typed exchange on the data lane — scheduled per RPC: the
/// connection lock covers exactly this request/reply pair, never the
/// operation's lifetime. `WouldBlock` is the bounded busy signal the
/// paced caller retries.
async fn exchange(
    endpoint: &Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    request: ProcessRequest,
) -> Exchange {
    exchange_until(
        endpoint,
        request,
        std::time::Instant::now() + rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT,
    )
    .await
}

/// [`exchange`] under one absolute deadline shared with the wire exchange.
async fn exchange_until(
    endpoint: &Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    request: ProcessRequest,
    deadline: std::time::Instant,
) -> Exchange {
    let endpoint = endpoint.clone();
    let result = tokio::task::spawn_blocking(move || match request {
        ProcessRequest::InputReceipt {
            abandon_pending: true,
            ..
        } => endpoint.finish_input_receipt_by(&request, deadline),
        _ => endpoint.process_until(&request, deadline),
    })
    .await;
    match result {
        Ok(Ok(reply)) => Exchange::Reply(reply),
        Ok(Err(error)) if ProcessOutcomeUnknown::is(&error) => {
            Exchange::Failed(domain(ErrorData::OutcomeUnknown))
        }
        Ok(Err(io)) if io.kind() == std::io::ErrorKind::WouldBlock => Exchange::Busy,
        Ok(Err(io)) => Exchange::Failed(match io.kind() {
            // A dead or revoked execution binding — the liveness gate's
            // `PermissionDenied` — is an execution-lane failure, not a
            // caller authorization fault.
            std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::PermissionDenied => domain(ErrorData::ExecutionUnavailable),
            _ => super::connection::environment_error(io),
        }),
        Err(_) => Exchange::Failed(domain(ErrorData::OutcomeUnknown)),
    }
}

/// The public stream of one output lane (`0` stdout, `1` stderr).
fn lane_stream(lane: usize) -> OutputStream {
    if lane == 0 {
        OutputStream::Stdout
    } else {
        OutputStream::Stderr
    }
}

/// A reply that contradicts the Client's contract after START: the pair that
/// produced it is retired — never repaired — and the outcome is unknown.
async fn contradicted(
    endpoint: &Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
) -> RpcError {
    let endpoint = endpoint.clone();
    let _ = tokio::task::spawn_blocking(move || endpoint.retire_connection()).await;
    domain(ErrorData::OutcomeUnknown)
}

/// The one absolute budget of a cancellation delivery, queue wait included.
const CANCEL_BUDGET: std::time::Duration =
    rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
/// The one absolute budget of the final release, prewire retries included.
const RELEASE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// The physical outcome pace of the operation task: far inside every
/// wire budget, never busy-spinning the data lane.
const OPERATION_POLL: std::time::Duration = std::time::Duration::from_millis(20);
/// `command/output` deltas are emitted at the physical read granularity:
/// one bounded notification per chunk keeps the delivery queue's own
/// bound as the consumer-stall fence.
const OUTPUT_CHUNK_BYTES: usize = 8192;

/// Drives one accepted operation from registration to release.
///
/// Every step is an honest typed exchange: registration replays attach
/// by fingerprint, `Start` only admits, the `Settled` snapshot is the
/// terminal proof, output is consumed through the ordered journal, and
/// the exact final sequence releases it. Cancellation observed locally —
/// UI detach, root retirement, connection close — is forwarded to the
/// operation's key on the dedicated control lane; the task then keeps
/// draining until the Client's own settled snapshot arrives or the lane
/// fails.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one coherent operation driver
async fn run(
    entry: Arc<Entry>,
    key: ProcessOperationKey,
    spec: ProcessSpec,
    process_id: String,
    streaming: bool,
    manager: crate::local_runtime::session_runtime_manager::SessionRuntimeManager,
    events: Arc<super::service_events::Events>,
    cancellation: crate::runtime::CancellationSignal,
    root_retired: crate::runtime::CancellationSignal,
) -> Result<ExecResult, RpcError> {
    let endpoint = entry.endpoint.clone();
    let mut accounting = OutputAccounting::new(spec.pty.is_some(), spec.output_cap);
    let register = ProcessRequest::Register {
        key: key.clone(),
        fingerprint: spec.fingerprint(),
        spec,
    };
    // Registration and start use the same paced retry for a busy lane —
    // a concurrent exchange can hold the single connection only for one
    // bounded wire budget.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if cancellation.is_cancelled()
            || root_retired.is_cancelled()
            || events.closed.is_cancelled()
        {
            // A record may already exist Client-side (a register reply
            // could be in flight or lost) — the cancel settles a landed
            // record or plants the tombstone that fences a late one.
            return Err(domain(ErrorData::ConnectionClosed));
        }
        match exchange(&endpoint, register.clone()).await {
            Exchange::Reply(ProcessReply::Registered { .. }) => break,
            Exchange::Reply(ProcessReply::State {
                state: OperationState::Settled(snapshot),
                ..
            }) => {
                // An exact replay attaching an already-released record: its
                // bytes were consumed by the earlier attempt and are not
                // available here. Only an output-free outcome can be
                // reported exactly; anything else is honestly unknown.
                entry.reconciled.store(
                    snapshot.physical_settlement_proven,
                    std::sync::atomic::Ordering::Release,
                );
                if snapshot.final_sequence.get() != 0
                    || snapshot.stdout_total.get() != 0
                    || snapshot.stderr_total.get() != 0
                {
                    return Err(domain(ErrorData::OutcomeUnknown));
                }
                return Ok(result_from(snapshot, streaming, Vec::new(), Vec::new()));
            }
            Exchange::Reply(ProcessReply::Refused { reason, .. }) => {
                entry
                    .reconciled
                    .store(true, std::sync::atomic::Ordering::Release);
                return Err(match reason {
                    SpecRefusal::InvalidCwd => domain(ErrorData::EnvironmentUnauthorized),
                    SpecRefusal::CapacityExhausted | SpecRefusal::TombstonesSaturated => {
                        domain(ErrorData::EnvironmentCapacityOrConflict)
                    }
                    _ => domain(ErrorData::InvalidParams),
                });
            }
            Exchange::Reply(ProcessReply::Conflict { .. }) => {
                entry
                    .reconciled
                    .store(true, std::sync::atomic::Ordering::Release);
                return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
            }
            Exchange::Reply(_) => return Err(domain(ErrorData::OperationFailed)),
            Exchange::Busy => {}
            Exchange::Failed(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(domain(ErrorData::OutcomeUnknown));
        }
        tokio::time::sleep(OPERATION_POLL).await;
    }

    #[cfg(test)]
    pause_start(&process_id, &key, false).await;

    let start_state = loop {
        if cancellation.is_cancelled()
            || root_retired.is_cancelled()
            || events.closed.is_cancelled()
        {
            // The record is registered but START was never dispatched —
            // the control-lane cancel settles it instead of leaving the
            // `Registered` record to pin a capacity slot.
            return Err(domain(ErrorData::ConnectionClosed));
        }
        #[cfg(test)]
        pause_start(&process_id, &key, true).await;
        match exchange(&endpoint, ProcessRequest::Start { key: key.clone() }).await {
            Exchange::Reply(ProcessReply::State { state, .. }) => break state,
            Exchange::Reply(ProcessReply::Unknown { .. }) => {
                return Err(domain(ErrorData::OperationFailed));
            }
            Exchange::Reply(ProcessReply::Conflict { .. }) => {
                return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
            }
            Exchange::Reply(_) => return Err(domain(ErrorData::OperationFailed)),
            Exchange::Busy => {}
            Exchange::Failed(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(domain(ErrorData::OutcomeUnknown));
        }
        tokio::time::sleep(OPERATION_POLL).await;
    };

    if accounting.observe(&start_state).is_err() {
        return Err(contradicted(&endpoint).await);
    }
    let mut settled = if let OperationState::Settled(snapshot) = start_state {
        entry.reconciled.store(
            snapshot.physical_settlement_proven,
            std::sync::atomic::Ordering::Release,
        );
        Some(snapshot)
    } else {
        None
    };
    let mut cancelled = false;
    // The UI can no longer receive output: the driver keeps tracking the
    // physical operation to settlement and releases it, never delivering.
    let mut detached = false;
    // Cancellation before START settles a minimal record with no journal.
    let mut drained = settled.is_some_and(|snapshot| {
        snapshot.disposition == SettledDisposition::Cancelled
            && snapshot.final_sequence.get() == 0
            && snapshot.stdout_total.get() == 0
            && snapshot.stderr_total.get() == 0
    });
    let mut finished = false;
    let mut delta_sequence = 0_u64;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut outbox: VecDeque<(OutputStream, String, bool)> = VecDeque::new();
    let mut last_receipt: Option<oneshot::Receiver<()>> = None;
    loop {
        if !cancelled
            && (cancellation.is_cancelled()
                || root_retired.is_cancelled()
                || events.closed.is_cancelled()
                || !manager
                    .process_policy()
                    .environment_roots
                    .iter()
                    .any(|root| endpoint.workspace().starts_with(root)))
        {
            cancelled = true;
            cancellation.cancel();
        }
        // Settle the outstanding delivery receipt without blocking the
        // lane: a queued notification the consumer dropped — never
        // delivered — ends the result honestly instead of hanging.
        if let Some(receipt) = last_receipt.as_mut() {
            match receipt.try_recv() {
                Ok(()) => last_receipt = None,
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    detached = true;
                    last_receipt = None;
                    outbox.clear();
                    cancellation.cancel();
                }
            }
        }
        // The settled snapshot is checked once the journal is fully
        // delivered. A truncation still unannounced by then — exact cap
        // with truncation visible only in the totals — is announced now,
        // after the retained output.
        if !finished
            && !detached
            && drained
            && let Some(snapshot) = settled
        {
            if accounting.finish(&snapshot).is_err() {
                return Err(contradicted(&endpoint).await);
            }
            if streaming {
                for (lane, truncated) in [snapshot.stdout_truncated, snapshot.stderr_truncated]
                    .into_iter()
                    .enumerate()
                {
                    if truncated && accounting.announce(lane) {
                        outbox.push_back((lane_stream(lane), String::new(), true));
                    }
                }
            }
            finished = true;
        }
        // While the operation is live at most one delta is outstanding
        // — the consumer's receipt is the delivery fence. Once settled
        // the outbox drains regardless so a consumer that never reads
        // overflows the bounded event queue rather than stalling the
        // operation on an unresolvable receipt.
        while let Some((stream, delta_base64, cap_reached)) = outbox.pop_front() {
            if last_receipt.is_some() && settled.is_none() {
                outbox.push_front((stream, delta_base64, cap_reached));
                break;
            }
            delta_sequence += 1;
            if let Ok(receipt) = events.emit(NotificationMethod::CommandOutputDelta(OutputDelta {
                process_id: process_id.clone(),
                sequence: delta_sequence,
                stream,
                delta_base64,
                cap_reached,
            })) {
                last_receipt = Some(receipt);
            } else {
                // The consumer is gone: the rest of the output is
                // discarded and the operation is cancelled — the driver
                // still settles and releases it.
                detached = true;
                outbox.clear();
                cancellation.cancel();
            }
        }
        // Drain the ordered journal one bounded page at a time until the
        // cursor sits at the journal's current end. A live operation
        // holds paging while a delivery receipt is still outstanding,
        // and only once the previous page's chunks are all queued — so
        // the outbox never holds more than one page plus its markers.
        if !drained
            && !detached
            && outbox.is_empty()
            && (last_receipt.is_none() || settled.is_some())
        {
            match exchange(
                &endpoint,
                ProcessRequest::OutputPage {
                    key: key.clone(),
                    cursor: DecimalU64::new(accounting.cursor()),
                    max_bytes: u32::try_from(
                        rustx_execution_protocol::process::MAX_PROCESS_DATA_BYTES,
                    )
                    .unwrap_or(u32::MAX),
                },
            )
            .await
            {
                Exchange::Reply(ProcessReply::Output {
                    sequence,
                    stream,
                    offset,
                    delta_base64,
                    capped,
                    last,
                    ..
                }) => {
                    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&delta_base64)
                    else {
                        return Err(contradicted(&endpoint).await);
                    };
                    let delivery = output::Delivery {
                        sequence: sequence.get(),
                        stream,
                        offset: offset.get(),
                        bytes: &bytes,
                        capped,
                        last,
                    };
                    // Stream, offset, and cap are verified before any byte
                    // is buffered or announced.
                    let Ok(lane) = accounting.admit(&delivery) else {
                        return Err(contradicted(&endpoint).await);
                    };
                    drained |= last;
                    if streaming {
                        let stream = match stream {
                            ProcessStream::Stdout | ProcessStream::Pty => OutputStream::Stdout,
                            ProcessStream::Stderr => OutputStream::Stderr,
                        };
                        // Emit bounded deltas at the physical read
                        // granularity so the bounded event queue keeps
                        // fencing a consumer that never drains.
                        for chunk in bytes.chunks(OUTPUT_CHUNK_BYTES) {
                            outbox.push_back((
                                stream,
                                base64::engine::general_purpose::STANDARD.encode(chunk),
                                false,
                            ));
                        }
                        // The lane's retained prefix ends with this page once
                        // it reaches the cap: announce the truncation live.
                        if accounting.prefix_complete(lane) && accounting.announce(lane) {
                            outbox.push_back((lane_stream(lane), String::new(), true));
                        }
                    } else if lane == 0 {
                        stdout.extend_from_slice(&bytes);
                    } else {
                        stderr.extend_from_slice(&bytes);
                    }
                    continue;
                }
                Exchange::Reply(ProcessReply::OutputPending { .. }) => {
                    if accounting.pending_allowed().is_err() {
                        return Err(contradicted(&endpoint).await);
                    }
                }
                Exchange::Busy => {}
                Exchange::Reply(ProcessReply::OutputEnd { final_sequence, .. }) => {
                    if accounting.end(final_sequence.get()).is_err() {
                        return Err(contradicted(&endpoint).await);
                    }
                    drained = true;
                }
                Exchange::Reply(_) => return Err(contradicted(&endpoint).await),
                Exchange::Failed(error) => return Err(error),
            }
        }
        if settled.is_none() {
            match exchange(&endpoint, ProcessRequest::Query { key: key.clone() }).await {
                Exchange::Reply(ProcessReply::State { state, .. }) => {
                    if accounting.observe(&state).is_err() {
                        return Err(contradicted(&endpoint).await);
                    }
                    if let OperationState::Settled(snapshot) = state {
                        entry.reconciled.store(
                            snapshot.physical_settlement_proven,
                            std::sync::atomic::Ordering::Release,
                        );
                        settled = Some(snapshot);
                    }
                }
                Exchange::Busy => {}
                Exchange::Reply(_) => return Err(contradicted(&endpoint).await),
                Exchange::Failed(error) => return Err(error),
            }
        }
        if settled.is_some() && (detached || (finished && drained && outbox.is_empty())) {
            break;
        }
        tokio::time::sleep(OPERATION_POLL).await;
    }
    let snapshot = settled.expect("settled after drain");
    let delivered = match last_receipt.take() {
        _ if detached => Err(domain(ErrorData::ConnectionClosed)),
        Some(receipt) => events
            .emitted(receipt)
            .await
            .map_err(|()| domain(ErrorData::ConnectionClosed)),
        None => Ok(()),
    };
    // The exact final sequence releases the settled journal whether its
    // bytes were delivered or discarded. A failed release leaves a
    // bounded record and never changes the known outcome.
    release_output(&endpoint, &process_id, &key, snapshot.final_sequence).await;
    delivered?;
    Ok(result_from(snapshot, streaming, stdout, stderr))
}

/// Arms the operation's one cancellation watcher, independent of the driver's
/// data exchanges: UI detach, root retirement, or a driver failure wakes it,
/// and it delivers the keyed cancel on the control lane exactly once. Its
/// budget starts at first observation, not after the blocking-pool wait.
/// Disarming (dropping the sender) without a wake-up sends nothing.
fn watch_cancellation(
    entry: Arc<Entry>,
    cancellation: crate::runtime::CancellationSignal,
    root_retired: crate::runtime::CancellationSignal,
    closed: crate::runtime::CancellationSignal,
    disarmed: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // An explicit disarm ends the watch. A dropped sender leaves
        // cleanup to the watcher if physical ownership is still unresolved.
        let disarmed_ok = tokio::select! {
            biased;
            disarmed = disarmed => disarmed.is_ok(),
            () = cancellation.cancelled() => false,
            () = root_retired.cancelled() => false,
            () = closed.cancelled() => false,
        };
        if disarmed_ok {
            return;
        }
        let deadline = std::time::Instant::now() + CANCEL_BUDGET;
        let _ = tokio::task::spawn_blocking(move || {
            entry.endpoint.cancel_operation(&entry.key, deadline, || {
                entry.reconciled.load(std::sync::atomic::Ordering::Acquire)
            });
        })
        .await;
    })
}

/// Releases the settled journal through its exact final sequence. Only a
/// refusal proven to precede the wire (`Busy`) is retried, within one
/// absolute budget; any other answer — including an ambiguous transport
/// failure — is final and never relaunches or retries the operation.
async fn release_output(
    endpoint: &Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    process_id: &str,
    key: &ProcessOperationKey,
    final_sequence: DecimalU64,
) {
    let deadline = std::time::Instant::now() + RELEASE_BUDGET;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return;
        }
        let answer = if take_forced_release_busy(process_id) {
            Exchange::Busy
        } else {
            exchange_until(
                endpoint,
                ProcessRequest::ReleaseOutput {
                    key: key.clone(),
                    final_sequence,
                },
                deadline,
            )
            .await
        };
        record_release(process_id, &answer);
        match answer {
            Exchange::Busy => {
                tokio::time::sleep(OPERATION_POLL.min(deadline - std::time::Instant::now())).await;
            }
            _ => return,
        }
    }
}

/// Server-side START-dispatch hold for scripted START-race tests: the
/// physical START gate itself lives in the Execution Client (its own
/// in-crate races hold it); this seam proves revocation fences a pending
/// dispatch on the Server side. Holds are keyed by the operation's
/// `process_id`: a parallel test's `command/exec` never claims a hold
/// installed for another operation.
#[cfg(test)]
struct PendingStartHold {
    process_id: String,
    after_gate: bool,
    entered: tokio::sync::watch::Sender<Option<ProcessOperationKey>>,
    released: tokio::sync::watch::Sender<bool>,
}

#[cfg(test)]
static START_HOLDS: std::sync::Mutex<Vec<PendingStartHold>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
async fn pause_start(process_id: &str, key: &ProcessOperationKey, after_gate: bool) {
    let hold = {
        let mut pending = START_HOLDS.lock().expect("start hold");
        pending
            .iter()
            .position(|item| item.process_id == process_id && item.after_gate == after_gate)
            .map(|index| pending.remove(index))
    };
    if let Some(hold) = hold {
        let _ = hold.entered.send(Some(key.clone()));
        hold.released
            .subscribe()
            .wait_for(|released| *released)
            .await
            .expect("start hold");
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct StartHold {
    process_id: String,
    entered: tokio::sync::watch::Receiver<Option<ProcessOperationKey>>,
    released: tokio::sync::watch::Sender<bool>,
}

#[cfg(test)]
impl StartHold {
    pub async fn await_entered(&self) {
        self.key().await;
    }

    pub async fn key(&self) -> ProcessOperationKey {
        self.entered
            .clone()
            .wait_for(Option::is_some)
            .await
            .expect("start hold")
            .clone()
            .expect("published operation")
    }

    pub fn release(&self) {
        self.released.send_replace(true);
    }
}

#[cfg(test)]
impl Drop for StartHold {
    fn drop(&mut self) {
        START_HOLDS
            .lock()
            .expect("start hold")
            .retain(|pending| pending.process_id != self.process_id);
    }
}

#[cfg(test)]
pub(crate) fn hold_start(process_id: &str) -> StartHold {
    install_start_hold(process_id, false)
}

#[cfg(test)]
pub(crate) fn hold_start_after_gate(process_id: &str) -> StartHold {
    install_start_hold(process_id, true)
}

#[cfg(test)]
fn install_start_hold(process_id: &str, after_gate: bool) -> StartHold {
    let (entered_tx, entered_rx) = tokio::sync::watch::channel(None);
    let (released_tx, _released_rx) = tokio::sync::watch::channel(false);
    START_HOLDS
        .lock()
        .expect("start hold")
        .push(PendingStartHold {
            process_id: process_id.to_string(),
            after_gate,
            entered: entered_tx,
            released: released_tx.clone(),
        });
    StartHold {
        process_id: process_id.to_string(),
        entered: entered_rx,
        released: released_tx,
    }
}

#[cfg(test)]
type CancelWatcherSender = oneshot::Sender<tokio::task::JoinHandle<()>>;
#[cfg(test)]
static CANCEL_WATCHERS: Mutex<Vec<(String, CancelWatcherSender)>> = Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn observe_cancellation_watcher(
    id: &str,
) -> oneshot::Receiver<tokio::task::JoinHandle<()>> {
    let (send, receive) = oneshot::channel();
    CANCEL_WATCHERS
        .lock()
        .expect("cancellation watchers")
        .push((id.to_owned(), send));
    receive
}

#[cfg(test)]
fn record_cancellation_watcher(
    id: &str,
    watcher: tokio::task::JoinHandle<()>,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut observers = CANCEL_WATCHERS.lock().expect("cancellation watchers");
    if let Some(index) = observers.iter().position(|(process, _)| id == process) {
        let (_, sender) = observers.remove(index);
        let _ = sender.send(watcher);
        None
    } else {
        Some(watcher)
    }
}

#[cfg(test)]
static FORCED_RELEASE_BUSY: std::sync::Mutex<Vec<(String, u32)>> =
    std::sync::Mutex::new(Vec::new());
/// A typed release answer, recorded without any peer-supplied text.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseOutcome {
    Busy,
    Released,
    Refused,
    Failed,
}
#[cfg(test)]
type ReleaseLog = std::sync::Arc<std::sync::Mutex<Vec<ReleaseOutcome>>>;
#[cfg(test)]
static RELEASE_OBSERVERS: std::sync::Mutex<Vec<(String, ReleaseLog)>> =
    std::sync::Mutex::new(Vec::new());

/// Test seam: a keyed observer of one operation's release answers, installed
/// for one test and removed when dropped.
#[cfg(test)]
pub(crate) struct ReleaseObserver {
    process_id: String,
    log: ReleaseLog,
}
#[cfg(test)]
pub(crate) fn observe_release(process_id: &str) -> ReleaseObserver {
    let log = ReleaseLog::default();
    RELEASE_OBSERVERS
        .lock()
        .expect("release observers")
        .push((process_id.to_owned(), log.clone()));
    ReleaseObserver {
        process_id: process_id.to_owned(),
        log,
    }
}
#[cfg(test)]
impl ReleaseObserver {
    pub(crate) fn outcomes(&self) -> Vec<ReleaseOutcome> {
        self.log.lock().expect("release log").clone()
    }
}
#[cfg(test)]
impl Drop for ReleaseObserver {
    fn drop(&mut self) {
        RELEASE_OBSERVERS
            .lock()
            .expect("release observers")
            .retain(|(id, _)| *id != self.process_id);
    }
}

/// Test seam: the next `times` release attempts of `process_id` are refused
/// before the wire as a busy data lane.
#[cfg(test)]
pub(crate) fn force_release_busy(process_id: &str, times: u32) {
    FORCED_RELEASE_BUSY
        .lock()
        .expect("release busy")
        .push((process_id.to_owned(), times));
}
#[cfg(test)]
fn take_forced_release_busy(process_id: &str) -> bool {
    let mut forced = FORCED_RELEASE_BUSY.lock().expect("release busy");
    let Some(index) = forced.iter().position(|(id, _)| id == process_id) else {
        return false;
    };
    forced[index].1 -= 1;
    if forced[index].1 == 0 {
        forced.remove(index);
    }
    true
}
#[cfg(not(test))]
fn take_forced_release_busy(_process_id: &str) -> bool {
    false
}
#[cfg(test)]
fn record_release(process_id: &str, answer: &Exchange) {
    let outcome = match answer {
        Exchange::Busy => ReleaseOutcome::Busy,
        Exchange::Reply(ProcessReply::OutputReleased { .. }) => ReleaseOutcome::Released,
        Exchange::Reply(ProcessReply::Conflict { .. } | ProcessReply::Unknown { .. }) => {
            ReleaseOutcome::Refused
        }
        Exchange::Failed(_) | Exchange::Reply(_) => ReleaseOutcome::Failed,
    };
    for (id, log) in RELEASE_OBSERVERS.lock().expect("release observers").iter() {
        if id == process_id {
            log.lock().expect("release log").push(outcome);
        }
    }
}
#[cfg(not(test))]
fn record_release(_process_id: &str, _answer: &Exchange) {}

fn result_from(
    snapshot: SettledSnapshot,
    streaming: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> ExecResult {
    ExecResult {
        exit_code: snapshot.exit_code,
        status: match snapshot.disposition {
            SettledDisposition::Known => ExecStatus::Completed,
            SettledDisposition::TimedOut => ExecStatus::TimedOut,
            SettledDisposition::Cancelled => ExecStatus::Terminated,
            SettledDisposition::Unknown | SettledDisposition::Failed => {
                ExecStatus::ProcessControlFailed
            }
        },
        stdout_base64: if streaming {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(stdout)
        },
        stderr_base64: if streaming {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(stderr)
        },
        stdout_truncated: snapshot.stdout_truncated,
        stderr_truncated: snapshot.stderr_truncated,
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::{hold_expired_input_cleanup, hold_expired_input_poll, hold_input_receipt};
