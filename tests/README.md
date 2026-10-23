# rustX test architecture

rustX tests are organized by the runtime layer that **owns the invariant**
under test, never by the issue or milestone that introduced them. Every
important architectural invariant has exactly one authoritative owner suite;
other suites may prove that the invariant crosses their boundary, but they
do not re-prove the lower-layer state machine.

## Two axes: semantic class × compilation placement

These are independent dimensions and must not be conflated.

**Semantic test class** — what kind of invariant is being proven:

1. **Unit test** — local behavior owned by one source module. A unit test
   of a boundary-owning module exercises that module's own primitive (the
   bash tool's unit tests spawn bash; the uv backend's run real uv builds);
   it is still a unit test — the conformance *suites* below are the
   cross-module proofs where the boundary itself is the contract.
2. **Deterministic contract test** — cross-module runtime contracts driven
   by scripted models/tools, manual clocks, explicit
   channels/barriers/watches, and deterministic `#[cfg(test)]` hooks. No
   real process is spawned or killed, no real stdio/IPC is crossed, and no
   filesystem/process/platform semantic is the invariant under test.
3. **Boundary conformance test** — the invariant *is* a real
   operating-system or runtime boundary: process spawn/death (SIGKILL),
   child-process supervision, real stdio/IPC to a spawned fixture server,
   shell background execution, SQLite durability/recovery over real files,
   the external provider emulator.
4. **Opt-in live provider test** — real credentialed provider smoke tests,
   always `#[ignore]`d. Never correctness authority for generic runtime
   semantics.

Some libtest functions are not ordinary correctness coverage, and are never
counted as such:

- **Measurement** — `#[ignore]`d instrumented runs
  (`issue419_measure_native_cold_load`,
  `stage_profile_real_create_pipeline`,
  `measure_native_request_index_complete_300_turn_source`) and the Web
  `pnpm test:issue-420-performance` counters. They record numbers for an
  issue report, assert no timing threshold and run only explicitly.
  Run the native request-index comparison with
  `CARGO_INCREMENTAL=0 cargo test --lib --all-features measure_native_request_index_complete_300_turn_source -- --ignored --nocapture`.
  The separate `t08_t15_t20_layout_construction_buffers_tokens_and_scopes_preview_reader`
  unit test proves exact preview bytes and bounded construction-write / scoped
  reader counts; its correctness result does not depend on elapsed time.
  The ignored `measure_native_scheduler_reconciliation` records native historical
  store measurements. Its explicit `RUSTX_SCHEDULER_BENCH_RELEASE_GATE` mode also
  checks predeclared supported-host resource/latency budgets; ordinary correctness
  lanes never select it. See [the Scheduler performance gate](../docs/issue-85-scheduler-performance.md).
