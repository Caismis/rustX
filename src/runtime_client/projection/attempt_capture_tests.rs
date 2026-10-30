use crate::runtime_client::checkpoint::{self, Capture, ContentBinding};
use crate::runtime_client::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
    ContentSource, PublicationContentComponent,
};
use crate::runtime_client::snapshot::{ForegroundToolExecution, InFlightAssistantMessage};

fn content_reference() -> ContentRef {
    ContentRef {
        conversation_id: ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8d2"),
        source: ContentSource::CanonicalMessageBlock {
            component: CanonicalContentComponent::BlockMetadata,
            locator: CanonicalContentLocator::ByLedgerPosition { position: 19 },
            block_index: ContentBlockIndex::new(0),
        },
        representation: ContentRepresentation::CanonicalJsonV1,
        version: 1,
        content_revision: 1,
        readable_end: 81,
    }
}

fn tool_result(bytes: usize) -> ToolExecutionResult {
    use crate::message::content::{FileReference, TextBlock};
    use crate::runtime::identity::ArtifactId;
    use crate::runtime::workflow::WorkflowId;
    use crate::tools::session_files::{SessionFileReference, SessionFileScope};
    use crate::tools::types::{ManagedOutputContinuation, ToolResultContent, TruncationState};
    ToolExecutionResult {
        workflow: Some(Box::new(crate::runtime::workflow::WorkflowToolIdentity {
            workflow_id: WorkflowId::parse("workflow").unwrap(),
            program_digest: "program digest".into(),
        })),
        status: ToolExecutionStatus::Failed {
            error: "retained error".into(),
        },
        content: vec![ToolResultContent::Text(TextBlock {
            text: "x".repeat(bytes),
        })],
        duration_ms: u64::MAX,
        exit_code: Some(-1),
        artifacts: vec![FileReference {
            artifact_id: ArtifactId::new("artifact"),
            name: Some("result".into()),
            mime_type: None,
            description: Some("artifact description".into()),
        }],
        deliveries: vec![SessionFileReference {
            scope: SessionFileScope {
                conversation_id: ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8d2"),
                device: "device".into(),
                inode: "inode".into(),
            },
            path: "result.txt".into(),
            name: "result.txt".into(),
            description: Some("delivery".into()),
            mime_type: "text/plain".into(),
        }],
        truncation: Some(TruncationState {
            truncated: true,
            original_bytes: Some(8_192),
        }),
        managed_output: Some(Box::new(ManagedOutputContinuation::Partial {
            locator: "/output/partial.txt".into(),
            diagnostic: "partial output".into(),
        })),
    }
}

fn foreground(state: ForegroundToolState, committed: bool) -> ForegroundToolExecution {
    ForegroundToolExecution {
        occurrence: crate::runtime_client::snapshot::RuntimeClientToolOccurrence {
            locator: CanonicalContentLocator::ByLedgerPosition { position: 17 },
            block_index: ContentBlockIndex::new(3),
        },
        message_id: Some(MessageId::new("proposal")),
        block_index: ContentBlockIndex::new(3),
        call_id: Some(ToolCallId::new("call")),
        metadata_ref: Some(content_reference()),
        assembly_complete: true,
        result_source: Some(crate::runtime_client::snapshot::JournalFieldRef {
            sequence: 31,
            path: vec!["result".into()],
            encoded_bytes: 123,
        }),
        result_metadata_ref: Some(content_reference()),
        committed_result: committed.then(|| tool_result(32_768)),
        committed_result_deferred: true,
        tool_id: Some(ToolId::new("tool")),
        name: "tool name".into(),
        state,
    }
}

fn old_deferred(
    mut tool: ForegroundToolExecution,
    source: Option<&super::super::ForegroundResultContent>,
) -> ForegroundToolExecution {
    match &mut tool.state {
        ForegroundToolState::Assembled { arguments }
        | ForegroundToolState::Running { arguments, .. }
        | ForegroundToolState::Settled { arguments, .. } => arguments.clear(),
    }
    if let Some(source) = source {
        tool.result_metadata_ref = Some(source.metadata.clone());
        tool.committed_result_deferred = tool.committed_result.is_some();
        let clear = |result: &mut ToolExecutionResult| {
            result.content.clear();
            result.artifacts.clear();
            result.deliveries.clear();
            result.workflow = None;
        };
        if let Some(result) = &mut tool.committed_result {
            clear(result);
        }
        if let ForegroundToolState::Settled { result, .. } = &mut tool.state {
            clear(result);
        }
    }
    tool
}

