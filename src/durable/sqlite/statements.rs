//! Single-statement helpers for the fixed native read/write working set.
//!
//! Only plans are retained by rusqlite. Parameters, rows, cuts and transaction
//! evidence are evaluated on every call, and the statement returns to the
//! bounded connection cache before this helper returns.
use rusqlite::{Connection, Params, Result, Row};

pub(super) trait FixedStatements {
    fn query_row_cached<T, P, F>(&self, sql: &'static str, params: P, map: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>;

    fn execute_cached<P: Params>(&self, sql: &'static str, params: P) -> Result<usize>;
}

impl FixedStatements for Connection {
    fn query_row_cached<T, P, F>(&self, sql: &'static str, params: P, map: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.prepare_cached(sql)?.query_row(params, map)
    }

    fn execute_cached<P: Params>(&self, sql: &'static str, params: P) -> Result<usize> {
        self.prepare_cached(sql)?.execute(params)
    }
}
