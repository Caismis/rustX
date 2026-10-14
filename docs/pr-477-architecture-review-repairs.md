# PR #477 architecture review repairs

This repair continues `issue-466-context-projection` in the existing dedicated
worktree. The starting HEAD is `bd89fe71c2fd399e6a3a7c2fbae172ea5e814006`.
Fetched main remains `6c73e456e826024fdbe153198a48c9128142e115`, already integrated.
No primary checkout, other worktree, transport contract, or canonical-history
ownership changes are part of this repair.

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

No schema, generated protocol, dependency, lockfile, or CI workflow changes were
required. Generated-contract drift checks passed.

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
