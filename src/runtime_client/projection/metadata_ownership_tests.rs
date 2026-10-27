use super::super::{
    DomainSources, EncodedPublication, PublicationDraft, encoded_event_bytes, event_kind,
    set_checkpoint_transition_cursor, stamp_event_trace_cut, stamp_trace_cut,
};
use crate::runtime_client::checkpoint::{
    CHECKPOINT_TARGET_BYTES, ContentBinding, ProjectionMetadata, fit_checkpoint,
};
use crate::runtime_client::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
    ContentSource,
};

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // Independent branch witnesses can overlap.
struct BaselinePath {
    captured_eviction: bool,
    fitted: bool,
    forced: bool,
    budget_fallback: bool,
}

// Frozen 623b41b encoding algorithm, excluding test-only elapsed-time counters.
// Keep its reserved metadata clone independent of the ownership candidate.
fn baseline_encode(
    mut draft: PublicationDraft,
    path: &mut BaselinePath,
) -> Result<EncodedPublication, RuntimeClientError> {
    let next = draft
        .seed
        .cursor
        .get()
        .checked_add(1)
        .ok_or(RuntimeClientError::ProjectionExhausted)?;
    draft.seed.cursor = RuntimeClientCursor::new(next);
    draft.seed.read_cut.revision = next;
    if let Some(cut) = &mut draft.seed.read_domains {
        cut.revision = next;
    }
    stamp_trace_cut(&mut draft.seed.snapshot.trace, &draft.seed.read_cut);
    for event in &mut draft.events {
        stamp_event_trace_cut(event, &draft.seed.read_cut);
    }
    let (mut checkpoint, captured_eviction) = draft.seed.checkpoint_unfitted()?;
    path.captured_eviction = captured_eviction;
    let fitted = fit_checkpoint(&mut checkpoint)?;
    path.fitted = fitted;
    path.forced = draft.replacement;
    let metadata = ProjectionMetadata {
        read_cut: checkpoint.read_cut.clone(),
        collections: checkpoint.collections.clone(),
        content: checkpoint.content.clone(),
    };
    let mut event = RuntimeClientEvent::ProjectionTransition {
        events: draft.events,
        metadata: metadata.clone(),
    };
    set_checkpoint_transition_cursor(&mut event, draft.seed.cursor);
    let encoded = if fitted || captured_eviction || draft.replacement {
        None
    } else {
        encoded_event_bytes(&event, CHECKPOINT_TARGET_BYTES)
    };
    path.budget_fallback = !fitted && !captured_eviction && !draft.replacement && encoded.is_none();
    if encoded.is_none() {
        let transition = event_kind(&event).to_owned();
        let mut replacement = checkpoint;
        replacement.content.clear();
        replacement.collections.clear();
        event = RuntimeClientEvent::ProjectionTransition {
            events: vec![RuntimeClientEvent::CheckpointAdvanced {
                transition,
                checkpoint: Box::new(replacement),
            }],
            metadata,
        };
    }
    let encoded = encoded
        .or_else(|| encoded_event_bytes(&event, CHECKPOINT_TARGET_BYTES))
        .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
            message: "finite display transition exceeds its encoded budget".into(),
        })?;
    Ok(EncodedPublication {
        seed: Arc::new(draft.seed),
        event,
        bytes: encoded.len(),
        sources: draft.sources,
    })
}

fn copy_draft(draft: &PublicationDraft) -> PublicationDraft {
    PublicationDraft {
        encoding_profile: None,
        seed: draft.seed.clone(),
        events: draft.events.clone(),
        replacement: draft.replacement,
        sources: draft.sources.clone(),
    }
}

fn empty_draft() -> PublicationDraft {
    let (projection, _store) = initialized_source_projection();
    PublicationDraft {
        encoding_profile: None,
        seed: (*projection.checkpoint_seed()).clone(),
        events: vec![RuntimeClientEvent::TraceChanged],
        replacement: false,
        sources: DomainSources::new(),
    }
}

