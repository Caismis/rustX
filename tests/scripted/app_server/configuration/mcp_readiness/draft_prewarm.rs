//! Draft ownership crosses the real native composition and loopback MCP/provider
//! boundaries. Gates and controlled time establish order, never elapsed sleeps.
use super::*;
use crate::local_runtime::composition::SubmissionIntent;
use crate::local_runtime::config::McpStartupPolicyDocument;
use crate::local_runtime::mcp_drafts::{McpDraftScope, McpDraftTarget};

fn target(generation: u32) -> McpDraftTarget {
    McpDraftTarget {
        draft_id: "tab-one".into(),
        generation,
        host_authority: "authorized-host".into(),
    }
}
fn select(fixture: &Fixture, scope: &McpDraftScope, generation: u32, workspace: usize) {
    scope
        .select(target(generation), fixture.workspaces[workspace].clone())
        .unwrap();
}
async fn submit(
    fixture: &mut Fixture,
    scope: &McpDraftScope,
    generation: u32,
    workspace: usize,
) -> SessionSnapshot {
    let claim = scope.claim(&target(generation), SubmissionIntent::now());
    let created = fixture
        .manager
        .create_session_submitted(
            SessionPersistentState {
                cwd: fixture.workspaces[workspace].clone(),
                model: None,
            },
            claim,
        )
        .await
        .unwrap();
    let session = fixture
        .manager
        .sessions
        .rename_session(&created.session.id, "Draft prewarm fixture")
        .await
        .unwrap();
    fixture.sessions.push(session.clone());
    session
}
async fn first_request(fixture: &Fixture, session: &SessionSnapshot, expected: &[&str]) {
    fixture.manager.load(&session.id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
    runtime.submit_inbound(input("request-A")).unwrap();
    fixture.gates[0].wait_entered().await;
    let bodies = fixture.provider.request_bodies();
    assert_eq!(bodies.len(), 1);
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let mut names: Vec<_> = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, expected);
    if expected.contains(&TOOL) {
        assert_eq!(tool_schema(&bodies[0]).as_deref(), Some("string"));
    }
    fixture.gates[0].release();
}
fn configure_other(fixture: &Fixture, mcp: &McpFixture) {
    let workspace = &fixture.workspaces[1];
    std::fs::create_dir_all(workspace.join(".agents")).unwrap();
    std::fs::write(
        workspace.join(".agents/mcp.toml"),
        format!("[mcp_servers.late]\nurl = {:?}\n", mcp.endpoint),
    )
    .unwrap();
    let path = workspace.join("rustx.toml");
    let mut document = std::fs::read_to_string(&path).unwrap();
    document.push_str("\n[agent.tools.sources]\nlate = 'all'\n");
    std::fs::write(path, document).unwrap();
}
fn only_mcp(fixture: &Fixture, workspace: usize) {
    let path = fixture.workspaces[workspace].join("rustx.toml");
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    document["agent"]["tools"]
        .as_table_mut()
        .unwrap()
        .insert("builtin".into(), toml::Value::Array(Vec::new()));
    std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
}

