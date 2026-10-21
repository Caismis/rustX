use super::*;
use crate::durable::CanonicalStore;
use crate::durable::ObservationStore;
use crate::durable::RequestStore;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use chrono::{TimeZone, Utc};
use futures_util::{StreamExt, stream};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::durable::auxiliary::{
    AuxiliaryCallStartCommit, AuxiliaryRequestStartCommit, AuxiliarySettlementCommit,
    AuxiliaryWriteFailure,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::model::adapter::{ModelAdapter, ModelStream, ModelStreamItem};
use crate::model::auxiliary::{
    AuxiliaryAttribution, AuxiliaryAuditFact, AuxiliaryBusinessSource, AuxiliaryCallOutcome,
    AuxiliaryCallStart, AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition,
    AuxiliaryFingerprint, AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal,
    AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryRequestTerminal,
    AuxiliaryUsageCompleteness,
};
use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::model::{
    ModelCapabilities, ModelCompat, ModelInvocationConfig, ModelProtocol, ModelRequest,
    SelectedModelInput,
};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AuxiliaryCallId, RequestId, SystemUuidV7Generator};

const MAX_STAGES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    CallStartedCommitted,
    RequestStartedCommitted,
    OptionalLayoutFailureArmed,
    CancelWon,
    DispatchGranted,
    AdapterEntered,
    AdapterStreamExited,
    SettlementCommitted,
    ReceiptReceiverDropped,
    ReceiptDelivered,
    ReceiptDeliveryFailed,
    WriterJoined,
    OperationFinished,
}

#[derive(Clone)]
struct Trace {
    stages: Arc<Mutex<Vec<Stage>>>,
    live: tokio_mpsc::UnboundedSender<Stage>,
}

impl Trace {
    fn new(live: tokio_mpsc::UnboundedSender<Stage>) -> Self {
        Self {
            stages: Arc::new(Mutex::new(Vec::new())),
            live,
        }
    }

    fn record(&self, stage: Stage) {
        let mut stages = self.stages.lock().expect("fixture stage lock");
        assert!(
            stages.len() < MAX_STAGES,
            "supervision fixture exceeded its bounded stage capacity"
        );
        stages.push(stage);
        let _ = self.live.send(stage);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterScript {
    Complete,
    UsageThenFailure,
    UsageThenCompletionWithoutFinalUsage,
    AwaitCancellation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamTerminal {
    Completed {
        finish_reason: ModelFinishReason,
        usage: Option<ModelUsage>,
        usage_completeness: AuxiliaryUsageCompleteness,
    },
    Failed {
        error: ModelError,
        usage: Option<ModelUsage>,
    },
}

struct ScriptedAdapter {
    store: Arc<SqliteConversationStore>,
    expected_request: AuxiliaryRequestStart,
    trace: Trace,
    entered: tokio_mpsc::UnboundedSender<()>,
    entries: Arc<AtomicUsize>,
    script: AdapterScript,
}

impl ModelAdapter for ScriptedAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, _request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
        let prior = self.entries.fetch_add(1, Ordering::AcqRel);
        assert_eq!(
            prior, 0,
            "a consumed dispatch grant entered the adapter more than once"
        );

        let cut = self
            .store
            .auxiliary_audit_cut()
            .expect("read the adapter-entry audit cut");
        let request = self
            .store
            .load_auxiliary_request(&self.expected_request.request_id, cut)
            .expect("read the committed request at adapter entry")
            .expect("adapter entry requires the exact committed RequestStarted fact");
        assert_eq!(request.call_id, self.expected_request.call_id);
        assert_eq!(request.ordinal, self.expected_request.ordinal);
        assert_eq!(request.request_id, self.expected_request.request_id);
        assert_eq!(request.model, self.expected_request.model);
        assert!(request.started_sequence > 0);
        assert!(request.terminal_sequence.is_none());

        self.trace.record(Stage::AdapterEntered);
        let _ = self.entered.send(());

        match self.script {
            AdapterScript::Complete
            | AdapterScript::UsageThenFailure
            | AdapterScript::UsageThenCompletionWithoutFinalUsage => {
                let terminal = match self.script {
                    AdapterScript::Complete => ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: Some(success_usage()),
                    },
                    AdapterScript::UsageThenCompletionWithoutFinalUsage => ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    },
                    AdapterScript::UsageThenFailure => ModelEvent::Failed {
                        error: ModelError::timeout(
                            crate::model::deadline::ModelTimeoutPhase::ResponseStart,
                        ),
                    },
                    AdapterScript::AwaitCancellation => {
                        unreachable!("cancellation owns its stream")
                    }
                };
                Box::pin(stream::iter([
                    ModelStreamItem::Event(ModelEvent::Started),
                    ModelStreamItem::Event(ModelEvent::UsageUpdate {
                        usage: earlier_usage(),
                    }),
                    ModelStreamItem::Event(ModelEvent::UsageUpdate {
                        usage: latest_usage(),
                    }),
                    ModelStreamItem::Event(terminal),
                ]))
            }
            AdapterScript::AwaitCancellation => {
                let state = CancellationState {
                    phase: 0,
                    cancellation,
                };
                Box::pin(stream::unfold(state, |mut state| async move {
                    match state.phase {
                        0 => {
                            state.phase = 1;
                            Some((ModelStreamItem::Event(ModelEvent::Started), state))
                        }
                        1 => {
                            state.cancellation.cancelled().await;
                            state.phase = 2;
                            Some((
                                ModelStreamItem::Event(ModelEvent::Failed {
                                    error: cancelled_error(),
                                }),
                                state,
                            ))
                        }
                        _ => None,
                    }
                }))
            }
        }
    }
}

struct CancellationState {
    phase: u8,
    cancellation: CancellationSignal,
}

