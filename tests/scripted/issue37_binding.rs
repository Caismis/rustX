//! Issue #37: one `ConversationToolRuntime` identity is bound to at most
//! one `RuntimeClientHost`, structurally.
//!
//! A Runtime Client host is the conversation coordinator over one runtime
//! identity: it owns canonical history, the current-attempt slot, the
//! projection and its cursor domain, attachment state, and the inbound and
//! attempt identity counters. Two hosts over one runtime identity would be
//! two coordinators over one authoritative runtime, and — because each
//! subsystem has one observer slot — the second would silently unhook the
//! first. These tests prove the binding rejects that, that a rejected
//! construction leaves no trace, and that the binding is a lifetime
//! binding rather than a lease.
//!
//! They also pin the companion ownership invariant: the
//! `ConversationToolRuntime` is the *one* conversation authority at this
//! boundary. `RuntimeClientHostConfig` has no conversation id field, so the
//! host derives its identity from the runtime it coordinates and cannot be
//! configured to name a different conversation.
//!
//! All synchronization is exact; no sleep participates in any proof.

use super::support;

use std::path::Path;
use std::sync::Arc;

use rustx::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use rustx::context::{AgentStatusComposer, DefaultTokenEstimator, TokenEstimator};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;

use rustx::runtime::identity::AgentId;
use rustx::runtime_client::{
    HostConstructionError, RuntimeClientContextConfig, RuntimeClientEvent, RuntimeClientHost,
    RuntimeClientHostConfig, RuntimeClientRequest, RuntimeClientResult,
};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::runtime::ConversationToolRuntime;

use support::fake::{FakeModel, FakeStep};

/// One independently constructed runtime bundle: a fresh runtime identity
/// with its own workspace, plus a coordinator over it.
struct Bundle {
    dir: tempfile::TempDir,
    runtime: ConversationToolRuntime,
    coordinator: CapabilityCoordinator,
}

async fn new_bundle(conversation: &str) -> Bundle {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let runtime = ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new(conversation),
        &workspace,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        conversation_id: runtime.conversation_id().clone(),
        workspace: runtime.workspace().clone(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: runtime.environment().clone(),
        environment_store_root: dir.path().join("skill-env"),
    })
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    Bundle {
        dir,
        runtime,
        coordinator,
    }
}

/// Builds a host config over the given runtime bundle handles.
fn config(
    runtime: ConversationToolRuntime,
    coordinator: CapabilityCoordinator,
    model: Arc<FakeModel>,
) -> RuntimeClientHostConfig {
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    RuntimeClientHostConfig {
        agent_id: AgentId::new("agent-a"),
        model: support::model::scripted_session_model(model),
        timezone: None,
        context: RuntimeClientContextConfig {
            policy: rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_composer: AgentStatusComposer::default(),
        },
        tool_runtime: runtime,
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        replay_limit: None,
    }
}

fn one_turn_stop() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

fn write_skill(workspace: &Path, name: &str) {
    let root = workspace.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&root).expect("skill dir");
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: \"a binding probe skill\"\n---\nbody\n"),
    )
    .expect("SKILL.md");
}

/// A background tool execution that never settles on its own: it runs until
/// its own cancellation fires, so the published record stays observable.
struct ParkedBackgroundTool;

impl rustx::tools::executor::ToolExecutor for ParkedBackgroundTool {
    fn execute<'a>(
        &'a self,
        _invocation: rustx::tools::types::ToolInvocation,
        context: rustx::tools::executor::ToolExecutionContext<'a>,
    ) -> futures_util::future::BoxFuture<'a, rustx::tools::types::ToolExecutionResult> {
        Box::pin(async move {
            context.cancellation.cancelled().await;
            rustx::tools::types::ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Cancelled {
                    reason: rustx::runtime::types::CancellationReason::UserRequested,
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            }
        })
    }
}

/// Commits one authoritative background dispatch on the runtime's registry
/// and returns its execution identity. The record is published under the
/// registry's ownership commit, before this returns.
fn dispatch_background(
    runtime: &ConversationToolRuntime,
) -> rustx::runtime::identity::ToolExecutionId {
    let executor: Arc<dyn rustx::tools::executor::ToolExecutor> = Arc::new(ParkedBackgroundTool);
    let invocation = rustx::tools::types::ToolInvocation {
        call_id: rustx::runtime::identity::ToolCallId::new("call-binding-seam"),
        tool_id: rustx::runtime::identity::ToolId::new("tool-binding-seam"),
        tool_name: "binding-seam".to_owned(),
        mode: rustx::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let registry = runtime.background();
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare background dispatch");
    let outcome = registry.commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new());
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("the background dispatch is accepted");
    };
    execution_id
}

fn text(text: &str) -> Vec<rustx::message::types::UserContentBlock> {
    vec![rustx::message::types::UserContentBlock::Text(
        rustx::message::content::TextBlock {
            text: text.to_owned(),
        },
    )]
}

