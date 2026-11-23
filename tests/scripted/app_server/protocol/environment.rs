//! Environment scenarios use real descriptors/processes; boundary wrappers own CI classification.
use super::*;
use crate::app_server::command::{ExecParams, ExecStatus, WriteParams};
use base64::Engine as _;
use std::path::Path;
use std::sync::Arc;
pub(crate) mod concurrency;
pub(crate) mod detach;
pub(crate) mod review;

fn parameters(fixture: &Fixture, command: &[&str]) -> ExecParams {
    serde_json::from_value(serde_json::json!({"sessionId":fixture.sessions[0].id,"command":command,"cwd":fixture.workspaces[0],"env":{},"processId":null,"tty":false,"streamStdin":false,"streamStdoutStderr":false,"outputBytesCap":null,"timeoutMs":10_000,"size":null})).unwrap()
}
fn grant(fixture: &Fixture) {
    fixture
        .manager
        .bind_process_policy(crate::local_runtime::app_server_policy::AppServerPolicy {
            environment_roots: fixture.workspaces.to_vec(),
            ..Default::default()
        });
}
async fn rejected(connection: &AppServerConnection, id: i64, call: Method) -> ErrorData {
    let response = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await;
    match response {
        Response::Failure(failure) => failure.error.data.unwrap(),
        other @ Response::Success(_) => panic!("expected rejection: {other:?}"),
    }
}
async fn output(connection: &AppServerConnection, id: &str) -> Vec<u8> {
    loop {
        let notification = bounded(connection.next_notification()).await;
        if let NotificationMethod::CommandOutputDelta(delta) = notification.notification {
            assert_eq!(delta.process_id, id);
            return base64::engine::general_purpose::STANDARD
                .decode(delta.delta_base64)
                .unwrap();
        }
    }
}
fn process_gone(pid: i32) {
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "descendant must be physically absent"
    );
}
pub(super) async fn admit_or_fail<T: std::fmt::Debug>(
    connection: &AppServerConnection,
    id: &str,
    command: &mut tokio::task::JoinHandle<T>,
) {
    bounded(async {
        tokio::select! {
            () = connection.command_admitted_for_test(id) => {}
            early = command => panic!("command {id} finished before admission check: {early:?}"),
        }
    })
    .await;
}

pub(crate) async fn buffered_scenario() {
    let fixture = Fixture::new().await;
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let params = parameters(&fixture, &["/usr/bin/printf", "%s", "literal;$(false)"]);
    // Without an admitted Client the Session cannot resolve — honest
    // unavailability, never a Server-local fallback.
    assert_eq!(
        rejected(&connection, 0, Method::CommandExec(params.clone())).await,
        ErrorData::ExecutionUnavailable
    );
    // With the Client admitted but no environment grant, the workspace
    // check refuses before anything is dispatched.
    let mut execution = fixture.start_execution(0).await;
    assert_eq!(
        rejected(&connection, 1, Method::CommandExec(params.clone())).await,
        ErrorData::EnvironmentUnauthorized
    );
    grant(&fixture);
    let MethodResult::CommandExec { result } =
        call(&connection, 2, Method::CommandExec(params)).await
    else {
        panic!("command result")
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
        b"literal;$(false)"
    );
    let mut params = parameters(
        &fixture,
        &[
            "/bin/sh",
            "-c",
            "printf '\\377\\000'; printf failure >&2; exit 7",
        ],
    );
    params
        .env
        .insert("EXPLICIT".into(), Some("authorized".into()));
    let MethodResult::CommandExec { result } =
        call(&connection, 3, Method::CommandExec(params)).await
    else {
        panic!("command result")
    };
    assert_eq!(result.exit_code, Some(7));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
        [255, 0]
    );
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stderr_base64)
            .unwrap(),
        b"failure"
    );
    let MethodResult::CommandExec { result } = call(
        &connection,
        4,
        Method::CommandExec(parameters(&fixture, &["/usr/bin/env"])),
    )
    .await
    else {
        panic!("command result")
    };
    let environment = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        environment.lines().count(),
        18,
        "native basics plus Git child defaults; no provider or App Server credentials"
    );
    assert!(environment.contains("LANG=C.UTF-8"));
    assert!(environment.contains("GIT_CONFIG_COUNT=5\n"));
    assert!(environment.contains("GIT_CONFIG_PARAMETERS=\n"));
    assert!(environment.contains("GIT_CONFIG_KEY_4=core.fsmonitor\n"));
    assert!(environment.contains("GIT_CONFIG_VALUE_4=false\n"));
    assert_eq!(fixture.manager.diagnostics().loaded, 0);
    execution.kill().await.unwrap();
    connection.close();
}

pub(crate) async fn connection_process_scenario() {
    let fixture = Fixture::new().await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let first = Arc::new(AppServerConnection::new(fixture.host.clone()));
    let second = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&first).await;
    initialize(&second).await;
    let mut params = parameters(
        &fixture,
        &["/bin/sh", "-c", "sleep 600 & printf '%s\\n' \"$!\"; wait"],
    );
    params.process_id = Some("same".into());
    params.stream_stdout_stderr = true;
    let first_call = first.clone();
    let first_params = params.clone();
    let first_result =
        tokio::spawn(async move { call(&first_call, 1, Method::CommandExec(first_params)).await });
    let bytes = output(&first, "same").await;
    let pid: i32 = String::from_utf8(bytes).unwrap().trim().parse().unwrap();
    assert_eq!(
        rejected(&first, 2, Method::CommandExec(params.clone())).await,
        ErrorData::EnvironmentCapacityOrConflict
    );
    assert_eq!(
        rejected(
            &second,
            2,
            Method::CommandTerminate {
                process_id: "same".into()
            }
        )
        .await,
        ErrorData::EnvironmentResourceNotFound
    );
    let mut cat = parameters(&fixture, &["/bin/cat"]);
    cat.process_id = Some("same".into());
    cat.stream_stdin = true;
    cat.stream_stdout_stderr = true;
    let second_call = second.clone();
    let mut second_result =
        tokio::spawn(async move { call(&second_call, 3, Method::CommandExec(cat)).await });
    admit_or_fail(&second, "same", &mut second_result).await;
    let payload = base64::engine::general_purpose::STANDARD.encode([0, 255, 10]);
    call(
        &second,
        4,
        Method::CommandWrite(WriteParams {
            process_id: "same".into(),
            delta_base64: Some(payload),
            close_stdin: true,
        }),
    )
    .await;
    assert_eq!(output(&second, "same").await, [0, 255, 10]);
    let MethodResult::CommandExec { result } = bounded(second_result).await.unwrap() else {
        panic!("cat result")
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.stdout_base64, "");
    call(
        &first,
        3,
        Method::CommandTerminate {
            process_id: "same".into(),
        },
    )
    .await;
    let MethodResult::CommandExec { result } = bounded(first_result).await.unwrap() else {
        panic!("terminated result")
    };
    assert_eq!(result.status, ExecStatus::Terminated);
    process_gone(pid);
    let disconnect_call = first.clone();
    let disconnected = tokio::spawn(async move {
        disconnect_call
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(5),
                call: Method::CommandExec(params),
            })
            .await
    });
    let pid: i32 = String::from_utf8(output(&first, "same").await)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut operations = fixture.host.request_operations();
    first.close();
    bounded(disconnected).await.unwrap();
    bounded(operations.wait_for(|count| *count == 0))
        .await
        .unwrap();
    process_gone(pid);
    execution.kill().await.unwrap();
    second.close();
}
async fn rejected_or_result(
    connection: &AppServerConnection,
    id: i64,
    call: Method,
) -> Result<MethodResult, ErrorData> {
    match connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await
    {
        Response::Success(response) => Ok(response.result),
        Response::Failure(failure) => Err(failure.error.data.unwrap()),
    }
}

