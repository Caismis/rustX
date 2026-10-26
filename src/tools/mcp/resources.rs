//! Standard read-only MCP Resource requests on the existing invocation owner.

use std::time::Instant;

use base64::Engine as _;
use rmcp::model::{
    CallToolResult, ClientRequest, ListResourceTemplatesRequest, ListResourcesRequest,
    PaginatedRequestParams, ReadResourceRequest, ReadResourceRequestParams, ResourceContents,
    ServerResult,
};

use crate::tools::executor::ToolExecutionContext;
use crate::tools::types::{
    ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
};

use super::outcome::{McpPhysicalOutcome, McpResponseOutcome, post_dispatch_cancellation_status};
use super::request::McpRequestOwnership;
use super::result::{failed_mcp, mcp_empty_terminal, translate_result, write_artifact};
use super::{McpConnection, McpServerRuntime, bound_error};

const MAX_CURSOR_BYTES: usize = 4096;
const MAX_URI_BYTES: usize = 8192;
const MAX_PAGE_ITEMS: usize = 256;
const MAX_METADATA_BYTES: usize = 16 * 1024;
const MAX_RESOURCE_BLOCKS: usize = 64;
const MAX_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;

/// Exactly one protocol page or resource read. No operation traverses pages or replays.
#[derive(Debug, Clone)]
pub(crate) enum McpResourceOperation {
    List { cursor: Option<String> },
    Templates { cursor: Option<String> },
    Read { uri: String },
}

impl McpResourceOperation {
    fn method(&self) -> &'static str {
        match self {
            Self::List { .. } => "resources/list",
            Self::Templates { .. } => "resources/templates/list",
            Self::Read { .. } => "resources/read",
        }
    }

    fn request(&self) -> Result<ClientRequest, &'static str> {
        match self {
            Self::List { cursor } | Self::Templates { cursor } => {
                validate_cursor(cursor.as_deref())?;
                let mut params = PaginatedRequestParams::default();
                params.cursor.clone_from(cursor);
                Ok(match self {
                    Self::List { .. } => ClientRequest::ListResourcesRequest(
                        ListResourcesRequest::with_param(params),
                    ),
                    _ => ClientRequest::ListResourceTemplatesRequest(
                        ListResourceTemplatesRequest::with_param(params),
                    ),
                })
            }
            Self::Read { uri } => {
                if uri.is_empty() || uri.len() > MAX_URI_BYTES {
                    return Err("MCP resource URI must contain 1 to 8192 bytes");
                }
                // The URI belongs exclusively to this peer. No filesystem or URL API sees it.
                Ok(ClientRequest::ReadResourceRequest(
                    ReadResourceRequest::new(ReadResourceRequestParams::new(uri.clone())),
                ))
            }
        }
    }
}

impl McpConnection {
    /// Resolves the transport once, verifies its own negotiated capability,
    /// and sends exactly one request through the shared physical request owner.
    pub(crate) async fn resource_request(
        &self,
        operation: McpResourceOperation,
        context: &ToolExecutionContext<'_>,
    ) -> ToolExecutionResult {
        let started = Instant::now();
        let request = match operation.request() {
            Ok(request) => request,
            Err(error) => return failed_mcp(error, context, started),
        };
        if context.cancellation.is_cancelled() {
            return cancelled(context, started);
        }
        let transport = match self.validate_resources(context).await {
            Ok(transport) => transport,
            Err(_) if context.cancellation.is_cancelled() => return cancelled(context, started),
            Err(error) => return failed_mcp(&error.to_string(), context, started),
        };
        let runtime = transport.runtime();
        let outcome = runtime
            .dispatch_owned_request(
                request,
                McpRequestOwnership::resource(operation.method()),
                context,
                transport.generation(),
                started,
            )
            .await;
        let mut result = runtime
            .resource_outcome(
                outcome,
                &operation,
                context,
                transport.generation(),
                started,
            )
            .await;
        match &mut result.status {
            ToolExecutionStatus::Failed { error }
            | ToolExecutionStatus::OutcomeUnknown { detail: error } => {
                *error = runtime.credentials.redact(error);
            }
            _ => {}
        }
        result
    }
}

