# Issue #407 implementation and validation

## Checkout and reference

- Primary rustX: `/home/caismis/Documents/codes/rustX`, `main`,
  `f268175bb8d31010706e7070aae80d2b46b7aced`; initial status only
  `?? .playwright-mcp/`. No primary files were changed.
- Implementation: `/home/caismis/Documents/codes/rustX-issue-407`, branch
  `issue-407-harness-turn-trajectory`, created from freshly fetched `origin/main`
  at the same SHA.
- Harness: `/home/caismis/Documents/codes/deepseek-harness`, clean `master` at
  `477b4f420553e8a52c2fbccc464d7561b239c443`, equal to fetched `origin/master`.
  Read-only source review; no checkout/reset/edit in that worktree.
- `web-console/PROVENANCE.md` and the inventory distinguish adapted hierarchy,
  geometry and detail organization from studied-only native-incompatible code.
  The provenance audit uses a separate local clone at the repository-wide pin,
  and verifies each additional source at its recorded commit via `git show`.

## Contract and regressions

Runtime vocabulary is unchanged. `projectTrajectory` groups only native
`TraceLocation` facts: AttemptId → Turn, TurnId/step_id → Step. It produces one
Turn and one Step group per native identity, even across interleaved records.
Attempt-only input goes into Message; unscoped records stay outside. The grouped
presentation can bring interleaved members together, but never changes runtime
history. Retry requests remain compact request metadata in their native Step.

The ledger flattens that projection and the timeline consumes it directly. There
is no callback reconstructing boundaries from raw rows. Display ordinals never
key interaction state. Drag intervals retain native record IDs; their displayed
coordinates may shift after prepend. Native timing/bridge rules remain unchanged.

The old AttemptSectionHeader, StepHeader and SystemRow structures are gone.
SystemPromptCell follows native classification, with initial System Prompt/Tools
and changed Diff/System Prompt/Tools tabs. Summary/Native remain inspectable.
Context selects exact frozen message identity. Truncated/missing evidence blocks
complete diff; jsdiff never classifies changes. Detail stays on the exact request
record and cache epoch, using immutable historical RequestSnapshot input.

The deterministic trajectory suites prove:

- One Turn per exact Attempt and one Step group per exact Step, including retries,
  interleaving, arbitrary opaque IDs, Message ownership and outside input.
- Prepend changes Turn 1 to Turn 2 while a selected, collapsed prompt retains
  record identity, frozen detail, collapse and focus. Search changes/clearing
  restore collapse exactly and perform no extra history/detail reads.
- Timeline/ledger boundary identities and labels agree from the same projection.
- Initial, changed, combined Tool update and unavailable prompt states follow
  native enums; all three independent truncation combinations reject false
  complete equality. Existing controlled out-of-order detail tests remain active.
- Native IDs are absent from primary hierarchy and present in Native detail.
- Scoped proposal/execution joins require exact Attempt/Step/Tool/call identity;
  Background/Subagent/Workflow/Interaction are never folded by proximity/name.
- Missing and incomplete timing remains missing; request-relative bridge and
  provider timing remain separate from Journal duration.
- Finite 512-record loaded windows, bounded virtual DOM, loaded-only search,
  prepend anchors, stale-response fences and cache/frontier suites remain active.

Browser coverage uses explicit counts, focus, native identity and application
signals, with no sleeps for semantic correctness. It covers desktop/390px,
light/dark, prompt inspection, tabs, keyboard, resizing, search/collapse,
renumbering, timeline selection and drag focus, sticky virtual Turn headers,
prepend/tail isolation, and Request/Tool/header focus across virtualization.

## Visual review

The flow under test is Trajectory → native Turn/group/cell selection → semantic
Inspector, with prepend/search/timeline navigation preserving identity.
Agent-browser session `rustx-407` inspected the local fixture at 1440×844 and
390×844. Screenshots were saved outside the repository under `/tmp/issue407-*`.
No blank page, framework error overlay, horizontal overflow or fresh-load browser
error was observed. A dev-only createRoot warning occurred during fixture hot
replacement; fresh navigation and the browser tests do not reproduce it.

Intentional goldens cover the new Turn/Message/Step headers, compact request
metadata, selected-row rail, semantic prompt tabs and sticky virtual headers.
They were updated only after rendered review, then checked in normal comparison
mode. The authority is Playwright 1.63.0 in the repository's immutable Noble image,
run with Podman (Docker is absent). No noise thresholds or rasterizer allowances
were changed.

