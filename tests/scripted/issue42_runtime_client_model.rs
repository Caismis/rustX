//! Issue #42: the Runtime Client model read/update contract.
//!
//! The client boundary must let #39 control the real session model without
//! reading `models.jsonc`, without rebuilding the runtime, and without
//! inferring "which model is this attempt actually using" from event
//! ordering. It must also never expose a credential.

use super::{common, support};

use std::sync::Arc;

use rustx::context::SessionContextPolicy;
use rustx::message::content::TextBlock;
use rustx::message::types::{ContentBlockIndex, UserContentBlock};
use rustx::model::catalog::{
    CredentialSourceView, MapCredentialEnvironment, ModelCatalog, ModelRef, ProviderId,
    ReasoningProfileId,
};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::{SessionModelConfig, SessionModelState, SummaryModelView};
use rustx::model::{ModelAdapter, ModelEvent, ModelFinishReason, ModelProtocol};
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};
use rustx::runtime_client::{
    EventDelivery, EventSubscription, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor,
    RuntimeClientError, RuntimeClientEvent, RuntimeClientHost, RuntimeClientProtocolEvent,
};
use support::fake::{FakeModel, FakeStep};
use support::model::{FixtureModel, MappedAdapterFactory, fixture_catalog_document};

const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);

/// The literal credential the fixture catalog binds. It must never appear in
/// any client-visible value.
const FIXTURE_SECRET: &str = "fixture-key-alpha";

const NO_COMPACTION: SessionContextPolicy = SessionContextPolicy {
    reserve_tokens: 0,
    keep_recent_tokens: 0,
    summary_output_cap: None,
};

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

