//! Registered foreground Workflow boundary and shipped-program conformance.
//!
//! Foundational local fixtures prove explicit registration and the canonical
//! outer Tool boundary. The #223 reference scenarios compose fixed Parallel,
//! real candidate writes and frozen verification, bounded repair, and root HITL
//! through strict provider-emulator sequences and native execution.
//! Private child transcripts remain outside the single parent `ToolResult`.

use crate::launch_fixture::LaunchFixture;
use rustx::durable::{CanonicalStore, JournalStore, ObservationStore};
use std::sync::Arc;

use crate::common::provider_emulator::ProviderEmulator;
use rustx::durable::SqliteConversationStore;
use rustx::local_runtime::composition::{
    LocalConversationCore, LocalConversationRuntime, LocalRuntimeDependencies,
};
use rustx::message::content::TextBlock;
use rustx::message::types::UserContentBlock;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::workflow::WorkflowId;
use rustx::runtime::workflow::read_model::{WorkflowNodeKind, WorkflowState};
use rustx::runtime_client::attachment::RuntimeAttachment;
use rustx::runtime_client::host::{EventDelivery, EventSubscription};
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent, RuntimeClientOutcome,
};

const MODEL: &str = "workflow-model";
const TOOL_WORKFLOW: &str = r"description: Inspect discovered workflow files.
timeout_ms: 10000
block:
  input:
    type: object
    properties: {task: {type: string}}
    required: [task]
    additionalProperties: false
  output:
    type: object
    properties: {files: {type: string}}
    required: [files]
    additionalProperties: false
  entry: inspect
  nodes:
    inspect:
      type: tool
      selector: {origin: builtin, name: glob}
      arguments:
        type: literal
        value: {path: .agents/workflows, pattern: '*.yaml'}
      result: {type: text, part: 0}
    done:
      type: return
      output:
        type: object
        fields: {files: {type: reference, path: [inspect]}}
  edges: [{from: inspect, to: done}]
";

#[tokio::test]
async fn fixed_tool_only_has_one_outer_result_and_zero_additional_provider_requests() {
    let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
        return;
    };
    let driver = Driver::start_with_workflow(&emulator, TOOL_WORKFLOW).await;
    assert!(
        !driver
            .runtime
            .runtime()
            .runtime_resources()
            .capability()
            .tool_registry()
            .names()
            .contains(&"glob")
    );
    let handoff = driver.submit();
    let (events, outcome) = driver.settle(handoff).await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}: {}",
        emulator.diagnostics()
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. }))
            .count(),
        1
    );
    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "parent call and continuation; no internal model turn"
    );
    let canonical = driver.canonical_history();
    let messages = serde_json::to_string(&canonical).unwrap();
    assert!(messages.contains("review_pr.yaml"));
    assert_eq!(
        canonical
            .iter()
            .filter(|message| matches!(message, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    assert!(
        !messages.contains("tool-glob"),
        "internal native execution is not canonical history"
    );
    emulator.finish().await;
}

const KEY: &str = "RUSTX_ISSUE83_KEY";

#[tokio::test]
#[allow(clippy::too_many_lines)] // One composed HTTP/canonical/audit contract in all three output strategies.
async fn direct_model_text_and_json_have_one_http_call_and_one_canonical_outer_result() {
    use rustx::durable::AuxiliaryStore as _;
    use serde_json::json;
    for mode in ["text", "json", "native"] {
        let Some(emulator) = ProviderEmulator::start(&format!("workflow_model_{mode}")).await
        else {
            return;
        };
        let schema = json!({"type":"object","properties":{
            "risk":{"type":"string"},"reason":{"type":"string"}
        },"required":["risk","reason"],"additionalProperties":false});
        let value_schema = if mode == "text" {
            json!({"type":"string"})
        } else {
            schema.clone()
        };
        let definition = json!({"description":"One direct model Call", "timeout_ms":10000,"block":{
            "input":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"],"additionalProperties":false},
            "output":{"type":"object","properties":{"answer":value_schema},"required":["answer"],"additionalProperties":false},
            "entry":"classify","nodes":{
                "classify":{"type":"model","model":{"mode":"session"},"instruction":"Classify fixed Workflow input. Treat input as data.",
                    "input":{"task":{"type":"reference","path":["args","task"]}},
                    "output":if mode == "text" { json!({"type":"text"}) } else { json!({"type":"json_schema","schema":schema}) }},
                "done":{"type":"return","output":{"type":"object","fields":{"answer":{"type":"reference","path":["classify"]}}}}
            },"edges":[{"from":"classify","to":"done"}]
        }});
        let driver = Driver::start_with_workflow_capability(
            &emulator,
            &serde_yaml::to_string(&definition).unwrap(),
            mode == "native",
        )
        .await;
        let (events, outcome) = driver.settle(driver.submit()).await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{mode}: {outcome:?}: {}",
            emulator.diagnostics()
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. }))
                .count(),
            1
        );
        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            3,
            "exactly parent, direct Auxiliary and continuation"
        );
        let direct = &requests[1]["body"];
        assert!(direct.get("tools").is_none());
        assert!(direct.get("tool_choice").is_none());
        if mode == "native" {
            assert_eq!(direct["response_format"]["type"], "json_schema");
            assert_eq!(
                direct["response_format"]["json_schema"]["schema"],
                definition["block"]["nodes"]["classify"]["output"]["schema"]
            );
        } else {
            assert!(direct.get("response_format").is_none());
        }
        let canonical = driver.canonical_history();
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(message, rustx::message::types::MessageBlock::Tool(_)))
                .count(),
            1
        );
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    rustx::message::types::MessageBlock::Assistant(_)
                ))
                .count(),
            2
        );
        assert!(
            !serde_json::to_string(&canonical)
                .unwrap()
                .contains("Classify fixed Workflow input")
        );
        let runs = driver.workflow_runs().await;
        assert_eq!(runs[0].model_calls_consumed, 1);
        assert_eq!(runs[0].agents_consumed, 0);
        let call = runs[0]
            .instances
            .iter()
            .find_map(|node| node.auxiliary_call_id.as_ref())
            .unwrap();
        let store = SqliteConversationStore::open_existing(
            driver.runtime.runtime().conversation_id().clone(),
            &driver
                .runtime
                .tool_runtime()
                .artifacts()
                .root()
                .join("conversation.sqlite"),
        )
        .unwrap();
        let cut = store.auxiliary_audit_cut().unwrap();
        assert_eq!(
            store
                .read_auxiliary_call_requests(call, cut, None, 32)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .load_auxiliary_call(call, cut)
                .unwrap()
                .unwrap()
                .settled_sequence
                .is_some()
        );
        emulator.finish().await;
    }
}

#[tokio::test]
async fn bounded_loop_exhaustion_keeps_one_outer_result_and_no_internal_provider_turns() {
    use serde_json::json;
    let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
        return;
    };
    let mut definition: serde_json::Value = serde_yaml::from_str(TOOL_WORKFLOW).unwrap();
    let body = definition["block"].take();
    let output = json!({"type":"object","properties":{
        "status":{"type":"string","enum":["satisfied","exhausted"]},"iterations":{"type":"integer"},"result":body["output"]
    },"required":["status","iterations","result"],"additionalProperties":false});
    definition["block"] = json!({"input":body["input"],"output":output,"entry":"feedback","nodes":{
        "feedback":{"type":"loop","input":{"type":"reference","path":["args"]},"body":body,"max_iterations":3,
            "until":{"type":"boolean","value":{"type":"literal","value":false}},"carry":{"type":"literal","value":{"task":"inspect again"}}},
        "done":{"type":"return","output":{"type":"reference","path":["feedback"]}}
    },"edges":[{"from":"feedback","port":"satisfied","to":"done"},{"from":"feedback","port":"exhausted","to":"done"}]});
    let driver =
        Driver::start_with_workflow(&emulator, &serde_yaml::to_string(&definition).unwrap()).await;
    let handoff = driver.submit();
    let (events, outcome) = driver.settle(handoff).await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}: {}",
        emulator.diagnostics()
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RuntimeClientEvent::ToolExecutionSettled { .. }))
            .count(),
        1
    );
    assert_eq!(emulator.requests().await.len(), 2);
    let canonical = driver.canonical_history();
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    let history = serde_json::to_string(&canonical).unwrap();
    assert!(history.contains("exhausted"));
    assert!(history.contains("review_pr.yaml"));
    assert!(!history.contains("tool-glob"));
    emulator.finish().await;
}

fn models_json_for_base_url(base_url: &str) -> String {
    toml::to_string_pretty(&serde_json::json!({
        "providers": { "emulator": {"base_url": base_url, "api_key": "issue83-secret"} },
        "models": {format!("emulator/{MODEL}"): {
            "provider": "emulator", "id": MODEL, "protocol": "openai_chat_completions",
            "context_window": 128_000, "max_output_tokens": 1024,
            "capabilities": {"input_modalities": ["text"], "output_modalities": ["text"], "tool_calls": true, "reasoning": false},
            "compat": {"chat_reasoning_replay": "omit"}
        }}
    })).unwrap()
}

