//! Official-rmcp fixture server shared by the M7 local integration tests.
//!
//! Feature-gated behind `mcp-fixture`; never used by production code. The
//! fixture is served either in-process (Streamable HTTP) or as a self-spawned
//! stdio server (the test binary re-runs itself in fixture mode).
//!
//! The fixture exposes:
//!
//! - `echo` — deterministic success;
//! - `mutate` — flips the catalog, emits one fractional progress
//!   notification, then a `tools/list_changed` notification;
//! - `slow` — notifies call-start, awaits the server-side cancellation
//!   context (proving the client's cancellation notification reached the
//!   server), records the observation, and returns;
//! - when pagination is enabled, a multi-page `tools/list` catalog of
//!   `[alpha, beta, gamma, delta, echo]` served two tools per page;
//! - when [`FixtureServer::modern_conformance_tools`] is set, the MCP 2026
//!   conformance tools: [`STRUCTURED_SCALAR_TOOL`] and
//!   [`STRUCTURED_ARRAY_TOOL`] (non-object `structuredContent`) and
//!   [`ROUTED_TOOL`] (an `x-mcp-header`-annotated argument the SDK promotes
//!   to a SEP-2243 `Mcp-Param-*` routing header).
//!
//! # Observation seams
//!
//! Beyond the notification channels, the fixture counts what actually
//! reached it: `server/discover` probes, `subscriptions/listen` streams, and
//! `tools/list` requests. The last one is the client-cache seam — a refresh
//! answered from an SDK response cache never arrives here — and pairs with
//! [`FixtureServer::list_tools_ttl_ms`] (a positive SEP-2549 freshness
//! window) and [`FixtureServer::list_tools_fail_from`] (a refresh that fails
//! after a successful one).
//!
//! [`legacy`] is the one deliberate exception to the official-rmcp rule: a
//! hand-written pre-2026 wire fixture for the one peer shape an rmcp server
//! cannot represent.

pub mod legacy;
pub mod raw;
pub mod recovery;
/// The in-process Streamable HTTP fixture.
///
/// It is `cfg(test)` rather than merely feature-gated because it serves over
/// `axum`, which is a development dependency: the only consumer is the
/// in-crate boundary conformance suite, which compiles into the lib test
/// binary alongside it.
#[cfg(test)]
pub mod streamable_http;

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
    JsonObject, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion,
    ServerCapabilities, ServerInfo, SubscriptionFilter, Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{RoleServer, ServerHandler, ServiceExt};

/// The environment variable selecting fixture mode when the test binary is
/// re-executed as its own stdio MCP server.
pub const FIXTURE_MODE_ENV: &str = "RUSTX_M7_MCP_FIXTURE";
/// The environment variable naming the marker file the `slow` tool writes
/// the moment its server-side cancellation context fires (self-spawned
/// stdio fixtures, where the fixture state lives in another process).
pub const CANCEL_FILE_ENV: &str = "RUSTX_M7_FIXTURE_CANCEL_FILE";
/// The environment variable selecting the paginated `tools/list` catalog
/// page size (self-spawned stdio fixtures).
pub const PAGE_SIZE_ENV: &str = "RUSTX_M7_FIXTURE_PAGE_SIZE";
/// The environment variable prefixing fixture tool names. This is primarily
/// a multi-server test seam: MCP tool names are model-facing and therefore
/// must be unique across simultaneously active fixture servers.
pub const TOOL_PREFIX_ENV: &str = "RUSTX_M7_FIXTURE_TOOL_PREFIX";
/// The environment variable narrowing the protocol revisions the fixture
/// server supports, as a comma-separated list (self-spawned stdio fixtures).
///
/// This is the deterministic protocol-negotiation seam: the value flows
/// straight into rmcp's `ServerHandler::supported_protocol_versions`, which
/// bounds `server/discover` advertisement, `initialize` negotiation, and
/// per-request version validation alike.
pub const PROTOCOL_VERSIONS_ENV: &str = "RUSTX_M7_FIXTURE_PROTOCOL_VERSIONS";
/// The environment variable making the fixture's `tools/list` fail with a
/// correlated error message of exactly the given byte length
/// (self-spawned stdio fixtures).
///
/// This is the deterministic oversized-diagnostic seam (Issue #81): an
/// external MCP peer can produce an arbitrarily large failure payload, and
/// the capability availability contract must bound it before the
/// diagnostic enters authoritative state.
pub const LIST_TOOLS_ERROR_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_LIST_TOOLS_ERROR_BYTES";
/// The environment variable selecting the byte length of the successful
/// `echo` text result (Issue #103).
pub const RESULT_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_RESULT_BYTES";
/// The environment variable selecting comma-separated successful `echo`
/// content-block byte lengths. Blocks are joined by the Tool Plane's
/// deterministic newline representation, so their aggregate can cross the
/// bound even when each block is individually below it.
pub const RESULT_BLOCK_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_RESULT_BLOCK_BYTES";
/// The environment variable naming a marker file to which each successful
/// `echo` call appends one line. This keeps exactly-once assertions
/// deterministic for self-spawned stdio fixtures, whose server state lives
/// in another process.
pub const ECHO_CALL_COUNT_FILE_ENV: &str = "RUSTX_M7_FIXTURE_ECHO_CALL_COUNT_FILE";
/// The environment variable publishing the SEP-2322 multi round-trip guard
/// tools in the fixture catalog (self-spawned stdio fixtures).
///
/// They are opt-in so that every pre-existing fixture catalog assertion keeps
/// its exact tool set.
pub const MRTR_TOOLS_ENV: &str = "RUSTX_M7_FIXTURE_MRTR";
/// The environment variable selecting how many `tools/call` rounds the MRTR
/// guard tool [`MRTR_CONFIRM_TOOL`] performs before it returns its final
/// result (self-spawned stdio fixtures). The default is two: one
/// `input_required` round and one final round.
pub const MRTR_ROUNDS_ENV: &str = "RUSTX_M7_FIXTURE_MRTR_ROUNDS";
/// The environment variable naming the file every MRTR fixture round appends
/// one JSON observation line to.
///
/// This is the cross-process MRTR wire seam: a self-spawned stdio fixture's
/// server state lives in another process, so "what did round N actually
/// receive" is asserted from this file. Each line records the tool name, the
/// echoed `requestState`, the `inputResponses` map, and whether the request's
/// own `_meta` advertised the client elicitation capability.
pub const MRTR_OBSERVATION_FILE_ENV: &str = "RUSTX_M7_FIXTURE_MRTR_FILE";

