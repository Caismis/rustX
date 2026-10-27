use crate::durable::ConversationStore;
use crate::runtime_client::capability_source::CapabilityReadSource;
use crate::runtime_client::snapshot::{CapabilityView, RuntimeClientTool};
use crate::runtime_client::{checkpoint, preflight};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

fn tool(index: usize) -> RuntimeClientTool {
    RuntimeClientTool {
        id: ToolId::new(format!("tool-{index}")),
        name: format!("tool_{index}"),
        description: "unchanged 水🙂\\\"\n".repeat(64),
        input_schema: serde_json::json!({"type":"object","properties":{"n":{"minimum":-0.0}}}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Parallel,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

fn header(tools: Vec<RuntimeClientTool>) -> CapabilityView {
    CapabilityView {
        revision: crate::runtime::identity::CapabilityRevision::new(7),
        configured_tools: tools.clone(),
        available_tools: tools.clone(),
        tools,
        skills: Vec::new(),
        sources: Vec::new(),
    }
}

fn initialized(header: CapabilityView) -> RuntimeClientProjection {
    let mut projection = RuntimeClientProjection::new(
        ConversationId::new("conv_2c7c0275-c7f3-7cc1-8357-9e493eae087a"),
        Vec::new(),
        header,
        Some(model_view()),
        64,
    );
    let store = crate::durable::SqliteConversationStore::in_memory(
        projection.snapshot.conversation_id.clone(),
    )
    .unwrap();
    projection
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    projection.initialize_publication_view();
    projection
}

fn source(conversation: ConversationId) -> CapabilityReadSource {
    use crate::scripted_suites::support::fake::{FakeTool, success_result};
    let mut registry = ToolRegistry::new();
    let definition = serde_json::from_value(serde_json::to_value(tool(99)).unwrap()).unwrap();
    FakeTool::new(definition, success_result("replacement")).register(&mut registry);
    let definitions = registry.definitions();
    let snapshot = Arc::new(crate::capabilities::CapabilitySnapshot::new(
        conversation,
        std::path::PathBuf::from("."),
        crate::runtime::identity::CapabilityRevision::new(7),
        Arc::new(registry),
        Arc::new(crate::capabilities::AvailableToolCatalog::metadata(
            definitions,
        )),
        Arc::new(crate::skills::SkillSnapshot::from_frozen(
            Vec::new(),
            Vec::new(),
        )),
        None,
        None,
        crate::tools::environment::ToolEnvironment::default(),
        Arc::new(crate::tools::mcp::McpRuntimeLeaseAuthority::empty()),
        Arc::default(),
    ));
    let source = CapabilityReadSource::new(
        snapshot,
        crate::capabilities::CapabilityAvailability::new(),
        None,
    );
    source.prepare();
    source
}

fn compare_original(seed: &super::super::CheckpointSeed) {
    let (mut original, original_evicted) = seed.checkpoint_unfitted().unwrap();
    let (mut actual, evicted, budget) = seed.checkpoint_prepared().unwrap();
    assert_eq!(evicted, original_evicted);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    if let Some(budget) = budget {
        assert!(
            preflight::required_safe_integer_budget(&actual, checkpoint::CHECKPOINT_TARGET_BYTES)
                .is_some_and(|direct| direct <= budget)
        );
    }
    assert_eq!(
        checkpoint::fit_checkpoint_with_budget(&mut actual, budget).unwrap(),
        checkpoint::fit_checkpoint(&mut original).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
}

#[test]
fn immutable_capability_owner_is_shared_across_unrelated_publications() {
    let mut projection = initialized(header((0..8).map(tool).collect()));
    let first = projection.checkpoint_seed();
    let first_bytes = serde_json::to_vec(&first.snapshot.capabilities).unwrap();
    first.checkpoint_candidate().unwrap();
    let mut retained = vec![first.clone()];
    for _ in 0..32 {
        checkpoint::take_capture_work();
        publish_source_fact(&mut projection, |projection| {
            projection.publish(RuntimeClientEvent::TraceChanged);
        });
        let current = projection.checkpoint_seed();
        assert!(Arc::ptr_eq(
            &first.snapshot.capabilities,
            &current.snapshot.capabilities
        ));
        assert!(Arc::ptr_eq(
            &projection.snapshot.capabilities,
            &current.snapshot.capabilities
        ));
        let work = checkpoint::take_capture_work();
        assert_eq!(
            work[0], 0,
            "unchanged capability rows are never normalized: {work:?}"
        );
        assert_eq!(
            work[4], 0,
            "ordinary delta does not copy a complete checkpoint"
        );
        retained.push(current);
    }
    let owners = retained
        .iter()
        .map(|seed| Arc::as_ptr(&seed.snapshot.capabilities))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        owners.len(),
        1,
        "33 frozen cuts retain one raw header allocation"
    );
    assert_eq!(
        serde_json::to_vec(&first.snapshot.capabilities).unwrap(),
        first_bytes
    );
    compare_original(retained.last().unwrap());
}

fn compare_raw_capability_pages(retained: &super::super::CheckpointSeed, raw: &CapabilityView) {
    for (domain, rows) in [
        ("capabilities.tools", &raw.tools),
        ("capabilities.configured_tools", &raw.configured_tools),
        ("capabilities.available_tools", &raw.available_tools),
    ] {
        for offset in 0..=rows.len() as u64 {
            let expected = checkpoint::page(
                domain,
                retained.read_cut.clone(),
                RuntimeClientCursor::new(retained.read_cut.revision),
                rows,
                offset,
                1,
                |tool| tool.id.to_string(),
            )
            .unwrap();
            let actual = retained.checkpoint_page(&retained.read_cut, domain, offset, 1).unwrap();
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }
}

#[test]
fn immutable_capability_owner_preserves_raw_pages_and_historical_cut_on_replacement() {
    let mut oversized = tool(1);
    oversized.description = "x".repeat(checkpoint::CHECKPOINT_ITEM_BYTES);
    let mut overwide = tool(2);
    overwide.input_schema = serde_json::json!({"enum": vec![0; 33]});
    let mut opaque_integer = tool(3);
    opaque_integer.input_schema = serde_json::json!({"integer": u64::MAX});
    let raw = header(vec![tool(0), oversized, overwide, opaque_integer, tool(4)]);
    let raw_bytes = serde_json::to_vec(&raw).unwrap();
    let mut projection = initialized(raw.clone());
    let retained = projection.checkpoint_seed();
    let cut = retained.read_cut.clone();
    assert!(retained.capability_head.header().tools.len() < raw.tools.len());
    assert_eq!(
        serde_json::to_vec(&retained.snapshot.capabilities).unwrap(),
        raw_bytes
    );
    compare_raw_capability_pages(&retained, &raw);
    let pages = retained
        .checkpoint_page(&cut, "capabilities.tools", 0, 5)
        .unwrap();
    assert_eq!(pages.total, 5);
    assert!(!pages.items[0].deferred);
    assert!(pages.items[1].deferred && pages.items[2].deferred);
    // Structural pages preserve opaque JSON numbers. A wide number alone is
    // not an invalid row; the preceding 33-item container exceeds admission.
    assert!(!pages.items[3].deferred);
    assert_eq!(
        pages.items[3].value.as_ref().unwrap()["input_schema"],
        raw.tools[3].input_schema
    );
    assert!(
        !pages.items[4].deferred,
        "the raw page must retain rows beyond an invalid head"
    );
    let whole = checkpoint::page(
        "capabilities",
        cut.clone(),
        RuntimeClientCursor::new(cut.revision),
        std::slice::from_ref(&raw),
        0,
        1,
        |_| "capabilities".into(),
    )
    .unwrap();
    assert_eq!(
        retained
            .checkpoint_page(&cut, "capabilities", 0, 1)
            .unwrap(),
        whole
    );
    compare_original(&retained);

    let source = source(projection.snapshot.conversation_id.clone());
    let replacement = source.header();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::Capability { source });
    });
    let current = projection.checkpoint_seed();
    assert_eq!(current.snapshot.capabilities.as_ref(), &replacement);
    assert_eq!(current.snapshot.capabilities.revision, raw.revision);
    assert!(!Arc::ptr_eq(
        &current.snapshot.capabilities,
        &retained.snapshot.capabilities
    ));
    assert_ne!(
        current
            .read_cut
            .domain_revisions
            .get("capabilities")
            .copied()
            .unwrap_or(0),
        cut.domain_revisions
            .get("capabilities")
            .copied()
            .unwrap_or(0)
    );
    assert!(matches!(
        current.checkpoint_page(&cut, "capabilities.tools", 0, 1),
        Err(RuntimeClientError::ReadCutExpired)
    ));
    assert_eq!(
        retained
            .checkpoint_page(&cut, "capabilities.tools", 0, 5)
            .unwrap(),
        pages
    );
    assert_eq!(
        serde_json::to_vec(&retained.snapshot.capabilities).unwrap(),
        raw_bytes
    );
    compare_original(&current);
}

#[test]
fn immutable_capability_owner_keeps_schema_wire_and_snapshot_copy_independent() {
    let raw = header(vec![tool(0)]);
    let projection = initialized(raw.clone());
    let seed = projection.checkpoint_seed();
    assert_eq!(
        serde_json::to_vec(seed.snapshot.capabilities.as_ref()).unwrap(),
        serde_json::to_vec(&raw).unwrap()
    );
    assert_eq!(
        serde_json::to_value(schemars::schema_for!(Arc<CapabilityView>)).unwrap(),
        serde_json::to_value(schemars::schema_for!(CapabilityView)).unwrap()
    );
    let original = seed.checkpoint_candidate().unwrap();
    let mut copy = seed.snapshot.clone();
    assert!(Arc::ptr_eq(&copy.capabilities, &seed.snapshot.capabilities));
    Arc::make_mut(&mut copy.capabilities).tools[0].description = "changed copy".into();
    assert!(!Arc::ptr_eq(
        &copy.capabilities,
        &seed.snapshot.capabilities
    ));
    assert_eq!(seed.snapshot.capabilities.as_ref(), &raw);
    assert_eq!(seed.checkpoint_candidate().unwrap(), original);
    let encoded = serde_json::to_value(&copy).unwrap();
    let decoded: crate::runtime_client::RuntimeClientSnapshot =
        serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, copy);
    compare_original(&seed);
}
