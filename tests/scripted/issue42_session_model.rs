//! Issue #42: the immutable attempt model snapshot, the model-update /
//! attempt-admission linearization, attempt-scoped context windows, and the
//! `session` / `explicit` summary policies.
//!
//! # How the race is proven, not inferred
//!
//! Every ordering assertion here is established by an explicit
//! synchronization point, never by a delay:
//!
//! - the scripted model parks on a `tokio::sync::watch` channel
//!   ([`FakeStep::ParkUntilReleased`]) and publishes that it parked on a
//!   second watch channel ([`FakeModel::parked`]), so a test knows the
//!   attempt is *inside* a specific model turn;
//! - the Runtime Client observation stream is awaited to an exact predicate
//!   ([`receive_until`]), so settlement and admission are observed, not
//!   assumed;
//! - each provider binding is a *distinct* scripted model, so "which model
//!   did this request go to" is answered by which handle recorded it.

use super::{common, support};

use std::sync::Arc;

use rustx::context::SessionContextPolicy;
use rustx::message::content::TextBlock;
use rustx::message::types::{ContentBlockIndex, UserContentBlock};
use rustx::model::catalog::{MapCredentialEnvironment, ModelRef, ProviderId, ReasoningProfileId};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::{SessionModelConfig, SessionModelState, SummaryModelPolicy};
use rustx::model::{ModelAdapter, ModelEvent, ModelFinishReason, ModelProtocol};
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};
use rustx::runtime_client::{
    EventDelivery, EventSubscription, RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientCursor,
    RuntimeClientEvent, RuntimeClientHost, RuntimeClientProtocolEvent,
};
use rustx::tools::executor::ToolRegistry;
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, success_result, tool_call_events,
};
use support::model::{FixtureModel, MappedAdapterFactory, fixture_catalog};

/// The outer liveness guard: waiting is exact (watch channels and the
/// observation stream), so this only bounds a pathological stall.
const LIVENESS: std::time::Duration = std::time::Duration::from_secs(120);

/// The three fixture providers of this file.
const ALPHA: &str = "alpha";
const BETA: &str = "beta";
const SUMMARY: &str = "summary";

/// One scripted provider binding of the fixture catalog.
struct Provider {
    id: &'static str,
    model: &'static str,
    handle: Arc<FakeModel>,
    context_window: u64,
    max_output_tokens: u32,
    request_params: serde_json::Value,
    always_on_reasoning: bool,
}

impl Provider {
    fn new(id: &'static str, model: &'static str, scripts: Vec<Vec<FakeStep>>) -> Self {
        Self {
            id,
            model,
            handle: Arc::new(FakeModel::new(scripts)),
            context_window: 1_000_000,
            max_output_tokens: 4096,
            request_params: serde_json::json!({}),
            always_on_reasoning: false,
        }
    }

    const fn window(mut self, tokens: u64) -> Self {
        self.context_window = tokens;
        self
    }

    const fn output(mut self, tokens: u32) -> Self {
        self.max_output_tokens = tokens;
        self
    }

    fn params(mut self, value: serde_json::Value) -> Self {
        self.request_params = value;
        self
    }

    const fn always_on_reasoning(mut self) -> Self {
        self.always_on_reasoning = true;
        self
    }

    fn reference(&self) -> ModelRef {
        ModelRef::parse(&format!("{}/{}", self.id, self.model)).expect("valid fixture reference")
    }

    fn fixture_model(&self) -> FixtureModel {
        let model = FixtureModel::text(
            &format!("{}/{}", self.id, self.model),
            ModelProtocol::OpenAiChatCompletions,
        )
        .with_context_window(self.context_window)
        .with_max_output_tokens(self.max_output_tokens)
        .with_request_params(self.request_params.clone());
        if self.always_on_reasoning {
            model.always_on_reasoning()
        } else {
            model
        }
    }
}

