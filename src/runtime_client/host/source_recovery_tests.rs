// Explicit checkpoint repair stays on the existing cancellable Source worker.
use super::*;

pub(super) struct OccupancySourceFault {
    connection: rusqlite::Connection,
    original: Option<Vec<u8>>,
}
impl OccupancySourceFault {
    pub(super) fn new(fixture: &HostFixture) -> Self {
        let connection =
            rusqlite::Connection::open(fixture.directory.path().join("conversation.sqlite"))
                .expect("the original native Store");
        let original: Option<Vec<u8>> = connection
            .query_row(
                "SELECT occupancy_observation FROM bootstrap_identity WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            original.is_some(),
            "the original finite occupancy Source is present"
        );
        assert_eq!(
            connection
                .execute(
                    "UPDATE bootstrap_identity SET occupancy_observation=NULL WHERE id=1",
                    [],
                )
                .unwrap(),
            1
        );
        Self {
            connection,
            original,
        }
    }
    pub(super) fn repair(&mut self) {
        assert_eq!(
            self.connection
                .execute(
                    "UPDATE bootstrap_identity SET occupancy_observation=?1 WHERE id=1",
                    [self.original.as_deref()],
                )
                .unwrap(),
            1
        );
    }
}
impl Drop for OccupancySourceFault {
    fn drop(&mut self) {
        self.repair();
    }
}

