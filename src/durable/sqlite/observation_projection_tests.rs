fn observation_projection_input(
    store: &SqliteConversationStore,
    id: &str,
) -> crate::durable::projection::ProjectionCommitInput {
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        store,
        id,
        1,
        &"\u{1}".repeat(130_000),
    );
    let mut input =
        crate::scripted_suites::support::context::checkpoint_commit_fixture(store, candidate, None);
    // Exercise a large opaque model field while remaining within the native
    // checkpoint occupancy record's 1 MiB admission/read budget.
    input.occupancy = Some(occupancy_origin_native(
        &"original-model".repeat(20_000),
        200,
    ));
    input
}
fn projection_receipt_witness(
    receipt: &crate::durable::projection::ProjectionCommitReceipt,
) -> &crate::durable::observation::ObservationEventWitness {
    let batch = receipt.completed.observation.as_ref().unwrap();
    assert_eq!(batch.read_cut.read.journal, receipt.completed.sequence);
    let crate::durable::observation::NativeObservationTransition::Single(witness) =
        &batch.transition
    else {
        panic!("original completion must carry one native witness");
    };
    assert_eq!(witness.sequence, receipt.completed.sequence);
    witness
}
fn assert_projection_receipt_head(receipt: &crate::durable::projection::ProjectionCommitReceipt) {
    let witness = projection_receipt_witness(receipt);
    assert!(!witness.source_unavailable);
    let reading = witness.occupancy.as_ref().unwrap();
    assert_eq!(
        reading.projection_head.transcript_anchor.get(),
        receipt.completed.observation.as_ref().unwrap().read_cut.read.transcript
    );
    assert_eq!(
        reading.projection_head.activation_id,
        receipt.checkpoint.activation.activation
    );
    assert_eq!(
        reading.projection_head.checkpoint_id,
        receipt.checkpoint.candidate.checkpoint_id
    );
    assert_eq!(
        reading.projection_head.origin.completed_journal.0,
        receipt.completed.sequence
    );
    assert_eq!(
        reading.compaction.activation_id,
        reading.projection_head.activation_id
    );
}
#[test]
fn compaction_marker_keeps_its_original_transcript_boundary_as_history_grows() {
    let store = store();
    store
        .initialize(&[user_message("anchor-original", "original C")])
        .unwrap();
    // Accepted user input has a T position before it enters the canonical Ledger.
    store
        .accept_inbound(draft("accepted before compaction"))
        .unwrap();
    let receipt = store
        .commit_projection(observation_projection_input(&store, "anchored-head"))
        .unwrap();
    assert_projection_receipt_head(&receipt);
    let before = store
        .observation_projection_activation(receipt.checkpoint.activation.activation)
        .unwrap();
    assert_eq!(before.transcript_anchor.get(), 2);
    assert_eq!(before.activation_ledger.0, 1);
    store
        .accept_inbound(draft("accepted after compaction"))
        .unwrap();
    store
        .append_canonical(&user_message("anchor-later", "later C"))
        .unwrap();
    let later = store
        .commit_projection(observation_projection_input(&store, "later-head"))
        .unwrap();
    let after = store
        .observation_projection_activation(receipt.checkpoint.activation.activation)
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        store
            .observation_projection_head(later.completed.sequence)
            .unwrap()
            .unwrap()
            .transcript_anchor
            .get(),
        4
    );
    let page = crate::runtime_client::projection_history::page(
        &store,
        later.completed.sequence,
        crate::context::checkpoint::ProjectionActivationId::default(),
        16,
    )
    .unwrap();
    assert_eq!(
        page.records
            .iter()
            .map(|head| head.transcript_anchor.get())
            .collect::<Vec<_>>(),
        [2, 4]
    );
}
#[test]
fn inherited_compaction_marker_uses_the_destination_transcript_boundary() {
    let source = store();
    let original = user_message("inherited-anchor", "original C");
    source.initialize(std::slice::from_ref(&original)).unwrap();
    source
        .accept_inbound(draft("source-only pending T"))
        .unwrap();
    let mut checkpoint = source
        .commit_projection(observation_projection_input(&source, "inherited-head"))
        .unwrap()
        .checkpoint;
    assert_eq!(
        source
            .observation_projection_activation(checkpoint.activation.activation)
            .unwrap()
            .transcript_anchor
            .get(),
        2
    );
    checkpoint.activation.destination_installation = Some(
        crate::runtime::identity::LineageInstallationId::new("installed-anchor"),
    );
    let destination = SqliteConversationStore::in_memory(source.conversation_id().clone()).unwrap();
    let seed = crate::durable::LineageSeed::history(vec![
        original,
        user_message("later-inherited-anchor", "later C"),
    ])
    .with_projections(vec![checkpoint])
    .unwrap();
    destination.initialize_lineage(&seed).unwrap();
    let head = destination
        .observation_projection_activation(crate::context::checkpoint::ProjectionActivationId(1))
        .unwrap();
    assert_eq!(head.transcript_anchor.get(), 1);
}
fn deny_projection_body(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name: "projection_checkpoints",
            column_name: "candidate_json" | "occupancy_json",
        }
        | AuthAction::Read {
            table_name: "projection_activations",
            column_name: "activation_json",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}
