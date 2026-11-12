//! Typed, cursor-relative edits of bounded observation lists. A slot addresses
//! the immutable base followed by the newly supplied values; its high bit
//! selects a typed field edit of a base row. No slot addresses a live cache.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ListDelta<MetadataFragment<ContentBinding>, BindingDelta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collections: Option<ListDelta<MetadataFragment<CollectionFrontier>, FrontierDelta>>,
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
            content: (content != next.content.as_slice())
                .then(|| ListDelta::between(content, &next.content)),
            collections: (collections != next.collections.as_slice())
                .then(|| ListDelta::between(collections, &next.collections)),
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
            content: match &self.content {
                Some(edit) => Arc::new(edit.apply(&base.content)?),
                None => base.content.clone(),
            },
            collections: match &self.collections {
                Some(edit) => Arc::new(edit.apply(&base.collections)?),
                None => base.collections.clone(),
            },
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
    pub source_cut_patch: Option<ObservationCutDelta>,
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
        let patch_source = base
            .source_cut
            .as_ref()
            .zip(next.source_cut.as_ref())
            .filter(|(a, b)| ObservationCutDelta::same_owner(a, b));
        Self {
            conversation_id: next.conversation_id.clone(),
            activation_id: next.activation_id.clone(),
            revision: next.revision,
            through: (base.through != next.through).then_some(next.through),
            source_cut: (base.source_cut != next.source_cut && patch_source.is_none()).then(|| {
                Changed {
                    value: next.source_cut.clone(),
                }
            }),
            source_cut_patch: (base.source_cut != next.source_cut)
                .then(|| patch_source.map(|(a, b)| ObservationCutDelta::between(a, b)))
                .flatten(),
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
        if let Some(value) = &self.source_cut_patch {
            if self.source_cut.is_some() {
                return None;
            }
            next.source_cut = Some(value.apply_to(base.source_cut.as_ref()?)?);
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

/// A typed edit of one stable native owner; identity selection happens before
/// comparing fields, so changed rows never force unchanged bodies onto the wire.
pub trait ItemDelta<T>: Sized {
    fn same_owner(base: &T, next: &T) -> bool;
    fn between(base: &T, next: &T) -> Self;
    fn apply_to(&self, base: &T) -> Option<T>;
}
impl<T> ItemDelta<T> for () {
    fn same_owner(_: &T, _: &T) -> bool {
        false
    }
    fn between(_: &T, _: &T) {}
    fn apply_to(&self, _: &T) -> Option<T> {
        None
    }
}
macro_rules! object_delta {
    ($name:ident, $target:ty, $identity:expr, scalars {$($field:ident:$ty:ty),* $(,)?} lists {$($list:ident:$item:ty => $patch:ty),* $(,)?} $(nested {$($nested:ident:$nested_patch:ty),* $(,)?})?) => {
        #[derive(Debug,Clone,PartialEq,Serialize,Deserialize,schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(#[serde(default,skip_serializing_if="Option::is_none")] pub $field:Option<Changed<$ty>>,)*
            $(#[serde(default,skip_serializing_if="Option::is_none")] pub $list:Option<ListDelta<$item,$patch>>,)*
            $($(#[serde(default,skip_serializing_if="Option::is_none")] pub $nested:Option<$nested_patch>,)*)?
        }
        impl ItemDelta<$target> for $name {
            fn same_owner(base:&$target,next:&$target)->bool {($identity)(base,next)}
            fn between(base:&$target,next:&$target)->Self {Self {
                $($field:(base.$field!=next.$field).then(||Changed {value:next.$field.clone()}),)*
                $($list:(base.$list!=next.$list).then(||ListDelta::between(&base.$list,&next.$list)),)*
                $($($nested:(base.$nested!=next.$nested).then(||<$nested_patch>::between(&base.$nested,&next.$nested)),)*)?
            }}
            fn apply_to(&self,base:&$target)->Option<$target> {
                let mut next=base.clone();
                $(if let Some(change)=&self.$field {next.$field=change.value.clone();})*
                $(if let Some(change)=&self.$list {next.$list=change.apply(&base.$list)?;})*
                $($(if let Some(change)=&self.$nested {next.$nested=change.apply_to(&base.$nested)?;})*)?
                Some(next)
            }
        }
    };
}
object_delta!(ReadEvidenceDelta, crate::durable::reading::ConversationReadCut,
    |a:&crate::durable::reading::ConversationReadCut,b:&crate::durable::reading::ConversationReadCut|a.conversation_id==b.conversation_id,
    scalars {
        journal:u64, transcript:u64, mutation_revision:u64, ledger_through:u64,
        surface_revision:crate::conversation::SurfaceRevision, transcript_total:u64,
        pending_through:u64,pending_total:u64,
    } lists {});
object_delta!(ObservationCutDelta, crate::durable::observation::ObservationReadCut,
    |a:&crate::durable::observation::ObservationReadCut,b:&crate::durable::observation::ObservationReadCut|a.read.conversation_id==b.read.conversation_id,
    scalars {
        ledger_through:u64,transcript_total:u64,surface_revision:crate::conversation::SurfaceRevision,
        inherited_through:crate::durable::TranscriptCursor,
    } lists {} nested {read:ReadEvidenceDelta});
macro_rules! arc_delta {
    ($patch:ty,$target:ty) => {
        impl ItemDelta<Arc<$target>> for $patch {
            fn same_owner(base: &Arc<$target>, next: &Arc<$target>) -> bool {
                <Self as ItemDelta<$target>>::same_owner(base, next)
            }
            fn between(base: &Arc<$target>, next: &Arc<$target>) -> Self {
                <Self as ItemDelta<$target>>::between(base, next)
            }
            fn apply_to(&self, base: &Arc<$target>) -> Option<Arc<$target>> {
                Some(Arc::new(<Self as ItemDelta<$target>>::apply_to(
                    self, base,
                )?))
            }
        }
    };
}
macro_rules! fragment_delta {
    ($patch:ty,$target:ty) => {
        impl ItemDelta<MetadataFragment<$target>> for $patch {
            fn same_owner(
                base: &MetadataFragment<$target>,
                next: &MetadataFragment<$target>,
            ) -> bool {
                <Self as ItemDelta<$target>>::same_owner(base, next)
            }
            fn between(base: &MetadataFragment<$target>, next: &MetadataFragment<$target>) -> Self {
                <Self as ItemDelta<$target>>::between(base, next)
            }
            fn apply_to(
                &self,
                base: &MetadataFragment<$target>,
            ) -> Option<MetadataFragment<$target>> {
                Some(<Self as ItemDelta<$target>>::apply_to(self, base)?.into())
            }
        }
    };
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SafeQuantity {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub value: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_revision: Option<SafeQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readable_end: Option<SafeQuantity>,
}
impl ItemDelta<crate::observation::content::ContentRef> for RefDelta {
    fn same_owner(
        a: &crate::observation::content::ContentRef,
        b: &crate::observation::content::ContentRef,
    ) -> bool {
        a.conversation_id == b.conversation_id
            && a.source == b.source
            && a.representation == b.representation
            && a.version == b.version
    }
    fn between(
        a: &crate::observation::content::ContentRef,
        b: &crate::observation::content::ContentRef,
    ) -> Self {
        Self {
            content_revision: (a.content_revision != b.content_revision).then_some(SafeQuantity {
                value: b.content_revision,
            }),
            readable_end: (a.readable_end != b.readable_end).then_some(SafeQuantity {
                value: b.readable_end,
            }),
        }
    }
    fn apply_to(
        &self,
        a: &crate::observation::content::ContentRef,
    ) -> Option<crate::observation::content::ContentRef> {
        let mut next = a.clone();
        if let Some(v) = &self.content_revision {
            next.content_revision = v.value;
        }
        if let Some(v) = &self.readable_end {
            next.readable_end = v.value;
        }
        Some(next)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Changed<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<Changed<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<RefDelta>,
}
impl ItemDelta<ContentBinding> for BindingDelta {
    fn same_owner(a: &ContentBinding, b: &ContentBinding) -> bool {
        a.source_path == b.source_path && RefDelta::same_owner(&a.content_ref, &b.content_ref)
    }
    fn between(a: &ContentBinding, b: &ContentBinding) -> Self {
        Self {
            path: (a.path != b.path).then(|| Changed {
                value: b.path.clone(),
            }),
            deferred: (a.deferred != b.deferred).then_some(Changed { value: b.deferred }),
            content_ref: (a.content_ref != b.content_ref)
                .then(|| RefDelta::between(&a.content_ref, &b.content_ref)),
        }
    }
    fn apply_to(&self, a: &ContentBinding) -> Option<ContentBinding> {
        let mut next = a.clone();
        if let Some(v) = &self.path {
            next.path.clone_from(&v.value);
        }
        if let Some(v) = &self.deferred {
            next.deferred = v.value;
        }
        if let Some(v) = &self.content_ref {
            next.content_ref = Arc::new(v.apply_to(&a.content_ref)?);
        }
        Some(next)
    }
}
fragment_delta!(BindingDelta, ContentBinding);
object_delta!(FrontierDelta,CollectionFrontier,|a:&CollectionFrontier,b:&CollectionFrontier|a.domain==b.domain,scalars {revision:u64,total:u64,loaded:u64,next_offset:Option<u64>} lists {});
fragment_delta!(FrontierDelta, CollectionFrontier);
object_delta!(ToolDelta,crate::observation::tool::ForegroundToolExecution,|a:&crate::observation::tool::ForegroundToolExecution,b:&crate::observation::tool::ForegroundToolExecution|a.occurrence==b.occurrence || a.message_id.is_some() && a.message_id==b.message_id && a.block_index==b.block_index,scalars {
    occurrence:crate::observation::tool::RuntimeClientToolOccurrence,
    message_id:Option<crate::runtime::identity::MessageId>,block_index:crate::message::types::ContentBlockIndex,
    call_id:Option<crate::runtime::identity::ToolCallId>,metadata_ref:Option<crate::observation::content::ContentRef>,assembly_complete:bool,
    result_source:Option<crate::observation::tool::JournalFieldRef>,result_metadata_ref:Option<crate::observation::content::ContentRef>,
    committed_result:Option<crate::tools::types::ToolExecutionResult>,committed_result_deferred:bool,
    tool_id:Option<crate::runtime::identity::ToolId>,name:String,state:crate::observation::tool::ForegroundToolState,
} lists {});
object_delta!(DescriptorDelta,crate::observation::transcript::RuntimeClientMessageDescriptor,|a:&crate::observation::transcript::RuntimeClientMessageDescriptor,b:&crate::observation::transcript::RuntimeClientMessageDescriptor|a.locator==b.locator,scalars {
    deliveries:Option<crate::observation::delivery::RuntimeClientDeliveryHead>,identity:Option<crate::runtime::identity::MessageId>,locator:crate::observation::content::CanonicalContentLocator,
    role:String,kind:String,block_count:u64,tool_call_count:u64,
    turn_process:Option<crate::observation::turn::TurnProcessView>,completed_response:Option<crate::observation::turn::CompletedResponseView>,
} lists {tool_calls:crate::observation::tool::ForegroundToolExecution=>ToolDelta,content:ContentBinding=>BindingDelta,collections:CollectionFrontier=>FrontierDelta});
arc_delta!(
    DescriptorDelta,
    crate::observation::transcript::RuntimeClientMessageDescriptor
);
object_delta!(EntryDelta,RuntimeClientTranscriptEntry,|a:&RuntimeClientTranscriptEntry,b:&RuntimeClientTranscriptEntry|a.cursor==b.cursor,scalars {
    turn_process:Option<crate::observation::turn::TurnProcessView>,response_pending:bool,completed_response:Option<crate::observation::turn::CompletedResponseView>,
    cursor:crate::observation::transcript::RuntimeClientTranscriptCursor,item:crate::observation::transcript::RuntimeClientTranscriptItem,
} lists {tool_calls:crate::observation::tool::ForegroundToolExecution=>ToolDelta});
arc_delta!(EntryDelta, RuntimeClientTranscriptEntry);

macro_rules! snapshot_delta {
    (scalars { $($field:ident : $ty:ty),* $(,)? } lists { $($list:ident : $item:ty => $patch:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct SnapshotDelta {
            $(#[serde(default, skip_serializing_if="Option::is_none")] pub $field: Option<Changed<$ty>>),*,
            $(#[serde(default, skip_serializing_if="Option::is_none")] pub $list: Option<ListDelta<$item,$patch>>),*,
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
        messages: crate::message::types::MessageBlock => (),
        message_descriptors: Arc<crate::observation::transcript::RuntimeClientMessageDescriptor> => DescriptorDelta,
        publication_settlements: crate::observation::transcript::RuntimeClientPublicationSettlementRecord => (),
        trace_updates: crate::observation::trace::TraceLifecycle => (),
        pending_interactions: crate::observation::interaction::RuntimeClientInteractionHeader => (),
        jobs: super::snapshot::RuntimeClientJob => (),
        agents: super::snapshot::RuntimeClientAgent => (),
        statuses: crate::observation::status::AgentStatusView => (),
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
    pub content: ListDelta<ContentBinding, BindingDelta>,
    pub collections: ListDelta<CollectionFrontier, FrontierDelta>,
    pub deferred_entries: ListDelta<CheckpointPageItem>,
    pub entries: ListDelta<Arc<RuntimeClientTranscriptEntry>, EntryDelta>,
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
#[serde(
    deny_unknown_fields,
    bound(deserialize = "T: Deserialize<'de>, P: Deserialize<'de>")
)]
pub struct ListDelta<T, P = ()> {
    pub slots: Vec<u32>,
    pub values: Vec<T>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<ListItemDelta<P>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItemDelta<P> {
    pub base: u32,
    pub delta: P,
}
impl<T: Clone + PartialEq, P: ItemDelta<T>> ListDelta<T, P> {
    pub(crate) fn between(base: &[T], next: &[T]) -> Self {
        let mut values = Vec::new();
        let mut patches = Vec::new();
        let mut slots = Vec::with_capacity(next.len());
        let mut used = vec![false; base.len()];
        for value in next {
            if let Some(index) = base
                .iter()
                .enumerate()
                .find_map(|(i, old)| (!used[i] && old == value).then_some(i))
            {
                used[index] = true;
                slots.push(u32::try_from(index).expect("bounded list"));
            } else if let Some(index) = base
                .iter()
                .enumerate()
                .find_map(|(i, old)| (!used[i] && P::same_owner(old, value)).then_some(i))
            {
                used[index] = true;
                slots.push(0x8000_0000 | u32::try_from(patches.len()).expect("bounded edits"));
                patches.push(ListItemDelta {
                    base: u32::try_from(index).expect("bounded list"),
                    delta: P::between(&base[index], value),
                });
            } else {
                slots.push(u32::try_from(base.len() + values.len()).expect("bounded list"));
                values.push(value.clone());
            }
        }
        values.shrink_to_fit();
        patches.shrink_to_fit();
        Self {
            slots,
            values,
            patches,
        }
    }
    #[must_use]
    pub fn apply(&self, base: &[T]) -> Option<Vec<T>> {
        if self.slots.len()
            > base
                .len()
                .saturating_add(self.values.len())
                .saturating_add(self.patches.len())
        {
            return None;
        }
        self.slots
            .iter()
            .map(|&slot| {
                if slot & 0x8000_0000 != 0 {
                    let patch = self.patches.get((slot & 0x7fff_ffff) as usize)?;
                    patch.delta.apply_to(base.get(patch.base as usize)?)
                } else {
                    let index = slot as usize;
                    if index < base.len() {
                        base.get(index).cloned()
                    } else {
                        self.values.get(index - base.len()).cloned()
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{BindingDelta, DescriptorDelta, ListDelta, ListItemDelta};
    use crate::observation::{
        content::{CanonicalContentLocator, ContentRef},
        cut::ContentBinding,
        transcript::RuntimeClientMessageDescriptor,
    };
    use crate::runtime::identity::{ConversationId, MessageId};
    use std::sync::Arc;

    fn binding() -> ContentBinding {
        ContentBinding {
            source_path: None,
            path: vec!["transcript".into(), "0".into()],
            content_ref: Arc::new(ContentRef::configuration_instructions(
                ConversationId::new("conv_00000000-0000-7000-8000-000000000001"),
                1,
                100,
            )),
            deferred: false,
        }
    }

    #[test]
    fn list_delta_preserves_reordering_replacement_and_eviction() {
        let base = vec!["old", "kept", "moved"];
        for next in [
            vec![],
            vec!["moved", "new", "kept"],
            vec!["kept"; 8],
            base.clone(),
        ] {
            let delta: ListDelta<_> = ListDelta::between(&base, &next);
            assert_eq!(delta.apply(&base), Some(next));
        }
        assert!(ListDelta::<_>::between(&base, &base).values.is_empty());
    }

    #[test]
    fn invalid_slot_does_not_silently_drop_a_row() {
        let delta = ListDelta {
            slots: vec![1],
            values: Vec::<String>::new(),
            patches: Vec::<super::ListItemDelta<()>>::new(),
        };
        assert_eq!(delta.apply(&["only".into()]), None);
    }

    #[test]
    fn source_cut_edit_keeps_exact_unchanged_historical_evidence() {
        let source = serde_json::from_value(serde_json::json!({
            "read":{"conversation_id":"conv_00000000-0000-7000-8000-000000000001","journal":1,"transcript":2,
                "mutation_revision":3,"ledger_through":4,"surface_revision":5,
                "transcript_total":6,"pending_through":7,"pending_total":8},
            "ledger_through":4,"transcript_total":6,"surface_revision":5,"inherited_through":0,
        }))
        .unwrap();
        let base = super::CheckpointReadCut {
            conversation_id: ConversationId::new("conv_00000000-0000-7000-8000-000000000001"),
            activation_id: None,
            revision: 1,
            through: 1,
            source_cut: Some(source),
            live_publication: None,
            canonical_foreground: None,
            domain_revisions: super::MetadataFragment::default(),
            collection_cuts: super::MetadataFragment::default(),
        };
        let mut next = base.clone();
        next.revision = 2;
        next.source_cut.as_mut().unwrap().read.journal = 9;
        let edit = super::ReadCutDelta::between(&base, &next);
        assert_eq!(edit.apply(&base), Some(next));
        assert!(edit.source_cut.is_none());
        assert_eq!(
            serde_json::to_value(edit.source_cut_patch).unwrap(),
            serde_json::json!({"read":{"journal":{"value":9}}})
        );
        next = base.clone();
        next.source_cut = None;
        let edit = super::ReadCutDelta::between(&base, &next);
        assert!(edit.source_cut_patch.is_none());
        assert_eq!(edit.apply(&base), Some(next));
    }

    #[test]
    fn field_edits_reuse_content_identity_and_reject_invalid_patch_bases() {
        let base = vec![binding()];
        let mut next = base.clone();
        next[0].path[1] = "1".into();
        Arc::make_mut(&mut next[0].content_ref).content_revision = 2;
        Arc::make_mut(&mut next[0].content_ref).readable_end = 200;
        let mut delta: ListDelta<_, BindingDelta> = ListDelta::between(&base, &next);
        assert!(delta.values.is_empty());
        assert_eq!(delta.apply(&base), Some(next.clone()));
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            serde_json::json!({"slots":[2_147_483_648_u32],"values":[],"patches":[{
                "base":0,"delta":{
                    "path":{"value":["transcript","1"]},
                    "content_ref":{"content_revision":{"value":2},"readable_end":{"value":200}}
                }
            }]})
        );
        let decoded: ListDelta<ContentBinding, BindingDelta> =
            serde_json::from_value(serde_json::to_value(&delta).unwrap()).unwrap();
        assert_eq!(decoded.apply(&base), Some(next.clone()));
        delta.patches[0].base = 1;
        assert_eq!(delta.apply(&base), None);
        delta.slots[0] = 0x8000_0001;
        assert_eq!(delta.apply(&base), None);
        next[0].content_ref = Arc::new(ContentRef::configuration_instructions(
            ConversationId::new("conv_00000000-0000-7000-8000-000000000002"),
            1,
            200,
        ));
        let delta: ListDelta<_, BindingDelta> = ListDelta::between(&base, &next);
        assert!(delta.patches.is_empty());
        assert_eq!(delta.values, next);
    }

    #[test]
    fn descriptor_refinement_keeps_unchanged_content_and_distinguishes_clear() {
        let base = vec![Arc::new(RuntimeClientMessageDescriptor {
            deliveries: None,
            identity: Some(MessageId::new("message")),
            locator: CanonicalContentLocator::ByMessageId {
                message_id: MessageId::new("message"),
            },
            role: "assistant".into(),
            kind: "model".into(),
            block_count: 1,
            tool_call_count: 0,
            tool_calls: vec![],
            turn_process: None,
            completed_response: None,
            content: vec![binding()],
            collections: vec![],
        })];
        let mut next = base.clone();
        let refined = Arc::make_mut(&mut next[0]);
        refined.identity = None;
        refined.block_count = 2;
        let delta: ListDelta<_, DescriptorDelta> = ListDelta::between(&base, &next);
        assert!(delta.values.is_empty());
        assert_eq!(delta.patches.len(), 1);
        assert!(delta.patches[0].delta.content.is_none());
        let encoded = serde_json::to_value(&delta).unwrap();
        assert_eq!(
            encoded["patches"][0]["delta"]["identity"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(delta.apply(&base), Some(next));
        let invalid = ListDelta::<_, DescriptorDelta> {
            slots: vec![0x8000_0000],
            values: Vec::<Arc<RuntimeClientMessageDescriptor>>::new(),
            patches: vec![ListItemDelta {
                base: 1,
                delta: delta.patches[0].delta.clone(),
            }],
        };
        assert_eq!(invalid.apply(&base), None);
    }
}
