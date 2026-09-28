# Trajectory semantic ledger contract

Issue #421 is based independently on `a64e8ae79b2fa03da87d9995038670f179434845`
(fetched `origin/main`). The primary worktree contained only untracked
`.playwright-mcp/`; it was left untouched. Implementation worktree:
`/home/caismis/Documents/codes/rustX-issue-421`, branch
`issue-421-trajectory-semantic-ledger`. PR #416 (finite Jobs / durable Agents)
is independent and was not used as a base or dependency.

## Audited owners

| Concern | Native / Web owner inspected |
| --- | --- |
| Journal-backed Trace read cut, bounded page/detail, refresh | `src/runtime_client/trace/mod.rs`, `record.rs`, `anchor.rs`, `lifecycle.rs`, `bounds.rs` |
| Tool proposal and historical definition | `trace/detail.rs::step_tool_call`, `owning_request_snapshot`, `trace/tool.rs` |
| Immutable request-relative presentation | `trace/summary.rs`, `record.rs`; native predecessor and Context joins |
| Tool summary and lifecycle separation | `TraceToolSummary` in `trace/types.rs`, `record.rs` immutable materialization, `anchor.rs` lifecycle evidence |
| Runtime protocol | `src/runtime_client/types.rs`; baseline 50, final 51 |
| App Server protocol and schemas | `src/app_server/protocol.rs`, `schema.rs`, `examples/generate_app_server_protocol.rs`; baseline 25, final 26 |
| Generated consumers | `protocol/app-server/{generate.mjs,package.json,type-contracts.ts,fixtures.json,fixtures.ts,v26.ts,v26.schema.json}` |
| Web transport and Trace cache | `web-console/src/client/{app-server.ts,trace.ts}` |
| TUI transport and decoding | `tui/src/app-server/{client.ts,websocket-transport.ts}`, `tui/src/protocol/{app-server.ts,decoder.ts}` |
| Native grouping, selection, folding | `web-console/src/app/trajectory/{layout.ts,Trajectory.tsx}` |
| Viewport, virtual keys, prepend and focus restoration | `TrajectoryLedger.tsx`; previously embedded in `Trajectory.tsx` |
| Semantic rendering and structural controls | `TrajectoryRow.tsx`, `TrajectoryCell.tsx` |
| Summary-only bilingual search | `search.ts`, `layout.ts::matchedRecordIds` |
| Timeline native geometry and gesture lifetime | `timeline.ts`, `TrajectoryTimeline.tsx`; epoch, projection revision, pointer owner preserved |
| Inspector | `TrajectoryInspector.tsx`; existing React Aria tabs and react-resizable-panels |
| Locale | `src/locale/dictionaries/trajectory.ts`, locale controller, i18n checker |
| Browser evidence | `test/trace-fixture.ts`, `test/fixtures/trajectory.tsx`, `test/e2e/trajectory*.spec.ts`, pinned `scripts/browser-tests.sh` |
| Provenance | `PROVENANCE.md`, `source-inventory.json`, `scripts/provenance.ts` |

