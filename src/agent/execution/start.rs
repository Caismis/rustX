//! The one cancellation-vs-start arbitration of an actual model request, its
//! fused durable start commit, and the provider-observed input measurements
//! that outlive the request that produced them.

use std::sync::Arc;

#[cfg(test)]
use super::attribution;
use super::request::{FrozenLogicalInput, PreparedModelTurn};
use super::step::LogicalModelStepState;
use super::terminal::Terminal;
use super::{AgentExecution, DurableFailureKind};
use crate::agent::cancellation::StartAdjudication;
use crate::agent::observer::AgentExecutionObserver;
use crate::context::tokens::ObservedAnchor;
use crate::context::tokens::ProviderObservedInput;
use crate::durable::{ModelTurnStartCommit, ModelTurnStartCommitDisposition};
use crate::events::types::AttemptFailure;
use crate::message::types::MessageBlock;
use crate::model::types::{ModelRequest, ModelUsage};
use crate::runtime::identity::{AttemptId, MessageId};
use crate::runtime::types::RuntimeError;
use chrono::Utc;

/// One actual model request after it crossed the durable request-start
/// frontier. The request and provisional Assistant identity are kept
/// together so publication can never be attributed to a different actual
/// request generation.
pub(super) struct StartedModelTurn {
    #[cfg(feature = "issue-387-profile")]
    pub(super) adapter_input_profile: crate::model::adapter::profile::InputLayout,
    pub(super) request_start_ms: Option<u64>,
    pub(super) request: ModelRequest,
    pub(super) message_id: MessageId,
}

#[derive(Clone, PartialEq)]
pub(super) struct RequestObservationIdentity {
    pub(super) invocation: crate::model::ModelInvocationConfig,
    pub(super) continuation: Option<crate::runtime::continuation::ProviderContinuationState>,
}

/// Publishes the newly committed start-owned Event Journal facts through the
/// live execution observer in their durable order. An idempotent retry gets a
/// receipt for historical verification, but must not replay those historical
/// facts into a live observer.
fn publish_model_turn_start_events(
    observer: Option<&dyn AgentExecutionObserver>,
    attempt_id: &AttemptId,
    commit: &ModelTurnStartCommit,
) {
    if commit.disposition != ModelTurnStartCommitDisposition::NewlyCommitted {
        return;
    }
    for event in commit.events() {
        if let Some(observer) = observer {
            observer.observe_event(
                attempt_id,
                &event.event,
                event.sequence,
                event.tool_occurrence.as_ref(),
                event.observation.as_deref(),
            );
        }
    }
}

/// The provider-observed input of this attempt and the identity it was
/// measured under, plus the facts of the last started request that a later
/// provider report is promoted from. Unlike the request that produced it, a
/// measurement survives into later compatible requests and logical steps; it
/// is withdrawn only when the request state it described is replaced.
#[derive(Default)]
pub(super) struct RequestMeasurements {
    pub(super) observed: Option<ProviderObservedInput>,
    /// The request identity [`Self::observed`] was measured under.
    ///
    /// A provider counted the input of one concrete request. That count is
    /// evidence about a later request only while the parts of the request
    /// the conversation projection does not describe — the resolved
    /// invocation and the opaque provider continuation — are still the
    /// same. When they are not, the measurement is dropped rather than
    /// stretched over a request the provider never measured.
    pub(super) observed_request_identity: Option<Arc<RequestObservationIdentity>>,
    /// The request identity of the last started request, promoted alongside
    /// its measurement when the provider reports one.
    pub(super) last_request_identity: Option<Arc<RequestObservationIdentity>>,
    /// The structural anchor of the last started request, kept so its
    /// provider-reported input measurement can be reused for every later
    /// request context that extends it.
    pub(super) last_request_anchor: Option<ObservedAnchor>,
    /// The runtime's deterministic input estimate for the last started
    /// request, kept so a provider overflow can be turned into a measured
    /// estimator correction instead of being retried against the same
    /// budget.
    pub(super) last_request_estimated_input: Option<u64>,
}

impl RequestMeasurements {
    /// Records the facts of one request that crossed the durable start
    /// frontier, so its provider report can later be promoted.
    fn started(
        &mut self,
        identity: Arc<RequestObservationIdentity>,
        anchor: ObservedAnchor,
        estimated_input: u64,
    ) {
        self.last_request_identity = Some(identity);
        self.last_request_anchor = Some(anchor);
        self.last_request_estimated_input = Some(estimated_input);
    }

