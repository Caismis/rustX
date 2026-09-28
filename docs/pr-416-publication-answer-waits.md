# PR #416: activation publication, terminal answers, and Web request lifetime

Follow-up: [TUI admission and native portability](pr-416-tui-portability.md).

Repair worktree: `/home/caismis/Documents/codes/rustX-issue-411`.
Branch: `issue-411-jobs-continuable-subagents`. Starting HEAD and fetched PR head:
`0a9df8b84fe906976b34d872b41a503aaf45f5ee`. Fetched main, PR base and merge base:
`6ff49deb4c0855e64f0acaed85ebea2b6a277103`. The starting worktree was clean,
equal to its remote, and 11 commits ahead of main. The primary checkout's
untracked `.playwright-mcp/` was left alone. No reset or replacement PR was used.

## Physical publication contract

The original `ParentPhysicalLease::reserve` created the published activation name
before initializing its lease and receipt. A failure in that interval consumed
an ordinal but produced evidence that exact physical recovery could never prove.

`physical_recovery.rs` now separates allocation identity from executable authority:

1. Acquire the per-conversation physical namespace `.allocation-owner` file lock.
2. Exclusively create `.pending-<activation>`. Fsync that directory, then its
   parent and every linking ancestor through ProductRoot. This durably consumes
   the ordinal independently of lease/receipt initialization.
3. Create, exclusively lock and fsync `physical-owner`. Write the exact Unstarted
   receipt through the existing file-fsync, rename, directory-fsync protocol.
4. Atomically rename the complete private directory to `<activation>` and fsync
   its parent. Only then may the authority handle return, be inherited by a
   child, or reserve a supervised workspace helper.

Recovery acquires the same namespace lock before classifying an unpublished
allocation. A live initializer therefore cannot race recovery into publishing or
spawning after its obligation is released. Recovery seals private names by rename
to `.abandoned-<activation>` and a parent-directory barrier (repeated on retry).
The namespace lock remains in `RecoveredPhysicalProof` through the existing
in-memory settlement cut. Both private and abandoned names reseed the ordinal
allocator, and reserve rejects all three consumed forms. Nothing deletes them
before Session deletion.

Published names always take the original exact lease/identity/receipt and helper
proof path. Unstarted plus exclusive lease acquisition can be sealed; Running,
missing, corrupt, mismatched or held authority remains unresolved. The main lease
excludes further continuation creation, and all proof locks survive the existing
durable append and registry publication cut. Helpers keep their independent UUID
staging protocol beneath published parent authority.

`AllocationError` reports the storage owner's positive consumption fact.
`finish_physical_reservation` handles errors in initial admission and resume:
consumed allocations enter the existing `recovery_unreserved`/`recovery_pending`
owner immediately, fence workspace reuse where applicable, wake coordination and
attempt exact reconciliation outside the registry mutex. Thus an error after
publication is not interpreted as no authority, even before restart. Existing
Goal/shutdown reconciliation and later reopen retain any unresolved obligation.
There are no filesystem, SQLite or process waits under the registry mutex.

No uncommitted allocation creates an Agent row, Reserved/rollback event or logical
terminal. Existing pending participation in Goal idle, residency and drain remains
intact. SessionRuntimeManager still requires successful physical retirement before
deletion; failed retirement retains its writer slot and Session fence.

These tests exercise ordered process/task-visible interruptions and injected I/O
errors, not power loss. The fsync ordering above is the durability argument; no
claim of tested storage-device crash durability is made.

## Exact answer ownership

`AttemptTerminal::Completed` retains the authoritative AttemptId from
`AttemptCompleted`. Normal guidance draining replaces it with each later attempt's
terminal, so the selected identity is the activation's concluding attempt.
`ConversationRuntime::durable_final_assistant` reads the last
`AssistantMessageCommitted` Journal fact scoped to that exact attempt using the
existing indexed bounded fact reader, then loads that exact canonical MessageId.
The SQLite canonical publication validator already binds that message, attempt,
turn and frozen publication generation in one transaction.

