//! Original owner fields, indexed at their native provenance commit.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::observation::turn::{AttemptIdentitySource, InheritedAttemptIdentityKind as Kind};

fn label(kind: Kind) -> &'static str {
    match kind {
        Kind::Turn => "turn",
        Kind::Response => "response",
    }
}
fn domain(kind: Kind, ordinal: u64) -> String {
    match kind {
        Kind::Turn => format!("transcript.turns.{ordinal}.identity"),
        Kind::Response => format!("transcript.responses.{ordinal}.identity"),
    }
}
fn key(kind: Kind, ordinal: u64) -> String {
    format!("{}:{ordinal}", label(kind))
}

pub(super) fn capture(
    connection: &Connection,
    kind: Kind,
    ordinal: u64,
    attempt: &crate::runtime::identity::AttemptId,
) -> Result<(), ConversationStoreError> {
    super::content::index_structural_value(
        connection,
        "inherited_attempt_identity",
        &key(kind, ordinal),
        0,
        &attempt.as_str(),
    )?;
    Ok(())
}
pub(super) fn inherited(
    connection: &Connection,
    kind: Kind,
    ordinal: u64,
) -> Result<AttemptIdentitySource, ConversationStoreError> {
    let encoded_bytes: u64 = connection.query_row(
        "SELECT readable_end FROM content_frontiers WHERE source_kind='inherited_attempt_identity' AND source_id=?1 AND block_index=0 AND representation='canonical_json_v1' AND revision=1",
        [key(kind,ordinal)], |row| row.get(0),
    ).map_err(|_| storage("original inherited owner field unavailable"))?;
    Ok(AttemptIdentitySource::Inherited {
        kind,
        ordinal,
        domain: domain(kind, ordinal),
        encoded_bytes,
    })
}
pub(super) fn journal(
    sequence: u64,
    attempt: &crate::runtime::identity::AttemptId,
) -> Result<AttemptIdentitySource, ConversationStoreError> {
    let encoded_bytes = super::observation_request_summary::serialized_bytes(&attempt.as_str())?;
    Ok(AttemptIdentitySource::Journal {
        field: crate::observation::tool::JournalFieldRef {
            sequence,
            path: vec!["attempt_id".into()],
            encoded_bytes,
        },
        domain: format!("journal.{sequence}"),
    })
}

pub(super) fn response_owner(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    attempt: &crate::runtime::identity::AttemptId,
) -> Result<AttemptIdentitySource, ConversationStoreError> {
    let turn: Option<u64> = connection.query_row(
        "SELECT ordinal FROM inherited_turn_provenance INDEXED BY inherited_turn_identity_idx WHERE json_extract(payload,'$.id.conversation_id')=?1 AND json_extract(payload,'$.id.attempt_id')=?2",
        params![conversation.as_str(),attempt.as_str()], |row| row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    if let Some(ordinal) = turn {
        return inherited(connection, Kind::Turn, ordinal);
    }
    let ordinal: u64 = connection.query_row(
        "SELECT ordinal FROM inherited_response_provenance INDEXED BY inherited_response_owner_idx WHERE json_extract(payload,'$.origin.conversation_id')=?1 AND json_extract(payload,'$.origin.attempt_id')=?2 ORDER BY ordinal LIMIT 1",
        params![conversation.as_str(),attempt.as_str()], |row| row.get(0),
    ).map_err(|_|storage("original response owner unavailable"))?;
    inherited(connection, Kind::Response, ordinal)
}
pub(super) fn read(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    kind: Kind,
    ordinal: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let visible: bool = match kind {
        Kind::Turn => connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM inherited_turn_provenance p JOIN bootstrap_identity b ON b.id=1 WHERE p.ordinal=?1 AND p.ordinal<b.turn_count AND b.message_count<=?2 AND b.inherited_transcript_through<=?3)",
            params![ordinal,cut.ledger_through,cut.read.transcript], |row| row.get(0),
        ),
        Kind::Response => connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2)",
            params![ordinal,cut.ledger_through], |row| row.get(0),
        ),
    }.map_err(|error|storage(error.to_string()))?;
    if !visible {
        return Err(ConversationStoreError::InvalidReference(
            "original inherited owner outside cut".into(),
        ));
    }
    super::content::structural_field_read(
        connection,
        "inherited_attempt_identity",
        &key(kind, ordinal),
        0,
        offset,
        max_bytes,
    )
}
