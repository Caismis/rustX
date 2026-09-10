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

/// The frozen effective native Agent Extension composition of the Agent
/// runtime this snapshot projects (Issue #256).
///
/// This is a **projection of the attached runtime's own composition**, never
/// a reread of authoring configuration. It is closed and typed — one named
/// member per native extension — so the vocabulary grows only when a native
/// extension is deliberately added to it. There is no map, no
/// `serde_json::Value`, no plugin descriptor, and no dynamic registry view.
///
/// Two different nullabilities meet on this path and must not be confused:
///
/// - the snapshot's `effective_extensions` is `None` when there is no
///   authoritative Agent composition to project at all — historical-only
///   durable inspection. It is never filled from disk, built-in defaults,
///   or the latest runtime configuration;
/// - `agent_status` inside it is `None` when the extension is **not part of
///   this Agent's composition**. That is a different fact from "composed,
///   with both contributors switched off", which is
///   `Some(EffectiveAgentStatusExtension { time: disabled, background:
///   disabled })`.
///
/// It is also independent of whether any Agent Status was actually composed
/// for a step: a runtime with the extension enabled and no eligible status
/// contribution yet still reports `Some(..)`. Observations describe steps;
/// this describes the composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveNativeAgentExtensions {
    /// The composed Agent Status extension, or `None` when this Agent
    /// composes no Agent Status at all.
    pub agent_status: Option<EffectiveAgentStatusExtension>,
}

/// The frozen contributor configuration of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveAgentStatusExtension {
    pub time: EffectiveTimeStatus,
    pub background: EffectiveBackgroundStatus,
}

/// The frozen Time contributor of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveTimeStatus {
    pub enabled: bool,
    /// The IANA timezone frozen for this composition, or `None` when none was
    /// configured. `None` is "no explicit timezone", not "UTC".
    pub timezone: Option<chrono_tz::Tz>,
}

/// The frozen Background contributor of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveBackgroundStatus {
    pub enabled: bool,
}

impl EffectiveNativeAgentExtensions {
    /// Projects one frozen composition into the Runtime Client vocabulary.
    ///
    /// The input is always the composition an Agent runtime is already
    /// executing against — for a root, the value frozen at
    /// `LocalConversationCore::compose`; for a child, the value its invoking
    /// generation froze into `ResolvedSubagentSpec::extensions`. This
    /// function performs a total, information-preserving translation and
    /// makes no decision of its own: it does not resolve, default, widen,
    /// narrow, or validate extension scope.
    #[must_use]
    pub fn project(frozen: &crate::extensions::NativeAgentExtensions) -> Self {
        Self {
            agent_status: frozen
                .agent_status()
                .map(|config| EffectiveAgentStatusExtension {
                    time: EffectiveTimeStatus {
                        enabled: config.time.enabled,
                        timezone: config.time.timezone,
                    },
                    background: EffectiveBackgroundStatus {
                        enabled: config.background.enabled,
                    },
                }),
        }
    }
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
    /// The application boundary of the effective native Agent Extension
    /// composition (Issue #256).
    ///
    /// A root Agent's composition is launch-frozen, so this is
    /// [`SettingsBoundary::LaunchCapture`]: only a restart can produce a
    /// different one, and a resource reload never does. A Subagent child's
    /// composition is the frozen execution profile its invoking generation
    /// resolved, so a `frozen_child` snapshot reports
    /// [`SettingsBoundary::FrozenAdmission`] instead — the same vocabulary
    /// the frozen child model already uses.
    pub extensions: SettingsBoundary,
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
            extensions: SettingsBoundary::LaunchCapture,
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
    /// Approval/resource availability, launch provenance, and the effective
    /// native Agent Extension composition are unavailable: this evidence
    /// class reports them absent rather than reconstructing them from the
    /// configuration that happens to be on disk today.
    HistoricalPartial,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_client::types::{RuntimeClientRequest, RuntimeClientResult};

    #[test]
    fn cfg238_protocol_fixture_and_finite_write_vocabulary() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/settings-v26.json"
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

    /// Issue #256 regression 10 (Rust half): the shared protocol fixture is
    /// the one wire definition of the effective-extension projection, and
    /// the Rust types encode and decode it byte-exactly. The TypeScript half
    /// reads the same file (`tui/test/settings.test.ts`).
    ///
    /// The fixture carries all three semantically distinct states on
    /// purpose: composed with an explicit timezone, composed with both
    /// contributors off and no timezone, and not composed at all.
    #[test]
    fn ext256_effective_extension_protocol_fixture_round_trips_exactly() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/settings-v26.json"
        ))
        .unwrap();
        let extensions = &fixture["effective_extensions"];
        for state in ["composed", "contributors_disabled", "not_composed"] {
            let wire = extensions[state].clone();
            let decoded: EffectiveNativeAgentExtensions =
                serde_json::from_value(wire.clone()).expect(state);
            assert_eq!(serde_json::to_value(&decoded).unwrap(), wire, "{state}");
        }

        // The three states are semantically distinct values, not spellings
        // of one another: "not composed" is never "composed and idle".
        let composed: EffectiveNativeAgentExtensions =
            serde_json::from_value(extensions["composed"].clone()).unwrap();
        let disabled: EffectiveNativeAgentExtensions =
            serde_json::from_value(extensions["contributors_disabled"].clone()).unwrap();
        let absent: EffectiveNativeAgentExtensions =
            serde_json::from_value(extensions["not_composed"].clone()).unwrap();
        assert_ne!(composed, disabled);
        assert_ne!(disabled, absent);
        assert!(absent.agent_status.is_none());
        assert!(disabled.agent_status.is_some());

        // The projection is a total translation of the frozen composition,
        // and the frozen composition is the only input it has.
        let frozen = serde_json::from_value::<crate::extensions::NativeAgentExtensionsDocument>(
            serde_json::json!({"agentStatus": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": true}
            }}),
        )
        .unwrap()
        .resolve();
        assert_eq!(EffectiveNativeAgentExtensions::project(&frozen), composed);
        assert_eq!(
            EffectiveNativeAgentExtensions::project(
                &crate::extensions::NativeAgentExtensions::none()
            ),
            absent
        );

        // Root and frozen-child lifetimes use the existing vocabulary.
        assert_eq!(
            serde_json::to_value(SettingsLifetimes::default()).unwrap()["extensions"],
            fixture["lifetimes"]["extensions"]
        );
        let child = SettingsLifetimes {
            model: SettingsBoundary::FrozenAdmission,
            extensions: SettingsBoundary::FrozenAdmission,
            ..SettingsLifetimes::default()
        };
        assert_eq!(
            serde_json::to_value(&child).unwrap(),
            fixture["child_lifetimes"]
        );

        // The closed record rejects an unknown extension name and an
        // unknown contributor field: the vocabulary grows only in Rust.
        for invalid in [
            serde_json::json!({"agent_status": null, "todo": {"enabled": true}}),
            serde_json::json!({"agent_status": {
                "time": {"enabled": true, "timezone": null, "future": true},
                "background": {"enabled": true}
            }}),
        ] {
            assert!(
                serde_json::from_value::<EffectiveNativeAgentExtensions>(invalid.clone()).is_err(),
                "accepted {invalid}"
            );
        }
    }
}
