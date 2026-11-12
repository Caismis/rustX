//! Fixed write work of one warmed completed turn through the production
//! append path. Every count is deterministic: statements executed, fresh
//! prepares, SQLite VM steps, commits and WAL frames, never timing.
use super::query_work::{self, PhaseWork};
use crate::durable::CanonicalStore;
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TurnWork {
    calls: usize,
    prepares: usize,
    steps: i64,
    commits: usize,
    wal_frames: u64,
}

const WAL_HEADER: u64 = 32;
const WAL_FRAME: u64 = 24 + 512;

fn truncate_wal(store: &super::SqliteConversationStore) {
    store
        .lock()
        .unwrap()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
}

fn measure(tool: bool) -> (TurnWork, BTreeMap<(&'static str, String), PhaseWork>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.sqlite");
    let store = super::SqliteConversationStore::open(
        crate::runtime::identity::ConversationId::generate(),
        &path,
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    super::append_completed_turns(&store, 0, 8, tool);
    truncate_wal(&store);
    store.begin_query_work();
    super::append_completed_turns(&store, 8, 1, tool);
    let _ = store.finish_query_work();
    let wal = std::fs::metadata(path.with_extension("sqlite-wal"))
        .unwrap()
        .len();
    let phases = query_work::finish_phases();
    let mut work = TurnWork {
        wal_frames: wal.saturating_sub(WAL_HEADER) / WAL_FRAME,
        ..TurnWork::default()
    };
    for ((_, sql), entry) in &phases {
        work.calls += entry.calls;
        work.prepares += entry.prepares;
        work.steps += entry.steps;
        if sql == "COMMIT" {
            work.commits += entry.calls;
        }
    }
    (work, phases)
}

fn report(tool: bool, phases: &BTreeMap<(&'static str, String), PhaseWork>) {
    let mut totals: BTreeMap<&str, PhaseWork> = BTreeMap::new();
    for ((phase, _), entry) in phases {
        let total = totals.entry(phase).or_default();
        total.calls += entry.calls;
        total.prepares += entry.prepares;
        total.steps += entry.steps;
        total.rows += entry.rows;
    }
    for (phase, total) in &totals {
        eprintln!("PHASE tool={tool} {phase} {total:?}");
    }
    for ((phase, sql), entry) in phases {
        eprintln!(
            "STMT tool={tool} phase={phase} calls={} prepares={} steps={} rows={} sql={sql:?}",
            entry.calls, entry.prepares, entry.steps, entry.rows
        );
    }
}

/// Set `RUSTX_TURN_WORK` to print the per-phase, per-statement breakdown.
#[test]
fn warmed_turn_write_work_is_fixed() {
    let expected = [
        TurnWork {
            calls: 621,
            prepares: 18,
            steps: 20_413,
            commits: 7,
            wal_frames: 258,
        },
        TurnWork {
            calls: 1392,
            prepares: 38,
            steps: 45_266,
            commits: 13,
            wal_frames: 553,
        },
    ];
    let actual = [false, true].map(|tool| {
        let (work, phases) = measure(tool);
        if std::env::var_os("RUSTX_TURN_WORK").is_some() {
            report(tool, &phases);
        }
        work
    });
    assert_eq!(actual, expected, "[text, tool]");
}
