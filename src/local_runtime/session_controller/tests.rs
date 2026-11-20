use crate::durable::{CanonicalStore, JournalStore, ProjectionStore};
use crate::message::types::UserContentBlock;
include!("cold_statistics_tests.rs");
use super::super::configuration::SessionConfigInput;
use super::super::session::deletion::SessionDeleteResult;
use super::*;
/// Durable Session creation is independent of execution: while another
/// Session's runtime holds an in-flight provider request, the native
/// catalog owner still publishes a new Session without touching that
/// runtime or issuing a model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_create_publishes_while_another_session_provider_is_blocked() {
    use crate::model::{ModelEvent, ModelFinishReason};
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
    };
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep, model_release};

    let (release_tx, release_rx) = model_release();
    let catalog_root = tempfile::tempdir().expect("catalog root");
    let template = SessionPersistentState::from_input(
        &crate::local_runtime::SessionConfigInput::new(catalog_root.path().to_path_buf()),
    );
    let catalog = SessionCatalog::create(catalog_root.path(), &template).expect("catalog");
    let adapter = Arc::new(FakeModel::new(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]));
    let tools =
        crate::scripted_suites::common::tool_runtime("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let capabilities = crate::scripted_suites::common::capability_lease(
        crate::tools::executor::ToolRegistry::new(),
        &tools,
    )
    .await;
    let coordinator = capabilities.coordinator.clone();
    let runtime = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: crate::runtime::identity::AgentId::new("agent-a"),
        model: crate::scripted_suites::support::model::scripted_session_model(adapter.clone()),
        approval_mode: crate::runtime::ApprovalMode::Plan,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            status_clock: Arc::new(crate::context::SystemClock),

            estimator: Arc::new(crate::context::DefaultTokenEstimator),
            status_engine: None,
        },
        tool_runtime: (*tools).clone(),
        resources: Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            coordinator.current_snapshot(),
        )),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })
    .expect("runtime");
    let controller = crate::local_runtime::session_controller::SessionController::new(catalog);
    runtime.activate();
    runtime
        .submit_inbound(vec![UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "unsettled turn".into(),
            },
        )])
        .unwrap();
    let mut parked = adapter.parked();
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        parked.wait_for(|value| *value),
    )
    .await
    .expect("provider must park")
    .expect("provider gate stays open");
    assert_eq!(adapter.requests().len(), 1);
    assert!(runtime.has_current_attempt());
    let before = controller.list_sessions(None, 0, 8).await.unwrap().sessions;
    let created = controller.create_session(template).await.unwrap();
    assert!(before.iter().all(|row| row.id != created.session.id));
    assert_eq!(
        SessionCatalog::open_existing(catalog_root.path())
            .unwrap()
            .unwrap()
            .persisted_session_ids()
            .len(),
        2
    );
    assert_eq!(adapter.requests().len(), 1);
    assert!(runtime.has_current_attempt());
    let _ = release_tx.send(true);
    runtime.shutdown().await.unwrap();
}

