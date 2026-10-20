//! Contribution emissions, the latest-emission head, and pending carryover.

use crate::durable::sqlite::canonical::ledger::load_message_tx;
use crate::durable::sqlite::journal::find_event_by_id;
use crate::durable::sqlite::request::snapshot::read_request_snapshot_tx;
use crate::durable::sqlite::{decode, nonnegative, seq_to_i64, sequence_from_i64, storage};
use crate::durable::{ContributionEmissionRecord, ConversationStoreError};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, contribution_emission_event_id};
use crate::message::types::{ContributionEmission, MessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::publication::PublicationAudit;
use crate::runtime::identity::{MessageId, PublicationStreamId, RequestId};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Transaction, params};
use std::collections::BTreeSet;

pub(in crate::durable::sqlite) fn validate_contribution_emission_shape(
    emission: &ContributionEmission,
) -> Result<(), ConversationStoreError> {
    emission
        .validate()
        .map_err(ConversationStoreError::InvalidReference)
}

/// Validates the exact status message/emission binding before a start chooses
/// its fresh or idempotent path. This is the construction boundary that keeps
/// a free-standing emission vector from entering durable storage.
pub(in crate::durable::sqlite) fn validate_contribution_start(
    snapshot: &RequestSnapshot,
    context: &[MessageBlock],
) -> Result<(), ConversationStoreError> {
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for contribution in &snapshot.contributions {
        if !ids.insert(&contribution.message_id) {
            return Err(ConversationStoreError::InvalidReference(
                "duplicate accepted contribution identity".to_owned(),
            ));
        }
        if !snapshot
            .context_generation
            .contributors
            .iter()
            .any(|generation| generation.identity == contribution.producer)
        {
            return Err(ConversationStoreError::InvalidReference(
                "contribution producer is absent from the accepted generation".to_owned(),
            ));
        }
        for emission in &contribution.emissions {
            validate_contribution_emission_shape(emission)?;
            if !keys.insert((&contribution.producer, &emission.key)) {
                return Err(ConversationStoreError::InvalidReference(
                    "duplicate producer-scoped contribution key".to_owned(),
                ));
            }
        }
        if snapshot.identity.retry_number == 0 {
            let message = context
                .iter()
                .find(|message| message.id() == &contribution.message_id)
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "accepted contribution has no canonical context".to_owned(),
                    )
                })?;
            contribution.validate_message(message)?;
        } else if context
            .iter()
            .any(|message| message.id() == &contribution.message_id)
        {
            return Err(ConversationStoreError::InvalidReference(
                "retry must not recommit an accepted contribution".to_owned(),
            ));
        }
    }
    if snapshot.identity.retry_number == 0
        && context.iter().any(|message| !ids.contains(message.id()))
    {
        return Err(ConversationStoreError::InvalidReference(
            "canonical context has no accepted contribution record".to_owned(),
        ));
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_frozen_contributions_tx(
    transaction: &Transaction<'_>,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let visible = |id: &MessageId| {
        snapshot.input.iter().any(|item| match item {
            crate::model::projected::FrozenModelInput::Original(reference) => {
                &reference.message_id == id
            }
            crate::model::projected::FrozenModelInput::Derived(derived) => {
                derived
                    .sources
                    .iter()
                    .any(|source| &source.message_id == id)
                    && !derived.transformations.is_empty()
                    && derived.transformations.iter().all(|t| {
                        matches!(
                            t,
                            crate::model::projected::SemanticTransformation::ImageDowngrade
                                | crate::model::projected::SemanticTransformation::UploadRendering
                        )
                    })
            }
            crate::model::projected::FrozenModelInput::RequestOnly(_) => false,
        })
    };
    for contribution in &snapshot.contributions {
        contribution.validate_message(&load_message_tx(transaction, &contribution.message_id)?)?;
        if !visible(&contribution.message_id) {
            return Err(ConversationStoreError::InvalidReference(
                "frozen contribution is absent from exact typed request".to_owned(),
            ));
        }
        // The original snapshot is immutable even if a newer head for this
        // producer/key exists. A retry verifies its own logical-step receipt.
        let mut origin = snapshot.identity.clone();
        origin.retry_number = 0;
        let (original, started, _) = read_request_snapshot_tx(transaction, &origin.request_id())?
            .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "retry has no original accepted contribution state".to_owned(),
            )
        })?;
        if started.is_none() || !original.contributions.contains(contribution) {
            return Err(ConversationStoreError::InvalidReference(
                "retry changed its frozen accepted contribution".to_owned(),
            ));
        }
        for emission in &contribution.emissions {
            let event_id = contribution_emission_event_id(
                &origin.request_id(),
                &contribution.producer,
                emission,
            );
            let event = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "frozen contribution has no committed receipt".to_owned(),
                )
            })?;
            if !matches!(&event.event, RuntimeEvent::ContextContributionEmitted { producer, message_id, emission: committed, .. } if producer == &contribution.producer && message_id == &contribution.message_id && committed == emission)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "frozen contribution receipt disagrees".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_contribution_start_tx(
    transaction: &Transaction<'_>,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
    started: &RuntimeEventEnvelope,
) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
    validate_contribution_start(snapshot, context)?;
    if snapshot.identity.retry_number != 0 {
        verify_frozen_contributions_tx(transaction, snapshot)?;
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    let mut sequence = started.sequence;
    for contribution in &snapshot.contributions {
        for emission in &contribution.emissions {
            let event_id = contribution_emission_event_id(
                &snapshot.request_id,
                &contribution.producer,
                emission,
            );
            let event = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed contribution lacks receipt".to_owned(),
                )
            })?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| storage("contribution sequence overflow"))?;
            let RuntimeEvent::ContextContributionEmitted {
                producer,
                request_id,
                message_id,
                emission: committed,
                logical_step_origin,
            } = &event.event
            else {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution receipt has wrong event kind".to_owned(),
                ));
            };
            if producer != &contribution.producer
                || request_id != &snapshot.request_id
                || message_id != &contribution.message_id
                || committed != emission
                || event.sequence != sequence
                || event.conversation_id != started.conversation_id
                || event.attempt_id != started.attempt_id
                || event.turn_id != started.turn_id
                || event.timestamp != started.timestamp
            {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution receipt disagrees with startup".to_owned(),
                ));
            }
            let head = read_contribution_head_tx(transaction, producer, &emission.key)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "contribution receipt has no durable head".to_owned(),
                    )
                })?;
            if head.event_sequence < sequence
                || (head.event_sequence == sequence
                    && (head.fingerprint != emission.fingerprint
                        || head.canonical_message_id != *message_id
                        || head.request_id != *request_id
                        || head.logical_step_origin != *logical_step_origin
                        || head.emitted_at != event.timestamp))
            {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution head disagrees with receipt".to_owned(),
                ));
            }
            events.push(event);
        }
    }
    Ok(events)
}