/// Builds the session model authority over a multi-provider fixture catalog,
/// binding each provider to its own scripted adapter.
///
/// The catalog goes through the ordinary validating load and credential
/// resolution, so endpoint and credential requirements are exercised exactly
/// as in production; only the adapter behind each binding is scripted.
fn session_model(providers: &[&Provider], initial: SessionModelConfig) -> SessionModelState {
    let models: Vec<FixtureModel> = providers.iter().map(|p| p.fixture_model()).collect();
    let bindings: Vec<(ProviderId, Arc<dyn ModelAdapter>)> = providers
        .iter()
        .map(|p| {
            (
                ProviderId::new(p.id),
                Arc::clone(&p.handle) as Arc<dyn ModelAdapter>,
            )
        })
        .collect();
    let factory = MappedAdapterFactory::new(move |provider: &ProviderId, _protocol| {
        bindings
            .iter()
            .find(|(id, _)| id == provider)
            .map(|(_, adapter)| Arc::clone(adapter))
    });
    let resolved = fixture_catalog(&models)
        .resolve(&MapCredentialEnvironment::default())
        .expect("literal fixture credentials resolve");
    let registry = ModelBindingRegistry::new_with_scripted_adapters(resolved, &factory)
        .expect("bindings resolve");
    SessionModelState::new(registry, initial).expect("the initial selection resolves")
}

/// Waits until the scripted model reports it parked inside a turn.
async fn await_parked(model: &Arc<FakeModel>) {
    let mut parked = model.parked();
    tokio::time::timeout(LIVENESS, parked.wait_for(|parked| *parked))
        .await
        .expect("the model must park")
        .expect("the park watch stays open");
}

/// Receives from the observation stream until the predicate matches.
async fn receive_until(
    subscription: &EventSubscription,
    mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
) -> Vec<RuntimeClientProtocolEvent> {
    tokio::time::timeout(LIVENESS, async {
        let mut seen = Vec::new();
        loop {
            let EventDelivery::Event(event) = subscription.next().await else {
                panic!("the subscription must stay open and contiguous");
            };
            let matched = predicate(&event);
            seen.push(event);
            if matched {
                return seen;
            }
        }
    })
    .await
    .expect("the observation stream must not stall")
}

fn text(value: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: value.to_owned(),
    })]
}

fn one_turn_stop() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

/// A turn that assembles one tool call, parks, and only then completes.
fn parked_tool_turn(
    call: &ScriptedCall,
    release: tokio::sync::watch::Receiver<bool>,
) -> Vec<FakeStep> {
    let events = tool_call_events(0, call);
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(events[0].clone()),
        FakeStep::Emit(events[1].clone()),
        FakeStep::Emit(events[2].clone()),
        FakeStep::ParkUntilReleased(release),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]
}

const ALPHA_CALL: ScriptedCall = ScriptedCall {
    id: "call-1",
    tool_id: "tool-alpha",
    name: "alpha",
    arguments: serde_json::Value::Null,
};

/// Builds the runtime under test over an explicit session model authority.
async fn runtime(model: SessionModelState, policy: SessionContextPolicy) -> RuntimeClientHost {
    let mut tools = ToolRegistry::new();
    FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok")).register(&mut tools);
    support::runtime_client_fixture::RuntimeClientFixture::builder("conv-42")
        .tools(tools)
        .session_model(model)
        .context_policy(policy)
        .build()
        .await
        .into_parts()
        .1
}

/// The unconstrained context policy: no compaction is ever possible.
const NO_COMPACTION: SessionContextPolicy = SessionContextPolicy {
    reserve_tokens: 0,
    keep_recent_tokens: 0,
    summary_output_cap: None,
};

/// Submits one inbound message and returns the attachment/subscription pair
/// used to observe the runtime.
fn attach(
    host: &RuntimeClientHost,
) -> (rustx::runtime_client::RuntimeAttachment, EventSubscription) {
    let attachment = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach")
        .0;
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    (attachment, subscription)
}

fn submit(attachment: &rustx::runtime_client::RuntimeAttachment, id: u64, value: &str) {
    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: RequestId::new(id),
        content: text(value),
    });
    assert!(
        response.error.is_none(),
        "submit must be accepted: {response:?}"
    );
}

