//! Explicit diagnostic census of known live owners; never storage/read authority.
use super::{RuntimeClientEvent, RuntimeClientProjection};
use crate::observation::cut::{CheckpointReadCut, CollectionFrontier, ContentBinding};
use crate::observation::metadata::MetadataFragment;
use crate::observation::transcript::{RuntimeClientMessageDescriptor, RuntimeClientTranscriptPage};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::mem::{size_of, size_of_val};
use std::sync::Arc;

const ARC_HEADER: usize = 2 * size_of::<usize>();

fn event_variant(event: &RuntimeClientEvent) -> &'static str {
    match event {
        RuntimeClientEvent::ProjectionTransition { .. } => "variant_ProjectionTransition",
        RuntimeClientEvent::ProjectionDelta { .. } => "variant_ProjectionDelta",
        RuntimeClientEvent::CheckpointPatched { .. } => "variant_CheckpointPatched",
        RuntimeClientEvent::ReadDomainsPatched { .. } => "variant_ReadDomainsPatched",
        RuntimeClientEvent::MessageDescriptorPatched { .. } => "variant_MessageDescriptorPatched",
        RuntimeClientEvent::CheckpointAdvanced { .. } => "variant_CheckpointAdvanced",
        RuntimeClientEvent::BlockContentAdvanced { .. } => "variant_BlockContentAdvanced",
        RuntimeClientEvent::ReadDomainsUpdated { .. } => "variant_ReadDomainsUpdated",
        RuntimeClientEvent::TraceChanged => "variant_TraceChanged",
        RuntimeClientEvent::GoalChanged { .. } => "variant_GoalChanged",
        RuntimeClientEvent::WorkflowsUpdated { .. } => "variant_WorkflowsUpdated",
        RuntimeClientEvent::AttemptStarted { .. } => "variant_AttemptStarted",
        RuntimeClientEvent::AttemptSettled { .. } => "variant_AttemptSettled",
        RuntimeClientEvent::AttemptTurnUpdated { .. } => "variant_AttemptTurnUpdated",
        RuntimeClientEvent::AttemptUsageUpdated { .. } => "variant_AttemptUsageUpdated",
        RuntimeClientEvent::PendingHeadChanged { .. } => "variant_PendingHeadChanged",
        RuntimeClientEvent::InteractionPending { .. } => "variant_InteractionPending",
        RuntimeClientEvent::InteractionSettled { .. } => "variant_InteractionSettled",
        RuntimeClientEvent::InteractionRemoved { .. } => "variant_InteractionRemoved",
        RuntimeClientEvent::InteractionAuditRequested { .. } => "variant_InteractionAuditRequested",
        RuntimeClientEvent::InteractionAuditSettled { .. } => "variant_InteractionAuditSettled",
        RuntimeClientEvent::ConfigurationApplied { .. } => "variant_ConfigurationApplied",
        RuntimeClientEvent::ContextCompactionStarted { .. } => "variant_ContextCompactionStarted",
        RuntimeClientEvent::ContextCompactionFailed { .. } => "variant_ContextCompactionFailed",
        RuntimeClientEvent::ContextCompacted { .. } => "variant_ContextCompacted",
        RuntimeClientEvent::AssistantMessageStarted { .. } => "variant_AssistantMessageStarted",
        RuntimeClientEvent::ToolCallStarted { .. } => "variant_ToolCallStarted",
        RuntimeClientEvent::ToolCallAssembled { .. } => "variant_ToolCallAssembled",
        RuntimeClientEvent::AssistantPublicationSettled { .. } => {
            "variant_AssistantPublicationSettled"
        }
        RuntimeClientEvent::AssistantPublicationDeferredSettled { .. } => {
            "variant_AssistantPublicationDeferredSettled"
        }
        RuntimeClientEvent::ToolOccurrenceRebound { .. } => "variant_ToolOccurrenceRebound",
        RuntimeClientEvent::ToolResultCommitted { .. } => "variant_ToolResultCommitted",
        RuntimeClientEvent::ToolExecutionStarted { .. } => "variant_ToolExecutionStarted",
        RuntimeClientEvent::ToolExecutionProgress { .. } => "variant_ToolExecutionProgress",
        RuntimeClientEvent::ToolExecutionSettled { .. } => "variant_ToolExecutionSettled",
        RuntimeClientEvent::MessageDescriptorCommitted { .. } => {
            "variant_MessageDescriptorCommitted"
        }
        RuntimeClientEvent::MessageCommitted { .. } => "variant_MessageCommitted",
        RuntimeClientEvent::AgentStatusComposed { .. } => "variant_AgentStatusComposed",
        RuntimeClientEvent::PendingInboundChanged { .. } => "variant_PendingInboundChanged",
        RuntimeClientEvent::InboundEnqueued { .. } => "variant_InboundEnqueued",
        RuntimeClientEvent::InboundDrained { .. } => "variant_InboundDrained",
        RuntimeClientEvent::CollectionChanged { .. } => "variant_CollectionChanged",
        RuntimeClientEvent::JobUpdated { .. } => "variant_JobUpdated",
        RuntimeClientEvent::AgentUpdated { .. } => "variant_AgentUpdated",
        RuntimeClientEvent::CapabilityUpdated { .. } => "variant_CapabilityUpdated",
        RuntimeClientEvent::ResourceGenerationUpdated { .. } => "variant_ResourceGenerationUpdated",
        RuntimeClientEvent::SessionModelChanged { .. } => "variant_SessionModelChanged",
        RuntimeClientEvent::RuntimeShutdown => "variant_RuntimeShutdown",
        RuntimeClientEvent::RuntimeDurabilityFailed { .. } => "variant_RuntimeDurabilityFailed",
    }
}

