//! Interaction, Goal, subagent and background control requests routed to
//! their native owners.

use super::*;

impl ConversationRuntime {
    /// Answers one live native interaction through its originating
    /// conversation. The root runtime is only a routing surface: it never
    /// creates a parent interaction for a child and never settles a child
    /// coordinator itself.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: crate::runtime::interaction::InteractionResponse,
    ) -> Result<(), RoutedInteractionError> {
        self.control_interaction(
            interaction,
            crate::runtime::interaction::InteractionControl::Respond { response },
        )
        .await
    }

    pub(crate) fn pending_interaction_page(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        after: Option<u64>,
        limit: usize,
    ) -> Result<
        crate::runtime::interaction::PendingHeadProof,
        crate::runtime::identity::ObservationCollectionReadError,
    > {
        self.inner.interaction.observation_page(cut, after, limit)
    }
    pub(crate) fn pending_interaction_source_at(
        &self,
        cut: crate::runtime::identity::ObservationCollectionCut,
        sequence: u64,
    ) -> Result<
        Arc<crate::runtime::interaction::PendingInteractionSource>,
        crate::runtime::identity::ObservationCollectionReadError,
    > {
        self.inner.interaction.observation_source_at(cut, sequence)
    }
    pub(crate) fn pending_interaction_source(
        &self,
        interaction: &crate::runtime::interaction::InteractionRef,
    ) -> Option<Arc<crate::runtime::interaction::source::PendingInteractionSource>> {
        self.inner.interaction.observation_source(interaction)
    }

    #[cfg(test)]
    pub(crate) fn interaction_test_owner(
        &self,
    ) -> Arc<crate::runtime::interaction::InteractionCoordinator> {
        self.inner.interaction.clone()
    }

    pub(crate) async fn control_interaction(
        &self,
        interaction: &InteractionRef,
        control: crate::runtime::interaction::InteractionControl,
    ) -> Result<(), RoutedInteractionError> {
        if interaction.conversation_id == self.inner.conversation_id {
            let result = match control {
                crate::runtime::interaction::InteractionControl::Respond { response } => {
                    self.inner
                        .interaction
                        .respond_async(&interaction.interaction_id, response)
                        .await
                }
                crate::runtime::interaction::InteractionControl::Cancel => {
                    self.inner
                        .interaction
                        .cancel_async(
                            &interaction.interaction_id,
                            CancellationReason::UserRequested,
                        )
                        .await
                }
            };
            return result.map_err(|error| route_error(interaction, error));
        }
        if let Some(subagents) = &self.inner.subagents {
            return subagents.control_interaction(interaction, control).await;
        }
        Err(RoutedInteractionError::NotPending {
            interaction: interaction.clone(),
        })
    }

    /// Installs the reliable parent route on a child conversation's own
    /// coordinator. The route receives only already-authoritative semantic
    /// request/settlement facts and no interaction ownership state.
    pub(crate) fn install_interaction_route(&self, route: Arc<dyn InteractionRoute>) {
        self.inner.interaction.install_route(route.clone());
        if let Some(registry) = &self.inner.subagents {
            registry.install_upstream_interaction_route(route);
        }
    }

    /// Installs the root Runtime Client's synchronized publication-admission
    /// frontier for child interaction routes. The authority answers only
    /// whether a capable root runtime projection exists; the child
    /// coordinator remains the semantic owner of every interaction.
    pub(crate) fn install_interaction_publication_authority(
        &self,
        authority: Arc<dyn crate::runtime::subagent::InteractionPublicationAuthority>,
    ) {
        if let Some(subagents) = &self.inner.subagents {
            subagents.install_interaction_publication_authority(authority);
        }
    }

    /// Publishes an early provider-availability hint for future interactions.
    /// Runtime Client host binding is the production caller; the
    /// root host's publication authority is the actual admission frontier,
    /// and existing pending work is intentionally unaffected.
    pub(crate) fn set_interaction_provider_available(&self, available: bool) {
        self.inner.interaction.set_provider_available(available);
        if let Some(subagents) = &self.inner.subagents {
            subagents.set_interaction_provider_available(available);
        }
    }

    /// Authoritative Conversation Goal state, independent of model Plugin selection.
    ///
    /// # Errors
    /// Returns a durable Goal read failure.
    pub fn goal_view(
        &self,
    ) -> Result<Option<crate::goal::GoalView>, crate::durable::ConversationStoreError> {
        self.inner
            .tool_runtime
            .goal()
            .map(crate::goal::GoalDomain::view)
            .transpose()
    }

    /// User control uses the existing coordinator and lifecycle commit boundary.
    ///
    /// # Errors
    /// Returns disabled, inactive, storage, transition, or stale-revision diagnostics.
    ///
    /// # Panics
    /// Panics only if a runtime synchronization lock was poisoned.
    pub fn control_goal(
        &self,
        control: crate::goal::GoalControl,
    ) -> Result<crate::goal::GoalView, String> {
        let state = self.inner.lock_state();
        let domain = self
            .inner
            .tool_runtime
            .goal()
            .ok_or("Goal state is unavailable")?;
        let write = match control {
            crate::goal::GoalControl::Show => {
                return domain.view().map_err(|error| error.to_string());
            }
            crate::goal::GoalControl::Create { objective, budget } => {
                crate::goal::GoalWrite::Create {
                    objective,
                    budget,
                    origin: crate::goal::GoalOrigin::RuntimeControl,
                }
            }
            crate::goal::GoalControl::Mutate { expected, mutation } => {
                crate::goal::GoalWrite::Mutate { expected, mutation }
            }
        };
        let committed = self
            .inner
            .mailbox
            .with_running_commit(|| domain.write(write))
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|rejection| {
                let refusal=serde_json::json!({"reason":rejection.reason,"current":crate::observation::goal::RuntimeClientGoalView::from_native(rejection.current.as_ref()).current});
                crate::observation::bounded::bounded_bytes(&refusal,16_384).and_then(|bytes|String::from_utf8(bytes).ok()).unwrap_or_else(||"Goal control refused; read the current Goal state".into())
            })?;
        let phase = committed.phase;
        let view = crate::goal::GoalView {
            current: Some(committed),
        };
        drop(state);
        // Issue #351: a successful Create or Resume commits durable
        // authorization to continue, so the ordinary admission owner is woken
        // here — by runtime control, not by Goal code holding a wake handle.
        // If an attempt is already running the worker finds the runtime busy
        // and changes nothing; the settlement handoff then reaches the next
        // eligible idle boundary. This wake is a liveness notification only:
        // every eligibility question is re-answered under the coordinator
        // lock against durable state.
        if phase == crate::goal::GoalPhase::Active {
            self.inner.wake.notify.notify_one();
        }
        Ok(view)
    }

    /// Inspects one subagent child through the authoritative registry
    /// (Issue #60).
    #[must_use]
    pub fn subagent_status(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Option<crate::runtime::subagent::SubagentSnapshot> {
        self.inner
            .subagents
            .as_ref()
            .and_then(|subagents| subagents.snapshot(subagent_id))
    }

    /// Requests cancellation of one subagent child through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct; the snapshot is the state after the intent commit.
    #[must_use]
    pub fn subagent_cancel(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Option<crate::runtime::subagent::SubagentSnapshot> {
        self.inner
            .subagents
            .as_ref()
            .and_then(|subagents| subagents.cancel(subagent_id, CancellationReason::UserRequested))
    }

    /// Inspects one background execution through the authoritative
    /// registry.
    #[must_use]
    pub fn background_status(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        self.inner.tool_runtime.background().snapshot(execution_id)
    }

    /// Requests cancellation of one background execution through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct.
    #[must_use]
    pub fn background_cancel(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        let reason = if matches!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent
        ) {
            CancellationReason::RuntimeShutdown
        } else {
            CancellationReason::UserRequested
        };
        self.inner
            .tool_runtime
            .background()
            .cancel_with_reason(execution_id, reason)
    }

    /// Commit bounded Job cancellation intent without cloning result or command presentation.
    pub(crate) fn background_cancel_intent(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<Option<ToolExecutionId>, crate::runtime::inbound::MailboxError> {
        let reason = if matches!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent
        ) {
            CancellationReason::RuntimeShutdown
        } else {
            CancellationReason::UserRequested
        };
        self.inner
            .tool_runtime
            .background()
            .cancel_intent(execution_id, reason)
    }
}
