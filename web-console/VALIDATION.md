# Historical Web validation records (before CFG3)

This file preserves dated evidence from earlier Web issues. Its configuration,
trust, protocol and UI ownership statements describe those historical commits,
not the current product. CFG3 supersedes them. For current contracts and validation,
use [configuration](../docs/configuration.md), [Web Settings](../docs/web-settings.md),
[conformance](CONFORMANCE.md) and [CFG3 validation](../docs/cfg3-validation.md).

# WEB-10 / #313 final Full Web acceptance — 2026-09-16

See [CONFORMANCE.md](CONFORMANCE.md) for the contract-to-test map and
[DOGFOODING.md](DOGFOODING.md) for developer steps independent of test source.
This record distinguishes browser composition from native owner proof.

## Repository and prerequisites

Original checkout `/home/caismis/Documents/codes/rustX` was clean at
`58f17e3c90f60275c3f58ebf9f9061dc0c5412e7` and remained unchanged/clean.
Implementation used `/home/caismis/Documents/codes/rustX-issue-313`, branch
`issue-313-web-conformance`, from fetched `origin/main`
`9980fc719a573ece0cab0114311345dc4c78b621`. Refetch before delivery found the same
base; no rebase was needed. #313 including its later #319 dependency comment,
#303, and prerequisite merged PRs were reinspected. #305, #306, #307, #308, #309,
#310, #311, #312 and #319 were closed and their merge commits verified ancestors
of the chosen base (individual merges are in CONFORMANCE.md).

## Commands actually executed

Linux, Rust 1.95.0, Node 24.20.0, pnpm 11.13.1. All final runs below passed.
Working directories are explicit; each semicolon-separated command in a cell was
run, not merely recommended. Ignored tests retain the repository's existing policy.

| Directory | Command | Result |
| --- | --- | --- |
| `web-console`, `tui`, `protocol/app-server` | `pnpm install --frozen-lockfile` in each | Passed, no lockfile changes |
| `test-support/fake-provider` | `uv sync --frozen`; `uv run --frozen pytest` | Passed; 51 Python tests |
| repository | `cargo build --bins` | Passed; real App Server and supervisors |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,894 passed, 1 ignored; bin/example harnesses passed |
| repository | `cargo test --test contracts --test provider --all-features` | 25 + 166 passed; 5 opt-in live Provider tests ignored |
| repository | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 127 durable, 52 process, 53 subagent, 157 tools, 23 conformance passed |
| repository | `cargo fmt --all -- --check` | Passed |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| repository | `git diff --check` | Passed |
| `protocol/app-server` | `pnpm check`; `pnpm typecheck` | Passed; no generated protocol drift |
| `tui` | `pnpm typecheck`; `pnpm test` | Passed; 816 tests, 96 suites |
| `web-console` | `pnpm typecheck`; `pnpm test` | Passed; 297 tests, 22 files |
| `web-console` | `pnpm check:provenance` | 74 destination records; 100 production dependency notices |
| `web-console` | `node scripts/provenance.ts --reference /home/caismis/Documents/codes/deepseek-harness-reference` | Passed; clean exact pinned HEAD and all 91 upstream input hashes |
| `web-console` | `pnpm build` | Passed, including generated license/provenance artifact checks |
| `web-console` | `pnpm test:e2e` | Production rebuild + 18 Chromium tests passed, approximately 1.2 minutes |

Focused Playwright runs for the chat, upload, recovery, settings, accessibility and
Workflow specs were also used during development, followed by the complete suite.
Vite reports its existing large-chunk warning (main bundle approximately 1.2 MB);
this is not a failed build. No dependency or raw-source runtime fetch was added.
The CI review retained existing deterministic/boundary/protocol/frontend/provenance/
production/browser classes and renamed the existing Web gate for final conformance.
No expensive parallel acceptance matrix was added.

## Failures found and repaired

- Real keyboard acceptance failed on arrow navigation for elements already marked
  as tabs. Shared presentation keyboard handling and labelled panels fixed it.
- Actual native HTTP MCP save rejected a new draft carrying an empty stdio
  command. Draft absence is now `null`; native validation stays authoritative.
- Running the documented launcher outside Playwright exposed a Node strip-only
  constructor error and missing Host carrier instructions. Explicit property
  initialization plus printed Host config/single-carrier ownership fixed startup.
- Test development corrected assumptions about separate provider system reminders,
  transport loss being `stale`, deliberately discarded drafts on Session navigation,
  controlled asynchronous checkbox updates, and newly labelled panel selectors.
  Provider capability validation now deliberately proves rejection before write,
  then supplies the required explicit replay policy. Subscription instrumentation
  uses the existing disconnect/connect API. These corrections do not add retries,
  sleeps, synthetic server state or browser semantic ownership.

## Provenance audit

External checkout HEAD was exactly
`c291e7961a515f6d7af9304e7fd1d257929aef26`, with a clean worktree. All 74 destination
records, 91 primary/additional source inputs and 201 recorded inspected paths were
checked. Existing notices cover 100 production packages; built notices match the
checked-in inputs. No new Harness source was imported. The existing records for
adapted Trajectory and Integrations were updated for this PR's changes; the shared
tab helper is rustX-authored. No Harness Host/Cordis/Session runtime, compatibility
gateway, branding substitution or build-time source download was introduced.

## Dogfooding actually performed and limitations