pub(crate) async fn terminal_scenario() {
    let fixture = Fixture::new().await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let mut params = parameters(
        &fixture,
        &[
            "/bin/sh",
            "-c",
            "stty size; read line; stty size; printf 'value:%s\\n' \"$line\"",
        ],
    );
    params.process_id = Some("terminal".into());
    params.tty = true;
    let worker = connection.clone();
    let result = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    let initial = String::from_utf8(output(&connection, "terminal").await).unwrap();
    assert!(initial.contains("24 80"), "{initial:?}");
    call(
        &connection,
        2,
        Method::CommandResize {
            process_id: "terminal".into(),
            size: crate::app_server::command::TerminalSize {
                rows: 40,
                cols: 100,
            },
        },
    )
    .await;
    call(
        &connection,
        3,
        Method::CommandWrite(WriteParams {
            process_id: "terminal".into(),
            delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(b"typed\n")),
            close_stdin: false,
        }),
    )
    .await;
    let mut bytes = Vec::new();
    tokio::pin!(result);
    let final_result = loop {
        tokio::select! {
            result = &mut result => break result.unwrap(),
            notification = connection.next_notification() => if let NotificationMethod::CommandOutputDelta(delta) = notification.notification { bytes.extend(base64::engine::general_purpose::STANDARD.decode(delta.delta_base64).unwrap()); },
        }
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("40 100"), "{text:?}");
    assert!(text.contains("value:typed"), "{text:?}");
    let MethodResult::CommandExec { result } = final_result else {
        panic!("terminal result")
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.stdout_base64, "");
    execution.kill().await.unwrap();
    connection.close();
}

async fn terminal_completion(
    connection: &AppServerConnection,
    command: tokio::task::JoinHandle<MethodResult>,
) -> (crate::app_server::command::ExecResult, Vec<u8>) {
    let (response, bytes) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::pin!(command);
        let mut bytes = Vec::new();
        loop {
            tokio::select! {
                response = &mut command => break (response.unwrap(), bytes),
                notification = connection.next_notification() => {
                    if let NotificationMethod::CommandOutputDelta(delta) = notification.notification {
                        bytes.extend(base64::engine::general_purpose::STANDARD.decode(delta.delta_base64).unwrap());
                    }
                },
            }
        }
    }).await.expect("terminal must finish naturally before the command timeout");
    let MethodResult::CommandExec { result } = response else {
        panic!("PTY completion")
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.exit_code, Some(0));
    (result, bytes)
}

pub(crate) async fn terminal_descriptor_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    // /dev/fd and shell test are native on both supported Unix platforms.
    // Stdio is intentional; any extra character-device fd is a leaked PTY.
    let inspect = r#"for fd in /dev/fd/*; do case "${fd##*/}" in 0|1|2) continue;; esac; if test -c "$fd"; then printf 'EXTRA_FD:%s\n' "$fd"; fi; done"#;
    let script = format!("{inspect}; printf READY; read line");
    let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
    params.process_id = Some("descriptor-owner".into());
    params.tty = true;
    let worker = connection.clone();
    let command = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    let mut ready = Vec::new();
    while !ready.ends_with(b"READY") {
        ready.extend(output(&connection, "descriptor-owner").await);
    }
    assert_eq!(
        ready, b"READY",
        "the terminal command must not inherit its raw endpoints"
    );
    let observer = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&observer).await;
    let script = format!("{inspect}; printf CHECKED; read line");
    let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
    params.process_id = Some("descriptor-observer".into());
    params.stream_stdin = true;
    params.stream_stdout_stderr = true;
    let worker = observer.clone();
    let other = tokio::spawn(async move { call(&worker, 2, Method::CommandExec(params)).await });
    let mut checked = Vec::new();
    while !checked.ends_with(b"CHECKED") {
        checked.extend(output(&observer, "descriptor-observer").await);
    }
    assert_eq!(
        checked, b"CHECKED",
        "another command must not inherit the live PTY"
    );
    call(
        &connection,
        3,
        Method::CommandWrite(WriteParams {
            process_id: "descriptor-owner".into(),
            delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(b"done\n")),
            close_stdin: false,
        }),
    )
    .await;
    terminal_completion(&connection, command).await;
    assert!(
        !other.is_finished(),
        "the PTY must settle while the unrelated command is still alive"
    );
    call(
        &observer,
        4,
        Method::CommandWrite(WriteParams {
            process_id: "descriptor-observer".into(),
            delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(b"done\n")),
            close_stdin: false,
        }),
    )
    .await;
    terminal_completion(&observer, other).await;
    execution.kill().await.unwrap();
    observer.close();
    connection.close();
}

pub(crate) async fn terminal_eof_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let cases: [(&str, bool, &[u8], &[u8]); 5] = [
        ("stty -echo", false, b"abc", b"abc"),
        ("stty -echo eof '^A'", false, b"abc", b"abc"),
        ("stty -echo", true, b"abc", b"abc"),
        ("stty -echo", false, b"abc\x16", b"abc\x04"),
        ("stty -echo", true, b"abc\x16", b"abc\x04"),
    ];
    for (setup, split, payload, expected) in cases {
        let script = format!("{setup}; printf READY; exec /bin/cat");
        let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
        params.process_id = Some("canonical-eof".into());
        params.tty = true;
        let worker = connection.clone();
        let command =
            tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
        assert_eq!(output(&connection, "canonical-eof").await, b"READY");
        if split {
            call(
                &connection,
                2,
                Method::CommandWrite(WriteParams {
                    process_id: "canonical-eof".into(),
                    delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(payload)),
                    close_stdin: false,
                }),
            )
            .await;
        }
        call(
            &connection,
            3,
            Method::CommandWrite(WriteParams {
                process_id: "canonical-eof".into(),
                delta_base64: (!split)
                    .then(|| base64::engine::general_purpose::STANDARD.encode(payload)),
                close_stdin: true,
            }),
        )
        .await;
        let (_, bytes) = terminal_completion(&connection, command).await;
        assert_eq!(bytes, expected);
    }
    let rejected_cases: [(&str, &[u8]); 3] = [
        ("stty raw -echo", b"x"),
        ("stty -echo eof undef", b"x\n"),
        ("stty -echo eof '^C'", b"x\n"),
    ];
    for (setup, accepted) in rejected_cases {
        let script = format!("{setup}; printf READY; head -c 1");
        let mut params = parameters(&fixture, &["/bin/sh", "-c", &script]);
        params.process_id = Some("rejected-eof".into());
        params.tty = true;
        let worker = connection.clone();
        let command =
            tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
        assert_eq!(output(&connection, "rejected-eof").await, b"READY");
        assert_eq!(
            rejected(
                &connection,
                2,
                Method::CommandWrite(WriteParams {
                    process_id: "rejected-eof".into(),
                    delta_base64: Some(
                        base64::engine::general_purpose::STANDARD.encode(b"must-not-be-written")
                    ),
                    close_stdin: true,
                })
            )
            .await,
            ErrorData::InvalidParams
        );
        call(
            &connection,
            3,
            Method::CommandWrite(WriteParams {
                process_id: "rejected-eof".into(),
                delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(accepted)),
                close_stdin: false,
            }),
        )
        .await;
        let (_, bytes) = terminal_completion(&connection, command).await;
        assert_eq!(
            bytes, b"x",
            "invalid EOF must leave stdin open and write none of its batch"
        );
    }
    execution.kill().await.unwrap();
    connection.close();
}

