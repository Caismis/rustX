use super::*;
use crate::runtime_client::host::{ClientInner, commit_display};
// Isolated reconstruction from published source; no original unpushed work recovered.

use crate::runtime_client::checkpoint::{CheckpointPage, RuntimeClientCheckpoint};
use base64::Engine as _;

async fn completed_trace_fixture() -> (HostFixture, RuntimeClientCheckpoint, CheckpointPage, String)
{
    let (_, fixture) = host_fixture(
        vec![one_turn_stop(), one_turn_stop()],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let original = "original held Trace content ".repeat(700);
    assert!(original.len() > crate::runtime_client::checkpoint::CHECKPOINT_ITEM_BYTES);
    {
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content(&original))
            .unwrap();
        tokio::time::timeout(STREAM_LIVENESS_GUARD, settled)
            .await
            .unwrap();
    }
    fixture.host.projection_settled().await;
    let checkpoint = fixture.host.checkpoint().unwrap();
    let record = checkpoint
        .snapshot
        .trace
        .records
        .iter()
        .find(|record| record.kind == crate::runtime_client::trace::TraceKind::Request)
        .expect("actual native request record");
    let RuntimeClientResult::DomainPage { page } =
        fixture.host.inner.trace_detail(record.id.clone()).unwrap()
    else {
        panic!("native immutable detail page");
    };
    (fixture, checkpoint, page, original)
}

fn original_content_reference(
    checkpoint: &RuntimeClientCheckpoint,
    original: &str,
) -> crate::runtime_client::content::ContentRef {
    checkpoint
        .content
        .iter()
        .chain(
            checkpoint
                .snapshot
                .message_descriptors
                .iter()
                .flat_map(|header| &header.content),
        )
        .chain(checkpoint.snapshot.transcript.content.iter())
        .find(|binding| {
            binding.content_ref.representation
                == crate::runtime_client::content::ContentRepresentation::Utf8Text
                && binding.content_ref.readable_end == original.len() as u64
        })
        .expect("advertised original canonical content reader")
        .content_ref
        .clone()
}

fn assert_original_content(
    inner: &ClientInner,
    reference: &crate::runtime_client::content::ContentRef,
    original: &str,
) {
    let chunk = inner.content_read(reference, 0, 65_536).unwrap();
    assert_eq!(&chunk.content_ref, reference);
    assert!(chunk.next_offset.is_none());
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        original.as_bytes()
    );
}

fn hold_admitted_detail(
    inner: &Arc<ClientInner>,
    page: &CheckpointPage,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    tokio::task::JoinHandle<Result<CheckpointPage, RuntimeClientError>>,
) {
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    *inner.checkpoint_page_admitted_hook.lock().unwrap() = Some(Box::new(move || {
        let _ = entered.send(());
        let _ = release_rx.recv();
    }));
    let reader = Arc::clone(inner);
    let page = page.clone();
    let pending = tokio::task::spawn_blocking(move || {
        reader.checkpoint_page(&page.read_cut, &page.domain, 0, 32)
    });
    (entered_rx, release, pending)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deferred_trace_detail_and_content_keep_original_cut_across_native_commit_shutdown_and_reattach()
 {
    let (fixture, checkpoint, original_page, original) = completed_trace_fixture().await;
    let reference = original_content_reference(&checkpoint, &original);
    let inner = Arc::clone(&fixture.host.inner);
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let (entered, release, pending) = hold_admitted_detail(&inner, &original_page);
    tokio::time::timeout(STREAM_LIVENESS_GUARD, entered)
        .await
        .unwrap()
        .unwrap();
    let later_attempt = AttemptId::new("later-held-detail-attempt");
    let committed = inner
        .store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new("later-held-detail-start"),
            sequence: 0,
            conversation_id: inner.conversation_id.clone(),
            attempt_id: Some(later_attempt.clone()),
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: later_attempt,
            },
        })
        .unwrap();
    let sink = super::super::NativePublicationProjection {
        inner: Arc::downgrade(&inner),
    };
    assert!(
        crate::runtime::observation::PublicationProjectionCommit::native_batch(
            &sink,
            committed.observation.as_deref().unwrap(),
        )
        .is_installed()
    );
    inner
        .store
        .mark_observation_direct(committed.sequence, committed.sequence)
        .unwrap();
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
    attachment.detach();
    let (reattached, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let current = reattached.checkpoint().unwrap();
    assert!(current.snapshot.shutting_down);
    assert!(current.read_cut.through > original_page.read_cut.through);
    release.send(()).unwrap();
    let returned = pending.await.unwrap().unwrap();
    assert_eq!(returned, original_page);
    assert_original_content(&inner, &reference, &original);
    assert_eq!(
        inner
            .checkpoint_page(&returned.read_cut, &returned.domain, 0, 32)
            .unwrap(),
        returned
    );
    reattached.detach();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_detail_does_not_bypass_a_later_pending_mutation_epoch() {
    let (fixture, checkpoint, original_page, original) = completed_trace_fixture().await;
    let reference = original_content_reference(&checkpoint, &original);
    let inner = Arc::clone(&fixture.host.inner);
    let (entered, release, pending) = hold_admitted_detail(&inner, &original_page);
    tokio::time::timeout(STREAM_LIVENESS_GUARD, entered)
        .await
        .unwrap()
        .unwrap();
    {
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content("later native pending mutation"))
            .unwrap();
        tokio::time::timeout(STREAM_LIVENESS_GUARD, settled)
            .await
            .unwrap();
    }
    fixture.host.projection_settled().await;
    let current = fixture.host.checkpoint().unwrap();
    assert!(
        current
            .read_cut
            .source_cut
            .as_ref()
            .unwrap()
            .read
            .mutation_revision
            > original_page
                .read_cut
                .source_cut
                .as_ref()
                .unwrap()
                .read
                .mutation_revision
    );
    release.send(()).unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap_err(),
        RuntimeClientError::ReadCutExpired
    );
    let fresh = inner
        .checkpoint_page(&current.read_cut, &original_page.domain, 0, 32)
        .unwrap();
    assert_eq!(fresh.items, original_page.items);
    assert_original_content(&inner, &reference, &original);
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

