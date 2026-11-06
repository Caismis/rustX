//! Current projection metadata arrays share native immutable fragments.
use super::{CollectionFrontier, ContentBinding, ProjectionMetadata};
use crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
use crate::observation::metadata::{MetadataFragment, Size};
use serde::Serialize;
use std::sync::{Arc, Mutex, OnceLock};
fn array_size<T: Serialize + schemars::JsonSchema>(values: &[MetadataFragment<T>]) -> Size {
    // Check::serialize_seq reserves len + 2, then one delimiter per element.
    let mut size = Size {
        budget: values.len().checked_mul(2).and_then(|n| n.checked_add(2)),
        exact: values.len().saturating_sub(1).checked_add(2),
    };
    for value in values {
        let item = value.size();
        size.budget = size
            .budget
            .zip(item.budget)
            .and_then(|(a, b)| a.checked_add(b));
        size.exact = size
            .exact
            .zip(item.exact)
            .and_then(|(a, b)| a.checked_add(b));
    }
    size.budget = size
        .budget
        .filter(|budget| *budget <= CHECKPOINT_TARGET_BYTES);
    size
}

struct PreparedArray<T> {
    original: Vec<MetadataFragment<T>>,
    order: OnceLock<SortOrder>,
    size: Size,
}
enum SortOrder {
    Identity,
    /// Original index -> sorted destination. Owners remain only in `original`.
    Permute(Vec<usize>),
}
impl<T> PreparedArray<T> {
    fn sort_into(
        &self,
        values: &mut [MetadataFragment<T>],
        compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    ) {
        // prepare() authenticates this exact input order. The publication only
        // reads its budget before sorting, and applies this permutation once.
        debug_assert!(
            self.original.len() == values.len()
                && self
                    .original
                    .iter()
                    .zip(values.iter())
                    .all(|(a, b)| a.same_owner(b))
        );
        let order = self.order.get_or_init(|| {
            #[cfg(test)]
            ARRAY_WORK.with(|work| {
                let mut counts = work.get();
                counts[1] += 1;
                work.set(counts);
            });
            if self
                .original
                .windows(2)
                .all(|pair| !compare(&pair[0], &pair[1]).is_gt())
            {
                return SortOrder::Identity;
            }
            let mut sources: Vec<_> = (0..self.original.len()).collect();
            sources.sort_by(|a, b| compare(&self.original[*a], &self.original[*b]));
            let mut destinations = vec![0; sources.len()];
            for (destination, source) in sources.into_iter().enumerate() {
                destinations[source] = destination;
            }
            SortOrder::Permute(destinations)
        });
        if let SortOrder::Permute(destinations) = order {
            let mut remaining = destinations.clone();
            for index in 0..remaining.len() {
                while remaining[index] != index {
                    let destination = remaining[index];
                    values.swap(index, destination);
                    remaining.swap(index, destination);
                }
            }
        }
    }
}

struct ArrayCache<T>(Option<Arc<PreparedArray<T>>>);
impl<T> Default for ArrayCache<T> {
    fn default() -> Self {
        Self(None)
    }
}
impl<T: Serialize + schemars::JsonSchema> ArrayCache<T> {
    fn prepare(&mut self, values: &[MetadataFragment<T>]) -> Arc<PreparedArray<T>> {
        if let Some(previous) = &self.0
            && previous.original.len() == values.len()
            && previous
                .original
                .iter()
                .zip(values)
                .all(|(a, b)| a.same_owner(b))
        {
            #[cfg(test)]
            ARRAY_WORK.with(|work| {
                let mut counts = work.get();
                counts[2] += 1;
                work.set(counts);
            });
            return Arc::clone(previous);
        }
        #[cfg(test)]
        ARRAY_WORK.with(|work| {
            let mut counts = work.get();
            counts[0] += 1;
            work.set(counts);
        });
        let prepared = Arc::new(PreparedArray {
            original: values.to_vec(),
            order: OnceLock::new(),
            size: array_size(values),
        });
        self.0 = Some(Arc::clone(&prepared));
        prepared
    }
}

/// One latest array per kind, shared only as a computation cache. A retained
/// old seed may replace that entry but cannot change any published value.
/// Neither the cache nor its entries retain a seed or a previous cache entry.
#[derive(Clone, Default)]
pub(crate) struct MetadataArrays(Arc<Mutex<LatestMetadata>>);
pub(crate) type BindingFragments = Arc<Vec<MetadataFragment<ContentBinding>>>;
#[derive(Default)]
struct LatestMetadata {
    content: ArrayCache<ContentBinding>,
    collections: ArrayCache<CollectionFrontier>,
    evidence: Option<PreparedEvidence>,
}
struct PreparedEvidence {
    prefix: usize,
    heads: Vec<BindingFragments>,
    // Raw bindings only: current authority resolution still runs afterwards.
    merged: Vec<MetadataFragment<ContentBinding>>,
}
pub(crate) struct PreparedMetadata {
    content: Arc<PreparedArray<ContentBinding>>,
    collections: Arc<PreparedArray<CollectionFrontier>>,
    // Captured with these arrays; sorting never changes the read cut.
    read_cut: Size,
}
impl MetadataArrays {
    pub(crate) fn prepare(&self, metadata: &ProjectionMetadata) -> PreparedMetadata {
        let mut latest = self.0.lock().expect("immutable metadata cache");
        PreparedMetadata {
            content: latest.content.prepare(&metadata.content),
            collections: latest.collections.prepare(&metadata.collections),
            read_cut: metadata.read_cut.size(),
        }
    }

