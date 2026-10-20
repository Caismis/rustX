//! Test-only independent oracle of full historical request reconstruction.
//! Production request message shaping is `crate::observation::trace::request`.

use crate::durable::{ConversationStore, ConversationStoreError};
use crate::model::inspection::REQUEST_OPTION_ALLOWLIST;
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::observation::trace::bounds::{TRACE_DETAIL_MESSAGES, TRACE_DETAIL_TOOLS, TraceJson};
use crate::observation::trace::bounds::{TraceText, identity_fits};
use crate::observation::trace::request::{protocol_label, request_message};
use crate::observation::trace::types::TraceRequestMessage;
use crate::observation::trace::types::{
    TraceRequestDetail, TraceRequestFailure, TraceRequestOption, TraceToolDefinition,
};

/// The terminal outcome facts of one actual request, read from its own
/// Journal terminal rather than from any later request's.
pub(super) struct RequestOutcome {
    pub usage: Option<crate::model::types::ModelUsage>,
    pub failure: Option<TraceRequestFailure>,
    pub generation: Option<crate::observation::trace::types::TraceGeneration>,
}

/// Projects the exact historical request of one frozen snapshot.
/// Returns None when a mandatory identity exceeds the Trace bound.
///
/// # Errors
///
/// Propagates the durable read failure of snapshot or immutable-input
/// reconstruction; a failure is never interpreted as an empty request.
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

pub(super) fn contribution_metadata(
    snapshot: &RequestSnapshot,
) -> Result<Vec<crate::observation::trace::types::TraceContributionMetadata>, ConversationStoreError>
{
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
                    Some(crate::observation::trace::types::TraceContributionPresentation::AgentStatus(
                        crate::observation::status::status_view(
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
            Ok(crate::observation::trace::types::TraceContributionMetadata {
                message_id: record.message_id.clone(),
                producer: record.producer.clone(),
                metadata: record.metadata.clone(),
                presentation,
            })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()
}

/// Projects the reconstructed provider-neutral request context, in wire order.
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

/// Splits configured request parameters into allowlisted and omitted.
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
