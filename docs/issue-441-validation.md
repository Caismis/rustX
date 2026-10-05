# Issue #441 implementation and validation

The starting dependency-complete `origin/main` was
`3aeefdc771f24d1e5acfc53da16c8bcbcbba35cd`, containing #430, #431 and
#440 / PR #449. The issue worktree is
`/home/caismis/Documents/codes/rustX-issue-441`, branch
`issue-441-multi-preview-workspace`.

The primary checkout was `/home/caismis/Documents/codes/rustX`, on `main` at
that same SHA. Its initial status contained only the untracked
`.playwright-mcp/` directory. Implementation and validation use the issue
worktree; the primary checkout is not an implementation surface.
The pre-delivery fetch still resolved `origin/main` to that same SHA; main did
not move, and the final merge base is the starting SHA above. A later read-only
status check again found the primary on the same `main`/HEAD with only its
original `.playwright-mcp/` directory, and both Harness checkouts remained clean
at their recorded commits.

The normative ownership, identity, budgets and presentation contract is in
[Preview workspace](preview-workspace.md), alongside the authoritative
[document-viewer](document-previews.md) and [file-delivery](file-delivery.md)
contracts.

## Pinned reference audit

The read-only reference worktree is
`/home/caismis/Documents/codes/deepseek-harness-441-reference`, at
`deepseek-ai/deepseek-harness@639ed015397290b3745d163aafe02ffee4aa3f84`.
The user's existing reference checkout was clean at
`477b4f420553e8a52c2fbccc464d7561b239c443`; it was not switched to the pin.

The following exact reference files were inspected. Patterns are architectural
references, not imported source. No new source-provenance entry is justified by
inspection alone.

| Inspected file (under `packages/client/`) | Relevant pattern or deliberate exclusion |
| --- | --- |
| `ui-sidebar-right/README.md` | One Session surface; duplicate reveal; requested and automatic presentation; two horizontal panes; excluded default guide, persistent layout and provider registry. |
| `ui-sidebar-right/src/client/tab-domain.ts` | Separate logical tab and opening occurrence, occurrence abort signal and retirement before reopened work; excluded resource pinning across hidden tabs. |
| `ui-sidebar-right/src/client/session-view.ts` | Explicit view retirement and idempotent holds; excluded retaining initialized hidden bodies. |
| `ui-sidebar-right/src/client/session-views.ts` | Session foreground selection distinct from retained logical state; rustX adds a finite retained-Session policy. |
| `ui-sidebar-right/src/client/shell/SidebarRight.tsx` | Same keyed content tree for normal/fullscreen; automatic narrow presentation distinct from requested mode; at most two horizontal panes. |
| `ui-sidebar-right/src/client/shell/close-focus.ts` | Commit before focus restoration; `preventScroll`; respect focus deliberately taken elsewhere. |
| `ui-dockkit/README.md` | Fixed strip controls, scrolling chip region and measured split admission; explicit accessibility gap is not adopted. |
| `ui-dockkit/src/engine/geometry.ts` | Pure room and divider arithmetic; exclude DnD drop zones and floating geometry. |
| `ui-dockkit/src/components/measure.ts` | Measure actual pane/strip/control geometry; do not infer room from viewport alone. |
| `ui-dockkit/src/components/TabPanel.tsx` | Roving tab focus, unmodified keyboard events and bounded horizontal strip overflow. |
| `ui-dockkit/src/components/TabLayout.tsx` | Stable keyed hosts; visibility-mounted bodies; exclude `keepMounted` and floating hosts. |
| `ui-dockkit/src/components/PaneTree.tsx` | Transient divider preview with one final intent; exclude recursive splits and pointer-only divider semantics. |
| `ui-dockkit/src/components/DockSurface.tsx` | `ResizeObserver`, pointer gesture capture and committed divider ratio. |
| `ui-dockkit/src/components/pointer.ts` | Exact pointer ownership and cancellation cleanup. |
| `ui-layout/README.md` | Separate panel geometry from Session state; no automatic reopening after collapse. Its lack of conversation scroll anchoring is not adopted. |
| `ui-layout/src/client/columns.ts` | Bounded measured right-column geometry; rustX retains its own existing shell/ChatViewport owners. |

Excluded architecture: Cordis, tab-provider/plugin registries, generic docking
runtime, floating panels, recursive/vertical split trees, guide/default document
backfill, undo/redo layout history, layout localStorage and retained hidden
document runtimes. rustX retains #430's sole `ChatViewport` scroll writer and
#431/#440's native source/advanced-viewer authority.

## Existing validation owners

