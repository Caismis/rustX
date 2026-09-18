# Full Web conformance gate

This is a map of executable evidence, not a new semantic specification. The
browser tests use one existing Playwright configuration and `startDogfood` fixture,
real App Server WebSockets, local provider HTTP/SSE and the actual Node Product
Host. Owner races stay in deterministic Rust tests; component tests cover bounded
presentation mechanics. Passing only the browser suite is insufficient.

## Local bootstrap and explicit Remote Settings (WEB-13)

Normal startup uses the real dev launcher and requires zero endpoint/token input;
`--no-open` prints the authenticated startup URL without browser handoff. External
fixtures explicitly select Settings → Connection → Remote App Server. Browser auth
never grants Workspace filesystem authority; see [CONNECTION.md](CONNECTION.md).

| Boundary | Executable evidence |
| --- | --- |
| Separate fresh credentials, 0600 bootstrap scratch, readiness/settlement | `dev/test/launcher.test.ts`, `process.test.ts` |
| Boolean forwarding and sanitized browser handoff | `dev/test/browser.test.ts` |
| Resource-free root exchange/CSP, separated fresh proofs, exact bootstrap, missing/wrong proof rejection, bounded sessions, restart, exact roots | `test/browser-auth.test.ts` |
| Origin-only proof headers on bootstrap and Product Host; redirects/external destinations refused | `test/carrier-http.test.ts` |
| Real-browser cross-port capture and replay rejected; origin-scoped storage, no Cookie bearer, stale-proof rejection | `test/e2e/browser-origin-auth.spec.ts` |
| Mode-local recovery, no fallback, delayed bootstrap fence, exactly-once close before replacement | `test/connection-controller.test.ts` |
| Two-server Session ID collision, same-authority restoration, view/focus/dialog retirement, detached uncertainty, deletion refusal, bounded evidence and close timeout | `test/authority.test.tsx` |
| Overview by default; recovery targets Connection | `test/authority.test.tsx`, `test/e2e/dev-launcher.spec.ts`, `accessibility.spec.ts` |
| Non-Windows acceptance without process-lifetime wait; Windows launcher exit; helper-only cancellation | `dev/test/browser.test.ts` |
| Native refusal of browser token, clean URL, automatic local connect/reload, storage isolation, exact roots | `test/e2e/dev-launcher.spec.ts` |
| Explicit Remote Settings and keyboard reachability | Existing real-server browser fixtures and `accessibility.spec.ts` |

App Server protocol remains v8. Carrier bootstrap is not JSON-RPC and ordinary
App Server traffic continues directly over its native authenticated WebSocket.

## Historical prerequisite baseline (before CFG3)

Fetched base: `9980fc719a573ece0cab0114311345dc4c78b621`. The following issues are
closed and their merge commits are ancestors of this base:

| Issue | Merged PR | Merge prefix |
| --- | --- | --- |
| #305 WEB-02 | #317 | `94dfa0ae` |
| #306 WEB-03 | #318 | `e256998f` |
| #307 WEB-04 | #321 | `5537d1e2` |
| #308 WEB-05 | #322 | `d5e56132` |
| #309 WEB-06 | #323 | `5e5359cd` |
| #310 WEB-07 | #324 | `e15b55e0` |
| #311 WEB-08 | #325 | `58f17e3c` |
| #312 WEB-09 | #326 | `9980fc71` |
| #319 WEB-02A | #320 | `289bd22e` |

WEB-01 #304/#316 (`8b8e99e8`) is also included. #319 is mandatory because of the
later acceptance dependency in #313's comment; the original dependency line alone
is incomplete. No compatibility path for pre-#319 chat artifacts is retained.

## Evidence by product boundary

Paths in the Browser column are relative to `test/e2e/`; component paths to `test/`.
Native paths are relative to the repository root. The full native commands in
VALIDATION.md execute these existing suites; their evidence is not inferred from
what a screenshot happens to show.

