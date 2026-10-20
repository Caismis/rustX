use crate::runtime::conversation_runtime::test_support::*;

/// The headless full turn: a conversation runtime with zero Runtime
/// Client attachments publishes ordinary inbound, runs a real
/// `AgentExecution` through the real Context Assembly / provider path,
/// and settles canonically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_full_turn_without_any_runtime_client() {
    let fixture = headless_fixture().await;

    let admission = fixture
        .runtime
        .submit_inbound(text_content("hello"))
        .expect("accepted");
    assert_eq!(
        admission.message_id.as_str(),
        "conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-1"
    );
    assert_eq!(admission.inbound_sequence.get(), 1);

    // The terminal settlement is observable through the runtime-owned
    // observation queue: the Agent Loop emits exactly one terminal
    // event, then the settlement handoff restores the conversation.
    let terminal = await_observation(
        fixture
            .pending
            .as_ref()
            .expect("the bridged fixture carries its queue"),
        |observation| {
            matches!(
                observation,
                ConversationObservation::Event { event, .. } if is_terminal_event(event)
            )
        },
    )
    .await;
    assert_eq!(
        count_terminal(&terminal),
        1,
        "exactly one terminal settlement event"
    );

    // The canonical conversation committed the inbound message and the
    // assistant reply: the same real AgentExecution path an interactive
    // runtime uses.
    let ledger = await_settled_ledger(&fixture.runtime, 0).await;
    let roles: Vec<&str> = ledger
        .iter()
        .map(|message| match message {
            MessageBlock::User(_) => "user",
            MessageBlock::Assistant(_) => "assistant",
            MessageBlock::Tool(_) => "tool",
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "user", "assistant"],
        "inbound + admitted Agent Status + assistant reply"
    );
    // The real provider path observed the inbound message.
    let requests = fixture.model.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages.iter().any(|message| {
        matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == admission.message_id)
    }));
    // The attempt was admitted exactly once and the runtime is idle
    // again.
    assert!(!fixture.runtime.has_current_attempt());
    assert_eq!(
        request_snapshots(&fixture.runtime.request_history()).len(),
        1
    );
}

/// The mailbox and every full-store operation are derived from one
/// `ConversationStoreBinding`. Two independent stores may happen to use
/// the same `ConversationId`, but there is no production constructor that
/// can pair one store's mailbox with the other's canonical/event store.
/// The runtime also performs a normal turn without enumerating its
/// historical Request Snapshots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_uses_one_durable_binding_without_snapshot_enumeration() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_e80836f4-9cfc-7703-834b-1d9831f9f8e5");
    let store_a = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("store A"),
    );
    let store_b = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id).expect("store B"),
    );
    let (runtime, model) = headless_runtime_over_store(
        &dir,
        "conv_e80836f4-9cfc-7703-834b-1d9831f9f8e5",
        store_a.clone(),
    )
    .await;
    runtime.activate();

    let accepted = runtime
        .submit_inbound(text_content("one durable authority"))
        .expect("accepted");
    let ledger = await_settled_ledger(&runtime, 0).await;

    assert!(ledger.iter().any(|message| {
        matches!(message, MessageBlock::User(user) if user.id == accepted.message_id)
    }));
    assert_eq!(model.requests().len(), 1, "one normal admitted turn");
    assert_eq!(store_a.request_snapshot_page_reads(), 0);
    assert!(
        store_b
            .load_canonical()
            .expect("store B canonical")
            .is_empty(),
        "an independent same-id store never becomes a hidden second authority"
    );
    assert!(
        store_b
            .read_events(None, 32)
            .expect("store B events")
            .events
            .is_empty(),
        "the Event Journal also remains on the bound store"
    );
}

