//! Idle installation prices the next start's canonical permission context.

use super::*;
use crate::message::types::{ContextKind, UserMessageBlock};
use crate::model::{ModelInputMessage, ModelProtocol};
use crate::runtime::identity::MessageId;
use crate::scripted_suites::support::model::{
    FixtureModel, ScriptedAdapterFactory, fixture_session_model,
};

const OUTPUT: u32 = 256;

fn set_model_window(runtime: &ConversationRuntime, model: &Arc<FakeModel>, window: u64) {
    runtime.inner.lock_state().model = fixture_session_model(
        &[
            FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions)
                .with_context_window(window)
                .with_max_output_tokens(OUTPUT),
        ],
        "scripted/scripted",
        &ScriptedAdapterFactory::new(model.clone()),
    );
}

fn current_input(
    runtime: &ConversationRuntime,
) -> (crate::context::ContextRuntime, crate::model::ModelRequest) {
    let state = runtime.inner.lock_state();
    let model = state.model.snapshot().unwrap();
    let resources = state.resources.clone();
    let context = runtime
        .inner
        .context_runtime_with_assembly(
            &model,
            resources.context_assembly().clone(),
            runtime.inner.model_timeout_policy,
            &resources,
            Arc::new(runtime.inner.auxiliary.clone()),
        )
        .unwrap()
        .with_runtime_resources(&resources);
    let sections = resources
        .context_assembly()
        .system_sections(&resources.native_system_input())
        .unwrap();
    let request = crate::model::ModelRequest {
        prompt_fallback_authorization: crate::model::types::PromptFallbackAuthorization::default(),
        response_schema: None,
        response_byte_limit: None,
        images: BTreeMap::default(),
        invocation: model.primary().invocation_config(),
        messages: crate::model::SelectedModelInput::default(),
        tools: resources.capability().tool_registry().model_definitions(),
        effective_system_prompt: render_effective_system_prompt(&sections),
        continuation: None,
        continuation_source: None,
    };
    (context, request)
}

struct IdleFixture {
    _directory: tempfile::TempDir,
    runtime: ConversationRuntime,
    model: Arc<FakeModel>,
    admission: CompactionAdmission,
    work: Arc<Work>,
    original: Vec<MessageBlock>,
    without_reminder: u64,
    with_reminder: u64,
}

/// Hold idle maintenance while real background generation and physical join
/// settle, then release that slot only after choosing the exact fit boundary.
async fn ready_fixture() -> IdleFixture {
    let directory = tempfile::tempdir().unwrap();
    let original: Vec<_> = ["idle-old-one", "idle-old-two"]
        .into_iter()
        .map(|id| {
            let mut user = runtime_inbound_message(id);
            user.content = text_content(&"old history ".repeat(1_000));
            MessageBlock::User(user)
        })
        .collect();
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![summary_turn_script("idle retained fact"), one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: original.clone(),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    set_model_window(&runtime, &model, 100_000);
    {
        let mut state = runtime.inner.lock_state();
        state.last_permission_reminder_mode = Some(ApprovalMode::FullAccess);
        state.manual_compaction = Some(CurrentManualCompaction {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "idle-fit-maintenance",
            ),
            cancellation: AgentCancellation::new(CancellationReason::RuntimeShutdown),
        });
    }
    runtime.activate();
    let (admission, work, generated) = generate_ready(&runtime).await;
    // Installation must use current authority, not the capture's Full Access mode.
    runtime.inner.lock_state().effective_approval_mode = ApprovalMode::Automatic;
    let (context, mut request) = current_input(&runtime);
    let mut projected = vec![ModelInputMessage::Derived(
        generated.generated_summary().clone(),
    )];
    projected.extend(
        generated
            .source_history()
            .messages
            .iter()
            .skip(generated.retired_count())
            .cloned(),
    );
    request.messages = projected.clone().into();
    let without_reminder = context.engine.estimate_frozen_request(&request);
    projected.push(ModelInputMessage::Canonical(MessageBlock::User(
        UserMessageBlock {
            id: MessageId::new("expected-next-start-reminder"),
            content: crate::context::assembly::permission_reminder_content(
                &crate::runtime::permission::permission_reminder_text(ApprovalMode::Automatic),
            ),
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::NativeEnvironment),
            timestamp: None,
        },
    )));
    request.messages = projected.into();
    let with_reminder = context.engine.estimate_frozen_request(&request);
    assert!(with_reminder > without_reminder);
    IdleFixture {
        _directory: directory,
        runtime,
        model,
        admission,
        work,
        original,
        without_reminder,
        with_reminder,
    }
}

