use super::super::publication::{event_kind, stamp_event_trace_cut};
use super::super::{
    DomainSources, EncodedPublication, PublicationDraft, encoded_event_bytes,
    set_checkpoint_transition_cursor,
};
use crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
use crate::observation::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
    ContentSource,
};
use crate::observation::cut::ContentBinding;
use crate::runtime_client::checkpoint::{ProjectionMetadata, fit_checkpoint};

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
    draft.seed.snapshot.trace.stamp_cut(&draft.seed.read_cut);
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
        collections: Arc::new(checkpoint.collections.as_ref().clone()),
        content: Arc::new(checkpoint.content.as_ref().clone()),
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
        replacement.content = Arc::default();
        replacement.collections = Arc::default();
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
        .insert(message.id().clone(), vec![evidence].into());
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
        RuntimeClientEvent::TraceChanged,
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
        RuntimeClientEvent::TraceChanged,
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
    let evidence: Vec<_> = (0..400)
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
        .insert(message.id().clone(), evidence.into());
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
        .insert("x".repeat(450_000).into(), 1);
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
        .insert("x".repeat(CHECKPOINT_TARGET_BYTES * 2).into(), 1);
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
    use crate::observation::capture::take_capture_work;
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
            assert!(
                crate::observation::bounded::bounded_structural_len(
                    &actual.event,
                    actual.bytes - 1
                )
                .is_none()
            );
        }
    }
}

#[test]
fn assembled_publication_budget_is_conservative_at_all_admission_boundaries() {
    use super::super::prepared_capture::FitBudget;
    use crate::observation::preflight;
    use crate::runtime_client::checkpoint;
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
        inner.content = Arc::default();
        inner.collections = Arc::default();
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
                    crate::observation::bounded::bounded_structural_len_with_budget(
                        &event,
                        limit,
                        Some(budget)
                    ),
                    crate::observation::bounded::bounded_structural_len(&event, limit),
                    "revision={revision}, replacement={replacement}, limit={limit}",
                );
                assert_eq!(
                    checkpoint::metadata_fragment::transition_length(
                        &event,
                        None,
                        Some(budget),
                        limit,
                        None
                    ),
                    crate::observation::bounded::bounded_structural_len(&event, limit),
                    "fragment exact composition: revision={revision}, replacement={replacement}, limit={limit}",
                );
            }
        }
    }
}

#[test]
fn native_metadata_heads_reuse_fragments_across_publication_and_keep_old_paths() {
    use super::super::prepared_capture::{PreparedBindingHead, take_binding_work};
    use crate::observation::metadata::take_work;
    let mut draft = empty_draft();
    let message = compactable_user("immutable-native-evidence");
    let raw = binding(
        &draft,
        &message,
        vec![
            "display".into(),
            "content".into(),
            "0".into(),
            "text".into(),
        ],
    );
    let head = PreparedBindingHead::from(vec![raw.clone()]);
    draft
        .seed
        .head_bindings
        .insert(message.id().clone(), head.clone());
    draft.seed.snapshot.messages.push(message);
    let transcript_raw = binding(
        &draft,
        &draft.seed.snapshot.messages[0],
        vec!["entries".into(), "0".into(), "text".into()],
    );
    draft.seed.snapshot.transcript.content = vec![transcript_raw.clone()].into();
    draft.seed.transcript_bindings = vec![transcript_raw].into();
    draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
    let first = copy_draft(&draft).encode().unwrap();
    draft.seed = (*first.seed).clone();
    let old_wire = encoded_event_bytes(&first.event, CHECKPOINT_TARGET_BYTES).unwrap();
    take_work();
    take_binding_work();
    crate::runtime_client::checkpoint::metadata_fragment::take_array_work();
    let next = draft.encode().unwrap();
    assert_eq!(
        crate::runtime_client::checkpoint::metadata_fragment::take_array_work(),
        [0, 0, 2],
        "consecutive publication reuses both sorted arrays and aggregate charges"
    );
    assert_eq!(
        take_work(),
        0,
        "unchanged native metadata uses its exact owner certificate"
    );
    assert_eq!(
        take_binding_work(),
        0,
        "delta does not copy or prefix native evidence again"
    );
    assert_eq!(
        next.bytes,
        encoded_event_bytes(&next.event, CHECKPOINT_TARGET_BYTES)
            .unwrap()
            .len()
    );
    assert_eq!(
        encoded_event_bytes(&first.event, CHECKPOINT_TARGET_BYTES).unwrap(),
        old_wire
    );

    let original = head.message(0);
    let original_bytes = serde_json::to_vec(original.as_ref()).unwrap();
    let moved = head.message(7);
    let mut expected = raw;
    expected.path.remove(0);
    let mut path = vec!["messages".into(), "7".into()];
    path.append(&mut expected.path);
    expected.path = path;
    assert_eq!(
        serde_json::to_vec(moved.as_ref()).unwrap(),
        serde_json::to_vec(&vec![expected]).unwrap()
    );
    assert!(!original[0].same_owner(&moved[0]));
    assert!(Arc::ptr_eq(&moved, &head.message(7)));
    assert_eq!(
        serde_json::to_vec(original.as_ref()).unwrap(),
        original_bytes
    );
    assert_eq!(
        encoded_event_bytes(&first.event, CHECKPOINT_TARGET_BYTES).unwrap(),
        old_wire
    );
}

