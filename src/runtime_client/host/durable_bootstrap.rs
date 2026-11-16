//! Constructor-only finite historical reading, with no live publication or execution owner.
use crate::durable::observation::{
    ObservationReadCut, TraceBootstrapFact as B, TraceFactData as D, TraceFactHeader,
    TraceFactKind as K, TraceFactQuery, TraceFactScope,
};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::observation::head::RuntimeClientCompactionView;
use crate::runtime_client::event::RuntimeClientOutcome;
use crate::runtime_client::projection::PreparedDurableToolReading;
use crate::runtime_client::snapshot::{
    RuntimeClientAttempt, RuntimeClientAttemptPhase, RuntimeClientContextView,
};

pub(super) struct DurableReading {
    pub attempt: Option<RuntimeClientAttempt>,
    pub context: RuntimeClientContextView,
    pub tool_content: Vec<PreparedDurableToolReading>,
}
fn latest(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
    kinds: Vec<K>,
    scope: TraceFactScope,
    limit: usize,
) -> Result<Vec<TraceFactHeader>, ConversationStoreError> {
    store.observation_trace_facts(
        cut,
        &TraceFactQuery {
            scope,
            kinds,
            before: None,
            after: 0,
            ascending: false,
            limit,
        },
    )
}
fn same_attempt(
    left: &crate::observation::trace::TraceLocation,
    right: &crate::observation::trace::TraceLocation,
) -> bool {
    left.attempt_id
        .as_ref()
        .is_some_and(|id| right.attempt_id.as_ref() == Some(id))
        || left
            .attempt_id_source
            .as_ref()
            .is_some_and(|source| right.attempt_id_source.as_ref() == Some(source))
}
pub(super) fn read(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
) -> Result<DurableReading, ConversationStoreError> {
    let mut reading = DurableReading {
        attempt: None,
        context: RuntimeClientContextView::default(),
        tool_content: Vec::new(),
    };
    read_compaction(store, cut, &mut reading.context)?;
    let Some(start) = latest(store, cut, vec![K::AttemptStarted], TraceFactScope::All, 1)?.pop()
    else {
        return Ok(reading);
    };
    let summary = store.observation_execution_at(cut, start.sequence)?;
    let mut attempt = RuntimeClientAttempt {
        attempt_id: start.location.attempt_id.clone(),
        attempt_id_source: start.location.attempt_id_source.clone(),
        phase: RuntimeClientAttemptPhase::Incomplete,
        turn: 0,
        last_usage: None,
        in_flight: None,
        foreground: Vec::new(),
        model: None,
        execution_settings: None,
    };
    if let Some(terminal) = latest(
        store,
        cut,
        vec![
            K::AttemptCompleted,
            K::AttemptFailed,
            K::AttemptCancelled,
            K::AttemptTimedOut,
            K::AttemptLimitExceeded,
        ],
        TraceFactScope::Owner {
            anchor_sequence: start.sequence,
        },
        1,
    )?
    .pop()
    {
        attempt.phase = RuntimeClientAttemptPhase::Settled {
            outcome: terminal_outcome(store, cut, &terminal, &summary)?,
        };
    }
    for kind in [K::TurnStarted, K::ModelRequestCompleted] {
        let scope = if kind == K::ModelRequestCompleted {
            TraceFactScope::Agent
        } else {
            TraceFactScope::All
        };
        if let Some(fact) = latest(store, cut, vec![kind], scope, 1)?.pop()
            && same_attempt(&start.location, &fact.location)
        {
            if let Some(B::Turn { number }) = fact.bootstrap {
                attempt.turn = number;
            }
            if let D::Terminal(terminal) = fact.fact {
                attempt.last_usage = terminal.usage;
            }
        }
    }
    read_tools(store, cut, &start, &mut attempt, &mut reading.tool_content)?;
    reading.attempt = Some(attempt);
    Ok(reading)
}
fn field(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
    sequence: u64,
    name: &str,
) -> Result<crate::observation::tool::JournalFieldRef, ConversationStoreError> {
    let path = vec!["event".into(), name.into()];
    let chunk = store.event_value_read(sequence, cut.read.journal, &path, 0, 1)?;
    Ok(crate::observation::tool::JournalFieldRef {
        sequence,
        path,
        encoded_bytes: chunk.readable_end,
    })
}
fn terminal_outcome(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
    terminal: &TraceFactHeader,
    summary: &crate::durable::observation::AttemptExecutionSummary,
) -> Result<RuntimeClientOutcome, ConversationStoreError> {
    let missing =
        || ConversationStoreError::Storage("historical native terminal reading unavailable".into());
    Ok(match terminal.kind {
        K::AttemptCompleted => RuntimeClientOutcome::Completed {
            finish_reason: summary.finish_reason.clone().ok_or_else(missing)?,
            finish_reason_deferred: summary.finish_reason_deferred,
            finish_reason_source: summary
                .finish_reason_deferred
                .then(|| field(store, cut, terminal.sequence, "finish_reason"))
                .transpose()?,
        },
        K::AttemptFailed => RuntimeClientOutcome::Failed {
            error: summary.failure.clone().ok_or_else(missing)?,
            error_deferred: summary.failure_deferred,
            error_source: if summary.failure_deferred {
                if summary.failure_readable_end == 0 {
                    return Err(missing());
                }
                Some(crate::observation::tool::JournalFieldRef {
                    sequence: terminal.sequence,
                    path: vec!["normalized_failure".into()],
                    encoded_bytes: summary.failure_readable_end,
                })
            } else {
                None
            },
        },
        K::AttemptTimedOut => RuntimeClientOutcome::TimedOut,
        K::AttemptCancelled => match terminal.bootstrap {
            Some(B::AttemptCancelled { reason }) => RuntimeClientOutcome::Cancelled { reason },
            _ => return Err(missing()),
        },
        K::AttemptLimitExceeded => match terminal.bootstrap {
            Some(B::AttemptLimited { limit }) => RuntimeClientOutcome::LimitExceeded { limit },
            _ => return Err(missing()),
        },
        _ => return Err(missing()),
    })
}
fn read_compaction(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
    context: &mut RuntimeClientContextView,
) -> Result<(), ConversationStoreError> {
    context.occupancy = store.observation_occupancy(cut.read.journal)?;
    // Progress is a bounded Journal observation, intentionally absent from
    // Trace's closed event vocabulary. Read the latest lifecycle/progress
    // witness directly at the same frozen Journal cut.
    let kinds = [
        "compaction_started",
        "compaction_chunk_accepted",
        "compaction_skipped",
        "compaction_cancelled",
        "compaction_failed",
        "compaction_completed",
    ]
    .map(str::to_owned);
    if let Some(latest) = store
        .latest_observation_events(cut.read.journal, &kinds, None, 1)?
        .pop()
    {
        let witness = latest.witness.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "compaction observation witness unavailable".into(),
            )
        })?;
        if witness.source_unavailable {
            return Err(ConversationStoreError::InvalidReference(
                "compaction observation witness unavailable".into(),
            ));
        }
        match witness.event_kind.as_str() {
            "compaction_started" | "compaction_chunk_accepted" => {
                let event: crate::events::types::RuntimeEvent =
                    serde_json::from_value(witness.value).map_err(|_| {
                        ConversationStoreError::InvalidReference(
                            "invalid compaction observation witness".into(),
                        )
                    })?;
                context.compaction_progress = match event {
                    crate::events::types::RuntimeEvent::CompactionStarted { operation_id } => Some(
                        crate::runtime_client::snapshot::RuntimeClientCompactionProgress {
                            operation_id,
                            accepted_fragments: 0,
                            source_ranges: Vec::new(),
                            generation: None,
                        },
                    ),
                    crate::events::types::RuntimeEvent::CompactionChunkAccepted {
                        operation_id,
                        accepted_fragments,
                        source_ranges,
                        generation,
                    } => Some(
                        crate::runtime_client::snapshot::RuntimeClientCompactionProgress {
                            operation_id,
                            accepted_fragments,
                            source_ranges,
                            generation,
                        },
                    ),
                    _ => {
                        return Err(ConversationStoreError::InvalidReference(
                            "invalid compaction observation family".into(),
                        ));
                    }
                };
                // This reader has only historical authorities and no execution
                // owner. An unfinished prefix is not proof that work survived
                // a process restart; live hosts receive owner publications.
                context.compaction_in_progress = false;
            }
            "compaction_failed" => {
                context.compaction_error = witness
                    .value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
            }
            _ => {}
        }
    }
    if let Some(compaction) = read_latest_compaction(store, cut)? {
        context.latest_compaction = Some(compaction);
        context.compaction_count = 1;
    }
    Ok(())
}

