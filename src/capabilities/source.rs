//! Canonical source identity above the two native materialization owners.
#[cfg(test)]
use crate::runtime::identity::McpServerId;

pub use rustx_types::capabilities::source::ToolSourceId;

/// Finite source demand admitted by the composition owner for one candidate.
/// The Python catalog carries discovery/path authority; selecting a missing
/// identity cannot manufacture a package or admitted materialization demand.
#[derive(Debug, Clone, Default)]
pub struct ToolSourceDemand {
    pub sources: std::collections::BTreeSet<ToolSourceId>,
    pub managed_python: crate::runtime::resources::ManagedPythonCatalog,
}
impl ToolSourceDemand {
    #[must_use]
    pub fn new(
        sources: impl IntoIterator<Item = ToolSourceId>,
        managed_python: crate::runtime::resources::ManagedPythonCatalog,
    ) -> Self {
        Self {
            sources: sources.into_iter().collect(),
            managed_python,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_mcp_identity_and_selection_have_the_same_domain() {
        for name in [
            "github",
            "Team / 工具?*[]",
            ".hidden",
            " leading ",
            " ",
            "other:namespace",
            "\t",
            "",
        ] {
            let config: crate::local_runtime::config::CurrentRuntimeConfig =
                serde_json::from_value(serde_json::json!({
                    "agent": {"model": {"model":"local/test"}},
                    "mcpServers": {name: {"command":"unused"}}
                }))
                .unwrap();
            let bindings = config.mcp_bindings();
            let source = ToolSourceId::try_from(name.to_owned());
            assert_eq!(bindings.is_ok(), source.is_ok(), "{name:?}");
            if let Ok(source) = source {
                assert_eq!(source, ToolSourceId::Mcp(McpServerId::new(name)));
                let document = crate::capabilities::selection::ToolSelectionDocument {
                    builtin: vec![],
                    sources: [(
                        source.clone(),
                        crate::capabilities::selection::SourceToolSelection::All,
                    )]
                    .into(),
                };
                let parsed: crate::capabilities::selection::ToolSelectionDocument =
                    toml::from_str(&toml::to_string(&document).unwrap()).unwrap();
                assert_eq!(parsed, document);
            }
        }
        for name in ["python:analysis", "python:", "python:bad/name"] {
            let config: crate::local_runtime::config::CurrentRuntimeConfig =
                serde_json::from_value(serde_json::json!({
                    "agent": {"model": {"model":"local/test"}}, "mcpServers": {name: {"command":"unused"}}
                }))
                .unwrap();
            assert!(config.mcp_bindings().is_err());
        }
        assert_eq!(
            ToolSourceId::try_from("python:analysis".to_owned()).unwrap(),
            ToolSourceId::ManagedPython("analysis".into())
        );
        for name in ["", "python:", "python:bad/name", "python:bad name"] {
            assert!(ToolSourceId::try_from(name.to_owned()).is_err(), "{name:?}");
        }
    }
}