| Contract | Browser composition | Deterministic owner / presentation evidence |
| --- | --- | --- |
| Multi-Session and residency | `console.spec.ts`: A parked while B executes, TUI controller handoff, close/open beyond the 32-attachment capacity, disconnect/reload, explicit unload/cold reopen, durable identity/cwd/history, no duplicate side effect | `tests/scripted/app_server/{protocol,residency_policy,transports}.rs`: operation/idle/attach winners, manual-clock idle eviction, connection ownership; `client.test.ts` generation fencing and exact release |
| Chat / Markdown / history | `chat.spec.ts`: 34 durable turns cross the bootstrap page, prepend anchor geometry, streamed/settled rich text, reconnect, managed image decode/lightbox | `transcript.test.ts`: 512-entry/8-MiB finite window, stale pages and concurrent live refresh; Markdown incremental/code-settlement tests prove bounded current-document retention and canonical settlement; `scroll.test.tsx` measures stable anchors |
| Harness shell | `shell.spec.ts`: ten checked-in deterministic light/dark, desktop/rail, Workspace/search/status, Settings, right-panel and mobile references; real typed client fixture | `shell.test.tsx`: columns, collapse/toggle, theme/Settings/Inspector without RPCs, pending status priority, native paging; existing native ownership and reconnect tests retained |
| Session-owned uploads | `uploads.spec.ts`: text + PNG, filesystem bytes, exact absolute model paths/XML-before-body, real native bash file IO, browser Fork, source delete before destination execution, reload, deletion of each owned root; no `artifact/read` request | `src/local_runtime/session/uploads/tests.rs`: commit/copy/publication/deletion gates, restart recovery, historical cwd roots, failed cleanup; `src/model/uploads.rs` exact escaping/order; protocol receipt admission and refusal of manufactured canonical metadata |
| Trace / Trajectory | `chat.spec.ts`, `workflow.spec.ts`: live/historical paging, model/Tool/result, exact native trace identity, running unavailable timing, Workflow/child, reload | Native Trace projection tests and `protocol::trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts`; `trace-cache.test.ts` 512-entry/4-MiB bound, lifecycle repair, contiguous rebase; `trajectory.test.tsx` bounded virtual DOM, native ordering, selection |
| Todo / Goal / Queue | `composer.spec.ts`: empty→active Todo, fixed seat order, running Goal, exact pending edit/remove, post-claim disabled draft, paused state/revision through reload | `composer-context.test.tsx`: absent/empty/current distinctions, stale Goal CAS without retry; native Goal tests, durable inbound mutation/adoption transactions and `protocol::exact_pending_mutations_are_routed_cas_bound_and_do_not_cancel_attempts` |
| Commands / lineage | `commands.spec.ts`: fuzzy identity, typed model selection, unknown refusal, Retry branch, original history, exact Fork boundary/upload copies; `uploads.spec.ts` source deletion independence | `commands.test.tsx`: stale cuts, response-loss native reread without replay, pending-inbound admission boundary, obsolete navigation epochs; native fork/cut tests retain immutable historical revisions (old does not necessarily mean invalid) |
| Host / routing | `workspaces.spec.ts`: independent Hosts/processes, exact-root routing, native Workspace settings | Host navigation authorization is separate from configuration; no Workspace trust gate |
| Provider/model | `settings.spec.ts`: independent User/Workspace add/edit/delete and complete replacement; `recovery.spec.ts`: CAS and response loss | `cfg3_catalog` credential isolation, semantic overlay and source CAS tests; Settings components |
| MCP | `integrations.spec.ts`: inert definitions, complete Workspace shadow, CAS and explicit Reload | Native MCP definition/source-demand tests; no browser connection manager |
| Skills / Agents / Workflows / Plugins | `workflow.spec.ts`: inventory and separate allowlists; `integrations.spec.ts`: complete named-Agent editor and default-off Plugins | Native frozen-child, Skill visibility and Workflow demand tests; full Skill/Python/Workflow source editors are out of scope |
| Configuration lifecycle | `console.spec.ts`: busy/frozen Attempt; `settings.spec.ts`: Save/pending/Reload/failure; `recovery.spec.ts`: lost replies without replay | One immutable generation; native candidate/publication gates, source CAS and current-file cold composition |
| Accessibility / responsive | `accessibility.spec.ts`: real keyboard-only connection→Workspace→Session→composer→command→Trajectory→Settings→MCP at 390/820/1280/1600, Escape restoration, focus, reduced motion; existing foundation, composer and inspector tests | Shared horizontal tab keyboard behavior, labelled panels, Harness modal/menu/hover-card contracts. Chromium coverage; not a WCAG certification or other-browser result |
| Browser lifetime | `chat.spec.ts`: direct object-URL registry counts through repeated decode/lightbox/reconnect/unmount; `console.spec.ts`: 34 controller releases; `recovery.spec.ts`: scoped drafts across User/Workspace and Workspace A/B | Exact bounded log counters (`diagnostics.test.ts`), cache capacities, URL caps/disposal, stale socket/attachment/page callbacks, listener unsubscribe closures and remount tests. No noisy RSS thresholds |
| Provenance / license | Existing build artifact gate and `check:provenance`; optional `--reference` audit | Per-file immutable original/local hashes and separately pinned current/historical inspections; pinned SHA, notices for 100 production packages, presentation import boundary. See PROVENANCE.md |

## CFG3 authority

