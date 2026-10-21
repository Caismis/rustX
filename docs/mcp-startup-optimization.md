# MCP startup optimization

This change follows the session latency investigation in
[session-operation-latency.md](session-operation-latency.md). It optimizes
network transport reuse, independent-source startup and best-effort remote
cleanup without changing MCP session ownership or replaying requests.

## DSH comparison

The reference is `deepseek-harness` at
`687f282e8625c6b8d11ff58211a90524be2e4317`, specifically
`packages/mcp/mcp-client/src/`:

- `index.ts` owns one connection supervisor per MCP plugin instance. An instance
  can be global or Agent-scoped; its registration scope defines isolation.
- `connection.ts` retains the negotiated Client until loss or disposal. Tool-list
  notifications re-sync the existing Client through a serial `syncChain`.
- `transport.ts` constructs the SDK's default Streamable HTTP transport. The
  plugin does not explicitly disable HTTP connection reuse.
- Initial activation still awaits connection and tool discovery. Its README
  explicitly documents SDK startup/discovery timeouts. DSH does not guarantee
  instant cold MCP startup.

rustX owns MCP generations per Conversation, including working directory,
credentials, notifications, cancellation and physical settlement. A single global
MCP client would change that contract. HTTP connection reuse within one generation
requires no such change and addresses repeated TCP/TLS setup within its handshake
and discovery sequence.

## Pi Durable and Pi MCP comparison

The Pi checkout is `1cedd32724abfcb0915f76cc61b6827e2c16dbad`.
These are distinct components, not one built-in Durable MCP implementation:

- `packages/durable/src/harness/harness.ts` keeps an application-supplied
  Registry on the Harness. Its built-in fork commits the child through
  `tx.forkConversation` and returns a Conversation handle sharing that Host.
  `session/transaction.ts` records the parent cut and copies the selected durable
  documents. Neither built-in path connects to MCP. An application-provided
  creation hook can add its own work.
- `packages/coding-agent/src/extensions/mcp/index.ts` starts enabled MCP
  connections in the background, using `Promise.all`, after yielding for the
  first render. Opening a session does not await that `pending` promise. The
  first prompt waits up to ten seconds for direct tool exposure; deferred and
  codemode tools wait when their discovery or execution actually needs them.
- `extensions/mcp/runtime.ts` shares one pending opening promise per server and
  reuses an established Client. Tools and resources are discovered concurrently.
- The coding-agent runtime **does close and replace MCP connections on fork**:
  `core/agent-session-runtime.ts` calls `teardownCurrent("fork", ...)`, which
  emits `session_shutdown`; the extension closes its connections there. Pi's
  smoothness must not be attributed to automatic cross-fork client sharing.
- `packages/mcp/src/transports/streamable-http.ts` first aborts local transport
  work, then makes a best-effort remote DELETE with a one-second timeout, using
  its last token rather than refreshing authentication during close.
- Pi's MCP client directly performs the legacy `initialize` lifecycle and
  currently lists supported revisions through `2025-11-25`. rustX also supports
  the inline lifecycle and its real negotiation. Removing that protocol support
  merely to avoid a probe would change functionality, not just performance.

The most important architectural lesson is separating durable conversation
publication from readiness to execute tools. It supports the explicit read-only
navigation state proposed in the original latency investigation; it does not
justify granting live controls before their native attachment proof exists.

## Implementation

`McpHttpClient` now retains at most one idle HTTP connection per host within its
own generation. reqwest continues to own HTTP protocol reuse and response-body
handling. The per-request ownership registry still terminates the exact tracked
POST or SSE response body on cancellation. Redirects remain disabled. A new
Conversation/credential binding receives its own HTTP client and pool.

The default rmcp client disabled pooling as a workaround for approximately
40 ms Linux delayed-ACK stalls, documented in its reqwest transport source.
For remote MCP startup, disabling reuse also causes repeated network connection
setup. The rustX transport now chooses bounded reuse, with regressions that
observe actual peer connections and request cancellation rather than relying on
a builder setting or a wall-clock threshold.

