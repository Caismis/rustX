//! Root projection of child-owned routed interactions: publication admission,
//! the pending-interaction head, and relayed control. The originating child
//! coordinator stays the semantic owner of every entry; the registry holds
//! only the presentation cache under its one mutex.

use super::DelegateDelivery;
use super::RegistryState;
use super::SubagentLifecycle;
use super::SubagentRegistry;
use super::observation::publish_snapshot;
use crate::runtime::identity::SubagentId;
use crate::runtime::interaction::InteractionOutcome;
use crate::runtime::interaction::InteractionRef;
use crate::runtime::interaction::RoutedInteractionError;
use std::sync::Arc;
use std::sync::PoisonError;

/// The observation seam of the subagent plane (TUI / Runtime Client).
///
/// Two publication classes (Issue #178): [`on_snapshot`](Self::on_snapshot)
/// is the **reliable** lifecycle/identity publication — every transition
/// reaches the consumer exactly once, in order — and
/// [`on_activity`](Self::on_activity) is the **disposable** latest-value
/// activity publication, which the consumer may coalesce or drop.
#[derive(Clone)]
pub struct ChildPendingHeadProof {
    pub cut: crate::runtime::identity::ObservationCollectionCut,
    pub head: Vec<crate::observation::interaction::RuntimeClientInteractionHeader>,
    pub next_sequence: Option<u64>,
}

/// The root Runtime Client's publication-admission authority.
///
/// The authority is intentionally narrower than the interaction domain: it
/// answers only whether a capable root runtime projection is bound
/// at the root host's synchronized admission frontier. It owns no child
/// interaction state, waiter, audit, cancellation, settlement, or execution
/// authority.
pub(crate) trait InteractionPublicationAuthority: Send + Sync {
    /// Attempts to admit publication of one exact routed interaction.
    fn admit(&self, interaction: &InteractionRef) -> bool;
}

/// The narrow reliable semantic sink the child driver task holds for one
/// child. It can update the root's presentation projection only; it cannot
/// answer, cancel, or settle the child's interaction.
#[derive(Clone)]
pub(crate) struct SubagentInteractionSink {
    pub(super) subagent_id: SubagentId,
    pub(super) registry: SubagentRegistry,
}

impl SubagentInteractionSink {
    /// The start gate already resolved start-vs-cancel under the registry
    /// mutex. Record the actual write attempt without introducing a second
    /// cancellation arbiter: a released gate sends Delegate before queued Cancel.
    pub(crate) fn begin_delegate(&self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let index = state.index[&self.subagent_id];
        state.records[index].delegate_delivery = DelegateDelivery::Started;
    }

    pub(crate) fn accept_delegate(&self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(&index) = state.index.get(&self.subagent_id) {
            if state.records[index].delegate_delivery != DelegateDelivery::Started {
                return;
            }
            state.records[index].delegate_delivery = DelegateDelivery::Accepted;
            self.registry
                .state_version
                .send_modify(|version| *version += 1);
        }
    }
    pub(crate) fn reopen_admission(&self) -> bool {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(&index) = state.index.get(&self.subagent_id)
            && state.records[index].lifecycle == SubagentLifecycle::Stopping
            && state.records[index].cancel_reason.is_none()
        {
            state.records[index].lifecycle = SubagentLifecycle::Running;
            publish_snapshot(&mut state, &self.registry.state_version, index);
            return true;
        }
        false
    }

    pub(crate) fn begin_seal(&self) {
        self.registry.begin_seal(&self.subagent_id);
    }