Rust owns the [typed overlay matrix](../docs/configuration.md#exact-overlay-matrix).
User < Workspace authors independent Provider/Model identities, ordinary runtime
policy and Root profiles. Same-name resources shadow completely before parsing.
Only process bindings and User `app_server` process policy are process-owned.

Session durable configuration is exactly `cwd` plus optional explicit `model`.
Root and named Agents independently select Tools, Skills and default-off Plugins.
Resource definitions create no capability or materialization authority. Finite
admitted demand drives MCP/Python preparation. Current Todo/Goal/Queue state belongs
to its conversation/domain owner, outside configuration.

Web owns drafts/forms; Effective is a native read-only projection. Save commits
source bytes by CAS; Reload publishes one coherent generation. Named-Agent and MCP
editors are structured; full Skill/Python/Workflow source editors are not required.

## Synchronization and fault injection

- Provider named gates: `finish-a`, `publish-question`, `settle-chat`, `goal-round`,
  `retry-request-reached`, `workflow-child-admitted`. The real provider validates
  each request before its gate; explicit release orders admission/settlement.
- `wire-probe.ts` forwards actual WebSocket traffic. After a selected real
  `configuration/sourceWrite` result arrives, it drops only that acknowledgement and
  closes transport. Write counts and on-disk/native rereads prove no replay. It
  never supplies a fake snapshot/result. Reconnect is explicit.
- CAS conflicts append a source comment **before** the browser sends its pinned
  revision. Invalid catalog assertions compare exact source bytes before/after.
- Upload/fork/delete checks use native receipts and acknowledgements plus owned
  path bytes. Native tests separately park commit/copy/delete publication with
  channels and injected durability failures; no browser sleep claims a race win.
- Native idle tests use the existing manual clock; configuration/catalog/queue tests use
  channels, ordered lock probes and native transactional winner receipts. Client
  tests use held replies, explicit fake socket generations and manual timers.
- Polling waits for authoritative state; timeouts guard liveness only. The E2E
  fixture's only explicit timers guard child readiness/shutdown.

## Historical fixes from the original Web gate (before CFG3)

1. Browser presentation: tab roles lacked directional keyboard behavior and panel
   relationships. Shared keyboard handling and labelled, focusable panels restore
   semantics without changing native selection/execution ownership.
2. Browser MCP draft encoding: a new draft used an authored empty command even
   when switching to HTTP. `null` now represents absence, matching the existing
   field-clear contract. Rust still refuses mixed/invalid fields before mutation.
3. Dogfood/Host carrier: Node 24 strip-only mode could not load a constructor
   parameter property; explicit property assignment fixes execution. The launcher
   now emits the required Host config and relinquishes its own Host instance before
   the Web carrier starts. No authorization fallback is added.
4. Documentation/provenance: obsolete feature exclusions and the four-icon/98-
   dependency descriptions are corrected in the existing mechanisms.

No Rust semantic defect required a new implementation, protocol DTO or native
owner change in this PR. No compatibility mode, new browser semantic authority,
extra E2E runner or parallel provenance inventory is introduced.

## Integrated Harness Agent (#346)

`RESET-346-VALIDATION.md` records the current commands, results, immutable browser
authority and intentional reference changes. `agent.spec.ts` adds nine deterministic
Agent references; the real-server suite runs the same production entry path.

| Contract | Deterministic evidence |
| --- | --- |
| Cold attach, transcript/subscription, replay gap and stale generation/target/incarnation | `client.test.ts`, `transcript.test.ts`, native App Server contracts |
| Streaming/native canonical identity, reasoning/Tool order | `chat.test.tsx`, `presentation.test.tsx`, `agent.test.tsx` |
| One Tool lifecycle, cross-page results, native status and unknown fallback | native `agent_transcript_tools_resolve_native_results_across_page_boundaries`; `agent.test.tsx` |
| Approval absent-browser recovery, one response, acknowledgement is not settlement | `agent.test.tsx`, existing `client.test.ts` uncertainty contracts |
| Questionnaire pages and exact schema values | `presentation.test.tsx`, `questionnaire.test.ts`, `agent.spec.ts` |
| One Stop request; native terminal phase; disconnect inert | `agent.test.tsx`, `client.test.ts` |
| Queue/Steer native mailbox semantics, IME/uploads/typed commands | `composer-context.test.tsx`, `commands.test.tsx`, real `composer.spec.ts` |
| Native model/catalog/profile, no inferred options, uncertain mutation Send fence | `agent.test.tsx`, `commands.test.tsx` |
| Desired source CAS versus active frozen policy and explicit Reload | `agent.test.tsx`, native `agent_permission_projection_uses_native_resolution_and_source_cas`, CFG3 tests |
| Light/dark/narrow and shared shell | pinned `agent.spec.ts`, `shell.spec.ts`, `foundation.spec.ts`; keyboard real-server acceptance |


## WEB-11 composer interaction contract

| Contract | Deterministic evidence |
| --- | --- |
| One primary seat, Queue default, accelerated Steer, commands, upload/ack/cancel gates | `submission-policy.test.ts` (21-case pure matrix), `composer.test.tsx`, `composer-context.test.tsx` exact App Server request assertions |
| 36px floor, measured growth/shrink, configured cap, one text scrollport, restored draft mount/Session switch | `agent.spec.ts` DOM/computed-style geometry assertions at 1440/390px |
| Todo → Goal → Queue → Composer; draft/node identity survives independent docks | `composer-context.test.tsx`; `agent.spec.ts` light/dark desktop/mobile context references |
| Upload picker/remove, plain Enter/button/accelerated native method, compact desktop toolbar | `agent.spec.ts`; existing `artifacts.test.tsx` and real `uploads.spec.ts` retain failure, receipts and no-replay coverage |
| Keyboard focus on the rounded composer card | `accessibility.spec.ts` keyboard traversal at 390/820/1280/1600px checks the rendered focus stroke and unchanged focus restoration |
| Native queue CAS, cancellation settlement and reconnect uncertainty | Existing `agent.test.tsx`, `composer-context.test.tsx`, real `composer.spec.ts`, `console.spec.ts`, `commands.spec.ts`, `recovery.spec.ts` |

Visual authority remains the digest-pinned Playwright 1.63.0 container. Use
`pnpm --dir web-console test:e2e:update` only for reviewed baseline changes, then
`pnpm --dir web-console test:e2e` with zero pixel tolerance. `CONTAINER_ENGINE=podman`
is the supported local engine selection when Docker is absent. Current composer
references cover idle empty/draft, running Stop/Queue, attachments and the context
stack in both themes at 1440px and 390px. Shell/Settings baselines also include the
changed composer behind their overlays; no shell or Settings redesign is implied.

## WEB-12 product / Inspector boundary

`session-surface.test.tsx` proves Sidebar-only selection, background work across
focus changes, finite view capacity and explicit close/release, native-only naming
and first-commit catalog invalidation (including file-only input), manual name
precedence, A/B and unscoped uncertainty, scoped interaction evidence, local-only
diagnostic acknowledgement and exact revision-confirmed deletion without raw DTOs.
The former Session-tab selectors are removed; Chat/Trajectory keep tab semantics.

Exact metadata regressions use `session/summary`, never fuzzy `session/list(query=id)`.
Native catalog tests prove a 32-row search collision cannot obscure exact identity,
and exact/list projections agree. App Server tests prove exact reads and not-found
errors do not compose runtimes or change residency. Web tests cover RPC/socket read
failure followed by reconnect retry off-page, in-flight coalescing, successful
file-only/no-preview completion, manual rename, and older-list/newer-summary fencing.
`view.summary` remains a replaceable observation, not catalog membership authority.

Product-surface baseline validation: 421 deterministic tests in 29 files; all 39 browser acceptance
tests; 23 intentional snapshot/geometry update cases followed by normal zero-tolerance
E2E. Keyboard acceptance now explicitly reaches Sidebar row actions/Close view and
Inspector at all four widths. Reviewed rendered light/dark/narrow, native preview,
manual name, empty Session, background work, scoped uncertainty, Inspector and deletion
captures. Typecheck, production build, 104-source provenance/100-package notices,
upstream reference audit and the 30-test development-launcher lane pass. The subsequent
exact-summary repair adds one durable native protocol read and runs the complete
Rust/protocol/TUI/Web lanes; it requires no visual snapshot changes, screenshot
tolerance changes or semantic sleeps. Per-head validation is recorded in PR #361.
The unlisted-restoration regression also fills all 32 view slots without matching
catalog rows, then recovers through explicit Sidebar Close all views. Its companion
proves bulk close only releases observed controllers and leaves running work intact.

`session-product.test.tsx` covers deterministic status priority, idle silence,
accepted versus unaccepted inbound, cancellation request versus settlement,
attachment recovery, durability and uncertain outcomes. It proves exact native
identity/phase/revision evidence remains in Inspector, Inspector navigation and
local log controls send no native operations, and lost cancellation is not replayed.
Existing client/runtime tests remain authoritative for attachment and residency.

Browser tests no longer synchronize through `.attempt-status`, permanent lifecycle
buttons or attachment suffixes. They use canonical resulting messages, product
readiness, actual wire responses, native diagnostics, or explicit Inspector reads.
Advanced unload is tested through its disclosed dialog; controller handoff uses
close/open and exact native detach acknowledgements. Shell references now include
idle, queued, stopping, reconnect and uncertainty, alongside desktop light/dark,
Inspector and narrow layouts. Existing four-width keyboard acceptance remains.

Each Agent reference mode runs as its own test/page/context rather than sharing
one multi-navigation capture. This keeps reference setup independent, including
the browser's paint caches. Screenshot tolerances remain zero; no composer style
workaround, semantic sleep or retry is introduced.
