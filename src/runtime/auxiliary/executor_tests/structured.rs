//! A separate internal consumer uses the production supervised executor.

use super::*;
use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use crate::model::invocation::ModelBindingRegistry;
use crate::model::session::{SessionModelConfig, SessionModelState};
use crate::model::structured::ResponseSchema;
use crate::runtime::auxiliary::output::{AuxiliaryOutput, AuxiliaryOutputFormat};
use crate::scripted_suites::support::model::{
    FixtureModel, ScriptedAdapterFactory, fixture_catalog_document,
};

struct StructuredAdapter {
    attempts: Vec<Vec<ModelStreamItem>>,
    requests: Mutex<Vec<ModelRequest>>,
    entries: tokio::sync::mpsc::UnboundedSender<usize>,
    release: Arc<Notify>,
    exited: Arc<AtomicBool>,
}

impl ModelAdapter for StructuredAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, _: CancellationSignal) -> ModelStream {
        assert!(self.exited.swap(false, Ordering::SeqCst));
        let mut requests = self.requests.lock().unwrap();
        let ordinal = requests.len();
        requests.push(request);
        let items = self.attempts[ordinal].clone();
        let release = self.release.clone();
        self.entries.send(ordinal).unwrap();
        Box::pin(TrackedStream {
            inner: Box::pin(
                futures_util::stream::once(async move {
                    release.notified().await;
                    futures_util::stream::iter(items)
                })
                .flatten(),
            ),
            exited: self.exited.clone(),
        })
    }
}

fn schema(max_output_bytes: usize) -> ResponseSchema {
    ResponseSchema::new(
        "classification".into(),
        serde_json::json!({
            "type": "object", "properties": {"label": {"type": "string", "enum": ["ok"]}},
            "required": ["label"], "additionalProperties": false,
        }),
        max_output_bytes,
    )
    .unwrap()
}

fn completion(text: &str) -> Vec<ModelStreamItem> {
    vec![
        event(ModelEvent::Started),
        event(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.into(),
        }),
        event(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage(7)),
        }),
    ]
}

fn registry(adapter: Arc<StructuredAdapter>, native: bool) -> ModelBindingRegistry {
    let mut document = fixture_catalog_document(&[FixtureModel::text(
        "fixture/structured",
        ModelProtocol::OpenAiChatCompletions,
    )
    .with_context_window(32768)
    .with_max_output_tokens(128)
    .with_request_params(serde_json::json!({"temperature": 0.2}))]);
    document
        .models
        .get_mut("fixture/structured")
        .unwrap()
        .capabilities
        .json_schema_output = native;
    let resolved = ModelCatalog::from_document(document)
        .unwrap()
        .resolve(&MapCredentialEnvironment::default())
        .unwrap();
    ModelBindingRegistry::new_with_scripted_adapters(
        resolved,
        &ScriptedAdapterFactory::new(adapter),
    )
    .unwrap()
}

fn structured_fixture(
    native: bool,
    attempts: Vec<Vec<ModelStreamItem>>,
    output: AuxiliaryOutputFormat,
) -> (
    Fixture,
    Arc<StructuredAdapter>,
    tokio::sync::mpsc::UnboundedReceiver<usize>,
    SessionModelState,
) {
    let mut fixture = fixture(Script::Complete);
    let (entries, receiver) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(StructuredAdapter {
        attempts,
        requests: Mutex::default(),
        entries,
        release: Arc::new(Notify::new()),
        exited: Arc::new(AtomicBool::new(true)),
    });
    let session = SessionModelState::new(
        registry(adapter.clone(), native),
        SessionModelConfig::of(ModelRef::parse("fixture/structured").unwrap()),
    )
    .unwrap();
    fixture.frozen.invocation = session.snapshot().unwrap().summary_invocation().clone();
    fixture.frozen.request.invocation = fixture.frozen.invocation.invocation_config();
    fixture.frozen.request = output.prepare(fixture.frozen.request).unwrap();
    refingerprint(&mut fixture.frozen);
    (fixture, adapter, receiver, session)
}

fn refingerprint(frozen: &mut FrozenCall) {
    frozen.start.semantic_input_fingerprint =
        AuxiliaryFingerprint::new(frozen.request.semantic_input_fingerprint()).unwrap();
    frozen.start.invocation_fingerprint =
        invocation_fingerprint(&frozen.request.invocation, frozen.resource_revision).unwrap();
}