The current scripts and `.github/workflows/ci.yml` were inspected at the starting
SHA. `web-console/package.json` uses Node >=24, pnpm 11.13.1, Vitest 5.0.0 and
Playwright 1.63.0. The existing browser runner is
`web-console/scripts/browser-tests.sh`; `test:e2e` builds once and invokes it.
It builds native binaries plus `document_artifact_fixture`, and supplies the
immutable Chromium/font environment from
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
No second runner, host-font mount or screenshot tolerance change is needed.

Existing focused owners include `document-view-lifetime.test.tsx`,
`pdf-view-lifetime.test.tsx`, `pdf-document.test.ts`, `session-files.test.tsx`,
`document-integration.test.ts`, `document-security.test.ts`,
`document-operation-lifetime.test.ts` and `document-host-settlement.test.ts`.
`scroll.test.tsx` controls ResizeObserver and animation frames and already
asserts detached-anchor and user-scroll precedence. Browser coverage builds on
`documents.spec.ts`, `artifact-document.spec.ts`, `file-delivery.spec.ts` and
`reading.spec.ts`, which use the real Product Host/App Server and provider gates.

CI prerequisites include `uv sync --frozen` in `test-support/fake-provider`,
Bubblewrap namespace admission, and
`node web-console/scripts/check-office-sandbox.ts`. The Office probe runs the
actual cgroup/filesystem admission path rather than substituting a converter.
The Browser plugin/browser skill was not available. The requested existing
pinned Playwright runner owns browser acceptance.

## Results

The results below come from commands executed in the issue worktree. An inspected
script is not counted as a passing validation result.

Initial prerequisite checks passed in the issue worktree:

- `uv sync --frozen` in `test-support/fake-provider` (Python 3.12.13).
- `bwrap --unshare-all --unshare-user --disable-userns --die-with-parent --new-session --cap-drop ALL --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 -- /usr/bin/true`.
- `node web-console/scripts/check-office-sandbox.ts` reported
  `Office cgroup and filesystem admission passed`.
- Podman already held the exact digest-pinned Playwright image above.
- `uv run --frozen pytest` in `test-support/fake-provider`: 52 passed after
  adding `web_preview_workspace`.
- `pnpm --dir web-console typecheck`: passed after updating the two existing
  preview fixtures to explicit coordinator/lease and Download intents.
- `pnpm --dir web-console check:provenance`: 148 source records and 136
  production-package notices passed. Four previously inventoried adapted files
  received new local hashes and a #441 treatment note; no upstream pin or new
  reference-source reuse was added. The later explicit AttachmentCard Download
  change refreshed a fifth existing local hash, also without changing its upstream
  source or pin.

## Test-to-invariant mapping

All paths in this table are relative to `web-console/test/`. Tests use controlled
promises, native provider gates, exact abort signals, controlled geometry/frames
or real browser observations; no elapsed sleep establishes a race.

