//! The ecosystem-compatible MCP configuration contract (Issue #46).
//!
//! `mcpServers` is a named map keyed by MCP server identity, spelled exactly
//! the way mainstream MCP clients spell it. This suite pins the complete
//! accepted syntax surface, every rejection, the rustX policy overlay, and
//! the determinism of the normalized runtime bindings.

use std::collections::BTreeMap;

use rustx::local_runtime::config::{LocalSessionConfig, LocalSessionConfigError};
use rustx::runtime::identity::McpServerId;
use rustx::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy};

/// Wraps an MCP configuration fragment in an otherwise minimal session.
fn session_json(fragment: &str) -> String {
    format!(
        r#"{{
            "conversationId": "conv-46",
            "agentId": "agent-46",
            "model": {{"model": "p/m"}},
            "context": {{"reserveTokens": 1024, "keepRecentTokens": 4096}},
            {fragment}
        }}"#
    )
}

fn bindings(fragment: &str) -> McpServerBindings {
    LocalSessionConfig::from_json_slice(session_json(fragment).as_bytes())
        .expect("the configuration must parse")
        .mcp_bindings()
        .expect("the configuration must normalize")
}

fn rejection(fragment: &str) -> LocalSessionConfigError {
    LocalSessionConfig::from_json_slice(session_json(fragment).as_bytes())
        .expect_err("the configuration must be rejected")
}

fn single(bindings: &McpServerBindings) -> (&McpServerId, &McpServerBinding) {
    assert_eq!(bindings.len(), 1, "exactly one binding");
    bindings.iter().next().expect("one binding")
}

// ---------------------------------------------------------------- HTTP ----

/// The canonical remote form: an explicit `type` and a `url`.
#[test]
fn canonical_http_entry_normalizes_to_the_streamable_http_transport() {
    let bindings =
        bindings(r#""mcpServers": {"exa": {"type": "http", "url": "https://mcp.exa.ai/mcp"}}"#);
    let (server_id, binding) = single(&bindings);
    assert_eq!(server_id.as_str(), "exa");
    assert_eq!(
        binding.transport,
        McpTransportConfig::StreamableHttp {
            endpoint: "https://mcp.exa.ai/mcp".to_owned(),
            headers: BTreeMap::new(),
        }
    );
}

/// The shorthand every remote MCP README uses: a bare `url`.
#[test]
fn url_only_entry_infers_the_same_http_transport() {
    let canonical =
        bindings(r#""mcpServers": {"exa": {"type": "http", "url": "https://mcp.exa.ai/mcp"}}"#);
    let inferred = bindings(r#""mcpServers": {"exa": {"url": "https://mcp.exa.ai/mcp"}}"#);
    assert_eq!(
        canonical, inferred,
        "the canonical and inferred HTTP forms normalize identically"
    );
}

/// Static HTTP headers reach the runtime transport byte-for-byte.
#[test]
fn http_headers_survive_normalization_exactly() {
    let bindings = bindings(
        r#""mcpServers": {"exa": {
            "type": "http",
            "url": "https://mcp.exa.ai/mcp",
            "headers": {"x-api-key": "secret-value", "X-Trace": "on"}
        }}"#,
    );
    let (_, binding) = single(&bindings);
    let McpTransportConfig::StreamableHttp { headers, .. } = &binding.transport else {
        panic!("HTTP transport");
    };
    assert_eq!(
        headers,
        &BTreeMap::from([
            ("X-Trace".to_owned(), "on".to_owned()),
            ("x-api-key".to_owned(), "secret-value".to_owned()),
        ])
    );
}

/// A missing endpoint is a configuration failure, never an empty URL that
/// only fails at connect time.
#[test]
fn empty_or_blank_http_url_is_rejected() {
    for url in ["", "   "] {
        let error = rejection(&format!(
            r#""mcpServers": {{"exa": {{"type": "http", "url": "{url}"}}}}"#
        ));
        assert!(
            error.to_string().contains("url must be a non-empty"),
            "unexpected error: {error}"
        );
    }
}

/// An explicit HTTP entry that also carries stdio fields is a contradiction.
#[test]
fn http_entry_with_command_fields_is_rejected() {
    for fragment in [
        r#""mcpServers": {"exa": {"type": "http", "url": "https://x", "command": "npx"}}"#,
        r#""mcpServers": {"exa": {"type": "http", "url": "https://x", "args": ["-y"]}}"#,
        r#""mcpServers": {"exa": {"type": "http", "url": "https://x", "env": {"K": "v"}}}"#,
        r#""mcpServers": {"exa": {"type": "http", "url": "https://x", "cwd": "sub"}}"#,
    ] {
        let error = rejection(fragment);
        assert!(
            error.to_string().contains("declares stdio fields"),
            "unexpected error: {error}"
        );
    }
}

/// An explicit HTTP entry with no `url` at all is rejected rather than
/// silently reinterpreted.
#[test]
fn http_entry_without_url_is_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {"type": "http"}}"#);
    assert!(
        error.to_string().contains("declares no url"),
        "unexpected error: {error}"
    );
}

