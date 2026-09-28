# PR #416: recovery ownership and Workflow cancellation ordering

Follow-up: [publication, terminal answers, and Web waits](pr-416-publication-answer-waits.md)
extends this historical repair; its final validation is recorded separately.

This repair starts at `e407d09d662c22a29a5f3d7f2558b038a61550ad` on
`issue-411-jobs-continuable-subagents`, in
`/home/caismis/Documents/codes/rustX-issue-411`. The fetched main and merge-base
are `6ff49deb4c0855e64f0acaed85ebea2b6a277103`; main was already integrated.
The starting branch was clean, equal to its remote, and 10 commits ahead of main.
PR #416 was open, unmerged, and had no auto-merge request.

## Recovered physical obligations

Recovery already discovered allocations before Reserved and retained them in
`recovery_pending`, with `recovery_unreserved` supplying their association.
But Goal idle checked records/reservations, and runtime drain enumerated records
and failed reservations. Neither included the pending set. An obligation without
any such logical record could therefore disappear from both decisions.

The physical allocation scan now retains exact activation/child Conversation
pairs, including initial allocations before any Agent ownership record. Replay
filters them to the owning parent identity and retains every unrecorded allocation
as a pending physical obligation. The unreserved map stores the Conversation;
Agent association is optional and never invented. A positive proof for an owner
without an Agent record removes that exact obligation and wakes coordination
without creating an Agent snapshot or logical event. The existing initial
uncommitted-allocation regression now holds the real physical lease across
recovery, proves non-idleness and the exact unresolved ID, then discharges it by
positive receipt/lease proof and checks idempotence and permanent ID consumption.

`recovery_pending` now participates in the registry's shared `owns_idle_work`
predicate, under the same mutex as admission. Goal idle and runtime residency idle
use that predicate; a missing activation record cannot authorize idle eviction.
The residency query performs only in-memory reads under its coordinator lock. `unproven_settlements()` includes that exact
set, sorted and deduplicated alongside terminal and failed-staging obligations.
This existing domain API is the runtime drain's physical-failure source. No new
caller-specific filesystem checks or alternate settlement authority were added.
The bounded reconciliation owner's exit leaves the pending set unchanged.

The runtime awaits reconciliation, enumerates unresolved physical ownership and
returns failed logical and physical drain outcomes before `mark_quiescent()`.
SessionRuntimeManager's `delete_session` requires retirement to return
`WriterAbsent`; `spawn_unload` returns on shutdown failure before removing the
composition. `RetirementUnproven` preserves the writer slot and deletion fence.
The crash fixture exercises the actual runtime boundary used by this path; it
does not directly compose a SessionRuntimeManager deletion request. The existing
`deletion_retirement_failure_retains_writer_slot_and_session_fence` regression
covers that manager guard.

The existing exact proof path is unchanged: it captures an immutable activation
claim, acquires authority/receipt/lease proof outside the registry lock, retains
proof locks while consuming the claim, removes the exact pending ID, and publishes
state plus a mailbox wake. Unreserved allocation proof seals physical authority
without inventing a Reserved or rollback event. Duplicate reconciliation changes
neither the state version nor the journal. PID absence, a clean tree, missing
runtime files, and elapsed retry time never discharge an obligation.

