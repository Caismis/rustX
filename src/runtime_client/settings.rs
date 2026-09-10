//! Bounded settings vocabulary. Live values remain in their canonical snapshot sections.
use super::types::RuntimeClientError;
use crate::model::catalog::{ModelRef, ReasoningProfileId};
use crate::runtime::ApprovalMode;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

/// Only the two primary selection fields; never request parameters or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDefault {
    pub model: ModelRef,
    pub reasoning_profile: Option<ReasoningProfileId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultScope {
    User,
}

/// The native setting to capture at the save operation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultTarget {
    ModelSelection,
    ApprovalMode,
}

/// Already-captured finite mutation for the disk writer and published result.
/// Clients request a `DefaultTarget`; they never supply this as a save input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefaultValue {
    ModelSelection { selection: ModelDefault },
    ApprovalMode { mode: ApprovalMode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultDocument {
    pub scope: DefaultScope,
    pub document: String,
    /// SHA-256 of exact bytes; the missing document has a distinct revision.
    pub revision: String,
    pub model: Option<ModelDefault>,
    pub approval_mode: Option<ApprovalMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsBoundary {
    LaunchCapture,
    NextAdmission,
    SafeBoundary,
    ResourcePublication,
    FrozenAdmission,
    ClientLocal,
    NextLaunch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDefaultResult {
    pub scope: DefaultScope,
    pub document: String,
    pub revision: String,
    pub changed: DefaultValue,
    pub live_unchanged: bool,
    pub applies_at: SettingsBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettingOrigin {
    Builtin,
    User { document: String },
    Project { document: String },
    Cli,
}

/// Captured resolver facts, explicitly NOT a read of today's disk defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSettings {
    pub model: ModelDefault,
    pub model_origin: SettingOrigin,
    pub reasoning_origin: SettingOrigin,
    pub approval_mode: ApprovalMode,
    pub approval_origin: SettingOrigin,
    pub runtime_root_origin: SettingOrigin,
    pub tool_selection_origin: SettingOrigin,
}

/// Metadata names canonical sections rather than maintaining another copy of live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsLifetimes {
    pub launch: SettingsBoundary,
    pub model: SettingsBoundary,
    pub approval: SettingsBoundary,
    pub resources: SettingsBoundary,
    pub attempt: SettingsBoundary,
    pub presentation: SettingsBoundary,
    pub saved_defaults: SettingsBoundary,
}
impl Default for SettingsLifetimes {
    fn default() -> Self {
        Self {
            launch: SettingsBoundary::LaunchCapture,
            model: SettingsBoundary::NextAdmission,
            approval: SettingsBoundary::SafeBoundary,
            resources: SettingsBoundary::ResourcePublication,
            attempt: SettingsBoundary::FrozenAdmission,
            presentation: SettingsBoundary::ClientLocal,
            saved_defaults: SettingsBoundary::NextLaunch,
        }
    }
}

pub type SettingsFuture<T> = Pin<Box<dyn Future<Output = Result<T, RuntimeClientError>> + Send>>;

/// Implemented by the local configuration owner, never by the TUI or client host.
pub trait DefaultSettingsStore: Send + Sync {
    fn read(&self, scope: DefaultScope) -> SettingsFuture<DefaultDocument>;
    fn save(
        &self,
        scope: DefaultScope,
        expected: String,
        value: DefaultValue,
    ) -> SettingsFuture<SaveDefaultResult>;
}

/// Facts frozen together with the attempt model under native admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedSettings {
    pub resource_revision: crate::runtime::identity::RuntimeResourceRevision,
    pub approval_mode: ApprovalMode,
}

/// Which native evidence is available for the canonical settings sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsEvidence {
    LiveSession,
    FrozenChild,
    /// No live Session model is available. Retained requests have their own evidence.
    /// Approval/resource availability and launch provenance are unavailable.
    HistoricalPartial,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_client::types::{RuntimeClientRequest, RuntimeClientResult};

    #[test]
    fn cfg238_protocol_fixture_and_finite_write_vocabulary() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/settings-v25.json"
        ))
        .unwrap();
        let request: RuntimeClientRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        let result: RuntimeClientResult =
            serde_json::from_value(fixture["result"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), fixture["request"]);
        assert_eq!(serde_json::to_value(result).unwrap(), fixture["result"]);
        assert!(request.is_mutating());
        assert!(request.requires_async());
        assert_eq!(
            serde_json::to_value(SettingsLifetimes::default()).unwrap(),
            fixture["lifetimes"]
        );
        let _: AdmittedSettings = serde_json::from_value(fixture["admitted"].clone()).unwrap();
        for (field, value) in [
            ("scope", serde_json::json!("project")),
            ("target", serde_json::json!("arbitrary.path")),
        ] {
            let mut invalid = fixture["request"].clone();
            invalid[field] = value;
            assert!(serde_json::from_value::<RuntimeClientRequest>(invalid).is_err());
        }
        let mut invalid = fixture["request"].clone();
        invalid["value"] = serde_json::json!({"api_key":"SECRET_SENTINEL"});
        assert!(serde_json::from_value::<RuntimeClientRequest>(invalid).is_err());
    }
}
