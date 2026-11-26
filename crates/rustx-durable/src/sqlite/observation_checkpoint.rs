//! One transaction for the bounded domains consumed by live checkpoint refresh.
//! The transcript establishes the native cut proof reused by response decoration.
use super::{ConversationStoreError, Transaction};
use crate::observation::{CheckpointHead, ObservationReadCut};
use crate::response_completed as response;
use rustx_types::runtime::identity::ConversationId;

pub fn read(
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
    let mut transcript = crate::observation::transcript::transcript_header_view(headers);
    // Keep the original validation/error order: transcript/cut, response, occupancy,
    // trace total, projection head. All subsequent reads retain this snapshot.
    response::validate_decoration_cut(&transcript, through)?;
    #[cfg(any(test, feature = "test-hooks"))]
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
    let transcript_rows: Vec<_> = transcript
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
    transcript.entries = transcript_rows
        .iter()
        .map(crate::observation::transcript::PreparedTranscriptRow::native_entry)
        .collect::<Vec<_>>()
        .into();
    Ok(CheckpointHead {
        transcript,
        transcript_rows,
        occupancy,
        trace_total,
        projection_head,
    })
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static READ_PROBE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(any(test, feature = "test-hooks"))]
pub fn probe_after_transcript(probe: impl FnOnce() + 'static) {
    READ_PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
}
#[cfg(any(test, feature = "test-hooks"))]
fn after_transcript() {
    let probe = READ_PROBE.with(|slot| slot.borrow_mut().take());
    if let Some(probe) = probe {
        probe();
    }
}
