# Completed-response tails (#367)

The native Runtime Client derives one tail for the last accepted canonical
Assistant message of a successfully completed Attempt. Intermediate requests,
Tool calls, and interrupted output do not acquire completed tails. The Message
Ledger remains canonical; this projection persists no telemetry or second
message. App Server exposes the same projection to Web, TUI, and headless clients.

The historical cut is the first Surface append revision of that exact closing
message. Native validation rejects a mismatched revision or response. Branch and
independent Fork both retain the response (`after`); Retry retains its existing
pre-User cut (`before`) and replays canonical returned input once. Catalog version
12 records the explicit side; obsolete development catalogs are refused, with no
migration or fallback.

Usage folds existing durable normalized request terminals within the exact
Attempt. Response totals require every request to report usage; optional buckets
require every included report to provide that bucket. Conversation totals describe
the current Conversation's execution epoch and disclose usage-report coverage.
They are not loaded-window totals and are not inherited execution totals of a
forked Conversation. Context Engine exposes only the latest provider-measured
request's input and frozen model capacity; compaction or a newer unmeasured request
invalidates it. The UI explicitly calls it “Last request context”.

## Dependency boundary

At validation, #364 is still open as PR #368, head
`b1dfc36f4babb1112cc2a606a2e73342a3895895`; its generation evidence has not landed
on main. Its request-owned monotonic timing contract was inspected read-only.
The final read-only worktree check also found a newer local #364 commit,
`bc832d4f`, adding optional `dispatch_after_start_ms` to bridge the durable start
origin to dispatch. That revision is not on main or the PR head yet. This confirms
that the timing contract is still evolving; no copy of either revision is included.
This change consumes existing normalized usage and does not copy the in-flight
implementation, invent a timing schema, or infer durations from wall-clock events.
Consequently no runtime/TTFT/throughput control appears yet. When #364 lands,
`runtime_client::response` is the native integration point for aggregating that
contract to a completed response; React must not join individual request evidence.

## Deterministic regression mapping

| Acceptance | Evidence |
| --- | --- |
| User Copy/time without primary lineage toolbar; Assistant Copy excludes reasoning | `web-console/test/response-tail.test.tsx` |
| One Attempt, multiple requests and Tool-call content, exactly one closing tail | `runtime_client::response::tests::one_attempt_many_requests_has_one_exact_tail_and_missing_buckets_stay_absent` |
| Interrupted output, missing usage and optional buckets, no fabricated timing | native response tests and Web response-tail tests |
| Exact historical response identity across pages, reopen, newer Attempts, finite read cuts | native response tests |
| Compaction preserves complete response metadata/cut and cumulative totals | native real-compaction response regression |
| Native request/model context, invalidation after compaction/new request | native response context regression and composer component test |
| Branch/Fork include Assistant B, empty composer, distinct Session ownership; Retry excludes User B and preserves source | session catalog regression and scripted App Server `completed_response_cut_is_shared_by_branch_and_fork_and_distinct_from_retry` |
| Mismatched revision and non-Assistant after-cut refused | scripted App Server regression |
| No stale replay; uncertain admission and lineage-switch safety | existing rewritten `commands.test.tsx` and native lineage suites |
| Late history page cannot replace newest totals; unresolved off-window response is reread | Web response-tail/transcript tests |
| Real upload-bearing Retry, independent post-response Fork, copied upload ownership | pinned-container `commands.spec.ts` and `uploads.spec.ts` |
| 34 paged native tails, pointer reveal, keyboard focus/activation, narrow layout | pinned-container `chat.spec.ts` |
| Product keyboard/editor reachability at 390/820/1280/1600px | pinned-container `accessibility.spec.ts` |
| Light/dark desktop/narrow presentation references | pinned-container `agent.spec.ts` (obsolete User-toolbar references replaced) |
| MIT source inventory, pinned Harness and dependency boundary | provenance/reference audit and notice validation |

No new synchronization test uses sleeps. Native tests construct exact durable
identities or use existing gates; browser tests wait on native settlement or
observable UI conditions.

## Validation commands

Commands run in the isolated issue worktree. Repeated development runs are
collapsed below; early fixture/schema/screenshot failures were corrected and
revalidated. All existing screenshot references were generated only by the
repository-pinned Playwright container.

- `pnpm --dir {web-console,tui,protocol/app-server} install --frozen-lockfile`
  (each package separately).
- `cargo check --lib --all-features`.
- `cargo fmt --all` and `cargo fmt --all -- --check`.
- `cargo clippy --all-targets --all-features -- -D warnings`.
- `cargo test --all-features runtime_client::response::tests`.
- `cargo test --all-features completed_response_cut_is_shared`.
- `cargo test --all-features post_response_fork_and_branch_include_the_response_and_retry_excludes_input`.
- `cargo test --all-features`; also `RUST_TEST_THREADS=4 cargo test --all-features`.
  Parallel runs exposed intermittent managed-Python source preparation failures;
  `cargo test --all-features boundary_suites::runtime_client::python_capability -- --test-threads=1`
  passed both. A full `RUST_TEST_THREADS=1` run was stopped because the inherited
  setting changes an existing subprocess gate test’s stdout framing. The final
  full run uses `cargo test --all-features -- --test-threads=1`, which does not
  alter child-process test scheduling.
- `cargo build --bins`.
- `pnpm --dir protocol/app-server generate`, `check`, and `typecheck`.
- `pnpm --dir tui typecheck` and `pnpm --dir tui test`.
- `pnpm --dir web-console typecheck`, `test`, `check:provenance`, and `build`.
- `node web-console/scripts/provenance.ts --reference /home/caismis/Documents/codes/deepseek-harness-364`.
- `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests`.
- `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e --config /tmp/rustx-367-playwright.config.ts`:
  Chat/commands/uploads; all other native browser suites; all static reference
  suites plus development launcher use `/tmp/rustx-367-playwright-static.config.ts`. Agent reference updates used
  `agent.spec.ts --update-snapshots`, followed by comparison without updating.
- `git diff --check`.

The Browser plugin and Docker are unavailable here; the frontend-testing skill's
fallback is the repository's pinned Playwright image through Podman. Existing
5173/5174 servers belong to other worktrees, so temporary external configurations
use 53673–53675. Static fixture URLs are temporarily redirected for validation and
restored byte-for-byte afterward. No other checkout or server is modified.

## Final results

| Check | Result |
| --- | --- |
| Full Rust, all features, all targets and doctests | PASS: 3,735 passed, 0 failed, 6 existing ignored |
| Rust formatting / Clippy with warnings denied / whitespace | PASS |
| Protocol regeneration, committed-file freshness and TypeScript contracts | PASS |
| TUI typecheck / tests | PASS: 799 tests |
| Web typecheck / unit and component tests | PASS: 477 tests |
| Fake-provider Python suite | PASS: 51 tests |
| All browser suites in pinned container | PASS: 40 tests (3 Chat/lineage/upload, 13 other native, 24 reference/launcher) |
| Source inventory / approved-reference audit / notices / production build | PASS: 107 source records, 100 production package notices |

The final upstream check remains `a775e7094a7664326da1f81861deec704d0baca7`;
no rebase was required. #364/PR #368 remains open, so timing integration is the
explicit dependency limitation above. The environment issues in earlier runs
were resolved through isolation/test invocation; no production behavior or test
expectation was weakened to accommodate them.