#[test]
fn indexed_metadata_merge_preserves_existing_duplicates_and_first_source_path() {
    use crate::observation::capture::Capture;
    let mut draft = empty_draft();
    let message = compactable_user("metadata-path-priority");
    let raw = binding(
        &draft,
        &message,
        vec!["content".into(), "0".into(), "text".into()],
    );
    let mut second = raw.clone();
    second.path[1] = "1".into();
    second.content_ref.content_revision = 2;
    let mut duplicate = second.clone();
    duplicate.content_ref.content_revision = 3;
    let head =
        super::super::prepared_capture::PreparedBindingHead::from(vec![raw, second, duplicate]);
    let mapped = head.message(0);
    draft.seed.head_bindings.insert(message.id().clone(), head);
    let mut current = Capture::new(7);
    for revision in [90, 91] {
        let mut existing = mapped[0].clone().into_owned();
        existing.content_ref.content_revision = revision;
        current.content.push(existing.into());
    }
    let initial = current.content.clone();
    draft.seed.capture_evidence(&[message], &mut current);
    assert_eq!(current.content.len(), 3);
    assert!(current.content[0].same_owner(&initial[0]));
    assert!(current.content[1].same_owner(&initial[1]));
    assert!(current.content[2].same_owner(&mapped[1]));
    assert_eq!(current.content[2].content_ref.content_revision, 2);
}

#[test]
fn merged_frontier_pass_preserves_cut_and_duplicate_override_precedence() {
    use crate::observation::capture::Capture;
    use crate::observation::cut::CollectionFrontier;
    use crate::runtime::identity::ObservationCollectionCut;
    let mut draft = empty_draft();
    let seed = &mut draft.seed;
    seed.live_block_total = 11;
    seed.live_tool_total = 12;
    seed.canonical_foreground = None;
    seed.source_frontiers.clear();
    seed.collection_cuts.clear();
    seed.snapshot.pending_interactions.clear();
    seed.domain_revisions
        .insert("pending_interactions".into(), 8);
    for (domain, total) in [
        ("pending_primary", 3),
        ("pending_children", 4),
        ("pending_interactions", 17),
    ] {
        seed.collection_cuts.insert(
            domain.into(),
            ObservationCollectionCut {
                generation: 1,
                total,
            },
        );
    }
    let frontier = |domain: &str| CollectionFrontier {
        domain: domain.into(),
        revision: 0,
        total: 9,
        loaded: 1,
        next_offset: Some(7),
    };
    let mut original = frontier("ordinary");
    original.total = 100;
    original.loaded = 5;
    seed.source_frontiers.insert("ordinary".into(), original);
    let mut capture = Capture::new(7);
    capture.collections = [
        "pending_interactions",
        "pending_interactions",
        "ordinary",
        "ordinary",
        "attempt.in_flight.blocks",
        "attempt.foreground",
        "messages",
        "inbound.pending",
        "transcript.entries",
    ]
    .into_iter()
    .map(frontier)
    .collect();
    let cut = seed.source_cut.as_mut().expect("initialized native cut");
    cut.ledger_through = 31;
    cut.read.pending_total = 32;
    cut.transcript_total = 33;
    seed.capture_evidence(&[], &mut capture);
    assert_eq!(
        capture
            .collections
            .iter()
            .map(|value| value.total)
            .collect::<Vec<_>>(),
        [7, 17, 100, 9, 11, 12, 31, 32, 33]
    );
    assert_eq!(
        capture
            .collections
            .iter()
            .map(|value| value.next_offset)
            .collect::<Vec<_>>(),
        [
            Some(0),
            Some(1),
            Some(0),
            Some(7),
            Some(1),
            Some(1),
            Some(0),
            Some(0),
            Some(0)
        ]
    );
    assert_eq!(capture.collections[0].revision, 8);
    assert_eq!(capture.collections[1].revision, 8);
}