/// **The linearization test.**
///
/// An attempt is admitted with model A and parked at a deterministic barrier
/// inside its first model turn, with its tool call already assembled. A valid
/// `model_set` to B then linearizes *after* that admission. The attempt is
/// released and runs its tool result → model continuation.
///
/// Every provider request of the attempt must still use A's binding, model,
/// effective request parameters, and output budget. The next admitted attempt
/// must use B.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_update_after_admission_affects_only_future_attempts() {
    let (release, receiver) = support::fake::model_release();
    let alpha = Provider::new(
        ALPHA,
        "model-a",
        vec![parked_tool_turn(&ALPHA_CALL, receiver), one_turn_stop()],
    )
    .output(4096)
    .params(serde_json::json!({"temperature": 0.1, "provider_tag": "alpha"}));
    let beta = Provider::new(BETA, "model-b", vec![one_turn_stop()])
        .output(2048)
        .params(serde_json::json!({"temperature": 0.9, "provider_tag": "beta"}));

    let host = runtime(
        session_model(&[&alpha, &beta], SessionModelConfig::of(alpha.reference())),
        NO_COMPACTION,
    )
    .await;
    let (attachment, subscription) = attach(&host);

    submit(&attachment, 1, "first");
    // The attempt is now provably inside its first model turn, with the tool
    // call assembled and not yet settled.
    await_parked(&alpha.handle).await;

    // The update linearizes strictly after the attempt's admission.
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(SessionModelConfig::of(beta.reference())),
    });
    assert!(
        response.error.is_none(),
        "the update is valid: {response:?}"
    );

    // The runtime can truthfully report both facts at once, so a client never
    // has to infer them from event ordering.
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.model.configured.model,
        beta.reference(),
        "the session desired model is B"
    );
    let attempt = snapshot.attempt.as_ref().expect("a running attempt");
    assert_eq!(
        attempt.model.primary.model,
        alpha.reference(),
        "the running attempt still reports A"
    );

    release.send_replace(true);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    // Attempt 1 issued its first turn and its tool→model continuation, both
    // against A's frozen snapshot.
    let alpha_requests = alpha.handle.requests();
    assert_eq!(
        alpha_requests.len(),
        2,
        "the attempt ran its first turn and its continuation"
    );
    for request in &alpha_requests {
        assert_eq!(request.model(), "model-a");
        assert_eq!(request.max_output_tokens(), 4096);
        assert_eq!(
            request.request_params()["provider_tag"],
            serde_json::json!("alpha")
        );
        assert_eq!(
            request.request_params()["temperature"],
            serde_json::json!(0.1)
        );
    }
    assert!(
        beta.handle.requests().is_empty(),
        "B's binding was never used by the already-admitted attempt"
    );

    // The next attempt snapshots B.
    submit(&attachment, 3, "second");
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let beta_requests = beta.handle.requests();
    assert_eq!(beta_requests.len(), 1, "the next attempt used B's binding");
    assert_eq!(beta_requests[0].model(), "model-b");
    assert_eq!(beta_requests[0].max_output_tokens(), 2048);
    assert_eq!(
        beta_requests[0].request_params()["provider_tag"],
        serde_json::json!("beta")
    );
    assert_eq!(
        alpha.handle.requests().len(),
        2,
        "A received nothing further"
    );
}

/// The inverse ordering: an update that linearizes **before** admission is
/// observed by that attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_update_before_admission_is_observed_by_that_attempt() {
    let alpha = Provider::new(ALPHA, "model-a", vec![one_turn_stop()]);
    let beta = Provider::new(BETA, "model-b", vec![one_turn_stop()]).output(2048);

    let host = runtime(
        session_model(&[&alpha, &beta], SessionModelConfig::of(alpha.reference())),
        NO_COMPACTION,
    )
    .await;
    let (attachment, subscription) = attach(&host);

    // No attempt exists yet, so this update strictly precedes any admission.
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(1),
        config: Box::new(SessionModelConfig::of(beta.reference())),
    });
    assert!(response.error.is_none());

    submit(&attachment, 2, "hello");
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    assert!(
        alpha.handle.requests().is_empty(),
        "the superseded model was never used"
    );
    let requests = beta.handle.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model(), "model-b");
    assert_eq!(requests[0].max_output_tokens(), 2048);
}