The standalone documented `pnpm --dir web-console dogfood:server
web_console_dogfood` launcher and production preview with
`RUSTX_WORKSPACE_HOST_CONFIG=... pnpm preview --port 4173 --strictPort` were run.
Separate exploratory Playwright scripts drove the real same-origin Host route
(without the E2E Host route interception): concurrent A/B, Settings, narrow MCP
forms, reconnect, approval/questionnaire and detached question. The provider's
shutdown report recorded `ok: true`, all eight requests consumed. Desktop and
390px screenshots were visually inspected; no page error or horizontal overflow
was observed. The servers were shut down afterward.

This was automated exploratory dogfooding plus screenshot inspection, **not a
human keyboard/screen-reader session**. The native browser-control tool returned
`No browser is available`; direct interactive dogfooding through that tool was
unavailable. The checked-in 18-test browser suite supplies the other documented
flows, including actual uploads/fork/delete and Workflow execution. Automatic idle
expiry is proven by native manual-clock tests; the browser exercises explicit
unload/cold reopen. macOS CI and Firefox/Safari were not run on this Linux machine.
No WCAG certification, OS sandbox guarantee, live paid Provider call, external MCP
service, secret-write endpoint or human manual pass is claimed.

---

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
| Close view / Open Session | Client relationship changes issue detach/attach; runtime residency is implicit |
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

## WEB-04 composer context docks (#307) — 2026-09-15; rebased 2026-09-16

Base: `origin/main` `289bd22e`. PR #320 (#319) is merged, so this slice sits on
Session-owned workspace uploads and **App Server v4**. It was first validated on
`e256998f` (WEB-03), then rebased onto `289bd22e` and revalidated end to end; every
command below is the rebased-head run, not the pre-rebase one. The Harness checkout
was reverified at `c291e7961a515f6d7af9304e7fd1d257929aef26`. Ownership is recorded
in [COMPOSER.md](COMPOSER.md) and provenance in [PROVENANCE.md](PROVENANCE.md).

### Owner audit

- Todo: `snapshot.todos` (`Option<TodoSnapshot>`) is present exactly when Todo is
  composed; `effective_extensions.todo` agrees. No Todo mutation exists.
- Goal: `snapshot.goal` (`Option<GoalView>`), `goal_changed` invalidation and
  `goal/control`. Stale CAS is `InvalidParams` with a serialized `GoalRejection`.
  GoalDomain names pause, resume, objective and budget as user controls.
- Queue: `snapshot.inbound.pending` and `snapshot.attempt`. `turn/start` and
  `turn/steer` dispatch to the same native `submit_inbound`.
- No App Server protocol, Rust owner or schema change. Generated v4 DTOs did not drift.

### Deterministic evidence (`test/composer-context.test.tsx`, 26 tests)

Todo absent/empty/tasks and native order, deleted tombstones hidden, projection-only
updates with historical `todo` Tool facts present, no Todo mutation or configuration
and storage writes; Goal active/paused/blocked/complete/absent, exact user controls,
pause/resume/objective/budget `goal/control` requests with the rendered GoalRef, no
configuration writes, open draft kept across revision-only change and dropped on
authoritative objective change, activation-only change without revision change;
Queue rows from pending inbound in sequence order, Send/Queue label from the attempt
with only `turn/start`; stack order and independent appear/disappear without
disclosure or draft leakage, per-Session isolation, reconnect rebuilding from a
changed server snapshot, shared column CSS. Race and loss cases use held fixture
responses, injected read failures and socket close, never sleeps.

### PR #321 review corrections

**Queue identity.** A `turn/start` in flight is composer transport state only. A
provisional Queue row is created only from `inbound_accepted`, keyed by its server
`MessageId`, and settles only when an authoritative snapshot contains that id.
Regressions: a held request shows *Awaiting acknowledgement…* with no Queue row,
count or client submission; acknowledgement creates exactly one row keyed by
`accepted-user`; identical text under another id does not settle it; the exact id
does; a lost acknowledgement leaves only the `turn/start` uncertain diagnostic, and
reconnect shows the committed native row with one request and no echo; disconnect
clears only accepted presentation rows and sends nothing.

**Goal convergence.** `controlGoal` separates outcome from authority: `observed` is
true only when a snapshot read issued after the outcome succeeded. The dock stays
locked after any unobserved or uncertain outcome until the client replaces the
snapshot. Regressions: applied + successful reread unlocks; applied + failed reread
keeps r3 rendered and every control disabled, a click sends no second control, and a
later event read renders r4 and unlocks; stale refusal + successful reread renders
the new Goal with one request; stale refusal + failed reread keeps the reason, locks
the old GoalRef, and recovers only on a later read; a component test proves the dock
itself stays locked for unobserved applied/refused outcomes until a new observation;
lost response stays uncertain with one request; an obsolete result leaves the dock
unlocked and silent.

**Goal validation ownership and protocol representability.** The Web budget form owns
input grammar and wire representability only; the hardcoded native ceiling and
consumption floor stay removed. `GoalMutation::Budget.rounds` is a Rust `u32`
(`src/goal.rs`), so the draft is parsed through `BigInt` and refused above
`4294967295`: a syntactically positive integer the protocol integer cannot hold never
becomes a typed mutation, and so can never round, reach `Infinity` or serialize as
JSON `null`. Regressions: `''`, `'0'`, `'2.5'`, `'1e3'`, `'+12'`, `'04'`,
`4294967296`, `4294967300`, and 40-digit and 400-digit decimals all leave the form
invalid and emit no `goal/control` at all; `4294967295` is representable, so the
browser sends it and the request serializes as exactly `"rounds":4294967295` for
GoalDomain to refuse. The fixture still refuses as GoalDomain does: 150 and a value
below consumption are sent, refused with the typed reason and unlock after reread;
the next deliberate value is applied. Source check confirms no `MAX_ROUND_BUDGET`
and no consumption floor remain in the dock.

