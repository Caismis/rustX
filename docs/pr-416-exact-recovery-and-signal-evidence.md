# PR #416: exact recovery waits and native cancellation evidence

Starting head: `a6a18d52422ff7e57695aa182f20dea9b287953a`.
Fetched main/merge base: `ad863a24cf48fbb0d5182746e1046d4b64e8c167`.

## Exact activation recovery waiting

The previous transfer of consumed pre-Reserved authority into recovery ownership
was correct. Its admission owner then joined the registry-wide recovery completion
watch, which unnecessarily waited for unrelated Agents' pending allocations.

Recovery still has one shared bounded worker. `wait_recovery_settlement_for` now
subscribes to state changes and worker completion before inspecting its exact
SubagentId under the registry mutex. It returns settled only after that exact
pending obligation and claim are released, or unresolved after the responsible
worker has finished and no exact pass still owns the claim. It performs no proof
I/O and has no polling timer. The admission owner retains its existing result
classification and rechecks its exact obligation.

Each successful reconciliation removes its exact inflight claim at the same
settlement cut as pending removal, while retaining the acquired proof locks.
A pass releases unsuccessful claims on drop and publishes a state-version change,
so a waiter cannot miss that boundary. A slow or unresolved different entry cannot
extend an already-proven activation's semantic lifetime. The registry-wide wait
remains appropriate for startup/drain callers that actually own all obligations.

The transfer, workspace fence, deletion proofs, ordinal consumption, and absence
of invented Reserved/RolledBack facts for unadmitted allocations are unchanged.
No proof is inferred from handle drop, a worker deadline or clean Git state.

`pre_reserved_wait_settles_independently_of_another_agents_recovery` creates two
real durable Agents and exact physical allocations. A channel parks the shared
worker after claiming A's allocation X; a retained descriptor makes X unprovable.
B's real resume is cancelled before Reserved and transfers Y into recovery. Its
retained descriptor holds Y pending. Explicit unlock releases Y, reconciliation
settles it, and B's captured wait/interrupt completes while X's worker and lease
are still held. B is Inactive, X stays pending, and Goal idle stays blocked.
Releasing X subsequently permits global recovery completion. The deadline is only
a deadlock guard; ordering comes from claims, channels and descriptor ownership.
Temporarily restoring the old global wait made this regression fail at that guard;
the corrected source was immediately restored.

## Bash cancellation: evidence and limits

