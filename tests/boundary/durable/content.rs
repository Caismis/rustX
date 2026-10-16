//! T08/T20: physical work is counted at the actual SQLite BLOB row handoff.
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime_client::content::{ContentRepresentation, ContentSource};
#[test]
fn t08_t15_native_schema_initializes_all_bounded_read_indexes_before_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-schema.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8b1");
    // Actual native construction executes the schema transaction. An index
    // declaration preceding its table must fail here, before any RPC can
    // claim SessionCreated or before initialize can write its native seed.
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    for name in [
        "events_observation_statistics_idx",
        "events_observation_tool_idx",
        "events_observation_members_idx",
        "events_observation_execution_idx",
    ] {
        let table: String = connection
            .query_row(
                "SELECT tbl_name FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table, "events");
    }
    drop(connection);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(reopened.observation_read_cut().unwrap().ledger_through, 0);
    assert_eq!(reopened.observation_statistics(0).unwrap().turns, 0);
}

#[test]
fn t08_t20_content_range_reads_touch_at_most_two_finite_sqlite_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("range.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a2");
    let message_id = MessageId::new("large-message");
    let bytes = "x".repeat(16 * 1024 * 1024);
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .initialize(&[MessageBlock::Assistant(AssistantMessageBlock {
                id: message_id.clone(),
                content: vec![AssistantContentBlock::Text(TextBlock { text: bytes })],
            })])
            .unwrap();
    }
    // Deliberately make the whole-message encoding undecodable. Range reads
    // must use only the transaction-owned byte index and identity, proving
    // they cannot deserialize the enormous body as hidden preliminary work.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE message_ledger SET message_json='invalid JSON' WHERE message_id=?1",
            [message_id.as_str()],
        )
        .unwrap();
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::Payload,
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                block_index: ContentBlockIndex::new(0),
            },
            ContentRepresentation::Utf8Text,
        )
        .unwrap()
        .unwrap();
    for offset in [0, 1, 65_535, 8_000_001, reference.readable_end - 1] {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        let (rows, materialized_bytes) = SqliteConversationStore::content_read_work();
        assert!((1..=2).contains(&rows));
        assert!(materialized_bytes <= 131_072);
        assert_eq!(
            chunk.end_offset,
            (offset + 65_536).min(reference.readable_end)
        );
    }
    let raw = rusqlite::Connection::open(&path).unwrap();
    let plan:String=raw.query_row("EXPLAIN QUERY PLAN SELECT segment,bytes FROM content_segments WHERE source_kind='canonical' AND source_id='large-message' AND block_index=0 AND representation='utf8_text' AND segment>=1 AND segment<=2 ORDER BY segment LIMIT 2",[],|r|r.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH content_segments USING PRIMARY KEY"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN"));
    // Retention remains the native Ledger owner. An orphan byte index never
    // permits reads after its authoritative source has been removed.
    assert!(
        raw.execute(
            "DELETE FROM message_ledger WHERE message_id='large-message'",
            []
        )
        .is_err(),
        "native descriptor foreign keys protect retained canonical authority"
    );
    // Deliberately corrupt retention through a separate diagnostic connection.
    // Native foreign keys prevent this during normal operation; even damaged
    // retained layouts must not let an orphan index authorize source bytes.
    raw.pragma_update(None, "foreign_keys", false).unwrap();
    raw.execute(
        "DELETE FROM message_ledger WHERE message_id='large-message'",
        [],
    )
    .unwrap();
    assert!(store.content_read(&reference, 0, 1).is_err());
}

#[test]
fn t15_t20_message_and_transcript_headers_advance_without_source_body_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("headers.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a3");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let messages: Vec<_> = (0..73)
        .map(|index| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("header-{index}")),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: if index == 72 {
                        "λ🙂".repeat(400_000)
                    } else {
                        format!("body-{index}")
                    },
                })],
            })
        })
        .collect();
    store.initialize(&messages).unwrap();
    let captured = store.observation_read_cut().unwrap();
    assert_eq!(captured.ledger_through, 73);
    drop(store);
    // Every source body is now undecodable; read success proves neither the
    // snapshot/Surface bootstrap nor a hidden canonical-body load is used.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    connection
        .execute("UPDATE surface_head SET active_json='invalid JSON'", [])
        .unwrap();
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let tail = store.message_header_page(&captured, None, 32).unwrap();
    assert_eq!(tail.offset, 41);
    assert_eq!(tail.headers.len(), 32);
    assert_eq!(tail.total, 73);
    assert_eq!(
        tail.headers
            .last()
            .unwrap()
            .identity
            .as_ref()
            .unwrap()
            .as_str(),
        "header-72"
    );
    let last = tail.headers.last().unwrap();
    let payload = last
        .content
        .iter()
        .find(|binding| {
            matches!(
                binding.content_ref.source,
                ContentSource::CanonicalMessageBlock {
                    component: crate::runtime_client::content::CanonicalContentComponent::Payload,
                    ..
                }
            )
        })
        .expect("exact native payload locator");
    assert_eq!(payload.content_ref.readable_end, 2_400_000);
    assert!(
        last.content.iter().any(|binding| matches!(
            binding.content_ref.source,
            ContentSource::CanonicalMessageBlock {
                component:
                    crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
                ..
            }
        )),
        "original metadata stays separately readable"
    );
    assert!(last.deferred);
    let mut offset = Some(0);
    let mut identities = Vec::new();
    while let Some(at) = offset {
        let page = store.message_header_page(&captured, Some(at), 32).unwrap();
        assert!(page.headers.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 786_432);
        assert!(page.next_offset.is_none_or(|next| next > at));
        identities.extend(
            page.headers
                .into_iter()
                .map(|header| header.identity.unwrap()),
        );
        offset = page.next_offset;
    }
    assert_eq!(identities.len(), 73);
    for (index, id) in identities.iter().enumerate() {
        assert_eq!(id.as_str(), format!("header-{index}"));
    }
    let mut before = None;
    let mut positions = Vec::new();
    loop {
        let page = store.transcript_header_page(&captured, before, 32).unwrap();
        assert!(page.headers.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 786_432);
        positions.extend(page.headers.iter().map(|header| header.cursor.get()));
        if let Some(next) = page.next_cursor {
            assert!(before.is_none_or(|prior| next < prior));
            before = Some(next);
        } else {
            break;
        }
    }
    positions.sort_unstable();
    assert_eq!(positions, (1..=73).collect::<Vec<_>>());
    let exact = store
        .read_window_headers(
            &crate::durable::reading::ConversationWindowAt::Message {
                id: MessageId::new("header-72"),
                cut: Some(captured.read.clone()),
            },
            32,
        )
        .unwrap();
    assert_eq!(exact.target_cursor.unwrap().get(), 73);
    assert_eq!(exact.page.headers.len(), 1);
    assert_eq!(exact.page.headers[0].reference_id, "header-72");
    assert_eq!(exact.cut.ledger_through, captured.ledger_through);
    let newer = store
        .read_window_headers(
            &crate::durable::reading::ConversationWindowAt::Newer {
                after: crate::durable::TranscriptCursor::new(50),
                cut: captured.read.clone(),
            },
            32,
        )
        .unwrap();
    assert_eq!(newer.page.headers.len(), 23);
    assert_eq!(newer.page.headers.first().unwrap().cursor.get(), 51);
    assert_eq!(newer.page.headers.last().unwrap().cursor.get(), 73);
    assert!(newer.newer_cursor.is_none());

    // Physical schema enforcement prevents an oversized derived descriptor
    // from defeating the bounded query even if a corrupt writer is attempted.
    assert!(
        connection
            .execute(
                "UPDATE observation_headers SET header_json=zeroblob(16385)",
                []
            )
            .is_err()
    );
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT h.header_json FROM message_ledger m CROSS JOIN observation_headers h WHERE m.position>41 AND m.position<=73 AND h.reference_kind='message' AND h.reference_id=m.message_id ORDER BY m.position LIMIT 32", [], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH m USING COVERING INDEX message_ledger_identity_position_idx"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN"));
    assert!(store.message_header_page(&captured, Some(74), 32).is_err());
    assert!(store.message_header_page(&captured, None, 33).is_err());
}

#[test]
fn t08_t15_tool_result_headers_never_deserialize_historical_result_json() {
    use crate::message::types::{ToolCallOccurrenceRef, ToolMessageBlock};
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::types::{
        ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tool-header.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a4");
    let assistant_id = MessageId::new("tool-owner");
    let call_id = ToolCallId::new("tool-call");
    let tool_id = ToolId::new("native-tool");
    let messages = vec![
        MessageBlock::Assistant(AssistantMessageBlock {
            id: assistant_id.clone(),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: "native".into(),
                arguments: serde_json::json!({"argument":"a".repeat(2_000_000)}),
            })],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("tool-result"),
            occurrence: ToolCallOccurrenceRef::new(assistant_id, ContentBlockIndex::new(0)),
            tool_call_id: call_id,
            tool_id,
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Json {
                    value: serde_json::json!({"result":"r".repeat(4_000_000)}),
                }],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
                deliveries: Vec::new(),
            },
        }),
    ];
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&messages).unwrap();
    let cut = store.observation_read_cut().unwrap();
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let page = store.transcript_header_page(&cut, None, 32).unwrap();
    assert_eq!(page.headers.len(), 2);
    let associations = store
        .tool_association_headers(&cut, &MessageId::new("tool-owner"), 0, 32)
        .unwrap();
    assert_eq!(associations.total, 1);
    assert_eq!(associations.headers.len(), 1);
    assert_eq!(
        associations.headers[0]
            .result
            .as_ref()
            .unwrap()
            .identity
            .as_ref()
            .unwrap()
            .as_str(),
        "tool-result"
    );
    assert_eq!(
        associations.headers[0].result.as_ref().unwrap().content[0]
            .content_ref
            .readable_end,
        4_000_013
    );
    assert!(serde_json::to_vec(&associations).unwrap().len() < 32_768);

    let result = &page.headers[1];
    assert_eq!(result.reference_id, "tool-result");
    assert!(matches!(
        result.display.as_ref().unwrap().item,
        crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Tool(_)
        }
    ));
    assert_eq!(
        result.content.len(),
        2,
        "deferred payload and exact original MessageMetadata are both recoverable"
    );
    assert!(result.content.iter().any(|binding| matches!(
        binding.content_ref.source,
        ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
            ..
        }
    )));
    assert!(result.deferred);
    assert_eq!(result.content[0].content_ref.readable_end, 4_000_013);
    let chunk = store
        .content_read(&result.content[0].content_ref, 0, 65_536)
        .unwrap();
    assert_eq!(chunk.end_offset, 65_536);
    assert!(chunk.next_offset.is_some());
    assert!(serde_json::to_vec(&page).unwrap().len() < 32_768);
}

