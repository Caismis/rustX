//! The Session-owned post-commit summary and ownership invalidation log.
//! It records only after Catalog visibility commits and carries no values.

use crate::runtime::identity::SessionId;
use std::collections::BTreeMap;

/// The Session-owned post-commit summary invalidation seam (Issue #386).
///
/// A display-projection publication is a Session metadata change that no
/// client can predict from canonical history: the publisher derives the line
/// on its own task *after* the canonical commit, so a live client can legally
/// read `session/summary` between the canonical commit and the projection
/// commit and cache the pre-publication `None`. This log is how the native
/// metadata owner tells observers to read the Catalog again.
///
/// It is an invalidation, not a value and not a durability claim: it says
/// "this Session's authoritative summary changed, read it again", and it is
/// recorded only after the Catalog **visibility** commit point — including
/// the post-visibility outcome that reports uncertain durability, because
/// that mutation *is* visible and losing it would strand the client on stale
/// metadata.
///
/// The log is level-triggered rather than a queue: each Session carries the
/// sequence of its latest publication, and an observer carries the sequence
/// it last delivered. Nothing is replayed durably, nothing is scheduled, and
/// an observer that stops reading costs one map entry per published Session —
/// the same order as the resident catalog document itself. Because
/// publication is once-only per Session, that map gains at most one entry per
/// Session for the life of the process.
#[derive(Debug, Default)]
pub(crate) struct SessionSummaryInvalidations {
    state: std::sync::Mutex<SummaryInvalidationState>,
}

#[derive(Debug, Default)]
struct SummaryInvalidationState {
    /// Monotonic across Sessions, so one observer cursor orders every
    /// publication this process made.
    sequence: u64,
    /// The sequence of each Session's latest recorded publication.
    published: BTreeMap<SessionId, SessionInvalidation>,
    /// Woken on every record. A `watch` is a level-triggered wake-up, not a
    /// delivery channel: observers always reread the map above, so a coalesced
    /// wake-up can never drop an invalidation.
    changed: Option<tokio::sync::watch::Sender<u64>>,
}

#[derive(Debug, Default)]
struct SessionInvalidation {
    metadata_sequence: u64,
    membership_sequence: u64,
    retirement_sequence: u64,
    /// Native catalog ownership, not the process-local delivery sequence above.
    retired_through: u64,
}

impl SessionSummaryInvalidations {
    /// Records a published display projection, after its Catalog visibility
    /// point. Never blocks on a client, a socket, or an acknowledgement: it
    /// takes one uncontended `std::sync::Mutex` and wakes parked observers.
    pub(crate) fn record(&self, session_id: &SessionId) {
        self.record_change(session_id, false);
    }

    pub(super) fn record_change(&self, session_id: &SessionId, catalog_changed: bool) {
        let mut state = self.state.lock().expect("summary invalidation log lock");
        state.sequence += 1;
        let sequence = state.sequence;
        let entry = state.published.entry(session_id.clone()).or_default();
        entry.metadata_sequence = sequence;
        if catalog_changed {
            entry.membership_sequence = sequence;
        }
        if let Some(changed) = &state.changed {
            changed.send_replace(sequence);
        }
    }

    /// A committed deletion or active-node replacement retires Host execution
    /// ownership, independently of ordinary metadata/activity invalidations.
    pub(super) fn retire_ownership(&self, id: &SessionId, retired_through: u64) {
        let mut state = self.state.lock().expect("summary invalidation log lock");
        state.sequence += 1;
        let sequence = state.sequence;
        let entry = state.published.entry(id.clone()).or_default();
        entry.retirement_sequence = sequence;
        entry.retired_through = entry.retired_through.max(retired_through);
        if let Some(changed) = &state.changed {
            changed.send_replace(sequence);
        }
    }

    pub(crate) fn next_ownership_after(&self, delivered: u64) -> Option<(u64, SessionId, u64)> {
        self.state
            .lock()
            .expect("summary invalidation log lock")
            .published
            .iter()
            .filter(|(_, entry)| entry.retirement_sequence > delivered)
            .map(|(id, entry)| (entry.retirement_sequence, id.clone(), entry.retired_through))
            .min_by_key(|(sequence, _, _)| *sequence)
    }

    /// The current frontier. A new observer starts here, so publications that
    /// predate it are not re-announced: any read that observer makes already
    /// reflects them.
    pub(crate) fn frontier(&self) -> u64 {
        self.state
            .lock()
            .expect("summary invalidation log lock")
            .sequence
    }

    /// The next invalidation after `delivered`, in publication order, or
    /// `None` when the observer is current.
    pub(crate) fn next_after(&self, delivered: u64) -> Option<(u64, SessionId, bool)> {
        let state = self.state.lock().expect("summary invalidation log lock");
        state
            .published
            .iter()
            .filter_map(|(id, entry)| {
                if entry.membership_sequence > delivered {
                    Some((entry.membership_sequence, id.clone(), true))
                } else if entry.metadata_sequence > delivered {
                    Some((entry.metadata_sequence, id.clone(), false))
                } else {
                    None
                }
            })
            .min_by_key(|(sequence, _, _)| *sequence)
    }

    /// A wake-up source for a parked observer. The value carried is the
    /// frontier; observers use it only as a change signal.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        let mut state = self.state.lock().expect("summary invalidation log lock");
        let sequence = state.sequence;
        state
            .changed
            .get_or_insert_with(|| tokio::sync::watch::channel(sequence).0)
            .subscribe()
    }

    /// How many invalidations this log has recorded. Tests assert exact
    /// counts; production only ever compares cursors.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> u64 {
        self.frontier()
    }
}
