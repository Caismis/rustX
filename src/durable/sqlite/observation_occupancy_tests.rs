// Native original requests remain intact; occupancy reads only finite committed scalars.
fn occupancy_original_snapshot(store: &SqliteConversationStore) -> RequestSnapshot {
    let mut snapshot = progress_request_snapshot(
        "occupancy-source",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = "\u{feff}🙂水".repeat(120_000);
    snapshot.effective_system_prompt = "\u{feff} native prompt\n🙂水".repeat(70_000);
    snapshot.tool_definitions = vec![crate::tools::types::ModelToolDefinition {
        id: ToolId::new("original-tool"),
        name: "originalTool".into(),
        description: "native original schema".repeat(80_000),
        input_schema: serde_json::json!({"n":u64::MAX,"zero":-0.0,"string":"水".repeat(400_000)}),
    }];
    snapshot
}
#[test]
fn t08_t14_t19_occupancy_reads_fixed_original_request_scalars_and_model_ranges() {
    let store = store();
    store
        .initialize(&[user_message("occupancy-input", "original")])
        .unwrap();
    let mut snapshot = occupancy_original_snapshot(&store);
    let usage = crate::model::types::ModelUsage {
        input_tokens: 3_000_000,
        output_tokens: 1,
        total_tokens: 3_000_001,
        details: None,
    };
    let expected = crate::context::occupancy::measure(&usage, &snapshot)
        .unwrap()
        .unwrap();
    freeze_test_input(&store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        "occupancy-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let terminal = store.append_event(terminal).unwrap();
    let request_reads = store.request_snapshot_body_reads.load(Ordering::SeqCst);
    let message_reads = store.checkpoint_read_counts[0].load(Ordering::SeqCst);
    let observed = store
        .observation_occupancy(terminal.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(observed.input_tokens, expected.input_tokens);
    assert_eq!(
        observed.context_window_tokens,
        expected.context_window_tokens
    );
    assert_eq!(observed.breakdown, expected.breakdown);
    assert!(observed.model.is_none());
    assert!(observed.model_deferred);
    assert_eq!(
        observed.model_source,
        Some(format!(
            "trace.records.{}.detail.request.model",
            start.started.sequence
        ))
    );
    assert_eq!(
        store.request_snapshot_body_reads.load(Ordering::SeqCst),
        request_reads
    );
    assert_eq!(
        store.checkpoint_read_counts[0].load(Ordering::SeqCst),
        message_reads
    );
    let (rows, bytes) = observation_occupancy::read_work();
    assert_eq!(rows, 1);
    assert!(bytes <= 16_384);
    let mut original = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .request_value_read(
                &snapshot.request_id,
                terminal.sequence,
                &["model".into()],
                offset,
                65_536,
            )
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        original.extend(chunk.bytes);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(
        serde_json::from_slice::<String>(&original).unwrap(),
        snapshot.invocation.model
    );
    assert_eq!(
        store.observation_occupancy(start.started.sequence).unwrap(),
        None
    );
}

#[test]
fn t14_t19_request_tool_fingerprints_preserve_native_partial_eq() {
    let tool = |input_schema| crate::tools::types::ModelToolDefinition {
        id: ToolId::new("fingerprint-tool"),
        name: "native".into(),
        description: "original".into(),
        input_schema,
    };
    let left = vec![tool(
        serde_json::from_str(r#"{"zero":-0.0,"ordered":{"a":1,"b":2},"large":"original"}"#)
            .unwrap(),
    )];
    let right = vec![tool(
        serde_json::from_str(r#"{"large":"original","ordered":{"b":2,"a":1},"zero":0.0}"#).unwrap(),
    )];
    assert_eq!(left, right);
    assert_eq!(
        observation_request_summary::tools_digest(&left),
        observation_request_summary::tools_digest(&right)
    );
    let different = vec![tool(
        serde_json::json!({"zero":0,"ordered":{"a":1,"b":2},"large":"original"}),
    )];
    assert_ne!(left, different);
    assert_ne!(
        observation_request_summary::tools_digest(&left),
        observation_request_summary::tools_digest(&different)
    );
    let object = |reverse: bool| {
        let mut values = serde_json::Map::new();
        for ordinal in 0..4097 {
            let ordinal = if reverse { 4096 - ordinal } else { ordinal };
            values.insert(
                format!("opaque-key-{ordinal}"),
                serde_json::json!({"nested": [ordinal, null, "original"]}),
            );
        }
        vec![tool(serde_json::Value::Object(values))]
    };
    let forward = object(false);
    let reverse = object(true);
    assert_eq!(forward, reverse);
    assert_eq!(
        observation_request_summary::tools_digest(&forward),
        observation_request_summary::tools_digest(&reverse)
    );
    let ordered = vec![left[0].clone(), different[0].clone()];
    let reordered = vec![different[0].clone(), left[0].clone()];
    assert_ne!(ordered, reordered);
    assert_ne!(
        observation_request_summary::tools_digest(&ordered),
        observation_request_summary::tools_digest(&reordered)
    );
    for field in 0..3 {
        let mut changed = left.clone();
        match field {
            0 => changed[0].id = ToolId::new("different-native-id"),
            1 => changed[0].name.push('!'),
            _ => changed[0].description.push('!'),
        }
        assert_ne!(left, changed);
        assert_ne!(
            observation_request_summary::tools_digest(&left),
            observation_request_summary::tools_digest(&changed)
        );
    }
    let first = vec![tool(serde_json::json!({"large":"a".repeat(2_100_000)}))];
    let second = vec![tool(serde_json::json!({"large":"a".repeat(2_099_999)+"b"}))];
    assert_ne!(first, second);
    assert_ne!(
        observation_request_summary::tools_digest(&first),
        observation_request_summary::tools_digest(&second)
    );
}

#[test]
fn t10_t14_occupancy_derived_header_failure_preserves_original_provider_terminal() {
    let store = store();
    store
        .initialize(&[user_message("occupancy-fault-input", "original")])
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        "occupancy-fault",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER occupancy_layout_fault BEFORE UPDATE OF observation_occupancy ON events BEGIN SELECT RAISE(FAIL,'derived occupancy fault'); END;").unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        "occupancy-fault-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(crate::model::types::ModelUsage {
                input_tokens: 10,
                output_tokens: 1,
                total_tokens: 11,
                details: None,
            }),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let committed = store.append_event(terminal).unwrap();
    assert!(store.observation_occupancy(committed.sequence).is_err());
    let original = store.read_events(Some(committed.sequence - 1), 1).unwrap();
    assert!(matches!(
        original.events[0].event,
        RuntimeEvent::ModelRequestCompleted { .. }
    ));
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
}

#[allow(clippy::too_many_lines)] // End-to-end auxiliary exclusion fixture.
#[test]
fn mixed_auxiliary_terminal_never_replaces_primary_occupancy_statistics_or_trace() {
    use crate::durable::observation::{TraceFactKind, TraceFactQuery, TraceFactScope};
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryCallStart, AuxiliaryFingerprint,
        AuxiliaryRequestStart,
    };
    use crate::model::snapshot::ModelRequestOwner;
    use crate::runtime::identity::{AuxiliaryCallId, RequestId};

    let store = store();
    store.initialize(&[]).unwrap();
    let mut snapshot = progress_request_snapshot(
        "primary-occupancy-mixed",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let primary_usage = crate::model::types::ModelUsage {
        input_tokens: 321,
        output_tokens: 12,
        total_tokens: 333,
        details: None,
    };
    let mut primary = envelope(
        store.conversation_id(),
        "primary-mixed-terminal",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(primary_usage),
            generation: None,
        },
    );
    primary.turn_id = Some(snapshot.identity.turn.clone());
    let primary = store.append_event(primary).unwrap();
    let expected_occupancy = store
        .observation_occupancy(primary.sequence)
        .unwrap()
        .unwrap();
    let expected_statistics = store
        .observation_agent_statistics(primary.sequence)
        .unwrap();
    let trace_before = store
        .observation_trace_facts(
            &store.observation_read_cut().unwrap(),
            &TraceFactQuery {
                scope: TraceFactScope::Agent,
                kinds: vec![TraceFactKind::ModelRequestStarted],
                before: None,
                after: 0,
                ascending: true,
                limit: 32,
            },
        )
        .unwrap()
        .len();

    let fingerprint = AuxiliaryFingerprint::new("a".repeat(64)).unwrap();
    let call_id = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(
            &AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: AuxiliaryAttribution::new("summary").unwrap(),
                trigger: AuxiliaryAttribution::new("automatic").unwrap(),
                semantic_input_fingerprint: fingerprint.clone(),
                invocation_fingerprint: fingerprint,
                business_source: crate::model::auxiliary::AuxiliaryBusinessSource::Independent,
                input: crate::model::auxiliary::AuxiliaryInputEvidence::Temporary,
                selected_message_ids: Vec::new(),
            },
            Utc::now(),
        )
        .unwrap();
    let request_id = RequestId::new("mixed-auxiliary-request");
    let auxiliary_start = store
        .commit_auxiliary_request_start(
            &AuxiliaryRequestStart {
                call_id: call_id.clone(),
                ordinal: 0,
                request_id: request_id.clone(),
                model: "auxiliary".into(),
            },
            Utc::now(),
        )
        .unwrap();
    assert!(matches!(
        store.request_summary_header_at(
            auxiliary_start.request.started_sequence,
            auxiliary_start.event.sequence
        ),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let auxiliary_terminal = envelope(
        store.conversation_id(),
        &format!("auxiliary-request-completed:{request_id}"),
        None,
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary {
                call_id: call_id.clone(),
                ordinal: 0,
            },
            request_id,
            finish_reason: ModelFinishReason::Stop,
            usage: Some(crate::model::types::ModelUsage {
                input_tokens: 9_999_999,
                output_tokens: 9_999,
                total_tokens: 10_009_998,
                details: None,
            }),
            generation: None,
        },
    );
    let RuntimeEvent::ModelRequestCompleted { request_id, usage, finish_reason, .. } = &auxiliary_terminal.event else { unreachable!() };
    let evidence = crate::model::auxiliary::AuxiliaryRequestEvidence {
        request_id: request_id.clone(),
        observed_provider_terminal: Some(
            crate::model::auxiliary::AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: finish_reason.into(),
            },
        ),
        usage: usage.clone(),
        usage_completeness: crate::model::auxiliary::AuxiliaryUsageCompleteness::Complete,
        remote_execution_uncertainty: false,
    };
    let auxiliary_terminal = store
        .commit_auxiliary_request_settlement(
            auxiliary_terminal,
            envelope(
                store.conversation_id(),
                &format!("auxiliary-call-settled:{call_id}"),
                None,
                RuntimeEvent::AuxiliaryCallSettled {
                    call_id,
                    request_evidence: Some(evidence),
                    outcome: crate::model::auxiliary::AuxiliaryCallOutcome::Completed,
                },
            ),
        )
        .unwrap()
        .request_terminal
        .unwrap();

    assert_eq!(
        store
            .observation_occupancy(auxiliary_terminal.sequence)
            .unwrap(),
        Some(expected_occupancy)
    );
    assert_eq!(
        store
            .observation_agent_statistics(auxiliary_terminal.sequence)
            .unwrap(),
        expected_statistics
    );
    assert_eq!(
        store
            .observation_trace_facts(
                &store.observation_read_cut().unwrap(),
                &TraceFactQuery {
                    scope: TraceFactScope::Agent,
                    kinds: vec![TraceFactKind::ModelRequestStarted],
                    before: None,
                    after: 0,
                    ascending: true,
                    limit: 32,
                },
            )
            .unwrap()
            .len(),
        trace_before
    );
    let witness = store
        .observation_event_witness(auxiliary_terminal.sequence, auxiliary_terminal.sequence)
        .unwrap()
        .unwrap();
    assert!(matches!(
        witness.owner,
        Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
    ));
    assert!(witness.auxiliary_call_id.is_some());
    let owner_bytes = store
        .event_value_read(
            auxiliary_terminal.sequence,
            auxiliary_terminal.sequence,
            &["event".into(), "owner".into()],
            0,
            16_384,
        )
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<ModelRequestOwner>(&owner_bytes.bytes).unwrap(),
        ModelRequestOwner::Auxiliary { .. }
    ));
}

fn commit_primary_occupancy_terminal(
    store: &SqliteConversationStore,
    attempt_id: &str,
    input_tokens: u64,
) -> (RequestSnapshot, RuntimeEventEnvelope) {
    let mut snapshot = progress_request_snapshot(
        attempt_id,
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let mut terminal = envelope(
        store.conversation_id(),
        &format!("{attempt_id}-occupancy-terminal"),
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(crate::model::types::ModelUsage {
                input_tokens,
                output_tokens: 1,
                total_tokens: input_tokens + 1,
                details: None,
            }),
            generation: None,
        },
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    let terminal = store.append_event(terminal).unwrap();
    (snapshot, terminal)
}

#[test]
fn latest_primary_occupancy_requires_an_exact_mandatory_owner_row() {
    for corruption in [
        "DELETE FROM request_owners WHERE request_id=?1",
        "UPDATE request_owners SET request_id='foreign-request' WHERE request_id=?1",
        "UPDATE request_owners SET terminal_kind='failed' WHERE request_id=?1",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let (_older, _) = commit_primary_occupancy_terminal(&store, "occupancy-older", 10);
        let (latest_snapshot, latest) =
            commit_primary_occupancy_terminal(&store, "occupancy-latest", 20);
        store
            .conn
            .lock()
            .unwrap()
            .execute(corruption, [latest_snapshot.request_id.as_str()])
            .unwrap();
        assert!(
            store.observation_occupancy(latest.sequence).is_err(),
            "{corruption}"
        );
    }
}

fn occupancy_sql_steps<T>(store: &SqliteConversationStore, read: impl FnOnce() -> T) -> (T, usize) {
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = steps.clone();
    store
        .lock()
        .unwrap()
        .progress_handler(
            1,
            Some(move || {
                counted.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
    let result = read();
    store
        .lock()
        .unwrap()
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();
    (result, steps.load(Ordering::Relaxed))
}

#[test]
fn occupancy_compaction_lookup_work_is_bounded_across_history_growth() {
    use crate::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
    use crate::context::occupancy::{ContextBreakdown, ContextOccupancy};
    use crate::model::projected::CanonicalMessageRef;
    let store = store();
    let message = user_message("occupancy-history", "history");
    store.initialize(std::slice::from_ref(&message)).unwrap();
    let mut first_cut = 0;
    let mut baseline = Vec::new();
    let mut negative_baseline = Vec::new();
    for index in 1..=256 {
        let cut = store.compaction_resource_cut().unwrap();
        let candidate = CheckpointCandidate {
            checkpoint_id: crate::runtime::identity::CheckpointId::new(format!(
                "occupancy-{index}"
            )),
            expected_head: store
                .projection_head()
                .unwrap()
                .map(|p| p.candidate.checkpoint_id),
            base_frontier: cut.ledger,
            read_cut: cut,
            items: vec![CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &message,
                )),
                call_sources: Vec::new(),
                result_call: None,
            }],
            summary_metadata: None,
            summary_generation: None,
        };
        let mut input = crate::scripted_suites::support::context::checkpoint_commit_fixture(
            &store, candidate, None,
        );
        input.estimated_tokens_after = index;
        input.occupancy = Some(ContextOccupancy {
            estimated: true,
            input_tokens: index,
            context_window_tokens: 1000,
            model: "model".into(),
            breakdown: ContextBreakdown {
                system_tokens: 0,
                tool_tokens: 0,
                message_tokens: index,
            },
        });
        let committed = store.commit_projection(input).unwrap();
        if index == 1 {
            first_cut = committed.completed.sequence;
        }
        if ![1, 32, 256].contains(&index) {
            continue;
        }
        for (case, through, expected) in
            [(0, first_cut, 1), (1, committed.completed.sequence, index)]
        {
            let (actual, steps) = occupancy_sql_steps(&store, || {
                store.observation_occupancy(through).unwrap().unwrap()
            });
            assert_eq!(actual.input_tokens, expected);
            assert!(steps > 0);
            if index == 1 {
                baseline.push(steps);
            } else {
                assert!(
                    steps <= baseline[case] + 128,
                    "{index} compactions: {steps} versus {}",
                    baseline[case]
                );
            }

            // Negative control: omitting the middle index equality returns the
            // same row but scans/sorts growing history, even at a fixed old cut.
            let (sequence, unbounded) = occupancy_sql_steps(&store, || {
                store
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT sequence FROM events INDEXED BY events_context_reading_idx
                     WHERE json_extract(event_json,'$.event.type')='compaction_completed'
                       AND sequence<=?1 ORDER BY sequence DESC LIMIT 1",
                        [through],
                        |row| row.get::<_, u64>(0),
                    )
                    .unwrap()
            });
            assert_eq!(sequence, through);
            if index == 1 {
                negative_baseline.push(unbounded);
            }
            if index == 256 {
                assert!(unbounded > negative_baseline[case] * 16);
            }
        }
    }
}
