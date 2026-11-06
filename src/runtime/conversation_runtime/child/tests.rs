use crate::runtime::conversation_runtime::test_support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one ordered ownership transaction
async fn child_transcript_invalid_limits_precede_unavailable_history_access() {
    use crate::runtime_client::RuntimeClientError;
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
        store.clone(),
        None,
    )
    .await;
    let host = crate::runtime_client::RuntimeClientHost::new(
        crate::runtime_client::RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
    )
    .expect("host");
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    runtime.activate();

    // Ownership commits first, while the runtime is healthy.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
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
                            task: "owned".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-owned"),
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
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));

    // The staged child commits real ownership but never creates a child
    // database. Its existing history is therefore deterministically unavailable.
    let unknown = crate::runtime::identity::AgentId::new("unknown-child");
    assert!(matches!(attachment.agent_statistics(&unknown).await,
        Err(RuntimeClientError::UnknownAgent { agent_id }) if agent_id == unknown));
    assert!(matches!(
        attachment.agent_statistics(&accepted.child_agent_id).await,
        Err(RuntimeClientError::RuntimeFailure { .. })
    ));
    for id in [&accepted.child_agent_id, &unknown] {
        for limit in [0, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX + 1] {
            assert!(matches!(
                attachment.agent_transcript_window(
                    id,
                    &crate::durable::reading::ConversationWindowAt::Latest,
                    limit
                ),
                Err(RuntimeClientError::InvalidRequest { .. })
            ));
        }
    }
    for limit in [1, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX] {
        assert!(matches!(
            attachment.agent_transcript_window(&unknown, &crate::durable::reading::ConversationWindowAt::Latest, limit),
            Err(RuntimeClientError::UnknownAgent { agent_id }) if agent_id == unknown
        ));
        assert!(matches!(
            attachment.agent_transcript_window(&accepted.child_agent_id, &crate::durable::reading::ConversationWindowAt::Latest, limit),
            Err(RuntimeClientError::RuntimeFailure { message })
                if message.starts_with("subagent history unavailable:")
        ));
    }
    let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
    subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
}
