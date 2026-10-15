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
    /// Historical projection activation prefix, independent of canonical commits.
    pub projections: i64,
    pub projection_head: Option<crate::runtime::identity::CheckpointId>,
    pub publication_audits: i64,
    /// Presence frontier of the immutable lineage bootstrap row, shared by
    /// completed-response and turn-reading provenance.
    pub bootstrap: i64,
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
        super::provenance::turns(&conn).map_err(error)?;
        super::provenance::responses(&conn).map_err(error)?;
        let max = |table: &str, key: &str| {
            conn.query_row(
                &format!("SELECT COALESCE(MAX({key}),0) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .map_err(error)
        };
        let projection_head = super::projection::head(&conn).map_err(error)?;
        // Current dependency closure is independently checked; historical snapshots
        // may be deliberately exported lossily without contaminating active execution.
        super::projection::history(&conn, &self.conversation_id).map_err(error)?;
        Ok(ConversationArchiveFrontiers {
            journal: max("events", "sequence")?,
            messages: max("message_ledger", "position")?,
            surface: max("surface_ops", "revision")?,
            requests: max("request_snapshots", "rowid")?,
            projections: max("projection_activations", "activation")?,
            projection_head: projection_head.map(|checkpoint| checkpoint.candidate.checkpoint_id),
            publication_audits: max("publication_audits", "rowid")?,
            bootstrap: max("bootstrap_identity", "id")?,
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
        if matches!(authority, Authority::Projections) {
            let connection = self.lock().map_err(error)?;
            let checkpoint = super::projection::timeline(
                &connection,
                crate::context::checkpoint::ProjectionActivationId(
                    u64::try_from(after.max(0)).map_err(error)?,
                ),
                1,
            )
            .map_err(error)?
            .into_iter()
            .next();
            return checkpoint
                .filter(|checkpoint| {
                    checkpoint.activation.activation.0 <= u64::try_from(through).unwrap_or(0)
                })
                .map(|checkpoint| {
                    Ok((
                        i64::try_from(checkpoint.activation.activation.0).map_err(error)?,
                        serde_json::to_string(&checkpoint).map_err(error)?,
                    ))
                })
                .transpose();
        }
        if let Some(table) = match authority {
            Authority::InheritedResponses => Some("inherited_response_provenance"),
            Authority::InheritedTurns => Some("inherited_turn_provenance"),
            _ => None,
        } {
            return self.lock().map_err(error)?.query_row(
                &format!("SELECT ordinal,payload FROM {table} WHERE ordinal>?1 AND EXISTS(SELECT 1 FROM bootstrap_identity WHERE id<=?2) ORDER BY ordinal LIMIT 1"),
                params![after, through], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(error);
        }
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
    Projections,
    PublicationAudits,
    InheritedResponses,
    InheritedTurns,
}
impl Authority {
    fn columns(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Journal => ("events", "sequence", "event_json"),
            Self::Messages => ("message_ledger", "position", "message_json"),
            Self::Surface => ("surface_ops", "revision", "op_json"),
            Self::Requests => ("request_snapshots", "rowid", "snapshot_json"),
            Self::PublicationAudits => ("publication_audits", "rowid", "audit_json"),
            Self::InheritedResponses | Self::InheritedTurns | Self::Projections => {
                unreachable!("bootstrap provenance uses its immutable presence frontier")
            }
        }
    }
}
