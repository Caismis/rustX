use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::ProjectionStore;
use crate::durable::RequestStore;
use crate::durable::inbox::InboxStore;
use crate::runtime::conversation_runtime::test_support::*;

/// The runtime rejects a `SubagentRegistry` that belongs to another
/// conversation's ownership domain before anything is claimed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn construction_rejects_a_registry_for_another_conversation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
        ))
        .expect("in-memory store"),
    );
    let (_subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
        store,
        &ConversationId::new("conv_a2120f51-0e8e-7082-8caf-04e5d3e2a71d"),
        &AgentId::new("agent-a"),
        &AgentId::new("agent-a"),
    )
    .await;
    let error = ConversationRuntime::new(config).expect_err("mismatched registry domain");
    assert!(
        matches!(
            &error,
            ConversationRuntimeError::SubagentOwnershipMismatch {
                registry_conversation,
                runtime_conversation,
            } if registry_conversation == &ConversationId::new("conv_a2120f51-0e8e-7082-8caf-04e5d3e2a71d")
                && runtime_conversation == &ConversationId::new("conv_efeaf863-95c1-73d9-8623-5a766b1967be")
        ),
        "the constructor names both ownership domains: {error}"
    );
}

/// The runtime rejects a `SubagentRegistry` whose parent agent identity
/// disagrees with the runtime's own agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn construction_rejects_a_registry_for_another_parent_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
        ))
        .expect("in-memory store"),
    );
    let (_subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
        store,
        &ConversationId::new("conv_efeaf863-95c1-73d9-8623-5a766b1967be"),
        &AgentId::new("agent-other"),
        &AgentId::new("agent-a"),
    )
    .await;
    let error = ConversationRuntime::new(config).expect_err("mismatched parent agent");
    assert!(
        matches!(
            error,
            ConversationRuntimeError::SubagentOwnershipMismatch { .. }
        ),
        "a registry for another parent agent is rejected: {error}"
    );
}

/// A registry that already owns a committed child record can be
/// independently committed before runtime construction; the constructor
/// must not silently adopt it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn construction_rejects_a_non_pristine_registry_with_committed_children() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_3e98d554-da03-7c85-899c-22164a246018",
        ))
        .expect("in-memory store"),
    );
    let (subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_3e98d554-da03-7c85-899c-22164a246018",
        store.clone(),
        &ConversationId::new("conv_3e98d554-da03-7c85-899c-22164a246018"),
        &AgentId::new("agent-a"),
        &AgentId::new("agent-a"),
    )
    .await;
    // A standalone registry over the unbound mailbox can commit a child
    // before any runtime exists.
    let (staged, _peer) = stage_runtime_test_child(&dir.path().join("pre-constructed-child"));
    subagents.push_staged_override(staged);
    let accepted = match subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "pre-constructed".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-pre-constructed"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("standalone commit")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("no cancellation was requested")
        }
    };

    let error = ConversationRuntime::new(config).expect_err("non-pristine registry");
    assert!(
        matches!(
            error,
            ConversationRuntimeError::SubagentRegistryNotPristine { .. }
        ),
        "the constructor rejects a registry that already owns children: {error}"
    );

    // Settle the committed child (escalate and reap) so the fixture
    // leaks no process.
    let _ = subagents.cancel(
        &accepted.subagent_id,
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let settled = subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        crate::runtime::subagent::SubagentState::Cancelled
    );
}

/// The matching pristine registry is accepted: construction keeps its
/// normal composition behavior.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn construction_accepts_a_matching_pristine_registry() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_77a234cd-423d-7899-862c-728e8960bb43",
        ))
        .expect("in-memory store"),
    );
    let (_subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_77a234cd-423d-7899-862c-728e8960bb43",
        store,
        &ConversationId::new("conv_77a234cd-423d-7899-862c-728e8960bb43"),
        &AgentId::new("agent-a"),
        &AgentId::new("agent-a"),
    )
    .await;
    let runtime = ConversationRuntime::new(config).expect("matching pristine registry");
    assert!(!runtime.is_activated());
}

/// The runtime/subagent ownership transfer has one deterministic total
/// order. A standalone `SubagentRegistry::commit` that entered its
/// mailbox standalone decision before the constructor bound the mailbox
/// Inactive publishes its durable ownership and record first; the
/// constructor's post-claim pristine arbitration then observes the
/// non-pristine plane, rolls back every claim it acquired, and rejects
/// typed. The runtime never silently adopts a child started outside its
/// ownership transfer.
///
/// Production synchronization: the registry installs the exact committing
/// owner before the off-lock durable append. `is_pristine` observes either
/// that claim or the committed record, with no empty cut between them.
///
/// The commit hook parks after standalone arbitration, before `SQLite`.
/// Construction must reject while the hook is still parked, proving both
/// ownership exclusion and absence of a registry I/O lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn a_standalone_subagent_commit_winning_the_transfer_race_rejects_construction() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b",
        ))
        .expect("in-memory store"),
    );
    let (subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b",
        store.clone(),
        &ConversationId::new("conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b"),
        &AgentId::new("agent-a"),
        &AgentId::new("agent-a"),
    )
    .await;
    let tool_runtime = config.tool_runtime.clone();
    let hook = Arc::new(crate::runtime::subagent::CommitBoundaryHook::default());
    subagents.install_commit_boundary_hook(hook.clone());
    let (staged, _peer) = stage_runtime_test_child(&dir.path().join("race-child"));
    subagents.push_staged_override(staged);

    // The standalone commit task: prepares privately and parks inside
    // the off-lock ownership commit boundary (the CommitBoundaryHook),
    // proving it crossed the mailbox standalone decision with the
    // mailbox still unbound.
    let commit_registry = subagents.clone();
    let committer = tokio::spawn(async move {
        let prepared = commit_registry
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        title: "Explore task".into(),
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("explore"),
                        approval_mode: crate::runtime::ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        attachments: Vec::new(),
                        task: "transfer race".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("call-transfer-race"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                    },
                },
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("prepare");
        commit_registry
            .commit(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || hook.wait_until_entered()
        }),
    )
    .await
    .expect("commit-boundary liveness")
    .expect("commit-boundary entered");

    // Start the constructor only now: static domain validation passes,
    // the mailbox is bound to the runtime's Inactive lifecycle, and the
    // post-claim pristine arbitration sees the exact committing owner
    // without waiting for its parked durability operation.
    let constructor = tokio::spawn(async move { ConversationRuntime::new(config) });
    let construction = tokio::time::timeout(std::time::Duration::from_secs(10), constructor)
        .await
        .expect("constructor liveness while durability is parked")
        .expect("constructor task");
    hook.release();

    let commit_outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
        .await
        .expect("commit liveness")
        .expect("committer")
        .expect("standalone commit succeeds on the unbound mailbox");
    let accepted = match commit_outcome {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("no cancellation was requested")
        }
    };
    assert!(
        matches!(
            construction,
            Err(ConversationRuntimeError::SubagentRegistryNotPristine { .. })
        ),
        "a standalone child commit that won the transfer must make the constructor reject: {construction:?}"
    );

    // The committed child remains owned by the standalone registry; the
    // constructor never adopted it and never spawned anything.
    let snapshots = subagents.all_snapshots();
    assert_eq!(snapshots.len(), 1, "exactly the standalone child is owned");
    assert_eq!(
        snapshots[0].subagent_id, accepted.subagent_id,
        "the same identity the standalone commit published"
    );
    assert!(
        !tool_runtime.mailbox().is_bound_inactive(),
        "the failed constructor rolled back the mailbox binding"
    );
    // Exactly one durable ownership fact exists — no duplicate child
    // ownership event from the constructor.
    let ownership_facts = store
        .read_events(None, 64)
        .expect("events")
        .events
        .iter()
        .filter(|envelope| {
            matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    ..
                } if *subagent_id == accepted.subagent_id
            )
        })
        .count();
    assert_eq!(ownership_facts, 1, "one durable ownership fact");

    // Settle the committed child (escalate and reap) so the fixture
    // leaks no process.
    let _ = subagents.cancel(
        &accepted.subagent_id,
        crate::runtime::types::CancellationReason::UserRequested,
    );
    subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
}

