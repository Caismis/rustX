//! Runtime-owned Tool availability and startup activation selection.
//!
//! Availability is the set of validated, currently eligible definitions the
//! runtime knows how to execute. Activation is the smaller immutable registry
//! exposed to the model and Agent Loop. Keeping both in this capability layer
//! prevents a discovery source or client projection from becoming a second
//! capability authority.
//!
//! # This layer owns the ordinary capability plane only
//!
//! Everything here — `defaultTools`, `--tools`, `--exclude-tools`,
//! `--no-tools`, `--no-builtin-tools` — addresses *ordinary execution
//! capabilities*. A Native Agent Extension may also contribute a model-facing
//! Tool, and that Tool belongs to the extension's composition, not to this
//! selection:
//!
//! ```text
//! active model Tool set = selected ordinary capabilities
//!                       + extension-provided Tool surfaces
//! ```
//!
//! [`select_tools`] therefore takes the two sets separately and never lets one
//! decide the other. Selection cannot remove an extension Tool (so `--no-tools`
//! plus an enabled Todo still exposes `todo`, and a truly Tool-free request
//! needs both zero ordinary Tools and no Tool-providing extension), and it
//! cannot add one either: an extension's Tool name is not an ordinary
//! identity, so naming it in an allowlist, an exclusion, or `defaultTools` is
//! rejected by [`ToolActivationPolicy::validate`] rather than silently
//! accepted. The classification is semantic: `--no-builtin-tools` removes
//! ordinary built-ins, not every Tool that happens to be implemented in Rust.
//!
//! Extension registrations are still *validated* exactly like ordinary ones,
//! and the composed active registry is built through the same
//! [`ToolRegistry`] identity rules — so a foreign Tool colliding with an
//! extension Tool's name fails loudly instead of shadowing it.

use std::collections::BTreeSet;

use crate::tools::executor::{ToolRegistration, ToolRegistry};
use crate::tools::types::{ToolDefinition, ToolOrigin};

/// Startup activation controls supplied by current runtime/project settings
/// and CLI options. They are never Session-persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolActivationPolicy {
    /// Complete root profile intent, independent of admitted catalogs.
    pub profile: crate::local_runtime::config::AgentProfileDocument,
    pub admitted_agents: BTreeSet<crate::runtime::subagent::SubagentName>,
    pub admitted_workflows: BTreeSet<crate::runtime::workflow::WorkflowId>,
    pub project_files: Vec<crate::runtime::resources::ProjectContextFile>,
    /// Remove all built-ins from default selection, including generated Tools.
    pub no_builtin_tools: bool,
    /// Expose and authorize zero ordinary main-model Tools.
    pub no_tools: bool,
    /// Exact model-facing allowlist across applicable origins.
    pub tools: Option<Vec<String>>,
    /// Final subtraction from selection, resolved against applicable identities.
    pub exclude_tools: Vec<String>,
}

impl Default for ToolActivationPolicy {
    fn default() -> Self {
        Self {
            profile: crate::local_runtime::config::builtin_root_profile(),
            admitted_agents: BTreeSet::new(),
            admitted_workflows: BTreeSet::new(),
            project_files: Vec::new(),
            no_builtin_tools: false,
            no_tools: false,
            tools: None,
            exclude_tools: Vec::new(),
        }
    }
}

/// The extension that owns `name` as a Tool surface, when one does
/// (Issue #259).
///
/// This is the ordinary capability plane's view of the extension plane: it
/// knows only that certain model-facing names are *not its to decide*, so it
/// can refuse them with a diagnostic that says where the name actually lives
/// instead of the misleading "unknown or ineligible".
///
/// The answer is derived from the closed extension vocabulary's own Tool
/// composition, never from a second hand-written list, so it cannot drift from
/// what an enabled extension actually registers.
#[must_use]
pub fn extension_provided_tool(name: &str) -> Option<&'static str> {
    if name == crate::tools::native::TODO_TOOL_NAME {
        Some(crate::extensions::TODO_EXTENSION)
    } else if crate::tools::native::GOAL_TOOL_NAMES.contains(&name) {
        Some("goal")
    } else {
        None
    }
}

/// Rejects an ordinary selection entry that names an extension-provided Tool.
fn reject_extension_tool(name: &str, label: &str) -> Result<(), String> {
    match extension_provided_tool(name) {
        None => Ok(()),
        Some(extension) => Err(format!(
            "Tool {label} entry {name:?} is provided by the {extension:?} Agent \
             Extension, not by ordinary Tool selection; compose it with \
             extensions.{extension}.enabled instead"
        )),
    }
}