/// An attempt's compaction decision uses **its own** model's context window.
///
/// The same canonical history is presented to two models: A's window
/// comfortably holds it, B's does not. The attempt on A performs exactly one
/// provider request; the attempt on B performs a summary request *and* the
/// turn, proving the second attempt planned against B's 4 000-token window
/// and not against A's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attempt_plans_compaction_with_its_own_model_window() {
    // Attempt 1 on A: one turn, no compaction.
    let alpha = Provider::new(ALPHA, "model-a", vec![one_turn_stop()])
        .window(1_000_000)
        .output(256);
    // Attempt 2 on B: the summary one-off first, then the turn. In `session`
    // summary mode the summary goes to B's own binding.
    let beta = Provider::new(
        BETA,
        "model-b",
        vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "compact summary".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
    )
    .window(4_000)
    .output(256);

    let host = runtime(
        session_model(&[&alpha, &beta], SessionModelConfig::of(alpha.reference())),
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 200,
            summary_output_cap: Some(128),
        },
    )
    .await;
    let (attachment, subscription) = attach(&host);

    // A long inbound turn: comfortably inside A's window, well past B's.
    let long = "compaction pressure ".repeat(600);
    submit(&attachment, 1, &long);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(
        alpha.handle.requests().len(),
        1,
        "A's window holds the history: no compaction, exactly one request"
    );

    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(SessionModelConfig::of(beta.reference())),
    });
    assert!(response.error.is_none(), "{response:?}");

    submit(&attachment, 3, &long);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    let beta_requests = beta.handle.requests();
    assert_eq!(
        beta_requests.len(),
        2,
        "B's smaller window forced a compaction summary before the turn"
    );
    // The summary one-off is recognizable: no tools, no continuation, and the
    // context plane's output safety cap applied through the runtime-owned
    // protected max-output field.
    assert!(beta_requests[0].tools.is_empty());
    assert_eq!(beta_requests[0].continuation, None);
    assert_eq!(
        beta_requests[0].max_output_tokens(),
        128,
        "the summary safety cap flows through the protected max-output field"
    );
    assert_eq!(
        beta_requests[1].max_output_tokens(),
        256,
        "the attempt's own turn keeps the model's output budget"
    );
    assert_eq!(
        alpha.handle.requests().len(),
        1,
        "no stale process-start window sent anything to A"
    );
}

/// `summaryModel.mode = "session"` sends the summary to the admitted
/// attempt's own primary invocation, subject only to the documented context
/// summary output cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_summary_mode_uses_the_admitted_primary_invocation() {
    let alpha = Provider::new(
        ALPHA,
        "model-a",
        vec![
            // Attempt 1: fits, one turn.
            one_turn_stop(),
            // Attempt 2: the summary one-off, then the turn.
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "compact summary".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
    )
    .window(4_000)
    .output(300)
    .params(serde_json::json!({"temperature": 0.25, "provider_tag": "alpha"}));

    let host = runtime(
        session_model(&[&alpha], SessionModelConfig::of(alpha.reference())),
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 200,
            summary_output_cap: Some(100),
        },
    )
    .await;
    let (attachment, subscription) = attach(&host);
    // The first turn fits the 4 000-token window; the second pushes the
    // accumulated history past the soft input limit and forces exactly one
    // compaction.
    let long = "compaction pressure ".repeat(600);
    submit(&attachment, 1, &long);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(alpha.handle.requests().len(), 1, "the first attempt fits");

    submit(&attachment, 2, &long);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    let requests = alpha.handle.requests();
    assert_eq!(
        requests.len(),
        3,
        "the first turn, then the second attempt's summary one-off and turn"
    );
    let summary = &requests[1];
    assert_eq!(summary.model(), "model-a");
    assert!(summary.tools.is_empty());
    assert_eq!(summary.continuation, None);
    assert_eq!(
        summary.request_params(),
        requests[2].request_params(),
        "the summary uses the attempt's exact effective request parameters"
    );
    assert_eq!(
        summary.request_params()["provider_tag"],
        serde_json::json!("alpha")
    );
    assert_eq!(
        summary.max_output_tokens(),
        100,
        "only the documented context summary output cap differs"
    );
    assert_eq!(requests[2].max_output_tokens(), 300);
}

