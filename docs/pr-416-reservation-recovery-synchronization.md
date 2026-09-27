# PR #416: resume recovery synchronization

This bounded follow-up repairs the test synchronization in
`agent411_resume_reservation_recovery_requires_rollback_containment_proof`.
It does not change production recovery, physical proof, admission or idle policy.

## Evidence and diagnosis

Starting PR head: `bb99d8aeab75689a92d6eb21289c2ce0e415373e`.
Main and merge base: `ad863a24cf48fbb0d5182746e1046d4b64e8c167`.
[Hosted run 36310624214](https://github.com/Caismis/rustX/actions/runs/36310624214)
failed this test on macOS: Goal idle returned `None`, not `Some(true)`.
The macOS unit command completed with 2607 passed, 1 failed, 2 ignored and
748 filtered tests. Its log does not record lock errno or inherited descriptors;
the original hosted syscall cannot be reconstructed from that assertion alone.

The old test acquired an external `RecoveredPhysicalProof`, dropped it, then
required an immediate production nonblocking reacquisition to succeed. That is
not a valid descriptor-lifetime contract. Rust's Unix implementation uses
`flock` for both platforms. A fork inherits the open file descriptions even when
the descriptors are CLOEXEC; exec or exit closes the inherited copies later.
Closing the original descriptor need not release the lock. See
[Apple's flock documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html)
and [Linux flock documentation](https://man7.org/linux/man-pages/man2/flock.2.html).

The test uses a current-thread Tokio runtime. Between restoring the unproven
reservation and the failed assertion, its wait/acquire calls return immediately:
there is no suspension that lets the spawned recovery reconciler run. Thus a
background recovery claim is not the explanation for this particular cut.
If proof acquisition is unavailable, the synchronous pass releases its
`recovery_inflight` claim but retains `recovery_pending` and `agent.resuming`.
The workspace remains awaiting physical proof. The old committed activation is
already settled. Both pending recovery and the resume reservation legitimately
block `RegistryState::owns_idle_work`; workspace fencing is another consequence
of the same outstanding obligation, not an alternative idle inference.

## Deterministic reproduction and repair

The regression now forks while holding the real proof. The child inherits its
native locks and remains parked on a Unix-stream EOF gate. The parent drops its
copy. No elapsed-time assumption is needed: inheritance occurs at fork, and the
child cannot exit until the parent closes the gate. Its child path only uses
async-signal-safe native operations and `_exit`. A guard closes the gate and
owns the exact child's `waitpid`, including during assertion unwinding.

While the inherited proof is held, the test verifies:

- Production `prove()` returns `None`.
- A reconciliation pass leaves the exact reservation pending and resuming,
  releases its in-flight claim, and keeps workspace authority fenced.
- The previous activation is settled; no unrelated idle commit is in progress.
- No proven rollback exists, and Goal idle remains unavailable.

Keeping the old positive assertion at this deliberately parked cut reproduced
the failure deterministically on Linux (`None` versus `Some(true)`), before the
test synchronization repair. This proves the invalid assumption without relying
on repeated probabilistic failures. It does not claim an instrumented macOS
reproduction of the original hosted execution.

After releasing and joining the descriptor holder, the test awaits the existing
registry `wait_recovery_reconciliation()` completion watch. It then requires
exactly one durable `RolledBack { physical_settlement_proven: true }`, the original
`ClientControl` origin, empty pending/in-flight ownership, no resume reservation,
released workspace authority, an Inactive Agent, and available Goal idle.
Completion of a reconciler is not itself physical proof: the durable and
in-memory assertions remain necessary and are checked before claiming success.
No retry count, interval, timeout or production lock policy changed.

The test uses disk-backed SQLite and reopens a separate store connection.
Reopening must retain the Inactive Agent, no unproven settlement, no replayed
reservation, one historical committed activation, consumed ordinal 2 and next
ordinal 3. All three original admission cases remain covered: Reserved, rollback
without proof, and rollback with proof.

The existing test-only `prove_after_release()` now waits for the allocation
namespace lock as well as the authority locks. Otherwise the same temporary
inheritance could invalidate the helper before authority inspection. Production
`Try` and `AfterSupervision` acquisition remain nonblocking where they were
nonblocking; all exact identity/receipt/continuation validation is unchanged.

The first broad run also failed
`recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
with `Unavailable` versus `Inactive`. That nearby test used the same external
proof/drop/single-probe assumption after releasing its FIFO-held Git helper.
Its earlier bounded recovery owner had already completed while shutdown correctly
reported unresolved containment. The test now starts and joins a recovery owner
after native release, retaining all failed-shutdown, physical exclusion, ordinal,
idempotence and successful-shutdown assertions. This is a directly related test
synchronization repair; production shutdown and reconciliation remain unchanged.

## Validation

Validation results below are Linux results with Rust 1.98.1 and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. macOS is not available locally. CI and its
platform coverage are unchanged; hosted macOS must validate the pushed SHA.

| Command | Result |
| --- | --- |
| `cargo test --all-features runtime::subagent::registry::tests::agent411_resume_reservation_recovery_requires_rollback_containment_proof -- --exact --nocapture` | Three planned focused runs passed, one test each. Also passed in every final broader suite containing it. Repetition is a stress check, not the synchronization argument. |
| `cargo test --all-features runtime::subagent::registry::tests::recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --exact --nocapture` | Passed, one test; subsequently passed in all final broader suites containing it. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed, no warnings. |
| `cargo build --bins` | Passed. |
| `cargo test --lib --all-features physical_recovery::` | 6 passed, 0 ignored, 3381 filtered. |
| `cargo test --lib --all-features subagent` | 337 passed, 0 ignored, 3050 filtered. |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3190 passed, 3 existing ignores, 194 filtered; binary/example targets passed with zero tests. |
| `cargo test --lib --bins --all-features -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` | Exact hosted macOS unit command, executed on Linux: 2636 passed, 2 existing ignores, 749 filtered; binary targets passed with zero tests. Normal harness concurrency, no serialization override. |
| `cargo test --all-features --test subagent` | 45 passed, 0 ignored or filtered. |
| `git diff --check` and `git diff --cached --check` | Passed. |

The existing ignores are `stage_profile_real_create_pipeline`,
`issue419_measure_native_cold_load` (filtered from the hosted-command run), and
`regenerate_committed_fixture_corpus`. No skip, ignore, deadline or CI change was
introduced. The client/protocol/browser matrix was not rerun for this test-only
native repair; previous integration evidence remains in its separate report.

Development failures: the deliberately retained old positive assertion failed
once with the fork gate held. The first broad hosted-command run failed the
related recovered-workspace test (2635 passed, 1 failed, 2 ignored, 749 filtered).
After repairing that test's same synchronization assumption, the entire validation
group above was rerun successfully, not just its failing test. No final validation
command was interrupted; the conversational interruption occurred while the
runner continued and completed successfully.
