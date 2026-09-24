# Issue #394 acceptance record

Base: `e8f700dae5b252e7cdf5b78a9e0000b05edeee04` (latest fetched `origin/main`).
Branch: `issue-394-trajectory-harness-convergence`.
Isolated worktree: `/home/caismis/Documents/codes/rustX-issue-394`.
The primary worktree and unrelated worktrees were not modified.

Contracts: [#394](https://github.com/Caismis/rustX/issues/394),
[approved #303 shared delivery contract](https://github.com/Caismis/rustX/issues/303#issuecomment-5770169051).
Main already contained #393 (PR #400); the shared composition checks apply now,
not to a hypothetical future Settings branch. No Settings implementation dependency
or unmerged chain was introduced.

## Ownership and closure

See [Trace architecture](trace.md) and [Harness provenance](../web-console/PROVENANCE.md).
Native `TraceProjection` continues to own Request/Attempt/Turn identities,
canonical acceptance, Context introductions, lifecycle, usage, clocks and frozen
input. Only three missing facts were added: shared predecessor identity, full
frozen Tool-catalog relationship classification, and bounded predecessor prompt
content. Existing Context, ToolCall scope, canonical message IDs, request ordinal,
provider clock bridge and detail/authorization primitives were reused.

App Server v20 / Runtime Client v46 replace v19/v45 because mandatory Trace fields
change the strict negotiated vocabulary; SQLite remains v43, with no migrations,
aliases, new events or persistent Trace store. Generated Rust-schema/TS fixtures,
consumers, client admission and protocol tests move together. Native summary/page/
detail bounds remain 8 KiB / 128 KiB / 512 KiB encoded JSON. Each prompt string is
bounded independently at 16 KiB. One indexed predecessor seek supplies **both**
comparisons; missing snapshot differs from durable corruption/I/O failure. Tests
instrument predecessor, Context and Tool relationship resolutions; lifecycle
refresh does zero of all three.

Browser display identity differs from native detail ownership. Selection is
`{display_key, owner_record_id, facet, context_message_id?}`. SYSTEM and CONTEXT
reuse the Request cache entry. Semantic tuple keys drive virtual rows/focus and
pixel anchors. Structural headers have separate native structure IDs, segment anchors and local focus, with no detail owner. Step segments can merge without changing a logical Turn or moving
records. Calls match exact conversation-cache/Attempt/Turn/ToolCall/Tool scope,
with separate proposed/loaded/started/state counts. Search never fetches. The
local Inspector uses React Aria and `react-resizable-panels@4.12.4`; jsdiff
`diff@9.0.0` only renders complete native-classified text differences. Existing
TanStack Virtual, Markdown, Shiki, JSON, artifact and authorization code remain.

## T1 matrix: concrete executable tests

`N` below means `src/runtime_client/trace/tests.rs`; `W` means
`web-console/test/trajectory.test.tsx`; `B` means
`web-console/test/e2e/trajectory.spec.ts`. Names are exact test titles or exact
Rust function names (parameterized expansions retain the listed title prefix).

| Case | Exact tests and assertions |
| --- | --- |
| T1-01 | N `system_prompt_state_follows_the_previous_actual_request`, `a_fresh_step_at_retry_zero_is_still_compared_with_the_previous_request`, `a_page_boundary_cannot_turn_changed_into_initial`, `shared_predecessor_compares_complete_prompt_and_tool_definitions`, `unavailable_predecessor_and_durable_read_failure_are_distinct`; W `T1-01 maps every native prompt/tool combination without reading details`. Exact predecessor/Request IDs and states, including unavailable versus no predecessor. |
| T1-02 | N `predecessor_prompt_bounds_are_independent_and_empty_is_available`, `shared_predecessor_compares_complete_prompt_and_tool_definitions`, `unavailable_predecessor_and_durable_read_failure_are_distinct`; W `T1-02 diff distinguishes initial, unavailable, empty and complete changed content` and parameterized `T1-02 independent truncation previous=%s current=%s forbids complete diff`. Empty, current-only/previous-only/both truncation, differing suffix beyond bounds, encoded size and durable read failure. |
| T1-03 | N `shared_predecessor_compares_complete_prompt_and_tool_definitions`, `predecessor_prompt_bounds_are_independent_and_empty_is_available`, `unavailable_predecessor_and_durable_read_failure_are_distinct`; W T1-01 mapping. Complete frozen Tool schema suffix controls equality, all four prompt/Tools combinations, initial/unavailable, one exact shared predecessor. |
| T1-04 | W `T1-04 preserves frozen Context order and exact owner/display/facet identities`, `T1-04 controlled out-of-order owner responses cannot change a newer facet or steal focus`; N `canonical_context_is_projected_from_the_frozen_request_identities`, `two_certified_extensions_stay_distinguishable_by_exact_contributor_identity`, `all_context_assembly_semantic_pairs_project_from_durable_request_start`. Structural ownership and delayed-child-response regressions are mapped in the structural correction section below. Two controlled promises resolve newer owner then older owner; exact two reads, owner/facet/Message ID and focus assertions. No sleeps. |
| T1-05 | W `T1-05 latest unchanged-only page directly discovers prompt with one lazy owner read`; latest page has no fake SYSTEM row and Summary opens the exact prompt with one read. |
| T1-06 | W `T1-06 retries stay one logical Step; failed and running requests need no Assistant`, `T1-06/10 segment anchors regroup within the same structure without acquiring detail ownership`; exact retry ordinals `[0,1,2,3]`, each boundary independently selectable for failed/failed/running/provider-completed without Assistant. Mid-page and exact-native-start structural regressions are mapped below. |
| T1-07 | W `T1-07 exact scope isolates reused call IDs across Step/Attempt/Tool and page split`; exact execution IDs, unrelated same-name Tool, missing scope, ambiguous proposer, page-split owner/execution and two-proposal/one-execution distinction. |
| T1-08 | W `T1-08 Calls summary exposes warnings and leaves native domains independent`; B `T1-08/09/15 Calls warnings, independent background, search and truncated failed Request Diff`; failed/denied/waiting/outcome_unknown, independent Background/Subagent/Workflow, running/incomplete/failed/completed compaction. |
| T1-09 | W `T1-09 search reveals both collapsed kinds without any detail/history reads`; B T1-08/09/15; search overrides both folds, exact zero detail/history reads. `preferredItem` same-owner Request fallback is asserted by W T1-09; structural migration uses `preferredStructure` instead. |
| T1-10 | W `T1-06/10 segment anchors regroup within the same structure without acquiring detail ownership`, `T1-10 512 native records plus synthetic items keep mounted rows and reads bounded`; B `T1-10/11 semantic prepend and tail isolation long`, `T1-10/11 semantic prepend and tail isolation threshold`, `T1-04/06/10 structural request anchor stays structural through threshold prepend` and its `tool` variant. 512 native records, over 2,000 display items, fewer than 60 unit-test mounted items / 65 browser items; prepend removes history boundary, inserts SYSTEM, merges Step, crosses virtual threshold, preserves semantic item + pixel offset, focus stays on the corresponding native Step structure without inspecting any child. Browser DOM read invocation preserves keyboard ownership; no sleep. |
| T1-11 | W `T1-11 content-only updates never move an off-tail reader`; B long/threshold tests assert exact history/detail counts, unchanged off-tail offset and explicit latest/append tail following. Existing `trace-cache.test.ts` retain bounded owner cache, epoch and lifecycle repair coverage. |
| T1-12 | `web-console/test/trajectory-timing.test.ts`: all `authoritative request phase positions` cases, `preserves measured zero separately from absent and running timing`, `T1-12 epoch zero, missing instant, parallel domains and canonical acceptance do not invent or duplicate spans`, `T1-12 four modes keep native time distinct from a shared idle-compression transform`; W `T1-12 sequence keeps equal glyph widths even when native duration is missing`; browser `trajectory-timing.spec.ts` `native phase coordinates survive browser layout at 1440px` and `390px`. Native request terminal/dispatch evidence remains separately tested by the full Trace/Agent Loop suites. |
| T1-13 | B `T1-13/15 semantic ledger, facets and keyboard ${width} ${theme}` at 1440/390 × 844 in light/dark; `T1-13 library drag, keyboard separator, double-click reset and narrow Ledger on wide viewport` (1050 × 844). Keyboard tabs, close/focus restoration, real drag/keys/reset, 320px Inspector/340px Ledger, compact Event column and no horizontal overflow. |
| T1-14 | N `a_lifecycle_refresh_resolves_no_immutable_presentation_relationship`, `shared_predecessor_compares_complete_prompt_and_tool_definitions`, `unavailable_predecessor_and_durable_read_failure_are_distinct`; thread-local read counters assert zero predecessor/Context/Tool relationship resolutions; corrupt predecessor does not affect lifecycle refresh. |
| T1-15 | B all visual cases plus `trajectory-integration.spec.ts` real App Server/provider-emulator adoption. Committed screenshots listed below are actual rendered components/native integration, not mockups. |
| T1-16 | Existing `playwright.config.ts` discovers **all** `test/e2e/*.spec.ts`; both normal and `test:e2e:update` use the same `scripts/browser-tests.sh` with forwarded arguments. CI invokes `pnpm test:e2e`. Full run plus normal comparison verifies discovery; schema/codegen/types/provenance/artifact commands below verify closure. No dormant spec or special discovery path. |
| T1-17 | `trajectory-integration.spec.ts`: `T1-17 X03 X04 X05 X06 X09 Settings save/reread, busy gate, exact adoption and historical Trace`; N `historical_system_classification_survives_reopen_and_later_requests`. Gated real Request → Workspace instructions save and User Tool save/reread → native busy rejection → settle → exact explicit adoption → later Request; compare all old immutable Request fields before/after, new predecessor/Request IDs and exact provider count 2. |

## Shared X01–X10 composition

All cases apply because #393 was already merged. Settings owner tests remain
responsible for Settings mechanics; the new integration compares historical
Trace against the same real source save/adoption chain.

| Case | Concrete tests / synchronization |
| --- | --- |
| X01 | `settings-surface-ownership.test.tsx`: `S1-01 User Settings opens with zero Sessions and zero Workspaces without hidden runtime allocation`; `settings-ownership.spec.ts` C01/C02/C06/C08/C09/C10 test verifies native zero Sessions and zero provider requests. |
| X02 | `settings-surface-ownership.test.tsx`: `S1-02 Workspace Settings stays bound to its exact target across Session focus and fences a revoked target without rerouting the draft`; `settings.test.tsx`: `C09 Workspace A/B drafts survive navigation and Session focus without retargeting`; real browser C01…C10 tests native Workspace revocation and separate A/B drafts. Controlled responses/source authorities, exact target and mutation assertions. |
| X03 | New T1-17 integration edits Workspace A's instructions while its provider is held at `trace-before`; frozen Request detail remains identical. Workspace source is reread through the real Workspace Host. |
| X04 | Same provider gate proves busy rejection, no extra provider admission; native eligibility becomes eligible only after explicit gate release. Existing C13…C17 browser test also checks candidate fencing and displayed native busy reason. |
| X05 | Exact candidate identity + expected binding adopted only for A; compare old immutable detail; B's actual `effectiveConfiguration.adopted_binding` remains equal to its captured value. |
| X06 | Same integration submits exactly one later Request; asserts frozen new instructions, removed `read` Tool, predecessor ID, both native relationship states, previous prompt and unchanged old input. |
| X07 | Extended C13…C17 real browser test captures exact old Trace detail, drops one adoption reply, reconnects and rereads exact detail; exactly one adoption mutation and one provider request. C01…C10 drops one source-write response; S1-09/S1-16 unit tests fence lost replies and retired authority using controlled promises. No sleeps/replay. |
| X08 | N shared-predecessor test changes prompt and Tool schema only beyond the detail bound; old/new detail prefixes can equal while native states differ. W parameterized incomplete diff and B truncated-Diff screenshot make incompleteness explicit. |
| X09 | Same T1-17 integration deliberately corrupts B's Workspace source before the shared User Tool save. A prepares/adopts; B retains its binding and independent failed unit. Both application snapshots and provider counts are checked. |
| X10 | B four light/dark desktop/390 × 844 Inspector cases plus `settings-presentation.spec.ts` `dark desktop Advanced keeps the same family and native diagnostics`, `390 × 844: section menu, list → detail → back, long identities and no horizontal overflow`, and `layout follows the Settings panel width, not the window`. Same theme primitives, actual container resize, no page overflow. |

## Browser evidence

References live in `web-console/test/e2e/trajectory.spec.ts-snapshots/`:

- `trajectory-ledger-{1440,390}-{light,dark}-linux.png`: SYSTEM + ordered CONTEXT,
  prompt-only update, Tool-only update, combined update, failed Request with no
  Assistant, independent native domains. Heights 844.
- `trajectory-inspector-{1440,390}-{light,dark}-linux.png`: local System Prompt
  facet, selected SYSTEM, stacked Inspector at 390 × 844.
- `trajectory-calls-linux.png`, `trajectory-search-linux.png`: visible failure
  summary and search revealing its collapsed execution (1440 × 1000, light).
- `trajectory-truncated-diff-linux.png`: failed Request and explicit both-side
  incomplete diff (1440 × 1000, light).
- `trajectory-prepend-{long,threshold}-linux.png`: real paging, synthetic headers
  and anchored virtual display (1440 × 1000, light).
- `trajectory-resized-narrow-ledger-linux.png`, `trajectory-resize-reset-linux.png`:
  pointer resize and default reset (1050 × 844, light); keyboard separator also tested.
- `docs/evidence/issue-394/trajectory-native-adoption.png`: real App Server and
  local provider emulator, changed frozen prompt after explicit Settings adoption.

The exact Linux Chromium/Playwright image is pinned in `browser-tests.sh`.
Podman supplies the container here. No paid/live model or fabricated performance
measurement is used. `agent-browser` and a Browser plugin were unavailable; the
repository's prescribed Playwright container is the browser evidence authority.

## Executed validation

Final command results are recorded below after the final normal comparison run.
Earlier failed development iterations are retained as diagnostics, not claimed
as passing: strict-version fixtures initially retained old numbers; an early
broad test-only replacement changed sample IDs (restored); a panel selector used
the wrong library attribute; old E2E Summary assertions expected native IDs;
initial cache selection was restored and the standalone managed-output fixture
was given the same constrained-height container as the application;
Clippy requested a small detail helper and backticked API documentation. One
full update run was interrupted after reporting these failures and was rerun.

Commands ran from the isolated worktree unless noted. Environment prefixes are
shown where relevant; they change test execution prerequisites/concurrency, not
production code. No test expectation or screenshot tolerance was relaxed.

| Executed command | Result |
| --- | --- |
| `git fetch origin`; `git status`; `git worktree list`; `git log -1 --oneline origin/main` | Passed before implementation; exact base recorded above. A final `git ls-remote origin refs/heads/main` still returned that base. |
| `pnpm --dir tui install --frozen-lockfile`; protocol package install/generate; exact Web dependency installation | Passed; only the intended Web production lockfile additions. |
| `cargo build --bins` | Passed; actual App Server/supervisor binaries used by integration suites. |
| `cargo fmt --all`; `cargo fmt --all -- --check` | Formatting applied, final check passed. |
| `cargo check --workspace --all-targets --all-features` | Passed. |
| `cargo test --lib runtime_client::trace --all-features` | 75 passed. |
| `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,888 passed; 3 existing Python-tool tests failed on PyPI connection/download errors; 7 intentionally ignored. All 3,291 library tests and real App Server/process/provider/subagent targets passed. Tools retry is recorded separately below. An earlier complete run before the final helper/read-count assertion passed all 3,891. |
| `UV_OFFLINE=1 CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --test tools --all-features` | 124 passed, 6 failed. The managed native subprocess still attempted PyPI access; this environment-only retry did not remove the network limitation. |
| `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --test tools --all-features` | 130 passed, 0 failed. This reruns the entire tools target with unchanged production code and resolves the prior transient download failures. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` (final `CARGO_BUILD_JOBS=2`) | Passed. |
| `pnpm --dir protocol/app-server generate`; `pnpm --dir protocol/app-server check`; `pnpm --dir protocol/app-server typecheck` | Passed; generated schema/TypeScript/fixtures have no drift and only v20 artifacts exist. |
| `pnpm --dir web-console typecheck` | Passed. |
| Web targeted Vitest run (`trajectory.test.tsx`, `trajectory-timing.test.ts`) | Initial targeted run passed 22 tests; subsequent additions were executed by the final full suite. |
| `pnpm --dir web-console test` | Final 50 files / 881 tests passed. Earlier version-fixture/import-boundary failures were corrected and rerun. |
| `pnpm --dir web-console build` | Passed, including production artifact provenance. Existing nonfatal Vite chunk-size advisory remains. |
| `pnpm --dir web-console check:provenance` | Passed: 114 source records / 131 production package notices. |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-394-harness` | Passed; local hashes/imports and immutable upstream hashes verified against the exact pin. |
| `node web-console/scripts/notices.ts --write` | Regenerated notices successfully; later notice and packaged-artifact checks passed. |
| `pnpm --dir tui typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Passed / 852 tests passed. |
| `uv sync --frozen`; `uv run --frozen pytest` in `test-support/fake-provider` | Passed / 51 tests passed. |
| `CONTAINER_ENGINE=podman RUSTX_SCREENSHOT_UPDATE=1 pnpm --dir web-console test:e2e trajectory.spec.ts` | Development run: 7 passed, 1 panel-selector failure; fixed and rerun. |
| Same update invocation with `trajectory.spec.ts trajectory-integration.spec.ts` | Development run: 8 passed, 1 overly broad tab-panel locator failure; corrected to the local Inspector. |
| Same update invocation over the full E2E suite | Development run exposed old Summary assertions and missing standalone-fixture height; interrupted and superseded by complete runs below. |
| Same update invocation with `trajectory.spec.ts trajectory-integration.spec.ts chat.spec.ts managed-output.spec.ts` | 12 passed, 1 narrow managed-output fixture failure; corrected its application-like height container. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e:update trajectory.spec.ts trajectory-integration.spec.ts settings-ownership.spec.ts chat.spec.ts managed-output.spec.ts` | 15 passed; verifies actual update-command discovery and the final Conversation/attachment lifetime key. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e:update trajectory.spec.ts trajectory-integration.spec.ts trajectory-timing.spec.ts` | 12 passed; final sequence glyph references regenerated from stable real browser captures. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 85 passed in normal strict screenshot-comparison mode; no tolerance changes. |
| `git diff --check`; `git diff --cached --check` | Passed. |

The seven Rust ignores are the repository's five paid/live-provider tests, the
fixture corpus writer and the explicitly instrumented reservation stage profile.
They were not invoked; no paid/live model was required. Failed parallel tools
cases were `mcp_managed::one_connected_runtime_reuses_one_process_across_calls`,
`mcp_managed::two_folders_prepare_distinct_environment_identities`, and
`uv::production_uv_materializes_a_managed_package_environment`. Their failures
were `pypi.org/simple/fastmcp` network-unreachable and a
`files.pythonhosted.org` wheel download timeout, not Trace assertions. The retry
results above are reported separately rather than describing a failed command as
passing. Interrupted runs during the conversation pause were rerun; no incomplete
run is treated as validation evidence.

The final sequence-width correction intentionally changed six timing screenshots.
A normal run detected exactly those six reference mismatches (79 other tests
passed); the explicit update command regenerated the references without changing
tolerances, followed by the final normal comparison above.

## PR #401 review correction: protocol and timing contracts

Starting HEAD: `52ff9b6c5e81f77c1fb73d5e970ae74656db9a03`. This bounded correction
retains the implementation architecture and changes no timing algorithm, native
DTO, protocol version, runtime semantics or screenshot reference.

The protocol audit corrected current browser subprotocol guidance, batch and
controller behavior, generation filenames, inbound/SourceTarget wording, Session
initialization, current Trace vocabulary and the Subagent heading/links. TUI
current-version wording was corrected alongside those links. Initialization is
exactly App Server v20; WebSocket selects `rustx.app-server.v20`; generated files
are `v20.ts` and `v20.schema.json`; v19 and earlier are unsupported.

The remaining v19 mentions in `app-server-protocol.md` are the explicit rejection
and removed-artifact statements, “App Server v19 made…” and “Before v19…” history.
Repository-wide search also retains the historical PR #333 CFG3 residue audit,
this acceptance record's migration history, and unrelated SQLite schema history.
The former Subagent v19 heading described current behavior, not a historical
introduction, so it and both incoming links now say v20.

**Timing invariant:** A span requires two authoritative endpoints in the timing
domain being rendered; Request Model-lane spans use provider-domain
`GenerationEvidence`, while Journal duration remains separate native evidence.

The reviewed Rust `TraceTiming`, `TraceGeneration`, `TraceGenerationTimeline`,
`generation_metrics` and `generation_timeline` retain their native ownership.
Numeric metrics cannot manufacture a bridge. Request Journal settlement may
include persistence latency, so its terminal cannot substitute for a provider
terminal. No fallback to Journal Request duration was added. Equal-width sequence
and start-only time projections remain unchanged.

| Regression | Contract proved |
| --- | --- |
| `documentation::current_app_server_documentation_matches_negotiated_version` | Current heading, exact initialization, WebSocket and generation sections agree with `APP_SERVER_PROTOCOL_VERSION`; older generated artifacts are absent. Runs in the existing Rust contracts target. |
| `Journal terminal at 9000ms cannot supply a Model endpoint without the native bridge` | Exact Journal start/end and 9000ms duration remain available, but Model start=end=T0; numeric TTFT supplies no phase positions. |
| `T1-12 Journal duration remains visible in Inspector when Model timing lacks a bridge` | Timing facet explicitly renders Journal wall duration as 9.00s and its durable timestamp source. |
| `does not stretch phases to fit Journal wall duration or settlement delay` | The fixture now supplies consistent Journal endpoints 9000ms apart; provider span still ends at +2000ms. |
| `renders dispatch without inventing first output for a silent request` | Provider span ends at +2000ms with no first/last output positions. |
| `preserves measured zero separately from absent and running timing` | Start-only running Request stays a marker; zero provider terminal and measured zero remain present, not missing. |
| Existing T1-12 canonical-acceptance/parallel-domain and four-mode tests | No Assistant duplicate span; parallel work retains overlap; duration compression uses one shared transform across lanes. |

All timing fixtures use literal timestamps and native offsets, without sleeps or
clock-dependent ordering. The two focused Web files pass 26 tests. The requested
`pnpm ... test -- ...` invocation runs all files with this script/toolchain;
`pnpm --dir web-console exec vitest run test/trajectory-timing.test.ts test/trajectory.test.tsx`
was also run to verify the precise focused selection.

Review-correction validation (Linux; the repository CI workflow and package scripts
were inspected before choosing these commands):

| Command | Result |
| --- | --- |
| `cargo fmt --all` | Applied formatting to the new documentation test. |
| `cargo fmt --all --check` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo check --all-targets --all-features` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo test --test contracts current_app_server_documentation_matches_negotiated_version --all-features` | 1 passed. |
| `pnpm --dir protocol/app-server check` | Passed regeneration and drift check; generated artifacts unchanged. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir web-console typecheck` | Passed. |
| `pnpm --dir web-console test -- trajectory-timing.test.ts trajectory.test.tsx` | 882 passed; this script invocation selected all files. |
| `pnpm --dir web-console exec vitest run test/trajectory-timing.test.ts test/trajectory.test.tsx` | 26 passed in the exact two focused files. |
| `pnpm --dir web-console test` | 882 passed in 50 files. |
| `pnpm --dir web-console check:provenance` | Passed: 114 source records, 131 package notices. |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-394-harness` | Passed against the pinned upstream checkout. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 85 passed in strict normal screenshot comparison; includes the canonical `pnpm build` and packaged-artifact check. No reference update. |
| `CARGO_BUILD_JOBS=2 cargo build --bins` | Passed. |
| `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,892 passed, 0 failed, 7 intentionally ignored; no environment failure or retry in this correction run. |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 37 tests passed. |
| `pnpm --dir tui typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Passed; 852 tests passed. |
| `uv sync --frozen`; `uv run --frozen pytest` in `test-support/fake-provider` | Passed; 51 tests passed. |
| `git diff --check`; `git diff --cached --check` | Passed. |

The seven existing ignored Rust tests remain the five paid/live cases, fixture
writer and stage profile described above. macOS execution is left to the existing
GitHub Actions platform job; this local environment is Linux.


## PR #401 structural identity and missing Tool facts correction

Starting HEAD: `2740079e4d9d75fd75445c83a833b67a237f00ee`; fetched base remains
`e8f700dae5b252e7cdf5b78a9e0000b05edeee04`. All seven Actions jobs passed at the
starting head. The former common `Origin` type incorrectly assigned a segment's
first loaded child as a header's detail owner. Partial pages exposed that error;
the old prepend test then preserved the wrong child-inspection fallback.

`InspectableDisplayItem` now exclusively owns native detail identity. Structural
headers carry exact Attempt/Turn identity, segment anchor/membership, display key
and optional exact loaded structural record evidence. They remain presentation-only
whether or not that record is loaded. Structural activation/focus clears Inspector
selection and never reads details; regrouping follows the same structure's segment
containing the old anchor, never the child itself. Search no longer borrows an
anchor child's content/state for structural headers. Attempt ordinals remain
loaded-window labels. No protocol/native change is necessary: v20/v46 and SQLite
are unchanged. See `docs/trace.md` for keyboard and disappearance behavior.

Tool Input/Result/Schema render explicit unavailable states; absent bounded Tool
payload differs from a loaded Tool with no canonical result. Code still requires
native source evidence. No lifecycle or successful outcome is inferred.

| Required regression | Exact executable test |
| --- | --- |
| A/B, T1-04/06 | `T1-04/06 mid-Step %s anchor never owns structural inspection` for Request and Tool: stable semantic keys, header focus/click/Enter issue zero detail/history reads; child selection issues exactly one read to that child. |
| C, T1-06 | `T1-06 exact loaded native structures remain presentation-only and expose only their own evidence`: exact Attempt/Step record IDs, no Inspector or detail read. |
| D, T1-06/10 | `T1-06/10 segment anchors regroup within the same structure without acquiring detail ownership`: exact segment membership, native Step evidence, split order and missing-segment behavior. Browser `T1-04/06/10 structural request anchor stays structural through threshold prepend` and `tool` variant: native starts arrive, semantic focus and pixel offset survive threshold crossing, zero detail reads, one history read, then explicit child selection. |
| E, T1-04 | `T1-04 controlled late %s detail cannot hijack structural focus` for Request and Tool: controlled promise resolves after structural focus/Enter; exact read ID/count, Inspector absence and retained DOM focus. |
| Nearby identity invariants | `T1-09 structural search and Attempt collapse never borrow child facts or another Attempt`; `T1-04 timeline navigation explicitly selects its native Request after structural focus`. |
| F/G/H | `Tool facets explicitly disclose absent %s at the read cut` for result, definition, arguments and entire bounded Tool payload: every advertised Input/Result/Schema panel is nonempty; exact absence message and conditional Code. |

Existing SYSTEM/CONTEXT owner/facet race, Calls scope, search/no-read, retry,
500+ record virtualization, off-tail, timing and real T1-17 provider-emulator
integration remain in the full suite. Provenance hashes are refreshed only for
changed tracked source-derived files; no dependency, notice or upstream pin change.

Validation commands for this correction (Linux; current CI/package scripts inspected):

| Command | Result |
| --- | --- |
| `pnpm --dir web-console typecheck` | Passed after fixing a stale test type import and invalid Testing Library option during development. |
| `pnpm --dir web-console exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts` | 37 passed after the final nearby-invariant additions (earlier focused run: 35 passed). |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e trajectory.spec.ts trajectory-integration.spec.ts` | 11 passed, including actual T1-17 App Server/provider-emulator integration and both structural prepend cases. |
| `pnpm --dir web-console test` | First run: 890 passed, Settings C09 hit its unchanged five-second timeout. Final rerun: 893 passed in 50 files, including two subsequently added nearby-invariant tests. Settings code/assertions/timeouts were unchanged. |
| `pnpm --dir web-console check:provenance` | Passed: 114 source records and 131 production packages. |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-394-harness` | Passed against the pinned source. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 86 passed in normal strict screenshot comparison; includes production build and packaged-artifact checks; no screenshot reference changes. |
| `cargo fmt --all -- --check` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo check --all-targets --all-features` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `CARGO_BUILD_JOBS=2 cargo test --lib runtime_client::trace --all-features` | 75 passed. |
| `CARGO_BUILD_JOBS=2 cargo build --bins` | Passed. |
| `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | Library target: 3,289 passed, 2 failed, 2 ignored; Cargo stopped before external targets. Exact failures below. |
| `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | Interrupted: single-threaded libtest prefixes the child gate marker on the same line, so the unchanged process-death parent cannot recognize its exact-line marker. Only this run's two test processes were stopped. Not counted as passing. |
| Final `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,892 passed, 0 failed, 7 intentionally ignored; includes all 130 Tool tests. |
| `pnpm --dir protocol/app-server check`; `pnpm --dir protocol/app-server typecheck` | Passed; generated artifacts unchanged. |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 37 tests. |
| `pnpm --dir tui typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Passed; 852 tests. |
| `uv sync --frozen`; `uv run --frozen pytest` in `test-support/fake-provider` | Passed; 51 tests. |
| `git diff --check`; `git diff --cached --check` | Passed. |

The first full Rust run failed `runtime::workspace::tests::concurrent_children_have_distinct_deterministic_paths_and_refs`
with Git `worktree add` reporting “failed to read .git/worktrees/…/commondir: Success”,
and `boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`
with native managed-Python source preparation unavailable. Rust source, native
tests and protocol artifacts are byte-for-byte unchanged from the starting HEAD;
`git diff 2740079e -- src tests protocol` is empty. The starting HEAD's seven CI
jobs passed. The serial retry passed managed selection but exposed the unrelated harness
marker issue above. A bounded Python subprocess diagnostic invoked the unchanged
`deletion_borrowed_workspace_process_gate` binary with `RUST_TEST_THREADS=1`, a
temporary `RUSTX_260_BORROW_ROOT` and one supplied stdin byte. It exited 0 and
captured exactly `test local_runtime::session::tests::deletion_tests::borrowed_workspace::deletion_borrowed_workspace_process_gate ... BORROW_COMMITTED`
on one line, confirming why the parent's `line.trim() == "BORROW_COMMITTED"`
loop did not advance. This diagnostic is not the process-death acceptance test.
The final full run restores the normal parallel harness after other heavy suites
finished. No native test, timeout or production behavior was changed. Failed or
interrupted commands are not reported as passing or silently attributed to a
specific network cause.

Local execution is Linux; macOS remains the existing Actions job. The seven
standard paid/live, fixture writer and profile ignores remain intentional.

The full browser run also emits the existing React warning about updating global
`Inspector` while rendering `SessionConfiguration` (outside Trajectory). The same
warning appears at the prior reviewed HEAD's `/tmp/401-fix-e2e.log` and this run's
`/tmp/401-structure-e2e.log`; both browser suites pass. That unrelated Settings
warning was not changed by this correction.
