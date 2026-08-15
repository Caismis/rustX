# Development Plan

This plan prioritizes proving the execution kernel locally before integrating rustX into production infrastructure.

## Milestone 0 — Repository foundation

Deliverables:

- Minimal Rust crate
- Formatting and linting baseline
- Architecture documentation
- Runtime invariants
- Deterministic test structure

Exit criteria:

- `cargo check`, `cargo fmt --check`, and `cargo clippy` can be introduced without restructuring the repository.
- The module boundaries are explicit and documented.

## Milestone 1 — Canonical runtime model

Implement runtime-owned types for:

- `SystemMessageBlock`
- `UserMessageBlock`
- `AssistantMessageBlock`
- `ToolMessageBlock`
- Content blocks
- Tool definitions, calls, and results
- `ModelRequest`
- `ModelEvent`
- `RuntimeEvent`
- `RuntimeManifest`

Implemented in M1 as the Layer 0 contracts described in
[`docs/architecture.md`](architecture.md) section 2.1, with deterministic
serialization fixtures under `tests/fixtures/m1/` and round-trip contract
tests in `tests/m1_contracts.rs`.

Exit criteria:

- No provider SDK type appears in the canonical model.
- Types serialize deterministically where persistence is required.

## Milestone 2 — Model execution

Implement model adapters for:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Responsibilities:

- Streaming normalization
- Tool-call normalization
- Reasoning normalization with provider continuation state
- Usage normalization
- Error normalization
- Cancellation propagation

Exit criteria:

- A local CLI can stream a single real model response through canonical `ModelEvent` values.
- Adapter tests cover text, tool calls, reasoning, usage, errors, and cancellation.

## Milestone 3 — Agent loop

Implement the attempt and turn state machines.

Features:

- Multi-turn conversation
- Model -> tool -> model loop
- Sequential and parallel tool batches
- Deterministic tool-result ordering
- Attempt termination rules
- Turn-boundary inbound-message drain point (implemented by Issue #22 as a
  safe-boundary mailbox drain)
- Mock model executor for deterministic tests

Exit criteria:

- A deterministic fixture can execute a complete multi-turn tool-using agent without network access.
- A live local session can hold a normal multi-turn conversation.

M3's sequential/parallel tool-batch scheduling and deterministic
tool-result ordering are implemented by the M5 tool plane PR: a
`Sequential` invocation is an exclusive scheduling barrier, adjacent
`Parallel` invocations execute concurrently as one group, and canonical
results are committed in model call order.

## Milestone 4 — Context engine and compaction

Implemented in PR #21 (see [`docs/context-engine.md`](context-engine.md)):

- Context Engine projection and the explicit `ContextProjection` boundary;
  unified Context Assembly is specified by M7.5b below
- Token accounting with explicit provenance (provider-reported vs.
  deterministic estimate) and a pluggable `TokenEstimator` (default
  `ceil(bytes / 4)`); the anti-loop progress rule compares deterministic
  estimates on both sides
- Provider-context compilation into canonical `ModelRequest.messages`
- Automatic compaction at the derived soft input limit
  (`window - reserve - max_output_tokens`, checked arithmetic)
- Valid structural cut-point detection with a tool-call/result edge index
- No cuts at tool-result boundaries; no orphan tool messages
- Recent-token retention by token target, not message count, measured over
  conversation content only (tool definitions never satisfy the target)
- Whole-message compaction only: no partial Assistant projection or split
  Assistant/tool structural unit
- Message Ledger + Conversation Surface architecture from Issue #54:
  immutable canonical facts, current active identities/order/visibility, and
  exact `SurfaceRevision` reconstruction
- One canonical runtime User compaction summary plus one Surface Replace;
  `ContextCheckpoint` no longer owns summary or projection truth, and no
  separate summary authority exists
- Actual summary-model request bounding through the shared deterministic
  `SummaryRequest::model_input()` assembly, using the summary invocation's
  own context window and output budget
- Bounded compact-and-retry on `ContextWindowExceeded` (exactly one retry
  per model turn)
- Continuation invalidation after successful incompatible Surface replacement;
  explicit failure when the continuation-owning turn cannot be retired under
  the bounded #54 System rule
