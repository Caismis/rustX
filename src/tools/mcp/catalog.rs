//! A finite complete management inventory on one native connection owner.
//! This is discovery only: no tools/call or resource-content shortcut exists.
use super::{McpError, McpServerRuntime};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    Tools,
    Resources,
    Templates,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceDescriptor {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateDescriptor {
    pub uri_template: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogPage {
    pub capabilities: super::McpNegotiatedCapabilities,
    pub kind: CatalogKind,
    pub tools: Vec<super::CanonicalMcpTool>,
    pub resources: Vec<ResourceDescriptor>,
    pub templates: Vec<TemplateDescriptor>,
}
impl McpServerRuntime {
    #[allow(clippy::too_many_lines)] // Each native inventory kind has one explicit projection and shared bounds.
    pub(crate) async fn catalog_inventory(
        &self,
        kind: CatalogKind,
    ) -> Result<CatalogPage, McpError> {
        let _gate = self.call_gate.read().await;
        if self.closed.load(Ordering::Acquire) || self.unusable_reason().is_some() {
            return Err(McpError::Discovery("MCP connection unavailable".into()));
        }
        let mut page = CatalogPage {
            capabilities: self.capabilities,
            kind,
            tools: Vec::new(),
            resources: Vec::new(),
            templates: Vec::new(),
        };
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let mut parameters = rmcp::model::PaginatedRequestParams::default();
            parameters.cursor = cursor;
            let params = Some(parameters);
            let next;
            match kind {
                CatalogKind::Tools if self.capabilities.tools => {
                    let result = self
                        .peer
                        .list_tools(params)
                        .await
                        .map_err(|_| McpError::Discovery("MCP catalog unavailable".into()))?;
                    next = result.next_cursor;
                    let tools = result
                        .tools
                        .into_iter()
                        .map(super::CanonicalMcpTool::try_from)
                        .collect::<Result<Vec<_>, _>>()?;
                    page.tools.extend(tools);
                }
                CatalogKind::Resources if self.capabilities.resources => {
                    let result = self.peer.list_resources(params).await.map_err(|_| {
                        McpError::Discovery("MCP resource catalog unavailable".into())
                    })?;
                    next = result.next_cursor;
                    page.resources
                        .extend(
                            result
                                .resources
                                .into_iter()
                                .map(|resource| ResourceDescriptor {
                                    uri: resource.uri,
                                    name: resource.name,
                                    description: resource.description,
                                    mime_type: resource.mime_type,
                                }),
                        );
                }
                CatalogKind::Templates if self.capabilities.resources => {
                    let result = self
                        .peer
                        .list_resource_templates(params)
                        .await
                        .map_err(|_| {
                            McpError::Discovery("MCP template catalog unavailable".into())
                        })?;
                    next = result.next_cursor;
                    page.templates
                        .extend(result.resource_templates.into_iter().map(|resource| {
                            TemplateDescriptor {
                                uri_template: resource.uri_template,
                                name: resource.name,
                                description: resource.description,
                                mime_type: resource.mime_type,
                            }
                        }));
                }
                _ => {
                    return Err(McpError::Unsupported(
                        "MCP catalog capability unavailable".into(),
                    ));
                }
            }
            if page.tools.len() + page.resources.len() + page.templates.len() > 128
                || !crate::bounded_file::json_fits(&page, 65_536)
                || self.unusable_reason().is_some()
            {
                return Err(McpError::Discovery(
                    "MCP inventory exceeds bounds or connection unavailable".into(),
                ));
            }
            let Some(next) = next else { break };
            if next.len() > 4096 || seen.len() >= 32 || !seen.insert(next.clone()) {
                return Err(McpError::Discovery(
                    "MCP inventory pagination exceeds bounds or does not advance".into(),
                ));
            }
            cursor = Some(next);
        }
        // Even a peer echoing its own credentials cannot turn management
        // observations into credential readback. Recurse through JSON strings
        // without ever splicing secret bytes into JSON syntax.
        let mut value = serde_json::to_value(page)
            .map_err(|_| McpError::Discovery("invalid MCP catalog".into()))?;
        redact(&mut value, &self.credentials);
        if !crate::bounded_file::json_fits(&value, 65_536) {
            return Err(McpError::Discovery(
                "redacted MCP inventory exceeds bounds".into(),
            ));
        }
        serde_json::from_value(value).map_err(|_| McpError::Discovery("invalid MCP catalog".into()))
    }
}
fn redact(value: &mut serde_json::Value, credentials: &crate::credentials::SourceCredentials) {
    match value {
        serde_json::Value::String(text) => *text = credentials.redact(text),
        serde_json::Value::Array(values) => {
            for value in values {
                redact(value, credentials);
            }
        }
        serde_json::Value::Object(values) => {
            let old = std::mem::take(values);
            for (key, mut value) in old {
                redact(&mut value, credentials);
                values.insert(credentials.redact(&key), value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_catalog_projection_redacts_credential_values_and_keys() {
        let mut credentials: crate::credentials::SourceCredentials =
            serde_json::from_value(serde_json::json!({"headers":{"Authorization":"$KEY"}}))
                .unwrap();
        credentials.capture(crate::credentials::CredentialSnapshot::new([(
            "KEY".into(),
            "source-secret-sentinel".into(),
        )]));
        credentials.resolve().unwrap();
        let mut value = serde_json::json!({"source-secret-sentinel":{"description":"echo source-secret-sentinel","schema":["source-secret-sentinel"]}});
        super::redact(&mut value, &credentials);
        assert!(!value.to_string().contains("source-secret-sentinel"));
        assert!(value.to_string().contains("redacted"));
    }
}