pub(in crate::durable::sqlite) fn read_logical_step_progress_tx(
    transaction: &Transaction<'_>,
) -> Result<u64, ConversationStoreError> {
    let value: i64 = transaction
        .query_row(
            "SELECT logical_step_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Todo progress transaction lookup: {error}")))?;
    nonnegative(value, "Todo progress sequence")
}

pub(in crate::durable::sqlite) struct ContributionHeadRow {
    pub(in crate::durable::sqlite) producer: String,
    pub(in crate::durable::sqlite) key: String,
    pub(in crate::durable::sqlite) fingerprint: String,
    pub(in crate::durable::sqlite) emitted_at: String,
    pub(in crate::durable::sqlite) request_id: String,
    pub(in crate::durable::sqlite) message_id: String,
    pub(in crate::durable::sqlite) logical_step_origin: i64,
    pub(in crate::durable::sqlite) event_sequence: i64,
}

pub(in crate::durable::sqlite) fn read_contribution_head_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ContributionHeadRow> {
    Ok(ContributionHeadRow {
        producer: row.get(0)?,
        key: row.get(1)?,
        fingerprint: row.get(2)?,
        emitted_at: row.get(3)?,
        request_id: row.get(4)?,
        message_id: row.get(5)?,
        logical_step_origin: row.get(6)?,
        event_sequence: row.get(7)?,
    })
}

pub(in crate::durable::sqlite) fn head_record_from_row(
    row: ContributionHeadRow,
) -> Result<ContributionEmissionRecord, ConversationStoreError> {
    let emitted_at = DateTime::parse_from_rfc3339(&row.emitted_at)
        .map_err(|error| {
            ConversationStoreError::InvalidReference(format!(
                "invalid Agent Status head timestamp {:?}: {error}",
                row.emitted_at
            ))
        })?
        .with_timezone(&Utc);
    Ok(ContributionEmissionRecord {
        producer: decode(&row.producer, "contribution producer")?,
        key: row.key,
        fingerprint: row.fingerprint,
        emitted_at,
        request_id: RequestId::new(row.request_id),
        canonical_message_id: MessageId::new(row.message_id),
        logical_step_origin: nonnegative(
            row.logical_step_origin,
            "Agent Status logical-step origin",
        )?,
        event_sequence: sequence_from_i64(row.event_sequence)?,
    })
}

pub(in crate::durable::sqlite) fn read_contribution_head(
    connection: &Connection,
    producer: &crate::runtime::identity::ContextContributorIdentity,
    key: &str,
) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence
             FROM contribution_emission_heads
             WHERE producer=?1 AND semantic_key=?2",
            params![producer.durable_key(), key],
            read_contribution_head_row,
        )
        .optional()
        .map_err(|error| storage(format!("Agent Status head lookup: {error}")))?
        .map(head_record_from_row)
        .transpose()
}

pub(in crate::durable::sqlite) fn read_contribution_head_tx(
    transaction: &Transaction<'_>,
    producer: &crate::runtime::identity::ContextContributorIdentity,
    key: &str,
) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence
             FROM contribution_emission_heads
             WHERE producer=?1 AND semantic_key=?2",
            params![producer.durable_key(), key],
            read_contribution_head_row,
        )
        .optional()
        .map_err(|error| storage(format!("Agent Status head lookup: {error}")))?
        .map(head_record_from_row)
        .transpose()
}