/// The MRTR guard tool: it asks for one bounded elicitation choice and then
/// returns the chosen value as its final result.
pub const MRTR_CONFIRM_TOOL: &str = "mrtr_confirm";
/// The MRTR guard tool that asks two input requests in one round.
pub const MRTR_MULTI_TOOL: &str = "mrtr_multi";
/// The MRTR guard tool that returns a `requestState`-only round (the spec's
/// load-shedding shape) before completing.
pub const MRTR_STATE_ONLY_TOOL: &str = "mrtr_state_only";
/// The MRTR guard tool that asks for MCP sampling, which rustX refuses.
pub const MRTR_SAMPLING_TOOL: &str = "mrtr_sampling";
/// The MRTR guard tool that asks for MCP roots, which rustX refuses.
pub const MRTR_ROOTS_TOOL: &str = "mrtr_roots";
/// The MRTR guard tool that mixes one supported and one unsupported request.
pub const MRTR_MIXED_TOOL: &str = "mrtr_mixed";
/// The MRTR guard tool whose `requestState` exceeds the rustX retention bound.
pub const MRTR_OVERSIZED_STATE_TOOL: &str = "mrtr_oversized_state";
/// The MRTR guard tool whose elicitation schema rustX cannot represent.
///
/// It asks for a `string` with `format: "email"`. The **type** is supported —
/// free-form strings became a typed Text question in Issue #242 — but rustX
/// has no deterministic, faithful email validator, so it refuses that schema
/// form rather than accepting the field and silently discarding its
/// constraint.
pub const MRTR_UNSUPPORTED_SCHEMA_TOOL: &str = "mrtr_unsupported_schema";
/// The MRTR guard tool that asks one **mixed typed form**: a bounded `enum`,
/// a free-form `string`, an `integer`, and a `boolean` in one schema.
pub const MRTR_TYPED_TOOL: &str = "mrtr_typed";
/// The MRTR guard tool that asks one multi-select `enum` with explicit
/// `minItems`/`maxItems`.
pub const MRTR_MULTI_SELECT_TOOL: &str = "mrtr_multi_select";
/// The MRTR guard tool that emits one progress notification on every round
/// before it asks or completes.
pub const MRTR_PROGRESS_TOOL: &str = "mrtr_progress";
/// The MRTR guard tool that parks server-side until its cancellation context
/// fires, on its **continuation** round only.
pub const MRTR_SLOW_CONTINUATION_TOOL: &str = "mrtr_slow_continuation";

/// The bounded elicitation choices the MRTR fixture tools ask about.
pub const MRTR_CHOICES: [&str; 2] = ["stable", "beta"];

/// Parses a comma-separated protocol revision list.
///
/// Every MCP revision string is accepted, including ones no SDK knows: that
/// is exactly what an unsupported-revision fixture needs.
#[must_use]
pub fn parse_protocol_versions(value: &str) -> Vec<ProtocolVersion> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            serde_json::from_value::<ProtocolVersion>(serde_json::Value::String(entry.to_owned()))
                .expect("a protocol revision string always deserializes")
        })
        .collect()
}

