//! Opt-in offline measurement through the production composition and provider boundary.
//! Run in a fresh process with `RUSTX_BENCH_CASE` and `RUSTX_BENCH_OUTPUT`.
use super::*;
use rustx::durable::{CanonicalStore, JournalStore};
use serde_json::json;
use std::time::Instant;

mod profiling;
mod request_oracle;

fn footprint(root: &std::path::Path) -> serde_json::Value {
    fn visit(root: &std::path::Path, files: &mut Vec<serde_json::Value>) {
        use std::os::unix::fs::MetadataExt;
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    files.push(json!({"path":entry.path(),"vanished_during_scan":true,"logical":null,"allocated":null}));
                    continue;
                }
                Err(error) => panic!("footprint metadata: {error}"),
            };
            if metadata.is_dir() {
                visit(&entry.path(), files);
            } else {
                files.push(json!({"path":entry.path(), "logical":metadata.len(),
                    "allocated":metadata.blocks()*512}));
            }
        }
    }
    let mut files = Vec::new();
    visit(root, &mut files);
    json!(files)
}

fn process_sample() -> serde_json::Value {
    json!({
        "stat": std::fs::read_to_string("/proc/self/stat").ok(),
        "io": std::fs::read_to_string("/proc/self/io").ok(),
        "smaps_rollup": std::fs::read_to_string("/proc/self/smaps_rollup").ok(),
    })
}

fn lifecycle_sample(samples: &mut Vec<serde_json::Value>, phase: &str, began: Instant) {
    let process = process_sample();
    samples.push(json!({"phase":phase,"elapsed_us":began.elapsed().as_micros(),"process":process}));
    profiling::boundary(phase);
}

