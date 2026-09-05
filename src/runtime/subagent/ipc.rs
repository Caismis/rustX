//! The bounded parent/child control transport of the subagent plane
//! (Issue #60).
//!
//! IPC transports bounded envelopes and control; it is **not** the
//! conversation message bus. No frame ever appends to a canonical history,
//! allocates a destination `InboundSequence`, or schedules an attempt: the
//! receiving side routes any model-visible payload through its own ordinary
//! durable inbound path.
//!
//! # Wire format
//!
//! One stream direction carries length-prefixed frames:
//!
//! ```text
//! [u32 LE length][u8 kind][payload]
//! ```
//!
//! where `length` covers `kind + payload`, the payload is one JSON document,
//! and every frame is bounded by [`MAX_FRAME_BYTES`]. The protocol is
//! versioned by the `Hello` handshake: a child that does not speak exactly
//! [`SUBAGENT_IPC_VERSION`] exits before composing anything, and a parent
//! rejects every malformed, oversized, unknown, or out-of-order frame as a
//! protocol failure of the child — never as semantic evidence.
//!
//! # Transports
//!
//! The child inherits **two** `UnixStream` pair endpoints with independent
//! backpressure domains (Issue #178):
//!
//! - fd 0 (standard input): the **reliable control channel**, duplex. It
//!   carries the `Hello` handshake, delegation/cancellation, routed
//!   interaction requests and responses, anchor acknowledgements, and every
//!   child-bound lifecycle/ownership frame — ordered and non-lossy. The
//!   parent's endpoint closes when the parent process dies — for any reason,
//!   including `SIGKILL` — so this channel is the parent-liveness authority:
//!   a child that observes EOF before its terminal settlement drains and exits.
//! - fd 1 (standard output): the **disposable observation channel**,
//!   child-to-parent only. It carries `Activity` frames with latest-value
//!   semantics. A stalled or lost observation channel delays nothing on the
//!   control channel, and its EOF is never lifecycle evidence.
//!
//! No socket path, no listener, no network endpoint, and no PID polling is
//! involved.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::context::{AgentStatusConfig, SessionContextPolicy};
use crate::model::deadline::ModelTimeoutPolicy;
use crate::runtime::identity::{AgentId, ConversationId, ProcessUnitId, SubagentId};
use crate::runtime::types::{ApprovalMode, CancellationReason};

use super::resolver::ResolvedSubagentSpec;
use super::workspace::WorkspaceSnapshot;

/// The only subagent control protocol version this build speaks.
///
/// Version 2 replaced the profile/persona-shaped startup identity with the
/// frozen named-agent semantic specification (Issue #144). Version 3 added
/// the nested process-unit anchor handshake and the frozen external
/// materialization plane (Issue #145). Version 4 carries the launch-scoped
/// frozen `ModelTimeoutPolicy` and version 5 carries the parent registry's
/// typed cancellation provenance (Issue #138). Version 6 carries the
/// resolved workspace authority and immutable Git snapshot facts (Issue
/// #146). Version 7 carries the invoking attempt's effective `ApprovalMode`
/// and the reserved Workflow Agent terminal protocol (Issue #83). Version 8
/// carried the child→parent live activity projection frames (Issue #178);
/// version 9 moves them onto the dedicated disposable observation channel
/// (fd 1), so observation backpressure can never occupy the reliable
/// control transport. Version 10 adds bidirectional routed interaction
/// control frames and root-provider availability state; version 11 adds the
/// root publication-admission handshake. Version 12 separates the child's
/// logical project workspace from the physical Git worktree root owned by
/// the parent. Version 13 carries the definition-level whole-lifecycle
/// execution deadline inside the frozen resolved launch specification (Issue
/// #191). Version 14 carries the parent's frozen generic tool
/// execution-liveness deadline policy (Issue #204), inherited unchanged by
/// the child runtime's foreground Tool lifecycle. HITL traffic remains on
/// fd 0 and never uses the disposable activity lane.
/// There is no compatibility decoding: a peer that does not speak exactly
/// this version exits before composing anything.
pub(crate) const SUBAGENT_IPC_VERSION: u16 = 14;

/// The hard upper bound of one control frame (`kind + payload`).
///
/// Every payload is a small typed envelope; the largest legal payload is a
/// delegated task plus its explicit context package or a bounded result,
/// each far below this bound. A peer that exceeds it is terminated as a
/// protocol failure.
pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;

// Parent -> child frame kinds.
const KIND_HELLO: u8 = 1;
const KIND_DELEGATE: u8 = 2;
const KIND_CANCEL: u8 = 3;
const KIND_ANCHOR_ACCEPTED: u8 = 4;
const KIND_ANCHOR_REFUSED: u8 = 5;
const KIND_INTERACTION_RESPOND: u8 = 6;
const KIND_PROVIDER_AVAILABILITY: u8 = 7;
const KIND_INTERACTION_ADMISSION_RESULT: u8 = 8;

// Child -> parent frame kinds (reliable control channel, fd 0).
const KIND_READY: u8 = 101;
const KIND_STARTUP_ERROR: u8 = 102;
const KIND_RESULT: u8 = 103;
const KIND_DIAGNOSTIC: u8 = 104;
const KIND_ANCHOR_OFFERED: u8 = 105;
const KIND_ANCHOR_RELEASED: u8 = 106;
const KIND_INTERACTION_REQUESTED: u8 = 108;
const KIND_INTERACTION_SETTLED: u8 = 109;
const KIND_INTERACTION_RESPONSE_RESULT: u8 = 110;
const KIND_INTERACTION_ADMISSION_REQUESTED: u8 = 111;