// --------------------------------------------------------------- stdio ----

/// The canonical local form: an explicit `type` and a `command`.
#[test]
fn canonical_stdio_entry_normalizes_to_the_stdio_transport() {
    let bindings = bindings(
        r#""mcpServers": {"exa": {
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "exa-mcp-server"],
            "env": {"EXA_API_KEY": "key"},
            "cwd": "servers/exa"
        }}"#,
    );
    let (server_id, binding) = single(&bindings);
    assert_eq!(server_id.as_str(), "exa");
    assert_eq!(
        binding.transport,
        McpTransportConfig::Stdio {
            program: "npx".to_owned(),
            args: vec!["-y".to_owned(), "exa-mcp-server".to_owned()],
            cwd: Some(std::path::PathBuf::from("servers/exa")),
            environment: BTreeMap::from([("EXA_API_KEY".to_owned(), "key".to_owned())]),
        },
        "command/args/env/cwd survive normalization exactly"
    );
}

/// The shorthand every local MCP README uses: a bare `command`.
#[test]
fn command_only_entry_infers_the_same_stdio_transport() {
    let canonical = bindings(
        r#""mcpServers": {"exa": {"type": "stdio", "command": "npx", "args": ["-y", "mcp-remote"]}}"#,
    );
    let inferred =
        bindings(r#""mcpServers": {"exa": {"command": "npx", "args": ["-y", "mcp-remote"]}}"#);
    assert_eq!(
        canonical, inferred,
        "the canonical and inferred stdio forms normalize identically"
    );
}

/// A missing executable is a configuration failure.
#[test]
fn empty_or_blank_stdio_command_is_rejected() {
    for command in ["", "   "] {
        let error = rejection(&format!(
            r#""mcpServers": {{"exa": {{"type": "stdio", "command": "{command}"}}}}"#
        ));
        assert!(
            error.to_string().contains("command must be a non-empty"),
            "unexpected error: {error}"
        );
    }
}

/// An explicit stdio entry that also carries HTTP fields is a contradiction.
#[test]
fn stdio_entry_with_http_fields_is_rejected() {
    for fragment in [
        r#""mcpServers": {"exa": {"type": "stdio", "command": "npx", "url": "https://x"}}"#,
        r#""mcpServers": {"exa": {"type": "stdio", "command": "npx", "headers": {"k": "v"}}}"#,
    ] {
        let error = rejection(fragment);
        assert!(
            error.to_string().contains("declares http fields"),
            "unexpected error: {error}"
        );
    }
}

/// An explicit stdio entry with no `command` at all is rejected.
#[test]
fn stdio_entry_without_command_is_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {"type": "stdio"}}"#);
    assert!(
        error.to_string().contains("declares no command"),
        "unexpected error: {error}"
    );
}

// ----------------------------------------------------------- rejection ----

/// An entry carrying both transports is ambiguous, and ambiguity never
/// resolves to a guess.
#[test]
fn command_and_url_together_are_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {"url": "https://x", "command": "npx"}}"#);
    assert!(
        error.to_string().contains("declares both url and command"),
        "unexpected error: {error}"
    );
}

/// An entry carrying neither transport is incomplete.
#[test]
fn entry_without_url_or_command_is_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {}}"#);
    assert!(
        error.to_string().contains("declares neither url"),
        "unexpected error: {error}"
    );
}

/// rustX has exactly two runtime transports. The accepted `type` set is
/// exactly `http` and `stdio`: no aliases, no SSE, no WebSocket.
#[test]
fn unsupported_transport_types_are_rejected_with_the_accepted_set() {
    for transport_type in [
        "sse",
        "ws",
        "websocket",
        "streamable-http",
        "streamable_http",
    ] {
        let error = rejection(&format!(
            r#""mcpServers": {{"exa": {{"type": "{transport_type}", "url": "https://x"}}}}"#
        ));
        let message = error.to_string();
        assert!(
            matches!(error, LocalSessionConfigError::Syntax { .. }),
            "unexpected error kind for {transport_type}: {message}"
        );
        assert!(
            message.contains("unknown variant") && message.contains("expected `http` or `stdio`"),
            "the failure must name the accepted set, got: {message}"
        );
    }
}

/// A typo must fail startup rather than silently change runtime semantics.
#[test]
fn unknown_entry_fields_are_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {"url": "https://x", "timeoutMs": 5000}}"#);
    assert!(
        matches!(error, LocalSessionConfigError::Syntax { .. }),
        "unexpected error: {error}"
    );
}

/// The obsolete Issue #42 array schema is gone with no compatibility path.
#[test]
fn the_obsolete_array_schema_is_rejected() {
    let error = rejection(
        r#""mcpServers": [{"serverId": "exa", "transport": {"type": "streamable_http", "endpoint": "https://x"}}]"#,
    );
    assert!(
        matches!(error, LocalSessionConfigError::Syntax { .. }),
        "unexpected error: {error}"
    );
}