- Mandatory Agent Status input: explicit `FreshInboundTurn` identity
  with a mandatory canonical-order validation and an explicit
  `InitialTurnTrigger` (fresh inbound vs pure continuation), structured
  section composition with reserved ids and registration-frozen section
  identities, the mandatory temporal section (clock + IANA timezone), the
  canonical deterministic renderer over structured extension facts, the
  canonical Runtime context UserMessageBlock admitted through Context
  Assembly, full canonical token accounting, fresh-inbound compaction
  protection, and a
  `ContextPreparationFailed`/`ContextCompactionFailed` failure distinction
- Agent Status integration with Issue #22 inbound batching: one drained
  batch becomes one fresh inbound turn with exactly one admitted status fact
  targeting the final message
- Opt-in live repeated-compaction validation (`tests/m4_live.rs`)

The M1 `ContextManifest` gained `context_window_tokens` (additive pre-1.0
contract change; fixture and round-trip tests updated).

Issue #7 (M4: context engine and Agent Status) is **completed**; Issue #27
owns the deferred live multi-compaction/TUI verification, and Issue #8 (M5)
owns the background-execution Agent Status integration, which is implemented
by the M5 tool plane PR as a runtime-owned built-in section.

Issue #22 (inbound batching) is implemented in the Issue #22 PR: the
conversation inbound mailbox foundation (`src/runtime/inbound.rs`),
including the canonical `UserMessageBlock.timestamp`, the shared
`InboundSequence` domain, atomic enqueue, the finite watermark-bounded
drain, safe-boundary agent-loop integration, and the deterministic
mailbox/race/agent-loop/M4/provider test coverage. The remaining
cross-issue acceptance work — Agent Status integration with the drained
batch — is implemented by the Agent Status PR (issue-7/agent-status).
Background runtime producers are implemented by Issue #8 (M5), and mailbox
persistence/recovery remains later milestone work.

Exit criteria:

- A long local session can compact multiple times and continue correctly.
- Compaction never rewrites or deletes canonical history.
- Deterministic fixtures cover normal and repeated whole-message compaction,
  exact historical Surface reconstruction, and equal-content identity.
- Fresh inbound material is never compacted before a successful model
  invocation observes it; preserving it or failing explicitly with
  `CannotFit` are the only two outcomes.

Deferred to later milestones: durable Ledger/Surface/event storage (M8),
conversation summarization in the CLI (M10), and any provider fallback or
routing. Parallel tool scheduling is implemented by the M5 tool plane PR;
the turn-boundary mailbox drain is implemented in the Issue #22 PR as a
safe-boundary contract.

## Milestone 5 — Native tool plane

Implemented in the M5 tool plane PR (Issue #8):

- The canonical `ToolExecutor` contract and validating `ToolRegistry`
  (definition/executor ownership, registration validation, deterministic
  model-facing ordering, preflight boundary with JSON Schema validation)
- Two independent policy axes: `ToolExecutionPolicy`
  (foreground/background ownership) and `ToolConcurrencyPolicy`
  (sequential/parallel batch scheduling)
- The compiled `ModelToolDefinition` with the reserved `__rustx_`
  invocation namespace (`__rustx_execution` for model-selectable tools)
- `ToolExecutionId` and the conversation-owned
  `ConversationBackgroundRegistry` with the dispatch ownership commit,
  lifecycle state machine, cancel-vs-complete linearization, and
  exactly-once terminal inbound publication
- The `background_task` runtime intrinsic (status and idempotent cancel)
- The runtime-owned Agent Status `background_execution` built-in section
- Native Read, Write, Edit, Glob, Grep, and Bash tools plus the workspace
  boundary, artifact store, and explicit tool environment. Their
  model-facing schemas follow established Pi coding-agent conventions
  (`read` takes `offset`/`limit`, `edit` takes an atomic `edits` array of
  `oldText`/`newText`, `grep` takes `ignoreCase`/`literal`/`context`/
  `limit`, `bash` takes a `timeout` in seconds); Glob and Grep share one
  private Rust-native search substrate built on the ripgrep crates, with no
  `rg` executable dependency
- The concrete bounded `NativeToolPolicies` configuration: each ordinary
  native tool independently selects its `ToolInvocationPolicy` (execution +
  concurrency axes; foreground-only sequential by default), with
  `background_task` fixed foreground-only sequential outside the
  configurable set
- One canonical conversation mailbox owned by the conversation tool
  runtime, drained by the Agent Loop at every safe boundary; a configured
  mailbox must belong to the runtime's own conversation (construction-time
  identity check)