/// `summaryModel.mode = "explicit"` resolves a different catalog
/// model/provider/profile through the same resolution path, and the
/// resolution is **frozen at admission**: mutating live session state during
/// the attempt cannot change that attempt's summary model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_summary_mode_is_resolved_once_and_frozen_at_admission() {
    let (release, receiver) = support::fake::model_release();
    // The attempt: a parked tool turn, then a continuation that overflows and
    // triggers compaction, then the retry.
    let alpha = Provider::new(
        ALPHA,
        "model-a",
        vec![
            parked_tool_turn(&ALPHA_CALL, receiver),
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::Failed {
                    error: rustx::model::ModelError {
                        kind: rustx::model::ModelErrorKind::ContextWindowExceeded,
                        message: "too long".to_owned(),
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                    },
                }),
            ],
            one_turn_stop(),
        ],
    )
    .output(300);
    // The frozen explicit summary model.
    let summary = Provider::new(
        SUMMARY,
        "summary-model",
        vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "frozen summary".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
    )
    .output(512)
    .params(serde_json::json!({"temperature": 0.05, "provider_tag": "summary"}));
    // A decoy the session is switched to mid-attempt; it must stay unused by
    // the already-admitted attempt.
    let decoy = Provider::new(BETA, "decoy-summary", vec![one_turn_stop()]);

    let initial = SessionModelConfig {
        summary_model: SummaryModelPolicy::Explicit {
            model: summary.reference(),
            reasoning_profile: None,
            request_params: rustx::model::RequestParams::new(),
            max_output_tokens: None,
        },
        ..SessionModelConfig::of(alpha.reference())
    };
    let host = runtime(
        session_model(&[&alpha, &summary, &decoy], initial),
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 20,
            summary_output_cap: None,
        },
    )
    .await;
    let (attachment, subscription) = attach(&host);

    submit(&attachment, 1, "start");
    await_parked(&alpha.handle).await;

    // Mutate the live session's explicit summary model while the attempt runs.
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: decoy.reference(),
                reasoning_profile: None,
                request_params: rustx::model::RequestParams::new(),
                max_output_tokens: None,
            },
            ..SessionModelConfig::of(alpha.reference())
        }),
    });
    assert!(response.error.is_none(), "{response:?}");

    release.send_replace(true);
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    let summary_requests = summary.handle.requests();
    assert_eq!(
        summary_requests.len(),
        1,
        "the attempt's compaction used the summary model frozen at admission"
    );
    assert_eq!(summary_requests[0].model(), "summary-model");
    assert_eq!(
        summary_requests[0].request_params()["provider_tag"],
        serde_json::json!("summary"),
        "the explicit summary binding keeps its own effective request parameters"
    );
    assert_eq!(summary_requests[0].max_output_tokens(), 512);
    assert!(
        decoy.handle.requests().is_empty(),
        "the live session mutation never reached the already-admitted attempt"
    );
    // Ordinary attempt traffic stayed on the primary model throughout.
    assert!(alpha.handle.requests().len() >= 2);
    for request in alpha.handle.requests() {
        assert_eq!(request.model(), "model-a");
    }
}