### Real App Server acceptance (`test/e2e/composer.spec.ts`)

Scenario `web_composer_context` enables `agent.extensions.todo` and
`agent.extensions.goal`. The browser observes a composed-empty Todo strip; the model
creates native tasks and a Goal (budget 1). While the native continuation round is
gated, the browser sees Ongoing Goal at 1/1 rounds, the **Queue** label, and a pending
inbound row with no echo; order is Todo, Goal, Queue, Composer and all docks align
with the composer card. After release, pause, resume, pause, budget 3 and objective
edit each advance the durable revision by one through `goal/control`; focus returns
to the edit control. Settings and localStorage contain no Todo, Goal or queue text;
the Goal extension flag is unchanged. Disconnect disables controls; reconnect and
reload rebuild the same revision, Todo list and empty queue; alignment holds at
1440px and 390px with no horizontal overflow. The provider scenario must be fully
consumed, so no unexpected continuation round was admitted.

### Commands (Linux, rebased feature worktree)

Every row below is a rebased-head run. The Rust rows are not optional for this
slice: #320 brought substantial Rust, runtime and protocol change into the base,
and this branch now carries a Rust change of its own — the managed MCP handshake
fix recorded in
[pr-321-mcp-handshake-flake.md](../docs/pr-321-mcp-handshake-flake.md). The
`--lib` count below includes that fix's new regression test, so it is one higher
than the pre-fix figure quoted in the contention note above.

| Directory | Exact command | Result |
| --- | --- | --- |
| root | `cargo fmt --all -- --check` | Passed |
| root | `git diff --check` | Passed |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| root | `cargo build --bins` | Passed |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,850 passed; 0 failed; 1 ignored; 226 boundary tests filtered out |
| root | `cargo test --test contracts --test provider --all-features` | 25 contracts + 166 provider passed; five opt-in live-provider tests ignored |
| root | `cargo test --lib --all-features -- boundary_suites::` | 226 passed, 0 failed |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 401 passed, 0 failed: durable 116, process 52, subagent 53, tools 157, conformance 23 |
| test-support/fake-provider | `uv sync --frozen`; `uv run --frozen pytest` | 51 passed |
| protocol/app-server | `pnpm install --frozen-lockfile` | Passed |
| protocol/app-server | `pnpm check` | Passed; generated v4 schema/TypeScript showed no drift |
| protocol/app-server | `pnpm typecheck` | Passed |
| web-console | `pnpm install --frozen-lockfile` | Passed |
| web-console | `pnpm typecheck` | Passed |
| web-console | `pnpm test` | 181 passed, 15 files (26 composer-context tests) |
| web-console | `pnpm check:provenance` | 64 source records and 100 production package notices passed |
| web-console | `pnpm build` (via `pnpm test:e2e`) | Passed; existing non-fatal bundle-size advisory |
| web-console | `RUSTX_BINARY=../target/debug/rustx pnpm test:e2e` | 7 passed against the real App Server, including `composer.spec.ts` |
| tui | `pnpm install --frozen-lockfile` | Passed (required by web-console typecheck) |
| tui | `pnpm typecheck` | Passed |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 816 passed, zero skipped |

This is the Linux lane only; it makes no claim about the macOS-only
platform-boundary job, which GitHub Actions runs on the pushed head.

### Host-contention flake, recorded rather than waived

One early local run of `cargo test --lib --bins --examples --all-features -- --skip
boundary_suites::` reported `2848 passed; 1 failed` while the TUI integration suite
and the browser e2e run — both of which spawn the real `rustx` binary and the
provider emulator — were executing concurrently on the same host. Re-run alone, the
identical command reports `2849 passed; 0 failed; 1 ignored; 226 filtered out`,
matching `main`'s own CI at `289bd22e` exactly (2849 passed, 0 failed, 1 ignored,
226 filtered) and so confirming the same test set. This branch changes no Rust
source, manifest or toolchain — only `web-console/` and one fake-provider Python
scenario — so the transient failure was host contention, not a branch regression.
Nothing was retried until green, no test was skipped, weakened or excluded, and no
sleep was added; GitHub Actions on the pushed head remains the authoritative gate.

### Rebase onto App Server v4 `main`

This branch was rebased onto `289bd22e` (PR #320 merged), and every row above is the
rebased-head run, not the pre-rebase one. Conflicts were resolved by ownership rather
than by taking either side wholesale:

- `src/client/app-server.ts` keeps #320's `upload()` and receipt-carrying `send()`,
  and #321's accepted-`MessageId` submissions, `settleSubmissions` and `controlGoal`
  convergence. `send()` lost its `steer` flag and always calls `turn/start`.
- `src/app/components/InputBar.tsx` keeps #320's upload lifecycle (transfer-refusal,
  receipt retention, uncertain outcomes, paste/drop) and #321's single Send/Queue
  action; the Steer button is gone.
- `src/app/App.tsx` keeps `onUpload` wired through the `ComposerContextStack`.
- `test/artifacts.test.tsx` keeps #320's `session/upload` tests; the deleted
  `artifact/upload` modality-preflight tests were not resurrected.
