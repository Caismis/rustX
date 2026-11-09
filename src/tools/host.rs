//! Application-executed capabilities through the ordinary Tool Runtime.
//! No transport, HTTP or control-plane type enters this contract.
use super::{
    ToolDefinition, ToolExecutionContext, ToolExecutionHandle, ToolExecutionResult,
    ToolExecutionStatus, ToolExecutor, ToolInvocation, ToolOrigin, ToolRegistry,
};
use crate::runtime::{
    cancellation::ExecutionCancellation,
    identity::{AttemptId, ConversationId},
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

/// Immutable declarations owned by native Session-node metadata. Omitted
/// namespaces are unqualified; explicit namespaces are nonempty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostToolDefinition {
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub trait HostToolDispatcher: Send + Sync {
    fn call<'a>(
        &'a self,
        conversation: &'a ConversationId,
        attempt: &'a AttemptId,
        definition: &'a HostToolDefinition,
        invocation: ToolInvocation,
        cancellation: ExecutionCancellation,
    ) -> BoxFuture<'a, ToolExecutionResult>;
}

/// One composition binding, independent of connected clients or declarations.
#[derive(Default)]
pub struct HostToolService(OnceLock<Arc<dyn HostToolDispatcher>>);
impl std::fmt::Debug for HostToolService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostToolService").finish_non_exhaustive()
    }
}
impl HostToolService {
    pub(crate) fn bind(&self, dispatcher: Arc<dyn HostToolDispatcher>) {
        assert!(
            self.0.set(dispatcher).is_ok(),
            "one application callback binding"
        );
    }
}

impl HostToolDefinition {
    pub(crate) fn identity(&self) -> (&str, &str) {
        (self.namespace.as_deref().unwrap_or(""), &self.name)
    }
    /// Provider-neutral canonical key, using the same bounded name on every
    /// adapter. Full namespace/name remain in the native origin and callback.
    pub(crate) fn canonical_name(&self) -> String {
        let key = serde_json::to_vec(&self.identity()).expect("native tool identity");
        format!("host_{}", &format!("{:x}", Sha256::digest(key))[..56])
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        let valid_part = |v: &str| {
            v.len() <= 128
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        };
        if self.name.is_empty()
            || !valid_part(&self.name)
            || !valid_part(self.namespace.as_deref().unwrap_or(""))
            || self.namespace.as_deref() == Some("")
            || self.description.len() > 16 * 1024
            || serde_json::to_vec(&self.input_schema)
                .map_err(|e| e.to_string())?
                .len()
                > 64 * 1024
        {
            return Err("invalid Host Tool declaration bounds or identity".into());
        }
        super::schema::validate_canonical_schema(&self.input_schema).map_err(|e| e.to_string())?;
        super::executor::validate_model_facing_name(&self.name).map_err(|e| format!("{e:?}"))?;
        if super::mcp::is_resource_tool(&self.name) {
            return Err("reserved Host Tool name".into());
        }
        Ok(())
    }
}

pub(crate) fn validate_definitions(definitions: &[HostToolDefinition]) -> Result<(), String> {
    if definitions.is_empty() {
        return Ok(());
    }
    if definitions.len() > 64 {
        return Err("at most 64 Host Tools may be declared".into());
    }
    let mut identities = std::collections::BTreeSet::new();
    let builtins = super::native::definitions(
        super::native::NativeToolPolicies::default(),
        &crate::runtime::subagent::AgentCatalog::empty(),
    );
    for definition in definitions {
        definition.validate()?;
        if definition.namespace.is_none()
            && builtins
                .iter()
                .any(|(tool, _)| tool.name == definition.name)
        {
            return Err("Host Tool collides with a Builtin identity".into());
        }
        if !identities.insert(definition.identity()) {
            return Err("duplicate Host Tool identity".into());
        }
    }
    if serde_json::to_vec(definitions)
        .map_err(|e| e.to_string())?
        .len()
        > 256 * 1024
    {
        return Err("Host Tool declarations exceed aggregate limit".into());
    }
    Ok(())
}

