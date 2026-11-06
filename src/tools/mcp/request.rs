//! The one physical MCP request owner: `dispatch_owned_request`.
//!
//! Every request one invocation sends — Resource requests, `tools/call` rounds and the
//! SEP-2663 `tasks/get`, `tasks/update`, and `tasks/cancel` requests — goes
//! through this one path. It owns the pre-frontier refusals, the
//! `send_cancellable_request` external-effect frontier, request-lifecycle
//! admission, caller progress ownership, cancellation arbitration, the
//! local release proof, protocol poisoning, and transport-loss recording.
//! It reports physical facts ([`super::outcome::McpPhysicalOutcome`]); the
//! meaning of a response belongs to each caller.

use std::sync::atomic::Ordering;
use std::time::Instant;

use rmcp::model::ClientRequest;
use rmcp::service::RoleClient;

use crate::tools::executor::ToolExecutionContext;

use super::outcome::{
    McpPhysicalOutcome, McpResponseOutcome, PostFrontierCancellation, RemoteCancellation,
    is_transport_loss,
};
use super::progress::{McpProgressLease, drain_remote_progress, report_remote_progress};
use super::result::failed_mcp;
use super::runtime::McpServerRuntime;
#[cfg(test)]
use super::test_sync;
use super::{bound_error, streamable_http};

