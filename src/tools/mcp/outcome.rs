//! The physical-request outcome vocabulary.
//!
//! These are the facts `dispatch_owned_request` reports and its callers
//! interpret. None of them is settlement evidence beyond what it names: a
//! correlated remote response is the only proof of remote terminality, and
//! an accepted cancellation is never proof of physical termination.

use rmcp::model::ServerResult;

use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

use super::bound_error;

/// What became of **one** physical MCP request, in facts rather than in one
/// caller's result vocabulary (Issue #243).
///
/// Every variant reports a state in which no rustX-owned local activity of
/// that request remains: a refusal never started one, and both settled
/// variants awaited the local release proof before they were built.
pub(super) enum McpPhysicalOutcome {
    /// Local cancellation intent was already observable, so the request was
    /// never dispatched and no remote side effect was possible.
    RefusedByCancellation {
        /// The cancellation this invocation was asked to honour.
        reason: crate::runtime::types::CancellationReason,
    },
    /// The request never reached the transport for a protocol or runtime
    /// reason. `result` is the settled outcome for a caller with no further
    /// context; `detail` is the same fact as a bounded diagnostic, for a
    /// caller whose own certainty rules differ.
    Refused {
        /// The settled result of the invocation.
        result: Box<ToolExecutionResult>,
        /// The bounded diagnostic behind it.
        detail: String,
    },
    /// The request crossed the effect frontier and its response channel
    /// resolved, or proved that it cannot.
    Observed {
        /// What the response channel produced.
        response: McpResponseOutcome,
        /// Whether *this request* terminated its own transport-level half.
        terminated_local_request: bool,
        /// Whether local cancellation intent won before the response did.
        /// A correlated remote response still outranks it.
        cancellation_won: bool,
    },
    /// Local cancellation intent won past the effect frontier with no
    /// correlated remote response, or the request's own rustX-owned bound
    /// expired. The remote outcome cannot be established.
    CancelledPostFrontier {
        /// What became of the best-effort protocol cancellation.
        requested: RemoteCancellation,
        /// Whether *this request* terminated its own transport-level half.
        #[allow(dead_code, reason = "reported for symmetry with `Observed`")]
        terminated_local_request: bool,
    },
}

/// One post-frontier observation of a dispatched request's response channel.
///
/// The distinction is the whole post-frontier contract: `Answered` is a
/// correlated remote response — the only proof of remote terminality rustX
/// accepts — while `ChannelEnded` is the absence of one.
pub(super) enum McpResponseOutcome {
    /// The peer answered this request id, with a result or a JSON-RPC error,
    /// or rmcp reported a service-level failure for it.
    ///
    /// Boxed because `ServerResult` is a large union of every MCP result
    /// shape, and this value travels through one `select!` state machine.
    Answered(Box<Result<ServerResult, rmcp::service::ServiceError>>),
    /// The response channel ended without a correlated response.
    ChannelEnded,
}

impl McpResponseOutcome {
    /// Whether this observation is a **correlated remote response**: a
    /// result, or a JSON-RPC error, that the peer produced for this request
    /// id. It is the only proof of remote terminality rustX accepts.
    ///
    /// rmcp's synthesized `ServiceError::Cancelled` is deliberately *not*
    /// one: it is a local outcome rmcp writes into the request's responder
    /// when it reports a cancellation notification's send outcome, and it
    /// says nothing about what the remote did.
    pub(super) const fn is_correlated_remote_response(&self) -> bool {
        matches!(
            self,
            Self::Answered(answered)
                if matches!(
                    **answered,
                    Ok(_) | Err(rmcp::service::ServiceError::McpError(_))
                )
        )
    }

    /// The observation of one resolved response channel.
    pub(super) fn observed(
        response: Result<
            Result<ServerResult, rmcp::service::ServiceError>,
            tokio::sync::oneshot::error::RecvError,
        >,
    ) -> Self {
        match response {
            Ok(answered) => Self::Answered(Box::new(answered)),
            Err(_) => Self::ChannelEnded,
        }
    }
}

