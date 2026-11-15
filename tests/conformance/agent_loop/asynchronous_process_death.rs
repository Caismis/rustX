//! Real process death while an independently owned Summary is generating or Ready.
//!
//! The parent owns the provider and temporary workspace. A re-executed
//! conformance binary owns the real `SessionRuntimeManager` and App Server;
//! the provider's gates and native Ready notification are the only barriers.

use super::*;
use crate::common::provider_emulator::ProviderEmulator;
use rustx::context::operation::{CompactionOperationPhase, CompactionOperationStatus};
use rustx::durable::AuxiliaryStore;
use rustx::model::auxiliary::{
    AuxiliaryBusinessSource, AuxiliaryCallAudit, AuxiliaryCallOutcome, AuxiliaryRequestAudit,
};
use rustx::runtime::identity::{CompactionOperationId, ConversationId};
use serde::Deserialize;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Write as _;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

const SCENARIO: &str = "asynchronous_compaction_crash";
const CHILD_TEST: &str = "agent_loop::asynchronous_process_death::crash_child";
const CASE_ENV: &str = "RUSTX_529_CRASH_CASE";
const ROOT_ENV: &str = "RUSTX_529_CRASH_ROOT";
const PORT_ENV: &str = "RUSTX_529_CRASH_PROVIDER_PORT";
const INFO_MARKER: &str = "RUSTX_529_CHILD_INFO ";
const CAPTURE_MARKER: &str = "RUSTX_529_CHILD_CAPTURE ";
const READY_MARKER: &str = "RUSTX_529_CHILD_READY ";
const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Deserialize)]
struct ChildInfo {
    root: PathBuf,
    conversation_id: ConversationId,
    database: PathBuf,
}

/// Only the parent launches this ignored entry with an externally owned provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "child entry for the process-death conformance test"]
async fn crash_child() {
    let case = std::env::var(CASE_ENV).expect("parent-selected crash case");
    assert!(matches!(case.as_str(), "generating" | "ready"));
    let root = tempfile::Builder::new()
        .prefix("rustx-529-child-")
        .tempdir_in(PathBuf::from(
            std::env::var_os(ROOT_ENV).expect("parent root"),
        ))
        .expect("child workspace under parent-owned root");
    let port: u16 = std::env::var(PORT_ENV)
        .expect("provider port")
        .parse()
        .unwrap();
    let emulator = ProviderEmulator::externally_owned(
        &serde_json::json!({"ready": true, "scenario": SCENARIO, "host": "127.0.0.1", "port": port}),
        SCENARIO,
    );
    let setup = Setup {
        context_window: 256_000,
        auxiliary: Some(format!("emulator/{SUMMARY_MODEL}")),
        summary_scenario: SummaryScenario::Chunked,
        summary_window: Some(256_000),
        ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
    };
    let driver = Driver::start_in(root, &emulator, &setup).await;
    println!(
        "{INFO_MARKER}{}",
        serde_json::json!({
            "root": driver.root.path(),
            "session_id": &driver.target.session_id,
            "conversation_id": &driver.target.conversation_id,
            "database": &driver.access.database_path,
        })
    );
    std::io::stdout().flush().unwrap();
    driver.submit(CHUNKED_PROMPT);
    let mut commands = BufReader::new(tokio::io::stdin());
    let mut command = String::new();
    commands.read_line(&mut command).await.unwrap();
    assert_eq!(command, "capture\n");
    let cut = driver.store.compaction_resource_cut().unwrap();
    let statuses = driver
        .store
        .latest_compaction_operations(cut.journal.0)
        .unwrap();
    let matching: Vec<_> = statuses
        .iter()
        .filter(|status| {
            let calls = auxiliary_state(&driver.store, &status.operation_id);
            calls.len() == 1
                && calls[0].0.outcome.is_none()
                && calls[0].1.len() == 1
                && calls[0].1[0].terminal_sequence.is_none()
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "first gated Summary must identify one live native operation: {statuses:?}"
    );
    let captured = (*matching[0]).clone();
    assert_eq!(captured.phase, CompactionOperationPhase::Generating);
    println!(
        "{CAPTURE_MARKER}{}",
        serde_json::to_string(&captured).unwrap()
    );
    std::io::stdout().flush().unwrap();
    if case == "ready" {
        // Begin the readiness wait only after the parent has observed both
        // real provider gates and released the Summary response.
        command.clear();
        commands.read_line(&mut command).await.unwrap();
        assert_eq!(command, "await-ready\n");
        let ready = super::asynchronous_compaction::next_status(
            &driver,
            CompactionOperationPhase::Ready,
            Some(&captured.operation_id),
        )
        .await;
        let snapshot = driver.snapshot().expect("live Ready snapshot").0;
        assert!(
            snapshot
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.phase == RuntimeClientAttemptPhase::Running),
            "the primary Attempt must still be running at the Ready barrier"
        );
        println!("{READY_MARKER}{}", serde_json::to_string(&ready).unwrap());
        std::io::stdout().flush().unwrap();
    }
    // Parent SIGKILL, never a graceful runtime drop, is the tested transition.
    std::future::pending::<()>().await;
}

