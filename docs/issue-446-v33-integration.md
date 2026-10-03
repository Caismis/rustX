# PR #446 integration onto v33 / PR #447 main

## Repository state and strategy

Starting remote PR HEAD: `faa630a3057a031c5937b75742787ba17682533e`.
Starting and integrated main: `7d08560a50840d71b465adc09b384c61fc744b0a`.
Old merge base: `45e8ca434994910587078538d80ca10d4a7ce9d0`.
Initial topology: ahead 5 / behind 4, GitHub CONFLICTING / DIRTY.
All eight checks on the starting HEAD passed, but are not integrated validation.
The issue worktree was clean; the primary checkout retained its existing
untracked `.playwright-mcp/` directory and was not modified.

The five existing commits were replayed with `git rebase origin/main`.
Main's complete delta consists of:

- `80c08a50`: observe native compaction without a modal.
- `a27e6c7d`: bound compaction correlation and release gesture ownership.
- `1d9cdafa`: integrate observable compaction with merged Composer control policy.
- `7d08560a`: merge PR #447 / issue #435.

The actual latest main, not the old #445 tree, owns App Server v33, ContextSeat,
compaction submission/correlation/release, Composer behavior and screenshot policy.
The recorded remote HEAD above is the explicit force-with-lease expectation.

## Source conflict decisions

Actual textual conflicts were `web-console/source-inventory.json` (two replays),
`web-console/test/fixtures/rasterizer-noise.json` (two replays), and
`web-console/test/screenshot-comparison.test.ts`. No other text file conflicted.

- Inventory: start from latest main's 148 source records and current v33 import
  closures; union five unique desktop inspected-only records by commit/path.
  Recompute only the composed agent dictionary local hash. Main #447 provenance
  and all upstream hashes remain intact; no duplicates or old v32 closure.
- Noise/comparator: retain current main exactly. The old light live entry and
  old live light/dark comparator parameterization are superseded. Frozen dark
  comparator evidence remains where main intentionally retains it.
- App auto-merge: verified its sole diff from main passes `workspaceHost` and
  `workspaceAuthority` to ConversationHeader. Main's direct compact command path
  is unchanged.
- Header auto-merge: current v33 SourceTarget and main structure plus OpenWorkspace,
  Host/authority props, and exact current Session/node target. Existing actions,
  settings, inspector and tabs remain.
- ProductHostWorkspaces: current v33 interface plus only optional desktopCatalog
  and openWorkspace methods. No generated desktop RPC or protocol modifications.
- Agent fixture: latest-main v33 fixture plus the 11-line deterministic desktop
  mode block using the existing Server/workspaceHost. Main ContextSeat, compaction,
  cancellation and preference controls remain intact; no second authority.
- Localization/provenance/README auto-merges retain current main and desktop copy
  and records. Localization and provenance checkers pass.
- CI: latest main plus the dedicated macOS Node desktop job. Its pinned current
  dependencies and Host tests execute the v33-integrated source.

Main's ContextSeat, ConversationSeat, AgentComposer, AppServerClient, Composer
policy modules/CSS, Rust runtime, TUI and generated v33 tree are unchanged.
The desktop Host/adapter, OpenWorkspace, exact environment filtering, bounded
refresh and desktop regressions are unchanged from starting #446 HEAD.
Linux/macOS scope and unsupported Windows behavior remain unchanged.
The audit for current v32 imports, schema paths, WebSocket subprotocols and version
constants found no matches in Web/TUI/dev/protocol sources.

## Screenshot review

Used the exact pinned authority:
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.

A temporary capture helper outside the repository selected exactly 39 existing
header-bearing feature references. It copied latest-main specs/fixture sequences,
used unchanged captureStable and wrote candidates only under `/tmp/pr446-v33`.
All 18 selected fixture cases passed, outer exit 0. A preliminary invocation
failed to load the temporary helper's relative import.meta manifest path before
running tests; it was corrected outside the repository. No acceptance retry,
policy change or product helper change resulted.

All candidates were reviewed against base/main/old PR in seven contact sheets,
with full-size inspection of the expanded-mobile-sidebar composition. Thirty-eight
candidates have disjoint pixel sets differing from main versus old PR. The remaining
`mobile-expanded-dark-linux.png` has 4,946 overlapping changed pixels: the wrapped
Open Workspace header and added ContextSeat jointly reduce the transcript viewport,
changing its existing bottom-follow scroll/clipping. Main's ContextSeat/Composer
remains at the bottom; no Composer CSS/scroll ownership changes were made.

All 31 actual PNG conflicts below are resolved with the reviewed generated
integrated candidate, never bulk ours/theirs. Main changed ContextSeat/context
presentation and resulting transcript/menu layout; #446 adds the header action.
Overlay references include the header through existing background blur.
Bounds below are half-open candidate-vs-main changed-pixel bounds.

