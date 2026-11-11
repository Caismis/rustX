// Fixed per-turn observation work of a subscribed native host.
//
// Each scenario runs complete text or tool turns through the production
// runtime, publication and projection worker with one attached subscriber,
// then counts every SQL statement the conversation connection executed (from
// any thread), the bounded checkpoint-head read-backs among them, and the
// encoded publication bytes. Counts are per settled turn after warm-up.
use super::*;
use crate::durable::sqlite::query_work::{self, ScopeWork};

struct FixedTool;

impl ToolExecutor for FixedTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: vec![ToolResultContent::Text(TextBlock {
                        text: "fixed tool result\n".to_owned(),
                    })],
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

fn fixed_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::new("tool-read"),
        name: "read".to_owned(),
        description: String::new(),
        input_schema: serde_json::json!({"type": "object"}).into(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

fn tool_call_step(turn: usize, definition: &ToolDefinition) -> Vec<GatedStep> {
    let id = ToolCallId::new(format!("read-{turn:08}"));
    let arguments = r#"{"path":"fixture.txt"}"#;
    vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: id.clone(),
            arguments_delta: arguments.to_owned(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCall {
                id,
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
                arguments: serde_json::from_str(arguments).unwrap(),
            },
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]
}

#[derive(Debug, Default)]
struct TurnWork {
    sql: std::collections::BTreeMap<&'static str, ScopeWork>,
    records: u64,
    encoded_bytes: usize,
    /// Delivered transitions, their read-domain facts, and their JSON bytes.
    transitions: usize,
    read_domain_facts: usize,
    delivered_bytes: usize,
}

fn count_delivered(work: &mut TurnWork, events: &[RuntimeClientProtocolEvent]) {
    for event in events {
        work.transitions += 1;
        work.delivered_bytes += serde_json::to_vec(event.event.as_ref()).unwrap().len();
        work.read_domain_facts += event
            .event
            .facts()
            .iter()
            .filter(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
            .count();
    }
}

const WARM_TURNS: usize = 3;
const MEASURED_TURNS: usize = 8;

async fn measure_turns(tool: bool) -> TurnWork {
    let turns = WARM_TURNS + MEASURED_TURNS;
    let definition = fixed_tool();
    let mut registry = ToolRegistry::new();
    if tool {
        registry
            .register(definition.clone(), Arc::new(FixedTool))
            .unwrap();
    }
    let scripts = (0..turns)
        .flat_map(|turn| {
            if tool {
                vec![tool_call_step(turn, &definition), one_turn_stop()]
            } else {
                vec![one_turn_stop()]
            }
        })
        .collect();
    let (adapter, fixture) = host_fixture(scripts, registry, status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let profile = Arc::new(Mutex::new(
        crate::runtime_client::projection::PublicationEncodingProfile::default(),
    ));
    inner
        .state
        .lock()
        .unwrap()
        .projection
        .profile_encoding(Arc::clone(&profile));
    let store = fixture.store.clone().unwrap();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = attachment
        .subscribe_events(fixture.host.checkpoint().unwrap().cursor)
        .unwrap();
    let mut work = TurnWork::default();
    for turn in 0..turns {
        if turn == WARM_TURNS {
            fixture.host.projection_settled().await;
            while let EventDelivery::Event(_) = subscription.try_next() {}
            store.begin_shared_query_work();
            *profile.lock().unwrap() =
                crate::runtime_client::projection::PublicationEncodingProfile::default();
            work = TurnWork::default();
        }
        fixture
            .host
            .submit_inbound(submit_content(&format!("turn {turn}")))
            .unwrap();
        let mut events = receive_until(&subscription, |event| {
            matches!(
                event.event.as_ref(),
                RuntimeClientEvent::AttemptSettled { .. }
            )
        })
        .await;
        fixture.host.projection_settled().await;
        while let EventDelivery::Event(event) = subscription.try_next() {
            events.push(event);
        }
        count_delivered(&mut work, &events);
    }
    for (scope, measured) in query_work::take_shared() {
        work.sql.insert(scope, measured);
    }
    store.finish_shared_query_work();
    let encoded = profile.lock().unwrap().clone();
    work.records = encoded.records;
    work.encoded_bytes = encoded.bytes;
    assert_eq!(adapter.requests().len(), turns * if tool { 2 } else { 1 });
    fixture.runtime.shutdown().await.unwrap();
    work
}

#[allow(
    clippy::cast_precision_loss,
    reason = "per-turn averages of small test counts"
)]
fn report(kind: &str, work: &TurnWork) {
    let per_turn = |value: f64| value / MEASURED_TURNS as f64;
    for (scope, sql) in &work.sql {
        eprintln!(
            "{kind} turn work scope={scope} calls/turn={:.2} statements/turn={:.1} steps/turn={:.1} rows/turn={:.1} transactions/turn={:.2}",
            per_turn(sql.entries as f64),
            per_turn(sql.statements as f64),
            per_turn(sql.steps as f64),
            per_turn(sql.rows as f64),
            per_turn(sql.transactions as f64),
        );
        if std::env::var_os("RUSTX_TURN_WORK_SQL").is_some() {
            let mut statements: Vec<_> = sql.by_sql.iter().collect();
            statements.sort_by_key(|(_, work)| std::cmp::Reverse(work.1));
            for (text, (calls, steps, rows)) in statements.into_iter().take(60) {
                eprintln!(
                    "  {kind}/{scope} calls/turn={:.2} steps/turn={:.1} rows/turn={:.1} {}",
                    per_turn(*calls as f64),
                    per_turn(*steps as f64),
                    per_turn(*rows as f64),
                    text.chars().take(220).collect::<String>()
                );
            }
        }
    }
    eprintln!(
        "{kind} turn work publications/turn={:.2} encoded_bytes/turn={:.1} delivered/turn={:.2} read_domain_facts/turn={:.2} delivered_json_bytes/turn={:.1}",
        per_turn(work.records as f64),
        per_turn(work.encoded_bytes as f64),
        per_turn(work.transitions as f64),
        per_turn(work.read_domain_facts as f64),
        per_turn(work.delivered_bytes as f64),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_turn_observation_work() {
    for (kind, tool) in [("text", false), ("tool", true)] {
        let work = measure_turns(tool).await;
        report(kind, &work);
    }
}
