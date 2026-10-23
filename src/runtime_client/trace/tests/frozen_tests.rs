// Captured native C/J cuts retain distinct positive canonical and execution evidence.
use super::*;
use crate::durable::{CanonicalStore, ObservationStore};

fn running_trace_snapshot(
    store: &dyn ConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    current: crate::observation::tool::ForegroundToolExecution,
) -> crate::runtime_client::snapshot::RuntimeClientSnapshot {
    use crate::observation::capability_source::CapabilityView;
    use crate::runtime_client::snapshot::{RuntimeClientAttempt, RuntimeClientAttemptPhase};
    let mut snapshot = crate::runtime_client::projection::RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            configured_tools: vec![],
            revision: CapabilityRevision::new(0),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    snapshot.attempt = Some(RuntimeClientAttempt {
        attempt_id: Some(AttemptId::new("attempt-a")),
        attempt_id_source: None,
        phase: RuntimeClientAttemptPhase::Running,
        turn: 1,
        last_usage: None,
        in_flight: None,
        foreground: vec![current],
        model: None,
        execution_settings: None,
    });
    snapshot.trace = TraceProjection::at_cut(store, cut)
        .unwrap()
        .page(None, 32)
        .unwrap();
    snapshot
}

#[test]
fn t08_t14_trace_live_tool_uses_numeric_occurrence_before_repeated_aliases() {
    use crate::observation::content::CanonicalContentLocator;
    let store = store("conv_672fc113-c7cb-7782-986d-65cb9d216dd1");
    start(&store);
    let call = bash_call("repeated-trace-call");
    for (owner, seconds) in [
        ("first-repeated-proposal", 2),
        ("second-repeated-proposal", 4),
    ] {
        propose_tool_call(&store, owner, &call, seconds);
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            seconds + 1,
        );
    }
    let cut = store.observation_read_cut().unwrap();
    let tool_page = store
        .transcript_tool_headers(
            &cut,
            &CanonicalContentLocator::ByLedgerPosition { position: 2 },
            0,
            1,
        )
        .unwrap();
    let current = crate::observation::tool::foreground(&tool_page, &tool_page.headers[0]).unwrap();
    let mut snapshot = running_trace_snapshot(&store, &cut, current);
    let before = snapshot
        .trace
        .records
        .iter()
        .filter(|row| row.kind == TraceKind::Tool)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(before.len(), 2);
    assert_eq!(
        before[0].tool.as_ref().unwrap().call_id,
        before[1].tool.as_ref().unwrap().call_id
    );
    assert_eq!(
        before[0].tool.as_ref().unwrap().tool_id,
        before[1].tool.as_ref().unwrap().tool_id
    );
    assert_ne!(
        before[0].tool.as_ref().unwrap().occurrence,
        before[1].tool.as_ref().unwrap().occurrence
    );
    repair_live(&mut snapshot);
    let after = snapshot
        .trace
        .records
        .iter()
        .filter(|row| row.kind == TraceKind::Tool)
        .collect::<Vec<_>>();
    assert_eq!(
        after[0].state,
        TraceState::Incomplete,
        "reused aliases cannot claim another canonical occurrence"
    );
    assert_eq!(after[1].state, TraceState::Running);
    assert_eq!(after[1].timing.duration_ms, None);
    assert_eq!(after[0].fields, before[0].fields);
    assert_eq!(after[0].collections, before[0].collections);
    let first = snapshot
        .trace
        .records
        .iter_mut()
        .find(|row| row.id == before[0].id)
        .unwrap();
    first.tool.as_mut().unwrap().occurrence = None;
    repair_live(&mut snapshot);
    assert_eq!(
        snapshot
            .trace
            .records
            .iter()
            .find(|row| row.id == before[0].id)
            .unwrap()
            .state,
        TraceState::Running,
        "aliases are used only when numeric occurrence evidence is absent"
    );
}

fn frozen_result() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(TextBlock {
            text: "canonical result before terminal J".into(),
        })],
        duration_ms: 25,
        exit_code: None,
        artifacts: vec![crate::message::content::FileReference {
            artifact_id: ArtifactId::new("frozen-artifact"),
            name: Some("original.md".into()),
            mime_type: Some("text/markdown".into()),
            description: None,
        }],
        deliveries: vec![],
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn frozen_tool_record(page: &TracePage) -> &TraceRecord {
    page.records
        .iter()
        .find(|record| record.kind == TraceKind::Tool)
        .unwrap()
}