| Invariant | Discovered regression evidence |
| --- | --- |
| Artifact/Session-file identities remain closed and exact across names, declarations, indexes and reference fields | `preview-workspace.test.tsx`: `exact closed sources distinguish names, namespaces, declarations, indices and every reference field`; `session-files.test.tsx`: `typed source equality preserves declarations, delivery indexes, namespaces and managed Artifact distinction`. |
| Duplicate open reveals the existing other-pane occurrence; explicit close/reopen allocates a new lifetime | `preview-workspace.test.tsx`: `duplicate opens reveal the existing other-pane occurrence; close and reopen allocate a new lifetime`. |
| Old read success/error cannot enter a reopened exact source | `session-files.test.tsx`: gated `close then reopen source gets a new lease; late old %s cannot publish`, both byte/error terminals. |
| Eight tabs and four retained Sessions have explicit limits without unrelated loss | `preview-workspace.test.tsx`: `eight tabs reject overflow without loss and retain metadata without hidden leases`; `four retained Sessions reject a fifth without evicting unrelated logical state`. |
| Right-neighbor-then-left close, no unselected-selection disturbance, empty pane removal and no fabricated document | `preview-workspace.test.tsx`: `selected close prefers right then left; unselected close preserves selection; final pane has no default document`. |
| View state survives safe body unmount without retaining runtime resources | `preview-view-state.test.tsx` checks text/Markdown/image scroll, HTML source mode/scroll and opaque sandbox, workbook sheet/window/scroll with changed-source clamping, PDF page/zoom/scroll with a fresh owner, and lease retirement during a gated render. |
| Exact active-pane open/move, two-pane maximum | `preview-workspace.test.tsx`: `split, active-pane open, explicit move and third-pane rejection share one membership contract`. |
| Width admission is explained to keyboard users and uses the same ratio clamp as resize | `preview-workspace.test.tsx`: `insufficient measured width explains split, and keyboard divider uses bounded ratio`, including consistent ARIA endpoint/current rounding at 620px and the real 300px working minimum. |
| Keyboard focus reveals an overflowing tab without selecting it or scrolling the Conversation | `preview-workspace.test.tsx`: `keyboard focus reveals a clipped tab through only its bounded strip without selecting it`; the browser opens all eight tabs and checks actual 390px strip overflow, Home/End item bounds and unchanged selected DOCX occurrence/rendered body. |
| Pointer capture, one frame-coalesced gesture preview, one final commit and cancellation rollback | `preview-workspace.test.tsx`: `pointer divider captures, coalesces visual frames, commits once and cancellation restores`. |
| Geometry changes, Inspector and collapse retire divider capture and pending frames before old input can commit | `preview-workspace.test.tsx`: `divider geometry replacement retires pointer capture and pending paint; keyboard cannot alter an active gesture`; parameterized `hiding a captured divider via %s retires its frame and capture before hidden input can commit`. |
| Two live PDF workers are admitted; a third is refused; release occurs exactly once | `pdf-document.test.ts`: `admits two visible PDF workers, rejects a third and releases each once on disposal or worker failure`; `e2e/preview-workspace.spec.ts` observes actual worker count through hidden tabs, split, collapse and Session changes. |
| Existing PDF page/canvas/scratch/text/watchdog/security limits remain enforced | `pdf-document.test.ts`, `pdf-view-lifetime.test.tsx`, `document-security.test.ts` retain page/text gates, exact canvas counters, failed-worker watchdog and policy assertions. |
| At most one Office operation physically active, at most visible demand waiting; waiting and active cancellation differ | `session-files.test.tsx`: `two visible Office intents serialize; cancel waiting demand immediately, active demand holds until settlement`; `a new Session coordinator waits for old aborted Office physical settlement`. |
| Private operation cancellation is exact and unknown physical settlement cannot release admission | `document-http-lifetime.test.ts`: `cancellation requires the exact operation token and authority scope; final response releases the sole carrier`; `lost terminal transport is typed as unknown settlement instead of reusable admission`; `session-files.test.tsx`: `unknown %s settlement fails subsequent admission closed without retry`. |
| Private native cancellation acknowledgement waits for the admitted descriptor read and permit to retire | Native `boundary_suites::app_server_file_read::committed_present_reads_exact_native_scope_through_current_authorized_attachment` gates before descriptor bytes, observes retirement pending and no close acknowledgement with the permit still held, then releases the gate and requires clean close, two available permits and a successful subsequent read. `product-host-file-read.test.ts` covers two occupied slots after abort until acknowledgement, abnormal/unknown transport failing closed, pre-dispatch non-admission and malformed responses that cannot restore publication. |
| Two original reads plus a waiting transient Download remain within physical native read admission | `document-http-lifetime.test.ts`: `two original reads keep their physical slots after abort; queued Download starts only when one settles`; `session-files.test.tsx`: `Download waits behind two active reads without increasing native transfer admission` and `active derivation reserves a shared read slot so pane and Download transfers cannot race Host reauthorization`. |
| Download leaves all navigation/presentation state unchanged, reauthorizes original bytes/name, and releases its third transient URL once | `preview-workspace.test.tsx`: `Download does not create, select, reveal or change pane/presentation state`; `Download inside an inactive pane leaves pane activation and tab selection unchanged for pointer and keyboard focus`; `session-files.test.tsx`: `two pane URLs plus one transient original Download are bounded and released exactly once`; browser cases compare original bytes/name and assert both inactive-pane footer Download and Conversation-card Download preserve pane/tab state. |
| Inline managed Artifact Download uses the same explicit current-authority intent before and after thumbnail loading | `preview-intents.test.tsx` checks image/non-image managed cards; the separate local user-attachment case retains its draft-owned local Download URL. |
| Host/attachment/runtime authority replacement and retirement reject gated read/derive success and error publication | `session-files.test.tsx`: parameterized gated obsolete and derived Host/attachment/authority/close/abort responses; `document-view-lifetime.test.tsx`: late result/error/loading completion; `e2e/artifact-document.spec.ts`: real immutable Artifact/private carrier and late attachment publication. |
| Exact scope replacement synchronously retires the old occurrence and rejects its later view-state writes | `preview-scope.test.ts` compares exact runtime/authority/Session/target coordinates, observes aborted leases and zero old workspace ownership after Host/runtime/native-authority/Conversation replacement, then requires a fresh occurrence with empty view state. This is a synchronous owner-retirement test; gated completion evidence belongs to the resource and native boundary suites. |
| Session switch retains only bounded metadata and returning reacquires authority/resources | `preview-workspace.test.tsx`: `Session A to B retains only metadata and restores exact tab state; scope replacement retires immediately`; browser Session A→B→A checks same occurrences, fresh reads and zero background workers/URLs. |
| Fullscreen changes geometry without new occurrence/read/worker; narrow keeps logical split; explicit collapse survives widening | `preview-workspace.test.tsx`: `fullscreen preserves leases; narrow presents active pane and wide restores split; collapse stays closed`; browser observes read and worker counters through presentation changes. |
| Inspector coexistence retains logical tabs but releases hidden bodies | `preview-workspace.test.tsx`: `Inspector retains logical state and reacquires only selected visible bodies`; browser Inspector↔preview verifies zero hidden URLs/workers and same tabs. |
| Manual tab keyboard activation, close focus, composition/repeat/modifier Escape priority | `preview-workspace.test.tsx`: `real tabs keyboard path is manual activation; focus returns deterministically on close`; `Escape respects composition, repeat and modifiers; fullscreen restores before collapse`; `tab and divider navigation leave IME and modified keys alone, and held Delete closes no extra occurrences`; browser verifies real focus/pointer/keyboard paths and zero `turn/cancel`. |
| Fullscreen restore that removes the focused divider transfers focus to the active selected tab | `preview-workspace.test.tsx`: `restoring fullscreen into one visible pane transfers divider focus to the active occurrence`, including IME keyCode 229; the workspace browser case exercises 1200px normal→fullscreen→Escape and exact active-tab focus. |
| Conversation remains owned by ChatViewport, detached streaming does not follow, pending correction loses to user scroll | `preview-scroll.test.tsx` renders the real workspace owner/RightPanel and controls ResizeObserver/RAF through open, collapse, split, resize, fullscreen, restore, narrow/wide and Inspector transitions in follow/detached/historical modes, plus a user scroll before the pending correction. Existing `scroll.test.tsx` and `e2e/reading.spec.ts` retain native reading tests; workspace browser coverage checks detached geometry and gated streaming. |
| EN/ZH, light/dark, reduced motion and no narrow page overflow | `e2e/preview-workspace.spec.ts` switches actual settings and 1920/390px viewports, asserts overflow and captures both heavy-viewer presentations; existing file/document E2E retains hostile HTML and language coverage. |

