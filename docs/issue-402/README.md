# Issue #402 implementation and acceptance

[PR #404 review corrections and regression evidence](review-corrections.md).

## Repository and inspection

Implementation worktree: `/home/caismis/Documents/codes/rustX-issue-402`.
Branch: `issue-402-harness-conversation-models`.
Implementation base: `da43450b77d3c195d95b06818be8142317663c91` (latest fetched
`origin/main`, including #401/#394). Other worktrees were not modified.

The complete issue was read before implementation. Inspection covered App,
WorkspaceNavigation/Host/navigation, AgentControls/Composer/Transcript/Status/Tool,
Tool/status/projection bindings, all affected agent/Markdown primitives,
ModelsPage and Settings machines/forms/primitives, App Server/transcript clients,
unit and browser fixtures, CI and generation/provenance/browser scripts, and the
Agent/Chat/Settings/Workspace/provenance architecture documents. Native response,
transcript, durable lineage and generated contracts were inspected before design.
Main exposed App Server 20, Runtime Client 46 and SQLite schema 43.

## Ownership and commit boundaries

- App owns an explicit New Conversation or Session center route, with a navigation
  epoch. New Conversation owns only Workspace selection, text, browser Files and
  optional Session model intent. It does not create a Session on opening or
  Workspace selection. Host catalog/resolve/adopt remain the only Workspace
  authority; no arbitrary path field was added.
- `firstSubmitMachine` is XState: drafting → creating_session → attaching_session
  → optional applying_session_model → individually uploading_attachments →
  submitting_turn → session. Known pre-commit rejection returns to editable drafting; only a new explicit submission retries. Uncertain creation and committed-Session failures have no replay transition.
- A confirmed `session/create` is the Session commit point. Its real native IDs
  survive later failures; the UI opens that Session with an accurate failure.
  An uncertain create leaves no fabricated identity and tells the user to inspect
  native Sessions before another attempt.
- Explicit model intent uses the existing Session mutation owner, then an
  authoritative snapshot reread. Both configured and effective model (and explicit
  reasoning profile) must match before upload/send. Workspace default configuration
  is untouched.
- Each acknowledged upload receipt is recorded individually. Later upload/send
  failure does not roll back prior receipts or Session creation. Uncertain
  mutations are never retried automatically; native reread/reconnect is recovery.
- Every step checks endpoint, connection generation, authority revision,
  navigation epoch and, once attached, exact native attachment target. Stopped
  XState promise actors cannot publish into replacement routes.
- The permission seat uses the same Settings target and Unit transaction actors
  as Settings, through Product Host registered Workspace identity and exact source
  revision/CAS. Only `policy` and `full_access` are displayed. Source intent,
  commit/reread outcome and admitted Attempt permissions remain distinct. Multiple
  presentations retain one target actor; closing one does not detach another.
- Models keeps existing TanStack typed fields, semantic-unit writes, source repair,
  CAS, external-change handling, application observation and focus restoration.
  Cards say Configured/Inherited and separately expose application observations;
  they do not claim network health.

## Native/protocol change

App Server **21**, Runtime Client **47**, SQLite schema **44**. No compatibility
alias, fallback schema or migration was added. Rust DTOs/schema/serialization,
generated TypeScript, fixture imports, version tests, Web/TUI/dev consumers and
current protocol documents change together.

`completed_process` publishes immutable response origin plus local final-message
identity on exactly owned Assistant and Tool transcript entries. Native
AssistantMessageCommitted/AttemptCompleted evidence supplies membership;
Tool occurrence identity supplies its owning Assistant. Active members remain
pending. Durable lineage stores/remaps exact member IDs and validates retained
Assistant ownership. Pagination never invents or merges identities.

Web disclosure state is a local Set keyed by native origin and final identity.
Final text/actions, User/steering input, live process and independent failures
remain visible. Agent Status shares disclosure only by exact native
Conversation/Attempt and remains at its original anchor. Canonical entries/cache,
ordering, pagination and settlement are unchanged. Independently owned subagents
are not assigned to a process by call-ID coincidence.

## Pinned Harness reuse and libraries

Pin: **`ddefc45fbc7f8e46dd73185e68295696d1297887`**, unchanged.
The complete inspected/adapted/excluded file mapping, including exact source
paths, is in [PROVENANCE](../../web-console/PROVENANCE.md#issue-402--conversation-and-models-convergence).
The machine-readable [inventory](../../web-console/source-inventory.json) records
SHA-256, import closure, treatment and exclusion notes. 135 source records and
131 production-package notices are checked. There is no build/runtime fetch.

Adaptations include EmptyHero/HeroShell; the existing InputBar composer;
WorkspacePicker; PermissionSelect/RiskConfirmation; ReasoningRow compact Markdown;
ContextInjectionRow status chrome; TurnProcessNodeView; ToolRow's TerminalBlock,
DiffBlock, ReadBlock, SearchBlock, FoldToggle and copy/cap helpers; and
ModelsSection/ProviderEditor card/detail grammar. Existing CodeBlock/Shiki is reused.

Inspected Tool model builders (`terminal-card-model`, `diff-card-model`,
`read-card-model`, `search-card-model`) were excluded because their Harness runtime
metadata is not rustX authority. Harness stores/controllers/preset catalogs,
credential writes, provider reachability claims, event reduction and fish branding
were excluded. Terminal ANSI interpretation was excluded; native bounded text is
shown verbatim. Read/Search use the opaque body where native structured positions
are unavailable. Diff shows explicitly labelled requested Write/Edit changes,
without inventing a filesystem before-image.

Existing libraries reused: XState/@xstate/react (first submit and shared Settings
actors), React Aria (Menu/GridList and existing selects), Base UI (dialog focus,
Escape and restoration), Floating UI (existing menu placement), TanStack Form
(typed Settings fields), TanStack Virtual (existing bounded lists), existing
Markdown parser/security/streaming stack, Shiki and diff (code/diff presentation).
Existing resizable panels remain the shell owner. **No dependency added.**

## Removed behavior

Removed primary New Session modal and generic center instructions; the duplicate
creation helpers; ordinary Ungrouped pseudo-Workspace and search label; absent
permission seat; generic-only specialized Tool bodies; status-only left offset;
permanently expanded completed process; verbose Models landing composition.
Unclassified real Sessions remain reachable through a bounded disclosure/flat
search. No feature flag, legacy route, duplicate composer or Settings flow remains.

## Deterministic proof mapping

| Tests | Invariant proved |
| --- | --- |
| `first-submit.test.ts` | No creation before submit/without Workspace; exactly once; model gate precedes upload/send; committed Session and individual receipts survive later failure; no replay; stopped actor and changed authority fence each phase using controlled promises |
| Workspace/Host/command authority tests | Exact Host resolution before creation; stale resolution cannot create/attach; adoption uses authorized handles; Workspace switching creates nothing; unclassified Sessions survive |
| Settings machine/unit/source/recovery suites | Exact revisions and target ownership; conflicts retain drafts; confirmed write versus reread failure; uncertain writes never replay; shared presentation lifetime and fresh observation |
| Session configuration/model suites | Native model observation; application failure distinct from active; admitted/native state separate from authored configuration |
| `agent.test.tsx`, Agent Status tests | Exact native Tool dispatch/lifecycle/artifacts; compact versus normal Markdown; status facets, 24px disclosure axes and suppressed-result anchors |
| `turn-process.test.tsx` | 0/1/N calls; multiple reasoning/messages; final/live/User outside fold; interleaved native identities; pagination/retry identity; canonical immutability; status folds without moving anchor |
| Native response/archive suites | Native Attempt membership across steering/interleaving/pages; lineage remapping/validation; final response ownership; protocol/version strictness |
| Models Settings/browser suites | Exact Add/Edit/Delete units, inherited overrides, invalid-source repair, application/commit truth, narrow overflow and focus restoration |
| Real convergence browser scenario | Host adoption, zero pre-submit creates, explicit model before first turn, unchanged default, real terminal/diff, folded/expanded final answer, keyboard permission confirmation, axe at every capture |

## Browser evidence

All captures use the real application, native App Server and isolated Product Host
with the scripted provider; they are not static component mockups. Pinned browser:
Playwright 1.63.0 noble container, Podman. Dark theme throughout this matrix.

| Scenario | Viewport | Evidence | Result |
| --- | --- | --- | --- |
| New Conversation | 1440×1000 | [01](browser/01-new-conversation-desktop-dark.png) | Pass |
| New Conversation narrow, permission/model seats | 390×844 | [02](browser/02-new-conversation-mobile-dark.png) | Pass |
| Workspace picker | 1440×1000 | [03 picker](browser/03-workspace-picker.png) | Pass |
| Host-authorized Add Workspace | 1440×1000 | [03 add](browser/03-add-workspace.png) | Pass |
| Native permission menu | 1440×1000 | [04](browser/04-permission-menu.png) | Pass |
| Elevated confirmation | 1440×1000 | [05](browser/05-elevated-confirmation.png) | Pass |
| Draft Session model picker | 1440×1000 | [06](browser/06-new-conversation-model-picker.png) | Pass |
| Completed process folded, final visible | 1440×1000 | [07](browser/07-process-folded.png) | Pass |
| Expanded reasoning/Tools/status | 1440×1000 | [08](browser/08-process-expanded.png) | Pass |
| Expanded native Bash terminal | 1440×1000 | [09](browser/09-terminal.png) | Pass |
| Expanded native Write diff | 1440×1000 | [10](browser/10-write-diff.png) | Pass |
| Models Provider cards | 1440×1000 | [11](browser/11-models-desktop.png) | Pass |
| Models narrow | 390×844 | [12](browser/12-models-mobile.png) | Pass |

Manual review checked alignment, spacing/density, typography, disclosure rhythm,
focus/menu geometry, mobile wrapping and hierarchy against pinned source. Review
corrected dialog inner spacing, long Provider action labels, mobile rail settling
and collapsed-row layout space. Existing Agent/Settings/Shell references were
intentionally regenerated for the new route label, exceptional Session disclosure,
Tool bodies, permission seat and Provider cards; strict pixel comparison and noise
policy were not relaxed. Shell fixture now provides a Workspace source for its
permission seat instead of displaying an unrelated unavailable-Host error.

Intentional differences: rustX branding, exact native permission/model vocabulary,
Host-authorized location choices, truthful provenance/application labels,
unclassified-Session disclosure and opaque native Read/Search bodies. Full typed
Provider/Model editing remains available in details.

## Validation and development iterations

Commands run from the isolated worktree. Final source is checked with frozen
lockfiles and the repository's pinned browser authority; no acceptance threshold
or native test was disabled.

| Command | Result |
| --- | --- |
| `git diff --check` | Pass |
| `pnpm --dir web-console install --frozen-lockfile` | Pass |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | Pass: 52 files, 920 tests |
| `pnpm --dir web-console check:provenance` | Pass: 135 source records, 131 notices |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-402-harness` | Pass against exact pinned checkout |
| `pnpm --dir web-console build` | Pass; existing large-chunk advisory remains |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Pass: all 89 scenarios, snapshot updates disabled |
| `cargo build --bins` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets --all-features` | Pass |
| `CARGO_BUILD_JOBS=2 cargo test --workspace --all-targets --all-features` | Pass: 3,893 tests, 7 pre-existing ignored tests |
| `CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `pnpm --dir protocol/app-server install --frozen-lockfile`, `pnpm --dir protocol/app-server generate` | Pass |
| `pnpm --dir protocol/app-server check` | Pass: regeneration matches staged reviewed contract |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir tui install --frozen-lockfile`, `pnpm --dir tui typecheck`, `pnpm --dir tui test` | Pass: 852 tests |
| `pnpm --dir dev install --frozen-lockfile`, `pnpm --dir dev typecheck`, `pnpm --dir dev test` | Pass: 37 tests |
| `uv sync --frozen`, `uv run --frozen pytest` in `test-support/fake-provider` | Pass: 51 tests |

The seven pre-existing native ignores are an instrumented profile, a corpus
regenerator and five credential-dependent live-provider tests. No new ignore was
added. Docker is unavailable; the supported `CONTAINER_ENGINE=podman` path runs
real browser acceptance. One rerun encountered ports 5173/5174 occupied by the
concurrent #396 worktree and stopped before tests; that worktree/process was not
modified. The rerun proceeded after those ports were released.

Development failures were retained as findings, then repaired:

- Initial native parallel compilation exhausted available memory; the owned run
  was stopped and subsequent full gates used two jobs. Initial pinned-source
  clone had a transient SSL failure; retry and exact-pin provenance verification
  succeeded.
- Initial TypeScript/unit integration exposed obsolete modal/Provider landing
  expectations and a generated Ajv2020 import typo from a version replacement.
  The typo and consumers were fixed. An attempted promise-read delay caused 21
  Settings actor regressions and was reverted; Session actor acquisition instead
  moved to React commit. The unchanged actor machines then passed.
- Native test iterations found five stale SQLite/Runtime version assertions,
  then one documentation contract and one process handshake assertion. Final
  full native tests passed after exact version updates. Clippy rejected a
  101-line projection function; extracting entry construction resolved it.
- TUI iterations found two stale protocol-20 assertions; final 852 passed.
- Early convergence browser runs exposed a captured preconnection endpoint,
  obsolete Tool locators, and rail-brand contrast. They were fixed. Broad browser
  acceptance initially reported 61 pass/27 fail: intentional screenshot changes,
  the Session actor render notification, stale shared source observation,
  reload-based fixture draft loss and obsolete folded/unclassified reachability.
  Shared actors and normal native fixture attachment corrected the real issues.
- Targeted six-scenario acceptance initially had two failures: a changing
  disclosure locator and unclassified rows after registry refresh. Stable
  locators/explicit native refresh observation corrected them. The reference
  pass then had 41 pass/2 fail: cold statuses now fold and Models cards use the
  pinned 12px/14px padding and 16px radius. Assertions were updated to the new
  contract; strict screenshot checks remain unchanged.
- The next targeted pass had 18 pass/1 fail: axe found unsupported
  `aria-expanded` on the textarea. Expansion belongs to the existing Commands
  trigger, so the invalid textarea attribute was removed. WCAG 2/2.1 A/AA axe
  acceptance now runs for every convergence capture. The final targeted
  Shell/convergence reference run passed all 10 scenarios.
- A provenance iteration correctly rejected ToolCard source drift while review
  removed obsolete diff fields; hashes/import closure were refreshed and both
  local and exact-upstream checks passed.
- A full strict browser run passed 87/88: a native-Session fixture helper wrongly
  required the location label to be visible at 390px. It now asserts the exact
  native location text, retaining responsive hiding. The subsequent full 89-scenario rerun passed.
- A second full strict browser run again passed 87/88. Its only mismatch was a
  reconnect reference that incorrectly depicted a connected Session. The actual
  capture correctly showed Connection interrupted; both images were inspected
  and the reference was corrected without changing pixel tolerances.
- A new direct Provider-card deletion browser test exposed Enter propagating from
  a native button to the React Aria row action. The trigger now uses the existing
  React Aria Button/onPress semantics. The focused test proves Escape returns
  focus, confirmation writes exactly the Provider unit at `user-1`, and focus
  settles on the surviving landing page. That targeted test passed.

Final review also clarified the Provider deletion confirmation: deleting source intent may reveal inherited configuration; it does not claim that existing Session application is unchanged. The direct-card browser regression was rerun after this wording-only change.