- Artifact storage structurally disjoint from the model workspace
- Deterministic foreground/background scheduling through the agent loop
  with structural batch settlement

Bash requirements:

- Full `/bin/bash`
- Foreground and background execution
- One per-invocation supervisor process unit (outer reaper-of-last-resort
  plus inner session/group leader; both subreapers)
- stdout/stderr/combined capture
- Timeouts
- `TERM -> grace period -> KILL` driven by the invocation supervisor
- Complete lifecycle ownership: shell-parent exit is not the Bash
  settlement boundary. The invocation settles naturally only when the
  shell's terminal status is known, the invocation-owned process group
  reached its kernel-mediated terminal state, and the capture is settled;
  a live in-group descendant (pipes held or redirected away) keeps the
  invocation active under the same deadline and cancellation until the
  owned group is terminal or cancellation/timeout/process-control failure
  settles it
- **Fixed process-group-scoped ownership**: the invocation's ownership
  boundary is its dedicated process group, and membership is immutable for
  Bash descendants. The inner supervisor installs a narrow inherited
  seccomp policy (after its own `setsid()` setup, before the `/bin/bash`
  spawn) that rejects `setsid`/`setpgid` with `EPERM` — the only syscalls
  that can change process-group/session membership on Linux — so a
  descendant can never leave the group or hide an in-group process behind
  an out-of-group ancestor. A `setsid` escape attempt fails deterministically;
  subreaper adoption of such children is a reaping detail, not an ownership
  claim
- Target-ABI seccomp policy: membership syscall numbers come from the
  compiled Linux target's libc constants; x86-64 rejects the x32 syscall
  namespace explicitly because it shares `AUDIT_ARCH_X86_64`
- Reuse-safe process-group ownership: `TERM`/`KILL` are issued by the
  inner supervisor with `killpg` against its own process group, whose
  numeric id is its own pid — provably allocated while it lives; the final
  signal is the last `killpg`, after which the anchor is released by the
  reap and no further signal exists
- Kernel-mediated group terminality: shell descendants that outlive the
  shell are reparented into the invocation supervisor's child domain
  (`PR_SET_CHILD_SUBREAPER`), and the terminal point is the group-scoped
  wait (`waitid` with `Id::PGid`) returning `ECHILD` at the outer
  supervisor — a complete whole-group proof only because membership is
  immutable (an in-group process is always a matching child of the
  supervisor that owns the gate) — never a `/proc` membership scan and
  never a `killpg(..., 0)` probe (an un-reaped leader zombie keeps the
  numeric group observable)
- Explicit ownership protocol: `AnchorReady -> Start ->
  OwnershipEstablished`; the successful Bash spawn is the OS commit point,
  and post-start channel loss is conservatively treated as possible
  ownership. `NoOwnership` covers pre-spawn setup failure.
- Control-channel EOF is never post-ownership terminality. Normal settlement
  uses `AllChildrenReaped`; catastrophic supervisor loss uses rustX's own
  subreaper adoption, retained `WNOWAIT` anchor, anchored group containment,
  and group-scoped `ECHILD` proof before returning `Failed`. An
  `AnchorUnavailable` result (adopted anchor `ECHILD` without a prior
  terminal event) is never a terminal proof and never commits a result.
- Single-reaper anchor ownership: the inner supervisor pid is an ownership
  anchor with exactly one reaping owner (the outer's dedicated anchor path
  in the normal lifecycle; rustX's adopted-anchor path after both
  supervisors are lost). The outer supervisor has no generic `waitpid(-1)`
  reaping loop — generic child reaping can never consume the invocation
  anchor, and an anchor `ECHILD` before the intentional release is an
  ownership invariant violation, never process terminality.
- Runtime child-subreaper capability: rustX's process-wide
  `PR_SET_CHILD_SUBREAPER` activation is a runtime-level kernel
  coordination primitive (lazy one-time, idempotent, sticky activation;
  owned by `src/runtime/process_supervision.rs`), established before
  `START` and never toggled per invocation. It is the catastrophic
  fallback authority for Bash supervisor units only — in M5, Bash is the
  only production subprocess hierarchy relying on orphan adoption, no
  generic unknown-child reaper exists, and catastrophic Bash containment
  remains invocation-scoped (anchor pid and invocation PGID only, never a
  broad wait), so concurrent invocations and unrelated adopted children
  are never signaled or reaped cross-group. Any future production
  subprocess hierarchy must define its process-supervision/reaping
  ownership before introduction.
