//! Ordered read-cut maps share immutable nodes and their wire-size certificates.
use super::{CHECKPOINT_TARGET_BYTES, CheckpointReadCut, metadata_fragment::Size};
use serde::{Deserialize, Serialize};

/// The wire value and JSON schema remain an ordinary ordered object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ReadCutMap<V: Clone>(
    #[schemars(with = "std::collections::BTreeMap<String, V>")]
    imbl::OrdMap<std::sync::Arc<str>, V>,
);
impl<V: Clone> Default for ReadCutMap<V> {
    fn default() -> Self {
        Self(imbl::OrdMap::new())
    }
}
impl<V: Clone> std::ops::Deref for ReadCutMap<V> {
    type Target = imbl::OrdMap<std::sync::Arc<str>, V>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<V: Clone> std::ops::DerefMut for ReadCutMap<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ReadCutMap<u64> {
    pub(crate) fn advance_revision(&mut self, owner: &str) -> Option<u64> {
        let next = self.0.get(owner).copied().unwrap_or(0).checked_add(1)?;
        if let Some(revision) = self.0.get_mut(owner) {
            *revision = next;
        } else {
            self.0.insert(owner.into(), next);
        }
        Some(next)
    }
}

impl Size {
    pub(super) fn measure(value: &(impl Serialize + schemars::JsonSchema)) -> Self {
        let budget = crate::runtime_client::preflight::required_safe_integer_budget(
            value,
            CHECKPOINT_TARGET_BYTES,
        );
        let exact =
            super::bounded_structural_len_with_budget(value, CHECKPOINT_TARGET_BYTES, budget);
        Self { budget, exact }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
struct Header<'a> {
    canonical_foreground: &'a Option<super::CanonicalForegroundReadCut>,
    live_publication: &'a Option<super::LivePublicationReadCut>,
    conversation_id: &'a crate::runtime::identity::ConversationId,
    activation_id: &'a Option<crate::runtime::identity::SubagentId>,
    revision: u64,
    through: u64,
    source_cut: &'a Option<crate::durable::observation::ObservationReadCut>,
}
impl CheckpointReadCut {
    pub(super) fn size(&self) -> Size {
        // Deliberately list all public fields: adding a field requires deciding
        // how its exact encoding and safe-integer charge are composed.
        let Self {
            canonical_foreground,
            live_publication,
            conversation_id,
            activation_id,
            revision,
            through,
            source_cut,
            domain_revisions,
            collection_cuts,
        } = self;
        let mut size = Size::measure(&Header {
            canonical_foreground,
            live_publication,
            conversation_id,
            activation_id,
            revision: *revision,
            through: *through,
            source_cut,
        });
        for (name, value) in [
            ("domain_revisions", domain_revisions.size()),
            ("collection_cuts", collection_cuts.size()),
        ] {
            size.budget = size
                .budget
                .zip(value.budget)
                .and_then(|(a, b)| a.checked_add(name.len() + 5)?.checked_add(b));
            // Append ,"name":value to the nonempty Header object.
            size.exact = size
                .exact
                .zip(value.exact)
                .and_then(|(a, b)| a.checked_add(name.len() + 4)?.checked_add(b));
        }
        size.budget = size
            .budget
            .filter(|value| *value <= CHECKPOINT_TARGET_BYTES);
        size.exact = size.exact.filter(|value| *value <= CHECKPOINT_TARGET_BYTES);
        size
    }
}

#[cfg(test)]
mod tests;
