//! Consumption of one started provider request: dispatch, deadline and
//! generation-guard arbitration, canonical event assembly, and settlement of
//! the provider outcome fact.

use super::AgentExecution;
use super::start::StartedModelTurn;
use super::terminal::Terminal;
use crate::agent::assembly::ModelEventAssembler;
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::model::adapter::{ModelStream, ModelStreamItem};
use crate::model::deadline::ModelRequestDeadline;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::generation::GenerationGuard;
use crate::model::generation_evidence::GenerationTiming;
use crate::runtime::identity::MessageId;
use crate::runtime::types::RuntimeError;
use futures_util::StreamExt;
use futures_util::future::{BoxFuture, pending};

/// How one model stream of a turn ended.
pub(super) enum StreamTerminal {
    /// The canonical assembler owns the successful terminal data; this
    /// marker only tells the loop which control-flow path to take.
    Completed,
    Failed {
        error: ModelError,
    },
}

/// One completed model invocation: the provisional message identity, the
/// assembler holding the provisional stream content, and the stream
/// terminal.
///
/// The three pieces travel together: a recovery retry replaces the whole
/// invocation, so provisional output and tool calls of the failed request
/// are never committed under the retry's message identity.
pub(super) struct ModelInvocation {
    pub(super) message_id: MessageId,
    pub(super) assembler: ModelEventAssembler,
    pub(super) terminal: StreamTerminal,
}

impl<'a> AgentExecution<'a> {
    /// Consumes one started model invocation: sends the exact request that
    /// crossed the durable start frontier, assembles its provisional stream
    /// under the identity from that same snapshot, and returns the complete
    /// invocation (identity + assembler + terminal).
    pub(super) async fn consume_invocation(
        &mut self,
        started: StartedModelTurn,
    ) -> Result<ModelInvocation, Terminal> {
        let StartedModelTurn {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile,
            request_start_ms,
            request,
            message_id,
        } = started;
        // The provider binding comes from the attempt's frozen snapshot: the
        // loop never resolves an adapter and never observes a later session
        // model change.
        // Start the response-start clock at the dispatch frontier, immediately
        // after durable reconstruction verification and before entering the
        // adapter's synchronous dispatch method.
        let dispatch_frontier_ms = self.monotonic_clock.now_millis();
        let deadline = ModelRequestDeadline::new(self.model_timeout_policy, dispatch_frontier_ms);
        // Generation timing evidence shares the deadline's and the guard's
        // exact request-local lifetime and origin. Installing it here — after
        // durable reconstruction verification, before the adapter stream
        // exists — means time to first output measures generation rather than
        // request preparation, and a retry cannot inherit the spans of the
        // generation it replaces.
        self.generation_timing = Some(GenerationTiming::started_at(
            dispatch_frontier_ms,
            request_start_ms,
        ));
        // The generation guard is request-local execution state exactly like
        // the deadline: one physical generation, one guard, derived from the
        // attempt's own frozen resolved output budget. It is created here,
        // at the same dispatch frontier, so a retry can never inherit the
        // accounting or the repetition window of the generation it replaces.
        let guard = GenerationGuard::new(self.generation_policy);
        #[cfg(feature = "issue-387-profile")]
        let adapter_dispatch_profile = crate::model::adapter::profile::NativeScope::new(
            self.last_request_id
                .as_ref()
                .expect("native started request")
                .as_str(),
            adapter_input_profile,
        );
        let stream = self
            .request
            .model
            .primary()
            .adapter()
            .stream(request, self.cancellation.model_cancellation());
        #[cfg(feature = "issue-387-profile")]
        drop(adapter_dispatch_profile);
        let mut assembler = ModelEventAssembler::new();
        let terminal = match self
            .consume_model_stream(&mut assembler, &message_id, stream, deadline, guard)
            .await
        {
            Ok(stream_terminal) => stream_terminal,
            Err(terminal) => return Err(terminal),
        };
        Ok(ModelInvocation {
            message_id,
            assembler,
            terminal,
        })
    }

