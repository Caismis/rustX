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

## Native lineage inheritance

Local execution facts stay in the original Conversation's Journal. Previously the
read model used only that Journal, so a copied Ledger/Surface lost all finalized
response semantics. Now `LineageSeed` additionally carries validated immutable
`CompletedResponseProvenance`: closing message, optional preceding Retry input,
original Conversation/Attempt/closing identity, native completion timestamp and
optional exact usage. It is persisted atomically in `bootstrap_identity` alongside
the seeded history, not in another Chat telemetry store. Local execution continues
to be derived from its Journal; only inherited response meaning is snapshotted.

The canonical/Surface identity map also remaps closing and Retry IDs. Origins
remain original provenance, never destination content addresses or execution
ownership. Missing retained input removes Retry. Seed validation rejects duplicate
or non-Assistant anchors, Tool-call anchors, and invalid preceding input. Repeated
bootstrap initialization cannot replace these facts; ordinary reopen retains them.
This works identically for source → branch A → branch B → fork C, even after the
original execution owner is unavailable. No copied or synthesized Attempt/request
Journal events, request snapshots, or recovery state are needed.

`CompletedResponseView.origin` makes original execution ownership explicit. Its
`closing_message_id`, `retry_message_id`, and `surface_revision` always address the
current destination. The native `After` validator checks the exact destination
append revision plus the shared finalized-response projection. `Before` still
excludes/replays the selected User once. Source Assistant history is immutable.
SQLite schema 41 and catalog schema 12 reject obsolete development state; there
is no migration or compatibility path.

Inherited usage belongs to the historical response. Current Conversation
statistics count only current-Conversation execution events, so a newly copied
Conversation has zero requests/responses and no cumulative usage despite visible
historical tails. Its context occupancy is absent until its own request supplies
provider/model evidence. Compaction changes neither historical response identity
nor cumulative execution totals.

## Dependency boundary and timing

PR #368 remains **open** at `14f24082845988be97d562ad1593caf26314925e`. Its preceding
head passed all seven CI jobs; the latest head is undergoing CI. It has not landed in `origin/main` (`a775e709`). The intended
order is #368 → rebase #369 → timing/protocol integration. #369 is **not ready to
merge** until that final integration and its response-level timing regressions
are complete. No dependency code was copied from its active worktree or branch.

The inspected current native contract is request-owned `GenerationEvidence` on
completed/failed request terminals:

- `dispatch_after_start_ms` is an optional measured monotonic bridge from the
  durable-start clock pair to dispatch. Missing bridge forbids start-relative
  phase positions.
- `first_output_ms` is dispatch-to-first provider-independent output (text,
  reasoning, refusal, or assembled Tool call). This is the contract's TTFT.
- `last_output_ms` is the last such output; provider framing is not output.
- `terminal_ms` is the provider terminal offset, not canonical acceptance.
- `generation_ms()` is checked `terminal_ms - first_output_ms`.
- Throughput requires output usage and a strictly positive generation interval.

These definitions must be consumed, not replaced. For multi-request responses,
any summed model-generation duration must require every included request's native
evidence; throughput must divide correspondingly covered output usage by that
positive summed duration. A request-local TTFT must not be labeled whole-Attempt
latency. Total user-visible execution includes Tool/inter-request time; summing
request spans or subtracting Journal wall timestamps cannot establish that total.
The post-#368 integration must emit only aggregate metrics supported by measured
clock relationships, with unsupported metrics absent. This revision does not add
unused timing DTO fields or fabricate any of those measurements.

Main currently mandates App Server v8. The branch regenerates only v8 from its
actual Rust DTOs. #368 replaces it with v9; the required rebase must remove v8,
regenerate v9, update imports, and add one-/multi-request, missing-bridge, positive
throughput and reopen regressions against the landed native contract. The latest
#368 also reserves SQLite schema 41 for generation evidence. This branch independently
requires schema 41 for bootstrap provenance against current main; the post-#368
rebase must advance the combined schema to 42 (or the next current version),
rejecting both obsolete shapes rather than treating them as compatible.

## Projection cost

One shared native fold serves response decoration and lineage provenance. The
lineage owner reuses its result for `After` validation, avoiding a second Journal
scan. It reads a finite published prefix in indexed 128-event batches and retains
requested response identities. It is still O(J + R) per projection (Journal facts
plus inherited bootstrap summaries), not an incremental cache. The inspected
#364 Trace implementation has bounded indexed reads but no reusable incremental
response/statistics accumulator. A native checkpoint/index is a bounded future
performance task; current paging/read-cut correctness does not depend on it.

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

## Original implementation validation results (before this revision)

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

The original upstream check was `a775e7094a7664326da1f81861deec704d0baca7`;
no rebase was required. Timing integration remains the
explicit dependency limitation above. The environment issues in earlier runs
were resolved through isolation/test invocation; no production behavior or test
expectation was weakened to accommodate them.

