//! Bash process-stream capture.
//!
//! Process ownership remains in `runtime::process_runner`. The Tool Plane
//! output module owns the shared preview/spill/sink policy; this module only
//! decodes Bash pipes, multiplexes them, and waits for capture tasks to settle.

use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use super::text::IncrementalUtf8Decoder;
use crate::tools::managed_output::ManagedToolOutput;
pub(super) use crate::tools::output::{
    BackgroundOutputCapture, ForegroundOutputCapture as SpillCapture,
    TextPreviewCapture as PreviewCapture,
};

/// The test-only seam that holds one output reader task open after EOF until
/// the bounded settlement path force-finalizes it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHold {
    parked_tx: tokio::sync::watch::Sender<bool>,
    parked_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl CaptureHold {
    pub(super) fn new() -> Self {
        let (parked_tx, parked_rx) = tokio::sync::watch::channel(false);
        Self {
            parked_tx,
            parked_rx,
        }
    }

    /// Returns the reader-side handle.
    pub(super) fn reader(&self) -> CaptureHoldReader {
        CaptureHoldReader {
            parked: self.parked_tx.clone(),
        }
    }

    /// Waits until the reader has provably parked after EOF.
    pub(super) async fn await_parked(&self) {
        let mut receiver = self.parked_rx.clone();
        if !*receiver.borrow() {
            let _ = receiver.changed().await;
        }
    }
}

/// Reader-side capture-hold handle.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHoldReader {
    parked: tokio::sync::watch::Sender<bool>,
}

/// Test-only reader parking seam.
#[cfg(test)]
pub(super) type CapturePark = Option<CaptureHoldReader>;
#[cfg(not(test))]
pub(super) type CapturePark = Option<std::convert::Infallible>;

/// Test-only observation seam for committed background appends.
#[cfg(test)]
pub(super) type AppendWatch = Option<tokio::sync::watch::Sender<u64>>;
#[cfg(not(test))]
pub(super) type AppendWatch = Option<std::convert::Infallible>;

/// A Bash-local capture settlement failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BashProcessControlError {
    /// Capture did not settle within the bounded confirmation window.
    CaptureTimeout,
}

impl core::fmt::Display for BashProcessControlError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CaptureTimeout => write!(
                formatter,
                "the bash output capture did not settle within the bounded confirmation window"
            ),
        }
    }
}

/// Failures retain their owning boundary until every capture task settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CaptureFailure {
    Read(String),
    Combined(String),
    /// A downstream symptom: the combined consumer stopped accepting chunks.
    ConsumerClosed(&'static str),
    Task(String),
}

impl core::fmt::Display for CaptureFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Read(detail) | Self::Combined(detail) | Self::Task(detail) => {
                formatter.write_str(detail)
            }
            Self::ConsumerClosed(stream) => {
                write!(formatter, "the combined {stream} capture is unavailable")
            }
        }
    }
}

/// One reader or combined-consumer task handle.
pub(super) type StreamHandle = tokio::task::JoinHandle<Result<(), CaptureFailure>>;

/// Streams one child pipe through an incremental UTF-8 decoder into its
/// bounded per-stream preview and the combined multiplex.
pub(super) async fn capture_stream<R>(
    mut pipe: R,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, String)>,
    stream_id: u8,
    name: &'static str,
    park: CapturePark,
) -> Result<(), CaptureFailure>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(not(test))]
    let _ = park;
    let mut decoder = IncrementalUtf8Decoder::default();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return Err(CaptureFailure::Read(format!(
                    "cannot read the {name} stream: {error}"
                )));
            }
        };
        let text = decoder.push(&buffer[..read]);
        if text.is_empty() {
            continue;
        }
        capture.lock().expect("preview lock").push(&text);
        combined_tx
            .send((stream_id, text))
            .await
            .map_err(|_| CaptureFailure::ConsumerClosed(name))?;
    }
    let tail = decoder.finish();
    if !tail.is_empty() {
        capture.lock().expect("preview lock").push(&tail);
        combined_tx
            .send((stream_id, tail))
            .await
            .map_err(|_| CaptureFailure::ConsumerClosed(name))?;
    }
    drop(combined_tx);
    #[cfg(test)]
    if let Some(park) = park {
        park.parked.send(true).ok();
        std::future::pending::<()>().await;
    }
    Ok(())
}

