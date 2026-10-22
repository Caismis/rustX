//! Finite display evidence captured by the original immutable activation owner.
use super::{Connection, ConversationStoreError, OptionalExtension, Transaction, params, storage};
use crate::context::checkpoint::{CheckpointCandidate, ProjectionActivation};
use crate::durable::projection::ProjectionCheckpoint;
use crate::observation::head::RuntimeClientProjectionHeadView;
use sha2::{Digest, Sha256};
const MAX: usize = 16_384;
#[cfg(test)]
thread_local! { static READ_WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))}; }
#[cfg(test)]
pub(super) fn take_read_work() -> (usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0)))
}
fn witness(fingerprint: &str, activation: &str, bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"rustx/projection-observation/v1\0");
    digest.update(fingerprint.as_bytes());
    digest.update(activation.as_bytes());
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    digest.finalize().into()
}
fn view(
    connection: &Connection,
    candidate: &CheckpointCandidate,
    activation: &ProjectionActivation,
) -> Result<RuntimeClientProjectionHeadView, ConversationStoreError> {
    use crate::context::checkpoint::CheckpointMessage;
    let original_items = u32::try_from(
        candidate
            .items
            .iter()
            .filter(|item| matches!(item.message, CheckpointMessage::Original(_)))
            .count(),
    )
    .map_err(|_| storage("projection display count overflow"))?;
    let total = u32::try_from(candidate.items.len())
        .map_err(|_| storage("projection display count overflow"))?;
    let summary_preview = candidate.items.iter().find_map(|item| match &item.message {
        CheckpointMessage::Derived(derived)
            if derived
                .transformations
                .contains(&crate::model::projected::SemanticTransformation::Summary) =>
        {
            match &derived.message {
                crate::model::projected::ProjectedMessage::User { content, .. } => {
                    content.iter().find_map(|block| match block {
                        crate::message::types::UserContentBlock::Text(text) => Some(
                            crate::observation::trace::TraceText::bounded(&text.text, 512),
                        ),
                        _ => None,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    });
    // Local activation observes pending inputs, publication audits and terminal
    // controls as well as canonical messages. An inherited activation instead
    // uses the destination's translated canonical prefix, never the source's T.
    let transcript_anchor: u64 = if activation.destination_installation.is_none() {
        connection.query_row(
            "SELECT next_transcript_position FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
    } else {
        connection.query_row("SELECT coalesce((SELECT native_transcript FROM message_ledger WHERE position<=?1 AND native_transcript IS NOT NULL ORDER BY position DESC LIMIT 1),0)", [activation.ledger.0], |row| row.get(0))
    }
    .map_err(|error| storage(error.to_string()))?;
    Ok(RuntimeClientProjectionHeadView {
        checkpoint_id: candidate.checkpoint_id.clone(),
        activation_id: activation.activation,
        transcript_anchor: crate::observation::transcript::RuntimeClientTranscriptCursor::new(
            transcript_anchor,
        ),
        base_frontier: candidate.base_frontier,
        activation_ledger: activation.ledger,
        origin: activation.origin.clone(),
        destination_installation: activation.destination_installation.clone(),
        original_items,
        derived_items: total - original_items,
        summary_preview,
    })
}
pub(super) fn index(
    transaction: &Transaction<'_>,
    checkpoint: &ProjectionCheckpoint,
) -> Result<(), ConversationStoreError> {
    index_activation(
        transaction,
        &checkpoint.candidate,
        &checkpoint.fingerprint,
        &checkpoint.activation,
    )
}
/// Capture from the already validated native activation before its Completed J
/// witness is built. Contained layout failure affects only display availability;
/// transaction-integrity failure still aborts the original write.
pub(super) fn index_activation(
    transaction: &Transaction<'_>,
    candidate: &CheckpointCandidate,
    fingerprint: &str,
    activation: &ProjectionActivation,
) -> Result<(), ConversationStoreError> {
    let _ = super::optional_display_layout(transaction, || {
        let value = view(transaction, candidate, activation)?;
        let bytes = crate::observation::bounded::bounded_bytes(&value, MAX)
            .ok_or_else(|| storage("projection display header exceeds bound"))?;
        let native:Option<String>=transaction.query_row("SELECT CASE WHEN typeof(activation_digest)='text' AND octet_length(activation_digest)=64 THEN activation_digest END FROM projection_activations WHERE activation=?1",[activation.activation.0],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        let native =
            native.ok_or_else(|| storage("projection activation integrity unavailable"))?;
        let integrity = witness(fingerprint, &native, &bytes);
        transaction.execute("UPDATE projection_activations SET observation_header=?1,observation_header_witness=?2 WHERE activation=?3",params![bytes,integrity.as_slice(),activation.activation.0]).map_err(|error|storage(error.to_string()))?;
        Ok(())
    })?;
    Ok(())
}
pub(super) fn read(
    connection: &Connection,
    through: u64,
) -> Result<Option<RuntimeClientProjectionHeadView>, ConversationStoreError> {
    let Some(id) = super::projection::observation_head_id(connection, through)? else {
        return Ok(None);
    };
    read_id(connection, id).map(Some)
}
pub(super) fn read_id(
    connection: &Connection,
    id: u64,
) -> Result<RuntimeClientProjectionHeadView, ConversationStoreError> {
    type Row = (
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<String>,
        Option<String>,
        Option<String>,
        u64,
        u64,
        Option<String>,
    );
    let row:Row=connection.query_row("SELECT CASE WHEN typeof(a.observation_header)='blob' AND length(a.observation_header)<=16384 THEN a.observation_header END,CASE WHEN typeof(a.observation_header_witness)='blob' AND length(a.observation_header_witness)=32 THEN a.observation_header_witness END,CASE WHEN typeof(c.candidate_fingerprint)='text' AND octet_length(c.candidate_fingerprint)=64 AND c.candidate_fingerprint NOT GLOB '*[^0-9a-f]*' THEN c.candidate_fingerprint END,CASE WHEN typeof(a.activation_digest)='text' AND octet_length(a.activation_digest)=64 AND a.activation_digest NOT GLOB '*[^0-9a-f]*' THEN a.activation_digest END,CASE WHEN octet_length(a.checkpoint_id)<=1024 THEN a.checkpoint_id END,a.ledger_frontier,a.visible_journal,CASE WHEN octet_length(a.installation_id)<=1024 THEN a.installation_id END FROM projection_activations a JOIN projection_checkpoints c ON c.checkpoint_id=a.checkpoint_id WHERE a.activation=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(||storage("projection display original row missing"))?;
    let (bytes, integrity, fingerprint, native, checkpoint, ledger, journal, installation) = row;
    #[cfg(test)]
    READ_WORK.with(|work| {
        let (rows, materialized) = work.get();
        let returned = bytes.as_ref().map_or(0, Vec::len)
            + integrity.as_ref().map_or(0, Vec::len)
            + fingerprint.as_ref().map_or(0, String::len)
            + native.as_ref().map_or(0, String::len)
            + checkpoint.as_ref().map_or(0, String::len)
            + installation.as_ref().map_or(0, String::len)
            + 16;
        work.set((rows + 1, materialized + returned));
    });
    let (Some(bytes), Some(integrity), Some(fingerprint), Some(native), Some(checkpoint)) =
        (bytes, integrity, fingerprint, native, checkpoint)
    else {
        return Err(storage("projection display source unavailable"));
    };
    if integrity != witness(&fingerprint, &native, &bytes) {
        return Err(storage("projection display integrity differs"));
    }
    let value: RuntimeClientProjectionHeadView =
        serde_json::from_slice(&bytes).map_err(|_| storage("projection display header corrupt"))?;
    if value.activation_id.0 != id
        || value.checkpoint_id.as_str() != checkpoint
        || value.activation_ledger.0 != ledger
        || value
            .destination_installation
            .as_ref()
            .map(ToString::to_string)
            != installation
        || (installation.is_none()
            && (value.origin.completed_journal.0 != journal || value.origin.activation.0 != id))
    {
        return Err(storage("projection display original coordinates differ"));
    }
    Ok(value)
}

fn provenance_key(activation: u64, item: u64) -> String {
    format!("projection-provenance/{activation}/{item}")
}
pub(super) fn index_provenance(
    transaction: &Transaction<'_>,
    checkpoint: &ProjectionCheckpoint,
) -> Result<(), ConversationStoreError> {
    let _ = super::optional_display_layout(transaction, || {
        use crate::context::checkpoint::CheckpointMessage;
        use crate::observation::trace::TraceInputProvenance;
        for (ordinal, item) in checkpoint.candidate.items.iter().enumerate() {
            let key = provenance_key(checkpoint.activation.activation.0, ordinal as u64);
            match &item.message {
                CheckpointMessage::Original(reference) => {
                    let position: u64 = transaction
                        .query_row(
                            "SELECT position FROM message_ledger WHERE message_id=?1",
                            [reference.message_id.as_str()],
                            |row| row.get(0),
                        )
                        .map_err(|error| storage(error.to_string()))?;
                    let view=TraceInputProvenance::Original {source:None,conversation_id:reference.conversation_id.clone(),content_digest:reference.content_digest.clone(),locator:Some(crate::observation::content::CanonicalContentLocator::ByLedgerPosition{position}),message_id_source:None};
                    let original =
                        super::observation_request::provenance::Raw::Original(reference, &view);
                    super::observation_request::index_state(
                        transaction,
                        "projection_provenance",
                        &key,
                        &original,
                    )?;
                }
                CheckpointMessage::Derived(derived) => {
                    let original = super::observation_request::provenance::Raw::Derived(derived);
                    super::observation_request::index_state(
                        transaction,
                        "projection_provenance",
                        &key,
                        &original,
                    )?;
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}
#[allow(clippy::too_many_arguments)] // One source-owned range: identity, cut, selected item/path, and byte window.
pub(super) fn provenance_read(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    cut: &crate::durable::observation::ObservationReadCut,
    activation: u64,
    item: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    if activation == 0 || item > crate::observation::content::CONTENT_SAFE_INTEGER_MAX {
        return Err(ConversationStoreError::InvalidReference(
            "invalid projection provenance coordinate".into(),
        ));
    }
    super::observation::validate(connection, conversation, cut)?;
    super::projection::validate_activation_boundary(
        connection,
        crate::context::checkpoint::ProjectionActivationId(activation),
        crate::context::resources::LedgerFrontier(cut.ledger_through),
        crate::context::resources::JournalFrontier(cut.read.journal),
    )?;
    let value = read_id(connection, activation)?;
    if item >= u64::from(value.original_items) + u64::from(value.derived_items) {
        return Err(ConversationStoreError::InvalidReference(
            "projection item outside original cut".into(),
        ));
    }
    super::observation_request::read_from(
        connection,
        "projection_provenance",
        "state_observation_fields",
        &provenance_key(activation, item),
        path,
        offset,
        max_bytes,
    )
}
