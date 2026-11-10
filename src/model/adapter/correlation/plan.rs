//! Incremental form of the exact wire-label allocator. A new raw ID can displace
//! a prior generated label; only that label's collision group and its messages
//! become dirty. Native IDs retain the allocator's original priority.
use std::collections::BTreeSet;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::super::cache::{Prefix, RequestCache};
use crate::message::types::AssistantContentBlock;
use crate::model::projected::ModelMessageView;
use crate::model::replay::ReplayOrigin;
use crate::model::{ModelProtocol, ModelRequest};
use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};

#[derive(Clone, PartialEq, Eq)]
struct Key {
    protocol: ModelProtocol,
    origin: Option<ReplayOrigin>,
    stored_at: Option<usize>,
}
#[derive(Clone, Copy, Default)]
struct RawId {
    last: Option<usize>,
    native: Option<usize>,
}
#[derive(Clone)]
struct Call {
    coordinate: (u32, u32),
    allocation_index: usize,
    first: Arc<str>,
    label: Arc<str>,
    synthetic: bool,
    hash: Option<String>,
    result: Option<usize>,
}
#[derive(Clone, Default)]
pub(crate) struct Plan {
    key: Option<Key>,
    prefix: Prefix,
    pub(crate) epoch: u64,
    non_native: usize,
    calls: imbl::Vector<Arc<Call>>,
    coordinates: imbl::HashMap<(u32, u32), usize>,
    results: imbl::HashMap<usize, usize>,
    raw: imbl::HashMap<Arc<str>, RawId>,
    pending: imbl::HashMap<Arc<str>, usize>,
    synthetic: imbl::HashMap<Arc<str>, usize>,
    hashed: imbl::HashMap<Arc<str>, usize>,
    groups: imbl::HashMap<String, imbl::OrdSet<usize>>,
    pub(crate) dirty: imbl::OrdSet<usize>,
    wire_error: Option<&'static str>,
}
impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CorrelationPlan")
            .field("calls", &self.calls.len())
            .finish_non_exhaustive()
    }
}

pub(crate) fn prepare(
    cache: &mut RequestCache,
    request: &ModelRequest,
    boundary: Option<usize>,
) -> Result<Plan, String> {
    let key = Key {
        protocol: request.protocol(),
        origin: request.invocation.replay_origin(),
        stored_at: if super::is_stored(request) {
            Some(boundary.ok_or("stored continuation lacks accepted output")?)
        } else {
            None
        },
    };
    let mut plan =
        if let Some(plan) = cache.correlation.as_ref().filter(|plan| {
            plan.key.as_ref() == Some(&key) && plan.prefix.matches(&request.messages)
        }) {
            plan.clone()
        } else {
            cache.correlation_epoch = cache
                .correlation_epoch
                .checked_add(1)
                .expect("bounded request cache lifetime");
            cache.chat = None;
            cache.responses = None;
            cache.anthropic = None;
            Plan {
                key: Some(key),
                epoch: cache.correlation_epoch,
                ..Plan::default()
            }
        };
    plan.extend(request)?;
    cache.correlation = Some(plan.clone());
    Ok(plan)
}