The first focused browser run passed the four existing document/file-delivery
cases and found an actual nested text scrollport defect in the new workspace
case: the global `pre` height cap absorbed scrolling outside the retained body
owner. The viewer CSS now removes that nested cap/overflow; the test asserts the
exact initial and restored 420px position. No assertion or screenshot tolerance
was relaxed.

The focused command
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e preview-workspace.spec.ts documents.spec.ts file-delivery.spec.ts artifact-document.spec.ts`
subsequently passed all five discovered tests in 50.1 seconds. It builds the
production app and uses the repository's pinned browser server. The preexisting
document acceptance test now establishes its exact reading baseline after the
measured responsive frame columns finish changing, instead of sampling an
intermediate 390px sidebar transition. Its scroll-position assertion remains
exact. No screenshot reference or tolerance was modified.

The new real-browser case observes a maximum of two live Workers and three
original-source URLs, zero duplicate URL revocations, and zero workers/URLs after
final close. It compares original Download bytes/name while both PDFs are open,
checks numeric occurrences before/after duplicate open and Session switch, and
requires a larger occurrence after explicit close/reopen. Pointer capture and
keyboard ratio assertions use the actual separator. Provider request counts and
captured `turn/start`/`turn/cancel` calls prove preview navigation starts no model
work and cancels no native turn.

Browser screenshots are supplemental evidence:
`workspace-two-heavy-light.png` shows DOCX and PPTX simultaneously rendered with
separate selected tabs and the visible divider; `workspace-two-heavy-dark-zh.png`
shows the same workspace after theme/language changes. The final screenshot
asserts both PDF text layers are populated before capture. The acceptance flow
also exercises reduced motion and 390px narrow layout with one visible pane and
no page-level overflow. Browser page-error collection remains empty.

Final applicable repository checks:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed. |
| `cargo build --bins --all-features --locked` | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 TMPDIR=/var/tmp/r441.PJLSx9 cargo test --all-targets --all-features --locked` | Passed: 4,106 tests, zero failures, eight preexisting opt-in ignores, zero filtered tests across 20 harnesses. |
| `pnpm --dir protocol/app-server check` | Passed; generated no diff. |
| `pnpm --dir protocol/app-server typecheck` | Passed. |
| `pnpm --dir tui typecheck` | Passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, zero skipped. |
| `pnpm --dir dev typecheck` | Passed. |
| `pnpm --dir dev test` | 38 passed. |
| `pnpm --dir web-console typecheck` | Passed on frozen final source. |
| `pnpm --dir web-console test` | 100 files, 1,663 tests passed on frozen final source. |
| `pnpm --dir web-console build` | Passed; existing >500 kB bundle advisory only. |
| `pnpm --dir web-console check:i18n` | Passed. |
| `pnpm --dir web-console check:provenance` | Passed: 148 source records, 136 production-package notices. |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Passed: 169 tests, zero failures, 9.9 minutes in the pinned browser. |
| `git diff --check` | Passed. |
| `pnpm --dir web-console exec vitest run test/artifacts.test.tsx test/session-files.test.tsx test/document-http-lifetime.test.ts test/pdf-document.test.ts test/document-host-settlement.test.ts test/document-view-lifetime.test.tsx test/pdf-view-lifetime.test.tsx test/document-operation-lifetime.test.ts test/product-host-file-read.test.ts` | 9 files, 98 tests passed. |
| `pnpm --dir web-console exec vitest run test/document-integration.test.ts test/document-security.test.ts` | 2 files, 7 tests passed. |
| `cargo test --lib --all-features --locked boundary_suites::app_server_file_read::committed_present_reads_exact_native_scope_through_current_authorized_attachment -- --exact --nocapture` | Passed: 1 gated native boundary test, 3,472 unrelated tests filtered. |

