# PR #477 architecture review repairs

This record covers successive reviews of `issue-466-context-projection` in the
existing dedicated worktree. The R1–R5 review began at
`bd89fe71c2fd399e6a3a7c2fbae172ea5e814006`, with main
`6c73e456e826024fdbe153198a48c9128142e115` already integrated. The final F1/F2
review began at `e87867ebf3166af0111d23935654f7c1a40773e6`; its evidence and
subsequent latest-main integration are recorded below. Other checkouts and
worktrees are preserved throughout.

## Findings and owning boundaries

| Finding | Confirmed counterexample | Correction |
| --- | --- | --- |
| R1 | A checkpoint based at L100 and activated at L110 became active in a fork through L104. | Source selection and the lineage remapper require activation eligibility as well as base eligibility. Installation checks checkpoint identity, predecessor order, monotone activation Ledger, and base <= activation <= destination Ledger. |
| R2 | Stored Responses continuation rejected a prefix changed by the seventeenth image. | Agent materializes semantic transformations, reconciles exact covered input, and validates complete replay through the adapter before freezing or starting when coverage changes. Incompatible native replay is Runtime Unsupported with an explicit native-input compatibility diagnostic. |
| R3 | A dependency-free Derived candidate claiming future Ledger 100 committed against Ledger 1. | The shared durable Resource Read Cut validator runs unconditionally during candidate dependency validation, inside the checkpoint transaction after matching operation-receipt lookup. |
| R4 | Thirty-three individually bounded 512 KiB sources bypassed the 16 MiB cumulative limit. | Canonical and parent-checkpoint metadata reads share remaining bytes/items. Raw size is checked before body decoding; each canonical body is reduced immediately to physical metadata. Parent sources are deduplicated before reading; only trusted metadata fields are decoded. |
| R5 | Later large checkpoint bodies exhausted an early fork's budget. | Native timeline SQL selects Ledger/Journal/activation eligibility before candidate hydration. Ordinary message-boundary Fork and explicit activation Fork both supply the selection at the native read boundary. |

The continuation source still comes exclusively from actual accepted Assistant
output. Reconciliation validates that mapping before changing the active pointer;
it never edits canonical opaque units. Complete signed/encrypted replay is validated
by the existing protocol translators without exposing SDK or wire types to the Agent.
Snapshot reconstruction and ordinary retries do not invoke reconciliation again.
They retain the materialized request, continuation decision, media, and semantic
fingerprint of the latest frozen logical input version.

Historical activation IDs remain separate from checkpoint base frontiers and source
Journal provenance. Source origin is unchanged during installation. Destination
canonical references, checkpoints, occurrence bindings, fingerprints and Artifacts
remain owned by the existing lineage closure. Missing predecessors/provenance and
corrupted eligibility coordinates fail explicitly. Later ineligible candidate bodies
consume no historical materialization allowance. No inherited execution facts are
created.

Checkpoint content, activation/head, metadata, completion and receipt still share
one SQLite transaction. COMMIT remains the durable linearization point. Historical
cuts do not require equality with current frontiers: unrelated Journal appends and
post-base canonical growth are allowed. Existing matching operation receipts return
before revalidation, preserving lost-receipt recovery. Existing execution fencing,
cancellation grants, request-start arbitration and hot-head reconciliation remain
owned by their original runtime boundaries.

Implementation owners and files:

- R1/R5: `src/durable/sqlite/lineage.rs`, `projection.rs`, `sqlite.rs`,
  `src/local_runtime/session.rs`, and `session_controller.rs`.
- R2: `src/model/types.rs`, `src/model/adapter/validation.rs`,
  `src/agent/execution.rs`, `tests/conformance/agent_loop.rs`, and the existing
  Provider Emulator conformance scenarios.
- R3/R4: `src/durable/sqlite/compaction_resources.rs`, `projection.rs`, and
  `src/durable/sqlite/projection/tests.rs`.
- Browser producer synchronization: `web-console/test/e2e/compaction.spec.ts`.
- Invariant documentation: `docs/invariants.md` and this acceptance record.

The initial R1–R5 repair required no schema, generated protocol, dependency,
lockfile, or CI workflow changes. Generated-contract drift checks passed. The
subsequent latest-main protocol reconciliation is documented below.

## Deterministic regressions

The four durable/lineage counterexamples were run red against the reviewed behavior;
the cross-turn emulator test was red for stored Responses. No race proof uses sleeps.

| Owner | Tests |
| --- | --- |
| Session lineage (R1) | `historical_fork_excludes_checkpoint_activated_after_selected_ledger`; existing `projection_activation_closure_survives_clone_fork_clone_again_and_source_deletion` |
| SQLite projection (R1/R5) | `inherited_activation_coordinates_are_validated_before_atomic_installation`; `early_lineage_selection_never_hydrates_later_large_checkpoints`; `corrupted_eligibility_header_cannot_hide_a_required_historical_projection`; `historical_selection_rejects_missing_eligible_checkpoint_closure` |
| Adapter preflight (R2) | `reconciled_coverage_replays_only_complete_native_units` (stored, encrypted stateless, signed Anthropic, missing replay) |
| Real local Provider Emulator / Agent / SQLite (R2) | `cross_turn_image_projection_reconciles_native_coverage_before_frozen_retry`; `invalidated_stored_reasoning_fails_compatibility_before_any_new_request_start` |
| SQLite cut validation (R3) | `dependency_free_checkpoint_rejects_every_future_resource_cut_atomically`; `original_and_checkpoint_only_candidates_validate_their_cut_before_commit`; `historical_read_cut_with_inconsistent_activation_is_rejected_without_dependencies` |
| SQLite metadata budgeting (R4) | `trusted_metadata_sources_share_one_predecode_materialization_budget`; `metadata_checkpoint_sources_cannot_reset_the_operation_budget` |

