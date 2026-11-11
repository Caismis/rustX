//! Debug-only oracle for the persisted recovery frontier.
//!
//! Every debug reconstruction recomputes the Journal fold from the first fact
//! and asserts that it equals the persisted frontier plus its folded tail.
//! Release builds never read retained history for this.

use std::cell::Cell;

use super::{RecoveryEvidence, RecoveryFrontier};
use crate::durable::ConversationStore;
use crate::runtime::identity::ConversationId;

thread_local! { static ENABLED: Cell<bool> = const { Cell::new(true) }; }

pub(super) fn assert_matches_full_fold(
    store: &dyn ConversationStore,
    conversation_id: &ConversationId,
    frontier: &RecoveryFrontier,
) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    let full = RecoveryEvidence::fold_journal(store, conversation_id, RecoveryFrontier::default())
        .unwrap_or_else(|error| {
            panic!("recovery frontier oracle cannot fold the whole Journal: {error}")
        });
    assert_eq!(
        &full, frontier,
        "the persisted recovery frontier disagrees with the full Journal fold"
    );
}