- `CHAT.md` keeps #320's attachment section.

Every #321 import of `protocol/app-server/v3` moved to the post-#320 generated `v4`,
including the two `retained_dependencies` records in `source-inventory.json`, which
`pnpm check:provenance` re-verifies against each file's actual imports.
`Submission.content` is now `UserInputBlock[]`, matching what `send()` actually
builds. No v3/v4 or old/new upload compatibility path exists.

Because #320 carries substantial Rust, runtime and protocol change, the full Rust
contract and boundary suites were run locally on the rebased head rather than
skipped. That proved necessary: CI's macOS lane then exposed a real handshake
defect on the managed MCP path, so this branch also carries a Rust fix and its
diff is no longer Web-only. The defect, its root cause in rmcp's `Auto`
lifecycle, the fix and its residual risk are recorded in
[pr-321-mcp-handshake-flake.md](../docs/pr-321-mcp-handshake-flake.md).

## WEB-05 / #308 validation (2026-09-16)

Base: `5537d1e27240c0b239ea0ba8403e784e54bd3117`, including #304, #292,
#319 (native Session uploads), and #307. Work was isolated in the
`rustX-issue-308` worktree. The original checkout was not edited.
Harness HEAD was verified as `c291e7961a515f6d7af9304e7fd1d257929aef26`;
the shared provenance inventory records inspected and adapted sources.

### Commands and results

Commands below are from the repository root unless a directory is indicated.
The final diff changes Web source/docs and one deterministic provider scenario;
it changes no Rust source, protocol DTO, dependency manifest, or lockfile.

| Command | Result |
| --- | --- |
| `pnpm --dir web-console install --frozen-lockfile` | Passed |
| `pnpm --dir tui install --frozen-lockfile` | Passed; shared TUI dependency for Web checks |
| `uv sync --frozen` (test-support/fake-provider) | Passed |
| `cargo build --bins` | Passed; real App Server and supervisors |
| `uv run --frozen pytest` (test-support/fake-provider) | 51 passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 208 passed, 16 files |
| `pnpm --dir web-console exec vitest run test/commands.test.tsx` | Passed during focused development; included in the full run above |
| `pnpm --dir web-console check:provenance` | 68 source records, 100 production package notices passed |
| `pnpm --dir web-console build` | Passed; existing non-fatal bundle-size advisory |
| `pnpm --dir web-console exec playwright test test/e2e/commands.spec.ts` | Passed against the real App Server |
| `pnpm --dir web-console test:e2e` | 8 passed against the real App Server |
| `git diff --check` | Passed |

The current CI workflow was inspected. All applicable Web-lane checks and the
changed fake-provider suite ran locally. Rust-wide clippy/test/format and protocol
regeneration checks were not rerun for this Web-only implementation; neither Rust
nor generated bindings changed. No applicable validation was skipped for an
environment limitation. Chromium was already installed. GitHub Actions still
owns the full platform matrix, including macOS.

### Deterministic evidence

- Registry tests cover `/`, exact identities, localized/alternate aliases, fuzzy
  subsequences, stable tie ordering, keyboard selection/Escape, unsupported slash
  refusal, and ordinary prompt submission.
- Typed selector tests assert native catalog/current/setModel and approval
  requests, filtering/Enter, and stale Session/attachment completion fences.
- Historical tests assert exact target/node/message/revision, explicit rejection
  with no revision substitution, authoritative Fork-before-open, in-Session
  Branch, and upload receipts without browser file copies. An unavailable
  historical message cannot substitute another displayed boundary.
- Retry tests assert branch -> unload -> exact attach -> one native editor input
  submission; no previous Assistant replacement or browser history copying.
- Deferred response barriers commit and hold Fork/Branch/Retry, navigate, then
  release. The mutation remains committed but the continuation does not redirect.
  App-level tests exercise dismissal and navigation to Session B as well.
- Queue/Steer tests use authoritative running snapshots and assert `turn/start`
  versus `turn/steer` requests; idle always uses `turn/start`. Accepted queue
  edit/remove/reorder remains absent and deferred to #309.
- Lost responses for settings, lineage, create/compact, and each retry step stop
  continuation and never replay. Reconnect rereads native settings/Session state;
  old generation/socket callbacks cannot alter the new view.
- `commands.spec.ts` uses the real App Server and controlled provider to select
  model/approval, restore focus on Escape, reject unsupported slash input, retry
  upload-bearing history, reopen the original native node, independently Fork,
  verify destination upload bytes, and reconnect. Provider assertions require a
  fresh execution with no old Assistant response in its context.
- Desktop retry and mobile selector screenshots were visually inspected; the
  mobile test also asserts no horizontal overflow and no browser errors. Evidence
  lives in ignored `test-results/commands-*.png`, collected by CI.

No sleeps establish ordering: barriers, native acknowledgements/projections, and
Playwright assertions do; timeouts only guard liveness.

### Findings and corrections during development

Initial unit failures exposed fixture wait-count and selector ambiguity, plus
old callback/queue-label expectations; these were corrected without weakening
native request assertions. Initial provenance checks identified the new derived
sources and imports; the existing inventory was extended, not bypassed.
The first browser run found focus restoration blocked by disabling the composer
behind the modal; native modal inertness now handles that. A later provider
assertion incorrectly expected live model selection to persist into a cold branch:
native semantics correctly use Session/launch defaults on cold resume. The fixture
and documentation now state this lifetime explicitly. All final checks pass.