    /// Ordered immutable owners authenticate the complete merge input, including
    /// duplicates. This cache cannot authorize a resolved or historical source.
    pub(crate) fn append_evidence(
        &self,
        output: &mut Vec<MetadataFragment<ContentBinding>>,
        heads: Vec<BindingFragments>,
    ) {
        let mut latest = self.0.lock().expect("immutable metadata cache");
        if let Some(previous) = &latest.evidence
            && previous.prefix == output.len()
            && previous.heads.len() == heads.len()
            && previous
                .heads
                .iter()
                .zip(&heads)
                .all(|(a, b)| Arc::ptr_eq(a, b))
            && previous.merged[..previous.prefix]
                .iter()
                .zip(&*output)
                .all(|(a, b)| a.same_owner(b))
        {
            output.extend(previous.merged[previous.prefix..].iter().cloned());
            return;
        }
        #[cfg(test)]
        EVIDENCE_WORK.with(|work| work.set(work.get() + 1));
        let prefix = output.len();
        let mut paths = output
            .iter()
            .map(|binding| binding.path.as_slice())
            .collect::<std::collections::BTreeSet<_>>();
        let mut appended = Vec::new();
        for head in &heads {
            for binding in head.iter() {
                if paths.insert(binding.path.as_slice()) {
                    appended.push(binding.clone());
                }
            }
        }
        drop(paths);
        output.extend(appended);
        latest.evidence = Some(PreparedEvidence {
            prefix,
            heads,
            merged: output.clone(),
        });
    }
}
impl PreparedMetadata {
    pub(crate) fn sort_into(&self, metadata: &mut ProjectionMetadata) {
        self.content
            .sort_into(&mut metadata.content, |a, b| a.path.cmp(&b.path));
        self.collections
            .sort_into(&mut metadata.collections, |a, b| a.domain.cmp(&b.domain));
    }
    pub(crate) fn fragment_budget(&self) -> Option<usize> {
        ProjectionMetadata::budget_with_arrays(
            self.read_cut.budget,
            self.collections.size.budget,
            self.content.size.budget,
        )
    }
}

impl ProjectionMetadata {
    #[cfg(test)]
    pub(crate) fn fragment_budget(&self) -> Option<usize> {
        Self::budget_with_arrays(
            self.read_cut.size().budget,
            array_size(&self.collections).budget,
            array_size(&self.content).budget,
        )
    }
    fn budget_with_arrays(
        read_cut: Option<usize>,
        collections: Option<usize>,
        content: Option<usize>,
    ) -> Option<usize> {
        // The same conservative struct charge as FitBudget::field.
        2_usize
            .checked_add("read_cut".len() + 5)?
            .checked_add(read_cut?)?
            .checked_add("collections".len() + 5)?
            .checked_add(collections?)?
            .checked_add("content".len() + 5)?
            .checked_add(content?)
    }

    fn exact_length(&self, arrays: Option<&PreparedMetadata>) -> Option<usize> {
        let read_cut = arrays.map_or_else(
            || self.read_cut.size().exact,
            |arrays| arrays.read_cut.exact,
        )?;
        b"{\"read_cut\":,\"collections\":,\"content\":}"
            .len()
            .checked_add(read_cut)?
            .checked_add(arrays.map_or_else(
                || array_size(&self.collections).exact,
                |arrays| arrays.collections.size.exact,
            )?)?
            .checked_add(arrays.map_or_else(
                || array_size(&self.content).exact,
                |arrays| arrays.content.size.exact,
            )?)
    }
}

/// Compose exactly the existing closed transition envelope. The event array
/// still uses the schema-aware encoder, including opaque numbers and all native
/// constraints. Immutable metadata fragments were validated with that codec too.
pub(crate) fn transition_length(
    event: &crate::runtime_client::event::RuntimeClientEvent,
    event_budget: Option<usize>,
    prepared_budget: Option<usize>,
    limit: usize,
    arrays: Option<&PreparedMetadata>,
) -> Option<usize> {
    if prepared_budget.is_none_or(|budget| budget > limit) {
        return crate::observation::bounded::bounded_structural_len_with_budget(
            event,
            limit,
            prepared_budget,
        );
    }
    let crate::runtime_client::event::RuntimeClientEvent::ProjectionTransition { events, metadata } =
        event
    else {
        return crate::observation::bounded::bounded_structural_len_with_budget(event, limit, None);
    };
    let events = crate::observation::bounded::bounded_structural_len_with_budget(
        events,
        limit,
        event_budget.or(prepared_budget),
    )?;
    let length = b"{\"type\":\"projection_transition\",\"events\":,\"metadata\":}"
        .len()
        .checked_add(events)?
        .checked_add(metadata.exact_length(arrays)?)?;
    (length <= limit).then_some(length)
}

#[cfg(test)]
thread_local! { static ARRAY_WORK: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) }; }
#[cfg(test)]
pub(crate) fn take_array_work() -> [usize; 3] {
    ARRAY_WORK.with(|work| work.replace([0; 3]))
}

#[cfg(test)]
thread_local! {static EVIDENCE_WORK:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
pub(crate) fn take_evidence_work() -> usize {
    EVIDENCE_WORK.with(|work| work.replace(0))
}