async fn generate_ready(
    runtime: &ConversationRuntime,
) -> (CompactionAdmission, Arc<Work>, Arc<GeneratedCompaction>) {
    let admission = start_background(runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    let generated = within_liveness_guard("published Ready Summary and physical join", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if state.published.as_ref() == Some(&state.status)
                    && work.joined.load(Ordering::Acquire)
                    && let Phase::Ready(generated) = &state.phase
                {
                    break generated.clone();
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;
    (admission, work, generated)
}

async fn install_at_idle(fixture: &IdleFixture) {
    fixture.runtime.inner.lock_state().manual_compaction = None;
    fixture.runtime.inner.install_ready_async_when_idle().await;
    // The ordinary driver may claim maintenance first. In either ordering,
    // observe its durable settlement before inspecting admission or history.
    within_liveness_guard("idle installation settlement", async {
        loop {
            let changed = fixture.work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = fixture.work.state.lock().unwrap();
                if state.published.as_ref() == Some(&state.status)
                    && (matches!(state.phase, Phase::Terminal)
                        || (matches!(state.phase, Phase::Ready(_))
                            && state.status.diagnostic.is_some()))
                {
                    break;
                }
            }
            changed.await;
        }
    })
    .await;
}

fn set_input_limit(fixture: &IdleFixture, limit: u64) {
    let minimum_window = limit + u64::from(OUTPUT);
    let maximum_window = minimum_window + crate::context::policy::automatic_margin(u64::MAX);
    let window = (minimum_window..=maximum_window)
        .find(|window| {
            crate::context::policy::CompactionPolicy::resolve(*window, OUTPUT, 100_000, OUTPUT)
                .is_ok_and(|policy| policy.primary.admissible_input_limit(OUTPUT).unwrap() == limit)
        })
        .unwrap();
    set_model_window(&fixture.runtime, &fixture.model, window);
    let (context, _) = current_input(&fixture.runtime);
    assert_eq!(
        context.engine.admissible_input_limit(OUTPUT).unwrap(),
        limit
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_install_defers_when_rearmed_user_reminder_exceeds_fit_boundary() {
    let fixture = ready_fixture().await;
    let limit = fixture.with_reminder - 1;
    assert!(
        fixture.without_reminder <= limit,
        "omitting User context would incorrectly pass admission"
    );
    set_input_limit(&fixture, limit);
    install_at_idle(&fixture).await;
    assert!(
        fixture
            .runtime
            .inner
            .store
            .projection_head()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture.runtime.inner.store.load_canonical().unwrap(),
        fixture.original
    );
    {
        let state = fixture.work.state.lock().unwrap();
        assert!(
            matches!(state.phase, Phase::Ready(_)),
            "a non-fitting idle result remains lazy Ready"
        );
        assert_eq!(state.status.phase, CompactionOperationPhase::Ready);
        assert_eq!(state.published.as_ref(), Some(&state.status));
        let diagnostic = state.status.diagnostic.as_ref().unwrap();
        assert!(
            diagnostic.contains(&format!(
                "estimated input {}, limit {limit}",
                fixture.with_reminder
            )),
            "{diagnostic}"
        );
    }
    assert_eq!(
        fixture.model.requests().len(),
        1,
        "deferral never invokes another model"
    );
    assert!(!fixture.admission.cancellation.is_cancelled());
    fixture.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_install_prices_rearmed_user_reminder_and_commits_only_checkpoint() {
    let fixture = ready_fixture().await;
    set_input_limit(&fixture, fixture.with_reminder);
    install_at_idle(&fixture).await;
    assert!(
        fixture
            .runtime
            .inner
            .store
            .projection_head()
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fixture.runtime.inner.store.load_canonical().unwrap(),
        fixture.original,
        "idle installation commits no placeholder or canonical reminder"
    );
    assert_eq!(
        fixture
            .runtime
            .inner
            .lock_state()
            .last_permission_reminder_mode,
        None
    );
    let events = fixture
        .runtime
        .inner
        .store
        .read_events(None, 128)
        .unwrap()
        .events;
    let prices: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::CompactionCompleted {
                operation_id,
                estimated_tokens_after,
                ..
            } if operation_id == &fixture.admission.operation_id => Some(*estimated_tokens_after),
            _ => None,
        })
        .collect();
    assert_eq!(
        prices,
        [fixture.with_reminder],
        "the native receipt prices the complete future request"
    );
    assert_eq!(
        fixture.model.requests().len(),
        1,
        "idle installation starts no primary request"
    );

    assert_later_start_owns_reminder(&fixture).await;
    fixture.runtime.shutdown().await.unwrap();
}

async fn assert_later_start_owns_reminder(fixture: &IdleFixture) {
    // A later real start owns allocation and canonical admission of that body.
    set_model_window(&fixture.runtime, &fixture.model, 100_000);
    fixture
        .runtime
        .submit_inbound(text_content("continue after idle installation"))
        .unwrap();
    within_liveness_guard(
        "later primary settlement",
        fixture.runtime.child_attempt_settlement(0),
    )
    .await;
    let snapshots = request_snapshots(&fixture.runtime.request_history());
    assert_eq!(snapshots.len(), 1);
    let reminders: Vec<_> = snapshots[0]
        .contributions
        .iter()
        .filter(|contribution| {
            contribution.producer
                == crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::PermissionReminder,
                )
        })
        .collect();
    assert_eq!(reminders.len(), 1);
    let reminder = reminders[0];
    let canonical = fixture.runtime.inner.store.load_canonical().unwrap();
    assert!(
        canonical
            .iter()
            .all(|message| !message.id().as_str().starts_with("rustx-context-idle-"))
    );
    let message = canonical
        .iter()
        .find(|message| message.id() == &reminder.message_id)
        .unwrap();
    let MessageBlock::User(user) = message else {
        panic!("the actual start admits User context");
    };
    assert_eq!(
        user.kind,
        InboundKind::Context(ContextKind::NativeEnvironment)
    );
    assert_eq!(user.source, UserSource::Runtime);
    assert_eq!(
        user.content,
        crate::context::assembly::permission_reminder_content(
            &crate::runtime::permission::permission_reminder_text(ApprovalMode::Automatic)
        )
    );
    assert_eq!(fixture.model.requests().len(), 2);
    assert_eq!(
        fixture
            .runtime
            .reconstruct_request(&snapshots[0].identity)
            .unwrap(),
        fixture.model.requests()[1]
    );
}
