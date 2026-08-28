//! Shared source selection and bounded rendering for unresolved output.
//!
//! Publication Audit remains the only body authority. This module only
//! derives a source identity and constructs a bounded request-only value from
//! a keyed audit; it never creates a canonical message or a Tool Plane fact.

use crate::model::input::{
    CARRYOVER_RENDER_HEADER, CARRYOVER_SOURCE_SETTLEMENT_PREFIX,
    CARRYOVER_SOURCE_SETTLEMENT_SUFFIX, CarryoverBlockKind, CarryoverOmissionCounts,
    RenderedCarryoverRecord, RenderedCarryoverText, RenderedCarryoverToolCall,
    RenderedUnresolvedOutputCarryover, UnresolvedOutputSettlement, render_carryover_record,
};
use crate::model::snapshot::RequestIdentity;
use crate::publication::{PublicationAudit, PublicationAuditBlock, PublicationAuditKind};
use crate::runtime::identity::PublicationStreamId;

/// The maximum UTF-8 bytes admitted for one complete carryover rendering.
pub const MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES: usize = 4_096;
/// The maximum UTF-8 bytes retained from one textual audit block.
pub const MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES: usize = 2_048;
/// The maximum UTF-8 bytes retained for one tool-proposal argument body.
pub const MAX_CARRYOVER_TOOL_ARGUMENT_BYTES: usize = 512;

/// Selects the one highest-ordinal eligible audit of the unresolved logical
/// model step.
///
/// Only keyed, identity-derived reads are performed. The caller supplies the
/// durable audit lookup, so the same function is used by live settlement and
/// crash recovery without either path scanning publication history.
///
/// # Errors
///
/// Returns the error from the keyed publication-audit lookup.
pub fn select_unresolved_output_source<E>(
    last_started: &RequestIdentity,
    mut load: impl FnMut(&PublicationStreamId) -> Result<Option<PublicationAudit>, E>,
) -> Result<Option<PublicationStreamId>, E> {
    for retry_number in (0..=last_started.retry_number).rev() {
        let candidate = RequestIdentity {
            attempt_id: last_started.attempt_id.clone(),
            turn: last_started.turn.clone(),
            retry_number,
        };
        let stream_id = PublicationStreamId::for_request(
            &candidate.attempt_id,
            &candidate.provisional_message_id(),
        );
        let Some(audit) = load(&stream_id)? else {
            continue;
        };
        if audit.stream_id != stream_id
            || audit.attempt_id != candidate.attempt_id
            || audit.turn_id != candidate.turn
            || audit.request_id != candidate.request_id()
            || audit.message_id != candidate.provisional_message_id()
            || !matches!(
                audit.kind,
                PublicationAuditKind::Incomplete | PublicationAuditKind::Unaccepted
            )
        {
            continue;
        }
        if render_unresolved_output_carryover(&audit).is_some() {
            return Ok(Some(stream_id));
        }
    }
    Ok(None)
}