impl ToolActivationPolicy {
    pub(crate) fn conflict(&self) -> Option<(&'static str, &'static str)> {
        if self.no_tools {
            for (present, flag) in [
                (self.tools.is_some(), "--tools"),
                (!self.exclude_tools.is_empty(), "--exclude-tools"),
                (self.no_builtin_tools, "--no-builtin-tools"),
            ] {
                if present {
                    return Some(("--no-tools", flag));
                }
            }
        }
        (self.tools.is_some() && self.no_builtin_tools).then_some(("--tools", "--no-builtin-tools"))
    }

    /// Validates selection intent independently of capability discovery.
    /// The same boundary is used by CLI parsing and resolved composition.
    ///
    /// # Errors
    ///
    /// Returns the first violation: a flag conflict, a malformed name list,
    /// or an entry naming a Tool an Agent Extension owns rather than the
    /// ordinary capability plane (Issue #259).
    pub fn validate(&self) -> Result<(), String> {
        self.profile.tools.validate_spelling()?;
        if let Some((first, second)) = self.conflict() {
            return Err(format!("{first} conflicts with {second}"));
        }
        if let Some(names) = &self.tools {
            validate_names(names, "allowlist")?;
        }
        if !self.exclude_tools.is_empty() {
            validate_names(&self.exclude_tools, "exclusion")?;
        }
        // An extension-provided Tool is refused on bare-name startup selection
        // surfaces, including `default_tools` — where an unknown name is
        // otherwise harmlessly ignored, and would therefore have made
        // `defaultTools: ["todo"]` look like it worked while deciding
        // nothing at all (Issue #259).
        for (names, label) in [
            (
                Some(self.profile.tools.builtin.as_slice()),
                "profile selection",
            ),
            (self.tools.as_deref(), "allowlist"),
            (Some(self.exclude_tools.as_slice()), "exclusion"),
        ] {
            for name in names.unwrap_or_default() {
                reject_extension_tool(name, label)?;
            }
        }
        Ok(())
    }
}

/// Explicit lists never discard empty entries or silently deduplicate.
pub(crate) fn validate_names(names: &[String], label: &str) -> Result<(), String> {
    if names.is_empty() || names.iter().any(|name| name.trim().is_empty()) {
        return Err(format!(
            "Tool {label} must contain non-empty names; use --no-tools for zero Tools"
        ));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(format!("Tool {label} entry {name:?} is repeated"));
        }
    }
    Ok(())
}

fn resolve_name<'a>(
    eligible: &[&'a ToolDefinition],
    name: &str,
    label: &str,
) -> Result<&'a ToolDefinition, String> {
    let mut matches = eligible.iter().copied().filter(|entry| entry.name == name);
    let first = matches
        .next()
        .ok_or_else(|| format!("Tool {label} entry {name:?} is unknown or ineligible"))?;
    if matches.next().is_some() {
        return Err(format!(
            "Tool {label} entry {name:?} is ambiguous across available origins"
        ));
    }
    Ok(first)
}

/// One available validated Tool, including inactive tools.
#[derive(Debug, Clone, PartialEq)]
pub struct AvailableTool {
    /// The canonical definition known to the runtime.
    pub definition: ToolDefinition,
}

/// The immutable available Tool catalog of one capability candidate.
#[derive(Debug, Clone, Default)]
pub struct AvailableToolCatalog {
    tools: Vec<AvailableTool>,
    registrations: Vec<ToolRegistration>,
}

impl PartialEq for AvailableToolCatalog {
    fn eq(&self, other: &Self) -> bool {
        self.tools == other.tools
            && self
                .registrations
                .iter()
                .map(ToolRegistration::foreground)
                .eq(other.registrations.iter().map(ToolRegistration::foreground))
    }
}

impl AvailableToolCatalog {
    pub(crate) fn metadata(definitions: impl IntoIterator<Item = ToolDefinition>) -> Self {
        Self {
            tools: definitions
                .into_iter()
                .map(|definition| AvailableTool { definition })
                .collect(),
            registrations: Vec::new(),
        }
    }

    /// Creates an available catalog in deterministic registration order.
    #[must_use]
    pub(crate) fn new(registrations: Vec<ToolRegistration>) -> Self {
        Self {
            tools: registrations
                .iter()
                .map(|entry| entry.definition.clone())
                .map(|definition| AvailableTool { definition })
                .collect(),
            registrations,
        }
    }