fn crash_timeline(info: &ChildInfo) -> Vec<String> {
    use rustx::events::types::RuntimeEvent as E;
    let Ok(store) =
        SqliteConversationStore::open_existing(info.conversation_id.clone(), &info.database)
    else {
        return vec!["native journal unavailable".into()];
    };
    let mut cursor = None;
    let mut selected = VecDeque::new();
    for _ in 0..128 {
        let Ok(page) = store.read_events(cursor, 128) else {
            break;
        };
        if page.events.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            let detail = match envelope.event {
                E::ModelRequestStarted {
                    owner: rustx::model::ModelRequestOwner::Agent { .. },
                    request_id,
                    ..
                } => Some(format!("primary started {request_id}")),
                E::ModelRequestFailed {
                    owner: rustx::model::ModelRequestOwner::Agent { .. },
                    request_id,
                    error,
                    ..
                } => Some(format!("primary failed {request_id}: {error:?}")),
                E::CompactionOperationUpdated { status } => Some(format!(
                    "compaction {:?}: {:?}",
                    status.operation_id, status.phase
                )),
                E::AttemptFailed { attempt_id, error } => {
                    Some(format!("attempt failed {attempt_id}: {error:?}"))
                }
                _ => None,
            };
            if let Some(detail) = detail {
                if selected.len() == 24 {
                    selected.pop_front();
                }
                selected.push_back(format!(
                    "{} J={} {detail}",
                    envelope.timestamp, envelope.sequence
                ));
            }
        }
    }
    selected.into_iter().collect()
}

async fn while_primary_connected<T>(
    emulator: &ProviderEmulator,
    primary_index: u64,
    info: &ChildInfo,
    barrier: &str,
    future: impl Future<Output = T>,
) -> T {
    let disconnected = async {
        let mut diagnosed = false;
        loop {
            let url = format!(
                "{}/__control/observations/await?kind=client_disconnected&requestIndex={primary_index}&timeoutMs=120000",
                emulator.base_url()
            );
            let response = reqwest::get(url).await.expect("primary disconnect watch");
            if response.status() == reqwest::StatusCode::GATEWAY_TIMEOUT {
                // No disconnect is the expected state. The enclosing semantic
                // barrier, rather than one bounded observation read, owns expiry.
                if !diagnosed {
                    eprintln!(
                        "primary remains connected while awaiting {barrier}; native timeline: {:?}",
                        crash_timeline(info)
                    );
                    diagnosed = true;
                }
                continue;
            }
            assert!(
                response.status().is_success(),
                "primary disconnect watch failed"
            );
            break response
                .json::<serde_json::Value>()
                .await
                .expect("disconnect JSON");
        }
    };
    tokio::select! {
        biased;
        disconnected = disconnected => {
            panic!("primary disconnected before {barrier}: {disconnected}; native timeline: {:?}", crash_timeline(info));
        }
        value = future => value,
    }
}

struct ProcessOwner {
    child: tokio::process::Child,
    group: nix::unistd::Pid,
    alive: bool,
}