/// The headless real tool cycle (Issue #61): a conversation runtime
/// with **zero** observation bridge and zero Runtime Client
/// attachments executes a real tool turn end to end:
///
/// ```text
/// inbound -> AgentExecution -> provider ToolCall
///   -> real ConversationToolRuntime execution
///   -> canonical ToolResult commit
///   -> same attempt's next model step (observing the ToolResult)
///   -> final assistant result -> terminal settlement
///   -> ConversationRuntime regains the authoritative ConversationState
/// ```
///
/// The production `AgentExecution` / Context Assembly / `ToolRuntime` /
/// `CapabilityCoordinator` path is used unchanged; the test observes
/// only the runtime-owned observation stream and authoritative state.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_tool_cycle_with_zero_attachments() {
    use crate::message::types::ContentBlockIndex;
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::scripted_suites::support::fake::{FakeTool, success_result};
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy};

    // The real tool: registered in the base registry the capability
    // coordinator composes over, executed by the production
    // ConversationToolRuntime path.
    let definition = ToolDefinition {
        id: ToolId::new("tool-echo"),
        name: "echo".to_owned(),
        description: "echoes the call arguments".to_owned(),
        input_schema: serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: crate::tools::types::ToolConcurrencyPolicy::default(),
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let tool = FakeTool::new(definition.clone(), success_result("echoed"));
    let calls = tool.calls();
    let mut registry = ToolRegistry::new();
    tool.register(&mut registry);

    let dir = tempfile::tempdir().expect("temp dir");
    let call_id = "call-echo";
    let scripts = vec![
        // Turn 1: one real tool call.
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: ToolCallId::new(call_id),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new(call_id),
                arguments_delta: "{\"text\":\"hi\"}".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: ToolCallId::new(call_id),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                    arguments: serde_json::json!({"text": "hi"}),
                },
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ],
        // Turn 2: the final assistant answer.
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "tool worked".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ];
    // Zero observation bridge, zero client: the runtime runs the exact
    // production path with no consumer attached.
    let (runtime, model) = headless_runtime(&dir, scripts, Some(registry), None).await;
    runtime.activate();

    let admission = runtime
        .submit_inbound(text_content("use echo"))
        .expect("accepted");

    // Deterministic settlement handoff: the settlement signal fires
    // once when the authoritative ConversationState returns to the
    // runtime.
    runtime.child_attempt_settlement(0).await;
    let ledger = runtime.coordinator_ledger().expect("settled");

    // The real tool executed exactly once under the canonical ToolCall
    // identity.
    let invocations = calls.borrow().clone();
    assert_eq!(invocations.len(), 1, "exactly one real tool execution");
    assert_eq!(invocations[0].tool_name, "echo");
    assert_eq!(invocations[0].arguments, serde_json::json!({"text": "hi"}));

    // Structural ordering: inbound user + Agent Status + the Assistant
    // ToolCall message + the canonical ToolResult + the final
    // assistant reply.
    let roles: Vec<&str> = ledger
        .iter()
        .map(|message| match message {
            MessageBlock::User(_) => "user",
            MessageBlock::Assistant(_) => "assistant",
            MessageBlock::Tool(_) => "tool",
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "user", "assistant", "tool", "assistant"],
        "inbound + status + tool call + canonical tool result + final reply"
    );
    // The canonical ToolResult commits under the real call identity.
    let tool_message = ledger
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("a ToolMessage was committed");
    assert_eq!(tool_message.tool_call_id.as_str(), call_id);
    assert_eq!(tool_message.tool_id.as_str(), "tool-echo");

    // Two real primary provider requests; the second observes the
    // canonical ToolResult.
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "two primary provider requests");
    assert!(requests[0].messages.iter().any(|message| {
        matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == admission.message_id)
    }));
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::Tool(tool)) if tool.tool_call_id.as_str() == call_id)),
        "the second provider request observes the canonical ToolResult"
    );

    // Terminal settlement happened exactly once and the runtime owns
    // the authoritative state again, with the request facts retained.
    assert!(!runtime.has_current_attempt());
    let history = runtime.request_history();
    let snapshots = request_snapshots(&history);
    assert_eq!(snapshots.len(), 2, "both requests retained");
    assert_eq!(
        snapshots[0].runtime_resource_revision, snapshots[1].runtime_resource_revision,
        "tool continuation retains the attempt-admitted resource generation"
    );
    assert_eq!(
        snapshots[0].capability_revision, snapshots[1].capability_revision,
        "tool continuation retains the attempt capability revision"
    );
    assert_eq!(snapshots[0].tool_definitions, snapshots[1].tool_definitions);
    assert_eq!(
        snapshots[0].effective_system_prompt,
        snapshots[1].effective_system_prompt
    );
    // The coordinator owns the one authoritative ConversationState
    // again: the runtime keeps no second derived read model to reset.
    assert!(runtime.coordinator_ledger().is_some());
}