The current Linux CI native partition was also run, preserving its default-feature
binary builds and exact selectors, with `TMPDIR=/var/tmp/r441.PJLSx9`:

```sh
cargo build --bins --locked
cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features --locked
cargo run --example check_test_lanes --locked -- --job rust-contracts
cargo build --bins --locked
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features --locked -- boundary_suites::
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output
cargo run --example check_test_lanes --locked -- --job rust-boundaries
```

Both Linux lane-coverage checks passed. These lanes ran all 4,106 runnable native
tests successfully; their filters partition discovery rather than omit a new
boundary. The eight ignored tests are preexisting opt-in cases. The final complete
aggregate command above independently passed the same runnable tests without
filters. An earlier aggregate run had failed the unchanged
`finite_workflow_recovery_tracks_physical_obligation_until_exact_owner_proof`
fixture's pending-obligation assertion (zero versus one); investigation found its
existing fork/CLOEXEC fixture race, and that test passed in both final validation
runs. Its fixture and assertions were not modified for this issue.

The first full browser run was deliberately interrupted with owned process-group
SIGINT after 27 passing cases so that the final IME/modifier guard change would
be included in a fresh production build. It recorded no test failure before
interruption; the runner's browser-container cleanup completed. The complete
runner was then restarted using the same command and pinned image.
The first restart refused port 5173 before running tests because Playwright's
separate, issue-worktree Vite process groups survived the interrupted parent.
Those two identified owned groups were terminated and both ports were verified
free before the complete command was run again; server reuse remains disabled.

That diagnostic run continued while the final native/Host retirement repair was
being completed. It reached case 94 with 93 passing cases and one Settings
failure: its trace records the fixture Vite server connection being lost and
reconnected while the Settings dialog was open, leaving the screenshot locator
waiting for a dialog removed by the reload. The runner later stopped making
progress before the next shell-delete case completed for over three minutes,
beyond its 120-second test deadline, with the worker waiting in `ep_poll` and no
terminal case artifact.
The owned diagnostic process group was interrupted, its browser container and
remaining owned Vite/worker groups retired, and its log and Settings trace were
preserved. This interrupted diagnostic is not final acceptance evidence. No
application deadline, screenshot baseline, tolerance or assertion was changed.

