//! The closed, Rust-owned, launch-scoped **Native Agent Extension**
//! composition boundary (Issue #256).
//!
//! ```text
//! Agent core
//!   ConversationRuntime
//!   Agent Loop
//!   Tool Plane
//!   Context Assembly / Context Engine
//!   durability / cancellation / recovery
//!
//! Native Agent Extensions
//!   Agent Status        <- the first migrated extension
//! ```
//!
//! # What a Native Agent Extension is
//!
//! An extension is *optional Agent behavior or context augmentation* that
//! belongs to one concrete Agent/Conversation composition. It is not a
//! plugin, not a capability, and not an ordinary Tool: ordinary tool
//! selection stays the business of `defaultTools`/`--tools` and the
//! capability plane.
//!
//! The one core invariant:
//!
//! > An extension may contribute behavior only through an existing native
//! > owner/seam. Extension composition never becomes a second Agent Loop,
//! > Tool Plane, Context Engine, conversation owner, admission path, or
//! > durability authority.
//!
//! Agent Status obeys it literally: it produces one bounded structured
//! contribution that **Context Assembly** admits as an ordinary Runtime
//! context fact. Context Assembly remains the request-time owner of
//! admission, ordering, provenance, projection, and token semantics.
//!
//! # Why this is deliberately not a plugin runtime
//!
//! The composition is a *closed struct with one named member per extension*,
//! not `Vec<Box<dyn Extension>>`. There is no lifecycle trait, no dynamic
//! registration, no event-hook registry, no arbitrary model-request
//! mutation, and no third-party loading. Adding an extension means adding a
//! typed member here and wiring it through its real owning subsystem — Tool
//! Plane for tools, Context Assembly for context, `ConversationRuntime` for
//! runtime coordination, Runtime Client for projection. That cost is the
//! point: it keeps every extension's authority reviewable.
//!
//! # Launch-scoped lifetime
//!
//! > A running `ConversationRuntime` executes against the native extension
//! > composition frozen for that launch.
//!
//! [`NativeAgentExtensionsDocument`] is authored configuration;
//! [`NativeAgentExtensions`] is the frozen decision. The document is read
//! once, at composition, through the ordinary launch resolver. Resource
//! reload republishes a `RuntimeResourceSnapshot` and deliberately never
//! reaches this value, so a configuration edit cannot install or uninstall
//! an extension inside an already-composed runtime. Restart/resume is a new
//! launch: it resolves the current document through the same resolver and
//! rewrites no canonical Session history.
//!
//! # Root and child are independently authored
//!
//! > Root Agent extensions and named-Subagent extensions are independently
//! > authored compositions.
//!
//! The root composition comes from `CurrentRuntimeConfig::extensions`; a
//! named role's comes from its own canonical frontmatter. A child never
//! inherits the root's set: the resolver that freezes a child reads only the
//! definition, so the root value is not even in scope there. The frozen
//! child composition rides inside `ResolvedSubagentSpec`, and the child
//! process materializes exactly that value without rereading `rustx.jsonc`,
//! project configuration, host configuration, role files, or any later
//! resource generation.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::{
    AgentStatusClock, AgentStatusConfig, AgentStatusEngine, BackgroundStatusConfig,
    TimeStatusConfig,
};

/// The closed authored composition surface of native Agent Extensions.
///
/// Every member is a concrete named extension. The map-like spelling in
/// JSONC (`"extensions": { "agentStatus": { ... } }`) is a *closed* record,
/// not an open registry: an unknown extension name is rejected by the
/// surrounding strict serde boundary exactly like any other unknown field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativeAgentExtensionsDocument {
    /// The Agent Status extension: optional provider-independent runtime
    /// context for an already-established model step.
    pub agent_status: AgentStatusExtensionDocument,
}

/// The authored Agent Status extension.
///
/// `enabled` composes the extension in or out of the runtime as a whole;
/// `time` and `background` remain the two bounded status contributors, with
/// exactly the semantics they had before the migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct AgentStatusExtensionDocument {
    /// Whether this composition includes the Agent Status extension at all.
    ///
    /// With `false` the runtime composes no status engine: the Agent Loop
    /// emits no Agent Status and is otherwise a completely ordinary
    /// `ConversationRuntime`.
    pub enabled: bool,
    /// The Time contributor configuration.
    pub time: TimeStatusConfig,
    /// The Background contributor configuration.
    pub background: BackgroundStatusConfig,
}

impl Default for AgentStatusExtensionDocument {
    fn default() -> Self {
        Self {
            enabled: true,
            time: TimeStatusConfig::default(),
            background: BackgroundStatusConfig::default(),
        }
    }
}