impl FixtureServer {
    /// Builds a fixture from the self-spawn environment: cancellation
    /// marker file, pagination page size, and protocol behavior, when set.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            list_changed_supported: true,
            cancel_observed_file: std::env::var_os(CANCEL_FILE_ENV).map(PathBuf::from),
            page_size: std::env::var(PAGE_SIZE_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            tool_prefix: std::env::var(TOOL_PREFIX_ENV).ok(),
            supported_versions: std::env::var(PROTOCOL_VERSIONS_ENV)
                .ok()
                .map(|value| parse_protocol_versions(&value)),
            list_tools_error_bytes: std::env::var(LIST_TOOLS_ERROR_BYTES_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            result_bytes: std::env::var(RESULT_BYTES_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            result_block_bytes: std::env::var(RESULT_BLOCK_BYTES_ENV).ok().map(|value| {
                value
                    .split(',')
                    .filter_map(|entry| entry.parse::<usize>().ok())
                    .collect()
            }),
            echo_call_count_file: std::env::var_os(ECHO_CALL_COUNT_FILE_ENV).map(PathBuf::from),
            mrtr_tools: std::env::var_os(MRTR_TOOLS_ENV).is_some(),
            mrtr_rounds: std::env::var(MRTR_ROUNDS_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            mrtr_observation_file: std::env::var_os(MRTR_OBSERVATION_FILE_ENV).map(PathBuf::from),
            ..Self::default()
        }
    }

    /// The revisions this fixture serves, newest first.
    fn versions(&self) -> Vec<ProtocolVersion> {
        self.supported_versions.clone().unwrap_or_else(|| {
            let mut versions = ProtocolVersion::KNOWN_VERSIONS.to_vec();
            versions.sort_by(|left, right| right.as_str().cmp(left.as_str()));
            versions
        })
    }

    /// The fixture tool catalog for the current state. In pagination mode
    /// the catalog is the finite five-tool set served two tools per page.
    fn catalog(&self) -> Vec<Tool> {
        if let Some(page_size) = self.page_size {
            let _ = page_size;
            return vec![
                self.fixture_tool_named("alpha"),
                self.fixture_tool_named("beta"),
                self.fixture_tool_named("delta"),
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("gamma"),
            ];
        }
        let mut tools = if self.changed.load(Ordering::Acquire) {
            vec![
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("new_tool"),
            ]
        } else {
            vec![
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("mutate"),
                self.fixture_tool_named("slow"),
            ]
        };
        if self.modern_conformance_tools {
            tools.push(routed_tool_named(&self.tool_name(ROUTED_TOOL)));
            tools.push(self.fixture_tool_named(STRUCTURED_ARRAY_TOOL));
            tools.push(self.fixture_tool_named(STRUCTURED_SCALAR_TOOL));
        }
        if self.mrtr_tools {
            for name in MRTR_TOOLS {
                tools.push(self.fixture_tool_named(name));
            }
        }
        tools
    }

    fn fixture_tool_named(&self, name: &str) -> Tool {
        let name = self.tool_name(name);
        fixture_tool_named(&name)
    }

    fn tool_name(&self, name: &str) -> String {
        self.tool_prefix
            .as_deref()
            .map_or_else(|| name.to_owned(), |prefix| format!("{prefix}{name}"))
    }
}

/// The shared observable state of one fixture instance.
#[derive(Clone, Default)]
pub struct FixtureServer {
    /// The catalog flip observed by `tools/list`.
    pub changed: Arc<AtomicBool>,
    /// The current subscription sink, installed by `listen`.
    pub sink: Arc<tokio::sync::Mutex<Option<rmcp::service::SubscriptionSink>>>,
    /// Fired when the `slow` tool starts executing server-side.
    pub slow_started: Arc<tokio::sync::Notify>,
    /// Fired when the `slow` tool's server-side cancellation context
    /// becomes observable.
    pub cancel_observed: Arc<tokio::sync::Notify>,
    /// When set, the `slow` tool additionally writes a marker file the
    /// moment its cancellation context fires (for self-spawned stdio
    /// fixtures, where the fixture state lives in another process).
    pub cancel_observed_file: Option<PathBuf>,
    /// Whether the server advertises and accepts `tools/list_changed`.
    pub list_changed_supported: bool,
    /// When set, `tools/list` paginates its catalog with this page size.
    pub page_size: Option<usize>,
    /// When set, the exact protocol revisions this server supports; `None`
    /// means every revision the SDK knows.
    pub supported_versions: Option<Vec<ProtocolVersion>>,
    /// Optional prefix applied to model-facing fixture tool names.
    pub tool_prefix: Option<String>,
    /// The number of `subscriptions/listen` streams this server has accepted.
    ///
    /// A client that installed more than one invalidation mechanism per
    /// connection shows up here as a count above one.
    pub listen_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// Fired after a `subscriptions/listen` handler has installed its sink.
    /// Tests use this to distinguish connection completion from server-side
    /// subscription-handler delivery.
    pub listen_ready: Arc<tokio::sync::Notify>,
    /// When set, `tools/list` fails with a correlated error message of
    /// exactly this many bytes (the oversized-diagnostic seam).
    pub list_tools_error_bytes: Option<usize>,
    /// When set, the successful `echo` tool returns one text block of exactly
    /// this many bytes.
    pub result_bytes: Option<usize>,
    /// When set, the successful `echo` tool returns one text block for each
    /// listed byte length.
    pub result_block_bytes: Option<Vec<usize>>,
    /// When set, each successful `echo` invocation appends one line to this
    /// file for deterministic cross-process exactly-once assertions.
    pub echo_call_count_file: Option<PathBuf>,
    /// The number of `server/discover` probes this server answered.
    ///
    /// This is the inline-lifecycle wire evidence: rmcp's `Auto` lifecycle
    /// only ever falls back to the legacy `initialize` handshake *after* a
    /// probe, so a connection that negotiated 2026-07-28 against a server
    /// which advertises nothing else is provably a discover negotiation —
    /// the legacy fallback offers a pre-inline revision such a server does
    /// not speak and could not have succeeded.
    pub discover_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// The number of remote `tools/list` requests this server answered.
    ///
    /// This is the cache observation seam: a client that answered a refresh
    /// from an SDK response cache never reaches the server, so this counter
    /// distinguishes "the peer was contacted again" from "something replayed
    /// the previous response".
    pub list_tools_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// When set, every successful `tools/list` response declares this
    /// positive SEP-2549 `ttlMs`, inviting a client-side response cache to
    /// treat the catalog as fresh.
    pub list_tools_ttl_ms: Option<u64>,
    /// When set, the `tools/list` request with this 1-based number — and
    /// every later one — fails with a correlated server error.
    ///
    /// A client that serves a stale cached success on re-fetch failure hides
    /// exactly this fact from the capability coordinator.
    pub list_tools_fail_from: Option<usize>,
    /// Whether the catalog additionally publishes the MCP 2026 conformance
    /// tools ([`STRUCTURED_SCALAR_TOOL`], [`STRUCTURED_ARRAY_TOOL`],
    /// [`ROUTED_TOOL`]).
    pub modern_conformance_tools: bool,
    /// Whether the catalog additionally publishes the SEP-2322 multi
    /// round-trip guard tools (Issue #242).
    pub mrtr_tools: bool,
    /// How many `tools/call` rounds [`MRTR_CONFIRM_TOOL`] performs before its
    /// final result. `2` means one `input_required` round then one final
    /// round; `1` would make it an ordinary single-round tool.
    pub mrtr_rounds: Option<usize>,
    /// When set, every MRTR round appends one JSON observation line here.
    pub mrtr_observation_file: Option<PathBuf>,
}

/// The conformance tool whose result carries a **scalar** `structuredContent`.
pub const STRUCTURED_SCALAR_TOOL: &str = "structured_scalar";
/// The conformance tool whose result carries an **array** `structuredContent`.
pub const STRUCTURED_ARRAY_TOOL: &str = "structured_array";
/// The conformance tool whose input schema annotates one argument with
/// `x-mcp-header`, so a SEP-2243 `Mcp-Param-*` routing header is generated
/// by the SDK for its `tools/call`.
pub const ROUTED_TOOL: &str = "routed";
/// The `x-mcp-header` name annotated on [`ROUTED_TOOL`]'s `region` argument.
pub const ROUTED_TOOL_HEADER: &str = "Region";
/// The scalar value [`STRUCTURED_SCALAR_TOOL`] returns as `structuredContent`.
pub const STRUCTURED_SCALAR_VALUE: i64 = 42;

/// The prefixed names of one fixture's MCP 2026 conformance tools, together
/// with the results they answer.
///
/// Kept beside `call_tool` rather than inside it: the conformance tools are
/// one bounded family with one shape (a fixed result per name), and the
/// fixture's own catalog behaviour stays readable when they are not inlined
/// into the general dispatch chain.
struct ConformanceTools {
    routed: String,
    structured_scalar: String,
    structured_array: String,
}

impl ConformanceTools {
    fn of(fixture: &FixtureServer) -> Self {
        Self {
            routed: fixture.tool_name(ROUTED_TOOL),
            structured_scalar: fixture.tool_name(STRUCTURED_SCALAR_TOOL),
            structured_array: fixture.tool_name(STRUCTURED_ARRAY_TOOL),
        }
    }

    /// The conformance result for `request`, when it names one of these
    /// tools; `None` leaves the request to the fixture's ordinary dispatch.
    fn result(&self, request: &CallToolRequestParams) -> Option<CallToolResult> {
        if request.name == self.structured_scalar {
            // MCP 2026 complete result framing carrying a *scalar*
            // `structuredContent`: the field is arbitrary JSON, not an
            // object.
            return Some(CallToolResult::structured(serde_json::json!(
                STRUCTURED_SCALAR_VALUE
            )));
        }
        if request.name == self.structured_array {
            return Some(CallToolResult::structured(serde_json::json!([1, 2, 3])));
        }
        if request.name == self.routed {
            // Echoes the argument the SDK promoted to a `Mcp-Param-*`
            // routing header, so a successful call is also evidence the
            // body still carried it.
            let region = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("region"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Some(CallToolResult::success(vec![ContentBlock::text(region)]));
        }
        None
    }
}

/// Every SEP-2322 guard tool the fixture publishes when
/// [`FixtureServer::mrtr_tools`] is set.
pub const MRTR_TOOLS: [&str; 12] = [
    MRTR_CONFIRM_TOOL,
    MRTR_MIXED_TOOL,
    MRTR_MULTI_SELECT_TOOL,
    MRTR_MULTI_TOOL,
    MRTR_OVERSIZED_STATE_TOOL,
    MRTR_PROGRESS_TOOL,
    MRTR_ROOTS_TOOL,
    MRTR_SAMPLING_TOOL,
    MRTR_SLOW_CONTINUATION_TOOL,
    MRTR_STATE_ONLY_TOOL,
    MRTR_TYPED_TOOL,
    MRTR_UNSUPPORTED_SCHEMA_TOOL,
];

/// The prefixed names of one fixture's SEP-2322 guard tools.
///
/// These are **guard tools** in the modern MRTR sense, not imperative
/// `elicitation/create` callers: a round returns an `InputRequiredResult` as
/// its complete result, and the next round observes the client's
/// `inputResponses` and echoed `requestState` in its own request params.
struct MrtrTools {
    confirm: String,
    mixed: String,
    multi_select: String,
    multi: String,
    typed: String,
    oversized_state: String,
    progress: String,
    roots: String,
    sampling: String,
    slow_continuation: String,
    state_only: String,
    unsupported_schema: String,
}

impl MrtrTools {
    fn of(fixture: &FixtureServer) -> Self {
        Self {
            confirm: fixture.tool_name(MRTR_CONFIRM_TOOL),
            mixed: fixture.tool_name(MRTR_MIXED_TOOL),
            multi_select: fixture.tool_name(MRTR_MULTI_SELECT_TOOL),
            multi: fixture.tool_name(MRTR_MULTI_TOOL),
            typed: fixture.tool_name(MRTR_TYPED_TOOL),
            oversized_state: fixture.tool_name(MRTR_OVERSIZED_STATE_TOOL),
            progress: fixture.tool_name(MRTR_PROGRESS_TOOL),
            roots: fixture.tool_name(MRTR_ROOTS_TOOL),
            sampling: fixture.tool_name(MRTR_SAMPLING_TOOL),
            slow_continuation: fixture.tool_name(MRTR_SLOW_CONTINUATION_TOOL),
            state_only: fixture.tool_name(MRTR_STATE_ONLY_TOOL),
            unsupported_schema: fixture.tool_name(MRTR_UNSUPPORTED_SCHEMA_TOOL),
        }
    }

    fn owns(&self, name: &str) -> bool {
        [
            &self.confirm,
            &self.mixed,
            &self.multi_select,
            &self.multi,
            &self.typed,
            &self.oversized_state,
            &self.progress,
            &self.roots,
            &self.sampling,
            &self.slow_continuation,
            &self.state_only,
            &self.unsupported_schema,
        ]
        .into_iter()
        .any(|candidate| candidate == name)
    }
}

/// One bounded single-select elicitation request over [`MRTR_CHOICES`].
#[must_use]
pub fn mrtr_choice_request(message: &str, property: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": {property: {"type": "string", "enum": MRTR_CHOICES}},
                "required": [property],
            },
        },
    })
}