impl ProcessOwner {
    async fn kill(&mut self) {
        nix::sys::signal::killpg(self.group, nix::sys::signal::Signal::SIGKILL)
            .expect("kill the isolated conformance process group");
        let status = tokio::time::timeout(TIMEOUT, self.child.wait())
            .await
            .expect("child exits after SIGKILL")
            .expect("wait for child");
        self.alive = false;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}

impl Drop for ProcessOwner {
    fn drop(&mut self) {
        if self.alive {
            let _ = nix::sys::signal::killpg(self.group, nix::sys::signal::Signal::SIGKILL);
            let _ = self.child.start_kill();
        }
    }
}

async fn marker(
    lines: &mut Lines<BufReader<tokio::process::ChildStdout>>,
    prefix: &str,
    emulator: &ProviderEmulator,
) -> serde_json::Value {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let Some(line) = lines.next_line().await.expect("read child stdout") else {
                let state = match reqwest::get(format!("{}/__control/state", emulator.base_url())).await {
                    Ok(response) => response.json::<serde_json::Value>().await.ok(),
                    Err(_) => None,
                };
                panic!("child exited before {prefix}; provider state={state:?}; provider diagnostics={}", emulator.diagnostics());
            };
            if let Some((_, value)) = line.split_once(prefix) {
                return serde_json::from_str(value).expect("child barrier JSON");
            }
        }
    })
    .await
    .expect("child reached its semantic barrier")
}

async fn observation(emulator: &ProviderEmulator, kind: &str, index: u64) -> serde_json::Value {
    let url = format!(
        "{}/__control/observations/await?kind={kind}&requestIndex={index}&timeoutMs=120000",
        emulator.base_url()
    );
    let response = tokio::time::timeout(TIMEOUT, reqwest::get(url))
        .await
        .expect("provider observation timeout")
        .expect("provider control request");
    assert!(
        response.status().is_success(),
        "provider did not observe {kind} for request {index}"
    );
    response.json().await.expect("provider observation JSON")
}

fn auxiliary_state(
    store: &SqliteConversationStore,
    operation: &CompactionOperationId,
) -> Vec<(AuxiliaryCallAudit, Vec<AuxiliaryRequestAudit>)> {
    let mut cursor = None;
    let mut calls = Vec::new();
    for _ in 0..128 {
        let page = store.read_events(cursor, 128).unwrap();
        if page.events.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            if let rustx::events::types::RuntimeEvent::AuxiliaryCallStarted {
                call_id,
                business_source: AuxiliaryBusinessSource::Compaction { operation_id, .. },
                ..
            } = envelope.event
                && &operation_id == operation
            {
                calls.push(call_id);
            }
        }
    }
    let cut = store.auxiliary_audit_cut().unwrap();
    calls
        .into_iter()
        .map(|id| {
            let call = store
                .load_auxiliary_call(&id, cut)
                .unwrap()
                .expect("durable Summary Call");
            let requests = store
                .read_auxiliary_call_requests(&id, cut, None, 32)
                .unwrap();
            (call, requests)
        })
        .collect()
}

