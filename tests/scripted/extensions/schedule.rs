//! Real `AgentExecution` grants and native Schedule command receipts.
use super::*;
use crate::scheduler::ScheduleBinding;

fn schedule_tools() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    for registration in crate::tools::native::schedule_test_registrations() {
        tools
            .register_with_execution_metadata(
                registration.definition,
                registration.executor,
                registration.normalizer,
                registration.foreground,
            )
            .unwrap();
    }
    tools
}
fn bind(tools: &rustx::tools::runtime::ConversationToolRuntime) {
    tools
        .schedule()
        .bind(
            ScheduleBinding {
                session_id: crate::runtime::identity::SessionId::new(
                    "ses_01900000-0000-7000-8000-000000000001",
                ),
                node_id: crate::runtime::identity::SessionNodeId::new(
                    "node_01900000-0000-7000-8000-000000000001",
                ),
                conversation_id: tools.conversation_id().clone(),
            },
            Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
            Arc::new(tokio::sync::Notify::new()),
        )
        .unwrap();
}
fn reminder(id: &'static str, title: &str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id: "native.schedule_command",
        name: "schedule_command",
        arguments: serde_json::json!({"mutation":{"type":"create","title":title,"text":"Requested reminder","timing":{"type":"after","seconds":600}}}),
    }
}
fn statuses(result: &AgentExecutionResult) -> Vec<serde_json::Value> {
    result
        .messages()
        .iter()
        .filter_map(|message| {
            if let MessageBlock::Tool(tool) = message {
                tool.result.content.iter().find_map(|part| {
                    if let rustx::tools::types::ToolResultContent::Json { value } = part {
                        Some(value.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        })
        .collect()
}
#[tokio::test]
async fn scheduler85_two_reminders_and_reused_provider_call_ids_have_distinct_native_occurrences() {
    let extensions = NativeAgentExtensions::none();
    let (_dir, tools) = todo_tool_runtime("conv_01900000-0000-7000-8000-000000000001", &extensions);
    bind(&tools);
    let model = fake_model(vec![
        tool_turn(&[reminder("same-call", "Morning")]),
        tool_turn(&[reminder("same-call", "Evening")]),
        stop_turn(),
    ]);
    let (result, _) = run_with_text(
        &extensions,
        &tools,
        schedule_tools(),
        model,
        "Remind me in the morning and evening",
    )
    .await;
    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
    let rows = tools.durable_store().schedule_list(0, 100).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Morning");
    assert_eq!(rows[1].title, "Evening");
    let receipts = statuses(&result);
    assert_eq!(receipts.len(), 2);
    assert_eq!(
        receipts[0]["receipt"]["request_budget"]["mutations_used"],
        1
    );
    assert_eq!(
        receipts[1]["receipt"]["request_budget"]["mutations_used"],
        2
    );
    assert_eq!(
        receipts[1]["receipt"]["request_budget"]["human_message_id"],
        "ext256-inbound"
    );
}
#[tokio::test]
async fn scheduler85_ninth_model_mutation_is_refused_without_renewing_the_request() {
    let extensions = NativeAgentExtensions::none();
    let (_dir, tools) = todo_tool_runtime("conv_01900000-0000-7000-8000-000000000002", &extensions);
    bind(&tools);
    let calls = ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
        .into_iter()
        .map(|id| reminder(id, id))
        .collect::<Vec<_>>();
    let model = fake_model(vec![tool_turn(&calls), stop_turn()]);
    let (result, _) = run_with_text(
        &extensions,
        &tools,
        schedule_tools(),
        model,
        "Create these nine reminders",
    )
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(
        tools.durable_store().schedule_list(0, 100).unwrap().len(),
        8
    );
    let results = statuses(&result);
    assert_eq!(results.len(), 9);
    assert_eq!(results[7]["receipt"]["request_budget"]["mutations_used"], 8);
    assert_eq!(results[8]["status"], "mutation_budget_exhausted");
}

async fn interleaving(source: UserSource, newer_human: bool) {
    let extensions = NativeAgentExtensions::none();
    let (_dir, tools) = todo_tool_runtime("conv_01900000-0000-7000-8000-000000000003", &extensions);
    bind(&tools);
    let (release, wait) = support::fake::model_release();
    let mut first = tool_turn(&[ScriptedCall {
        id: "read",
        tool_id: "native.list_schedules",
        name: "list_schedules",
        arguments: serde_json::json!({}),
    }]);
    first.insert(1, FakeStep::ParkUntilReleased(wait));
    let model = fake_model(vec![
        first,
        tool_turn(&[reminder("create", "Explicit requested reminder")]),
        stop_turn(),
    ]);
    let capability = common::capability_lease(schedule_tools(), &tools).await;
    let mut initial = inbound("initial-user-request", "Please create a reminder");
    initial.source = source.clone();
    let trigger =
        InitialTurnTrigger::FreshInbound(FreshInboundTurn::new(vec![initial.id.clone()]).unwrap());
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("schedule-root"),
        conversation_id: tools.conversation_id().clone(),
        attempt_id: AttemptId::new("schedule-request"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(initial),
        ])
        .unwrap(),
        initial_turn_trigger: trigger,
        model: support::attempt_model(model.clone(), "schedule-model"),
    };
    let mailbox = tools.mailbox();
    let mut parked = model.parked();
    let controller = tokio::spawn(async move {
        parked.wait_for(|parked| *parked).await.unwrap();
        if newer_human {
            mailbox
                .enqueue(inbound(
                    "human-before-latest",
                    "First explicit reminder request",
                ))
                .unwrap();
            mailbox
                .enqueue(inbound("latest-human", "Make the requested reminder"))
                .unwrap();
        }
        let mut tail = inbound(
            "runtime-notification",
            "A notification; this is not new scheduling authority",
        );
        tail.source = UserSource::Runtime;
        mailbox.enqueue(tail).unwrap();
        release.send(true).unwrap();
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(&model, &extensions),
        &tools,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    controller.await.unwrap();
    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
    let rows = tools.durable_store().schedule_list(0, 100).unwrap();
    if source == UserSource::Human || newer_human {
        assert_eq!(rows.len(), 1);
        let receipts = statuses(&result);
        let applied = receipts
            .iter()
            .find(|value| value["status"] == "applied")
            .unwrap();
        assert_eq!(
            applied["receipt"]["request_budget"]["human_message_id"],
            if newer_human {
                "latest-human"
            } else {
                "initial-user-request"
            }
        );
        assert_eq!(applied["receipt"]["request_budget"]["mutations_used"], 1);
    } else {
        assert!(rows.is_empty());
        assert!(result.messages().iter().any(|message|matches!(message,MessageBlock::Tool(tool) if matches!(&tool.result.status,rustx::tools::types::ToolExecutionStatus::Failed{error} if error.contains("live current Human request")))));
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler85_runtime_notification_inside_human_request_preserves_the_grant() {
    interleaving(UserSource::Human, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler85_latest_human_wins_even_when_runtime_follows_in_the_same_batch() {
    interleaving(UserSource::Human, true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler85_independent_runtime_activation_has_no_human_grant() {
    interleaving(UserSource::Runtime, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler85_fresh_human_handoff_can_authorize_an_eligible_root_runtime_request() {
    interleaving(UserSource::Runtime, true).await;
}

#[tokio::test]
async fn scheduler85_historical_human_transcript_does_not_reopen_request_authority() {
    let extensions = NativeAgentExtensions::none();
    let (_dir, tools) = todo_tool_runtime("conv_01900000-0000-7000-8000-000000000004", &extensions);
    bind(&tools);
    let model = fake_model(vec![
        tool_turn(&[reminder("history", "Must be refused")]),
        stop_turn(),
    ]);
    let capability = common::capability_lease(schedule_tools(), &tools).await;
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("schedule-root"),
        conversation_id: tools.conversation_id().clone(),
        attempt_id: AttemptId::new("independent-recovered-request"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(inbound("old-human", "An old reminder request")),
        ])
        .unwrap(),
        initial_turn_trigger: InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "schedule-model"),
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(&model, &extensions),
        &tools,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert!(
        tools
            .durable_store()
            .schedule_list(0, 100)
            .unwrap()
            .is_empty()
    );
    assert!(result.messages().iter().any(|message|matches!(message,MessageBlock::Tool(tool) if matches!(&tool.result.status,rustx::tools::types::ToolExecutionStatus::Failed{error} if error.contains("live current Human request")))));
}
#[tokio::test]
async fn scheduler85_fresh_human_text_without_native_root_binding_cannot_create_a_schedule() {
    let extensions = NativeAgentExtensions::none();
    let (_dir, tools) = todo_tool_runtime("conv_01900000-0000-7000-8000-000000000005", &extensions);
    let model = fake_model(vec![
        tool_turn(&[reminder("child-like", "No root binding")]),
        stop_turn(),
    ]);
    let (result, _) = run_with_text(
        &extensions,
        &tools,
        schedule_tools(),
        model,
        "Explicit reminder text copied into an unbound execution",
    )
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert!(
        tools
            .durable_store()
            .schedule_list(0, 100)
            .unwrap()
            .is_empty()
    );
    assert!(result.messages().iter().any(|message|matches!(message,MessageBlock::Tool(tool) if matches!(&tool.result.status,rustx::tools::types::ToolExecutionStatus::Failed{error} if error.contains("native root Session binding")))));
}
#[test]
fn scheduler85_schedule_control_has_no_child_or_fixed_workflow_materialization() {
    for registration in crate::tools::native::schedule_test_registrations() {
        assert!(
            crate::tools::native::subagent_child_definition(
                &registration.definition.name,
                crate::tools::types::ToolInvocationPolicy::default()
            )
            .is_none()
        );
        assert!(crate::tools::native::is_domain_control(
            &registration.definition.name
        ));
    }
    assert!(
        crate::runtime::workflow::selectable_builtin_tools()
            .iter()
            .all(|name| !crate::tools::native::SCHEDULE_TOOL_NAMES.contains(&name.as_str()))
    );
}
