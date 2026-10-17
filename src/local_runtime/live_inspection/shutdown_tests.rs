use super::*;
use std::sync::Arc;

#[tokio::test]
async fn failed_listener_join_remains_explicit_on_every_shutdown_wait() {
    for cancelled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unproven-inspection-route");
        std::fs::write(&path, b"original routing marker").unwrap();
        let task = if cancelled {
            let task =
                tokio::spawn(std::future::pending::<Result<(), InspectionShutdownFailure>>());
            task.abort();
            task
        } else {
            tokio::spawn(async { panic!("listener failure proof") })
        };
        let (_, workset) = tokio::sync::watch::channel(0);
        let mut server = LiveConversationInspectionServer {
            path: path.clone(),
            stop: tokio_util::sync::CancellationToken::new(),
            task: Some(task),
            shutdown_failure: None,
            workset,
        };
        for _ in 0..2 {
            let error = server.shutdown().await.unwrap_err();
            assert!(matches!(
                (cancelled, error),
                (true, InspectionShutdownFailure::Cancelled)
                    | (false, InspectionShutdownFailure::Panicked)
            ));
            assert!(server.task.is_none());
            assert!(path.exists(), "failed join is not physical release proof");
        }
    }
}

#[tokio::test]
async fn failed_connection_is_reported_only_after_survivors_join_and_persists() {
    for cancelled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("failed-connection-route");
        std::fs::write(&path, b"original routing marker").unwrap();
        let mut connections = JoinSet::new();
        if cancelled {
            connections.spawn(std::future::pending::<()>()).abort();
        } else {
            connections.spawn(async { panic!("connection failure proof") });
        }
        let (release, released) = tokio::sync::oneshot::channel();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let survivor_finished = Arc::clone(&finished);
        connections.spawn(async move {
            released.await.unwrap();
            survivor_finished.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let (failed, failure_recorded) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut failure = None;
            // The survivor is explicitly held, so this actual join result
            // necessarily belongs to the failed connection.
            record_connection_failure(&mut failure, connections.join_next().await.unwrap());
            assert!(failure.is_some());
            failed.send(()).unwrap();
            drain_connections(&mut connections, &mut failure).await;
            failure.map_or(Ok(()), Err)
        });
        let (_, workset) = tokio::sync::watch::channel(0);
        let mut server = LiveConversationInspectionServer {
            path: path.clone(),
            stop: tokio_util::sync::CancellationToken::new(),
            task: Some(task),
            shutdown_failure: None,
            workset,
        };
        failure_recorded.await.unwrap();
        {
            let mut shutdown = Box::pin(server.shutdown());
            assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        }
        assert!(
            server.task.is_some(),
            "cancelled waiter retains the original join"
        );
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        release.send(()).unwrap();
        for _ in 0..2 {
            let error = server.shutdown().await.unwrap_err();
            assert!(matches!(
                (cancelled, error),
                (true, InspectionShutdownFailure::ConnectionCancelled)
                    | (false, InspectionShutdownFailure::ConnectionPanicked)
            ));
            assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
            assert!(server.task.is_none());
            assert!(path.exists(), "connection failure is not release proof");
        }
    }
}

/// Shutdown's first poll is the output linearization point: every connection's
/// socket I/O stops then, before the listener task next runs, so a Source
/// request completing after shutdown began cannot deliver its response. Bytes
/// the socket accepted earlier were already delivered and stay valid.
#[tokio::test(flavor = "current_thread")]
async fn shutdown_stops_connection_io_before_the_listener_runs() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("linearized-inspection-route");
    std::fs::write(&path, b"routing marker").unwrap();
    // A listener that cannot run before the assertions below.
    let (_, workset) = tokio::sync::watch::channel(0);
    let mut server = LiveConversationInspectionServer {
        path,
        stop: tokio_util::sync::CancellationToken::new(),
        task: Some(tokio::spawn(std::future::pending())),
        shutdown_failure: None,
        workset,
    };
    let (client, socket) = tokio::io::duplex(64);
    let (socket_reader, socket_writer) = tokio::io::split(socket);
    let mut reader = InspectionIo::new(socket_reader, server.stop.clone());
    let mut writer = InspectionIo::new(socket_writer, server.stop.clone());
    let (mut client_reader, mut client_writer) = tokio::io::split(client);
    writer
        .write_all(b"accepted before shutdown\n")
        .await
        .unwrap();
    {
        let mut shutdown = Box::pin(server.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
    }
    let refused = writer.write_all(b"completed after shutdown began\n").await;
    assert_eq!(refused.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    client_writer.write_all(b"late request\n").await.unwrap();
    let mut read = [0u8; 16];
    assert_eq!(
        reader.read(&mut read).await.unwrap(),
        0,
        "input ends at shutdown"
    );
    drop((reader, writer));
    let mut delivered = Vec::new();
    client_reader.read_to_end(&mut delivered).await.unwrap();
    assert_eq!(delivered, b"accepted before shutdown\n");
    server.task.take().unwrap().abort();
}