#[allow(clippy::large_enum_variant)] // Private bounded test channel owns complete write inputs.
enum WriterRequest {
    CallStart {
        start: AuxiliaryCallStart,
        timestamp: chrono::DateTime<Utc>,
        reply: oneshot::Sender<Result<AuxiliaryCallStartCommit, AuxiliaryWriteFailure>>,
    },
    RequestStart {
        start: AuxiliaryRequestStart,
        timestamp: chrono::DateTime<Utc>,
        reply: oneshot::Sender<Result<AuxiliaryRequestStartCommit, AuxiliaryWriteFailure>>,
    },
    Settlement {
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
        committed: oneshot::Sender<()>,
        receipt_release: mpsc::Receiver<()>,
        receipt: oneshot::Sender<Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure>>,
        reply: oneshot::Sender<Result<SettlementReply, AuxiliaryWriteFailure>>,
    },
}

struct SettlementReply {
    disposition: AuxiliaryCommitDisposition,
    request_terminal: RuntimeEventEnvelope,
    call_settled: RuntimeEventEnvelope,
}

struct OwnedWriter {
    requests: mpsc::Sender<WriterRequest>,
    join: std::thread::JoinHandle<WriterExit>,
}

#[derive(Debug)]
struct WriterExit {
    thread_id: std::thread::ThreadId,
}

impl OwnedWriter {
    fn spawn(store: Arc<SqliteConversationStore>, trace: Trace) -> Self {
        let (requests, receiver) = mpsc::channel::<WriterRequest>();
        let join = std::thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                match request {
                    WriterRequest::CallStart {
                        start,
                        timestamp,
                        reply,
                    } => {
                        let _ = reply.send(store.commit_auxiliary_call_start(&start, timestamp));
                    }
                    WriterRequest::RequestStart {
                        start,
                        timestamp,
                        reply,
                    } => {
                        let _ = reply.send(store.commit_auxiliary_request_start(&start, timestamp));
                    }
                    WriterRequest::Settlement {
                        request_terminal,
                        call_settled,
                        committed,
                        receipt_release,
                        receipt,
                        reply,
                    } => {
                        match store
                            .commit_auxiliary_request_settlement(request_terminal, call_settled)
                        {
                            Ok(commit) => {
                                trace.record(Stage::SettlementCommitted);
                                let disposition = commit.disposition;
                                let reply_value = SettlementReply {
                                    disposition,
                                    request_terminal: canonical_terminal_envelope(
                                        commit.request_terminal.clone().expect(
                                            "final auxiliary settlement has a request terminal",
                                        ),
                                    ),
                                    call_settled: canonical_terminal_envelope(
                                        commit.call_settled.clone(),
                                    ),
                                };
                                let _ = committed.send(());
                                receipt_release
                                    .recv()
                                    .expect("fixture releases the receipt handoff");
                                if receipt.send(Ok(commit)).is_err() {
                                    trace.record(Stage::ReceiptDeliveryFailed);
                                } else {
                                    trace.record(Stage::ReceiptDelivered);
                                }
                                let _ = reply.send(Ok(reply_value));
                            }
                            Err(failure) => {
                                let _ = reply.send(Err(failure));
                            }
                        }
                    }
                }
            }
            WriterExit {
                thread_id: std::thread::current().id(),
            }
        });
        Self { requests, join }
    }

    async fn commit_call_start(
        &self,
        start: AuxiliaryCallStart,
        timestamp: chrono::DateTime<Utc>,
    ) -> Result<AuxiliaryCallStartCommit, String> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(WriterRequest::CallStart {
                start,
                timestamp,
                reply,
            })
            .map_err(|_| "blocking writer closed before CallStarted".to_owned())?;
        response
            .await
            .map_err(|_| "blocking writer dropped the CallStarted reply".to_owned())?
            .map_err(|failure| failure.to_string())
    }

    async fn commit_request_start(
        &self,
        start: AuxiliaryRequestStart,
        timestamp: chrono::DateTime<Utc>,
    ) -> Result<AuxiliaryRequestStartCommit, String> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(WriterRequest::RequestStart {
                start,
                timestamp,
                reply,
            })
            .map_err(|_| "blocking writer closed before RequestStarted".to_owned())?;
        response
            .await
            .map_err(|_| "blocking writer dropped the RequestStarted reply".to_owned())?
            .map_err(|failure| failure.to_string())
    }

    async fn settle(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
        committed: oneshot::Sender<()>,
        receipt_release: mpsc::Receiver<()>,
        receipt: oneshot::Sender<Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure>>,
    ) -> Result<SettlementReply, String> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(WriterRequest::Settlement {
                request_terminal,
                call_settled,
                committed,
                receipt_release,
                receipt,
                reply,
            })
            .map_err(|_| "blocking writer closed before settlement".to_owned())?;
        response
            .await
            .map_err(|_| "blocking writer dropped the settlement reply".to_owned())?
            .map_err(|failure| failure.to_string())
    }

    async fn shutdown(self) -> Result<WriterExit, String> {
        let Self { requests, join } = self;
        drop(requests);
        tokio::task::spawn_blocking(move || join.join())
            .await
            .map_err(|error| format!("blocking writer join task: {error}"))?
            .map_err(|_| "blocking writer panicked".to_owned())
    }
}

fn canonical_terminal_envelope(mut envelope: RuntimeEventEnvelope) -> RuntimeEventEnvelope {
    envelope.tool_occurrence = None;
    envelope.observation = None;
    envelope
}

struct OperationCore {
    request_terminal: RuntimeEventEnvelope,
    call_settled: RuntimeEventEnvelope,
    request_evidence: AuxiliaryRequestEvidence,
    call_outcome: AuxiliaryCallOutcome,
    settlement_disposition: AuxiliaryCommitDisposition,
}

struct SupervisedFixture {
    call_start: AuxiliaryCallStart,
    request_start: AuxiliaryRequestStart,
    cancellation: CancellationSignal,
    grant: Option<oneshot::Sender<()>>,
    entered: tokio_mpsc::UnboundedReceiver<()>,
    stages: tokio_mpsc::UnboundedReceiver<Stage>,
    trace: Trace,
    committed: Option<oneshot::Receiver<()>>,
    receipt_release: mpsc::Sender<()>,
    receipt: Option<oneshot::Receiver<Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure>>>,
    adapter_entries: Arc<AtomicUsize>,
    optional_probe: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<Result<(OperationCore, WriterExit), String>>,
}