/// A reasoning profile selection resolves through the catalog and reaches the
/// wire as exactly its configured parameters; the runtime assigns no meaning
/// to the profile name.
#[test]
fn reasoning_profiles_resolve_to_their_exact_configured_parameters() {
    let model = FixtureModel::text("p/reasoner", ModelProtocol::AnthropicMessages).with_reasoning(
        serde_json::json!({
            "defaultProfile": "on",
            "profiles": {
                "off": {
                    "enabled": false,
                    "requestParams": {"thinking": {"type": "disabled"}, "temperature": 0.7}
                },
                "on": {
                    "enabled": true,
                    "requestParams": {
                        "thinking": {"type": "enabled", "budget_tokens": 32000},
                        "temperature": 1.0
                    }
                },
                "thinking-32k": {
                    "enabled": true,
                    "requestParams": {"thinking": {"type": "enabled", "budget_tokens": 32000}}
                }
            }
        }),
    );
    let handle: Arc<dyn ModelAdapter> = Arc::new(support::model::NullAdapter);
    let factory = support::model::ScriptedAdapterFactory::new(handle);
    let registry = support::model::fixture_registry(&[model], &factory);
    let reference = ModelRef::parse("p/reasoner").expect("reference");

    // The declared default is selected when the session chooses nothing.
    let default = registry
        .resolve(&rustx::model::ModelSelection::of(reference.clone()))
        .expect("default profile resolves");
    assert_eq!(
        default.reasoning_profile(),
        Some(&ReasoningProfileId::new("on"))
    );
    assert!(default.reasoning_enabled());
    assert_eq!(
        serde_json::Value::Object(default.request_params().clone()),
        serde_json::json!({
            "thinking": {"type": "enabled", "budget_tokens": 32000},
            "temperature": 1.0
        }),
        "the effective parameters are exactly the profile overlay"
    );

    // Selecting `off` yields exactly the off overlay — a completely different
    // provider shape, not a remapped enum value.
    let off = registry
        .resolve(&rustx::model::ModelSelection {
            reasoning_profile: Some(ReasoningProfileId::new("off")),
            ..rustx::model::ModelSelection::of(reference.clone())
        })
        .expect("off profile resolves");
    assert!(!off.reasoning_enabled());
    assert_eq!(
        serde_json::Value::Object(off.request_params().clone()),
        serde_json::json!({"thinking": {"type": "disabled"}, "temperature": 0.7})
    );

    // A model-specific profile name carries no runtime meaning.
    let named = registry
        .resolve(&rustx::model::ModelSelection {
            reasoning_profile: Some(ReasoningProfileId::new("thinking-32k")),
            ..rustx::model::ModelSelection::of(reference.clone())
        })
        .expect("named profile resolves");
    assert!(named.reasoning_enabled());
    assert!(!named.request_params().contains_key("temperature"));

    // An undeclared profile fails; no profile is ever synthesized.
    assert!(
        registry
            .resolve(&rustx::model::ModelSelection {
                reasoning_profile: Some(ReasoningProfileId::new("medium")),
                ..rustx::model::ModelSelection::of(reference.clone())
            })
            .is_err(),
        "off/low/medium/high are never synthesized"
    );
}

