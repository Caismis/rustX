//! Real regressions for credential projection, terminal signals and watch rechecks.
use super::*;

async fn input(connection: &AppServerConnection, id: &str, bytes: &[u8]) {
    call(
        connection,
        2,
        Method::CommandWrite(WriteParams {
            process_id: id.into(),
            delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            close_stdin: false,
        }),
    )
    .await;
}

pub(super) async fn through_marker(
    connection: &AppServerConnection,
    id: &str,
    marker: &[u8],
) -> Vec<u8> {
    bounded(async {
        let mut bytes = Vec::new();
        loop {
            bytes.extend(output(connection, id).await);
            if bytes.windows(marker.len()).any(|part| part == marker) {
                return bytes;
            }
        }
    })
    .await
}

async fn signal_marker(
    connection: &AppServerConnection,
    id: &str,
    marker: &[u8],
    command: &mut tokio::task::JoinHandle<MethodResult>,
) {
    let mut bytes = Vec::new();
    bounded(async {
        loop {
            tokio::select! {
                chunk = output(connection, id) => {
                    bytes.extend(chunk);
                    if bytes.windows(marker.len()).any(|part| part == marker) {
                        return;
                    }
                },
                result = &mut *command => panic!("command ended before terminal marker {marker:?}: {result:?}; observed {bytes:?}"),
            }
        }
    })
    .await;
}

async fn completion(
    connection: &AppServerConnection,
    command: tokio::task::JoinHandle<MethodResult>,
) -> crate::app_server::command::ExecResult {
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::pin!(command);
        loop {
            tokio::select! {
                response = &mut command => break response.unwrap(),
                _ = connection.next_notification() => {},
            }
        }
    })
    .await
    .expect("physical terminality must settle before the 10-second command deadline");
    let MethodResult::CommandExec { result } = response else {
        panic!("command result")
    };
    result
}

pub(crate) async fn terminal_signals() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let id = "terminal-signal";
    let script = "stty -echo; trap 'printf INT' INT; trap 'printf QUIT' QUIT; printf READY; while :; do if IFS= read -r line; then if [ \"$line\" = done ]; then exit 0; fi; printf 'NEXT:%s' \"$line\"; fi; done";
    let mut params = parameters(&fixture, &["/bin/bash", "-c", script]);
    params.process_id = Some(id.into());
    params.tty = true;
    let worker = connection.clone();
    let mut command =
        tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    signal_marker(&connection, id, b"READY", &mut command).await;
    for (signal, marker) in [(3, b"INT".as_slice()), (28, b"QUIT".as_slice())] {
        // Bash defers a trap received just before read(2) until that read
        // completes. The empty canonical line wakes that exact pre-read race;
        // it neither supplies the trap marker nor substitutes for later input.
        input(&connection, id, &[signal, b'\n']).await;
        signal_marker(&connection, id, marker, &mut command).await;
        input(&connection, id, b"alive\n").await;
        signal_marker(&connection, id, b"NEXT:alive", &mut command).await;
    }
    input(&connection, id, b"done\n").await;
    let result = completion(&connection, command).await;
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.exit_code, Some(0));

    // Cat itself echoes readiness, after exec has restored default signals.
    for signal in [3, 28] {
        let mut params = parameters(
            &fixture,
            &["/bin/sh", "-c", "ulimit -c 0; stty -echo; exec /bin/cat -n"],
        );
        params.process_id = Some(id.into());
        params.tty = true;
        let worker = connection.clone();
        let mut command =
            tokio::spawn(async move { call(&worker, 3, Method::CommandExec(params)).await });
        connection.command_admitted_for_test(id).await;
        input(&connection, id, b"READY\n").await;
        signal_marker(&connection, id, b"1\tREADY", &mut command).await;
        input(&connection, id, &[signal]).await;
        let result = completion(&connection, command).await;
        assert_eq!(result.status, ExecStatus::Completed);
        assert_eq!(
            result.exit_code, None,
            "default terminal signal kills the business program"
        );
    }

    // The inner owner is deliberately killed after a client-established gate.
    // The outer must contain the group and report a control failure promptly.
    let mut params = parameters(
        &fixture,
        &[
            "/bin/sh",
            "-c",
            "stty -echo; printf READY; read line; kill -KILL \"$PPID\"; exec /bin/cat",
        ],
    );
    params.process_id = Some(id.into());
    params.tty = true;
    let worker = connection.clone();
    let mut command =
        tokio::spawn(async move { call(&worker, 4, Method::CommandExec(params)).await });
    signal_marker(&connection, id, b"READY", &mut command).await;
    input(&connection, id, b"crash\n").await;
    let result = completion(&connection, command).await;
    assert_eq!(result.status, ExecStatus::ProcessControlFailed);
    assert_eq!(result.exit_code, None);
    connection.close();
    assert!(bounded(fixture.host.drain()).await.is_empty());
}