fn binding(draft: &PublicationDraft, message: &MessageBlock, path: Vec<String>) -> ContentBinding {
    ContentBinding {
        source_path: Some(vec!["text".into()]),
        path,
        content_ref: ContentRef {
            conversation_id: draft.seed.snapshot.conversation_id.clone(),
            source: ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::Payload,
                locator: CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                block_index: ContentBlockIndex::new(0),
            },
            representation: ContentRepresentation::Utf8Text,
            version: 1,
            content_revision: 1,
            readable_end: 7,
        },
        deferred: true,
    }
}

fn add_owned_metadata(draft: &mut PublicationDraft) {
    draft
        .seed
        .read_cut
        .domain_revisions
        .insert("metadata-ownership-wide-native".into(), u64::MAX);
    let message = compactable_user("metadata-ownership-message");
    let evidence = binding(
        draft,
        &message,
        vec!["content".into(), "0".into(), "text".into()],
    );
    draft
        .seed
        .head_bindings
        .insert(message.id().clone(), vec![evidence]);
    draft.seed.snapshot.messages.push(message);
    draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
}

fn traced_draft() -> PublicationDraft {
    let (mut projection, store, _, _) = native_running_tool_fixture();
    let fence = projection.read_domain_cut().unwrap();
    let read = fence.read(&store).unwrap();
    publish_source_fact(&mut projection, |projection| {
        assert!(projection.install_read_domains(&fence, Ok(read), true));
    });
    let mut draft = PublicationDraft {
        encoding_profile: None,
        seed: (*projection.checkpoint_seed()).clone(),
        events: Vec::new(),
        replacement: false,
        sources: projection.domain_sources.clone(),
    };
    assert!(!draft.seed.snapshot.trace.records.is_empty());
    draft.seed.snapshot.trace.read_cut = None;
    add_owned_metadata(&mut draft);
    let mut nested = draft.seed.checkpoint_candidate().unwrap();
    nested.cursor = RuntimeClientCursor::new(0);
    nested.read_cut.revision = 0;
    nested.snapshot.trace.read_cut = None;
    draft.events = vec![
        RuntimeClientEvent::TraceChanged,
        RuntimeClientEvent::CheckpointAdvanced {
            transition: "nested-native-cut".into(),
            checkpoint: Box::new(nested),
        },
        RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged {
            eligibility:
                crate::local_runtime::configuration::application::AdoptionEligibility::Eligible,
        },
    ];
    draft
}

fn equivalent_success(mut draft: PublicationDraft) -> (EncodedPublication, BaselinePath) {
    // These fixtures directly mutate the frozen private seed after creation.
    // Production seeds are immutable; give the injected state fresh owners.
    draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
    let mut path = BaselinePath::default();
    let expected = baseline_encode(copy_draft(&draft), &mut path)
        .unwrap_or_else(|error| panic!("baseline {path:?}: {error:?}"));
    let actual = draft
        .encode()
        .unwrap_or_else(|error| panic!("candidate: {error:?}"));
    assert_eq!(actual.event, expected.event);
    let expected_bytes = encoded_event_bytes(&expected.event, CHECKPOINT_TARGET_BYTES).unwrap();
    let actual_bytes = encoded_event_bytes(&actual.event, CHECKPOINT_TARGET_BYTES).unwrap();
    assert_eq!(actual_bytes, expected_bytes, "full structural wire bytes");
    assert_eq!(actual.bytes, expected.bytes);
    assert_eq!(actual.bytes, actual_bytes.len());
    assert_eq!(actual.seed.cursor, expected.seed.cursor);
    assert_eq!(actual.seed.read_cut, expected.seed.read_cut);
    assert_eq!(actual.seed.read_domains, expected.seed.read_domains);
    assert_eq!(actual.seed.snapshot, expected.seed.snapshot);
    assert_eq!(actual.sources.len(), expected.sources.len());
    for (
        (key, (source, size, revision)),
        (other_key, (other_source, other_size, other_revision)),
    ) in actual.sources.iter().zip(expected.sources.iter())
    {
        assert_eq!(
            (key, size, revision),
            (other_key, other_size, other_revision)
        );
        assert!(
            Arc::ptr_eq(source, other_source),
            "native Source owners stay identical"
        );
    }
    let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &actual.event else {
        panic!("one outer transition");
    };
    assert_eq!(metadata.read_cut, actual.seed.read_cut);
    assert_eq!(metadata.read_cut.revision, actual.seed.cursor.get());
    eprintln!(
        "ownership equivalence {path:?}: {} encoded bytes",
        actual.bytes
    );
    (actual, path)
}

