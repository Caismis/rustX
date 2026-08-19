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
//! # Transport
//!
//! The channel is one inherited `UnixStream` pair endpoint passed as the
//! child process's standard input (fd 0). The parent's endpoint closes when
//! the parent process dies — for any reason, including `SIGKILL` — so the
//! same channel is the parent-liveness authority: a child that observes EOF
//! before its terminal settlement drains and exits. No socket path, no
//! listener, no network endpoint, and no PID polling is involved.

use std::path::PathBuf;

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::context::SessionContextPolicy;
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId};

/// The only subagent control protocol version this build speaks.
pub(crate) const SUBAGENT_IPC_VERSION: u16 = 1;

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

// Child -> parent frame kinds.
const KIND_READY: u8 = 101;
const KIND_STARTUP_ERROR: u8 = 102;
const KIND_RESULT: u8 = 103;
const KIND_DIAGNOSTIC: u8 = 104;

/// The typed startup specification of one subagent child, carried by the
/// `Hello` frame.
///
/// This is the one typed composition boundary between the parent and the
/// child runtime: the child composes its headless `ConversationRuntime`
/// from exactly this typed input plus the model catalog file at
/// [`SubagentChildSpec::models`]. No temporary session configuration file
/// is ever written.
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
    /// The frozen profile identity.
    pub profile: String,
    /// The profile persona composed as the child's bootstrap system
    /// configuration.
    pub persona: String,
    /// The model catalog file path (inherited from parent startup).
    pub models: PathBuf,
    /// The frozen session model configuration of the child.
    pub model: SessionModelConfig,
    /// The conversation timezone of the child.
    pub timezone: Option<Tz>,
    /// The session context policy of the child.
    pub context: SessionContextPolicy,
    /// The shared (read-only) workspace root.
    pub workspace: PathBuf,
    /// The child-private runtime root (artifacts store, diagnostics).
    pub runtime_root: PathBuf,
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

/// One decoded parent-bound frame.
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
}

/// One decoded child-bound frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParentFrame {
    /// The startup specification (exactly once, first).
    Hello(Box<SubagentChildSpec>),
    /// The delegated task (exactly once, after `Ready`).
    Delegate(DelegationFrame),
    /// Cancellation/shutdown request.
    Cancel,
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
pub(crate) async fn write_frame(
    stream: &mut tokio::net::UnixStream,
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
pub(crate) async fn read_frame(
    stream: &mut tokio::net::UnixStream,
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

/// Writes one typed parent-bound frame.
pub(crate) async fn write_child_frame(
    stream: &mut tokio::net::UnixStream,
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
    }
}

/// Reads and decodes one typed parent-bound frame.
pub(crate) async fn read_child_frame(
    stream: &mut tokio::net::UnixStream,
) -> Result<Option<ChildFrame>, ProtocolError> {
    let Some((kind, payload)) = read_frame(stream).await? else {
        return Ok(None);
    };
    let frame = match kind {
        KIND_READY => ChildFrame::Ready(decode(&payload)?),
        KIND_STARTUP_ERROR => ChildFrame::StartupError(decode(&payload)?),
        KIND_RESULT => ChildFrame::Result(decode(&payload)?),
        KIND_DIAGNOSTIC => ChildFrame::Diagnostic(decode(&payload)?),
        other => return Err(ProtocolError::UnknownKind { kind: other }),
    };
    Ok(Some(frame))
}

/// Writes one typed child-bound frame.
pub(crate) async fn write_parent_frame(
    stream: &mut tokio::net::UnixStream,
    frame: &ParentFrame,
) -> Result<(), ProtocolError> {
    match frame {
        ParentFrame::Hello(payload) => write_frame(stream, KIND_HELLO, &encode(payload)?).await,
        ParentFrame::Delegate(payload) => {
            write_frame(stream, KIND_DELEGATE, &encode(payload)?).await
        }
        ParentFrame::Cancel => write_frame(stream, KIND_CANCEL, &[]).await,
    }
}

/// Reads and decodes one typed child-bound frame.
pub(crate) async fn read_parent_frame(
    stream: &mut tokio::net::UnixStream,
) -> Result<Option<ParentFrame>, ProtocolError> {
    let Some((kind, payload)) = read_frame(stream).await? else {
        return Ok(None);
    };
    let frame = match kind {
        KIND_HELLO => ParentFrame::Hello(Box::new(decode(&payload)?)),
        KIND_DELEGATE => ParentFrame::Delegate(decode(&payload)?),
        KIND_CANCEL => ParentFrame::Cancel,
        other => return Err(ProtocolError::UnknownKind { kind: other }),
    };
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        tokio::net::UnixStream::pair().expect("control socket pair")
    }

    #[tokio::test]
    async fn typed_frames_round_trip() {
        let (mut parent, mut child) = pair();
        let spec = SubagentChildSpec {
            protocol_version: SUBAGENT_IPC_VERSION,
            subagent_id: SubagentId::new("conv-1-subagent-1"),
            child_conversation_id: ConversationId::new("conv-1-subagent-1"),
            child_agent_id: AgentId::new("agent-subagent-1"),
            parent_agent_id: AgentId::new("agent-parent"),
            profile: "explore".to_owned(),
            persona: "persona".to_owned(),
            models: PathBuf::from("/tmp/models.json"),
            model: SessionModelConfig::of(
                serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
            ),
            timezone: None,
            context: SessionContextPolicy {
                reserve_tokens: 1,
                keep_recent_tokens: 2,
                summary_output_cap: None,
            },
            workspace: PathBuf::from("/tmp/ws"),
            runtime_root: PathBuf::from("/tmp/rr"),
        };
        write_parent_frame(&mut parent, &ParentFrame::Hello(Box::new(spec.clone())))
            .await
            .expect("write hello");
        write_parent_frame(
            &mut parent,
            &ParentFrame::Delegate(DelegationFrame {
                task: "inspect".to_owned(),
                context: Some("ctx".to_owned()),
            }),
        )
        .await
        .expect("write delegate");
        write_parent_frame(&mut parent, &ParentFrame::Cancel)
            .await
            .expect("write cancel");
        assert_eq!(
            read_parent_frame(&mut child).await.expect("read hello"),
            Some(ParentFrame::Hello(Box::new(spec)))
        );
        assert!(matches!(
            read_parent_frame(&mut child).await.expect("read delegate"),
            Some(ParentFrame::Delegate(_))
        ));
        assert_eq!(
            read_parent_frame(&mut child).await.expect("read cancel"),
            Some(ParentFrame::Cancel)
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