fn settings(path: &std::path::Path) -> SessionPersistentState {
    SessionPersistentState::from_input(&SessionConfigInput::new(path.to_path_buf()))
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercise a retry family across real durable copies and reopen.
async fn message_versions_follow_inherited_prompts_and_survive_reopen() {
    use crate::local_runtime::session::LineageSide;
    use crate::local_runtime::session::observation::{
        SessionMessageVersion, SessionMessageVersionFamily,
    };
    use crate::message::types::{MessageBlock, UserSource};
    use crate::observation::content::CanonicalContentLocator;

    async fn append(
        controller: &SessionController,
        session: &SessionSnapshot,
        message: MessageBlock,
    ) {
        let path = controller
            .catalog
            .lock()
            .await
            .database_path(&session.id, &session.active_conversation_id);
        crate::durable::SqliteConversationStore::open(
            session.active_conversation_id.clone(),
            &path,
        )
        .unwrap()
        .append_canonical(&message)
        .unwrap();
    }
    async fn branch(
        controller: &SessionController,
        session: &SessionId,
        version: &SessionMessageVersion,
    ) -> SessionSnapshot {
        let message = controller
            .resolve_boundary(
                session,
                Some(&version.node_id),
                version.surface_revision,
                &version.boundary,
                LineageSide::Before,
            )
            .await
            .unwrap();
        if let Some(expected) = &version.message_id {
            assert_eq!(&message, expected);
        }
        controller
            .branch_session_node(
                session,
                &version.node_id,
                version.surface_revision,
                &message,
            )
            .await
            .unwrap()
            .session
    }
    async fn boundary(
        controller: &SessionController,
        session: &SessionSnapshot,
        position: u64,
    ) -> SessionMessageVersion {
        controller
            .message_boundary(
                &session.id,
                session.active_node.clone(),
                CanonicalContentLocator::ByLedgerPosition { position },
            )
            .await
            .unwrap()
            .unwrap()
    }

    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let original = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let oversized_id = "long native identity ".repeat(32);
    append(&controller, &original, user(&oversized_id)).await;
    assert!(
        boundary(&controller, &original, 1)
            .await
            .message_id
            .is_none()
    );
    append(&controller, &original, user("original prompt")).await;
    append(&controller, &original, user("original suffix")).await;
    let original_prompt = boundary(&controller, &original, 2).await;

    let blank = branch(&controller, &original.id, &original_prompt).await;
    let retry = branch(&controller, &original.id, &original_prompt).await;
    let query = |node: &SessionSnapshot, offset| {
        controller.message_versions(node.id.clone(), node.active_node.clone(), offset, 1)
    };
    assert_eq!(
        query(&original, 0).await.unwrap().versions,
        Vec::<SessionMessageVersionFamily>::new()
    );
    assert_eq!(
        query(&blank, 0).await.unwrap().versions,
        Vec::<SessionMessageVersionFamily>::new()
    );
    let MessageBlock::User(mut fleet_input) = user("fleet input") else {
        unreachable!()
    };
    fleet_input.source = UserSource::Fleet;
    append(&controller, &retry, MessageBlock::User(fleet_input)).await;
    assert_eq!(
        query(&original, 0).await.unwrap().versions,
        Vec::<SessionMessageVersionFamily>::new()
    );
    assert!(
        controller
            .message_boundary(
                &retry.id,
                retry.active_node.clone(),
                original_prompt.boundary.clone()
            )
            .await
            .unwrap()
            .is_none()
    );
    // The first real prompt continues the cut, including recovery after an
    // empty editor or an unsuccessful submit.
    append(&controller, &retry, user("replacement prompt")).await;
    append(&controller, &retry, user("later prompt")).await;
    let replacement = boundary(&controller, &retry, 3).await;
    let later = boundary(&controller, &retry, 4).await;

    let later_retry = branch(&controller, &retry.id, &later).await;
    append(&controller, &later_retry, user("later replacement")).await;
    let inherited = boundary(&controller, &later_retry, 3).await;
    assert_ne!(inherited.message_id, replacement.message_id);
    let second_retry = branch(&controller, &later_retry.id, &inherited).await;
    append(&controller, &second_retry, user("second replacement")).await;
    let second_replacement = boundary(&controller, &second_retry, 3).await;

    let earlier = query(&later_retry, 0).await.unwrap();
    assert_eq!(earlier.next_offset, Some(1));
    assert_eq!(earlier.versions[0].current, inherited);
    assert_eq!(earlier.versions[0].current_index, 1);
    assert_eq!(earlier.versions[0].total, 3);
    assert_eq!(
        earlier.versions[0].previous.as_ref(),
        Some(&original_prompt)
    );
    assert_eq!(earlier.versions[0].next.as_ref(), Some(&second_replacement));
    let later_page = query(&later_retry, 1).await.unwrap();
    assert_eq!(later_page.next_offset, None);
    assert_eq!(later_page.versions[0].current_index, 1);
    assert_eq!(later_page.versions[0].total, 2);
    assert_eq!(later_page.versions[0].previous.as_ref(), Some(&later));

    let independent = controller
        .fork_session(
            &retry.id,
            Some(&retry.active_node),
            later.surface_revision,
            None,
        )
        .await
        .unwrap()
        .session;
    assert_eq!(
        query(&independent, 0).await.unwrap().versions,
        Vec::<SessionMessageVersionFamily>::new()
    );
    assert!(matches!(
        controller
            .message_versions(independent.id, retry.active_node, 0, 1)
            .await,
        Err(SessionError::UnknownNode { .. })
    ));
    drop(controller);

    let controller = SessionController::open(root.path()).unwrap();
    let page = controller
        .message_versions(original.id.clone(), original.active_node, 0, 32)
        .await
        .unwrap();
    assert_eq!(page.versions.len(), 1);
    assert_eq!(page.versions[0].current, original_prompt);
    assert_eq!(page.versions[0].current_index, 0);
    assert_eq!(page.versions[0].next.as_ref(), Some(&replacement));
    assert_eq!(page.versions[0].total, 3);
}

#[tokio::test]
async fn committed_delete_rejects_load_while_cleanup_releases_catalog_mutex() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await else {
        panic!("unloaded Session is deletable")
    };
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = gate.arm_scoped();
    *controller.cleanup_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let id = a.id.clone();
    let delete =
        tokio::spawn(async move { worker.delete_session(&id, &preview.target_revision).await });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    assert!(controller.catalog.try_lock().is_ok());
    assert!(matches!(
        controller.acquire_session(&a.id, None).await,
        Err(SessionError::DeletingSession { .. })
    ));
    let access_b = controller.acquire_session(&b.id, None).await.unwrap();
    controller
        .rename_session(&b.id, "during cleanup")
        .await
        .unwrap();
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(!delete.is_finished());
    drop(release);
    assert!(matches!(
        delete.await.unwrap().unwrap(),
        SessionDeleteResult::Deleted { .. }
    ));
    assert_eq!(access_b.session.id, b.id);
}
#[tokio::test]
async fn cancelled_create_retains_allocation_exclusion_until_preparation_finishes() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = gate.arm_scoped();
    *controller.create_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let input = settings(root.path());
    let create = tokio::spawn(async move { worker.create_session(input).await });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    let reserved: Vec<_> = std::fs::read_dir(root.path().join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    create.abort();
    assert!(create.await.unwrap_err().is_cancelled());
    assert!(controller.preparation.try_lock().is_err());
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
    *controller.create_gate.lock().unwrap() = None;
    drop(release);
    let created = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    // The cancelled preparation's private storage cannot be reused or published.
    assert!(!reserved.iter().any(|name| name == created.id.as_str()));
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .len(),
        1
    );
}
#[tokio::test]
async fn independent_sessions_reopen_without_focus_or_runtime_effects() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("rustx.toml"), "malformed = [").unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
    let a = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    controller.rename_session(&a.id, "A renamed").await.unwrap();
    assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
    let (reads, effects) = super::super::static_effects::measure(|| {
        let catalog = controller.catalog.try_lock().unwrap();
        (
            catalog.snapshot(&a.id).unwrap(),
            catalog.list_page(None, 0, 32).unwrap(),
        )
    });
    assert_eq!(effects, [0; 11]);
    assert_eq!(reads.1.sessions.len(), 2);
    let bytes = std::fs::read(root.path().join("sessions/catalog.json")).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["schema_version"],
        crate::local_runtime::session::SESSION_CATALOG_SCHEMA_VERSION
    );
    assert!(json.get("active_session").is_none());
    assert!(
        serde_json::to_value(&reads.1.sessions)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("active").is_none())
    );
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
    assert_eq!(controller.read_session(&a.id).await.unwrap(), reads.0);
    assert_eq!(
        std::fs::read(root.path().join("sessions/catalog.json")).unwrap(),
        bytes
    );
}
#[tokio::test]
async fn cold_trace_reads_without_runtime_or_valid_configuration() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    std::fs::write(root.path().join("rustx.toml"), "malformed = [").unwrap();
    let (conversation, page) = controller
        .read_trace(&session.id, None, |projection| projection.page(None, 32))
        .await
        .unwrap();
    assert_eq!(conversation, session.active_conversation_id);
    assert_eq!(
        page.records,
        [] as [crate::observation::trace::types::TraceRecord; 0]
    );
    let (_, detail) = controller
        .read_trace(&session.id, None, |projection| {
            projection.detail("trace:999")
        })
        .await
        .unwrap();
    assert!(detail.is_none());
    assert!(
        controller
            .read_trace(&session.id, None, |projection| projection.page(None, 33))
            .await
            .is_err()
    );
    let SessionStatisticsReading {
        conversation_id: owner,
        statistics,
        occupancy,
        read_cut,
    } = controller
        .read_statistics(&session.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_cut.conversation_id, owner);
    assert!(read_cut.activation_id.is_none());
    assert_eq!(owner, session.active_conversation_id);
    assert_eq!(statistics.turns, 0);
    assert!(occupancy.is_none());
    assert!(controller.runtime_owner.get().is_none());
}
#[tokio::test]
async fn bounded_cwd_projection_reads_the_only_durable_owner_without_runtime() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let original = settings(root.path());
    let session = controller
        .create_session(original.clone())
        .await
        .unwrap()
        .session;
    let page = controller.list_sessions(None, 0, 1).await.unwrap();
    assert_eq!(page.sessions[0].cwd, original.cwd);
    assert_eq!(page.next_offset, None);
    controller
        .rename_session(&session.id, "named cold session")
        .await
        .unwrap();
    assert_eq!(
        controller.read_settings(&session.id).await.unwrap(),
        (0, original.clone())
    );
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    let page = controller
        .list_sessions(Some("named cold"), 0, 1)
        .await
        .unwrap();
    assert_eq!(page.sessions[0].cwd, original.cwd);
    let mut changed = original;
    changed.cwd = root.path().join("another");
    controller
        .replace_settings(&session.id, 0, changed.clone())
        .await
        .unwrap();
    assert_eq!(
        controller.list_sessions(None, 0, 1).await.unwrap().sessions[0].cwd,
        changed.cwd
    );
}

