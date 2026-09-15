# Issue #319: Session-owned workspace uploads

The [PR #320 correction record](../docs/pr-320-review-validation.md) supersedes
the initial validation for cleanup, editor restoration, resolution layering, and
committed upload uncertainty.

See the [complete validation record](../docs/issue-319-validation.md) and
[upload contract](../docs/session-uploads.md). Web sends native receipts; typed
canonical metadata rebuilds transcript attachments after reconnect. The browser
acceptance suite also retains managed Tool image decoding/lightbox coverage.

---

# PR #316 blocking-review correction: canonical code settlement

The initial PR CI run [34917901069](https://github.com/Caismis/rustX/actions/runs/34917901069)
failed Developer Web Console: **101 passed, three failed**, all awkward-chunk
settlement equalities in `markdown.test.tsx`. Provenance/build/E2E were skipped in
that job. The original local 104-pass result below was real but insufficient.

Reproduction on reviewed HEAD `4d45c704501fa04893ddf8dc681e904837032835`:
`pnpm test` again passed 104 locally. Adding the controlled lazy-grammar regression
without changing production code then failed both cases. Thus passing the old suite
locally did not establish timing independence.

Cause: CodeBlock returned `previous.body` after settlement when an unchanged
highlighted streaming cache existed. That tree had a direct React `<pre>` and
React-serialized styles; cold settlement used Shiki HTML inside a `<div>`. If Rust
registered during streaming, the cache selected the first final representation.
If registration happened after settlement, both paths used the second representation.
The original tests did not control this async boundary; CI exposed the loaded arm.
This was an architectural defect, not an irrelevant serialization difference.

Correction: `streaming !== true` clears the incremental session and line cache.
There is no `settledRef` or retained-stream settled mode. The settled HTML memo no
longer depends on streaming output. Every highlighted settled fence uses the same
cold-render path. Parser/freezing, incremental tokenization, grammar laziness,
security and App Server code are unchanged.

New `code-settlement.test.tsx` resets module state and gates the **real Rust grammar
import** through a test-only mock. It observes plain output, releases the gate,
awaits the actual registration notification inside React act, observes highlighted
output, and compares complete DOM including wrappers/styles after settling the same
instance against a cold mount. It covers registration before/after settlement and
already-loaded streaming. Eager TypeScript and full-document tests now assert
canonical settled equality instead of identity across settlement. All awkward-chunk,
CRLF, frozen-prefix, completed-line retention and bounded-state assertions remain.

All local correction checks passed: `pnpm typecheck`; three consecutive independent
`pnpm test` runs (106 tests/eight files each); `pnpm check:provenance`; `pnpm build`;
`pnpm test:e2e` (five passed); protocol `pnpm check`/`pnpm typecheck`; `cargo fmt --all -- --check`;
`git diff --check`. Results and the corrected-head GitHub CI outcome are also
recorded in the PR description after inspecting the completed run. No retry setting,
arbitrary sleep, grammar eager-loading or weakened equality was added.

The historical #304 record below predates this correction. Its statement about
preserving DOM identity on settlement is superseded: retention applies **during
streaming only**; final presentation is canonical and history-independent.

---

# Issue #304 validation record

Recorded 2026-09-15, Linux; Node 24.20.0, pnpm 11.13.1. Original worktree
`/home/caismis/Documents/codes/rustX`, branch `main`, HEAD
`6dd1ef144fdfedbd99cb0c9dd9856e1e8defbf51` remained clean and unchanged.
Feature worktree: `/home/caismis/Documents/codes/rustX-issue-304`, branch
`issue-304-web-foundation`. Initial and final fetched base:
`5a066dd642cbbaad8309fc7756daf4937b4fdf0a` (#292 merged). Base CI run
34915683408 was successful. Issues #304/#303/#289 and WEB-02 #305 were inspected.

## Checks executed

| Directory | Command | Result |
| --- | --- | --- |
| web-console | `corepack enable`; `corepack install`; `pnpm install --frozen-lockfile` | Passed, including final frozen install after dependency additions |
| tui | `pnpm install --frozen-lockfile` | Passed; required by existing shared-server browser test |
| web-console | `pnpm typecheck` | Passed |
| web-console | `pnpm test` | 104 tests, seven files passed |
| web-console | `pnpm check:provenance` | 50 source records/import boundaries and 98 production dependency notices passed |
| web-console | `node scripts/notices.ts --write` | Generated complete installed production-closure notices; subsequent check passed |
| web-console | `pnpm build` | Passed, including production notice-byte verification |
| root | `cargo build --bins` | Passed; ordinary real App Server and Tool supervisors |
| test-support/fake-provider | `uv sync --frozen` | Passed |
| web-console | `pnpm exec playwright install chromium` | Passed; Playwright used its Ubuntu 24.04 fallback binary for this Linux distribution |
| web-console | `pnpm exec playwright test foundation.spec.ts` | Four focused browser contracts passed after fixes |
| web-console | `pnpm test:e2e` | Five tests passed: original real-server scenario plus four foundation contracts |
| protocol/app-server | `corepack install`; `pnpm install --frozen-lockfile`; `pnpm check`; `pnpm typecheck` | Passed; no generated schema/DTO/fixture changes |
| root (also from web-console) | `cargo fmt --all -- --check`; `git diff --check` | Passed |

No Rust code changed, so a new full Rust contract/conformance/clippy run was not
necessary. Existing real-server acceptance and protocol generation were actually
executed, not inferred from unchanged source.

## Deterministic evidence

- Original 43 native client/presentation/log tests retained. The sole existing
  presentation-test edit changes the Questionnaire import after its layer move.
  `test/e2e/console.spec.ts` is unchanged and still runs the production build
  against the real Rust server/provider emulator and shared TUI client.
- Incremental parser tests record grammar inputs: settled paragraphs leave later
  parse slices; an 800-line open fence parses bounded completed-line slices.
  Awkward 1/3/7/16-unit chunks, surrogate pairs, CRLF splits, lists and fences match
  fresh-prefix rendering. Frozen DOM nodes retain identity.
- A separate 1200-line structural traversal finds one retained code AST node,
  fewer than 30 retained state objects, and fewer than four source-lengths of
  retained strings; closing the fence converges with a fresh parse. No timing
  benchmark is used as correctness evidence.
- Rich semantic tests assert headings, emphasis, lists, links, quotes, inline/fenced
  code, tables and KaTeX. Awkward math/delimiter chunks settle identically to a
  one-shot document. Raw scripts, unsafe links, images/local paths and trusted TeX
  commands cannot gain executable/document or workspace authority.
- Highlighting tests compare incremental tokens with fresh tokens, preserve CRLF,
  unknown-language fallback, completed line arrays and 32-line DOM groups, and
  preserve unchanged highlighted DOM on settlement.
- Chromium tests exercise actual Enter/Space, menu arrows/Home/End/typeahead,
  disabled items, Tab exit, Escape/restoration, outside dismissal, popover focus
  entry/tabbing and modal focus wrapping/restoration. A nested popover remains
  interactive inside a native dialog and Escape closes the inner surface first.
- Geometry tests at 390, 900 and 1440 pixels inspect real bounding boxes and no page
  overflow. The fixture imports only foundation CSS, proving independence from
  console styling. AppFrame remains presentation geometry with explicit children.
- Provenance checks parse static/dynamic imports, verify recorded dependencies and
  forbid presentation-to-product/protocol/client layer escapes. Build verifies
  both notices verbatim in dist. Original pinned-source hashes remain available
  for comparison with the separate reference checkout.

## Fixes found during validation

Initial typecheck caught a removed image-vocabulary export and absent installed
TUI dependencies; both were corrected. Imported highlighting tests initially
expected deliberately excluded Markdown/Ruby grammars; their relevant contracts
were adapted to the selected Rust/Python closure, with CRLF assertions retained.
A duplicate Python first-load assumption was corrected to use the untouched Rust
lazy grammar. The first dialog browser test exposed Tab movement toward browser
chrome; explicit first/last focus wrapping now passes. Shared anchored surfaces
were moved outside shell clipping and tested inside the native modal.

## Architecture review

Generic presentation imports no app, bindings, client or generated protocol.
Product cards now live in app/components, especially the native Questionnaire
DTO/draft owner. Canonical messages remain server-projected plain text. Markdown
ASTs, frozen elements, code token state and open/focus state are disposable browser
presentation caches. No Harness runtime packages, RPCs, notifications, mutations,
reconnect rules or interaction-lifecycle changes were added.

The source inventory distinguishes inspected files from actual derivation. The
external reference remains detached at the exact required SHA with a clean tree.
Existing #289 headers were made explicit, and imported code/math/parser dependencies
are pinned and licensed. README documents the two frontiers, linear retained-state
bound, full settlement pass and known reference/math streaming behavior.

## Browser QA and limitations

Browser skill/plugin unavailable; repository Playwright used. Automated localhost
acceptance exercised production console plus isolated foundation fixture, page
identity, meaningful content, no Vite overlay, interaction state and screenshots.
The generated desktop/mobile console and foundation screenshots were inspected.
These are automated browser observations, not a separate manual dogfooding run.
Chromium only; no paid provider, other browsers or new WEB product surfaces tested.

No environment blocker prevented a required check. Non-failing tool warnings:
Playwright's Linux fallback distribution, Node FORCE_COLOR/NO_COLOR, and Vite's
500-kB chunk warning. The main bundle is approximately 1 MB minified (about 274 kB
compressed), plus lazy Rust/Python grammars and KaTeX fonts. This bounded closure
has a real payload cost; no performance claim is based on wall-clock test times.
Streaming TeX remains literal and cross-frontier references self-heal at settlement.
Long unstable paragraphs/lists or single lines still incur tail parsing, as
explicitly documented; this is not a general constant-time document engine.

---

# Issue #289 validation record

The attachment-lifecycle review correction and its fresh validation are recorded
in [REVIEW-301.md](REVIEW-301.md). The initial implementation record below predates
that correction.

Recorded 2026-09-14 on Linux, Node v24.20.0, pnpm 11.13.1, Cargo 1.95.0 and
uv 0.11.12. This records executed checks, not a claim of manual browser coverage.

## Base and ownership audit

Initial fetched `origin/main`: `cd5b9a04a1f0cb1403059c01b723be4f90e1811f`.
A final fetch before validation returned the same SHA; no integration was required.
The base includes #288 via PR #298 and #36 via PR #299. The original worktree
`/home/caismis/Documents/codes/rustX` stayed clean at
`a81dd97ec900b9dbf9c97cd9c4cc2504fd35ee2e`; work was isolated in
`/home/caismis/Documents/codes/rustX-issue-289` on
`issue-289-deepseek-harness-web-console`.

Source inventory verification compared every original SHA-256 with an independent
checkout of Harness `c291e7961a515f6d7af9304e7fd1d257929aef26`: **29 file mappings**,
including the MIT notice, all matched. At inspection, 14 destination files were
byte-identical to upstream; 1,529 nonempty lines matched in order after ignoring
leading/trailing whitespace across mapped sources. These counts are supporting
inspection evidence; the inventory and explicitly described extractions, not a
similarity threshold, define source ownership.

The final source/import/package review found only React, React DOM and clsx as
production direct dependencies. Harness semantic/Host/Remote references remain only
in provenance comments. Runtime requests are native generated `Request1` unions;
no Harness wire client, backend, browser event reducer, persistent conversation,
provider setup, branding assets or compatibility service is retained. Browser
storage writes only endpoint/tab preferences. All source, license and lockfile
inputs needed for the production build live in rustX.

## Executed validation commands

Commands are relative to the repository root unless a directory is named. Repeated
runs during fixes are condensed; final outcomes below refer to the final relevant
code. No frontend lint configuration was introduced, so there is no lint command.

| Directory | Command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | PASS |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| root | `cargo build --bins` | PASS; real rustx and native Tool supervisor binaries |
| root | `cargo test --lib --all-features app_server` | PASS, 10 native wire/schema tests |
| root | `cargo test --test process --all-features app_server` | PASS, 8 real process/stdio/WebSocket/configuration tests |
| `protocol/app-server` | `pnpm install --frozen-lockfile` | PASS |
| `protocol/app-server` | `node generate.mjs` | PASS during generator repair |
| `protocol/app-server` | `pnpm check` | PASS; re-generated Rust schema, TypeScript and fixtures did not drift |
| `protocol/app-server` | `pnpm typecheck` | PASS; Rust-produced fixtures and reference/default type contracts |
| `web-console` | `pnpm install` | PASS; initial dependency/lockfile creation, with registry retries |
| `web-console` | `pnpm install --frozen-lockfile` | PASS |
| `web-console` | `pnpm typecheck` | PASS |
| `web-console` | `pnpm test` | PASS, 33 deterministic tests |
| `web-console` | `pnpm build` | PASS; standalone Vite production assets and notices, no Harness fetch |
| `web-console` | `pnpm exec playwright --version` | PASS, 1.63.0 |
| `web-console` | `pnpm exec playwright install chromium` | PASS; Chromium installed using Playwright's Ubuntu 24.04 fallback for this Linux distribution |
| `web-console` | `pnpm test:e2e` | PASS, one focused real-server browser scenario |
| `test-support/fake-provider` | `uv sync --frozen` | PASS |
| `test-support/fake-provider` | `uv run --frozen pytest` | PASS, 51 emulator tests |
| root | `git diff --check` and `git diff --cached --check` | PASS after final whitespace correction |
| root | `git diff --exit-code -- protocol/app-server` | PASS, generated artifacts clean after validation |
| root/original worktree | `git status --porcelain=v1`, `git rev-parse`, `git fetch origin main`, source/import and final diff inspection | PASS; isolated branch, original files untouched, base unchanged |

Initial checks deliberately exposed and then verified fixes for: TypeScript `$ref`
field loss and default intersections; native Questionnaire custom-answer shape;
fixture/socket typing; the native 32-Session list-page bound; create returning
`session_transition`; unload-close/reply ordering; an undersized truncation test
input; and a provider final-report stdout drain deadlock. The first staged whitespace
check found an extra blank line in dependency notices, which was removed. These
failed development attempts are not counted as final passing evidence.

The existing TUI suite and the entire 3,000+ Rust test suite were not rerun: no TUI
implementation, Rust runtime, or public wire/schema semantics changed. Existing CI
checks are preserved; protocol checks moved intact from the TUI job into a separate
job, and the Web Console has its own frozen-install/typecheck/test/build/browser job.

## Deterministic regressions

| File / coverage | Invariant established |
| --- | --- |
| `test/client.test.ts`: initialize and incompatible negotiation | Uses Rust-serialized capability fixture and generated unions; rejects unsupported versions/capabilities before Session work |
| Pipelining | Reverse-order responses resolve the correct request |
| A/B streaming and committed messages | Independent target routing; one canonical message replaces its same-ID streaming projection |
| Disconnect/manual timers | Last facts become stale; no fabricated cancellation or automatic reconnect |
| New connection delivery | Old socket notifications/responses and obsolete attachment targets cannot mutate a fresh snapshot |
| Resync and pre-attach notification interleaving | Authoritative snapshot plus subscription at its exact string cursor repairs observation without a browser event log |
| Approval/Questionnaire disconnect, fresh client and absent-client publication | Pending facts are read from the server, never recovered from a browser Promise/store |
| Lost create/delete/turn replies | Transmitted side effects stay uncertain and are never replayed |
| Lost interaction acknowledgement and A/B resolution | No fabricated settlement or blind resend; only the matching Session's authoritative absence removes controls/uncertainty |
| Explicit detach/unload/cold attach | Lifetime controls issue native operations only on explicit calls |
| Close-before-unload reply and same-connection attachment replacement | Native reply order is accepted; old reads and unload acknowledgements cannot retire a newer target |
| Native unload error | Route is retired without claiming successful shutdown or unloaded residency |
| Response microtask after disconnect | Old request continuation cannot publish into a new connection |
| Eight transmitted calls plus queued interaction | Unsent interaction is discarded, not falsely classified as a transmitted uncertain response |
| Pause during notification processing | Log presentation pause never pauses correctness processing |
| Endpoint validation | URL credentials/query/fragment never become handshake/log inputs |
| `test/presentation.test.tsx`: source-derived shell | Boots without Host; unsupported Harness controls absent |
| Tab switch/unmount (original coverage) | Presentation-only; the former close assertion was incorrect and is replaced by the review regressions linked above |
| Streaming/Approval rendering | Canonical replacement has no duplicate; authoritative resolution removes obsolete Approval actions |
| Questionnaire stale/reconnect and duplicate option labels | Disabled while stale; answer identity is native option index, not display label |
| Native Review | Preserves native instance and subject digest and removes controls on authoritative resolution |
| `test/diagnostics.test.ts`: log limits | Count/byte bounds and explicit truncation/drop reporting |
| Filter/pause/clear/resume | Deterministic bounded view behavior |
| Native response Session identity | A create response is filterable before attach |
| Questionnaire wire encoding | Native i64 strings, finite binary64 hex, text/boolean/options/custom and partial answers |
| `protocol/app-server/type-contracts.ts` | Generated MessageBlock retains referenced ID/content; exact string revisions do not intersect numeric default annotations |

## Real-server browser evidence (automated)

`test/e2e/console.spec.ts` ran Chromium against the real Rust binary, actual
WebSocket authentication/JSON-RPC, Vite's production-served UI, native bash/ask_user Tools, SQLite Session
state, canonical user configuration and the existing external HTTP/SSE provider
emulator. No Harness process was started.

Observed through the source-derived UI:

1. Initialize, create/attach A and B with distinct explicit cwds, keep both tabs.
2. Hold A at provider gate `finish-a`, use B and commit its response while A runs.
3. Disconnect/reconnect A, then reload/re-enter the socket token and recover A's
   streaming state and both tab IDs; release the gate and observe committed output.
4. Recover an Approval across disconnect/reload; allow a real bash Tool and see its
   result/committed response. Recover a Questionnaire across reload and answer it.
5. Detach A before the provider publishes another Questionnaire; await the exact
   completed-response barrier, reattach and answer the server-owned interaction.
6. Change the canonical server settings file; verify the loaded model stays
   `console-model`, explicit unload/cold attach resolves `second-model`, and native
   history/cwd survive.
7. Inspect/filter actual raw protocol; pause/resume its presentation.
8. Verify desktop and 390px viewport render without horizontal page overflow;
   inspect saved desktop/mobile screenshots. Check for browser errors, stored
   transport token, and fake provider credential appearing in rendered content.
9. Explicitly unload B, preview/delete it through native revision-checked deletion,
   observe `deleted`, and remove its obsolete presentation tab.
10. Require all eight provider requests/responses to satisfy their scripted contract
    and require provider process exit code 0.

Screenshots are at `test-results/console-desktop.png` and `console-mobile.png` in
the retained worktree; CI uploads browser evidence. Screenshots were visually
inspected. They are automated rendered evidence, not a manual browser run.

## Separately exercised launcher / manual limitations

`pnpm dogfood:server` successfully started private fake-provider configuration and
a real WebSocket App Server. A separate Node WebSocket CLI probe used the printed
endpoint/token file to initialize, create two explicit-cwd Sessions, attach both and
list them. This verified the documented launcher and transport, **not the browser
UI**. Stopping that probe fixture before any model turn correctly returned exit 1
with 0/8 provider steps; it was an intentional incomplete-scenario shutdown check.

The interactive computer/browser tool returned exactly **“No browser is available”**
when asked to open the console. Its Browser plugin skill was also unavailable.
Consequently the separate manual browser checklist was blocked: no manual claim is
made for clicks, long-action concurrency, disconnect/reload, interaction recovery,
inspector controls or cold resume. Those paths have the automated evidence above
and a complete operator procedure in README.md. No paid/real model, real workflow
Review, Subagent run, Todo or Goal execution was manually exercised; their exposed
native facts use generic renderers, and Review response wiring has deterministic
coverage. This frontend run does not claim transport slow-consumer/backpressure
conformance beyond the existing #36 process tests.

## Historical WEB-01 scope decisions (superseded below)

The pinned upstream commit is unchanged. No Rust runtime or public protocol/schema
change was needed. The only shared protocol change is the compiler-input
normalization and generated TypeScript correction, with compile-time regressions.
All Cordis/Host boot services were excluded because plain prop-driven extracted
presentation has the smaller coherent dependency closure. Specialized Subagent,
Workflow and Goal views were replaced with native read-only JSON disclosures.
Recovery is explicit reconnect; no automatic reconnect loop or mutation retry is
provided. Rich Markdown/image/IDE/provider configuration features remain excluded.

## WEB-02 historical validation

PR #317 established Chat, Markdown, transcript paging and managed Tool artifact
presentation. Issue #319 replaces its user-upload architecture and acceptance
coverage; the old upload/provider-modality and cumulative shared-upload-capacity
tests are obsolete. Historical validation is retained in PR #317. Current
upload semantics and limits are documented in CHAT.md and docs/session-uploads.md.

## WEB-03 native Trace / Trajectory — 2026-09-15

Validated on Linux in the isolated `issue-306-native-trace-trajectory` worktree,
based on `94dfa0ae77b49619e2c164428cce75b4bcf7267d` (including WEB-02).
This section supersedes the earlier App Server v2 statements: the mandatory
App Server contract is now v3; separate native Runtime Client envelopes are v35.

### Final validation

Repository-root commands:

| Exact command | Result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,821 passed, 1 pre-existing ignored process-writer helper; bins/examples passed with 0 tests |
| `cargo test --test contracts --test provider --all-features` | Contracts 25 passed; provider 166 passed, 5 pre-existing opt-in live tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | Conformance 23, durable 116, process 52, subagent 53, tools 157 passed (401 total) |

Package commands (each executed in the directory shown):

| Directory | Exact commands | Result |
| --- | --- | --- |
| `test-support/fake-provider` | `uv sync --frozen`; `uv run --frozen pytest` | Pass; 51 tests |
| `protocol/app-server` | `corepack enable`; `corepack install`; `pnpm install --frozen-lockfile`; `pnpm check`; `pnpm typecheck` | Pass, including generated Schema/TypeScript/fixture drift |
| `web-console` | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test`; `pnpm check:provenance`; `pnpm build`; `pnpm test:e2e` | Pass; 145 component/client tests in 14 files, 56 source records, 100 production dependency notices, 6 browser tests |
| `tui` | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass; 810 tests, no skips |

The production build reports the existing bundler's non-fatal chunk-size advisory
for chunks over 500 kB. `.github/workflows/ci.yml` was re-read; the additional local
CI commands are the emulator sync/pytest and mandatory-emulator TUI run above.
The macOS platform lane cannot be claimed from this Linux run and remains CI
coverage. No failing test was skipped.

### Deterministic owner evidence

`src/runtime_client/trace/tests.rs` covers exact request ordinals within one Step,
transient/context-overflow/corrective failure classes, historical model metadata,
request usage, reopen equality, read cuts during progress, finite pagination,
unchanged native heads/transcript/frontiers, parallel Tool completion and exact
call/Tool correlation, unknown/incomplete results, cancellation/timeout, deep
historical compaction boundaries, live identity matching, encoded-byte bounds,
and distinct Workflow run identity despite an identical definition/call name.
Sensitive prompt/parameter/schema/Tool fixtures remain withheld; Unicode and
JSON escaping exercise the actual public bounds.

The direct App Server test
`trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts` holds a
real provider request at an explicit gate. It proves historical Trace reads leave
the entire snapshot/live cursor unchanged, then proves attach/snapshot repair
before and after controlled settlement. Trace introduces no recovery input.

`trace-cache.test.ts` checks its independent bounded cache, overlap replacement,
pending history with live updates, stale-response fences, reconnect, reattach and
resync. `trajectory.test.tsx` uses measured layout to verify bounded virtualization,
Attempt/Step folding, retries, search/filtering, supported inspector sections,
unavailable/redacted/truncated presentation, stable selection/removal, follow at
the tail, scroll-away suspension, stable prepend, and payload updates above/below
the reader anchor. View switching remains on one native attachment.

### Real App Server acceptance

The existing `web_chat_history` provider-emulator flow now creates 34 historical
turns and pages native Trace through the actual App Server. In Chromium it checks
32→64 Trace records, stable prepend geometry, request/Step identity, redacted
input inspection, tail position, live execution during disconnect/reconnect,
scroll-away preservation through settlement, canonical Tool/artifact identity,
record selection and reconnect deduplication. The original rich Chat, transcript,
image decode/lightbox, the then-current upload assertions have since been replaced by #319 receipt acceptance.
There is no alternate fake product path or extra subscription.

All six existing browser flows pass, including two-Session ownership and shell
geometry at 390/900/1440 pixels. Browser evidence includes
`test-results/trajectory-inspector.png` and `test-results/chat-history.png`;
the Trajectory screenshot was visually inspected. The existing CI browser-evidence
artifact collects these files. The Trace inspector assertions exclude the private
fixture workspace path; raw internal request fields never enter its DTO.

Fixture repairs reflect the new contract: Session-tab counts now exclude the
Chat/Trajectory tabs; exact-u64 validation checks JSON number types instead of
digit substrings inside opaque Trace cursors; version rejection checks reject the
obsolete and next unsupported versions. The hot-journal fixture now dirties large
**valid JSON** so expression indexes can evaluate it while retaining the same
crash-before-commit/nonmutating-preflight recovery invariant.

### Deliberate omissions

See [Trace architecture](../docs/trace.md) for the complete policy. Arbitrary
request/Tool payloads are withheld, not secret-scanned; unaccepted publication
bodies remain in Chat's existing audit presentation. Harness-only semantic
categories, server search, telemetry and TUI Trajectory are excluded. Interaction
settlement remains in the existing Chat controls. The exact inspected paths and
adaptation subset are in [PROVENANCE.md](PROVENANCE.md) and the existing inventory.


## PR #318 architectural review corrections

Validated on 2026-09-15 in `/home/caismis/Documents/codes/rustX-issue-306`,
branch `issue-306-native-trace-trajectory`, against reviewed head
`595f9b959631497e4989fa2504b5f9dc8e16bc5e`. Fetched `origin/main` remains
`94dfa0ae77b49619e2c164428cce75b4bcf7267d`; no upstream integration was needed.
The original main checkout remained at `6dd1ef144fdfedbd99cb0c9dd9856e1e8defbf51`.
This section supersedes the preceding Trace cut/cache and SQLite version claims.

### Deterministic regressions

- `trace_snapshot_cut_excludes_commits_after_cursor_capture`: parks the worker,
  captures snapshot/cursor/prefix, blocks Trace materialization with channels,
  commits another Journal fact, and delays its fold. The returned Trace excludes
  that fact at the old cursor. An independent historical read does not move the
  live cursor; ordinary repair and reattachment converge with one stable record.
- `journal_cut_observer_reports_commits_but_never_reads_or_rollbacks`: committed
  prefixes publish, rejected duplicate inserts/read guards/rolled-back writes do
  not. There is no Trace mutation or durable cache.
- `old_background_and_workflow_records_are_repaired_and_settle_by_identity`:
  pushes both native starts beyond the newest page with 68 later Step anchors,
  pages back, repairs both to running by exact native IDs, then settles both.
  The old fixed cut still shows running; new-cut patches update the same IDs to
  terminal with authoritative durations, even against stale running overlays.
- `trace_schema_indexes_are_required_and_queries_use_them`: every required
  index is present; missing/redefined indexes fail structural validation. The
  actual reader SQL uses the required index for all nine scopes, both ordering
  directions, without a full scan or temporary sort. Schema 34 is rejected by
  `schema_34_without_fixed_trace_index_contract_is_rejected`.
- `loaded_lifecycle_refresh_is_bounded_and_never_repeats_internal_request_input`:
  512 interests are finite, 513 are rejected; private prompt/schema/provider/MCP
  fields never enter patches. Existing retry, Tool correlation, redaction,
  missing-terminal and timestamp tests continue to pass.
- Web tests cover contiguous-window rebasing without tail overlap, terminal patch
  application, selected inspector stability, settlement while older paging is
  pending, newly revealed prefix ordering, and restoration of paging in a fresh
  resync epoch. Existing stale-response, reconnect, folding, virtualization and
  scroll-anchor tests remain enabled. No race regression uses sleeps.

### Commands and results

All commands below passed after corrections; no failure was waived.

| Directory | Exact command | Result |
| --- | --- | --- |
| root | `git diff --check` | Pass |
| root | `cargo fmt --all -- --check` | Pass |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| root | `cargo build --bins` | Pass |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,827 passed; one existing ignored process-writer helper; bins/examples passed |
| root | `cargo test --test contracts --test provider --all-features` | 25 contracts + 166 provider passed; five existing opt-in live tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 401 passed: durable 116, process 52, subagent 53, tools 157, conformance 23 |
| test-support/fake-provider | `uv sync --frozen`; `uv run --frozen pytest` | Pass; 51 tests |
| protocol/app-server | `corepack enable`; `corepack install`; `pnpm install --frozen-lockfile`; `pnpm check`; `pnpm typecheck` | Pass; generated v3 Rust Schema/TypeScript drift checked |
| web-console | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `pnpm test` | Pass; 150 tests in 14 files |
| web-console | `pnpm check:provenance`; `pnpm build` | Pass; 56 source records, 100 production package notices |
| web-console | `pnpm test:e2e` | Six real App Server/browser tests passed |
| tui | `pnpm install --frozen-lockfile`; `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass; 810 tests, no skips |

The existing real-server Chat/Trajectory acceptance includes model and Tool work,
multiple Trace pages, exact native identity inspection, continued live work,
scroll-away/follow, reconnect without duplicate records, and secret/path checks.
The generated App Server contract remains v3; SQLite advances independently from
34 to 35. No additional upstream Harness code or dependency was introduced.
CI was re-read. This Linux run does not claim execution of the macOS-only lane.
The production bundle retains its existing non-fatal chunk-size advisory.

### Second-review corrections: final local validation

Current regressions add a real Background COMMIT/native-publication barrier,
real SQLite interaction publication barrier, cross-owner receipt ordering,
Workflow revision-chain delivery, parked-reader storage-lifetime independence,
non-overlap rebase with stale-page fencing and retained selected-record repair,
and canonical Tool artifact-only/deduplication cases. The earlier command table
records the previous reviewed head; final-head validation is recorded separately.


The final corrected code passed the following commands (Linux). The unchanged
original main checkout remained clean. App Server generated v3 DTOs did not drift;
SQLite schema 35 and all required index/query-plan contracts remain covered.

| Directory | Exact command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | Passed |
| root | `git diff --check` | Passed |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| root | `cargo build --bins` | Passed |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,833 passed; one existing fixture-generation test ignored |
| root | `cargo test --test contracts --test provider --all-features` | 25 contracts + 166 provider passed; five opt-in live-provider tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 401 passed: durable 116, process 52, Subagent 53, Tools 157, conformance 23 |
| test-support/fake-provider | `uv sync --frozen` | Passed |
| test-support/fake-provider | `uv run --frozen pytest` | 51 passed |
| protocol/app-server | `pnpm install --frozen-lockfile` | Passed |
| protocol/app-server | `pnpm check` | Passed Rust schema/TypeScript generation check |
| protocol/app-server | `pnpm typecheck` | Passed |
| web-console | `pnpm install --frozen-lockfile` | Passed |
| web-console | `pnpm typecheck` | Passed |
| web-console | `pnpm test` | 152 passed, 14 files |
| web-console | `pnpm check:provenance` | 56 source records and 100 production package notices passed |
| web-console | `pnpm build` | Passed; existing non-fatal bundle-size advisory |
| web-console | `pnpm test:e2e` | Six passed against the real App Server |
| tui | `pnpm install --frozen-lockfile` | Passed |
| tui | `pnpm typecheck` | Passed |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 810 passed, zero skipped |

Earlier development runs exposed and corrected headless observation interference,
Workflow revision delivery gaps, and runtime/projection-worker reference lifetimes.
Two boundary retries also encountered PyPI network-unreachable errors; the full
unchanged command passed after connectivity recovered, without offline mode,
changed dependency settings, sleeps, weakened assertions or excluded tests.
The table above records full successful runs after the code corrections.

The macOS job budget is 40 minutes: the previous reviewed-head run spent about
19 minutes in cold compilation and reached passing external targets before the
old 25-minute job deadline cancelled it. Native test liveness limits are unchanged.
Final-head GitHub Actions results are recorded on PR #318 after all seven jobs
finish; this local record alone makes no claim about pending remote jobs.


The first pushed correction (`83eeaa84`, Actions run 34966252256) exposed one
remaining test synchronization error in the Linux deterministic lane:
`detach_never_mutates_background_or_mailbox_state` observed durable request #2
and immediately assumed its mailbox-drain observation had published. The test
now awaits the existing native attempt-settlement signal before checking the
projected drain. The detached-running/mailbox-preservation assertions are
unchanged; no delay, retry or assertion weakening was added. This is the same
COMMIT/publication distinction enforced by the production cut contract.