impl NativeAgentExtensionsDocument {
    /// Freezes this authored document into the composition one launch runs
    /// against.
    ///
    /// This is the **only** transition from mutable configuration to
    /// executed composition. Root composition calls it once, at
    /// `LocalConversationCore::compose`; named-role loading calls it once,
    /// while building the immutable `SubagentDefinition`.
    #[must_use]
    pub fn resolve(&self) -> NativeAgentExtensions {
        NativeAgentExtensions {
            agent_status: self.agent_status.enabled.then(|| AgentStatusConfig {
                time: self.agent_status.time.clone(),
                background: self.agent_status.background.clone(),
            }),
        }
    }
}

/// The **frozen** native Agent Extension composition of one concrete
/// Agent/Conversation.
///
/// An absent member means the extension is not part of this composition —
/// not that it is present and idle. An empty composition is an ordinary
/// runtime with nothing added, never a second semantic runtime mode.
///
/// The type is serializable because a child's frozen composition crosses the
/// subagent process boundary inside `ResolvedSubagentSpec`. It is
/// deliberately not mutable after construction: there is no installer,
/// uninstaller, or runtime extension manager anywhere in the runtime.
///
/// There is deliberately no `Default`: "the composition an unconfigured
/// launch or role gets" is a decision of
/// [`NativeAgentExtensionsDocument::resolve`], and "no extension at all" is
/// [`NativeAgentExtensions::none`]. Conflating the two behind a derive is
/// exactly how an empty extension set would start meaning something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeAgentExtensions {
    /// The frozen Agent Status extension configuration, when this
    /// composition includes the extension.
    #[serde(default)]
    agent_status: Option<AgentStatusConfig>,
}

impl NativeAgentExtensions {
    /// The composition with no native Agent Extension at all.
    #[must_use]
    pub const fn none() -> Self {
        Self { agent_status: None }
    }

    /// The composition containing exactly the Agent Status extension with
    /// the supplied contributor configuration.
    #[must_use]
    pub const fn with_agent_status(agent_status: AgentStatusConfig) -> Self {
        Self {
            agent_status: Some(agent_status),
        }
    }

    /// The frozen Agent Status configuration, when the extension is composed.
    #[must_use]
    pub const fn agent_status(&self) -> Option<&AgentStatusConfig> {
        self.agent_status.as_ref()
    }

