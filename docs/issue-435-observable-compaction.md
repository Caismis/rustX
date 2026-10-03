# Issue #435: observable native context maintenance

## Scope and baseline

Implementation starts at `06dc8b101acf5a880466548e9e9b3214e41ac7e8`, fetched
`origin/main`, in `rustX-issue-435` on `issue-435-observable-compaction`.
The primary checkout remains on that commit with its existing untracked
`.playwright-mcp/` untouched. Main was fetched again before final validation;
PRs #445 and #446 remained open, and main had not moved. Neither PR is a base.

## Ownership and ordering

`AgentComposer` command invocation → `App.invokeCommand` →
`AppServerClient.compact` → App Server `context/compact` → Runtime Client →
`ConversationRuntime::compact_context` is the sole manual execution path.
The client synchronously claims its Session/attachment/generation request slot
before consuming the exact command draft and before RPC dispatch. The generic
compact modal, second confirmation action, compact-specific modal copy, and
`CommandSession.compact` path are removed. Other commands retain their architecture.

The Context engine owns summary construction and the durable summary/Surface
commit. The maintenance coordinator admits, excludes competitors, restores the
Conversation, and releases ownership. Runtime Client owns projection; Web owns
intent, transport evidence, scope fences, and presentation only.

1. Local request evidence exists before dispatch. It is not native admission.
2. Native admission checks out ConversationState and publishes manual start.
3. The owned task commits summary and Surface. Its settlement gate can still hold
   coordinator ownership at this point; the RPC is unfinished and no terminal
   manual observation has been published.
4. `finish_manual_compaction` restores ConversationState and clears the slot under
   the coordinator lock, then publishes committed history and terminal manual
   observation. The waiter completes afterward. This is the release boundary used
   by the resident surface. Automatic context completion does not settle its Attempt.

The new UI adds no canonical history. Existing native summary identity and transcript
paging remain responsible for checkpoints. No fake command message, synthetic
Attempt, execution queue, polling, retry, or cancellation mechanism is introduced.

## Repair and applicability

Old projection was insufficient: a false progress bit or a newer checkpoint cannot
identify a lost request. App Server v33 requires `request_id`; Runtime Client v57
retains one live `manual_compaction` record with that ID, `released`, and the native
pre-commit error. Start/failure events now carry the complete context projection.
The correlation travels with the owned maintenance task, independent of its waiter.
All generated/current consumers move atomically; v32 is removed, not supported.

Reconnect reads native state without resending the mutation. Matching released
correlation resolves the pending request. No record, an overwritten record, another
client's ID, or a newer unrelated checkpoint leaves the request uncertain. Automatic
compaction does not overwrite manual correlation. This is deliberately not durable
operation recovery across runtime incarnation replacement. Authority, Conversation,
incarnation, attachment and connection-generation fences retire obsolete evidence
and callbacks. A confirmed successful RPC remains successful after a read failure.

Occupancy required no new native field. The native reader already pairs the latest
prepared request's measured input with its frozen model/capacity. A newer unmeasured
request and compaction invalidate it; later measured usage restores it. Zero is
measured zero. Missing input or invalid capacity is unavailable, not zero. Desired
model changes, transcript paging, and unsent draft text cannot supply measurements.
The resident Composer disclosure names the historical model and input/capacity.

PR #409 (`f268175bb8d31010706e7070aae80d2b46b7aced`) removed occupancy from
`ConversationStats` while rebuilding resident conversation/Turn presentation. Its
diff and PR description establish that placement change, not a separate rationale
for removing occupancy as a product capability. This implementation retains the
resident Turn design and adds the measurement to the current Composer context stack.

## Deterministic acceptance evidence