#[test]
fn t08_t14_t19_trace_c_without_j_keeps_both_frozen_native_cuts() {
    let store = store("conv_40164b38-6f23-7e6f-9a23-b2906f6c2aa7");
    start(&store);
    let call = bash_call("frozen-tool-call");
    propose_tool_call(&store, "frozen-proposal", &call, 2);
    append(
        &store,
        E::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
        3,
    );
    let before = store.observation_read_cut().unwrap();
    let before_projection = TraceProjection::at_cut(&store, &before).unwrap();
    let before_page = before_projection.page(None, 32).unwrap();
    let prior = frozen_tool_record(&before_page);
    assert_eq!(prior.state, TraceState::Incomplete);
    assert!(prior.tool.as_ref().unwrap().outcome.is_none());
    assert!(prior.message_id.is_none());
    let result = frozen_result();
    let tool = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("frozen-result"),
        occurrence: ToolCallOccurrenceRef::new(
            MessageId::new("frozen-proposal"),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: result.clone(),
    });
    store.append_canonical(&tool).unwrap();
    let after = store.observation_read_cut().unwrap();
    assert_eq!(after.read.journal, before.read.journal);
    assert_eq!(after.ledger_through, before.ledger_through + 1);
    let after_projection = TraceProjection::at_cut(&store, &after).unwrap();
    let after_page = after_projection.page(None, 32).unwrap();
    let committed = frozen_tool_record(&after_page);
    assert_eq!(
        committed.state,
        TraceState::Incomplete,
        "C result does not invent J settlement"
    );
    assert_eq!(
        committed.tool.as_ref().unwrap().outcome,
        Some(TraceToolOutcome::Success)
    );
    assert_eq!(committed.message_id, Some(MessageId::new("frozen-result")));
    assert_eq!(
        committed.preview.as_ref().unwrap().text,
        "canonical result before terminal J"
    );
    assert_eq!(committed.attachments.len(), 1);
    assert_eq!(
        committed.attachments[0].artifact_id,
        ArtifactId::new("frozen-artifact")
    );
    append(
        &store,
        E::ToolExecutionCompleted {
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result,
        },
        4,
    );
    let final_page = page(&store);
    let terminal = frozen_tool_record(&final_page);
    assert_eq!(terminal.state, TraceState::Completed);
    assert_eq!(terminal.timing.duration_ms, Some(1_000));
    assert_eq!(before_projection.page(None, 32).unwrap(), before_page);
    assert_eq!(after_projection.page(None, 32).unwrap(), after_page);
    let interests = [prior.position.clone()];
    assert!(
        before_projection.refresh(&interests, None).unwrap()[0]
            .tool
            .as_ref()
            .unwrap()
            .outcome
            .is_none()
    );
    let update = after_projection
        .refresh(&interests, None)
        .unwrap()
        .remove(0);
    assert_eq!(update.state, TraceState::Incomplete);
    assert_eq!(
        update.tool.as_ref().unwrap().outcome,
        Some(TraceToolOutcome::Success)
    );
}

#[test]
fn t08_t14_t19_trace_request_lifecycle_is_independent_of_optional_summary_holes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("optional-trace-summary.sqlite");
    let store = SqliteConversationStore::open(ConversationId::generate(), &path).unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let first = request(&store, 0);
    completion(&store, &first, None, None, 4);
    let second = request(&store, 1);
    let usage = ModelUsage {
        input_tokens: Some(13),
        output_tokens: Some(7),
        total_tokens: Some(20),
        details: None,
    };
    completion(&store, &second, Some(usage.clone()), None, 6);
    let cut = store.observation_read_cut().unwrap();
    let projection = TraceProjection::at_cut(&store, &cut).unwrap();
    let request_record = projection.page(None, 1).unwrap().records.pop().unwrap();
    assert_eq!(request_record.kind, TraceKind::Request);
    let interest = [request_record.position];
    let baseline = projection.refresh(&interest, None).unwrap();
    assert_eq!(baseline[0].state, TraceState::Completed);
    assert_eq!(baseline[0].request.as_ref().unwrap().usage, Some(usage));
    let connection = rusqlite::Connection::open(&path).unwrap();
    for (column, request_id) in [
        ("observation_summary", &second.request_id),
        ("observation_summary", &first.request_id),
        ("observation_context_total", &second.request_id),
    ] {
        let sql = format!("SELECT {column} FROM request_snapshots WHERE request_id=?1");
        let original: rusqlite::types::Value = connection
            .query_row(&sql, [request_id.as_str()], |row| row.get(0))
            .unwrap();
        assert_eq!(
            connection
                .execute(
                    &format!("UPDATE request_snapshots SET {column}=NULL WHERE request_id=?1"),
                    [request_id.as_str()]
                )
                .unwrap(),
            1
        );
        assert_eq!(projection.refresh(&interest, None).unwrap(), baseline);
        assert!(matches!(
            projection.page(None, 1),
            Err(ConversationStoreError::Storage(_))
        ));
        assert_eq!(
            connection
                .execute(
                    &format!("UPDATE request_snapshots SET {column}=?1 WHERE request_id=?2"),
                    rusqlite::params![original, request_id.as_str()]
                )
                .unwrap(),
            1
        );
    }
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_eq!(projection.refresh(&interest, None).unwrap(), baseline);
}

#[test]
fn t06_t08_t14_trace_refresh_covers_every_admitted_interest_and_rejects_ninth() {
    let store = store("conv_40164b38-6f23-7e6f-9a23-b2906f6c2aa8");
    start(&store);
    for retry in 0..8 {
        let snapshot = request(&store, retry);
        completion(
            &store,
            &snapshot,
            Some(ModelUsage {
                input_tokens: Some(u64::MAX),
                output_tokens: Some(0),
                total_tokens: Some(u64::MAX),
                details: None,
            }),
            None,
            i64::from(retry) + 4,
        );
    }
    let projection = TraceProjection::new(&store).unwrap();
    let mut page = projection.page(None, 8).unwrap();
    let interests: Vec<_> = page
        .records
        .iter()
        .map(|record| record.position.clone())
        .collect();
    assert_eq!(interests.len(), TRACE_RECORD_LIMIT);
    page.updates = projection.refresh(&interests, None).unwrap();
    assert_eq!(page.updates.len(), interests.len());
    for (interest, update) in interests.iter().zip(&page.updates) {
        assert_eq!(update.id, interest.0);
        assert_eq!(update.state, TraceState::Completed);
        assert_eq!(
            update
                .request
                .as_ref()
                .unwrap()
                .usage
                .as_ref()
                .unwrap()
                .input_tokens,
            Some(u64::MAX)
        );
        assert_eq!(
            update.fields,
            page.records
                .iter()
                .find(|record| &record.position == interest)
                .unwrap()
                .fields
        );
    }
    super::super::ensure_page_bound(&page).unwrap();
    let mut too_many = interests;
    too_many.push(TraceCursor::at(1));
    assert!(matches!(
        projection.refresh(&too_many, None),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}