## Intentional Harness differences

- Prompt/Context remain in the native owning Step, including initial prompts;
  no guessed Session-start or next-Turn ownership.
- Unscoped records have standalone placement. Native domain records retain their
  own bounded cells; Tool correlation does not imply lifecycle ownership.
- Request metadata stays directly inspectable, including failures without an
  accepted Assistant; request identity is the immutable snapshot owner.
- Historical Tool schemas are inspectable; no previous-catalog diff is fabricated
  because Trace supplies classification and the current frozen catalog.
- Native request-relative generation evidence determines timing. Actual-time
  mode and honest missing timing remain, without browser-clock duration.
- Finite native paging, stable native keys and TanStack virtualization replace
  Harness event assembly and ordinal-owned interaction. Semantic Event/Content
  columns retain the compact ledger rather than the legacy standalone cell's
  permanent metrics grid; timing/usage remain available in the overview/details.

## Commands and outcomes

Commands were run from the dedicated issue worktree (Web commands from
`web-console`, or with `pnpm --dir web-console`).

| Command | Final outcome |
| --- | --- |
| `pnpm install --frozen-lockfile` in `web-console` and `tui` | Passed; lockfiles unchanged. An initial invocation at repository root had no package manifest and was corrected. |
| `pnpm typecheck` | Passed. Initial migration runs identified the obsolete timeline fixture prop and missing fresh-worktree TUI dependencies; both corrected. |
| `pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts` | 46 passed. Initial expectations for repeated structural segments were replaced with exact one-Turn/Step ownership; a test-edit syntax error was corrected. |
| `pnpm test` | 55 files, 971 tests passed, including finite Trace cache/frontier/read-fencing suites. |
| `pnpm build` | Passed, including artifact provenance. Existing >500 kB chunk warning remains. |
| `pnpm check:provenance` | Passed: 135 source records and 131 production-package notices. |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-407-harness-audit` | Passed: upstream bytes at each recorded commit and local import/hash closure. |
| `cargo build --locked --bin rustx` | Passed, but insufficient alone for full browser fixtures. |
| `cargo build --locked --bins` | Passed; includes native helper binaries required by browser fixtures. |
| `cargo fmt --all -- --check` | Passed. |
| `git diff --check` | Passed. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts` | First comparison: 3 passed / 10 failed. Expected golden changes plus a real structural focus/virtual-threshold bug and an obsolete virtual row-count bound. Corrected and revalidated. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts --grep 'structural\|407:'` | Intermediate 2 structural failures before the focus fix was applied; final structural tests pass. |
| `RUSTX_SCREENSHOT_UPDATE=1 CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts` | Intentional reviewed goldens updated. 15 passed / 1 new drag test failed because it inspected an unrelated/unsettled virtual window; corrected to await the exact focused native record. No product timing sleeps added. |
| `CONTAINER_ENGINE=podman pnpm test:e2e` | Broad run: 90 passed / 3 failed. Chat/Console exposed missing helper binaries from the initial single-binary build; Workspace fixture had a refused control connection. All were rerun successfully below. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts chat.spec.ts console.spec.ts workspaces.spec.ts` | Final normal comparison: **22 passed**, including all previously failed broader tests and the two additional selected-cell threshold tests. |
| `agent-browser --session rustx-407-final open …/test/fixtures/trajectory.html`, `errors`, `console` | Fresh load: correct page title, no errors, only Vite connection/React DevTools informational output. Desktop/narrow interaction screenshots reviewed. |

No frontend formatter/linter is configured. No native source, schema or generated
protocol surface changed, so affected Rust unit tests, clippy and protocol
regeneration were not applicable. The native server was built and exercised by
real App Server browser integration. All failures above were resolved or passed
on the final affected-suite rerun; no failing test was skipped or weakened.

## PR #414 presentation-contract repair (2026-09-26)

The live PR was re-read before edits, after fetching `origin`: base/main
`f268175bb8d31010706e7070aae80d2b46b7aced`, head
`d33dfb884b45d40aa5afbffef3cadf4304e29fd8`, clean branch
`issue-407-harness-turn-trajectory` in the existing issue worktree. Main was an
ancestor (2 ahead / 0 behind). The PR was open, non-draft, with auto-merge null;
GitHub returned no formal reviews or review/issue comments. Initial checks showed
six successes and the macOS boundary job in progress. A second fetch after local
validation found main unchanged. No integration or native/schema change was needed.