impl McpServerRuntime {
    async fn resource_outcome(
        &self,
        outcome: McpPhysicalOutcome,
        operation: &McpResourceOperation,
        context: &ToolExecutionContext<'_>,
        generation: u64,
        started: Instant,
    ) -> ToolExecutionResult {
        match outcome {
            McpPhysicalOutcome::RefusedByCancellation { .. } => cancelled(context, started),
            McpPhysicalOutcome::Refused { result, .. } => *result,
            McpPhysicalOutcome::CancelledPostFrontier { requested, .. } => mcp_empty_terminal(
                post_dispatch_cancellation_status(requested, &self.generation_tag(generation)),
                context,
                started,
            ),
            McpPhysicalOutcome::Observed { response, .. } => match response {
                McpResponseOutcome::Answered(answered) => match *answered {
                    Ok(response) => {
                        translate_resource_response(response, operation, context, started)
                    }
                    Err(rmcp::service::ServiceError::McpError(error)) => {
                        let mut result = failed_mcp(&error.to_string(), context, started);
                        let data = error
                            .data
                            .filter(|value| bounded_json(value, MAX_METADATA_BYTES).is_ok());
                        result.content.push(ToolResultContent::Json { value: serde_json::json!({
                            "error": {"code": error.code, "message": bound_error(&self.credentials.redact(&error.message)), "data": data}
                        }) });
                        result
                    }
                    Err(error) => {
                        self.resource_unknown(&error.to_string(), context, generation, started)
                            .await
                    }
                },
                McpResponseOutcome::ChannelEnded => {
                    self.resource_unknown(
                        "response channel ended without a correlated reply",
                        context,
                        generation,
                        started,
                    )
                    .await
                }
            },
        }
    }

    async fn resource_unknown(
        &self,
        error: &str,
        context: &ToolExecutionContext<'_>,
        generation: u64,
        started: Instant,
    ) -> ToolExecutionResult {
        let detail = self.poisoned_protocol_violation().await.unwrap_or_else(|| {
            format!(
                "MCP resource request produced no correlated remote response ({}): {error}",
                self.generation_tag(generation)
            )
        });
        mcp_empty_terminal(
            ToolExecutionStatus::OutcomeUnknown {
                detail: bound_error(&detail),
            },
            context,
            started,
        )
    }
}

fn cancelled(context: &ToolExecutionContext<'_>, started: Instant) -> ToolExecutionResult {
    mcp_empty_terminal(
        ToolExecutionStatus::Cancelled {
            reason: context.cancellation.reason(),
            phase: ToolCancellationPhase::DuringExecution,
        },
        context,
        started,
    )
}

fn validate_cursor(cursor: Option<&str>) -> Result<(), &'static str> {
    if cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES) {
        Err("MCP resource cursor must contain 1 to 4096 bytes")
    } else {
        Ok(())
    }
}

fn translate_resource_response(
    response: ServerResult,
    operation: &McpResourceOperation,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    let result = match (operation, response) {
        (McpResourceOperation::List { cursor }, ServerResult::ListResourcesResult(page)) => {
            resource_page(
                page.resources.len(),
                cursor.as_deref(),
                page.next_cursor.as_deref(),
                &page,
            )
        }
        (
            McpResourceOperation::Templates { cursor },
            ServerResult::ListResourceTemplatesResult(page),
        ) => resource_page(
            page.resource_templates.len(),
            cursor.as_deref(),
            page.next_cursor.as_deref(),
            &page,
        ),
        (McpResourceOperation::Read { .. }, ServerResult::ReadResourceResult(page)) => {
            return match translate_resource_contents(page.contents, context, started) {
                Ok(result) => result,
                Err(error) => failed_mcp(&error, context, started),
            };
        }
        _ => Err("unexpected MCP resource response".to_owned()),
    };
    match result {
        Ok(value) => {
            // Pages are already fully bounded. Preserve structured pagination
            // (including its opaque cursor) rather than truncating it as text.
            let mut result = mcp_empty_terminal(ToolExecutionStatus::Success, context, started);
            result.content.push(ToolResultContent::Json { value });
            result
        }
        Err(error) => failed_mcp(&error, context, started),
    }
}