fn models_json(emulator: &ProviderEmulator) -> String {
    models_json_for_base_url(&emulator.openai_base_url())
}

const CONFIG: &str = r#"schema_version = 10
agent_id = "agent-issue83"

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[subagents]
max_concurrent = 4


[agent]
agents = []
workflows = ["review_pr"]

[agent.model]
model = "emulator/workflow-model"


[agent.tools]
builtin = ["read"]
"#;

const WORKFLOW: &str = r"description: Review the request with a native child agent.
block:
  input:
    type: object
    properties:
      task:
        type: string
    required:
    - task
    additionalProperties: false
  output:
    type: object
    properties:
      summary:
        type: string
    required:
    - summary
    additionalProperties: false
  entry: review
  nodes:
    review:
      type: agent
      profile: reviewer
      task: Review the request and commit the result.
      input:
        task:
          type: reference
          path:
          - args
          - task
      output:
        type: object
        properties:
          passed:
            type: boolean
          summary:
            type: string
        required:
        - passed
        - summary
        additionalProperties: false
    decision:
      type: branch
      condition:
        type: boolean
        value:
          type: reference
          path:
          - review
          - passed
    success:
      type: return
      output:
        type: object
        fields:
          summary:
            type: reference
            path:
            - review
            - summary
    failure:
      type: return
      output:
        type: object
        fields:
          summary:
            type: reference
            path:
            - args
            - task
  edges:
  - from: review
    to: decision
  - from: decision
    to: success
    port: true
  - from: decision
    to: failure
    port: false
";

struct Driver {
    #[allow(dead_code)]
    root: tempfile::TempDir,
    runtime: LocalConversationRuntime,
    #[allow(dead_code)]
    attachment: RuntimeAttachment,
    events: EventSubscription,
}

impl Driver {
    /// Failure-only bounded evidence from the genuine provider/native owners.
    async fn failure_diagnostics(&self, emulator: &ProviderEmulator, stage: &str) {
        let requests = emulator.requests().await;
        eprintln!(
            "Workflow failure stage={stage} provider_requests={}",
            requests.len()
        );
        provider_request_diagnostics(&requests);
        provider_tool_diagnostics(&requests);
        self.child_failure_diagnostics().await;
        let workflows = self.runtime.tool_runtime().workflows().snapshot();
        eprintln!(
            "native Workflow revision={} runs={} omitted={}",
            workflows.revision.0,
            workflows.runs.len(),
            workflows.omitted_runs
        );
        for run in workflows.runs.iter().take(8) {
            eprintln!(
                "native run={} state={:?} agents={} steps={}",
                diagnostic_text(run.workflow_id.as_str(), 120),
                run.state,
                run.agents_consumed,
                run.steps_consumed
            );
            for instance in run.instances.iter().take(32) {
                eprintln!(
                    "native node={} kind={:?} state={:?} has_child={} has_invocation={}",
                    diagnostic_text(instance.node.as_deref().unwrap_or("<block>"), 120),
                    instance.kind,
                    instance.state,
                    instance.child.is_some(),
                    instance.invocation.is_some()
                );
            }
        }
    }

    /// Failure-only original Workflow owner evidence. Workflow activations are
    /// deliberately absent from the continuable Agent directory.
    async fn child_failure_diagnostics(&self) {
        let checkpoint = self
            .runtime
            .host()
            .checkpoint()
            .expect("original Workflow cut");
        let children = self.workflow_children_at(&checkpoint).await;
        let conversations = self
            .runtime
            .tool_runtime()
            .artifacts()
            .root()
            .parent()
            .expect("native conversations directory")
            .to_path_buf();
        let mut budget = 16 * 1024;
        for child in &children {
            let store = SqliteConversationStore::open_existing(
                child.conversation.clone(),
                &conversations
                    .join(child.conversation.as_str())
                    .join("conversation.sqlite"),
            )
            .expect("original Workflow-owned Child diagnostic store");
            // new() captures one authentic native Child Source cut; every field
            // below uses that same page cut, never a later capture or fake A.
            let page = rustx::runtime_client::trace::TraceProjection::new(&store)
                .and_then(|projection| projection.page(None, 32))
                .expect("original bounded Workflow Child Trace");
            let cut = page.read_cut.as_ref().expect("original Child history cut");
            assert_eq!(cut.conversation_id, child.conversation);
            assert!(cut.activation_id.is_none());
            let source = cut
                .source_cut
                .as_ref()
                .expect("original Child Source tuple");
            assert_eq!(source.read.conversation_id, child.conversation);
            assert_eq!(source.read.journal, cut.through);
            for record in &page.records {
                if let Some(tool) = &record.tool {
                    print_tool_diagnostic(
                        &format!(
                            "Workflow Child Tool A={} record={} name={:?} outcome={:?} detail={:?}",
                            diagnostic_text(child.activation.as_str(), 160),
                            record.id,
                            tool.name,
                            tool.outcome,
                            tool.detail
                        ),
                        &mut budget,
                    );
                }
                child_failure_fields(&store, child, cut, record, &mut budget);
            }
        }
    }

    async fn workflow_children_at(
        &self,
        checkpoint: &rustx::runtime_client::checkpoint::RuntimeClientCheckpoint,
    ) -> Vec<WorkflowChildEvidence> {
        use rustx::durable::presentation::{FactQuery, FactScope};
        let runs = self.workflow_runs_at(checkpoint).await;
        let activations = runs
            .iter()
            .flat_map(|run| &run.instances)
            .filter_map(|instance| instance.child.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let store = SqliteConversationStore::open_existing(
            self.runtime.runtime().conversation_id().clone(),
            &self
                .runtime
                .tool_runtime()
                .artifacts()
                .root()
                .join("conversation.sqlite"),
        )
        .expect("original Root ownership Journal");
        let mut children = Vec::new();
        for activation in activations.iter().take(32) {
            let ownership = store
                .read_presentation_events(&FactQuery {
                    scope: FactScope::Subagent(activation.as_str().into()),
                    kinds: vec!["subagent_ownership_committed"],
                    before: None,
                    after: 0,
                    ascending: true,
                    through: checkpoint.read_cut.through,
                    limit: 2,
                })
                .expect("indexed original Workflow Child ownership");
            assert_eq!(ownership.len(), 1, "one exact original Child ownership");
            let original = ownership.into_iter().next().unwrap();
            assert_eq!(
                original.conversation_id,
                checkpoint.read_cut.conversation_id
            );
            let rustx::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                child_conversation_id,
                ownership,
                ..
            } = original.event
            else {
                unreachable!()
            };
            assert_eq!(&subagent_id, activation);
            assert_eq!(
                ownership,
                rustx::events::types::SubagentOwnershipKind::Workflow
            );
            eprintln!(
                "original Workflow Child RootJ={} A={} CID={} at_root_through={}",
                original.sequence, subagent_id, child_conversation_id, checkpoint.read_cut.through
            );
            children.push(WorkflowChildEvidence {
                agent: child_agent_id,
                activation: subagent_id,
                conversation: child_conversation_id,
            });
        }
        eprintln!(
            "original Workflow Child total={} selected={}",
            activations.len(),
            children.len()
        );
        children
    }

    async fn workflow_runs(&self) -> Vec<rustx::runtime::workflow::read_model::WorkflowRunView> {
        let checkpoint = self
            .runtime
            .host()
            .checkpoint()
            .expect("native Workflow cut");
        self.workflow_runs_at(&checkpoint).await
    }

