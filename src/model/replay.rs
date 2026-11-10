//! Identity of model-owned reasoning, independent of profiles and request parameters.

use serde::{Deserialize, Serialize};

/// Only the originating provider binding, protocol and wire model can replay
/// opaque reasoning. Other invocations use the readable history instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayOrigin {
    /// Fingerprint of the configured provider identity and endpoint; no credentials.
    pub binding: String,
    pub protocol: super::ModelProtocol,
    pub model: String,
}

pub(crate) fn binding_id(provider: &str, endpoint: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(provider.as_bytes());
    hash.update([0]);
    hash.update(endpoint.as_bytes());
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::binding_id;
    #[test]
    fn binding_changes_with_provider_or_endpoint_without_exposing_either() {
        let value = binding_id("account-a", "https://provider.example/v1");
        assert_eq!(value.len(), 64);
        assert_eq!(
            value,
            binding_id("account-a", "https://provider.example/v1")
        );
        assert_ne!(
            value,
            binding_id("account-b", "https://provider.example/v1")
        );
        assert_ne!(value, binding_id("account-a", "https://other.example/v1"));
        assert!(!value.contains("account-a"));
    }
}