/// Renders one eligible audit into a bounded request-only value.
///
/// Text is retained from the interruption-adjacent tail and is escaped as a
/// JSON string. Tool proposals retain their completion state and are always
/// marked unaccepted/not executed; oversized argument bodies are omitted in
/// full rather than cut into an executable-looking JSON fragment. The source
/// audit settlement is converted once into the model-input-owned semantic and
/// remains visible in every retained representation. Final admission is
/// whole-record and newest-first, then the admitted records are restored to
/// audit order.
#[must_use]
pub fn render_unresolved_output_carryover(
    audit: &PublicationAudit,
) -> Option<RenderedUnresolvedOutputCarryover> {
    let source_settlement = match audit.kind {
        PublicationAuditKind::Incomplete => UnresolvedOutputSettlement::Incomplete,
        PublicationAuditKind::Unaccepted => UnresolvedOutputSettlement::Unaccepted,
    };

    let candidates: Vec<(CarryoverBlockKind, RenderedCarryoverRecord)> =
        audit.content.iter().filter_map(render_block).collect();
    if candidates.is_empty() {
        return None;
    }

    let mut admitted = vec![false; candidates.len()];
    let mut omitted_blocks = CarryoverOmissionCounts::default();
    for (kind, _) in &candidates {
        omitted_blocks.increment(*kind);
    }
    let mut admitted_bytes = CARRYOVER_RENDER_HEADER
        .len()
        .saturating_add(CARRYOVER_SOURCE_SETTLEMENT_PREFIX.len())
        .saturating_add(source_settlement.as_str().len())
        .saturating_add(CARRYOVER_SOURCE_SETTLEMENT_SUFFIX.len());
    for index in (0..candidates.len()).rev() {
        let (kind, record) = &candidates[index];
        let record_bytes = render_carryover_record(record).len().saturating_add(1);
        let candidate_omitted = omitted_blocks.without_one(*kind);
        let footer_bytes = omission_footer(&candidate_omitted).len();
        if admitted_bytes
            .saturating_add(record_bytes)
            .saturating_add(footer_bytes)
            <= MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES
        {
            admitted[index] = true;
            admitted_bytes = admitted_bytes.saturating_add(record_bytes);
            omitted_blocks = candidate_omitted;
        }
    }
    let records = candidates
        .into_iter()
        .zip(admitted)
        .filter_map(|((_, record), admitted)| admitted.then_some(record))
        .collect::<Vec<_>>();
    if records.is_empty() {
        return None;
    }
    let carryover = RenderedUnresolvedOutputCarryover {
        source_stream_id: audit.stream_id.clone(),
        source_settlement,
        records,
        omitted_blocks,
    };
    (carryover.rendered_bytes() <= MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES).then_some(carryover)
}

fn render_block(
    block: &PublicationAuditBlock,
) -> Option<(CarryoverBlockKind, RenderedCarryoverRecord)> {
    match block {
        PublicationAuditBlock::Text { text, .. } if !text.is_empty() => Some((
            CarryoverBlockKind::Text,
            RenderedCarryoverRecord::Text(bound_text(CarryoverBlockKind::Text, text)),
        )),
        PublicationAuditBlock::Reasoning { text, .. } if !text.is_empty() => Some((
            CarryoverBlockKind::Reasoning,
            RenderedCarryoverRecord::Text(bound_text(CarryoverBlockKind::Reasoning, text)),
        )),
        PublicationAuditBlock::Refusal { text, .. } if !text.is_empty() => Some((
            CarryoverBlockKind::Refusal,
            RenderedCarryoverRecord::Text(bound_text(CarryoverBlockKind::Refusal, text)),
        )),
        PublicationAuditBlock::ProposedToolCall {
            call_id,
            tool_id,
            name,
            arguments,
            complete,
            ..
        } => {
            let (arguments, omitted_argument_bytes) =
                if arguments.len() <= MAX_CARRYOVER_TOOL_ARGUMENT_BYTES {
                    (Some(arguments.clone()), 0)
                } else {
                    (None, arguments.len())
                };
            Some((
                CarryoverBlockKind::ProposedToolCall,
                RenderedCarryoverRecord::ProposedToolCall(RenderedCarryoverToolCall {
                    call_id: call_id.clone(),
                    tool_id: tool_id.clone(),
                    name: name.clone(),
                    complete: *complete,
                    arguments,
                    omitted_argument_bytes,
                }),
            ))
        }
        _ => None,
    }
}

fn bound_text(kind: CarryoverBlockKind, text: &str) -> RenderedCarryoverText {
    let (tail, omitted_prefix_bytes) = tail_excerpt(text, MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES);
    RenderedCarryoverText {
        kind,
        text: Some(tail),
        omitted_prefix_bytes,
        omitted_detail_bytes: 0,
    }
}

