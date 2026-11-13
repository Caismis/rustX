use crate::durable::JournalStore;
use crate::runtime::configuration_commit::{
    ConfigurationChangeReason, DesiredApplication, PreparedConfiguration, StagedConfiguration,
    StagedOutcome,
};
use crate::runtime::conversation_runtime::test_support::*;

/// A context-only candidate over the runtime's current generation, staged
/// under `desired`, reporting its single outcome on the returned receiver.
fn stage(
    runtime: &ConversationRuntime,
    prepared: PreparedConfiguration,
    desired: DesiredApplication,
) -> tokio::sync::oneshot::Receiver<StagedOutcome> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    runtime.stage_configuration(StagedConfiguration {
        prepared,
        desired,
        reason: ConfigurationChangeReason::Source,
        retain: Box::new(|_| {}),
        settled: Box::new(move |outcome| {
            Box::pin(async move {
                let _ = sender.send(outcome);
            })
        }),
    });
    receiver
}

/// Use the current preparation owner so the fixture carries exactly the
/// configuration/context contract a production staged candidate must satisfy.
fn prepare_context(runtime: &ConversationRuntime) -> PreparedConfiguration {
    let resources = runtime.runtime_resources();
    runtime
        .prepare_context_configuration(|configuration| crate::runtime::resources::ContextBinding {
            configuration: configuration.clone(),
            project_context_files: resources.project_context_files().to_vec(),
            skills: resources.capability().skills().clone(),
        })
        .expect("prepare bound context")
}

/// Issue #518: a running subagent genuinely owns the committed generation,
/// so a staged candidate waits for it; a newer desire supersedes the waiting
/// candidate so it can never commit late; and the exact child settlement
/// alone commits the current candidate — no client action, no rebinding by
/// a stale baseline, and no model request beyond the child's own terminal
/// continuation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // scenario bodies are deliberately linear
async fn subagent_settlement_commits_the_current_staged_configuration() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_385a385a-3850-7385-8385-385385385385",
        ))
        .expect("in-memory store"),
    );
    let conversation = ConversationId::new("conv_385a385a-3850-7385-8385-385385385385");
    let agent = AgentId::new("agent-a");
    let (subagents, mut runtime_config) = subagent_runtime_config_with_registry(
        &dir,
        conversation.as_str(),
        store.clone(),
        &conversation,
        &agent,
        &agent,
    )
    .await;
    let model = Arc::new(FakeModel::new(Vec::new()));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    runtime_config.model = scripted_session_model(adapter);
    let config = crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(
        b"agent_id='agent-a'\n[agent]\nskills='all'\n[agent.model]\nmodel='scripted/scripted'\n",
    )
    .expect("valid root configuration");
    runtime_config.resources = Arc::new(runtime_config.resources.as_ref().clone().with_configuration(
        crate::runtime::resources::RuntimeConfiguration {
            config: Arc::new(config),
            models: runtime_config.model.registry().unwrap().clone(),
            resource_definitions: Vec::new(),
            resource_diagnostics: Vec::new(),
            authored: crate::local_runtime::configuration::generation::AuthoredConfiguration {
                component_revisions: std::collections::BTreeMap::new(),
                source_revisions: std::collections::BTreeMap::new(),
                effective: crate::local_runtime::configuration::settings::SourceDocumentView::default(),
                provenance: std::collections::BTreeMap::new(),
            },
        }
    ));
    let runtime = ConversationRuntime::new(runtime_config).expect("bound runtime");
    runtime.activate();
    let baseline = runtime.inner.lock_state().binding_revision;
    let stale_candidate = prepare_context(&runtime);
    let desire = Arc::new(std::sync::atomic::AtomicU64::new(1));

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
                            task: "hold the committed configuration".to_owned(),
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
    // nothing; the subagent registry alone owns the generation.
    assert!(!runtime.tool_runtime().background().configuration_busy());
    assert!(subagents.configuration_busy());
    assert_eq!(
        runtime.idle_epoch(),
        Err(crate::runtime::conversation_runtime::IdleBusyReason::Subagent)
    );

    // Staged behind the child: nothing commits while it owns the generation.
    let superseded = stage(
        &runtime,
        prepare_context(&runtime),
        DesiredApplication::new(desire.clone(), 1),
    );
    assert!(runtime.has_staged_configuration());
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline);
    // Newer desire is captured while V1 waits; V1 can never commit now.
    desire.store(2, std::sync::atomic::Ordering::SeqCst);
    let current = stage(
        &runtime,
        prepare_context(&runtime),
        DesiredApplication::new(desire.clone(), 2),
    );
    assert_eq!(
        superseded.await.expect("superseded outcome"),
        StagedOutcome::Superseded
    );
    assert!(runtime.has_staged_configuration());
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline);

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
    // Settlement alone committed the current candidate, exactly once.
    assert_eq!(
        within_liveness_guard("staged commit", current)
            .await
            .expect("outcome"),
        StagedOutcome::Committed
    );
    assert!(!runtime.has_staged_configuration());
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline + 1);
    // The only model activity across settlement is the orphan result's
    // own continuation: the runtime consumes the terminal notice with one
    // attempt, admitted after the commit. The commit itself invokes no model.
    within_liveness_guard("terminal notice continuation", async {
        loop {
            let settled = runtime.settlement_signal().notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            if !model.requests().is_empty() && !runtime.has_current_attempt() {
                break;
            }
            settled.await;
        }
    })
    .await;
    let requests = model.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        format!("{:?}", requests[0]).contains("terminal-notice"),
        "the single request is the subagent terminal continuation: {:?}",
        requests[0]
    );
    assert!(runtime.inner.durability_failure_diagnostic().is_none());
    // A candidate prepared against the replaced binding is superseded.
    let stale = stage(
        &runtime,
        stale_candidate,
        DesiredApplication::new(desire, 2),
    );
    assert_eq!(
        stale.await.expect("stale outcome"),
        StagedOutcome::Superseded
    );
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline + 1);
    // An identical generation records no change fact.
    assert!(
        !store
            .read_events(None, 256)
            .expect("events")
            .events
            .iter()
            .any(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::ConfigurationApplied { .. }
            )),
        "no effective semantic change, no change fact"
    );
}

