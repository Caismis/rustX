# PR #416: resume recovery synchronization

This bounded follow-up repairs the test synchronization in
`agent411_resume_reservation_recovery_requires_rollback_containment_proof`.
It does not change production recovery, physical proof, admission or idle policy.

## Evidence and diagnosis

Original synchronization repair started at `bb99d8aeab75689a92d6eb21289c2ce0e415373e`.
The descriptor-duplication follow-up starts at `37af3b4c0bf506aef870717102877e69ad69cef7`.
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

The regression duplicates only the lock files owned by `RecoveredPhysicalProof`
through a small `#[cfg(test)]` `duplicate_for_test()` method using `File::try_clone`.
The duplicate retains the same open-file descriptions after the original proof
is dropped. It is the deterministic ownership gate: production `prove()` cannot
reacquire the locks until the duplicate is dropped. No timing or polling is
introduced.

The previous raw-fork fixture also retained every unrelated descriptor in the
parallel test process, potentially extending another test's resource lifetime.
It has been removed entirely, including its unsafe code, child process, Unix
stream gate and waiter. The replacement duplicates exactly the proof descriptors
and does not create process-wide descriptor inheritance.

While the duplicated proof is held, the test verifies:

- Production `prove()` returns `None`.
- A reconciliation pass leaves the exact reservation pending and resuming,
  releases its in-flight claim, and keeps workspace authority fenced.
- The previous activation is settled; no unrelated idle commit is in progress.
- No proven rollback exists, and Goal idle remains unavailable.

The negative assertion directly proves the original invalid assumption: the
original proof has been dropped, but a nonblocking probe still cannot acquire
the exact authority. This does not claim an instrumented macOS reproduction of
the original hosted execution.

After dropping the duplicate, the test awaits the existing
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

The existing test-only `prove_after_release()` still waits for the allocation
namespace lock as well as the authority locks. Otherwise the same temporary
inheritance could invalidate the helper before authority inspection. Production
`Try` and `AfterSupervision` acquisition remain nonblocking where they were
nonblocking; all exact identity/receipt/continuation validation is unchanged.

During the previous synchronization repair, the first broad run also failed
`recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
with `Unavailable` versus `Inactive`. That nearby test used the same external
proof/drop/single-probe assumption after releasing its FIFO-held Git helper.
Its earlier bounded recovery owner had already completed while shutdown correctly
reported unresolved containment. The test now starts and joins a recovery owner
after native release, retaining all failed-shutdown, physical exclusion, ordinal,
idempotence and successful-shutdown assertions. This is a directly related test
synchronization repair; production shutdown and reconciliation remain unchanged.

## Validation

The descriptor-duplication follow-up is validated on Linux with Rust 1.98.1 and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. macOS is not available locally. CI and its
platform coverage are unchanged; hosted macOS must validate the pushed SHA.

| Command | Result |
| --- | --- |
| `cargo test --all-features runtime::subagent::registry::tests::agent411_resume_reservation_recovery_requires_rollback_containment_proof -- --exact --nocapture` | 1 passed; 3386 library tests filtered. |
| `cargo test --all-features runtime::subagent::registry::tests::recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --exact --nocapture` | 1 passed; 3386 library tests filtered. |
| `cargo test --lib --all-features physical_recovery::` | 6 passed; 3381 filtered. |
| `cargo test --lib --all-features subagent` | 337 passed; 3050 filtered. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo build --bins` | Passed. |
| `cargo test --lib --bins --all-features -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` | Exact hosted macOS unit command executed on Linux: 2636 passed, 2 existing ignores, 749 filtered. Binary targets passed with zero tests. |
| `cargo test --all-features --test subagent` | 45 passed; no ignored or filtered tests. |
| `git diff --check` and `git diff --cached --check` | Passed. |

Both focused commands also discovered other targets with zero matching tests.
The two existing ignores are `stage_profile_real_create_pipeline` and
`regenerate_committed_fixture_corpus`. No new skips, ignores, serialization,
retries, timeout changes or CI changes were introduced. All commands above passed
on their first execution for this follow-up; no run failed or was interrupted.
The descriptor itself supplies deterministic exclusion, not repeated execution.

The previous commit's validation remains historical evidence, not validation of
this follow-up. Client/protocol/browser suites were not rerun for this test-only
native fixture change. Hosted macOS results for the final SHA are not claimed and
will not be monitored after push.
