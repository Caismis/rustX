use crate::runtime::conversation_runtime::test_support::*;

/// Issue #385: a running subagent is the busy owner of the native
/// configuration-adoption gate. The gate reports Busy from the real
/// registry lifecycle, explicit adoption is refused typed, and the exact
/// child settlement restores Eligible with the pending candidate
/// untouched — no automatic adoption, no model request, no rebinding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // scenario bodies are deliberately linear
async fn subagent_settlement_restores_configuration_adoption_eligibility() {
    use crate::runtime::adoption::{AdoptionEligibility, AdoptionError, PreparedConfiguration};
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_385a385a-3850-7385-8385-385385385385",
        ))
        .expect("in-memory store"),
    );
    let (runtime, model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_385a385a-3850-7385-8385-385385385385",
        store.clone(),
        None,
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // A pending, ready candidate owned by this runtime at the current
    // binding baseline. General context adoption still requires idle ownership.
    let mut prepared = {
        let state = runtime.inner.lock_state();
        Some(PreparedConfiguration {
            owner: runtime.inner.configuration_owner.clone(),
            baseline: state.binding_revision,
            preview: state.resources.clone(),
            model: state.model.clone(),
            capability: None,
            resources: None,
            impact: crate::model::request_shape::CacheImpact::Unproven,
        })
    };
    let baseline = prepared.as_ref().expect("candidate").baseline;
    assert_eq!(
        runtime.configuration_adoption_eligibility(),
        AdoptionEligibility::Eligible
    );
    let mut published = None;
    published_eligibility_until(&pending, &mut published, AdoptionEligibility::Eligible).await;

    // The busy owner is a real Running child record: committed through
    // the registry and delegated over the staged control channel.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("busy-child"));
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
                            task: "hold the adoption gate".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-busy-subagent"),
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
        .expect("commit")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("subagent unexpectedly rolled back")
        }
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    // Attribution: no foreground attempt and the background plane owns
    // nothing; the subagent registry alone owns the Busy lifecycle.
    assert!(!runtime.tool_runtime().background().configuration_busy());
    assert!(subagents.configuration_busy());
    assert_eq!(
        runtime.idle_epoch(),
        Err(crate::runtime::conversation_runtime::IdleBusyReason::Subagent)
    );
    assert_eq!(
        runtime.configuration_adoption_eligibility(),
        AdoptionEligibility::Busy
    );
    // The running child's admission made the runtime publish Busy.
    published_eligibility_until(&pending, &mut published, AdoptionEligibility::Busy).await;
    assert_eq!(
        runtime.adopt_configuration(&mut prepared, baseline, true, || Ok(())),
        Err(AdoptionError::Busy)
    );
    assert!(prepared.is_some(), "a Busy refusal retains the candidate");

    // The exact child settles through its real terminal publication.
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("done".to_owned()),
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("child result");
    let settled = within_liveness_guard(
        "busy subagent settlement",
        subagents.wait_until_settled(&accepted.subagent_id),
    )
    .await
    .expect("child settles");
    assert_eq!(
        settled.state,
        crate::runtime::subagent::SubagentState::Succeeded
    );
    assert!(!subagents.configuration_busy());
    // The settling child releases its lifecycle admission guard after the
    // record is already terminal, and the terminal notice's continuation
    // attempt holds its own admission. The runtime publishes Eligible
    // only once the last of those owners releases; nothing polls.
    published_eligibility_until(&pending, &mut published, AdoptionEligibility::Eligible).await;
    assert!(runtime.idle_epoch().is_ok());

    // The pending candidate survived untouched: the binding baseline
    // never advanced (no automatic adoption) and nothing re-prepared it.
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline);
    assert_eq!(
        prepared.as_ref().expect("candidate retained").baseline,
        baseline
    );
    // The only model activity across settlement is the orphan result's
    // own continuation: the runtime consumes the terminal notice with one
    // attempt. The eligibility transition itself invokes no model.
    let requests = model.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        format!("{:?}", requests[0]).contains("terminal-notice"),
        "the single request is the subagent terminal continuation: {:?}",
        requests[0]
    );
    assert!(runtime.inner.durability_failure_diagnostic().is_none());
    // Fencing still rejects a stale binding after eligibility returns.
    assert_eq!(
        runtime.adopt_configuration(&mut prepared, baseline + 1, true, || Ok(())),
        Err(AdoptionError::Conflict)
    );
    assert!(
        prepared.is_some(),
        "a Conflict refusal retains the candidate"
    );
    assert_eq!(
        runtime.adopt_configuration(&mut prepared, baseline, true, || Ok(())),
        Ok(baseline + 1)
    );
    assert!(
        prepared.is_none(),
        "the successful adoption consumed the candidate"
    );
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline + 1);
    assert_eq!(
        model.requests().len(),
        1,
        "adoption invoked no model request"
    );
}
