//! Consumed real-statement progress gates for cancellable auxiliary start probes.
//! This module and its query tags exist only in the crate's test build.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, mpsc};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AuxiliaryStartProbe {
    Orphan,
    StartEvent,
    HistoricalCut,
    CompactionFrontier,
    ReadinessInsert,
    ObservationFields,
    ObservationJson,
}
type Key = (String, AuxiliaryStartProbe);
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
pub(super) fn activate(id: &str, probe: AuxiliaryStartProbe) -> ActiveProbe {
    ActiveProbe(ACTIVE.with(|slot| slot.replace(Some((id.to_owned(), probe)))))
}
pub(super) fn for_event(
    event: &crate::events::types::RuntimeEventEnvelope,
    probe: AuxiliaryStartProbe,
) -> ActiveProbe {
    use crate::events::types::RuntimeEvent;
    let id = match &event.event {
        RuntimeEvent::AuxiliaryCallStarted { call_id, .. } => call_id.as_str(),
        RuntimeEvent::ModelRequestStarted { request_id, .. } => request_id.as_str(),
        _ => event.event_id.as_str(),
    };
    activate(id, probe)
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
    pub(crate) fn fail_optional_start_layout_for_test(&self, request_start: bool) {
        let kind = if request_start {
            "model_request_started"
        } else {
            "auxiliary_call_started"
        };
        self.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER optional_start_layout_fault BEFORE UPDATE OF observation_json ON events WHEN json_extract(NEW.event_json,'$.event.type')='{kind}' BEGIN SELECT RAISE(FAIL,'benign optional start layout failure'); END;"
        )).unwrap();
    }
    /// Journal head and physical event/Call/Request/owner/audit row counts.
    pub(crate) fn auxiliary_integrity_counts_for_test(&self) -> (u64, [u64; 5]) {
        self.lock().unwrap().query_row(
            "SELECT next_event_sequence,(SELECT count(*) FROM events),(SELECT count(*) FROM auxiliary_calls),(SELECT count(*) FROM auxiliary_requests),(SELECT count(*) FROM request_owners),(SELECT count(*) FROM auxiliary_audit_facts) FROM rustx_store WHERE id=1",
            [], |row| Ok((row.get(0)?, [row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?])),
        ).unwrap()
    }
    pub(crate) fn hold_auxiliary_start_probe_for_test(
        request: String,
        probe: AuxiliaryStartProbe,
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
