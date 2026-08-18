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
- Deterministic Issue #27 repeated-compaction validation through the final
  runtime path, the real provider adapter/HTTP-SSE boundary, and the real
  rustx child/stdio composition against the local fake provider

The M1 `ContextManifest` gained `context_window_tokens` (additive pre-1.0
contract change; fixture and round-trip tests updated).

Issue #7 (M4: context engine and Agent Status) is **completed**; Issue #27
owns the deterministic multi-compaction/TUI verification against the local
fake provider, and Issue #8 (M5)
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
Background runtime producers are implemented by Issue #8 (M5); M8 now
composes one `ConversationStoreBinding` per conversation and derives the
narrow mailbox capability from it, while mailbox coordination and restart
recovery policy remain separate concerns.

Exit criteria:

- A long local session can compact multiple times and continue correctly.
- Compaction never rewrites or deletes canonical history.
- Deterministic fixtures cover normal and repeated whole-message compaction,
  exact historical Surface reconstruction, and equal-content identity.
- Fresh inbound material is never compacted before a successful model
  invocation observes it; preserving it or failing explicitly with
  `CannotFit` are the only two outcomes.

Deferred to later milestones: conversation summarization in the CLI (M10),
provider fallback, and routing. Parallel tool scheduling is implemented by the M5 tool plane PR;
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
  plus inner session/group leader). Linux enables child-subreaper adoption;
  macOS uses the direct process-group path with an injected EXIT `wait` as a
  best-effort convenience (not an ownership boundary) because it has no
  equivalent orphan-adoption primitive.
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
  boundary is its dedicated process group. Linux adds inherited seccomp
  rejection of `setsid`/`setpgid` and child-subreaper adoption, making the
  group wait a complete descendant proof. macOS keeps the real group and
  cancellation lifecycle but has no equivalent seccomp or orphan-adoption
  primitive, so it proves terminality by escalating to the outer's fallback
  containment `SIGKILL` and probing the group absent (`killpg(pgid, 0)` ->
  `ESRCH`); a descendant that deliberately leaves the group exits rustX's
  ownership domain and is never claimed contained or reaped, while a lost
  anchor remains explicitly unproven.
- Target-ABI seccomp policy: membership syscall numbers come from the
  compiled Linux target's libc constants; x86-64 rejects the x32 syscall
  namespace explicitly because it shares `AUDIT_ARCH_X86_64`
- Reuse-safe process-group ownership: `TERM`/`KILL` are issued by the
  inner supervisor with `killpg` against its own process group, whose
  numeric id is its own pid — provably allocated while it lives; the final
  signal is the last `killpg`, after which the anchor is released by the
  reap and no further signal exists
- Kernel-mediated group terminality: the terminal point is the
  group-scoped wait (`waitid` with `Id::PGid`) returning `ECHILD` at the
  outer supervisor. On Linux, child-subreaper adoption plus immutable
  membership makes that a complete whole-group proof. On macOS, that
  `ECHILD` only proves the waiting supervisor has no waitable group child
  left (a reparented descendant is invisible), so the group's absence is
  proven by a bounded `killpg(pgid, 0)` probe reaching `ESRCH` after the
  fallback containment signal — never inferred from `ECHILD` alone, from
  `/proc`, or from a `killpg` `EPERM` (`EPERM` proves only that the signal
  operation was not authorized — on macOS the kernel also reports a
  zombie-only group as `EPERM` — so it is not a terminal result by itself).
- Explicit ownership protocol: `AnchorReady -> Start ->
  OwnershipEstablished`; the successful Bash spawn is the OS commit point,
  and post-start channel loss is conservatively treated as possible
  ownership. `NoOwnership` covers pre-spawn setup failure.
- Control-channel EOF is never post-ownership terminality. Normal settlement
  uses `AllChildrenReaped`; on Linux, catastrophic supervisor loss uses
  rustX's own subreaper adoption, retained `WNOWAIT` anchor, anchored group
  containment, and group-scoped `ECHILD` proof before returning `Failed`. On
  macOS, a lost outer without a waitable anchor is reported as
  `AnchorUnavailable` and remains unproven; it is never converted into a
  terminal result.