Re-checked Harness at `477b4f420553e8a52c2fbccc464d7561b239c443`:
`trajectory-search-index.ts`, `TrajectoryTable.tsx`, `timeline.ts`, and
`TrajectoryView.tsx` under `packages/client/ui-trajectory/src/client/`.
The external checkout remained clean and unchanged. Adapted the concepts of
structural context on actual cell search entries, filtering cells before exposing
headers, sharing membership between table and overview, and deriving boundaries
from projected member spans. rustX keeps native record/cell identity and timing;
Harness numeric cell/Turn identity and event assembly were not imported. The
inventory records the additional pinned search/timeline sources and current hashes.

The previous header-result expansion exposed native owners only to Timeline,
while Ledger retained headers without their cells. Search now consumes
`TrajectoryProjection` and returns only semantic cell keys. Turn/Step/Message
labels are context on each member cell. Ledger filters exact keys and exposes
necessary headers; Timeline uses the deduplicated native owners of those same
cells. Owner conversion never expands a text match to sibling cells. Prompt text
is confined to its System Prompt cell and model/failure text to Request metadata.
Turn/Step ordinal phrases match the whole structural label, avoiding accidental
cross-source matches such as Step 1 plus Turn 2 satisfying “Step 2”.

The previous boundary coordinate came from a non-rendered structural record.
Boundaries now use the minimum final span start belonging to each projected Turn,
after duration compression. Sequence uses the same rule in sequence coordinates.
A Turn without spans has no boundary. Native timing, provider phases versus
Journal settlement, missing-duration policy, and native selection/drag IDs remain
unchanged. No second ownership grouping algorithm or compatibility renderer exists.

Added/strengthened deterministic coverage:

- Folded Step 2, Turn 2, Message, and native identity queries assert actual Ledger
  inspectable rows and matching Timeline owner IDs, zero reads, and exact collapse
  restoration. Desktop/390px browser tests also require those rows to be visible.
- Structural searches expose all semantic cells of a rich Request. Exact prompt,
  Context, Request-model, and Tool-content searches expose one precise cell while
  preserving selected Inspector ownership and causing no reads.
- Structural search temporarily reveals collapsed Call executions and restores
  their summary/collapse state when cleared.
- All four timeline modes cover structural timestamps before activity, multiple
  Turns, out-of-order activity timestamps, and structural-only Turns. Every
  boundary equals its own Turn's minimum projected span start and stays within
  the model domain. Duration explicitly checks both intra-Turn and inter-Turn idle
  compression. A Turn lacking a usable timed start has no timed boundary.
- Existing prepend, >100-row virtualization, sticky headers, finite loaded-window,
  native focus/selection, lazy-detail and timing tests remain passing.

Final local commands (Web commands from `web-console`):

| Command | Result |
| --- | --- |
| `pnpm typecheck` | Passed. A new test initially omitted required `started_at`; corrected to the existing invalid-timestamp fixture pattern. |
| `pnpm build` | Passed, including artifact provenance; existing >500 kB chunk warning only. |
| `pnpm test` | 55 files / 988 tests passed. |
| `pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts` | 2 files / 63 tests passed. Initial structural assertions caught cross-source ordinal matching; corrected before final runs. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` | 21 passed; pinned browser, normal golden comparison, no reference changes. Includes native App Server integration and desktop/390px runs. |
| `pnpm check:provenance` | 135 source records and 131 production-package notices verified. |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-407-harness-audit` | Passed; pinned upstream byte hashes and local source/import closure verified. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed (1m 30s); no native edits. |
| `git diff --check` | Passed. |

Browser plugin was not available; the configured Playwright suite supplied browser
and golden validation. Agent-browser session `rustx-414` additionally inspected
`http://127.0.0.1:5176/test/fixtures/trajectory.html?structural-search`: correct
page title/content, no blank page or framework overlay, no browser errors, and
only Vite/React DevTools informational console output. Fold Turns → Step 2 search
exposed the two real Request rows at both widths. Reviewed screenshots are
`/tmp/rustx-414-search-1440.png` and `/tmp/rustx-414-search-390.png` (not committed).
An initial manual navigation used the already-stopped E2E server; starting the
separate fixture server resolved it. No semantic test sleeps were added.

