//! Immutable capture fragments belong to one exact installed domain value.
//! Reuse compares immutable domain owners, never values or serialized witnesses.
use super::CheckpointSeed;
use crate::runtime_client::checkpoint;
use serde::Serialize;
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

type Owner = Arc<OnceLock<Arc<dyn Any + Send + Sync>>>;

#[derive(Clone)]
pub(super) struct PreparedCaptures(imbl::OrdMap<&'static str, Owner>);
impl Default for PreparedCaptures {
    fn default() -> Self {
        Self(
            [
                "context",
                "effective_plugins",
                "workflows",
                "messages",
                "message_descriptors",
                "transcript_deferred_entries",
                "transcript_statistics",
                "trace_records",
                "trace_updates",
                "trace_page_updates",
                "inbound",
                "pending_interactions",
                "jobs",
                "agents",
                "statuses",
                "capabilities",
                "resources",
                "model",
                "todos",
                "goal",
            ]
            .into_iter()
            .map(|key| (key, Arc::default()))
            .collect(),
        )
    }
}
impl PreparedCaptures {
    pub(super) fn empty() -> Self {
        Self(imbl::OrdMap::new())
    }

    pub(super) fn inherit(current: &CheckpointSeed, previous: Option<&CheckpointSeed>) -> Self {
        let Some(previous) = previous else {
            return Self::default();
        };
        let mut result = previous.prepared.clone();
        let left = &current.snapshot;
        let right = &previous.snapshot;
        macro_rules! reuse {
            ($key:expr, $same:expr) => {
                if !$same {
                    result.0.insert($key, Arc::default());
                }
            };
        }
        macro_rules! field {
            ($field:ident) => {
                reuse!(stringify!($field), left.$field.same_owner(&right.$field));
            };
        }
        // Capture of native message bodies uses the conversation identity in
        // deferred content locators, in addition to the body itself.
        reuse!(
            "messages",
            left.conversation_id == right.conversation_id
                && left.messages.same_owner(&right.messages)
        );

        reuse!(
            "transcript_deferred_entries",
            left.transcript
                .deferred_entries
                .same_owner(&right.transcript.deferred_entries)
        );
        reuse!(
            "transcript_statistics",
            left.transcript
                .statistics
                .same_owner(&right.transcript.statistics)
        );
        field!(context);
        field!(effective_plugins);
        field!(workflows);
        field!(message_descriptors);
        field!(trace_updates);
        field!(inbound);
        field!(pending_interactions);
        field!(jobs);
        field!(agents);
        field!(statuses);
        field!(resources);
        field!(model);
        field!(todos);
        field!(goal);
        reuse!(
            "trace_page_updates",
            left.trace.updates.same_owner(&right.trace.updates)
        );
        reuse!(
            "trace_records",
            current.trace_head.same_owner(&previous.trace_head)
        );
        reuse!(
            "capabilities",
            current
                .capability_head
                .same_owner(&previous.capability_head)
        );
        result
    }

    pub(super) fn capture<'a, T: Clone + Serialize + Send + Sync + 'static>(
        &'a self,
        key: &'static str,
        capture: &mut crate::observation::capture::Capture,
        build: impl Fn(&mut crate::observation::capture::Capture) -> T,
    ) -> (std::borrow::Cow<'a, T>, Option<usize>) {
        let owner = self.0[key].get_or_init(|| {
            Arc::new(OnceLock::<crate::observation::capture::PreparedCapture<T>>::new())
        });
        capture.prepared(
            owner.downcast_ref().expect("one capture type per domain"),
            build,
        )
    }

    #[cfg(test)]
    pub(super) fn shared(&self, other: &Self, key: &str) -> bool {
        Arc::ptr_eq(&self.0[key], &other.0[key])
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingPrefix {
    Message(usize),
    Transcript,
}
type BindingFragments = checkpoint::metadata_fragment::BindingFragments;
struct BindingHead {
    values: checkpoint::MetadataFragment<Vec<crate::observation::cut::ContentBinding>>,
    budget: OnceLock<Option<usize>>,
    prefixed: std::sync::Mutex<Option<(BindingPrefix, BindingFragments)>>,
}
/// The native mutation owns this finite evidence. Only its latest positional
/// mapping is cached; previously published values remain immutable Arc owners.
#[derive(Clone)]
pub(super) struct PreparedBindingHead(Arc<BindingHead>);
impl Default for PreparedBindingHead {
    fn default() -> Self {
        Vec::new().into()
    }
}
impl From<Vec<crate::observation::cut::ContentBinding>> for PreparedBindingHead {
    fn from(values: Vec<crate::observation::cut::ContentBinding>) -> Self {
        Self::from(checkpoint::MetadataFragment::from(values))
    }
}
impl From<checkpoint::MetadataFragment<Vec<crate::observation::cut::ContentBinding>>>
    for PreparedBindingHead
{
    fn from(
        values: checkpoint::MetadataFragment<Vec<crate::observation::cut::ContentBinding>>,
    ) -> Self {
        Self(Arc::new(BindingHead {
            values,
            budget: OnceLock::new(),
            prefixed: std::sync::Mutex::new(None),
        }))
    }
}
impl PreparedBindingHead {
    pub(super) fn values(&self) -> &[crate::observation::cut::ContentBinding] {
        &self.0.values
    }
    pub(super) fn budget(&self) -> Option<usize> {
        *self.0.budget.get_or_init(|| {
            crate::observation::preflight::required_safe_integer_budget(
                &self.0.values,
                crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
            )
        })
    }
    pub(super) fn message(&self, index: usize) -> BindingFragments {
        self.prefixed(BindingPrefix::Message(index))
    }
    pub(super) fn transcript(&self) -> BindingFragments {
        self.prefixed(BindingPrefix::Transcript)
    }
    fn prefixed(&self, prefix: BindingPrefix) -> BindingFragments {
        let mut current = self
            .0
            .prefixed
            .lock()
            .expect("immutable binding prefix cache");
        if let Some((old, values)) = &*current
            && *old == prefix
        {
            return Arc::clone(values);
        }
        #[cfg(test)]
        BINDING_WORK.with(|work| work.set(work.get() + 1));
        let values = Arc::new(
            self.0
                .values
                .iter()
                .map(|value| {
                    let path = match prefix {
                        BindingPrefix::Message(index) => {
                            let skip = usize::from(
                                value.path.first().is_some_and(|field| field == "display"),
                            );
                            ["messages".to_owned(), index.to_string()]
                                .into_iter()
                                .chain(value.path[skip..].iter().cloned())
                                .collect()
                        }
                        BindingPrefix::Transcript => std::iter::once("transcript".into())
                            .chain(value.path.iter().cloned())
                            .collect(),
                    };
                    crate::observation::cut::ContentBinding {
                        source_path: value.source_path.clone(),
                        path,
                        content_ref: Arc::clone(&value.content_ref),
                        deferred: value.deferred,
                    }
                    .into()
                })
                .collect(),
        );
        *current = Some((prefix, Arc::clone(&values)));
        values
    }
}

#[cfg(test)]
thread_local! { static BINDING_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn take_binding_work() -> usize {
    BINDING_WORK.with(|work| work.replace(0))
}

/// Only the latest finite attempt evidence is retained. Exact value equality
/// includes position, source revision, readable end and deferred state; cache
/// ownership never grants authority to an older cut. Issued owners stay frozen.
#[derive(Clone, Default)]
pub(super) struct PreparedAttemptBindings(
    Arc<
        std::sync::Mutex<
            Vec<checkpoint::MetadataFragment<crate::observation::cut::ContentBinding>>,
        >,
    >,
);
impl PreparedAttemptBindings {
    pub(super) fn append(
        &self,
        values: Vec<crate::observation::cut::ContentBinding>,
        output: &mut Vec<checkpoint::MetadataFragment<crate::observation::cut::ContentBinding>>,
    ) {
        let mut current = self.0.lock().expect("immutable attempt binding cache");
        current.truncate(values.len());
        for (index, value) in values.into_iter().enumerate() {
            if let Some(owner) = current.get_mut(index) {
                if **owner != value {
                    *owner = value.into();
                }
            } else {
                current.push(value.into());
            }
        }
        output.extend(current.iter().cloned());
    }
}

type FrontierOwners =
    BTreeMap<String, checkpoint::MetadataFragment<crate::observation::cut::CollectionFrontier>>;
/// The most recently issued finite frontier set. Comparison touches only a
/// short domain name and four scalars; unchanged values keep their exact wire
/// certificate. No previous seed or unbounded history is retained.
#[derive(Clone, Default)]
pub(super) struct MetadataFrontiers {
    previous: Arc<FrontierOwners>,
    current: OnceLock<Arc<FrontierOwners>>,
}
impl MetadataFrontiers {
    pub(super) fn inherit(previous: Option<&CheckpointSeed>) -> Self {
        Self {
            previous: previous.map_or_else(Arc::default, |seed| {
                seed.metadata_frontiers
                    .current
                    .get()
                    .unwrap_or(&seed.metadata_frontiers.previous)
                    .clone()
            }),
            current: OnceLock::new(),
        }
    }
    pub(super) fn prepare(
        &self,
        values: Vec<crate::observation::cut::CollectionFrontier>,
    ) -> Vec<checkpoint::MetadataFragment<crate::observation::cut::CollectionFrontier>> {
        let owners = self.current.get().unwrap_or(&self.previous);
        // Collecting owned 64-byte frontiers into 8-byte owners can reuse the
        // source allocation and retain its much larger byte capacity. This is
        // the immutable pointer array: allocate only its own finite slots.
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            result.push(
                owners
                    .get(&value.domain)
                    .filter(|owner| ***owner == value)
                    .cloned()
                    .unwrap_or_else(|| value.into()),
            );
        }
        let _ = self.current.get_or_init(|| {
            Arc::new(
                result
                    .iter()
                    .map(|value| (value.domain.clone(), value.clone()))
                    .collect(),
            )
        });
        result
    }
}

/// Add sufficient native-encoding charges; an absent certificate always uses
/// the ordinary complete checkpoint admission, including its eviction order.
#[derive(Clone, Copy)]
pub(super) struct FitBudget(Option<usize>);
impl FitBudget {
    pub(super) fn from_budget(budget: Option<usize>) -> Self {
        Self(budget)
    }
    pub(super) fn new() -> Self {
        Self(Some(2))
    }
    pub(super) fn field(&mut self, name: &str, budget: Option<usize>) {
        self.0 = self
            .0
            .zip(budget)
            .and_then(|(total, value)| total.checked_add(name.len() + 5)?.checked_add(value));
    }
    pub(super) fn value(&mut self, name: &str, value: &impl Serialize) {
        self.field(
            name,
            crate::observation::preflight::required_safe_integer_budget(
                value,
                crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
            ),
        );
    }
    pub(super) fn finish(self) -> Option<usize> {
        self.0
    }

    pub(super) fn transition(events: Option<usize>, metadata: Option<usize>) -> Option<usize> {
        let mut budget = Self::new();
        budget.value("type", &"projection_transition");
        budget.field("events", events);
        budget.field("metadata", metadata);
        budget.finish()
    }

    pub(super) fn replacement(checkpoint: Option<usize>, metadata: Option<usize>) -> Option<usize> {
        let mut event = Self::new();
        event.value("type", &"checkpoint_advanced");
        event.value("transition", &"projection_transition");
        event.field("checkpoint", checkpoint);
        // Check::serialize_seq(Some(1)) plus its one element's delimiter.
        let events = event.finish().and_then(|budget| budget.checked_add(4));
        Self::transition(events, metadata)
    }
}

/// Capturing admission and metadata does not require owning a second complete
/// snapshot. The closure borrows normalized domains until a reader/replacement
/// actually needs them; pressured domains own their exact admission result.
pub(super) struct CapturedCheckpoint<S> {
    pub(super) snapshot: S,
    pub(super) metadata: checkpoint::ProjectionMetadata,
    pub(super) cursor: crate::runtime_client::RuntimeClientCursor,
    pub(super) evicted: bool,
    pub(super) fit_budget: Option<usize>,
    pub(super) metadata_budget: Option<usize>,
    pub(super) prepared_metadata: checkpoint::metadata_fragment::PreparedMetadata,
}

impl<S: FnOnce() -> crate::runtime_client::RuntimeClientSnapshot> CapturedCheckpoint<S> {
    pub(super) fn materialize(self) -> checkpoint::RuntimeClientCheckpoint {
        checkpoint::RuntimeClientCheckpoint {
            snapshot: (self.snapshot)(),
            cursor: self.cursor,
            read_cut: self.metadata.read_cut,
            content: self.metadata.content,
            collections: self.metadata.collections,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_frontier_owners_reuse_only_identical_fixed_fields() {
        let owners = MetadataFrontiers::default();
        let original = crate::observation::cut::CollectionFrontier {
            domain: "transcript.entries".into(),
            revision: 9,
            total: 12,
            loaded: 8,
            next_offset: Some(4),
        };
        let first = owners.prepare(vec![original.clone()]);
        let second = owners.prepare(vec![original.clone()]);
        assert!(first[0].same_owner(&second[0]));
        let before = serde_json::to_vec(&first).unwrap();
        for field in 0..5 {
            let mut changed = original.clone();
            match field {
                0 => changed.domain.push_str(".changed"),
                1 => changed.revision += 1,
                2 => changed.total += 1,
                3 => changed.loaded += 1,
                _ => changed.next_offset = None,
            }
            let current = owners.prepare(vec![changed]);
            assert!(!first[0].same_owner(&current[0]));
            assert_ne!(serde_json::to_vec(&current).unwrap(), before);
        }
        assert_eq!(serde_json::to_vec(&first).unwrap(), before);
    }
    #[test]
    fn frontier_pointer_array_does_not_retain_larger_value_allocation() {
        let mut values = Vec::with_capacity(128);
        for index in 0..37 {
            values.push(crate::observation::cut::CollectionFrontier {
                domain: format!("domain-{index}"),
                revision: 9,
                total: 1,
                loaded: 1,
                next_offset: None,
            });
        }
        let wanted = serde_json::to_vec(&values).unwrap();
        let prepared = MetadataFrontiers::default().prepare(values);
        assert!(
            prepared.capacity() <= prepared.len() * 2,
            "a pointer array must not keep the wider input's byte allocation"
        );
        assert_eq!(serde_json::to_vec(&prepared).unwrap(), wanted);
    }
}