The extended
`recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
regression starts a durable isolated Agent, finishes its first activation, then
blocks a real recovered-workspace Git fsmonitor helper on named pipes during
resume. It aborts the parent admission owner before Reserved and reopens SQLite.
It proves send refusal, the exact unresolved ID, Goal non-idleness, Unavailable
Agent state, failed physical lifetime/shutdown, non-quiescence, and a consumed
ordinal even after bounded reconciliation finishes. Only then does it release
the FIFO, await exact kernel lease/receipt proof, and reconcile. Goal idle and
Inactive state return, a newly composed recovered runtime settles and becomes
quiescent, duplicate reconciliation is inert, and the old ID cannot be reserved.
A failed runtime drain retains its existing one-shot failure; the test does not
reset that result to success.

## Workflow cancellation ordering

The Agent Loop handles the reserved `workflow_output` call before the ordinary
Tool Plane. Schema-valid submission commits the existing Workflow output latch;
cancellation and submission arbitrate through that latch. Successful submission
returns Completed, and the attempt's durable terminal publishes AttemptCompleted.
The child may report success only from that completed terminal plus the committed
latch value. The parent atomically publishes WorkflowAgentOutputCommitted with
the successful child terminal and exposes the committed Workflow value.

`await_terminal` now preserves the authoritative AttemptCompleted observation.
The enclosing ownership-aware child loop applies absorbing cancellation only to
normal continuable Agents. For Workflow ownership, the first attempt terminal
remains final, and the existing output-latch validation still determines whether
Completed can produce Succeeded. Late cancellation cannot replace a committed
Workflow success. Early cancellation still follows the existing runtime intent,
AgentCancellation and Workflow latch and yields AttemptCancelled. Workflow never
enters close/seal/reopen. No second cancellation state machine was introduced.

Two regressions exercise the real semantic child path, then forward its actual
result through the parent driver/publication path:

- `workflow_output_committed_before_cancel_remains_successful` commits the real
  reserved output and durable AttemptCompleted, holds that observation, sends one
  Cancel and observes the runtime cancellation call completing, then releases the
  older terminal. It preserves Succeeded and the value, one parent output commit,
  no parent Pending Inbound, no seal probe/reopen frames, and one model request.
- `workflow_cancel_before_output_commit_remains_cancelled` parks the real admitted
  attempt at the existing request-start barrier, delivers Cancel through the same
  dispatcher, waits for cancellation to commit, then releases the barrier. It
  reports Cancelled with no latch value, parent output commit or Pending Inbound,
  and zero model requests or seal probes.

The cancellation test notification now fires after the runtime cancellation call,
so releasing the early-output barrier is ordered by the actual cancellation
commit. No sleep or widened timeout establishes either race.

## Adjacent contracts and locks

Recovered workspace Git remains supervised under activation physical authority
before Reserved. Recovery excludes workspace reuse, and Agent snapshot/admission
consult that exclusion: the older proven terminal cannot make the Agent executable.
Admitting, Active, Stopping, Inactive and Unavailable retain their existing owner
transitions. Reconciliation releases only AwaitingPhysicalProof; independent
workspace poison remains absorbing. Positive physical allocations still reseed
the ordinal allocator even when no durable admission exists.

Normal Agent cancellation still spans terminal observation, admission close and
seal/reopen. The single-Cancel/stale-Completed regression, pre-admission and
in-flight cancellation tests, and durable-guidance continuation tests are retained.
Guidance write-start evidence and acknowledgements are unchanged: no write is
NotDelivered, uncertain acknowledgement after a write is DeliveryUnknown, explicit
refusal stays refusal, and acknowledged durable acceptance is accepted. No replay
was added. TUI and Web failed-send drafts retain their existing behavior.

Filesystem allocation/scans, recovered Git, physical receipt reads and flock,
SQLite reads/appends, process waits and positive-proof waiting execute outside
the registry mutex. Recovery captures an exact claim under lock, performs proof
and durable work outside it, then revalidates under lock. Workspace availability
changes are atomic in-memory transitions; snapshot publication and wakeups add no
filesystem or process work to the critical section. The new idle/drain checks
only inspect in-memory IDs under lock.

Runtime Client v52, App Server v26, child IPC v29 and schema 47 are unchanged.
No protocol regeneration changes, compatibility paths or migration were needed.

## Validation

The completed validation commands and diagnostic failures are recorded below. Linux execution covers
both existing platform-neutral logic and the Unix physical ownership paths.
macOS execution remains CI-only; this repair introduces no new platform API.

During development, the extended runtime fixture first hit the runtime's exact
mailbox-identity guard, then its pristine-registry guard. The fixture now composes
a pristine registry with the runtime's own mailbox and lets real runtime startup
perform recovery. Strict Clippy also caught test placement/length and the longer
reconciliation function; test placement was corrected and the repeated per-Agent
recovery query was consolidated. The crash regression remains one explicitly
annotated test boundary.

An exploratory `cargo test --lib --all-features workflow_ -- --nocapture` run was
stopped after an existing disposal test stalled. The two new Workflow tests had
passed; the stalled test subsequently passed in isolation, and the complete
library suite passed. Intermediate broad runs were stopped/restarted while the
ownership audit added initial-allocation and residency coverage; they are not
counted as completed validation. An initial mandatory boundary run had 192 passes
and one failure because uv could not reach PyPI for FastMCP setup. The exact
FastMCP case passed unchanged on retry; the full boundary suite was then rerun.
No assertion or timeout was weakened, and no test was newly skipped or ignored.

### Completed Rust validation

Commands ran from the repair worktree root. The boundary suites were serialized
and required the provider emulator. The two existing library ignores are the
instrumented stage profile and fixture-corpus writer; the five provider ignores
require live credentials. The 193 boundary tests excluded from the broad library
command were all run separately, not omitted from validation.

| Command | Final result |
| --- | --- |
| `cargo fmt --all` | Applied formatting |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features subagent` | 332 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,172 library tests passed; 2 existing ignores; all bin/example harnesses passed |
| `cargo test --test contracts --test provider --all-features` | 28 contracts and 166 provider tests passed; 5 existing live-credential ignores |
| `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 193 passed on complete rerun |
| `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 419 passed across seven targets |