| Conflicting reference | Candidate vs main bounds |
| --- | --- |
| `agent-dark-desktop-linux.png` | (1185, 23, 1307, 35) |
| `agent-dark-narrow-linux.png` | (92, 22, 257, 35) |
| `agent-error-light-linux.png` | (1185, 23, 1307, 35) |
| `agent-selectors-light-linux.png` | (1185, 23, 1307, 35) |
| `agent-settled-light-linux.png` | (1185, 23, 1307, 35) |
| `agent-streaming-light-linux.png` | (1185, 23, 1307, 35) |
| `agent-tools-light-linux.png` | (1185, 23, 1307, 35) |
| `locale-general-zh-linux.png` | (1210, 17, 1311, 39) |
| `settings-delete-confirm-light-linux.png` | (1179, 17, 1312, 40) |
| `desktop-collapsed-rail-linux.png` | (1185, 23, 1307, 35) |
| `desktop-expanded-dark-linux.png` | (1185, 23, 1307, 35) |
| `desktop-expanded-light-linux.png` | (1185, 23, 1307, 35) |
| `desktop-right-panel-linux.png` | (537, 23, 659, 35) |
| `mobile-expanded-dark-linux.png` | (280, 10, 390, 621) |
| `mobile-rail-dark-linux.png` | (92, 22, 257, 35) |
| `mobile-settings-dark-linux.png` | (324, 5, 390, 305) |
| `session-idle-light-linux.png` | (1185, 23, 1307, 35) |
| `session-queued-light-linux.png` | (1185, 23, 1307, 35) |
| `session-reconnect-light-linux.png` | (1185, 23, 1307, 35) |
| `session-stopping-light-linux.png` | (1185, 23, 1307, 35) |
| `session-uncertain-light-linux.png` | (1185, 23, 1307, 35) |
| `session-uncertain-mobile-linux.png` | (93, 22, 257, 35) |
| `settings-shell-dark-linux.png` | (1180, 18, 1311, 40) |
| `settings-shell-light-linux.png` | (1180, 18, 1311, 40) |
| `sidebar-delete-light-linux.png` | (1180, 18, 1311, 40) |
| `sidebar-empty-light-linux.png` | (1185, 23, 1307, 35) |
| `sidebar-named-light-linux.png` | (1185, 23, 1307, 35) |
| `sidebar-other-uncertain-light-linux.png` | (1185, 23, 1307, 35) |
| `sidebar-preview-light-linux.png` | (1185, 23, 1307, 35) |
| `sidebar-scoped-inspector-linux.png` | (537, 23, 659, 35) |
| `workspace-session-browser-linux.png` | (1185, 23, 1307, 35) |

The eight other header-bearing candidates are RGBA-identical to the existing
feature references and were not rewritten:

- `agent-approval-light-linux.png`
- `agent-questionnaire-light-linux.png`
- `session-banner-blocked-light-linux.png`
- `session-banner-failed-light-linux.png`
- `session-banner-failed-mobile-dark-linux.png`
- `session-banner-preparing-light-linux.png`
- `session-configuration-banner-light-linux.png`
- `sidebar-background-light-linux.png`

Every reference outside this 39-image feature set stays byte-identical to latest
main, including all Composer-only, ContextSeat/compaction and new #447 references.
Latest-main changed references explicitly retained unchanged:

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
- `settings-presentation.spec.ts-snapshots/settings-agent-narrow-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-mobile-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-mobile-menu-dark-linux.png`
- `settings-presentation.spec.ts-snapshots/settings-provider-detail-mobile-dark-linux.png`

The light and dark 390 running-draft references remain current-main 334×244.
No broad screenshot update, unrelated baseline edit, global tolerance, screenshot
retry, timeout/deadline or capture stabilization change was made.

## Current noise contract and historical evidence

Latest-main live rasterizer-noise.json and comparator/stability implementation/tests
are byte-identical. Neither running-draft light nor dark has a live allowance.
Current-main frozen comparator fixtures are unchanged. No new desktop allowance
was introduced. The current comparator/stability suites pass all 39 tests.

`issue-446-web-conformance.md` retains both historical 30-context measurements,
clearly distinguishing pre-#445 evidence from the later main-45e8ca43 evidence.
Both precede #447's ContextSeat and strict remeasurement. The earlier integration
report is also marked historical. These old measurements are not relabeled or
used as current tolerance.

## Validation

- Web typecheck; 1,482 tests in 80 files; i18n; provenance (148 records / 132
  package notices); production build: passed.
- Focused 16-file desktop / #445 Composer / #447 ContextSeat and incremental
  suite: 247 passed. Includes the unchanged 30 Host and 6 desktop UI tests.
- Screenshot comparator/stability: 39 passed before candidate replacement and again against the final integrated references.
- Protocol v33 generation check/typecheck: passed; no generated drift.
- Dev typecheck and 38 tests; TUI typecheck: passed.
- Rust fmt, strict all-target/all-feature Clippy, binary build: passed.

Additional native validation: `TMPDIR=/var/tmp/rustx-issue-432-v33 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` passed **4090 tests**, zero failures, 8 existing ignored, across 19 binaries; outer exit 0.
The isolated TMPDIR avoids the unrelated ambient `/tmp/.git` and rustx.toml parent
discovery interference already documented by #447. No native assertions changed. The eight existing ignored tests are two opt-in
measurements, one fixture-regeneration writer, and five credentialed live-provider
tests; none was newly ignored by this integration.
Both repository lane-coverage checks (`rust-contracts` and `rust-boundaries`) passed.
The native manual-compaction admission/commit/release/projection cases passed.

Full pinned-browser acceptance: **163 passed (10.2m), outer exit 0** with
`setsid --wait env CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e`.
The normal ports were free. No unrelated processes were stopped. Both current
running-draft themes passed strictly, all four native compaction browser cases
passed, and desktop fixture/native-cwd browser acceptance passed.

Final whitespace and conflict-marker audits passed. A pre-push fetch still showed
main `7d08560a50840d71b465adc09b384c61fc744b0a` and remote PR
`faa630a3057a031c5937b75742787ba17682533e`; neither moved during integration.
The PR is pushed with an explicit lease against that remote HEAD. Fresh hosted
results are reported in the PR and final report, not inferred from old checks.
No real Finder/Terminal GUI smoke is claimed. Desktop process acknowledgement
is not GUI completion; macOS Node evidence is supplied by fresh hosted CI.