/// The fixture catalog: a reasoning-capable primary model, a second model on
/// another provider, and a summary model — all with explicit endpoints and
/// explicit credentials.
fn fixture_models() -> Vec<FixtureModel> {
    vec![
        FixtureModel::text("alpha/model-a", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(128_000)
            .with_max_output_tokens(4_096)
            .with_request_params(serde_json::json!({"temperature": 0.2}))
            .with_reasoning(serde_json::json!({
                "defaultProfile": "on",
                "profiles": {
                    "off": {"enabled": false, "requestParams": {"thinking": {"type": "disabled"}}},
                    "on": {"enabled": true, "requestParams": {"thinking": {"type": "enabled"}}}
                }
            }))
            // A generous raw claim, so the effective intersection is visibly
            // narrower than the declaration.
            .claiming_input("image"),
        FixtureModel::text("beta/model-b", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(32_000)
            .with_max_output_tokens(2_048),
        FixtureModel::text(
            "summary/summary-model",
            ModelProtocol::OpenAiChatCompletions,
        )
        .with_context_window(16_000)
        .with_max_output_tokens(1_024),
        FixtureModel::text("always/always-on", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(64_000)
            .with_max_output_tokens(512)
            .always_on_reasoning(),
    ]
}

fn model_ref(value: &str) -> ModelRef {
    ModelRef::parse(value).expect("valid fixture reference")
}

/// Builds the session model authority over the fixture catalog, with one
/// scripted adapter shared by every provider.
fn session_model(scripts: Vec<Vec<FakeStep>>) -> (Arc<FakeModel>, SessionModelState) {
    let handle = Arc::new(FakeModel::new(scripts));
    let adapter: Arc<dyn ModelAdapter> = Arc::clone(&handle) as Arc<dyn ModelAdapter>;
    let factory = MappedAdapterFactory::new(move |_provider: &ProviderId, _protocol| {
        Some(Arc::clone(&adapter))
    });
    let catalog = ModelCatalog::from_document(fixture_catalog_document(&fixture_models()))
        .expect("the fixture catalog validates");
    let resolved = catalog
        .resolve(&MapCredentialEnvironment::default())
        .expect("literal fixture credentials resolve");
    let registry = ModelBindingRegistry::new_with_scripted_adapters(resolved, &factory)
        .expect("bindings resolve");
    let state =
        SessionModelState::new(registry, SessionModelConfig::of(model_ref("alpha/model-a")))
            .expect("the initial selection resolves");
    (handle, state)
}

async fn runtime(scripts: Vec<Vec<FakeStep>>) -> (Arc<FakeModel>, RuntimeClientHost) {
    let (handle, model) = session_model(scripts);
    let host = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-42-rc")
        .session_model(model)
        .context_policy(NO_COMPACTION)
        .build()
        .await
        .into_parts()
        .1;
    (handle, host)
}

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

/// `initialize` returns a snapshot whose session model section is complete
/// and redacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_initialize_snapshot_carries_the_redacted_session_model() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (_attachment, result) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized { snapshot, .. } = result else {
        panic!("initialize returns the initial snapshot");
    };

    let model = &snapshot.model;
    assert_eq!(model.configured.model, model_ref("alpha/model-a"));
    assert_eq!(
        model.effective.protocol,
        ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(model.effective.context_window, 128_000);
    assert_eq!(model.effective.max_output_tokens, 4_096);
    assert_eq!(
        model.effective.reasoning_profile,
        Some(ReasoningProfileId::new("on")),
        "the model's declared default profile is selected"
    );
    assert!(model.effective.reasoning_enabled);
    assert_eq!(model.summary, SummaryModelView::Session);

    // The effective capabilities are the intersection, not the raw claim.
    assert!(
        model
            .effective
            .declared_capabilities
            .input_modalities
            .contains(&rustx::model::Modality::Image),
        "the catalog claim is preserved for explanation"
    );
    assert!(
        !model
            .effective
            .capabilities
            .input_modalities
            .contains(&rustx::model::Modality::Image),
        "image input is never advertised while no adapter can transmit it"
    );

    // The effective request parameters are provider-owned config and carry no
    // credential material.
    assert_eq!(
        model.effective.request_params["temperature"],
        serde_json::json!(0.2)
    );
    assert_eq!(
        model.effective.request_params["thinking"],
        serde_json::json!({"type": "enabled"})
    );

    let serialized = serde_json::to_string(&snapshot).expect("the snapshot serializes");
    assert!(
        !serialized.contains(FIXTURE_SECRET),
        "no credential may appear in the snapshot"
    );
    assert!(!serialized.contains("apiKey") && !serialized.contains("api_key"));
}

/// `model_catalog_get` exposes exactly the data a client needs to select a
/// model and a profile, and never a credential value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_catalog_query_exposes_safe_selectable_models() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::ModelCatalogGet {
        id: RequestId::new(1),
    });
    let Some(RuntimeClientResult::ModelCatalog { catalog }) = response.result else {
        panic!("model_catalog_get returns the public catalog: {response:?}");
    };

    let references: Vec<String> = catalog
        .models
        .iter()
        .map(|model| model.model.to_string())
        .collect();
    assert_eq!(
        references,
        vec![
            "alpha/model-a",
            "always/always-on",
            "beta/model-b",
            "summary/summary-model",
        ],
        "every selectable model is listed in deterministic reference order"
    );

    let primary = &catalog.models[0];
    assert_eq!(primary.protocol, ModelProtocol::OpenAiChatCompletions);
    assert_eq!(primary.context_window, 128_000);
    assert_eq!(primary.max_output_tokens, 4_096);
    assert_eq!(
        primary.default_reasoning_profile,
        Some(ReasoningProfileId::new("on"))
    );
    let profiles: Vec<(String, bool)> = primary
        .reasoning_profiles
        .iter()
        .map(|profile| (profile.id.to_string(), profile.enabled))
        .collect();
    assert_eq!(
        profiles,
        vec![("off".to_owned(), false), ("on".to_owned(), true)],
        "profile identities and their semantic enabled state are exposed"
    );
    assert!(
        primary
            .declared_capabilities
            .input_modalities
            .contains(&rustx::model::Modality::Image)
            && !primary
                .effective_capabilities
                .input_modalities
                .contains(&rustx::model::Modality::Image),
        "the view distinguishes the raw claim from the effective capability"
    );
    // The credential *source kind* is safe; the value never is.
    assert_eq!(primary.credential_source, CredentialSourceView::Literal);

    let serialized = serde_json::to_string(&catalog).expect("the catalog view serializes");
    assert!(!serialized.contains(FIXTURE_SECRET));
    assert!(
        !serialized.contains("baseUrl") && !serialized.contains("base_url"),
        "provider endpoints are not part of the selectable-model contract"
    );
    assert!(
        !serialized.contains("requestParams") && !serialized.contains("compat"),
        "adapter internals and compat objects stay out of the catalog view"
    );
}