// Observation channel frame kind (disposable, fd 1, child -> parent only).
const KIND_ACTIVITY: u8 = 107;

/// The typed startup specification of one subagent child, carried by the
/// `Hello` frame.
///
/// This is the one typed composition boundary between the parent and the
/// child runtime: the child composes its headless `ConversationRuntime`
/// from exactly this typed input and nothing else. No configuration file of
/// any kind is opened by the child, and no temporary runtime configuration
/// file is ever written.
///
/// # The parent resolves; the child consumes
///
/// [`SubagentChildSpec::resolved`] is the complete frozen result of
/// parent-side resolution against the invoking attempt's runtime resource
/// generation: the named-agent identity and its definition digest, the
/// optional whole-lifecycle execution deadline, the child instruction
/// document, the completely resolved model invocation, the exact
/// source-qualified capability identities with their exact admitted
/// `ToolDefinition`s, the exact Skill version identities with their
/// model-visible metadata, and the exact project instruction chain. The
/// child therefore never reads `rustx.jsonc` to look up the agent, never
/// reopens `models.jsonc` to re-resolve a model, never rediscovers project
/// instructions or Skills, and never widens or substitutes Tool identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SubagentChildSpec {
    /// The control protocol version; must equal [`SUBAGENT_IPC_VERSION`].
    pub protocol_version: u16,
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The child agent identity (provenance of its answer).
    pub child_agent_id: AgentId,
    /// The delegating parent agent identity (provenance of the task).
    pub parent_agent_id: AgentId,
    /// The complete frozen named-agent specification resolved by the parent
    /// against the invoking attempt's runtime resource generation.
    pub resolved: ResolvedSubagentSpec,
    /// The effective approval mode frozen by the invoking Agent attempt.
    /// This can bypass approval only for the exact Tools in resolved and
    /// never changes the child's capability or execution authority.
    pub approval_mode: ApprovalMode,
    /// The parent runtime's frozen model timeout policy, inherited by the
    /// child unchanged (Issue #138): the child applies it to its own
    /// response-start deadlines, stream-idle deadlines, and model-backed
    /// summarization. The parent never enforces child provider deadlines
    /// itself.
    pub model_timeout_policy: ModelTimeoutPolicy,
    /// The parent runtime's frozen generic tool execution-liveness policy,
    /// inherited by the child unchanged (Issue #204): the child applies it
    /// to the hard/idle deadlines of its own foreground Tool executions.
    pub tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
    /// The launch-scoped Agent Status configuration of the child.
    pub agent_status: AgentStatusConfig,
    /// The session context policy of the child.
    pub context: SessionContextPolicy,
    /// The authoritative logical project workspace and runtime-owned
    /// isolation facts selected by the parent. In worktree mode the nested
    /// facts separately carry the physical checkout root, repository-relative
    /// scope, exact committed base, and runtime-created ref.
    pub workspace_snapshot: WorkspaceSnapshot,
    /// The exact spawn-incarnation-private mutable runtime root (artifacts,
    /// diagnostics, Skills, and private Python state). It is never the stable
    /// semantic `SubagentId` grouping path.
    pub runtime_root: PathBuf,
    /// The child terminal protocol. Workflow-owned children receive a
    /// frozen `workflow_output` schema; ordinary named subagents use the
    /// normal parent-inbound answer protocol.
    pub terminal: ChildTerminalMode,
}

/// The child-side terminal protocol selected by the parent registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ChildTerminalMode {
    /// Publish an ordinary named-subagent answer candidate.
    Normal,
    /// Require the reserved Workflow Agent output protocol.
    WorkflowOutput {
        /// The frozen JSON Schema shown to the child model.
        output_schema: serde_json::Value,
    },
}

/// The delegated task envelope (`Delegate` frame payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DelegationFrame {
    /// The delegated task.
    pub task: String,
    /// The explicit bounded context package, when the delegating call
    /// supplied one.
    pub context: Option<String>,
    /// Whether a capable root Runtime Client human surface existed when the
    /// child was admitted. This is provider state, not `ask_user` capability
    /// selection and not a settlement decision.
    pub interaction_provider_available: bool,
}

/// The result of one parent-routed interaction response.
///
/// `response_id` is transport correlation only; the semantic target remains
/// the full `InteractionRef` and is validated by the originating coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InteractionResponseResultFrame {
    /// Transport-only response correlation identity.
    pub response_id: u64,
    /// The routed semantic address echoed by the child.
    pub interaction: crate::runtime::interaction::InteractionRef,
    /// The originating coordinator's accepted or fail-closed result.
    pub result: Result<(), crate::runtime::interaction::RoutedInteractionError>,
}

/// The child's `Ready` frame: composition and activation completed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadyFrame {
    /// The child echoes its assigned identity; a mismatch is a protocol
    /// failure.
    pub subagent_id: SubagentId,
}

/// A bounded diagnostic note from the child; never semantic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiagnosticFrame {
    /// The bounded diagnostic text.
    pub message: String,
}

/// The terminal semantic status the child reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildResultStatus {
    /// The child attempt completed; `content` carries the bounded final
    /// answer.
    Succeeded,
    /// The child attempt failed; `diagnostic` carries the bounded failure.
    Failed,
    /// The child observed cancellation and drained.
    Cancelled,
}

