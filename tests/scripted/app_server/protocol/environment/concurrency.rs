//! Native START authorization and watch publication ordering regressions.
use super::*;

pub(crate) async fn watch_terminal_order() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let path = fixture.workspaces[0].join("watched");
    std::fs::write(&path, b"before").unwrap();
    let watch_id = "terminal-order";
    call(
        &connection,
        1,
        Method::FsWatch {
            watch_id: watch_id.into(),
            path: path.display().to_string(),
        },
    )
    .await;
    let (entered, release, completed) =
        crate::app_server::filesystem::pause_publication(path.clone());
    let queued = connection.watch_retirement_for_test(watch_id);
    std::fs::write(&path, b"changed").unwrap();
    bounded(entered).await.unwrap();
    // Identity validation has finished, and the callback holds no I/O or
    // publication lock. Complete root revocation before allowing it to emit.
    fixture
        .manager
        .bind_process_policy(crate::local_runtime::app_server_policy::AppServerPolicy::default());
    bounded(queued).await.unwrap();
    release.send(()).unwrap();
    bounded(completed).await.unwrap();
    connection.environment_barrier_for_test();
    let mut retirement = None;
    loop {
        let event = bounded(connection.environment_event_for_test()).await;
        assert!(
            !event
                .fence
                .as_ref()
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        );
        let NotificationMethod::FsChanged(change) = event.notification.notification else {
            panic!("filesystem event")
        };
        if change.watch_id == "publication-barrier" {
            let _ = event.emitted.send(());
            break;
        }
        assert_eq!(change.watch_id, watch_id);
        if change.retired {
            assert!(retirement.is_none(), "one terminal notice per incarnation");
            // Do not acknowledge retirement yet: the old implementation's
            // stop fence must not hide a callback queued after this notice.
            retirement = Some(event.emitted);
        } else {
            assert!(
                retirement.is_none(),
                "ordinary watch event followed its terminal notice"
            );
            let _ = event.emitted.send(());
        }
    }
    retirement
        .expect("automatic retirement was actually published")
        .send(())
        .unwrap();
    bounded(fixture.host.settle_file_observers()).await;
    assert_eq!(fixture.host.file_observers().available_permits(), 32);
    // The terminal incarnation releases its identity and native resources;
    // the same client can register and observe its successor normally.
    grant(&fixture);
    call(
        &connection,
        2,
        Method::FsWatch {
            watch_id: watch_id.into(),
            path: path.display().to_string(),
        },
    )
    .await;
    std::fs::write(&path, b"successor").unwrap();
    let change = loop {
        if let NotificationMethod::FsChanged(change) =
            bounded(connection.next_notification()).await.notification
        {
            break change;
        }
    };
    assert_eq!(change.watch_id, watch_id);
    assert!(!change.retired);
    call(
        &connection,
        3,
        Method::FsUnwatch {
            watch_id: watch_id.into(),
        },
    )
    .await;
    bounded(fixture.host.settle_file_observers()).await;
    assert_eq!(fixture.host.file_observers().available_permits(), 32);
    assert_eq!(fixture.manager.diagnostics().loaded, 0);
    connection.close();
    assert!(bounded(fixture.host.drain()).await.is_empty());
}

