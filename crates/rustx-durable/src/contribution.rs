//! Store-side canonical-binding verification for contribution records.

use rustx_identity::ContextContributorIdentity;
use rustx_types::context::contribution::{ContributionPresentation, ContributionStart};
use rustx_types::message::types::{ContextKind, MessageBlock};

use crate::ConversationStoreError;

/// Checks the canonical content/metadata/provenance binding before persistence.
pub fn validate_contribution_message(
    start: &ContributionStart,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    use rustx_types::message::types::{InboundKind, MessageBlock, UserSource};
    let MessageBlock::User(user) = message else {
        return Err(ConversationStoreError::InvalidReference(
            "contribution is not a User context fact".to_owned(),
        ));
    };
    let source = match &start.producer {
        ContextContributorIdentity::Native(_) => UserSource::Runtime,
        ContextContributorIdentity::CertifiedExtension(identity) => UserSource::Extension {
            contributor: identity.clone(),
        },
    };
    if user.id != start.message_id
        || user.source != source
        || user.kind != InboundKind::Context(start.metadata.clone())
    {
        return Err(ConversationStoreError::InvalidReference(
            "contribution canonical binding disagrees".to_owned(),
        ));
    }
    if matches!(start.metadata, ContextKind::ExtensionEnvironment)
        && !matches!(
            start.producer,
            ContextContributorIdentity::CertifiedExtension(_)
        )
    {
        return Err(ConversationStoreError::InvalidReference(
            "extension context has native provenance".to_owned(),
        ));
    }
    if matches!(start.metadata, ContextKind::NativeEnvironment)
        && !matches!(start.producer, ContextContributorIdentity::Native(_))
    {
        return Err(ConversationStoreError::InvalidReference(
            "native context has foreign provenance".to_owned(),
        ));
    }
    if let Some((owner, _)) = start.metadata.native_contribution_owner()
        && start.producer != ContextContributorIdentity::Native(owner)
    {
        return Err(ConversationStoreError::InvalidReference(
            "contribution metadata has a foreign producer".to_owned(),
        ));
    }
    if let Some(ContributionPresentation::AgentStatus(status)) = &start.presentation
        && (start.metadata
            != ContextKind::AgentStatus(
                status
                    .checked_generation_metadata()
                    .map_err(ConversationStoreError::InvalidReference)?,
            )
            || user.content
                != vec![rustx_types::message::types::UserContentBlock::Text(
                    rustx_types::message::content::TextBlock {
                        text: rustx_types::context::status::render_agent_status(status),
                    },
                )])
    {
        return Err(ConversationStoreError::InvalidReference(
            "typed presentation disagrees with accepted content".to_owned(),
        ));
    }
    Ok(())
}
