//! Admission certificates owned by one already prepared capability header.
use super::{CapabilitySourceView, CapabilityView, RuntimeClientSkill, RuntimeClientTool};
use crate::runtime_client::checkpoint::{Capture, CollectionFrontier};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

#[derive(Debug)]
struct PreparedPage<T> {
    items: Vec<(T, usize, usize)>,
    total: u64,
    invalid_head: bool,
}
impl<T: Serialize + DeserializeOwned + Clone> PreparedPage<T> {
    fn new(items: &[T]) -> Self {
        let mut prepared = Self {
            items: Vec::new(),
            total: items.len() as u64,
            invalid_head: false,
        };
        for item in items
            .iter()
            .take(crate::runtime_client::checkpoint::CHECKPOINT_PAGE_ITEMS)
        {
            let Some(row) = crate::runtime_client::checkpoint::prepare_capture_item(item) else {
                prepared.invalid_head = true;
                break;
            };
            prepared.items.push(row);
        }
        prepared
    }
    fn header(&self) -> Vec<T> {
        self.items.iter().map(|(item, _, _)| item.clone()).collect()
    }
    fn capture(&self, domain: &str, capture: &mut Capture) -> Vec<T> {
        let mut result = Vec::new();
        for (item, bytes, required) in &self.items {
            if !capture.admit_prepared(*bytes, *required) {
                break;
            }
            result.push(item.clone());
        }
        if result.len() == self.items.len() && self.invalid_head {
            capture.evicted = true;
        }
        let loaded = result.len() as u64;
        capture.frontier(
            domain,
            self.total,
            loaded,
            (loaded < self.total).then_some(loaded),
        );
        result
    }
}

#[derive(Debug)]
struct PreparedCapability {
    revision: crate::runtime::identity::CapabilityRevision,
    configured: PreparedPage<RuntimeClientTool>,
    tools: PreparedPage<RuntimeClientTool>,
    available: PreparedPage<RuntimeClientTool>,
    skills: PreparedPage<RuntimeClientSkill>,
    sources: PreparedPage<CapabilitySourceView>,
    frontiers: Vec<CollectionFrontier>,
}

/// Only bounded normalized header rows and fixed per-row charges are retained.
/// This handle carries no native registry, availability owner or read authority.
#[derive(Debug, Clone)]
pub(crate) struct PreparedCapabilityHead(Arc<PreparedCapability>);
impl PreparedCapabilityHead {
    pub(crate) fn new(header: &CapabilityView, frontiers: Vec<CollectionFrontier>) -> Self {
        Self(Arc::new(PreparedCapability {
            revision: header.revision,
            configured: PreparedPage::new(&header.configured_tools),
            tools: PreparedPage::new(&header.tools),
            available: PreparedPage::new(&header.available_tools),
            skills: PreparedPage::new(&header.skills),
            sources: PreparedPage::new(&header.sources),
            frontiers,
        }))
    }
    pub(crate) fn header(&self) -> CapabilityView {
        CapabilityView {
            revision: self.0.revision,
            configured_tools: self.0.configured.header(),
            tools: self.0.tools.header(),
            available_tools: self.0.available.header(),
            skills: self.0.skills.header(),
            sources: self.0.sources.header(),
        }
    }
    pub(crate) fn capture(&self, capture: &mut Capture) -> CapabilityView {
        CapabilityView {
            revision: self.0.revision,
            configured_tools: self
                .0
                .configured
                .capture("capabilities.configured_tools", capture),
            tools: self.0.tools.capture("capabilities.tools", capture),
            available_tools: self
                .0
                .available
                .capture("capabilities.available_tools", capture),
            skills: self.0.skills.capture("capabilities.skills", capture),
            sources: self.0.sources.capture("capabilities.sources", capture),
        }
    }
    #[cfg(test)]
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    #[cfg(test)]
    pub(crate) fn metadata_entries(&self) -> usize {
        self.0.configured.items.len()
            + self.0.tools.items.len()
            + self.0.available.items.len()
            + self.0.skills.items.len()
            + self.0.sources.items.len()
    }
    pub(crate) fn frontiers(&self) -> Vec<CollectionFrontier> {
        self.0.frontiers.clone()
    }
}
