use super::*;
use crate::durable::JournalStore;

#[tokio::test]
async fn agent_bootstrap_ignores_activation_record_iteration_order() {
    let plane = plane(4);
    let first_child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("bootstrap one Agent")).await;
    first_child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let mut latest = first.subagent_id.clone();
    for message in ["second", "third"] {
        let mut child = stage_exit0(&plane);
        let signal = CancellationSignal::new();
        let (accepted, _) = tokio::join!(
            plane.registry.send_message(
                &first.child_agent_id,
                message,
                AgentActivationOrigin::ClientControl,
                signal.clone()
            ),
            child.accept_delegate(),
        );
        latest = accepted.unwrap().activation_id;
        child
            .send_result(ChildResultStatus::Succeeded, Some(message))
            .await;
        plane.registry.wait_until_settled(&latest).await.unwrap();
    }
    // Reproduce recovery's nonchronological finite record order: 1, 3, 2.
    // The authoritative Agent still names activation 3.
    {
        let mut state = plane.registry.state.lock().unwrap();
        state.records.swap(1, 2);
        let index = state
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.subagent_id.clone(), index))
            .collect();
        state.index = index;
    }
    let observed = Arc::new(RecordingObserver::default());
    let (owners, _) = plane
        .registry
        .install_observer_and_agent_snapshots(observed.clone());
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].0.agent_id, first.child_agent_id);
    assert_eq!(owners[0].0.latest_activation, latest);
    assert_eq!(owners[0].1.subagent_id, latest);
    assert_eq!(owners[0].0.state, AgentState::Inactive);
    assert_eq!(
        observed.0.lock().unwrap().len(),
        1,
        "bootstrap emits only exact latest activation"
    );
    let mut fourth_child = stage_exit0(&plane);
    let signal = CancellationSignal::new();
    let (fourth, _) = tokio::join!(
        plane.registry.send_message(
            &first.child_agent_id,
            "fourth",
            AgentActivationOrigin::ClientControl,
            signal.clone()
        ),
        fourth_child.accept_delegate(),
    );
    let fourth = fourth.unwrap();
    let owner = plane
        .registry
        .agent_snapshot(&first.child_agent_id)
        .unwrap();
    assert_eq!(owner.current_activation, Some(fourth.activation_id.clone()));
    assert_ne!(fourth.activation_id, latest);
    fourth_child
        .send_result(ChildResultStatus::Succeeded, Some("fourth"))
        .await;
    plane
        .registry
        .wait_until_settled(&fourth.activation_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn proven_child_loss_has_identical_resume_authority_before_and_after_reopen() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let admitted = start(&plane, &start_spec("unexpected child loss")).await;
    child.accept_delegate().await;
    // EOF after exact delegation acceptance, with the staged process exiting 0,
    // proves direct-child containment without inventing a semantic result.
    drop(child);
    let lost = plane
        .registry
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    assert_eq!(lost.state, SubagentState::Interrupted);
    assert!(events(&plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
            state: SubagentTerminalState::Interrupted,
            physical_settlement_proven: true,
            ..
        }
    )));
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_sequence_watermark(1);
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    // The same committed settlement makes both resident and recovered owners
    // resumable; this inspection performs no activation side effect.
    for registry in [&plane.registry, &recovered] {
        assert_eq!(
            registry
                .agent_snapshot(&admitted.child_agent_id)
                .unwrap()
                .state,
            AgentState::Inactive
        );
    }
    let recovered_plane = TestPlane {
        registry: recovered,
        ..plane
    };
    let mut next = stage_exit0(&recovered_plane);
    let signal = CancellationSignal::new();
    let (resumed, _) = tokio::join!(
        recovered_plane.registry.send_message(
            &admitted.child_agent_id,
            "continue after proven loss",
            AgentActivationOrigin::ClientControl,
            signal.clone()
        ),
        next.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    assert_eq!(resumed.agent_id, admitted.child_agent_id);
    next.send_result(ChildResultStatus::Succeeded, Some("recovered report"))
        .await;
    recovered_plane
        .registry
        .wait_until_settled(&resumed.activation_id)
        .await
        .unwrap();
}

