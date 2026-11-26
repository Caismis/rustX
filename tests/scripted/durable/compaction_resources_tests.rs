//! Compaction resource accounting suites (context-owned resources).

mod tests {
    use crate::context::resources::CompactionResources;
    use crate::durable::ConversationStoreError;
    use crate::durable::context_resources::*;
    use crate::durable::{CanonicalStore, JournalStore};
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use rustx_identity::ConversationId;
    use rustx_identity::MessageId;
    use rustx_types::events::types::EVENT_SCHEMA_VERSION;
    use rustx_types::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use rustx_types::message::types::MessageBlock;
    use rustx_types::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind,
        UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx_types::runtime::identity::{
        ArtifactId, AttemptId, EventId, ToolCallId, ToolId, TurnId,
    };
    use rustx_types::tools::types::ToolCall;
    fn fact(
        store: &SqliteConversationStore,
        id: &str,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(id),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(AttemptId::new("owner")),
            turn_id: Some(TurnId::new("step")),
            timestamp: chrono::Utc::now(),
            event,
        }
    }
    #[test]
    fn detached_trajectory_reads_physical_occurrence_and_lifecycle_only_as_of_cut() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let message = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("wire"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            })],
        });
        store
            .append_canonical_with_event(
                &message,
                fact(
                    &store,
                    "commit",
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: message.id().clone(),
                    },
                ),
            )
            .unwrap();
        store
            .append_event(fact(
                &store,
                "started",
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("wire"),
                    tool_id: ToolId::new("tool"),
                },
            ))
            .unwrap();
        // Capturing this cut is the SQL read linearization point. Complete
        // afterward, then deliberately delay the first trajectory page read.
        let mut resources =
            CompactionResources::capture(&store, ResourceLimits::default()).unwrap();
        store
            .append_event(fact(
                &store,
                "future",
                RuntimeEvent::ToolExecutionFailed {
                    tool_call_id: ToolCallId::new("wire"),
                    tool_id: ToolId::new("tool"),
                    error: "future terminal".into(),
                },
            ))
            .unwrap();
        let page = resources.trajectory(LedgerFrontier(0)).unwrap();
        let row = &page.rows[0].1;
        assert_eq!(row.message, message);
        assert!(row.transcript_position.is_some());
        assert_eq!(
            row.committed.as_ref().unwrap().attempt_id,
            Some(AttemptId::new("owner"))
        );
        assert_eq!(
            row.tools[0].occurrence,
            rustx_types::message::types::ToolCallOccurrenceRef::new(
                message.id().clone(),
                ContentBlockIndex::new(0)
            )
        );
        assert_eq!(row.tools[0].facts.len(), 1);
        assert!(matches!(
            row.tools[0].facts[0].event,
            RuntimeEvent::ToolExecutionStarted { .. }
        ));
        let now = CompactionResources::capture(&store, ResourceLimits::default())
            .unwrap()
            .trajectory(LedgerFrontier(0))
            .unwrap();
        assert_eq!(now.rows[0].1.tools[0].facts.len(), 2);
        // Mutating the detached working copy never touches source facts.
        let mut working = page.rows.into_iter().next().unwrap().1;
        working.tools.clear();
        assert_eq!(
            store.load_messages(&[message.id().clone()]).unwrap(),
            vec![message]
        );
    }
    #[test]
    fn artifact_read_requires_frozen_source_authority_settlement_and_byte_admission() {
        use std::io::Write as _;
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let artifacts = crate::durable::artifacts::ArtifactStore::new(
            store.conversation_id().clone(),
            directory.path(),
        )
        .unwrap();
        let id = artifacts.create_artifact().unwrap();
        {
            let mut writer = artifacts.open_writer(&id).unwrap();
            writer.write_all(&vec![b'x'; 1024]).unwrap();
            writer.finish().unwrap();
        }
        let message = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("image-source"),
            content: vec![UserContentBlock::Image(
                rustx_types::message::content::ImageReference {
                    artifact_id: id.clone(),
                    alt: None,
                },
            )],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        store.append_canonical(&message).unwrap();
        let mut bounded = CompactionResources::capture(
            &store,
            ResourceLimits {
                record_bytes: 512,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        assert!(matches!(
            bounded.artifact(&artifacts, message.id(), &id),
            Err(ConversationStoreError::ResourceBoundExceeded(_))
        ));
        let mut allowed = CompactionResources::capture(&store, ResourceLimits::default()).unwrap();
        assert_eq!(
            allowed
                .artifact(&artifacts, message.id(), &id)
                .unwrap()
                .len(),
            1024
        );
        assert!(
            allowed
                .artifact(&artifacts, message.id(), &ArtifactId::new("unreferenced"))
                .is_err()
        );
        let mut frozen = CompactionResources::capture(&store, ResourceLimits::default()).unwrap();
        let mut later = message.clone();
        let MessageBlock::User(user) = &mut later else {
            unreachable!()
        };
        user.id = MessageId::new("after-cut");
        store.append_canonical(&later).unwrap();
        assert!(frozen.artifact(&artifacts, later.id(), &id).is_err());
        let mut external = std::fs::OpenOptions::new()
            .write(true)
            .open(directory.path().join(format!("{}.bin", id.as_str())))
            .unwrap();
        external.write_all(&[b'y'; 1024]).unwrap();
        assert_eq!(external.metadata().unwrap().len(), 1024);
        let mut tampered = CompactionResources::capture(&store, ResourceLimits::default()).unwrap();
        assert!(tampered.artifact(&artifacts, message.id(), &id).is_err());
    }
}
