//! Immutable native metadata and its bounded encoding certificates.
use super::cut::ContentBinding;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
mod read_cut;
pub use read_cut::ReadCutMap;
#[derive(Debug)]
struct Inner<T> {
    value: T,
    size: OnceLock<Size>,
    resolved: OnceLock<MetadataFragment<T>>,
}
impl<T: Clone> Clone for Inner<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            size: OnceLock::new(),
            resolved: OnceLock::new(),
        }
    }
}

/// A transparent immutable metadata value. Clone shares both its native value
/// and its exact structural encoding certificate; mutation creates a new owner.
#[derive(Debug)]
pub struct MetadataFragment<T>(Arc<Inner<T>>);
impl<T> Clone for MetadataFragment<T> {
    fn clone(&self) -> Self {
        #[cfg(test)]
        CLONE_WORK.with(|work| work.set(work.get() + 1));
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
impl<T: Default> Default for MetadataFragment<T> {
    fn default() -> Self {
        T::default().into()
    }
}
impl<T> std::ops::Deref for MetadataFragment<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0.value
    }
}
impl<T: Clone> std::ops::DerefMut for MetadataFragment<T> {
    fn deref_mut(&mut self) -> &mut T {
        let inner = Arc::make_mut(&mut self.0);
        inner.size.take();
        inner.resolved.take();
        &mut inner.value
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
    fn inline_schema() -> bool {
        T::inline_schema()
    }
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
impl<T: Clone + IntoIterator> IntoIterator for MetadataFragment<T> {
    type Item = T::Item;
    type IntoIter = T::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.into_owned().into_iter()
    }
}
// The wrapped container exposes iter/iter_mut through Deref/DerefMut.
#[allow(clippy::into_iter_without_iter)]
impl<'a, T> IntoIterator for &'a MetadataFragment<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&**self).into_iter()
    }
}
#[allow(clippy::into_iter_without_iter)]
impl<'a, T: Clone> IntoIterator for &'a mut MetadataFragment<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&mut **self).into_iter()
    }
}
impl<T> MetadataFragment<T> {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    #[cfg(test)]
    pub(crate) fn owner_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}
impl<T: Clone> MetadataFragment<T> {
    pub(crate) fn into_owned(self) -> T {
        Arc::try_unwrap(self.0).map_or_else(|owner| owner.value.clone(), |owner| owner.value)
    }
}
impl MetadataFragment<ContentBinding> {
    pub(crate) fn resolve(&self, reference: &crate::observation::content::ContentRef) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) budget: Option<usize>,
    pub(crate) exact: Option<usize>,
}
impl<T: Serialize + schemars::JsonSchema> MetadataFragment<T> {
    pub(crate) fn size(&self) -> Size {
        *self.0.size.get_or_init(|| {
            #[cfg(test)]
            WORK.with(|work| work.set(work.get() + 1));
            Size::measure(&self.0.value)
        })
    }
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_work() -> usize {
    WORK.with(|work| work.replace(0))
}

#[cfg(test)]
thread_local! { static CLONE_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_clone_work() -> usize {
    CLONE_WORK.with(|work| work.replace(0))
}