#[test]
fn t15_t19_canonical_block_pages_advance_and_binary_does_not_alias_artifacts() {
    use crate::message::content::ImageReference;
    use crate::runtime::identity::ArtifactId;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("block-pages.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a5");
    let id = MessageId::new("many-blocks");
    let mut content: Vec<_> = (0..71)
        .map(|index| {
            AssistantContentBlock::Text(TextBlock {
                text: format!("block-{index}"),
            })
        })
        .collect();
    content.push(AssistantContentBlock::Image(ImageReference {
        artifact_id: ArtifactId::new("binary-artifact-identity"),
        alt: Some("image metadata".into()),
    }));
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store
        .initialize(&[MessageBlock::Assistant(AssistantMessageBlock {
            id: id.clone(),
            content,
        })])
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let mut offset = 0;
    let mut blocks = Vec::new();
    loop {
        let page = store
            .message_block_header_page(&cut, &id, offset, 32)
            .unwrap();
        assert_eq!(page.total, 72);
        assert!(page.headers.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
        blocks.extend(page.headers);
        match page.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(blocks.len(), 72);
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(block.block_index.get() as usize, index);
    }
    let image = &blocks[71];
    assert_eq!(image.kind, "image");
    assert_eq!(image.content.len(), 1);
    assert_eq!(
        image.content[0].content_ref.representation,
        ContentRepresentation::CanonicalJsonV1
    );
    // Metadata for this absent artifact is readable; a client cannot turn its
    // unresolved reference into retained BinaryBytes by changing representation.
    let mut forged = image.content[0].content_ref.clone();
    forged.representation = ContentRepresentation::BinaryBytes;
    assert!(store.content_read(&forged, 0, 65_536).is_err());
    let mut text_forged = blocks[0].content[0].content_ref.clone();
    text_forged.representation = ContentRepresentation::BinaryBytes;
    assert!(store.content_read(&text_forged, 0, 65_536).is_err());
    assert!(
        store
            .content_descriptor(&forged.source, ContentRepresentation::BinaryBytes)
            .unwrap()
            .is_none()
    );
    assert!(store.message_block_header_page(&cut, &id, 73, 32).is_err());
}

#[test]
fn t15_t20_journal_family_headers_use_indexed_finite_native_columns() {
    use crate::events::types::{
        AttemptFailure, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope,
    };
    use crate::runtime::identity::{AttemptId, EventId};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal-header.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a6");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let timestamp = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let envelope = |index, attempt: AttemptId, event| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!("journal-{index}")),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(attempt),
        turn_id: None,
        timestamp,
        event,
    };
    for index in 0..41 {
        let attempt = AttemptId::new(format!("attempt-{index}"));
        store
            .append_event(envelope(
                index,
                attempt.clone(),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt,
                },
            ))
            .unwrap();
    }
    let start_cut = store.observation_read_cut().unwrap().read.journal;
    let before = store.observation_statistics(start_cut).unwrap();
    assert_eq!(before.turns, 41);
    assert_eq!(before.model_requests, 0);
    assert_eq!(
        before.latest_turn.as_ref().unwrap().attempt_id,
        Some(AttemptId::new("attempt-40"))
    );
    assert!(!before.latest_turn.as_ref().unwrap().attempt_id_deferred);
    assert!(
        before
            .latest_turn
            .as_ref()
            .unwrap()
            .attempt_id_source
            .is_none()
    );
    assert!(before.latest_turn.as_ref().unwrap().ended_at.is_none());
    let attempt = AttemptId::new("attempt-40");
    store
        .append_event(envelope(
            41,
            attempt.clone(),
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: AttemptFailure::Runtime {
                    error: crate::runtime::RuntimeError::Internal {
                        message: "diagnostic".repeat(300_000),
                    },
                },
            },
        ))
        .unwrap();
    let through = store.observation_read_cut().unwrap().read.journal;
    drop(store);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let after = store.observation_statistics(through).unwrap();
    assert_eq!(after.turns, before.turns);
    assert_eq!(
        after.latest_turn.as_ref().unwrap().ended_at,
        Some(timestamp)
    );
    assert_eq!(store.observation_statistics(start_cut).unwrap(), before);
    assert!(store.observation_statistics(through + 1).is_err());
    let summary = store
        .observation_attempt_summary(&attempt, through)
        .unwrap();
    assert_eq!(summary.terminal_kind.as_deref(), Some("attempt_failed"));
    assert_eq!(summary.ended_at, Some(timestamp));
    assert!(summary.failure_deferred);
    assert!(
        summary.failure.is_some(),
        "a giant diagnostic retains exact failure authority"
    );
    assert!(summary.failure_readable_end > 2_000_000);
    assert!(serde_json::to_vec(&summary).unwrap().len() < 16_384);
    assert!(
        store
            .observation_attempt_summary(&attempt, start_cut)
            .unwrap()
            .failure
            .is_none()
    );
    let mut offset = 0;
    let mut bytes = Vec::new();
    while offset < summary.failure_readable_end {
        let chunk = store
            .observation_failure_value_read(&attempt, through, offset, 65_536)
            .unwrap();
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
        assert!(chunk.end_offset > offset);
        bytes.extend_from_slice(&chunk.bytes);
        offset = chunk.end_offset;
    }
    let failure: crate::runtime_client::RuntimeClientAttemptFailure =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        failure,
        crate::runtime_client::RuntimeClientAttemptFailure::Runtime {
            error: crate::runtime::RuntimeError::Internal {
                message: "diagnostic".repeat(300_000)
            },
        }
    );
    let headers = store
        .latest_observation_events(through, &["attempt_started".into()], None, 32)
        .unwrap();
    assert_eq!(headers.len(), 32);
    assert_eq!(headers.first().unwrap().sequence, 10);
    assert_eq!(headers.last().unwrap().sequence, 41);
    assert!(
        headers
            .iter()
            .all(|header| header.display.is_some() && !header.deferred)
    );
    assert!(serde_json::to_vec(&headers).unwrap().len() < 786_432);
    let failed = store
        .latest_observation_events(through, &["attempt_failed".into()], Some(&attempt), 1)
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].sequence, 42);
    assert!(failed[0].deferred);
    assert!(failed[0].display.is_none());
    assert_eq!(failed[0].attempt_id, Some(attempt));
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert!(
        connection
            .execute("UPDATE events SET observation_json=zeroblob(16385)", [])
            .is_err()
    );
    let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT sequence,event_id,attempt_id,turn_id,observation_json FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')='attempt_started' AND sequence<=42 AND sequence<41 ORDER BY sequence DESC LIMIT 1",[],|row|row.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH events USING INDEX events_kind_idx"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN"));
    let stats_plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT observation_statistics FROM events INDEXED BY events_observation_statistics_idx WHERE observation_statistics IS NOT NULL AND sequence<=42 ORDER BY sequence DESC LIMIT 1",[],|row|row.get(3)).unwrap();
    assert!(
        stats_plan.contains("SEARCH events USING INDEX events_observation_statistics_idx"),
        "{stats_plan}"
    );
    assert!(!stats_plan.contains("SCAN"));
    assert!(
        connection
            .execute(
                "UPDATE events SET observation_statistics=zeroblob(16385)",
                []
            )
            .is_err()
    );
    assert!(
        store
            .latest_observation_events(
                through,
                &["attempt_started".into(), "attempt_started".into()],
                None,
                32
            )
            .is_err()
    );
}

#[test]
fn t09_t12_pending_edits_never_alias_immutable_canonical_content() {
    use crate::durable::inbox::{InboundDraft, PendingInboundRef, PendingMutationOutcome};
    use crate::message::types::{InboundKind, UserContentBlock, UserSource};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending-content.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a7");
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    store.initialize(&[]).unwrap();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "pending".repeat(400_000),
            })],
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    let canonical = ContentSource::CanonicalMessageBlock {
        component: crate::runtime_client::content::CanonicalContentComponent::Payload,
        locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: accepted.message_id.clone(),
        },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(
        store
            .content_descriptor(&canonical, ContentRepresentation::Utf8Text)
            .is_err()
    );
    let before = store.observation_read_cut().unwrap();
    assert_eq!(before.ledger_through, 0);
    assert_eq!(before.transcript_total, 1);
    let pending = store.transcript_header_page(&before, None, 32).unwrap();
    assert!(pending.headers[0].deferred);
    assert!(pending.headers[0].content.is_empty());
    assert_eq!(before.read.pending_total, 1);
    let heads = store.pending_header_page(&before.read, None, 32).unwrap();
    assert_eq!(heads.headers.len(), 1);
    assert_eq!(heads.headers[0].sequence, accepted.sequence.get());
    assert_eq!(heads.headers[0].revision, 0);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .pending_value_read(&before.read, accepted.sequence.get(), offset, 65_536)
            .unwrap();
        let (rows, work) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && work <= 131_072);
        bytes.extend(chunk.bytes);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    let original: crate::message::types::UserMessageBlock = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(original, accepted.message);
    let expected = PendingInboundRef {
        sequence: accepted.sequence,

        revision: 0,
    };
    let PendingMutationOutcome::Applied {
        read_cut: mutation_cut,
        ..
    } = store.edit_pending(&expected, "revised").unwrap()
    else {
        panic!("pending edit committed")
    };
    assert_eq!(mutation_cut, store.observation_read_cut().unwrap().read);
    assert!(store.transcript_header_page(&before, None, 32).is_err());
    assert!(
        store
            .pending_value_read(&before.read, accepted.sequence.get(), 0, 1)
            .is_err()
    );
    let edited = store.observation_read_cut().unwrap();
    let pending = store.transcript_header_page(&edited, None, 32).unwrap();
    let crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message {
        message: MessageBlock::User(user),
    } = &pending.headers[0].display.as_ref().unwrap().item
    else {
        panic!("pending User body")
    };
    assert!(matches!(&user.content[0],UserContentBlock::Text(text) if text.text=="revised"));
    assert!(
        store
            .content_descriptor(&canonical, ContentRepresentation::Utf8Text)
            .is_err()
    );
    let PendingMutationOutcome::Applied {
        read_cut: remove_cut,
        ..
    } = store
        .remove_pending(&PendingInboundRef {
            revision: 1,
            ..expected
        })
        .unwrap()
    else {
        panic!("pending remove committed")
    };
    assert_eq!(remove_cut, store.observation_read_cut().unwrap().read);
    let removed = store.observation_read_cut().unwrap();
    assert_eq!(removed.transcript_total, 0);
    assert!(
        store
            .transcript_header_page(&removed, None, 32)
            .unwrap()
            .headers
            .is_empty()
    );
    assert!(
        store
            .content_descriptor(&canonical, ContentRepresentation::Utf8Text)
            .is_err()
    );
}

