//! Request-free immutable Tool facts. Each append shares every older node.
use super::{CanonicalCallSource, ProviderWireCorrelationId};
use crate::model::{ModelProtocol, replay::ReplayOrigin};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoricalToolAssociation {
    pub call_message: u32,
    pub call_block: u32,
    pub result_message: u32,
    pub source: CanonicalCallSource,
    pub wire_id: Option<ProviderWireCorrelationId>,
    /// One shared set for the complete assistant; stored separately from each fact.
    #[serde(skip)]
    pub native_origins: Arc<Vec<[u8; 32]>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AssociationRoot {
    pub id: u64,
    pub digest: [u8; 32],
    pub count: u32,
    pub private_bytes: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredAssociation {
    pub owner: u64,
    pub epoch: u64,
    pub root: AssociationRoot,
}
#[derive(Debug)]
pub(crate) struct AssociationNode {
    pub parent: Option<Arc<Self>>,
    pub value: HistoricalToolAssociation,
    pub count: u32,
    pub first_origin_set: bool,
    pub stored: Mutex<Option<StoredAssociation>>,
}
// A retained old cut may own the last reference to a long suffix. Release it
// iteratively rather than recursing once for every historical association.
impl Drop for AssociationNode {
    fn drop(&mut self) {
        let mut next = self.parent.take();
        while let Some(parent) = next {
            match Arc::try_unwrap(parent) {
                Ok(mut node) => next = node.parent.take(),
                Err(_) => break,
            }
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NativeIds {
    ids: imbl::HashSet<String>,
    invalid: bool,
}
#[derive(Debug, Clone)]
struct NativeMembership {
    origins: Arc<Vec<[u8; 32]>>,
    occurrences: usize,
}
// Only the active replay origin consumes this summary. Keep one cursor into
// the immutable history instead of updating every possible origin per call.
#[derive(Debug, Clone, Default)]
struct NativeValidation {
    origin: Option<[u8; 32]>,
    count: usize,
    head: Weak<AssociationNode>,
    ids: NativeIds,
    membership: imbl::HashMap<u32, NativeMembership>,
}
#[derive(Debug, Clone, Default)]
pub struct ToolAssociationHistory {
    pub(crate) head: Option<Arc<AssociationNode>>,
    values: imbl::Vector<Arc<AssociationNode>>,
    native: Arc<Mutex<NativeValidation>>,
    origins: imbl::HashMap<u32, Arc<Vec<[u8; 32]>>>,
    empty_id: bool,
    has_native_origins: bool,
    pub(super) encoded_bytes_upper: usize,
}
impl PartialEq for ToolAssociationHistory {
    fn eq(&self, other: &Self) -> bool {
        self.same_prefix(other) || self.iter().eq(other.iter())
    }
}
impl Eq for ToolAssociationHistory {}
impl ToolAssociationHistory {
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &HistoricalToolAssociation> {
        self.values.iter().map(|node| {
            #[cfg(test)]
            HISTORICAL_VISITS.with(|work| work.set(work.get() + 1));
            &node.value
        })
    }
    pub(crate) fn same_prefix(&self, other: &Self) -> bool {
        match (&self.head, &other.head) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    /// Appends one immutable fact.
    ///
    /// # Panics
    /// Panics if the association count exceeds `u32::MAX`. Native selections
    /// admit at most 8192 occurrences.
    pub fn push_back(&mut self, mut value: HistoricalToolAssociation) {
        let first_origin_set = !self.origins.contains_key(&value.call_message);
        if let Some(origins) = self.origins.get(&value.call_message) {
            if Arc::ptr_eq(origins, &value.native_origins) || origins == &value.native_origins {
                value.native_origins = Arc::clone(origins);
            }
        } else {
            self.origins
                .insert(value.call_message, Arc::clone(&value.native_origins));
        }
        // Wire labels contain at most 64 ASCII bytes. Reserve ten decimal
        // digits for either shifted coordinate and bind the request ID later.
        let maximum = super::ActualRequestToolAssociation {
            call: super::ActualRequestCallCoordinate {
                message: crate::context::checkpoint::RequestMessageOccurrence {
                    request_id: crate::runtime::RequestId::new(""),
                    item_index: u32::MAX,
                },
                block: crate::context::checkpoint::RequestBlockIndex(value.call_block),
            },
            result: crate::context::checkpoint::RequestMessageOccurrence {
                request_id: crate::runtime::RequestId::new(""),
                item_index: u32::MAX,
            },
            source: value.source.clone(),
            wire_id: Some(ProviderWireCorrelationId(String::new())),
        };
        self.encoded_bytes_upper = self.encoded_bytes_upper.saturating_add(
            serde_json::to_vec(&maximum)
                .expect("Tool association serializes")
                .len()
                .saturating_add(64),
        );
        self.has_native_origins |= !value.native_origins.is_empty();
        let id = value.wire_id.as_ref().map_or("", |id| id.0.as_str());
        self.empty_id |= id.is_empty();
        let node = Arc::new(AssociationNode {
            parent: self.head.clone(),
            value,
            first_origin_set,
            count: u32::try_from(self.len() + 1).expect("bounded Tool history"),
            stored: Mutex::new(None),
        });
        self.values.push_back(node.clone());
        self.head = Some(node);
    }
    #[cfg(test)]
    pub(crate) fn native_summary_entries(&self) -> usize {
        self.native
            .lock()
            .expect("native replay summary")
            .ids
            .ids
            .len()
    }
    pub(crate) fn validate_wire_policy(&self, policy: &WirePolicy) -> Result<(), String> {
        if policy.stored_boundary.is_some() {
            return Ok(());
        }
        if self.empty_id {
            return Err("empty provider wire correlation ID".into());
        }
        if self.has_native_origins
            && policy.protocol == ModelProtocol::OpenAiResponses
            && let Some(origin) = &policy.origin
        {
            self.validate_native(origin)?;
        }
        Ok(())
    }
    fn validate_native(&self, origin: &ReplayOrigin) -> Result<(), String> {
        let origin = origin_key(origin);
        let mut cache = self.native.lock().expect("native replay summary");
        let matches = cache.count <= self.len()
            && (cache.count == 0
                || cache.head.upgrade().is_some_and(|head| {
                    self.values
                        .get(cache.count - 1)
                        .is_some_and(|node| Arc::ptr_eq(node, &head))
                }));
        if cache.origin != Some(origin) || !matches {
            *cache = NativeValidation {
                origin: Some(origin),
                ..NativeValidation::default()
            };
        }
        for index in cache.count..self.len() {
            let value = &self.values[index].value;
            #[cfg(test)]
            NATIVE_WORK.with(|work| {
                let (origins, calls) = work.get();
                work.set((origins, calls + 1));
            });
            let occurrences = if let Some(membership) = cache
                .membership
                .get(&value.call_message)
                .filter(|membership| Arc::ptr_eq(&membership.origins, &value.native_origins))
            {
                membership.occurrences
            } else {
                let occurrences = value
                    .native_origins
                    .iter()
                    .filter(|candidate| {
                        #[cfg(test)]
                        NATIVE_WORK.with(|work| {
                            let (origins, calls) = work.get();
                            work.set((origins + 1, calls));
                        });
                        *candidate == &origin
                    })
                    .count();
                cache.membership.insert(
                    value.call_message,
                    NativeMembership {
                        origins: value.native_origins.clone(),
                        occurrences,
                    },
                );
                occurrences
            };
            if occurrences > 0 {
                let id = value.wire_id.as_ref().map_or("", |id| id.0.as_str());
                cache.ids.invalid |=
                    occurrences > 1 || !valid_native_id(id) || cache.ids.ids.contains(id);
                cache.ids.ids.insert(id.to_owned());
            }
        }
        cache.count = self.len();
        cache.head = self.head.as_ref().map_or_else(Weak::new, Arc::downgrade);
        if cache.ids.invalid {
            return Err("native output has conflicting or invalid wire correlation IDs".into());
        }
        Ok(())
    }
}
impl FromIterator<HistoricalToolAssociation> for ToolAssociationHistory {
    fn from_iter<T: IntoIterator<Item = HistoricalToolAssociation>>(values: T) -> Self {
        let mut history = Self::default();
        for value in values {
            history.push_back(value);
        }
        history
    }
}
fn origin_key(origin: &ReplayOrigin) -> [u8; 32] {
    let bytes = serde_json::to_vec(origin).expect("replay origin serializes");
    Sha256::digest(bytes).into()
}
fn valid_native_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
pub(crate) fn native_origins(
    message: &crate::model::projected::ModelMessageView<'_>,
) -> Arc<Vec<[u8; 32]>> {
    let crate::model::projected::ModelMessageView::Assistant(content) = message else {
        return Arc::default();
    };
    let mut origins = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in *content {
        if let crate::message::types::AssistantContentBlock::Reasoning(reasoning) = block
            && matches!(
                reasoning.provider_state,
                Some(
                    crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                        crate::runtime::continuation::OpenAiResponsesContinuation::Stateless { .. }
                    )
                )
            )
            && let Some(origin) = &reasoning.origin
        {
            let key = origin_key(origin);
            if seen.insert(key) {
                origins.push(key);
            }
        }
    }
    Arc::new(origins)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WirePolicy {
    pub protocol: ModelProtocol,
    pub origin: Option<ReplayOrigin>,
    pub stored_boundary: Option<u32>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AssociationCoordinates {
    pub inserted_at: Option<u32>,
    pub wire: Option<WirePolicy>,
}
impl AssociationCoordinates {
    pub(crate) fn position(&self, index: u32) -> u32 {
        index + u32::from(self.inserted_at.is_some_and(|at| index >= at))
    }
    pub(crate) fn labels(
        &self,
        history: &ToolAssociationHistory,
    ) -> std::collections::BTreeMap<(u32, u32), String> {
        let Some(policy) = &self.wire else {
            return std::collections::BTreeMap::default();
        };
        let origin = policy.origin.as_ref().map(origin_key);
        let mut calls = history
            .iter()
            .filter_map(|a| {
                let message = self.position(a.call_message);
                if policy.stored_boundary.is_some_and(|start| message < start) {
                    return None;
                }
                let native = policy.protocol == ModelProtocol::OpenAiResponses
                    && (policy.stored_boundary == Some(message)
                        || origin
                            .as_ref()
                            .is_some_and(|origin| a.native_origins.contains(origin)));
                Some((
                    (message, a.call_block),
                    a.wire_id.as_ref().expect("validated wire ID").0.clone(),
                    native,
                ))
            })
            .collect::<Vec<_>>();
        calls.sort_by_key(|call| call.0);
        let order = (0..calls.len())
            .filter(|i| !calls[*i].2)
            .chain((0..calls.len()).filter(|i| calls[*i].2))
            .collect::<Vec<_>>();
        let labels = crate::model::adapter::correlation::wire_labels(
            &order
                .iter()
                .map(|i| calls[*i].1.clone())
                .collect::<Vec<_>>(),
            policy.protocol,
        );
        order
            .into_iter()
            .zip(labels)
            .map(|(index, label)| (calls[index].0, label))
            .collect()
    }
}

#[cfg(test)]
thread_local! {pub(crate) static HISTORICAL_VISITS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};}

#[cfg(test)]
thread_local! { static NATIVE_WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))}; }
#[cfg(test)]
mod tests;