Native historical Surface revisions are immutable cuts: an older valid revision
is not inherently stale. Tests reject invalid revisions/boundaries and prove the
browser never replaces the selected cut with a freshly fetched one.

### Final architecture audit

One registry/grammar feeds explicit closed typed dispatch. No command interpreter,
arbitrary command string RPC, compatibility upload path, Harness runtime authority,
or second composer/queue/history state machine was introduced. `/settings` has no
current legitimate surface and is omitted. Goal navigation uses the existing dock.
Native branch/fork cuts and #319 upload ownership remain unchanged. Reconnect and
navigation invalidate UI continuations, never pretend to cancel committed native
mutations. Lost mutation responses remain uncertain; no browser idempotency or
automatic replay was added. Todo -> Goal -> Queue -> Composer is preserved.

## PR #322 review correction: native admission before Attempt projection

Reviewed head: `0def5008e9a8e637097aee82c763075fbd752d38` (2026-09-16).
The defect was treating `!activeAttempt(snapshot)` as permission to replace a
resident lineage. A native acknowledgement already proves accepted work even
when the replaceable snapshot has not projected an Attempt yet.

### Native-owner findings

1. `ConversationRuntime::admit_sourced_inbound` commits durable mailbox acceptance
   under the coordinator lock. Its returned MessageId/sequence names runtime-owned
   work; it is not completion. App Server start/steer both use this owner.
2. `accepted_inbound_before_attempt_admission_prevents_idle_claim` gates Attempt
   admission after acceptance, proves no current Attempt, and proves native idle
   reclamation is refused. Acceptance can precede Attempt projection.
3. Pending inbox observations are independent of the Attempt projection. Pending
   can coexist with absent/settled Attempt; Web tests cover both explicitly.
4. Manager unload drains admitted operations, invokes native shutdown, waits for
   settlement and projection drain, then releases composition. It is not a promise
   to execute every pending message before shutdown. The drain-linearization test
   proves pre-drain accepted input remains durably pending and later input is refused.
5. `SessionController::copy_lineage` reads the exact immutable Surface cut and
   retains allocation access. Source appends do not alter that cut. Independent
   Fork neither unloads nor replaces the source and remains allowed.
6. `compact_context` rejects current Attempt, manual compaction,
   lifecycle/durability conflicts, but intentionally does not reject pending inbox
   entries. It owns the Conversation while maintenance runs. `/compact` therefore
   has `no-attempt` availability, not the stronger lineage-switch condition.
7. App Server converts `UserInputBlock` in order; native `editor_input` maps each
   canonical block in place. The native upload test returns Upload/Text/Upload.
   TUI `app-server/editor.ts` preserves arbitrary unchanged ordering and rejects
   ambiguous edits; its session transport submits the array directly. Web normal
   send authors uploads then one nonempty text, while Retry submits native arrays
   directly. Headless composition uses ordered `UserContentBlock` admission rather
   than the Web editor or an alternative `UserInputBlock` normalizer. Thus the Web
   cannot assume every native producer uses its flat editable shape.

### Bounded implementation and deterministic regressions

`bindings/projection.ts::executionIdle` requires an observed snapshot, no active
Attempt, no authoritative pending inbound, and no unprojected acknowledged
submission. It is only a Web product guard, not a native idle lease or queue owner.
The existing exact-MessageId reconciliation is unchanged. App command discovery,
historical Branch/Retry, tree availability and open selectors use that definition;
native integration rechecks it before Branch/Retry mutation, after branch commit
before unload, and before tree switching. Compact follows the distinct contract
above. Fork remains available with accepted source work.

Gate-based tests hold admission acknowledgement ahead of projection, then project
the exact MessageId into pending and canonical history. They assert blocked
Branch/Retry/tree RPCs and controls, native-legal compaction, unchanged reconciliation,
and re-enabling at genuine idle. Separate absent/settled Attempt tests start with
authoritative pending and no provisional submission. Already-open selectors also
track acknowledgement and reconciliation. Commit/hold/admit/release tests prove
Branch and Retry preserve the published node, report its identity, never repeat
the branch, and never unload accepted source work. Fork has a positive busy-source
test. No sleeps synchronize any of these races.

Restored rows now key by `(batch_id, token)`. Two same-batch receipts render with
no React warnings; removing either independently preserves the other exact receipt
in the typed outbound payload. Following React's stable-data identity guidance,
the key does not depend on the array index.

The bounded flat editor supports uploads followed by at most one **nonempty** Text
block. It validates before decomposition and visibly disables unsupported restores;
it does not merge, reorder or send them. Tests cover Text/Upload/Text, multiple Text
blocks, and an empty Text block that the ordinary sender would otherwise drop.
Supported unchanged content round-trips in exact order. Retry remains unrestricted
by this editor and retains its direct ordered `sendContent` coverage.

### Validation of the correction