#[tokio::test]
async fn unsent_ready_draft_has_no_session_or_deadline_and_transfers_one_live_catalog() {
    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    only_mcp(&fixture, 0);
    let scope = fixture.manager.mcp_draft_scope();
    select(&fixture, &scope, 1, 0);
    let startup = scope.prepared_for_test().await;
    assert!(
        fixture
            .manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(fixture.provider.request_bodies().is_empty());
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    assert!(startup.deadline().deadline().is_none());
    select(&fixture, &scope, 1, 0); // Same identity is idempotent.
    tokio::time::resume();
    let session = submit(&mut fixture, &scope, 1, 0).await;
    fixture.manager.load(&session.id, None).await.unwrap();
    scope.cancel(&target(1));
    scope.close(); // Transfer already won.
    first_request(&fixture, &session, &[TOOL]).await;
    assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
    fixture.close().await;
    mcp.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_ack_precedes_validated_transfer_without_releasing_the_reserved_generation() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        let mut transferred = scope.transfer_for_test();
        let manager = fixture.manager.clone();
        let (parked, probe) = tokio::sync::oneshot::channel();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert("create_before_load", move || {
                let id = manager
                    .registry
                    .0
                    .lock()
                    .unwrap()
                    .entries
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                let probe = manager.probe(&id);
                probe.before_compose.arm();
                parked.send(probe).unwrap();
            });
        let created = fixture
            .manager
            .create_session_submitted(
                SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: None,
                },
                scope.claim(&target(1), SubmissionIntent::now()),
            )
            .await
            .unwrap();
        let probe = probe.await.unwrap();
        probe.before_compose.entered().await;
        assert!(!*transferred.borrow(), "the ACK does not imply transfer");
        assert_eq!(fixture.manager.diagnostics().loading, 1);
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
        probe.before_compose.release();
        transferred
            .wait_for(|transferred| *transferred)
            .await
            .unwrap();
        let session = fixture
            .manager
            .sessions
            .rename_session(&created.session.id, "ACK before native transfer")
            .await
            .unwrap();
        fixture.sessions.push(session.clone());
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test]
async fn sixty_seconds_preparation_then_submission_has_the_full_budget_on_the_same_task() {
    let mcp = McpFixture::start().await;
    mcp.control.gate.arm();
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    only_mcp(&fixture, 0);
    let scope = fixture.manager.mcp_draft_scope();
    select(&fixture, &scope, 1, 0);
    mcp.control.gate.entered().await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    let received = tokio::time::Instant::now();
    let claim = scope.claim(&target(1), SubmissionIntent::now());
    claim
        .startup
        .bind_policy(McpStartupPolicyDocument::default())
        .unwrap();
    let startup = claim.startup.clone();
    assert_eq!(
        startup.deadline().deadline(),
        Some(received + std::time::Duration::from_secs(15))
    );
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    assert!(!startup.deadline().has_expired());
    assert!(fixture.provider.request_bodies().is_empty());
    mcp.control.gate.release();
    tokio::time::resume();
    let created = fixture
        .manager
        .create_session_submitted(
            SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            },
            claim,
        )
        .await
        .unwrap();
    let session = fixture
        .manager
        .sessions
        .rename_session(&created.session.id, "One shared deadline")
        .await
        .unwrap();
    fixture.sessions.push(session.clone());
    first_request(&fixture, &session, &[TOOL]).await;
    assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
    assert_eq!(
        startup.deadline().deadline(),
        Some(received + std::time::Duration::from_secs(15))
    );
    fixture.close().await;
    mcp.close().await;
}

