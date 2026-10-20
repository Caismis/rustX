//! Agent registry reads and Agent control forwarded to the native registry.
use super::control::agent_control_error;
use super::history::read_conversation_window;
use super::history::validate_transcript_page_limit;
use super::{Arc, ClientInner, RuntimeClientError, RuntimeClientResult};
use crate::durable::JournalStore;
use crate::durable::ObservationStore;

impl ClientInner {
    pub(in crate::runtime_client) fn agent_registry(
        &self,
    ) -> Result<&crate::runtime::subagent::SubagentRegistry, RuntimeClientError> {
        self.runtime
            .as_ref()
            .and_then(|r| r.subagent_registry())
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "Agent registry unavailable".into(),
            })
    }

    pub(in crate::runtime_client) fn agent_view(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<super::super::snapshot::RuntimeClientAgent, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let (agent, activation) = registry.agent_snapshot_with_activation(id).ok_or_else(|| {
            RuntimeClientError::UnknownAgent {
                agent_id: id.clone(),
            }
        })?;
        Ok(super::super::projection::views::agent_view(
            &agent,
            &activation,
        ))
    }

    pub(crate) async fn agent_statistics(
        self: &Arc<Self>,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let owner = Arc::clone(self);
        let id = id.clone();
        tokio::task::spawn_blocking(move || owner.read_agent_statistics(&id))
            .await
            .map_err(|_| RuntimeClientError::RuntimeFailure {
                message: "Agent statistics reader failed".to_owned(),
            })?
    }

    pub(super) fn read_agent_statistics(
        &self,
        id: &crate::runtime::identity::AgentId,
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
        let failed = RuntimeClientError::observation_store_error;
        let through = store.presentation_frontier().map_err(failed)?;
        let metrics = crate::observation::statistics::read(&store, through, || {
            self.agent_registry()
                .ok()?
                .execution_started_at(&agent.activation_id)
        })
        .map_err(failed)?;
        Ok(RuntimeClientResult::AgentStatistics { metrics })
    }

    pub(crate) fn agent_status(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page(&format!("agents.{id}"), 0, 32)
    }

    pub(crate) fn agent_list(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("agents", 0, 32)
    }

    pub(crate) fn check_agent_message_target(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<(), RuntimeClientError> {
        self.agent_registry()?
            .check_message_target(id)
            .map_err(|error| agent_control_error(id, error))
    }

    pub(crate) async fn agent_send_message(
        &self,
        id: &crate::runtime::identity::AgentId,
        message: String,
        attachments: Vec<crate::message::content::UploadedFileRef>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let accepted = self
            .agent_registry()?
            .send_message_with_attachments(
                id,
                &message,
                &attachments,
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .map_err(|error| agent_control_error(id, error))?;
        Ok(RuntimeClientResult::AgentMessage { accepted })
    }

    pub(crate) fn agent_interrupt_intent(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let activation_id = self
            .agent_registry()?
            .interrupt_agent_intent(id)
            .map_err(|error| agent_control_error(id, error))?;
        Ok(RuntimeClientResult::AgentInterruptionAccepted {
            agent_id: id.clone(),
            activation_id,
        })
    }

    pub(crate) async fn agent_wait(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let result = registry
            .wait_agent(id)
            .await
            .map_err(|error| agent_control_error(id, error))?;
        let outcome = result.outcome.as_ref().map(|s| s.state);
        Ok(RuntimeClientResult::AgentWait {
            agent_id: result.agent_id,
            activation_id: result.activation_id,
            outcome,
        })
    }

    pub(super) fn agent_reading_store(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::durable::SqliteConversationStore, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let agent =
            registry
                .agent_snapshot(id)
                .ok_or_else(|| RuntimeClientError::UnknownAgent {
                    agent_id: id.clone(),
                })?;
        registry
            .transcript_store(&agent.latest_activation)
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentTranscriptError::Unknown(subagent_id) => {
                    RuntimeClientError::UnknownSubagent { subagent_id }
                }
                crate::runtime::subagent::SubagentTranscriptError::Unavailable(message) => {
                    RuntimeClientError::RuntimeFailure {
                        message: format!("subagent history unavailable: {message}"),
                    }
                }
            })
    }

    pub(crate) fn agent_turns(
        &self,
        id: &crate::runtime::identity::AgentId,
        offset: Option<usize>,
        limit: usize,
    ) -> Result<crate::observation::turn::ObservedTurnPage, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        self.agent_reading_store(id)?
            .observation_turns(offset.map(|value| value as u64), limit)
            .map_err(RuntimeClientError::observation_store_error)
    }

    pub(crate) fn agent_transcript_window(
        &self,
        id: &crate::runtime::identity::AgentId,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        read_conversation_window(&self.agent_reading_store(id)?, at, limit)
    }

    pub(crate) fn agent_file_workspace(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<std::path::PathBuf, RuntimeClientError> {
        self.agent_registry()?
            .agent_workspace(id)
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "child workspace unavailable".into(),
            })
    }

    pub(crate) fn agent_file_reference(
        &self,
        id: &crate::runtime::identity::AgentId,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: usize,
    ) -> Result<crate::tools::session_files::SessionFileReference, RuntimeClientError> {
        let agent = self.agent_view(id)?;
        let store = self
            .agent_registry()?
            .transcript_store(&agent.activation_id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "child history unavailable".into(),
            })?;
        let index = u64::try_from(index).map_err(|_| RuntimeClientError::InvalidRange)?;
        let selected = store
            .observation_file_delivery(locator, index)
            .map_err(RuntimeClientError::observation_store_error)?;
        if selected.cut.read.conversation_id != agent.child_conversation_id
            || selected.file.scope.conversation_id != agent.child_conversation_id
        {
            return Err(RuntimeClientError::InvalidState {
                message: "delivery does not belong to this child".into(),
            });
        }
        let current = self.agent_view(id)?;
        let durable = |state| {
            matches!(
                state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            )
        };
        if current.activation_id != agent.activation_id
            || current.child_conversation_id != agent.child_conversation_id
            || durable(current.state) != durable(agent.state)
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(selected.file)
    }

    pub(crate) fn agent_artifact_read(
        &self,
        id: &crate::runtime::identity::AgentId,
        artifact_id: &crate::runtime::identity::ArtifactId,
    ) -> Result<String, RuntimeClientError> {
        use base64::Engine;
        let agent = self.agent_view(id)?;
        let registry = self.agent_registry()?;
        let _store = registry
            .transcript_store(&agent.activation_id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "child history unavailable".into(),
            })?;
        let root =
            registry
                .agent_artifact_root(id)
                .ok_or_else(|| RuntimeClientError::UnknownAgent {
                    agent_id: id.clone(),
                })?;
        let bytes = crate::tools::artifacts::ArtifactStore::read_bounded_from(&root, artifact_id)
            .map_err(|_| RuntimeClientError::InvalidState {
            message: "child artifact unavailable or exceeds 256 KiB".into(),
        })?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}
