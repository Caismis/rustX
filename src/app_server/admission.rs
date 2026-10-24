//! Finite connection-local association, installed before polling semantic work.
//! An association lives through response disposition, not just future completion.
use super::protocol::{AttachmentTarget, ErrorData, Method, RequestId};
use crate::runtime::identity::AgentId;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestLane {
    Observation,
    Ordinary,
    Control,
}

impl RequestLane {
    pub(crate) fn for_method(method: &Method) -> Self {
        match method {
            Method::AgentObserve { .. } => Self::Observation,
            Method::AuxiliaryCancel { .. }
            | Method::AgentObserveCancel { .. }
            | Method::DeliveryCancel { .. }
            | Method::TurnCancel { .. }
            | Method::AgentInterrupt { .. }
            | Method::JobCancel { .. }
            | Method::SessionDetach { .. }
            | Method::SessionSubmissionIntent { .. }
            | Method::InteractionCancel { .. } => Self::Control,
            _ => Self::Ordinary,
        }
    }

    fn capacity(self) -> usize {
        match self {
            Self::Observation => 8,
            Self::Ordinary => 6,
            Self::Control => 2,
        }
    }

    pub(crate) fn host_capacity_per_connection(self) -> usize {
        let total = super::transport::IN_FLIGHT_REQUESTS;
        if self == Self::Control {
            total
        } else {
            total - Self::Control.capacity()
        }
    }
}

struct Entry {
    owner: Arc<()>,
    lane: RequestLane,
    observation: Option<ObservationAdmission>,
    submission: Option<crate::runtime::auxiliary::submission::SubmissionPermit>,
}

struct ObservationAdmission {
    target: AttachmentTarget,
    agent_id: AgentId,
    control: ObservationControl,
}
#[derive(Clone)]
pub(super) struct ObservationControl {
    pub(super) cancel: CancellationToken,
    pub(super) deadline: tokio::time::Instant,
}

#[derive(Default)]
pub(super) struct Associations(Mutex<BTreeMap<RequestId, Entry>>);

#[derive(Clone)]
pub(crate) struct Association(Arc<Reservation>);

struct Reservation {
    table: Arc<Associations>,
    id: RequestId,
    owner: Arc<()>,
}

impl Associations {
    pub(super) fn admit(
        self: &Arc<Self>,
        id: &RequestId,
        method: &Method,
    ) -> Result<Association, ErrorData> {
        let mut entries = self.0.lock().expect("request associations");
        if entries.contains_key(id) {
            return Err(ErrorData::DuplicateRequestId);
        }
        let lane = RequestLane::for_method(method);
        let observation = if let Method::AgentObserve {
            target, agent_id, ..
        } = method
        {
            if entries.values().any(|entry| {
                entry.observation.as_ref().is_some_and(|admission| {
                    admission.target == *target && admission.agent_id == *agent_id
                })
            }) {
                return Err(ErrorData::ObservationInUse);
            }
            Some(ObservationAdmission {
                target: target.clone(),
                agent_id: agent_id.clone(),
                control: ObservationControl {
                    cancel: CancellationToken::new(),
                    deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(15),
                },
            })
        } else {
            None
        };
        let capacity = lane.capacity();
        if entries.values().filter(|entry| entry.lane == lane).count() >= capacity {
            return Err(if lane == RequestLane::Observation {
                ErrorData::ObservationCapacityExceeded
            } else {
                ErrorData::RequestCapacity
            });
        }
        match method {
            Method::AuxiliaryClassify { target, submission } => {
                submission
                    .fingerprint(&target.conversation_id)
                    .map_err(|_| ErrorData::InvalidParams)?;
            }
            Method::AuxiliaryBtw { target, submission } => {
                submission
                    .fingerprint(&target.conversation_id)
                    .map_err(|_| ErrorData::InvalidParams)?;
            }
            _ => {}
        }
        let owner = Arc::new(());
        entries.insert(
            id.clone(),
            Entry {
                owner: owner.clone(),
                lane,
                observation,
                submission: match method {
                    Method::AuxiliaryClassify { target, submission } => Some(
                        crate::runtime::auxiliary::submission::SubmissionPermit::new_for(
                            target.conversation_id.clone(),
                            submission.correlation.clone(),
                        )
                        .map_err(|_| ErrorData::RequestCapacity)?,
                    ),
                    Method::AuxiliaryBtw { target, submission } => Some(
                        crate::runtime::auxiliary::submission::SubmissionPermit::new_for(
                            target.conversation_id.clone(),
                            submission.correlation.clone(),
                        )
                        .map_err(|_| ErrorData::RequestCapacity)?,
                    ),
                    _ => None,
                },
            },
        );
        Ok(Association(Arc::new(Reservation {
            table: self.clone(),
            id: id.clone(),
            owner,
        })))
    }

    pub(super) fn cancel_observation(&self, id: &RequestId) -> bool {
        let entries = self.0.lock().expect("request associations");
        let Some(admission) = entries.get(id).and_then(|entry| entry.observation.as_ref()) else {
            return false;
        };
        admission.control.cancel.cancel();
        true
    }

    pub(super) fn close(&self) {
        for entry in self.0.lock().expect("request associations").values() {
            if let Some(permit) = &entry.submission {
                permit.close();
            }
            if let Some(admission) = &entry.observation {
                admission.control.cancel.cancel();
            }
        }
    }
}

impl Association {
    pub(super) fn submission_permit(
        &self,
    ) -> Option<crate::runtime::auxiliary::submission::SubmissionPermit> {
        self.0
            .table
            .0
            .lock()
            .expect("request associations")
            .get(&self.0.id)
            .and_then(|entry| entry.submission.clone())
    }
    pub(super) fn observation_control(&self) -> Option<ObservationControl> {
        self.0
            .table
            .0
            .lock()
            .expect("request associations")
            .get(&self.0.id)
            .filter(|entry| Arc::ptr_eq(&entry.owner, &self.0.owner))
            .and_then(|entry| {
                entry
                    .observation
                    .as_ref()
                    .map(|admission| admission.control.clone())
            })
    }