impl Plan {
    fn extend(&mut self, request: &ModelRequest) -> Result<(), String> {
        let key = self.key.as_ref().expect("correlation key").clone();
        let start = self.prefix.len().max(key.stored_at.unwrap_or(0));
        for (message, input) in request.messages.iter().enumerate().skip(start) {
            #[cfg(test)]
            super::super::cache::work(|work| work.correlation_items += 1);
            #[cfg(feature = "issue-387-profile")]
            super::super::profile::item(message);
            match input.view() {
                ModelMessageView::Assistant(content) => {
                    let native = key.protocol == ModelProtocol::OpenAiResponses && (
                        key.stored_at == Some(message) || content.iter().any(|block| matches!(block,
                            AssistantContentBlock::Reasoning(reasoning) if reasoning.belongs_to(key.origin.as_ref())
                                && matches!(reasoning.provider_state, Some(ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless { .. })))
                        ))
                    );
                    for (block, item) in content.iter().enumerate() {
                        if let AssistantContentBlock::ToolCall(call) = item {
                            self.push(
                                (
                                    u32::try_from(message)
                                        .map_err(|_| "request message coordinate overflow")?,
                                    u32::try_from(block)
                                        .map_err(|_| "request block coordinate overflow")?,
                                ),
                                call.id.as_str(),
                                native,
                            )?;
                        }
                    }
                }
                ModelMessageView::Tool(id, _) => {
                    if id.as_str().is_empty() {
                        self.wire_error = Some("empty provider wire correlation ID");
                    }
                    if let Some(index) = self.pending.remove(id.as_str()) {
                        self.results.insert(message, index);
                        Arc::make_mut(&mut self.calls[index]).result = Some(message);
                    }
                }
                _ => {}
            }
        }
        self.prefix = Prefix::capture(&request.messages);
        Ok(())
    }

    fn protocol(&self) -> ModelProtocol {
        self.key.as_ref().expect("correlation key").protocol
    }

    fn set_label(&mut self, index: usize, label: Arc<str>) {
        if self.calls[index].label == label {
            return;
        }
        let call = Arc::make_mut(&mut self.calls[index]);
        call.label = label;
        self.dirty.insert(call.coordinate.0 as usize);
        if let Some(result) = call.result {
            self.dirty.insert(result);
        }
    }

    fn first_label(
        &mut self,
        index: usize,
        first: Arc<str>,
        synthetic: bool,
        affected: &mut BTreeSet<String>,
    ) {
        let previous = self.calls[index].clone();
        if previous.synthetic {
            self.synthetic.remove(previous.first.as_ref());
        }
        if let Some(hash) = &previous.hash {
            self.hashed.remove(previous.label.as_ref());
            self.groups
                .get_mut(hash)
                .expect("hashed label group")
                .remove(&index);
            affected.insert(hash.clone());
        }
        let hash = if super::valid_wire_id(&first, self.protocol()) {
            None
        } else {
            Some(format!("{:x}", Sha256::digest(first.as_bytes()))[..12].to_owned())
        };
        let call = Arc::make_mut(&mut self.calls[index]);
        call.first = first.clone();
        call.synthetic = synthetic;
        call.hash.clone_from(&hash);
        if synthetic {
            self.synthetic.insert(first.clone(), index);
        }
        if let Some(hash) = hash {
            self.groups.entry(hash.clone()).or_default().insert(index);
            affected.insert(hash);
        } else {
            self.set_label(index, first);
        }
    }

    fn synthetic_label(&mut self, index: usize, affected: &mut BTreeSet<String>) {
        let mut label = format!("rustx_occurrence_{}", self.calls[index].allocation_index);
        while self.raw.contains_key(label.as_str()) {
            label.push('_');
        }
        self.first_label(index, Arc::from(label), true, affected);
    }

    fn rehash(&mut self, hash: &str) {
        let indices = self.groups.get(hash).cloned().unwrap_or_default();
        if indices.is_empty() {
            self.groups.remove(hash);
            return;
        }
        for &index in &indices {
            self.hashed.remove(self.calls[index].label.as_ref());
        }
        for index in indices {
            let mut nonce = self.calls[index].allocation_index;
            let label = loop {
                let label = format!("rustx_{hash}_{nonce}");
                if !self.raw.contains_key(label.as_str())
                    && !self.hashed.contains_key(label.as_str())
                {
                    break Arc::<str>::from(label);
                }
                nonce = nonce
                    .checked_add(1)
                    .expect("labels are bounded by request memory");
            };
            self.hashed.insert(label.clone(), index);
            self.set_label(index, label);
        }
    }

    fn push(&mut self, coordinate: (u32, u32), raw: &str, native: bool) -> Result<(), String> {
        if raw.is_empty() {
            return Err("empty provider wire correlation ID".into());
        }
        let previous = self.raw.get(raw).copied().unwrap_or_default();
        if native && (previous.native.is_some() || !super::valid_wire_id(raw, self.protocol())) {
            return Err("native output has conflicting or invalid wire correlation IDs".into());
        }
        let index = self.calls.len();
        let raw: Arc<str> = Arc::from(raw);
        let mut affected = BTreeSet::new();
        if !self.raw.contains_key(raw.as_ref())
            && let Some(&shadowed) = self.hashed.get(raw.as_ref())
        {
            affected.insert(self.calls[shadowed].hash.clone().expect("hashed label"));
        }
        self.raw.insert(
            raw.clone(),
            RawId {
                last: if native { previous.last } else { Some(index) },
                native: if native { Some(index) } else { previous.native },
            },
        );
        if let Some(&shadowed) = self.synthetic.get(raw.as_ref()) {
            self.synthetic_label(shadowed, &mut affected);
        }
        self.calls.push_back(Arc::new(Call {
            coordinate,
            allocation_index: self.non_native,
            first: raw.clone(),
            label: raw.clone(),
            synthetic: false,
            hash: None,
            result: None,
        }));
        self.coordinates.insert(coordinate, index);
        if !native {
            self.non_native += 1;
        }
        if previous.native.is_none()
            && let Some(prior) = previous.last
        {
            self.synthetic_label(prior, &mut affected);
        }
        if !native && previous.native.is_some() {
            self.synthetic_label(index, &mut affected);
        } else {
            self.first_label(index, raw.clone(), false, &mut affected);
        }
        for hash in affected {
            self.rehash(&hash);
        }
        if self.key.as_ref().expect("correlation key").stored_at != Some(coordinate.0 as usize)
            && self.pending.insert(raw, index).is_some()
        {
            self.wire_error = Some("ambiguous unresolved provider wire correlation ID");
        }
        Ok(())
    }

    pub(crate) fn label(&self, coordinate: (u32, u32)) -> &str {
        &self.calls[*self
            .coordinates
            .get(&coordinate)
            .expect("translated call coordinate")]
        .label
    }
    pub(crate) fn matches(&self, input: &crate::model::SelectedModelInput) -> bool {
        self.prefix.len() == input.len() && self.prefix.matches(input)
    }
    pub(crate) fn result_label<'a>(&'a self, message: usize, raw: &'a str) -> &'a str {
        self.results
            .get(&message)
            .map_or(raw, |index| self.calls[*index].label.as_ref())
    }
    #[cfg(test)]
    pub(crate) fn transmitted(&self) -> impl Iterator<Item = &str> {
        let boundary = self.key.as_ref().and_then(|key| key.stored_at);
        self.calls
            .iter()
            .filter(move |call| boundary != Some(call.coordinate.0 as usize))
            .map(|call| call.label.as_ref())
    }
    pub(crate) fn coordinates(&self) -> std::collections::BTreeMap<(u32, u32), String> {
        self.calls
            .iter()
            .map(|call| (call.coordinate, call.label.to_string()))
            .collect()
    }
    pub(crate) fn validate_wire(&self) -> Result<(), &'static str> {
        self.wire_error.map_or(Ok(()), Err)
    }
    pub(crate) fn encode_message_fields(
        &self,
        index: usize,
        message: &crate::model::ModelInputMessage,
        mut visit: impl FnMut(&mut dyn FnMut(bool, &mut serde_json::Value)),
    ) -> Result<(), &'static str> {
        let content = match message.view() {
            ModelMessageView::Assistant(content) => content,
            _ => &[],
        };
        #[cfg(feature = "issue-387-profile")]
        let _profile = super::super::profile::phase(super::super::profile::Phase::WireCorrelation);
        #[cfg(feature = "issue-387-profile")]
        super::super::profile::wire_item();
        let mut labels = content
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, AssistantContentBlock::ToolCall(_)))
            .map(|(block, _)| {
                self.label((
                    u32::try_from(index).expect("validated message coordinate"),
                    u32::try_from(block).expect("validated block coordinate"),
                ))
            });
        let mut missing = false;
        let mut invalid = false;
        visit(&mut |call, id| {
            #[cfg(feature = "issue-387-profile")]
            super::super::profile::wire_field();
            let raw = id.as_str().unwrap_or_default();
            invalid |= raw.is_empty();
            let label = if call {
                labels.next().unwrap_or_else(|| {
                    missing = true;
                    raw
                })
            } else {
                self.result_label(index, raw)
            };
            *id = serde_json::Value::String(label.to_owned());
        });
        if invalid {
            return Err("empty provider wire correlation ID");
        }
        if missing || labels.next().is_some() {
            return Err("wire call count differs from request correlation plan");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
