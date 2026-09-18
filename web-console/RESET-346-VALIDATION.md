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
  owner through persisted Assistant MessageId/block-index associations across page
  boundaries, never in React. Current foreground facts update only that occurrence;
  canonical settled facts win. Unresolved old cache entries
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

Native changes are two read projections and a canonical Tool occurrence/result
index in SQLite schema 39, with regenerated v6 TypeScript/schema and protocol
documentation. Obsolete development stores are refused. No new mutation, Harness
compatibility protocol, migration, execution controller or browser event journal
was introduced. The PR #349 review correction is recorded below.

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

## Initial PR validation commands

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

## PR #349 Tool identity review correction (2026-09-18)

Preflight fetched the existing PR branch at
`aa02e87b3990bd4517fe1311b06b77de03e2a33e`; no subsequent commits needed preserving.
A fresh pre-push fetch found the same PR head and the same main/base
`204f7ccc8fbaf4bc1b6842e02e8d0d68f19d5837`. Work remains on the original isolated
issue branch; #349 is the only open PR for this work. The original checkout is clean.

### Ownership and bounded reads

**A historical Tool result is associated with the exact canonical Tool-call
occurrence that owns it, not with a Conversation-global ToolCallId lookup.**

`src/runtime/recovery.rs` already states that “the durable authority does not
guarantee ToolCallId uniqueness across the whole conversation lifetime”; recovery
keys execution evidence by owning Attempt plus call ID. That invariant is unchanged.
The old transcript JSON lookup and browser foreground match used only call/Tool IDs,
allowing an old result to settle a new Attempt's reused provider ID.

SQLite schema 39 persists `canonical_tool_calls`, whose primary key is canonical
`(assistant_message_id, block_index)`. The Assistant commit inserts its occurrences;
the Tool-result commit atomically links its canonical result MessageId. The existing
publication owner resolves the exact Assistant through Attempt/turn and call ID.
Direct canonical commits use their native active Surface; lineage initialization
uses its retained Surface history at each original transition, including retired
spans. Ambiguous ownership is refused. Results and lifecycle bodies remain in the
canonical ledger, not duplicated in the index.

The primary-key index supplies an Assistant's ordered occurrence range;
`message_ledger(message_id)` supplies each linked result body. The store also has
unique Assistant/call and result-message constraints and the native
`canonical_tool_calls_by_call(call_id, tool_id, assistant_message_id)` write-side
index. A page uses O(page rows + required associations) indexed seeks rather than
one full-history JSON scan per Tool. The query-plan regression proves occurrence
and result-index SEARCH operations with no SCAN or temporary sort. Normal transcript
reads never materialize the full ledger. Schema 38 is refused without migration,
backfill or an alternate lookup path.

`RuntimeClientTranscriptEntry.tool_calls` and live `attempt.foreground` share
`ForegroundToolExecution { message_id, block_index, call_id, tool_id, name, state }`.
The two occurrence fields are required in regenerated v6 artifacts. Live identity
comes from native publication/canonical bootstrap. The Web adapter matches that
exact occurrence (and checks call/Tool IDs); a settled canonical result wins over a
lagging live observation of that same occurrence. No historical call/result pairing
or browser identity construction was added. TUI incremental events preserve the
same native identity and cannot create it from execution events alone.

### Regression evidence

| Requirement | Deterministic evidence | Result |
| --- | --- | --- |
| Reused `call-1` / `tool-bash` across Attempts A and B | `interaction_audit::transcript_tool_occurrences_do_not_alias_across_attempts_and_reopen` uses distinct native publication generations | Pass |
| A settled, B unresolved | B has no result after A commits `old-result`; lineage/native Runtime Client conversion asserts exactly `assembled` and no old result | Pass |
| Both settled | A retains `old-result`, B receives only `new-result` | Pass |
| Physical settlement order | The duplicate-ID test runs A→B and B→A settlement, with exact ownership in both orders | Pass |
| Cross-page resolution | One-row Assistant pages resolve results outside their page; existing two-call reversed-order test also passes | Pass |
| Reopen | Drop the store and reopen the SQLite file before rereading both occurrences | Pass |
| Lineage/retired Surface | Seed replays A's retirement, retains A's result and leaves reused B assembled until its own commit | Pass |
| Web running versus old result | `agent.test.tsx`: one row at B, running, no old result; A retains its old result; B's own canonical settlement wins over lagging live state; an unresolved A cannot borrow B's live state | Pass |
| Query bounds and obsolete schema | SQLite EXPLAIN index checks and explicit schema-38 refusal without creating/backfilling the new table | Pass |
| Live identity | Runtime Client reversed-completion test preserves native block positions; TUI requires native call occurrence and preserves it through execution | Pass |

### Validation after correction

Exact commands below ran in the same worktree; shell log redirection is omitted.
The affected tests ran first, followed by all current Linux CI lanes. Intermediate
compile/lint failures, an obsolete schema-version assertion and TUI fixtures without
publication identity were corrected before the final successful runs.

| Cwd | Command | Result |
| --- | --- | --- |
| root | `cargo check --lib` | Pass |
| root | `cargo test --test durable --all-features transcript_tool_occurrences` | Pass: 1 duplicate-ID regression, both settlement orders and reopen |
| root | `cargo test --lib --all-features transcript_tool` | Pass: 4; subsequent full unit run includes stronger assembled-state assertion |
| root | `cargo fmt --all` | Pass |
| root | `cargo fmt --all -- --check` | Pass |
| root | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| root | `cargo build --bins` | Pass |
| root | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | Pass: 2,840; 1 ignored; bin/example harnesses 0 tests |
| root | `cargo test --test contracts --test provider --all-features` | Pass: 27 + 166; 5 ignored live-provider tests |
| root | `cargo test --lib --all-features -- boundary_suites::` | Pass: 226 |
| root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Pass: 421 (29 + 5 + 23 + 129 + 52 + 53 + 130), default concurrency |
| root | `cargo run --example generate_app_server_protocol` | Pass |
| `protocol/app-server` | `node generate.mjs` | Pass |
| root | `pnpm --dir protocol/app-server check` | Pass: normal generator, no drift from staged artifacts |
| root | `pnpm --dir protocol/app-server typecheck` | Pass |
| root | `pnpm --dir web-console typecheck` | Pass |
| root | `pnpm --dir web-console test` | Pass: 299 tests / 23 files |
| `web-console` | `pnpm check:provenance` | Pass: 100 source records and 100 production package notices |
| `web-console` | `pnpm build` | Pass; existing chunk-size advisory |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | Pass: 18, unchanged zero-diff screenshot comparisons |
| `tui` | `pnpm typecheck` | Pass |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass: 788 tests / 94 suites |
| `dev` | `pnpm typecheck` | Pass |
| `dev` | `pnpm test` | Pass: 30 |
| `test-support/fake-provider` | `uv run --frozen pytest` | Pass: 51 |
| root | `git diff --check` | Pass |

No screenshot was regenerated or modified for this correction. Comparison used the
same Playwright 1.63.0 immutable Noble image recorded above, through
`scripts/browser-tests.sh`. Harness-derived presentation, source closure, licensing,
model/permission behavior, interactions, cancellation and reconnect ownership are
unchanged. There is no legacy Tool-association or Agent presentation mode. No local
check is blocked; macOS execution remains CI's responsibility.