The frozen-source Settings and shell-delete cases both passed in the subsequent
focused run. A newly added narrow logical-occurrence assertion initially used a
role locator that excludes hidden tabs; it now enumerates the retained occurrence
markers, while visible-pane and keyboard assertions retain role locators. The
complete strengthened workspace case then passed in 22.9 seconds. A subsequent
full run was deliberately stopped after eight passing cases to include the final
bounded keyboard tab-reveal repair; it had no test failure before interruption.
The eight-tab browser case then measured the last tab ending at 364.328125px in
a 364px strip whose browser scroll range stopped at 209px. A 1px content end
gutter now makes the complete tab/close control revealable despite integer
scroll-extent rounding; the exact browser bounds assertion was retained. The
final focused command
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e preview-workspace.spec.ts`
passed in 18.6 seconds (15.7 seconds for the test), including eight logical tabs,
Home/End overflow reveal, inactive-pane original Download, focused-divider
retirement, two heavy viewers and exact final-opener focus.

During implementation, an independent retirement review demonstrated that the old native private
file-read socket could acknowledge cancellation while its detached blocking
descriptor read still held a native permit. Browser-only cancellation could not
prove settlement. The required repair joins that already admitted read before
the private socket's clean-close acknowledgement; the Node Host only releases
its shared read slot after that acknowledgement, and unknown settlement fails
closed. This changes the existing private cancellation lifetime in
`src/app_server/product_host.rs` and the Product Host adapter, without a new
public Method, source kind, payload schema, authority or tab registry. The native
gate and Node transport tests above cover both affected sides. This repair
preceded the final frozen-source validation.

The final full pinned command passed all 169 discovered tests in 9.9 minutes;
the complete workspace case passed in 16.2 seconds. It included every existing
Settings/shell/screenshot case, the native distant-reading fixture, document
security, original delivery, uploads and private Artifact derivation. No test
was retried by the runner and no screenshot reference or tolerance changed.
The final dark/ZH screenshot was inspected at original resolution and by canvas
crops: both DOCX and PPTX pages contain their rendered text. `PdfDocumentOwner`
awaits the page-render promise before constructing either populated text layer.

Final evidence is retained in `/var/tmp/rustx-441-browser-final.log`,
`/var/tmp/rustx-441-native-final-aggregate.log`,
`/var/tmp/rustx-441-native-ci-run.log` and
`/var/tmp/rustx-441-browser-evidence/`; the browser also writes its screenshots
and attachments into `web-console/test-results/`. All required local Linux
prerequisites were available. macOS and browsers outside the repository's pinned
Chromium runner were not run locally. The committed three-dot whitespace check
(`git diff origin/main...HEAD --check`) and final HEAD are recorded at PR delivery.


## PR #450 contract repair (2026-10-05)

Starting PR HEAD: `776323a42fb02c61d0e6a674860e152fd3bf74ec`.
Fetched main and merge base: `3aeefdc771f24d1e5acfc53da16c8bcbcbba35cd`.
The existing `issue-441-multi-preview-workspace` branch in
`/home/caismis/Documents/codes/rustX-issue-441` was clean before editing.
Both reviewed defects remained present at that HEAD. The primary checkout was
not edited; its pre-existing untracked `.playwright-mcp/` directory was retained.

The repair adds current-presentation measurement epochs, conservatively admitting
only the active pane after hidden presentation or Session changes. Old observer
callbacks cannot authorize the new presentation. Fullscreen commits use the same
keyed tree and a synchronous layout witness; a missing witness conservatively
retires the second lease before the transaction returns. Explicit Close persists
`expanded: false` even in Inspector, independently of temporary Inspector toggles.
See [preview-workspace.md](preview-workspace.md) for the complete contract.

Controlled geometry tests cover collapse and Inspector wide-to-hidden-narrow
interleavings, including delivery from the obsolete observer, exact acquisition
counts before fresh measurement, narrow/wide delivery, Session epochs, zero
measurement, and fullscreen without a layout witness. Explicit Inspector Close
is exercised across native refresh, Session round-trip, widening and subsequent
Inspector toggles; explicit Reopen restores tabs and metadata.

The real browser test adds both hidden-narrow directions, counts original reads
and peak PDF workers, and proves that a selected hidden Office pane sends no
new derivation demand. It also uses the actual Close Inspector button, switches
Sessions, resizes while closed, explicitly reopens and verifies occurrence IDs,
text scroll and wrap. Existing hidden-widen coverage remains. The focused pinned
browser run passed (20.8 seconds for the test; 23.8 seconds total).

View state still commits on scroll/control interactions, independently of lease
retirement and React cleanup. The seven focused test files passed all 73 tests:

```sh
pnpm --dir web-console exec vitest run \
  test/preview-workspace.test.tsx test/preview-scroll.test.tsx \
  test/preview-view-state.test.tsx test/session-files.test.tsx \
  test/document-view-lifetime.test.tsx test/pdf-view-lifetime.test.tsx \
  test/document-http-lifetime.test.ts
CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e preview-workspace.spec.ts
```

The repair does not change `ChatViewport` or the five reviewed native/Product
Host settlement files. Exact operation cancellation, admitted native read join,
terminal response/clean settlement witness and unknown-settlement fail-closed
behavior are preserved byte-for-byte relative to the starting PR HEAD.

All of these commands passed:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --bins --all-features --locked
pnpm --dir protocol/app-server check
pnpm --dir protocol/app-server typecheck
pnpm --dir tui typecheck
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test
pnpm --dir dev typecheck
pnpm --dir dev test
pnpm --dir web-console typecheck
pnpm --dir web-console test
pnpm --dir web-console build
pnpm --dir web-console check:i18n
pnpm --dir web-console check:provenance
bwrap --unshare-all --unshare-user --disable-userns --die-with-parent --new-session \
  --cap-drop ALL --ro-bind /usr /usr --symlink usr/lib /lib \
  --symlink usr/lib64 /lib64 -- /usr/bin/true
node web-console/scripts/check-office-sandbox.ts
git diff --check
git diff origin/main...HEAD --check
```