fn admitted_catalog(names: &[&str]) -> super::super::super::AgentCatalog {
    super::super::super::AgentCatalog::new(names.iter().map(|name| {
        let document = crate::local_runtime::agent_resources::parse(&format!(
            "description = '{name} role'\ninstructions = 'Act as {name}'\n[tools]\nbuiltin = ['read']"
        ))
        .unwrap();
        super::super::super::NamedAgentDefinition::new(
            super::super::super::SubagentName::parse(name).unwrap(),
            crate::runtime::agent_profile::AgentProfile::from_document(
                &document,
                crate::runtime::agent_profile::AgentProfileKind::Named,
                Vec::new(),
            )
            .unwrap(),
            format!("{name}.toml").into(),
        )
        .unwrap()
    }))
    .unwrap()
}

/// Issue #462 A01/A03/A13: one Root decision over one admitted catalog
/// drives metadata and executable registrations. Zero admissions compose
/// none of the five controls, any admission composes all five exactly once,
/// and an admission without the registry service fails composition instead
/// of publishing a degraded zero-tool state.
#[tokio::test]
async fn domain_tool_metadata_matches_registered_authority() {
    use crate::tools::native::{AGENT_CONTROL_TOOL_NAMES, NativeToolPolicies, NativeToolResources};
    let plane = plane(4);
    let artifacts = plane.dir.path().join("tool-artifacts");
    let background = crate::tools::background::ConversationBackgroundRegistry::new(
        plane.conversation_id.clone(),
        crate::tools::background::BackgroundResources {
            mailbox: plane.registry.config.mailbox.clone(),
            workspace: crate::tools::workspace::Workspace::new(plane.dir.path().join("workspace"))
                .unwrap(),
            artifacts: crate::tools::artifacts::ArtifactStore::new(
                plane.conversation_id.clone(),
                &artifacts,
            )
            .unwrap(),
            tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                plane.conversation_id.clone(),
                artifacts.join("output"),
            )
            .unwrap(),
            clock: Arc::new(SystemClock),
            event_sink: None,
        },
    );
    let policies = NativeToolPolicies::default();
    for (catalog, owns_registry) in [
        (super::super::super::AgentCatalog::empty(), false),
        (super::super::super::AgentCatalog::empty(), true),
        (admitted_catalog(&["reviewer"]), true),
        (admitted_catalog(&["reviewer", "explorer"]), true),
    ] {
        let metadata = crate::tools::native::definitions(policies, &catalog);
        let mut executable = crate::tools::executor::ToolRegistry::new();
        crate::tools::native::register_native_tools(
            &mut executable,
            NativeToolResources {
                background: background.clone(),
                subagents: owns_registry.then(|| plane.registry.clone()),
                subagent_catalog: catalog.clone(),
            },
            policies,
        )
        .unwrap();
        assert_eq!(
            metadata
                .into_iter()
                .map(|(definition, _)| definition)
                .collect::<Vec<_>>(),
            executable.definitions()
        );
        let names = executable.names();
        for control in AGENT_CONTROL_TOOL_NAMES {
            assert_eq!(
                names.iter().filter(|name| **name == control).count(),
                usize::from(!catalog.is_empty()),
                "{control} follows the admitted catalog alone"
            );
        }
        assert!(names.contains(&"job_wait"));
        // Both native and Workflow children share the frozen child registration
        // boundary, which must reject every domain control lacking its owner.
        for definition in executable
            .definitions()
            .into_iter()
            .filter(|d| crate::tools::native::is_domain_control(&d.name))
        {
            let mut child = crate::tools::executor::ToolRegistry::new();
            assert!(
                crate::tools::native::register_subagent_child_tools(&mut child, &[definition])
                    .is_err()
            );
            assert!(child.definitions().is_empty());
        }
    }
    let mut degraded = crate::tools::executor::ToolRegistry::new();
    let error = crate::tools::native::register_native_tools(
        &mut degraded,
        NativeToolResources {
            background: background.clone(),
            subagents: None,
            subagent_catalog: admitted_catalog(&["reviewer"]),
        },
        policies,
    )
    .expect_err("a nonempty admission without the registry is a composition failure");
    assert!(matches!(
        error,
        crate::tools::executor::ToolRegistryError::MissingService(_)
    ));
}

