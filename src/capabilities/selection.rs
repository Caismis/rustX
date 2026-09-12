//! Source-qualified selection from one immutable capability generation.
use super::{AvailableToolCatalog, CapabilityAvailability, CapabilitySourceState, ToolSourceId};
use crate::tools::types::{ToolDefinition, ToolOrigin};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Exactly two trust granularities for one source. This is also the authoring
/// boundary: only the literal "all" or an exact array is accepted.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(from = "SourceSelectionAuthoring", into = "SourceSelectionAuthoring")]
#[schemars(with = "SourceSelectionAuthoring")]
pub enum SourceToolSelection {
    All,
    Exact(Vec<String>),
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum SourceSelectionAuthoring {
    All(AllTools),
    Exact(Vec<String>),
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum AllTools {
    All,
}
impl From<SourceSelectionAuthoring> for SourceToolSelection {
    fn from(value: SourceSelectionAuthoring) -> Self {
        match value {
            SourceSelectionAuthoring::All(_) => Self::All,
            SourceSelectionAuthoring::Exact(names) => Self::Exact(names),
        }
    }
}
impl From<SourceToolSelection> for SourceSelectionAuthoring {
    fn from(value: SourceToolSelection) -> Self {
        match value {
            SourceToolSelection::All => Self::All(AllTools::All),
            SourceToolSelection::Exact(names) => Self::Exact(names),
        }
    }
}

/// One exact executable identity, used by Workflow Tool leaves and allowlists.
/// Source-wide Agent capability selection cannot be authored in this type.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExactToolSelector {
    Builtin {
        name: String,
    },
    Source {
        source_id: ToolSourceId,
        name: String,
    },
}
impl ExactToolSelector {
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Source { source_id, name } => format!("source:{source_id}/{name}"),
        }
    }
    #[must_use]
    pub const fn source(&self) -> Option<&ToolSourceId> {
        match self {
            Self::Builtin { .. } => None,
            Self::Source { source_id, .. } => Some(source_id),
        }
    }
}
impl std::fmt::Display for ExactToolSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Agent admission/projection intent, lowered from `ToolSelectionDocument`.
/// This is never a Workflow Tool leaf or a frozen child executable identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum AgentToolSelection {
    Builtin {
        name: String,
    },
    Source {
        source_id: ToolSourceId,
        name: String,
    },
    All {
        source_id: ToolSourceId,
    },
}
impl From<&ExactToolSelector> for AgentToolSelection {
    fn from(selector: &ExactToolSelector) -> Self {
        match selector {
            ExactToolSelector::Builtin { name } => Self::Builtin { name: name.clone() },
            ExactToolSelector::Source { source_id, name } => Self::Source {
                source_id: source_id.clone(),
                name: name.clone(),
            },
        }
    }
}
impl AgentToolSelection {
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Source { source_id, name } => format!("source:{source_id}/{name}"),
            Self::All { source_id } => format!("source:{source_id}"),
        }
    }
    #[must_use]
    pub const fn source(&self) -> Option<&ToolSourceId> {
        match self {
            Self::Builtin { .. } => None,
            Self::Source { source_id, .. } | Self::All { source_id } => Some(source_id),
        }
    }
}
impl std::fmt::Display for AgentToolSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Shared Agent and Workflow Agent override selection. Extensions are composed
/// by their native owner and never enter this ordinary capability vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ToolSelectionDocument {
    pub builtin: Vec<String>,
    pub sources: BTreeMap<ToolSourceId, SourceToolSelection>,
}
impl ToolSelectionDocument {
    #[must_use]
    pub fn selectors(&self) -> Vec<AgentToolSelection> {
        let mut result: Vec<_> = self
            .builtin
            .iter()
            .map(|name| AgentToolSelection::Builtin { name: name.clone() })
            .collect();
        for (source_id, selection) in &self.sources {
            match selection {
                SourceToolSelection::All => result.push(AgentToolSelection::All {
                    source_id: source_id.clone(),
                }),
                SourceToolSelection::Exact(names) => {
                    result.extend(names.iter().map(|name| AgentToolSelection::Source {
                        source_id: source_id.clone(),
                        name: name.clone(),
                    }));
                }
            }
        }
        result.sort();
        result
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selectors().is_empty()
    }
    /// Validate exact names and extension-plane separation.
    ///
    /// # Errors
    /// Returns the first malformed or duplicate entry in deterministic order.
    pub fn validate_spelling(&self) -> Result<(), String> {
        fn names(names: &[String]) -> Result<(), String> {
            let mut seen = BTreeSet::new();
            for name in names {
                if name.is_empty() {
                    return Err(format!("malformed exact Tool name {name:?}"));
                }
                if !seen.insert(name) {
                    return Err(format!("duplicate exact Tool name {name:?}"));
                }
            }
            Ok(())
        }
        names(&self.builtin)?;
        for name in &self.builtin {
            if let Some(extension) = super::extension_provided_tool(name) {
                return Err(format!(
                    "{name} is provided by the {extension:?} Agent Extension, not by ordinary Tool selection; compose it with extensions.{extension}.enabled instead"
                ));
            }
        }
        for selection in self.sources.values() {
            if let SourceToolSelection::Exact(exact) = selection {
                names(exact)?;
            }
        }
        Ok(())
    }
}

