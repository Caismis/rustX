# PR #416 ownership repair

The inspected PR was open on `issue-411-jobs-continuable-subagents`, based on
`656c39f97e50d444fffa68447057867324db8708`, at reviewed head
`524ac57117aaeaa93261d717246144d27af77ba7`. The remote head had not moved.
Work used the dedicated `rustX-issue-411` worktree; the primary checkout was left
untouched by this task. This repair retains separate finite Tool Jobs and durable
Agents. During validation, `origin/main` independently advanced to
`b63fc97763823fd55709c409c9f5a9027ea3bd23` (PR #417, a Settings test correction).
PR #416's remote head remained the reviewed `524ac571` before the repair push.
The repair does not rebase or merge that branch.

## Review resolutions

| Finding | Resolution and owner |
| --- | --- |
| #1 | Preserve the existing pre-activation child turn gate. `src/local_runtime/composition.rs::into_subagent_child_with_route` calls `gate_child_turns()` before `activate()`. `ConversationRuntime` checks the permit before adopting pending child input; Delegate durable acceptance precedes permit release. A new composition regression forces admission before Delegate and proves no request/adoption. No second startup state machine. |
| #2 | `src/runtime/subagent/registry.rs::publish_committed_snapshot` captures owner Agent plus activation under the same mutex. One combined observation owns one publication receipt. Terminal-unproven never projects Inactive. |
| #3 | Keep Interrupted logical outcome independent of later physical proof. Mandatory native incarnation lease plus a Quiescent-only receipt provides evidence; the registry owns bounded recovery reconciliation and durable proof publication. Missing proof remains fail-closed. Historical terminal and reservation obligations have concrete reconciliation owners. |
| #4 | `serve_child_delegation` closes FIFO admission and ends every non-Completed activation. Already accepted child guidance stays durable for a later explicit activation. Only successful normal completion may seal/reopen. |
| #5 | `SubagentSettlement` separates publication Pending/Committed/Abandoned from physical Unproven/Proven; finite logical state remains separate. Workflow reports a typed failure for the actual unsettled dimension and cannot succeed from valid output without physical proof. |
| #6 | Existing `LifecycleAdmission` ownership is sound. The new real-runtime race proves physical rollback and exact durable RolledBack proof before admission release, followed by Quiescent and valid reopen without reserved-ID reuse. No replacement admission protocol. |
| #7 | Starting activation publishes the owner's Admitting state in the same complete cut. Runtime Client has no activation-to-Agent lifecycle inference. |
| #8 | Preserve Agent-lifetime workspace ownership. AgentRetained is not a finite disposable handoff. Also close the recovered-resource hole: normal Agent recovery must not manufacture a Retained handoff, and finite disposal checks the durable owner before mutation. Live and recovered isolated workspace regressions prove retention. Agent deletion remains a separate non-goal. |
| #9 | Unavailable plus typed Agent Settlement/agent_settlement replaces retry advice for terminal-unproven, failed reservations/publication and poisoned workspaces. Only concrete live admission/drain owners report transient Stopping. |
| #10 | Remove duplicate test-only ConversationRuntime::subagents accessor; retain crate-private subagent_registry. Share MAX_AGENT_LIST_LIMIT. Registry listing captures all Agent/latest-activation pairs under one lock, preserving newest-created order and honest bounds/counts. |

## Synchronization evidence

- `starting_owner_publication_is_one_complete_admitting_cut` parks the real
  ownership-to-driver handoff. The store's journal receipt is present before the
  observer publishes exactly one complete Agent/activation pair. One drained
  batch equals a fresh owner read. A listing captured at that cut remains
  unchanged after the driver progresses; later listing reflects Inactive.
  `src/runtime_client/host.rs::ClientInner::agent_list` projects those captured pairs directly; it does
  not reacquire the registry for individual rows. The registry's one mutex guard
  covers ordering, matching count, bounds, and both halves of every listed row.
- `unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent` injects
  the physical result at the driver/registry boundary, awaits durable terminal,
  and compares the published pair with fresh status. Send, wait and interrupt
  return typed Settlement, and Goal idle remains blocked.
- `runtime_shutdown_rejects_unproven_child_physical_settlement` binds the real
  Runtime Client host and subscribes before a native child starts. An unresolved
  process anchor forces physical uncertainty. The test receives the terminal
  AgentUpdated through the registry, RuntimeObserver, publication queue, and
  host before reading any snapshot. That live Unavailable row exactly equals
  the fresh registry pair, an actual agent/status response, and a host snapshot.
  Native shutdown rejects the unresolved physical obligation and stays outside
  Quiescent.
- `failed_activation_preserves_admitted_guidance_for_the_next_activation` uses
  SealRequested as the committed-failure boundary, waits for exact FIFO durable
  Accepted acknowledgements before SealGranted, and proves there is no SealOpen
  or extra request. Reopening the child store and accepting a later Delegate
  consumes pending guidance in order. It tests actual model failure and exhausted
  generation budget. The current runtime has no active max_turns policy; a
  separate receiver-contract regression proves existing timeout/limit terminal
  observations enter the same absorbing Failed branch without introducing one.
- `workflow_distinguishes_committed_physical_failure_from_abandoned_publication`
  waits for Delegate, then injects exact cleanup failure or terminal transaction
  failure. Real journal facts, typed failure and output status distinguish the
  two; neither reports Workflow success.
  The registry snapshot carries `SubagentSettlement { publication, physical }`;
  `WorkflowRunError::ChildSettlement` distinguishes Pending, Abandoned, missing
  logical terminal, and committed publication with unproven physical containment.
  The cleanup-failure case must say publication committed; the abandoned case
  has no committed terminal or Workflow value despite native containment proof.
- `workflow_agent_value_and_child_terminal_are_one_atomic_transition` rejects a
  successful finite terminal/value pair whose physical proof is false, before
  any row commits. Its existing transaction-fault gate proves the successful
  terminal and structured value commit atomically; idempotent retry returns
  their original receipts. A valid JSON output grants no physical authority.
- `shutdown_waits_for_reserved_resume_physical_and_durable_rollback` starts from
  Inactive, parks a real staged child after Reserved and before ownership commit,
  waits for the runtime's Draining notification, then releases staging. A second
  gate holds the admission owner after native root removal and exact RolledBack
  commit. Quiescent cannot publish there. Releasing it permits shutdown; reopen
  preserves Agent/Conversation identity and allocates beyond the reserved ID.
- `child_binding_gates_historical_guidance_before_activation` composes the actual
  child binding over durable pending input and explicitly invokes admission
  before Delegate. No model request, attempt start or canonical input adoption
  occurs; the existing child permit gate is the synchronization authority.
- Workspace tests await native activation settlement before finite disposal and
  then resume. The recovered isolated case holds and releases the exact native
  proof lease. Neither path emits an activation workspace disposal event or
  removes Agent-owned files.

## Recovery containment proof and synchronization

`src/runtime/subagent/physical_recovery.rs` owns the exact incarnation lease and
receipt. The child acquires the mandatory exclusive lease before composition can
start capability processes, and holds it through dispatcher shutdown. Only
successful native runtime drain to Quiescent writes the receipt containing the
activation and child Conversation identities. Parent loss enters the existing
drain owner through `ConversationRuntime::shutdown_after_parent_loss`: interaction
waiters first lose control authority, then ordinary attempt/tool/capability
containment runs. This preserves requested interaction history without inventing
a human cancellation outcome. The direct child's optional inspection marker,
PID disappearance, elapsed time, and Git cleanliness do not prove descendants
settled.

`src/runtime/subagent/registry/recovery_settlement.rs` is the runtime owner of
recovered obligations. It must read the exact receipt and exclusively acquire
its native lease before committing `SubagentPhysicalSettlementProven`. That
journal event leaves the existing Interrupted outcome and terminal message
unchanged. An incomplete resume reservation instead commits its exact-origin
`RolledBack { physical_settlement_proven: true }` event before clearing the
reservation. An earlier unproven rollback remains a distinct, open resource fact
so later native proof can settle it. The registry publishes the complete owner
cut and wakes the existing idle coordinator only after the durable commit.

Finite Workflow terminal facts now carry their own required
`physical_settlement_proven` field. Restoration folds both normal
`SubagentTerminalPublished` and finite `SubagentTerminalSettled`, including
children with shared workspaces and no retained resource record. A recovered
record preserves its immutable Normal/Workflow ownership, but its live terminal
protocol is absent: recovery does not invent a Workflow output schema or turn a
finite child into a durable Agent. Resource-only reconstruction starts unproven;
only the actual terminal fact or subsequent native proof supplies containment.
The physical-proof durable validator selects the correct terminal event identity
from the immutable ownership fact before accepting either domain's proof.

The startup reconciler performs at most 150 probes over 15 seconds. Shutdown
joins its shared completion watch, and Goal-idle/shutdown boundaries can perform
another bounded pass. Expiry retains a concrete activation or reservation,
Conversation namespace, and native receipt/lease reconciliation path; it never
grants resume or successful shutdown. A later explicit reopen retries that path.
The inert incarnation evidence remains until Session deletion. Recovery can
clear only the workspace's AwaitingPhysicalProof exclusion; independent workspace
poison stays absorbing. Session deletion folds later proof across the entire
journal before classifying remaining physical obligations.

- `recovered_physical_receipt_requires_owner_release_and_durable_proof` holds the
  exact exclusive OS lease after publishing a valid receipt. Goal idle and
  message admission remain closed at that barrier. Releasing the lease permits
  one durable proof, restores Inactive and Goal idle, and preserves Interrupted
  across another reconstruction. Repeating the proof with a later proposed
  timestamp returns the original committed receipt, proving acknowledgement
  loss cannot strand the obligation behind event identity uniqueness.
- `finite_workflow_committed_physical_proof_survives_runtime_reopen` completes
  a real finite child driver, waits for native reap and durable terminal commit,
  then shuts down and constructs a fresh runtime over the same store. Its shared
  workspace has no resource handoff to accidentally reconstruct the child from.
  The restored finite record retains Succeeded and physical proof, permits Goal
  idle and Quiescent, and grants neither Agent listing nor wait/resume authority.
- `finite_workflow_recovery_tracks_physical_obligation_until_exact_owner_proof`
  receives Delegate before arming the exact three terminal transaction failures.
  Native reap finishes, but the durable ownership prefix lacks a terminal;
  startup recovery therefore commits Interrupted with physical proof false.
  A held native writer lease excludes proof even when its receipt exists. The
  shutdown Draining notification is observed before releasing that lease; an
  explicit reconciliation pass commits one physical proof before Goal idle and
  successful shutdown become available. Interrupted stays unchanged and there
  is still exactly one finite activation, with no model request or Agent identity.
  Its no-receipt case releases the same lease but remains blocked and returns
  the typed shutdown settlement failure. Paused virtual time only advances the
  bounded reconciliation deadline; the native lease and notification establish
  the tested ordering, with no sleeps.
- `finite_workflow_child_publishes_a_valid_handoff_and_client_projection` also
  reconstructs a retained finite workspace through the same complete durable
  fold used at runtime startup. The recovered handoff and Workflow ownership
  survive, terminal physical proof permits Goal idle, and the historical record
  has no executable terminal protocol. Finite disposal remains authorized.
  The disposal-recovery regressions cover durable intent before mutation,
  worktree removal before branch cleanup, and a crash before partial settlement
  publication. Replaying disposal intent transfers the old public handoff into
  private cleanup authority; replaying WorktreeRemoved never re-advertises it.
  All three cases also assert that resource disposal preserves the independently
  committed activation containment proof.
- `agent411_resume_reservation_recovery_requires_rollback_containment_proof`
  covers unfinished Reserved, unproven RolledBack, and proven RolledBack prefixes.
  For either unproven prefix, the held native lease excludes Goal idle; only its
  release permits exact-origin durable rollback proof. Reopen retains one
  original activation and the consumed reserved ordinal, so recovery neither
  reattaches the staged child nor reuses its ID.
- `hard_parent_death_terminates_child_and_recovery_is_idempotent` observes the
  real child's provider header gate before SIGKILL of the actual parent. The
  provider stays blocked until the old child has exited after control EOF.
  Reopening then shows the same Agent, Conversation, and activation as
  Interrupted/Inactive, creates no replacement child, and completes graceful
  shutdown. A second reopen proves terminal publication and physical proof are
  idempotent.
- `hard_parent_and_child_death_without_native_proof_remains_unavailable` first
  SIGSTOPs the gated child and observes its kernel stopped state. It also stops
  the parent driver before killing the child and parent, so neither child
  receipt production nor parent reap/terminal publication can race their deaths.
  Both recovered
  runtimes expose Unavailable, send/wait return typed AgentSettlement, and
  graceful shutdown reports the unresolved physical obligation. No timeout or
  process disappearance changes that classification.
- The existing unresolved-anchor recovery regressions use the native physical
  result seam and exact settlement wait to preserve a genuinely unprovable
  nested-process obligation in both shared and isolated workspaces.
- `reconciliation_owner_cancellation_releases_completion_without_physical_proof`
  uses a oneshot entry gate before aborting the parked recovery owner. Its RAII
  completion releases the shutdown watch after task cancellation; it carries no
  registry, journal, or physical authority and cannot settle an obligation.

These tests use provider gates, native file-lock ownership, process-death gates,
and committed journal cuts. Sleeps establish none of their orderings.

## Client and protocol impact

Agent rows and selection remain keyed by stable AgentId. TUI and WebUI render
Unavailable without send/retry advice, preserve transcript access, and consume
owner state. App Server retains strict protocol v25 on this unreleased branch;
generated artifacts change because its public contract adds `unavailable` and
`agent_settlement`, and finite workspace disposal returns
`{subagent_id, workspace, outcome}` instead of an invented Agent projection.
SQLite schema 47 adds the required finite Workflow physical-proof field; older
development stores are rejected without migration. No Job lifecycle or unified
execution surface was introduced.

Current architecture and invariant documents updated: `jobs-and-agents.md`,
`subagent-resources.md`, `agent-loop.md`, `invariants.md`, `tool-lifecycle.md`,
and `app-server-protocol.md`. Current SQLite-version references in Goal, uploads,
archive, and Trace documentation also now identify schema 47.

## Validation

The final Linux validation commands and results are listed below. Provider-emulator
requirements were enabled for every requested boundary and TUI command. The
existing opt-in ignored tests stayed ignored; no required suite was skipped.

| Directory | Command | Result |
| --- | --- | --- |
| repository | `cargo fmt --all -- --check` | Passed |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| repository | `git diff --check` | Passed |
| repository | `cargo build --bins` | Passed |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,130 passed; 2 existing ignored; bin/example targets passed |
| repository | `cargo test --test contracts --test provider --all-features` | Contracts 28 passed; Provider 166 passed, 5 existing ignored |
| repository | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 190 passed |
| repository | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 419 passed: catalog 26, managed output 5, conformance 22, durable 129, process 62, subagent 45, tools 130 |
| `test-support/fake-provider` | `uv sync --frozen` | Passed |
| `test-support/fake-provider` | `uv run --frozen pytest` | 51 passed |
| `tui` | `pnpm install --frozen-lockfile` | Passed |
| `tui` | `pnpm typecheck` | Passed |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 872 passed |
| `web-console` | `pnpm install --frozen-lockfile` | Passed |
| `web-console` | `pnpm typecheck` | Passed |
| `web-console` | `pnpm test` | 983 passed in 56 files |
| `web-console` | `pnpm check:provenance` | Passed: 135 source records and 131 package notices |
| `web-console` | `pnpm build` | Passed; existing bundle-size advisory |
| `web-console` | `CONTAINER_ENGINE=podman RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test:e2e` | 92 passed against the final native build (5.9 minutes) |
| `protocol/app-server` | `pnpm install --frozen-lockfile` | Passed |
| `protocol/app-server` | `pnpm check` | Passed after staging intentional generated changes; regeneration leaves no drift |
| `protocol/app-server` | `pnpm typecheck` | Passed |
| `dev` | `pnpm install --frozen-lockfile` | Passed |
| `dev` | `pnpm typecheck` | Passed |
| `dev` | `pnpm test` | 37 passed |

Targeted native regressions ran before the full matrix, including all 101 registry
tests, the 12 child-delegation tests, three finite Workflow recovery tests, the
real shutdown/resume race, and both actual hard-process-death cases. The full
matrix includes them again. Browser acceptance used the repository's pinned
Playwright container through Podman because Docker is absent on this host.

Before the final runs, lint/test-fixture failures were corrected and the full
registry fold exposed the finite-disposal handoff bug fixed above. One boundary
run failed while preparing a healthy optional managed Python source; its exact
targeted rerun passed without a source change. The underlying preparation cause
was redacted, so it is not attributed to a particular network or process error.
The final whole-suite result above is the release evidence. An initial protocol
check detected the expected unstaged generated diff; staging the intended wire
contract and rerunning generation produced a clean check.

This host runs Linux. The separate macOS CI lane cannot be executed locally;
remote CI status is reported after the repair push, not inferred from local
results.