/// The opposite side of the same total order: a runtime ownership claim
/// that wins first binds the canonical mailbox to the runtime's
/// `Inactive` lifecycle, so a later standalone `SubagentRegistry::commit`
/// is refused by the lifecycle before it can publish anything. The
/// constructor observes a pristine plane and succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runtime_claim_winning_first_refuses_the_later_standalone_commit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_21809a09-ae58-7275-8747-47667d792339",
        ))
        .expect("in-memory store"),
    );
    let (subagents, config) = subagent_runtime_config_with_registry(
        &dir,
        "conv_21809a09-ae58-7275-8747-47667d792339",
        store.clone(),
        &ConversationId::new("conv_21809a09-ae58-7275-8747-47667d792339"),
        &AgentId::new("agent-a"),
        &AgentId::new("agent-a"),
    )
    .await;

    // The runtime ownership claim wins first: construction succeeds on
    // the pristine plane and binds the mailbox Inactive.
    let _runtime = ConversationRuntime::new(config).expect("construction succeeds");
    assert!(subagents.is_pristine());

    // A later standalone subagent start is refused by the runtime-owned
    // Inactive lifecycle at `prepare` — the same lifecycle gate that
    // guards the ownership commit, and the earliest point of the start
    // path — so nothing is ever spawned or published.
    let error = subagents
        .prepare(
            &crate::runtime::subagent::SubagentStartSpec {
                authority: crate::runtime::subagent::DurableAgentAuthority {
                    title: "Explore task".into(),
                    execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
                    resolved: test_resolved_subagent("explore"),
                    approval_mode: crate::runtime::ApprovalMode::Policy,
                },
                admission: crate::runtime::subagent::ActivationAdmission {
                    attachments: Vec::new(),
                    task: "refused after claim".to_owned(),
                    context: None,
                    origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("call-refused"),
                    },
                    terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                },
            },
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect_err("standalone start refused by the runtime-owned lifecycle");
    assert!(matches!(
        error,
        crate::runtime::subagent::SubagentStartError::ConversationInactive
    ));
    assert!(
        subagents.is_pristine(),
        "the refused start published no record"
    );
    assert!(subagents.all_snapshots().is_empty());
    assert!(
        store
            .read_events(None, 64)
            .expect("events")
            .events
            .iter()
            .all(|envelope| !matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
            )),
        "no durable ownership fact entered the journal"
    );
}

/// A quarantine belongs to one attempt, not to the conversation's
/// launch-scoped status template. The first real runtime attempt loses
/// its Time contribution to a deterministic capture failure; the next
/// attempt is constructed through the coordinator's normal path and
/// retries Time with a fresh engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_constructs_a_fresh_status_engine_for_each_attempt() {
    let seam = crate::context::AgentStatusTestSeam::new();
    seam.fail_capture_once(crate::context::AgentStatusModuleId::Time);
    let status_engine = AgentStatusEngine::default().with_test_seam(seam.clone());
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script(), one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            status_engine,
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();

    runtime
        .submit_inbound(text_content("first attempt"))
        .expect("first inbound accepted");
    let first_ledger = await_settled_ledger(&runtime, 0).await;
    assert_eq!(
        seam.capture_count(crate::context::AgentStatusModuleId::Time),
        1,
        "the first attempt captures Time once before quarantining it"
    );
    assert!(
        first_ledger.iter().all(|message| !matches!(
            message,
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    InboundKind::Context(crate::message::types::ContextKind::AgentStatus(_))
                )
        )),
        "the failed module contributes no status message"
    );

    runtime
        .submit_inbound(text_content("second attempt"))
        .expect("second inbound accepted");
    let second_ledger = await_settled_ledger(&runtime, 1).await;
    assert_eq!(
        seam.capture_count(crate::context::AgentStatusModuleId::Time),
        2,
        "a new runtime attempt retries the previously quarantined module"
    );
    assert!(
        second_ledger.iter().any(|message| matches!(
            message,
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    InboundKind::Context(crate::message::types::ContextKind::AgentStatus(_))
                )
        )),
        "the retried Time module contributes through the normal runtime path"
    );
    assert_eq!(
        model.requests().len(),
        2,
        "one provider request per attempt"
    );
}