use crate::runtime::observation::{ConversationObservation, PendingObservations};
struct AdmissionObserver {
    queue: Arc<PendingObservations>,
    store: Arc<dyn crate::durable::ConversationStore>,
    fanout: crate::runtime::observation::ObservationFanout,
    before_owner: Mutex<Vec<ConversationObservation>>,
}
impl SubagentObserver for AdmissionObserver {
    fn on_snapshot(
        &self,
        agent: Option<&AgentSnapshot>,
        snapshot: &SubagentSnapshot,
        _cut: crate::runtime::identity::ObservationCollectionCut,
        _ordinal: Option<u64>,
    ) {
        self.queue.push(ConversationObservation::SubagentLifecycle {
            agent: agent.cloned().map(Box::new),
            snapshot: snapshot.clone(),
        });
    }
    fn on_committed(
        &self,
        agent: Option<&AgentSnapshot>,
        snapshot: &SubagentSnapshot,
        committed: &crate::events::types::RuntimeEventEnvelope,
        _cut: crate::runtime::identity::ObservationCollectionCut,
        _ordinal: Option<u64>,
    ) {
        // The durable receipt is pending until the entire owner transition
        // arrives. This drain is deliberately before the one publication.
        let earlier = self.queue.drain();
        assert!(earlier.iter().flat_map(ConversationObservation::facts).all(|fact|
            !matches!(fact, ConversationObservation::JournalFact(witness) if witness.sequence >= committed.sequence)
        ), "current original Journal fact waits for owner installation");
        self.before_owner.lock().unwrap().extend(earlier);
        let observation = ConversationObservation::Published {
            journal_sequence: committed.sequence,
            observation: Box::new(ConversationObservation::SubagentLifecycle {
                agent: agent.cloned().map(Box::new),
                snapshot: snapshot.clone(),
            }),
        };
        self.fanout.push(&observation);
        observation.publish_journal_source(self.store.as_ref(), &self.fanout);
    }
}

#[tokio::test]
async fn admission_receipts_release_observation_frontier_after_owner_installation() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("admission observation cut")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let queue = Arc::new(PendingObservations::new());
    let initial_journal = plane.store.observe_journal(queue.clone()).unwrap();
    let journal_source: Arc<dyn crate::durable::ConversationStore> = plane.store.clone();
    queue.bind_journal(&journal_source, initial_journal);
    let observer = Arc::new(AdmissionObserver {
        queue: queue.clone(),
        store: plane.store.clone(),
        fanout: crate::runtime::observation::ObservationFanout::new(queue.clone()),
        before_owner: Mutex::new(Vec::new()),
    });
    plane
        .registry
        .install_observer_and_agent_snapshots(observer.clone());
    queue.drain();
    plane.registry.state.lock().unwrap().allocation_failure = Some(
        super::super::super::process::SpawnError::ConversationIdentityInUse {
            conversation_id: first.child_conversation_id.clone(),
            path: plane.runtime_root.clone(),
        },
    );
    assert!(
        plane
            .registry
            .send_message(
                &first.child_agent_id,
                "rollback",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await
            .is_err()
    );
    let frontier = plane.store.presentation_frontier().unwrap();
    let mut batches = std::mem::take(&mut *observer.before_owner.lock().unwrap());
    batches.extend(queue.drain());
    let witnessed: Vec<_> = batches
        .iter()
        .flat_map(ConversationObservation::facts)
        .filter_map(|fact| match fact {
            ConversationObservation::JournalFact(witness) => Some(witness.sequence),
            _ => None,
        })
        .collect();
    assert_eq!(
        witnessed.last(),
        Some(&frontier),
        "all reserved and rollback facts published"
    );
    assert!(witnessed.windows(2).all(|pair| pair[0] < pair[1]));
    let states: Vec<_> = batches
        .iter()
        .flat_map(ConversationObservation::facts)
        .filter_map(|fact| match fact {
            ConversationObservation::SubagentLifecycle {
                agent: Some(agent),
                snapshot,
            } => {
                assert_eq!(agent.latest_activation, snapshot.subagent_id);
                assert_eq!(agent.agent_id, snapshot.child_agent_id);
                Some((agent.state, agent.current_activation.clone()))
            }
            _ => None,
        })
        .collect();
    assert!(states.contains(&(
        AgentState::Admitting,
        Some(SubagentId::for_conversation(&plane.conversation_id, 2))
    )));
    assert!(states.contains(&(AgentState::Inactive, None)));
    assert!(queue.drain().is_empty());
}