/// The Session-level change fact gets its own transcript position, a complete
/// derived layout (so ordered observation never stalls on it), and reads back
/// as a configuration transcript item rather than an Attempt terminal.
#[test]
#[allow(clippy::too_many_lines)] // scenario bodies are deliberately linear
fn configuration_change_fact_is_a_complete_transcript_row() {
    use crate::durable::{CanonicalStore, ObservationStore};
    let conversation = ConversationId::new("conv_385a385a-3850-7385-8385-385385385386");
    let store = crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap();
    let change = crate::runtime::configuration_commit::ConfigurationChange {
        reason: ConfigurationChangeReason::Source,
        previous_binding_revision: 1,
        binding_revision: 2,
        domains: vec![crate::runtime::configuration_commit::ConfigurationDomain::Instructions],
        tools: crate::runtime::configuration_commit::NamedChanges::default(),
        skills: crate::runtime::configuration_commit::NamedChanges::default(),
        instructions_bytes: 14,
        catalog_bytes: 14,
    };
    let envelope = |attempt: Option<crate::runtime::identity::AttemptId>| {
        crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(""),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: attempt,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: crate::events::types::RuntimeEvent::ConfigurationApplied {
                change: change.clone(),
            },
        }
    };
    assert!(
        store
            .append_configuration_change(
                envelope(Some(AttemptId::new("attempt"))),
                "Instructions B",
                "{\"tools\":[\"B\"]}"
            )
            .is_err(),
        "a configuration change belongs to no Attempt"
    );
    assert!(
        store.append_event(envelope(None)).is_err(),
        "only the dedicated transition records a configuration change"
    );
    // A failed append commits nothing: no row, no cursor, no partial fact.
    // The runtime reports that commit as uncertain and absorbs the failure.
    store.arm_fail_event_times(1);
    assert!(
        store
            .append_configuration_change(envelope(None), "Instructions B", "{\"tools\":[\"B\"]}")
            .is_err()
    );
    assert!(
        store
            .load_transcript_page(None, 8)
            .unwrap()
            .entries
            .is_empty()
    );
    let (event, cursor) = store
        .append_configuration_change(envelope(None), "Instructions B", "{\"tools\":[\"B\"]}")
        .unwrap();
    // A second commit before any request: each fact keeps its own
    // instructions and installed catalog.
    let (later, _) = store
        .append_configuration_change(envelope(None), "Instructions C", "{\"tools\":[\"C\"]}")
        .unwrap();
    let instructions: fn(ConversationId, u64, u64) -> crate::observation::content::ContentRef =
        crate::observation::content::ContentRef::configuration_instructions;
    let catalog =
        crate::observation::content::ContentRef::configuration_catalog as fn(_, _, _) -> _;
    for (reference, text) in [
        (instructions, event.sequence, "Instructions B"),
        (instructions, later.sequence, "Instructions C"),
        (catalog, event.sequence, "{\"tools\":[\"B\"]}"),
        (catalog, later.sequence, "{\"tools\":[\"C\"]}"),
    ]
    .map(|(reference, sequence, text)| {
        (
            reference(conversation.clone(), sequence, text.len() as u64),
            text,
        )
    }) {
        let mut read = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = store.content_read(&reference, offset, 5).unwrap();
            read.extend(
                base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &chunk.data_base64,
                )
                .unwrap(),
            );
            match chunk.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(read, text.as_bytes());
    }
    // A reference that names no retained fact or claims other bytes is refused.
    for (sequence, bytes) in [(later.sequence + 7, 14), (event.sequence, 3)] {
        let reference = crate::observation::content::ContentRef::configuration_instructions(
            conversation.clone(),
            sequence,
            bytes,
        );
        assert!(store.content_read(&reference, 0, 64).is_err());
    }
    let witness = store
        .observation_event_witness(event.sequence, event.sequence)
        .unwrap()
        .unwrap();
    assert!(!witness.source_unavailable, "{witness:?}");
    assert_eq!(witness.transcript_cursor, Some(cursor));
    let page = store.load_transcript_page(None, 8).unwrap();
    assert!(matches!(
        &page.entries[..],
        [
            crate::durable::TranscriptEntry {
                item: crate::durable::TranscriptItem::ConfigurationApplied { .. },
                ..
            },
            crate::durable::TranscriptEntry {
                item: crate::durable::TranscriptItem::ConfigurationApplied { .. },
                ..
            }
        ]
    ));
}
