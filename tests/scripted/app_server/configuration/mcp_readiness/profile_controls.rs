//! Runtime catalog facts preserve the profile-owned Agent control plane.
use super::*;

fn admit_reviewer_without_workflows(fixture: &Fixture) {
    issue459_write_reviewer(fixture, "runtime catalog reviewer", None);
    let path = fixture.workspaces[0].join("rustx.toml");
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let agent = document
        .as_table_mut()
        .unwrap()
        .entry("agent")
        .or_insert_with(|| toml::Value::Table(toml::Table::default()))
        .as_table_mut()
        .unwrap();
    agent.insert("agents".into(), vec![toml::Value::from("reviewer")].into());
    agent.insert("workflows".into(), Vec::<toml::Value>::new().into());
    std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
}

fn assert_agent_controls(resources: &crate::runtime::resources::RuntimeResourceSnapshot) {
    let capability = resources.capability();
    let definitions = capability.tool_registry().definitions();
    for name in crate::tools::native::AGENT_CONTROL_TOOL_NAMES {
        assert!(
            definitions.iter().any(|tool| tool.name == name),
            "missing profile-composed Agent control {name}"
        );
        assert!(
            capability
                .available_tools()
                .tools()
                .iter()
                .all(|tool| tool.definition.name != name),
            "Agent control {name} entered the ordinary selectable catalog"
        );
    }
    assert!(resources.workflows().is_empty());
}

fn tool_names(resources: &crate::runtime::resources::RuntimeResourceSnapshot) -> Vec<String> {
    let mut names = resources
        .capability()
        .tool_registry()
        .definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_request_controls(body: &str, expected: &[String]) {
    let request: serde_json::Value = serde_json::from_str(body).unwrap();
    let tools = request["tools"].as_array().unwrap();
    for name in crate::tools::native::AGENT_CONTROL_TOOL_NAMES {
        assert!(
            tools.iter().any(|tool| tool["function"]["name"] == name),
            "provider request lost profile-composed Agent control {name}"
        );
    }
    let mut names = tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names.as_slice(),
        expected,
        "the complete adopted tool inventory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_catalog_keeps_named_agent_controls_without_a_workflow() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        admit_reviewer_without_workflows(&fixture);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let original = runtime.configuration_cut();
        assert_agent_controls(&original.resources);
        let expected = tool_names(&original.resources);
        let mut publications = runtime.runtime_catalog_publications();
        let published = *publications.borrow();
        let mut observed = runtime.runtime_catalog_observations();

        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"unchanged"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        let discovered = runtime.capability().catalog_revision();
        observed
            .wait_for(|revision| *revision >= discovered)
            .await
            .unwrap();
        let current = runtime.configuration_cut();
        assert_eq!(current.binding_revision, original.binding_revision);
        assert_eq!(current.resources.revision(), original.resources.revision());
        assert_eq!(
            current.resources.capability().revision(),
            original.resources.capability().revision()
        );
        assert_agent_controls(&current.resources);
        assert_eq!(*publications.borrow(), published);
        assert!(!publications.has_changed().unwrap());
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 1);

        // A supported explicit recovery owns a new live connection, but
        // recomposes exactly the same admitted Agent control plane.
        runtime.refresh_runtime_catalogs().await.unwrap();
        wait_runtime_catalog(&runtime, &mut publications, published).await;
        assert_agent_controls(&runtime.runtime_resources());
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 1);

        runtime
            .submit_inbound(input("request-B: unchanged Agent controls"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1);
        assert_request_controls(&requests[0], &expected);
        assert_eq!(tool_schema(&requests[0]).as_deref(), Some("string"));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_attempt_keeps_agent_controls_and_next_attempt_gets_live_schema() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(Some("read"), 0).await;
        configure(&fixture, &[("late", &mcp)]);
        admit_reviewer_without_workflows(&fixture);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let frozen = runtime.runtime_resources();
        assert_agent_controls(&frozen);
        let expected = tool_names(&frozen);
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        runtime
            .submit_inbound(input("request-A: held Agent-enabled Attempt"))
            .unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        assert_eq!(runtime.runtime_resources().revision(), frozen.revision());
        assert_eq!(
            runtime.configuration_cut().admitted_attempt.unwrap(),
            admitted
        );
        assert_agent_controls(&runtime.runtime_resources());
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);

        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 2, "one whole two-step Attempt");
        for request in &requests {
            assert_request_controls(request, &expected);
            assert_eq!(tool_schema(request).as_deref(), Some("string"));
        }
        assert_agent_controls(&runtime.runtime_resources());
        runtime
            .submit_inbound(input("request-B: automatically reconciled Agent controls"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 3);
        assert_request_controls(&requests[2], &expected);
        assert_eq!(tool_schema(&requests[2]).as_deref(), Some("integer"));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}
