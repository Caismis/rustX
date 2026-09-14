//! App Server JSONL; unrelated to the legacy Runtime Client endpoint.
use super::{MAX_MESSAGE_BYTES, WRITE_TIMEOUT, failure};
use crate::app_server::connection::AppServerConnection;
use std::{io, sync::Arc};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Read at most the named payload limit, without allocating an oversized record.
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
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| failure("JSONL record is not UTF-8"));
        }
    }
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
        connection,
        incoming,
        |mut receiver| async move {
            while let Some(record) = receiver.recv().await {
                tokio::time::timeout(WRITE_TIMEOUT, async {
                    writer.write_all(record.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await
                })
                .await
                .map_err(|_| failure("JSONL write deadline exceeded"))??;
            }
            Ok(())
        },
        shutdown,
    )
    .await;
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
