//! Schema declarations cannot authorize host I/O or disclose resources to a
//! provider. The registry admits only the resource document its owner supplied.

use std::sync::Arc;

use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelEvent, ModelProtocol,
    OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter,
};
use rustx::runtime::identity::{McpServerId, ToolCallId, ToolId};
use rustx::tools::executor::{
    PreflightOutcome, ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistry,
};
use rustx::tools::types::{
    ToolCall, ToolDefinition, ToolExecutionPolicy, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};
use serde_json::{Value, json};

struct UnusedExecutor;

impl ToolExecutor for UnusedExecutor {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        panic!("the schema/provider boundary never executes the tool")
    }

    fn progress_capability(&self) -> rustx::tools::deadline::ToolProgressCapability {
        rustx::tools::deadline::ToolProgressCapability::None
    }
}

fn declaration(schema: &Value) -> ToolDefinition {
    let remote: rmcp::model::Tool = serde_json::from_value(json!({
        "name": "remote_schema", "description": "An externally declared tool",
        "inputSchema": schema,
    }))
    .expect("MCP declaration");
    let canonical = rustx::tools::mcp::CanonicalMcpTool::try_from(remote).unwrap();
    ToolDefinition {
        id: ToolId::new("remote-schema"),
        name: canonical.name,
        description: canonical.description,
        input_schema: canonical.input_schema,
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Mcp {
            server_id: McpServerId::new("remote"),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_schema_references_cannot_disclose_host_resources_to_any_provider() {
    const PRIVATE_MARKER: &str = "local-content-must-not-leave-schema-boundary";
    const OWNED_MARKER: &str = "owner-supplied-schema-resource";
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.json");
    let private = json!({"x-review-marker": PRIVATE_MARKER});
    std::fs::write(&path, private.to_string()).unwrap();
    let file_uri = url::Url::from_file_path(&path).unwrap().to_string();
    let resource_server = crate::common::FixtureServer::start(move |_, _| {
        crate::common::FixtureReply::body(
            200,
            "OK",
            "application/json",
            private.to_string().into_bytes(),
        )
    })
    .await;
    let mut registry = ToolRegistry::new();
    for uri in [&file_uri, &resource_server.url("/private.json")] {
        let result = registry.register(
            declaration(&json!({
                "$id": "https://schema.example.invalid/tool", "type": "object", "$ref": uri,
            })),
            Arc::new(UnusedExecutor),
        );
        assert!(
            result.is_err(),
            "a declaration cannot authorize reading {uri}"
        );
        assert!(registry.is_empty(), "failed registration is atomic");
    }
    assert_eq!(resource_server.attempt_count(), 0, "no schema HTTP request");

    // A URI is an identity, not permission to read it. The owner can supply
    // the resource in the declaration, even when its ID names an existing file.
    registry
        .register(
            declaration(&json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$id": "https://schema.example.invalid/tool", "type": "object",
                "$ref": file_uri,
                "$defs": {"owned": {
                    "$id": file_uri, "type": "object", "x-owner-marker": OWNED_MARKER,
                    "properties": {"command": {"type": "string", "enum": ["status"]}},
                    "required": ["command"], "additionalProperties": false,
                }},
            })),
            Arc::new(UnusedExecutor),
        )
        .expect("owner-supplied resource registers without host I/O");
    for (command, accepted) in [("status", true), ("mutate", false)] {
        let prepared = registry
            .preflight(&ToolCall {
                id: ToolCallId::new("check-schema"),
                tool_id: ToolId::new("remote-schema"),
                name: "remote_schema".into(),
                arguments: json!({"arguments": {"command": command}}),
            })
            .unwrap();
        assert_eq!(matches!(prepared, PreflightOutcome::Ready(_)), accepted);
    }

    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        let provider = crate::common::FixtureServer::start(move |_, _| match protocol {
            ModelProtocol::OpenAiChatCompletions => {
                crate::common::sse_fixture("openai_chat", "plain_text.sse")
            }
            ModelProtocol::OpenAiResponses => {
                crate::common::sse_fixture("openai_responses", "plain_text.sse")
            }
            ModelProtocol::AnthropicMessages => crate::common::sse_fixture("anthropic", "text.sse"),
        })
        .await;
        let adapter: Box<dyn ModelAdapter> = match protocol {
            ModelProtocol::OpenAiChatCompletions => Box::new(OpenAiChatCompletionsAdapter::new(
                OpenAiAdapterConfig::new("fixture-key", provider.url("/v1")),
            )),
            ModelProtocol::OpenAiResponses => Box::new(OpenAiResponsesAdapter::new(
                OpenAiAdapterConfig::new("fixture-key", provider.url("/v1")),
            )),
            ModelProtocol::AnthropicMessages => Box::new(AnthropicMessagesAdapter::new(
                AnthropicAdapterConfig::new("fixture-key", provider.url("")),
            )),
        };
        let mut request = crate::common::simple_request(protocol, "schema-fixture", "inspect");
        request.tools = registry.model_definitions();
        let events = crate::common::collect_events(adapter.as_ref(), request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        assert_eq!(provider.attempt_count(), 1);
        let body = provider.request_body(0);
        assert!(
            !body.contains(PRIVATE_MARKER),
            "host content never reaches {protocol:?}"
        );
        assert!(
            body.contains(OWNED_MARKER),
            "the supplied schema reaches {protocol:?}"
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        let schema = match protocol {
            ModelProtocol::OpenAiChatCompletions => &body["tools"][0]["function"]["parameters"],
            ModelProtocol::OpenAiResponses => &body["tools"][0]["parameters"],
            ModelProtocol::AnthropicMessages => &body["tools"][0]["input_schema"],
        };
        let validator = jsonschema::Validator::new(schema).expect("closed model schema");
        assert!(validator.is_valid(&json!({"arguments": {"command": "status"}})));
        assert!(!validator.is_valid(&json!({"arguments": {"command": "mutate"}})));
    }
    assert_eq!(resource_server.attempt_count(), 0);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        json!({"x-review-marker": PRIVATE_MARKER}).to_string()
    );
}