pub(crate) async fn revocation_before_start() {
    use crate::tools::native::bash_supervisor::diagnostics::{Entry, Event};
    struct ReleaseOnDrop(crate::runtime::process_runner::RunnerStartHold);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    for start_first in [false, true] {
        for tty in [false, true] {
            for revocation in ["root", "connection", "host"] {
                let fixture = Fixture::with_session_count(None, 0).await;
                grant(&fixture);
                let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
                initialize(&connection).await;
                let marker = fixture.workspaces[0].join("business-started");
                let trace = fixture.workspaces[0].join("native-trace");
                let (hold, lifecycle) = crate::app_server::command::hold_start(
                    fixture.workspaces[0].clone(),
                    trace.clone(),
                );
                let _release_on_panic = ReleaseOnDrop(hold.clone());
                let mut params = parameters(
                    &fixture,
                    &[
                        "/bin/sh",
                        "-c",
                        "printf started > business-started; printf READY; exec sleep 600",
                    ],
                );
                params.tty = tty;
                params.process_id = Some("start-order".into());
                params.stream_stdout_stderr = true;
                let worker = connection.clone();
                let command = tokio::spawn(async move {
                    worker
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(1),
                            call: Method::CommandExec(params),
                        })
                        .await
                });
                bounded(hold.await_entered()).await;
                assert!(!marker.exists(), "the actual native START gate is held");
                if start_first {
                    hold.release();
                    super::review::through_marker(&connection, "start-order", b"READY").await;
                    bounded(lifecycle.await_ownership_established()).await;
                    assert!(marker.exists(), "START won before revocation");
                }
                match revocation {
                    "root" => fixture.manager.bind_process_policy(
                        crate::local_runtime::app_server_policy::AppServerPolicy::default(),
                    ),
                    "connection" => connection.close(),
                    "host" => fixture.host.begin_drain(),
                    _ => unreachable!(),
                }
                hold.release();
                match bounded(command).await.unwrap() {
                    Response::Success(success) => assert!(
                        matches!(success.result, MethodResult::CommandExec { result } if result.status == ExecStatus::Terminated)
                    ),
                    Response::Failure(failure) => {
                        assert_eq!(failure.error.data, Some(ErrorData::ConnectionClosed));
                    }
                }
                bounded(lifecycle.await_direct_child_reaped()).await;
                assert!(
                    marker.exists() == start_first,
                    "revocation={revocation}, tty={tty}, start_first={start_first}: only an authorized command can run"
                );
                let evidence: Vec<Entry> = std::fs::read_to_string(trace)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(
                    evidence
                        .iter()
                        .any(|entry| matches!(entry.event, Event::ShellGroup { .. })),
                    start_first,
                    "revocation={revocation}, tty={tty}: native spawn matches the winning authorization"
                );
                for entry in evidence {
                    if let Event::ShellGroup { pid, .. } = entry.event {
                        process_gone(pid);
                    }
                }
                let mut operations = fixture.host.request_operations();
                bounded(operations.wait_for(|count| *count == 0))
                    .await
                    .unwrap();
                assert_eq!(*operations.borrow(), 0);
                assert_eq!(fixture.manager.diagnostics().loaded, 0);
                connection.close();
                assert!(bounded(fixture.host.drain()).await.is_empty());
            }
        }
    }
}

pub(crate) async fn watch_immediate_successor() {
    for retire_first in [false, true] {
        let fixture = Fixture::with_session_count(None, 0).await;
        grant(&fixture);
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let path = fixture.workspaces[0].join("watched-successor");
        std::fs::write(&path, b"before").unwrap();
        let watch_id = "reused-watch";
        call(
            &connection,
            1,
            Method::FsWatch {
                watch_id: watch_id.into(),
                path: path.display().to_string(),
            },
        )
        .await;
        let (entered, release, completed) =
            crate::app_server::filesystem::pause_publication(path.clone());
        let queued = retire_first.then(|| connection.watch_retirement_for_test(watch_id));
        std::fs::write(&path, b"changed").unwrap();
        bounded(entered).await.unwrap();
        if let Some(queued) = queued {
            fixture.manager.bind_process_policy(
                crate::local_runtime::app_server_policy::AppServerPolicy::default(),
            );
            bounded(queued).await.unwrap();
        }
        call(
            &connection,
            2,
            Method::FsUnwatch {
                watch_id: watch_id.into(),
            },
        )
        .await;
        grant(&fixture);
        // Reuse the identity before the old native callback or retirement owner
        // can finish. Neither is allowed to publish or remove this successor.
        call(
            &connection,
            3,
            Method::FsWatch {
                watch_id: watch_id.into(),
                path: path.display().to_string(),
            },
        )
        .await;
        release.send(()).unwrap();
        bounded(completed).await.unwrap();
        let capacity = fixture.host.file_observers();
        let old_returned = bounded(capacity.acquire_many(31)).await.unwrap();
        drop(old_returned);
        assert_eq!(capacity.available_permits(), 31);
        std::fs::write(&path, b"successor").unwrap();
        let change = loop {
            if let NotificationMethod::FsChanged(change) =
                bounded(connection.next_notification()).await.notification
            {
                break change;
            }
        };
        assert_eq!(change.watch_id, watch_id);
        assert!(!change.retired);
        call(
            &connection,
            4,
            Method::FsUnwatch {
                watch_id: watch_id.into(),
            },
        )
        .await;
        bounded(fixture.host.settle_file_observers()).await;
        assert_eq!(capacity.available_permits(), 32);
        assert_eq!(fixture.manager.diagnostics().loaded, 0);
        connection.close();
        assert!(bounded(fixture.host.drain()).await.is_empty());
    }
}
