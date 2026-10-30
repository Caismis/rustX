//! The canonical MCP tool surface: [`CanonicalMcpTool`], the
//! [`McpToolExecutor`] bound to a stable connection, catalog-to-registry
//! translation, and the stable MCP tool identities.

use std::sync::Arc;
use std::time::Instant;

use sha2::Digest;

use crate::runtime::identity::{McpServerId, ToolId};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::types::{
    ToolDefinition, ToolInvocation, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy,
};

use super::generation::McpRuntimeBinding;
use super::invocation::McpCallIdentity;
use super::result::{failed_mcp, mcp_empty_terminal};
use super::runtime::McpServerRuntime;
use super::{McpError, bound_error, connection, identity};

/// A canonicalized MCP tool definition at the adapter boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalMcpTool {
    /// Remote model-facing name.
    pub name: String,
    /// Remote description, normalized to a string.
    pub description: String,
    /// Canonical JSON schema.
    pub input_schema: serde_json::Value,
    /// The declared schema of the tool's `structuredContent`, when it has one.
    pub output_schema: Option<serde_json::Value>,
}

impl TryFrom<rmcp::model::Tool> for CanonicalMcpTool {
    type Error = McpError;

    fn try_from(tool: rmcp::model::Tool) -> Result<Self, Self::Error> {
        let name = tool.name.into_owned();
        if name.is_empty() {
            return Err(McpError::Discovery("MCP tool name is empty".to_owned()));
        }
        let input_schema = serde_json::to_value(tool.input_schema)
            .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
        Ok(Self {
            name,
            description: tool
                .description
                .map_or_else(String::new, std::borrow::Cow::into_owned),
            input_schema,
            output_schema: tool
                .output_schema
                .map(|schema| serde_json::Value::Object((*schema).clone())),
        })
    }
}

/// One canonical executor bound to the exact server runtime captured at
/// capability preparation.
pub struct McpToolExecutor {
    /// The **stable connection owner**, never one transport generation
    /// (Issue #205).
    ///
    /// The executor is long-lived: it is registered once per discovered
    /// remote tool and outlives any single transport. Binding it to the
    /// connection is what makes a replaced generation immediately usable by
    /// every already-admitted tool, and what keeps a dead transport from
    /// permanently poisoning a published capability generation.
    connection: Arc<connection::McpConnection>,
    /// The generation binding is present for coordinator-owned tools. Direct
    /// public adapter users retain the standalone runtime path and explicitly
    /// own/close that runtime themselves.
    binding: Option<McpRuntimeBinding>,
    remote_name: String,
    expected: Option<CanonicalMcpTool>,
}

impl McpToolExecutor {
    /// Creates an executor bound to one discovered remote name.
    ///
    /// The caller owns the runtime and closes it itself, so the executor's
    /// connection is *fixed*: it serves exactly this transport and never
    /// establishes a replacement.
    #[must_use]
    pub fn new(runtime: Arc<McpServerRuntime>, remote_name: String) -> Self {
        Self {
            connection: connection::McpConnection::fixed(runtime),
            binding: None,
            remote_name,
            expected: None,
        }
    }

    fn new_owned(binding: McpRuntimeBinding, expected: CanonicalMcpTool) -> Self {
        Self {
            connection: binding.connection().clone(),
            binding: Some(binding),
            remote_name: expected.name.clone(),
            expected: Some(expected),
        }
    }
}

