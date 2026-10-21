//! Pure views of native registry snapshots (background, agents, interactions, status window).
#[cfg(test)]
use super::Arc;
#[cfg(test)]
use super::CapabilityView;
#[cfg(test)]
use super::RuntimeEvent;
use super::{
    AGENT_STATUS_WINDOW, AgentStatusView, AttemptFailure, BackgroundExecutionSnapshot,
    ConversationId, InboundItem, InboundItemView, MessageBlock, RuntimeClientAttemptFailure,
    RuntimeClientCursor, RuntimeClientEvent, RuntimeClientJob, bounded_diagnostic,
};

/// Whether the native event settles its attempt.
#[cfg(test)]
pub(crate) fn is_attempt_terminal(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    )
}
pub(super) fn accumulate_finite_facts(
    buffer: &mut (Vec<RuntimeClientEvent>, usize, bool),
    event: RuntimeClientEvent,
) {
    if buffer.2 {
        return;
    }
    let events = match event {
        RuntimeClientEvent::ProjectionTransition { events, .. } => events,
        event => vec![event],
    };
    if buffer.0.len() + events.len() > crate::observation::bounded::CHECKPOINT_PAGE_ITEMS {
        buffer.0.clear();
        buffer.2 = true;
    } else {
        buffer.0.extend(events);
    }
}
#[cfg(test)]
pub(super) fn accumulate_transition(
    buffer: &mut (Vec<RuntimeClientEvent>, usize, bool),
    event: RuntimeClientEvent,
) {
    if buffer.2 {
        return;
    }
    let count = match &event {
        RuntimeClientEvent::ProjectionTransition { events, .. } => events.len(),
        _ => 1,
    };
    let bytes = crate::observation::bounded::bounded_bytes(
        &event,
        crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 2,
    );
    if let Some(bytes) = bytes
        && buffer.0.len() + count <= crate::observation::bounded::CHECKPOINT_PAGE_ITEMS
        && buffer.1 + bytes.len() <= crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 2
    {
        buffer.1 += bytes.len();
        match event {
            RuntimeClientEvent::ProjectionTransition { events, .. } => buffer.0.extend(events),
            event => buffer.0.push(event),
        }
    } else {
        buffer.0.clear();
        buffer.2 = true;
    }
}
pub(super) fn set_checkpoint_transition_cursor(
    event: &mut RuntimeClientEvent,
    cursor: RuntimeClientCursor,
) {
    match event {
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            checkpoint.cursor = cursor;
            checkpoint.read_cut.revision = cursor.get();
        }
        RuntimeClientEvent::ProjectionTransition { events, .. } => {
            for event in events {
                set_checkpoint_transition_cursor(event, cursor);
            }
        }
        _ => {}
    }
}
pub(crate) fn client_failure(failure: &AttemptFailure) -> RuntimeClientAttemptFailure {
    crate::observation::failure::finite_attempt_failure(failure).0
}
/// One pending inbound item of the diagnostics view.
pub(super) fn inbound_item_view(item: &InboundItem) -> InboundItemView {
    let copied = crate::observation::bounded::bounded_clone(
        item.message(),
        crate::observation::bounded::CHECKPOINT_ITEM_BYTES,
    );
    let deferred = copied.is_none();
    let message = copied.unwrap_or_else(|| {
        let mut capture = crate::observation::capture::Capture::new(0);
        let MessageBlock::User(message) = capture.user_message(
            item.message(),
            &ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            &[],
        ) else {
            unreachable!("native inbound is User")
        };
        message
    });
    InboundItemView {
        revision: item.revision(),
        sequence: item.sequence(),
        deferred,
        message,
    }
}
/// Projects one authoritative background registry snapshot into the
/// external Runtime Client shape.
pub(crate) fn background_view(snapshot: &BackgroundExecutionSnapshot) -> RuntimeClientJob {
    let bash = snapshot
        .bash
        .as_deref()
        .and_then(|value| crate::observation::bounded::bounded_clone(value, 1024));
    let result = snapshot
        .result
        .as_deref()
        .and_then(|value| crate::observation::bounded::bounded_clone(value, 4096));
    let mut deferred_fields = Vec::new();
    if snapshot.bash.is_some() && bash.is_none() {
        deferred_fields.push(vec!["bash".into()]);
    }
    if snapshot.result.is_some() && result.is_none() {
        deferred_fields.push(vec!["result".into()]);
    }
    if snapshot.tool_name.len() > 256 {
        deferred_fields.push(vec!["tool_name".into()]);
    }
    let tool_id = crate::observation::tool::inline_tool_id(&snapshot.tool_id);
    if tool_id.is_none() {
        deferred_fields.push(vec!["tool_id".into()]);
    }
    RuntimeClientJob {
        deferred_fields,
        bash,
        job_id: snapshot.execution_id.clone(),
        tool_id,
        tool_name: snapshot.tool_name.chars().take(256).collect(),
        state: snapshot.state,
        progress: snapshot.progress.clone(),
        result,
    }
}
pub(crate) fn background_source_view(
    value: &crate::tools::background::BackgroundObservationValue<'_>,
) -> RuntimeClientJob {
    let bash = value
        .bash
        .and_then(|value| crate::observation::bounded::bounded_clone(value, 1024));
    let result = value
        .result
        .and_then(|value| crate::observation::bounded::bounded_clone(value, 4096));
    let progress = value
        .progress
        .and_then(|value| crate::observation::bounded::bounded_clone(value, 1024));
    let mut deferred_fields = Vec::new();
    if value.bash.is_some() && bash.is_none() {
        deferred_fields.push(vec!["bash".into()]);
    }
    if value.result.is_some() && result.is_none() {
        deferred_fields.push(vec!["result".into()]);
    }
    if value.progress.is_some() && progress.is_none() {
        deferred_fields.push(vec!["progress".into()]);
    }
    if value.tool_name.len() > 256 {
        deferred_fields.push(vec!["tool_name".into()]);
    }
    let tool_id = crate::observation::tool::inline_tool_id(value.tool_id);
    if tool_id.is_none() {
        deferred_fields.push(vec!["tool_id".into()]);
    }
    RuntimeClientJob {
        deferred_fields,
        bash,
        result,
        progress,
        job_id: value.job_id.clone(),
        tool_id,
        tool_name: value.tool_name.chars().take(256).collect(),
        state: value.state,
    }
}
pub(crate) fn agent_source_view(
    value: &crate::runtime::subagent::AgentObservationValue<'_>,
) -> super::super::snapshot::RuntimeClientAgent {
    let profile = value
        .execution_profile
        .and_then(|profile| crate::observation::bounded::bounded_clone(profile, 2048));
    let observation = crate::observation::bounded::bounded_clone(value.observation, 1024);
    let mut deferred_fields = Vec::new();
    if value.title.len() > 256 {
        deferred_fields.push(vec!["title".into()]);
    }
    if value.detail.is_some_and(|detail| detail.len() > 256) {
        deferred_fields.push(vec!["detail".into()]);
    }
    if value.execution_profile.is_some() && profile.is_none() {
        deferred_fields.push(vec!["execution_profile".into()]);
    }
    if observation.is_none() {
        deferred_fields.push(vec!["observation".into()]);
    }
    super::super::snapshot::RuntimeClientAgent {
        deferred_fields,
        title: value.title.chars().take(256).collect(),
        parent_agent_id: value.parent_agent_id.clone(),
        activation_id: value.activation_id.clone(),
        agent_id: value.agent_id.clone(),
        child_conversation_id: value.child_conversation_id.clone(),
        agent: value.agent.into(),
        definition_digest: value.definition_digest.into(),
        profile_digest: value.profile_digest.into(),
        state: value.state,
        current_activation: value.current_activation.cloned(),
        activation_state: value.activation_state,
        detail: value.detail.map(bounded_diagnostic),
        observation: observation.unwrap_or_default(),
        execution_profile: profile,
        started_at: value.started_at,
        workspace: value.workspace.into(),
    }
}
/// Builds the deterministic active capability projection from the
/// authoritative capability snapshot and the coordinator-owned
/// availability state (Issue #81).
///
/// The tool catalog preserves registry order; the Skill catalog is
/// ordered by Skill name (the two snapshot lists derive from one sorted
/// package order); the source list preserves the deterministic
/// source-identity order of the authoritative availability map. No
/// executors, environment paths, or dependency internals ever appear.
#[cfg(test)]
pub(crate) fn capability_view(
    snapshot: &crate::capabilities::CapabilitySnapshot,
    availability: &crate::capabilities::CapabilityAvailability,
    model: Option<&crate::model::catalog::ModelCapabilities>,
) -> CapabilityView {
    let source = crate::observation::capability_source::CapabilityReadSource::new(
        Arc::new(snapshot.clone()),
        availability.clone(),
        model.cloned(),
    );
    source.prepare();
    source.header()
}
/// Inserts or replaces one background view, preserving execution
/// allocation order.
pub(super) fn upsert_background(
    background: &mut Vec<RuntimeClientJob>,
    view: RuntimeClientJob,
) -> Option<crate::runtime::identity::ToolExecutionId> {
    if let Some(existing) = background
        .iter_mut()
        .find(|entry| entry.job_id == view.job_id)
    {
        *existing = view;
        return None;
    }
    background.insert(0, view);
    (background.len() > 32).then(|| background.pop().unwrap().job_id)
}
/// Projects one authoritative subagent registry snapshot into the external
/// Runtime Client shape (Issue #60).
pub(crate) fn agent_view(
    agent: &crate::runtime::subagent::AgentSnapshot,
    activation: &crate::runtime::subagent::SubagentSnapshot,
) -> super::super::snapshot::RuntimeClientAgent {
    activation_view(
        activation,
        agent.state,
        agent.current_activation.clone(),
        &agent.title,
    )
}
/// Activation data enriches a view only with explicitly supplied Agent authority.
pub(crate) fn activation_view(
    snapshot: &crate::runtime::subagent::SubagentSnapshot,
    state: crate::runtime::subagent::AgentState,
    current_activation: Option<crate::runtime::identity::SubagentId>,
    title: &str,
) -> super::super::snapshot::RuntimeClientAgent {
    super::super::snapshot::RuntimeClientAgent {
        deferred_fields: {
            let mut fields = Vec::new();
            if title.len() > 256 {
                fields.push(vec!["title".into()]);
            }
            if snapshot
                .detail
                .as_deref()
                .is_some_and(|detail| detail.len() > 256)
            {
                fields.push(vec!["detail".into()]);
            }
            if snapshot.profile.as_deref().is_some_and(|profile| {
                crate::observation::bounded::bounded_bytes(profile, 2048).is_none()
            }) {
                fields.push(vec!["execution_profile".into()]);
            }
            fields
        },
        title: title.chars().take(256).collect(),
        parent_agent_id: snapshot.parent_agent_id.clone(),
        activation_id: snapshot.subagent_id.clone(),
        agent_id: snapshot.child_agent_id.clone(),
        child_conversation_id: snapshot.child_conversation_id.clone(),
        agent: snapshot.agent.clone(),
        definition_digest: snapshot.definition_digest.clone(),
        profile_digest: snapshot.profile_digest.clone(),
        state,
        current_activation,
        activation_state: snapshot.state,
        detail: snapshot.detail.as_deref().map(bounded_diagnostic),
        observation: snapshot.observation.clone(),
        execution_profile: snapshot
            .profile
            .as_deref()
            .and_then(|profile| crate::observation::bounded::bounded_clone(profile, 2048)),
        started_at: snapshot.started_at,
        workspace: snapshot.workspace.clone(),
    }
}
pub(super) fn upsert_subagent(
    subagents: &mut Vec<super::super::snapshot::RuntimeClientAgent>,
    seen: &mut std::collections::HashSet<crate::runtime::identity::SubagentId>,
    view: super::super::snapshot::RuntimeClientAgent,
) -> bool {
    if let Some(existing) = subagents
        .iter_mut()
        .find(|entry| entry.agent_id == view.agent_id)
    {
        if existing.activation_id != view.activation_id && seen.contains(&view.activation_id) {
            return false;
        }
        seen.insert(view.activation_id.clone());
        *existing = view;
    } else {
        seen.insert(view.activation_id.clone());
        subagents.insert(0, view);
        subagents.truncate(32);
    }
    seen.retain(|activation| {
        subagents
            .iter()
            .any(|agent| &agent.activation_id == activation)
    });
    true
}
/// Inserts or replaces one live native interaction projection, preserving
/// coordinator publication order. Identity never determines presentation order.
pub(super) fn upsert_interaction(
    interactions: &mut Vec<crate::observation::interaction::RuntimeClientInteractionHeader>,
    request: crate::observation::interaction::RuntimeClientInteractionHeader,
) {
    if let Some(existing) = interactions
        .iter_mut()
        .find(|existing| existing.interaction == request.interaction)
    {
        *existing = request;
    } else {
        interactions.push(request);
    }
}
/// Admits one composed status into the bounded projection window and returns
/// the composition that admission pushed out, when the window was full.
///
/// Identity is the composed status message: re-observing the same
/// composition is idempotent rather than a second historical fact, and an
/// admission that does nothing evicts nothing.
///
/// This is the **only** retention decision in the system. The returned
/// eviction is published with the composition as one transition, so a client
/// folding incrementally reproduces this window without ever knowing
/// [`AGENT_STATUS_WINDOW`]. Because the window is only ever grown by one
/// before it is trimmed, one admission can retire at most one composition,
/// and the transition is fully described by a single optional identity.
pub(super) fn admit_status(
    statuses: &mut Vec<AgentStatusView>,
    view: AgentStatusView,
) -> Option<AgentStatusView> {
    if statuses.iter().any(|existing| {
        if let Some(locator) = &view.source_locator {
            existing.source_locator.as_ref() == Some(locator)
        } else {
            view.status_message_id.is_some() && existing.status_message_id == view.status_message_id
        }
    }) {
        return None;
    }
    statuses.push(view);
    if statuses.len() > AGENT_STATUS_WINDOW {
        return Some(statuses.remove(0));
    }
    None
}

pub(crate) fn checkpoint_view(
    store: &dyn crate::durable::ObservationStore,
    checkpoint: &crate::durable::projection::ProjectionCheckpoint,
) -> Result<
    crate::observation::head::RuntimeClientProjectionHeadView,
    crate::durable::ConversationStoreError,
> {
    let summary_preview = checkpoint
        .candidate
        .items
        .iter()
        .find_map(|item| match &item.message {
            crate::context::checkpoint::CheckpointMessage::Derived(derived)
                if derived
                    .transformations
                    .contains(&crate::model::projected::SemanticTransformation::Summary) =>
            {
                match &derived.message {
                    crate::model::projected::ProjectedMessage::User { content, .. } => {
                        content.iter().find_map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) => {
                                Some(crate::observation::trace::TraceText::detail(&text.text))
                            }
                            _ => None,
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        });
    let mut head = store.observation_projection_activation(checkpoint.activation.activation)?;
    head.summary_preview = summary_preview;
    Ok(head)
}
