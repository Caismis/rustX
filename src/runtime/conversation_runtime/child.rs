//! Subagent-child coordination: parent guidance, child turn gating, child
//! settlement observation, and the child's physical lifetime.

use super::{
    ConversationRuntime, DrainTrigger, DurableOperation, InboundAdmission, InboundAdmissionError,
    NativeAttemptSettlement, SemanticInboundClass, UserContentBlock, UserSource,
};

/// The result of one child-conversation terminal seal evaluation (Issue
/// #193).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentGuidanceSeal {
    /// The seal committed: no accepted guidance remains unobserved and none
    /// can be accepted afterwards. The child may report its terminal.
    Sealed,
    /// Semantic work is still owed — an unobserved admitted attempt, a live
    /// attempt, or accepted guidance still pending adoption. The child must
    /// observe another ordinary attempt terminal before it may seal.
    Open,
    /// The seal could not be *proven*: the durable Pending Inbound Inbox
    /// read failed, so the runtime cannot rule out an accepted, unadopted
    /// guidance. The runtime has committed its absorbing durability-failure
    /// fact; the child must fail closed and must not publish a successful
    /// terminal.
    DurabilityFailed {
        /// The bounded diagnostic of the failed durable read.
        diagnostic: String,
    },
}

impl ConversationRuntime {
    /// Submits one **parent-authored in-flight guidance** message into this
    /// child conversation (Issue #193).
    ///
    /// This is the child half of steering, and it is deliberately the same
    /// admission path as every other semantic inbound: the guidance becomes
    /// an ordinary durable inbound item that the ordinary Agent Loop adopts
    /// at its ordinary safe boundary. It carries no model, tool, skill,
    /// instruction, workspace, definition, or execution authority, so it can
    /// never re-author the child's frozen launch authority, and it never
    /// creates an attempt, a conversation, or a lifecycle of its own.
    ///
    /// # The acceptance contract
    ///
    /// Success means exactly: *the guidance is durably accepted into this
    /// child conversation's Pending Inbound Inbox, ahead of this
    /// conversation's terminal seal.* Because the seal is what lets the
    /// activation driver publish a terminal at all, it follows that a
    /// **naturally completing** child cannot publish an answer that
    /// predates this guidance.
    ///
    /// It does **not** mean the child model has observed it, that the
    /// in-flight provider request or tool call was interrupted, that
    /// anything changed yet, or that observation is guaranteed: a later
    /// cancellation of this activation, or physical loss of the child
    /// process, legitimately ends the activation with accepted guidance
    /// unobserved. Cancellation stays authoritative.
    ///
    /// The parent registry owns admission against stopping and interruption.
    /// Messages already admitted there must remain durably deliverable even
    /// after child-local cancellation. The child coordinator serializes the
    /// durable acceptance against these remaining refusal boundaries:
    ///
    /// - the conversation's terminal seal already committed
    ///   ([`InboundAdmissionError::GuidanceSealed`]);
    /// - the ordinary lifecycle/durability gates.
    ///
    /// The seal grant follows all previously admitted messages on the same
    /// FIFO control lane. Child cancellation cannot revoke their acceptance.
    ///
    /// Multiple accepted guidance messages preserve their acceptance order
    /// by the durable inbox's own `InboundSequence` domain; no scheduler
    /// ordering is involved.
    ///
    /// # Errors
    ///
    /// Returns the same variants as
    /// [`ConversationRuntime::submit_sourced_inbound`], plus
    /// [`InboundAdmissionError::GuidanceSealed`].
    pub(crate) fn submit_parent_guidance(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.admit_sourced_inbound(source, content, SemanticInboundClass::ParentGuidance)
    }