mod scale {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn admitted_immutable_trace_detail_survives_4097_publications_after_public_receipt_expires()
     {
        let (fixture, checkpoint, original_page, original) = completed_trace_fixture().await;
        let reference = original_content_reference(&checkpoint, &original);
        let inner = Arc::clone(&fixture.host.inner);
        let (entered, release, pending) = hold_admitted_detail(&inner, &original_page);
        tokio::time::timeout(STREAM_LIVENESS_GUARD, entered)
            .await
            .unwrap()
            .unwrap();
        let before = inner.lock_published_state().projection.cursor();
        for _ in 0..4_097 {
            commit_display(&inner.state, &inner.publication_gate, |projection| {
                projection.apply(ConversationObservation::Shutdown);
            });
        }
        {
            let state = inner.lock_published_state();
            assert_eq!(state.projection.cursor().get(), before.get() + 4_097);
            assert!(matches!(
                state
                    .projection
                    .validate_checkpoint_cut(&original_page.read_cut),
                Err(RuntimeClientError::ReadCutExpired)
            ));
        }
        assert!(matches!(
            inner.checkpoint_page(&original_page.read_cut, &original_page.domain, 0, 32),
            Err(RuntimeClientError::ReadCutExpired)
        ));
        release.send(()).unwrap();
        assert_eq!(pending.await.unwrap().unwrap(), original_page);
        assert_original_content(&inner, &reference, &original);
        fixture.runtime.shutdown().await.unwrap();
        inner.drain_projection().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_only_page_fault_defers_to_explicit_reads_and_same_cut_retry_succeeds() {
    let (fixture, _, _, _) = completed_trace_fixture().await;
    let inner = Arc::clone(&fixture.host.inner);
    let before = inner
        .lock_published_state()
        .projection
        .checkpoint_read_cut();
    let mut fault = TraceIndexFault::new(&fixture);
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
        let _ = entered.send(());
        Ok(())
    }));
    inner
        .lock_state()
        .projection
        .request_read_domains_for_test();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    tokio::time::timeout(STREAM_LIVENESS_GUARD, entered_rx)
        .await
        .unwrap()
        .unwrap();
    fixture.host.projection_settled().await;
    {
        let state = inner.lock_published_state();
        state.projection.validate_published_source().unwrap();
        assert!(state.projection.read_domain_cut().is_none());
    }
    let expected = RuntimeClientError::ObservationUnavailable {
        message: "native durable observation source is unavailable".into(),
    };
    assert_eq!(inner.checkpoint().unwrap_err(), expected);
    for _ in 0..2 {
        assert_eq!(
            inner.trace_page(None, 32, Vec::new()).unwrap_err(),
            expected
        );
    }
    assert_eq!(
        inner
            .lock_published_state()
            .projection
            .checkpoint_read_cut()
            .source_cut,
        before.source_cut
    );
    fault.repair();
    let repaired = inner.checkpoint().unwrap();
    let RuntimeClientResult::TracePage { page } = inner.trace_page(None, 32, Vec::new()).unwrap()
    else {
        panic!("same-cut explicit retry");
    };
    assert_eq!(repaired.read_cut.source_cut, before.source_cut);
    assert_eq!(page.read_cut.as_ref(), Some(&repaired.read_cut));
    assert!(!page.records.is_empty());
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

struct TraceIndexFault {
    connection: rusqlite::Connection,
    sequence: i64,
    original: Option<rusqlite::types::Value>,
}

impl TraceIndexFault {
    fn new(fixture: &HostFixture) -> Self {
        let connection =
            rusqlite::Connection::open(fixture.directory.path().join("conversation.sqlite"))
                .unwrap();
        let (sequence, original): (i64, rusqlite::types::Value) = connection.query_row(
            "SELECT sequence,observation_trace_fact FROM events WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NOT NULL ORDER BY sequence LIMIT 1",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(
            connection
                .execute(
                    "UPDATE events SET observation_trace_fact=NULL WHERE sequence=?1",
                    [sequence]
                )
                .unwrap(),
            1
        );
        Self {
            connection,
            sequence,
            original: Some(original),
        }
    }

    fn repair(&mut self) {
        if let Some(original) = self.original.take() {
            assert_eq!(
                self.connection
                    .execute(
                        "UPDATE events SET observation_trace_fact=?1 WHERE sequence=?2",
                        rusqlite::params![original, self.sequence]
                    )
                    .unwrap(),
                1
            );
        }
    }
}

impl Drop for TraceIndexFault {
    fn drop(&mut self) {
        self.repair();
    }
}