/// The idle async wakeup: an idle conversation runtime with a purely
/// asynchronous inbound enqueue admits exactly one attempt without any
/// client command.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_async_inbound_wakes_the_runtime_without_a_client_request() {
    let fixture = headless_fixture().await;

    // A purely async producer: a Runtime-source message enqueued
    // directly into the authoritative mailbox. No client exists at all.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(runtime_inbound_message(
            "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4",
        ))
        .expect("enqueue");

    // No client command: the runtime wake gate admits exactly one
    // attempt.
    fixture.runtime.child_attempt_settlement(0).await;
    assert_eq!(
        fixture.model.requests().len(),
        1,
        "exactly one attempt was admitted"
    );
    let ledger = fixture.runtime.coordinator_ledger().expect("settled");
    assert!(
        ledger.iter().any(|message| matches!(
            message,
            MessageBlock::User(user) if user.id.as_str() == "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4"
        )),
        "the asynchronous inbound was consumed exactly once"
    );
}

/// The async-wake vs client-submit race: while an admission is gated,
/// an async enqueue and a client submit both land in the mailbox; the
/// single admission drains one finite batch in mailbox order and
/// admits exactly one attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_wake_vs_client_submit_admits_exactly_one_attempt() {
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        admission_gate: Some(gate.clone()),
        settlement_gate: None,
        before_restore_gate: None,
        activation_gate: None,
        manual_compaction_settlement_gate: None,
        submit_gate: None,
        submit_arrival: None,
        shutdown_arrival: Some(Arc::new(tokio::sync::Notify::new())),
        mcp_failure_drain_gate: None,
        drain_linearization: None,
        start_boundary_pause: None,
        model_arbitration_pause: None,
        tool_start_pause: None,
        drain_supervision: None,
        attempt_exit_gate: None,
        parent_guidance_seal_gate: None,
        background_failure_gate: None,
        subagent_failure_published_gate: None,
        ..CoordinatorProbe::default()
    }))
    .await;
    gate.arm();

    // The async producer enqueues first; the wake gate starts an
    // admission that parks before the coordinator lock.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(runtime_inbound_message(
            "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4",
        ))
        .expect("async enqueue");
    gate.wait_entered();

    // The client submit races the gated admission: it can still publish
    // because the admission parks before the coordinator lock.
    fixture
        .runtime
        .submit_inbound(text_content("client"))
        .expect("client submit accepted");

    // Release: one admission drains both messages in mailbox order.
    gate.release();
    let mut terminal_seen = false;
    let mut adoption_seen = false;
    let observations = await_observation(
        fixture
            .pending
            .as_ref()
            .expect("the bridged fixture carries its queue"),
        |observation| {
            terminal_seen |= matches!(
                observation,
                ConversationObservation::Event { event, .. } if is_terminal_event(event)
            );
            adoption_seen |= matches!(
                observation,
                ConversationObservation::PendingReceipt(receipt)
                    if receipt.adopted_through.is_some()
            );
            terminal_seen && adoption_seen
        },
    )
    .await;

    assert_eq!(
        fixture.model.requests().len(),
        1,
        "at most one active attempt"
    );
    let ledger = await_settled_ledger(&fixture.runtime, 0).await;
    let inbound: Vec<&str> = ledger
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user) => Some(user.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inbound[0], "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4");
    // The client submit is the second item of the one durable sequence
    // domain, so its deterministic identity derives from sequence 2.
    assert!(inbound.contains(&"conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-2"));
    // Exactly one finite drain was observed, carrying both messages in
    // mailbox order.
    let drained: Vec<_> = observations
        .iter()
        .filter_map(|observation| match observation {
            ConversationObservation::PendingReceipt(receipt)
                if receipt.adopted_through.is_some() =>
            {
                Some(receipt)
            }
            _ => None,
        })
        .collect();
    assert_eq!(drained.len(), 1, "one finite inbound batch");
    assert_eq!(
        drained[0].adopted_count,
        Some(2),
        "both messages in one native adoption"
    );
}