    async fn workflow_runs_at(
        &self,
        checkpoint: &rustx::runtime_client::checkpoint::RuntimeClientCheckpoint,
    ) -> Vec<rustx::runtime::workflow::read_model::WorkflowRunView> {
        assert_eq!(
            &checkpoint.read_cut.conversation_id,
            self.runtime.runtime().conversation_id()
        );
        assert_eq!(checkpoint.read_cut.revision, checkpoint.cursor.get());
        let mut offset = 0;
        let mut total = None;
        let mut runs = Vec::new();
        loop {
            let reply = self
                .attachment
                .handle_request_async(
                    rustx::runtime_client::RuntimeClientRequest::CheckpointPageGet {
                        id: rustx::runtime_client::RequestId::new(467),
                        cut: checkpoint.read_cut.clone(),
                        domain: "workflows.runs".into(),
                        offset,
                        limit: 32,
                    },
                )
                .await;
            let Some(RuntimeClientResult::CheckpointPage { page }) = reply.result else {
                panic!("original Workflow page: {reply:?}");
            };
            assert_eq!(page.read_cut, checkpoint.read_cut);
            assert_eq!(page.cursor, checkpoint.cursor);
            assert_eq!(page.domain, "workflows.runs");
            assert_eq!(*total.get_or_insert(page.total), page.total);
            assert!(page.items.len() <= 32);
            assert!(
                page.total
                    <= u64::try_from(rustx::runtime::workflow::read_model::MAX_PROJECTED_RUNS)
                        .unwrap()
            );
            let before = offset;
            for item in page.items {
                assert_eq!(item.offset, offset);
                runs.push(
                    self.workflow_run_at(&checkpoint.read_cut, item.offset)
                        .await,
                );
                offset += 1;
            }
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(runs.len()).unwrap(), page.total);
                return runs;
            }
            assert!(offset > before, "the original Workflow page advances");
        }
    }

    fn assert_inflight_workflow_journal(
        &self,
        checkpoint: &rustx::runtime_client::checkpoint::RuntimeClientCheckpoint,
        run: &rustx::runtime::workflow::read_model::WorkflowRunView,
        check: &rustx::runtime::workflow::read_model::WorkflowInstanceView,
    ) {
        use rustx::durable::presentation::{FactQuery, FactScope};
        use rustx::events::types::RuntimeEvent;
        let source_cut = checkpoint.read_cut.source_cut.as_ref().unwrap();
        assert_eq!(source_cut.read.journal, checkpoint.read_cut.through);
        let store = SqliteConversationStore::open_existing(
            checkpoint.read_cut.conversation_id.clone(),
            &self
                .runtime
                .tool_runtime()
                .artifacts()
                .root()
                .join("conversation.sqlite"),
        )
        .expect("original held Workflow Journal");
        let facts = store
            .read_presentation_events(&FactQuery {
                scope: FactScope::All,
                kinds: vec![
                    "workflow_agent_admitted",
                    "workflow_node_settled",
                    "workflow_completed",
                    "workflow_failed",
                    "workflow_cancelled",
                ],
                before: None,
                after: 0,
                ascending: true,
                through: checkpoint.read_cut.through,
                limit: 128,
            })
            .expect("bounded original Workflow facts at the issued checkpoint");
        assert!(
            facts.len() < 128,
            "the fixture's complete selected fact set fits"
        );
        assert!(facts.iter().all(|fact| fact.conversation_id
            == checkpoint.read_cut.conversation_id
            && fact.sequence <= checkpoint.read_cut.through));
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(&fact.event,
                    RuntimeEvent::WorkflowAgentAdmitted {run_id, node_id, ..}
                    if run_id == &run.id && node_id.node == "implement"
                ))
                .count(),
            2
        );
        let checks = facts
            .iter()
            .filter_map(|fact| match &fact.event {
                RuntimeEvent::WorkflowNodeSettled { instance, outcome }
                    if instance.block.run == run.id && instance.node == "check" =>
                {
                    Some((instance, outcome))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].0.block, check.block);
        assert_eq!(Some(checks[0].0.visit), check.visit);
        assert_eq!(
            *checks[0].1,
            rustx::runtime::workflow::WorkflowExecutionOutcome::Completed
        );
        assert!(!facts.iter().any(|fact| matches!(&fact.event,
            RuntimeEvent::WorkflowCompleted {run_id, ..}
            | RuntimeEvent::WorkflowFailed {run_id, ..}
            | RuntimeEvent::WorkflowCancelled {run_id, ..} if run_id == &run.id
        )));
    }

    async fn workflow_run_at(
        &self,
        cut: &rustx::observation::cut::CheckpointReadCut,
        item_offset: u64,
    ) -> rustx::runtime::workflow::read_model::WorkflowRunView {
        use base64::Engine as _;
        let mut bytes = Vec::new();
        let mut offset = 0;
        let mut frontier = None;
        loop {
            let reply = self
                .attachment
                .handle_request_async(
                    rustx::runtime_client::RuntimeClientRequest::CheckpointValueRead {
                        id: rustx::runtime_client::RequestId::new(467),
                        cut: cut.clone(),
                        domain: "workflows.runs".into(),
                        item_offset,
                        path: Vec::new(),
                        offset,
                        max_bytes: 65_536,
                    },
                )
                .await;
            let Some(RuntimeClientResult::CheckpointValueChunk { chunk }) = reply.result else {
                panic!("original selected Workflow run: {reply:?}");
            };
            assert_eq!(&chunk.cut, cut);
            assert_eq!(
                chunk.representation,
                rustx::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
            );
            assert_eq!(chunk.domain, "workflows.runs");
            assert_eq!(chunk.item_offset, item_offset);
            assert!(chunk.path.is_empty());
            assert_eq!(chunk.offset, offset);
            assert_eq!(
                *frontier.get_or_insert(chunk.readable_end),
                chunk.readable_end
            );
            assert!(chunk.readable_end <= 16 * 1024 * 1024);
            assert!(chunk.bytes_base64.len() <= 87_384);
            let part = base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap();
            assert!(!part.is_empty() && part.len() <= 65_536);
            bytes.extend(&part);
            offset += u64::try_from(part.len()).unwrap();
            assert!(offset <= chunk.readable_end);
            if let Some(next) = chunk.next_offset {
                assert_eq!(next, offset);
                assert!(next < chunk.readable_end);
            } else {
                assert_eq!(offset, chunk.readable_end);
                return rustx::observation::bounded::decode_domain_value(
                    serde_json::from_slice(&bytes).unwrap(),
                )
                .unwrap();
            }
        }
    }

    /// Original canonical oracle; the checkpoint remains a bounded body cache.
    fn canonical_history(&self) -> Vec<rustx::message::types::MessageBlock> {
        let store = SqliteConversationStore::open_existing(
            self.runtime.runtime().conversation_id().clone(),
            &self
                .runtime
                .tool_runtime()
                .artifacts()
                .root()
                .join("conversation.sqlite"),
        )
        .expect("original native history read handle");
        let cut = store
            .observation_read_cut()
            .expect("captured native history");
        assert_eq!(
            &cut.read.conversation_id,
            self.runtime.runtime().conversation_id()
        );
        let mut offset = 0;
        let mut messages = Vec::new();
        loop {
            let page = store
                .message_header_page(&cut, Some(offset), 32)
                .expect("bounded original header page");
            assert_eq!(page.cut, cut);
            assert_eq!(page.offset, offset);
            assert_eq!(page.total, cut.ledger_through);
            assert!(page.headers.len() <= 32);
            let ids: Vec<_> = page
                .headers
                .iter()
                .enumerate()
                .map(|(index, header)| {
                    assert_eq!(
                        header.ledger_position,
                        Some(offset + u64::try_from(index).unwrap() + 1)
                    );
                    header
                        .identity
                        .clone()
                        .expect("exact finite original fixture ID")
                })
                .collect();
            let originals = store
                .load_messages(&ids)
                .expect("selected original messages");
            assert_eq!(originals.len(), ids.len());
            for (message, id) in originals.iter().zip(&ids) {
                assert_eq!(message.id(), id);
            }
            messages.extend(originals);
            offset += u64::try_from(page.headers.len()).unwrap();
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(messages.len()).unwrap(), cut.ledger_through);
                return messages;
            }
            assert!(!page.headers.is_empty(), "the native header walk advances");
        }
    }

    async fn interaction_body(
        &self,
        header: &rustx::observation::interaction::RuntimeClientInteractionHeader,
    ) -> rustx::runtime::InteractionRequest {
        use base64::Engine as _;
        let mut bytes = Vec::new();
        let mut offset = 0;
        let mut frontier = None;
        loop {
            let reply = self
                .attachment
                .handle_request_async(
                    rustx::runtime_client::RuntimeClientRequest::InteractionValueRead {
                        id: rustx::runtime_client::RequestId::new(467),
                        interaction: header.interaction.clone(),
                        path: Vec::new(),
                        offset,
                        max_bytes: 65_536,
                    },
                )
                .await;
            let Some(RuntimeClientResult::InteractionValueChunk {
                interaction,
                path,
                offset: actual,
                bytes_base64,
                readable_end,
                next_offset,
            }) = reply.result
            else {
                panic!("original routed interaction read: {reply:?}");
            };
            assert_eq!(interaction, header.interaction);
            assert!(path.is_empty());
            assert_eq!(actual, offset);
            assert_eq!(*frontier.get_or_insert(readable_end), readable_end);
            assert!(readable_end <= 16 * 1024 * 1024);
            assert!(bytes_base64.len() <= 87_384);
            let part = base64::engine::general_purpose::STANDARD
                .decode(bytes_base64)
                .unwrap();
            assert!(!part.is_empty() && part.len() <= 65_536);
            offset += u64::try_from(part.len()).unwrap();
            assert!(offset <= readable_end);
            bytes.extend(part);
            if let Some(next) = next_offset {
                assert_eq!(next, offset);
                assert!(next < readable_end);
            } else {
                assert_eq!(offset, readable_end);
                break;
            }
        }
        let request: rustx::runtime::InteractionRequest =
            rustx::observation::bounded::decode_domain_value(
                serde_json::from_slice(&bytes).unwrap(),
            )
            .unwrap();
        assert_eq!(request.interaction_ref(), header.interaction);
        assert_eq!(request.id, header.request_id);
        assert_eq!(request.conversation_id, header.conversation_id);
        assert_eq!(request.execution, header.execution);
        let kind = match &request.kind {
            rustx::runtime::InteractionKind::Questionnaire { .. } => {
                rustx::observation::interaction::RuntimeClientInteractionKind::Questionnaire
            }
            rustx::runtime::InteractionKind::Review { .. } => {
                rustx::observation::interaction::RuntimeClientInteractionKind::Review
            }
            rustx::runtime::InteractionKind::Approval { .. } => {
                rustx::observation::interaction::RuntimeClientInteractionKind::Approval
            }
        };
        assert_eq!(kind, header.kind);
        request
    }

    async fn start_with_workflow(emulator: &ProviderEmulator, workflow: &str) -> Self {
        Self::start_with_workflow_capability(emulator, workflow, false).await
    }

    async fn start_with_workflow_capability(
        emulator: &ProviderEmulator,
        workflow: &str,
        native_schema: bool,
    ) -> Self {
        let root = tempfile::tempdir().expect("temp root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents/agents/reviewer"))
            .expect("subagent directory");
        std::fs::create_dir_all(workspace.join(".agents/workflows")).expect("workflow directory");
        std::fs::write(
            root.path().join("rustx.toml"),
            format!(
                "{CONFIG}\n{}",
                models_json(emulator).replace(
                    "reasoning = false",
                    &format!("reasoning = false\njson_schema_output = {native_schema}")
                )
            ),
        )
        .expect("rustx.toml");
        std::fs::write(
            workspace.join(".agents/agents/reviewer.toml"),
            "description = \"The Workflow-only reviewer.\"\ninstructions = \"Review requests carefully.\\n\"\n",
        )
        .expect("reviewer instructions");
        std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), workflow)
            .expect("workflow YAML");
        // Noncanonical incidental files have no Workflow identity.
        std::fs::write(
            workspace.join(".agents/workflows/inactive.txt"),
            "this is not a canonical Workflow definition: [",
        )
        .expect("inactive workflow YAML");

        let paths = LaunchFixture {
            config: root.path().join("rustx.toml"),
            startup_session: rustx::local_runtime::StartupSession::Empty,
            workspace,
            runtime_root: root.path().join("private"),
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                KEY.to_owned(),
                "issue83-secret".to_owned(),
            )]))),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies)
            .await
            .expect("native Workflow runtime composes");
        let resources = runtime.runtime().runtime_resources();
        assert!(
            resources
                .workflows()
                .enabled_ids()
                .iter()
                .any(|id| id.as_str() == "review_pr")
        );
        assert!(resources.delegatable_agents().is_empty());

        let tools = resources.capability().tool_registry().names();
        assert!(tools.contains(&"review_pr"));
        assert!(
            resources
                .workflows()
                .get(&WorkflowId::parse("inactive").expect("workflow id"))
                .is_none()
        );
        assert!(!tools.contains(&"inactive"));
        assert!(!tools.contains(&"subagent"));

        let (attachment, initialized) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("initialize returns the initial snapshot");
        };
        let cursor = runtime
            .host()
            .checkpoint()
            .expect("bounded checkpoint")
            .cursor;
        let (events, _) = runtime
            .host()
            .subscribe_events(attachment.attachment_id(), cursor)
            .expect("subscribe");
        Self {
            root,
            runtime,
            attachment,
            events,
        }
    }

    fn submit(&self) -> std::pin::Pin<Box<tokio::sync::futures::Notified<'_>>> {
        // Enable the existing native handoff waiter before admission. A terminal
        // display event can arrive after notify_waiters has already fired.
        let mut handoff = Box::pin(self.runtime.runtime().settlement_signal().notified());
        handoff.as_mut().enable();
        self.runtime
            .host()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: "workflow conformance request".to_owned(),
            })])
            .expect("inbound accepted");
        handoff
    }

    async fn settle(
        &self,
        handoff: std::pin::Pin<Box<tokio::sync::futures::Notified<'_>>>,
    ) -> (Vec<RuntimeClientEvent>, RuntimeClientOutcome) {
        let mut events = Vec::new();
        // Human-interaction scenarios already consumed the admission/start
        // records. Capture their genuine current owner before following the
        // remaining terminal stream; this driver submits exactly one attempt.
        let checkpoint = self
            .runtime
            .host()
            .checkpoint()
            .expect("captured owner cut");
        assert_eq!(
            &checkpoint.read_cut.conversation_id,
            self.runtime.runtime().conversation_id()
        );
        assert_eq!(checkpoint.read_cut.revision, checkpoint.cursor.get());
        if let Some(source) = &checkpoint.read_cut.source_cut {
            assert_eq!(
                source.read.conversation_id,
                checkpoint.read_cut.conversation_id
            );
        }
        let mut original = checkpoint.snapshot.attempt.as_ref().map(|attempt| {
            attempt
                .attempt_id
                .as_ref()
                .expect("original finite attempt")
                .clone()
        });
        loop {
            let delivery =
                tokio::time::timeout(std::time::Duration::from_secs(30), self.events.next())
                    .await
                    .expect("Workflow attempt settles");
            match delivery {
                EventDelivery::Event(published) => {
                    let outcome = workflow_terminal(
                        &published,
                        &mut original,
                        self.runtime.runtime().conversation_id(),
                    );
                    events.extend(published.event.facts().iter().cloned());
                    if let Some(outcome) = outcome {
                        // The client terminal observation precedes transfer
                        // back to the runtime's idle slot. Await the native
                        // handoff notification before attempting reload.
                        tokio::time::timeout(std::time::Duration::from_secs(30), handoff)
                            .await
                            .expect("Workflow native handoff completes");
                        return (events, outcome);
                    }
                }
                other => panic!("Runtime Client event stream ended: {other:?}"),
            }
        }
    }
}

