//! Child-owned Trace reads use the same durable projection as the root.
//! Registry identity chooses the store; callers never supply a child path or
//! substitute the parent's observation cursor. Reads remain valid after exit.
use super::trace::TraceCursor;
use super::{RuntimeClientError, RuntimeClientResult, host::ClientInner};
use crate::runtime::identity::AgentId;
use std::sync::Arc;

impl ClientInner {
    pub(crate) async fn agent_trace_locate_tool(
        self: &Arc<Self>,
        id: &AgentId,
        locator: super::trace::TraceToolLocator,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.agent_trace_locate_via_inspection(id, locator).await
    }

    pub(crate) async fn agent_trace(
        self: &Arc<Self>,
        id: &AgentId,
        before: Option<TraceCursor>,
        limit: usize,
        records: Vec<TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.agent_trace_via_inspection(id, before, limit, records)
            .await
    }

    pub(crate) async fn agent_trace_detail(
        self: &Arc<Self>,
        id: &AgentId,
        record_id: String,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if record_id.len() > 256 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace record identity is too long".into(),
            });
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let observed = self.agent_observe_with_deadline(id, None, deadline).await?;
        let RuntimeClientResult::AgentObserve { observation } = observed else {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native child checkpoint result is unavailable".into(),
            });
        };
        let super::agent_observation::AgentObservation::Checkpoint { checkpoint, .. } = observation
        else {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native child checkpoint is required for Trace inspection".into(),
            });
        };
        let domain = super::trace_observation::detail_domain(&record_id)?;
        let page = tokio::time::timeout_at(
            deadline,
            self.agent_checkpoint_page(id, &checkpoint.read_cut, &domain, 0, 32),
        )
        .await
        .map_err(|_| RuntimeClientError::ObservationUnavailable {
            message: "total child Trace inspection deadline exhausted".into(),
        })??;
        Ok(RuntimeClientResult::DomainPage { page })
    }
}
