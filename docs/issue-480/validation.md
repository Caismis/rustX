# Issue 480 validation record

The original implementation results below are retained. Benchmark acceptance
repairs and final-head verification are tracked in [the re-review report](review-repair.md).

All provider requests use the local Python emulator or Pi's in-process faux
provider. No real model API is used. Timings are observations, never correctness
thresholds. Final benchmark runs start only after task-owned builds/tests finish.

## Deterministic WAL regressions

The focused `cargo test --lib --all-features --locked wal_ -- --nocapture` run
passed all 10 tests. These cover WAL/NORMAL schema birth; coherent management head
reads during another writer's commit; snapshot-scoped native certificate
invalidation; non-creating management and writer-owned conversion; common archive
capture across ordered writes; cancellation and partial-acquisition cleanup;
preflight at the captured head despite later activation; Fork from uncheckpointed
source facts followed by source removal; and live-WAL management lifecycle reads.
The existing slow ZIP test also completes TRUNCATE checkpoints while delivery is
gated, proving archive snapshots do not survive downstream backpressure.

The accepted-inbound SIGKILL boundary now proves WAL header format and actual
uncheckpointed frames before killing the writer. Existing process-death tests
continue to validate ordered committed P/U/C prefixes, artifact settlement,
publication, recovery and installation; no assertions are weakened for NORMAL.

## Findings during verification

- Archive corruption tests now invoke post-capture preflight, where full history
  validation belongs. They still reject the same corrupt records; capture only
  reads finite frontiers under the common writer exclusion.
- The VACUUM INTO fixture produces a DELETE-mode image. It now explicitly opens
  through the writer path before cold management reads, preserving the new
  management rule against hidden journal conversion.
- An App Server pipe test timed out waiting for EOF in the broad parallel suite
  but passed alone. Its raw pipe descriptors could be inherited by concurrently
  spawned process fixtures. The six affected fixtures now use `std::io::pipe`
  (then convert to owned descriptors), preserving close-on-exec behavior on Unix
  without changing production transport code or extending any deadline.
- Three capacity fixtures used the file constructor with the SQLite special
  path `:memory:`. They now use the existing explicit `in_memory` constructor;
  production file opens still reject failure to establish WAL.
- New dependency hook APIs return fallible results. Production propagates hook
  installation failures and restores scoped handler policy; tests unwrap their
  fixture hook installation instead of silently ignoring the result.

- The unrestricted parallel unit run passed 3,969 tests but timed out in the
  existing ten-second parallel tool-progress guard. Diagnostics showed both
  physical completions, both canonical tool results, all bounded progress, and
  the next turn-start fact. The isolated unchanged test passed in 2.03 seconds.
  A follow-up complete local run uses eight test threads to reduce host CPU
  contention; it retains every assertion and deadline. The failed run is retained
  as validation evidence, not excluded from reporting.

## Verification status

Formatting and whitespace checks pass. All-target/all-feature Clippy passes including the portable pipe and benchmark
accounting edits. The complete unit/binary/example rerun passed 3,970 library tests plus 30
additional tests (four intentionally ignored), with eight test threads. Contracts
passed 31 tests; provider tests passed 179 (five intentionally ignored). The first
in-crate boundary run passed 240 tests and failed only the three `:memory:`
constructor fixtures described above; the corrected complete rerun passed all 243 tests. The external boundary targets passed 452 tests (one benchmark intentionally
ignored). The Linux test-lane coverage checker passed. The fake
provider's complete Python suite passed 55 tests. Remote FULL verification must
pass on the final PR head; no merge is authorized.


## Benchmark validation

The original three-pair matrix passed 17/18 BEFORE and 18/18 AFTER runs. One
BEFORE large-response case failed at the original attempt Journal frontier read;
its log/process status are retained. An additional large-response pair passed,
so every workload has three complete pairs and all successful observations remain
reported, including the unmatched first AFTER large sample. Corrected tool smoke
checks passed both revisions. Twelve pinned Pi version-2 runs, six native profile
runs, and both separately traced short-small executions passed their oracles.
Raw process status, fixture hashes, logs and compressed traces are under
`evidence/after`; initial failures and preliminary measurements remain under
`evidence/before`. No timing threshold was used to select or accept a sample.