pub(crate) async fn output_after_cap() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let id = "post-cap";
    for tty in [false, true] {
        let setup = if tty { "stty raw -echo; " } else { "" };
        let script = format!("{setup}printf READY; read go; exec /bin/cat /dev/zero");
        let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
        params.process_id = Some(id.into());
        params.tty = tty;
        params.stream_stdin = !tty;
        params.stream_stdout_stderr = true;
        params.output_bytes_cap = Some(8);
        let worker = connection.clone();
        let command =
            tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
        through_marker(&connection, id, b"READY").await;
        input(&connection, id, b"go\n").await;
        loop {
            if let NotificationMethod::CommandOutputDelta(delta) =
                bounded(connection.next_notification()).await.notification
                && delta.cap_reached
            {
                break;
            }
        }
        call(
            &connection,
            3,
            Method::CommandTerminate {
                process_id: id.into(),
            },
        )
        .await;
        let result = completion(&connection, command).await;
        assert_eq!(result.status, ExecStatus::Terminated);
        assert!(result.stdout_truncated);
    }
    connection.close();
    assert!(bounded(fixture.host.drain()).await.is_empty());
}

pub(crate) async fn watch_recheck() {
    use futures_util::FutureExt as _;
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let directory = fixture.workspaces[0].join("review-watch");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("document");
    std::fs::write(&path, b"before").unwrap();
    let next = directory.join("next");
    std::fs::write(&next, b"unchanged").unwrap();
    let capacity = fixture.host.file_observers();
    for (id, source) in [("file", &path), ("directory", &directory)] {
        call(
            &connection,
            1,
            Method::FsWatch {
                watch_id: id.into(),
                path: source.display().to_string(),
            },
        )
        .await;
    }
    let MethodResult::FileMetadata { metadata } = call(
        &connection,
        2,
        Method::FsGetMetadata {
            path: path.display().to_string(),
        },
    )
    .await
    else {
        panic!("stat")
    };
    let (renamed, at_commit) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    crate::local_runtime::environment::files::write_commit_hook(
        &path,
        Box::new(move || {
            renamed.send(()).unwrap();
            proceed
                .recv()
                .expect("release atomic write before directory fsync");
        }),
    );
    let mut checks = Vec::new();
    for source in [&path, &directory] {
        let (observed, checked) = tokio::sync::oneshot::channel();
        crate::app_server::filesystem::observe_recheck(source.clone(), observed);
        checks.push(checked);
    }
    let worker = connection.clone();
    let target = path.display().to_string();
    let write = tokio::spawn(async move {
        call(
            &worker,
            3,
            Method::FsWriteFile {
                path: target,
                data_base64: "YWZ0ZXI=".into(),
                expected_revision: Some(metadata.revision),
            },
        )
        .await
    });
    bounded(at_commit).await.unwrap();
    // Rename is complete, fsync/unlock have not happened, and both real native
    // callbacks have hit WouldBlock before the test releases the writer.
    for check in checks {
        bounded(check).await.unwrap();
    }
    assert_eq!(capacity.available_permits(), 30);
    release.send(()).unwrap();
    bounded(write).await.unwrap();
    loop {
        if let NotificationMethod::FsChanged(change) =
            bounded(connection.next_notification()).await.notification
        {
            if change.watch_id == "file" && change.retired {
                break;
            }
            assert!(
                !change.retired,
                "unchanged directory identity must remain live"
            );
        }
    }
    let returned = bounded(capacity.acquire_many(31)).await.unwrap();
    drop(returned);
    // A native event for this unchanged child proves the same directory
    // observer handles a new mutation after the file observer has settled.
    // Hold I/O so its identity check must publish a pathless resync hint.
    // Observe the event before its paths are discarded, so an earlier queued
    // resync cannot stand in for post-retirement native observation.
    let (held, locked) = tokio::sync::oneshot::channel();
    let (release, unlocked) = std::sync::mpsc::channel();
    let native = fixture.manager.environment.clone();
    let source = directory.clone();
    let lock = tokio::task::spawn_blocking(move || {
        native.with_paths(&[source.to_str().unwrap()], |_| {
            held.send(()).unwrap();
            unlocked.recv().unwrap();
            Ok(())
        })
    });
    bounded(locked).await.unwrap();
    let published =
        crate::app_server::filesystem::observe_native_change(directory.clone(), next.clone());
    std::fs::write(&next, b"still observed").unwrap();
    bounded(published).await.unwrap();
    loop {
        if let NotificationMethod::FsChanged(change) =
            bounded(connection.next_notification()).await.notification
        {
            assert!(!change.retired);
            if change.watch_id == "directory"
                && change.resync_required
                && change.changed_paths.is_empty()
            {
                break;
            }
        }
    }
    release.send(()).unwrap();
    bounded(lock).await.unwrap().unwrap();
    let MethodResult::FileChunk { chunk } = call(
        &connection,
        4,
        Method::FsReadFile {
            path: next.display().to_string(),
            offset: 0,
            max_bytes: 64,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("read observed change")
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        b"still observed"
    );
    let (scheduled, recheck) = tokio::sync::oneshot::channel();
    crate::app_server::filesystem::observe_recheck(directory.clone(), scheduled);
    let (held, locked) = tokio::sync::oneshot::channel();
    let (release, unlocked) = std::sync::mpsc::channel();
    let native = fixture.manager.environment.clone();
    let source = directory.clone();
    let lock = tokio::task::spawn_blocking(move || {
        native.with_paths(&[source.to_str().unwrap()], |_| {
            held.send(()).unwrap();
            unlocked.recv().unwrap();
            Ok(())
        })
    });
    bounded(locked).await.unwrap();
    let (started, checking) = tokio::sync::oneshot::channel();
    let (finish_check, finish_native) = std::sync::mpsc::channel();
    // Install only while I/O is held, so no physical check can have passed
    // this checkpoint. Its own I/O grant stays held through unwatch/drain.
    crate::app_server::filesystem::hold_recheck(
        directory.clone(),
        Box::new(move || {
            let _ = started.send(());
            let _ = finish_native.recv();
        }),
    );
    std::fs::write(&next, b"pending recheck").unwrap();
    bounded(recheck).await.unwrap();
    release.send(()).unwrap();
    bounded(lock).await.unwrap().unwrap();
    bounded(checking).await.unwrap();
    call(
        &connection,
        5,
        Method::FsUnwatch {
            watch_id: "directory".into(),
        },
    )
    .await;
    connection.close();
    assert!(bounded(fixture.host.drain()).await.is_empty());
    let mut settled = Box::pin(fixture.host.settle_file_observers());
    assert!(
        settled.as_mut().now_or_never().is_none(),
        "a blocked native recheck retains physical observer capacity after unwatch/disconnect"
    );
    finish_check.send(()).unwrap();
    bounded(settled).await;
    let returned = bounded(capacity.acquire_many(32)).await.unwrap();
    drop(returned);
    unwatch_queued_retirement().await;
}

async fn unwatch_queued_retirement() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let path = fixture.workspaces[0].join("retired-document");
    let replacement = fixture.workspaces[0].join("replacement");
    std::fs::write(&path, b"before").unwrap();
    std::fs::write(&replacement, b"after").unwrap();
    call(
        &connection,
        1,
        Method::FsWatch {
            watch_id: "retiring".into(),
            path: path.display().to_string(),
        },
    )
    .await;
    let queued = connection.watch_retirement_for_test("retiring");
    std::fs::rename(&replacement, &path).unwrap();
    bounded(queued).await.unwrap();
    // No notification is ever read. Unwatch must wake the obsolete receipt
    // wait while this healthy client and its transport remain connected.
    call(
        &connection,
        2,
        Method::FsUnwatch {
            watch_id: "retiring".into(),
        },
    )
    .await;
    let capacity = fixture.host.file_observers();
    let returned = bounded(capacity.acquire_many(32)).await.unwrap();
    drop(returned);
    assert!(matches!(
        call(&connection, 3, Method::ServerInfo {}).await,
        MethodResult::ServerInfo { .. }
    ));
    connection.close();
}

