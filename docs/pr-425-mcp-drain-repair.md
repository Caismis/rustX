# PR #425: Streamable HTTP response observation

## Failure and ownership

CI run 1045 ([36513135705](https://github.com/Caismis/rustX/actions/runs/36513135705))
failed only in Boundary suites (Linux):
`mcp_runtime::unix_tests::modern_streamable_http_is_stateless_and_forwards_sdk_routing_headers`.
The routed call returned ``-32020: missing Mcp-Param-Region header for `region` ``.
The tools target otherwise passed 134 tests. All six other CI jobs, including
macOS, passed. The same focused test passed locally before the repair; that does
not disprove the source-level race.

rmcp 3.2.0 observes a normal `ServerMessage` with `cache_tools_from_response`
before publishing it. Its competing `StreamResult` path drains queued SSE
messages directly to the service without that observation. A tools/list response
can therefore reach rustX before rmcp's transport-private Tool schema cache knows
about its `x-mcp-header` annotations. The following tools/call then lacks the
promoted header despite correct schema and arguments in rustX. The image changes
do not own or alter this protocol state; scheduling can expose the pre-existing
asymmetry.

**Invariant:** Every ListToolsResult updates SEP-2243 Tool schema state before
upper-layer publication, regardless of the Streamable HTTP response-drain path.

## Dependency repair

The original registry rmcp 3.2.0 was published from upstream commit
`51ccb42993d6eb5075399672ce7a0c21a0e55eea`. Inspection found the same omission in
released rmcp 3.5.0 and upstream main
`ae2f9c9b45a2c98d24ee345406e79f507c9f9282`.

The dedicated patch in `Caismis/rust-sdk` changes only
`crates/rmcp/src/transport/streamable_http_client.rs`, on the exact 3.2.0 base.
Normal SSE messages, queued terminal/recovery drains and JSON responses now use
one `observe_server_message` helper. It clears matching pending state, updates
and validates schema state, then publishes. JSON keeps its per-request protocol
version. SSE drain uses the same negotiated version as normal SSE delivery;
recovery drains before installing the newly negotiated version.

No SDK source is vendored into rustX. Cargo pins immutable revision
[`cd83bcbb8e0a2512bb9df98db8acd1c2bb0233d0`](https://github.com/Caismis/rust-sdk/commit/cd83bcbb8e0a2512bb9df98db8acd1c2bb0233d0).
There is one rmcp package/source in Cargo's resolved graph.

## Deterministic evidence

`terminal_sse_drain_caches_tool_schema_before_next_call_headers` finishes the
actual SSE worker while its ListToolsResult remains queued, proves the response
channel is closed with one queued message, and directly executes the terminal
drain before ordinary ServerMessage handling can occur. After publication, the
production request header constructor produces `Mcp-Param-Region: us-west1` for
the next tools/call, alongside method, Tool name and protocol version.

Restoring the original drain bypass makes this regression fail with the missing
header. Restoring the shared observation helper makes it pass. This is a forced
ordering test, not repeated execution intended to provoke scheduling.

`observation_precedes_blocked_publication_and_respects_drain_version` fills the
publication channel, polls the drain once to its blocked send, then checks that
protocol observation already occurred. It covers modern and pre-SEP versions.
Neither new regression uses sleeps, retries or arbitrary task yields.

The existing rustX wire integration test remains unchanged. It checks modern
2026-07-28 negotiation, all SDK routing headers, the unchanged `us-west1` body
argument and absence of session identity. `McpHttpClient` passes custom headers
unchanged to rmcp's HTTP client and owns only local cancellation/settlement.
Production rustX does not parse annotations or synthesize routing headers.
`ClientCacheConfig::disabled()` remains in place: semantic response caching is
separate from required transport-private schema state.

Image capability/transport, artifact ownership, projection, context accounting,
lineage, Web Settings, Git settlement and Bash descriptions are unchanged.

## Dependency validation at the pinned revision

The SDK checkout is `/home/caismis/Documents/codes/rust-sdk-rustx-425`, branch
`rustx-sep2243-drain`. Its committed source is identical to Cargo's resolved
`~/.cargo/git/checkouts/rust-sdk-15d34526de1d34f0/cd83bcb` copy (`cmp` passed).
Both functional commands below were repeated after the commit, at the exact
pinned SHA, with a clean SDK worktree:

```sh
cargo +nightly fmt --all -- --check
cargo test -p rmcp --lib --features client,transport-streamable-http-client-reqwest transport::streamable_http_client::tests -- --nocapture
cargo test -p rmcp --features server,client,transport-streamable-http-server,transport-streamable-http-client-reqwest,reqwest --test test_streamable_http_standard_headers --test test_streamable_http_client_concurrency --test test_streamable_http_protocol_version --test test_streamable_http_json_response --test test_streamable_http_priming --test test_streamable_http_stale_session
```

Formatting passed. Worker tests: **9 passed**. Integration tests: **69 passed**
(concurrency 20, JSON 6, priming 5, protocol versions 25, stale sessions 4,
standard headers 9). No ignored or failing tests in these selections.

The original-bypass counterexample command selected
`terminal_sse_drain_caches_tool_schema_before_next_call_headers` with the same
library/features: **0 passed, 1 failed**, missing promoted header, as expected.
The original behavior was restored only temporarily for this proof and is not
in the committed dependency. Early test-authoring attempts exposed unsupported
direct message equality and then a string lookup against `HashMap<HeaderName, _>`;
assertions now compare serialized messages and use typed HeaderName keys. An
initial integration invocation omitted the explicit `reqwest` feature and was
rejected before running tests; the corrected command above passed.

Additional SDK Clippy checks did **not** pass:

```sh
cargo clippy -p rmcp --lib --tests --features server,client,transport-streamable-http-server,transport-streamable-http-client-reqwest,reqwest -- -D warnings
cargo clippy -p rmcp --lib --no-default-features --features client,transport-streamable-http-client-reqwest,reqwest -- -D warnings
```

The first reports 11 pre-existing diagnostics in unchanged
`transport/streamable_http_server/tower.rs` (collapsible conditions, large error
variants, type complexity and question-mark suggestions). The second reports
three dead-code diagnostics in unchanged `service.rs` under the reduced client
feature set. No lint suppression or unrelated SDK cleanup was added. These extra
SDK lint results are separate from rustX's required all-target/all-feature lint.

## rustX starting state

Worktree: `/home/caismis/Documents/codes/rustX-issue-412`.
Branch: `issue-412-image-reading-bash-description`.
Starting head: `dfa5b06c0375e605bff0018984059811e1798cff`.
Fetched main / merge base: `5d2e382154a9d3434afb860a6892d76e98ad6bfb`.
The starting worktree was clean, six commits ahead and zero behind main. A second
fetch confirmed both remote SHAs unchanged. No merge or history rewrite was
needed. Current-main CI 36493246236 passed. PR #425 remains the existing open,
non-draft PR with auto-merge disabled.

## rustX focused validation

```sh
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features mcp_runtime::unix_tests::modern_streamable_http_is_stateless_and_forwards_sdk_routing_headers -- --exact --nocapture
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features mcp -- --nocapture
cargo tree -i rmcp
cargo metadata --locked --format-version 1
```

The exact wire test passed before and after the dependency change (**1 passed**
each). The post-change MCP selection passed **296 tests**, zero failures and
zero ignored: 257 library tests, one catalog test, five process tests and 33
Tool-boundary tests. The wire test specifically asserts
`Mcp-Param-Region: us-west1` at the peer. Tree, lockfile and metadata identify
exactly one rmcp 3.2.0 package from the immutable fork revision above.

## Client and emulator validation

Commands ran in their respective package directories:

| Package | Commands | Result |
| --- | --- | --- |
| `test-support/fake-provider` | `uv sync --frozen`; `uv run --frozen pytest` | Passed; **51 tests** |
| `tui` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Passed; **894 tests**, 96 suites, none skipped |
| `web-console` | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm check:i18n`; `pnpm check:provenance`; `pnpm test`; `pnpm build` | Passed; **1,156 tests**, 64 files; 145 provenance records and 131 package notices verified |
| `dev` | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test` | Passed; **37 tests**, none skipped |

The production Web build emitted its existing large-chunk advisory. No assertion,
timeout, retry, ignore marker, CI configuration or screenshot baseline was changed.

Protocol validation ran in `protocol/app-server`:

```sh
corepack install
pnpm install --frozen-lockfile
pnpm generate
pnpm check
pnpm typecheck
```

All passed. Regeneration produced no diff in the sole v29 schema, TypeScript or
fixtures. No App Server vocabulary or protocol version changed in this repair.

## Full native validation

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --bins
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features
git diff --check
cargo tree -i rmcp
```

All passed. The full test command ran **4,058 passing tests, zero failures and
eight existing ignored tests** across 18 targets (including zero-test binary
and example targets). Breakdown:

| Target | Passed | Existing ignored |
| --- | ---: | ---: |
| Library | 3,433 | 3 |
| `cfg3_catalog` | 26 | 0 |
| `cfg3_managed_output` | 5 | 0 |
| `conformance` | 26 | 0 |
| `contracts` | 28 | 0 |
| `durable` | 129 | 0 |
| `process` | 63 | 0 |
| `provider` | 168 | 5 |
| `subagent` | 45 | 0 |
| `tools` | 135 | 0 |

The all-target run includes the Linux deterministic contracts and boundary
selections from the current workflow, with the provider emulator required. The
MCP regression passes both in isolation and in the complete Tool boundary target.
Local platform is Linux; no local macOS validation is claimed. The new pushed
rustX SHA requires its own GitHub macOS result.

## Full browser acceptance and validation ordering

```sh
cd web-console
CONTAINER_ENGINE=podman pnpm test:e2e
```

The first full run finished **142 passed, 1 failed**. The failure was
`composer primary seat, uploads and context stack dark 390`, with
`locator.screenshot: Element is not attached to the DOM` at `agent.spec.ts:126`.
This run overlapped protocol generation. Trace network records show
`protocol/app-server/fixtures.ts` was rewritten at **05:59:46.776 UTC** and Vite
requested its hot update at **05:59:46.779 UTC**, then invalidated/reloaded
`test/fixtures/agent.tsx` while screenshot capture held the old element.
Generation writes fixtures even when their contents remain identical.

The validation orchestration caused this interruption. Failure trace/screenshot
were preserved in `/tmp/rustx425-mcp-e2e-generation-interference`; no test,
assertion, timeout, retry count, browser helper or baseline was changed. After
all generation finished, the **same complete command** ran with no watched-file
writes and passed **143 tests, zero failures (7.4 minutes)**. This corrects a
known validation interference condition; it is not a repeated timing experiment
used as proof of the MCP repair. That proof is the dependency regression's forced
queued-response/terminal-drain ordering and the original-bypass counterexample.