/// Cloning a `ConversationToolRuntime` shares one binding identity: the
/// clone is not a second bindable runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cloning_a_tool_runtime_does_not_create_a_new_binding_identity() {
    let bundle = new_bundle("conv-37-bind-clone").await;
    let clone = bundle.runtime.clone();
    let second_clone = clone.clone();
    assert!(!bundle.runtime.is_runtime_client_bound());
    assert!(!clone.is_runtime_client_bound());

    let _host = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ))
    .expect("the first host binds the runtime identity");

    // Every handle of the same identity observes the binding — including
    // clones taken before the host existed and clones of clones.
    assert!(bundle.runtime.is_runtime_client_bound());
    assert!(clone.is_runtime_client_bound());
    assert!(second_clone.is_runtime_client_bound());
    assert!(bundle.coordinator.is_runtime_client_bound());
}

/// A second host over a clone of the same runtime identity is rejected with
/// the typed error, leaves every observation seam pointing at the first
/// host, and leaves the first host fully operational.
// One rejection observed end to end: splitting it would lose the
// before/after continuity that is the whole point.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_host_over_the_same_runtime_is_rejected_without_side_effects() {
    let bundle = new_bundle("conv-37-bind-reject").await;
    let model = Arc::new(FakeModel::new(vec![one_turn_stop()]));
    let host_a = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        model.clone(),
    ))
    .expect("first host");

    let attachment = host_a
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .0
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    let (baseline, baseline_cursor) = host_a.snapshot().expect("snapshot");

    // The rejected construction, over clones of the very same identities.
    let rejected = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ));
    match rejected {
        Err(HostConstructionError::RuntimeClientAlreadyBound { conversation_id }) => {
            assert_eq!(conversation_id.as_str(), "conv-37-bind-reject");
        }
        Err(other) => panic!("expected already-bound, got {other:?}"),
        Ok(_) => panic!("a second host over one runtime identity must be rejected"),
    }

    // No semantic side effect: the first host's projection did not move.
    let (after_rejection, after_cursor) = host_a.snapshot().expect("snapshot");
    assert_eq!(after_cursor, baseline_cursor, "no event was published");
    assert_eq!(after_rejection.messages, baseline.messages);
    assert_eq!(
        after_rejection.capabilities.revision, baseline.capabilities.revision,
        "the rejected construction never touched capability state"
    );
    assert!(
        after_rejection.inbound.pending.is_empty(),
        "the rejected construction never drained or enqueued inbound"
    );
    assert!(after_rejection.attempt.is_none());

    // Every authoritative seam still reaches host A, so no observer was
    // replaced.
    bundle
        .runtime
        .mailbox()
        .enqueue(rustx::message::types::UserMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-seam"),
            content: text("seam"),
            source: rustx::message::types::UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-13T00:00:00Z")
                    .expect("fixed timestamp")
                    .with_timezone(&chrono::Utc),
            ),
        })
        .expect("enqueue");
    write_skill(&bundle.dir.path().join("workspace"), "binding-skill");
    let candidate = bundle
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let committed = bundle.coordinator.commit(candidate).expect("commit");
    // The background registry transition is published under the registry's
    // ownership commit, so its arrival at the observer is exact.
    let background_id = dispatch_background(&bundle.runtime);

    let (observed, _) = host_a.snapshot().expect("snapshot");
    assert!(
        observed
            .inbound
            .pending
            .iter()
            .any(|item| item.message.id.as_str() == "msg-seam"),
        "the mailbox seam still reaches host A"
    );
    assert_eq!(
        observed.capabilities.revision,
        committed.revision(),
        "the capability seam still reaches host A"
    );
    assert!(
        observed
            .background
            .iter()
            .any(|execution| execution.execution_id == background_id),
        "the background seam still reaches host A"
    );

    // Host A still coordinates execution end to end after the rejection.
    let response = attachment
        .0
        .handle_request(RuntimeClientRequest::SubmitInbound {
            id: rustx::runtime_client::RequestId::new(1),
            content: text("go"),
        });
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));
    loop {
        // Liveness guard only: the delivery wait itself is exact.
        let delivery =
            tokio::time::timeout(std::time::Duration::from_secs(120), subscription.next())
                .await
                .expect("the stream must not stall");
        let rustx::runtime_client::EventDelivery::Event(event) = delivery else {
            panic!("subscription stays open, got {delivery:?}");
        };
        if matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }) {
            break;
        }
    }
    let (settled, settled_cursor) = host_a.snapshot().expect("snapshot");
    assert!(settled_cursor > baseline_cursor);
    assert!(settled.attempt.is_some(), "host A ran the attempt");
    assert_eq!(
        model.requests().len(),
        1,
        "exactly one host drove the model"
    );
}

