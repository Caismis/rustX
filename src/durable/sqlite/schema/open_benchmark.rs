//! Explicit diagnostic over fresh native SQLite connections; never a release gate.
use super::*;
use crate::durable::{CanonicalStore, ScheduleStore};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[test]
#[ignore = "native schema/open cost diagnostic; run in isolation with --release --nocapture"]
#[allow(clippy::too_many_lines)] // One instrumented fresh-connection sequence.
fn measure_native_schema_open_cost() {
    let iterations = std::env::var("RUSTX_SCHEMA_BENCH_ITERATIONS")
        .map_or(1000, |value| value.parse::<u32>().unwrap());
    assert!(iterations > 0);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-history.sqlite");
    let id = ConversationId::generate();
    {
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store
            .append_canonical(&crate::durable::sqlite::test_support::user_message(
                "schema-benchmark-history",
                "Historical native conversation",
            ))
            .unwrap();
    }
    let started = Instant::now();
    for _ in 0..iterations {
        let scope = Arc::new(super::super::schedule_read::ScheduleReadScope::default());
        let store =
            SqliteConversationStore::open_schedule_reader(id.clone(), &path, &scope).unwrap();
        let summary = store.schedule_summary().unwrap();
        assert_eq!(summary.enabled, 0);
        assert!(summary.pending.is_empty());
    }
    let native = started.elapsed();
    let mut initialization = Duration::ZERO;
    let mut shape = Duration::ZERO;
    let mut identity = Duration::ZERO;
    let mut finish = Duration::ZERO;
    let selections = Arc::new(AtomicU64::new(0));
    for _ in 0..iterations {
        let started = Instant::now();
        let connection =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let scope = Arc::new(super::super::schedule_read::ScheduleReadScope::default());
        scope.install(&connection).unwrap();
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        register_integrity_function(&connection).unwrap();
        initialization += started.elapsed();
        let count = selections.clone();
        connection
            .authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if matches!(context.action, rusqlite::hooks::AuthAction::Select) {
                    count.fetch_add(1, Ordering::Relaxed);
                }
                rusqlite::hooks::Authorization::Allow
            }))
            .unwrap();
        let started = Instant::now();
        validate_existing_schema(&connection).unwrap();
        shape += started.elapsed();
        // Count only compilation during complete schema verification.
        connection
            .authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            )
            .unwrap();
        let started = Instant::now();
        validate_existing_identity(&connection, &id).unwrap();
        identity += started.elapsed();
        let started = Instant::now();
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
            .unwrap();
        let store =
            SqliteConversationStore::from_validated_connection(id.clone(), connection).unwrap();
        assert!(store.schedule_summary().unwrap().pending.is_empty());
        drop(store);
        finish += started.elapsed();
    }
    let per_open_ms = |duration: Duration| duration.as_secs_f64() * 1000.0 / f64::from(iterations);
    let report = serde_json::json!({
        "schema": SQLITE_SCHEMA_VERSION,
        "optimized": !cfg!(debug_assertions),
        "iterations": iterations,
        "full_native_read_ms_per_open": per_open_ms(native),
        "initialization_ms_per_open": per_open_ms(initialization),
        "complete_schema_ms_per_open": per_open_ms(shape),
        "identity_ms_per_open": per_open_ms(identity),
        "owner_summary_close_ms_per_open": per_open_ms(finish),
        "schema_select_compilations_per_open": selections.load(Ordering::Relaxed) / u64::from(iterations),
        "method": "fresh read-only connections to one warm native historical database; authorizer counts SELECT compilation only in split schema validation; diagnostic excludes manager/allocation/driver and is not the scale gate"
    });
    eprintln!("SCHEMA_OPEN_BENCHMARK_JSON={report}");
    if let Some(output) = std::env::var_os("RUSTX_SCHEMA_BENCH_JSON") {
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
