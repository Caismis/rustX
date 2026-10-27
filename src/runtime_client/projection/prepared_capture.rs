//! Immutable capture fragments belong to one exact installed domain value.
//! Reuse compares the existing frozen native heads, never serialized witnesses.
use super::CheckpointSeed;
use crate::runtime_client::checkpoint;
use serde::{Serialize, Serializer, ser};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

type Owner = Arc<OnceLock<Arc<dyn Any + Send + Sync>>>;

#[derive(Clone)]
pub(super) struct PreparedCaptures(BTreeMap<&'static str, Owner>);
impl Default for PreparedCaptures {
    fn default() -> Self {
        Self(
            [
                "context",
                "effective_plugins",
                "workflows",
                "messages",
                "message_descriptors",
                "transcript_entries",
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
        Self(BTreeMap::new())
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
                reuse!(
                    stringify!($field),
                    encoding_equal(&left.$field, &right.$field)
                );
            };
        }
        // Capture of native message bodies uses the conversation identity in
        // deferred content locators, in addition to the body itself.
        reuse!(
            "messages",
            left.conversation_id == right.conversation_id
                && encoding_equal(&left.messages, &right.messages)
        );
        reuse!(
            "transcript_entries",
            left.conversation_id == right.conversation_id
                && encoding_equal(&left.transcript.entries, &right.transcript.entries)
        );
        reuse!(
            "transcript_deferred_entries",
            encoding_equal(
                &left.transcript.deferred_entries,
                &right.transcript.deferred_entries
            )
        );
        reuse!(
            "transcript_statistics",
            encoding_equal(&left.transcript.statistics, &right.transcript.statistics)
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
            encoding_equal(&left.trace.updates, &right.trace.updates)
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
        capture: &mut checkpoint::Capture,
        build: impl Fn(&mut checkpoint::Capture) -> T,
    ) -> (std::borrow::Cow<'a, T>, Option<usize>) {
        let owner =
            self.0[key].get_or_init(|| Arc::new(OnceLock::<checkpoint::PreparedCapture<T>>::new()));
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
type BindingFragments = Arc<Vec<checkpoint::MetadataFragment<checkpoint::ContentBinding>>>;
struct BindingHead {
    values: Vec<checkpoint::ContentBinding>,
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
impl From<Vec<checkpoint::ContentBinding>> for PreparedBindingHead {
    fn from(values: Vec<checkpoint::ContentBinding>) -> Self {
        Self(Arc::new(BindingHead {
            values,
            budget: OnceLock::new(),
            prefixed: std::sync::Mutex::new(None),
        }))
    }
}
impl PreparedBindingHead {
    pub(super) fn values(&self) -> &[checkpoint::ContentBinding] {
        &self.0.values
    }
    pub(super) fn budget(&self) -> Option<usize> {
        *self.0.budget.get_or_init(|| {
            crate::runtime_client::preflight::required_safe_integer_budget(
                &self.0.values,
                checkpoint::CHECKPOINT_TARGET_BYTES,
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
                    let mut value = value.clone();
                    match prefix {
                        BindingPrefix::Message(index) => {
                            if value.path.first().is_some_and(|field| field == "display") {
                                value.path.remove(0);
                            }
                            let mut path = vec!["messages".to_owned(), index.to_string()];
                            path.append(&mut value.path);
                            value.path = path;
                        }
                        BindingPrefix::Transcript => value.path.insert(0, "transcript".into()),
                    }
                    value.into()
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

type FrontierOwners =
    BTreeMap<String, checkpoint::MetadataFragment<checkpoint::CollectionFrontier>>;
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
        values: Vec<checkpoint::CollectionFrontier>,
    ) -> Vec<checkpoint::MetadataFragment<checkpoint::CollectionFrontier>> {
        let owners = self.current.get().unwrap_or(&self.previous);
        let result: Vec<_> = values
            .into_iter()
            .map(|value| {
                owners
                    .get(&value.domain)
                    .filter(|owner| ***owner == value)
                    .cloned()
                    .unwrap_or_else(|| value.into())
            })
            .collect();
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
            crate::runtime_client::preflight::required_safe_integer_budget(
                value,
                checkpoint::CHECKPOINT_TARGET_BYTES,
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
}

impl<S: FnOnce() -> crate::runtime_client::RuntimeClientSnapshot> CapturedCheckpoint<S> {
    pub(super) fn materialize(self) -> checkpoint::RuntimeClientCheckpoint {
        checkpoint::RuntimeClientCheckpoint {
            snapshot: (self.snapshot)(),
            cursor: self.cursor,
            read_cut: self.metadata.read_cut,
            content: self
                .metadata
                .content
                .into_iter()
                .map(checkpoint::MetadataFragment::into_owned)
                .collect(),
            collections: self
                .metadata
                .collections
                .into_iter()
                .map(checkpoint::MetadataFragment::into_owned)
                .collect(),
        }
    }
}

/// Native DTO equality does not distinguish signed zero or the insertion
/// order of `serde_json` objects. Rebuild float-bearing or nonempty map-bearing
/// values; immutable native owner identities do not require this witness.
/// This visitor neither scans strings nor allocates JSON.
fn encoding_equal<T: PartialEq + Serialize>(left: &T, right: &T) -> bool {
    left == right && left.serialize(UnambiguousEquality).is_ok()
}
#[derive(Clone, Copy)]
struct UnambiguousEquality;
#[derive(Debug)]
struct Ambiguous;
impl std::fmt::Display for Ambiguous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("floating point or ordered map")
    }
}
impl std::error::Error for Ambiguous {}
impl ser::Error for Ambiguous {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}
macro_rules! scalar {
    ($($method:ident($type:ty)),* $(,)?) => { $(
        fn $method(self, _: $type) -> Result<(), Ambiguous> { Ok(()) }
    )* };
}
impl Serializer for UnambiguousEquality {
    type Ok = ();
    type Error = Ambiguous;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );
    fn serialize_f32(self, _: f32) -> Result<(), Ambiguous> {
        Err(Ambiguous)
    }
    fn serialize_f64(self, _: f64) -> Result<(), Ambiguous> {
        Err(Ambiguous)
    }
    fn serialize_none(self) -> Result<(), Ambiguous> {
        Ok(())
    }
    fn serialize_unit(self) -> Result<(), Ambiguous> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Ambiguous> {
        value.serialize(self)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Ambiguous> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Ambiguous> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Ambiguous> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Ambiguous> {
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, Ambiguous> {
        if len == Some(0) {
            Ok(self)
        } else {
            Err(Ambiguous)
        }
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Ambiguous> {
        Ok(self)
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, _: &T) -> Result<(), Ambiguous> {
        Ok(())
    }
}
macro_rules! compound {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for UnambiguousEquality {
            type Ok = ();
            type Error = Ambiguous;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Ambiguous> {
                value.serialize(*self)
            }
            fn end(self) -> Result<(), Ambiguous> {
                Ok(())
            }
        }
    };
}
compound!(SerializeSeq, serialize_element);
compound!(SerializeTuple, serialize_element);
compound!(SerializeTupleStruct, serialize_field);
compound!(SerializeTupleVariant, serialize_field);
impl ser::SerializeMap for UnambiguousEquality {
    type Ok = ();
    type Error = Ambiguous;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Ambiguous> {
        value.serialize(*self)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Ambiguous> {
        value.serialize(*self)
    }
    fn end(self) -> Result<(), Ambiguous> {
        Ok(())
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl ser::$trait for UnambiguousEquality {
            type Ok = ();
            type Error = Ambiguous;
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                _: &'static str,
                value: &T,
            ) -> Result<(), Ambiguous> {
                value.serialize(*self)
            }
            fn end(self) -> Result<(), Ambiguous> {
                Ok(())
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_frontier_owners_reuse_only_identical_fixed_fields() {
        let owners = MetadataFrontiers::default();
        let original = checkpoint::CollectionFrontier {
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
    #[allow(clippy::float_cmp)] // The test exercises exact native equality of signed zeros.
    fn incremental_checkpoint_reuse_rejects_signed_zero_nan_and_reordered_maps() {
        assert_eq!(0.0_f64, -0.0_f64);
        assert!(!encoding_equal(&0.0_f64, &-0.0_f64));
        assert!(!encoding_equal(&f64::NAN, &f64::NAN));
        let mut first = serde_json::Map::new();
        first.insert("alpha".into(), serde_json::json!(1));
        first.insert("beta".into(), serde_json::json!(2));
        let mut second = serde_json::Map::new();
        second.insert("beta".into(), serde_json::json!(2));
        second.insert("alpha".into(), serde_json::json!(1));
        let first = serde_json::Value::Object(first);
        let second = serde_json::Value::Object(second);
        assert_eq!(
            first, second,
            "native equality ignores object insertion order"
        );
        assert_ne!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert!(!encoding_equal(&first, &second));
        assert!(encoding_equal(
            &vec!["large unchanged body".repeat(100)],
            &vec!["large unchanged body".repeat(100)]
        ));
    }
}