struct HandoffRelease(Arc<ControlHandoffHook>);
impl Drop for HandoffRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The handoff gate parks after ownership commits but before the registry may
/// install its driver handle and change Starting to Running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // Keep both gates and their one-cut assertions together.
async fn starting_owner_publication_is_one_complete_admitting_cut() {
    use crate::runtime::observation::{ConversationObservation, PendingObservations};
    struct OwnerObserver {
        queue: Arc<PendingObservations>,
        store: Arc<dyn crate::durable::ConversationStore>,
        fanout: crate::runtime::observation::ObservationFanout,
    }
    impl SubagentObserver for OwnerObserver {
        fn on_snapshot(
            &self,
            agent: Option<&AgentSnapshot>,
            snapshot: &SubagentSnapshot,
            _cut: crate::runtime::identity::ObservationCollectionCut,
            _ordinal: Option<u64>,
        ) {
            self.queue.push(ConversationObservation::SubagentLifecycle {
                agent: agent.cloned().map(Box::new),
                snapshot: snapshot.clone(),
            });
        }
        fn on_committed(
            &self,
            agent: Option<&AgentSnapshot>,
            snapshot: &SubagentSnapshot,
            committed: &crate::events::types::RuntimeEventEnvelope,
            _cut: crate::runtime::identity::ObservationCollectionCut,
            _ordinal: Option<u64>,
        ) {
            if snapshot.state == SubagentState::Stopping
                && agent.is_some_and(|agent| agent.state == AgentState::Admitting)
            {
                let before_owner = self.queue.drain();
                assert!(before_owner.iter().flat_map(ConversationObservation::facts).all(|fact|
                    !matches!(fact, ConversationObservation::JournalFact(witness) if witness.sequence >= committed.sequence)
                ), "current native receipt cannot precede owner installation");
            }
            let observation = ConversationObservation::Published {
                journal_sequence: committed.sequence,
                observation: Box::new(ConversationObservation::SubagentLifecycle {
                    agent: agent.cloned().map(Box::new),
                    snapshot: snapshot.clone(),
                }),
            };
            self.fanout.push(&observation);
            observation.publish_journal_source(self.store.as_ref(), &self.fanout);
        }
    }
    let plane = plane(4);
    let queue = Arc::new(PendingObservations::new());
    let initial_journal = plane.store.observe_journal(queue.clone()).unwrap();
    let journal_source: Arc<dyn crate::durable::ConversationStore> = plane.store.clone();
    queue.bind_journal(&journal_source, initial_journal);
    plane
        .registry
        .install_observer_and_agent_snapshots(Arc::new(OwnerObserver {
            queue: queue.clone(),
            store: plane.store.clone(),
            fanout: crate::runtime::observation::ObservationFanout::new(queue.clone()),
        }));
    let handoff = Arc::new(ControlHandoffHook::default());
    plane.registry.install_control_handoff_hook(handoff.clone());
    let _release_handoff = HandoffRelease(handoff.clone());
    let child = stage_exit0(&plane);
    let registry = plane.registry.clone();
    let committing = tokio::spawn(async move {
        let spec = start_spec("one owner cut");
        let prepared = registry
            .prepare(&spec, &CancellationSignal::new())
            .await
            .unwrap();
        registry
            .commit(prepared, &CancellationSignal::new())
            .await
            .unwrap()
    });
    handoff.wait_until_entered();
    let activation_id = SubagentId::for_conversation(&plane.conversation_id, 1);
    let activation = plane.registry.snapshot(&activation_id).unwrap();
    let fresh = plane
        .registry
        .agent_snapshot_with_activation(&activation.child_agent_id)
        .unwrap();
    assert_eq!(fresh.0.state, AgentState::Admitting);
    assert_eq!(
        fresh.1.state,
        SubagentState::Stopping,
        "finite Starting does not duplicate Agent Admitting"
    );
    let batches = queue.drain();
    let frontier = plane.store.presentation_frontier().unwrap();
    let mut witnessed = Vec::new();
    let mut owners = Vec::new();
    for batch in &batches {
        for fact in batch.facts() {
            match fact {
                ConversationObservation::JournalFact(witness) => {
                    assert!(witness.sequence <= frontier);
                    witnessed.push(witness.sequence);
                }
                ConversationObservation::SubagentLifecycle {
                    agent: Some(owner),
                    snapshot,
                } => {
                    assert_eq!(owner.as_ref(), &fresh.0);
                    assert_eq!(snapshot, &fresh.1);
                    owners.push((owner.as_ref(), snapshot));
                }
                _ => {}
            }
        }
    }
    assert_eq!(witnessed.last(), Some(&frontier));
    assert!(witnessed.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        owners,
        vec![(&fresh.0, &fresh.1)],
        "one complete native owner cut"
    );
    assert!(
        queue.drain().is_empty(),
        "there is no trailing second owner publication"
    );
    // Listing materializes this exact owner cut; later activation progress
    // cannot change the already captured pairs used by Runtime Client.
    let listing = plane.registry.list_agents(MAX_AGENT_LIST_LIMIT);
    assert_eq!(listing.agents, vec![fresh.clone()]);
    handoff.release();
    let SubagentStartOutcome::Accepted(accepted) = committing.await.unwrap() else {
        panic!("owned activation");
    };
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(listing.agents[0], fresh);
    assert_eq!(
        plane.registry.list_agents(MAX_AGENT_LIST_LIMIT).agents[0]
            .0
            .state,
        AgentState::Inactive
    );
}

