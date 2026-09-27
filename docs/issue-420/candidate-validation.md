# Captured snapshot repair

This records the b7a2768b repair. The subsequent live-failure recovery repair is
recorded in [read-failure-validation.md](read-failure-validation.md); candidate
completion now additionally offers a successful result for optional fenced live repair.

Starting clean implementation worktree: `/home/caismis/Documents/codes/rustX-issue-420`,
branch `issue-420-incremental-projection-chat-stability`. `git fetch origin` confirmed
local/remote/PR head `c793100c38cbd57a9419b163b864454a107adff6`, main
`ad863a24cf48fbb0d5182746e1046d4b64e8c167`. PR #423 was open, non-draft, targeting
main, auto-merge disabled. The primary worktree remains untouched.

## Hosted regression evidence

Inspected run 36324878850 and its failed-job logs before editing. Linux Boundary
failed `runtime_process::the_process_serves_a_real_conversation_runtime` with the
old snapshot acquisition refusal, and
`app_server::app_server_reference_host_two_users_and_external_crash_recovery` with
App Server InvalidState. These are implementation regressions, not environment noise.

The completed Web job also failed chat acceptance. Its downloaded Playwright trace
first records native `Operation rejected` before page navigation, then the diagnostic
catch path waits for an unavailable Inspector button and reaches the test timeout.
The timeout alone did not identify the original failure. No test timeout was changed.
Protocol, Linux deterministic contracts, format/lint, TUI and macOS boundary jobs
passed on that reviewed head; no hosted result is claimed for the new commit.

## Architecture and regressions

See [the captured-cut contract](read-domains.md). Snapshot acquisition captures one
candidate C and completes its own copy; no live-head validation/retry is needed.
The follow-up adds optional matching-cut live repair without changing that contract.
The background materializer shares captured transcript membership but separately
validates the live installation revision. Its scoped Store cancellation and join
are unchanged. Inspection obtains one transactionally coherent seed and folds a
finite Journal prefix. Trace cannot rewrite captured Session fields.

New deterministic tests:

- `snapshot_candidate_survives_moving_head_and_replays_exact_suffix`: eight
  read-domain-invalidating transitions during materialization; C still succeeds,
  retains its original state and every suffix cursor is contiguous.
- `captured_snapshot_excludes_later_durable_messages_and_replay_converges`: real
  durable commits occur after capture; no newer body leaks backward; C's result
  cannot install over the newer live revision; replay reaches an independently
  materialized authoritative snapshot without a recovery request.
- `captured_snapshot_survives_replay_eviction_but_subscribe_requires_resync`:
  candidate completion succeeds after history eviction; subscription owns the
  typed ResyncRequired.
- `captured_pending_body_survives_later_edit_removal_and_commit`: pending body,
  revision and transcript membership remain frozen across actual Store mutations.
- `snapshot_transcript_excludes_later_tool_results`: a later canonical Tool result
  cannot enter an old Assistant's mutable association.

Existing real JournalBatch/Trace, stale installation, pending publication,
independent canonical equivalence, snapshot/cursor races and Store cancellation
regressions remain. Both hosted-failing process tests and both shutdown process
tests are unchanged. There is no test-side read release before process terminal proof.

Removed string recovery from both subagent process polling loops, the cold-recovery
Initialize loop and mixed-child conformance. Those tests return to ordinary successful
snapshot expectations. The two obsolete snapshot retry-count tests were replaced.
Detach conformance, catalog-summary isolation and the Trace race now prove that any asynchronous cursor suffix
contains only the existing derived publications; canonical/settlement assertions
remain. The Goal freeze waits for its request-start read-domain publication, and
admission-failure assertions allow only contiguous derived updates after semantic
failure. No new error variant, wire version or client retry mechanism is needed.

## Local validation

