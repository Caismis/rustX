# Atomic ordinary model settlement

The native transaction now owns successful provider completion, final frames,
canonical Assistant acceptance and staging cleanup. A no-tool output adds
TurnCompleted and either adoption of the finite selected pending batch or
AttemptCompleted for an empty inbox. Existing append validation is seated only
after COMMIT. Tool output still needs its existing intent/execution/result
transitions; no external effect is made atomic with SQLite.

Cancellation observed before an ordinary no-tool final acceptance follows the
noncanonical audit path. Once its native empty-inbox completion commits, a later
cancellation cannot reclassify that Completed winner. The deterministic post-commit cancellation test confirms this winner; final
FULL client validation remains required.

The Agent holds the existing mailbox publication mutex from pending selection
through committed event/adoption/terminal notification. This prevents a later
acceptance from overtaking the final terminal notification. It is not a cache,
queue or new persistence owner. Native envelopes, snapshot ownership, proposal
identity, pending cursor/order and empty-inbox expectations are checked inside
the transaction. Observations remain optional; canonical acceptance does not.

Workflow output has a separate submission owner and retains its P/U preparation.
Rejected noncanonical output retains P/U/audit preparation; those facts never
permit an unaccepted proposal to execute. These paths are explicit rather than
legacy-layout fallbacks. Gate 75 rejects all earlier development stores without
migration or dual writes. Event wire fields and client protocol are unchanged.

Deterministic native tests cover ordered final facts and readable canonical output,
rollback at final frames/canonical/terminal, finite inbox selection and later
unselected input, and foreign envelope rejection. Real process tests cover
SIGKILL inside provider/frames/canonical/before COMMIT and after COMMIT, with exact
all-or-none assertions. The prior P-only and U-only audit cases now use a real
unknown-tool preflight rejection rather than an unreachable ordinary-output cut.
Internal retry success commits its Attempt terminal with the final answer, so
recovery must not resurrect an earlier partial audit.

Targeted validation passed as recorded below; measured results and final FULL
client validation are pending. The partial coalescer change is
not included here. No whole-request O(1), exactly-once tool execution, power-loss
safety, or end-to-end performance gain is claimed.

The first native run passed four tests and exposed unreadable final-frame refs:
canonical settlement had already deleted their live staging backing. Ordinary
accepted final output now publishes only the complete canonical receipt after P;
it does not release transient final-frame receipts whose backing no longer exists.
Previously committed partial frames remain supported and the client already
rebinds them to canonical content on that receipt. Native tests verify exact final
canonical bytes, and the post-commit cancellation fixture checks ordered event
publication and the Completed winner. This intentional final-notification change
passed real process validation; final Web/TUI validation remains required. No stale
reference is silently accepted.

## Targeted validation

On 2026-10-18, using Rust 1.98.1, one build job, debug/test debug info disabled
and incremental compilation disabled:

- Five native final-settlement tests passed (0.23 s).
- All 35 Agent execution tests passed (9.38 s), including exact event/generation
  publication ordering and cancellation at the first post-commit notification.
- Fresh `cargo build --bins --all-features` preceded all 65 real-process
  crash/recovery tests (19.62 s), with `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`.
- All 413 durable SQLite tests passed (301.82 s).
- All 26 Source tests passed (1.04 s).
- `cargo clippy --all-targets --all-features -- -D warnings` passed (2m 52s).
- `git diff --check` passed.

Unit filters were `durable::sqlite::final_settlement_tests`, `agent::execution::`,
`durable::sqlite::`, `source::`, and
`boundary_suites::durable::process_death::conformance::` on the all-features library
test executable. These overlapping suites are not additive coverage counts.

[Checks archive](evidence/atomic-settlement/checks.tar.xz) and
[SHA-256 manifest](evidence/atomic-settlement/checks-manifest.json) preserve all
initial failures and final successful logs, including the invalid transient-ref
failure, incorrectly turn-scoped Attempt terminal fixture, and generation-stripping
test-helper mismatch. Fixes retain exact assertions. No timeout inflation, retry
loop, or weakened recovery oracle was used. Final lint-only fixture formatting and
a documented test-length lint allowance followed the behavioral checks.

These are targeted checks, not an exact-head FULL/Merge-verification claim.
Partial cadence is unchanged at this stage. Performance and final dependency
comparisons remain necessary before a retain/revert recommendation.

## Client notification and concurrent-inbox follow-up

The broader scripted suite initially exposed six assertions tied to transient
final frames/P-only failure and four client delta-only expectations. Fixtures now
record the actual canonical notification, verify exact complete content after a
partial checkpoint, retain interleaved partial ordering, and reject success/P
publication after a failed final transaction. Streaming-specific fixtures cross
a deterministic pressure threshold; no sleep or timeout increase is used.

The post-commit cancellation test now includes a concurrent inbox producer. An
existing nonblocking mailbox snapshot probe proves the publication lock is held
at P notification and still held at the final Attempt callback. A channel gate
places the producer at that boundary; its acceptance must observe terminal
publication first. The late input remains pending, only one model request ran,
and published events equal native events exactly, including generation evidence.

Cross-stage validation on the locally integrated cadence candidate passed 606
scripted tests (90.90 s), 89 tests matching `agent::execution::` (19.62 s), and
all-target/all-feature Clippy (2m 52s). These filters overlap. The mailbox probe
and exact publication test both passed. The new ordering fixture's initial UUIDv4
was corrected to the required UUIDv7; its failure is retained. Two test futures
were boxed after the larger recording helper crossed the lint threshold.

[Checks](evidence/atomic-client-validation/checks.tar.xz) and
[manifest](evidence/atomic-client-validation/checks.tar.manifest.json) retain the
initial failures and successful follow-up. This is not a final FULL result or an
atomic speedup claim. The separate cadence stage supplies canonical tool proposal
installation when structural frames no longer force a flush; that integration
change belongs to the cadence increment, not this test-only follow-up.