#[tokio::test]
async fn explicit_session_model_and_omitted_default_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let omitted = settings(root.path());
    let mut explicit = omitted.clone();
    explicit.model = Some(crate::model::session::SessionModelConfig::of(
        crate::model::catalog::ModelRef::parse("chosen").unwrap(),
    ));
    let a = controller
        .create_session(omitted.clone())
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(explicit.clone())
        .await
        .unwrap()
        .session;
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert_eq!(controller.read_settings(&a.id).await.unwrap(), (0, omitted));
    assert_eq!(
        controller.read_settings(&b.id).await.unwrap(),
        (0, explicit)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("sessions/catalog.json")).unwrap())
            .unwrap();
    let state = &json["sessions"][a.id.as_str()]["state"];
    for key in ["model", "config", "tools", "exclude_tools"] {
        assert!(state.get(key).is_none(), "{key}");
    }
}
#[tokio::test]
async fn retained_allocations_and_committed_delete_have_one_winner() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let access_a = controller.acquire_session(&a.id, None).await.unwrap();
    let access_b = controller.acquire_session(&b.id, None).await.unwrap();
    assert!(matches!(
        controller.delete_preview(&a.id).await,
        SessionDeleteResult::Preview { .. }
    ));
    drop(access_a);
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await else {
        panic!("released allocation")
    };
    assert!(matches!(
        controller
            .delete_session(&a.id, &preview.target_revision)
            .await
            .unwrap(),
        SessionDeleteResult::Deleted { .. }
    ));
    assert!(matches!(
        controller.acquire_session(&a.id, None).await,
        Err(SessionError::UnknownSession { .. })
    ));
    assert_eq!(
        controller.read_session(&b.id).await.unwrap(),
        access_b.session
    );
    drop(access_b);
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&b.id).await else {
        panic!("released B")
    };
    controller
        .delete_session(&b.id, &preview.target_revision)
        .await
        .unwrap();
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
}
#[tokio::test]
async fn settings_cas_has_one_winner_without_touching_other_metadata() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let initial = settings(root.path());
    let a = controller
        .create_session(initial.clone())
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(initial.clone())
        .await
        .unwrap()
        .session;
    let mut candidate = initial.clone();
    candidate.model = Some(crate::model::session::SessionModelConfig::of(
        crate::model::catalog::ModelRef::parse("chosen").unwrap(),
    ));
    let (one, two) = tokio::join!(
        controller.replace_settings(&a.id, 0, candidate.clone()),
        controller.replace_settings(&a.id, 0, initial)
    );
    assert_eq!(one.unwrap(), 1);
    assert!(matches!(
        two,
        Err(SessionError::StaleSettings {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(
        controller.read_settings(&a.id).await.unwrap(),
        (1, candidate)
    );
    assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
}
/// R04: two distinct Session create requests genuinely overlap at the
/// actual synchronization boundary. The controller's preparation owner
/// serializes preparation by design, so the regression parks the first
/// request *inside* preparation (holding the preparation reservation), then
/// waits on a deterministic test-only signal that fires only after the
/// second request's preparation-lock future has been polled and returned
/// `Pending`. That makes the second request a registered waiter on the very
/// mutex the first request holds, not merely a spawned task that might be
/// scheduled after the first completes. Only then is the first released;
/// both publish through the generation-checked Catalog owner and both
/// distinct destinations remain complete, so a stale whole-document Catalog
/// replacement would be detected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r04_concurrent_distinct_session_creates_preserve_both_publications() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = gate.arm_scoped();
    *controller.create_gate.lock().unwrap() = Some(gate.clone());

    let first = {
        let controller = controller.clone();
        let path = root.path().to_path_buf();
        tokio::spawn(async move { controller.create_session(settings(&path)).await })
    };
    // Park the first request inside preparation and prove it holds the
    // preparation reservation while the second request is in flight.
    let entered = gate.clone();
    tokio::task::spawn_blocking(move || entered.wait_entered())
        .await
        .unwrap();
    assert!(
        controller.preparation.try_lock().is_err(),
        "the first create did not hold the preparation owner"
    );
    let second = {
        let controller = controller.clone();
        let path = root.path().to_path_buf();
        tokio::spawn(async move { controller.create_session(settings(&path)).await })
    };
    // Deterministic overlap proof: `acquire_preparation` notifies this only
    // after the second create's lock future has been polled and returned
    // `Pending`, so the second request is registered as a waiter on the
    // preparation mutex the first request still holds. No sleep, no
    // `yield_now`, no timing assumption.
    controller.create_preparation_wait.notified().await;
    drop(release);
    let a = first.await.unwrap().unwrap().session;
    let b = second.await.unwrap().unwrap().session;
    *controller.create_gate.lock().unwrap() = None;

    assert_ne!(a.id, b.id, "distinct create requests reused an identity");
    let listed = controller
        .list_sessions(None, 0, 32)
        .await
        .unwrap()
        .sessions;
    assert_eq!(listed.len(), 2, "a Catalog publication was lost");
    assert!(listed.iter().any(|row| row.id == a.id));
    assert!(listed.iter().any(|row| row.id == b.id));
    // Both destinations are complete and valid, not just visible.
    for session in [&a, &b] {
        let access = controller.acquire_session(&session.id, None).await.unwrap();
        assert!(access.database_path.is_file());
        let store = crate::durable::SqliteConversationStore::open_existing(
            access.node.conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        assert_eq!(
            store.load_canonical().unwrap(),
            [] as [crate::message::types::MessageBlock; 0]
        );
    }
}
/// R04 companion: two creates prepare from snapshots taken before either
/// competing publication, then both publish. Publishing must build from
/// the current Catalog document rather than a stale pre-publication whole
/// document, so neither publication is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r04_pre_publication_snapshots_preserve_both_publications() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let catalog = controller.catalog.clone();
    let snapshot_a = catalog.lock().await.clone();
    let snapshot_b = catalog.lock().await.clone();
    let settings_a = settings(root.path());
    let settings_b = settings(root.path());
    let prepared_a =
        tokio::task::spawn_blocking(move || snapshot_a.prepare_session(&settings_a, &[]))
            .await
            .unwrap()
            .unwrap();
    let prepared_b =
        tokio::task::spawn_blocking(move || snapshot_b.prepare_session(&settings_b, &[]))
            .await
            .unwrap()
            .unwrap();
    assert_ne!(prepared_a.session_id, prepared_b.session_id);
    assert_ne!(prepared_a.conversation_id, prepared_b.conversation_id);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let publish_a = {
        let catalog = catalog.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let mut guard = catalog.lock().await;
            guard.publish_session(&prepared_a, super::super::session::SessionNodeOrigin::New)
        })
    };
    let publish_b = {
        let catalog = catalog.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let mut guard = catalog.lock().await;
            guard.publish_session(&prepared_b, super::super::session::SessionNodeOrigin::New)
        })
    };
    let a = publish_a.await.unwrap().unwrap();
    let b = publish_b.await.unwrap().unwrap();
    assert_ne!(a.id, b.id);
    let listed = controller
        .list_sessions(None, 0, 32)
        .await
        .unwrap()
        .sessions;
    assert_eq!(listed.len(), 2, "a stale publication replaced a sibling");
    assert!(listed.iter().any(|row| row.id == a.id));
    assert!(listed.iter().any(|row| row.id == b.id));
}
#[tokio::test]
async fn create_and_rename_visibility_follow_catalog_rename_not_durability_barrier() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    controller
        .catalog
        .lock()
        .await
        .arm_write_fault_before_rename();
    assert!(
        controller
            .create_session(settings(root.path()))
            .await
            .is_err()
    );
    assert_eq!(
        controller
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .len(),
        1
    );
    controller
        .catalog
        .lock()
        .await
        .arm_write_fault_before_rename();
    assert!(
        !controller
            .rename_session(&a.id, "new")
            .await
            .unwrap_err()
            .committed()
    );
    assert_eq!(controller.read_session(&a.id).await.unwrap(), a);
    controller
        .catalog
        .lock()
        .await
        .arm_write_fault_after_rename();
    assert!(
        controller
            .rename_session(&a.id, "new")
            .await
            .unwrap_err()
            .committed()
    );
    let reopened = SessionCatalog::open_existing(root.path()).unwrap().unwrap();
    assert_eq!(
        reopened.snapshot(&a.id).unwrap(),
        controller.read_session(&a.id).await.unwrap()
    );
    assert_eq!(
        reopened.snapshot(&a.id).unwrap().name.as_deref(),
        Some("new")
    );
}
fn user(id: &str) -> crate::message::types::MessageBlock {
    crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock { text: id.into() },
        )],
        source: crate::message::types::UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        timestamp: None,
    })
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn exact_fork_boundary_excludes_delete_and_releases_metadata_lock() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let path = controller
        .catalog
        .lock()
        .await
        .database_path(&a.id, &a.active_conversation_id);
    let store =
        crate::durable::SqliteConversationStore::open(a.active_conversation_id.clone(), &path)
            .unwrap();
    store.append_canonical(&user("before")).unwrap();
    store.append_canonical(&user("boundary")).unwrap();
    let revision = store.load_head().unwrap().revision;
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = gate.arm_scoped();
    *controller.copy_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let id = a.id.clone();
    let fork = tokio::spawn(async move {
        worker
            .fork_session(
                &id,
                None,
                revision,
                Some(&crate::runtime::identity::MessageId::new("boundary")),
            )
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    assert!(!fork.is_finished());
    assert!(matches!(
        controller.delete_preview(&a.id).await,
        SessionDeleteResult::Preview { .. }
    ));
    controller
        .rename_session(&b.id, "B during copy")
        .await
        .unwrap();
    store.append_canonical(&user("later")).unwrap();
    drop(release);
    let copied = fork.await.unwrap().unwrap();
    assert_eq!(
        copied.editor_content.unwrap(),
        vec![super::super::session::uploads::UserInputBlock::Text(
            crate::message::content::TextBlock {
                text: "boundary".into()
            }
        )]
    );
    let path = controller
        .catalog
        .lock()
        .await
        .database_path(&copied.session.id, &copied.session.active_conversation_id);
    let destination = crate::durable::SqliteConversationStore::open_existing(
        copied.session.active_conversation_id.clone(),
        &path,
    )
    .unwrap();
    let history = destination.load_canonical().unwrap();
    assert_eq!(history.len(), 1);
    let crate::message::types::MessageBlock::User(message) = &history[0] else {
        panic!("copied user")
    };
    assert_ne!(message.id.as_str(), "before");
    assert_eq!(
        message.content,
        match user("before") {
            crate::message::types::MessageBlock::User(message) => message.content,
            _ => unreachable!(),
        }
    );
    assert_eq!(store.load_canonical().unwrap().len(), 3);
    drop(store);
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await else {
        panic!("fork released source allocation")
    };
    controller
        .delete_session(&a.id, &preview.target_revision)
        .await
        .unwrap();
    assert!(matches!(
        controller.fork_session(&a.id, None, revision, None).await,
        Err(SessionError::UnknownSession { .. })
    ));
    assert_eq!(
        controller.read_session(&copied.session.id).await.unwrap(),
        copied.session
    );
}

/// Writes one message into the Session's root conversation store, the way
/// the crash-window state (boundary committed, projection never
/// published) is built.
async fn append_root_history(
    controller: &SessionController,
    id: &SessionId,
    message: crate::message::types::MessageBlock,
) {
    let conversation = controller
        .acquire_session(id, None)
        .await
        .unwrap()
        .node
        .conversation_id;
    let path = controller
        .catalog
        .lock()
        .await
        .database_path(id, &conversation);
    let store = crate::durable::SqliteConversationStore::open(conversation, &path).unwrap();
    store.append_canonical(&message).unwrap();
}

fn image_only(id: &str) -> crate::message::types::MessageBlock {
    crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: vec![crate::message::types::UserContentBlock::Image(
            crate::message::content::ImageReference {
                artifact_id: crate::runtime::identity::ArtifactId::new("artifact-1"),
                alt: None,
            },
        )],
        source: crate::message::types::UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        timestamp: None,
    })
}