- **Fixture generator** — `#[ignore]`d `regenerate_committed_fixture_corpus`
  rewrites a committed corpus; the `generate_schemas` and
  `generate_app_server_protocol` examples regenerate checked-in artifacts
  (the latter is the App Server protocol lane's drift check).
- **Fixture child entry point** — a `#[test]` that a parent test
  re-executes as its child process (for example `child_process_entry`,
  `staged_rollback_wire_child`, `deletion_process_child`, and the lane
  checker's `real_cargo_discovery_child`). Without its mode variable it
  returns at once, so its ordinary "pass" proves nothing; the parent test is
  the coverage.
- **Lexical source convention** — a test that checks the spelling of
  production source text. Rust cannot restrict imports between modules of
  one crate, so these name a dependency-direction or vocabulary convention
  honestly and prove neither runtime behavior nor a complete dependency
  graph. Their names say so (`*_source_spells_*`, `*_sources_spell_*`), and
  each points to the behavior or type proof that owns the invariant.
  Checking serialized output, diagnostics, protocol data or generated
  artifacts is ordinary behavior testing, not this class.

**Physical compilation placement** — where the test code compiles:

- **Source-module unit test** — `#[cfg(test)]` modules in `src/**`.
- **In-crate lib test** — sources under `tests/`, compiled into the crate's
  own test build because they need `#[cfg(test)] pub(crate)` seams.
- **External Cargo integration target** — `tests/<domain>/main.rs`, sees
  the published API only.

A test does **not** become a deterministic contract merely because it must
compile into the lib test binary to reach private seams. A test that kills
a real process remains boundary conformance even when it physically lives
in the in-crate test tree. That is why the in-crate tree has two roots:

| Namespace | Sources | Semantic class | Physical placement |
| --- | --- | --- | --- |
| `scripted_suites::` | `tests/scripted/` except `app_server/` | deterministic contracts | in-crate lib test via `src/lib.rs` |
| `local_runtime::session_runtime_manager::tests::` | `tests/scripted/app_server/` | deterministic contracts | the manager's private `#[cfg(test)] mod tests` |
| `boundary_suites::` | `tests/boundary/` | boundary conformance | in-crate lib test via `src/lib.rs` |

The namespace prefix is the stable semantic marker and is part of the CI
contract: jobs select or exclude classes by prefix
(`cargo test --lib -- boundary_suites::` / `-- --skip boundary_suites::`).
The compiled namespace, not the directory, is what a selector sees:
`tests/scripted/app_server/` is included by
`src/local_runtime/session_runtime_manager.rs` as that owner's private test
module, so its App Server host, connection, loopback transport and residency
contracts can reach the manager's `cfg(test)` probes and gates without
making them `pub(crate)`. It is therefore **not** under `scripted_suites::`,
and macOS skips it by its own prefix. Do not move it or widen the owner's
visibility to make the directory match a selector.

## Why anything compiles into the crate's test build

Two seams may not exist in the published API: a scripted `ModelAdapter`
behind a real catalog binding, and a scripted `ContextSummarizer` behind a
real context runtime. An external integration-test binary can only reach
`pub` items, so any suite needing these seams compiles into the crate's own
test build via `src/lib.rs`, where the seams are `#[cfg(test)] pub(crate)`.
Boundary suites additionally need the process-death child entry point and
the subagent registry's staged-child seam, which are likewise
`#[cfg(test)]`-only. Sources stay under `tests/` so `src/` carries
production code only; Cargo auto-discovers integration targets from
`tests/*.rs` and `tests/*/main.rs` only, so neither in-crate tree is also
built as a separate test binary.

`tests/support/` is the shared in-crate fixture layer (scripted adapters,
conformance drivers, settlement assertions); `tests/common/` is shared with
the external integration targets. Boundary suites use the same fixtures —
a boundary test may legitimately drive a scripted model adapter while its
invariant is a real process boundary.

## In-crate deterministic contract suites

`tests/scripted/` mirrors runtime ownership:

- `agent/` — **the owner of generic execution semantics**: attempt state
  machine, terminal uniqueness/terminal-last, `AttemptOutcome`
  correspondence, request start/settlement lifecycle, exact request counts
  and ordinals, canonical Assistant commit rules, tool lifecycle and
  canonical result ordering, cancellation arbitration and structural
  settlement, transient retry with frozen replay, model deadlines, tool
  execution-liveness deadlines (hard/idle deadline arbitration, cancellation
  intent vs physical settlement, frozen admitted policy, batch exactly-once),
  unresolved-output carryover, publication interaction at the loop boundary,
  and the Agent-Loop half of single-generation safety: the
  malformed-tool-proposal boundary and the degeneration/budget guard share one
  suite pair (`malformed_tool_proposal`, `generation_safety`) proving one
  bounded corrective generation for all semantic anomaly classes,
  canonical-history exclusion, budget composition, and the
  cancellation-versus-rejected-generation race. Which *provider* evidence
  becomes a malformed proposal is owned by the `provider` target instead, and
  the degeneration detector's own algorithm — evidence threshold, chunk
  invariance, false-positive controls — is owned by the unit tests in
  `src/model/generation.rs`.
- `context/` — layered context ownership: `engine` (provider-independent
  projection, token accounting, typed historical compaction prefix selection, driven
  through `ContextEngine` directly), `compaction_pipeline` (the shared
  committed transition — plan → summarize → validate exact post-summary fit
  → durable commit → hot-state installation — driven through
  `execute_compaction` against a real `SQLite` store, with the full
  failure-atomicity matrix), `compaction_metadata` (summary lineage
  metadata extraction), `runtime_integration` (`AgentExecution` ↔ context
  boundary composition: proactive compaction, overflow compact-and-retry,
  failure classification, cancellation, continuation invalidation), and
  `runtime_multi_compaction` (multi-attempt `ConversationRuntime`
  composition: request reconstruction, client detach/reattach, frozen
  session summary model).
- `runtime_client/` — host/endpoint/protocol/transport contracts, including
  the transport-independent conformance matrix run through the direct
  endpoint and the internal JSONL framing over in-memory pipes. That framing's
  one production consumer is a running child's read-only live inspection
  socket; no process serves the Runtime Client envelopes to a client.
- `capability/` — capability snapshots, quiescent commits, environment
  materialization, and the executable-identity no-op contracts: a changed
  MCP executable binding (a managed Python package whose source edit moved
  its prepared state, or a configured server whose launch changed) is a
  new publication even with a byte-identical `tools/list` schema, an
  unchanged binding is a true no-op, and an already-leased old generation
  keeps serving while future admissions resolve to the new generation.
- `interaction/` — the durable interaction audit's runtime half.
- `background/` — the background registry contracts and the deterministic
  half of the `execution` intrinsic control plane (routing for detached
  tool executions).
- `tools/` — native registry contracts and the conversation task list.

## In-crate boundary conformance suites

`tests/boundary/` — each suite's invariant is a real boundary:

- `durable/process_death/` — the FND-06 matrix: a real child process (this
  test binary re-executed) frozen at a deterministic gate, ended with real
  SIGKILL, then recovery asserted against the durable authority.
- `background/text_spill` — real bash background execution through the
  actual `bash-supervisor` binary: oversized output spills and the terminal
  inbound.
- `tool_deadline` — the Issue #204 generic hard deadline bounding a real
  foreground Bash process: the supervisor's process-group kill and reap are
  the physical settlement behind the proven `TimedOut`.
- `mcp_mrtr` — MCP multi-round-trip (SEP-2322) execution against a real MCP
  `2026-07-28` stdio child (this test binary re-executed as a guard-tool
  fixture server): one `ToolCall` stays one invocation across N bounded
  rounds with 0..N runtime-owned Interactions and exactly one terminal
  result, the opaque `requestState` round-trips byte for byte, unsupported
  Sampling/Roots/schemas fail before any prompt is published, and each
  cancellation frontier — before the first dispatch, while the Interaction
  is pending, at the continuation dispatch frontier in both directions — is
  decided through a deterministic barrier rather than a sleep. It also proves
  the typed interaction contract end to end: a mixed `enum`/`string`/
  `integer`/`boolean` form round-trips each answer as its own JSON type, a
  multi-select `enum`'s `minItems`/`maxItems` are enforced by the runtime
  before any continuation is dispatched, and an out-of-bound human answer is
  refused while the Interaction stays pending rather than failing the call.
  A second form declares scalars at the binary64 exact-value frontier — an
  `integer` whose entire legal answer set lies above the JavaScript
  safe-integer range, and a `number` bounded at `2^53` — proving that the
  exact values the runtime validated are the values the server receives, with
  one step outside either bound refused while the Interaction stays pending.
- `mcp_mrtr_managed` — the same contract end to end against a real managed
  `FastMCP` 4 child built by a real, network-bound `uv`: one model
  `ToolCall`, a negotiated `2026-07-28` connection, two real `tools/call`
  rounds, one runtime Interaction, one final `ToolResult`. Its form mixes a
  bounded `enum` with a free-form `string` in one schema, so the acceptance
  also proves the repaired typed schema mapping against a real server. It
  requires uv through the shared prerequisite (see
  [Prerequisites](#prerequisites)).
- `mcp_tasks` / `mcp_tasks_managed` — the MCP Tasks extension (SEP-2663) as
  an adapter-local remote sub-lifecycle of one admitted `ToolInvocation`,
  against the fixture child and a real managed `FastMCP` 4 child.
- `mcp_recovery` — MCP tool-call liveness, cancellation, transport loss,
  reconnection and last-known-good capability recovery over real MCP stdio
  children, ordered by the generic deadline's arming signal and a manual
  clock.
- `managed_selection` — real prepared `FastMCP` 4 sources entering the
  single frozen Agent registry, with source failure and exposure filtering
  independent.
The external `subagent` target additionally owns
`overrides` — the Issue #258 invocation-scoped override contract driven
against real composed runtime generations: replacement and isolation, the
dynamic delegation ceiling, Skill visibility, R1/R2 freeze, effective-profile
identity, and the Workflow half. The exact-identity algebra of the ceiling is
owned by the resolver's own unit tests, and the pre-staging refusal at the
model-facing Tool by the `subagent` intrinsic's unit tests; neither is
re-proven there.

- `subagent/conformance` — the child ownership boundary with real staged
  children (`sh`, own process group, real control socket): frozen authority
  crossing, registry lifecycle, exactly one terminal child notice, parent
  isolation, cancellation/drain across the boundary, and reliable routed
  Approval/Questionnaire control addressed by full conversation-local
  interaction references. It also proves root detach/reconnect presentation,
  child-death removal, and stale-response rejection. Also the Issue #178
  live-activity observation plane: activity projects while the lifecycle
  stays `Running`, a stalled or absent consumer changes nothing about child
  execution (the same workload fingerprints identically with no observer, a
  draining consumer, and a stalled one), a stalled parent projection
  coalesces superseded activity and converges on the newest revision,
  foreground live tool progress projects while the tool runs and is never
  durable (and is readable only through the child-owned read-only inspection
  socket, which refuses mutation, survives observer reconnects, and settles
  held connections and its socket at child shutdown), a retry's next request projects retry ordinal zero, activity
  frames commit no parent journal facts and never enter parent model
  context or the result channel, the frozen execution profile is the only
  projected configuration, and snapshot repair serves the latest
  observation. A child is an ordinary
  `ConversationRuntime`; generic retry/deadline/cancellation/settlement
  semantics belong to `scripted_suites::agent` and must not be replayed
  here.
- `subagent/execution_routing` — the subagent half of the `execution`
  intrinsic: status/cancel routing and terminal answer delivery against
  real staged children, including activity frames racing terminal
  settlement (dropped, never rewriting the terminal).
- `runtime_client/mcp_capability` — capability projection over a real MCP
  stdio child server (this binary re-executed in fixture mode).
- `runtime_client/python_capability` — capability projection over a managed
  Python tool package (Issue #174): a real, network-bound `uv` environment
  build serving a real `FastMCP` stdio child.

Every one of these is platform-sensitive (process groups, signals, unix
sockets, shell supervision), so the `boundary_suites::` prefix also runs in
the macOS CI job.

## External integration targets

A separate Cargo integration-test binary exists because it represents a
meaningful domain/boundary/dependency topology:

| Target | Boundary | Suites |
| --- | --- | --- |
| `provider` | one adapter over the in-process `FixtureServer` | request serialization, protocol translation, stream parsing, normalized error mapping, ToolCall acceptance and malformed-tool-proposal classification, capability/context translation at the adapter, opaque request params, opt-in live smoke |
| `conformance` | the external provider-emulator process | composed Agent Loop / lifecycle / Workflow conformance through the real runtime and a real provider boundary |
| `durable` | file-backed SQLite | recovery classification, pending-inbound inbox, publication store contract, interaction audit store, transcript history |
| `process` | the real `rustx` binary / local composition | the public CLI contract (bare help, retired launch grammar rejected without effects), App Server over real stdio and WebSocket processes, composition identity, capability startup isolation, runtime config, committed-example resource composition |
| `subagent` | the child-process boundary | named definition admission/resolution, frozen-policy handshake through a real launched child, end-to-end parent/child composition under a real App Server parent, the running child's internal read-only inspection socket |
| `tools` | OS/tooling boundary | Bash supervision, Read/Write/Edit/Grep/Glob, Skills, MCP config/runtime, uv backend, managed `FastMCP` materialization |
| `cfg3_catalog` | real configuration source files | CFG3 source resolution, shadowing and replacement, native writers' commit races and stale-write refusal, frozen model/provider invocation identity |
| `cfg3_managed_output` | managed Tool output storage | execution-identity locators, collision refusal without overwrite, spill reconstruction |
| `contracts` | none (pure) | serialization fixture round-trips, committed configuration examples |

The stdio MCP canonical-executor test holds the fixture's `mutate` response
behind a parent-created marker until the real `ProgressReporter` has observed
its fractional progress. rmcp schedules notification handlers independently of
response delivery: receiving a result does not prove its preceding wire
notification reached rustX before the caller released progress ownership
([MCP liveness invariant](../docs/invariants.md#mcp-liveness-connection-generations-and-recovery-issue-205)).
The marker establishes ordering; its polling interval does not. The existing
progress deadline, exact fractional values, result, invalidation and physical
cancellation assertions remain in force.

## Verification profiles

Fast feedback for ordinary development; complete verification at deliberate
integration checkpoints. CI runs one of two profiles, which differ in which
suites run, never in what a suite asserts:

- **FAST** — every pull request push: the always-on checks plus the suites
  of the surfaces the change touches.
- **FULL** — every job, complete: `main`, the nightly schedule, manual
  dispatch, and a pull request carrying the `full-verification` label, for
  its current head and every later one.

The `plan` job (`.github/scripts/verification.py`) owns selection. It reads
the pull request's changed files — each path with GitHub's change status and,
for a rename or copy, its original path — and decides the profile, the risk, the
affected surfaces (`rust`, `protocol`, `web`, `tui`, `dev`, `documentation`)
and which jobs and affected steps run. Documentation is low risk but not
inert: the `contracts` target reads the top-level, `docs/`, Web and TUI
Markdown, so in FAST such a change selects the `documentation-contracts`
job, which runs that target alone (FULL runs it in `rust-contracts`). Every gated job runs only when the plan's
`run[<job id>]` is true, and the script's tests check that `ci.yml` gates
each job exactly that way.

**Risk.** Low risk is an explicit allowlist (`LOW_RISK` in
`verification.py`); every other path is high risk, unknown paths and an
unreadable or possibly truncated diff included. Low risk is:

- documentation (`*.md` at the top level, `docs/`, the Web and TUI READMEs
  and guides, `tests/README.md`, `LICENSE`) — Markdown under `examples/` is
  not documentation, it defines Agents, Skills and Workflows;
- Web presentation: `web-console/src/presentation/**` (the provenance check
  fails a file there that imports a client, protocol, binding, storage,
  fetch or WebSocket authority), the translation dictionaries, stylesheets
  and `public/`;
- leaf tests: `web-console/test/*.test.ts(x)`, `tui/test/*.test.ts`, browser
  specs and their screenshot references, when added or modified. A changed
  leaf test runs in FAST.

Removing or renaming a leaf test or a screenshot reference is high risk:
FAST runs only the tests that still exist and cannot notice coverage that
disappeared, so the removal needs FULL, and its review needs the replacement
proof or the changed test contract that justifies it. A rename is classified
by both of its paths. A change status GitHub does not document, or none, is
high risk.

Everything that owns behaviour is therefore high risk, including Web
connection, authorization, workspace admission and client ownership
(`web-console/src/connection/`, `src/workspaces/`, `src/client/`, …), the
TUI's command dispatcher, presentation state and protocol, shared test
fixtures and helpers, CI, manifests and all Rust.

**Browser FAST and FULL.** Both run the same build-once inputs, pinned
Chromium and one Playwright worker per runner; they differ only in the
selection the plan passes to `browser-prepare`:

- **FULL** — every test Playwright discovers, in two shards. The coverage
  gate fails unless all of them ran exactly once.
- **FAST** — one shard of the `@smoke` tests (startup and admission, the
  development launcher with the real Web carrier, App Server and Host,
  connection authentication, a native retry workflow, lost-acknowledgement
  model selection, configuration recovery, Harness primitives and an Agent
  screenshot reference), every test of a changed spec (or of a spec whose
  screenshot references changed), and, for a low-risk presentation change,
  the presentation contracts — every spec that holds screenshot references
  and every test tagged `@presentation` (the fixture matrices of the
  replacement ledger below) — then over two shards. The coverage gate fails
  if FAST selects no smoke test, omits a changed spec, names a changed spec
  Playwright no longer discovers, or, for a presentation change, selects no
  `@presentation` test; its summary says it is a subset, never full
  coverage.

**Merge.** `Merge verification` (the `verification` job) is the verdict, and
the repository ruleset "Merge verification on main" requires it from GitHub
Actions on `main`, with branches up to date, so a pull request whose check
is missing, pending, cancelled or failed cannot merge. The job always runs
and fails unless:

- every job the plan selected succeeded and nothing unselected ran;
- a high-risk change was verified by FULL;
- the tested merge commit's parents are exactly the event's base and head,
  and that head is still the pull request's head.

The run's summary names the pull request head, base and tested merge commit
separately; the browser inventory records the merge commit it checked out.

**Promotion.** Add the `full-verification` label when a high-risk change is
a stable candidate: the `labeled` event runs FULL on the current head, and
every later push runs FULL while the label stays. To return to FAST for
further development, remove the label (removal starts no run); add it again
for the next candidate. Until a high-risk head has a successful FULL run, its
`Merge verification` fails with the reason.

**FAST Rust.** `rust-fast` compiles the library test harness and the
`process` target once, runs every unit and deterministic contract module
except the in-crate boundary suites, the capacity suite and four heavy
contract groups, and runs the real-process `process` target. Each heavy group
also runs in FAST when the change touches its owner:

| Group | FAST runs it when the change touches |
| --- | --- |
| `durable::sqlite::` | `src/durable/` |
| `local_runtime::session_runtime_manager::tests::`, `local_runtime::session::` | `src/app_server/`, `src/local_runtime/`, `tests/scripted/app_server/`, `tests/support/` |
| `runtime_client::response::` | `src/runtime_client/` |

On Linux the in-crate boundary suites, the other external boundary targets
and the `contracts`/`provider` targets run in FULL only. macOS core (the
seven external boundary targets) runs on every push that touches Rust;
macOS full (those targets plus unit tests and in-crate boundary suites)
runs in FULL instead. This is a
deliberate scheduling tradeoff: before #479 every pull request push ran
everything; now a high-risk change gets that complete verification once, on
its promoted head, and its intermediate pushes get FAST feedback. A
low-risk change merges on FAST, which selects the suites of its surfaces (a
Web presentation change runs the Web client checks and the browser FAST
selection with every screenshot-reference spec).

**Capacity tests.** A test whose value comes from its scale — a thousand
turns, thousands of requests, 2,048 tool calls, 4,097 releases, a thousand
sessions — lives in a `scale` module beside the code it measures, so its
name contains `::scale::`. FAST skips that suite and FULL runs all of it;
the lane check requires the suite to be non-empty and completely executed
in FULL. An ordinary regression test may use small boundary cases, but it
never replaces a capacity test. When a change touches the logic a capacity
test measures, run it locally (see Running) or promote the pull request.

## How CI selects the classes

The CI jobs (`.github/workflows/ci.yml`) mirror the semantic classes. The
workflow's `cargo test` steps are the only selection source; this table
summarizes them and the lane check below verifies them against Cargo:

| Suite / group | Class | Linux lane | macOS |
| --- | --- | --- | --- |
| source-module unit tests (`src/**`) | unit | rust-contracts | full |
| `scripted_suites::` | deterministic contracts | rust-contracts | skipped |
| `local_runtime::session_runtime_manager::tests::` | deterministic contracts | rust-contracts | skipped |
| `boundary_suites::` | boundary conformance | rust-boundaries | full (required) |
| bin and example harnesses | unit (examples: workload checkers, lane checker) | rust-contracts | bins only |
| `contracts`, `provider` targets | pure contracts / adapter translation | rust-contracts | no |
| `durable`, `process`, `subagent`, `tools`, `conformance`, `cfg3_catalog`, `cfg3_managed_output` | boundary conformance | rust-boundaries | core and full (required) |
| `test-support/fake-provider` pytest | emulator's own tests | rust-boundaries | no |

- **quality** (ubuntu) — fmt, clippy, whitespace.
- **rust-contracts** (ubuntu) — `cargo build --bins`, then
  `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::`
  and `cargo test --test contracts --test provider --all-features`.
  The build comes first because the bash tool's unit tests exec
  `target/debug/bash-supervisor` and the interactive-process unit tests exec
  `target/debug/interactive-supervisor`; `cargo test --bins` builds the bin
  *test harnesses*, which are a different Cargo unit and place neither
  executable there. The three bin harnesses define no test today, and of
  the example harnesses only `session_list_benchmark` (workload checker)
  and `check_test_lanes` (the lane checker's own tests) do. Bins and
  examples stay selected so Cargo's own target discovery picks up a future
  test; the lane check allows their harnesses to be empty.
- **rust-boundaries** (ubuntu) — the emulator's pytest suite, then
  `cargo test --all-features --no-run` over the seven external boundary
  targets, then `cargo test --lib --all-features -- boundary_suites::`, then
  `cargo test --all-features --test durable --test process --test subagent
  --test tools --test conformance --test cfg3_catalog --test
  cfg3_managed_output`. Compiling an integration target also builds every
  real executable against the library the tests link and places it at
  `target/debug/`, where the text_spill suite execs `bash-supervisor`; a
  separate `cargo build --bins` would compile another complete library
  (without dev-dependency features) that no test in the lane links. Both
  test steps set `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`.
- **rust-platform-core** (macos; FAST) — the seven external boundary
  targets, natively: compiled first (which, as in rust-boundaries, places
  the real executables at `target/debug/`), then run with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Real process start and termination,
  signals, PTY, stdio and WebSocket transports, the native configuration
  writer lock race (`cfg3_catalog`), SQLite and filesystem durability and
  cleanup. It never compiles the library test harness, which was about half
  of the macOS lane's compilation (721 s of about 1,450 s in run
  39015122730).
- **rust-platform-boundaries** (macos; FULL) — one job: the same external
  targets, compiled first, then `cargo test --lib --bins --all-features
  -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::`
  (unit tests — including the boundary-owning bash/uv modules, whose
  primitives differ across OSes — plus the in-crate boundary suites) and the
  external targets, both with `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Running
  the lib half on a second runner in parallel was measured and rejected:
  with cold caches it shortened FULL by about a minute for about 17 more
  macOS runner-minutes, since that runner compiles the dependencies and the
  ordinary library again. The `--no-run` compile steps separate native
  compile timing from execution and execute nothing; both macOS jobs keep
  `CARGO_BUILD_JOBS=1`. `contracts`/`provider` and the deterministic
  contract majority are Linux-only: they carry no process or filesystem
  semantics. Overlap with Linux is intentional: a macOS boundary test is
  native proof, not a duplicate. A pull request push that touches Rust runs
  macOS core; the macOS unit tests and in-crate boundary suites run on its
  promoted FULL head, on `main` and nightly. A low-risk change
  (presentation, documentation) runs no macOS Rust lane at all; that is the
  deliberate tradeoff of the FAST profile.
- **tui** — independent Node/pnpm lane. `pnpm test` includes the real-child
  integration suite, which drives the actual `rustx` binary (built by
  `cargo build --bin rustx` in that lane) over the real stdio and WebSocket
  transports against the provider emulator, with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. The lane is not an owner of Rust
  semantic validation.
- **app-server-protocol** — `pnpm check` regenerates the schema and
  TypeScript through `cargo run --example generate_app_server_protocol` and
  fails on drift; `pnpm typecheck` checks the fixtures and reference
  intersections.
- **Browser and Web client** — five jobs. `browser-prepare` builds once from
  the checked-out source: `cargo build --bins --example
  document_artifact_fixture` and the only production Web build (`pnpm
  build`: Vite, then `provenance.ts --artifact` over the new `dist/`), both
  run by `web-console/scripts/browser-artifact.ts pack`. It records them
  (debug info stripped, symbols kept) with their identity — commit, runner
  platform and architecture, the `rust-toolchain.toml` pin it proved its
  compiler matched, the Cargo command, Node version and ABI, pnpm,
  Playwright, and checksums of the Cargo and pnpm manifests and lockfiles —
  and a checksum and mode per file, and takes Playwright's own `--list`
  inventory of the suite. It assigns whole spec files to the two shards,
  heaviest first onto the lighter shard, by their measured seconds in
  `web-console/test/e2e/shard-weights.json` (a file without a measurement
  weighs its test count at the measured mean); Playwright's own `--shard`
  splits by test count, which put 1,298 s on one shard and 397 s on the other
  in run 39026668816. Refresh the weights from complete runs' shard
  reports with `node scripts/browser-shards.ts weights <report.json>...`;
  each file's weight is its mean wall span (until the next test starts, so
  fixture time outside a test's reported duration counts) over the runs
  given. Runner speed varies, so several runs balance better than one.
  Weights only steer balance: coverage is verified independently of them. Node native addons are not shared: `pnpm install`
  rebuilds them from the lockfile in every job. `web-client` (launcher
  `typecheck`/`test`, Web `typecheck`, Vitest `pnpm test` with the
  `*.measurement.tsx` counters excluded, `check:i18n`, `check:provenance`)
  and the two `browser-shard` jobs install those inputs only after
  `browser-artifact.ts install` verifies the identity against their own
  checkout and runner and every checksum, into a checkout holding no build
  output; any mismatch fails, and nothing is rebuilt in its place. Each
  shard runs `browser-tests.sh --test-list shard-<i>.txt` with one worker,
  after the script re-verifies the installed
  inputs (`RUSTX_BROWSER_PREPARED=1` selects that path instead of
  `cargo build`) and `provenance.ts --artifact` re-checks the installed
  bundle. `browser-report` ("Browser acceptance coverage", the one browser
  verdict) merges the shards' blob reports into one HTML report with every
  trace and screenshot, and `browser-shards.ts verify` passes only when every
  inventoried test of the selection ran exactly once in exactly one shard,
  against the inventory's commit and prepared inputs, and passed or skipped
  itself; a missing or empty shard, a duplicate, an unexecuted test, a FULL
  inventory smaller than discovery, a FAST selection without a smoke test or
  without a changed spec, or another identity fails it, as does any shard's
  failure. The preparation build refuses build-affecting overrides
  (`RUSTFLAGS`, `CARGO_PROFILE_*`, `CARGO_BUILD_*` except `JOBS`, compiler
  and linker flags, `NODE_ENV`, `VITE_*`, …), so the identity's `build:
  repository-defaults` plus the checksums of the Cargo, toolchain, pnpm and
  Vite configuration are its whole build configuration. Locally,
  `pnpm test:e2e` stays self-contained: `pnpm build && bash
  scripts/browser-tests.sh`, which runs `cargo build` itself.

### Browser presentation matrices

A native browser workflow (a real App Server, provider emulator and Agent
execution per test) proves business semantics once per distinct failure
boundary. Locale, theme and width are presentation dimensions: each value
stays covered, by a retained native run or by a stable fixture or component
test that asserts the same UI, but not every combination re-executes the
Agent. Distinct runtime boundaries — lost acknowledgement, reconnect, Stop
and late output, compaction ownership, retry transitions, model adoption —
are never merged into one run.

| Removed native run | Invariant | Retained or replacement proof | Saved (CI run 39015122730) |
| --- | --- | --- | --- |
| `model-retry` en dark, zh light | timeout retry disclosure, recovery placement, Stop during retry, late output hidden | the same workflow in en light 1440 (plus reload) and zh dark 390; `model-retry-presentation.spec.ts` (fixture: native Trace retry states) for en and zh × light and dark × 1440 and 390, active and settled: copy, active state, readable contrast in the theme, in-viewport without overflow, disclosure by pointer and keyboard, settled placement before the reply; both locales' timeout/active/settled copy in `test/model-retry.test.tsx` | 8.4 s + 8.8 s |
| `compaction` en dark, zh light | failed and held manual compaction, reconnect, automatic compaction after a questionnaire, projection history, Stop and late output | the same workflow in en light 1440 (plus reconnect and reload) and zh dark 390; the Context summary card in en and zh, light and dark, 1440 and 390 in `compaction-summary.spec.ts` (fixture) | 9.5 s + 9.9 s |
| `live-model` profile selection (1440, lost), (390, received) | same-model profile mutation with a received or lost acknowledgement, frozen admission, reload without replay, provider request parameters | (1440, received) and (390, lost): each acknowledgement outcome and each width once; neither the placement nor the lost-ACK recovery depends on the other | 7.1 s + 6.6 s |

Both replacement matrices are tagged `@presentation`, so a low-risk Web
presentation change, which merges on FAST, still runs every locale, theme
and width of them (`test/browser-verification.test.ts` checks the
discovered suite selects all eight cases of each).
None of the removed runs held a screenshot reference; their captures went to
`/tmp`. Each replacement was checked against a deliberate violation: a wrong
Chinese retry label fails `model-retry.test.tsx`, a fixed dark retry summary
colour fails the four dark `model-retry-presentation` cases, and a wrong
English projection description fails the four English `compaction-summary`
cases.
The suite shrank by six native runs (about 50 s of test time) and gained four
fixture cases (about 7 s). Fixture-based matrices (`trajectory`, `agent`,
`shell`, `settings-presentation`) start no Agent and were left unchanged.

### The lane check

`examples/check_test_lanes.rs` keeps CI selection honest without becoming a
second copy of it. Each Rust test lane ends with
`cargo run --all-features --example check_test_lanes -- --job <job id>`, which reads the
workflow's executing `cargo test` steps and asks libtest, through `--list`,
what each one selects — so filters, `--skip`, features and `cfg` are
Cargo's and libtest's own semantics. Every test job declares its profiles
(`env: VERIFICATION_PROFILE: fast`, `full` or `fast full` — not a `RUST*`
name, which the Rust cache would hash into its key) and must be gated by
exactly `if: fromJSON(needs.plan.outputs.run)['<its id>']`; a FAST step may
also be gated by the plan's affected-component map, and nothing else may
make a test step conditional. Each profile's coverage is its own. It fails
for:

- a Cargo test target (lib, bin, example, integration test) selected by no
  executing FULL Linux step (`--no-run` and FAST steps do not count);
- a Linux-discovered runnable test that no FULL Linux step executes, the
  capacity suite included;
- a step that selects no runnable test of a lib or integration-test target;
- a positive filter that matches nothing, only `#[ignore]`d tests, or only
  `--skip`ped tests, and a `--skip` that excludes nothing;
- a suite in its `REQUIREMENTS` table that is missing or not fully executed
  in its profile on its platform: FAST on Linux must always run
  `scripted_suites::`, `runtime::conversation_runtime::` and the `process`
  target (affected steps never satisfy this; the capacity suite is
  excluded), FULL on Linux the whole `::scale::` suite, FULL on macOS
  `boundary_suites::` and the seven external boundary targets;
- a test job without a declared profile or the canonical plan gate, and a
  step or libtest output it cannot interpret exactly.

Discovery is always native: the macOS job checks macOS, and `--job` refuses
a job of another platform. Run it without `--job` to check every target of
the host platform (it compiles what is missing).

Discovery owns the output it parses: each internal `cargo test ... --list`
passes Cargo's own `--color never` (before `--`), so an inherited
`CARGO_TERM_COLOR=always` — which the hosted Rust toolchain action exports —
cannot hide a harness's `Running` line. A harness it cannot recognize is an
error, never an empty inventory; a recognized bin or example harness may
list no test.

Its deterministic tests run with `--examples` in rust-contracts. Besides
the selection rules over a stand-in inventory,
`real_cargo_discovery_is_independent_of_inherited_color` runs the
production discovery with real Cargo over a small fixture package (its own
temporary directory and target directory), once with
`CARGO_TERM_COLOR=always` and once with `never`, each set only in a
re-executed child's environment.

### Measuring verification cost

Compare only runs with matched conditions: the same lane, runner image,
toolchain and cache state. Pull-request runs restore the caches published by
`main`, so two PR runs in the same window are comparable; a failed or
cancelled run is diagnostic evidence, not a timing baseline.

- **Cache state**: the `Cache Rust dependencies` step logs either
  `Restored from cache key "…" full match: true|false` or `No cache found`.
  A miss turns dependency compilation into part of the first build step.
- **Compilation vs execution**: Cargo's `Finished … in` line is the build
  span of one command; libtest's `finished in` is execution of one harness.
  A `cargo test` step's duration is their sum plus process startup, never
  pure test time. The macOS lane separates them with `--no-run` steps,
  `/usr/bin/time -l` (wall time, peak RSS) and the `macos-cargo-timings`
  artifact (per-unit `--timings` reports).
- **Library units**: two commands share compiled work only when they build
  the same unit. Compare artifact hashes rather than command lines:

  ```bash
  cargo test --all-features --test process --no-run --message-format=json \
    | jq -r 'select(.reason=="compiler-artifact" and .target.name=="rustx") | .filenames[0]'
  ```

  Features, `cfg(test)` and dev-dependency feature unification (any test or
  example target in the invocation) each select a different unit. The macOS
  lane keeps `CARGO_BUILD_JOBS=1`: a single rustX library compilation peaks
  near 3.5 GB RSS there (`/usr/bin/time -l`), so two concurrent ones would
  approach the runner's memory.
- **Browser acceptance**: each `browser-shard-<i>` artifact holds
  Playwright's JSON report of that shard (`shard-report/report.json`):
  `stats.duration` is the shard's run and each result's `duration` covers one
  test, including the fixture it starts; their difference is time outside
  test bodies. `browser-report` prints a per-shard table (tests, wall time,
  summed test time) in its step summary, and the critical path is
  `browser-prepare` plus the slower shard plus `browser-report`. Runner
  minutes are the sum over all five jobs.

## Prerequisites

Linux CI installs its existing native and Office prerequisites through
`.github/scripts/install_ubuntu_packages.py`. GitHub's Ubuntu image already
uses `/etc/apt/apt-mirrors.txt`; the installer selects the official HTTPS
Ubuntu archive directly, retaining the image's suites, components and signing
keyrings. Each APT connection/read has a 30-second idle timeout and each file
has at most two retries. An index or package failure fails the job; stale
indexes never stand in for a failed refresh. This avoids spending the test
budget waiting for Azure mirror failover (37 min 43 s in run 39780556818,
attempt 1). Package requirements, sandbox admission, test selection, assertions
and job/test deadlines are unchanged. The installer is covered by the existing
`.github/scripts/test_*.py` checks that execute on every CI run.

Mandatory prerequisites fail closed; only explicitly opt-in tooling may
skip:

- **Real executables.** Tests that exec `bash-supervisor`,
  `interactive-supervisor` or `rustx` need `cargo build --bins` first;
  integration targets receive `CARGO_BIN_EXE_*` from Cargo. A missing
  executable fails the spawn.
- **A verified case-insensitive filesystem on macOS.** The `process` target's
  reserved-configuration-name startup regression probes the temporary
  volume using file identity. If it is case sensitive, the fixture uses
  system `hdiutil` to create and mount an isolated case-insensitive APFS
  image. It verifies that the literal lowercase metadata path resolves to
  each physically authored alias before testing rejection; unavailable
  fixtures fail rather than skip. Both macOS external-boundary lanes execute
  this test, including the no-runtime-storage and unchanged-document checks.
- **uv and Python 3.12** (the provider emulator and managed `FastMCP`
  children). Every uv-dependent Rust test resolves uv through
  `common::provider_emulator::required_uv`: a local checkout without uv
  skips with a "was NOT exercised" message, and with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR` set a missing uv fails. CI sets it on
  every step that runs such a boundary. The TUI integration suite applies
  the same variable to uv and to the built `rustx` binary.
- **`python3` on the supervised PATH** — required, never skipped, by the
  Linux process-containment escape regressions.
- **Container engine** — `pnpm test:e2e` requires Docker or Podman
  (`CONTAINER_ENGINE`) on Linux x86_64 and fails otherwise.
- **Opt-in only:** the `#[ignore]`d live provider smoke tests (credentials),
  measurements and fixture generators. They never run in CI and are not
  correctness coverage.

Every Rust-bearing lane restores a `Swatinem/rust-cache@v2` lineage of its
own before its first cargo command, named by a per-lane `shared-key`. Two
properties keep that an optimization rather than a semantic input:

- the cache holds the **dependency graph only** — workspace crates are
  excluded, so rustX itself and all of its test code are compiled from the
  checked-out source on every run;
- a cold, stale, or evicted cache changes how much dependency compilation is
  repeated and nothing else. It can never decide which tests are selected or
  what they prove, and a completely cold runner remains a supported CI state.

Sibling lanes deliberately do not share one key: GitHub cache entries are
immutable and the lanes start concurrently, so a single shared key would let
whichever lane finishes first freeze its own smallest `target/` as the entry
every other lane then restores and could never correct.

Cache *ownership* is separate from cache *use*: every lane may restore a
compatible cache, but only a trusted push to `main` publishes one. Each cache
step carries
`save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}`,
so pull requests — including those from forks — are consumers only. The
reuse path is `main` cache → a later run with a compatible key, which keeps
the repository's shared cache budget owned by one trusted publisher instead
of every PR.

## Where does a new test belong?

First decide the **semantic class** by the invariant, not by where similar
code lives:

- **The invariant is a real process/signal/stdio/shell/filesystem
  boundary** → boundary conformance. If it needs a private `cfg(test)`
  seam, add it to the owning `tests/boundary/<domain>/` suite; otherwise it
  belongs to an external integration target.
- **The invariant is provable with scripted adapters / manual clocks /
  in-memory fixtures** → deterministic contract. If it needs a private
  seam, the owning `tests/scripted/<domain>/` suite; otherwise a
  source-module unit test or the pure `contracts`/`provider` target.

Then the owning domain:

- **A new Agent Loop invariant** (settlement, lifecycle, retry, deadline,
  cancellation, ordering): `tests/scripted/agent/`, using
  `support::fake`/`support::model` fixtures and the shared settlement
  assertions in `support::audit`.
- **A provider adapter translation/stream/error-mapping assertion**:
  `tests/provider/` over the in-process `FixtureServer`. Never route a
  one-field wire assertion through the emulator process.
- **A contract that only holds when the real runtime composes with a real
  provider boundary**: `tests/conformance/` over the provider emulator.
- **A durability/recovery contract**: `tests/durable/` (SQLite crash prefix
  + reopen); if the invariant is death of a real *process*, the FND-06
  matrix in `tests/boundary/durable/process_death/`.
- **Context planning/projection**: `tests/scripted/context/engine.rs`.
  **Compaction pipeline atomicity**: `tests/scripted/context/compaction_pipeline.rs`.
  **Runtime integration** (proactive compaction, overflow recovery,
  continuation invalidation): `tests/scripted/context/runtime_integration.rs`
  — prove the invocation at the boundary and rely on the pipeline owner for
  the internal transition; do not re-run the compaction algorithm through
  every runtime scenario.
- **A Subagent test**: only if it proves a boundary the Agent Loop suites
  cannot see (authority crossing, registry lifecycle, terminal notice,
  cross-boundary cancellation/drain, real process handshake). If the same
  test would pass verbatim against a non-child runtime, it belongs to
  `agent/` instead.
- **A feature integration test** should assert the lower layer's observable
  boundary result and rely on the lower layer's owner suite for the state
  machine itself.

Create a **new integration target** only when no existing target's
dependency topology fits — e.g. a new external boundary with its own
fixture process. Never split a target merely because a file grew. Assign a
new target to a lane in `.github/workflows/ci.yml` in the same change; the
lane check fails until an executing step selects it.

## Determinism rules

For race/cancellation/order tests — in *both* in-crate trees:

- identify the actual linearization point (a durable commit, a watch
  transition, a gate release) and synchronize on it with channels, watches,
  barriers, manual clocks, durable facts, or `#[cfg(test)]` hooks;
- never use `sleep` to manufacture an interleaving;
- wall-clock timeouts are outer liveness guards only — their expiry is a
  harness failure, never a verdict.

Provider request arrival does not prove that the runtime has consumed a
stream item. Before choosing a quiet observation baseline or injecting a
source fault, synchronize with the scripted producer's relevant boundary,
then wait for the projection to settle. Projection settlement drains current
input; it cannot fence an event the producer has not emitted yet.

Boundary suites are not exempt: the process-death harness freezes the child
at instrumented durable transitions or a control rendezvous before the
kill; it never infers a race from timing.

Shared settlement assertions (`support::audit`): exactly one attempt
terminal, terminal-is-last, outcome/terminal-fact correspondence, exact
trace comparison. Use them instead of restating the generic lifecycle in a
feature suite.

## Running

During implementation run the focused tests of what you change; at a
checkpoint, run the affected modules; let CI's FAST run verify each push,
and promote the final candidate to FULL instead of repeating the whole
suite locally. CI verifies only committed, pushed heads — never local edits.

```bash
# Focused: one module, one target, one test.
cargo test --lib --all-features durable::sqlite::
cargo test --all-features --test process app_server::
(cd web-console && pnpm exec vitest run test/model-retry.test.tsx)

# The FAST Rust lane (CI: rust-fast), without the affected heavy groups:
cargo test --all-features --no-run --lib --test process
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- \
  --skip boundary_suites:: --skip ::scale:: --skip durable::sqlite:: \
  --skip local_runtime::session_runtime_manager::tests:: \
  --skip local_runtime::session:: --skip runtime_client::response::
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test process

# The capacity suite (FULL), or one capacity test:
cargo test --lib --all-features ::scale::

# FULL: the lanes below. Promote a pull request with the
# `full-verification` label to run all of them in CI on its exact head.

# Unit tests + in-crate deterministic contracts (CI: rust-contracts):
cargo build --bins   # unit tests exec target/debug/{bash,interactive}-supervisor
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features

# Boundary lanes (CI: rust-boundaries); uv is mandatory. Compiling the
# external targets first also places the real executables at target/debug/,
# where the text_spill suite execs bash-supervisor:
BOUNDARY_TARGETS='--test durable --test process --test subagent --test tools
  --test conformance --test cfg3_catalog --test cfg3_managed_output'
cargo test --all-features --no-run $BOUNDARY_TARGETS
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features $BOUNDARY_TARGETS

# macOS core (CI: rust-platform-core): the seven external boundary targets
# above. macOS full (CI: rust-platform-boundaries) adds, after the same
# external-target compilation:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --bins --all-features -- \
  --skip scripted_suites:: \
  --skip local_runtime::session_runtime_manager::tests::

# Lane coverage for one CI job on its own platform, or every target here
# (--all-features reuses the library the test targets already compiled):
cargo run --all-features --example check_test_lanes -- --job rust-contracts
cargo run --all-features --example check_test_lanes

# Everything, the safety net (it does not prove the CI lanes' selection):
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features

# One domain target:
cargo test --test durable --all-features

# The in-crate scripted contract suites only:
cargo test --lib --all-features scripted_suites::

# The App Server contracts under the manager's private test module:
cargo test --lib --all-features local_runtime::session_runtime_manager::tests::
```