#[test]
fn foreground_admission_matches_old_fields_without_allocating_discarded_payloads() {
    let mut seed = projection().capture_working_seed();
    for size in [8, 32_768] {
        for state in [
            ForegroundToolState::Assembled {
                arguments: "x".repeat(size),
            },
            ForegroundToolState::Running {
                arguments: "x".repeat(size),
                progress: Some(ToolProgress {
                    message: Some("running".into()),
                    completed: Some(0.25),
                    total: Some(4.0),
                }),
            },
            ForegroundToolState::Settled {
                arguments: "x".repeat(size),
                result: tool_result(size),
            },
        ] {
            for committed in [false, true] {
                let tool = foreground(state.clone(), committed);
                for authoritative_result in [false, true] {
                    seed.foreground_content = authoritative_result
                        .then(|| super::super::ForegroundResultContent {
                            occurrence: tool.occurrence.clone(),
                            metadata: ContentRef {
                                readable_end: 1_234,
                                ..content_reference()
                            },
                            content: Vec::new(),
                            frontier: checkpoint::CollectionFrontier {
                                domain: "result.content".into(),
                                revision: 19,
                                total: 1,
                                loaded: 1,
                                next_offset: None,
                            },
                        })
                        .into_iter()
                        .collect();
                    let mut old_capture = Capture::new(9);
                    let admitted = old_capture.reserve(&tool);
                    let expected = if admitted {
                        tool.clone()
                    } else {
                        old_deferred(tool.clone(), seed.foreground_content.first())
                    };
                    let mut actual_capture = Capture::new(9);
                    let mut bindings = Vec::new();
                    let actual = seed.capture_foreground(
                        &mut actual_capture,
                        std::slice::from_ref(&tool),
                        &mut bindings,
                    );
                    assert_eq!(
                        serde_json::to_vec(&actual[0]).unwrap(),
                        serde_json::to_vec(&expected).unwrap()
                    );
                    assert_eq!(actual_capture.evicted, old_capture.evicted);
                    if !admitted {
                        let arguments = match &actual[0].state {
                            ForegroundToolState::Assembled { arguments }
                            | ForegroundToolState::Running { arguments, .. }
                            | ForegroundToolState::Settled { arguments, .. } => arguments,
                        };
                        assert_eq!(arguments.capacity(), 0);
                        let mut results: Vec<_> = actual[0].committed_result.iter().collect();
                        if let ForegroundToolState::Settled { result, .. } = &actual[0].state {
                            results.push(result);
                        }
                        if authoritative_result {
                            for result in results {
                                assert_eq!(result.content.capacity(), 0);
                                assert_eq!(result.artifacts.capacity(), 0);
                                assert_eq!(result.deliveries.capacity(), 0);
                                assert!(result.workflow.is_none());
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn attempt_bindings_reuse_only_identical_position_source_frontier_and_deferred_state() {
    let owners = super::super::prepared_capture::PreparedAttemptBindings::default();
    let binding = ContentBinding {
        source_path: Some(vec!["result".into()]),
        path: vec!["attempt".into(), "foreground".into(), "0".into()],
        content_ref: content_reference(),
        deferred: false,
    };
    let mut original = Vec::new();
    owners.append(vec![binding.clone()], &mut original);
    let bytes = serde_json::to_vec(&original).unwrap();
    let _ = original[0].size();
    checkpoint::metadata_fragment::take_work();
    let mut unchanged = Vec::new();
    owners.clone().append(vec![binding.clone()], &mut unchanged);
    assert!(unchanged[0].same_owner(&original[0]));
    let _ = unchanged[0].size();
    assert_eq!(checkpoint::metadata_fragment::take_work(), 0);
    let mut changes = vec![binding.clone(); 5];
    changes[0].path[2] = "1".into();
    changes[1].content_ref.content_revision += 1;
    changes[2].content_ref.readable_end += 1;
    changes[3].deferred = true;
    changes[4].source_path = None;
    for change in changes {
        let mut updated = Vec::new();
        owners.append(vec![change], &mut updated);
        assert!(!updated[0].same_owner(&original[0]));
        assert_eq!(serde_json::to_vec(&original).unwrap(), bytes);
    }
    owners.append(Vec::new(), &mut Vec::new());
    let mut after_clear = Vec::new();
    owners.append(vec![binding], &mut after_clear);
    assert!(!after_clear[0].same_owner(&original[0]));
    assert_eq!(serde_json::to_vec(&original).unwrap(), bytes);
}

#[test]
fn live_attempt_keeps_32_descriptors_and_reuses_binding_owners_across_unchanged_cuts() {
    use crate::runtime::identity::{PublicationStreamId, RequestId};
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    let start = crate::publication::PublicationStreamStart {
        stream_id: PublicationStreamId::new("stream"),
        attempt_id: attempt(),
        turn_id: TurnId::new("turn"),
        request_id: RequestId::new("request"),
        message_id: MessageId::new("message"),
    };
    projection.publication_start = Some(start.clone());
    let blocks: Vec<_> = (0..35)
        .map(|index| {
            let block_index = ContentBlockIndex::new(index);
            projection.live_frontiers.insert(
                (start.stream_id.clone(), block_index, "utf8_text", "live"),
                ContentRef {
                    conversation_id: projection.snapshot.conversation_id.clone(),
                    source: ContentSource::LivePublicationPrefix {
                        component: PublicationContentComponent::Payload,
                        stream_id: start.stream_id.clone(),
                        message_id: start.message_id.clone(),
                        attempt_id: start.attempt_id.clone(),
                        turn_id: start.turn_id.clone(),
                        publication_request_id: start.request_id.clone(),
                        block_index,
                    },
                    representation: ContentRepresentation::Utf8Text,
                    version: 1,
                    content_revision: 8,
                    readable_end: 25_000,
                },
            );
            InFlightBlock::Text {
                block_index,
                text: "水".repeat(8_000),
            }
        })
        .collect();
    projection.snapshot.attempt.as_mut().unwrap().in_flight = Some(InFlightAssistantMessage {
        message_id: start.message_id.clone(),
        blocks,
    });
    let original = projection.capture_working_seed();
    let mut old_capture = Capture::new(1);
    checkpoint::take_prepared_fit_work();
    let old = original.capture_attempt(&mut old_capture).unwrap();
    assert_eq!(checkpoint::take_prepared_fit_work(), 0);
    let old_bytes = serde_json::to_vec(&old_capture.content).unwrap();
    assert_eq!(old.in_flight.as_ref().unwrap().blocks.len(), 32);
    assert_eq!(old_capture.content.len(), 32);
    assert!(old_capture.content.iter().all(|binding| binding.deferred));
    assert!(
        old_capture
            .collections
            .iter()
            .any(|frontier| frontier.domain == "attempt.in_flight.blocks"
                && frontier.total == 35
                && frontier.loaded == 32
                && frontier.next_offset == Some(32))
    );
    let mut next = original.clone();
    next.cursor = RuntimeClientCursor::new(2);
    let mut repeated = Capture::new(2);
    assert_eq!(next.capture_attempt(&mut repeated).unwrap(), old);
    assert!(
        repeated
            .content
            .iter()
            .zip(&old_capture.content)
            .all(|(current, old)| current.same_owner(old))
    );
    next.live_frontiers
        .get_mut(&(
            start.stream_id.clone(),
            ContentBlockIndex::new(0),
            "utf8_text",
            "live",
        ))
        .unwrap()
        .content_revision += 1;
    let mut changed = Capture::new(3);
    next.capture_attempt(&mut changed);
    assert!(!changed.content[0].same_owner(&old_capture.content[0]));
    assert!(changed.content[1].same_owner(&old_capture.content[1]));
    assert_eq!(serde_json::to_vec(&old_capture.content).unwrap(), old_bytes);
    let mut historical = Capture::new(1);
    assert_eq!(original.capture_attempt(&mut historical).unwrap(), old);
    assert_eq!(serde_json::to_vec(&historical.content).unwrap(), old_bytes);
}

#[test]
fn prepared_tool_results_invalidate_order_and_signed_zero_at_shared_budget_boundaries() {
    use crate::tools::types::ToolResultContent;
    for committed in [false, true] {
        for reorder in [false, true] {
            let mut result = tool_result(0);
            result.content = vec![ToolResultContent::Json {
                value: serde_json::json!({"zero": -0.0, "nested": {"a": 1, "b": 2}}),
            }];
            let mut tool = foreground(
                ForegroundToolState::Settled {
                    arguments: "{}".into(),
                    result: result.clone(),
                },
                false,
            );
            if committed {
                tool.committed_result = Some(result);
            }
            let mut projection = projection();
            projection.ensure_attempt(&attempt());
            projection.snapshot.attempt.as_mut().unwrap().foreground = vec![tool];
            let original = projection.capture_working_seed();
            let mut warm = Capture::new(17);
            let old_bytes = serde_json::to_vec(&original.capture_attempt(&mut warm)).unwrap();
            let old_items = original
                .prepared_attempt
                .select(original.snapshot.attempt.owner());
            let mut changed = original.clone();
            let row = &mut changed.snapshot.attempt.as_mut().unwrap().foreground[0];
            let result = if committed {
                row.committed_result.as_mut().unwrap()
            } else {
                let ForegroundToolState::Settled { result, .. } = &mut row.state else {
                    unreachable!()
                };
                result
            };
            let ToolResultContent::Json { value } = &mut result.content[0] else {
                unreachable!()
            };
            let values = value.as_object_mut().unwrap();
            if reorder {
                let value = values.shift_remove("zero").unwrap();
                values.insert("zero".into(), value);
            } else {
                values.insert("zero".into(), serde_json::json!(0.0));
            }
            let items = changed
                .prepared_attempt
                .select(changed.snapshot.attempt.owner());
            assert!(!Arc::ptr_eq(&old_items.tools[0], &items.tools[0]));
            for pressure in 0..=40 {
                let mut expected = Capture::new(17);
                let mut actual = Capture::new(17);
                for _ in 0..pressure {
                    expected.reserve(&"x".repeat(4000));
                    actual.reserve(&"x".repeat(4000));
                }
                let rows = &changed.snapshot.attempt.as_ref().unwrap().foreground;
                let mut expected_bindings = Vec::new();
                let mut actual_bindings = Vec::new();
                let expected_rows =
                    changed.capture_foreground(&mut expected, rows, &mut expected_bindings);
                let actual_rows = changed.capture_foreground_prepared(
                    &mut actual,
                    rows,
                    &mut actual_bindings,
                    Some(&items),
                );
                assert_eq!(
                    serde_json::to_vec(&actual_rows).unwrap(),
                    serde_json::to_vec(&expected_rows).unwrap()
                );
                assert_eq!(actual.admission_state(), expected.admission_state());
                assert_eq!(actual.collections, expected.collections);
                assert_eq!(actual_bindings, expected_bindings);
            }
            let mut current = Capture::new(17);
            assert_ne!(
                serde_json::to_vec(&changed.capture_attempt(&mut current)).unwrap(),
                old_bytes
            );
            let mut historical = Capture::new(17);
            assert_eq!(
                serde_json::to_vec(&original.capture_attempt(&mut historical)).unwrap(),
                old_bytes
            );
        }
    }
}

#[test]
fn prepared_tool_admission_reads_current_result_sources_with_unchanged_attempt_owner() {
    let tool = foreground(
        ForegroundToolState::Settled {
            arguments: "x".repeat(32_768),
            result: tool_result(32_768),
        },
        true,
    );
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    projection.snapshot.attempt.as_mut().unwrap().foreground = vec![tool.clone()];
    let mut original = projection.capture_working_seed();
    original.foreground_content = vec![super::super::ForegroundResultContent {
        occurrence: tool.occurrence.clone(),
        metadata: content_reference(),
        content: vec![ContentBinding {
            source_path: None,
            path: vec!["tool".into(), "result".into(), "content".into(), "0".into()],
            content_ref: content_reference(),
            deferred: true,
        }],
        frontier: checkpoint::CollectionFrontier {
            domain: "result.content".into(),
            revision: 1,
            total: 9,
            loaded: 1,
            next_offset: Some(1),
        },
    }];
    let mut warm = Capture::new(17);
    let old = original.capture_attempt(&mut warm).unwrap();
    let old_bytes = serde_json::to_vec(&(&old, &warm.content, &warm.collections)).unwrap();
    let mut changed = original.clone();
    changed.foreground_content[0].metadata.content_revision += 1;
    changed.foreground_content[0].metadata.readable_end += 10;
    changed.foreground_content[0].frontier.revision += 1;
    changed.foreground_content[0].content[0]
        .content_ref
        .content_revision += 1;
    assert!(Arc::ptr_eq(
        original.snapshot.attempt.owner(),
        changed.snapshot.attempt.owner()
    ));
    let mut current_capture = Capture::new(17);
    let current = changed.capture_attempt(&mut current_capture).unwrap();
    assert_eq!(
        current.foreground[0].result_metadata_ref.as_ref(),
        Some(&changed.foreground_content[0].metadata)
    );
    let current_bytes = serde_json::to_vec(&(
        &current,
        &current_capture.content,
        &current_capture.collections,
    ))
    .unwrap();
    assert_ne!(current_bytes, old_bytes);
    changed.prepared_attempt = super::super::prepared_attempt::PreparedAttemptItems::default();
    let mut cold_capture = Capture::new(17);
    let cold = changed.capture_attempt(&mut cold_capture).unwrap();
    assert_eq!(
        serde_json::to_vec(&(&cold, &cold_capture.content, &cold_capture.collections)).unwrap(),
        current_bytes
    );
    assert_eq!(
        cold_capture.admission_state(),
        current_capture.admission_state()
    );
    let mut historical_capture = Capture::new(17);
    let historical = original.capture_attempt(&mut historical_capture).unwrap();
    assert_eq!(
        serde_json::to_vec(&(
            &historical,
            &historical_capture.content,
            &historical_capture.collections
        ))
        .unwrap(),
        old_bytes
    );
}