Web unit validation passed 100 files / 1667 tests; TUI passed 895 tests and dev
passed 38. Rust uses the repository-pinned 1.98.1 toolchain. The initial unqualified
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked`
failed 32 configuration-discovery tests: live inspection found an ambient
`/tmp/.git` and `/tmp/rustx.toml`. The clean-environment invocation passed 4106 tests across 20 binaries (8 existing
ignored tests):

```sh
TMPDIR=/var/tmp/r450 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked
```

Two browser authoring failures were retained in the local logs: an Inspector
overlay intercepted navigation at phone width, and the new close-button locator
used the wrong case. The Inspector path now uses a real 1200px viewport whose
normal preview column cannot fit two panes while navigation remains reachable;
the collapse path still uses 390px. The close locator matches the actual
`Close Inspector` accessible name. No deadline, assertion, screenshot tolerance,
reference or runner retry policy was relaxed.

Full pinned browser validation:

```sh
CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e
```

Result: **167 passed, 2 failed (10.1 minutes)**. The complete preview-workspace
case passed in 20.0 seconds. The retained failures were:

- `agent.spec.ts:96`, light/390 Composer: 15 changed pixels in
  `composer-running-draft-light-390-linux.png`, all within x=24..33, y=117..120
  on the input's upper-left edge; dimensions remain 334x244, maximum channel
  delta 13. No noise policy exists for this reference; strict comparison failed.
- `settings-presentation.spec.ts:540`: the enabled `Remove Provider transport`
  button was not focused after the test's explicit focus operation at line 553.

For a controlled comparison, only the two changed production source files were
sourced from the reviewed HEAD in this same worktree, with automatic restoration
in a `finally` block. The two failed cases passed in isolation on that baseline.
After restoring the repair, the same two cases plus preview-workspace passed
(3 tests, 28.5 seconds):

```sh
CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e \
  agent.spec.ts settings-presentation.spec.ts preview-workspace.spec.ts \
  --grep 'composer primary seat, uploads and context stack light 390|confirming a removal settles focus|bounded preview workspace' \
  --output=/var/tmp/rustx-450-repair-evidence/repair-control
```

The isolated passes do **not** erase the full-suite failures or establish their
cause. Full browser validation is not reported green. These failures remain
outside the two repaired contract paths; no speculative product changes were
made to Composer or Settings. Evidence, including the full failed screenshots
and traces, is retained under `/var/tmp/rustx-450-repair-evidence/`.


## Settlement-domain repair (starting HEAD 966d7fbe)

The repair started from PR HEAD `966d7fbe2b1978f2e3e09c772787cb777541b544`
on the clean existing `issue-441-multi-preview-workspace` worktree. Starting
`origin/main` and merge base were both
`3aeefdc771f24d1e5acfc53da16c8bcbcbba35cd`. All eight hosted jobs for that starting
HEAD were successful; they are historical evidence, not validation of this repair.

The defect crossed two physical failure domains: the Host mapped both an Office
retirement failure and an uncertain native read to `document_settlement_unknown`,
then the shared finite-demand primitive poisoned both composed domains for either
settlement error. A converter-only failure could thus disable original reads and
Download across Sessions.

The final private taxonomy and owner-to-domain mapping are normative in
[document-previews.md](document-previews.md): converter-only uncertainty closes
only document admission; native read uncertainty closes raw-read admission and
any enclosing document demand; an unknown document carrier terminal witness
closes both because its physical phase is not known. The primitive receives each
domain's explicit predicate instead of interpreting arbitrary Host errors.
HTTP preserves all three physical facts before inspecting cancellation, and a
malformed terminal envelope does not count as a settlement witness. No public
App Server schema changes or error aliases are introduced.

Deterministic regressions:

- `session-files`: converter-only failure followed by successful original load,
  Download and another Session's read; exact calls and URL revocation; file and
  whole-document uncertainty refuse subsequent reads/Download/derive locally
  across coordinators; Host authority replacement alone creates fresh domains.
- `document-http-lifetime`: gated cancellation preserves each typed physical
  failure through the terminal envelope; lost body/invalid envelope fail closed.
  Existing cancellation gates still prove replacement starts only after exact
  settlement, without marking canceled operations unavailable.
- `product-host-file-read`: real private socket seam preserves native-read
  uncertainty through document derivation for Session files and Artifacts.
- `product-host-document-settlement`: real document orchestration and OOXML
  parser, with only native read and converter boundaries controlled, prove two
  settled reads precede Office admission; converter failure preserves raw reads,
  while reread failure prevents conversion and retains its exact error kind.
- `preview-workspace`: a failed Download followed by a successful explicit retry
  clears the notice at new intent without changing retained workspace identity,
  selection, presentation or active leases. `preview-intents` remains covered.

The six focused files passed 93 tests:

```sh
pnpm --dir web-console exec vitest run \
  test/session-files.test.tsx test/document-http-lifetime.test.ts \
  test/product-host-file-read.test.ts test/product-host-document-settlement.test.ts \
  test/preview-workspace.test.tsx test/preview-intents.test.tsx
