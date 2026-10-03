# PR #446 integration onto PR #445 main

> Historical #445 integration report. The later v33 / PR #447 integration is
> documented in [the current integration report](issue-446-v33-integration.md).
> The light running-draft allowance described here is no longer live policy.

## Starting state and history

- Starting remote PR HEAD: `865427af2dd59b42cd79260f8f7c8199b4998349`.
- Starting and integrated main: `45e8ca434994910587078538d80ca10d4a7ce9d0`.
- Old merge base: `06dc8b101acf5a880466548e9e9b3214e41ac7e8`; ahead 4 / behind 5.
- GitHub initially reported `CONFLICTING` / `DIRTY`, no checks for the old HEAD, auto-merge disabled.
- Main's delta is `ac8c5e70` (Composer policy/preferences), `ffbb5a2d` (unsent cancellation and accessible names), `678b1024` (bounded lifecycle-control cancellation), `77b2c3de` (current-contract docs), and merge `45e8ca43` (PR #445).

The existing four feature commits were rebased with `git rebase origin/main` in
the issue worktree. No merge commit, replacement branch or replacement PR was
introduced. Before integration validation the rebased source HEAD was
`1bb6eb7a6f791f6dbc35193183e2b17a28442b9d`. Final references and renewed evidence
are recorded by the subsequent integration commit. Push must use the explicit
lease `refs/heads/feat/issue-432-open-workspace:865427af2dd59b42cd79260f8f7c8199b4998349`.

## Text and ownership decisions

| File | Resolution |
| --- | --- |
| `web-console/PROVENANCE.md` | Retain both #439 Composer and #432 desktop provenance sections. No upstream attribution removed. |
| `web-console/source-inventory.json` | Preserve all 148 main source records and current import closures; add the five distinct #432 inspected-only paths by commit/path identity; recompute only the composed agent dictionary's local hash. The second replay conflict was this hash again after the rediscovery copy correction. |
| `web-console/src/locale/dictionaries/agent.ts` | Retain main's four new Composer keys in both locales and #446's sixteen desktop keys in both locales. No duplicate keys or old dictionary replacement. |

`README.md`, `App.tsx`, `ConversationHeader.tsx`, `test/fixtures/agent.tsx` and CI
auto-merged and were reviewed semantically. The fixture differs from main only by
its desktop-mode block using the existing Server/workspaceHost; main's cancellation
and preference fixture controls are intact. App passes the existing Host/authority
to the Session header; the header retains the Open Workspace control. Neither
Composer nor a second mock authority owns desktop actions. README contains both
current cancellation capacity and Open Workspace contracts. CI is main plus the
macOS Node desktop lane and its explanatory comments.

Main's `AgentComposer`, `ConversationSeat`, Composer preference/submission/stop
modules, AppServerClient, Composer CSS and `agent.spec.ts` are byte-identical to
main. #446's desktop Host/adapter, workspace interfaces, OpenWorkspace control,
desktop regression tests and macOS Node lane are unchanged from starting PR HEAD.
No Rust source or generated App Server protocol changes were introduced.

## Reference conflict review

The 39 header-bearing feature references were captured into a temporary candidate
directory using the exact digest-pinned Playwright 1.63.0 browser authority. The
candidate-only helper was outside the repository, selected exactly those 39 file
names, used the unchanged `captureStable`, and never wrote checked-in references.
All other comparisons stayed strict. The 18 targeted fixture cases passed.

Each candidate was reviewed against base, main and old PR (contact sheets and
full-size narrow examples). A pixel-by-pixel check established that the candidate
pixels differing from main and those differing from the old PR are disjoint for
**every one of the 39 images**. Thus the integrated capture preserves main's
Composer rendering and #446's header rendering, without unexplained new pixel
values. Main changed Composer/preference placement and associated content/menu
layout; #446 changed the Session header action. Overlay images include the header
through their existing blur. With the mobile sidebar expanded, the added header
wraps and shifts the narrow transcript; the current-main bottom Composer remains.

The 31 conflicting PNGs below all use their generated, reviewed integrated
candidate. Bounds are half-open `(left, top, right, bottom)` of changed pixels
against each input. No PNG was accepted through bulk ours/theirs selection.
Current-main images used provisionally during replay were replaced before push.