#[cfg(feature = "mcp-fixture")]
pub(crate) async fn catalog_credentials() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::tools::mcp::{catalog::CatalogKind, fixture};
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    use std::fmt::Write as _;
    let service = StreamableHttpService::<
        fixture::credential_echo::CredentialEcho,
        LocalSessionManager,
    >::new(
        || Ok(fixture::credential_echo::CredentialEcho(None)),
        Arc::default(),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
            .await
            .unwrap();
    });
    let secret = "catalog-secret-sentinel-533";
    for (header, bearer) in [
        (Some("Authorization"), false),
        (Some("Authorization"), true),
        (Some("pRoXy-AuThOrIzAtIoN"), true),
        (None, false),
    ] {
        let http = header.is_some();
        for reference in [false, true] {
            let options: &[Option<&str>] = if bearer {
                &[None, Some("name"), Some("tools"), Some("e"), Some("1")]
            } else {
                &[None]
            };
            for option in options {
                let fixture = Fixture::with_session_count(None, 0).await;
                let authorization = if bearer {
                    format!("bEaReR {secret}")
                } else {
                    secret.to_owned()
                };
                let transport = if let Some(header) = header {
                    let extra = option
                        .map(|value| format!(", \"X-Option\" = {value:?}"))
                        .unwrap_or_default();
                    let mut transport = format!(
                        "url = {endpoint:?}\n{} = {{ {header:?} = {:?}{} }}\n",
                        if reference {
                            "sensitive_headers"
                        } else {
                            "headers"
                        },
                        if reference {
                            if bearer {
                                "$RUSTX_CATALOG_BEARER_REGRESSION_KEY"
                            } else {
                                "$RUSTX_CATALOG_REGRESSION_KEY"
                            }
                        } else {
                            &authorization
                        },
                        if reference { "" } else { &extra }
                    );
                    if reference && let Some(option) = option {
                        // Keep ordinary transport controls in the complete secret
                        // set while native DTO keys and enum facts stay intact.
                        writeln!(transport, "headers = {{ \"X-Option\" = {option:?} }}").unwrap();
                    }
                    transport
                } else {
                    let args = fixture::fixture_spawn_args(
                        "boundary_suites::app_server_environment::mcp_catalog_redacts_literal_and_reference_http_and_stdio_credentials",
                    );
                    let env = if reference {
                        format!(
                            "env = {{ {} = \"1\" }}\nsensitive_env = {{ {} = \"$RUSTX_CATALOG_REGRESSION_KEY\" }}\n",
                            fixture::FIXTURE_MODE_ENV,
                            fixture::credential_echo::ECHO_ENV
                        )
                    } else {
                        format!(
                            "env = {{ {} = \"1\", {} = {secret:?} }}\n",
                            fixture::FIXTURE_MODE_ENV,
                            fixture::credential_echo::ECHO_ENV
                        )
                    };
                    format!(
                        "command = {:?}\nargs = {args:?}\n{env}",
                        std::env::current_exe().unwrap().display().to_string()
                    )
                };
                let text = format!("[mcp_servers.echo]\n{transport}");
                let root = fixture
                    .manager
                    .configuration
                    .resource_root(&SourceTarget::User);
                std::fs::create_dir_all(&root).unwrap();
                std::fs::write(root.join("mcp.toml"), &text).unwrap();
                let revision = crate::local_runtime::settings::revision(Some(text.as_bytes()));
                let connection = AppServerConnection::new(fixture.host.clone());
                initialize(&connection).await;
                for kind in [
                    CatalogKind::Tools,
                    CatalogKind::Resources,
                    CatalogKind::Templates,
                ] {
                    let response = call(
                        &connection,
                        1,
                        Method::McpCatalog {
                            target: SourceTarget::User,
                            id: crate::runtime::identity::McpServerId::new("echo"),
                            expected_revision: revision.clone(),
                            kind,
                        },
                    )
                    .await;
                    let MethodResult::McpProbe { result } = response else {
                        panic!("catalog response")
                    };
                    assert_eq!(
                        result.outcome,
                        crate::local_runtime::mcp_probe::McpProbeOutcome::Reachable,
                        "header={header:?}, bearer={bearer}, reference={reference}, option={option:?}, kind={kind:?}"
                    );
                    let page = result.page.expect("real peer catalog");
                    assert_eq!(page.kind, kind, "native catalog kind is not peer text");
                    assert!(page.capabilities.tools && page.capabilities.resources);
                    let projected = serde_json::to_string(&page).unwrap();
                    assert!(
                        !projected.contains(secret),
                        "credential escaped http={http}, reference={reference}, kind={kind:?}"
                    );
                    assert!(
                        projected.contains("<redacted>"),
                        "peer must actually echo the received credential"
                    );
                    if kind == CatalogKind::Tools && *option != Some("e") {
                        let properties = page.tools[0].input_schema.get("properties").unwrap();
                        assert!(
                            properties.get("<redacted>").is_some(),
                            "nested schema keys must be redacted"
                        );
                        for index in 0..2 {
                            assert!(
                                properties["<redacted>"]["properties"]["nested"]["enum"][index]
                                    .as_str()
                                    .unwrap()
                                    .contains("<redacted>")
                            );
                        }
                    }
                }
                assert_eq!(fixture.manager.diagnostics().loaded, 0);
                connection.close();
                assert!(bounded(fixture.host.drain()).await.is_empty());
            }
        }
    }
    server.abort();
    let _ = server.await;
}
