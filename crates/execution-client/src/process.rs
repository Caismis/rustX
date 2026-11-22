//! Client-owned managed process operations.
//!
//! One binding owns a finite record map keyed by `OperationId`: a record
//! is `Full` (complete spec, descended cwd descriptor, ordered output
//! journal, worker wiring) or `Minimal` (fingerprint, settled snapshot and
//! independently retained results after `ReleaseOutput`). `Full` records — including
//! settled-but-unreleased ones — occupy the per-Session slots.
//!
//! Every request is gated on the bound [`Admission`]: a key naming a
//! foreign incarnation/session/instance/generation is refused before any
//! lookup. Only `Start` may cross the physical spawn boundary: the outer
//! supervisor spawn is *not* business START — `Running` is published only
//! when the runner's [`ProcessWitness`] proves the complete START frame
//! crossed its grant. Cancellation rides the operation's own
//! [`ProcessSignal`]; `cancel_with` reads the crossing under the same
//! grant, so `CancelledBeforeStart` vs `CancellationRequested` is atomic
//! and containment is only ever claimed after real settlement.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::process::{
    BashCaptureResult, InputOutcome, MAX_OPERATIONS_PER_SESSION, MAX_PROCESS_DATA_BYTES,
    MAX_PROCESS_IDENTITIES_PER_GENERATION, MAX_PROCESS_OUTPUT_BYTES, OperationId, OperationState,
    ProcessOperationKey, ProcessProgram, ProcessReply, ProcessRequest, ProcessSpec, ProcessStdin,
    ProcessStream, PtySpec, SettledDisposition, SettledSnapshot, SpecFingerprint, SpecRefusal,
};
use rustx_execution_protocol::{Admission, DecimalU64, ProcessCancelDisposition};
use rustx_process_supervisor::invocation::ProgramInvocation;
use rustx_process_supervisor::process_runner::{
    ProcessOutcomeIntent, ProcessWitness, SupervisedCommandRunner, SupervisedCommandSpec,
};
use rustx_process_supervisor::signal::ProcessSignal;
use sha2::{Digest as _, Sha256};

use crate::bash_capture::{Capture, CaptureFile, Captures};
use crate::terminal::Terminal;

/// The bounded ordered segment journal of one operation — a metadata
/// bound, never one event per byte. When the bound is met the retained
/// prefix freezes and the stream is honestly marked truncated.
const MAX_JOURNAL_SEGMENTS: usize = 128;

/// The bounded accepted-input queue; a write refused for capacity is a
/// `Conflict`, never silently queued.
const INPUT_QUEUE_DEPTH: usize = 8;

/// One journal segment: `bytes` covers the global output offsets
/// `[start, end)` of `stream`, whose first byte is that stream's
/// `stream_offset`.
struct Segment {
    start: u64,
    end: u64,
    stream: ProcessStream,
    stream_offset: u64,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct StreamStats {
    retained: u64,
    total: u64,
    offset: u64,
    truncated: bool,
}

/// The interleaved ordered-output journal. The global cursor is a byte
/// boundary in the retained interleaved region `[base, end)`; every
/// segment knows its own stream-local offset.
#[derive(Default)]
struct Journal {
    segments: VecDeque<Segment>,
    base: u64,
    end: u64,
    stdout: StreamStats,
    stderr: StreamStats,
    /// The metadata bound was hit: the retained prefix is frozen and no
    /// further bytes are admitted — later totals still count truthfully.
    frozen: bool,
}

impl Journal {
    fn stream(&mut self, stream: ProcessStream) -> &mut StreamStats {
        match stream {
            ProcessStream::Stdout | ProcessStream::Pty => &mut self.stdout,
            ProcessStream::Stderr => &mut self.stderr,
        }
    }

    /// Append one drained chunk: totals always advance; the retained
    /// prefix is bounded per stream and globally by segment count. Once
    /// either bound is hit the prefix freezes and the stream is marked
    /// truncated — the retained evidence is never silently edited.
    fn push(&mut self, stream: ProcessStream, bytes: &[u8], cap: u32) {
        if self.frozen {
            self.stream(stream).total += bytes.len() as u64;
            self.stream(stream).truncated = true;
            return;
        }
        let stats = self.stream(stream);
        stats.total += bytes.len() as u64;
        let cap = u64::from(cap).min(MAX_PROCESS_OUTPUT_BYTES as u64);
        let room = cap.saturating_sub(stats.retained);
        let room = usize::try_from(room).unwrap_or(usize::MAX);
        let kept = &bytes[..bytes.len().min(room)];
        if kept.len() < bytes.len() {
            stats.truncated = true;
        }
        if kept.is_empty() {
            return;
        }
        let stream_offset = stats.offset;
        stats.offset += kept.len() as u64;
        stats.retained += kept.len() as u64;
        if let Some(tail) = self.segments.back_mut()
            && tail.stream == stream
            && tail.end == self.end
            && tail.bytes.len() + kept.len() <= MAX_PROCESS_DATA_BYTES
        {
            tail.bytes.extend_from_slice(kept);
            tail.end += kept.len() as u64;
            self.end = tail.end;
            return;
        }
        if self.segments.len() >= MAX_JOURNAL_SEGMENTS {
            self.frozen = true;
            self.stream(stream).truncated = true;
            return;
        }
        let start = self.end;
        self.end += kept.len() as u64;
        self.segments.push_back(Segment {
            start,
            end: self.end,
            stream,
            stream_offset,
            bytes: kept.to_vec(),
        });
    }
}

enum InputKind {
    Write { bytes: Vec<u8>, close: bool },
    Close,
    Resize(PtySpec),
}

impl InputKind {
    fn fingerprint(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        match self {
            Self::Write { bytes, close } => {
                digest.update([0, u8::from(*close)]);
                digest.update(bytes);
            }
            Self::Close => digest.update([1]),
            Self::Resize(size) => {
                digest.update([2]);
                digest.update(size.rows.to_be_bytes());
                digest.update(size.cols.to_be_bytes());
            }
        }
        digest.finalize().into()
    }
}

struct RetainedInput {
    fingerprint: [u8; 32],
    outcome: InputOutcome,
}

/// One admitted ordered control — the data lane acknowledges admission
/// only; the worker records the physical outcome into `receipts` for a
/// later `InputReceipt` query.
struct InputEvent {
    kind: InputKind,
    sequence: u64,
}

/// A record while the operation owns physical resources.
struct Operation {
    key: ProcessOperationKey,
    fingerprint: SpecFingerprint,
    spec: ProcessSpec,
    /// The descended cwd descriptor retained at registration — the
    /// spawn's cwd authority, never a Server-supplied path.
    cwd: PackageDirectory,
    state: OperationState,
    journal: Journal,
    input_sequence: u64,
    input: Option<tokio::sync::mpsc::Sender<InputEvent>>,
    /// The receiver half created at registration — `Start` takes it into
    /// the worker so ordered input admitted while `Registered`/`Starting`
    /// queues honestly instead of being refused for a missing sink.
    input_rx: Option<tokio::sync::mpsc::Receiver<InputEvent>>,
    /// Each admitted input owns a slot until its terminal receipt is
    /// collected or the observer explicitly abandons it.
    receipts: BTreeMap<u64, RetainedInput>,
    #[cfg(test)]
    pipe_close_received: Option<std::sync::mpsc::Sender<()>>,
    #[cfg(test)]
    partial_input_failure: Option<PartialInputFailure>,
    cancel: ProcessSignal,
    witness: Option<ProcessWitness>,
    bash_file: Option<Arc<CaptureFile>>,
    bash_result: Option<BashCaptureResult>,
}

/// A record after an exact `ReleaseOutput` consumed the journal, retaining
/// the fingerprint, settled snapshot and any independently owned results.
/// A `None` fingerprint is the cancelled-before-register
/// tombstone a control-lane cancel plants for an owned key whose
/// `Register` has not landed yet: a later `Register` attaches the
/// `Cancelled` snapshot and can never spawn.
struct Minimal {
    fingerprint: Option<SpecFingerprint>,
    snapshot: SettledSnapshot,
    bash_file: Option<(String, Arc<CaptureFile>)>,
    never_started: bool,
    receipts: BTreeMap<u64, RetainedInput>,
}

impl Minimal {
    fn cancelled_before_start(fingerprint: Option<SpecFingerprint>) -> Self {
        Self {
            fingerprint,
            snapshot: SettledSnapshot {
                disposition: SettledDisposition::Cancelled,
                physical_settlement_proven: true,
                exit_code: None,
                stdout_total: 0.into(),
                stderr_total: 0.into(),
                stdout_truncated: false,
                stderr_truncated: false,
                final_sequence: 0.into(),
            },
            bash_file: None,
            never_started: true,
            receipts: BTreeMap::new(),
        }
    }
}

struct PreparingBash {
    fingerprint: SpecFingerprint,
    cancelled: bool,
    reply: Arc<OnceLock<ProcessReply>>,
}

enum RegistrationReply {
    Ready(ProcessReply),
    Preparing(Arc<OnceLock<ProcessReply>>),
}

impl RegistrationReply {
    fn wait(self) -> ProcessReply {
        match self {
            Self::Ready(reply) => reply,
            Self::Preparing(reply) => reply.wait().clone(),
        }
    }
}

enum Record {
    PreparingBash(PreparingBash),
    Full(Box<Operation>),
    Minimal(Minimal),
}

impl Record {
    fn capture_file(&self) -> Option<&Arc<CaptureFile>> {
        match self {
            Self::PreparingBash(_) => None,
            Self::Full(operation) => operation.bash_file.as_ref(),
            Self::Minimal(operation) => operation.bash_file.as_ref().map(|(_, file)| file),
        }
    }

