//! The typed publication vocabulary.
//!
//! The vocabulary is deliberately lossless enough to reconstruct exactly what
//! rustX committed for release, and no more: it is a release record, never a
//! second canonical message representation and never a Tool Plane execution
//! record.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::message::types::ContentBlockIndex;
use crate::runtime::identity::{
    AttemptId, MessageId, PublicationStreamId, RequestId, ToolCallId, ToolId, TurnId,
};
use crate::tools::types::{ToolCall, ToolCallStart};

/// One committed-for-release payload of a publication stream.
///
/// Every suffix variant carries the **increment** rustX committed, not the
/// accumulated block: the accumulated form is recovered by folding a stream's
/// frames in sequence order (see [`consolidate_audit_content`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicationPayload {
    /// A committed-for-release suffix of one text output block.
    TextSuffix {
        /// The output block the suffix extends.
        block_index: ContentBlockIndex,
        /// The committed suffix.
        suffix: String,
    },
    /// A committed-for-release suffix of one reasoning output block.
    ReasoningSuffix {
        /// The output block the suffix extends.
        block_index: ContentBlockIndex,
        /// The committed suffix.
        suffix: String,
    },
    /// A committed-for-release suffix of one refusal output block. Refusal is
    /// preserved as refusal and never flattened into text.
    RefusalSuffix {
        /// The output block the suffix extends.
        block_index: ContentBlockIndex,
        /// The committed suffix.
        suffix: String,
    },
    /// A model **proposal** to call a tool started.
    ///
    /// This is not a Tool Plane fact: nothing was authorized, invoked, or
    /// executed by the appearance of this frame.
    ProposedToolCallStarted {
        /// The output block carrying the proposal.
        block_index: ContentBlockIndex,
        /// The proposal identity and metadata known at start.
        call: ToolCallStart,
    },
    /// A committed-for-release suffix of one tool-call proposal's raw
    /// argument text.
    ProposedToolCallArgumentsSuffix {
        /// The output block carrying the proposal.
        block_index: ContentBlockIndex,
        /// The proposal being assembled.
        call_id: ToolCallId,
        /// The committed raw argument suffix.
        suffix: String,
    },
    /// A model tool-call **proposal** finished assembling.
    ///
    /// A completed proposal is still only a proposal. Whether it was ever
    /// authorized or executed is a Tool Plane fact recorded in the Event
    /// Journal, never in this plane.
    ProposedToolCallCompleted {
        /// The output block carrying the proposal.
        block_index: ContentBlockIndex,
        /// The fully assembled proposal.
        call: ToolCall,
    },
    /// Provider completion was structurally accepted with no buffered visible
    /// payload remaining.
    ///
    /// The frame exists so the publication terminal transition always has a
    /// frame to commit atomically with its marker, without delaying any
    /// visible text that does not exist.
    TerminalOnly,
}

impl PublicationPayload {
    /// The committed byte weight of this payload, used by the bounded
    /// coalescing policy. Structural frames weigh their carried text only.
    #[must_use]
    pub fn byte_weight(&self) -> usize {
        match self {
            Self::TextSuffix { suffix, .. }
            | Self::ReasoningSuffix { suffix, .. }
            | Self::RefusalSuffix { suffix, .. }
            | Self::ProposedToolCallArgumentsSuffix { suffix, .. } => suffix.len(),
            Self::ProposedToolCallStarted { .. }
            | Self::ProposedToolCallCompleted { .. }
            | Self::TerminalOnly => 0,
        }
    }

    /// Whether this payload is a structural boundary that must not be
    /// coalesced across: a proposal start or completion is released as its
    /// own observable transition.
    #[must_use]
    pub const fn is_structural_boundary(&self) -> bool {
        matches!(
            self,
            Self::ProposedToolCallStarted { .. } | Self::ProposedToolCallCompleted { .. }
        )
    }

