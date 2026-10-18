use super::*;

fn bounded_tail(store: &dyn ConversationStore) -> CompletedResponseView {
    let cut = store.observation_read_cut().unwrap();
    let mut page = super::super::super::snapshot::transcript_header_view(
        store.transcript_header_page(&cut, None, 32).unwrap(),
    );
    decorate_window(store, &mut page, cut.read.journal).unwrap();
    let tails = tails(&page);
    assert_eq!(tails.len(), 1);
    tails[0].clone()
}

#[test]
fn opening_adoption_retry_is_identical_in_live_wire_and_fork_lineage_after_steering() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    admitted_user(&store, "opening-input", "original-attempt");
    request(&store, "original-attempt", 0, Some(usage(None)));
    assistant(&store, "original-attempt", "intermediate-response");
    let steering = store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: Some(MessageId::new("later-steering")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Later steering question".into(),
            })],
            timestamp: Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    store
        .adopt_pending_batch(steering.sequence, Some(AttemptId::new("original-attempt")))
        .unwrap();
    request(&store, "original-attempt", 1, Some(usage(None)));
    assistant(&store, "original-attempt", "closing-response");
    finish(&store, "original-attempt");
    let live = bounded_tail(&store);
    let wire = crate::runtime_client::checkpoint::structural_value(&live).unwrap();
    assert_eq!(wire["retry_message_id"], "opening-input");
    assert_eq!(
        wire["retry_locator"],
        serde_json::json!({"type":"by_ledger_position","position":1})
    );
    assert_eq!(live.retry_message_id, Some(MessageId::new("opening-input")));
    let cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(cut.completed_responses.len(), 1);
    assert_eq!(
        cut.completed_responses[0].retry_message_id,
        live.retry_message_id
    );
    let fork_id = ConversationId::generate();
    let seed = crate::local_runtime::session::lineage_cut(
        &fork_id,
        &cut,
        None,
        crate::local_runtime::session::LineageSide::After,
    )
    .unwrap();
    let fork = SqliteConversationStore::in_memory(fork_id).unwrap();
    fork.initialize_lineage(&seed).unwrap();
    let inherited = bounded_tail(&fork);
    let inherited_wire = crate::runtime_client::checkpoint::structural_value(&inherited).unwrap();
    assert_eq!(inherited_wire["retry_locator"], wire["retry_locator"]);
    assert_eq!(inherited.origin, live.origin);
    let locator = inherited.retry_locator.unwrap();
    let crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position } =
        locator
    else {
        panic!("original numeric boundary")
    };
    let headers = fork
        .message_header_page(&fork.observation_read_cut().unwrap(), Some(position - 1), 1)
        .unwrap();
    assert_eq!(headers.headers.len(), 1);
    assert_eq!(headers.headers[0].ledger_position, Some(position));
    assert_eq!(
        headers.headers[0].opening_preview.as_deref(),
        Some("opening-input")
    );
    assert_eq!(
        fork.presentation_frontier().unwrap(),
        0,
        "Fork never executes its origin Attempt"
    );
    assert_eq!(store.load_canonical().unwrap().len(), 4);
}
