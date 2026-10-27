use crate::durable::ConversationStore;
use crate::runtime_client::projection::CheckpointSeed;
use crate::runtime_client::{checkpoint, preflight};

fn compare_original(seed: &CheckpointSeed) {
    let (mut original, eviction) = seed.checkpoint_unfitted().unwrap();
    let (mut actual, actual_eviction, budget) = seed.checkpoint_prepared().unwrap();
    assert_eq!(actual_eviction, eviction);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    if let Some(budget) = budget {
        let direct =
            preflight::required_safe_integer_budget(&actual, checkpoint::CHECKPOINT_TARGET_BYTES);
        assert!(
            direct.is_some_and(|direct| direct <= budget),
            "additive certificate is conservative"
        );
    }
    let original_fitted = checkpoint::fit_checkpoint(&mut original).unwrap();
    let actual_fitted = checkpoint::fit_checkpoint_with_budget(&mut actual, budget).unwrap();
    assert_eq!(actual_fitted, original_fitted);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
}

#[test]
fn incremental_checkpoint_reuses_unchanged_domains_without_capture_or_whole_fit() {
    let (mut projection, store) = initialized_source_projection();
    for index in 0..8 {
        let mut message = compactable_user(&format!("prepared-message-{index}"));
        let MessageBlock::User(user) = &mut message else {
            unreachable!()
        };
        user.content = vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "immutable 水\n".repeat(40),
            },
        )];
        let receipt = store.append_canonical(&message).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        });
    }
    let historical = projection.checkpoint_seed();
    let old = historical.checkpoint_candidate().unwrap();
    checkpoint::take_capture_work();
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    let current = projection.checkpoint_seed();
    let work = checkpoint::take_capture_work();
    assert_eq!(work[0], 0, "no immutable domain recapture: {work:?}");
    assert!(work[1] >= 19, "prepared domains replay admission: {work:?}");
    assert_eq!(work[2], 0, "normal workset stays inside certified budgets");
    assert_eq!(work[3], 0, "no complete checkpoint fit walk");
    assert_eq!(
        work[4], 0,
        "ordinary delta never materializes a full checkpoint"
    );
    assert_eq!(
        work[5], 0,
        "prepared charges skip repeated full-event preflight"
    );
    assert!(current.prepared.shared(&historical.prepared, "messages"));
    compare_original(&current);
    assert_eq!(
        historical.checkpoint_candidate().unwrap(),
        old,
        "held cut remains immutable"
    );
}

#[test]
fn incremental_checkpoint_changed_domains_and_cursor_widths_match_original_bytes() {
    let (mut projection, store) = initialized_source_projection();
    for revision in [9, 99, 999, 9_007_199_254_740_990] {
        projection.cursor = RuntimeClientCursor::new(revision);
        projection.initialize_publication_view();
        let before = projection.checkpoint_seed();
        before.checkpoint_candidate().unwrap();
        let receipt = store
            .append_canonical(&compactable_user(&format!("cut-{revision}")))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
            projection.snapshot.context.compaction_count += 1;
            projection.snapshot.model.as_mut().unwrap().unavailable =
                Some(format!("model-{revision}"));
        });
        let seed = projection.checkpoint_seed();
        assert!(!seed.prepared.shared(&before.prepared, "messages"));
        assert!(!seed.prepared.shared(&before.prepared, "context"));
        assert!(!seed.prepared.shared(&before.prepared, "model"));
        compare_original(&seed);
        compare_original(&before);
    }
}

#[test]
fn incremental_checkpoint_budget_pressure_and_unsafe_quantities_match_original() {
    let (projection, _) = initialized_source_projection();
    for size in [0, 16_000, 16_384, 32_768, 100_000] {
        let mut seed = (*projection.checkpoint_seed()).clone();
        seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
        seed.snapshot.model.as_mut().unwrap().unavailable = Some("x".repeat(size));
        seed.read_cut
            .domain_revisions
            .insert("wide".into(), u64::MAX);
        compare_original(&seed);
        assert!(
            seed.checkpoint_prepared().unwrap().2.is_none(),
            "wide quantities keep schema admission"
        );
    }
}

fn transcript_capture_seed() -> CheckpointSeed {
    let (projection, store) = initialized_source_projection();
    let mut seed = (*projection.checkpoint_seed()).clone();
    seed.snapshot.transcript.source_cut = Some(store.observation_read_cut().unwrap());
    seed.snapshot.transcript.statistics =
        Some(crate::runtime_client::response::ConversationStatistics::default());
    seed.snapshot.transcript.entries = (1..=8)
        .map(
            |index| crate::runtime_client::RuntimeClientTranscriptEntry {
                cursor: crate::runtime_client::RuntimeClientTranscriptCursor::new(index),
                item: crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
                    message: compactable_user(&format!("transcript-{index}")),
                },
                turn_process: None,
                response_pending: false,
                completed_response: None,
                tool_calls: Vec::new(),
            },
        )
        .collect();
    seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
    seed
}

fn change_transcript_seed(
    previous: &CheckpointSeed,
    change: impl FnOnce(&mut CheckpointSeed),
) -> CheckpointSeed {
    let mut next = previous.clone();
    change(&mut next);
    next.prepared =
        super::super::prepared_capture::PreparedCaptures::inherit(&next, Some(previous));
    next
}

