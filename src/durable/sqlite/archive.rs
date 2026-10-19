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
    /// Existing, bound WAL access solely for short archive writer exclusion.
    /// No creation, journal conversion or application recovery is permitted.
    pub(crate) fn open_for_archive(
        conversation_id: crate::runtime::identity::ConversationId,
        path: &std::path::Path,
    ) -> Result<Self, super::ConversationStoreError> {
        let store = Self::open_bound_existing(
            conversation_id,
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let mode: String = store
            .lock()?
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|error| super::storage(error.to_string()))?;
        if mode != "wal" {
            return Err(super::storage(
                "archive capture requires writer-established WAL storage",
            ));
        }
        Ok(store)
    }

    /// Exclude every SQLite writer, including other processes. Acquire in
    /// `ConversationId` order and retain all barriers through the final frontier.
    /// Contention fails immediately, allowing cancellation/partial cleanup
    /// without an unbounded wait while earlier databases remain excluded.
    pub(crate) fn archive_barrier(&self) -> io::Result<()> {
        let conn = self.lock().map_err(error)?;
        conn.busy_timeout(std::time::Duration::ZERO)
            .map_err(error)?;
        let acquired = conn.execute_batch("BEGIN IMMEDIATE").map_err(error);
        let restored = super::writer_busy_policy(&conn).map_err(error);
        acquired.and(restored)
    }

    pub(crate) fn archive_release(&self) -> io::Result<()> {
        self.lock()
            .map_err(error)?
            .execute_batch("ROLLBACK")
            .map_err(error)
    }

    pub(crate) fn archive_frontiers(
        &self,
    ) -> io::Result<(
        ConversationArchiveFrontiers,
        crate::context::resources::ResourceReadCut,
    )> {
        let conn = self.lock().map_err(error)?;
        let max = |table: &str, key: &str| {
            conn.query_row(
                &format!("SELECT COALESCE(MAX({key}),0) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .map_err(error)
        };
        let projection_head = super::projection::current_checkpoint_id(&conn).map_err(error)?;
        let read_cut =
            super::compaction_resources::cut(&conn, &self.conversation_id).map_err(error)?;
        Ok((
            ConversationArchiveFrontiers {
                journal: max("events", "sequence")?,
                messages: max("message_ledger", "position")?,
                surface: max("surface_ops", "revision")?,
                requests: max("request_snapshots", "rowid")?,
                projections: max("projection_activations", "activation")?,
                projection_head,
                publication_audits: max("publication_audits", "rowid")?,
                bootstrap: max("bootstrap_identity", "id")?,
            },
            read_cut,
        ))
    }

    /// Expensive integrity/dependency work belongs after capture, in a scoped
    /// reader which ends before ZIP delivery. It selects the captured head/Tail.
    pub(crate) fn archive_validate(
        &self,
        frontiers: &ConversationArchiveFrontiers,
        read_cut: &crate::context::resources::ResourceReadCut,
    ) -> io::Result<()> {
        let conn = self.lock().map_err(error)?;
        let transaction = conn.unchecked_transaction().map_err(error)?;
        super::provenance::turns(&transaction).map_err(error)?;
        super::provenance::responses(&transaction).map_err(error)?;
        super::projection::validate_auxiliary_lineage_store(&transaction, read_cut)
            .map_err(error)?;
        super::projection::archive_history(
            &transaction,
            &self.conversation_id,
            read_cut,
            frontiers.projection_head.as_ref(),
        )
        .map_err(error)
    }

    /// Inspection-only inherited Summary generation records at their captured
    /// source cuts. These are destination-owned facts, never local Call rows.
    pub(crate) fn archive_inherited_auxiliary_audit_rows(
        &self,
    ) -> io::Result<Vec<serde_json::Value>> {
        let connection = self.lock().map_err(error)?;
        super::projection::inherited_auxiliary_archive_rows(&connection).map_err(error)
    }

    /// Fetch one immutable logical record; the database cursor/read snapshot is
    /// gone before the consumer sees it or waits on downstream backpressure.
    pub(crate) fn archive_record(
        &self,
        authority: Authority,
        after: i64,
        through: i64,
    ) -> io::Result<Option<(i64, String)>> {
        use rusqlite::OptionalExtension;
        if matches!(authority, Authority::Journal) {
            // Bounds apply before copying a corrupted auxiliary payload into Rust.
            return self.lock().map_err(error)?.query_row(
                "SELECT sequence,CASE WHEN
                    (json_extract(event_json,'$.event.type') IN ('auxiliary_call_started','auxiliary_call_settled')
                     OR json_extract(event_json,'$.event.owner.type')='auxiliary')
                    AND octet_length(event_json)>16384 THEN NULL ELSE event_json END
                 FROM events WHERE sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT 1",
                params![after, through],
                |row| Ok((row.get(0)?, row.get::<_, String>(1)?)),
            ).optional().map_err(error);
        }
        if matches!(authority, Authority::Requests) {
            let connection = self.lock().map_err(error)?;
            let selected: Option<(i64, Option<String>)> = connection.query_row(
                "SELECT rowid,CASE WHEN octet_length(request_id)<=67108864 THEN request_id END FROM request_snapshots
                 WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
                params![after, through], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(error)?;
            return selected
                .map(|(ordinal, request)| {
                    let request = request
                        .ok_or_else(|| error("archived request identity exceeds byte bound"))?;
                    // Archives expose the original logical Snapshot, not its
                    // normalized storage metadata. The native loader checks the
                    // exact root and the combined 64 MiB bound before hydration.
                    let snapshot = super::load_request_snapshot(
                        &connection,
                        &self.conversation_id,
                        &crate::runtime::identity::RequestId::new(request),
                    )
                    .map_err(error)?;
                    Ok((ordinal, serde_json::to_string(&snapshot).map_err(error)?))
                })
                .transpose();
        }
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
            Self::PublicationAudits => ("publication_audits", "rowid", "audit_json"),
            Self::Requests
            | Self::InheritedResponses
            | Self::InheritedTurns
            | Self::Projections => {
                unreachable!("this authority requires its native logical record reader")
            }
        }
    }
}