/// Typed facts for admission owners; they choose their own failure policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceResolutionFailure {
    Undefined,
    Inactive(super::activation::SourceActivation),
    Unprepared,
    Unavailable { reason: String },
}
impl std::fmt::Display for SourceResolutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undefined => f.write_str("source is not defined or discovered"),
            Self::Inactive(activation) => {
                f.write_str(activation.admit().err().unwrap_or("source is inactive"))
            }
            Self::Unprepared => f.write_str("source is eligible but not prepared"),
            Self::Unavailable { reason } => f.write_str(reason),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSelectionError {
    SourceUnavailable {
        selector: String,
        source: ToolSourceId,
        reason: SourceResolutionFailure,
    },
    ExactToolAbsent {
        source: ToolSourceId,
        name: String,
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
            Self::ExactToolAbsent { source, name } => write!(
                f,
                "ready source {source} does not publish exact Tool {name:?}"
            ),
            Self::UnknownCapability { selector } => write!(f, "unknown capability {selector}"),
        }
    }
}
impl std::error::Error for ToolSelectionError {}

/// Read one generation's source readiness.
///
/// # Errors
/// Returns the precise absence, authority, or materialization failure.
pub fn source_state(
    source: &ToolSourceId,
    availability: &CapabilityAvailability,
) -> Result<(), SourceResolutionFailure> {
    match availability.get(source) {
        None => Err(SourceResolutionFailure::Undefined),
        Some(CapabilitySourceState::Inactive { activation }) => {
            Err(SourceResolutionFailure::Inactive(*activation))
        }
        Some(CapabilitySourceState::Unprepared) => Err(SourceResolutionFailure::Unprepared),
        Some(CapabilitySourceState::Unavailable { reason }) => {
            Err(SourceResolutionFailure::Unavailable {
                reason: reason.clone(),
            })
        }
        Some(CapabilitySourceState::Ready) => Ok(()),
    }
}

/// Complete typed source resolution for later Agent/Workflow admission policy.
#[derive(Debug, PartialEq)]
pub enum SourceToolResolution<'a> {
    Unavailable(SourceResolutionFailure),
    Ready {
        selected: Vec<&'a ToolDefinition>,
        missing_exact: Vec<String>,
    },
}
/// Resolve source trust and exact identities without deciding admission policy.
pub fn resolve_source<'a>(
    source: &ToolSourceId,
    selection: &SourceToolSelection,
    definitions: impl IntoIterator<Item = &'a ToolDefinition>,
    availability: &CapabilityAvailability,
) -> SourceToolResolution<'a> {
    if let Err(reason) = source_state(source, availability) {
        return SourceToolResolution::Unavailable(reason);
    }
    let mut published: BTreeMap<_, _> = definitions
        .into_iter()
        .filter(|definition| definition.origin.source().as_ref() == Some(source))
        .map(|definition| (definition.name.clone(), definition))
        .collect();
    match selection {
        SourceToolSelection::All => SourceToolResolution::Ready {
            selected: published.into_values().collect(),
            missing_exact: Vec::new(),
        },
        SourceToolSelection::Exact(names) => {
            let mut selected = Vec::new();
            let mut missing_exact = Vec::new();
            for name in names {
                if let Some(tool) = published.remove(name) {
                    selected.push(tool);
                } else {
                    missing_exact.push(name.clone());
                }
            }
            selected.sort_by(|a, b| a.name.cmp(&b.name));
            missing_exact.sort();
            SourceToolResolution::Ready {
                selected,
                missing_exact,
            }
        }
    }
}

