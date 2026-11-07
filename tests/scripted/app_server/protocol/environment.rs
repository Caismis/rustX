//! Environment scenarios use real descriptors/processes; boundary wrappers own CI classification.
use super::*;
use crate::app_server::command::{ExecParams, ExecStatus, WriteParams};
use base64::Engine as _;
use std::path::Path;
use std::sync::Arc;

fn parameters(fixture: &Fixture, command: &[&str]) -> ExecParams {
    serde_json::from_value(serde_json::json!({"command":command,"cwd":fixture.workspaces[0],"env":{},"processId":null,"tty":false,"streamStdin":false,"streamStdoutStderr":false,"outputBytesCap":null,"timeoutMs":10_000,"size":null})).unwrap()
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

pub(crate) async fn buffered_scenario() {
    let fixture = Fixture::new().await;
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let params = parameters(&fixture, &["/usr/bin/printf", "%s", "literal;$(false)"]);
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
        4,
        "only native basics, no provider or App Server credentials"
    );
    assert!(environment.contains("LANG=C.UTF-8"));
    assert_eq!(fixture.manager.diagnostics().loaded, 0);
    connection.close();
}

pub(crate) async fn connection_process_scenario() {
    let fixture = Fixture::new().await;
    grant(&fixture);
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
    let second_result =
        tokio::spawn(async move { call(&second_call, 3, Method::CommandExec(cat)).await });
    second.command_admitted_for_test("same").await;
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
    assert!(result.stdout_base64.is_empty());
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
    assert!(result.stdout_base64.is_empty());
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

pub(crate) async fn terminal_eof_scenario() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
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
    connection.close();
}

pub(crate) async fn watch_registration_scenario() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    for (symlink, capture, directory) in [
        (true, false, true),
        (false, false, true),
        (false, true, true),
        (true, false, false),
        (false, false, false),
    ] {
        let source = fixture.workspaces[0].join("watch-source");
        let saved = fixture.workspaces[0].join("saved-source");
        let outside_root = tempfile::tempdir().unwrap();
        let outside = outside_root.path().join("outside");
        if directory {
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&outside).unwrap();
        } else {
            std::fs::write(&source, b"inside").unwrap();
            std::fs::write(&outside, b"outside").unwrap();
        }
        crate::app_server::filesystem::expect_native_source(
            source.clone(),
            std::fs::metadata(if capture { &outside } else { &source }).unwrap(),
        );
        let (before_source, before_saved, before_outside) =
            (source.clone(), saved.clone(), outside.clone());
        let (after_source, after_saved, after_outside) =
            (source.clone(), saved.clone(), outside.clone());
        let before: Box<dyn FnOnce() + Send> = Box::new(move || {
            std::fs::rename(&before_source, &before_saved).unwrap();
            if symlink {
                std::os::unix::fs::symlink(&before_outside, &before_source).unwrap();
            } else {
                std::fs::rename(&before_outside, &before_source).unwrap();
            }
        });
        let after: Box<dyn FnOnce() + Send> = Box::new(move || {
            if symlink {
                std::fs::remove_file(&after_source).unwrap();
            } else {
                std::fs::rename(&after_source, &after_outside).unwrap();
            }
            std::fs::rename(&after_saved, &after_source).unwrap();
        });
        if capture {
            crate::app_server::filesystem::registration_hooks(
                source.clone(),
                Box::new(|| {}),
                after,
            );
            crate::app_server::filesystem::capture_hook(&source, before);
        } else {
            crate::app_server::filesystem::registration_hooks(source.clone(), before, after);
        }
        let watch = Method::FsWatch {
            watch_id: "inode".into(),
            path: source.display().to_string(),
        };
        if capture {
            assert_eq!(
                rejected(&connection, 1, watch).await,
                ErrorData::EnvironmentUnauthorized
            );
            std::fs::remove_dir_all(&source).unwrap();
            continue;
        }
        call(
            &connection,
            1,
            Method::FsWatch {
                watch_id: "inode".into(),
                path: source.display().to_string(),
            },
        )
        .await;
        // The outside name must never be translated beneath the restored grant.
        if directory {
            std::fs::write(outside.join("outside-private-name"), b"outside").unwrap();
        }
        let sentinel = if directory {
            source.join("authorized-sentinel")
        } else {
            source.clone()
        };
        std::fs::write(&sentinel, b"inside").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let notification = connection.next_notification().await;
                if let NotificationMethod::FsChanged(changed) = notification.notification {
                    assert!(!changed.retired);
                    assert!(
                        changed
                            .changed_paths
                            .iter()
                            .all(|path| path.starts_with(&source))
                    );
                    assert!(!changed.changed_paths.iter().any(|path| {
                        path.file_name()
                            .is_some_and(|name| name == "outside-private-name")
                    }));
                    if changed.changed_paths.contains(&sentinel)
                        || cfg!(target_os = "macos") && changed.resync_required
                    {
                        break;
                    }
                }
            }
        })
        .await
        .expect("only the retained authorized directory must produce the sentinel hint");
        call(
            &connection,
            2,
            Method::FsUnwatch {
                watch_id: "inode".into(),
            },
        )
        .await;
        if directory {
            std::fs::remove_dir_all(&source).unwrap();
        } else {
            std::fs::remove_file(&source).unwrap();
        }
    }
    connection.close();
}

