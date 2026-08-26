//! Deterministic fixture model and tool executors for M3 loop tests.
//!
//! A [`FakeModel`] is a scripted `ModelAdapter`: each invocation consumes the
//! next script of [`FakeStep`]s and yields the scripted canonical events,
//! optionally parking until the invocation's cancellation signal fires. A
//! [`FakeTool`] records its calls and returns one fixed normalized result,
//! optionally parking until the test releases it.
//!
//! All observation is through `tokio::sync::watch` channels, so tests can
//! deterministically wait until a scripted model emitted a known number of
//! events or parked, without timing races.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::stream::unfold;
use rustx::model::{
    ModelAdapter, ModelError, ModelErrorKind, ModelEvent, ModelProtocol, ModelRequest,
};
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::ToolCallId;
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolCall, ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
};
use tokio::sync::watch;

/// One scripted step of a fake model invocation.
#[derive(Debug, Clone)]
pub enum FakeStep {
    /// Yield one canonical model event.
    Emit(ModelEvent),
    /// Wait until the invocation's cancellation signal fires, then fail
    /// with a cancelled model error, exactly like a real adapter.
    ParkUntilCancelled,
    /// Wait until the test releases the invocation through the shared watch
    /// channel, then continue the script without yielding an event. The
    /// watch retains its value, so a release signalled before the park is
    /// observed is never lost.
    ParkUntilReleased(tokio::sync::watch::Receiver<bool>),
}

/// Creates a release channel for [`FakeStep::ParkUntilReleased`]: the test
/// keeps the sender to release the model, and the receiver (cloned into the
/// step and into controller tasks) observes whether the model parked.
#[must_use]
pub fn model_release() -> (
    tokio::sync::watch::Sender<bool>,
    tokio::sync::watch::Receiver<bool>,
) {
    tokio::sync::watch::channel(false)
}

/// A scripted deterministic model adapter.
///
/// The script is a queue of invocation scripts: `stream` pops the next
/// script per invocation, so multi-turn tests script one sub-script per
/// model request. An exhausted script fails explicitly instead of hanging.
/// A scripted model shared as an `Arc`, ready to bind into an attempt model
/// snapshot.
///
/// The Agent Loop receives its provider binding through the attempt's
/// immutable model snapshot, which owns an `Arc<dyn ModelAdapter>`; tests
/// therefore hold the scripted model behind the same handle.
#[must_use]
pub fn fake_model(scripts: Vec<Vec<FakeStep>>) -> std::sync::Arc<FakeModel> {
    std::sync::Arc::new(FakeModel::new(scripts))
}

pub struct FakeModel {
    scripts: Mutex<VecDeque<Vec<FakeStep>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    emitted: watch::Sender<u64>,
    parked: watch::Sender<bool>,
    /// The number of invocation streams whose owner has left: the stream ran
    /// to completion or the loop dropped it. Once this reaches the number of
    /// invocations, no stream owner exists that could still observe a stale
    /// release/callback, which is what a post-quiescence regression needs to
    /// assert instead of merely poking a channel and looking immediately.
    streams_exited: watch::Sender<u64>,
}

/// Increments the exited-stream counter when one invocation stream's owner
/// leaves, whether it completed or was dropped mid-park.
struct StreamExitGuard {
    exited: watch::Sender<u64>,
}

impl Drop for StreamExitGuard {
    fn drop(&mut self) {
        self.exited.send_modify(|count| *count += 1);
    }
}

impl FakeModel {
    /// Creates a fake model from one script per expected invocation.
    #[must_use]
    pub fn new(scripts: Vec<Vec<FakeStep>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Arc::new(Mutex::new(Vec::new())),
            emitted: watch::Sender::new(0),
            parked: watch::Sender::new(false),
            streams_exited: watch::Sender::new(0),
        }
    }

    /// A receiver observing how many invocation stream owners have left.
    #[must_use]
    pub fn streams_exited(&self) -> watch::Receiver<u64> {
        self.streams_exited.subscribe()
    }

    /// How many invocation stream owners have left.
    #[must_use]
    pub fn streams_exited_count(&self) -> u64 {
        *self.streams_exited.borrow()
    }

    /// The canonical requests the loop has sent so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("fake model request lock")
            .clone()
    }

    /// A receiver observing the number of events yielded so far.
    #[must_use]
    pub fn emitted(&self) -> watch::Receiver<u64> {
        self.emitted.subscribe()
    }

    /// A receiver observing whether the current invocation is parked
    /// awaiting cancellation.
    #[must_use]
    pub fn parked(&self) -> watch::Receiver<bool> {
        self.parked.subscribe()
    }

    /// The number of events yielded so far.
    #[must_use]
    pub fn emitted_count(&self) -> u64 {
        *self.emitted.borrow()
    }

    fn pop_script(&self) -> Vec<FakeStep> {
        self.scripts
            .lock()
            .expect("fake model script lock")
            .pop_front()
            .unwrap_or_else(|| {
                vec![FakeStep::Emit(ModelEvent::Failed {
                    error: ModelError {
                        kind: ModelErrorKind::ProviderError,
                        message: "fake model script exhausted".to_owned(),
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                    },
                })]
            })
    }
}

