//! One transaction for the bounded domains consumed by live checkpoint refresh.
//! The transcript establishes the native cut proof reused by response decoration.
use super::{ConversationStoreError, Transaction};
use crate::durable::observation::{CheckpointHead, ObservationReadCut};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::{response, snapshot};

pub(super) fn read(
    transaction: &Transaction<'_>,
    response_owner: &super::observation_response::Owner,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    through: u64,
    limit: usize,
) -> Result<CheckpointHead, ConversationStoreError> {
    let validated = super::observation::ValidatedCut::new(transaction, conversation, cut)?;
    let (headers, mut row_proofs) =
        super::observation::transcript_validated(&validated, limit, response_owner)?;
    let mut transcript = snapshot::transcript_header_view(headers);
    // Keep the original validation/error order: transcript/cut, response, occupancy,
    // trace total, projection head. All subsequent reads retain this snapshot.
    response::validate_decoration_cut(&transcript, through)?;
    #[cfg(test)]
    after_transcript();
    let selections = response::response_page_selections(&transcript);
    let mut facts =
        super::observation_response::read_validated(&validated, response_owner, &selections)?;
    response::apply_page_items(
        &mut transcript,
        through,
        facts.statistics,
        facts.items.iter_mut().map(|item| item.take_value()),
    )?;
    let connection = validated.connection()?;
    let occupancy = super::observation_occupancy::read(connection, through)?;
    let trace_total = super::observation_statistics::trace_total(connection, through)?;
    let projection_head = super::observation_projection::read(connection, through)?;
    let transcript_rows = transcript
        .entries
        .iter()
        .zip(&facts.items)
        .map(|(entry, response)| {
            let header = row_proofs
                .iter()
                .position(|proof| proof.cursor() == entry.cursor.get())
                .map(|index| row_proofs.swap_remove(index));
            response_owner.transcript_row(conversation, entry, header, response.owner.as_ref())
        })
        .collect::<Result<_, _>>()?;
    Ok(CheckpointHead {
        transcript,
        transcript_rows,
        occupancy,
        trace_total,
        projection_head,
    })
}

#[cfg(test)]
thread_local! {
    static READ_PROBE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(super) fn probe_after_transcript(probe: impl FnOnce() + 'static) {
    READ_PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
}
#[cfg(test)]
fn after_transcript() {
    let probe = READ_PROBE.with(|slot| slot.borrow_mut().take());
    if let Some(probe) = probe {
        probe();
    }
}
