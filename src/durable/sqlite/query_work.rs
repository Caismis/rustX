//! SQL complexity contracts, measured with the same bundled SQLite as the store.
//!
//! A LIMIT is not a work bound. Measure the executed statement, not just its
//! returned rows or application decoder calls. Fixtures must vary retained
//! history independently of the requested window/current work.
use rusqlite::{
    Connection, Params, StatementStatus,
    trace::{TraceEvent, TraceEventCodes},
};
use std::cell::RefCell;
use std::collections::BTreeMap;

pub(super) const HISTORY_SIZES: [usize; 3] = [32, 320, 3200];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Work {
    pub steps: i32,
    pub scans: i32,
    pub sorts: i32,
    pub rows: usize,
    pub bytes: usize,
}

pub(super) fn measure(connection: &Connection, sql: &str, values: impl Params) -> Work {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    let mut work = Work::default();
    {
        let mut rows = statement.query(values).unwrap();
        while let Some(row) = rows.next().unwrap() {
            work.rows += 1;
            for column in 0..columns {
                use rusqlite::types::ValueRef;
                work.bytes += match row.get_ref(column).unwrap() {
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
                    ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                    ValueRef::Null => 0,
                };
            }
        }
    }
    work.steps = statement.get_status(StatementStatus::VmStep);
    work.scans = statement.get_status(StatementStatus::FullscanStep);
    work.sorts = statement.get_status(StatementStatus::Sort);
    assert_eq!(statement.get_status(StatementStatus::AutoIndex), 0, "{sql}");
    work
}

/// Strictly flat VM work (B-tree seeks do not count tree height as VM steps),
/// no sorter/automatic index, plus an absolute row/byte and instruction ceiling.
pub(super) fn assert_flat(
    samples: &[(usize, Work)],
    max_steps: i32,
    max_rows: usize,
    max_bytes: usize,
) {
    assert!(samples.len() >= 3);
    assert!(samples.last().unwrap().0 >= samples[0].0 * 100);
    let baseline = samples[0].1.steps;
    for &(history, work) in samples {
        assert_eq!(work.steps, baseline, "history {history}: {samples:?}");
        assert_eq!(work.sorts, 0, "history {history}: {work:?}");
        assert!(
            work.steps <= max_steps && work.rows <= max_rows && work.bytes <= max_bytes,
            "history {history}: {work:?}"
        );
    }
}

#[derive(Debug, Default)]
pub(super) struct Executions {
    pub calls: usize,
    pub steps: i32,
    /// VM steps summed over every execution.
    pub total_steps: i64,
    /// Rows returned, summed over every execution.
    pub rows: usize,
    pub scans: i32,
    pub sorts: i32,
    pub automatic: i32,
}

#[derive(Default)]
struct Trace {
    before: BTreeMap<String, [i32; 4]>,
    completed: BTreeMap<String, Executions>,
    phase: &'static str,
    phases: BTreeMap<(&'static str, String), PhaseWork>,
}

/// Work of one statement text inside one labelled phase of a measured run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PhaseWork {
    pub calls: usize,
    /// Executions whose statement had never run before, i.e. fresh prepares.
    pub prepares: usize,
    pub steps: i64,
    pub rows: usize,
}
thread_local! { static TRACE: RefCell<Trace> = RefCell::default(); }

fn counters(statement: &rusqlite::trace::StmtRef<'_>) -> [i32; 4] {
    [
        StatementStatus::VmStep,
        StatementStatus::FullscanStep,
        StatementStatus::Sort,
        StatementStatus::AutoIndex,
    ]
    .map(|status| statement.get_status(status))
}

fn record(event: TraceEvent<'_>) {
    TRACE.with_borrow_mut(|trace| match event {
        TraceEvent::Stmt(statement, text) if !text.starts_with("--") => {
            let sql = statement.sql().into_owned();
            if statement.get_status(StatementStatus::Run) == 0 && !trace.before.contains_key(&sql) {
                trace
                    .phases
                    .entry((trace.phase, sql.clone()))
                    .or_default()
                    .prepares += 1;
            }
            trace.before.insert(sql, counters(&statement));
        }
        TraceEvent::Profile(statement, _) => {
            let sql = statement.sql().into_owned();
            if let Some(before) = trace.before.remove(&sql) {
                let after = counters(&statement);
                let phase = trace.phases.entry((trace.phase, sql.clone())).or_default();
                phase.calls += 1;
                phase.steps += i64::from(after[0] - before[0]);
                let execution = trace.completed.entry(sql).or_default();
                execution.calls += 1;
                execution.steps = execution.steps.max(after[0] - before[0]);
                execution.total_steps += i64::from(after[0] - before[0]);
                execution.scans = execution.scans.max(after[1] - before[1]);
                execution.sorts = execution.sorts.max(after[2] - before[2]);
                execution.automatic = execution.automatic.max(after[3] - before[3]);
            }
        }
        TraceEvent::Row(statement) => {
            let sql = statement.sql().into_owned();
            trace
                .phases
                .entry((trace.phase, sql.clone()))
                .or_default()
                .rows += 1;
            trace.completed.entry(sql).or_default().rows += 1;
        }
        _ => {}
    });
}

/// Observe *all* statements of an actual store operation, including prepared
/// direct reads/writes and SQL hidden behind helpers. Counters are per execution,
/// not cumulative statement-cache totals. Only test builds enable SQLite trace.
pub(super) fn capture(connection: &Connection) {
    TRACE.with_borrow_mut(|trace| *trace = Trace::default());
    connection.trace_v2(
        TraceEventCodes::SQLITE_TRACE_STMT
            | TraceEventCodes::SQLITE_TRACE_PROFILE
            | TraceEventCodes::SQLITE_TRACE_ROW,
        Some(record),
    );
}

pub(super) fn finish(connection: &Connection) -> BTreeMap<String, Executions> {
    connection.trace_v2(TraceEventCodes::empty(), None);
    TRACE.with_borrow_mut(|trace| {
        let completed = std::mem::take(&mut trace.completed);
        for (sql, work) in &completed {
            if work.sorts > 0 || work.automatic > 0 {
                println!("SQL audit sorter/automatic index {work:?}: {sql}");
            }
        }
        completed
    })
}

/// Labels the statements traced from now on; see [`finish_phases`].
pub(crate) fn phase(label: &'static str) {
    TRACE.with_borrow_mut(|trace| trace.phase = label);
}

/// Takes the per-phase work recorded since the last [`capture`].
pub(crate) fn finish_phases() -> BTreeMap<(&'static str, String), PhaseWork> {
    TRACE.with_borrow_mut(|trace| std::mem::take(&mut trace.phases))
}