    /// Applies one child-owned interaction request received over reliable
    /// control transport.
    pub(crate) async fn apply_requested(
        &self,
        request: crate::observation::interaction::RuntimeClientInteractionHeader,
    ) -> Result<(), crate::runtime::interaction::InteractionRouteError> {
        if let Some(header) = self
            .registry
            .apply_child_interaction_requested(&self.subagent_id, request)
        {
            self.registry
                .relay_child_interaction(
                    crate::runtime::interaction::InteractionRouteEvent::Requested(Box::new(header)),
                )
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn apply_settled(
        &self,
        interaction: &InteractionRef,
        outcome: &InteractionOutcome,
    ) -> Result<(), crate::runtime::interaction::InteractionRouteError> {
        if self
            .registry
            .apply_child_interaction_settled(&self.subagent_id, interaction, outcome)
        {
            self.registry
                .relay_child_interaction(
                    crate::runtime::interaction::InteractionRouteEvent::Settled {
                        interaction: interaction.clone(),
                        outcome: outcome.clone(),
                    },
                )
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn apply_removed(
        &self,
        interaction: &InteractionRef,
    ) -> Result<(), crate::runtime::interaction::InteractionRouteError> {
        if self
            .registry
            .remove_child_interaction(&self.subagent_id, interaction)
        {
            self.registry
                .relay_child_interaction(
                    crate::runtime::interaction::InteractionRouteEvent::Removed {
                        interaction: interaction.clone(),
                    },
                )
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn retire_pending_routes(&self) {
        loop {
            let next = {
                let state = self
                    .registry
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let Some(index) = state.index.get(&self.subagent_id) else {
                    return;
                };
                let cid = &state.records[*index].child_conversation_id;
                state
                    .routed_by_child
                    .get(cid)
                    .and_then(|pending| pending.values().next())
                    .cloned()
            };
            let Some(interaction) = next else {
                return;
            };
            if self.apply_removed(&interaction).await.is_err() {
                return;
            }
        }
    }
    pub(crate) async fn admit_publication(&self, interaction: &InteractionRef) -> bool {
        self.registry
            .admit_child_interaction_publication(&self.subagent_id, interaction)
            .await
    }
}

impl SubagentRegistry {
    /// Updates the root human-provider state for all live and future child
    /// conversations. This changes only whether a new interaction may be
    /// published; it never settles an interaction already pending.
    pub(crate) fn set_interaction_provider_available(&self, available: bool) {
        self.provider_available.send_replace(available);
    }

    /// Installs the root Runtime Client's synchronized publication frontier.
    /// This is a composition seam, not an interaction owner, and is installed
    /// before the runtime activates so every child route sees one authority.
    pub(crate) fn install_interaction_publication_authority(
        &self,
        authority: Arc<dyn InteractionPublicationAuthority>,
    ) {
        let mut installed = self
            .publication_authority
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        debug_assert!(installed.is_none(), "one publication authority only");
        *installed = Some(authority);
    }

    /// Subscribes one child driver to the current root provider state.
    pub(crate) fn interaction_provider_receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.provider_available.subscribe()
    }

    /// Admits one child interaction publication at the root Runtime Client's
    /// synchronized provider frontier. The registry lock covers the child
    /// liveness/identity check while the root authority lock defines the
    /// attach-vs-detach ordering. No child semantic state is created here.
    pub(crate) fn install_upstream_interaction_route(
        &self,
        route: Arc<dyn crate::runtime::interaction::InteractionRoute>,
    ) {
        *self
            .upstream_interactions
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(route);
    }

    async fn relay_child_interaction(
        &self,
        event: crate::runtime::interaction::InteractionRouteEvent,
    ) -> Result<(), crate::runtime::interaction::InteractionRouteError> {
        let route = self
            .upstream_interactions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(route) = route {
            route.publish(event).await?;
        }
        Ok(())
    }

    pub(crate) async fn admit_child_interaction_publication(
        &self,
        subagent_id: &SubagentId,
        interaction: &InteractionRef,
    ) -> bool {
        let admitted = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.index.get(subagent_id).is_some_and(|index| {
                let record = &state.records[*index];
                record.lifecycle.is_active()
                    && !record.publication_abandoned
                    && interaction.conversation_id != self.config.conversation_id
            })
        };
        if !admitted {
            return false;
        }
        let upstream = self
            .upstream_interactions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(route) = upstream {
            if route.admit_publication(interaction.clone()).await.is_err() {
                return false;
            }
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            return state.index.get(subagent_id).is_some_and(|index| {
                state.records[*index].lifecycle.is_active()
                    && !state.records[*index].publication_abandoned
            });
        }
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.index.get(subagent_id).is_some_and(|index| {
            state.records[*index].lifecycle.is_active()
                && !state.records[*index].publication_abandoned
        }) {
            return false;
        }
        self.publication_authority
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|authority| authority.admit(interaction))
    }

    /// Returns the live child interaction projection for the Runtime Client
    /// bootstrap cut. These values are never used as recovery authority.
    #[must_use]
    pub(crate) fn pending_interaction_head(&self) -> ChildPendingHeadProof {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::pending_interaction_head_locked(&state)
    }

    fn pending_interaction_cut_locked(
        state: &RegistryState,
    ) -> crate::runtime::identity::ObservationCollectionCut {
        crate::runtime::identity::ObservationCollectionCut {
            generation: state.routed_generation,
            total: state.routed_interactions.len() as u64,
        }
    }

    pub(super) fn pending_interaction_head_locked(state: &RegistryState) -> ChildPendingHeadProof {
        Self::pending_interaction_page_locked(state, None, 32)
    }

    fn pending_interaction_page_locked(
        state: &RegistryState,
        after: Option<u64>,
        limit: usize,
    ) -> ChildPendingHeadProof {
        let start = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        ChildPendingHeadProof {
            cut: Self::pending_interaction_cut_locked(state),
            head: state
                .routed_order
                .range((start, std::ops::Bound::Unbounded))
                .take(if state.routed_generation == u64::MAX {
                    0
                } else {
                    limit
                })
                .map(|(_, reference)| state.routed_interactions[reference].clone())
                .collect(),
            next_sequence: state
                .routed_order
                .range((start, std::ops::Bound::Unbounded))
                .nth(limit)
                .map(|(sequence, _)| *sequence),
        }
    }

    pub(crate) fn pending_interaction_page(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        after: Option<u64>,
        limit: usize,
    ) -> Result<ChildPendingHeadProof, crate::runtime::identity::ObservationCollectionReadError>
    {
        if limit == 0 || limit > 32 {
            return Err(
                crate::runtime::identity::ObservationCollectionReadError::InvalidRequest(
                    "invalid Child pending page bound",
                ),
            );
        }
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        cut.validate_current(Self::pending_interaction_cut_locked(&state))?;
        Ok(Self::pending_interaction_page_locked(&state, after, limit))
    }

    pub(crate) fn pending_interaction_source_at(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        sequence: u64,
    ) -> Result<
        crate::observation::interaction::RuntimeClientInteractionHeader,
        crate::runtime::identity::ObservationCollectionReadError,
    > {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        cut.validate_current(Self::pending_interaction_cut_locked(&state))?;
        let reference = state.routed_order.get(&sequence).ok_or(
            crate::runtime::identity::ObservationCollectionReadError::Unavailable(
                "Child pending source unavailable",
            ),
        )?;
        Ok(state.routed_interactions[reference].clone())
    }

    /// Applies one child-owned interaction request to the root-facing
    /// projection cache. The request is accepted only from the active child
    /// incarnation whose conversation identity it names; the cache has no
    /// authority to settle the child.
    pub(crate) fn apply_child_interaction_requested(
        &self,
        subagent_id: &SubagentId,
        mut request: crate::observation::interaction::RuntimeClientInteractionHeader,
    ) -> Option<crate::observation::interaction::RuntimeClientInteractionHeader> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let &index = state.index.get(subagent_id)?;
        let record = &state.records[index];
        if !record.lifecycle.is_active()
            || record.publication_abandoned
            || request.conversation_id == self.config.conversation_id
        {
            return None;
        }
        if request.interaction
            != InteractionRef::new(request.conversation_id.clone(), request.request_id.clone())
            || request.body.is_some()
            || request.body_complete
        {
            return None;
        }
        let direct_cid = record.child_conversation_id.clone();
        match &request.source {
            crate::runtime::interaction::InteractionSource::Primary
                if request.conversation_id == direct_cid =>
            {
                request.source = crate::runtime::interaction::InteractionSource::Subagent {
                    subagent_id: record.subagent_id.clone(),
                    child_conversation_id: direct_cid.clone(),
                    agent_name: record.agent.clone(),
                };
            }
            crate::runtime::interaction::InteractionSource::Subagent {
                child_conversation_id,
                ..
            } if *child_conversation_id == request.conversation_id => {}
            _ => return None,
        }
        state
            .routed_direct
            .insert(request.interaction.clone(), subagent_id.clone());
        if state.routed_interactions.contains_key(&request.interaction) {
            return None;
        }
        if state.next_routed_sequence < 4_503_599_627_370_495 {
            let sequence = state.next_routed_sequence;
            state.next_routed_sequence = sequence + 1;
            request.source_offset = sequence * 2 + 1;
            state
                .routed_order
                .insert(sequence, request.interaction.clone());
            state
                .routed_by_child
                .entry(direct_cid)
                .or_default()
                .insert(sequence, request.interaction.clone());
        } else {
            state.routed_generation = u64::MAX;
        }
        state.routed_generation = state.routed_generation.saturating_add(1);
        let routed = request;
        let previous = state
            .routed_interactions
            .insert(routed.interaction.clone(), routed.clone());
        debug_assert!(
            previous.is_none(),
            "a live child interaction identity was reused"
        );
        if let Some(observer) = state.observer.clone() {
            observer
                .on_interaction_pending(&routed, &Self::pending_interaction_head_locked(&state));
        }
        #[cfg(test)]
        self.interaction_published.notify_one();
        Some(routed)
    }

    /// Applies one child-owned terminal interaction transition to the
    /// root-facing projection. The originating child coordinator has already
    /// selected and durably audited the outcome; this method only removes the
    /// presentation entry and forwards its route event.
    pub(crate) fn apply_child_interaction_settled(
        &self,
        subagent_id: &SubagentId,
        interaction: &InteractionRef,
        outcome: &InteractionOutcome,
    ) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return false;
        };
        let record = &state.records[index];
        if state.routed_direct.get(interaction) != Some(subagent_id) {
            return false;
        }
        let direct_cid = record.child_conversation_id.clone();
        let Some(header) = state.routed_interactions.remove(interaction) else {
            // Duplicate/late child route events are harmless projection
            // duplicates. The coordinator's own pending map already made the
            // semantic transition exactly once.
            return false;
        };
        let sequence = header.source_offset / 2;
        state.routed_order.remove(&sequence);
        state.routed_direct.remove(interaction);
        if let Some(child) = state.routed_by_child.get_mut(&direct_cid) {
            child.remove(&sequence);
        }
        state.routed_generation = state.routed_generation.saturating_add(1);
        if let Some(observer) = state.observer.clone() {
            observer.on_interaction_settled(
                interaction,
                outcome,
                &Self::pending_interaction_head_locked(&state),
            );
        }
        true
    }

