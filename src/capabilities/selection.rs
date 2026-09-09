//! Source-qualified selection from one immutable capability generation.
use super::{
    AvailableToolCatalog, CapabilityAvailability, CapabilitySourceId, CapabilitySourceState,
};
use crate::runtime::identity::McpServerId;
use crate::tools::types::{ToolDefinition, ToolOrigin};
use serde::{Deserialize, Serialize};

/// Trusted selection syntax shared by Subagent and Workflow authoring.
/// Managed Python uses the existing `python:<package>` MCP server identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum ToolSelector {
    Builtin {
        name: String,
    },
    Mcp {
        server_id: McpServerId,
        name: String,
    },
}

impl ToolSelector {
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Mcp { server_id, name } => format!("mcp:{server_id}/{name}"),
        }
    }
}

impl std::fmt::Display for ToolSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Source health and invalid selection are deliberately distinct failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSelectionError {
    SourceUnavailable {
        selector: String,
        source: String,
        reason: String,
    },
    UnknownCapability {
        selector: String,
    },
}

impl std::fmt::Display for ToolSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceUnavailable {
                selector,
                source,
                reason,
            } => write!(
                f,
                "{selector} requires unavailable source {source}: {reason}"
            ),
            Self::UnknownCapability { selector } => write!(f, "unknown capability {selector}"),
        }
    }
}
impl std::error::Error for ToolSelectionError {}

/// Resolves an exact definition, never consulting the model-active registry.
pub(crate) fn resolve_selector<'a>(
    selector: &ToolSelector,
    available: &'a AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<&'a ToolDefinition, ToolSelectionError> {
    resolve_metadata(
        selector,
        available.tools().iter().map(|tool| &tool.definition),
        availability,
    )
}