/// The bounded multi-select choices the MRTR fixture asks about.
pub const MRTR_MULTI_SELECT_CHOICES: [&str; 3] = ["eu", "us", "ap"];

/// One **mixed typed form**: a bounded `enum`, a free-form `string` with its
/// own length bounds, an `integer` with a range, and a `boolean`.
///
/// This is the shape the repaired typed interaction vocabulary exists for: it
/// becomes one questionnaire carrying one `SingleChoice`, one `Text`, one
/// `Integer`, and one `Boolean` question.
#[must_use]
pub fn mrtr_typed_form_request(message: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "channel": {"type": "string", "enum": MRTR_CHOICES},
                    "operator": {
                        "type": "string",
                        "title": "Operator",
                        "description": "What is your GitHub username?",
                        "minLength": 1,
                        "maxLength": 39,
                    },
                    "attempts": {"type": "integer", "minimum": 1, "maximum": 5},
                    "notify": {"type": "boolean"},
                },
                "required": ["channel", "operator", "attempts", "notify"],
            },
        },
    })
}

/// One multi-select `enum` request with explicit `minItems`/`maxItems`.
#[must_use]
pub fn mrtr_multi_select_request(
    message: &str,
    min_items: u64,
    max_items: u64,
) -> serde_json::Value {
    serde_json::json!({
        "method": "elicitation/create",
        "params": {
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "regions": {
                        "type": "array",
                        "items": {"type": "string", "enum": MRTR_MULTI_SELECT_CHOICES},
                        "minItems": min_items,
                        "maxItems": max_items,
                    },
                },
                "required": ["regions"],
            },
        },
    })
}