/// The obsolete redundant identity field is gone: the map key is the one
/// authoritative identity.
#[test]
fn the_obsolete_server_id_field_is_rejected() {
    let error = rejection(r#""mcpServers": {"exa": {"serverId": "exa", "url": "https://x"}}"#);
    assert!(
        matches!(error, LocalSessionConfigError::Syntax { .. }),
        "unexpected error: {error}"
    );
}

/// The obsolete nested transport object is gone.
#[test]
fn the_obsolete_nested_transport_field_is_rejected() {
    let error = rejection(
        r#""mcpServers": {"exa": {"transport": {"type": "streamable_http", "endpoint": "https://x"}}}"#,
    );
    assert!(
        matches!(error, LocalSessionConfigError::Syntax { .. }),
        "unexpected error: {error}"
    );
}

/// The obsolete policy-inside-connection coupling is gone: rustX policy lives
/// in its own keyed surface.
#[test]
fn embedded_policy_inside_a_connection_entry_is_rejected() {
    let error = rejection(
        r#""mcpServers": {"exa": {"url": "https://x", "policy": {"execution": "foreground_only"}}}"#,
    );
    assert!(
        matches!(error, LocalSessionConfigError::Syntax { .. }),
        "unexpected error: {error}"
    );
}

// ----------------------------------------------------- identity/policy ----

/// Duplicate MCP identity is structurally impossible: a JSON object cannot
/// yield two entries under one key, so no duplicate check exists anywhere.
#[test]
fn duplicate_server_identity_cannot_produce_two_bindings() {
    let bindings = bindings(
        r#""mcpServers": {"exa": {"url": "https://first"}, "exa": {"url": "https://second"}}"#,
    );
    let (server_id, binding) = single(&bindings);
    assert_eq!(server_id.as_str(), "exa");
    assert_eq!(
        binding.transport,
        McpTransportConfig::StreamableHttp {
            endpoint: "https://second".to_owned(),
            headers: BTreeMap::new(),
        }
    );
}

/// An empty map key is not a usable server identity.
#[test]
fn empty_server_identity_is_rejected() {
    let error = rejection(r#""mcpServers": {"": {"url": "https://x"}}"#);
    assert!(
        error.to_string().contains("non-empty server identities"),
        "unexpected error: {error}"
    );
}

/// A server with no policy entry gets the deterministic default policy.
#[test]
fn absent_policy_entry_uses_the_deterministic_default() {
    let bindings = bindings(r#""mcpServers": {"exa": {"url": "https://x"}}"#);
    let (_, binding) = single(&bindings);
    assert_eq!(
        binding.policy.execution,
        ToolExecutionPolicy::ForegroundOnly
    );
    assert_eq!(
        binding.policy.concurrency,
        ToolConcurrencyPolicy::Sequential
    );
}

/// The keyed policy overlay applies exactly, and only to the named server.
#[test]
fn policy_overlay_applies_to_exactly_the_named_server() {
    let bindings = bindings(
        r#""mcpServers": {
            "exa": {"url": "https://exa"},
            "local": {"command": "npx"}
        },
        "mcpToolPolicies": {
            "exa": {"execution": "background_only", "concurrency": "parallel"}
        }"#,
    );
    let exa = &bindings[&McpServerId::new("exa")];
    assert_eq!(exa.policy.execution, ToolExecutionPolicy::BackgroundOnly);
    assert_eq!(exa.policy.concurrency, ToolConcurrencyPolicy::Parallel);
    let local = &bindings[&McpServerId::new("local")];
    assert_eq!(local.policy.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(local.policy.concurrency, ToolConcurrencyPolicy::Sequential);
}

/// A policy for a server that does not exist is a configuration error, never
/// a silently ignored entry.
#[test]
fn policy_for_an_unknown_server_is_rejected() {
    let error = rejection(
        r#""mcpServers": {"exa": {"url": "https://x"}},
           "mcpToolPolicies": {"typo": {"execution": "background_only"}}"#,
    );
    assert!(
        error
            .to_string()
            .contains("mcpToolPolicies names typo, which mcpServers does not declare"),
        "unexpected error: {error}"
    );
}

// --------------------------------------------------------- determinism ----

/// JSON object insertion order never reaches the normalized binding set: the
/// keyed representation is the ordering authority.
#[test]
fn json_insertion_order_does_not_affect_the_binding_order() {
    let forward = bindings(
        r#""mcpServers": {
            "alpha": {"url": "https://alpha"},
            "beta": {"command": "beta-server"},
            "gamma": {"url": "https://gamma"}
        }"#,
    );
    let reversed = bindings(
        r#""mcpServers": {
            "gamma": {"url": "https://gamma"},
            "beta": {"command": "beta-server"},
            "alpha": {"url": "https://alpha"}
        }"#,
    );
    assert_eq!(forward, reversed);
    assert_eq!(
        forward.keys().map(McpServerId::as_str).collect::<Vec<_>>(),
        ["alpha", "beta", "gamma"],
        "iteration order is identity order, not document order"
    );
}