#[tokio::test]
async fn t15_t20_native_agent_source_shares_immutable_authority_and_releases_owner_lock() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let admitted = start(&plane, &start_spec("native source ownership")).await;
    let cut = plane.registry.observation_cut();
    assert_eq!(cut.total, 1);
    let page = plane.registry.observation_sources_page(cut, 0, 32).unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next_offset.is_none());
    let authority = plane.registry.state.lock().unwrap().agents[&admitted.child_agent_id]
        .authority
        .clone();
    assert!(Arc::ptr_eq(&page.items[0].authority, &authority));
    assert_eq!(
        plane
            .registry
            .observation_ordinal(cut, &admitted.child_agent_id)
            .unwrap(),
        0
    );
    plane
        .registry
        .with_observation_value_by_id(cut, &admitted.child_agent_id, |view| {
            assert_eq!(view.title, authority.title.as_ref());
            assert!(plane.registry.state.try_lock().is_ok());
        })
        .unwrap();
    let original = plane
        .registry
        .agent_snapshot(&admitted.child_agent_id)
        .unwrap();
    plane
        .registry
        .with_observation_value(cut, 0, |view| {
            assert!(
                plane.registry.state.try_lock().is_ok(),
                "native field serialization cannot hold the activation/lifecycle mutex"
            );
            assert_eq!(view.title, authority.title.as_ref());
            assert_eq!(view.state, original.state);
            assert_eq!(view.activation_id, &admitted.subagent_id);
            plane
                .registry
                .cancel(&admitted.subagent_id, CancellationReason::UserRequested)
                .unwrap();
            assert_eq!(
                view.state, original.state,
                "the captured source is immutable across later native cancellation intent"
            );
        })
        .unwrap();
    assert!(plane.registry.observation_sources_page(cut, 0, 1).is_err());
    child.complete(ChildResultStatus::Cancelled, None).await;
    plane
        .registry
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
}

fn child_head_header(
    conversation: &crate::runtime::identity::ConversationId,
    index: u64,
) -> crate::observation::interaction::RuntimeClientInteractionHeader {
    let request = crate::runtime::interaction::InteractionRequest {
        id: crate::runtime::identity::InteractionId::new(format!("child-head-{index}")),
        conversation_id: conversation.clone(),
        execution: rustx::runtime::interaction::InteractionExecution::Attempt {
            attempt_id: crate::runtime::identity::AttemptId::new("child-head-attempt"),
            turn: 1,
        },
        kind: crate::runtime::interaction::InteractionKind::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new(format!(
                    "child-head-call-{index}"
                )),
            },
            tool_id: crate::runtime::identity::ToolId::new("child-head-tool"),
            tool_name: "child-head".into(),
            origin: crate::tools::types::ToolOrigin::Builtin,
            mode: crate::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::Value::Null,
            reason: "head test".into(),
        },
    };
    crate::observation::interaction::interaction_request_header(
        &request,
        crate::runtime::interaction::InteractionSource::Primary,
    )
}