#[test]
fn t08_t12_t15_canonical_metadata_components_are_independent_bounded_sources() {
    use crate::message::types::{ReasoningBlock, ToolCallOccurrenceRef, ToolMessageBlock};
    use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
    use crate::runtime::identity::{ArtifactId, ToolCallId, ToolId};
    use crate::runtime_client::content::CanonicalContentComponent as Component;
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metadata.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a8");
    let assistant_id = MessageId::new("large-metadata-call");
    let result_id = MessageId::new("large-metadata-result");
    let call = ToolCall {
        id: ToolCallId::new("call"),
        tool_id: ToolId::new("native"),
        name: "name".repeat(600_000),
        arguments: serde_json::json!({"argument":"payload".repeat(500_000)}),
    };
    let provider = serde_json::json!({"opaque":"provider".repeat(300_000)});
    let result = MessageBlock::Tool(ToolMessageBlock {
        id: result_id.clone(),
        occurrence: ToolCallOccurrenceRef::new(assistant_id.clone(), ContentBlockIndex::new(0)),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 17,
            exit_code: Some(0),
            artifacts: vec![crate::message::content::FileReference {
                artifact_id: ArtifactId::new("owned"),
                name: Some("file".repeat(600_000)),
                mime_type: None,
                description: None,
            }],
            deliveries: Vec::new(),
            workflow: None,
            truncation: None,
            managed_output: None,
        },
    });
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .initialize(&[
                MessageBlock::Assistant(AssistantMessageBlock {
                    id: assistant_id.clone(),
                    content: vec![
                        AssistantContentBlock::ToolCall(call.clone()),
                        AssistantContentBlock::Reasoning(ReasoningBlock {
                            text: Some("visible text".into()),
                            provider_state: Some(ProviderContinuationState::Anthropic(
                                AnthropicContinuation {
                                    opaque: provider.clone(),
                                },
                            )),
                        }),
                    ],
                }),
                result.clone(),
            ])
            .unwrap();
    }
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let read = |message_id: MessageId, block: u32, component| {
        let source = ContentSource::CanonicalMessageBlock {
            component,
            locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                message_id,
            },
            block_index: ContentBlockIndex::new(block),
        };
        let reference = store
            .content_descriptor(&source, ContentRepresentation::CanonicalJsonV1)
            .unwrap()
            .unwrap();
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = store.content_read(&reference, offset, 65_536).unwrap();
            let (rows, work) = SqliteConversationStore::content_read_work();
            assert!(rows <= 2 && work <= 131_072);
            bytes.extend(
                base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    chunk.data_base64,
                )
                .unwrap(),
            );
            match chunk.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        (
            reference,
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        )
    };
    let (metadata_ref, metadata) = read(assistant_id.clone(), 0, Component::BlockMetadata);
    assert_eq!(metadata["name"], call.name);
    assert!(metadata.get("arguments").is_none());
    let (payload_ref, payload) = read(assistant_id.clone(), 0, Component::Payload);
    assert_eq!(payload, call.arguments);
    assert_ne!(metadata_ref.source, payload_ref.source);
    let mut forged = metadata_ref.clone();
    forged.source = payload_ref.source;
    assert!(store.content_read(&forged, 0, 1).is_err());
    let (_, reasoning) = read(assistant_id, 1, Component::BlockMetadata);
    assert_eq!(reasoning["provider_state"]["anthropic"]["opaque"], provider);
    assert!(reasoning.get("text").is_none());
    let (_, metadata) = read(result_id.clone(), 0, Component::MessageMetadata);
    assert_eq!(
        metadata["result"]["artifacts"][0]["name"],
        "file".repeat(600_000)
    );
    assert!(metadata["result"].get("content").is_none());
    assert!(
        store
            .content_descriptor(
                &ContentSource::CanonicalMessageBlock {
                    component: Component::MessageMetadata,
                    locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                        message_id: result_id
                    },
                    block_index: ContentBlockIndex::new(1)
                },
                ContentRepresentation::CanonicalJsonV1
            )
            .is_err()
    );
    let cut = store.observation_read_cut().unwrap();
    let blocks = store
        .message_block_header_page(&cut, &MessageId::new("large-metadata-call"), 0, 32)
        .unwrap();
    assert!(blocks.headers.iter().all(|header| header.deferred));
    assert!(
        blocks
            .headers
            .iter()
            .all(|header| header.content.iter().any(|binding| matches!(
                binding.content_ref.source,
                ContentSource::CanonicalMessageBlock {
                    component: Component::BlockMetadata,
                    ..
                }
            )))
    );
    assert!(serde_json::to_vec(&blocks).unwrap().len() < 32_768);
}

#[test]
fn t08_t14_t15_native_attempt_summary_matches_timing_oracle_and_pages_giant_model() {
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::model::{ModelFinishReason, ModelUsage, generation_evidence::GenerationEvidence};
    use crate::runtime::identity::{AttemptId, EventId};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("attempt-models.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8b3");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("timed-attempt");
    let start = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let end = start + chrono::Duration::milliseconds(20_000);
    let mut event_index = 0;
    let mut append = |event, timestamp, turn_id| {
        event_index += 1;
        store
            .append_event(RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new(format!("timing-{event_index}")),
                sequence: 0,
                conversation_id: conversation.clone(),
                attempt_id: Some(attempt.clone()),
                turn_id,
                timestamp,
                event,
            })
            .unwrap()
    };
    append(
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt.clone(),
        },
        start,
        None,
    );
    let giant = "model-λ🙂".repeat(300_000);
    let usage = ModelUsage {
        input_tokens: 11,
        output_tokens: 7,
        total_tokens: 18,
        details: None,
    };
    let mut oracle = crate::durable::response::timing::TimingFold::default();
    for index in 0..41 {
        let snapshot = timed_request_snapshot(
            &store,
            &attempt,
            index,
            if index == 0 {
                giant.clone()
            } else {
                format!("model-{index}")
            },
        );
        store
            .commit_model_turn_start(&[], &snapshot, start)
            .unwrap();
        let request = snapshot.request_id;
        let turn = snapshot.identity.turn;
        oracle.start(request.clone());
        let generation = GenerationEvidence {
            dispatch_after_start_ms: Some(2),
            first_output_ms: Some(5),
            last_output_ms: Some(15),
            terminal_ms: 25,
        };
        oracle.terminal(&request, Some(generation), Some(&usage));
        append(
            RuntimeEvent::ModelRequestCompleted {
                request_id: request,
                finish_reason: ModelFinishReason::Stop,
                usage: Some(usage.clone()),
                generation: Some(generation),
            },
            end,
            Some(turn),
        );
    }
    let through = append(
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt.clone(),
            finish_reason: ModelFinishReason::Stop,
        },
        end,
        None,
    )
    .sequence;
    let expected = oracle.summary(Some(start), end);
    let summary = store
        .observation_attempt_summary(&attempt, through)
        .unwrap();
    assert_eq!(summary.timing, expected);
    assert_eq!(summary.requests, 41);
    assert_eq!(summary.reports, 41);
    assert_eq!(summary.usage.as_ref().unwrap().output_tokens, 287);
    assert_eq!(summary.models_total, 41);
    assert!(
        summary.models.is_empty(),
        "a deferred first model cannot leave a fake contiguous prefix"
    );
    assert_eq!(summary.models_next_offset, Some(0));
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    // Any all-J deserialization would now fail; finite native summaries and
    // field byte indexes remain sufficient after durable restart.
    connection
        .execute("UPDATE events SET event_json='{}'", [])
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_eq!(
        store
            .observation_attempt_summary(&attempt, through)
            .unwrap(),
        summary
    );
    let page = store
        .observation_models_page(&attempt, through, 0, 32)
        .unwrap();
    assert_eq!(page.names.len(), 32);
    assert_eq!(page.next_offset, Some(32));
    assert!(page.names[0].value.is_none());
    assert!(serde_json::to_vec(&page).unwrap().len() < 16_384);
    let rest = store
        .observation_models_page(&attempt, through, 32, 32)
        .unwrap();
    assert_eq!(rest.names.len(), 9);
    assert_eq!(rest.next_offset, None);
    let mut offset = 0;
    let mut bytes = Vec::new();
    while offset < page.names[0].encoded_bytes {
        let chunk = store
            .observation_model_value_read(&attempt, through, 0, offset, 65_536)
            .unwrap();
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
        assert!(chunk.end_offset > offset);
        bytes.extend_from_slice(&chunk.bytes);
        offset = chunk.end_offset;
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), giant);
    assert!(
        store
            .observation_model_value_read(&attempt, through, 41, 0, 1)
            .is_err()
    );
}

fn timed_request_snapshot(
    store: &SqliteConversationStore,
    attempt: &crate::runtime::identity::AttemptId,
    index: usize,
    model: String,
) -> crate::model::RequestSnapshot {
    use crate::model::{ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestSnapshot};
    RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt.clone(),
            turn: crate::runtime::identity::TurnId::new(format!("timing-{index}")),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier(store.load_head().unwrap().revision.get()),
        "prompt".into(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model,
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: crate::model::RequestParams::new(),
            capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
            compat: crate::model::catalog::ModelCompat::default(),
        },
        64_000,
        None,
        false,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(1),
        crate::context::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}

fn assert_request_source_point_indexes(connection: &rusqlite::Connection, sequence: u64) {
    let hash_index: String = connection
        .query_row(
            "SELECT name FROM pragma_index_list('request_source_pages') WHERE \"unique\"=1 AND origin='pk'",
            [], |row| row.get(0),
        )
        .unwrap();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
        .unwrap()
        .query_map([&hash_index], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, ["hash"]);
    for (sql, key, expected) in [
        (
            "EXPLAIN QUERY PLAN SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM request_source_pages WHERE hash=?1",
            rusqlite::types::Value::Blob(vec![0; 32]),
            format!("SEARCH request_source_pages USING INDEX {hash_index} (hash=?)"),
        ),
        (
            "EXPLAIN QUERY PLAN SELECT metadata,witness FROM request_source_roots WHERE source_id=?1",
            rusqlite::types::Value::Text(sequence.to_string()),
            "SEARCH request_source_roots USING PRIMARY KEY (source_id=?)".into(),
        ),
    ] {
        let plan: String = connection.query_row(sql, [key], |row| row.get(3)).unwrap();
        assert!(plan.contains(&expected), "{plan}");
        assert!(!plan.contains("SCAN"), "{plan}");
    }
}

