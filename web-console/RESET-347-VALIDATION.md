# WEB-RESET-03 acceptance record

Recorded 2026-09-18. Architecture: [Settings and native surfaces](SETTINGS-ARCHITECTURE.md).

## Repository and prerequisite

- Original checkout: `/home/caismis/Documents/codes/rustX`, clean `main` at
  `204f7ccc8fbaf4bc1b6842e02e8d0d68f19d5837`; left unchanged.
- Isolated worktree: `/home/caismis/Documents/codes/rustX-issue-347`, branch
  `issue-347-harness-native`.
- Fetched main/base: `146fa10f44ead5e70bd5584d1a2c6bc7cc81754f` (#349).
- Reviewed full #347, merged #348/#349/#333, current generated schema, native
  configuration/resource owners, client/recovery tests, provenance and all open PRs.
- #352 remained the only open overlapping PR, at
  `3d675b06ef6d174bbd84ff09f9903ac889d87116`. Its exact head is incorporated by
  merge `77de2840`, preserving #349's deletion of the old Conversation component
  and moving Goal Tool display labels into the current binding/ToolCard contract.
  Native Goal changes in the branch come from this prerequisite, not a second
  #347 lifecycle implementation. This PR depends on #352; if that head changes or
  lands by squash, reconcile its resulting main history before merge.
- The only additional Rust semantics introduced for #347 are the missing Root
  identity/description source-mutation units through the existing native writer.

## Validation commands and results

Commands below ran in the isolated worktree. The host is Linux x86_64, Node
24.20.0, pnpm 11.13.1, rustc 1.95.0. Rust used the existing build cache via
`CARGO_TARGET_DIR=/home/caismis/Documents/codes/rustX/target`; an ignored `target`
symlink lets the existing browser fixtures locate those binaries.

| Directory | Command | Result |
| --- | --- | --- |
| web-console | `corepack enable` | Pass |
| web-console | `corepack install` | Pass |
| web-console | `pnpm install --frozen-lockfile` | Pass |
| web-console | `pnpm typecheck` | Pass |
| web-console | `pnpm test` | 327 passed, 25 files |
| web-console | `pnpm check:provenance` | 103 source records and 100 production-package notices verified |
| web-console | `node scripts/provenance.ts --reference /tmp/rustx-345-harness` | Pass against clean pinned upstream checkout |
| web-console | `pnpm build` | Pass, source/license artifact check included |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e:update` | 4 reference suites passed; six new Settings references and four updated shell references reviewed |
| web-console | `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh accessibility.spec.ts commands.spec.ts console.spec.ts settings.spec.ts uploads.spec.ts workflow.spec.ts chat.spec.ts` | 10 passed |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e` | 19 passed against the real App Server/provider emulator/Product Host; repeated after the clean-removal CAS fix |
| web-console | `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh settings.spec.ts integrations.spec.ts workflow.spec.ts` | 3 passed with additional real-server review captures |
| web-console | `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh composer.spec.ts` | 1 passed with responsive-layout evidence synchronization |
| protocol/app-server | `pnpm install --frozen-lockfile` | Pass |
| protocol/app-server | `pnpm generate` | Pass; Rust-owned schema and generated TypeScript regenerated |
| protocol/app-server | `pnpm check` | Pass; no generated schema/type/fixture drift |
| protocol/app-server | `pnpm typecheck` | Pass |
| dev | `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` | Pass; 30 tests |
| tui | `pnpm install --frozen-lockfile`, `pnpm typecheck`, `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass; 778 tests |
| test-support/fake-provider | `uv sync --frozen`, `uv run --frozen pytest` | Pass; 51 tests |
| repository | `cargo fmt --all -- --check` | Pass |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| repository | `cargo build --bins` | Pass |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,853 passed; 1 existing ignored test |
| repository | `cargo test --test contracts --test provider --all-features` | 27 + 166 passed; 5 existing opt-in live tests ignored |
| repository | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 422 passed across all seven targets |
| repository | `git diff --check` | Pass |

The full native Linux CI commands above ran after the #352 reconciliation. No
native source changed afterward. macOS execution belongs to its separate CI
runner; this Linux host does not claim a macOS result. Existing Vite chunk-size
and Node color-environment advisories are non-failing. No failure was waived.

During development, TypeScript caught a fixture missing native origin `base` and
Testing Library options that belonged to Playwright; both were corrected. The
first full browser run had nine failures from outdated textarea/Tab-order/raw
Inspector assertions and duplicate diagnostic text rendering. The Inspector now
has a readable native summary with explicitly disclosed JSON diagnostics; tests
open that disclosure and preserve the same incarnation/lineage/interaction
assertions. Scope tests use standard arrow-key tab navigation. All failed cases
passed in the focused run and subsequent complete runs. Visual review also caught
captures during transitions; evidence now waits for actual responsive geometry,
with reduced motion and no ordering sleeps.

## Deterministic contract coverage

- Effective/User/Workspace, read-only Effective, native provenance and published
  generation; User-only/restart-required process policy.
- Complete Provider/Model create, edit, remove, redaction, reasoning profiles,
  native request parameters, compatibility fields and Root model selection.
- Independent Root native/MCP/Python selections, all/none/exact, omission versus
  empty replacement, guidance, Skills, closed Plugins and delegation allowlists.
- Complete independent Agent resource replacement, model inheritance, guidance,
  worktree, timeout and delegation fields; resource scope, shadowing, validity,
  selection and readiness remain separate native facts.
- Exact CAS base revision, draft retention across navigation, reviewed-revision
  replacement, clean-unit removal conflict, validation rejection and credential
  draft clearing after successful native redaction.
- Save leaves the old generation published; Reload publishes coherent native
  state; failed/busy Reload retains old state. Lost Save/Reload replies and
  reconnect reread without replay. Deterministic overlapping-read tests prevent
  obsolete observations from replacing newer reads/write acknowledgements.
- Native GoalPhase/Todo/Queue controls, current Subagent/Workflow/background
  projection, Trace history, managed artifact bounds/fences/URL cleanup, inert
  text preview and browser-local appearance.

## Browser and visual review

No Browser plugin was available, so validation used the repository's pinned
Playwright browser container (`v1.63.0-noble`, digest in `browser-tests.sh`). No
host fonts or browser cache became reference authority. Browser suites use the
real compiled Rust App Server, shared provider emulator and Product Host. The
six new screenshot references separately use deterministic native-shaped
projections; they are not represented as real-server evidence.

Manually reviewed full-size captures of desktop/narrow Settings, light/dark,
Effective, User and Workspace, Provider/Model forms, native source Save and pending
reload, successful publication, conflict review, failed publication, resource
inventory, Plugins, named Agents, Goal/Todo, Subagent/Workflow, Trace, artifact
preview and Inspector. Keyboard tests traverse 390/820/1280/1600px layouts, close
paths and focus return; reference tests cover the existing shell and Agent seats.

Selected real-server evidence is checked in under
[docs/images/web-reset-347](../docs/images/web-reset-347):

- [User Provider, saved source / old generation](../docs/images/web-reset-347/cfg3-user-provider-pending.png)
- [User Model detail](../docs/images/web-reset-347/cfg3-user-model.png)
- [Effective catalog and provenance](../docs/images/web-reset-347/cfg3-effective-provenance.png)
- [MCP conflict and explicit revision review](../docs/images/web-reset-347/cfg3-cas-conflict.png)
- [Native Plugin composition](../docs/images/web-reset-347/cfg3-native-plugins.png)
- [Dark narrow Agent profile](../docs/images/web-reset-347/cfg3-agent-mobile-dark.png)
- [Native Subagent and Workflow](../docs/images/web-reset-347/native-workflow-subagent.png)
- [Workflow inventory](../docs/images/web-reset-347/native-workflow-inventory.png)
- [GoalPhase and Todo on narrow layout](../docs/images/web-reset-347/composer-mobile.png)
- [Native Trace inspector](../docs/images/web-reset-347/trajectory-inspector.png)
- [Managed artifact right panel](../docs/images/web-reset-347/native-artifact-preview.png)
  (the native image fixture intentionally returns a one-pixel PNG).

CI uploads the rest of `web-console/test-results`. Fixed visual references live
beside `settings-presentation.spec.ts` and the existing shell/Agent suites.

## Dependency and cleanup review

The provenance checker enforces presentation imports; `presentation/` has no
configuration/client/runtime owner imports. Only safe appearance plus existing
connection/tab presentation preferences use browser storage. Native drafts and
secrets do not. Removed the old Settings CSS, obsolete activity-card rules and
stale source-inventory record. The deleted legacy Conversation component was not
resurrected by #352. Updated obsolete textarea/raw-layout assertions instead of
adding compatibility wrappers. There is one Settings root, one right-panel seat,
one token/theme system and no old/new mode.

The native authoring API has no Workflow-program, Skill-package or Python-source
write operation. Their inventory/diagnostics and supported Root selection are
implemented; no speculative editor, marketplace or lifecycle authority was added.
