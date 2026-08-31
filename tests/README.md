# rustX test architecture

rustX tests are organized by the runtime layer that **owns the invariant**
under test, never by the issue or milestone that introduced them. Every
important architectural invariant has exactly one authoritative owner suite;
other suites may prove that the invariant crosses their boundary, but they
do not re-prove the lower-layer state machine.

## The four test classes

1. **Unit tests** (`src/**`, `#[cfg(test)]` modules) — pure/local behavior
   owned by one source module.
2. **Deterministic contract tests** (`tests/scripted/`, compiled into the
   crate's own test build) — cross-module runtime contracts driven by
   scripted models/tools, manual clocks, explicit channels/barriers/watches,
   and `#[cfg(test)]`-only synchronization seams.
3. **Boundary conformance tests** (`tests/<domain>/main.rs` integration
   targets) — behavior that genuinely requires an external boundary:
   provider protocol translation, SQLite durability/recovery, real
   process/IPC/stdio, the filesystem, the external provider emulator.
4. **Opt-in live tests** (`tests/provider/live.rs`) — real credentialed
   provider smoke tests, always `#[ignore]`d. They are never correctness
   authority for generic runtime semantics.

## Why the scripted suites compile into the crate's test build

Two seams may not exist in the published API: a scripted `ModelAdapter`
behind a real catalog binding, and a scripted `ContextSummarizer` behind a
real context runtime. An external integration-test binary can only reach
`pub` items, so these suites compile into the crate's own test build via
`src/lib.rs` (`#[path = "../tests/scripted/mod.rs"]`), where the seams are
`#[cfg(test)] pub(crate)`. Their sources stay under `tests/` so `src/`
carries production code only. Cargo auto-discovers integration targets from
`tests/*.rs` and `tests/*/main.rs` only, so nothing under `tests/scripted/`
is also built as a separate test binary.

## Integration targets and what each one owns

A separate Cargo integration-test binary exists because it represents a
meaningful domain/boundary/dependency topology:

| Target | Boundary | Suites |
| --- | --- | --- |
| `provider` | one adapter over the in-process `FixtureServer` | request serialization, protocol translation, stream parsing, normalized error mapping, capability/context translation at the adapter, opaque request params, opt-in live smoke |
| `conformance` | the external provider-emulator process | composed Agent Loop / lifecycle / Workflow conformance through the real runtime and a real provider boundary |
| `durable` | file-backed SQLite | recovery classification, pending-inbound inbox, publication store contract, interaction audit store, transcript history |
| `process` | the real `rustx` binary / local composition | stdio/JSONL transport over a spawned process, composition identity, capability startup isolation, sessions, runtime config |
| `subagent` | the child-process boundary | named definition admission/resolution, frozen-policy handshake through a real launched child, end-to-end parent/child composition |
| `tools` | OS/tooling boundary | Bash supervision, Read/Write/Edit/Grep/Glob, Skills, MCP config/runtime, uv backend |
| `contracts` | none (pure) | serialization fixture round-trips, committed configuration examples |

Fixtures shared by integration targets live in `tests/common/`; fixtures
that need a `cfg(test)` seam live in `tests/scripted/support/`.

## Scripted domain ownership

`tests/scripted/` mirrors runtime ownership:

- `agent/` — **the owner of generic execution semantics**: attempt state
  machine, terminal uniqueness/terminal-last, `AttemptOutcome`
  correspondence, request start/settlement lifecycle, exact request counts
  and ordinals, canonical Assistant commit rules, tool lifecycle and
  canonical result ordering, cancellation arbitration and structural
  settlement, transient retry with frozen replay, model deadlines,
  unresolved-output carryover, publication interaction at the loop boundary.
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
  endpoint and the stdio/JSONL framing.
- `capability/` — capability snapshots, quiescent commits, environment
  materialization.
- `interaction/` — the durable interaction audit's runtime half.
- `background/` — the background registry contracts and the `execution`
  intrinsic control plane.
- `subagent/` — **only the child ownership boundary**: frozen authority
  crossing into the child, parent registry lifecycle, exactly one terminal
  child notice, parent isolation from child-internal retry/deadline state,
  cancellation/drain across the boundary. A child is an ordinary
  `ConversationRuntime`; generic retry/deadline/cancellation/settlement
  semantics belong to `agent/` and must not be replayed here.
- `tools/` — native registry contracts and the conversation task list.
- `durable/` — real process-death conformance (a killed real process at a
  named durable boundary).

## Where does a new test belong?

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
  + reopen) unless it needs the scripted seams, in which case the runtime
  half belongs to the owning scripted domain.
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

For race/cancellation/order tests:

- identify the actual linearization point (a durable commit, a watch
  transition, a gate release) and synchronize on it with channels, watches,
  barriers, manual clocks, durable facts, or `#[cfg(test)]` hooks;
- never use `sleep` to manufacture an interleaving;
- wall-clock timeouts are outer liveness guards only — their expiry is a
  harness failure, never a verdict.

Shared settlement assertions (`support::audit`): exactly one attempt
terminal, terminal-is-last, outcome/terminal-fact correspondence, exact
trace comparison. Use them instead of restating the generic lifecycle in a
feature suite.

## Running

The CI jobs mirror the test classes (see `.github/workflows/ci.yml`):

```bash
# Fast deterministic contracts (CI: rust-contracts):
cargo build --bins   # the lib test binary execs bash-supervisor/rustx by path
cargo test --lib --bins --examples --all-features
cargo test --test contracts --test provider --all-features

# Real boundaries (CI: rust-boundaries); the emulator is mandatory:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features \
  --test durable --test process --test subagent --test tools --test conformance

# Platform-sensitive classes on macOS (CI: rust-platform-boundaries):
# the lib binary (process-death/bash/SQLite-touching scripted suites) plus
# the five boundary targets. contracts/provider are Linux-only: deterministic
# JSON/SSE translation with no process or filesystem semantics.

# Everything, the safety net:
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features

# One domain target:
cargo test --test durable --all-features

# The in-crate scripted suites only:
cargo test --lib --all-features scripted_suites::
```
