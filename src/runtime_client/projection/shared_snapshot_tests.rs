use crate::runtime_client::checkpoint;
use crate::runtime_client::projection::CheckpointSeed;

fn assert_shared_checkpoint_bytes(seed: &CheckpointSeed) {
    let (mut expected, _) = seed.checkpoint_unfitted().unwrap();
    let (mut actual, _, budget) = seed.checkpoint_prepared().unwrap();
    checkpoint::fit_checkpoint(&mut expected).unwrap();
    checkpoint::fit_checkpoint_with_budget(&mut actual, budget).unwrap();
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap(),
        "shared-domain capture preserves the original admission and wire bytes"
    );
}

#[test]
fn shared_snapshot_reuses_opaque_model_without_comparison_or_normalization() {
    let (mut projection, _) = initialized_source_projection();
    let params = &mut projection
        .snapshot
        .model
        .as_mut()
        .unwrap()
        .configured
        .request_params;
    params.insert(
        "ordered".into(),
        serde_json::from_str(r#"{"first":0.0,"second":2}"#).unwrap(),
    );
    projection.initialize_publication_view();
    let previous = projection.checkpoint_seed();
    let original = previous.checkpoint_candidate().unwrap();
    checkpoint::take_capture_work();
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    let current = projection.checkpoint_seed();
    assert!(current.snapshot.model.same_owner(&previous.snapshot.model));
    assert!(current.prepared.shared(&previous.prepared, "model"));
    assert_eq!(
        checkpoint::take_capture_work()[0],
        0,
        "unchanged maps and floats do not recapture"
    );
    assert_shared_checkpoint_bytes(&current);
    assert_eq!(previous.checkpoint_candidate().unwrap(), original);
}

#[test]
fn shared_snapshot_tracks_signed_zero_and_map_order_without_domain_revision_changes() {
    let (mut projection, _) = initialized_source_projection();
    projection
        .snapshot
        .model
        .as_mut()
        .unwrap()
        .configured
        .request_params
        .insert(
            "ordered".into(),
            serde_json::from_str(r#"{"first":0.0,"second":2}"#).unwrap(),
        );
    projection.initialize_publication_view();
    for changed in [
        r#"{"first":-0.0,"second":2}"#,
        r#"{"second":2,"first":-0.0}"#,
    ] {
        let previous = projection.checkpoint_seed();
        let before = previous.checkpoint_candidate().unwrap();
        let native = previous.snapshot.model.owned();
        publish_source_fact(&mut projection, |projection| {
            projection
                .snapshot
                .model
                .as_mut()
                .unwrap()
                .configured
                .request_params
                .insert("ordered".into(), serde_json::from_str(changed).unwrap());
            projection.publish(RuntimeClientEvent::TraceChanged);
        });
        let current = projection.checkpoint_seed();
        assert_eq!(
            native,
            current.snapshot.model.owned(),
            "PartialEq deliberately misses this wire change"
        );
        assert_eq!(current.domain_revisions, previous.domain_revisions);
        assert!(!current.snapshot.model.same_owner(&previous.snapshot.model));
        assert!(!current.prepared.shared(&previous.prepared, "model"));
        assert!(
            current
                .snapshot
                .messages
                .same_owner(&previous.snapshot.messages)
        );
        assert!(current.snapshot.jobs.same_owner(&previous.snapshot.jobs));
        assert_ne!(
            serde_json::to_vec(&current.snapshot.model).unwrap(),
            serde_json::to_vec(&previous.snapshot.model).unwrap(),
        );
        assert_shared_checkpoint_bytes(&current);
        assert_eq!(
            previous.checkpoint_candidate().unwrap(),
            before,
            "issued cut stays frozen"
        );
    }
}

#[test]
fn shared_snapshot_copies_only_mutated_domain_and_keeps_envelope_owners() {
    let (mut projection, _) = initialized_source_projection();
    projection
        .snapshot
        .messages
        .push(compactable_user("shared-body"));
    let previous = projection.capture_working_seed();
    let old = serde_json::to_vec(&previous.snapshot.materialize()).unwrap();
    let mut current = previous.clone();
    assert!(std::ptr::eq(
        &raw const current.snapshot.messages[0],
        &raw const previous.snapshot.messages[0]
    ));
    assert!(current.snapshot.model.same_owner(&previous.snapshot.model));
    current.snapshot.context.compaction_count += 1;
    current.snapshot.transcript.next_cursor = Some(RuntimeClientTranscriptCursor::new(8));
    current.snapshot.trace.read_cut = Some(current.read_cut.clone());
    current.prepared =
        super::super::prepared_capture::PreparedCaptures::inherit(&current, Some(&previous));
    assert!(
        !current
            .snapshot
            .context
            .same_owner(&previous.snapshot.context)
    );
    assert!(
        current
            .snapshot
            .messages
            .same_owner(&previous.snapshot.messages)
    );
    assert!(
        current
            .prepared
            .shared(&previous.prepared, "transcript_statistics")
    );
    assert!(
        current
            .prepared
            .shared(&previous.prepared, "transcript_deferred_entries")
    );
    assert!(
        current
            .prepared
            .shared(&previous.prepared, "trace_page_updates")
    );
    assert_eq!(
        serde_json::to_vec(&previous.snapshot.materialize()).unwrap(),
        old
    );
}
