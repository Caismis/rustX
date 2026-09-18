# WEB-RESET-02 validation and acceptance

## Repository and source authority

The original checkout `/home/caismis/Documents/codes/rustX` was clean on `main` at
`204f7ccc8fbaf4bc1b6842e02e8d0d68f19d5837`. Preflight fetched origin, read #346,
closed #345/merged #348 and found no overlapping open PRs. The isolated worktree
is `/home/caismis/Documents/codes/rustX-issue-346`, branch
`issue-346-harness-agent-experience`, based on that actual `origin/main` SHA.
A later fetch confirmed the same main SHA. The original checkout remains clean.

All new Harness derivations use **ddefc45fbc7f8e46dd73185e68295696d1297887**.
The audit checkout was inspected at that exact commit, never a floating branch.
`AGENT-ARCHITECTURE.md` records the preimplementation A/B/C classification;
`source-inventory.json` records exact original/local hashes, destinations, imports
and exclusions. The optional source audit verifies 100 records against the actual
upstream objects. Existing MIT/third-party notices still cover the unchanged
100-package production closure; no dependency or lockfile was added.

## Final architecture

Harness-derived `presentation/agent` owns disposable UI. `app/agent` and
`bindings/tools.ts` adapt finite native facts. `AppServerClient` remains the typed
transport/reconnect/uncertainty owner. Presentation imports neither protocol nor
client/runtime/workspace authority; the provenance gate enforces this direction.

- Conversation uses native transcript cursor order. Committed message identity
  wins over separately identified `attempt.in_flight`; no Session event assembler.
- The durable ledger now projects `TranscriptEntry.tool_calls`, converted into
  native `ForegroundToolExecution` records. Calls/results associate in the native
  owner across page boundaries, never in React. Current foreground facts repair
  the exact live call; canonical settled facts win. Unresolved old cache entries
  outside a fresh page force reread rather than freezing obsolete lifecycle data.
- Tool dispatch uses native ToolId. Bash, Read, Write/Edit, Glob/Grep and generic
  cards share Harness chrome. Image/file results use bounded native artifact
  reads. Diffs explicitly describe requested changes; no applied diff is invented.
- Pending interactions come only from snapshots. Composer takeover preserves its
  hidden draft. Local questionnaire pages/drafts never own interaction lifetime.
  One native response/cancel is fenced until authoritative settlement; uncertainty
  survives response loss and never causes replay.
- Composer retains native Send/Queue/Steer, IME/focus, typed commands, uploads,
  Todo/Goal/Queue and lineage. Stop acknowledgement does not settle an attempt;
  an acknowledged Stop remains fenced even if its subsequent snapshot read fails.
- Model menus and `/model` expose exact native references/defaults/advertised
  profiles. Live selection uses actual v6 `settings/setModel`; durable
  `settings/selectModel` remains a distinct revision-CAS authoring operation.
  Acknowledgement/lost-response guards fence dependent Send until authority is read.
- Permission uses existing CFG3 source-unit CAS and explicit Reload. The bounded
  native `SourceSettings.prospective_approval_mode` projection supplies desired
  policy. Loaded effective and active frozen policy remain separate native facts.
  Save is not publication; Apply saved policy is disabled while an attempt runs.
- Existing generation, AttachmentTarget/incarnation, replay-gap, late-socket and
  navigation fences remain. Disconnect does not cancel work or settle interactions.

Native changes are two read projections, with regenerated v6 TypeScript/schema and
protocol documentation. No new mutation, Harness compatibility protocol, durable
schema migration, execution controller or browser event journal was introduced.

Deleted the former Conversation, InputBar, ChatMessage, MessageItem, ToolRow,
ApprovalPanel and QuestionComposer APIs and replaced CSS. Normal dogfooding uses
the integrated Agent path. No old/new feature flag, fallback presentation mode,
compatibility export or duplicate Tool/interaction path remains.

## Deterministic contract map

| #346 scenario | Evidence |
| --- | --- |
| Cold attach, canonical transcript, live subscription | `client.test.ts`, `transcript.test.ts`; real `console.spec.ts`/`chat.spec.ts` |
| Turn admission, streaming, exact canonical reconciliation | `presentation.test.tsx`, `chat.test.tsx`, `composer-context.test.tsx` |
| Reasoning and Tool ordering without guessed process groups | `agent.test.tsx`, `chat.test.tsx` |
| Running → success/failure/cancelled/unknown Tool outcomes | `agent.test.tsx`; native cross-page Tool projection test |
| Unknown ToolId generic fallback | `agent.test.tsx` (a Tool named read with unknown identity remains generic) |
| Approval created during browser absence; one response; ack is not settlement | `agent.test.tsx`, `client.test.ts`; real `console.spec.ts` |
| Questionnaire paging, choice/multichoice/boolean/text/custom/schema validation | `questionnaire.test.ts`, `presentation.test.tsx`, `agent.spec.ts` |
| One Stop request, terminal from snapshot, no disconnect cancellation | `agent.test.tsx`, including failed authoritative reread after acknowledgement |
| Active Steer is native mailbox admission | `composer-context.test.tsx`; real `composer.spec.ts` |
| Exact model/catalog/default and advertised reasoning profiles | `agent.test.tsx`, `commands.test.tsx`; real `commands.spec.ts` |
| Desired/effective approval boundary and source CAS | `agent.test.tsx`; native `agent_permission_projection_uses_native_resolution_and_source_cas`; existing CFG3 frozen-generation/busy tests |
| Lost side-effecting response, visible uncertainty, no replay, repair fence | `agent.test.tsx`, `client.test.ts`, `commands.test.tsx`; real `recovery.spec.ts` |
| Obsolete generation/attachment/incarnation rejection, no retarget | existing `client.test.ts`, `transcript.test.ts`, `commands.test.tsx` and native App Server contracts |
| Light/dark/narrow Agent | `agent.spec.ts`, real keyboard tests at 390/820/1280/1600px |