The integrated #419 work commits navigation at native Session creation (#422).
The integrated #420 work separates Session incremental projection from Trace
invalidation/read ownership and owns Chat scrolling (#423). Neither authority
is changed here. Trace cache bounds, exact attachment/epoch fencing and selection
fencing stay in their existing owners. No Session store, Chat scroll controller,
Journal event, Agent Loop, provider behavior, durable Trace table or SQLite
version changes are introduced.

## Final ownership and row contract

The pipeline remains Journal → native `TraceProjection` → bounded page/detail →
Web native grouping → Ledger / Timeline / Inspector. Trace is a read model;
canonical messages remain authoritative after commit. Context is frozen,
request-relative presentation evidence, not an execution event.

`TrajectoryProjection` retains exact Attempt and Step grouping. The inspectable
universe retains Turn/Step structural objects and Request targets, but those
objects no longer imply ordinary ledger rows. `TrajectoryLedgerRow` names the
measurable seat, semantic item, exact Turn, Step actions, exact Request action,
start markers and compact summary. The renderer never discovers these relations
from its neighbors or DOM position.

* Content seats are 30px; Calls and collapsed Turn summaries are 20px;
  request-only and otherwise empty native-structure seats are 10px.
* Initial System cells move visually before Turn chrome using native
  `system_prompt.state === initial`; their Request/Step identities do not change.
* Turn ordinal/fold/inspection and Step state/inspection are inline controls.
  Empty native Steps remain actionable through tiny seats. No full-width Turn,
  Group/Step or Request row and no Event/Content headings remain.
* Each Request selects its own native record. The marker prioritizes its own
  System cell, then its own Context cell; otherwise it owns a marker seat.
  Retry ordinals remain visible on that exact marker. An Assistant never supplies
  a seat by adjacency. Search may expose a Context without its System sibling;
  the marker still comes from that Context's exact Request record.
* Structural inspection reads the exact loaded native Attempt/Step summary.
  Missing native starts are explicitly unavailable; the first child is never
  substituted as native evidence. Structural selection clears detail selection
  and performs zero heavy reads.
* Collapsed Turns retain all System cells and the first main semantic row, with
  a compact native-membership Step/Tool-call count and actionable controls.
  Search bypasses folding without mutating the saved fold sets. English and
  Chinese role/Turn/Step/Context vocabulary and native IDs share one index.
* TanStack Virtual retains stable semantic seat keys and the existing end anchor.
  Row estimates equal CSS heights. Nested structural/action keys are distinct
  from marker-seat keys. Prepend can transfer an exact focused structural action
  to its new semantic seat; focus restoration runs after panel commits.

`Trajectory.tsx` owns selection, cache/read access, folds, search and Timeline
focus. `TrajectoryLedger.tsx` owns the scroll viewport, virtualization and prepend
anchor. `TrajectoryRow.tsx` renders the already-resolved native actions;
`TrajectoryCell.tsx` renders content. No generic renderer or new scroll framework
is introduced. Inspector is closed without selection, uses existing semantic tabs
and library resizing, and opens bounded detail only for semantic inspection.

## Native Tool and protocol contract

`TraceToolSummary.arguments: Option<TracePreview>` represents genuine absence of
a loadable canonical proposal. It is not a decoder compatibility field. Complete
small JSON (including `null` or `{}`) has `truncated: false`; long arguments carry
a UTF-8-safe prefix of at most 512 bytes with `truncated: true`. The serializer
stops at the byte ceiling; it does not allocate a full encoded argument string.
This preview is text, not necessarily parseable complete JSON. Full structured
arguments remain in the existing bounded detail DTO.

One call to `step_tool_call(anchor, call_id, tool_id)` supplies both the model-facing
name and preview. Attempt + native Step + ToolCallId + ToolId define correlation;
provider IDs reused in another Step and equal names cannot cross-correlate.
The counter is in the actual resolver, not a wrapper around two independent reads.
Lifecycle refresh resolves none of these immutable proposal facts. Results remain
the bounded canonical ToolMessage preview, and an absent result is not replaced
by the Tool name. The ledger shows input → result without a detail read.

Runtime Client 51 and App Server 26 are the sole final vocabulary. Rust generation,
JSON schema, TypeScript, native serialized fixture, Web and TUI move atomically.
The previous generated files are removed. No dual decoder, flags, aliases,
protocol fallback, runtime events or storage migration are added.

## Timeline, Inspector and completeness

The permanent Overview/help/reset row is removed. Input/Model/Tools occupy a
compact 50px region with a 44px label column. User is blue, native Request violet,
Tool amber, errors red; Background/Subagent/Workflow distinctions remain.
`+` zooms and `Home` resets; drag, wheel, arrow and Escape ownership is unchanged.
Localized hover/accessible help describes the keys. The native Request owns the
Model lane. Accepted Assistant is not a duplicate generation span, and Context
has no invented duration.

Projection revision still retires stale pointer coordinates on prepend, refresh
with changed inner geometry, mode changes and replacement. Trace epoch remount
retires the domain; committed focus survives same-epoch updates by native ID.
Inspector keeps exact semantic tabs, bounded/code/result typography, narrower
padding and semantic header colors. System/Context colors follow the selected
semantic cell; its native Request remains the exact detail owner. Native/structured
evidence remains available.
Ledger ellipses have a localized truncation label; Inspector retains the explicit
bounded/truncated explanations. No completeness information is suppressed.

## Pinned Harness mapping

All references below were inspected read-only with `git show` against exactly
`477b4f420553e8a52c2fbccc464d7561b239c443` in the existing reference repository.
No checkout, modification, runtime dependency or silent repin was performed.
Paths are under `packages/client/ui-trajectory/src/client/`.

| Pinned source | Pattern | rustX owner and adaptation / rejection |
| --- | --- | --- |
| `TrajectoryTable.tsx` (`TableRecord`, `collapseTurnRecords`) | Semantic record + Turn/group chrome; preserve System and first content on fold | `layout.ts::ledgerRows`, `TrajectoryRow.tsx`; native Attempt/Step/Request replaces Harness Turn/group inference |
| `TrajectoryTable.module.css` | 30px density, 122px English / 84px Chinese role track, input/result split, 20px folded summary | `Trajectory.module.css`; 50px narrow role track and compact stacked input name/preview, existing rustX tokens |
| `trajectory-virtual-rows.ts` | Measurable stable seats and 9px terminal boundaries | `TrajectoryLedger.tsx`, `TrajectoryLedgerRow`; 10px exact-owner seats. Reject attaching request-only records to the next content by list position |
| `TrajectoryTurn.tsx`, `TrajectoryTurn.module.css`, `TrajectoryGroupHeader.tsx` | Turn and Step hierarchy | Exact grouping retained; reject full standalone headers and padded card bodies |
| `TrajectoryCell.module.css` | Semantic role colors | Neutral System/Compaction, green Context, violet Assistant, amber Tool; native User convention remains blue rather than Harness green |
| `TrajectoryTimeline.module.css`, existing attributed `TrajectoryTimeline.tsx` | Three lanes / 50px plot / 44px labels | Existing rustX Timeline gesture and native timing owners; no Harness timing inference, no Context or duplicate Assistant span |
| `TrajectoryTable.tsx` Inspector and `.detailsHeader` CSS | Compact tabs/header and code typography | `TrajectoryInspector.tsx` and CSS; retain bounded lazy native details and react-resizable-panels instead of upstream manual resize state |
| `trajectory-search-index.ts` | Structural vocabulary searchable on semantic cells | `search.ts`; reject indexing heavy detail blocks/prompts and skipping native Request inspection targets. Both locales indexed independently of active locale |

Provenance preserves earlier per-file lineage and adds this pinned mapping and
hashes for changed descendants; imports and local source hashes are refreshed.

## Deterministic evidence map

| Invariant | Exact test / evidence owner |
| --- | --- |
| System before Turn, exact original Request/Step, zero heavyweight rows, retry marker seat, no Assistant inference, zero structural detail reads | `trajectory.test.tsx`: `421: ledger seats carry exact native actions and System precedes Turn chrome without heavyweight structure` |
| Exact loaded and unavailable structural evidence; late detail cannot steal native structure | Existing `407: exact native Attempt and Step records are inspectable structural evidence without any detail read`; `T1-04/06 mid-Step ...`; `T1-04 controlled late ... detail cannot hijack structural focus` rewritten for controls |
| System/first main/summary retention, fold search restoration, locale-independent membership | `421: fold retains System, first main semantic content and actionable compact summary; bilingual search restores exact folds`; existing structural search matrix |
| Tool input/result and truncation, no list/search detail | `421: Tool input and result render bounded summary facts without detail reads` |
| Same-name Tools, cross-Step reused call IDs, one resolver per materialization, zero lifecycle re-resolution | Native `tool_summary_one_exact_proposal_and_no_lifecycle_resolution`; Web `T1-07 exact scope isolates reused call IDs across Step/Attempt/Tool and page split` |
| JSON/UTF-8 bounds, genuine empty/null arguments, bounded result | Native `argument_preview_bounds_serialization_and_preserves_utf8`, `summary_pages_carry_no_heavy_request_or_tool_payload` |
| Generated wire | `src/app_server/schema.rs` round-trip/drift tests and `trace-tool-summary` fixture; TUI `app-server-dto.test.ts`; Web `421: Web consumes the generated bounded Tool vocabulary through the actual client` |
| Cache bound, epochs, paging/refresh/replacement, #420 domain separation | `trace-cache.test.ts`: `detail retention is finite and never evicts the selected record`; `a detail reply is fenced by the epoch it was requested in`; `resync fences an older read even when snapshot refresh installs the same content`; `native lifecycle patches repair a selected old record separately from rebased history`; `latest Trace reads its own current page and never reuses the attachment snapshot`; `native TraceChanged coalesces burst reads independently of durable Session updates` |
| Stale drag/pan retirement on projection change, prepend/inner geometry; same-epoch native focus | `trajectory.test.tsx`: `407: in-flight %s drag cannot cross a same-epoch projection revision` (sequence/duration/actual); `407: obsolete same-epoch span press and synthesized pointer click cannot open Inspector`; `407: an obsolete pan cannot mutate a newly zoomed same-epoch viewport`; `407: committed native focus survives a timing revision with an identical outer domain` |
| replaceTrace, recurring IDs, epoch remount | `trajectory.test.tsx`: `407: a pressed E1 span cannot select a recurring E2 record after a rebase`; `407: Timeline zoom, pan and hover belong to the Trace epoch even across an identical numeric domain`; `407: Timeline focus keeps native identity across prepend and lifecycle refresh within one Trace epoch` |
| No duplicate Assistant Model span or invented Context duration | `trajectory-timing.test.ts`: `T1-12 epoch zero, missing instant, parallel domains and canonical acceptance do not invent or duplicate spans`; `T1-12 four modes keep native time distinct from a shared idle-compression transform` |
| Prepend, threshold, exact structural focus, Inspector keyboard/resize | Browser `T1-10/11 semantic prepend...`, `T1-04/06/10 structural ... threshold prepend`, existing native selection/resize cases |
| 1440/390 × English/Chinese, expanded/folded/Inspector and search | `421: semantic ledger acceptance ...`; fixed `semanticLedgerRecords()` fixture, `?ledger` browser route |

Browser screenshots follow the existing repository convention: stable captures
from the immutable Playwright 1.63.0 Noble container, committed under
`test/e2e/trajectory.spec.ts-snapshots/ledger-421-*`. No arbitrary screenshot
thresholds, sleeps for races, new skips or raised timeouts are used.

## Validation record

Validation ran in the dedicated worktree against the fetched main base above.
The actual CI owners are `.github/workflows/ci.yml`, the three package scripts,
and `web-console/scripts/browser-tests.sh`. The browser lane uses the repository's
immutable Playwright container through Podman; no local-browser screenshot
baselines were substituted. Frozen-lockfile installs succeeded for protocol,
TUI and Web. `cargo build --bins` supplied the native integration executables.

| Command (directory) | Executed result |
| --- | --- |
| `cargo fmt --all -- --check` (root) | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` (root) | Pass |
| `cargo test --all-targets --all-features` (root) | Pass: 3,968 tests, eight existing ignored tests; no new ignores |
| `cargo test --lib --all-features runtime_client::trace::` (root) | Pass: 78 tests, including exact proposal count and lifecycle regressions |
| `pnpm generate && pnpm check && pnpm typecheck` (protocol/app-server) | Pass; one generated v26 vocabulary, no drift |
| `pnpm typecheck && pnpm test` (tui) | Pass: 852 tests |
| `pnpm typecheck` (web-console) | Pass |
| `pnpm build` (web-console) | Pass; existing Vite large-chunk advisory remains |
| `pnpm test` (web-console) | Pass: 1,097 tests in 62 files |
| `pnpm check:i18n` (web-console) | Pass |
| `pnpm check:provenance` (web-console) | Pass: 145 source records, 131 production-package notices |
| `pnpm exec vitest run test/trajectory.test.tsx test/trajectory-timing.test.ts test/trace-cache.test.ts` (web-console) | Pass: 125 tests |
| `CONTAINER_ENGINE=podman RUSTX_SCREENSHOT_UPDATE=1 bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` (web-console) | Initial iterations exposed the obsolete selectors, focus/geometry issues and missing binary described below; final screenshot-update full run passed all 33 of these tests |
| `CONTAINER_ENGINE=podman RUSTX_SCREENSHOT_UPDATE=1 pnpm test:e2e` (web-console) | 126 passed, three obsolete cross-surface Request selectors failed and were replaced |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh chat.spec.ts composer.spec.ts settings-ownership.spec.ts trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` (web-console) | 36 passed; short-page Chat fixture geometry failed, diagnosed and corrected below |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh chat.spec.ts` (web-console) | Pass after establishing a scrollable native history window |
| `CONTAINER_ENGINE=podman pnpm test:e2e` (web-console) | Pass: all 129 browser tests with screenshot comparison enabled |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh trajectory.spec.ts trajectory-timing.spec.ts trajectory-integration.spec.ts` (web-console) | Pass: 33 tests with screenshot comparison, after the final semantic-color fixes |
| `git diff --check` and `git diff --cached --check` (root) | Pass |

After the full 129-test browser pass, the final semantic-color corrections were
rebuilt, typechecked and checked by the focused unit and 33-test browser lanes.
The final screenshot-comparison run passed all 33 tests.

All four fixture combinations were visually reviewed: 1440px English, 1440px
Chinese, 390px English, 390px Chinese. Each has expanded, folded and Inspector
captures (12 new `ledger-421-*` baselines). The deterministic `semanticLedgerRecords`
fixture includes initial/updated System, two Context kinds, two Turns, multiple
Steps, success/failure Tools, retry/recovery and truncated input/result. Existing
Trajectory light/dark, resize, search, diff and prepend baselines were updated.
No unrelated screenshot changes are included.

### Failures found and corrected

* Native generation initially caught an initializer edit placed on the wrong
  struct; the Tool initializer and argument field were corrected before generation.
  Clippy caught a panic-documentation issue and collapsible conditional; both
  were removed. The first focused Trace run had 75 passes and two failures: an
  out-of-range test page limit and an obsolete “arguments are detail-only” assertion.
* Full Rust runs exposed old current/future version literals in Runtime Client,
  App Server process tests and current protocol documentation. They now test
  exactly 51/26 and reject future/obsolete versions. No decoder fallback was added.
* One intermediate full Rust run observed the unchanged
  `stalled_subscriber_resyncs_explicitly_and_never_buffers` test's snapshot-to-subscribe
  cursor race. That code was not modified. Subsequent complete runs passed that
  test, including the final successful all-target run; the observation is retained
  here rather than hidden.
* Final visual review corrected System/Context Inspector colors to follow the
  selected semantic cell, and corrected Timeline CSS precedence so failed Requests
  remain red. The four-locale/viewport acceptance cases compare failed Request
  and failed Tool colors explicitly.
* The final browser build guard caught a stale provenance hash after an added
  Request-to-Context assertion; the source inventory was regenerated and verified.
* Web intermediate runs caught obsolete row assertions, missing generated field
  types and changed projection call signatures. They were updated to assert the
  final semantic contract. One concurrent full run timed out in the unchanged
  Workspace A/B draft test; focused and subsequent full runs passed without
  changing its timeout or assertions.
* The first native browser integration run lacked `target/debug/rustx`; building
  the prescribed binaries resolved it. Browser checks then exposed duplicate
  seat/action display keys, focus restoration before panel commit, an 8px
  structural prepend offset, and a threshold test that scrolled to the tail.
  Stable distinct keys, post-commit focus and exact structural geometry fixed the
  product issues. Tests now choose an actual off-tail position and count measured
  seats rather than nested action keys.
* Chat, composer and settings browser integration still selected removed Request
  rows. They now select `data-request-owner`. The real-history Chat anchor test
  additionally assumed 32 native records must overflow the ledger. The compact
  first page fits inside its 689px viewport: the first prepend necessarily clamped
  at the available scroll extent, moving the tracked marker by 199px. The test now
  explicitly loads a scrollable window, selects a visible native Request, then
  verifies the original sub-2px tolerance through four prepends and virtualization.
  No spacer, new scroll framework, sleep, timeout increase or weakened tolerance
  was introduced.

No required validation lane was omitted for an environment limitation.