## PR #414 review repair: focus epoch and structural evidence (2026-09-26)

Starting head `1764de64bae844e707227a542f5f2bff4f2c2bea`; base `origin/main`
`f268175bb8d31010706e7070aae80d2b46b7aced` had not moved.

**Timeline focus is owned by the Trace read epoch.** Drag focus is stored as
`{ epoch, ids }`, and `Trajectory` treats it as active only while `focus.epoch ===
cache.epoch`. Prepend and lifecycle refresh keep the epoch, so native record IDs
keep the focus across Turn renumbering and moved coordinates. Every `replaceTrace`
rebase (Jump to latest, resynchronizing, a disjoint or over-bound refresh) moves to
a new epoch and retires the focus, even when the new window reuses those record IDs.
Previously the ID set outlived the epoch. It projected no range in the new domain,
yet it still marked every new row `data-timeline-focus="outside"`. A drag that
covers no span now sets no focus.

**Structural evidence without detail ownership.** Turn/Step headers are still not
`InspectableDisplayItem`s, and they never call `onLoadDetail`. Selecting one opens a
separate `TrajectoryStructureInspector`. It reads only the header's exact
`native_record`, taken from the current projection, and shows record ID, native
kind, Attempt ID, Step ID, state, start/end/duration, native ID and preview. When
that record is not loaded, it says exact structural evidence is unavailable at this
read cut. A Message group has no structural record by construction. Focus no
longer activates a header, so closing the structure inspector returns focus to the
header without reopening it. The earlier test required headers never to open an
inspector, which left a loaded Attempt/Step record uninspectable. That test was
replaced.

New or rewritten deterministic regressions (no sleeps):

- Unit: prepend and lifecycle refresh preserve focus IDs within one epoch and move
  the overlay. `replaceTrace` clears the overlay and every `data-timeline-focus`,
  including when identities recur. A Jump to latest fixture that mirrors
  `latestTrace()` leaves no stale dimming. With the epoch check removed, the
  rebase tests fail.
- Unit: exact Attempt/Step evidence (IDs, state, timing) is reachable by click,
  Enter/Space, arrows, Escape and Close with no detail read. Headers without an
  exact record lend no child facts. Late Request/Tool detail cannot replace
  structural inspection. Prepend renumbering and lifecycle refresh keep the same
  structural selection.
- Browser: the fixture `latest` now performs the real `replaceTrace` rebase. The
  virtual sticky/drag test checks that Jump to latest retires focus. The structural
  prepend test checks unavailable → exact `trace:51` Step evidence. A new
  1440/390px keyboard test inspects Turn/Step evidence with zero detail reads.