    fn forget_capture(&mut self, file: &Arc<CaptureFile>) {
        if self
            .capture_file()
            .is_some_and(|retained| Arc::ptr_eq(retained, file))
        {
            match self {
                Self::PreparingBash(_) => unreachable!("preparation has no registered capture"),
                Self::Full(operation) => operation.bash_file = None,
                Self::Minimal(operation) => operation.bash_file = None,
            }
        }
    }
}

struct Inner {
    #[cfg(test)]
    fail_pty_read: bool,
    records: BTreeMap<OperationId, Record>,
}

impl Inner {
    /// Called before resolving cwd and again before first allocation. An
    /// already-owned identity needs neither a new directory lookup nor room
    /// in the registry. Every new record reserves its eventual tombstone.
    fn registration_reply(
        &mut self,
        key: &ProcessOperationKey,
        fingerprint: SpecFingerprint,
    ) -> Option<RegistrationReply> {
        let reply = match self.records.get_mut(&key.operation) {
            Some(Record::PreparingBash(pending)) if pending.fingerprint == fingerprint => {
                return Some(RegistrationReply::Preparing(pending.reply.clone()));
            }
            Some(Record::Full(op)) if op.key == *key && op.fingerprint == fingerprint => {
                // The original journal is still owned. The caller can
                // attach to it and retrieve the actual state with START or Query.
                ProcessReply::Registered { key: key.clone() }
            }
            Some(Record::Minimal(minimal)) => {
                if *minimal.fingerprint.get_or_insert(fingerprint) == fingerprint {
                    ProcessReply::State {
                        key: key.clone(),
                        state: OperationState::Settled(minimal.snapshot),
                    }
                } else {
                    ProcessReply::Conflict { key: key.clone() }
                }
            }
            Some(Record::Full(_) | Record::PreparingBash(_)) => {
                ProcessReply::Conflict { key: key.clone() }
            }
            None => {
                let reason = if self.records.len() >= MAX_PROCESS_IDENTITIES_PER_GENERATION {
                    SpecRefusal::TombstonesSaturated
                } else if self
                    .records
                    .values()
                    .filter(|r| !matches!(r, Record::Minimal(_)))
                    .count()
                    >= MAX_OPERATIONS_PER_SESSION
                {
                    SpecRefusal::CapacityExhausted
                } else {
                    return None;
                };
                ProcessReply::Refused {
                    key: key.clone(),
                    reason,
                }
            }
        };
        Some(RegistrationReply::Ready(reply))
    }
}

/// The finite per-binding operation registry, the bound identity, and
/// the Client-owned physical runtime.
pub(crate) struct Operations {
    inner: Arc<Mutex<Inner>>,
    admission: Admission,
    root: PackageDirectory,
    workspace: PathBuf,
    /// The independent generation authority bound to every runner's
    /// START guard — a different signal family than each operation's
    /// invocation cancellation, so the grants never share a mutex.
    revoke: ProcessSignal,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    workers: Arc<WorkerCount>,
    captures: Arc<Captures>,
    python: Arc<rustx_execution_client::workspace_python::WorkspacePython>,
    /// Test-only physical controls, each owned by exactly one operation id.
    #[cfg(test)]
    runner_controls:
        Mutex<BTreeMap<OperationId, rustx_process_supervisor::process_runner::RunnerTestControl>>,
    #[cfg(test)]
    capture_holds: Mutex<BTreeMap<OperationId, CaptureHold>>,
}

#[cfg(test)]
type CaptureHold = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

// Keep the capture ahead of its worker guard in the drop order, including
// when the runtime discards the START future before polling it.
struct ActiveProcess {
    capture: Option<Capture>,
    worker: WorkerGuard,
}

struct WorkerCount {
    state: Mutex<WorkerState>,
    drained: Condvar,
}

impl WorkerCount {
    fn retain_work(self: &Arc<Self>) -> WorkerGuard {
        self.state.lock().expect("worker count").live += 1;
        WorkerGuard {
            workers: self.clone(),
            unconfirmed: false,
        }
    }
}

#[derive(Default)]
struct WorkerState {
    live: usize,
    /// Completed workers for which no physical settlement was proven.
    unconfirmed: usize,
}

fn registered_capture(op: &Operation) -> Option<(String, Arc<CaptureFile>)> {
    let ProcessProgram::Shell { conversation, .. } = &op.spec.program else {
        return None;
    };
    Some((conversation.clone(), op.bash_file.as_ref()?.clone()))
}

impl Operations {
    /// Creates the registry for one admitted binding; the workspace root
    /// descriptor is the Client's own — never the Server's view.
    pub(crate) fn new(admission: Admission) -> io::Result<Self> {
        let revoke = ProcessSignal::new();
        let root = PackageDirectory::open(&admission.workspace)?;
        let root_meta = std::fs::File::from(root.descriptor().try_clone()?).metadata()?;
        let python = Arc::new(
            rustx_execution_client::workspace_python::WorkspacePython::new(
                admission.workspace.clone(),
                (root_meta.dev(), root_meta.ino()),
                revoke.child(),
            ),
        );
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                #[cfg(test)]
                fail_pty_read: false,
                records: BTreeMap::new(),
            })),
            root,
            workspace: admission.workspace.clone(),
            admission,
            revoke,
            python,
            runtime: Mutex::new(Some(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?,
            )),
            captures: Arc::new(Captures::new()),
            workers: Arc::new(WorkerCount {
                state: Mutex::new(WorkerState::default()),
                drained: Condvar::new(),
            }),
            #[cfg(test)]
            runner_controls: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            capture_holds: Mutex::new(BTreeMap::new()),
        })
    }

    /// The bound identity gate: a key naming any foreign
    /// incarnation/session/instance/generation is refused before lookup —
    /// including `Register`.
    pub(crate) fn own_key(&self, key: &ProcessOperationKey) -> bool {
        key.incarnation == self.admission.server_incarnation
            && key.session == self.admission.session
            && key.instance == self.admission.instance
            && key.generation == self.admission.generation
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub(crate) fn workspace_root(&self) -> &PackageDirectory {
        &self.root
    }

    pub(crate) fn auxiliary_signal(&self) -> ProcessSignal {
        self.revoke.child()
    }

    pub(crate) fn start_auxiliary(
        &self,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> bool {
        let runtime = self.runtime.lock().expect("Client runtime");
        let Some(runtime) = runtime.as_ref() else {
            return false;
        };
        let Some(guard) = self.admit_auxiliary() else {
            return false;
        };
        runtime.spawn(async move {
            let _guard = guard;
            future.await;
        });
        true
    }

    pub(crate) fn admit_auxiliary(&self) -> Option<WorkerGuard> {
        let mut state = self.workers.state.lock().expect("worker count");
        if self.revoke.is_cancelled() {
            return None;
        }
        state.live += 1;
        Some(WorkerGuard {
            workers: self.workers.clone(),
            unconfirmed: false,
        })
    }

    pub(crate) fn workspace_python_status(
        &self,
    ) -> rustx_execution_protocol::WorkspacePythonStatus {
        if self.root.verify().is_err() {
            return rustx_execution_protocol::WorkspacePythonStatus::Unavailable;
        }
        self.python.status()
    }

    pub(crate) fn schedule_workspace_python(&self) -> bool {
        if self.root.verify().is_err() || self.revoke.is_cancelled() {
            return false;
        }
        let Some(permit) = self.python.prepare_permit() else {
            return true;
        };
        let python = self.python.clone();
        self.start_auxiliary(async move {
            let _permit = permit;
            python.prepare().await;
        })
    }

    /// The control-lane cancellation for one keyed operation.
    pub(crate) fn cancel(&self, key: &ProcessOperationKey) -> ProcessCancelDisposition {
        if !self.own_key(key) {
            return ProcessCancelDisposition::Unknown;
        }
        let mut inner = self.inner.lock().expect("operation registry");
        match inner.records.get_mut(&key.operation) {
            Some(Record::PreparingBash(pending)) => {
                pending.cancelled = true;
                ProcessCancelDisposition::CancelledBeforeStart
            }
            Some(Record::Full(operation)) if operation.key == *key => {
                if let OperationState::Settled(snapshot) = operation.state {
                    settled_cancel_disposition(snapshot)
                } else {
                    // The crossing read shares the START grant: either
                    // cancellation won before any business spawn, or
                    // START provably crossed and the worker performs
                    // real containment.
                    let crossed = operation.cancel.cancel_with(|| {
                        operation
                            .witness
                            .as_ref()
                            .is_some_and(ProcessWitness::start_crossed)
                    });
                    if crossed {
                        ProcessCancelDisposition::CancellationRequested
                    } else {
                        // A `Registered` record has no worker to observe
                        // the signal and nothing physical to retain —
                        // demote it to the settled tombstone now so it
                        // never pins a capacity slot waiting for a
                        // `Start` or `ReleaseOutput` that cannot come.
                        if matches!(operation.state, OperationState::Registered) {
                            resolve_pending(operation);
                            let mut minimal =
                                Minimal::cancelled_before_start(Some(operation.fingerprint));
                            // START can lag behind Server-side background acceptance.
                            // Only the output owner may discard a published capture.
                            minimal.bash_file = registered_capture(operation);
                            minimal.receipts = std::mem::take(&mut operation.receipts);
                            inner
                                .records
                                .insert(key.operation, Record::Minimal(minimal));
                        }
                        ProcessCancelDisposition::CancelledBeforeStart
                    }
                }
            }
            Some(Record::Minimal(retired)) => settled_cancel_disposition(retired.snapshot),
            None => {
                // The cancel outraced the `Register` on the data lane:
                // plant the bounded tombstone so the operation can never
                // spawn — a late `Register` attaches the `Cancelled`
                // snapshot instead of starting an operation the caller
                // already withdrew.
                if inner.records.len() >= MAX_PROCESS_IDENTITIES_PER_GENERATION {
                    return ProcessCancelDisposition::Unknown;
                }
                inner.records.insert(
                    key.operation,
                    Record::Minimal(Minimal::cancelled_before_start(None)),
                );
                ProcessCancelDisposition::CancelledBeforeStart
            }
            _ => ProcessCancelDisposition::Unknown,
        }
    }

    /// The data-lane dispatch: one request, one honest reply.
    pub(crate) fn dispatch(self: &Arc<Self>, request: ProcessRequest) -> ProcessReply {
        match request {
            ProcessRequest::Register {
                key,
                spec,
                fingerprint,
            } => self.register(key, spec, fingerprint, false),
            ProcessRequest::RegisterAgentBash {
                key,
                spec,
                fingerprint,
            } => self.register(key, spec, fingerprint, true),
            ProcessRequest::BashCapture { key } => self.bash_capture(key),
            ProcessRequest::BashOutputTail { key, conversation } => {
                match self
                    .with_bash_file(&key, &conversation, |file| file.tail().ok())
                    .flatten()
                {
                    Some((text, truncated)) => ProcessReply::BashOutputTail {
                        key,
                        text,
                        truncated,
                    },
                    None => ProcessReply::Unknown { key },
                }
            }

            ProcessRequest::DiscardBashOutput { key } => self.discard_bash_output(key),
            ProcessRequest::Start { key } => self.start(key),
            ProcessRequest::Query { key } => self.query(key),
            ProcessRequest::OutputPage {
                key,
                cursor,
                max_bytes,
            } => self.output_page(key, cursor, max_bytes),
            ProcessRequest::WriteStdin {
                key,
                sequence,
                delta_base64,
                close,
            } => self.write_stdin(key, sequence, &delta_base64, close),
            ProcessRequest::CloseStdin { key, sequence } => self.close_stdin(key, sequence),
            ProcessRequest::Resize {
                key,
                sequence,
                size,
            } => self.resize(key, sequence, size),
            ProcessRequest::InputReceipt {
                key,
                sequence,
                abandon_pending,
            } => self.input_receipt(key, sequence, abandon_pending),
            ProcessRequest::ReleaseOutput {
                key,
                final_sequence,
            } => self.release(key, final_sequence),
        }
    }

    fn register(
        &self,
        key: ProcessOperationKey,
        spec: ProcessSpec,
        fingerprint: SpecFingerprint,
        agent_bash: bool,
    ) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        if spec.fingerprint() != fingerprint {
            return ProcessReply::Conflict { key };
        }
        if let Err(reason) = spec.validate() {
            return ProcessReply::Refused { key, reason };
        }
        if matches!(&spec.program, ProcessProgram::Shell { .. }) != agent_bash {
            return ProcessReply::Refused {
                key,
                reason: SpecRefusal::ProgramNotAuthorized,
            };
        }
        let Some(_active) = self.admit_auxiliary() else {
            return ProcessReply::Unknown { key };
        };
        let reply = self
            .inner
            .lock()
            .expect("operation registry")
            .registration_reply(&key, fingerprint);
        if let Some(reply) = reply {
            return reply.wait();
        }
        // Only a new identity checks cwd. The filesystem lookup does not
        // hold the registry lock needed by control cancellation.
        let cwd = self.root.descend(&spec.cwd);
        let mut inner = self.inner.lock().expect("operation registry");
        if self.revoke.is_cancelled() {
            return ProcessReply::Unknown { key };
        }
        if let Some(reply) = inner.registration_reply(&key, fingerprint) {
            drop(inner);
            return reply.wait();
        }
        // A concurrent registration or cancel could have won during the
        // lookup. Only a still-unclaimed key reaches this allocation.
        let Ok(cwd) = cwd else {
            return ProcessReply::Refused {
                key,
                reason: SpecRefusal::InvalidCwd,
            };
        };
        let prepare_capture = matches!(
            &spec.program,
            ProcessProgram::Shell {
                live_output: true,
                ..
            }
        );
        let input = (spec.stdin == ProcessStdin::Streamed || spec.pty.is_some())
            .then(|| tokio::sync::mpsc::channel::<InputEvent>(INPUT_QUEUE_DEPTH));
        let (input_tx, input_rx) = match input {
            Some((tx, rx)) => (Some(tx), Some(rx)),
            None => (None, None),
        };
        let operation = Box::new(Operation {
            key: key.clone(),
            fingerprint,
            spec,
            cwd,
            state: OperationState::Registered,
            journal: Journal::default(),
            input_sequence: 1,
            input: input_tx,
            input_rx,
            receipts: BTreeMap::new(),
            #[cfg(test)]
            pipe_close_received: None,
            #[cfg(test)]
            partial_input_failure: None,
            cancel: ProcessSignal::new(),
            witness: None,
            bash_file: None,
            bash_result: None,
        });
        if !prepare_capture {
            inner.records.insert(key.operation, Record::Full(operation));
            return ProcessReply::Registered { key };
        }
        let reply = Arc::new(OnceLock::new());
        inner.records.insert(
            key.operation,
            Record::PreparingBash(PreparingBash {
                fingerprint,
                cancelled: false,
                reply: reply.clone(),
            }),
        );
        drop(inner);
        // The counted registration on the blocking data lane owns allocation and
        // rollback until the file is transferred to the registry.
        let result = self.prepare_bash_capture(operation);
        reply
            .set(result.clone())
            .expect("one capture registration owner");
        result
    }

    fn prepare_bash_capture(&self, mut operation: Box<Operation>) -> ProcessReply {
        let key = operation.key.clone();
        let allocated = self.captures.create(key.operation);
        let mut inner = self.inner.lock().expect("operation registry");
        let Some(Record::PreparingBash(pending)) = inner.records.get(&key.operation) else {
            unreachable!("capture allocation owns its registration slot");
        };
        if pending.cancelled || self.revoke.is_cancelled() {
            drop(inner);
            let mut minimal = Minimal::cancelled_before_start(Some(operation.fingerprint));
            operation.bash_file = allocated.ok().filter(|file| {
                if let Err(error) = file.discard() {
                    tracing::warn!(%error, "uncommitted Bash capture unlink failed");
                    return true;
                }
                false
            });
            // If deletion could not be confirmed, retain the file and its
            // quota for explicit discard or the binding's revocation sweep.
            minimal.bash_file = registered_capture(&operation);
            let state = OperationState::Settled(minimal.snapshot);
            self.inner
                .lock()
                .expect("operation registry")
                .records
                .insert(key.operation, Record::Minimal(minimal));
            return if self.revoke.is_cancelled() {
                ProcessReply::Unknown { key }
            } else {
                ProcessReply::State { key, state }
            };
        }
        if let Ok(file) = allocated {
            operation.bash_file = Some(file);
            inner.records.insert(key.operation, Record::Full(operation));
            ProcessReply::Registered { key }
        } else {
            inner.records.remove(&key.operation);
            ProcessReply::Refused {
                key,
                reason: SpecRefusal::BashCaptureUnavailable,
            }
        }
    }

    #[allow(clippy::too_many_lines)] // one coherent registration→START-admission path
    fn start(self: &Arc<Self>, key: ProcessOperationKey) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let mut inner = self.inner.lock().expect("operation registry");
        let operation = match inner.records.get_mut(&key.operation) {
            Some(Record::Full(operation)) if operation.key == key => operation,
            Some(Record::Minimal(minimal)) => {
                // The settled tombstone is the authoritative answer —
                // a second Start never resurrects or respawns.
                return ProcessReply::State {
                    key,
                    state: OperationState::Settled(minimal.snapshot),
                };
            }
            _ => return ProcessReply::Unknown { key },
        };
        match operation.state {
            OperationState::Registered => {}
            state => {
                return ProcessReply::State {
                    key: key.clone(),
                    state,
                };
            }
        }
        // A cancel that already owns the signal settles the record — the
        // tombstone is written once, never resurrected by a later Start.
        let Some(active) = self
            .admit_auxiliary()
            .filter(|_| !operation.cancel.is_cancelled())
        else {
            let snapshot = SettledSnapshot {
                disposition: SettledDisposition::Cancelled,
                physical_settlement_proven: true,
                exit_code: None,
                stdout_total: 0.into(),
                stderr_total: 0.into(),
                stdout_truncated: false,
                stderr_truncated: false,
                final_sequence: 0.into(),
            };
            operation.state = OperationState::Settled(snapshot);
            resolve_pending(operation);
            return ProcessReply::State {
                key: key.clone(),
                state: operation.state,
            };
        };
        // Verify the retained cwd descriptor right before the spawn is
        // admitted; the trusted workspace path it names is what the
        // supervisor chdir's into, and its dev/ino is the business-spawn
        // identity check.
        if operation.cwd.verify().is_err() {
            operation.state = OperationState::Settled(SettledSnapshot {
                disposition: SettledDisposition::Failed,
                physical_settlement_proven: true,
                exit_code: None,
                stdout_total: 0.into(),
                stderr_total: 0.into(),
                stdout_truncated: false,
                stderr_truncated: false,
                final_sequence: 0.into(),
            });
            resolve_pending(operation);
            return ProcessReply::State {
                key: key.clone(),
                state: operation.state,
            };
        }
        operation.state = OperationState::Starting;
        let input_rx = operation.input_rx.take();
        let spec = operation.spec.clone();
        let signal = operation.cancel.clone();
        let revoke = self.revoke.clone();
        let inner_arc = Arc::clone(&self.inner);
        let workspace = self.workspace.clone();
        let control = self.run_control(&key);
        let active = ActiveProcess {
            capture: matches!(&spec.program, ProcessProgram::Shell { .. })
                .then(|| Capture::new(operation.bash_file.clone())),
            worker: active,
        };
        let captures = self.captures.clone();
        let cwd_fd = operation.cwd.descriptor().try_clone();
        let cwd_path = self.workspace.join(&spec.cwd);
        drop(inner);
        #[cfg(test)]
        self.hold_capture(&key);
        // A configured runtime may have loaded before its remote Client
        // connected. An authorized Shell START can request the same readiness
        // work again; it does not wait for it or affect argv-only commands.
        if matches!(&spec.program, ProcessProgram::Shell { .. }) {
            self.schedule_workspace_python();
        }
        let Some(runtime) = self
            .runtime
            .lock()
            .expect("runtime")
            .as_ref()
            .map(|rt| rt.handle().clone())
        else {
            self.settle_failed(&key);
            return ProcessReply::State {
                key: key.clone(),
                state: self.current_state(&key),
            };
        };
        let Ok(cwd_fd) = cwd_fd else {
            self.settle_failed(&key);
            return ProcessReply::State {
                key: key.clone(),
                state: self.current_state(&key),
            };
        };
        let python = Arc::clone(&self.python);
        runtime.spawn(run_operation(
            inner_arc,
            cwd_fd,
            cwd_path,
            workspace,
            key.clone(),
            spec,
            signal,
            revoke,
            input_rx,
            control,
            active,
            captures,
            python,
        ));
        ProcessReply::State {
            key,
            state: OperationState::Starting,
        }
    }

    fn current_state(&self, key: &ProcessOperationKey) -> OperationState {
        let inner = self.inner.lock().expect("operation registry");
        match inner.records.get(&key.operation) {
            Some(Record::Full(op)) => op.state,
            Some(Record::Minimal(m)) => OperationState::Settled(m.snapshot),
            Some(Record::PreparingBash(_)) | None => OperationState::Registered,
        }
    }

    fn settle_failed(&self, key: &ProcessOperationKey) {
        let mut inner = self.inner.lock().expect("operation registry");
        if let Some(Record::Full(op)) = inner.records.get_mut(&key.operation) {
            resolve_pending(op);
            op.state = OperationState::Settled(SettledSnapshot {
                disposition: SettledDisposition::Failed,
                physical_settlement_proven: true,
                exit_code: None,
                stdout_total: op.journal.stdout.total.into(),
                stderr_total: op.journal.stderr.total.into(),
                stdout_truncated: op.journal.stdout.truncated,
                stderr_truncated: op.journal.stderr.truncated,
                final_sequence: op.journal.end.into(),
            });
        }
    }

    fn query(&self, key: ProcessOperationKey) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let inner = self.inner.lock().expect("operation registry");
        match inner.records.get(&key.operation) {
            Some(Record::Full(op)) if op.key == key => ProcessReply::State {
                key,
                state: op.state,
            },
            Some(Record::Minimal(m)) => ProcessReply::State {
                key,
                state: OperationState::Settled(m.snapshot),
            },
            _ => ProcessReply::Unknown { key },
        }
    }

    fn output_page(
        &self,
        key: ProcessOperationKey,
        cursor: DecimalU64,
        max_bytes: u32,
    ) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let inner = self.inner.lock().expect("operation registry");
        let Some(Record::Full(op)) = inner.records.get(&key.operation) else {
            return ProcessReply::Unknown { key };
        };
        if op.key != key {
            return ProcessReply::Unknown { key };
        }
        let cursor = cursor.get();
        if cursor < op.journal.base || cursor > op.journal.end || max_bytes == 0 {
            return ProcessReply::Conflict { key };
        }
        if cursor == op.journal.end {
            return match op.state {
                OperationState::Settled(snapshot) => ProcessReply::OutputEnd {
                    key,
                    final_sequence: snapshot.final_sequence,
                },
                _ => ProcessReply::OutputPending { key },
            };
        }
        let Some(segment) = op.journal.segments.iter().find(|s| s.end > cursor) else {
            return ProcessReply::Conflict { key };
        };
        let within = usize::try_from(cursor - segment.start).unwrap_or(0);
        let available = &segment.bytes[within..];
        let take = available.len().min(usize::try_from(max_bytes).unwrap_or(0));
        let delivered = &available[..take];
        let next = cursor + delivered.len() as u64;
        let stream_offset = segment.stream_offset + within as u64;
        let stats = match segment.stream {
            ProcessStream::Stdout | ProcessStream::Pty => &op.journal.stdout,
            ProcessStream::Stderr => &op.journal.stderr,
        };
        ProcessReply::Output {
            key,
            sequence: DecimalU64::new(next),
            stream: segment.stream,
            offset: DecimalU64::new(stream_offset),
            delta_base64: base64::engine::general_purpose::STANDARD.encode(delivered),
            capped: stats.truncated,
            last: next == op.journal.end && matches!(op.state, OperationState::Settled(_)),
        }
    }

    /// One ordered input control: sequence-gated and admitted into the
    /// bounded input queue — the reply acknowledges *admission* only;
    /// the physical outcome is observed through `InputReceipt`, so the
    /// data lane never waits on a full child pipe.
    /// `Pending` is recorded under the registry hold itself, before the
    /// admission leaves: a racing `InputReceipt` can never outrun the
    /// record.
    fn input_control(
        &self,
        key: ProcessOperationKey,
        sequence: DecimalU64,
        kind: InputKind,
    ) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let fingerprint = kind.fingerprint();
        let mut inner = self.inner.lock().expect("operation registry");
        let Some(record) = inner.records.get_mut(&key.operation) else {
            return ProcessReply::Unknown { key };
        };
        let receipts = match record {
            Record::Full(op) if op.key == key => &op.receipts,
            Record::Minimal(retired) => &retired.receipts,
            Record::Full(_) => return ProcessReply::Conflict { key },
            Record::PreparingBash(_) => return ProcessReply::Unknown { key },
        };
        if let Some(receipt) = receipts.get(&sequence.get()) {
            return if receipt.fingerprint == fingerprint {
                ProcessReply::InputAccepted { key, sequence }
            } else {
                ProcessReply::Conflict { key }
            };
        }
        let Record::Full(op) = record else {
            return ProcessReply::Unknown { key };
        };
        if sequence.get() != op.input_sequence {
            return ProcessReply::Conflict { key };
        }
        if self.revoke.is_cancelled()
            || matches!(op.state, OperationState::Settled(_))
            || op.receipts.len() >= INPUT_QUEUE_DEPTH
        {
            return ProcessReply::Conflict { key };
        }
        let Some(tx) = &op.input else {
            return ProcessReply::Conflict { key };
        };
        if tx
            .try_send(InputEvent {
                kind,
                sequence: sequence.get(),
            })
            .is_err()
        {
            return ProcessReply::Conflict { key };
        }
        op.input_sequence += 1;
        op.receipts.insert(
            sequence.get(),
            RetainedInput {
                fingerprint,
                outcome: InputOutcome::Pending,
            },
        );
        ProcessReply::InputAccepted { key, sequence }
    }

    /// Claim the physical receipt. Pending polling never consumes a slot,
    /// and abandoning a receipt leaves the already-admitted input in order.
    fn input_receipt(
        &self,
        key: ProcessOperationKey,
        sequence: DecimalU64,
        abandon_pending: bool,
    ) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let mut inner = self.inner.lock().expect("operation registry");
        let receipts = match inner.records.get_mut(&key.operation) {
            Some(Record::Full(op)) if op.key == key => &mut op.receipts,
            Some(Record::Minimal(op)) => &mut op.receipts,
            _ => return ProcessReply::Unknown { key },
        };
        let status = receipts
            .get(&sequence.get())
            .map_or(InputOutcome::Unknown, |receipt| receipt.outcome);
        if status != InputOutcome::Pending || abandon_pending {
            receipts.remove(&sequence.get());
        }
        ProcessReply::InputReceipt {
            key,
            sequence,
            status,
        }
    }

    fn write_stdin(
        &self,
        key: ProcessOperationKey,
        sequence: DecimalU64,
        delta_base64: &str,
        close: bool,
    ) -> ProcessReply {
        // The encoding inflates by 4/3: an encoding too long to ever
        // decode under the bound is refused before any allocation.
        if delta_base64.len() > MAX_PROCESS_DATA_BYTES.div_ceil(3) * 4 {
            return ProcessReply::Conflict { key };
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(delta_base64) else {
            return ProcessReply::Conflict { key };
        };
        if (bytes.is_empty() && !close) || bytes.len() > MAX_PROCESS_DATA_BYTES {
            return ProcessReply::Conflict { key };
        }
        self.input_control(key, sequence, InputKind::Write { bytes, close })
    }

    fn close_stdin(&self, key: ProcessOperationKey, sequence: DecimalU64) -> ProcessReply {
        self.input_control(key, sequence, InputKind::Close)
    }

    fn resize(
        &self,
        key: ProcessOperationKey,
        sequence: DecimalU64,
        size: PtySpec,
    ) -> ProcessReply {
        // Only a genuine tty resize may be admitted; a pipe-mode
        // operation gets an honest refusal, not a no-op ack.
        {
            let inner = self.inner.lock().expect("operation registry");
            if let Some(Record::Full(op)) = inner.records.get(&key.operation)
                && op.spec.pty.is_none()
            {
                return ProcessReply::Conflict { key };
            }
        }
        self.input_control(key, sequence, InputKind::Resize(size))
    }

    fn release(&self, key: ProcessOperationKey, final_sequence: DecimalU64) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let mut inner = self.inner.lock().expect("operation registry");
        if let Some(Record::Minimal(minimal)) = inner.records.get(&key.operation) {
            return if final_sequence == minimal.snapshot.final_sequence {
                ProcessReply::OutputReleased { key }
            } else {
                ProcessReply::Conflict { key }
            };
        }
        let Some(Record::Full(op)) = inner.records.get_mut(&key.operation) else {
            return ProcessReply::Unknown { key };
        };
        if op.key != key {
            return ProcessReply::Unknown { key };
        }
        // Release is valid only over the *complete* settled journal —
        // the caller consumed or discarded through the snapshot's final
        // sequence exactly.
        let OperationState::Settled(snapshot) = op.state else {
            return ProcessReply::Conflict { key };
        };
        if final_sequence.get() != op.journal.end || final_sequence != snapshot.final_sequence {
            return ProcessReply::Conflict { key };
        }
        let fingerprint = op.fingerprint;
        let bash_file = registered_capture(op);
        let never_started = op
            .witness
            .as_ref()
            .is_none_or(|witness| !witness.start_crossed());
        let receipts = std::mem::take(&mut op.receipts);
        inner.records.insert(
            key.operation,
            Record::Minimal(Minimal {
                fingerprint: Some(fingerprint),
                snapshot,
                bash_file,
                never_started,
                receipts,
            }),
        );
        ProcessReply::OutputReleased { key }
    }

    fn discard_bash_output(&self, key: ProcessOperationKey) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let Some(_active) = self.admit_auxiliary() else {
            return ProcessReply::Unknown { key };
        };
        let file = {
            let inner = self.inner.lock().expect("operation registry");
            if self.revoke.is_cancelled() {
                return ProcessReply::Unknown { key };
            }
            match inner.records.get(&key.operation) {
                Some(Record::Minimal(minimal)) => {
                    minimal.bash_file.as_ref().map(|(_, file)| file.clone())
                }
                Some(Record::Full(_)) => return ProcessReply::Conflict { key },
                Some(Record::PreparingBash(_)) | None => return ProcessReply::Unknown { key },
            }
        };
        #[cfg(test)]
        self.hold_capture(&key);
        if let Some(file) = file {
            if let Err(error) = file.discard() {
                tracing::warn!(%error, "Bash capture unlink failed");
                return ProcessReply::Conflict { key };
            }
            let mut inner = self.inner.lock().expect("operation registry");
            if let Some(record) = inner.records.get_mut(&key.operation) {
                record.forget_capture(&file);
            }
        }
        ProcessReply::OutputReleased { key }
    }

    #[cfg(test)]
    fn hold_capture(&self, key: &ProcessOperationKey) {
        let hold = self.capture_holds.lock().unwrap().remove(&key.operation);
        if let Some((entered, resume)) = hold {
            entered.send(()).unwrap();
            resume.recv().unwrap();
        }
    }

    fn bash_capture(&self, key: ProcessOperationKey) -> ProcessReply {
        if !self.own_key(&key) {
            return ProcessReply::Unknown { key };
        }
        let Some(_active) = self.admit_auxiliary() else {
            return ProcessReply::Unknown { key };
        };
        let (snapshot, capture, empty_file) = {
            let inner = self.inner.lock().expect("operation registry");
            if self.revoke.is_cancelled() {
                return ProcessReply::Unknown { key };
            }
            match inner.records.get(&key.operation) {
                Some(Record::Full(op))
                    if matches!(op.spec.program, ProcessProgram::Shell { .. }) =>
                {
                    let OperationState::Settled(snapshot) = op.state else {
                        return ProcessReply::Conflict { key };
                    };
                    (snapshot, op.bash_result.clone(), op.bash_file.clone())
                }
                Some(Record::Minimal(retired))
                    if retired.never_started && retired.bash_file.is_some() =>
                {
                    (
                        retired.snapshot,
                        None,
                        retired.bash_file.as_ref().map(|(_, file)| file.clone()),
                    )
                }
                _ => return ProcessReply::Unknown { key },
            }
        };
        let capture = capture.or_else(|| {
            (snapshot.disposition == SettledDisposition::Cancelled
                && snapshot.final_sequence.get() == 0
                && snapshot.stdout_total.get() == 0
                && snapshot.stderr_total.get() == 0
                && empty_file.is_some_and(|file| file.proves_empty()))
            .then(|| BashCaptureResult {
                stdout: String::new(),
                stderr: String::new(),
                combined: String::new(),
                truncated: false,
                total: Some(0.into()),
                output: rustx_execution_protocol::process::BashCaptureState::Complete,
                started: false,
                signal: None,
            })
        });
        ProcessReply::BashCapture { key, capture }
    }

    pub(crate) fn with_bash_file<T>(
        &self,
        key: &ProcessOperationKey,
        owner: &str,
        read: impl FnOnce(&CaptureFile) -> T,
    ) -> Option<T> {
        let _active = self.admit_auxiliary()?;
        self.bash_file(key, owner).map(|file| read(&file))
    }

    fn bash_file(&self, key: &ProcessOperationKey, owner: &str) -> Option<Arc<CaptureFile>> {
        if !self.own_key(key) {
            return None;
        }
        let inner = self.inner.lock().expect("operation registry");
        if self.revoke.is_cancelled() {
            return None;
        }
        let (conversation, file) = match inner.records.get(&key.operation)? {
            Record::PreparingBash(_) => return None,
            Record::Full(op) => {
                let ProcessProgram::Shell { conversation, .. } = &op.spec.program else {
                    return None;
                };
                (conversation, op.bash_file.as_ref()?)
            }
            Record::Minimal(minimal) => {
                let (conversation, file) = minimal.bash_file.as_ref()?;
                (conversation, file)
            }
        };
        (conversation == owner).then(|| file.clone())
    }

    /// Fences every operation: the generation authority cancels all
    /// pending START grants and each invocation's own signal cancels —
    /// workers then perform and prove real containment.
    pub(crate) fn revoke(&self) {
        self.revoke.cancel();
        let inner = self.inner.lock().expect("operation registry");
        for record in inner.records.values() {
            if let Record::Full(op) = record {
                op.cancel.cancel();
            }
        }
    }

    /// Await physical worker settlement within `budget`. Returns live
    /// workers plus those that ended without confirming physical settlement.
    /// A failed or dropped waiter cannot turn an unproven unit into evidence.
    pub(crate) fn settle(&self, budget: Duration) -> usize {
        let mut state = self.workers.state.lock().expect("worker count");
        let deadline = std::time::Instant::now() + budget;
        while state.live != 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            state = self
                .workers
                .drained
                .wait_timeout(state, remaining)
                .expect("worker count")
                .0;
        }
        state.live + state.unconfirmed
    }

    /// After revocation, each idle capture can be reclaimed even if an
    /// unrelated process has no settlement proof. A worker or reader
    /// retaining a reference still owns its capture.
    fn release_bash_captures(&self) -> io::Result<()> {
        let mut failures = Vec::new();
        let owned = {
            let inner = self.inner.lock().expect("operation registry");
            inner
                .records
                .iter()
                .filter_map(|(id, record)| {
                    let file = record.capture_file()?;
                    if Arc::strong_count(file) == 1 {
                        Some((*id, file.clone()))
                    } else {
                        failures.push(format!("Bash capture {id:?} still has a reader or writer"));
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        for (id, capture) in owned {
            match capture.discard() {
                Ok(()) => {
                    let mut inner = self.inner.lock().expect("operation registry");
                    if let Some(record) = inner.records.get_mut(&id) {
                        record.forget_capture(&capture);
                    }
                }
                Err(error) => failures.push(format!("Bash capture {id:?}: {error}")),
            }
        }
        // An already admitted allocation might still be opening a file
        // outside the registry. It can attach after this sweep, so keep
        // the directory until the live owners are finished.
        let workers_idle = self.workers.state.lock().expect("worker count").live == 0;
        if workers_idle && let Err(error) = self.captures.close() {
            failures.push(format!("Bash capture directory: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join(". ")))
        }
    }

    pub(crate) fn finish_revocation(&self, budget: Duration) -> io::Result<()> {
        self.finish_settlement(self.settle(budget))
    }

    fn finish_settlement(&self, unresolved: usize) -> io::Result<()> {
        let cleanup = self.release_bash_captures();
        if unresolved == 0 {
            return cleanup;
        }
        let mut detail = format!("{unresolved} process workers unresolved");
        if let Err(error) = cleanup {
            use std::fmt::Write as _;
            let _ = write!(detail, ". {error}");
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, detail))
    }

    /// Signal every worker and await physical completion — including
    /// registered-never-started records — then stop the runtime with a
    /// bounded drain. A worker still live when the drain budget lapses
    /// is reported honestly: the caller exits *unresolved*, never with
    /// a forged clean shutdown.
    pub(crate) fn shutdown(&self) -> io::Result<()> {
        self.revoke();
        let unresolved = self.settle(Duration::from_secs(30));
        if let Some(runtime) = self.runtime.lock().expect("runtime").take() {
            runtime.shutdown_timeout(Duration::from_secs(10));
        }
        self.finish_settlement(unresolved)
    }
}

impl Drop for Operations {
    fn drop(&mut self) {
        // The teardown cannot propagate — the explicit `shutdown` call
        // is where unresolved workers become a visible failure.
        let _ = self.shutdown();
    }
}

pub(crate) struct WorkerGuard {
    workers: Arc<WorkerCount>,
    unconfirmed: bool,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let mut state = self.workers.state.lock().expect("worker count");
        state.live -= 1;
        state.unconfirmed += usize::from(self.unconfirmed);
        self.workers.drained.notify_all();
    }
}

/// Runtime-owned Git keys an override can never supply — the same
/// pattern the product baseline owns.
fn is_runtime_owned_git_key(key: &str) -> bool {
    matches!(
        key,
        "GIT_CONFIG_PARAMETERS" | "GIT_CONFIG_COUNT" | "GIT_PAGER" | "GIT_EDITOR"
    ) || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
}

/// The composition of the command environment: the fixed product
/// baseline (PATH, HOME = workspace, UTF-8 locale) plus the spec's
/// authorized non-baseline overrides and the deterministic Git
/// execution guards — never a Server host value.
fn command_environment(workspace: &Path, spec: &ProcessSpec) -> Vec<(String, String)> {
    const GIT_GUARDS: &[(&str, &str)] = &[
        ("GIT_CONFIG_COUNT", "5"),
        ("GIT_CONFIG_KEY_0", "core.useBuiltinFSMonitor"),
        ("GIT_CONFIG_VALUE_0", "false"),
        ("GIT_CONFIG_KEY_1", "diff.orderFile"),
        ("GIT_CONFIG_VALUE_1", "/dev/null"),
        ("GIT_CONFIG_KEY_2", "core.editor"),
        ("GIT_CONFIG_VALUE_2", "true"),
        ("GIT_CONFIG_KEY_3", "core.pager"),
        ("GIT_CONFIG_VALUE_3", "cat"),
        ("GIT_CONFIG_KEY_4", "core.fsmonitor"),
        ("GIT_CONFIG_VALUE_4", "false"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_CONFIG_PARAMETERS", ""),
    ];
    let mut entries = vec![
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), workspace.display().to_string()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
    ];
    let is_shell = matches!(spec.program, ProcessProgram::Shell { .. });
    for (key, value) in &spec.env {
        if is_shell && matches!(key.as_str(), "BASH_ENV" | "ENV" | "SHELLOPTS" | "BASHOPTS") {
            continue;
        }
        if is_shell
            && (key.starts_with("BASH_FUNC_")
                || key.starts_with("RUSTX_TEST_")
                || key.starts_with("RUSTX_SUPERVISOR_")
                || key.starts_with("RUSTX_INTERACTIVE_")
                || matches!(
                    key.as_str(),
                    "RUSTX_COMMAND_ENVIRONMENT_FILE" | "RUSTX_PHYSICAL_CONTINUATION"
                ))
        {
            continue;
        }
        if let Some(value) = value {
            if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
                if is_shell {
                    existing.clone_from(value);
                }
            } else {
                entries.push((key.clone(), value.clone()));
            }
        }
    }
    entries.retain(|(key, _)| !is_runtime_owned_git_key(key));
    for (key, value) in GIT_GUARDS {
        entries.push(((*key).to_owned(), (*value).to_owned()));
    }
    entries
}

/// One partially-delivered stdin/PTY write — the single in-flight input
/// the worker advances nonblockingly.
struct PendingInput {
    bytes: Vec<u8>,
    offset: usize,
    close: bool,
    sequence: u64,
}

#[cfg(test)]
struct PartialInputFailure {
    offset: Option<std::sync::mpsc::Sender<usize>>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// The physical worker of one operation. The outer supervisor spawn is
/// not business START: `Running` publishes only on the runner's own
/// witness of the crossed grant, `Exited` only on `ShellExited`, and the
/// settled snapshot only after real drain + reap.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one coherent physical worker
async fn run_operation(
    inner: Arc<Mutex<Inner>>,
    cwd_fd: std::os::unix::io::OwnedFd,
    cwd_path: PathBuf,
    workspace: PathBuf,
    key: ProcessOperationKey,
    spec: ProcessSpec,
    signal: ProcessSignal,
    revoke: ProcessSignal,
    mut input_rx: Option<tokio::sync::mpsc::Receiver<InputEvent>>,
    control: Option<rustx_process_supervisor::process_runner::RunnerTestControl>,
    mut active: ActiveProcess,
    captures: Arc<Captures>,
    python: Arc<rustx_execution_client::workspace_python::WorkspacePython>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // Verify the descended directory one final time immediately before
    // the irreversible boundary: dev/ino must still match what Register
    // retained.
    let Ok(identity) = nix::sys::stat::fstat(&cwd_fd).map(|s| (s.st_dev, s.st_ino)) else {
        settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
        return;
    };
    let Ok(temporary) = tempfile::tempdir() else {
        settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
        return;
    };
    let input_socket = temporary.path().join("stdin");
    let mut terminal = match spec.pty {
        Some(size) => {
            let Ok(terminal) = Terminal::open(size) else {
                settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
                return;
            };
            Some(terminal)
        }
        None => None,
    };
    let listener = if spec.pty.is_none() && spec.stdin == ProcessStdin::Streamed {
        let Ok(listener) = tokio::net::UnixListener::bind(&input_socket) else {
            settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
            return;
        };
        Some(listener)
    } else {
        None
    };
    let (command, argv) = match &spec.program {
        ProcessProgram::Argv(argv) => (argv.join(" "), Some(argv.clone())),
        ProcessProgram::Shell { command, .. } => (command.clone(), None),
    };
    let mut environment = command_environment(&workspace, &spec);
    if matches!(&spec.program, ProcessProgram::Shell { .. })
        && let Err(error) = python.apply(&mut environment)
    {
        tracing::warn!(%error, "Workspace Python guard unavailable");
        settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
        return;
    }
    let command_spec = SupervisedCommandSpec {
        command,
        cwd: cwd_path,
        command_environment: environment,
        timeout: (spec.timeout_ms != 0).then(|| Duration::from_millis(spec.timeout_ms)),
        cancellation: signal.clone(),
    };
    let invocation = ProgramInvocation {
        argv,
        stdin_socket: listener.as_ref().map(|_| input_socket.clone()),
        terminal: terminal.as_ref().map(|terminal| terminal.path.clone()),
        cwd_identity: identity,
    };
    // Pre-START cancellation under the same grant decides: a revoked or
    // already-cancelled signal admits zero spawn.
    let spawn_blocked = signal.with_active(|| false).is_none() || revoke.is_cancelled();
    if spawn_blocked {
        settle_outcome(&inner, &key, SettledDisposition::Cancelled, None, true);
        return;
    }
    let spawned = SupervisedCommandRunner::spawn_program(&command_spec, control, Some(&invocation));
    let Ok((mut runner, mut stdout, mut stderr)) = spawned else {
        settle_outcome(&inner, &key, SettledDisposition::Failed, None, true);
        return;
    };
    active.worker.unconfirmed = true;
    // The generation authority is an independent family — it never
    // shares the invocation signal's grant mutex.
    runner.guard_start(revoke);
    let witness = runner.witness();
    {
        let mut guard = inner.lock().expect("operation registry");
        if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation) {
            op.witness = Some(witness.clone());
        }
    }
    let mut input_stream: Option<tokio::net::UnixStream> = None;
    let mut input_closed = input_rx.is_none() && terminal.is_none();
    let mut pending_input: Option<PendingInput> = None;
    #[cfg(test)]
    let mut partial_input_failure = match inner.lock().unwrap().records.get_mut(&key.operation) {
        Some(Record::Full(op)) => op.partial_input_failure.take(),
        _ => None,
    };
    let mut stdout_buf = vec![0u8; 16 * 1024];
    let mut stderr_buf = vec![0u8; 16 * 1024];
    let mut terminal_buf = vec![0u8; 16 * 1024];
    let mut stdout_done = stdout.is_none();
    let mut stderr_done = stderr.is_none();
    let mut terminal_done = terminal.is_none();
    // A read error is physical loss, never EOF: the outcome must be
    // honestly Failed/Unknown rather than a clean completion.
    let mut output_failed = false;
    let workers = active.worker.workers.clone();
    let allocate_capture = || {
        let inner = inner.clone();
        let captures = captures.clone();
        let id = key.operation;
        let owner = workers.retain_work();
        tokio::task::spawn_blocking(move || {
            // The blocking task retains physical ownership even if its
            // async waiter is dropped. Register the file before returning
            // it to the writer or handing it to binding teardown.
            let _owner = owner;
            let file = captures.create(id)?;
            let mut guard = inner.lock().expect("operation registry");
            let Some(Record::Full(operation)) = guard.records.get_mut(&id) else {
                unreachable!(
                    "an operation cannot be retired while its output worker is allocating"
                );
            };
            operation.bash_file = Some(file.clone());
            Ok(file)
        })
    };
    let mut running_published = false;
    let mut exit_published = false;
    let settlement = tokio::spawn(async move { runner.settle().await });
    tokio::pin!(settlement);
    let mut termination = None;
    loop {
        if termination.is_some()
            && stdout_done
            && stderr_done
            && terminal_done
            && pending_input.is_none()
        {
            break;
        }
        // Publish lifecycle only from the physical witness — never from
        // the outer helper's spawn or an assumption.
        if !running_published && witness.start_crossed() {
            running_published = true;
            let mut guard = inner.lock().expect("operation registry");
            if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation)
                && matches!(op.state, OperationState::Starting)
            {
                op.state = OperationState::Running;
            }
        }
        if !exit_published && witness.exited() {
            exit_published = true;
            let mut guard = inner.lock().expect("operation registry");
            if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation)
                && matches!(op.state, OperationState::Running | OperationState::Starting)
            {
                op.state = OperationState::Exited {
                    exit_code: witness.exit_code(),
                };
            }
        }
        tokio::select! {
            result = &mut settlement, if termination.is_none() => {
                termination = result.ok();
                let Some(proof) = &termination else {
                    // A failed join is no settlement — the outcome stays
                    // honestly Unknown rather than a fabricated one.
                    settle_outcome(&inner, &key, SettledDisposition::Unknown, None, false);
                    return;
                };
                active.worker.unconfirmed = !proof.physical_settlement_proven;
                if let Some(term) = &mut terminal { term.slave.take(); }
                input_stream.take();
                input_closed = true;
                if let Some(pending) = pending_input.take() {
                    record_input(&inner, &key, pending.sequence, InputOutcome::Failed);
                }
                // Orphaned queued controls are answered false — the
                // physical operation can never apply them now.
                if let Some(rx) = &mut input_rx {
                    while let Ok(event) = rx.try_recv() {
                        record_input(&inner, &key, event.sequence, InputOutcome::Failed);
                    }
                }
            }
            result = async { stdout.as_mut().expect("stdout").read(&mut stdout_buf).await }, if !stdout_done => {
                match result {
                    Ok(0) => {
                        stdout_done = true;
                        if let Some(capture) = &mut active.capture {
                            capture.push(ProcessStream::Stdout, None, allocate_capture).await;
                        }
                    }
                    Ok(n) => {
                        let bytes = &stdout_buf[..n];
                        if let Some(capture) = &mut active.capture {
                            capture.push(ProcessStream::Stdout, Some(bytes), allocate_capture).await;
                        }
                        push(&inner, &key, ProcessStream::Stdout, bytes);
                    }
                    Err(_) => {
                        stdout_done = true;
                        output_failed = true;
                    }
                }
            }
            result = async { stderr.as_mut().expect("stderr").read(&mut stderr_buf).await }, if !stderr_done => {
                match result {
                    Ok(0) => {
                        stderr_done = true;
                        if let Some(capture) = &mut active.capture {
                            capture.push(ProcessStream::Stderr, None, allocate_capture).await;
                        }
                    }
                    Err(_) => {
                        stderr_done = true;
                        output_failed = true;
                    }
                    Ok(n) => {
                        let bytes = &stderr_buf[..n];
                        if let Some(capture) = &mut active.capture {
                            capture.push(ProcessStream::Stderr, Some(bytes), allocate_capture).await;
                        }
                        push(&inner, &key, ProcessStream::Stderr, bytes);
                    }
                }
            }
            result = async { terminal.as_ref().expect("terminal").read(&mut terminal_buf).await }, if !terminal_done => {
                #[cfg(test)]
                let result = if std::mem::take(&mut inner.lock().unwrap().fail_pty_read) {
                    Err(std::io::ErrorKind::PermissionDenied.into())
                } else {
                    result
                };
                match result {
                    Ok(0) => terminal_done = true,
                    Err(_) => {
                        terminal_done = true;
                        output_failed = true;
                    }
                    Ok(n) => push(&inner, &key, ProcessStream::Pty, &terminal_buf[..n]),
                }
            }
            accepted = async { listener.as_ref().expect("stdin listener").accept().await }, if listener.is_some() && input_stream.is_none() && !input_closed && termination.is_none() => {
                input_stream = accepted.ok().map(|(stream, _)| stream);
            }
            written = async {
                let pending = pending_input.as_ref().expect("pending input");
                let bytes = &pending.bytes[pending.offset..];
                #[cfg(test)]
                let bytes = if let Some(fault) = &mut partial_input_failure {
                    if pending.offset > 0 {
                        if let Some(reached) = fault.offset.take() {
                            let _ = reached.send(pending.offset);
                        }
                        let _ = (&mut fault.release).await;
                        return Err(io::ErrorKind::BrokenPipe.into());
                    }
                    &bytes[..bytes.len().min(1)]
                } else {
                    bytes
                };
                if let Some(term) = &terminal {
                    term.write(bytes).await
                } else {
                    let stream = input_stream.as_mut().expect("stdin");
                    if bytes.is_empty() {
                        stream.shutdown().await.map(|()| 0)
                    } else {
                        stream.write(bytes).await
                    }
                }
            }, if pending_input.is_some() && termination.is_none() && (terminal.is_some() || input_stream.is_some()) => {
                let pending = pending_input.as_mut().expect("pending input");
                match written {
                    Ok(count) if count > 0 || pending.bytes.is_empty() => {
                        pending.offset += count;
                        if pending.offset == pending.bytes.len() {
                            let pending = pending_input.take().expect("finished input");
                            if pending.close {
                                input_closed = true;
                                input_stream.take();
                            }
                            record_input(&inner, &key, pending.sequence, InputOutcome::Applied);
                        }
                    }
                    _ => {
                        let pending = pending_input.take().expect("failed input");
                        record_input(&inner, &key, pending.sequence, InputOutcome::Failed);
                        input_closed = true;
                        input_stream.take();
                    }
                }
            }
            event = async { input_rx.as_mut().expect("input").recv().await }, if input_rx.is_some() && pending_input.is_none() && termination.is_none() => {
                match event {
                    Some(InputEvent { kind: InputKind::Write { bytes, close }, sequence }) => {
                        if input_closed {
                            record_input(&inner, &key, sequence, InputOutcome::Rejected);
                            continue;
                        }
                        // Atomic write+close: an impossible EOF vetoes the
                        // whole batch — no byte is written.
                        let mut batch = bytes;
                        let mut veto = false;
                        if close && let Some(term) = &terminal {
                            match term.eof() {
                                Ok(eof) => batch.extend_from_slice(&eof),
                                Err(_) => veto = true,
                            }
                        }
                        if veto {
                            record_input(&inner, &key, sequence, InputOutcome::Rejected);
                        } else {
                            pending_input = Some(PendingInput { bytes: batch, offset: 0, close, sequence });
                        }
                    }
                    Some(InputEvent { kind: InputKind::Close, sequence }) => {
                        if input_closed {
                            record_input(&inner, &key, sequence, InputOutcome::Rejected);
                        } else if let Some(term) = &terminal {
                            match term.eof() {
                                Ok(eof) => {
                                    pending_input = Some(PendingInput { bytes: eof.to_vec(), offset: 0, close: true, sequence });
                                }
                                Err(_) => {
                                    record_input(&inner, &key, sequence, InputOutcome::Rejected);
                                }
                            }
                        } else {
                            // Keep accepting until the supervisor connects its real
                            // stdin. An accepted close is not yet a physical EOF.
                            pending_input = Some(PendingInput {
                                bytes: Vec::new(), offset: 0, close: true, sequence
                            });
                            #[cfg(test)]
                            if let Some(Record::Full(op)) = inner.lock().unwrap().records.get_mut(&key.operation)
                                && let Some(received) = op.pipe_close_received.take()
                            {
                                let _ = received.send(());
                            }
                        }
                    }
                    Some(InputEvent { kind: InputKind::Resize(size), sequence }) => {
                        let outcome = if terminal.as_ref().is_some_and(|term| term.resize(size).is_ok()) {
                            InputOutcome::Applied
                        } else {
                            InputOutcome::Failed
                        };
                        record_input(&inner, &key, sequence, outcome);
                    }
                    None => {
                        input_rx = None;
                    }
                }
            }
        }
        // AsyncFd readiness can stay immediately ready across small PTY
        // fragments — cooperate so the input channel and settlement arm
        // keep progressing.
        tokio::task::yield_now().await;
    }
    let Some(termination) = termination else {
        settle_outcome(&inner, &key, SettledDisposition::Unknown, None, false);
        return;
    };
    if let Some(capture) = active.capture.take() {
        use std::os::unix::process::ExitStatusExt as _;
        let signal = termination.exit_status.and_then(|status| status.signal());
        let (result, complete) = capture.finish(!output_failed, witness.start_crossed(), signal);
        output_failed |= !complete;
        let mut guard = inner.lock().expect("operation registry");
        if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation) {
            op.bash_result = Some(result);
        }
    }
    let exit_code = termination.exit_status.and_then(|status| status.code());
    let disposition = if termination.physical_settlement_proven {
        match &termination.intent {
            ProcessOutcomeIntent::Completed if output_failed => SettledDisposition::Failed,
            ProcessOutcomeIntent::Completed => SettledDisposition::Known,
            ProcessOutcomeIntent::TimedOut => SettledDisposition::TimedOut,
            ProcessOutcomeIntent::Cancelled => SettledDisposition::Cancelled,
            ProcessOutcomeIntent::ProcessControlFailed(_)
                if matches!(spec.program, ProcessProgram::Shell { .. })
                    && witness.start_crossed() =>
            {
                SettledDisposition::Unknown
            }
            ProcessOutcomeIntent::ProcessControlFailed(_) => SettledDisposition::Failed,
        }
    } else {
        SettledDisposition::Unknown
    };
    settle_outcome(
        &inner,
        &key,
        disposition,
        exit_code,
        termination.physical_settlement_proven,
    );
}

