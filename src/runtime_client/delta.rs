//! Typed, cursor-relative edits of bounded observation lists. A slot addresses
//! the immutable base followed by the newly supplied values, never a live cache.
use super::{
    checkpoint::{ProjectionMetadata, RuntimeClientCheckpoint},
    types::RuntimeClientCursor,
};
use crate::observation::metadata::ReadCutMap;
use crate::observation::{
    cut::{CheckpointPageItem, CheckpointReadCut, CollectionFrontier, ContentBinding},
    metadata::MetadataFragment,
    transcript::{RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetadataDelta {
    /// Native replay validation retains the complete issued witness. It is
    /// derived state, never a second wire representation or decoding fallback.
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) native_cut: Option<CheckpointReadCut>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) native_transition: Option<Arc<super::event::RuntimeClientEvent>>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) native_bytes: usize,
    pub base_cursor: RuntimeClientCursor,
    pub read_cut: ReadCutDelta,
    pub content: ListDelta<MetadataFragment<ContentBinding>>,
    pub collections: ListDelta<MetadataFragment<CollectionFrontier>>,
}
impl MetadataDelta {
    pub(crate) fn from_parts(
        cursor: RuntimeClientCursor,
        content: &[MetadataFragment<ContentBinding>],
        collections: &[MetadataFragment<CollectionFrontier>],
        read_cut: &CheckpointReadCut,
        next: &ProjectionMetadata,
    ) -> Self {
        Self {
            native_cut: Some(next.read_cut.clone()),
            native_transition: None,
            native_bytes: 0,
            base_cursor: cursor,
            read_cut: ReadCutDelta::between(read_cut, &next.read_cut),
            content: ListDelta::between(content, &next.content),
            collections: ListDelta::between(collections, &next.collections),
        }
    }

    #[must_use]
    pub fn apply(&self, base: &RuntimeClientCheckpoint) -> Option<ProjectionMetadata> {
        if base.cursor != self.base_cursor
            || base.cursor.get().checked_add(1) != Some(self.read_cut.revision)
        {
            return None;
        }
        Some(ProjectionMetadata {
            read_cut: self.read_cut.apply(&base.read_cut)?,
            content: Arc::new(self.content.apply(&base.content)?),
            collections: Arc::new(self.collections.apply(&base.collections)?),
        })
    }
}

