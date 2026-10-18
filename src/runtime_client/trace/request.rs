//! Finite canonical-message shaping for the native request read index.
//! Full historical request reconstruction is a test-only independent oracle.

#[cfg(test)]
use super::bounds::{TRACE_DETAIL_MESSAGES, TRACE_DETAIL_TOOLS, TraceJson};
use super::bounds::{TraceText, identity_fits};
use super::content::{assistant_blocks, tool_result_block, user_blocks, user_source_label};
use super::types::{TraceMessageRole, TraceRequestMessage};
#[cfg(test)]
use super::types::{
    TraceRequestDetail, TraceRequestFailure, TraceRequestOption, TraceToolDefinition,
};
#[cfg(test)]
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::message::types::MessageBlock;
use crate::model::input::ModelInputMessage;
#[cfg(test)]
use crate::model::inspection::REQUEST_OPTION_ALLOWLIST;
#[cfg(test)]
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelProtocol;
#[cfg(test)]
use crate::model::types::ModelRequest;

/// The provider-neutral protocol label of one historical invocation.
pub(super) const fn protocol_label(protocol: ModelProtocol) -> &'static str {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => "openai_chat_completions",
        ModelProtocol::OpenAiResponses => "openai_responses",
        ModelProtocol::AnthropicMessages => "anthropic_messages",
    }
}

/// The terminal outcome facts of one actual request, read from its own
/// Journal terminal rather than from any later request's.
#[cfg(test)]
pub(super) struct RequestOutcome {
    pub usage: Option<crate::model::types::ModelUsage>,
    pub failure: Option<TraceRequestFailure>,
    pub generation: Option<super::types::TraceGeneration>,
}

/// Projects the exact historical request of one frozen snapshot.
/// Returns None when a mandatory identity exceeds the Trace bound.
///
/// # Errors
///
/// Propagates the durable read failure of snapshot or immutable-input
/// reconstruction; a failure is never interpreted as an empty request.
#[cfg(test)]
pub(super) fn request_detail(
    store: &dyn ConversationStore,
    snapshot: &RequestSnapshot,
    outcome: RequestOutcome,
    previous: &super::oracle_summary::OriginalPreviousRequest,
) -> Result<Option<TraceRequestDetail>, ConversationStoreError> {
    if !identity_fits(snapshot.request_id.as_str())
        || !identity_fits(snapshot.identity.attempt_id.as_str())
        || !identity_fits(snapshot.identity.turn.as_str())
        || !identity_fits(snapshot.provisional_message_id.as_str())
    {
        return Ok(None);
    }
    // Resolve only this snapshot's ordered frozen input and immutable dependencies.
    // No historical selection or content transformation runs during reconstruction.
    let reconstructed = store.reconstruct_model_request(&snapshot.request_id)?;
    let (messages, messages_truncated) = request_messages(
        &reconstructed,
        store.conversation_id(),
        &snapshot.request_id,
    );
    let mut tools_truncated = reconstructed.tools.len() > TRACE_DETAIL_TOOLS;
    let mut tools = Vec::new();
    for definition in reconstructed.tools.iter().take(TRACE_DETAIL_TOOLS) {
        if !identity_fits(definition.id.as_str()) {
            tools_truncated = true;
            continue;
        }
        tools.push(TraceToolDefinition {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            description: TraceText::detail(&definition.description),
            input_schema: TraceJson::bounded(&definition.input_schema),
        });
    }
    let (options, omitted_option_count) = request_options(&snapshot.invocation.request_params);
    let contributions = contribution_metadata(snapshot)?;
    Ok(Some(TraceRequestDetail {
        tool_associations: snapshot
            .tool_associations
            .iter()
            .take(32)
            .filter(|a| {
                a.wire_id.as_ref().is_none_or(|id| identity_fits(&id.0))
                    && identity_fits(a.source.assistant.message_id.as_str())
            })
            .cloned()
            .collect(),
        tool_associations_truncated: snapshot.tool_associations.len() > 32
            || snapshot.tool_associations.iter().take(32).any(|a| {
                !a.wire_id.as_ref().is_none_or(|id| identity_fits(&id.0))
                    || !identity_fits(a.source.assistant.message_id.as_str())
            }),
        contributions,
        predecessor: previous.identity(),
        previous_system_prompt: previous.prompt(),
        request_id: snapshot.request_id.clone(),
        attempt_id: snapshot.identity.attempt_id.clone(),
        step_id: snapshot.identity.turn.clone(),
        retry_number: snapshot.identity.retry_number,
        assistant_message_id: snapshot.provisional_message_id.clone(),
        model: snapshot.invocation.model.clone(),
        protocol: protocol_label(snapshot.invocation.protocol).to_owned(),
        max_output_tokens: snapshot.invocation.max_output_tokens,
        context_window_tokens: snapshot.context_window_tokens,
        reasoning_enabled: snapshot.reasoning_enabled,
        profile: snapshot
            .profile
            .as_ref()
            .map(|profile| profile.as_str().to_owned()),
        options,
        omitted_option_count,
        // The prompt actually sent, from the snapshot rather than from the
        // current prompt assembly. Reconstruction carries the same value;
        // taking it from the snapshot keeps the authority explicit.
        effective_system_prompt: TraceText::detail(&snapshot.effective_system_prompt),
        messages,
        messages_truncated,
        tools,
        tools_truncated,
        usage: outcome.usage,
        failure: outcome.failure,
        generation: outcome.generation,
    }))
}