`final_answer` extracts supported ordinary Text only from that selected message
and retains the existing UTF-8 result bound. A refusal-only/textless message
produces the existing explicit no-final-answer failure; older messages are never
searched for a replacement. Canonical reads work while the coordinator still has
its conversation state checked out. Streamed provisional content and publication
audits do not supply answers. No provider adapter changes were made.

Workflow children still use committed reserved `workflow_output` plus successful
attempt terminal, independently of ordinary text. Late cancellation ordering,
normal absorbing cancellation, accepted guidance durability, the pre-Delegate
turn gate and exactly-once parent publication remain unchanged.

## Web lifetime and capacity

The owning layer is `web-console/src/client/app-server.ts`, not ActivityCards.
The existing server actually awaits runtime settlement for waits and cancellation,
and durable acceptance/admission for sendMessage. TUI has no analogous short
response timer and does not replay lost Agent waits.

| Methods | Response deadline | Outstanding capacity |
| --- | --- | --- |
| `agent/wait`, `job/wait` | None | 4 shared observation slots |
| `agent/sendMessage` | None | 2 admission slots |
| `agent/interrupt`, `job/cancel` | None | 2 settlement-control slots |
| Ordinary RPCs | Existing 30 seconds | 8 transmitted slots |

Domain lanes reject excess local admission before transmission. They do not queue
unboundedly or occupy ordinary RPC slots; admissions cannot consume interrupt/cancel
slots. The total transmitted bound is 16, matching the existing server limit.
The existing overall 64 pending/uncertainty bounds remain. An ordinary RPC timeout
retains its deliberately tested response-loss disconnect behavior.

Actual close/error settles each pending operation once. Sent mutations retain
OutcomeUncertain; observations end without cancelling work. No operation is
replayed on reconnect, particularly Agent waits that might capture a newer
activation. Exact generation/attachment checks continue to fence adoption and
failed sends retain drafts. No heartbeat, notification-based timer reset, polling,
wire change, version bump, feature flag or compatibility path was added.

## Deterministic regressions

- `interrupted_authority_publication_reopens_without_logical_facts`: real
  reservation callbacks at Created, LeaseCreated, Initialized and Published;
  real SQLite reopen twice; empty Agent/event history; consumed ordinal and
  same-ID refusal; Goal idle/physical convergence; duplicate reconciliation;
  actual runtime physical lifetime and shutdown reach Quiescent.
- `live_private_initializer_excludes_recovery_at_every_publication_boundary`:
  synchronous callbacks run under the real initializer namespace lock at each
  boundary; recovery cannot prove it, and no handle has escaped. A returned
  published owner also excludes proof until exact descriptor release.
- `published_initialization_error_never_excuses_missing_or_corrupt_evidence`:
  post-publication injected error with missing lease, missing receipt, corrupt
  receipt and wrong identity; proof stays unresolved and reuse is refused.
- `published_initialization_error_retains_live_registry_obligation_until_exact_proof`:
  the live caller's actual error handler and reopened registry both retain a
  corrupt published obligation. Only restoration and validation of its exact
  original receipt releases idle; no logical facts are manufactured.
- `deletion_retires_abandoned_unpublished_activation_without_inventing_an_agent`:
  all three pre-publication interruptions, actual SessionRuntimeManager load,
  retirement and deletion. Inspection leases are released before cleanup.
- `deletion_keeps_live_published_activation_fenced`: hold the real published
  lease through actual failed retirement/deletion; the Session, writer slot and
  non-quiescent runtime remain. Bounded retry expiry supplies no proof.
- Existing inherited-child, Running continuation, recovered Git, missing-proof,
  Workflow ordering, cancellation and shutdown regressions remain in the suites.
- `persistent_child_result_belongs_to_concluding_terminal_attempt`: real scripted
  child semantic loop, IPC Result, on-disk child SQLite reopened for each
  activation, and actual parent driver/publication. A→refusal fails, A→B returns
  B, earlier committed tool narration cannot replace a final refusal, and two
  guidance-driven attempts return the concluding answer. Same Agent/conversation,
  different activation, exactly one parent terminal and matching durable inbound
  report are asserted. The coordinator's before-restoration gate additionally
  verifies canonical selection while it still owns the current attempt. The
  guidance seal gate establishes accepted work before concluding termination.