The image scenario uses two accepted tool batches (16 then 1), actual native output,
and a scripted HTTP 503 before success. It asserts equal retry wire bodies, equal
frozen semantic fingerprints, exact reconstructed requests, seventeen immutable
physical Tool results, and no extra requests. The missing-replay scenario checks
that failure creates neither a third request snapshot nor a third dispatch.

Thread-local counters are located at actual candidate/body decode entry. The early
selection test charges only one eligible checkpoint, including explicit activation
selection and the ordinary message-boundary path. The canonical metadata test
rejects the thirty-second 512 KiB source before decoding it. Parent checkpoint
metadata uses the same allowance, hydrates no retired body, and refuses the next
parent before its metadata field is decoded.

Existing owner tests continue to cover same-Ledger activations, repeated copy/fork,
source deletion, operation idempotency/conflicts, cancellation before/after grant,
unknown commit outcomes, late receipts, retry input versions, and process-death
atomicity. Their synchronization and linearization points are documented in
[Issue #466 acceptance evidence](issue-466-implementation-status.md).

## Validation record

The following checks have completed on the repaired implementation. Native test
commands use `TMPDIR=/var/tmp/rustx-466-tests` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`; live provider tests remain ignored.

| Command | Result |
| --- | --- |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins --example document_artifact_fixture --locked` | Passed |
| `cargo test --workspace -- --test-threads=2` | 4,016 passed; 0 failed; 8 existing ignored |
| `cargo test --lib agent::execution::tests -- --test-threads=2` | 33 passed |
| `cargo test --test conformance image_projection_reconciles -- --test-threads=2` | 1 passed (two native protocol scenarios) |
| `cargo test --test conformance invalidated_stored_reasoning -- --test-threads=2` | 1 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites:: --test-threads=2` | 3,328 passed; 3 existing ignored |
| `cargo test --test contracts --test provider --all-features -- --test-threads=2` | 208 passed; 5 live tests ignored |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=2` | 200 passed on complete rerun; initial run 199 passed / 1 liveness timeout |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=2` | 430 passed on complete rerun: 129 durable, 60 process, 47 subagent, 135 tools, 28 conformance, 31 CFG3 |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Passed |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Passed |
| `uv run --project test-support/fake-provider --frozen pytest` | 54 passed |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir tui typecheck` | Passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | 276 source records and 264 notices verified |
| `pnpm --dir dev test` | 41 passed |
| `pnpm --dir tui test` | 956 passed; 1 existing platform skip |
| `pnpm --dir web-console test` | 2,530 passed; 4 existing platform skips |
| `pnpm --dir web-console build` | Passed; existing bundle-size warning |
| `node web-console/scripts/check-office-sandbox.ts` | Office cgroup/filesystem admission passed |
| `bash web-console/scripts/browser-tests.sh test/e2e/compaction.spec.ts` | 4 passed (English/Chinese, light/dark) |
| `pnpm --dir protocol/app-server check` | Passed; generated protocol/schema unchanged |
| `pnpm --dir protocol/app-server typecheck` | Passed |

An initial broad run exposed an overbroad SDK preflight on generic scripted
empty-history requests. That run was interrupted after failed owner assertions;
preflight was restricted to replay following coverage reconciliation, and all
33 execution owner tests and the full workspace run above passed without changing
their assertions. Early decode-counter regressions also identified and corrected
the counter's placement before the raw-body guard and an unrelated test-setup
head read. These are not represented as passing executions.

The initial all-features boundary run timed out at the existing 30-second
`bounded()` guard in `committed_present_reads_exact_native_scope_through_current_authorized_attachment`.
Its isolated execution passed in 2.06 seconds; the full 200-test lane then passed
unchanged in 166.78 seconds. The timeout's internal phase was not captured and did
not recur. No timeout, assertion, test synchronization, or CI workflow was modified.

The initial external all-features run passed CFG3, all 28 composed scenarios,
129 durability and 60 process tests, then timed out in
`running_child_inspection_is_execution_independent` waiting for the existing
two-minute provider gate. Its isolated execution passed all modes in 13.63 seconds;
the complete external lane then passed all 430 tests unchanged. The original
internal cause was not established. An independent inotify probe succeeded.
The initial failure remains recorded rather than attributed to an unproven cause.

The initial full browser run passed 315 tests and failed the final post-cancellation
text assertion in Chinese dark-mode compaction. Earlier checkpoint, activation, and
projection-detail assertions had passed; the trace showed the next turn still queued.
The assertion now waits for the existing emulator response-completion fact before
checking rendered text, retaining the original five-second UI assertion. Failure
attachments capture native diagnostics and the emulator report. All four focused
variants passed unchanged UI/console assertions; no product behavior, timeout, golden
reference, or screenshot tolerance was changed. The final complete browser rerun and final-commit GitHub CI results are published
in the [PR #477 review report and checks](https://github.com/Caismis/rustX/pull/477).
They were still running when the repair commit was prepared; this record does not
claim those executions passed before their completion. Earlier baseline CI had nine successful checks and one Full Web
conformance failure: 315 browser tests passed, while a model-error fixture shutdown
reported a closed socket after its UI assertions completed. Downloaded trace and
job logs were inspected; that baseline result is not represented as a green check.


## Final review follow-up: F1 and F2

This follow-up starts at `e87867ebf3166af0111d23935654f7c1a40773e6`, whose
complete local browser rerun passed 316 cases and whose ten GitHub checks passed.
Fetched main remains `6c73e456e826024fdbe153198a48c9128142e115`.

F1 was confirmed by `checkpoint_predecessor_must_match_the_head_visible_at_its_read_cut`: the old transaction accepted a dependency-free candidate naming P1 but
claiming activation zero after P1's completion. The transaction now reuses the
existing journal-cut selection authority to read an integrity-validated activation
header, checks both its activation and checkpoint identity against the candidate,
and performs this validation before checkpoint mutations. Current-head CAS and
execution fencing still protect commit separately. Historical matching receipts
resolve first. No current Ledger/Journal equality is required.

The regression checks zero activation, predecessor mismatch and inconsistent
Journal/activation coordinates, asserting unchanged content/activation/receipt
tables, Projection Head and Journal state after rejection.
`historical_head_validation_preserves_growth_same_ledger_and_old_receipts` proves
a legal empty first head, subsequent canonical and unrelated Journal growth,
same-Ledger activations, and historical receipt recovery after a newer head.
All 21 SQLite projection owner tests passed.

F2 was confirmed by the composed Provider Emulator regression
`retired_native_coverage_does_not_anchor_overflow_compaction_or_frozen_retry`.
The stale owner made planning require retirement through the seventeenth-image
Assistant instead of permitting the first legal prefix. The final scripted summary
expectation explicitly excludes that old-owner span. The Agent now performs adapter replay preflight before retiring native state,
accepted-output source and continuation compaction anchor through one synchronous
private transition. Successful Summary and existing retirement call sites use the
same transition. Changed coverage also discards old measured-input evidence.
No canonical native units, Fresh Inbound, Context or carryover ownership are erased.

The final scenario provides signed native thinking and usage observations, retains
an unchanged continuation, then introduces image 17, returns overflow, performs
real summary compaction, fails transiently and succeeds. It proves five primary
snapshots plus one separate summary invocation, identical final retry wire/input
content and fingerprints, distinct request/provisional identities, exact
reconstruction, Context introduction once, three accepted Assistant facts and
seventeen physical Tool results. The test passed. The existing missing-native-replay
regression continues to verify that failed preflight starts no extra request.

The updated complete validation matrix and final-commit CI results are recorded in
the [PR #477 review report](https://github.com/Caismis/rustX/pull/477). Prior R1–R5
regressions remain in their existing owner suites. F1/F2 themselves change no
schema, generated protocol, dependency, transport, UI behavior, golden reference
or CI selector; the separate latest-main integration below combines existing
protocol contracts.


Follow-up validation completed on the repaired implementation, using the same
isolated environment and serialized native/client order as above:

| Command | Follow-up result |
| --- | --- |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins --example document_artifact_fixture --locked` | Passed, repeated before client/process fixtures |
| `cargo build --bins --all-features --locked` | Passed |
| `cargo test --workspace -- --test-threads=2` | 4,019 passed; 0 failed; 8 existing ignored |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites:: --test-threads=2` | 3,330 passed; 3 existing ignored |
| `cargo test --test contracts --test provider --all-features -- --test-threads=2` | 208 passed; 5 existing live tests ignored |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=2` | 200 passed |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=2` | 431 passed: CFG3 31, conformance 29, SQLite 129, process 60, subagent 47, tools 135 |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Passed |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Passed |
| `uv run --project test-support/fake-provider --frozen pytest` | 54 passed |
| `pnpm --dir protocol/app-server check` | Passed; no generated drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 41 tests passed |
| `pnpm --dir tui typecheck`; `pnpm --dir tui test` | Passed; 956 passed, 1 existing platform skip |
| `pnpm --dir web-console typecheck`; `pnpm --dir web-console test` | Passed; 2,530 passed, 4 existing platform skips |
| `pnpm --dir web-console build` | Passed; existing bundle-size warning |
| `pnpm --dir web-console check:i18n`; `pnpm --dir web-console check:provenance` | Passed |
| `node web-console/scripts/check-office-sandbox.ts` | Passed |
| `bash web-console/scripts/browser-tests.sh test/e2e/compaction.spec.ts test/e2e/reading.spec.ts test/e2e/uploads.spec.ts` | 8 passed (4.5 minutes), pinned browser container |

The F1 red execution accepted an activation-zero candidate after P1's completion;
the revised owner suite rejects all three incoherent cases atomically. The F2 red
execution failed overflow recovery with the stale owner. The first green execution
then exposed a fixture omission: its first response had no native continuation,
so the unchanged-continuation assertion was inapplicable. The fixture now provides
signed thinking on both accepted tool turns and actual usage observations. The
complete final scenario and all full lanes passed without weakening assertions.
No unexplained liveness failures occurred in this follow-up matrix.

Final-commit GitHub CI is recorded in the linked PR after push. This local validation
record does not claim remote checks completed before the commit was published.


## Latest-main integration

Main advanced during validation to `97096020d64ea00218493a0a99b9b805821eeb28`
(PR #465, deterministic User/Workspace configuration auto-repair). The F1/F2
repair is preserved in commit `2a09dfcf`; its complete validation above was
performed before this integration. Shared client/documentation conflicts were
resolved by retaining auto-repair notices and typed projection history, with no
canonical compaction-marker or Surface compaction path restored.

Both lines had independently used App Server v47 for different contracts. The
combined authoritative contract is now v48, with regenerated schema/TypeScript
artifacts, synchronized client imports/subprotocols/initialization checks, and no
v47 reader or alias. Runtime Client remains v69. Local Harness source hashes and
retained import inventories were refreshed for the actual merged source; original
upstream attribution, hashes and license data remain intact. Protocol drift, all
client typechecks, i18n and provenance checks passed. Full integrated-source
validation and final-commit CI are reported in the PR.

The upstream macOS CI job budget change belongs to PR #465 and is retained from
main; this repair changes no native test deadline, assertion or lane selector.

## Integrated validation diagnostics

The first integrated workspace library run passed 3,536 cases but
`issue422_creation_binding_precedes_multi_client_visibility` exceeded its existing
30-second `bounded` liveness deadline. The captured failure did not identify the
particular awaited stage. The same test passed unchanged in isolation (2.40 seconds)
and in both subsequent full-library invocations. Its exact timeout cause remains
unestablished; it is not classified as an environmental failure. No assertion,
synchronization gate or deadline changed.

The second invocation passed the library and composed native F2 regression, then
the external documentation contract detected a stale predecessor label in the
Session lifecycle section: v46 was named instead of v47 after the authoritative
protocol advanced to v48. The document now explicitly rejects v47 and earlier.
`current_app_server_documentation_matches_negotiated_version` passed immediately
after this correction. A complete workspace invocation was restarted rather than
claiming the earlier partial invocations passed. Original diagnostic logs remain
in the local validation record.

The initial integrated all-features contract run passed 3,446 library cases but
`issue462_agent_control_group_follows_one_admitted_generation` and
`live_model_selection_accepts_background_ownership_without_adopting_pending_context`
hit the same existing 30-second liveness check. Both passed unchanged in isolation
(2.13 and 0.98 seconds), and both passed in the next complete-lane attempt.
Their original timeout causes are not established. That next attempt then reported
a sequence of protocol failures and was interrupted rather than continued as a
successful run. During that sequence the host reported severe I/O pressure and
the active test workers were observed in `rq_qos_wait`/`jbd2_log_wait_commit`;
these observations do not establish the failed tests' exact awaited stages.

Attaching a syscall tracer was denied by the host. Starting the affected protocol
case as a traced child passed in 1.90 seconds; the complete protocol owner suite
then passed all 67 cases, with traced sync calls below 0.16 seconds. Consequently,
the protocol failure sequence also remains unexplained rather than being labeled
an infrastructure failure. The complete contract lane was restarted with
`--nocapture` for immediate diagnostics, retaining identical selectors, concurrency,
assertions and deadlines. No host quotas, other tasks or other worktrees were changed.

WebUI validation exposed one more protocol-union fixture omission: its WebSocket
admission was v48 but its default initialize response still advertised v47. That
incompatible fixture caused 974 failures (1,606 other tests passed). The default
version now comes from the existing Rust-serialized initialize fixture, captured
at module scope for TypeScript narrowing; tests may still override it to verify
incompatible peers. The 67 client owner tests and complete 2,580-test WebUI suite
passed after correction. No production client fallback or compatibility mode was
added.

## Integrated-source validation results

The final integrated checks use the same isolated native environment as the
follow-up matrix. The complete default workspace invocation passed. The complete
all-features contract rerun used `--nocapture` solely for immediate diagnostics;
selectors, concurrency, deadlines and assertions were unchanged. Historical
failures and the interrupted diagnostic run above are not counted as passing
commands.

| Command | Integrated result |
| --- | --- |
| `git diff --check`; `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins --example document_artifact_fixture --locked` | Passed; native fixtures prepared before client tests |
| `cargo build --bins --all-features --locked` | Passed |
| `cargo test --workspace -- --test-threads=2` | 4,158 passed; 0 failed; 8 existing ignored |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites:: --test-threads=2 --nocapture` | 3,468 passed; 0 failed; 3 existing ignored |
| `cargo test --test contracts --test provider --all-features -- --test-threads=2` | 208 passed; 5 existing live tests ignored |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=2` | 200 passed |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=2` | 432 passed: CFG3 31, conformance 29, SQLite 129, process 61, subagent 47, tools 135 |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Passed |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Passed |
| `uv run --project test-support/fake-provider --frozen pytest` | 54 passed |
| `pnpm --dir protocol/app-server check`; `pnpm --dir protocol/app-server typecheck` | Passed; no generated drift |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Passed; 41 tests passed |
| `pnpm --dir tui typecheck`; `pnpm --dir tui test` | Passed; 1,025 passed, 1 existing platform skip |
| `pnpm --dir web-console typecheck`; `pnpm --dir web-console test` | Passed; 2,580 passed, 4 existing platform skips |
| `pnpm --dir web-console build` | Passed; existing chunk-size warning |
| `pnpm --dir web-console check:i18n`; `pnpm --dir web-console check:provenance` | Passed; 276 source records, 264 package notices |
| `node web-console/scripts/check-office-sandbox.ts` | Passed |
| `pnpm --dir web-console test:e2e` | 317 passed (19.2 minutes), pinned container |

Final native conformance includes F1/F2 and preserved R1–R5 regressions, real
SQLite commits/restarts, process-death atomicity, cancellation/start arbitration,
local HTTP Provider Emulator replay, exact snapshots and native client routes.
No real model API credentials or Kubernetes infrastructure were used. Remote macOS
coverage and final-commit CI are reported separately in the PR after publication.

The complete diagnostic contract rerun and all subsequent integrated lanes passed.
The earlier protocol failure sequence did not reproduce in the full rerun. The
317-case browser run covers Compaction, historical projection/request detail,
Fork, native trajectory ordering, file delivery, uploads and attachment/source
closure with unchanged assertions and golden references. The branch remains
available for independent review; final remote CI status is maintained in PR #477.

## Three-finding review beginning at 3252f6a

This review starts at `3252f6a296c00f653bf0c1d7c5ea62bf9a6500d9`, with PR
base/main `97096020d64ea00218493a0a99b9b805821eeb28`. The sampled F1 planner described here is superseded by N1 below. These F1–F3 labels belong
to this review; the earlier F1/F2 review above remains a separate historical record.

- **F1, Context planning:** exhaustive full-prefix cloning/rendering was quadratic.
  That repair introduced one render/index pass and at most 64 complete candidate
  probes. Whole requests were priced without additive assumptions, but uniformly
  sampled ranks could miss feasible prefixes and incorrectly report `CannotFit`.
  This incomplete search and preference are corrected by N1 below. Cooperative
  Agent cancellation and existing ownership/grants remain preserved.
  Files: `context/planning.rs`, `summarizer.rs`, `compaction.rs`, existing Context
  owner tests and their async call sites.
- **F2, WebUI inspection:** both page streams replaced old data and stale promises
  could publish obsolete selections. Session-keyed state and current request tokens
  accumulate stable activation/item identities, reject stale completion/error paths,
  prevent duplicate current-page requests and preserve pages on later failure.
  Files: `ProjectionHistory.tsx`, `projection-history.test.tsx` and the native
  Compaction browser acceptance in `compaction.spec.ts`.
- **F3, core Checkpoint compiler:** source-free historical Derived User/Assistant
  content compiled and committed. Core compilation now requires at least one scoped
  immutable canonical/parent source; transformation labels confer no authority.
  Durable source integrity/eligibility checks remain in their existing owner.
  Legitimate historical test fixtures now supply real canonical or parent provenance;
  request-only synthetic inputs remain unaffected. Files: `context/checkpoint.rs`,
  SQLite projection owner tests and `tests/durable/main.rs`.

### Deterministic red/green evidence

Before each production correction, focused tests were run against the defective path:
F1's actual rendering counter failed on 512 messages; F2's four controlled-pagination
regressions failed while the original single-page test passed; F3's compiler and
SQLite tests both failed because source-free content was accepted. No timing threshold
or sleep is used as proof.

| Finding | Regression names / proof |
| --- | --- |
| F1 | `long_prefix_planning_bounds_actual_rendering_and_complete_estimator_work` counts actual render entries and whole-estimator message visits for 512/1024 messages; `non_additive_planning_checks_complete_requests_and_retention_without_inferred_fit` uses a non-monotone estimator; `incremental_transcript_boundaries_match_complete_summary_requests` proves identical rendered requests; `complete_tool_batch_and_core_owned_boundaries_constrain_every_probe` protects reordered Tool siblings, Fresh Inbound, Context and carryover anchors. |
| F1 cancellation | `cancellation_observed_during_planning_starts_no_summary_and_commits_nothing` publishes a watch from the first full estimator query, cancels at the following cooperative yield, and asserts zero summary invocations/checkpoint transitions/Journal facts. |
| F2 | Existing historical inspection test plus accumulated overlapping activation/detail pages, superseded in-flight selection, A→B→A Session replacement, later-page failure/retry and duplicate concurrent-click tests. Deferred promises establish completion order directly. Cursor exhaustion removes the more control. |
| F3 | `historical_derived_requires_a_real_source_even_with_summary_label`; `historical_derived_compiler_accepts_canonical_and_parent_only_sources_but_rejects_wrong_scope`; `source_free_historical_derived_is_rejected_without_any_durable_mutation`; `parent_only_and_combined_immutable_provenance_commit_and_recover_receipts`; `invalid_immutable_derived_provenance_rolls_back_every_durable_authority`. Rejection compares content/activation/receipt/Journal row counts, head, resource cut and immutable canonical history. |

The WebUI regression uses existing mocked App Server routes and English-localized
controls. No generated protocol or transport redesign is required. Native retries,
continuation replay, lineage and process-death atomicity remain covered by the
existing complete validation lanes below.

Browser acceptance extends the existing real-native Compaction flow to switch between
both historical Checkpoints and assert that both activation records remain visible.
It captures open-history screenshots outside the repository for English/Chinese,
light/dark and desktop/mobile QA. Browser plugin capabilities are not available in
this session; the repository's digest-pinned Chromium/Playwright runner is used.

The first complete workspace attempt reported 3,546 library tests passed, one failed
and three existing ignored. The failure was the existing
`scripted_suites::agent::tool_scheduling::cancellation_during_mixed_batch_settles_structurally`
10-second `run terminates` liveness guard (`Elapsed(())`). Its one-message input,
10-million-token model window and unchanged fixture never enter summary prefix
planning. The unchanged eight-test scheduling owner rerun passed in 12.26 seconds.
Fixture preparation and foreground/background watches were inspected; the timeout's
exact cause is not established and is not labeled an infrastructure failure. The
complete workspace command is rerun below; no deadline, assertion or selector is
weakened to hide the initial result.

The second disk-backed workspace attempt again passed 3,546 library tests, with a
different one failing: `goal350_background_terminal_publication_wakes_next_continuation`
at its existing 10-second parked-provider wake guard (`Elapsed(())`, extensions
owner line 4131). The earlier mixed-batch cancellation test passed in this run.
The unchanged Goal test passed alone (0.34 seconds). During the full-run stalls,
live process/thread inspection captured the test binary's threads in kernel
`jbd2_log_wait_commit`; `/proc/pressure/io` reported full pressure above 70%.
These observations establish an actual journal-I/O condition; they do not establish
the exact phase responsible for either earlier timeout.

Final isolation uses `TMPDIR=/run/user/1000/r466-477` on the existing executable
user-owned tmpfs (verified fixture execution), with
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. No filesystem mount, host quota, deadline,
assertion, retry budget or CI selector is changed. SQLite remains file-backed,
with its actual transactions, WAL/locking, restart and process-death cuts. This
isolates temporary fixture writes from the stalled root-filesystem journal; it is
not power-loss durability testing. Native builds and client/browser execution
remain serialized. Earlier disk-backed failures are retained as distinct attempts.

### Final three-finding validation

The implementation was validated in the dedicated worktree. Final passing commands
use the isolated scratch environment described above; builds complete before fixture
execution. Counts below are per invocation, not distinct-test totals across overlapping
lanes. Existing live-model tests remain ignored; no real model API is called.

| Command | Final result |
| --- | --- |
| `git diff --check` | Passed, repeated after review |
| `cargo fmt --all -- --check` | Passed, repeated after review |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed; initial new-code style diagnostics corrected |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins --example document_artifact_fixture --locked` | Passed; repeated before client tests |
| `cargo build --bins --all-features --locked` | Passed |
| `cargo test --workspace -- --test-threads=2` | 4,168 passed; 0 failed; 8 existing ignored |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites:: --test-threads=2` | 3,478 passed; 0 failed; 3 existing ignored |
| `cargo test --test contracts --test provider --all-features -- --test-threads=2` | 208 passed; 0 failed; 5 existing live-model ignores |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=2` | 200 passed; 0 failed |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=2` | 432 passed; 0 failed: CFG3 31, conformance 29, SQLite 129, process 61, subagent 47, tools 135 |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Passed, Linux lane coverage |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Passed, Linux lane coverage |
| `uv run --project test-support/fake-provider --frozen pytest` | 54 passed |
| `pnpm --dir protocol/app-server check` | Passed, no generated drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 41 passed; 0 failed |
| `pnpm --dir tui typecheck` | Passed |
| `pnpm --dir tui test` | 1,025 passed; 0 failed; 1 existing platform skip |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 2,585 passed; 0 failed; 4 existing platform skips |
| `pnpm --dir web-console build` | Passed, existing bundle-size warning |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed, 276 source records / 264 package notices |
| `node web-console/scripts/check-office-sandbox.ts` | Passed |
| `bash web-console/scripts/browser-tests.sh test/e2e/compaction.spec.ts test/e2e/trajectory-integration.spec.ts` | 5 passed, 37.7 seconds, pinned Chromium container |

Focused owner checks also passed: Context 150, SQLite projection 24, Projection
History component 6. The new native tests passed in default and all-features lanes.
The complete local browser suite was not repeated: the selected acceptance covers
both affected native historical inspection/compaction and adjacent trajectory
behavior; the full browser suite remains a required remote CI lane. macOS coverage
is provided by remote CI, not claimed as local coverage.

Browser QA used `http://127.0.0.1:5173/`, the real App Server/Product Host and local
Provider Emulator, with 1440x1000 desktop and 390x1000 mobile, English/Chinese and
light/dark themes. Page identity, populated content, interaction completion and
console/page-error checks passed. Open-history screenshots show both activation
records and the second selected Derived detail; screenshots were inspected and are
stored outside the repository (`/tmp/rustx-projection-history-*.png`). Paging races
are proven by controlled promises in component tests, not screenshot timing.

The code changes preserve immutable canonical facts and typed projection ownership.
SQLite COMMIT, operation receipt ordering/CAS/fencing, request-start cancellation
arbitration, frozen retry versions and native replay are unchanged. Existing tests
exercise these boundaries, inherited activation distinctions, source deletion,
exact reconstruction and lossy archive failure. Passing lanes supplement the source
ownership analysis; they do not prove arbitrary future algorithms/providers correct.

The >64-boundary sampled selection recorded for that implementation was incomplete
and is corrected by N1 below. Estimator internals
have their own cost; planner-controlled rendering, materialization and query counts
are bounded. Earlier disk-backed liveness failures and their unestablished exact
phases remain reported above. Final branch/commit and current-head CI status are
published in PR #477 after pushing; prior-head green CI is not substituted.

### Final coverage-strengthening follow-up

After the full local matrix and first push, final review found that the older
R3 dependency-free future-cut regression asserted only rejection. F3 now independently
rejects source-free historical content, so that broad assertion could mask removal
of the unconditional durable cut validator. It now requires the specific durable
Resource Read Cut error in every coordinate domain, before the later compiler
provenance guard. This strengthens the existing assertion; no production code,
deadline or selector changes.

The complete matrix above ran on `7117275c0390ffe345d21aad3a8e47345a084187`.
The coverage-only follow-up reran
`cargo test --lib durable::sqlite::projection::tests:: -- --test-threads=2`
and its `--all-features` form: both passed all 24 tests. Both complete workspace
Clippy modes, `git diff --check` and `cargo fmt --all -- --check` are repeated for
this follow-up. Full remote CI is inspected for the actual final pushed commit;
earlier-head CI is retained only as historical evidence.

## N1: complete candidate discovery without quadratic recomputation

Starting HEAD: `5d52e20d765fd244f63151e89889d0d8cde14a93`; reviewed planner:
`7117275c0390ffe345d21aad3a8e47345a084187`; base/main:
`97096020d64ea00218493a0a99b9b805821eeb28`.

The uniform 64-boundary sampler skipped boundary 2 of 129. Two deterministic red
tests returned `CannotFit` despite boundary 2 being the only valid fit: one arbitrary
estimator, and one actual `DefaultTokenEstimator` budget construction. The pre-fix
run had zero passes and two failures. This was a semantic defect, not an approximate
fit or a resource exhaustion.

The Context Engine retains one transcript/index pass and complete Tool-batch
boundaries. A narrow estimator guarantee enables complete monotone interval search
for production semantic JSON-byte accounting, including identity media/upload
wrappers. It does not assume rounded token additivity. Exact Summary prices increase
with appended escaped text; whole primary/recent prices decrease with prefix removal
and fixed core inputs. Three binary partitions establish the feasible interval and
keep-recent preference; cached whole-candidate prices are validated before selection.
O(B log N) materialization and O(N + B) memory replace repeated O(N B) prefix copies.

Without a monotonicity guarantee, at most 64 descending exact queries may establish
a preferred fit. Exhaustive evaluation alone authorizes the earliest fallback or
proven infeasibility. Remaining unexamined candidates yield `SearchIncomplete`, even
when a nonpreferred fit is already known. This makes the guarantee explicit for
non-monotone estimators and typed dynamic media. Agent execution settles that outcome
once as a runtime compaction failure, including after a real overflow; it does not
start a Summary, commit a Checkpoint or start another actual provider request.
Cancellation retains its existing owner and is observed between exact queries.

### N1 deterministic evidence

| Requirement | Authoritative regression / observation |
| --- | --- |
| T1 | `n1_unsampled_only_feasible_boundary_is_not_proven_cannot_fit` and `n1_default_estimator_finds_only_feasible_second_of_129_boundaries`: both red on the sampler; arbitrary estimator now explicitly incomplete, production estimator selects 2. |
| T2, T4 | `n1_default_search_preserves_extremes_preference_and_fallback`: exhaustive exact-price oracle over 129 boundaries, first-only/last-only fits and multiple preferred/fallback cases. |
| T3 | `n1_arbitrary_non_monotone_prices_require_actual_candidate_queries`: disconnected feasible boundaries, no inference from neighbors; `non_additive_planning_checks_complete_requests_and_retention_without_inferred_fit` retains whole-request/core-overlay proof. |
| T5 | `complete_tool_batch_and_core_owned_boundaries_constrain_every_probe`: reordered sibling results, mandatory coverage, Fresh Inbound, Context and carryover constraints; a split protected batch proves infeasible. Existing missing-anchor and Fresh Original tests remain. |
| T6 | `n1_default_search_bounds_actual_render_copy_and_serialization_work`: 512/1024 messages, actual render entries, estimator entries, Summary string bytes, logically encoded cloned assemblies and counting-writer bytes; no timing threshold. |
| T7 | `n1_cancellation_during_exact_query_settles_without_summary_commit_or_request`: existing estimator test hook cancels synchronously inside the first exact Summary estimate → next cooperative observation; exactly one Summary estimate; one cancelled terminal, zero Summary/provider invocations, no start/completion/failure/receipt/head transition. Existing pipeline cancellation proof remains. |
| T8 | `n1_generic_exhaustion_distinguishes_incomplete_search_from_infeasibility`: 129 unknown versus 64 exhaustively rejected boundaries; `n1_incomplete_search_invokes_no_summary_and_leaves_no_durable_transition` checks the known operation's absent receipt; `n1_incomplete_search_settles_once_as_runtime_failure_even_after_real_overflow` checks proactive and actual-overflow terminal classification and request counts. |
| T9 | `n1_default_middle_candidate_commits_and_freezes_the_subsequent_primary_input`: file-backed SQLite Agent composition, production estimator through identity wrappers, only boundary 2 selected, Derived checkpoint installed, all eligible Originals retained, canonical prefix unchanged and exact snapshot reconstruction equals dispatched input. |

The focused N1 run passed all ten tests. Existing full Context owners initially
passed 82 tests before adding the explicit Agent cancellation regression. Native
retry/continuation, lineage, process-death and client/browser preservation are
checked in the full matrix below. No new testing framework or live API is used.

### N1 complete local validation

All commands below passed using the final production implementation. After the full
native runs, the cancellation regression was strengthened to cancel synchronously
inside its first estimator query and assert exactly one query. Both complete Context
owners (83 tests each), both Clippy modes, formatting and whitespace were rerun after
that test-only change. Cancellation proof does not depend on executor scheduling.
The Linux discovery checker initially compared the old/new test names during that
rename; stable-source reruns of both unchanged lane commands passed. No CI selector,
timeout, assertion or golden was weakened. An initial test-accounting slice type
error was corrected before the successful matrix; the mandatory red failures remain
separate from final passing validation.

Native, client and browser fixtures used `TMPDIR=/run/user/1000/r466-477` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`, the established executable scratch isolation.
SQLite remains file-backed with real COMMIT, locking, restart and process-death
boundaries. This does not claim power-loss durability. The slow isolated `uv lock`
fixtures completed successfully; no environmental test failure was suppressed.

| Command | Result |
| --- | --- |
| `git diff --check` | Pass, including final documentation |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass, repeated after cancellation test hardening |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass, repeated after cancellation test hardening |
| `cargo build --bins --example document_artifact_fixture --locked` | Pass; default native binaries/fixture built before process tests and restored before clients |
| `cargo build --bins --all-features --locked` | Pass |
| `cargo test --workspace -- --test-threads=2` | 4,178 passed, 0 failed, 8 existing ignored |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites:: --test-threads=2` | 3,488 passed, 0 failed, 3 existing ignored |
| `cargo test --test contracts --test provider --all-features -- --test-threads=2` | 208 passed, 0 failed, 5 opt-in live-model tests ignored |
| `cargo test --lib --all-features -- boundary_suites:: --test-threads=2` | 200 passed, 0 failed |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output -- --test-threads=2` | 432 passed: SQLite 129, process 61, subagent 47, tools 135, conformance 29, CFG3 31 |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Pass on stable source after mixed-name discovery rejection |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Pass |
| `uv run --project test-support/fake-provider --frozen pytest` | 54 passed |
| `pnpm --dir protocol/app-server check` | Generated-contract drift check passed; no regeneration required |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir dev typecheck` | Pass |
| `pnpm --dir dev test` | 41 passed |
| `pnpm --dir tui typecheck` | Pass |
| `pnpm --dir tui test` | 1,025 passed, 1 existing platform skip |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | 2,585 passed, 4 existing platform skips |
| `pnpm --dir web-console build` | Pass |
| `pnpm --dir web-console check:i18n` | Pass |
| `pnpm --dir web-console check:provenance` | Pass: 276 source records, 264 package notices |
| `node web-console/scripts/check-office-sandbox.ts` | Pass |
| `bash web-console/scripts/browser-tests.sh test/e2e/compaction.spec.ts test/e2e/trajectory-integration.spec.ts` | Pinned browser acceptance 5/5 passed |
| `cargo test --lib scripted_suites::context:: -- --test-threads=2` | Final hardened Context owners: 83 passed |
| `cargo test --lib --all-features scripted_suites::context:: -- --test-threads=2` | Final hardened Context owners: 83 passed |

No real model API, K3 infrastructure, compatibility path, generated version change,
canonical summary insertion or new execution state machine is introduced. Schema 59,
App Server v48 and Runtime Client v69 remain unchanged. Current-head remote CI is
reported in PR #477 after pushing; previous-head CI is historical evidence only.

### N1 remaining bounded limitation

Arbitrary/non-monotone estimators and dynamic typed image/upload estimators do not
promise complete efficient selection. If their 64 exact queries cannot establish
preference or feasibility, the runtime terminates once with `SearchIncomplete`.
This explicitly preserves the unknown result and does not claim `CannotFit`.
Individual synchronous estimator calls finish before cancellation is observed at
the next checkpoint; the planner bounds calls/materialization, not arbitrary callback
CPU behavior. Production JSON-byte accounting has the documented complete guarantee.