/// A cold reopen publishes current resource authority for a new attempt,
/// while the old compaction summary and old `RequestSnapshot` remain exact
/// historical values. Reopen does not synthesize a resource-change fact.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_reopen_uses_current_resources_without_rewriting_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("workspace/AGENTS.md");
    std::fs::create_dir_all(path.parent().expect("workspace")).expect("workspace");
    std::fs::write(&path, "old project authority").expect("old project file");

    let (runtime_a, model_a) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script(), text_turn_script("summary-A")],
        None,
        None,
        HeadlessRuntimeOptions {
            project_context_files: vec![project_file(&path, "old project authority")],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime_a.activate();
    runtime_a
        .submit_inbound(text_content("historical request A"))
        .expect("request A");
    let _ = await_settled_ledger(&runtime_a, 0).await;
    let snapshots_a = request_snapshots(&runtime_a.request_history());
    assert_eq!(snapshots_a.len(), 1);
    assert!(
        snapshots_a[0]
            .effective_system_prompt
            .contains("old project authority")
    );
    let snapshot_a_bytes = serde_json::to_vec(&snapshots_a[0]).expect("snapshot A bytes");

    let outcome = runtime_a
        .compact_context(None)
        .await
        .expect("compaction under resource generation A");
    let ledger_a = runtime_a.coordinator_ledger().expect("generation A ledger");
    let summary_a = runtime_a
        .inner
        .store
        .projection_checkpoint(&outcome.checkpoint_id)
        .unwrap();
    let summary_a_bytes = serde_json::to_vec(&summary_a).expect("summary A bytes");
    assert!(
        ledger_a.iter().all(|message| {
            !serde_json::to_string(message)
                .expect("ledger message JSON")
                .contains("old project authority")
        }),
        "project instructions remain request-time authority, not history"
    );
    runtime_a.shutdown().await.expect("stop generation A");
    drop(runtime_a);
    drop(model_a);

    std::fs::write(&path, "new project authority").expect("new project file");
    let (runtime_b, model_b) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            project_context_files: vec![project_file(&path, "new project authority")],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime_b.activate();
    runtime_b
        .submit_inbound(text_content("new request B"))
        .expect("request B");
    let ledger_b = await_settled_ledger(&runtime_b, 0).await;
    let snapshots_b = request_snapshots(&runtime_b.request_history());
    assert_eq!(snapshots_b.len(), 2);
    assert_eq!(
        serde_json::to_vec(&snapshots_b[0]).expect("reopened snapshot bytes"),
        snapshot_a_bytes,
        "cold reopen does not rewrite the old RequestSnapshot"
    );
    assert_eq!(
        serde_json::to_vec(
            &runtime_b
                .inner
                .store
                .projection_checkpoint(&outcome.checkpoint_id)
                .unwrap()
        )
        .unwrap(),
        summary_a_bytes,
        "cold reopen preserves immutable checkpoint bodies and provenance"
    );
    assert!(
        model_b.requests()[0]
            .effective_system_prompt
            .contains("new project authority")
    );
    assert!(
        !model_b.requests()[0]
            .effective_system_prompt
            .contains("old project authority")
    );
    assert!(
        snapshots_b[1]
            .effective_system_prompt
            .contains("new project authority")
    );
    assert!(ledger_b.iter().all(|message| {
        !serde_json::to_string(message)
            .expect("ledger message JSON")
            .contains("resources changed")
    }));
    runtime_b.shutdown().await.expect("stop generation B");
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preactivation_mcp_settlement_failure_is_replayed_and_fences_activation() {
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::mcp::test_sync::CloseProbe;

    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }

    let test_name = "runtime::conversation_runtime::bootstrap::tests::preactivation_mcp_settlement_failure_is_replayed_and_fences_activation";
    let binding = crate::tools::mcp::McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: crate::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: fixture_spawn_args(test_name),
            cwd: None,
            environment: std::collections::BTreeMap::from([(
                FIXTURE_MODE_ENV.to_owned(),
                "1".to_owned(),
            )]),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let server_id = crate::runtime::identity::McpServerId::new("preactivation-failure");
    let close = Arc::new(CloseProbe::failing(
        "preactivation MCP terminal state is unproven",
    ));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            mcp_servers: std::collections::BTreeMap::from([(server_id, binding)]),
            // The helper retires and settles this generation before it
            // constructs ConversationRuntime, so the callback installed
            // by the runtime must replay an already-authoritative failure.
            pre_activation_mcp_close_probe: Some(close),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;

    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining,
        "a replayed pre-activation settlement failure enters the explicit failure drain"
    );
    assert!(!runtime.inner.lifecycle.is_running());
    runtime.activate();
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining,
        "activation cannot reopen healthy admission after callback replay"
    );
    assert_eq!(
        runtime.submit_inbound(text_content("must never start")),
        Err(InboundAdmissionError::Shutdown)
    );
    assert!(model.requests().is_empty(), "no attempt crossed activation");

    assert!(
        runtime.settle_child_physical_lifetime(false).await.is_err(),
        "MCP containment failure must remain physically unproven"
    );
    let shutdown = runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("shutdown must retain the pre-activation settlement failure: {shutdown:?}");
    };
    assert!(detail.contains("preactivation MCP terminal state is unproven"));
    assert_eq!(
        runtime.inner.capability.pending_mcp_retirements(),
        1,
        "the failed generation remains authoritative through final reporting"
    );
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_settlement_failure_wins_deterministic_activation_race() {
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::mcp::test_sync::CloseProbe;

    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }

    let test_name = "runtime::conversation_runtime::bootstrap::tests::mcp_settlement_failure_wins_deterministic_activation_race";
    let binding = crate::tools::mcp::McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: crate::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: fixture_spawn_args(test_name),
            cwd: None,
            environment: std::collections::BTreeMap::from([(
                FIXTURE_MODE_ENV.to_owned(),
                "1".to_owned(),
            )]),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let server_id = crate::runtime::identity::McpServerId::new("activation-race");
    let activation_gate = Arc::new(Gate::default());
    activation_gate.arm();
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script()],
        None,
        Some(CoordinatorProbe {
            activation_gate: Some(activation_gate.clone()),
            ..CoordinatorProbe::default()
        }),
        HeadlessRuntimeOptions {
            mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    let old_runtime = runtime
        .inner
        .capability
        .current_mcp_runtime(&server_id)
        .expect("generation A");
    let close = Arc::new(CloseProbe::failing(
        "activation race MCP terminal state is unproven",
    ));
    old_runtime.install_close_probe(close);

    let activation_runtime = runtime.clone();
    let activation = tokio::spawn(async move { activation_runtime.activate() });
    // The gate proves activation has not acquired the coordinator lock or
    // crossed Inactive -> Running yet.
    activation_gate.wait_entered();

    runtime.inner.capability.retire_current_mcp_runtimes();
    let settlement = runtime.inner.capability.settle_ready_mcp_runtimes().await;
    assert!(
        settlement.is_err(),
        "the retired generation must fail settlement"
    );
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining,
        "failure publication wins while activation is still gated"
    );

    activation_gate.release();
    activation.await.expect("activation task");
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining
    );
    assert_eq!(
        runtime.submit_inbound(text_content("must be fenced")),
        Err(InboundAdmissionError::Shutdown)
    );
    assert!(model.requests().is_empty());

    let shutdown = runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("shutdown must retain the activation-race failure: {shutdown:?}");
    };
    assert!(detail.contains("activation race MCP terminal state is unproven"));
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_settlement_failure_fences_after_activation_wins() {
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::mcp::test_sync::CloseProbe;

    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }

    let test_name = "runtime::conversation_runtime::bootstrap::tests::mcp_settlement_failure_fences_after_activation_wins";
    let binding = crate::tools::mcp::McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: crate::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: fixture_spawn_args(test_name),
            cwd: None,
            environment: std::collections::BTreeMap::from([(
                FIXTURE_MODE_ENV.to_owned(),
                "1".to_owned(),
            )]),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let server_id = crate::runtime::identity::McpServerId::new("activation-first");
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Running
    );
    let old_runtime = runtime
        .inner
        .capability
        .current_mcp_runtime(&server_id)
        .expect("generation A");
    old_runtime.install_close_probe(Arc::new(CloseProbe::failing(
        "activation-first MCP terminal state is unproven",
    )));

    runtime.inner.capability.retire_current_mcp_runtimes();
    assert!(
        runtime
            .inner
            .capability
            .settle_ready_mcp_runtimes()
            .await
            .is_err()
    );
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining,
        "a later failure immediately fences a runtime that activated first"
    );
    assert_eq!(
        runtime.submit_inbound(text_content("must be fenced")),
        Err(InboundAdmissionError::Shutdown)
    );
    assert!(model.requests().is_empty());

    let shutdown = runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("shutdown must retain the activation-first failure: {shutdown:?}");
    };
    assert!(detail.contains("activation-first MCP terminal state is unproven"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_profile_is_snapshot_system_authority_not_bootstrap_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let persona = "immutable child exploration persona";
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            agent_profile: Some(persona.to_owned()),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    assert!(
        runtime
            .coordinator_ledger()
            .expect("bootstrap history")
            .is_empty(),
        "agent profile creates no fake bootstrap conversation message"
    );
    runtime.activate();
    runtime
        .submit_inbound(text_content("delegated task"))
        .expect("delegated inbound");
    let _ = await_settled_ledger(&runtime, 0).await;
    assert!(
        model.requests()[0]
            .effective_system_prompt
            .contains(persona)
    );
    let snapshots = request_snapshots(&runtime.request_history());
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].effective_system_prompt.contains(persona));
    assert!(snapshots[0].system_sections.iter().any(|section| {
        section.lane == crate::context::SystemSectionLane::AgentProfile
            && section.content == persona
    }));
}

/// A runtime whose capability coordinator and tool runtime disagree on
/// ownership is rejected at construction.
#[test]
fn construction_validates_ownership() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_bf9033a7-86e2-71aa-8314-b791ebfdbfec");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let other = ConversationId::new("conv_449370cc-308b-7409-84bd-ff539142e4fb");
    let other_workspace = dir.path().join("workspace-other");
    std::fs::create_dir_all(&other_workspace).expect("other workspace");
    let other_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        other.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &other_workspace,
            dir.path().join("artifacts-other"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("other tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: other_runtime.conversation_id().clone(),
            workspace: other_runtime.workspace().clone(),
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: other_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: other_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let error = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })
    .expect_err("mismatched ownership is rejected");
    assert!(matches!(
        error,
        ConversationRuntimeError::OwnershipMismatch { .. }
    ));
}

/// Construction outside a Tokio execution runtime fails with the typed
/// activation error instead of silently creating a coordinator whose
/// admission worker may never exist.
#[test]
fn construction_outside_execution_runtime_fails_typed() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "this test runs on a plain thread"
    );
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_6d7e77c3-41c2-710b-8817-1134b3ee72c2");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    // This fixture's subject is the missing execution runtime, and it runs
    // on a plain thread — so it can never `await` a prepared candidate,
    // and its coordinator necessarily stays at revision zero with an
    // empty executable registry. It therefore composes Agent Status and
    // no Tool-providing extension, which is a coherent composition whose
    // required extension Tool authority is *empty*: the Issue #259
    // active-capability check passes on it, and construction reaches the
    // `NoExecutionRuntime` decision this test is about. A Todo-composed
    // fixture here would be genuinely incoherent — configured to publish
    // `todo`, with nothing executable carrying it — and would be refused
    // earlier, for a reason unrelated to Tokio.
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        ))
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let error = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })
    .expect_err("construction outside Tokio is rejected");
    assert!(matches!(
        error,
        ConversationRuntimeError::NoExecutionRuntime
    ));
}

