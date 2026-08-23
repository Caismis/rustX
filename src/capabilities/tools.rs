//! Runtime-owned Tool availability and startup activation selection.
//!
//! Availability is the set of validated, currently eligible definitions the
//! runtime knows how to execute. Activation is the smaller immutable registry
//! exposed to the model and Agent Loop. Keeping both in this capability layer
//! prevents a discovery source or client projection from becoming a second
//! capability authority.

use std::collections::BTreeSet;

use crate::tools::executor::{ToolRegistration, ToolRegistry};
use crate::tools::types::{ToolDefinition, ToolOrigin};

/// Startup activation controls supplied by current runtime/project settings
/// and CLI options. They are never Session-persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolActivationPolicy {
    /// Optional built-in names active by default. `None` means every
    /// available optional built-in tool; external capabilities remain
    /// eligible. Canonical native Read is always active when it is present in
    /// the native base registry.
    pub default_tools: Option<Vec<String>>,
    /// Remove optional built-in tools from the default/allowlist eligibility
    /// set. Canonical native Read remains active.
    pub no_builtin_tools: bool,
    /// Disable every optional Tool while retaining availability. Canonical
    /// native Read remains active.
    pub no_tools: bool,
    /// A strict model-facing allowlist across all eligible origins. Canonical
    /// native Read is added even when it is not named.
    pub tools: Option<Vec<String>>,
    /// Final model-facing name exclusions. Canonical native Read cannot be
    /// excluded.
    pub exclude_tools: Vec<String>,
}

/// One available validated Tool, including inactive tools.
#[derive(Debug, Clone, PartialEq)]
pub struct AvailableTool {
    /// The canonical definition known to the runtime.
    pub definition: ToolDefinition,
}

/// The immutable available Tool catalog of one capability candidate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AvailableToolCatalog {
    tools: Vec<AvailableTool>,
}

impl AvailableToolCatalog {
    /// Creates an available catalog in deterministic registration order.
    #[must_use]
    pub fn new(definitions: Vec<ToolDefinition>) -> Self {
        Self {
            tools: definitions
                .into_iter()
                .map(|definition| AvailableTool { definition })
                .collect(),
        }
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
/// registration set.
///
/// The returned registry contains only active Tools. The available catalog
/// contains every provided available Tool and is safe to project
/// independently.
pub(crate) fn select_tools(
    available: &[ToolRegistration],
    policy: &ToolActivationPolicy,
) -> Result<(AvailableToolCatalog, ToolRegistry), String> {
    let available_catalog = AvailableToolCatalog::new(
        available
            .iter()
            .map(|registration| registration.definition.clone())
            .collect(),
    );
    let eligible = available
        .iter()
        .filter(|registration| {
            !policy.no_builtin_tools
                || registration.mandatory
                || !matches!(registration.definition.origin, ToolOrigin::Builtin)
        })
        .collect::<Vec<_>>();

    let mut selected = if policy.no_tools {
        Vec::new()
    } else if let Some(names) = &policy.tools {
        let mut selected = Vec::with_capacity(names.len());
        for name in names {
            let matches = eligible
                .iter()
                .filter(|registration| registration.definition.name == *name)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => {
                    return Err(format!(
                        "Tool allowlist entry {name:?} is unknown or ineligible"
                    ));
                }
                [registration] => {
                    if selected.iter().any(|item: &&ToolRegistration| {
                        item.definition.id == registration.definition.id
                    }) {
                        return Err(format!("Tool allowlist entry {name:?} is repeated"));
                    }
                    selected.push(*registration);
                }
                _ => {
                    return Err(format!(
                        "Tool allowlist entry {name:?} is ambiguous across available origins"
                    ));
                }
            }
        }
        selected
    } else {
        eligible
            .into_iter()
            .filter(|registration| {
                !matches!(registration.definition.origin, ToolOrigin::Builtin)
                    || policy
                        .default_tools
                        .as_ref()
                        .is_none_or(|names| names.contains(&registration.definition.name))
            })
            .collect::<Vec<_>>()
    };

    let excluded = policy.exclude_tools.iter().collect::<BTreeSet<_>>();
    selected.retain(|registration| {
        registration.mandatory || !excluded.contains(&registration.definition.name)
    });

    // The normal native runtime composition always supplies Read in the base
    // registry. Keep that capability active regardless of optional-tool
    // selection. Bare lower-level registries without native Read remain valid
    // for tests and specialized composition paths; they simply have no
    // mandatory registration to activate here.
    if let Some(mandatory) = available.iter().find(|registration| {
        registration.mandatory
            && !selected
                .iter()
                .any(|item: &&ToolRegistration| item.definition.id == registration.definition.id)
    }) {
        selected.insert(0, mandatory);
    }

    let active = ToolRegistry::from_registrations(selected.into_iter().cloned())
        .map_err(|error| format!("active Tool selection is invalid: {error}"))?;
    Ok((available_catalog, active))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;

    use super::{ToolActivationPolicy, select_tools};
    use crate::runtime::identity::ToolId;
    use crate::tools::executor::{
        ToolExecutionContext, ToolExecutor, ToolRegistration, ToolRegistry,
    };
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionResult, ToolExecutionStatus,
        ToolInvocation, ToolOrigin, ToolReplayPolicy,
    };

