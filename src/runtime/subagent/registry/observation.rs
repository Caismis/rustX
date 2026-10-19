//! The registry read model: public snapshots and listings, observation cuts
//! and pages, live activity, and observer publication. Every value is captured
//! under the registry mutex; nothing here owns lifecycle authority.

use super::super::activity::SubagentExecutionProfile;
use super::super::activity::SubagentObservation;
use super::RegistryState;
use super::SubagentLifecycle;
use super::SubagentRecord;
use super::SubagentRegistry;
use super::SubagentTerminalMode;
use super::agents;
use super::agents::AgentSnapshot;
use super::interactions::ChildPendingHeadProof;
use super::preparation::AgentActivationOrigin;
use crate::events::types::SubagentOwnershipKind;
use crate::runtime::identity::AgentId;
use crate::runtime::identity::ConversationId;
use crate::runtime::identity::SubagentId;
use crate::runtime::interaction::InteractionOutcome;
use crate::runtime::interaction::InteractionRef;
use crate::runtime::types::CancellationReason;
use chrono::DateTime;
use chrono::Utc;
use std::sync::Arc;
use std::sync::PoisonError;

/// The public lifecycle vocabulary of one subagent snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum SubagentState {
    /// Message admission is closed while the activation drains and settles.
    Stopping,
    /// Ownership committed; the delegation is in flight or running.
    Running,
    /// Cancellation intent committed; escalation may be in flight.
    Cancelling,
    /// The terminal outcome is known; publication is not yet durable.
    PublishingTerminal,
    /// The result is durably published.
    Succeeded,
    /// The failure is durably published.
    Failed,
    /// The cancellation is durably published.
    Cancelled,
    /// The child process/control plane settled without a valid semantic
    /// terminal result; the child's outcome is unknown.
    Interrupted,
}

impl SubagentState {
    /// Whether this state is terminal (absorbing).
    ///
    /// This is the domain's own lifecycle classification, kept beside the
    /// vocabulary it classifies: `PublishingTerminal` is deliberately **not**
    /// terminal — the outcome is known but its durable publication has not
    /// committed, so the child still owns unfinished settlement work.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Whether this state is active (non-terminal).
    #[must_use]
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// A consistency snapshot of one subagent child.
///
/// Read-model materialization only: every field is derived from the
/// registry's state machine, never an authority of its own. This is the
/// **rich finite activation projection** consumed by runtime diagnostics and
/// Agent projections. Durable Agent identity and current state come from
/// `AgentSnapshot`; this type describes one exact activation's facts.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SubagentSnapshot {
    /// Only Normal activations belong to a durable native Agent.
    pub ownership: SubagentOwnershipKind,
    pub parent_agent_id: AgentId,
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity (provenance of its answer).
    pub child_agent_id: AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The delegating tool call.
    pub origin: AgentActivationOrigin,
    /// The canonical named-agent identity frozen at start (Issue #144).
    pub agent: String,
    /// The deterministic definition digest frozen at start (Issue #144).
    ///
    /// The snapshot reports the definition the child actually started with,
    /// so a configuration adoption that redefines the same agent name can never
    /// make an already-running child appear to have the new definition.
    pub definition_digest: String,
    /// The deterministic **effective execution profile** digest frozen at
    /// start (Issue #258).
    ///
    /// Two children of one named agent that were specialized differently by
    /// an authorized invocation override report the same `definition_digest`
    /// and different values here. It is committed durably with ownership, so
    /// a recovery-projected record reports exactly the value its child
    /// started with.
    ///
    /// This is diagnostic/recovery correlation only. No admission,
    /// resolution, or execution decision reads it.
    pub profile_digest: String,
    /// The invoking Session's execution binding frozen before ownership.
    pub workspace: std::path::PathBuf,
    /// The lifecycle state.
    pub state: SubagentState,
    /// The committed cancellation reason, while cancellation intent exists
    /// or the child settled cancelled with one.
    ///
    /// A child that reported `Cancelled` without a committed parent
    /// cancellation intent has no semantic reason (`None`): the runtime
    /// never fabricates one.
    pub cancel_reason: Option<CancellationReason>,
    /// The bounded failure/cancellation diagnostic, once known.
    ///
    /// A successful child's answer content never appears here (Issue #178):
    /// the durable terminal inbound publication is the one result channel,
    /// and the live observation/control projection carries diagnostics only.
    pub detail: Option<Arc<str>>,
    /// The latest live activity projection reported by the child (Issue
    /// #178). Observation-plane state only: it never changes lifecycle
    /// semantics, and every terminal settlement resets it to neutral.
    pub observation: SubagentObservation,
    /// The redacted execution profile frozen at child start (Issue #178);
    /// `None` for recovery-projected records.
    pub profile: Option<Arc<SubagentExecutionProfile>>,
    /// Terminal durability and physical containment are independent of the
    /// logical outcome in `state`. A committed terminal may remain physically
    /// unresolved; proven containment does not invent a logical outcome.
    pub settlement: SubagentSettlement,
    /// When the ownership committed.
    pub started_at: DateTime<Utc>,
}

