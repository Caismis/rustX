# Completed response tails: architecture and validation

## Ownership and versions

PR #368 / Issue #364 merged at `3063ebd63356f057e8c3f9ec2efb44feffe9252d`.
PR #369 rebases on that architecture. Canonical content remains Message Ledger /
Conversation authority. Runtime Client derives at most one completed tail for the
exact closing Assistant of a successfully completed Attempt. Intermediate Tool
requests, interrupted output and unsuccessful Attempts do not create final tails.

Mandatory versions: **App Server 10, Runtime Client 42, SQLite 42, Session catalog
12**. SQLite 42 combines schema-41 generation evidence with immutable bootstrap
response provenance. Obsolete development stores are rejected without migration.
Only generated `v10.schema.json` / `v10.ts` exist. Both clients, initialization,
WebSocket admission, fixtures and freshness checks use v10; no old aliases/readers.

```text
Native ModelRequestCompleted/Failed.generation: GenerationEvidence
                   /                          \
          Trace / Trajectory          completed-response projection
                                                |
                                          Chat response tail
```

Chat does not join Trace records or persist telemetry. Native generation semantics
and Trace's request-relative phase projection remain unchanged.

## Timing and coverage

- Total runtime: authoritative `AttemptStarted.timestamp` to the owning successful
  `AttemptCompleted.timestamp`. Includes model/tool/retry work. Missing/reversed
  endpoints omit this value; browser clocks and last-request duration are irrelevant.
- TTFT: `time_to_first_output_ms()` of the first actual request in Journal order,
  including a failed first request. It measures adapter dispatch to first
  provider-independent output, never Attempt start. No fallback to a later request.
- Generation work: checked sum of `GenerationEvidence::generation_ms()` across
  output-producing actual requests. This helper defines first output to provider
  terminal, not last output or later canonical acceptance. Every actual request
  must have terminal generation evidence. Known no-output requests add no invented
  span; if none produced output the aggregate is absent. Missing evidence or invalid
  endpoints makes the aggregate absent. A measured zero span remains `0`.
- Throughput: corresponding summed output tokens divided by the summed generation
  seconds. Every actual request must have usage; no-output requests must report zero
  output; every output-producing span must be positive. Otherwise the rate is absent.
  Failed/retried requests are included in the same actual-request set as usage.
  The displayed floating-point rate is derived, not an exact token-count claim.
- Missing dispatch bridge prevents request-start-relative phases in Trace. It does
  not erase measured dispatch-relative TTFT or generation in Chat. No phase is
  proportionally reconstructed from wall duration. Optional usage buckets still
  require complete coverage; absence is never zero.

## Lineage and statistics

Local completion comes from source execution evidence. Native lineage capture
freezes `CompletedResponseProvenance` into `LineageSeed` and the destination's
immutable SQLite bootstrap row, atomically with canonical history and Surface
provenance. It carries the minimal product summary (completion, usage, timing,
Retry input) and `ResponseOrigin`, not individual GenerationEvidence, request
snapshots, Journal events, or recovery residue.

The same canonical identity map remaps closing Assistant and retained Retry User
IDs. Origin remains the original Conversation/Attempt/closing identity as provenance,
never a destination content address. Missing retained input removes Retry. Deeper
lineage repeats this operation without a first-child exception. Bootstrap identity
validation rejects attempts to mutate the frozen summary on reopen.

`After` Branch/Fork includes the selected finalized Assistant and leaves an empty
composer. Both local and inherited responses are valid anchors with exact destination
message and immutable append revision validation. Stale revisions are not refreshed
or replayed. `Before` Retry cuts before the remapped User and replays once, preserving
old history. Upload ownership and uncertain-response fences remain native.

Inherited historical tails retain usage/timing. Whole-Conversation statistics count
only destination-local execution, starting from zero in a new lineage. Paging and
compaction do not change totals. Context occupancy remains latest applicable request
provider input usage divided by frozen RequestSnapshot capacity; compaction or a
newer unmeasured request invalidates it. No browser tokenization or Trace authority.

## Presentation and provenance

User messages expose Copy and authoritative time without a lineage toolbar.
Assistant tails retain Copy, compact Lineage/Usage, optional `Ran for`, and completion
time. Timing details label first-request dispatch TTFT, model generation work and
output speed without developer identifiers. Existing Modal, Tooltip, Button and
Harness-derived clock icon are reused. Hover/focus/no-hover and narrow wrapping
remain in the existing CSS; no second icon/component system was added.

Source inventory retains both #368 Trace resources and #367 tail resources. Local
hashes and protocol imports are updated. The pinned Harness TurnTailNodeView,
TurnUsagePanel and StatsPills remain presentation references only.

## Projection cost

One shared fold serves response decoration and lineage validation/copy. It reads
finite published Journal prefixes in indexed 128-event batches plus inherited
bootstrap summaries: O(J + R) per snapshot. This pass adds no second scan or cache
framework. Bounded incremental optimization remains a separate follow-up.

## Deterministic regression mapping

- `response::tests::timing`: exact 400/320/1280 native evidence, 19s whole Attempt,
  failed/retried request aggregation, missing bridge/output/usage, zero generation,
  and absent/reversed lifecycle endpoints.
- `one_attempt_many_requests...`: Tool-bearing intermediate content and two model
  requests yield one tail, first-request TTFT and summed generation/throughput.
- `response::tests::lineage`: source → child → reopened child → grandchild preserves
  exact timing/usage/origin; content/Retry IDs remap, no fake execution events,
  bootstrap is immutable, subsequent local execution alone adds statistics.
