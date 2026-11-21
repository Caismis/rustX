//! Native START authorization and watch publication ordering regressions.
use super::*;

pub(crate) async fn watch_terminal_order() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let _execution = fixture.start_execution(0).await;
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let path = fixture.workspaces[0].join("watched");
    std::fs::write(&path, b"before").unwrap();
    let watch_id = "terminal-order";
    call(
        &connection,
        1,
        Method::FsWatch {
            session_id: fixture.sessions[0].id.clone(),
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
    assert_eq!(
        fixture.host.file_observers().available_permits(),
        31,
        "the host retains retirement ownership until the old UI watch ID can be released"
    );
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
            session_id: fixture.sessions[0].id.clone(),
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
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
}

pub(crate) async fn revocation_before_start() {
    struct ReleaseOnDrop(crate::app_server::command::StartHold);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    for start_first in [false, true] {
        for tty in [false, true] {
            for revocation in ["root", "connection", "host"] {
                let fixture = Fixture::with_session_count(None, 1).await;
                grant(&fixture);
                let marker = fixture.workspaces[0].join("business-started");
                let mut execution = fixture.start_execution(0).await;
                let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
                initialize(&connection).await;
                // The Server-side dispatch hold: revocation observed
                // while held fences the Start dispatch itself — the
                // Client-side physical gate has its own in-crate race.
                let hold = crate::app_server::command::hold_start("start-order");
                let _release_on_panic = ReleaseOnDrop(hold.clone());
                let mut params = parameters(
                    &fixture,
                    &[
                        "/bin/sh",
                        "-c",
                        "printf '%s\n' \"$$\" > business-started; printf READY; exec sleep 600",
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
                assert!(!marker.exists(), "the Start dispatch is held");
                if start_first {
                    hold.release();
                    super::review::through_marker(&connection, "start-order", b"READY").await;
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
                    Response::Failure(failure) => assert!(
                        matches!(
                            failure.error.data,
                            Some(ErrorData::ConnectionClosed | ErrorData::ExecutionUnavailable)
                        ),
                        "revocation={revocation}: honest failure, got {failure:?}"
                    ),
                }
                assert!(
                    marker.exists() == start_first,
                    "revocation={revocation}, tty={tty}, start_first={start_first}: only an authorized command can run"
                );

                let mut operations = fixture.host.request_operations();
                bounded(operations.wait_for(|count| *count == 0))
                    .await
                    .unwrap();
                assert_eq!(*operations.borrow(), 0);
                if revocation == "host" {
                    // The Host closes new dispatch before the physical
                    // receipt. A released request slot alone cannot prove
                    // that the Client has reaped the business process.
                    assert_eq!(
                        bounded(fixture.host.settle_execution()).await,
                        [] as [std::string::String; 0]
                    );
                }
                // The business process names its own pid before READY; `exec`
                // keeps that pid for the long-running program, so the physical
                // exit is observed on the exact business process after the
                // operation drained.
                if start_first {
                    let pid: i32 = std::fs::read_to_string(&marker)
                        .unwrap()
                        .trim()
                        .parse()
                        .expect("business pid");
                    process_gone(pid);
                }
                assert_eq!(fixture.manager.diagnostics().loaded, 0);
                connection.close();
                // Host admission closes synchronously; physical Client
                // revocation settles independently. Keep the real peer alive
                // until the Host has collected its receipt.
                assert_eq!(
                    bounded(fixture.host.drain()).await,
                    [] as [std::string::String; 0],
                    "revocation={revocation}, tty={tty}, start_first={start_first}"
                );
                execution.kill().await.unwrap();
            }
        }
    }
}

pub(crate) async fn watch_immediate_successor() {
    for retire_first in [false, true] {
        let fixture = Fixture::with_session_count(None, 1).await;
        grant(&fixture);
        let _execution = fixture.start_execution(0).await;
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let path = fixture.workspaces[0].join("watched-successor");
        std::fs::write(&path, b"before").unwrap();
        let watch_id = "reused-watch";
        call(
            &connection,
            1,
            Method::FsWatch {
                session_id: fixture.sessions[0].id.clone(),
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
                session_id: fixture.sessions[0].id.clone(),
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
        assert_eq!(
            bounded(fixture.host.drain()).await,
            [] as [std::string::String; 0]
        );
    }
}

/// A poisoned supervisor trace variable reaches the real staged Client only
/// through a test launcher; the Client must never create or append that path.
pub(crate) async fn poisoned_trace_variable_is_never_authority() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let scratch = tempfile::tempdir().unwrap();
    let poison = scratch.path().join("poisoned-trace");
    let client = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    let launcher = scratch.path().join("poisoned-launcher");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexport RUSTX_TEST_SUPERVISION_TRACE='{}'\nexec '{}' \"$@\"\n",
            poison.display(),
            client.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut execution = fixture.start_execution_via(0, Some(&launcher)).await;
    let connection = std::sync::Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let params = parameters(&fixture, &["/bin/true"]);
    let worker = connection.clone();
    let command = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    let MethodResult::CommandExec { result } = bounded(command).await.unwrap() else {
        panic!("the bound command completes through the real Client")
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.status, ExecStatus::Completed);
    assert!(
        !poison.exists(),
        "a poisoned trace path must never be created"
    );
    execution.kill().await.unwrap();
    connection.close();
}