/// The binding is a lifetime binding, not a lease: dropping the bound host
/// never makes the same runtime identity bindable again. A genuinely fresh
/// runtime identity is required — and is accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_host_never_rebinds_the_runtime_identity() {
    let bundle = new_bundle("conv-37-bind-lifetime").await;
    let host = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ))
    .expect("first host");
    drop(host);
    assert!(
        bundle.runtime.is_runtime_client_bound(),
        "the binding outlives the host it bound"
    );

    let rebind = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ));
    assert!(
        matches!(
            rebind,
            Err(HostConstructionError::RuntimeClientAlreadyBound { .. })
        ),
        "a surviving runtime bundle is never rebound: pre-M8 owns no recovery model"
    );

    // A genuinely fresh runtime identity binds normally, even under the
    // same conversation id.
    let fresh = new_bundle("conv-37-bind-lifetime").await;
    assert!(!fresh.runtime.is_runtime_client_bound());
    let host = RuntimeClientHost::new(config(
        fresh.runtime.clone(),
        fresh.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ))
    .expect("a fresh runtime identity is bindable");
    host.snapshot().expect("the fresh host is operational");
    assert!(fresh.runtime.is_runtime_client_bound());
}

/// Host lifetime is not attachment lifetime: reconnect replaces the
/// attachment on the same host, and yields a fresh attachment identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_replaces_the_attachment_not_the_host() {
    let bundle = new_bundle("conv-37-bind-reconnect").await;
    let host = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    ))
    .expect("host");

    let first = host.endpoint();
    let response = first.handle_request(RuntimeClientRequest::Initialize {
        id: rustx::runtime_client::RequestId::new(1),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized {
        attachment_id: first_id,
        ..
    }) = response.result
    else {
        panic!("initialized");
    };

    // Dropping the endpoint detaches; the host is untouched and is not
    // rebindable-through-the-back-door either.
    drop(first);
    assert!(
        bundle.runtime.is_runtime_client_bound(),
        "detach never releases the runtime binding"
    );

    let second = host.endpoint();
    let response = second.handle_request(RuntimeClientRequest::Initialize {
        id: rustx::runtime_client::RequestId::new(1),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized {
        attachment_id: second_id,
        ..
    }) = response.result
    else {
        panic!("reconnect on the same host initializes");
    };
    assert_ne!(
        first_id, second_id,
        "reconnect receives a fresh attachment identity"
    );
    host.snapshot().expect("the host served both attachments");
}

/// The `ConversationToolRuntime` is the one conversation authority at the
/// Runtime Client host boundary.
///
/// `RuntimeClientHostConfig` carries no conversation id of its own — the
/// field this test would otherwise have to set to a *different* conversation
/// does not exist — so the host derives its identity from the runtime it
/// coordinates. This test pins the runtime consequence of that structural
/// absence: everything the host reports, publishes, or generates names the
/// runtime's conversation, including the `AgentExecutionRequest` of an
/// admitted attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_host_conversation_identity_is_the_tool_runtime_identity() {
    let bundle = new_bundle("conv-37-authority").await;
    let model = Arc::new(FakeModel::new(vec![one_turn_stop()]));
    let host = RuntimeClientHost::new(config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        model.clone(),
    ))
    .expect("host");
    let authority = bundle.runtime.conversation_id().clone();

    // The host reports exactly the tool runtime's conversation.
    assert_eq!(host.conversation_id(), &authority);

    // `initialize` publishes that same identity, over the protocol path a
    // transport uses.
    let endpoint = host.endpoint();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: rustx::runtime_client::RequestId::new(1),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized {
        conversation_id, ..
    }) = response.result
    else {
        panic!("initialized");
    };
    assert_eq!(
        conversation_id, authority,
        "initialize reports the tool runtime's conversation"
    );
    // Detach, so the attachment below is admitted on the same host.
    drop(endpoint);

    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.conversation_id(),
        &authority,
        "the projection read model carries the tool runtime's conversation"
    );

    // A client-submitted inbound message is allocated in that identity
    // domain.
    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(2),
        content: text("go"),
    });
    let Some(RuntimeClientResult::InboundAccepted { message_id, .. }) = response.result else {
        panic!("inbound accepted");
    };
    assert_eq!(
        message_id.as_str(),
        format!("{authority}-inbound-1"),
        "generated inbound message ids are scoped to the one conversation"
    );

    // The attempt admitted for it is allocated in the same domain, and it
    // reaches settlement: `AgentExecution::new` is handed the very runtime
    // the request's conversation came from, so it cannot reject the request
    // with `MailboxError::ConversationMismatch` and the spawned attempt task
    // cannot panic after admission.
    let settled_attempt = loop {
        // Liveness guard only: the delivery wait itself is exact.
        let delivery =
            tokio::time::timeout(std::time::Duration::from_secs(120), subscription.next())
                .await
                .expect("the stream must not stall");
        let rustx::runtime_client::EventDelivery::Event(event) = delivery else {
            panic!("subscription stays open, got {delivery:?}");
        };
        if let RuntimeClientEvent::AttemptSettled { attempt_id, .. } = event.event {
            break attempt_id;
        }
    };
    assert_eq!(
        settled_attempt.as_str(),
        format!("{authority}-attempt-0"),
        "generated attempt ids are scoped to the one conversation"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the attempt actually ran against the model"
    );
    let (settled, _) = host.snapshot().expect("snapshot");
    let attempt = settled.attempt.expect("the attempt is projected");
    assert_eq!(attempt.attempt_id, settled_attempt);
}