/// A public runtime composition rejects an invalid model timeout policy
/// before it initializes storage or claims either ownership plane. The
/// same untouched tool-runtime/capability bundle can immediately build a
/// valid runtime afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::too_many_lines,
    reason = "one complete deterministic fixture boundary"
)]
async fn invalid_model_timeout_policy_fails_before_runtime_ownership_transfer() {
    struct Fixture {
        tool_runtime: crate::tools::runtime::ConversationToolRuntime,
        capability: crate::capabilities::CapabilityCoordinator,
        resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn config(
            &self,
            model_timeout_policy: crate::model::ModelTimeoutPolicy,
        ) -> RuntimeConversationConfig {
            RuntimeConversationConfig {
                explicit_model: true,
                agent_id: AgentId::new("agent-timeout-construction"),
                model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
                approval_mode: ApprovalMode::Policy,
                model_timeout_policy,
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator: Arc::new(DefaultTokenEstimator),
                    status_engine: Some(AgentStatusEngine::default()),
                },
                tool_runtime: self.tool_runtime.clone(),
                capability: self.capability.clone(),
                resources: self.resources.clone(),
                clock: None,
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            }
        }
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_7aeab4f1-ecc6-7d5d-85f7-f7c3015f36ee");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let capability = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id,
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("capability coordinator");
    let candidate = capability.prepare_candidate().await.expect("candidate");
    capability.commit(candidate).expect("candidate commit");
    let fixture = Fixture {
        resources: test_resources(&capability),
        tool_runtime,
        capability,
        _dir: dir,
    };

    for model_timeout_policy in [
        crate::model::ModelTimeoutPolicy::new(
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
        ),
        crate::model::ModelTimeoutPolicy::new(
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
        ),
    ] {
        assert!(matches!(
            ConversationRuntime::new(fixture.config(model_timeout_policy)),
            Err(ConversationRuntimeError::InvalidModelTimeoutPolicy)
        ));
    }

    let runtime =
        ConversationRuntime::new(fixture.config(crate::model::ModelTimeoutPolicy::default()))
            .expect("valid construction can reuse both untouched ownership planes");
    runtime.activate();
    runtime
        .shutdown()
        .await
        .expect("valid runtime admits no work and shuts down cleanly");
}

/// A public runtime composition rejects an invalid tool
/// execution-liveness policy (Issue #204) before it initializes storage
/// or claims either ownership plane: a zero hard deadline would admit an
/// execution that can never live, and a zero idle window would fire at
/// every observation. The same untouched tool-runtime/capability bundle
/// can immediately build a valid runtime afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::too_many_lines,
    reason = "one complete deterministic fixture boundary"
)]
async fn invalid_tool_deadline_policy_fails_before_runtime_ownership_transfer() {
    struct Fixture {
        tool_runtime: crate::tools::runtime::ConversationToolRuntime,
        capability: crate::capabilities::CapabilityCoordinator,
        resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn config(
            &self,
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
        ) -> RuntimeConversationConfig {
            RuntimeConversationConfig {
                explicit_model: true,
                agent_id: AgentId::new("agent-tool-deadline-construction"),
                model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
                approval_mode: ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy,
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator: Arc::new(DefaultTokenEstimator),
                    status_engine: Some(AgentStatusEngine::default()),
                },
                tool_runtime: self.tool_runtime.clone(),
                capability: self.capability.clone(),
                resources: self.resources.clone(),
                clock: None,
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            }
        }
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_b3920643-26f1-71a1-81eb-f19d0c413769");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let capability = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id,
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("capability coordinator");
    let candidate = capability.prepare_candidate().await.expect("candidate");
    capability.commit(candidate).expect("candidate commit");
    let fixture = Fixture {
        resources: test_resources(&capability),
        tool_runtime,
        capability,
        _dir: dir,
    };

    for tool_deadline_policy in [
        crate::tools::deadline::ToolExecutionDeadlinePolicy::new(std::time::Duration::ZERO, None),
        crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
            std::time::Duration::from_secs(1),
            Some(std::time::Duration::ZERO),
        ),
    ] {
        assert!(matches!(
            ConversationRuntime::new(fixture.config(tool_deadline_policy)),
            Err(ConversationRuntimeError::InvalidToolDeadlinePolicy)
        ));
    }

    let runtime = ConversationRuntime::new(
        fixture.config(crate::tools::deadline::ToolExecutionDeadlinePolicy::default()),
    )
    .expect("valid construction can reuse both untouched ownership planes");
    runtime.activate();
    runtime
        .shutdown()
        .await
        .expect("valid runtime admits no work and shuts down cleanly");
}

/// Issue #12 (M9a): an incomplete Assistant tool-call durable tail is
/// **repaired**, not refused.
///
/// The M8 gate returned `RecoveryRequired` here, which left the
/// conversation permanently unusable. M9a supersedes it: the missing
/// canonical sibling is committed from durable evidence — this call has
/// no `ToolExecutionStarted` fact at all, so it provably never ran and is
/// recorded as parent-cancelled rather than as an unknown outcome — and
/// the recovered pending inbound keeps its exact durable identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incomplete_tool_tail_is_repaired_and_pending_remains_intact() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_6536aaf7-ead1-793b-883d-d1105c35ac07");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let store_path = artifacts.join("conversation.sqlite");
    let seed = vec![seed_user("msg-u0", "start")];
    {
        let store =
            crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                .expect("open");
        store.initialize(&seed).expect("seed");
        store
            .append_canonical(&assistant_tool_block("assistant-tool", "call-1"))
            .expect("assistant tool call");
        store
            .accept_inbound(crate::durable::inbox::InboundDraft {
                message_id: None,
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: text_content("pending"),
                timestamp: fixed_time(),
                correlation: None,
            })
            .expect("accept pending");
    }

    let runtime = runtime_at(
        &dir,
        "conv_6536aaf7-ead1-793b-883d-d1105c35ac07",
        seed,
        vec![one_turn_script()],
    )
    .await
    .expect("M9a repairs the incomplete tool turn instead of refusing it");
    // The classification is Class D: no attempt ever entered durable
    // authority in this fixture, so nothing is terminalized — only the
    // structure is repaired.
    assert_eq!(
        runtime.recovery().attempt_class(),
        &crate::runtime::recovery::AttemptRecoveryClass::NotStarted
    );
    assert_eq!(
        runtime.recovery().reconciliation().repaired_tool_results,
        vec![ToolCallId::new("call-1")],
        "the missing canonical sibling was committed"
    );
    assert_eq!(runtime.recovery().pending_inbound(), 1);

    // The repaired turn is canonical and structurally complete, and the
    // recovered pending inbound kept its exact durable identity.
    let active = runtime
        .coordinator_active_ids()
        .expect("the coordinator owns the recovered state while idle");
    assert!(
        active
            .iter()
            .any(|id| id.as_str() == "assistant-tool-recovered-tool-call-1"),
        "the recovery-generated ToolResult is active: {active:?}"
    );
    let ledger = runtime.coordinator_ledger().expect("ledger");
    let repaired = ledger
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.clone())
            }
            _ => None,
        })
        .expect("the repaired sibling is a canonical Ledger fact");
    assert_eq!(
        repaired.result.status,
        crate::tools::types::ToolExecutionStatus::Cancelled {
            reason: CancellationReason::ParentCancelled,
            phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
        },
        "a call with no durable start evidence never ran; it is not reported as unknown"
    );
    assert!(
        repaired.result.content.is_empty(),
        "recovery never invents a result body"
    );

    let reopened = crate::durable::SqliteConversationStore::open(conversation_id, &store_path)
        .expect("reopen");
    let pending = reopened.load_pending().expect("load pending");
    assert_eq!(pending.len(), 1, "recovered pending stays intact");
    assert_eq!(pending[0].sequence.get(), 1);
    assert_eq!(
        pending[0].message_id.as_str(),
        "conv_6536aaf7-ead1-793b-883d-d1105c35ac07-inbound-1"
    );
}