fn tail_excerpt(text: &str, max_bytes: usize) -> (String, usize) {
    if text.len() <= max_bytes {
        return (text.to_owned(), 0);
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    (text[start..].to_owned(), start)
}

fn omission_footer(omitted: &CarryoverOmissionCounts) -> String {
    format!(
        "[carryover omitted blocks text={} reasoning={} refusal={} proposed_tool_call={}]",
        omitted.text, omitted.reasoning, omitted.refusal, omitted.proposed_tool_call
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        CarryoverBlockKind, MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES, MAX_CARRYOVER_TOOL_ARGUMENT_BYTES,
        MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES, RenderedCarryoverRecord,
        render_unresolved_output_carryover, select_unresolved_output_source,
    };
    use crate::message::types::ContentBlockIndex;
    use crate::model::snapshot::RequestIdentity;
    use crate::model::{CarryoverDetailLevel, UnresolvedOutputSettlement};
    use crate::publication::{PublicationAudit, PublicationAuditBlock, PublicationAuditKind};
    use crate::runtime::identity::{
        AttemptId, MessageId, PublicationStreamId, RequestId, ToolCallId, ToolId, TurnId,
    };

    fn audit(identity: &RequestIdentity, content: Vec<PublicationAuditBlock>) -> PublicationAudit {
        audit_with_kind(identity, PublicationAuditKind::Incomplete, content)
    }

    fn audit_with_kind(
        identity: &RequestIdentity,
        kind: PublicationAuditKind,
        content: Vec<PublicationAuditBlock>,
    ) -> PublicationAudit {
        PublicationAudit {
            stream_id: PublicationStreamId::for_request(
                &identity.attempt_id,
                &identity.provisional_message_id(),
            ),
            attempt_id: identity.attempt_id.clone(),
            turn_id: identity.turn.clone(),
            request_id: identity.request_id(),
            message_id: identity.provisional_message_id(),
            kind,
            content,
            settled_at: Utc::now(),
        }
    }

    #[test]
    fn selector_walks_descending_ordinals_and_falls_back_from_empty_latest() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt"),
            turn: TurnId::new("1"),
            retry_number: 2,
        };
        let earlier = audit(
            &RequestIdentity {
                retry_number: 1,
                ..identity.clone()
            },
            vec![PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "partial".to_owned(),
            }],
        );
        let latest_empty = audit(&identity, Vec::new());
        let mut seen = Vec::new();
        let selected = select_unresolved_output_source(&identity, |stream| {
            seen.push(stream.clone());
            if *stream == latest_empty.stream_id {
                Ok::<_, ()>(Some(latest_empty.clone()))
            } else if *stream == earlier.stream_id {
                Ok::<_, ()>(Some(earlier.clone()))
            } else {
                Ok(None)
            }
        })
        .expect("selector");
        assert_eq!(selected, Some(earlier.stream_id.clone()));
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0],
            PublicationStreamId::for_request(
                &identity.attempt_id,
                &identity.provisional_message_id()
            )
        );
    }

    #[test]
    fn selector_ignores_wrong_identity_and_ineligible_audits() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-identity"),
            turn: TurnId::new("turn"),
            retry_number: 2,
        };
        let valid = audit(
            &RequestIdentity {
                retry_number: 0,
                ..identity.clone()
            },
            vec![PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "earlier".to_owned(),
            }],
        );
        let mut wrong_request = audit(
            &identity,
            vec![PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "wrong request".to_owned(),
            }],
        );
        wrong_request.request_id = RequestId::new("not-derived");
        let mut calls = 0;
        let selected = select_unresolved_output_source(&identity, |stream| {
            calls += 1;
            if *stream
                == PublicationStreamId::for_request(
                    &identity.attempt_id,
                    &identity.provisional_message_id(),
                )
            {
                Ok::<_, ()>(Some(wrong_request.clone()))
            } else if *stream == valid.stream_id {
                Ok(Some(valid.clone()))
            } else {
                Ok(None)
            }
        })
        .expect("selector");
        assert_eq!(selected, Some(valid.stream_id));
        assert_eq!(calls, 3, "the selector checks exactly N, N-1, ..., 0");
    }

    #[test]
    fn text_tail_and_tool_argument_bounds_are_utf8_safe() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let text = "🙂".repeat(MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES);
        let arguments = "x".repeat(MAX_CARRYOVER_TOOL_ARGUMENT_BYTES + 1);
        let carryover = render_unresolved_output_carryover(&audit(
            &identity,
            vec![
                PublicationAuditBlock::Text {
                    block_index: ContentBlockIndex::new(0),
                    text,
                },
                PublicationAuditBlock::ProposedToolCall {
                    block_index: ContentBlockIndex::new(1),
                    call_id: ToolCallId::new("call"),
                    tool_id: ToolId::new("tool"),
                    name: "name".to_owned(),
                    arguments,
                    complete: true,
                },
            ],
        ))
        .expect("meaningful carryover");
        assert!(carryover.rendered_bytes() <= MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES);
        assert!(carryover.records.iter().any(|record| matches!(
            record,
            crate::model::input::RenderedCarryoverRecord::ProposedToolCall(call)
                if call.arguments.is_none() && call.omitted_argument_bytes > MAX_CARRYOVER_TOOL_ARGUMENT_BYTES
        )));
    }

    #[test]
    fn reasoning_refusal_and_tool_proposals_keep_their_noncanonical_status() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-kinds"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let carryover = render_unresolved_output_carryover(&audit_with_kind(
            &identity,
            PublicationAuditKind::Unaccepted,
            vec![
                PublicationAuditBlock::Text {
                    block_index: ContentBlockIndex::new(0),
                    text: "answer".to_owned(),
                },
                PublicationAuditBlock::Reasoning {
                    block_index: ContentBlockIndex::new(1),
                    text: "thinking aloud".to_owned(),
                },
                PublicationAuditBlock::Refusal {
                    block_index: ContentBlockIndex::new(2),
                    text: "cannot comply".to_owned(),
                },
                PublicationAuditBlock::ProposedToolCall {
                    block_index: ContentBlockIndex::new(3),
                    call_id: ToolCallId::new("call-incomplete"),
                    tool_id: ToolId::new("tool"),
                    name: "lookup".to_owned(),
                    arguments: "{\"q\":\"x\"}".to_owned(),
                    complete: false,
                },
                PublicationAuditBlock::ProposedToolCall {
                    block_index: ContentBlockIndex::new(4),
                    call_id: ToolCallId::new("call-complete"),
                    tool_id: ToolId::new("tool"),
                    name: "lookup".to_owned(),
                    arguments: "{}".to_owned(),
                    complete: true,
                },
            ],
        ))
        .expect("all output kinds are meaningful carryover");
        assert_eq!(carryover.records.len(), 5);
        assert!(carryover.render().contains("kind=reasoning narration"));
        assert!(carryover.render().contains("kind=refusal"));
        assert!(
            carryover
                .render()
                .contains("status=incomplete proposal;unaccepted;not_executed")
        );
        assert!(
            carryover
                .render()
                .contains("status=complete proposal;unaccepted;not_executed")
        );
        assert!(carryover.render().contains("\\\"q\\\":\\\"x\\\""));
    }

    #[test]
    fn source_settlement_survives_rendering_and_metadata_only_degradation() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-settlement"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let content: Vec<PublicationAuditBlock> = (0..3)
            .map(|index| PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(index),
                text: format!("same-content-{index}-{}", "x".repeat(1_600)),
            })
            .collect();
        let incomplete = render_unresolved_output_carryover(&audit_with_kind(
            &identity,
            PublicationAuditKind::Incomplete,
            content.clone(),
        ))
        .expect("incomplete audit has meaningful carryover");
        let unaccepted = render_unresolved_output_carryover(&audit_with_kind(
            &identity,
            PublicationAuditKind::Unaccepted,
            content,
        ))
        .expect("unaccepted audit has meaningful carryover");

        assert_eq!(incomplete.records, unaccepted.records);
        assert_eq!(incomplete.omitted_blocks, unaccepted.omitted_blocks);
        assert!(incomplete.omitted_blocks.text > 0);
        assert_eq!(
            incomplete.source_settlement,
            UnresolvedOutputSettlement::Incomplete
        );
        assert_eq!(
            unaccepted.source_settlement,
            UnresolvedOutputSettlement::Unaccepted
        );
        assert!(incomplete.render().contains("source_settlement=incomplete"));
        assert!(!incomplete.render().contains("source_settlement=unaccepted"));
        assert!(unaccepted.render().contains("source_settlement=unaccepted"));
        assert!(!unaccepted.render().contains("source_settlement=incomplete"));

        let incomplete_reduced = incomplete.degraded(CarryoverDetailLevel::Reduced);
        let incomplete_metadata = incomplete_reduced.degraded(CarryoverDetailLevel::MetadataOnly);
        let unaccepted_reduced = unaccepted.degraded(CarryoverDetailLevel::Reduced);
        let unaccepted_metadata = unaccepted_reduced.degraded(CarryoverDetailLevel::MetadataOnly);
        for (reduced, metadata, settlement, full) in [
            (
                &incomplete_reduced,
                &incomplete_metadata,
                UnresolvedOutputSettlement::Incomplete,
                &incomplete,
            ),
            (
                &unaccepted_reduced,
                &unaccepted_metadata,
                UnresolvedOutputSettlement::Unaccepted,
                &unaccepted,
            ),
        ] {
            assert_eq!(reduced.source_settlement, settlement);
            assert!(
                reduced
                    .render()
                    .contains(&format!("source_settlement={}", settlement.as_str()))
            );
            assert_eq!(metadata.source_settlement, settlement);
            assert_eq!(metadata.omitted_blocks, full.omitted_blocks);
            assert!(
                metadata
                    .records
                    .iter()
                    .all(|record| record.block_kind() == CarryoverBlockKind::Text)
            );
            assert!(metadata.records.iter().all(|record| match record {
                RenderedCarryoverRecord::Text(text) => text.text.is_none(),
                RenderedCarryoverRecord::ProposedToolCall(_) => false,
            }));
            assert!(
                metadata
                    .render()
                    .contains(&format!("source_settlement={}", settlement.as_str()))
            );
        }
    }

    #[test]
    fn final_admission_is_newest_first_but_restores_audit_order() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-admission"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let carryover = render_unresolved_output_carryover(&audit(
            &identity,
            (0..3)
                .map(|index| PublicationAuditBlock::Text {
                    block_index: ContentBlockIndex::new(index),
                    text: format!("block-{index}-{}", "x".repeat(1_600)),
                })
                .collect(),
        ))
        .expect("at least the newest bounded record fits");
        assert!(carryover.rendered_bytes() <= MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES);
        assert_eq!(carryover.omitted_blocks.text, 1);
        let retained: Vec<&str> = carryover
            .records
            .iter()
            .filter_map(|record| match record {
                crate::model::input::RenderedCarryoverRecord::Text(text) => text.text.as_deref(),
                crate::model::input::RenderedCarryoverRecord::ProposedToolCall(_) => None,
            })
            .collect();
        assert_eq!(retained.len(), 2);
        assert!(retained[0].starts_with("block-1-"));
        assert!(retained[1].starts_with("block-2-"));
    }

    #[test]
    fn tail_excerpt_reports_omitted_utf8_prefix_bytes() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-utf8"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let text = format!("{}é", "🙂".repeat(512));
        let carryover = render_unresolved_output_carryover(&audit(
            &identity,
            vec![PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text,
            }],
        ))
        .expect("UTF-8 text is meaningful");
        let crate::model::input::RenderedCarryoverRecord::Text(text) = &carryover.records[0] else {
            panic!("text record expected");
        };
        assert_eq!(text.omitted_prefix_bytes, 4);
        assert!(
            text.text
                .as_deref()
                .is_some_and(|tail| tail.starts_with('🙂'))
        );
        assert!(
            text.text
                .as_deref()
                .is_some_and(|tail| tail.len() <= MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES)
        );
    }

    #[test]
    fn empty_audit_never_produces_carryover() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt-empty"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        assert!(render_unresolved_output_carryover(&audit(&identity, Vec::new())).is_none());
    }

    #[test]
    fn canonical_message_identity_is_not_used_by_carryover() {
        let identity = RequestIdentity {
            attempt_id: AttemptId::new("attempt"),
            turn: TurnId::new("1"),
            retry_number: 0,
        };
        let carryover = render_unresolved_output_carryover(&audit(
            &identity,
            vec![PublicationAuditBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "x".to_owned(),
            }],
        ))
        .expect("carryover");
        assert_eq!(carryover.source_stream_id, identity_stream(&identity));
        assert_ne!(
            carryover.source_stream_id.as_str(),
            MessageId::new("x").as_str()
        );
        let _ = RequestId::new("proof-only");
    }

    fn identity_stream(identity: &RequestIdentity) -> PublicationStreamId {
        PublicationStreamId::for_request(&identity.attempt_id, &identity.provisional_message_id())
    }
}