    /// Commits this child conversation's **terminal seal** for
    /// parent-authored guidance (Issue #193), reports that semantic work is
    /// still owed, or fails closed.
    ///
    /// `observed_terminals` is how many attempt terminals the caller has
    /// already consumed. Under the one coordinator lock — the same lock that
    /// owns durable inbound acceptance — the seal commits only when the
    /// runtime **positively proves** that nothing can still carry accepted
    /// guidance into a model turn:
    ///
    /// - no admitted attempt is unobserved (`admitted_attempts` does not
    ///   exceed `observed_terminals`);
    /// - no attempt is live;
    /// - the durable Pending Inbound Inbox is *proven* empty.
    ///
    /// Because acceptance and the seal share that lock, a guidance accepted
    /// before the seal is necessarily visible to it — the seal then answers
    /// [`ParentGuidanceSeal::Open`] and the ordinary coordinator admits the
    /// turn that observes it — and a guidance arriving after the seal is
    /// necessarily refused. There is no interleaving in which a durably
    /// accepted guidance is silently discarded by a natural terminal.
    ///
    /// # Failing closed
    ///
    /// A durable read failure is **not** evidence that the inbox is empty,
    /// so it can never be folded into `Ok(false)`. The three answers are
    /// exactly:
    ///
    /// - `Ok(true)`  -> [`ParentGuidanceSeal::Open`]: semantic work remains;
    /// - `Ok(false)` -> eligible to seal;
    /// - `Err(..)`   -> [`ParentGuidanceSeal::DurabilityFailed`]: the seal
    ///   does **not** commit and the caller must not publish a successful
    ///   terminal.
    ///
    /// The failure is routed into the runtime's one absorbing
    /// durability-failure authority ([`DurabilityGate::commit_failure`], via
    /// [`DurableOperation::ParentGuidanceSeal`], which is non-transient by
    /// construction) before it is returned, so the degraded state is
    /// observable exactly once and the runtime rejects further durable work.
    ///
    /// The seal is absorbing for one activation. It is used by that activation
    /// driver; an ordinary interactive conversation never seals, because its
    /// coordinator simply admits the next attempt.
    ///
    /// # Who may seal
    ///
    /// Only a **steerable** normal asynchronous subagent child ever consults
    /// the seal. A Workflow-owned `AgentRun` (terminal mode
    /// [`crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput`]) is
    /// structurally not steerable:
    /// the generic control plane refuses a steer before any `Guidance` frame
    /// exists, so no accepted guidance can ever be pending in its
    /// conversation. Its natural completion is therefore its terminal, and
    /// `serve_child_delegation` never calls this method for it — the
    /// steering-specific seal can never add a failure surface to Workflow
    /// execution. The isolation is decided from the child's frozen terminal
    /// mode (explicit ownership), never from incidental timing.
    pub(crate) fn gate_child_turns(&self) {
        let mut state = self.inner.lock_state();
        state.parent_turn_permits = Some(0);
    }

