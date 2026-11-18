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
async fn permission_review_reaches_provider_and_settles_after_completion_or_cancellation() {
    use crate::runtime::permission::review::{
        AuxiliaryPermissionReviewer, PermissionReviewPort, PermissionReviewVerdict,
    };
    for (native, cancel) in [(false, false), (false, true), (true, false), (true, true)] {
        let (fixture, adapter, mut entries, _) = structured_fixture(
            native,
            vec![completion(
                r#"{"decision":"allow","explanation":"requested"}"#,
            )],
            AuxiliaryOutputFormat::Text,
        );
        drop(fixture.reservation);
        let cancellation = CancellationSignal::new();
        let reviewer = AuxiliaryPermissionReviewer::new(
            fixture.executor.clone(),
            fixture.frozen.invocation.clone(),
            ModelTimeoutPolicy::default(),
            7,
            cancellation.child(),
        );
        let conversation_id = ConversationId::generate();
        let attempt_id = crate::runtime::identity::AttemptId::new("review-attempt");
        let invocation_id = crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new("review-call"),
        };
        let tool_id = crate::runtime::identity::ToolId::new("write");
        let origin = crate::tools::types::ToolOrigin::Builtin;
        // Exercise both output strategies with escaped and multibyte content
        // beyond the old combined prompt limit.
        let content = format!("{}END OF FILE", "水\n\"".repeat(5_000));
        let arguments = serde_json::json!({"path": "notes.txt", "content": content});
        let audit_preview = serde_json::json!({"content": "preview"});
        let task = crate::runtime::permission::PermissionTaskEvidence::canonical(
            &MessageBlock::User(UserMessageBlock {
                id: MessageId::new("task"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "Write notes.txt".into(),
                })],
            }),
        )
        .unwrap();
        let oversized = serde_json::json!({"path": "notes.txt", "content": "a".repeat(65_536)});
        let mut view = crate::agent::lifecycle::PreToolView {
            task: Some(&task),
            conversation_id: &conversation_id,
            execution: crate::runtime::interaction::InteractionExecution::Attempt {
                attempt_id,
                turn: 1,
            },
            invocation_id: &invocation_id,
            tool_id: &tool_id,
            tool_name: "write",
            origin: &origin,
            mode: crate::tools::types::ToolInvocationMode::Foreground,
            arguments: &oversized,
            audit_arguments: &audit_preview,
            elevation_reason: None,
            workspace_root: Some(std::path::Path::new("/workspace")),
            mutation_target: Some(std::path::Path::new("/workspace/notes.txt")),
            resource_permission: crate::runtime::permission::ResourcePermissionValue::Default,
        };
        let rejected = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            reviewer.review(&view, "workspace write", &cancellation),
        )
        .await
        .unwrap();
        assert!(matches!(rejected, PermissionReviewVerdict::Failed { .. }));
        assert!(adapter.requests.lock().unwrap().is_empty());
        view.arguments = &arguments;
        let review = reviewer.review(&view, "workspace write", &cancellation);
        tokio::pin!(review);
        tokio::select! {
            result = &mut review => panic!("review settled before provider entry: {result:?}"),
            _ = entered(&mut entries) => {}
        }
        {
            let requests = adapter.requests.lock().unwrap();
            let ModelInputMessage::RequestOnly(
                crate::model::RequestOnlyModelContext::AuxiliaryQuestion { text },
            ) = &requests[0].messages[0]
            else {
                panic!("expected the frozen review question")
            };
            let (_, encoded) = text.split_once("native_facts_json: ").unwrap();
            let facts: serde_json::Value = serde_json::from_str(encoded).unwrap();
            assert_eq!(facts["invocation"]["business_arguments"], arguments);
            assert_eq!(
                facts["invocation"]["mutation_target"],
                "/workspace/notes.txt"
            );
        }
        if cancel {
            cancellation.cancel();
        } else {
            adapter.release.notify_one();
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), review)
            .await
            .unwrap();
        if cancel {
            assert!(matches!(result, PermissionReviewVerdict::Failed { .. }));
        } else {
            assert!(
                matches!(result, PermissionReviewVerdict::Allow { .. }),
                "{result:?}"
            );
        }
        assert!(adapter.exited.load(Ordering::SeqCst));
        assert!(!fixture.executor.with_activity(|active| active).unwrap());
    }
}