    /// Merges `other` into this payload when both are adjacent suffixes of
    /// the same target, returning `other` unchanged when they cannot merge.
    ///
    /// Coalescing is only ever suffix concatenation on one identical target,
    /// so a merged frame carries exactly the bytes the provider produced, in
    /// the order it produced them.
    pub fn merge(&mut self, other: Self) -> Option<Self> {
        match (self, other) {
            (
                Self::TextSuffix {
                    block_index,
                    suffix,
                },
                Self::TextSuffix {
                    block_index: other_index,
                    suffix: other_suffix,
                },
            )
            | (
                Self::ReasoningSuffix {
                    block_index,
                    suffix,
                },
                Self::ReasoningSuffix {
                    block_index: other_index,
                    suffix: other_suffix,
                },
            )
            | (
                Self::RefusalSuffix {
                    block_index,
                    suffix,
                },
                Self::RefusalSuffix {
                    block_index: other_index,
                    suffix: other_suffix,
                },
            ) if *block_index == other_index => {
                suffix.push_str(&other_suffix);
                None
            }
            (
                Self::ProposedToolCallArgumentsSuffix {
                    block_index,
                    call_id,
                    suffix,
                },
                Self::ProposedToolCallArgumentsSuffix {
                    block_index: other_index,
                    call_id: other_call,
                    suffix: other_suffix,
                },
            ) if *block_index == other_index && *call_id == other_call => {
                suffix.push_str(&other_suffix);
                None
            }
            (_, other) => Some(other),
        }
    }
}

/// One durable publication frame: the unit rustX commits before release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationFrame {
    /// The publication stream this frame belongs to.
    pub stream_id: PublicationStreamId,
    /// The provisional canonical identity the stream publishes under.
    pub message_id: MessageId,
    /// The stream-local monotonic frame sequence, from zero.
    pub sequence: u64,
    /// The committed-for-release payload.
    pub payload: PublicationPayload,
}

/// The immutable identity of one opened publication stream.
///
/// Every field pins the stream to the exact request generation that started
/// it: an external resource, Skill, or Tool configuration edit during
/// streaming can never re-associate an in-flight stream with a newer
/// generation, and recovery classifies the stream from these frozen
/// identities alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationStreamStart {
    /// The publication identity.
    pub stream_id: PublicationStreamId,
    /// The owning attempt.
    pub attempt_id: AttemptId,
    /// The owning turn.
    pub turn_id: TurnId,
    /// The exact provider request whose output this stream publishes.
    pub request_id: RequestId,
    /// The provisional canonical identity the stream publishes under.
    pub message_id: MessageId,
}

/// The one final settlement of a publication stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationSettlement {
    /// Publication reached U and the Assistant reached C. The canonical
    /// Assistant is the long-term conversation authority.
    Canonical,
    /// Publication reached U but C never occurred.
    Unaccepted,
    /// Publication never reached U.
    Incomplete,
}

impl PublicationSettlement {
    /// The durable textual form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Unaccepted => "unaccepted",
            Self::Incomplete => "incomplete",
        }
    }

    /// Parses the durable textual form.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "canonical" => Some(Self::Canonical),
            "unaccepted" => Some(Self::Unaccepted),
            "incomplete" => Some(Self::Incomplete),
            _ => None,
        }
    }
}

/// The audit settlement kinds — the two terminal settlements that are not
/// canonical conversation acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationAuditKind {
    /// Publication reached U, but the Assistant was never accepted.
    Unaccepted,
    /// Publication never reached its own durable terminal boundary.
    Incomplete,
}

impl From<PublicationAuditKind> for PublicationSettlement {
    fn from(kind: PublicationAuditKind) -> Self {
        match kind {
            PublicationAuditKind::Unaccepted => Self::Unaccepted,
            PublicationAuditKind::Incomplete => Self::Incomplete,
        }
    }
}

