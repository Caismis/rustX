use super::*;
use crate::observation::bounded::prepare_capture_item;
use crate::observation::capability_source::{CapabilityReadSource, PreparedCapabilityHead};
use crate::observation::capability_source::{CapabilityView, RuntimeClientTool};
use crate::observation::capture::Capture;
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

fn tool() -> RuntimeClientTool {
    RuntimeClientTool {
        id: crate::runtime::identity::ToolId::new("tool-original"),
        name: "original".into(),
        description: "水🙂\\\"\n".into(),
        input_schema: serde_json::json!({"type":"object","properties":{"n":{"minimum":1}}}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Parallel,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}
fn view(tools: Vec<RuntimeClientTool>) -> CapabilityView {
    CapabilityView {
        revision: crate::runtime::identity::CapabilityRevision::new(7),
        configured_tools: Vec::new(),
        tools,
        available_tools: Vec::new(),
        skills: Vec::new(),
        sources: Vec::new(),
    }
}
fn old_capture(header: &CapabilityView, capture: &mut Capture) -> CapabilityView {
    CapabilityView {
        revision: header.revision,
        configured_tools: capture.page("capabilities.configured_tools", &header.configured_tools),
        tools: capture.page("capabilities.tools", &header.tools),
        available_tools: capture.page("capabilities.available_tools", &header.available_tools),
        skills: capture.page("capabilities.skills", &header.skills),
        sources: capture.page("capabilities.sources", &header.sources),
    }
}
fn compare(header: &CapabilityView, prepared: &PreparedCapabilityHead, budget: usize) {
    let mut old = Capture::new(41);
    let mut actual = Capture::new(41);
    old.remaining = budget;
    actual.remaining = budget;
    assert_eq!(
        prepared.capture(&mut actual),
        old_capture(header, &mut old),
        "budget {budget}"
    );
    assert_eq!(actual.remaining, old.remaining, "budget {budget}");
    assert_eq!(actual.evicted, old.evicted, "budget {budget}");
    assert_eq!(actual.collections, old.collections, "budget {budget}");
    assert_eq!(actual.content, old.content);
}

#[test]
fn prepared_capability_matches_original_capture_at_every_remaining_item_budget() {
    let header = view(vec![tool()]);
    let prepared = PreparedCapabilityHead::new(&header, Vec::new());
    let bytes = bounded_item_bytes(&header.tools[0], CHECKPOINT_ITEM_BYTES).unwrap();
    let required =
        crate::observation::preflight::required_budget(&header.tools[0], CHECKPOINT_ITEM_BYTES)
            .unwrap();
    assert!(
        required > bytes.len(),
        "conservative numeric accounting remains significant"
    );
    for budget in 0..=CHECKPOINT_ITEM_BYTES {
        compare(&header, &prepared, budget);
    }
}

#[test]
fn prepared_capability_preserves_large_initial_items_and_exact_rejection_frontiers() {
    let mut large = tool();
    large.description = "original".repeat(1000);
    let encoded = bounded_item_bytes(&large, CHECKPOINT_ITEM_BYTES).unwrap();
    assert!(encoded.len() > 2048 && encoded.len() < CHECKPOINT_ITEM_BYTES);
    let mut giant_id = tool();
    giant_id.id = crate::runtime::identity::ToolId::new("x".repeat(CHECKPOINT_ITEM_BYTES));
    let mut giant_body = tool();
    giant_body.input_schema = serde_json::json!({"text":"x".repeat(CHECKPOINT_ITEM_BYTES)});
    let mut unsafe_json = tool();
    unsafe_json.input_schema = serde_json::json!({"integer":u64::MAX});
    let mut wide = tool();
    wide.input_schema = serde_json::json!(vec![0; 33]);
    let cases = [
        view(vec![large]),
        view(vec![tool(), giant_id, tool()]),
        view(vec![tool(), giant_body, tool()]),
        view(vec![tool(), unsafe_json, tool()]),
        view(vec![tool(), wide, tool()]),
        view(vec![tool(); CHECKPOINT_PAGE_ITEMS + 1]),
    ];
    for header in cases {
        let prepared = PreparedCapabilityHead::new(&header, Vec::new());
        let mut budgets = vec![
            0,
            1,
            2048,
            CHECKPOINT_ITEM_BYTES,
            CHECKPOINT_TARGET_BYTES / 8,
        ];
        let mut charged = 0;
        for item in header.tools.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some((_, bytes, floor)) = prepare_capture_item(item) else {
                break;
            };
            for boundary in [charged + bytes, charged + floor] {
                budgets.extend([boundary.saturating_sub(1), boundary, boundary + 1]);
            }
            charged += bytes;
        }
        for budget in budgets {
            compare(&header, &prepared, budget);
        }
    }
}

fn native_snapshot() -> std::sync::Arc<crate::capabilities::CapabilitySnapshot> {
    use crate::scripted_suites::support::fake::{FakeTool, success_result};
    let mut registry = crate::tools::executor::ToolRegistry::new();
    for id in ["tool-original", "tool-read-image"] {
        let mut item = tool();
        item.id = crate::runtime::identity::ToolId::new(id);
        item.name = id.into();
        let definition = serde_json::from_value(serde_json::to_value(item).unwrap()).unwrap();
        FakeTool::new(definition, success_result("original")).register(&mut registry);
    }
    let definitions = registry.definitions();
    std::sync::Arc::new(crate::capabilities::CapabilitySnapshot::new(
        crate::runtime::identity::ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b"),
        std::path::PathBuf::from("."),
        crate::runtime::identity::CapabilityRevision::new(7),
        std::sync::Arc::new(registry),
        std::sync::Arc::new(crate::capabilities::AvailableToolCatalog::metadata(
            definitions,
        )),
        std::sync::Arc::new(crate::skills::SkillSnapshot::from_frozen(
            Vec::new(),
            Vec::new(),
        )),
        None,
        None,
        crate::tools::environment::ToolEnvironment::default(),
        std::sync::Arc::new(crate::tools::mcp::McpRuntimeLeaseAuthority::empty()),
        std::sync::Arc::default(),
    ))
}

#[test]
fn prepared_capability_same_revision_model_and_availability_keep_exact_source_owners() {
    use crate::capabilities::{CapabilityAvailability, CapabilitySourceState, ToolSourceId};
    let snapshot = native_snapshot();
    let source = CapabilityReadSource::new(snapshot.clone(), CapabilityAvailability::new(), None);
    source.prepare();
    let original = source.prepared_head();
    assert!(original.same_owner(&source.prepared_head()));
    let mut image_model = crate::model::catalog::ModelCapabilities::text_only(true);
    image_model
        .input_modalities
        .insert(crate::model::catalog::Modality::Image);
    let changed = source.with_model(Some(image_model));
    changed.prepare();
    let image = changed.prepared_head();
    assert_eq!(changed.revision(), source.revision());
    assert!(!original.same_owner(&image));
    assert!(
        !source
            .header()
            .tools
            .iter()
            .any(|tool| tool.id.as_str() == "tool-read-image")
    );
    assert!(
        changed
            .header()
            .tools
            .iter()
            .any(|tool| tool.id.as_str() == "tool-read-image")
    );
    let availability = CapabilityAvailability::from([(
        ToolSourceId::ManagedPython("original-package".into()),
        CapabilitySourceState::Ready,
    )]);
    let changed_availability = CapabilityReadSource::new(snapshot, availability, None);
    changed_availability.prepare();
    assert!(!original.same_owner(&changed_availability.prepared_head()));
    assert!(source.header().sources.is_empty());
    assert_eq!(changed_availability.header().sources.len(), 1);
    for (head, header) in [
        (original, source.header()),
        (image, changed.header()),
        (
            changed_availability.prepared_head(),
            changed_availability.header(),
        ),
    ] {
        compare(&header, &head, CHECKPOINT_TARGET_BYTES / 8);
        assert!(head.metadata_entries() <= 5 * CHECKPOINT_PAGE_ITEMS);
    }
}
