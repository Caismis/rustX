//! Bounded settings vocabulary. Live values remain in their canonical snapshot sections.
use crate::runtime::ApprovalMode;
use serde::{Deserialize, Serialize};

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
/// - the snapshot's `effective_plugins` is `None` when there is no
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
#[derive(schemars::JsonSchema)]
pub struct EffectivePlugins {
    /// The composed Agent Status extension, or `None` when this Agent
    /// composes no Agent Status at all.
    pub agent_status: Option<EffectiveAgentStatusExtension>,
    /// The composed Todo extension, or `None` when this Agent composes no
    /// Todo at all (Issue #259).
    ///
    /// This is the authoritative answer to "does this runtime have a current
    /// task list, a `todo` Tool, and a Todo panel?" — and it is the only
    /// authoritative answer. A client must not infer it from the presence of
    /// `todo` results in the transcript, which are historical facts of the
    /// conversation rather than facts about the runtime attached to it.
    pub todo: Option<EffectiveTodoExtension>,
    /// Root Goal capability, frozen for this launch.
    pub goal: Option<crate::extensions::GoalExtensionConfig>,
}

/// The frozen Todo extension of a composition that includes it.
///
/// It carries no field: Todo has no contributor configuration, so being
/// composed is the whole fact. It is a struct rather than a bare `bool`
/// because the vocabulary is closed and typed, and because a later
/// contributor would be added here rather than by changing the shape of the
/// value clients already parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct EffectiveTodoExtension {}

/// The frozen contributor configuration of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct EffectiveAgentStatusExtension {
    pub time: EffectiveTimeStatus,
    pub background: EffectiveBackgroundStatus,
}

/// The frozen Time contributor of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct EffectiveTimeStatus {
    pub enabled: bool,
    /// The IANA timezone frozen for this composition, or `None` when none was
    /// configured. `None` is "no explicit timezone", not "UTC".
    #[schemars(with = "Option<String>")]
    pub timezone: Option<chrono_tz::Tz>,
}

/// The frozen Background contributor of a composed Agent Status extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct EffectiveBackgroundStatus {
    pub enabled: bool,
}

impl EffectivePlugins {
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
            todo: frozen.todo().map(|_| EffectiveTodoExtension {}),
            goal: frozen.goal().copied(),
        }
    }
}

/// Facts frozen together with the attempt model under native admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct AdmittedSettings {
    pub resource_revision: crate::runtime::identity::RuntimeResourceRevision,
    pub approval_mode: ApprovalMode,
}

/// Which native evidence is available for the canonical settings sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
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
    use crate::runtime_client::types::RuntimeClientRequest;

    #[test]
    fn cfg332_old_session_settings_mutations_are_rejected() {
        for kind in [
            "settings_defaults",
            "settings_save_default",
            "approval_mode_set",
            "resources_reload",
        ] {
            assert!(
                serde_json::from_value::<RuntimeClientRequest>(serde_json::json!({"type": kind}),)
                    .is_err()
            );
        }
    }

    /// Issue #256 regression 10 (Rust half): the shared protocol fixture is
    /// the one wire definition of the effective-extension projection, and
    /// the Rust types encode and decode it byte-exactly. The TypeScript half
    /// reads the same file (`tui/test/settings.test.ts`).
    ///
    /// The fixture carries every semantically distinct state on purpose:
    /// composed with an explicit timezone, composed with both contributors
    /// off and no timezone, not composed at all, and — since Issue #259 —
    /// Todo composed while Agent Status is not, which is the combination that
    /// proves the two extensions project independently.
    #[test]
    fn ext256_effective_extension_protocol_fixture_round_trips_exactly() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/plugins.json"
        ))
        .unwrap();
        let extensions = &fixture;
        for state in [
            "composed",
            "contributors_disabled",
            "not_composed",
            "todo_only",
        ] {
            let wire = extensions[state].clone();
            let decoded: EffectivePlugins = serde_json::from_value(wire.clone()).expect(state);
            assert_eq!(serde_json::to_value(&decoded).unwrap(), wire, "{state}");
        }

        // The three states are semantically distinct values, not spellings
        // of one another: "not composed" is never "composed and idle".
        let composed: EffectivePlugins =
            serde_json::from_value(extensions["composed"].clone()).unwrap();
        let disabled: EffectivePlugins =
            serde_json::from_value(extensions["contributors_disabled"].clone()).unwrap();
        let absent: EffectivePlugins =
            serde_json::from_value(extensions["not_composed"].clone()).unwrap();
        let todo_only: EffectivePlugins =
            serde_json::from_value(extensions["todo_only"].clone()).unwrap();
        assert_ne!(composed, disabled);
        assert_ne!(disabled, absent);
        assert_ne!(absent, todo_only);
        assert!(absent.agent_status.is_none());
        assert!(disabled.agent_status.is_some());

        // Issue #259: the two members are independent axes, not one switch.
        assert!(todo_only.agent_status.is_none() && todo_only.todo.is_some());
        assert!(absent.todo.is_none());
        assert!(composed.todo.is_some());
        assert_eq!(
            EffectivePlugins::project(&crate::extensions::NativeAgentExtensions::with_todo()),
            todo_only
        );

        // The projection is a total translation of the frozen composition,
        // and the frozen composition is the only input it has.
        let frozen = serde_json::from_value::<crate::extensions::NativeAgentExtensionsDocument>(
            serde_json::json!({
                "agent_status": {
                    "enabled": true,
                    "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                    "background": {"enabled": true}
                },
                "todo": {"enabled": true}
            }),
        )
        .unwrap()
        .resolve();
        assert_eq!(EffectivePlugins::project(&frozen), composed);
        assert_eq!(
            EffectivePlugins::project(&crate::extensions::NativeAgentExtensions::none()),
            absent
        );

        // The closed record rejects an unknown extension name and an
        // unknown contributor field: the vocabulary grows only in Rust.
        for invalid in [
            serde_json::json!({"agent_status": null, "todo": null, "futureGoal": {}}),
            // The Todo member carries no contributor at all: the closed
            // record refuses an invented one rather than ignoring it.
            serde_json::json!({"agent_status": null, "todo": {"enabled": true}}),
            serde_json::json!({"agent_status": {
                "time": {"enabled": true, "timezone": null, "future": true},
                "background": {"enabled": true}
            }, "todo": null}),
        ] {
            assert!(
                serde_json::from_value::<EffectivePlugins>(invalid.clone()).is_err(),
                "accepted {invalid}"
            );
        }
    }
}