#[derive(Default, Serialize)]
struct Allocation {
    owners: usize,
    known_requested_bytes: usize,
}
#[derive(Default, Serialize)]
struct Domain {
    instances: usize,
    serde_bytes: usize,
    unique_values: usize,
    unique_serde_bytes: usize,
    #[serde(skip)]
    hashes: HashSet<[u8; 32]>,
}
#[derive(Default, Serialize)]
struct Census {
    allocations: BTreeMap<&'static str, Allocation>,
    counts: BTreeMap<&'static str, usize>,
    logical_domains: BTreeMap<&'static str, Domain>,
    #[serde(skip)]
    seen: HashSet<(&'static str, usize)>,
}
#[derive(Default)]
struct Encoded {
    hash: Sha256,
    bytes: usize,
}
impl std::io::Write for Encoded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hash.update(bytes);
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Census {
    fn count(&mut self, key: &'static str, amount: usize) {
        *self.counts.entry(key).or_default() += amount;
    }
    fn bytes(&mut self, key: &'static str, bytes: usize) {
        self.allocations
            .entry(key)
            .or_default()
            .known_requested_bytes += bytes;
    }
    fn owner(&mut self, key: &'static str, pointer: usize, bytes: usize) -> bool {
        if !self.seen.insert((key, pointer)) {
            return false;
        }
        self.allocations.entry(key).or_default().owners += 1;
        self.bytes(key, bytes);
        true
    }
    fn vector<T>(&mut self, key: &'static str, value: &Vec<T>) -> bool {
        value.capacity() > 0
            && size_of::<T>() > 0
            && self.owner(
                key,
                value.as_ptr() as usize,
                value.capacity() * size_of::<T>(),
            )
    }
    fn fragment<T>(&mut self, key: &'static str, value: &MetadataFragment<T>) -> bool {
        let (pointer, bytes) = value.allocation_identity();
        self.owner(key, pointer, bytes)
    }
    fn paths(&mut self, binding: &ContentBinding) {
        self.owner(
            "content_ref_owners",
            Arc::as_ptr(&binding.content_ref) as usize,
            size_of::<crate::observation::content::ContentRef>() + ARC_HEADER,
        );
        for path in std::iter::once(&binding.path).chain(binding.source_path.iter()) {
            if self.vector("binding_path_vectors", path) {
                self.bytes(
                    "binding_path_strings",
                    path.iter().map(String::capacity).sum(),
                );
            }
        }
    }
    fn binding(&mut self, value: &MetadataFragment<ContentBinding>) {
        if self.fragment("binding_fragments", value) {
            self.paths(value);
            if let Some(alternate) = value.prepared_alternate() {
                self.binding(alternate);
            }
        }
    }
    fn plain_bindings(&mut self, values: &Vec<ContentBinding>) {
        if self.vector("raw_binding_vectors", values) {
            for value in values {
                self.paths(value);
            }
        }
    }
    fn plain_frontiers(&mut self, values: &Vec<CollectionFrontier>) {
        if self.vector("raw_frontier_vectors", values) {
            self.bytes(
                "frontier_domain_strings",
                values.iter().map(|v| v.domain.capacity()).sum(),
            );
        }
    }
    fn content(&mut self, values: &Arc<Vec<MetadataFragment<ContentBinding>>>) {
        if self.owner(
            "content_array_owners",
            Arc::as_ptr(values) as usize,
            size_of::<Vec<usize>>() + ARC_HEADER,
        ) {
            self.vector("content_pointer_vectors", values);
            for value in values.iter() {
                self.binding(value);
            }
        }
    }
    fn collections(&mut self, values: &Arc<Vec<MetadataFragment<CollectionFrontier>>>) {
        if self.owner(
            "frontier_array_owners",
            Arc::as_ptr(values) as usize,
            size_of::<Vec<usize>>() + ARC_HEADER,
        ) {
            self.vector("frontier_pointer_vectors", values);
            for value in values.iter() {
                if self.fragment("frontier_fragments", value) {
                    self.bytes("frontier_domain_strings", value.domain.capacity());
                }
            }
        }
    }
    fn cut(&mut self, cut: &CheckpointReadCut) {
        if self.fragment("domain_revision_roots", &cut.domain_revisions) {
            self.count(
                "domain_revision_logical_entries",
                cut.domain_revisions.len(),
            );
        }
        if self.fragment("collection_cut_roots", &cut.collection_cuts) {
            self.count("collection_cut_logical_entries", cut.collection_cuts.len());
        }
    }
    fn domain(&mut self, name: &'static str, value: &impl Serialize) {
        let mut encoded = Encoded::default();
        if serde_json::to_writer(&mut encoded, value).is_err() {
            self.count("domain_serialization_errors", 1);
            return;
        }
        let domain = self.logical_domains.entry(name).or_default();
        domain.instances += 1;
        domain.serde_bytes += encoded.bytes;
        if domain.hashes.insert(encoded.hash.finalize().into()) {
            domain.unique_values += 1;
            domain.unique_serde_bytes += encoded.bytes;
        }
    }
    fn descriptor(&mut self, row: &Arc<RuntimeClientMessageDescriptor>) {
        if self.owner(
            "descriptor_rows",
            Arc::as_ptr(row) as usize,
            size_of::<RuntimeClientMessageDescriptor>() + ARC_HEADER,
        ) {
            self.plain_bindings(&row.content);
            self.plain_frontiers(&row.collections);
            self.bytes(
                "descriptor_strings",
                row.role.capacity() + row.kind.capacity(),
            );
            self.domain("unique_descriptor_rows", row);
        }
    }
    fn transcript_entries(
        &mut self,
        entries: &MetadataFragment<
            Vec<Arc<crate::observation::transcript::RuntimeClientTranscriptEntry>>,
        >,
    ) {
        if self.fragment("transcript_head_owners", entries) {
            self.vector("transcript_pointer_vectors", entries);
            for row in entries {
                if self.owner(
                    "transcript_rows",
                    Arc::as_ptr(row) as usize,
                    size_of_val(row.as_ref()) + ARC_HEADER,
                ) {
                    self.domain("unique_transcript_rows", row);
                }
            }
        }
    }
    fn transcript(&mut self, page: &RuntimeClientTranscriptPage) {
        self.transcript_entries(&page.entries);
        if self.fragment("transcript_binding_heads", &page.content) {
            self.plain_bindings(&page.content);
        }
        if self.fragment("transcript_frontier_heads", &page.collections) {
            self.plain_frontiers(&page.collections);
        }
    }
    fn status(&mut self, value: &crate::observation::status::AgentStatusView) {
        self.count("status_references", 1);
        // Inline rows are charged by their containing Vec or event envelope.
        // Count their actual owned buffers separately, without a second debit.
        if !self.owner("status_values", std::ptr::from_ref(value) as usize, 0) {
            return;
        }
        self.vector("status_section_vectors", &value.sections);
        if value.rendered.capacity() > 0 {
            self.owner(
                "status_rendered_strings",
                value.rendered.as_ptr() as usize,
                value.rendered.capacity(),
            );
        }
        self.domain("individual_status_values", value);
    }
    #[allow(clippy::too_many_lines)] // Closed event ownership census.
    fn event(&mut self, event: &RuntimeClientEvent) {
        self.count("event_values", 1);
        self.count(event_variant(event), 1);
        match event {
            RuntimeClientEvent::ProjectionTransition { events, metadata } => {
                self.vector("nested_event_vectors", events);
                self.cut(&metadata.read_cut);
                self.content(&metadata.content);
                self.collections(&metadata.collections);
                for event in events {
                    self.event(event);
                }
            }
            RuntimeClientEvent::ProjectionDelta { events, metadata } => {
                self.vector("nested_event_vectors", events);
                if let Some(complete) = &metadata.native_transition
                    && self.owner(
                        "bootstrap_transition_owners",
                        Arc::as_ptr(complete) as usize,
                        size_of_val(complete.as_ref()),
                    )
                {
                    self.event(complete);
                }
                if let Some(cut) = &metadata.native_cut {
                    self.cut(cut);
                }
                self.domain("delta_content", &metadata.content);
                self.domain("delta_collections", &metadata.collections);
                for event in events {
                    self.event(event);
                }
            }
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                self.count("replacement_checkpoints", 1);
                self.owner(
                    "checkpoint_boxes",
                    std::ptr::from_ref(checkpoint.as_ref()) as usize,
                    size_of_val(checkpoint.as_ref()),
                );
                self.cut(&checkpoint.read_cut);
                self.content(&checkpoint.content);
                self.collections(&checkpoint.collections);
                let snapshot = &checkpoint.snapshot;
                self.vector("descriptor_pointer_vectors", &snapshot.message_descriptors);
                for row in &snapshot.message_descriptors {
                    self.descriptor(row);
                }
                self.transcript(&snapshot.transcript);
                self.vector("checkpoint_status_vectors", &snapshot.statuses);
                for status in &snapshot.statuses {
                    self.status(status);
                }
                self.domain("checkpoint_messages", &snapshot.messages);
                self.domain("checkpoint_model", &snapshot.model);
                self.domain("checkpoint_capabilities", &snapshot.capabilities);
                self.domain("checkpoint_context", &snapshot.context);
                self.domain("checkpoint_trace_records", &snapshot.trace.records);
                self.domain("checkpoint_attempt", &snapshot.attempt);
                self.domain("checkpoint_resources", &snapshot.resources);
                self.domain("checkpoint_effective_plugins", &snapshot.effective_plugins);
            }
            RuntimeClientEvent::ReadDomainsUpdated { transcript, .. } => {
                self.owner(
                    "transcript_page_boxes",
                    std::ptr::from_ref(transcript.as_ref()) as usize,
                    size_of_val(transcript.as_ref()),
                );
                self.transcript(transcript);
                self.count("read_domain_events", 1);
            }
            RuntimeClientEvent::BlockContentAdvanced {
                inline_delta, kind, ..
            } => {
                self.bytes(
                    "event_strings",
                    kind.capacity() + inline_delta.as_ref().map_or(0, String::capacity),
                );
            }
            RuntimeClientEvent::AgentStatusComposed { status, .. } => {
                self.owner(
                    "status_event_boxes",
                    std::ptr::from_ref(status.as_ref()) as usize,
                    size_of_val(status.as_ref()),
                );
                self.status(status);
            }
            RuntimeClientEvent::ToolExecutionSettled { result, .. } => {
                self.owner(
                    "tool_result_boxes",
                    std::ptr::from_ref(result.as_ref()) as usize,
                    size_of_val(result.as_ref()),
                );
            }
            RuntimeClientEvent::ToolCallAssembled { call, .. } => {
                self.owner(
                    "tool_call_boxes",
                    std::ptr::from_ref(call.as_ref()) as usize,
                    size_of_val(call.as_ref()),
                );
            }
            RuntimeClientEvent::ContextCompactionStarted { context, .. }
            | RuntimeClientEvent::ContextCompactionFailed { context, .. }
            | RuntimeClientEvent::ContextCompacted { context, .. } => {
                self.owner(
                    "context_event_boxes",
                    std::ptr::from_ref(context.as_ref()) as usize,
                    size_of_val(context.as_ref()),
                );
            }
            _ => self.count("other_event_values", 1),
        }
    }
}
impl RuntimeClientProjection {
    pub(crate) fn retention_profile(&self) -> serde_json::Value {
        let mut census = Census::default();
        census.count("replay_records", self.replay.len());
        census.count("replay_encoded_charge", self.replay_bytes);
        for (_, event) in &self.replay {
            if census.owner(
                "replay_event_arcs",
                Arc::as_ptr(event) as usize,
                size_of::<RuntimeClientEvent>() + ARC_HEADER,
            ) {
                census.event(event);
            }
        }
        let mut output = serde_json::to_value(&census).expect("finite retention census");
        output["known_requested_bytes"] = census
            .allocations
            .values()
            .map(|value| value.known_requested_bytes)
            .sum::<usize>()
            .into();
        output["event_inline_bytes"] = size_of::<RuntimeClientEvent>().into();
        output["scope"] = "Replay-reachable unique Arc/Vec/Box owners, metadata paths and shallow rows, including status row vectors, section vectors and rendered strings. Excludes nested status-section bodies, persistent-map nodes, other nested row/tool/model payloads, IDs, prepared caches, working/current seeds, allocator rounding and SQLite. Inline status rows are charged only through their containing vector or event. Domain serde bytes are logical repetition, not heap bytes.".into();
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ContentBinding {
        ContentBinding {
            source_path: None,
            path: vec!["messages".into(), "0".into(), "display".into()],
            content_ref: crate::observation::content::ContentRef::configuration_instructions(
                crate::runtime::identity::ConversationId::generate(),
                1,
                3,
            )
            .into(),
            deferred: false,
        }
    }

    #[test]
    fn shared_references_and_equal_distinct_owners_are_counted_separately() {
        let raw = binding();
        let left: MetadataFragment<_> = raw.clone().into();
        let shared = left.clone();
        let right = raw.into();
        let mut census = Census::default();
        census.binding(&left);
        census.binding(&shared);
        assert_eq!(census.allocations["binding_fragments"].owners, 1);
        census.binding(&right);
        assert_eq!(census.allocations["binding_fragments"].owners, 2);
        assert_eq!(census.allocations["binding_path_vectors"].owners, 2);
        assert_eq!(census.allocations["content_ref_owners"].owners, 1);
    }

    #[test]
    fn a_prepared_alternate_is_counted_once_without_changing_its_owner() {
        let original: MetadataFragment<_> = binding().into();
        let mut reference = original.content_ref.clone();
        std::sync::Arc::make_mut(&mut reference).content_revision += 1;
        let alternate = original.resolve(&reference);
        let mut census = Census::default();
        census.binding(&original);
        census.binding(&alternate);
        assert_eq!(census.allocations["binding_fragments"].owners, 2);
        assert!(
            original
                .prepared_alternate()
                .unwrap()
                .same_owner(&alternate)
        );
    }

    #[test]
    fn logical_duplicates_are_not_charged_as_physical_owners() {
        let mut census = Census::default();
        census.domain("field", &vec!["same"]);
        census.domain("field", &vec!["same"]);
        assert_eq!(census.logical_domains["field"].instances, 2);
        assert_eq!(census.logical_domains["field"].unique_values, 1);
        assert!(census.allocations.is_empty());
    }

    #[test]
    fn equal_owned_status_rows_are_counted_without_double_charging_inline_storage() {
        let view = crate::observation::status::AgentStatusView {
            attempt_id: crate::runtime::identity::AttemptId::new("status-owner-attempt"),
            turn: 1,
            status_message_id: None,
            source_locator: None,
            request_id: None,
            rendered_ref: None,
            opportunities: crate::observation::status::AgentStatusOpportunityView {
                fresh_inbound: None,
                post_tool_batch: None,
            },
            sections: Vec::new(),
            rendered: "status body".into(),
        };
        let rows = vec![view.clone(), view];
        let mut census = Census::default();
        census.vector("checkpoint_status_vectors", &rows);
        census.status(&rows[0]);
        census.status(&rows[0]);
        census.status(&rows[1]);
        assert_eq!(census.allocations["status_values"].owners, 2);
        assert_eq!(census.allocations["status_values"].known_requested_bytes, 0);
        assert_eq!(census.allocations["status_rendered_strings"].owners, 2);
        assert_eq!(
            census.logical_domains["individual_status_values"].unique_values,
            1
        );
        assert_eq!(census.counts["status_references"], 3);
    }

    #[test]
    fn event_payload_boxes_are_included_and_shared_visits_do_not_duplicate_them() {
        let event = RuntimeClientEvent::ContextCompactionStarted {
            context: Box::default(),
            attempt_id: None,
        };
        let mut census = Census::default();
        census.event(&event);
        census.event(&event);
        let allocation = &census.allocations["context_event_boxes"];
        assert_eq!(allocation.owners, 1);
        assert_eq!(
            allocation.known_requested_bytes,
            size_of::<crate::runtime_client::snapshot::RuntimeClientContextView>()
        );
        assert_eq!(census.counts["variant_ContextCompactionStarted"], 2);
    }
}
