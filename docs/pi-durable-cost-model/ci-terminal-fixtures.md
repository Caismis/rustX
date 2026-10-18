# Exact-head FULL cancellation diagnosis

Head: `770248f10fe63ba50f7457ed17436ea339521618`.
Run: https://github.com/Caismis/rustX/actions/runs/39146279121
Linux deterministic job: `117730983806`.

The job started at 16:44:40 UTC and was cancelled at 17:29:56 UTC, matching
its existing 45-minute job limit. The PR head was unchanged and macOS remained
active. This was not supersession. The unit harness started at 16:54:35 UTC;
it reported 4,029 passes, one failure and four ignored tests of 4,035, with
one test unfinished. The last other test completed at 17:10:34 UTC. There is
no aggregate success/failure summary because the unfinished test prevented exit.

## Terminal durability fixture

`terminal_event_durable_failure_degrades_without_fabricated_terminal` arms
`fail_terminal_event_remaining` and supplies an ordinary successful model output.
That fault now occurs inside `settle_model_turn`, where terminal acceptance is
part of the atomic canonical transaction. Its error becomes
`CanonicalCommitError::Durable`, is classified `CanonicalCommit`, and the runtime
publishes `DurabilityFailed(operation="canonical_commit")`. The old test awaited
only `event_journal` through an unbounded queue loop. It missed the real degradation
and waited forever. The unchanged test reproduced locally: the diagnostic runner
stopped it after 30 seconds without a result. This is a test wait defect, not
proof of a hung production coordinator.

The local fix splits coverage by transaction owner. A successful model output
with injected atomic-terminal failure asserts `canonical_commit`, rollback of
provider-completed/Assistant/turn-completed/attempt-completed facts, no Assistant
in canonical history, one truthful failure terminal and degraded later admission.
A non-retryable provider failure reaches the distinct standalone terminal append;
its injected failure asserts `event_journal`, no observed or durable terminal,
and degraded admission. Both wait for any degradation, then assert the exact
operation, inside the existing one-minute liveness helper. No timeout is increased.

## Native snapshot fixture

`incremental_projection_independent_snapshot_capture` emitted a tiny complete
stream without gating. With bounded coalescing, the stream can complete before
an intermediate live publication. The fixture nevertheless required a real
nonempty LiveRef. The unchanged test failed locally in 0.55 seconds at that exact
assertion, matching CI's failed test.

The local fix parks the provider after the first text delta, awaits a committed
nonempty LiveRef through the real client subscription, then releases the rest of
the stream. It preserves the original text, exact cursor continuity, real SQLite
and Host expiry assertions, retained replay equality, and independent checkpoint
capture. There are no sleeps, retries, fabricated references or production cadence
changes. A fresh capture is additionally folded through the production TypeScript
projection and compared exactly to the independent native checkpoint.

## Validation

See `reproduce-before.json`, original failing logs, `validation-after.json`, and
all corresponding logs. The before/after builds use the same Rust 1.98.1 compiler,
locked offline all-feature library test harness, one build job, debug info disabled,
and no incremental compilation. No performance measurements were repeated and
all earlier benchmark evidence is unchanged. The edit is confined to two existing
`cfg(test)` modules. Final exact-head FULL/Merge remains required after review.

Final targeted results: standalone terminal **1 pass**, atomic terminal **1 pass**,
native snapshot **1 pass**, native final settlement **5 passes**, publication
**14 passes**, and projection **82 passes** (104 total). The fresh native capture
contains 22 contiguous events; its exact TypeScript checkpoint fold and expired
LiveRef progress checks pass. `cargo clippy --offline --locked --all-features
--lib --tests -- -D warnings`, `cargo fmt --all -- --check`, and diff whitespace
checks pass. The initial native-final command selected zero tests because its
filter omitted `tests::`; that invocation is retained and not counted as a pass.

[Readable results and logs](evidence/ci-terminal-fixtures/validation-after.json),
[before-fix reproduction](evidence/ci-terminal-fixtures/reproduce-before.json),
and [complete preserved evidence archive](evidence/ci-terminal-fixtures/evidence.tar.xz)
include commands, original failures, exact patch, compiler build records and the
fresh capture. Readable log copies omit trailing blank lines; the archive retains
the original files. This report is prepared locally before pushing the fix, as
requested by the parent. No merge or benchmark rerun was performed.