Important mutations assert request counts and held acknowledgements/snapshot
transitions. Runtime ordering uses controlled promises and protocol fixtures, not
sleep-based proofs. The new native Tool test resolves reversed result order across
page boundaries and validates native runtime conversion.

## Visual evidence

Nine new references in `test/e2e/agent.spec.ts-snapshots` cover settled Conversation,
streaming/reasoning, running Tool, completed/error Tool with requested diff, Approval
takeover, multi-page Questionnaire, model/profile menu plus permission boundary,
dark desktop and dark narrow (390px).

All ten existing shell references were intentionally regenerated because their
center seat now contains the integrated Agent. The right-panel header now wraps
without overlap. The pinned `gradient-shadow-text.css` dependency supplies the
previously absent elevation/Markdown tokens, restoring the light Composer outline
and consistent typography. AppFrame/Sidebar/Settings architecture is unchanged;
the existing expanded mobile Sidebar still takes its established 280px seat.

Every changed/new reference was visually inspected. Initial review found and fixed
header overlap, clipped narrow controls, a stray conditional `0`, and missing light
Composer elevation before accepting the final references.

The only rendering authority was Playwright **1.63.0**, through
`scripts/browser-tests.sh`, using:

```
mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7
```

`test:e2e:update` now includes Agent, shell and foundation contracts. Semantic tests
passed before reference updates. Ordinary comparison mode uses the unchanged
zero-difference expectations. No workstation Chromium generated reference images.

## Validation commands

Commands ran in the isolated worktree on Linux. Paths below identify cwd; commands
are exact (log redirection omitted). The shared ignored Cargo target cache contains
actual binaries built from this worktree. No macOS execution is claimed.

| Cwd | Command | Result |
| --- | --- | --- |
| `web-console` | `corepack enable` | Pass |
| `web-console` | `corepack install` | Pass |
| `web-console` | `pnpm install --frozen-lockfile` | Pass |
| `web-console` | `pnpm typecheck` | Pass |
| `web-console` | `pnpm test` | Pass: 298 tests, 23 files |
| `web-console` | `pnpm check:provenance` | Pass: 100 source records, 100 production package notices |
| `web-console` | `node scripts/provenance.ts --reference /tmp/rustx-345-harness` | Pass: exact source audit |
| `web-console` | `pnpm build` | Pass; existing >500 kB chunk-size advisory |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e:update` | Pass: 3 reference/primitive contracts |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | Pass: 18 tests, zero-diff comparison |
| root | `cargo check --lib` | Pass |
| root | `cargo run --example generate_app_server_protocol` | Pass |
| `protocol/app-server` | `node generate.mjs` | Pass |
| `protocol/app-server` | `corepack enable`, `corepack install`, `pnpm install --frozen-lockfile` | Pass |
| `protocol/app-server` | `pnpm check` | Pass: normal regeneration, no drift against deliberately staged generated artifacts |
| `protocol/app-server` | `pnpm typecheck` | Pass: Rust fixtures and reference intersections |
| root | `cargo build --bins` | Pass: real App Server/native supervisors |
| root | `cargo fmt --all` | Pass |
| root | `cargo fmt --all -- --check` | Pass |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | Pass: 2,837; 1 ignored; bin/example harnesses 0 tests |
| root | `cargo test --test contracts --test provider --all-features` | Pass: 27 + 166; 5 ignored live-provider tests |
| root | `cargo test --lib --all-features -- boundary_suites::` | Pass: 226 |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Pass on final default-concurrency rerun: 420; initial hang described below |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=1` | Pass: 420 total (29 catalog, 5 managed output, 23 conformance, 128 durable, 52 process, 53 subagent, 130 tools) |
| `tui` | `corepack install`, `pnpm install --frozen-lockfile` | Pass |
| `tui` | `pnpm typecheck` | Pass |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass: 787 tests, 94 suites |
| `dev` | `corepack install`, `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test` | Pass: 30 tests |
| `test-support/fake-provider` | `uv sync --frozen` | Pass |
| `test-support/fake-provider` | `uv run --frozen pytest` | Pass: 51 |
| root | `git diff --check` | Pass |

During implementation, typecheck caught a narrowed mutable client field and an
invalid test outcome shape; Clippy caught Rust constructor field ordering. Those
were corrected. Initial browser failures referenced deleted toolbar CSS or assumed
always-expanded Tool attachments; tests now use labelled Session metadata and
explicit Harness disclosure. The full real-server suite subsequently passed.

The first parallel external-boundary run completed six targets but hung in
`uv::production_uv_materializes_a_managed_package_environment` with an unreaped
supervisor child. It was terminated after inspection. A complete serial rerun
passed all 420 tests, including that test; no unrelated runtime workaround was
added. The final default-concurrency rerun also passed all 420 tests (including all
130 Tool tests). No check remains blocked by that initial hang.

## Deliberate limits

No trustworthy native turn/process grouping or foreground subcall relation is
exposed, so there is no inferred folding or nesting. Harness Host file-opening,
terminal/search rich metadata and permission presets are not imported. Native
text/JSON/artifact output and requested diffs provide the supported truthful subset.
No legacy Agent implementation remains. All relevant Linux checks are run; macOS
platform execution remains the existing CI lane's responsibility.
