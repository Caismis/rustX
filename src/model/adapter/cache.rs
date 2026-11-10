//! Transient preparation certificates belong to a selected-input lineage, never
//! to a public message ID or a durable observation domain. They retain only the
//! latest input and share immutable owners with it. Edits and old cuts are misses.
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::model::catalog::Modality;
use crate::model::selected_input::InputNode;
use crate::model::{ModelInputMessage, SelectedModelInput};
use crate::runtime::identity::MessageId;

#[derive(Clone, Default)]
pub(crate) struct Prefix {
    root: Option<Arc<InputNode>>,
}
impl Prefix {
    pub(crate) fn capture(input: &SelectedModelInput) -> Self {
        Self {
            root: input.0.root.clone(),
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |node| node.count)
    }
    pub(crate) fn matches(&self, input: &SelectedModelInput) -> bool {
        input.shares_prefix(self.root.as_ref())
    }
}

#[derive(Default)]
struct Admission {
    prefix: Prefix,
    modalities: Vec<Modality>,
    images: usize,
}

#[derive(Default)]
struct ContinuationIndex {
    prefix: Prefix,
    positions: imbl::HashMap<MessageId, imbl::Vector<usize>>,
}

#[derive(Clone, Default)]
struct SemanticPrefix {
    prefix: Prefix,
    sha: Sha256,
}

#[derive(Default)]
pub(crate) struct RequestCache {
    admission: Admission,
    continuation: Option<ContinuationIndex>,
    semantic: SemanticPrefix,
    pub(super) correlation: Option<super::correlation::Plan>,
    pub(super) correlation_epoch: u64,
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
            crate::model::invocation::append_content_modalities(
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
    fn candidates(&mut self, input: &SelectedModelInput, id: &MessageId) -> imbl::Vector<usize> {
        let cache = self
            .continuation
            .get_or_insert_with(ContinuationIndex::default);
        if !cache.prefix.matches(input) {
            *cache = ContinuationIndex::default();
        }
        for (index, message) in input.iter().enumerate().skip(cache.prefix.len()) {
            let mut ids = std::collections::HashSet::new();
            if let Some(id) = message.canonical_id() {
                ids.insert(id);
            }
            if let ModelInputMessage::Derived(message) = message {
                ids.extend(message.sources.iter().map(|source| &source.message_id));
            }
            for id in ids {
                cache
                    .positions
                    .entry(id.clone())
                    .or_default()
                    .push_back(index);
            }
        }
        cache.prefix = Prefix::capture(input);
        cache.positions.get(id).cloned().unwrap_or_default()
    }
    fn fingerprint(&mut self, input: &SelectedModelInput, count: usize) -> String {
        let prefix = input.range(0..count);
        let cache = &mut self.semantic;
        if !cache.prefix.matches(&prefix) {
            *cache = SemanticPrefix::default();
        }
        for message in prefix.iter().skip(cache.prefix.len()) {
            crate::model::types::append_semantic_message(&mut cache.sha, message);
        }
        cache.prefix = Prefix::capture(&prefix);
        format!("{:x}", cache.sha.clone().finalize())
    }
}

pub(super) fn admission(input: &SelectedModelInput) -> (Vec<Modality>, usize) {
    input
        .preparation()
        .lock()
        .expect("input preparation cache")
        .admission(input)
}

/// Encode new fragments within the same admission budget as the final body.
/// Unbounded preparation uses serde's direct `RawValue` encoder; capped input
/// never materializes an oversized fragment before discovering the limit.
pub(super) fn encode_raw<T: serde::Serialize>(
    value: &T,
    limit: Option<usize>,
) -> Result<Arc<serde_json::value::RawValue>, crate::model::ModelError> {
    let raw = if let Some(limit) = limit {
        let bytes = crate::bounded_json::encode_capped_bytes(value, limit)
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

pub(crate) fn continuation_candidates(
    input: &SelectedModelInput,
    id: &MessageId,
) -> imbl::Vector<usize> {
    input
        .preparation()
        .lock()
        .expect("input preparation cache")
        .candidates(input, id)
}

pub(crate) fn prefix_fingerprint(input: &SelectedModelInput, count: usize) -> String {
    input
        .preparation()
        .lock()
        .expect("input preparation cache")
        .fingerprint(input, count)
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
    WORK.with(|slot| slot.replace(Work::default()))
}

#[cfg(test)]
mod tests;