- `request-lifetime.test.ts`: real AppServerClient, fake socket/server and fake
  timers. All five methods remain pending through 120 seconds without traffic;
  waits also receive notifications while full capacity is exercised. Ordinary
  inspection and interrupt/cancel complete, target replies settle once, and real
  close/error retains uncertainty without replay or obsolete-generation adoption.
  Both admission/control capacity limits are exercised independently.
- `client.test.ts` / `a timed-out turn acknowledgement closes transport, keeps uncertainty, and does not replay` keeps the ordinary 30-second disconnect contract explicit.
- `activity-controls.test.tsx`: existing independent wait/interrupt/cancel controls
  and failed-send drafts, plus a late wait result after detach/reattach that
  cannot publish its obsolete activation. Transport timer/queue proof remains in
  the client tests, separate from presentation tests.

## Validation and diagnostic history

This host is Linux;
macOS validation is not claimed. Existing ignored live-provider/maintenance tests
are not newly skipped. Browser acceptance uses the repository's digest-pinned
Playwright 1.63.0 Podman image against the final built binaries.

Development failures: a fixture initially waited for the next fake-socket request
instead of the request already sent; its synchronization was corrected without
changing deadlines. Two command setup errors used the wrong working directory.
An initial Rust edit used a pattern placeholder in an expression; additional
CoordinatorProbe fixture initializers needed the new optional gate field.
The first narration fixture used an unexposed tool, producing a real UnknownTool
failure rather than the intended final refusal. It now registers and explicitly
exposes the scripted tool. An early assertion left a gate parked during test
unwinding; that run was terminated after stack inspection, and the gate now has
scoped release and does not retain the probe mutex while parked. Session deletion
initially retained an inspection lease, correctly returning incomplete cleanup;
the fixture now releases that lease before requesting deletion. Strict lint found
a test-only import, an avoidable clone assignment and a dropped reference; these
were corrected. None of these diagnostic runs count as final validation.

Baseline executions restored only the old allocation body (with identical boundary
fault callbacks) and old historical-answer extractor, then restored the repaired
source in a finally block. The physical regression failed because recovered idle
ownership remained unresolved after the Created interruption (14.92 seconds);
the answer regression failed with `Some("A")` instead of no ordinary answer for
the refusal (0.18 seconds). The five Web lifetime cases run against the original
client all failed because its connection became stale. These expected failures
are defect reproduction, not successful validation; selected-test filters were
used for these diagnostic runs only.


The first broad Rust command was interrupted after 399 seconds: three existing
workspace-candidate tests were parked, with no live child process remaining.
A second unchanged full run was interrupted after 223 seconds, again with parked
process-backed candidate tests; it also reported
`cancel_during_close_admission_is_absorbing_and_preserves_guidance` as failed before
the harness could print its captured assertion. Stack inspection showed parked
Tokio runtimes, not a held allocation lock. A diagnostic SIGCHLD wake did not
unstick the second run. These runs are not counted as passes. The subsequent investigation established
the settlement stall described below. The cancellation failure's captured
assertion was unavailable when the stalled harness was interrupted. The cancellation test then passed once under
Cargo and 100 direct unchanged diagnostic repetitions; those do not replace full
validation. The next full run disabled output capture only (`RUST_TEST_NOCAPTURE=1`)
so failures could be inspected immediately. No source, timeout or assertion was
changed for these retries.