/// One consolidated block of an immutable publication audit.
///
/// Consolidation is what keeps the audit bounded: a stream that staged ten
/// thousand frames leaves one audit object whose size is the released output,
/// never O(number-of-frames) permanent staging rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicationAuditBlock {
    /// The complete released text of one output block.
    Text {
        /// The output block.
        block_index: ContentBlockIndex,
        /// The released text.
        text: String,
    },
    /// The complete released reasoning of one output block.
    Reasoning {
        /// The output block.
        block_index: ContentBlockIndex,
        /// The released reasoning text.
        text: String,
    },
    /// The complete released refusal of one output block.
    Refusal {
        /// The output block.
        block_index: ContentBlockIndex,
        /// The released refusal text.
        text: String,
    },
    /// A released **model tool-call proposal**.
    ///
    /// Its presence in an audit proves only that rustX committed the proposal
    /// for release. It is never evidence that the call was authorized,
    /// started, or executed.
    ProposedToolCall {
        /// The output block.
        block_index: ContentBlockIndex,
        /// The proposal identity.
        call_id: ToolCallId,
        /// The registry-facing tool identity the model named.
        tool_id: ToolId,
        /// The tool name the model named.
        name: String,
        /// The released raw argument text, exactly as far as it was released.
        arguments: String,
        /// Whether the proposal finished assembling before the stream ended.
        /// A partial proposal can never have been executed.
        complete: bool,
    },
}

/// One bounded immutable audit of a settled non-canonical publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationAudit {
    /// The settled publication stream.
    pub stream_id: PublicationStreamId,
    /// The attempt that owned the stream.
    pub attempt_id: AttemptId,
    /// The turn that owned the stream.
    pub turn_id: TurnId,
    /// The provider request the stream published.
    pub request_id: RequestId,
    /// The provisional canonical identity the stream published under. No
    /// canonical message exists under it.
    pub message_id: MessageId,
    /// Which of the two audit settlements this is.
    pub kind: PublicationAuditKind,
    /// The consolidated committed-for-release content, in block order.
    pub content: Vec<PublicationAuditBlock>,
    /// When the audit terminalized.
    pub settled_at: DateTime<Utc>,
}