/// The child's one terminal semantic result envelope.
///
/// It is a **candidate**, never a terminal fact: the parent-side
/// settlement owner validates it, awaits process exit and reap, and only
/// then drives the parent's durable result acceptance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResultFrame {
    /// The terminal semantic status.
    pub status: ChildResultStatus,
    /// The bounded final answer (succeeded only).
    pub content: Option<String>,
    /// The bounded failure diagnostic (failed only).
    pub diagnostic: Option<String>,
}

/// One nested supervised process unit's containment anchor (Issue #145).
///
/// `pgid` is the numeric identity of the unit's own `setsid()` group — the
/// group the child's local supervisor would otherwise be the only owner of.
/// The parent retains exactly this pair; it never scans, guesses, or
/// correlates by ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessUnitAnchorFrame {
    /// The typed identity of the offering supervised unit.
    pub unit_id: ProcessUnitId,
    /// The unit's containment process-group id.
    pub pgid: i32,
}

/// The parent's acknowledgement of one retained anchor (Issue #145).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessUnitAckFrame {
    /// The exact unit acknowledged. Correlation is by identity only.
    pub unit_id: ProcessUnitId,
}

/// The parent's refusal to retain one offered anchor (Issue #145).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessUnitRefusalFrame {
    /// The exact unit refused.
    pub unit_id: ProcessUnitId,
    /// The bounded refusal reason.
    pub reason: String,
}

/// One live activity projection update from the child (Issue #178).
///
/// This is observation-plane traffic only: it carries the child's newest
/// [`SubagentObservation`] with latest-value coalescing semantics, is never
/// durable, never semantic evidence, and never blocks the child's execution.
/// It travels on the **dedicated disposable observation channel** (fd 1),
/// never on the reliable control channel: the child publishes it through a
/// latest-value slot drained by an independent observation writer, so the
/// parent may receive revision `n` without ever having seen any earlier
/// revision, and a stalled observation transport delays no control frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActivityFrame {
    /// The child's latest live activity projection.
    pub observation: super::activity::SubagentObservation,
}

/// One child publication-admission request/result. `request_id` is
/// transport correlation only; the exact `InteractionRef` is echoed so a
/// stale or mismatched result can never authorize another interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InteractionPublicationAdmissionFrame {
    /// Transport-only admission correlation identity.
    pub request_id: u64,
    /// The exact interaction whose publication is asking for admission.
    pub interaction: crate::runtime::interaction::InteractionRef,
    /// Whether the root human-facing provider admitted this publication.
    pub admitted: bool,
}

/// One decoded parent-bound frame of the reliable control channel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChildFrame {
    /// Composition and activation completed.
    Ready(ReadyFrame),
    /// Composition failed before any semantic work began.
    StartupError(DiagnosticFrame),
    /// The terminal semantic result candidate.
    Result(ResultFrame),
    /// A bounded diagnostic note.
    Diagnostic(DiagnosticFrame),
    /// A nested supervised unit offers its containment anchor and blocks on
    /// the acknowledgement.
    AnchorOffered(ProcessUnitAnchorFrame),
    /// A nested supervised unit is proven physically terminal, so the
    /// parent may drop exactly that retained anchor.
    AnchorReleased(ProcessUnitAnchorFrame),
    /// A child-owned interaction request committed by its coordinator.
    InteractionRequested(crate::runtime::interaction::InteractionRequest),
    /// A child-owned interaction terminal transition.
    InteractionSettled {
        /// The originating routed identity.
        interaction: crate::runtime::interaction::InteractionRef,
        /// The originating coordinator's terminal outcome.
        outcome: crate::runtime::interaction::InteractionOutcome,
    },
    /// The result of one parent-routed response.
    InteractionResponseResult(InteractionResponseResultFrame),
    /// A child asks the root provider authority to admit one exact
    /// interaction publication before the child commits `InteractionRequested`.
    InteractionPublicationAdmissionRequested(InteractionPublicationAdmissionFrame),
}

/// One decoded child-bound frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParentFrame {
    /// The startup specification (exactly once, first).
    Hello(Box<SubagentChildSpec>),
    /// The delegated task (exactly once, after `Ready`).
    Delegate(DelegationFrame),
    /// Cancellation/shutdown request. A `Some` cause is the semantic reason
    /// already committed by the parent registry; the process driver
    /// transports it but never chooses it. `None` is used only by the
    /// pre-ownership preparation-cancellation path, where no child attempt
    /// exists and therefore no attempt-scoped semantic reason is available.
    Cancel {
        /// The parent registry's first-winner cancellation cause, when this
        /// is a committed child cancellation.
        reason: Option<CancellationReason>,
    },
    /// The parent retains the named unit's anchor; its local `START` gate
    /// may now open.
    AnchorAccepted(ProcessUnitAckFrame),
    /// The parent will not retain the named unit's anchor; it must never
    /// start.
    AnchorRefused(ProcessUnitRefusalFrame),
    /// A root Runtime Client response addressed to an originating child.
    InteractionRespond {
        /// Transport-only response correlation identity.
        response_id: u64,
        /// The full semantic route target.
        interaction: crate::runtime::interaction::InteractionRef,
        /// The typed response; the child coordinator validates it.
        response: crate::runtime::interaction::InteractionResponse,
    },
    /// Early root Runtime Client human-provider availability hint for future
    /// child publications. The admission result is authoritative, and
    /// existing pending interactions are unaffected.
    InteractionProviderAvailable {
        /// Whether a capable root Runtime Client control attachment exists.
        available: bool,
    },
    /// The root provider authority's answer to one child publication request.
    InteractionPublicationAdmissionResult(InteractionPublicationAdmissionFrame),
}

