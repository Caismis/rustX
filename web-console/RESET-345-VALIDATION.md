# WEB-RESET-01 implementation and acceptance record

## Repository and reference

- Original checkout: `/home/caismis/Documents/codes/rustX`; unchanged tracked files.
- Isolated worktree: `/home/caismis/Documents/codes/rustX-issue-345`.
- Branch: `issue-345-harness-presentation-reset`.
- Fetched `origin/main` / exact base: `533cd341a3cd899e5fa9fdc693899cd35cd9a15b`.
- Open PR inspection at start returned no open PRs.
- Harness fetched from `https://github.com/deepseek-ai/deepseek-harness`, current
  default `master`, selected immutable HEAD `ddefc45fbc7f8e46dd73185e68295696d1297887`.
- Issue #345, repository instructions, Web docs, App Server protocol and Product
  Host/native Session ownership contracts were read before implementation.

[Architecture/classification](SHELL-ARCHITECTURE.md) and
[final provenance inventory](PROVENANCE.md) document the source/adapter boundary.
The presentation reset changes are in `web-console`; the rendering-infrastructure
correction also changes the Web lane in `.github/workflows/ci.yml`. No Rust,
generated protocol, Host contract or native client semantic implementation changed. The workspace presentation adapter
now rejects classifications paired with a replaced native summary array.

## Deterministic tests

| Test | Contract |
| --- | --- |
| `shell.test.tsx` | Exact upstream columns/rail; Sidebar collapse/expand; Settings/Inspector/Appearance gestures emit no native requests; waiting interactions outrank running; native page offsets and bounded persisted navigation hints |
| `workspaces.test.tsx` | Existing create/rename/fork/admission/reconnect/unregister/supersession tests retained with new UI selectors; new deferred-classification test proves a new page cannot inherit the previous page's Workspace classification |
| `primitives.test.tsx` | Disabled Menu action refusal; accessible disclosure; new nested modal test proves Escape closes only the top layer and retains background isolation |
| `presentation.test.tsx` | App boots through native client into the new shell; existing canonical history/interaction/unmount checks retained |
| `client.test.ts` (unchanged) | Native initialization, generation fencing, authoritative snapshot/subscribe recovery, response loss without replay, controller release versus cancellation/unload, attachment lifetime isolation |
| `commands.test.tsx` | Native historical operation semantics retained; selector dismissal uses new modal contract |
| Existing remaining unit suites | Host authorization, CFG3 CAS/publication, Tool/interaction, streaming/canonical rendering, uploads and resource limits remain covered |

The final unit run has **289 passing tests across 22 files** (283 tests existed
before the reset). Race tests use held promises, explicit request observation and
commit/reply control. The one fake-timer advance in `shell.test.tsx` concerns the
upstream 150ms presentation fade only; it proves no execution race by elapsed time.
No native semantic tests were deleted to accommodate the new shell.

## Browser reference evidence

`test/fixtures/shell.html` is a test-only Vite entry using `test/fixture.ts`'s
controlled JSON-RPC transport and the real typed AppServerClient. Session identity,
Host metadata, canonical message content, snapshots and dates are fixed. It is
not a production feature, alternate runtime or build entry. The clock is fixed,
reduced motion is explicit, and Screenshot assertions wait for browser stability.

Checked-in references under `test/e2e/shell.spec.ts-snapshots/`:

