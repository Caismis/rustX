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

use futures_util::future::BoxFuture;
use futures_util::stream::unfold;
use rustx::model::{
    ModelAdapter, ModelCancellation, ModelError, ModelErrorKind, ModelEvent, ModelProtocol,
    ModelRequest,
};
use rustx::runtime::identity::ToolCallId;
use rustx::tools::executor::Tool;
use rustx::tools::types::{ToolCall, ToolDefinition, ToolExecutionResult};
use tokio::sync::{Notify, watch};

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
pub struct FakeModel {
    scripts: Mutex<VecDeque<Vec<FakeStep>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    emitted: watch::Sender<u64>,
    parked: watch::Sender<bool>,
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
        }
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
        cancellation: ModelCancellation,
    ) -> rustx::model::ModelEventStream {
        self.requests
            .lock()
            .expect("fake model request lock")
            .push(request);
        let script = self.pop_script();
        let emitted = self.emitted.clone();
        let parked = self.parked.clone();
        Box::pin(unfold(script, move |mut script| {
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

/// A scripted deterministic tool.
///
/// Every call receives the same fixed result (recorded verbatim by the
/// loop). When constructed as a parking tool, `execute` waits until the
/// test releases it, simulating a tool whose execution is still running
/// while the attempt is cancelled.
pub struct FakeTool {
    definition: ToolDefinition,
    result: ToolExecutionResult,
    release: Option<Arc<Notify>>,
    calls: watch::Sender<Vec<ToolCall>>,
    started: watch::Sender<bool>,
}

impl FakeTool {
    /// Creates a fake tool returning `result` for every call.
    #[must_use]
    pub fn new(definition: ToolDefinition, result: ToolExecutionResult) -> Self {
        Self {
            definition,
            result,
            release: None,
            calls: watch::Sender::new(Vec::new()),
            started: watch::Sender::new(false),
        }
    }

    /// Creates a parking fake tool; the returned notify releases the tool.
    #[must_use]
    pub fn parking(definition: ToolDefinition, result: ToolExecutionResult) -> (Self, Arc<Notify>) {
        let release = Arc::new(Notify::new());
        (
            Self {
                definition,
                result,
                release: Some(release.clone()),
                calls: watch::Sender::new(Vec::new()),
                started: watch::Sender::new(false),
            },
            release,
        )
    }

    /// The calls this tool received, in order. Subscribe before inserting
    /// the tool into a registry to observe calls deterministically.
    #[must_use]
    pub fn calls(&self) -> watch::Receiver<Vec<ToolCall>> {
        self.calls.subscribe()
    }

    /// A receiver observing whether execution started.
    #[must_use]
    pub fn started(&self) -> watch::Receiver<bool> {
        self.started.subscribe()
    }
}

impl Tool for FakeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'a>(&'a self, call: &'a ToolCall) -> BoxFuture<'a, ToolExecutionResult> {
        self.started.send_replace(true);
        self.calls.send_modify(|calls| calls.push(call.clone()));
        Box::pin(async move {
            if let Some(release) = &self.release {
                release.notified().await;
            }
            self.result.clone()
        })
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
    }
}