impl SupervisedFixture {
    async fn next_stage(&mut self, expected: Stage) {
        let actual = self
            .stages
            .recv()
            .await
            .expect("supervision fixture stage stream closed");
        assert_eq!(actual, expected, "unexpected supervision stage order");
    }

    fn send_grant(&mut self) {
        self.grant
            .take()
            .expect("dispatch grant was already consumed")
            .send(())
            .expect("owner still accepts exactly one dispatch grant");
    }

    async fn wait_committed(&mut self) {
        self.committed
            .take()
            .expect("settlement commit notification")
            .await
            .expect("the blocking writer reported the completed native settlement commit");
    }
}

#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)] // Transfer shared ownership to the supervised fixture.
fn spawn_supervised(
    store: Arc<SqliteConversationStore>,
    script: AdapterScript,
    optional_layout_failure: bool,
) -> SupervisedFixture {
    let call_id = AuxiliaryCallId::generate();
    let request_id = RequestId::generate_auxiliary(&SystemUuidV7Generator)
        .expect("native auxiliary RequestId allocation");
    let call_start = AuxiliaryCallStart {
        call_id: call_id.clone(),
        purpose: AuxiliaryAttribution::new("classification").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
        invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
        selected_message_ids: Vec::new(),
    };
    let request_start = AuxiliaryRequestStart {
        call_id,
        ordinal: 0,
        request_id,
        model: "auxiliary-supervision-fixture".to_owned(),
    };

    let (stage_tx, stages) = tokio_mpsc::unbounded_channel();
    let trace = Trace::new(stage_tx);
    let (entered_tx, entered) = tokio_mpsc::unbounded_channel();
    let entries = Arc::new(AtomicUsize::new(0));
    let optional_probe = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(ScriptedAdapter {
        store: store.clone(),
        expected_request: request_start.clone(),
        trace: trace.clone(),
        entered: entered_tx,
        entries: entries.clone(),
        script,
    });
    let (grant_tx, grant_rx) = oneshot::channel();
    let (committed_tx, committed) = oneshot::channel();
    let (receipt_release_tx, receipt_release_rx) = mpsc::channel();
    let (receipt_tx, receipt_rx) = oneshot::channel();

    let task_store = store.clone();
    let task_call_start = call_start.clone();
    let task_request_start = request_start.clone();
    let task_trace = trace.clone();
    let cancellation = CancellationSignal::new();
    let task_cancellation = cancellation.clone();
    let task_optional_probe = optional_probe.clone();
    let task = tokio::spawn(async move {
        let writer = OwnedWriter::spawn(task_store.clone(), task_trace.clone());
        let executed = run_operation(
            &writer,
            &task_store,
            &task_trace,
            task_call_start,
            task_request_start,
            adapter,
            task_cancellation,
            grant_rx,
            committed_tx,
            receipt_release_rx,
            receipt_tx,
            optional_layout_failure,
            task_optional_probe,
        )
        .await;
        let writer_exit = writer.shutdown().await;
        task_trace.record(Stage::WriterJoined);
        task_trace.record(Stage::OperationFinished);
        match executed {
            Ok(core) => writer_exit.map(|exit| (core, exit)),
            Err(error) => {
                let _ = writer_exit;
                Err(error)
            }
        }
    });

    SupervisedFixture {
        call_start,
        request_start,
        cancellation,
        grant: Some(grant_tx),
        entered,
        stages,
        trace,
        committed: Some(committed),
        receipt_release: receipt_release_tx,
        receipt: Some(receipt_rx),
        adapter_entries: entries,
        optional_probe,
        task,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_operation(
    writer: &OwnedWriter,
    store: &Arc<SqliteConversationStore>,
    trace: &Trace,
    call_start: AuxiliaryCallStart,
    request_start: AuxiliaryRequestStart,
    adapter: Arc<ScriptedAdapter>,
    cancellation: CancellationSignal,
    mut grant: oneshot::Receiver<()>,
    committed: oneshot::Sender<()>,
    receipt_release: mpsc::Receiver<()>,
    receipt: oneshot::Sender<Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure>>,
    optional_layout_failure: bool,
    optional_probe: Arc<AtomicUsize>,
) -> Result<OperationCore, String> {
    let call_commit = writer
        .commit_call_start(call_start.clone(), fixture_time())
        .await?;
    assert_eq!(
        call_commit.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    trace.record(Stage::CallStartedCommitted);

    let request_commit = writer
        .commit_request_start(request_start.clone(), fixture_time())
        .await?;
    assert_eq!(
        request_commit.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    trace.record(Stage::RequestStartedCommitted);

    if optional_layout_failure {
        arm_optional_layout_failure(store, optional_probe)?;
        trace.record(Stage::OptionalLayoutFailureArmed);
    }

    let dispatched = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            grant.close();
            trace.record(Stage::CancelWon);
            false
        }
        result = &mut grant => {
            result.map_err(|_| "dispatch grant owner disappeared".to_owned())?;
            trace.record(Stage::DispatchGranted);
            true
        }
    };

    let terminal = if dispatched {
        let mut model_stream =
            adapter.stream(adapter_request(&request_start), cancellation.clone());
        let mut terminal = None;
        // Trusted scripted cumulative updates exercise the Store handoff only.
        // General provider stream-integrity validation belongs to AUX-02.
        let mut observed_usage = None;
        while let Some(item) = model_stream.next().await {
            if terminal.is_some() {
                return Err("adapter emitted an item after its terminal".to_owned());
            }
            match item {
                ModelStreamItem::Event(ModelEvent::UsageUpdate { usage }) => {
                    observed_usage = Some(usage);
                }
                ModelStreamItem::Event(ModelEvent::Completed {
                    finish_reason,
                    usage,
                }) => {
                    let usage_completeness = if usage.is_some() {
                        AuxiliaryUsageCompleteness::Complete
                    } else if observed_usage.is_some() {
                        AuxiliaryUsageCompleteness::Partial
                    } else {
                        AuxiliaryUsageCompleteness::Unreported
                    };
                    terminal = Some(StreamTerminal::Completed {
                        finish_reason,
                        usage: usage.or_else(|| observed_usage.take()),
                        usage_completeness,
                    });
                }
                ModelStreamItem::Event(ModelEvent::Failed { error }) => {
                    terminal = Some(StreamTerminal::Failed {
                        error,
                        usage: observed_usage.take(),
                    });
                }
                ModelStreamItem::Event(_) | ModelStreamItem::Progress(_) => {}
            }
        }
        trace.record(Stage::AdapterStreamExited);
        terminal.ok_or_else(|| "adapter stream ended without a terminal".to_owned())?
    } else {
        StreamTerminal::Failed {
            error: cancelled_error(),
            usage: None,
        }
    };

    let settlement = settlement_for(store, &request_start, dispatched, terminal);
    let settlement_reply = writer
        .settle(
            settlement.request_terminal.clone(),
            settlement.call_settled.clone(),
            committed,
            receipt_release,
            receipt,
        )
        .await?;

    Ok(OperationCore {
        request_terminal: settlement_reply.request_terminal,
        call_settled: settlement_reply.call_settled,
        request_evidence: settlement.request_evidence,
        call_outcome: settlement.call_outcome,
        settlement_disposition: settlement_reply.disposition,
    })
}

struct Settlement {
    request_terminal: RuntimeEventEnvelope,
    call_settled: RuntimeEventEnvelope,
    request_evidence: AuxiliaryRequestEvidence,
    call_outcome: AuxiliaryCallOutcome,
}

fn settlement_for(
    store: &SqliteConversationStore,
    request_start: &AuxiliaryRequestStart,
    adapter_entered: bool,
    terminal: StreamTerminal,
) -> Settlement {
    let owner = ModelRequestOwner::Auxiliary {
        call_id: request_start.call_id.clone(),
        ordinal: request_start.ordinal,
    };
    match terminal {
        StreamTerminal::Completed {
            finish_reason,
            usage,
            usage_completeness,
        } => {
            let evidence_usage = usage.clone();
            let request_terminal = runtime_envelope(
                store,
                RuntimeEvent::ModelRequestCompleted {
                    owner,
                    request_id: request_start.request_id.clone(),
                    finish_reason: finish_reason.clone(),
                    usage: usage.clone(),
                    generation: None,
                },
            );
            let request_evidence = AuxiliaryRequestEvidence {
                request_id: request_start.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: (&finish_reason).into(),
                }),
                usage: evidence_usage,
                usage_completeness,
                remote_execution_uncertainty: false,
            };
            Settlement {
                call_settled: call_settled_envelope(
                    store,
                    request_start.call_id.clone(),
                    AuxiliaryCallOutcome::Completed,
                    request_evidence.clone(),
                ),
                request_terminal,
                request_evidence,
                call_outcome: AuxiliaryCallOutcome::Completed,
            }
        }
        StreamTerminal::Failed { error, usage } => {
            let evidence_usage = usage.clone();
            let call_outcome = if error.kind == ModelErrorKind::Cancelled {
                AuxiliaryCallOutcome::Cancelled
            } else {
                AuxiliaryCallOutcome::Failed
            };
            let request_terminal = runtime_envelope(
                store,
                RuntimeEvent::ModelRequestFailed {
                    owner,
                    request_id: request_start.request_id.clone(),
                    error,
                    usage: usage.clone(),
                    generation: None,
                },
            );
            let request_evidence = AuxiliaryRequestEvidence {
                request_id: request_start.request_id.clone(),
                observed_provider_terminal: None,
                usage: evidence_usage,
                usage_completeness: if usage.is_some() {
                    AuxiliaryUsageCompleteness::Partial
                } else {
                    AuxiliaryUsageCompleteness::Unreported
                },
                remote_execution_uncertainty: adapter_entered,
            };
            Settlement {
                call_settled: call_settled_envelope(
                    store,
                    request_start.call_id.clone(),
                    call_outcome,
                    request_evidence.clone(),
                ),
                request_terminal,
                request_evidence,
                call_outcome,
            }
        }
    }
}

