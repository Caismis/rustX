# Issue #430 validation

The final reading contract is [conversation-reading.md](conversation-reading.md).
App Server v30 is the only negotiated/generated protocol. Runtime Client v55
and subagent IPC v29 retain their separate version ownership.

## Deterministic regression map

| Contract | Exact regression |
| --- | --- |
| More than 512 entries, exact native destination, 64-entry direct window, 240-character preview, adjacency, reads commit no work | `durable::sqlite::reading_tests::distant_turn_windows_are_exact_bounded_and_read_only` |
| Attempt ordering/identity, empty failed terminal, pending disabled location and paging | `durable::sqlite::reading_tests::paging_failed_empty_and_automatic_attempts_keep_native_identity_order` |
| Copied origins map to destination positions | `durable::sqlite::reading_tests::copied_turns_preserve_origin_and_use_destination_locations` |
| Pending edits/removals retire cuts while turn identity/location stays stable; conflicting edits do not retire cuts; removed tail creates no phantom newer page | `durable::sqlite::reading_tests::pending_edits_and_removals_retire_cuts_without_moving_turns` |
| Stale/foreign cut or turn is rejected without fallback | `durable::sqlite::reading_tests::stale_cuts_and_foreign_turns_are_rejected_without_nearest_fallback` |
| Actual Human work and automatic Goal continuation produce two turns from three model requests; reads do no work; interrupted continuation keeps identity and exact terminal | `scripted_suites::extensions::goal351_model_create_goal_starts_no_nested_attempt_and_continues_after_settlement` |
| Compaction preserves origins/order and retires the previous cut | `runtime_client::response::tests::real_compaction_preserves_response_identity_cut_and_cumulative_usage` |
| Three-generation branch/fork/reopen preserves original identities, destination locations and inherited-before-local order | `runtime_client::response::tests::lineage::deep_lineage_reopen_preserves_response_facts_without_execution_ownership` |
| Unfinished copied content creates no fabricated Attempt provenance | `runtime_client::response::tests::lineage::unfinished_process_content_crosses_lineage_without_source_execution_outcome` |
| Window annotation preserves exact Attempt, response identity and Tool/result occurrences | `runtime_client::response::tests::exact_window_decoration_keeps_attempt_tool_occurrences_and_response_identity` |
| Old protocol generations are all rejected transactionally | `local_runtime::session_runtime_manager::tests::protocol::initialize_and_malformed_wire_are_transactional` |
| Actual provider-invocation barrier keeps the intended queue item pending before exercising committed/unpublished edit/removal repair | `runtime_client::host::tests::pending_snapshot_and_reattach_repair_a_committed_unpublished_mutation` |

`web-console/test/reading-navigation.test.ts` uses held native requests:

- `only newest navigation installs and requests scroll for reply order 1,0`
  and `... reply order 0,1` prove both A/B completion orders.
- `a >512-entry distant turn uses one direct read with bounded transcript and outline caches`
  proves the destination, one native read, bounded caches and historical/latest live behavior.
- `session replacement fences a gated navigation without changing the replacement view`
  and the corresponding `attachment`, `node`, `runtime`, `generation`, `resync`,
  `user` cases prove authority retirement.
- `outstanding navigation reads are bounded at two and newer intent retires both`
  proves finite transport work with no queued third read.
- `a loaded exact native anchor at the same cut commits without a second read`
  proves the native-keyed cache optimization.
- `a mismatched native journal cannot install or leave pending navigation stuck`
  and the corresponding `pending_count`, `pending_revision`, `target` cases
  proves validation before install.
- `outline read survives a window switch but a resync retires it synchronously`
  proves independent outline paging authority.
- `ordinary older paging is superseded by a newer navigation intent` and
  `an ordinary prepend read survives user detachment without claiming a replacement navigation`
  distinguish replacement intent from safe ordinary cache merging.

`web-console/test/scroll.test.tsx` controls ResizeObserver and animation frames:

- `preserves the stable reading anchor for prepend and growth, follows only at bottom`;
- `one frame owns 10 observer deliveries and 5 React updates; newer user intent wins`;
- `short-history prepend retains reading ownership through disappearing anchors and shrink`;
- `ordinary short detached reading exposes Return to latest and subsequently follows streaming`;
- `newer native user scroll retires navigation both before reply and before its layout frame`;
- `target replacement exits follow, preserves reflow and missing anchors never enter follow`;
- `authority replacement after native installation still retires the scheduled navigation frame`.

The reflow tests change semantic row geometry for Tool/image/content growth and
width/sidebar/right-panel geometry, including missing-anchor fallback. They
assert actual scroll positions, not only observer delivery or a sent request.

