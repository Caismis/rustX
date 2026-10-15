//! App Server JSONL, distinct from the internal Runtime Client JSONL/Unix protocol.
use super::{MAX_MESSAGE_BYTES, WRITE_TIMEOUT, failure};
use crate::app_server::connection::AppServerConnection;
use std::{io, pin::Pin, sync::Arc};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Read at most the payload limit and request no buffer capacity above it.
/// Allocator size-class or page rounding is outside this retention bound.
async fn record(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(failure("EOF inside JSONL record"))
            };
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let count = end.unwrap_or(available.len());
        if count > MAX_MESSAGE_BYTES - bytes.len() {
            return Err(failure("JSONL record exceeds limit"));
        }
        let required = bytes.len() + count;
        if required > bytes.capacity() {
            let capacity = bytes
                .capacity()
                .max(8)
                .saturating_mul(2)
                .max(required)
                .min(MAX_MESSAGE_BYTES);
            bytes.reserve_exact(capacity - bytes.len());
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| failure("JSONL record is not UTF-8"));
        }
    }
}

/// Finish one accepted record irrespective of subsequent registration changes.
/// The writer must perform physical acceptance within `poll_write`; Pending
/// must not dispatch a deferred write. Production uses reactor-owned pipe
/// Senders/UnixStreams, never blocking-file or stdout worker adapters.
pub(crate) async fn write_outbound<W: AsyncWrite + Unpin>(
    writer: &mut W,
    outbound: &mut super::Outbound,
) -> io::Result<()> {
    // The first write that takes bytes is the handoff; while
    // the pipe is full it takes none and nothing is decided.
    let (record, written) = std::future::poll_fn(|cx| {
        if outbound.discarded(cx) {
            return std::task::Poll::Ready(Ok((String::new(), None)));
        }
        outbound.poll_hand_off(|record| {
            Pin::new(&mut *writer)
                .poll_write(cx, record.as_bytes())
                .map(|written| match written {
                    Ok(0) => Err(io::ErrorKind::WriteZero.into()),
                    other => other,
                })
        })
    })
    .await?;
    let Some(written) = written else {
        return Ok(());
    };
    writer.write_all(&record.as_bytes()[written..]).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// Serve one pipe connection. EOF and broken pipe detach, without runtime actions.
/// # Errors
/// Invalid framing, exceeded bounds, or failed I/O terminate the connection.
pub async fn serve<R, W>(
    connection: Arc<AppServerConnection>,
    reader: R,
    mut writer: W,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin,
{
    let incoming =
        futures_util::stream::try_unfold(BufReader::new(reader), |mut reader| async move {
            Ok(record(&mut reader).await?.map(|record| (record, reader)))
        });
    let result = super::serve(
        connection.clone(),
        incoming,
        |mut receiver| async move {
            while let Some(mut outbound) = receiver.next().await {
                tokio::time::timeout(WRITE_TIMEOUT, write_outbound(&mut writer, &mut outbound))
                    .await
                    .map_err(|_| failure("JSONL write deadline exceeded"))??;
            }
            Ok(())
        },
        shutdown,
    )
    .await;
    if result.is_err() {
        connection.transport_failure();
    }
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn t01_t20_fragmented_input_reservations_stay_within_the_record_budget() {
        use tokio::io::AsyncReadExt as _;

        let mut input = vec![b' '; MAX_MESSAGE_BYTES];
        input.extend_from_slice(b"\n{}\n");
        for first_read in [5_001, 8_191] {
            // The non-power-of-two first read followed by ordinary 8KiB
            // buffers formerly caused amortized Vec growth above the limit.
            let (first, rest) = input.split_at(first_read);
            let mut reader = BufReader::new(first.chain(rest));
            let captured = record(&mut reader).await.unwrap().unwrap();
            assert_eq!(captured.as_bytes(), &input[..MAX_MESSAGE_BYTES]);
            assert!(captured.capacity() <= MAX_MESSAGE_BYTES);
            assert_eq!(record(&mut reader).await.unwrap().as_deref(), Some("{}"));
            assert_eq!(record(&mut reader).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn exact_framing_boundaries() {
        for size in [0, MAX_MESSAGE_BYTES - 1, MAX_MESSAGE_BYTES] {
            let mut input = vec![b' '; size];
            input.push(b'\n');
            assert_eq!(
                record(&mut input.as_slice()).await.unwrap().unwrap().len(),
                size
            );
        }
        for input in [
            vec![b' '; MAX_MESSAGE_BYTES + 1],
            vec![0xff, b'\n'],
            b"{}".to_vec(),
        ] {
            assert!(record(&mut input.as_slice()).await.is_err());
        }
        assert_eq!(record(&mut &b""[..]).await.unwrap(), None);
        let mut input = &b"{}\r\n[]\n"[..];
        assert_eq!(record(&mut input).await.unwrap().unwrap(), "{}\r");
        assert_eq!(record(&mut input).await.unwrap().unwrap(), "[]");
    }
}