    /// Consumes one model stream: emits runtime events for non-terminal
    /// model facts, feeds the assembler, and validates the canonical stream
    /// contract. Returns the stream terminal, or the attempt terminal when
    /// the attempt must settle before the stream finished.
    #[allow(clippy::too_many_lines)]
    async fn consume_model_stream(
        &mut self,
        assembler: &mut ModelEventAssembler,
        assistant_message_id: &MessageId,
        mut stream: ModelStream,
        mut deadline: ModelRequestDeadline,
        mut guard: GenerationGuard,
    ) -> Result<StreamTerminal, Terminal> {
        // The response-start deadline was created at the dispatch frontier,
        // after the durable RequestSnapshot + ModelRequestStarted commit and
        // reconstruction verification. A fresh deadline state belongs to this
        // exact actual request; retries create another one at their own
        // frontier.
        let mut stream_terminal = None;
        let mut stream_started = false;
        #[cfg(test)]
        let mut model_stream_item_count = 0_u32;
        loop {
            // Every provider wait has the same four-way arbitration. An
            // empty publication buffer contributes a permanently pending
            // branch rather than changing the provider/timeout semantics.
            let publication_wait: BoxFuture<'static, ()> = self
                .publication
                .as_ref()
                .and_then(|publication| publication.coalescer.latency_wait())
                .unwrap_or_else(|| Box::pin(pending::<()>()));
            // The pending deadline is the liveness contract this request is
            // currently running against, together with the instant it is
            // violated. Taking both from one place is what makes an
            // impossible expiry unrepresentable: a request that owns no
            // deadline yields nothing to wait on *and* no phase to report,
            // so the timeout branch below can never invent one.
            let pending_deadline = deadline.pending();
            let timeout_wait: BoxFuture<'static, ()> = match pending_deadline {
                Some((_, deadline_millis)) => {
                    self.monotonic_clock.wait_until_millis(deadline_millis)
                }
                None => Box::pin(pending::<()>()),
            };
            let next = tokio::select! {
                biased;
                // This order is contractual: provider stream item > attempt
                // cancellation > publication flush > request timeout.
                item = stream.next() => item,
                () = self.cancellation.cancelled() => {
                    if let Some(terminal) = stream_terminal.take() {
                        return Ok(terminal);
                    }
                    // Timeout never uses or mutates AgentCancellation. If
                    // cancellation wins this cut, retain its provenance.
                    return Err(self.cancelled_terminal());
                }
                () = publication_wait => {
                    self.flush_publication();
                    if let Some(terminal) = self.durable_failure_terminal_from_state() {
                        return Err(terminal);
                    }
                    // Publication activity is not provider progress and does
                    // not reset the request-local stream-idle deadline.
                    continue;
                }
                () = timeout_wait => {
                    // Only a pending deadline can select this branch, and it
                    // carries the phase that expired. Transport liveness is
                    // not generation integrity: this is a transient model
                    // failure on the existing retry budget, it carries no
                    // generation detail, and it never touches the semantic
                    // corrective-generation budget.
                    let Some((phase, _)) = pending_deadline else {
                        return Err(Terminal::Failed {
                            failure: AttemptFailure::Runtime {
                                error: RuntimeError::ContractViolation {
                                    message: "a model request deadline expired with no pending deadline"
                                        .to_owned(),
                                },
                            },
                        });
                    };
                    // The pull-based adapter stream is the request-local
                    // authority. Dropping it terminates adapter work without
                    // touching the attempt cancellation authority.
                    drop(stream);
                    let error = ModelError::timeout(phase);
                    self.settle_model_request_failure(assembler, &error)?;
                    return Ok(StreamTerminal::Failed { error });
                }
            };
            let Some(item) = next else {
                if let Some(terminal) = stream_terminal.take() {
                    return Ok(terminal);
                }
                break;
            };
            #[cfg(test)]
            {
                model_stream_item_count = model_stream_item_count.saturating_add(1);
            }
            if stream_terminal.is_some() {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: "model stream item after the terminal event".to_owned(),
                        },
                    },
                });
            }
            // The provider branch is intentionally ahead of cancellation in
            // the biased select. Do not perform a second cancellation check
            // here: doing so would make an item ready at the same cut lose to
            // cancellation after arbitration already chose it. The one
            // exception is the adapter's normalized cancellation terminal.
            if matches!(
                &item,
                ModelStreamItem::Event(ModelEvent::Failed { error })
                    if error.kind == ModelErrorKind::Cancelled
                        && self.cancellation.is_cancelled()
            ) {
                return Err(self.cancelled_terminal());
            }
            if matches!(&item, ModelStreamItem::Progress(_)) && !stream_started {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: "model stream progress before Started".to_owned(),
                        },
                    },
                });
            }
            let observed_at_ms = self.monotonic_clock.now_millis();
            deadline.observe(&item, observed_at_ms);
            if let (ModelStreamItem::Event(event), Some(timing)) =
                (&item, self.generation_timing.as_mut())
            {
                // Observed before generation-integrity and budget evaluation
                // so that a generation discarded as degenerate still reports
                // when the provider actually started producing output.
                timing.observe(event, observed_at_ms);
            }
            // Generation integrity and generation budget are evaluated here,
            // *before* the item reaches assembly or publication: a delta that
            // proves the generation is unusable is not part of a message this
            // loop will ever commit. Liveness has already been observed above
            // and is deliberately unaffected — a degenerate stream is a live
            // stream, which is exactly why the deadline cannot bound it.
            if let Some(failure) = guard.observe(&item) {
                // The pull-based adapter stream is the request-local
                // authority. Dropping it terminates adapter work without
                // touching the attempt cancellation authority.
                drop(stream);
                let error = self.settle_generation_anomaly(assembler, failure)?;
                return Ok(StreamTerminal::Failed { error });
            }
            match item {
                ModelStreamItem::Progress(_) => {
                    // Progress is provider-derived execution state only. It
                    // participates in deadline observation but never enters
                    // canonical assembly, publication, or durable history.
                }
                ModelStreamItem::Event(event) => {
                    if let Err(error) = assembler.push(&event) {
                        return Err(Terminal::Failed {
                            failure: AttemptFailure::Runtime { error },
                        });
                    }
                    match &event {
                        ModelEvent::Completed { finish_reason, .. } => {
                            // A provider that stopped at its own token limit
                            // is stating that the generation is incomplete.
                            // Fail closed here, before the terminal can reach
                            // the canonical commit path, rather than
                            // committing a truncated assistant turn.
                            if let Some(failure) =
                                GenerationGuard::classify_completion(finish_reason)
                            {
                                let error = self.settle_generation_anomaly(assembler, failure)?;
                                stream_terminal = Some(StreamTerminal::Failed { error });
                            } else {
                                stream_terminal = Some(StreamTerminal::Completed);
                            }
                        }
                        ModelEvent::Failed { error } => {
                            self.settle_model_request_failure(assembler, error)?;
                            stream_terminal = Some(StreamTerminal::Failed {
                                error: error.clone(),
                            });
                        }
                        ModelEvent::Started => {
                            stream_started = true;
                            // The provider stream physically began: the
                            // publication stream opens durably before any of
                            // its output can be staged, so a crash always
                            // leaves either no stream at all or a stream
                            // recovery can classify.
                            self.open_publication(assistant_message_id);
                            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                                return Err(terminal);
                            }
                        }
                        _ => {
                            self.publish_model_event(&event);
                            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                                return Err(terminal);
                            }
                        }
                    }
                }
            }
            #[cfg(test)]
            if let Some(pause) = self
                .pauses
                .model_stream_item_pause
                .lock()
                .expect("model stream item pause lock")
                .as_ref()
            {
                pause.park();
            }
            #[cfg(test)]
            {
                let arbitration_pause = self
                    .pauses
                    .model_arbitration_pause
                    .lock()
                    .expect("model arbitration pause lock")
                    .take();
                if let Some(pause) = arbitration_pause {
                    if pause.after_items() == model_stream_item_count {
                        // This one-shot hook is after event processing and
                        // before the next provider wait. The controller can
                        // make every competing readiness source observable
                        // before this exact arbitration is polled.
                        pause.wait().await;
                    } else {
                        *self
                            .pauses
                            .model_arbitration_pause
                            .lock()
                            .expect("model arbitration pause lock") = Some(pause);
                    }
                }
            }
        }
        Err(Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContractViolation {
                    message: "model stream ended without a terminal event".to_owned(),
                },
            },
        })
    }

    /// Settles the one durable provider failure for the current actual
    /// request. Both adapter failures and runtime-owned timeouts use this
    /// path, so usage retention and exactly-once `ModelRequestFailed`
    /// emission cannot diverge between them.
    pub(super) fn settle_model_request_failure(
        &mut self,
        assembler: &ModelEventAssembler,
        error: &ModelError,
    ) -> Result<(), Terminal> {
        // A failed request may have emitted one or more cumulative usage
        // updates before the failure. Keep only the latest trustworthy
        // snapshot from this exact request; never synthesize usage from
        // partial output and never combine snapshots.
        let usage = assembler.latest_usage();
        if let Some(usage) = usage.as_ref() {
            self.record_provider_observation(usage);
        }
        let Some(request_id) = self.last_request_id.clone() else {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: "a model failure has no started request".to_owned(),
                    },
                },
            });
        };
        // The stream is consumed or dropped by the caller before this path
        // returns. No late adapter terminal can re-enter the loop and create
        // a second outcome for this RequestId.
        let generation = self.settle_generation_evidence();
        self.emit(RuntimeEvent::ModelRequestFailed {
            request_id,
            error: error.clone(),
            usage,
            generation,
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        Ok(())
    }

    /// Settles the in-flight request's generation timing evidence at its
    /// provider terminal.
    ///
    /// Taking the accumulator is what makes the evidence exactly-once: the
    /// completion and failure paths call this from the same position in
    /// their own terminal emission, so a request cannot record two spans and
    /// a second terminal for the same request cannot invent one. A request
    /// whose terminal is recorded without an installed accumulator — no
    /// adapter stream was ever created — truthfully reports no evidence.
    pub(super) fn settle_generation_evidence(
        &mut self,
    ) -> Option<crate::model::generation_evidence::GenerationEvidence> {
        let terminal_ms = self.monotonic_clock.now_millis();
        self.generation_timing
            .take()
            .map(|timing| timing.settle(terminal_ms))
    }
}