| Reference | State |
| --- | --- |
| [desktop-expanded-light](test/e2e/shell.spec.ts-snapshots/desktop-expanded-light-linux.png) | 1440×1000, grouped Workspace, Ungrouped Sessions, selected running A and waiting B |
| [desktop-collapsed-rail](test/e2e/shell.spec.ts-snapshots/desktop-collapsed-rail-linux.png) | 56px rail and expanded main column |
| [desktop-right-panel](test/e2e/shell.spec.ts-snapshots/desktop-right-panel-linux.png) | Harness three-column geometry with rustX Inspector |
| [workspace-session-browser](test/e2e/shell.spec.ts-snapshots/workspace-session-browser-linux.png) | Native metadata search in upstream contextual result rows |
| [settings-shell-light](test/e2e/shell.spec.ts-snapshots/settings-shell-light-linux.png) | Settings frame/nav and Appearance seat |
| [settings-shell-dark](test/e2e/shell.spec.ts-snapshots/settings-shell-dark-linux.png) | Same Settings chrome using upstream dark tokens |
| [desktop-expanded-dark](test/e2e/shell.spec.ts-snapshots/desktop-expanded-dark-linux.png) | Dark grouped shell |
| [mobile-rail-dark](test/e2e/shell.spec.ts-snapshots/mobile-rail-dark-linux.png) | 390×844, responsive default rail |
| [mobile-expanded-dark](test/e2e/shell.spec.ts-snapshots/mobile-expanded-dark-linux.png) | Explicitly expanded Sidebar; upstream squeezed-center behavior retained |
| [mobile-settings-dark](test/e2e/shell.spec.ts-snapshots/mobile-settings-dark-linux.png) | Narrow Settings with horizontal section navigation |

### Authoritative rendering contract

The source of truth is `scripts/browser-tests.sh`, invoked by the **Full Web
conformance** CI lane and by local comparison/update commands. Arbitrary workstation
rendering is **not** a reference authority. The browser alone runs in a fresh
container; the existing real App Server, Product Host, provider emulator, native
TUI and Playwright test runner continue running on the Linux host.

- OS/architecture: Ubuntu **24.04.4 LTS (Noble), linux/amd64**.
- Immutable image: `mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
- Playwright: **1.63.0**, exactly pinned in `package.json` / `pnpm-lock.yaml`.
  The runner rejects a package/image version mismatch. The container uses the
  lockfile-installed Playwright JS through a read-only `node_modules` mount;
  there is no separate npm download or floating browser channel.
- Browser: image-provided Chromium Headless Shell **153.0.8010.12**, Playwright
  revision **1243**. Browser binaries, fontconfig, FreeType and all OS libraries
  are fixed by the image digest. Host fonts and browser caches are not mounted.
- Fonts: image-provided `fonts-liberation` **1:2.1.5-3** (the production Harness
  stack's Helvetica resolves to **Liberation Sans**; code uses Liberation Mono),
  `fonts-freefont-ttf` **20211204+svn4273-2**, `fonts-ipafont-gothic`
  **00303-21ubuntu1**, `fonts-noto-color-emoji` **2.047-0ubuntu0.24.04.1**,
  `fonts-tlwg-loma-otf` **1:0.7.3-1**, `fonts-unifont` **1:15.1.01-1build1**,
  `fonts-wqy-zenhei` **0.9.45-8**, plus the image's X fonts. These are test
  environment prerequisites, not imported Harness source or production assets.
  A startup guard explicitly checks Helvetica resolves to Liberation Sans.
- Provisioning: Docker pulls that exact image on first use and reuses its immutable
  layers afterward. Podman is supported through `CONTAINER_ENGINE=podman`.
  A native Linux x86_64 host is required; emulated ARM/macOS rendering is not an
  authority. Host networking lets Chromium reach the unchanged loopback fixtures;
  the Playwright server binds only to `127.0.0.1` on an ephemeral port. The wrapper
  removes its container on success, failure or interruption.
- Comparison: **`threshold: 0`, `maxDiffPixels: 0`**, no retries. Normal runs use
  `updateSnapshots: 'none'`, so even missing references fail instead of being
  written. Direct host Playwright runs are rejected with the required command.
  No screenshot-only fonts/CSS, skipped states or production theme changes.

Install the existing host prerequisites (Node 24, Corepack, Docker or Podman,
Rust, uv and the repository's Python toolchain), then from the repository root:

```sh
cargo build --bins
(cd tui && corepack enable && corepack install && pnpm install --frozen-lockfile)
(cd dev && corepack install && pnpm install --frozen-lockfile)
(cd test-support/fake-provider && uv sync --frozen)
cd web-console
corepack enable
corepack install
pnpm install --frozen-lockfile