#[cfg(test)]
pub(super) fn contribution_metadata(
    snapshot: &RequestSnapshot,
) -> Result<Vec<super::types::TraceContributionMetadata>, ConversationStoreError> {
    snapshot
        .contributions
        .iter()
        .map(|record| {
            let presentation = match &record.presentation {
                Some(crate::context::contribution::ContributionPresentation::AgentStatus(
                    status,
                )) => {
                    let turn = snapshot.identity.turn.as_str().parse().map_err(|_| {
                        ConversationStoreError::InvalidReference(
                            "historical contribution has invalid logical step".to_owned(),
                        )
                    })?;
                    Some(super::types::TraceContributionPresentation::AgentStatus(
                        crate::runtime_client::projection::status_view(
                            &crate::agent::AgentStatusObservation {
                                attempt_id: snapshot.identity.attempt_id.clone(),
                                turn,
                                status_message_id: record.message_id.clone(),
                                opportunities: record.opportunities.clone(),
                                post_tool_batch_anchor: record.post_tool_batch_anchor,
                                status: status.clone(),
                            },
                        ),
                    ))
                }
                None => None,
            };
            Ok(super::types::TraceContributionMetadata {
                message_id: record.message_id.clone(),
                producer: record.producer.clone(),
                metadata: record.metadata.clone(),
                presentation,
            })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()
}

/// Projects the reconstructed provider-neutral request context, in wire order.
#[cfg(test)]
fn request_messages(
    request: &ModelRequest,
    scope: &crate::runtime::identity::ConversationId,
    request_id: &crate::runtime::identity::RequestId,
) -> (Vec<TraceRequestMessage>, bool) {
    // The newest items explain a request best, so an oversized context keeps
    // its tail rather than its head: the request-scoped context, the current
    // user turn and the tool results that prompted the call are all there.
    let truncated = request.messages.len() > TRACE_DETAIL_MESSAGES;
    let start = request.messages.len().saturating_sub(TRACE_DETAIL_MESSAGES);
    let selected = request.messages.range(start..request.messages.len());
    let messages = selected
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let mut detail = request_message(message, scope);
            detail.request_occurrence =
                Some(crate::context::checkpoint::RequestMessageOccurrence {
                    request_id: request_id.clone(),
                    item_index: u32::try_from(start + index)
                        .expect("bounded frozen request coordinates"),
                });
            detail
        })
        .collect();
    (messages, truncated)
}

fn provenance_prefix<T: Clone + serde::Serialize + schemars::JsonSchema>(
    references: &[T],
    remaining: &mut usize,
) -> Vec<T> {
    references
        .iter()
        .take(64)
        .take_while(|reference| {
            let Some(bytes) =
                crate::runtime_client::checkpoint::bounded_bytes(*reference, *remaining)
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
    use crate::runtime_client::content::{
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
        *message_id_source = Some(Box::new(
            crate::runtime_client::checkpoint::ContentBinding {
                path: vec!["provenance".into(), "source".into(), "message_id".into()],
                source_path: Some(vec!["id".into()]),
                content_ref: original.content_ref.clone(),
                deferred: true,
            },
        ));
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
            crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
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

/// Splits configured request parameters into allowlisted and omitted.
#[cfg(test)]
fn request_options(
    params: &crate::model::invocation::RequestParams,
) -> (Vec<TraceRequestOption>, usize) {
    let mut options = Vec::new();
    let mut omitted = 0;
    for (name, value) in params {
        if REQUEST_OPTION_ALLOWLIST.contains(&name.as_str()) {
            options.push(TraceRequestOption {
                name: name.clone(),
                value: TraceJson::bounded(value),
            });
        } else {
            omitted += 1;
        }
    }
    options.sort_by(|left, right| left.name.cmp(&right.name));
    (options, omitted)
}

#[cfg(test)]
mod tests {
    use super::{REQUEST_OPTION_ALLOWLIST, request_options};

    /// Only allowlisted provider-neutral options cross the boundary, and the
    /// rest are counted so the omission stays visible.
    #[test]
    fn request_options_use_a_typed_allowlist() {
        let mut params = serde_json::Map::new();
        params.insert("temperature".into(), serde_json::json!(0.2));
        params.insert("top_p".into(), serde_json::json!(0.9));
        params.insert("api_key".into(), serde_json::json!("sk-secret"));
        params.insert("authorization".into(), serde_json::json!("Bearer secret"));
        params.insert("x_vendor_tuning".into(), serde_json::json!({ "a": 1 }));
        let (options, omitted) = request_options(&params);
        assert_eq!(omitted, 3);
        let names: Vec<_> = options.iter().map(|option| option.name.as_str()).collect();
        assert_eq!(names, vec!["temperature", "top_p"]);
        let encoded = serde_json::to_string(&options).expect("options");
        assert!(!encoded.contains("sk-secret"));
        assert!(!encoded.contains("Bearer"));
        assert!(!encoded.contains("x_vendor_tuning"));
    }

    /// The allowlist itself contains no credential-shaped key.
    #[test]
    fn the_allowlist_names_no_credential_key() {
        for name in REQUEST_OPTION_ALLOWLIST {
            assert!(!name.contains("key"), "{name}");
            assert!(!name.contains("token"), "{name}");
            assert!(!name.contains("secret"), "{name}");
            assert!(!name.contains("auth"), "{name}");
        }
    }
}