fn assert_request_scalar_range_work() {
    let work = SqliteConversationStore::request_source_read_work();
    // One field lookup and at most nine 8KiB chunks for a 64KiB window;
    // every independent authenticated fragment descent has at most 16 pages.
    assert!((1..=10 * 16).contains(&work.index_pages), "{work:?}");
    assert!((1..=65_536).contains(&work.max_index_page));
    assert!(work.index_bytes <= work.index_pages * 65_536);
    assert_eq!(work.source_nodes, 1, "one original Request root");
    assert!(work.source_metadata_bytes <= 16_384);
    assert!(work.max_source_metadata <= 16_384);
    assert_eq!(work.native_nodes, 0, "this Request has no selected inputs");
    assert_eq!(
        work.header_decodes, 0,
        "ranges do not build display headers"
    );
}

#[test]
fn t08_t20_request_source_ranges_use_bounded_authenticated_composed_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("request-range.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8b4");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let attempt = crate::runtime::identity::AttemptId::new("source-range");
    let mut snapshot = timed_request_snapshot(&store, &attempt, 0, "model".into());
    snapshot.effective_system_prompt = "λ🙂".repeat(400_000);
    // Freeze the actual changed request, including its exact prompt fingerprint.
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            &conversation,
            &crate::model::ModelRequest {
                images: snapshot.images.clone(),
                invocation: snapshot.invocation.clone(),
                messages: history.messages,
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
                continuation_source: snapshot.continuation_source.clone(),
            },
        )
        .unwrap();
    let prompt = snapshot.effective_system_prompt.clone();
    let started = store
        .commit_model_turn_start(
            &[],
            &snapshot,
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        )
        .unwrap()
        .started;
    let request = snapshot.request_id;
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE request_snapshots SET snapshot_json='invalid JSON'",
            [],
        )
        .unwrap();
    connection
        .execute("UPDATE surface_head SET active_json='invalid JSON'", [])
        .unwrap();
    assert_request_source_point_indexes(&connection, started.sequence);
    drop(connection);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let field = vec!["effective_system_prompt".into(), "text".into()];
    let header = store
        .request_field_header(&request, started.sequence, &field)
        .unwrap();
    let mut offset = 0;
    let mut bytes = Vec::new();
    while offset < header.encoded_bytes {
        SqliteConversationStore::request_source_read_work();
        let chunk = store
            .request_value_read(&request, started.sequence, &field, offset, 65_536)
            .unwrap();
        assert_request_scalar_range_work();
        assert!(chunk.bytes.len() <= 65_536);
        assert!(chunk.end_offset > offset);
        assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
        assert_eq!(chunk.readable_end, header.encoded_bytes);
        assert_eq!(
            chunk.next_offset,
            (chunk.end_offset < chunk.readable_end).then_some(chunk.end_offset)
        );
        bytes.extend_from_slice(&chunk.bytes);
        offset = chunk.end_offset;
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), prompt);
    assert!(
        store
            .request_value_read(
                &request,
                started.sequence,
                &field,
                header.encoded_bytes + 1,
                1
            )
            .is_err()
    );
}

#[test]
fn t08_t15_t20_native_state_sources_survive_large_cache_eviction_and_reject_stale_goal() {
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::goal::{GoalMutation, GoalOrigin, GoalWrite};
    use crate::runtime::identity::EventId;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-state-range.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8b5");
    let body = "x".repeat(17 * 1024 * 1024);
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&todo_source_messages(&body)).unwrap();
    let todo_cut = store.observation_read_cut().unwrap();
    let todo_head = store.todo_observation_head(&todo_cut).unwrap().unwrap();
    assert_eq!(todo_head.source_ledger_position, 2);
    assert_eq!((todo_head.total, todo_head.loaded), (73, 0));
    assert_eq!(todo_head.next_offset, Some(0));
    assert_eq!(todo_head.snapshot.next_id, 74);
    assert!(todo_head.snapshot.tasks.is_empty());
    assert!(serde_json::to_vec(&todo_head).unwrap().len() <= 16_384);
    let goal = store
        .write_goal(GoalWrite::Create {
            objective: "goal".into(),
            budget: 3,
            origin: GoalOrigin::RuntimeControl,
        })
        .unwrap()
        .0
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    let goal_source = store
        .observation_state_source("goal", &before)
        .unwrap()
        .unwrap();
    let time = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let failure = store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("giant-compaction-diagnostic"),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: time,
            event: RuntimeEvent::CompactionFailed {
                operation_id: crate::runtime::identity::CompactionOperationId::new(
                    "diagnostic-compaction",
                ),
                error: body.clone(),
            },
        })
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let context_source = store
        .observation_state_source("context", &cut)
        .unwrap()
        .unwrap();
    assert_eq!(context_source, format!("context/{}", failure.sequence));
    assert_eq!(failure.event_id.as_str(), "giant-compaction-diagnostic");
    assert!(
        store
            .observation_state_source("context", &before)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .observation_state_header("context", &context_source, &before, &[])
            .is_err()
    );
    let todo_source = store
        .observation_state_source("todos", &cut)
        .unwrap()
        .unwrap();
    let first = store
        .observation_state_page("todos", &todo_source, &cut, &["tasks".into()], 0, 32)
        .unwrap();
    assert_eq!(first.total, 73);
    assert_eq!(first.fields.len(), 32);
    assert_eq!(first.next_offset, Some(32));
    assert!(serde_json::to_vec(&first).unwrap().len() < 786_432);
    store
        .write_goal(GoalWrite::Mutate {
            expected: goal.reference,
            mutation: GoalMutation::Budget { rounds: 4 },
        })
        .unwrap()
        .0
        .unwrap();
    assert!(
        store
            .observation_state_header("goal", &goal_source, &cut, &[])
            .is_err()
    );
    let current = store.observation_read_cut().unwrap();
    let new_goal = store
        .observation_state_source("goal", &current)
        .unwrap()
        .unwrap();
    assert_ne!(new_goal, goal_source);
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    connection
        .execute("UPDATE events SET event_json='{}'", [])
        .unwrap();
    connection
        .execute("UPDATE goal_state SET snapshot_json='invalid JSON'", [])
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let finite_goal = store.goal_observation_view().unwrap().current.unwrap();
    assert_eq!(finite_goal.objective, "goal");
    assert_eq!(finite_goal.autonomous_round_budget, 4);
    assert!(!finite_goal.objective_deferred);
    assert!(
        store.load_goal().is_err(),
        "the finite Goal header is read without deserializing the original row"
    );
    assert!(
        store
            .observation_state_header("goal", &new_goal, &current, &[])
            .unwrap()
            .value["current"]
            .is_object()
    );
    for (domain, source, field) in [
        ("context", context_source, vec!["compaction_error".into()]),
        (
            "todos",
            todo_source,
            vec!["tasks".into(), "0".into(), "description".into()],
        ),
    ] {
        let header = store
            .observation_state_header(domain, &source, &cut, &field)
            .unwrap();
        assert!(header.encoded_bytes > 16 * 1024 * 1024);
        let mut offset = 0;
        let mut bytes = Vec::new();
        while offset < header.encoded_bytes {
            let chunk = store
                .observation_state_value_read(domain, &source, &cut, &field, offset, 65_536)
                .unwrap();
            let (rows, materialized) = SqliteConversationStore::content_read_work();
            assert!((1..=2).contains(&rows) && materialized <= 131_072);
            assert!(chunk.end_offset > offset);
            bytes.extend_from_slice(&chunk.bytes);
            offset = chunk.end_offset;
        }
        assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), body);
        assert!(
            store
                .observation_state_value_read(
                    domain,
                    &source,
                    &cut,
                    &field,
                    header.encoded_bytes + 1,
                    1
                )
                .is_err()
        );
    }
}

#[test]
fn t14_t15_t20_canonical_todo_receipt_preserves_exact_bounded_head_and_allocator() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_601cccec-75c7-703d-af18-cdf2cb42a8c8",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    let mut messages = todo_source_messages("finite detail");
    let MessageBlock::Tool(tool) = &mut messages[1] else {
        panic!("canonical Todo source");
    };
    let crate::tools::types::ToolResultContent::Json { value } = &mut tool.result.content[0] else {
        panic!("native Todo JSON");
    };
    value["next_id"] = serde_json::Value::from(u64::MAX);
    let receipts = store.append_canonical_batch(&messages).unwrap();
    let todo = receipts.last().unwrap().observation.todos.as_ref().unwrap();
    assert_eq!(todo.source_ledger_position, 2);
    assert_eq!((todo.total, todo.loaded), (73, 32));
    assert_eq!(todo.next_offset, Some(32));
    assert_eq!(todo.snapshot.next_id, u64::MAX);
    assert_eq!(todo.snapshot.tasks.first().unwrap().id, 1);
    assert_eq!(todo.snapshot.tasks.last().unwrap().id, 32);
    assert_eq!(
        store
            .todo_observation_head(&receipts.last().unwrap().observation.read_cut)
            .unwrap()
            .as_ref(),
        Some(todo)
    );
    let source = store
        .observation_state_source("todos", &receipts.last().unwrap().observation.read_cut)
        .unwrap()
        .unwrap();
    let allocator = store
        .observation_state_value_read(
            "todos",
            &source,
            &receipts.last().unwrap().observation.read_cut,
            &["next_id".into()],
            0,
            64,
        )
        .unwrap();
    assert_eq!(allocator.bytes, b"\"18446744073709551615\"");
    let page = store
        .observation_state_page(
            "todos",
            &source,
            &receipts.last().unwrap().observation.read_cut,
            &["tasks".into()],
            32,
            32,
        )
        .unwrap();
    assert_eq!(page.total, 73);
    assert_eq!(page.next_offset, Some(64));
    assert_eq!(page.fields.len(), 32);
}