/// Issue #63 (Blocker 1, test B): a structurally complete tool group is
/// a recovery-safe boundary; the runtime reconstructs and admits normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_tool_group_is_recoverable_and_admits_normally() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_47e0d303-9630-73a3-8913-fea76e160536");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let store_path = artifacts.join("conversation.sqlite");
    let seed = vec![seed_user("msg-u0", "start")];
    {
        let store =
            crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                .expect("open");
        store.initialize(&seed).expect("seed");
        store
            .append_canonical(&assistant_tool_block("assistant-tool", "call-1"))
            .expect("assistant tool call");
        store
            .append_canonical(&tool_result_block("call-1"))
            .expect("tool result");
    }

    let runtime = runtime_at(
        &dir,
        "conv_47e0d303-9630-73a3-8913-fea76e160536",
        seed,
        vec![one_turn_script()],
    )
    .await
    .expect("a complete tool group is recovery-safe");
    runtime.activate();

    // Normal admission is allowed and the attempt settles.
    let admission = runtime
        .submit_inbound(text_content("hello"))
        .expect("accepted after recovery");
    runtime.child_attempt_settlement(0).await;
    let ledger = runtime.coordinator_ledger().expect("settled");
    assert!(
        ledger
            .iter()
            .any(|m| matches!(m, MessageBlock::User(user) if user.id == admission.message_id)),
        "the recovered runtime admitted the inbound exactly once"
    );
}

/// Durable compaction reopens the exact typed projection and canonical working set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_compaction_projection_reopens_as_one_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_b0652278-531a-725d-8621-2ecfe4f19ad7");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let store_path = artifacts.join("conversation.sqlite");
    let seed = vec![seed_user("msg-a", "A"), seed_user("msg-b", "B")];
    {
        let store =
            crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                .expect("open");
        store.initialize(&seed).expect("seed");
        // A summary is durable only through the complete compaction
        // transition: immutable checkpoint content, activation/head, operation receipt,
        // and completion fact share one transaction.
        store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "seed-checkpoint",
                        1,
                        "restored history",
                    ),
                    None,
                ),
            )
            .expect("atomic compaction summary");
    }

    let runtime = runtime_at(
        &dir,
        "conv_b0652278-531a-725d-8621-2ecfe4f19ad7",
        seed,
        vec![one_turn_script()],
    )
    .await
    .expect("durable summary history reopens");
    let store = crate::durable::SqliteConversationStore::open(conversation_id, &store_path)
        .expect("reopen store");
    let head = store.load_head().expect("head");
    assert_eq!(store.projected_history().unwrap().messages.len(), 2);
    assert_eq!(
        head.active_message_ids,
        vec![crate::runtime::identity::MessageId::new("msg-b")]
    );
    assert_eq!(
        runtime.coordinator_active_ids().expect("coordinator head"),
        head.active_message_ids
    );
}

/// Issue #12 (M9a), Test A (runtime half): an idle runtime recovered with
/// pending inbound admits it by itself.
///
/// No Runtime Client is constructed, no attachment exists, and no client
/// request is made: activation alone is enough, because the durable
/// Pending Inbound Inbox *is* the queue of accepted-but-unadopted work.
/// There is deliberately no separate "recovery queue".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_pending_inbound_is_auto_admitted_with_zero_clients() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut accepted_id = None;
    let store_path = seed_crash_prefix(
        &dir,
        "conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc",
        &[],
        |store, _| {
            let accepted = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("recovered work"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept");
            accepted_id = Some(accepted.message_id);
        },
    );
    let accepted_id = accepted_id.expect("accepted");

    let (runtime, model) = runtime_with_model_at(
        &dir,
        "conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc",
        Vec::new(),
        vec![one_turn_script()],
    )
    .await
    .expect("runtime recovers");
    assert_eq!(runtime.recovery().pending_inbound(), 1);
    assert_eq!(
        runtime.recovery().resume(),
        crate::runtime::recovery::ResumeDisposition::PendingInboundOnly
    );

    // Activation is the only trigger. Nothing submits, nothing attaches.
    runtime.activate();
    runtime.child_attempt_settlement(0).await;

    let ledger = runtime.coordinator_ledger().expect("settled");
    assert_eq!(
        ledger
            .iter()
            .filter(|block| crate::conversation::message_id_of(block) == accepted_id)
            .count(),
        1,
        "the recovered pending item is adopted exactly once"
    );
    assert_eq!(model.requests().len(), 1, "exactly one model turn ran");
    let store = crate::durable::SqliteConversationStore::open(
        ConversationId::new("conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc"),
        &store_path,
    )
    .expect("reopen");
    assert!(
        store.load_pending().expect("pending").is_empty(),
        "the adopted item is no longer pending"
    );
}

/// Issue #12 (M9a), recovery Class B: an attempt that crashed before any
/// external start commit lets the already-canonical turn continue.
///
/// The continuation runs as a **new** attempt over the existing canonical
/// history: the `UserMessage` is neither re-adopted nor duplicated, and
/// the model sees exactly the recovered turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn class_b_restart_continues_the_adopted_turn_without_duplicating_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let dead_attempt = AttemptId::for_conversation(
        &ConversationId::new("conv_4b829d29-175d-72f9-8648-175183dc40d2"),
        0,
    );
    let dead = dead_attempt.clone();
    let mut adopted_id = None;
    seed_crash_prefix(
        &dir,
        "conv_4b829d29-175d-72f9-8648-175183dc40d2",
        &[],
        |store, conversation_id| {
            let accepted = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("answer me"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept");
            adopt_accepted(store, &accepted);
            adopted_id = Some(accepted.message_id);
            store
                .append_event(attempt_event(
                    conversation_id,
                    "attempt-started",
                    &dead,
                    crate::events::types::RuntimeEvent::AttemptStarted {
                        attempt_id: dead.clone(),
                    },
                ))
                .expect("attempt started");
            // CRASH: no request start, no tool start.
        },
    );
    let adopted_id = adopted_id.expect("adopted");

    let (runtime, model) = runtime_with_model_at(
        &dir,
        "conv_4b829d29-175d-72f9-8648-175183dc40d2",
        Vec::new(),
        vec![one_turn_script()],
    )
    .await
    .expect("runtime recovers");
    assert_eq!(
        runtime.recovery().attempt_class(),
        &crate::runtime::recovery::AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: dead_attempt.clone(),
        }
    );
    assert_eq!(
        runtime.recovery().resume(),
        crate::runtime::recovery::ResumeDisposition::ContinueAdoptedTurn { goal: None }
    );

    runtime.activate();
    runtime.child_attempt_settlement(0).await;

    let ledger = runtime.coordinator_ledger().expect("settled");
    assert_eq!(
        ledger
            .iter()
            .filter(|block| crate::conversation::message_id_of(block) == adopted_id)
            .count(),
        1,
        "the adopted turn is never duplicated by the continuation"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the continuation runs exactly one model turn"
    );
    assert!(
        model.requests()[0]
            .messages
            .iter()
            .any(|block| block.canonical_id() == Some(&adopted_id)),
        "the continuation carries the recovered canonical turn"
    );
    // The continuation is a genuinely new attempt, above every durable
    // ordinal.
    assert_eq!(
        runtime.recovery().next_attempt_ordinal(),
        1,
        "the allocator starts past the interrupted attempt"
    );
}

