use super::*;

#[test]
fn admitted_command_does_not_wait_for_revision_writer() {
    let commands = Arc::new(Commands::default());
    let (control, _controls) = mpsc::channel(1);
    commands.state.lock().unwrap().active.insert(
        "admitted".into(),
        Entry {
            cancellation: CancellationSignal::new(),
            control,
        },
    );
    let (entered, writing) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = commands.clone();
    let writer = std::thread::spawn(move || {
        writer.revisions.send_modify(|revision| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            *revision += 1;
        });
    });
    writing.recv().unwrap();
    let (done, completed) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(commands.admitted_for_test("admitted"));
        done.send(()).unwrap();
    });
    let observed = completed.recv_timeout(std::time::Duration::from_secs(2));
    release.send(()).unwrap();
    writer.join().unwrap();
    waiter.join().unwrap();
    assert!(
        observed.is_ok(),
        "native admission is observable while its revision publication is blocked"
    );
}

fn parameters() -> ExecParams {
    ExecParams {
        command: vec!["unused".into()],
        cwd: "/".into(),
        env: BTreeMap::new(),
        process_id: Some("fragmented".into()),
        tty: true,
        stream_stdin: false,
        stream_stdout_stderr: false,
        output_bytes_cap: None,
        timeout_ms: None,
        size: None,
    }
}

