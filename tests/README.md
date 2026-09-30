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
  `stage_profile_real_create_pipeline`) and the Web
  `pnpm test:issue-420-performance` counters. They record numbers for an
  issue report, assert no timing threshold and run only explicitly.
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
  projection, token accounting, compaction planning/span selection, driven
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
  endpoint and the stdio/JSONL framing over in-memory pipes.
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
  durable, a retry's next request projects retry ordinal zero, activity
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
| `process` | the real `rustx` binary / local composition | stdio/JSONL transport over a spawned process, composition identity, capability startup isolation, sessions, runtime config, committed-example resource composition |
| `subagent` | the child-process boundary | named definition admission/resolution, frozen-policy handshake through a real launched child, end-to-end parent/child composition |
| `tools` | OS/tooling boundary | Bash supervision, Read/Write/Edit/Grep/Glob, Skills, MCP config/runtime, uv backend, managed `FastMCP` materialization |
| `cfg3_catalog` | real configuration source files | CFG3 source resolution, shadowing and replacement, native writers' commit races and stale-write refusal, frozen model/provider invocation identity |
| `cfg3_managed_output` | managed Tool output storage | execution-identity locators, collision refusal without overwrite, spill reconstruction |
| `contracts` | none (pure) | serialization fixture round-trips, committed configuration examples |

## How CI selects the classes

The CI jobs (`.github/workflows/ci.yml`) mirror the semantic classes. The
workflow's `cargo test` steps are the only selection source; this table
summarizes them and the lane check below verifies them against Cargo:

| Suite / group | Class | Linux lane | macOS |
| --- | --- | --- | --- |
| source-module unit tests (`src/**`) | unit | rust-contracts | yes |
| `scripted_suites::` | deterministic contracts | rust-contracts | skipped |
| `local_runtime::session_runtime_manager::tests::` | deterministic contracts | rust-contracts | skipped |
| `boundary_suites::` | boundary conformance | rust-boundaries | yes (required) |
| bin and example harnesses | unit (examples: workload checkers, lane checker) | rust-contracts | bins only |
| `contracts`, `provider` targets | pure contracts / adapter translation | rust-contracts | no |
| `durable`, `process`, `subagent`, `tools`, `conformance`, `cfg3_catalog`, `cfg3_managed_output` | boundary conformance | rust-boundaries | yes (required) |
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
- **rust-boundaries** (ubuntu) — the emulator's pytest suite,
  `cargo build --bins` (the text_spill suite execs `bash-supervisor`),
  `cargo test --lib --all-features -- boundary_suites::`, then
  `cargo test --all-features --test durable --test process --test subagent
  --test tools --test conformance --test cfg3_catalog --test
  cfg3_managed_output`. Both test steps set
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`.
- **rust-platform-boundaries** (macos) — only platform-sensitive classes,
  natively: `cargo build --bins --all-features`, then
  `cargo test --lib --bins --all-features -- --skip scripted_suites::
  --skip local_runtime::session_runtime_manager::tests::`
  (unit tests — including the boundary-owning bash/uv modules, whose
  primitives differ across OSes — plus the in-crate boundary suites) and the
  seven external boundary targets, both with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. The `--no-run` compile steps before
  them exist for native timing measurement and execute nothing.
  `contracts`/`provider` and the deterministic contract majority are
  Linux-only: they carry no process or filesystem semantics. Overlap with
  Linux is intentional: a macOS boundary test is native proof, not a
  duplicate.
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
- **web-console** — development launcher `typecheck`/`test`, Web
  `typecheck`, deterministic `pnpm test` (Vitest over `test/**/*.test.ts(x)`;
  the `*.measurement.tsx` counters are excluded), `check:i18n`,
  `check:provenance` (source provenance and dependency notices), then
  `cargo build --bins` and `pnpm test:e2e`. `test:e2e` is
  `pnpm build && bash scripts/browser-tests.sh`: the job's only production
  build (Vite, then `provenance.ts --artifact` over the new `dist/`) runs
  from checked-in source immediately before Playwright, whose preview server
  serves that `dist/`, in the digest-pinned browser container. No earlier
  step builds or reuses `dist/`.

### The lane check

`examples/check_test_lanes.rs` keeps CI selection honest without becoming a
second copy of it. Each Rust test lane ends with
`cargo run --example check_test_lanes -- --job <job id>`, which reads the
workflow's executing `cargo test` steps and asks libtest, through `--list`,
what each one selects — so filters, `--skip`, features and `cfg` are
Cargo's and libtest's own semantics. It fails for:

- a Cargo test target (lib, bin, example, integration test) selected by no
  executing Linux step (`--no-run` does not count);
- a Linux-discovered runnable test that no Linux step executes;
- a step that selects no runnable test of a lib or integration-test target;
- a positive filter that matches nothing, only `#[ignore]`d tests, or only
  `--skip`ped tests, and a `--skip` that excludes nothing;
- a suite in its `REQUIREMENTS` table (macOS: `boundary_suites::` and the
  seven external boundary targets) that is missing or not fully executed on
  its platform;
- a step or libtest output it cannot interpret exactly.

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

## Prerequisites

Mandatory prerequisites fail closed; only explicitly opt-in tooling may
skip:

- **Real executables.** Tests that exec `bash-supervisor`,
  `interactive-supervisor` or `rustx` need `cargo build --bins` first;
  integration targets receive `CARGO_BIN_EXE_*` from Cargo. A missing
  executable fails the spawn.
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

Boundary suites are not exempt: the process-death harness freezes the child
at instrumented durable transitions or a control rendezvous before the
kill; it never infers a race from timing.

Shared settlement assertions (`support::audit`): exactly one attempt
terminal, terminal-is-last, outcome/terminal-fact correspondence, exact
trace comparison. Use them instead of restating the generic lifecycle in a
feature suite.

## Running

```bash
# Unit tests + in-crate deterministic contracts (CI: rust-contracts):
cargo build --bins   # unit tests exec target/debug/{bash,interactive}-supervisor
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features

# In-crate boundary conformance (CI: rust-boundaries); uv is mandatory:
cargo build --bins   # text_spill execs the bash-supervisor binary by path
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::

# External boundary targets (CI: rust-boundaries); the emulator is mandatory:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features \
  --test durable --test process --test subagent --test tools --test conformance \
  --test cfg3_catalog --test cfg3_managed_output

# Platform-sensitive classes on macOS (CI: rust-platform-boundaries):
cargo build --bins --all-features
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --bins --all-features -- \
  --skip scripted_suites:: \
  --skip local_runtime::session_runtime_manager::tests::
# plus the seven external boundary targets above.

# Lane coverage for one CI job on its own platform, or every target here:
cargo run --example check_test_lanes -- --job rust-contracts
cargo run --example check_test_lanes

# Everything, the safety net (it does not prove the CI lanes' selection):
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features

# One domain target:
cargo test --test durable --all-features

# The in-crate scripted contract suites only:
cargo test --lib --all-features scripted_suites::

# The App Server contracts under the manager's private test module:
cargo test --lib --all-features local_runtime::session_runtime_manager::tests::
```