`web-console/test/conversation-width.test.tsx` has 16 regressions: both captured
handle drags and frame coalescing; sub-threshold press; pointer cancel, lost
capture and Escape; measured sidebar/right-panel/narrow clamps and preference
restoration; seven corrupt values; blocked/absent storage; keyboard bounds and
reduced motion. `turn-navigator.test.tsx` proves the bounded 64 marks, hover/focus
preview retention and keyboard focus without implicit navigation. Existing
incremental-rendering/streaming isolation tests remain in the full Web suite.

## Real-browser acceptance

`web-console/test/e2e/reading.spec.ts` creates 300 real native Attempts (600
transcript entries), using native terminal events as preparation barriers. The
first target is 535 positions before the latest window. One held native locate
read proves visible busy state, exact anchor, active state and bounded rendering.
A rejected unloaded target keeps the old window and exposes reload.

Provider gates prove ordinary detached streaming holds position and Return to
latest restores following for subsequent output. The same native Session tests
rail keyboard/hover/focus, resize/keyboard preference, 390px layout, English,
Chinese, reduced motion and no horizontal overflow. The seed fixture has a
separate 240-second preparation budget; browser assertions retain the existing
120-second deadline and have no retries or sleep-based race proof.

Supplementary host Chromium runs passed this acceptance and the existing native
Chat, typed command/lineage/upload, and two-Session/disconnect/interaction suites.
They do not replace the pinned container's screenshot or full-suite authority.
No reference screenshots were changed.

## Environment limitations