    #[cfg(test)]
    pub(super) fn observation_token(&self) -> Option<CancellationToken> {
        self.observation_control().map(|control| control.cancel)
    }

    pub(super) fn finish_observation(&self) {
        let mut entries = self.0.table.0.lock().expect("request associations");
        if let Some(entry) = entries
            .get_mut(&self.0.id)
            .filter(|entry| Arc::ptr_eq(&entry.owner, &self.0.owner))
        {
            entry.observation = None;
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut entries = self.table.0.lock().expect("request associations");
        if let Some(permit) = entries
            .get(&self.id)
            .filter(|entry| Arc::ptr_eq(&entry.owner, &self.owner))
            .and_then(|entry| entry.submission.as_ref())
        {
            permit.close();
        }
        if entries
            .get(&self.id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.owner, &self.owner))
            && let Some(admission) = entries.remove(&self.id).and_then(|entry| entry.observation)
        {
            admission.control.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation() -> Method {
        Method::AgentObserve {
            target: AttachmentTarget {
                session_id: crate::local_runtime::session::SessionId::new(
                    "ses_00000000-0000-7000-8000-000000000467",
                ),
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_00000000-0000-7000-8000-000000000467",
                ),
                runtime_incarnation: serde_json::from_str("1").unwrap(),
                attachment_id: crate::runtime_client::types::AttachmentId::new("attachment"),
            },
            agent_id: AgentId::new("agent"),
            after: None,
        }
    }
    #[test]
    fn t20_admission_cancellation_deduplication_and_exact_cleanup() {
        let table = Arc::new(Associations::default());
        let id = RequestId::Integer(1);
        let owner = table.admit(&id, &observation()).unwrap();
        let token = owner.observation_token().unwrap();
        assert!(table.cancel_observation(&id));
        assert!(
            token.is_cancelled(),
            "cancellation is installed without polling work"
        );
        assert!(matches!(
            table.admit(&id, &Method::ServerInfo {}),
            Err(ErrorData::DuplicateRequestId)
        ));
        assert!(matches!(
            table.admit(&RequestId::Integer(2), &observation()),
            Err(ErrorData::ObservationInUse)
        ));
        // Model delayed cleanup after an exact owner has retired and its ID reused.
        table.0.lock().unwrap().remove(&id);
        let successor = table.admit(&id, &Method::ServerInfo {}).unwrap();
        drop(owner);
        assert!(table.0.lock().unwrap().contains_key(&id));
        drop(successor);
        assert!(table.0.lock().unwrap().is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn t20_observe_absolute_deadline_begins_at_admission_before_first_poll() {
        let table = Arc::new(Associations::default());
        let admitted = tokio::time::Instant::now();
        let owner = table.admit(&RequestId::Integer(1), &observation()).unwrap();
        let control = owner.observation_control().unwrap();
        assert_eq!(
            control.deadline,
            admitted + std::time::Duration::from_secs(15)
        );
        tokio::time::advance(std::time::Duration::from_secs(14)).await;
        assert_eq!(
            owner.observation_control().unwrap().deadline,
            control.deadline
        );
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(
            tokio::time::Instant::now() >= control.deadline,
            "scheduling cannot reset the complete Observe budget"
        );
    }

    #[test]
    fn t20_capacity_lanes_preserve_control_and_connection_local_identity() {
        let table = Arc::new(Associations::default());
        let mut owners = Vec::new();
        for id in 0..6 {
            owners.push(
                table
                    .admit(&RequestId::Integer(id), &Method::ServerInfo {})
                    .unwrap(),
            );
        }
        assert!(matches!(
            table.admit(&RequestId::Integer(7), &Method::ServerInfo {}),
            Err(ErrorData::RequestCapacity)
        ));
        let control = table
            .admit(
                &RequestId::Integer(8),
                &Method::AgentObserveCancel {
                    request_id: RequestId::Integer(0),
                },
            )
            .unwrap();
        let submission = table
            .admit(
                &RequestId::Integer(9),
                &Method::SessionSubmissionIntent {
                    session_id: crate::local_runtime::session::SessionId::new(
                        "ses_00000000-0000-7000-8000-000000000467",
                    ),
                    node_id: crate::local_runtime::session::SessionNodeId::new(
                        "node_00000000-0000-7000-8000-000000000467",
                    ),
                },
            )
            .unwrap();
        let other = Arc::new(Associations::default());
        let independent = other
            .admit(&RequestId::Integer(0), &Method::ServerInfo {})
            .unwrap();
        drop((owners, control, submission, independent));
        assert!(table.0.lock().unwrap().is_empty());
    }

    #[test]
    fn t20_physical_work_keeps_reservation_after_logical_waiter_is_dropped() {
        let table = Arc::new(Associations::default());
        let id = RequestId::Integer(1);
        let logical = table.admit(&id, &observation()).unwrap();
        let physical = logical.clone();
        let token = logical.observation_token().unwrap();
        assert!(table.cancel_observation(&id));
        drop(logical);
        assert!(token.is_cancelled());
        assert!(matches!(
            table.admit(&id, &Method::ServerInfo {}),
            Err(ErrorData::DuplicateRequestId)
        ));
        assert!(matches!(
            table.admit(&RequestId::Integer(2), &observation()),
            Err(ErrorData::ObservationInUse)
        ));
        drop(physical); // Actual owner proves termination, releasing capacity and occupancy.
        assert!(table.admit(&id, &observation()).is_ok());
    }
}
