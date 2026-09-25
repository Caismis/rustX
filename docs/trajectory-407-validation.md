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
