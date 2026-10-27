//! Immutable metadata keeps its validated wire size with the value that owns it.
//! The cache is never keyed by a revision, JSON equality, or a lossy hash.
use super::{CHECKPOINT_TARGET_BYTES, ContentBinding, ProjectionMetadata};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
struct Inner<T> {
    value: T,
    size: OnceLock<Size>,
    resolved: OnceLock<MetadataFragment<T>>,
}

/// A transparent immutable metadata value. Clone shares both its native value
/// and its exact structural encoding certificate; mutation creates a new owner.
#[derive(Debug)]
pub struct MetadataFragment<T>(Arc<Inner<T>>);
impl<T> Clone for MetadataFragment<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T> From<T> for MetadataFragment<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(Inner {
            value,
            size: OnceLock::new(),
            resolved: OnceLock::new(),
        }))
    }
}
impl<T> std::ops::Deref for MetadataFragment<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0.value
    }
}
impl<T: PartialEq> PartialEq for MetadataFragment<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.value == other.0.value
    }
}
impl<T: Eq> Eq for MetadataFragment<T> {}
impl<T: Serialize> Serialize for MetadataFragment<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.value.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for MetadataFragment<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::from)
    }
}
impl<T: schemars::JsonSchema> schemars::JsonSchema for MetadataFragment<T> {
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
impl<T> MetadataFragment<T> {
    #[cfg(test)]
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T: Clone> MetadataFragment<T> {
    pub(crate) fn into_owned(self) -> T {
        Arc::try_unwrap(self.0).map_or_else(|owner| owner.value.clone(), |owner| owner.value)
    }
}
impl MetadataFragment<ContentBinding> {
    pub(crate) fn resolve(&self, reference: &crate::runtime_client::content::ContentRef) -> Self {
        if &self.content_ref == reference {
            return self.clone();
        }
        // One alternate belongs to the original captured binding. A changed
        // authority gets a fresh owner; it can never overwrite an old cut.
        let resolved = self.0.resolved.get_or_init(|| {
            let mut value = self.0.value.clone();
            value.content_ref.clone_from(reference);
            value.into()
        });
        if &resolved.content_ref == reference {
            resolved.clone()
        } else {
            let mut value = self.0.value.clone();
            value.content_ref.clone_from(reference);
            value.into()
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Size {
    pub(crate) budget: Option<usize>,
    pub(crate) exact: Option<usize>,
}
impl<T: Serialize + schemars::JsonSchema> MetadataFragment<T> {
    pub(crate) fn size(&self) -> Size {
        *self.0.size.get_or_init(|| {
            #[cfg(test)]
            WORK.with(|work| work.set(work.get() + 1));
            let budget = crate::runtime_client::preflight::required_safe_integer_budget(
                &self.0.value,
                CHECKPOINT_TARGET_BYTES,
            );
            let exact = super::bounded_structural_len_with_budget(
                &self.0.value,
                CHECKPOINT_TARGET_BYTES,
                budget,
            );
            Size { budget, exact }
        })
    }
}

pub(crate) fn array_budget<T: Serialize + schemars::JsonSchema>(
    values: &[MetadataFragment<T>],
) -> Option<usize> {
    // Check::serialize_seq reserves len + 2, then one delimiter per element.
    values
        .iter()
        .try_fold(
            values.len().checked_mul(2)?.checked_add(2)?,
            |sum, value| sum.checked_add(value.size().budget?),
        )
        .filter(|budget| *budget <= CHECKPOINT_TARGET_BYTES)
}
fn array_length<T: Serialize + schemars::JsonSchema>(
    values: &[MetadataFragment<T>],
) -> Option<usize> {
    values
        .iter()
        .try_fold(2 + values.len().saturating_sub(1), |sum, value| {
            sum.checked_add(value.size().exact?)
        })
}

impl ProjectionMetadata {
    pub(crate) fn fragment_budget(&self) -> Option<usize> {
        let read_cut = crate::runtime_client::preflight::required_safe_integer_budget(
            &self.read_cut,
            CHECKPOINT_TARGET_BYTES,
        )?;
        // The same conservative struct charge as FitBudget::field.
        2_usize
            .checked_add("read_cut".len() + 5)?
            .checked_add(read_cut)?
            .checked_add("collections".len() + 5)?
            .checked_add(array_budget(&self.collections)?)?
            .checked_add("content".len() + 5)?
            .checked_add(array_budget(&self.content)?)
    }

    fn exact_length(&self, budget: Option<usize>) -> Option<usize> {
        let read_cut = super::bounded_structural_len_with_budget(
            &self.read_cut,
            CHECKPOINT_TARGET_BYTES,
            budget,
        )?;
        b"{\"read_cut\":,\"collections\":,\"content\":}"
            .len()
            .checked_add(read_cut)?
            .checked_add(array_length(&self.collections)?)?
            .checked_add(array_length(&self.content)?)
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
) -> Option<usize> {
    if prepared_budget.is_none_or(|budget| budget > limit) {
        return super::bounded_structural_len_with_budget(event, limit, prepared_budget);
    }
    let crate::runtime_client::event::RuntimeClientEvent::ProjectionTransition { events, metadata } =
        event
    else {
        return super::bounded_structural_len_with_budget(event, limit, None);
    };
    let events =
        super::bounded_structural_len_with_budget(events, limit, event_budget.or(prepared_budget))?;
    let length = b"{\"type\":\"projection_transition\",\"events\":,\"metadata\":}"
        .len()
        .checked_add(events)?
        .checked_add(metadata.exact_length(prepared_budget)?)?;
    (length <= limit).then_some(length)
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_work() -> usize {
    WORK.with(|work| work.replace(0))
}
