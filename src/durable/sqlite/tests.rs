//! Connection-owner regressions: writer busy waits and presentation-read cancellation.

use crate::durable::sqlite::journal::persist_event_tx;
use crate::durable::sqlite::observation::tests::inspection::{
    inspection_assistant, inspection_request, inspection_result,
};
use crate::durable::sqlite::observation::tests::member::{
    commit_original_tool_result, derived_enrichment_proposal, member_completed_lineage,
    original_success_tool_result, original_tool_before_result,
};
use crate::durable::sqlite::observation::tests::projection::{
    assert_projection_receipt_head, observation_projection_input,
};
use crate::durable::sqlite::request::snapshot::{
    REQUEST_SNAPSHOT_READ_WORK, encode_snapshot, load_request_snapshot_bounded,
    read_stored_snapshot_root, require_started_request_tx, require_started_request_with_validation,
    stored_snapshot_digest,
};
use crate::model::ModelFinishReason;
use crate::runtime::ToolId;
use crate::runtime::types::CancellationReason;
use crate::tools::types::{ToolCall, ToolExecutionResult};
mod auxiliary_writer;
mod executor_wait;
mod optional_layout;
mod presentation_cancellation;

use crate::durable::ConversationStore;
use crate::durable::sqlite::*;
use crate::durable::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, PublicationStore, RequestStore,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::AssistantMessageBlock;
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, ToolCallId, TurnId,
};
use chrono::Utc;
use std::sync::Arc;

use crate::durable::sqlite::test_support::*;

mod attempt_start_tests {
    use super::*;
    include!("attempt_start_tests.rs");
}

mod event_insert_tests {
    use super::*;
    include!("event_insert_tests.rs");
}

mod append_facts_tests {
    use super::*;
    include!("append_facts_tests.rs");
}

pub(super) mod observation_response_tests {
    use super::*;
    include!("observation_response_tests.rs");
}

mod native_readiness_tests {
    use super::*;
    include!("native_readiness_tests.rs");
}

mod final_settlement_tests {
    use super::*;
    include!("final_settlement_tests.rs");
}

mod native_request_tests {
    use super::*;
    include!("native_request_tests.rs");
}

include!("observation_statistics_history_tests.rs");
include!("observation_occupancy_tests.rs");

use crate::durable::InboxStore;