The required `pnpm --dir web-console test:e2e` was attempted with the final
production build. Vite and artifact provenance passed, but Docker could not pull
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`:
the configured proxy returned `Forbidden` (exit 125). Full pinned E2E and exact
reference comparisons remain unverified locally.

Five in-crate boundary tests and seven external Tool tests fail during managed
Python environment preparation. Native ToolEnvironment clears inherited proxy
variables; this environment cannot resolve `pypi.org` for `uv lock --no-config`.
The production environment policy and test assertions were retained. Every
applicable external boundary target was executed; none was skipped to obtain a
passing result.

The affected in-crate tests are:

- `boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`;
- `boundary_suites::mcp_mrtr_managed::a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction`;
- `boundary_suites::mcp_tasks_managed::a_real_managed_fastmcp_task_completes_through_one_tool_result`;
- `boundary_suites::runtime_client::python_capability::capability_projection_covers_native_python_and_skills`;
- `boundary_suites::runtime_client::python_capability::capability_projection_covers_python_origins`.

The external failures are `mcp_managed::a_real_managed_child_negotiates_the_modern_mcp_revision`,
`cold_python_composition_rereads_sources_while_loaded_capture_is_frozen`,
`one_connected_runtime_reuses_one_process_across_calls`,
`one_folder_serves_multiple_tools_through_one_server_identity`,
`stderr_diagnostics_do_not_participate_in_framing`,
`two_folders_prepare_distinct_environment_identities`, and
`uv::production_uv_materializes_a_managed_package_environment` (all but the last
under `mcp_managed::`).

macOS platform-boundary CI was unavailable locally. Linux builds used
`CARGO_INCREMENTAL=0` and `CARGO_BUILD_JOBS=1`; completed generated worktree test
executables had only debug symbols removed to fit the 32GiB filesystem. An early
all-targets compilation/linking exhausted disk (including a linker bus error) and was rerun after generated-cache
cleanup. The primary checkout and its build artifacts were left untouched.
The dev process-group tests also needed a Linux child-subreaper wrapper because
this container's PID 1 does not reap orphaned children; the unchanged suite then
passed all 38 tests.

During validation, a stale test-only hardcoded supported version was replaced
with `APP_SERVER_PROTOCOL_VERSION`. Browser preparation text was made distinct
to avoid the existing native repetition guard; the guard stayed enabled.
Ordinary prepend/user-scroll cancellation was corrected and covered by a new
regression. Early contended Web runs had settings-page deadline failures;
final full-suite runs retained all assertions/deadlines and passed.

## Executed validation commands

Commands use pnpm 11.13.1 through Corepack, from the named package directory
(equivalent to `pnpm --dir <directory> ...`). Rust runs set
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`; provider-bearing runs also set
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` and use the configured Rust/uv toolchains.
Repeated development runs are grouped by command, with final results and
resolved earlier failures recorded.

| Directory | Command | Result / relevant count |
| --- | --- | --- |
| Web, protocol, TUI, dev | `corepack pnpm install --frozen-lockfile` | Passed; no dependency/lockfile changes |
| `web-console` | `corepack pnpm typecheck` | Passed |
| `web-console` | `corepack pnpm test` | Passed: 71 files, 1,314 tests |
| `web-console` | `corepack pnpm exec vitest run test/reading-navigation.test.ts test/conversation-width.test.tsx test/scroll.test.tsx test/turn-navigator.test.tsx` | Passed: 4 files, 43 tests (19 navigation, 16 width, 7 scroll, 1 rail) |
| `web-console` | `corepack pnpm check:i18n` | Passed |
| `web-console` | `corepack pnpm check:provenance` | Passed: 147 source records, 131 production-package notices |
| `web-console` | `corepack pnpm build` | Passed; final E2E command also rebuilt production and verified artifact provenance |
| `web-console` | `corepack pnpm test:e2e` | Failed before browser execution: pinned-image pull forbidden, Docker exit 125 |
| `web-console` | `corepack pnpm exec playwright test reading.spec.ts` | Supplementary host Chromium passed: 1 test |
| `web-console` | `corepack pnpm exec playwright test chat.spec.ts commands.spec.ts console.spec.ts` | 2 passed, Chat failed; ordinary-prepend defect repaired and Chat rerun below |
| `web-console` | `corepack pnpm exec playwright test reading.spec.ts chat.spec.ts` | Final production contract: 2 passed; EN/ZH, reduced motion, distant native navigation, streaming, measured width |
| `protocol/app-server` | `corepack pnpm generate` | Passed: native v30 schema/fixtures and generated TypeScript |
| `protocol/app-server` | `corepack pnpm check` | Passed after committing intended artifacts: regeneration produced no diff |
| `protocol/app-server` | `corepack pnpm typecheck` | Passed |
| repository | `cargo fmt --all -- --check` | Passed |
| repository | `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed, including rerun after final test-barrier repair |
| repository | `cargo build --bins --all-features --locked` | Passed after generated-cache cleanup |
| repository | `cargo test --all-targets --all-features --locked` | 3,423 passed, 5 managed-Python failures, 3 ignored; Cargo stops after failed lib target. Other CI targets executed below |
| repository | `cargo build --bins --locked` | Passed: actual CI default-feature prerequisite |
| repository | `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | Final pass: 3,234 unit/contract tests, 3 ignored, 194 boundary tests excluded; binary harnesses 0 tests; examples 17 checker + 3 benchmark tests passed. Earlier obsolete supported-version assertion and premature pending-queue test signal were repaired |
| repository | `cargo test --test contracts --test provider --all-features --locked` | Passed: 28 contracts + 168 provider; 5 opt-in tests ignored |
| repository | `cargo test --lib --all-features --locked -- boundary_suites::` | 189 passed, 5 managed-Python failures; 194 selected |
| repository | `cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | All 7 targets ran: 412 passed, 7 managed-Python failures. Catalog 26, managed-output 5, conformance 26, durable 129, process 53, subagent 45 passed; Tools 128 passed / 7 failed |
| repository | `cargo run --example check_test_lanes -- --job rust-contracts` | Passed: Linux lane coverage, including 3,234 runnable unit/contract tests and 194 boundary tests |
| repository | `cargo run --example check_test_lanes -- --job rust-boundaries` | Passed: Cargo-discovered Linux lane coverage |
| `tui` | `corepack pnpm typecheck` | Passed |
| `tui` | `corepack pnpm test` | Passed: 895 tests, 96 suites |
| `test-support/fake-provider` | `uv sync --frozen` | Passed |
| `test-support/fake-provider` | `uv run --frozen pytest` | Passed: 51 tests |
| `dev` | `corepack pnpm typecheck` | Passed |
| `dev` | `corepack pnpm test` | Initial orphan-reaping failure under container PID 1; unchanged suite passed below |
| `dev` | `python3 /workspace/issue-430-notes/subreaper.py corepack pnpm test` | Passed: 38 tests with Linux child-subreaper environment repair |
| repository | `git diff --check` / `git diff --cached --check` | Passed |

The full production-contract Rust, browser and TUI runs preceded a final test-only
synchronization repair. `pending_snapshot_and_reattach_repair_a_committed_unpublished_mutation`
intermittently admitted its intended pending message into the first prompt:
request-history publication precedes final prompt admission. It now waits on the
existing adapter invocation watch signal before enqueueing. No assertion, timeout
or production behavior changed. The actual CI unit/bin/example selector and Clippy
were rerun after this repair.

Earlier focused/full Web iterations exposed an unsupported test matcher,
incremental-render commit isolation and native message-seat identity issues;
these were corrected with existing regression assertions retained. Early browser
preparation failures (disk capacity, distinct stream text, locale selection and
width-value rounding) were repaired before the final host run.