/// A control-protocol violation of the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    /// A frame exceeded [`MAX_FRAME_BYTES`].
    OversizedFrame {
        /// The advertised frame length.
        length: usize,
    },
    /// An unknown frame kind.
    UnknownKind {
        /// The offending kind byte.
        kind: u8,
    },
    /// A frame payload did not decode as its typed envelope.
    Malformed {
        /// The decode failure detail.
        detail: String,
    },
    /// The transport closed or failed mid-protocol.
    Transport {
        /// The I/O failure detail.
        detail: String,
    },
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OversizedFrame { length } => {
                write!(f, "control frame of {length} bytes exceeds the bound")
            }
            Self::UnknownKind { kind } => write!(f, "unknown control frame kind {kind}"),
            Self::Malformed { detail } => write!(f, "malformed control frame: {detail}"),
            Self::Transport { detail } => write!(f, "control transport failed: {detail}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Writes one bounded frame.
///
/// The transport is generic over the sink so the control `UnixStream` can
/// be split into a read half and a write half owned by the single child
/// control dispatcher (Issue #145), and so the observation channel — a
/// separate `UnixStream` with its own independent backpressure domain
/// (Issue #178) — reuses exactly the same framing.
pub(crate) async fn write_frame<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut W,
    kind: u8,
    payload: &[u8],
) -> Result<(), ProtocolError> {
    let frame_len = u32::try_from(1 + payload.len())
        .map_err(|_| ProtocolError::OversizedFrame { length: usize::MAX })?;
    if 1 + payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame {
            length: 1 + payload.len(),
        });
    }
    let mut frame = Vec::with_capacity(4 + frame_len as usize);
    frame.extend_from_slice(&frame_len.to_le_bytes());
    frame.push(kind);
    frame.extend_from_slice(payload);
    stream
        .write_all(&frame)
        .await
        .map_err(|error| ProtocolError::Transport {
            detail: error.to_string(),
        })?;
    stream
        .flush()
        .await
        .map_err(|error| ProtocolError::Transport {
            detail: error.to_string(),
        })?;
    Ok(())
}

/// Reads one bounded frame; `Ok(None)` is a clean EOF at a frame boundary.
pub(crate) async fn read_frame<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut R,
) -> Result<Option<(u8, Vec<u8>)>, ProtocolError> {
    let mut length_buf = [0u8; 4];
    match stream.read_exact(&mut length_buf).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(ProtocolError::Transport {
                detail: error.to_string(),
            });
        }
    }
    let length = u32::from_le_bytes(length_buf) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(ProtocolError::OversizedFrame { length });
    }
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|error| ProtocolError::Transport {
            detail: error.to_string(),
        })?;
    Ok(Some((payload[0], payload.split_off(1))))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    serde_json::to_vec(value).map_err(|error| ProtocolError::Malformed {
        detail: error.to_string(),
    })
}

fn decode<'a, T: Deserialize<'a>>(payload: &'a [u8]) -> Result<T, ProtocolError> {
    serde_json::from_slice(payload).map_err(|error| ProtocolError::Malformed {
        detail: error.to_string(),
    })
}

/// Writes one typed parent-bound frame of the reliable control channel.
pub(crate) async fn write_child_frame<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut W,
    frame: &ChildFrame,
) -> Result<(), ProtocolError> {
    match frame {
        ChildFrame::Ready(payload) => write_frame(stream, KIND_READY, &encode(payload)?).await,
        ChildFrame::StartupError(payload) => {
            write_frame(stream, KIND_STARTUP_ERROR, &encode(payload)?).await
        }
        ChildFrame::Result(payload) => write_frame(stream, KIND_RESULT, &encode(payload)?).await,
        ChildFrame::Diagnostic(payload) => {
            write_frame(stream, KIND_DIAGNOSTIC, &encode(payload)?).await
        }
        ChildFrame::AnchorOffered(payload) => {
            write_frame(stream, KIND_ANCHOR_OFFERED, &encode(payload)?).await
        }
        ChildFrame::AnchorReleased(payload) => {
            write_frame(stream, KIND_ANCHOR_RELEASED, &encode(payload)?).await
        }
        ChildFrame::InteractionRequested(request) => {
            write_frame(stream, KIND_INTERACTION_REQUESTED, &encode(request)?).await
        }
        ChildFrame::InteractionSettled {
            interaction,
            outcome,
        } => {
            write_frame(
                stream,
                KIND_INTERACTION_SETTLED,
                &encode(&(interaction, outcome))?,
            )
            .await
        }
        ChildFrame::InteractionResponseResult(result) => {
            write_frame(stream, KIND_INTERACTION_RESPONSE_RESULT, &encode(result)?).await
        }
        ChildFrame::InteractionPublicationAdmissionRequested(request) => {
            write_frame(
                stream,
                KIND_INTERACTION_ADMISSION_REQUESTED,
                &encode(request)?,
            )
            .await
        }
    }
}