/// The MCP sampling request rustX must refuse without calling any model.
#[must_use]
pub fn mrtr_sampling_request() -> serde_json::Value {
    serde_json::json!({
        "method": "sampling/createMessage",
        "params": {
            "messages": [{"role": "user", "content": {"type": "text", "text": "capital?"}}],
            "maxTokens": 64,
        },
    })
}

/// The MCP roots request rustX must refuse without disclosing any workspace.
#[must_use]
pub fn mrtr_roots_request() -> serde_json::Value {
    serde_json::json!({"method": "roots/list"})
}

fn input_requests(
    entries: serde_json::Value,
) -> Result<rmcp::model::InputRequests, rmcp::ErrorData> {
    serde_json::from_value(entries).map_err(|error| {
        rmcp::ErrorData::internal_error(format!("invalid fixture input requests: {error}"), None)
    })
}

/// The round number this call is: round 1 has no echoed state, and every
/// later round carries the `rustx-fixture-round-<n>` state the fixture minted.
fn fixture_round(request: &CallToolRequestParams) -> usize {
    request
        .request_state
        .as_deref()
        .and_then(|state| state.strip_prefix(FIXTURE_ROUND_STATE_PREFIX))
        .and_then(|round| round.parse::<usize>().ok())
        .map_or(1, |round| round + 1)
}

/// The prefix of the fixture's own opaque continuation state.
///
/// It is opaque **to the client**: rustX never parses it, and the fixture
/// asserts it comes back byte-identical.
pub const FIXTURE_ROUND_STATE_PREFIX: &str = "rustx-fixture-round-";

/// The exact opaque state the fixture mints for round `round`.
#[must_use]
pub fn fixture_round_state(round: usize) -> String {
    format!("{FIXTURE_ROUND_STATE_PREFIX}{round}")
}

/// One observation line appended by every MRTR fixture round.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MrtrObservation {
    /// The model-facing tool name the round called.
    pub tool: String,
    /// The 1-based round number, derived from the echoed state.
    pub round: usize,
    /// The exact `requestState` this round received, verbatim.
    pub request_state: Option<String>,
    /// The exact `inputResponses` map this round received.
    pub input_responses: Option<serde_json::Value>,
    /// Whether this request's own `_meta` advertised a client elicitation
    /// capability (SEP-2575 per-request capabilities).
    pub elicitation_advertised: bool,
    /// The business arguments this round received, so a test can prove they
    /// are unchanged across rounds.
    pub arguments: serde_json::Value,
}

fn record_mrtr(
    path: Option<&std::path::Path>,
    observation: &MrtrObservation,
) -> Result<(), rmcp::ErrorData> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            rmcp::ErrorData::internal_error(
                format!("cannot record fixture MRTR round: {error}"),
                None,
            )
        })?;
    let line = serde_json::to_string(observation).map_err(|error| {
        rmcp::ErrorData::internal_error(format!("cannot encode fixture MRTR round: {error}"), None)
    })?;
    writeln!(file, "{line}").map_err(|error| {
        rmcp::ErrorData::internal_error(format!("cannot record fixture MRTR round: {error}"), None)
    })
}

/// Reads back every observation one MRTR fixture run recorded.
///
/// # Panics
///
/// Panics when the observation file exists but contains a line the fixture
/// did not write.
#[must_use]
pub fn mrtr_observations(path: &std::path::Path) -> Vec<MrtrObservation> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a fixture MRTR observation line"))
        .collect()
}