fn verify_requests(
    store: Arc<SqliteConversationStore>,
    case: &str,
    workspace: &std::path::Path,
    requests: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use rustx::durable::presentation::{FactQuery, FactScope};
    let starts = store
        .read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec!["model_request_started"],
            before: None,
            after: 0,
            ascending: true,
            through: store.presentation_frontier().unwrap(),
            limit: 128,
        })
        .unwrap();
    let conversation = store.conversation_id().clone();
    let history = rustx::runtime::RequestHistory::new(store);
    let snapshots = crate::common::request_snapshots(&history);
    assert_eq!(snapshots.len(), requests.len());
    assert_eq!(starts.len(), requests.len());
    let mut identities = std::collections::BTreeSet::new();
    snapshots
        .into_iter()
        .zip(&starts)
        .zip(requests)
        .enumerate()
        .map(|(index, ((snapshot, start), wire))| {
            assert!(identities.insert(snapshot.request_id.clone()));
            assert_eq!(snapshot.request_id, snapshot.identity.request_id());
            assert_eq!(
                snapshot.provisional_message_id,
                snapshot.identity.provisional_message_id()
            );
            assert_eq!(
                history.get(&snapshot.identity).unwrap().as_ref(),
                Some(&snapshot)
            );
            assert_eq!(wire["index"], index);
            assert_eq!(wire["method"], "POST");
            assert_eq!(wire["path"], "/v1/chat/completions");
            assert_eq!(wire["protocol"], "openai_chat_completions");
            assert_eq!(wire["model"], "chat-model");
            let reconstructed = history.reconstruct(&snapshot.identity).unwrap();
            assert_eq!(
                reconstructed.semantic_input_fingerprint(),
                snapshot.semantic_input_fingerprint
            );
            let request = serde_json::to_value(reconstructed).unwrap();
            let snapshot = serde_json::to_value(snapshot).unwrap();
            request_oracle::assert_identity(
                &snapshot,
                &request,
                &serde_json::to_value(start).unwrap(),
                conversation.as_str(),
            );
            request_oracle::assert_bodies(
                case,
                index,
                workspace.to_str().unwrap(),
                &wire["body"],
                &request_oracle::reconstructed_body(&request),
            );
            // Retain plain values, never a Source/store owner across final close.
            json!({"snapshot":snapshot,"request":request})
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "offline performance measurement; no timing gate"]
async fn measure_runtime() {
    let case = std::env::var("RUSTX_BENCH_CASE").expect("case");
    let output = PathBuf::from(std::env::var_os("RUSTX_BENCH_OUTPUT").expect("output"));
    let (history, measured) = match case.as_str() {
        "short-small" => (8, 32),
        "short-large" => (64, 32),
        "stream" | "large" | "tool" | "cancel" => (0, 1),
        _ => panic!("unknown benchmark case"),
    };
    let lifecycle_began = Instant::now();
    let mut lifecycle = Vec::new();
    lifecycle_sample(&mut lifecycle, "before_initialization", lifecycle_began);
    let root = tempfile::tempdir_in(output.parent().unwrap()).unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join(NOTE_PATH), NOTE_MARKER).unwrap();
    let scenario = format!("runtime_benchmark_{case}");
    // The external controller owns both siblings. Provider reaping can never
    // enter the product's /proc/self/io counters, even on failure paths.
    println!(
        "RUSTX_BENCH_READY {}",
        json!({"workspace":workspace,"scenario":scenario})
    );
    let mut ready = String::new();
    std::io::stdin().read_line(&mut ready).unwrap();
    let emulator = ProviderEmulator::externally_owned(
        &serde_json::from_str(&ready).expect("controller provider readiness"),
        &scenario,
    );
    lifecycle_sample(&mut lifecycle, "after_provider_start", lifecycle_began);
    let began = Instant::now();
    let mut driver = Driver::start_in(root, &emulator, &Setup::new("emulator/chat-model")).await;
    let open_us = began.elapsed().as_micros();
    let initial = process_sample();
    lifecycle_sample(&mut lifecycle, "after_initialization", lifecycle_began);
    let mut samples = Vec::new();
    for turn in 0..history + measured {
        if turn == history {
            profiling::boundary("before_measured_turns");
        }
        let began = Instant::now();
        driver.submit(&format!("benchmark turn {turn:04}"));
        let accepted_us = began.elapsed().as_micros();
        if case == "cancel" {
            emulator.await_gate("cancel").await;
            let snapshots = driver.request_history().page(None, 32).unwrap().snapshots;
            assert_eq!(snapshots.len(), 1);
            driver.cancel_current_attempt().await.unwrap();
            emulator.await_client_disconnect().await;
            emulator.release_gate("cancel").await;
        } else if case == "stream" {
            // Deliberate workload pacing, never synchronization for a race assertion.
            for delta in 0..128 {
                let gate = format!("delta-{delta}");
                emulator.await_gate(&gate).await;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                emulator.release_gate(&gate).await;
            }
        }
        let mut first_publication_us = None;
        let (events, outcome) = driver
            .settle_observing(|event| {
                if matches!(event, RuntimeClientEvent::BlockContentAdvanced { .. })
                    && first_publication_us.is_none()
                {
                    first_publication_us = Some(began.elapsed().as_micros());
                }
            })
            .await;
        let settled_us = began.elapsed().as_micros();
        if case == "cancel" {
            assert!(matches!(outcome, RuntimeClientOutcome::Cancelled { .. }));
        } else {
            assert!(
                matches!(outcome, RuntimeClientOutcome::Completed { .. }),
                "{outcome:?}"
            );
        }
        if case == "tool" {
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. }))
            );
        }
        samples.push(json!({"turn":turn,"measured":turn>=history,
            "accepted_us":accepted_us,"settled_us":settled_us,
            "first_consumed_publication_us":first_publication_us,
            "notification_records":events.records.len(),"process":process_sample(),
            "footprint":footprint(driver.root.path())}));
    }
    lifecycle_sample(&mut lifecycle, "after_turns", lifecycle_began);
    let requests = emulator.requests().await;
    let expected_requests = history + measured + usize::from(case == "tool");
    assert_eq!(requests.len(), expected_requests);
    let history_messages = driver.canonical_history();
    let text: String = history_messages
        .iter()
        .filter_map(|m| match m {
            MessageBlock::Assistant(m) => Some(m),
            _ => None,
        })
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            rustx::message::types::AssistantContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    // Keep the frozen BEFORE oracle identical; this allocation is outside turn timing.
    #[allow(clippy::format_collect)]
    let expected = match case.as_str() {
        "large" => "L".repeat(32768),
        "stream" => (0..128).map(|i| format!("stream-delta-{i:04} ")).collect(),
        "tool" => NOTE_MARKER.to_owned(),
        "cancel" => String::new(),
        _ => (0..history + measured)
            .map(|i| format!("reply {i:04}"))
            .collect(),
    };
    assert_eq!(text, expected);
    let verified_requests = verify_requests(driver.store.clone(), &case, &workspace, &requests);
    lifecycle_sample(&mut lifecycle, "after_verification", lifecycle_began);
    let database = driver.access.database_path.clone();
    let cid = driver.access.node.conversation_id.clone();
    let began = Instant::now();
    driver.shutdown().await.unwrap();
    if let Err(error) = (&mut driver.relay).await {
        assert!(error.is_cancelled(), "event relay failed: {error}");
    }
    let shutdown_us = began.elapsed().as_micros();
    lifecycle_sample(&mut lifecycle, "after_shutdown", lifecycle_began);
    let root_path = driver.root.path().to_owned();
    driver.root.disable_cleanup(true);
    let before_close = footprint(&root_path);
    let began = Instant::now();
    drop(driver);
    let close_us = began.elapsed().as_micros();
    let closed_footprint = footprint(&root_path);
    lifecycle_sample(&mut lifecycle, "after_driver_drop", lifecycle_began);
    let began = Instant::now();
    // A final read-only owner can leave committed WAL frames after shutdown.
    // Include a normal writable open/close before deleting the allocation.
    drop(SqliteConversationStore::open(cid.clone(), &database).unwrap());
    let final_writer_close_us = began.elapsed().as_micros();
    let checkpoint_footprint = footprint(&root_path);
    let wal = std::path::PathBuf::from(format!("{}-wal", database.display()));
    match std::fs::metadata(wal) {
        Ok(info) => assert_eq!(info.len(), 0, "checkpoint left WAL frames"),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
    lifecycle_sample(&mut lifecycle, "after_final_writer_close", lifecycle_began);
    let began = Instant::now();
    let reopened = Arc::new(SqliteConversationStore::open_existing(cid, &database).unwrap());
    assert_eq!(reopened.load_canonical().unwrap(), history_messages);
    let reopened_requests = verify_requests(reopened.clone(), &case, &workspace, &requests);
    assert_eq!(
        reopened_requests, verified_requests,
        "exact frozen identities and input survive reopen"
    );
    let reopen_us = began.elapsed().as_micros();
    let verified_live_requests = verified_requests.len();
    let verified_reopened_requests = reopened_requests.len();
    drop(reopened);
    lifecycle_sample(&mut lifecycle, "after_reopen_verification", lifecycle_began);
    let began = Instant::now();
    std::fs::remove_dir_all(&root_path).unwrap();
    let cleanup_us = began.elapsed().as_micros();
    lifecycle_sample(&mut lifecycle, "after_cleanup", lifecycle_began);
    drop(emulator);
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "case":case,"schema":rustx::durable::sqlite::SQLITE_SCHEMA_VERSION,
            "protocol":APP_SERVER_PROTOCOL_VERSION,"history":history,"measured":measured,
            "requests":expected_requests,"open_us":open_us,"samples":samples,
            "initial_process":initial,"lifecycle":lifecycle,"measurement_version":3,
            "wire_requests":requests,"live_requests":verified_requests,
            "reopened_requests":reopened_requests,"canonical":history_messages,
            "oracle_version":2,"verified_live_requests":verified_live_requests,
            "verified_reopened_requests":verified_reopened_requests,
            "shutdown_us":shutdown_us,"reopen_us":reopen_us,
            "final_writer_close_us":final_writer_close_us,"checkpoint_footprint":checkpoint_footprint,
            "before_close":before_close,"closed_footprint":closed_footprint,"close_us":close_us,"cleanup_us":cleanup_us,
            "oracle":"passed","sql_statements":null,"allocations":null
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Marginal native canonical acceptance, without provider/context construction.
/// A fresh process/store is required for each history size and repetition.
#[test]
#[ignore = "offline canonical append measurement; no timing gate"]
fn measure_canonical_append() {
    canonical_append_measurement(false);
}

/// Reopen before each measured append; reopen cost stays in lifecycle counters,
/// while operation counters and elapsed time surround only append itself.
#[test]
#[ignore = "offline cold canonical append measurement; no timing gate"]
fn measure_cold_canonical_append() {
    canonical_append_measurement(true);
}

fn canonical_append_measurement(cold: bool) {
    use rustx::message::types::{InboundKind, UserMessageBlock, UserSource};
    use rustx::runtime::identity::{ConversationId, MessageId};
    let history: usize = std::env::var("RUSTX_BENCH_HISTORY")
        .expect("history size")
        .parse()
        .unwrap();
    assert!([0, 64, 512, 2048].contains(&history));
    let output = PathBuf::from(std::env::var_os("RUSTX_BENCH_OUTPUT").expect("output"));
    let root = tempfile::tempdir_in(output.parent().unwrap()).unwrap();
    let database = root.path().join("conversation.sqlite");
    let cid = ConversationId::new("conv_018f0000-0000-7000-8000-000000000001");
    let began = Instant::now();
    let mut lifecycle = Vec::new();
    lifecycle_sample(&mut lifecycle, "before_initialization", began);
    let store = SqliteConversationStore::open(cid.clone(), &database).unwrap();
    let message = |index: usize| {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(format!("msg_018f0000-0000-7000-8000-{index:012x}")),
            content: vec![UserContentBlock::Text(TextBlock {
                text: format!("canonical input {index:05}"),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    };
    for index in 0..history {
        store.append_canonical(&message(index)).unwrap();
    }
    lifecycle_sample(&mut lifecycle, "after_seed", began);
    let mut store = Some(store);
    let mut samples = Vec::new();
    let mut operations = Vec::new();
    for index in history..history + 32 {
        let message = message(index);
        if cold {
            drop(store.take());
            store = Some(SqliteConversationStore::open(cid.clone(), &database).unwrap());
        }
        let before = cold.then(process_sample);
        let start = Instant::now();
        store.as_ref().unwrap().append_canonical(&message).unwrap();
        samples.push(start.elapsed().as_nanos());
        if let Some(before) = before {
            operations.push(json!({"before":before,"after":process_sample()}));
        }
    }
    lifecycle_sample(&mut lifecycle, "after_appends", began);
    let before_close = footprint(root.path());
    drop(store);
    lifecycle_sample(&mut lifecycle, "after_close", began);
    let closed_footprint = footprint(root.path());
    let reopened = SqliteConversationStore::open_existing(cid, &database).unwrap();
    assert_eq!(
        reopened.load_canonical().unwrap(),
        (0..history + 32).map(message).collect::<Vec<_>>()
    );
    drop(reopened);
    lifecycle_sample(&mut lifecycle, "after_reopen_verification", began);
    root.close().unwrap();
    lifecycle_sample(&mut lifecycle, "after_cleanup", began);
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({"history":history,"measured":32,
            "cold":cold,"operation_counters":operations,
            "append_ns":samples,"lifecycle":lifecycle,"before_close":before_close,
            "closed_footprint":closed_footprint,"oracle":"passed",
            "schema":rustx::durable::sqlite::SQLITE_SCHEMA_VERSION}))
        .unwrap(),
    )
    .unwrap();
}