fn workflow_interactions(
    published: &rustx::runtime_client::RuntimeClientProtocolEvent,
) -> Vec<rustx::observation::interaction::RuntimeClientInteractionHeader> {
    let mut pending = Vec::new();
    for fact in published.event.facts() {
        let headers: &[rustx::observation::interaction::RuntimeClientInteractionHeader] = match fact
        {
            RuntimeClientEvent::InteractionPending { interaction } => {
                std::slice::from_ref(interaction)
            }
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                &checkpoint.snapshot.pending_interactions
            }
            _ => &[],
        };
        for header in headers {
            if !pending.iter().any(
                |old: &rustx::observation::interaction::RuntimeClientInteractionHeader| {
                    old.interaction == header.interaction
                },
            ) {
                pending.push(header.clone());
            }
        }
    }
    pending
}

fn workflow_terminal(
    published: &rustx::runtime_client::RuntimeClientProtocolEvent,
    original: &mut Option<rustx::runtime::identity::AttemptId>,
    conversation: &rustx::runtime::identity::ConversationId,
) -> Option<RuntimeClientOutcome> {
    let mut outcome = None;
    for fact in published.event.facts() {
        match fact {
            RuntimeClientEvent::AttemptStarted { attempt_id, .. } => {
                remember_attempt(original, attempt_id);
            }
            RuntimeClientEvent::AttemptSettled {
                attempt_id,
                outcome: terminal,
            } => {
                assert_eq!(original.as_ref(), Some(attempt_id));
                outcome = Some(terminal.clone());
            }
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                assert_eq!(checkpoint.cursor, published.cursor);
                assert_eq!(checkpoint.read_cut.revision, published.cursor.get());
                assert_eq!(&checkpoint.read_cut.conversation_id, conversation);
                assert_eq!(checkpoint.snapshot.conversation_id(), conversation);
                if let Some(source) = &checkpoint.read_cut.source_cut {
                    assert_eq!(&source.read.conversation_id, conversation);
                }
                if let Some(attempt) = &checkpoint.snapshot.attempt {
                    remember_attempt(
                        original,
                        attempt
                            .attempt_id
                            .as_ref()
                            .expect("original finite attempt"),
                    );
                    if let rustx::runtime_client::RuntimeClientAttemptPhase::Settled {
                        outcome: terminal,
                    } = &attempt.phase
                    {
                        outcome = Some(terminal.clone());
                    }
                }
            }
            _ => {}
        }
    }
    outcome
}