/// Durable publication of an activation terminal and its required result value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentPublication {
    #[default]
    Pending,
    Committed,
    /// The bounded publication owner exhausted durability retries.
    Abandoned,
}

/// Evidence that the activation's physical incarnation no longer owns effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentPhysicalSettlement {
    #[default]
    Unproven,
    Proven,
}

/// The two independent settlement dimensions; logical lifecycle is `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct SubagentSettlement {
    pub publication: SubagentPublication,
    pub physical: SubagentPhysicalSettlement,
}

impl SubagentSnapshot {
    /// Full publication and containment proof, independent of the logical outcome.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.state.is_terminal()
            && self.settlement.publication == SubagentPublication::Committed
            && self.settlement.physical == SubagentPhysicalSettlement::Proven
    }
}

/// The subagent domain's own bounded discovery read model (Issue #180).
///
/// This type is owned by the subagent domain because everything in it is a
/// subagent-domain fact: which children the registry still knows, the
/// registry's own authoritative newest-first allocation order, which of
/// them match the requested lifecycle filter under
/// [`SubagentState::is_active`], and how many matched. Nothing here is a
/// model-facing concern, and the domain deliberately does **not** depend on
/// the model-facing `execution` control plane that consumes it: the
/// consumer knows the producer, never the other way round.
///
/// `snapshots` carries at most the caller's requested number of
/// authoritative snapshots, most recently started first, and `matched`
/// reports how many records matched the filter *before* that bound was
/// applied. Reporting `matched` separately is what keeps truncation honest:
/// a consumer can always tell a complete listing from a bounded prefix
/// without the registry ever materializing the whole set.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentListing {
    /// At most the requested number of authoritative snapshots, in the
    /// registry's newest-first authoritative order.
    pub snapshots: Vec<SubagentSnapshot>,
    /// How many records matched the filter in total, before the bound.
    pub matched: usize,
}

pub trait SubagentObserver: Send + Sync {
    /// One complete owner transition, captured under the registry lock.
    /// Workflow activations have no durable Agent owner.
    fn on_snapshot(
        &self,
        agent: Option<&AgentSnapshot>,
        snapshot: &SubagentSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    );
    /// One durable receipt releases the complete Agent and activation transition.
    fn on_committed(
        &self,
        agent: Option<&AgentSnapshot>,
        snapshot: &SubagentSnapshot,
        _committed: &crate::events::types::RuntimeEventEnvelope,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.on_snapshot(agent, snapshot, cut, ordinal);
    }

    /// Called under the registry lock with each new live-activity snapshot
    /// (Issue #178).
    ///
    /// This is a disposable, latest-value publication: the consumer may
    /// coalesce or drop intermediate values, and it must never treat an
    /// activity snapshot as lifecycle evidence. The default body forwards
    /// to [`on_snapshot`](Self::on_snapshot), so an observer that does not
    /// distinguish the two classes keeps capturing everything.
    fn on_activity(
        &self,
        snapshot: &SubagentSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.on_snapshot(None, snapshot, cut, ordinal);
    }