#[cfg(target_os = "macos")]
pub(crate) async fn macos_watch_scenario() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let source = fixture.workspaces[0].join("bounded-watch");
    std::fs::create_dir(&source).unwrap();
    let existing = source.join("existing");
    std::fs::write(&existing, b"before").unwrap();
    let child_directory = source.join("directory");
    std::fs::create_dir(&child_directory).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), source.join("outside-symlink")).unwrap();
    let capacity = fixture.manager.environment.watch_files.clone();
    let _reserved = capacity.clone().acquire_many_owned(1021).await.unwrap();
    let watch = || Method::FsWatch {
        watch_id: "macos".into(),
        path: source.display().to_string(),
    };
    call(&connection, 1, watch()).await;
    assert_eq!(
        capacity.available_permits(),
        1,
        "only the two immediate native sources consume descriptors"
    );
    async fn hint(connection: &AppServerConnection, expected: &Path, retired: bool) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = connection.next_notification().await;
                if let NotificationMethod::FsChanged(changed) = event.notification {
                    assert!(changed.resync_required);
                    if changed.retired == retired
                        && (retired || changed.changed_paths.contains(&expected.to_path_buf()))
                    {
                        break;
                    }
                }
            }
        })
        .await
        .expect("native descriptor event / physical retirement");
    }
    std::fs::write(&existing, b"after").unwrap();
    hint(&connection, &existing, false).await;
    let mode = std::fs::metadata(&child_directory)
        .unwrap()
        .permissions()
        .mode();
    std::fs::set_permissions(
        &child_directory,
        std::fs::Permissions::from_mode(mode ^ 0o100),
    )
    .unwrap();
    hint(&connection, &child_directory, false).await;
    let created = source.join("created");
    std::fs::write(&created, b"new").unwrap();
    hint(&connection, &source, false).await;
    assert_eq!(capacity.available_permits(), 0);
    std::fs::write(&created, b"changed").unwrap();
    hint(&connection, &created, false).await;
    std::fs::remove_file(&created).unwrap();
    hint(&connection, &source, false).await;
    assert_eq!(capacity.available_permits(), 1);
    let renamed = source.join("renamed");
    std::fs::rename(&existing, &renamed).unwrap();
    hint(&connection, &source, false).await;
    std::fs::write(&renamed, b"changed again").unwrap();
    hint(&connection, &renamed, false).await;
    call(
        &connection,
        2,
        Method::FsUnwatch {
            watch_id: "macos".into(),
        },
    )
    .await;
    let all = bounded(capacity.clone().acquire_many_owned(3))
        .await
        .unwrap();
    drop(all);
    // Initial refusal releases every partially registered source.
    std::fs::write(&created, b"third").unwrap();
    let held = capacity.clone().acquire_owned().await.unwrap();
    assert_eq!(
        rejected(&connection, 3, watch()).await,
        ErrorData::EnvironmentCapacityOrConflict
    );
    assert_eq!(capacity.available_permits(), 2);
    drop(held);
    std::fs::remove_file(&created).unwrap();
    call(&connection, 4, watch()).await;
    let held = capacity.clone().acquire_owned().await.unwrap();
    std::fs::write(&created, b"overflow").unwrap();
    hint(&connection, &source, true).await;
    drop(held);
    let all = bounded(capacity.clone().acquire_many_owned(3))
        .await
        .unwrap();
    drop(all);
    connection.close();
}