impl ModelAdapter for FakeModel {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationSignal,
    ) -> rustx::model::ModelEventStream {
        self.requests
            .lock()
            .expect("fake model request lock")
            .push(request);
        let script = self.pop_script();
        let emitted = self.emitted.clone();
        let parked = self.parked.clone();
        let exit_guard = Arc::new(StreamExitGuard {
            exited: self.streams_exited.clone(),
        });
        // The guard lives in the stream's own closure state: it fires when the
        // stream owner leaves, whether the script completed or the loop
        // dropped the stream mid-park.
        Box::pin(unfold(script, move |mut script| {
            let _keep_owner_alive = &exit_guard;
            let emitted = emitted.clone();
            let parked = parked.clone();
            let cancellation = cancellation.clone();
            async move {
                loop {
                    if script.is_empty() {
                        return None;
                    }
                    let step = script.remove(0);
                    match step {
                        FakeStep::Emit(event) => {
                            emitted.send_modify(|count| *count += 1);
                            return Some((event, script));
                        }
                        FakeStep::ParkUntilCancelled => {
                            parked.send_replace(true);
                            cancellation.cancelled().await;
                            return Some((
                                ModelEvent::Failed {
                                    error: ModelError {
                                        kind: ModelErrorKind::Cancelled,
                                        message: "fake model cancelled".to_owned(),
                                        retry_after_ms: None,
                                        provider_code: None,
                                        context_overflow: None,
                                    },
                                },
                                script,
                            ));
                        }
                        FakeStep::ParkUntilReleased(mut release) => {
                            parked.send_replace(true);
                            release
                                .wait_for(|released| *released)
                                .await
                                .expect("model release watch closed");
                        }
                    }
                }
            }
        }))
    }
}

/// A scripted deterministic tool executor.
///
/// Every call receives the same fixed result (recorded verbatim by the
/// loop). When constructed as a parking tool, `execute` waits until the
/// test releases it or until the invocation's cancellation signal fires —
/// a parking tool always settles: on cancellation it returns a normalized
/// cancelled result (the loop normalizes the reason to the attempt's), so
/// the committed tool-result batch stays structurally complete.
pub struct FakeTool {
    definition: ToolDefinition,
    result: ToolExecutionResult,
    release: Option<watch::Sender<bool>>,
    progress_reports: usize,
    calls: watch::Sender<Vec<ToolInvocation>>,
    started: watch::Sender<bool>,
    completed: watch::Sender<Vec<String>>,
}