fn assert_completion_covering_work(connection: &Connection) {
    let root: i64 = connection
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let json_column: i64 = connection
        .query_row(
            "SELECT cid FROM pragma_table_info('events') WHERE name='event_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut statement = connection
        .prepare(&format!("EXPLAIN {}", projection::completion_query()))
        .unwrap();
    let opcodes = statement
        .query_map([u64::MAX / 2], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let native_cursors = opcodes
        .iter()
        .filter_map(|(op, p1, p2)| (op == "OpenRead" && *p2 == root).then_some(*p1))
        .collect::<Vec<_>>();
    assert!(
        !opcodes.iter().any(|(op, p1, p2)| op == "Column"
            && native_cursors.contains(p1)
            && *p2 == json_column),
        "covering original completion probe must not fetch the growing Journal body"
    );
}
#[test]
fn t08_t12_t19_t20_projection_head_reads_only_finite_original_activation_evidence() {
    let store = store();
    let original = user_message("finite-head-original", "immutable C");
    store.initialize(std::slice::from_ref(&original)).unwrap();
    let receipt = store
        .commit_projection(observation_projection_input(&store, "finite-head"))
        .unwrap();
    assert_projection_receipt_head(&receipt);
    projection::take_candidate_decodes();
    projection::take_header_work();
    projection::take_completion_work();
    observation_projection::take_read_work();
    store.lock().unwrap().authorizer(Some(deny_projection_body)).unwrap();
    let head = store
        .observation_projection_head(receipt.completed.sequence)
        .unwrap()
        .unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert_eq!(
        head.checkpoint_id,
        receipt.checkpoint.candidate.checkpoint_id
    );
    assert_eq!(head.activation_id, receipt.checkpoint.activation.activation);
    assert!(head.summary_preview.unwrap().truncated);
    assert_eq!(projection::take_candidate_decodes(), 0);
    assert_eq!(projection::take_header_work().1, 0);
    let (rows, bytes) = projection::take_completion_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 1280);
    let (rows, bytes) = observation_projection::take_read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384 + 2304);
    assert_completion_covering_work(&store.lock().unwrap());
    assert_eq!(store.load_canonical().unwrap(), vec![original]);
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
}
#[test]
fn t10_t12_t19_projection_display_holes_and_tamper_do_not_change_native_checkpoint_commit() {
    let store = store();
    store
        .initialize(&[user_message("header-fault-original", "original")])
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_projection_header BEFORE UPDATE OF observation_header ON projection_activations BEGIN SELECT RAISE(FAIL,'optional projection header'); END;").unwrap();
    let receipt = store
        .commit_projection(observation_projection_input(&store, "header-fault"))
        .unwrap();
    let witness = projection_receipt_witness(&receipt);
    assert!(witness.source_unavailable && witness.occupancy.is_none());
    assert!(matches!(
        store.observation_projection_head(receipt.completed.sequence),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_projection_header")
        .unwrap();
    let next = store
        .commit_projection(observation_projection_input(&store, "header-good"))
        .unwrap();
    assert_projection_receipt_head(&next);
    let head = store
        .observation_projection_head(next.completed.sequence)
        .unwrap()
        .unwrap();
    let bytes: Vec<u8> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_header FROM projection_activations WHERE activation=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    changed["original_items"] = serde_json::json!(999);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET observation_header=?1 WHERE activation=2",
            [serde_json::to_vec(&changed).unwrap()],
        )
        .unwrap();
    assert!(
        store
            .observation_projection_head(next.completed.sequence)
            .is_err()
    );
    store.lock().unwrap().execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE projection_activations SET observation_header=zeroblob(4000000) WHERE activation=2; PRAGMA ignore_check_constraints=OFF;").unwrap();
    observation_projection::take_read_work();
    assert!(
        store
            .observation_projection_head(next.completed.sequence)
            .is_err()
    );
    assert!(
        observation_projection::take_read_work().1 < 2304,
        "oversized header never allocated into Rust"
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET observation_header=?1 WHERE activation=2",
            [bytes],
        )
        .unwrap();
    assert_eq!(
        store
            .observation_projection_head(next.completed.sequence)
            .unwrap()
            .unwrap(),
        head
    );
    assert_eq!(store.projection_head().unwrap().unwrap(), next.checkpoint);
}
#[test]
fn t10_t12_t19_uncertain_projection_receipt_retains_the_original_publication_cut() {
    let store = store();
    store
        .initialize(&[user_message("recovery-original", "original")])
        .unwrap();
    let input = observation_projection_input(&store, "recovered-head");
    let operation = input.operation_id.clone();
    let fingerprint = input.candidate.fingerprint();
    store.arm_compaction_fault_script([CompactionFaultOperation::AfterCommit]);
    assert!(store.commit_projection(input).is_err());
    let first = store
        .projection_receipt(&operation, &fingerprint)
        .unwrap()
        .unwrap();
    assert!(!first.newly_committed);
    assert_projection_receipt_head(&first);
    let original = first.completed.observation.as_ref().unwrap();
    assert_eq!(original.read_cut.read.journal, first.completed.sequence);
    store
        .append_canonical(&user_message("later-recovery-input", "later immutable C"))
        .unwrap();
    store
        .append_event(envelope(
            store.conversation_id(),
            "later-j",
            Some(AttemptId::new("later-execution")),
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("later-execution"),
            },
        ))
        .unwrap();
    let replay = store
        .projection_receipt(&operation, &fingerprint)
        .unwrap()
        .unwrap();
    let retained = replay.completed.observation.as_ref().unwrap();
    assert_projection_receipt_head(&replay);
    assert_eq!(
        projection_receipt_witness(&replay).occupancy,
        projection_receipt_witness(&first).occupancy
    );
    assert_eq!(retained.read_cut, original.read_cut);
    assert_eq!(retained.journal_range, original.journal_range);
    assert_eq!(retained.ready_before, original.ready_before);
    assert_eq!(retained.ready_t_through, original.ready_t_through);
    assert_eq!(retained.trace_total, original.trace_total);
    assert_eq!(replay.completed.event, first.completed.event);
    assert_eq!(replay.completed.sequence, first.completed.sequence);
    assert!(store.observation_read_cut().unwrap().read.journal > retained.read_cut.read.journal);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE compaction_operations SET observation_receipt=NULL WHERE operation_id=?1",
            [operation.as_str()],
        )
        .unwrap();
    let unavailable = store
        .projection_receipt(&operation, &fingerprint)
        .unwrap()
        .unwrap();
    assert!(unavailable.completed.observation.is_none());
    assert_eq!(unavailable.completed.event, first.completed.event);
}