#[tokio::test]
async fn submission_timeout_survives_handoff_and_cannot_resurrect_late_ready() {
    let mcp = McpFixture::start().await;
    mcp.control.gate.arm();
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    only_mcp(&fixture, 0);
    let scope = fixture.manager.mcp_draft_scope();
    select(&fixture, &scope, 1, 0);
    mcp.control.gate.entered().await;
    let mut transferred = scope.transfer_for_test();
    let claim = scope.claim(&target(1), SubmissionIntent::now());
    let startup = claim.startup.clone();
    let created = fixture
        .manager
        .create_session_submitted(
            SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            },
            claim,
        )
        .await
        .unwrap();
    let session = fixture
        .manager
        .sessions
        .rename_session(&created.session.id, "Timed out draft")
        .await
        .unwrap();
    fixture.sessions.push(session.clone());
    let deadline = startup.deadline().deadline().unwrap();
    transferred
        .wait_for(|transferred| *transferred)
        .await
        .unwrap();
    scope.close();
    scope.cancel(&target(1));
    tokio::time::pause();
    tokio::time::advance(deadline.saturating_duration_since(tokio::time::Instant::now())).await;
    assert!(startup.deadline().has_expired());
    // Release after expiry, before requiring the timeout worker to have polled.
    // The native terminal fence must reject this ordering too.
    mcp.control.gate.release();
    tokio::time::resume();
    first_request(&fixture, &session, &[]).await;
    let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
    assert!(matches!(
        runtime
            .capability()
            .availability()
            .get(&crate::capabilities::ToolSourceId::Mcp(McpServerId::new(
                "late"
            ))),
        Some(crate::capabilities::CapabilitySourceState::Unavailable { .. })
    ));
    assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
    fixture.close().await;
    mcp.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latest_workspace_wins_and_ready_unclaimed_resources_retire_before_replacement() {
    bounded(async {
        let a = McpFixture::start().await;
        let b = McpFixture::named("workspace_b_tool").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &a)]);
        configure_other(&fixture, &b);
        only_mcp(&fixture, 0);
        only_mcp(&fixture, 1);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        select(&fixture, &scope, 2, 1);
        scope.prepared_for_test().await;
        select(&fixture, &scope, 3, 0);
        scope.prepared_for_test().await;
        assert!(
            scope
                .select(target(1), fixture.workspaces[0].clone())
                .is_err()
        );
        scope.cancel(&target(2));
        let session = submit(&mut fixture, &scope, 3, 0).await;
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(a.control.discovers.load(Ordering::SeqCst), 2);
        assert_eq!(a.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(b.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(b.control.lists.load(Ordering::SeqCst), 1);
        fixture.close().await;
        a.close().await;
        b.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_preparation_uses_normal_initialization_and_tabs_cannot_claim_each_other() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let first = fixture.manager.mcp_draft_scope();
        let other = fixture.manager.mcp_draft_scope();
        select(&fixture, &first, 1, 0);
        first.prepared_for_test().await;
        // Same target bytes on another native connection carry no ownership.
        let session = submit(&mut fixture, &other, 1, 0).await;
        first_request(&fixture, &session, &[TOOL]).await;
        first.cancel(&target(1));
        first.close();
        other.close();
        fixture.manager.mcp_drafts.settle().await;
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_binding_retires_stale_draft_and_initializes_the_actual_session_inputs() {
    bounded(async {
        let a = McpFixture::start().await;
        let b = McpFixture::named("workspace_b_tool").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &a)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        std::fs::write(
            fixture.workspaces[0].join(".agents/mcp.toml"),
            format!("[mcp_servers.late]\nurl = {:?}\n", b.endpoint),
        )
        .unwrap();
        let session = submit(&mut fixture, &scope, 1, 0).await;
        first_request(&fixture, &session, &["workspace_b_tool"]).await;
        assert_eq!(a.control.lists.load(Ordering::SeqCst), 1);
        assert_eq!(b.control.lists.load(Ordering::SeqCst), 1);
        fixture.close().await;
        a.close().await;
        b.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_a_pending_listing_settles_the_old_owner_before_preparing_the_latest_workspace() {
    bounded(async {
        let a = McpFixture::start().await;
        a.control.gate.arm();
        let b = McpFixture::named("workspace_b_tool").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &a)]);
        configure_other(&fixture, &b);
        only_mcp(&fixture, 0);
        only_mcp(&fixture, 1);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        a.control.gate.entered().await;
        select(&fixture, &scope, 2, 1);
        select(&fixture, &scope, 3, 0);
        select(&fixture, &scope, 4, 1);
        scope.prepared_for_test().await;
        // A is still held remotely. Native cancellation, not its successful
        // response or a discarded browser promise, permitted B to start.
        assert_eq!(a.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(b.control.discovers.load(Ordering::SeqCst), 1);
        let session = submit(&mut fixture, &scope, 4, 1).await;
        first_request(&fixture, &session, &["workspace_b_tool"]).await;
        a.control.gate.release();
        fixture.close().await;
        a.close().await;
        b.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_session_creation_settles_reserved_preparation_without_a_durable_session() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        let claim = scope.claim(&target(1), SubmissionIntent::now());
        let result = fixture
            .manager
            .create_session_submitted(
                SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: Some(crate::model::session::SessionModelConfig::of(
                        crate::model::catalog::ModelRef::parse("missing/model").unwrap(),
                    )),
                },
                claim,
            )
            .await;
        assert!(result.is_err());
        assert!(fixture.manager.mcp_drafts.settle().await.is_empty());
        assert!(fixture.manager.mcp_drafts.is_settled());
        assert!(
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
        assert!(fixture.provider.request_bodies().is_empty());
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_validated_transfer_uses_normal_startup_under_the_original_deadline() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        let claim = scope.claim(&target(1), SubmissionIntent::now());
        let startup = claim.startup.clone();
        // The reservation is not a Conversation handoff. Cancel wins before
        // target configuration validation and the transfer lock can commit.
        scope.cancel(&target(1));
        let created = fixture
            .manager
            .create_session_submitted(
                SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: None,
                },
                claim,
            )
            .await
            .unwrap();
        let deadline = startup.deadline().deadline();
        let session = fixture
            .manager
            .sessions
            .rename_session(&created.session.id, "Cancelled reservation")
            .await
            .unwrap();
        fixture.sessions.push(session.clone());
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(startup.deadline().deadline(), deadline);
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test]
async fn capacity_queueing_uses_the_same_submission_deadline_without_starting_a_process() {
    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    only_mcp(&fixture, 0);
    let capacity = fixture.manager.mcp_drafts.reserve_all_for_test().await;
    let scope = fixture.manager.mcp_draft_scope();
    select(&fixture, &scope, 1, 0);
    let mut waiting = scope.capacity_wait_for_test();
    waiting.wait_for(|waiting| *waiting).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    let received = tokio::time::Instant::now();
    let claim = scope.claim(&target(1), SubmissionIntent::now());
    claim
        .startup
        .bind_policy(McpStartupPolicyDocument::default())
        .unwrap();
    let startup = claim.startup.clone();
    tokio::time::advance(std::time::Duration::from_secs(15)).await;
    tokio::time::resume();
    let created = fixture
        .manager
        .create_session_submitted(
            SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            },
            claim,
        )
        .await
        .unwrap();
    let session = fixture
        .manager
        .sessions
        .rename_session(&created.session.id, "Capacity timeout")
        .await
        .unwrap();
    fixture.sessions.push(session.clone());
    drop(capacity);
    first_request(&fixture, &session, &[]).await;
    assert_eq!(
        startup.deadline().deadline(),
        Some(received + std::time::Duration::from_secs(15))
    );
    assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 0);
    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 0);
    fixture.close().await;
    mcp.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_directory_at_the_same_path_cannot_reuse_the_old_physical_workspace() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        scope.prepared_for_test().await;
        let workspace = &fixture.workspaces[0];
        let previous = workspace.with_extension("previous");
        std::fs::rename(workspace, &previous).unwrap();
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        for path in ["rustx.toml", ".agents/mcp.toml"] {
            std::fs::copy(previous.join(path), workspace.join(path)).unwrap();
        }
        let session = submit(&mut fixture, &scope, 1, 0).await;
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submitted_failure_before_attach_is_terminal_until_an_explicit_new_submission() {
    bounded(async {
        let mcp = McpFixture::named("wrong_tool").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let path = fixture.workspaces[0].join("rustx.toml");
        let mut document: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        document["agent"]["tools"]["sources"]["late"] = toml::Value::Array(vec![TOOL.into()]);
        std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        let startup = scope.prepared_for_test().await;
        let session = submit(&mut fixture, &scope, 1, 0).await;
        let flight = {
            let registry = fixture.manager.registry.0.lock().unwrap();
            match &registry.entries[&session.active_conversation_id] {
                Entry::Loading(flight) | Entry::FailedSubmission(flight) => flight.clone(),
                entry => panic!("required selection must fail, got {entry:?}"),
            }
        };
        let failure = flight.wait().await.operation_result().unwrap_err();
        let deadline = startup.deadline().deadline();
        for _ in 0..3 {
            assert_eq!(
                fixture.manager.load(&session.id, None).await.unwrap_err(),
                failure
            );
        }
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
        assert_eq!(startup.deadline().deadline(), deadline);
        assert!(fixture.provider.request_bodies().is_empty());
        let connection =
            crate::app_server::connection::AppServerConnection::new(fixture.host.clone());
        super::super::super::protocol::initialize(&connection).await;
        for (attempt, recovery) in [false, true].into_iter().enumerate() {
            if recovery {
                super::super::super::protocol::call(
                    &connection,
                    30,
                    crate::app_server::protocol::Method::SessionRetryInitialization {
                        session_id: session.id.clone(),
                    },
                )
                .await;
            } else {
                let receipt = fixture
                    .manager
                    .accept_submission_intent(
                        &session.id,
                        &session.active_node,
                        SubmissionIntent::now(),
                    )
                    .unwrap();
                fixture
                    .manager
                    .record_submission_intent(receipt)
                    .await
                    .unwrap();
            }
            let flight = {
                let registry = fixture.manager.registry.0.lock().unwrap();
                match &registry.entries[&session.active_conversation_id] {
                    Entry::Loading(flight) | Entry::FailedSubmission(flight) => flight.clone(),
                    entry => panic!("retry must remain failed, got {entry:?}"),
                }
            };
            let failure = flight.wait().await.operation_result().unwrap_err();
            for _ in 0..2 {
                assert_eq!(
                    fixture.manager.load(&session.id, None).await.unwrap_err(),
                    failure
                );
            }
            assert_eq!(mcp.control.lists.load(Ordering::SeqCst), attempt + 2);
        }
        *mcp.control.name.lock().unwrap() = TOOL;
        super::super::super::protocol::call(
            &connection,
            31,
            crate::app_server::protocol::Method::SessionRetryInitialization {
                session_id: session.id.clone(),
            },
        )
        .await;
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 4);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_submission_failure_releases_residency_and_all_recovery_paths_reserve_again() {
    bounded(async {
        for recovery in ["retry", "submission", "replace"] {
            let mcp = McpFixture::named("wrong_tool").await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .policy
                .max_resident_runtimes = 1;
            configure(&fixture, &[("late", &mcp)]);
            only_mcp(&fixture, 0);
            let path = fixture.workspaces[0].join("rustx.toml");
            let mut document: toml::Value =
                std::fs::read_to_string(&path).unwrap().parse().unwrap();
            document["agent"]["tools"]["sources"]["late"] = toml::Value::Array(vec![TOOL.into()]);
            std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
            let scope = fixture.manager.mcp_draft_scope();
            select(&fixture, &scope, 1, 0);
            scope.prepared_for_test().await;
            let a = submit(&mut fixture, &scope, 1, 0).await;
            let failure = fixture.manager.load(&a.id, None).await.unwrap_err();
            let diagnostics = fixture.manager.diagnostics();
            assert_eq!(
                diagnostics.loaded + diagnostics.loading + diagnostics.unloading,
                0
            );

            // A known WriterAbsent failure must not block another submitted
            // creation or the ordinary cold-load admission path.
            let other = fixture.manager.mcp_draft_scope();
            let b = submit(&mut fixture, &other, 1, 1).await;
            let resident = fixture.manager.load(&b.id, None).await.unwrap();
            assert_eq!(fixture.manager.diagnostics().loaded, 1);
            assert_eq!(
                fixture.manager.load(&a.id, None).await.unwrap_err(),
                failure
            );
            assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
            for available in [false, true] {
                if available {
                    fixture
                        .manager
                        .unload(resident.conversation_id())
                        .await
                        .unwrap();
                    // Prove ordinary load also reclaims B's physically absent
                    // slot while A's retained error remains observable.
                    let reloaded = fixture.manager.load(&b.id, None).await.unwrap();
                    fixture
                        .manager
                        .unload(reloaded.conversation_id())
                        .await
                        .unwrap();
                    *mcp.control.name.lock().unwrap() = TOOL;
                }
                let result = match recovery {
                    "retry" => fixture
                        .manager
                        .retry_failed_initialization(&a.id, SubmissionIntent::now()),
                    "submission" => {
                        let receipt = fixture
                            .manager
                            .accept_submission_intent(
                                &a.id,
                                &a.active_node,
                                SubmissionIntent::now(),
                            )
                            .unwrap();
                        fixture.manager.record_submission_intent(receipt).await
                    }
                    "replace" => fixture.manager.replace(&a.id, None).await.map(|_| ()),
                    _ => unreachable!(),
                };
                if available {
                    result.unwrap();
                    first_request(&fixture, &a, &[TOOL]).await;
                    assert_eq!(fixture.manager.diagnostics().loaded, 1);
                    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
                } else {
                    assert_eq!(
                        result,
                        Err(RuntimeManagerError::ResidencyCapacity),
                        "{recovery}"
                    );
                    assert_eq!(
                        fixture.manager.load(&a.id, None).await.unwrap_err(),
                        failure
                    );
                    assert_eq!(fixture.manager.diagnostics().loaded, 1);
                    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
                    assert!(fixture.provider.request_bodies().is_empty());
                }
            }
            fixture.close().await;
            mcp.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_preparation_rechecks_physical_workspace_after_capacity_and_before_transfer() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let capacity = fixture.manager.mcp_drafts.reserve_all_for_test().await;
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        let mut waiting = scope.capacity_wait_for_test();
        waiting.wait_for(|waiting| *waiting).await.unwrap();
        let mut consuming = scope.consume_wait_for_test();
        let transferred = scope.transfer_for_test();
        let session = submit(&mut fixture, &scope, 1, 0).await;
        consuming.wait_for(|consuming| *consuming).await.unwrap();
        let workspace = &fixture.workspaces[0];
        let previous = workspace.with_extension("previous");
        std::fs::rename(workspace, &previous).unwrap();
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        for path in ["rustx.toml", ".agents/mcp.toml"] {
            std::fs::copy(previous.join(path), workspace.join(path)).unwrap();
        }
        drop(capacity);
        first_request(&fixture, &session, &[TOOL]).await;
        assert!(
            !*transferred.borrow(),
            "the queued stale physical owner never transfers"
        );
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credential_rotation_retires_preparation_while_model_only_changes_reuse_it() {
    bounded(async {
        for rotate_credentials in [false, true] {
            let mcp = McpFixture::start().await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            configure(&fixture, &[("late", &mcp)]);
            only_mcp(&fixture, 0);
            std::fs::write(
                fixture.workspaces[0].join(".agents/mcp.toml"),
                format!("[mcp_servers.late]\nurl = {:?}\n[mcp_servers.late.sensitive_headers]\nAuthorization = '$DRAFT_MCP_KEY'\n", mcp.endpoint),
            ).unwrap();
            let credentials = |value: &str| crate::credentials::CredentialSnapshot::new([
                ("TEST_KEY".into(), "fixture".into()), ("DRAFT_MCP_KEY".into(), value.into()),
            ]);
            fixture.manager.credentials = credentials("draft-secret-before");
            let scope = fixture.manager.mcp_draft_scope();
            select(&fixture, &scope, 1, 0);
            scope.prepared_for_test().await;
            let transferred = scope.transfer_for_test();
            if rotate_credentials {
                fixture.manager.credentials = credentials("draft-secret-after");
            }
            let created = fixture.manager.create_session_submitted(
                SessionPersistentState { cwd: fixture.workspaces[0].clone(), model: Some(crate::model::session::SessionModelConfig::of(crate::model::catalog::ModelRef::parse("local/b").unwrap())) },
                scope.claim(&target(1), SubmissionIntent::now()),
            ).await.unwrap();
            let session = fixture.manager.sessions.rename_session(&created.session.id, "Model and credential boundaries").await.unwrap();
            fixture.sessions.push(session.clone());
            first_request(&fixture, &session, &[TOOL]).await;
            assert_eq!(*transferred.borrow(), !rotate_credentials);
            assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), if rotate_credentials { 2 } else { 1 });
            assert_eq!(mcp.control.lists.load(Ordering::SeqCst), if rotate_credentials { 2 } else { 1 });
            let request: serde_json::Value = serde_json::from_str(&fixture.provider.request_bodies()[0]).unwrap();
            assert_eq!(request["model"], "b");
            let diagnostics = serde_json::to_string(&fixture.manager.mcp_drafts.diagnostics()).unwrap();
            assert!(!diagnostics.contains("draft-secret"));
            fixture.close().await;
            mcp.close().await;
        }
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_creation_durability_warning_keeps_the_same_preparation_and_send_clock() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        only_mcp(&fixture, 0);
        let scope = fixture.manager.mcp_draft_scope();
        select(&fixture, &scope, 1, 0);
        let startup = scope.prepared_for_test().await;
        fixture
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        let claim = scope.claim(&target(1), SubmissionIntent::now());
        let created = fixture
            .manager
            .create_session_submitted(
                SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: None,
                },
                claim,
            )
            .await
            .unwrap();
        assert!(created.durability_diagnostic.is_some());
        let deadline = startup.deadline().deadline();
        assert!(deadline.is_some());
        let session = fixture
            .manager
            .sessions
            .rename_session(&created.session.id, "Committed with durability warning")
            .await
            .unwrap();
        fixture.sessions.push(session.clone());
        first_request(&fixture, &session, &[TOOL]).await;
        assert_eq!(startup.deadline().deadline(), deadline);
        assert_eq!(mcp.control.discovers.load(Ordering::SeqCst), 1);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}