/// Reads and decodes one typed parent-bound frame of the reliable control
/// channel. An observation-channel kind arriving here is an unknown-kind
/// protocol failure: the two channels never mix traffic.
pub(crate) async fn read_child_frame<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut R,
) -> Result<Option<ChildFrame>, ProtocolError> {
    let Some((kind, payload)) = read_frame(stream).await? else {
        return Ok(None);
    };
    let frame = match kind {
        KIND_READY => ChildFrame::Ready(decode(&payload)?),
        KIND_STARTUP_ERROR => ChildFrame::StartupError(decode(&payload)?),
        KIND_RESULT => ChildFrame::Result(decode(&payload)?),
        KIND_DIAGNOSTIC => ChildFrame::Diagnostic(decode(&payload)?),
        KIND_ANCHOR_OFFERED => ChildFrame::AnchorOffered(decode(&payload)?),
        KIND_ANCHOR_RELEASED => ChildFrame::AnchorReleased(decode(&payload)?),
        KIND_INTERACTION_REQUESTED => ChildFrame::InteractionRequested(decode(&payload)?),
        KIND_INTERACTION_SETTLED => {
            let (interaction, outcome): (
                crate::runtime::interaction::InteractionRef,
                crate::runtime::interaction::InteractionOutcome,
            ) = decode(&payload)?;
            ChildFrame::InteractionSettled {
                interaction,
                outcome,
            }
        }
        KIND_INTERACTION_RESPONSE_RESULT => {
            ChildFrame::InteractionResponseResult(decode(&payload)?)
        }
        KIND_INTERACTION_ADMISSION_REQUESTED => {
            ChildFrame::InteractionPublicationAdmissionRequested(decode(&payload)?)
        }
        other => return Err(ProtocolError::UnknownKind { kind: other }),
    };
    Ok(Some(frame))
}

/// Writes one disposable activity frame to the observation channel.
pub(crate) async fn write_activity_frame<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut W,
    frame: &ActivityFrame,
) -> Result<(), ProtocolError> {
    write_frame(stream, KIND_ACTIVITY, &encode(frame)?).await
}

/// Reads and decodes one disposable activity frame of the observation
/// channel; `Ok(None)` is a clean EOF at a frame boundary. Anything but an
/// activity frame is an unknown-kind protocol failure of the channel.
pub(crate) async fn read_activity_frame<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut R,
) -> Result<Option<ActivityFrame>, ProtocolError> {
    let Some((kind, payload)) = read_frame(stream).await? else {
        return Ok(None);
    };
    if kind != KIND_ACTIVITY {
        return Err(ProtocolError::UnknownKind { kind });
    }
    Ok(Some(decode(&payload)?))
}

/// Writes one typed child-bound frame.
pub(crate) async fn write_parent_frame<W: tokio::io::AsyncWrite + Unpin + ?Sized>(
    stream: &mut W,
    frame: &ParentFrame,
) -> Result<(), ProtocolError> {
    match frame {
        ParentFrame::Hello(payload) => write_frame(stream, KIND_HELLO, &encode(payload)?).await,
        ParentFrame::Delegate(payload) => {
            write_frame(stream, KIND_DELEGATE, &encode(payload)?).await
        }
        ParentFrame::Cancel { reason } => write_frame(stream, KIND_CANCEL, &encode(reason)?).await,
        ParentFrame::AnchorAccepted(payload) => {
            write_frame(stream, KIND_ANCHOR_ACCEPTED, &encode(payload)?).await
        }
        ParentFrame::AnchorRefused(payload) => {
            write_frame(stream, KIND_ANCHOR_REFUSED, &encode(payload)?).await
        }
        ParentFrame::InteractionRespond {
            response_id,
            interaction,
            response,
        } => {
            write_frame(
                stream,
                KIND_INTERACTION_RESPOND,
                &encode(&(response_id, interaction, response))?,
            )
            .await
        }
        ParentFrame::InteractionProviderAvailable { available } => {
            write_frame(stream, KIND_PROVIDER_AVAILABILITY, &encode(available)?).await
        }
        ParentFrame::InteractionPublicationAdmissionResult(result) => {
            write_frame(stream, KIND_INTERACTION_ADMISSION_RESULT, &encode(result)?).await
        }
    }
}