    /// Withdraws the measurement once the request state it described
    /// (provider continuation or replayed history) has been replaced.
    pub(super) fn withdraw(&mut self) {
        self.observed = None;
        self.observed_request_identity = None;
    }
}
impl AgentExecution<'_> {
    /// The one cancellation-vs-start linearization point of every model
    /// turn (Issue #12, M9b): the first turn, every tool→model
    /// continuation, every recovered continuation, every transient retry, and
    /// every overflow retry reach exactly this boundary.
    ///
    /// The attempt's model-turn start gate serializes the cancellation
    /// observation against the durable start commit
    /// (`ConversationStore::commit_model_turn_start`: the request-scoped
    /// context, the immutable Request Snapshot, and the complete typed start
    /// receipt — `ModelRequestStarted` followed by any contribution emission
    /// facts — in one transaction). Exactly one side
    /// wins:
    ///
    /// - cancellation linearized first ⇒ the prepared turn is discarded:
    ///   no provider invocation, no `ModelRequestStarted`, no started
    ///   Request Snapshot, no request-scoped context commit, and no start
    ///   claim left behind;
    /// - the durable start commit succeeded ⇒ the request has durably
    ///   started; the validated context installs infallibly, the start fact
    ///   is recorded, and only then may the provider be invoked. A
    ///   cancellation that raced the commit is post-start cancellation of
    ///   the now-started request;
    /// - the durable start commit failed ⇒ a durable-authority failure
    ///   (Issue #63): no provider invocation, no start fact, no partial
    ///   request-owned state, and the coordinator learns the durable
    ///   failure kind.
    #[allow(clippy::too_many_lines)]
    fn start_model_turn(
        &mut self,
        prepared: PreparedModelTurn,
    ) -> Result<StartedModelTurn, Terminal> {
        #[cfg(test)]
        let _profile = attribution::Timer::new(attribution::Phase::Start);
        #[cfg(test)]
        if let Some(pause) = self
            .pauses
            .start_boundary_pause
            .lock()
            .expect("start boundary pause lock")
            .as_ref()
        {
            pause.park_before_start_arbitration();
        }
        let store = std::sync::Arc::clone(&self.store);
        let arbitration = self.cancellation.arbitrate_model_turn_start(|| {
            #[cfg(test)]
            if let Some(pause) = self
                .pauses
                .start_boundary_pause
                .lock()
                .expect("start boundary pause lock")
                .as_ref()
            {
                pause.park_before_start_commit();
            }
            let context: Vec<MessageBlock> = prepared
                .context
                .iter()
                .map(|commit| commit.message().clone())
                .collect();
            // Pair the monotonic origin with the exact timestamp this transaction
            // persists. The cancellation gate still owns start arbitration; no
            // dispatch can precede commit or reconstruction verification.
            let request_start_ms = self.monotonic_clock.now_millis();
            let timestamp = Utc::now();
            let committed = {
                #[cfg(test)]
                let _profile = attribution::Timer::new(attribution::Phase::Commit);
                store.commit_model_turn_start(&context, &prepared.snapshot, timestamp)
            };
            committed.map(|commit| (commit, request_start_ms))
        });
        let (start_commit, request_start_ms) = match arbitration {
            Ok(StartAdjudication::CancelledBeforeStart) => {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            Ok(StartAdjudication::Started(start_commit)) => start_commit,
            Err(error) => {
                // The durable start transaction failed: no start fact, no
                // request-scoped context, and no provider invocation
                // exist. This is a durable-authority failure (Issue #63),
                // never a context-preparation failure.
                self.durable_failure_kind = Some(DurableFailureKind::RequestStart);
                self.durable_failure = Some(format!(
                    "request start could not be committed durably: {error}"
                ));
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "request start could not be committed durably: {error}"
                            ),
                        },
                    },
                });
            }
        };
        // The durable start commit won: the request-scoped context installs
        // infallibly (validated at preparation, still exact), the start fact
        // is recorded, and only after this point may the provider be
        // invoked.
        self.logical_step.last_started_request = Some(prepared.snapshot.identity.clone());
        self.logical_step.model_step = LogicalModelStepState::Unresolved;
        for (commit, receipt) in prepared
            .context
            .into_iter()
            .zip(&start_commit.context_receipts)
        {
            let block = commit.message().clone();
            self.conversation.install_prepared(commit);
            if let Some(observer) = self.observer {
                // Request-scoped Context-kind messages are model history, not
                // ordinary transcript items, so they carry no transcript
                // cursor and never create a semantic user echo.
                observer.observe_committed(&self.request.attempt_id, &block, receipt);
            }
        }
        if let Some(observer) = self.observer {
            for (ordinal, contribution) in prepared.snapshot.contributions.iter().enumerate() {
                if prepared
                    .snapshot
                    .request_context_ids
                    .contains(&contribution.message_id)
                {
                    contribution.observe(
                        observer,
                        &self.request.attempt_id,
                        self.turn,
                        start_commit.started.observation.as_ref().and_then(|_| {
                            start_commit
                                .status_sources
                                .iter()
                                .find(|source| source.contribution_ordinal == ordinal as u64)
                        }),
                    );
                }
            }
        }
        // The receipt is the only live publication source for the start-owned
        // Event Journal facts. Publish only a fresh durable transition; an
        // idempotent store verification returns the historical receipt but
        // must not replay those facts to a live observer.
        publish_model_turn_start_events(self.observer, &self.request.attempt_id, &start_commit);
        self.measurements.started(
            prepared.request_identity.clone(),
            prepared.anchor.clone(),
            prepared.estimated_input,
        );
        self.last_request_id = Some(prepared.snapshot.request_id.clone());
        let accepts_prepared = {
            #[cfg(test)]
            let _profile = attribution::Timer::new(attribution::Phase::CommitEvidence);
            start_commit.accepts_prepared(&prepared.snapshot, &prepared.request)
        };
        if !accepts_prepared {
            self.durable_failure_kind = Some(DurableFailureKind::RequestStart);
            let message =
                "durable request commit evidence differs from the immutable prepared input"
                    .to_owned();
            self.durable_failure = Some(message.clone());
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }
        self.logical_step.current_input = Some(Box::new(FrozenLogicalInput {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile: prepared.adapter_input_profile,
            snapshot: prepared.snapshot.clone(),
            request: prepared.request.clone(),
            anchor: prepared.anchor.clone(),
            request_identity: prepared.request_identity.clone(),
            estimated_input: prepared.estimated_input,
        }));
        Ok(StartedModelTurn {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile: prepared.adapter_input_profile,
            // A replay's old timestamp must never be paired with a new clock read.
            request_start_ms: (start_commit.disposition
                == ModelTurnStartCommitDisposition::NewlyCommitted)
                .then_some(request_start_ms),
            request: prepared.request,
            message_id: prepared.snapshot.provisional_message_id,
        })
    }

    /// Starts one actual request and advances the logical step's shared
    /// ordinal only after the durable start frontier has succeeded. A
    /// cancellation that wins before that frontier therefore leaves the
    /// ordinal available, while every committed request consumes exactly one
    /// collision-free generation.
    pub(super) fn start_actual_model_request(
        &mut self,
        prepared: PreparedModelTurn,
        next_request_ordinal: &mut u32,
    ) -> Result<StartedModelTurn, Terminal> {
        let started = self.start_model_turn(prepared)?;
        *next_request_ordinal = next_request_ordinal
            .checked_add(1)
            .expect("actual request ordinal cannot overflow");
        Ok(started)
    }

    /// The provider measurement that is still evidence about the request
    /// this attempt is about to make, or `None`.
    ///
    /// A provider counted the input of one concrete request. The context
    /// engine decides whether the measured *conversation* is this request's
    /// conversation or a prefix of it — but the engine only ever sees the
    /// projection, the Effective System Prompt, and the tool definitions. It
    /// cannot see the rest of the request. This gate supplies what it cannot
    /// see: the resolved invocation and the opaque provider continuation.
    ///
    /// The continuation is why this matters in practice. Under a stored
    /// continuation the provider bills the entire referenced prior
    /// generation as input, so two requests with the same visible messages
    /// can have been measured against completely different context. Handing
    /// the engine a measurement from a different continuation would produce
    /// a confident, provider-attributed number for a request no provider
    /// ever measured — and that number decides compaction.
    pub(super) fn reusable_observation(&self) -> Option<&ProviderObservedInput> {
        let observed = self.measurements.observed.as_ref()?;
        let identity = self.measurements.observed_request_identity.as_deref()?;
        (identity.invocation == self.request.model.primary().invocation_config()
            && identity.continuation == self.pending_continuation)
            .then_some(observed)
    }

    /// Retains the latest trustworthy provider input measurement for the
    /// exact actual request that produced it. The context engine may reuse it
    /// only while its structural anchor and request identity remain
    /// compatible; no estimate is promoted here.
    pub(super) fn record_provider_observation(&mut self, usage: &ModelUsage) {
        let Some(anchor) = self.measurements.last_request_anchor.take() else {
            return;
        };
        self.measurements.observed = Some(ProviderObservedInput {
            input_tokens: usage.input_tokens,
            anchor,
        });
        // The measurement travels with the request identity it was taken
        // under, so a later request that changed invocation or continuation
        // cannot inherit it.
        self.measurements
            .observed_request_identity
            .clone_from(&self.measurements.last_request_identity);
    }
}

#[cfg(test)]
mod tests;