fn call_settled_envelope(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    outcome: AuxiliaryCallOutcome,
    request_evidence: AuxiliaryRequestEvidence,
) -> RuntimeEventEnvelope {
    runtime_envelope(
        store,
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome,
            request_evidence: Some(request_evidence),
        },
    )
}

fn runtime_envelope(store: &SqliteConversationStore, event: RuntimeEvent) -> RuntimeEventEnvelope {
    crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        fixture_time(),
        event,
    )
    .unwrap()
}

fn adapter_request(request_start: &AuxiliaryRequestStart) -> ModelRequest {
    ModelRequest {
        images: crate::model::images::ResolvedImages::new(),
        invocation: ModelInvocationConfig {
            model: request_start.model.clone(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: crate::model::RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        messages: SelectedModelInput::default(),
        tools: Vec::new(),
        effective_system_prompt: String::new(),
        continuation: None,
        continuation_source: None,
    }
}

fn earlier_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: 2,
        output_tokens: 1,
        total_tokens: 3,
        details: Some(crate::model::types::UsageDetails {
            reasoning_tokens: Some(1),
            cached_input_tokens: Some(0),
        }),
    }
}

fn latest_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: 7,
        output_tokens: 4,
        total_tokens: 11,
        details: None,
    }
}

fn success_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: 11,
        output_tokens: 7,
        total_tokens: 18,
        details: None,
    }
}

fn cancelled_error() -> ModelError {
    ModelError {
        kind: ModelErrorKind::Cancelled,
        message: "supervision fixture cancelled the owned stream".to_owned(),
        retry_disposition: ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 4, 0, 0).unwrap()
}

