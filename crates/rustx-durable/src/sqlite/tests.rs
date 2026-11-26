//! Connection-owner regressions: writer busy waits and presentation-read cancellation.

use crate::sqlite::journal::persist_event_tx;
use crate::sqlite::observation::tests::inspection::{
    inspection_assistant, inspection_request, inspection_result,
};
use crate::sqlite::request::snapshot::{
    REQUEST_SNAPSHOT_READ_WORK, encode_snapshot, load_request_snapshot_bounded,
    read_stored_snapshot_root, require_started_request_tx, require_started_request_with_validation,
    stored_snapshot_digest,
};
use crate::sqlite::test_support::{assert_projection_receipt_head, observation_projection_input};
use crate::sqlite::test_support::{
    commit_original_tool_result, derived_enrichment_proposal, member_completed_lineage,
    original_success_tool_result, original_tool_before_result,
};
use rustx_types::model::finish::ModelFinishReason;
use rustx_types::runtime::identity::ToolId;
use rustx_types::runtime::types::CancellationReason;
use rustx_types::tools::types::{ToolCall, ToolExecutionResult};
mod auxiliary_writer;
mod executor_wait;
mod optional_layout;
mod presentation_cancellation;

use crate::ConversationStore;
use crate::model::snapshot::RequestSnapshot;
use crate::sqlite::*;
use crate::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, PublicationStore, RequestStore,
};
use chrono::Utc;
use rustx_publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use rustx_types::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use rustx_types::message::types::AssistantMessageBlock;
use rustx_types::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use rustx_types::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, ToolCallId, TurnId,
};
use std::sync::Arc;

use crate::sqlite::test_support::*;

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

pub mod observation_response_tests {
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

use crate::InboxStore;