#[test]
fn t10_t19_projection_publication_evidence_fault_preserves_confirmed_native_receipt() {
    let store = store();
    store
        .initialize(&[user_message("publication-fault", "original")])
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_original_publication BEFORE UPDATE OF observation_receipt ON compaction_operations BEGIN SELECT RAISE(FAIL,'optional original publication'); END;").unwrap();
    let input = observation_projection_input(&store, "publication-evidence-fault");
    let operation = input.operation_id.clone();
    let fingerprint = input.candidate.fingerprint();
    store.arm_compaction_fault_script([CompactionFaultOperation::AfterCommit]);
    assert!(store.commit_projection(input).is_err());
    let receipt = store
        .projection_receipt(&operation, &fingerprint)
        .unwrap()
        .unwrap();
    assert!(receipt.completed.observation.is_none());
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    assert_eq!(
        store
            .read_events(Some(receipt.completed.sequence - 1), 1)
            .unwrap()
            .events[0]
            .event,
        receipt.completed.event
    );
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![user_message("publication-fault", "original")]
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_original_publication")
        .unwrap();
    let next = store
        .commit_projection(observation_projection_input(
            &store,
            "publication-evidence-good",
        ))
        .unwrap();
    let recovered = store
        .projection_receipt(&next.operation_id, &next.candidate_fingerprint)
        .unwrap()
        .unwrap();
    assert!(recovered.completed.observation.is_some());
}