#[test]
fn incremental_transcript_cut_changes_reuse_rows_and_keep_current_envelope() {
    let original = transcript_capture_seed();
    let retained = original.checkpoint_candidate().unwrap();
    for revision in [9, 99, 999, 9_007_199_254_740_990] {
        let current = change_transcript_seed(&original, |seed| {
            seed.cursor = RuntimeClientCursor::new(revision);
            seed.read_cut.revision = revision;
            let page = &mut seed.snapshot.transcript;
            page.source_cut.as_mut().unwrap().read.journal = revision;
            page.next_cursor = Some(crate::runtime_client::RuntimeClientTranscriptCursor::new(1));
            page.inherited_through =
                Some(crate::runtime_client::RuntimeClientTranscriptCursor::new(1));
            page.collections.push(checkpoint::CollectionFrontier {
                domain: "transcript.native".into(),
                revision,
                total: 8,
                loaded: 8,
                next_offset: None,
            });
        });
        checkpoint::take_capture_work();
        let captured = current.capture_checkpoint().unwrap();
        let work = checkpoint::take_capture_work();
        assert_eq!(
            work[0], 0,
            "cut changes must not normalize any rows: {work:?}"
        );
        assert_eq!(work[2], 0, "ordinary head fits its original admission");
        assert_eq!(work[4], 0, "capture remains borrowed until materialization");
        assert!(
            current
                .prepared
                .shared(&original.prepared, "transcript_entries")
        );
        let page = captured.materialize().snapshot.transcript;
        assert_eq!(page.source_cut, current.snapshot.transcript.source_cut);
        assert_eq!(page.next_cursor, current.snapshot.transcript.next_cursor);
        assert_eq!(page.collections, current.snapshot.transcript.collections);
        compare_original(&current);
    }
    assert_eq!(original.checkpoint_candidate().unwrap(), retained);
}

#[test]
fn incremental_transcript_statistics_and_deferred_rows_have_independent_admission() {
    let original = transcript_capture_seed();
    original.checkpoint_candidate().unwrap();
    let statistics = change_transcript_seed(&original, |seed| {
        seed.snapshot.transcript.statistics.as_mut().unwrap().turns = 17;
    });
    let deferred = change_transcript_seed(&original, |seed| {
        seed.snapshot
            .transcript
            .deferred_entries
            .push(checkpoint::CheckpointPageItem {
                body_complete: None,
                key: None,
                offset: 8,
                identity: "deferred-transcript".into(),
                value: None,
                content: Vec::new(),
                deferred: true,
            });
    });
    for (seed, changed) in [
        (&statistics, "transcript_statistics"),
        (&deferred, "transcript_deferred_entries"),
    ] {
        assert!(
            seed.prepared
                .shared(&original.prepared, "transcript_entries")
        );
        assert!(!seed.prepared.shared(&original.prepared, changed));
        checkpoint::take_capture_work();
        seed.capture_checkpoint().unwrap();
        assert_eq!(
            checkpoint::take_capture_work()[0],
            1,
            "only {changed} rebuilt"
        );
        compare_original(seed);
    }
    let changed = change_transcript_seed(&original, |seed| {
        seed.snapshot.transcript.entries[0].response_pending = true;
    });
    assert!(
        !changed
            .prepared
            .shared(&original.prepared, "transcript_entries")
    );
    compare_original(&changed);
}

#[test]
fn incremental_transcript_split_capture_keeps_shared_budget_pressure_exact() {
    let original = transcript_capture_seed();
    original.checkpoint_candidate().unwrap();
    for (count, text_bytes) in [(0, 0), (4, 12_000), (8, 12_000), (32, 12_000), (32, 16_383)] {
        let seed = change_transcript_seed(&original, |seed| {
            seed.snapshot.messages = (0..count)
                .map(|index| {
                    let mut message = compactable_user(&format!("pressure-{index}"));
                    let MessageBlock::User(user) = &mut message else {
                        unreachable!()
                    };
                    user.content = vec![UserContentBlock::Text(TextBlock {
                        text: "x".repeat(text_bytes),
                    })];
                    message
                })
                .collect();
        });
        assert!(
            seed.prepared
                .shared(&original.prepared, "transcript_entries")
        );
        checkpoint::take_capture_work();
        seed.checkpoint_prepared().unwrap();
        let work = checkpoint::take_capture_work();
        if count == 32 && text_bytes == 12_000 {
            assert!(work[2] > 0, "pressure exercises actual admission: {work:?}");
        }
        compare_original(&seed);
    }
}

#[test]
fn incremental_transcript_row_ownership_keeps_opaque_json_encoding_exact() {
    use crate::message::types::{AssistantContentBlock, AssistantMessageBlock};
    let mut original = transcript_capture_seed();
    let argument = |value| crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
        message: MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("transcript-call"),
            content: vec![AssistantContentBlock::ToolCall(
                crate::tools::types::ToolCall {
                    id: crate::runtime::identity::ToolCallId::new("call"),
                    tool_id: crate::runtime::identity::ToolId::new("tool"),
                    name: "inspect".into(),
                    arguments: value,
                },
            )],
        }),
    };
    for (before, after) in [
        (serde_json::json!(-0.0), serde_json::json!(0.0)),
        (
            serde_json::json!({"alpha": 1, "beta": 2}),
            serde_json::json!({"beta": 2, "alpha": 1}),
        ),
        (
            serde_json::json!([]),
            serde_json::Value::Array(vec![serde_json::Value::Null; 33]),
        ),
        (serde_json::json!(0), serde_json::json!(u64::MAX)),
    ] {
        original.snapshot.transcript.entries[0].item = argument(before);
        original.prepared = super::super::prepared_capture::PreparedCaptures::default();
        original.checkpoint_candidate().unwrap();
        let changed = change_transcript_seed(&original, |seed| {
            seed.snapshot.transcript.entries[0].item = argument(after);
        });
        assert!(
            !changed
                .prepared
                .shared(&original.prepared, "transcript_entries")
        );
        compare_original(&changed);
        compare_original(&original);
    }
}