    /// Whether this composition contains no native Agent Extension.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.agent_status.is_none()
    }

    /// Recovers this composition from the runtime extension owners it
    /// materialized.
    ///
    /// This is deliberately **not** a second stored copy of the frozen
    /// decision. The Agent Status engine a composition materializes carries
    /// the exact frozen contributor configuration it was built from, and
    /// whether that engine exists at all *is* the composed/absent fact. So
    /// reading the owners back yields the same value by construction: there
    /// is no second field that could drift from the materialization, and no
    /// path here that could consult a configuration document instead.
    ///
    /// [`ConversationRuntime::native_extensions`](crate::runtime::ConversationRuntime::native_extensions)
    /// is the one caller: it is how the Runtime Client effective-extension
    /// projection reads the composition of the attached Agent runtime.
    #[must_use]
    pub fn from_materialized(status_engine: Option<&AgentStatusEngine>) -> Self {
        Self {
            agent_status: status_engine.map(|engine| engine.config().clone()),
        }
    }

    /// Materializes the attempt-owned Agent Status engine of this frozen
    /// composition.
    ///
    /// This is the one materialization seam between the extension set and
    /// the runtime: `None` composes a runtime with no status engine, and no
    /// other Agent Loop, Tool Plane, cancellation, or durability path
    /// consults the extension set at all.
    #[must_use]
    pub fn agent_status_engine(
        &self,
        clock: Arc<dyn AgentStatusClock>,
    ) -> Option<AgentStatusEngine> {
        self.agent_status
            .clone()
            .map(|config| AgentStatusEngine::new(config, clock))
    }

    /// The deterministic canonical framing of this composition, for the
    /// named-role semantic digest.
    ///
    /// Length-prefixed like every other digest field, and explicit about
    /// absence: a role that omits Agent Status is a different definition
    /// from one that disables its two contributors.
    #[must_use]
    pub fn digest_framing(&self) -> String {
        match &self.agent_status {
            None => "agent_status=\u{0}absent".to_owned(),
            Some(config) => format!(
                "agent_status=present:time={}:timezone={}:background={}",
                config.time.enabled,
                config
                    .time
                    .timezone
                    .map_or_else(|| "\u{0}none".to_owned(), |zone| zone.name().to_owned()),
                config.background.enabled,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(value: serde_json::Value) -> NativeAgentExtensionsDocument {
        serde_json::from_value(value).expect("the closed extension document parses")
    }

    #[test]
    fn ext256_an_omitted_document_composes_the_default_agent_status_extension() {
        let resolved = NativeAgentExtensionsDocument::default().resolve();
        let agent_status = resolved.agent_status().expect("Agent Status is composed");
        assert!(agent_status.time.enabled);
        assert!(agent_status.background.enabled);
        assert_eq!(agent_status.time.timezone, None);
        assert!(!resolved.is_empty());
    }

    #[test]
    fn ext256_disabling_agent_status_removes_the_extension_from_the_composition() {
        let resolved = document(serde_json::json!({"agentStatus": {"enabled": false}})).resolve();
        assert_eq!(resolved, NativeAgentExtensions::none());
        assert!(resolved.is_empty());
        assert!(resolved.agent_status().is_none());
        assert!(
            resolved
                .agent_status_engine(Arc::new(crate::context::SystemClock))
                .is_none(),
            "an absent extension materializes no engine"
        );
    }

    #[test]
    fn ext256_contributor_settings_survive_the_freeze_unchanged() {
        let resolved = document(serde_json::json!({
            "agentStatus": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": false}
            }
        }))
        .resolve();
        let agent_status = resolved.agent_status().expect("Agent Status is composed");
        assert!(agent_status.time.enabled);
        assert_eq!(
            agent_status
                .time
                .timezone
                .map(|zone| zone.name().to_owned()),
            Some("Asia/Shanghai".to_owned())
        );
        assert!(!agent_status.background.enabled);
    }

    /// Disabling the extension keeps its contributor settings out of the
    /// frozen composition entirely: absence is one fact, not "present but
    /// with both contributors off".
    #[test]
    fn ext256_disabled_agent_status_discards_contributor_configuration() {
        let resolved = document(serde_json::json!({
            "agentStatus": {"enabled": false, "time": {"timezone": "Asia/Shanghai"}}
        }))
        .resolve();
        assert!(resolved.agent_status().is_none());
    }

    #[test]
    fn ext256_unknown_extension_names_and_fields_are_rejected() {
        for value in [
            serde_json::json!({"todo": {"enabled": true}}),
            serde_json::json!({"agentStatus": {"future": true}}),
            serde_json::json!({"agentStatus": {"time": {"future": true}}}),
            serde_json::json!({"agentStatus": {"background": {"future": true}}}),
        ] {
            assert!(
                serde_json::from_value::<NativeAgentExtensionsDocument>(value.clone()).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn ext256_the_digest_framing_separates_every_semantic_composition() {
        let framings = [
            NativeAgentExtensions::none(),
            document(serde_json::json!({})).resolve(),
            document(serde_json::json!({"agentStatus": {"time": {"enabled": false}}})).resolve(),
            document(serde_json::json!({"agentStatus": {"background": {"enabled": false}}}))
                .resolve(),
            document(serde_json::json!({"agentStatus": {"time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
        ]
        .map(|composition| composition.digest_framing());
        let mut unique = framings.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            framings.len(),
            "every semantic composition frames differently"
        );
    }

    /// The Runtime Client effective-extension projection reads the frozen
    /// composition back off the owners it materialized. That round trip is
    /// the whole reason there is only one source of truth, so it is proven
    /// for every semantic composition rather than assumed.
    #[test]
    fn ext256_materialized_owners_recover_the_exact_frozen_composition() {
        for composition in [
            NativeAgentExtensions::none(),
            document(serde_json::json!({})).resolve(),
            document(serde_json::json!({"agentStatus": {"time": {"enabled": false}}})).resolve(),
            document(serde_json::json!({"agentStatus": {"background": {"enabled": false}}}))
                .resolve(),
            document(serde_json::json!({"agentStatus": {"time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
        ] {
            let engine = composition.agent_status_engine(Arc::new(crate::context::SystemClock));
            assert_eq!(
                NativeAgentExtensions::from_materialized(engine.as_ref()),
                composition,
                "the materialized owners recover exactly what was frozen"
            );
        }
    }

    /// The frozen composition is what crosses the subagent process
    /// boundary, so it must survive its real serialization contract exactly.
    #[test]
    fn ext256_the_frozen_composition_survives_its_wire_contract() {
        for composition in [
            NativeAgentExtensions::none(),
            document(serde_json::json!({"agentStatus": {"time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
        ] {
            let encoded = serde_json::to_vec(&composition).expect("encodes");
            let decoded: NativeAgentExtensions = serde_json::from_slice(&encoded).expect("decodes");
            assert_eq!(decoded, composition);
        }
    }
}