Additional diagnostic invocations actually run:

| Command | Result |
| --- | --- |
| `cargo test --lib --all-features workflow_ -- --nocapture` | Interrupted exploratory run, as described above; not counted as a pass |
| `cargo test --lib --all-features recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --nocapture` | Initial fixture mailbox guard failure; corrected and passed in final focused/full runs |
| `cargo test --lib --all-features subagent -- --nocapture` | Initial fixture pristine-registry guard failure (331 passed, 1 failed); corrected and final 332 passed |
| `cargo test --lib --all-features deletion_workflow_disposal_first_excludes_preflight_through_physical_settlement` | 1 passed |
| `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features boundary_suites::mcp_mrtr_managed::a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction` | 1 passed after the PyPI setup failure |

### Client and support validation

| Directory | Command | Result |
| --- | --- | --- |
| `test-support/fake-provider` | `uv run --frozen pytest` | 51 passed |
| `dev` | `pnpm typecheck` | Passed |
| `dev` | `pnpm test` | 37 passed |
| `protocol/app-server` | `pnpm check` | Generation and drift checks passed; no generated changes |
| `protocol/app-server` | `pnpm typecheck` | Passed |
| `tui` | `pnpm typecheck` | Passed |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 879 passed, none skipped |
| `web-console` | `pnpm typecheck` | Passed |
| `web-console` | `pnpm test` | 1,082 passed across 59 files |
| `web-console` | `pnpm check:i18n` | Passed |
| `web-console` | `pnpm check:provenance` | Passed: 143 source records and 131 dependency notices |
| `web-console` | `pnpm build` | Passed; existing Vite chunk-size warning |
| `web-console` | `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh integrations.spec.ts` | 1 passed unchanged after the full-run teardown failure |

The first `CONTAINER_ENGINE=podman pnpm test:e2e` run had 109 passes and one
integrations-case teardown failure. Its trace shows the test's final assertion
completed, followed by a fetch socket closure and browser-context teardown
timeout. The original trace and log were preserved; no fixture, assertion or
timeout was changed. The isolated case passed in a fresh pinned browser container.

The unchanged full `CONTAINER_ENGINE=podman pnpm test:e2e` rerun then passed all
110 tests, including the native Workflow and Session deletion browser paths.
`git diff --check` and `git diff --cached --check` both passed; the staged check
was repeated after staging the final repair and this report.
