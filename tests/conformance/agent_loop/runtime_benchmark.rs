//! Opt-in offline measurement through the production composition and provider boundary.
//! Run in a fresh process with RUSTX_BENCH_CASE and RUSTX_BENCH_OUTPUT.
use super::*;
use serde_json::json;
use std::time::Instant;

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
    let root = tempfile::tempdir_in(output.parent().unwrap()).unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join(NOTE_PATH), NOTE_MARKER).unwrap();
    let scenario = format!("runtime_benchmark_{case}");
    let emulator = ProviderEmulator::start_with_workspace(&scenario, Some(&workspace))
        .await
        .expect("offline emulator required");
    let began = Instant::now();
    let mut driver = Driver::start_in(root, &emulator, &Setup::new("emulator/chat-model")).await;
    let open_us = began.elapsed().as_micros();
    let initial = process_sample();
    let mut samples = Vec::new();
    for turn in 0..history + measured {
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
    let mut cursor = None;
    let mut snapshot_count = 0;
    loop {
        let page = driver.request_history().page(cursor, 32).unwrap();
        if page.snapshots.is_empty() {
            break;
        }
        assert_ne!(page.next_sequence, cursor);
        cursor = page.next_sequence;
        snapshot_count += page.snapshots.len();
        for snapshot in page.snapshots {
            driver
                .reconstruct_request(&snapshot.identity)
                .expect("frozen exact request remains reconstructable");
        }
    }
    assert_eq!(snapshot_count, expected_requests);
    let final_sample = process_sample();
    let database = driver.access.database_path.clone();
    let cid = driver.access.node.conversation_id.clone();
    let began = Instant::now();
    driver.shutdown().await.unwrap();
    let shutdown_us = began.elapsed().as_micros();
    let root_path = driver.root.path().to_owned();
    driver.root.disable_cleanup(true);
    let before_close = footprint(&root_path);
    let began = Instant::now();
    drop(driver);
    let close_us = began.elapsed().as_micros();
    let closed_footprint = footprint(&root_path);
    let began = Instant::now();
    let reopened = SqliteConversationStore::open_existing(cid, &database).unwrap();
    assert_eq!(reopened.load_canonical().unwrap(), history_messages);
    let reopen_us = began.elapsed().as_micros();
    drop(reopened);
    let began = Instant::now();
    std::fs::remove_dir_all(&root_path).unwrap();
    let cleanup_us = began.elapsed().as_micros();
    emulator.finish().await;
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "case":case,"schema":rustx::durable::sqlite::SQLITE_SCHEMA_VERSION,
            "protocol":APP_SERVER_PROTOCOL_VERSION,"history":history,"measured":measured,
            "requests":expected_requests,"open_us":open_us,"samples":samples,
            "initial_process":initial,"final_process":final_sample,
            "shutdown_us":shutdown_us,"reopen_us":reopen_us,
            "before_close":before_close,"closed_footprint":closed_footprint,"close_us":close_us,"cleanup_us":cleanup_us,
            "oracle":"passed","sql_statements":null,"allocations":null
        }))
        .unwrap(),
    )
    .unwrap();
}
