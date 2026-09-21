//! The one-shot Session display-projection publisher (Issue #386).
//!
//! A fresh Session's persisted `display_preview` is `None` until its root
//! lineage commits its first ordinary user message. The composition paths
//! arm exactly one publisher per affected root runtime: a task that
//! subscribes to the runtime's observation stream, waits for the first
//! `ConversationObservation::Committed` carrying an ordinary
//! (`InboundKind::Message`) user message, renders it with
//! [`crate::local_runtime::session::preview_of`], and publishes the line
//! through the same once-only catalog mutation every other publication seam
//! uses.
//!
//! The projection is derived display metadata, so failure here must never
//! touch the canonical path: a failed subscription, a failed render, or a
//! failed catalog write is logged and forgotten — no replay, no resubmission,
//! no error toward the turn. Publication is one-shot: after the first
//! qualifying commit the task exits whether or not that commit rendered text
//! or the write succeeded, and later inputs never retrigger it. When the
//! observation queue closes (runtime unloaded or dropped) before any
//! qualifying commit, the task exits quietly.
//!
//! The publisher holds the Session's catalog through a *weak* handle, so an
//! armed publisher never extends native product ownership past the Session's
//! own lifetime: dropping the product releases the catalog immediately, and
//! a publisher that observes its commit while teardown is already underway
//! simply exits — the next composition's repair seam settles the projection.

use std::sync::Weak;

use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock};
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::observation::ConversationObservation;

use super::session::{SessionCatalog, SessionId};

/// Arms the one-shot display-projection publisher on an inert root runtime.
///
/// `catalog` must come from [`super::session_controller::SessionController::downgrade_catalog`]:
/// never a strong controller clone, so arming cannot keep native product
/// ownership alive after the Session itself is gone.
///
/// **Precondition, enforced by the composition callers**: the Session's root
/// conversation had no ordinary user boundary when this runtime was composed,
/// so the first qualifying commit in this runtime's lifetime *is* the
/// Session's first boundary — including a previously accepted-but-never-
/// adopted pending inbound that activation recovers. Branch-node runtimes and
/// subagent/child conversations are never armed: the projection is
/// Session-level root-lineage metadata.
///
/// The subscription is taken while the runtime is still inert (pre-
/// activation), which is exactly when
/// [`ConversationRuntime::subscribe_observations`] linearizes, so no commit
/// can slip between arming and activation. Arming never fails the caller: a
/// subscription error is logged and the Session simply keeps `None` until the
/// explicit repair seam runs.
pub(crate) fn arm_display_projection(
    catalog: Weak<tokio::sync::Mutex<SessionCatalog>>,
    session_id: SessionId,
    runtime: &ConversationRuntime,
) {
    let observations = match runtime.subscribe_observations() {
        Ok(observations) => observations,
        Err(error) => {
            tracing::warn!(
                %session_id,
                %error,
                "display projection publisher could not subscribe; the projection stays absent"
            );
            return;
        }
    };
    #[cfg(test)]
    let probe = install_probe(&session_id);
    tokio::spawn(async move {
        // One-shot: this loop returns after handling the first qualifying
        // commit (rendered or not, published or not) and when the queue
        // closes. It never outlives its purpose.
        loop {
            observations.wait().await;
            let mut subject: Option<UserMessageBlock> = None;
            for observation in observations.drain() {
                collect_first_user_boundary(&observation, &mut subject);
            }
            let Some(user) = subject else {
                if observations.is_closed() {
                    #[cfg(test)]
                    finish(&probe, false, false);
                    return;
                }
                continue;
            };
            let Some(preview) = super::session::preview_of(&user) else {
                // The Session's first ordinary user message has no renderable
                // text. That is a settled `None`, not a reason to wait for a
                // later message.
                #[cfg(test)]
                finish(&probe, false, false);
                return;
            };
            // The one test-only interleaving seam: the subject is derived and
            // the projection is still known absent, and no Catalog lock is
            // held. See `publication_test_support`.
            #[cfg(test)]
            publication_test_support::park(&session_id).await;
            let Some(catalog) = catalog.upgrade() else {
                // The Session's native ownership is already gone: the runtime
                // is being torn down with this commit still in flight. The
                // next composition's repair seam derives the projection.
                #[cfg(test)]
                finish(&probe, true, false);
                return;
            };
            match catalog
                .lock()
                .await
                .publish_display_preview(&session_id, &preview)
            {
                Ok(published) => {
                    let _ = published;
                    #[cfg(test)]
                    finish(&probe, true, published);
                }
                Err(error) => {
                    // The committed canonical input is unaffected; the row
                    // falls back to identity until the explicit repair seam.
                    tracing::warn!(
                        %session_id,
                        %error,
                        "display projection publication failed; canonical history is unaffected"
                    );
                    #[cfg(test)]
                    finish(&probe, true, false);
                }
            }
            return;
        }
    });
}

/// The first ordinary user commit inside one drained observation, unwrapping
/// the queue's publication envelopes (`JournalBatch`, `Published`) so the
/// publisher sees the same semantic stream every consumer sees.
fn collect_first_user_boundary(
    observation: &ConversationObservation,
    subject: &mut Option<UserMessageBlock>,
) {
    if subject.is_some() {
        return;
    }
    match observation {
        ConversationObservation::Committed {
            block: MessageBlock::User(user),
            ..
        } if user.kind == InboundKind::Message => *subject = Some(user.clone()),
        ConversationObservation::JournalBatch { observations, .. } => {
            for observation in observations {
                collect_first_user_boundary(observation, subject);
            }
        }
        ConversationObservation::Published { observation, .. } => {
            collect_first_user_boundary(observation, subject);
        }
        _ => {}
    }
}

