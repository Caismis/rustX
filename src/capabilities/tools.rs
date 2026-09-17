//! Profile-owned Tool exposure and global invocation-policy preservation.
//! Discovery and materialization do not grant model-facing authority. Plugins
//! and Agent/Workflow dispatch are independently selected by the same profile.

use std::collections::BTreeSet;

use crate::tools::executor::{ToolRegistration, ToolRegistry};
use crate::tools::types::ToolDefinition;

/// Startup activation controls supplied by current runtime/project settings
/// and CLI options. They are never Session-persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivation {
    /// Complete root profile intent, independent of admitted catalogs.
    pub profile: crate::local_runtime::config::AgentProfileDocument,
    pub admitted_agents: BTreeSet<crate::runtime::subagent::SubagentName>,
    pub admitted_workflows: BTreeSet<crate::runtime::workflow::WorkflowId>,
    pub project_files: Vec<crate::runtime::resources::ProjectContextFile>,
}

impl Default for AgentActivation {
    fn default() -> Self {
        Self {
            profile: crate::local_runtime::config::builtin_root_profile(),
            admitted_agents: BTreeSet::new(),
            admitted_workflows: BTreeSet::new(),
            project_files: Vec::new(),
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
            "Tool {label} entry {name:?} is provided by the {extension:?} Plugin; configure plugins.{extension}.enabled instead"
        )),
    }
}

impl AgentActivation {
    /// The Agent profile is the sole capability selection authority.
    /// # Errors
    /// Rejects invalid selections and names owned by Plugins.
    pub fn validate(&self) -> Result<(), String> {
        self.profile.tools.validate_spelling()?;
        for name in &self.profile.tools.builtin {
            reject_extension_tool(name, "profile selection")?;
        }
        Ok(())
    }
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
    policy: &AgentActivation,
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
    // intentionally hide ordinary tools (an empty or a strict
    // allowlist), but it must never hide an identity collision with a
    // runtime-owned protocol name.
    for registration in available.iter().chain(extensions) {
        // Validate each available capability even when activation hides it.
        // A one-entry registry reuses native registration validation without
        // treating same-name, source-qualified available tools as collisions.
        ToolRegistry::from_registrations([registration.clone()])
            .map_err(|error| format!("available Tool selection is invalid: {error}"))?;
    }
    let available_catalog = AvailableToolCatalog::new(
        available
            .iter()
            .filter(|entry| !crate::runtime::agent_profile::is_dispatcher(&entry.definition))
            .cloned()
            .collect(),
    );
    let definitions = available
        .iter()
        .map(|registration| &registration.definition)
        .collect::<Vec<_>>();
    let profile = resolve_profile(&available_catalog, policy, skills, availability)?;
    if let Some(diagnostic) = profile.diagnostics.first() {
        return Err(format!(
            "selected Agent capability cannot be admitted: {:?}",
            diagnostic.redacted()
        ));
    }
    let selected = selected_definitions(&definitions, &profile);
    let plugin_tools = crate::extensions::composed_extension_tool_names(&profile.extensions);
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
        // filter pass: no ordinary selection outcome — an empty whitelist, an exact
        // allowlist, an exclusion — participates in whether an extension
        // Tool is active. The frozen extension composition already decided.
        .chain(
            extensions
                .iter()
                .filter(|registration| plugin_tools.contains(&registration.definition.name))
                .cloned(),
        );
    let active = ToolRegistry::from_registrations(registrations)
        .map_err(|error| format!("active Tool selection is invalid: {error}"))?;
    Ok((available_catalog, active, profile))
}