| Command (repository root unless stated) | Result |
| --- | --- |
| `pnpm --dir web-console install --frozen-lockfile` | Passed |
| `pnpm --dir tui install --frozen-lockfile` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console exec vitest run test/commands.test.tsx` | 42 passed |
| `pnpm --dir web-console test` | 223 passed, 16 files |
| `pnpm --dir web-console check:provenance` | 68 source records, 100 notices passed |
| `pnpm --dir web-console build` | Passed; existing bundle-size advisory |
| `pnpm --dir web-console test:e2e` | 8 passed, real App Server and provider |
| `cargo test --lib --all-features accepted_inbound_before_attempt_admission_prevents_idle_claim` | 1 passed |
| `cargo test --lib --all-features unload_and_inbound_have_both_native_admission_winners` | 1 passed |
| `cargo test --lib --all-features manual_compaction_` | 7 passed |
| `cargo test --lib --all-features restored_editor_uploads_are_ordered_owned_and_prepared_before_publication` | 1 passed |
| `cargo test --lib --all-features drain_linearization_precedes_the_refused_acceptance` | 1 passed |
| `git diff --check` | Passed |

Two initial typecheck iterations caught test-fixture DTO omissions and a
Playwright-only locator option used in Testing Library. Those test authoring errors
were corrected; no product failure was waived. No environment limitation blocked
validation. The existing binary/provider/browser installations were reused; Rust,
protocol, TUI and provider sources are unchanged by this correction. CI was inspected
and all affected Web checks rerun; targeted native tests verify the relied-on owners.

Browser plugin unavailable: repository Playwright used `127.0.0.1:5174` with the
real App Server fixture. Flow: connect/create -> selectors -> upload-bearing Retry
-> original node -> independent Fork -> reconnect. Page identity, meaningful DOM,
interactions, console health and mobile overflow assertions passed. Desktop
1440×1000 and mobile 390×844 evidence remains in the existing ignored CI screenshot
paths; screenshots were inspected. No rendering overlay or layout regression found.

No generic command RPC, browser queue authority, replay, #309 mutations, #319
compatibility shim, or canonical history rewriting was introduced. This correction
changes no native semantics or protocol. Main was refetched unchanged before push;
the existing PR #322 is the only publication target.

## PR #322 follow-up: unresolved inbound transport and discovered drafts

Reviewed head: `8e856c6d44c4378a68de9ba3076b42d25c00398a` (2026-09-16).
The prior correction remains intact. Its remaining gap was **before** browser
acknowledgement: native acceptance can commit while both `submissions` and the
idle-looking snapshot still contain no evidence of that work.

### Linearization and ownership

`ConversationRuntime::admit_sourced_inbound` calls `mailbox.accept_draft` under the
coordinator lock. That durable transaction commits sequence, MessageId and pending
record before returning `InboundAdmission`. `RuntimeClientHost::submit_session_inbound`
then constructs `InboundAccepted`; App Server `TurnStart`/`TurnSteer` dispatch maps
that result into the response. Socket delivery and browser observation happen later.
Thus a transmitted request and an idle-looking Web snapshot can coexist after commit.

Manager unload drains admitted operations, invokes shutdown, waits for native
settlement and projection drain, and releases composition. It does not promise to
execute all pending input first; pre-drain pending input remains durable. Compact
still has its own coordinator preconditions and permits pending inbound before
Attempt adoption. Independent Fork reads an immutable historical cut under source
allocation access, without unloading the source. Neither gains a transport-idle gate.

`executionIdle(view)` is unchanged. New `lineageSwitchSafe(view)` additionally
requires a current attached/wanted view and no `inboundRequests`. AppServerClient
derives this count from its actual bounded pending request map, not React's sending
flag. Registration publishes it before pumping the socket; concurrent calls are
counted separately, including requests still waiting for a transmission slot.

The four stages are:

1. Unresolved inbound request: transport ownership, no invented MessageId.
2. Acknowledged MessageId awaiting projection: existing reconciliation evidence.
3. Authoritative pending/canonical/Attempt observation: runtime ownership.
4. Stable lineage-switch frontier: none of the above remains unresolved/active.

Success publishes the decremented request count and acknowledged MessageId together,
then uses existing exact-identity reconciliation. No intermediate safe state is
published. Definite rejection removes request ownership without a submission.
Transmitted response loss remains OutcomeUncertain, invalidates the generation and
attachment, and is never replayed. Reconnect rereads native state without retaining
old generation counts. Old socket/command continuations remain fenced.

The stronger guard is used by command availability, historical Branch/Retry,
Session-tree controls, open selectors, native Branch/Retry before mutation and
after publication, and native other-node opening before unload. Already committed
branches remain discoverable when continuation stops. No runtime/protocol change.

### Draft success semantics

The composer records stable command identity plus the **exact invocation draft**.
Success clears only an unchanged matching draft, covering `/model`, `/mdl`, `/`,
and `/模型`. Dismissal and failure preserve it; edits made while a selector is
pending survive success. Panel success and close callbacks are separate: `/tools`
consumes on successful native read while keeping its panel open. No parser expansion,
event bus, immediate-on-selection clearing, or focus workaround was introduced.

### Deterministic and browser evidence

Tests separate `server.commit(request)` from `socket.deliver(response)` for both
start and steer. Before delivery they assert zero branch/unload/child-attach effects,
even though native commit has happened. A state subscriber proves no safe publication
from send initiation through acknowledged and pending reconciliation. Additional
cases cover transmission-slot waiting, concurrent known refusals, loss/reconnect,
old socket rejection, per-Session isolation, open-selector updates, and concurrent
unacknowledged admission after branch publication. Fork/Compact have positive tests
with held inbound acknowledgement. Earlier pending/settled/acknowledged guards,
upload identity, ordered input refusal and exact Retry tests remain passing.

Draft tests cover exact/fuzzy/bare/alias success, cancellation/failure preservation,
concurrent edits and tools read success/failure. The real App Server browser flow
now explicitly uses `/mdl` and checks successful `/tools` consumption with the panel
still open, alongside existing Fork/Retry/reconnect and focus tests. No sleeps
synchronize races. Desktop/mobile screenshot and console/layout checks passed.

| Command | Result |
| --- | --- |
| `pnpm --dir web-console install --frozen-lockfile` | Passed |
| `pnpm --dir tui install --frozen-lockfile` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console exec vitest run test/commands.test.tsx` | 59 passed |
| `pnpm --dir web-console test` | 240 passed, 16 files |
| `pnpm --dir web-console check:provenance` | 68 source records, 100 notices passed |
| `pnpm --dir web-console build` | Passed; existing bundle-size advisory |
| `pnpm --dir web-console test:e2e` | 8 passed, real App Server |
| `cargo test --lib --all-features accepted_inbound_before_attempt_admission_prevents_idle_claim` | 1 passed |
| `cargo test --lib --all-features unload_and_inbound_have_both_native_admission_winners` | 1 passed |
| `cargo test --lib --all-features drain_linearization_precedes_the_refused_acceptance` | 1 passed |
| `cargo test --lib --all-features manual_compaction_` | 7 passed |
| `cargo test --lib --all-features exact_fork_boundary_excludes_delete_and_releases_metadata_lock` | 1 passed |
| `git diff --check` | Passed |

