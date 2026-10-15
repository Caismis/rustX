//! Optional original publication evidence on the existing operation receipt.
//! Replay never recaptures a newer native cut or creates another Journal fact.
use super::{Connection, ConversationStoreError, OptionalExtension, Transaction, params, storage};
use crate::durable::observation::{
    NativeObservationBatch, NativeObservationTransition, ObservationEventWitness,
    ObservationReadCut,
};
use crate::runtime::identity::CompactionOperationId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const MAX: usize = 65_536;
#[derive(Serialize, Deserialize)]
struct Evidence {
    ready_before: Option<u64>,
    ready_t_through: Option<u64>,
    trace_total: Option<u64>,
    source_unavailable: bool,
    read_cut: ObservationReadCut,
    journal_range: Option<(u64, u64)>,
    witness: ObservationEventWitness,
}
fn digest(native: &str, bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"rustx/compaction-publication/v1\0");
    digest.update(native.as_bytes());
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    digest.finalize().into()
}
pub(super) fn index(
    transaction: &Transaction<'_>,
    id: &CompactionOperationId,
    native: &str,
    batch: Option<&NativeObservationBatch>,
) {
    let _ = super::optional_display_layout(transaction, || {
        let batch = batch.ok_or_else(|| storage("original compaction publication unavailable"))?;
        let NativeObservationTransition::Single(witness) = &batch.transition else {
            return Err(storage("compaction publication family differs"));
        };
        let evidence = Evidence {
            ready_before: batch.ready_before,
            ready_t_through: batch.ready_t_through,
            trace_total: batch.trace_total,
            source_unavailable: batch.source_unavailable,
            read_cut: batch.read_cut.clone(),
            journal_range: batch.journal_range,
            witness: witness.clone(),
        };
        let bytes = crate::runtime_client::checkpoint::bounded_bytes(&evidence, MAX)
            .ok_or_else(|| storage("compaction publication exceeds bound"))?;
        let witness = digest(native, &bytes);
        transaction.execute("UPDATE compaction_operations SET observation_receipt=?1,observation_receipt_witness=?2 WHERE operation_id=?3",params![bytes,witness.as_slice(),id.as_str()]).map_err(|error|storage(error.to_string()))?;
        Ok(())
    });
}
pub(super) fn read(
    connection: &Connection,
    id: &CompactionOperationId,
    native: &str,
    original: &crate::events::types::RuntimeEventEnvelope,
) -> Result<Option<std::sync::Arc<NativeObservationBatch>>, ConversationStoreError> {
    type Row = (Option<Vec<u8>>, Option<Vec<u8>>);
    let row:Option<Row>=connection.query_row("SELECT CASE WHEN typeof(observation_receipt)='blob' AND length(observation_receipt)<=65536 THEN observation_receipt END,CASE WHEN typeof(observation_receipt_witness)='blob' AND length(observation_receipt_witness)=32 THEN observation_receipt_witness END FROM compaction_operations WHERE operation_id=?1",[id.as_str()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
    let Some((Some(bytes), Some(witness))) = row else {
        return Err(storage("original compaction publication unavailable"));
    };
    if witness != digest(native, &bytes) {
        return Err(storage("original compaction publication corrupt"));
    }
    let evidence: Evidence = serde_json::from_slice(&bytes)
        .map_err(|_| storage("original compaction publication corrupt"))?;
    if evidence.read_cut.read.conversation_id != original.conversation_id
        || evidence.read_cut.read.journal != original.sequence
        || evidence.witness.sequence != original.sequence
        || evidence.witness.event_kind != "compaction_completed"
        || evidence.journal_range != Some((original.sequence, original.sequence))
    {
        return Err(storage("original compaction publication identity differs"));
    }
    Ok(Some(std::sync::Arc::new(NativeObservationBatch {
        ready_before: evidence.ready_before,
        ready_t_through: evidence.ready_t_through,
        trace_total: evidence.trace_total,
        pending: None,
        source_unavailable: evidence.source_unavailable,
        failure: None,
        failure_deferred: false,
        failure_readable_end: 0,
        failure_sequence: None,
        finish_reason_deferred: false,
        finish_reason: None,
        read_cut: evidence.read_cut,
        journal_range: evidence.journal_range,
        canonical: None,
        transition: NativeObservationTransition::Single(evidence.witness),
    })))
}