impl ToolExecutor for McpToolExecutor {
    /// Starts one remote `tools/call`.
    ///
    /// # Why the operation *is* the settlement plane
    ///
    /// The whole physical operation — transport resolution, the bounded
    /// reconnect attempt it may perform, dispatch, progress forwarding, the
    /// cancellation path (local HTTP request termination, its release proof,
    /// and the best-effort `notifications/cancelled`), and post-frontier
    /// classification — lives inside the single operation future of
    /// [`ToolExecutionHandle::settled_by_operation`], and the settlement
    /// plane takes exclusive ownership of that future and drives it to its
    /// terminal end.
    ///
    /// [`ToolExecutionHandle::settled_by_operation`] is therefore still the
    /// honest abstraction after the Streamable HTTP request lifecycle: it is
    /// correct exactly when "the operation future returned" implies "all
    /// rustX-owned local execution for this invocation is over", and it does
    /// here. This executor spawns no task and owns no process outside that
    /// future. The one local resource a dispatched call *does* own outside
    /// of Rust's stack — the in-flight Streamable HTTP request — is
    /// terminated inside the same future, and the settlement is only
    /// reported once that request's lifecycle state says no local owner
    /// remains:
    ///
    /// - `AwaitingDispatch` — the outbound participant has not reached the
    ///   seam yet, so the release proof is **awaited** and fires when that
    ///   participant arrives and refuses, or when the generation's outbound
    ///   seam ends and it provably never can;
    /// - `DispatchOwned` / `HttpOwned` — the release proof is **awaited**
    ///   before anything is reported, and it fires only after the send
    ///   future, the POST future, or the response body was actually dropped;
    /// - `Released` — every local participant was already terminal.
    ///
    /// The first case is why the claim holds for the *whole* invocation and
    /// not merely for its HTTP half: when this future returns, no rmcp send
    /// future for this request id can still dispatch it, so
    /// `Unconfirmed` keeps its Issue #204 meaning — every rustX-owned local
    /// activity of the invocation is over, and only the remote effect
    /// remains uncertain.
    ///
    /// The invocation's admission guard is a stack local of that same
    /// future. It is deliberately *not* the whole forget point: an entry
    /// whose outbound participant has not decided yet outlives it, because
    /// forgetting such an entry is exactly what let a stale send recreate
    /// uncancelled authority. Nothing is left for a separate settlement
    /// plane to reclaim, so splitting completion from settlement would add a
    /// second ownership plane with nothing in it.
    ///
    /// The executor never chooses a canonical terminal status: it reports
    /// physical outcomes and settlement evidence, and the Agent Loop's
    /// generic lifecycle owns deadlines, cancellation intent, and the
    /// canonical `ToolResult`.
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        // The cancellation path stays inside the operation future: on
        // cancellation the executor terminates this call's own local
        // request ownership, issues the best-effort protocol cancellation,
        // keeps the response channel long enough to see whether a
        // correlated remote response exists, awaits the local release
        // proof, and returns either that proven remote outcome or its
        // honest `OutcomeUnknown`, which `settled_by_operation` surfaces as
        // unconfirmed settlement evidence.
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let started = Instant::now();
                let lease = self
                    .binding
                    .as_ref()
                    .and_then(McpRuntimeBinding::acquire_lease);
                if self.binding.is_some() && lease.is_none() {
                    return pre_dispatch_failure(
                        "MCP server runtime generation is physically retired",
                        &context,
                        started,
                    );
                }
                // Transport resolution happens strictly before this call's
                // own external-effect frontier: a dead generation is retired
                // and at most one bounded replacement attempt is made here.
                // A failure is therefore an ordinary pre-frontier failure of
                // this one call — no remote side effect was possible, and no
                // previously dispatched request is ever handed to the
                // replacement.
                let resolved = match &self.expected {
                    Some(expected) => self
                        .connection
                        .validate_tool(expected, &context)
                        .await
                        .map(|(transport, epoch)| (transport, Some(epoch))),
                    None => self
                        .connection
                        .acquire_owned(Some(&context.cancellation))
                        .await
                        .map(|transport| (transport, None)),
                };
                let (generation, catalog_epoch) = match resolved {
                    Ok(generation) => generation,
                    Err(error) => {
                        let facts = self.connection.recent_diagnostic();
                        return pre_dispatch_failure(
                            &format!(
                                "no MCP transport was available to dispatch the call, so it \
                                 never reached the server: {error}; connection history: {facts}"
                            ),
                            &context,
                            started,
                        );
                    }
                };
                generation
                    .runtime()
                    .call(
                        &self.remote_name,
                        &McpCallIdentity {
                            invocation_id: &invocation.id,
                            tool_id: &invocation.tool_id,
                            tool_name: &invocation.tool_name,
                            catalog_epoch,
                        },
                        invocation.arguments,
                        &context,
                        generation.generation(),
                    )
                    .await
            }),
            cancellation,
        )
    }

    // The executor forwards genuine remote MCP progress notifications to
    // `context.progress.report(...)`, so a configured idle-liveness window
    // is honestly informed. It fabricates no heartbeats: a server that sends
    // no progress notifications simply stays bounded by the generic hard
    // deadline.
    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::Meaningful
    }
}

/// Converts one server catalog into canonical registry entries.
#[must_use]
pub fn definitions(
    server_id: &McpServerId,
    policy: ToolInvocationPolicy,
    runtime: &Arc<McpServerRuntime>,
    tools: Vec<CanonicalMcpTool>,
) -> Vec<(ToolDefinition, Arc<dyn ToolExecutor>)> {
    // One fixed connection is shared by the whole catalog: the caller owns
    // this runtime and closes it, so no generation is ever replaced here.
    let connection = connection::McpConnection::fixed(runtime.clone());
    tools
        .into_iter()
        .map(|tool| {
            let id = ToolId::new(mcp_tool_id(server_id, &tool.name));
            let definition = ToolDefinition {
                id,
                name: tool.name.clone(),
                description: tool.description,
                input_schema: tool.input_schema,
                execution_policy: policy.execution,
                concurrency_policy: policy.concurrency,
                approval_policy: policy.approval,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Mcp {
                    server_id: server_id.clone(),
                },
            };
            (
                definition,
                Arc::new(McpToolExecutor {
                    connection: connection.clone(),
                    binding: None,
                    remote_name: tool.name,
                    expected: None,
                }) as Arc<dyn ToolExecutor>,
            )
        })
        .collect()
}