#[test]
fn cached_raw_evidence_merge_rechecks_current_resolution_and_preserves_old_cut() {
    use crate::observation::capture::Capture;
    use crate::runtime_client::checkpoint::metadata_fragment::take_evidence_work;
    let mut draft = empty_draft();
    let message = compactable_user("raw-cache-authority");
    let raw = binding(
        &draft,
        &message,
        vec!["content".into(), "0".into(), "text".into()],
    );
    let mut reference = raw.content_ref.clone();
    reference.content_revision = 10;
    let ContentSource::CanonicalMessageBlock {
        component,
        locator,
        block_index,
    } = &reference.source
    else {
        unreachable!()
    };
    let key = (
        component.key(),
        locator.key().into_owned(),
        block_index.get(),
        reference.representation.key(),
    );
    draft
        .seed
        .head_bindings
        .insert(message.id().clone(), vec![raw].into());
    draft
        .seed
        .content_metadata
        .insert(key.clone(), Arc::new(reference.clone()));
    let old = draft.seed.clone();
    let mut first = Capture::new(7);
    take_evidence_work();
    old.capture_evidence(std::slice::from_ref(&message), &mut first);
    assert_eq!(take_evidence_work(), 1);
    assert_eq!(first.content[0].content_ref.content_revision, 10);
    let first_wire = serde_json::to_vec(&first.content).unwrap();
    reference.content_revision = 11;
    reference.readable_end += 1;
    draft.seed.content_metadata.insert(key, Arc::new(reference));
    let mut second = Capture::new(8);
    draft
        .seed
        .capture_evidence(std::slice::from_ref(&message), &mut second);
    assert_eq!(
        take_evidence_work(),
        0,
        "raw merge reuses ordered owners, not resolved authority"
    );
    assert_eq!(second.content[0].content_ref.content_revision, 11);
    assert_eq!(serde_json::to_vec(&first.content).unwrap(), first_wire);
    let mut historical = Capture::new(7);
    old.capture_evidence(&[message], &mut historical);
    assert_eq!(take_evidence_work(), 0);
    assert_eq!(serde_json::to_vec(&historical.content).unwrap(), first_wire);
}

#[test]
fn content_resolution_keeps_argument_precedence_and_missing_representation_semantics() {
    let mut draft = empty_draft();
    let message = compactable_user("content-resolution-precedence");
    for path in [vec![], vec!["text".into()], vec!["arguments".into()]] {
        for representation in [
            ContentRepresentation::Utf8Text,
            ContentRepresentation::RawJsonUtf8,
        ] {
            for available in [0_u8, 1, 2, 3] {
                draft.seed.content_metadata.clear();
                let mut original = binding(&draft, &message, path.clone());
                original.content_ref.representation = representation;
                let ContentSource::CanonicalMessageBlock {
                    component,
                    locator,
                    block_index,
                } = &original.content_ref.source
                else {
                    unreachable!()
                };
                for (bit, kind, revision) in [
                    (1, ContentRepresentation::Utf8Text, 10),
                    (2, ContentRepresentation::RawJsonUtf8, 20),
                ] {
                    if available & bit == 0 {
                        continue;
                    }
                    let mut reference = original.content_ref.clone();
                    reference.representation = kind;
                    reference.content_revision = revision;
                    draft.seed.content_metadata.insert(
                        (
                            component.key(),
                            locator.key().into_owned(),
                            block_index.get(),
                            kind.key(),
                        ),
                        Arc::new(reference),
                    );
                }
                let expected = if available & 2 != 0
                    && (path.last().is_some_and(|field| field == "arguments")
                        || representation == ContentRepresentation::RawJsonUtf8)
                {
                    20
                } else if available & 1 != 0 && representation == ContentRepresentation::Utf8Text {
                    10
                } else {
                    1
                };
                let historical = original.clone();
                let mut bindings = vec![original.into()];
                draft.seed.resolve_captured_content(&mut bindings);
                assert_eq!(bindings[0].content_ref.content_revision, expected);
                assert_eq!(historical.content_ref.content_revision, 1);
            }
        }
    }
}