/// Reads one committed completion at the bootstrap cut without replaying live state.
pub(super) fn read_latest_compaction(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
) -> Result<Option<RuntimeClientCompactionView>, ConversationStoreError> {
    Ok(latest(
        store,
        cut,
        vec![K::CompactionCompleted],
        TraceFactScope::All,
        1,
    )?
    .pop()
    .and_then(|fact| match fact.bootstrap {
        Some(B::CompactionCompleted { reading }) => Some(reading.compaction),
        _ => None,
    }))
}
fn read_tools(
    store: &dyn ConversationStore,
    cut: &ObservationReadCut,
    start: &TraceFactHeader,
    attempt: &mut RuntimeClientAttempt,
    content: &mut Vec<PreparedDurableToolReading>,
) -> Result<(), ConversationStoreError> {
    for fact in latest(
        store,
        cut,
        vec![K::ToolExecutionStarted],
        TraceFactScope::All,
        32,
    )?
    .into_iter()
    .rev()
    {
        if !same_attempt(&start.location, &fact.location) {
            continue;
        }
        let D::ToolStart {
            occurrence: Some(occurrence),
            ..
        } = fact.fact
        else {
            return Err(ConversationStoreError::Storage(
                "historical Tool occurrence unavailable".into(),
            ));
        };
        let locator = crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
            position: occurrence.ledger_position,
        };
        let page = store.transcript_tool_headers(
            cut,
            &locator,
            u64::from(occurrence.block_index.get()),
            1,
        )?;
        let header = page
            .headers
            .first()
            .filter(|header| header.association.block_index == occurrence.block_index)
            .ok_or_else(|| {
                ConversationStoreError::Storage("historical Tool header unavailable".into())
            })?;
        let view = crate::observation::tool::foreground(&page, header).map_err(|_| {
            ConversationStoreError::Storage("historical Tool reading unavailable".into())
        })?;
        content.push(PreparedDurableToolReading {
            occurrence: view.occurrence.clone(),
            content: crate::observation::tool::content(header, &view),
            collections: crate::observation::tool::collections(header),
        });
        attempt.foreground.push(view);
    }
    Ok(())
}