fn replacement(
    output: &EncodedPublication,
) -> &crate::runtime_client::checkpoint::RuntimeClientCheckpoint {
    let RuntimeClientEvent::ProjectionTransition { events, metadata } = &output.event else {
        panic!("outer transition");
    };
    let [
        RuntimeClientEvent::CheckpointAdvanced {
            transition,
            checkpoint,
        },
    ] = events.as_slice()
    else {
        panic!("one replacement preserves event ordering");
    };
    assert_eq!(transition, "projection_transition");
    assert!(checkpoint.collections.is_empty());
    assert!(checkpoint.content.is_empty());
    assert!(
        !metadata.collections.is_empty(),
        "outer metadata survives inner clearing"
    );
    assert_eq!(checkpoint.cursor, output.seed.cursor);
    assert_eq!(checkpoint.read_cut, metadata.read_cut);
    checkpoint
}

fn equivalent_error(draft: PublicationDraft) -> RuntimeClientError {
    let mut path = BaselinePath::default();
    let expected = baseline_encode(copy_draft(&draft), &mut path)
        .err()
        .expect("baseline error");
    let actual = draft.encode().err().expect("candidate error");
    assert_eq!(actual, expected, "exact existing error and message");
    actual
}

#[test]
fn normal_encoding_preserves_wire_metadata_trace_cursors_and_event_order() {
    let draft = traced_draft();
    let next = draft.seed.cursor.get() + 1;
    let RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } = &draft.events[1] else {
        panic!("fixture contains a nested checkpoint");
    };
    // Nested Trace stamping precedes the legacy checkpoint cursor rewrite.
    let nested_trace_cut = checkpoint.read_cut.clone();
    assert_ne!(nested_trace_cut.revision, next);
    let (output, path) = equivalent_success(draft);
    assert!(!path.captured_eviction && !path.fitted && !path.forced && !path.budget_fallback);
    let RuntimeClientEvent::ProjectionTransition { events, metadata } = &output.event else {
        unreachable!()
    };
    assert!(!metadata.content.is_empty());
    let [
        RuntimeClientEvent::TraceChanged,
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. },
        RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged { .. },
    ] = events.as_slice()
    else {
        panic!("original three facts retain their order");
    };
    assert_eq!(checkpoint.cursor.get(), next);
    assert_eq!(checkpoint.read_cut.revision, next);
    assert!(!checkpoint.snapshot.trace.records.is_empty());
    assert_eq!(
        checkpoint.snapshot.trace.read_cut.as_ref(),
        Some(&nested_trace_cut)
    );
    assert_eq!(
        output.seed.snapshot.trace.read_cut.as_ref(),
        Some(&metadata.read_cut)
    );
}

#[test]
fn forced_replacement_preserves_owned_metadata_when_inner_fields_are_cleared() {
    let mut draft = traced_draft();
    draft.replacement = true;
    let (output, path) = equivalent_success(draft);
    assert!(path.forced && !path.captured_eviction && !path.fitted && !path.budget_fallback);
    assert!(!replacement(&output).snapshot.trace.records.is_empty());
    let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &output.event else {
        unreachable!()
    };
    assert!(
        !metadata.content.is_empty(),
        "checkpoint descriptors survive transfer to outer metadata"
    );
}