fn todo_source_messages(description: &str) -> Vec<MessageBlock> {
    use crate::message::types::{ToolCallOccurrenceRef, ToolMessageBlock};
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::types::{
        ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    };
    let tool_id = ToolId::new(crate::tools::todo::TODO_TOOL_ID);
    let call = ToolCallId::new("todo-source-call");
    let assistant = MessageId::new("todo-source-owner");
    let snapshot = crate::tools::todo::TodoSnapshot {
        next_id: 74,
        tasks: (1..=73)
            .map(|id| crate::tools::todo::TodoTask {
                id,
                subject: format!("task-{id}"),
                description: Some(if id == 1 {
                    description.to_owned()
                } else {
                    "detail".into()
                }),
                active_form: None,
                status: crate::tools::todo::TodoStatus::Pending,
                blocked_by: Vec::new(),
                owner: None,
                metadata: None,
            })
            .collect(),
    };
    snapshot.validate().unwrap();
    vec![
        MessageBlock::Assistant(AssistantMessageBlock {
            id: assistant.clone(),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: call.clone(),
                tool_id: tool_id.clone(),
                name: "todo".into(),
                arguments: serde_json::json!({}),
            })],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("todo-source-result"),
            occurrence: ToolCallOccurrenceRef::new(assistant, ContentBlockIndex::new(0)),
            tool_call_id: call,
            tool_id,
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Json {
                    value: serde_json::to_value(snapshot).unwrap(),
                }],
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
                deliveries: Vec::new(),
            },
        }),
    ]
}

#[test]
fn t08_t12_t19_binary_canonical_artifact_ranges_are_frozen_owned_and_restartable() {
    use crate::message::content::FileReference;
    use crate::runtime_client::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
    };
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::new("conv_17886270-8682-7777-8444-99abcf664493");
    let path = dir.path().join("binary.sqlite3");
    let artifacts =
        crate::tools::artifacts::ArtifactStore::new(conversation.clone(), dir.path()).unwrap();
    let artifact = artifacts.create_artifact().unwrap();
    let bytes = (0..1_500_003)
        .map(|index| u8::try_from(index % 256).unwrap())
        .collect::<Vec<_>>();
    let mut writer = artifacts.open_writer(&artifact).unwrap();
    writer.write_all(&bytes).unwrap();
    drop(writer);
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let message = MessageBlock::User(crate::message::types::UserMessageBlock {
        id: MessageId::new("binary-native-owner"),
        content: vec![crate::message::types::UserContentBlock::File(
            FileReference {
                artifact_id: artifact.clone(),
                name: Some("bytes.bin".into()),
                mime_type: Some("application/octet-stream".into()),
                description: None,
            },
        )],
        source: crate::message::types::UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        timestamp: None,
    });
    store.initialize(&[message]).unwrap();
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::Payload,
                locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
                block_index: ContentBlockIndex::new(0),
            },
            ContentRepresentation::BinaryBytes,
        )
        .unwrap()
        .unwrap();
    assert_eq!(reference.readable_end, bytes.len() as u64);
    assert!(
        artifacts.open_writer(&artifact).is_err(),
        "native settled artifact cannot be reopened for replacement"
    );
    drop(store);
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let mut actual = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        let (_, work) = SqliteConversationStore::content_read_work();
        assert!(work <= 65_536);
        actual.extend(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                chunk.data_base64,
            )
            .unwrap(),
        );
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(actual, bytes);
    let mut forged = reference.clone();
    forged.readable_end += 1;
    assert!(store.content_read(&forged, 0, 1).is_err());
    let mut forged = reference.clone();
    forged.conversation_id = ConversationId::new("conv_17886270-8682-7777-8444-99abcf664494");
    assert!(store.content_read(&forged, 0, 1).is_err());
    let mut forged = reference.clone();
    forged.source = ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::MessageMetadata,
        locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(store.content_read(&forged, 0, 1).is_err());
    let oldcut = store.observation_read_cut().unwrap();
    let mut oldcut = oldcut;
    oldcut.ledger_through = 0;
    oldcut.read.ledger_through = 0;
    assert!(
        store
            .validate_content_source_cut(&reference, &oldcut)
            .is_err()
    );
    // The existing lineage Artifact owner materializes settled bytes before C;
    // the inherited locator belongs exclusively to the Child Conversation.
    let mut captured = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    let child_dir = tempfile::tempdir().unwrap();
    let child_conversation = ConversationId::new("conv_17886270-8682-7777-8444-99abcf664499");
    let child_artifacts =
        crate::tools::artifacts::ArtifactStore::new(child_conversation.clone(), child_dir.path())
            .unwrap();
    let child_artifact = child_artifacts
        .copy_from(&artifacts, &artifact, &mut (bytes.len() as u64))
        .unwrap();
    let MessageBlock::User(user) = &mut captured.canonical[0] else {
        panic!("native File owner")
    };
    let crate::message::types::UserContentBlock::File(file) = &mut user.content[0] else {
        panic!("native File source")
    };
    file.artifact_id = child_artifact.clone();
    let seed = crate::durable::LineageSeed::replayed(captured.canonical, captured.surface_history)
        .unwrap()
        .with_projections(captured.projections)
        .unwrap()
        .with_raw_tool_arguments(captured.raw_tool_arguments)
        .unwrap();
    let child_path = child_dir.path().join("child.sqlite3");
    let child = SqliteConversationStore::open(child_conversation.clone(), &child_path).unwrap();
    child.initialize_lineage(&seed).unwrap();
    let child_ref = child
        .content_descriptor(&reference.source, ContentRepresentation::BinaryBytes)
        .unwrap()
        .unwrap();
    assert_eq!(child_ref.conversation_id, child_conversation);
    assert_eq!(child_ref.readable_end, reference.readable_end);
    drop(child);
    drop(store);
    drop(artifacts);
    drop(dir);
    let child = SqliteConversationStore::open(child_conversation, &child_path).unwrap();
    assert!(child.content_read(&reference, 0, 1).is_err());
    let (mut offset, mut inherited) = (0, Vec::new());
    loop {
        let chunk = child.content_read(&child_ref, offset, 65536).unwrap();
        inherited.extend(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                chunk.data_base64,
            )
            .unwrap(),
        );
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(inherited, bytes);
    // Rebinding into the Child captures its own durable integrity evidence;
    // even a same-length filesystem replacement after reopen cannot change it.
    let binary_path = child_dir
        .path()
        .join(format!("{}.bin", child_artifact.as_str()));
    let mut external = std::fs::OpenOptions::new()
        .write(true)
        .open(binary_path)
        .unwrap();
    external.write_all(b"other").unwrap();
    external.sync_all().unwrap();
    assert_eq!(external.metadata().unwrap().len(), child_ref.readable_end);
    assert!(child.content_read(&child_ref, 0, 5).is_err());
}

#[test]
fn t08_t12_t15_t20_giant_canonical_identity_uses_finite_native_ledger_locator() {
    use crate::runtime_client::content::{CanonicalContentComponent, CanonicalContentLocator};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("giant-id.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c2");
    let id = MessageId::new("original-identity-工具".repeat(100_000));
    let text = "λ🙂".repeat(150_000);
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store
        .initialize(&[MessageBlock::Assistant(AssistantMessageBlock {
            id: id.clone(),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: text.clone(),
            })],
        })])
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let headers = store.message_header_page(&cut, Some(0), 32).unwrap();
    let header = &headers.headers[0];
    assert!(header.identity.is_none());
    assert!(header.display.is_none());
    assert!(header.deferred);
    assert_eq!(header.ledger_position, Some(1));
    assert_eq!(
        header.locator,
        Some(CanonicalContentLocator::ByLedgerPosition { position: 1 })
    );
    assert!(serde_json::to_vec(header).unwrap().len() <= 16_384);
    let source = ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
        block_index: ContentBlockIndex::new(0),
    };
    let reference = store
        .content_descriptor(&source, ContentRepresentation::Utf8Text)
        .unwrap()
        .unwrap();
    let metadata = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::MessageMetadata,
                locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
                block_index: ContentBlockIndex::new(0),
            },
            ContentRepresentation::CanonicalJsonV1,
        )
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&reference).unwrap().len() < 1024);
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let maximum: u64 = connection
        .query_row(
            "SELECT MAX(length(source_id)) FROM content_segments WHERE source_kind='canonical'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(maximum < 64);
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let mut offset = 0;
    let mut actual = Vec::new();
    loop {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        let (rows, bytes) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && bytes <= 131_072);
        actual.extend(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                chunk.data_base64,
            )
            .unwrap(),
        );
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(actual, text.as_bytes());
    let first = store.content_read(&metadata, 0, 65_536).unwrap();
    assert_eq!(first.end_offset, 65_536);
    let mut forged = reference.clone();
    forged.source = ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByMessageId { message_id: id },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(
        store.content_read(&forged, 0, 1).is_err(),
        "oversized client identity cannot bypass finite locator admission"
    );
    let mut forged = reference;
    forged.source = ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByLedgerPosition { position: 2 },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(store.content_read(&forged, 0, 1).is_err());
}

#[test]
fn t08_t15_t20_surface_boundary_pages_read_native_headers_without_surface_or_message_bodies() {
    use crate::conversation::SurfaceRevision;
    use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("surface-headers.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8d2");
    let messages = (0..73)
        .map(|index| {
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new(if index == 72 {
                    "giant-boundary-id".repeat(100_000)
                } else {
                    format!("boundary-{index}")
                }),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: if index == 0 {
                        "x".repeat(17 * 1024 * 1024)
                    } else {
                        format!("body-{index}")
                    },
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })
        })
        .collect::<Vec<_>>();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&messages).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    connection
        .execute("UPDATE surface_head SET active_json='invalid JSON'", [])
        .unwrap();
    connection
        .execute("UPDATE surface_ops SET op_json='invalid JSON'", [])
        .unwrap();
    drop(connection);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let through = SurfaceRevision::new(73);
    let mut offset = 0;
    let mut count = 0;
    loop {
        let page = store
            .surface_boundary_header_page(through, offset, 32)
            .unwrap();
        assert_eq!(page.total, 73);
        assert!(page.boundaries.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 786_432);
        count += page.boundaries.len();
        for boundary in &page.boundaries {
            assert_eq!(boundary.header.role, "user");
            assert_eq!(boundary.header.kind, "message");
            let locator = boundary.header.locator.as_ref().unwrap();
            let id = store
                .surface_boundary_message_id(boundary.surface_revision, locator)
                .unwrap();
            assert_eq!(
                id,
                messages[usize::try_from(boundary.surface_revision.get() - 1).unwrap()]
                    .id()
                    .clone()
            );
        }
        if let Some(next) = page.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(count, 73);
    let old = store
        .surface_boundary_header_page(SurfaceRevision::new(32), 0, 32)
        .unwrap();
    assert_eq!(old.total, 32);
    assert!(old.next_offset.is_none());
    assert!(
        store
            .surface_boundary_header_page(SurfaceRevision::new(74), 0, 32)
            .is_err()
    );
    let locator =
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 73 };
    assert!(
        store
            .surface_boundary_message_id(SurfaceRevision::new(72), &locator)
            .is_err()
    );
    let captured = store.observation_read_cut().unwrap();
    let blocks = store
        .message_block_header_page_by_locator(&captured, &locator, 0, 32)
        .unwrap();
    assert!(blocks.message_id.is_none());
    assert_eq!(blocks.total, 1);
    assert_eq!(blocks.headers.len(), 1);
}