| Contract | Evidence |
| --- | --- |
| Direct command, one request, exact draft consumption, no modal or User turn | `commands.test.tsx`: compact invocation with held RPC, repeated Enter, subsequent draft, request count, no `turn/start` |
| Submission, start, commit versus release, remount | `context-seat.test.tsx`: held response; native progress with newer committed count remains running; remount; matching release |
| Actual native commit/release boundary, no synthetic Attempt | `manual_compaction_commits_and_restores_the_idle_conversation`: post-commit settlement gate, durable generation, owned slot, unfinished waiter, correlated observations; projection regression |
| Native-only automatic/other-client observation and useful errors | `context-seat.test.tsx`: uninitiated progress/error and other-client progress during own uncertainty; native pre-commit maintenance regressions |
| Lost response before sufficient evidence, after release, failed release, unrelated/newer checkpoint | Four correlation cases in `context-seat.test.tsx`; exactly one request after reconnect, exact uncertainty removal only |
| Obsolete authority/Conversation/incarnation/attachment/generation/later request | Held old replies and old-socket events in `context-seat.test.tsx`; replacement attachment starts a newer request before old response is released |
| Pending inbound admission | Command registry/client pending-inbound regression and existing native maintenance admission contract; no empty-queue rule added |
| History overlap/paging | Existing `transcript.test.ts` overlap, refresh, resync and reconnect fences; no second historical representation added |
| Occupancy zero/absent/invalid capacity and remeasurement | Four Web measurement cases; native `context_measurement_is_native_and_is_invalidated_by_compaction` adds newer unmeasured request and genuine zero to existing frozen-model, compaction and remeasurement coverage |
| Browser languages, themes, keyboard details, narrow layout and non-modal interaction | Four native `compaction.spec.ts` cases: en/zh, light/dark, 1440/390; real provider gate, exact measurement, diagnostic disclosure, retained editable draft, invalidation; en/light reconnects while held |
| Composer residency, reading/streaming stability | Existing full browser suite, including Agent, accessibility, transcript/streaming and navigation coverage, retained with the new Context stack seat |

Tests use deferred RPCs, native settlement gates and provider emulator gates. Timeouts
remain liveness guards; no sleeps prove ordering. Screenshot references reflect the
new resident row. The obsolete live Composer corner noise allowance is removed;
its original image and all comparator assertions remain frozen regression evidence.
The original crop used exact comparison; the review repair below records the
subsequently measured reference-local raster exceptions. No global tolerance or
timeout was increased.

## Reference audit

The approved read-only DeepSeek Harness pin is
`639ed015397290b3745d163aafe02ffee4aa3f84`. The eight actually inspected paths and
hashes are listed in `web-console/PROVENANCE.md` and `source-inventory.json`.
They cover CompactionCommandCard, CompactionItem, GenericCommandCard, compaction and
command node projection, the chat snapshot contract, ContextMeter, and occupancy.
The local reference checkout was neither switched nor modified. Behavioral ideas
inform the non-modal disclosure; native execution/storage remain rustX-owned.

## Original implementation validation environment

Validation uses Linux x86_64, the repository toolchains, provider emulator, and pinned
Podman Playwright image. Native tests use
`TMPDIR=/var/tmp/rustx-issue-435-validation`: the ambient `/tmp` already contains an
unrelated `.git` and `rustx.toml`, which interfere with parent-discovery tests.
Those files were left untouched. The isolated temporary root changes no assertions.
macOS and Windows execution were not available in this environment.

The CI-equivalent commands used for this change are:

| Command | Result |
| --- | --- |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 74 files, 1,373 tests passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | 147 source records and 132 production package notices verified |
| `pnpm --dir protocol/app-server check` | Generated contract matches |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo build --bins --all-features --locked` | Passed |
| `TMPDIR=/var/tmp/rustx-issue-435-validation RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | 4,089 passed, 0 failed, 8 existing ignored, across 19 test binaries |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, 0 failed, 0 skipped |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 38 passed, 0 failed |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `git diff --check` | Passed |

Focused discovery was also verified: 111 command/Composer/context-seat tests;
6 native manual-compaction tests; 18 Agent/compaction browser cases; and the native
Composer dock case after its stack assertion was updated. Initial browser failures
identified the expected changed screenshots and obsolete stack cardinality, which
were corrected without changing product admission behavior or weakening assertions.