pub(in crate::durable::sqlite) fn upsert_contribution_head_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::ContextContributionEmitted {
        producer,
        request_id,
        message_id,
        emission,
        logical_step_origin,
    } = &event.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "latest-emission head update requires an contribution emission fact".to_owned(),
        ));
    };
    let existing = read_contribution_head_tx(transaction, producer, &emission.key)?;
    if let Some(existing) = existing {
        if existing.event_sequence > event.sequence {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Agent Status head for {}:{} is newer than event {}",
                producer.durable_key(),
                emission.key,
                event.sequence
            )));
        }
        if existing.event_sequence == event.sequence {
            if existing.fingerprint == emission.fingerprint
                && existing.request_id == *request_id
                && existing.canonical_message_id == *message_id
                && existing.logical_step_origin == *logical_step_origin
                && existing.emitted_at == event.timestamp
            {
                return Ok(());
            }
            return Err(ConversationStoreError::InvalidReference(
                "Agent Status latest-emission head conflicts at one event sequence".to_owned(),
            ));
        }
        if existing.logical_step_origin > *logical_step_origin {
            return Err(ConversationStoreError::InvalidReference(
                "Agent Status latest-emission head regresses the committed logical-step origin"
                    .to_owned(),
            ));
        }
        transaction
            .execute(
                "UPDATE contribution_emission_heads
                 SET fingerprint=?1,emitted_at=?2,request_id=?3,message_id=?4,logical_step_origin=?5,event_sequence=?6
                 WHERE producer=?7 AND semantic_key=?8",
                params![
                    emission.fingerprint,
                    event.timestamp.to_rfc3339(),
                    request_id.as_str(),
                    message_id.as_str(),
                    seq_to_i64(*logical_step_origin)?,
                    seq_to_i64(event.sequence)?,
                    producer.durable_key(),
                    emission.key,
                ],
            )
            .map_err(|error| storage(format!("update Agent Status head: {error}")))?;
    } else {
        transaction
            .execute(
                "INSERT INTO contribution_emission_heads
                 (producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    producer.durable_key(),
                    emission.key,
                    emission.fingerprint,
                    event.timestamp.to_rfc3339(),
                    request_id.as_str(),
                    message_id.as_str(),
                    seq_to_i64(*logical_step_origin)?,
                    seq_to_i64(event.sequence)?,
                ],
            )
            .map_err(|error| storage(format!("insert Agent Status head: {error}")))?;
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn validate_carryover_snapshot(
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    match (
        &snapshot.unresolved_output_carryover_source,
        &snapshot.unresolved_output_carryover,
        &snapshot.unresolved_output_carryover_anchor,
    ) {
        (None, None, None) => Ok(()),
        (Some(source), representation, Some(_)) => {
            if let Some(representation) = representation
                && representation.source_stream_id != *source
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request snapshot {} carryover representation disagrees with its source",
                    snapshot.request_id
                )));
            }
            Ok(())
        }
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} has an incomplete frozen carryover tuple",
            snapshot.request_id
        ))),
    }
}

pub(in crate::durable::sqlite) fn consume_pending_carryover_tx(
    transaction: &Transaction<'_>,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let Some(source) = snapshot.unresolved_output_carryover_source.as_ref() else {
        return Ok(());
    };
    if snapshot.identity.retry_number != 0 {
        return Ok(());
    }
    let pending: Option<String> = transaction
        .query_row(
            "SELECT pending_unresolved_output_stream_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("pending carryover consume probe: {error}")))?;
    if pending.as_deref() != Some(source.as_str()) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} attempted to consume pending carryover {}, but the durable pending source is {:?}",
            snapshot.request_id, source, pending
        )));
    }
    transaction
        .execute(
            "UPDATE rustx_store SET pending_unresolved_output_stream_id=NULL WHERE id=1",
            [],
        )
        .map_err(|error| storage(format!("consume pending carryover: {error}")))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn validate_pending_carryover_source_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    source: Option<&PublicationStreamId>,
) -> Result<(), ConversationStoreError> {
    let Some(source) = source else {
        return Ok(());
    };
    let Some(attempt_id) = event.attempt_id.as_ref() else {
        return Err(ConversationStoreError::InvalidReference(
            "pending carryover requires an attempt-scoped terminal event".to_owned(),
        ));
    };
    let Some(audit_json): Option<String> = transaction
        .query_row(
            "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
            [source.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("pending carryover audit probe: {error}")))?
    else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} has no durable Publication Audit"
        )));
    };
    let audit: PublicationAudit = decode(&audit_json, "pending carryover audit")?;
    if audit.stream_id != *source || audit.attempt_id != *attempt_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} does not belong to terminal attempt {attempt_id}"
        )));
    }
    if !matches!(
        audit.kind,
        crate::publication::PublicationAuditKind::Incomplete
            | crate::publication::PublicationAuditKind::Unaccepted
    ) || crate::publication::render_unresolved_output_carryover(&audit).is_none()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} is not a meaningful unresolved Publication Audit"
        )));
    }
    Ok(())
}
