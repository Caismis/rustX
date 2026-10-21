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
#[allow(clippy::too_many_lines)] // Keep the single publication interleaving and all three consumer assertions together.
async fn drained_activity_preserves_terminal_owner_and_unrelated_live_updates() {
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
        };
    let mut first = activity_activation("first");
    first.state = SubagentState::Stopping;
    first.observation.revision = 3;
    first.observation.activity = SubagentActivity::Compacting;
    let mut second = activity_activation("second");
    publish_owner(&first, AgentState::Active, 3, 0);
    publish_owner(&second, AgentState::Active, 3, 1);
    let before = host.checkpoint().unwrap().cursor;
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let replay = attachment.subscribe_events(before).unwrap();

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

    first.state = SubagentState::Succeeded;
    first.observation.settle_neutral();
    publish_owner(&first, AgentState::Inactive, 6, 0);
    let terminal = host
        .checkpoint()
        .unwrap()
        .snapshot
        .agents
        .into_iter()
        .find(|agent| agent.agent_id == first.child_agent_id)
        .unwrap();
    assert_eq!(terminal.activation_state, SubagentState::Succeeded);
    drop(parked);
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
    {
        let state = inner.state.lock().unwrap();
        for agent in [actual, other] {
            let source = state
                .projection
                .domain_source(&format!("agents.{}", agent.agent_id), 0)
                .unwrap();
            let crate::runtime_client::domain::DomainSource::Agent(value) = source.as_ref() else {
                panic!("Agent source")
            };
            assert_eq!(
                value.value(),
                agent,
                "checkpoint value reads agree with the snapshot"
            );
        }
    }
    let mut updates = Vec::new();
    while let EventDelivery::Event(event) = replay.try_next() {
        for fact in event.event.facts() {
            if let RuntimeClientEvent::AgentUpdated { agent, .. } = fact {
                updates.push(agent.as_ref().clone());
            }
        }
    }
    assert_eq!(
        updates,
        vec![terminal, other.clone()],
        "replay never regresses the settled activation"
    );
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}