| Directory | Exact command | Final result |
| --- | --- | --- |
| repository | `cargo fmt --all -- --check` | Passed |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| repository | `cargo build --bins` | Passed; browser tests use these binaries |
| repository | `cargo test --lib --all-features subagent` | 337 passed, 0 ignored |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,180 library tests passed; 2 existing ignored; bin/example targets passed with 0 tests |
| repository | `cargo test --test contracts --test provider --all-features` | 28 contracts and 166 provider tests passed; 5 existing live-provider ignores |
| repository | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 193 passed, 0 ignored |
| repository | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 419 passed, 0 ignored: CFG3 catalog 26, managed output 5, conformance 22, durable 129, process 62, subagent 45, tools 130 |
| repository | `git diff --check` | Passed |
| repository | `git diff --cached --check` | Passed |
| `test-support/fake-provider` | `uv run --frozen pytest` | 51 passed |
| `dev` | `pnpm typecheck` | Passed |
| `dev` | `pnpm test` | 37 passed, 0 skipped |
| `protocol/app-server` | `pnpm check` | Passed; generated artifacts unchanged |
| `protocol/app-server` | `pnpm typecheck` | Passed |
| `tui` | `pnpm typecheck` | Passed |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 879 passed, 0 skipped |
| `web-console` | `pnpm typecheck` | Passed |
| `web-console` | `pnpm test` | 1,093 passed across 60 files |
| `web-console` | `pnpm check:i18n` | Passed |
| `web-console` | `pnpm check:provenance` | Passed |
| `web-console` | `pnpm build` | Passed; existing bundle-size warning |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | 110 passed; pinned image, final binaries |


### Validation-discovered supervisor status ownership

The third broad run (output capture disabled) was interrupted after 321 seconds.
Inspection of a parked Git task showed `ProcessLifecycle::Terminal`, supervisor
channel Lost, no exit status/failure/cancellation, and both output collectors
already complete. The runner therefore had physical proof but no semantic
outcome. The existing inner supervisor's group-scoped consuming `waitid` could
reap the shell between hygiene passes without publishing `ShellExited`.

The narrow correction makes that inner group gate a non-consuming `WNOWAIT`
observation. The existing hygiene owner alone consumes and publishes shell exit;
`ECHILD` still requires every owned child to be reaped. Outer retained-anchor
containment and proof are unchanged. This is not a timeout or relaxed proof.
`group_proof_cannot_consume_shell_exit_between_hygiene_passes` uses a pipe to
release a real shell after the first empty hygiene pass, then blocking `WNOWAIT`
to establish its exited-but-unreaped state before calling the actual production
group gate. The exact shell exit must remain available to hygiene, followed by
`ECHILD`. This interleaving requires neither a sleep nor scheduler luck.

The regression failed before the correction: the group observation consumed exit
code 23 and hygiene received `ECHILD`. It passed after `WNOWAIT`. Strict Clippy
then required the test's child handle to be explicitly waited; the fixture now
uses `Child::wait` for that consuming assertion rather than raw `waitpid` (the
same exit-code assertion and no lint suppression). The full final matrix was
restarted after this fixture correction.

The final broad command passed in 107 seconds including target preparation
(94 seconds of library tests), with default test parallelism. Its cancellation
regression also passed. The final Rust runner inherited
`RUST_TEST_NOCAPTURE=1`, which changes output capture only. The sole broad-suite
filter is the established `boundary_suites::` exclusion; those 193 tests are
executed separately below. The two existing ignored library tests are
`stage_profile_real_create_pipeline` and `regenerate_committed_fixture_corpus`.
The five provider ignores require external credentials/network: Anthropic
Messages, OpenAI Chat, OpenAI Chat tool call, OpenAI Responses and stateless
Responses. No ignores or skips were added.


Final focused commands also passed (selected-test filters are intentional):

```text
cargo test --lib --all-features physical_recovery::inheritance_tests
cargo test --lib --all-features persistent_child_result_belongs_to_concluding_terminal_attempt
cargo test --lib --all-features deletion_retires_abandoned_unpublished_activation_without_inventing_an_agent
cargo test --lib --all-features group_proof_cannot_consume_shell_exit_between_hygiene_passes
```

These ran 6, 1, 1 and 1 tests respectively. Registry interruption/shutdown and
post-publication-error cases also passed in both the 337-test subagent run and
the final broad run. All client checks were repeated; protocol/TUI checks and all
110 browser cases passed after the final supervisor binary rebuild. Browser
acceptance used the repository-pinned Podman image. No test timeout was increased,
assertion weakened, or skip added; provider adapters, wire schemas and versions
are unchanged.

All required local validation completed successfully on Linux. macOS and actual
power-loss testing remain unexecuted. The earlier isolated cancellation failure
was not reproduced in 100 diagnostic repetitions or the final full run; its
captured assertion was unavailable after interrupting the independently stalled
harness. It is disclosed rather than attributed to an unproven cause.
