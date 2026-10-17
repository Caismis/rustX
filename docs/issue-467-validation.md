# Issue #467: bounded observation and transport isolation

This ledger records the RFC v3 implementation at the existing execution, durable,
publication, Runtime Client, connection, registration, and transport owners.
The pre-review PR HEAD `6f9cfbdb3ebd4b145ba5d54c044a91b0832fada3` had local validation passing, including the full Native, client and
317-case browser commands recorded below. The browser filesystem qualification
and unavailable local macOS checks remain explicit. Earlier epoch sections are
historical diagnostics; they do not override the pre-review results or imply
current repaired-source or remote CI success.
The normative contract is in [invariants](invariants.md), the public methods and
lock order in [App Server protocol](app-server-protocol.md), and source/read-cut
semantics in [conversation reading](conversation-reading.md).

## Transport fixture accounting follow-up (after `dab517f7`)

Final-head CI run [38983227430](https://github.com/Caismis/rustX/actions/runs/38983227430)
passed Linux boundaries (237 in-crate cases and all external targets, including
all 47 Subagent cases and the original skill-identity failure), TUI, protocol
regeneration, quality, macOS Node/Host, macOS native boundaries and Full Web.
macOS passed 3,268 in-crate tests (three existing ignores) plus every external
target, including the corrected lock fixture and all 47 Subagent tests. Full Web
passed 2,853 unit tests (four existing skips) and all 317 browser cases (32.7m).
Thus eight CI jobs passed; Linux contracts passed 3,953 tests but
failed `local_runtime::session_runtime_manager::tests::transports::t17_stdio_teardown_retires_unaccepted_record_without_write_readiness`:
its accepted-byte baseline was 563 and the later value 564. The active-Attempt
no-op and one-worker handoff regressions both passed on that remote head.

The transport fixture's inner duplex write wakes its peer before `Valve` returns
and increments its diagnostic byte counter. Reading both warmup JSONL records
therefore proves physical delimiter acceptance, but does not join the writer's
counter update. The test sampled `written()` before `grant(0)`, allowing precisely
one already-accepted delimiter byte to appear later as an alleged new reply byte.
The correction closes the valve first: `grant(0)` shares the mutex held across the
inner physical poll and counter update. Its return joins that update, closes
further acceptance, and only then permits a stable baseline. Production transport
behavior and every zero-new-byte assertion, RPC identity and deadline are unchanged.

`local_runtime::session_runtime_manager::tests::transports::valve_zero_budget_barrier_includes_physically_accepted_delimiter`
uses a real duplex peer and a scoped hold immediately after physical newline
acceptance but before the inner poll returns. It proves the peer reads the full
record while accounting is exactly one byte behind and the original poll owns
the valve lock. After release, closing the valve captures all seven warmup bytes;
the exact next record is Pending with zero accepted bytes. The physical writer
handle is joined, and the scoped release prevents a parked writer on unwinding.
The exact new regression and original T17 failure passed, followed by all 37
transport-module tests (80.33s). Formatting and strict all-targets/all-features
Clippy passed. Full native validation of this test-only repair is in progress.

## Archive stream integrity follow-up (after `3b0c0256`)

Independent review identified a remaining consumer omission: archive preflight
opened a native sealed reader, but ZIP streaming used its raw `Read` implementation
without verifying payload chunks. Header/length validation and ZIP CRC do not
prove that copied bytes match the native settled source. The previous lineage
copy repair remains intact; this is a separate archive consumer.

The correction verifies the existing 64 KiB buffer before writing the same bytes
into ZIP, then checks aggregate/EOF before successful producer completion. Native
capture/frontier ownership, cancellation, the two-chunk queue and backpressure
remain unchanged. No body prescan is added under the capture barrier, no import
API or format compatibility path is introduced, and archive format v4 remains
unchanged. The existing HTTP consumer emits its terminating chunk only after
successful native producer EOF; a verification error follows the existing failed
stream/partial-download cleanup contract.

The per-cut test hook runs at the actual source-buffer read/verification boundary,
without global scheduling state. These six real-stream regressions require an
error before successful EOF for corrupted inputs and join the original producer:

| Exact selector under `local_runtime::session::tests::archive_tests::integrity_tests::` | Controlled boundary |
| --- | --- |
| `archive_rejects_equal_length_tamper_after_settlement_before_preflight` | Settled bytes changed before the finite cut; header-only preflight succeeds, streamed verification rejects. |
| `archive_rejects_equal_length_tamper_after_preflight` | Captured reader/identity retained while same-length bytes change before streaming. |
| `archive_rejects_future_chunk_mutation_between_verified_buffers` | Hook after first verified buffer changes the next chunk; the next verification fails. |
| `archive_exports_the_verified_buffer_after_source_mutation` | Hook after physical read changes the source; ZIP contains the already-read original buffer, never a reread. |
| `archive_checks_completion_for_empty_and_extended_artifacts` | Empty healthy archive succeeds; post-cut extension and extension after the final verified chunk fail exact EOF/length checks. |
| `archive_rejects_rehashed_chunk_evidence_against_captured_aggregate` | Future payload and per-chunk digest change together; all three chunk checks pass, but captured aggregate rejects successful completion. |

Local archive validation passed `git diff --check`, `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo build --bins`,
the six exact integrity regressions, `cargo test --all-targets --all-features archive`
(39 passed / zero failed), and `cargo run --all-features --example check_test_lanes`.
The real Session export/Artifact document browser subset passed both tests
(17.4s), including browser download and remote TUI consuming the same archive.

CI run [38965057499](https://github.com/Caismis/rustX/actions/runs/38965057499)
belongs to `3b0c0256afad89a69738338e8e3b9a92489297bf` and cannot validate this
follow-up. Its Linux boundaries passed. Linux contracts reproduced the no-op
baseline publication guard failure (3,946 passed / one failed / four existing
ignores): the retained predicate was not rechecked for 29 seconds despite the
final healthy source. The subsequent gate investigation and correction are recorded below. macOS in-crate boundaries
failed the new shared/initialization lock fixture at `SourceDirectory::create`
with `ENOTDIR` (3,260 passed / one failed / three existing ignores); its temporary
path was not canonicalized before entering the owner that refuses symlinked
ancestors. The exact path was not logged. The fixture now canonicalizes its temporary root,
following existing configuration tests; production nofollow behavior is unchanged.
The external competing-writer CAS test
was not reached, so this run does not establish its remote outcome.

The no-op diagnostic now retains its work future until the failure snapshot is
captured, so timeout cleanup cannot first release the held execution gate. Fixed-size
`#[cfg(test)]` owner snapshots retain only the latest worker, physical read,
preparation/publication and Journal readiness stages. Failure reporting uses
nonblocking lock attempts. The production notification path, source work, the
30-second guard and correctness assertions are unchanged; this diagnostic change
does not itself repair the Linux failure; the separate gate handoff below addresses the demonstrated scheduling boundary.

The `3b0c0256` Web job completed with 2,853 unit tests passing (four existing
skips), and 316 browser passes / one failure (34.3m). Both original reading cases
passed. The failure was `model-errors.spec.ts` / `persistent native model failures
en dark`, at its conditional Collapse Sidebar click. The retained trace proves
that successful native navigation dismissed the narrow overlay between the
visibility check and click; the failure page already contained both expected
errors and recovered output. The fixture now asserts the positive pre-navigation
overlay and the navigation-owned closure/interactivity/Expand control, preserving
all original content assertions and all four locale/theme cases.

The diagnostic validation pass completed formatting, strict Clippy, ten selected
interrupted-operation tests and all 26 `cfg3_catalog` tests. The four-worker
`model_selection_noop::` group then failed four existing 30-second setup guards
(two passed); the active-Attempt case had not completed fixture construction.
A contemporaneous `/proc/pressure/io` sample reported 83.54% full pressure over ten
seconds. This is distinct from the remote 29-second post-admission stall and does
not waive either failure. The driver stopped before the full contracts command.
Initial diagnostic compilation also found four lint issues, corrected before the
passing strict-Clippy run; the failed log is retained.

The subsequent owner audit found a concrete harness scheduling defect. The installed
`attempt_execution_gate` synchronously parks the Tokio admission worker immediately
after native readiness can wake Projection. Tokio 1.53.1's worker-local LIFO slot
is not stealable, so this test gate can strand that ready task until timeout cleanup
releases the gate. No Store, coordinator, mailbox, dispatcher or probe lock remains
held across this gate. The test-only correction uses `block_in_place` for this
installed gate alone, preserving its exact post-admission/pre-execution location.
Absent hooks and production scheduling are unchanged.

`runtime::conversation_runtime::tests::attempt_execution_probe_hands_off_woken_peer_before_parking`
uses one executor worker, a Notify waiter enabled before admission and an external
channel controller. Its synchronous negative control proves that the peer cannot
run while the gate is held; the actual helper must let the peer run before release.
Both physical tasks are joined before checking the handoff result. This directly
proves the executor handoff; the pinned Tokio source establishes the LIFO mechanism
that matches the four-worker CI stall. It is not a captured reproduction of that
entire CI fixture. The existing
native no-op fixture still requires an Admitted Attempt and stable native
projection before measuring the no-op. The exact handoff regression passed (one test), followed by all six native no-op
tests (3.16s) with one libtest worker and unchanged guards. Formatting, strict
Clippy and binary build passed on this source. The complete follow-up validation
results below are from this source; prior runs are not used as proof of the handoff.


The first complete Rust run of this follow-up ended with 4,190 passed, one failed
and four existing ignores. `scripted_suites::agent::tool_progress::parallel_calls_bound_progress_per_call_and_keep_canonical_order`
exceeded its unchanged ten-second guard (11.315s observed; fixture setup 17.790ms).
Both Tools, all retained progress, and canonical Tool results had committed;
the retained Journal ended at the next `TurnStarted`, before the second Model
request. This direct Agent fixture has no Projection worker or admission gate.
Synchronous durable commits followed by the existing model-preparation yield are
consistent with cumulative work exhausting the guard; the snapshot does not
prove a post-`TurnStarted` deadlock. Host I/O full pressure was 86.90% at the later
sample, which is environmental evidence, not proof of the cause of every delay.
The exact test passed unchanged in 1.65s; its complete four-test module passed in
14.26s. The initial failed full run remains recorded, and a full repeat is required.

The protocol generation/drift check and typecheck, development client checks, TUI
checks, WebUI typecheck/unit tests/build/i18n/provenance, original skill-identity
boundary test, and fake-provider tests passed. All four model-error browser
locale/theme cases passed (29.5s). Both original reading cases passed separately
(2.8m and 2.5m), retaining all 300 Attempts and the 240-second fixture guard.
The complete canonical browser suite then passed all 317 tests (21.4m), including
both reading cases and all four model-error cases. The unchanged repeated full Rust
run passed 4,872 tests across 20 targets, zero failures and nine existing ignores
(2,457.19s); the library contributed 4,191 passes and four ignores. It includes all
47 Subagent integration tests and the originally failing skill-identity test, plus
real process/Stdio/WebSocket/Unix IPC, storage, publication and shutdown coverage.
The previously timed-out parallel Tool-progress test passed in that complete run.
No guards, workloads, assertions or test discovery were weakened.

Final follow-up command ledger (single exclusive local lane;
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1`):

| Command | Final source result |
| --- | --- |
| `git diff --check` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo build --bins` | Passed. |
| `cargo test --lib --all-features runtime::conversation_runtime::tests::attempt_execution_probe_hands_off_woken_peer_before_parking -- --exact --nocapture` | One passed; negative control and actual handoff both exercised. |
| `cargo test --lib --all-features model_selection_noop:: -- --nocapture` | Six passed; also passed in both full library runs. |
| `cargo test --all-targets --all-features` | Final repeat: 4,872 passed / zero failed / nine existing ignores. Initial failed run retained above. |
| `cargo run --all-features --example check_test_lanes` | Passed. |
| `pnpm --dir protocol/app-server check` | Generation/drift passed. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir dev typecheck` / `pnpm --dir dev test` | Passed; 41 tests. |
| `pnpm --dir tui typecheck` / `pnpm --dir tui test` | Passed; 1,180 passed / one existing platform skip. |
| `pnpm --dir web-console typecheck` / `pnpm --dir web-console test` | Passed; 2,853 passed / four existing skips. |
| `pnpm --dir web-console build` | Passed. |
| `pnpm --dir web-console check:i18n` / `pnpm --dir web-console check:provenance` | Passed. No separate maintained WebUI lint script exists. |
| `cargo test --test subagent --all-features definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact` | One passed, also passed within all 47 Subagent tests. |
| `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests` | 55 passed. No live Model API used. |
| `bash web-console/scripts/browser-tests.sh test/e2e/model-errors.spec.ts` | Four passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'native distant reading rail, detached/latest follow and measured width in both locales'` | One passed, 2.8m. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'Fork source exact Message landing and cancellation by scrolling, newer navigation and attachment replacement'` | One passed, 2.5m. |
| `pnpm --dir web-console test:e2e` | 317 passed / zero failures / no skips, 21.4m. |

Logs and exact driver results are retained in
`/home/capi/Documents/codes/rustX-review2-20261016/final-repair-validation/`, with
successful continuation in `completion/` and targeted timeout reruns in
`tool-progress-rerun/`. Final fetch still reports main
`8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already integrated by normal merge
`b8a961eb`; no additional main merge was required. macOS cannot run on this Linux
host. Final-head remote checks must be verified separately on [PR #478](https://github.com/Caismis/rustX/pull/478/checks);
the prior `3b0c0256` failures are not claimed as passing validation.

## Artifact lineage integrity follow-up (after `fee5d8b7`)

Independent review found a reachable source-authentication gap: `ContentRead`
rejected an equal-length external artifact overwrite against its frozen digest,
but Fork/Clone/tree preparation used a length-only physical copy. The destination
could then index the changed bytes as inherited Original content. Destination
rehashing is not source authentication. A regression against `fee5d8b7` reproduced the gap: source ContentRead rejected
the overwrite while Clone preparation still succeeded.

The correction belongs to native Artifact settlement, independently of optional
Runtime Client display indexing. An explicit fallible `ArtifactWriter::finish()`
publishes the native integrity seal under the existing exclusive reservation.
Copying verifies source chunks and writes those same buffers, finishes the
new artifact, then permits destination initialization/catalog publication.
Canonical binary indexing must also verify native settlement evidence before
capturing a source reference. Optional-index rollback preserves the native seal
and canonical commit; it must not prevent healthy lineage. SQLite 69 refuses
older development stores rather than adding a format fallback. Existing bounded
ContentRead and aggregate lineage-copy budgets remain unchanged.

The exact `fee5d8b7061a8843bb3aa706d733e906339de0de` CI run
[38944322845](https://github.com/Caismis/rustX/actions/runs/38944322845) passed
Linux boundaries, formatting/Clippy, generated protocol, TUI and macOS Node Host.
Linux contracts failed one test: `local_runtime::session_runtime_manager::tests::model_selection_noop::identical_explicit_selection_is_inert_during_an_attempt`
at the existing 30-second guard, stage `no-op baseline publication`. Its retained
diagnostic reported cursor 4, matching native/published source cuts, empty reliable
and progress queues, and no source/layout/Journal failure. Library results were
3,928 passed / one failed / four existing ignores (564.78s). The previously failing
T15 effective-configuration test passed. The no-op barrier failure is under
investigation; no timeout increase or passing rerun is treated as its resolution.
Full Web also passed 2,853 unit tests (four existing skips) and all 317 browser
cases (21.4m). macOS passed 3,243 in-crate tests (three existing ignores), then
failed the external `cfg3_catalog::competing_native_writers_have_exactly_one_commit_winner`:
results were `[Err(Io), Ok(())]` where one success and one revision conflict were
required; a fresh read succeeded. The job's unit `Io` error does not retain the
underlying operation/errno, so its cause is still under investigation. Overall,
seven jobs passed and two failed. These results belong to `fee5d8b7`, not the
in-progress seal correction.

The first new lineage reproduction ran against `fee5d8b7`: the healthy copy test
passed for Clone/Fork/tree and the tamper test failed at Clone because preparation
succeeded after source ContentRead had rejected changed bytes. After the native
seal correction, the initial lineage group had six passes and two new fixture
errors (invalid page size and an incorrect deletion-root expectation). Both
fixtures now follow the actual bounded-page/catalog deletion contracts. The
current rebuilt source passes strict Clippy, all 17 Artifact owner tests, eight
lineage tests, eight binary-content tests, the Compaction artifact boundary,
the worker native-cut regression, six no-op tests, ten interrupted-operation
tests, and all 26 external configuration catalog tests. The complete four-worker
Rust run completed with 4,177 passes, seven failures and four existing ignores
(733.72s). Six failures were the boundary liveness guards; the seventh was
`large_write_progress_crosses_idle_timeout_without_retry_or_duplicate_execution`
at its existing two-second guard. Final full validation remains pending. Kernel samples captured physical test processes in
`jbd2_log_wait_commit` and `folio_wait_bit_common` during high I/O pressure. This
is evidence of contention, not proof of each failure's cause or a test waiver.
The revocation fixture timed out while creating its second native Session, before
App Server/read admission: configuration 2.865s, launch 4.680s, first Session
13.449s, second Session still running after 9.299s. Three other fixtures reached
native settlement waits, before display notification consumption; their
constructors had already consumed 16.279–22.029s of the cumulative 30-second
guard. These localize observed delays without attributing every stalled call to
a particular filesystem operation. All assertions and guard durations remain
unchanged for the exact and full serial reruns.

### Current follow-up validation

With `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4`,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` and `RUST_TEST_THREADS=1`,
`cargo test --all-targets --all-features` passed all 20 targets:
**4,865 passed / zero failed / nine existing ignores** (2,456.27s).
The library contributed 4,184 passes and four ignores; the complete Subagent
suite includes the originally failing skill-version identity case. All seven
four-worker failures also passed exact-selector reruns unchanged before this
complete run. The previous full four-worker failures remain recorded above;
serial success does not establish their full cause.

Current-source protocol generation/drift, protocol/Dev/TUI/Web type checks,
Dev tests (41), TUI tests (1,180; one existing skip), Web tests (2,853; four existing
skips), production build, i18n, provenance and fake-provider tests (55) passed.
The original skill-version identity test also passed individually. Both original
reading scenarios passed independently with 300 Attempts and the 240-second
fixture limit unchanged: rail/locales reported 2.4m and Fork navigation 2.5m.
The first complete browser run finished with 316 passes and one failure (21.7m).
Both original reading scenarios passed again. The failure was
`artifact-document.spec.ts`: the fixture-only Rust example still dropped its
writer after flushing, so the new native reader correctly rejected an unfinished
Artifact. `examples/document_artifact_fixture.rs` now explicitly calls `finish()`;
all repository `open_writer` callers were audited, retaining intentional abandoned
writer/lifetime tests. No product logic, browser assertion or timeout changed.
The exact artifact browser case passed after fresh formatting/strict Clippy
checks. The four-worker in-crate boundary lane also passed all 237 cases
(101.82s). The complete browser rerun passed **317 / zero failures** (21.5m), including
both original reading scenarios and the corrected Artifact fixture. No assertions,
workloads or fixture deadlines changed. Final pushed-head CI remains pending at
this pre-push capture; the PR checks must be read against their exact head SHA.
A final fetch retained main `8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already
integrated by the existing normal merge.

### Follow-up command results

| Command | Result |
| --- | --- |
| `git diff --check` | Passed. |
| `cargo fmt --all -- --check` | Passed, including the final fixture correction. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed, including the final fixture correction. |
| `cargo build --bins` | Passed before full native validation. |
| `cargo test --all-targets --all-features` | 4,865 passed, nine existing ignores; serial worker qualification above. The later change only finishes the browser-only example's artifact and is covered by fresh Clippy and exact/full browser runs. |
| `cargo run --all-features --example check_test_lanes` | Passed. |
| `pnpm --dir protocol/app-server check` | Generated schema/TypeScript/fixture drift check passed. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 41 tests. |
| `pnpm --dir tui typecheck`; `pnpm --dir tui test` | Passed; 1,180 tests, one existing skip. |
| `pnpm --dir web-console typecheck`; `pnpm --dir web-console test` | Passed; 2,853 tests, four existing skips. |
| `pnpm --dir web-console build` | Passed. |
| `pnpm --dir web-console check:i18n`; `pnpm --dir web-console check:provenance` | Passed. |
| `cargo test --test subagent --all-features definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact` | Passed individually and in all 47 Subagent cases. |
| `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests` | 55 passed. |
| `cargo test --lib --all-features boundary_suites::` with four test workers | 237 passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'native distant reading rail, detached/latest follow and measured width in both locales'` | Passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'Fork source exact Message landing and cancellation by scrolling, newer navigation and attachment replacement'` | Passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/artifact-document.spec.ts` | Passed after explicit fixture settlement. |
| `pnpm --dir web-console test:e2e` | Final run: 317 passed, zero failed, 21.5m. Initial 316/1 result retained above. |

Only fake/scripted/emulated providers were used. macOS native checks require CI;
local Linux results do not substitute for them. The existing pinned browser-image
and filesystem qualification applies to this run as well.

### Follow-up ownership and deterministic coverage

The source seal is stored in the existing native `.reserved` owner file: a fixed
64-byte header and 32-byte digest per 64 KiB chunk. Explicit finish syncs bytes and
digest records, publishes the header last, and retains exclusive ownership until
completion. The fixed header/count/file-length checks allocate no untrusted-size
buffer. A nonempty reservation cannot be reused even after external deletion of
its byte file. Copy verifies source buffers, and destination finalization must
match the captured source aggregate before publishing its own seal. This closes
both source-before-copy and destination-before-finish reauthentication gaps.
Native bounded image/model reads and Compaction resource reads verify the same
returned slices under their existing byte/admission budgets. Inspection archives
remain inspection-only; this does not introduce an import or restore format.

| Exact regression selector / group | Controlled boundary |
| --- | --- |
| `tools::artifacts::integrity_tests::abandoned_writer_and_failed_finish_never_produce_a_successful_settlement` | Active/abandoned writer and injected pre-header failure; no successful native settlement. |
| `native_artifact_copy_rejects_equal_length_tamper_and_keeps_failed_target_unsealed` | Real equal-length source overwrite; target stays unfinished. |
| `native_artifact_copy_rejects_later_chunk_changes_after_first_verified_buffer` | Hook after one verified chunk changes a later chunk; copy fails. |
| `native_artifact_copy_uses_same_verified_buffer_despite_after_read_or_verified_changes` | Changes after source read/verification cannot replace the copied buffer. |
| `native_artifact_copy_cannot_reseal_changed_destination_as_an_inherited_original` | Last-source-chunk hook changes an earlier destination chunk before finish; destination seal publication fails. |
| `native_artifact_seal_missing_malformed_and_partial_verification_fail_closed` | Missing/truncated seal, invalid lengths/counts, digest mismatch and incomplete verification. |
| `native_bounded_artifact_reads_require_finish_and_reject_same_length_changes` | Bounded native reader refuses unfinished/modified bytes across cold reopen. |
| `settled_artifact_identity_cannot_reseal_after_external_bytefile_removal` | Removing a settled byte file does not permit reopening its existing identity for replacement bytes. |
| `local_runtime::session::tests::artifact_lineage_tests::integrity_tests` | Actual Clone/Fork/tree preparation and catalog publication; tamper rejection and allocation cleanup, optional-display-index rollback, source artifact removal and ordinary source Session deletion with cold destination reads. |
| `durable::sqlite::content::binary_tests::canonical_binary_index_cannot_reauthenticate_a_modified_native_artifact` | Mutation after native finish but before canonical indexing cannot publish a replacement reference. |
| `runtime_client::projection::tests::worker_read_candidate_keeps_native_cut_without_capturing_checkpoint` | Actual SQLite source read and installation retain the exact native cut without preparing the lazy published Trace head; explicit snapshot capture keeps its own behavior. |
| `interrupted_native_writer_locks_preserve_exactly_one_cas_winner` | Two native settings writers, barrier and syscall-boundary EINTR injection: one commit, one exact revision conflict. |
| `noninterrupted_native_writer_lock_failure_does_not_retry_or_publish` | Injected EACCES is returned once and leaves authored bytes unchanged. |
| `interrupted_shared_and_initialization_locks_continue_but_other_errors_survive` | Shared/initialization lock continuation and non-interrupted error preservation. |

The settings correction continues only an Interrupted wait on the same descriptor;
it does not retry the CAS transaction or replay a mutation. [Rust 1.98.1's Unix lock implementation](https://github.com/rust-lang/rust/blob/1.98.1/library/std/src/sys/fs/unix.rs#L1468-L1516)
uses `cvt(flock)` without interrupted-call retry, and [Apple's native lock contract](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_lockf.c#L186-L195)
lists EINTR. These establish the supported-platform gap, but the
previous CI log does not prove that EINTR caused its generic Io. Failure-only
operation/ErrorKind/errno tracing now retains that distinction without source paths
or document contents.

The no-op test passed in isolation (six tests) and under its existing containing
four-worker group. The latter run is not a valid suite pass: it used a schema-69
libtest executable with stale schema-68 child binaries, causing eight explicit
version-mismatch failures (282 passed / eight failed / one existing ignore).
The combined validation rebuilds all binaries first. Worker scheduling now captures
only its native ReadDomainCut in both production and libtest. Previously cfg-only
snapshot fields forced checkpoint capture/fit under the worker locks; that extra
work/error gate is removed and tested without a retry, extra wake, source repair,
or changed deadline. This improves production/test equivalence; the earlier CI
no-op timeout's exact cause remains unproven. Its last failed predicate is now
captured before timeout cancellation releases the native execution gate, with
bounded elapsed/cut/queue diagnostics rather than only post-cancellation state.

## Second-stage review corrections (after `915f6a57`)

The second-stage review was checked against actual source, rather than assumed
correct. `origin/main` remains `8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already
an ancestor. This correction retains the existing PR/worktree and the approved
45-minute Linux contracts job budget; no individual guard or workload is raised.

- **Cancellation gestures:** Web's fresh gesture really was blocked by both
  `following` and a disabled/hidden composer. TUI's direct cancellation method
  already bypassed `#call`/`#assertControlReady`, contrary to that part of the
  review, but its stale idle/HITL gesture routing still used presentation to
  decide whether to cancel. Current connection and exact attachment authority
  now permit cancellation during unavailable/recovering observation, including
  absent or settled displayed Attempts. The native coordinator remains the only
  current-Attempt selector. Ordinary Send/edit readiness remains strict; unknown
  execution is not relabeled Running. Connection/attachment replacement fences
  retained gestures, and recovery performs no semantic replay.
- **Binary integrity:** the new real-artifact regression first failed against
  the length-only reader after a same-length external overwrite and Store reopen.
  Schema 68 freezes a SHA-256 digest for each 64 KiB canonical binary segment
  during the original optional indexing transaction. Each read verifies at most
  two segments and returns slices from those same buffers: at most 128 KiB actual
  file input/hash work, two 32-byte digest rows and a 64 KiB response. Missing or
  modified evidence fails closed. Native writer settlement, source authorization,
  canonical commitment and optional-index rollback retain their original owners.
  Empty EOF reads do no file/hash work. Older schema 67 stores are refused without
  migration. This is the canonical ContentRef read contract; it does not claim
  that advisory artifact locks prevent external writers or authenticate every
  independent archive/export API.
- **Backpressure:** immediate closure on one full `try_send` was documented and
  was not itself established as an RFC prohibition. The refined finite policy
  reserves capacity before native notification consumption or reply encoding.
  Original admitted request futures keep their response ownership while awaiting
  the existing 32-slot queue; at most one immediate protocol reply backpressures
  further input. Writer, shutdown and admitted controls remain polled. A reply
  has one absolute ten-second capacity deadline, distinct from the unchanged
  physical-write deadline. There is no extra encoded queue, silent drop, or
  semantic retry. Accepted Stdio bytes retain their exact remainder/delimiter.

### Previous-head CI failure retained

[Run 38921167776](https://github.com/Caismis/rustX/actions/runs/38921167776), for
`915f6a57`, passed Linux boundaries (237 in-crate and 451 external tests) but
failed Linux contracts: 3,916 passed, one failed, four existing ignores and
237 boundary filters. The sole failure was
`local_runtime::session_runtime_manager::tests::protocol::t15_effective_configuration_preserves_all_fields_and_advancing_pages`
at its unchanged 30-second liveness guard. Binary build finished at 15:10:54Z;
test-harness compilation ended at 15:17:21.852Z, and library execution ran from
15:17:22.645Z through 15:34:11.892Z (1,009.25s). This was an actual test failure,
not exhaustion of the 45-minute job budget and not a proven infrastructure fault.

The exact selector passed in isolation before correction in 9.39s (9.32s CPU).
Configuration revision hashing invoked SHA-256 separately for tiny JSON escape
writes on every page/read. Fixed 8 KiB standard buffered writers now feed hashing
and range accounting with the same complete serialized stream, without caching
or omitting evidence. Regressions compare digests and ranged bytes against the
original JSON/structural encoding across buffer boundaries, large escaped text,
nested values, changed revisions and EOF. The exact selector measured 8.52s after
hash buffering and 8.11s after range buffering (8.04s CPU), versus 9.39s before.
This is a modest isolated improvement, not proof of the remote failure's cause.
Debugger breakpoints at actual phase boundaries measured about 1.96s field
reconstruction and 5.41s collection paging. A separate page-boundary measurement
at 95 calls attributed 5.266s to full revision validation and 0.194s to page item
construction. Revision validation is preserved; no broader configuration caching
or performance refactor is claimed. These are wall durations between debugger
breakpoints, not a CPU sampling profile. Failure-only phase diagnostics
retain fixture/load/projection/reconstruction/page/cleanup stages. The original
3,000-entry/large-field reconstruction and pagination assertions remain intact.
`perf record` was unavailable locally (`perf_event_paranoid=4`); no CPU sampling
claim is made from that rejected command.

### Added synchronization and integrity evidence

| Selector / owner | Controlled boundary and assertion |
| --- | --- |
| `web-console/test/composer-cancellation.test.tsx` unavailable/stale/held-recovery cases | Real composer gestures over a controlled connection, held reply and explicit checkpoint repair. Exact attachment/generation gates dispatch; failed observation neither clears an uncertain operation nor invents the native cancellation target. Send and editing remain unavailable. |
| `tui/test/composer-app.test.ts` unavailable Escape/Ctrl+C cases | Actual key routing with retained idle, absent or pending-interaction display state. The exact live session dispatches cancellation; retired session authority cannot cross to its replacement. |
| `durable::sqlite::content::binary_tests::binary_content_reference_rejects_same_length_external_modification_after_reopen` | Commit, close, overwrite the real artifact with equal-length bytes, reopen and read the original reference; the read fails closed against the original digest. |
| `durable::sqlite::content::binary_tests::binary_integrity_checks_open_read_races_and_returns_only_verified_buffers` | Narrow hooks modify the actual file before chunk read and after verification. Pre-read corruption fails; post-verification mutation cannot change bytes already verified and returned. |
| `durable::sqlite::content::binary_tests::binary_ranges_verify_only_intersecting_fixed_chunks_with_exact_eof` | Exact unaligned chunk reconstruction, physical read/hash counters and EOF prove the two-segment bound independent of artifact size. |
| `durable::sqlite::content::binary_tests::binary_digest_index_failure_is_optional_and_rolls_back_its_frontier` | Injected index failure retains canonical commitment while rolling back binary digest/frontier publication. Missing evidence cannot fall back to mutable bytes. |
| `app_server::transport::tests::transient_outbound_saturation_waits_for_capacity_without_encoding_another_record` | Fill exactly 32 slots, poll the reply pending, release one slot and recover the original correlated response. |
| `app_server::transport::tests::sustained_outbound_saturation_keeps_one_absolute_wait_deadline` | Paused clock and explicit polling prove that repeated polls do not reset the ten-second capacity deadline. |
| `local_runtime::session_runtime_manager::tests::transports::outbound_backpressure::transient_full_queue_preserves_admitted_control_and_other_conversation_execution` | Exact queue-full watch and held writer on one carrier with two native attachments. The other conversation completes and the first accepts native cancellation before output resumes; both original receipts then arrive exactly once, with two model calls total. This is the shared serving boundary with a controlled writer, not an OS pipe. |
| `local_runtime::session_runtime_manager::tests::transports::t17_t20_stdio_saturation_deadline_finishes_the_physically_accepted_record` | Actual OS pipe, gated first positive write, full queue and post-retirement watch. Capacity deadline retires unaccepted work; the accepted record still finishes its exact bytes and delimiter. |
| `local_runtime::configuration::observation::application_tests::configuration_revision_preserves_exact_json_bytes_across_buffer_boundaries` and `configuration_ranges_preserve_exact_structural_bytes_across_buffer_boundaries` | Exact digest/byte equivalence at 8 KiB buffer edges, escaped text and EOF, preserving complete revision evidence and bounded retained output. |

### Second-stage validation

The first complete local four-worker attempt reached the library result with
4,159 passed, seven failed and four existing ignores (809.69s); Cargo then
stopped before external targets. Five file-read boundaries and one Child model
selection boundary reached their unchanged 30-second cumulative guard. Three
failed during fixture construction itself, before the tested read/control
operation; retained phase timings include roughly 9–10s launch analysis and
15–16s native Session creation. The seventh was
`scripted_suites::agent::tool_deadlines::issue206_executor_panic_cannot_orphan_parallel_siblings`:
its expected scripted panics occurred, but settlement exceeded its existing
five-second guard with one model request and the original Journal at
`turn_started`. This is a failed run, not waived correctness coverage.

Contemporaneous `/proc` samples showed I/O pressure around 75–80%, no CPU
pressure, and native test threads in `jbd2_log_wait_commit` and
`folio_wait_bit_common` on ext4. These directly observed kernel waits help
localize resource contention but do not by themselves prove the cause of every
timeout. No filesystem durability setting, test assertion, individual guard or
workload was changed. The five file-read tests subsequently passed together
(6.66s), the model-selection boundary passed (6.87s), the exact Tool-panic test
passed, and its entire 19-test Tool-deadline suite passed (5.24s), using one
libtest worker. The complete serial `cargo test --all-targets --all-features` run then passed
4,847 tests with zero failures and nine existing ignores across 20 targets
(2,649.54s), including all seven selectors. The separate four-worker in-crate boundary lane then passed all 237 tests
(105.47s), and the 19-test Tool-deadline suite passed with four workers too. The
initial complete-run failures remain part of the evidence.

The full TUI run initially exposed an incomplete test fixture: the healthy
`app.test.ts` Session fake still provided only `controlReady`, omitting the new
live cancellation readiness and observation phase. Its overlay-free Escape
assertion failed and the old test skipped UI cleanup on assertion failure. The
fake now models the actual healthy/following authority and the test joins its UI
in `finally`; production has no fallback for missing authority. The exact test
passed, the affected routing group passed 135 tests, and full TUI passed 1,180
with zero failures/cancellations and one existing macOS-only skip. Type checking
passed. The stopped failed process tree was physically reaped before continuing.

Full Web validation initially reported 13 failures across four files: the
shared composer default inferred cancellation availability without a native
session, and the generalized focusable/read-only exception altered Child
Stopping/Admitting, unsupported-restoration and pending-model draft behavior.
The correction gives only the resident Root's unavailable-observation path an
explicit cancellation-only mode with exact attachment proof. Other disabled,
editing and Send rules retain their previous behavior. All 13 existing assertions
pass unchanged; two additional tests reject a Stop action without native
cancellation authority. Full Web then passed 2,853 tests with zero failures and
four existing skips (180 passing files, one skipped), with TypeScript passing.
No protocol DTO, mutation replay or execution-state fallback was added.

Focused client checks passed: 117 Web tests across five files, 47 TUI tests across
two files, and both TypeScript checks. These are actual component/app gesture
checks with controlled client responses; native coordinator identity and physical
transport boundaries retain their separate Rust tests. Seven binary owner tests,
the existing binary restart/lineage boundary, the exact-digest test, two queue
capacity unit tests and the shared-connection transient-saturation test passed.
The first real-pipe deadline run caught a genuine ordering regression: a ready
notification overtook the deferred immediate parse-error reply. Immediate reply
polling was moved ahead of notification selection to preserve the original order;
its accepted-prefix/delimiter assertions were not weakened. The corrected pipe
case and all 36 transport tests passed. Integrated Rust and full client checks
have now passed as recorded above. The first build correctly refused stale
reviewed local hashes for the two changed Harness-derived sources; their hashes
and treatment notes were explicitly updated without changing upstream pins,
imports or attribution. The rebuilt artifact, i18n and provenance checks passed.
The two original reading cases passed independently on the default filesystem
(2.6m and 2.5m), retaining 300 Attempts and the 240-second setup limit. The complete browser suite then passed all 317 cases in 21.2 minutes with
one worker, including both original reading scenarios. The next pushed-head CI
remains pending; earlier-head results do not establish that outcome.

| Exact command | Current local result |
| --- | --- |
| `git diff --check` | Passed. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed after correcting new lint findings; initial output retained. |
| `cargo build --bins` | Passed. |
| `cargo test --all-targets --all-features` | 4,847 passed / 0 failed / 9 existing ignores / 20 targets with one libtest worker; initial four-worker failure is detailed above. |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=4` | 237 passed / 0 failed. |
| `cargo test --lib --all-features scripted_suites::agent::tool_deadlines:: -- --test-threads=4` | 19 passed / 0 failed. |
| `cargo run --all-features --example check_test_lanes` | Linux discovery and coverage passed. |
| `pnpm --dir protocol/app-server check` and `pnpm --dir protocol/app-server typecheck` | Generation/drift and TypeScript passed. |
| `pnpm --dir dev typecheck` and `pnpm --dir dev test` | Passed; 41 tests. |
| `pnpm --dir tui typecheck` and `pnpm --dir tui test` | Passed; 1,180 passed / 1 existing macOS-only skip. |
| `pnpm --dir web-console typecheck` and `pnpm --dir web-console test` | Passed; 2,853 passed / 4 existing skips. |
| `pnpm --dir web-console build` | Passed, including artifact provenance. |
| `pnpm --dir web-console check:i18n` and `pnpm --dir web-console check:provenance` | Passed. |
| `cargo test --test subagent --all-features definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact` | Passed alone and in the complete 47-test Subagent target. |
| `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests` | 55 passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'native distant reading rail, detached/latest follow and measured width in both locales'` | Passed. |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'Fork source exact Message landing and cancellation by scrolling, newer navigation and attachment replacement'` | Passed. |
| `pnpm --dir web-console test:e2e` | 317 passed / 0 failed (21.2m), original workload and limits unchanged. |

Native validation used `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`; the final complete run used
`RUST_TEST_THREADS=1`, with the explicit additional four-worker commands above.
Only local fake/scripted providers ran; opt-in live tests remained ignored.


## PR #478 repair and latest-main integration

Historical validation at `b0928347`: the repaired source passed **4,784 Rust tests** (zero failures,
ten existing ignores) with `RUST_TEST_THREADS=1`, including the new publication
contention tests and corrected real-process readiness handshakes. Earlier merged production/client validation passed **317 browser cases** under
the filesystem qualifications recorded in the historical sections. That is not
a passing Web result for `b0928347`. Strict Clippy, binary build and test-lane
discovery passed for that historical source.
That head’s Linux contract and boundary lanes passed. Its Web lane
passed 315 browser cases but exceeded the unchanged preparation budget in both
300-Attempt reading fixtures; bounded failure diagnostics now retain their exact
phase and original native process counters. Local exact preparation runs completed
in 208.378s and 223.145s, which do not override the CI failure. The merge macOS
lane exhausted its existing 70-minute compilation budget; the follow-up serializes
compiler jobs and completed native compilation. Its library run found three
fixture-root failures (3,178 passed): raw macOS TempDir ancestors hit the intended
no-symlink reader admission before file-identity checks. The fixtures now use
their exact canonical roots; production admission and assertions are unchanged.
The external macOS targets were not reached and a new platform run is required.
[Repair history](issue-467-repair-history.md) records every intervening failure
and environment qualification. PR #478 remains open for independent review and
must not be merged by this task.

### Pre-correction published head and local production comparison (2026-10-16)

The pre-correction PR head was `ee1635479b4ecafe5f7c7b114cfccf8e8b501585`, with
`origin/main` at `8cb41624eb2dc2ca87e4409961f42553ceb029cf` already integrated.
Both remote refs were checked again after the comparison. PR #478 remains open,
non-draft, and unmerged. The subsequent user instruction authorizes a normal
commit and push after the scoped Source/CI corrections and their local validation;
new-head CI must then be checked separately. No force-push or merge is authorized.

[CI run 38883956631](https://github.com/Caismis/rustX/actions/runs/38883956631)
completed with eight successful jobs, including all 317 Full Web conformance
cases and Linux/macOS boundaries. Linux deterministic contracts were cancelled
at the existing 25-minute whole-job budget. This is **not green CI**. Preparation
and compilation consumed about 11 minutes; test execution then ran about 14
minutes without an assertion failure before cancellation. The exact indexed
occupancy fixture remained unfinished for at least 12 minutes 48 seconds.
Its unchanged 2,048-event workload was measured independently; no individual
test budget, workload, storage exclusion, or query assertion has been weakened.
The subsequent explicitly approved CI correction raises only the Linux
`rust-contracts` whole-job limit from 25 to 45 minutes. The observed job began
09:57:09, finished preparation at 09:58:28 and binary build at 10:02:05, began
library execution at 10:08:14, and was cancelled at 10:22:24. Thus preparation
and native builds consumed about 11 minutes before the test phase; the remaining
contract/provider targets and lane check were never reached. The new finite
job budget permits complete evidence, while every 30s test guard, 240s browser
fixture budget, assertion, selection and other lane's job limit stays unchanged.
A single-test failure still requires investigation; the job budget is not a waiver.

The exact failing-lane fixture is
`durable::sqlite::tests::indexed_fact_tests::indexed_occupancy_and_message_lookup_cost_does_not_grow_with_irrelevant_history`.
The cold fixture passed locally in 339.07s, but spent 323.709s in RequestStart
commits, including 302.498s in optional Request Source construction. The
actual indexed queries took 672 microseconds. Each additional request
cold-froze the same nine-message selection with fresh node identities, causing
18,440 new Source leaves and 2,636,761,896 cumulative Source index bytes.
Those are constructed metadata counters, not physical-write or retained-size
measurements. The expensive consequence was inside Source capture, not merely
the `to_vec` call's own elapsed time.

The local correction is confined to `indexed_fact_tests.rs`: assemble input
through the existing native selected-root API, preserve the exact history cut,
seal associations for this request, and call `freeze_selected_input`. The
shared cold helper remains byte-identical to HEAD, preserving dedicated cold
reconstruction and rollback coverage. There is no product/runtime behavior or
protocol change, and no Source validation is bypassed.

The paired exact fixture passed in 33.62s with the same 2,057 actual request
starts/outcomes, including the original 2,048 added requests, and the same
2,048 added canonical Assistants. Native RequestStart commits took 18.615s;
Source construction 9.511s (leaf capture 0.372s versus 293.774s). It created
nine Source leaves and 1,286,928 Source index bytes. Both indexed lookups
retained exactly 72 and 29 VM steps across small/large histories; each old
kind-only scan still required 12,355 steps. Old-cut assertions also passed.
Native terminal appends and canonical Assistant batch work remained intact.
Nested durations overlap and must not be summed.

Both versions used freshly Cargo-emitted all-features libtest executables and
`--exact --nocapture --test-threads=1`, with identical opt-in test and Source
profiling. Each discovered exactly one test: one passed, zero failed/ignored,
4,153 filtered out. The temporary instrumentation was then removed; the final
source patch is 36 added lines and one changed call in this single test file.
The first uninstrumented full run used `CARGO_INCREMENTAL=0`,
`CARGO_BUILD_JOBS=4`, `RUST_TEST_THREADS=4`, and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. `git diff --check`, formatting, strict
all-target/all-feature Clippy, and `cargo build --bins` passed. The library
portion of `cargo test --all-targets --all-features` finished in 702.03s:
4,147 passed, three failed, four existing ignores. The corrected indexed
fixture, native cancellation/compaction receipts, and residual physical-read
join regressions passed. The three failures were the existing 30-second
liveness guard in `boundary_suites::app_server_file_read::{committed_present_reads_exact_native_scope_through_current_authorized_attachment,delivery_cancellation_is_request_scoped_at_every_native_interleaving,delivery_cancellation_is_admitted_as_the_sixteenth_in_flight_request}`.
Cargo stopped before the remaining targets; this is **not a passing full run**.
The same five-test containing group then passed at four threads in 3.27s,
which does not establish the cause of the full-run failures. Bounded,
failure-only fixture phase timings were added for a full-run reproduction;
no deadline, native operation, assertion, or scheduling rule was changed.
The diagnostic follow-up library run passed **4,150 tests, zero failures,
four existing ignores**, in 603.49s, including all five file-read cases. Its
fresh containing-group run also passed five tests in 3.26s. No incomplete-phase
report was emitted. The original three timeouts remain **unlocalized**; later
passes do not establish a fix or justify calling them infrastructure failures.
The first diagnostic compilation failed on a missing test-module import; that
was corrected before these runs, and its log is retained.

The follow-up `cargo test --all-targets --all-features` exited 101 after
879.58s because `tools::mcp_managed::two_folders_prepare_distinct_environment_identities`
could not prepare its first environment: `uv sync --frozen` failed downloading
`keyring==25.7.0` from `files.pythonhosted.org` after three built-in retries in
127.6s. This is direct external download-failure evidence, not an inferred
application exemption. The tools target reported 134 passed/one failed;
all earlier targets passed, including the 47-test subagent target and
`definitions::skill_version_identity_is_frozen_across_the_boundary`.
No owned Cargo, test, or uv process remained after exit. An unchanged exact
rerun passed one test in 38.12s; the complete unchanged tools target then passed
135 tests, zero failures/ignores, in 88.90s. No package-source override,
cache-mutation command, timeout increase, or test weakening was applied.
The remaining seven example targets passed (20 tests). Across the successful
follow-up targets and the tools rerun, **4,831 distinct Rust tests passed**,
with nine existing ignores and no filtered-out tests in the full target runs.
This aggregate is not a claim that the original full Cargo invocation exited
zero: its wheel-download failure remains above.

Final canonical commands on the retained diagnostic source:

| Command | Result |
| --- | --- |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --examples --all-features` | Passed |
| `cargo run --all-features --example check_test_lanes` | Passed |
| `pnpm --dir protocol/app-server check` | Passed |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | Passed |
| `pnpm --dir tui typecheck` | Passed |
| `pnpm --dir tui test` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | Passed |
| `pnpm --dir web-console build` | Passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed |

Dev reported 41 passed/zero skipped; TUI 1,163 passed/one existing skip;
Web 2,828 passed/four existing skips. No failures in these client commands.
Generated protocol checks produced no tracked changes. No fresh browser or
macOS run was performed for this test-fixture-only follow-up; the unchanged
published production/client head has the separately identified 317-case Web
and macOS CI passes. Those results do not validate unpublished changes on CI.
All logs and exact commands are preserved in
`../rustX-pi-performance-20261016/local-validation/`.

The paired local runtime reduction is not a claim that final-head CI has passed.
Raw logs, both instrumented patches, commands and SHA-256 evidence are retained
in `../rustX-pi-performance-20261016/ci-fixture/`.

### Final scoped Source index correction (schema 67)

Child-ordinal entries now retain only the canonical child path. The field entry
remains the sole complete Fact; original bytes, bounded previews, immutable
input identities, independent capture seats and publication frontiers are
unchanged. Pagination already followed the field lookup, so this adds no lookup.
The authenticated page remains at most 64 KiB with bounded depth; path decoding
checks its 4,096-byte limit before parsing, exact parent/depth and canonical
encoding. Rehashed replacement of a child path still fails against the original
independent capture seat. SQLite format 67 explicitly refuses format 66 without
conversion; App Server 50 and Runtime Client 71 remain unchanged.

Four added deterministic regressions cover these boundaries:

| Selector suffix | Boundary proved |
| --- | --- |
| `t08_t15_t20_child_references_keep_one_exact_fact_with_the_original_read_bound` | Independent original-source oracle, exact F bytes/headers/ranges, compact C payload, actual bounded page work and comparison with the former duplicated layout |
| `t08_t12_t19_authenticated_child_paths_reject_malformed_legacy_and_reparented_metadata` | Valid-page-hash malformed, noncanonical, overbound and wrong-parent references fail closed |
| `t08_t12_t19_original_child_references_reject_page_tamper_and_recapture_without_the_original_seat` | Actual native RequestStart; corrupt and fully rehashed C-tree replacement cannot replace original capture evidence; rollback preserves native facts and subsequent valid admission |
| `t10_t19_schema_66_full_child_facts_are_refused_without_conversion` | Real format-66 duplicated-C store is refused without changing its schema or bytes |

With the same four-thread/emulator environment above, `cargo test --lib
--all-features child_reference` passed three tests; `cargo test --lib
--all-features observation_request::composed::schema_tests` passed four;
`cargo test --lib --all-features durable::sqlite::` passed all 353 tests in
240.06s; and `cargo test --lib --all-features boundary_suites::durable::content`
passed all 27 in 8.95s. Formatting and diff checks passed.

The new default-feature production build used `CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=4 cargo build --release --locked --bins` and binary SHA-256
`3779ec09f2657abf025a39a437563c187af375ae4e43b76131ec4bfc3d0beb4e`.
Nine serial fresh-store runs (100/300/1,000, three repetitions each) passed all
4,200 requests. Every request's input/system/Tools hashes matched the previous
rustX, default Pi and both real pi-durable baselines. Original physical processes
joined and exited in every case; no mutation was retried or outlier discarded.

At 1,000 turns the old/new medians were: Source page payload 74.15/44.62 MiB;
combined Source tables and indexes 118.91/78.76 MiB; complete native database
204.01/163.86 MiB; runtime `write_bytes` 6,294.39/6,098.14 MiB. Request/root,
canonical and Journal counts remain N/N, 2N and 8N respectively, and Source node
counts are unchanged. Source pages fell from 63,010 to 44,021. These are measured
storage reductions, **not an end-to-end speedup**: CPU was 108.46/108.68 seconds,
wall time 153.937/154.442 seconds, and RSS 207.85/208.50 MiB. The 100-turn median
wall time increased from 11.044 to 20.824 seconds; those slow samples are retained
without an unsupported causal explanation. Historical campaigns are not an
interleaved paired experiment. No FULL synchronous durability, native history,
audit, Source authentication, or cold-read bound was weakened.

The first final-source all-target Rust invocation rebuilt its test targets and
failed after 813.86s of library execution: 4,148 passed, six failed, four ignored.
The five `boundary_suites::app_server_file_read` cases and
`boundary_suites::app_server_model_selection::children_created_after_selection_inherit_the_parent_and_running_children_allow_selection`
reached their existing cumulative 30s guard. For the three instrumented cases,
fixture construction alone took 16.480–19.625s, followed by 8.580–11.904s waiting
for native handoff. Observation was serviceable (J=2, no read failure, empty
observation queues); the measured initialize/attach stage was 0.433s. This
localizes elapsed time before the actual delivery-read gates, but does not
identify the physical owner responsible for the slowdown.

A narrow `strace` run of all five file-read cases passed in 5.31s; its 1,688
measured sync/flock calls peaked at 4.4ms. The exact original 114-test prefix
(70 Agent, 38 App Server, six boundary cases), using the same emitted library
binary, four workers and unchanged workloads, then passed in 32.58s. Its 2,648
measured sync/flock calls peaked at 6.9ms. Neither passing trace reproduces or
explains the original failure. No guard, assertion, scheduling policy or
production ownership was changed. The full rerun added only a bounded external
initial-180s read-only process-wait/I/O-pressure sampler; high aggregate host
pressure alone is not treated as proof of an infrastructure failure. The six
initial timeouts remain an explicitly unlocalized intermittent risk.

The unchanged-source full rerun exited zero in 809.85s: **4,835 passed, zero
failed, nine existing ignores across all 20 targets**, with no full-run filters.
The library reported 4,154 passed/zero failed/four ignored in 605.70s; all six
previously timed-out cases passed. The original skill-identity/recomposition
case passed in the complete 47-test Subagent target and again alone with
`--exact`. The bounded sampler recorded filesystem journal waits during this
successful run, which cannot explain the earlier run retrospectively.

Strict Clippy, formatting, binary build, test-lane discovery, App Server
regeneration/drift and TypeScript passed. Dev passed 41 tests; TUI 1,163 with
one existing skip; Web 2,828 with four existing skips. Their typechecks, Web
production build, i18n/provenance and all 55 fake-provider tests passed. Both
unchanged 300-Attempt reading cases passed independently with the pinned browser
on the default filesystem (2.5m each; wrapper times 170.10s/150.70s). The complete
`pnpm --dir web-console test:e2e` then passed **317 tests, zero failures/skips**
in 21.6m (wrapper 1,302.46s), including both original reading cases again. The
300-Attempt workload, 240s fixture limit, pinned browser and assertions are
unchanged. These local results do not assert new-head CI success.

Final-source command ledger (exact commands and logs are retained in
`../rustX-pi-durable-performance-20261016/source67-validation2/results.json`):

| Command | Final local result |
| --- | --- |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --all-targets --all-features` | Passed, 4,835/0/9; earlier failed invocation retained above |
| `cargo run --all-features --example check_test_lanes` | Passed |
| `pnpm --dir protocol/app-server check` | Passed, no generated drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | Passed, 41/0/0 |
| `pnpm --dir tui typecheck` | Passed |
| `pnpm --dir tui test` | Passed, 1,163/0/1 |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | Passed, 2,828/0/4 |
| `pnpm --dir web-console build` | Passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed |
| `cargo test --test subagent --all-features definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact` | Passed, one test |
| `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests` | Passed, 55 tests |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'native distant reading rail, detached/latest follow and measured width in both locales'` | Passed, one test |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'Fork source exact Message landing and cancellation by scrolling, newer navigation and attachment replacement'` | Passed, one test |
| `pnpm --dir web-console test:e2e` | Passed, 317/0/0 |

Counts use passed/failed/ignored-or-skipped. No live-provider tests were enabled.
Native validation retained `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4`,
`RUST_TEST_THREADS=4` and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. macOS and final-head
GitHub Actions remain remote checks; the published predecessor's green lanes do
not validate this commit.

After the explicitly approved workflow-budget edit, `cargo test --all-features
--example check_test_lanes` passed all 17 tests; `cargo run --all-features --example
check_test_lanes`, formatting and whitespace checks passed again. Parsing the old
and new YAML and comparing their complete structures confirmed that only the
`rust-contracts` whole-job timeout changed (25 to 45). The final pre-publication
fetch still resolved main to `8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already
an ancestor of the issue branch; no new merge or unmerged dependency was needed.

Raw runs, exact build/source/supervisor hashes, all-role resource tables,
closed-store accounting and reproducible reductions are retained under
`../rustX-pi-durable-performance-20261016/`, especially
`rustx-schema67-formal-report.md` and `source67-validation/`.

A separate production comparison uses official `earendil-works/pi` at
`90d863f7c1183b965f099855715d6e88c7b8a52c` (1.3.0), independently cloned to
`../pi-rustx-benchmark`; the pre-existing `../pi` checkout was untouched. rustX
was built with `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo build --release
--locked --bins` (default features), binary SHA-256
`78ccf74cc45c87ce1a76525ff931d9d98beb0520213151e3d53d42ec24377c92`.
Pi used `npm ci --ignore-scripts`, documented catalog hydration, and
`npm run build:offline`; bundle entry SHA-256
`e79626f2dd6f94aa45d30f3fa63cd84319a6eefcd150b353cfaf274366926774`.
Catalog hydration fetched metadata only; all inference used the unchanged local
rustX fake-provider HTTP/SSE engine.

All 18 formal runs (three repetitions per runtime at 100/300/1,000 turns) and
four supplemental temporary-space runs passed. Each used a fresh persistent
session, identical ordered text and output, no declared/invoked Tools, and an
empty system prompt. Every actual provider request's complete history, system,
and Tool hashes matched across products and workload prefixes. Exact native
settlement and exactly N requests were required; no semantic retries occurred.
Separate 10-turn warm-ups warm filesystem caches, not each new Pi process's JIT.
Runtime affinity was four physical cores; provider, driver, and sampler each
had a separate core. Affinity is not exclusive host isolation. High host I/O
pressure and all latency outliers remain recorded.

| Runtime | Turns | Loop seconds, median [range] | Runtime CPU seconds, median | Runtime sampled PSS peak MiB, median | Runtime process write_bytes MiB, median | Final DB/JSONL MiB, median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rustX | 100 | 11.04 [11.00, 31.33] | 8.95 | 176.12 | 537.93 | 19.93 |
| Pi | 100 | 0.28 [0.28, 0.30] | 0.40 | 123.69 | 0.08 | 0.08 |
| rustX | 300 | 43.83 [33.89, 44.53] | 28.44 | 185.37 | 1767.32 | 60.27 |
| Pi | 300 | 0.97 [0.96, 1.07] | 1.30 | 180.03 | 0.23 | 0.23 |
| rustX | 1,000 | 153.94 [153.75, 154.30] | 108.46 | 205.62 | 6294.39 | 204.01 |
| Pi | 1,000 | 6.61 [6.59, 6.63] | 5.69 | 378.00 | 0.75 | 0.75 |

These are product-cost measurements, **not equal durability or observation
work**. rustX retains canonical/audit/frozen Source evidence with SQLite
DELETE/FULL transactions; Pi's default session JSONL uses `appendFileSync`
without explicit fsync and lacks the same bounded observation contract. The
1,000-turn rustX database includes approximately 118.91 MiB of Request Source
and 46.84 MiB of native journal tables plus indexes. Pi's separate approximately
2.64 MiB shutdown compile cache is excluded from its history JSONL column.
The no-Tool scenario does not replace the earlier 17-Tool debug measurements or
the full browser's 300-Attempt correctness fixture.

CPU/IO deltas are measured before child reaping, avoiding double-counting
waited-child I/O in the driver. Provider, driver, and sampler are separate in
the raw report. Early/middle/late mean latency at 1,000 turns was
104.67/126.35/137.58 ms for rustX and 7.32/5.70/10.76 ms for Pi. Sampled
residency is not heap allocation, and temporary maxima can miss short-lived
files. All original runtime/provider processes were absent after physical
shutdown; final files were intentionally retained. Supplemental runs fill the
earlier sampler's missing temp-only metric without replacing formal samples.
No sampler caps, dropped records, or request-equality failures were reported.

Raw records, every sample, workload/adapter/source/binary hashes, build logs,
exact commands, host evidence, and the complete report are retained locally in
`../rustX-pi-performance-20261016/` (`final-report-v2.md`, `final-report-v2.json`,
`results/*`; the original report is retained). The report separates sampled
rollback-journal/WAL/SHM and deleted-FD space from explicit temporary roots,
without summing potentially overlapping categories. No new product optimization is justified solely by the timing
ratio; phase ownership must be measured before changing native work.

### Official pi-durable supplement and scoped Source follow-up

The previous 18-run comparison used default pi coding-agent, **not**
`packages/durable`. Its results and original release binaries remain preserved.
A separate campaign used the actual official pi-durable 1.3.0 production SDK at
the same `90d863f7` pin. Its native `Harness`/`Submission.wait`/idle/answer APIs
ran against the same local fake provider with SQLite and JSONL separately.
All 18 formal runs (100/300/1,000 turns, three repetitions per storage mode)
passed; every one of 8,400 requests matched corresponding historical rustX and
default-pi input/history/system/Tool hashes. No real model API was used, no extra
model call occurred, and all original runtime/provider/sampler processes exited.

| Native pi-durable storage | Turns | Median loop seconds | Runtime CPU seconds | Runtime sampled PSS peak MiB | Final native DB/JSONL MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| SQLite | 100 | 0.458 | 0.57 | 93.47 | 0.234 |
| SQLite | 300 | 1.159 | 1.47 | 161.23 | 0.520 |
| SQLite | 1,000 | 4.385 | 3.95 | 312.63 | 1.488 |
| JSONL | 100 | 0.478 | 0.63 | 91.10 | 0.171 |
| JSONL | 300 | 1.207 | 1.57 | 162.58 | 0.513 |
| JSONL | 1,000 | 4.502 | 4.36 | 303.93 | 1.716 |

These are separate campaign measurements, not equal contracts or a schema-67
rustX performance claim. Pi SQLite configures WAL/NORMAL; JSONL defaults to no
fsync. rustX retains DELETE/FULL, original request evidence, publication/audit
ordering and its bounded observation contracts. A new readonly diagnostic
connection's `synchronous` setting does not establish the original writer's
setting. Pi's task recovery may reissue committed work and rerun request hooks;
the timed lane disables both retry layers and does not claim immutable native
request or crash-recovery equivalence. The package remains experimental.

Exact builds, source/dist hashes, all repetitions/outliers, early/middle/late
growth, per-table/sidecar storage, temporary/WAL/FD observations and separate
runtime/provider/driver/sampler CPU and memory are retained in
`../rustX-pi-durable-performance-20261016/pi-durable-formal-report.{md,json}`.
This supplements rather than relabels the default-pi baseline.

The selected PR-sized Source optimization removes only duplicate child-index
Fact payloads. A read-only audit of a retained 100-turn database verified all
6,298 page hashes: every one of 7,500 `C` values duplicated an existing `F`
payload. Replacing those values with bounded paths removes 2,243,580 encoded
value bytes before page repacking, about 29.82% of that database's Source page
BLOB bytes. This is a layout measurement, not a measured application speedup.
The actual new binary and final validation must be measured separately.
`F` payloads, raw fragments, capture witnesses and FULL durability remain intact.
Broader binary-codec, lazy-preview, small temporary-index and static-configuration
sharing changes are not included without their own cold-read/equivalence evidence;
no asynchronous queue or cache lifetime authority is introduced here.

### Published-head CI and local resource investigation

At published HEAD `831c4010fc7c0c1a8b2f3bb0780b45eb0eb3cc52`,
[CI run 38858905282](https://github.com/Caismis/rustX/actions/runs/38858905282)
completed with passing Linux and macOS boundary suites, formatting/lint,
generated protocol, TUI, and macOS desktop checks. The separate Workspace Host
run passed on both platforms. **This head is not green:** Linux deterministic
contracts exhausted the existing 25-minute job budget, and Full Web conformance
reported 314 passes and three failures. No timeout or workload was increased.

Both reading fixtures reached 246 of the required 300 Attempts before the
unchanged 240-second setup deadline. Their applied cursors continued advancing;
the diagnostics did not report a source fault or replay gap. The third failure,
`convergence.spec.ts`'s Workspace-ready assertion, occurred after three successful
Tool results and continuous event cursors 1–44. The fourth model turn began only
about 153 ms before the assertion deadline. The trace contains no final assistant
output before teardown. These observations establish insufficient progress within
the guards, but do not identify every internal cost from CI alone.

The following changes and measurements were completed under a local-only hold.
On 2026-10-16 the user approved committing and pushing this validated batch to
the existing PR, without merging it. That approval does not establish new-head
CI success or resolve the measured memory and intermittent-latency limitations. The fixed
instrumented baseline binary has SHA-256
`e8c30e0702e3220a302e68e45c6687b8554715f20cd589c01e5fa18587904e39`.
Serial 100/300/1,000-Attempt measurements retain real App Server execution,
checkpoint/subscription, exact history reads, and fork preparation. They separately
record adapter preparation, Source construction, provider parsing, and client fold
cost. Resource measurements include per-process CPU and residency, cumulative
process I/O, retained SQLite table/index space, and sampled temporary-file space.
Process `write_bytes` is not final database size or SSD write amplification;
RSS/PSS is not live heap allocation; milestone temporary-file samples do not prove
an exact peak. The observed database journal mode is DELETE, not WAL.

The initial short resource runs omitted the Python provider descendant because
it was spawned from a launcher worker thread. Their native/storage data is retained
with that qualification; separately labeled corrected runs enumerate all launcher
threads. No incomplete provider measurement is treated as a complete total.
These measurements and local changes do not supersede failed published-head CI.

### Local scoped-reader measurement (review212)

The temporary immutable Source now lends one synchronous byte reader to a capture.
It retains its original geometry, preview, field, and span validation. The borrow
pins the scratch directory and the scoped File closes before capture returns;
it adds no task, authorization cache, or persistent content owner.
Three new regressions prove exact original bytes/field previews, the same actual
read work, errors before range I/O, physical read failure, and final scratch release.
Scalar capture uses three index opens and four byte-file opens instead of a count
proportional to fields and chunks. All 17 read-layout tests and three composed
Source-module tests passed; these targeted results do not replace full validation.

A fixed after binary (`31a782a1bc836439ea419fc47a96fb6c8b3974d1302dd1c67b6267502534ea24`)
ran the same serial workloads as the fixed baseline above. Each run retained exact
provider-request/settlement counts, bounded eight-entry heads, exact history/fork
assertions, and physically joined native/provider cleanup.

| Attempts | Total wall seconds, before → after | Native user+system CPU seconds | Median turn ms |
| --- | --- | --- | --- |
| 100 | 62.88 → 64.64 | 63.43 → 49.99 | 529.84 → 399.96 |
| 300 | 172.23 → 158.90 | 192.75 → 154.41 | 538.55 → 411.59 |
| 1,000 | 599.37 → 476.56 | 681.51 → 545.38 | 569.15 → 438.80 |

All outliers remain in these totals. In particular, the after-100 run includes a
22.34-second turn and does **not** demonstrate better total wall time. Most of that
interval precedes the first received adapter-preflight record. Current telemetry
cannot identify the physically blocked owner or exclude delayed telemetry delivery;
this is not established infrastructure failure. Source duration on that turn was
117 ms, not 22 seconds. No retries or new recovery operations were introduced.

At 1,000 turns, native RSS was 329.38 → 323.90 MiB and final databases were
348.45 → 348.41 MiB. Native logical read accounting (`rchar`) fell from 44.959 to
29.486 GB, whereas task write accounting stayed essentially flat at 7.855 →
7.851 GB. These are process counters, not measured SSD traffic. Source tables,
including their unused page space, retain the same physical layout in this step.
The fake provider separately retains every full-history request; its memory and
the Node harness's full shutdown-report decoding are test infrastructure, not
bounded product observation state. Exact heap allocation and transient temp peaks
were not measured.

A separate offline experiment rebuilt only `request_source_pages` on disposable
copies of the closed 100-turn database, then compacted both variants equally.
All 7,260 page hashes and BLOBs matched exactly; integrity, foreign-key checks,
unrelated schema and row counts passed. A rowid table with explicit hash NOT NULL
and a unique hash index used 13,996,032 bytes versus 24,616,960 bytes for WITHOUT
ROWID; seeded insertion used 14,041,088 bytes. Those totals include the new index.
This is packing evidence only. At this offline-experiment stage, the additional
B-tree lookup and application CPU, I/O, memory, and correctness remained untested;
the subsequent real application measurements and targeted tests follow below.

### Local Source page-layout validation (review215)

Schema **66** retains hash-addressed Source identities and all existing payload,
cut, and publication checks. Only the large page BLOB table changes physical
layout: a rowid table plus a separate unique 32-byte hash index. Explicit NOT NULL
preserves the prior WITHOUT ROWID primary-key constraint. Old schema 65 is refused
without conversion. App Server 50 and Runtime Client 71 are unchanged.

All **349 SQLite tests** and **27 durable-content boundary tests** passed on the
combined change. New tests verify effective constraints, the exact 64KiB boundary,
hash deduplication, unchanged native epoch on optional page writes, and refusal
of both old DDL and old schema versions. The real range test verifies a unique
hash index search without scanning. Existing exact-byte, cold-reopen, corruption,
optional/outer-transaction rollback, and native manifest tests remain intact.

The fixed combined binary has SHA-256
`b0bac729901de14ea54ec0f7a21bc99b766387f8b5643fdc98ccdb092a964acb`.
All three application runs passed exact request/settlement and physical-cleanup
checks. These are local debug-build measurements with scripted providers, not
release-build throughput guarantees.

| Attempts | Wall seconds: initial → scoped reader → combined | Native CPU seconds: initial → combined | Final database MiB: initial → combined | Combined Source tables and indexes MiB |
| --- | --- | --- | --- | --- |
| 100 | 62.88 → 64.64 → 86.36 | 63.43 → 50.50 | 36.87 → 26.10 | 25.81 → 15.11 |
| 300 | 172.23 → 158.90 → 160.91 | 192.75 → 154.37 | 106.19 → 74.66 | 75.59 → 44.07 |
| 1,000 | 599.37 → 476.56 → 471.85 | 681.51 → 540.60 | 348.45 → 244.25 | 249.08 → 144.92 |

The rowid step itself shows storage/I/O savings with essentially unchanged CPU,
RSS and median turn duration relative to the scoped reader; the 100-turn wall
result is worse and remains reported. At 1,000 turns the new hash index occupies
2.984 MiB, included in the combined totals. Source payload rises from 115.64 to
118.10 MiB because the index stores hash keys, while unused Source page space
falls from 132.53 to 25.56 MiB. No history, audit, Source or canonical content is
truncated or removed to obtain the reduction.

Initial → combined native counters at 1,000 turns: `rchar` 44.959 → 20.037 GB,
`write_bytes` 7.855 → 6.824 GB, RSS 329.38 → 327.29 MiB. The I/O and residency
qualifications above still apply. The repeated adapter-preparation proposal is
not implemented: its small measured CPU cost did not justify retaining the
provider payload across native commit and logical retries. Metadata deduplication
and wider layout redesigns are also deferred; neither is needed for these two
measured owner-local changes.

Full final validation follows these targeted results. The first strict Clippy
attempt (review216) rejected a new test's unseparated numeric literal; only the
literal spelling was corrected, with no value or production-code change. The
review217 rerun then identified function-length, iterator-counter, import-order
and semicolon lints. Small helper extraction and lazy iterator instrumentation
resolved those without changing capture or serialization semantics. Review218
`cargo clippy --all-targets --all-features -- -D warnings` passed (75.50s).
Review218 `cargo test --all-targets --all-features` passed in 3331.75s:
4,831 passed, zero failed, nine ignored across 20 targets (library: 4,150 passed,
four ignored). `cargo run --all-features --example check_test_lanes` passed.
The original `definitions::skill_version_identity_is_frozen_across_the_boundary`
test passed both inside the full Subagent target and individually with `--exact`.
`cargo build --bins`, App Server protocol generation/drift and typecheck,
`cargo fmt --all -- --check`, and `git diff --check` also passed. Both affected
reading tests passed independently through `web-console/scripts/browser-tests.sh`
with their exact title selectors and unchanged 300-Attempt fixtures (runner totals
2.7m and 2.6m). `pnpm --dir web-console test:e2e` then passed all 317 tests
with zero failures/skips in 21.8m, including both reading cases and the previously
failed convergence scenario. Published-head CI remains failed and cannot validate
this subsequently approved source. No commit, push, public PR update or merge was
performed during the local-only validation phase; publication was authorized only
after these results were reviewed.

### Final local command ledger (review214/218)

All commands ran serially. Native tests used `CARGO_INCREMENTAL=0`,
`CARGO_BUILD_JOBS=4`, `RUST_TEST_THREADS=1`, and the required local Provider Emulator.
No live Model API was called. The final fetch still resolves `origin/main` to
`8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already an ancestor of local HEAD
`831c4010fc7c0c1a8b2f3bb0780b45eb0eb3cc52`; the validated changes were uncommitted at that checkpoint. The subsequent
publication commit contains this ledger and the same tested source.

| Exact command | Result |
| --- | --- |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS; earlier review216/217 lint failures corrected without suppression |
| `cargo test --all-targets --all-features` | PASS, 4,831 passed / 0 failed / 9 existing ignores |
| `cargo run --all-features --example check_test_lanes` | PASS |
| `cargo test --test subagent --all-features definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact` | PASS, 1 test; full Subagent target also 47/47 |
| `cargo build --bins` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `git diff --check` | PASS |
| `pnpm --dir protocol/app-server check` | PASS; generated files unchanged |
| `pnpm --dir protocol/app-server typecheck` | PASS |
| `pnpm --dir dev typecheck` / `pnpm --dir dev test` | PASS; 41 tests |
| `pnpm --dir tui typecheck` / `pnpm --dir tui test` | PASS; 1,163 passed / 1 existing skip |
| `pnpm --dir web-console typecheck` | PASS |
| `pnpm --dir web-console test` | PASS; 2,828 passed / 4 existing skips |
| `pnpm --dir web-console build` | PASS; repeated by final E2E command |
| `pnpm --dir web-console check:i18n` | PASS |
| `pnpm --dir web-console check:provenance` | PASS |
| `uv run --frozen pytest` in `test-support/fake-provider` | PASS, 55 tests |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'native distant reading rail, detached/latest follow and measured width in both locales'` | PASS, 1 test |
| `bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts --grep 'Fork source exact Message landing and cancellation by scrolling, newer navigation and attachment replacement'` | PASS, 1 test |
| `pnpm --dir web-console test:e2e` | PASS, 317 tests / 0 failures / 0 skips |

There is no separate Web lint script in the maintained package scripts. Local
macOS checks were not run on Linux. The published-head macOS boundary/Host jobs
passed, but the new local SQLite layout still requires final-head platform CI.
The published Linux deterministic job's budget cancellation and Web failure remain
unresolved remote checks until the approved publication receives a new run; neither is
classified as an external infrastructure failure. Local passing tests do not
replace the controlled race evidence or guarantee latency on the CI runner.

### Current review corrections (previous local validation complete)

The current correction starts at `b0928347`. Current-attempt cancellation now
selects and cancels under the native coordinator mutex shared with admission and
settlement; the Host never consults a published Attempt or prepares observation
to authorize that control. Exact-ID cancellation retains its native identity check.
Manual compaction maps all five fields from that invocation’s native
`ManualCompactionOutcome`, without reading the latest presentation context.

TUI and Web cancellation dispatch use the exact live attachment authority even
when its observation is unavailable or behind. They do not treat a projected
Attempt as the command target. Web binds its acknowledgement marker to the
returned native Attempt identity, and cancellation does not request or await a
checkpoint refresh. The existing contiguous observation fold owns settlement.

All four `runtime_client::host::tests::native_cancellation_tests::` and four
`runtime_client::host::tests::compaction_head_tests::` tests pass on the combined
source. Cancellation proves exact settled-A display versus active native B,
a held native cancellation receipt across successor admission, real source
unavailability, and an actually blocked physical read plus held preparation lock.
Compaction proves actual optional-source failure, valid consecutive compactions
with a held first receipt, and a lost JSONL response over a Tokio duplex connection.
The latter is not an OS-pipe test. Model-call and native terminal counts remain exact.
The unchanged reading fixtures and complete local validation passed in the
historical review201/202 runs recorded below. Review218 independently passed the
current local source as recorded above; it does not establish remote CI success. Remote results are recorded in [PR #478 checks](https://github.com/Caismis/rustX/pull/478/checks)
for the exact pushed commit; historical results are not proof for the repaired source.

The current incremental Request Source shares augmentation on the native
selected-input tree. It retains one finite Request root and its original capture
witness instead of recopying every historical descriptor, field row and content
segment on each append. The former transient fragment import, message-shape
packing and `OriginalSpan` request-header implementations have been removed.
The independent flat byte/header producer remains a test oracle. The current
contracts and validation requirements are recorded under
[bounded work and source identity](#bounded-work-and-source-identity).

### Integrated native and client validation (review202)

On the frozen integrated source,
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features`
passed **4,821 tests, zero failures, nine existing ignores** in 3,272.496s.
The library contributed 4,140 passes/four ignores; external provider tests
contributed 179 passes/five ignored credentialed tests. All 47 subagent tests
passed, including `definitions::skill_version_identity_is_frozen_across_the_boundary`.
No live provider test was enabled.
`cargo run --all-features --example check_test_lanes` passed and confirmed
Linux contract/boundary discovery and coverage. Strict all-target/all-feature
Clippy had passed on this same production/test source in review200.

The preceding four-worker run passed 4,137 library tests but failed three initial
file-read fixture setups under their unchanged 30s guard. All five file-read
boundary tests then passed in isolation in 7.93s, and again in the complete serial
run. Their prior failure evidence and unproven cold-validator/contention hypothesis
remain in the repair history; the serial pass does not claim arbitrary parallel
scheduling robustness or final-head CI success.

Client checks passed: protocol generation/drift and TypeScript, dev TypeScript
and 41 tests, TUI TypeScript and 1,163 tests with one existing skip, and Web
TypeScript plus 2,828 tests with four existing skips. The isolated canonical Web
production build, i18n and provenance checks passed. The fake provider suite
passed 55 tests. Both original reading selectors then passed independently on the default
filesystem with the pinned browser, 300 Attempts and unchanged 240s setup limit:
distant-reading one pass in 3.5m (page assertions 4.1s); Fork-reading one pass
in 3.2m (page assertions 6.2s). Wrapper times 260.172s/193.437s include native/web
setup and are not the fixture durations. The complete `pnpm --dir web-console test:e2e` run then passed all **317 cases**,
zero failures/skips, in 23.2m (wrapper 1,401.974s), on the default filesystem.
Both original reading cases passed again in that full run. No fixture timeout,
workload, screenshot baseline or assertion changed. Remote CI must be read from PR #478 checks for the exact pushed commit;
these local results do not substitute for it.

### Native command regression evidence

| Requirement | Exact deterministic selector | Controlled interleaving and evidence |
| --- | --- | --- |
| R1-A: stale settled display, native successor active | `runtime_client::host::tests::native_cancellation_tests::cancellation_selects_native_successor_while_real_source_fault_retains_settled_display` | Native A settles, then a real SQLite trigger refuses optional event-witness capture during genuine B submission/execution. The original published settled-A attempt remains exact while the provider request-count watch proves B was invoked. A separate thread holds observation preparation before the actual cancellation RPC. Its correlated response names native B; original J contains exactly one Completed terminal for A and one Cancelled terminal for B; two model calls total. Unrelated nonattempt cursor advancement is permitted. No native fact, canonical write, or admission receipt is fabricated. |
| R1-B: successor cannot inherit cancellation | `runtime_client::host::tests::native_cancellation_tests::held_cancellation_receipt_keeps_selected_identity_after_native_successor_admission` | A cfg hook signals only after native cancellation has returned and physically holds the RPC response. The retained native settlement receipt proves A cancelled, then real successor B reaches its parked provider call. Releasing the response returns original request ID 51 and A's identity. A stale exact native cancellation naming A is refused. Releasing B's model gate yields Completed, with one original terminal per attempt and exactly two model calls. |
| R1-C: unavailable observation is not a cancellation prerequisite | `runtime_client::host::tests::native_cancellation_tests::native_cancellation_remains_available_during_real_observation_source_failure` | The original occupancy Source BLOB is made unavailable through SQLite; the actual subscriber reports typed Unavailable. With observation preparation held, the correlated cancellation RPC succeeds for the exact active native attempt while observation remains unavailable. The native settlement receipt and original J prove one Cancelled terminal, with one model call. The original BLOB is restored for cleanup. |
| R1-D: blocked physical observation read | `runtime_client::host::tests::native_cancellation_tests::native_cancellation_does_not_join_or_cancel_the_blocked_physical_observation_read` | An existing physical-reader hook signals actual blocked Source work. Observation preparation is also held. Cancellation succeeds and the exact native attempt settles Cancelled while the reader's cancellation signal is still absent. Only subsequent explicit cleanup cancels/releases and joins that original read. One model call and one original Cancelled terminal. |
| R2-A: committed receipt despite unavailable published context | `runtime_client::host::tests::compaction_head_tests::manual_compaction_receipt_survives_unavailable_published_context` | A real optional `projection_activations.observation_header` capture trigger fails inside its savepoint. Native compaction J and the immutable checkpoint still commit; published latest compaction remains old/absent and checkpoint reads report typed Unavailable. The successful response exactly matches the one original CompactionCompleted outcome and native checkpoint/activation; two model calls and no extra compaction. |
| R2-B: late A response after valid B commit | `runtime_client::host::tests::compaction_head_tests::manual_compaction_wire_receipt_keeps_a_after_a_valid_successor_b_commits` | A oneshot signals native A's committed outcome and holds its response. A genuine intervening Attempt supplies new history, then B commits through the real endpoint. The published context names B. Releasing and JSON-encoding/decoding A's endpoint response still gives request ID 41 and A's exact original outcome; B's response has ID 42 and B's outcome. Two original compactions and four model calls remain exact. |
| R2-C: lost response, reconcile without replay | `runtime_client::host::tests::compaction_head_tests::manual_compaction_lost_stdio_response_reconciles_without_replaying_operation` | The real JSONL serve/dispatch/write path runs over `tokio::io::duplex`, not OS pipes or a spawned stdio process. After the native committed-outcome signal, the client is dropped before the held response is released; the writer reports OutputBrokenPipe. Reattachment and the existing checkpoint reconcile that same outcome without another CompactContext request. Original compaction count stays one and model calls stay two. |
| Existing synchronous compaction publication contract | `runtime_client::host::tests::compaction_head_tests::committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked` | The actual worker is parked before the second compaction. Its checkpoint and contiguous replay already carry the exact second native head, Source cut and J frontier; one ContextCompacted replay fact, two original completion events and four model calls. Unparking preserves the same head. |

Client cancellation evidence on review189 includes TUI
`native cancellation forwards under stale unavailable observation and returns its actual owner`
and Web `unavailable stale A forwards cancellation and retains native B receipt without a repair read`.
They inject unavailable observation while keeping the real attachment authority,
then assert the exact native acceptance identity and no checkpoint request.
`native B acknowledgement survives stale A folds and permits a genuine observed successor`
holds the receipt/observation order and proves unrelated stale-A publication does
not mislabel or discard B's acknowledgement. The ordinary-RPC saturation test
keeps those slots occupied while cancellation resolves through its reserved lane,
with zero observation rereads. Retired attachment dispatch remains refused.

The earlier 30-test fragment group and eight request-shape passes describe the
pre-incremental implementation. Its header-emission and Native imported-shape
selectors were removed with that implementation and are not current acceptance
proofs. Current Source tests compare the composed representation with independent
flat serialization, every field, page and original range, and preserve reopen,
corruption, native-commit and original-witness checks. Native default structural
encoding, path and physical source-copy tests remain active.

On the compiled epoch-153 artifact, seven of the eight migrated request-shape
tests passed. The remaining test imposed a global successor-capture failure after
a selected leaf was corrupted; the current test instead proves that the successor
retains the original expected hash and that selecting the corrupted leaf still
fails. That test correction and the subsequent removal of retired code require a
fresh integrated run. The exact 20-by-4 Journal/rank admission matrix, normalized
Request/archive paths and persistent-manifest scaling tests are current selectors
below; no historical full-run result validates the final combined source.

### Repository and ownership

The repair starts from PR HEAD `6f9cfbdb3ebd4b145ba5d54c044a91b0832fada3` and merges
`origin/main` at `8cb41624eb2dc2ca87e4409961f42553ceb029cf` into the existing
`issue-467-bounded-observation` worktree. Main contributes PR #477 / Issue #466's
immutable-history model-context projection. The old base was
`97096020d64ea00218493a0a99b9b805821eeb28`. No unmerged PR is a dependency.

Execution and canonical history retain their existing owners. Original/Derived
provenance and frozen Attempt model input remain authoritative; display reads do
not reconstruct deprecated canonical compaction summaries. The original Completed-J
occupancy witness carries a finite projection head, so compaction head and context
publish atomically before asynchronous refinement. Lost commitment acknowledgments
reuse that original receipt, without repeating semantic work.

Composition shutdown first settles the semantic runtime, then closes presentation
reads and joins the original Projection Worker and every outstanding physical
Source read. The read previously could retain the Store Arc and exclusive product
storage lock after semantic shutdown. Cancelling a shutdown waiter now leaves the
original JoinHandle in its supervised slot; another waiter joins the same task.
A separate physical-read slot survives worker panic/abort and is drained before
successful ownership release. Dropping a handle is not claimed as physical proof.
Runtime Manager retirement waits for admitted operations and supervised drain;
internal inspection tasks retain their JoinSet/capacity ownership until termination.
Explicitly retained external leases still own storage until released.

The subsequent browser run exposed an admitted Observe wait that outlived listener
stop. Inspection shutdown now permanently retires its exact endpoint registrations
before cancelling socket readiness and joining all connection tasks. Retirement
shares initialization's identity lock, then detaches at the native subscription
lock; a racing Observe either closes or fails admission. Physical Source reads are
still awaited. The shutdown method borrows its owner and retains the original
listener handle across cancellation. Listener/connection join failure remains an
explicit persistent failure after surviving tasks drain; it is not release proof.

Source failure is typed observation unavailability, distinct from Replay eviction.
A retained historical checkpoint is not evidence that the current native source is
serviceable. Checkpoint/subscription admission consults current serviceability;
failed native reads fence the affected registration without inventing a cursor or
execution outcome. Only a successful read at the current dependency fence clears
the failure. Explicit recovery requests one bounded read through the existing
Projection Worker; concurrent requests at the same fence share that work. WebUI
and TUI preserve Applied, stop automatic resynchronization, and fence late results
by original registration/activation. Real Child Unix recovery preserves activation
and never falls back to a second Parent projection or semantic replay.

Current independent versions: App Server **50**, WebSocket **rustx.app-server.v50**,
Runtime Client **71**, SQLite **67**, Runtime Event **1**, Subagent Control **33**,
private Product Host **4**, Session catalog **16**. Generated contracts agree.

### Bounded work and source identity

Schema 65 introduced (and schema 67 retains) the replacement of prior per-Request copied segments and shape packs with
one native persistent selected-input tree and optional Source augmentation on
those same nodes. Request metadata binds the exact root; full historical input is
materialized only by explicit consumers. Publication and terminal checks use a
metadata-only path without selected-message hydration. Native semantic SHA and provider input identity
remain unchanged. SQL mutation certificates and post-INSERT validation retain
canonical trigger rejection and rollback behavior.

The same immutable compiled root now survives staging, selected-input assembly,
context anchors, default measurement, native Request commitment and provider input.
Exact incremental SHA state extends only for a certified append; edits, reordered
selection, changed media and compaction invalidate that evidence. Public fingerprint
algorithms and flat provider wire encoding are unchanged. Arbitrary estimators
still receive the whole original input; only the established JSON-byte estimator
uses the exact aggregate with its original rounding. Post-COMMIT evidence binds
the exact prepared values rather than reconstructing the whole Request a second
time. Canonical contributor snapshots separately share the existing Ledger bodies
through the Surface's exact selection; they never substitute model Derived input.

Incremental selected-input and Source maintenance does not make all request work
constant. Provider request materialization and exact wire fingerprinting still
visit actual input. Exact per-Request Tool association metadata is also serialized
into each Request and decoded by publication open/terminal/settlement checks;
ordinary frame staging does not perform that metadata read. The native validated
association proof removes repeated canonical-body validation, but does not remove
this metadata cost. Production observation Source omits the association array and
serves its own bounded authenticated pages. The scaling evidence must distinguish
these costs rather than claim constant work for an entire Tool-bearing request.

Source roots retain original capture witnesses and bind exact native identities.
Schema 67 stores one complete field Fact under its field key. Parent/ordinal
lookup entries contain only the bounded canonical child path. Pagination already
loads the field Fact after resolving that path, so this removes duplicate payload
and parsing without adding a lookup or moving work to a background owner. Original
fragment bytes, field headers, native selection and capture-witness validation
remain at their existing owners. Schema 66 stores are refused rather than decoded
through a compatibility path.
Fragments are authenticated pages capped at 64 KiB, with fragment depth below 16
and native path height at most 32. Range reads emit at most 64 pieces and return an
advancing partial chunk when needed; previews admit at most 512 pieces and retain
the 32-node display budget. The old schema-64 two-segment bound is retired rather
than attributed to composed reads. Tests count actual page, node and metadata
work and compare every original byte, field and continuation against the old
serialization oracle. Optional Source failure still rolls back independently of
native Request, canonical and Journal commitment. Authenticating a reused subtree
also follows its original capture path: worst-case node work can scale with
changed-path depth times origin-path depth, each bounded by 32. The implementation
does not claim a single-path visit count or maintain another cross-request cache.

The source-copy path pins one original file owner, reads at most 64 KiB at a time,
and copies only its certified extent. Physical ownership remains with that read
until termination. The new incremental storage tests and browser scaling evidence
must pass on the integrated source before these changes are treated as validated;
previous full-run passes below belong to the earlier source.

Prepared capability and Trace heads retain bounded normalized DTOs and their exact
encoded/conservative admission charges under their original owner. Changed records
replace that owner; held checkpoint seeds retain the previous rows and cut. Lazy
Trace preparation occurs off the Projection lock. Final full-record bounds,
checkpoint fitting, frontiers and normalization still apply. Exact UTF-8 string
admission charges raw bytes once plus escape expansion, with an original-byte-walk
oracle at every budget boundary.

### Deterministic repair evidence

The original repair selectors below were discovered and executed by final109's
complete Rust command; later opening-adoption tests are also covered by repair112
and final113. The latest full-run results remain distinguished below. The original T01–T20 matrix
below remains the architecture-wide mapping.

| Test selector | Boundary and synchronization evidence |
| --- | --- |
| `composition_shutdown_retry_joins_physical_read_before_immediate_recomposition` | Actual blocking Store read signals entry and logical cancellation; channel release gates physical termination. Cancelling the first shutdown waiter retains the original join handle. Retry stays pending until release, then weak owners expire and immediate real-lock recomposition succeeds. |
| `explicit_checkpoint_repairs_a_real_source_fault_on_the_original_worker_without_native_work` | Actual SQLite finite occupancy-source fault held through public checkpoint/subscription refusal; exact original BLOB restoration followed by public checkpoint wakes the original worker, publishes one read transition, preserves native cuts and makes zero model requests. |
| `unix_checkpoint_source_repair_keeps_the_current_thread_executor_serviceable` | Real Unix socket and JSONL server on a single-thread executor prove that checkpoint recovery yields the executor while the original Projection worker completes; persistent SQL fault stays unavailable and restoration advances one display transition without semantic work. |
| `projection_drain_joins_residual_read_after_worker_panic_and_waiter_cancellation` and `projection_drain_joins_residual_read_after_worker_abort_and_waiter_cancellation` | An actually blocked Source read retains its physical permit across worker failure and cancellation of the first drain waiter; retry stays pending until channel release, then preserves the worker outcome and proves both handles joined. |
| `committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked` | Two real native compactions with the read worker parked at the second completion; exact Completed-J, checkpoint cut and full contiguous replay already contain head activation two before background refinement. |
| `source_failure_is_unavailable_until_same_cut_native_read_repair` | Failed native read after a published checkpoint, no intervening semantic event, typed checkpoint/subscriber refusal, and successful same-cut source repair. |
| `read_failure_requires_successful_source_read_at_the_current_dependency_revision` | Installation under the Projection lock rejects obsolete read results; only the matching dependency fence clears source failure. |
| `failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut` | Native failure wakes observation without fabricating a cursor or execution fact. |
| `t18_child_source_failure_recovers_over_unix_at_the_same_activation_without_native_work` | Real Unix inspection transport preserves Child activation, typed unavailability and original source identity across repair without cold fallback or semantic work. |
| `manual_compaction_lost_commit_ack_publishes_the_original_receipt_after_owner_release` | Actual SQLite post-commit failure and held semantic owner prove original receipt publication ordering, unchanged canonical history and one compaction transition. |
| `giant_original_provenance_retains_scope_digest_and_exact_canonical_metadata_source` | A large original identity retains ancestor conversation scope and digest while its local canonical locator remains authoritative for deferred metadata. |
| `derived_provenance_admits_exact_byte_bounded_prefixes_without_copying_giant_aliases` | Combined encoded-byte admission retains exact provenance prefixes and explicit truncation. |


Opening-adoption regression selectors (six Store tests and one production-wire/Fork test):

| Selector | Proof |
| --- | --- |
| `uploaded_frozen_request_retains_the_exact_original_retry_boundary` | Real idle adoption, AttemptStart and upload-rendered Derived input; native retry identity/C1, reconstructed model request, RequestStart frontier and exact replay receipt remain original. |
| `retry_opening_source_keeps_native_frozen_input_validation` | Forged canonical digest fails before native RequestStart; adoption cannot bypass model-input integrity. |
| `uploaded_retry_deferred_identity_uses_the_same_original_anchor_as_its_locator` | A giant opening identity is deferred; both requests around later steering retain C1 and exact original UTF-8 ID bytes. |
| `retry_eligibility_uses_original_adoption_and_native_user_classification` | Nine source/content cases preserve Human/delegated-Agent text eligibility and exclude unadopted context, notices, Unicode whitespace and text-free media. |
| `retry_capture_ignores_optional_display_holes_and_other_attempt_steering` | Actual deletion of the optional C header cannot refuse native R/J or change captured opening identity; selected display read returns its exact missing-source error, C1 remains, and no hidden repair occurs. Previous-Attempt steering never opens a later Attempt. |
| `a_projection_summary_without_adoption_cannot_invent_a_retry_opening` | A real committed context projection and frozen Summary without adoption confer no retry authority. |
| `opening_adoption_retry_is_identical_in_live_wire_and_fork_lineage_after_steering` | Opening A, first request, later steering B, second request and settlement use the production bounded transcript serializer; real native lineage/Fork bootstrap preserves original A/C1 and execution origin without executing the copied Attempt. |

Additional bounded-work regression selectors follow. Unqualified composed-Source
selectors use `durable::sqlite::tests::request_fragment_tests::`; the three
unqualified same-transaction validation selectors and normalized load/archive
selectors use `durable::sqlite::tests::request_snapshot_validation_tests::`.
This is a current proof directory, not a claim that the final integrated run has
completed.

| Selector | Proof |
| --- | --- |
| `durable::sqlite::tests::request_fragment_tests::t08_t12_t14_t19_t20_canonical_fragments_match_full_source_every_field_and_original_range` | Independent flat producer versus composed Source: every original byte, selected field and advancing page/range. |
| `t08_t12_t19_parametric_request_holes_reject_root_corruption_before_allocation_and_restore_exactly` | SQL admission bounds and exact restored root metadata; no corrupt-reference expansion. |
| `t10_t12_t19_parametric_hole_identity_cannot_be_rehashed_or_laundered_into_new_requests` | Original capture witness prevents rebasing and later-request laundering. |
| `t10_t14_t19_parametric_root_insert_failure_preserves_native_request_and_recovers_missing_augmentation` | Optional-index savepoint failure leaves native commitments intact. |
| `t08_t19_exact_j_rank_pair_variants_preserve_admission_and_every_original_byte_at_the_16k_boundary` | Independent full producer for all 80 Journal/rank width pairs, including equal-width sums with different admission outcomes. |
| `t08_t14_t19_certified_request_roots_finalize_once_with_exact_legacy_geometry_and_witness` | Current composed root geometry and original witness versus the independent flat oracle. The historical selector name remains; retired packed-shape finalization is not exercised. |
| `t10_t14_t19_certified_root_finalization_failure_rolls_back_only_optional_source` | Actual root-insert failure trigger and healthy subsequent capture; no native rollback. |
| `model::selected_input::tests::selected_append_insert_replace_share_old_leaves_with_logarithmic_actual_node_work` | Persistent-root sharing and actual leaf/branch allocation counters at 100, 300, 1,000 and the 8,192-item boundary. |
| `durable::sqlite::tests::manifest_store_tests::native_manifest_warm_store_appends_are_delta_and_depth_bounded_at_100_300_1000` | Native warm append work and original full-input identity, without historical input reconstruction. |
| `durable::sqlite::tests::request_snapshot_validation_tests::normalized_request_metadata_publication_never_hydrates_selected_input_bodies` | SQLite authorizer refuses original input-body reads while scalar publication validation succeeds; explicit historical reconstruction still requires those bodies. |
| `normalized_request_resource_budget_charges_metadata_and_the_exact_flat_input_array` and `archived_normalized_request_preserves_the_exact_original_flat_snapshot` | Full-load budget boundary and exact archived original flat Snapshot JSON after normalized storage. |
| `normalized_request_metadata_budget_rejects_before_returning_any_owned_json` | SQL rejects metadata above the caller budget before returning an owned String; counters distinguish metadata rejection, exact metadata admission and full-budget reconstruction. |
| `normalized_request_digest_guards_every_reader_before_materializing_malformed_cells` | Every native Request digest reader admits only a 64-byte text cell; corrupt-size/type rejection returns zero owned digest bytes, preserves J and retains exact reconstruction after restoration. |
| `t14_t19_t20_provider_outcomes_decode_the_same_transaction_snapshot_once` | Old two-read versus new one-read materialization counters, for success and failure. |
| `t14_t19_snapshot_reuse_preserves_original_provider_validation_errors_and_precedence` | Independent original-path oracle across twelve malformed/foreign/duplicate cases. |
| `t14_t19_t20_publication_p_u_c_and_audit_each_reuse_one_validated_snapshot` | Actual public P/U/C/audit transactions retain all native start/outcome checks. |
| `runtime_client::checkpoint::prepared_capability_tests::` | Three exhaustive budget/normalization/exact-source-owner tests. |
| `runtime_client::checkpoint::prepared_trace_tests::` and `prepared_trace_owner_reuses_exact_rows_and_retires_after_native_terminal_install` | Four tests covering every budget, rejection/count frontiers, serde normalization, real native Tool transitions and held old cuts. |
| `runtime::read_layout::source_copy_tests::` | Four tests: exact original extent, bounded writes, actual file/index open counts, EOF/destination failure and owner release. |
| `t01_t15_string_budget_matches_original_byte_walk_at_every_boundary` | Original UTF-8/escape admission oracle for every surrounding budget. |

The original model no-op test now establishes the initial published Eligible state
before admitting an Attempt; waiting for that obsolete state afterward could miss
a coalesced transition. Background conformance preserves the initial dispatch
assertion, then waits for Running on the exact parked Job before reading status;
Starting→Running legitimately changes its native collection cut. Neither fix
retries mutations, accepts expired cuts, changes a deadline, or removes an assertion.

A retained-progress timeout occurred in earlier full runs while the same module
passed alone. Failure-only diagnostics now read one native J frontier and at most
12 indexed facts, with the original 10-second guard and all 256 retained facts
unchanged. Production progress semantics were not changed. Run 102 passed the
unchanged workload in the complete suite; earlier failures remain recorded in the
[repair history](issue-467-repair-history.md), not relabeled as passes or proven
infrastructure failures. Remote CI remains an independent required gate.

### Browser-discovered integration repairs

Browser103 exposed three distinct issues, without changing its existing assertions:

- A new Observe after RuntimeShutdown waited for its ten-second deadline while
  the inspection listener was trying to join connections. Exact endpoint retirement
  now wakes the pure wait while physical Source work remains joined.
- Upload rendering in main produces a Derived input, and later steering can
  change the last frozen input within one Attempt. Retry authority must instead
  come from the original opening adoption, shared by live and inherited responses.
  The final correction captures that original native identity inside request-start
  using canonical eligibility and adoption order, independently of optional display
  sources. Frozen Original/Derived model context cannot grant retry authority. The
  same captured anchor supplies the optional deferred retry-ID field.
- The exact Compaction Completed-J bootstrap already held its original bounded
  summary, but Trace assembly discarded it. Trace now retains that original preview
  and truncation flag. A later activation cannot replace historical content. No
  canonical summary Message, new read authority or schema change is introduced;
  truncated Summary/Raw views explicitly show Content deferred.

The added selectors are `runtime_client::endpoint::retirement_tests::` (three),
`local_runtime::live_inspection::shutdown_tests::` (two),
`t20_real_unix_shutdown_retires_the_exact_pending_observe_before_its_deadline`,
`request_retry_tests::` (six),
`runtime_client::response::tests::retry::` (one), and
`trace_preview_tests::` (two).
The existing `t20_real_unix_shutdown_joins_admitted_physical_source_worker` now
also cancels and retries the original shutdown waiter. These tests use installed
native subscriptions, immediate future polls, exact channels, actual Unix sockets
and native transaction facts. Web component tests add four summary identity and
truncation cases. Repair107 passed all twelve selected native tests (eleven new and one extended),
strict Clippy, binary build and all-target compilation. Browser108 passed all six
originally failed cases unchanged. Browser103 remains a failed historical run.

### Current validation commands

Final127's complete Rust command passed **4,784 tests, zero failures, ten
existing ignores** in 2,212.844 seconds: 4,103 library, 661 external-target and
20 example-target passes. The exact original failing test passed within the
47-test Subagent target and had also passed alone. All 237 native and 451 external
boundary tests are included; these subsets do not add distinct tests to the total.
The earlier final121 run passed all 4,782 then-existing tests at four libtest
threads before the two publication-barrier regressions were added.

Rust repair commands use `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Earlier runs used `RUST_TEST_THREADS=4`;
final126/127 use `RUST_TEST_THREADS=1` to separate cross-test storage contention
from each test's internal concurrency. Per-test budgets, barriers, work and
filesystem remain unchanged. Real provider APIs were not invoked. Existing
opt-in/credentialed ignores and client skips remain; none was added.

The two new `runtime_client::host::tests::settlement_tests::` cases positively
observe lock contention at drained-preparation and in-progress encoding before
releasing each boundary. Both pass alone and in the full command. The exact
cross-process ownership test also passes with serial libtest after child harnesses
explicitly select terse output. Failure-only diagnostics retain exact request,
publication and deadline evidence without issuing recovery operations. The
intervening failures, direct Linux I/O wait evidence, and macOS compiler timeout
are preserved in the repair history, not classified as proven external failures.

Client/production behavior is unchanged by the final cfg-only test repairs;
clients114 and the complete browser120 remain the local client evidence. Remote
Web at merge HEAD also passed all 317 browser cases and 2,823 unit cases. The
macOS compiler job now runs one Cargo compiler process at a time, retaining its
original debug level, libtest concurrency, selectors, assertions and 70-minute
budget. The next pushed-head CI result is required to validate that resource change.

| Exact command | Result | Repair run |
| --- | --- | --- |
| `git diff --check` | Passed | final127 final inspection |
| `cargo fmt --all -- --check` | Passed (4.530s) | final127 |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed (73.846s) | final127 |
| `cargo build --bins` | Passed (79.229s) | final126; subsequent source changes are cfg-only |
| `cargo test --all-targets --all-features --no-run` | Passed (147.206s) | repair107 |
| `cargo test --lib --all-features runtime_client::host::tests::settlement_tests:: -- --nocapture` | 2 passed (160.844s including compile) | repair122; both also pass final127 |
| `cargo test --lib --all-features scripted_suites::runtime_client::protocol:: -- --nocapture` | 14 passed (0.566s) | repair122 |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::model_selection_noop:: -- --nocapture` | 6 passed (1.379s) | repair122 |
| `cargo test --lib --all-features local_runtime::session::tests::deletion_tests::deletion_cross_process_parent_death_preserves_nested_ownership -- --exact --nocapture` | 1 passed (164.542s including compile; 0.58s execution) | final127, serial libtest |
| `cargo test --lib --all-features runtime_client::endpoint::retirement_tests:: -- --nocapture` | Passed (160.112s) | repair107 |
| `cargo test --lib --all-features local_runtime::live_inspection::shutdown_tests:: -- --nocapture` | Passed (0.273s) | repair107 |
| `cargo test --lib --all-features t20_real_unix_shutdown_ -- --nocapture` | Passed (0.479s) | repair107 |
| `cargo test --lib --all-features request_retry_tests:: -- --nocapture` | Passed (177.713s) | final113 |
| `cargo test --lib --all-features trace_preview_tests:: -- --nocapture` | Passed (0.313s) | repair107 |
| `cargo test --lib --all-features runtime_client::response::tests:: -- --nocapture` | Passed (58.444s) | repair112 |
| `cargo test --lib --all-features scripted_suites::agent::tool_progress:: -- --nocapture` | Passed (164.508s) | final121 |
| `cargo test --all-targets --all-features` | Passed (2,212.844s), 4,784 tests | final127, serial libtest |
| `cargo test --all-features --test subagent definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact --nocapture` | Passed (0.843s) | remaining117 |
| `cargo test --all-features --test subagent` | Passed (35.549s) | remaining117 |
| `cargo test --lib --all-features -- boundary_suites::` | Passed (80.332s) | remaining117 |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Passed (149.023s) | remaining117 |
| `cargo run --all-features --example check_test_lanes` | Passed (16.182s) | final127 |
| `pnpm --dir protocol/app-server check` | Passed (79.945s including generator compile) | final127 |
| `uv run --frozen pytest` | Passed (0.359s) | remaining117 |
| `pnpm --dir protocol/app-server typecheck` | Passed (0.873s) | clients114 |
| `pnpm --dir dev typecheck` | Passed (1.400s) | clients114 |
| `pnpm --dir dev test` | Passed (2.730s) | clients114 |
| `pnpm --dir tui typecheck` | Passed (3.699s) | clients114 |
| `pnpm --dir tui test` | Passed (35.800s) | clients114 |
| `pnpm --dir web-console typecheck` | Passed (13.023s) | clients114 |
| `pnpm --dir web-console test` | Passed (35.241s) | clients114 |
| `pnpm --dir web-console build` | Passed (5.890s) | clients114 |
| `pnpm --dir web-console check:i18n` | Passed (0.879s) | clients114 |
| `pnpm --dir web-console check:provenance` | Passed (1.061s) | clients114 |
| `bash web-console/scripts/browser-tests.sh test/e2e/child-capabilities.spec.ts test/e2e/commands.spec.ts test/e2e/compaction.spec.ts` | Passed (195.696s) | browser108 |
| `pnpm --dir web-console test:e2e` | Passed (1499.771s) | browser120 |

Linux/macOS remote results must be checked against the exact pushed HEAD in
[PR #478 checks](https://github.com/Caismis/rustX/pull/478/checks). Old-head CI
failure is not represented as repaired-head success. The dedicated worktree and
branch remain available for review.

Earlier repair commands, failed runs, environment diagnostics and intermediate
results are preserved in [the chronological repair history](issue-467-repair-history.md).
That history contains obsolete intermediate statuses and versions; this section
is the current repair evidence. Pre-review records below are separately historical.

## Historical pre-review complete-command evidence

All Native validation used `CARGO_INCREMENTAL=0` and required Provider Emulator
availability (`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`). The full Rust command used
`RUST_TEST_THREADS=4`. Epoch numbers identify retained command logs, not versions.
Within this pre-review record, later changes after Native111 were confined to
clients/tests and documentation. The main integration and subsequent Native
repairs are recorded separately above.

| Command | Pre-review result |
| --- | --- |
| `git diff --check` | Passed (114; repeated after documentation edits). |
| `cargo fmt --all -- --check` | Passed (111). |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed (111). |
| `cargo build --bins` | Passed (111). |
| `cargo test --all-targets --all-features --no-run` | Passed (111); expected targets discovered. |
| `cargo test --all-targets --all-features` | Passed (111), 4,604 passed; four opt-in measurements, one corpus generator and five credential-dependent live-provider tests ignored. |
| `cargo run --all-features --example check_test_lanes` | Passed (111); every runnable Linux test assigned. |
| `pnpm --dir protocol/app-server check` | Passed (111 and final staged-tree rerun), including generated schema/TypeScript/fixtures drift. |
| `pnpm --dir protocol/app-server typecheck` | Passed (113). |
| `pnpm --dir dev typecheck` | Passed (111). |
| `pnpm --dir dev test` | Passed (111), 41 tests. |
| `pnpm --dir tui typecheck` | Passed (113). |
| `pnpm --dir tui test` | Passed (113), 1,153 passed / one existing macOS ACL skip. |
| `pnpm --dir web-console typecheck` | Passed (114). |
| `pnpm --dir web-console test` | Passed (114), 2,809 passed / four existing macOS Office skips. |
| `pnpm --dir web-console build` | Passed (114). |
| `pnpm --dir web-console test:e2e` | Passed (114), all 317 cases, zero failures/skips, 1,403.459 seconds; fresh tmpfs temporary data, as qualified below. |
| `pnpm --dir web-console check:i18n` | Passed (114). |
| `pnpm --dir web-console check:provenance` | Passed (114), 276 source records / 264 package notices. |
| `uv run --frozen pytest` in `test-support/fake-provider` | Passed (107), 54 tests; unchanged provider source. |
| `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v` | Passed (107), 15 tests; unchanged notification source. |
| `node web-console/scripts/check-office-sandbox.ts` | Passed (107), actual Linux sandbox admission. |

Real Stdio/WebSocket, Child Unix IPC, storage/restart/range and publication gates
are part of the full Native and client commands, with separately executed selectors
recorded below. The standalone Office sandbox preflight also passed. macOS-only
execution is not available in this Linux workspace and remains a distinct CI gate.
Historical failed commands are retained in their epoch sections; their later
passing reruns do not erase the original diagnostic evidence.

## Ownership and commit boundaries

| Owner | Authority and boundary |
| --- | --- |
| Agent Loop and supervised Tool owners | Execution admission, cancellation, physical completion and terminal settlement. Observation teardown has no execution cancellation capability. |
| Conversation Runtime / Durable Store | Canonical messages, native Journal facts, pending inbound inbox and source retention. Finite observation layouts derive from the original transaction; a derived-layout failure cannot refuse or replace its semantic commit. |
| InteractionCoordinator / Child route owner | Pending interaction decision and cancellation authority. Immutable bounded byte layouts are observation evidence, not a second interaction owner. |
| Publication Plane | Assembler → Coalescer → Frame → durable staging commit → native observation → user release. Frame receipts prove the readable frontier; reads cannot flush, stage or settle. |
| Runtime Client Projection | One native cursor for a complete bounded display transition, finite checkpoint descriptors, native Replay and exact cut evidence. Source callbacks carry complete canonical replacement facts; delayed callbacks cannot roll a later published cut backwards. |
| Subagent Registry / inspection owners | Registry owns Child activation. Runtime Client `ClientInner` and the Unix listener own physical inspection capacity; logical cancellation does not release a permit still owned by a physical task. |
| App Server Connection | Root attachments, connection-local request associations, authorization and delivery coordination. Exact admission tokens fence delayed cleanup; no durable idempotency registry. |
| Registration owner / Writer | PendingReceipt → Following → RecoveryRequired → Retired. Final validation, bounded candidate selection, one non-suspending physical acceptance and finite disposition commit share one boundary. |
| Client observation state | Applied cursor after complete contiguous fold, exact target/activation/view fencing, finite byte caches. Eviction never changes Applied or native facts. |

Checkpoint capture freezes finite descriptors under Projection synchronization,
materializes outside it, validates the original native cut, and shares two attempts
and one five-second deadline. The Journal, canonical, pending and transcript source
frontiers remain distinct. Canonical replacement is complete or explicitly
unavailable when derived source evidence cannot be obtained. The loaded transcript window is a cache at its captured
cut; canonical commitment does not invent transcript rows or Tool completion.
Strict independent checkpoint/event fixtures compare every wire-visible field,
including empty block frontiers and C-before-J result facts.

Provider terminal outcome P, Publication terminal outcome U, and Canonical
acceptance C retain their original P → U → C ordering. Streaming release is a
separate per-frame boundary: the original durable frame transaction commits,
its finite receipt is installed as publication observation, then the producer
returns to user-facing release. A later canonical Tool result may exist without
its execution-terminal J fact, and a successful Tool J fact may precede result C;
neither is substituted for the other. Public reads cannot create either fact.

The concrete checkpoint owner is `ClientInner::materialize_checkpoint`: under
the published-state mutex it validates the published Source and clones the finite
`Arc<CheckpointSeed>`. After releasing the mutex, it builds the candidate,
materializes ContentBindings and explicitly requested Trace records, and fits
the bounded checkpoint. It then validates that exact cut under the published-state
fence before returning. `ReadDomainCut` remains part of native worker preparation,
not a new read-side projection owner. A failed candidate consumes the
same two-attempt/five-second budget, not a fresh deadline. Domain Page/Value,
transcript and ContentRead likewise select the already-published source; they
do not prepare the worker or drain its queued facts. The parked-worker proof is
`runtime_client::host::tests::pure_read_tests::t08_t14_t19_pages_and_value_reads_leave_native_progress_queued`.

| Finite boundary | Current owner and limit |
| --- | --- |
| Complete transport record | `app_server/transport`: 1,048,576 encoded bytes including envelope/ID/escaping. |
| Checkpoint | `runtime_client/checkpoint.rs`: 786,432 target bytes; item budget 16,384; ordinary page 32 items; native head 8 items; two attempts under one five-second deadline. Final encoding still checks aggregate size. |
| Source byte window | Ordinary segmented canonical/Journal/state sources: 65,536 returned decoded bytes and at most two finite segment rows. Composed Request Source: at most 64 emitted pieces per range, possibly a shorter positive chunk; authenticated pages ≤65,536 bytes, fragment depth <16 and native height ≤32. Previews admit at most 512 pieces under the 32-node display budget. Metadata has independent finite-column guards. |
| Native Replay | `runtime_client/projection.rs`: 4,096 records and 16 MiB; initial-cut evidence is charged to this owner and expires with its Replay cursor. A retained Trace cache separately pins only its actual represented cut. |
| Request admission | Per connection: eight Observe, six ordinary and two control associations. The existing shared AppServerHost residual-work counter separately reserves controls at `max_connections × 14` ordinary / `max_connections × 16` total; retired connections cannot free still-running operations. |
| Child inspection | `ClientInner.inspection_capacity`: eight physical inspection permits. One admission-time 15-second deadline survives Registry, Unix initialization, materialization and fallback. Blocking Source work owns its permit until physical exit; the Unix listener also bounds and reaps its workset. |
| Trace/UI working sets | Native Trace refresh admits eight interests; the Web cache keeps at most eight detail entries/16 MiB. A selected request holds one advancing child page and one replaceable ≤64 KiB original byte window. |
| Selected non-Request Trace detail | `trace/inspection.rs` and `trace_observation/inspection.rs`: explicit finite head, capped at 512 KiB; eight canonical heads initially; advancing selected pages at most 32 items. Original bodies use existing ≤64 KiB range reads. The complete transport encoder remains the final record bound. |

Native attempt admission publishes the frozen Admitted view and its model/settings
as one complete checkpoint transition before the Agent Loop starts execution.
It does not publish AttemptStarted. A prepared Started witness that arrives after
the exact native occurrence is already installed does not reset its disposable
progress or advance the published cursor. Terminal Journal facts still settle that
occurrence. Current-cut foreground pages may retain progress from the identical
published seed; historical pages reconstruct only their captured durable facts.

The acceptance lock order is carrier retirement fence → route table →
registration → attachment/native Projection.
The WebSocket writer acquires the Socket first, completes readiness/previous Flush,
then enters the registration boundary only for the final tungstenite handoff.
Replacement acquires neither Socket nor the carrier fence. Teardown marks the
carrier retired and releases that fence before closing connection routes.
No source I/O, readiness wait or Flush is held
under registration/Projection coordination. Stdio acceptance is its first positive
nonblocking write; that accepted JSON record and delimiter are then finished even
after replacement. Unaccepted records retire on EOF or shutdown without requiring
physical readiness. WebSocket Flush cannot retract an accepted Frame.

App Server is v48 (`rustx.app-server.v48`), Runtime Client v69, Subagent Control v33,
SQLite layout v58, and Session catalog v16. The independent private Product Host
file-read handshake is v4 (`rustx.product-host.file-read.v4`), with a finite
canonical locator replacing its obsolete message-ID-only source. Runtime Event
envelope remains v1.
These are independent domains; the new storage layout indexes existing authority
and does not introduce a second history or durable display-event journal.

## Method and receipt audit

All 98 current `Method` variants are classified exhaustively by `Method::may_commit`;
the native routing match is also exhaustive. There are 66 read variants, 31 variants
that can commit domain/connection resources, and `Goal`, whose `Show` is a read.
The internal Runtime Client has 39 current request variants; none reintroduces a full-snapshot endpoint. Its synchronous/async dispatch retains bounded typed refusal for requests requiring the other path.
An encoding failure after a successful potentially committing operation reports
`OutcomeUnknown`; definite admission refusal is separate. No client automatically
replays a mutation. The audit covers each current variant in these groups:

| Variants | Receipt / read boundary |
| --- | --- |
| Initialize; SessionAttach, SessionDetach, SessionSubscribe | Finite authority/capability or attachment/registration identity. AttachmentStatus reconciles same-connection receipt loss; Subscribe owns a pending physical receipt. |
| SessionCreate, SessionName, SessionFork, SessionBranch, SessionSwitchNode | Finite Session metadata and immutable editor locator; editor bytes and node origin are independent bounded reads. Post-commit failed readback remains OutcomeUnknown. |
| SessionDelete, SessionRecoverDeletion, SessionExportPrepare | Existing finite deletion/durability outcome and download identity; no embedded archive or node graph. |
| SourcesWrite, ConfigurationReconcile, AdoptConfiguration | Exact finite revision/application outcome; source/effective/application documents are independent bounded fields and pages. |
| ModelSet, ModelSelect | Exact settings revision; catalog selection and current settings are independent native reads. |
| SessionUploadPrepare | Finite transfer identity/outcome; admitted manifests/chunks retain existing upload bounds and uncertainty ownership. |
| TurnStart, TurnSteer, InboundEdit, InboundRemove, TurnCancel | Finite inbound identity/sequence/CAS or cancellation outcome, never copied canonical history. |
| InteractionRespond, InteractionCancel; JobCancel; AgentSendMessage, AgentInterrupt; DeliveryCancel, AgentObserveCancel | Existing exact decision/action/cancellation owner with finite outcome or identity. Observation cancellation has no execution cancellation capability. |
| CompactContext; Goal | Finite maintenance/Goal display facts and source descriptors; original Compaction/Goal authority and model-context semantics are unchanged. Goal Show is read-only. |
| AgentArtifactRead, AgentDeliveryRead, AgentDeliveryLocate, ArtifactRead, DeliveryRead, DeliveryLocate | Existing protected artifact/delivery authority and transfer bounds; ContentRead cannot widen those permissions. |
| SessionUploadStatus; ServerInfo, ServerDiagnostics; SessionList, SessionRead, SessionSummary, SessionNode, SessionTree, SessionBoundaries, SessionDeletePreview, SessionAttachmentStatus | Bounded identity/metadata/diagnostics or advancing native catalog pages. |
| SessionHistory, SessionTraceHistory, SessionTraceHistoryDetail, SessionTracePage, SessionTraceValueRead, SessionTraceContentRead, SessionStatistics, SessionNodeValueRead, SessionNodeOriginHistory, SessionHistoryContentRead, SessionHistoryPage, SessionHistoryValueRead, SessionEditorRead | Exact historical native source/cut, finite headers, advancing pages and bounded range/value reads; no node activation. |
| Trace, TraceDetail, TraceLocateTool, Transcript, ConversationTurns; AgentTrace, AgentTraceDetail, AgentTraceLocateTool, AgentTranscript, AgentTurns, AgentStatistics | Bounded exact-cut live/retained projection and directories; typed unavailable/cut/source failures remain distinct. |
| ModelGet, ModelCatalog, Capability; JobStatus, JobList, JobWait; AgentStatus, AgentList, AgentObserve, AgentWait | Finite current heads and source descriptors/pages; Observe alone follows the Child's own native Replay with admission/deadline ownership. |
| SessionCheckpoint, ConversationContentRead, ConversationCheckpointPage, ConversationCheckpointValueRead | One native checkpoint cut or bounded source selection/range. No read-side flush, commit, settlement or model work. |
| ConfigurationGet, ConfigurationValueRead, ConfigurationPage, SourcesRead, SourcesPage, SourcesValueRead, McpProbe, SettingsRead, SettingsValueRead, SessionConfiguration, SessionApplicationValueRead | Bounded settings/effective/source/application fields, exact native revisions and advancing collection/value reads. |

## Bounded source evidence

Native canonical, Journal and inherited provenance records remain authoritative.
Display identities may omit an oversized inline alias while retaining its original
numeric locator or immutable field selector. Original bytes are available through
explicit bounded reads at the advertised cut. Public Turn navigation uses the
returned native ordinal and gesture-captured cut; native execution/control IDs
remain required. A missing source is an explicit observation failure, never an
empty result, fabricated identity or request to mutate execution.

Inherited Turn headers carry a finite row witness over the header kind, original
ordinal, validated native payload digest and exact encoded header bytes. Native
and observed headers have separate witnesses. Reads verify this binding before
accepting owner, preview, cursor or byte-coordinate fields, without loading the
original payload or member list. The optional header savepoint can fail while the
original bootstrap, canonical history and inherited provenance remain committed.

Inherited Assistant membership retains the original paired foreign key over
Ledger position and MessageId. Its finite read witness binds the numeric position,
original identity digest, Turn/Response ordinals and original membership digest.
A different valid canonical position cannot reuse that witness. The witness is
derived read evidence; it neither replaces the paired native relationship nor
confers source or filesystem authorization.

Both read paths guard SQL column types and byte lengths **before** `row.get`
allocates Rust metadata. Turn header reads select at most a 16,384-byte BLOB,
64-byte native digest and 32-byte witness. Membership lookup selects one indexed
row containing three 32-byte digests/witnesses, a 64-byte digest and scalar
ordinals. Corrupt oversized columns become unavailable through SQL `CASE`
expressions, including when a diagnostic fixture bypasses schema checks. The
membership proof measures at most 160 returned metadata bytes and checks that
its query plan uses neither a full scan nor a temporary B-tree.

| Source proof | Contract exercised |
| --- | --- |
| `inherited_turn_row_witness_rejects_changed_owner_preview_cursor_and_encoded_bytes` | Reject changed finite header fields while original provenance and its row evidence remain unchanged. |
| `inherited_turn_evidence_sql_bounds_precede_rust_metadata_allocation` | Reject oversized header/digest/witness columns before Rust allocation, with original payload columns denied. |
| `inherited_turn_header_row_witness_failure_preserves_original_bootstrap` | Optional witness-write failure preserves the original bootstrap and returns explicit observation unavailability. |
| `inherited_turn_header_row_witness_columns_are_required_by_schema` | Require the current witness columns and bounded table shape; no old-layout compatibility path. |
| `t08_t12_t14_t19_giant_native_membership_witness_binds_exact_numeric_c` | Resolve giant original membership through numeric C with original ID/body columns denied; reject a substituted Ledger position. |
| `t10_t14_t19_membership_read_witness_failure_preserves_native_lineage` | Preserve original canonical, Turn and Response authority when the optional membership witness write fails. |
| `t08_t14_t19_membership_sql_bounds_reject_corruption_before_metadata_allocation` | Reject oversized membership metadata at SQL selection, retain exact native lineage and measure the finite read workset. |

Native request-start and provider-outcome probes use the existing
`events_request_id_kind_idx` over original Journal JSON. They never use optional
observation headers as semantic authority. The start probe decodes at most two
selected original envelopes and still rejects duplicate starts. The outcome
probe takes the earliest original Completed and Failed candidate separately,
then selects their earliest native sequence. Original request/envelope ownership
validation, provider terminal refusal and transaction durability remain unchanged.
No schema, index, event authority or model-context semantics were added.

The pinned epoch-105 new-store path creates all current tables, indexes and root
metadata and verifies their shape inside one existing SQLite `Immediate`
transaction. Only a successful verification commits that schema birth. This
removes repeated DDL commits without weakening native `FULL` synchronization or
`DELETE` journal mode; `bind_identity` remains its separate original native
transaction. It introduces no migration or compatibility path for layout 58.
The new deterministic selectors
`durable::sqlite::tests::new_schema_birth_rolls_back_late_ddl_and_commits_once_with_full_durability`
and
`durable::sqlite::tests::new_schema_birth_rolls_back_when_shape_validation_fails`
respectively inject a late real index-name conflict and an authorizer refusal
during final shape validation. They require rollback of every newly created
object/root row, preservation of pre-existing sentinel bytes, successful retry,
one measured schema commit, unchanged durability pragmas and normal identity
binding/reopen. Both exact tests passed at epoch 105 in 7.342s and 0.030s,
respectively (`/tmp/467-focused105-results.json`, logs
`/tmp/467-focused105-0.log` and `/tmp/467-focused105-1.log`).

The auxiliary-write audit found that bounded event fallback JSON and several
derived coordinates could still refuse the original Journal commit on SQL
failure. The pinned closure initializes the existing row availability marker to
unavailable and clears it only after all auxiliary writes succeed. The clear is
itself optional. Independent savepoints preserve already-captured own lifecycle
facts when a different layout fails, and witness reads OR the final committed row
marker rather than retaining an initial unavailable bit. Native Tool ownership,
canonical-call correlation, lifecycle settlement, original J/T allocation and P
remain mandatory. The two new trigger regressions,
`t10_t14_t19_bounded_event_fallback_failure_keeps_original_terminal_and_lifecycle`
and `t10_t14_t19_assistant_tool_enrichment_failure_keeps_native_c_and_tool_owner`,
passed again on the linked epoch-99 binary in 0.200s and 0.082s respectively
(`/tmp/467-final-focused99-results.json`, logs
`/tmp/467-final-focused99-3.log` and `/tmp/467-final-focused99-4.log`). The first
injects fallback-JSON, marker-clear, transcript-coordinate and witness SQL
failures, retaining the exact original Failed/Cancelled J, native T, terminal
absorption and explicit Source unavailability. The second retains original
Assistant C bytes, mandatory canonical-call ownership and commit sequence, and
the actual Tool occurrence despite derived Assistant/Tool-coordinate failures.
Both verify a subsequent unfaulted native event has available Source evidence;
no canonical Tool result or execution settlement is invented. At that historical stage, these focused passes did not establish the then-pending
full-suite or browser results. Current integrated results are recorded above.

Recoverable readiness has a separate existing owner flag.
`observation_readiness_missing` fences a row until `observe_journal` has installed
its native readiness layout; the optional repair clears only that flag. It is
not folded into the permanent `observation_unavailable` marker. Receipt batches
OR both indexed flags and selected witnesses inspect both, so a readiness-only
repair can recover while a simultaneous unrelated auxiliary fault remains
unavailable. The deterministic regressions are
`durable::sqlite::tests::t10_t19_t20_readiness_layout_failure_never_refuses_native_journal_commit`
and
`durable::sqlite::tests::t10_t19_t20_readiness_repair_preserves_independent_auxiliary_failure`.
The first exposed the stale marker in the epoch-102 full run; the repaired
implementation and strengthened cases are compiled in 104. At that historical stage, their presence in discovery did not substitute for
full execution. The current integrated run executes both regressions.

The epoch-95 retained delivery database isolated a separate healthy-path failure:
generic event-field paging interpreted original Tool result properties named
`content` and `collections` as Source metadata. Successful Tool J8 therefore
retained its native outcome but fell back to an unavailable observation before
the later Tool C. Both structural field readers now decode bindings/frontiers
only at `request_observation_fields`' exact `messages.<ordinal>` path, whose
native `message_value` producer appends that metadata. Original Tool, state and
arbitrary request JSON keep their own property semantics.
`t08_t12_t19_successful_tool_j_before_c_keeps_original_content_and_available_source`
passed in 0.152s at epoch 96 (`/tmp/467-new-focused96-0.log`): successful J precedes
result C, original opaque fields and range bytes survive, Source remains available,
and each range reads at most two finite rows/64 KiB. The companion
`t08_t12_t19_request_metadata_requires_the_original_message_producer_path`
passed on the linked epoch-99 binary in 0.066s
(`/tmp/467-final-focused99-1.log`). It distinguishes safe inline JSON from unsafe
opaque integers retained through exact selected bytes, rejects metadata inference
at both generic readers, and preserves positive intentional message bindings and
frontiers. The successful Tool J-before-C proof also passed again in 0.090s
(`/tmp/467-final-focused99-0.log`).

Pending adoption now isolates only derived `pending` segment/frontier cleanup in
the existing optional savepoint. Original Pending deletion, canonical C,
`InboundTurnAdopted` J, transcript identity, pending count and mutation revision
remain owned by the native adoption transaction.
`t09_t12_t19_pending_layout_cleanup_failure_cannot_refuse_native_adoption`
passed in 0.075s on epoch 99 (`/tmp/467-final-focused99-2.log`). Actual SQLite
DELETE triggers separately fail segment and frontier cleanup: native adoption and
healthy Source receipts still commit, while the retained derived rows cannot be
read using an expired cut, the removed ordinal under a fresh cut, or a foreign
Conversation. A later real ordinal remains readable and unfaulted cleanup works.
All five Source cases are recorded in `/tmp/467-final-focused99-results.json`;
these focused passes leave full-suite and browser validation pending.

The actual stdio delivery reproduction passed 1/1 with no skips in 81.118s against
the default epoch-95 binary (`/tmp/467-tui-delivery96.log`). Exact command, from
`tui/`:

```sh
RUSTX_BINARY=/home/capi/Documents/codes/rustX-issue-467/target/debug/rustx RUSTX_REQUIRE_PROVIDER_EMULATOR=1 node --test --test-name-pattern='lists, renders, saves byte-exactly' test/delivery-integration.test.ts > /tmp/467-tui-delivery96.log 2>&1
```

This selected result does not establish full TUI, browser or final-suite success.

All three exact tests in
`durable::sqlite::tests::request_probe_tests` passed on the linked epoch-86 binary
(`/tmp/467-final-focused86-results.json`):

| Exact test | Executed result and evidence |
| --- | --- |
| `t14_t19_t20_native_request_probes_skip_unrelated_giant_original_journal_rows` | Passed in 2.346s. Sixteen genuinely appended unrelated original events each have an identity larger than 1 MiB; indexed VM work remains unchanged, only the two selected small original envelopes are materialized, absent requests load no rows, and the native cut is unchanged. |
| `t14_t19_native_request_probe_rejects_duplicate_original_starts_and_owner_mismatch` | Passed in 0.054s. Deliberately corrupted original selected ownership is rejected; two original starts still fail as duplicate authority. |
| `t14_t19_native_request_outcome_probe_keeps_earliest_original_success_or_failure` | Passed in 0.609s. Both success-before-failure and failure-before-success retain the earliest original Journal outcome. |

Canonical, Audit and Live references remain the three closed public content
sources. A public read checks its published release evidence or captured native
cut before the Store reads exact source identity, representation/version and
frontier in one transaction. Ordinary segmented ranges use SQL slices from at
most two 65,536-byte rows; at most 65,536 decoded bytes are returned. Live block
metadata uses at most two finite metadata rows, with the selected slice copied
into the same bounded reply. Positioned binary reads retain the original managed
artifact lock and length authority. These reads do not stage, flush or settle.
Selected delivery metadata likewise retains the original successful Tool C,
numeric Ledger position, delivery ordinal and native cut; the protected file
action separately applies its original scope, filesystem and Delivery Access
authorization. Reference caches retain finite evidence, not body leases or an
independent source frontier.

Standalone Trace pages retain their own complete native `read_cut`; cached rows
keep the real installation cut through unrelated publications and Replay eviction.
The currently published Trace cut remains pinned by that existing cache owner.
An embedded page may inherit its outer checkpoint/event cut only when it represents
that exact cut; a retained older head carries its original explicit cut. Closed Value/Content field selectors preserve
original J/C identity and outcome evidence; lifecycle replacement cannot discard
immutable request fields.

Selected non-Request detail now dispatches on its exact original Journal anchor
and returns `TraceInspectionHead`, not a partial `TraceDetail`. Tool heads retain
proposal/result fields, canonical occurrence and artifacts; User/Context,
Assistant and Compaction heads retain original canonical locators and block
frontiers. Adoption membership advances beyond the eight-item prefix in original
ordinal order. Existing numeric-L block/tool pages use the same captured cut
through Root, historical Session and authenticated cold Child routes. No reader
recaptures a later Ledger frontier or creates a Runtime cursor.

Tool Schema is selected only through the exact accepted Assistant C/J and its
original frozen RequestStart/tool ordinal. Original provisional identity and
native Attempt/Step establish that relation at the existing commit boundary;
fixed Tool identity digests validate the selected original ordinal at read time.
Neither the latest request, reused wire call ID nor the current Tool registry can
supply a definition. Available, proved absent and unavailable are distinct typed
states. Catalog/relation/body-layout failures are isolated in optional savepoints,
including the existing accepted-request join, so they cannot refuse native J/C
or hide independent Assistant/Tool facts. Original schema bytes use the existing
Request field Page/Value index; payload/result bytes and protected artifacts keep
their existing authorities. All 14 named native/Source/real-Child proofs below
passed at epoch 108; current full-suite and new client execution remain separate
gates.

Refresh batches admit at most eight interests while
the loaded cache remains bounded. Successful canonical Tool results alone own
the delivery head; original collection indices can advance beyond its eight-item
prefix. Selected metadata and protected file reads preserve original grant,
catalog and filesystem authorization independently of execution settlement.

Workflow subdomains select the existing `workflows` source owner; a path such
as `workflows.runs` does not create another source handle. The
`t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut`
regression checks that owner and subdomain resolve the same native source,
Page/Value retain the original captured cut and run, and an actual subsequent
Workflow revision still expires the old mutable-domain cut. This fixes an absent
owner-key mapping; no expiry retry or new-cut stamping conceals it. The exact
regression passed at epoch 86 in 0.116s
(`/tmp/467-final-focused86-3.log`).

The existing native Workflow publication owner releases preceding read-model
revisions after every successfully appended Workflow observability fact, using
that original committed Journal sequence. The former start/terminal-only
restriction hid intermediate child and checker state until the outer Tool
settled. A failed append still releases nothing: no callback, original Journal
or native cut changes. The exact regression
`t10_t14_failed_workflow_append_does_not_release_uncommitted_source_revisions`
passed at epoch 99 in 1.712s (`/tmp/467-final-focused99-6.log`).

Native Workflow revisions remain distinct from Runtime cursors. A complete
source-backed replacement may coalesce native revisions while advancing exactly
one Runtime cursor; it retains replay and initial-cut evidence instead of
clearing the newly published record. The
`t14_t20_workflow_native_revision_gaps_preserve_complete_replay_and_checkpoint`
regression passed at epoch 99 in 0.092s (`/tmp/467-final-focused99-7.log`), checking
native revisions 37 then 41, contiguous replay and whole-checkpoint equivalence
including all event metadata and frontiers. An older mutable Workflow domain cut
still expires after a genuine new revision.

The full external conformance target passed 26/26 at epoch 99 in 105.40s
(`/tmp/467-conformance99.log`), including all nine Workflow cases. At the held
second-writer provider gate, the tampering case waits for the actual native
revision to appear in event metadata, captures one checkpoint, and verifies its
exact Page/Value source plus original Journal facts: two implementation
admissions, one completed first checker and no Workflow terminal. Physical gates,
request counts and deadlines remain unchanged. At epoch 99, full Rust, TUI and
browser checks remained separate pending gates; later execution is recorded below.

Cold Child Trace reads retain the immutable historical Source tuple captured by
TraceProjection. Its revision-zero cut is distinct from a durable Host's
publication/cache cut. The Registry authenticates the original Child CID and
Activation; selected Trace, Journal, context, occupancy, identity and delivery
fields/pages read the original Store at that same historical cut. The wrapper
removes only its authenticated Activation for the native historical read and
restores it on the reply after verifying that CID/Activation still match the
Registry. The pinned epoch-105 durable checkpoint path applies that same
authenticated binding to the checkpoint's retained embedded Trace head before
returning the outer checkpoint. It preserves the Trace head's original
Source/revision/J tuple, rather than borrowing the outer checkpoint's newer
publication cut. The extended exact T18 selector is
`boundary_suites::subagent::conformance::observation::t18_live_durable_only_and_reactivation_preserve_source_and_execution_identity`.
It reads the real embedded terminal result and request detail at the original
A0 cut, verifies unchanged native source state, then checks that real A1
reactivation rejects that retained A0 selection. This extended exact T18 case passed at 105 in 64.606s
(`/tmp/467-focused105-2.log`). Active/Stopping Child reads continue through the
actual live Peer and its native cut. No historical reader activates a node, adopts a newer Source cut
or infers terminal settlement from observation absence. The genuine Child
continuation case, including original A0/A1 reads, mutation-cut expiry, frozen
history and recovered resume, passed at epoch 89 in 5.07s
(`/tmp/467-continuation89.log`).

Recovered Registry construction now installs the existing selected-source layout
cache alongside the Agent's native index and activation records under the same
Registry lock. This closes the missing-cache panic before the recovered Agent's
first publication without adding a fallback, cursor or execution transition.
`t15_t20_recovered_agent_installs_its_selected_source_before_any_publication`
passed at epoch 90 in 0.063s (`/tmp/467-new-focused90-2.log`), proving immediate
selected-source reads, unchanged original Journal/cut, and the same cache owner
before any republish.

Production transcript decoration requires the page's original Source cut and
uses only bounded numeric selectors. The former exported cutless decorator and
its inherited lookup graph are removed from production. Explicitly named
`cfg(test)` native oracles retain full original-history comparisons. The
`cutless_response_decoration_is_rejected_without_changing_native_page` regression
requires both production decorators to reject an absent cut before any mutation.

## Client cut validation and executed evidence

The epoch-104 deferred Trace path retains a positive generated `CheckpointPage`
in the existing `TraceDetailEntry`; it never casts that header into a complete
`TraceDetail`. Root, Child and historical readers use the page's own returned
cut. The wrapper checks exact owner, cache epoch and retained page identity
before and after reads. Root checkpoint supersession evicts deferred pages and
inline entries with content bindings whose readers capture the retired authority;
complete inline bytes without those bindings remain cached. The existing UTF-8
cache weight includes the retained page, and eviction changes no native fact.

Options, prompt, Tool catalog and Diff use fixed selected request fields and
existing Page/Value methods. Selected pages must match domain, cut,
`cursor == read_cut.revision`, contiguous original offsets and continuation for
these request-field pages. Canonical `.tools` pages separately preserve sparse
original block coordinates, while `.blocks` pages are contiguous. The
first original byte reply fixes H; later windows preserve H, representation and
source. A streaming UTF-8 decoder retains split character bytes, and original JSON
windows are never parsed/reencoded or rounded. The original predecessor prompt
is selected only through an exact native predecessor ID/shared-source match to a
loaded request. A missing loaded predecessor offers the existing earlier-history
gesture, rather than asserting native absence or inventing a nearby request.

`web-console/test/trace-deferred-detail.test.tsx` has 15 cases covering these
boundaries, positive deferred evidence versus unavailable errors, foreign cuts,
late Root/Child/historical replies, field/page replacement, both Root inline
retention cases, malformed cursor and exact unsafe-u64/UTF-8 bytes. The focused
six-suite run passed 159 cases before the final cursor case, then the final
15-case file and fresh typecheck passed
(`/tmp/467-source-trace104-focused.log`,
`/tmp/467-source-trace104-cursor-focused.log`,
`/tmp/467-source-trace104-cursor-typecheck.log`). These are scripted-client/UI
proofs; actual Composer and historical Diff browser reruns are still required.

Root checkpoint installation validates the returned snapshot and read-cut CID
against the exact attachment target, requires no Child Activation, and requires
checkpoint Cursor to equal native cut revision. Present Source evidence must
retain that CID, the cut's Journal frontier, and its duplicated Ledger, Surface
and transcript-count coordinates. An absent optional Source cut remains absent;
validation does not invent one. A repair older than locally Applied is rejected
before replacing either presentation state or its checkpoint.

Root event handling performs the same checks before folding scope-bearing
`checkpoint_advanced` and `projection_transition` carriers, including nested
carriers. Matching outer Target and Registration cannot authorize a foreign inner
cut. Child handling requires its actual CID/Activation and each record's own
Cursor/revision; the batch's final Cursor is not substituted for intermediate
records. Checkpoint source evidence is checked before replacing the Child
baseline. Leaf facts acquire no invented Cursor or identity. Both clients retain
prior state and Applied when a cut is refused. Root recovery remains bounded;
the Child watch reports observation unavailability and explicit Transcript repair
can establish a new checkpoint before following resumes. No guard fabricates an
execution outcome or retries a mutation.

Root release also retires the TUI observation chain immediately. Both initial
attachment and repair check the exact acquisition epoch, release/closed state
and chain after the Subscribe await, before accepting a receipt or folding its
buffered events. Deterministic tests deliver a valid late receipt after Detach
or node switch, for both acquisition paths, and retain the previous state,
checkpoint and Applied baseline. An immediate release from the prepared-attach
callback starts no subscription or later configuration acquisition.

The deterministic client tests exercise correlated responses and notifications
through the actual client handlers over scripted transports. They cover foreign
snapshot/read-cut/Source CID, unexpected Child Activation, stale revisions,
checkpoint/event Cursor mismatch, inconsistent Source coordinates, nested invalid
carriers, valid repair and contiguous following. A positive two-record Child batch
checks its distinct intermediate cuts. The rendered AgentCard test refuses a
same-CID/Activation stale inner revision before rendering its message, performs
explicit native-checkpoint repair, then follows the next Cursor without execution
control calls. These are client contract proofs, not new native Socket evidence.

Authored fixtures were corrected to supply their actual Session CID and complete
transition metadata: reconnect no longer advances Cursor while retaining revision
zero, repair snapshots no longer borrow another fixture Conversation, and the
Child transcript continuation now constructs its checkpoint and record at Cursor
one together. Existing identity, continuity, no-replay, cancellation, DurableOnly,
file-byte and provider-count assertions remain unchanged.

| Executed client command | Result and evidence |
| --- | --- |
| `node --test test/subagent-transcript.test.ts test/bounded-observation.test.ts test/root-checkpoint.test.ts` from `tui` | 52 passed, no failures or skips; `/tmp/467-tui-pure-observation96.log`. This includes the formerly blocked Child continuation fixture. |
| `pnpm exec vitest run test/root-checkpoint.test.ts test/child-observation.test.ts` from `web-console` | 22 passed across two files; `/tmp/467-web-pure-observation96.log`. |
| Earlier focused actual-client/AgentCard checks | Web 48 passed and TUI 40 passed; `/tmp/467-web-child-cut-focused90-2.log` and `/tmp/467-tui-child-cut-focused90-2.log`. All four lightweight client typechecks passed for that pin. |
| `node --test test/app-server-client.test.ts test/reconnect-observation.test.ts test/session-focus.test.ts test/root-checkpoint.test.ts` from `tui` | 100 passed, no failures or skips at epoch 97; `/tmp/467-tui-late-receipt97-focused.log`. Includes four late-receipt release cases and immediate prepared release. TUI typecheck also passed (`/tmp/467-tui-late-receipt97-typecheck.log`). |
| Full TUI epoch 93 | 1,100 passed, 7 failed, 1 existing skip; `/tmp/467-final-tui93-client-full.log`. Six real delivery/Child/interrupt failures still required Source diagnostics; one pure Child continuation was stopped after its invalid authored cut prevented the expected next request. The corrected pure case passed in the 52-test run above. |

The epoch-102 status below supersedes the earlier client failure counts; final
client/native safety nets and complete browser validation remain outstanding. The
fresh-bin Stdio delivery repro at epoch 96 is separate evidence: its isolated
log reports one pass and no failure (`/tmp/467-tui-delivery96.log`). That result
does not establish the remote delivery or full-TUI safety net. Failure-only
diagnostics preserve original native record/cut evidence and the owned fixture
Store for finite Source inspection; they do not issue repair reads or change the
original wait/assertions.

Source inspection identified the real native failure in retained Journal 8:
`observation_request::page_from` and `field_header_at` interpreted arbitrary
original properties named `content`/`collections` as display bindings/frontiers.
The successful original Tool result contained Text content, so this optional
indexing attempt failed while original execution continued through canonical C3.
Metadata decoding now requires the actual `request_observation_fields`
`messages/<native ordinal>` producer path; ordinary Tool/state/schema properties
remain original values. The focused
`t08_t12_t19_successful_tool_j_before_c_keeps_original_content_and_available_source`
regression passed at epoch 96 (0.152s). The related metadata-origin fixture and successful-Tool regression both passed
again among the eleven focused epoch-102 proofs. Complete native/client reruns
remain separate gates.

### Accepted output during queue failure

Every enqueue/encoding failure in the common transport serve loop now retires
the carrier before dropping unaccepted work, then joins its existing writer.
An already accepted Stdio record retains its exact remainder and LF. A physical
write failure or the unchanged ten-second writer deadline takes precedence over
the enqueue error; the enqueue error is returned if the accepted record finishes.
This adds no queue, execution cancellation or new physical timeout.

The real OS-pipe regression
`t17_t20_stdio_queue_overflow_finishes_the_physically_accepted_record` passed on
epoch 100 in 1.19s (`/tmp/467-pipe-overflow100.log`). The original pipe accepted
one byte; its incomplete prefix could not parse as a response. A one-shot gate
held the remainder while 33 further bounded replies filled the 32-record queue
and caused overflow. The connection retired while the writer remained owned;
releasing the gate produced exactly the original ParseError JSON and one LF,
then physical EOF, with no queued successor record or provider request.

The same test previously failed its unchanged 30-second fixture liveness guard
at epoch 99 (`/tmp/467-final-focused99-8.log`). That run had no stage diagnostics,
so the blocked await remains unlocalized. The only intervening test change added
finite stage prints; it did not alter a gate, assertion, timeout or production
path. Printing can change scheduling and does not establish why the earlier run
failed. The successful epoch-100 trace proves this executed interleaving; it is
not a claim that the earlier liveness failure has an identified cause. Keep the
stage evidence and require the complete safety net separately.

## Deterministic owner barriers

The existing M9b cancellation-versus-start boundary has two `cfg(test)` pauses:
`pre_start` before arbitration, and `pre_commit` inside the original start gate
immediately before the durable commit. `PhasePause::park` enters
`tokio::task::block_in_place` **before** announcing its ordinal and waiting on its
existing synchronous release channel. This hands off the worker so a control task
in its unstealable LIFO slot can progress; both original arbitration points and
their lock ownership stay intact. It introduces no production execution owner,
clock change or altered commit boundary.

Other race proofs retain the original native owner gates, physical handoff
receipts, exact request/registration tokens and explicit worker/listener exit.
Execution waiters capture their existing native handoff waiter or receipt ordinal
before admission and then require the same Attempt in an original fact or
authoritative replacement checkpoint. Predicates inspect complete records at their actual native cursor;
fixtures do not create synthetic fact-cloned event records.

The exclusive `LocalConversationRuntime::shutdown(&mut self)` owner boundary
first awaits the existing native runtime drain, then cancels presentation reads,
closes the observation queue and joins its worker and finite in-flight Source
read. The same worker slot retains its JoinHandle while awaiting completion;
cancelling a shutdown waiter releases its mutex guard and a retry still joins
that original worker. Storage remains owned until the composition and its handles
are dropped.
The headless owner has no presentation worker and awaits only native shutdown.
`LocalConversationShutdownError` keeps native settlement failure separate from
the finite cancelled/panicked projection-join classification; a failed downstream
join does not relabel an already-quiescent execution as `Draining`.

Child terminal cleanup closes the existing live inspection server before awaiting
this composition-owner boundary. A cleanup failure produces a bounded diagnostic
and returns the original semantic result unchanged, including an already-accepted
Result frame. Connection close, attachment detach and observation cancellation do
not invoke this owner shutdown.

The updated external durable requirements 05, 06 and 14 exercise detach/reattach,
headless-to-interactive reopen and resource-refresh reopen through that explicit
owner boundary. Requirement 12 subscribes to the original native activation's
Eligible publication before capturing its unchanged-cursor paging baseline. The
pending-mutation isolation fixture uses a genuinely absent foreign occurrence,
then proves the other Store's real selected occurrence succeeds without changing
the first Store's bytes. These descriptions are ownership/proof contracts. The
complete durable target passed again at 86-2 (147/147,
`/tmp/467-external86-2.log`); later fixture pins still require a fresh safety net. The
existing `blocked_read_materialization_does_not_own_shutdown_or_projection_drain`
test independently exercises the underlying cancelled-read/worker-join seam.
The new `projection_drain_retry_retains_the_original_worker_after_waiter_cancellation`
proof gates that actual Source task after cancellation, drops the first pending
join waiter, verifies the retry remains pending, then releases and joins the same
worker. It passed again in the focused epoch-86 run (27.743s); the existing blocked
read/physical-drain proof also passed (1.112s). The explicit release gate, rather
than elapsed time, establishes both orderings. These results are recorded in
`/tmp/467-final-focused86-results.json` and do not establish a full library pass.

## Deterministic RFC matrix

Tests remain in their owning suites under `tests/README.md`. Fully qualified Rust
selectors below come from captured native discovery; `publication::` selects the
external `--test durable` target, and the CLI EOF case selects `--test process`.
Other qualified Rust selectors select `--lib`. File paths identify JavaScript
consumer suites, not Rust discovery. Names below identify
the contract proofs; a real boundary is exercised where the invariant concerns
Stdio, WebSocket, Unix inspection, SQLite or a Child process. Wall-clock deadlines
are liveness guards, never race evidence.

| RFC | Implemented tests / consumers | Invariants | Synchronization or real boundary |
| --- | --- | --- | --- |
| T01 | `app_server::transport::tests::t01_complete_envelope_exact_byte_limit_and_one_byte_overflow`, `app_server::protocol::lifecycle_protocol_tests::t01_request_id_budget_counts_encoded_utf8_and_escaping`, `local_runtime::session_runtime_manager::tests::protocol::t01_invalid_identity_precedes_method_and_connection_admission`, bounded response substitution and typed cancellation tests | 02, 03, 14 | Actual capped serializer at 1,048,576 bytes and one-byte overflow; domain/admission counters remain unchanged on rejected identities. |
| T02 | `local_runtime::session_runtime_manager::tests::protocol::child_delivery::bounded_observation::t02_t03_large_child_observation_keeps_parent_websocket_and_other_session_following` | 01, 02, 13, 14, 16, 17 | Actual Child process + Unix inspection + authenticated TCP WebSocket; provider gate holds original Child execution while oversized observation is read. |
| T03 | The exact T02/T03 selector above; `local_runtime::session_runtime_manager::tests::transports::t16_pending_receipt_supersession_and_stale_queue_progression` | 01, 04, 07, 14 | Independent Session starts and reaches native terminal events on the same carrier; pending Child pages advance 2 → 3 at the same cut. |
| T04 | `boundary_suites::runtime_client::observation::t04_native_unix_initialize_and_checkpoint_never_carry_an_eight_mib_snapshot` (`tests/boundary/runtime_client/observation.rs`) | 02, 04, 14, 16 | Actual Unix peer initialization carries only bounded identity/capabilities; explicit checkpoint reads replace the old full snapshot. |
| T05 | `protocol/app-server/observation.ts` ObservationChain, `tui/test/reconnect-observation.test.ts`, `local_runtime::session_runtime_manager::tests::transports::t17_partial_event_disconnect_replays_from_locally_applied_cursor` (`tests/scripted/app_server/transports.rs`), Child observation/hide/reopen tests | 05, 06, 08, 14, 17 | Exact native cursor identity, contiguous fold and locally Applied reconnect cursor; deferred content is unnecessary for advancement. |
| T06 | `runtime_client::projection::tests::t06_t14_oversized_transition_is_one_foldable_checkpoint_and_converges`, `runtime_client::host::tests::t06_t14_many_live_blocks_keep_every_descriptor_after_inline_budget_exhaustion`, real WebSocket oversized-notification recovery in `tests/scripted/app_server/oversized_notification.rs` | 02, 05, 06, 16 | Native publication constructs one complete bounded transition. A deterministic transport-budget fault fences the affected registration; the independent checkpoint includes the original native canonical commitment and a fresh subscription resumes contiguously while held execution and a sibling Session continue. |
| T07 | `local_runtime::session_runtime_manager::tests::transports::t07_oversized_controlled_response_keeps_publication_owner_at_websocket_handoff`, existing Delivery Access acceptance/cancel/revoke tests | 01, 07, 15 | Actual final tungstenite handoff; original publication owner arbitrates substituted errors and exactly one accepted response. |
| T08 | `publication::t08_large_canonical_content_exact_byte_chunks_survive_restart` (`--test durable`), `boundary_suites::durable::content::t08_t20_content_range_reads_touch_at_most_two_finite_sqlite_rows`, `boundary_suites::durable::content::t08_t20_request_source_ranges_use_bounded_authenticated_composed_pages`, `boundary_suites::durable::content::t08_t12_t19_binary_canonical_artifact_ranges_are_frozen_owned_and_restartable`, `durable::sqlite::tests::t08_t14_t19_agent_statistics_use_fixed_headers_with_giant_original_bodies`, `runtime_client::host::tests::pure_read_tests::t08_t14_t19_pages_and_value_reads_leave_native_progress_queued`; `durable::sqlite::tests::t08_t14_t15_t19_trace_headers_and_scalar_ranges_never_read_growing_original_bodies` | 02, 10, 11, 12, 14, 16 | Ordinary segmented Text/raw/CanonicalJson ranges return ≤65,536 decoded bytes from ≤2 finite rows. The Request scalar range selector checks actual composed-page work: ≤160 page handoffs of ≤64 KiB, one ≤16 KiB root, no native-node traversal or header decoding for its scalar-only fixture, exact bytes and advancing chunks. General composed reads retain the separate ≤64-piece bound; the retired two-segment representation is not asserted. Binary uses frozen retained ManagedArtifactStore authority with positioned reads under its shared lock. Statistics read one ≤16 KiB original-J aggregate and no full event/request/message bodies. Exact bytes survive reopen. New finite-head selectors are listed in the [epoch-107 proof directory](#epoch-107-finite-inspection-proof-directory): original tiny/giant Tool payload, result and frozen Schema, canonical-kind dispatch and bounded Store-only source reads; all 14 additions passed in the exact epoch-108 run. |
| T09 | `boundary_suites::durable::content::t09_t12_pending_edits_never_alias_immutable_canonical_content`, `publication::t09_t12_t19_committed_live_prefix_fixed_identity_and_frontier`, `web-console/test/trace-deferred-detail.test.tsx`: exact selected-window extent/representation and late-owner rejection | 09, 10, 12, 15 | Native revision/stream replacement with old reads in flight; old references remain fixed or explicitly expire. |
| T10 | `app_server::t10_stdio_cli_eof_preserves_gated_attempt_and_original_canonical_commit` (`--test process`), `local_runtime::session_runtime_manager::tests::transports::adapter_disconnect_preserves_active_execution_and_replacement_authority`, `local_runtime::subagent_child::tests::t10_t18_t19_closed_observation_source_preserves_native_child_completion`, real Web/TUI parity | 01, 13, 17 | Provider/Tool gates and actual carrier/listener close; original incarnation, activation, request counts and settlement remain authoritative. |
| T11 | `boundary_suites::subagent::conformance::observation::t11_t12_t20_nested_pending_reads_follow_authenticated_direct_routes_over_two_unix_peers`, `web-console/test/bounded-observation-native.test.ts` (real disconnect and byte-exact WebUI/TUI recovery), `tui/test/integration.test.ts` (same operations/projection over Stdio and WebSocket), `local_runtime::session_runtime_manager::tests::transports::t11_t15_effective_configuration_ranges_survive_model_commit_on_real_websocket` | 05, 08, 10, 13, 17 | Actual Root/Middle/Leaf Unix routing and client carriers; many bounded configuration chunks preserve unrelated held execution and carrier progress. The real Child live/cold finite-head selector and native large-J identity proof in the [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) cover shared native routes. All 14 directory proofs passed at 108. The four real TUI Root Delivery/Child Stdio/WebSocket cases passed at 111 with zero skips. The final 113 Child cases additionally select the actual canonical Tool result collection and reconstruct its original questionnaire bytes; browser114 Chat separately validates the advertised image block and original binary bytes, preview and reconnect. |
| T12 | Actual Unix source-substitution tests, `boundary_suites::durable::content::t12_t14_t19_source_cut_provenance_rejects_old_journal_with_new_canonical_frontier`, `boundary_suites::durable::content::t08_t12_canonical_locator_namespaces_preserve_literal_ids_and_full_inline_id_budget`, binary retention and protected Delivery tests | 09, 10, 12, 15 | Server validates native ownership, representation, cut, revision and frontier; modified references grant no new authority. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds exact original retry/catalog ownership, foreign Child Activation rejection, wrong real RequestStart/ordinal and missing/corrupt-layout negatives; all 14 directory proofs passed at 108. |
| T13 | `local_runtime::session_runtime_manager::tests::protocol::t13_attach_identity_checkpoint_and_subscribe_preserve_the_native_cut`, `local_runtime::session_runtime_manager::tests::protocol::t13_lost_huge_root_attach_receipt_is_reconciled_without_second_admission`, giant catalog selection and immutable editor/origin tests | 03, 04, 14, 17 | Bounded identity/revision receipts, same-connection AttachmentStatus after an unaccepted receipt and independent checkpoint over a 2.4 MiB retained message; no uncertain mutation replay. |
| T14 | `web-console/test/incremental-equivalence.test.ts`, `runtime_client::host::tests::incremental_projection_independent_snapshot_capture`, `runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture`, `runtime_client::projection::tests::t14_t20_retained_cut_lookup_uses_exact_metadata_and_bounded_cursor_index`, `runtime_client::projection::tests::t14_native_admission_publishes_frozen_model_before_execution_starts`, `runtime_client::projection::tests::t14_t19_worker_read_fence_keeps_one_native_cut_before_publication`, `boundary_suites::durable::content::t08_t14_t19_response_decoration_keeps_one_frozen_native_journal_cut`, `runtime_client::projection::tests::t14_t19_complete_canonical_head_survives_an_earlier_delayed_callback` | 05, 06, 08, 10, 11, 12 | Strict structural fold equivalence at real native cuts; held C1/C2, empty block frontier, C-before-J and Audit/terminal facts remain distinct. |
| T15 | `boundary_suites::durable::content::t15_t20_message_and_transcript_headers_advance_without_source_body_reads`, `boundary_suites::durable::content::t08_t15_t20_missing_middle_descriptor_never_skips_native_history`, `runtime_client::host::tests::owner_tests::t08_t12_t14_t19_cold_giant_turn_page_navigates_by_native_ordinal`, Source/block/journal/settings/model/editor/page suites, `runtime_client::projection::tests::t15_many_small_message_pages_advance_under_both_native_budgets`, `runtime_client::projection::tests::t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut`, collection page clients | 02, 06, 10, 14, 16 | Indexed native ordinals and exact cut/revision continuation; item count and aggregate encoded bytes both bound pages; giant items advance through finite descriptors. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds 19-member User/Context inspection paging beyond eight and Source selected-member validation, with original order and advancing continuation; all 14 directory proofs passed at 108. |
| T16 | `local_runtime::session_runtime_manager::tests::transports::t16_pending_receipt_supersession_and_stale_queue_progression`, `local_runtime::session_runtime_manager::tests::transports::t16_physical_acceptance_wins_before_replacement_no_lock_cycle`, `local_runtime::session_runtime_manager::tests::transports::t16_pending_physical_pipe_replacement_advances_without_readiness`, `runtime_client::host::tests::resubscription_is_superseded_before_local_handle_publication` (Host); transport proofs in `tests/scripted/app_server/transports.rs` | 01, 07, 14 | Real full OS pipe and tungstenite Frame handoff; hooks gate before/inside acceptance with valid winners, Pending revalidation, RecoveryRequired and Discarded progression. |
| T17 | `local_runtime::session_runtime_manager::tests::transports::t17_partial_event_disconnect_replays_from_locally_applied_cursor`, `local_runtime::session_runtime_manager::tests::transports::t17_partial_os_pipe_record_finishes_after_registration_replacement`, `local_runtime::session_runtime_manager::tests::transports::t17_stdio_input_eof_finishes_the_physically_accepted_output_record`, `local_runtime::session_runtime_manager::tests::transports::t17_stdio_teardown_retires_unaccepted_record_without_write_readiness`, `local_runtime::session_runtime_manager::tests::transports::t17_t20_stdio_saturation_deadline_finishes_the_physically_accepted_record`, late TUI Subscribe receipt release tests, `local_runtime::session_runtime_manager::tests::transports::t16_t17_websocket_frame_handoff_precedes_physical_write_failure` (same transport suite) | 07, 08, 17 | Actual partial OS-pipe writes and EOF; incomplete parse never advances Applied, first positive write finishes its record, later Flush is not acceptance. EOF/shutdown discards a zero-acceptance queued record even when physical write readiness remains blocked. |
| T18 | `boundary_suites::subagent::conformance::observation::t18_live_durable_only_and_reactivation_preserve_source_and_execution_identity`, actual Unix old-activation initialization, closed inspection listener tests | 01, 10, 13, 17 | Live → DurableOnly → new Activation; retained content remains readable and observation absence never fabricates execution completion. |
| T19 | `scripted_suites::agent::publication::no_frame_is_released_before_its_staging_commit`, `runtime_client::host::tests::t19_staged_frame_is_projected_before_observer_returns_to_release`, `runtime_client::projection::tests::t14_t19_prepared_started_witness_preserves_exact_live_progress_and_terminal_facts`, `publication::t19_staging_receipts_pin_each_committed_frame_without_later_source_reads`, `publication::t19_raw_tool_json_retains_original_bytes_after_canonical_commit`, full durable Publication suite, binary/UTF-8 split proofs; separate original-source expiry and native event-fold proofs (combined expired-ref/Replay proof at 105 below) | 09, 10, 11, 12, 15 | Frame staging commit and release-authorized evidence before observation; parked Audit sink and failed staging cannot expose new bytes or be repaired by reads. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds C-without-J frozen inspection, original giant schema/payload ranges, independent optional-layout failures and native J identity distinct from safe byte coordinates; all 14 directory proofs passed at 108. |
| T20 | `local_runtime::session_runtime_manager::tests::protocol::child_delivery::control_intent::t14_t20_two_execution_intents_bypass_six_waits_and_eight_unpolled_observations`, `local_runtime::session_runtime_manager::tests::protocol::child_delivery::control_intent::reconnect::t14_t20_reaped_websockets_preserve_global_control_reserves_over_native_waits`, `app_server::admission::tests::t20_admission_cancellation_deduplication_and_exact_cleanup`, `local_runtime::session_runtime_manager::tests::protocol::t20_observation_cancel_after_admission_before_first_poll_never_dispatches`, `runtime_client::agent_observation::tests::t20_real_unix_initialize_and_checkpoint_share_one_absolute_deadline`, `runtime_client::agent_observation::tests::t20_cancelled_physical_sql_worker_retains_inspection_capacity_until_exit`, `boundary_suites::durable::content::t08_t19_t20_canonical_json_orders_unbounded_keys_by_original_utf8`, `durable::sqlite::tests::t14_t19_t20_transcript_source_prefix_recovers_after_4097_later_owner_releases`, `app_server::transport::tests::t20_complete_record_candidate_capacity_obeys_the_physical_byte_budget`, checkpoint encoder capacity and fragmented Stdio input reservation proofs, `boundary_suites::subagent::conformance::observation::t20_real_unix_listener_bounds_and_reaps_its_physical_workset`, `runtime_client::projection::tests::t20_replay_byte_budget_covers_every_event_and_evicts_in_cursor_order`, predecode client guards | 01, 03, 04, 07, 08, 14, 17 | Actual reaped TCP WebSocket connections leave twelve physical waits resident; two fresh waits fill the ordinary ceiling14, while two original-owner control intent receipts are physically received before terminal release. One shared Host counter preserves14/16 reserves. Manual clock conserves one 15-second deadline; blocked physical SQL retains permits; actual listeners reap JoinSet tasks; finite encoded/decoded worksets and exact token fencing. |

## Integrated boundary coverage

The N/B/C labels refer to the integrated boundaries in
[invariants](invariants.md#bounded-conversation-observation-issue-467-rfc-v3),
not additional owners or alternate protocols. Every INV-01–17 appears in the RFC
matrix above. The table identifies implementation and proof ownership; discovery
is not execution evidence.

| Boundary | Existing implementation owner | RFC / invariant cross-reference and proof location |
| --- | --- | --- |
| N1 / B1 | `app_server/admission.rs`, Connection associations | T01/T13/T20; INV-03/04/14/17. Exact admission token, duplicate/cancel/deadline and delayed cleanup tests; identity receipts in `tests/scripted/app_server/protocol.rs`. |
| N2 / B2 | `app_server/registration.rs`, common transport writer, native Host subscription | T07/T16/T17; INV-01/07/08/14/15. `tests/scripted/app_server/transports.rs` gates actual pipe acceptance and tungstenite Frame handoff; replacement and native subscription installation share the slot. |
| N3 / C1 | Runtime Client Projection, checkpoint capture, Store read domains | T06/T14/T15; INV-02/05/06/10/11/12/16. Independent Host/canonical captures, exact retained-cut lookup, worker-fence and complete canonical replacement tests. |
| N4 / B3 / C2 | Publication staging/release and original Store ContentRef selectors | T08/T09/T12/T19; INV-09/10/11/12/15/16. `tests/durable/publication.rs` and `tests/boundary/durable/content.rs` retain distinct staged/released, Live/Audit/C and immutable/range evidence. |
| N5 / C3 / C4 | Registry, `runtime_client/agent_observation.rs`, authenticated Unix Peer, listener/physical task owners | T02/T03/T10/T11/T18/T20; INV-01/04/13/14/17. `tests/boundary/subagent/conformance/observation.rs` exercises direct nested routes, original Child CID/Activation and Live/DurableOnly; physical SQL and canceled drain proofs retain permits/JoinHandles until exit. No Parent full Child mirror is introduced. |

All 60 unique qualified library selectors in the T01–T20 matrix are present in
the epoch-111 library discovery (3,935 tests; `/tmp/467-discovery111-0.txt`).
All five named external selectors were also confirmed using `--list` on the
current compiled durable and process targets, including the CLI EOF proof in
`tests/process/app_server.rs`. Discovery establishes execution eligibility;
the scoped run records below separately establish which tests passed. These
are distinct test binaries.
The matrix also includes external process and JavaScript tests, which are not
members of library discovery. T11's real client parity is distinct from T14's
source-captured strict fold oracle. T16 covers both valid winners, zero-byte
Pending replacement, RecoveryRequired and stale queue progression; T17 separately
covers accepted remainders, EOF, overflow and post-handoff failure. These named
cases do not assert exhaustive exploration of all scheduler interleavings.

## Recorded checkpoint 102: incomplete historical evidence

Results below are scoped to their actual source/binary epoch. The epoch-102
`cargo test --all-targets --all-features` exited 101 after 3,907 passed, one failed
and five ignored library tests; the readiness-marker failure described above
stopped it before a complete external-target pass. Its command record is
`/tmp/467-final-validation102-native-full-results.json`. A new complete epoch-104
run has not been executed at this documentation pin. Root serializes native and
browser execution with `CARGO_INCREMENTAL=0`,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` and `RUST_TEST_THREADS=4`.

| Execution | Recorded result | Remaining scope |
| --- | --- | --- |
| Quality 102 | Diff, formatting, full Clippy, default bins and all-target/all-feature no-run compile all passed. `/tmp/467-final-validation102-quality-results.json`. | Compilation does not establish execution of all discovered tests. |
| Focused native 102 | All 11 exact proofs passed. `/tmp/467-focused102-results.json`. Includes general JSON-pointer semantics, exact integer/opaque JSON, bounded layout traversal and previews, original metadata producer paths, and both physical two-row SQLite range proofs. | No full-suite inference. |
| Independent captures 102 | Both native capture tests passed (1.440s / 0.309s) and regenerated the actual full-runtime and canonical fixtures. `/tmp/467-captures102-results.json`. | Strict all-field Node comparison is a separate consumer test; captures are not Socket traces. |
| Protocol 102 | Generation, staging the four generated artifacts and `protocol/app-server check` passed. `/tmp/467-protocol102-results.json`. | Final later-source drift must still be checked if source changes. |
| Static clients 101/101b | All four typechecks, dev 41/41, i18n, provenance (276 source records / 264 notices) and provider emulator Python 54/54 passed. The initial provenance mismatch was corrected and rerun. | Later client edits need their own current checks. |
| Full clients 102 | TUI 1,122 passed / one existing platform skip. Web 2,731 passed / one failed / four existing skips, 2,736 discovered. `/tmp/467-final-validation102-client-full-results.json`. | Web navigation/control-readiness repair is pinned with focused tests/typecheck; its full rerun is pending. TUI readiness follow-up is owned separately and does not turn this earlier pass into final-source evidence. |
| Full browser 100 | 274 passed / 43 failed / 317 discovered; artifacts preserved under `/tmp/467-browser100-results`. | Complete rerun after repairs remains required. |
| Affected browser 101 | 24 passed / two failed / 26 discovered. `/tmp/467-browser101-focused-results.json`, `/tmp/467-browser101-focused.log`. All 19 Settings cases, four model-error variants and the incremental 200-update / exact scroll assertions passed. | Both failures exhausted the unchanged 240-second setup budget while preparing 300 native attempts. This is not a passing reading gate. |

The epoch-102 traversal regression compares more than 10,000 original fields
against the independent legacy oracle and proves the repeated point-SELECT work
is removed. The schema lookup retains general JSON-pointer fallback; native
integer and opaque JSON proofs preserve exact wire semantics. These focused
results validate the narrow performance changes' bounds/equivalence, not their
ability to finish the normal browser workload. No optimized compiler profile,
reduced workload or increased timeout substitutes for the failed reading gate.

The two fresh capture tests are
`runtime_client::host::tests::incremental_projection_independent_snapshot_capture`
and
`runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture`.
Their consumer is `web-console/test/incremental-equivalence.test.ts`, which compares
all actual serialized fields, cuts, frontiers, C/J facts and native cursors against
independent Checkpoint(D); it only omits JavaScript `undefined` properties that do
not exist in JSON. Vocabulary checks remain separate. The current full-client
failure does not justify weakening those or any native assertions.

Concrete missing final evidence remains: a complete current all-target Rust
run; current full Web/TUI after the final client pins; the unchanged normal
300-attempt reading gate on the latest performance build; and a complete pinned
browser rerun after the epoch-100 repairs. Final command logs and any CI results
must be added from actual execution. Earlier scoped passes and implemented proof
names do not fill these gaps.

The integrated main remains `97096020d64ea00218493a0a99b9b805821eeb28`; the original
development base was `3c4b85a627e87383f244600e8b19887f103b82f0`. Independent
Compaction PR #477 is not an implementation dependency. No final commit, push or
PR success is claimed in this ledger.

### Browser 103: Chat prepend assertion scope (execution pending)

The affected browser run exposed a fixture assertion spanning two intentional
viewport gestures. Its Chat trace records the first bounded click growing anchor
rows from 12 to 60. Before the second click, the real conversation scrollport is
at 3,699px; Playwright scrolls the newly distant Load earlier button into view,
returning it to zero. That deliberate movement exactly matches the failed final
anchor delta. The second page still reaches 102 anchors, Answer 0 and no further
Load earlier button. Evidence is the `call@112` / `call@118` snapshots and action
logs in `web-console/test-results/chat-native-history-rich-s-bfa12--and-native-upload-receipts/trace.zip`.

The fixture now checks the unchanged <2px original-anchor invariant immediately
after the first real bounded prepend, before any subsequent click can move the
viewport. Its existing default assertion deadline allows ChatViewport's scheduled
animation-frame restoration after the DOM row count grows; no tolerance or
configured timeout changes. Remaining pages still require genuine clicks, and
all 34 turns, Answer 0, exhaustion and native/provider assertions remain. This is
a fixture pin, not a passing browser result; the completed epoch-103 affected
run sampled the previous fixture.


## Historical checkpoint 105: focused/client passes, final validation open

At epoch 104, diff/format, full all-target/all-feature Clippy, default bins and
all-target/all-feature **no-run** compilation passed
(`/tmp/467-final-validation104-quality-results.json`). Native library discovery
contains 3,918 cases. Neither that compile nor the focused/capture passes execute
the complete test set. Full `cargo test --all-targets --all-features` at this epoch
is still required; Root will record its result separately.

Full Web attempts 104 and 104b exited nonzero
(`/tmp/467-final-validation104-client-results.json`,
`/tmp/467-final-validation104b-client-results.json`): the Office converter deadline
and a native timeout remain failed run evidence, not skips or passes. The
serialized full epoch-104 rerun then passed **2,748 tests with four existing
skips / 2,752 total** in 377.826s, using
`pnpm --dir web-console test --maxWorkers=1`
(`/tmp/467-final-web104-serial-results.json`,
`/tmp/467-final-web104-serial.log`). It includes the final selected-page cursor
case; neither earlier failed attempt is relabeled. The new epoch-105 expired-reference consumer case was added afterward; this
104 pass does not cover that case. Its genuine regenerated capture and later
105b execution are recorded below.

The full epoch-100 browser run recorded 274 passed / 43 failed / 317 total.
Affected epoch 103 recorded 93 passed / five failed / 98 total
(`/tmp/467-browser103-results.json`, `/tmp/467-browser103-1.log`): Chat prepend
assertion scope, Composer Options availability, historical Diff availability and
both normal 300-attempt reading setups. The pinned UI/fixture/performance repairs
require actual epoch-104 browser execution. No screenshot, reduced workload,
longer timeout, optimized profile or focused mocked pass replaces that gate.

### Executed epoch-105 native and client checks

| Execution | Actual result and evidence | Scope |
| --- | --- | --- |
| Quality 105 | Diff, formatting, full all-target/all-feature Clippy, default bins and all-target/all-feature no-run compile passed; 3,920 library tests discovered. `/tmp/467-final-validation105-quality-results.json`. | This does not execute the complete library/external targets or CI lane selection. |
| Three exact native proofs 105 | Both schema-birth rollback/commit-count tests passed in 7.342s / 0.030s; the extended T18 embedded-Trace Activation/selected-read/reactivation test passed in 64.606s. `/tmp/467-focused105-results.json`. | Real native SQLite/Unix/Registry boundaries described above; no full-suite inference. |
| Genuine native captures 105 | `runtime_client::host::tests::incremental_projection_independent_snapshot_capture` passed 51.208s and `runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture` passed 0.550s. `/tmp/467-captures105-results.json`. | Fixtures were regenerated by actual native test output, including the original expired-LiveRef witness and fresh native Replay through D. |
| Full default Web 105b | 2,749 passed / four existing skips / 2,753 total, 35.770s. `/tmp/467-final-web105b-client.log`. | Includes the strict all-field capture consumers and new genuine expired-LiveRef/Replay pure-fold case. |
| Full default TUI 105b | 1,127 passed / one existing platform skip / 1,128 total, 36.407s. `/tmp/467-final-tui105b-client.log`. | Original byte, cancellation, registration and client fold assertions retained. |
| Web build 105b | Passed 5.700s; `/tmp/467-final-web-build105b-client.log`. | These three client/build commands are recorded in `/tmp/467-final-validation105b-client-results.json`; browser execution is separate. |

The earlier full default Web105 attempt failed only the real native parity
fixture's unchanged ten-second provider-gate wait. Its captured native state was
still **Admitted**, request count zero; the provider had not reached the execution
gate (`/tmp/467-final-web105-client.log`). The fixture now observes actual native
**Running** before starting that provider-gate wait. The original 120-second
case, ten-second provider wait and all semantic assertions remain unchanged; no
production code or timeout was relaxed. The later full105b pass does not erase
that earlier failed run.

### Default-disk browser105: failed evidence, separate from tmpfs106

The focused default-disk run remains incomplete in
`/tmp/467-browser105-1.log`. Chat failed its five-second native preparation
checkpoint/transcript poll before any `page.goto`; its old failure-only Inspector
lookup on a blank page then masked that original stage until the case deadline.
The diagnostic-only catch is corrected separately. Composer failed the
five-second `Plan recorded.` assertion while its original Attempt still showed
three running Tools. These observations do not establish a UI104 Trace defect
or prove execution had already completed.

The first normal 300-attempt reading preparation exhausted its unchanged
240-second budget. Read-only native sampling captured an actual worker in
`fsync → ext4_sync_file → rq_qos_throttle`, with host I/O pressure around
90% (`/tmp/467-reading105-first-fsync-stack.log`,
`/tmp/467-reading105-first-sample.json`). That identifies a concrete blocked
filesystem operation, not a completed reading/performance gate or proof that
every remaining delay has the same cause. These failed default-disk results are
not superseded by a successful disk performance claim.

The later complete epoch-106 diagnostic run used
`TMPDIR=/dev/shm/r4.tXR0dP` and finished **311 passed / six failed / 317 total**,
as recorded below. Both unchanged 300-attempt reading preparations and browser
cases passed there. Scenarios, counts, protocol/security assertions, 120-second
cases and 240-second reading preparation were unchanged. Native processes still
used real SQLite, managed files and registered paths on the host; only their
temporary filesystem changed. This establishes the recorded tmpfs execution,
not physical-media durability, default-disk latency or the default-disk reading
performance gate. Current full Rust and CI-lane execution remain separate gates.

### Proof closure and remaining validation gates

- **T19 / C2 integrated expired-LiveRef case:** the actual native Source-expiry
  proof is `publication::t09_t12_t19_committed_live_prefix_fixed_identity_and_frontier`;
  strict captured Checkpoint/Event folds are separate. The `expired-stream` case
  in `web-console/test/incremental-vocabulary.test.ts` is synthetic vocabulary
  coverage, not proof that a genuine expired LiveRef remains in native Replay
  while Applied reaches the following terminal/commit without a ContentRead.
  The existing
  `runtime_client::host::tests::incremental_projection_independent_snapshot_capture`
  is now extended to assert typed Host/SQLite expiry of that untouched nonempty
  reference, then subscribe from C only after expiry and export native Replay
  through D. The added exact consumer test is
  `T19 genuine expired LiveRef remains in native Replay and Applied reaches D without materialization`
  in `web-console/test/incremental-equivalence.test.ts`. It requires the genuine
  cfg-only capture witness and strict endpoint equality. The actual Rust capture
  passed at 105 (51.208s) and regenerated that witness; the pure consumer passed
  in the complete Web105b run. Thus this specific proof gap is closed by actual
  execution, without hand-editing fixture JSON, adding a content reader or
  hiding any native cut/frontier/settlement difference.
- **Historical epoch-108 execution:** the native capture, protocol and full
  Rust/CI-lane driver was still running at this checkpoint. Its failure and
  the complete repaired-source epoch-111 pass are recorded below.
  Epoch-108 diff/format/Clippy/no-run and all 14 exact finite-inspection proofs
  passed. Earlier full Web/TUI105b results remain scoped to that source epoch.
  The latest complete browser is the qualified tmpfs106 **311/6** result; its
  six repaired cases and the new native/client layer require fresh browser
  execution. Root owns the serialized final execution ledger.
- **CI/platform selection:** `tests/README.md` and the current CI lane checker
  distinguish in-crate `boundary_suites::` from external durable/process/subagent/
  tools/conformance/cfg3 targets. A successful safety-net run does not prove CI
  selection coverage. macOS discovery/platform tests and remote CI status cannot
  be inferred from this Linux workspace. Required Provider Emulator/uv and real
  binaries must fail closed; ignored measurements and existing platform skips
  must remain explicitly reported.

The earlier architecture/matrix review did not establish final product closure:
the subsequent real browser exposed the Trace kind-dispatch defect recorded
below. Its repair and the isolated optional accepted-request join passed the
14 exact epoch-108 proofs; older or focused suites do not establish full closure. This document makes no
complete/green, final commit/push or PR-success claim.

## Checkpoint 106 browser evidence and 107–108 repair scope

The complete unchanged 317-case browser command, `pnpm --dir web-console
 test:e2e`, finished with **311 passed / six failed** in 1,654.188 seconds
(`/tmp/467-final-validation106-browser-tmpfs-results.json`). Its temporary native
runtime, SQLite and workspaces used `TMPDIR=/dev/shm/r4.tXR0dP`; the pinned
Playwright image, provider workloads, screenshot tolerances and test deadlines
were unchanged. This is actual native/IPC/HTTP/browser evidence on tmpfs, **not
physical-media durability or default-disk latency evidence**. Both original
300-attempt reading preparations and their browser cases passed. The earlier
failed default-disk run remains a failed run.

The six failures identified these concrete boundaries. Repairs are pinned;
fresh complete browser execution is still required:

| Failure | Owner and correction |
| --- | --- |
| Chat Tool inspector | Shared native Trace detail dispatched every advertised kind through the Request-only reader. Non-Request kinds now select an explicit finite typed inspection head. Tool schema selection derives from the exact original frozen request and canonical Tool occurrence; raw payload/result readers and original artifact authority remain separate. |
| Child Inspect disclosure | Fixture opened the Tool before the original completed process-membership transition, which legitimately folds the process. Wait for that exact native membership, then perform the original reveal and Inspect gestures. Attempt terminal and membership are distinct facts. |
| Compaction questionnaire | The component retired a pending-interaction read on an unrelated checkpoint revision, then left its successor permanently loading. Ownership uses the actual pending domain revision, finite header/source and view identity; the original server cut is retained. Held-response tests cover unrelated checkpoint advancement and real Page/Value owner replacement. |
| DOCX preview | The assertion began while the actual Office conversion was still running. The fixture waits for its exact operation's complete successful response body before the unchanged DOM assertion; production conversion deadlines and cancellation/security tests are preserved. |
| Settings observation | The fixture gated an initial convergence read needed to enable its own field. Establish initial form readiness before arming the original post-write response gates; all write/read interleavings and no-replay assertions remain. |
| Historical Trajectory Diff | A conditional locator count skipped the explicit bounded read while detail admission was still pending. Wait for the captured original field selector, then perform the same reads and assertions. |

All 14 native finite-head, Store relationship and real-Child proofs passed at
epoch 108 (`/tmp/467-focused108-results.json`). Original-request catalog, Tool relation and adoption-member
layouts remain optional derived observation evidence. Their savepoint failure
cannot refuse native canonical or Journal commits. Public metadata reads use
fixed columns before growing IDs and bounded indexed seeks; no original giant
Message/Request deserialization is added. Missing or corrupt evidence is typed
unavailable rather than a fabricated absent schema or a different request.

The first epoch-107 quality run passed diff/format but failed Clippy on one new
range tuple call, one unchecked platform-width conversion and two function-size
lint violations. Those fixes and the optional missing-request-body-layout
isolation are saved. **107c Clippy passed in 70.345s** and unchanged production
bins built in **78.638s**. Epoch 108 then passed diff/format, all-target/all-feature
Clippy (**72.306s**) and no-run compilation (**153.061s**), with **3,934** library
tests discovered. `/tmp/467-final-validation108-quality-results.json` and
`/tmp/467-discovery108.txt` record those boundaries; compilation alone is not
execution.

The earlier epoch-107 diagnostic failed two test expectations: an invalid foreign
ConversationId fixture and a comparison of canonical ResultMetadata against a full
ToolExecutionResult. Both are retained as failed diagnostic history. The repaired
result proof compares the exact metadata contract, follows the advertised
same-cut canonical blocks, reads every giant text byte through bounded original
payload references, and reconstructs the complete original result before and
after terminal J. It preserves the old heads and independent C-without-J outcome.
All 14 exact selectors passed at 108, including this proof and the real Child
live/cold route; `/tmp/467-focused108-results.json` contains their individual logs.

Real TUI 107 Root Delivery (two) and Child transcript (two) tests passed over
Stdio/WebSocket, **four passed / zero skips**
(`/tmp/467-tui-real107-results.json`). Protocol/dev typechecks, dev **41/41**,
provider **54/54**, notification **15/15** and Office preflight passed at 107.
The initial focused Web run including the 20 new Messages tests passed **187**
tests. At that historical checkpoint, its type correction and strict Request
guard still required final checks. New inspection-head client parity was not
inferred from the four existing real TUI cases. Native captures108, protocol
and full Rust/CI-lane execution were still running; later results below supersede
that pending status.


### Epoch-107 finite inspection proof directory

These **14 tests all passed at epoch 108** in the exact serial runner
(`/tmp/467-focused108-results.json`), and all selectors are present in the
3,934-test library discovery (`/tmp/467-discovery108.txt`). They extend the
existing T01–T20 matrix, not a new acceptance numbering scheme. The directory
records each tested boundary; it does not establish a complete Rust or browser
pass. New Web/TUI finite-head consumers still require their final generated and
client checks; the native Child route proof alone is not T11 client parity.

| Exact selector | Matrix rows | Actual boundary and evidence asserted by the saved test |
| --- | --- | --- |
| `durable::sqlite::tests::t08_t12_t15_t19_trace_definition_uses_exact_original_retry_catalog_without_body_reads` | T08/T12/T15/T19 | Actual retry 0/1 requests reuse wire Tool identities. The later request exists before the first accepted C; each original C still selects its own catalog and first matching Tool ordinal. Authorizer denies original J/C/R and growing alias columns while exact giant schema ranges advance. |
| `durable::sqlite::tests::t10_t12_t19_trace_definition_auxiliary_failure_preserves_native_c_j_and_own_lifecycle` | T08/T12/T19 | Actual SQLite auxiliary-write fault leaves original C/J and independent own facts committed; Schema failure cannot become another request or execution failure. |
| `durable::sqlite::tests::t10_t12_t19_trace_definition_missing_and_ambiguous_catalogs_never_become_absent` | T12/T19 | Missing/ambiguous optional relationships report unavailable; absence requires the original complete catalog. |
| `durable::sqlite::tests::t08_t12_t15_t19_trace_adoption_advances_and_validates_only_selected_native_members` | T08/T12/T15/T19 | Original adoption ordinal/L membership advances beyond its prefix. Selected-member witnesses, cut bounds and authorizer/work assertions reject wrong original membership without loading all bodies. |
| `durable::sqlite::tests::t10_t12_t19_trace_adoption_index_failure_never_refuses_native_adoption` | T08/T12/T15/T19 | An actual optional membership-index failure cannot refuse or replace native adoption, canonical rows or Journal facts. Missing derived evidence stays explicit. |
| `durable::sqlite::tests::t12_t19_trace_definition_rejects_another_real_request_and_later_matching_ordinal` | T12/T19 | Wrong existing RequestStart, wrong existing ordinal, later duplicate Tool ID and cleared positive ordinal cannot select a different schema or fabricate absence. |
| `durable::sqlite::tests::t10_t12_t19_missing_request_body_layout_never_hides_own_assistant_tool_facts` | T08/T12/T19 | A trigger rejects the entire optional Request field layout. Original Assistant/Tool J, canonical proposal and exact argument bytes remain serviceable; the optional accepted-request/Schema join fails separately. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_tool_schema_unavailable_preserves_own_lifecycle_and_original_fields` | T08/T12/T19 | Clearing optional Schema readiness leaves the complete own Tool record, original fields, canonical history, Journal bytes and native cut unchanged. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_tool_inspection_has_frozen_schema_and_bounded_original_ranges` | T08/T12/T19 | Real durable Host checkpoint and historical readers return the same typed Tool head; tiny/giant original payload and frozen definition chunks match independent original values, retain ≤2-row/64 KiB range bounds, reject foreign CID and do not mutate the checkpoint. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_tool_inspection_keeps_c_without_j_and_old_cut_after_terminal` | T08/T19 | Capture before result C, commit the actual Tool C without terminal J, then append the real terminal. Result outcome/artifact/original bytes appear only at the C-inclusive cut; C alone does not invent terminal J, and both old heads remain equal. Exact metadata plus all canonical block payload chunks reconstruct the entire original giant result, including another read at that frozen cut after terminal J; every range retains the two-row/64 KiB bound. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_trace_assistant_compaction_and_lifecycle_heads_use_their_own_kind` | T08/T19 | Request retains its indexed reader; Assistant, unsettled Compaction and lifecycle anchors use their actual kinds. Original canonical block pages match independent same-cut Store headers; unsettled Compaction invents no summary C. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_all_advertised_canonical_kinds_have_advancing_inspection_heads` | T08/T15/T19 | Real acceptance/adoption of 19 Human or Runtime messages yields User/Context heads, eight initial members and advancing pages preserving all original identities/order; each canonical page matches its independent Store source. |
| `runtime_client::trace::tests::inspection_tests::t11_t19_trace_and_journal_sources_keep_native_sequence_above_safe_byte_coordinates` | T11/T19 | The native Store assigns original J above 2^53 after sequence-boundary setup. Trace/Journal string identities still select the exact original terminal bytes; zero identities and unsafe byte offsets remain rejected. |
| `boundary_suites::subagent::conformance::tool_inspection::t08_t12_t18_t19_tool_inspection_uses_real_child_live_and_cold_original_sources` | T08/T11/T12/T19 | Real Child runtime, Unix inspection and Registry settlement hold the model at an actual provider gate, then move Live → DurableOnly. Both routes read the original Tool head/frozen definition at exact CID/Activation/cut, reject foreign Activation and preserve the two original model requests and native source. The gate releases on fixture drop. |

These proofs cover existing Source, Projection/read, Registry and carrier owners.
They add no parent Child-history mirror, execution identity alias, cursor owner,
content source family or schema-registry fallback. The 14 exact proof passes and
quality results above are recorded execution, while remaining client/full Rust,
CI-lane and browser results stay owned by the execution ledger rather than being
inferred from this directory.

## Historical validation checkpoints 108–111

The full epoch-108 Rust command (`RUST_TEST_THREADS=4 cargo test --all-targets
--all-features`, with the required provider emulator) executed 3,934 library tests:
3,929 passed, five ignored measurement/generator tests, and no library failures. The command
then stopped in the conformance target: 25 passed and
`agent_loop::image_tool_round_trip_and_text_model_switch` failed while its fixture
looked up the immutable original AttemptStarted through an independently mutable
full transcript cut. Actual pending adoption may expire that full cut. The fixture
now queries the existing bounded original Journal index through one captured J
frontier, preserving exact Conversation/Attempt identity, original terminal
uniqueness and the independent complete original-J terminal-last oracle. It does
not retry, restamp or weaken production read-cut validation. A separate real-disk
Store regression captures that interleaving explicitly:
`durable::sqlite::tests::t08_t12_t19_pending_adoption_expires_full_cut_but_preserves_indexed_original_start`.
The complete repaired-source Rust command subsequently passed at epoch 111,
as recorded below.

After that command stopped, direct execution of its immutable compiled external
binaries passed the contracts, durable (147 tests), process, provider, subagent
(47 tests) and tools (135 tests) targets. Those diagnostics do not substitute for
a complete current-source run. The lane checker was not executed by the stopped
full-command runner. Logs and exact commands are in
`/tmp/467-final-validation108-native-full-results.json` and
`/tmp/467-external108-diagnostic-results.json`.

Epoch-110 full Web tests executed 2,793 tests: 2,788 passed, four existing skips,
and one failure at the five-second deadline of the real PTY delayed-ABA retirement
test. The epoch-109 obsolete Tool-detail fixture had been updated to the actual
finite-head/read-cut contract and passed in this run. The PTY failure is being
localized at its existing native ownership/retirement awaits, without increasing
its deadline.

Epoch-110 full TUI tests executed 1,144 tests: 1,141 passed, one existing skip,
and the two real Child cases (Stdio and WebSocket) failed. Their new selected
canonical-block body assertion attempted a generic ValueRead and received the
explicit expired-cut error. The source contract and both clients' advertised
canonical-content readers are under review; the assertion must reconstruct the
original bytes using the authoritative advertised source. The provider cleanup
reported its unreleased gated scenario after this primary failure; that cleanup
error is not the primary cause. Web build was not reached by the stopped runners.

The following epoch-111 results supersede these failed runs. The historical
failures remain recorded; a local pass does not imply remote CI success.

### Executed epoch-111 results

- `git diff --check`, `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo build --bins`, and `cargo test --all-targets --all-features --no-run`: passed.
- `RUST_TEST_THREADS=4 cargo test --all-targets --all-features`, with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`: passed in 940.778 seconds. Library:
  3,930 passed / five ignored measurement/generator tests; external cfg3_catalog 26,
  cfg3_managed_output 5, conformance 26, contracts 31, durable 147, process 62,
  provider 175 (five existing live-provider ignores), subagent 47, and tools 135
  all passed. Example tests: lane checker 17 and list workload checker three passed.
- The five library ignores are four opt-in measurements (`issue419_measure_native_cold_load`,
  `stage_profile_real_create_pipeline`, the new `measure_native_request_index_complete_300_turn_source`,
  and the new `t19_native_publication_encoding_profile`) plus the existing
  document fixture generator. No correctness test was disabled. The five
  provider ignores require external live-provider credentials.
- `cargo run --all-features --example check_test_lanes`: passed; every runnable
  Linux test is assigned to a CI lane. This is local discovery evidence, not a
  claim that remote CI or macOS execution has completed.
- Both exact new mutation-boundary proofs passed: the two-handle SQLite
  pending-adoption test (5.547 seconds) and the original failing
  `agent_loop::image_tool_round_trip_and_text_model_switch` (25.216 seconds).
  The latter also passed inside the full command above.
- `pnpm --dir protocol/app-server check`, its `typecheck`, Dev `typecheck` and
  `test`, Web `typecheck`, `check:i18n`, and `check:provenance`: passed. Source
  provenance verifies 276 records and notices for 264 production packages.

The canonical-content failure was a concrete client contract error. Native
canonical PageItems provide finite inline metadata and explicit ContentBindings;
they do not advertise a whole-item ValueRead. Web and TUI now use only those
original ContentRefs for bytes, including explicit reads of small raw JSON whose
parsed header is already complete. Sparse Tool pages retain original block
coordinates, independently of Tool count. An advertised continuation returning
an empty page is rejected, while a manually requested out-of-range empty page
remains valid. TUI Root and Child selection owners retain only the current page
and bounded byte window; late pages cannot replace a newer selection.

The repaired Web focused suites passed 64 tests. TUI focused suites passed 148
tests after Root/Child exact-source, sparse-coordinate, late-selection and
advertised-continuation coverage; TUI typecheck passed. The real carrier rerun,
full clients and fresh browser validation are recorded separately below.

Epoch-111 real carrier and full-client gates passed:

| Command / actual boundary | Result |
| --- | --- |
| `node --test --test-name-pattern='lists, renders, saves byte-exactly\|shows metadata without the delivery credential' test/delivery-integration.test.ts` (TUI directory; required emulator and current binary) | Two real Stdio/WebSocket cases passed, zero skips; 3.436 seconds. |
| `node --test --test-name-pattern='native child transcript' test/integration.test.ts` (same environment) | Both real Child Stdio/WebSocket cases passed, zero skips; 6.794 seconds. Original finite Tool metadata and advertised raw argument bytes, exact Child ownership, provider request counts and reconnection assertions remain active. |
| `pnpm --dir tui typecheck` and `pnpm --dir web-console typecheck` | Passed on the final continuation-validation source. |
| `pnpm --dir web-console test` | 2,795 passed / zero failed / four existing skips; 2,799 discovered. The real delayed-ABA PTY case passed in this complete run. The earlier timeout remains recorded; no production lifecycle/deadline change was inferred from the isolated pass. |
| `pnpm --dir tui test` | 1,151 passed / zero failed / one existing skip; 1,152 discovered, including the repaired real Child cases. |
| `pnpm --dir web-console build` | Passed, including native Host modules and bundled-artifact provenance. |

Current-source browser validation starts with the six previously failing files
on the default temporary filesystem, followed by the complete pinned-browser
suite. Browser results are not implied by the client passes above.

### Browser epoch-111: nine repaired scenarios pass; Tool result controls incomplete

The six previously failing files executed ten current-source browser cases on
the default temporary filesystem with the pinned Chromium image: nine passed
and Chat failed (277.405 seconds including fixture startup). Child capabilities,
all four Compaction locale/theme cases, real Office document preview, both
Settings observation cases and historical Trajectory passed. The complete
command and output are recorded in `/tmp/467-browser-111-focused-results.json`
and `/tmp/467-browser-111-focused.log`; failure artifacts were retained separately.

Chat's new result-body assertion exposed an actual client omission. ResultMetadata
correctly excludes canonical result content, and its `artifacts` may be empty
while an image block owns `artifact_1`. Native already advertises the positive
`record.collections` canonical result-block source and its exact ContentRefs.
Web Tool Result and TUI Child Tool inspection omitted those collection controls.
The fix reuses bounded canonical collection controls under the original cut;
it does not fabricate a result Message or request a whole-body ValueRead.
The test must separately verify original metadata and the advertised image-block
bytes while retaining its artifact preview and provider-count assertions.

A related ownership review found incidental callback identity changes could
reset DeferredContent's byte window during an unrelated parent rerender even
when source/cut/reader ownership was unchanged. The repair must distinguish that
case from actual reader, cut, source or view retirement, with held-response and
continuation evidence. These client changes require fresh client/browser runs;
the epoch-111 Rust pass remains evidence for unchanged Native sources.


## Historical pre-review client validation epochs 113–114

The Tool result omission recorded at browser111 is repaired in both clients.
Web `TraceToolFields` selects the original `record.collections` source under the
retained cut and mounts one metadata field or one canonical collection at a time.
A sole advertised collection remains reachable when metadata is absent or a
previous selected field disappears. The current canonical page is at most 32
items; explicit content reads replace one at-most-65,536-byte window. TUI Child
`b`/`p` uses the same advertised Tool result collections as Root; no synthetic
result Message or whole-item ValueRead was introduced.

Reader callbacks now remain stable across an unrelated same-owner rerender, while
held replies check the latest ownership predicate, reader identity, cut and
source before decoding or applying bytes. Deterministic tests cover held Content
and Value responses, reader replacement, owner retirement, and valid unchanged
owner continuation. The final five focused Web suites passed 62 tests; a first
new assertion matched both a field label and a byte window and was corrected to
select the exact `pre` window before the complete rerun.

| Current-source command | Result |
| --- | --- |
| `pnpm --dir protocol/app-server typecheck` (113) | Passed. |
| `pnpm --dir tui typecheck` (113) | Passed. |
| `pnpm --dir tui test` (113) | 1,153 passed, zero failed, one existing skip; 1,154 discovered, 36.341 seconds. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUSTX_BINARY=target/debug/rustx node --test --test-name-pattern='native child transcript' test/integration.test.ts` (TUI directory, absolute binary path, 113) | Two actual Stdio/WebSocket Child cases passed, zero skips, 6.959 seconds. Exact original Tool metadata, questionnaire result bytes, held provider execution, request counts and reconnect ownership remain asserted. |
| `git diff --check` (114) | Passed. |
| `pnpm --dir web-console typecheck` (114) | Passed, 12.631 seconds. |
| `pnpm --dir web-console test` (114) | 2,809 passed, zero failed, four existing skips; 2,813 discovered, 34.331 seconds including preparation. |
| `pnpm --dir web-console build` (114) | Passed, 5.692 seconds. |
| `pnpm --dir web-console check:i18n` (114) | Passed. |
| `pnpm --dir web-console check:provenance` (114) | Passed; 276 source records and 264 production package notices. |

At this pre-review checkpoint, Native sources were unchanged from the complete
epoch-111 Rust and lane passes. Its Chat and complete browser results are recorded
below; both passed before the original commit and PR publication.

A fresh main fetch at epoch114 still resolves to
`97096020d64ea00218493a0a99b9b805821eeb28`. Open PR #477 advanced to
`843dd7dd17ef86cb3a14253dc8bf4dfaf86d3926`; its model-context projection head
publication integrity, complete timeline record admission and fixed fingerprint
bounds were inspected. That PR remains unmerged and is not an implementation
dependency. This change preserves the separate existing Compaction owner.


The isolated current-source Chat browser run passed on the default temporary
filesystem: `bash scripts/browser-tests.sh test/e2e/chat.spec.ts` from
`web-console`, one case passed in 47.309 seconds including fixture preparation.
Its exact original ResultMetadata, canonical image header and Base64 bytes,
artifact lightbox, provider request counts, reconnect and upload assertions remain.
The complete `pnpm --dir web-console test:e2e` command then passed all 317
scenarios, with zero failures or skips, in 1,403.459 seconds (23.3 minutes):
`TMPDIR=/dev/shm/r4.x0j58s0d`, the unchanged pinned browser, workloads and deadlines.
This environment qualifies real carrier/SQLite/source behavior, not physical-media
fsync latency; native epoch111 exercised real-disk Store/restart/range boundaries.


Final browser evidence is `/tmp/467-browser-114-full-results.json` and
`/tmp/467-browser-114-full.log`; the repository's final Playwright artifacts remain
available in the worktree. The earlier failed browser evidence was preserved,
including default-disk writeback throttling and the actual Tool collection UI gap.
Both 300-Attempt preparations completed under their original 240-second budgets.
No screenshot baseline, race assertion, workload or correctness timeout was relaxed.

That pre-review pre-publication fetch again confirmed main
`97096020d64ea00218493a0a99b9b805821eeb28` and unmerged PR #477 head
`843dd7dd17ef86cb3a14253dc8bf4dfaf86d3926`. There is no known unresolved local
contract failure in the mapped T01–T20 evidence. Remote CI and macOS execution
must be reported from their actual run states, independently of these local passes.

Pre-review staged-tree `git diff --cached --check` and protocol regeneration/drift checks
passed. The index contains v48 generated contracts and all intended source/tests,
with v47 artifacts removed and no logs, databases or browser recordings staged.


### Controlled full-chain measurements (review190)

The default debug App Server binary completed the same 1000-Attempt measurement
scenario plus exact Source landing, Fork and detach in 600.121 seconds, versus
864.990 seconds for the preserved pre-full-chain review172 binary. Physical
cleanup completed at 602.350 seconds; the original process and temporary storage
were absent. Native CPU fell from 972.04 to 669.45 seconds. Physical writes stayed
approximately 7.85 GB: the change does not claim to eliminate durable writes.
There were no concurrent task-owned builds/tests. Both runs recorded high and
variable host I/O pressure, so these are qualified measurements, not a guaranteed
speedup on other machines. The formal browser fixture remains 300 Attempts/240s.

| Attempt bucket | Preserved baseline median ms | Shared full-chain median ms |
| --- | ---: | ---: |
| 1–50 | 538.731 | 516.556 |
| 251–300 | 715.535 | 554.062 |
| 501–550 | 836.302 | 574.533 |
| 751–800 | 974.549 | 582.782 |
| 951–1000 | 1093.178 | 602.734 |

The separate 300-Attempt scripted native profile passed with 600 preparation
leaf visits, zero old items revisited, zero estimator materialized items and
144,709 incremental hash bytes. Its measured Attempt interval was 222.121s,
compared with 201.407s in review174; overall scripted elapsed time did **not**
improve in this run. Inclusive prepare/start/commit/evidence phases were
4.852/90.565/84.602/0.071 seconds; the former post-commit reconstruction phase
was 19.304 seconds. These phases overlap and must not be added.

Source consumed 73.017 inclusive seconds: current-request scalar metadata capture
52.318s, capture18.975s (including new-leaf15.762s and authentication1.798s),
paths0.859s and length0.786s. Scalar capture omits messages and its per25-Request
cost stayed approximately4.33–4.44s. It is measured per-request work, not evidence
of old-message traversal or justification for a new cross-request cache. Source
query counts include authentication counts and are themselves included in native
index counts. No old canonical bytes, publication checks or storage exclusion
were removed to obtain these results. The complete native/client/browser validation above is separate from these
qualified performance measurements.