#[tokio::test]
async fn tiny_reads_batch_with_no_consumer_progress_and_preserve_every_byte() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let cap = 262_144;
    assert!(
        output
            .push(
                b"READY",
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_none()
    );
    let mut last_receipt = output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap();
    // Deliberately withhold every receipt while the native producer makes
    // 131,072 one-byte reads. Scheduling cannot rescue a per-read queue here.
    for _ in 0..131_072 {
        if let Some(receipt) = output
            .push(
                &[0],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap()
        {
            last_receipt = Some(receipt);
        }
        assert!(output.pending.len() < OUTPUT_CHUNK_BYTES);
    }
    assert_eq!(sequence, 17);
    assert!(!events.overloaded.is_cancelled());
    output
        .push(
            b"DONE",
            cap,
            OutputStream::Stdout,
            &params,
            &events,
            &mut sequence,
        )
        .unwrap();
    assert_eq!(sequence, 17, "the short tail awaits progress or EOF");
    last_receipt = output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap()
        .or(last_receipt);
    assert_eq!(sequence, 18);
    let mut received = Vec::new();
    for expected in 1..=sequence {
        let event = events.next().await;
        let NotificationMethod::CommandOutputDelta(delta) = event.notification.notification else {
            panic!("command output")
        };
        assert_eq!(delta.sequence, expected);
        assert!(!delta.cap_reached);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(delta.delta_base64)
            .unwrap();
        assert!(bytes.len() <= OUTPUT_CHUNK_BYTES);
        received.extend(bytes);
        let _ = event.emitted.send(());
    }
    assert_eq!(&received[..5], b"READY");
    assert!(received[5..5 + 131_072].iter().all(|byte| *byte == 0));
    assert_eq!(&received[5 + 131_072..], b"DONE");
    assert!(last_receipt.unwrap().await.is_ok());
}

#[test]
fn full_chunks_still_retire_the_original_saturated_queue() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let bytes = [0; OUTPUT_CHUNK_BYTES];
    for _ in 0..32 {
        output
            .push(
                &bytes,
                usize::MAX,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap();
    }
    assert!(!events.overloaded.is_cancelled());
    assert!(
        output
            .push(
                &bytes,
                usize::MAX,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .is_err()
    );
    assert!(events.overloaded.is_cancelled());
}

#[tokio::test]
async fn exact_cap_flushes_a_short_tail_and_overflow_emits_one_empty_marker() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    assert!(
        output
            .push(
                b"12345678",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_some()
    );
    let first = events.next().await;
    let NotificationMethod::CommandOutputDelta(first) = first.notification.notification else {
        panic!("output")
    };
    assert_eq!(first.sequence, 1);
    assert!(!first.cap_reached);
    assert_eq!(first.delta_base64, "MTIzNDU2Nzg=");
    assert!(
        output
            .push(
                b"overflow",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_some()
    );
    let second = events.next().await;
    let NotificationMethod::CommandOutputDelta(second) = second.notification.notification else {
        panic!("output")
    };
    assert_eq!(second.sequence, 2);
    assert!(second.cap_reached);
    assert_eq!(second.delta_base64, "");
    assert!(
        output
            .push(
                b"discard",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(sequence, 2);
    assert_eq!(output.pending, [] as [u8; 0]);
}

#[test]
fn reaching_the_cap_cannot_hide_a_saturated_consumer_behind_a_pending_tail() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let cap = 262_144;
    output
        .push(
            &[0],
            cap,
            OutputStream::Stdout,
            &params,
            &events,
            &mut sequence,
        )
        .unwrap();
    output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap();
    // The first short frame and 31 full frames fill the original 32 slots.
    for _ in 0..31 {
        output
            .push(
                &[0; OUTPUT_CHUNK_BYTES],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap();
    }
    assert_eq!(sequence, 32);
    assert!(!events.overloaded.is_cancelled());
    // No EOF, acknowledgement or timer may be needed to flush the cap tail.
    assert!(
        output
            .push(
                &[0; OUTPUT_CHUNK_BYTES - 1],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .is_err()
    );
    assert_eq!(output.total, cap);
    assert_eq!(output.pending, [] as [u8; 0]);
    assert!(events.overloaded.is_cancelled());
}

fn command_fixture_root() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let alias = temporary.path().join("workspace-alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    // Native grants reject symlink components. Exercise the macOS temporary
    // directory alias precondition on every platform, then grant its real path.
    let root = alias.canonicalize().unwrap();
    (temporary, root)
}

type CommandSettlement =
    Result<Result<ExecResult, RpcError>, tokio::sync::oneshot::error::RecvError>;

struct ReadyObservation {
    output: Result<Vec<u8>, String>,
    settlement: Option<CommandSettlement>,
}

async fn wait_for_ready_output(
    events: &super::super::service_events::Events,
    process_id: &str,
    received: &mut oneshot::Receiver<Result<ExecResult, RpcError>>,
) -> ReadyObservation {
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut output = Vec::new();
        while output.len() < b"READY".len() {
            let event = events.next().await;
            let _ = event.emitted.send(());
            let NotificationMethod::CommandOutputDelta(delta) = event.notification.notification
            else {
                return Err("expected command output delta".to_owned());
            };
            if delta.process_id != process_id {
                return Err(format!("unexpected process identity: {}", delta.process_id));
            }
            output.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(delta.delta_base64)
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(output)
    });
    tokio::select! {
        result = output => ReadyObservation {
            output: result.map_err(|error| error.to_string()).and_then(|result| result),
            settlement: None,
        },
        settlement = received => ReadyObservation {
            output: Err(format!("command settled before READY: {settlement:?}")),
            settlement: Some(settlement),
        },
    }
}

#[tokio::test]
async fn start_releases_state_mutex_before_watch_publication() {
    let (_temporary, root) = command_fixture_root();
    let environment = Arc::new(crate::local_runtime::environment::Environment::default());
    environment.configure(std::slice::from_ref(&root));
    environment
        .directory(&root.display().to_string())
        .expect("canonical fixture root must be admitted");
    let events = Arc::new(super::super::service_events::Events::default());
    let commands = Arc::new(Commands::default());
    let params = ExecParams {
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf READY; exec /bin/cat".into(),
        ],
        cwd: root.display().to_string(),
        env: BTreeMap::new(),
        process_id: Some("lock-order".into()),
        tty: false,
        stream_stdin: true,
        stream_stdout_stderr: true,
        output_bytes_cap: None,
        timeout_ms: Some(10_000),
        size: None,
    };

    let witness_rx = commands.arm_pre_publish_for_test();
    let watch_rx = commands.watch_receiver_for_test();
    let watch_read_guard = watch_rx.borrow();

    let commands_for_start = commands.clone();
    let environment_for_start = environment.clone();
    let events_for_start = events.clone();
    let start_task = tokio::task::spawn_blocking(move || {
        commands_for_start.start(
            params,
            environment_for_start,
            events_for_start,
            super::super::host::ServerOperation::for_test(),
        )
    });

    witness_rx.await.expect("pre-publish witness fired");
    let state_try = commands.state.try_lock();
    let state_available = state_try.is_ok();
    let entry_inserted = state_try
        .as_ref()
        .is_ok_and(|state| state.active.contains_key("lock-order"));
    drop(state_try);

    drop(watch_read_guard);
    let mut received = start_task
        .await
        .expect("start blocking task")
        .expect("Commands::start admitted command");

    let active_after_start = commands
        .state
        .lock()
        .expect("connection commands")
        .active
        .contains_key("lock-order");
    let ready = wait_for_ready_output(&events, "lock-order", &mut received).await;
    if ready.output.is_err() {
        events.closed.cancel();
    }
    let terminate = commands.terminate("lock-order");
    let settled = match ready.settlement {
        Some(settled) => settled,
        None => received.await,
    };
    commands.close();
    let active_after_settle = commands
        .state
        .lock()
        .expect("connection commands")
        .active
        .is_empty();

    assert_eq!(
        ready.output.as_deref(),
        Ok(b"READY".as_slice()),
        "READY observation failed; command settlement: {settled:?}"
    );
    assert!(
        terminate.is_ok(),
        "terminate active command must succeed: {terminate:?}"
    );
    let exec_result = settled.expect("result channel").expect("exec result");
    assert_eq!(exec_result.status, ExecStatus::Terminated);
    assert!(
        active_after_settle,
        "settled command must clear active entry"
    );
    assert!(
        active_after_start,
        "Commands::start must retain active entry lock-order"
    );
    assert!(
        state_available,
        "Commands::start must release State mutex before publishing to watch"
    );
    assert!(
        entry_inserted,
        "active entry lock-order must be inserted before pre-publish witness"
    );
}

#[tokio::test]
async fn admitted_for_test_observes_concurrent_start_without_holding_watch_across_state() {
    let (_temporary, root) = command_fixture_root();
    let environment = Arc::new(crate::local_runtime::environment::Environment::default());
    environment.configure(std::slice::from_ref(&root));
    environment
        .directory(&root.display().to_string())
        .expect("canonical fixture root must be admitted");
    let events = Arc::new(super::super::service_events::Events::default());
    let commands = Arc::new(Commands::default());
    let params = ExecParams {
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf READY; exec /bin/cat".into(),
        ],
        cwd: root.display().to_string(),
        env: BTreeMap::new(),
        process_id: Some("concurrent-admit".into()),
        tty: false,
        stream_stdin: true,
        stream_stdout_stderr: true,
        output_bytes_cap: None,
        timeout_ms: Some(10_000),
        size: None,
    };

    let observer = commands.admitted_for_test("concurrent-admit");
    tokio::pin!(observer);
    let pending_before_start = futures_util::poll!(observer.as_mut()).is_pending();

    let mut received = commands
        .start(
            params,
            environment,
            events.clone(),
            super::super::host::ServerOperation::for_test(),
        )
        .expect("Commands::start admitted command");

    let observer_result = tokio::time::timeout(std::time::Duration::from_secs(5), observer).await;
    let active_when_admitted = commands
        .state
        .lock()
        .expect("connection commands")
        .active
        .contains_key("concurrent-admit");
    let ready = wait_for_ready_output(&events, "concurrent-admit", &mut received).await;
    if ready.output.is_err() {
        events.closed.cancel();
    }
    let terminate = commands.terminate("concurrent-admit");
    let settled = match ready.settlement {
        Some(settled) => settled,
        None => received.await,
    };
    commands.close();
    let active_after_settle = commands
        .state
        .lock()
        .expect("connection commands")
        .active
        .is_empty();

    assert_eq!(
        ready.output.as_deref(),
        Ok(b"READY".as_slice()),
        "READY observation failed; command settlement: {settled:?}"
    );
    assert!(
        terminate.is_ok(),
        "terminate concurrent command: {terminate:?}"
    );
    let exec_result = settled.expect("result channel").expect("exec result");
    assert_eq!(exec_result.status, ExecStatus::Terminated);
    assert!(active_after_settle);
    assert!(
        pending_before_start,
        "admitted_for_test must return Pending when polled before Commands::start"
    );
    assert!(
        observer_result.is_ok(),
        "admitted_for_test must wake and settle after Commands::start without deadlock"
    );
    assert!(active_when_admitted);
}

#[tokio::test]
async fn readiness_observes_ungranted_root_failure_and_settles_before_assertions() {
    let (_temporary, root) = command_fixture_root();
    // Deliberately leave the real root ungranted. The actual command actor
    // must reject it before spawning, rather than leaving READY waiting alone.
    let environment = Arc::new(crate::local_runtime::environment::Environment::default());
    let events = Arc::new(super::super::service_events::Events::default());
    let commands = Arc::new(Commands::default());
    let mut params = parameters();
    params.command = vec!["/bin/sh".into(), "-c".into(), "printf READY".into()];
    params.cwd = root.display().to_string();
    params.process_id = Some("ungranted-root".into());
    params.tty = false;
    params.stream_stdin = true;
    params.stream_stdout_stderr = true;
    params.timeout_ms = Some(10_000);
    let mut received = commands
        .start(
            params,
            environment,
            events.clone(),
            super::super::host::ServerOperation::for_test(),
        )
        .expect("command admission");
    let ready = wait_for_ready_output(&events, "ungranted-root", &mut received).await;
    let observed_settlement = ready.settlement.is_some();
    events.closed.cancel();
    commands.close();
    let settled = match ready.settlement {
        Some(settled) => settled,
        None => received.await,
    };
    let active_after_settle = commands
        .state
        .lock()
        .expect("connection commands")
        .active
        .is_empty();

    assert!(
        observed_settlement,
        "native result must win before READY timeout"
    );
    assert!(ready.output.is_err());
    assert_eq!(
        settled.expect("result channel"),
        Err(domain(ErrorData::EnvironmentUnauthorized))
    );
    assert!(active_after_settle);
}