/// Issue #12 (M9b), recovery Class B reaching the same gate: the
/// recovered pending continuation drives the attempt to the identical
/// model-turn start arbitration. Cancelling there is again
/// cancellation-before-start — the same gate, the same adjudication, the
/// same never-started outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn class_b_recovery_uses_the_same_model_turn_start_gate() {
    use crate::agent::execution::test_sync::StartBoundaryPause;
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation = ConversationId::new("conv_2af196f9-fdf0-75af-85c5-b012b4f771a9");
    let dead_attempt = AttemptId::for_conversation(&conversation, 0);
    let dead = dead_attempt.clone();
    let store_path = seed_crash_prefix(
        &dir,
        "conv_2af196f9-fdf0-75af-85c5-b012b4f771a9",
        &[],
        |store, conversation_id| {
            let accepted = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("start the turn"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept");
            adopt_accepted(store, &accepted);
            store
                .append_event(attempt_event(
                    conversation_id,
                    "attempt-started",
                    &dead,
                    crate::events::types::RuntimeEvent::AttemptStarted {
                        attempt_id: dead.clone(),
                    },
                ))
                .expect("attempt started");
            // CRASH: no request start, no tool start.
        },
    );
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let (runtime, model) = runtime_with_model_probe_at(
        &dir,
        "conv_2af196f9-fdf0-75af-85c5-b012b4f771a9",
        vec![],
        vec![one_turn_script()],
        Some(CoordinatorProbe {
            start_boundary_pause: Some(pause),
            ..CoordinatorProbe::default()
        }),
    )
    .await
    .expect("runtime");
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // The recovered continuation attempt is parked immediately before the
    // start arbitration — the same boundary as a fresh inbound turn.
    pre_start.await_park(1).await;
    let attempt = AttemptId::for_conversation(&conversation, 1);
    runtime
        .cancel_current_attempt(&attempt)
        .expect("the recovered attempt is cancellable");
    pre_start.release();

    let observations = await_observation(
        pending.as_ref(),
        |o| matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event)),
    )
    .await;
    assert!(
        observations.iter().any(|o| matches!(
            o,
            ConversationObservation::Event { event, .. }
                if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
        )),
        "the recovered attempt settles cancelled"
    );
    assert!(
        model.requests().is_empty(),
        "the provider request never started"
    );

    let store =
        crate::durable::SqliteConversationStore::open(conversation, &store_path).expect("reopen");
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .is_empty(),
        "no started Request Snapshot exists"
    );
    let journal = store.read_events(None, 128).expect("journal").events;
    assert!(
        !journal.iter().any(|envelope| matches!(
            envelope.event,
            crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
        )),
        "no request-start fact exists"
    );
    let canonical = store.load_canonical().expect("canonical");
    assert_eq!(
        canonical.len(),
        1,
        "only the adopted inbound is canonical: no request-scoped context half-commit"
    );
}

/// Issue #12 (M9a), recovery Class C: a restart after a committed
/// request start starts **nothing**.
///
/// The proof is deterministic rather than a timed absence: a new
/// user-driven turn is submitted and awaited, and because at most one
/// attempt runs per conversation, its settlement happens-after any
/// recovery-initiated attempt would have. Exactly one provider request
/// exists at that point, so recovery issued none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One exact unknown-request crash prefix and restart proof.
async fn class_c_restart_issues_no_provider_request_of_its_own() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation = ConversationId::new("conv_e4c44f69-7140-7c07-8660-a8791da60c71");
    let dead_attempt = AttemptId::for_conversation(&conversation, 0);
    let dead = dead_attempt.clone();
    seed_crash_prefix(
        &dir,
        "conv_e4c44f69-7140-7c07-8660-a8791da60c71",
        &[],
        |store, conversation_id| {
            let accepted = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("ask the model"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept");
            adopt_accepted(store, &accepted);
            store
                .append_event(attempt_event(
                    conversation_id,
                    "attempt-started",
                    &dead,
                    crate::events::types::RuntimeEvent::AttemptStarted {
                        attempt_id: dead.clone(),
                    },
                ))
                .expect("attempt started");
            let mut snapshot = crate::model::RequestSnapshot::new(
                crate::model::RequestIdentity {
                    attempt_id: dead.clone(),
                    turn: crate::runtime::identity::TurnId::new("0"),
                    retry_number: 0,
                },
                crate::context::resources::LedgerFrontier(
                    (store.load_head().expect("head").revision).get(),
                ),
                "frozen prompt".to_owned(),
                Vec::new(),
                crate::runtime::RuntimeResourceRevision::new(1),
                crate::model::ModelInvocationConfig {
                    model: "model-before-restart".to_owned(),
                    protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
                    max_output_tokens: 64,
                    request_params: crate::model::RequestParams::new(),
                    capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
                    compat: crate::model::catalog::ModelCompat::default(),
                },
                64_000,
                None,
                false,
                Vec::new(),
                crate::runtime::identity::CapabilityRevision::new(1),
                crate::context::ContextGeneration {
                    id: 1,
                    contributors: Vec::new(),
                },
                None,
                Vec::new(),
            );
            let history = store.projected_history().unwrap();
            snapshot.history_cut = history.cut;
            snapshot
                .freeze_input(
                    store.conversation_id(),
                    &crate::model::ModelRequest {
                        response_schema: None,
                        messages: history.messages.clone(),
                        images: crate::model::images::ResolvedImages::new(),
                        invocation: snapshot.invocation.clone(),
                        tools: snapshot.tool_definitions.clone(),
                        effective_system_prompt: snapshot.effective_system_prompt.clone(),
                        continuation: None,
                        continuation_source: None,
                    },
                )
                .unwrap();
            store
                .commit_model_turn_start(&[], &snapshot, fixed_time())
                .expect("request start");
            // CRASH: the provider may or may not have executed this request.
        },
    );

    let (runtime, model) = runtime_with_model_at(
        &dir,
        "conv_e4c44f69-7140-7c07-8660-a8791da60c71",
        Vec::new(),
        vec![one_turn_script()],
    )
    .await
    .expect("runtime recovers");
    assert!(matches!(
        runtime.recovery().attempt_class(),
        crate::runtime::recovery::AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id,
            model_request: Some(_),
            ..
        } if attempt_id == &dead_attempt
    ));
    assert_eq!(
        runtime.recovery().resume(),
        crate::runtime::recovery::ResumeDisposition::BlockedIndeterminate
    );

    runtime.activate();
    // New user-driven work is still admissible: the ambiguity belongs to
    // the old request, not to the conversation.
    runtime
        .submit_inbound(text_content("a new turn"))
        .expect("accepted after recovery");
    runtime.child_attempt_settlement(0).await;
    assert_eq!(
        model.requests().len(),
        1,
        "only the user-driven turn reached the provider; recovery resent nothing"
    );
}

/// Issue #12 (M9a), recovery Class E: a restart after a **durably
/// completed** model request — whose canonical Assistant message never
/// committed — starts nothing either.
///
/// The provider already executed the request; rustX durably observed the
/// outcome. The proof mirrors Class C: a new user-driven turn is
/// submitted and awaited, and because at most one attempt runs per
/// conversation, its settlement happens-after any recovery-initiated
/// attempt would have. Exactly one provider request exists at that
/// point, so the recovered runtime itself initiated no replacement
/// request — and it fabricated no Assistant body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One native restart observation and actual-request settlement scenario.
async fn class_e_restart_issues_no_replacement_request() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation = ConversationId::new("conv_0224cd71-1cda-74e4-8010-dad3ad2ce447");
    let dead_attempt = AttemptId::for_conversation(&conversation, 0);
    let dead = dead_attempt.clone();
    seed_crash_prefix(
        &dir,
        "conv_0224cd71-1cda-74e4-8010-dad3ad2ce447",
        &[],
        |store, conversation_id| {
            let accepted = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("ask the model"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept");
            adopt_accepted(store, &accepted);
            store
                .append_event(attempt_event(
                    conversation_id,
                    "attempt-started",
                    &dead,
                    crate::events::types::RuntimeEvent::AttemptStarted {
                        attempt_id: dead.clone(),
                    },
                ))
                .expect("attempt started");
            let mut snapshot = crate::model::RequestSnapshot::new(
                crate::model::RequestIdentity {
                    attempt_id: dead.clone(),
                    turn: crate::runtime::identity::TurnId::new("0"),
                    retry_number: 0,
                },
                crate::context::resources::LedgerFrontier(
                    (store.load_head().expect("head").revision).get(),
                ),
                "frozen prompt".to_owned(),
                Vec::new(),
                crate::runtime::RuntimeResourceRevision::new(1),
                crate::model::ModelInvocationConfig {
                    model: "model-before-restart".to_owned(),
                    protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
                    max_output_tokens: 64,
                    request_params: crate::model::RequestParams::new(),
                    capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
                    compat: crate::model::catalog::ModelCompat::default(),
                },
                64_000,
                None,
                false,
                Vec::new(),
                crate::runtime::identity::CapabilityRevision::new(1),
                crate::context::ContextGeneration {
                    id: 1,
                    contributors: Vec::new(),
                },
                None,
                Vec::new(),
            );
            let history = store.projected_history().unwrap();
            snapshot.history_cut = history.cut;
            snapshot
                .freeze_input(
                    store.conversation_id(),
                    &crate::model::ModelRequest {
                        response_schema: None,
                        messages: history.messages.clone(),
                        images: crate::model::images::ResolvedImages::new(),
                        invocation: snapshot.invocation.clone(),
                        tools: snapshot.tool_definitions.clone(),
                        effective_system_prompt: snapshot.effective_system_prompt.clone(),
                        continuation: None,
                        continuation_source: None,
                    },
                )
                .unwrap();
            store
                .commit_model_turn_start(&[], &snapshot, fixed_time())
                .expect("request start");
            append_completed_request(store, conversation_id, &dead, &snapshot.request_id);
            // CRASH: the provider outcome is durably known, but the
            // canonical Assistant message never committed.
        },
    );

    let (runtime, model) = runtime_with_model_at(
        &dir,
        "conv_0224cd71-1cda-74e4-8010-dad3ad2ce447",
        Vec::new(),
        vec![one_turn_script()],
    )
    .await
    .expect("runtime recovers");
    assert!(matches!(
        runtime.recovery().attempt_class(),
        crate::runtime::recovery::AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id,
            model_request: Some(_),
            ..
        } if attempt_id == &dead_attempt
    ));
    assert_eq!(
        runtime.recovery().resume(),
        crate::runtime::recovery::ResumeDisposition::PendingInboundOnly,
        "the answered model turn is never automatically continued"
    );

    runtime.activate();
    // A later user-driven turn proceeds according to the intended runtime
    // semantics — startup itself must not replay the missing Assistant
    // turn.
    runtime
        .submit_inbound(text_content("a new turn"))
        .expect("accepted after recovery");
    runtime.child_attempt_settlement(0).await;
    assert_eq!(
        model.requests().len(),
        1,
        "exactly the user-driven turn reached the provider; recovery initiated no replacement request"
    );
}

