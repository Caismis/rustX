//! Consumed real-statement progress gates for cancellable `RequestStart` probes.
//! This module and both query tags exist only in the crate's test build.
use crate::runtime::identity::RequestId;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, mpsc};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RequestJournalProbe {
    Orphan,
    StartEvent,
}
type Key = (RequestId, RequestJournalProbe);
struct Gate {
    entered: oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}
static GATES: OnceLock<Mutex<BTreeMap<Key, Gate>>> = OnceLock::new();
thread_local! { static ACTIVE: RefCell<Option<Key>> = const { RefCell::new(None) }; }
fn gates() -> &'static Mutex<BTreeMap<Key, Gate>> {
    GATES.get_or_init(Mutex::default)
}

pub(super) fn pending() -> bool {
    !gates().lock().unwrap().is_empty()
}
pub(super) struct ActiveProbe(Option<Key>);
pub(super) fn activate(id: &RequestId, probe: RequestJournalProbe) -> ActiveProbe {
    ActiveProbe(ACTIVE.with(|slot| slot.replace(Some((id.clone(), probe)))))
}
impl Drop for ActiveProbe {
    fn drop(&mut self) {
        ACTIVE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}
/// Return true only when this exact real statement consumed its gate. The
/// caller still obtains cancellation exclusively from the native writer scope.
pub(super) fn progress() -> bool {
    let Some(key) = ACTIVE.with(|slot| slot.borrow().clone()) else {
        return false;
    };
    let gate = gates().lock().unwrap().remove(&key);
    let Some(gate) = gate else {
        return false;
    };
    let _ = gate.entered.send(());
    let _ = gate
        .release
        .recv_timeout(std::time::Duration::from_secs(10));
    true
}

pub(crate) struct ProbeRelease {
    key: Key,
    release: Option<mpsc::Sender<()>>,
}
impl Drop for ProbeRelease {
    fn drop(&mut self) {
        gates().lock().unwrap().remove(&self.key);
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl super::SqliteConversationStore {
    pub(crate) fn hold_request_journal_probe_for_test(
        request: RequestId,
        probe: RequestJournalProbe,
    ) -> (oneshot::Receiver<()>, ProbeRelease) {
        let (entered, observed) = oneshot::channel();
        let (release, held) = mpsc::channel();
        let key = (request, probe);
        assert!(
            gates()
                .lock()
                .unwrap()
                .insert(
                    key.clone(),
                    Gate {
                        entered,
                        release: held
                    }
                )
                .is_none()
        );
        (
            observed,
            ProbeRelease {
                key,
                release: Some(release),
            },
        )
    }
}