/// The selected reasoning profile owns every top-level key it declares: a
/// session override that also declares one fails deterministically instead of
/// being resolved by merge order.
#[test]
fn a_session_override_may_not_claim_a_profile_owned_key() {
    let model = FixtureModel::text("p/reasoner", ModelProtocol::AnthropicMessages).with_reasoning(
        serde_json::json!({
            "defaultProfile": "on",
            "profiles": {
                "on": {
                    "enabled": true,
                    "requestParams": {"thinking": {"type": "enabled"}, "temperature": 1.0}
                }
            }
        }),
    );
    let handle: Arc<dyn ModelAdapter> = Arc::new(support::model::NullAdapter);
    let factory = support::model::ScriptedAdapterFactory::new(handle);
    let registry = support::model::fixture_registry(&[model], &factory);
    let reference = ModelRef::parse("p/reasoner").expect("reference");

    let error = registry
        .resolve(&rustx::model::ModelSelection {
            request_params: common::request_params(serde_json::json!({"temperature": 0.2})),
            ..rustx::model::ModelSelection::of(reference.clone())
        })
        .expect_err("a contested key must fail");
    assert!(
        matches!(
            error,
            rustx::model::ModelInvocationError::ReasoningProfileKeyOwnership { .. }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("temperature"));

    // A key the profile does not declare is accepted and overlays normally.
    let ok = registry
        .resolve(&rustx::model::ModelSelection {
            request_params: common::request_params(serde_json::json!({"top_k": 40})),
            ..rustx::model::ModelSelection::of(reference)
        })
        .expect("an uncontested key resolves");
    assert_eq!(ok.request_params()["top_k"], serde_json::json!(40));
    assert_eq!(ok.request_params()["temperature"], serde_json::json!(1.0));
}

/// A failed `model_set` is transactional: the session keeps its previous
/// configuration and nothing is published.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_model_update_changes_nothing() {
    let alpha = Provider::new(ALPHA, "model-a", vec![one_turn_stop()]);
    // A model whose whole window is consumed by the policy reserve.
    let tiny = Provider::new(BETA, "tiny", vec![one_turn_stop()])
        .window(500)
        .output(100);
    let host = runtime(
        session_model(&[&alpha, &tiny], SessionModelConfig::of(alpha.reference())),
        SessionContextPolicy {
            reserve_tokens: 1_000,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
    )
    .await;
    let (attachment, _subscription) = attach(&host);
    let (_, before) = host.snapshot().expect("snapshot");

    for invalid in [
        // An unknown model.
        SessionModelConfig::of(ModelRef::parse("alpha/missing").expect("reference")),
        // An unknown provider.
        SessionModelConfig::of(ModelRef::parse("nowhere/model-a").expect("reference")),
        // An impossible output budget.
        SessionModelConfig {
            max_output_tokens: Some(u32::MAX),
            ..SessionModelConfig::of(alpha.reference())
        },
        // A protected wire key in the session overrides.
        SessionModelConfig {
            request_params: common::request_params(serde_json::json!({"messages": []})),
            ..SessionModelConfig::of(alpha.reference())
        },
        // An unresolvable explicit summary model.
        SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: ModelRef::parse("alpha/missing").expect("reference"),
                reasoning_profile: None,
                request_params: rustx::model::RequestParams::new(),
                max_output_tokens: None,
            },
            ..SessionModelConfig::of(alpha.reference())
        },
        // A protected wire key in the explicit summary overrides.
        SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: alpha.reference(),
                reasoning_profile: None,
                request_params: common::request_params(serde_json::json!({"stream": false})),
                max_output_tokens: None,
            },
            ..SessionModelConfig::of(alpha.reference())
        },
        // A model whose window cannot run under the session context policy.
        SessionModelConfig {
            model: tiny.reference(),
            ..SessionModelConfig::of(alpha.reference())
        },
    ] {
        let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
            id: RequestId::new(9),
            config: Box::new(invalid),
        });
        assert!(
            matches!(
                response.error,
                Some(rustx::runtime_client::RuntimeClientError::InvalidModelConfiguration { .. })
            ),
            "an invalid update must be rejected: {response:?}"
        );
    }

    let (snapshot, after) = host.snapshot().expect("snapshot");
    assert_eq!(
        after, before,
        "a rejected update allocates no cursor and publishes no event"
    );
    assert_eq!(
        snapshot.model.configured,
        SessionModelConfig::of(alpha.reference())
    );
    let RuntimeClientResult::Model { model } = attachment
        .handle_request(RuntimeClientRequest::ModelGet {
            id: RequestId::new(10),
        })
        .result
        .expect("model_get result")
    else {
        panic!("model_get returns the session model");
    };
    assert_eq!(model.configured, SessionModelConfig::of(alpha.reference()));
}

/// A reasoning-capable model without a reasoning profile is semantically
/// always on, while its session selection has no profile to choose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn always_on_reasoning_is_preserved_by_session_resolution() {
    let always_on = Provider::new(ALPHA, "always-on", vec![one_turn_stop()]).always_on_reasoning();
    let state = session_model(&[&always_on], SessionModelConfig::of(always_on.reference()));

    let view = state.view();
    assert_eq!(view.effective.reasoning_profile, None);
    assert!(view.effective.reasoning_enabled);
    let snapshot = state.snapshot();
    assert_eq!(snapshot.primary().reasoning_profile(), None);
    assert!(snapshot.primary().reasoning_enabled());
}