/// Durable pending inbound accepted before the runtime exists (for
/// example by a crashed process) drives admission on recovery without a
/// new client request (Issue #63 recovery seam).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_durable_pending_drives_admission_without_a_client_request() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let artifacts = dir.path().join("artifacts");

    // First process: accept one inbound item durably, then die before
    // any adoption.
    {
        let _tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
            crate::tools::runtime::ConversationRuntimeConfig::new(&workspace, &artifacts)
                .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                )),
        )
        .expect("tool runtime");
        let store = crate::durable::SqliteConversationStore::open(
            ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
            &artifacts.join("conversation.sqlite"),
        )
        .map(Arc::new)
        .expect("durable store");
        store
            .accept_inbound(crate::durable::inbox::InboundDraft {
                message_id: None,
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: text_content("recovered"),
                timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
                correlation: None,
            })
            .expect("durable acceptance");
    }

    // Second process: reconstruct the runtime over the same durable
    // store and activate. The recovered pending item drives admission by
    // itself, without any new submit.
    let (runtime, model) = headless_runtime(&dir, vec![one_turn_script()], None, None).await;
    runtime.activate();
    let ledger = await_settled_ledger(&runtime, 0).await;
    let inbound: Vec<&str> = ledger
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user) if user.kind == InboundKind::Message => Some(user.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        inbound,
        vec!["conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-1"],
        "the recovered pending item is adopted exactly once"
    );
    assert_eq!(model.requests().len(), 1, "one admitted attempt");
    assert!(
        runtime
            .tool_runtime()
            .mailbox()
            .select_pending_batch()
            .expect("select")
            .is_none(),
        "no pending record remains after recovery adoption"
    );
}

/// External resource edits made while an admitted provider call is
/// parked cannot splice a new generation into that attempt's automatic
/// overflow-compaction retry. Project instructions, Skill metadata, and
/// Tool definitions all remain byte-identical across the retry.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_compaction_keeps_attempt_pinned_resources_after_external_edits() {
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;

    fn tool_registry(id: &str, name: &str, description: &str) -> ToolRegistry {
        let definition = ToolDefinition {
            id: ToolId::new(id),
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::default(),
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let tool = FakeTool::new(definition, success_result("unused"));
        let mut registry = ToolRegistry::new();
        tool.register(&mut registry);
        with_native_read(registry)
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    let project_path = workspace.join("AGENTS.md");
    let skill = workspace.join(".agents/skills/pinned-skill");
    let skill_markdown = skill.join("SKILL.md");
    std::fs::create_dir_all(&skill).expect("Skill package");
    std::fs::write(&project_path, "project authority generation A").expect("project generation A");
    std::fs::write(
        &skill_markdown,
        "---\nname: pinned-skill\ndescription: Skill generation A.\n---\n\nBody A.\n",
    )
    .expect("Skill generation A");

    let (release, release_rx) = model_release();
    let overflow_script = vec![
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::ContextWindowExceeded,
                message: "context window exceeded".to_owned(),
                retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
        }),
    ];
    let (runtime, model) = headless_runtime_with_options(
        &dir,
        vec![overflow_script, text_turn_script("s"), one_turn_script()],
        Some(tool_registry(
            "tool-generation-a",
            "tool_generation_a",
            "Tool generation A",
        )),
        None,
        HeadlessRuntimeOptions {
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                skill.parent().unwrap(),
            ),
            initial_messages: vec![seed_user(
                "old",
                "old history that the overflow compaction retires: this seed carries enough \
                 retired content that replacing it with the compact summary is measurable \
                 progress even with the validated metadata the immutable checkpoint carries",
            )],
            project_context_files: vec![project_file(
                &project_path,
                "project authority generation A",
            )],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("fresh request"))
        .expect("fresh request");
    let mut parked = model.parked();
    parked
        .wait_for(|is_parked| *is_parked)
        .await
        .expect("the first provider request is parked");

    std::fs::write(&project_path, "project authority generation B")
        .expect("edit project authority");
    std::fs::write(
        &skill_markdown,
        "---\nname: pinned-skill\ndescription: Skill generation B.\n---\n\nBody B.\n",
    )
    .expect("edit Skill metadata");
    release
        .send(true)
        .expect("release the first provider request");
    let _ = await_settled_ledger(&runtime, 0).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 3, "overflow, isolated summary, and retry");
    assert!(
        requests[0]
            .effective_system_prompt
            .contains("project authority generation A")
    );
    assert!(
        requests[0]
            .effective_system_prompt
            .contains("Skill generation A.")
    );
    assert!(!requests[0].effective_system_prompt.contains("generation B"));
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "tool_generation_a")
    );
    assert!(
        requests[0]
            .tools
            .iter()
            .all(|tool| tool.name != "tool_generation_b")
    );

    assert!(requests[1].tools.is_empty());
    assert!(requests[1].effective_system_prompt.is_empty());
    assert!(requests[1].continuation.is_none());

    assert_eq!(
        requests[2].effective_system_prompt, requests[0].effective_system_prompt,
        "the retry keeps the admitted System bytes"
    );
    assert_eq!(
        requests[2].tools, requests[0].tools,
        "the retry keeps the admitted Tool definitions"
    );
    assert!(
        requests[2]
            .effective_system_prompt
            .contains("project authority generation A")
    );
    assert!(
        requests[2]
            .effective_system_prompt
            .contains("Skill generation A.")
    );
    assert!(!requests[2].effective_system_prompt.contains("generation B"));
    assert!(
        requests[2]
            .tools
            .iter()
            .all(|tool| tool.name != "tool_generation_b")
    );
}