/// The routing-conformance tool definition: one primitive argument carrying
/// the SEP-2243 `x-mcp-header` annotation the SDK promotes to a header.
#[must_use]
fn routed_tool_named(name: &str) -> Tool {
    let mut tool = fixture_tool_named(name);
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), serde_json::json!("object"));
    schema.insert(
        "properties".to_owned(),
        serde_json::json!({
            "region": {"type": "string", "x-mcp-header": ROUTED_TOOL_HEADER},
        }),
    );
    schema.insert("additionalProperties".to_owned(), serde_json::json!(false));
    tool.input_schema = Arc::new(schema);
    tool
}

impl FixtureServer {
    /// A fixture with `tools/list_changed` support.
    #[must_use]
    pub fn with_list_changed() -> Self {
        Self {
            list_changed_supported: true,
            ..Self::default()
        }
    }
}

fn record_echo_call(path: Option<&std::path::Path>) -> Result<(), rmcp::ErrorData> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            rmcp::ErrorData::internal_error(
                format!("cannot record fixture echo call: {error}"),
                None,
            )
        })?;
    writeln!(file, "echo").map_err(|error| {
        rmcp::ErrorData::internal_error(format!("cannot record fixture echo call: {error}"), None)
    })
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder().enable_tools();
        if self.list_changed_supported {
            capabilities = capabilities.enable_tool_list_changed();
        }
        let mut info = ServerInfo::new(capabilities.build());
        // The legacy `initialize` fallback echoes this revision whenever the
        // client asks for one the fixture does not serve, so it must be a
        // revision the fixture really serves.
        if let Some(newest) = self.versions().first() {
            info.protocol_version = newest.clone();
        }
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.versions())
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, rmcp::ErrorData>> + Send {
        // Overridden only so the advertised set matches `versions()` even
        // when the fixture narrows it, and so the probe itself is countable
        // wire evidence of the inline lifecycle.
        self.discover_calls.fetch_add(1, Ordering::Release);
        std::future::ready(Ok(DiscoverResult::from_server_info(
            self.versions(),
            self.get_info(),
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if self.modern_conformance_tools && name == self.tool_name(ROUTED_TOOL) {
            return Some(routed_tool_named(name));
        }
        Some(fixture_tool_named(name))
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + Send
    {
        // Counted before any early return: the seam answers "did this
        // request reach the server", which is exactly as true for a failed
        // catalog as for a successful one.
        let request_number = self.list_tools_calls.fetch_add(1, Ordering::Release) + 1;
        if let Some(bytes) = self.list_tools_error_bytes {
            let message = format!("catalog unavailable: {}", "x".repeat(bytes));
            return std::future::ready(Err(rmcp::ErrorData::internal_error(message, None)));
        }
        if self
            .list_tools_fail_from
            .is_some_and(|first| request_number >= first)
        {
            return std::future::ready(Err(rmcp::ErrorData::internal_error(
                format!("the fixture catalog is unavailable from request {request_number}"),
                None,
            )));
        }
        let ttl_ms = self.list_tools_ttl_ms;
        let tools = self.catalog();
        let Some(page_size) = self.page_size else {
            let mut result = rmcp::model::ListToolsResult {
                tools,
                ..Default::default()
            };
            result.ttl_ms = ttl_ms;
            return std::future::ready(Ok(result));
        };
        // Cursor-based pagination: the cursor is the index of the first tool
        // of the next page; `None` starts at page zero.
        let cursor = request.and_then(|request| request.cursor);
        let start = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let next_start = start + page_size;
        let page = tools
            .into_iter()
            .skip(start)
            .take(page_size)
            .collect::<Vec<_>>();
        let next_cursor = if next_start < self.catalog().len() {
            Some(next_start.to_string())
        } else {
            None
        };
        let mut result = rmcp::model::ListToolsResult {
            tools: page,
            next_cursor,
            ..Default::default()
        };
        result.ttl_ms = ttl_ms;
        std::future::ready(Ok(result))
    }

    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.list_changed_supported
            .then(|| SubscriptionFilter::builder().tools_list_changed().build())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        self.listen_calls.fetch_add(1, Ordering::Release);
        *self.sink.lock().await = Some(context.sink().clone());
        self.listen_ready.notify_one();
        context.cancelled().await;
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one fixture dispatch chain stays readable in one place"
    )]
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send {
        let changed = self.changed.clone();
        let sink = self.sink.clone();
        let slow_started = self.slow_started.clone();
        let cancel_observed = self.cancel_observed.clone();
        let cancel_observed_file = self.cancel_observed_file.clone();
        let result_bytes = self.result_bytes;
        let result_block_bytes = self.result_block_bytes.clone();
        let echo_call_count_file = self.echo_call_count_file.clone();
        let echo_name = self.tool_name("echo");
        let mutate_name = self.tool_name("mutate");
        let slow_name = self.tool_name("slow");
        let conformance = ConformanceTools::of(self);
        let mrtr = MrtrTools::of(self);
        let mrtr_rounds = self.mrtr_rounds.unwrap_or(2).max(1);
        let mrtr_observation_file = self.mrtr_observation_file.clone();
        async move {
            if mrtr.owns(&request.name) {
                return mrtr_call(
                    &mrtr,
                    &request,
                    &context,
                    mrtr_rounds,
                    mrtr_observation_file.as_deref(),
                )
                .await;
            }
            if let Some(result) = conformance.result(&request) {
                return Ok(result.into());
            }
            if request.name == echo_name {
                record_echo_call(echo_call_count_file.as_deref())?;
                let blocks = result_block_bytes.map_or_else(
                    || {
                        vec![ContentBlock::text(result_bytes.map_or_else(
                            || "fixture echo".to_owned(),
                            |bytes| "x".repeat(bytes),
                        ))]
                    },
                    |blocks| {
                        blocks
                            .into_iter()
                            .map(|bytes| ContentBlock::text("x".repeat(bytes)))
                            .collect()
                    },
                );
                Ok(CallToolResult::success(blocks).into())
            } else if request.name == mutate_name {
                changed.store(true, Ordering::Release);
                if let Some(token) = context.meta.get_progress_token() {
                    context
                        .peer
                        .notify_progress(
                            ProgressNotificationParam::new(token, 0.5)
                                .with_total(3.5)
                                .with_message("fractional"),
                        )
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify progress: {error}"),
                                None,
                            )
                        })?;
                }
                // Inline-lifecycle clients open a `subscriptions/listen`
                // stream; legacy clients get the plain
                // `notifications/tools/list_changed` their revision defines.
                if let Some(sink) = sink.lock().await.clone() {
                    sink.notify_tool_list_changed().await.map_err(|error| {
                        rmcp::ErrorData::internal_error(
                            format!("cannot notify tool list change: {error}"),
                            None,
                        )
                    })?;
                } else {
                    context
                        .peer
                        .notify_tool_list_changed()
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify tool list change: {error}"),
                                None,
                            )
                        })?;
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("fixture changed")]).into())
            } else if request.name == slow_name {
                if let Some(token) = context.meta.get_progress_token() {
                    context
                        .peer
                        .notify_progress(ProgressNotificationParam::new(token, 0.25))
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify slow-call progress: {error}"),
                                None,
                            )
                        })?;
                }
                slow_started.notify_one();
                context.ct.cancelled().await;
                cancel_observed.notify_one();
                if let Some(path) = &cancel_observed_file {
                    let _ = std::fs::write(path, "cancel_observed");
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("fixture cancelled")]).into())
            } else {
                Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >())
            }
        }
    }
}