fn arm_optional_layout_failure(
    store: &SqliteConversationStore,
    probe: Arc<AtomicUsize>,
) -> Result<(), String> {
    let connection = store.lock().map_err(|error| error.to_string())?;
    connection
        .create_scalar_function(
            "auxiliary_optional_layout_probe",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| {
                probe.fetch_add(1, Ordering::SeqCst);
                Ok(1_i64)
            },
        )
        .map_err(|error| format!("install optional auxiliary layout probe: {error}"))?;
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER auxiliary_optional_layout_failure
             BEFORE UPDATE OF observation_json ON events
             WHEN auxiliary_optional_layout_probe()=1
             BEGIN
                 SELECT RAISE(FAIL, 'auxiliary optional layout failure');
             END;",
        )
        .map_err(|error| format!("arm optional auxiliary layout failure: {error}"))
}

fn drop_optional_layout_failure(store: &SqliteConversationStore) -> Result<(), String> {
    store
        .lock()
        .map_err(|error| error.to_string())?
        .execute_batch("DROP TRIGGER auxiliary_optional_layout_failure;")
        .map_err(|error| format!("drop optional auxiliary layout failure: {error}"))
}

fn observation_unavailable(store: &SqliteConversationStore, sequence: u64) -> Result<bool, String> {
    store
        .lock()
        .map_err(|error| error.to_string())?
        .query_row(
            "SELECT observation_unavailable=1 FROM events WHERE sequence=?1",
            [seq_to_i64(sequence).map_err(|error| error.to_string())?],
            |row| row.get(0),
        )
        .map_err(|error| format!("read optional auxiliary layout state: {error}"))
}

