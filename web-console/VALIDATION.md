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

## Intentional scope decisions

The pinned upstream commit is unchanged. No Rust runtime or public protocol/schema
change was needed. The only shared protocol change is the compiler-input
normalization and generated TypeScript correction, with compile-time regressions.
All Cordis/Host boot services were excluded because plain prop-driven extracted
presentation has the smaller coherent dependency closure. Specialized Subagent,
Workflow and Goal views were replaced with native read-only JSON disclosures.
Recovery is explicit reconnect; no automatic reconnect loop or mutation retry is
provided. Rich Markdown/image/IDE/provider configuration features remain excluded.