fn remember_attempt(
    original: &mut Option<rustx::runtime::identity::AttemptId>,
    observed: &rustx::runtime::identity::AttemptId,
) {
    if let Some(original) = original {
        assert_eq!(
            original, observed,
            "the Workflow keeps its original attempt"
        );
    } else {
        *original = Some(observed.clone());
    }
}

struct WorkflowChildEvidence {
    agent: rustx::runtime::identity::AgentId,
    activation: rustx::runtime::identity::SubagentId,
    conversation: rustx::runtime::identity::ConversationId,
}

fn child_failure_fields(
    store: &SqliteConversationStore,
    agent: &WorkflowChildEvidence,
    cut: &rustx::observation::cut::CheckpointReadCut,
    record: &rustx::observation::trace::TraceRecord,
    budget: &mut usize,
) {
    use rustx::observation::trace::{TraceFieldKind, TraceFieldSource, TraceKind};
    if !matches!(record.kind, TraceKind::Request | TraceKind::Attempt) {
        return;
    }
    for field in &record.fields {
        let TraceFieldSource::Value {
            field: TraceFieldKind::Failure,
            domain,
            item_offset,
            path,
            encoded_bytes,
        } = field
        else {
            continue;
        };
        let sequence: u64 = domain
            .strip_prefix("journal.")
            .expect("advertised original failure Journal domain")
            .parse()
            .expect("original failure Journal sequence");
        assert_eq!(*item_offset, 0);
        assert!(sequence > 0 && sequence <= cut.through);
        let kind = store
            .observation_event_kind(sequence, cut.through)
            .expect("original bounded failure kind")
            .expect("original failure exists");
        let normalized = path.as_slice() == ["normalized_failure"];
        let chunk = if normalized {
            assert_eq!(kind, "attempt_failed");
            store.observation_failure_read_at(
                cut.source_cut
                    .as_ref()
                    .expect("retained original failure cut"),
                sequence,
                0,
                65_536,
            )
        } else {
            assert_eq!(path.as_slice(), ["event", "error"]);
            assert!(matches!(
                kind.as_str(),
                "model_request_failed" | "compaction_failed"
            ));
            store.event_value_read(sequence, cut.through, path, 0, 65_536)
        }
        .expect("selected original failure bytes");
        assert!(chunk.bytes.len() <= 65_536);
        assert_eq!(chunk.readable_end, *encoded_bytes);
        assert_eq!(chunk.end_offset, u64::try_from(chunk.bytes.len()).unwrap());
        assert_eq!(
            chunk.next_offset,
            (chunk.end_offset < chunk.readable_end).then_some(chunk.end_offset)
        );
        let header = format!(
            "Child original failure agent={} CID={} activation={} record={} kind={kind} J={sequence} through={} encoded_bytes={encoded_bytes}",
            diagnostic_text(agent.agent.as_str(), 160),
            diagnostic_text(agent.conversation.as_str(), 160),
            diagnostic_text(agent.activation.as_str(), 160),
            diagnostic_text(&record.id, 160),
            cut.through,
        );
        if !print_tool_diagnostic(&header, budget) {
            return;
        }
        let text = String::from_utf8_lossy(&chunk.bytes);
        let prefix = diagnostic_text(&text, 4096);
        print_tool_diagnostic(&format!("Child original failure bytes={prefix}"), budget);
    }
}

fn provider_request_diagnostics(requests: &[serde_json::Value]) {
    // Keep every finite request's model visible even when an early prompt is large.
    let selected = &requests[..requests.len().min(32)];
    let mut budget = 16 * 1024;
    for request in selected {
        print_tool_diagnostic(
            &format!(
                "provider original request={} model={}",
                request["index"].as_u64().expect("original request index"),
                diagnostic_text(request["model"].as_str().unwrap_or("<absent>"), 120),
            ),
            &mut budget,
        );
    }
    let per_request = budget / selected.len().max(1);
    for request in selected {
        let mut row_budget = per_request;
        print_tool_diagnostic(
            &format!(
                "provider original request={} classifier evidence",
                request["index"].as_u64().expect("original request index")
            ),
            &mut row_budget,
        );
        let Some(messages) = request["body"]["messages"].as_array() else {
            continue;
        };
        // The classifier consumes the original task, not repeated historical Tool results.
        for role in ["user", "system"] {
            if let Some(message) = messages
                .iter()
                .rev()
                .find(|message| message["role"] == role)
            {
                if let Some(text) = message["content"].as_str() {
                    print_provider_text(role, text, 1000, &mut row_budget);
                } else if let Some(parts) = message["content"].as_array() {
                    for part in parts.iter().take(8) {
                        if let Some(text) = part["text"].as_str() {
                            print_provider_text(role, text, 1000, &mut row_budget);
                        }
                    }
                }
            }
        }
        for message in messages
            .iter()
            .filter(|message| message["role"] == "assistant")
        {
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls.iter().take(8) {
                    print_tool_diagnostic(
                        &format!(
                            "provider original assistant call={} name={}",
                            diagnostic_text(call["id"].as_str().unwrap_or("<absent>"), 160),
                            diagnostic_text(
                                call["function"]["name"].as_str().unwrap_or("<absent>"),
                                160
                            ),
                        ),
                        &mut row_budget,
                    );
                    provider_argument_diagnostics(&call["function"]["arguments"], &mut row_budget);
                }
            }
        }
    }
}

fn provider_argument_diagnostics(value: &serde_json::Value, budget: &mut usize) {
    if let Some(text) = value.as_str() {
        print_provider_text("arguments", text, 1000, budget);
    } else if let Some(fields) = value.as_object() {
        for (name, field) in fields.iter().take(8) {
            let label = format!("arguments.{}", diagnostic_text(name, 80));
            if let Some(text) = field.as_str() {
                print_provider_text(&label, text, 1000, budget);
            } else if let Some(parts) = field.as_array() {
                for part in parts.iter().take(8) {
                    if let Some(text) = part.as_str().or_else(|| part["text"].as_str()) {
                        print_provider_text(&label, text, 1000, budget);
                    } else if let Some(question) = part.as_object() {
                        for (key, text) in question
                            .iter()
                            .take(8)
                            .filter_map(|(key, value)| value.as_str().map(|text| (key, text)))
                        {
                            print_provider_text(
                                &format!("{label}.{}", diagnostic_text(key, 80)),
                                text,
                                1000,
                                budget,
                            );
                        }
                    }
                }
            } else if field.is_number() || field.is_boolean() || field.is_null() {
                print_tool_diagnostic(&format!("provider original {label}={field}"), budget);
            } else {
                print_tool_diagnostic(
                    &format!("provider original {label}=<structured object>"),
                    budget,
                );
            }
        }
    } else {
        print_tool_diagnostic(
            "provider original arguments=<non-string/non-object>",
            budget,
        );
    }
}

fn print_provider_text(label: &str, text: &str, max_chars: usize, budget: &mut usize) {
    let header = format!("provider original {label}=");
    // Leave room for the other genuine classifier/call fields of this same request.
    let mut remaining = budget.saturating_sub(header.len() + 1).min(320);
    let prefix: String = text
        .chars()
        .take(max_chars)
        .take_while(|character| {
            if character.len_utf8() > remaining {
                false
            } else {
                remaining -= character.len_utf8();
                true
            }
        })
        .collect();
    print_tool_diagnostic(&format!("{header}{prefix}"), budget);
}

fn provider_tool_diagnostics(requests: &[serde_json::Value]) {
    let mut first = Vec::with_capacity(16);
    let mut last = std::collections::VecDeque::with_capacity(16);
    let mut total = 0;
    for request in requests {
        if let Some(messages) = request["body"]["messages"].as_array() {
            for message in messages {
                if message["role"] != "tool" {
                    continue;
                }
                total += 1;
                if first.len() < 16 {
                    first.push((request, message));
                } else {
                    if last.len() == 16 {
                        last.pop_front();
                    }
                    last.push_back((request, message));
                }
            }
        }
    }
    eprintln!("provider Tool messages={total}; showing first/last at most32");
    let mut budget = 16 * 1024;
    for (request, message) in first.into_iter().chain(last) {
        let header = format!(
            "provider Tool result request_index={} model={} call={}",
            request["index"].as_u64().expect("recorded request index"),
            diagnostic_text(request["model"].as_str().unwrap_or("<absent>"), 120),
            diagnostic_text(message["tool_call_id"].as_str().unwrap_or("<absent>"), 120),
        );
        if !print_tool_diagnostic(&header, &mut budget) {
            break;
        }
        if let Some(text) = message["content"].as_str() {
            print_tool_text(text, &mut budget);
        } else if let Some(parts) = message["content"].as_array() {
            for part in parts.iter().take(8) {
                if let Some(text) = part["text"].as_str() {
                    print_tool_text(text, &mut budget);
                } else {
                    print_tool_diagnostic(
                        &format!(
                            "provider Tool part type={}",
                            diagnostic_text(part["type"].as_str().unwrap_or("<absent>"), 80)
                        ),
                        &mut budget,
                    );
                }
            }
        } else {
            print_tool_diagnostic(
                "provider Tool content has no original text/parts",
                &mut budget,
            );
        }
    }
}