impl McpServerRuntime {
    /// Dispatches **one** physical MCP request under every ownership rule of
    /// the `ToolInvocation` that owns it, and reports the physical facts
    /// (Issues #205, #242, #243).
    ///
    /// This is the whole MCP request path, and there is exactly one of it.
    /// The pre-frontier rejections, the `send_cancellable_request`
    /// external-effect frontier, request-lifecycle admission, progress
    /// ownership, cancellation arbitration, the local release proof, protocol
    /// poisoning, and transport-loss recording live here and nowhere else.
    /// Every request one invocation sends goes through it: standard Resource
    /// requests, the `tools/call` rounds of the MRTR driver, and the `tasks/get`, `tasks/update`, and
    /// `tasks/cancel` requests of a remote task one of those rounds
    /// materialized. No caller re-implements any of it, which is why the
    /// Tasks extension adds a protocol, not a second dispatch owner.
    ///
    /// What this function deliberately does **not** own is the *meaning* of a
    /// response: the outcome carries facts, and each caller applies its own
    /// result vocabulary — a `tools/call` result, a task snapshot, an
    /// acknowledgement — to them.
    ///
    /// The read side of the call gate is taken **per request**, not for the
    /// whole invocation. The gate's guarantee is "no call or notification that
    /// began before `close()` is still running when the write barrier is
    /// crossed", and that stays exactly true: while a round's human
    /// interaction or a task's poll wait is pending no request exists, and the
    /// next request re-takes the gate and observes `closed` before it can
    /// dispatch. Holding the gate across a human interaction — or across a
    /// remote task's whole lifetime — would instead make runtime drain wait
    /// for a person or for a server.
    #[allow(
        clippy::too_many_lines,
        reason = "the effect frontier and its ownership seams must stay in one function"
    )]
    pub(super) async fn dispatch_owned_request(
        &self,
        request: ClientRequest,
        ownership: McpRequestOwnership<'_>,
        context: &ToolExecutionContext<'_>,
        generation: u64,
        started: Instant,
    ) -> McpPhysicalOutcome {
        let _call_gate = self.call_gate.read().await;
        // ---------- before the external-effect frontier ----------
        //
        // Every rejection below happens while no request exists, so each one
        // is a proven ordinary outcome: rustX can prove no remote side
        // effect was possible, and claiming an unknown external outcome here
        // would be dishonest in the opposite direction.
        //
        // A generation that already violated the protocol never serves
        // another request as healthy, whether the violation arrived during an
        // earlier call or while the connection was idle.
        if let Some(violation) = self.poisoned_protocol_violation().await {
            return McpPhysicalOutcome::Refused {
                result: Box::new(failed_mcp(&violation, context, started)),
                detail: violation,
            };
        }
        if self.closed.load(Ordering::Acquire) {
            const CLOSED: &str = "MCP server runtime is closed";
            return McpPhysicalOutcome::Refused {
                result: Box::new(failed_mcp(CLOSED, context, started)),
                detail: CLOSED.to_owned(),
            };
        }
        // The pre-frontier cancellation checkpoint. For a `tools/call` it is
        // also the MRTR **continuation dispatch frontier** (Issue #242); for
        // a `tasks/get` or `tasks/update` it is the **task dispatch
        // frontier** (Issue #243). Cancellation observed here means the
        // request is never started, so a human response — or a poll wait —
        // that lost the cancellation race can never create fresh remote
        // ambiguity.
        //
        // The one exempt request is the cooperative `tasks/cancel`, which
        // exists *because* cancellation won and would be self-defeating to
        // refuse for that reason.
        if ownership.refused_by_cancellation() && context.cancellation.is_cancelled() {
            return McpPhysicalOutcome::RefusedByCancellation {
                reason: context.cancellation.reason(),
            };
        }
        #[cfg(test)]
        let is_task_poll = matches!(&request, ClientRequest::GetTaskRequest(_));
        let mut enqueue = std::pin::pin!(
            self.peer
                .send_cancellable_request(request, self.invocation_request_options(context))
        );
        // Polling the enqueue while holding the epoch guard makes a received
        // list_changed and the effect frontier choose exactly one winner.
        // Pending releases the guard; a later poll must validate again.
        let enqueued = tokio::select! {
            biased;
            () = context.cancellation.cancelled(), if ownership.refused_by_cancellation() => {
                return McpPhysicalOutcome::RefusedByCancellation {
                    reason: context.cancellation.reason(),
                };
            }
            enqueued = std::future::poll_fn(|cx| {
                let epoch = self.invalidation.lock();
                if ownership
                    .catalog_epoch
                    .is_some_and(|expected| epoch.epoch(&self.server_id) != expected)
                {
                    return std::task::Poll::Ready(None);
                }
                enqueue.as_mut().poll(cx).map(Some)
            }) => enqueued,
        };
        let Some(enqueued) = enqueued else {
            const STALE: &str = "MCP catalog changed before tools/call dispatch; refresh and adopt the validated catalog";
            return McpPhysicalOutcome::Refused {
                result: Box::new(failed_mcp(STALE, context, started)),
                detail: STALE.to_owned(),
            };
        };
        let mut handle = match enqueued {
            Ok(handle) => handle,
            Err(error) => {
                // rmcp rejects a request here only when its service event
                // loop is gone, so the request was never enqueued, never
                // serialized, and never written: the frontier was not
                // crossed and this is an ordinary failure. The generation
                // itself is proven unusable.
                self.note_transport_loss(&format!(
                    "the MCP service of '{}' is gone: {error}",
                    self.server_id
                ));
                let detail = format!(
                    "the MCP request was refused before dispatch and never reached the \
                     transport ({}): {}",
                    self.generation_tag(generation),
                    bound_error(&error.to_string())
                );
                return McpPhysicalOutcome::Refused {
                    result: Box::new(failed_mcp(&detail, context, started)),
                    detail,
                };
            }
        };
        // ---------- the external-effect frontier is crossed ----------
        //
        // The request id exists for the first time here, and there is no
        // await between the frontier and this admission, so an admitted
        // request always has a lifecycle entry before anything can read one.
        // The admission is this invocation's hold on that entry: dropping it
        // — on every path out of this function — is the request-local forget
        // point, which is why normal completion cleans up its own state
        // instead of leaving one record per historical request behind.
        let admission = self.admit_local_request(&handle.id);
        // This request's progress-consumer ownership, taken in the same
        // await-free step as the admission above and released only when this
        // frame ends — however it ends. It is what makes "a correlated
        // response arrived" and "the dispatching caller is gone" two
        // independent facts: the router forgets this request's progress only
        // once both are true, so a response can never delete evidence this
        // call has not claimed yet, and a call that exits before subscribing
        // still leaves nothing behind.
        //
        // Tool and Resource requests take one. A task-control request is transport
        // activity of rustX's own poll loop, and turning "another `tasks/get`
        // was dispatched" into Tool progress would fabricate liveness the
        // server never reported.
        let progress_lease = ownership.progress_of.map(|_| {
            self.handler
                .progress
                .lease(handle.id.clone(), handle.progress_token.clone())
        });
        // Test-only: holds this call inside the pre-subscription window so a
        // regression can prove that genuine remote progress arrived before
        // the subscription registration completed. No production path
        // installs a race, so this is a lock read that returns immediately.
        #[cfg(test)]
        if let Some(remote_name) = ownership.progress_of {
            test_sync::park_before_progress_subscription(
                remote_name,
                &handle.id,
                self.handler.progress.scope(),
            )
            .await;
        }
        // Subscribing after dispatch is unavoidable — rmcp mints the token
        // inside the request — but it is no longer a race: the outbound
        // dispatch seam registered this token as known and live before the
        // request could reach the server, so anything the peer sent for it
        // is already owned and is claimed here, including evidence the peer
        // sent before it answered.
        let mut progress = progress_lease.as_ref().map(McpProgressLease::subscribe);
        // The rustX-owned bound on a cancellation-exempt request. `None` for
        // every ordinary request: those are bounded by the invocation's own
        // cancellation and deadline contract, which this adapter neither owns
        // nor duplicates.
        #[cfg(test)]
        if is_task_poll {
            test_sync::park_task_arbitration(self.server_id.as_str(), &handle).await;
        }
        // One absolute bound covers both the acknowledgement wait and its
        // request-local release; observing an ACK does not restart the bound.
        let bound = ownership
            .response_bound()
            .map(|duration| tokio::time::Instant::now() + duration);
        let response = loop {
            tokio::select! {
                biased;
                // A correlated remote response always outranks cancellation
                // intent: it is proven remote terminality, and this bias is
                // the deterministic winner of the response-versus-
                // cancellation race.
                response = &mut handle.rx => break response,
                () = context.cancellation.cancelled(), if ownership.arbitrated_by_cancellation() => {
                    // Liveness evidence that already arrived is never
                    // discarded by arbitration: it genuinely happened before
                    // the cancellation intent won.
                    if let Some(progress) = progress.as_mut() {
                        drain_remote_progress(context, progress);
                    }
                    drop(progress);
                    // This call stops consuming progress here, so it
                    // relinquishes here. Consumed admission makes progress
                    // forgettable even if settlement reports OutcomeUnknown;
                    // remote uncertainty does not own local progress.
                    drop(progress_lease);
                    return self
                        .settle_post_frontier_cancellation(handle, admission.as_ref())
                        .await;
                }
                // The cancellation-exempt request's own bound. It is a
                // rustX-owned local fact, never settlement evidence: a
                // cooperative `tasks/cancel` that outlives it is abandoned
                // rather than allowed to hold this invocation open.
                () = async {
                    match bound {
                        Some(bound) => tokio::time::sleep_until(bound).await,
                        None => std::future::pending().await,
                    }
                } => {
                    drop(progress);
                    drop(progress_lease);
                    let termination = Self::terminate_local_request(admission.as_ref());
                    termination.settled().await;
                    return McpPhysicalOutcome::CancelledPostFrontier {
                        requested: RemoteCancellation::Abandoned,
                        terminated_local_request: termination.terminated_local_request(),
                    };
                }
                progress_item = async {
                    match progress.as_mut() {
                        Some(progress) => progress.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    // Genuine remote liveness evidence, forwarded through the
                    // one generic progress seam. It refreshes the Agent
                    // Loop's idle watchdog and can never extend the generic
                    // hard deadline, which this adapter neither owns nor
                    // observes.
                    if let Some(progress_item) = progress_item {
                        report_remote_progress(context, progress_item);
                    }
                }
            }
        };
        #[cfg(test)]
        if let Some(ownership) = &self.request_ownership {
            ownership.note_response_observed(&handle.id);
        }
        // An error already observed before cleanup is independent transport
        // evidence. Later local termination must not explain it away.
        match &response {
            Ok(Err(error)) if is_transport_loss(error) => {
                self.note_transport_loss(&error.to_string());
            }
            Err(_) => self.note_transport_loss(
                "the MCP response channel ended without a correlated response",
            ),
            _ => {}
        }
        let mut terminated_local_request = false;
        // rmcp makes a correlated POST response visible before its SSE tail
        // drain drops our HTTP guard. Retain admission and await the exact
        // request's release proof before returning any physical outcome.
        // The response has already won arbitration; cleanup cannot replace it.
        if let Some(admission) = admission.as_ref() {
            tokio::select! {
                biased;
                () = admission.released() => {},
                () = context.cancellation.cancelled(), if ownership.arbitrated_by_cancellation() => {
                    let termination = admission.terminate();
                    terminated_local_request = termination.terminated_local_request();
                    termination.settled().await;
                }
                () = async {
                    match bound {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {
                    let termination = admission.terminate();
                    terminated_local_request = termination.terminated_local_request();
                    termination.settled().await;
                }
            }
        }
        // A correlated response winning the biased arbitration must not
        // silently discard liveness evidence the peer already delivered on
        // the same ordered transport: those notifications genuinely arrived
        // before the response, and they are reported before the terminal
        // result so the durable fact order stays terminal-last.
        if let Some(progress) = progress.as_mut() {
            drain_remote_progress(context, progress);
        }
        drop(progress);
        // Every occurrence this call owned has now been reported, so its
        // progress-consumer ownership ends. The correlated response already
        // completed the remote dimension at the inbound seam, which makes
        // this the request's terminal forget point.
        drop(progress_lease);
        // A response that won the biased arbitration on its own is a pure
        // response-plane outcome. Any transport error was captured before
        // release cleanup, so that cleanup cannot explain away the error as
        // self-inflicted or change the established arbitration facts.
        McpPhysicalOutcome::Observed {
            response: McpResponseOutcome::observed(response),
            terminated_local_request,
            cancellation_won: false,
        }
    }

    /// Settles one cancellation intent that arrived after the effect
    /// frontier (Issue #205).
    ///
    /// # Two planes, and only one of them holds settlement authority
    ///
    /// ```text
    /// remote control plane          local ownership plane
    ///   notifications/cancelled       this request's in-flight HTTP
    ///   best-effort, unacknowledged   request (Streamable HTTP only)
    ///   MAY never complete            terminated and *proven released*
    ///                                 by rustX-owned state alone
    /// ```
    ///
    /// `notifications/cancelled` is the strongest cancellation the MCP
    /// protocol defines, and the protocol defines no acknowledgement of it,
    /// so sending it is never proof that the remote operation stopped. It is
    /// also not allowed to hold local settlement hostage: over a transport
    /// whose client cannot make progress on an outbound send while a
    /// previous request is outstanding, awaiting that send would leave this
    /// branch pending indefinitely and hand the call's only bound to the
    /// generic Issue #204 settlement-control guard. That is architecturally
    /// wrong — the MCP executor's own settlement plane must terminate.
    ///
    /// So the send is *raced*, never awaited as a prerequisite, and the
    /// bound is rustX's own local request ownership: terminating this
    /// request's local half and awaiting its release proof depends on no
    /// remote response, no protocol acknowledgement, and no timer.
    ///
    /// What "terminating the local half" means is read from the request's
    /// **explicit lifecycle phase**, never inferred from an absent record:
    ///
    /// ```text
    /// AwaitingDispatch  the request is on rmcp's peer outbound queue: no
    ///                   local activity has begun, and an outbound
    ///                   participant that is still capable of dispatching it
    ///                   has not reached the seam
    ///                   -> terminate it and await the release proof, which
    ///                      that participant publishes when it arrives and
    ///                      refuses (or which the end of the generation's
    ///                      outbound seam publishes when it never can)
    /// DispatchOwned     the send future owns it and has not handed it to
    ///                   the inner transport
    ///                   -> terminate it and await the release proof
    /// HttpOwned         the POST future or the response body owns it
    ///                   -> terminate it and await the release proof
    /// Released          every local participant is already terminal
    ///                   -> nothing to terminate, and no record is created
    /// ```
    ///
    /// The first row is the Issue #205 review finding stated as a phase.
    /// Treating it as "nothing pending" reported settlement while a live
    /// rmcp send future for the same request id was still going to run — and
    /// that future then recreated the lifecycle entry the settlement had
    /// forgotten and dispatched the call. Settlement now happens-after every
    /// participant that could dispatch has become terminal.
    ///
    /// Over stdio there is no local half at all, so the termination is
    /// already settled and the race collapses to the cancellation send and
    /// the response channel.
    ///
    /// # Arbitration
    ///
    /// A **correlated remote response always wins**: it is proven remote
    /// terminality, and the response channel is deliberately retained across
    /// the whole path so a result that beat the cancellation inside rmcp
    /// still becomes the call's outcome. rmcp resolves this request's local
    /// responder in the same event-loop step in which it reports the
    /// notification's send outcome, so after an observed send the response
    /// channel is checked without waiting; and rustX's own local
    /// termination resolves that responder too, through the ordinary
    /// transport-send failure path. Only when no correlated remote response
    /// can be established is the external outcome unknown. Dropping a future
    /// is never used as evidence of anything.
    ///
    /// Nothing is reported until local ownership is settled: that await is
    /// the last thing this function does before it reports a fact, so
    /// `Unconfirmed` keeps its Issue #204 meaning — every rustX-owned local
    /// activity of this request is over, and only the remote effect remains
    /// uncertain.
    async fn settle_post_frontier_cancellation(
        &self,
        mut handle: rmcp::service::RequestHandle<RoleClient>,
        admission: Option<&streamable_http::McpRequestAdmission>,
    ) -> McpPhysicalOutcome {
        // Armed first and synchronously: from here on this request cannot
        // reach the network even if its POST had not started yet.
        let termination = Self::terminate_local_request(admission);
        let outcome = self
            .arbitrate_post_frontier_cancellation(&mut handle, &termination)
            .await;
        // Test-only: everything the MCP settlement plane can do before the
        // local-ownership proof is now done — the best-effort protocol
        // cancellation has been raced and the response channel arbitrated.
        // A regression uses this to distinguish "settlement is waiting for a
        // local participant" from "settlement had nothing to wait for",
        // without timing the network work above.
        #[cfg(test)]
        test_sync::note_awaiting_local_proof(&handle.id);
        // The local ownership proof. Bounded by rustX-owned state alone, and
        // a precondition of *reporting*, never a competitor to it.
        termination.settled().await;
        match outcome {
            // A correlated remote response beat the cancellation: remote
            // terminality is proven and the remote's own outcome is
            // authoritative. The caller then applies its documented rule to
            // this proven settlement.
            PostFrontierCancellation::Correlated(response) => McpPhysicalOutcome::Observed {
                response,
                terminated_local_request: termination.terminated_local_request(),
                cancellation_won: true,
            },
            // No correlated remote response exists. The request may have
            // executed remotely and its final external outcome cannot be
            // established.
            PostFrontierCancellation::Uncorrelated {
                requested,
                observed,
            } => {
                // A transport-class failure observed here is still genuine
                // evidence that the generation is unusable — unless this
                // call terminated its own transport-level request, which
                // fully explains it.
                if !termination.terminated_local_request() {
                    match &observed {
                        Some(McpResponseOutcome::Answered(answered)) => {
                            if let Err(error) = answered.as_ref()
                                && is_transport_loss(error)
                            {
                                self.note_transport_loss(&error.to_string());
                            }
                        }
                        Some(McpResponseOutcome::ChannelEnded) => {
                            self.note_transport_loss(
                                "the MCP response channel ended without a correlated response",
                            );
                        }
                        _ => {}
                    }
                }
                McpPhysicalOutcome::CancelledPostFrontier {
                    requested,
                    terminated_local_request: termination.terminated_local_request(),
                }
            }
        }
    }

    /// Admits one dispatched request into this generation's request
    /// lifecycle registry.
    ///
    /// Stdio owns no per-request local half, so there is no lifecycle to
    /// open there and the settlement plane has nothing to await. Over
    /// Streamable HTTP the entry created here is what a later cancellation
    /// reads its state from, and the returned admission is what forgets it.
    fn admit_local_request(
        &self,
        id: &rmcp::model::RequestId,
    ) -> Option<streamable_http::McpRequestAdmission> {
        self.request_ownership
            .as_ref()
            .map(|ownership| ownership.admit(id))
    }

    /// Terminates rustX's own local ownership of one dispatched request.
    ///
    /// Stdio has no per-request local ownership to terminate, so it settles
    /// immediately; Streamable HTTP reads the request's explicit lifecycle
    /// state and terminates whatever local owner it actually has.
    fn terminate_local_request(
        admission: Option<&streamable_http::McpRequestAdmission>,
    ) -> streamable_http::LocalRequestTermination {
        admission.map_or(
            streamable_http::LocalRequestTermination::NoLocalOwnership,
            streamable_http::McpRequestAdmission::terminate,
        )
    }

    /// Races the best-effort protocol cancellation against the two facts
    /// that can settle this call locally.
    async fn arbitrate_post_frontier_cancellation(
        &self,
        handle: &mut rmcp::service::RequestHandle<RoleClient>,
        termination: &streamable_http::LocalRequestTermination,
    ) -> PostFrontierCancellation {
        let notification =
            rmcp::model::CancelledNotification::new(rmcp::model::CancelledNotificationParam::new(
                Some(handle.id.clone()),
                Some("rustX execution cancellation".to_owned()),
            ));
        let cancel = self.peer.send_notification(notification.into());
        tokio::pin!(cancel);
        // The non-correlated response-channel fact, when the channel
        // resolved with one. rmcp's synthesized local cancellation outcome
        // and a transport failure both land here: neither proves remote
        // terminality, and neither ends the arbitration.
        let mut observed: Option<McpResponseOutcome> = None;
        let requested = loop {
            tokio::select! {
                biased;
                // Proven remote terminality outranks every local fact.
                response = &mut handle.rx, if observed.is_none() => {
                    let response = McpResponseOutcome::observed(response);
                    if response.is_correlated_remote_response() {
                        return PostFrontierCancellation::Correlated(response);
                    }
                    observed = Some(response);
                }
                outcome = &mut cancel => break match outcome {
                    Ok(()) => RemoteCancellation::Sent,
                    Err(error) => RemoteCancellation::Failed(bound_error(&error.to_string())),
                },
                // The local ownership plane reached its terminal decision
                // first, so the best-effort remote control send is abandoned
                // rather than allowed to hold settlement.
                //
                // The guard is what keeps this honest. This arm exists only
                // when the termination is genuinely *waiting* for a local
                // owner to be dropped — the Streamable HTTP `Live` case the
                // bound is for. Every other termination is already settled
                // when it is created, and racing an already-resolved fact
                // would abandon every cancellation send before it could
                // leave.
                () = termination.settled(), if termination.awaits_release() => {
                    break RemoteCancellation::Abandoned;
                }
            }
        };
        // The correlated response still wins if one exists: after an
        // observed cancellation send rmcp has already resolved this
        // request's responder, and rustX's own local termination resolves it
        // through the transport-send failure path. This is a check, never a
        // wait.
        if observed.is_none() {
            match handle.rx.try_recv() {
                Ok(response) => {
                    let response = McpResponseOutcome::Answered(Box::new(response));
                    if response.is_correlated_remote_response() {
                        return PostFrontierCancellation::Correlated(response);
                    }
                    observed = Some(response);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    observed = Some(McpResponseOutcome::ChannelEnded);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        PostFrontierCancellation::Uncorrelated {
            requested,
            observed,
        }
    }
}

/// How one physical MCP request participates in the ownership rules of the
/// `ToolInvocation` that owns it (Issue #243).
pub(super) struct McpRequestOwnership<'a> {
    catalog_epoch: Option<u64>,
    /// The remote tool whose idle-liveness this request's progress evidences.
    ///
    /// `Some` for a `tools/call` or Resource request: remote progress is
    /// genuine Tool liveness. `None` for every task-control request, because
    /// dispatching another `tasks/get` is rustX's own transport activity and
    /// reporting it as progress would fabricate liveness.
    progress_of: Option<&'a str>,
    /// How local cancellation intent applies to this request.
    cancellation: McpRequestCancellation,
}

/// How local cancellation intent applies to one physical MCP request.
enum McpRequestCancellation {
    /// Cancellation refuses the dispatch before the effect frontier and
    /// arbitrates the response after it. Every request of a live invocation.
    Arbitrated,
    /// The request exists **because** cancellation already won, so
    /// cancellation can neither refuse nor arbitrate it — doing either would
    /// make the cooperative `tasks/cancel` unsendable exactly when it is
    /// needed. It is bounded instead by a fixed rustX-owned wait, after which
    /// its local half is terminated and proven released like any other.
    Exempt {
        /// The rustX-owned bound on awaiting the acknowledgement.
        bound: std::time::Duration,
    },
}

/// The rustX-owned bound on the cooperative `tasks/cancel` acknowledgement.
///
/// It is a bound on a **best-effort remote control**, not a deadline and not
/// settlement evidence: the acknowledgement proves nothing about the remote
/// task either way, so the only thing this value protects is the invocation's
/// own ability to finish reporting.
const MCP_TASK_CANCEL_ACK_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

impl<'a> McpRequestOwnership<'a> {
    /// One `tools/call` round: it owns progress, and cancellation arbitrates.
    pub(super) const fn tool_call(remote_name: &'a str, catalog_epoch: Option<u64>) -> Self {
        Self {
            catalog_epoch,
            progress_of: Some(remote_name),
            cancellation: McpRequestCancellation::Arbitrated,
        }
    }

    /// One model-invoked Resource request, with genuine remote progress.
    pub(super) const fn resource(method: &'a str) -> Self {
        Self::tool_call(method, None)
    }

    /// One catalog or task-control request: locally owned, silent, arbitrated.
    pub(super) const fn control() -> Self {
        Self {
            catalog_epoch: None,
            progress_of: None,
            cancellation: McpRequestCancellation::Arbitrated,
        }
    }

    /// One `tasks/get` or `tasks/update`: locally owned, silent, arbitrated.
    pub(super) const fn task_control() -> Self {
        Self::control()
    }

    /// The one cooperative `tasks/cancel` an invocation may send.
    pub(super) const fn task_cancellation() -> Self {
        Self {
            catalog_epoch: None,
            progress_of: None,
            cancellation: McpRequestCancellation::Exempt {
                bound: MCP_TASK_CANCEL_ACK_BOUND,
            },
        }
    }

    /// Whether observable cancellation refuses this request before dispatch.
    const fn refused_by_cancellation(&self) -> bool {
        matches!(self.cancellation, McpRequestCancellation::Arbitrated)
    }

    /// Whether observable cancellation competes with this request's response.
    const fn arbitrated_by_cancellation(&self) -> bool {
        matches!(self.cancellation, McpRequestCancellation::Arbitrated)
    }

    /// The rustX-owned bound on awaiting this request's response, when it has
    /// one instead of the invocation's cancellation/deadline contract.
    const fn response_bound(&self) -> Option<std::time::Duration> {
        match self.cancellation {
            McpRequestCancellation::Arbitrated => None,
            McpRequestCancellation::Exempt { bound } => Some(bound),
        }
    }
}