/// Consumes the combined multiplex of one foreground Bash invocation.
pub(super) async fn consume_combined(
    mut receiver: tokio::sync::mpsc::Receiver<(u8, String)>,
    store: ManagedToolOutput,
    capture: Arc<Mutex<SpillCapture>>,
) -> Result<(), CaptureFailure> {
    while let Some((_stream_id, text)) = receiver.recv().await {
        capture
            .lock()
            .expect("combined capture lock")
            .push(&text, &store)
            .map_err(CaptureFailure::Combined)?;
    }
    Ok(())
}

/// Consumes the combined multiplex of one background Bash invocation.
pub(super) async fn consume_background(
    mut receiver: tokio::sync::mpsc::Receiver<(u8, String)>,
    capture: Arc<Mutex<BackgroundOutputCapture>>,
) -> Result<(), CaptureFailure> {
    while let Some((_stream_id, text)) = receiver.recv().await {
        capture
            .lock()
            .expect("background capture lock")
            .push(&text)
            .map_err(CaptureFailure::Combined)?;
    }
    Ok(())
}

/// Awaits every output reader task and the combined capture consumer.
pub(super) async fn await_drain(
    stdout_task: &mut Option<StreamHandle>,
    stderr_task: &mut Option<StreamHandle>,
    combined_task: &mut Option<StreamHandle>,
) -> Result<(), String> {
    // A failed reader does not release ownership of its siblings. Join all
    // three before returning any error or publishing terminal output.
    let stdout = await_handle(stdout_task).await;
    let stderr = await_handle(stderr_task).await;
    let combined = await_handle(combined_task).await;
    // Semantic owner order, not completion/join order: combined storage
    // first, then independent stdout/stderr failures. Preserve all genuine
    // failures; a closed consumer is useful only when no owner reported why
    // it stopped. In particular it can never mask a native storage error.
    let failures: Vec<_> = [combined, stdout, stderr]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    let has_cause = failures
        .iter()
        .any(|failure| !matches!(failure, CaptureFailure::ConsumerClosed(_)));
    let diagnostics: Vec<_> = failures
        .into_iter()
        .filter(|failure| !has_cause || !matches!(failure, CaptureFailure::ConsumerClosed(_)))
        .map(|failure| failure.to_string())
        .collect();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics.join("; "))
    }
}

async fn await_handle(handle: &mut Option<StreamHandle>) -> Result<(), CaptureFailure> {
    let result = match handle {
        Some(handle) => handle
            .await
            .map_err(|join| CaptureFailure::Task(format!("the output reader task failed: {join}")))
            .and_then(std::convert::identity),
        None => return Ok(()),
    };
    // A timeout may resume draining. Never poll a joined task twice.
    handle.take();
    result
}

#[cfg(test)]
mod settlement_tests {
    use super::*;
    use crate::runtime::identity::ConversationId;