Initial typecheck iterations caught old test callback names after the success/close
split and a nonexistent `fireEvent.cancel` convenience method; tests now dispatch
the native cancel event. All final checks pass. No environment limitations or
waived checks. Current CI configuration was inspected; native binaries, provider
and Chromium installations were reused because those sources/dependencies did not
change. Full platform CI remains GitHub-owned. Browser plugin was unavailable, so
the repository Playwright workflow was used at `127.0.0.1:5174`.

## WEB-07 / issue #310 (2026-09-16)

Base: `5e5359cd0687944e0a40ee83b47be77e2d018385` (latest `origin/main` after
fetch). Prerequisites #292/#304/#308/#309 were closed. Work used the dedicated
`rustX-issue-310` worktree and `issue-310-workspace-manager` branch. The original
`rustX` checkout remained clean on main. The issue and parent Epic #303, repository
instructions, current CI, and pinned Harness sources were inspected before editing.
See [WORKSPACES.md](WORKSPACES.md) for the concrete Host contract and source-trust
lifetime, and [PROVENANCE.md](PROVENANCE.md) for inspected/adapted/excluded sources.

### Coverage and evidence

- Host filesystem/HTTP tests prove finite authorized handles, exact canonical cwd
  grouping, metadata persistence/order/rename/unregister, disabled picker,
  unauthorized path rejection, endpoint binding and cross-origin refusal. Host
  metadata has no Session IDs or trust flags.
- Component/client tests cover cold listing without attach, separate selected
  Workspace/Session, no cancellation/unload on navigation, unknown/untrusted Settings
  refusal, capability-gated picking, exact Host cwd creation, typed native rename
  and exact-boundary Fork. Deferred resolution and manually delivered wire replies
  fence late search, rename refresh, cold open and committed Fork responses. No
  sleeps synchronize these races. Existing command races continue to pass.
- Native tests cover bounded durable cwd projection after cold catalog reopen and
  metadata/settings changes, unloaded residency without composition, read-only
  trust projection, and untrusted composition/reload ignoring malformed project
  config/resources. A later external grant cannot widen a loaded generation.
- Playwright runs against actual rustX child processes and HTTP Product Hosts.
  Two independent fixtures reject foreign handles/endpoints, keep registrations
  separate and resolve identical process-local Session IDs to different durable
  cwd values. An untrusted cold Session remains listed with zero runtimes, resumes
  through App Server, retains native history/settings, and cannot activate project
  instructions through reload. Host rename/order/unregister leave its native state
  unchanged. This proves process/Host separation, not OS filesystem sandboxing.
- Real browser checks cover grouped/flat views, collapse, narrow/wide layouts,
  no horizontal overflow, and the existing chat/commands/composer/connection flows.
  Desktop/mobile evidence is in the ignored `test-results/workspaces-*.png` artifacts;
  both were visually inspected. The Browser plugin was unavailable, so the existing
  repository Playwright workflow was used.

### Commands and final results

Commands run from the feature worktree unless a working directory is stated.