#[test]
fn t08_t12_canonical_locator_namespaces_preserve_literal_ids_and_full_inline_id_budget() {
    use crate::runtime_client::content::{CanonicalContentComponent, CanonicalContentLocator};
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8b9");
    let store = SqliteConversationStore::in_memory(conversation).unwrap();
    let exact_id = "i".repeat(256);
    let message = |id: &str, text: &str| {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::Text(TextBlock { text: text.into() })],
        })
    };
    store
        .initialize(&[
            message("ledger:2", "literal-id"),
            message("ordinary", "position-two"),
            message(&exact_id, "full-id-budget"),
        ])
        .unwrap();
    let source = |locator| ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator,
        block_index: ContentBlockIndex::new(0),
    };
    let literal = source(CanonicalContentLocator::ByMessageId {
        message_id: MessageId::new("ledger:2"),
    });
    let numeric = source(CanonicalContentLocator::ByLedgerPosition { position: 2 });
    assert_ne!(literal.key(), numeric.key());
    for (source, expected) in [
        (literal, "literal-id"),
        (numeric, "position-two"),
        (
            source(CanonicalContentLocator::ByMessageId {
                message_id: MessageId::new(exact_id),
            }),
            "full-id-budget",
        ),
    ] {
        let reference = store
            .content_descriptor(&source, ContentRepresentation::Utf8Text)
            .unwrap()
            .unwrap();
        let chunk = store.content_read(&reference, 0, 65536).unwrap();
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            chunk.data_base64,
        )
        .unwrap();
        assert_eq!(bytes, expected.as_bytes());
        assert_eq!(chunk.next_offset, None);
    }
}

#[test]
fn t08_t14_t15_many_blocks_keep_original_metadata_and_advancing_frontiers() {
    use crate::runtime_client::content::{CanonicalContentComponent, CanonicalContentLocator};
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_601cccec-75c7-703d-af18-cdf2cb42a8c0",
    ))
    .unwrap();
    let message = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("many-blocks"),
        content: (0..73)
            .map(|index| {
                AssistantContentBlock::Text(TextBlock {
                    text: format!("{index}:{}", "x".repeat(18000)),
                })
            })
            .collect(),
    });
    store.initialize(&[message]).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let header = store
        .message_header_page(&cut, Some(0), 32)
        .unwrap()
        .headers
        .remove(0);
    assert!(header.display.is_none());
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&header, 16384).is_some());
    let metadata = header
        .content
        .iter()
        .find(|binding| {
            matches!(
                binding.content_ref.source,
                ContentSource::CanonicalMessageBlock {
                    component: CanonicalContentComponent::MessageMetadata,
                    ..
                }
            )
        })
        .expect("original metadata remains readable");
    assert!(metadata.deferred);
    assert!(store.content_read(&metadata.content_ref, 0, 65536).is_ok());
    let frontier = header
        .collections
        .iter()
        .find(|frontier| frontier.domain == "messages.ledger:1.blocks")
        .expect("advancing block source retained");
    assert_eq!(frontier.total, 73);
    let locator = CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let mut offset = 0;
    let mut seen = 0;
    loop {
        let page = store
            .message_block_header_page_by_locator(&cut, &locator, offset, 32)
            .unwrap();
        assert!(!page.headers.is_empty());
        seen += page.headers.len();
        let Some(next) = page.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(seen, 73);
}

#[test]
fn t08_t12_t15_t20_pending_sequence_reads_giant_identity_without_body_deserialization() {
    use crate::durable::inbox::{InboundDraft, PendingInboundRef, PendingMutationOutcome};
    use crate::message::types::{InboundKind, UserContentBlock, UserSource};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending-giant-id.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9");
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    store.initialize(&[]).unwrap();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("id雪".repeat(400_000))),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "body".repeat(400_000),
            })],
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = store.pending_header_page(&cut.read, None, 32).unwrap();
    assert!(page.headers[0].message.identity.is_none());
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE pending_inbound SET message_json='invalid original body' WHERE sequence=?1",
            [accepted.sequence.get()],
        )
        .unwrap();
    drop(connection);
    let chunk = store
        .pending_value_read(&cut.read, accepted.sequence.get(), 65_530, 65_536)
        .unwrap();
    assert_eq!(chunk.bytes.len(), 65_536);
    assert_eq!(chunk.next_offset, Some(131_066));
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert_eq!(rows, 2);
    assert!(bytes <= 131_072);
    let descriptor = PendingInboundRef {
        sequence: accepted.sequence,
        revision: 1,
    };
    assert_eq!(
        serde_json::to_value(&descriptor)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        2
    );
    // A stale revision is refused by the scalar authority probe before even
    // touching the deliberately corrupted enormous original Message JSON.
    assert_eq!(
        store.remove_pending(&descriptor).unwrap(),
        PendingMutationOutcome::Conflict
    );
}

#[test]
fn t10_t14_t19_derived_canonical_and_pending_headers_do_not_refuse_native_commit() {
    use crate::durable::inbox::InboundDraft;
    use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    use crate::runtime_client::content::CanonicalContentLocator;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("header-layout-failure.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8d0");
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    store.initialize(&[]).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_observation_header BEFORE INSERT ON observation_headers BEGIN SELECT RAISE(ABORT,'derived header unavailable'); END;").unwrap();
    let timestamp = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let message = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("original-native-c"),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "retained opening".into(),
        })],
    });
    let receipt = store.append_canonical(&message).unwrap();
    assert!(receipt.observation.source_unavailable);
    assert!(receipt.observation.head.is_empty());
    assert_eq!(receipt.observation.human_activity_at, Some(timestamp));
    assert_eq!(
        receipt.observation.opening_preview.as_deref(),
        Some("retained opening")
    );
    assert_eq!(store.load_canonical().unwrap(), vec![message]);
    assert_eq!(
        store
            .message_ledger_position(&MessageId::new("original-native-c"))
            .unwrap(),
        Some(1)
    );
    let source_revision = receipt.observation.read_cut.surface_revision;
    assert_eq!(
        store
            .surface_boundary_message_id(
                source_revision,
                &CanonicalContentLocator::ByLedgerPosition { position: 1 }
            )
            .unwrap(),
        MessageId::new("original-native-c"),
        "native mutation locator never depends on optional display layout"
    );
    assert!(
        store
            .surface_boundary_header_page(source_revision, 0, 32)
            .is_err(),
        "display hole is unavailable, never an empty boundary page"
    );
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("original-pending")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "pending decision".into(),
            })],
            timestamp,
            correlation: None,
        })
        .unwrap();
    // Native inbox remains authoritative and actionable despite missing display layout.
    assert_eq!(accepted.message_id.as_str(), "original-pending");
    assert!(accepted.pending_observation.is_none());
    let pending = store.load_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message, accepted.message);
    assert_eq!(store.observation_read_cut().unwrap().read.pending_total, 1);
}

#[test]
fn t08_t14_t15_tool_observation_headers_preserve_c_j_distinctions_without_body_reads() {
    use crate::events::types::RuntimeEvent;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tool-observation.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let messages = todo_source_messages("finite native detail");
    let MessageBlock::Assistant(assistant) = &messages[0] else {
        panic!("Assistant");
    };
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("ToolCall");
    };
    let MessageBlock::Tool(tool) = &messages[1] else {
        panic!("ToolResult");
    };
    store
        .append_canonical_with_event(
            &messages[0],
            tool_observation_event(
                &conversation,
                "accepted",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.id.clone(),
                },
            ),
        )
        .unwrap();
    let locator =
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let assembled_cut = store.observation_read_cut().unwrap();
    assert_tool_not_executed(&store, &assembled_cut, &locator);
    let started = store
        .append_event(tool_observation_event(
            &conversation,
            "started",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    store
        .append_canonical_with_event(
            &messages[1],
            tool_observation_event(
                &conversation,
                "result-committed",
                RuntimeEvent::ToolMessageCommitted {
                    message_id: tool.id.clone(),
                    tool_call_id: call.id.clone(),
                },
            ),
        )
        .unwrap();
    let committed_cut = store.observation_read_cut().unwrap();
    assert_tool_committed_before_terminal(&store, &committed_cut, &locator, started.sequence);
    let terminal = store
        .append_event(tool_observation_event(
            &conversation,
            "terminal",
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: tool.result.clone(),
            },
        ))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    drop(store);
    corrupt_tool_observation_bodies(&path);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let page = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(page.headers.len(), 1);
    assert_eq!(
        page.headers[0].terminal.as_ref().unwrap().sequence,
        terminal.sequence
    );
    assert_eq!(
        page.headers[0].terminal.as_ref().unwrap().event_kind,
        "tool_execution_completed"
    );
    assert_eq!(
        page.headers[0].terminal.as_ref().unwrap().value["result"]["status"],
        serde_json::json!({"type":"success"})
    );
    assert!(
        store
            .transcript_tool_headers(&committed_cut, &locator, 0, 32)
            .unwrap()
            .headers[0]
            .terminal
            .is_none()
    );
    assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
}
fn corrupt_tool_observation_bodies(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    connection
        .execute(
            "UPDATE events SET event_json=json_set(event_json,'$.event.result',json('{}'))",
            [],
        )
        .unwrap();
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_phase IS NOT NULL AND observation_tool_ledger=1 AND observation_tool_block=0 AND observation_tool_phase=2 AND sequence<=99 ORDER BY sequence DESC LIMIT 1", [], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("events_observation_tool_lifecycle_idx"),
        "{plan}"
    );
    drop(connection);
}
fn tool_observation_event(
    conversation: &ConversationId,
    name: &str,
    event: crate::events::types::RuntimeEvent,
) -> crate::events::types::RuntimeEventEnvelope {
    crate::events::types::RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id: crate::runtime::identity::EventId::new(name),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(crate::runtime::identity::AttemptId::new(
            "tool-header-attempt",
        )),
        turn_id: Some(crate::runtime::identity::TurnId::new("1")),
        timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        event,
    }
}