#[tokio::test]
async fn t15_t18_t20_child_pending_head_uses_sparse_native_route_index_and_retires_without_settlement()
 {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let admitted = start(&plane, &start_spec("bounded Child pending head")).await;
    let conversation = plane
        .registry
        .snapshot(&admitted.subagent_id)
        .unwrap()
        .child_conversation_id;
    for index in 0..73 {
        let header = child_head_header(&conversation, index);
        let _ = plane
            .registry
            .apply_child_interaction_requested(&admitted.subagent_id, header);
    }
    let first = plane.registry.pending_interaction_head();
    assert_eq!(first.cut.total, 73);
    assert_eq!(first.head.len(), 32);
    assert_eq!(first.next_sequence, Some(32));
    let second = plane
        .registry
        .pending_interaction_page(
            first.cut,
            Some(first.head.last().unwrap().source_offset / 2),
            32,
        )
        .unwrap();
    assert_eq!(second.head.first().unwrap().source_offset, 65);
    assert_eq!(second.next_sequence, Some(64));
    let third = plane
        .registry
        .pending_interaction_page(
            first.cut,
            Some(second.head.last().unwrap().source_offset / 2),
            32,
        )
        .unwrap();
    assert_eq!(third.head.len(), 9);
    assert_eq!(third.next_sequence, None);
    let source = plane
        .registry
        .pending_interaction_source_at(first.cut, 70)
        .unwrap();
    assert_eq!(source.request_id.as_str(), "child-head-70");
    assert!(source.body.is_none());
    // Repeated delivery cannot overwrite the exact previous route owner or
    // consume a new native sequence/generation.
    let _ = plane
        .registry
        .apply_child_interaction_requested(&admitted.subagent_id, first.head[0].clone());
    assert_eq!(plane.registry.pending_interaction_head().cut, first.cut);
    plane.registry.apply_child_interaction_settled(
        &admitted.subagent_id,
        &first.head[0].interaction,
        &crate::runtime::interaction::InteractionOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    let changed = plane.registry.pending_interaction_head();
    assert_eq!(changed.cut.total, 72);
    assert_eq!(changed.head.first().unwrap().source_offset, 3);
    assert_eq!(changed.head.last().unwrap().source_offset, 65);
    assert!(
        plane
            .registry
            .pending_interaction_source_at(first.cut, 70)
            .is_err()
    );
    assert!(
        plane
            .registry
            .pending_interaction_page(first.cut, None, 32)
            .is_err()
    );
    assert_eq!(
        plane
            .registry
            .pending_interaction_source_at(changed.cut, 70)
            .unwrap(),
        source
    );
    // Actual physical Child termination retires the Root observation index;
    // it does not manufacture an interaction response or terminal audit.
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    let retired = plane.registry.pending_interaction_head();
    assert_eq!(retired.cut.total, 0);
    assert!(retired.head.is_empty());
}

#[tokio::test]
async fn t14_t18_agent_and_routed_pending_errors_preserve_source_cut_and_range() {
    use crate::runtime::identity::{ObservationCollectionCut, ObservationCollectionReadError};
    let plane = plane(4);
    let registry = &plane.registry;
    let cut = registry.observation_cut();
    let missing = AgentId::new("unallocated-native-agent");
    assert!(matches!(
        registry.observation_sources_page(cut, 1, 1),
        Err(ObservationCollectionReadError::InvalidRequest(_))
    ));
    assert!(matches!(
        registry.with_observation_value_by_id(cut, &missing, |_| ()),
        Err(ObservationCollectionReadError::Unavailable(_))
    ));
    assert!(matches!(
        registry.observation_sources_page(
            ObservationCollectionCut {
                generation: cut.generation + 1,
                ..cut
            },
            0,
            1
        ),
        Err(ObservationCollectionReadError::CutExpired)
    ));
    let pending = registry.pending_interaction_head().cut;
    assert!(matches!(
        registry.pending_interaction_page(pending, None, 0),
        Err(ObservationCollectionReadError::InvalidRequest(_))
    ));
    assert!(matches!(
        registry.pending_interaction_source_at(pending, 0),
        Err(ObservationCollectionReadError::Unavailable(_))
    ));
    {
        let mut state = registry.state.lock().unwrap();
        state.observation_generation = u64::MAX;
        state.routed_generation = u64::MAX;
    }
    assert!(matches!(
        registry.observation_sources_page(registry.observation_cut(), 0, 1),
        Err(ObservationCollectionReadError::Unavailable(_))
    ));
    assert!(matches!(
        registry.pending_interaction_page(registry.pending_interaction_head().cut, None, 1),
        Err(ObservationCollectionReadError::Unavailable(_))
    ));
}