fn resource_page<T: serde::Serialize>(
    count: usize,
    cursor: Option<&str>,
    next_cursor: Option<&str>,
    page: &T,
) -> Result<serde_json::Value, String> {
    if count > MAX_PAGE_ITEMS {
        return Err("MCP resource page exceeds the 256 descriptor limit".into());
    }
    validate_cursor(next_cursor).map_err(str::to_owned)?;
    if next_cursor.is_some() && next_cursor == cursor {
        return Err("MCP resource page returned the request cursor again".into());
    }
    bounded_json(page, crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES)?;
    serde_json::to_value(page).map_err(|error| error.to_string())
}

/// Count serialized bytes without allocating an unbounded second payload.
fn bounded_json<T: serde::Serialize>(value: &T, limit: usize) -> Result<(), String> {
    struct Counter {
        remaining: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.remaining = self.remaining.checked_sub(bytes.len()).ok_or_else(|| {
                std::io::Error::other("MCP resource metadata or page exceeds its byte limit")
            })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { remaining: limit }, value).map_err(|error| error.to_string())
}

fn translate_resource_contents(
    contents: Vec<ResourceContents>,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> Result<ToolExecutionResult, String> {
    if contents.len() > MAX_RESOURCE_BLOCKS {
        return Err("MCP resource read exceeds the 64 content block limit".into());
    }
    let mut remaining = MAX_RESOURCE_BYTES;
    let mut blocks = Vec::new();
    let mut artifacts = Vec::new();
    for content in contents {
        let (uri, mime_type, meta, text, blob) = match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                meta,
                text,
            } => (uri, mime_type, meta, Some(text), None),
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                meta,
                blob,
            } => (uri, mime_type, meta, None, Some(blob)),
            _ => return Err("unsupported MCP resource content variant".into()),
        };
        if uri.len() > MAX_URI_BYTES {
            return Err("MCP resource content URI exceeds its byte limit".into());
        }
        bounded_json(&(&mime_type, &meta), MAX_METADATA_BYTES)?;
        let mut block = serde_json::json!({"uri": uri, "mimeType": mime_type, "_meta": meta});
        if let Some(text) = text {
            remaining = remaining
                .checked_sub(text.len())
                .ok_or("MCP resource contents exceed the aggregate byte limit")?;
            block["text"] = text.into();
        }
        if let Some(blob) = blob {
            if blob.len() > MAX_BLOB_BYTES.div_ceil(3) * 4 {
                return Err("MCP resource blob exceeds the decoded byte limit".into());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(blob)
                .map_err(|error| format!("invalid MCP resource blob: {error}"))?;
            if bytes.len() > MAX_BLOB_BYTES {
                return Err("MCP resource blob exceeds the decoded byte limit".into());
            }
            remaining = remaining
                .checked_sub(bytes.len())
                .ok_or("MCP resource contents exceed the aggregate byte limit")?;
            let artifact_id = write_artifact(context.artifacts, &bytes)?;
            block["artifactId"] =
                serde_json::to_value(&artifact_id).map_err(|error| error.to_string())?;
            block["size"] = bytes.len().into();
            artifacts.push(crate::message::content::FileReference {
                artifact_id,
                name: None,
                mime_type,
                description: Some("MCP resource content".into()),
            });
        }
        blocks.push(block);
    }
    let mut call = CallToolResult::success(Vec::new());
    call.structured_content = Some(serde_json::json!({"contents": blocks}));
    let mut result = translate_result(call, context, started);
    result.artifacts = artifacts;
    Ok(result)
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod tests;