```

Final source Web unit validation passed 101 files / 1677 tests. Typecheck, build,
i18n and provenance checks passed. The native private file-read join, Node socket
settlement, Office cgroup/sandbox limits, geometry epoch/fullscreen witness,
explicit Close semantics and sole Conversation scroll ownership remain intact.
The final source browser run uses the existing pinned container without changing
screenshots, tolerances, retries or deadlines.

Linux validation uses `TMPDIR=/var/tmp/p450s` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. The short clean temporary root avoids the
previously observed ambient `/tmp` ancestor configuration. macOS-specific
execution is left to the hosted macOS jobs, not claimed as a local run.

Exact final-source Linux commands and results (the environment above applies to
the native/repository lane commands):

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | PASS |
| `cargo build --bins --all-features --locked` | PASS |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | PASS |
| `cargo test --test contracts --test provider --all-features --locked` | PASS |
| `cargo run --locked --example check_test_lanes -- --job rust-contracts` | PASS |
| `cargo test --lib --all-features --locked -- boundary_suites::` | PASS |
| `cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | PASS |
| `cargo run --locked --example check_test_lanes -- --job rust-boundaries` | PASS |
| `pnpm --dir protocol/app-server check` | PASS |
| `pnpm --dir protocol/app-server typecheck` | PASS |
| `pnpm --dir tui typecheck` | PASS |
| `pnpm --dir tui test` | PASS |
| `pnpm --dir dev typecheck` | PASS |
| `pnpm --dir dev test` | PASS |
| `bwrap --unshare-all --unshare-user --disable-userns --die-with-parent --new-session --cap-drop ALL --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 -- /usr/bin/true` | PASS |
| `node web-console/scripts/check-office-sandbox.ts` | PASS |
| `cd test-support/fake-provider && uv sync --frozen && uv run --frozen pytest` | PASS, 52 tests |
| `pnpm --dir web-console typecheck` | PASS |
| `pnpm --dir web-console test` | PASS, 101 files / 1677 tests |
| `pnpm --dir web-console build` | PASS |
| `pnpm --dir web-console check:i18n` | PASS |
| `pnpm --dir web-console check:provenance` | PASS |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | FAIL: 165 passed, 4 failed (12.4m) |
| `git diff --check` | PASS |
| `git diff origin/main...HEAD --check` | PASS |

The single full browser run against frozen final production/test source failed:

- `agent.spec.ts:96`, light 390 and dark 390: stable 334×244 Composer captures
  differ by 15 and 16 pixels respectively, outside any registered noise region
  (maximum channel delta 13 and 18). Strict screenshot assertions remain intact.
- `locale.spec.ts:6`: selecting English timed out after the option detached from
  the DOM; trace points to `shell-actions.ts:79` from `locale.spec.ts:25`.
- `settings-presentation.spec.ts:540`: the Remove Provider transport trigger was
  inactive when dismissal expected it to regain focus.

These required acceptance failures remain unresolved in the excluded
Composer/Settings paths. They are not waived or claimed to be caused by the
settlement repair. No retry, deadline, assertion, baseline or tolerance changes
were made. The PR is **not ready** on this local validation evidence. The real
preview-workspace, document, original Download and reading acceptance paths
passed within this run. No browser source changed after the run began.

Logs and browser traces/screenshots are retained locally under
`/var/tmp/rustx-450-settlement-evidence/`. Final hosted status is reported in the
PR description, separately from these local results. The starting SHA's green
hosted run is historical evidence only.

Before committing/pushing, main was fetched again and remained
`3aeefdc771f24d1e5acfc53da16c8bcbcbba35cd`, also the merge base. No integration was
required. The primary checkout remained at that SHA with only its pre-existing
untracked `.playwright-mcp/` directory.