    fn remove_child_interaction(
        &self,
        subagent_id: &SubagentId,
        interaction: &InteractionRef,
    ) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.routed_direct.get(interaction) != Some(subagent_id) {
            return false;
        }
        let Some(index) = state.index.get(subagent_id).copied() else {
            return false;
        };
        let direct_cid = state.records[index].child_conversation_id.clone();
        let Some(header) = state.routed_interactions.remove(interaction) else {
            return false;
        };
        let sequence = header.source_offset / 2;
        state.routed_direct.remove(interaction);
        state.routed_order.remove(&sequence);
        if let Some(child) = state.routed_by_child.get_mut(&direct_cid) {
            child.remove(&sequence);
        }
        state.routed_generation = state.routed_generation.saturating_add(1);
        if let Some(observer) = state.observer.clone() {
            observer.on_interaction_head(&Self::pending_interaction_head_locked(&state));
        }
        true
    }

    /// Forwards a root response to the live child identified by the routed
    /// conversation identity. The response id is transport correlation only;
    /// the child coordinator validates the semantic target and response.
    pub(crate) async fn control_interaction(
        &self,
        interaction: &InteractionRef,
        response: crate::runtime::interaction::InteractionControl,
    ) -> Result<(), RoutedInteractionError> {
        let (control, response_id, result, receiver) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.routed_interactions.contains_key(interaction) {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            }
            let control = state
                .routed_direct
                .get(interaction)
                .and_then(|activation| state.index.get(activation))
                .map(|index| &state.records[*index])
                .filter(|record| record.lifecycle.is_active() && !record.publication_abandoned)
                .and_then(|record| record.control.clone());
            let Some(control) = control else {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            };
            let response_id = state.next_response_id;
            if response_id == 0 {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            }
            state.next_response_id = response_id.checked_add(1).unwrap_or(0);
            let (result, receiver) = tokio::sync::oneshot::channel();
            (control, response_id, result, receiver)
        };
        // Keep the response's semantic identity intact across the process
        // boundary; only the response_id is newly allocated transport data.
        if control
            .send(super::super::process::DriverCommand::Route(
                super::super::process::ChildBoundRoute::InteractionRespond {
                    response_id,
                    interaction: interaction.clone(),
                    response,
                    result,
                },
            ))
            .is_err()
        {
            return Err(RoutedInteractionError::NotPending {
                interaction: interaction.clone(),
            });
        }
        receiver.await.unwrap_or_else(|_| {
            Err(RoutedInteractionError::NotPending {
                interaction: interaction.clone(),
            })
        })
    }

    /// Removes every actionable interaction owned by one child incarnation.
    ///
    /// The registry deliberately has no terminal outcome to assign here: the
    /// child coordinator is process-owned and dies with the child. The only
    /// root-side fact is that the presentation entry is no longer answerable.
    pub(super) fn remove_child_interactions(&self, subagent_id: &SubagentId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let child_conversation_id = state.records[index].child_conversation_id.clone();
        let Some(removed) = state.routed_by_child.remove(&child_conversation_id) else {
            return;
        };
        for (sequence, interaction) in removed {
            state.routed_order.remove(&sequence);
            state.routed_interactions.remove(&interaction);
            state.routed_direct.remove(&interaction);
        }
        state.routed_generation = state.routed_generation.saturating_add(1);
        if let Some(observer) = state.observer.clone() {
            observer.on_interaction_head(&Self::pending_interaction_head_locked(&state));
        }
    }
}