- Single-reaper anchor ownership: the inner supervisor pid is an ownership
  anchor with exactly one reaping owner (the outer's dedicated anchor path
  in the normal lifecycle; rustX's adopted-anchor path after both
  supervisors are lost). The outer supervisor has no generic `waitpid(-1)`
  reaping loop — generic child reaping can never consume the invocation
  anchor, and an anchor `ECHILD` before the intentional release is an
  ownership invariant violation, never process terminality.
- Runtime child-subreaper capability: on Linux, rustX's process-wide
  `PR_SET_CHILD_SUBREAPER` activation is a runtime-level kernel coordination
  primitive (lazy one-time, idempotent, sticky activation; owned by
  `src/runtime/process_supervision.rs`), established before `START` and
  never toggled per invocation. It is the catastrophic fallback authority
  for Bash supervisor units only — in M5, Bash is the only production
  subprocess hierarchy relying on orphan adoption, no generic unknown-child
  reaper exists, and catastrophic Bash containment
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
  ownership core (Linux fixed-membership seccomp and child-subreaper
  fallback; macOS process-group lifecycle, group-scoped kernel terminal
  proof, single-owner anchor discipline, TERM/grace/KILL against the inner's
  own group, driver-owned settlement with direct-child reap before
  publication, EOF-drained bounded stderr).
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
- The ConversationStore durably freezes every actual primary snapshot at
  request start. `RequestHistory` is a bounded, fallible, cursor-paged read
  handle over those facts, retaining no second transcript or unbounded
  snapshot vector.
- The active `AgentExecution` retains only bounded continuation state and the
  current `ConversationState`. It retains neither a complete Request Snapshot
  collection nor a duplicate Event Journal trace; durable request and event
  history is read from the store by key or bounded page.
- Generic pre-start cancellation linearization, no rollback after the
  start commit, and bounded overflow compact-and-retry that reuses the
  staged context generation without reinvoking contributors.
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
- Cancellation before the start commit has no durable effect; failure after
  the start commit preserves historical context and snapshots.
- Overflow retry produces no duplicate dynamic context and reconstructs
  both the original and compacted request independently.

## Milestone 7.5c — Typed lifecycle interception and deterministic post-tool context settlement (Issue #56)

Implemented in the current architecture:

- One required immutable `AttemptLifecycle` per attempt carrying exactly two
  phase-specific typed seams. `AttemptLifecycle::inert()` is the identity
  configuration, so no execution path branches on whether a seam is attached.
- `PreStepPolicy`: an awaited `Enter`/`Reject(reason)` boundary over the
  final immutable `AcceptedContext`, evaluated after Context Assembly and
  before the model-turn start arbitration. It is the single
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
  preserving one terminal settlement candidate; the terminal Event Journal
  fact is published only after its durable append succeeds.

## Milestone 7.75 — Conversation runtime coordination extraction (Issue #61)

Implemented in the current architecture:

