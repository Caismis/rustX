# Local validation ledger

All implementation/validation ran in the issue worktree. The primary worktree was
read-only. No GitHub CI checks were watched. Commands below are the actual lanes;
repetitions with the same arguments are grouped. Logs were retained locally under
`/tmp/rustx-420-*.log`; durable measurements are in `evidence/`.

## Final results and exact commands

| Command (repository root unless stated) | Result |
| --- | --- |
| `git fetch origin` | Initial and bounded final synchronization; origin/main remained `ad863a24cf48fbb0d5182746e1046d4b64e8c167`. |
| `pnpm --dir protocol/app-server install --frozen-lockfile`; equivalent installs in `tui` and `web-console` | Dependencies installed; baseline Web dependencies installed separately. |
| `cargo check --all-targets --all-features` | Pass after implementation compile repairs. |
| `cargo fmt --all -- --check` | Pass, including final source commit. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass, including final source commit. |
| `cargo test --all-targets --all-features` | **Not green**: final run 3,334 passed, five managed-Python preparation failures, three existing ignored tests. See environment limitation below. |
| `cargo test --test '*' --all-features` | Pass: 611 integration tests, five existing ignored fixture/measurement tests. Includes conformance, protocol contracts, actual processes and transports. |
| `cargo test --bins --examples --all-features` | Pass; runs remaining targets separately because the all-targets command stops after library failures. |
| `cargo build --bins` | Pass; real browser/native process lanes use built binaries. |
| `cargo test --lib launch --all-features` | 69 passed. |
| `cargo test --lib a_parked_projection_fold_cannot_retain_the_host_or_storage_authority --all-features -- --nocapture` | Pass after the short Store-read gate repair. |
| `RUSTX_PROJECTION_CAPTURE=web-console/test/fixtures/incremental-native.json cargo test --lib incremental_projection_independent_snapshot_capture --all-features -- --nocapture` | Pass; native snapshots/events captured independently of the TypeScript fold. |
| `(cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck)` | Pass; mandatory v25 schema and TypeScript have no drift. |
| `(cd tui && pnpm typecheck && pnpm test)` | Pass: 852 tests. |
| `(cd web-console && pnpm typecheck)` | Pass. |
| `(cd web-console && pnpm build && pnpm check:i18n && pnpm check:provenance)` | Pass; existing bundle-size advisory remains. No missing English/Chinese strings or source attribution. |
| `(cd web-console && pnpm test)` | Executed repeatedly. Last unrestricted concurrent run: 1,075 passed, performance fixture exceeded its unchanged 5s limit under other concurrent work. |
| `(cd web-console && RUSTX_PERFORMANCE_OUTPUT=/tmp/rustx-420-after.json pnpm test --maxWorkers=4)` | **1,076 passed**, all 63 files. Same assertions and deadlines; bounded workers only. |
| `(cd web-console && RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 CONTAINER_ENGINE=podman pnpm test:e2e)` | **125 passed**, 6.5 minutes. Uses repository pinned Chromium container and actual native process lanes. |
| `(cd web-console && RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 CONTAINER_ENGINE=podman bash scripts/browser-tests.sh incremental.spec.ts trajectory.spec.ts trajectory-integration.spec.ts trajectory-timing.spec.ts convergence.spec.ts --output=/tmp/rustx-420-browser-after-8acb)` | Pass: **31 tests** after final independent Trace paging/recovery changes, at exact source commit `8acb8924`. |
| `git diff --check` | Pass before implementation commit and final evidence commit. |

Focused Web commands also executed with `pnpm test` plus these file groups:
`test/incremental-client.test.ts` (7), `test/trace-cache.test.ts` (19),
`test/incremental-equivalence.test.ts test/incremental-vocabulary.test.ts`,
`test/incremental-presentation.test.tsx test/agent.test.tsx` (15),
`test/scroll.test.tsx test/incremental-vocabulary.test.ts` (4), and the performance
fixture alone on both worktrees (1 each). All final focused runs passed; the full
1,076-test run includes every new test. The focused browser repair run used
`bash scripts/browser-tests.sh agent.spec.ts incremental.spec.ts` (15 passed).
The acceptance-to-test mapping is in [README.md](README.md).

## Reproducible measurement commands

At implementation source commit `8acb89249361a90dba4e4b2fc998983fd691aaae`:

```sh
cd /home/caismis/Documents/codes/rustX-issue-420/web-console
RUSTX_PERFORMANCE_OUTPUT=/tmp/rustx-420-after-8acb.json pnpm test test/incremental-performance.test.tsx
RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 CONTAINER_ENGINE=podman \
  bash scripts/browser-tests.sh incremental.spec.ts trajectory.spec.ts trajectory-integration.spec.ts trajectory-timing.spec.ts convergence.spec.ts --output=/tmp/rustx-420-browser-after-8acb
```

At detached baseline `ad863a24cf48fbb0d5182746e1046d4b64e8c167`:

```sh
cd /home/caismis/Documents/codes/rustX-issue-420-baseline/web-console
RUSTX_PERFORMANCE_OUTPUT=/tmp/rustx-420-before-ad863.json pnpm test test/incremental-performance.test.tsx
RUSTX_E2E_PREVIEW_PORT=15573 RUSTX_E2E_FIXTURE_PORT=15574 CONTAINER_ENGINE=podman \
  bash scripts/browser-tests.sh incremental.spec.ts --output=/tmp/rustx-420-browser-before-ad863
```

