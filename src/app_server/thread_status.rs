//! Wire projection of native Thread activity. Native owners retain all control.
use super::protocol::RpcError;
use crate::local_runtime::session::{SessionId, SessionNodeId};
use crate::local_runtime::session_runtime_manager::{
    RuntimeIncarnationId, SessionRuntimeManager,
    status::{ThreadRuntimeStatus, ThreadStatusObservation},
};
use crate::runtime::identity::ConversationId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active {
        #[serde(rename = "activeFlags")]
        active_flags: Vec<ThreadActiveFlag>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}
impl From<ThreadRuntimeStatus> for ThreadStatus {
    fn from(status: ThreadRuntimeStatus) -> Self {
        match status {
            ThreadRuntimeStatus::NotLoaded => Self::NotLoaded,
            ThreadRuntimeStatus::Idle => Self::Idle,
            ThreadRuntimeStatus::SystemError => Self::SystemError,
            ThreadRuntimeStatus::Active {
                approval,
                user_input,
            } => {
                let mut active_flags = Vec::with_capacity(2);
                if approval {
                    active_flags.push(ThreadActiveFlag::WaitingOnApproval);
                }
                if user_input {
                    active_flags.push(ThreadActiveFlag::WaitingOnUserInput);
                }
                Self::Active { active_flags }
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadStatusEntry {
    pub thread_id: ConversationId,
    pub status: ThreadStatus,
    pub revision: u64,
    pub runtime_incarnation: Option<RuntimeIncarnationId>,
}
impl From<ThreadStatusObservation> for ThreadStatusEntry {
    fn from(value: ThreadStatusObservation) -> Self {
        Self {
            thread_id: value.thread_id,
            status: value.status.into(),
            revision: value.revision,
            runtime_incarnation: value.incarnation,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub id: ConversationId,
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub name: Option<String>,
    pub archived: bool,
    pub dynamic_tools: Vec<crate::tools::host::HostToolDefinition>,
    pub status: ThreadStatus,
    pub status_revision: u64,
    pub runtime_incarnation: Option<RuntimeIncarnationId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSummary {
    pub id: ConversationId,
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub name: Option<String>,
    pub archived: bool,
    pub status: ThreadStatus,
    pub status_revision: u64,
    pub runtime_incarnation: Option<RuntimeIncarnationId>,
}
impl Thread {
    pub(crate) async fn project(
        metadata: crate::local_runtime::session::threads::Thread,
        manager: &SessionRuntimeManager,
    ) -> Result<Self, RpcError> {
        let observation = manager
            .thread_statuses(vec![metadata.id.clone()])
            .await
            .map_err(super::rpc_error::manager_error)?
            .pop()
            .expect("one addressed Thread");
        Ok(Self {
            id: metadata.id,
            session_id: metadata.session_id,
            node_id: metadata.node_id,
            name: metadata.name,
            archived: metadata.archived,
            dynamic_tools: metadata.dynamic_tools,
            status: observation.status.into(),
            status_revision: observation.revision,
            runtime_incarnation: observation.incarnation,
        })
    }
}
impl ThreadSummary {
    pub(crate) async fn project(
        metadata: Vec<crate::local_runtime::session::threads::ThreadSummary>,
        manager: &SessionRuntimeManager,
    ) -> Result<Vec<Self>, RpcError> {
        let observations = manager
            .thread_statuses(metadata.iter().map(|thread| thread.id.clone()).collect())
            .await
            .map_err(super::rpc_error::manager_error)?;
        Ok(metadata
            .into_iter()
            .zip(observations)
            .map(|(thread, observation)| Self {
                id: thread.id,
                session_id: thread.session_id,
                node_id: thread.node_id,
                name: thread.name,
                archived: thread.archived,
                status: observation.status.into(),
                status_revision: observation.revision,
                runtime_incarnation: observation.incarnation,
            })
            .collect())
    }
}

/// Session catalog surfaces carry the exact active-node status without one
/// client RPC or transcript attachment per sidebar row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    #[serde(flatten)]
    pub metadata: crate::local_runtime::session::SessionSummary,
    pub thread_id: ConversationId,
    pub thread_status: Option<ThreadStatusEntry>,
}
impl std::ops::Deref for SessionSummary {
    type Target = crate::local_runtime::session::SessionSummary;
    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}
impl SessionSummary {
    pub(crate) async fn project(
        metadata: Vec<crate::local_runtime::session::SessionSummary>,
        manager: &SessionRuntimeManager,
        sessions: &crate::local_runtime::session_controller::SessionController,
    ) -> Result<Vec<Self>, RpcError> {
        let mut ids = Vec::with_capacity(metadata.len());
        for row in &metadata {
            ids.push(
                sessions
                    .observe_node(&row.id, &row.active_node)
                    .await
                    .map_err(super::rpc_error::session_error)?
                    .conversation_id,
            );
        }
        let statuses = manager
            .thread_status_proofs(ids.clone())
            .await
            .map_err(super::rpc_error::manager_error)?;
        Ok(metadata
            .into_iter()
            .zip(ids)
            .zip(statuses)
            .map(|((metadata, thread_id), status)| Self {
                metadata,
                thread_id,
                thread_status: status.map(Into::into),
            })
            .collect())
    }
}
