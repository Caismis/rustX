//! Transient preparation certificates belong to a selected-input lineage, never
//! to a public message ID or a durable observation domain. They retain only the
//! latest input and share immutable owners with it. Edits and old cuts are misses.
use std::sync::Arc;

use crate::model::SelectedModelInput;
use crate::model::catalog::Modality;
pub(crate) use rustx_types::model::adapter::correlation::Prefix;

#[derive(Default)]
struct Admission {
    prefix: Prefix,
    modalities: Vec<Modality>,
    images: usize,
}

#[derive(Default)]
pub(crate) struct RequestCache {
    admission: Admission,
    pub(super) correlation: rustx_types::model::adapter::correlation::CorrelationSlot,
    pub(super) chat: Option<super::openai::chat_completions::ChatPrefix>,
    pub(super) responses: Option<super::openai::responses::ResponsesPrefix>,
    pub(super) anthropic: Option<super::anthropic::mapping::AnthropicPrefix>,
    pub(super) tools: Option<Arc<super::validation::ToolCatalog>>,
}
impl std::fmt::Debug for RequestCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestCache")
            .field("checked_items", &self.admission.prefix.len())
            .finish_non_exhaustive()
    }
}
impl RequestCache {
    pub(super) fn admission(&mut self, input: &SelectedModelInput) -> (Vec<Modality>, usize) {
        let cache = &mut self.admission;
        if !cache.prefix.matches(input) {
            *cache = Admission::default();
        }
        let start = cache.prefix.len();
        {
            #[cfg(feature = "issue-387-profile")]
            let _phase = super::profile::phase(super::profile::Phase::Modalities);
            rustx_types::model::request_params::append_content_modalities(
                input
                    .iter()
                    .enumerate()
                    .skip(start)
                    .map(|(index, message)| {
                        #[cfg(not(feature = "issue-387-profile"))]
                        let _ = index;
                        #[cfg(feature = "issue-387-profile")]
                        super::profile::item(index);
                        #[cfg(test)]
                        work(|work| work.admission_items += 1);
                        message
                    }),
                &mut cache.modalities,
            );
        }
        {
            #[cfg(feature = "issue-387-profile")]
            let _phase = super::profile::phase(super::profile::Phase::ImageCount);
            cache.images += crate::model::images::image_count(input.iter().skip(start));
        }
        cache.prefix = Prefix::capture(input);
        (cache.modalities.clone(), cache.images)
    }
}

pub(super) fn admission(input: &SelectedModelInput) -> (Vec<Modality>, usize) {
    lock(input).admission(input)
}

/// Encode new fragments within the same admission budget as the final body.
/// Unbounded preparation uses serde's direct `RawValue` encoder; capped input
/// never materializes an oversized fragment before discovering the limit.
pub(super) fn encode_raw<T: serde::Serialize>(
    value: &T,
    limit: Option<usize>,
) -> Result<Arc<serde_json::value::RawValue>, crate::model::ModelError> {
    let raw = if let Some(limit) = limit {
        let bytes = rustx_bounded_json::encode_capped_bytes(value, limit)
            .map_err(|error| super::validation::wire_encoding_error(&error))?;
        let json = String::from_utf8(bytes).expect("encoded JSON is UTF-8");
        serde_json::value::RawValue::from_string(json)
            .map_err(|error| super::validation::wire_encoding_error(&error))?
    } else {
        serde_json::value::to_raw_value(value)
            .map_err(|error| super::validation::wire_encoding_error(&error))?
    };
    Ok(raw.into())
}

#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Work {
    pub admission_items: usize,
    pub correlation_items: usize,
    pub encoded_items: usize,
    pub encoded_bytes: usize,
    pub catalog_bytes: usize,
}
#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<Work> = const {std::cell::Cell::new(Work {admission_items:0,correlation_items:0,encoded_items:0,encoded_bytes:0,catalog_bytes:0})}; }
#[cfg(test)]
pub(crate) fn work(update: impl FnOnce(&mut Work)) {
    WORK.with(|slot| {
        let mut value = slot.get();
        update(&mut value);
        slot.set(value);
    });
}
#[cfg(test)]
pub(crate) fn take_work() -> Work {
    let mut work = WORK.with(|slot| slot.replace(Work::default()));
    work.correlation_items +=
        rustx_types::model::adapter::correlation::take_correlation_work().correlation_items;
    work
}

#[cfg(test)]
pub(crate) mod tests;

/// Locks the selected input's preparation cell and exposes the engine-side
/// [`RequestCache`] stored inside it.
pub(crate) fn lock(messages: &crate::model::SelectedModelInput) -> Guard<'_> {
    Guard {
        guard: messages
            .preparation()
            .lock()
            .expect("input preparation cache"),
    }
}

/// Deref'd view of the [`RequestCache`] living in the preparation cell.
pub(crate) struct Guard<'a> {
    guard: std::sync::MutexGuard<'a, rustx_types::model::selected_input::InputPreparation>,
}

impl std::ops::Deref for Guard<'_> {
    type Target = RequestCache;
    fn deref(&self) -> &RequestCache {
        self.guard
            .engine
            .as_ref()
            .and_then(|cell| cell.downcast_ref::<RequestCache>())
            .expect("preparation cell holds RequestCache")
    }
}

impl std::ops::DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut RequestCache {
        self.guard
            .engine
            .get_or_insert_with(|| Box::new(RequestCache::default()))
            .downcast_mut::<RequestCache>()
            .expect("preparation cell holds RequestCache")
    }
}