impl PublicationAudit {
    /// The tool-call proposals released by this settled publication.
    ///
    /// Every returned identity is permanently forbidden from having a
    /// dependent Tool Plane execution fact.
    #[must_use]
    pub fn proposed_call_ids(&self) -> Vec<ToolCallId> {
        self.content
            .iter()
            .filter_map(|block| match block {
                PublicationAuditBlock::ProposedToolCall { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The durable record of one publication stream, as recovery reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationStreamRecord {
    /// The frozen stream identity.
    pub start: PublicationStreamStart,
    /// The publication terminal marker: the sequence of the final frame of
    /// the U transaction, or `None` when U never committed.
    pub terminal_sequence: Option<u64>,
    /// The final settlement, or `None` while the stream is unsettled.
    pub settlement: Option<PublicationSettlement>,
}

impl PublicationStreamRecord {
    /// Whether U committed for this stream.
    #[must_use]
    pub const fn reached_publication_terminal(&self) -> bool {
        self.terminal_sequence.is_some()
    }

    /// The audit kind an unsettled stream must terminalize as, derived
    /// entirely from durable evidence.
    ///
    /// `U` present means the released output was complete but never accepted;
    /// `U` absent means publication never reached its own terminal boundary,
    /// regardless of whether the provider reached transport termination.
    #[must_use]
    pub const fn audit_kind(&self) -> PublicationAuditKind {
        if self.reached_publication_terminal() {
            PublicationAuditKind::Unaccepted
        } else {
            PublicationAuditKind::Incomplete
        }
    }
}

/// Folds a stream's staged frames, in sequence order, into the bounded
/// immutable audit content.
///
/// The fold is pure and total: the same frames always produce the same audit,
/// so a consolidation performed at settlement time and one performed by a
/// later recovery are identical.
#[must_use]
pub fn consolidate_audit_content(frames: &[PublicationFrame]) -> Vec<PublicationAuditBlock> {
    let mut blocks: Vec<PublicationAuditBlock> = Vec::new();
    for frame in frames {
        match &frame.payload {
            PublicationPayload::TextSuffix {
                block_index,
                suffix,
            } => {
                push_text(&mut blocks, *block_index, suffix, TextKind::Text);
            }
            PublicationPayload::ReasoningSuffix {
                block_index,
                suffix,
            } => {
                push_text(&mut blocks, *block_index, suffix, TextKind::Reasoning);
            }
            PublicationPayload::RefusalSuffix {
                block_index,
                suffix,
            } => {
                push_text(&mut blocks, *block_index, suffix, TextKind::Refusal);
            }
            PublicationPayload::ProposedToolCallStarted { block_index, call } => {
                blocks.push(PublicationAuditBlock::ProposedToolCall {
                    block_index: *block_index,
                    call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                    arguments: String::new(),
                    complete: false,
                });
            }
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                call_id, suffix, ..
            } => {
                if let Some(PublicationAuditBlock::ProposedToolCall { arguments, .. }) =
                    proposal_mut(&mut blocks, call_id)
                {
                    arguments.push_str(suffix);
                }
            }
            PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
                if let Some(PublicationAuditBlock::ProposedToolCall {
                    arguments,
                    complete,
                    ..
                }) = proposal_mut(&mut blocks, &call.id)
                {
                    *complete = true;
                    if arguments.is_empty() {
                        *arguments = call.arguments.to_string();
                    }
                } else {
                    blocks.push(PublicationAuditBlock::ProposedToolCall {
                        block_index: *block_index,
                        call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.to_string(),
                        complete: true,
                    });
                }
            }
            PublicationPayload::TerminalOnly => {}
        }
    }
    blocks
}

/// The three textual audit block kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Text,
    Reasoning,
    Refusal,
}

fn push_text(
    blocks: &mut Vec<PublicationAuditBlock>,
    block_index: ContentBlockIndex,
    suffix: &str,
    kind: TextKind,
) {
    for block in blocks.iter_mut() {
        match (kind, block) {
            (
                TextKind::Text,
                PublicationAuditBlock::Text {
                    block_index: existing,
                    text,
                },
            )
            | (
                TextKind::Reasoning,
                PublicationAuditBlock::Reasoning {
                    block_index: existing,
                    text,
                },
            )
            | (
                TextKind::Refusal,
                PublicationAuditBlock::Refusal {
                    block_index: existing,
                    text,
                },
            ) if *existing == block_index => {
                text.push_str(suffix);
                return;
            }
            _ => {}
        }
    }
    blocks.push(match kind {
        TextKind::Text => PublicationAuditBlock::Text {
            block_index,
            text: suffix.to_owned(),
        },
        TextKind::Reasoning => PublicationAuditBlock::Reasoning {
            block_index,
            text: suffix.to_owned(),
        },
        TextKind::Refusal => PublicationAuditBlock::Refusal {
            block_index,
            text: suffix.to_owned(),
        },
    });
}

fn proposal_mut<'a>(
    blocks: &'a mut [PublicationAuditBlock],
    call_id: &ToolCallId,
) -> Option<&'a mut PublicationAuditBlock> {
    blocks.iter_mut().find(|block| {
        matches!(
            block,
            PublicationAuditBlock::ProposedToolCall { call_id: existing, .. } if existing == call_id
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PublicationAuditBlock, PublicationFrame, PublicationPayload, PublicationSettlement,
        consolidate_audit_content,
    };
    use crate::message::types::ContentBlockIndex;
    use crate::runtime::identity::{MessageId, PublicationStreamId, ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolCallStart};

    fn frame(sequence: u64, payload: PublicationPayload) -> PublicationFrame {
        PublicationFrame {
            stream_id: PublicationStreamId::new("stream-1"),
            message_id: MessageId::new("message-1"),
            sequence,
            payload,
        }
    }

    /// Adjacent suffixes of the same target merge; a different target does
    /// not merge into the previous payload.
    #[test]
    fn suffix_merging_is_target_scoped() {
        let mut payload = PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: "he".to_owned(),
        };
        assert!(
            payload
                .merge(PublicationPayload::TextSuffix {
                    block_index: ContentBlockIndex::new(0),
                    suffix: "llo".to_owned(),
                })
                .is_none()
        );
        assert_eq!(
            payload,
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "hello".to_owned(),
            }
        );
        let rejected = payload.merge(PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(1),
            suffix: "!".to_owned(),
        });
        assert!(rejected.is_some());
        let rejected = payload.merge(PublicationPayload::ReasoningSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: "?".to_owned(),
        });
        assert!(rejected.is_some());
    }

    /// Consolidation folds many frames into one bounded audit object that
    /// keeps text, reasoning, refusal, and proposal identity distinct.
    #[test]
    fn consolidation_is_bounded_and_lossless_per_block() {
        let frames = vec![
            frame(
                0,
                PublicationPayload::ReasoningSuffix {
                    block_index: ContentBlockIndex::new(0),
                    suffix: "think".to_owned(),
                },
            ),
            frame(
                1,
                PublicationPayload::TextSuffix {
                    block_index: ContentBlockIndex::new(1),
                    suffix: "hel".to_owned(),
                },
            ),
            frame(
                2,
                PublicationPayload::TextSuffix {
                    block_index: ContentBlockIndex::new(1),
                    suffix: "lo".to_owned(),
                },
            ),
            frame(
                3,
                PublicationPayload::ProposedToolCallStarted {
                    block_index: ContentBlockIndex::new(2),
                    call: ToolCallStart {
                        id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-a"),
                        name: "alpha".to_owned(),
                    },
                },
            ),
            frame(
                4,
                PublicationPayload::ProposedToolCallArgumentsSuffix {
                    block_index: ContentBlockIndex::new(2),
                    call_id: ToolCallId::new("call-1"),
                    suffix: r#"{"x":"#.to_owned(),
                },
            ),
        ];
        let content = consolidate_audit_content(&frames);
        assert_eq!(content.len(), 3);
        assert!(matches!(
            &content[0],
            PublicationAuditBlock::Reasoning { text, .. } if text == "think"
        ));
        assert!(matches!(
            &content[1],
            PublicationAuditBlock::Text { text, .. } if text == "hello"
        ));
        assert!(matches!(
            &content[2],
            PublicationAuditBlock::ProposedToolCall { arguments, complete, .. }
                if arguments == r#"{"x":"# && !complete
        ));
    }

    /// A completed proposal marks the audit block complete without losing the
    /// released argument text.
    #[test]
    fn completed_proposal_is_marked_complete() {
        let frames = vec![
            frame(
                0,
                PublicationPayload::ProposedToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: ToolCallStart {
                        id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-a"),
                        name: "alpha".to_owned(),
                    },
                },
            ),
            frame(
                1,
                PublicationPayload::ProposedToolCallArgumentsSuffix {
                    block_index: ContentBlockIndex::new(0),
                    call_id: ToolCallId::new("call-1"),
                    suffix: r#"{"x":1}"#.to_owned(),
                },
            ),
            frame(
                2,
                PublicationPayload::ProposedToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call: ToolCall {
                        id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-a"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({"x": 1}),
                    },
                },
            ),
        ];
        let content = consolidate_audit_content(&frames);
        assert_eq!(content.len(), 1);
        assert!(matches!(
            &content[0],
            PublicationAuditBlock::ProposedToolCall { arguments, complete, .. }
                if arguments == r#"{"x":1}"# && *complete
        ));
    }

    /// The durable settlement form round-trips.
    #[test]
    fn settlement_round_trips_through_its_durable_form() {
        for settlement in [
            PublicationSettlement::Canonical,
            PublicationSettlement::Unaccepted,
            PublicationSettlement::Incomplete,
        ] {
            assert_eq!(
                PublicationSettlement::parse(settlement.as_str()),
                Some(settlement)
            );
        }
        assert!(PublicationSettlement::parse("published").is_none());
    }
}
