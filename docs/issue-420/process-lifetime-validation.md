# Issue 420 process-lifetime follow-up

Starting worktree: `/home/caismis/Documents/codes/rustX-issue-420`; branch:
`issue-420-incremental-projection-chat-stability`. Fetched origin and verified a clean
worktree, matching local/remote/PR head `9fbfda118b2875038974cc18a2ecb6f0533a1e47`,
open/non-draft PR #423 targeting main with auto-merge disabled. Fetched main:
`ad863a24cf48fbb0d5182746e1046d4b64e8c167`. The primary worktree is untouched.

## Deterministic regressions

- `app_server_process_exits_with_cancelled_blocked_presentation_read`: actual rustx
  App Server process; debug fault injection parks a background Store read at connection
  admission and reports the exact boundary on stderr. SIGTERM requests production
  drain. The test has no release action, never rolls back a lock to rescue shutdown,
  and proves successful OS process exit under the unchanged outer liveness guard.
- `blocked_read_materialization_does_not_own_shutdown_or_projection_drain`: strengthened
  to use the Store cancellation wait and join. The old test's release channel is gone.
- `presentation_cancellation_interrupts_running_sql_and_restores_writer_policy`:
  an actual recursive SQLite query reports entry from its progress callback; cancellation
  interrupts the VM, the read thread joins, and the ordinary busy policy and writes work.
- `presentation_cancellation_ends_connection_wait_without_releasing_writer`:
  a retained writer lease cannot prevent a cancelled presentation reader from returning.
- `snapshot_acquisition_is_finite_when_every_read_is_superseded`: exactly three
  candidates are invalidated at the capture/read boundary; typed RuntimeFailure and
  dirty newest revision prove finite termination without stale installation.
- `snapshot_acquisition_retries_once_then_returns_exact_cursor_cut`: first candidate
  superseded, second installed, exact DTO/cursor preserved, next event replays at C+1.
- `trace_only_progress_does_not_supersede_a_durable_read_cut`: a Trace/Tool Session
  transition advances its independent frontier without invalidating unchanged durable
  dependencies. Existing stale-read and real JournalBatch/Trace tests remain.
- Both detach tests now wait for actual provider invocation instead of polling
  snapshots during request-start progress. `snapshot_cursor_race_snapshot_wins`
  attaches before its controlled race; its publish-first companion holds the next
  model-start transition at the existing native gate. Background final snapshots
  wait for the terminal notice's root attempt to settle. All semantic/replay
  assertions remain.
- Child-questionnaire/child-death/mixed-interaction conformance waits for root notice settlement
  before its final projection assertion. The mixed test allows at most three reads
  for two terminal-notice attempts, waiting on native settlement rather than time.
- Real subagent process drivers retain their original 4,000-observation budget but
  recognize the exact typed cut-exhaustion diagnostic as a refused read. Cold
  recovery's Initialize uses that same budget: this explicit refusal occurs before
  attachment allocation; lost/other responses are not retried. No execution
  mutation or uncertain outcome is replayed; final identity, lineage, process death,
  recovery-idempotence and message assertions remain unchanged.
- The existing SQLite-boundary shutdown process test is unchanged.

## Validation

Commands run in the implementation worktree. Native compilation uses
`CARGO_BUILD_JOBS=2` to bound compiler memory, without changing test threads, Tokio
workers or liveness deadlines. Normal Web tests keep standard concurrency. Logs:
`/tmp/issue420-followup-*.log`.

| Command | Result |
| --- | --- |
| `cargo test --all-features --lib runtime_client:: -- --nocapture` | 341 passed, including finite acquisition, cursor races, stale rejection, shutdown and real JournalBatch Trace publication |
| `cargo test --all-features --lib runtime_client::projection::tests -- --nocapture` | 45 passed |
| `cargo test --all-features --lib presentation_cancellation -- --nocapture` | 2 passed |
| `cargo test --all-features --test process app_server_process_exits_with_cancelled_blocked_presentation_read -- --nocapture` | Passed; exit code 0, no test release |
| `cargo test --all-features --test process app_server_websocket_drain_supervises_active_root_and_cold_resume -- --nocapture` | Passed unchanged |
| `cargo test --all-features --lib child_death_removes_only_its_routed_interactions -- --nocapture` | Passed |
| `cargo test --all-features --lib death_inside_the_projection_commit_is_atomic_and_repairs_exactly -- --nocapture` | Passed |
| `cargo check --all-features` | Passed |
| `cargo build --bins` | Passed; this binary was used by the complete E2E run |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 3,960 passed; eight existing ignored; zero failures, including managed Python |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 418 passed across seven external boundary targets |
| `(cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck)` | Passed; generated files unchanged |
| `(cd tui && pnpm typecheck && pnpm test)` | Passed; 852 tests |
| `(cd web-console && pnpm exec vitest run test/incremental-client.test.ts test/trace-cache.test.ts test/incremental-equivalence.test.ts test/incremental-vocabulary.test.ts test/incremental-presentation.test.tsx test/scroll.test.tsx)` | 38 passed across six files |
| `(cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance)` | Passed; 1,080 tests / 62 files at standard concurrency; 143 provenance packages / 131 notices |
| `(cd web-console && pnpm test:issue-420-performance)` | One fixed measurement passed; recorded counters unchanged |
| `(cd web-console && CONTAINER_ENGINE=podman RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 pnpm test:e2e)` | 125 passed; 6.9 minutes |
| `git diff --check` | Passed |

Focused host tests were also run separately (63 passed). Focused tests and full
commands were repeated when failures led to test synchronization corrections;
the table records final results, not just the initial green subset. No hosted CI
or macOS run is claimed. Managed-Python preparation succeeded locally in this
follow-up; the previous repair's PyPI failure remains historical evidence only.

Both generated metrics objects were compared with the committed after-evidence:
`/tmp/issue420-followup-performance.json` equals `evidence/after/jsdom.json`, and
`web-console/test-results/incremental-issue420-fixed-long-response-browser-recording/metrics.json`
equals `evidence/after/metrics.json`. The fixed fixture and before/after claims are
unchanged. The browser still records zero streaming snapshots, 210 commits,
94 automatic writes, zero message-seat replacements and zero bottom writes while
reading history; growth corrects the reading anchor from 210 to 333.

## Development findings

Initial focused runs exposed snapshot polling assumptions and an unnecessary veto
from Trace-only frontier movement. Read-domain candidates now use their own required
Journal prefix, still validated with the exact durable revision. The snapshot owner
also accepts a concurrently repaired clean cut without another read. Dirty cuts are
not returned merely because a native receipt is unpublished. No assertion, process
liveness deadline or global Runtime shutdown policy was relaxed.

The first process fixture omitted its token argument and read readiness from stdout;
that fixture setup was corrected to the existing stderr/token-file convention. A
new test initially lacked an Ordering qualification and SQLite test identities
needed canonical UUIDv7 values; compilation/identity assertions caught both. Clippy
caught documentation markup, Duration spelling, clone assignment and a nested if;
these were fixed without lint suppressions. Initial broad runs identified the old
snapshot-always-succeeds assumptions described above; those are recorded as repair
findings, not mislabeled environment failures.

The full suite also exposed that the Session catalog projection-death fixture
promised one canonical user boundary while inheriting optional Agent Status context
injection. That fixture now explicitly disables only that optional plugin before
composition. Its original exact message, transaction atomicity and recovery
idempotence assertions remain unchanged; the shutdown process regression is untouched.

No new issue/PR is created. After push, only one bounded PR/head/state verification
is performed; no CI watch or polling and no self-merge.