| Command | Result |
| --- | --- |
| `pnpm --dir web-console install --frozen-lockfile` | Passed |
| `pnpm --dir tui install --frozen-lockfile` | Passed |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 259 passed, 18 files |
| `pnpm --dir web-console check:provenance` | 70 source records and 100 notices passed |
| `pnpm --dir web-console build` | Passed; existing bundle-size advisory |
| `pnpm --dir web-console test:e2e` | 9 passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo build --bins` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,861 passed, 1 existing ignored; bin/example targets passed |
| `cargo test --test contracts --test provider --all-features` | 25 contracts + 166 provider passed; 5 opt-in live tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 127 durable + 52 process + 53 subagent + 157 tools + 23 conformance passed |
| `uv sync --frozen --project test-support/fake-provider` | Passed |
| `uv run --frozen pytest` (in `test-support/fake-provider`) | 51 passed |
| `pnpm --dir protocol/app-server generate` | Generated Rust-owned schema/TypeScript |
| `pnpm --dir protocol/app-server check` | Passed; no generated drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 816 passed |
| `git diff --check` | Passed |

During implementation, old native tests expecting all untrusted cwd admission to
fail were updated to assert inactive project sources instead. TUI fixtures needed
the newly required summary fields; an initial fixture-invalid TUI run was stopped
and rerun successfully after the typed fixtures were updated. Browser deletion
coverage was updated for the new native Session action menu. One external-boundary
run exposed an existing extension-reload history test's incomplete-publication
race (`ext256_reload_cannot_recompose_extensions_but_the_next_launch_does`); the
complete external lane passed on repeat without changing that unrelated test.
No required Linux check was waived. The macOS CI lane cannot run on this Linux host
and remains GitHub CI coverage.

### Architectural audit

1. Removing a Workspace registration leaves its Sessions untouched: **yes**.
2. Authorized but untrusted cwd is usable with project sources inactive: **yes**.
3. Workspace navigation can mutate project trust: **no**.
4. Typing a path in the browser can authorize it: **no**; the old cwd form and
   alternate `/new` creation path were removed/replaced.
5. Cold Sessions can be grouped without starting runtimes: **yes**.
6. Switching Workspace/Session cancels or unloads unrelated work: **no**.
7. There is one authoritative durable Session cwd: **yes**, `SessionPersistentState`.
8. Workspace authorization belongs outside App Server: **yes**, Node Product Host.
9. Host metadata and native project trust are separate facts: **yes**.
10. Stale async search/navigation can reopen an obsolete Session: **no**.

The adapter deliberately supports an operator-configured finite root set, exact
canonical root grouping and one writer per metadata file. It is a local/trusted
adapter, without OS directory browsing, remote authentication or a filesystem
sandbox. These boundaries are explicit in the Host contract and deployment docs.

## PR #324 review repair (2026-09-16)

Reviewed/start head: `d42bdab1feb604c40e69f8df070f3162ef5aed5e`. The worktree
was clean on `issue-310-workspace-manager`; the complete reviews and unresolved
endpoint thread were read. `origin/main` remained
`5e5359cd0687944e0a40ee83b47be77e2d018385`, so no integration/rebase was needed.
This repair changes only Web Host/navigation code, its consumers/tests and docs.
Rust, native trust/configuration and generated protocol artifacts are unchanged.

`classifyLocations` separates operator-root authorization from Workspace
registration. The Web admission owner reads current native durable settings and
asks the Host before the client's single new-attachment entry sends `session/attach`.
Missing admission policy refuses. Saved views/reconnect, toolbar attach, sidebar
Open/Fork and command transitions use that entry; Fork also authorizes its source
before creating a child. Active focus is a single Session/Workspace pair populated
by `focusSession`, with an empty Workspace while classification is pending or the
Session is unregistered. Endpoint binding and routing share URL identity.

Deterministic coverage includes registered authorized open exactly once, authorized
unregistered cold open without registration recreation, unauthorized current cwd
(including trusted native project state), stale summaries, saved hints/reconnect,
toolbar resume, sidebar Fork and attached-source Fork refusal, missing admission
owner, late authorization across navigation/connection generations, and repeated
Open after supersession. The top-tab A/B `/new` regression checks exact Host
Workspace resolution and native create cwd; unregistered A cannot reuse B. Host
tests cover canonical aliases, actual descendant refusal, URL slash/host casing/
default-port/dot normalization and rejection of different hosts/ports. No sleeps
synchronize new tests.

Playwright extends the two-process fixture with an outside-root native Session and
saved tab: it remains visible and unchanged with zero loaded runtimes after denied
restoration/open. An unregistered authorized Session still resumes through the
toolbar without recreating its registration. Existing untrusted-source inactivity,
Fork/retry/native uploads, reconnect, and narrow/wide layouts remain covered.
Browser plugin not available; the existing Playwright workflow was used.

| Command | Final result |
| --- | --- |
| `pnpm --dir web-console exec vitest run test/workspaces.test.tsx test/workspace-host.test.ts test/client.test.ts` | 59 passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 273 passed, 18 files |
| `pnpm --dir web-console check:provenance` | 70 records / 100 notices passed |
| `pnpm --dir web-console build` | Passed; existing bundle-size advisory |
| `pnpm --dir web-console test:e2e` | 9 passed |
| `git diff --check` | Passed |

Current CI was reread. No Rust/protocol changes require additional native suites.
During repair, existing presentation tests were updated to permit the new read-only
cwd check during focus changes. The provenance check caught the endpoint import
until its inventory was updated. Browser validation caught a restored Fork draft
being cleared by reconnect reclassification; the common focus transition now
preserves that draft. The new browser refusal assertion was made exact because
both the row and the refusal notice legitimately display authorization status.
All final checks passed after those fixes.

Final invariant audit, in review order: **no, no, no, no, yes, no, no, no, yes,
yes, yes, no**. New Web attachments cannot bypass Host admission; registration
remains separate from authorization, authorization remains separate from trust,
and native Session cwd remains the sole durable authority. No Workspace authority
was added to App Server. The adapter's finite exact roots, single metadata writer,
and lack of OS sandbox/remote authentication remain its explicit boundaries.


## SESSION-01 supersedes historical residency workflows

The current App Server removes public manual unload and list residency. Earlier recorded
runs mentioning those operations describe superseded behavior, not current UX.
Current flows: open/resume implicitly ensures residency; Close view detaches;
branch switching owns replacement; confirmed deletion owns manager retirement.
The focused Session can be deleted without switching first. Inspector retains
attachment/incarnation observations, while server diagnostics own residency.
