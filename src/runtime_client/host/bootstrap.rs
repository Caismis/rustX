//! Live projection bootstrap from the conversation's durable authorities.
use super::sources::source_value;
use super::sources::weighted_sources;
use super::{
    Arc, ConversationRuntime, ConversationStore, HostConstructionError, PendingObservations,
    RuntimeBootstrapError, RuntimeClientProjection, durable_bootstrap,
};
use crate::runtime_client::types::RuntimeClientError;

/// Builds the ordinary Runtime Client projection from one conversation's
/// durable authorities. Durable journal facts update the read model only;
/// they do not become live Runtime Client events or consume the live cursor
/// domain.
pub(super) fn durable_projection(
    store: &dyn ConversationStore,
    replay_limit: usize,
) -> Result<RuntimeClientProjection, HostConstructionError> {
    let conversation_id = store.conversation_id().clone();
    let source_cut = store
        .observation_read_cut()
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let through = source_cut.read.journal;
    let auxiliary_total = store
        .auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut { journal: through })
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    if auxiliary_total > 9_007_199_254_740_991 {
        return Err(HostConstructionError::Durable(
            "auxiliary audit total exceeds the protocol integer domain".into(),
        ));
    }
    let capabilities = crate::observation::capability_source::CapabilityView {
        configured_tools: Vec::new(),
        revision: crate::runtime::identity::CapabilityRevision::new(0),
        tools: Vec::new(),
        available_tools: Vec::new(),
        skills: Vec::new(),
        sources: Vec::new(),
    };
    let mut projection = RuntimeClientProjection::new(
        conversation_id,
        Vec::new(),
        capabilities,
        None,
        replay_limit,
    );
    projection.set_auxiliary_audit_total(auxiliary_total);
    projection.set_settings_evidence(super::super::settings::SettingsEvidence::HistoricalPartial);
    // No runtime stands behind a durable projection, so there is no
    // authoritative Agent composition to project: `effective_plugins`
    // stays absent (Issue #256). It is deliberately *not* reconstructed from
    // the configuration document on disk today, from built-in defaults, or
    // from the Agent Status observations this journal replay does install —
    // historical evidence stays honest about what it cannot know.
    projection
        .initialize_descriptor_heads(store, source_cut.clone())
        .map_err(|error| HostConstructionError::Durable(format!("{error:?}")))?;
    let reading = durable_bootstrap::read(store, &source_cut)
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    projection.bootstrap_durable_reading(reading.attempt, reading.context, reading.tool_content);

    projection.set_inspection_frontier(through);
    // This local constructor has no worker. Resolve its bounded read domains
    // at the same native cut before the caller freezes the published seed.
    projection.initialize_read_domains(store);
    let references = projection
        .snapshot_ref()
        .messages
        .iter()
        .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
        .flat_map(|message| super::super::checkpoint::message_metadata(store, message))
        .collect();
    projection.install_content_metadata(references);
    Ok(projection)
}
pub(super) fn bootstrap_live_projection(
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    store: &dyn ConversationStore,
    replay_limit: usize,
) -> RuntimeClientProjection {
    seed.model.prepare();
    let resource_source =
        crate::observation::resource_source::ResourceReadSource::new(Arc::clone(&seed.resources));
    resource_source.prepare();
    let capability_source = crate::observation::capability_source::CapabilityReadSource::new(
        Arc::clone(&seed.capabilities),
        seed.capability_availability.clone(),
        seed.model.capabilities().cloned(),
    );
    capability_source.prepare();
    let mut projection = RuntimeClientProjection::new(
        seed.conversation_id.clone(),
        {
            let mut capture = crate::observation::capture::Capture::new(0);
            capture.message_page("messages", &seed.messages, &seed.conversation_id)
        },
        capability_source.header(),
        seed.model.inline(),
        replay_limit,
    );
    let auxiliary_journal = seed
        .observation_cut
        .as_ref()
        .map_or(seed.journal_through, |cut| cut.read.journal);
    if let Ok(total) = store.auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut {
        journal: auxiliary_journal,
    }) && total <= 9_007_199_254_740_991
    {
        projection.set_auxiliary_audit_total(total);
    }
    install_bootstrap_sources(&mut projection, seed, &resource_source, &capability_source);
    projection.bootstrap(seed);
    install_bootstrap_frontiers(&mut projection, seed, &resource_source, &capability_source);
    if seed.source_failure.is_none()
        && let Some(cut) = seed.observation_cut.clone()
    {
        let initialized = durable_bootstrap::read_latest_compaction(store, &cut)
            .map_err(RuntimeClientError::observation_store_error)
            .and_then(|compaction| {
                projection.bootstrap_compaction(compaction);
                projection.initialize_descriptor_heads(store, cut)
            });
        if let Err(error) = initialized {
            projection.mark_observation_unavailable(format!("{error:?}"));
        } else {
            projection.initialize_read_domains(store);
        }
    }
    if let Ok(checkpoint) = projection.checkpoint_candidate() {
        projection.install_domain_sources(weighted_sources(vec![(
            "context".into(),
            Arc::new(super::super::domain::DomainSource::Context(source_value(
                checkpoint.snapshot.context,
            ))),
        )]));
    }
    let references = seed
        .messages
        .iter()
        .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
        .flat_map(|message| super::super::checkpoint::message_metadata(store, message))
        .collect();
    projection.install_content_metadata(references);
    projection
}
pub(super) fn install_bootstrap_sources(
    projection: &mut RuntimeClientProjection,
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    resource_source: &crate::observation::resource_source::ResourceReadSource,
    capability_source: &crate::observation::capability_source::CapabilityReadSource,
) {
    projection.install_domain_sources(weighted_sources(vec![
        (
            "resources".into(),
            Arc::new(super::super::domain::DomainSource::Resources(source_value(
                resource_source.clone(),
            ))),
        ),
        (
            "model_catalog".into(),
            Arc::new(super::super::domain::DomainSource::Catalog(source_value(
                seed.model.catalog_source(),
            ))),
        ),
        (
            "model".into(),
            Arc::new(super::super::domain::DomainSource::Model(source_value(
                seed.model.clone(),
            ))),
        ),
        (
            "capabilities".into(),
            Arc::new(super::super::domain::DomainSource::Capability(
                source_value(capability_source.clone()),
            )),
        ),
    ]));
    if let Some(goal) = &seed.goal {
        projection.install_domain_sources(weighted_sources(vec![(
            "goal".into(),
            Arc::new(super::super::domain::DomainSource::Goal(source_value(
                goal.clone(),
            ))),
        )]));
    }
    if let Some(todos) = &seed.todos {
        projection.install_domain_sources(weighted_sources(vec![(
            "todos".into(),
            Arc::new(super::super::domain::DomainSource::Todo(source_value(
                todos.clone(),
            ))),
        )]));
    }
}
pub(super) fn install_bootstrap_frontiers(
    projection: &mut RuntimeClientProjection,
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    resource_source: &crate::observation::resource_source::ResourceReadSource,
    capability_source: &crate::observation::capability_source::CapabilityReadSource,
) {
    projection.install_source_frontiers(capability_source.frontiers());
    projection.install_source_frontiers(vec![crate::observation::cut::CollectionFrontier {
        domain: "model_catalog.models".into(),
        revision: 0,
        total: seed.model.catalog_source().model_count() as u64,
        loaded: 0,
        next_offset: (seed.model.catalog_source().model_count() > 0).then_some(0),
    }]);
    projection.install_source_frontiers(resource_source.frontiers());
    projection.install_source_frontiers(vec![crate::observation::cut::CollectionFrontier {
        domain: "model".into(),
        revision: 0,
        total: 1,
        loaded: u64::from(seed.model.inline().is_some()),
        next_offset: seed.model.inline().is_none().then_some(0),
    }]);
    projection.install_resource_header(resource_source.header());
}
pub(super) fn install_bootstrap_bridge(
    runtime: &ConversationRuntime,
    pending: &Arc<PendingObservations>,
) -> Result<crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot, HostConstructionError> {
    runtime
        .install_client_observation_bridge(Arc::clone(pending))
        .map_err(|error| {
            runtime.release_client_binding();
            match error {
                RuntimeBootstrapError::BridgeAlreadyInstalled { conversation_id } => {
                    HostConstructionError::ObservationBridgeAlreadyInstalled { conversation_id }
                }
                RuntimeBootstrapError::RuntimeAlreadyActivated { conversation_id } => {
                    HostConstructionError::RuntimeAlreadyActivated { conversation_id }
                }
                RuntimeBootstrapError::Durable(message) => HostConstructionError::Durable(message),
            }
        })
}