/// A valid `model_set` publishes exactly one coherent model-configuration
/// change, and the result and the snapshot agree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_valid_update_publishes_exactly_one_coherent_change() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    let desired = SessionModelConfig {
        reasoning_profile: Some(ReasoningProfileId::new("off")),
        request_params: common::request_params(serde_json::json!({"top_k": 40})),
        max_output_tokens: Some(1_000),
        ..SessionModelConfig::of(model_ref("alpha/model-a"))
    };
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(1),
        config: Box::new(desired.clone()),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = response.result else {
        panic!("model_set returns the session state: {response:?}");
    };
    let model = *model;
    assert_eq!(model.configured, desired);
    assert_eq!(model.effective.max_output_tokens, 1_000);
    assert!(!model.effective.reasoning_enabled);
    assert_eq!(
        model.effective.request_params["thinking"],
        serde_json::json!({"type": "disabled"}),
        "the selected profile's parameters replaced the previous profile's"
    );
    assert_eq!(
        model.effective.request_params["top_k"],
        serde_json::json!(40)
    );
    assert_eq!(
        model.effective.request_params["temperature"],
        serde_json::json!(0.2),
        "model defaults survive under the profile and session overlays"
    );

    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. })
    })
    .await;
    let changes = events
        .iter()
        .filter(|event| matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. }))
        .count();
    assert_eq!(changes, 1, "exactly one model-configuration change");
    let RuntimeClientEvent::SessionModelChanged { model: published } =
        &events.last().expect("an event").event
    else {
        panic!("the terminal event is the model change");
    };
    assert_eq!(**published, model, "the event carries the same state");

    // The snapshot folds to the same value.
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.model, model);
    let serialized = serde_json::to_string(&events).expect("events serialize");
    assert!(!serialized.contains(FIXTURE_SECRET));
}

/// `model_get` returns the authoritative session state, and switching models
/// updates every derived field coherently.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_get_returns_the_authoritative_session_state() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");

    let read = |id: u64| {
        let response = attachment.handle_request(RuntimeClientRequest::ModelGet {
            id: RequestId::new(id),
        });
        match response.result {
            Some(RuntimeClientResult::Model { model }) => *model,
            other => panic!("model_get returns the session model: {other:?}"),
        }
    };

    let before = read(1);
    assert_eq!(before.configured.model, model_ref("alpha/model-a"));
    assert_eq!(before.effective.context_window, 128_000);

    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(SessionModelConfig::of(model_ref("beta/model-b"))),
    });
    assert!(response.error.is_none(), "{response:?}");

    let after = read(3);
    assert_eq!(after.configured.model, model_ref("beta/model-b"));
    assert_eq!(
        after.effective.context_window, 32_000,
        "the context window follows the selected model"
    );
    assert_eq!(after.effective.max_output_tokens, 2_048);
    assert_eq!(
        after.effective.reasoning_profile, None,
        "a model that declares no profiles selects none"
    );
    assert!(!after.effective.reasoning_enabled);
}

/// The TUI `/model X` operation sends a complete replacement: primary
/// overrides reset to the selected model defaults while an independent summary
/// policy remains exactly the one the authoritative session already held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn primary_model_selection_resets_primary_overrides_and_preserves_summary_policy() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");

    let summary_policy = rustx::model::session::SummaryModelPolicy::Explicit {
        model: model_ref("summary/summary-model"),
        reasoning_profile: None,
        request_params: common::request_params(serde_json::json!({"summary_tag": "keep"})),
        max_output_tokens: Some(300),
    };
    let initial = SessionModelConfig {
        model: model_ref("alpha/model-a"),
        reasoning_profile: Some(ReasoningProfileId::new("off")),
        request_params: common::request_params(serde_json::json!({"top_k": 40})),
        max_output_tokens: Some(1_000),
        summary_model: summary_policy.clone(),
    };
    let initial_response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(1),
        config: Box::new(initial),
    });
    assert!(initial_response.error.is_none(), "{initial_response:?}");

    let selected = SessionModelConfig {
        model: model_ref("beta/model-b"),
        reasoning_profile: None,
        request_params: rustx::model::RequestParams::new(),
        max_output_tokens: None,
        summary_model: summary_policy.clone(),
    };
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(selected.clone()),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = response.result else {
        panic!("primary selection returns the replacement: {response:?}");
    };

    assert_eq!(model.configured, selected);
    assert_eq!(model.effective.model, model_ref("beta/model-b"));
    assert_eq!(model.effective.reasoning_profile, None);
    assert_eq!(model.effective.max_output_tokens, 2_048);
    assert!(model.effective.request_params.is_empty());
    let SummaryModelView::Explicit(summary) = model.summary else {
        panic!("the explicit summary policy survives the primary switch");
    };
    assert_eq!(summary.model, model_ref("summary/summary-model"));
    assert_eq!(summary.max_output_tokens, 300);
    assert_eq!(
        summary.request_params["summary_tag"],
        serde_json::json!("keep")
    );

    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.model.configured, selected);
    assert_eq!(snapshot.model.summary, SummaryModelView::Explicit(summary));
}

