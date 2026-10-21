//! Preparation of display domain sources from native observations, outside the
//! projection synchronization boundary.
use super::{
    Arc, ClientState, ConversationObservation, ConversationStore, MessageId, Mutex, background_view,
};

/// No Store operation occurs under the native Projection synchronization
/// boundary. Preparation serializes drain order; every prepared observation
/// still folds and publishes only under the one Projection lock.
pub(super) fn source_value<T>(value: T) -> Arc<super::super::domain::SourceValue<T>> {
    Arc::new(super::super::domain::SourceValue::new(Arc::new(value)))
}
pub(super) fn prepare_display_sources(
    observation: &mut ConversationObservation,
    model_capabilities: &mut Option<crate::model::catalog::ModelCapabilities>,
    capability_source: &mut Option<crate::observation::capability_source::CapabilityReadSource>,
) {
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            prepare_display_sources(observation, model_capabilities, capability_source);
        }
        ConversationObservation::ObservationBatch(values)
        | ConversationObservation::JournalBatch {
            observations: values,
            ..
        } => {
            for value in values {
                prepare_display_sources(value, model_capabilities, capability_source);
            }
        }
        ConversationObservation::Capability { source } => {
            *source = source.with_model(model_capabilities.clone());
            source.prepare();
            *capability_source = Some(source.clone());
        }
        ConversationObservation::Resources {
            model,
            capability,
            display,
            ..
        } => {
            model.prepare();
            *model_capabilities = model.capabilities().cloned();
            capability.prepare();
            *capability_source = Some(capability.clone());
            display.prepare();
        }
        ConversationObservation::SessionModelChanged { model, capability } => {
            model.prepare();
            *model_capabilities = model.capabilities().cloned();
            *capability = capability_source
                .as_ref()
                .map(|source| source.with_model(model_capabilities.clone()));
            if let Some(source) = capability.as_ref() {
                source.prepare();
            }
            capability_source.clone_from(capability);
        }
        ConversationObservation::AttemptAdmitted { model, .. } => model.prepare(),
        ConversationObservation::Workflow(cuts) => {
            for source in cuts {
                source.prepare();
            }
        }
        _ => {}
    }
}
/// These callbacks carry complete immutable owner receipts and no J debt.
/// Registry callbacks hold their native owner lock: this selection must never
/// invoke Registry reads or source preparation that reacquires that owner.
pub(super) fn native_actor_transition(observation: &ConversationObservation) -> bool {
    match observation {
        ConversationObservation::Published { observation, .. } => {
            native_actor_transition(observation)
        }
        ConversationObservation::AttemptAdmitted { .. }
        | ConversationObservation::CanonicalCommitted { .. }
        | ConversationObservation::SessionModelChanged { .. }
        | ConversationObservation::Capability { .. }
        | ConversationObservation::Resources { .. }
        | ConversationObservation::GoalChanged(_)
        | ConversationObservation::Workflow(_)
        | ConversationObservation::PendingHead { .. }
        | ConversationObservation::InteractionPending { .. }
        | ConversationObservation::InteractionSettled { .. } => true,
        #[cfg(test)]
        ConversationObservation::InteractionRemoved { .. } => true,
        ConversationObservation::CollectionProof {
            owner, observation, ..
        } => {
            matches!(
                (*owner, observation.as_ref()),
                ("agents", ConversationObservation::SubagentLifecycle { .. })
                    | ("jobs", ConversationObservation::Background(_))
            )
        }
        ConversationObservation::ObservationBatch(items) => {
            !items.is_empty() && items.iter().all(native_actor_transition)
        }
        // Durable transitions are discharged by their indexed source proof,
        // and disposable progress/activity remains in the existing finite lanes.
        _ => false,
    }
}
pub(super) fn weighted_sources(
    sources: Vec<(String, Arc<super::super::domain::DomainSource>)>,
) -> Vec<(String, Arc<super::super::domain::DomainSource>, usize)> {
    sources
        .into_iter()
        .map(|(key, source)| {
            let weight = source.encoded_weight();
            (key, source, weight)
        })
        .collect()
}
pub(super) fn observation_domain_sources(
    observation: &ConversationObservation,
) -> Vec<(String, Arc<super::super::domain::DomainSource>)> {
    use super::super::domain::DomainSource;
    let pair = |key: String, source| (key, Arc::new(source));
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            observation_domain_sources(observation)
        }
        ConversationObservation::ObservationBatch(observations)
        | ConversationObservation::JournalBatch { observations, .. } => observations
            .iter()
            .flat_map(observation_domain_sources)
            .collect(),
        ConversationObservation::GoalChanged(view) => vec![pair(
            "goal".into(),
            DomainSource::Goal(source_value(view.clone())),
        )],
        ConversationObservation::AttemptAdmitted { model, .. } => vec![pair(
            "attempt.model".into(),
            DomainSource::AttemptModel(source_value(model.clone())),
        )],
        ConversationObservation::SessionModelChanged { model, capability } => {
            let mut sources = vec![pair(
                "model".into(),
                DomainSource::Model(source_value(model.clone())),
            )];
            if let Some(capability) = capability {
                sources.push(pair(
                    "capabilities".into(),
                    DomainSource::Capability(source_value(capability.clone())),
                ));
            }
            sources
        }
        ConversationObservation::Capability { source } => vec![pair(
            "capabilities".into(),
            DomainSource::Capability(source_value(source.clone())),
        )],
        ConversationObservation::Resources {
            model,
            capability,
            display,
            ..
        } => vec![
            pair(
                "resources".into(),
                DomainSource::Resources(source_value(display.clone())),
            ),
            pair(
                "model".into(),
                DomainSource::Model(source_value(model.clone())),
            ),
            pair(
                "capabilities".into(),
                DomainSource::Capability(source_value(capability.clone())),
            ),
            pair(
                "model_catalog".into(),
                DomainSource::Catalog(source_value(model.catalog_source())),
            ),
        ],
        ConversationObservation::Background(job) => {
            let view = background_view(job);
            vec![pair(
                format!("jobs.{}", view.job_id),
                DomainSource::Job(source_value(view)),
            )]
        }
        ConversationObservation::SubagentLifecycle {
            agent: Some(agent),
            snapshot,
        } => {
            let view = super::super::projection::views::agent_view(agent, snapshot);
            vec![pair(
                format!("agents.{}", view.agent_id),
                DomainSource::Agent(source_value(view)),
            )]
        }
        _ => Vec::new(),
    }
}
pub(super) fn activity_domain_sources(
    state: &Mutex<ClientState>,
    observation: &ConversationObservation,
    prepared: &[(String, Arc<super::super::domain::DomainSource>)],
) -> Vec<(String, Arc<super::super::domain::DomainSource>)> {
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            activity_domain_sources(state, observation, prepared)
        }
        ConversationObservation::ObservationBatch(observations)
        | ConversationObservation::JournalBatch { observations, .. } => observations
            .iter()
            .flat_map(|observation| activity_domain_sources(state, observation, prepared))
            .collect(),
        ConversationObservation::SubagentActivity(snapshot) => {
            let key = format!("agents.{}", snapshot.child_agent_id);
            let existing = prepared
                .iter()
                .rev()
                .find(|(candidate, _)| candidate == &key)
                .map(|(_, source)| source.clone())
                .or_else(|| {
                    state
                        .lock()
                        .expect("native projection lock")
                        .projection
                        .domain_source(&key, 0)
                });
            let Some(existing) = existing else {
                return Vec::new();
            };
            let super::super::domain::DomainSource::Agent(agent) = existing.as_ref() else {
                return Vec::new();
            };
            let agent = agent.value();
            if agent.activation_id != snapshot.subagent_id {
                return Vec::new();
            }
            let view = super::super::projection::views::activation_view(
                snapshot,
                agent.state,
                agent.current_activation.clone(),
                &agent.title,
            );
            vec![(
                key,
                Arc::new(super::super::domain::DomainSource::Agent(source_value(
                    view,
                ))),
            )]
        }
        _ => Vec::new(),
    }
}
#[cfg(test)]
pub(super) fn observation_terminal_positions(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
    through: Option<u64>,
) -> Vec<(
    crate::runtime::identity::AttemptId,
    crate::durable::TranscriptCursor,
)> {
    match observation {
        ConversationObservation::Published {
            journal_sequence,
            observation,
        } => observation_terminal_positions(store, observation, Some(*journal_sequence)),
        ConversationObservation::JournalBatch {
            observations,
            through,
        } => observations
            .iter()
            .flat_map(|observation| observation_terminal_positions(store, observation, *through))
            .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
            .collect(),
        ConversationObservation::JournalFact(witness)
            if witness.event_kind.starts_with("attempt_")
                && witness.event_kind != "attempt_started" =>
        {
            witness
                .attempt_id
                .as_ref()
                .and_then(|attempt| {
                    store
                        .observation_attempt_summary(attempt, witness.sequence)
                        .ok()
                        .map(|summary| (attempt, summary))
                })
                .and_then(|(attempt, summary)| {
                    summary
                        .terminal_event_id
                        .and_then(|identity| {
                            store.event_transcript_cursor(&identity).ok().flatten()
                        })
                        .map(|cursor| (attempt.clone(), cursor))
                })
                .into_iter()
                .collect()
        }
        #[cfg(test)]
        ConversationObservation::Event { attempt_id, event }
            if super::super::projection::views::is_attempt_terminal(event) =>
        {
            through
                .and_then(|through| store.observation_attempt_summary(attempt_id, through).ok())
                .and_then(|summary| summary.terminal_event_id)
                .and_then(|identity| store.event_transcript_cursor(&identity).ok().flatten())
                .map(|cursor| (attempt_id.clone(), cursor))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}
pub(super) fn observation_headers(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
) -> Vec<(
    MessageId,
    crate::durable::observation::MessageHeader,
    Option<crate::conversation::SurfaceRevision>,
)> {
    let _ = store; // The direct Committed source seam exists only in cfg(test).
    observation
        .facts()
        .flat_map(|fact| match fact {
            ConversationObservation::CanonicalCommitted { receipt, .. } => receipt
                .observation
                .head
                .iter()
                .filter_map(|header| {
                    header.identity.as_ref().map(|identity| {
                        (
                            identity.clone(),
                            header.clone(),
                            Some(receipt.observation.read_cut.surface_revision),
                        )
                    })
                })
                .collect::<Vec<_>>(),
            #[cfg(test)]
            ConversationObservation::Committed { block, .. } => store
                .message_header(block.id())
                .ok()
                .flatten()
                .map(|header| {
                    let revision = store.message_append_revision(block.id()).ok().flatten();
                    (block.id().clone(), header, revision)
                })
                .into_iter()
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
        .collect()
}
