//! The listening record is readiness; diagnostics are not. One buffered reader
//! owns stderr before and after startup, including bytes read ahead with it.
use std::{
    collections::VecDeque, future::Future, io, net::SocketAddr, process::ExitStatus, time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr},
    time::Instant,
};

const PREFIX: &str = "rustx app-server listening ";
const MAX_LINE: usize = 512;
const MAX_DIAGNOSTICS: usize = 8;

async fn record(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::other("EOF inside startup record"))
            };
        }
        let length = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |index| index + 1);
        if line.len() + length > MAX_LINE {
            return Err(io::Error::other("startup line exceeds 512 bytes"));
        }
        line.extend_from_slice(&buffer[..length]);
        reader.consume(length);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line).map(Some).map_err(io::Error::other);
        }
    }
}

async fn listening(
    reader: &mut (impl AsyncBufRead + Unpin),
    exited: impl Future<Output = io::Result<ExitStatus>>,
    deadline: Instant,
) -> io::Result<String> {
    let mut diagnostics = VecDeque::new();
    let outcome = {
        let records = async {
            loop {
                let line = record(reader)
                    .await?
                    .ok_or_else(|| io::Error::other("EOF before listening record"))?;
                if let Some(endpoint) = line.strip_prefix(PREFIX) {
                    let address = endpoint
                        .strip_prefix("ws://")
                        .and_then(|value| value.parse::<SocketAddr>().ok())
                        .filter(|address| address.port() != 0)
                        .ok_or_else(|| {
                            io::Error::other(format!("malformed listening endpoint: {endpoint}"))
                        })?;
                    return Ok(format!("ws://{address}"));
                }
                if line.starts_with("rustx app-server listening") {
                    return Err(io::Error::other(format!(
                        "malformed listening record: {line}"
                    )));
                }
                if diagnostics.len() == MAX_DIAGNOSTICS {
                    diagnostics.pop_front();
                }
                diagnostics.push_back(line);
            }
        };
        tokio::select! {
            biased;
            status = exited => Err(io::Error::other(format!("child exited before readiness: {}", status?))),
            result = tokio::time::timeout_at(deadline, records) => result.unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "startup deadline exceeded"))),
        }
    };
    outcome.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "{error}; recent stderr: {}",
                diagnostics.into_iter().collect::<Vec<_>>().join("\n")
            ),
        )
    })
}

pub(super) async fn websocket_startup(child: &mut Child) -> (String, BufReader<ChildStderr>) {
    let mut stderr = BufReader::new(child.stderr.take().expect("owned stderr"));
    // Child::wait closes its stored stdin. Keep that pipe with its existing
    // owner while using exit as a competing readiness event.
    let stdin = child.stdin.take();
    let result = listening(
        &mut stderr,
        child.wait(),
        Instant::now() + Duration::from_secs(60),
    )
    .await;
    child.stdin = stdin;
    (result.expect("App Server startup"), stderr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn first_record_and_batched_diagnostics_preserve_read_ahead() {
        for prefix in ["", "diagnostic one\ndiagnostic two\n"] {
            let bytes = format!("{prefix}{PREFIX}ws://127.0.0.1:1234\r\nafter startup\n");
            let mut reader = BufReader::new(bytes.as_bytes());
            assert_eq!(
                listening(
                    &mut reader,
                    std::future::pending(),
                    Instant::now() + Duration::from_secs(1)
                )
                .await
                .unwrap(),
                "ws://127.0.0.1:1234"
            );
            assert_eq!(record(&mut reader).await.unwrap().unwrap(), "after startup");
        }
    }

    #[tokio::test]
    async fn fragmented_records_require_a_complete_line() {
        let (mut writer, reader) = tokio::io::duplex(512);
        let mut reader = BufReader::new(reader);
        let mut ready = Box::pin(listening(
            &mut reader,
            std::future::pending(),
            Instant::now() + Duration::from_secs(1),
        ));
        for fragment in [
            "diagnostic\nru",
            "stx app-server listening ws://",
            "[::1]:1234",
            "\r",
        ] {
            writer.write_all(fragment.as_bytes()).await.unwrap();
            assert!(futures_util::poll!(&mut ready).is_pending());
        }
        writer.write_all(b"\n").await.unwrap();
        assert_eq!(ready.await.unwrap(), "ws://[::1]:1234");
    }

    #[tokio::test]
    async fn eof_malformed_endpoints_and_oversized_records_fail_boundedly() {
        for input in [
            "diagnostic\n".to_owned(),
            "partial".into(),
            format!("{PREFIX}http://127.0.0.1:12\n"),
            format!("{PREFIX}ws://127.0.0.1:0\n"),
            format!("{PREFIX}ws://127.0.0.1:65536\n"),
            format!("{PREFIX}ws://127.0.0.1:12/extra\n"),
            format!("{PREFIX}ws://127.0.0.1:12 unexpected\n"),
            "rustx app-server listening\n".into(),
            "x".repeat(MAX_LINE + 1),
        ] {
            let mut reader = BufReader::new(input.as_bytes());
            let error = listening(
                &mut reader,
                std::future::pending(),
                Instant::now() + Duration::from_secs(1),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().len() < 1024, "{error}");
        }
    }

    #[tokio::test]
    async fn child_exit_ends_startup_even_while_stderr_remains_open() {
        let (_writer, reader) = tokio::io::duplex(512);
        let mut reader = BufReader::new(reader);
        let (exit, exited) = tokio::sync::oneshot::channel();
        let mut ready = Box::pin(listening(
            &mut reader,
            async { Ok(exited.await.unwrap()) },
            Instant::now() + Duration::from_secs(60),
        ));
        assert!(futures_util::poll!(&mut ready).is_pending());
        exit.send(ExitStatus::from_raw(7 << 8)).unwrap();
        let error = ready.await.unwrap_err().to_string();
        assert!(error.contains("child exited before readiness"), "{error}");
        assert!(error.contains('7'), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn diagnostics_do_not_reset_deadline_and_retention_is_bounded() {
        let (mut writer, reader) = tokio::io::duplex(512);
        let mut reader = BufReader::new(reader);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = Box::pin(listening(&mut reader, std::future::pending(), deadline));
        for index in 0..12 {
            writer
                .write_all(format!("diagnostic-{index}\n").as_bytes())
                .await
                .unwrap();
            assert!(futures_util::poll!(&mut ready).is_pending());
        }
        for _ in 0..2 {
            tokio::time::advance(Duration::from_secs(4)).await;
            writer.write_all(b"still starting\n").await.unwrap();
            assert!(futures_util::poll!(&mut ready).is_pending());
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        let error = ready.await.unwrap_err();
        assert_eq!(Instant::now(), deadline);
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(!error.to_string().contains("diagnostic-0\n"));
        assert!(error.to_string().contains("diagnostic-11"));
        assert_eq!(error.to_string().matches("diagnostic-").count(), 6);
    }
}