pub(crate) async fn watch_registration_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let session_id = &fixture.sessions[0].id;
    let root = &fixture.workspaces[0];
    let source = root.join("exact-source");
    let saved = root.join("retired-source");
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("outside");
    std::fs::write(&external, b"private").unwrap();
    std::os::unix::fs::symlink(&external, &source).unwrap();
    let watch = || Method::FsWatch {
        session_id: session_id.clone(),
        watch_id: "inode".into(),
        path: source.display().to_string(),
    };
    assert_eq!(
        rejected(&connection, 0, watch()).await,
        ErrorData::ExecutionUnavailable,
    );
    let _execution = fixture.start_execution(0).await;
    assert_eq!(
        rejected(&connection, 1, watch()).await,
        ErrorData::EnvironmentUnauthorized,
    );
    std::fs::remove_file(&source).unwrap();
    for directory in [true, false] {
        if directory {
            std::fs::create_dir(&source).unwrap();
        } else {
            std::fs::write(&source, b"inside").unwrap();
        }
        call(&connection, 2, watch()).await;
        let sentinel = if directory {
            source.join("owned")
        } else {
            source.clone()
        };
        observed_mutation_hint(&connection, "inode", &source, &sentinel, || {
            std::fs::write(&sentinel, b"authorized").unwrap();
        })
        .await;
        let retiring = connection.watch_retirement_for_test("inode");
        std::fs::rename(&source, &saved).unwrap();
        std::os::unix::fs::symlink(&external, &source).unwrap();
        std::fs::write(&external, b"outside-private-name").unwrap();
        bounded(retiring).await.unwrap();
        loop {
            if let NotificationMethod::FsChanged(changed) =
                bounded(connection.next_notification()).await.notification
            {
                assert!(
                    !changed
                        .changed_paths
                        .iter()
                        .any(|path| path.starts_with(outside.path()))
                );
                if changed.retired {
                    assert_eq!(changed.watch_id, "inode");
                    assert!(changed.resync_required);
                    assert_eq!(changed.changed_paths, [] as [std::path::PathBuf; 0]);
                    break;
                }
            }
        }
        std::fs::remove_file(&source).unwrap();
        if directory {
            std::fs::remove_dir_all(&saved).unwrap();
        } else {
            std::fs::remove_file(&saved).unwrap();
        }
        bounded(fixture.host.settle_file_observers()).await;
    }
    connection.close();
}

pub(crate) async fn observed_mutation_hint(
    connection: &AppServerConnection,
    watch_id: &str,
    source: &Path,
    expected: &Path,
    mutation: impl FnOnce(),
) -> crate::app_server::filesystem::Changed {
    use futures_util::FutureExt as _;
    let mut phases = FixturePhase::new("native filesystem mutation hint");
    phases.enter("drain earlier routed hints");
    while connection.next_notification().now_or_never().is_some() {}
    phases.enter("fresh physical mutation and Client hint");
    mutation();
    phases.enter("routed hint after native publication");
    let mut last = None;
    let change = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let NotificationMethod::FsChanged(change) =
                connection.next_notification().await.notification
            {
                if change.watch_id != watch_id {
                    continue;
                }
                assert!(!change.retired);
                assert!(
                    change
                        .changed_paths
                        .iter()
                        .all(|path| path.starts_with(source))
                );
                if change.changed_paths.contains(&expected.to_path_buf())
                    || cfg!(target_os = "macos") && change.resync_required
                {
                    return change;
                }
                last = Some(change);
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "published native hint for {}: last={last:?}",
            expected.display()
        )
    });
    phases.finish();
    change
}

#[cfg(target_os = "macos")]
pub(crate) async fn macos_watch_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    // fs/readFile dispatches through Session A's admitted Execution Client.
    let mut execution = fixture.start_execution(0).await;
    let session_id = fixture.sessions[0].id.clone();
    let source = fixture.workspaces[0].join("bounded-watch");
    std::fs::create_dir(&source).unwrap();
    let existing = source.join("existing");
    std::fs::write(&existing, b"before").unwrap();
    let child_directory = source.join("directory");
    std::fs::create_dir(&child_directory).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), source.join("outside-symlink")).unwrap();
    call(
        &connection,
        1,
        Method::FsWatch {
            session_id: session_id.clone(),
            watch_id: "macos".into(),
            path: source.display().to_string(),
        },
    )
    .await;
    macos_watch_existing(&connection, &session_id, &source).await;
    macos_watch_membership(&connection, &session_id, &source).await;
    call(
        &connection,
        2,
        Method::FsUnwatch {
            watch_id: "macos".into(),
        },
    )
    .await;
    assert_eq!(fixture.host.file_observers().available_permits(), 32);
    execution.kill().await.unwrap();
    connection.close();
}

#[cfg(target_os = "macos")]
async fn macos_watch_existing(
    connection: &AppServerConnection,
    session_id: &crate::local_runtime::session::SessionId,
    source: &Path,
) {
    use std::os::unix::fs::PermissionsExt;
    let mut phases = FixturePhase::new("macOS descriptor existing children");
    let existing = source.join("existing");
    let child_directory = source.join("directory");
    phases.enter("existing file contents");
    observed_mutation_hint(connection, "macos", source, &existing, || {
        std::fs::write(&existing, b"after").unwrap();
    })
    .await;
    macos_watch_read(connection, session_id, &existing, b"after").await;
    let mode = std::fs::metadata(&child_directory)
        .unwrap()
        .permissions()
        .mode();
    phases.enter("child directory attributes");
    observed_mutation_hint(connection, "macos", source, &child_directory, || {
        std::fs::set_permissions(
            &child_directory,
            std::fs::Permissions::from_mode(mode ^ 0o100),
        )
        .unwrap();
    })
    .await;
    assert_eq!(
        std::fs::metadata(&child_directory)
            .unwrap()
            .permissions()
            .mode(),
        mode ^ 0o100
    );
    phases.finish();
}

#[cfg(target_os = "macos")]
async fn macos_watch_membership(
    connection: &AppServerConnection,
    session_id: &crate::local_runtime::session::SessionId,
    source: &Path,
) {
    let mut phases = FixturePhase::new("macOS descriptor membership refresh");
    let created = source.join("created");
    phases.enter("register created child");
    observed_mutation_hint(connection, "macos", source, source, || {
        std::fs::write(&created, b"new").unwrap();
    })
    .await;
    macos_watch_read(connection, session_id, &created, b"new").await;
    phases.enter("created child contents");
    observed_mutation_hint(connection, "macos", source, &created, || {
        std::fs::write(&created, b"changed").unwrap();
    })
    .await;
    macos_watch_read(connection, session_id, &created, b"changed").await;
    phases.enter("remove child descriptor");
    observed_mutation_hint(connection, "macos", source, source, || {
        std::fs::remove_file(&created).unwrap();
    })
    .await;
    assert!(!created.exists());
    let existing = source.join("existing");
    let renamed = source.join("renamed");
    phases.enter("rename child descriptor");
    observed_mutation_hint(connection, "macos", source, source, || {
        std::fs::rename(&existing, &renamed).unwrap();
    })
    .await;
    phases.enter("renamed child contents");
    observed_mutation_hint(connection, "macos", source, &renamed, || {
        std::fs::write(&renamed, b"changed again").unwrap();
    })
    .await;
    macos_watch_read(connection, session_id, &renamed, b"changed again").await;
    phases.finish();
}

