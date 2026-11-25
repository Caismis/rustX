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
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
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
        admit_or_fail(&connection, id, &mut command).await;
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
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
    execution.kill().await.unwrap();
}

pub(crate) async fn output_after_cap() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
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
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
    execution.kill().await.unwrap();
}

pub(crate) async fn watch_recheck() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let mut execution = fixture.start_execution(0).await;
    let session_id = fixture.sessions[0].id.clone();
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
                session_id: session_id.clone(),
                watch_id: id.into(),
                path: source.display().to_string(),
            },
        )
        .await;
    }
    assert_eq!(capacity.available_permits(), 30);
    let MethodResult::FileMetadata { metadata } = call(
        &connection,
        2,
        Method::FsGetMetadata {
            session_id: session_id.clone(),
            path: path.display().to_string(),
        },
    )
    .await
    else {
        panic!("stat")
    };
    let retired = connection.watch_retirement_for_test("file");
    call(
        &connection,
        3,
        Method::FsWriteFile {
            session_id: session_id.clone(),
            path: path.display().to_string(),
            data_base64: "YWZ0ZXI=".into(),
            expected_revision: Some(metadata.revision),
        },
    )
    .await;
    bounded(retired).await.unwrap();
    loop {
        if let NotificationMethod::FsChanged(change) =
            bounded(connection.next_notification()).await.notification
        {
            if change.watch_id == "file" {
                assert!(change.retired && change.resync_required);
                assert_eq!(change.changed_paths, [] as [std::path::PathBuf; 0]);
                break;
            }
            assert!(!change.retired, "the directory identity is unchanged");
        }
    }
    let returned = bounded(capacity.acquire_many(31)).await.unwrap();
    drop(returned);
    assert_eq!(capacity.available_permits(), 31);
    super::observed_mutation_hint(&connection, "directory", &directory, &next, || {
        std::fs::write(&next, b"still observed").unwrap();
    })
    .await;
    let MethodResult::FileChunk { chunk } = call(
        &connection,
        4,
        Method::FsReadFile {
            session_id: session_id.clone(),
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

    // Keep the data lane occupied while an independent control receipt
    // proves the directory observer stopped and returns its capacity.
    let endpoint = fixture.manager.execution_endpoint(&session_id).unwrap();
    let (held, locked) = tokio::sync::oneshot::channel();
    let (release, unlocked) = std::sync::mpsc::channel();
    let lock = tokio::task::spawn_blocking(move || {
        endpoint.hold_data_exchange_for_test(|| {
            held.send(()).unwrap();
            unlocked.recv().unwrap();
        })
    });
    bounded(locked).await.unwrap();
    assert_eq!(capacity.available_permits(), 31);
    std::fs::write(&next, b"pending change").unwrap();
    call(
        &connection,
        5,
        Method::FsUnwatch {
            watch_id: "directory".into(),
        },
    )
    .await;
    assert_eq!(capacity.available_permits(), 32);
    release.send(()).unwrap();
    bounded(lock).await.unwrap().unwrap();
    connection.close();
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
    execution.kill().await.unwrap();
    unwatch_queued_retirement().await;
}

async fn unwatch_queued_retirement() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let _execution = fixture.start_execution(0).await;
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
            session_id: fixture.sessions[0].id.clone(),
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
                let fixture = Fixture::with_session_count(None, usize::from(!http)).await;
                let mut client = if http {
                    None
                } else {
                    Some(fixture.start_execution(0).await)
                };
                let authorization = if bearer {
                    format!("bEaReR {secret}")
                } else {
                    secret.to_owned()
                };
                let transport = if let Some(header) = header {
                    let extra = option
                        .map_or_else(String::new, |value| format!(", \"X-Option\" = {value:?}"));
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
                            execution_session: (!http).then(|| fixture.sessions[0].id.clone()),
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
                assert_eq!(
                    bounded(fixture.host.drain()).await,
                    [] as [std::string::String; 0]
                );
                if let Some(client) = &mut client {
                    assert!(bounded(client.wait()).await.unwrap().success());
                }
            }
        }
    }
    server.abort();
    let _ = server.await;
}