/// Runtime Client model projections preserve always-on reasoning as enabled
/// without inventing a selectable profile or a provider wire field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_client_reports_always_on_reasoning_without_a_profile() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(1),
        config: Box::new(SessionModelConfig::of(model_ref("always/always-on"))),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = response.result else {
        panic!("model_set returns the session model: {response:?}");
    };
    assert_eq!(model.effective.reasoning_profile, None);
    assert!(model.effective.reasoning_enabled);

    let catalog_response = attachment.handle_request(RuntimeClientRequest::ModelCatalogGet {
        id: RequestId::new(2),
    });
    let Some(RuntimeClientResult::ModelCatalog { catalog }) = catalog_response.result else {
        panic!("model_catalog_get returns the catalog: {catalog_response:?}");
    };
    let always_on = catalog
        .models
        .iter()
        .find(|model| model.model == model_ref("always/always-on"))
        .expect("always-on model is listed");
    assert!(always_on.reasoning_profiles.is_empty());
    assert_eq!(always_on.default_reasoning_profile, None);
}

/// A reconnecting client recovers the complete model state from the
/// authoritative snapshot alone — no client-side file read, no replay of the
/// update it missed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnecting_client_recovers_model_state_from_the_snapshot() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (first, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let desired = SessionModelConfig {
        max_output_tokens: Some(777),
        ..SessionModelConfig::of(model_ref("beta/model-b"))
    };
    assert!(
        first
            .handle_request(RuntimeClientRequest::ModelSet {
                id: RequestId::new(1),
                config: Box::new(desired.clone()),
            })
            .error
            .is_none()
    );
    // The client disconnects without ever having subscribed.
    drop(first);

    let (_second, result) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("re-attach");
    let RuntimeClientResult::Initialized { snapshot, .. } = result else {
        panic!("initialize returns the snapshot");
    };
    assert_eq!(snapshot.model.configured, desired);
    assert_eq!(snapshot.model.effective.max_output_tokens, 777);
    assert_eq!(snapshot.model.effective.context_window, 32_000);
}

/// The attempt read model carries the model the attempt was admitted with for
/// the attempt's whole lifetime, including after it settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_attempt_view_reports_the_model_it_was_admitted_with() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    assert!(
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: RequestId::new(1),
                content: text("hello"),
            })
            .error
            .is_none()
    );
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    // Switch the session after the attempt settled.
    assert!(
        attachment
            .handle_request(RuntimeClientRequest::ModelSet {
                id: RequestId::new(2),
                config: Box::new(SessionModelConfig::of(model_ref("beta/model-b"))),
            })
            .error
            .is_none()
    );

    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.model.configured.model, model_ref("beta/model-b"));
    let attempt = snapshot.attempt.as_ref().expect("the latest attempt");
    assert_eq!(
        attempt.model.primary.model,
        model_ref("alpha/model-a"),
        "the settled attempt still reports the model it ran with"
    );
    assert_eq!(attempt.model.summary, SummaryModelView::Session);
}