/// Expands source-wide trust only against this supplied immutable generation.
pub(crate) fn project<'a>(
    selector: &AgentToolSelection,
    definitions: impl IntoIterator<Item = &'a ToolDefinition>,
    availability: &CapabilityAvailability,
) -> Result<Vec<&'a ToolDefinition>, ToolSelectionError> {
    if let Some(source) = selector.source() {
        let mode = match selector {
            AgentToolSelection::All { .. } => SourceToolSelection::All,
            AgentToolSelection::Source { name, .. } => {
                SourceToolSelection::Exact(vec![name.clone()])
            }
            AgentToolSelection::Builtin { .. } => unreachable!("builtin has no source"),
        };
        return match resolve_source(source, &mode, definitions, availability) {
            SourceToolResolution::Unavailable(reason) => {
                Err(ToolSelectionError::SourceUnavailable {
                    selector: selector.canonical(),
                    source: source.clone(),
                    reason,
                })
            }
            SourceToolResolution::Ready {
                selected,
                missing_exact,
            } if missing_exact.is_empty() => Ok(selected),
            SourceToolResolution::Ready { missing_exact, .. } => {
                Err(ToolSelectionError::ExactToolAbsent {
                    source: source.clone(),
                    name: missing_exact[0].clone(),
                })
            }
        };
    }
    let AgentToolSelection::Builtin { name } = selector else {
        unreachable!("source handled above")
    };
    definitions
        .into_iter()
        .find(|definition| definition.origin == ToolOrigin::Builtin && definition.name == *name)
        .map(|definition| vec![definition])
        .ok_or_else(|| ToolSelectionError::UnknownCapability {
            selector: selector.canonical(),
        })
}
pub(crate) fn resolve_selector<'a>(
    selector: &ExactToolSelector,
    available: &'a AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<&'a ToolDefinition, ToolSelectionError> {
    resolve_metadata(
        selector,
        available.tools().iter().map(|tool| &tool.definition),
        availability,
    )
}
pub(crate) fn resolve_metadata<'a>(
    selector: &ExactToolSelector,
    available: impl IntoIterator<Item = &'a ToolDefinition>,
    availability: &CapabilityAvailability,
) -> Result<&'a ToolDefinition, ToolSelectionError> {
    let selected = project(&AgentToolSelection::from(selector), available, availability)?;
    selected
        .into_iter()
        .next()
        .ok_or_else(|| ToolSelectionError::UnknownCapability {
            selector: selector.canonical(),
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::McpServerId;
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
        let local = ExactToolSelector::Builtin {
            name: "check".into(),
        };
        let remote = ExactToolSelector::Source {
            source_id: ToolSourceId::Mcp(server_id.clone()),
            name: "check".into(),
        };
        let mut availability = CapabilityAvailability::from([(
            ToolSourceId::Mcp(server_id.clone()),
            CapabilitySourceState::Ready,
        )]);
        assert_eq!(
            resolve_selector(&local, &catalog, &availability).unwrap(),
            &builtin
        );
        assert_eq!(
            resolve_selector(&remote, &catalog, &availability).unwrap(),
            &mcp
        );
        let invalid = ExactToolSelector::Source {
            source_id: ToolSourceId::Mcp(server_id.clone()),
            name: "unknown".into(),
        };
        assert!(matches!(
            resolve_selector(&invalid, &catalog, &availability),
            Err(ToolSelectionError::ExactToolAbsent { .. })
        ));
        availability.insert(
            ToolSourceId::Mcp(server_id),
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
    fn source_definition(source: &ToolSourceId, name: &str) -> ToolDefinition {
        let origin = match source {
            ToolSourceId::Mcp(id) => ToolOrigin::Mcp {
                server_id: id.clone(),
            },
            ToolSourceId::ManagedPython(package) => ToolOrigin::ManagedPython {
                package: package.clone(),
            },
        };
        let mut result = definition(origin);
        result.name = name.into();
        result.id = crate::runtime::identity::ToolId::new(format!("{source}/{name}"));
        result
    }
    #[test]
    fn all_exact_and_same_names_use_canonical_source_provenance() {
        let mcp = ToolSourceId::Mcp(McpServerId::new("github"));
        let python = ToolSourceId::ManagedPython("data-analysis".into());
        let tools = [
            source_definition(&mcp, "b"),
            source_definition(&python, "a"),
            source_definition(&mcp, "a"),
        ];
        let availability = [
            (mcp.clone(), CapabilitySourceState::Ready),
            (python.clone(), CapabilitySourceState::Ready),
        ]
        .into();
        for source in [&mcp, &python] {
            let SourceToolResolution::Ready {
                selected,
                missing_exact,
            } = resolve_source(source, &SourceToolSelection::All, &tools, &availability)
            else {
                panic!("ready source")
            };
            assert!(missing_exact.is_empty());
            assert!(
                selected
                    .iter()
                    .all(|tool| tool.origin.source().as_ref() == Some(source))
            );
            assert_eq!(selected.len(), if source == &mcp { 2 } else { 1 });
            let exact = resolve_source(
                source,
                &SourceToolSelection::Exact(vec!["a".into()]),
                &tools,
                &availability,
            );
            assert_eq!(
                exact,
                SourceToolResolution::Ready {
                    selected: vec![
                        tools
                            .iter()
                            .find(|tool| tool.origin.source().as_ref() == Some(source)
                                && tool.name == "a")
                            .unwrap()
                    ],
                    missing_exact: vec![]
                }
            );
        }
    }
    #[test]
    fn resolution_classifies_absence_authority_materialization_and_exact_absence() {
        use super::super::activation::SourceActivation;
        let source = ToolSourceId::Mcp(McpServerId::new("github"));
        let tools = [source_definition(&source, "a")];
        for mode in [
            SourceToolSelection::All,
            SourceToolSelection::Exact(vec!["a".into()]),
        ] {
            for (state, expected) in [
                (None, SourceResolutionFailure::Undefined),
                (
                    Some(CapabilitySourceState::Inactive {
                        activation: SourceActivation::Disabled,
                    }),
                    SourceResolutionFailure::Inactive(SourceActivation::Disabled),
                ),
                (
                    Some(CapabilitySourceState::Inactive {
                        activation: SourceActivation::Untrusted,
                    }),
                    SourceResolutionFailure::Inactive(SourceActivation::Untrusted),
                ),
                (
                    Some(CapabilitySourceState::Unprepared),
                    SourceResolutionFailure::Unprepared,
                ),
                (
                    Some(CapabilitySourceState::unavailable("offline")),
                    SourceResolutionFailure::Unavailable {
                        reason: "offline".into(),
                    },
                ),
            ] {
                let availability = state
                    .map(|state| (source.clone(), state))
                    .into_iter()
                    .collect();
                assert_eq!(
                    resolve_source(&source, &mode, &tools, &availability),
                    SourceToolResolution::Unavailable(expected)
                );
            }
        }
        let ready = [(source.clone(), CapabilitySourceState::Ready)].into();
        assert_eq!(
            resolve_source(
                &source,
                &SourceToolSelection::Exact(vec!["missing".into()]),
                &tools,
                &ready
            ),
            SourceToolResolution::Ready {
                selected: vec![],
                missing_exact: vec!["missing".into()]
            }
        );
    }
    #[test]
    fn authoring_has_one_strict_source_vocabulary_and_rejects_duplicate_exact_names() {
        #[derive(serde::Deserialize)]
        struct Document {
            tools: ToolSelectionDocument,
        }
        let valid = r#"[tools]
builtin = ["read", "grep"]
[tools.sources]
github = "all"
"python:data-analysis" = ["run_python", "inspect_dataframe"]
"#;
        let parsed: Document = toml::from_str(valid).unwrap();
        parsed.tools.validate_spelling().unwrap();
        assert_eq!(
            parsed.tools.sources[&ToolSourceId::Mcp(McpServerId::new("github"))],
            SourceToolSelection::All
        );
        for bad in [
            "mcp = {}",
            "python = {}",
            "sources = {github = 'ALL'}",
            "sources = {github = '*'}",
            "sources = {'python:' = 'all'}",
        ] {
            assert!(
                toml::from_str::<ToolSelectionDocument>(bad).is_err(),
                "{bad}"
            );
        }
        for bad in [
            "sources = {github = ['a','a']}",
            "sources = {github = ['']}",
            "builtin = ['todo']",
            "builtin = ['read','read']",
        ] {
            let parsed: ToolSelectionDocument = toml::from_str(bad).unwrap();
            assert!(parsed.validate_spelling().is_err(), "{bad}");
        }
    }
    #[test]
    fn root_and_named_agent_documents_keep_source_all() {
        let selection = "[tools.sources]\ngithub = 'all'\n";
        let root: crate::local_runtime::config::CurrentRuntimeConfig =
            toml::from_str(&format!("[model]\nmodel = 'local/test'\n{selection}")).unwrap();
        let named: crate::local_runtime::config::AgentDocument = toml::from_str(&format!(
            "description = 'Review'\ninstructions = 'Review code'\n{selection}"
        ))
        .unwrap();
        let source = ToolSourceId::Mcp(McpServerId::new("github"));
        assert_eq!(
            root.tools.unwrap().sources[&source],
            SourceToolSelection::All
        );
        assert_eq!(named.tools.sources[&source], SourceToolSelection::All);
    }

    #[test]
    fn exact_leaf_resolution_preserves_broad_names_and_source_provenance() {
        let sources = [
            ToolSourceId::Mcp(McpServerId::new("a/b ? 工具")),
            ToolSourceId::ManagedPython("data-analysis".into()),
        ];
        let availability = sources
            .iter()
            .cloned()
            .map(|source| (source, CapabilitySourceState::Ready))
            .collect();
        for name in ["run", " leading / 工具*? ", "all"] {
            let tools = sources
                .iter()
                .map(|source| source_definition(source, name))
                .collect::<Vec<_>>();
            for (index, source) in sources.iter().enumerate() {
                let selector = ExactToolSelector::Source {
                    source_id: source.clone(),
                    name: name.into(),
                };
                assert_eq!(
                    resolve_metadata(&selector, &tools, &availability).unwrap(),
                    &tools[index]
                );
                assert!(matches!(
                    resolve_metadata(&selector, [&tools[1 - index]], &availability),
                    Err(ToolSelectionError::ExactToolAbsent { .. })
                ));
            }
        }
    }

    #[test]
    fn ordering_is_independent_of_source_and_definition_insertion_order() {
        let a = ToolSourceId::Mcp(McpServerId::new("a"));
        let b = ToolSourceId::ManagedPython("b".into());
        let make = |entries: Vec<_>| ToolSelectionDocument {
            builtin: vec!["read".into()],
            sources: entries.into_iter().collect(),
        };
        let one = make(vec![
            (a.clone(), SourceToolSelection::All),
            (b.clone(), SourceToolSelection::Exact(vec!["x".into()])),
        ]);
        let two = make(vec![
            (b.clone(), SourceToolSelection::Exact(vec!["x".into()])),
            (a.clone(), SourceToolSelection::All),
        ]);
        assert_eq!(one.selectors(), two.selectors());
        let tools = [source_definition(&a, "b"), source_definition(&a, "a")];
        let ready = [(a.clone(), CapabilitySourceState::Ready)].into();
        assert_eq!(
            resolve_source(&a, &SourceToolSelection::All, &tools, &ready),
            resolve_source(&a, &SourceToolSelection::All, tools.iter().rev(), &ready)
        );
    }
    #[test]
    fn exact_multi_entry_projects_only_the_named_tools_for_both_source_kinds() {
        for source in [
            ToolSourceId::Mcp(McpServerId::new("github")),
            ToolSourceId::ManagedPython("data-analysis".into()),
        ] {
            let tools = [
                source_definition(&source, "c"),
                source_definition(&source, "a"),
                source_definition(&source, "b"),
            ];
            let ready = [(source.clone(), CapabilitySourceState::Ready)].into();
            assert_eq!(
                resolve_source(
                    &source,
                    &SourceToolSelection::Exact(vec!["b".into(), "a".into()]),
                    &tools,
                    &ready
                ),
                SourceToolResolution::Ready {
                    selected: vec![&tools[1], &tools[2]],
                    missing_exact: vec![]
                }
            );
        }
    }
    #[test]
    fn source_names_matching_extensions_are_selected_by_provenance() {
        for source in [
            ToolSourceId::Mcp(McpServerId::new("github")),
            ToolSourceId::ManagedPython("analysis".into()),
        ] {
            for name in
                std::iter::once("todo").chain(crate::tools::native::GOAL_TOOL_NAMES.iter().copied())
            {
                let tool = source_definition(&source, name);
                let registry = ToolRegistry::from_registrations([ToolRegistration::plain(
                    tool.clone(),
                    Arc::new(Unused),
                )])
                .unwrap();
                assert_eq!(
                    crate::extensions::ExtensionToolPlaneShape::of_published_registry(&registry),
                    crate::extensions::NativeAgentExtensions::none().expected_tool_plane()
                );
                let availability = [(source.clone(), CapabilitySourceState::Ready)].into();
                for mode in [
                    SourceToolSelection::All,
                    SourceToolSelection::Exact(vec![name.into()]),
                ] {
                    let document = ToolSelectionDocument {
                        builtin: vec![],
                        sources: [(source.clone(), mode.clone())].into(),
                    };
                    let authored = toml::to_string(&document).unwrap();
                    let parsed: ToolSelectionDocument = toml::from_str(&authored).unwrap();
                    parsed.validate_spelling().unwrap();
                    let SourceToolResolution::Ready {
                        selected,
                        missing_exact,
                    } = resolve_source(&source, &mode, [&tool], &availability)
                    else {
                        panic!("ready source")
                    };
                    assert_eq!(selected, vec![&tool]);
                    assert!(missing_exact.is_empty());
                    assert_eq!(selected[0].origin.source(), Some(source.clone()));
                }
            }
            let registration =
                ToolRegistration::plain(source_definition(&source, "todo"), Arc::new(Unused));
            let policy = super::super::ToolActivationPolicy {
                sources: [(source, SourceToolSelection::All)].into(),
                ..Default::default()
            };
            let (_, selected) = super::super::tools::select_tools(
                std::slice::from_ref(&registration),
                &[],
                &policy,
            )
            .unwrap();
            assert_eq!(selected.definitions().len(), 1);
            let extension = crate::tools::native::todo_tool_registration();
            let error = super::super::tools::select_tools(&[registration], &[extension], &policy)
                .unwrap_err();
            assert!(
                error.contains("active Tool selection is invalid"),
                "{error}"
            );
            assert!(error.contains("todo"), "{error}");
        }
        assert!(
            ToolSelectionDocument {
                builtin: vec!["todo".into()],
                sources: BTreeMap::new()
            }
            .validate_spelling()
            .is_err()
        );
    }

    #[test]
    fn exact_and_all_share_the_canonical_materialized_name_universe() {
        for name in [
            "todo",
            "a b",
            " leading ",
            "a*?/[b]\\c",
            "工具",
            "all",
            "\t",
        ] {
            let wire: rmcp::model::Tool = serde_json::from_value(
                serde_json::json!({"name":name,"inputSchema":{"type":"object"}}),
            )
            .unwrap();
            let canonical = crate::tools::mcp::CanonicalMcpTool::try_from(wire).unwrap();
            for source in [
                ToolSourceId::Mcp(McpServerId::new("github")),
                ToolSourceId::ManagedPython("analysis".into()),
            ] {
                let tool = source_definition(&source, &canonical.name);
                ToolRegistry::from_registrations([ToolRegistration::plain(
                    tool.clone(),
                    Arc::new(Unused),
                )])
                .unwrap();
                let exact = SourceToolSelection::Exact(vec![canonical.name.clone()]);
                let document = ToolSelectionDocument {
                    builtin: vec![],
                    sources: [(source.clone(), exact.clone())].into(),
                };
                let parsed: ToolSelectionDocument =
                    toml::from_str(&toml::to_string(&document).unwrap()).unwrap();
                parsed.validate_spelling().unwrap();
                let availability = [(source.clone(), CapabilitySourceState::Ready)].into();
                for mode in [SourceToolSelection::All, exact] {
                    let SourceToolResolution::Ready {
                        selected,
                        missing_exact,
                    } = resolve_source(&source, &mode, [&tool], &availability)
                    else {
                        panic!("ready")
                    };
                    assert_eq!(selected, vec![&tool]);
                    assert!(missing_exact.is_empty());
                }
            }
        }
        let empty: rmcp::model::Tool =
            serde_json::from_value(serde_json::json!({"name":"","inputSchema":{"type":"object"}}))
                .unwrap();
        assert!(crate::tools::mcp::CanonicalMcpTool::try_from(empty).is_err());
    }
}