/// Reads and decodes one typed child-bound frame.
pub(crate) async fn read_parent_frame<R: tokio::io::AsyncRead + Unpin + ?Sized>(
    stream: &mut R,
) -> Result<Option<ParentFrame>, ProtocolError> {
    let Some((kind, payload)) = read_frame(stream).await? else {
        return Ok(None);
    };
    let frame = match kind {
        KIND_HELLO => ParentFrame::Hello(Box::new(decode(&payload)?)),
        KIND_DELEGATE => ParentFrame::Delegate(decode(&payload)?),
        KIND_CANCEL => ParentFrame::Cancel {
            reason: decode(&payload)?,
        },
        KIND_ANCHOR_ACCEPTED => ParentFrame::AnchorAccepted(decode(&payload)?),
        KIND_ANCHOR_REFUSED => ParentFrame::AnchorRefused(decode(&payload)?),
        KIND_INTERACTION_RESPOND => {
            let (response_id, interaction, response): (
                u64,
                crate::runtime::interaction::InteractionRef,
                crate::runtime::interaction::InteractionResponse,
            ) = decode(&payload)?;
            ParentFrame::InteractionRespond {
                response_id,
                interaction,
                response,
            }
        }
        KIND_PROVIDER_AVAILABILITY => ParentFrame::InteractionProviderAvailable {
            available: decode(&payload)?,
        },
        KIND_INTERACTION_ADMISSION_RESULT => {
            ParentFrame::InteractionPublicationAdmissionResult(decode(&payload)?)
        }
        other => return Err(ProtocolError::UnknownKind { kind: other }),
    };
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{InteractionId, McpServerId, ToolId};
    use crate::runtime::interaction::{
        InteractionKind, InteractionOutcome, InteractionRequest, InteractionResponse,
        QuestionnaireResponse,
    };
    use crate::runtime::subagent::catalog::SubagentName;
    use crate::runtime::subagent::resolver::ResolvedSubagentTool;
    use crate::tools::types::{
        ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
        ToolReplayPolicy,
    };

    fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        tokio::net::UnixStream::pair().expect("control socket pair")
    }

    fn tool_definition(name: &str, origin: ToolOrigin) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new(format!("tool-{name}")),
            name: name.to_owned(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin,
        }
    }

    /// A frozen spec exercising both capability origins, so the wire
    /// contract is proven to preserve exact source identity. Managed Python
    /// packages ride the MCP origin under their synthesized server identity
    /// (Issue #174).
    fn resolved_spec() -> ResolvedSubagentSpec {
        ResolvedSubagentSpec {
            agent: SubagentName::parse("explore").expect("name"),
            definition_digest: serde_json::from_value(serde_json::json!("sha256:abc"))
                .expect("digest"),
            execution_deadline: None,
            workspace_policy: crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
            instructions: "instructions".to_owned(),
            model: crate::model::frozen::test_frozen_model_spec(
                serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
            ),
            tools: vec![
                ResolvedSubagentTool::Builtin {
                    tool_id: ToolId::new("tool-read"),
                    name: "read".to_owned(),
                    definition: tool_definition("read", ToolOrigin::Builtin),
                },
                ResolvedSubagentTool::Mcp {
                    server_id: McpServerId::new("github"),
                    tool_id: ToolId::new("tool-get_issue"),
                    name: "get_issue".to_owned(),
                    identity: crate::tools::mcp::identity::definition_identity(&tool_definition(
                        "get_issue",
                        ToolOrigin::Mcp {
                            server_id: McpServerId::new("github"),
                        },
                    ))
                    .expect("an MCP definition has an MCP identity"),
                    definition: tool_definition(
                        "get_issue",
                        ToolOrigin::Mcp {
                            server_id: McpServerId::new("github"),
                        },
                    ),
                },
                ResolvedSubagentTool::Mcp {
                    server_id: McpServerId::new("python:symbols"),
                    tool_id: ToolId::new("tool-symbols"),
                    name: "repository_symbols".to_owned(),
                    identity: crate::tools::mcp::identity::definition_identity(&tool_definition(
                        "repository_symbols",
                        ToolOrigin::Mcp {
                            server_id: McpServerId::new("python:symbols"),
                        },
                    ))
                    .expect("an MCP definition has an MCP identity"),
                    definition: tool_definition(
                        "repository_symbols",
                        ToolOrigin::Mcp {
                            server_id: McpServerId::new("python:symbols"),
                        },
                    ),
                },
            ],
            skills: vec![crate::runtime::subagent::ResolvedSubagentSkill {
                binding: crate::protocol::manifest::SkillBinding {
                    skill_id: crate::runtime::identity::SkillId::new("skill-repository-navigation"),
                    version_id: crate::runtime::identity::SkillVersionId::new("sha256:skill-v1"),
                },
                catalog_entry: crate::skills::SkillCatalogEntry {
                    name: "repository-navigation".to_owned(),
                    description: "Navigate the repository.".to_owned(),
                    location: "/w/.agents/skills/nav/SKILL.md".to_owned(),
                },
                source_root: PathBuf::from("/w/.agents/skills/nav"),
                files: vec![PathBuf::from("SKILL.md"), PathBuf::from("ref/guide.md")],
            }],
            project_instructions: vec![crate::runtime::resources::ProjectContextFile {
                path: PathBuf::from("/w/AGENTS.md"),
                content: "workspace instructions".to_owned(),
            }],
            materialization: crate::runtime::subagent::resolver::ResolvedSubagentMaterialization {
                mcp_servers: [(
                    McpServerId::new("github"),
                    crate::tools::mcp::McpServerBinding {
                        transport: crate::tools::mcp::McpTransportConfig::Stdio {
                            program: "github-mcp".to_owned(),
                            args: vec!["--stdio".to_owned()],
                            cwd: None,
                            environment: [("TOKEN".to_owned(), "x".to_owned())]
                                .into_iter()
                                .collect(),
                        },
                        policy: crate::tools::types::ToolInvocationPolicy::default(),
                    },
                )]
                .into_iter()
                .collect(),
            },
        }
    }

    #[test]
    fn the_resolved_source_identity_survives_serialization() {
        let spec = resolved_spec();
        let encoded = serde_json::to_vec(&spec).expect("encode");
        let decoded: ResolvedSubagentSpec = serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded, spec);
        assert!(matches!(
            &decoded.tools[1],
            ResolvedSubagentTool::Mcp { server_id, .. } if server_id.as_str() == "github"
        ));
        assert!(matches!(
            &decoded.tools[2],
            ResolvedSubagentTool::Mcp { server_id, .. }
                if server_id.as_str() == "python:symbols"
        ));
    }

    /// IPC v11 carries the child→parent live activity projection on the
    /// dedicated observation channel: the frame round-trips exactly and its
    /// payload is the typed `SubagentObservation`.
    #[tokio::test]
    async fn the_activity_frame_round_trips() {
        let (mut parent, mut child) = pair();
        let observation = super::super::activity::SubagentObservation {
            revision: 7,
            activity: super::super::activity::SubagentActivity::Tool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-1"),
                tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
                progress: None,
            },
            last_activity_at: None,
            counters: super::super::activity::SubagentActivityCounters {
                model_requests: 2,
                model_retries: 0,
                tool_executions: 1,
            },
        };
        write_activity_frame(
            &mut child,
            &ActivityFrame {
                observation: observation.clone(),
            },
        )
        .await
        .expect("write activity");
        assert_eq!(
            read_activity_frame(&mut parent)
                .await
                .expect("read activity"),
            Some(ActivityFrame { observation })
        );
    }

    #[tokio::test]
    async fn a_malformed_activity_payload_is_rejected() {
        let (mut parent, mut child) = pair();
        write_frame(&mut child, KIND_ACTIVITY, b"{\"observation\":")
            .await
            .expect("write");
        assert!(matches!(
            read_activity_frame(&mut parent).await,
            Err(ProtocolError::Malformed { .. })
        ));
    }

    /// The two channels never mix traffic: the observation kind on the
    /// reliable control channel is a protocol failure, and a control kind
    /// on the observation channel is one too.
    #[tokio::test]
    async fn the_two_channels_reject_each_others_kinds() {
        let (mut parent, mut child) = pair();
        let payload = encode(&ActivityFrame {
            observation: super::super::activity::SubagentObservation::default(),
        })
        .expect("encode");
        write_frame(&mut child, KIND_ACTIVITY, &payload)
            .await
            .expect("write an activity frame onto the control channel");
        assert_eq!(
            read_child_frame(&mut parent).await,
            Err(ProtocolError::UnknownKind {
                kind: KIND_ACTIVITY
            })
        );

        let (mut parent, mut child) = pair();
        write_child_frame(
            &mut child,
            &ChildFrame::Ready(ReadyFrame {
                subagent_id: SubagentId::new("conv-1-subagent-1"),
            }),
        )
        .await
        .expect("write a control frame onto the observation channel");
        assert_eq!(
            read_activity_frame(&mut parent).await,
            Err(ProtocolError::UnknownKind { kind: KIND_READY })
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // one complete typed protocol round-trip
    async fn typed_frames_round_trip() {
        let (mut parent, mut child) = pair();
        let spec = SubagentChildSpec {
            protocol_version: SUBAGENT_IPC_VERSION,
            subagent_id: SubagentId::new("conv-1-subagent-1"),
            child_conversation_id: ConversationId::new("conv-1-subagent-1"),
            child_agent_id: AgentId::new("agent-subagent-1"),
            parent_agent_id: AgentId::new("agent-parent"),
            resolved: resolved_spec(),
            approval_mode: ApprovalMode::FullAccess,
            model_timeout_policy: ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            agent_status: AgentStatusConfig::default(),
            context: SessionContextPolicy {
                reserve_tokens: 1,
                keep_recent_tokens: 2,
                summary_output_cap: None,
            },
            workspace_snapshot: WorkspaceSnapshot::shared(PathBuf::from("/tmp/ws")),
            runtime_root: PathBuf::from("/tmp/rr"),
            terminal: ChildTerminalMode::Normal,
        };
        write_parent_frame(&mut parent, &ParentFrame::Hello(Box::new(spec.clone())))
            .await
            .expect("write hello");
        write_parent_frame(
            &mut parent,
            &ParentFrame::Delegate(DelegationFrame {
                task: "inspect".to_owned(),
                context: Some("ctx".to_owned()),
                interaction_provider_available: false,
            }),
        )
        .await
        .expect("write delegate");
        write_parent_frame(
            &mut parent,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .expect("write cancel");
        let decoded_hello = read_parent_frame(&mut child)
            .await
            .expect("read hello")
            .expect("hello frame");
        let ParentFrame::Hello(decoded_spec) = decoded_hello else {
            panic!("hello frame");
        };
        assert_eq!(decoded_spec.approval_mode, ApprovalMode::FullAccess);
        assert_eq!(*decoded_spec, spec);
        assert!(matches!(
            read_parent_frame(&mut child).await.expect("read delegate"),
            Some(ParentFrame::Delegate(_))
        ));
        assert_eq!(
            read_parent_frame(&mut child).await.expect("read cancel"),
            Some(ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            })
        );

        let routed_request = InteractionRequest {
            id: InteractionId::new("interaction-questionnaire"),
            conversation_id: ConversationId::new("child-conversation"),
            attempt_id: crate::runtime::identity::AttemptId::new("attempt-1"),
            turn: 3,
            kind: InteractionKind::Questionnaire {
                questionnaire: crate::runtime::interaction::QuestionnaireSpecification {
                    questions: vec![crate::runtime::interaction::QuestionSpecification {
                        question: "Which target?".to_owned(),
                        header: "Target".to_owned(),
                        options: vec![crate::runtime::interaction::OptionSpecification {
                            label: "staging".to_owned(),
                            description: "safe".to_owned(),
                            preview: None,
                        }],
                        multi_select: false,
                    }],
                },
            },
        };
        let routed_ref = routed_request.interaction_ref();
        let response = InteractionResponse::Questionnaire {
            response: QuestionnaireResponse::Declined,
        };
        write_parent_frame(
            &mut parent,
            &ParentFrame::InteractionRespond {
                response_id: 44,
                interaction: routed_ref.clone(),
                response: response.clone(),
            },
        )
        .await
        .expect("write routed response");
        write_parent_frame(
            &mut parent,
            &ParentFrame::InteractionProviderAvailable { available: true },
        )
        .await
        .expect("write provider availability");
        write_parent_frame(
            &mut parent,
            &ParentFrame::InteractionPublicationAdmissionResult(
                InteractionPublicationAdmissionFrame {
                    request_id: 17,
                    interaction: routed_ref.clone(),
                    admitted: true,
                },
            ),
        )
        .await
        .expect("write publication admission result");
        assert_eq!(
            read_parent_frame(&mut child).await.expect("read response"),
            Some(ParentFrame::InteractionRespond {
                response_id: 44,
                interaction: routed_ref.clone(),
                response: response.clone(),
            })
        );
        assert_eq!(
            read_parent_frame(&mut child)
                .await
                .expect("read provider availability"),
            Some(ParentFrame::InteractionProviderAvailable { available: true })
        );
        assert_eq!(
            read_parent_frame(&mut child)
                .await
                .expect("read publication admission result"),
            Some(ParentFrame::InteractionPublicationAdmissionResult(
                InteractionPublicationAdmissionFrame {
                    request_id: 17,
                    interaction: routed_ref.clone(),
                    admitted: true,
                }
            ))
        );

        write_child_frame(
            &mut child,
            &ChildFrame::InteractionRequested(routed_request.clone()),
        )
        .await
        .expect("write routed request");
        write_child_frame(
            &mut child,
            &ChildFrame::InteractionPublicationAdmissionRequested(
                InteractionPublicationAdmissionFrame {
                    request_id: 17,
                    interaction: routed_ref.clone(),
                    admitted: false,
                },
            ),
        )
        .await
        .expect("write publication admission request");
        write_child_frame(
            &mut child,
            &ChildFrame::InteractionSettled {
                interaction: routed_ref.clone(),
                outcome: InteractionOutcome::Responded {
                    response: response.clone(),
                },
            },
        )
        .await
        .expect("write routed settlement");
        write_child_frame(
            &mut child,
            &ChildFrame::InteractionResponseResult(InteractionResponseResultFrame {
                response_id: 44,
                interaction: routed_ref.clone(),
                result: Ok(()),
            }),
        )
        .await
        .expect("write routed response result");
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("read routed request"),
            Some(ChildFrame::InteractionRequested(routed_request.clone()))
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("read publication admission request"),
            Some(ChildFrame::InteractionPublicationAdmissionRequested(
                InteractionPublicationAdmissionFrame {
                    request_id: 17,
                    interaction: routed_ref.clone(),
                    admitted: false,
                }
            ))
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("read routed settlement"),
            Some(ChildFrame::InteractionSettled {
                interaction: routed_ref.clone(),
                outcome: InteractionOutcome::Responded {
                    response: response.clone(),
                },
            })
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("read routed response result"),
            Some(ChildFrame::InteractionResponseResult(
                InteractionResponseResultFrame {
                    response_id: 44,
                    interaction: routed_ref,
                    result: Ok(()),
                }
            ))
        );

        write_child_frame(
            &mut child,
            &ChildFrame::Result(ResultFrame {
                status: ChildResultStatus::Succeeded,
                content: Some("answer".to_owned()),
                diagnostic: None,
            }),
        )
        .await
        .expect("write result");
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("read result"),
            Some(ChildFrame::Result(ResultFrame {
                status: ChildResultStatus::Succeeded,
                ..
            }))
        ));
    }

    /// Current IPC does not decode obsolete pre-v7 Cancel payloads. A stale
    /// peer is rejected as malformed rather than silently losing cancellation
    /// provenance through a compatibility path.
    #[tokio::test]
    async fn the_v4_empty_cancel_payload_is_not_compatibility_decoded() {
        let (mut parent, mut child) = pair();
        write_frame(&mut parent, KIND_CANCEL, &[])
            .await
            .expect("write the obsolete v4-shaped frame");
        assert!(matches!(
            read_parent_frame(&mut child).await,
            Err(ProtocolError::Malformed { .. })
        ));
    }

    #[tokio::test]
    async fn an_oversized_frame_is_rejected_before_allocation() {
        let (mut parent, mut child) = pair();
        let length = u32::try_from(MAX_FRAME_BYTES + 1).expect("bound fits u32");
        parent
            .write_all(&length.to_le_bytes())
            .await
            .expect("write length");
        assert_eq!(
            read_child_frame(&mut child).await,
            Err(ProtocolError::OversizedFrame {
                length: (MAX_FRAME_BYTES + 1)
            })
        );
    }

    #[tokio::test]
    async fn an_unknown_kind_is_rejected() {
        let (mut parent, mut child) = pair();
        write_frame(&mut parent, 77, b"{}").await.expect("write");
        assert_eq!(
            read_child_frame(&mut child).await,
            Err(ProtocolError::UnknownKind { kind: 77 })
        );
    }

    #[tokio::test]
    async fn a_malformed_payload_is_rejected() {
        let (mut parent, mut child) = pair();
        write_frame(&mut parent, KIND_RESULT, b"{not json")
            .await
            .expect("write");
        assert!(matches!(
            read_child_frame(&mut child).await,
            Err(ProtocolError::Malformed { .. })
        ));
    }

    #[tokio::test]
    async fn eof_at_a_frame_boundary_is_clean() {
        let (parent, mut child) = pair();
        drop(parent);
        assert_eq!(read_parent_frame(&mut child).await, Ok(None));
    }
}
