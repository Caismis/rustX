//! Live catalog validation belongs to one stable, conversation-owned connection.

use super::{McpConnection, McpConnectionGeneration};
use crate::tools::mcp::{CanonicalMcpTool, McpError};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct ConnectionCatalog {
    pub(super) discovery: tokio::sync::Mutex<()>,
    completed: Mutex<Option<CatalogOutcome>>,
    #[cfg(test)]
    pub(super) validated_pause:
        Mutex<Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>>,
    #[cfg(test)]
    pub(super) before_validate_pause:
        Mutex<Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>>,
}

struct CatalogOutcome {
    generation: u64,
    epoch: u64,
    tools: Result<Vec<CanonicalMcpTool>, String>,
}

impl McpConnection {
    pub(crate) fn catalog_epoch(&self) -> u64 {
        self.reconnect
            .as_ref()
            .map_or(0, |authority| authority.invalidation.epoch(&self.server_id))
    }
    /// A completed live observation, fenced by the current invalidation epoch.
    pub(crate) fn catalog_snapshot(&self) -> Option<(u64, Result<Vec<CanonicalMcpTool>, String>)> {
        let completed = self.catalog.completed.lock().expect("MCP catalog lock");
        let outcome = completed.as_ref()?;
        let current_epoch = self.catalog_epoch();
        match &outcome.tools {
            Ok(tools) if current_epoch == outcome.epoch => Some((outcome.epoch, Ok(tools.clone()))),
            Ok(_) => Some((
                current_epoch,
                Err("MCP tool catalog changed after discovery".into()),
            )),
            // An invalidation can revoke successful metadata, but cannot
            // turn a completed failure back into an in-flight initializer.
            Err(error) => Some((current_epoch, Err(error.clone()))),
        }
    }

    /// Coalesces concurrent discovery on this connection. A replacement
    /// transport or an observed invalidation must perform a real full list.
    pub(crate) async fn discover_catalog(&self) -> Result<Vec<CanonicalMcpTool>, McpError> {
        self.discover_catalog_owned(None).await
    }

    pub(super) async fn discover_catalog_owned(
        &self,
        context: Option<&crate::tools::executor::ToolExecutionContext<'_>>,
    ) -> Result<Vec<CanonicalMcpTool>, McpError> {
        let _discovery = match context {
            Some(context) => tokio::select! {
                biased;
                () = context.cancellation.cancelled() => return Err(McpError::Discovery("MCP catalog validation cancelled".into())),
                guard = self.catalog.discovery.lock() => guard,
            },
            None => self.catalog.discovery.lock().await,
        };
        let transport = match self
            .acquire_owned(context.map(|context| &context.cancellation))
            .await
        {
            Ok(transport) => transport,
            Err(error) => {
                if !context.is_some_and(|context| context.cancellation.is_cancelled()) {
                    *self.catalog.completed.lock().expect("MCP catalog lock") =
                        Some(CatalogOutcome {
                            generation: 0,
                            epoch: self.catalog_epoch(),
                            tools: Err(error.to_string()),
                        });
                }
                return Err(error);
            }
        };
        let epoch = transport.runtime().change_epoch();
        if let Some(tools) = self
            .catalog
            .completed
            .lock()
            .expect("MCP catalog lock")
            .as_ref()
            .filter(|outcome| {
                outcome.generation == transport.generation() && outcome.epoch == epoch
            })
            .map(|outcome| outcome.tools.clone())
        {
            return tools.map_err(McpError::Discovery);
        }
        let result = self.fetch_catalog(&transport, context).await;
        let result = result.and_then(|tools| {
            self.validate_catalog(&tools)?;
            let invalidation = transport.runtime().invalidation.lock();
            if invalidation.epoch(&self.server_id) != epoch {
                return Err(McpError::Discovery(
                    "MCP tool catalog changed during discovery".into(),
                ));
            }
            Ok(tools)
        });
        // One invocation's cancellation is not a terminal directory outcome
        // for other callers. Server outcomes remain authoritative for this
        // epoch until invalidation, replacement, or an explicit fresh candidate.
        if !context.is_some_and(|context| context.cancellation.is_cancelled()) {
            *self.catalog.completed.lock().expect("MCP catalog lock") = Some(CatalogOutcome {
                generation: transport.generation(),
                epoch,
                tools: result.as_ref().cloned().map_err(ToString::to_string),
            });
            if result.is_ok()
                && let Some(authority) = &self.reconnect
            {
                authority.invalidation.successful_catalog_discovery();
            }
        }
        result
    }