| Command | Result |
| --- | --- |
| `pnpm typecheck` | Passed. |
| `pnpm build` | Passed, including artifact provenance; existing chunk warning only. |
| `pnpm test` | 55 files / 992 tests passed. |
| `pnpm exec vitest run test/trajectory.test.tsx` | 55 tests passed. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` | 23 passed. |
| `CONTAINER_ENGINE=podman pnpm test:e2e` | 99 passed; normal golden comparison, no reference changes. |
| `pnpm check:provenance` | 135 source records and 131 notices verified after structural inventory rehash. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed; no native edits. |
| `git diff --check` | Passed. |

## PR #414 earlier repair: Timeline coordinate state per epoch (2026-09-26)

Historical result; the same-epoch interaction assumption below is superseded by
the projection ownership repair in the next section.

Starting head `f09fa6ca65abe99576e715b67f15171d7c75e527`; `origin/main`
`f268175bb8d31010706e7070aae80d2b46b7aced` had not moved.

Root cause: committed focus was fenced by epoch, but `TrajectoryTimeline` kept its
coordinate-local state (`dragRef`, `panRef`, `draft`, `viewport`, `hover`) across
a rebase. Its only reset key was the numeric `model.start:model.end`, which two
epochs can share. A pointerDown/pointerMove in E1 followed by `replaceTrace` and a
pointerUp committed the E1 interval through the E2 model and epoch. A press on an
E1 span could likewise select a recurring E2 record.

Repair: `Trajectory` renders `<TrajectoryTimeline key={cache.epoch}>`. The epoch
owns the Timeline coordinate domain and, through it, every drag/pan/draft/
viewport/hover state. A rebase remounts the component and drops all of it at once.
The old instance's handlers and wheel listener go with it, so no E1 gesture can
reach the E2 callbacks. Committed focus stays in the parent as `{ epoch, ids }`.
Same-epoch prepend and refresh keep the same Timeline instance, so native-ID focus
survives while its coordinates move. The same-epoch `domainKey` viewport reset is
unchanged.

Regressions (no sleeps). Each new one fails when the key is removed:

- Unit: pointerDown → pointerMove → `replaceTrace` → pointerUp (sent to both the
  detached E1 canvas and the E2 canvas). E2 IDs are either unrelated or recurring,
  and E1/E2 have an identical numeric domain. Result: no overlay, focus, selection,
  inspector or `onSelect`, and a new E2 drag focuses normally.
- Unit: a pressed E1 span released after a rebase selects nothing and reads nothing.
- Unit: zoom + pan + hover in E1, then rebase onto an identical numeric domain.
  The full default domain returns and the hint is empty.
- Unit: prepend/refresh focus tests now also assert the same Timeline instance.
- Browser, 1440/390px (`?long`): zoom/pan reset across Jump to latest, and a real
  mouse held across a Jump to latest rebase commits nothing. A fresh E3 gesture
  focuses normally.

| Command | Result |
| --- | --- |
| `pnpm typecheck` | Passed. |
| `pnpm build` | Passed, including artifact provenance. |
| `pnpm test` | 55 files / 996 tests passed. |
| `pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts` | 2 files / 71 tests passed. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` | 25 passed. The first run of the new test failed at 390px only because the test reused a canvas box measured before the toolbar lost its Jump button; the test now measures again. |
| `CONTAINER_ENGINE=podman pnpm test:e2e` | 100 passed, 1 failed. All Trajectory specs passed. The failure is `workspaces.spec.ts`: every assertion in the test body, including `errors == []`, passed. The trace then shows a stackless `TypeError: fetch failed` (ECONNREFUSED) in the `finally` block, after `a.stop()`. That is the known Product Host proxy fetch during context teardown, and it is unrelated to Trajectory. It was recorded as-is, not re-run into a pass. The previous lane on `f09fa6ca` passed 99/99. |
| `pnpm check:provenance` | 135 source records and 131 notices verified after structural rehash. |
| `cargo fmt --all -- --check` / `cargo clippy --all-targets --all-features -- -D warnings` | Passed; no native edits. |
| `git diff --check` | Passed. |

## PR #414 blocking repair: separate Timeline projection ownership (2026-09-26)

Starting HEAD: `b50589b2d36bb480c16a32fcf7c9f95b6d5ca8bd`.
Fetched PR #414 still targeted `main` at
`f268175bb8d31010706e7070aae80d2b46b7aced`; the clean implementation worktree
`/home/caismis/Documents/codes/rustX-issue-407` was five commits ahead, zero
behind. The main checkout's unrelated untracked `.playwright-mcp/` was untouched.
All seven starting GitHub CI checks passed. PR remained open, non-draft, with
no auto-merge request.

### Root cause and final ownership

Trace epoch identifies a replaceable read domain, not a fixed coordinate map.
The epoch key correctly retired state across `replaceTrace`, but same-epoch
prepend and overlapping refresh retained the pointer-down refs. Resetting only
the viewport from `start:end` neither retired gestures nor detected inner span
geometry changes with unchanged endpoints.

Committed focus remains `{ epoch, ids }` in `Trajectory`. A same-epoch projection
revision relocates those native IDs; an epoch rebase invalidates them even if IDs
recur. `Trajectory` now supplies its one Timeline model to both rendering and the
focus resolver; the child no longer builds a second copy.

`timelineProjectionRevision` serializes coordinate meaning: mode, domain, ordered
native span IDs/lanes/endpoints/phase positions and native Turn boundary positions.
It excludes display ordinals, labels, status and model object identity. Thus
ordinary renders and status-only refresh do not remount the interaction owner.
A changed semantic revision keys a new `TimelineInteraction` instance. The outer
Trace epoch key independently retires every instance on a read-domain rebase.