#[cfg(target_os = "macos")]
async fn macos_watch_read(
    connection: &AppServerConnection,
    session_id: &crate::local_runtime::session::SessionId,
    path: &Path,
    expected: &[u8],
) {
    let MethodResult::FileChunk { chunk } = call(
        connection,
        99,
        Method::FsReadFile {
            session_id: session_id.clone(),
            path: path.display().to_string(),
            offset: 0,
            max_bytes: 64,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("read observed child");
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        expected
    );
}

pub(crate) async fn filesystem_scenario() {
    let fixture = Fixture::new().await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let session_id = fixture.sessions[0].id.clone();
    let unbound = fixture.workspaces[0].join("unbound");
    assert_eq!(
        rejected(
            &connection,
            20,
            Method::FsCreateDirectory {
                session_id: session_id.clone(),
                path: unbound.display().to_string(),
                recursive: true,
            },
        )
        .await,
        ErrorData::ExecutionUnavailable
    );
    assert!(!unbound.exists());
    let mut execution = fixture.start_execution(0).await;
    let mut execution_b = fixture.start_execution(1).await;
    let session_b = fixture.sessions[1].id.clone();
    let path = fixture.workspaces[0].join("document").display().to_string();
    let MethodResult::FileMetadata { metadata } = call(
        &connection,
        1,
        Method::FsWriteFile {
            session_id: session_id.clone(),
            path: path.clone(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(b"before"),
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("write")
    };
    assert_eq!(
        rejected(
            &connection,
            2,
            Method::FsWriteFile {
                session_id: session_id.clone(),
                path: path.clone(),
                data_base64: "YWZ0ZXI=".into(),
                expected_revision: None
            }
        )
        .await,
        ErrorData::EnvironmentCapacityOrConflict
    );
    let revision = metadata.revision;
    let write = Method::FsWriteFile {
        session_id: session_id.clone(),
        path: path.clone(),
        data_base64: "YWZ0ZXI=".into(),
        expected_revision: Some(revision.clone()),
    };
    let (first, second) = tokio::join!(
        rejected_or_result(&connection, 3, write.clone()),
        rejected_or_result(&connection, 4, write)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let MethodResult::FileChunk { chunk } = call(
        &connection,
        5,
        Method::FsReadFile {
            session_id: session_id.clone(),
            path: path.clone(),
            offset: 0,
            max_bytes: 2,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("read")
    };
    assert_eq!(chunk.data_base64, "YWY=");
    assert_eq!(chunk.next_offset, Some(2));
    assert_eq!(
        rejected(
            &connection,
            6,
            Method::FsReadFile {
                session_id: session_id.clone(),
                path: path.clone(),
                offset: 0,
                max_bytes: 2,
                expected_revision: Some(revision)
            }
        )
        .await,
        ErrorData::EnvironmentCapacityOrConflict
    );
    let outside = fixture.archive_root.join("outside");
    std::fs::write(&outside, b"private").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.workspaces[0].join("link")).unwrap();
    for denied in [
        outside.display().to_string(),
        fixture.workspaces[0]
            .join("../outside")
            .display()
            .to_string(),
        fixture.workspaces[0].join("link").display().to_string(),
    ] {
        assert!(
            rejected_or_result(
                &connection,
                7,
                Method::FsReadFile {
                    session_id: session_id.clone(),
                    path: denied,
                    offset: 0,
                    max_bytes: 32,
                    expected_revision: None
                }
            )
            .await
            .is_err()
        );
    }
    let directory = fixture.workspaces[0].join("copy").display().to_string();
    call(
        &connection,
        8,
        Method::FsCreateDirectory {
            session_id: session_id.clone(),
            path: directory.clone(),
            recursive: true,
        },
    )
    .await;
    call(
        &connection,
        9,
        Method::FsCopy {
            session_id: session_id.clone(),
            source_path: path,
            destination_path: format!("{directory}/copy"),
            recursive: false,
        },
    )
    .await;
    assert_eq!(
        std::fs::read(format!("{directory}/copy")).unwrap(),
        b"after"
    );
    let MethodResult::DirectoryPage { page } = call(
        &connection,
        10,
        Method::FsReadDirectory {
            session_id: session_id.clone(),
            path: directory.clone(),
            offset: 0,
            limit: 16,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("list")
    };
    assert_eq!(page.entries.len(), 1);
    let copied_root = fixture.workspaces[0].join("copied-root");
    assert_eq!(
        rejected(
            &connection,
            15,
            Method::FsCopy {
                session_id: session_b,
                source_path: directory.clone(),
                destination_path: fixture.workspaces[1].join("denied").display().to_string(),
                recursive: true,
            },
        )
        .await,
        ErrorData::EnvironmentUnauthorized
    );
    assert!(!fixture.workspaces[1].join("denied").exists());
    call(
        &connection,
        16,
        Method::FsCopy {
            session_id: session_id.clone(),
            source_path: directory.clone(),
            destination_path: copied_root.display().to_string(),
            recursive: true,
        },
    )
    .await;
    assert_eq!(std::fs::read(copied_root.join("copy")).unwrap(), b"after");
    let inside = format!("{directory}/recursive-cycle");
    assert_eq!(
        rejected(
            &connection,
            19,
            Method::FsCopy {
                session_id: session_id.clone(),
                source_path: directory.clone(),
                destination_path: inside.clone(),
                recursive: true
            }
        )
        .await,
        ErrorData::InvalidParams
    );
    assert!(!Path::new(&inside).exists());
    call(
        &connection,
        17,
        Method::FsRemove {
            session_id: session_id.clone(),
            path: copied_root.display().to_string(),
            recursive: true,
            force: false,
        },
    )
    .await;
    call(
        &connection,
        18,
        Method::FsRemove {
            session_id: session_id.clone(),
            path: copied_root.display().to_string(),
            recursive: true,
            force: true,
        },
    )
    .await;
    call(
        &connection,
        11,
        Method::FsWatch {
            session_id: fixture.sessions[0].id.clone(),
            watch_id: "watch".into(),
            path: directory.clone(),
        },
    )
    .await;
    std::fs::write(format!("{directory}/changed"), b"hint").unwrap();
    loop {
        if matches!(
            bounded(connection.next_notification()).await.notification,
            NotificationMethod::FsChanged(_)
        ) {
            break;
        }
    }
    call(
        &connection,
        12,
        Method::FsUnwatch {
            watch_id: "watch".into(),
        },
    )
    .await;
    call(
        &connection,
        21,
        Method::FsRemove {
            session_id: session_id.clone(),
            path: fixture.workspaces[0].join("link").display().to_string(),
            recursive: true,
            force: false,
        },
    )
    .await;
    assert!(!fixture.workspaces[0].join("link").is_symlink());
    assert_eq!(std::fs::read(outside).unwrap(), b"private");
    call(
        &connection,
        13,
        Method::FsRemove {
            session_id: session_id.clone(),
            path: directory.clone(),
            recursive: true,
            force: false,
        },
    )
    .await;
    assert!(!Path::new(&directory).exists());
    fixture
        .manager
        .bind_process_policy(crate::local_runtime::app_server_policy::AppServerPolicy::default());
    assert_eq!(
        rejected(
            &connection,
            14,
            Method::FsGetMetadata {
                session_id: session_id.clone(),
                path: fixture.workspaces[0].display().to_string()
            }
        )
        .await,
        ErrorData::EnvironmentUnauthorized
    );
    execution.kill().await.unwrap();
    execution_b.kill().await.unwrap();
    connection.close();
}

#[tokio::test]
async fn skills_management_keeps_provenance_metadata_and_content_revisions() {
    let fixture = Fixture::with_session_count(None, 0).await;
    let directory = fixture.workspaces[0].join(".agents/skills/inspection");
    std::fs::create_dir_all(&directory).unwrap();
    let body = "---\nname: inspection\ndescription: Inspect native management\nlicense: MIT\n---\nVersion one.\n";
    let external = fixture.workspaces[0]
        .parent()
        .unwrap()
        .join("shared-instructions.md");
    std::fs::write(&external, body).unwrap();
    std::os::unix::fs::symlink(&external, directory.join("SKILL.md")).unwrap();
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let target = crate::local_runtime::configuration::settings::SourceTarget::Workspace {
        directory: fixture.workspaces[0].clone(),
    };
    let MethodResult::Skills { page } = call(
        &connection,
        1,
        Method::SkillsList {
            execution_session: None,
            target: target.clone(),
            offset: 0,
            limit: 32,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("skills catalog")
    };
    let skill = page
        .data
        .iter()
        .find(|skill| skill.catalog.name == "inspection")
        .unwrap();
    assert_ne!(skill.metadata_revision, "");
    assert_eq!(
        skill.provenance.source,
        crate::skills::SkillSource::Workspace
    );
    let MethodResult::SkillContent { content } = call(
        &connection,
        2,
        Method::SkillsRead {
            execution_session: None,
            target: target.clone(),
            name: "inspection".into(),
            kind: crate::local_runtime::skill_management::SkillContentKind::Instructions,
            offset: 0,
            max_bytes: 65536,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("skill content")
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&content.data_base64)
            .unwrap(),
        body.as_bytes()
    );
    std::fs::write(&external, body.replace("Version one", "Version two")).unwrap();
    assert!(matches!(
        rejected(
            &connection,
            3,
            Method::SkillsRead {
                execution_session: None,
                target: target.clone(),
                name: "inspection".into(),
                kind: crate::local_runtime::skill_management::SkillContentKind::Instructions,
                offset: 0,
                max_bytes: 65536,
                expected_revision: Some(content.revision)
            }
        )
        .await,
        ErrorData::SourceConflict { .. }
    ));
    assert_eq!(
        rejected(
            &connection,
            4,
            Method::SkillsRead {
                execution_session: None,
                target,
                name: "../inspection".into(),
                kind: crate::local_runtime::skill_management::SkillContentKind::Instructions,
                offset: 0,
                max_bytes: 65536,
                expected_revision: None
            }
        )
        .await,
        ErrorData::InvalidParams
    );
    connection.close();
}

pub(crate) async fn output_capacity_and_timeout_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let mut capped = parameters(
        &fixture,
        &["/bin/sh", "-c", "printf 123456789; printf abcdef >&2"],
    );
    capped.output_bytes_cap = Some(3);
    let MethodResult::CommandExec { result } =
        call(&connection, 1, Method::CommandExec(capped)).await
    else {
        panic!("bounded output")
    };
    assert_eq!(result.stdout_base64, "MTIz");
    assert_eq!(result.stderr_base64, "YWJj");
    assert!(result.stdout_truncated && result.stderr_truncated);
    let mut timed = parameters(
        &fixture,
        &["/bin/sh", "-c", "sleep 600 & printf '%s\\n' \"$!\"; wait"],
    );
    timed.process_id = Some("timed".into());
    timed.stream_stdout_stderr = true;
    timed.timeout_ms = Some(5000);
    let worker = connection.clone();
    let completion =
        tokio::spawn(async move { call(&worker, 2, Method::CommandExec(timed)).await });
    let pid: i32 = String::from_utf8(output(&connection, "timed").await)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let MethodResult::CommandExec { result } = bounded(completion).await.unwrap() else {
        panic!("timeout result")
    };
    assert_eq!(result.status, ExecStatus::TimedOut);
    process_gone(pid);
    let healthy = AppServerConnection::new(fixture.host.clone());
    initialize(&healthy).await;
    let mut streamed = parameters(
        &fixture,
        &["/bin/sh", "-c", "read go; head -c 1048576 /dev/zero"],
    );
    streamed.process_id = Some("slow".into());
    streamed.stream_stdout_stderr = true;
    streamed.stream_stdin = true;
    streamed.output_bytes_cap = Some(262_144);
    let worker = connection.clone();
    let mut slow =
        tokio::spawn(async move { rejected(&worker, 3, Method::CommandExec(streamed)).await });
    admit_or_fail(&connection, "slow", &mut slow).await;
    call(
        &connection,
        4,
        Method::CommandWrite(WriteParams {
            process_id: "slow".into(),
            delta_base64: Some("Z28K".into()),
            close_stdin: true,
        }),
    )
    .await;
    let (error, info) = tokio::join!(bounded(slow), call(&healthy, 5, Method::ServerInfo {}));
    assert_eq!(error.unwrap(), ErrorData::ConnectionClosed);
    assert!(matches!(info, MethodResult::ServerInfo { .. }));
    execution.kill().await.unwrap();
    connection.close();
    healthy.close();
    bounded(
        fixture
            .host
            .request_operations()
            .wait_for(|count| *count == 0),
    )
    .await
    .unwrap();
}

pub(crate) async fn host_drain_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    call(
        &connection,
        2,
        Method::FsWatch {
            session_id: fixture.sessions[0].id.clone(),
            watch_id: "drain-watch".into(),
            path: fixture.workspaces[0].to_str().unwrap().into(),
        },
    )
    .await;
    let mut params = parameters(
        &fixture,
        &["/bin/sh", "-c", "sleep 600 & printf '%s\\n' \"$!\"; wait"],
    );
    params.process_id = Some("draining".into());
    params.stream_stdout_stderr = true;
    params.timeout_ms = Some(60_000);
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
    let pid: i32 = String::from_utf8(output(&connection, "draining").await)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // This shutdown guard is shorter than the command's 60-second deadline.
    // Drain must cancel the process before waiting for its request owner.
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), fixture.host.drain())
            .await
            .expect("host drain cancels independent work before its execution deadline"),
        [] as [std::string::String; 0]
    );
    process_gone(pid);
    match bounded(command).await.unwrap() {
        Response::Success(success) => assert!(matches!(success.result,
            MethodResult::CommandExec { result } if result.status == ExecStatus::Terminated)),
        // A fenced endpoint mid-operation reports unavailability; a
        // closed request context reports ConnectionClosed — both honest.
        Response::Failure(failure) => assert!(matches!(
            failure.error.data,
            Some(ErrorData::ConnectionClosed | ErrorData::ExecutionUnavailable)
        )),
    }
    execution.kill().await.unwrap();
    assert_eq!(*fixture.host.request_operations().borrow(), 0);
    // Pending watch publication cannot prevent physical Client drain.
    assert!(fixture.host.file_observers().available_permits() >= 31);
    connection.close();
    bounded(fixture.host.settle_file_observers()).await;
    assert!(fixture.host.finish_drain().is_ok());
}

#[cfg(feature = "mcp-fixture")]
pub(crate) async fn mcp_admission_scenario() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::tools::mcp::{
        catalog::CatalogKind,
        fixture::streamable_http::{HttpFixture, HttpFixtureControl},
        test_sync::CloseProbe,
    };
    use futures_util::FutureExt as _;
    for (catalog, failure, disconnect) in [
        (false, false, true),
        (true, false, true),
        (false, true, false),
        (true, true, false),
        (false, true, true),
        (true, true, true),
    ] {
        let fixture = Fixture::with_session_count(None, 0).await;
        let native = HttpFixture::start(HttpFixtureControl::default()).await;
        let text = format!("[mcp_servers.held]\nurl = {:?}\n", native.endpoint);
        let root = fixture
            .manager
            .configuration
            .resource_root(&SourceTarget::User);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mcp.toml"), &text).unwrap();
        let revision = crate::local_runtime::settings::revision(Some(text.as_bytes()));
        let close = Arc::new(if failure {
            CloseProbe::parking_with_failure("injected unproven management cleanup")
        } else {
            CloseProbe::parking()
        });
        crate::local_runtime::mcp_probe::install_close_probe(
            fixture.manager.configuration.sources.config_path.clone(),
            close.clone(),
        );
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        initialize(&connection).await;
        let worker = connection.clone();
        let request = tokio::spawn(async move {
            let call = if catalog {
                Method::McpCatalog {
                    target: SourceTarget::User,
                    id: crate::runtime::identity::McpServerId::new("held"),
                    expected_revision: revision,
                    kind: CatalogKind::Tools,
                }
            } else {
                Method::McpProbe {
                    target: SourceTarget::User,
                    id: crate::runtime::identity::McpServerId::new("held"),
                    expected_revision: revision,
                }
            };
            worker
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(1),
                    call,
                })
                .await
        });
        bounded(close.wait_entered()).await;
        let failures = if disconnect {
            connection.close();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert_eq!(*fixture.host.request_operations().borrow(), 1);
            let mut drain = Box::pin(fixture.host.drain());
            assert!(
                drain.as_mut().now_or_never().is_none(),
                "physical close still owns host admission"
            );
            close.release();
            bounded(drain).await
        } else {
            close.release();
            let Response::Success(success) = bounded(request).await.unwrap() else {
                panic!("management result");
            };
            assert!(
                matches!(success.result, MethodResult::McpProbe { result } if result.outcome == crate::local_runtime::mcp_probe::McpProbeOutcome::SettlementFailed)
            );
            connection.close();
            bounded(fixture.host.drain()).await
        };
        assert_eq!(failures.is_empty(), !failure);
        assert_eq!(*fixture.host.request_operations().borrow(), 0);
        assert_eq!(fixture.manager.mcp_probes.is_settled(), !failure);
        assert_eq!(fixture.host.finish_drain().is_ok(), !failure);
        if failure {
            assert!(
                fixture
                    .host
                    .forced_resources(false)
                    .contains("physical settlement unproven")
            );
        }
    }
}