The root capability preparation pipeline now prepares up to eight selected MCP
sources concurrently. Completion frees a slot even while an earlier source is
waiting. Results are sorted by server identity before tool composition and
publication, preserving deterministic ordering. Unselected sources remain inert;
a failed optional source remains isolated. Every connect still uses the existing
conversation-counted physical owner and cancellation root.

Remote HTTP session deletion now has the same one-second best-effort bound as
Pi. rmcp invokes this cleanup only after stopping its local POSTs and streams.
The bound does not shorten a tool request's cancellation, process settlement or
call-gate barrier. A timeout is an explicit transport timeout; it is not evidence
that remote effects stopped. This removes up to four seconds of remote cleanup
waiting from the SDK's five-second allowance when Retry unloads the old runtime.

## Verification

The regression fixtures use real HTTP servers and native protocol handlers:

- Consecutive complete responses reuse the same server-observed TCP peer. A
  separate generation's client uses a different connection.
- One held tools/list response cannot prevent a sibling's handler from starting.
- Releasing a later source frees a slot for the ninth source while the first
  remains blocked; no more than eight sources are in flight.
- A failed optional source does not suppress healthy siblings; publication order
  and physical retirement remain deterministic.
- A real remote DELETE is attempted; a withheld reply expires at the cleanup
  bound, driven by a manual clock after server-side receipt.
- Existing MCP request cancellation, response arbitration, drain, reconnect and
  no-replay boundaries remain applicable to the pooled transport.

Completed checks:

- `cargo test --lib --features mcp-fixture mcp -- --test-threads=4`: 270 passed.
- `cargo test --test tools --features mcp-fixture 'mcp_runtime::unix_tests::' -- --test-threads=4`:
  19 passed, including legacy and inline HTTP protocol negotiation.
- `cargo clippy --lib --tests --features mcp-fixture -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo build --bins --locked`: passed with the default production features.

An opt-in real-endpoint measurement records connect, list and close phases
separately, without invoking tools or logging response bodies or credentials:

```sh
RUSTX_MCP_PROFILE_ENDPOINT=https://mcp.exa.ai/mcp \
  cargo test --test tools --features mcp-fixture \
  measure_configured_http_mcp -- --ignored --nocapture
```

Normal test runs do not contact this endpoint. External network samples are
observations, not CI performance thresholds or a universal latency guarantee.

## Remaining boundary

This is TCP connection reuse within a negotiated MCP generation, not shared MCP
Clients across Conversations. Fork/Retry still pay for creating the successor
MCP session. Retry also waits for the old runtime's physical close, including the
SDK's remote HTTP-session cleanup, now capped at one second for that best-effort
remote request. Local settlement proof and execution admission are unchanged.

DSH-style shared plugin clients could remove more repeated initialization when
a source is deliberately global. Implementing that requires an explicit service
scope and ownership model for credentials, workspaces, invalidations and active
calls; it should not be inferred solely from an equal server URL.

## External sample results

Three fresh Exa connections before and after the complete change, using the same
debug build profile, machine and public endpoint on 2026-10-21:

| Sample | Before: connect + list | After: connect + list | Before: close | After: close |
| --- | ---: | ---: | ---: | ---: |
| 1 | 10,014.8 ms | 5,381.8 ms | 2,531.7 ms | 368.7 ms |
| 2 | 7,883.6 ms | 4,386.1 ms | 1,212.1 ms | 399.5 ms |
| 3 | 7,374.1 ms | 3,308.7 ms | 1,010.0 ms | 316.4 ms |

The connect-plus-list median was 7.88 seconds before and 4.39 seconds after;
the full connect/list/close median was 9.10 seconds before and 4.79 seconds
after. An earlier verification run with pooling alone produced a 7.09-second
connect-plus-list median, with samples ranging from 5.87 to 8.02 seconds. The
network was not controlled, and the cleanup bound cannot explain differences in
connect/list time. These small, unpaired external samples must not be presented
as a causal estimate or guaranteed percentage improvement.

TCP reuse, concurrent-source progress and the cleanup bound are established by
the deterministic local tests; Exa's remaining cold initialization still takes
seconds. The single-server measurement does not exercise the multi-source
parallel optimization, and none of these successful close samples reaches the
one-second cleanup cap.