| Reference (under `test/e2e/*-snapshots`) | Candidate vs main bounds | Candidate vs old PR bounds |
| --- | --- | --- |
| `agent-dark-desktop-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `agent-dark-narrow-linux.png` | (92, 22, 257, 35) | (58, 93, 388, 844) |
| `agent-error-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `agent-selectors-light-linux.png` | (1185, 23, 1307, 35) | (459, 829, 1376, 1000) |
| `agent-settled-light-linux.png` | (1185, 23, 1307, 35) | (459, 830, 1261, 980) |
| `agent-streaming-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `agent-tools-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `locale-general-zh-linux.png` | (1210, 17, 1311, 39) | (281, 860, 1262, 1000) |
| `settings-delete-confirm-light-linux.png` | (1179, 17, 1312, 40) | (281, 860, 1257, 1000) |
| `desktop-collapsed-rail-linux.png` | (1185, 23, 1307, 35) | (275, 860, 1221, 1000) |
| `desktop-expanded-dark-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `desktop-expanded-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `desktop-right-panel-linux.png` | (537, 23, 659, 35) | (282, 860, 790, 1000) |
| `mobile-expanded-dark-linux.png` | (280, 10, 390, 556) | (280, 604, 390, 844) |
| `mobile-rail-dark-linux.png` | (92, 22, 257, 35) | (58, 668, 388, 844) |
| `mobile-settings-dark-linux.png` | (324, 5, 390, 305) | (275, 613, 390, 844) |
| `session-idle-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `session-queued-light-linux.png` | (1185, 23, 1307, 35) | (459, 822, 1261, 1000) |
| `session-reconnect-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `session-stopping-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `session-uncertain-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `session-uncertain-mobile-linux.png` | (93, 22, 257, 35) | (58, 668, 388, 844) |
| `settings-shell-dark-linux.png` | (1180, 18, 1311, 40) | (278, 860, 1259, 1000) |
| `settings-shell-light-linux.png` | (1180, 18, 1311, 40) | (281, 860, 1262, 1000) |
| `sidebar-delete-light-linux.png` | (1180, 18, 1311, 40) | (459, 860, 1261, 1000) |
| `sidebar-empty-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `sidebar-named-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `sidebar-other-uncertain-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `sidebar-preview-light-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |
| `sidebar-scoped-inspector-linux.png` | (537, 23, 659, 35) | (282, 860, 790, 1000) |
| `workspace-session-browser-linux.png` | (1185, 23, 1307, 35) | (459, 860, 1261, 1000) |

The eight non-conflicting candidates were RGBA-identical to the existing feature
references and were not rewritten:

- `agent-approval-light-linux.png`
- `agent-questionnaire-light-linux.png`
- `session-banner-blocked-light-linux.png`
- `session-banner-failed-light-linux.png`
- `session-banner-failed-mobile-dark-linux.png`
- `session-banner-preparing-light-linux.png`
- `session-configuration-banner-light-linux.png`
- `sidebar-background-light-linux.png`

Current-main Composer-only references, including the 334×198 light running draft,
remain unchanged. No unrelated reference was updated. No global threshold,
stabilization rule, screenshot retry, deadline, or CI-only behavior changed.
Raw review artifacts are retained in `/tmp/pr446-integration`: conflict list,
per-image comparison bounds, candidates, base/main/old images and seven contact sheets.

## Renewed light-noise evidence

See [historical and integrated measurement](issue-446-web-conformance.md). The old
30-context measurement is preserved as pre-integration history, not relabeled as
current-main evidence. Initial comparator binding passed, but a new product-state
probe was still required. An initial probe was invalidated by protocol-generator
HMR and discarded; the accepted cohort runs after generation completes.

## Validation

- Focused desktop Host/UI: 36 passed (30 Host, 6 UI).
- PR #445's 12 focused Composer/cancellation/request-lifetime/incremental suites: 155 passed.
- Renewed screenshot comparator/stability suite: 36 passed, exit 0.
- Integrated light probe: 30 fresh contexts, 29 current reference / 1 stable variant; outer exit 0. See the separate current-state hashes in the evidence document.
- Full Web unit suite: 1,460 passed in 79 files.
- Web typecheck, i18n, provenance (148 source records / 132 package notices), build: exit 0.
- Dev typecheck and 38 tests: exit 0.
- App Server protocol check/typecheck, TUI typecheck: exit 0; no generated drift.
- Rust format, strict all-target/all-feature Clippy and binary build: exit 0.
- Whitespace: exit 0.
- Complete pinned browser suite: **159 passed (9.5m), outer exit 0**, using `setsid --wait env CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` on the free normal ports. No screenshot updates or acceptance retries.

No real Finder/Terminal GUI smoke is claimed. Linux headless validation and hosted
macOS Node/process tests establish their respective boundaries, not GUI completion.
The primary checkout remains untouched, including its existing `.playwright-mcp/` directory.

The first full-suite invocation used the configured isolated ports 5273/5274.
Two pre-existing `activity.spec.ts` navigations hard-code port 5174; both failed
at navigation while that port was unused. That invocation was stopped (exit 130),
not counted as acceptance. Only its own remaining Vite process groups were stopped.
The complete acceptance run uses free normal ports and an isolated process session
(`setsid --wait`); no unrelated test, port fallback or product code was changed.

Immediately before committing/pushing, a fresh fetch still showed main `45e8ca43`
and the remote feature HEAD `865427af`; no further main integration was needed.
Fresh hosted results for the pushed integrated HEAD are reported in the PR, not
inferred from the old green checks.