Historical run [36317341024](https://github.com/Caismis/rustX/actions/runs/36317341024)
reported Cancelled without the TERM marker. It did not record control receipt,
killpg results, shell status or escalation ordering. Those historical facts cannot
be reconstructed from the Job result. No exact cause for that failure is claimed.

During this repair, starting-head run
[36327293316](https://github.com/Caismis/rustX/actions/runs/36327293316) completed
macOS successfully: 2,610 unit/boundary passes (two existing ignores), and external
catalog 26, managed-output 5, conformance 22, durable 129, process 59, subagent 45,
tools 130. Both the original marker regression and pipe-gated trap regression
passed. This is actual macOS evidence for the starting SHA, not an explanation of
the earlier failure and not validation of the new SHA. No local macOS is available.

The unchanged production cancellation contract is:

1. Background cancellation commits intent and signals its executor.
2. The runner writes TERMINATE. Only the inner interprets that frame.
3. The inner calls killpg(owned PGID, SIGTERM), then starts the existing grace.
4. The sole shell reaper reports exit. Non-consuming WNOWAIT observations never
   consume that result or release the retained inner anchor.
5. Grace expiry allows self/group KILL if work remains. Abnormal inner loss has
   a distinct fail-safe containment path; it must not be described as normal
   grace escalation.
6. Darwin has no subreaper. After shell exit, its outer fallback contains the
   retained group and proves group absence rather than inferring all descendants
   reaped from ECHILD. The injected EXIT `wait` is a convenience, not proof.
7. The runner requires physical terminal evidence and direct-child reap before
   publishing a result; Cancelled names intent, not delivery of a particular signal.

The [Bash manual](https://www.gnu.org/software/bash/manual/html_node/Signals.html)
explains that foreground-command waiting can defer a trap. A successful group
signal is not proof that each process handled it, nor that a trap finished before
legitimate grace expiry. This does not establish why the historical marker was
missing, so its assertion remains intact. Its trap now separately records entry
with a builtin and the marker command's exit status for failure diagnostics.

### Bounded diagnostics, never authority

An explicitly configured per-invocation regression trace records only scalar
native facts: successful/failed TERMINATE write, inner receipt, shell PID/PGID at
spawn and signal boundary, TERM syscall result (zero or errno), grace expiry,
KILL intent, fallback KILL syscall result, shell/inner raw exit status, control
failure occurrence, terminal publication/observation and direct-child reap.

Each runner/supervisor is bounded to 32 entries. Records contain no command,
environment values or output. The optional trace is not read by production
ownership logic. The dedicated supervisors own their recorder; the parallel Rust
test process uses an invocation-local recorder. No trace descriptor is passed to the command. Trace and gate environment keys are removed before launching Bash.
Self-KILL cannot report its return, so its record explicitly has no result; the
outer independently reports its fallback syscall result and physical proof.
The grace deadline uses the syscall-completion instant, so trace I/O cannot
extend the configured grace. Native syscall success remains distinct from Bash
trap-entry evidence.

The test-only before-TERM socket parks the inner after parsing TERMINATE but before
issuing TERM. It uses a short, uniquely owned `/tmp` namespace. EOF releases a
failed fixture; the gate is never armed by ordinary runtime configuration.

`background_cancel_records_term_before_trap_and_physical_terminal` uses that
socket plus nonblocking FIFO readiness. It proves trap installation precedes
cancel, the Job remains Cancelling with the pre-signal gate held, no signal has
occurred at that cut, TERM targets the shell's observed PGID and succeeds, and
actual trap entry still cannot permit terminal publication while its release
pipe is held. After release it requires marker, terminal observation and direct
reap. Any normal Darwin fallback KILL must follow the shell-exit report; no grace
expiry occurs in this successful TERM path. Nonblocking FIFO reads cannot strand
blocking-pool threads when a liveness guard fails.

`bash_kill_escalates_when_term_is_ignored` retains its existing command/timeout and
now requires native evidence ordered TERM-success < grace-expiry < KILL. The
original background marker assertion and the earlier pipe-gated unit regression
remain. No TERM-grace increase, repeated signals, weakened assertion, platform
skip, suite serialization or new retry policy was introduced.

**Remaining blocker:** the old macOS failure is still unexplained. This commit
adds the missing discriminating evidence and deterministic boundary coverage; it
does not claim a production signaling repair or merge readiness. Hosted macOS
must validate the final SHA. A future failure will distinguish receipt, syscall,
trap entry, marker failure and escalation instead of inferring them from Cancelled.

## Validation

Final commands/results are recorded below after execution. All local execution is
Linux/Rust 1.98.1 with the required provider emulator enabled. No protocol types,
wire versions, identity documentation or generated artifacts are changed.

### Development diagnostics

- The first new recovery test compile failed because `Duration` was not in scope;
  using its qualified name corrected it.
- An automated insertion initially split a supervisor module-doc sentence;
  parsing/formatting failed before execution. The insertion was moved to the
  module's item boundary.
- The first diagnostic build then found private-module imports and two existing
  interactive callers destructuring the formerly unit-returning TERMINATE helper.
  Imports now use the existing supervisor re-export; those callers explicitly
  ignore the new write-success observation. Their behavior is unchanged.
- The initial strict Clippy run rejected similar local names and a collapsible
  conditional; both were corrected without lint suppression.
- The intentional before-fix global-wait run failed at the five-second deadlock
  guard while A remained parked. Unwinding closed the test channel and its parked
  worker reported `RecvError`; this belongs to the intentional negative run.
  The source was restored in a `finally` block and the repaired test passed.

No failed diagnostic run is counted as validation, and repeated success is not
the argument for either synchronization contract. The original historical macOS
failure is retained as unexplained evidence, not classified as infrastructure.

- The first broad hosted-unit command on Linux had 2,638 passes and one
  managed-FastMCP setup failure: a PyPI wheel download timed out. The exact wheel
  was then fetched successfully (HTTP 200, 48,172 bytes), and the full command
  was rerun. That run had 2,634 passes and five preparation failures: two explicit
  PyPI `Network is unreachable` errors and three opaque source-unavailable errors.
  The following external run passed its other six targets but tools had 124 passes
  and seven preparation failures: six explicit PyPI network-unreachable errors,
  one opaque source-unavailable error. These failures are not hidden or attributed
  to a code fix; opaque nested causes remain unestablished.
- Final review anchored the grace deadline at syscall completion before trace I/O,
  preventing diagnostic writes from extending grace. Validation was then rerun on
  that final source. Earlier successful runs are supplementary, not substitutes.

### Final local commands and results

These are final-source results. The failed combined hosted-unit command is **not**
reported as passed. It finished with 2,635 passes, four failures and two existing
ignores: `mcp_tasks_managed` explicitly reported PyPI network unreachable;
`managed_selection` and both `runtime_client::python_capability` cases reported
source preparation unavailable without nested causes. No further unchanged rerun
was performed. The separate boundary suite and subsequent full external command
passed, but do not erase this failed command or prove its opaque causes.

| Directory | Exact command | Result |
| --- | --- | --- |
| `.` | `cargo fmt --all -- --check` | Pass |
| `.` | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `.` | `cargo build --bins` | Pass |
| `.` | `cargo test --lib --all-features subagent` | 338 passed, 0 failed, 0 ignored |
| `.` | `cargo test --lib --all-features deletion_tests` | 62 passed, 0 failed, 0 ignored |
| `.` | `cargo test --lib --all-features tools::native::bash::tests` | 37 passed, 0 failed, 0 ignored |
| `.` | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3193 passed, 0 failed, 3 ignored |
| `.` | `cargo test --test contracts --test provider --all-features` | 28 passed, 0 failed, 0 ignored; 166 passed, 0 failed, 5 ignored |
| `.` | `cargo test --lib --all-features -- boundary_suites::` | 194 passed, 0 failed, 0 ignored |
| `.` | `cargo test --lib --bins --all-features -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` | 2635 passed, 4 failed, 2 ignored |
| `.` | `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 26 passed, 0 failed, 0 ignored; 5 passed, 0 failed, 0 ignored; 22 passed, 0 failed, 0 ignored; 129 passed, 0 failed, 0 ignored; 62 passed, 0 failed, 0 ignored; 45 passed, 0 failed, 0 ignored; 131 passed, 0 failed, 0 ignored |
| `protocol/app-server` | `pnpm check` | Pass |
| `protocol/app-server` | `pnpm typecheck` | Pass |
| `tui` | `pnpm typecheck` | Pass |
| `tui` | `pnpm test` | 893 passed |
| `web-console` | `pnpm typecheck` | Pass |
| `web-console` | `pnpm test` | 1,094 passed |
| `web-console` | `pnpm check:i18n` | Pass |
| `web-console` | `pnpm check:provenance` | Pass |
| `web-console` | `pnpm build` | Pass |
| `.` | `cargo test --all-features --test tools bash::background_cancel_records_term_before_trap_and_physical_terminal -- --exact --nocapture` | 1 passed |
| `.` | `cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture` | 1 passed |
| `.` | `cargo test --all-features --test tools bash::bash_kill_escalates_when_term_is_ignored -- --exact --nocapture` | 1 passed |
| `.` | `git diff --check` and `git diff --cached --check` | Pass |

Focused recovery commands also passed (then passed again in the final subagent /
deletion groups):

```sh
cargo test --lib --all-features pre_reserved_wait_settles_independently -- --nocapture
cargo test --lib --all-features recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --nocapture
cargo test --lib --all-features recovered_physical_receipt_requires_owner_release_and_durable_proof -- --nocapture
cargo test --lib --all-features agent411_resume_reservation_recovery_requires_rollback_containment_proof -- --nocapture
cargo test --lib --all-features pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof -- --nocapture
```

The three existing broad-unit ignores are fixture regeneration and startup/stage
measurement tests. The five existing provider ignores require external credentials.
No new ignores were added. All selected suites ran at default test concurrency.
The final hosted-unit command was executed on Linux, not macOS. No browser E2E
rerun was required by this bounded, wire-unchanged repair; no previous browser run
is claimed as final-source evidence here. No hosted CI was rerun or watched after
push. Generated App Server v27 artifacts and both identity contracts are unchanged.
