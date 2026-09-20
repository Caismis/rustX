//! Logical-step contribution facts and immutable presentation data.

use serde::{Deserialize, Serialize};

use crate::context::status::AgentStatus;
use crate::durable::TranscriptCursor;
use crate::message::types::{ContextKind, ContributionEmission};
use crate::runtime::identity::{ContextContributorIdentity, MessageId};

/// Finite opportunities offered by an already scheduled logical step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionOpportunities {
    pub fresh_inbound: Option<FreshInboundOpportunity>,
    pub post_tool_batch: Option<PostToolBatchOpportunity>,
}

impl ContributionOpportunities {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.fresh_inbound.is_none() && self.post_tool_batch.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshInboundOpportunity {
    pub target_message_id: MessageId,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostToolBatchOpportunity;

/// Closed typed presentation vocabulary; it carries no execution authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContributionPresentation {
    AgentStatus(AgentStatus),
}

/// One accepted contribution, bound to its canonical identity at staging.
/// Retries retain this value and select it by their actual Surface membership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributionStart {
    pub message_id: MessageId,
    pub producer: ContextContributorIdentity,
    pub metadata: ContextKind,
    pub presentation: Option<ContributionPresentation>,
    pub emissions: Vec<ContributionEmission>,
    pub opportunities: ContributionOpportunities,
    pub post_tool_batch_anchor: Option<TranscriptCursor>,
}

impl ContributionStart {
    /// Checks the canonical content/metadata/provenance binding before persistence.
    pub(crate) fn validate_message(
        &self,
        message: &crate::message::MessageBlock,
    ) -> Result<(), crate::durable::ConversationStoreError> {
        use crate::message::{InboundKind, MessageBlock, UserSource};
        let MessageBlock::User(user) = message else {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "contribution is not a User context fact".to_owned(),
            ));
        };
        let source = match &self.producer {
            ContextContributorIdentity::Native(_) => UserSource::Runtime,
            ContextContributorIdentity::CertifiedExtension(identity) => UserSource::Extension {
                contributor: identity.clone(),
            },
        };
        if user.id != self.message_id
            || user.source != source
            || user.kind != InboundKind::Context(self.metadata.clone())
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "contribution canonical binding disagrees".to_owned(),
            ));
        }
        if matches!(self.metadata, ContextKind::ExtensionEnvironment)
            && !matches!(
                self.producer,
                ContextContributorIdentity::CertifiedExtension(_)
            )
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "extension context has native provenance".to_owned(),
            ));
        }
        if matches!(self.metadata, ContextKind::NativeEnvironment)
            && !matches!(self.producer, ContextContributorIdentity::Native(_))
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "native context has foreign provenance".to_owned(),
            ));
        }
        if let Some((owner, _)) = self.metadata.native_contribution_owner()
            && self.producer != ContextContributorIdentity::Native(owner)
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "contribution metadata has a foreign producer".to_owned(),
            ));
        }
        if let Some(ContributionPresentation::AgentStatus(status)) = &self.presentation
            && (self.metadata
                != ContextKind::AgentStatus(
                    status
                        .checked_generation_metadata()
                        .map_err(crate::durable::ConversationStoreError::InvalidReference)?,
                )
                || user.content
                    != vec![crate::message::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: crate::context::render_agent_status(status),
                        },
                    )])
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "typed presentation disagrees with accepted content".to_owned(),
            ));
        }
        Ok(())
    }

    /// Typed client publication after the owning startup transaction commits.
    pub(crate) fn observe(
        &self,
        observer: &dyn crate::agent::AgentExecutionObserver,
        attempt: &crate::runtime::identity::AttemptId,
        turn: u32,
    ) {
        if let Some(ContributionPresentation::AgentStatus(status)) = &self.presentation {
            observer.observe_status(&crate::agent::AgentStatusObservation {
                attempt_id: attempt.clone(),
                turn,
                status_message_id: self.message_id.clone(),
                opportunities: self.opportunities.clone(),
                post_tool_batch_anchor: self.post_tool_batch_anchor,
                status: status.clone(),
            });
        }
    }
}
