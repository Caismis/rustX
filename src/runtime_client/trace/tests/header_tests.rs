//! Root Trace headers use the same immutable cut in live and cold readers.
use super::*;
use crate::message::content::FileReference;
use crate::observation::bounded::decode_domain_value;
use crate::runtime_client::host::RuntimeClientHost;
use base64::Engine as _;
use std::sync::Arc;

#[test]
fn deferred_trace_headers_are_readable_by_exact_journal_sequence() {
    let (store, early, gap) = adopted_files();
    let history = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let live = host.checkpoint().unwrap().read_cut;
    let root = host
        .inner
        .checkpoint_page(&live, "trace.records", 0, 32)
        .unwrap();
    let item = root
        .items
        .iter()
        .find(|item| item.deferred)
        .expect("large native adoption header");
    assert!(item.value.is_none());
    let sequence = item.offset;
    assert_eq!(item.identity, format!("trace:{sequence}"));
    let cold = crate::runtime_client::trace_observation::history_page(
        store.as_ref(),
        &history,
        "trace.records",
        0,
        32,
    )
    .unwrap();
    assert_eq!(cold.items, root.items);
    let older = crate::runtime_client::trace_observation::history_page(
        store.as_ref(),
        &history,
        "trace.records",
        sequence,
        32,
    )
    .unwrap();
    assert!(older.items.iter().all(|item| item.offset < sequence));

    let mut values = Vec::new();
    for cut in [&live, &history] {
        let bytes = header_bytes(&host, store.as_ref(), cut, sequence, cut == &live);
        let record: TraceRecord =
            decode_domain_value(serde_json::from_slice(&bytes).unwrap()).unwrap();
        assert_eq!(record.id, item.identity);
        assert_eq!(record.kind, TraceKind::User);
        assert_eq!(record.attachments.len(), 32);
        values.push(bytes);
    }
    assert_eq!(values[0], values[1]);
    let read = |cut: &crate::observation::cut::CheckpointReadCut, sequence, path: &[String]| {
        crate::runtime_client::trace_observation::history_value_read(
            store.as_ref(),
            cut,
            "trace.records",
            sequence,
            path,
            (0, 65_536),
        )
    };
    assert!(
        read(&early, sequence, &[]).is_err(),
        "later adoption is absent at the earlier cut"
    );
    assert!(!root.items.iter().any(|item| item.offset == gap));
    assert!(
        read(&history, gap, &[]).is_err(),
        "Journal sequence is never a collection rank"
    );
    assert!(read(&history, sequence, &["preview".into()]).is_err());
    let mut foreign = history.clone();
    foreign.conversation_id = ConversationId::new("conv_19000000-0000-7000-8000-000000000210");
    assert!(read(&foreign, sequence, &[]).is_err());
    append_in_step(store.as_ref(), E::TurnStarted, 10, "2");
    let after = read(&history, sequence, &[]).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(after.bytes_base64)
            .unwrap(),
        values[0]
    );
    assert!(
        host.inner
            .checkpoint_value_read(&foreign, "trace.records", sequence, &[], 0, 1024)
            .is_err()
    );
}

fn header_bytes(
    host: &RuntimeClientHost,
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    sequence: u64,
    live: bool,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = if live {
            host.inner
                .checkpoint_value_read(cut, "trace.records", sequence, &[], offset, 1024)
        } else {
            crate::runtime_client::trace_observation::history_value_read(
                store,
                cut,
                "trace.records",
                sequence,
                &[],
                (offset, 1024),
            )
        }
        .unwrap();
        assert_eq!(&chunk.cut, cut);
        assert_eq!(chunk.item_offset, sequence);
        assert_eq!(chunk.domain, "trace.records");
        assert_eq!(chunk.offset, offset);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 1024);
        bytes.extend(part);
        let Some(next) = chunk.next_offset else {
            assert_eq!(chunk.readable_end, bytes.len() as u64);
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    bytes
}

fn adopted_files() -> (
    Arc<SqliteConversationStore>,
    crate::observation::cut::CheckpointReadCut,
    u64,
) {
    let store = Arc::new(store("conv_19000000-0000-7000-8000-000000000209"));
    start(store.as_ref());
    let early = TraceProjection::new(store.as_ref())
        .unwrap()
        .history_cut()
        .unwrap();
    // Each accepted message has a small canonical header. The real native
    // adoption aggregates 32 file headers into one Trace record larger than
    // the 16 KiB checkpoint-item budget, but below the Trace record budget.
    for message in 0..4 {
        store
            .accept_inbound(crate::durable::InboundDraft {
                message_id: Some(MessageId::new(format!("user-{message}"))),
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: (0..8)
                    .map(|file| {
                        UserContentBlock::File(FileReference {
                            artifact_id: ArtifactId::new(format!("file-{message}-{file}")),
                            name: Some("n".repeat(512)),
                            mime_type: Some("application/pdf".into()),
                            description: None,
                        })
                    })
                    .collect(),
                timestamp: timestamp(message),
                correlation: None,
            })
            .unwrap();
    }
    let batch = store.select_pending_batch().unwrap().unwrap();
    store
        .adopt_pending_batch(batch.watermark, Some(AttemptId::new("attempt-a")))
        .unwrap();
    append(store.as_ref(), E::TurnCompleted, 9);
    let gap = store.presentation_frontier().unwrap();
    (store, early, gap)
}