struct CallbackConsumer {
    visible_limit: usize,
    registered: AtomicUsize,
    attempts: AtomicUsize,
    stopped: AtomicUsize,
    text: Mutex<String>,
    accepted: Mutex<Option<Result<AuxiliaryOutput, crate::model::ModelError>>>,
}

impl CallbackConsumer {
    fn new(visible_limit: usize) -> Self {
        Self {
            visible_limit,
            registered: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            stopped: AtomicUsize::new(0),
            text: Mutex::default(),
            accepted: Mutex::default(),
        }
    }
}

impl AuxiliaryConsumer for CallbackConsumer {
    fn registered(&self) {
        self.registered.fetch_add(1, Ordering::SeqCst);
    }
    fn output_limit(&self) -> usize {
        self.visible_limit
    }
    fn generation_limit(&self) -> u64 {
        262_144
    }
    fn stopped(&self) {
        self.stopped.fetch_add(1, Ordering::SeqCst);
    }
    fn attempt(&self, _: &RequestId, _: u32) {
        self.attempts.fetch_add(1, Ordering::SeqCst);
    }
    fn text(&self, _: &RequestId, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    fn settled(&self, result: &Result<ConfirmedExecution, ContextError>) {
        *self.accepted.lock().unwrap() = Some(result.as_ref().unwrap().output.clone());
    }
    fn retired(&self, _: &Result<ConfirmedExecution, ContextError>) {}
}

#[tokio::test(flavor = "current_thread")]
async fn structured_consumers_expose_only_validated_settled_values_through_delivery_hooks() {
    for (structured, text) in [
        (false, "ordinary text"),
        (true, r#"{"label":"ok"}"#),
        (true, "invalid JSON"),
    ] {
        let format = if structured {
            AuxiliaryOutputFormat::JsonSchema(schema(128))
        } else {
            AuxiliaryOutputFormat::Text
        };
        let (fixture, adapter, mut entries, _) =
            structured_fixture(false, vec![completion(text)], format);
        let consumer = Arc::new(CallbackConsumer::new(256));
        let reply = fixture
            .reservation
            .with_consumer(consumer.clone())
            .submit(fixture.frozen)
            .unwrap();
        entered(&mut entries).await;
        let gates = fixture.executor.hold_settlement_for_test();
        adapter.release.notify_one();
        gates.committed.await.unwrap();
        assert_eq!(
            *consumer.text.lock().unwrap(),
            if structured { "" } else { text }
        );
        assert!(
            consumer.accepted.lock().unwrap().is_none(),
            "unconfirmed settlement cannot publish the typed result"
        );
        drop(gates.release_ack);
        gates.confirming.await.unwrap();
        assert!(consumer.accepted.lock().unwrap().is_none());
        gates.release_confirmation.send(()).unwrap();
        let completed = reply.await.unwrap().unwrap();
        assert_eq!(
            consumer.accepted.lock().unwrap().as_ref().unwrap(),
            &completed.output
        );
        assert_eq!(completed.output.is_ok(), text != "invalid JSON");
        assert_eq!(consumer.registered.load(Ordering::SeqCst), 1);
        assert_eq!(consumer.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(consumer.stopped.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn structured_schema_and_consumer_visible_limits_both_remain_enforced() {
    for (schema_limit, consumer_limit) in [(8, 128), (128, 8)] {
        let (fixture, adapter, mut entries, _) = structured_fixture(
            false,
            vec![completion(r#"{"label":"ok"}"#)],
            AuxiliaryOutputFormat::JsonSchema(schema(schema_limit)),
        );
        let consumer = Arc::new(CallbackConsumer::new(consumer_limit));
        let reply = fixture
            .reservation
            .with_consumer(consumer.clone())
            .submit(fixture.frozen)
            .unwrap();
        entered(&mut entries).await;
        adapter.release.notify_one();
        assert_eq!(
            reply.await.unwrap().unwrap().output.unwrap_err().kind,
            ModelErrorKind::GenerationBudgetExceeded
        );
        assert!(consumer.text.lock().unwrap().is_empty());
        assert!(consumer.accepted.lock().unwrap().as_ref().unwrap().is_err());
    }
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
        assert_eq!(
            fixture.store.read_events(None, 32).unwrap().events,
            [] as [crate::events::types::RuntimeEventEnvelope; 0]
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
    assert_eq!(
        store.load_canonical().unwrap(),
        [] as [crate::message::types::MessageBlock; 0]
    );
}