pub(crate) fn register(
    registry: &mut ToolRegistry,
    definitions: &[HostToolDefinition],
    service: &Arc<HostToolService>,
) -> Result<(), String> {
    validate_definitions(definitions)?;
    for definition in definitions {
        if definition.namespace.is_none()
            && registry.definitions().iter().any(|existing| {
                existing.origin == ToolOrigin::Builtin && existing.name == definition.name
            })
        {
            return Err("Host Tool shadows an effective tool identity".into());
        }
        let name = definition.canonical_name();
        registry
            .register(
                ToolDefinition {
                    id: crate::runtime::identity::ToolId::new(format!("tool-{name}")),
                    name,
                    description: format!(
                        "Application tool {}{}: {}",
                        definition
                            .namespace
                            .as_deref()
                            .map_or(String::new(), |n| format!("{n}.")),
                        definition.name,
                        definition.description
                    ),
                    input_schema: definition.input_schema.clone(),
                    execution_policy: super::ToolExecutionPolicy::ForegroundOnly,
                    concurrency_policy: super::ToolConcurrencyPolicy::Parallel,
                    approval_policy: super::ToolApprovalPolicy::Never,
                    replay_policy: super::ToolReplayPolicy::Never,
                    origin: ToolOrigin::Host {
                        namespace: definition.namespace.clone(),
                        name: definition.name.clone(),
                    },
                },
                Arc::new(Executor {
                    definition: definition.clone(),
                    service: service.clone(),
                }),
            )
            .map_err(|e| format!("invalid Host Tool registration: {e:?}"))?;
    }
    Ok(())
}

struct Executor {
    definition: HostToolDefinition,
    service: Arc<HostToolService>,
}
impl ToolExecutor for Executor {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let Some(attempt) = context.attempt_id else {
                    return result(
                        ToolExecutionStatus::Failed {
                            error: "Host Tools require an explicit root Attempt".into(),
                        },
                        Vec::new(),
                        0,
                    );
                };
                let Some(dispatcher) = self.service.0.get() else {
                    return result(
                        ToolExecutionStatus::Failed {
                            error: "application callback service is unavailable".into(),
                        },
                        Vec::new(),
                        0,
                    );
                };
                dispatcher
                    .call(
                        context.conversation_id,
                        attempt,
                        &self.definition,
                        invocation,
                        context.cancellation.clone(),
                    )
                    .await
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> super::deadline::ToolProgressCapability {
        super::deadline::ToolProgressCapability::None
    }
}

pub(crate) fn result(
    status: ToolExecutionStatus,
    content: Vec<super::ToolResultContent>,
    duration_ms: u64,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status,
        content,
        duration_ms,
        exit_code: None,
        artifacts: Vec::new(),
        deliveries: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition(namespace: Option<&str>) -> HostToolDefinition {
        HostToolDefinition {
            namespace: namespace.map(str::to_owned),
            name: "lookup".into(),
            description: "lookup".into(),
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        }
    }
    #[test]
    fn distinct_namespaces_are_distinct_and_duplicates_are_rejected() {
        let a = definition(Some("erp"));
        let b = definition(Some("cad"));
        assert!(validate_definitions(&[a.clone(), b.clone()]).is_ok());
        assert!(validate_definitions(&[a.clone(), a.clone()]).is_err());
        assert_ne!(a.canonical_name(), b.canonical_name());
        let mut registry = ToolRegistry::new();
        register(&mut registry, &[a, b], &Arc::default()).unwrap();
        assert_eq!(registry.len(), 2);
        let mut reserved = definition(None);
        reserved.name = "workflow_output".into();
        assert!(validate_definitions(&[reserved]).is_err());
    }

    fn named_definition(namespace: Option<&str>, name: &str) -> HostToolDefinition {
        HostToolDefinition {
            namespace: namespace.map(str::to_owned),
            name: name.into(),
            description: "Application lookup".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}),
        }
    }
    #[test]
    fn declarations_validate_full_identity_schema_and_reserved_collisions() {
        let business = named_definition(Some("business"), "lookup");
        let inventory = named_definition(Some("inventory"), "lookup");
        validate_definitions(&[business.clone(), inventory.clone()]).unwrap();
        assert_ne!(business.canonical_name(), inventory.canonical_name());
        assert!(business.canonical_name().len() <= 64);
        assert!(validate_definitions(&[business.clone(), business]).is_err());
        for invalid in [
            named_definition(None, "read"),
            named_definition(None, "workflow_output"),
            named_definition(Some(""), "lookup"),
            named_definition(None, "bad/name"),
        ] {
            assert!(validate_definitions(&[invalid]).is_err());
        }
        let mut invalid = inventory;
        invalid.input_schema = serde_json::json!({"type":"invalid-json-schema-type"});
        assert!(validate_definitions(&[invalid]).is_err());
    }
}
