//! The provider-neutral invocation configuration carried by one model request.
//!
//! Immutable for the whole attempt: no credentials, no adapter object, and
//! never part of canonical conversation facts. Catalog binding owns how the
//! `replay_binding` fingerprint is produced; this module only stores it.

use serde::{Deserialize, Serialize};

use crate::model::capabilities::{ModelCapabilities, ModelCompat};
use crate::model::request_params::RequestParams;
use crate::model::types::ModelProtocol;

/// The provider-neutral invocation configuration one model request carries
/// to its adapter.
///
/// This is the only provider-configuration channel of a [`ModelRequest`](crate::model::types::ModelRequest): it is immutable for the whole
/// attempt, carries no credential and no adapter object, and never leaks
/// into canonical conversation facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvocationConfig {
    /// Frozen provider/endpoint fingerprint, when the invocation has a catalog
    /// binding. Unattributed invocations cannot authorize native reasoning replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_binding: Option<String>,
    /// The provider-facing model identifier.
    pub model: String,
    /// The protocol the adapter must speak.
    pub protocol: ModelProtocol,
    /// The runtime-resolved effective maximum output tokens.
    ///
    /// The runtime resolves this before the adapter boundary: no adapter
    /// invents a generation limit, and providers that require an explicit
    /// maximum (Anthropic) are always satisfiable.
    pub max_output_tokens: u32,
    /// The effective opaque provider request parameters.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The effective capabilities of this invocation.
    pub capabilities: ModelCapabilities,
    /// The bounded structural translation metadata.
    #[serde(default)]
    pub compat: ModelCompat,
}

impl ModelInvocationConfig {
    /// The exact owner of reasoning produced by this frozen invocation.
    #[must_use]
    pub fn replay_origin(&self) -> Option<super::replay::ReplayOrigin> {
        self.replay_binding
            .as_ref()
            .map(|binding| super::replay::ReplayOrigin {
                binding: binding.clone(),
                protocol: self.protocol,
                model: self.model.clone(),
            })
    }
}