/// Issue #204 (case L, runtime level): the policy frozen at runtime
/// construction is the exact policy the admitted execution runs under.
/// The runtime is composed with a 10s hard deadline on the manual
/// monotonic clock; when the clock crosses that deadline, the loop fires
/// cancellation intent for exactly this execution and the
/// executor-proven `Cancelled` settles as the canonical `TimedOut` —
/// both facts journaled through the real attempt-admission path.
///
/// Happens-before: the executor publishes `started` only after its
/// executor-start frontier — at which point the frozen liveness window
/// is already running — and only then is the clock advanced, so the
/// deadline winner is provably the construction-time hard deadline and
/// no wall clock is involved. The settlement waiter is registered
/// before the advance, so the winner cannot settle unobserved.
#[allow(clippy::too_many_lines)] // one linear deterministic scenario
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_attempt_executes_under_the_frozen_tool_deadline_policy() {
    let (tool, mut started, _observed) = CauseProbeTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_35fc42a3-4f04-7145-8c5b-1a37fc603ef5");
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
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id,
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = fixture_activation(&tool_runtime);
                activation.profile.tools.builtin = registry
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(registry),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let model = Arc::new(FakeModel::new(vec![script, one_turn_script()]));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
            std::time::Duration::from_secs(10),
            None,
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
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    };
    let runtime = ConversationRuntime::with_test_monotonic_clock(
        config,
        clock.clone() as Arc<dyn crate::runtime::MonotonicClock>,
    )
    .expect("runtime composition");
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start at its executor-start frontier",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let settled = runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    clock.advance(10_000);
    within_liveness_guard("the admitted attempt to settle", settled).await;
    within_liveness_guard("runtime shutdown", runtime.shutdown())
        .await
        .expect("drain reaches quiescence");

    let events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("events")
        .events;
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionDeadlineFired {
                kind: crate::tools::deadline::ToolDeadlineKind::Hard,
                ..
            }
        )),
        "the frozen construction-time hard deadline fired for the admitted execution"
    );
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionCancellationRequested {
                cause: crate::tools::deadline::ToolCancellationCause::Deadline(
                    crate::tools::deadline::ToolDeadlineKind::Hard
                ),
                ..
            }
        )),
        "the deadline winner's physical cancellation request is journaled"
    );
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: crate::tools::deadline::ToolSettlementCertainty::Confirmed,
                ..
            }
        )),
        "the executor's proven settlement is journaled"
    );
    let completed = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1, "exactly one canonical tool result");
    assert!(
        matches!(completed[0].status, ToolExecutionStatus::TimedOut),
        "executor-proven cancellation after the deadline is the proven timeout: {:?}",
        completed[0].status
    );
}