/// Source-qualified semantic resolution over known definitions. Static callers
/// carry genuine native metadata and unprepared source facts, never executors.
pub(crate) fn resolve_metadata<'a>(
    selector: &ToolSelector,
    available: impl IntoIterator<Item = &'a ToolDefinition>,
    availability: &CapabilityAvailability,
) -> Result<&'a ToolDefinition, ToolSelectionError> {
    if let ToolSelector::Mcp { server_id, .. } = selector {
        let source = CapabilitySourceId::Mcp(server_id.clone());
        let reason = match availability.get(&source) {
            Some(CapabilitySourceState::Unavailable { reason }) => Some(reason.clone()),
            Some(CapabilitySourceState::Inactive { activation }) => Some(
                activation
                    .admit()
                    .err()
                    .unwrap_or("source is not prepared")
                    .to_owned(),
            ),
            Some(CapabilitySourceState::Unprepared) => {
                Some("source is enabled but not prepared".to_owned())
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(ToolSelectionError::SourceUnavailable {
                selector: selector.canonical(),
                source: source.to_string(),
                reason,
            });
        }
    }
    available
        .into_iter()
        .find(|definition| match (selector, &definition.origin) {
            (ToolSelector::Builtin { name }, ToolOrigin::Builtin) => definition.name == *name,
            (ToolSelector::Mcp { server_id, name }, ToolOrigin::Mcp { server_id: actual }) => {
                actual == server_id && definition.name == *name
            }
            _ => false,
        })
        .ok_or_else(|| ToolSelectionError::UnknownCapability {
            selector: selector.canonical(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::deadline::{
        ForegroundPolicy, ToolExecutionDeadlinePolicy, ToolProgressCapability,
    };
    use crate::tools::executor::{
        ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistration, ToolRegistry,
    };
    use crate::tools::types::*;
    use std::sync::Arc;

    struct Unused;
    impl ToolExecutor for Unused {
        fn start<'a>(
            &'a self,
            _: ToolInvocation,
            _: ToolExecutionContext<'a>,
        ) -> ToolExecutionHandle<'a> {
            panic!("selection never executes")
        }
        fn progress_capability(&self) -> ToolProgressCapability {
            ToolProgressCapability::None
        }
    }
    fn definition(origin: ToolOrigin) -> ToolDefinition {
        ToolDefinition {
            id: crate::runtime::identity::ToolId::new(format!("{origin:?}")),
            name: "check".into(),
            description: "check".into(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin,
        }
    }

    #[test]
    fn qualified_selection_distinguishes_source_health_names_and_exact_identity() {
        let builtin = definition(ToolOrigin::Builtin);
        let server_id = McpServerId::new("python:check");
        let mcp = definition(ToolOrigin::Mcp {
            server_id: server_id.clone(),
        });
        let catalog = AvailableToolCatalog::new(vec![
            ToolRegistration::plain(builtin.clone(), Arc::new(Unused)),
            ToolRegistration::plain(mcp.clone(), Arc::new(Unused)),
        ]);
        let local = ToolSelector::Builtin {
            name: "check".into(),
        };
        let remote = ToolSelector::Mcp {
            server_id: server_id.clone(),
            name: "check".into(),
        };
        let mut availability = CapabilityAvailability::default();
        assert_eq!(
            resolve_selector(&local, &catalog, &availability).unwrap(),
            &builtin
        );
        assert_eq!(
            resolve_selector(&remote, &catalog, &availability).unwrap(),
            &mcp
        );
        let invalid = ToolSelector::Mcp {
            server_id: server_id.clone(),
            name: "unknown".into(),
        };
        assert!(matches!(
            resolve_selector(&invalid, &catalog, &availability),
            Err(ToolSelectionError::UnknownCapability { .. })
        ));
        availability.insert(
            CapabilitySourceId::Mcp(server_id),
            CapabilitySourceState::Unavailable {
                reason: "offline".into(),
            },
        );
        assert!(matches!(
            resolve_selector(&invalid, &catalog, &availability),
            Err(ToolSelectionError::SourceUnavailable { .. })
        ));
        assert_eq!(
            resolve_selector(&local, &catalog, &availability).unwrap(),
            &builtin
        );
        let mut changed = builtin;
        changed.description.push_str(" changed");
        assert!(catalog.registration(&changed).is_err());
    }

    #[test]
    fn registration_policy_is_frozen_independently_of_executor_and_model_input() {
        let executor: Arc<dyn ToolExecutor> = Arc::new(Unused);
        let definition = definition(ToolOrigin::Builtin);
        let leaf = ToolRegistration::plain(definition.clone(), executor.clone());
        let composite = ForegroundPolicy::Composite {
            total: ToolExecutionDeadlinePolicy::new(std::time::Duration::from_mins(10), None),
        };
        let mut registry = ToolRegistry::new();
        registry
            .register_with_execution_metadata(
                definition.clone(),
                executor,
                |value| Ok(value.clone()),
                composite,
            )
            .unwrap();
        let rebuilt = ToolRegistry::from_registrations(registry.registrations()).unwrap();
        assert_eq!(leaf.foreground(), ForegroundPolicy::Leaf);
        assert_eq!(rebuilt.foreground_policy(&definition.id), composite);
        let changed = rebuilt.registrations().remove(0);
        let id = ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new("fixture"),
        };
        assert!(
            changed
                .prepare_fixed(id.clone(), &definition, &serde_json::json!({}))
                .is_err(),
            "a frozen Leaf cannot be rebuilt as Composite even with identical definition and executor"
        );
        assert!(matches!(
            leaf.prepare_fixed(id, &definition, &serde_json::json!({"timeout_ms":1}))
                .unwrap(),
            crate::tools::executor::PreflightOutcome::Rejected { .. }
        ));
        assert_ne!(
            AvailableToolCatalog::new(vec![leaf]),
            AvailableToolCatalog::new(vec![changed])
        );
    }

    #[test]
    fn workflow_selection_has_no_subagent_feature_dependency() {
        let workflow = include_str!("../runtime/workflow/tool.rs");
        assert!(!workflow.contains("subagent::resolver"));
        assert!(!include_str!("../tools/executor.rs").contains("fn foreground_policy(&self)"));
    }
}