fn print_tool_text(text: &str, budget: &mut usize) {
    let overhead = "provider Tool text=".len() + 1;
    if *budget <= overhead {
        return;
    }
    let mut remaining = (*budget - overhead).min(768);
    let prefix: String = text
        .chars()
        .take_while(|character| {
            if character.len_utf8() > remaining {
                false
            } else {
                remaining -= character.len_utf8();
                true
            }
        })
        .collect();
    print_tool_diagnostic(&format!("provider Tool text={prefix}"), budget);
}

fn print_tool_diagnostic(line: &str, budget: &mut usize) -> bool {
    let bytes = line.len() + 1;
    if bytes > *budget {
        return false;
    }
    *budget -= bytes;
    eprintln!("{line}");
    true
}

fn diagnostic_text(value: &str, chars: usize) -> String {
    value.chars().take(chars).collect()
}

fn record_writer_starts(
    workflows: &[rustx::runtime::workflow::read_model::WorkflowRunView],
    started: &tokio::sync::watch::Sender<usize>,
) {
    // The native owner installs child and Waiting{Agent} together. Running
    // may already be superseded when its event is read; inline run bodies may
    // be absent. The provider's own gate independently proves request arrival.
    let count = workflows
        .iter()
        .flat_map(|run| &run.instances)
        .filter(|row| {
            row.kind == WorkflowNodeKind::Agent
                && row.node.as_deref() == Some("implement")
                && row.child.is_some()
        })
        .count();
    started.send_modify(|old| *old = (*old).max(count));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_selection_rejects_an_identity_outside_the_canonical_root() {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/agents/reviewer")).expect("subagent directory");
    std::fs::create_dir_all(workspace.join(".rustx/workflows")).expect("obsolete directory");
    std::fs::write(
        root.path().join("rustx.toml"),
        format!(
            "{CONFIG}\n{}",
            models_json_for_base_url("http://127.0.0.1:1/v1")
        ),
    )
    .expect("rustx.toml");
    std::fs::write(
        workspace.join(".agents/agents/reviewer.toml"),
        "description = \"The Workflow-only reviewer.\"\ninstructions = \"Review requests carefully.\\n\"\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".rustx/workflows/review_pr.yaml"), WORKFLOW)
        .expect("legacy workflow YAML");

    let paths = LaunchFixture {
        config: root.path().join("rustx.toml"),
        startup_session: rustx::local_runtime::StartupSession::Empty,
        workspace,
        runtime_root: root.path().join("private"),
    };
    let error =
        LocalConversationCore::compose(&paths.resolve(), &LocalRuntimeDependencies::default())
            .await
            .expect_err("a selected missing Workflow fails admission");
    assert!(
        format!("{error:?}").contains("WorkflowUnavailable"),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovered_workflow_can_remain_out_of_main_model_admission() {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/agents/reviewer")).expect("subagent directory");
    std::fs::create_dir_all(workspace.join(".agents/workflows")).expect("workflow directory");
    std::fs::write(
        root.path().join("rustx.toml"),
        format!(
            "{}\n{}",
            CONFIG.replace("workflows = [\"review_pr\"]", "workflows = []"),
            models_json_for_base_url("http://127.0.0.1:1/v1")
        ),
    )
    .expect("rustx.toml");
    std::fs::write(
        workspace.join(".agents/agents/reviewer.toml"),
        "description = \"The Workflow-only reviewer.\"\ninstructions = \"Review requests carefully.\\n\"\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), WORKFLOW)
        .expect("workflow YAML");

    let paths = LaunchFixture {
        config: root.path().join("rustx.toml"),
        startup_session: rustx::local_runtime::StartupSession::Empty,
        workspace,
        runtime_root: root.path().join("private"),
    };
    let resources =
        LocalConversationCore::compose(&(paths).resolve(), &LocalRuntimeDependencies::default())
            .await
            .expect("discovered but non-main Workflow composes")
            .runtime()
            .runtime_resources();
    assert!(
        resources
            .workflows()
            .entries()
            .contains_key(&WorkflowId::parse("review_pr").expect("workflow id"))
    );
    assert!(resources.root_profile().unwrap().workflows.is_empty());
    assert!(
        !resources
            .capability()
            .tool_registry()
            .names()
            .contains(&"review_pr")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call() {
    use rustx::events::review::{ReviewDecision, ReviewResponse};
    use rustx::runtime::{InteractionKind, InteractionResponse, QuestionnaireResponse};
    for accepted in [false, true] {
        let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
            return;
        };
        let mut definition: serde_json::Value = serde_yaml::from_str(TOOL_WORKFLOW).unwrap();
        definition["block"]["entry"] = serde_json::json!("question");
        definition["block"]["nodes"]["question"] = serde_json::json!({
            "type":"tool","selector":{"origin":"builtin","name":"ask_user"},
            "arguments":{"type":"literal","value":{"questions":[{"question":"PRIVATE QUESTION: choose a priority","header":"Priority","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}]}]}},
            "result":{"type":"json","part":0,"schema":{"type":"object","properties":{"cancelled":{"type":"boolean"},"answers":{"type":"array","items":{"type":"object"}}},"required":["cancelled","answers"],"additionalProperties":false}}
        });
        definition["block"]["nodes"]["human"] = serde_json::json!({"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[{"type":"reference","path":["question"]}]});
        definition["block"]["nodes"]["decision"] = serde_json::json!({"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["human","accepted"]}}});
        definition["block"]["nodes"]["rejected"] = serde_json::json!({"type":"return","output":{"type":"literal","value":{"files":"review_pr.yaml review rejected"}}});
        definition["block"]["edges"]
            .as_array_mut()
            .unwrap()
            .extend([
                serde_json::json!({"from":"question","to":"human"}),
                serde_json::json!({"from":"human","to":"decision"}),
                serde_json::json!({"from":"decision","to":"inspect","port":"true"}),
                serde_json::json!({"from":"decision","to":"rejected","port":"false"}),
            ]);
        let driver =
            Driver::start_with_workflow(&emulator, &serde_yaml::to_string(&definition).unwrap())
                .await;
        let handoff = driver.submit();
        let mut seen = 0;
        let mut answered = Vec::new();
        while seen < 2 {
            let delivery =
                tokio::time::timeout(std::time::Duration::from_secs(30), driver.events.next())
                    .await
                    .expect("human publication liveness guard");
            let EventDelivery::Event(event) = delivery else {
                panic!("live event stream")
            };
            let Some(interaction) = workflow_interactions(&event)
                .into_iter()
                .find(|header| !answered.contains(&header.interaction))
            else {
                continue;
            };
            let request = driver.interaction_body(&interaction).await;
            assert_eq!(interaction.interaction, request.interaction_ref());
            assert!(matches!(
                interaction.source,
                rustx::runtime::InteractionSource::Primary
            ));
            assert_eq!(
                emulator.requests().await.len(),
                1,
                "parent remains inside its pending outer Workflow call"
            );
            let response = match request.kind {
                InteractionKind::Questionnaire { invocation_id, .. } => {
                    assert!(matches!(
                        invocation_id,
                        rustx::tools::types::ToolInvocationId::Workflow { .. }
                    ));
                    // Decline is ordinary data; this fixed program still reaches Review.
                    InteractionResponse::Questionnaire {
                        response: QuestionnaireResponse::Declined,
                    }
                }
                InteractionKind::Review {
                    review,
                    subject_digest,
                } => InteractionResponse::Review {
                    response: ReviewResponse {
                        instance: review.instance,
                        subject_digest,
                        decision: if accepted {
                            ReviewDecision::Accepted
                        } else {
                            ReviewDecision::Rejected {
                                feedback: "PRIVATE FEEDBACK".into(),
                            }
                        },
                    },
                },
                InteractionKind::Approval { .. } => panic!("no approval expected"),
            };
            for wrong in 0..2 {
                let mut target = interaction.interaction.clone();
                if wrong == 0 {
                    target.conversation_id = rustx::runtime::identity::ConversationId::new(
                        "conv_caa0e144-b0ac-7276-830d-e1900f83d43c",
                    );
                } else {
                    target.interaction_id =
                        rustx::runtime::identity::InteractionId::new("different-interaction");
                }
                let rejected = driver
                    .attachment
                    .handle_request_async(
                        rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                            id: rustx::runtime_client::RequestId::new(1000 + seen * 2 + wrong),
                            interaction: target,
                            response: response.clone(),
                        },
                    )
                    .await;
                assert!(
                    rejected.error.is_some(),
                    "wrong routed owner/id cannot settle the prompt"
                );
            }
            let result = driver
                .attachment
                .handle_request_async(
                    rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                        id: rustx::runtime_client::RequestId::new(220 + seen),
                        interaction: interaction.interaction.clone(),
                        response,
                    },
                )
                .await;
            assert!(
                matches!(result.result, Some(RuntimeClientResult::InteractionResponseAccepted { interaction: original }) if original == interaction.interaction)
            );
            answered.push(interaction.interaction);
            seen += 1;
        }
        let (_, outcome) = driver.settle(handoff).await;
        assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));
        assert_eq!(
            emulator.requests().await.len(),
            2,
            "one parent call and its ordinary continuation only"
        );
        let canonical = driver.canonical_history();
        let messages = serde_json::to_string(&canonical).unwrap();
        assert!(!messages.contains("PRIVATE QUESTION"));
        assert!(!messages.contains("PRIVATE FEEDBACK"));
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(message, rustx::message::types::MessageBlock::Tool(_)))
                .count(),
            1
        );
        emulator.finish().await;
    }
}

fn copy_reference(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_reference(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

impl Driver {
    async fn reference(emulator: &ProviderEmulator) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime");
        let workspace = root.path().join("workspace");
        copy_reference(&source, &workspace);
        let reviewer = workspace.join(".agents/agents/reviewer.toml");
        let role = std::fs::read_to_string(&reviewer)
            .unwrap()
            .replace("example/demo-model", "emulator/workflow-model");
        std::fs::write(reviewer, role).unwrap();
        let mut config: toml::Value =
            toml::from_str(&std::fs::read_to_string(workspace.join("rustx.toml")).unwrap())
                .unwrap();
        let catalog: toml::Value =
            toml::from_str(&models_json(emulator).replace("1024", "4096")).unwrap();
        config["providers"] = catalog["providers"].clone();
        config["models"] = catalog["models"].clone();
        config["agent"]["model"]["model"] = "emulator/workflow-model".into();
        config["agent"]["model"]
            .as_table_mut()
            .unwrap()
            .remove("profile");
        std::fs::write(
            root.path().join("rustx.toml"),
            toml::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
        // The copied example's authored configuration is now the bound User fixture;
        // leave Workspace resources intact and provide no duplicate authored override.
        std::fs::remove_file(workspace.join("rustx.toml")).unwrap();

        let paths = LaunchFixture {
            config: root.path().join("rustx.toml"),
            workspace,
            runtime_root: root.path().join("private"),
            startup_session: rustx::local_runtime::StartupSession::Empty,
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                KEY.to_owned(),
                "fixture".to_owned(),
            )]))),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies)
            .await
            .unwrap();
        let (attachment, initialized) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("initialized")
        };
        let cursor = runtime
            .host()
            .checkpoint()
            .expect("bounded checkpoint")
            .cursor;
        let (events, _) = runtime
            .host()
            .subscribe_events(attachment.attachment_id(), cursor)
            .unwrap();
        Self {
            root,
            runtime,
            attachment,
            events,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_uses_real_writes_checks_and_root_human_decisions() {
    Box::pin(reference_repair(false, false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_exhaustion_leaves_completed_writes_without_another_body() {
    Box::pin(reference_repair(true, false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_checker_tampering_cannot_redefine_frozen_verification() {
    Box::pin(reference_repair(false, true)).await;
}

/// Issue #462 C01/C05/C06/C07/C12: the shipped reference runs over a plain
/// (non-Git) shared Session workspace. Implement -> Check failure -> Repair
/// -> Check success follows the finite Loop; the frozen Bash checker observes
/// the Implementer's writes in place; the outer result is exactly the
/// declared output; and completed writes stay in place on every outcome.
#[allow(clippy::too_many_lines)]
async fn reference_repair(exhausted: bool, tampered: bool) {
    use rustx::events::review::{ReviewDecision, ReviewResponse};
    use rustx::runtime::{InteractionKind, InteractionResponse, QuestionnaireResponse};
    let scenario = if tampered {
        "workflow_reference_tampering"
    } else if exhausted {
        "workflow_reference_exhaustion"
    } else {
        "workflow_reference_repair"
    };
    let iterations = if exhausted { 3 } else { 2 };
    let Some(emulator) = ProviderEmulator::start(scenario).await else {
        return;
    };
    let driver = Driver::reference(&emulator).await;
    let (writer_started, mut writer_starts) = tokio::sync::watch::channel(0usize);
    let (published_workflows, mut workflow_publications) = tokio::sync::watch::channel(0u64);
    let handoff = driver.submit();
    let release_writers = async {
        for iteration in 1..=iterations {
            // The provider guard owns provider readiness, not the previous
            // writer's physical settlement/check/workspace transitions. The
            // native event loop below already owns their liveness guard.
            while *writer_starts.borrow_and_update() < iteration {
                writer_starts
                    .changed()
                    .await
                    .expect("native writer start publication");
            }
            emulator
                .await_gate(&format!("reference-writer-{iteration}"))
                .await;
            assert_eq!(emulator.requests().await.len(), 2 * iteration + 2);
            if tampered && iteration == 2 {
                // The second writer's provider gate is after the first native
                // check's settlement and failed-finding commit, before repair.
                // The forged checker the first writer left in the shared
                // workspace did not redefine the frozen verification.
                // Physical provider admission can precede the Root's source
                // publication. Wait for that exact native revision through
                // the subscribed records before capturing one public cut.
                let required_revision = driver
                    .runtime
                    .tool_runtime()
                    .workflows()
                    .snapshot()
                    .revision
                    .0;
                while *workflow_publications.borrow_and_update() < required_revision {
                    workflow_publications
                        .changed()
                        .await
                        .expect("the held writer's native Workflow revision publishes");
                }
                let checkpoint = driver
                    .runtime
                    .host()
                    .checkpoint()
                    .expect("published first-check Workflow cut");
                assert!(checkpoint.read_cut.domain_revisions["workflows"] >= required_revision);
                let workflows = driver.workflow_runs_at(&checkpoint).await;
                let run = &workflows[0];
                let checks = run
                    .instances
                    .iter()
                    .filter(|row| row.node.as_deref() == Some("check") && row.invocation.is_some())
                    .collect::<Vec<_>>();
                assert_eq!(checks.len(), 1);
                assert!(matches!(checks[0].state, WorkflowState::Settled { .. }));
                driver.assert_inflight_workflow_journal(&checkpoint, run, checks[0]);
                assert!(
                    driver
                        .root
                        .path()
                        .join("workspace/checks/verify_greeting.py")
                        .exists()
                );
            }
            emulator
                .release_gate(&format!("reference-writer-{iteration}"))
                .await;
        }
    };
    let interact = async {
        let writer_started = writer_started;
        let mut reviews = 0;
        let mut approvals = 0;
        let mut questions = 0;
        let mut check_commands = Vec::new();
        let mut writes = 0;
        let mut original = None;
        let mut answered = Vec::new();
        'wait: loop {
            let EventDelivery::Event(event) =
                tokio::time::timeout(std::time::Duration::from_mins(1), driver.events.next())
                    .await
                    .unwrap()
            else {
                panic!("events")
            };
            // Every complete published record is a readiness observation.
            // Workflow source revisions can arrive in outer metadata without
            // an inline WorkflowsUpdated fact or checkpoint replacement.
            let (kind, revision) = match &event.event {
                RuntimeClientEvent::ProjectionTransition { metadata, .. } => (
                    "projection_transition",
                    metadata.read_cut.domain_revisions.get("workflows").copied(),
                ),
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => (
                    "checkpoint_advanced",
                    checkpoint
                        .read_cut
                        .domain_revisions
                        .get("workflows")
                        .copied(),
                ),
                _ => ("other", None),
            };
            if let Some(revision) = revision {
                published_workflows.send_modify(|old| *old = (*old).max(revision));
            }
            let native = driver.runtime.tool_runtime().workflows().snapshot();
            let previous = *writer_started.borrow();
            record_writer_starts(&native.runs, &writer_started);
            let admitted_count = *writer_started.borrow();
            if admitted_count > previous {
                eprintln!(
                    "Workflow writer readiness cursor={} type={kind} published_workflows={revision:?} native_workflows={} writers={admitted_count}",
                    event.cursor.get(),
                    native.revision.0,
                );
            }
            let terminal = workflow_terminal(
                &event,
                &mut original,
                driver.runtime.runtime().conversation_id(),
            );
            for fact in event.event.facts() {
                if let RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } = fact {
                    assert_eq!(checkpoint.cursor, event.cursor);
                    assert_eq!(checkpoint.read_cut.revision, event.cursor.get());
                    assert_eq!(
                        &checkpoint.read_cut.conversation_id,
                        driver.runtime.runtime().conversation_id()
                    );
                }
            }
            for interaction in workflow_interactions(&event) {
                if answered.contains(&interaction.interaction) {
                    continue;
                }
                let response = match driver.interaction_body(&interaction).await.kind {
                    InteractionKind::Questionnaire {
                        ref questionnaire, ..
                    } => {
                        questions += 1;
                        // The answer names the option by its index in the
                        // published request, exactly as a Runtime Client
                        // resolves a display label locally.
                        let option_index = questionnaire.questions[0]
                            .answer
                            .options()
                            .iter()
                            .position(|option| option.label == "Minimal change")
                            .expect("the questionnaire offers the minimal change option");
                        InteractionResponse::Questionnaire {
                            response: QuestionnaireResponse::Submitted(
                                rustx::events::QuestionnaireSubmission {
                                    answers: vec![rustx::events::QuestionnaireAnswerEntry {
                                        question_index: 0,
                                        answer: rustx::events::QuestionnaireAnswer::Option(
                                            rustx::events::OptionAnswer { option_index },
                                        ),
                                    }],
                                },
                            ),
                        }
                    }
                    InteractionKind::Review {
                        review,
                        subject_digest,
                    } => {
                        reviews += 1;
                        // Only the plan is reviewed, before any writer
                        // can start: the explicit Branch gates the Loop.
                        assert_eq!(emulator.requests().await.len(), 2);
                        assert_eq!(*writer_started.borrow(), 0);
                        InteractionResponse::Review {
                            response: ReviewResponse {
                                instance: review.instance,
                                subject_digest,
                                decision: ReviewDecision::Accepted,
                            },
                        }
                    }
                    InteractionKind::Approval {
                        tool_name,
                        arguments,
                        ..
                    } => {
                        approvals += 1;
                        if tool_name == "bash" {
                            check_commands.push(arguments["command"].as_str().unwrap().to_owned());
                        } else {
                            assert_eq!(tool_name, "write");
                            writes += 1;
                        }
                        serde_json::from_value::<InteractionResponse>(
                            serde_json::json!({"type":"approval","decision":{"type":"allow"}}),
                        )
                        .unwrap()
                    }
                };
                let reply = driver
                    .attachment
                    .handle_request_async(
                        rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                            id: rustx::runtime_client::RequestId::new(
                                questions + reviews + approvals,
                            ),
                            interaction: interaction.interaction.clone(),
                            response,
                        },
                    )
                    .await;
                assert!(reply.error.is_none(), "{reply:?}");
                assert!(
                    matches!(reply.result, Some(RuntimeClientResult::InteractionResponseAccepted { interaction: original }) if original == interaction.interaction)
                );
                answered.push(interaction.interaction);
            }
            if let Some(outcome) = terminal {
                if !matches!(outcome, RuntimeClientOutcome::Completed { .. }) {
                    driver
                        .failure_diagnostics(&emulator, "reference terminal")
                        .await;
                }
                assert!(
                    matches!(outcome, RuntimeClientOutcome::Completed { .. }),
                    "{outcome:?}: {}",
                    emulator.diagnostics()
                );
                break 'wait;
            }
        }
        if (questions, reviews, approvals)
            != (
                2,
                1,
                u64::try_from(2 * iterations + usize::from(tampered)).unwrap(),
            )
        {
            eprintln!(
                "reference decisions questions={questions} reviews={reviews} approvals={approvals} writes={writes} checks={}",
                check_commands.len()
            );
            driver
                .failure_diagnostics(&emulator, "reference decision count")
                .await;
        }
        assert_eq!(
            (questions, reviews, approvals),
            (
                2,
                1,
                u64::try_from(2 * iterations + usize::from(tampered)).unwrap()
            )
        );
        assert_eq!(writes, iterations + usize::from(tampered));
        assert_eq!(check_commands.len(), iterations);
        let definition: serde_json::Value = serde_yaml::from_str(include_str!(
            "../../examples/local-runtime/.agents/workflows/implement_and_review.yaml"
        ))
        .unwrap();
        let frozen = definition["block"]["nodes"]["repair"]["body"]["nodes"]["check"]["arguments"]
            ["value"]["command"]
            .as_str()
            .unwrap();
        assert!(frozen.starts_with("python3 -I -B - <<'PY'"));
        assert!(!frozen.contains("checks/verify_greeting.py"));
        assert!(check_commands.iter().all(|command| command == frozen));
    };
    tokio::join!(release_writers, interact);
    tokio::time::timeout(std::time::Duration::from_secs(30), handoff)
        .await
        .expect("Workflow native handoff completes");
    assert_eq!(emulator.requests().await.len(), 4 + 2 * iterations);
    let workspace = driver.root.path().join("workspace");
    assert!(
        !workspace.join(".git").exists(),
        "the reference needs no Git repository"
    );
    let canonical = driver.canonical_history();
    let messages = serde_json::to_string(&canonical).unwrap();
    assert!(!messages.contains("PRIVATE CLAIM"));
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    let outer = canonical
        .iter()
        .find_map(|m| match m {
            rustx::message::types::MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .unwrap();
    let [rustx::tools::types::ToolResultContent::Json { value: output }] =
        outer.result.content.as_slice()
    else {
        panic!(
            "the Workflow result is one JSON part: {:?}",
            outer.result.content
        )
    };
    assert_eq!(
        *output,
        if exhausted {
            serde_json::json!({
                "status": "needs_revision",
                "feedback": "The checker ran successfully but greeting.py failed at least one fixed case. Repair greeting.py."
            })
        } else {
            serde_json::json!({
                "status": "implemented",
                "feedback": "Both fixed greeting cases passed."
            })
        },
        "the outer result is exactly the declared output"
    );
    let workflows = driver.workflow_runs().await;
    let run = &workflows[0];
    assert_eq!(run.agents_consumed, 1 + iterations);
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.node.as_deref() == Some("implement"))
            .count(),
        iterations
    );
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.node.as_deref() == Some("check"))
            .count(),
        iterations
    );
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.kind == WorkflowNodeKind::Tool)
            .count(),
        1 + iterations
    );
    assert!(
        run.instances
            .iter()
            .filter(|row| row.child.is_some() || row.invocation.is_some())
            .all(|row| matches!(row.state, WorkflowState::Settled { .. }))
    );
    if tampered {
        assert_eq!(
            std::fs::read_to_string(workspace.join("checks/verify_greeting.py")).unwrap(),
            "print(\"passed\", end=\"\")\n",
            "completed effects stay in place; they never redefine verification"
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("greeting.py")).unwrap(),
        if exhausted {
            "def greeting(name):\n    return \"wrong\"\n"
        } else {
            "def greeting(name):\n    return 'Hello, ' + (name or 'friend') + '!'\n"
        },
        "the Implementer's writes land in the shared workspace with no rollback"
    );

    emulator.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_parallel_reverses_completion_without_git_or_parent_orchestration() {
    let Some(emulator) = ProviderEmulator::start("workflow_reference_parallel").await else {
        return;
    };
    let driver = Driver::reference(&emulator).await;
    let handoff = driver.submit();
    emulator.await_gate("parallel-child-0").await;
    emulator.await_gate("parallel-child-1").await;
    assert_eq!(emulator.requests().await.len(), 3);
    emulator.release_gate("parallel-child-1").await;
    // The native owner publication, not elapsed time, proves the second child
    // settled while the first provider response is still held.
    loop {
        let EventDelivery::Event(_) =
            tokio::time::timeout(std::time::Duration::from_secs(30), driver.events.next())
                .await
                .unwrap()
        else {
            panic!("events")
        };
        let native = driver.runtime.tool_runtime().workflows().snapshot();
        if native
            .runs
            .iter()
            .flat_map(|run| &run.instances)
            .any(|row| {
                row.kind == WorkflowNodeKind::Agent
                    && matches!(row.state, WorkflowState::Settled { .. })
            })
        {
            break;
        }
    }
    emulator.release_gate("parallel-child-0").await;
    emulator.await_gate("parallel-child-2").await;
    emulator.release_gate("parallel-child-2").await;
    let (_, outcome) = driver.settle(handoff).await;
    if !matches!(outcome, RuntimeClientOutcome::Completed { .. }) {
        driver
            .failure_diagnostics(&emulator, "parallel terminal")
            .await;
    }
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}: {}",
        emulator.diagnostics()
    );
    assert_eq!(emulator.requests().await.len(), 5);
    let canonical = driver.canonical_history();
    let workflows = driver.workflow_runs().await;
    let run = &workflows[0];
    assert_eq!(run.agents_consumed, 3);
    assert!(!driver.root.path().join("workspace/.git").exists());
    let history = serde_json::to_string(&canonical).unwrap();
    assert!(history.contains("quality") && history.contains("security"));
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    emulator.finish().await;
}
