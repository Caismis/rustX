//! Shared Session Catalog test fixtures: catalogs over temporary product roots,
//! canonical history builders, and lineage helpers. Each child module holds the
//! regressions of one Catalog responsibility.

mod catalog_document;
mod cfg3_identity;
mod cfg3_reservation;
mod compaction_lineage;
mod files;
mod lineage_copies;
mod lineage_cuts;
mod projection;
mod usage;

use std::collections::BTreeSet;
use std::fs;

use super::{
    CatalogCommitError, LineageReadCut, PreparedLineage, SessionCatalog, SessionError, SessionId,
    SessionNodeOrigin, SessionPersistentState,
};
fn fixture_source_locator(
    store: &dyn crate::durable::ConversationStore,
    id: &str,
) -> crate::observation::content::CanonicalContentLocator {
    let position = store
        .message_header(&crate::runtime::identity::MessageId::new(id))
        .unwrap()
        .unwrap()
        .ledger_position
        .unwrap();
    crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
}

use crate::context::assembly::ContextGeneration;
use crate::conversation::SurfaceRevision;
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::local_runtime::CurrentRuntimeConfig;
use crate::message::content::TextBlock;
use crate::message::types::{
    AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantContentBlock,
    AssistantMessageBlock, ContextKind, ContributionEmission, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::catalog::{ModelCapabilities, ModelCompat};
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::snapshot::{ContributionStart, RequestIdentity, RequestSnapshot};
use crate::model::types::ModelProtocol;
use crate::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, MessageId, ToolCallId, ToolId, TurnId,
};

use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

const CONFIG: &str = r#"agent_id = "agent-a"

[context]
reserve_tokens = 1024
keep_recent_tokens = 4096


[agent]
[agent.model]
model = "provider/model"
"#;

fn config() -> CurrentRuntimeConfig {
    CurrentRuntimeConfig::from_toml_slice(CONFIG.as_bytes()).expect("valid test config")
}

fn state() -> SessionPersistentState {
    SessionPersistentState {
        model: Some(config().initial_model().clone()),
        ..SessionPersistentState::from_input(&crate::local_runtime::SessionConfigInput::new(
            std::path::PathBuf::from("/"),
        ))
    }
}

fn text(value: &str) -> UserContentBlock {
    UserContentBlock::Text(TextBlock {
        text: value.to_owned(),
    })
}

fn user(id: &str, value: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![text(value)],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn status(id: &str, value: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![text(value)],
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(
                chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                vec![AgentStatusModuleId::Time],
            )
            .expect("valid Agent Status metadata"),
        )),
        timestamp: None,
    })
}

fn todo_status(id: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![text("Todo status")],
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(
                chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                vec![AgentStatusModuleId::Todo],
            )
            .expect("valid Agent Status metadata"),
        )),
        timestamp: None,
    })
}

fn freeze_session_test_input(
    store: &dyn ConversationStore,
    snapshot: &mut RequestSnapshot,
    contexts: &[MessageBlock],
) {
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    let mut messages = history.messages;
    messages.extend(crate::model::input::canonical_input(contexts));
    snapshot
        .freeze_input(
            store.conversation_id(),
            &crate::model::ModelRequest {
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                messages,
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
                continuation_source: snapshot.continuation_source.clone(),
            },
        )
        .unwrap();
}

fn todo_status_start_snapshot(
    revision: SurfaceRevision,
    message_id: MessageId,
    emission: ContributionEmission,
) -> RequestSnapshot {
    let identity = RequestIdentity {
        attempt_id: AttemptId::new("clone-source-attempt"),
        turn: TurnId::new("clone-source-turn"),
        retry_number: 0,
    };
    let mut snapshot = RequestSnapshot::new(
        identity,
        crate::context::resources::LedgerFrontier((revision).get()),
        "frozen".to_owned(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "provider/model".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        1024,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        vec![message_id.clone()],
    );
    let producer = crate::runtime::identity::ContextContributorIdentity::Native(
        crate::runtime::identity::NativeContextContributor::AgentStatus,
    );
    snapshot
        .context_generation
        .contributors
        .push(crate::context::ContributorGeneration {
            identity: producer.clone(),
            attestation: None,
        });
    snapshot.contributions = vec![ContributionStart {
        message_id,
        producer,
        metadata: ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(
                chrono::DateTime::from_timestamp(0, 0).unwrap(),
                vec![AgentStatusModuleId::Todo],
            )
            .unwrap(),
        ),
        presentation: None,
        emissions: vec![emission],
        opportunities: crate::context::ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    }];
    snapshot
}

fn source_history() -> Vec<MessageBlock> {
    vec![
        user("source-user-a", "A"),
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("source-assistant"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "B".to_owned(),
                }),
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("source-call"),
                    tool_id: ToolId::new("tool-test"),
                    name: "test_tool".to_owned(),
                    arguments: serde_json::json!({"value": 1}),
                }),
            ],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("source-assistant"),
                crate::message::types::ContentBlockIndex::new(1),
            ),
            id: MessageId::new("source-tool-result"),
            tool_call_id: ToolCallId::new("source-call"),
            tool_id: ToolId::new("tool-test"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        }),
        user("source-user-c", "C"),
    ]
}

fn first_session(catalog: &SessionCatalog) -> SessionId {
    catalog
        .document
        .sessions
        .values()
        .min_by_key(|session| session.ordinal)
        .expect("fixture has a Session")
        .id
        .clone()
}

pub(crate) fn open_catalog() -> (TempDir, SessionCatalog, CurrentRuntimeConfig) {
    let directory = tempfile::tempdir().expect("temp directory");
    let config = config();
    let catalog = SessionCatalog::create(directory.path(), &state()).expect("catalog");
    (directory, catalog, config)
}

fn reopen_catalog(root: &std::path::Path) -> SessionCatalog {
    SessionCatalog::open_existing(root)
        .expect("open catalog")
        .expect("catalog exists")
}

fn append_history(
    catalog: &SessionCatalog,
    history: &[MessageBlock],
) -> (
    ConversationId,
    crate::local_runtime::SessionId,
    crate::local_runtime::SessionNodeId,
) {
    let session_id = catalog.list_page(None, 0, 1).unwrap().sessions[0]
        .id
        .clone();
    let (node, _) = catalog.lineage(&session_id, None).expect("root lineage");
    let path = catalog.database_path(&session_id, &node.conversation_id);
    let store =
        SqliteConversationStore::open(node.conversation_id.clone(), &path).expect("root store");
    for message in history {
        store
            .append_canonical(message)
            .expect("append canonical history");
    }
    (node.conversation_id, session_id, node.id)
}

fn store_for(
    catalog: &SessionCatalog,
    session_id: &crate::local_runtime::SessionId,
    conversation_id: &ConversationId,
) -> SqliteConversationStore {
    SqliteConversationStore::open(
        conversation_id.clone(),
        &catalog.database_path(session_id, conversation_id),
    )
    .expect("conversation store")
}

/// Native lineage authority: structural Surface/history at R and temporal
/// provenance at the invocation-time C captured by this read.
fn lineage_at(
    store: &SqliteConversationStore,
    conversation_id: &ConversationId,
    revision: SurfaceRevision,
) -> LineageReadCut {
    assert_eq!(store.conversation_id(), conversation_id);
    store
        .read_lineage_cut(revision)
        .expect("coherent native lineage cut")
}

mod archive_tests;

mod artifact_lineage_tests;
pub(crate) mod deletion_tests;