    struct NoopTool;

    impl ToolExecutor for NoopTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            Box::pin(async {
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                }
            })
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
        let mut registrations = registry().registrations();
        // The production native Read registration carries this activation
        // marker from `read::registration`; the fixture supplies the same
        // internal metadata without needing native runtime resources.
        registrations[0].mandatory = true;
        registrations
    }

    #[test]
    fn available_and_active_sets_are_distinct_and_selection_is_deterministic() {
        let policy = ToolActivationPolicy {
            default_tools: Some(vec!["read".to_owned()]),
            ..ToolActivationPolicy::default()
        };
        let (available, active) =
            select_tools(&registrations(), &policy).expect("activation selection");

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
            &ToolActivationPolicy {
                default_tools: Some(Vec::new()),
                ..ToolActivationPolicy::default()
            },
        )
        .expect("empty native defaults");
        assert_eq!(names(&active), vec!["read", "search"]);
        assert_eq!(available.tools().len(), 3);

        let (available, active) = select_tools(
            &registrations(),
            &ToolActivationPolicy {
                no_builtin_tools: true,
                ..ToolActivationPolicy::default()
            },
        )
        .expect("native disable");
        assert_eq!(names(&active), vec!["read", "search"]);
        assert_eq!(available.tools().len(), 3);

        let (available, active) = select_tools(
            &registrations(),
            &ToolActivationPolicy {
                no_tools: true,
                ..ToolActivationPolicy::default()
            },
        )
        .expect("all tools disable");
        assert_eq!(names(&active), vec!["read"]);
        assert_eq!(available.tools().len(), 3);
    }

    #[test]
    fn strict_allowlist_and_final_exclusions_cross_origins() {
        let (available, active) = select_tools(
            &registrations(),
            &ToolActivationPolicy {
                tools: Some(vec!["bash".to_owned(), "search".to_owned()]),
                exclude_tools: vec!["bash".to_owned()],
                ..ToolActivationPolicy::default()
            },
        )
        .expect("cross-origin allowlist");
        assert_eq!(names(&active), vec!["read", "search"]);
        assert_eq!(available.tools().len(), 3);

        let error = select_tools(
            &registrations(),
            &ToolActivationPolicy {
                tools: Some(vec!["missing".to_owned()]),
                ..ToolActivationPolicy::default()
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
            &ToolActivationPolicy {
                tools: Some(vec!["duplicate".to_owned()]),
                ..ToolActivationPolicy::default()
            },
        )
        .expect_err("ambiguous identity");
        assert!(error.contains("ambiguous"));
    }
}