#[test]
fn captured_eviction_matches_baseline_replacement() {
    let mut draft = empty_draft();
    add_owned_metadata(&mut draft);
    draft.seed.snapshot.model.as_mut().unwrap().unavailable = Some("x".repeat(32_768));
    let (output, path) = equivalent_success(draft);
    assert!(path.captured_eviction && !path.fitted && !path.forced && !path.budget_fallback);
    assert!(replacement(&output).snapshot.model.is_none());
}

#[test]
fn fitting_evicts_loaded_messages_and_keeps_the_same_finite_fallback() {
    let mut draft = empty_draft();
    add_owned_metadata(&mut draft);
    let message = draft.seed.snapshot.messages[0].clone();
    let evidence = (0..400)
        .map(|index| {
            binding(
                &draft,
                &message,
                vec!["content".into(), index.to_string(), "x".repeat(2_000)],
            )
        })
        .collect();
    draft
        .seed
        .head_bindings
        .insert(message.id().clone(), evidence);
    let (output, path) = equivalent_success(draft);
    assert!(path.fitted && !path.captured_eviction && !path.forced && !path.budget_fallback);
    assert!(replacement(&output).snapshot.messages.is_empty());
    let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &output.event else {
        unreachable!()
    };
    assert!(metadata.content.is_empty());
    let messages = metadata
        .collections
        .iter()
        .find(|frontier| frontier.domain == "messages")
        .unwrap();
    assert_eq!((messages.loaded, messages.next_offset), (0, Some(0)));
}

#[test]
fn provisional_encoding_budget_triggers_the_same_replacement() {
    let mut draft = traced_draft();
    let RuntimeClientEvent::CheckpointAdvanced { transition, .. } = &mut draft.events[1] else {
        unreachable!()
    };
    *transition = "overflow".repeat(CHECKPOINT_TARGET_BYTES / 4);
    let (output, path) = equivalent_success(draft);
    assert!(path.budget_fallback && !path.captured_eviction && !path.fitted && !path.forced);
    replacement(&output);
}

#[test]
fn final_transition_budget_preserves_the_existing_failure() {
    let mut draft = empty_draft();
    draft
        .seed
        .read_cut
        .domain_revisions
        .insert("x".repeat(450_000), 1);
    draft.replacement = true;
    assert_eq!(
        equivalent_error(draft),
        RuntimeClientError::ObservationUnavailable {
            message: "finite display transition exceeds its encoded budget".into(),
        }
    );
}

#[test]
fn authoritative_checkpoint_budget_preserves_the_existing_failure() {
    let mut draft = empty_draft();
    draft
        .seed
        .read_cut
        .domain_revisions
        .insert("x".repeat(CHECKPOINT_TARGET_BYTES * 2), 1);
    assert_eq!(
        equivalent_error(draft),
        RuntimeClientError::ObservationUnavailable {
            message:
                "finite authoritative checkpoint descriptors exceed the native checkpoint budget"
                    .into(),
        }
    );
}

#[test]
fn existing_cursor_and_journal_failures_remain_identical() {
    let mut draft = empty_draft();
    draft.seed.cursor = RuntimeClientCursor::new(u64::MAX);
    assert_eq!(
        equivalent_error(draft),
        RuntimeClientError::ProjectionExhausted
    );
    let mut draft = empty_draft();
    draft.seed.journal_failure = true;
    assert_eq!(
        equivalent_error(draft),
        RuntimeClientError::ObservationUnavailable {
            message: "native Journal observation source unavailable".into(),
        }
    );
}

