//! `SQLite` durability and recovery boundary tests.
//!
//! Every suite here speaks to the real file-backed durable authority. The
//! crash boundary is a store `drop` and the reopen is a
//! `SqliteConversationStore::open`; recovery classification runs the real
//! recovery pipeline. There is no sleep, no timer, and no timing assumption:
//! an exact committed prefix is the synchronization.
//!
//! These suites prove the store/recovery contract at the `SQLite` boundary.
//! The Agent-Loop-facing halves of the same invariants (publication
//! coalescing, interaction settlement, terminal uniqueness) are owned by the
//! in-crate scripted suites and are not re-proven here.

#![allow(clippy::too_many_lines)] // deterministic store scenarios stay linear

mod interaction_audit;
#[path = "../common/launch.rs"]
mod launch_fixture;
mod pending_inbound;
mod publication;
mod recovery;
mod transcript_history;

#[path = "../common/identity.rs"]
mod identity_fixture;

#[path = "../common/agent_authority.rs"]
mod agent_authority;

/// Write a typed checkpoint through the actual durable grant/CAS/receipt contract.
fn checkpoint(
    store: &dyn rustx::durable::ConversationStore,
    id: &str,
    retired: usize,
    text: &str,
) -> rustx::durable::projection::ProjectionCommitReceipt {
    use rustx::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
    use rustx::model::projected::{
        CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
        SemanticTransformation,
    };
    let history = store.projected_history().unwrap();
    assert!(retired <= history.messages.len());
    let mut items = vec![CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![rustx::message::types::UserContentBlock::Text(
                    rustx::message::TextBlock { text: text.into() },
                )],
                source: rustx::message::types::UserSource::Runtime,
                kind: rustx::model::projected::ProjectedUserKind::Message,
                timestamp: None,
            },
            sources: history
                .messages
                .iter()
                .take(retired)
                .filter_map(|m| m.as_canonical())
                .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
                .collect(),
            checkpoint_sources: store
                .projection_head()
                .unwrap()
                .filter(|_| {
                    history
                        .messages
                        .iter()
                        .take(retired)
                        .any(|m| matches!(m, rustx::model::ModelInputMessage::Derived(_)))
                })
                .map(|p| CheckpointSourceRef {
                    conversation_id: store.conversation_id().clone(),
                    checkpoint_id: p.candidate.checkpoint_id,
                    content_digest: p.fingerprint,
                })
                .into_iter()
                .collect(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: vec![],
        result_call: None,
    }];
    items.extend(history.items.into_iter().skip(retired));
    let operation_id =
        rustx::runtime::identity::CompactionOperationId::new(format!("operation:{id}"));
    store
        .commit_projection(rustx::durable::projection::ProjectionCommitInput {
            grant: rustx::runtime::CancellationSignal::new()
                .authorize_compaction_commit(operation_id.clone())
                .unwrap(),
            operation_id,
            candidate: CheckpointCandidate {
                checkpoint_id: rustx::runtime::identity::CheckpointId::new(id),
                expected_head: history.cut.checkpoint_id,
                read_cut: store.compaction_resource_cut().unwrap(),
                base_frontier: history.cut.ledger,
                items,
                summary_metadata: None,
                summary_generation: None,
            },
            owner: store.projection_owner(None).unwrap(),
            tokens_before: rustx::runtime::TokenMeasurement {
                input_tokens: 20,
                source: rustx::runtime::TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 4,
            occupancy: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
        })
        .unwrap()
}

// Fixtures freeze the same typed input that the actual-start boundary validates.
fn freeze_request(
    store: &dyn rustx::durable::ConversationStore,
    snapshot: &mut rustx::model::RequestSnapshot,
) {
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            store.conversation_id(),
            &rustx::model::ModelRequest {
                response_byte_limit: None,
                messages: history.messages.clone(),
                images: rustx::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
                continuation_source: snapshot.continuation_source.clone(),
            },
        )
        .unwrap();
}
