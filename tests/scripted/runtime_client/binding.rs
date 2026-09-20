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

use super::super::support;

use std::sync::Arc;

use rustx::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use rustx::context::{AgentStatusEngine, DefaultTokenEstimator, TokenEstimator};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;

use rustx::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
};
use rustx::runtime::identity::AgentId;
use rustx::runtime_client::{
    RuntimeClientEvent, RuntimeClientHost, RuntimeClientHostConfig, RuntimeClientRequest,
    RuntimeClientResult,
};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::runtime::ConversationToolRuntime;

use support::fake::{FakeModel, FakeStep};

/// One independently constructed runtime bundle: a fresh runtime identity
/// with its own workspace, plus a coordinator over it.
struct Bundle {
    _dir: tempfile::TempDir,
    runtime: ConversationToolRuntime,
    coordinator: CapabilityCoordinator,
}

async fn new_bundle(conversation: &str) -> Bundle {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let runtime = ConversationToolRuntime::from_config(
        rustx::runtime::identity::ConversationId::new(conversation),
        rustx::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(rustx::extensions::NativeAgentExtensions::with_agent_status(
            rustx::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: runtime.conversation_id().clone(),
        workspace: runtime.workspace().clone(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: runtime.extension_tool_plane(),
        agent_activation: rustx::capabilities::AgentActivation::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: runtime.environment().clone(),
        environment_store_root: dir.path().join("skill-env"),
    })
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    Bundle {
        _dir: dir,
        runtime,
        coordinator,
    }
}

/// Builds the conversation runtime coordinator and the host config over the
/// given runtime bundle handles.
///
/// The one-time **coordinator** binding is claimed by `ConversationRuntime`
/// construction, so a second runtime over any handle of the same identity
/// is rejected with the typed already-bound error. The one-time **client**
/// binding is claimed by host construction, so a second host over the same
/// runtime is rejected with [`HostConstructionError::RuntimeClientAlreadyBound`].
fn try_config(
    runtime: ConversationToolRuntime,
    coordinator: CapabilityCoordinator,
    model: Arc<FakeModel>,
) -> Result<(ConversationRuntime, RuntimeClientHostConfig), rustx::runtime::ConversationRuntimeError>
{
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let conversation_runtime = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: support::model::scripted_session_model(model),
        approval_mode: rustx::runtime::ApprovalMode::Policy,
        model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime: runtime,
        resources: Arc::new(rustx::runtime::RuntimeResourceSnapshot::new(
            rustx::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            rustx::context::ContextAssembly::new(),
            coordinator.current_snapshot(),
        )),
        resource_loader: Arc::new(rustx::runtime::FilesystemRuntimeResourceLoader::new(
            coordinator.current_snapshot().workspace_root(),
        )),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })?;
    Ok((
        conversation_runtime.clone(),
        RuntimeClientHostConfig {
            runtime: conversation_runtime,
            replay_limit: None,
        },
    ))
}

/// Infallible construction; panics when the runtime identity is already
/// bound to a conversation runtime.
fn config(
    runtime: ConversationToolRuntime,
    coordinator: CapabilityCoordinator,
    model: Arc<FakeModel>,
) -> (ConversationRuntime, RuntimeClientHostConfig) {
    try_config(runtime, coordinator, model).expect("conversation runtime")
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
    let bundle = new_bundle("conv_978c7a00-2dc9-77bb-883d-e0ffc411cb3c").await;
    let clone = bundle.runtime.clone();
    let second_clone = clone.clone();
    assert!(!bundle.runtime.is_runtime_client_bound());
    assert!(!bundle.runtime.is_conversation_runtime_bound());
    assert!(!clone.is_conversation_runtime_bound());

    let (runtime, host_config) = config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    );
    let _host =
        RuntimeClientHost::new(host_config).expect("the first host binds the runtime identity");
    runtime.activate();

    // Every handle of the same identity observes both bindings — including
    // clones taken before the coordinator existed and clones of clones.
    assert!(bundle.runtime.is_conversation_runtime_bound());
    assert!(clone.is_conversation_runtime_bound());
    assert!(second_clone.is_conversation_runtime_bound());
    assert!(bundle.coordinator.is_conversation_runtime_bound());
    assert!(bundle.runtime.is_runtime_client_bound());
    assert!(clone.is_runtime_client_bound());
    assert!(bundle.coordinator.is_runtime_client_bound());
    assert_eq!(
        runtime.conversation_id().as_str(),
        "conv_978c7a00-2dc9-77bb-883d-e0ffc411cb3c"
    );
}