/// Reopen the same on-disk Session through the real Manager and App Server.
async fn reopen(
    info: &ChildInfo,
) -> (
    SessionRuntimeManager,
    Arc<AppServerConnection>,
    AttachmentTarget,
) {
    let paths = LaunchFixture {
        config: info.root.join("rustx.toml"),
        startup_session: rustx::local_runtime::StartupSession::Empty,
        workspace: info.root.join("workspace"),
        runtime_root: info.root.join("private"),
    };
    let environment =
        HostEnvironment::from_paths(paths.workspace.clone(), info.root.join(".test-host")).unwrap();
    let (configuration, _) = paths.request().session_input(&environment).unwrap();
    let sessions = SessionController::open(&paths.runtime_root).unwrap();
    let manager = SessionRuntimeManager::new(
        sessions,
        configuration,
        CredentialSnapshot::new([(CREDENTIAL_VARIABLE.to_owned(), CREDENTIAL_VALUE.to_owned())]),
        LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                CREDENTIAL_VARIABLE.to_owned(),
                CREDENTIAL_VALUE.to_owned(),
            )]))),
            child_program: Some(PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        },
        RuntimeResidencyPolicy {
            max_resident_runtimes: 8,
            idle_grace_ms: 300_000,
        },
    )
    .unwrap();
    let connection = Arc::new(AppServerConnection::new(AppServerHost::new(
        manager.clone(),
        AppServerPolicy::default(),
    )));
    rpc(
        &connection,
        Method::Initialize(InitializeParams {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "process-death".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    )
    .await
    .unwrap();
    let attached = rpc(
        &connection,
        Method::SessionAttach {
            thread_id: info.conversation_id.clone(),
        },
    )
    .await
    .unwrap();
    let MethodResult::Attached { target, .. } = attached else {
        panic!("reopened Session attachment")
    };
    assert_eq!(target.conversation_id, info.conversation_id);
    (manager, connection, target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Two controlled SIGKILL cuts share the same end-to-end proof.
async fn proactive_generation_and_ready_reconcile_after_real_process_death() {
    for case in ["generating", "ready"] {
        let Some(emulator) = ProviderEmulator::start(SCENARIO).await else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let port = emulator
            .base_url()
            .rsplit(':')
            .next()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                CHILD_TEST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CASE_ENV, case)
            .env(ROOT_ENV, root.path())
            .env(PORT_ENV, port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        let mut child = command
            .spawn()
            .expect("spawn the isolated conformance child");
        let group = nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap());
        let stdout = child.stdout.take().expect("child barrier stream");
        let mut stdin = child.stdin.take().expect("child barrier input");
        let mut owner = ProcessOwner {
            child,
            group,
            alive: true,
        };
        let mut lines = BufReader::new(stdout).lines();
        let info: ChildInfo =
            serde_json::from_value(marker(&mut lines, INFO_MARKER, &emulator).await).unwrap();
        assert!(info.root.starts_with(root.path()));
        let (summary_gate, primary_gate) = tokio::join!(
            emulator.await_gate_with_timeout("crash-summary", std::time::Duration::from_secs(300)),
            emulator.await_gate_with_timeout("crash-primary", std::time::Duration::from_secs(300))
        );
        let summary_index = summary_gate["requestIndex"].as_u64().unwrap();
        let primary_index = primary_gate["requestIndex"].as_u64().unwrap();
        stdin
            .write_all(b"capture\n")
            .await
            .expect("request native operation identity");
        let captured: CompactionOperationStatus = serde_json::from_value(
            while_primary_connected(
                &emulator,
                primary_index,
                &info,
                "native capture",
                marker(&mut lines, CAPTURE_MARKER, &emulator),
            )
            .await,
        )
        .unwrap();
        assert_eq!(captured.phase, CompactionOperationPhase::Generating);
        if case == "ready" {
            emulator.release_gate("crash-summary").await;
            stdin
                .write_all(b"await-ready\n")
                .await
                .expect("release child Ready wait");
            let ready: CompactionOperationStatus = serde_json::from_value(
                while_primary_connected(
                    &emulator,
                    primary_index,
                    &info,
                    "Ready",
                    marker(&mut lines, READY_MARKER, &emulator),
                )
                .await,
            )
            .unwrap();
            assert_eq!(ready.phase, CompactionOperationPhase::Ready);
            assert_eq!(ready.operation_id, captured.operation_id);
            assert_eq!(ready.admission_generation, captured.admission_generation);
        }
        let provider_before = while_primary_connected(
            &emulator,
            primary_index,
            &info,
            "SIGKILL",
            emulator.requests(),
        )
        .await;
        owner.kill().await;
        let primary_disconnect = observation(&emulator, "client_disconnected", primary_index).await;
        assert_eq!(primary_disconnect["requestIndex"], primary_index);
        if case == "generating" {
            let disconnected = observation(&emulator, "client_disconnected", summary_index).await;
            assert_eq!(disconnected["requestIndex"], summary_index);
            emulator.release_gate("crash-summary").await;
        }
        // A gate remains parked after its client disconnects; release it only
        // after observing EOF so the provider settles the expected disconnect.
        emulator.release_gate("crash-primary").await;
        let primary_done = observation(&emulator, "response_completed", primary_index).await;
        assert_eq!(primary_done["detail"]["terminal"], "client_disconnect");
        for request in &provider_before {
            observation(
                &emulator,
                "response_completed",
                request["index"].as_u64().unwrap(),
            )
            .await;
        }

        let store =
            SqliteConversationStore::open_existing(info.conversation_id.clone(), &info.database)
                .unwrap();
        let cut = store.compaction_resource_cut().unwrap();
        let before = store
            .latest_compaction_operations(cut.journal.0)
            .unwrap()
            .pop()
            .expect("persisted background status");
        assert_eq!(before.operation_id, captured.operation_id);
        assert_eq!(before.admission_generation, captured.admission_generation);
        assert_eq!(
            before.phase,
            if case == "ready" {
                CompactionOperationPhase::Ready
            } else {
                CompactionOperationPhase::Generating
            }
        );
        assert!(store.projection_head().unwrap().is_none());
        assert!(
            !store
                .compaction_restart_evidence(&before.operation_id)
                .unwrap()
                .committed
        );
        let actual = auxiliary_state(&store, &before.operation_id);
        assert!(!actual.is_empty());
        if case == "ready" {
            assert!(actual.iter().all(|(call, requests)| call.outcome
                == Some(AuxiliaryCallOutcome::Completed)
                && !requests.is_empty()));
            assert!(
                actual
                    .iter()
                    .flat_map(|(_, requests)| requests)
                    .any(|request| request
                        .usage
                        .as_ref()
                        .is_some_and(|usage| usage.input_tokens == Some(123)
                            && usage.output_tokens == Some(45)))
            );
        } else {
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].0.outcome, None);
            assert_eq!(actual[0].1.len(), 1);
            assert!(actual[0].1[0].terminal_sequence.is_none());
            assert!(actual[0].1[0].request_evidence.is_none());
        }
        drop(store);

        let (manager, connection, target) = reopen(&info).await;
        let result = rpc(
            &connection,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: Vec::new(),
            },
        )
        .await
        .unwrap();
        let MethodResult::Checkpoint { checkpoint } = result else {
            panic!("recovered Session checkpoint")
        };
        let operation = checkpoint
            .snapshot
            .context
            .compaction_operations
            .iter()
            .find(|value| value.status.operation_id == before.operation_id)
            .unwrap_or_else(|| {
                panic!(
                    "{case}: reconciled operation {:?} absent at checkpoint J={}; observed {:?}",
                    before.operation_id,
                    checkpoint.read_cut.through,
                    checkpoint.snapshot.context.compaction_operations
                )
            });
        assert_eq!(operation.status.phase, CompactionOperationPhase::NoEffect);
        assert!(!operation.live);
        assert_eq!(
            operation.status.admission_generation,
            before.admission_generation
        );
        assert_eq!(
            operation.status.accepted_fragments,
            before.accepted_fragments
        );
        assert!(operation.status.waiting_attempt_id.is_none());
        assert!(
            operation
                .status
                .diagnostic
                .as_deref()
                .is_some_and(|message| message.contains("not resumed"))
        );
        let reopened =
            SqliteConversationStore::open_existing(info.conversation_id.clone(), &info.database)
                .unwrap();
        assert!(reopened.projection_head().unwrap().is_none());
        assert!(
            !reopened
                .compaction_restart_evidence(&before.operation_id)
                .unwrap()
                .committed
        );
        assert_eq!(
            auxiliary_state(&reopened, &before.operation_id),
            actual,
            "recovery preserves exact prior Call, request and usage facts"
        );
        drop(reopened);
        connection.close();
        manager.unload(&target.conversation_id).await.unwrap();
        assert_eq!(
            emulator.requests().await,
            provider_before,
            "cold recovery dispatched no provider request"
        );
        emulator.finish_expected_partial().await;
    }
}
