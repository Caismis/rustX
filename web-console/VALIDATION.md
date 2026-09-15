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

## WEB-02 (#305), 2026-09-15

This section supersedes the older scope limitations above for Chat, Markdown,
transcript paging and artifacts. Baseline: `8b8e99e87df61b1fe8a91134f818fddc54968e46`.
The original `rustX` checkout remains clean at `6dd1ef144fdfedbd99cb0c9dd9856e1e8defbf51`;
implementation lives in the retained `rustX-issue-305` worktree.

Final validation commands (Linux, Node 24.20.0, pinned pnpm 11.13.1):

| Location | Exact command | Result |
| --- | --- | --- |
| repository | `cargo fmt --all -- --check` | Passed |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| repository | `git diff --check` | Passed |
| repository | `cargo build --bins` | Passed |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2805 passed, 1 existing ignored; bin/example targets passed |
| repository | `cargo test --test contracts --test provider --all-features` | 25 + 166 passed; 5 existing opt-in provider tests ignored |
| repository | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance` | 116 durable, 52 process, 53 subagent, 157 tools, 23 conformance passed |
| test-support/fake-provider | `uv sync --frozen` | Passed, Python 3.12.13 |
| test-support/fake-provider | `uv run --frozen pytest` | 51 passed |
| web-console, protocol/app-server, tui | `corepack install` and `pnpm install --frozen-lockfile` | Passed; `corepack enable` also run |
| web-console | `pnpm typecheck` | Passed |
| web-console | `pnpm test` | 131 tests passed across 12 files |
| web-console | `pnpm check:provenance` | 53 source records, 98 production dependency notices passed |
| web-console | `pnpm build` | Passed; existing large-chunk advisory remains |
| web-console | `pnpm exec playwright install chromium` | Passed |
| web-console | `pnpm test:e2e` | 6 passed against real App Server and mandatory provider emulator |
| protocol/app-server | `pnpm check` and `pnpm typecheck` | Passed; regeneration left all four generated artifacts unchanged |
| tui | `pnpm typecheck` and `pnpm test` | Passed; 810 tests |

The new native owner regression also passed independently with
`cargo test --lib --all-features artifact_carrier_is_native_scoped_bounded_and_cold_reopen_safe`.

The browser flow admits 34 turns through the real App Server, crosses the 64-entry
bootstrap boundary, asserts history is absent before paging, measures the stable
prepend anchor, renders rich streaming Markdown through a deterministic provider
gate, settles once, reconnects/reloads history and refuses an unsupported file
while preserving text and attachment drafts. It also exercises native missing
artifact rejection and a successful native MCP image Tool: canonical result ->
artifact read -> decoded Chromium thumbnail -> original-image dialog -> fresh
load after reconnect. The image fixture explicitly selects its MCP Tool through
the native Agent profile. No provider multimodal translation is claimed. Existing multi-Session, interaction,
transport-loss and cold-runtime flows remain green. No mock Web backend is used.

Browser plugin not available: used the repository Playwright lane per the testing
skill. Screenshots were visually inspected (`test-results/chat-history.png`,
console desktop/mobile); desktop Chat plus 390/900/1440px shell tests passed.
No blank page, framework overlay or page errors occurred. Initial new-test
synchronization/mock errors and Clippy findings were corrected and the affected
commands rerun; no failed command was waived. Initial root-directory pnpm
invocations were corrected to package-specific working directories.

Deliberate limits: durable image bytes load on explicit activation. Current
provider adapters remain text-only, so successful provider multimodal execution
is neither implemented nor claimed. Native bounded byte storage/read and browser
supported-content construction have deterministic tests. Existing declared ignored
Rust tests were not converted into passing coverage. See CHAT.md for all finite
limits and PROVENANCE.md/source-inventory.json for the pinned source closure.

## PR #317 review repair, 2026-09-15

Repairs supersede the reviewed head `cd20592938b6c64b06ad204021588c5f0d7c2f3e`.
The full validation table above was rerun after removing that gate, against unchanged
`origin/main` (`8b8e99e87df61b1fe8a91134f818fddc54968e46`). The current CI workflow
was re-read. GitHub Actions results for the final pushed head are recorded on
PR #317; local Linux results do not substitute for the macOS job.

- App Server v2 is mandatory for all clients and transports; v1 semantic
  initialization and v1-only WebSocket offers are rejected. Generated v2 files
  replace v1 files, with no duplicate API or Runtime Client version bump.
- `artifact/upload { target, data }` is storage-only. The acceptance-time
  `current_attempt` inference was removed after review identified the finite
  mailbox-watermark and idle-acceptance/next-freeze gaps. No speculative
  consumer-binding semantics remain in WEB-02.
- Native `model::adapter::validation::validate_request` continues to validate
  canonical modalities against the actual request's frozen invocation before
  provider I/O. The existing gated A/B tests exercise real Attempt model
  lifetimes; they are not claimed as proof of pending-inbound consumer binding.
- `cargo test --lib --all-features a_model_update_`: 2 passed.
  `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::`:
  71 passed, including the non-text acceptance/admission race, v2 negotiation, storage-only upload, invalid/oversized
  carrier refusal, scoped reads and cold reopen. The complete process target
  covers a v1-only offer with a valid token and successful v2 reconnection.
- `cargo test --test provider --all-features capability_boundary::`: 11 passed,
  covering image/file refusal before any network request. The real MCP image flow also proves that a canonical
  Tool image is renderable while a text-only consumer refuses the subsequent
  model invocation natively. Future multimodal inbound consumer binding is
  explicitly outside this repair.
- Web tests cover frozen Attempt preflight despite changed Session capabilities,
  safe Blob MIME, native background image/file galleries, duplicate-name
  Subagent/Workflow identities and authoritative lifecycle changes. The
  transcript race uses exclusive `before=10` -> `[8,9]` and proves
  `[8,9,10,11]` survives while the independent live cursor advances.
- Real Chromium asserts `complete && naturalWidth > 0 && naturalHeight > 0`,
  opens the original-image dialog, reconnects and loads the same native artifact
  again. The fixture's MCP PNG has no browser-invented canonical reference.
- Initial image-fixture selection failure and a stale v1 expected value in the
  process reconnect test were corrected. The complete affected browser and
  external boundary commands were rerun; no failing test was waived.

No additional Harness files were imported or adapted for the repair. The
Subagent/Workflow cards are rustX product composition over existing native DTOs.
Pinned provenance remains authoritative and unchanged.

### Final consumer-boundary correction

The review head `250c53db` incorrectly treated acceptance-time `current_attempt`
presence as consumer binding. The correction removes that validator, its
`CurrentAttempt.model` copy, `InboundAdmissionError::UnsupportedContent`, Runtime
Client mapping, and unused `validate_user_content_modalities` helper. The existing
request-level `validate_content_modalities` is unchanged.

`tests/scripted/app_server/inbound_model.rs` exercises the real Session manager,
durable store, native runtime and production OpenAI Chat adapter. For both image
and file input it arms the existing `ConversationRuntime` `admission_gate`
(`Gate`, backed by a mutex/condition variable), accepts inbound with A selected,
waits for admission to park before the coordinator lock, then selects B. The
pending content is checked directly in durable storage before releasing the gate.
A registered settlement notification then proves the admitted Attempt froze B,
consumed the pending item, and failed through the existing model-request
capability validator, with zero HTTP attempts at the provider fixture. There are
no sleeps or widened capabilities. The original active-A/future-B model lifetime
regressions remain intact.

CI run 34930949461 exposed a separate five-second presentation assertion racing
completion of the real approval Tool's provider continuation. The browser test
now awaits the emulator's explicit fourth `response_completed` observation before
asserting that answer in Chat, using the existing bounded observation API. This
changes test synchronization only; it does not alter interaction settlement.

Final focused commands:

- `cargo test --lib --all-features accepted_non_text_inbound_crosses_model_change_before_attempt_admission`: 1 test, both image/file cases passed, including durable request model B.
- `cargo test --lib --all-features pending_inbound`: 6 passed.
- `cargo test --lib --all-features safe_boundary`: 5 passed.
- `cargo test --lib --all-features before_attempt_admission`: 3 passed.
- The literal `admission_gate` name filter selected zero tests; the actual regression
  is selected by its full name above (the gate is an existing synchronization field).

The final complete command results are reflected in the validation table above.
The new regression initially called a model mutation on the managed read handle;
this compile error was corrected to call the existing native `model_set` owner.
No runtime API was added to accommodate the test.

The first final external-boundary run stopped in
`runtime_config::ext256_reload_cannot_recompose_extensions_but_the_next_launch_does`
with `local product storage is in use` on its third composition. The unchanged
test passed in isolation (`cargo test --test process --all-features
ext256_reload_cannot_recompose_extensions_but_the_next_launch_does`). The entire
external command was rerun, rather than excluding that test or proceeding with
unrun targets. This observation does not establish a runtime lock-lifetime fix;
no unrelated storage behavior was changed in this repair.

The complete external rerun passed: 23 conformance, 116 durable, 52 process,
53 subagent and 157 Tool tests. Final Web E2E: 6 passed (24.6 seconds).

CI run 34932328314 found a TUI integration assertion that allowed only process
exit/EOF prose after SIGKILL. A pending write can instead observe EPIPE first;
its typed `UncertainOutcomeError` and `TransportClosedError` were already correct.
The test now checks `process_exit`/`input_eof`, or strictly `write_error` with an
EPIPE cause, and separately awaits/asserts the actual SIGKILL child exit. It
changes no transport or runtime semantics and introduces no delay/replay.
