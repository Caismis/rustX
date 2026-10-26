//! Finite transcript descriptors prepared by native owners from the durable
//! transcript and canonical Ledger. Clients never fabricate a display item.
use super::tool::{
    ForegroundToolExecution, ForegroundToolState, RuntimeClientToolOccurrence, inline_call_id,
    inline_message_id, inline_tool_id,
};
use crate::events::interaction::{InteractionSettlement, InteractionSubject};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::publication::PublicationAudit;
use crate::runtime::identity::{AttemptId, EventId, InteractionId, MessageId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientMessageDescriptor {
    pub deliveries: Option<super::delivery::RuntimeClientDeliveryHead>,
    pub identity: Option<MessageId>,
    pub locator: super::content::CanonicalContentLocator,
    pub role: String,
    pub kind: String,
    pub block_count: u64,
    pub tool_call_count: u64,
    /// Finite native Tool occurrences; remaining calls use the advertised frontier.
    pub tool_calls: Vec<ForegroundToolExecution>,
    pub turn_process: Option<super::turn::TurnProcessView>,
    pub completed_response: Option<super::turn::CompletedResponseView>,
    pub content: Vec<super::cut::ContentBinding>,
    pub collections: Vec<super::cut::CollectionFrontier>,
}
impl RuntimeClientMessageDescriptor {
    pub(crate) fn from_header(header: &crate::durable::observation::MessageHeader) -> Option<Self> {
        Some(Self {
            deliveries: header.deliveries.clone(),
            identity: header.identity.clone(),
            locator: header.locator.clone()?,
            role: header.role.clone(),
            kind: header.kind.clone(),
            block_count: header.block_count,
            tool_call_count: header.tool_call_count,
            tool_calls: header.tool_calls.clone(),
            turn_process: None,
            completed_response: None,
            content: header.content.clone(),
            collections: header.collections.clone(),
        })
    }
}
/// One bounded newest-or-older page of derived transcript history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientTranscriptPage {
    /// The exact native source cut from which bodies and evidence were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_cut: Option<crate::durable::observation::ObservationReadCut>,
    /// Deferred bytes remain explicitly recoverable on older historical pages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<super::cut::ContentBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<super::cut::CollectionFrontier>,
    /// Native items whose structural display requires bounded further reading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_entries: Vec<super::cut::CheckpointPageItem>,

    /// Last native transcript position inherited by this lineage; zero for an empty prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_through: Option<RuntimeClientTranscriptCursor>,
    /// Native whole-conversation totals, independent of this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistics: Option<super::statistics::ConversationStatistics>,
    /// Items in chronological order within this page.
    #[serde(default)]
    pub entries: Vec<RuntimeClientTranscriptEntry>,
    /// The exclusive cursor for the next older page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<RuntimeClientTranscriptCursor>,
}
/// One derived transcript item and its stable durable cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientTranscriptEntry {
    /// Exact native Attempt process membership, derived by native owners.
    /// Absence is not permission for a client to infer a process boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_process: Option<super::turn::TurnProcessView>,
    /// The native Attempt has not yet settled this accepted Assistant candidate.
    /// A client retaining this row outside a refresh must reread it, not freeze absence.
    #[serde(default)]
    pub response_pending: bool,
    /// Exact completed-response evidence, never inferred by clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_response: Option<super::turn::CompletedResponseView>,
    /// Canonical calls in block order with their native committed results.
    /// A missing result means no committed result, never success or cancellation.
    #[serde(default)]
    pub tool_calls: Vec<ForegroundToolExecution>,
    /// The durable transcript position, not the Runtime Client event cursor.
    pub cursor: RuntimeClientTranscriptCursor,
    /// The typed item resolved from a canonical durable owner.
    pub item: RuntimeClientTranscriptItem,
}
/// One live-published requested interaction audit projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientTranscriptInteractionRequested {
    /// Durable Event Journal event identity.
    pub event_id: EventId,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Owning Attempt; absent for native Scheduled work.
    pub attempt_id: Option<AttemptId>,
    /// Owning turn; absent for native Scheduled work.
    pub turn_id: Option<TurnId>,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Bounded durable subject.
    pub subject: InteractionSubject,
}
/// One live-published settled interaction audit projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientTranscriptInteractionSettled {
    /// Durable Event Journal event identity.
    pub event_id: EventId,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Owning Attempt; absent for native Scheduled work.
    pub attempt_id: Option<AttemptId>,
    /// Owning turn; absent for native Scheduled work.
    pub turn_id: Option<TurnId>,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Bounded durable settlement.
    pub settlement: InteractionSettlement,
}
/// The explicit Runtime Client transcript vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientTranscriptItem {
    /// Journal-owned terminal execution history, including attempts without messages.
    AttemptTerminal { turn: super::turn::TurnProcessView },
    /// A user, Assistant, or Tool message body from Pending Inbound or Ledger.
    Message {
        /// The canonical or durably accepted message.
        message: MessageBlock,
    },
    /// A noncanonical Assistant publication audit.
    PublicationAudit {
        /// The bounded immutable publication audit.
        audit: PublicationAudit,
    },
    /// A historical interaction request audit. It is never a live waiter.
    InteractionRequested {
        /// Durable Event Journal event identity.
        event_id: EventId,
        /// Event timestamp.
        timestamp: DateTime<Utc>,
        /// Owning Attempt; absent for native Scheduled work.
        attempt_id: Option<AttemptId>,
        /// Owning turn; absent for native Scheduled work.
        turn_id: Option<TurnId>,
        /// Interaction identity.
        interaction_id: InteractionId,
        /// Bounded durable subject.
        subject: InteractionSubject,
    },
    /// A historical interaction settlement audit. It is never actionable.
    InteractionSettled {
        /// Durable Event Journal event identity.
        event_id: EventId,
        /// Event timestamp.
        timestamp: DateTime<Utc>,
        /// Owning Attempt; absent for native Scheduled work.
        attempt_id: Option<AttemptId>,
        /// Owning turn; absent for native Scheduled work.
        turn_id: Option<TurnId>,
        /// Interaction identity.
        interaction_id: InteractionId,
        /// Bounded durable settlement.
        settlement: InteractionSettlement,
    },
}
/// The cursor domain of durable transcript paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientTranscriptCursor(u64);
impl RuntimeClientTranscriptCursor {
    /// Creates a transcript cursor from its wire value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the wire value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
impl From<crate::durable::TranscriptCursor> for RuntimeClientTranscriptCursor {
    fn from(cursor: crate::durable::TranscriptCursor) -> Self {
        Self::new(cursor.get())
    }
}
impl From<RuntimeClientTranscriptCursor> for crate::durable::TranscriptCursor {
    fn from(cursor: RuntimeClientTranscriptCursor) -> Self {
        Self::new(cursor.get())
    }
}
/// Converts one durable transcript page into the explicit Runtime Client
/// read model. Invalid durable interaction shapes are rejected rather than
/// fabricated into a display item.
pub(crate) fn transcript_page_view(
    page: crate::durable::TranscriptPage,
) -> Result<RuntimeClientTranscriptPage, String> {
    let entries = page
        .entries
        .into_iter()
        .map(transcript_entry_view)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RuntimeClientTranscriptPage {
        source_cut: None,
        content: Vec::new(),
        collections: Vec::new(),
        deferred_entries: Vec::new(),
        inherited_through: Some(page.inherited_through.into()),
        statistics: None,
        entries,
        next_cursor: page.next_cursor.map(Into::into),
    })
}
/// Converts indexed native headers without loading canonical, audit, or event bodies.
pub(crate) fn transcript_header_view(
    page: crate::durable::observation::TranscriptHeaderPage,
) -> RuntimeClientTranscriptPage {
    let mut result = RuntimeClientTranscriptPage {
        source_cut: Some(page.cut.clone()),
        inherited_through: Some(page.cut.inherited_through.into()),
        next_cursor: page.next_cursor.map(Into::into),
        ..RuntimeClientTranscriptPage::default()
    };
    for header in page.headers {
        if let Some(mut display) = header.display {
            display.tool_calls = header.tool_calls;
            let index = result.entries.len();
            result.entries.push(display);
            for mut binding in header.content {
                if binding
                    .path
                    .first()
                    .is_some_and(|segment| segment == "display")
                {
                    binding.path.remove(0);
                }
                let mut path = vec!["entries".to_owned(), index.to_string()];
                path.append(&mut binding.path);
                binding.path = path;
                result.content.push(binding);
            }
            result.collections.extend(header.collections);
        } else {
            result.collections.extend(header.collections);
            result
                .deferred_entries
                .push(super::cut::CheckpointPageItem {
                    body_complete: None,
                    key: None,
                    offset: header.cursor.get(),
                    identity: header.reference_id,
                    value: header
                        .message
                        .as_ref()
                        .and_then(RuntimeClientMessageDescriptor::from_header)
                        .map(|mut descriptor| {
                            descriptor.tool_calls = header.tool_calls;
                            descriptor
                        })
                        .as_ref()
                        .and_then(super::bounded::structural_value)
                        .or_else(|| {
                            header
                                .publication_settlement
                                .as_ref()
                                .and_then(super::bounded::structural_value)
                        }),
                    content: header.content,
                    deferred: true,
                });
        }
    }
    result.collections.push(super::cut::CollectionFrontier {
        domain: "transcript.entries".into(),
        revision: page.cut.read.journal,
        total: page.total,
        loaded: result.entries.len() as u64,
        next_offset: page.next_cursor.map(crate::durable::TranscriptCursor::get),
    });
    result
}
fn transcript_entry_view(
    entry: crate::durable::TranscriptEntry,
) -> Result<RuntimeClientTranscriptEntry, String> {
    let item = match entry.item {
        crate::durable::TranscriptItem::InheritedTurn { provenance } => {
            RuntimeClientTranscriptItem::AttemptTerminal {
                turn: inherited_terminal(provenance, entry.cursor.into())?,
            }
        }
        crate::durable::TranscriptItem::AttemptTerminal { event } => {
            RuntimeClientTranscriptItem::AttemptTerminal {
                turn: super::turn::terminal_turn(event, entry.cursor.into())?,
            }
        }

        crate::durable::TranscriptItem::Message { message } => {
            RuntimeClientTranscriptItem::Message { message }
        }
        crate::durable::TranscriptItem::PublicationAudit { audit } => {
            RuntimeClientTranscriptItem::PublicationAudit { audit }
        }
        crate::durable::TranscriptItem::InteractionRequested { event } => {
            let audit = interaction_requested_view(event)?;
            RuntimeClientTranscriptItem::InteractionRequested {
                event_id: audit.event_id,
                timestamp: audit.timestamp,
                attempt_id: audit.attempt_id,
                turn_id: audit.turn_id,
                interaction_id: audit.interaction_id,
                subject: audit.subject,
            }
        }
        crate::durable::TranscriptItem::InteractionSettled { event } => {
            let audit = interaction_settled_view(event)?;
            RuntimeClientTranscriptItem::InteractionSettled {
                event_id: audit.event_id,
                timestamp: audit.timestamp,
                attempt_id: audit.attempt_id,
                turn_id: audit.turn_id,
                interaction_id: audit.interaction_id,
                settlement: audit.settlement,
            }
        }
    };
    Ok(RuntimeClientTranscriptEntry {
        turn_process: None,
        completed_response: None,
        response_pending: false,
        cursor: entry.cursor.into(),
        tool_calls: entry
            .tool_calls
            .into_iter()
            .map(transcript_tool_view)
            .collect(),
        item,
    })
}
fn transcript_tool_view(tool: crate::durable::TranscriptTool) -> ForegroundToolExecution {
    ForegroundToolExecution {
        committed_result_deferred: false,
        committed_result: tool.result,
        result_source: None,
        result_metadata_ref: None,
        occurrence: RuntimeClientToolOccurrence {
            locator: super::content::CanonicalContentLocator::ByMessageId {
                message_id: tool.message_id.clone(),
            },
            block_index: tool.block_index,
        },
        message_id: inline_message_id(&tool.message_id),
        block_index: tool.block_index,
        call_id: inline_call_id(&tool.call.id),
        metadata_ref: None,
        assembly_complete: true,
        tool_id: inline_tool_id(&tool.call.tool_id),
        name: tool.call.name,
        state: ForegroundToolState::Assembled {
            arguments: String::new(),
        },
    }
}
fn inherited_terminal(
    provenance: crate::durable::reading::TurnReadingProvenance,
    cursor: RuntimeClientTranscriptCursor,
) -> Result<super::turn::TurnProcessView, String> {
    if provenance.id.attempt_id.as_str().len() > 512 {
        return Err("inherited turn requires its native owner field source".into());
    }
    Ok(super::turn::TurnProcessView {
        conversation_id: provenance.id.conversation_id,
        attempt_id: Some(provenance.id.attempt_id),
        attempt_id_source: None,
        event_id: None,
        control_cursor: cursor,
        final_message_id: None,
        final_message_locator: None,
        final_message_id_deferred: false,
        final_message_id_source: None,
        message_count: 0,
        tool_call_count: 0,
        outcome: provenance.outcome.into(),
        failure: None,
        started_at: provenance.started_at,
        ended_at: provenance.ended_at,
    })
}
/// Converts one live requested interaction audit envelope into its client
/// transcript view.
pub(crate) fn interaction_requested_view(
    event: RuntimeEventEnvelope,
) -> Result<RuntimeClientTranscriptInteractionRequested, String> {
    let RuntimeEventEnvelope {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        event:
            crate::events::types::RuntimeEvent::InteractionRequested {
                interaction_id,
                subject,
            },
        ..
    } = event
    else {
        return Err("interaction requested audit has an invalid envelope".to_owned());
    };
    validate_interaction_audit_scope(&interaction_id, attempt_id.as_ref(), turn_id.as_ref())?;
    Ok(RuntimeClientTranscriptInteractionRequested {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        interaction_id,
        subject,
    })
}
/// Converts one live settled interaction audit envelope into its client
/// transcript view.
pub(crate) fn interaction_settled_view(
    event: RuntimeEventEnvelope,
) -> Result<RuntimeClientTranscriptInteractionSettled, String> {
    let RuntimeEventEnvelope {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        event:
            crate::events::types::RuntimeEvent::InteractionSettled {
                interaction_id,
                settlement,
            },
        ..
    } = event
    else {
        return Err("interaction settled audit has an invalid envelope".to_owned());
    };
    validate_interaction_audit_scope(&interaction_id, attempt_id.as_ref(), turn_id.as_ref())?;
    Ok(RuntimeClientTranscriptInteractionSettled {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        interaction_id,
        settlement,
    })
}
// This validates display metadata from the required native journal; it grants
// no execution or response authority. Scheduled requests own their reserved ID.
fn validate_interaction_audit_scope(
    id: &InteractionId,
    attempt: Option<&AttemptId>,
    turn: Option<&TurnId>,
) -> Result<(), String> {
    let scheduled = id.as_str().strip_prefix("scheduled:");
    if attempt.is_some() && turn.is_some() && scheduled.is_none() {
        return Ok(());
    }
    if attempt.is_none()
        && turn.is_none()
        && let Some((start, ordinal)) = scheduled.and_then(|scope| scope.split_once(':'))
        && crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId::try_from(
            start.to_owned(),
        )
        .is_ok()
        && ordinal.parse::<u64>().is_ok_and(|value| value > 0)
    {
        return Ok(());
    }
    Err("interaction audit has an inconsistent native execution scope".into())
}