/// Records one admitted input's physical outcome — the only writer of
/// `Applied`/`Failed` after admission's `Pending`.
fn record_input(
    inner: &Mutex<Inner>,
    key: &ProcessOperationKey,
    sequence: u64,
    outcome: InputOutcome,
) {
    let mut guard = inner.lock().expect("operation registry");
    if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation)
        && let Some(slot) = op.receipts.get_mut(&sequence)
        && slot.outcome == InputOutcome::Pending
    {
        slot.outcome = outcome;
    }
}

impl Operations {
    /// The physical-boundary control for `key`. Production has no test
    /// control; a test arms one START hold for exactly one operation.
    #[cfg(not(test))]
    #[allow(clippy::unused_self)] // production never carries a test control
    fn run_control(
        &self,
        _key: &ProcessOperationKey,
    ) -> Option<rustx_process_supervisor::process_runner::RunnerTestControl> {
        None
    }

    #[cfg(test)]
    fn run_control(
        &self,
        key: &ProcessOperationKey,
    ) -> Option<rustx_process_supervisor::process_runner::RunnerTestControl> {
        self.runner_controls
            .lock()
            .expect("process runner controls")
            .remove(&key.operation)
    }
}

/// Arms one START hold for exactly `key`; no other operation can take it.
#[cfg(test)]
fn arm_start_hold(
    ops: &Operations,
    key: &ProcessOperationKey,
) -> rustx_process_supervisor::process_runner::RunnerStartHold {
    let hold = rustx_process_supervisor::process_runner::RunnerStartHold::new();
    let mut control = rustx_process_supervisor::process_runner::RunnerTestControl::new();
    control.start_hold = Some(hold.clone());
    ops.runner_controls
        .lock()
        .expect("process runner controls")
        .insert(key.operation, control);
    hold
}