- Process-control failures (supervisor setup, shell spawning,
  waiting/reaping, signaling, IPC, SIGTERM handler installation, fixed-
  membership restriction installation) are
  explicit failed results; if ownership of a numeric process group can no
  longer be proven, no further signal is issued and the invocation fails
  explicitly
- Process confirmation watchdog: expiry records `QuiescenceTimeout` failure
  intent but never bypasses process terminality. After terminality, the
  separate capture deadline may force-finalize wedged readers. The outer
  supervisor un-wedges a `SIGSTOP`-frozen inner anchor with `SIGKILL`
- Explicit artifact-capture failures instead of silent success
- Large-output truncation with durable full output artifacts
- Explicit execution environment instead of inherited process environment

Exit criteria:

- Tool batches work through the same agent loop used by mock tools.
- Foreground Bash cancellation is reliable.

## Milestone 6 — Skills (implemented)

Implemented:

- Skill package discovery from the single project-local root
  `<workspace>/.agents/skills/` (one level, deterministic ordering,
  symlink rejection, whole-transaction failure on any malformed candidate)
- Standard Agent Skills `SKILL.md` frontmatter parsing/validation and the
  compact model-visible catalog projection
- Content-derived `SkillVersionId` hashing over the complete package state
- rustX dependency declarations (`rustx.python-dependencies`,
  `rustx.node-dependencies`) with deterministic normalization and
  merge/conflict detection before any package-manager subprocess
- One shared Python environment and one shared Node environment per
  capability set, with distinct `PythonEnvironmentDigest` /
  `NodeEnvironmentDigest` identities. Python is built directly at its final
  digest path and becomes reusable only after an exact deterministic ready
  manifest is atomically committed; Node uses private staging followed by
  atomic rename. Both are immutable after publication in a canonical,
  symlink-safe runtime-private store, and same-process preparations of one
  ecosystem/digest coalesce behind one `EnvironmentStore`-owned in-flight
  build task; candidate callers only wait on the shared result and caller
  cancellation does not release the entry before physical settlement.
- Scripts, references, and assets layout, executed through native
  Read/Bash against the Workspace (no `skill_search`/`activate_skill`/
  `skill_view`/`run_skill`/`run_skill_script`)
- The capability coordination layer (`src/capabilities`): immutable
  attempt capability snapshot, RAII attempt lease, quiescent atomic
  commit, `CapabilityRevision` swap, and background environment capture
- The shared supervised process runner (`src/runtime/process_runner`)
  reusing the M5 Bash process-group lifecycle for Skill environment
  materialization

Exit criteria:

- A skill can instruct the model to read its instructions and execute Python, Node, or shell scripts against the local workspace.
- Multiple skills can coexist in one shared environment with deterministic environment identity.
- An attempt observes one immutable Skill catalog for its complete lifetime.
- A capability commit is rejected while an attempt lease is active; failed preparation/commit leaves the current revision authoritative.
- Detached background executions retain the environment of the revision that dispatched them.

## Milestone 7 — External tool plane (implemented)

### MCP