#[allow(clippy::too_many_lines)]
fn assert_exact_committed_chain(
    store: &SqliteConversationStore,
    request_start: &AuxiliaryRequestStart,
    request_terminal: &RuntimeEventEnvelope,
    call_settled: &RuntimeEventEnvelope,
    expected_evidence: &AuxiliaryRequestEvidence,
    expected_outcome: AuxiliaryCallOutcome,
) {
    let confirmation = store
        .confirm_auxiliary_settlement(request_terminal, call_settled)
        .expect("exact settlement confirmation");
    let replay = match confirmation {
        AuxiliaryCommitConfirmation::ConfirmedCommitted(commit) => commit,
        other => panic!("expected exact committed settlement, got {other:?}"),
    };
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(replay.request_terminal.as_ref(), Some(request_terminal));
    assert_eq!(&replay.call_settled, call_settled);

    let cut = store.auxiliary_audit_cut().unwrap();
    let page = store
        .read_auxiliary_audit(cut, None, 32)
        .expect("bounded auxiliary audit page");
    assert_eq!(page.total, 4);
    assert_eq!(page.records.len(), 4);
    assert_eq!(page.next_offset, None);
    let facts = page
        .records
        .iter()
        .map(|record| {
            (
                record.journal_sequence,
                match &record.fact {
                    AuxiliaryAuditFact::CallStarted(_) => 0,
                    AuxiliaryAuditFact::RequestStarted(_) => 1,
                    AuxiliaryAuditFact::RequestSettled(_) => 2,
                    AuxiliaryAuditFact::CallSettled(_) => 3,
                },
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        facts.iter().map(|(_, fact)| *fact).collect::<Vec<_>>(),
        vec![0, 1, 2, 3],
        "the four Call lifecycle facts commit exactly once and in order"
    );
    assert!(
        facts.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "each lifecycle fact allocates a distinct Journal sequence"
    );

    let request = store
        .load_auxiliary_request(&request_start.request_id, cut)
        .unwrap()
        .expect("committed auxiliary request");
    assert_eq!(
        request.local_terminal,
        Some(if expected_outcome == AuxiliaryCallOutcome::Completed {
            AuxiliaryRequestTerminal::Completed
        } else {
            AuxiliaryRequestTerminal::Failed
        })
    );
    assert_eq!(
        request.normalized_failure_kind,
        match expected_outcome {
            AuxiliaryCallOutcome::Completed => None,
            AuxiliaryCallOutcome::Cancelled => Some(ModelErrorKind::Cancelled),
            AuxiliaryCallOutcome::Failed => match &request_terminal.event {
                RuntimeEvent::ModelRequestFailed { error, .. } => Some(error.kind.clone()),
                _ => panic!("a failed Call must retain its physical failure"),
            },
        }
    );
    assert_eq!(request.usage, expected_evidence.usage);
    assert_eq!(&request.request_evidence, &Some(expected_evidence.clone()));
    assert_eq!(request.terminal_sequence, Some(request_terminal.sequence));
    let call = store
        .load_auxiliary_call(&request_start.call_id, cut)
        .unwrap()
        .expect("committed auxiliary Call");
    assert_eq!(call.outcome, Some(expected_outcome));
    assert_eq!(call.settled_sequence, Some(call_settled.sequence));

    for event_id in [
        format!("auxiliary-call-started:{}", request_start.call_id),
        format!("auxiliary-request-started:{}", request_start.request_id),
        request_terminal.event_id.to_string(),
        call_settled.event_id.to_string(),
    ] {
        assert_eq!(
            event_count(store, &event_id),
            1,
            "canonical fact {event_id} must exist exactly once"
        );
    }
}

fn event_count(store: &SqliteConversationStore, event_id: &str) -> i64 {
    store
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE event_id=?1",
            [event_id],
            |row| row.get(0),
        )
        .unwrap()
}

async fn finish_receipts(fixture: &mut SupervisedFixture) -> AuxiliarySettlementCommit {
    fixture.wait_committed().await;
    let receipt = fixture.receipt.take().expect("receipt receiver");
    fixture
        .receipt_release
        .send(())
        .expect("release settlement receipt handoff");
    receipt
        .await
        .expect("receipt sender delivered the real native receipt")
        .expect("native settlement commit succeeded")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cancellation_before_dispatch_grant_never_enters_adapter_and_settles_cancelled() {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let mut fixture = spawn_supervised(store.clone(), AdapterScript::Complete, false);

    fixture.next_stage(Stage::CallStartedCommitted).await;
    fixture.next_stage(Stage::RequestStartedCommitted).await;
    fixture.cancellation.cancel();
    fixture.next_stage(Stage::CancelWon).await;
    assert_eq!(fixture.adapter_entries.load(Ordering::Acquire), 0);
    assert!(
        fixture.grant.take().unwrap().send(()).is_err(),
        "a cancellation winner drops the competing grant receiver"
    );

    fixture.next_stage(Stage::SettlementCommitted).await;
    let receipt = finish_receipts(&mut fixture).await;
    assert_eq!(
        receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    fixture.next_stage(Stage::ReceiptDelivered).await;
    fixture.next_stage(Stage::WriterJoined).await;
    fixture.next_stage(Stage::OperationFinished).await;

    let request_start = fixture.request_start.clone();
    let (core, writer_exit) = fixture.task.await.unwrap().unwrap();
    assert_ne!(writer_exit.thread_id, std::thread::current().id());
    assert_eq!(core.call_outcome, AuxiliaryCallOutcome::Cancelled);
    assert_eq!(
        core.settlement_disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    assert_eq!(fixture.adapter_entries.load(Ordering::Acquire), 0);
    assert_eq!(
        core.request_evidence,
        AuxiliaryRequestEvidence {
            request_id: request_start.request_id.clone(),
            observed_provider_terminal: None,
            usage: None,
            usage_completeness: AuxiliaryUsageCompleteness::Unreported,
            remote_execution_uncertainty: false,
        }
    );
    assert_exact_committed_chain(
        &store,
        &request_start,
        &core.request_terminal,
        &core.call_settled,
        &core.request_evidence,
        AuxiliaryCallOutcome::Cancelled,
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn dispatch_grant_wins_then_cancellation_exits_one_real_adapter_stream_before_settlement() {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let mut fixture = spawn_supervised(store.clone(), AdapterScript::AwaitCancellation, false);

    fixture.next_stage(Stage::CallStartedCommitted).await;
    fixture.next_stage(Stage::RequestStartedCommitted).await;
    fixture.send_grant();
    fixture.next_stage(Stage::DispatchGranted).await;
    fixture.next_stage(Stage::AdapterEntered).await;
    fixture
        .entered
        .recv()
        .await
        .expect("the adapter itself observed committed RequestStarted entry");
    assert_eq!(fixture.adapter_entries.load(Ordering::Acquire), 1);
    fixture.cancellation.cancel();
    fixture.next_stage(Stage::AdapterStreamExited).await;
    fixture.next_stage(Stage::SettlementCommitted).await;

    let receipt = finish_receipts(&mut fixture).await;
    assert_eq!(
        receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    fixture.next_stage(Stage::ReceiptDelivered).await;
    fixture.next_stage(Stage::WriterJoined).await;
    fixture.next_stage(Stage::OperationFinished).await;

    let request_start = fixture.request_start.clone();
    let (core, writer_exit) = fixture.task.await.unwrap().unwrap();
    assert_ne!(writer_exit.thread_id, std::thread::current().id());
    assert_eq!(fixture.adapter_entries.load(Ordering::Acquire), 1);
    assert_eq!(core.call_outcome, AuxiliaryCallOutcome::Cancelled);
    assert_eq!(core.request_evidence.observed_provider_terminal, None);
    assert!(core.request_evidence.remote_execution_uncertainty);
    assert_exact_committed_chain(
        &store,
        &request_start,
        &core.request_terminal,
        &core.call_settled,
        &core.request_evidence,
        AuxiliaryCallOutcome::Cancelled,
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn trusted_cumulative_usage_reaches_exact_settlement_without_primary_attribution() {
    for (script, expected_outcome) in [
        (
            AdapterScript::UsageThenFailure,
            AuxiliaryCallOutcome::Failed,
        ),
        (
            AdapterScript::UsageThenCompletionWithoutFinalUsage,
            AuxiliaryCallOutcome::Completed,
        ),
    ] {
        let store =
            Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
        store
            .initialize(&[user_message("usage-primary-human", "Primary input")])
            .unwrap();
        let mut primary = progress_request_snapshot(
            "usage-primary-attempt",
            "1",
            0,
            store.load_head().unwrap().revision,
            Vec::new(),
        );
        freeze_test_input(&store, &mut primary, &[]);
        store
            .commit_model_turn_start(&[], &primary, fixture_time())
            .unwrap();
        let before = store.auxiliary_audit_cut().unwrap().journal;
        let original_canonical = store.load_canonical().unwrap();
        let original_head = store.load_head().unwrap();
        let original_statistics = store.observation_agent_statistics(before).unwrap();
        let original_occupancy = store.observation_occupancy(before).unwrap();
        let original_trace_count = store.observation_trace_total(before).unwrap();
        assert!(
            original_trace_count > 0,
            "the control requires a real primary request"
        );

        let mut fixture = spawn_supervised(store.clone(), script, false);
        fixture.next_stage(Stage::CallStartedCommitted).await;
        fixture.next_stage(Stage::RequestStartedCommitted).await;
        fixture.send_grant();
        fixture.next_stage(Stage::DispatchGranted).await;
        fixture.next_stage(Stage::AdapterEntered).await;
        fixture.next_stage(Stage::AdapterStreamExited).await;
        fixture.next_stage(Stage::SettlementCommitted).await;
        let receipt = finish_receipts(&mut fixture).await;
        assert_eq!(
            receipt.disposition,
            AuxiliaryCommitDisposition::NewlyCommitted
        );
        fixture.next_stage(Stage::ReceiptDelivered).await;
        fixture.next_stage(Stage::WriterJoined).await;
        fixture.next_stage(Stage::OperationFinished).await;

        let request_start = fixture.request_start.clone();
        let entries = fixture.adapter_entries.clone();
        let (core, writer_exit) = fixture.task.await.unwrap().unwrap();
        assert_ne!(writer_exit.thread_id, std::thread::current().id());
        assert_eq!(entries.load(Ordering::Acquire), 1);
        assert_eq!(core.call_outcome, expected_outcome);
        let expected_usage = ModelUsage {
            input_tokens: 7,
            output_tokens: 4,
            total_tokens: 11,
            details: None,
        };
        assert_eq!(core.request_evidence.usage, Some(expected_usage.clone()));
        assert_eq!(
            core.request_evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
        assert_ne!(expected_usage, earlier_usage());
        assert_ne!(
            expected_usage.total_tokens,
            earlier_usage().total_tokens + latest_usage().total_tokens
        );
        match &core.request_terminal.event {
            RuntimeEvent::ModelRequestFailed { error, usage, .. } => {
                assert_eq!(expected_outcome, AuxiliaryCallOutcome::Failed);
                assert_eq!(error.kind, ModelErrorKind::Timeout);
                assert_eq!(usage.as_ref(), Some(&expected_usage));
                assert!(core.request_evidence.remote_execution_uncertainty);
                assert!(core.request_evidence.observed_provider_terminal.is_none());
            }
            RuntimeEvent::ModelRequestCompleted { usage, .. } => {
                assert_eq!(expected_outcome, AuxiliaryCallOutcome::Completed);
                assert_eq!(usage.as_ref(), Some(&expected_usage));
                assert!(!core.request_evidence.remote_execution_uncertainty);
                assert_eq!(
                    core.request_evidence.observed_provider_terminal,
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                    })
                );
            }
            _ => panic!("the fixture must commit the scripted physical terminal"),
        }
        assert_exact_committed_chain(
            &store,
            &request_start,
            &core.request_terminal,
            &core.call_settled,
            &core.request_evidence,
            expected_outcome,
        );
        let after = store.auxiliary_audit_cut().unwrap().journal;
        assert_eq!(store.load_canonical().unwrap(), original_canonical);
        assert_eq!(store.load_head().unwrap(), original_head);
        assert_eq!(
            store.load_request_snapshot(&primary.request_id).unwrap(),
            primary
        );
        assert_eq!(
            store.observation_agent_statistics(after).unwrap(),
            original_statistics
        );
        assert_eq!(
            store.observation_occupancy(after).unwrap(),
            original_occupancy
        );
        assert_eq!(
            store.observation_trace_total(after).unwrap(),
            original_trace_count
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn successful_stream_commits_then_lost_receipt_is_confirmed_only_after_both_joins() {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let mut fixture = spawn_supervised(store.clone(), AdapterScript::Complete, false);

    fixture.next_stage(Stage::CallStartedCommitted).await;
    fixture.next_stage(Stage::RequestStartedCommitted).await;
    fixture.send_grant();
    fixture.next_stage(Stage::DispatchGranted).await;
    fixture.next_stage(Stage::AdapterEntered).await;
    fixture.next_stage(Stage::AdapterStreamExited).await;
    fixture.next_stage(Stage::SettlementCommitted).await;

    fixture.wait_committed().await;
    drop(
        fixture
            .receipt
            .take()
            .expect("real settlement receipt receiver"),
    );
    fixture.trace.record(Stage::ReceiptReceiverDropped);
    fixture.next_stage(Stage::ReceiptReceiverDropped).await;
    fixture
        .receipt_release
        .send(())
        .expect("release handoff after dropping its receiver");
    fixture.next_stage(Stage::ReceiptDeliveryFailed).await;
    fixture.next_stage(Stage::WriterJoined).await;
    fixture.next_stage(Stage::OperationFinished).await;

    let request_start = fixture.request_start.clone();
    let call_start = fixture.call_start.clone();
    let (core, writer_exit) = fixture.task.await.unwrap().unwrap();
    assert_ne!(writer_exit.thread_id, std::thread::current().id());
    assert_eq!(fixture.adapter_entries.load(Ordering::Acquire), 1);
    assert_eq!(core.call_outcome, AuxiliaryCallOutcome::Completed);
    assert_eq!(
        core.settlement_disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    assert_eq!(
        core.request_evidence.observed_provider_terminal,
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        })
    );
    assert_eq!(core.request_evidence.usage, Some(success_usage()));

    let call_confirmation = store
        .confirm_auxiliary_call_start(&call_start)
        .expect("CallStarted confirmation after writer exit");
    assert!(matches!(
        call_confirmation,
        AuxiliaryCommitConfirmation::ConfirmedCommitted(commit)
            if commit.disposition == AuxiliaryCommitDisposition::IdempotentReplay
    ));
    let request_confirmation = store
        .confirm_auxiliary_request_start(&request_start)
        .expect("RequestStarted confirmation after writer exit");
    assert!(matches!(
        request_confirmation,
        AuxiliaryCommitConfirmation::ConfirmedCommitted(commit)
            if commit.disposition == AuxiliaryCommitDisposition::IdempotentReplay
    ));
    assert_exact_committed_chain(
        &store,
        &request_start,
        &core.request_terminal,
        &core.call_settled,
        &core.request_evidence,
        AuxiliaryCallOutcome::Completed,
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn optional_layout_failure_does_not_change_successful_mandatory_settlement() {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let mut fixture = spawn_supervised(store.clone(), AdapterScript::Complete, true);

    fixture.next_stage(Stage::CallStartedCommitted).await;
    fixture.next_stage(Stage::RequestStartedCommitted).await;
    fixture.next_stage(Stage::OptionalLayoutFailureArmed).await;
    fixture.send_grant();
    fixture.next_stage(Stage::DispatchGranted).await;
    fixture.next_stage(Stage::AdapterEntered).await;
    fixture.next_stage(Stage::AdapterStreamExited).await;
    fixture.next_stage(Stage::SettlementCommitted).await;

    let receipt = finish_receipts(&mut fixture).await;
    assert_eq!(
        receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    fixture.next_stage(Stage::ReceiptDelivered).await;
    fixture.next_stage(Stage::WriterJoined).await;
    fixture.next_stage(Stage::OperationFinished).await;

    let request_start = fixture.request_start.clone();
    let (core, writer_exit) = fixture.task.await.unwrap().unwrap();
    assert_ne!(writer_exit.thread_id, std::thread::current().id());
    assert!(
        fixture.optional_probe.load(Ordering::Acquire) >= 2,
        "the optional-layout hook must be consumed by both terminal events"
    );
    assert!(
        observation_unavailable(&store, core.request_terminal.sequence).unwrap(),
        "the consumed optional-layout trigger must retire the request terminal display layout"
    );
    assert!(
        observation_unavailable(&store, core.call_settled.sequence).unwrap(),
        "the consumed optional-layout trigger must retire the Call settlement display layout"
    );
    drop_optional_layout_failure(&store).unwrap();
    assert_exact_committed_chain(
        &store,
        &request_start,
        &core.request_terminal,
        &core.call_settled,
        &core.request_evidence,
        AuxiliaryCallOutcome::Completed,
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn unfinished_blocked_writer_is_unknown_then_joined_commit_is_exact() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("auxiliary-supervision.sqlite");
    let conversation = ConversationId::generate();
    let observer = Arc::new(SqliteConversationStore::open(conversation.clone(), &path).unwrap());
    let contender = Arc::new(SqliteConversationStore::open(conversation.clone(), &path).unwrap());
    let holder = Arc::new(SqliteConversationStore::open(conversation, &path).unwrap());

    let call_id = AuxiliaryCallId::generate();
    let request_start = AuxiliaryRequestStart {
        call_id: call_id.clone(),
        ordinal: 0,
        request_id: RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap(),
        model: "auxiliary-blocked-writer".to_owned(),
    };
    let call_start = AuxiliaryCallStart {
        call_id,
        purpose: AuxiliaryAttribution::new("classification").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: AuxiliaryFingerprint::new("c".repeat(64)).unwrap(),
        invocation_fingerprint: AuxiliaryFingerprint::new("d".repeat(64)).unwrap(),
        selected_message_ids: Vec::new(),
    };
    observer
        .commit_auxiliary_call_start(&call_start, fixture_time())
        .unwrap();

    let (holder_entered, holder_entered_rx) = mpsc::channel();
    let (holder_release, holder_release_rx) = mpsc::channel();
    let holder_task = std::thread::spawn(move || {
        let connection = holder.lock().unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        holder_entered.send(()).unwrap();
        holder_release_rx.recv().unwrap();
        connection.execute_batch("ROLLBACK").unwrap();
    });
    holder_entered_rx.recv().unwrap();

    let (busy, busy_rx) = mpsc::channel();
    let contender_start = request_start.clone();
    let writer = std::thread::spawn(move || {
        let _probe = busy_probe::observe(move |count| {
            if count == 0 {
                let _ = busy.send(());
            }
        });
        let outcome = contender.commit_auxiliary_request_start(&contender_start, fixture_time());
        (std::thread::current().id(), outcome)
    });
    busy_rx
        .recv()
        .expect("the actual SQLite writer reports its blocked transaction attempt");

    assert!(matches!(
        observer
            .confirm_auxiliary_request_start(&request_start)
            .unwrap(),
        AuxiliaryCommitConfirmation::Unknown
    ));
    assert_eq!(
        observer
            .auxiliary_audit_total(observer.auxiliary_audit_cut().unwrap())
            .unwrap(),
        1,
        "only CallStarted is visible while the request writer remains unfinished"
    );

    holder_release.send(()).unwrap();
    holder_task.join().unwrap();
    let (writer_thread, committed) = writer.join().unwrap();
    assert_ne!(writer_thread, std::thread::current().id());
    let committed = committed.expect("the joined request writer committed");
    assert_eq!(
        committed.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    let confirmation = observer
        .confirm_auxiliary_request_start(&request_start)
        .unwrap();
    let replay = match confirmation {
        AuxiliaryCommitConfirmation::ConfirmedCommitted(commit) => commit,
        other => panic!("expected exact request confirmation after writer join, got {other:?}"),
    };
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(replay.event.sequence, committed.event.sequence);
}

#[test]
fn actual_rolled_back_auxiliary_transaction_is_typed_known_not_committed() {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let call_id = AuxiliaryCallId::generate();
    let request_start = AuxiliaryRequestStart {
        call_id: call_id.clone(),
        ordinal: 0,
        request_id: RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap(),
        model: "auxiliary-rollback".to_owned(),
    };
    let call_start = AuxiliaryCallStart {
        call_id,
        purpose: AuxiliaryAttribution::new("classification").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: AuxiliaryFingerprint::new("e".repeat(64)).unwrap(),
        invocation_fingerprint: AuxiliaryFingerprint::new("f".repeat(64)).unwrap(),
        selected_message_ids: Vec::new(),
    };
    store
        .commit_auxiliary_call_start(&call_start, fixture_time())
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER auxiliary_request_rollback
             BEFORE INSERT ON auxiliary_requests
             BEGIN
                 SELECT RAISE(FAIL, 'auxiliary request index rollback');
             END;",
        )
        .unwrap();

    let writer_store = store.clone();
    let writer_start = request_start.clone();
    let writer = std::thread::spawn(move || {
        (
            std::thread::current().id(),
            writer_store.commit_auxiliary_request_start(&writer_start, fixture_time()),
        )
    });
    let (writer_thread, result) = writer.join().unwrap();
    assert_ne!(writer_thread, std::thread::current().id());
    assert!(matches!(
        result,
        Err(AuxiliaryWriteFailure::KnownNotCommitted { .. })
    ));

    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER auxiliary_request_rollback;")
        .unwrap();
    assert!(matches!(
        store
            .confirm_auxiliary_request_start(&request_start)
            .unwrap(),
        AuxiliaryCommitConfirmation::Unknown
    ));
    assert_eq!(
        store
            .auxiliary_audit_total(store.auxiliary_audit_cut().unwrap())
            .unwrap(),
        1,
        "the rolled-back request left no Journal or audit fact"
    );
}