To reconstruct baseline instrumentation, copy the new performance test,
`test/fixtures/incremental.{html,tsx}`, `test/e2e/incremental.spec.ts`, and the bounded
port Playwright configuration from the implementation commit. Change protocol
imports from v25 to v24 and omit the new `arguments_json` property in the two fixture
sources. In the baseline browser expectations use 209 snapshots and one row
replacement. Do not alter fixture content, geometry, notifications or observation
window. Build that baseline Web before running its browser lane. Baseline source
was never reset into the original development worktree.

## Development failures, diagnosis and repair

- Initial native/TypeScript compile errors reflected new DTO exhaustiveness,
  generation imports and optional-field shapes. Generated v25 consumers and
  fixtures were updated together. Clippy caught function length; native read
  synchronization was factored by responsibility. Final fmt/clippy/types pass.
- Native cursor/Goal/terminal regressions exposed initial bootstrap cursor
  advancement and live snapshot reads replacing native decorated transcript data.
  Bootstrap initializes without publishing; live snapshots read the established
  cut. Settlement now follows derived read domains. Existing lifecycle regressions
  pass in the final native library run.
- Initial worker reads held Store authority too long, causing physical-settlement
  and conformance storage-lock failures. A weak Store reference and short read
  gate fix ownership. An intermediate Drop implementation joined the whole state
  mutex and hung the parked-fold proof; only our own identified test processes
  were stopped. Drop now joins only synchronous Store reads. The parked-fold
  proof, launch suite and all 22 conformance tests pass.
- Independent native equivalence initially exposed missing inbound revision,
  omitted optional fields, transcript decorations and Tool JSON number spelling.
  These were corrected in the actual payload/fold, without stripping Session
  fields. Only the documented separate Trace read domain is excluded.
- Old TUI snapshot-loop and Web snapshot-after-event assertions were replaced with
  cursor/read-domain contracts. Early held-response tests needed to answer the
  new independent Trace RPC; final Trace tests pass with zero Session snapshots.
- Reasoning initially remounted because commit moved it into a different rendering
  branch. One component path now preserves the same expanded DOM. Initial process
  placement changed six screenshots; its keyed seat now remains after committed
  rows and before streaming rows. Existing screenshot references were not rewritten.
- Browser revision tests formerly waited for snapshot observations; they now wait
  for the actual native Goal event. Full browser runs progressed from 122/124 and
  119/125 passing to 125/125, plus the final affected rerun.
- Default port 5173 was occupied. No unrelated process was terminated; bounded
  environment-selected ports preserve default repository behavior.
- Measurement fixture revisions aligned reasoning/Tool final content and disabled
  browser CSS anchoring consistently. Only final identical v3 runs are compared.
  Earlier exploratory recordings/counters are not delivery evidence.
- Unrestricted Vitest concurrency caused one performance fixture timeout. Four
  workers passed the full suite without changing the test's 5s deadline.
- A serial native attempt (`RUST_TEST_THREADS=1 cargo test --all-targets --all-features`)
  stalled in an existing borrowed-workspace child stdout gate: serial harness output
  shares the marker line. Only that run's identified parent/child processes were
  stopped; the test and assertions were untouched. Default concurrency passes it.
  A four-thread attempt (`RUST_TEST_THREADS=4 ...`) still encountered PyPI failures.

## Environment limitation (not reported as passing)

The final all-target native run stops after five existing managed-Python lanes:

- `boundary_suites::mcp_tasks_managed::a_real_managed_fastmcp_task_completes_through_one_tool_result`
- `boundary_suites::mcp_mrtr_managed::a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction`
- `boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`
- `boundary_suites::runtime_client::python_capability::capability_projection_covers_native_python_and_skills`
- `boundary_suites::runtime_client::python_capability::capability_projection_covers_python_origins`

`uv lock --no-config` under CPython 3.14.7 cannot fetch
`https://pypi.org/simple/fastmcp-tasks/` / `fastmcp/`: **Network is unreachable
(os error 101)** after its existing retries. The other three report managed source
preparation failure. Earlier runs had 3,339 library tests pass but a then-unfixed
conformance ownership failure; that historical result is not presented as a green
final all-target run. The focused managed-selection rerun also failed source
preparation. No dependency substitution, skipped gate, deadline increase or product
fallback was introduced. Integration targets and binaries/examples were explicitly
run separately and passed. External hosted-provider latency and GitHub CI were not
measured or claimed.

## Final architecture review

Reviewed the complete diff against origin/main. Ordinary contiguous events never
request a Session snapshot, including non-text events; native publication suffixes
never rebuild the durable transcript. Explicit mutation reconciliation and recovery
remain allowed. Trace paging/latest use only Trace reads. Chat contains exactly one
automatic scroll assignment. Exact attachment target and connection epoch fence
both replies and events. Canonical commitment removes transient state while retaining
one MessageSeat component path. Transcript subscribers retain unrelated references;
controls still receive shutdown/attachment/lineage facts. No global store, provider
assembler, compatibility decoder or old generated v24 module remains. FirstSubmissions
changed only its protocol type import. Current architecture docs and provenance
agree with the delivered ownership. Included recordings are the requested bounded
performance evidence; no local configuration, unrelated cleanup, test-results tree
or primary-worktree untracked files are included.