The eight existing ignored native tests are two opt-in startup/stage measurements,
one fixture-corpus regeneration writer, and five live paid-provider tests requiring
API credentials. They were not executed; deterministic provider-emulator coverage
was required and executed. Hosted CI is left to run after PR creation and is not
continuously monitored by this task.

Final browser validation:
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` — **152 passed, 0 failed
(9.4 minutes)**, with screenshot updates disabled. Main was fetched once more before
delivery and remained `06dc8b101acf5a880466548e9e9b3214e41ac7e8`; #445 and #446
were still open. No main integration was necessary.

## PR #447 architecture-review repair

The repair starts from PR HEAD `9ee612f6ad3d50e84cfc707d3f85c8ce41386721`.
`ManualCompactionRequestId` now accepts exactly 1–64 ASCII letters, digits,
underscores or hyphens. UUID strings remain valid. Its private Rust field and
fallible deserializer enforce this before App Server dispatch reaches native
maintenance. The same type travels through the owned task and native projection.
Schema v33 has `minLength: 1`, `maxLength: 64`, and
`^[A-Za-z0-9_-]+$(?![\s\S])`; the final assertion excludes trailing newlines in
ECMAScript regex implementations too. Generated TypeScript documents the domain.
This unmerged PR keeps v33 and Runtime Client v57; no compatibility reader is added.

The 4096-event replay count now bounds correlation text in ordinary compaction
lifecycle events to at most 256 KiB total (plus fixed object/string overhead and
one current snapshot). The existing 16 MiB accounting for the selected large
read-domain/workflow variants is unchanged. The formerly unbounded caller string
cannot enter replay or the snapshot. This does not claim a new whole-replay byte
budget for all existing event fields, and does not change diagnostic semantics.

Previously, the local duplicate guard ended only in the RPC promise's `finally`.
Now an exact matching native `request_id` with `released=true` clears only that
operation's guard. The transport request remains pending until real settlement.
A later explicit B can be sent while A's reply is held; A's response/error
cannot overwrite or unlock B. Failed native release follows the same rule.
A false progress bit, changed count, or unrelated correlation cannot free A.

### CI screenshot evidence and correction

CI run `37044726558` (artifact `11245406105`) failed exactly the light/dark
`composer-idle-empty-*-390-linux.png` references: identical 334×154 dimensions,
12 changed pixels each, maximum raw channel delta 2. All differences are on the
top-left half-pixel Composer hairline at x=24–33, y=51–54; no text, control,
layout or other pixel differs. The card uses radius 22px, `superellipse(1.5)`,
and a 0.5px elevation stroke. This is corner coverage rasterization variance;
we do not infer Chromium's internal trigger from the artifact.

A separate probe ran 20 fresh contexts per theme in the repository's pinned
Playwright 1.63.0 container, with the same viewport, fixed clock, reduced motion,
locale, theme and capture options. All 40 local captures exactly match their
checked-in baseline; the CI variant was not reproduced locally. The probe recorded
DOM, computed styles and floating geometry. The card is x=72, y=742, 302×98;
its background and stroke match the intended theme. Raw evidence is in
`/tmp/435-ci` and `/tmp/435-probe`; the committed manifest retains all changed
before/after RGBA tuples, independent of temporary artifacts.

| Theme | Baseline decoded RGBA SHA256 | CI actual decoded RGBA SHA256 |
| --- | --- | --- |
| Light | `07c6161435ed976a8bc432152892f15667f19a284d5ef2f5774f9058f5b86d81` | `0cc01987af01ef2790b409f591d4071cb1580d5d5dd05d75579c43f198e2edd6` |
| Dark | `bc6ebdd547f48d0d0db69c4cca86dd564d93e63c56b55ad16d3c854e29ed1f70` | `70e6b65fe54db71057822e1105ae033257073be101ec194ef85708c119e9a75b` |

The correction grants each reference exactly twelve 1×1 regions, each limited to
one changed pixel and its own measured delta (1 or 2). No unmeasured coordinate
is included. Baselines, product styles, stabilization, retries and timeouts are
unchanged. Altering product curvature or elevation to work around browser coverage
would change the intended design; the measured per-pixel exception is narrower.
The earlier moved fixture concerns a different `running-draft` reference and
remains frozen. It cannot affect these two idle-reference comparisons.
Comparator regressions replay the CI tuples and reject excess deltas, neighboring
pixels, text/control changes, another reference name, shifted layout and dimensions.

### Locale and deterministic coverage

Chinese is selected before unavailable occupancy, empty-context failure, keyboard
opened diagnostic details, the read-repair action, known 25% occupancy and native
model/capacity details. A held outbound compact request proves localized submitting
text before native admission; the provider gate then proves running and completion.
The four en/zh × light/dark cases retain desktop/390px coverage, editable later
drafts, no modal and the English reconnect case. The full browser suite retains
streaming and reading-position contracts.

The native boundary regression exercises a normal UUID, the 64-character maximum,
65 and 900,000 characters, empty, non-ASCII, whitespace and trailing-newline values.
It checks generated schema agreement, exact JSON-RPC rejection, unchanged native
snapshot/cursor (no start or correlation mutation) and zero provider requests.
The two guard interleavings cover successful and failed release, B's distinct ID,
exactly two RPCs, A's late reply, and B's still-owned guard. Existing four reconnect
correlation cases remain in the focused suite.


The required repeated narrow tests also exposed focus-corner variants in the
current running-draft references (334×220): 15 light pixels, maximum delta 13;
16 dark pixels, maximum delta 18, at rows 117–120. A second probe used the
unchanged exact-RGBA stabilization cadence and the actual empty → filled →
running → queued-draft flow in 12 fresh contexts per theme. Light produced
2 baseline / 10 alternate captures; dark produced 8 baseline / 4 alternate.
Every element's DOM, complete computed styles, floating rectangles, scroll and
focus state were identical within each theme. Canonically serialized probe hashes:
light `acaf0d6e7acf6650c9bdb62065f3101306a7e4f1e68d9b49e93d7a941a26e9bd`,
dark `af43618b5e1d7506913a214557692f55375e707aa2308bd13b953968fda5c693`.
Only the measured focus-corner pixels differ. Current live exceptions were therefore
remeasured as 15/16 separate 1×1 sites with per-site channel bounds, not restored
from a broad obsolete region. The earlier frozen comparator fixture remains intact.
All four live references have adversarial policy tests; every other reference and
unmeasured pixel retains its existing exact comparison policy.

### Pre-integration repair validation

Focused checks: 46 Web guard/comparator tests; 7 native manual-compaction tests;
10 repeated 390px Agent runs (five per theme); and all 4 corrected locale lifecycle
cases passed. The new repair-button assertion caught an old setup assumption:
blindly toggling Inspector opened it on narrow layouts and covered the button.
Setup now closes Inspector only when expanded; no forced click or timeout change
was used. The initial failing diagnostic run was stopped after its cause was known.

| Command | Repair result |
| --- | --- |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 74 files, 1,383 passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | 147 source records, 132 package notices verified |
| `pnpm --dir protocol/app-server check` | Passed, v33 regenerated without drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo build --bins --all-features --locked` | Passed |
| `TMPDIR=/var/tmp/rustx-issue-435-validation RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | 4,090 passed, 0 failed, 8 existing ignored, 19 test binaries |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, 0 failed, 0 skipped |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 38 passed, 0 failed |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 152 passed, 0 failed (9.5 minutes), no screenshot updates |
| `git diff --check` and `git diff --cached --check` | Passed |

Focused browser commands used the pinned repository runner:
`CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh agent.spec.ts --grep 'composer primary seat.*390' --repeat-each=5`
and `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh compaction.spec.ts`.
No screenshot updates were enabled. The isolated TMPDIR and the eight existing
ignored native tests have the same reasons documented above. New-head macOS and
Windows execution were not performed locally.

Real RPC settlement also frees only its own guard before awaiting snapshot repair; a dedicated held-read regression proves the read cannot delay a later explicit invocation.

Final fetch before delivery kept `origin/main` and the merge base at
`06dc8b101acf5a880466548e9e9b3214e41ac7e8`. PRs #445 and #446 remained open;
no main integration was needed. The primary checkout retained only its pre-existing
untracked `.playwright-mcp/` directory and was not modified.


## Integration with merged PR #445

The clean PR-owned worktree started at `ff397c4652befede7fb409138e53133944a0fab4`,
matching the remote branch. Main had advanced from the old merge base
`06dc8b101acf5a880466548e9e9b3214e41ac7e8` to #445's merge commit
`45e8ca434994910587078538d80ca10d4a7ce9d0`. The two #447 commits were rebased
onto that accepted architecture; no merge commit or replacement branch was made.
Earlier validation and the no-integration statement above describe the preceding
head only, not this integration.

Actual textual conflicts were `web-console/source-inventory.json`,
`web-console/src/locale/dictionaries/agent.ts`, and
`web-console/test/conversation-residency.test.tsx`. Locale keys were united;
residency retained main's `RuntimeClientEvent` import and event-driven exact Attempt
rendering while migrating to v33. Inventory references were united and final source
hashes/import closures recomputed. Both parent inventories' destination and
inspected-only sets remain present; both approved reference audits are retained.

The mechanically merged owner files were reviewed against current main:

- `AgentComposer.tsx` differs only by its v33 import. Busy Enter, StopSequence,
  composition/focus/menu fences and Queue/Steer preference remain unchanged.
- `ConversationSeat.tsx` adds ContextSeat before the docks while preserving exact
  cancellationTarget, client/binding/native scope and Attempt selector dependency.
- `app-server.ts` retains main's cancellationTarget/cancelTurn and dispatch fence.
  `turn/cancel` remains in the two-slot lifecycle-control lane; ordinary RPC
  saturation cannot delay it and full control capacity is definitely unsent.
  Compaction correlation/gesture ownership remains a separate domain. Native
  exact release or actual transport settlement releases only its own invocation.
- Native Rust, generated protocol and TUI match the reviewed #447 head. There is
  no new native redesign or version bump: App Server v33 / Runtime Client v57.

The current-v32 audit also found conflict-free references in the cancellation test
and the agent fixture's Window declaration; both were migrated. Current normative
protocol prose was corrected, including the architecture, uploads, deletion,
file-delivery, jobs, invariant and contribution documents. Historical #431/#444
validation reports retain their then-current v32/v56 evidence. The old database
v32 rejection in session-deletion-ownership is a database version, not App Server.
The protocol introduction retains explicitly historical v32/v56 explanations and
its statement rejecting earlier peers. No current v32 generated reader remains.

The combined deterministic regression runs for both Queue and Steer. In the real
App fixture it observes ContextSeat, submits the ordinary draft through the correct
native method, verifies unchanged exact cancellation identity, cancels via double
Escape, then invokes `/compact` directly after native Attempt settlement. It proves
exact command consumption, no modal or extra turn, authoritative context completion,
and retention of text typed after invocation. The seven-file focused integration
run passed 111 tests, including both combined cases; native manual compaction passed
all seven focused regressions.

Browser execution uses the repository's pinned Chromium container. Another active
worktree occupied default ports, so this integration uses the existing
`RUSTX_E2E_PREVIEW_PORT=5273 RUSTX_E2E_FIXTURE_PORT=5274` overrides without touching
that worktree or its processes. All nine accepted #445 keyboard/persistence browser
cases passed before screenshot updates. Four primary-seat comparisons failed
against temporary main-side references, demonstrating the changed combined UI.
The three textual conflicts and 70 binary conflicts were handled separately;
main-side PNGs were only temporary mechanical resolutions.

### Integrated visual references

The scoped update ran `agent.spec.ts locale.spec.ts settings-presentation.spec.ts
shell.spec.ts` through the pinned runner with explicit screenshot update enabled:
54 tests passed. It regenerated the 70 overlapping conflict references from the
combined product; 59 changed bytes and 11 were identical. No changed PNG lies
outside the conflict list. Visual inspection covered light/dark narrow context
stacks, desktop Composer, and Settings. Context, To-dos, Goal, Queue, Composer and
Busy Enter remain visible in their intended seats. Every later verification run
uses no screenshot update mode.

All four pre-integration Composer noise entries were removed before testing. Fresh
strict repeats then reproduced the two idle corner variants in all five captures
per theme (each theme had one identical actual RGBA hash across those captures): each is now
334×178 (previously 334×154), exactly 12 changed pixels at x24–33/y51–54, maximum
channel delta 2. Both integrated references use the opposite pixel values from the
old reference, so their before/after tuples and RGBA hashes were remeasured rather
than copied. The manifest records one 1×1 region per measured site, budget one,
and each site's exact channel delta. Adversarial tests reject neighboring pixels,
text/controls, excess delta, another reference, dimension changes and layout shifts.

The running-draft crops are now 334×244 (previously 334×220). Both old running
allowances were deleted; five strict runs per theme passed all six Composer states
(10 tests, 44.0 seconds). Separate comparator assertions keep those running corner
and text/control sites strict. Existing frozen comparator evidence is unchanged;
no broad region, global tolerance, retry, timeout or product style was changed.

Changed references (relative to `web-console/test/e2e/`):

- `agent.spec.ts-snapshots/agent-dark-desktop-linux.png`
- `agent.spec.ts-snapshots/agent-dark-narrow-linux.png`
- `agent.spec.ts-snapshots/agent-error-light-linux.png`
- `agent.spec.ts-snapshots/agent-selectors-light-linux.png`
- `agent.spec.ts-snapshots/agent-settled-light-linux.png`
- `agent.spec.ts-snapshots/agent-streaming-light-linux.png`
- `agent.spec.ts-snapshots/agent-tools-light-linux.png`
- `agent.spec.ts-snapshots/composer-attachment-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-attachment-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-attachment-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-attachment-light-390-linux.png`
- `agent.spec.ts-snapshots/composer-context-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-context-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-context-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-context-light-390-linux.png`
- `agent.spec.ts-snapshots/composer-idle-draft-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-idle-draft-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-idle-draft-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-idle-draft-light-390-linux.png`
- `agent.spec.ts-snapshots/composer-idle-empty-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-idle-empty-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-idle-empty-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-idle-empty-light-390-linux.png`
- `agent.spec.ts-snapshots/composer-running-draft-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-running-draft-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-running-draft-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-running-draft-light-390-linux.png`
- `agent.spec.ts-snapshots/composer-running-empty-dark-1440-linux.png`
- `agent.spec.ts-snapshots/composer-running-empty-dark-390-linux.png`
- `agent.spec.ts-snapshots/composer-running-empty-light-1440-linux.png`
- `agent.spec.ts-snapshots/composer-running-empty-light-390-linux.png`
- `locale.spec.ts-snapshots/locale-general-zh-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-agent-narrow-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-delete-confirm-light-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-mobile-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-mobile-menu-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-provider-detail-mobile-dark-linux.png`
- `shell.spec.ts-snapshots/desktop-collapsed-rail-linux.png`
- `shell.spec.ts-snapshots/desktop-expanded-dark-linux.png`
- `shell.spec.ts-snapshots/desktop-expanded-light-linux.png`
- `shell.spec.ts-snapshots/desktop-right-panel-linux.png`
- `shell.spec.ts-snapshots/mobile-expanded-dark-linux.png`
- `shell.spec.ts-snapshots/mobile-rail-dark-linux.png`
- `shell.spec.ts-snapshots/mobile-settings-dark-linux.png`
- `shell.spec.ts-snapshots/session-idle-light-linux.png`
- `shell.spec.ts-snapshots/session-queued-light-linux.png`
- `shell.spec.ts-snapshots/session-reconnect-light-linux.png`
- `shell.spec.ts-snapshots/session-stopping-light-linux.png`
- `shell.spec.ts-snapshots/session-uncertain-light-linux.png`
- `shell.spec.ts-snapshots/session-uncertain-mobile-linux.png`
- `shell.spec.ts-snapshots/settings-shell-dark-linux.png`
- `shell.spec.ts-snapshots/settings-shell-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-delete-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-empty-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-named-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-other-uncertain-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-preview-light-linux.png`
- `shell.spec.ts-snapshots/sidebar-scoped-inspector-linux.png`
- `shell.spec.ts-snapshots/workspace-session-browser-linux.png`


### Integrated validation

The final Web unit run passed all 1,446 tests in 78 files. An earlier run timed out
one unchanged residency test at its existing 5-second limit while full native and
browser work overlapped; its focused rerun passed (43 residency/comparator tests),
and the complete final unit run passed without source, assertion or timeout changes.
The initial stale-noise-policy failures and the strict idle measurement failures
were diagnostic runs, not reported as passing verification.

| Command | Integrated result |
| --- | --- |
| `pnpm --dir protocol/app-server check` | Passed, regenerated v33 without drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 78 files, 1,446 passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | 148 source records, 132 package notices |
| `pnpm --dir web-console build` | Passed, artifact provenance verified |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 38 passed |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, 0 failed/skipped |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo build --bins --all-features --locked` | Passed |
| `TMPDIR=/var/tmp/rustx-issue-435-validation RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | 4,090 passed, 0 failed, 8 existing ignored, 19 binaries |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |

Focused Web command:
`pnpm --dir web-console exec vitest run test/composer-cancellation.test.tsx test/composer-escape.test.tsx test/submission-policy.test.ts test/composer-preferences.test.ts test/request-lifetime.test.ts test/conversation-residency.test.tsx test/context-seat.test.tsx`
passed 111 tests. `TMPDIR=/var/tmp/rustx-issue-435-validation cargo test --lib --all-features --locked manual_compaction`
passed 7 tests, including bounded admission and commit/release gates.

All browser commands below use the two port overrides stated above:

- `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh agent.spec.ts --grep 'scoped Composer keyboard policy|Busy Enter preference is read|composer primary seat'`: 9 keyboard/persistence passes; 4 expected pre-update reference failures.
- `RUSTX_SCREENSHOT_UPDATE=1 CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh agent.spec.ts locale.spec.ts settings-presentation.spec.ts shell.spec.ts`: 54 passed; intentional scoped reference update only.
- `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh agent.spec.ts --grep 'composer primary seat.*390' --repeat-each=5`: final 10 passed after fresh idle measurement; running references strict.
- `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh compaction.spec.ts`: 4 passed, both languages/themes.

The new head was validated locally on Linux; macOS/Windows were not executed
locally. The eight existing ignored native tests and TMPDIR isolation have the
reasons recorded above. Hosted CI for the rebased head is separate evidence.


Final complete browser verification:
`RUSTX_E2E_PREVIEW_PORT=5273 RUSTX_E2E_FIXTURE_PORT=5274 CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e`
passed **161 tests in 9.8 minutes**, with screenshot updates disabled.
Both `cargo run --locked --example check_test_lanes -- --job rust-contracts` and
`cargo run --locked --example check_test_lanes -- --job rust-boundaries` passed.
Final whitespace checks passed. The post-generation current-v32 reference audit
returned zero matches for generated imports/schema paths, WebSocket subprotocols,
initialize constants and App Server version constants.

Immediately before delivery, fetch confirmed main and the exact merge base still
`45e8ca434994910587078538d80ca10d4a7ce9d0`; #446 remained open. Main did not move
again. Remote #447 remained `ff397c4652befede7fb409138e53133944a0fab4`, the explicit
force-with-lease guard for the existing branch. The primary checkout was not
modified and retained its pre-existing untracked `.playwright-mcp/` directory.