/// Waits for a parking [`FakeTool`] to enter its returned execution future.
/// The watch state is the ordering proof; this bound only contains a broken
/// fixture so a test binary cannot wait forever.
pub async fn await_started(started: &mut watch::Receiver<bool>, description: &'static str) {
    tokio::time::timeout(
        Duration::from_secs(120),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .unwrap_or_else(|_| panic!("{description}: tool start wait exceeded liveness guard"))
    .expect("fake tool start channel stays open");
}

impl FakeTool {
    /// Creates a fake tool returning `result` for every call.
    #[must_use]
    pub fn new(definition: ToolDefinition, result: ToolExecutionResult) -> Self {
        Self {
            definition,
            result,
            release: None,
            progress_reports: 0,
            calls: watch::Sender::new(Vec::new()),
            started: watch::Sender::new(false),
            completed: watch::Sender::new(Vec::new()),
        }
    }

    /// Creates a parking fake tool; the returned watch sender releases the
    /// tool durably, even when the signal precedes the execution future's
    /// first poll.
    #[must_use]
    pub fn parking(
        definition: ToolDefinition,
        result: ToolExecutionResult,
    ) -> (Self, watch::Sender<bool>) {
        let (release, _receiver) = watch::channel(false);
        (
            Self {
                definition,
                result,
                release: Some(release.clone()),
                progress_reports: 0,
                calls: watch::Sender::new(Vec::new()),
                started: watch::Sender::new(false),
                completed: watch::Sender::new(Vec::new()),
            },
            release,
        )
    }

    /// The tool reports `count` numbered progress observations per call
    /// before settling, exactly like a chatty real executor.
    #[must_use]
    pub fn emitting_progress(mut self, count: usize) -> Self {
        self.progress_reports = count;
        self
    }

    /// The canonical definition of this tool, registered together with the
    /// executor.
    #[must_use]
    pub fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    /// The stripped invocations this tool received, in order. Subscribe
    /// before inserting the tool into a registry to observe calls
    /// deterministically.
    #[must_use]
    pub fn calls(&self) -> watch::Receiver<Vec<ToolInvocation>> {
        self.calls.subscribe()
    }

    /// A receiver observing whether execution started.
    #[must_use]
    pub fn started(&self) -> watch::Receiver<bool> {
        self.started.subscribe()
    }

    /// A receiver observing the physical completion order of this tool's
    /// executions, recorded when each execution future resolves.
    #[must_use]
    pub fn completed(&self) -> watch::Receiver<Vec<String>> {
        self.completed.subscribe()
    }

    /// Registers the fake tool (definition + executor) with a registry.
    pub fn register(self, registry: &mut ToolRegistry) {
        registry
            .register(self.definition.clone(), Arc::new(self))
            .expect("fake tool definitions are valid registrations");
    }
}

impl ToolExecutor for FakeTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        self.calls
            .send_modify(|calls| calls.push(invocation.clone()));
        let started = self.started.clone();
        let mut release = self.release.as_ref().map(watch::Sender::subscribe);
        let result = self.result.clone();
        let completed = self.completed.clone();
        let progress_reports = self.progress_reports;
        Box::pin(async move {
            started.send_replace(true);
            for index in 0..progress_reports {
                context.progress.report(rustx::tools::types::ToolProgress {
                    message: Some(format!("progress {index}")),
                    completed: None,
                    total: None,
                });
            }
            let outcome = if let Some(release) = release.as_mut() {
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        ToolExecutionResult {
                            status: ToolExecutionStatus::Cancelled {
                                reason: rustx::runtime::types::CancellationReason::UserRequested,
                            },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                            managed_output: None,
                        }
                    }
                    released = release.wait_for(|released| *released) => {
                        released.expect("fake tool release channel stays open");
                        result
                    },
                }
            } else {
                result
            };
            completed.send_modify(|order| order.push(invocation.tool_name.clone()));
            outcome
        })
    }
}

/// A canonical inbound user message fixture.
#[must_use]
pub fn inbound_message(
    id: &str,
    text: &str,
    source: rustx::message::types::UserSource,
) -> rustx::message::types::UserMessageBlock {
    rustx::message::types::UserMessageBlock {
        id: rustx::runtime::identity::MessageId::new(id),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: text.to_owned(),
            },
        )],
        source,
        kind: rustx::message::types::InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-09T12:00:00Z")
                .expect("fixed timestamp")
                .with_timezone(&chrono::Utc),
        ),
    }
}

/// A tool call scripted by a fake model.
#[derive(Debug, Clone)]
pub struct ScriptedCall {
    /// The canonical call id.
    pub id: &'static str,
    /// The tool identity the call references.
    pub tool_id: &'static str,
    /// The model-facing tool name.
    pub name: &'static str,
    /// The completed arguments.
    pub arguments: serde_json::Value,
}

/// Builds the canonical events of one scripted tool call at `block_index`.
#[must_use]
pub fn tool_call_events(index: u32, call: &ScriptedCall) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolCallStarted {
            block_index: rustx::message::types::ContentBlockIndex::new(index),
            call: rustx::tools::types::ToolCallStart {
                id: ToolCallId::new(call.id),
                tool_id: rustx::runtime::identity::ToolId::new(call.tool_id),
                name: call.name.to_owned(),
            },
        },
        ModelEvent::ToolCallArgumentsDelta {
            block_index: rustx::message::types::ContentBlockIndex::new(index),
            call_id: ToolCallId::new(call.id),
            arguments_delta: serde_json::to_string(&call.arguments).expect("serialize arguments"),
        },
        ModelEvent::ToolCallCompleted {
            block_index: rustx::message::types::ContentBlockIndex::new(index),
            call: ToolCall {
                id: ToolCallId::new(call.id),
                tool_id: rustx::runtime::identity::ToolId::new(call.tool_id),
                name: call.name.to_owned(),
                arguments: call.arguments.clone(),
            },
        },
    ]
}

/// A normalized successful tool result with fixed deterministic fields.
#[must_use]
pub fn success_result(text: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: rustx::tools::types::ToolExecutionStatus::Success,
        content: vec![rustx::tools::types::ToolResultContent::Text(
            rustx::message::content::TextBlock {
                text: text.to_owned(),
            },
        )],
        duration_ms: 7,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// A normalized failed tool result.
#[must_use]
pub fn failed_result(error: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: rustx::tools::types::ToolExecutionStatus::Failed {
            error: error.to_owned(),
        },
        content: Vec::new(),
        duration_ms: 3,
        exit_code: Some(1),
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}