    /// The first chunk reaches the real storage owner. Only after its
    /// failure has returned (and dropped the receiver) may the real reader
    /// send another chunk. No process, pipe capacity or scheduling decides
    /// the interleaving. Allocation and write failures share this path.
    #[tokio::test]
    async fn combined_storage_failure_precedes_later_reader_channel_closure() {
        for write_failure in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = ManagedToolOutput::new(
                ConversationId::new("conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38"),
                directory.path().join("tool-output"),
            )
            .unwrap();
            let results = store.root().join("results");
            if write_failure {
                store.fail_writes_after(0);
            } else {
                std::fs::remove_dir(&results).unwrap();
                std::fs::write(&results, "not a directory").unwrap();
            }
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let (failed, failure) = tokio::sync::oneshot::channel();
            let (release_combined, wait_combined) = tokio::sync::oneshot::channel();
            let mut combined = Some(tokio::spawn(async move {
                let result =
                    consume_combined(rx, store, Arc::new(Mutex::new(SpillCapture::with_limit(1))))
                        .await;
                // consume_combined has returned its native error and dropped rx.
                failed.send(result.clone()).unwrap();
                wait_combined.await.unwrap();
                result
            }));
            let (observed, observation) = tokio::sync::oneshot::channel();
            let mut stdout = Some(tokio::spawn(async move {
                tx.send((0, "first chunk".into())).await.unwrap();
                let cause = failure.await.unwrap().unwrap_err();
                assert!(matches!(cause, CaptureFailure::Combined(_)));
                let result = capture_stream(
                    &b"later chunk"[..],
                    Arc::new(Mutex::new(PreviewCapture::new(128))),
                    tx,
                    0,
                    "stdout",
                    None,
                )
                .await;
                assert_eq!(result, Err(CaptureFailure::ConsumerClosed("stdout")));
                observed.send(cause.to_string()).unwrap();
                result
            }));
            let (release_stderr, wait_stderr) = tokio::sync::oneshot::channel();
            let mut stderr = Some(tokio::spawn(async move {
                wait_stderr.await.unwrap();
                Ok(())
            }));
            let cause = observation.await.unwrap();
            if write_failure {
                assert!(cause.contains("cannot write the foreground result spill"));
                assert!(cause.contains("test-forced output write failure"));
            } else {
                assert!(cause.contains("cannot allocate the foreground result spill"));
                assert!(cause.contains(&results.display().to_string()));
            }
            {
                let drain = await_drain(&mut stdout, &mut stderr, &mut combined);
                tokio::pin!(drain);
                assert!(futures_util::poll!(&mut drain).is_pending());
                release_stderr.send(()).unwrap();
                assert!(futures_util::poll!(&mut drain).is_pending());
                release_combined.send(()).unwrap();
                assert_eq!(drain.await, Err(cause));
            }
            assert!(stdout.is_none() && stderr.is_none() && combined.is_none());
        }
    }

    #[tokio::test]
    async fn independent_reader_failures_are_preserved_in_owner_order() {
        for combined_error in [false, true] {
            let mut stdout = Some(tokio::spawn(async {
                Err(CaptureFailure::Read("stdout read failed".into()))
            }));
            let mut stderr = Some(tokio::spawn(async { Ok(()) }));
            let mut combined = Some(tokio::spawn(async move {
                if combined_error {
                    Err(CaptureFailure::Combined("native storage failure".into()))
                } else {
                    Ok(())
                }
            }));
            let expected = if combined_error {
                "native storage failure; stdout read failed"
            } else {
                "stdout read failed"
            };
            assert_eq!(
                await_drain(&mut stdout, &mut stderr, &mut combined).await,
                Err(expected.into())
            );
            assert!(stdout.is_none() && stderr.is_none() && combined.is_none());
        }
    }

    #[tokio::test]
    async fn unexplained_consumer_closure_remains_a_failure() {
        let mut stdout = Some(tokio::spawn(async {
            Err(CaptureFailure::ConsumerClosed("stdout"))
        }));
        assert_eq!(
            await_drain(&mut stdout, &mut None, &mut None).await,
            Err("the combined stdout capture is unavailable".into())
        );
        assert!(stdout.is_none());
    }

    /// A reader panic or abort is a joined failure; siblings must still
    /// finish before the caller can publish terminal output.
    #[tokio::test]
    async fn issue206_reader_task_failure_drains_every_sibling() {
        for abort in [false, true] {
            let failed = tokio::spawn(async move {
                if abort {
                    std::future::pending::<()>().await;
                }
                panic!("scripted capture panic");
                #[allow(unreachable_code)]
                Ok(())
            });
            if abort {
                failed.abort();
            }
            let mut stdout = Some(failed);
            let (release, wait) = tokio::sync::oneshot::channel();
            let mut stderr = Some(tokio::spawn(async move {
                wait.await.expect("release reader");
                Ok(())
            }));
            let (release_combined, wait_combined) = tokio::sync::oneshot::channel();
            let mut combined = Some(tokio::spawn(async move {
                wait_combined.await.expect("release combined capture");
                Ok(())
            }));
            {
                let drain = await_drain(&mut stdout, &mut stderr, &mut combined);
                tokio::pin!(drain);
                assert!(futures_util::poll!(&mut drain).is_pending());
                release.send(()).expect("reader still owned");
                assert!(futures_util::poll!(&mut drain).is_pending());
                release_combined
                    .send(())
                    .expect("combined capture still owned");
                assert!(drain.await.is_err());
            }
            assert!(stdout.is_none() && stderr.is_none() && combined.is_none());
        }
    }
}
