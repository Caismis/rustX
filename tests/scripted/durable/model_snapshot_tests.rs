//! Model snapshot store suites covering provider-adapter request shapes.

mod tests {
    use crate::durable::model::snapshot::*;
    use rustx_conversation::surface::SurfaceRevision;
    use rustx_identity::CapabilityRevision;
    use rustx_types::context::assembly::ContextGeneration;
    use rustx_types::model::input::ModelInputMessage;
    use rustx_types::model::projected::{ProjectedMessage, SemanticTransformation};
    use rustx_types::model::types::ModelRequest;
    use rustx_types::runtime::identity::RequestId;
    use rustx_types::runtime::identity::{AttemptId, ConversationId, TurnId};

    fn frozen(request: &ModelRequest) -> RequestSnapshot {
        let mut snapshot = RequestSnapshot::new(
            rustx_types::runtime::types::ApprovalMode::FullAccess,
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-frozen"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            rustx_types::context::resources::LedgerFrontier((SurfaceRevision::new(1)).get()),
            request.effective_system_prompt.clone(),
            vec![],
            rustx_types::runtime::identity::RuntimeResourceRevision::new(1),
            request.invocation.clone(),
            128_000,
            None,
            request.tools.clone(),
            CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: vec![],
            },
            request.continuation.clone(),
            vec![],
        );
        snapshot
            .freeze_input(
                &ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f"),
                request,
            )
            .unwrap();
        snapshot
    }

    #[test]
    fn structured_contract_survives_snapshot_reconstruction() {
        use rustx_types::model::structured::{
            PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy,
        };
        let mut request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "source",
        );
        request.response_schema = Some(PreparedResponseSchema {
            schema: ResponseSchema::new("answer".into(), serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false}), 128).unwrap(),
            strategy: StructuredOutputStrategy::Native,
        });
        request.invocation.capabilities.json_schema_output = true;
        let snapshot = frozen(&request);
        let decoded: RequestSnapshot =
            serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        let reconstructed = decoded
            .reconstruct_with(|source| {
                request
                    .messages
                    .iter()
                    .find_map(|input| match input {
                        ModelInputMessage::Canonical(message) if source.matches(message) => {
                            Some(Ok(message.clone()))
                        }
                        _ => None,
                    })
                    .unwrap()
            })
            .unwrap();
        assert_eq!(reconstructed, request);
        assert_eq!(
            reconstructed.semantic_input_fingerprint(),
            snapshot.semantic_input_fingerprint
        );
    }

    #[test]
    fn primary_snapshot_retains_request_identity_serialization() {
        let request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "input",
        );
        let snapshot = frozen(&request);
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            value["identity"],
            serde_json::json!({"attempt_id": "attempt-frozen", "turn": "1", "retry_number": 0})
        );
        assert!(value.get("owner").is_none());
        assert_eq!(value["request_id"], "request:14:attempt-frozen:1:1:0");
        let decoded: RequestSnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.request_id, decoded.identity.request_id());
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One captured cross-protocol/native/Original-Derived request matrix.
    fn frozen_tool_associations_equal_captured_adapter_wire_and_reconstruction() {
        use crate::model::{ModelProtocol, projected::CanonicalMessageRef};
        use rustx_types::message::types::{
            AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
            ReasoningBlock, ToolCallOccurrenceRef, ToolMessageBlock,
        };
        use rustx_types::runtime::continuation::{
            AcceptedOutputContinuationSource, AnthropicContinuation, OpenAiResponsesContinuation,
            ProviderContinuationState,
        };
        use rustx_types::runtime::identity::{MessageId, ToolCallId, ToolId};
        use rustx_types::tools::types::ToolCall;
        let scope = ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f");
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            for mode in 0..5 {
                if mode > 0 && protocol != ModelProtocol::OpenAiResponses {
                    continue;
                }
                for derived_call in [false, true] {
                    for derived_result in [false, true] {
                        let mut request = crate::durable::sqlite::test_support::simple_request(
                            protocol, "test", "history",
                        );
                        if mode >= 2 {
                            request.invocation.compat.responses_storage =
                                rustx_types::model::capabilities::ResponsesStorageMode::Stateless;
                        }
                        for batch in 0..2 {
                            let assistant_id = MessageId::new(format!("assistant-{batch}"));
                            let mut content = Vec::new();
                            let native = match protocol {
                                ModelProtocol::AnthropicMessages => Some(
                                    ProviderContinuationState::Anthropic(AnthropicContinuation {
                                        opaque: serde_json::json!({"type":"thinking","thinking":"native", "signature":"original-signature"}).into(),
                                    }),
                                ),
                                ModelProtocol::OpenAiResponses if mode == 1 && batch == 1 => {
                                    Some(ProviderContinuationState::OpenAiResponses(
                                        OpenAiResponsesContinuation::Stored {
                                            previous_response_id: "accepted-response".into(),
                                        },
                                    ))
                                }
                                ModelProtocol::OpenAiResponses
                                    if ((mode == 2 || mode == 3) && batch == 1
                                        || mode == 4 && batch == 0) =>
                                {
                                    Some(ProviderContinuationState::OpenAiResponses(
                                        OpenAiResponsesContinuation::Stateless {
                                            items: vec![
                                                serde_json::json!({"type":"reasoning","id":"reasoning-native","encrypted_content":"original-encrypted"}),
                                                serde_json::json!({"type":"function_call","id":"fc-a","call_id":"repeat-a","name":"tool","arguments":"{}"}),
                                                serde_json::json!({"type":"function_call","id":"fc-b","call_id":"repeat-b","name":"tool","arguments":"{}"}),
                                            ].into(),
                                        },
                                    ))
                                }
                                _ => None,
                            };
                            if let Some(state) = native.clone() {
                                content.push(AssistantContentBlock::Reasoning(ReasoningBlock {
                                    origin: request.invocation.replay_origin(),
                                    text: (protocol == ModelProtocol::AnthropicMessages)
                                        .then(|| "native".into()),
                                    provider_state: Some(state),
                                }));
                            }
                            let start = u32::try_from(content.len()).unwrap();
                            for id in ["repeat-a", "repeat-b"] {
                                content.push(AssistantContentBlock::ToolCall(ToolCall {
                                    id: ToolCallId::new(id),
                                    tool_id: ToolId::new("tool"),
                                    name: "tool".into(),
                                    arguments: serde_json::json!({}),
                                }));
                            }
                            let assistant = MessageBlock::Assistant(AssistantMessageBlock {
                                id: assistant_id.clone(),
                                content,
                            });
                            if (mode == 1 || mode == 2) && batch == 1 {
                                request.continuation_source = Some(
                                    AcceptedOutputContinuationSource::from_accepted_output(
                                        RequestId::new("accepted-request"),
                                        &scope,
                                        request.messages.semantic_fingerprint(),
                                        &assistant,
                                    )
                                    .unwrap(),
                                );
                                request.continuation = native;
                            }
                            request.messages.push(assistant.into());
                            // Results deliberately reverse the call order within a complete batch.
                            for (block, id) in [(start + 1, "repeat-b"), (start, "repeat-a")] {
                                request.messages.push(
                                    MessageBlock::Tool(ToolMessageBlock {
                                        id: MessageId::new(format!("result-{batch}-{block}")),
                                        tool_call_id: ToolCallId::new(id),
                                        occurrence: ToolCallOccurrenceRef::new(
                                            assistant_id.clone(),
                                            ContentBlockIndex::new(block),
                                        ),
                                        tool_id: ToolId::new("tool"),
                                        result:
                                            crate::durable::sqlite::test_support::success_result(
                                                "result",
                                            )
                                            .into(),
                                    })
                                    .into(),
                                );
                            }
                        }
                        let canonical: Vec<_> = request
                            .messages
                            .iter()
                            .filter_map(|m| m.as_canonical().cloned())
                            .collect();
                        let mut snapshot = frozen(&request);
                        if mode == 1 {
                            let covered =
                                serde_json::to_value(&snapshot.tool_associations[..2]).unwrap();
                            assert!(
                                covered
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .all(|a| a["wire_id"].is_null()),
                                "covered associations must not claim fields absent from the actual wire request"
                            );
                        }
                        let associations = snapshot.tool_associations.clone();
                        for message in &mut request.messages {
                            let convert = matches!(
                                message.view(),
                                rustx_types::model::projected::ModelMessageView::Assistant(_)
                            ) && derived_call
                                || matches!(
                                    message.view(),
                                    rustx_types::model::projected::ModelMessageView::Tool(_, _)
                                ) && derived_result;
                            if convert {
                                let original = message.as_canonical().unwrap();
                                *message = ModelInputMessage::Derived(
                                    rustx_types::model::projected::DerivedModelMessage {
                                        message: ProjectedMessage::from(original),
                                        sources: vec![CanonicalMessageRef::of(&scope, original)],
                                        checkpoint_sources: vec![],
                                        transformations: vec![],
                                    },
                                );
                            }
                        }
                        snapshot.freeze_input(&scope, &request).unwrap();
                        snapshot.tool_associations = associations.clone();
                        // Oracle: retain the old full-array mapping/relabeling semantics
                        // across every Original/Derived, native replay and duplicate-ID case.
                        let history = crate::durable::projection::HistoricalModelInput {
                            tool_associations: snapshot
                                .tool_associations
                                .iter()
                                .map(|a| {
                                    let message =
                                        &request.messages[a.call.message.item_index as usize];
                                    let rustx_types::model::projected::ModelMessageView::Assistant(
                                        content,
                                    ) = message.view()
                                    else {
                                        unreachable!()
                                    };
                                    let AssistantContentBlock::ToolCall(call) =
                                        &content[a.call.block.0 as usize]
                                    else {
                                        unreachable!()
                                    };
                                    crate::durable::model::occurrence::HistoricalToolAssociation {
                                        call_message: a.call.message.item_index,
                                        call_block: a.call.block.0,
                                        result_message: a.result.item_index,
                                        source: a.source.clone(),
                                        wire_id: Some(
                                            crate::durable::model::occurrence::ProviderWireCorrelationId(
                                                call.id.to_string(),
                                            ),
                                        ),
                                        native_origins: crate::durable::model::occurrence::native_origins(
                                            &message.view(),
                                        ),
                                    }
                                })
                                .collect(),
                            cut: snapshot.history_cut.clone(),
                            messages: request.messages.clone(),
                            input_manifest: snapshot.input.clone(),
                            checkpoint_fingerprint: None,
                            materialized_bytes: 0,
                        };
                        for insertion in [None, Some(0), Some(request.messages.len())] {
                            let mut actual = request.clone();
                            if let Some(at) = insertion {
                                actual.messages.insert(at,rustx_types::model::input::ModelInputMessage::RequestOnly(rustx_types::model::input::RequestOnlyModelContext::AuxiliaryQuestion{text:"request-only oracle".into()}));
                            }
                            for retry in 0..3 {
                                if retry == 2 {
                                    actual.invocation.replay_binding =
                                        Some("different-replay-origin".into());
                                }
                                let id = RequestId::new(format!("oracle-request-{retry}"));
                                let frozen = crate::durable::model::occurrence::freeze_associations(
                                    &history, &id, &actual, insertion,
                                );
                                let labels =
                                    rustx_types::model::adapter::correlation::compute_labels_at_boundary(&actual);
                                let Ok(labels) = labels else {
                                    assert!(frozen.is_err());
                                    continue;
                                };
                                let frozen = frozen.unwrap();
                                let mut expected = associations.to_vec();
                                for a in &mut expected {
                                    a.call.message.request_id = id.clone();
                                    a.result.request_id = id.clone();
                                    if insertion
                                        .is_some_and(|at| a.call.message.item_index as usize >= at)
                                    {
                                        a.call.message.item_index += 1;
                                    }
                                    if insertion
                                        .is_some_and(|at| a.result.item_index as usize >= at)
                                    {
                                        a.result.item_index += 1;
                                    }
                                    a.wire_id = labels
                                        .get(&(a.call.message.item_index, a.call.block.0))
                                        .cloned()
                                        .map(crate::durable::model::occurrence::ProviderWireCorrelationId);
                                }
                                let encoded = serde_json::to_vec(&frozen).unwrap();
                                assert!(frozen.encoded_len_upper_bound().unwrap() >= encoded.len());
                                assert_eq!(encoded, serde_json::to_vec(&expected).unwrap());
                                assert_eq!(
                                    *frozen, expected,
                                    "protocol={protocol:?} mode={mode} derived={derived_call}/{derived_result} insertion={insertion:?} retry={retry}"
                                );
                            }
                        }
                        let translate = |input: &ModelRequest| {
                            let tools = crate::model::adapter::validation::validate_request(
                                input, protocol,
                            )
                            .unwrap();
                            match protocol {
                                ModelProtocol::OpenAiChatCompletions => serde_json::to_value(crate::model::adapter::openai::chat_completions::translate_request(input,&tools).unwrap()).unwrap(),
                                ModelProtocol::OpenAiResponses => serde_json::to_value(crate::model::adapter::openai::responses::translate_request(input,&tools,input.invocation.compat.responses_storage).unwrap()).unwrap(),
                                ModelProtocol::AnthropicMessages => serde_json::to_value(crate::model::adapter::anthropic::mapping::translate_request(input,&tools).unwrap()).unwrap(),
                            }
                        };
                        let mut captured = translate(&request);
                        let actual =
                            rustx_types::model::adapter::correlation::observed_correlations(
                                &mut captured,
                                protocol,
                            );
                        let mut ordered = snapshot.tool_associations.clone();
                        ordered.sort_by_key(|a| (a.call.message.item_index, a.call.block.0));
                        let calls: Vec<_> = ordered
                            .iter()
                            .filter_map(|a| a.wire_id.as_ref().map(|id| id.0.clone()))
                            .collect();
                        ordered.sort_by_key(|a| a.result.item_index);
                        let results: Vec<_> = ordered
                            .iter()
                            .filter_map(|a| a.wire_id.as_ref().map(|id| id.0.clone()))
                            .collect();
                        let actual_calls: Vec<_> = actual
                            .iter()
                            .filter(|(call, _)| *call)
                            .map(|(_, id)| id.clone())
                            .collect();
                        let actual_results: Vec<_> = actual
                            .iter()
                            .filter(|(call, _)| !*call)
                            .map(|(_, id)| id.clone())
                            .collect();
                        if mode == 1 {
                            assert_eq!(actual_calls, [] as [std::string::String; 0]);
                        } else {
                            assert_eq!(actual_calls, calls);
                        }
                        assert_eq!(actual_results, results);
                        let reconstructed =
                            snapshot.reconstruct_from_canonical(&canonical).unwrap();
                        assert_eq!(translate(&request), translate(&reconstructed));
                    }
                }
            }
        }
    }

    #[test]
    fn frozen_derived_input_reconstructs_without_dynamic_selection() {
        let mut request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "original",
        );
        let source = request.messages[0].as_canonical().unwrap().clone();
        if let ModelInputMessage::Canonical(rustx_types::message::types::MessageBlock::User(user)) =
            &mut request.messages[0]
        {
            user.content = vec![rustx_types::message::types::UserContentBlock::Text(
                rustx_types::message::content::TextBlock {
                    text: "frozen upload path /workspace/input.csv".into(),
                },
            )];
        }
        request.messages[0].mark_transformed(
            &source,
            &ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f"),
            SemanticTransformation::UploadRendering,
        );
        assert_eq!(request.messages[0].canonical_id(), None);
        let snapshot = frozen(&request);
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: RequestSnapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            decoded
                .reconstruct_with(|_| panic!("Derived input must not resolve dynamic sources"))
                .unwrap(),
            request
        );
        assert!(
            matches!(&decoded.input[0], rustx_types::model::projected::FrozenModelInput::Derived(m) if matches!(m.message, ProjectedMessage::User { .. }))
        );
    }

    #[test]
    fn missing_original_or_redacted_derived_bytes_are_non_reconstructible() {
        let request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "required bytes",
        );
        let snapshot = frozen(&request);
        assert!(snapshot.reconstruct_from_canonical(&[]).is_err());
        let mut redacted = snapshot.clone();
        redacted.input.clear();
        assert!(
            redacted
                .reconstruct_from_canonical(&[])
                .unwrap_err()
                .to_string()
                .contains("missing input bytes")
        );
        assert_eq!(
            snapshot
                .reconstruct_from_canonical(&[request.messages[0].as_canonical().unwrap().clone()])
                .unwrap(),
            request
        );
    }

    #[test]
    fn semantic_fingerprint_ignores_occurrence_identity_but_includes_materialized_input() {
        let request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "unchanged",
        );
        let mut snapshot = frozen(&request);
        let fingerprint = snapshot.semantic_input_fingerprint.clone();
        snapshot.identity.retry_number = 1;
        snapshot.request_id = snapshot.identity.request_id();
        snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
        snapshot.request_context_ids.clear();
        assert_eq!(snapshot.semantic_input_fingerprint, fingerprint);
        let mut other = request.clone();
        other
            .effective_system_prompt
            .push_str("authorized correction");
        assert_ne!(other.semantic_input_fingerprint(), fingerprint);
        other = request.clone();
        other.images.insert(
            rustx_types::runtime::identity::ArtifactId::new("image"),
            serde_json::from_str("[1,2,3]").unwrap(),
        );
        assert_ne!(other.semantic_input_fingerprint(), fingerprint);
    }
    #[test]
    fn frozen_original_integrity_includes_provenance_metadata() {
        let request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "same provider text",
        );
        let snapshot = frozen(&request);
        let mut changed = request.messages[0].as_canonical().unwrap().clone();
        if let rustx_types::message::types::MessageBlock::User(user) = &mut changed {
            user.source = rustx_types::message::types::UserSource::Runtime;
        }
        assert!(
            snapshot
                .reconstruct_from_canonical(&[changed])
                .unwrap_err()
                .to_string()
                .contains("integrity mismatch")
        );
    }

    #[test]
    fn frozen_body_and_provenance_budgets_do_not_install_partial_input() {
        let request = crate::durable::sqlite::test_support::simple_request(
            rustx_types::model::types::ModelProtocol::OpenAiChatCompletions,
            "test",
            "original",
        );
        let source = request.messages[0].as_canonical().unwrap().clone();
        let mut oversized = request.clone();
        if let ModelInputMessage::Canonical(rustx_types::message::types::MessageBlock::User(user)) =
            &mut oversized.messages[0]
        {
            user.content = vec![rustx_types::message::types::UserContentBlock::Text(
                rustx_types::message::content::TextBlock {
                    text: "x".repeat(MAX_FROZEN_DERIVED_BYTES),
                },
            )];
        }
        let scope = ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f");
        oversized.messages[0].mark_transformed(
            &source,
            &scope,
            SemanticTransformation::UploadRendering,
        );
        let mut snapshot = frozen(&request);
        let before = snapshot.clone();
        assert!(
            snapshot
                .freeze_input(&scope, &oversized)
                .unwrap_err()
                .contains("byte budget")
        );
        assert_eq!(snapshot, before);
        let mut provenance = request.clone();
        provenance.messages[0] =
            ModelInputMessage::Derived(rustx_types::model::projected::DerivedModelMessage {
                message: ProjectedMessage::from(&source),
                sources: vec![
                    rustx_types::model::projected::CanonicalMessageRef::of(
                        &scope, &source
                    );
                    MAX_FROZEN_PROVENANCE_REFS + 1
                ],
                transformations: vec![SemanticTransformation::UploadRendering],
                checkpoint_sources: Vec::new(),
            });
        assert!(
            snapshot
                .freeze_input(&scope, &provenance)
                .unwrap_err()
                .contains("provenance budget")
        );
        assert_eq!(snapshot, before);
    }

    #[test]
    fn auxiliary_request_ids_use_a_distinct_uuid_domain() {
        #[derive(Debug)]
        struct FixedUuid(uuid::Uuid);
        impl rustx_types::runtime::identity::UuidV7Generator for FixedUuid {
            fn next_uuid(&self) -> uuid::Uuid {
                self.0
            }
        }

        let generator =
            FixedUuid(uuid::Uuid::parse_str("36524fd8-f674-7fc2-8125-06d01fee0e18").unwrap());
        let request_id =
            rustx_types::runtime::identity::RequestId::generate_auxiliary(&generator).unwrap();
        assert_eq!(
            request_id.as_str(),
            "req_36524fd8-f674-7fc2-8125-06d01fee0e18"
        );
        let primary = RequestIdentity {
            attempt_id: rustx_types::runtime::identity::AttemptId::new("attempt-1"),
            turn: rustx_types::runtime::identity::TurnId::new("turn-1"),
            retry_number: 0,
        }
        .request_id();
        assert_eq!(primary.as_str(), "request:9:attempt-1:6:turn-1:0");
        assert_ne!(request_id, primary);
        assert!(
            rustx_types::runtime::identity::AuxiliaryCallId::parse(request_id.into_string())
                .is_err()
        );
    }
}
