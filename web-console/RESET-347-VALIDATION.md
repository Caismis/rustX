# WEB-RESET-03 acceptance record

Final repair validation recorded 2026-09-18. Architecture: [Settings and native surfaces](SETTINGS-ARCHITECTURE.md).

## Repository and prerequisite

- Original checkout: `/home/caismis/Documents/codes/rustX`, clean `main` at
  `204f7ccc8fbaf4bc1b6842e02e8d0d68f19d5837`; left unchanged.
- Isolated worktree: `/home/caismis/Documents/codes/rustX-issue-347`, branch
  `issue-347-harness-native`.
- Initial review base: `146fa10f44ead5e70bd5584d1a2c6bc7cc81754f` (#349).
- Final main/base: `58ec906aaaca314a2ed6db2f2d2adfbed6aa37ac`, the merge of #352.
- #352 was reconciled independently from its published head, preserving the
  Harness Agent architecture. Its final head `606d2e839886ca96df2035a104cbdf2a58d9f873`
  passed all seven CI lanes and was merged before the final #357 rebase.
  The unrelated dirty `/home/caismis/Documents/codes/rustX-issue-351` worktree
  was left untouched.
- #357 was rebuilt from its #347 commits onto that main. The old Goal merge and
  duplicate Goal redesign are absent from the PR diff. GoalPhase and its Tool
  display binding now come from main; no Goal runtime/lifecycle authority is
  introduced here.
- Native #347 changes are Root identity/description source units, plus acceptance
  of the schema's omitted Agent text defaults while preserving native byte bounds.
  Generated protocol changes belong to the Root units and native Agent text
  projection metadata, not the Goal redesign.

## Validation commands and results

The full matrix below passed **after** rebasing onto main `58ec906a`. Linux
x86_64, Node 24.20.0, pnpm 11.13.1; Rust commands used
`RUSTUP_TOOLCHAIN=1.98.1` to match CI. The existing native build cache is exposed
through the ignored `target` symlink. Browser execution uses the repository's
real App Server, Product Host and mandatory provider emulator.

| Directory | Command | Result |
| --- | --- | --- |
| web-console | `corepack enable` | Pass |
| web-console | `corepack install` | Pass |
| web-console | `pnpm install --frozen-lockfile` | Pass |
| web-console | `pnpm typecheck` | Pass |
| web-console | `pnpm test` | 340 passed, 25 files |
| web-console | `pnpm check:provenance` | 103 source records and 100 production-package notices verified |
| web-console | `node scripts/provenance.ts --reference /tmp/rustx-345-harness` | Pass against the clean pinned upstream checkout |
| web-console | `pnpm build` | Pass, artifact provenance included |
| web-console | `CONTAINER_ENGINE=podman pnpm test:e2e` | 20 passed; unchanged screenshot references pass |
| protocol/app-server | `pnpm install --frozen-lockfile` | Pass |
| protocol/app-server | `pnpm generate` | Pass; regenerated native Agent projection metadata |
| protocol/app-server | `pnpm check` | Pass; invokes native generation, no schema/type/fixture drift |
| protocol/app-server | `pnpm typecheck` | Pass |
| dev | `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` | Pass; 30 tests |
| tui | `pnpm install --frozen-lockfile`, `pnpm typecheck`, `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass; 788 tests |
| test-support/fake-provider | `uv sync --frozen`, `uv run --frozen pytest` | Pass; 51 tests |
| repository | `cargo run --example generate_schemas` | Pass; regenerated native Agent schema |
| repository | `cargo fmt --all -- --check` | Pass |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| repository | `cargo build --bins` | Pass |
| repository | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 2,853 passed; 1 existing ignored test |
| repository | `cargo test --test contracts --test provider --all-features` | 27 + 166 passed; 5 existing opt-in live tests ignored |
| repository | `cargo test --lib --all-features -- boundary_suites::` | 226 passed |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 423 passed across all seven targets |
| repository | `git diff --check` | Pass |

Focused repair commands also passed: `pnpm exec vitest run
test/settings-units.test.tsx test/settings.test.tsx test/integrations.test.tsx`
(44 cases), `cargo test --test cfg3_catalog named_agent` (3 cases), and
`CONTAINER_ENGINE=podman bash scripts/browser-tests.sh settings-contracts.spec.ts`
(1 real-server test). Full Rust/Web suites also passed on the reconstructed tree
before the final rebase; those earlier runs are not substituted for the matrix above.

Initial development browser runs exposed a label locator problem and the native
non-empty Agent text check that contradicted the schema. Both were corrected. A strengthened wire assertion then caught native projection
materializing empty text defaults; native serialization now omits them and the
actual request/TOML assertions pass. The role-based browser flow and all final
suites pass. An accidentally restarted
browser runner was stopped and its port collision cleared before rerunning. The
new #352 worktree's first Web typecheck required installing its TUI dependencies;
its rerun passed. No semantic failure was waived. Vite's existing chunk-size and
Node's color-environment advisories remain non-failing.

#352 independently passed the same CI-equivalent local lanes, plus all seven
GitHub CI jobs at its final head before merge. Its local counts were 301 Web,
18 browser, 788 TUI, 2,852 Rust unit, 27 + 166 contract/provider, 226 in-crate and
422 external boundary tests. The final rebased #357 CI status is linked from
[PR #357](https://github.com/Caismis/rustX/pull/357), including macOS execution;
this Linux host does not claim to execute macOS locally.

## Deterministic contract coverage

- Effective/User/Workspace, read-only Effective, native provenance and published
  generation; User-only/restart-required process policy.
- Complete Provider/Model create, edit, remove, redaction, reasoning profiles,
  native request parameters, compatibility fields and primary Root model selection.
  The review repair below adds the previously missing nested Summary contract.
- Independent Root native/MCP/Python selections, all/none/exact, omission versus
  empty replacement, guidance, Skills, closed Plugins and delegation allowlists.
- Complete independent Agent resource replacement, model inheritance, guidance,
  worktree, timeout and delegation fields; resource scope, shadowing, validity,
  selection and readiness remain separate native facts.
- Exact CAS base revision, draft retention across navigation, reviewed-revision
  replacement, clean-unit removal conflict, validation rejection and credential
  draft clearing after successful native redaction.
- The historical configuration publication checks in this report are superseded
  by issue #380; see [current configuration contracts](../docs/configuration.md).
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
automatic application, successful adoption, conflict review, failed publication, resource
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

## PR #357 review repair

The repair preserves the Harness shell and native CFG3 ownership. Real-server
validation exposed a native mismatch: the schema defaults omitted Agent text to
empty, but definition construction rejected it. That native check now accepts
the native defaults while retaining byte bounds and all other validation. Native
projection/serialization also omits empty defaults. Schema and protocol metadata
were regenerated from those Rust attributes. Deterministic regressions now include:

- `settings-units.test.tsx`: both Root and named-Agent Summary identity changes
  retain a named reasoning profile, explicit output limit and request params in the
  exact replacement mutation. Nested edits, catalog defaults, intentional Session
  replacement and returning to a minimally authored explicit selection are covered.
- `integrations.test.tsx`: implicit HTTP and stdio project truthfully; opening
  writes nothing. Ordinary edits retain headers/environment and omitted type;
  intentional transport switching replaces the shape and clears old retained keys.
  Pure helper coverage includes explicit-type precedence and empty editor state.
- `settings-units.test.tsx`: omitted description, omitted instructions and both
  omitted allow a valid form submission when Tools change; the exact complete
  mutation does not fabricate missing text fields.
- `settings.test.tsx`: deferred rejections from both effect reads and manual refresh
  reads arrive after either a newer refresh or a successful write acknowledgement.
  In all four cases, current data/revision survive and no stale alert commits.
- `tests/cfg3_catalog.rs`: omitted Agent text remains valid through native discovery,
  exact-revision complete-profile Tool edits and source rereads. Projection JSON
  and serialized TOML do not materialize omitted empty defaults. Oversized profile
  text still fails before any file is committed.
- `settings-contracts.spec.ts`: real App Server Summary round trips for Root and
  named Agents, optional Agent text (including actual outgoing mutation omission),
  implicit MCP transport and retained secrets,
  followed by explicit native publication. The new controls were manually reviewed
  in the real-server [Summary editor capture](../docs/images/web-reset-347/summary-model-complete.png).

Final self-review confirms no broad Goal runtime/protocol redesign remains in
#357, no generated source was hand-edited, no browser configuration authority was
introduced, and all four review findings have explicit regression coverage.