#[cfg(feature = "mcp-fixture")]
pub(crate) async fn mcp_capacity_scenario() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::tools::mcp::{
        fixture::streamable_http::{HttpFixture, HttpFixtureControl},
        test_sync::CloseProbe,
    };
    let fixture = Fixture::with_session_count(None, 0).await;
    let independent = Fixture::with_session_count(None, 0).await;
    let native = HttpFixture::start(HttpFixtureControl::default()).await;
    let text = format!("[mcp_servers.held]\nurl = {:?}\n", native.endpoint);
    for instance in [&fixture, &independent] {
        let root = instance
            .manager
            .configuration
            .resource_root(&SourceTarget::User);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mcp.toml"), &text).unwrap();
    }
    let revision = crate::local_runtime::settings::revision(Some(text.as_bytes()));
    let method = || Method::McpProbe {
        target: SourceTarget::User,
        id: crate::runtime::identity::McpServerId::new("held"),
        expected_revision: revision.clone(),
    };
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let mut active = Vec::new();
    for id in 1..=4 {
        let close = Arc::new(CloseProbe::parking());
        crate::local_runtime::mcp_probe::install_close_probe(
            fixture.manager.configuration.sources.config_path.clone(),
            close.clone(),
        );
        let worker = connection.clone();
        let method = method();
        let completion = tokio::spawn(async move { call(&worker, id, method).await });
        bounded(close.wait_entered()).await;
        active.push((close, completion));
    }
    assert_eq!(
        rejected(&connection, 5, method()).await,
        ErrorData::InvalidParams
    );
    let other = AppServerConnection::new(independent.host.clone());
    initialize(&other).await;
    assert!(
        matches!(call(&other, 6, method()).await, MethodResult::McpProbe { result }
        if result.outcome == crate::local_runtime::mcp_probe::McpProbeOutcome::Reachable)
    );
    for (close, completion) in active {
        close.release();
        bounded(completion).await.unwrap();
    }
    assert!(
        matches!(call(&connection, 7, method()).await, MethodResult::McpProbe { result }
        if result.outcome == crate::local_runtime::mcp_probe::McpProbeOutcome::Reachable)
    );
    connection.close();
    other.close();
}