/// Issue #12 (M9a), Test L (runtime half): a restart never reuses an
/// `AttemptId` that already appears in durable history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_allocates_an_attempt_id_past_durable_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation = ConversationId::new("conv_d739e286-16f6-783d-89be-a52ec3d0651f");
    let mut durable_attempts = Vec::new();
    for ordinal in 0..3 {
        durable_attempts.push(AttemptId::for_conversation(&conversation, ordinal));
    }
    let seeded = durable_attempts.clone();
    let store_path = seed_crash_prefix(
        &dir,
        "conv_d739e286-16f6-783d-89be-a52ec3d0651f",
        &[],
        |store, id| {
            for (ordinal, attempt) in seeded.iter().enumerate() {
                store
                    .append_event(attempt_event(
                        id,
                        &format!("started-{ordinal}"),
                        attempt,
                        crate::events::types::RuntimeEvent::AttemptStarted {
                            attempt_id: attempt.clone(),
                        },
                    ))
                    .expect("attempt started");
                store
                    .append_event(attempt_event(
                        id,
                        &format!("completed-{ordinal}"),
                        attempt,
                        crate::events::types::RuntimeEvent::AttemptCompleted {
                            attempt_id: attempt.clone(),
                            finish_reason: crate::model::finish::ModelFinishReason::Stop,
                        },
                    ))
                    .expect("attempt completed");
            }
        },
    );

    let runtime = runtime_at(
        &dir,
        "conv_d739e286-16f6-783d-89be-a52ec3d0651f",
        Vec::new(),
        vec![one_turn_script()],
    )
    .await
    .expect("runtime recovers");
    assert_eq!(runtime.recovery().next_attempt_ordinal(), 3);
    runtime.activate();
    runtime
        .submit_inbound(text_content("after restart"))
        .expect("accepted");
    runtime.child_attempt_settlement(0).await;

    // The durable Event Journal is the proof: the new attempt's start
    // fact carries an identity that never appeared before.
    let store = crate::durable::SqliteConversationStore::open(conversation.clone(), &store_path)
        .expect("reopen");
    let mut started = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.read_events(cursor, 64).expect("events");
        if page.events.is_empty() {
            break;
        }
        for envelope in &page.events {
            if let crate::events::types::RuntimeEvent::AttemptStarted { attempt_id } =
                &envelope.event
            {
                started.push(attempt_id.clone());
            }
        }
        cursor = page.next_sequence;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(started.len(), 4, "one new attempt started: {started:?}");
    let new_attempt = started.last().expect("the new attempt");
    assert!(
        !durable_attempts.contains(new_attempt),
        "the restarted allocator reused a durable identity: {new_attempt}"
    );
    assert_eq!(new_attempt.conversation_ordinal(&conversation), Some(3));
}

// frontier, lifecycle drain, and child driver rather than a projected fixture.
async fn finish_finite_workflow_child(
    dir: &tempfile::TempDir,
    registry: &crate::runtime::subagent::SubagentRegistry,
    store: &crate::durable::SqliteConversationStore,
    abandon_publication: bool,
) -> crate::runtime::subagent::SubagentAccepted {
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        ActivationAdmission, AgentActivationOrigin, DurableAgentAuthority,
        InheritedExecutionPolicy, SubagentPublication, SubagentStartOutcome, SubagentStartSpec,
        SubagentTerminalMode,
    };
    let mut node = crate::runtime::workflow::test_instance("finite_recovery", "child");
    node.block.run.conversation_id = store.conversation_id().clone();
    let spec = SubagentStartSpec {
        authority: DurableAgentAuthority {
            title: "Explore task".into(),
            execution_policy: InheritedExecutionPolicy::default(),
            resolved: test_resolved_subagent("reviewer"),
            approval_mode: ApprovalMode::Policy,
        },
        admission: ActivationAdmission {
            attachments: Vec::new(),
            task: "finite structured child".into(),
            context: None,
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(node.clone()),
            },
            terminal: SubagentTerminalMode::WorkflowOutput {
                output_schema: serde_json::json!({"type":"object"}),
                workflow_id: crate::runtime::workflow::WorkflowId::parse("finite_recovery")
                    .unwrap(),
                run_id: node.block.run.clone(),
                node_id: Box::new(node),
            },
        },
    };
    let root = dir.path().join("finite-child");
    let (child, mut peer) = stage_runtime_test_child(&root);
    registry.push_staged_override(child);
    let cancellation = CancellationSignal::new();
    let prepared = registry.prepare(&spec, &cancellation).await.unwrap();
    let SubagentStartOutcome::Accepted(accepted) =
        registry.commit(prepared, &cancellation).await.unwrap()
    else {
        panic!("finite child ownership");
    };
    assert!(matches!(
        read_parent_frame(&mut peer).await.unwrap(),
        Some(ParentFrame::Delegate(_))
    ));
    // Delegate receipt is the ownership gate. No terminal can publish until
    // this peer supplies the result after the transaction fault is armed.
    if abandon_publication {
        store.arm_fail_event_times(3);
    }
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("{}".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    let settled = registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert!(
        !root.exists(),
        "the live native driver reaped and removed its incarnation"
    );
    assert_eq!(
        settled.settlement.publication,
        if abandon_publication {
            SubagentPublication::Abandoned
        } else {
            SubagentPublication::Committed
        }
    );
    accepted
}

