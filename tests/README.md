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

| Namespace | Semantic class | Physical placement |
| --- | --- | --- |
| `scripted_suites::` (`tests/scripted/`) | deterministic contracts | in-crate lib test |
| `boundary_suites::` (`tests/boundary/`) | boundary conformance | in-crate lib test |

The namespace prefix is the stable semantic marker and is part of the CI
contract: jobs select or exclude classes by prefix
(`cargo test --lib -- boundary_suites::` / `-- --skip boundary_suites::`).

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
  settlement, transient retry with frozen replay, model deadlines,
  unresolved-output carryover, publication interaction at the loop boundary,
  and the Agent-Loop half of the malformed-tool-proposal boundary (bounded
  one-regeneration recovery, canonical-history exclusion, budget composition).
  Which *provider* evidence becomes a malformed proposal is owned by the
  `provider` target instead.
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
| `tools` | OS/tooling boundary | Bash supervision, Read/Write/Edit/Grep/Glob, Skills, MCP config/runtime, uv backend |
| `contracts` | none (pure) | serialization fixture round-trips, committed configuration examples |

## How CI selects the classes

The CI jobs (`.github/workflows/ci.yml`) mirror the semantic classes:

- **quality** (ubuntu) — fmt, clippy, whitespace.
- **rust-contracts** (ubuntu) — unit tests + in-crate deterministic
  contracts + pure contract targets:
  `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::`
  and `cargo test --test contracts --test provider --all-features`.
  A `cargo build --bins` step comes first: the bash tool's unit tests exec
  `target/debug/bash-supervisor`, and `cargo test --bins` builds the bin
  test harnesses but does not place the executable there.
- **rust-boundaries** (ubuntu) — in-crate boundary suites + external
  boundary targets + the provider emulator's pytest suite:
  `cargo build --bins` (the text_spill suite execs `bash-supervisor`),
  `cargo test --lib --all-features -- boundary_suites::`, then
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable
  --test process --test subagent --test tools --test conformance`.
- **rust-platform-boundaries** (macos) — only platform-sensitive classes:
  `cargo test --lib --bins --all-features -- --skip scripted_suites::`
  (unit tests — including the boundary-owning bash/uv modules, whose
  primitives differ across OSes — plus the in-crate boundary suites; the
  deterministic scripted contract majority is *not* rerun on macOS) plus
  the five external boundary targets with the emulator mandatory.
  `contracts`/`provider` are Linux-only: deterministic JSON/SSE translation
  with no process or filesystem semantics.
- **tui** — Node/pnpm, independent of the Rust jobs.

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
fixture process. Never split a target merely because a file grew.

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
cargo build --bins   # the bash tool's unit tests exec target/debug/bash-supervisor
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features

# In-crate boundary conformance (CI: rust-boundaries):
cargo build --bins   # text_spill execs the bash-supervisor binary by path
cargo test --lib --all-features -- boundary_suites::

# External boundary targets (CI: rust-boundaries); the emulator is mandatory:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features \
  --test durable --test process --test subagent --test tools --test conformance

# Platform-sensitive classes on macOS (CI: rust-platform-boundaries):
cargo build --bins
cargo test --lib --bins --all-features -- --skip scripted_suites::
# plus the five external boundary targets above.

# Everything, the safety net:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features

# One domain target:
cargo test --test durable --all-features

# The in-crate scripted contract suites only:
cargo test --lib --all-features scripted_suites::
```
