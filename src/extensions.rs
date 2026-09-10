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

/// The **presence-aware** closed selection an invocation override expresses.
///
/// This is deliberately *not* [`NativeAgentExtensionsDocument`]. The authored
/// document supplies launch/role **defaults** — its `agentStatus` member has
/// `enabled: true` — which is exactly right for "an unconfigured role composes
/// Agent Status" and exactly wrong for an override, where the whole point is
/// that a present `extensions` dimension **replaces** the role's composition:
///
/// ```text
/// role frontmatter   extensions: {}   ->  Agent Status composed (role default)
/// invocation override "extensions": {} ->  no extension composed at all
/// ```
///
/// Every member is therefore an explicit `Option`: absent means "this
/// extension is not part of the requested composition", never "use a default".
/// Presence is the only way to compose an extension, and a present member
/// still carries its own complete authored configuration.
///
/// The record stays closed exactly like the authored document: an unknown
/// extension name is rejected by `deny_unknown_fields`, so a misspelled or
/// not-yet-implemented extension fails deterministically instead of being
/// silently ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativeAgentExtensionSelection {
    /// The requested Agent Status extension, when this selection composes it.
    ///
    /// `null` is not accepted: the field is either absent (not composed) or a
    /// complete authored configuration. Collapsing an explicit `null` into
    /// "absent" would give one wire spelling two meanings.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    // The published schema must say what the runtime accepts: an absent key
    // or a complete document, never `null`. The default `Option` rendering
    // would advertise a spelling the deserializer refuses.
    #[schemars(with = "AgentStatusExtensionDocument")]
    pub agent_status: Option<AgentStatusExtensionDocument>,
}

/// Deserializes a field that may be **absent**, but never explicitly `null`.
///
/// serde reaches this function only when the key is present, so delegating to
/// the inner type turns `"agentStatus": null` into that type's ordinary
/// "invalid type: null" rejection while an omitted key still takes the
/// container's `default`.
///
/// # Errors
///
/// Returns the inner type's deserialization error, including for `null`.
pub(crate) fn present_and_not_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl NativeAgentExtensionSelection {
    /// Freezes this requested selection into the composition a child runs
    /// against.
    ///
    /// An absent member composes nothing. A present member composes exactly
    /// what it authored, including `enabled: false`, which is "named and
    /// switched off" and therefore still composes nothing — the same rule
    /// [`NativeAgentExtensionsDocument::resolve`] applies.
    #[must_use]
    pub fn resolve(&self) -> NativeAgentExtensions {
        NativeAgentExtensions {
            agent_status: self
                .agent_status
                .as_ref()
                .filter(|status| status.enabled)
                .map(|status| AgentStatusConfig {
                    time: status.time.clone(),
                    background: status.background.clone(),
                }),
        }
    }

    /// The selection that reproduces an already-frozen composition exactly.
    ///
    /// This is what makes "no override" and "an explicit override restating
    /// the defaults" the same effective profile rather than two shapes that
    /// merely look alike.
    #[must_use]
    pub fn of(frozen: &NativeAgentExtensions) -> Self {
        Self {
            agent_status: frozen
                .agent_status()
                .map(|config| AgentStatusExtensionDocument {
                    enabled: true,
                    time: config.time.clone(),
                    background: config.background.clone(),
                }),
        }
    }
}

/// The one-shot child scope support of the closed extension vocabulary.
///
/// Extension **authorization** and child-**scope support** are independent
/// checks: an extension a caller is fully entitled to compose may still be
/// meaningless — or actively wrong — inside a one-shot child, and must then
/// fail deterministically before the child is staged rather than be silently
/// dropped.
///
/// The match below is exhaustive over the closed composition on purpose: it
/// is the seam a future extension author has to visit, and it is a compile
/// error to add a member without deciding this question. Today's whole
/// vocabulary is one member, Agent Status, and it is supported: a child is an
/// ordinary `ConversationRuntime` whose Agent Loop composes the same status
/// engine the root does.
#[must_use]
pub fn unsupported_child_scope(
    composition: &NativeAgentExtensions,
) -> Option<UnsupportedChildScope> {
    // The exhaustive match is the guarantee: adding a member to the closed
    // composition without deciding its child scope does not compile.
    match composition {
        // Agent Status contributes one bounded structured fact that Context
        // Assembly admits at request time. A one-shot child owns Context
        // Assembly exactly like a root Agent and needs no multi-round or
        // resumable lifecycle for it, so it is supported in child scope.
        NativeAgentExtensions {
            agent_status: None | Some(AgentStatusConfig { .. }),
        } => None,
    }
}

/// The canonical authored names of the extensions one composition composes.
///
/// The list is derived from the closed composition rather than written out at
/// each call site, so a diagnostic can never name an extension the composition
/// does not actually hold — and adding a member updates every caller at once.
#[must_use]
pub fn composed_extension_names(composition: &NativeAgentExtensions) -> Vec<&'static str> {
    let NativeAgentExtensions { agent_status } = composition;
    let mut names = Vec::new();
    if agent_status.is_some() {
        names.push("agentStatus");
    }
    names
}

/// One recognized extension that a one-shot child cannot own.
///
/// This is a scope fact, never an authority fact: the caller may have been
/// fully entitled to compose the extension, and the request still fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedChildScope {
    /// The canonical authored extension name.
    pub extension: &'static str,
    /// The bounded reason the one-shot child scope cannot own it.
    pub reason: &'static str,
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

    /// Whether this composition **authorizes** `requested` as a delegated
    /// child composition.
    ///
    /// This is the typed extension half of the dynamic delegation ceiling. It
    /// is deliberately not "the requested extension has the same name", and
    /// deliberately not "any configuration of an authorized extension": naming
    /// an extension is not permission to configure it arbitrarily, because the
    /// configuration is exactly what decides the behavior.
    ///
    /// The rule is concrete per supported extension rather than a speculative
    /// permission framework:
    ///
    /// ```text
    /// requested extension absent        -> always authorized (narrowing)
    /// requested extension present       -> this composition must compose it too, and
    ///   time.enabled       requested true  -> authority time.enabled must be true
    ///   background.enabled requested true  -> authority background.enabled must be true
    ///   time.timezone      requested Some  -> authority timezone must be exactly that zone
    /// ```
    ///
    /// Every contributor may therefore be switched **off** by a delegated
    /// child and never switched on, and a timezone is an exact match rather
    /// than a free parameter: a caller whose own composition renders UTC
    /// cannot make a child render another region's local time.
    #[must_use]
    pub fn authorizes(&self, requested: &Self) -> bool {
        let NativeAgentExtensions { agent_status } = requested;
        match agent_status {
            None => true,
            Some(requested) => self.agent_status.as_ref().is_some_and(|authority| {
                (!requested.time.enabled || authority.time.enabled)
                    && (!requested.background.enabled || authority.background.enabled)
                    && (requested.time.timezone.is_none()
                        || requested.time.timezone == authority.time.timezone)
            }),
        }
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