/// The whole SEP-2322 guard-tool behaviour of the fixture (Issue #242).
///
/// Every branch is a complete `tools/call` leg: it either returns an
/// `InputRequiredResult` (the ask) or a `CallToolResult` (the completion).
/// Nothing here blocks on a server-initiated client request, which is exactly
/// what distinguishes modern MRTR from the legacy `elicitation/create`
/// callback model.
#[allow(
    clippy::too_many_lines,
    reason = "the guard-tool family is one dispatch chain"
)]
async fn mrtr_call(
    tools: &MrtrTools,
    request: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
    rounds: usize,
    observation_file: Option<&std::path::Path>,
) -> Result<CallToolResponse, rmcp::ErrorData> {
    let round = fixture_round(request);
    let arguments = request
        .arguments
        .clone()
        .map_or(serde_json::Value::Null, serde_json::Value::Object);
    record_mrtr(
        observation_file,
        &MrtrObservation {
            tool: request.name.to_string(),
            round,
            request_state: request.request_state.clone(),
            input_responses: request
                .input_responses
                .as_ref()
                .map(|responses| serde_json::to_value(responses).unwrap_or_default()),
            elicitation_advertised: context
                .meta
                .client_capabilities()
                .is_some_and(|capabilities| capabilities.elicitation.is_some()),
            arguments: arguments.clone(),
        },
    )?;
    let name = request.name.as_ref();
    // The answer the client gave for `choice`, when this is a later round.
    let answered = |key: &str, property: &str| -> Option<String> {
        request
            .input_responses
            .as_ref()?
            .get(key)?
            .get("content")?
            .get(property)?
            .as_str()
            .map(str::to_owned)
    };
    if name == tools.sampling {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(
                serde_json::json!({"ask": mrtr_sampling_request()}),
            )?),
            Some(fixture_round_state(round)),
        )
        .into());
    }
    if name == tools.roots {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(
                serde_json::json!({"ask": mrtr_roots_request()}),
            )?),
            Some(fixture_round_state(round)),
        )
        .into());
    }
    if name == tools.mixed {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(serde_json::json!({
                "supported": mrtr_choice_request("Which channel?", "channel"),
                "unsupported": mrtr_sampling_request(),
            }))?),
            Some(fixture_round_state(round)),
        )
        .into());
    }
    if name == tools.unsupported_schema {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(serde_json::json!({
                "ask": {
                    "method": "elicitation/create",
                    "params": {
                        "message": "Your contact address?",
                        "requestedSchema": {
                            "type": "object",
                            // The type is supported; the `email` format is the
                            // one constraint rustX cannot faithfully enforce,
                            // so it refuses this schema instead of accepting
                            // the field and dropping the constraint.
                            "properties": {
                                "contact": {"type": "string", "format": "email"},
                            },
                            "required": ["contact"],
                        },
                    },
                },
            }))?),
            Some(fixture_round_state(round)),
        )
        .into());
    }
    if name == tools.typed {
        if round == 1 {
            return Ok(rmcp::model::InputRequiredResult::new(
                Some(input_requests(serde_json::json!({
                    "form": mrtr_typed_form_request("Configure the release"),
                }))?),
                Some(fixture_round_state(round)),
            )
            .into());
        }
        let accepted = request
            .input_responses
            .as_ref()
            .and_then(|responses| responses.get("form"))
            .and_then(|answer| answer.get("content"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        // The final result is the exact JSON the client sent, so a test can
        // prove the types on the wire, not merely their rendered text.
        return Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string(&accepted).unwrap_or_default(),
        )])
        .into());
    }
    if name == tools.multi_select {
        if round == 1 {
            let min = arguments
                .get("min_items")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2);
            let max = arguments
                .get("max_items")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(2);
            return Ok(rmcp::model::InputRequiredResult::new(
                Some(input_requests(serde_json::json!({
                    "regions": mrtr_multi_select_request("Which regions?", min, max),
                }))?),
                Some(fixture_round_state(round)),
            )
            .into());
        }
        let accepted = request
            .input_responses
            .as_ref()
            .and_then(|responses| responses.get("regions"))
            .and_then(|answer| answer.get("content"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string(&accepted).unwrap_or_default(),
        )])
        .into());
    }
    if name == tools.oversized_state {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(serde_json::json!({
                "ask": mrtr_choice_request("Which channel?", "channel"),
            }))?),
            // Deliberately far above the rustX retention bound.
            Some("s".repeat(64 * 1024)),
        )
        .into());
    }
    if name == tools.state_only {
        if round < rounds {
            // The spec's load-shedding shape: continuation state, no ask.
            return Ok(
                rmcp::model::InputRequiredResult::from_request_state(fixture_round_state(round))
                    .into(),
            );
        }
        return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "state-only rounds: {round}"
        ))])
        .into());
    }
    if name == tools.multi {
        if round == 1 {
            return Ok(rmcp::model::InputRequiredResult::new(
                Some(input_requests(serde_json::json!({
                    "first": mrtr_choice_request("Which channel?", "channel"),
                    "second": mrtr_choice_request("Which fallback?", "fallback"),
                }))?),
                Some(fixture_round_state(round)),
            )
            .into());
        }
        let first = answered("first", "channel").unwrap_or_else(|| "<none>".to_owned());
        let second = answered("second", "fallback").unwrap_or_else(|| "<none>".to_owned());
        return Ok(
            CallToolResult::success(vec![ContentBlock::text(format!("{first}+{second}"))]).into(),
        );
    }
    if name == tools.progress {
        if let Some(token) = context.meta.get_progress_token() {
            context
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(
                        token,
                        f64::from(u32::try_from(round).unwrap_or(u32::MAX)),
                    )
                    .with_message(format!("round {round}")),
                )
                .await
                .map_err(|error| {
                    rmcp::ErrorData::internal_error(
                        format!("cannot notify progress: {error}"),
                        None,
                    )
                })?;
        }
        if round < rounds {
            return Ok(rmcp::model::InputRequiredResult::new(
                Some(input_requests(serde_json::json!({
                    "ask": mrtr_choice_request(&format!("Round {round}: which channel?"), "channel"),
                }))?),
                Some(fixture_round_state(round)),
            )
            .into());
        }
        return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "progress rounds: {round}"
        ))])
        .into());
    }
    if name == tools.slow_continuation {
        if round == 1 {
            return Ok(rmcp::model::InputRequiredResult::new(
                Some(input_requests(serde_json::json!({
                    "ask": mrtr_choice_request("Which channel?", "channel"),
                }))?),
                Some(fixture_round_state(round)),
            )
            .into());
        }
        // The cross-process "the continuation round is executing" signal.
        // rustX forwards genuine remote progress through the one generic
        // progress seam, so a test observes this without polling and without
        // a sleep.
        if let Some(token) = context.meta.get_progress_token() {
            context
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(token, 1.0).with_message("continuation started"),
                )
                .await
                .map_err(|error| {
                    rmcp::ErrorData::internal_error(
                        format!("cannot notify continuation start: {error}"),
                        None,
                    )
                })?;
        }
        context.ct.cancelled().await;
        return Ok(
            CallToolResult::success(vec![ContentBlock::text("continuation cancelled")]).into(),
        );
    }
    // `tools.confirm`: the ordinary guard tool.
    if round < rounds {
        return Ok(rmcp::model::InputRequiredResult::new(
            Some(input_requests(serde_json::json!({
                "ask": mrtr_choice_request(&format!("Round {round}: which channel?"), "channel"),
            }))?),
            Some(fixture_round_state(round)),
        )
        .into());
    }
    let choice = answered("ask", "channel").unwrap_or_else(|| "<none>".to_owned());
    Ok(CallToolResult::success(vec![ContentBlock::text(format!(
        "rounds={round} choice={choice}"
    ))])
    .into())
}

