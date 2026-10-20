use super::*;
use crate::observation::bounded::decode_domain_value;
use crate::observation::cut::CheckpointReadCut;
use crate::observation::trace::{TraceKind, TraceState};
use crate::runtime_client::trace::{TraceInspectionHead, TraceProjection};

fn compact_with_trace(
    store: &SqliteConversationStore,
    id: &str,
    text: &str,
) -> (u64, ProjectionCommitReceipt) {
    let input = commit_input(
        store,
        id,
        candidate(store, id, vec![summary(text)]),
        &CancellationSignal::new(),
    );
    let started = store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(format!("trace-start-{id}")),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: input.operation_id.clone(),
            },
        })
        .unwrap();
    (started.sequence, store.commit_projection(input).unwrap())
}
fn inspected_compaction(
    store: &SqliteConversationStore,
    cut: &CheckpointReadCut,
    start: u64,
) -> TraceInspectionHead {
    let page = crate::runtime_client::trace_observation::history_detail(
        store,
        cut,
        &format!("trace:{start}"),
    )
    .unwrap();
    assert_eq!(&page.read_cut, cut);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].body_complete, Some(false));
    decode_domain_value(page.items[0].value.clone().unwrap()).unwrap()
}
fn deny_checkpoint_payload(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "projection_checkpoints",
            column_name: "candidate_json" | "occupancy_json",
        }
        | AuthAction::Read {
            table_name: "projection_activations",
            column_name: "activation_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}
#[test]
fn t08_t14_t19_compaction_trace_preview_stays_with_original_completed_j_after_new_activation() {
    let store = store();
    let canonical = vec![message(1)];
    store.initialize(&canonical).unwrap();
    let original = "# First original summary\n\nKeep  exact spacing.\n";
    let (first_start, first) = compact_with_trace(&store, "preview-first", original);
    let first_cut = TraceProjection::new(&store).unwrap().history_cut().unwrap();
    let old = inspected_compaction(&store, &first_cut, first_start);
    assert_eq!(old.kind, TraceKind::Compaction);
    assert_eq!(old.record.state, TraceState::Completed);
    assert_eq!(old.record.preview.as_ref().unwrap().text, original);
    assert!(!old.record.preview.as_ref().unwrap().truncated);
    let (second_start, second) =
        compact_with_trace(&store, "preview-second", "second original summary");
    assert_ne!(
        first.checkpoint.activation.activation,
        second.checkpoint.activation.activation
    );
    let current_cut = TraceProjection::new(&store).unwrap().history_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_checkpoint_payload))
        .unwrap();
    let current_first = inspected_compaction(&store, &current_cut, first_start);
    let current_second = inspected_compaction(&store, &current_cut, second_start);
    assert_eq!(current_first.record.preview, old.record.preview);
    assert_eq!(
        inspected_compaction(&store, &first_cut, first_start).record,
        old.record
    );
    assert_eq!(
        current_second.record.preview.as_ref().unwrap().text,
        "second original summary"
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    assert_eq!(
        store.load_canonical().unwrap(),
        canonical,
        "no canonical summary Message"
    );
}
#[test]
fn t08_t14_compaction_trace_preview_preserves_original_utf8_bound_and_truncation() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let original = "# Long original\n\n水🙂  exact spacing\n".repeat(400);
    let (started, receipt) = compact_with_trace(&store, "preview-long", &original);
    let cut = TraceProjection::new(&store).unwrap().history_cut().unwrap();
    let head = inspected_compaction(&store, &cut, started);
    let preview = head.record.preview.as_ref().unwrap();
    assert!(preview.truncated && head.record.truncated);
    assert!(!preview.text.is_empty() && preview.text.len() <= 512);
    assert!(original.starts_with(&preview.text));
    assert!(original.is_char_boundary(preview.text.len()));
    let batch = receipt.completed.observation.as_ref().unwrap();
    let crate::durable::observation::NativeObservationTransition::Single(witness) =
        &batch.transition
    else {
        panic!("original completion");
    };
    let expected = witness
        .occupancy
        .as_ref()
        .unwrap()
        .projection_head
        .summary_preview
        .as_ref()
        .unwrap();
    assert_eq!(preview.text, expected.text);
    assert_eq!(preview.truncated, expected.truncated);
    assert_eq!(
        store
            .projection_checkpoint(&receipt.checkpoint.candidate.checkpoint_id)
            .unwrap(),
        receipt.checkpoint
    );
}