#[test]
fn repeated_publication_metadata_owner_census() {
    use crate::observation::metadata::MetadataFragment;
    use std::collections::BTreeMap;

    fn inventory<T>(arrays: impl IntoIterator<Item = (usize, usize, usize)>) -> (usize, usize, usize) {
        let owners: BTreeMap<_, _> = arrays
            .into_iter()
            .filter(|(_, len, _)| *len != 0)
            .map(|(ptr, len, capacity)| (ptr, (len, capacity)))
            .collect();
        let used = owners.values().map(|(len, _)| len).sum();
        let reserved = owners.values().map(|(_, capacity)| capacity).sum::<usize>();
        (
            owners.len(),
            used,
            reserved * std::mem::size_of::<MetadataFragment<T>>(),
        )
    }

    let mut draft = empty_draft();
    add_owned_metadata(&mut draft);
    let mut held = Vec::new();
    let mut encoded_charge = 0;
    for _ in 0..8 {
        let publication = copy_draft(&draft).encode().unwrap();
        draft.seed = (*publication.seed).clone();
        encoded_charge += publication.bytes;
        held.push(publication.event);
    }
    let read = draft.seed.checkpoint_candidate().unwrap();
    let transitions: Vec<_> = held
        .iter()
        .map(|event| match event {
            RuntimeClientEvent::ProjectionTransition { metadata, .. } => metadata,
            _ => panic!("one metadata transition per publication"),
        })
        .collect();
    let content = inventory::<ContentBinding>(
        transitions
            .iter()
            .map(|m| (m.content.as_ptr() as usize, m.content.len(), m.content.capacity()))
            .chain([(read.content.as_ptr() as usize, read.content.len(), read.content.capacity())]),
    );
    let collections = inventory::<crate::observation::cut::CollectionFrontier>(
        transitions
            .iter()
            .map(|m| (m.collections.as_ptr() as usize, m.collections.len(), m.collections.capacity()))
            .chain([(
                read.collections.as_ptr() as usize,
                read.collections.len(),
                read.collections.capacity(),
            )]),
    );
    assert!(transitions.iter().all(|m| m.content == read.content && m.collections == read.collections));
    assert!(transitions.windows(2).all(|pair| pair[0].read_cut != pair[1].read_cut));
    assert_eq!(content.0, 1);
    assert_eq!(collections.0, 1);
    assert!(transitions.iter().all(|m| Arc::ptr_eq(&m.content, &read.content) && Arc::ptr_eq(&m.collections, &read.collections)));
    eprintln!("8 retained transitions + read, (owners, slots, reserved slot bytes): content={content:?}, collections={collections:?}, encoded replay charge={encoded_charge}");
}

#[test]
fn subscriber_delivery_and_eviction_preserve_held_metadata_owners() {
    use super::super::SubscriberPoll;

    let (mut projection, _store) = initialized_source_projection();
    projection.replay_limit = 2;
    let mut draft = empty_draft();
    add_owned_metadata(&mut draft);

    let first = copy_draft(&draft).encode().unwrap();
    draft.seed = (*first.seed).clone();
    projection.cursor = first.seed.cursor;
    projection.retain_replay(first.event, first.bytes);
    drop(first.seed);
    let (caught_up, _) = projection.subscribe(projection.earliest_serviceable()).unwrap();
    let (slow, _) = projection.subscribe(projection.earliest_serviceable()).unwrap();
    let SubscriberPoll::Event(held) = projection.poll_subscriber(caught_up) else {
        panic!("retained first transition");
    };
    let RuntimeClientEvent::ProjectionTransition { metadata: old, .. } = held.event.as_ref() else {
        unreachable!()
    };
    let old_bytes = encoded_event_bytes(&held.event, CHECKPOINT_TARGET_BYTES).unwrap();
    let released = Arc::downgrade(&old.content);
    let payload = old.content[0].owner_probe();
    let second = copy_draft(&draft).encode().unwrap();
    draft.seed = (*second.seed).clone();
    projection.cursor = second.seed.cursor;
    projection.retain_replay(second.event, second.bytes);
    drop(second.seed);
    let SubscriberPoll::Event(second) = projection.poll_subscriber(caught_up) else {
        panic!("caught-up subscriber receives the next transition");
    };
    let RuntimeClientEvent::ProjectionTransition { metadata: current, .. } = second.event.as_ref() else {
        unreachable!()
    };
    assert!(Arc::ptr_eq(&old.content, &current.content));
    assert!(Arc::ptr_eq(&old.collections, &current.collections));
    assert_ne!(old.read_cut, current.read_cut);

    draft.seed.snapshot.messages.clear();
    draft.seed.head_bindings.clear();
    draft.seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
    for _ in 0..2 {
        let next = copy_draft(&draft).encode().unwrap();
        draft.seed = (*next.seed).clone();
        projection.cursor = next.seed.cursor;
        projection.retain_replay(next.event, next.bytes);
        assert!(matches!(projection.poll_subscriber(caught_up), SubscriberPoll::Event(_)));
    }
    assert!(matches!(projection.poll_subscriber(slow), SubscriberPoll::Lagged { .. }));
    assert_eq!(projection.replay_len(), 2);
    assert_eq!(encoded_event_bytes(&held.event, CHECKPOINT_TARGET_BYTES).unwrap(), old_bytes);
    assert_ne!(released.strong_count(), 0);
    drop((held, second));
    assert_eq!(released.strong_count(), 0);
    drop(draft);
    assert_eq!(payload(), 0, "the independent raw evidence cache has also been released");
}