fn assert_original_source_unchanged(
    fixture: &HostFixture,
    native: &crate::durable::observation::ObservationReadCut,
    original_journal: &[RuntimeEventEnvelope],
) {
    let store = fixture.store.as_ref().unwrap();
    assert_eq!(&store.observation_read_cut().unwrap(), native);
    assert_eq!(
        store.read_events(None, 32).unwrap().events,
        original_journal
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_checkpoint_repairs_a_real_source_fault_on_the_original_worker_without_native_work()
 {
    let (model, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    fixture.host.projection_settled().await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let before = fixture.host.checkpoint().unwrap();
    let subscriber = attachment.subscribe_events(before.cursor).unwrap();
    let native = fixture
        .store
        .as_ref()
        .unwrap()
        .observation_read_cut()
        .unwrap();
    let original_journal = fixture
        .store
        .as_ref()
        .unwrap()
        .read_events(None, 32)
        .unwrap()
        .events;
    let mut fault = OccupancySourceFault::new(&fixture);
    let inner = &fixture.host.inner;
    inner
        .lock_published_state()
        .projection
        .request_read_domains_for_test();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    assert!(matches!(
        subscriber.next().await,
        EventDelivery::Unavailable { .. }
    ));
    assert!(matches!(
        fixture.host.checkpoint(),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert!(matches!(
        attachment.subscribe_events(before.cursor),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    {
        let state = inner.lock_published_state();
        assert_eq!(state.projection.cursor(), before.cursor);
        assert!(
            state.projection.read_domain_cut().is_none(),
            "a failed explicit read does not busy-retry"
        );
    }
    fault.repair();
    // This is the public API, not the cfg install helper. It wakes and joins
    // one bounded read completion at the original failed dependency fence.
    let repaired = fixture
        .host
        .checkpoint()
        .expect("explicit same-source checkpoint recovery");
    assert_eq!(repaired.cursor.get(), before.cursor.get() + 1);
    assert_eq!(repaired.read_cut.source_cut, before.read_cut.source_cut);
    assert_original_source_unchanged(&fixture, &native, &original_journal);
    assert!(model.requests().is_empty());
    let EventDelivery::Event(event) = subscriber.try_next() else {
        panic!("one original-worker repair transition")
    };
    assert_eq!(event.cursor, repaired.cursor);
    assert!(
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
    );
    assert!(matches!(subscriber.try_next(), EventDelivery::Pending));
    assert_eq!(fixture.host.checkpoint().unwrap(), repaired);
    let renewed = attachment.subscribe_events(repaired.cursor).unwrap();
    assert!(matches!(renewed.try_next(), EventDelivery::Pending));
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[cfg(unix)]
async fn unix_observation_request(
    reader: &mut tokio::io::BufReader<tokio::net::UnixStream>,
    request: crate::runtime_client::RuntimeClientRequest,
) -> crate::runtime_client::RuntimeClientResponse {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let mut bytes = serde_json::to_vec(&request).unwrap();
    bytes.push(b'\n');
    reader.get_mut().write_all(&bytes).await.unwrap();
    let mut record = String::new();
    assert!(reader.read_line(&mut record).await.unwrap() > 0);
    assert!(record.len() <= crate::runtime_client::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES);
    serde_json::from_str(&record).unwrap()
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn unix_checkpoint_source_repair_keeps_the_current_thread_executor_serviceable() {
    use crate::runtime_client::{RequestId, RuntimeClientRequest};
    let (model, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    fixture.host.projection_settled().await;
    let before = fixture.host.checkpoint().unwrap();
    let mut fault = OccupancySourceFault::new(&fixture);
    fixture
        .host
        .fail_projection_read_for_test("held original Source fault");
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let endpoint = crate::runtime_client::RuntimeClientEndpoint::new_read_only(&fixture.host);
    let (input, output) = server.into_split();
    let serving = tokio::spawn(
        crate::runtime_client::transport::stdio::serve_stdio_jsonl_with_io(endpoint, input, output),
    );
    let mut reader = tokio::io::BufReader::new(client);
    let initialized = unix_observation_request(
        &mut reader,
        RuntimeClientRequest::Initialize {
            id: RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    )
    .await;
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized {
            read_only: true,
            ..
        })
    ));
    let unavailable = unix_observation_request(
        &mut reader,
        RuntimeClientRequest::CheckpointGet {
            id: RequestId::new(2),
        },
    )
    .await;
    assert!(
        matches!(unavailable.error, Some(RuntimeClientError::ObservationUnavailable { ref message }) if message.contains("occupancy")),
        "actual Source failure, not an executor deadline: {unavailable:?}"
    );
    assert_eq!(
        fixture
            .host
            .inner
            .lock_published_state()
            .projection
            .cursor(),
        before.cursor
    );
    fault.repair();
    let response = unix_observation_request(
        &mut reader,
        RuntimeClientRequest::CheckpointGet {
            id: RequestId::new(3),
        },
    )
    .await;
    let Some(RuntimeClientResult::Checkpoint { checkpoint }) = response.result else {
        panic!("public Unix checkpoint recovery: {response:?}")
    };
    assert_eq!(checkpoint.cursor.get(), before.cursor.get() + 1);
    assert_eq!(checkpoint.read_cut.source_cut, before.read_cut.source_cut);
    assert!(
        fixture
            .store
            .as_ref()
            .unwrap()
            .read_events(None, 32)
            .unwrap()
            .events
            .is_empty()
    );
    assert!(model.requests().is_empty());
    drop(reader);
    assert!(matches!(
        serving.await.unwrap(),
        Ok(crate::runtime_client::transport::stdio::StdioSessionEnd::InputEof)
    ));
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}

/// An explicit checkpoint that requests a Source repair returns only after the
/// repair's publication is installed. The worker folds the repaired read under
/// the state lock, then encodes and installs its publication outside it; a
/// checkpoint that found the domains clean inside that window used to return
/// the previous published cursor (CI run 39032890413).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_source_repair_waits_for_its_publication_install() {
    #[derive(Debug)]
    enum Step {
        Settling,
        Returned(Result<u64, String>),
    }
    let (_model, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    fixture.host.projection_settled().await;
    let before = fixture.host.checkpoint().unwrap();
    fixture
        .host
        .fail_projection_read_for_test("held original Source fault");
    let gate = &fixture.host.inner.publication_gate;
    let (steps, step) = std::sync::mpsc::channel();
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    // Hold the repair's publication between its fold and its install.
    *gate.encoding_hook.lock().unwrap() = Some(Box::new(move || {
        entered_tx.send(()).unwrap();
        released.recv().unwrap();
    }));
    // Decide completion only once the fold is held inside that window.
    *gate.repair_request_hook.lock().unwrap() = Some(Box::new(move || {
        entered.recv().unwrap();
    }));
    let settling = steps.clone();
    *gate.repair_settle_hook.lock().unwrap() = Some(Box::new(move || {
        settling.send(Step::Settling).unwrap();
    }));
    let host = fixture.host.clone();
    let checkpoint = std::thread::spawn(move || {
        let result = host
            .checkpoint()
            .map(|checkpoint| checkpoint.cursor.get())
            .map_err(|error| format!("{error:?}"));
        steps.send(Step::Returned(result)).unwrap();
    });
    let guard = std::time::Duration::from_secs(30);
    let first = step.recv_timeout(guard).unwrap();
    assert!(
        matches!(first, Step::Settling),
        "the checkpoint returned while its repair publication was held: {first:?}"
    );
    release.send(()).unwrap();
    let Step::Returned(cursor) = step.recv_timeout(guard).unwrap() else {
        panic!("one settlement step only");
    };
    assert_eq!(cursor, Ok(before.cursor.get() + 1));
    checkpoint.join().unwrap();
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}
