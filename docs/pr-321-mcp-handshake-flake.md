# PR #321: the managed MCP handshake request-id collision

This records a defect that CI surfaced on PR #321's rebased head
`84bae05bc162988e2d17f748bfc15b7538aea2b5`, and its fix on the same branch
(`issue-307-composer-context-docks`). Fetched base `origin/main` remains
`289bd22e7589f33e0a5ba104dd03d220d4968015`, with PR #320 merged.

PR #321 is otherwise a Web-only slice ([WEB-04] composer context docks, see
[web-console/COMPOSER.md](../web-console/COMPOSER.md)). This fix is a deliberate
scope addition: the slice did not introduce the defect — its diff contains no
Rust — but the defect blocked the slice's CI and is a real production-path bug,
so it is corrected here rather than papered over with a re-run.

## The observed failure

`Platform-sensitive boundaries (macOS)` failed while the six other required jobs
passed on the same head:

```text
boundary_suites::mcp_tasks_managed::a_real_managed_fastmcp_task_completes_through_one_tool_result
panicked at src/../tests/boundary/mcp_tasks_managed.rs:137:6:
the managed FastMCP child connects: Discovery("conflict initialized response id:
expected 1, got 0; server stderr: ... Starting MCP server 'tasks-tool' with
transport 'stdio' ... ERROR Failed to run server: unhandled errors in a
TaskGroup (1 sub-exception)")

test result: FAILED. 2380 passed; 1 failed; 1 ignored; 670 filtered out
```

The child's `TaskGroup` error is a consequence, not the cause: rustX tears the
connection down after the handshake error, closing the pipe under the Python
server's stdio task group.

## Root cause

The error text is rmcp's, not rustX's. In
`ClientLifecycleMode::Auto`, rmcp (`serve_client_with_ct_inner`) probes
`server/discover` and, on a bounded timeout, falls back to `initialize` **on the
same transport without draining the abandoned probe**. Request ids come from one
shared `AtomicU32RequestIdProvider`, so the probe takes id 0 and the fallback
takes id 1:

```text
server/discover (id 0)   -> peer still starting (cold uv/Python import)
probe window elapses     -> fall back on the same transport
initialize      (id 1)   -> awaiting id 1
                         <- late response for id 0
                         => ConflictInitResponseId { expected: 1, got: 0 }
```

rmcp's `expect_response` continues past *notifications*, but on a **response**
whose id does not match it returns `ConflictInitResponseId` immediately. A late
reply correlated to an abandoned request is therefore fatal instead of being
discarded as stale.

Any MCP stdio server whose cold start can exceed the probe window is exposed.
It surfaces only as an intermittent connection failure, which is why the slow
macOS runner hit it first and Linux never did.

## Why this was not fixed by configuration or a version bump

- `DEFAULT_AUTO_DISCOVER_TIMEOUT` is ten seconds, passed into the **private**
  `serve_client_with_ct_inner`. Both public entry points
  (`serve_client_with_lifecycle`, `serve_client_with_lifecycle_and_ct`) omit it,
  so rustX cannot widen or disable the window.
- rmcp **3.4.0** (rustX pins 3.2.0) has a byte-identical `expect_response` and an
  identical `Auto` fallback with no drain. No released rmcp fixes this.

Widening the window would only have made the race rarer, never impossible.

## The fix

`handshake_lifecycle(server_id)` in `src/tools/mcp/mod.rs` chooses the lifecycle
per server:

- a server in the rustX-owned managed namespace is offered
  `ClientLifecycleMode::Discover`;
- every other server keeps `ClientLifecycleMode::Auto`.

`Discover` issues exactly one handshake request and — unlike `Auto` — wraps
`discover_startup` in **no timeout at all**. There is no second request id, so
the collision is structurally impossible rather than merely less likely, and a
slow cold start simply takes the time it needs.

This is sound because rustX materializes managed Python packages itself against
the pinned `MANAGED_FASTMCP_VERSION` (FastMCP 4), which always answers
`server/discover`, so those servers never needed the legacy fallback. The
`python:` namespace (`MANAGED_MCP_NAMESPACE`) is **reserved**: the session
configuration parser rejects any configured `mcp_servers` id in it, so no
external server can be misclassified as managed.

Classifying by identity avoids adding a field to `McpServerBinding`, which has
49 construction sites, and avoids sniffing the child's argv.

## Deliberately unchanged

- Externally configured servers keep the full `Auto` negotiation, because their
  revision is genuinely unknown and the legacy fallback is the point. Both
  legacy-interoperability regressions still pass unmodified
  (`a_discover_less_server_falls_back_to_the_legacy_initialize_handshake`,
  `a_legacy_server_rejecting_the_probe_with_a_non_modern_error_falls_back`).
- No test was skipped, weakened, retried or given a sleep.
- No dependency was upgraded, forked or patched.

## Regression coverage

`tools::mcp::tests::a_managed_package_is_offered_discover_while_unknown_peers_keep_auto`
asserts the managed namespace is offered `Discover` and that unknown peers keep
`Auto`, including the near-miss ids `python`, `pythonish` and
`not-python:tasks-tool` so the namespace test cannot over-match. It is a pure
selection test: deterministic, Linux-reproducible, and it costs no wall clock —
unlike a timing test that would have to outwait a ten-second probe window.

The real proof that the path still works end to end remains the existing managed
acceptances, which drive real `uv`-materialized FastMCP children.

## Residual risk

This fix covers rustX-materialized managed servers only. **An externally
configured MCP stdio server whose cold start exceeds ten seconds is still
exposed to the same upstream defect**, and rustX cannot fix that without an
upstream change to rmcp's `expect_response` (discard a response correlated to an
abandoned probe id and keep waiting) or a `[patch.crates.io]` fork. The upstream
fix is the general remedy and should be pursued separately; this change removes
the exposure for the only servers rustX owns end to end.

## Evidence (Linux, rebased worktree)

| Directory | Command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | Passed |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,850 passed; 0 failed; 1 ignored; 226 filtered |
| root | `cargo test --test contracts --test provider --all-features` | 25 + 166 passed; 5 opt-in live ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | 226 passed, 0 failed |
| root | `cargo test --all-features --test tools` | 157 passed, 0 failed |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 401 passed: durable 116, process 52, subagent 53, tools 157, conformance 23 |

The previously failing `mcp_tasks_managed` acceptance and its `mcp_mrtr_managed`
sibling both pass in the in-crate boundary run, as do every `mcp_managed` and
`uv` case in the tools target.

## Documentation moved with the code

`docs/invariants.md` (MCP protocol-revision negotiation; managed child
negotiation) and `docs/architecture.md` (connection setup; managed package
revision) asserted that rustX negotiates every peer through
`ClientLifecycleMode::Auto`. Both now state the managed `Discover` path.