pub(crate) fn inspect_profile(
    available: &[&ToolDefinition],
    policy: &AgentActivation,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<crate::runtime::agent_profile::ResolvedAgentProfile, String> {
    let catalog = AvailableToolCatalog::metadata(available.iter().map(|tool| (*tool).clone()));
    resolve_profile(&catalog, policy, skills, availability)
}

#[cfg(test)]
pub(crate) fn select_definitions<'a>(
    available: &[&'a ToolDefinition],
    policy: &AgentActivation,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<Vec<&'a ToolDefinition>, String> {
    let profile = inspect_profile(available, policy, skills, availability)?;
    Ok(selected_definitions(available, &profile))
}
fn resolve_profile(
    available: &AvailableToolCatalog,
    policy: &AgentActivation,
    skills: &crate::skills::SkillSnapshot,
    availability: &super::CapabilityAvailability,
) -> Result<crate::runtime::agent_profile::ResolvedAgentProfile, String> {
    use crate::runtime::agent_profile::{
        AgentProfile, AgentProfileAuthority, AgentScope, resolve_agent_profile,
    };
    policy.validate()?;
    let profile = AgentProfile::from_document(
        &policy.profile,
        crate::runtime::agent_profile::AgentProfileKind::Root,
        policy.project_files.clone(),
    )?;
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
fn selected_definitions<'a>(
    available: &[&'a ToolDefinition],
    profile: &crate::runtime::agent_profile::ResolvedAgentProfile,
) -> Vec<&'a ToolDefinition> {
    available
        .iter()
        .copied()
        .filter(|definition| {
            profile.tools.iter().any(|tool| tool.id == definition.id)
                || profile
                    .workflows
                    .iter()
                    .any(|id| definition.id == crate::tools::native::workflow_tool_id(id))
                || (!profile.agents.is_empty()
                    && definition.id == crate::tools::native::subagent_tool_id())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::AgentActivation;
    fn select_tools(
        available: &[ToolRegistration],
        extensions: &[ToolRegistration],
        policy: &AgentActivation,
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

    fn profile(native: &[&str]) -> AgentActivation {
        let mut result = AgentActivation::default();
        result.profile.tools.builtin = native.iter().map(|name| (*name).into()).collect();
        result
    }

    #[test]
    fn resource_existence_grants_no_root_tool_authority() {
        let (available, active) = select_tools(&registrations(), &[], &profile(&[])).unwrap();
        assert_eq!(available.definitions().len(), 3);
        assert!(active.is_empty());
    }

    #[test]
    fn native_whitelist_selects_exactly_and_preserves_invocation_policy() {
        let (available, active) = select_tools(&registrations(), &[], &profile(&["read"])).unwrap();
        assert_eq!(names(&active), ["read"]);
        assert_eq!(active.definitions()[0], available.definitions()[0]);
        for name in ["missing", "todo", "workflow_output"] {
            assert!(select_tools(&registrations(), &[], &profile(&[name])).is_err());
        }
    }

    #[test]
    fn source_all_exact_and_empty_are_distinct_from_discovery() {
        use crate::capabilities::selection::SourceToolSelection;
        let source = crate::capabilities::ToolSourceId::Mcp(
            crate::runtime::identity::McpServerId::new("search"),
        );
        for (selection, expected) in [
            (SourceToolSelection::All, vec!["search"]),
            (
                SourceToolSelection::Exact(vec!["search".into()]),
                vec!["search"],
            ),
            (SourceToolSelection::Exact(vec![]), vec![]),
        ] {
            let mut intent = profile(&[]);
            intent
                .profile
                .tools
                .sources
                .insert(source.clone(), selection);
            let (_, active) = select_tools(&registrations(), &[], &intent).unwrap();
            assert_eq!(names(&active), expected);
        }
        let mut intent = profile(&[]);
        intent
            .profile
            .tools
            .sources
            .insert(source, SourceToolSelection::Exact(vec!["missing".into()]));
        assert!(select_tools(&registrations(), &[], &intent).is_err());
    }

    #[test]
    fn plugin_tools_require_the_profile_plugin_and_are_not_ordinary_tools() {
        let extension = [ToolRegistration::plain(
            crate::tools::native::todo_tool_definition(),
            Arc::new(NoopTool),
        )];
        let mut intent = profile(&[]);
        assert!(
            select_tools(&registrations(), &extension, &intent)
                .unwrap()
                .1
                .is_empty()
        );
        intent.profile.extensions.todo.enabled = true;
        let (available, active) = select_tools(&registrations(), &extension, &intent).unwrap();
        assert_eq!(names(&active), ["todo"]);
        assert!(
            !available
                .definitions()
                .iter()
                .any(|tool| tool.name == "todo")
        );
        intent.profile.tools.builtin.push("todo".into());
        assert!(intent.validate().unwrap_err().contains("Plugin"));
    }

    #[test]
    fn dispatch_allowlists_do_not_expose_child_or_workflow_private_tools() {
        use crate::runtime::subagent::SubagentName;
        use crate::runtime::workflow::WorkflowId;
        let agent = SubagentName::parse("reviewer").unwrap();
        let workflow = WorkflowId::parse("review").unwrap();
        let mut delegate = definition("subagent", ToolOrigin::Builtin);
        delegate.id = crate::tools::native::subagent_tool_id();
        let mut run = definition("review", ToolOrigin::Builtin);
        run.id = crate::tools::native::workflow_tool_id(&workflow);
        let available = [delegate, run, definition("private", ToolOrigin::Builtin)]
            .map(|tool| ToolRegistration::plain(tool, Arc::new(NoopTool)));
        let mut intent = profile(&[]);
        intent.admitted_agents.insert(agent.clone());
        intent.admitted_workflows.insert(workflow.clone());
        assert!(select_tools(&available, &[], &intent).unwrap().1.is_empty());
        intent.profile.agents = vec![agent];
        intent.profile.workflows = vec![workflow];
        let (_, active) = select_tools(&available, &[], &intent).unwrap();
        assert_eq!(names(&active), ["subagent", "review"]);
    }

    #[test]
    fn reserved_names_are_rejected_even_when_the_profile_selects_nothing() {
        for origin in [
            ToolOrigin::Builtin,
            ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("server"),
            },
        ] {
            let registrations = [ToolRegistration::plain(
                definition(crate::tools::executor::WORKFLOW_OUTPUT_TOOL_NAME, origin),
                Arc::new(NoopTool),
            )];
            assert!(
                select_tools(&registrations, &[], &profile(&[]))
                    .unwrap_err()
                    .contains("workflow_output")
            );
        }
    }
}