/// The incremental A -> B invariant, proven from the event stream alone.
///
/// A continuously subscribed client must be able to answer "which model is
/// the running attempt actually using" without a second `snapshot_get` and
/// without inferring anything from ordering. `attempt_started` therefore
/// carries the frozen attempt model:
///
/// ```text
/// session = A ; attempt admitted -> attempt_started(model = A)
/// model_set(B) accepted while the attempt runs
///     -> session_model_changed(B), and no second attempt_started for A
/// next attempt admitted    -> attempt_started(model = B)
/// ```
///
/// Synchronization is the scripted model's park barrier and the observation
/// stream itself; nothing here sleeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempt_started_freezes_the_model_across_a_mid_attempt_switch() {
    let (release, release_rx) = support::fake::model_release();
    let (model, host) = runtime(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
        one_turn_stop(),
    ])
    .await;
    let (attachment, _) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    // The first attempt is admitted while the session model is A.
    assert!(
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: RequestId::new(1),
                content: text("first"),
            })
            .error
            .is_none()
    );
    let observed = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    let Some(RuntimeClientEvent::AttemptStarted {
        attempt_id: first_attempt,
        model: first_model,
    }) = observed.last().map(|event| event.event.clone())
    else {
        panic!("the first attempt started");
    };
    assert_eq!(
        first_model.primary.model,
        model_ref("alpha/model-a"),
        "the start event carries the model the attempt froze at admission"
    );

    // The model is parked, so the set of publications so far is fixed: the
    // switch below cannot race with an unobserved attempt transition.
    support::runtime_client_conformance::await_model_parked(&model).await;

    // Switch the session to B *while* the attempt runs.
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(SessionModelConfig::of(model_ref("beta/model-b"))),
    });
    let Some(RuntimeClientResult::ModelSet { model: desired }) = response.result else {
        panic!("the update is accepted while an attempt runs: {response:?}");
    };
    assert_eq!(desired.configured.model, model_ref("beta/model-b"));

    // The switch publishes exactly one session observation and never a
    // second start for the running attempt.
    let switch = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. })
    })
    .await;
    let RuntimeClientEvent::SessionModelChanged { model: published } =
        switch.last().expect("the change").event.clone()
    else {
        panic!("the session model change is published");
    };
    assert_eq!(published.configured.model, model_ref("beta/model-b"));
    assert_eq!(
        switch
            .iter()
            .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptStarted { .. }))
            .count(),
        0,
        "the running attempt never restarts and never re-announces a model"
    );

    // The running attempt is still, truthfully, on A.
    let (during, _) = host.snapshot().expect("snapshot");
    let attempt = during.attempt.as_ref().expect("the running attempt");
    assert_eq!(attempt.attempt_id, first_attempt);
    assert_eq!(
        attempt.model.primary.model,
        model_ref("alpha/model-a"),
        "desired session model = B, active attempt model = A"
    );
    assert_eq!(during.model.configured.model, model_ref("beta/model-b"));

    release.send_replace(true);
    let settled = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert!(matches!(
        settled.last().expect("settlement").event,
        RuntimeClientEvent::AttemptSettled { .. }
    ));

    // The next admission uses B, announced on the same self-contained event.
    assert!(
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: RequestId::new(3),
                content: text("second"),
            })
            .error
            .is_none()
    );
    let next = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    let Some(RuntimeClientEvent::AttemptStarted {
        attempt_id: second_attempt,
        model: second_model,
    }) = next.last().map(|event| event.event.clone())
    else {
        panic!("the second attempt started");
    };
    assert_ne!(second_attempt, first_attempt);
    assert_eq!(
        second_model.primary.model,
        model_ref("beta/model-b"),
        "the next attempt freezes the model the session moved to"
    );
    assert_eq!(second_model.primary.context_window, 32_000);
}

/// Model requests before `initialize` are rejected like every other method:
/// the model contract adds no out-of-band path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_methods_require_an_admitted_attachment() {
    let (_model, host) = runtime(vec![one_turn_stop()]).await;
    let endpoint = rustx::runtime_client::RuntimeClientEndpoint::new(host);
    for request in [
        RuntimeClientRequest::ModelCatalogGet {
            id: RequestId::new(1),
        },
        RuntimeClientRequest::ModelGet {
            id: RequestId::new(2),
        },
        RuntimeClientRequest::ModelSet {
            id: RequestId::new(3),
            config: Box::new(SessionModelConfig::of(model_ref("alpha/model-a"))),
        },
    ] {
        let response = endpoint.handle_request(request);
        assert!(
            matches!(response.error, Some(RuntimeClientError::NotAttached)),
            "{response:?}"
        );
    }
}

/// The new methods round-trip through the wire contract with their stable
/// discriminators, and unknown fields are still rejected.
#[test]
fn the_new_methods_round_trip_on_the_wire() {
    let cases = [
        (
            RuntimeClientRequest::ModelCatalogGet {
                id: RequestId::new(1),
            },
            "model_catalog_get",
        ),
        (
            RuntimeClientRequest::ModelGet {
                id: RequestId::new(2),
            },
            "model_get",
        ),
        (
            RuntimeClientRequest::ModelSet {
                id: RequestId::new(3),
                config: Box::new(SessionModelConfig::of(model_ref("alpha/model-a"))),
            },
            "model_set",
        ),
    ];
    for (request, method) in cases {
        assert_eq!(request.method(), method);
        let value = serde_json::to_value(&request).expect("serialize");
        assert_eq!(value["method"], method);
        let decoded: RuntimeClientRequest = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, request);
    }

    let unknown_field = r#"{"method":"model_set","id":1,"config":{"model":"a/b"},"extra":true}"#;
    assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_field).is_err());
    let unknown_config_field =
        r#"{"method":"model_set","id":1,"config":{"model":"a/b","future":1}}"#;
    assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_config_field).is_err());
}