## PR #369 revision: lineage inheritance

Starting HEAD: `cf2956df0cbd6efcd68289071d3d5844cc89e8a0` in the same isolated
Issue #367 worktree/branch. No unrelated worktree was edited or rebased.

New deterministic regressions:

- `runtime_client::response::tests::lineage::deep_lineage_reopen_preserves_response_facts_without_execution_ownership`:
  two source responses, three successive remaps, both tails preserved, exact
  original metrics/origin, destination closing/Retry IDs, identical reopen and
  paging, immutable bootstrap, empty Journal/frontier, zero destination totals,
  absent inherited context occupancy, then only local execution counted.
- `remapping_drops_missing_retry_input_and_bootstrap_rejects_invalid_response_addresses`:
  omitted input disables Retry; duplicate/non-Assistant anchors and invalid
  preceding input are rejected.
- The real-compaction response regression now also copies compacted history and
  verifies inherited completion/usage and the destination execution-epoch rule.
- The scripted App Server Branch/Fork regression now seeds authoritative request
  usage, attaches each child, reopens it, Branches and Forks again, checks remapped
  Retry input and its exact prefix, refuses stale destination revisions, and
  confirms source preservation and absence of copied execution events.
- Pinned-browser `commands.spec.ts` now reconnects to the inherited Fork tail,
  Branches again, submits inherited Retry once through a provider gate with copied
  uploads, and returns to the earlier lineage to prove its Assistant is unchanged.

The final timing integration remains dependency-blocked by unmerged #368. No
#364 generation tests can meaningfully run on this main base; running an absent
filter and reporting zero selected tests as success would not validate it.

### Validation commands for this revision

- `cargo check --lib --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features runtime_client::response::tests`
- `cargo test --all-features completed_response_cut_is_shared`
- `cargo test --all-features lineage`
- `cargo test --all-features local_runtime::session_runtime_manager::tests::protocol`
- `cargo test --all-targets --all-features -- --test-threads=1`
- `cargo test --doc --all-features`
- `cargo build --bins`
- `pnpm --dir protocol/app-server generate`, `check`, `typecheck`
- `pnpm --dir web-console typecheck`, `test`, `check:provenance`, `build`
- `node web-console/scripts/provenance.ts --reference /home/caismis/Documents/codes/deepseek-harness-364`
- `pnpm --dir tui typecheck`, `test`
- `pnpm --dir dev typecheck`, `test`
- `uv run --project test-support/fake-provider --frozen pytest test-support/fake-provider/tests`
- `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e --config /tmp/rev367-playwright.config.ts`
- The same browser command with `agent.spec.ts --grep 'Harness Agent error'`
  for the pre-revision fixture control, then `--update-snapshots` for the single
  reviewed reference, followed by the entire suite without updating.
- `git diff --check`

The first full Rust run found two explicit version-40 assertions in obsolete-store
rejection tests; these now assert 41, retaining all rejection checks. The first
Clippy pass found a long three-generation test, now annotated consistently with
other complete native lifecycle scenarios.

The first two full browser runs passed 39/40. The only failure was one comparator
pixel in `agent-error-light.png` (eight raw RGB differences in a 4×2 composer-corner
area). It also reproduced with the pre-revision fixture and in the previous PR
HEAD's [Full Web CI run](https://github.com/Caismis/rustX/actions/runs/35416757775/job/105826795197).
No production Web source changed. The image was visually inspected and this one
stale raster reference regenerated by the pinned browser. No threshold, assertion,
UI component, or style was relaxed or changed to accommodate it.

### Final revision results

All validation commands above passed after the corrections described above.
The full Rust run passed **3,728 tests, zero failures, six existing ignored**;
all **nine doctests** also passed. Focused response tests passed 8/8 and scripted
App Server protocol tests passed 33/33. Web passed 477 tests, TUI 799, developer
tools 37, fake provider 51, and the final complete pinned-browser suite **40/40**.
Formatting, strict all-target/all-feature Clippy, protocol generation/freshness,
all typechecks, production build, source/reference/notice audits, and staged and
unstaged whitespace checks passed. Freshness was checked against staged generated
DTO artifacts; only mandatory v8 artifacts exist on this main base.

Browser testing used the pinned Playwright container with Podman because the
Browser plugin was unavailable. Isolated ports 53674/53675 avoided other active
worktrees; temporary fixture URL overrides were restored. No production Web
component or style changed in this revision.

Final fetch still reported main `a775e7094a7664326da1f81861deec704d0baca7` and
#368 open at `14f24082845988be97d562ad1593caf26314925e`; no rebase was required.
Generation-evidence/timing regression execution remains blocked on that unmerged
dependency. This is an explicit outstanding integration requirement, not a claim
that the final #364 contract has been integrated or tested here.