Use `rmcp` 3.1.2 behind a rustX-owned executor boundary, negotiating a
mutually supported protocol revision instead of pinning one (Issue #46).

Implement:

- Typed stdio and Streamable HTTP configuration with explicit credentials,
  normalized once from the ecosystem-compatible named `mcpServers` map
- Paginated discovery and deterministic canonical ToolId/name ordering
- Shared `McpServerRuntime` ownership for transport, subscriptions, progress,
  cancellation, and supervised stdio process settlement
- Fractional provider-neutral progress, canonical result conversion, and
  response-vs-cancellation linearization
- Monotonic `tools/list_changed` invalidation epochs; refresh preparation and
  quiescent commit, never active-registry mutation
- One shared MCP invalidation synchronization boundary: notification epoch
  mutation, preparation epoch snapshots, and the commit's final epoch
  validation + snapshot swap all serialize through the same mutex-protected
  state, with explicit lock ordering (capability state lock ->
  invalidation guard; the notification path holds only the guard). The
  notification-wins and commit-wins interleavings are proven by
  coordinator-level deterministic regressions.
- The interactive MCP stdio supervisor unit: the M5 Bash supervisor shape
  applied to a long-lived server, composed from the same shared structural
  ownership core (fixed-membership seccomp, group-scoped kernel terminal
  proof, single-owner anchor discipline, TERM/grace/KILL against the inner's
  own group, adopted-anchor emergency containment, driver-owned settlement
  with direct-child reap before publication, EOF-drained bounded stderr).
  Deterministic regressions cover normal shutdown, outliving server
  children, `setsid`/`setpgid` escape attempts, TERM-resistant servers,
  inner-supervisor loss, business-handle drop, post-spawn handshake failure,
  direct supervisor reap, and >64 KiB stderr while the server continues
  operating.

### Custom Python tools

Implement:

- Immutable one-level packages at `<workspace>/.agents/tools/`
- Content-derived `ToolVersionId` plus separate
  `PythonToolEnvironmentDigest`
- Immutable source publication (`tool-versions/<id>/source/` + version
  marker; reuse validates the published source content digest against the
  claimed identity), checked `uv.lock`, and store-owned coalesced
  frozen `uv` materialization with ready metadata that locks every
  deterministic identity input; per-ToolVersion environment bindings are
  recorded outside the environment's immutable dependency identity
- The exact probed interpreter is pinned to uv (`UV_PYTHON`), managed Python
  downloads stay disabled, and every preparation command has a finite
  deadline (timeout = explicit preparation failure)
- Canonical schema preflight, private-file invocation harness, supervised
  process execution, and bounded JSON result normalization
- The M6 environment build-owner coordination pattern for same-digest
  builds: one store-owned logical owner per digest until terminal
  publication, no-lost-wakeup waiters, RAII owner guard, pointer-identity
  in-flight removal, and no overlap between retry and a previous owner

Exit criteria:

- MCP and Python tools are indistinguishable from native tools to the agent kernel.
- A capability revision owns one immutable composed registry. Background
  executions retain exact MCP runtimes or Python source/environment handles
  across later revisions.
- M7 raises rustX's MSRV to Rust 1.88 for the current rmcp release. Python
  environments isolate dependencies but are not security sandboxes; metadata
  for future GC is written, but no GC runs.
- Issue #10 acceptance criteria are complete: a fully local/offline fixture
  proves that two tools depending on conflicting versions of the same local
  package materialize distinct environments, both execute, and each observes
  its own version with no public PyPI access; coordinator-level MCP
  list-change race regressions prove the Busy/Stale/commit interleavings;
  stdio and Streamable HTTP cancellation prove server-side observation of
  the cancellation notification; and an official-rmcp paginated fixture
  proves the canonical registry contains the finite complete sorted
  catalog.

## Milestone 7.5b — Unified Context Assembly and Request Snapshots (Issue #55)

Implemented in the current architecture:

- One rustX-owned Context Assembly contract for native observations and
  certified-extension proposals.
- Finite immutable `ContributorInputSnapshot`; no arbitrary runtime handles,
  history mutators, provider adapters, or current-state lookups are exposed
  to contributors.
- Stable serializable contributor identities, separate attestation/content
  generations, finite semantic user/system lanes, native-reserved owners,
  and canonical logical-identity ordering for multi-extension lanes.
- Agent Status and Skill guidance admitted as ordinary canonical Runtime
  context User messages. The old model-request-only semantic attachment
  paths do not exist.
- RustX-owned Effective System Prompt sections and deterministic rendering;
  the exact rendered value is frozen per request.
- Frozen provider-independent `RequestSnapshot` with exact SurfaceRevision,
  effective model/reasoning/request parameters, tool definitions, capability
  revision, ContextGeneration, continuation, and request identity.
- Awaited typed `ContextContributor`/`ContextAssembly` boundary: bounded
  futures settle against a finite immutable input before the final generic
  admission cancellation observation.
- Runtime-owned append-only in-memory `RequestHistory` receives every actual
  primary snapshot at attempt settlement, retaining it beyond
  `AgentExecutionResult` without copying a second transcript. Issue #11 will
  later persist the same semantic object.
- Generic pre-admission cancellation linearization, no rollback after
  admission, and bounded overflow compact-and-retry that reuses the accepted
  context generation without reinvoking contributors.
- `ContextWindowExceeded` does not prove fresh inbound was observed; overflow
  compaction keeps the pending `FreshInboundTurn` constraint while reusing
  the accepted context generation.
- Structural `ModelRequest` reconstruction from historical Surface plus the
  frozen snapshot, checked before provider adapter translation.
- Mechanically derived `ContextCompatibilityManifest` and deterministic
  fake-provider regression coverage.

Exit criteria:

- An old provider-neutral request remains byte/structurally exact after live
  model configuration, Skills, contributors, package generation, and runtime
  state change.
- Cancellation before admission commits no dynamic context; failure after
  admission preserves historical context and snapshots.
- Overflow retry produces no duplicate dynamic context and reconstructs
  both the original and compacted request independently.

## Milestone 7.5c — Typed lifecycle interception and deterministic post-tool context settlement (Issue #56)

Implemented in the current architecture:

- One required immutable `AttemptLifecycle` per attempt carrying exactly two
  phase-specific typed seams. `AttemptLifecycle::inert()` is the identity
  configuration, so no execution path branches on whether a seam is attached.
- `PreStepPolicy`: an awaited `Enter`/`Reject(reason)` boundary over the
  final immutable `AcceptedContext`, evaluated after Context Assembly and
  before the generic pre-admission cancellation checkpoint. It is the single
  downstream authority every proposal — native, certified-extension, and
  deferred post-tool — converges on.
- `ToolResultObserver`: an immutable observation of each finalized tool
  result, run in canonical `ToolCall` order strictly after the owning batch
  reaches structural settlement. It carries canonical batch position,
  `ToolCallId`, registry-resolved `ToolId`, typed `ToolOrigin`, the committed
  `ToolExecutionResult`, and an `ObservedToolInvocation` (resolved
  `ToolInvocationMode` plus the validated business arguments, absent for a
  preflight-rejected call). The model-facing tool name is deliberately absent
  so capability recognition is a typed-identity question.
- Bounded preflight refactor: both `PreflightOutcome` variants carry the
  registry-resolved `ToolId` and `ToolOrigin` from the same resolved
  `ToolDefinition`, so no second stored identity can disagree with the
  registry.
- Timing/ownership separation: observers are *bound* to a
  `DeferredContextProducer` (at most one per semantic owner). "Post-tool" is a
  lifecycle *timing* fact owned by the Agent Loop; the lane, `UserSource`, and
  `ContextKind` come from the resolved producer through the same table Context
  Assembly applies to that owner's request-time proposals. A certified
  extension keeps its identity and provenance when it defers.
- Binding is not admission: `ContextAssembly::register_extension` is the one
  semantic identity/provenance/attestation authority. A deferred extension
  producer is resolved against it and uses **that registration's** generation
  and attestation; an unregistered key fails the assembly with
  `UnregisteredContributor` before admission, with no lane, no extension
  provenance, and no synthesized generation. Registration — not request-time
  output — is what makes an extension, so a post-tool-only certified extension
  works.
- Deferred output is User context only: `ToolResultObserver` returns
  `UserMessageProposal`, so a deferred Effective System Prompt section is
  unrepresentable. System sections stay on the request-time contributor path.
- Cancellation precedence: observable cancellation is checked before each
  observer starts and again once it settles, before its return value is
  consumed. An in-flight bounded observation settles rather than being
  dropped, but cancellation then wins over its success *and* its failure, and
  no later observer starts.
- Deferred context: an Agent-Loop-owned transient buffer ordered by
  `(canonical ToolCall batch position, producer identity, proposal FIFO)`,
  admitted through the ordinary Context Assembly path. The buffer is never
  canonical history. It is bounded at the observer transaction boundary —
  per-observation count against the established `MAX_PROPOSALS_PER_CONTRIBUTOR`
  limit, running attempt total, and per-proposal content — before anything is
  staged, and again in assembly.
- Typed failure settlement: `PreStepRejected`, `PreStepPolicyFailed`,
  `ToolResultObservationFailed`, and `DeferredContextRejected`, each
  preserving exactly one terminal event.

## Milestone 7.75 — Conversation runtime coordination extraction (Issue #61)

Implemented in the current architecture:

- `ConversationRuntime` (`src/runtime/conversation_runtime.rs`) is the
  semantic conversation coordinator: session model authority, attempt-id
  allocation, the current-attempt slot, attempt admission, between-attempt
  `ConversationState`, `RequestHistory`, the shutdown gate, the
  mailbox/admission relationship, and settlement handoff. It installs no
  client-bound observation seams, so a conversation executes identically
  with zero Runtime Client attachments (headless composition is the same
  `AgentExecution`/Context Assembly/ToolRuntime/Capability/provider path).
- `RuntimeClientHost` is the projection + control + attachment adapter over
  the coordinator: it owns the projection read model (snapshot/cursor/
  bounded replay/subscribers), the one-active-attachment policy, and
  protocol adaptation, and it forwards control (`model_set`, `shutdown`,
  `cancel_current_attempt`, background queries) to the coordinator. It no
  longer owns canonical conversation/session/admission state.
- One admission authority: every ordinary inbound producer (human submit
  through the Runtime Client, runtime/agent inbound, background terminal
  notifications) publishes into the conversation inbound mailbox; the
  mailbox's shared wake handle notifies the coordinator's admission worker,
  so an idle asynchronous enqueue is admitted without any client request.
  `admit_next_attempt` owns the admission linearization (idle + gate
  observation, finite drain, canonical commit, attempt-id allocation, model
  freeze, current-attempt publication) under the one coordinator lock.
- Observation handoff: the coordinator publishes semantic observations into
  a shared leaf queue; every projection lock acquisition drains it first, so
  `snapshot + cursor C` remains linearizable and `resume(after C)` observes
  every later projected fact or fails explicitly with `resync_required`
  (Issue #37 invariant preserved across the split).
- Identity claims: one conversation runtime coordinator per
  `ConversationToolRuntime` identity (claim at coordinator construction) and
  one Runtime Client host per coordinator (claim at host construction);
  both are one-time lifetime bindings with typed already-bound rejections.
- Deterministic regressions: headless full turn (no attachment), idle
  async wakeup, async-wake vs client-submit race, enqueue-vs-settlement
  race, enqueue-during-active-attempt, safe-boundary tool-batch structure,
  snapshot/cursor linearization races, model-update freeze at admission,
  capability revision immutability, attachment independence, and one
  human+runtime admission path — all with gates/barriers/Notify, never
  sleeps.

Intentionally absent (no concrete native owner or consumer):
`PreToolPolicy`, tool-execution wrappers/middleware, post-tool result
replacement or retroactive blocking, pre-tool argument or identity
rewriting, `Ask`/human approval (Issue #64), subagent lifecycle observation
(Issue #60), and turn-stopping/forced continuation.

Exit criteria:

- A pre-step rejection commits no dynamic context, advances no Surface
  revision, freezes no `RequestSnapshot`, and starts no provider request; no
  contributor and no observer can bypass it.
- `Assistant(A, B)` always produces `ToolResult A` then `ToolResult B`, and
  deferred context never interleaves between sibling results, even when B
  physically completes first.
- Observer failure keeps the complete canonical result batch, commits no
  deferred context — including proposals earlier observations of the same
  pass produced — and settles the attempt exactly once.
- A native producer's deferred proposal gets native provenance and a
  *registered* certified extension's keeps extension identity/provenance and
  its registered attestation; post-tool timing rewrites no contributor
  identity; producers with different identities keep a deterministic order
  that does not depend on registration order.
- An observer bound to an extension the attempt's Context Assembly never
  registered admits nothing and gets no extension provenance.
- Deferred context never changes the Effective System Prompt.
- Cancellation observed around an observation prevents any later observer from
  starting, discards the pass, and outranks the observer's own success or
  failure.
- An observer can identify the native Read target path from the validated
  invocation arguments, without any model-facing name, and a
  preflight-rejected call exposes no invocation arguments.
- A single observation above the per-observation bound, or observations that
  together cross the aggregate bound, are rejected at the transaction
  boundary and stage nothing.
- An overflow retry re-evaluates neither the pre-step policy nor the
  contributors, and duplicates no deferred context.

## Milestone 8 — Runtime events and durability

Implement interfaces for:

- Runtime event writer
- Message Ledger and Conversation Surface durability

Development backend:

- JSONL / filesystem storage is acceptable for local validation.

Semantics:

- Append-only runtime events
- Persist-before-publish ordering
- Stable event sequence numbers
- Crash reconciliation
- Unresolved tool-call handling

Exit criteria:

- A local session can be reconstructed from durable facts after process restart.

## Milestone 9 — Cancellation and runtime supervision

The M5 tool plane PR implements the concrete ownership seams required by
the native tool plane: the shared runtime `CancellationSignal`, attempt-owned
cancellable foreground executions (including Bash process-group
termination), conversation-owned background executions with the
`background_task` cancel path, and explicit runtime shutdown cancellation of
active background work.

The M6 skills PR implements the minimal concrete capability
snapshot/mutation semantics required for Skills: the immutable attempt
capability snapshot, the attempt capability lease, the quiescent capability
commit (zero active attempt leases for the conversation), the
`CapabilityRevision` swap, and background environment capture — all owned
by `src/capabilities`. Remaining M9 work is the broader runtime-wide
machinery that M6 deliberately does not implement:

- Hierarchical runtime supervisor tree and generic process supervision
  (beyond the concrete shared supervised command runner and the Bash
  supervisor units)
- Quiescent runtime state machine and graceful draining (runtime-wide busy
  state beyond attempt capability leases: active tool calls, foreground or
  background processes, event-writer and drain transitions)
- General scheduler/runtime busy state and generic process supervision
  beyond the concrete current ownership seam
- Recovery/lifecycle orchestration

Exit criteria:

- Capability changes are rejected while the conversation runtime is busy
  (the M6 attempt-lease guard is the concrete first instance; M9 extends
  the busy definition to the full runtime state machine).
- Attempt cancellation does not incorrectly terminate conversation-owned background work.

## Milestone 10 — Local runtime product

The spawnable local runtime *process* and its composition ownership already
exist (Issue #42): `LocalConversationRuntime::compose` builds one conversation
session — session model authority, `ConversationToolRuntime`, native
registry, `CapabilityCoordinator` (prepared and committed before serving),
  context policy/Surface pieces, one `ConversationRuntime` (Issue #61, the
  semantic conversation coordinator), and one `RuntimeClientHost`
  projection/control adapter over it — and serves its endpoint over the
  Issue #38 stdio/JSONL transport with a protocol-only stdout.
Model catalog and session configuration are explicit file paths.

M10 productizes that established seam. It owns configuration discovery and
precedence, named profiles, manifest/workspace UX, an interactive config
editor, durability/recovery UX, and soak testing — **not** the composition
ownership, which is frozen.

Build an interactive CLI for sustained manual testing.

Suggested commands:

- `/context`
- `/messages`
- `/events`
- `/tools`
- `/skills`
- `/compact`
- `/cancel`
- `/reset`

Run long-session soak tests covering:

- Many conversation turns
- Repeated compaction
- Tool failures
- Model errors
- Cancellation
- MCP reconnects
- Python tool execution
- Background Bash

Exit criteria:

- rustX can run as a standalone local agent runtime for extended sessions without relying on production control-plane services.

## Milestone 11 — Production integration boundary

Only after the local executor is stable, implement:

- Runtime command protocol
- HTTP/control interface
- Production event-store adapter
- AG-UI event projection
- Universal runtime image
- Orchestrator integration
- Control-plane integration

## Testing strategy

Four test layers are required:

1. Unit tests for pure state machines, transformations, ordering, and validation.
2. Deterministic runtime fixtures using mock model and tool executors.
3. Deterministic composed conformance through the real provider boundary: the real catalog, adapter, HTTP client, stream parser, Agent Loop, context engine, tool runtime, capability plane, and Runtime Client projection, against the external scripted provider emulator (`test-support/fake-provider`, Issue #47). Scenarios are strict ordered scripts; race-sensitive tests are ordered by provider-side gates rather than sleeps.
4. Live integration tests using real model endpoints, MCP servers, and process environments.

Core semantics must never depend exclusively on nondeterministic live-model tests.

## Development priority

The shortest validated path is:

```text
single model stream
-> canonical messages
-> multi-turn loop
-> fake tool loop
-> runtime events
-> compaction
-> native tools
-> skills
-> MCP
-> Python tools
-> recovery
-> production integration
```