#[tokio::test]
async fn activity_reopen_repairs_a_committed_message_without_changing_metadata() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let time = chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let crate::message::types::MessageBlock::User(mut message) = user("recovered activity") else {
        unreachable!()
    };
    message.timestamp = Some(time);
    append_root_history(
        &controller,
        &session.id,
        crate::message::types::MessageBlock::User(message),
    )
    .await;
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .updated_at,
        session.created_at
    );
    drop(controller);
    let reopened = SessionController::open(root.path()).unwrap();
    assert_eq!(
        reopened
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .updated_at,
        time
    );
    assert_eq!(reopened.read_session(&session.id).await.unwrap(), session);
    let generation = reopened.catalog.lock().await.document_generation();
    drop(reopened);
    let reopened = SessionController::open(root.path()).unwrap();
    assert_eq!(
        reopened.catalog.lock().await.document_generation(),
        generation
    );
    assert_eq!(
        reopened
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .updated_at,
        time
    );
}

// P12
#[tokio::test]
async fn display_preview_repair_is_idempotent_and_never_manufactures_a_subject() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    append_root_history(&controller, &session.id, user("repair subject")).await;
    assert_eq!(
        controller
            .repair_display_preview(&session.id)
            .await
            .unwrap(),
        DisplayPreviewRepair::Published
    );
    let generation = controller.catalog.lock().await.document_generation();
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview
            .as_deref(),
        Some("repair subject")
    );
    assert_eq!(
        controller
            .repair_display_preview(&session.id)
            .await
            .unwrap(),
        DisplayPreviewRepair::AlreadyPresent,
        "a repeated repair converges without writing"
    );
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation,
        "the repeated repair is a write-level no-op"
    );

    // An image-only first boundary is a settled `None`: every repair
    // reports the empty subject and writes nothing, forever.
    let image_session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    append_root_history(&controller, &image_session.id, image_only("image-only")).await;
    let generation = controller.catalog.lock().await.document_generation();
    for _ in 0..2 {
        assert_eq!(
            controller
                .repair_display_preview(&image_session.id)
                .await
                .unwrap(),
            DisplayPreviewRepair::EmptySubject
        );
    }
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation,
        "repair never manufactures a rewrite for a settled None"
    );
    assert_eq!(
        controller
            .read_session_summary(&image_session.id)
            .await
            .unwrap()
            .preview,
        None
    );
}