pub(crate) async fn duplex_terminal_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let mut params = parameters(
        &fixture,
        &[
            "/bin/sh",
            "-c",
            "stty raw -echo; printf READY; head -c 131072 /dev/zero; head -c 65536 >/dev/null; printf DONE",
        ],
    );
    params.process_id = Some("duplex".into());
    params.tty = true;
    params.output_bytes_cap = Some(262_144);
    let worker = connection.clone();
    let command = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    let first = output(&connection, "duplex").await;
    assert!(first.starts_with(b"READY"));
    let worker = connection.clone();
    let write = tokio::spawn(async move {
        call(
            &worker,
            2,
            Method::CommandWrite(WriteParams {
                process_id: "duplex".into(),
                delta_base64: Some(
                    base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 65_536]),
                ),
                close_stdin: false,
            }),
        )
        .await
    });
    tokio::pin!(command);
    let mut output = first;
    let final_result = bounded(async {
        loop { tokio::select! {
            result = &mut command => break result.unwrap(),
            notification = connection.next_notification() => if let NotificationMethod::CommandOutputDelta(delta) = notification.notification {
                output.extend(base64::engine::general_purpose::STANDARD.decode(delta.delta_base64).unwrap());
            },
        } }
    }).await;
    assert!(matches!(
        bounded(write).await.unwrap(),
        MethodResult::EnvironmentAccepted {}
    ));
    let MethodResult::CommandExec { result } = final_result else {
        panic!("duplex terminal result")
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert!(output.ends_with(b"DONE"));
    assert_eq!(output.len(), 5 + 131_072 + 4);
    execution.kill().await.unwrap();
    connection.close();
}

pub(crate) async fn skill_admission_scenario() {
    use crate::local_runtime::{
        configuration::settings::SourceTarget,
        skill_management::{SkillContentKind, install_native_gate},
    };
    let fixture = Fixture::with_session_count(None, 0).await;
    for method in [
        Method::SkillsList {
            execution_session: None,
            target: SourceTarget::User,
            offset: 0,
            limit: 1,
            expected_revision: None,
        },
        Method::SkillsRead {
            execution_session: None,
            target: SourceTarget::User,
            name: "absent".into(),
            kind: SkillContentKind::Instructions,
            offset: 0,
            max_bytes: 1,
            expected_revision: None,
        },
    ] {
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        initialize(&connection).await;
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        install_native_gate(
            fixture.manager.configuration.sources.config_path.clone(),
            (entered, released),
        );
        let worker = connection.clone();
        let request = tokio::spawn(async move {
            worker
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(1),
                    call: method,
                })
                .await
        });
        bounded(started).await.unwrap();
        connection.close();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let mut operations = fixture.host.request_operations();
        assert_eq!(
            *operations.borrow(),
            1,
            "native read retains host admission after disconnect"
        );
        release.send(()).unwrap();
        bounded(operations.wait_for(|count| *count == 0))
            .await
            .unwrap();
    }
}