Pointer-down linearizes gesture ownership against the currently committed
projection instance. Publishing new geometry and retiring its predecessor's
refs/DOM happen in the same React commit, not a deferred reset effect. A later
pointer-up on the new canvas has no originating drag/pan/press; the detached old
canvas cannot dispatch a React interaction into the new model. Returning later
to an earlier geometry still creates a new instance, not its discarded refs.
Pointer-generated `click` is no longer an independent selection authority;
pointer-up selects only from a matching press. Keyboard/AT click activation is
still supported. Draft, viewport and hover are local to the same owner.

No native protocol/runtime or product/native vocabulary changes. No compatibility
paths, duplicate models, dependency changes or screenshot reference updates.
The obsolete same-epoch-instance assertion and numeric-domain reset effect were
removed. Provenance hashes/import closure and ownership notes were updated.

### Deterministic reproduction and evidence

- Sequence P1: `trace:0..3`, domain `[0,4]`. Pointer-down at 30%, move to 45%
  stores `[1.2,1.8]`, selecting `trace:1`. Same-epoch prepend inserts `trace:9`;
  P2 has domain `[0,5]`, where that old range instead selects `trace:0`.
  The test proves this changed mapping, rerenders P2, releases on both canvases,
  and asserts no focus, selection, detail read or Inspector. A fresh P2 drag
  selects exactly `trace:1`.
- Duration and Actual modes use real overlapping `refreshTrace`. An inner Tool
  span grows from one to two seconds. Actual mode keeps exactly the same outer
  start/end while coordinate meaning changes. Both old gestures retire; fresh
  gestures focus exactly `trace:1`.
- A pressed span followed by prepend and release (including pointer-generated
  clicks against detached/current spans) selects nothing. A new press selects
  the correct native record and opens Inspector.
- A pan begins before prepend. After rerender, the test zooms P2, delivers the
  old movement/release and proves its viewport remains unchanged. A fresh pan
  moves it normally.
- Committed focus survives prepend, shifted Turn ordinal/coordinates and status
  refresh. A further test preserves exact native-ID focus across the timed
  revision while its overlay width changes.
- Equivalent-geometry status refresh retains both the canvas and an in-flight
  gesture, proving the boundary is semantic rather than every render/lifecycle.
- Existing epoch rebase tests still cover unrelated/recurring IDs, identical
  domains, old drag/press retirement, viewport/hover reset and Jump to latest.
- Browser tests hold an actual mouse button across synchronous fixture prepend
  at 1440px and 390px, verify unchanged epoch and changed domain, then release
  without stale focus/Inspector and complete a fresh drag. No sleeps or race
  timeouts; the fixture action and DOM assertions establish the interleaving.

Red/green check: temporarily restoring both original production components made
all five new stale-gesture cases fail (three drag modes, span press/click, pan).
Restoring the correction made them pass. No temporary source changes remained.
An initial typecheck caught unsupported Testing Library `exact` options in four
new assertions; those were removed before the complete validation below.

### Validation

| Command | Result |
| --- | --- |
| `pnpm typecheck` | Passed. |
| `pnpm build` | Passed, including artifact provenance; existing large-chunk warning only. |
| `pnpm test` | 55 files / 1,003 tests passed. |
| `pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts` | 2 files / 78 tests passed. |
| `pnpm check:provenance` | Passed: 135 source records and 131 package notices. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `git diff --check` | Passed. |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` | 27 passed in normal comparison mode; desktop and 390px; no golden changes. |
| `CONTAINER_ENGINE=podman pnpm test:e2e` | 102 passed, 1 failed in `workspaces.spec.ts`; all 27 Trajectory cases passed again. Normal screenshot comparison; no golden changes. |

Full-lane failure evidence: Playwright `test.trace` records successful completion
of every test-body assertion, through `expect(errors).toEqual([])` at
`workspaces.spec.ts:116` (`expect@160`). The next event is a stackless
`TypeError: fetch failed` caused by `SocketError: other side closed`, before
After Hooks, during the test's shutdown/finally phase. This is the same teardown
failure class recorded on the starting PR; no Timeline flow failed. It is
reported as a failure, not retried into a pass or fixed outside this scope.
Local evidence: `/tmp/407-full-e2e.log` and
`web-console/test-results/workspaces-two-isolated-Pr-26dc1-onsive-Workspace-navigation/trace.zip`.