fn push(inner: &Mutex<Inner>, key: &ProcessOperationKey, stream: ProcessStream, bytes: &[u8]) {
    let mut guard = inner.lock().expect("operation registry");
    if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation) {
        let cap = op.spec.output_cap;
        if op.spec.capture
            && (!matches!(op.spec.program, ProcessProgram::Shell { .. })
                || stream == ProcessStream::Stdout)
        {
            op.journal.push(stream, bytes, cap);
        } else {
            let stats = op.journal.stream(stream);
            stats.total += bytes.len() as u64;
            stats.truncated |= !bytes.is_empty();
        }
    }
}

/// A settled operation can never physically apply a queued control —
/// every `Pending` receipt resolves to `Failed`, never a guess.
fn resolve_pending(operation: &mut Operation) {
    for receipt in operation.receipts.values_mut() {
        if receipt.outcome == InputOutcome::Pending {
            receipt.outcome = InputOutcome::Failed;
        }
    }
}

fn settle_outcome(
    inner: &Mutex<Inner>,
    key: &ProcessOperationKey,
    disposition: SettledDisposition,
    exit_code: Option<i32>,
    physical_settlement_proven: bool,
) {
    let mut guard = inner.lock().expect("operation registry");
    if let Some(Record::Full(op)) = guard.records.get_mut(&key.operation) {
        // The first settle wins: a later failure path never rewrites a
        // proven outcome.
        if matches!(op.state, OperationState::Settled(_)) {
            return;
        }
        resolve_pending(op);
        op.state = OperationState::Settled(SettledSnapshot {
            disposition,
            physical_settlement_proven,
            exit_code: exit_code.filter(|_| disposition != SettledDisposition::Unknown),
            stdout_total: op.journal.stdout.total.into(),
            stderr_total: op.journal.stderr.total.into(),
            stdout_truncated: op.journal.stdout.truncated,
            stderr_truncated: op.journal.stderr.truncated,
            final_sequence: op.journal.end.into(),
        });
    }
}