    pub(crate) fn child_activity(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::runtime::subagent::activity::SubagentObservation> {
        self.inner
            .pending
            .get()
            .expect("native observation bridge")
            .child_activity()
    }

    /// Test capture of the existing retained execution receipt; no display cursor.
    #[cfg(test)]
    pub(crate) fn settlement_ordinal_for_test(&self) -> u64 {
        self.inner
            .lock_state()
            .latest_settlement
            .as_ref()
            .map_or(0, |receipt| receipt.ordinal)
    }

    /// Waits for the native execution handoff independently of display publication.
    /// Only the child driver uses this receipt: its existing turn permit/seal
    /// protocol prevents a later admission from overwriting an unconsumed result.
    pub(crate) async fn child_attempt_settlement(&self, after: u64) -> NativeAttemptSettlement {
        loop {
            let notified = self.inner.settlement.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(receipt) = self
                .inner
                .lock_state()
                .latest_settlement
                .as_ref()
                .filter(|receipt| receipt.ordinal > after)
                .cloned()
            {
                return receipt;
            }
            notified.await;
        }
    }

    pub(crate) fn release_child_turn(&self) {
        let mut state = self.inner.lock_state();
        if let Some(permits) = &mut state.parent_turn_permits {
            *permits = permits.saturating_add(1);
        }
        self.inner.wake.notify.notify_one();
    }

    pub(crate) async fn seal_parent_guidance(&self, observed_terminals: u64) -> ParentGuidanceSeal {
        // Test-only gate: parked before the coordinator lock, so a competing
        // guidance submission can still take that lock and durably accept
        // while the seal is provably pending. The gate handle is extracted
        // before the park so the probe mutex is not held while parked.
        #[cfg(test)]
        let seal_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.parent_guidance_seal_gate.clone());
        #[cfg(test)]
        if let Some(gate) = seal_gate {
            tokio::task::block_in_place(|| gate.enter());
        }
        loop {
            // Armed before the state is read, so a settlement that fires
            // while the lock is held is never missed.
            let settled = self.inner.settlement.notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            {
                let mut state = self.inner.lock_state();
                if state.parent_guidance_sealed {
                    return ParentGuidanceSeal::Sealed;
                }
                if state.admitted_attempts > observed_terminals {
                    // An attempt exists whose terminal the caller has not
                    // consumed: accepted guidance may still be running.
                    return ParentGuidanceSeal::Open;
                }
                if state.current_attempt.is_none() {
                    // Accepted guidance still pending means the ordinary
                    // coordinator will admit the attempt that adopts it, so
                    // the child owes one more terminal observation.
                    //
                    // The seal needs a *positive* proof of emptiness. An
                    // unreadable inbox proves nothing, so it can never be
                    // read as "nothing pending": the runtime enters its
                    // absorbing durability-failure state and the caller is
                    // told the seal did not commit.
                    match self.inner.mailbox.has_pending() {
                        Ok(true) => return ParentGuidanceSeal::Open,
                        Ok(false) => {}
                        Err(error) => {
                            let diagnostic = format!(
                                "the child terminal seal could not verify the pending inbound inbox: {error}"
                            );
                            drop(state);
                            self.inner.commit_failure_and_observe(
                                DurableOperation::ParentGuidanceSeal,
                                diagnostic.clone(),
                            );
                            return ParentGuidanceSeal::DurabilityFailed { diagnostic };
                        }
                    }
                    state.parent_guidance_sealed = true;
                    return ParentGuidanceSeal::Sealed;
                }
            }
            // The observed attempt is still handing its state back; wait for
            // exactly that settlement rather than polling.
            settled.await;
        }
    }

    /// How many times the terminal seal's pending-inbox probe
    /// ([`ConversationInboundMailbox::has_pending`]) has been invoked on
    /// this child's mailbox (test-only, Issue #193).
    ///
    /// [`ConversationInboundMailbox::has_pending`] has exactly one
    /// production caller — the seal in
    /// [`ConversationRuntime::seal_parent_guidance`] — so this counter is a
    /// direct non-invocation proof that the parent-guidance seal machinery
    /// was never consulted (for example by a Workflow-owned child, which
    /// must never enter the generic-steering terminal protocol).
    #[cfg(test)]
    pub(crate) fn seal_probe_calls(&self) -> usize {
        self.inner.mailbox.pending_probe_calls()
    }

    /// Arms `count` consecutive durable failures of the terminal seal's
    /// pending-inbox probe (test-only, Issue #193).
    ///
    /// It targets exactly [`ConversationInboundMailbox::has_pending`], which
    /// only the seal calls, so the fail-closed contract can be proven
    /// without arming a store-wide select fault that the coordinator's own
    /// admission loop would race for.
    #[cfg(test)]
    pub(crate) fn arm_seal_probe_failures(&self, count: usize) {
        self.inner.mailbox.arm_pending_probe_failures(count);
    }

    /// The child's physical lifetime joins the one native drain and consumes
    /// only its explicit physical result. Durable publication failure remains
    /// a logical shutdown error, but cannot erase already-proven containment.
    /// An inactive composition enters drain directly without ever opening
    /// semantic admission.
    pub(crate) async fn settle_child_physical_lifetime(
        &self,
        parent_lost: bool,
    ) -> Result<(), Vec<String>> {
        let trigger = if !self.is_activated() {
            DrainTrigger::PreparationRollback
        } else if parent_lost {
            DrainTrigger::ParentLost
        } else {
            DrainTrigger::RuntimeShutdown
        };
        let completion = self
            .inner
            .begin_drain_internal(trigger)
            .map_err(|error| vec![format!("{error:?}")])?;
        completion.wait_outcome().await.physical
    }
}

#[cfg(test)]
mod tests;
