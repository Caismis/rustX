use super::*;
use crate::observation::bounded::prepare_capture_item;
use crate::observation::capture::Capture;
use crate::observation::trace::{
    TraceFieldKind, TraceFieldSource, TraceKind, TraceLocation, TracePreview, TraceRecord,
    TraceState, TraceTiming,
};

fn record() -> TraceRecord {
    TraceRecord {
        fields: vec![TraceFieldSource::Value {
            field: TraceFieldKind::Name,
            domain: "journal.7".into(),
            item_offset: 0,
            path: vec!["event".into(), "name".into()],
            encoded_bytes: 17,
        }],
        collections: vec![CollectionFrontier {
            domain: "messages.ledger:2.blocks".into(),
            revision: 7,
            total: 2,
            loaded: 0,
            next_offset: Some(0),
        }],
        id: "trace:7".into(),
        position: serde_json::from_str("\"trace:7\"").unwrap(),
        location: TraceLocation::default(),
        kind: TraceKind::Tool,
        state: TraceState::Incomplete,
        timing: TraceTiming {
            started_at: chrono::DateTime::UNIX_EPOCH,
            ended_at: None,
            duration_ms: None,
        },
        preview: Some(TracePreview {
            text: "水🙂\\\"\n".into(),
            truncated: false,
        }),
        request: None,
        tool: None,
        calls: Vec::new(),
        native_id: None,
        agent_id: None,
        activation_id: None,
        activation_origin: None,
        originating_tool_call_id: None,
        message_id: None,
        attachments: Vec::new(),
        has_detail: true,
        truncated: false,
    }
}

fn compare(records: &[TraceRecord], prepared: &PreparedTraceHead, budget: usize) {
    let mut old = Capture::new(41);
    let mut actual = Capture::new(41);
    old.remaining = budget;
    actual.remaining = budget;
    assert_eq!(
        prepared.capture(records, &mut actual),
        old.page("trace.records", records),
        "budget {budget}"
    );
    assert_eq!(actual.remaining, old.remaining, "budget {budget}");
    assert_eq!(actual.evicted, old.evicted, "budget {budget}");
    assert_eq!(actual.collections, old.collections, "budget {budget}");
    assert_eq!(actual.content, old.content);
}

#[test]
fn prepared_trace_matches_original_capture_at_every_remaining_item_budget() {
    let records = vec![record()];
    let prepared = PreparedTraceHead::default();
    assert!(!prepared.prepared());
    for budget in 0..=CHECKPOINT_ITEM_BYTES {
        compare(&records, &prepared, budget);
    }
    assert!(prepared.prepared());
    let cloned = prepared.clone();
    assert!(prepared.same_owner(&cloned));
    assert!(cloned.prepared());
}

#[test]
fn prepared_trace_preserves_original_count_and_invalid_head_frontiers() {
    let mut large = record();
    large.preview.as_mut().unwrap().text = "original".repeat(1000);
    let mut giant = record();
    giant.id = "x".repeat(CHECKPOINT_ITEM_BYTES);
    let mut wide = record();
    wide.fields = vec![record().fields.remove(0); CHECKPOINT_PAGE_ITEMS + 1];
    let cases = [
        Vec::new(),
        vec![large],
        vec![record(), giant, record()],
        vec![record(), wide, record()],
        vec![record(); CHECKPOINT_PAGE_ITEMS + 1],
    ];
    for records in cases {
        let prepared = PreparedTraceHead::default();
        let mut budgets = vec![0, 1, CHECKPOINT_ITEM_BYTES, CHECKPOINT_TARGET_BYTES / 8];
        let mut charged = 0;
        for item in records.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some((_, bytes, floor)) = prepare_capture_item(item) else {
                break;
            };
            for boundary in [charged + bytes, charged + floor] {
                budgets.extend([boundary.saturating_sub(1), boundary, boundary + 1]);
            }
            charged += bytes;
        }
        for budget in budgets {
            compare(&records, &prepared, budget);
        }
    }
}

#[test]
fn prepared_trace_keeps_original_dto_normalization_before_cloning() {
    use crate::observation::trace::{
        TraceGeneration, TraceRequestPredecessor, TraceRequestSummary,
        TraceSystemPromptPresentation, TraceSystemPromptState, TraceToolCatalogState,
    };
    let mut row = record();
    row.request = Some(TraceRequestSummary {
        accepted_message_locator: None,
        request_id: None,
        request_id_source: None,
        retry_number: 0,
        assistant_message_id: None,
        assistant_message_id_source: None,
        model: None,
        model_source: None,
        previous_failure_kind: None,
        failure_kind: None,
        usage: None,
        generation: Some(TraceGeneration {
            timeline: None,
            ttft_ms: None,
            generation_ms: None,
            terminal_ms: 7,
            output_tokens_per_second: Some(f64::NAN),
        }),
        system_prompt: TraceSystemPromptPresentation {
            state: TraceSystemPromptState::Initial,
            preview: None,
        },
        predecessor: TraceRequestPredecessor::NotApplicable,
        tool_catalog: TraceToolCatalogState::Initial,
        context_additions: Vec::new(),
        context_truncated: false,
        context_total: 0,
        context_domain: "trace.records.7.context".into(),
        context_next_offset: None,
    });
    let rows = vec![row];
    let prepared = PreparedTraceHead::default();
    for budget in [0, 1024, CHECKPOINT_ITEM_BYTES] {
        compare(&rows, &prepared, budget);
    }
    let mut capture = Capture::new(9);
    let admitted = prepared.capture(&rows, &mut capture);
    assert_eq!(
        admitted[0]
            .request
            .as_ref()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .output_tokens_per_second,
        None,
        "first preparation performs the original DTO JSON normalization"
    );
}