fn assert_tool_not_executed(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
) {
    let page = store.transcript_tool_headers(cut, locator, 0, 32).unwrap();
    assert_eq!(page.total, 1);
    assert!(page.headers[0].started.is_none());
    assert!(page.headers[0].terminal.is_none());
    assert!(page.headers[0].committed_result.is_none());
}
fn assert_tool_committed_before_terminal(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
    started: u64,
) {
    let page = store.transcript_tool_headers(cut, locator, 0, 32).unwrap();
    assert_eq!(page.headers[0].started.as_ref().unwrap().sequence, started);
    assert!(page.headers[0].association.result.is_some());
    assert!(page.headers[0].committed_result.is_some());
    assert!(
        page.headers[0].terminal.is_none(),
        "C never fabricates execution settlement"
    );
}

#[test]
fn t08_t19_t20_canonical_json_indexes_wide_borrowed_values_with_finite_scratch() {
    use crate::runtime_client::content::{CanonicalContentComponent, CanonicalContentLocator};
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9"),
        std::path::Path::new(":memory:"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let mut fields = serde_json::Map::new();
    for ordinal in (0..4096).rev() {
        fields.insert(
            format!("key:{ordinal:04}"),
            serde_json::json!({"z":ordinal,"a":"water水"}),
        );
    }
    fields.insert(
        "large-string".into(),
        serde_json::Value::String("\u{feff}水🙂\\\n".repeat(300_000)),
    );
    let original = serde_json::Value::Object(fields);
    let mut messages = todo_source_messages("placeholder");
    let MessageBlock::Assistant(assistant) = &mut messages[0] else {
        panic!("Assistant")
    };
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else {
        panic!("ToolCall")
    };
    call.arguments = original;
    store.append_canonical(&messages[0]).unwrap();
    let source = ContentSource::CanonicalMessageBlock {
        locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
        block_index: ContentBlockIndex::new(0),
        component: CanonicalContentComponent::Payload,
    };
    let reference = store
        .content_descriptor(&source, ContentRepresentation::CanonicalJsonV1)
        .unwrap()
        .unwrap();
    let (maximum_read, maximum_key, maximum_page, written) =
        crate::runtime::read_layout::canonical::WORK.with(std::cell::Cell::get);
    assert!(written > 3_000_000);
    assert!(maximum_read <= 65_536);
    assert!(maximum_key <= 65_536);
    assert!(maximum_page <= 32);
    let mut restored = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        restored.extend(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                chunk.data_base64,
            )
            .unwrap(),
        );
        let Some(next) = chunk.next_offset else {
            break;
        };
        offset = next;
    }
    let MessageBlock::Assistant(assistant) = &messages[0] else {
        panic!("Assistant")
    };
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("ToolCall")
    };
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&restored).unwrap(),
        call.arguments
    );
    assert!(restored.starts_with(b"{\"key:0000\":{\"a\":\"water"));
    assert!(
        restored
            .windows(b"\"key:4095\":{\"a\":".len())
            .any(|bytes| bytes == b"\"key:4095\":{\"a\":")
    );
    assert!(SqliteConversationStore::content_read_work().0 <= 2);
}

#[test]
fn t08_t19_t20_canonical_json_orders_unbounded_keys_by_original_utf8() {
    use crate::runtime_client::content::{CanonicalContentComponent, CanonicalContentLocator};
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9"),
        std::path::Path::new(":memory:"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let prefix = format!("{}\0\n\\\"🙂", "水".repeat(24_000));
    assert!(prefix.len() > 65_536);
    let mut fields = serde_json::Map::new();
    for suffix in ["🙂", "水", "z", "a", "\\", "\"", "\n", "\0"] {
        fields.insert(
            format!("{prefix}{suffix}"),
            serde_json::json!({"original":suffix}),
        );
    }
    let expected =
        serde_json::to_vec(&fields.iter().collect::<std::collections::BTreeMap<_, _>>()).unwrap();
    let mut messages = todo_source_messages("placeholder");
    let MessageBlock::Assistant(assistant) = &mut messages[0] else {
        panic!("Assistant")
    };
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else {
        panic!("ToolCall")
    };
    call.arguments = serde_json::Value::Object(fields);
    store.append_canonical(&messages[0]).unwrap();
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
                block_index: ContentBlockIndex::new(0),
                component: CanonicalContentComponent::Payload,
            },
            ContentRepresentation::CanonicalJsonV1,
        )
        .unwrap()
        .unwrap();
    let (read, key, page, written) =
        crate::runtime::read_layout::canonical::WORK.with(std::cell::Cell::get);
    assert!(written > 65_536);
    assert!(read <= 65_536 && key <= 65_536 && page <= 32);
    let mut restored = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store.content_read(&reference, offset, 65_536).unwrap();
        restored.extend(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                chunk.data_base64,
            )
            .unwrap(),
        );
        let (rows, bytes) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && bytes <= 131_072);
        let Some(next) = chunk.next_offset else {
            break;
        };
        offset = next;
    }
    assert_eq!(
        restored, expected,
        "escape representation must not define lexical original key order"
    );
}

#[test]
fn t08_t14_t19_tool_headers_keep_exact_raw_arguments_and_giant_identity_sources() {
    use crate::durable::inbox::CanonicalRawToolArguments;
    use crate::runtime_client::content::CanonicalContentLocator;
    let raw = " {\"n\":1.0,\"escaped\":\"\\u6c34\"} \n";
    let mut messages = todo_source_messages("unused");
    messages.truncate(1);
    let MessageBlock::Assistant(assistant) = &mut messages[0] else {
        panic!("Assistant")
    };
    assistant.id = MessageId::new("original-message".repeat(100));
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else {
        panic!("ToolCall")
    };
    call.id = crate::runtime::identity::ToolCallId::new("original-call".repeat(100));
    call.tool_id = crate::runtime::identity::ToolId::new("original-tool".repeat(100));
    call.name = "native水".repeat(10_000);
    call.arguments = serde_json::from_str(raw).unwrap();
    let original_id = assistant.id.clone();
    let seed = crate::durable::LineageSeed::history(messages)
        .with_raw_tool_arguments(vec![CanonicalRawToolArguments {
            message_id: original_id,
            block_index: ContentBlockIndex::new(0),
            bytes: raw.as_bytes().to_vec(),
        }])
        .unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9"),
        std::path::Path::new(":memory:"),
    )
    .unwrap();
    store.initialize_lineage(&seed).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = store
        .transcript_tool_headers(
            &cut,
            &CanonicalContentLocator::ByLedgerPosition { position: 1 },
            0,
            32,
        )
        .unwrap();
    assert!(page.message_id.is_none());
    let header = &page.headers[0];
    assert!(header.association.call_id.is_none() && header.association.tool_id.is_none());
    assert!(header.name.len() <= 256 && !header.name.is_empty());
    assert_eq!(
        header.arguments_json.as_deref(),
        Some(raw),
        "native raw spelling is never reconstructed from Value"
    );
    assert!(header.association.call.content.iter().any(
        |binding| binding.content_ref.representation == ContentRepresentation::RawJsonUtf8
    ));
    assert!(header.started.is_none() && header.terminal.is_none());
}

#[test]
fn t12_t14_t19_source_cut_provenance_rejects_old_journal_with_new_canonical_frontier() {
    use crate::events::types::RuntimeEvent;
    use crate::runtime_client::content::CanonicalContentLocator;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source-cut.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8c9");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let mut messages = todo_source_messages("native C result before execution J");
    let MessageBlock::Assistant(assistant) = &messages[0] else {
        panic!("Assistant")
    };
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("ToolCall")
    };
    let call_id = call.id.clone();
    let tool_id = call.tool_id.clone();
    store
        .append_canonical_with_event(
            &messages[0],
            tool_observation_event(
                &conversation,
                "owned",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.id.clone(),
                },
            ),
        )
        .unwrap();
    store.append_canonical(&messages[1]).unwrap();
    let before = store.observation_read_cut().unwrap();
    store
        .append_event(tool_observation_event(
            &conversation,
            "execution-start",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id,
                tool_id,
            },
        ))
        .unwrap();
    let page = store
        .transcript_tool_headers(
            &before,
            &CanonicalContentLocator::ByLedgerPosition { position: 1 },
            0,
            32,
        )
        .unwrap();
    assert!(
        page.headers[0].committed_result.is_some(),
        "a genuine no-J canonical result remains readable at its exact older cut"
    );
    assert!(page.headers[0].started.is_none() && page.headers[0].terminal.is_none());
    let MessageBlock::Assistant(assistant) = &mut messages[0] else {
        panic!("Assistant")
    };
    assistant.id = MessageId::new("later-canonical");
    let AssistantContentBlock::ToolCall(call) = &mut assistant.content[0] else {
        panic!("ToolCall")
    };
    call.id = crate::runtime::identity::ToolCallId::new("later-native-call");
    let event = tool_observation_event(
        &conversation,
        "later-owned",
        RuntimeEvent::AssistantMessageCommitted {
            message_id: assistant.id.clone(),
        },
    );
    store
        .append_canonical_with_event(&messages[0], event)
        .unwrap();
    let current = store.observation_read_cut().unwrap();
    let mut forged = current.clone();
    forged.read.journal = before.read.journal;
    assert!(matches!(
        store.message_header_page(&forged, None, 32),
        Err(crate::durable::ConversationStoreError::InvalidReference(_))
    ));
    assert_canonical_cut_dimensions(&store, &before, &current);
    assert_eq!(
        store.message_header_page(&before, None, 32).unwrap().total,
        2
    );
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "DELETE FROM observation_headers WHERE ledger_position=3",
            [],
        )
        .unwrap();
    assert!(
        matches!(
            store.message_header_page(&current, None, 32),
            Err(crate::durable::ConversationStoreError::Storage(_))
        ),
        "missing provenance is unavailable, never recaptured or silently skipped"
    );
}

