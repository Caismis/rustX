use super::*;

struct PublicationPark {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Drop for PublicationPark {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

fn park_publication(inner: &super::super::ClientInner, encoding: bool) -> PublicationPark {
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        entered_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    let slot = if encoding {
        &inner.publication_gate.encoding_hook
    } else {
        &inner.publication_gate.preparation_hook
    };
    *slot.lock().unwrap() = Some(hook);
    PublicationPark { entered, release }
}

async fn wait_signal(receiver: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(BACKGROUND_LIVENESS_GUARD)
            .expect("the actual publication boundary was reached");
    })
    .await
    .unwrap();
}

fn start_settlement_waiter(
    inner: Arc<super::super::ClientInner>,
) -> (tokio::task::JoinHandle<()>, std::sync::mpsc::Receiver<()>) {
    let (contended_tx, contended_rx) = std::sync::mpsc::channel();
    *inner
        .publication_gate
        .settlement_contention_hook
        .lock()
        .unwrap() = Some(Box::new(move || contended_tx.send(()).unwrap()));
    let waiter = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(inner.projection_settled());
    });
    (waiter, contended_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_settled_joins_drained_activation_before_eligible_baseline() {
    use crate::runtime::adoption::AdoptionEligibility;

    let (adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    let inner = host.weak_inner().upgrade().unwrap();
    let parked = park_publication(&inner, false);
    let cursor = inner.state.lock().unwrap().projection.cursor();
    fixture.runtime.activate();
    parked
        .entered
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .unwrap();
    assert!(inner.pending.settled(), "the real activation was drained");
    {
        let state = inner.state.lock().unwrap();
        assert_eq!(state.projection.cursor(), cursor);
        assert_eq!(
            state
                .projection
                .snapshot_ref()
                .configuration_adoption_eligibility,
            AdoptionEligibility::Unavailable
        );
        assert!(state.projection.read_domain_cut().is_none());
        state.projection.validate_source_prefix().unwrap();
    }
    let (waiter, contended) = start_settlement_waiter(inner.clone());
    wait_signal(contended).await;
    drop(parked);
    waiter.await.unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert_eq!(
        checkpoint.snapshot.configuration_adoption_eligibility,
        AdoptionEligibility::Eligible
    );
    assert!(checkpoint.cursor > cursor);
    assert!(adapter.requests().is_empty());
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_settled_joins_encoding_before_model_baseline() {
    let (adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    fixture.runtime.activate();
    host.projection_settled().await;
    let inner = host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let cursor = inner.state.lock().unwrap().projection.cursor();
    let config = marked_model_config(&fixture.runtime, "settlement-marker");
    let parked = park_publication(&inner, true);
    let mutation_host = host.clone();
    let mutation = tokio::task::spawn_blocking(move || mutation_host.model_set(config).unwrap());
    parked
        .entered
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .unwrap();
    assert!(inner.observation_preparation.try_lock().is_ok());
    assert_eq!(inner.state.lock().unwrap().projection.cursor(), cursor);
    let (waiter, contended) = start_settlement_waiter(inner.clone());
    wait_signal(contended).await;
    drop(parked);
    mutation.await.unwrap();
    waiter.await.unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert!(checkpoint.cursor > cursor);
    assert_eq!(
        checkpoint
            .snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .request_params
            .get("settlement-marker"),
        Some(&serde_json::json!("changed"))
    );
    assert!(adapter.requests().is_empty());
    inner.unpark_projection_worker();
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

fn activity_activation(name: &str) -> crate::runtime::subagent::SubagentSnapshot {
    use crate::runtime::subagent::{
        AgentActivationOrigin, SubagentObservation, SubagentSettlement, SubagentSnapshot,
        SubagentState,
    };
    SubagentSnapshot {
        ownership: crate::events::types::SubagentOwnershipKind::Normal,
        parent_agent_id: AgentId::new("activity-parent"),
        child_agent_id: AgentId::new(name),
        child_conversation_id: ConversationId::generate(),
        subagent_id: crate::runtime::identity::SubagentId::new(format!("activation-{name}")),
        origin: AgentActivationOrigin::CreationTool {
            tool_call_id: ToolCallId::new(name),
        },
        agent: "explore".into(),
        definition_digest: "definition".into(),
        profile_digest: "profile".into(),
        workspace: std::path::PathBuf::from("/workspace"),
        state: SubagentState::Running,
        cancel_reason: None,
        detail: None,
        observation: SubagentObservation::default(),
        profile: None,
        settlement: SubagentSettlement::default(),
        started_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    }
}

fn activity_collection(
    generation: u64,
    ordinal: u64,
    observation: ConversationObservation,
) -> ConversationObservation {
    ConversationObservation::CollectionProof {
        owner: "agents",
        cut: crate::runtime::identity::ObservationCollectionCut {
            generation,
            total: 2,
        },
        ordinal: Some(ordinal),
        observation: Box::new(observation),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drained_activity_preserves_terminal_owner_and_unrelated_live_updates() {
    Box::pin(activity_preserves_terminal_owner(false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deferred_publication_preserves_terminal_owner_and_unrelated_live_updates() {
    Box::pin(activity_preserves_terminal_owner(true)).await;
}

#[allow(clippy::too_many_lines)] // One consumed source-gap interleaving preserves snapshot, replay and other-Agent coverage.
async fn activity_preserves_terminal_owner(deferred: bool) {
    use crate::runtime::observation::PublicationProjectionCommit;
    use crate::runtime::subagent::{AgentSnapshot, AgentState, SubagentActivity, SubagentState};

    let (_, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    fixture.runtime.activate();
    host.projection_settled().await;
    let inner = host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let sink = super::super::NativePublicationProjection {
        inner: Arc::downgrade(&inner),
    };
    let publish_owner =
        |activation: &crate::runtime::subagent::SubagentSnapshot, state, generation, ordinal| {
            let agent = AgentSnapshot {
                title: "Explore".into(),
                agent_id: activation.child_agent_id.clone(),
                conversation_id: activation.child_conversation_id.clone(),
                parent_agent_id: activation.parent_agent_id.clone(),
                agent: activation.agent.clone(),
                state,
                current_activation: (state != AgentState::Inactive)
                    .then(|| activation.subagent_id.clone()),
                latest_activation: activation.subagent_id.clone(),
                observation: activation.observation.clone(),
            };
            let expected = crate::runtime_client::projection::views::agent_view(&agent, activation);
            assert!(
                sink.native_actor(&activity_collection(
                    generation,
                    ordinal,
                    ConversationObservation::SubagentLifecycle {
                        agent: Some(Box::new(agent)),
                        snapshot: activation.clone(),
                    },
                ))
                .unwrap()
                .is_installed()
            );
            expected
        };
    let publish_canonical = |receipt: crate::durable::TranscriptCommitReceipt| {
        super::super::publication::commit_display(
            &inner.state,
            &inner.publication_gate,
            |projection| {
                projection.publication_commit_scope(true, true);
                projection.apply(ConversationObservation::CanonicalCommitted {
                    attempt_id: None,
                    receipt,
                });
                projection.publication_commit_scope(false, true);
            },
        );
    };
    let mut first = activity_activation("first");
    first.state = if deferred {
        SubagentState::Running
    } else {
        SubagentState::Stopping
    };
    first.observation.revision = 3;
    first.observation.activity = SubagentActivity::Compacting;
    let mut second = activity_activation("second");
    publish_owner(&first, AgentState::Active, 3, 0);
    publish_owner(&second, AgentState::Active, 3, 1);
    let baseline = host.checkpoint().unwrap();
    let initial_agents = baseline.snapshot.agents;
    let before = baseline.cursor;
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let replay = attachment.subscribe_events(before).unwrap();

    first.state = SubagentState::Stopping;
    first.observation.revision += 1;
    second.observation.revision += 1;
    second.observation.activity = SubagentActivity::Compacting;
    inner.pending.push(activity_collection(
        4,
        0,
        ConversationObservation::SubagentActivity(first.clone()),
    ));
    inner.pending.push(activity_collection(
        5,
        1,
        ConversationObservation::SubagentActivity(second.clone()),
    ));
    let parked = park_publication(&inner, false);
    inner.unpark_projection_worker();
    parked
        .entered
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .unwrap();
    assert_eq!(
        inner.pending.queued_activity(),
        1,
        "the first activity drained while the other Agent remains queued"
    );

    let store = fixture.runtime.tool_runtime().durable_store().clone();
    let gap = deferred.then(|| {
        let earlier = store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("held-agent-source-prefix"),
                content: Vec::new(),
            }))
            .unwrap();
        let later = store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("later-agent-source-prefix"),
                content: Vec::new(),
            }))
            .unwrap();
        let later_position = later.ledger_position;
        publish_canonical(later);
        store
            .mark_canonical_transcript_ready(later_position, later_position)
            .unwrap();
        assert!(
            inner
                .state
                .lock()
                .unwrap()
                .projection
                .validate_source_prefix()
                .is_err(),
            "the earlier native canonical owner still holds the source prefix"
        );
        earlier
    });
    first.state = SubagentState::Succeeded;
    first.observation.settle_neutral();
    let terminal = publish_owner(&first, AgentState::Inactive, 6, 0);
    assert_eq!(terminal.activation_state, SubagentState::Succeeded);
    let following = deferred.then(|| park_publication(&inner, false));
    if deferred {
        assert_eq!(
            host.checkpoint().unwrap().cursor,
            before,
            "native terminal advances only working state while the source gap is open"
        );
        assert_eq!(
            inner
                .state
                .lock()
                .unwrap()
                .projection
                .working_snapshot_for_test()
                .agents
                .iter()
                .find(|agent| agent.agent_id == first.child_agent_id),
            Some(&terminal)
        );
    }
    drop(parked);
    if let Some(following) = following {
        following
            .entered
            .recv_timeout(BACKGROUND_LIVENESS_GUARD)
            .unwrap();
        assert_eq!(
            host.checkpoint().unwrap().cursor,
            before,
            "old activity folds before source-prefix catch-up"
        );
        let earlier = gap.unwrap();
        let position = earlier.ledger_position;
        publish_canonical(earlier);
        store
            .mark_canonical_transcript_ready(position, position)
            .unwrap();
        let proof = store.transcript_source_ready().unwrap();
        super::super::publication::commit_display(
            &inner.state,
            &inner.publication_gate,
            |projection| {
                projection.publication_commit_scope(true, true);
                projection.apply(ConversationObservation::TranscriptSourceReady(proof));
                projection.publication_commit_scope(false, true);
            },
        );
        {
            let state = inner.state.lock().unwrap();
            assert!(
                state.projection.validate_source_prefix().is_ok(),
                "{}",
                state.projection.source_prefix_debug()
            );
        }
        drop(following);
    }
    host.projection_settled().await;
    let checkpoint = host.checkpoint().unwrap();
    let actual = checkpoint
        .snapshot
        .agents
        .iter()
        .find(|agent| agent.agent_id == first.child_agent_id)
        .unwrap();
    assert_eq!(
        actual, &terminal,
        "late activity must preserve the entire terminal owner view"
    );
    assert_eq!(
        actual.observation.activity,
        SubagentActivity::AwaitingActivity
    );
    let other = checkpoint
        .snapshot
        .agents
        .iter()
        .find(|agent| agent.agent_id == second.child_agent_id)
        .unwrap();
    assert_eq!(
        other.observation, second.observation,
        "a newer collection cut for another Agent cannot discard this valid activity"
    );
    for agent in [actual, other] {
        assert!(
            inner
                .state
                .lock()
                .unwrap()
                .projection
                .domain_source(&format!("agents.{}", agent.agent_id), 0)
                .is_none(),
            "Agent detail is owned by the native registry, never a duplicate Projection source"
        );
    }
    let mut updates = Vec::new();
    let mut replayed_agents = initial_agents;
    let mut replacements = 0;
    let mut replay_cursor = before;
    loop {
        let event = match replay.try_next() {
            EventDelivery::Event(event) => event,
            EventDelivery::Pending => break,
            other => panic!("unexpected replay boundary: {other:?}"),
        };
        assert_eq!(
            event.cursor.get(),
            replay_cursor.get() + 1,
            "replay cursors remain contiguous"
        );
        replay_cursor = event.cursor;
        for fact in event.event.facts() {
            match fact {
                RuntimeClientEvent::AgentUpdated { agent, .. } => {
                    if agent.agent_id == terminal.agent_id {
                        assert_eq!(
                            agent.as_ref(),
                            &terminal,
                            "no replay delta resurrects the terminal Agent"
                        );
                    }
                    if let Some(current) = replayed_agents
                        .iter_mut()
                        .find(|current| current.agent_id == agent.agent_id)
                    {
                        *current = agent.as_ref().clone();
                    } else {
                        replayed_agents.push(agent.as_ref().clone());
                    }
                    updates.push(agent.as_ref().clone());
                }
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                    assert_eq!(
                        checkpoint
                            .snapshot
                            .agents
                            .iter()
                            .find(|agent| agent.agent_id == terminal.agent_id),
                        Some(&terminal),
                        "replacement carries the terminal owner and neutral activity"
                    );
                    replayed_agents.clone_from(&checkpoint.snapshot.agents);
                    replacements += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        replay_cursor, checkpoint.cursor,
        "replay reaches the exact published checkpoint"
    );
    for expected in [&terminal, other] {
        assert_eq!(
            replayed_agents
                .iter()
                .find(|agent| agent.agent_id == expected.agent_id),
            Some(expected),
            "full replay agrees with terminal and unrelated live snapshot values"
        );
    }
    if deferred {
        assert!(
            replacements > 0,
            "native source catch-up publishes the bounded checkpoint replacement"
        );
    } else {
        assert_eq!(
            updates,
            vec![terminal, other.clone()],
            "ordinary replay retains both exact Agent deltas"
        );
    }
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}