    /// Called under the registry lock when a child coordinator publishes a
    /// live interaction request. This is reliable semantic control, not
    /// disposable activity. Observers that only project lifecycle/activity
    /// state can leave this callback empty.
    fn on_interaction_pending(
        &self,
        _interaction: &crate::runtime_client::snapshot::RuntimeClientInteractionHeader,
        _proof: &ChildPendingHeadProof,
    ) {
    }

    /// Child-owned settlement carries a finite post-removal native head.
    fn on_interaction_settled(
        &self,
        _interaction: &InteractionRef,
        _outcome: &InteractionOutcome,
        _proof: &ChildPendingHeadProof,
    ) {
    }

    /// Bootstrap or physical Child retirement updates observation only.
    fn on_interaction_head(&self, _proof: &ChildPendingHeadProof) {}
}

/// The narrow live-activity sink the child driver task holds (Issue #178).
///
/// Cheaply cloneable; routes decoded activity frames into the registry's
/// read model synchronously (one brief lock acquisition, no await). It
/// carries no authority: it cannot touch lifecycle, journal, or mailbox
/// state, and every update passes through
/// [`SubagentRegistry::apply_activity`]'s drop rules.
#[derive(Clone)]
pub(crate) struct SubagentActivitySink {
    pub(super) subagent_id: SubagentId,
    pub(super) registry: SubagentRegistry,
}

impl SubagentActivitySink {
    /// Applies one decoded child activity projection.
    pub(crate) fn apply(&self, observation: SubagentObservation) {
        self.registry.apply_activity(&self.subagent_id, observation);
    }
}

/// Emits the record's snapshot to the observer and bumps the watch
/// version. Called under the registry lock.
pub(super) fn publish_committed_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
    committed: &crate::events::types::RuntimeEventEnvelope,
) {
    state
        .observation_layouts
        .insert(state.records[index].child_agent_id.clone(), Arc::default());
    state.observation_generation = state.observation_generation.saturating_add(1);
    let cut = SubagentRegistry::observation_cut_locked(state);
    if let Some(observer) = &state.observer {
        let snapshot = state.records[index].snapshot();
        let agent = SubagentRegistry::agent_snapshot_locked(state, &snapshot.child_agent_id);
        observer.on_committed(
            agent.as_ref(),
            &snapshot,
            committed,
            cut,
            SubagentRegistry::observation_ordinal_locked(state, &snapshot.child_agent_id),
        );
    }
    version.send_modify(|v| *v += 1);
}

pub(super) fn publish_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    state
        .observation_layouts
        .insert(state.records[index].child_agent_id.clone(), Arc::default());
    state.observation_generation = state.observation_generation.saturating_add(1);
    let cut = SubagentRegistry::observation_cut_locked(state);
    if let Some(observer) = &state.observer {
        let agent = SubagentRegistry::agent_snapshot_locked(state, &snapshot.child_agent_id);
        observer.on_snapshot(
            agent.as_ref(),
            &snapshot,
            cut,
            SubagentRegistry::observation_ordinal_locked(state, &snapshot.child_agent_id),
        );
    }
    version.send_modify(|v| *v += 1);
}

/// The disposable sibling of [`publish_snapshot`] for live-activity
/// updates (Issue #178): emits the record's snapshot through
/// [`SubagentObserver::on_activity`] — the publication the consumer may
/// coalesce or drop — and bumps the watch version. Called under the
/// registry lock.
fn publish_activity_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    state
        .observation_layouts
        .insert(state.records[index].child_agent_id.clone(), Arc::default());
    state.observation_generation = state.observation_generation.saturating_add(1);
    let cut = SubagentRegistry::observation_cut_locked(state);
    if let Some(observer) = &state.observer {
        observer.on_activity(
            &snapshot,
            cut,
            SubagentRegistry::observation_ordinal_locked(state, &snapshot.child_agent_id),
        );
    }
    version.send_modify(|v| *v += 1);
}