#[test]
fn metadata_first_publication_skips_snapshot_copies_and_repeated_preflight() {
    use crate::runtime_client::checkpoint::{self, take_capture_work};
    for forced in [false, true] {
        let mut draft = empty_draft();
        draft.replacement = forced;
        for index in 0..8 {
            let mut message = compactable_user(&format!("borrowed-head-{index}"));
            let MessageBlock::User(user) = &mut message else {
                unreachable!()
            };
            user.content = vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "unchanged 水\"\n".repeat(300),
                },
            )];
            draft.seed.snapshot.messages.push(message);
        }
        draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
        for warm in [false, true] {
            let input = copy_draft(&draft);
            let mut path = BaselinePath::default();
            let expected = baseline_encode(copy_draft(&input), &mut path).unwrap();
            take_capture_work();
            let actual = input.encode().unwrap();
            let work = take_capture_work();
            assert!(!path.captured_eviction && !path.fitted && !path.budget_fallback);
            assert_eq!(
                work[4],
                u64::from(forced),
                "only required replacements materialize, warm={warm}: {work:?}"
            );
            assert_eq!(
                work[5], 0,
                "composed admission skips full-event preflight, warm={warm}: {work:?}"
            );
            if warm {
                assert_eq!(
                    work[0], 0,
                    "warm immutable domains require no normalization"
                );
            }
            assert_eq!(actual.event, expected.event);
            assert_eq!(actual.bytes, expected.bytes);
            assert_eq!(
                encoded_event_bytes(&actual.event, CHECKPOINT_TARGET_BYTES),
                encoded_event_bytes(&expected.event, CHECKPOINT_TARGET_BYTES)
            );
            let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &actual.event else {
                unreachable!()
            };
            assert!(
                metadata
                    .collections
                    .windows(2)
                    .all(|rows| rows[0].domain <= rows[1].domain)
            );
            assert!(
                metadata
                    .content
                    .windows(2)
                    .all(|rows| rows[0].path <= rows[1].path)
            );
            assert!(checkpoint::bounded_structural_len(&actual.event, actual.bytes - 1).is_none());
        }
    }
}

#[test]
fn assembled_publication_budget_is_conservative_at_all_admission_boundaries() {
    use super::super::prepared_capture::FitBudget;
    use crate::runtime_client::{checkpoint, preflight};
    let mut draft = empty_draft();
    draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
    for revision in [9, 99, 999, 9_007_199_254_740_990] {
        draft.seed.cursor = RuntimeClientCursor::new(revision);
        draft.seed.read_cut.revision = revision;
        let captured = draft.seed.capture_checkpoint().unwrap();
        let metadata_budget = captured.metadata_budget;
        let checkpoint_budget = captured.fit_budget;
        let owned = captured.materialize();
        let metadata = ProjectionMetadata {
            read_cut: owned.read_cut.clone(),
            collections: owned.collections.clone(),
            content: owned.content.clone(),
        };
        let mut inner = owned;
        inner.content.clear();
        inner.collections.clear();
        for replacement in [false, true] {
            let events = if replacement {
                vec![RuntimeClientEvent::CheckpointAdvanced {
                    transition: "projection_transition".into(),
                    checkpoint: Box::new(inner.clone()),
                }]
            } else {
                vec![RuntimeClientEvent::TraceChanged]
            };
            let budget = if replacement {
                FitBudget::replacement(checkpoint_budget, metadata_budget)
            } else {
                FitBudget::transition(
                    preflight::required_safe_integer_budget(&events, CHECKPOINT_TARGET_BYTES),
                    metadata_budget,
                )
            }
            .expect("ordinary native values produce a sufficient charge");
            let event = RuntimeClientEvent::ProjectionTransition {
                events,
                metadata: metadata.clone(),
            };
            let direct =
                preflight::required_safe_integer_budget(&event, CHECKPOINT_TARGET_BYTES).unwrap();
            assert!(
                direct <= budget,
                "composed charge must dominate the exact admission walk"
            );
            for limit in [
                0,
                direct - 1,
                direct,
                budget - 1,
                budget,
                budget + 1,
                CHECKPOINT_TARGET_BYTES,
            ] {
                assert_eq!(
                    checkpoint::bounded_structural_len_with_budget(&event, limit, Some(budget)),
                    checkpoint::bounded_structural_len(&event, limit),
                    "revision={revision}, replacement={replacement}, limit={limit}",
                );
            }
        }
    }
}