/// Waits until a physically spawned pid is gone: the Client reaped it.
async fn wait_gone(pid: i32) {
    bounded(async {
        while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            != Err(nix::errno::Errno::ESRCH)
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// Five sequential operations on one Session, each abandoned by its UI
/// connection after START: every one physically terminates, the next one
/// still starts (no leaked capacity), and an independent operation on another
/// connection keeps running to its own result.
pub(crate) async fn detached_operations_release_without_touching_siblings() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let sibling = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&sibling).await;
    let sibling_params = parameters(&fixture, &["/bin/sh", "-c", "sleep 4; printf SIBLING"]);
    let sibling_worker = sibling.clone();
    let sibling_run =
        tokio::spawn(
            async move { call(&sibling_worker, 1, Method::CommandExec(sibling_params)).await },
        );
    for round in 0..5 {
        let id = format!("detach-{round}");
        let pidfile = format!("pid-{round}");
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        initialize(&connection).await;
        let script = format!("echo $$ > {pidfile}; printf READY; exec sleep 600");
        let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
        params.process_id = Some(id.clone());
        params.stream_stdout_stderr = true;
        let worker = connection.clone();
        let command =
            tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
        through_marker(&connection, &id, b"READY").await;
        let pid: i32 = std::fs::read_to_string(fixture.workspaces[0].join(&pidfile))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        connection.close();
        let _ = bounded(command).await;
        wait_gone(pid).await;
    }
    let MethodResult::CommandExec { result } = bounded(sibling_run).await.unwrap() else {
        panic!("the independent operation must produce its own result")
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&result.stdout_base64)
            .unwrap(),
        b"SIBLING"
    );
    sibling.close();
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
    execution.kill().await.unwrap();
}

/// A release refused before the wire as a busy data lane is retried within the
/// one release budget and then succeeds; the business result is unchanged.
pub(crate) async fn release_retries_prewire_busy_then_releases() {
    use crate::app_server::command::ReleaseOutcome;
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let id = "release-busy";
    let observer = crate::app_server::command::observe_release(id);
    crate::app_server::command::force_release_busy(id, 2);
    let mut params = parameters(&fixture, &["/bin/sh", "-c", "printf busy-release"]);
    params.process_id = Some(id.into());
    let worker = connection.clone();
    let command = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    let MethodResult::CommandExec { result } = bounded(command).await.unwrap() else {
        panic!("the business result survives a busy release")
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&result.stdout_base64)
            .unwrap(),
        b"busy-release"
    );
    assert_eq!(
        observer.outcomes(),
        [
            ReleaseOutcome::Busy,
            ReleaseOutcome::Busy,
            ReleaseOutcome::Released
        ]
    );
    connection.close();
    assert_eq!(
        bounded(fixture.host.drain()).await,
        [] as [std::string::String; 0]
    );
    execution.kill().await.unwrap();
}

pub(crate) async fn start_returns_the_client_cancelled_snapshot() {
    use rustx_execution_protocol::ProcessCancelDisposition;
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let id = "client-cancel-before-start-arbitration";
    let held = crate::app_server::command::hold_start_after_gate(id);
    let release = crate::app_server::command::observe_release(id);
    let mut params = parameters(
        &fixture,
        &["/bin/sh", "-c", "printf spawned > started-marker"],
    );
    params.process_id = Some(id.into());
    let command = tokio::spawn({
        let connection = connection.clone();
        async move { call(&connection, 1, Method::CommandExec(params)).await }
    });
    let key = bounded(held.key()).await;
    let endpoint = fixture
        .manager
        .execution_endpoint(&fixture.sessions[0].id)
        .unwrap();
    let control = endpoint.control().unwrap();
    assert_eq!(
        tokio::task::spawn_blocking(move || control.cancel_process(&key))
            .await
            .unwrap()
            .unwrap(),
        ProcessCancelDisposition::CancelledBeforeStart,
    );
    held.release();
    let result = completion(&connection, command).await;
    assert_eq!(result.status, ExecStatus::Terminated);
    assert_eq!(result.stdout_base64, "");
    assert_eq!(result.stderr_base64, "");
    assert_eq!(
        release.outcomes(),
        [crate::app_server::command::ReleaseOutcome::Released]
    );
    assert!(!fixture.workspaces[0].join("started-marker").exists());

    let MethodResult::CommandExec { result } = call(
        &connection,
        2,
        Method::CommandExec(parameters(&fixture, &["/bin/echo", "still admitted"])),
    )
    .await
    else {
        panic!("the next operation must run on the same binding");
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        endpoint
            .read_range(std::path::Path::new("rustx.toml"), 0, 128, None)
            .unwrap()
            .bytes,
        std::fs::read(fixture.workspaces[0].join("rustx.toml")).unwrap(),
    );
    connection.close();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    assert!(bounded(execution.wait()).await.unwrap().success());
}