- `ConversationRuntime` (`src/runtime/conversation_runtime.rs`) is the
  semantic conversation coordinator: session model authority, attempt-id
  allocation, the current-attempt slot, attempt admission, between-attempt
  `ConversationState`, `RequestHistory` (now `src/runtime/request_history.rs`),
  the shared lifecycle/drain authority, the mailbox/admission relationship,
  and settlement handoff through quiescence. It installs no client-bound
  observation seams, so a conversation
  executes identically with zero Runtime Client attachments and with no
  Runtime Client host at all (headless composition is the same
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
- Runtime-owned observation contract: `src/runtime/observation.rs` defines
  `ConversationObservation` (semantic source types only) and the leaf
  `PendingObservations` queue. There is exactly **one** fold of that
  vocabulary — the Runtime Client projection — and the runtime keeps no
  mirrored client read model. The runtime never imports Runtime Client
  projection/snapshot types.
- Observation handoff: the coordinator publishes semantic observations into
  a shared leaf queue; every projection lock acquisition drains it first, so
  `snapshot + cursor C` remains linearizable and `resume(after C)` observes
  every later projected fact or fails explicitly with `resync_required`
  (Issue #37 invariant preserved across the split).
- Runtime lifecycle: `ConversationRuntime::new` constructs the runtime
  **inactive** through one `ConversationToolRuntime -> ConversationRuntime`
  ownership transfer: under the background registry lock it requires a
  pristine background plane (no prepared dispatch, no committed record),
  claims the one-time coordinator binding, and binds the canonical mailbox
  runtime-owned with a fresh `Inactive` shared lifecycle at one
  linearization point — so a standalone background commit
  either wins first (construction fails typed with
  `ConversationRuntimeError::ToolRuntimeNotQuiescent` and consumes
  nothing) or loses to the transfer (a later commit fails
  `BackgroundDispatchError::ConversationInactive`). The inactive phase is
  then structurally inert, not merely documented: the mailbox refuses
  inbound, `model_set` fails typed with
  `ModelUpdateError::Inactive`, `shutdown` fails typed with
  `ShutdownError::Inactive`, the background registry refuses
  `commit_dispatch` with `BackgroundDispatchError::ConversationInactive`,
  and the capability coordinator refuses a runtime-owned `commit` with
  `CapabilityCommitError::ConversationInactive` — all consuming nothing.
  Activation has **one authoritative lifecycle state**: the shared
  `ConversationLifecycle` token composed by the runtime, read by the
  mailbox (runtime ownership is the handle itself), the background
  registry (through its mailbox), the capability coordinator (attached at
  its claim), and the coordinator itself. `ConversationRuntime::activate`
  performs the single `Inactive -> Running` transition of that one token
  under the one coordinator lock — the activation linearization point —
  and the one winning caller spawns the admission worker and performs the
  initial admission kick; concurrent calls are idempotent. No
  subsystem-specific intermediate activation state exists, so background
  and capability commits can never disagree about whether the
  conversation admits new work. The ownership transfer
  (`standalone -> runtime-owned/inactive`) and activation
  (`Inactive -> Running`) are two distinct commit points.
  A `RuntimeClientHost` may then optionally bind;
  `ConversationRuntime::activate` is the one explicit composition boundary
  after which semantic execution may begin. Binding a client host is a
  pre-activation decision — a late bind fails with the typed
  `HostConstructionError::RuntimeAlreadyActivated` — while Runtime Client
  *attachments* remain fully dynamic afterwards. Headless runtimes
  (Issue #60 subagents, every zero-client regression) never construct a
  host at all.
- One global bootstrap cut: `ConversationRuntime::install_observation_bridge`
  runs entirely under the one coordinator lock over an inert runtime,
  installing the queue and every subsystem seam and capturing the seed as
  `coordinator facts → background → mailbox → capability (= the cut R)`.
  Every earlier authority is provably frozen across `[T0, R]` — coordinator
  facts by the held lock, background because the ownership transfer
  requires a pristine plane and the registry refuses `commit_dispatch`
  while its mailbox is bound inactive, the mailbox
  because an inactive conversation refuses `enqueue`, and capability
  because a runtime-owned `commit` is refused before activation — so the
  combined seed is one real global state, not four independent cuts. The
  projection installs every seeded fact as snapshot state: bootstrap
  publishes nothing and allocates no cursor, so the first
  `RuntimeClientCursor` always belongs to a real post-activation
  transition. `RuntimeClientHost::new` performs all
  fallible work before/at the binding claim, releases the claim if the
  handshake fails, and never leaves a claimed-but-invalid binding.
- Identity claims: one conversation runtime coordinator per
  `ConversationToolRuntime` identity — claimed by the ownership transfer
  at coordinator construction, transactional on failure — and
  one Runtime Client host per coordinator (claim at host construction);
  both are one-time lifetime bindings with typed already-bound rejections.
- Two-layer production composition: `LocalConversationCore` assembles the
  semantic composition (catalog, session model, tool runtime, capability,
  context) once and constructs the `ConversationRuntime` inactive;
  `LocalConversationRuntime::compose` (interactive) binds the Runtime
  Client host then activates, and `HeadlessConversationRuntime::compose`
  (headless) activates the same core with no host ever constructed. Both
  final paths return already-active runtimes.
- Deterministic regressions: headless full turn (no attachment), headless
  real tool cycle (ToolCall → canonical ToolResult → second model turn →
  terminal settlement, zero attachments), idle async wakeup, async-wake vs
  client-submit race, enqueue-vs-settlement race,
  enqueue-during-active-attempt, safe-boundary tool-batch structure,
  snapshot/cursor linearization races, model-update freeze at admission,
  capability revision immutability, attachment independence, one
  human+runtime admission path, lifecycle regressions (interactive
  pre-activation bind, headless activation, typed rejection of a late host
  bind, attach/detach/reattach after activation), inactive-runtime
  regressions (model_set, shutdown, background dispatch commit, and
  capability commit while inactive are all refused typed and consume
  nothing; cursor 0 stays stable until activation; the first cursor
  belongs to a real post-activation transition), activation regressions
  (an activation-gate test parks `activate` before the lifecycle
  transition and proves both sides: while parked, background commit,
  capability commit, and mailbox enqueue all observe `Inactive` and are
  refused typed; after the transition the same operations follow the
  normal running semantics; a real-time ordered cross-subsystem regression
  parks a background commit after it has observed `Running` at the
  registry ownership-commit boundary and proves a capability commit that
  begins afterwards — and one that begins after the background completed —
  cannot observe a stale `Inactive`; a host-bind-vs-activate race proves
  the host binds with the bootstrap seed at cursor 0 while `activate` is
  parked before the transition; concurrent `activate` calls are
  idempotent — one CAS winner, one worker, exactly one attempt from one
  inbound item), ownership-transfer
  regressions (a tool runtime with a committed background record or a
  prepared dispatch is rejected typed `ToolRuntimeNotQuiescent` with no
  claim consumed, the mailbox left standalone, and the staged/committed
  work keeping its standalone semantics; both race interleavings of the
  transfer against `commit_dispatch` at the commit boundary hook —
  background wins and the construction fails typed, transfer wins and the
  commit fails `ConversationInactive`; a failed capability claim rolls the
  transfer back to the exact standalone state), production-composition
  regressions (interactive and headless resolve the same semantic
  composition; a real headless turn runs with no host; the interactive
  path still runs over the same composition), and
  construction-outside-Tokio rejection — all with gates/barriers/
  Notify/watch, never sleeps.

Intentionally absent (no concrete native owner or consumer): tool-execution
wrappers/middleware, post-tool result replacement or retroactive blocking,
pre-tool argument or identity rewriting, question/form frameworks,
generalized permission/risk policy, subagent lifecycle observation (Issue
#60), and turn-stopping/forced continuation. The bounded native interaction
and approval seam is delivered in M9.2 below.

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

## Milestone 8 — Native SQLite conversation durability (Issue #11)

Issue #11 is one complete durability architecture, not separate table
increments. `ConversationStore` is the backend-independent semantic contract;
`SqliteConversationStore` is the development backend. One database contains
these distinct authorities:

- Pending Inbound Inbox: accepted, not-yet-adopted deliveries, one shared
  `InboundSequence`, and correlation/idempotency state;
- Message Ledger: append-only canonical message bodies and commit order;
- Conversation Surface: immutable `SurfaceOp` history and exact revisions;
- Request Snapshots: immutable non-history inputs for one actual `RequestId`;
- Event Journal: typed append-only execution facts, ordered by durable event
  sequence, with schema version, references, and terminal uniqueness;
- current Surface/checkpoint metadata: bounded bootstrap/index state only.

The mailbox is process-local coordination/wakeup, and Runtime Client state is
projection/control only. There is no full transcript, `ConversationRecord`,
request-message copy, generic repository, or client recovery cache.

The schema is development version 1. Incompatible files fail explicitly;
there is no migration framework, legacy reader, fallback, or dual write.
File-backed SQLite uses WAL, `synchronous=FULL`, foreign keys, and a busy
timeout. Commit is the local durability linearization point.

Semantic transitions are prepare → SQLite transaction → COMMIT → infallible
hot-state installation/reload:

- acceptance commits sequence allocation, pending row, and correlation state;
- finite adoption commits pending selection, canonical User Ledger rows,
  Surface Append revisions, checkpoint metadata, and pending deletion;
- ordinary canonical append and the model-call-ordered ToolResult sibling
  batch commit Ledger bodies, Surface revisions, and committed-message events
  atomically;
- compaction commits the summary Ledger row, immutable Surface Replace,
  generation/checkpoint metadata, and `CompactionCompleted` atomically;
- model request start commits the immutable Request Snapshot and exact
  `ModelRequestStarted` fact before adapter invocation;
- background terminal publication commits the terminal inbound delivery and
  its reference fact atomically.

The live provider-neutral request is independently reconstructed from the
just-committed snapshot, historical Surface revision, and keyed Ledger
bodies before dispatch. Historical reconstruction never reruns contributors,
Skills, extension/DSH logic, current status, workspace reads, or current
configuration. Current runtime bootstrap hydrates only the current Surface
working set; old requests, events, Ledger rows, and Surface revisions are
paged on demand. Active execution retains only bounded current state: the
current Surface working set, one structurally unsettled tool batch whose
per-call foreground progress is cardinality-bounded
(`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`, earliest prefix plus latest),
and bounded deferred-context staging. Historical growth lives in
ConversationStore only.

Exit criteria are complete: restart preserves pending delivery, canonical
Ledger identity/order, every retained Surface revision, exact started-request
reconstruction, and Event Journal ordering; injected pre-commit failures
expose either the complete old state or the complete new state; active
durability failure degrades the owning runtime explicitly; and headless and
Runtime Client paths use the same store semantics.

The #63 contracts retained in M8 are durable acceptance, shared sequence and
correlation semantics, finite watermark selection, shutdown ordering,
prepare→commit→install, bounded admission retries, incomplete-tool-turn
fail-closed behavior, and registry-owned background terminal settlement.
The temporary #63 compaction-surface fail-close seam and the inbound-only
store façade are superseded by atomic Surface revisions and the unified
ConversationStore. Startup recovery over that evidence is M9a (Issue #12,
below); replay/resend policy and retry orchestration remain open.

## Milestone 9 — Recovery, cancellation, and runtime supervision

Issue #12 is delivered in three slices, in order:

```text
M9a — durable startup recovery + recovery classification   (delivered)
  |
M9b — unified model-turn cancellation / request-start commit boundary
      (delivered)
  |
M9c — foreground/background/process supervision + runtime quiescence
```

### M9a — durable startup recovery (delivered)

M9a reconstructs one coherent `ConversationRuntime` from rustX-owned durable
authority, deterministically classifies crash-time non-terminal work,
reconciles only what can be stated honestly, and resumes only work proven safe
to continue. It builds on the completed #61 (conversation runtime ownership),
#63 (durable Pending Inbound), #11 (SQLite native durability), and #47
(external provider emulator).

```text
durable facts -> reconstruct -> classify -> reconcile -> recovered state -> resume
```

The full contract — owner, evidence sources, the four phases, the A/B/C/D/E
classification matrix, reconciliation transaction boundaries, terminal
uniqueness, identity recovery, the external-lifecycle/canonical-lifecycle
separation, the recovery-prefix invariant, and the bounded working set — is
frozen in
[architecture.md §7](architecture.md#7-recovery-model) and
[invariants.md](invariants.md#recovery). The tool plane splits external
history from canonical repair across two owners: the attempt owns a bounded
foreground-tool external summary (did execution happen; is any outcome
unknown), while detailed per-call results exist only while that call's
canonical `ToolResult` is still missing. The three rules that govern all of it:

```text
exact historical reconstruction  !=  safe replay permission
started + outcome unknown        !=  safe retry
started + outcome known          !=  never externally started
```

Only an attempt with zero durable external-start evidence — no
`ModelRequestStarted`, no `ToolExecutionStarted`, ever — may receive the
Class-B continuation, and a committed canonical `ToolResult` never erases
the historical `ToolExecutionStarted` while its owning attempt is still
non-terminal.

Exit criteria (met): accepted Pending Inbound survives a crash unchanged and
is auto-admitted headlessly; a crash after the adoption commit leaves the
`UserMessage` canonical exactly once; a committed request start with an unknown
outcome reconstructs exactly, classifies as indeterminate, and resends nothing
(proven at the real provider boundary against the #47 emulator); an
interrupted foreground tool becomes a typed `Interrupted` canonical result in
one atomic sibling batch; non-terminal background work is terminalized as
`Interrupted` with exactly one model-visible notification; durable terminals
and repeated restarts are idempotent; and recovered identity allocators never
collide with durable history.

Explicitly **not** in M9a: model-turn cancellation redesign (M9b, delivered
below), runtime
supervision/quiescence (M9c), a generic retry engine, automatic replay of
ambiguous side effects, a scheduler or durable job queue, subagent recovery,
interaction, or DSH session persistence.

### M9b — model-turn cancellation linearized against the durable start commit (delivered)

M9b replaces the old split "pre-admission cancellation check → separate
request-start persist" with one arbitration point per model turn:
`AgentCancellation::arbitrate_model_turn_start` holds the attempt's start
gate across the cancellation check and the fused durable
`ConversationStore::commit_model_turn_start` transaction (request-scoped
context appends + `RequestSnapshot` + `ModelRequestStarted` + sequence
binding). Exactly one of cancellation and the start commit can linearize
first:

- cancellation before the arbitration ⇒ no request-scoped context, no
  Surface advancement, no snapshot, no start fact, no provider request;
- the start commit first ⇒ the request is durably started: rustX has
  crossed the no-resend / external-start boundary, so a later cancellation
  is post-start and settles that started request — it can never be
  reclassified as never-started. Provider execution may or may not have
  actually occurred (the loop still reconstructs and verifies before adapter
  invocation, and a process may crash in between);
- a start-commit failure ⇒ the whole transaction rolls back (no
  half-committed request-scoped context) and the attempt settles with the
  honest durable-store failure.

The boundary is shared by every model turn: first turn, tool→model
continuation, recovered Class-B continuation, and overflow retry. Context
assembly output is staged in a scratch conversation state without durable
effect; the compaction between an overflow and its retry is an independent
durable commit whose candidates are evaluated through the same
`TokenEstimator` over the exact hypothetical post-compaction request —
retained Surface plus the staged request-scoped context overlay
(`CompactionConstraints::staged_request_context`) — never as a scalar token
delta. `TurnStarted`
means "turn preparation began", not request start. Race tests park the
execution immediately before the arbitration and inside it (before the
commit) through ordinal-counted, explicitly released test seams — never a
sleep.

The contract is frozen in [agent-loop.md §4.2](agent-loop.md) and
[invariants.md](invariants.md#context-assembly-admission-and-agent-status).

Explicitly **not** in M9b: runtime supervision/quiescence (M9c), a generic
retry engine, and any replay/resend policy for ambiguous side effects.

### M9c — supervision and quiescence

M9c is delivered as a concrete ownership composition, not as a generic
supervisor framework. `ConversationLifecycle` is the one authority:

```text
Inactive -> Running -> Draining -> Quiescent
```

`ConversationRuntime::shutdown()` linearizes `Running -> Draining` under the
coordinator lock shared with inbound acceptance and attempt admission. It
requests `RuntimeShutdown` on the current `AgentCancellation`, closes new
semantic admission, and awaits one shared idempotent drain completion. It
returns successfully only after the current attempt, foreground tool batch,
conversation-owned background registry, runtime-owned capability commits,
counted capability/environment preparation, retained MCP stdio runtimes,
existing supervised process terminality, and admission worker exit have
settled. `Draining` retains narrow durable settlement paths; `Quiescent`
rejects stale callbacks. An unproven owned process settlement returns a
runtime failure and does not claim `Quiescent`.

Drain is a supervisor, not a short-circuiting pipeline. It closes admission,
requests cancellation/closure of every concrete owner, supervises **each**
owner to its own native terminal boundary, and only then decides. A failure
in one participant — an exhausted background terminal-publication budget, an
MCP close that cannot prove physical settlement, a degraded durable authority
— is an error fact, never permission to abandon a sibling that can still act.
Failures are collected in deterministic identity order and reported as one
bounded diagnostic. `Ok(())` still means exactly `Quiescent`;
`Err(RuntimeOwnedSettlement)` means every settleable owner was supervised to
its strongest available boundary while some ownership/physical/durable
terminal condition stayed unproven. An unresolved `PublishingTerminal` record
stays explicitly non-terminal and is never reinterpreted as success.

A settlement fact never precedes the owner's last conversation-facing
callback. `publication_abandoned` — the fact drain consumes as one background
execution's settlement — is committed only after the runner exhausted its
bounded durable terminal publication *and* its failure report to the owning
runtime returned: failure callback, then abandoned commit, then waiter
notification. The continuation is held inside one counted settlement
admission across both steps, because a failed drain leaves the lifecycle
`Draining`, where settlement callbacks stay intentionally legal, so the
supervisor itself must prove no owned callback is still live before
`DrainCompletion` stores a result. Once the abandoned fact is observable that
execution owns no remaining callback authority, so a cached shutdown
completion can never precede a later conversation callback from it.

Waiter lifetime is not ownership lifetime. A conversation-owned MCP
connection runs in its own owner task holding the counted lifecycle admission
and an ownership cancellation signal; it releases that admission only after
transferring the connection into the coordinator's retained runtimes or
driving its already-spawned stdio process to the same physical settlement
proof normal drain uses. Aborting or dropping `prepare_candidate` therefore
cannot leave a detached physical owner outside the quiescence proof, and
drain cancels those owners rather than dropping their futures.

The current-attempt slot and the attempt task are distinct ownership facts.
The slot hands conversation state back to the coordinator; the task still
owes its final admission callback, so it holds its own counted admission
until its body has fully returned. `AgentExecution` remains the execution and
terminal semantic authority; `ConversationRuntime` remains the task/runtime
lifetime composition authority.

The M9b `arbitrate_model_turn_start` gate remains the model-start
linearization primitive. A cancellation before that gate commits no provider
request, Request Snapshot, or `ModelRequestStarted`; a started request is
awaited to native settlement. Cancellation causes are first-winner absorbing,
so runtime drain reports `RuntimeShutdown` rather than a fixed
`UserRequested` construction default. Executors observe that cause through an
`ExecutionCancellation` view — the signal plus a live read of the owning
authority — instead of a start-time copy, so an execution that began before
the cancellation race still reports the winner.

The Agent Loop still owns foreground structural result settlement. Started
foreground siblings receive cancellation and settle physically/logically;
the start frontier closes before an unstarted parallel sibling can begin;
result slots and canonical model-call ordering remain unchanged. A committed
background execution remains conversation-owned after attempt cancellation,
but runtime drain cancels and awaits its registry terminal state. Terminal
visibility follows exactly-once durable Pending Inbound publication, and an
inbox item accepted before drain remains durable without being adopted into a
new shutdown-time attempt.

The existing Bash/process contract is composed transitively:
`TERM -> grace -> KILL -> process-group terminality -> reap or explicit
containment proof -> execution settlement`. No global process registry or
generic task manager was added. Shared `EnvironmentStore` work remains
store-owned and is not globally cancelled by one conversation, while counted
preparation prevents a late completion from escaping the runtime boundary.
Retained conversation-owned MCP stdio runtimes close through their existing
physical settlement contract before quiescence; a late runtime capability
commit is refused at its lifecycle boundary.

Runtime Client remains a projection/control/attachment adapter. Detach,
stdio EOF, TUI exit, and attachment drop do not drain the conversation. The
async client shutdown request awaits `ConversationRuntime::shutdown`; its
`shutdown_completed` result therefore means quiescence, while the
`RuntimeShutdown` event only reports that new admission has closed.

Deterministic coverage includes admission-vs-drain and Pending Inbound
ordering (proved at the exact `Running -> Draining` linearization, not at a
shutdown-arrival hint), pre-start and post-start model settlement, parallel
foreground start-frontier closure, active background drain and terminal
publication, both background ownership-vs-drain outcomes, capability
commit-vs-drain, physical process terminality, repeated shutdown, client
detach, and the late-callback terminal ownership boundary (proved after every
model stream owner has exited, not by poking a channel and looking
immediately). The corrective pass adds: a durability failure that must not
abandon a live provider turn or a sibling background execution; two owned MCP
runtimes where the first close fails and the second must still be closed and
settled; an aborted MCP preparation whose spawned process still owes physical
settlement; dynamic foreground cancellation-cause observation and its
first-winner absorption; the attempt task's exit as part of the quiescence
proof; and a shutdown that races the background failure sink itself, parked
inside the runner's last conversation-facing callback, proving the abandoned
fact and the cached shutdown failure both linearize after it. No SQLite schema change was
required. M9.2 interaction coordination composes with this lifecycle
contract; subagents (#60) and the DSH sidecar (#57) remain separate work.

### M9.2 — Native human interaction and approval coordination (Issue #64, delivered)

M9.2 adds one provider-independent, conversation-owned interaction plane.
`InteractionCoordinator` owns non-reused interaction identities, the live
pending registry, typed Runtime Client projection, and the one response vs
owner-cancellation terminal transition. It owns rendezvous only; the Agent
Loop still owns cancellation checks, tool scheduling, tool start, and result
settlement.

The required `AttemptLifecycle` seam is one `PreToolPolicy` plus one
`InteractionRendezvous`. The policy sees the immutable facts already resolved
by `ToolRegistry::preflight`, after the Assistant `ToolCall` canonical commit,
and returns `Allow`, `Deny { reason }`, or `Ask { reason }`. An Allow continues
the exact original `PreparedInvocation`; no response can replace identity or
arguments. A Deny produces one typed `ToolExecutionStatus::Denied` result
slot, no executor future, and no `ToolExecutionStarted` event. Parallel
batches resolve all policy/interaction decisions in canonical call order
before any executor starts.

Runtime Client v1 carries `interaction_respond`, typed acceptance/errors,
pending/settled events, and `snapshot.pending_interactions`. The TUI remains a
projection/client: it renders pending Approval facts and sends only the typed
response. Snapshot/cursor/resubscribe remains the repair authority. A missing
provider fails closed at publication; detach never answers, denies, or
cancels an already-published interaction.

Interaction IDs derive from the already non-reused `AttemptId` domain plus a
per-attempt ordinal. Pending interactions are process-owned observations, not
durable workflow records. Crash recovery does not replay them or reconstruct
them from current policy/configuration; delayed old responses are rejected.

Drain closes new interaction admission at the shared lifecycle boundary,
cancels already-owned interactions through `AgentCancellation`, and waits for
both waiter callback authority and the interaction observation callback. The
pending map becoming empty is not sufficient for `Quiescent`.

Exit criteria (met): deterministic coordinator identity/publication and stale
response tests; both answer/cancel winner orders with explicit parked
transitions; exact PreparedInvocation identity/argument preservation; denial
without executor start; preflight-before-policy; parallel start-frontier and
canonical-order tests; provider detach/unavailable behavior; Runtime Client
snapshot/event/resync projection; crash/non-reuse tests; lifecycle drain and
waiter-settlement tests; and TUI projection/render/typed-response tests.
No permission framework, durable human workflow, provider-specific payload,
generic runtime participant abstraction, or ask-user forms were added.

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
