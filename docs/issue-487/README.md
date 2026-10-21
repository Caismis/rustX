# Issue #487 — layering refactor: requirement matrix

Starting base: `origin/main` at `6771928953a9b2815a27fd784f6d39c5ccde8b56`
(identical to the audit snapshot, so every audited finding applied).
Integrated main: `7cf6b2da6b360bb759ba82de5367ea1bb5d310f4` (PR #488, on top
of `0f3d978e43e2174d5b26feff6bccefc1bf25f71d`, PR #486). Both were ported into
the refactored layout; see "Integration with main" below.

Status: **done** = implemented with the evidence named; **resolved on base**
= already true at the base. No requirement is deferred.

Sizes: [`measure.py`](measure.py) (method in its docstring) and the generated
[size report](size-report.md). Shared-contract path moves:
[contract-moves.md](contract-moves.md).

## A. Configuration and Runtime contracts

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| A1 authored/source/provenance vs prepared bindings and adoption | `RuntimeConfiguration` carried component/source revisions, redacted document and provenance; the native runtime redacted and copied them | `runtime::resources::RuntimeConfiguration` (mixed) → execution bindings stay in `RuntimeConfiguration`; authored record is `local_runtime::configuration::generation::AuthoredConfiguration`, carried inert as `authored` | runtime no longer calls `redact` or reads captures; Context/Provider/policy units arrive as a native `ContextBinding` or a closure over the base generation selected under the runtime lock | `src/runtime/adoption.rs`, `src/local_runtime/configuration/generation.rs`; `runtime::conversation_runtime::` 92 tests | done |
| A2 ownership of prepared configuration, adoption eligibility, resource loader | `PreparedConfiguration`, `AdoptionEligibility`, `AdoptionError` in `local_runtime::configuration::application`; `AdmittedConfiguration` in `settings`; native `RuntimeResourceLoader` trait with filesystem and frozen-subagent impls | → `crate::runtime::adoption`; loader → application type `LocalRuntimeResourceLoader` owned by `LocalConversationCore` | removed `RuntimeResourceLoader`, `FilesystemRuntimeResourceLoader`, `FrozenSubagentResourceLoader`, `configuration_view()` (replaced by `EffectiveConfiguration::adopted`); native `local_runtime` references 54 → 9 (resolved config document and inert authored record, plus 3 test-only) | `SessionRuntimeManager::configuration_preparation` reads runtime + loader under one composition lock; stale-candidate rejection unchanged (`prepare_configuration` reads the binding baseline first) | done |
| A3 one cache-impact policy | three copies in `ConversationRuntime` plus two `== Preserved` checks in the worker | → `runtime::adoption::configuration_cache_impact` + `PreparedConfiguration::adopts_implicitly` | duplicates removed | `src/runtime/adoption.rs` | done |
| A4 configuration application split | `local_runtime/configuration.rs` 2,004 production lines | → root (193) + `capture`, `prospective`, `units`, `semantics`, `generation`; `application` stays the single publication worker | — | size report | done |
| A5 stale-owner/revision, lock order, atomic adoption, model selection, frozen snapshots | — | unchanged owners | no new locks or tasks | configuration and residency suites (`local_runtime::session_runtime_manager::tests::`), `--test cfg3_catalog`, `--test cfg3_managed_output` | done |

## B. Native observation/read contracts and Runtime Client

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| B1 shared bounded contracts at a native boundary | ~600 native references into `runtime_client::{content, snapshot, checkpoint::bounded_*, trace, response, structural_json, …}` | → new crate-level `crate::observation` (`bounded`, `content`, `cut`, `capture`, `transcript`, `tool`, `interaction`, `status`, `turn`, `statistics`, `occupancy`, `delivery`, `goal`, `head`, `failure`, `trace::{types,bounds,content,frozen,request,generation}`, `capability_source`, `resource_source`, `workflow_source`, `preflight`, `structural_json`) | one definition per type, no re-export at the old path; mapping in [contract-moves.md](contract-moves.md) | native production code in `durable`, `runtime`, `agent`, `context`, `tools`, `capabilities`, `observation` has 0 `runtime_client` references (remaining hits are `#[cfg(test)]` consumers) | done |
| B2 no native call into client display shaping | `conversation_runtime` and `durable/sqlite/observation_receipt` called `runtime_client::projection::prepared_status*`; `terminal_turn` built failures through the projection | prepared status is written into the request source encoding at commit, so it is a source-prepared descriptor → `observation::status`; failures → `observation::failure`; Tool descriptor preparation returns its own `ToolTerminalUnavailable` | dropped `runtime_client::response::TurnProcessOutcome` re-export and root re-exports of moved names | commit `6a441ccf` | done |
| B3 presentation stays in the adapter | — | linearization, checkpoint/page assembly, cursors, subscriptions, `TraceProjection`, `*_observation` readers, host, endpoint stay in `runtime_client` | — | module map in `docs/architecture.md` | done |
| B4 projection/host split | `projection.rs` 6,833 and `host.rs` 5,529 production lines | `projection/`: lifecycle, observe, fold, journal, canonical, publication, subscription, read, read_domains, checkpoint_seed, in_flight, views; `host/`: construction, bootstrap, durable_bootstrap, worker, sources, publication, read, checkpoint, history, agents, subscription, control | entry files 368 / 753; largest child `host/read.rs` 1,211 | `runtime_client::` tests | done |
| B5 bounded descriptors, exact cuts, content identity | — | unchanged | no new rereads; `include!` Trace tests became modules | `runtime_client::response::`, `durable::sqlite::observation` tests | done |

## C. SQLite and Store

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| C1 SQLite by domain | `sqlite.rs` 13,417 production lines, 22 `include!` fragments | `SqliteConversationStore` stays the one connection/transaction owner in `sqlite.rs` (736); domains in `sqlite/`: `schema` (+`shape`), `inbox`, `canonical` (+`ledger`, `transcript`), `request` (+`snapshot`, `contribution`), `journal` (+`reference`), `publication` (+`proposal`), `projection/store`, `observation/store` + `observation/index`, `auxiliary` (+9 children, from main) | no `include!` remains under `src/durable` | 395 `durable::sqlite::` tests before and after the split, identical names | done |
| C2 one read-cut transaction helper | the lock / read transaction / cut validation / query sequence repeated across observation reads | → `read_at_cut` (10 reads) and `read_transaction` (26 reads) | 13 reads that validate the cut on the bare connection stay there on purpose: a transaction would arm the append-validation rollback hook on every read and add rereads on the next append | `durable::sqlite::observation` tests | done |
| C3 narrower Store interfaces | one 202-method `ConversationStore` | union of `InboxStore` 12, `CanonicalStore` 32, `RequestStore` 10, `JournalStore` 10, `PublicationStore` 7, `ProjectionStore` 22, `ObservationStore` 106, `AuxiliaryStore` 14, plus 4 own methods | 11 single-domain consumers now take their domain trait (`ObservationStore`, `RequestStore`, `JournalStore`, `CanonicalStore`); `ObservationStore` stays whole because its runtime_client readers use its methods together | commit `6d627dd4`; `src/durable/store.rs` | done |
| C4 one transaction owner per semantic operation | — | domain helpers take the caller's `Connection`/`Transaction`; compound operations (canonical publication, model-turn start, subagent terminal acceptance, auxiliary settlement) stay one method in one transaction | SQL unchanged | rollback/corruption suites | done |
| C5 tests with their domains | inline tests and fragments in `sqlite.rs` | child test modules beside each domain, shared `sqlite::test_support` | rollback, corruption, read-cut, lineage, publication coverage kept | test inventory unchanged | done |

## D. Native Runtime, execution, Subagent, Session

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| D1 ConversationRuntime split | 7,305 production + 12,561 test lines in one file | one owner and one coordinator lock; `impl` blocks in `bootstrap`, `admission`, `attempt`, `child`, `compaction`, `drain`, `durability`, `configuration`, `model_selection`, `controls`, `history`, `observer`, `probe` | entry 1,079; tests in `<domain>/tests.rs`; two `include!` fragments became members | 92 `runtime::conversation_runtime::` tests keep the prefix (FAST requirement) | done |
| D2 AgentExecution split | 5,783 production lines | `AgentExecution` and its run loop stay the owner; `impl` blocks in `construction`, `step`, `request`, `start`, `stream`, `recovery`, `compaction`, `tools`, `publication`, `terminal`, `test_sync` | entry 452; largest `tools.rs` 1,122 | `agent::`, `scripted_suites::` | done |
| D3 state aggregates only on real lifetimes | — | `LogicalStep` (reset per logical step; retries reuse the latest frozen input), `RequestMeasurements` (provider-observed input that outlives a request; `withdraw()` where fields were cleared), `ExecutionPauses` (test-only) | pending continuation / owner / accepted source kept as separate fields so pending-before-commit and accepted-after-commit staging is unchanged; `SemanticGenerationRecovery::reset` removed (unused) | commit `a40199da` | done |
| D4 Subagent registry, Session, composition, manager | registry 4,522; session 3,400; composition 2,113; manager 2,078 | each owner type keeps its state and mutex; responsibilities in `registry/` (preparation, ownership_commit, control, drain, settlement, interactions, observation), `session/` (creation, lineage, metadata, projection, catalog_commit, invalidations), `composition/` (resources, subagent_child, runtimes), `session_runtime_manager/` (client, lifecycle, drain, sessions, model_selection, configuration) | entries 686 / 889 / 911 / 522 | `runtime::subagent::`, `local_runtime::session::`, `--test subagent` | done |
| D5 registration, drain, frozen input, settlement, ordering, terminal uniqueness | — | unchanged; terminal construction and `emit_terminal` now in one `terminal` module | no new executor, scheduler, locks, tasks or Drop cleanup | scripted execution/cancellation suites | done |

## E. Shared Web/TUI semantics

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| E1 one native event fold | TUI folded each `projection_transition` batch per inner event and again as a whole, folded content advances twice, and ran its own fold that the native result then overwrote | `protocol/app-server/projection.ts` `foldRuntimeEvent`, called once per event/batch | removed `reduceOrdinaryState` and helpers; TUI keeps an explicit transcript-display adapter | `tui/test/projection.test.ts` | done |
| E2 exact numbers | Web used `Number(text)` (rounded `9007199254740993`, accepted `0x10`, `Infinity`); TUI integer input sent `007`, `-0` that native `ExactInteger::parse` refuses | → `protocol/app-server/number.ts` (binary64 wire codec, exact draft reading, native-aligned integer reading, machine-readable reasons) | removed `tui/src/protocol/number.ts`, Web `finiteNumber`/`displayNumber` | regressions for all five required inputs in both consumers | done |
| E3 one exhaustive method policy | TUI `METHOD_RESPONSE_LOSS_CLASS` + lane switch; Web `READS`, lane switch, `longWait` | → `protocol/app-server/method-policy.ts` (`Record<MethodName, …>`, so a new generated method fails to typecheck until classified), incl. `goal/control` `action=show` | Web now treats `delivery/cancel`, `session/subscribe`, `session/exportPrepare` as connection-local like the TUI (they commit only connection-scoped state, which a lost response retires with the connection) | Web/TUI client tests | done |
| E4 transports keep their queues | — | each transport keeps its own queue, backpressure and lifetime code | — | — | done |
| E5 Source repair rules, attachment identity | copies in TUI host, client buffer, status line and Web client | → `protocol/app-server/source-repair.ts`, `attachment-identity.ts` | duplicates removed; notices/acknowledgement stay with each UI | — | done |
| E6 `AppServerSession.updateState` | production mutation entry used only by tests | removed; the four tests drive real `session/event` notifications | — | `tui/test/*` | done |

## F. Web/TUI and App Server controllers

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| F1 Web client split | `app-server.ts` 2,584 lines | root (890) keeps connection lifecycle, the one published state and Session lifecycle projection; collaborators with narrow interfaces: `contracts`, `errors`, `request-pipeline`, `source-repairs`, `session-catalog`, `session-reads` (`client.reads`), `configuration-observations`, `reading`, `cold-reads`, `trace-reads`, `session-controls`, `model-selection`, `interactions`, `session-uploads` | XState ownership unchanged | `web-console` typecheck/test/build | done |
| F2 client ↔ first-submit cycle | client imported `first-submit.ts` | application layer owns `firstSubmissions(client)` over new `onAuthorityRetired`/`onDispose` hooks; shared errors in `client/errors.ts` | cycle removed | `check:provenance` | done |
| F3 established seams, no forwarding context | — | collaborators receive only what they read/write | — | — | done |
| F4 TUI app split | `ui/app.ts` 2,376 | root (1,509) keeps global focus/exit, Session binding, draft/submit, the presentation lease; `presentation-lease`, `human-input`, `selectors`, `conversation-view` | 1,509 is just over the ~1,500 guideline: further splitting would separate run-loop key routing from Session binding, which share the app state | `tui` tests | done |
| F5 dispatcher helpers, Session readers | dispatcher 1,464; session 1,296 | `commands/outcome.ts`, `commands/presentation.ts`; `app-server/session-readers.ts` (epoch-fenced bounded reads); `questionnaire-answers.ts`; shared `diagnostic-text.ts` | entries 831 / 886 / 861 | `tui` tests | done |
| F6 App Server handler families | `connection.rs` 3,240 | `connection.rs` (974) keeps decoding, admission, runtime-operation admission under the route lock, attach reservation/commit, release, close, delivery registration and `Reply::commit`; `connection/{family, catalog, history, configuration, runtime, notifications}`; one error mapping in `app_server/rpc_error.rs` | `Family::of` replaces `runtime_target` with an exhaustive match | 4,252 lib tests before and after | done |

## G. MCP

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| G1 generation/lease/retirement | in `mod.rs` (5,676 production) | `McpServerRuntime` stays the owner; `generation.rs` | — | `tools::mcp::` | done |
| G2 progress | — | `progress.rs` with its tests | — | `progress::tests` | done |
| G3 invocation/outcome/result | — | `invocation.rs`, `request.rs`, `outcome.rs`, `task_driver.rs`, `result.rs`, `executor.rs`, `connect.rs`, `runtime.rs`, `handler.rs` | entry 430; largest new 740 | — | done |
| G4 one tools-list-changed path | modern subscription loop and legacy callback each took the gate, checked closed, advanced the epoch, notified | → `invalidation::ToolListChangedSink` (`admit` → `invalidate`) fed by both entry points | duplicate removed | `invalidation::tests` (admission once; closed runtime refuses; close waits for an admitted notification) | done |
| G5 `dispatch_owned_request` | — | unchanged single path in `request.rs` for tool calls and Task-control requests | protocol versions, negotiation, MRTR/Tasks, HTTP ownership, progress lifetimes unchanged | `boundary_suites::mcp`, `--test tools` | done |

## H. Obsolete entry points and tests

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| H1 history façades | `transcript_history`, `historical_surface_snapshot`, `historical_surface_history`, `historical_user_message_boundaries`, `historical_user_message_boundaries_page`, `historical_head_revision` had no callers | — | removed with their exports | repository-wide caller check | done |
| H2 `runtime/transcript_history.rs` | only the façade | — | file removed; `tests/durable/transcript_history.rs` kept (tests the Store's `load_transcript_page`) | `--test durable` | done |
| H3 App Server test split | `protocol.rs` 10,375; `configuration.rs` 9,902; `mod.rs` 2,334 | `protocol/{admission, attachment, control, history, delivery, upload, configuration, model_catalog, catalog, projection}`; `configuration/{sources, adoption, preparation, available, desired_sources, session_creation, source_documents, catalog_publication, catalog_recovery, catalog_agents, catalog_generations}`; fixture in `fixture.rs`; residency tests in `load`, `replacement`, `deletion`, `session_claim` | parents 975 / 385 / 166 | test inventory identical | done |
| H4 inline tests with their domain | — | every split moved its tests to child `tests` modules; no `include!` test fragments remain in the touched areas | no production visibility widened for tests | — | done |
| H5 shared constructors/factories | duplicated `selection()`, `assert_authority`, `gate_entered`, tool-call SSE factories, request/runtime helpers, UUID-sequence fixtures | → existing support (`runtime_client_conformance::text`, `scripted_suites::common`, fixture helpers) | duplicates removed | — | done |
| H6 session-count controls | fixtures prepared two Sessions by default | 160 tests ask `Fixture::with_session_count` for what they use (147 one, 13 none) | multi-Session tests unchanged | — | done |
| H7 discovery and module paths | — | CI selector prefixes unchanged; self-re-running tests updated to new paths | — | `check_test_lanes` for rust-fast, rust-contracts, rust-boundaries, documentation-contracts | done |

## I. Documentation, structural checks, CI selection

| Req | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| I1 documentation-contract selection | `DOCUMENTATION` paths had no surface, so a docs-only PR merged on FAST without running the `contracts` target that reads them | `verification.py` | `documentation` surface + FAST `documentation-contracts` job (FULL keeps it in rust-contracts) | `test_verification.py`; lane check for the new job | done |
| I2 structural tests and lane rules | — | selectors and lane rules unchanged in prefix; test paths that tests re-run by name updated | — | lane checks | done |
| I3 contract-focused ownership checks | — | the method policy is an exhaustive typed record; `Family::of` is an exhaustive match; Web `check:provenance` keeps the layer rules | — | — | done |
| I4 fast/full CI design | — | unchanged; one light FAST job added | — | `test_verification.py` | done |
| I5 docs | — | `docs/architecture.md` module maps (Store, SQLite, ConversationRuntime, Runtime Client, observation, MCP, App Server, Web/TUI), `docs/runtime-resources.md`, `docs/app-server-protocol.md`, `docs/invariants.md`, `tests/README.md` | stale paths removed | `cargo test --test contracts` | done |
| I6 generated artifacts | — | wire type descriptions kept unchanged, so no regeneration was needed | — | `pnpm --dir protocol/app-server check` (no drift); `committed_rust_artifacts_are_current` | done |

## Integration with main

### `0f3d978e` (#486)

Main added auxiliary request ownership (#486) while this branch was in
progress. Its changes were ported hunk by hunk into the refactored layout:

- new Store methods → `RequestStore::load_request_owner` and a new
  `AuxiliaryStore` domain contract (in the `ConversationStore` union);
- `sqlite/auxiliary.rs` (4,355 lines as merged) → split by responsibility into
  `schema`, `validation`, `authority`, `rows`, `index`, `audit`, `commit`,
  `confirm`, `read` (largest 736), helpers still inside the caller's transaction;
- its `include!`d SQLite tests → `sqlite/auxiliary/tests/*`,
  `sqlite/request/tests/owner.rs`, `sqlite/observation/tests/trace/auxiliary.rs`;
- execution, projection, host, session, schema, journal and request hunks were
  applied at their moved locations.

### `7cf6b2da` (#488)

Merged with 63 conflicting paths. Each conflict was resolved by tracing main's
diff against `0f3d978e` and applying it at the code's new owner; no old
monolith was restored.

| Main change | Final owner in this layout |
| --- | --- |
| App Server v51 handshake, generated `v51.*`, subprotocol | native unchanged from main; Web `client/app-server.ts`; every extracted Web/TUI module and test import (`contracts.ts`, `method-policy.ts`, `session-uploads.ts`, `session-reads.ts`, TUI `session-readers.ts`, …) moved to `v51` |
| one file per `session/uploadPrepare`; `UploadPolicy` file/transfer limits removed | `client/session-uploads.ts` (exactly one file per operation, tuple request); multi-file selections stay per-file operations in `first-submit.ts` / `uploads.ts` with their own operation identity, receipt and uncertain-outcome repair; TUI `session.ts` already sent one file |
| multi-page reading windows, initial-window policy, exact repair of rows that leave the live head, frozen vs live continuation | `client/reading.ts` (`readHistoryPage` keeps the first page's cut and strict cursor progress, `loadInitialHistory`, `refreshTranscriptDomain`, `returnToLatest`); triggered from the root observation path, attach and connection loss in `client/app-server.ts` |
| Trace: newer-head guard, multi-page older reads, same-epoch detail retention, independent pending-read token | `client/trace-reads.ts` (`refreshDomain`, `loadInitialTrace`, `loadEarlierTrace`, `loadTraceDetail`); epoch-preserving refresh in the root observation path; shared state in `client/trace.ts` |
| publication heads on the Session view | `client/contracts.ts` `SessionView.publicationHeads`, folded in the root observation path |
| developer protocol log and Inspector removed | removed from `request-pipeline.ts` and the client root; its tests dropped with it |
| `selectCatalogModel` removed from the Web client | removed from `client/model-selection.ts` |
| source-prepared `transcript_anchor` per activation | `ObservationStore::observation_projection_activation`; SQLite reader in `sqlite/observation/store.rs` over `observation_projection::read_id`; field on `observation::head::RuntimeClientProjectionHeadView`; history checkpoint views (`projection/views.rs::checkpoint_view`) read the original activation descriptor |
| Workflow child inspection without durable Agent authority | `runtime_client/host/agents.rs` (`child_view`, Workflow-child `agent_status`, inspection-only stores); App Server `connection/runtime.rs` reads `agent_status` natively; mutations still require the Registry's Agent record |
| child live → retained handoff authorized by Registry terminal state, shared deadline, same activation | `runtime_client/agent_observation.rs`, `agent_trace.rs` (moved types stay under `crate::observation`) |
| child input admission: text budget kept, attachment count limit removed | `registry/control.rs`, `registry/preparation.rs` (`valid_input`, error text) |
| upstream tests added to files this branch had split or removed | `workflow_child_inspection_does_not_grant_durable_agent_controls` → `registry/tests/settlement.rs`; `abandoned_workflow_publication_keeps_inspection_on_retained_history` (from the removed `workflow_recovery_tests.rs`) → `conversation_runtime/bootstrap/tests.rs`; anchor tests → `sqlite/observation/tests/projection.rs`; protocol upload/delivery hunks → `tests/scripted/app_server/protocol/{upload,delivery}.rs` |

Review follow-up: `observation_projection_activation` reads its one bounded
SELECT on the locked connection, as on main. Wrapping it in `read_transaction`
rolled back a read transaction, whose rollback hook retired the committed
append certificate and forced the next append into cold validation.
`activation_descriptor_read_keeps_the_append_validation_certificate` records
`(cold, warm) = (1, 0)` with the transactional reader and `(0, 1)` after the fix.

Upstream files written against the old monolithic client were adapted to the
extracted API (`client.reads.readContent`, `client.reads.modelRead`,
`client.reads.domainReader`, contract/error imports from `contracts.ts` /
`errors.ts`). Five tracked `*.orig` merge backups left by the first
integration were removed.

## Production modules above 2,000 lines

None of the audited hotspots or their resulting modules exceeds 2,000
production lines. The modules above 2,000 are outside the audit and unchanged
here apart from imports, each one cohesive owner: `runtime/workflow.rs`
(Workflow program runtime), `tools/background.rs` (background job supervisor),
`capabilities/coordinator.rs` (capability snapshot coordinator),
`runtime/recovery.rs` (startup recovery), `runtime/subagent/process.rs`
(child process owner), `model/catalog.rs` (model catalog),
`tools/native/bash/supervisor.rs` (bash supervisor).

## Known limitations

- Types moved into `crate::observation` keep their `RuntimeClient*` names so
  every move stayed a pure path change; renaming is cosmetic.
- `runtime_client/projection/tests.rs` and `host/tests.rs` were moved whole
  beside their owners rather than split per child module.

## Verification

Run locally on `4cf10216` (merge of main `7cf6b2da` plus documentation) with
the CI commands of the FULL lanes and the client packages. Results from the
earlier head (`74f17445`) are superseded.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| planner and benchmark-controller Python tests; provider emulator `uv sync --frozen` + `pytest` | pass |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 4,150 passed, 1 failed (see below) |
| `cargo test --test contracts --test provider --all-features` | pass |
| `cargo test --lib --all-features -- boundary_suites::` (emulator required) | first run under machine load: 7 failed (6 at their 30 s liveness guard, one `native canonical cut provenance unavailable` read that passes alone in 5.6 s on both branch and main); rerun: 247 passed |
| external targets `durable process subagent tools conformance cfg3_catalog cfg3_managed_output` | pass |
| `check_test_lanes` for rust-fast, rust-contracts, rust-boundaries, documentation-contracts | pass |
| `generate_schemas` + `git diff --exit-code -- schemas`; `pnpm --dir protocol/app-server check` and `typecheck` | pass (no drift) |
| `pnpm --dir tui typecheck` / `test` (real binary, emulator required) | pass (1,186 passed, 1 skipped) |
| `pnpm --dir web-console typecheck` / `test` / `check:i18n` / `check:provenance` / `build` | pass |
| `pnpm --dir dev typecheck` / `test` | pass |
| Playwright browser acceptance | not run locally (the pinned browser container image is not present); runs in CI |

Focused regressions for the #488 integration (all pass): the Web history,
continuation, Trace and upload suites (`transcript-refresh`,
`transcript-deferred-history`, `history-policy`, `agent-reading`,
`chat-block-continuation`, `transcript-continuation`, `trace-live-inspector`,
`trace-cache`, `trace-deferred-detail`, `upload-carrier`, `upload-dispatch`);
native `compaction_marker_keeps_its_original_transcript_boundary_as_history_grows`,
`inherited_compaction_marker_uses_the_destination_transcript_boundary`,
`workflow_child_inspection_does_not_grant_durable_agent_controls`,
`abandoned_workflow_publication_keeps_inspection_on_retained_history`,
`native_view_contains_only_the_selected_path_in_start_order` and the
`runtime::workflow::tests` order/cancellation/loop suites, the three
`input_admission` tests (32 attachments, oversized text rejected without
effects), and `protocol::{upload, delivery}` (40 tests).

The one `--skip boundary_suites::` failure is
`scripted_suites::agent::tool_progress::parallel_calls_bound_progress_per_call_and_keep_canonical_order`
hitting its 10 s wall-clock guard while sharing the machine with the rest of
the suite. Comparable baseline, `cargo test --lib --all-features --
scripted_suites::` on main `7cf6b2da` versus the branch, same machine: main
failed in 3 of 3 runs (once with this test alongside
`stream_progress::large_write_progress_crosses_idle_timeout_without_retry_or_duplicate_execution`
and `tool_deadlines::issue206_executor_panic_cannot_orphan_parallel_siblings`);
the branch passed 2 of 2. The upstream test
`protocol::upload::native_user_input_receipts_allow_large_collections_and_preserve_authority`
likewise varies alone from 3 s to 12 s on the branch and from 13 s to a 30 s
liveness failure on main. These are load-sensitive wall-clock guards present
on main; no assertion was weakened.

No runtime speed change is claimed.
