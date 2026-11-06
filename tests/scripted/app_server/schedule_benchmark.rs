//! Explicit resource measurement over native allocation/open/validate/read/close.
#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]
use super::*;
use crate::durable::{CanonicalStore, SqliteConversationStore, conversation_store_open_count};
use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::MessageId;
use crate::scheduler::{ScheduleCommand, ScheduleMutation, ScheduleTiming};
use std::path::Path;
use std::time::{Duration, Instant};

fn number(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |value| {
        value.parse().expect("positive benchmark parameter")
    })
}
fn system_number(name: &str) -> u64 {
    let output = std::process::Command::new("getconf")
        .arg(name)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
fn cpu_seconds(ticks: u64) -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .collect();
    (fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()) as f64 / ticks as f64
}
fn rss_bytes(page: u64) -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * page
}
fn io() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/self/io").unwrap();
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap()
    };
    (value("rchar:"), value("read_bytes:"))
}
fn percentile(values: &[f64], fraction: usize) -> Option<f64> {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values
        .get((values.len() * fraction).div_ceil(100).saturating_sub(1))
        .copied()
}
fn evict_database(path: &Path) {
    let file = std::fs::File::open(path).unwrap();
    file.sync_all().unwrap();
    nix::fcntl::posix_fadvise(
        &file,
        0,
        0,
        nix::fcntl::PosixFadviseAdvice::POSIX_FADV_DONTNEED,
    )
    .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Scheduler resource benchmark: explicit native data setup and optional ten-minute release gate"]
async fn measure_native_scheduler_reconciliation() {
    let size = number("RUSTX_SCHEDULER_BENCH_SIZE", 100);
    let reps = number("RUSTX_SCHEDULER_BENCH_PASSES", 1);
    let seconds = number("RUSTX_SCHEDULER_BENCH_SECONDS", 0);
    let sparse = std::env::var("RUSTX_SCHEDULER_BENCH_MODE").is_ok_and(|mode| mode == "sparse");
    let release_gate = std::env::var_os("RUSTX_SCHEDULER_BENCH_RELEASE_GATE").is_some();
    assert!(size >= 3 && reps > 0);
    #[cfg(debug_assertions)]
    assert!(!release_gate, "release gate requires an optimized build");
    if release_gate {
        assert!(!sparse && size == 10_000 && reps >= 10 && seconds >= 600);
    }
    let f = Fixture::with_session_count(None, 1).await;
    let mut sessions = vec![f.sessions[0].clone()];
    let setup_started = Instant::now();
    let history = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("benchmark-history"),
        content: input("Historical benchmark conversation"),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(chrono::Utc::now()),
    });
    // Seed the already-published fixture Session before the large catalog exists.
    let access = f
        .manager
        .sessions
        .acquire_session(&sessions[0].id, Some(&sessions[0].active_node))
        .await
        .unwrap();
    let store =
        SqliteConversationStore::open(access.node.conversation_id.clone(), &access.database_path)
            .unwrap();
    store.append_canonical(&history).unwrap();
    let mut databases = vec![access.database_path.clone()];
    drop(store);
    drop(access);

    let catalog = f.manager.sessions.catalog.clone();
    let state =
        SessionPersistentState::from_input(&SessionConfigInput::new(f.workspaces[0].clone()));
    let prepared = tokio::task::spawn_blocking(move || {
        catalog
            .blocking_lock()
            .seed_benchmark_sessions(&state, &[history], size - 1)
    })
    .await
    .unwrap()
    .unwrap();
    for (session, database) in prepared {
        sessions.push(session);
        databases.push(database);
    }
    eprintln!("Scheduler historical fixture ready: {size} conversations");
    for (index, session) in sessions.iter().enumerate() {
        if sparse && index % 100 == 0 {
            f.manager
                .schedule_command(
                    &session.id,
                    &session.active_node,
                    ScheduleCommand {
                        command_id: "sparse".into(),
                        mutation: ScheduleMutation::Create {
                            title: "Sparse future schedule".into(),
                            text: "benchmark".into(),
                            timing: ScheduleTiming::After {
                                seconds: 31 * 86400,
                            },
                        },
                    },
                )
                .await
                .unwrap();
        }
    }
    let control = &sessions[0];
    let create = f
        .manager
        .schedule_command(
            &control.id,
            &control.active_node,
            ScheduleCommand {
                command_id: "control".into(),
                mutation: ScheduleMutation::Create {
                    title: "Control".into(),
                    text: "benchmark".into(),
                    timing: ScheduleTiming::After {
                        seconds: 31 * 86400,
                    },
                },
            },
        )
        .await
        .unwrap();
    let crate::scheduler::ScheduleCommandResult::Applied { receipt } = create else {
        panic!("control schedule");
    };
    f.manager
        .schedule_command(
            &control.id,
            &control.active_node,
            ScheduleCommand {
                command_id: "disable".into(),
                mutation: ScheduleMutation::SetEnabled {
                    id: receipt.id.clone(),
                    expected_revision: 1,
                    enabled: false,
                },
            },
        )
        .await
        .unwrap();
    f.manager.schedule_dirty.lock().unwrap().clear();
    let fault = databases[2].with_extension("unavailable");
    if sparse {
        std::fs::rename(&databases[2], &fault).unwrap();
        std::fs::write(&databases[2], b"unavailable").unwrap();
    }
    for database in &databases {
        evict_database(database);
    }
    eprintln!("Scheduler measurement starting: {size} conversations");
    let ticks = system_number("CLK_TCK");
    let page = system_number("PAGE_SIZE");
    let baseline_rss = rss_bytes(page);
    let cpu_before = cpu_seconds(ticks);
    let io_before = io();
    let opens_before = conversation_store_open_count();
    let (waiter, mut waiting) = watch::channel(u64::MAX);
    f.manager.registry.0.lock().unwrap().schedule_waiting = Some(waiter);
    let probe = f.manager.probe(&control.active_conversation_id);
    let registered_probes = f.manager.registry.0.lock().unwrap().probes.len();
    probe.schedule_native_read.arm();
    let manager = f.manager.clone();
    let started = Instant::now();
    let driver = tokio::spawn(async move { manager.run_schedule_driver().await });
    probe.schedule_native_read.entered().await;
    let write_started = Instant::now();
    f.manager
        .schedule_command(
            &control.id,
            &control.active_node,
            ScheduleCommand {
                command_id: "write-during-pass".into(),
                mutation: ScheduleMutation::Update {
                    id: receipt.id,
                    expected_revision: 2,
                    title: "Updated during traversal".into(),
                    text: "benchmark".into(),
                    timing: None,
                },
            },
        )
        .await
        .unwrap();
    let write_ms = write_started.elapsed().as_secs_f64() * 1000.0;
    probe.schedule_native_read.release();
    let mut passes = 0;
    let mut durations = Vec::new();
    let mut controls = Vec::new();
    let mut peak_rss = baseline_rss;
    let mut repaired = false;
    let mut startup_resources = None;
    loop {
        tokio::select! {
            changed=waiting.changed()=>{
                changed.unwrap();let diagnostics=f.manager.schedule_driver_diagnostics();
                if diagnostics.passes_completed>passes{
                    passes=diagnostics.passes_completed;durations.push(diagnostics.last_pass_ms as f64);
                    if startup_resources.is_none() {startup_resources=Some((cpu_seconds(ticks), io(), started.elapsed().as_secs_f64()));}
                    if sparse && !repaired{assert!(!diagnostics.sources[&sessions[2].active_conversation_id].available);std::fs::rename(&fault,&databases[2]).unwrap();repaired=true;}
                    eprintln!("Scheduler pass {passes}, ms={}, sources={}",diagnostics.last_pass_ms,diagnostics.sources.len());
                }
            },
            ()=tokio::time::sleep(Duration::from_millis(250))=>{
                let before=Instant::now();f.manager.schedule_list(&control.id,&control.active_node,0,100).await.unwrap();controls.push(before.elapsed().as_secs_f64()*1000.0);
            },
        }
        peak_rss = peak_rss.max(rss_bytes(page));
        if passes >= u64::try_from(reps).unwrap()
            && started.elapsed().as_secs() >= u64::try_from(seconds).unwrap()
        {
            break;
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let cpu_after = cpu_seconds(ticks);
    let cpu = cpu_after - cpu_before;
    let io_after = io();
    let opens = conversation_store_open_count() - opens_before;
    let diagnostics = f.manager.schedule_driver_diagnostics();
    f.host.begin_drain();
    driver.await.unwrap().unwrap();
    assert!(f.host.drain().await.is_empty());
    assert!(f.manager.is_empty());
    let retained_probes = f.manager.registry.0.lock().unwrap().probes.len();
    let warm = &durations[1..];
    let cpu_cores = cpu / elapsed;
    let logical_rate = (io_after.0 - io_before.0) as f64 / elapsed;
    let physical_rate = (io_after.1 - io_before.1) as f64 / elapsed;
    let (startup_cpu, startup_io, startup_elapsed) = startup_resources.unwrap();
    let warm_elapsed = elapsed - startup_elapsed;
    let steady_cpu_cores = (cpu_after - startup_cpu) / warm_elapsed;
    let steady_logical_rate = (io_after.0 - startup_io.0) as f64 / warm_elapsed;
    let steady_physical_rate = (io_after.1 - startup_io.1) as f64 / warm_elapsed;
    let eligible =
        !cfg!(debug_assertions) && !sparse && size == 10_000 && passes >= 10 && elapsed >= 600.0;
    let probe_integrity = retained_probes == registered_probes;
    let within_budget = eligible
        && probe_integrity
        && steady_cpu_cores <= 0.5
        && steady_logical_rate <= 64.0 * 1024.0 * 1024.0
        && steady_physical_rate <= 8.0 * 1024.0 * 1024.0
        && percentile(warm, 95).is_some_and(|ms| ms <= 30_000.0)
        && percentile(&controls, 95).is_some_and(|ms| ms <= 150.0)
        && peak_rss.saturating_sub(baseline_rss) <= 128 * 1024 * 1024
        && diagnostics.sources.len() == size
        && diagnostics.inventory_size == size
        && diagnostics
            .sources
            .values()
            .all(|source| source.available && source.enabled == 0 && source.pending_targets == 0);
    let report = serde_json::json!({"size":size,"probe_integrity":probe_integrity,"registered_test_probes":registered_probes,"retained_test_probes":retained_probes,"setup_method":"native historical seed initialization and one validated durable catalog publication before measurement","mode":if sparse{"sparse_with_unavailable_and_repair"}else{"zero_enabled_zero_pending"},"optimized":!cfg!(debug_assertions),"setup_seconds":setup_started.elapsed().as_secs_f64()-elapsed,"passes":passes,"elapsed_seconds":elapsed,"cpu_seconds":cpu,"cpu_core_equivalents":cpu_cores,"startup_cpu_seconds":startup_cpu-cpu_before,"steady_elapsed_seconds":warm_elapsed,"steady_cpu_core_equivalents":steady_cpu_cores,"steady_logical_read_bytes_per_second":steady_logical_rate,"steady_physical_read_bytes_per_second":steady_physical_rate,"database_open_path_entries":opens,"logical_read_bytes_per_second":logical_rate,"physical_read_bytes_per_second":physical_rate,"startup_cold_database_cache_ms":durations.first(),"warm_pass_p50_ms":percentile(warm,50),"warm_pass_p95_ms":percentile(warm,95),"foreground_read_p95_ms":percentile(&controls,95),"write_during_traversal_ms":write_ms,"rss_baseline_bytes":baseline_rss,"rss_peak_sampled_bytes":peak_rss,"rss_delta_bytes":peak_rss.saturating_sub(baseline_rss),"diagnostic_pass_bound_ms":diagnostics.pass_bound_ms,"pass_overruns":diagnostics.pass_overruns,"unavailable_sources":diagnostics.sources.values().filter(|source|!source.available).count(),"release_gate_eligible":eligible,"release_gate_passed":within_budget,"cache_method":"fsync plus per-database POSIX_FADV_DONTNEED before startup; catalog metadata remains warm; later passes use ordinary kernel cache","logical_cpu_count":std::thread::available_parallelism().unwrap().get(),"kernel":std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap().trim(),"schema":crate::durable::sqlite::SQLITE_SCHEMA_VERSION});
    eprintln!("SCHEDULER_BENCHMARK_JSON={report}");
    if let Some(output) = std::env::var_os("RUSTX_SCHEDULER_BENCH_JSON") {
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert_eq!(
        retained_probes, registered_probes,
        "unregistered reads must not allocate test probes into resource measurements"
    );
    if release_gate {
        assert!(
            within_budget,
            "native Scheduler idle release budget failed: {report}"
        );
    }
    f.close().await;
}