// P13: the first projection write versus an independent rename and a
// settings commit, at the exact interleaving.
//
// The publication is parked *after* the root subject is derived and
// *before* the catalog mutex is acquired, which is the only window in which
// a competing metadata mutation can reach the catalog first. The gate holds
// no catalog lock, so the competing writes are genuinely concurrent with a
// repair that has already decided to publish.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One exact interleaving, asserted end to end.
async fn a_parked_first_publication_still_performs_the_first_projection_write() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    append_root_history(&controller, &session.id, user("race line")).await;
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview,
        None,
        "the projection is genuinely absent when the racer starts"
    );
    let invalidations = controller.summary_invalidations();
    let recorded = invalidations.recorded();
    let gate = super::super::session_display_projection::publication_test_support::arm(&session.id);
    let repairing = tokio::spawn({
        let controller = controller.clone();
        let id = session.id.clone();
        async move { controller.repair_display_preview(&id).await }
    });
    gate.parked(1).await;

    // The parked publication holds no catalog mutex: the competing writes
    // below acquire it while the repair is still parked.
    let generation = controller.catalog.lock().await.document_generation();
    let before = controller.read_session(&session.id).await.unwrap();
    let (revision, _) = controller.read_settings(&session.id).await.unwrap();
    controller
        .rename_session(&session.id, "race name")
        .await
        .unwrap();
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation + 1,
        "the rename is one isolated catalog commit"
    );
    let mut replacement = settings(root.path());
    replacement.cwd = root.path().join("elsewhere");
    assert_ne!(replacement, settings(root.path()));
    assert_eq!(
        controller
            .replace_settings(&session.id, revision, replacement.clone())
            .await
            .unwrap(),
        revision + 1,
        "the settings commit uses its real expected revision"
    );
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation + 2,
        "the settings commit is one isolated catalog commit"
    );
    let independent = controller.read_session(&session.id).await.unwrap();
    assert!(
        independent.updated_at > before.updated_at,
        "the independent metadata mutations moved updated_at"
    );
    assert_eq!(
        invalidations.recorded(),
        recorded,
        "rename and settings are not display-projection publications"
    );

    gate.release();
    assert_eq!(
        repairing.await.unwrap().unwrap(),
        DisplayPreviewRepair::Published,
        "the parked repair performs the first projection write"
    );
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation + 3,
        "the projection is exactly one further catalog commit"
    );
    assert_eq!(
        invalidations.recorded(),
        recorded + 1,
        "exactly one post-commit invalidation for the one projection commit"
    );

    let after = controller.read_session(&session.id).await.unwrap();
    assert_eq!(after.id, session.id, "identity survives");
    assert_eq!(
        after.name.as_deref(),
        Some("race name"),
        "the name survives"
    );
    assert_eq!(
        after.node_count, before.node_count,
        "the graph survives untouched"
    );
    assert_eq!(
        after.active_node, before.active_node,
        "the graph survives untouched"
    );
    assert_eq!(
        after.updated_at, independent.updated_at,
        "projection publication never moves the updated_at the independent mutation established"
    );
    let (settings_revision, state) = controller.read_settings(&session.id).await.unwrap();
    assert_eq!(settings_revision, revision + 1);
    assert_eq!(state, replacement, "the committed settings survive");
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview
            .as_deref(),
        Some("race line"),
    );
}

