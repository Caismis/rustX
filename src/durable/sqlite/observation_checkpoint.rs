//! One transaction for the bounded domains consumed by live checkpoint refresh.
//! The transcript establishes the native cut proof reused by response decoration.
use super::{Connection, ConversationStoreError};
use crate::durable::observation::{CheckpointHead, ObservationReadCut};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::{response, snapshot};

pub(super) fn read(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    through: u64,
    limit: usize,
) -> Result<CheckpointHead, ConversationStoreError> {
    let mut transcript = snapshot::transcript_header_view(super::observation::transcript(
        connection,
        conversation,
        cut,
        None,
        limit,
    )?);
    // Keep the original validation/error order: transcript/cut, response, occupancy,
    // trace total, projection head. All subsequent reads retain this snapshot.
    response::validate_decoration_cut(&transcript, through)?;
    #[cfg(test)]
    after_transcript();
    let selections = response::response_page_selections(&transcript);
    let facts =
        super::observation_response::read_validated(connection, conversation, cut, &selections)?;
    response::apply_page_facts(&mut transcript, through, facts)?;
    let occupancy = super::observation_occupancy::read(connection, through)?;
    let trace_total = super::observation_statistics::trace_total(connection, through)?;
    let projection_head = super::observation_projection::read(connection, through)?;
    Ok(CheckpointHead {
        transcript,
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