impl PartialEq for MetadataDelta {
    fn eq(&self, other: &Self) -> bool {
        self.base_cursor == other.base_cursor
            && self.read_cut == other.read_cut
            && self.content == other.content
            && self.collections == other.collections
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Changed<T> {
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapDelta<T> {
    pub removed: Vec<String>,
    pub values: BTreeMap<String, T>,
}
impl<T: Clone + PartialEq> MapDelta<T> {
    fn between(base: &ReadCutMap<T>, next: &ReadCutMap<T>) -> Option<Self> {
        let removed = base
            .keys()
            .filter(|key| !next.contains_key(*key))
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let values = next
            .iter()
            .filter(|(key, value)| base.get(*key) != Some(*value))
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        (!removed.is_empty() || !values.is_empty()).then_some(Self { removed, values })
    }
    fn apply(&self, base: &ReadCutMap<T>) -> ReadCutMap<T> {
        let mut result = base.clone();
        for key in &self.removed {
            result.remove(key.as_str());
        }
        for (key, value) in &self.values {
            result.insert(Arc::from(key.as_str()), value.clone());
        }
        result
    }
}

/// Scope remains explicit on each record. Every other unchanged cut component
/// is inherited only from the cursor-checked baseline in that same scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCutDelta {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub activation_id: Option<crate::runtime::identity::SubagentId>,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_cut: Option<Changed<Option<crate::durable::observation::ObservationReadCut>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_publication: Option<Changed<Option<crate::observation::cut::LivePublicationReadCut>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_foreground:
        Option<Changed<Option<crate::observation::cut::CanonicalForegroundReadCut>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_revisions: Option<MapDelta<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_cuts: Option<MapDelta<crate::runtime::identity::ObservationCollectionCut>>,
}
impl ReadCutDelta {
    fn between(base: &CheckpointReadCut, next: &CheckpointReadCut) -> Self {
        Self {
            conversation_id: next.conversation_id.clone(),
            activation_id: next.activation_id.clone(),
            revision: next.revision,
            through: (base.through != next.through).then_some(next.through),
            source_cut: (base.source_cut != next.source_cut).then(|| Changed {
                value: next.source_cut.clone(),
            }),
            live_publication: (base.live_publication != next.live_publication).then(|| Changed {
                value: next.live_publication.clone(),
            }),
            canonical_foreground: (base.canonical_foreground != next.canonical_foreground).then(
                || Changed {
                    value: next.canonical_foreground.clone(),
                },
            ),
            domain_revisions: MapDelta::between(&base.domain_revisions, &next.domain_revisions),
            collection_cuts: MapDelta::between(&base.collection_cuts, &next.collection_cuts),
        }
    }
    #[must_use]
    pub fn apply(&self, base: &CheckpointReadCut) -> Option<CheckpointReadCut> {
        if self.conversation_id != base.conversation_id || self.activation_id != base.activation_id
        {
            return None;
        }
        let mut next = base.clone();
        next.revision = self.revision;
        if let Some(value) = self.through {
            next.through = value;
        }
        if let Some(value) = &self.source_cut {
            next.source_cut.clone_from(&value.value);
        }
        if let Some(value) = &self.live_publication {
            next.live_publication.clone_from(&value.value);
        }
        if let Some(value) = &self.canonical_foreground {
            next.canonical_foreground.clone_from(&value.value);
        }
        if let Some(value) = &self.domain_revisions {
            next.domain_revisions = value.apply(&base.domain_revisions).into();
        }
        if let Some(value) = &self.collection_cuts {
            next.collection_cuts = value.apply(&base.collection_cuts).into();
        }
        Some(next)
    }
}

macro_rules! snapshot_delta {
    (scalars { $($field:ident : $ty:ty),* $(,)? } lists { $($list:ident : $item:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct SnapshotDelta {
            $(#[serde(default, skip_serializing_if="Option::is_none")] pub $field: Option<Changed<$ty>>),*,
            $(#[serde(default, skip_serializing_if="Option::is_none")] pub $list: Option<ListDelta<$item>>),*,
            #[serde(default, skip_serializing_if="Option::is_none")]
            pub transcript: Option<TranscriptDelta>,
        }
        impl SnapshotDelta {
            pub(crate) fn between(base: &super::snapshot::RuntimeClientSnapshot, next: &super::snapshot::RuntimeClientSnapshot) -> Self {
                let super::snapshot::RuntimeClientSnapshot {$($field: _),*, $($list: _),*, transcript: _} = next;
                Self {
                    $($field: (base.$field != next.$field).then(|| Changed {value:next.$field.clone()})),*,
                    $($list: (base.$list != next.$list).then(|| ListDelta::between(&base.$list,&next.$list))),*,
                    transcript: (base.transcript != next.transcript).then(|| TranscriptDelta::between(&base.transcript,&next.transcript)),
                }
            }
            #[must_use]
            pub fn apply(&self, base: &super::snapshot::RuntimeClientSnapshot) -> Option<super::snapshot::RuntimeClientSnapshot> {
                let mut next=base.clone();
                $(if let Some(change)=&self.$field {next.$field=change.value.clone();})*
                $(if let Some(change)=&self.$list {next.$list=change.apply(&base.$list)?;})*
                if let Some(change)=&self.transcript {next.transcript=change.apply(&base.transcript)?;}
                Some(next)
            }
        }
    };
}
snapshot_delta! {
    scalars {
        settings_evidence: super::settings::SettingsEvidence,
        effective_plugins: Option<super::settings::EffectivePlugins>,
        workflows: crate::runtime::workflow::read_model::WorkflowSnapshot,
        conversation_id: crate::runtime::identity::ConversationId,
        shutting_down: bool,
        effective_approval_mode: crate::runtime::types::ApprovalMode,
        durability_failure: Option<super::snapshot::RuntimeDurabilityFailure>,
        trace: crate::observation::trace::TracePage,
        attempt: Option<super::snapshot::RuntimeClientAttempt>,
        inbound: super::snapshot::InboundDiagnostics,
        context: super::snapshot::RuntimeClientContextView,
        capabilities: Arc<crate::observation::capability_source::CapabilityView>,
        resources: crate::observation::resource_source::RuntimeClientResourcesView,
        model: Option<crate::model::session::SessionModelView>,
        model_status: Option<super::snapshot::RuntimeClientModelStatus>,
        todos: Option<crate::tools::todo::TodoSnapshot>,
        goal: Option<crate::observation::goal::RuntimeClientGoalView>,
    }
    lists {
        messages: crate::message::types::MessageBlock,
        message_descriptors: Arc<crate::observation::transcript::RuntimeClientMessageDescriptor>,
        publication_settlements: crate::observation::transcript::RuntimeClientPublicationSettlementRecord,
        trace_updates: crate::observation::trace::TraceLifecycle,
        pending_interactions: crate::observation::interaction::RuntimeClientInteractionHeader,
        jobs: super::snapshot::RuntimeClientJob,
        agents: super::snapshot::RuntimeClientAgent,
        statuses: crate::observation::status::AgentStatusView,
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointDelta {
    pub snapshot: SnapshotDelta,
    pub cursor: RuntimeClientCursor,
    pub read_cut: CheckpointReadCut,
    pub content: Arc<Vec<MetadataFragment<ContentBinding>>>,
    pub collections: Arc<Vec<MetadataFragment<CollectionFrontier>>>,
}
impl CheckpointDelta {
    pub(crate) fn between(
        base: &super::snapshot::RuntimeClientSnapshot,
        next: &RuntimeClientCheckpoint,
    ) -> Self {
        Self {
            snapshot: SnapshotDelta::between(base, &next.snapshot),
            cursor: next.cursor,
            read_cut: next.read_cut.clone(),
            content: next.content.clone(),
            collections: next.collections.clone(),
        }
    }
    #[must_use]
    pub fn apply(
        &self,
        base: &super::snapshot::RuntimeClientSnapshot,
    ) -> Option<RuntimeClientCheckpoint> {
        Some(RuntimeClientCheckpoint {
            snapshot: self.snapshot.apply(base)?,
            cursor: self.cursor,
            read_cut: self.read_cut.clone(),
            content: self.content.clone(),
            collections: self.collections.clone(),
        })
    }
}

/// Expand one atomic edit using only its named published baseline. Native
/// consumers and equivalence tests use the same typed vocabulary as clients.
#[must_use]
pub fn expand_event(
    base: &RuntimeClientCheckpoint,
    event: &super::event::RuntimeClientEvent,
) -> Option<super::event::RuntimeClientEvent> {
    use super::event::RuntimeClientEvent as Event;
    let Event::ProjectionDelta { events, metadata } = event else {
        return Some(event.clone());
    };
    let metadata = metadata.apply(base)?;
    let events = events
        .iter()
        .map(|event| match event {
            Event::ReadDomainsPatched {
                projection_head,
                transcript,
                trace,
                occupancy,
                todos,
            } => Some(Event::ReadDomainsUpdated {
                projection_head: projection_head.clone(),
                transcript: Box::new(transcript.apply(&base.snapshot.transcript)?),
                trace: trace.clone(),
                occupancy: occupancy.clone(),
                todos: todos.clone(),
            }),
            Event::MessageDescriptorPatched {
                attempt_id,
                descriptor,
                head,
                transcript_cursor,
                evicted_message_id,
            } => {
                let mut prior = base.snapshot.message_descriptors.clone();
                prior.push(descriptor.clone());
                Some(Event::MessageDescriptorCommitted {
                    attempt_id: attempt_id.clone(),
                    descriptor: descriptor.clone(),
                    head: head.apply(&prior)?.into_iter().collect(),
                    transcript_cursor: *transcript_cursor,
                    evicted_message_id: evicted_message_id.clone(),
                })
            }
            Event::CheckpointPatched {
                transition,
                checkpoint,
            } => Some(Event::CheckpointAdvanced {
                transition: transition.clone(),
                checkpoint: Box::new(checkpoint.apply(&base.snapshot)?),
            }),
            Event::ProjectionDelta { .. } | Event::ProjectionTransition { .. } => None,
            event => Some(event.clone()),
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Event::ProjectionTransition { events, metadata })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptDelta {
    /// Scalar page evidence; list fields are carried by the edits below.
    pub page: RuntimeClientTranscriptPage,
    pub content: ListDelta<ContentBinding>,
    pub collections: ListDelta<CollectionFrontier>,
    pub deferred_entries: ListDelta<CheckpointPageItem>,
    pub entries: ListDelta<Arc<RuntimeClientTranscriptEntry>>,
}
impl TranscriptDelta {
    pub(crate) fn between(
        base: &RuntimeClientTranscriptPage,
        next: &RuntimeClientTranscriptPage,
    ) -> Self {
        let mut page = next.clone();
        page.content = MetadataFragment::default();
        page.collections = MetadataFragment::default();
        page.deferred_entries = MetadataFragment::default();
        page.entries = MetadataFragment::default();
        Self {
            page,
            content: ListDelta::between(&base.content, &next.content),
            collections: ListDelta::between(&base.collections, &next.collections),
            deferred_entries: ListDelta::between(&base.deferred_entries, &next.deferred_entries),
            entries: ListDelta::between(&base.entries, &next.entries),
        }
    }
    #[must_use]
    pub fn apply(&self, base: &RuntimeClientTranscriptPage) -> Option<RuntimeClientTranscriptPage> {
        let mut page = self.page.clone();
        page.content = self.content.apply(&base.content)?.into();
        page.collections = self.collections.apply(&base.collections)?.into();
        page.deferred_entries = self.deferred_entries.apply(&base.deferred_entries)?.into();
        page.entries = self.entries.apply(&base.entries)?.into();
        Some(page)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListDelta<T> {
    pub slots: Vec<u32>,
    pub values: Vec<T>,
}

impl<T: Clone + PartialEq> ListDelta<T> {
    pub(crate) fn between(base: &[T], next: &[T]) -> Self {
        let mut values = Vec::new();
        let mut slots = Vec::with_capacity(next.len());
        let mut used = vec![false; base.len()];
        for value in next {
            let index = base
                .iter()
                .enumerate()
                .find_map(|(index, old)| (!used[index] && old == value).then_some(index))
                .unwrap_or_else(|| {
                    let index = base.len() + values.len();
                    values.push(value.clone());
                    index
                });
            if index < base.len() {
                used[index] = true;
            }
            slots.push(u32::try_from(index).expect("bounded observation list"));
        }
        values.shrink_to_fit();
        Self { slots, values }
    }

    #[must_use]
    pub fn apply(&self, base: &[T]) -> Option<Vec<T>> {
        if self.slots.len() > base.len().saturating_add(self.values.len()) {
            return None;
        }
        self.slots
            .iter()
            .map(|&index| {
                let index = index as usize;
                if index < base.len() {
                    base.get(index)
                } else {
                    self.values.get(index - base.len())
                }
                .cloned()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ListDelta;

    #[test]
    fn list_delta_preserves_reordering_replacement_and_eviction() {
        let base = vec!["old", "kept", "moved"];
        for next in [
            vec![],
            vec!["moved", "new", "kept"],
            vec!["kept"; 8],
            base.clone(),
        ] {
            let delta = ListDelta::between(&base, &next);
            assert_eq!(delta.apply(&base), Some(next));
        }
        assert!(ListDelta::between(&base, &base).values.is_empty());
    }

    #[test]
    fn invalid_slot_does_not_silently_drop_a_row() {
        let delta = ListDelta {
            slots: vec![1],
            values: Vec::<String>::new(),
        };
        assert_eq!(delta.apply(&["only".into()]), None);
    }
}