All commands run from the implementation worktree unless a package is named.
Native compiler concurrency is bounded with `CARGO_BUILD_JOBS=2`; test threads,
Tokio workers, liveness deadlines and standard Web concurrency are unchanged.
Logs are `/tmp/issue420-candidate-*.log`.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 3,963 passed; eight existing ignored; zero failures on final rerun |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 418 passed across all seven targets |
| `cargo test --all-features --lib runtime_client::` | 343 passed |
| `cargo test --all-features --lib runtime_client::projection::` | 45 passed |
| `cargo test --all-features --test process runtime_process::the_process_serves_a_real_conversation_runtime -- --nocapture` | Passed unchanged |
| `cargo test --all-features --test process app_server_reference_host_two_users_and_external_crash_recovery -- --nocapture` | Passed unchanged |
| `cargo test --all-features --test process app_server_process_exits_with_cancelled_blocked_presentation_read -- --nocapture` | Passed unchanged, no release before terminal proof |
| `cargo test --all-features --test process app_server_websocket_drain_supervises_active_root_and_cold_resume -- --nocapture` | Passed unchanged |
| `cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck` | Passed; generated files unchanged |
| `cd tui && pnpm typecheck && pnpm test` | Passed; 852 tests |
| `cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance` | Passed; 1,080 tests, 62 files, standard concurrency |
| `cd web-console && pnpm exec vitest run test/incremental-client.test.ts test/trace-cache.test.ts test/incremental-equivalence.test.ts test/incremental-vocabulary.test.ts test/incremental-presentation.test.tsx test/scroll.test.tsx` | 38 passed |
| `cd web-console && RUSTX_PERFORMANCE_OUTPUT=/tmp/issue420-candidate-performance.json pnpm test:issue-420-performance` | Passed; exact fixed fixture and counters unchanged |
| `cargo build --bins` | Passed before browser validation |
| `cd web-console && CONTAINER_ENGINE=podman RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 pnpm test:e2e` | 125 passed on final production build (7.2m); first run also 125 passed |
| `git diff --check` | Passed |

Additional focused native executions used `cargo test --all-features --lib` with
filters `snapshot_candidate_`, `captured_`, `runtime_client::host::`, and
`scripted_suites::runtime_client::`: respectively 1, 9, 65 and 108 tests passed.
The focused Tool association, Trace race, admission failure, Goal freeze,
cross-Session routing and managed FastMCP tests also passed. Full validation
includes the new deterministic candidate tests and existing real JournalBatch
Trace regression. No macOS or new hosted CI execution is claimed.

## Development findings

The first full run was interrupted after three known assertion failures while
managed FastMCP remained outstanding; its isolated rerun passed. Two early managed
Python capability preparations failed without an exact network diagnostic; their
subsequent focused and full reruns passed. These are not claimed as PyPI failures.
The next full run passed 3,350 library tests but found one further catalog-summary
cursor assumption, subsequently repaired. A final-code full run then passed 3,350
library tests but failed `fastmcp4_availability_selection_request_and_invocation_share_one_authority`
at `tests/boundary/managed_selection.rs:89` with
`ToolActivation("selected Agent capability cannot be admitted: Tool(SourceUnavailable { selector: \"source:python:healthy/ping\", source: ManagedPython(\"healthy\"), reason: Unavailable { reason: \"source preparation failed\" } })")`.
That diagnostic does not establish a PyPI/network cause; it is recorded as an
intermittent managed-source preparation failure, not a snapshot regression or a
passing full run. No managed-source implementation or test was changed. The subsequent complete
rerun passed all 3,963 tests, including this managed-source test; the earlier
failed run remains recorded above.

The first focused run exposed old cursor-equality assumptions across asynchronous
background enrichment; tests now validate the exact allowed replay suffix. An early
capture attempted to hold mailbox publication through projection capture; the existing
snapshot race test exposed that unwanted ownership, so pending readback again releases
its publication guard before projection capture and is accepted only at its fence.
The first finite inspection traversal used an indexed-query API that requires a
nonempty kind selection; inspection now uses bounded ordinary Journal pages filtered
through its captured prefix. Clippy requested explicit collection constructors and a
separate focused Tool-association test; both were corrected.

No new PR/issue, self-merge or CI watch. After push only one bounded PR/head/state
verification is performed.

Both final jsdom and Chromium measurement JSON values exactly match committed
`evidence/after` records. Before/after metrics and fixtures are unchanged.
