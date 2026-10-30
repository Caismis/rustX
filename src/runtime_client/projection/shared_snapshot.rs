//! Internal copy-on-write domains. Published seeds share immutable values;
//! any mutable domain access detaches only that domain from issued snapshots.
use super::super::snapshot::{
    AgentStatusView, CapabilityView, InboundDiagnostics, RuntimeClientAgent, RuntimeClientAttempt,
    RuntimeClientContextView, RuntimeClientInteractionHeader, RuntimeClientJob,
    RuntimeClientMessageDescriptor, RuntimeClientModelStatus,
    RuntimeClientPublicationSettlementRecord, RuntimeClientResourcesView, RuntimeClientSnapshot,
    RuntimeClientTranscriptCursor, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage,
    RuntimeDurabilityFailure,
};
use crate::message::types::MessageBlock;
use crate::model::session::SessionModelView;
use crate::runtime::identity::ConversationId;
use crate::runtime::types::ApprovalMode;
use crate::tools::todo::TodoSnapshot;
use serde::{Serialize, Serializer};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct CowDomain<T>(Arc<T>);
impl<T> Clone for CowDomain<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T> From<T> for CowDomain<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(value))
    }
}
impl<T: Default> Default for CowDomain<T> {
    fn default() -> Self {
        T::default().into()
    }
}
impl<T> Deref for CowDomain<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone> DerefMut for CowDomain<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}
impl<T> CowDomain<T> {
    pub(super) fn value(&self) -> &T {
        &self.0
    }
    pub(super) fn owner(&self) -> &Arc<T> {
        &self.0
    }
    pub(super) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T: Clone> CowDomain<T> {
    pub(crate) fn owned(&self) -> T {
        self.0.as_ref().clone()
    }
    pub(super) fn replace_from(&mut self, value: &T) {
        *self = value.clone().into();
    }
}
impl<T: PartialEq> PartialEq for CowDomain<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T: PartialEq> PartialEq<T> for CowDomain<T> {
    fn eq(&self, other: &T) -> bool {
        self.0.as_ref() == other
    }
}
impl<T: Eq> Eq for CowDomain<T> {}
impl<T: Serialize> Serialize for CowDomain<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<T: schemars::JsonSchema> schemars::JsonSchema for CowDomain<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        T::schema_id()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}
impl<'a, T> IntoIterator for &'a CowDomain<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.value().into_iter()
    }
}
impl<'a, T: Clone> IntoIterator for &'a mut CowDomain<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SharedSnapshot {
    pub(crate) settings_evidence: super::super::settings::SettingsEvidence,
    pub(crate) effective_plugins: CowDomain<Option<super::super::settings::EffectivePlugins>>,
    pub(crate) workflows: CowDomain<crate::runtime::workflow::read_model::WorkflowSnapshot>,
    pub(crate) conversation_id: ConversationId,
    pub(crate) shutting_down: bool,
    pub(crate) effective_approval_mode: ApprovalMode,
    pub(crate) configuration_adoption_eligibility:
        crate::local_runtime::configuration::application::AdoptionEligibility,
    pub(crate) durability_failure: CowDomain<Option<RuntimeDurabilityFailure>>,
    pub(crate) messages: CowDomain<Vec<MessageBlock>>,
    pub(crate) message_descriptors: CowDomain<Vec<RuntimeClientMessageDescriptor>>,
    pub(crate) publication_settlements: CowDomain<Vec<RuntimeClientPublicationSettlementRecord>>,
    pub(crate) transcript: SharedTranscript,
    pub(crate) trace: SharedTrace,
    pub(crate) trace_updates: CowDomain<Vec<super::super::trace::TraceLifecycle>>,
    pub(crate) attempt: CowDomain<Option<RuntimeClientAttempt>>,
    pub(crate) inbound: CowDomain<InboundDiagnostics>,
    pub(crate) pending_interactions: CowDomain<Vec<RuntimeClientInteractionHeader>>,
    pub(crate) jobs: CowDomain<Vec<RuntimeClientJob>>,
    pub(crate) agents: CowDomain<Vec<RuntimeClientAgent>>,
    pub(crate) statuses: CowDomain<Vec<AgentStatusView>>,
    pub(crate) context: CowDomain<RuntimeClientContextView>,
    pub(crate) capabilities: std::sync::Arc<CapabilityView>,
    pub(crate) resources: CowDomain<RuntimeClientResourcesView>,
    pub(crate) model: CowDomain<Option<SessionModelView>>,
    pub(crate) model_status: Option<RuntimeClientModelStatus>,
    pub(crate) todos: CowDomain<Option<TodoSnapshot>>,
    pub(crate) goal: CowDomain<Option<crate::runtime_client::goal::RuntimeClientGoalView>>,
}
impl From<RuntimeClientSnapshot> for SharedSnapshot {
    fn from(value: RuntimeClientSnapshot) -> Self {
        let RuntimeClientSnapshot {
            settings_evidence,
            effective_plugins,
            workflows,
            conversation_id,
            shutting_down,
            effective_approval_mode,
            configuration_adoption_eligibility,
            durability_failure,
            messages,
            message_descriptors,
            publication_settlements,
            transcript,
            trace,
            trace_updates,
            attempt,
            inbound,
            pending_interactions,
            jobs,
            agents,
            statuses,
            context,
            capabilities,
            resources,
            model,
            model_status,
            todos,
            goal,
        } = value;
        Self {
            settings_evidence,
            effective_plugins: effective_plugins.into(),
            workflows: workflows.into(),
            conversation_id,
            shutting_down,
            effective_approval_mode,
            configuration_adoption_eligibility,
            durability_failure: durability_failure.into(),
            messages: messages.into(),
            message_descriptors: message_descriptors.into(),
            publication_settlements: publication_settlements.into(),
            transcript: transcript.into(),
            trace: trace.into(),
            trace_updates: trace_updates.into(),
            attempt: attempt.into(),
            inbound: inbound.into(),
            pending_interactions: pending_interactions.into(),
            jobs: jobs.into(),
            agents: agents.into(),
            statuses: statuses.into(),
            context: context.into(),
            capabilities,
            resources: resources.into(),
            model: model.into(),
            model_status,
            todos: todos.into(),
            goal: goal.into(),
        }
    }
}
impl SharedSnapshot {
    #[cfg(test)]
    pub(crate) fn materialize(&self) -> RuntimeClientSnapshot {
        RuntimeClientSnapshot {
            settings_evidence: self.settings_evidence,
            effective_plugins: self.effective_plugins.owned(),
            workflows: self.workflows.owned(),
            conversation_id: self.conversation_id.clone(),
            shutting_down: self.shutting_down,
            effective_approval_mode: self.effective_approval_mode,
            configuration_adoption_eligibility: self.configuration_adoption_eligibility.clone(),
            durability_failure: self.durability_failure.owned(),
            messages: self.messages.owned(),
            message_descriptors: self.message_descriptors.owned(),
            publication_settlements: self.publication_settlements.owned(),
            transcript: self.transcript.owned(),
            trace: self.trace.owned(),
            trace_updates: self.trace_updates.owned(),
            attempt: self.attempt.owned(),
            inbound: self.inbound.owned(),
            pending_interactions: self.pending_interactions.owned(),
            jobs: self.jobs.owned(),
            agents: self.agents.owned(),
            statuses: self.statuses.owned(),
            context: self.context.owned(),
            capabilities: self.capabilities.clone(),
            resources: self.resources.owned(),
            model: self.model.owned(),
            model_status: self.model_status,
            todos: self.todos.owned(),
            goal: self.goal.owned(),
        }
    }
}
impl PartialEq<SharedSnapshot> for RuntimeClientSnapshot {
    fn eq(&self, other: &SharedSnapshot) -> bool {
        self.settings_evidence == other.settings_evidence
            && self.effective_plugins == *other.effective_plugins
            && self.workflows == *other.workflows
            && self.conversation_id == other.conversation_id
            && self.shutting_down == other.shutting_down
            && self.effective_approval_mode == other.effective_approval_mode
            && self.configuration_adoption_eligibility == other.configuration_adoption_eligibility
            && self.durability_failure == *other.durability_failure
            && self.messages == *other.messages
            && self.message_descriptors == *other.message_descriptors
            && self.publication_settlements == *other.publication_settlements
            && other.transcript == self.transcript
            && other.trace == self.trace
            && self.trace_updates == *other.trace_updates
            && self.attempt == *other.attempt
            && self.inbound == *other.inbound
            && self.pending_interactions == *other.pending_interactions
            && self.jobs == *other.jobs
            && self.agents == *other.agents
            && self.statuses == *other.statuses
            && self.context == *other.context
            && self.capabilities == other.capabilities
            && self.resources == *other.resources
            && self.model == *other.model
            && self.model_status == other.model_status
            && self.todos == *other.todos
            && self.goal == *other.goal
    }
}
impl PartialEq<RuntimeClientSnapshot> for SharedSnapshot {
    fn eq(&self, other: &RuntimeClientSnapshot) -> bool {
        other == self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SharedTranscript {
    pub(crate) source_cut: Option<crate::durable::observation::ObservationReadCut>,
    pub(crate) content: CowDomain<Vec<super::super::checkpoint::ContentBinding>>,
    pub(crate) collections: CowDomain<Vec<super::super::checkpoint::CollectionFrontier>>,
    pub(crate) deferred_entries: CowDomain<Vec<super::super::checkpoint::CheckpointPageItem>>,
    pub(crate) inherited_through: Option<RuntimeClientTranscriptCursor>,
    pub(crate) statistics: CowDomain<Option<super::super::response::ConversationStatistics>>,
    pub(crate) entries: CowDomain<Vec<RuntimeClientTranscriptEntry>>,
    pub(crate) next_cursor: Option<RuntimeClientTranscriptCursor>,
}
impl From<RuntimeClientTranscriptPage> for SharedTranscript {
    fn from(value: RuntimeClientTranscriptPage) -> Self {
        let RuntimeClientTranscriptPage {
            source_cut,
            content,
            collections,
            deferred_entries,
            inherited_through,
            statistics,
            entries,
            next_cursor,
        } = value;
        Self {
            source_cut,
            content: content.into(),
            collections: collections.into(),
            deferred_entries: deferred_entries.into(),
            inherited_through,
            statistics: statistics.into(),
            entries: entries.into(),
            next_cursor,
        }
    }
}
impl SharedTranscript {
    pub(super) fn install(&mut self, transcript: &RuntimeClientTranscriptPage) {
        self.source_cut.clone_from(&transcript.source_cut);
        if self.content != transcript.content {
            self.content.replace_from(&transcript.content);
        }
        if self.collections != transcript.collections {
            self.collections.replace_from(&transcript.collections);
        }
        // Nonempty payloads can contain ordered JSON or signed zero, so
        // PartialEq cannot authorize keeping their old immutable owners.
        if !self.deferred_entries.is_empty() || !transcript.deferred_entries.is_empty() {
            self.deferred_entries
                .replace_from(&transcript.deferred_entries);
        }
        self.inherited_through = transcript.inherited_through;
        if self.statistics.is_some() || transcript.statistics.is_some() {
            self.statistics.replace_from(&transcript.statistics);
        }
        if !self.entries.is_empty() || !transcript.entries.is_empty() {
            self.entries.replace_from(&transcript.entries);
        }
        self.next_cursor = transcript.next_cursor;
    }
    pub(crate) fn owned(&self) -> RuntimeClientTranscriptPage {
        RuntimeClientTranscriptPage {
            source_cut: self.source_cut.clone(),
            content: self.content.owned(),
            collections: self.collections.owned(),
            deferred_entries: self.deferred_entries.owned(),
            inherited_through: self.inherited_through,
            statistics: self.statistics.owned(),
            entries: self.entries.owned(),
            next_cursor: self.next_cursor,
        }
    }
}
impl PartialEq<RuntimeClientTranscriptPage> for SharedTranscript {
    fn eq(&self, other: &RuntimeClientTranscriptPage) -> bool {
        self.source_cut == other.source_cut
            && *self.content == other.content
            && *self.collections == other.collections
            && *self.deferred_entries == other.deferred_entries
            && self.inherited_through == other.inherited_through
            && *self.statistics == other.statistics
            && *self.entries == other.entries
            && self.next_cursor == other.next_cursor
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SharedTrace {
    pub(crate) read_cut: Option<super::super::checkpoint::CheckpointReadCut>,
    pub(crate) records: CowDomain<Vec<super::super::trace::TraceRecord>>,
    pub(crate) updates: CowDomain<Vec<super::super::trace::TraceLifecycle>>,
    pub(crate) next_cursor: Option<super::super::trace::TraceCursor>,
}
impl From<super::super::trace::TracePage> for SharedTrace {
    fn from(value: super::super::trace::TracePage) -> Self {
        let super::super::trace::TracePage {
            read_cut,
            records,
            updates,
            next_cursor,
        } = value;
        Self {
            read_cut,
            records: records.into(),
            updates: updates.into(),
            next_cursor,
        }
    }
}
impl SharedTrace {
    pub(super) fn install(&mut self, trace: &super::super::trace::TracePage) {
        self.read_cut.clone_from(&trace.read_cut);
        if !self.records.is_empty() || !trace.records.is_empty() {
            self.records.replace_from(&trace.records);
        }
        if !self.updates.is_empty() || !trace.updates.is_empty() {
            self.updates.replace_from(&trace.updates);
        }
        self.next_cursor.clone_from(&trace.next_cursor);
    }
    #[cfg(test)]
    pub(crate) fn owned(&self) -> super::super::trace::TracePage {
        super::super::trace::TracePage {
            read_cut: self.read_cut.clone(),
            records: self.records.owned(),
            updates: self.updates.owned(),
            next_cursor: self.next_cursor.clone(),
        }
    }
}
impl PartialEq<super::super::trace::TracePage> for SharedTrace {
    fn eq(&self, other: &super::super::trace::TracePage) -> bool {
        self.read_cut == other.read_cut
            && *self.records == other.records
            && *self.updates == other.updates
            && self.next_cursor == other.next_cursor
    }
}

impl SharedTrace {
    pub(super) fn stamp_cut(&mut self, cut: &super::super::checkpoint::CheckpointReadCut) {
        if self.read_cut.is_none() && (!self.records.is_empty() || !self.updates.is_empty()) {
            self.read_cut = Some(cut.clone());
        }
    }
}
