//! Configuration commit vocabulary shared across layers.

use std::collections::BTreeMap;

/// Why the Session's configuration changed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationChangeReason {
    /// A saved or externally edited authored source.
    Source,
    /// An explicit Session model selection reconsidered the source.
    ModelSelection,
    /// The runtime allocation was rebuilt and reprepared its sources.
    Reallocation,
}

/// One configuration domain a commit changed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationDomain {
    /// The effective system instructions, project guidance or Agent profile.
    Instructions,
    /// The model-facing Tool catalog.
    Tools,
    /// The Skill catalog.
    Skills,
    /// The effective model of future work.
    Model,
    /// The execution policy future Attempts freeze: approval mode, model
    /// timeout, Tool deadline and MCP startup policies.
    ExecutionPolicy,
}

/// Most names one change category records; the total stays exact.
pub const CONFIGURATION_CHANGE_NAME_LIMIT: usize = 16;

/// Most JSON-encoded bytes the recorded names of one category occupy. Names
/// are not bounded at their source (an MCP server chooses its own), so the
/// six categories of one change stay within the 16 KiB transcript and Trace
/// headers by this budget, never by an assumed name length.
pub const CONFIGURATION_CHANGE_NAME_BYTES: usize = 1024;

/// A bounded, ordered list of names with its exact total.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct BoundedNames {
    /// Sorted names: at most [`CONFIGURATION_CHANGE_NAME_LIMIT`] of them,
    /// within [`CONFIGURATION_CHANGE_NAME_BYTES`] encoded bytes.
    pub names: Vec<String>,
    /// The exact number of names in this category.
    pub total: u32,
}

impl BoundedNames {
    fn from_names(mut names: Vec<String>) -> Self {
        names.sort();
        let total = u32::try_from(names.len()).unwrap_or(u32::MAX);
        // A name that alone would overflow the budget is counted, not
        // recorded; the shorter names after it still are.
        let mut bytes = 0;
        names.retain(|name| {
            let encoded = serde_json::to_string(name).map_or(usize::MAX, |encoded| encoded.len());
            let fits = bytes + encoded < CONFIGURATION_CHANGE_NAME_BYTES;
            if fits {
                bytes += encoded + 1;
            }
            fits
        });
        names.truncate(CONFIGURATION_CHANGE_NAME_LIMIT);
        Self { names, total }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Named-item differences of one catalog. A same-name item whose definition
/// changed is `changed`, never a removal plus an addition.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct NamedChanges {
    pub added: BoundedNames,
    pub removed: BoundedNames,
    pub changed: BoundedNames,
}

impl NamedChanges {
    #[must_use]
    pub fn between<T: PartialEq>(
        before: &BTreeMap<String, T>,
        after: &BTreeMap<String, T>,
    ) -> Self {
        let added = after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .cloned()
            .collect();
        let removed = before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .cloned()
            .collect();
        let changed = after
            .iter()
            .filter(|(name, value)| before.get(*name).is_some_and(|old| old != *value))
            .map(|(name, _)| name.clone())
            .collect();
        Self {
            added: BoundedNames::from_names(added),
            removed: BoundedNames::from_names(removed),
            changed: BoundedNames::from_names(changed),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The typed semantic summary of one actual configuration commit.
///
/// Recorded once per commit that changed something; never for preparation,
/// a file notification or a commit with no effective semantic change. It
/// names what changed, not the bodies: the exact prompt and Tool schemas a
/// request used remain in that request's own immutable snapshot.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ConfigurationChange {
    pub reason: ConfigurationChangeReason,
    /// The Session binding revision this commit replaced.
    pub previous_binding_revision: u64,
    /// The Session binding revision this commit installed.
    pub binding_revision: u64,
    /// The changed domains, in their stable order.
    pub domains: Vec<ConfigurationDomain>,
    pub tools: NamedChanges,
    pub skills: NamedChanges,
    /// The byte length of the effective system instructions this commit
    /// installed. The fact retains them exactly, readable in bounded ranges
    /// even when no request ever used them.
    pub instructions_bytes: u64,
    /// The byte length of the retained JSON catalog of every Tool and Skill
    /// this commit added, changed or removed, with their before and after
    /// states.
    pub catalog_bytes: u64,
}
