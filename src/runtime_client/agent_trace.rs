//! Child-owned Trace reads use the same durable projection as the root.
//! Registry identity chooses the store; callers never supply a child path or
//! substitute the parent's observation cursor. Reads remain valid after exit.
use super::trace::{TraceCursor, TraceProjection};
use super::{RuntimeClientError, RuntimeClientResult, host::ClientInner};
use crate::runtime::identity::AgentId;
use std::sync::Arc;

impl ClientInner {
    pub(crate) async fn agent_trace(
        self: &Arc<Self>,
        id: &AgentId,
        before: Option<TraceCursor>,
        limit: usize,
        records: Vec<TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let owner = Arc::clone(self);
        let id = id.clone();
        tokio::task::spawn_blocking(move || {
            owner.read_agent_trace(&id, |projection, agent| {
                let mut page = projection.page(before.as_ref(), limit)?;
                super::trace::repair_agent_records(&mut page.records, agent);
                page.updates = projection.refresh_agent(&records, agent)?;
                Ok(RuntimeClientResult::TracePage { page })
            })
        })
        .await
        .map_err(|_| RuntimeClientError::RuntimeFailure {
            message: "Agent Trace reader failed".into(),
        })?
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
        let owner = Arc::clone(self);
        let id = id.clone();
        tokio::task::spawn_blocking(move || {
            owner.read_agent_trace(&id, |projection, _| {
                Ok(RuntimeClientResult::TraceDetail {
                    detail: projection.detail(&record_id)?.map(Box::new),
                })
            })
        })
        .await
        .map_err(|_| RuntimeClientError::RuntimeFailure {
            message: "Agent Trace detail reader failed".into(),
        })?
    }

    fn read_agent_trace(
        &self,
        id: &AgentId,
        read: impl FnOnce(
            &TraceProjection<'_>,
            &super::snapshot::RuntimeClientAgent,
        )
            -> Result<RuntimeClientResult, crate::durable::ConversationStoreError>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let agent = self.agent_view(id)?;
        let store = self
            .agent_registry()?
            .transcript_store(&agent.activation_id)
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentTranscriptError::Unknown(subagent_id) => {
                    RuntimeClientError::UnknownSubagent { subagent_id }
                }
                crate::runtime::subagent::SubagentTranscriptError::Unavailable(message) => {
                    RuntimeClientError::RuntimeFailure { message }
                }
            })?;
        let projection = TraceProjection::new(&store).map_err(trace_error)?;
        read(&projection, &agent).map_err(trace_error)
    }
}
fn trace_error(error: crate::durable::ConversationStoreError) -> RuntimeClientError {
    match error {
        crate::durable::ConversationStoreError::InvalidReference(message) => {
            RuntimeClientError::InvalidRequest { message }
        }
        error => RuntimeClientError::RuntimeFailure {
            message: error.to_string(),
        },
    }
}