pub(crate) async fn filesystem_scenario() {
    let fixture = Fixture::new().await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let path = fixture.workspaces[0].join("document").display().to_string();
    let MethodResult::FileMetadata { metadata } = call(
        &connection,
        1,
        Method::FsWriteFile {
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
    let directory = fixture.workspaces[1].join("copy").display().to_string();
    call(
        &connection,
        8,
        Method::FsCreateDirectory {
            path: directory.clone(),
            recursive: true,
        },
    )
    .await;
    call(
        &connection,
        9,
        Method::FsCopy {
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
    call(
        &connection,
        15,
        Method::FsCopy {
            source_path: fixture.workspaces[1].display().to_string(),
            destination_path: copied_root.display().to_string(),
            recursive: true,
        },
    )
    .await;
    assert_eq!(
        std::fs::read(copied_root.join("copy/copy")).unwrap(),
        b"after"
    );
    let inside = format!("{directory}/recursive-cycle");
    assert_eq!(
        rejected(
            &connection,
            16,
            Method::FsCopy {
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
        13,
        Method::FsRemove {
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
                path: fixture.workspaces[0].display().to_string()
            }
        )
        .await,
        ErrorData::EnvironmentUnauthorized
    );
    connection.close();
}

#[tokio::test]
async fn skills_management_keeps_provenance_metadata_and_content_revisions() {
    let fixture = Fixture::with_session_count(None, 0).await;
    let directory = fixture.workspaces[0].join(".agents/skills/inspection");
    std::fs::create_dir_all(&directory).unwrap();
    let body = "---\nname: inspection\ndescription: Inspect native management\nlicense: MIT\n---\nVersion one.\n";
    std::fs::write(directory.join("SKILL.md"), body).unwrap();
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let target = crate::local_runtime::configuration::settings::SourceTarget::Workspace {
        directory: fixture.workspaces[0].clone(),
    };
    let MethodResult::Skills { page } = call(
        &connection,
        1,
        Method::SkillsList {
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
    assert!(!skill.metadata_revision.is_empty());
    assert_eq!(
        skill.provenance.source,
        crate::skills::SkillSource::Workspace
    );
    let MethodResult::SkillContent { content } = call(
        &connection,
        2,
        Method::SkillsRead {
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
    std::fs::write(
        directory.join("SKILL.md"),
        body.replace("Version one", "Version two"),
    )
    .unwrap();
    assert!(matches!(
        rejected(
            &connection,
            3,
            Method::SkillsRead {
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
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
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
    let slow =
        tokio::spawn(async move { rejected(&worker, 3, Method::CommandExec(streamed)).await });
    connection.command_admitted_for_test("slow").await;
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
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    call(
        &connection,
        2,
        Method::FsWatch {
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
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), fixture.host.drain())
            .await
            .expect("host drain cancels independent work before its execution deadline")
            .is_empty()
    );
    process_gone(pid);
    match bounded(command).await.unwrap() {
        Response::Success(success) => assert!(matches!(success.result,
            MethodResult::CommandExec { result } if result.status == ExecStatus::Terminated)),
        Response::Failure(failure) => {
            assert_eq!(failure.error.data, Some(ErrorData::ConnectionClosed));
        }
    }
    assert_eq!(*fixture.host.request_operations().borrow(), 0);
    assert!(
        fixture.host.finish_drain().is_err(),
        "native watch is still owned"
    );
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
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
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
            target: SourceTarget::User,
            offset: 0,
            limit: 1,
            expected_revision: None,
        },
        Method::SkillsRead {
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
    let header =
        format!("---\nname: large\ndescription: Large native resources\nlicense: {license}\n---\n");
    let file = directory.join("SKILL.md");
    std::fs::write(&file, &header).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(32 * 1024 * 1024)
        .unwrap();
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let target = SourceTarget::Workspace {
        directory: fixture.workspaces[0].clone(),
    };
    let MethodResult::Skills { page } = call(
        &connection,
        1,
        Method::SkillsList {
            target: target.clone(),
            offset: 0,
            limit: 32,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("catalog")
    };
    assert_eq!(page.data.len(), 1);
    assert!(crate::bounded_file::json_fits(&page, 32_768));
    let mut metadata = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::SkillContent { content } = call(
            &connection,
            2,
            Method::SkillsRead {
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
    let MethodResult::SkillContent { content } = call(
        &connection,
        3,
        Method::SkillsRead {
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

pub(crate) async fn partial_stdin_scenario() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
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
    let error = rejected(
        &connection,
        2,
        Method::CommandWrite(WriteParams {
            process_id: "prefix".into(),
            delta_base64: Some(base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 65536])),
            close_stdin: false,
        }),
    )
    .await;
    assert_eq!(
        error,
        ErrorData::OutcomeUnknown,
        "a prefix may already have reached the child"
    );
    assert!(matches!(
        bounded(command).await.unwrap(),
        MethodResult::CommandExec { .. }
    ));
    connection.close();
}

#[cfg(target_os = "linux")]
pub(crate) async fn idle_root_watch_scenario() {
    let fixture = Fixture::with_session_count(None, 0).await;
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let root = fixture.workspaces[0].clone();
    call(
        &connection,
        1,
        Method::FsWatch {
            watch_id: "idle".into(),
            path: root.display().to_string(),
        },
    )
    .await;
    let (observed, ignored) = tokio::sync::oneshot::channel();
    crate::app_server::filesystem::observe_access(root.clone(), observed);
    drop(std::fs::read_dir(&root).unwrap());
    bounded(ignored).await.unwrap();
    assert!(
        connection.environment_events_idle_for_test(),
        "native root opens never emit content hints or fill the connection queue"
    );
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
        assert!(!page.data.is_empty());
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