- Scripted App Server Branch/Fork/Retry test: independent Session ownership,
  inherited timing, repeat continuation anchors, empty composer, destination Retry,
  stale destination revision refusal and reopen.
- Existing compaction/history tests compare full response views across paging,
  reopen, compaction and later Attempts; Context occupancy tests retain native
  applicability/invalidation rules.
- #364 generation and Trace tests retain original dispatch bridge/phase semantics.
- Web tests cover native timing detail, absent metrics, measured zero, bounded Copy,
  missing usage buckets, paging identity and conservative mutation uncertainty.
- Real browser command flow covers Usage/timing, keyboard activation, Fork,
  reconnect, inherited timing, Branch, Retry, mobile and preserved original history.

## Final integration validation

Commands and final results are recorded below after execution. Initial integration
checks found obsolete protocol fixture values and one old missing-generation test
constructor; these were corrected at their native/version authorities.

### Commands

All commands run in the isolated Issue #367 worktree; package-specific commands
use the indicated directory. The native full suite uses the CLI serial-runner
flag, not an inherited `RUST_TEST_THREADS` variable, to retain subprocess framing.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --all-targets --all-features` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features generation` | 108 passed |
| `cargo test --lib --all-features runtime_client::response` | 12 passed |
| `cargo test --lib --all-features lineage` | 14 passed |
| `cargo test --lib --all-features durable::sqlite::tests` | 71 passed, including preceding-schema refusal/current reopen |
| `cargo test --lib --all-features runtime_client::trace` | 50 passed |
| `cargo test --lib --all-features context_measurement` | 1 passed |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::protocol` | 33 passed |
| `cargo test --lib --all-features scripted_suites::runtime_client` | 109 passed |
| `cargo test --test process --all-features app_server::` | 13 passed |
| `uv sync --frozen` in `test-support/fake-provider` | Passed |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Passed |
| `pnpm --dir protocol/app-server generate` | Passed, Rust DTOs generated v10 |
| `pnpm --dir protocol/app-server check` | Passed against staged generated artifacts; reproducible |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir tui install --frozen-lockfile` | Passed |
| `pnpm --dir tui typecheck` | Passed |
| `pnpm --dir tui test` | 799 passed |
| `pnpm --dir web-console install --frozen-lockfile` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 500 passed |
| `pnpm --dir web-console check:provenance` | 115 source records and 100 package notices passed |
| `node web-console/scripts/provenance.ts --reference /home/caismis/Documents/codes/deepseek-harness-364` | Passed |
| `pnpm --dir web-console build` (also executed by each E2E run) | Passed |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Final complete run: 44 passed |
| `pnpm --dir dev install --frozen-lockfile` | Passed |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 37 passed |
| `git diff --check` and `git diff --cached --check` | Passed |

Browser plugin not available: used the repository's prescribed immutable
Playwright container through Podman, with the standard config/ports. Existing
screenshot references passed unchanged. The real response-timing dialog was
visually inspected at 390px; it fits and retains legible labels. No console errors
occurred in the exercised command flow. macOS validation is delegated to PR CI.

### Corrections during validation

Initial version fixtures still answered v9 or treated Runtime Client 42 as a
future unsupported version; they now test current 10/42 and reject 9/41/43 as
appropriate. Strict Clippy required a named request-reading struct, a boxed large
lineage test future, and an unambiguous generated-version inventory check. A
focused invocation initially selected a nonexistent `scripted` integration target;
these tests live under `scripted_suites` in the library, and the corrected command
passed 109 tests. The `context::occupancy` filter selected zero tests; the actual
`context_measurement` regression passed and is also in the full response suite.

The earlier full Rust run used an already-running test executable while another
build replaced that executable. Self-spawning MCP/lifecycle tests then failed
source admission/ENOENT, in addition to the two outdated future-version assertions.
The final run is sequenced after all compilation and fixture corrections, with no
concurrent rebuild or test relaxation. This is recorded as validation interference,
not a product fallback or an accepted failed test.

The subsequent full run passed all 3,163 library tests, then exposed one stale
real-process reconnect assertion still expecting App Server 9. It now expects 10,
and the transport rejection test explicitly offers obsolete v9. The focused
real-process App Server suite passed 13/13; the full native suite was rerun again
after this final fixture correction.

One additional full run encountered an existing managed-Python preparation failure
in `capability_projection_covers_native_python_and_skills` (`source:python:py-echo`,
"source preparation failed"). The owner did not expose a more specific cause.
The same test passed unchanged in the preceding full run, its exact diagnostic
rerun, and the final full run. No retries were added to tests, assertions relaxed,
production source-preparation code changed, or environment workaround introduced.
Diagnostic command: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features
boundary_suites::runtime_client::python_capability::capability_projection_covers_native_python_and_skills
-- --exact --nocapture` (1 passed).

### Final full-run result

- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features -- --test-threads=1`: **3,779 passed, zero failed, six existing ignored**.
- `cargo test --doc --all-features`: **9 passed, zero failed**.
- Final complete pinned-browser run: **44 passed**, including Usage/timing details, inherited facts, mobile/keyboard interaction, and unchanged Trace phase coordinates.

Final fetch confirmed `origin/main` remained
`3063ebd63356f057e8c3f9ec2efb44feffe9252d`; no second rebase was required.
All implementation work stayed in `/home/caismis/Documents/codes/rustX-issue-367`
on `issue-367-chat-response-tail`. Other worktrees were untouched.
There is no outstanding #364 integration dependency or known implementation blocker.
