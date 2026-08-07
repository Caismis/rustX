//! Deterministic compiled runtime manifests.
//!
//! The Rust runtime never reads product/control-plane schemas directly; it
//! consumes the compiled [`RuntimeManifest`] boundary defined here.

pub mod manifest;

pub use manifest::{
    AgentManifest, AttemptLimitsManifest, CapabilitiesManifest, ContextManifest,
    MANIFEST_SCHEMA_VERSION, McpBinding, ModelManifest, RuntimeManifest, SkillBinding, ToolBinding,
};
