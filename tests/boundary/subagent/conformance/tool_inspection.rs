// Real Child inspection uses the same finite Tool heads on live IPC and cold Store routes.
use super::*;
use crate::observation::bounded::decode_domain_value;
use crate::observation::trace::{TraceFieldSource, TraceKind};
use crate::runtime_client::trace::{
    TraceInspectionContent, TraceInspectionHead, TraceToolDefinitionSelection,
};
use crate::runtime_client::{RuntimeClientError, RuntimeClientResult};
use base64::Engine as _;

struct ReleaseModel(tokio::sync::watch::Sender<bool>);
impl Drop for ReleaseModel {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

async fn selected_tool_head(
    host: &crate::runtime_client::RuntimeClientHost,
    agent: &AgentId,
    activation: &SubagentId,
    store: &dyn ConversationStore,
) -> TraceInspectionHead {
    let before = store.observation_read_cut().unwrap();
    let RuntimeClientResult::TracePage { page } = host
        .inner
        .agent_trace_via_inspection(agent, None, 32, Vec::new())
        .await
        .unwrap()
    else {
        panic!("original Child Trace page")
    };
    let cut = page.read_cut.as_ref().unwrap();
    assert_eq!(cut.activation_id.as_ref(), Some(activation));
    let record = page
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Tool)
        .unwrap();
    let detail_domain =
        crate::runtime_client::trace_observation::detail_domain(&record.id).unwrap();
    let detail = host
        .inner
        .agent_checkpoint_page(agent, cut, &detail_domain, 0, 8)
        .await
        .unwrap();
    assert_eq!(detail.read_cut, *cut);
    assert_eq!(detail.items[0].body_complete, Some(false));
    let head: TraceInspectionHead =
        decode_domain_value(detail.items[0].value.clone().unwrap()).unwrap();
    assert_eq!(head.id, record.id);
    assert_eq!(
        head.record.tool.as_ref().unwrap().name.as_deref(),
        Some("inspect_tool")
    );
    assert_child_definition(host, agent, cut, &head).await;
    let mut foreign = cut.clone();
    foreign.activation_id = Some(SubagentId::new("foreign-tool-inspection"));
    assert!(matches!(
        host.inner
            .agent_checkpoint_page(agent, &foreign, &detail_domain, 0, 8)
            .await,
        Err(RuntimeClientError::ReadCutExpired)
    ));
    assert_eq!(
        store.observation_read_cut().unwrap(),
        before,
        "Child inspection cannot advance native source"
    );
    head
}

async fn assert_child_definition(
    host: &crate::runtime_client::RuntimeClientHost,
    agent: &AgentId,
    cut: &crate::observation::cut::CheckpointReadCut,
    head: &TraceInspectionHead,
) {
    let TraceInspectionContent::Tool {
        definition:
            TraceToolDefinitionSelection::Available {
                field:
                    TraceFieldSource::Value {
                        domain,
                        item_offset,
                        path,
                        encoded_bytes,
                        ..
                    },
            },
    } = &head.content
    else {
        panic!("original Child request catalog")
    };
    let chunk = host
        .inner
        .agent_checkpoint_value_read(agent, cut, domain, *item_offset, path, (0, 65_536))
        .await
        .unwrap();
    assert_eq!(chunk.cut, *cut);
    assert_eq!(chunk.readable_end, *encoded_bytes);
    assert!(chunk.next_offset.is_none());
    let definition: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(definition["tool_id"], "tool-inspect");
    assert_eq!(definition["name"], "inspect_tool");
    assert_eq!(definition["description"]["text"], "Tool inspect_tool");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t12_t18_t19_tool_inspection_uses_real_child_live_and_cold_original_sources() {
    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000506",
        Vec::new(),
    )
    .await;
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000507");
    let path = crate::runtime::subagent::child_conversation_store_path(
        &parent.plane.runtime_root,
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &conversation,
    );
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool_policies(
            "inspect_tool",
            "tool-inspect",
            crate::tools::types::ToolExecutionPolicy::ForegroundOnly,
            crate::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("original Child inspection result"),
    )
    .register(&mut tools);
    let (release, released) = support::fake::model_release();
    let release = ReleaseModel(release);
    let child = child_fixture_scoped_at(
        &directory,
        &conversation,
        vec![
            tool_call_request(&support::fake::ScriptedCall {
                id: "child-inspection-call",
                tool_id: "tool-inspect",
                name: "inspect_tool",
                arguments: serde_json::json!({}),
            }),
            parking_answer_script("inspection answer", released),
        ],
        tools,
        Vec::new(),
        path.parent().unwrap().to_path_buf(),
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            1,
        )),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "inspect original tool").await;
    await_model_parks(&child.model, 1).await;
    child.host.inner.projection_settled().await;
    let socket = parent
        .plane
        .registry
        .agent_inspection_path(&wired.accepted.child_agent_id)
        .unwrap();
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            socket,
            child.host.clone(),
            wired.accepted.subagent_id.clone(),
        )
        .unwrap();
    let live = selected_tool_head(
        &host,
        &wired.accepted.child_agent_id,
        &wired.accepted.subagent_id,
        child.store.as_ref(),
    )
    .await;
    assert_eq!(child.model.requests().len(), 2);
    release.0.send(true).unwrap();
    await_serve(wired.serve).await;
    parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .unwrap();
    inspection.shutdown().await.unwrap();
    child.runtime.shutdown().await.unwrap();
    let cold = selected_tool_head(
        &host,
        &wired.accepted.child_agent_id,
        &wired.accepted.subagent_id,
        child.store.as_ref(),
    )
    .await;
    assert_eq!(cold.content, live.content);
    assert_eq!(cold.record.tool, live.record.tool);
    assert_eq!(cold.record.fields, live.record.fields);
    assert_eq!(child.model.requests().len(), 2);
    parent.runtime.shutdown().await.unwrap();
}