pub(crate) async fn skill_large_resources_scenario() {
    use crate::local_runtime::{
        configuration::settings::SourceTarget, skill_management::SkillContentKind,
    };
    let fixture = Fixture::with_session_count(None, 0).await;
    let directory = fixture.workspaces[0].join(".agents/skills/large");
    std::fs::create_dir_all(&directory).unwrap();
    let license = "L".repeat(240_000);
    let description = "界".repeat(50_000);
    let header = format!("---\nname: large\ndescription: {description}\nlicense: {license}\n---\n");
    let file = directory.join("SKILL.md");
    std::fs::write(&file, &header).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(32 * 1024 * 1024)
        .unwrap();
    for (folder, name) in [
        ("too-large", "m".repeat(70_000)),
        ("normal", "normal".into()),
    ] {
        let directory = directory.with_file_name(folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Normal descriptor\n---\n"),
        )
        .unwrap();
    }
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let target = SourceTarget::Workspace {
        directory: fixture.workspaces[0].clone(),
    };
    let MethodResult::Skills { page } = call(
        &connection,
        1,
        Method::SkillsList {
            execution_session: None,
            target: target.clone(),
            offset: 0,
            limit: 1,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("catalog")
    };
    assert_eq!(page.data.len(), 1);
    assert!(crate::bounded_file::json_fits(&page, 32_768));
    assert_eq!(page.data[0].catalog.name, "large");
    let preview = page.data[0].catalog.description.strip_suffix('…').unwrap();
    assert!(!preview.is_empty() && description.starts_with(preview));
    assert_eq!(page.next_offset, Some(1));
    let MethodResult::Skills { page: last } = call(
        &connection,
        1,
        Method::SkillsList {
            execution_session: None,
            target: target.clone(),
            offset: 1,
            limit: 32,
            expected_revision: Some(page.revision.clone()),
        },
    )
    .await
    else {
        panic!("last catalog page")
    };
    assert_eq!(last.data.len(), 1);
    assert_eq!(last.data[0].catalog.name, "normal");
    assert_eq!(last.next_offset, None);
    let mut metadata = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::SkillContent { content } = call(
            &connection,
            2,
            Method::SkillsRead {
                execution_session: None,
                target: target.clone(),
                name: "large".into(),
                kind: SkillContentKind::Metadata,
                offset,
                max_bytes: 65_536,
                expected_revision: Some(page.data[0].metadata_revision.clone()),
            },
        )
        .await
        else {
            panic!("metadata")
        };
        assert!(crate::bounded_file::json_fits(&content, 131_072));
        metadata.extend(
            base64::engine::general_purpose::STANDARD
                .decode(content.data_base64)
                .unwrap(),
        );
        let Some(next) = content.next_offset else {
            break;
        };
        offset = next;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata["license"], license);
    assert_eq!(metadata["description"], description);
    let MethodResult::SkillContent { content } = call(
        &connection,
        3,
        Method::SkillsRead {
            execution_session: None,
            target,
            name: "large".into(),
            kind: SkillContentKind::Instructions,
            offset: 31 * 1024 * 1024,
            max_bytes: 16,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("body")
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(content.data_base64)
            .unwrap(),
        [0; 16]
    );
    assert_eq!(content.next_offset, Some(31 * 1024 * 1024 + 16));
    connection.close();
}

pub(crate) async fn released_output_preserves_the_unclaimed_input_receipt() {
    use crate::app_server::command::{ReleaseOutcome, hold_input_receipt, observe_release};

    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let process_id = "output-release-before-input-receipt";
    let mut params = parameters(&fixture, &["/bin/cat"]);
    params.process_id = Some(process_id.into());
    params.stream_stdin = true;
    let (held, resume) = hold_input_receipt(process_id);
    let release = observe_release(process_id);

    let driver = connection.clone();
    let mut command =
        tokio::spawn(async move { call(&driver, 1, Method::CommandExec(params)).await });
    admit_or_fail(&connection, process_id, &mut command).await;
    let input = tokio::spawn({
        let connection = connection.clone();
        async move {
            call(
                &connection,
                2,
                Method::CommandWrite(WriteParams {
                    process_id: process_id.into(),
                    delta_base64: Some(
                        base64::engine::general_purpose::STANDARD.encode(b"before EOF"),
                    ),
                    close_stdin: true,
                }),
            )
            .await
        }
    });
    bounded(held).await.unwrap();
    let MethodResult::CommandExec { result } = bounded(command).await.unwrap() else {
        panic!("command result");
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
        b"before EOF"
    );
    assert_eq!(release.outcomes().last(), Some(&ReleaseOutcome::Released));
    assert!(!input.is_finished());
    resume.send(()).unwrap();
    assert!(matches!(
        bounded(input).await.unwrap(),
        MethodResult::EnvironmentAccepted {}
    ));
    connection.close();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    assert!(bounded(execution.wait()).await.unwrap().success());
}

pub(crate) async fn partial_stdin_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let mut execution = fixture.start_execution(0).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let mut params = parameters(
        &fixture,
        &[
            "/bin/sh",
            "-c",
            "stty raw -echo; printf READY; head -c 1 >/dev/null",
        ],
    );
    params.process_id = Some("prefix".into());
    params.tty = true;
    let worker = connection.clone();
    let command = tokio::spawn(async move { call(&worker, 1, Method::CommandExec(params)).await });
    assert!(output(&connection, "prefix").await.starts_with(b"READY"));
    // The receipt is the physical answer: the 64 KiB write can fully
    // apply into the PTY buffer before the one-byte reader exits, or
    // the peer can be gone already — both outcomes are honest, a partial
    // application is never reported as success.
    match connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(2),
            call: Method::CommandWrite(WriteParams {
                process_id: "prefix".into(),
                delta_base64: Some(
                    base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 65536]),
                ),
                close_stdin: false,
            }),
        })
        .await
    {
        Response::Success(success) => assert!(matches!(
            success.result,
            MethodResult::EnvironmentAccepted {}
        )),
        Response::Failure(failure) => {
            assert_eq!(failure.error.data, Some(ErrorData::OutcomeUnknown));
        }
    }
    assert!(matches!(
        bounded(command).await.unwrap(),
        MethodResult::CommandExec { .. }
    ));
    execution.kill().await.unwrap();
    connection.close();
}