    async fn fetch_catalog(
        &self,
        transport: &McpConnectionGeneration,
        context: Option<&crate::tools::executor::ToolExecutionContext<'_>>,
    ) -> Result<Vec<CanonicalMcpTool>, McpError> {
        match (context, &self.reconnect) {
            (Some(context), _) => {
                transport
                    .runtime()
                    .list_tools_for_invocation(context, transport.generation())
                    .await
            }
            (None, Some(authority)) => tokio::select! {
                biased;
                () = authority.cancellation.cancelled() => Err(McpError::Discovery("MCP discovery cancelled".into())),
                result = transport.runtime().list_tools() => result,
            },
            (None, None) => transport.runtime().list_tools().await,
        }
    }

    fn validate_catalog(&self, tools: &[CanonicalMcpTool]) -> Result<(), McpError> {
        if !valid_catalog(tools)
            || self.reconnect.as_ref().is_some_and(|authority| {
                tools.iter().any(|tool| {
                    crate::tools::schema::validate_execution_metadata_contract(
                        authority.binding.policy.execution,
                        &tool.input_schema,
                    )
                    .is_err()
                })
            })
        {
            return Err(McpError::Discovery(
                "invalid or duplicate MCP catalog declarations".into(),
            ));
        }
        Ok(())
    }

    /// Verifies the exact frozen declaration against the current live
    /// transport before a tools/call exists. The returned epoch is also
    /// checked at the request's enqueue frontier under the invalidation lock.
    pub(crate) async fn validate_tool(
        &self,
        expected: &CanonicalMcpTool,
        context: &crate::tools::executor::ToolExecutionContext<'_>,
    ) -> Result<(Arc<McpConnectionGeneration>, u64), McpError> {
        self.discover_catalog_owned(Some(context)).await?;
        #[cfg(test)]
        {
            let pause = self.catalog.before_validate_pause.lock().unwrap().take();
            if let Some(pause) = pause {
                pause.park().await;
            }
        }
        let transport = self.acquire_owned(Some(&context.cancellation)).await?;
        let epoch = transport.runtime().change_epoch();
        let validated = self
            .catalog
            .completed
            .lock()
            .expect("MCP catalog lock")
            .as_ref()
            .is_some_and(|outcome| {
                outcome.generation == transport.generation()
                    && outcome.epoch == epoch
                    && outcome
                        .tools
                        .as_ref()
                        .is_ok_and(|tools| tools.iter().any(|tool| tool == expected))
            });
        if !validated {
            // The newly discovered catalog remains valid for native publication.
            // A mismatch revokes this frozen invocation, not the live observation.
            return Err(McpError::Discovery(format!(
                "MCP tool '{}' differs from the current live catalog; the frozen invocation cannot dispatch",
                expected.name
            )));
        }
        #[cfg(test)]
        {
            let pause = self.catalog.validated_pause.lock().unwrap().take();
            if let Some(pause) = pause {
                pause.park().await;
            }
        }
        Ok((transport, epoch))
    }
}

fn valid_catalog(tools: &[CanonicalMcpTool]) -> bool {
    let mut names = std::collections::BTreeSet::new();
    tools.iter().all(|tool| {
        names.insert(&tool.name)
            && !tool.name.is_empty()
            && crate::tools::executor::validate_model_facing_name(&tool.name).is_ok()
            && crate::tools::schema::validate_canonical_schema(&tool.input_schema).is_ok()
    })
}