/// Converts a server catalog into registrations bound to a prepared MCP
/// generation. The generation owner remains with the candidate or published
/// capability state; each executor acquires a short explicit execution lease
/// when it runs.
pub(crate) fn definitions_owned(
    server_id: &McpServerId,
    policy: ToolInvocationPolicy,
    binding: &McpRuntimeBinding,
    tools: Vec<CanonicalMcpTool>,
) -> Vec<(ToolDefinition, Arc<dyn ToolExecutor>)> {
    tools
        .into_iter()
        .map(|tool| {
            let executor = Arc::new(McpToolExecutor::new_owned(binding.clone(), tool.clone()));
            let id = ToolId::new(mcp_tool_id(server_id, &tool.name));
            let definition = ToolDefinition {
                id,
                name: tool.name.clone(),
                description: tool.description,
                input_schema: tool.input_schema,
                execution_policy: policy.execution,
                concurrency_policy: policy.concurrency,
                approval_policy: policy.approval,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Mcp {
                    server_id: server_id.clone(),
                },
            };
            (definition, executor as Arc<dyn ToolExecutor>)
        })
        .collect()
}

/// Generates a stable, length-prefixed MCP logical tool identity.
#[must_use]
pub fn mcp_tool_id(server_id: &McpServerId, remote_name: &str) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"rustx:mcp-tool-id:v1\0");
    append_length_prefixed(&mut bytes, server_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, remote_name.as_bytes());
    let digest = sha2::Sha256::digest(bytes);
    format!("mcp:sha256:{}", hex_digest(&digest))
}

fn append_length_prefixed(target: &mut Vec<u8>, bytes: &[u8]) {
    target.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    target.extend_from_slice(bytes);
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

/// Physical MCP-compatible binding key, derived from typed source provenance at
/// the materialization boundary. Python remains owned by its preparation store.
#[must_use]
pub fn source_server_id(source: &crate::capabilities::ToolSourceId) -> McpServerId {
    match source {
        crate::capabilities::ToolSourceId::Mcp(id) => id.clone(),
        crate::capabilities::ToolSourceId::ManagedPython(package) => {
            crate::tools::python::python_server_id(package)
        }
    }
}

/// Freeze the canonical materializer identity without making callers interpret
/// a transport namespace as capability provenance.
#[must_use]
pub fn source_tool_identity(
    source: &crate::capabilities::ToolSourceId,
    definition: &ToolDefinition,
) -> crate::runtime::identity::SourceToolIdentity {
    identity::mcp_tool_identity(
        &source_server_id(source),
        &definition.name,
        &definition.description,
        &definition.input_schema,
        definition.execution_policy,
        definition.concurrency_policy,
        definition.approval_policy,
    )
}

/// Publish ordinary Tools with the identity of their native source owner.
pub(crate) fn definitions_owned_for_source(
    source: &crate::capabilities::ToolSourceId,
    server_id: &McpServerId,
    policy: ToolInvocationPolicy,
    binding: &McpRuntimeBinding,
    tools: Vec<CanonicalMcpTool>,
) -> Vec<(ToolDefinition, Arc<dyn ToolExecutor>)> {
    definitions_owned(server_id, policy, binding, tools)
        .into_iter()
        .map(|(mut definition, executor)| {
            definition.origin = match source {
                crate::capabilities::ToolSourceId::Mcp(id) => ToolOrigin::Mcp {
                    server_id: id.clone(),
                },
                crate::capabilities::ToolSourceId::ManagedPython(package) => {
                    ToolOrigin::ManagedPython {
                        package: package.clone(),
                    }
                }
            };
            (definition, executor)
        })
        .collect()
}

/// No tool effect exists while resolving and validating its transport. Once
/// cancellation wins there, report confirmed cancellation to the generic Tool
/// lifecycle so its deadline/cause arbitration remains authoritative.
fn pre_dispatch_failure(
    message: &str,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> crate::tools::types::ToolExecutionResult {
    if context.cancellation.is_cancelled() {
        mcp_empty_terminal(
            crate::tools::types::ToolExecutionStatus::Cancelled {
                reason: context.cancellation.reason(),
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            },
            context,
            started,
        )
    } else {
        failed_mcp(message, context, started)
    }
}

#[cfg(test)]
mod tests {
    use super::mcp_tool_id;
    use crate::runtime::identity::McpServerId;

    #[test]
    fn mcp_tool_ids_are_length_prefixed_and_stable() {
        let first = mcp_tool_id(&McpServerId::new("a"), "b");
        assert_eq!(first, mcp_tool_id(&McpServerId::new("a"), "b"));
        assert_ne!(first, mcp_tool_id(&McpServerId::new("ab"), ""));
        assert!(first.starts_with("mcp:sha256:"));
    }
}