/// How one post-frontier cancellation arbitration ended.
pub(super) enum PostFrontierCancellation {
    /// A correlated remote response was observed, or the response channel
    /// proved none can arrive. Either way the response plane, not the
    /// cancellation plane, decides this call.
    Correlated(McpResponseOutcome),
    /// No correlated remote response exists. `requested` carries what became
    /// of the best-effort protocol cancellation, and `observed` the
    /// non-correlated response-channel fact when there was one.
    Uncorrelated {
        requested: RemoteCancellation,
        observed: Option<McpResponseOutcome>,
    },
}

/// What became of one call's best-effort `notifications/cancelled`.
///
/// None of these three is settlement evidence: remote cancellation is
/// unacknowledged by the protocol, so even `Sent` proves only that rustX
/// asked. They exist to make the call's diagnostic honest about *which*
/// remote control was actually attempted.
pub(super) enum RemoteCancellation {
    /// The notification left rustX.
    Sent,
    /// The send itself failed.
    Failed(String),
    /// Local request ownership settled first, so the send was abandoned
    /// rather than allowed to hold local settlement authority.
    Abandoned,
}

/// Whether one rmcp service failure is transport-class evidence that the
/// connection generation itself can no longer carry MCP traffic
/// (Issue #205).
///
/// A remote error *result* proves the opposite — the transport delivered a
/// correlated answer — and never appears here.
pub(super) fn is_transport_loss(error: &rmcp::service::ServiceError) -> bool {
    matches!(
        error,
        rmcp::service::ServiceError::TransportClosed
            | rmcp::service::ServiceError::TransportSend(_)
    )
}

/// The terminal status of a post-dispatch cancellation attempt.
///
/// The request was already dispatched, so the call crossed the
/// external-effect frontier. An accepted cancellation notification proves
/// only that the request reached the peer, never that the remote operation
/// terminated; a failed cancellation request proves even less. Either way
/// the final external outcome is unknown — never `Cancelled`, never
/// `Failed`.
pub(super) fn post_dispatch_cancellation_status(
    cancelled: RemoteCancellation,
    generation_tag: &str,
) -> ToolExecutionStatus {
    let detail = match cancelled {
        RemoteCancellation::Sent => format!(
            "cancellation was requested after dispatch, but remote termination could not be \
             confirmed ({generation_tag})"
        ),
        RemoteCancellation::Failed(error) => format!(
            "cancellation was requested after dispatch and the cancellation request itself \
             failed ({generation_tag}): {error}"
        ),
        RemoteCancellation::Abandoned => format!(
            "cancellation was requested after dispatch; this call's own local request \
             ownership settled before the best-effort remote cancellation left rustX, so \
             remote termination could not be confirmed ({generation_tag})"
        ),
    };
    ToolExecutionStatus::OutcomeUnknown {
        detail: bound_error(&detail),
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoteCancellation, post_dispatch_cancellation_status};
    use crate::tools::types::ToolExecutionStatus;

    /// Issue #202: the post-dispatch cancellation mapping is deterministic
    /// for both cancellation-request outcomes — an accepted request and a
    /// failed request both settle as `OutcomeUnknown` with a bounded detail,
    /// because neither proves the remote operation terminated.
    #[test]
    fn post_dispatch_cancellation_is_outcome_unknown_whether_the_request_succeeded_or_failed() {
        let tag = "MCP server 'fixture' connection generation 2";
        let accepted = post_dispatch_cancellation_status(RemoteCancellation::Sent, tag);
        let ToolExecutionStatus::OutcomeUnknown { detail } = &accepted else {
            panic!("an accepted cancel request is OutcomeUnknown: {accepted:?}");
        };
        assert!(
            detail.contains("remote termination could not be confirmed"),
            "the accepted-request detail names the unproven termination: {detail}"
        );
        assert!(
            detail.contains(tag),
            "the detail correlates the ambiguity with its connection generation: {detail}"
        );

        let huge_error = "x".repeat(64 * 1024);
        let failed = post_dispatch_cancellation_status(RemoteCancellation::Failed(huge_error), tag);
        let ToolExecutionStatus::OutcomeUnknown { detail } = &failed else {
            panic!("a failed cancel request is OutcomeUnknown: {failed:?}");
        };
        assert!(
            detail.contains("the cancellation request itself failed"),
            "the failed-request detail names the failed request: {detail}"
        );
        assert!(
            detail.len() <= 1024 + 128,
            "the detail stays bounded far below the 64KiB input: {} bytes",
            detail.len()
        );
    }
}