fn settled_cancel_disposition(snapshot: SettledSnapshot) -> ProcessCancelDisposition {
    if snapshot.physical_settlement_proven {
        ProcessCancelDisposition::AlreadySettled
    } else {
        ProcessCancelDisposition::Unknown
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use rustx_execution_protocol::process::{BashCaptureState, MAX_BASH_CAPTURES};
    use rustx_execution_protocol::{
        Admission, ClientInstanceId, EnvironmentId, Generation, PROTOCOL_VERSION, SessionId,
    };

    use super::*;

    /// Whether the physical `bash-supervisor` sibling is staged beside
    /// this test binary — the tests exercise real spawn, so they require
    /// the prepared staging.
    fn supervisor_staged() -> bool {
        rustx_process_supervisor::invocation::supervisor_binary().is_file()
    }

    fn admission(workspace: &std::path::Path) -> Admission {
        Admission {
            version: PROTOCOL_VERSION,
            server_incarnation: rustx_execution_protocol::process::ServerIncarnation::generate(),
            session: SessionId::new("session-1"),
            environment: EnvironmentId::new("env-1"),
            instance: ClientInstanceId::new("client-1"),
            generation: Generation::try_from(1u64).unwrap(),
            workspace: workspace.to_path_buf(),
        }
    }

    fn fresh_key(admission: &Admission) -> ProcessOperationKey {
        ProcessOperationKey {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            operation: OperationId::new(),
        }
    }

    fn spec(argv: &[&str]) -> ProcessSpec {
        ProcessSpec {
            cwd: PathBuf::new(),
            program: ProcessProgram::Argv(argv.iter().map(|a| (*a).to_owned()).collect()),
            env: BTreeMap::new(),
            timeout_ms: 30_000,
            pty: None,
            stdin: ProcessStdin::Closed,
            capture: true,
            output_cap: 64 * 1024,
        }
    }

    fn register(
        ops: &Arc<Operations>,
        key: &ProcessOperationKey,
        spec: &ProcessSpec,
    ) -> ProcessReply {
        ops.dispatch(ProcessRequest::Register {
            key: key.clone(),
            spec: spec.clone(),
            fingerprint: spec.fingerprint(),
        })
    }

    /// Poll Query until the record settles — bounded, never busy-spins
    /// forever.
    fn await_settled(ops: &Arc<Operations>, key: &ProcessOperationKey) -> SettledSnapshot {
        for _ in 0..400 {
            match ops.dispatch(ProcessRequest::Query { key: key.clone() }) {
                ProcessReply::State {
                    state: OperationState::Settled(snapshot),
                    ..
                } => return snapshot,
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        panic!("operation never settled");
    }

    /// Drain the journal from cursor 0; returns (concatenated stdout/pty
    /// bytes, final cursor).
    fn drain(ops: &Arc<Operations>, key: &ProcessOperationKey) -> (Vec<u8>, u64) {
        let mut out = Vec::new();
        let mut cursor = 0u64;
        for _ in 0..200 {
            match ops.dispatch(ProcessRequest::OutputPage {
                key: key.clone(),
                cursor: DecimalU64::new(cursor),
                max_bytes: 8192,
            }) {
                ProcessReply::Output {
                    sequence,
                    delta_base64,
                    ..
                } => {
                    out.extend_from_slice(
                        &base64::engine::general_purpose::STANDARD
                            .decode(&delta_base64)
                            .unwrap(),
                    );
                    cursor = sequence.get();
                }
                ProcessReply::OutputEnd { final_sequence, .. } => {
                    return (out, final_sequence.get());
                }
                ProcessReply::OutputPending { .. } => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => panic!("unexpected page reply: {other:?}"),
            }
        }
        panic!("journal never ended");
    }

    #[test]
    fn a_foreign_key_is_refused_before_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let mut foreign = fresh_key(&adm);
        foreign.instance = ClientInstanceId::new("other-client");
        let spec = spec(&["/bin/pwd"]);
        assert!(matches!(
            register(&ops, &foreign, &spec),
            ProcessReply::Unknown { .. }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::Start {
                key: foreign.clone()
            }),
            ProcessReply::Unknown { .. }
        ));
        foreign.instance = adm.instance.clone();
        foreign.generation = Generation::try_from(2u64).unwrap();
        assert!(matches!(
            register(&ops, &foreign, &spec),
            ProcessReply::Unknown { .. }
        ));
        assert_eq!(ops.cancel(&foreign), ProcessCancelDisposition::Unknown);
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn argv_pwd_runs_in_the_workspace_and_settles_known() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut spec = spec(&["/bin/pwd"]);
        spec.cwd = PathBuf::from("sub");
        assert!(matches!(
            register(&ops, &key, &spec),
            ProcessReply::Registered { .. }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::Start { key: key.clone() }),
            ProcessReply::State {
                state: OperationState::Starting,
                ..
            }
        ));
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Known);
        assert_eq!(snapshot.exit_code, Some(0));
        let (out, _) = drain(&ops, &key);
        let text = String::from_utf8_lossy(&out);
        assert!(text.trim_end().ends_with("/sub"), "pwd output: {text}");
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn duplicate_register_attaches_and_second_start_cannot_spawn_twice() {
        assert_one_concurrent_start(false);
        assert_one_concurrent_start(true);
    }

    fn assert_one_concurrent_start(background_bash: bool) {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let marker = dir.path().join("marker");
        let command = "printf x >> marker; printf done";
        let mut specification = spec(&["/bin/sh", "-c", command]);
        if background_bash {
            specification.program = ProcessProgram::Shell {
                command: command.into(),
                conversation: "concurrent-start".into(),
                live_output: true,
            };
        }
        let prepare = |spec: &ProcessSpec| {
            ops.register(
                key.clone(),
                spec.clone(),
                spec.fingerprint(),
                background_bash,
            )
        };
        let (original_at_dispatch, waiting) = std::sync::mpsc::channel();
        let (continue_original, parked) = std::sync::mpsc::channel();
        let hold = arm_start_hold(&ops, &key);
        std::thread::scope(|scope| {
            let original = scope.spawn({
                let prepare = &prepare;
                let specification = &specification;
                move || {
                    original_at_dispatch.send(()).unwrap();
                    parked.recv_timeout(Duration::from_secs(5)).unwrap();
                    prepare(specification)
                }
            });
            waiting.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(
                ops.query(key.clone()),
                ProcessReply::Unknown { key: key.clone() }
            );
            assert_eq!(
                prepare(&specification),
                ProcessReply::Registered { key: key.clone() }
            );
            let capture = ops.bash_file(&key, "concurrent-start");
            assert_eq!(capture.is_some(), background_bash);
            dispatch_duplicate_starts(&ops, &key);
            continue_original.send(()).unwrap();
            assert_eq!(
                original.join().unwrap(),
                ProcessReply::Registered { key: key.clone() }
            );
            if let Some(capture) = capture {
                assert!(Arc::ptr_eq(
                    &capture,
                    &ops.bash_file(&key, "concurrent-start").unwrap()
                ));
            }
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(hold.await_entered());
        assert!(!marker.exists());
        let mut other = specification.clone();
        other.timeout_ms = 31_000;
        assert_eq!(prepare(&other), ProcessReply::Conflict { key: key.clone() });
        hold.release();
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Known);
        assert_eq!(snapshot.exit_code, Some(0));
        assert_eq!(
            prepare(&specification),
            ProcessReply::Registered { key: key.clone() }
        );
        ops.shutdown().unwrap();
        assert_eq!(std::fs::read(marker).unwrap(), b"x");
    }

    fn dispatch_duplicate_starts(ops: &Arc<Operations>, key: &ProcessOperationKey) {
        let contenders = std::sync::Barrier::new(3);
        let start = || {
            contenders.wait();
            ops.dispatch(ProcessRequest::Start { key: key.clone() })
        };
        std::thread::scope(|scope| {
            let first = scope.spawn(start);
            let second = scope.spawn(start);
            contenders.wait();
            for reply in [first.join().unwrap(), second.join().unwrap()] {
                assert_eq!(
                    reply,
                    ProcessReply::State {
                        key: key.clone(),
                        state: OperationState::Starting,
                    }
                );
            }
        });
    }

    #[test]
    fn replay_uses_the_original_registration_after_the_cwd_disappears() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut specification = spec(&["/bin/true"]);
        specification.cwd = PathBuf::from("nested");
        let registered = ProcessReply::Registered { key: key.clone() };
        assert_eq!(register(&ops, &key, &specification), registered);
        std::fs::remove_dir(nested).unwrap();
        assert_eq!(register(&ops, &key, &specification), registered);
        let first = ops.dispatch(ProcessRequest::Start { key: key.clone() });
        assert!(matches!(
            first,
            ProcessReply::State {
                state: OperationState::Settled(SettledSnapshot {
                    disposition: SettledDisposition::Failed,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(register(&ops, &key, &specification), registered);
        assert_eq!(ops.dispatch(ProcessRequest::Start { key }), first);
        ops.shutdown().unwrap();
    }

    #[test]
    fn cancelled_before_register_uses_one_reserved_identity_at_capacity() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let active = fresh_key(&admission);
        let specification = spec(&["/bin/true"]);
        assert_eq!(
            register(&ops, &active, &specification),
            ProcessReply::Registered {
                key: active.clone()
            }
        );
        let cancelled = fresh_key(&admission);
        assert_eq!(
            ops.cancel(&cancelled),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        for _ in 2..MAX_PROCESS_IDENTITIES_PER_GENERATION {
            assert_eq!(
                ops.cancel(&fresh_key(&admission)),
                ProcessCancelDisposition::CancelledBeforeStart
            );
        }
        let refused = fresh_key(&admission);
        assert_eq!(ops.cancel(&refused), ProcessCancelDisposition::Unknown);
        assert_eq!(
            register(&ops, &refused, &specification),
            ProcessReply::Refused {
                key: refused,
                reason: SpecRefusal::TombstonesSaturated,
            }
        );
        assert_eq!(
            ops.cancel(&active),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        let terminal = ops.dispatch(ProcessRequest::Start {
            key: cancelled.clone(),
        });
        assert!(matches!(
            terminal,
            ProcessReply::State {
                state: OperationState::Settled(SettledSnapshot {
                    disposition: SettledDisposition::Cancelled,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(register(&ops, &cancelled, &specification), terminal);
        assert_eq!(
            register(&ops, &cancelled, &spec(&["/bin/false"])),
            ProcessReply::Conflict { key: cancelled }
        );
        assert_eq!(
            ops.inner.lock().unwrap().records.len(),
            MAX_PROCESS_IDENTITIES_PER_GENERATION
        );
        ops.shutdown().unwrap();
    }

    #[test]
    fn dropping_a_worker_without_physical_proof_keeps_revoke_unconfirmed() {
        let directory = tempfile::tempdir().unwrap();
        let ops = Operations::new(admission(directory.path())).unwrap();
        let mut worker = ops.admit_auxiliary().unwrap();
        worker.unconfirmed = true;
        ops.revoke();
        assert_eq!(ops.settle(Duration::ZERO), 1);
        drop(worker);
        assert_eq!(ops.settle(Duration::ZERO), 1);
        assert_eq!(ops.shutdown().unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn real_supervisor_wait_failure_keeps_the_unknown_proof_after_output_release() {
        assert!(
            supervisor_staged(),
            "run test-support/prepare_native_tests.py"
        );
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let retained = register_bash_capture(&ops, &admission, "printf retained", true);
        ops.dispatch(ProcessRequest::Start {
            key: retained.clone(),
        });
        let output = await_settled(&ops, &retained);
        assert_eq!(output.disposition, SettledDisposition::Known);
        let captures = ops.captures.path().unwrap();
        let file = capture_path(&ops, &retained);
        assert_eq!(std::fs::read(&file).unwrap(), b"retained");
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: retained.clone(),
                final_sequence: output.final_sequence,
            }),
            ProcessReply::OutputReleased { key: retained }
        );
        let key = fresh_key(&admission);
        assert_eq!(
            register(&ops, &key, &spec(&["/bin/sh", "-c", "printf done; exit 0"])),
            ProcessReply::Registered { key: key.clone() }
        );
        let mut control = rustx_process_supervisor::process_runner::RunnerTestControl::new();
        control.fail_direct_wait_after_exit = true;
        ops.runner_controls
            .lock()
            .unwrap()
            .insert(key.operation, control);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Unknown);
        assert_eq!(snapshot.exit_code, None);
        assert!(!snapshot.physical_settlement_proven);
        let (output, cursor) = drain(&ops, &key);
        assert_eq!(output, b"done");
        assert_eq!(ops.cancel(&key), ProcessCancelDisposition::Unknown);
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: cursor.into()
            }),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert_eq!(
            ops.query(key.clone()),
            ProcessReply::State {
                key,
                state: OperationState::Settled(snapshot)
            }
        );
        ops.revoke();
        assert_eq!(ops.settle(Duration::from_secs(5)), 1);
        assert_eq!(ops.shutdown().unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(!file.exists());
        assert!(!captures.exists());
        assert_eq!(ops.settle(Duration::ZERO), 1);
    }

    fn register_bash_capture(
        ops: &Arc<Operations>,
        admission: &Admission,
        command: &str,
        live_output: bool,
    ) -> ProcessOperationKey {
        let key = fresh_key(admission);
        assert_eq!(
            ops.dispatch(bash_capture_registration(key.clone(), command, live_output)),
            ProcessReply::Registered { key: key.clone() }
        );
        key
    }

    fn bash_capture_registration(
        key: ProcessOperationKey,
        command: &str,
        live_output: bool,
    ) -> ProcessRequest {
        let mut bash = spec(&[]);
        bash.program = ProcessProgram::Shell {
            command: command.into(),
            conversation: "capture-fixture".into(),
            live_output,
        };
        ProcessRequest::RegisterAgentBash {
            key,
            fingerprint: bash.fingerprint(),
            spec: bash,
        }
    }

    fn capture_path(ops: &Operations, key: &ProcessOperationKey) -> std::path::PathBuf {
        ops.captures
            .path()
            .unwrap()
            .join(format!("{:032x}.output", key.operation.as_u128()))
    }

    fn fail_capture_unlink(ops: &Arc<Operations>, key: &ProcessOperationKey) {
        ops.with_bash_file(key, "capture-fixture", |file| {
            file.fail_next_unlink
                .store(true, std::sync::atomic::Ordering::Release);
        })
        .unwrap();
    }

    #[test]
    fn revocation_reclaims_other_captures_after_an_unlink_failure() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let first = register_bash_capture(&ops, &admission, ":", true);
        let second = register_bash_capture(&ops, &admission, ":", true);
        // The sweep orders records by operation ID, not registration time.
        let (failed, other) = if first.operation < second.operation {
            (first, second)
        } else {
            (second, first)
        };
        fail_capture_unlink(&ops, &failed);
        let failed = capture_path(&ops, &failed);
        let other = capture_path(&ops, &other);
        let captures = ops.captures.path().unwrap();
        ops.revoke();
        assert!(ops.finish_revocation(Duration::ZERO).is_err());
        assert!(failed.is_file());
        assert!(!other.exists());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        ops.shutdown().unwrap();
        assert!(!captures.exists());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
    }

    #[test]
    fn unreadable_foreground_capture_keeps_its_owner_until_discard_succeeds() {
        assert!(supervisor_staged());
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = register_bash_capture(&ops, &admission, "printf '%65536s' ''", false);
        ops.captures.fail_append_and_rollback_for(key.operation);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        assert!(snapshot.physical_settlement_proven);
        assert_eq!(snapshot.disposition, SettledDisposition::Failed);
        assert!(matches!(
            ops.dispatch(ProcessRequest::BashCapture { key: key.clone() }),
            ProcessReply::BashCapture {
                capture: Some(BashCaptureResult {
                    output: BashCaptureState::Unavailable,
                    started: true,
                    ..
                }),
                ..
            }
        ));
        let path = capture_path(&ops, &key);
        assert_eq!(std::fs::read(&path).unwrap(), b" ");
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: snapshot.final_sequence,
            }),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        fail_capture_unlink(&ops, &key);
        let discard = || ProcessRequest::DiscardBashOutput { key: key.clone() };
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::Conflict { key: key.clone() }
        );
        assert!(path.is_file());
        assert!(ops.bash_file(&key, "capture-fixture").is_some());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert!(!path.exists());
        assert!(ops.bash_file(&key, "capture-fixture").is_none());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        ops.shutdown().unwrap();
    }

    #[test]
    fn discard_retains_quota_until_the_last_capture_link_is_gone() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = register_bash_capture(&ops, &admission, ":", true);
        let path = capture_path(&ops, &key);
        let alias = path.with_extension("link");
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        std::fs::hard_link(&path, &alias).unwrap();
        let discard = || ProcessRequest::DiscardBashOutput { key: key.clone() };
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::Conflict { key: key.clone() }
        );
        assert!(!path.exists());
        assert!(alias.is_file());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        std::fs::remove_file(alias).unwrap();
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::OutputReleased { key }
        );
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        ops.shutdown().unwrap();
    }

    #[test]
    fn discard_proves_a_missing_capture_is_unlinked_and_preserves_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = register_bash_capture(&ops, &admission, ":", true);
        let path = capture_path(&ops, &key);
        let moved = path.with_extension("moved");
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        let discard = || ProcessRequest::DiscardBashOutput { key: key.clone() };
        std::fs::rename(&path, &moved).unwrap();
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::Conflict { key: key.clone() }
        );
        assert!(moved.is_file());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::Conflict { key: key.clone() }
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert!(moved.is_file());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        std::fs::rename(&moved, &path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        assert_eq!(
            ops.dispatch(discard()),
            ProcessReply::OutputReleased { key }
        );
        ops.shutdown().unwrap();
    }

    #[test]
    fn discard_reclaims_unlinked_capture_with_a_replacement_at_its_name() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = register_bash_capture(&ops, &admission, ":", true);
        let path = capture_path(&ops, &key);
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();

        for _ in 0..2 {
            assert_eq!(
                ops.dispatch(ProcessRequest::DiscardBashOutput { key: key.clone() }),
                ProcessReply::OutputReleased { key: key.clone() },
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
            assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        }
        std::fs::remove_file(path).unwrap();
        ops.shutdown().unwrap();
    }

    #[test]
    fn cancelled_background_prepare_explicitly_discards_its_capture_without_reviving() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(&directory.path().canonicalize().unwrap());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut specification = spec(&["/bin/true"]);
        specification.program = ProcessProgram::Shell {
            command: "touch revived".into(),
            conversation: "cancel-prepare".into(),
            live_output: true,
        };
        let prepare = ProcessRequest::RegisterAgentBash {
            key: key.clone(),
            fingerprint: specification.fingerprint(),
            spec: specification,
        };
        assert_eq!(
            ops.dispatch(prepare.clone()),
            ProcessReply::Registered { key: key.clone() }
        );
        let captures = ops.captures.path().unwrap();
        assert_eq!(std::fs::read_dir(&captures).unwrap().count(), 1);
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        assert_eq!(std::fs::read_dir(&captures).unwrap().count(), 1);
        let cancelled = ops.dispatch(prepare);
        assert!(matches!(
            cancelled,
            ProcessReply::State {
                state: OperationState::Settled(SettledSnapshot {
                    disposition: SettledDisposition::Cancelled,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(
            ops.dispatch(ProcessRequest::Start { key: key.clone() }),
            cancelled
        );
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: 0.into()
            }),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert_eq!(
            ops.dispatch(ProcessRequest::DiscardBashOutput { key: key.clone() }),
            ProcessReply::OutputReleased { key }
        );
        assert_eq!(std::fs::read_dir(captures).unwrap().count(), 0);
        let full_capture_quota = (0..rustx_execution_protocol::process::MAX_BASH_CAPTURES)
            .map(|_| ops.captures.create(OperationId::new()).unwrap())
            .collect::<Vec<_>>();
        for file in full_capture_quota {
            file.discard().unwrap();
        }
        ops.shutdown().unwrap();
        assert!(!directory.path().join("revived").exists());
    }

    #[test]
    fn failed_bash_capture_unlink_retains_its_owner_and_quota_for_retry() {
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(&directory.path().canonicalize().unwrap());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut bash = spec(&[]);
        bash.program = ProcessProgram::Shell {
            command: "touch unexpected-start".into(),
            conversation: "unlink-retry".into(),
            live_output: true,
        };
        assert_eq!(
            ops.dispatch(ProcessRequest::RegisterAgentBash {
                key: key.clone(),
                fingerprint: bash.fingerprint(),
                spec: bash,
            }),
            ProcessReply::Registered { key: key.clone() }
        );
        let path = ops
            .captures
            .path()
            .unwrap()
            .join(format!("{:032x}.output", key.operation.as_u128()));
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        ops.with_bash_file(&key, "unlink-retry", |file| {
            file.fail_next_unlink
                .store(true, std::sync::atomic::Ordering::Release);
        })
        .unwrap();
        let request = || ProcessRequest::DiscardBashOutput { key: key.clone() };
        assert_eq!(
            ops.dispatch(request()),
            ProcessReply::Conflict { key: key.clone() }
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        assert_eq!(
            ops.with_bash_file(&key, "unlink-retry", CaptureFile::proves_empty),
            Some(true)
        );
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);

        assert_eq!(
            ops.dispatch(request()),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert!(!path.exists());
        assert!(ops.bash_file(&key, "unlink-retry").is_none());
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        assert_eq!(
            ops.dispatch(request()),
            ProcessReply::OutputReleased { key }
        );
        assert!(ops.shutdown().is_ok());
        assert!(!directory.path().join("unexpected-start").exists());
    }

    #[test]
    fn same_input_attaches_until_the_receipt_is_claimed() {
        assert!(
            supervisor_staged(),
            "run test-support/prepare_native_tests.py"
        );
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut specification = spec(&["/bin/cat"]);
        specification.stdin = ProcessStdin::Streamed;
        assert_eq!(
            register(&ops, &key, &specification),
            ProcessReply::Registered { key: key.clone() }
        );
        let first = DecimalU64::new(1);
        let request = |bytes: &[u8], close| ProcessRequest::WriteStdin {
            key: key.clone(),
            sequence: first,
            delta_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            close,
        };
        let accepted = ProcessReply::InputAccepted {
            key: key.clone(),
            sequence: first,
        };
        for _ in 0..2 {
            assert_eq!(ops.dispatch(request(b"once", false)), accepted);
        }
        for conflicting in [request(b"different", false), request(b"once", true)] {
            assert_eq!(
                ops.dispatch(conflicting),
                ProcessReply::Conflict { key: key.clone() }
            );
        }
        let close = DecimalU64::new(2);
        assert_eq!(
            ops.dispatch(ProcessRequest::CloseStdin {
                key: key.clone(),
                sequence: close
            }),
            ProcessReply::InputAccepted {
                key: key.clone(),
                sequence: close
            }
        );
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        assert_eq!(await_settled(&ops, &key).exit_code, Some(0));
        let (stdout, cursor) = drain(&ops, &key);
        assert_eq!(stdout, b"once");
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: cursor.into()
            }),
            ProcessReply::OutputReleased { key: key.clone() }
        );
        assert_eq!(ops.dispatch(request(b"once", false)), accepted);
        assert_eq!(
            ops.dispatch(ProcessRequest::InputReceipt {
                key: key.clone(),
                sequence: first,
                abandon_pending: false
            }),
            ProcessReply::InputReceipt {
                key: key.clone(),
                sequence: first,
                status: InputOutcome::Applied
            }
        );
        assert_eq!(
            ops.dispatch(request(b"once", false)),
            ProcessReply::Unknown { key }
        );
        ops.shutdown().unwrap();
    }

    #[test]
    fn cancel_before_start_admits_zero_business_spawn() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let spec = spec(&["/bin/sh", "-c", "touch spawned"]);
        register(&ops, &key, &spec);
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        let reply = ops.dispatch(ProcessRequest::Start { key: key.clone() });
        assert!(matches!(
            reply,
            ProcessReply::State {
                state: OperationState::Settled(_),
                ..
            }
        ));
        std::thread::sleep(Duration::from_millis(200));
        assert!(!dir.path().join("spawned").exists());
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn agent_bash_has_separate_registration_and_a_client_owned_start_gate() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(&dir.path().canonicalize().unwrap());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut bash = spec(&["/bin/true"]);
        bash.program = ProcessProgram::Shell {
            command: "printf x >> shell-start-count; exec sleep 600".into(),
            conversation: "conv-start-gate".into(),
            live_output: true,
        };
        bash.timeout_ms = 0;
        let marker = dir.path().join("shell-start-count");
        assert_eq!(
            register(&ops, &key, &bash),
            ProcessReply::Refused {
                key: key.clone(),
                reason: SpecRefusal::ProgramNotAuthorized
            },
        );
        assert!(!marker.exists());
        let register = ProcessRequest::RegisterAgentBash {
            key: key.clone(),
            spec: bash.clone(),
            fingerprint: bash.fingerprint(),
        };
        for _ in 0..2 {
            assert_eq!(
                ops.dispatch(register.clone()),
                ProcessReply::Registered { key: key.clone() }
            );
        }
        let mut conflicting = bash.clone();
        let ProcessProgram::Shell { conversation, .. } = &mut conflicting.program else {
            unreachable!();
        };
        *conversation = "another-conversation".into();
        assert_eq!(
            ops.dispatch(ProcessRequest::RegisterAgentBash {
                key: key.clone(),
                fingerprint: conflicting.fingerprint(),
                spec: conflicting,
            }),
            ProcessReply::Conflict { key: key.clone() },
        );
        assert!(ops.bash_file(&key, "conv-start-gate").is_some());
        assert!(ops.bash_file(&key, "another-conversation").is_none());
        let hold = arm_start_hold(&ops, &key);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(hold.await_entered());
        assert!(!marker.exists());
        assert_eq!(
            ops.cancel(&key),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        hold.release();
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Cancelled);
        assert_eq!(snapshot.stdout_total.get(), 0);
        assert_eq!(snapshot.stderr_total.get(), 0);
        assert_eq!(snapshot.final_sequence.get(), 0);
        assert!(matches!(
            ops.dispatch(ProcessRequest::BashCapture { key: key.clone() }),
            ProcessReply::BashCapture {
                capture: Some(BashCaptureResult { started: false, .. }),
                ..
            }
        ));
        assert_eq!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: snapshot.final_sequence,
            }),
            ProcessReply::OutputReleased { key: key.clone() },
        );
        assert!(ops.bash_file(&key, "conv-start-gate").is_some());
        assert_eq!(
            ops.dispatch(ProcessRequest::DiscardBashOutput { key: key.clone() }),
            ProcessReply::OutputReleased { key: key.clone() },
        );
        assert!(ops.bash_file(&key, "conv-start-gate").is_none());
        assert_eq!(
            ops.dispatch(ProcessRequest::Start { key: key.clone() }),
            ProcessReply::State {
                key,
                state: OperationState::Settled(snapshot)
            },
        );
        ops.shutdown().unwrap();
        assert!(!marker.exists(), "no business START was executed");
    }

    /// The barrier-gated race at the physical START boundary: the hold
    /// parks the runner inside its own START transaction — cancellation
    /// delivered while held must be classified by the actual crossing
    /// flag, never assumed. Cancelled-before-crossing writes no marker.
    #[test]
    fn held_start_cancelled_before_crossing_writes_no_marker() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let marker = dir.path().join("business-marker");
        register(
            &ops,
            &key,
            &spec(&["/bin/sh", "-c", "touch business-marker; exec sleep 30"]),
        );
        let hold = arm_start_hold(&ops, &key);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        // Parked inside the physical START transaction — the grant is
        // entered but the crossing has not been published.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(hold.await_entered());
        assert!(!marker.exists());
        // Revoke cancels with the independent family — the runner's own
        // grant evaluates the crossing atomically.
        ops.revoke();
        hold.release();
        let snapshot = await_settled(&ops, &key);
        assert_eq!(
            snapshot.disposition,
            SettledDisposition::Cancelled,
            "cancel racing a held START is classified by the real crossing: {snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists(), "no business spawn after a held cancel");
        ops.shutdown().ok();
    }

    #[test]
    fn a_selected_start_hold_is_owned_by_exactly_one_operation() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let held = fresh_key(&adm);
        let other = fresh_key(&adm);
        register(
            &ops,
            &held,
            &spec(&["/bin/sh", "-c", "touch held-marker; exec sleep 30"]),
        );
        register(
            &ops,
            &other,
            &spec(&["/bin/sh", "-c", "touch other-marker"]),
        );
        let start_hold = arm_start_hold(&ops, &held);
        // The other operation starts first: it must not receive the hold and
        // runs to its own settlement.
        ops.dispatch(ProcessRequest::Start { key: other.clone() });
        let other_snapshot = await_settled(&ops, &other);
        assert_eq!(other_snapshot.disposition, SettledDisposition::Known);
        assert!(dir.path().join("other-marker").exists());
        // The held operation parks inside its physical START transaction.
        ops.dispatch(ProcessRequest::Start { key: held.clone() });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(start_hold.await_entered());
        assert!(!dir.path().join("held-marker").exists());
        // Cancelled while parked before the crossing: zero business spawn.
        assert_eq!(
            ops.cancel(&held),
            ProcessCancelDisposition::CancelledBeforeStart
        );
        start_hold.release();
        let held_snapshot = await_settled(&ops, &held);
        assert_eq!(held_snapshot.disposition, SettledDisposition::Cancelled);
        assert!(!dir.path().join("held-marker").exists());
        ops.shutdown().ok();
    }

    #[test]
    fn empty_positional_argument_reaches_real_argv_unchanged() {
        assert!(
            supervisor_staged(),
            "run test-support/prepare_native_tests.py"
        );
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        // Different output for empty versus absent: printf would
        // default a missing %s argument, so the shell checks argc first.
        let spec = spec(&[
            "/bin/bash",
            "-c",
            "test \"$#\" = 1 && printf '<%s>' \"$1\"",
            "bash",
            "",
        ]);
        register(&ops, &key, &spec);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.exit_code, Some(0));
        let (stdout, _) = drain(&ops, &key);
        assert_eq!(stdout, b"<>");
        assert_eq!(snapshot.stderr_total.get(), 0);
        ops.shutdown().unwrap();
    }

    #[test]
    fn partial_physical_stdin_write_records_failed() {
        assert!(
            supervisor_staged(),
            "run test-support/prepare_native_tests.py"
        );
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut specification = spec(&["/bin/cat"]);
        specification.stdin = ProcessStdin::Streamed;
        register(&ops, &key, &specification);

        let (reached, written) = std::sync::mpsc::channel();
        let (resume, release) = tokio::sync::oneshot::channel();
        {
            let mut inner = ops.inner.lock().unwrap();
            let Some(Record::Full(op)) = inner.records.get_mut(&key.operation) else {
                panic!("registered input owner");
            };
            op.partial_input_failure = Some(PartialInputFailure {
                offset: Some(reached),
                release,
            });
        }
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let sequence = DecimalU64::new(1);
        assert_eq!(
            ops.dispatch(ProcessRequest::WriteStdin {
                key: key.clone(),
                sequence,
                delta_base64: base64::engine::general_purpose::STANDARD.encode(b"prefix"),
                close: false,
            }),
            ProcessReply::InputAccepted {
                key: key.clone(),
                sequence
            }
        );
        assert_eq!(written.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        let receipt = || {
            ops.dispatch(ProcessRequest::InputReceipt {
                key: key.clone(),
                sequence,
                abandon_pending: false,
            })
        };
        assert!(matches!(
            receipt(),
            ProcessReply::InputReceipt {
                status: InputOutcome::Pending,
                ..
            }
        ));
        resume.send(()).unwrap();
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.exit_code, Some(0));
        let (stdout, _) = drain(&ops, &key);
        assert_eq!(stdout, b"p");
        for status in [InputOutcome::Failed, InputOutcome::Unknown] {
            assert_eq!(
                receipt(),
                ProcessReply::InputReceipt {
                    key: key.clone(),
                    sequence,
                    status
                }
            );
        }
        ops.shutdown().unwrap();
    }

    #[test]
    fn pipe_close_receipt_survives_capacity_pressure_before_physical_eof() {
        assert!(
            supervisor_staged(),
            "run test-support/prepare_native_tests.py"
        );
        let directory = tempfile::tempdir().unwrap();
        let admission = admission(directory.path());
        let ops = Arc::new(Operations::new(admission.clone()).unwrap());
        let key = fresh_key(&admission);
        let mut spec = spec(&[
            "/bin/bash",
            "-c",
            "if ! IFS= read -r line; then printf eof; fi",
        ]);
        spec.stdin = ProcessStdin::Streamed;
        register(&ops, &key, &spec);
        let (seen_tx, seen) = std::sync::mpsc::channel();
        {
            let mut inner = ops.inner.lock().unwrap();
            let Some(Record::Full(record)) = inner.records.get_mut(&key.operation) else {
                panic!("registered input owner");
            };
            record.pipe_close_received = Some(seen_tx);
        }
        let hold = arm_start_hold(&ops, &key);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(hold.await_entered());
        let close = |sequence| {
            ops.dispatch(ProcessRequest::CloseStdin {
                key: key.clone(),
                sequence: DecimalU64::new(sequence),
            })
        };
        let accept = |sequence| {
            assert_eq!(
                close(sequence),
                ProcessReply::InputAccepted {
                    key: key.clone(),
                    sequence: sequence.into()
                }
            );
        };
        accept(1);
        seen.recv_timeout(Duration::from_secs(5)).unwrap();
        // The worker has removed the first item from the channel, but has
        // not applied it. The receipt bound must refuse the ninth input
        // while the channel still has room.
        let bound = u64::try_from(INPUT_QUEUE_DEPTH).unwrap();
        for next in 2..=bound {
            accept(next);
        }
        assert_eq!(
            close(bound + 1),
            ProcessReply::Conflict { key: key.clone() }
        );
        let receipt = || {
            ops.dispatch(ProcessRequest::InputReceipt {
                key: key.clone(),
                sequence: 1.into(),
                abandon_pending: false,
            })
        };
        assert!(matches!(
            receipt(),
            ProcessReply::InputReceipt {
                status: InputOutcome::Pending,
                ..
            }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::InputReceipt {
                key: key.clone(),
                sequence: 2.into(),
                abandon_pending: true,
            }),
            ProcessReply::InputReceipt {
                status: InputOutcome::Pending,
                ..
            }
        ));
        accept(bound + 1);
        hold.release();
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.exit_code, Some(0));
        for expected in [InputOutcome::Applied, InputOutcome::Unknown] {
            assert!(
                matches!(receipt(), ProcessReply::InputReceipt { status, .. } if status == expected)
            );
        }
        let (stdout, _) = drain(&ops, &key);
        assert_eq!(stdout, b"eof");
        ops.shutdown().unwrap();
    }

    #[test]
    fn streamed_stdin_reaches_the_process_in_order() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut spec = spec(&["/bin/cat"]);
        spec.stdin = ProcessStdin::Streamed;
        register(&ops, &key, &spec);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        // Wait for Running before input so the listener exists.
        for _ in 0..200 {
            if let ProcessReply::State {
                state: OperationState::Running,
                ..
            } = ops.dispatch(ProcessRequest::Query { key: key.clone() })
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let payload = base64::engine::general_purpose::STANDARD.encode(b"hello-stdin\n");
        assert!(matches!(
            ops.dispatch(ProcessRequest::WriteStdin {
                key: key.clone(),
                sequence: DecimalU64::new(1),
                delta_base64: payload.clone(),
                close: false,
            }),
            ProcessReply::InputAccepted { .. }
        ));
        // Out-of-order sequence is a conflict, not a reorder.
        assert!(matches!(
            ops.dispatch(ProcessRequest::WriteStdin {
                key: key.clone(),
                sequence: DecimalU64::new(3),
                delta_base64: payload,
                close: false,
            }),
            ProcessReply::Conflict { .. }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::CloseStdin {
                key: key.clone(),
                sequence: DecimalU64::new(2),
            }),
            ProcessReply::InputAccepted { .. }
        ));
        // The physical close receipt resolves through the query, not
        // the admission reply.
        for _ in 0..200 {
            match ops.dispatch(ProcessRequest::InputReceipt {
                key: key.clone(),
                sequence: DecimalU64::new(2),
                abandon_pending: false,
            }) {
                ProcessReply::InputReceipt {
                    status: InputOutcome::Applied,
                    ..
                } => break,
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Known);
        let (out, _) = drain(&ops, &key);
        assert!(out.windows(11).any(|w| w == b"hello-stdin"), "got {out:?}");
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn pty_output_resize_and_eof_work() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut spec = spec(&["/bin/cat"]);
        spec.pty = Some(PtySpec { rows: 24, cols: 80 });
        spec.stdin = ProcessStdin::Streamed;
        register(&ops, &key, &spec);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        for _ in 0..200 {
            if let ProcessReply::State {
                state: OperationState::Running,
                ..
            } = ops.dispatch(ProcessRequest::Query { key: key.clone() })
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(matches!(
            ops.dispatch(ProcessRequest::Resize {
                key: key.clone(),
                sequence: DecimalU64::new(1),
                size: PtySpec {
                    rows: 40,
                    cols: 120
                },
            }),
            ProcessReply::InputAccepted { .. }
        ));
        let payload = base64::engine::general_purpose::STANDARD.encode(b"pty-input\r");
        assert!(matches!(
            ops.dispatch(ProcessRequest::WriteStdin {
                key: key.clone(),
                sequence: DecimalU64::new(2),
                delta_base64: payload,
                close: false,
            }),
            ProcessReply::InputAccepted { .. }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::CloseStdin {
                key: key.clone(),
                sequence: DecimalU64::new(3),
            }),
            ProcessReply::InputAccepted { .. }
        ));
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Known);
        let (out, _) = drain(&ops, &key);
        assert!(out.windows(9).any(|w| w == b"pty-input"), "got {out:?}");
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn a_non_eio_pty_read_failure_does_not_settle_as_known_output() {
        assert!(
            supervisor_staged(),
            "stage bash-supervisor for the PTY test"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut spec = spec(&["/bin/echo", "terminal output"]);
        spec.pty = Some(PtySpec { rows: 24, cols: 80 });
        spec.stdin = ProcessStdin::Streamed;
        ops.inner.lock().unwrap().fail_pty_read = true;
        assert!(matches!(
            register(&ops, &key, &spec),
            ProcessReply::Registered { .. }
        ));
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.exit_code, Some(0));
        assert_eq!(snapshot.disposition, SettledDisposition::Failed);
        assert!(!ops.inner.lock().unwrap().fail_pty_read);
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn four_concurrent_slots_refuse_a_fifth_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let spec = spec(&["/bin/pwd"]);
        let keys: Vec<_> = (0..4).map(|_| fresh_key(&adm)).collect();
        for key in &keys {
            assert!(matches!(
                register(&ops, key, &spec),
                ProcessReply::Registered { .. }
            ));
        }
        let fifth = fresh_key(&adm);
        assert!(matches!(
            register(&ops, &fifth, &spec),
            ProcessReply::Refused {
                reason: SpecRefusal::CapacityExhausted,
                ..
            }
        ));
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn forged_or_early_release_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let spec = spec(&["/bin/pwd"]);
        register(&ops, &key, &spec);
        // Not settled.
        assert!(matches!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: DecimalU64::new(0),
            }),
            ProcessReply::Conflict { .. }
        ));
        // Wrong operation.
        let other = fresh_key(&adm);
        assert!(matches!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: other,
                final_sequence: DecimalU64::new(0),
            }),
            ProcessReply::Unknown { .. }
        ));
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn release_requires_the_exact_final_cursor() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let spec = spec(&["/bin/echo", "done"]);
        register(&ops, &key, &spec);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        // A sequence that is not the exact final boundary is rejected.
        assert!(matches!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: DecimalU64::new(snapshot.final_sequence.get() + 1),
            }),
            ProcessReply::Conflict { .. }
        ));
        assert!(matches!(
            ops.dispatch(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: snapshot.final_sequence,
            }),
            ProcessReply::OutputReleased { .. }
        ));
        // The minimal tombstone still answers Query with the real snapshot.
        assert!(matches!(
            ops.dispatch(ProcessRequest::Query { key: key.clone() }),
            ProcessReply::State {
                state: OperationState::Settled(_),
                ..
            }
        ));
        // And a same-fingerprint replay returns that snapshot.
        assert!(matches!(
            register(&ops, &key, &spec),
            ProcessReply::State {
                state: OperationState::Settled(_),
                ..
            }
        ));
        // A different fingerprint is a conflict — no resurrection.
        let mut other = spec.clone();
        other.env.insert("X".into(), Some("1".into()));
        assert!(matches!(
            ops.dispatch(ProcessRequest::Register {
                key: key.clone(),
                fingerprint: other.fingerprint(),
                spec: other,
            }),
            ProcessReply::Conflict { .. }
        ));
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn authorized_env_reaches_the_command_and_baseline_wins() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut spec = spec(&["/bin/sh", "-c", "echo M:$MARKER P:$OVERRIDE_PATH"]);
        spec.env = BTreeMap::from([
            ("MARKER".into(), Some("hello-env".into())),
            // PATH is a fixed baseline — an override cannot replace it.
            ("OVERRIDE_PATH".into(), Some("x".into())),
        ]);
        register(&ops, &key, &spec);
        ops.dispatch(ProcessRequest::Start { key: key.clone() });
        let snapshot = await_settled(&ops, &key);
        assert_eq!(snapshot.disposition, SettledDisposition::Known);
        let (out, _) = drain(&ops, &key);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("M:hello-env"), "env output: {text}");
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    fn sequential_released_operations_do_not_exhaust_capacity() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let spec = spec(&["/bin/true"]);
        for _ in 0..5 {
            let key = fresh_key(&adm);
            assert!(matches!(
                register(&ops, &key, &spec),
                ProcessReply::Registered { .. }
            ));
            ops.dispatch(ProcessRequest::Start { key: key.clone() });
            let snapshot = await_settled(&ops, &key);
            assert!(matches!(
                ops.dispatch(ProcessRequest::ReleaseOutput {
                    key: key.clone(),
                    final_sequence: snapshot.final_sequence,
                }),
                ProcessReply::OutputReleased { .. }
            ));
        }
        assert!(ops.shutdown().is_ok());
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Keep the real control receipts and allocation barrier together.
    fn background_capture_preparation_keeps_control_independent_and_settles_its_owner() {
        use rustx_execution_protocol::transport::{read_record, write_record};
        use rustx_execution_protocol::{ControlPoll, ControlReply, ControlRequest, ControlTurn};
        use rustx_execution_transport::RecordChannel;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc::channel;

        fn send_turn(server: &mut UnixStream, poll: &ControlPoll, request: ControlRequest) {
            write_record(
                server,
                &ControlTurn {
                    sequence: poll.sequence,
                    health: poll.health,
                    request: Some(request),
                },
            )
            .unwrap();
        }

        enum Outcome {
            Accepted,
            Cancelled,
            Revoked,
        }
        for outcome in [Outcome::Accepted, Outcome::Cancelled, Outcome::Revoked] {
            let workspace = tempfile::tempdir().unwrap();
            let admission = admission(workspace.path());
            let ops = Arc::new(Operations::new(admission.clone()).unwrap());
            let key = fresh_key(&admission);
            let request = bash_capture_registration(key.clone(), "touch unexpected-start", true);
            let ProcessRequest::RegisterAgentBash { fingerprint, .. } = request else {
                unreachable!()
            };
            let other = fresh_key(&admission);
            assert_eq!(
                register(&ops, &other, &spec(&["/bin/true"])),
                ProcessReply::Registered { key: other.clone() }
            );

            let (entered, paused) = channel();
            let (resume, resume_rx) = channel();
            ops.captures
                .hold_allocation(key.operation, entered, resume_rx);
            let registration = std::thread::spawn({
                let (ops, request) = (ops.clone(), request.clone());
                move || ops.dispatch(request)
            });
            paused.recv_timeout(Duration::from_secs(5)).unwrap();

            let (client, mut server) = UnixStream::pair().unwrap();
            let (data, _server_data) = UnixStream::pair().unwrap();
            let worker = crate::client_control_worker(
                RecordChannel::unix(client),
                Arc::new(crate::mutation::Arbitration::new()),
                RecordChannel::unix(data).shutdown_handle().unwrap(),
                ops.clone(),
                Arc::new(crate::watch::Watches::new()),
            );
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut poll: ControlPoll = read_record(&mut server).unwrap();
            // Keep the wire exchange and receipt adjacent while allocation is parked.
            send_turn(
                &mut server,
                &poll,
                ControlRequest::CancelProcess { key: other.clone() },
            );
            poll = read_record(&mut server).unwrap();
            assert_eq!(
                poll.receipt,
                Some(ControlReply::ProcessCancelled {
                    key: other,
                    disposition: ProcessCancelDisposition::CancelledBeforeStart,
                })
            );
            send_turn(
                &mut server,
                &poll,
                ControlRequest::Health { nonce: 7.into() },
            );
            poll = read_record(&mut server).unwrap();
            assert_eq!(
                poll.receipt,
                Some(ControlReply::Healthy { nonce: 7.into() })
            );

            let matching = ops
                .inner
                .lock()
                .unwrap()
                .registration_reply(&key, fingerprint)
                .unwrap();
            assert!(matches!(matching, RegistrationReply::Preparing(_)));
            assert_eq!(
                ops.dispatch(bash_capture_registration(key.clone(), ":", true)),
                ProcessReply::Conflict { key: key.clone() }
            );
            assert_eq!(
                ops.dispatch(ProcessRequest::Start { key: key.clone() }),
                ProcessReply::Unknown { key: key.clone() }
            );
            assert!(ops.captures.path().is_none());
            assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);

            let request_on_control = match outcome {
                Outcome::Accepted => None,
                Outcome::Cancelled => Some(ControlRequest::CancelProcess { key: key.clone() }),
                Outcome::Revoked => Some(ControlRequest::Revoke),
            };
            if let Some(request) = request_on_control {
                send_turn(&mut server, &poll, request);
            }
            match outcome {
                Outcome::Accepted => {}
                Outcome::Cancelled => {
                    let receipt: ControlPoll = read_record(&mut server).unwrap();
                    assert_eq!(
                        receipt.receipt,
                        Some(ControlReply::ProcessCancelled {
                            key: key.clone(),
                            disposition: ProcessCancelDisposition::CancelledBeforeStart,
                        })
                    );
                }
                Outcome::Revoked => {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_time()
                        .build()
                        .unwrap()
                        .block_on(async {
                            tokio::time::timeout(Duration::from_secs(5), ops.revoke.cancelled())
                                .await
                                .unwrap();
                        });
                    assert_eq!(ops.settle(Duration::ZERO), 1);
                }
            }
            resume.send(()).unwrap();
            let reply = registration.join().unwrap();
            assert_eq!(matching.wait(), reply);
            assert_eq!(ops.dispatch(request), reply);
            match outcome {
                Outcome::Accepted => {
                    assert_eq!(reply, ProcessReply::Registered { key: key.clone() });
                    let path = capture_path(&ops, &key);
                    assert_eq!(std::fs::read(&path).unwrap(), b"");
                    assert_eq!(
                        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
                        1
                    );
                    assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
                }
                Outcome::Cancelled => {
                    let ProcessReply::State {
                        state: OperationState::Settled(snapshot),
                        ..
                    } = reply
                    else {
                        panic!("expected cancelled registration")
                    };
                    assert_eq!(snapshot.disposition, SettledDisposition::Cancelled);
                    assert!(snapshot.physical_settlement_proven);
                    assert_eq!(
                        ops.dispatch(ProcessRequest::Start { key: key.clone() }),
                        ProcessReply::State {
                            key: key.clone(),
                            state: OperationState::Settled(snapshot)
                        }
                    );
                    assert!(!capture_path(&ops, &key).exists());
                    assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
                }
                Outcome::Revoked => {
                    let receipt: ControlPoll = read_record(&mut server).unwrap();
                    assert_eq!(receipt.receipt, Some(ControlReply::Revoked));
                    assert_eq!(reply, ProcessReply::Unknown { key });
                    assert!(ops.captures.path().is_none());
                    assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
                }
            }
            assert!(!workspace.path().join("unexpected-start").exists());
            worker.stop();
            ops.shutdown().unwrap();
            assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        }
    }

    #[test]
    fn lazy_foreground_allocation_does_not_block_cancel_or_health() {
        use rustx_execution_protocol::transport::{read_record, write_record};
        use rustx_execution_protocol::{ControlPoll, ControlReply, ControlRequest, ControlTurn};
        use rustx_execution_transport::RecordChannel;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc::channel;

        assert!(supervisor_staged());
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let overflow = register_bash_capture(&ops, &adm, "printf '%65536s' ''", false);
        let other = fresh_key(&adm);
        assert_eq!(
            register(&ops, &other, &spec(&["/bin/touch", "other-started"])),
            ProcessReply::Registered { key: other.clone() }
        );
        let (entered, paused) = channel();
        let (resume, resume_rx) = channel();
        ops.captures
            .hold_allocation(overflow.operation, entered, resume_rx);
        ops.dispatch(ProcessRequest::Start {
            key: overflow.clone(),
        });
        paused.recv_timeout(Duration::from_secs(5)).unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        let (data, _server_data) = UnixStream::pair().unwrap();
        let worker = crate::client_control_worker(
            RecordChannel::unix(client),
            Arc::new(crate::mutation::Arbitration::new()),
            RecordChannel::unix(data).shutdown_handle().unwrap(),
            ops.clone(),
            Arc::new(crate::watch::Watches::new()),
        );
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut poll: ControlPoll = read_record(&mut server).unwrap();
        for (request, receipt) in [
            (
                ControlRequest::CancelProcess { key: other.clone() },
                ControlReply::ProcessCancelled {
                    key: other,
                    disposition: ProcessCancelDisposition::CancelledBeforeStart,
                },
            ),
            (
                ControlRequest::Health { nonce: 7.into() },
                ControlReply::Healthy { nonce: 7.into() },
            ),
        ] {
            write_record(
                &mut server,
                &ControlTurn {
                    sequence: poll.sequence,
                    health: poll.health,
                    request: Some(request),
                },
            )
            .unwrap();
            poll = read_record(&mut server).unwrap();
            assert_eq!(poll.receipt, Some(receipt));
        }
        assert!(ops.captures.path().is_none());
        assert!(!dir.path().join("other-started").exists());
        write_record(
            &mut server,
            &ControlTurn {
                sequence: poll.sequence,
                health: poll.health,
                request: Some(ControlRequest::Revoke),
            },
        )
        .unwrap();
        let wait = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        wait.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), ops.revoke.cancelled())
                .await
                .unwrap();
        });
        assert!(ops.settle(Duration::ZERO) >= 1);
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES - 1);
        resume.send(()).unwrap();
        let receipt: ControlPoll = read_record(&mut server).unwrap();
        assert_eq!(receipt.receipt, Some(ControlReply::Revoked));
        assert!(await_settled(&ops, &overflow).physical_settlement_proven);
        assert_eq!(ops.captures.available(), MAX_BASH_CAPTURES);
        assert!(ops.captures.path().is_none());
        worker.stop();
        ops.shutdown().unwrap();
    }

    fn revoke_during_capture_handoff(discard: bool) {
        use rustx_execution_protocol::transport::{read_record, write_record};
        use rustx_execution_protocol::{ControlPoll, ControlReply, ControlRequest, ControlTurn};
        use rustx_execution_transport::RecordChannel;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc::channel;

        let dir = tempfile::tempdir().unwrap();
        let adm = admission(&dir.path().canonicalize().unwrap());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let mut bash = spec(&[]);
        bash.program = ProcessProgram::Shell {
            command: "printf retained; touch did-start".into(),
            conversation: "capture-handoff".into(),
            live_output: true,
        };
        assert_eq!(
            ops.dispatch(ProcessRequest::RegisterAgentBash {
                key: key.clone(),
                fingerprint: bash.fingerprint(),
                spec: bash,
            }),
            ProcessReply::Registered { key: key.clone() }
        );
        let capture = ops.captures.path().unwrap();
        let request = if discard {
            assert!(supervisor_staged());
            ops.dispatch(ProcessRequest::Start { key: key.clone() });
            let snapshot = await_settled(&ops, &key);
            assert_eq!(snapshot.disposition, SettledDisposition::Known);
            assert_eq!(ops.settle(Duration::from_secs(5)), 0);
            assert_eq!(
                ops.dispatch(ProcessRequest::ReleaseOutput {
                    key: key.clone(),
                    final_sequence: snapshot.final_sequence,
                }),
                ProcessReply::OutputReleased { key: key.clone() }
            );
            ProcessRequest::DiscardBashOutput { key: key.clone() }
        } else {
            ProcessRequest::Start { key: key.clone() }
        };
        let (entered, held) = channel();
        let (resume, resume_rx) = channel();
        ops.capture_holds
            .lock()
            .unwrap()
            .insert(key.operation, (entered, resume_rx));
        let data_request = std::thread::spawn({
            let ops = ops.clone();
            move || ops.dispatch(request)
        });
        held.recv_timeout(Duration::from_secs(5)).unwrap();

        let (control_client, mut control_server) = UnixStream::pair().unwrap();
        let (data_client, _data_server) = UnixStream::pair().unwrap();
        let data_client = RecordChannel::unix(data_client);
        let worker = crate::client_control_worker(
            RecordChannel::unix(control_client),
            Arc::new(crate::mutation::Arbitration::new()),
            data_client.shutdown_handle().unwrap(),
            ops.clone(),
            Arc::new(crate::watch::Watches::new()),
        );
        control_server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let poll: ControlPoll = read_record(&mut control_server).unwrap();
        let revoke = ControlTurn {
            sequence: poll.sequence,
            health: poll.health,
            request: Some(ControlRequest::Revoke),
        };
        write_record(&mut control_server, &revoke).unwrap();
        let wait = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        wait.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), ops.revoke.cancelled())
                .await
                .unwrap();
        });
        assert_eq!(ops.settle(Duration::ZERO), 1);
        assert!(capture.is_dir());
        resume.send(()).unwrap();
        let receipt: ControlPoll = read_record(&mut control_server).unwrap();
        assert_eq!(receipt.receipt, Some(ControlReply::Revoked));
        assert!(!capture.exists());
        let result = data_request.join().unwrap();
        if discard {
            assert_eq!(result, ProcessReply::OutputReleased { key });
        } else {
            assert_eq!(
                await_settled(&ops, &key).disposition,
                SettledDisposition::Cancelled
            );
            assert!(!dir.path().join("did-start").exists());
        }
        worker.stop();
    }

    #[test]
    fn revoke_waits_for_start_capture_before_worker_spawn() {
        revoke_during_capture_handoff(false);
    }

    #[test]
    fn revoke_waits_for_discard_to_drop_the_removed_capture() {
        revoke_during_capture_handoff(true);
    }

    #[test]
    fn revoke_fences_a_pending_start() {
        assert!(
            supervisor_staged(),
            "staged bash-supervisor is absent: run test-support/prepare_native_tests.py"
        );
        let dir = tempfile::tempdir().unwrap();
        let adm = admission(dir.path());
        let ops = Arc::new(Operations::new(adm.clone()).unwrap());
        let key = fresh_key(&adm);
        let spec = spec(&["/bin/sh", "-c", "touch fenced"]);
        register(&ops, &key, &spec);
        ops.revoke();
        let reply = ops.dispatch(ProcessRequest::Start { key: key.clone() });
        assert!(matches!(reply, ProcessReply::State { .. }));
        assert!(ops.shutdown().is_ok());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!dir.path().join("fenced").exists());
    }
}