// P13 (racing-repair half): both repairs are forced to observe absence and
// derive the subject before either may publish. `tokio::join!` alone cannot
// establish that; the shared publication gate can.
#[tokio::test]
async fn racing_repairs_commit_the_projection_exactly_once() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    append_root_history(&controller, &session.id, user("raced repair line")).await;
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview,
        None
    );
    let invalidations = controller.summary_invalidations();
    let recorded = invalidations.recorded();
    let generation = controller.catalog.lock().await.document_generation();
    let gate = super::super::session_display_projection::publication_test_support::arm(&session.id);
    let mut racers = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let controller = controller.clone();
        let id = session.id.clone();
        racers.spawn(async move { controller.repair_display_preview(&id).await });
    }
    // Neither racer can publish until both have read absence and derived
    // the same subject.
    gate.parked(2).await;
    gate.release();
    let mut outcomes = Vec::new();
    while let Some(result) = racers.join_next().await {
        outcomes.push(result.expect("no racer panicked").expect("no racer failed"));
    }
    outcomes.sort_by_key(|repair| match repair {
        DisplayPreviewRepair::AlreadyPresent => 0,
        DisplayPreviewRepair::Published => 1,
        other => panic!("unexpected repair outcome: {other:?}"),
    });
    assert_eq!(
        outcomes,
        [
            DisplayPreviewRepair::AlreadyPresent,
            DisplayPreviewRepair::Published
        ],
        "exactly one racer commits; the other converges as AlreadyPresent"
    );
    assert_eq!(
        controller.catalog.lock().await.document_generation(),
        generation + 1,
        "racing repairs advance the catalog generation exactly once"
    );
    assert_eq!(
        invalidations.recorded(),
        recorded + 1,
        "one projection commit means one invalidation, not two"
    );
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview
            .as_deref(),
        Some("raced repair line")
    );
}