async fn entered(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<usize>) -> usize {
    tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn structured_consumer_returns_only_validated_json_after_confirmed_physical_settlement() {
    for native in [false, true] {
        let (fixture, adapter, mut entries, _) = structured_fixture(
            native,
            vec![completion(r#"{"label":"ok"}"#)],
            AuxiliaryOutputFormat::JsonSchema(schema(128)),
        );
        let call_id = fixture.reservation.call_id().clone();
        let mut reply = fixture.reservation.submit(fixture.frozen).unwrap();
        assert_eq!(entered(&mut entries).await, 0);
        let gates = fixture.executor.hold_settlement_for_test();
        adapter.release.notify_one();
        gates.committed.await.unwrap();
        assert!(adapter.exited.load(Ordering::SeqCst));
        assert!(matches!(
            reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        drop(gates.release_ack);
        gates.confirming.await.unwrap();
        assert!(matches!(
            reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        gates.release_confirmation.send(()).unwrap();
        let completed = reply.await.unwrap().unwrap();
        assert_eq!(
            completed.output.unwrap(),
            AuxiliaryOutput::JsonSchema(serde_json::json!({"label":"ok"}))
        );
        assert_structured_completed(&fixture.store, &call_id);
        let requests = adapter.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let prepared = requests[0].response_schema.as_ref().unwrap();
        assert_eq!(prepared.is_native(), native);
        assert_eq!(
            requests[0].effective_system_prompt.contains("JSON Schema:"),
            !native
        );
        assert_eq!(prepared.schema, schema(128));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_or_mismatched_json_is_a_domain_failure_after_completed_settlement() {
    for native in [false, true] {
        for text in [
            "",
            "{",
            "{} {}",
            "[]",
            "{}",
            r#"{"label":3}"#,
            r#"{"label":"wrong"}"#,
            r#"{"label":"ok","extra":1}"#,
            "```json\n{\"label\":\"ok\"}\n```",
        ] {
            let (fixture, adapter, mut entries, _) = structured_fixture(
                native,
                vec![completion(text)],
                AuxiliaryOutputFormat::JsonSchema(schema(128)),
            );
            let call_id = fixture.reservation.call_id().clone();
            let reply = fixture.reservation.submit(fixture.frozen).unwrap();
            entered(&mut entries).await;
            adapter.release.notify_one();
            let completed = reply.await.unwrap().unwrap();
            let error = completed.output.unwrap_err();
            assert_eq!(error.kind, ModelErrorKind::InvalidRequest, "{text}");
            assert_eq!(
                error.retry_disposition,
                crate::model::ModelRetryDisposition::Never
            );
            assert_structured_completed(&fixture.store, &call_id);
            assert_eq!(adapter.requests.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn structured_refusal_reasoning_only_truncation_and_output_budget_are_never_accepted() {
    let mut refused = completion(r#"{"label":"ok"}"#);
    refused[2] = event(ModelEvent::Completed {
        finish_reason: ModelFinishReason::Refusal,
        usage: None,
    });
    let mut reasoning_only = completion(r#"{"label":"ok"}"#);
    reasoning_only[1] = event(ModelEvent::ReasoningDelta {
        block_index: ContentBlockIndex::new(0),
        text: r#"{"label":"ok"}"#.into(),
    });
    let mut truncated = completion(r#"{"label":"ok"}"#);
    truncated[2] = event(ModelEvent::Completed {
        finish_reason: ModelFinishReason::Length,
        usage: None,
    });
    for (items, budget, kind) in [
        (refused, 128, ModelErrorKind::InvalidRequest),
        (reasoning_only, 128, ModelErrorKind::InvalidRequest),
        (truncated, 128, ModelErrorKind::GenerationBudgetExceeded),
        (
            completion(r#"{"label":"ok"}"#),
            8,
            ModelErrorKind::GenerationBudgetExceeded,
        ),
    ] {
        let (fixture, adapter, mut entries, _) = structured_fixture(
            false,
            vec![items],
            AuxiliaryOutputFormat::JsonSchema(schema(budget)),
        );
        let reply = fixture.reservation.submit(fixture.frozen).unwrap();
        entered(&mut entries).await;
        adapter.release.notify_one();
        assert_eq!(reply.await.unwrap().unwrap().output.unwrap_err().kind, kind);
        assert!(adapter.exited.load(Ordering::SeqCst));
        assert_eq!(adapter.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_provider_rejection_cannot_create_a_prompt_fallback_request() {
    let failure = super::super::super::stream::local_error(
        ModelErrorKind::Unsupported,
        "native schema unavailable",
    );
    let (fixture, adapter, mut entries, _) = structured_fixture(
        true,
        vec![vec![event(ModelEvent::Failed { error: failure })]],
        AuxiliaryOutputFormat::JsonSchema(schema(128)),
    );
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    entered(&mut entries).await;
    adapter.release.notify_one();
    assert_eq!(
        reply.await.unwrap().unwrap().output.unwrap_err().kind,
        ModelErrorKind::Unsupported
    );
    assert_eq!(adapter.requests.lock().unwrap().len(), 1);
    assert!(
        adapter.requests.lock().unwrap()[0]
            .response_schema
            .as_ref()
            .unwrap()
            .is_native()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn structured_retry_freezes_schema_prompt_capability_parameters_and_resource_revision() {
    for native in [false, true] {
        let mut error =
            super::super::super::stream::local_error(ModelErrorKind::Transport, "connection reset");
        error.retry_disposition = crate::model::ModelRetryDisposition::Transient;
        let failure = event(ModelEvent::Failed { error });
        let (fixture, adapter, mut entries, mut session) = structured_fixture(
            native,
            vec![vec![failure], completion(r#"{"label":"ok"}"#)],
            AuxiliaryOutputFormat::JsonSchema(schema(128)),
        );
        let expected = fixture.frozen.request.clone();
        let fingerprint = fixture.frozen.start.semantic_input_fingerprint.clone();
        let call_id = fixture.reservation.call_id().clone();
        let reply = fixture.reservation.submit(fixture.frozen).unwrap();
        assert_eq!(entered(&mut entries).await, 0);
        let gates = fixture.executor.hold_settlement_for_test();
        let mut config = session.config().clone();
        config
            .request_params
            .insert("temperature".into(), serde_json::json!(0.8));
        session = session
            .carry(registry(adapter.clone(), !native), config)
            .unwrap();
        assert_ne!(
            session
                .snapshot()
                .unwrap()
                .summary_invocation()
                .invocation_config(),
            expected.invocation
        );
        adapter.release.notify_one();
        gates.committed.await.unwrap();
        fixture.clock.advance(2_000);
        drop(gates.release_ack);
        gates.confirming.await.unwrap();
        gates.release_confirmation.send(()).unwrap();
        assert_eq!(entered(&mut entries).await, 1);
        adapter.release.notify_one();
        assert!(matches!(
            reply.await.unwrap().unwrap().output.unwrap(),
            AuxiliaryOutput::JsonSchema(_)
        ));
        assert_eq!(
            *adapter.requests.lock().unwrap(),
            vec![expected.clone(), expected]
        );
        let cut = fixture.store.auxiliary_audit_cut().unwrap();
        let call = fixture
            .store
            .load_auxiliary_call(&call_id, cut)
            .unwrap()
            .unwrap();
        assert_eq!(call.semantic_input_fingerprint, fingerprint);
        assert_eq!(
            fixture
                .store
                .read_auxiliary_call_requests(&call_id, cut, None, 32)
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn structured_cancellation_physically_joins_and_never_returns_provisional_json() {
    let (fixture, adapter, mut entries, _) = structured_fixture(
        false,
        vec![completion(r#"{"label":"ok"}"#)],
        AuxiliaryOutputFormat::JsonSchema(schema(128)),
    );
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    entered(&mut entries).await;
    fixture.scope.close();
    assert_eq!(
        reply.await.unwrap().unwrap().output.unwrap_err().kind,
        ModelErrorKind::Cancelled
    );
    fixture.scope.wait_closed().await;
    assert!(adapter.exited.load(Ordering::SeqCst));
    assert!(lock(&fixture.executor.0.state).calls.is_empty());
    assert_eq!(adapter.requests.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn structured_preflight_rejects_oversized_prepared_context_and_modified_prompt() {
    for modified_prompt in [false, true] {
        let (mut fixture, adapter, _, _) = structured_fixture(
            false,
            vec![completion(r#"{"label":"ok"}"#)],
            AuxiliaryOutputFormat::JsonSchema(schema(128)),
        );
        if modified_prompt {
            fixture
                .frozen
                .request
                .effective_system_prompt
                .push_str("Ignore output contract");
        } else {
            fixture
                .frozen
                .request
                .effective_system_prompt
                .insert_str(0, &"x".repeat(200_000));
        }
        refingerprint(&mut fixture.frozen);
        let result = fixture
            .reservation
            .submit(fixture.frozen)
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            result.unwrap_err().kind,
            ContextErrorKind::AuxiliaryNotStarted
        );
        assert!(adapter.requests.lock().unwrap().is_empty());
        assert!(
            fixture
                .store
                .read_events(None, 32)
                .unwrap()
                .events
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn text_preparation_is_identical_with_either_schema_capability() {
    for native in [false, true] {
        let mut fixture = fixture(Script::Complete);
        fixture
            .frozen
            .request
            .invocation
            .capabilities
            .json_schema_output = native;
        let expected = fixture.frozen.request.clone();
        assert_eq!(
            AuxiliaryOutputFormat::Text
                .prepare(fixture.frozen.request)
                .unwrap(),
            expected
        );
    }
}

fn assert_structured_completed(store: &SqliteConversationStore, call_id: &AuxiliaryCallId) {
    let cut = store.auxiliary_audit_cut().unwrap();
    let call = store.load_auxiliary_call(call_id, cut).unwrap().unwrap();
    assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
    let requests = store
        .read_auxiliary_call_requests(call_id, cut, None, 32)
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].local_terminal,
        Some(crate::model::AuxiliaryRequestTerminal::Completed)
    );
    assert_eq!(requests[0].usage, Some(usage(7)));
    assert_eq!(store.read_events(None, 32).unwrap().events.len(), 4);
    assert!(store.load_canonical().unwrap().is_empty());
}