fn original_provenance_fixture() -> (
    SqliteConversationStore,
    crate::durable::projection::ProjectionCommitReceipt,
) {
    let store = store();
    let original = user_message(&"retired🙂".repeat(8000), "retired original");
    let retained = user_message(&"retained🙂".repeat(7000), "retained original");
    store.initialize(&[original, retained]).unwrap();
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        &store,
        "long-provenance",
        1,
        "summary",
    );
    let receipt = store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store, candidate, None,
            ),
        )
        .unwrap();
    (store, receipt)
}
fn collect_provenance_windows(
    mut read: impl FnMut(
        u64,
    ) -> Result<
        crate::durable::observation::ObservationValueChunk,
        ConversationStoreError,
    >,
) -> serde_json::Value {
    let mut bytes = Vec::new();
    let mut offset = 0;
    let mut extent = None;
    loop {
        let chunk = read(offset).unwrap();
        assert!(chunk.bytes.len() <= 16_384);
        assert_eq!(
            *extent.get_or_insert(chunk.readable_end),
            chunk.readable_end
        );
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
        bytes.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert!(
        bytes.len() > 65_536,
        "complete original identity must be selected, never clipped"
    );
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn t08_t12_t19_long_original_provenance_keeps_finite_request_headers_and_exact_ranges() {
    use crate::runtime_client::trace::TraceInputProvenance;
    let (store, receipt) = original_provenance_fixture();
    let original = store.load_canonical().unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let page = store
        .request_field_page_at(
            started.sequence,
            started.sequence,
            &["messages".into()],
            0,
            2,
        )
        .unwrap();
    assert_eq!(page.total, 2);
    for (ordinal, field) in page.fields.iter().enumerate() {
        assert!(serde_json::to_vec(&field.value).unwrap().len() <= 16_384);
        assert!(field.value["provenance_source"].is_object());
        let provenance: TraceInputProvenance =
            crate::runtime_client::checkpoint::decode_domain_value(
                field.value["provenance"].clone(),
            )
            .unwrap();
        match provenance {
            TraceInputProvenance::Derived {
                sources, truncated, ..
            } => {
                assert!(sources.is_empty());
                assert!(truncated);
            }
            TraceInputProvenance::Original {
                source,
                locator,
                message_id_source,
                ..
            } => {
                assert!(source.is_none());
                assert!(locator.is_some());
                assert!(message_id_source.is_some());
            }
            TraceInputProvenance::RequestOnly => panic!("no synthetic request-only item"),
        }
        let path = vec!["messages".into(), ordinal.to_string(), "provenance".into()];
        let value = collect_provenance_windows(|offset| {
            store.request_value_read_at(started.sequence, started.sequence, &path, offset, 16_384)
        });
        let source = if ordinal == 0 {
            &value["sources"][0]
        } else {
            &value["source"]
        };
        let expected = crate::model::projected::CanonicalMessageRef::of(
            store.conversation_id(),
            &original[ordinal],
        );
        assert_eq!(
            serde_json::from_value::<crate::model::projected::CanonicalMessageRef>(source.clone())
                .unwrap(),
            expected
        );
    }
    assert_eq!(store.load_canonical().unwrap(), original);
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
}
#[test]
fn t08_t10_t12_t19_projection_provenance_ranges_are_cut_owned_and_auxiliary() {
    let (store, receipt) = original_provenance_fixture();
    let cut = store.observation_read_cut().unwrap();
    let activation = receipt.checkpoint.activation.activation.0;
    store.lock().unwrap().authorizer(Some(deny_projection_body)).unwrap();
    for item in 0..2 {
        let value = collect_provenance_windows(|offset| {
            store.observation_projection_provenance_read(
                &cut,
                activation,
                item,
                &[],
                offset,
                16_384,
            )
        });
        assert_eq!(
            value["type"],
            if item == 0 { "derived" } else { "original" }
        );
    }
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>).unwrap();
    assert!(
        store
            .observation_projection_provenance_read(&cut, activation, 2, &[], 0, 1)
            .is_err()
    );
    let mut foreign = cut.clone();
    foreign.read.conversation_id = ConversationId::generate();
    assert!(matches!(
        store.observation_projection_provenance_read(&foreign, activation, 0, &[], 0, 1),
        Err(ConversationStoreError::ObservationCutExpired)
    ));
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_projection_provenance BEFORE INSERT ON state_observation_fields WHEN NEW.source_id LIKE 'projection-provenance/%' BEGIN SELECT RAISE(FAIL,'optional provenance'); END;").unwrap();
    let next = store
        .commit_projection(observation_projection_input(&store, "provenance-hole"))
        .unwrap();
    let next_cut = store.observation_read_cut().unwrap();
    assert!(
        store
            .observation_projection_provenance_read(
                &next_cut,
                next.checkpoint.activation.activation.0,
                0,
                &[],
                0,
                1
            )
            .is_err()
    );
    assert_eq!(store.projection_head().unwrap().unwrap(), next.checkpoint);
    assert!(
        store
            .observation_projection_head(next.completed.sequence)
            .unwrap()
            .is_some()
    );
    assert_eq!(store.load_canonical().unwrap().len(), 2);
    assert!(
        store
            .observation_projection_provenance_read(&cut, activation, 0, &[], 0, 1)
            .is_ok()
    );
}
