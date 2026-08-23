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

/// One output reader task handle.
pub(super) type StreamHandle = tokio::task::JoinHandle<Result<(), String>>;

/// Streams one child pipe through an incremental UTF-8 decoder into its
/// bounded per-stream preview and the combined multiplex.
pub(super) async fn capture_stream<R>(
    mut pipe: R,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, String)>,
    stream_id: u8,
    name: &'static str,
    park: CapturePark,
) -> Result<(), String>
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
            Err(error) => return Err(format!("cannot read the {name} stream: {error}")),
        };
        let text = decoder.push(&buffer[..read]);
        if text.is_empty() {
            continue;
        }
        capture.lock().expect("preview lock").push(&text);
        combined_tx
            .send((stream_id, text))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
    }
    let tail = decoder.finish();
    if !tail.is_empty() {
        capture.lock().expect("preview lock").push(&tail);
        combined_tx
            .send((stream_id, tail))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
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
) -> Result<(), String> {
    while let Some((_stream_id, text)) = receiver.recv().await {
        capture
            .lock()
            .expect("combined capture lock")
            .push(&text, &store)?;
    }
    Ok(())
}

/// Consumes the combined multiplex of one background Bash invocation.
pub(super) async fn consume_background(
    mut receiver: tokio::sync::mpsc::Receiver<(u8, String)>,
    capture: Arc<Mutex<BackgroundOutputCapture>>,
) -> Result<(), String> {
    while let Some((_stream_id, text)) = receiver.recv().await {
        capture
            .lock()
            .expect("background capture lock")
            .push(&text)?;
    }
    Ok(())
}

/// Awaits every output reader task and the combined capture consumer.
pub(super) async fn await_drain(
    stdout_task: &mut Option<StreamHandle>,
    stderr_task: &mut Option<StreamHandle>,
    combined_task: &mut StreamHandle,
) -> Result<(), String> {
    await_handle(stdout_task).await?;
    await_handle(stderr_task).await?;
    combined_task
        .await
        .map_err(|join| format!("the combined output reader task failed: {join}"))?
}

async fn await_handle(handle: &mut Option<StreamHandle>) -> Result<(), String> {
    match handle {
        Some(handle) => handle
            .await
            .map_err(|join| format!("the output reader task failed: {join}"))?,
        None => Ok(()),
    }
}
