//! Read barriers and immutable prefixes of the existing `SQLite` authorities.
use super::SqliteConversationStore;
use rusqlite::params;
use serde::Serialize;
use std::io;

/// Native immutable append frontiers. Request/audit rowids are insertion order,
/// not a new persisted identity; only their immutable bodies are exported.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConversationArchiveFrontiers {
    pub journal: i64,
    pub messages: i64,
    pub surface: i64,
    pub requests: i64,
    pub publication_audits: i64,
}

pub(crate) fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

impl SqliteConversationStore {
    /// Establish a rollback-journal SHARED lock. All included databases retain
    /// these barriers until the last frontier has been observed.
    pub(crate) fn archive_barrier(&self) -> io::Result<()> {
        let conn = self.lock().map_err(error)?;
        conn.execute_batch("BEGIN DEFERRED").map_err(error)?;
        conn.query_row(
            "SELECT schema_version FROM rustx_store WHERE id=1",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(error)?;
        Ok(())
    }

    pub(crate) fn archive_release(&self) -> io::Result<()> {
        self.lock()
            .map_err(error)?
            .execute_batch("ROLLBACK")
            .map_err(error)
    }

    pub(crate) fn archive_frontiers(&self) -> io::Result<ConversationArchiveFrontiers> {
        let conn = self.lock().map_err(error)?;
        let max = |table: &str, key: &str| {
            conn.query_row(
                &format!("SELECT COALESCE(MAX({key}),0) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .map_err(error)
        };
        Ok(ConversationArchiveFrontiers {
            journal: max("events", "sequence")?,
            messages: max("message_ledger", "position")?,
            surface: max("surface_ops", "revision")?,
            requests: max("request_snapshots", "rowid")?,
            publication_audits: max("publication_audits", "rowid")?,
        })
    }

    /// Fetch one immutable logical record; the database cursor/SHARED lock is
    /// gone before the consumer sees it or waits on downstream backpressure.
    pub(crate) fn archive_record(
        &self,
        authority: Authority,
        after: i64,
        through: i64,
    ) -> io::Result<Option<(i64, String)>> {
        use rusqlite::OptionalExtension;
        let (table, key, body) = authority.columns();
        self.lock().map_err(error)?.query_row(
            &format!("SELECT {key},{body} FROM {table} WHERE {key}>?1 AND {key}<=?2 ORDER BY {key} LIMIT 1"),
            params![after, through], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Authority {
    Journal,
    Messages,
    Surface,
    Requests,
    PublicationAudits,
}
impl Authority {
    fn columns(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Journal => ("events", "sequence", "event_json"),
            Self::Messages => ("message_ledger", "position", "message_json"),
            Self::Surface => ("surface_ops", "revision", "op_json"),
            Self::Requests => ("request_snapshots", "rowid", "snapshot_json"),
            Self::PublicationAudits => ("publication_audits", "rowid", "audit_json"),
        }
    }
}
