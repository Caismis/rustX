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
