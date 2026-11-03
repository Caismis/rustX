//! One connection-owned fold. Provisional state stays private to the writer;
//! only the store guard after COMMIT publishes a reusable committed certificate.
use super::history::Frame;
use super::{Connection, ConversationStoreError, storage};
use crate::durable::sqlite::statements::FixedStatements;
use rusqlite::hooks::PreUpdateCase;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Certificate {
    frame: Frame,
    epoch: u64,
    data: u64,
    fact: Vec<u8>,
    checkpoint: Option<Vec<u8>>,
}
#[derive(Default)]
struct Seat {
    committed: Option<Certificate>,
    pending: Option<Certificate>,
}
pub(in crate::durable::sqlite) struct FoldCache {
    epoch: AtomicU64,
    enabled: AtomicBool,
    history_started: AtomicBool,
    columns: Vec<i32>,
    base_columns: Vec<i32>,
    seat: Mutex<Seat>,
}
fn columns(
    connection: &Connection,
    table: &str,
    wanted: &[&str],
) -> Result<Vec<i32>, ConversationStoreError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA main.table_xinfo({table})"))
        .map_err(|error| storage(error.to_string()))?;
    let actual = statement
        .query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    wanted
        .iter()
        .map(|name| {
            actual
                .iter()
                .find(|(_, candidate)| candidate.as_str() == *name)
                .map(|(index, _)| *index)
                .ok_or_else(|| storage("statistics dependency column unavailable"))
        })
        .collect()
}
impl FoldCache {
    pub(in crate::durable::sqlite) fn new(
        connection: &Connection,
    ) -> Result<Arc<Self>, ConversationStoreError> {
        Ok(Arc::new(Self {
            epoch: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            history_started: AtomicBool::new(false),
            columns: columns(
                connection,
                "events",
                &[
                    "sequence",
                    "observation_statistics",
                    "observation_statistics_checkpoint",
                    "observation_statistics_required",
                ],
            )?,
            base_columns: columns(
                connection,
                "rustx_store",
                &[
                    "observation_statistics",
                    "bootstrap_initialized",
                    "next_event_sequence",
                ],
            )?,
            seat: Mutex::new(Seat::default()),
        }))
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn uninstalled() -> Arc<Self> {
        Arc::new(Self {
            epoch: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            history_started: AtomicBool::new(false),
            columns: Vec::new(),
            base_columns: Vec::new(),
            seat: Mutex::new(Seat::default()),
        })
    }
    pub(in crate::durable::sqlite) fn invalidate(&self) {
        let _ = self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_add(1))
            });
    }
    pub(in crate::durable::sqlite) fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        self.invalidate();
    }
    pub(in crate::durable::sqlite) fn observe(&self, table: &str, change: &PreUpdateCase) {
        let selected = match table {
            "events" => &self.columns,
            "rustx_store" => &self.base_columns,
            _ => return,
        };
        let changed = match change {
            PreUpdateCase::Update {
                old_value_accessor: old,
                new_value_accessor: new,
            } => selected.iter().any(|column| {
                match (
                    old.get_old_column_value(*column),
                    new.get_new_column_value(*column),
                ) {
                    (Ok(a), Ok(b)) => a != b,
                    _ => true,
                }
            }),
            PreUpdateCase::Insert(new) if table == "events" => {
                selected.get(3).is_none_or(|column| {
                    !new.get_new_column_value(*column)
                        .is_ok_and(|value| value == rusqlite::types::ValueRef::Integer(0))
                }) || selected.get(1..3).is_none_or(|columns| {
                    columns.iter().any(|column| {
                        !new.get_new_column_value(*column)
                            .is_ok_and(|value| value == rusqlite::types::ValueRef::Null)
                    })
                })
            }
            _ => true,
        };
        if table == "events" && matches!(change, PreUpdateCase::Insert(_)) {
            if changed {
                self.history_started.store(true, Ordering::Release);
                self.invalidate();
            } else if let PreUpdateCase::Insert(new) = change {
                let seat = self.seat.lock().expect("statistics fold");
                if let Some(pending) = &seat.pending {
                    // A later unrelated append preserves the prefix. A reused
                    // provisional sequence can follow ROLLBACK TO, so fence it.
                    let later = new.get_new_column_value(self.columns[0]).is_ok_and(|value| {
                        matches!(value, rusqlite::types::ValueRef::Integer(sequence)
                            if u64::try_from(sequence).is_ok_and(|sequence| sequence > pending.frame.sequence))
                    });
                    if !later {
                        self.invalidate();
                    }
                }
            }
        } else if changed {
            // Product statistics are append-only after bootstrap. An UPDATE,
            // DELETE or bootstrap rewrite disables reuse for this connection,
            // including changes later undone by ROLLBACK TO.
            let base_changed = if table == "rustx_store" {
                match change {
                    PreUpdateCase::Update {
                        old_value_accessor: old,
                        new_value_accessor: new,
                    } => selected.iter().take(2).any(|column| {
                        match (
                            old.get_old_column_value(*column),
                            new.get_new_column_value(*column),
                        ) {
                            (Ok(a), Ok(b)) => a != b,
                            _ => true,
                        }
                    }),
                    _ => true,
                }
            } else {
                true
            };
            if base_changed {
                if table == "events" || self.history_started.load(Ordering::Acquire) {
                    self.disable();
                } else {
                    self.invalidate();
                }
            } else if let PreUpdateCase::Update {
                old_value_accessor: old,
                new_value_accessor: new,
            } = change
            {
                let seat = self.seat.lock().expect("statistics fold");
                if let Some(pending) = &seat.pending {
                    // Ordinary cursor advancement cannot remove a prefix. An
                    // old cursor below the certificate exposes ROLLBACK TO;
                    // a rewind or invalid value is not a native append.
                    let forward = match (
                        old.get_old_column_value(self.base_columns[2]),
                        new.get_new_column_value(self.base_columns[2]),
                    ) {
                        (
                            Ok(rusqlite::types::ValueRef::Integer(old)),
                            Ok(rusqlite::types::ValueRef::Integer(new)),
                        ) => {
                            u64::try_from(old).is_ok_and(|old| old >= pending.frame.sequence)
                                && new >= old
                        }
                        _ => false,
                    };
                    if !forward {
                        self.invalidate();
                    }
                }
            }
        }
    }
    pub(super) fn take(
        &self,
        connection: &Connection,
        through: u64,
    ) -> Result<(u64, Option<Frame>), ConversationStoreError> {
        let data: u64 = connection
            .query_row_cached("PRAGMA main.data_version", [], |row| row.get(0))
            .map_err(|error| storage(error.to_string()))?;
        let mut seat = self.seat.lock().expect("statistics fold");
        let certificate = seat.pending.take().or_else(|| seat.committed.take());
        let epoch = self.epoch.load(Ordering::Acquire);
        let frame = certificate
            .filter(|certificate| {
                self.enabled.load(Ordering::Acquire)
                    && epoch != u64::MAX
                    && certificate.epoch == epoch
                    && certificate.data == data
                    && certificate.frame.sequence <= through
            })
            .map(|certificate| certificate.frame);
        #[cfg(test)]
        WORK.with(|work| {
            let (hits, misses) = work.get();
            work.set((
                hits + usize::from(frame.is_some()),
                misses + usize::from(frame.is_none()),
            ));
        });
        Ok((data, frame))
    }
    pub(super) fn stage(
        &self,
        frame: Frame,
        data: u64,
        fact: Vec<u8>,
        checkpoint: Option<Vec<u8>>,
    ) {
        if self.enabled.load(Ordering::Acquire) {
            self.seat.lock().expect("statistics fold").pending = Some(Certificate {
                frame,
                epoch: self.epoch.load(Ordering::Acquire),
                data,
                fact,
                checkpoint,
            });
        }
    }
    pub(in crate::durable::sqlite) fn publish(&self, connection: &Connection) {
        let pending = self.seat.lock().expect("statistics fold").pending.take();
        let Some(certificate) = pending else { return };
        // Publish after COMMIT only if the actual newest statistical fact
        // and its optional checkpoint are exactly the ones we staged. Borrow
        // SQLite cells; no copy, new state row or transaction is introduced.
        let accepted = connection.is_autocommit()
            && self.enabled.load(Ordering::Acquire)
            && certificate.epoch == self.epoch.load(Ordering::Acquire)
            && connection.prepare_cached(super::super::event_heads::STATISTICS)
                .and_then(|mut statement|statement.query_row([i64::MAX],|row|{
                    let sequence:u64=row.get(0)?;
                    let fact=matches!(row.get_ref(1)?,rusqlite::types::ValueRef::Blob(bytes) if bytes==certificate.fact);
                    let checkpoint=match(row.get_ref(2)?,certificate.checkpoint.as_deref()){
                        (rusqlite::types::ValueRef::Null,None)=>true,
                        (rusqlite::types::ValueRef::Blob(bytes),Some(expected))=>bytes==expected,
                        _=>false,
                    };
                    Ok(sequence==certificate.frame.sequence&&fact&&checkpoint)
                })).unwrap_or(false);
        if accepted {
            self.seat.lock().expect("statistics fold").committed = Some(certificate);
        }
    }
}
#[cfg(test)]
thread_local! {static WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn work() -> (usize, usize) {
    WORK.with(|work| work.replace((0, 0)))
}
