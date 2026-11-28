//! Bounded Client-granted environment values for one Workspace HTTP MCP.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Maximum number of names in one source request.
pub const MAX_NAMES: usize = 64;
/// Maximum combined length of values returned for one source.
pub const MAX_VALUE_BYTES: usize = 32 * 1024;

/// The exact source, destination origin and references in its configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub service: String,
    pub origin: String,
    pub names: BTreeSet<String>,
}

/// The Client's answer; value bytes are never formatted in diagnostics.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub values: BTreeMap<String, String>,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpEnvironmentReply")
            .field("names", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Only ordinary environment identifiers may be requested or granted.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    name.len() <= 256
        && name
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

impl Request {
    #[must_use]
    pub fn valid(&self) -> bool {
        !self.service.is_empty()
            && self.service.len() <= 256
            && !self.origin.is_empty()
            && self.origin.len() <= 2048
            && !self.names.is_empty()
            && self.names.len() <= MAX_NAMES
            && self.names.iter().all(|name| valid_name(name))
    }
}
