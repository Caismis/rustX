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

/// Suspends the full-fold oracle on this thread while the guard lives, for
/// tests that measure the bounded work of the release open path itself.
#[cfg(test)]
pub(crate) fn suspend_frontier_oracle() -> impl Drop {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ENABLED.with(|enabled| enabled.set(self.0));
        }
    }
    Restore(ENABLED.with(|enabled| enabled.replace(false)))
}

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