# Intentional update: builds, starts the pinned browser, then executes
# playwright test shell.spec.ts foundation.spec.ts --update-snapshots
pnpm test:e2e:update

# Ordinary comparison: builds and runs ALL real-server and reference tests.
pnpm test:e2e

# Optional focused comparison, using the same authority (build first):
pnpm build
bash scripts/browser-tests.sh shell.spec.ts foundation.spec.ts
```

For Podman, prefix the three browser commands with `CONTAINER_ENGINE=podman`.
Do not run a bare `playwright test --update-snapshots` on a workstation. Upgrade
Playwright, the image tag/digest and the version guard together, then intentionally
regenerate and review all references. CI compares only. It does not install host
fonts or Chromium, perform apt upgrades inside the rendering container, or cache a
mutable browser environment. Existing Rust cache keys and non-Web lanes are unchanged.

Review PNG diffs and the corresponding source change together. Normal `pnpm
test:e2e` compares the checked-in references and never rewrites them. Reference
images establish composition evidence; semantic assertions establish authority.
Screenshots were inspected visually, including light/dark desktop, search, right
panel and narrow Settings/rail/expanded states.

The real-server suite uses the production build plus actual Rust App Server/Tools,
Product Hosts, native TUI client and fake provider over HTTP/SSE. Its gates test
chat/history, native uploads and fork independence, create/delete, 34 repeated
controller releases, pending interactions, reconnect/resync, CFG3 CAS and lost
mutation/reload responses, commands, Todo/Goal/Queue, Workflow and isolated Hosts.
Keyboard coverage exercises 390/820/1280/1600px. Inspector is explicitly opened for
diagnostics and closed before operating content covered by its narrow fullscreen
panel. Hover/focus reveals upstream row actions; tests use stable native IDs when
names/automatic titles can change.

## Original validation (superseded for screenshot reproducibility)

The original run used Fedora Linux, Node 24.20.0, Corepack pnpm 11.13.1 and
Playwright 1.63.0 with host browser libraries. Its local screenshot pass proved
only consistency on that workstation, **not CI reproducibility**. The following
historical results are retained as the reset's implementation record; the pinned
rendering validation below supersedes its screenshot claims. The
Browser plugin was unavailable, so the repository's regular Playwright workflow
was used. No external model credentials were needed.

Commands below ran in `web-console/` unless another directory is specified.

| Command | Result |
| --- | --- |
| `corepack enable` | Passed |
| `corepack install` | Passed; pinned pnpm cached |
| `pnpm install --frozen-lockfile` | Passed; no manifest or lockfile changes |
| `pnpm typecheck` | Passed |
| `pnpm test` | Passed: 289 tests / 22 files |
| `pnpm check:provenance` | Passed: 90 derived destinations; 100 production package notices; import, persistence/transport and hash boundaries |
| `pnpm build` | Passed; artifact includes exact MIT and dependency notices |
| `pnpm exec playwright install chromium` | Passed |
| `pnpm exec playwright test shell.spec.ts foundation.spec.ts --update-snapshots` | Passed: 2 tests; 10 intentional reference images |
| `pnpm test:e2e` | Passed: 17 tests, 1.3 minutes; zero skipped |
| `node scripts/provenance.ts --reference /tmp/rustx-345-harness` | Passed: current and historical source bytes at their recorded exact commits |
| `CARGO_TARGET_DIR=/home/caismis/Documents/codes/rustX/target cargo build --bins` (repository root) | Passed; real server/tool binaries, reused existing compilation cache |
| `uv sync --frozen` (`test-support/fake-provider`) | Passed |
| `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` (`dev`) | Passed; 30 development composition tests |
| `git diff --check` (repository root) | Passed |

An ignored `target` symlink in the isolated worktree points to the original
checkout's compilation cache for the existing browser fixture's binary paths.
Original source files remain unchanged. Rust fmt/clippy and the broad Rust matrix
were not run because no Rust owner or generated protocol changed.

Intermediate runs found and fixed: portal autofocus before placement, unbounded
large native dialogs, nested modal isolation, stale classification/page pairing,
hover-only row action discovery, transitional resize assertions and selectors
coupled to old chrome/Session titles. Earlier failed runs are not counted as passes.
The build still reports its existing >500kB chunk advisory; this reset adds no
packages. Browser coverage is Chromium on Linux; it does not claim Safari/Firefox
or a full Agent/CFG3 experience rewrite.

## PR #348 rendering-infrastructure correction

Inspected the actual remote head `36425bff329a4f210b14ea421f92caeb270b45bd`,
[existing review thread](https://github.com/Caismis/rustX/pull/348#discussion_r4040309910),
and [failed run 35255086307](https://github.com/Caismis/rustX/actions/runs/35255086307)
before editing. Only Full Web conformance failed: `shell.spec.ts:18`,
`desktop-expanded-light.png`, **13,323 pixels** different under the original
comparison settings; all preceding semantic assertions and the other **16 E2E
tests** passed. Inspected expected/actual/diff PNGs, error context and trace.
The trace records loaded fonts and two identical actual screenshots. Differences
are glyph shapes/widths and associated text-dependent control layout; the shell
column boundaries remain aligned. Fedora's system fallback/rasterization stack
and the rolling GitHub Ubuntu 24.04 host were different rendering environments.
A `-linux.png` filename did not define either environment sufficiently.

Before updating references, the pinned container on the Fedora host reproduced
the original GitHub CI actual image with **zero raw pixel differences**. This
isolates the rendering environment as the cause without changing product code.
The ten references are normalized under that single image; all representative
states, fixed clock/data, reduced motion, branding and geometry assertions remain.
No production dependency, font, theme, asset or Harness provenance record changes.

Manual old/new review covered every PNG: expanded light/dark, collapsed rail,
Inspector, Workspace/Session search, Settings light/dark, narrow rail, narrow
expanded Sidebar and narrow Settings. Text metrics change wrapping (including the
Inspector's main-column toolbar and narrow conversation/composer); the unchanged
product CSS supplies all geometry. Sidebar/rail widths, Settings frame, icons,
colors and responsive behavior are preserved. There were no product edits hidden
in reference regeneration.

Correction validation (Node 24.20.0 / pnpm 11.13.1; browser authority above):

| Command | Result |
| --- | --- |
| `corepack enable`, `corepack install`, `pnpm install --frozen-lockfile` | Passed; no dependency/lockfile change |
| `pnpm typecheck` | Passed |
| `pnpm test` | Passed: 289 tests / 22 files |
| `pnpm check:provenance` | Passed: 90 source records / 100 production package notices |
| `pnpm build` | Passed; existing chunk-size advisory only |
| `pnpm exec playwright install --with-deps chromium` | Passed in a disposable copy of the pinned image; 0 packages installed/upgraded. That container was discarded; baseline runs use pristine image instances, never apt-mutated state |
| `CONTAINER_ENGINE=podman pnpm test:e2e:update` | Passed: 2 tests, all 10 references generated under the pinned authority |
| `CONTAINER_ENGINE=podman pnpm test:e2e` | Passed: 17 tests, 1.3 minutes, no skips/retries; all 10 references compared with zero threshold/differing pixels |
| `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh shell.spec.ts foundation.spec.ts` | Passed: 2 tests in a second fresh container; all reference comparisons passed |
| `pnpm exec playwright test --list` outside the wrapper | Rejected as intended with the pinned-environment command, before rendering |
| `git diff --check` | Passed |

The existing real-server prerequisites also passed: `cargo build --bins`,
`uv sync --frozen`, TUI dependency install, and dev dependency install/typecheck/
30 tests. The Browser plugin was unavailable; repository Playwright supplied the
page identity, no-overlay/page-error, responsive, interaction and screenshot checks.
Workflow review confirmed only the Web browser provisioning changes: no added
package installation, unchanged Rust cache policy, no browser cache, unchanged
non-Web lanes. The digest-addressed image's local layer reuse cannot substitute a
new browser/font filesystem. GitHub Actions results are recorded in the existing
PR review thread after the pushed head completes CI.