// P13 (deletion half): deletion wins at its real visibility point while a
// repair is parked past its read. The resumed publication must not
// resurrect the Session and must not announce a metadata change.
#[tokio::test]
async fn a_repair_parked_past_deletion_neither_resurrects_nor_announces() {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let generation = controller
        .read_session_summary(&session.id)
        .await
        .unwrap()
        .ownership_generation
        .parse::<u64>()
        .unwrap();
    append_root_history(&controller, &session.id, user("deleted before publication")).await;
    let invalidations = controller.summary_invalidations();
    let recorded = invalidations.recorded();
    let gate = super::super::session_display_projection::publication_test_support::arm(&session.id);
    let repairing = tokio::spawn({
        let controller = controller.clone();
        let id = session.id.clone();
        async move { controller.repair_display_preview(&id).await }
    });
    gate.parked(1).await;
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&session.id).await
    else {
        panic!("an unloaded Session is deletable")
    };
    assert!(
        matches!(
            controller
                .delete_session(&session.id, &preview.target_revision)
                .await,
            Ok(SessionDeleteResult::Deleted { .. }
                | SessionDeleteResult::CommittedCleanupPending { .. })
        ),
        "deletion wins at its own visibility point"
    );
    assert_eq!(
        invalidations.recorded(),
        recorded + 2,
        "deletion announces membership and ownership retirement exactly once each"
    );
    assert_eq!(
        invalidations.next_after(recorded),
        Some((recorded + 2, session.id.clone(), true))
    );
    assert_eq!(
        invalidations.next_ownership_after(recorded),
        Some((recorded + 1, session.id.clone(), generation))
    );
    gate.release();
    assert_eq!(
        repairing.await.unwrap().unwrap(),
        DisplayPreviewRepair::SessionGone,
        "a Session that vanished mid-repair is not an error and not a publication"
    );
    assert!(matches!(
        controller.read_session_summary(&session.id).await,
        Err(SessionError::UnknownSession { .. } | SessionError::DeletingSession { .. })
    ));
    assert_eq!(
        invalidations.recorded(),
        recorded + 2,
        "the resumed repair adds nothing to deletion's membership invalidation"
    );
}