/// Builds one canonical fixture tool definition.
#[must_use]
pub fn fixture_tool_named(name: &str) -> Tool {
    let mut tool = Tool::default();
    tool.name = name.to_owned().into();
    tool.description = Some(format!("fixture {name}").into());
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), serde_json::json!("object"));
    schema.insert("properties".to_owned(), serde_json::json!({}));
    schema.insert("additionalProperties".to_owned(), serde_json::json!(false));
    tool.input_schema = Arc::new(schema);
    tool
}

/// Serves one fixture over stdio until the client closes the transport.
pub async fn serve_stdio(fixture: FixtureServer) {
    let server = fixture
        .serve(rmcp::transport::stdio())
        .await
        .expect("fixture server");
    server.waiting().await.expect("fixture server wait");
}

/// Runs the current test binary as a stdio fixture server, when
/// [`FIXTURE_MODE_ENV`] selects fixture mode.
///
/// Every M7 local stdio test starts with this branch: with the fixture env
/// variable set, the re-executed binary serves the fixture and returns, so
/// the parent test process can drive the exact same fixture through the real
/// rustX `McpServerRuntime` stdio transport.
pub async fn serve_if_fixture_mode(fixture: FixtureServer) -> bool {
    if std::env::var_os(FIXTURE_MODE_ENV).is_some() {
        serve_stdio(fixture).await;
        true
    } else {
        false
    }
}

/// The argument vector that re-runs the current test binary as exactly this
/// test in fixture mode.
#[must_use]
pub fn fixture_spawn_args(test_name: &str) -> Vec<String> {
    vec![
        "--exact".to_owned(),
        test_name.to_owned(),
        "--quiet".to_owned(),
        "--nocapture".to_owned(),
        "--test-threads".to_owned(),
        "1".to_owned(),
    ]
}