/// Borrowed fields of the native durable Agent owner and its exact activation.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct AgentObservationValue<'a> {
    pub title: &'a str,
    pub parent_agent_id: &'a AgentId,
    pub activation_id: &'a SubagentId,
    pub agent_id: &'a AgentId,
    pub child_conversation_id: &'a ConversationId,
    pub agent: &'a str,
    pub definition_digest: &'a str,
    pub profile_digest: &'a str,
    pub state: agents::AgentState,
    pub current_activation: Option<&'a SubagentId>,
    pub activation_state: SubagentState,
    pub detail: Option<&'a str>,
    pub observation: &'a SubagentObservation,
    pub execution_profile: Option<&'a SubagentExecutionProfile>,
    pub started_at: DateTime<Utc>,
    pub workspace: &'a std::path::Path,
}

#[derive(Clone)]
pub(crate) struct AgentObservationSource {
    pub(super) layout: crate::runtime::read_layout::ObservationLayoutCache,
    pub(super) authority: Arc<agents::DurableAgentAuthority>,
    profile: Option<Arc<SubagentExecutionProfile>>,
    title_agent_id: AgentId,
    parent_agent_id: AgentId,
    activation_id: SubagentId,
    child_conversation_id: ConversationId,
    agent: String,
    definition_digest: String,
    profile_digest: String,
    state: agents::AgentState,
    current_activation: Option<SubagentId>,
    activation_state: SubagentState,
    detail: Option<Arc<str>>,
    observation: SubagentObservation,
    started_at: DateTime<Utc>,
    workspace: std::path::PathBuf,
}