    /// The exact executable registrations of this authorized generation.
    /// Model exposure is independently selected and never widened here.
    #[must_use]
    pub(crate) fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }

    pub(crate) fn registration(
        &self,
        expected: &ToolDefinition,
    ) -> Result<&ToolRegistration, String> {
        self.registrations
            .iter()
            .find(|entry| entry.definition.id == expected.id && entry.definition == *expected)
            .ok_or_else(|| "frozen capability identity changed or disappeared".into())
    }

    /// Every available Tool definition, including inactive definitions.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    /// Every available Tool in deterministic order.
    #[must_use]
    pub fn tools(&self) -> &[AvailableTool] {
        &self.tools
    }
}

/// Applies the bounded startup selection pipeline to one complete available
/// registration set, then composes the extension-provided Tool surfaces of
/// the same Agent composition (Issue #259).
///
/// The returned registry contains only active Tools. The available catalog
/// contains every provided *ordinary* available Tool and is safe to project
/// independently.
///
/// `extensions` is deliberately outside the catalog. The available catalog is
/// the ordinary capability universe — it is what `--tools`, `--exclude-tools`,
/// a role's `tools.builtin`, and a Workflow capability selection resolve
/// against — and an extension Tool is not selectable through any of them. It
/// still passes the same per-registration validation and the same final
/// registry identity check, so it can never shadow or be shadowed by an
/// ordinary capability of the same name.
pub(crate) fn select_tools(
    available: &[ToolRegistration],
    extensions: &[ToolRegistration],
    policy: &ToolActivationPolicy,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<
    (
        AvailableToolCatalog,
        ToolRegistry,
        crate::runtime::agent_profile::ResolvedAgentProfile,
    ),
    String,
> {
    // Validate every candidate before projecting availability. Selection can
    // intentionally hide ordinary tools (`no_tools`, exclusions, or a strict
    // allowlist), but it must never hide an identity collision with a
    // runtime-owned protocol name.
    for registration in available.iter().chain(extensions) {
        // Validate each available capability even when activation hides it.
        // A one-entry registry reuses native registration validation without
        // treating same-name, source-qualified available tools as collisions.
        ToolRegistry::from_registrations([registration.clone()])
            .map_err(|error| format!("available Tool selection is invalid: {error}"))?;
    }
    let available_catalog = AvailableToolCatalog::new(available.to_vec());
    let definitions = available
        .iter()
        .map(|registration| &registration.definition)
        .collect::<Vec<_>>();
    let profile = resolve_profile(&available_catalog, policy, skills, availability)?;
    let selected = apply_cli(&definitions, &profile, policy)?;
    let registrations = selected
        .into_iter()
        .map(|definition| {
            available
                .iter()
                .find(|registration| std::ptr::eq(&raw const registration.definition, definition))
                .expect("selected available definition")
                .clone()
        })
        // The composition, and the reason it is an append rather than a
        // filter pass: no ordinary selection outcome — `no_tools`, an exact
        // allowlist, an exclusion — participates in whether an extension
        // Tool is active. The frozen extension composition already decided.
        .chain(extensions.iter().cloned());
    let active = ToolRegistry::from_registrations(registrations)
        .map_err(|error| format!("active Tool selection is invalid: {error}"))?;
    Ok((available_catalog, active, profile))
}

/// Prospective metadata projection uses the same profile boundary as execution.
pub(crate) fn select_definitions<'a>(
    available: &[&'a ToolDefinition],
    policy: &ToolActivationPolicy,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<Vec<&'a ToolDefinition>, String> {
    let catalog =
        AvailableToolCatalog::metadata(available.iter().map(|definition| (*definition).clone()));
    let profile = resolve_profile(&catalog, policy, skills, availability)?;
    apply_cli(available, &profile, policy)
}
fn resolve_profile(
    available: &AvailableToolCatalog,
    policy: &ToolActivationPolicy,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<crate::runtime::agent_profile::ResolvedAgentProfile, String> {
    use crate::runtime::agent_profile::{
        AgentProfile, AgentProfileAuthority, AgentScope, resolve_agent_profile,
    };
    let profile = AgentProfile::from_document(&policy.profile, policy.project_files.clone())?;
    Ok(resolve_agent_profile(
        &profile,
        &AgentProfileAuthority {
            tools: available,
            availability,
            skills,
            agents: &policy.admitted_agents,
            workflows: &policy.admitted_workflows,
            scope: AgentScope::Root,
        },
    ))
}
/// CLI controls are a final restriction of resolved profile exposure.
fn apply_cli<'a>(
    available: &[&'a ToolDefinition],
    profile: &crate::runtime::agent_profile::ResolvedAgentProfile,
    policy: &ToolActivationPolicy,
) -> Result<Vec<&'a ToolDefinition>, String> {
    policy.validate()?;
    let eligible: Vec<_> = available
        .iter()
        .copied()
        .filter(|definition| !policy.no_builtin_tools || definition.origin != ToolOrigin::Builtin)
        .filter(|definition| {
            profile
                .tools
                .iter()
                .any(|selected| selected.id == definition.id)
                || profile
                    .workflows
                    .iter()
                    .any(|id| definition.id == crate::tools::native::workflow_tool_id(id))
                || (!profile.agents.is_empty()
                    && definition.id == crate::tools::native::subagent_tool_id())
        })
        .collect();
    let mut selected = if policy.no_tools {
        Vec::new()
    } else if let Some(names) = &policy.tools {
        names
            .iter()
            .map(|name| resolve_name(&eligible, name, "allowlist"))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        eligible.clone()
    };
    let excluded = policy
        .exclude_tools
        .iter()
        .map(|name| resolve_name(available, name, "exclusion").map(|entry| &entry.id))
        .collect::<Result<BTreeSet<_>, _>>()?;
    selected.retain(|definition| !excluded.contains(&definition.id));
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ToolActivationPolicy;
    fn select_tools(
        available: &[ToolRegistration],
        extensions: &[ToolRegistration],
        policy: &ToolActivationPolicy,
    ) -> Result<(super::AvailableToolCatalog, ToolRegistry), String> {
        let availability = available
            .iter()
            .filter_map(|entry| entry.definition.origin.source())
            .map(|id| (id, crate::capabilities::CapabilitySourceState::Ready))
            .collect();
        super::select_tools(
            available,
            extensions,
            policy,
            &crate::skills::SkillSnapshot::new(Vec::new()),
            &availability,
        )
        .map(|(catalog, registry, _)| (catalog, registry))
    }
    use crate::runtime::identity::ToolId;
    use crate::tools::deadline::ToolProgressCapability;
    use crate::tools::executor::{
        ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistration, ToolRegistry,
    };
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionResult, ToolExecutionStatus,
        ToolInvocation, ToolOrigin, ToolReplayPolicy,
    };

    struct NoopTool;

    impl ToolExecutor for NoopTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> ToolExecutionHandle<'a> {
            ToolExecutionHandle::settled_by_operation(
                Box::pin(async {
                    ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    }
                }),
                context.cancellation.clone(),
            )
        }

        fn progress_capability(&self) -> ToolProgressCapability {
            ToolProgressCapability::None
        }
    }

    fn source_policy() -> ToolActivationPolicy {
        ToolActivationPolicy {
            profile: crate::local_runtime::config::AgentProfileDocument {
                tools: crate::capabilities::selection::ToolSelectionDocument {
                    builtin: crate::local_runtime::config::builtin_root_profile()
                        .tools
                        .builtin,
                    sources: [(
                        crate::capabilities::ToolSourceId::Mcp(
                            crate::runtime::identity::McpServerId::new("search"),
                        ),
                        crate::capabilities::selection::SourceToolSelection::All,
                    )]
                    .into(),
                },
                ..crate::local_runtime::config::builtin_root_profile()
            },
            ..ToolActivationPolicy::default()
        }
    }

    #[test]
    fn materialization_does_not_implicitly_expose_external_tools_to_main() {
        let (_, active) =
            select_tools(&registrations(), &[], &ToolActivationPolicy::default()).unwrap();
        assert_eq!(names(&active), ["read", "bash"]);
    }

    fn definition(name: &str, origin: ToolOrigin) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new(format!("tool-{name}")),
            name: name.to_owned(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false
            }),
            execution_policy: crate::tools::types::ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin,
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for (name, origin) in [
            ("read", ToolOrigin::Builtin),
            ("bash", ToolOrigin::Builtin),
            (
                "search",
                ToolOrigin::Mcp {
                    server_id: crate::runtime::identity::McpServerId::new("search"),
                },
            ),
        ] {
            registry
                .register(definition(name, origin), Arc::new(NoopTool))
                .expect("test Tool registration");
        }
        registry
    }

    fn names(registry: &ToolRegistry) -> Vec<String> {
        registry.names().into_iter().map(str::to_owned).collect()
    }

    fn registrations() -> Vec<ToolRegistration> {
        registry().registrations()
    }

    #[test]
    fn available_and_active_sets_are_distinct_and_selection_is_deterministic() {
        let policy = ToolActivationPolicy {
            profile: crate::local_runtime::config::AgentProfileDocument {
                tools: crate::capabilities::selection::ToolSelectionDocument {
                    builtin: (Some(vec!["read".to_owned()])).unwrap_or_else(|| {
                        crate::local_runtime::config::builtin_root_profile()
                            .tools
                            .builtin
                    }),
                    sources: Default::default(),
                },
                ..crate::local_runtime::config::builtin_root_profile()
            },
            ..source_policy()
        };
        let (available, active) =
            select_tools(&registrations(), &[], &policy).expect("activation selection");

        assert_eq!(
            available
                .definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "bash", "search"]
        );
        assert_eq!(names(&active), vec!["read", "search"]);
        assert!(!active.names().contains(&"bash"));
    }

    #[test]
    fn builtin_disable_and_no_tools_retain_truthful_availability() {
        let (available, active) = select_tools(
            &registrations(),
            &[],
            &ToolActivationPolicy {
                profile: crate::local_runtime::config::AgentProfileDocument {
                    tools: crate::capabilities::selection::ToolSelectionDocument {
                        builtin: (Some(Vec::new())).unwrap_or_else(|| {
                            crate::local_runtime::config::builtin_root_profile()
                                .tools
                                .builtin
                        }),
                        sources: Default::default(),
                    },
                    ..crate::local_runtime::config::builtin_root_profile()
                },
                ..source_policy()
            },
        )
        .expect("empty native defaults");
        assert_eq!(names(&active), vec!["search"]);
        assert_eq!(available.tools().len(), 3);

        let (available, active) = select_tools(
            &registrations(),
            &[],
            &ToolActivationPolicy {
                no_builtin_tools: true,
                ..source_policy()
            },
        )
        .expect("native disable");
        assert_eq!(names(&active), vec!["search"]);
        assert_eq!(available.tools().len(), 3);

        let (available, active) = select_tools(
            &registrations(),
            &[],
            &ToolActivationPolicy {
                no_tools: true,
                ..source_policy()
            },
        )
        .expect("all tools disable");
        assert!(names(&active).is_empty());
        assert_eq!(available.tools().len(), 3);
    }

    #[test]
    fn strict_allowlist_and_final_exclusions_cross_origins() {
        let (available, active) = select_tools(
            &registrations(),
            &[],
            &ToolActivationPolicy {
                tools: Some(vec!["bash".to_owned(), "search".to_owned()]),
                exclude_tools: vec!["bash".to_owned()],
                ..source_policy()
            },
        )
        .expect("cross-origin allowlist");
        assert_eq!(names(&active), vec!["search"]);
        assert_eq!(available.tools().len(), 3);

        let error = select_tools(
            &registrations(),
            &[],
            &ToolActivationPolicy {
                tools: Some(vec!["missing".to_owned()]),
                ..source_policy()
            },
        )
        .expect_err("unknown allowlist entry");
        assert!(error.contains("unknown or ineligible"));
    }

    #[test]
    fn ambiguous_allowlist_identity_fails_without_last_wins() {
        let first = definition(
            "duplicate",
            ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("one"),
            },
        );
        let second = definition(
            "duplicate",
            ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("two"),
            },
        );
        let mut first_registry = ToolRegistry::new();
        first_registry
            .register(first, Arc::new(NoopTool))
            .expect("first duplicate candidate");
        let mut second_registry = ToolRegistry::new();
        second_registry
            .register(second, Arc::new(NoopTool))
            .expect("second duplicate candidate");
        let registrations: Vec<_> = first_registry
            .registrations()
            .into_iter()
            .chain(second_registry.registrations())
            .collect();

        let error = select_tools(
            &registrations,
            &[],
            &ToolActivationPolicy {
                profile: crate::local_runtime::config::AgentProfileDocument {
                    tools: crate::capabilities::selection::ToolSelectionDocument {
                        builtin: crate::local_runtime::config::builtin_root_profile()
                            .tools
                            .builtin,
                        sources: ["one", "two"]
                            .map(|id| {
                                (
                                    crate::capabilities::ToolSourceId::Mcp(
                                        crate::runtime::identity::McpServerId::new(id),
                                    ),
                                    crate::capabilities::selection::SourceToolSelection::All,
                                )
                            })
                            .into(),
                    },
                    ..crate::local_runtime::config::builtin_root_profile()
                },
                tools: Some(vec!["duplicate".to_owned()]),
                ..ToolActivationPolicy::default()
            },
        )
        .expect_err("ambiguous identity");
        assert!(error.contains("ambiguous"));
        let error = select_tools(
            &registrations,
            &[],
            &ToolActivationPolicy {
                profile: crate::local_runtime::config::AgentProfileDocument {
                    tools: crate::capabilities::selection::ToolSelectionDocument {
                        builtin: crate::local_runtime::config::builtin_root_profile()
                            .tools
                            .builtin,
                        sources: ["one", "two"]
                            .map(|id| {
                                (
                                    crate::capabilities::ToolSourceId::Mcp(
                                        crate::runtime::identity::McpServerId::new(id),
                                    ),
                                    crate::capabilities::selection::SourceToolSelection::All,
                                )
                            })
                            .into(),
                    },
                    ..crate::local_runtime::config::builtin_root_profile()
                },
                exclude_tools: vec!["duplicate".to_owned()],
                ..ToolActivationPolicy::default()
            },
        )
        .expect_err("exclusion may not silently choose or remove multiple origins");
        assert!(error.contains("ambiguous"));
    }

    #[test]
    fn explicit_selection_fails_closed_and_exclusions_are_final() {
        for policy in [
            ToolActivationPolicy {
                tools: Some(vec![]),
                ..Default::default()
            },
            ToolActivationPolicy {
                tools: Some(vec![String::new()]),
                ..Default::default()
            },
            ToolActivationPolicy {
                tools: Some(vec!["read".into(), "read".into()]),
                ..Default::default()
            },
            ToolActivationPolicy {
                exclude_tools: vec![String::new()],
                ..Default::default()
            },
            ToolActivationPolicy {
                exclude_tools: vec!["read".into(), "read".into()],
                ..Default::default()
            },
            ToolActivationPolicy {
                exclude_tools: vec!["typo".into()],
                ..Default::default()
            },
            ToolActivationPolicy {
                tools: Some(vec!["subagent".into()]),
                ..Default::default()
            },
            ToolActivationPolicy {
                no_builtin_tools: true,
                exclude_tools: vec!["read".into()],
                ..Default::default()
            },
            ToolActivationPolicy {
                no_tools: true,
                tools: Some(vec!["read".into()]),
                ..Default::default()
            },
            ToolActivationPolicy {
                no_tools: true,
                exclude_tools: vec!["read".into()],
                ..Default::default()
            },
            ToolActivationPolicy {
                no_tools: true,
                no_builtin_tools: true,
                ..Default::default()
            },
            ToolActivationPolicy {
                tools: Some(vec!["read".into()]),
                no_builtin_tools: true,
                ..Default::default()
            },
        ] {
            assert!(
                select_tools(&registrations(), &[], &policy).is_err(),
                "{policy:?}"
            );
        }
        for (policy, expected) in [
            (source_policy(), vec!["read", "bash", "search"]),
            (
                ToolActivationPolicy {
                    tools: Some(vec!["bash".into()]),
                    ..source_policy()
                },
                vec!["bash"],
            ),
            (
                ToolActivationPolicy {
                    exclude_tools: vec!["read".into()],
                    ..source_policy()
                },
                vec!["bash", "search"],
            ),
            (
                ToolActivationPolicy {
                    tools: Some(vec!["read".into()]),
                    exclude_tools: vec!["read".into()],
                    ..source_policy()
                },
                vec![],
            ),
            (
                ToolActivationPolicy {
                    no_builtin_tools: true,
                    exclude_tools: vec!["search".into()],
                    ..source_policy()
                },
                vec![],
            ),
        ] {
            let (_, active) = select_tools(&registrations(), &[], &policy).unwrap();
            assert_eq!(names(&active), expected, "{policy:?}");
        }
    }

    /// Candidate availability is validated before activation filtering, so a
    /// hidden ordinary capability cannot collide with a runtime protocol.
    #[test]
    fn reserved_workflow_output_is_rejected_before_no_tools_hides_it() {
        for origin in [
            ToolOrigin::Builtin,
            ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("server-1"),
            },
        ] {
            let error = select_tools(
                &[ToolRegistration::plain(
                    definition(crate::tools::executor::WORKFLOW_OUTPUT_TOOL_NAME, origin),
                    Arc::new(NoopTool),
                )],
                &[],
                &ToolActivationPolicy {
                    no_tools: true,
                    ..source_policy()
                },
            )
            .expect_err("reserved candidate must be rejected before selection");
            assert!(error.contains("workflow_output"));
        }
    }
}