fn assert_canonical_cut_dimensions(
    store: &SqliteConversationStore,
    before: &crate::durable::observation::ObservationReadCut,
    current: &crate::durable::observation::ObservationReadCut,
) {
    let mut forged_surface = current.clone();
    forged_surface.surface_revision = before.surface_revision;
    forged_surface.read.surface_revision = before.surface_revision;
    assert!(matches!(
        store.message_header_page(&forged_surface, None, 32),
        Err(crate::durable::ConversationStoreError::InvalidReference(_))
    ));
    let mut forged_transcript = current.clone();
    forged_transcript.read.transcript = before.read.transcript;
    assert!(matches!(
        store.message_header_page(&forged_transcript, None, 32),
        Err(crate::durable::ConversationStoreError::InvalidReference(_))
    ));
}

#[test]
fn t08_t15_t20_missing_middle_descriptor_never_skips_native_history() {
    use crate::durable::inbox::ConversationStoreError;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("middle-descriptor.sqlite");
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8ca");
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let messages = (0..3)
        .map(|index| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("middle-{index}")),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: format!("body-{index}"),
                })],
            })
        })
        .collect::<Vec<_>>();
    store.initialize(&[]).unwrap();
    store.append_canonical_batch(&messages).unwrap();
    let cut = store.observation_read_cut().unwrap();
    assert_eq!(
        store
            .message_header_page(&cut, Some(0), 32)
            .unwrap()
            .headers
            .len(),
        3
    );
    assert_eq!(
        store
            .transcript_header_page(&cut, None, 32)
            .unwrap()
            .headers
            .len(),
        3
    );
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "DELETE FROM observation_headers WHERE ledger_position=2",
            [],
        )
        .unwrap();
    for error in [
        store.message_header_page(&cut, Some(0), 32).unwrap_err(),
        store.transcript_header_page(&cut, None, 32).unwrap_err(),
    ] {
        assert!(
            matches!(error, ConversationStoreError::Storage(_)),
            "{error:?}"
        );
    }
    let counts: (u64, u64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM message_ledger),(SELECT COUNT(*) FROM transcript_order)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        counts,
        (3, 3),
        "optional layout corruption cannot remove native C or T"
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    let plan: String = connection.query_row(
        "EXPLAIN QUERY PLAN SELECT t.position,h.header_json FROM transcript_order t LEFT JOIN observation_headers h INDEXED BY observation_headers_transcript_idx ON h.transcript_position=t.position WHERE t.position<4 ORDER BY t.position DESC LIMIT 33",
        [], |row| row.get(3),
    ).unwrap();
    assert!(plan.contains("INTEGER PRIMARY KEY"), "{plan}");
}

#[test]
fn t14_t19_complete_canonical_replacement_carries_earlier_native_effects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("complete-canonical.sqlite");
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8cb"),
        &path,
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let time = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let opening = canonical_complete_opening(time);
    let held = store.append_canonical(&opening).unwrap();
    let todo = store
        .append_canonical_batch(&todo_source_messages("global original Todo"))
        .unwrap();
    let last_todo = todo
        .last()
        .unwrap()
        .observation
        .todos
        .as_ref()
        .unwrap()
        .clone();
    let messages = (0..9)
        .map(|index| canonical_complete_tail(index, time))
        .collect::<Vec<_>>();
    let latest_activity = time + chrono::Duration::seconds(120);
    let later = store.append_canonical_batch(&messages).unwrap();
    let complete = &later.last().unwrap().observation;
    assert_eq!(
        (
            complete.first_position,
            complete.last_position,
            complete.count
        ),
        (4, 12, 9)
    );
    assert_eq!(
        complete
            .head
            .iter()
            .map(|header| header.ledger_position.unwrap())
            .collect::<Vec<_>>(),
        (5..=12).collect::<Vec<_>>()
    );
    assert_global_canonical_scalars(complete, &held.observation, &last_todo, latest_activity);
    assert_eq!(held.observation.read_cut.ledger_through, 1);
    assert_eq!(
        held.observation.head.len(),
        1,
        "old receipt remains its captured complete cut"
    );
    let connection = rusqlite::Connection::open(&path).unwrap();
    let extended_year = chrono::DateTime::from_timestamp(253_402_300_800, 0).unwrap();
    let follow = store
        .append_canonical(&canonical_complete_tail(9, extended_year))
        .unwrap();
    assert_global_canonical_scalars(
        &follow.observation,
        &held.observation,
        &last_todo,
        extended_year + chrono::Duration::seconds(60),
    );
    assert_scalar_source_reads_ignore_original_bodies(&store, &connection, &follow.observation);
    connection
        .execute(
            "DELETE FROM state_observation_sources WHERE domain='todos'",
            [],
        )
        .unwrap();
    let unavailable = store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("after-auxiliary-failure"),
            content: Vec::new(),
        }))
        .unwrap();
    assert!(unavailable.observation.source_unavailable);
    assert_eq!(
        unavailable.ledger_position, 14,
        "missing optional Todo header cannot refuse native C"
    );
    let count: u64 = connection
        .query_row("SELECT COUNT(*) FROM message_ledger", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 14);
    assert_complete_canonical_index_plans(&connection);
}

fn assert_scalar_source_reads_ignore_original_bodies(
    store: &SqliteConversationStore,
    connection: &rusqlite::Connection,
    expected: &crate::durable::inbox::CanonicalObservationBatch,
) {
    connection.execute_batch(
        "CREATE TEMP TABLE native_body_backup AS SELECT position,message_json FROM message_ledger WHERE position IN(1,3); UPDATE message_ledger SET message_json='invalid JSON' WHERE position IN(1,3)",
    ).unwrap();
    assert!(store.canonical_scalar_sources_match(expected).unwrap());
    assert_eq!(
        store
            .message_header_page(&expected.read_cut, None, 8)
            .unwrap()
            .headers
            .len(),
        8,
        "complete Source head and scalar reads never deserialize original bodies",
    );
    connection.execute_batch(
        "UPDATE message_ledger SET message_json=(SELECT message_json FROM native_body_backup b WHERE b.position=message_ledger.position) WHERE position IN(1,3); DROP TABLE native_body_backup",
    ).unwrap();
}

fn canonical_complete_opening(time: chrono::DateTime<chrono::Utc>) -> MessageBlock {
    use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new("global-opening"),
        kind: InboundKind::Message,
        source: UserSource::Human,
        timestamp: Some(time),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "global first preview".into(),
        })],
    })
}
fn canonical_complete_tail(index: u32, time: chrono::DateTime<chrono::Utc>) -> MessageBlock {
    let id = MessageId::new(format!("replacement-tail-{index}"));
    if index >= 7 {
        let mut user = canonical_complete_opening(
            time + chrono::Duration::seconds(if index == 7 { 120 } else { 60 }),
        );
        let MessageBlock::User(value) = &mut user else {
            unreachable!("ordinary User fixture")
        };
        value.id = id;
        return user;
    }
    MessageBlock::Assistant(AssistantMessageBlock {
        id,
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: "later native C".into(),
        })],
    })
}
fn assert_complete_canonical_index_plans(connection: &rusqlite::Connection) {
    for (sql, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT opening_preview FROM message_ledger INDEXED BY message_ledger_opening_idx WHERE ordinary_user=1 AND position<=14 ORDER BY position LIMIT 1",
            "message_ledger_opening_idx",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT human_activity_at FROM message_ledger INDEXED BY message_ledger_human_activity_idx WHERE human_activity_at IS NOT NULL AND position<=14 ORDER BY human_activity_seconds DESC,human_activity_nanos DESC LIMIT 1",
            "message_ledger_human_activity_idx",
        ),
    ] {
        let plan: String = connection.query_row(sql, [], |row| row.get(3)).unwrap();
        assert!(plan.contains(index), "{plan}");
    }
}

fn assert_global_canonical_scalars(
    complete: &crate::durable::inbox::CanonicalObservationBatch,
    first: &crate::durable::inbox::CanonicalObservationBatch,
    todo: &crate::durable::inbox::NativeTodoObservation,
    time: chrono::DateTime<chrono::Utc>,
) {
    assert_eq!(complete.opening_preview, first.opening_preview);
    assert!(complete.has_opening_user);
    assert_eq!(complete.human_activity_at, Some(time));
    assert_eq!(complete.todos.as_ref(), Some(todo));
    assert!(
        !complete.source_unavailable,
        "complete replacement reads bounded scalar/layout sources"
    );
}

#[test]
fn t08_t14_t19_response_decoration_keeps_one_frozen_native_journal_cut() {
    use crate::events::types::RuntimeEvent;
    use crate::runtime_client::snapshot::ForegroundToolState;
    let directory = tempfile::tempdir().unwrap();
    let conversation = ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8f1");
    let store = SqliteConversationStore::open(
        conversation.clone(),
        &directory.path().join("response-cut.sqlite"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let messages = todo_source_messages("original result");
    let MessageBlock::Assistant(assistant) = &messages[0] else {
        panic!("native Assistant");
    };
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("native tool proposal");
    };
    store
        .append_canonical_with_event(
            &messages[0],
            tool_observation_event(
                &conversation,
                "response-cut-c",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.id.clone(),
                },
            ),
        )
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    store
        .append_event(tool_observation_event(
            &conversation,
            "response-cut-start",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .unwrap();
    let after = store.observation_read_cut().unwrap();
    assert!(after.read.journal > before.read.journal);
    assert_eq!(after.ledger_through, before.ledger_through);
    assert_eq!(after.transcript_total, before.transcript_total);
    let read = |cut: &crate::durable::observation::ObservationReadCut| {
        crate::runtime_client::snapshot::transcript_header_view(
            store.transcript_header_page(cut, None, 32).unwrap(),
        )
    };
    let mut old_page = read(&before);
    let mut new_page = read(&after);
    crate::runtime_client::response::decorate_through(&store, &mut old_page, before.read.journal)
        .unwrap();
    crate::runtime_client::response::decorate_window(&store, &mut new_page, after.read.journal)
        .unwrap();
    assert_eq!(old_page.source_cut.as_ref(), Some(&before));
    assert_eq!(new_page.source_cut.as_ref(), Some(&after));
    assert!(matches!(
        old_page.entries[0].tool_calls[0].state,
        ForegroundToolState::Assembled { .. }
    ));
    assert!(matches!(
        new_page.entries[0].tool_calls[0].state,
        ForegroundToolState::Running { .. }
    ));
    let original = old_page.clone();
    for window in [false, true] {
        let result = if window {
            crate::runtime_client::response::decorate_window(
                &store,
                &mut old_page,
                after.read.journal,
            )
        } else {
            crate::runtime_client::response::decorate_through(
                &store,
                &mut old_page,
                after.read.journal,
            )
        };
        assert!(matches!(
            result,
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(
            old_page, original,
            "rejection preserves the entire frozen window"
        );
    }
}