#[cfg(target_os = "linux")]
pub(crate) async fn idle_root_watch_scenario() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let _execution = fixture.start_execution(0).await;
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let root = fixture.workspaces[0].clone();
    call(
        &connection,
        1,
        Method::FsWatch {
            session_id: fixture.sessions[0].id.clone(),
            watch_id: "idle".into(),
            path: root.display().to_string(),
        },
    )
    .await;
    let session_id = fixture.sessions[0].id.clone();
    let page = call(
        &connection,
        4,
        Method::FsReadDirectory {
            session_id,
            path: root.display().to_string(),
            offset: 0,
            limit: 16,
            expected_revision: None,
        },
    )
    .await;
    assert!(matches!(page, MethodResult::DirectoryPage { .. }));
    assert!(connection.environment_events_idle_for_test());
    let observed_file = root.join("first-real-change");
    std::fs::write(&observed_file, b"changed").unwrap();
    loop {
        if let NotificationMethod::FsChanged(change) =
            bounded(connection.next_notification()).await.notification
        {
            assert_eq!(change.watch_id, "idle");
            assert!(!change.retired);
            assert!(!change.resync_required);
            assert!(
                change
                    .changed_paths
                    .iter()
                    .all(|path| path == &observed_file)
            );
            if change.changed_paths.contains(&observed_file) {
                break;
            }
        }
    }
    assert!(matches!(
        call(&connection, 2, Method::ServerInfo {}).await,
        MethodResult::ServerInfo { .. }
    ));
    call(
        &connection,
        3,
        Method::FsUnwatch {
            watch_id: "idle".into(),
        },
    )
    .await;
    let capacity = fixture.host.file_observers();
    let permits = bounded(capacity.acquire_many(32)).await.unwrap();
    drop(permits);
    connection.close();
}

pub(crate) async fn skill_catalog_page_scenario() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    let fixture = Fixture::with_session_count(None, 0).await;
    let mut workspace = fixture.workspaces[0].clone();
    for _ in 0..2 {
        workspace.push("d".repeat(200));
    }
    let collection = workspace.join(".agents/skills");
    for index in 0..32 {
        let name = format!("skill-{index:02}");
        let directory = collection.join(&name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {}\n---\nbody\n",
                "😀".repeat(1024)
            ),
        )
        .unwrap();
    }
    let target = SourceTarget::Workspace {
        directory: workspace,
    };
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let mut offset = 0;
    let mut revision = None;
    let mut total = 0;
    let mut pages = 0;
    loop {
        let MethodResult::Skills { page } = call(
            &connection,
            1,
            Method::SkillsList {
                execution_session: None,
                target: target.clone(),
                offset,
                limit: 32,
                expected_revision: revision.clone(),
            },
        )
        .await
        else {
            panic!("catalog page")
        };
        assert!(crate::bounded_file::json_fits(&page.data, 131_072));
        assert_ne!(
            page.data,
            [] as [crate::local_runtime::skill_management::SkillDescriptor; 0]
        );
        total += page.data.len();
        pages += 1;
        revision = Some(page.revision);
        let Some(next) = page.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(total, 32);
    assert!(pages > 1, "byte budget trims a count-valid page");
    connection.close();
}

/// Agent Read through the real composition chain: a Session loaded with
/// an admitted Execution Client serves `read` through the
/// `SessionExecutionScope` `SessionRuntimeManager::load` bound into its
/// composed workspace, while another Session — loaded through the same
/// path but never given an execution reservation — reports the honest
/// unavailable diagnostic for the identical lexical path. The scope is a
/// per-dispatch lookup, never a claim: no Client is spawned or prepared
/// for the unbound Session, and there is no Server-local read to fall
/// back to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_read_dispatches_only_through_the_bound_session_scope() {
    bounded(async {
        let fixture = Fixture::with_session_count(Some("read"), 2).await;
        // The identical lexical name under each admitted workspace,
        // carrying distinct physical bodies.
        std::fs::write(
            fixture.workspaces[0].join("note.txt"),
            "bound-session-bytes\n",
        )
        .unwrap();
        std::fs::write(
            fixture.workspaces[1].join("note.txt"),
            "unbound-session-bytes\n",
        )
        .unwrap();

        // Session A: the real reservation, the real staged Client child,
        // and a real `SessionRuntimeManager::load` — the production route
        // that binds `execution_scope` into the composed Workspace.
        let mut execution = fixture.start_execution(0).await;
        let managed = fixture.load(0).await.unwrap().unwrap();
        // Session B: the same load path, deliberately without any
        // execution reservation.
        let unbound = fixture.load(1).await.unwrap().unwrap();
        assert!(
            fixture
                .manager
                .execution_endpoint(&fixture.sessions[1].id)
                .is_err(),
            "Session B keeps no endpoint claim: no Client was ever prepared"
        );

        // The bound Session returns the exact model-facing bytes — only
        // the admitted Client for Session A's workspace can produce them.
        let bound_runtime = managed.inspect_runtime().expect("resident runtime A");
        let bound = read_page_in_session(&bound_runtime, "read-bound").await;
        assert_eq!(
            bound.status,
            crate::tools::types::ToolExecutionStatus::Success
        );
        let Some(crate::tools::types::ToolResultContent::Text(text)) = bound.content.first() else {
            panic!("expected a textual page: {bound:?}");
        };
        assert_eq!(text.text, "bound-session-bytes\n");

        // The unbound Session — same lexical path, physically present —
        // gets the honest unavailable diagnostic, never a local read.
        let unbound_runtime = unbound.inspect_runtime().expect("resident runtime B");
        let refused = read_page_in_session(&unbound_runtime, "read-unbound").await;
        let crate::tools::types::ToolExecutionStatus::Failed { error } = refused.status else {
            panic!("the unbound Session must not serve a read: {refused:?}");
        };
        assert!(
            error.contains("Session execution unavailable"),
            "unexpected diagnostic: {error}"
        );
        assert!(
            !error.contains("unbound-session-bytes") && !error.contains("bound-session-bytes"),
            "no file body may leak through the refused route: {error}"
        );

        let _ = execution.kill().await;
        fixture.close().await;
    })
    .await;
}

/// One `read` dispatch through the composed root Agent tool boundary —
/// the resident runtime's own Workspace, artifacts, managed output, and
/// environment, exactly the `ToolExecutionContext` the resident tool
/// plane builds for a real tool call.
async fn read_page_in_session(
    runtime: &crate::runtime::ConversationRuntime,
    call: &str,
) -> crate::tools::types::ToolExecutionResult {
    struct Silent;
    impl crate::tools::executor::ProgressReporter for Silent {
        fn report(&self, _progress: crate::tools::types::ToolProgress) {}
    }
    let progress = Silent;
    let tool_runtime = runtime.tool_runtime();
    let mut registry = crate::tools::executor::ToolRegistry::new();
    crate::tools::native::register_native_tools(
        &mut registry,
        crate::tools::native::NativeToolResources {
            subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
            background: tool_runtime.background().clone(),
            subagents: None,
        },
        crate::tools::native::NativeToolPolicies::default(),
    )
    .expect("native registry");
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new(call),
        },
        tool_id: crate::runtime::identity::ToolId::new("tool-read"),
        tool_name: "read".to_owned(),
        mode: crate::tools::types::ToolInvocationMode::Foreground,
        arguments: serde_json::json!({"path": "note.txt"}),
    };
    let context = crate::tools::executor::ToolExecutionContext {
        stdout_capture: None,
        workspace_changes: None,
        model_invocation: None,
        attempt_id: None,
        goal: None,
        schedule: None,
        conversation_id: tool_runtime.conversation_id(),
        execution_id: None,
        cancellation: crate::runtime::ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        ),
        workspace: tool_runtime.workspace(),
        progress: &progress,
        artifacts: tool_runtime.artifacts(),
        tool_output: tool_runtime.tool_output(),
        environment: tool_runtime.environment(),
        questionnaire_requester: None,
        todos: None,
        subagent: None,
        bound_mutation: None,
    };
    registry
        .executor(&crate::runtime::identity::ToolId::new("tool-read"))
        .start(invocation, context)
        .completion
        .await
}