#[tokio::test]
async fn finite_workflow_committed_physical_proof_survives_runtime_reopen() {
    use crate::events::types::SubagentOwnershipKind;
    use crate::runtime::subagent::{AgentControlError, SubagentState};
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, _, registry) = headless_runtime_over_store_with_subagents(
        &dir,
        conversation.as_str(),
        store.clone(),
        None,
    )
    .await;
    runtime.activate();
    let accepted = finish_finite_workflow_child(&dir, &registry, store.as_ref(), false).await;
    assert_eq!(registry.with_goal_idle(|| true), Some(true));
    runtime.shutdown().await.unwrap();
    drop(runtime);
    drop(registry);

    let (reopened, model, registry) =
        headless_runtime_over_store_with_subagents(&dir, conversation.as_str(), store, None).await;
    let snapshot = registry.snapshot(&accepted.subagent_id).unwrap();
    assert_eq!(snapshot.ownership, SubagentOwnershipKind::Workflow);
    assert_eq!(snapshot.state, SubagentState::Succeeded);
    assert!(snapshot.is_settled());
    assert_eq!(registry.with_goal_idle(|| true), Some(true));
    assert!(
        registry
            .list_agents(crate::runtime::subagent::MAX_AGENT_LIST_LIMIT)
            .agents
            .is_empty()
    );
    assert!(matches!(
        registry.wait_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Unknown(_))
    ));
    reopened.activate();
    assert!(
        model.requests().is_empty(),
        "finite history grants no resumed turn"
    );
    reopened.shutdown().await.unwrap();
    assert!(reopened.is_quiescent());
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One lease boundary covers live owner exclusion and both shutdown outcomes.
async fn finite_workflow_recovery_tracks_physical_obligation_until_exact_owner_proof() {
    use crate::events::types::{RuntimeEvent, SubagentOwnershipKind, SubagentTerminalState};
    use crate::runtime::subagent::physical_recovery::ChildPhysicalLease;
    use crate::runtime::subagent::{
        AgentControlError, SubagentPhysicalSettlement, SubagentPublication, SubagentState,
    };
    for publish_receipt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap(),
        );
        let (runtime, _, registry) = headless_runtime_over_store_with_subagents(
            &dir,
            conversation.as_str(),
            store.clone(),
            None,
        )
        .await;
        runtime.activate();
        let accepted = finish_finite_workflow_child(&dir, &registry, store.as_ref(), true).await;
        assert!(
            runtime.shutdown().await.is_err(),
            "the first owner lost its terminal publication"
        );
        drop(runtime);
        drop(registry);
        let incarnation = crate::runtime::subagent::child_conversation_store_path(
            &dir.path().join("subagents"),
            &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
            &accepted.child_conversation_id,
        )
        .parent()
        .unwrap()
        .join("physical-settlement")
        .join(accepted.subagent_id.as_str());
        std::fs::create_dir_all(&incarnation).unwrap();
        // The real original driver has already reaped. This fixture retains
        // the exact native receipt writer lease to prove a receipt alone can
        // never permit overlap with a surviving physical incarnation.
        let lease = ChildPhysicalLease::for_test(
            incarnation,
            accepted.subagent_id.clone(),
            accepted.child_conversation_id.clone(),
        )
        .unwrap();
        if publish_receipt {
            lease.publish_quiescent().unwrap();
        }
        tokio::time::pause();
        let (reopened, model, registry) = headless_runtime_over_store_with_subagents(
            &dir,
            conversation.as_str(),
            store.clone(),
            None,
        )
        .await;
        let recovered = registry.snapshot(&accepted.subagent_id).unwrap();
        assert_eq!(recovered.ownership, SubagentOwnershipKind::Workflow);
        assert_eq!(recovered.state, SubagentState::Interrupted);
        assert_eq!(
            recovered.settlement.publication,
            SubagentPublication::Committed
        );
        assert_eq!(
            recovered.settlement.physical,
            SubagentPhysicalSettlement::Unproven
        );
        assert_eq!(
            registry.with_goal_idle(|| true),
            None,
            "an occupied incarnation lease excludes proof even with a receipt"
        );
        assert_eq!(
            registry.unproven_settlements(),
            std::slice::from_ref(&accepted.subagent_id)
        );
        assert!(
            registry
                .list_agents(crate::runtime::subagent::MAX_AGENT_LIST_LIMIT)
                .agents
                .is_empty()
        );
        assert!(matches!(
            registry.wait_agent(&accepted.child_agent_id).await,
            Err(AgentControlError::Unknown(_))
        ));
        assert!(model.requests().is_empty());
        let drained = Arc::new(tokio::sync::Notify::new());
        *reopened.inner.probe.lock().unwrap() = Some(CoordinatorProbe {
            drain_linearization: Some(drained.clone()),
            ..CoordinatorProbe::default()
        });
        reopened.activate();
        let shutdown = tokio::spawn({
            let runtime = reopened.clone();
            async move { runtime.shutdown().await }
        });
        drained.notified().await;
        assert_eq!(
            reopened.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert!(!shutdown.is_finished());
        drop(lease);
        registry.reconcile_recovered_settlements();
        let events = store.read_events(None, 128).unwrap().events;
        assert!(events.iter().any(|event| matches!(
            event.event,
            RuntimeEvent::SubagentTerminalSettled {
                state: SubagentTerminalState::Interrupted,
                physical_settlement_proven: false,
                ..
            }
        )));
        let proofs = events.iter().filter(|event| matches!(&event.event, RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if subagent_id == &accepted.subagent_id)).count();
        assert_eq!(proofs, usize::from(publish_receipt));
        if publish_receipt {
            assert_eq!(registry.with_goal_idle(|| true), Some(true));
            shutdown.await.unwrap().unwrap();
            assert!(reopened.is_quiescent());
            let terminal = registry.snapshot(&accepted.subagent_id).unwrap();
            assert_eq!(
                terminal.state,
                SubagentState::Interrupted,
                "physical proof cannot manufacture the lost logical answer"
            );
            assert!(terminal.is_settled());
        } else {
            assert_eq!(
                registry.with_goal_idle(|| true),
                None,
                "a free lease without a quiescence receipt proves no descendant containment"
            );
            let error = shutdown.await.unwrap().unwrap_err();
            assert!(matches!(
                error,
                crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }
                    if detail.contains("physical settlement is unresolved")
            ));
            assert!(!reopened.is_quiescent());
        }
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "recovery never reattaches or replays the finite activation"
        );
        assert!(model.requests().is_empty());
        drop(reopened);
        drop(registry);
        tokio::time::resume();
    }
}

#[tokio::test]
async fn abandoned_workflow_publication_keeps_inspection_on_retained_history() {
    use crate::message::types::UserMessageBlock;
    use crate::runtime::identity::MessageId;
    use crate::runtime::subagent::{AgentState, SubagentPhysicalSettlement, SubagentPublication};
    use crate::runtime_client::agent_observation::{AgentObservation, ObservationSource};
    use crate::runtime_client::types::RuntimeClientResult;
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, _, registry) = headless_runtime_over_store_with_subagents(
        &dir,
        conversation.as_str(),
        store.clone(),
        None,
    )
    .await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    runtime.activate();
    let accepted = finish_finite_workflow_child(&dir, &registry, store.as_ref(), true).await;
    let child = registry.snapshot(&accepted.subagent_id).unwrap();
    assert_eq!(child.settlement.publication, SubagentPublication::Abandoned);
    assert_eq!(
        child.settlement.physical,
        SubagentPhysicalSettlement::Proven
    );
    assert!(!child.is_settled());
    let path = crate::runtime::subagent::child_conversation_store_path(
        &dir.path().join("subagents"),
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &accepted.child_conversation_id,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let history = crate::durable::SqliteConversationStore::open(
        accepted.child_conversation_id.clone(),
        &path,
    )
    .unwrap();
    history
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new("retained-workflow-input"),
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::default(),
            timestamp: None,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "retained diagnostic input".into(),
            })],
        }))
        .unwrap();
    let status = attachment.handle_request(RuntimeClientRequest::AgentStatus {
        id: RequestId::new(1),
        agent_id: accepted.child_agent_id.clone(),
    });
    let header: crate::runtime_client::snapshot::RuntimeClientAgent =
        crate::scripted_suites::common::native_domain_single(
            status
                .result
                .clone()
                .unwrap_or_else(|| panic!("status: {status:?}")),
        );
    assert_eq!(header.state, AgentState::Unavailable);
    assert_eq!(
        header.activation_state,
        crate::runtime::subagent::SubagentState::PublishingTerminal
    );
    let response = attachment
        .handle_request_async(RuntimeClientRequest::AgentObserve {
            id: RequestId::new(2),
            agent_id: accepted.child_agent_id,
            after: None,
        })
        .await;
    let Some(RuntimeClientResult::AgentObserve {
        observation: AgentObservation::Checkpoint {
            source, checkpoint, ..
        },
    }) = response.result
    else {
        panic!("retained inspection checkpoint: {response:?}");
    };
    assert_eq!(source, ObservationSource::DurableOnly);
    assert!(
        checkpoint
            .snapshot
            .messages
            .iter()
            .any(|message| message.id().as_str() == "retained-workflow-input")
    );
    assert!(
        runtime.shutdown().await.is_err(),
        "inspection must not clear publication uncertainty"
    );
}
