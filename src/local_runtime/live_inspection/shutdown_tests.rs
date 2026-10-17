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