impl AgentObservationSource {
    pub(crate) fn direct_text(&self, path: &[String]) -> Option<&str> {
        match path {
            [field] => match field.as_str() {
                "title" => Some(&self.authority.title),
                "detail" => self.detail.as_deref(),
                "agent" => Some(&self.agent),
                "definition_digest" => Some(&self.definition_digest),
                "profile_digest" => Some(&self.profile_digest),
                "agent_id" => Some(self.title_agent_id.as_str()),
                "parent_agent_id" => Some(self.parent_agent_id.as_str()),
                "activation_id" => Some(self.activation_id.as_str()),
                "child_conversation_id" => Some(self.child_conversation_id.as_str()),
                "workspace" => self.workspace.to_str(),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn indexed(
        &self,
    ) -> Result<&crate::runtime::read_layout::SeekableObservationSource, &'static str> {
        self.layout
            .get_or_init(|| {
                self.with_value(|value| {
                    crate::runtime::read_layout::SeekableObservationSource::new(value)
                })
            })
            .as_ref()
            .map_err(|error| *error)
    }

    pub(crate) fn with_value<R>(&self, read: impl FnOnce(&AgentObservationValue<'_>) -> R) -> R {
        read(&AgentObservationValue {
            title: &self.authority.title,
            parent_agent_id: &self.parent_agent_id,
            activation_id: &self.activation_id,
            agent_id: &self.title_agent_id,
            child_conversation_id: &self.child_conversation_id,
            agent: &self.agent,
            definition_digest: &self.definition_digest,
            profile_digest: &self.profile_digest,
            state: self.state,
            current_activation: self.current_activation.as_ref(),
            activation_state: self.activation_state,
            detail: self.detail.as_deref(),
            observation: &self.observation,
            execution_profile: self.profile.as_deref(),
            started_at: self.started_at,
            workspace: &self.workspace,
        })
    }
}

impl SubagentRegistry {
    fn observation_ordinal_locked(state: &RegistryState, id: &AgentId) -> Option<u64> {
        state
            .agent_observation_index
            .get(id)
            .map(|index| (state.agent_observation_order.len() - 1 - index) as u64)
    }

    pub(super) fn observation_cut_locked(
        state: &RegistryState,
    ) -> crate::runtime::identity::ObservationCollectionCut {
        crate::runtime::identity::ObservationCollectionCut {
            generation: state.observation_generation,
            total: state.agent_observation_order.len() as u64,
        }
    }

    /// Exercise the real native callback and capture its exact source generation
    /// while its actual owner lock is held.
    #[cfg(test)]
    pub(crate) fn republish_lifecycle_for_test(
        &self,
        activation: &SubagentId,
    ) -> crate::runtime::identity::ObservationCollectionCut {
        let mut state = self.state.lock().expect("test registry owner");
        let index = state.index[activation];
        publish_snapshot(&mut state, &self.state_version, index);
        Self::observation_cut_locked(&state)
    }

    #[cfg(test)]
    pub(crate) fn observation_cut(&self) -> crate::runtime::identity::ObservationCollectionCut {
        Self::observation_cut_locked(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    #[cfg(test)]
    pub(crate) fn observation_ordinal(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        id: &AgentId,
    ) -> Result<u64, crate::runtime::identity::ObservationCollectionReadError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        cut.validate_current(Self::observation_cut_locked(&state))?;
        let index = *state.agent_observation_index.get(id).ok_or(
            crate::runtime::identity::ObservationCollectionReadError::Unavailable(
                "native Agent source unavailable",
            ),
        )?;
        Ok((state.agent_observation_order.len() - 1 - index) as u64)
    }

    fn observation_source_locked(state: &RegistryState, id: &AgentId) -> AgentObservationSource {
        let agent = &state.agents[id];
        let activation = &state.records[state.index[&agent.latest_activation]];
        let current_activation = agent
            .resuming
            .as_ref()
            .filter(|value| value.identity_persisted)
            .map(|value| value.activation_id.clone())
            .or_else(|| {
                (!activation.lifecycle.is_terminal() || !activation.physical_settlement_proven)
                    .then(|| agent.latest_activation.clone())
            });
        AgentObservationSource {
            layout: state.observation_layouts[id].clone(),
            authority: agent.authority.clone(),
            profile: activation.profile.clone(),
            title_agent_id: id.clone(),
            parent_agent_id: activation.parent_agent_id.clone(),
            activation_id: activation.subagent_id.clone(),
            child_conversation_id: agent.conversation_id.clone(),
            agent: activation.agent.as_str().into(),
            definition_digest: activation.definition_digest.as_str().into(),
            profile_digest: activation.profile_digest.as_str().into(),
            state: Self::agent_state_locked(state, id).expect("native Agent owns an activation"),
            current_activation,
            activation_state: activation.observation_state(),
            detail: activation.detail.clone(),
            observation: activation.observation.clone(),
            started_at: activation.started_at,
            workspace: activation.workspace.clone(),
        }
    }

    pub(crate) fn with_observation_value_by_id<R>(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        id: &AgentId,
        read: impl FnOnce(&AgentObservationValue<'_>) -> R,
    ) -> Result<R, crate::runtime::identity::ObservationCollectionReadError> {
        let source = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            cut.validate_current(Self::observation_cut_locked(&state))?;
            if !state.agents.contains_key(id) {
                return Err(
                    crate::runtime::identity::ObservationCollectionReadError::Unavailable(
                        "native Agent source unavailable",
                    ),
                );
            }
            Self::observation_source_locked(&state, id)
        };
        Ok(source.with_value(read))
    }

    #[cfg(test)]
    pub(crate) fn with_observation_value<R>(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: u64,
        read: impl FnOnce(&AgentObservationValue<'_>) -> R,
    ) -> Result<R, crate::runtime::identity::ObservationCollectionReadError> {
        let source = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            cut.validate_current(Self::observation_cut_locked(&state))?;
            let ordinal = usize::try_from(ordinal).map_err(|_| {
                crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                    "invalid native collection ordinal",
                )
            })?;
            let index = state
                .agent_observation_order
                .len()
                .checked_sub(ordinal.saturating_add(1))
                .ok_or(
                    crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                        "native collection ordinal exceeds frontier",
                    ),
                )?;
            Self::observation_source_locked(&state, &state.agent_observation_order[index])
        };
        Ok(source.with_value(read))
    }

    pub(crate) fn with_observation_source_by_id<R>(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        id: &AgentId,
        read: impl FnOnce(&AgentObservationSource) -> R,
    ) -> Result<R, crate::runtime::identity::ObservationCollectionReadError> {
        let source = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            cut.validate_current(Self::observation_cut_locked(&state))?;
            if !state.agents.contains_key(id) {
                return Err(
                    crate::runtime::identity::ObservationCollectionReadError::Unavailable(
                        "native Agent source unavailable",
                    ),
                );
            }
            Self::observation_source_locked(&state, id)
        };
        Ok(read(&source))
    }

    pub(crate) fn with_observation_source<R>(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: u64,
        read: impl FnOnce(&AgentObservationSource) -> R,
    ) -> Result<R, crate::runtime::identity::ObservationCollectionReadError> {
        let source = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            cut.validate_current(Self::observation_cut_locked(&state))?;
            let ordinal = usize::try_from(ordinal).map_err(|_| {
                crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                    "invalid native collection ordinal",
                )
            })?;
            let index = state
                .agent_observation_order
                .len()
                .checked_sub(ordinal.saturating_add(1))
                .ok_or(
                    crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                        "native collection ordinal exceeds frontier",
                    ),
                )?;
            Self::observation_source_locked(&state, &state.agent_observation_order[index])
        };
        Ok(read(&source))
    }

    pub(crate) fn observation_sources_page(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        offset: u64,
        limit: usize,
    ) -> Result<
        crate::runtime::identity::ObservationCollectionPage<AgentObservationSource>,
        crate::runtime::identity::ObservationCollectionReadError,
    > {
        if limit == 0 || limit > 32 {
            return Err(
                crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                    "invalid native collection page bound",
                ),
            );
        }
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        cut.validate_current(Self::observation_cut_locked(&state))?;
        let start = usize::try_from(offset).map_err(|_| {
            crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                "invalid native collection offset",
            )
        })?;
        if start > state.agent_observation_order.len() {
            return Err(
                crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                    "native collection offset exceeds frontier",
                ),
            );
        }
        let end = start
            .saturating_add(limit)
            .min(state.agent_observation_order.len());
        Ok(crate::runtime::identity::ObservationCollectionPage {
            cut,
            offset,
            items: (start..end)
                .map(|ordinal| {
                    Self::observation_source_locked(
                        &state,
                        &state.agent_observation_order
                            [state.agent_observation_order.len() - 1 - ordinal],
                    )
                })
                .collect(),
            next_offset: (end < state.agent_observation_order.len()).then_some(end as u64),
        })
    }

    /// The consistency snapshot of one subagent, if the registry knows it.
    #[must_use]
    pub fn snapshot(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .index
            .get(subagent_id)
            .map(|&index| state.records[index].snapshot())
    }

    /// Resolve only an exact child owned by this registry. Opening an existing
    /// store is read-only and cannot create a Conversation or extend execution.
    pub(crate) fn transcript_store(
        &self,
        subagent_id: &SubagentId,
    ) -> Result<crate::durable::SqliteConversationStore, super::super::SubagentTranscriptError>
    {
        use super::super::SubagentTranscriptError;
        let child = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let index = state
                .index
                .get(subagent_id)
                .ok_or_else(|| SubagentTranscriptError::Unknown(subagent_id.clone()))?;
            state.records[*index].child_conversation_id.clone()
        };
        let root = &self.config.spawn.product_root;
        let path = super::super::child_conversation_store_path(
            root.root(),
            &self.config.spawn.session_id,
            &child,
        );
        let unavailable = SubagentTranscriptError::Unavailable;
        let access = crate::runtime::local_storage::ConversationAccess::existing(
            root,
            path.parent().expect("child database has a parent"),
        )
        .map_err(|error| unavailable(error.to_string()))?;
        let path = access
            .confined(&path)
            .map_err(|error| unavailable(error.to_string()))?;
        crate::durable::SqliteConversationStore::open_existing(child, &path)
            .map(|store| store.with_lifecycle(std::sync::Arc::new(access)))
            .map_err(|error| unavailable(error.to_string()))
    }

    /// **Observation plane (Issue #178).** Applies one live activity
    /// projection reported by the child driver.
    ///
    /// This is a synchronous, lock-brief read-model update: it never touches
    /// lifecycle, the journal, or the mailbox, and it never blocks on any
    /// observation consumer. Two drop rules keep the projection honest:
    ///
    /// - post-terminal (or terminal-publishing) updates are dropped, so
    ///   late activity can never resurrect live-ness after settlement;
    /// - stale or reordered revisions are dropped, preserving latest-value
    ///   semantics under coalesced delivery.
    pub fn apply_activity(&self, subagent_id: &SubagentId, observation: SubagentObservation) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal()
            || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal)
        {
            return;
        }
        if observation.revision <= record.observation.revision {
            return;
        }
        record.observation = observation;
        publish_activity_snapshot(&mut state, &self.state_version, index);
    }

    /// The durably committed Workflow output value of one settled
    /// Workflow-owned child (Issue #83).
    ///
    /// This is the live Workflow result channel: the value was validated
    /// and committed atomically with the child's terminal lifecycle fact,
    /// and it deliberately never rides the observation snapshot (Issue
    /// #178). `None` for any non-Workflow child, any non-successful
    /// settlement, and any record that has not settled.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn workflow_agent_output(
        &self,
        subagent_id: &SubagentId,
    ) -> Option<serde_json::Value> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let &index = state.index.get(subagent_id)?;
        let record = &state.records[index];
        if record.lifecycle != SubagentLifecycle::Succeeded {
            return None;
        }
        record.terminal_workflow_value.clone()
    }

    /// Transfers the settled Workflow value to its live owner exactly once.
    /// The journal retains bounded evidence; the registry no longer retains
    /// private block data after this handoff.
    pub(crate) fn take_workflow_agent_output(
        &self,
        subagent_id: &SubagentId,
        instance: &crate::runtime::workflow::WorkflowNodeInstance,
    ) -> Option<serde_json::Value> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let &index = state.index.get(subagent_id)?;
        let record = &mut state.records[index];
        if record.lifecycle != SubagentLifecycle::Succeeded {
            return None;
        }
        if !matches!(&record.terminal, Some(SubagentTerminalMode::WorkflowOutput { node_id, .. }) if node_id.as_ref() == instance)
        {
            return None;
        }
        record.terminal_workflow_value.take()
    }

    /// The consistency snapshots of every known subagent, in ordinal order.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.records.iter().map(SubagentRecord::snapshot).collect()
    }

    /// The registry's bounded authoritative listing for conversation-owned
    /// execution discovery (Issue #180).
    ///
    /// This is the discovery read model of the subagent domain: the registry
    /// alone decides which children exist, in which authoritative order,
    /// which of them are lifecycle-active, and how many matched. The listing
    /// carries at most `limit` snapshots in **reverse ordinal order** — the
    /// most recently started child first — so applying the bound keeps the
    /// children a caller is most likely to still be acting on.
    ///
    /// `matched` reports how many records matched before the bound, so a
    /// caller can report truncation without the registry ever materializing
    /// an unbounded response. `limit` is the caller's materialization
    /// bound and nothing more: the registry has no opinion on how large a
    /// model-facing response may be, and never sees one.
    ///
    /// `active_only` selects the non-terminal (`Running`/`Cancelling`/
    /// `PublishingTerminal`) records exactly as [`SubagentState::is_active`]
    /// defines them; otherwise every record the registry still knows is
    /// listed, terminal ones included.
    ///
    /// This is a pure read: it takes the same lock every query takes and
    /// mutates nothing — no lifecycle, no observation revision, no
    /// notification state, and no observer seam. Listing a child is
    /// indistinguishable, from the child's side, from never having been
    /// listed at all.
    #[must_use]
    pub fn listing(&self, active_only: bool, limit: usize) -> SubagentListing {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let matching = state
            .records
            .iter()
            .rev()
            .filter(|record| !active_only || record.lifecycle.is_active());
        let matched = matching.clone().count();
        let snapshots = matching.take(limit).map(SubagentRecord::snapshot).collect();
        SubagentListing { snapshots, matched }
    }

    /// The unsettled subagents in deterministic ordinal order (drain).
    #[must_use]
    pub fn unsettled_snapshot(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .filter(|record| {
                (record.lifecycle.is_active()
                    || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal))
                    && !record.publication_abandoned
            })
            .map(SubagentRecord::snapshot)
            .collect()
    }
}
