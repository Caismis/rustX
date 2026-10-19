//! Finite canonical-message shaping for the native request read index.
use super::bounds::{TraceText, identity_fits};
use super::content::{assistant_blocks, tool_result_block, user_blocks, user_source_label};
use super::types::{TraceMessageRole, TraceRequestMessage};
use crate::message::types::MessageBlock;
use crate::model::input::ModelInputMessage;
use crate::model::types::ModelProtocol;

/// The provider-neutral protocol label of one historical invocation.
pub(crate) const fn protocol_label(protocol: ModelProtocol) -> &'static str {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => "openai_chat_completions",
        ModelProtocol::OpenAiResponses => "openai_responses",
        ModelProtocol::AnthropicMessages => "anthropic_messages",
    }
}
fn provenance_prefix<T: Clone + serde::Serialize + schemars::JsonSchema>(
    references: &[T],
    remaining: &mut usize,
) -> Vec<T> {
    references
        .iter()
        .take(64)
        .take_while(|reference| {
            let Some(bytes) = crate::observation::bounded::bounded_bytes(*reference, *remaining)
            else {
                return false;
            };
            *remaining -= bytes.len();
            true
        })
        .cloned()
        .collect()
}
pub(crate) fn bind_original_provenance(
    message: &mut TraceRequestMessage,
    header: &crate::durable::observation::MessageHeader,
) -> Result<(), crate::durable::ConversationStoreError> {
    use super::types::TraceInputProvenance;
    use crate::observation::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentSource,
    };
    let TraceInputProvenance::Original {
        source,
        locator,
        message_id_source,
        ..
    } = &mut message.provenance
    else {
        return Ok(());
    };
    let position = header.ledger_position.ok_or_else(|| {
        crate::durable::ConversationStoreError::Storage(
            "canonical provenance has no native Ledger position".into(),
        )
    })?;
    *locator = Some(CanonicalContentLocator::ByLedgerPosition { position });
    if source.is_none() {
        let original = header
            .content
            .iter()
            .find(|binding| {
                matches!(
                    binding.content_ref.source,
                    ContentSource::CanonicalMessageBlock {
                        component: CanonicalContentComponent::MessageMetadata,
                        ..
                    }
                )
            })
            .ok_or_else(|| {
                crate::durable::ConversationStoreError::Storage(
                    "canonical provenance identity metadata unavailable".into(),
                )
            })?;
        *message_id_source = Some(Box::new(crate::observation::cut::ContentBinding {
            path: vec!["provenance".into(), "source".into(), "message_id".into()],
            source_path: Some(vec!["id".into()]),
            content_ref: original.content_ref.clone(),
            deferred: true,
        }));
    }
    Ok(())
}
fn original_provenance(
    reference: &crate::model::projected::CanonicalMessageRef,
) -> super::types::TraceInputProvenance {
    let inline = identity_fits(reference.message_id.as_str());
    super::types::TraceInputProvenance::Original {
        conversation_id: reference.conversation_id.clone(),
        content_digest: reference.content_digest.clone(),
        locator: inline.then(|| {
            crate::observation::content::CanonicalContentLocator::ByMessageId {
                message_id: reference.message_id.clone(),
            }
        }),
        source: inline.then(|| reference.clone()),
        message_id_source: None,
    }
}
fn input_provenance(
    message: &ModelInputMessage,
    scope: &crate::runtime::identity::ConversationId,
) -> super::types::TraceInputProvenance {
    use super::types::TraceInputProvenance;
    match message {
        ModelInputMessage::Canonical(message) => original_provenance(
            &crate::model::projected::CanonicalMessageRef::of(scope, message),
        ),
        ModelInputMessage::Derived(derived) => {
            let mut remaining = 4096;
            let sources = provenance_prefix(&derived.sources, &mut remaining);
            let checkpoint_sources = provenance_prefix(&derived.checkpoint_sources, &mut remaining);
            TraceInputProvenance::Derived {
                truncated: sources.len() < derived.sources.len()
                    || checkpoint_sources.len() < derived.checkpoint_sources.len()
                    || derived.transformations.len() > 16,
                sources,
                checkpoint_sources,
                transformations: derived.transformations.iter().take(16).copied().collect(),
            }
        }
        ModelInputMessage::RequestOnly(_) => TraceInputProvenance::RequestOnly,
    }
}
fn derived_message(
    derived: &crate::model::projected::DerivedModelMessage,
    provenance: super::types::TraceInputProvenance,
) -> TraceRequestMessage {
    let provenance_truncated = matches!(
        &provenance,
        super::types::TraceInputProvenance::Derived {
            truncated: true,
            ..
        }
    );
    let (role, blocks, truncated) = match &derived.message {
        crate::model::projected::ProjectedMessage::User { content, .. } => {
            let (blocks, truncated) = user_blocks(content);
            (TraceMessageRole::User, blocks, truncated)
        }
        crate::model::projected::ProjectedMessage::Assistant { content } => {
            let (blocks, truncated) = assistant_blocks(content);
            (TraceMessageRole::Assistant, blocks, truncated)
        }
        crate::model::projected::ProjectedMessage::Tool {
            tool_call_id,
            tool_id,
            result,
        } => {
            let block = super::content::projected_tool_result_block(tool_call_id, tool_id, result);
            let truncated = block.is_none();
            (
                TraceMessageRole::Tool,
                block.into_iter().collect(),
                truncated,
            )
        }
    };
    TraceRequestMessage {
        request_occurrence: None,
        provenance,
        provenance_source: None,
        role,
        message_id: None,
        source: Some(format!(
            "derived:{:?}",
            derived.transformations.iter().take(16).collect::<Vec<_>>()
        )),
        blocks,
        truncated: truncated || provenance_truncated,
    }
}
#[cfg(test)]
pub(crate) fn request_message(
    message: &ModelInputMessage,
    scope: &crate::runtime::identity::ConversationId,
) -> TraceRequestMessage {
    request_message_with_original(message, scope, None)
}
/// The native reconstruction/commit owner has already validated this dependency.
/// Reusing its exact immutable reference avoids hashing the canonical body for display.
pub(crate) fn request_message_with_original(
    message: &ModelInputMessage,
    scope: &crate::runtime::identity::ConversationId,
    original: Option<&crate::model::projected::CanonicalMessageRef>,
) -> TraceRequestMessage {
    let provenance = match (message, original) {
        (ModelInputMessage::Canonical(_), Some(reference)) => original_provenance(reference),
        _ => input_provenance(message, scope),
    };
    match message {
        ModelInputMessage::Canonical(MessageBlock::User(user)) => {
            let (blocks, truncated) = user_blocks(&user.content);
            TraceRequestMessage {
                request_occurrence: None,
                provenance,
                provenance_source: None,
                role: TraceMessageRole::User,
                message_id: identity_fits(user.id.as_str()).then(|| user.id.clone()),
                source: Some(user_source_label(&user.source).to_owned()),
                blocks,
                truncated: truncated || !identity_fits(user.id.as_str()),
            }
        }
        ModelInputMessage::Canonical(MessageBlock::Assistant(assistant)) => {
            let (blocks, truncated) = assistant_blocks(&assistant.content);
            TraceRequestMessage {
                request_occurrence: None,
                provenance,
                provenance_source: None,
                role: TraceMessageRole::Assistant,
                message_id: identity_fits(assistant.id.as_str()).then(|| assistant.id.clone()),
                source: None,
                blocks,
                truncated: truncated || !identity_fits(assistant.id.as_str()),
            }
        }
        ModelInputMessage::Canonical(MessageBlock::Tool(tool)) => {
            let block = tool_result_block(tool);
            let truncated = block.is_none() || !identity_fits(tool.id.as_str());
            TraceRequestMessage {
                request_occurrence: None,
                provenance,
                provenance_source: None,
                role: TraceMessageRole::Tool,
                message_id: identity_fits(tool.id.as_str()).then(|| tool.id.clone()),
                source: None,
                blocks: block.into_iter().collect(),
                truncated,
            }
        }
        ModelInputMessage::Derived(derived) => derived_message(derived, provenance),
        ModelInputMessage::RequestOnly(context) => TraceRequestMessage {
            request_occurrence: None,
            provenance,
            provenance_source: None,
            role: TraceMessageRole::RequestOnly,
            // A request-only item deliberately has no canonical identity;
            // inventing one here would manufacture history.
            message_id: None,
            source: None,
            blocks: vec![super::types::TraceContentBlock::Text {
                text: TraceText::detail(&context.render()),
            }],
            truncated: false,
        },
    }
}