/// The binding is a lifetime binding, not a lease: dropping the bound host
/// never makes the same runtime identity bindable again. A genuinely fresh
/// runtime identity is required — and is accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_host_never_rebinds_the_runtime_identity() {
    let bundle = new_bundle("conv_a84801f6-efc4-7057-a622-44f40e7c36b0").await;
    let (runtime, host_config) = config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    );
    let host = RuntimeClientHost::new(host_config).expect("first host");
    runtime.activate();
    drop(host);
    assert!(
        bundle.runtime.is_runtime_client_bound(),
        "the client binding outlives the host it bound"
    );
    assert!(
        bundle.runtime.is_conversation_runtime_bound(),
        "the coordinator binding outlives the runtime it bound"
    );

    // A fresh coordinator over a surviving runtime bundle is rejected at
    // Coordinator construction owns no generic recovery policy; that
    // evidence is retained for the later recovery/supervision milestone.
    let rebind = try_config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    );
    assert!(
        matches!(
            rebind,
            Err(rustx::runtime::ConversationRuntimeError::RuntimeAlreadyBound {
                conversation_id,
            }) if conversation_id.as_str() == "conv_a84801f6-efc4-7057-a622-44f40e7c36b0"
        ),
        "a surviving runtime bundle is never rebound: recovery policy is not a host-binding concern"
    );

    // A genuinely fresh runtime identity binds normally, even under the
    // same conversation id.
    let fresh = new_bundle("conv_a84801f6-efc4-7057-a622-44f40e7c36b0").await;
    assert!(!fresh.runtime.is_runtime_client_bound());
    let (fresh_runtime, fresh_host_config) = config(
        fresh.runtime.clone(),
        fresh.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    );
    let host =
        RuntimeClientHost::new(fresh_host_config).expect("a fresh runtime identity is bindable");
    fresh_runtime.activate();
    host.snapshot().expect("the fresh host is operational");
    assert!(fresh.runtime.is_runtime_client_bound());
}

/// Host lifetime is not attachment lifetime: reconnect replaces the
/// attachment on the same host, and yields a fresh attachment identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_replaces_the_attachment_not_the_host() {
    let bundle = new_bundle("conv_090b02e3-c92a-7a0d-a2ef-7550fd8604bb").await;
    let (runtime, host_config) = config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        Arc::new(FakeModel::new(Vec::new())),
    );
    let host = RuntimeClientHost::new(host_config).expect("host");
    runtime.activate();

    let first = host.endpoint();
    let response = first.handle_request(RuntimeClientRequest::Initialize {
        id: rustx::runtime_client::RequestId::new(1),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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
    let bundle = new_bundle("conv_8be1e7cd-b2fb-777b-9ef7-7c224acce42c").await;
    let model = Arc::new(FakeModel::new(vec![one_turn_stop()]));
    let (runtime, host_config) = config(
        bundle.runtime.clone(),
        bundle.coordinator.clone(),
        model.clone(),
    );
    let host = RuntimeClientHost::new(host_config).expect("host");
    runtime.activate();
    let authority = bundle.runtime.conversation_id().clone();

    // The host reports exactly the tool runtime's conversation.
    assert_eq!(host.conversation_id(), &authority);

    // `initialize` publishes that same identity, over the protocol path a
    // transport uses.
    let endpoint = host.endpoint();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: rustx::runtime_client::RequestId::new(1),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        let delivery = tokio::time::timeout(std::time::Duration::from_mins(2), subscription.next())
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