/// Positive native audit settlement when its retained content is deferred.
/// This is Publication Plane authority, not canonical C or execution settlement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientPublicationSettlement {
    pub stream_id: Option<crate::runtime::identity::PublicationStreamId>,
    pub message_id: Option<MessageId>,
    pub attempt_id: Option<AttemptId>,
    pub turn_id: Option<TurnId>,
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub kind: crate::publication::PublicationAuditKind,
    pub settled_at: chrono::DateTime<chrono::Utc>,
    pub block_count: u64,
}
/// Exact source placement and native cut of one positive publication settlement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientPublicationSettlementRecord {
    pub settlement: RuntimeClientPublicationSettlement,
    pub transcript_cursor: RuntimeClientTranscriptCursor,
    pub read_cut: crate::durable::observation::ObservationReadCut,
}
impl From<crate::durable::PublicationAuditSummary> for RuntimeClientPublicationSettlement {
    fn from(summary: crate::durable::PublicationAuditSummary) -> Self {
        Self {
            stream_id: summary.stream_id,
            message_id: summary.message_id,
            attempt_id: summary.attempt_id,
            turn_id: summary.turn_id,
            request_id: summary.request_id,
            kind: summary.kind,
            settled_at: summary.settled_at,
            block_count: summary.block_count,
        }
    }
}