/// What the armed publisher did, observable by tests without sleeps.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DisplayProjectionProbe {
    /// The publisher task finished: it handled its first qualifying commit
    /// (published, skipped, or failed) or exited on queue close.
    pub(crate) finished: bool,
    /// A publish was attempted: the first qualifying commit rendered a line.
    pub(crate) attempted: bool,
    /// The catalog accepted the projection (`false` on failure or on a lost
    /// once-only race).
    pub(crate) published: bool,
}

/// The latest probe of each armed Session, replaced on every re-arm.
#[cfg(test)]
fn probes() -> &'static std::sync::Mutex<
    std::collections::BTreeMap<SessionId, tokio::sync::watch::Sender<DisplayProjectionProbe>>,
> {
    static PROBES: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::BTreeMap<
                SessionId,
                tokio::sync::watch::Sender<DisplayProjectionProbe>,
            >,
        >,
    > = std::sync::OnceLock::new();
    PROBES.get_or_init(Default::default)
}

#[cfg(test)]
fn install_probe(session_id: &SessionId) -> tokio::sync::watch::Sender<DisplayProjectionProbe> {
    let (sender, _) = tokio::sync::watch::channel(DisplayProjectionProbe::default());
    probes()
        .lock()
        .expect("display projection probe registry lock poisoned")
        .insert(session_id.clone(), sender.clone());
    sender
}

/// Subscribes to the probe of the publisher armed for `session_id`, when one
/// was armed. Tests await `finished` (with a timeout as a deadlock guard
/// only) instead of sleeping.
#[cfg(test)]
pub(crate) fn display_projection_probe(
    session_id: &SessionId,
) -> Option<tokio::sync::watch::Receiver<DisplayProjectionProbe>> {
    probes()
        .lock()
        .expect("display projection probe registry lock poisoned")
        .get(session_id)
        .map(tokio::sync::watch::Sender::subscribe)
}

#[cfg(test)]
fn finish(
    probe: &tokio::sync::watch::Sender<DisplayProjectionProbe>,
    attempted: bool,
    published: bool,
) {
    probe.send_replace(DisplayProjectionProbe {
        finished: true,
        attempted,
        published,
    });
}

/// The one test-only interleaving seam of display-projection publication.
///
/// Both publication paths — the one-shot publisher armed on a root runtime and
/// the explicit repair seam — park here at exactly the same phase: **after**
/// the subject has been derived (the store read is done, the projection is
/// known absent) and **before** the Catalog mutex is acquired for the
/// once-only write. That is the only window in which a competing rename,
/// settings commit, deletion, or second repair can reach the Catalog first, so
/// it is the only window a race regression needs.
///
/// Parking here deliberately holds no Catalog lock: a gate that held it would
/// block the very reads whose staleness the regression is about.
///
/// Gates are keyed by Session identity and held by weak reference, so two
/// fixtures running in parallel cannot park each other's Sessions and a gate
/// whose test finished stops parking anything.
#[cfg(test)]
pub(crate) mod publication_test_support {
    use std::sync::{Arc, Mutex, Weak};

    use tokio::sync::watch;

    use super::SessionId;

    /// One parking place. `arrivals` counts publications parked *right now*,
    /// so a test can require that N racers all observed absence before any of
    /// them is allowed to write.
    #[derive(Debug)]
    pub(crate) struct PublicationGate {
        arrivals: watch::Sender<usize>,
        release: watch::Sender<bool>,
    }

    static GATES: Mutex<Vec<(SessionId, Weak<PublicationGate>)>> = Mutex::new(Vec::new());

    /// Arms the gate for one Session. The returned handle owns the gate:
    /// dropping it disarms the seam for that Session.
    pub(crate) fn arm(session_id: &SessionId) -> Arc<PublicationGate> {
        let gate = Arc::new(PublicationGate {
            arrivals: watch::channel(0).0,
            release: watch::channel(false).0,
        });
        let mut gates = GATES.lock().expect("display publication gate registry");
        gates.retain(|(id, weak)| id != session_id && weak.strong_count() > 0);
        gates.push((session_id.clone(), Arc::downgrade(&gate)));
        gate
    }

    impl PublicationGate {
        /// Resolves once `count` publications are parked at the seam together.
        pub(crate) async fn parked(&self, count: usize) {
            self.arrivals
                .subscribe()
                .wait_for(|arrivals| *arrivals >= count)
                .await
                .expect("the gate outlives its parked publications");
        }

        /// Releases every parked publication, and every later one.
        pub(crate) fn release(&self) {
            self.release.send_replace(true);
        }
    }

    /// The production-side seam. Compiled only in test builds; an unarmed
    /// Session never parks.
    pub(crate) async fn park(session_id: &SessionId) {
        let gate = GATES
            .lock()
            .expect("display publication gate registry")
            .iter()
            .find_map(|(id, weak)| (id == session_id).then(|| weak.upgrade()).flatten());
        let Some(gate) = gate else { return };
        let mut release = gate.release.subscribe();
        gate.arrivals.send_modify(|arrivals| *arrivals += 1);
        release
            .wait_for(|released| *released)
            .await
            .expect("the gate outlives its parked publications");
        gate.arrivals.send_modify(|arrivals| *arrivals -= 1);
    }
}
