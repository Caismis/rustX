# Issue #466 implementation and acceptance evidence

The subsequent PR #477 review repair is documented in
[architecture review repairs](pr-477-architecture-review-repairs.md). The commands
and counts below describe the original implementation baseline, not a substitute
for validation of those repairs.

The ownership transition and local validation are complete. All development
remains in the dedicated worktree. The associated PR provides the final implementation
commit and current platform CI status; local validation does not claim a macOS result.

## Repository and scope

- Starting main: `3c4b85a627e87383f244600e8b19887f103b82f0`.
- Integrated main: `6c73e456e826024fdbe153198a48c9128142e115`, including #471's live
  Session model selection with immutable running Attempts.
- Branch: `issue-466-context-projection`.
- Worktree: `/home/capi/Documents/codes/rustX-issue-466`.
- Primary checkout and independent #467 worktree were not modified by this task.
  Open PRs and #467 overlap were inspected; #467's transport redesign is excluded.
- The branch is submitted as one non-draft PR against `main`, with `Closes #466`.
  No merge or auto-merge; the branch and worktree remain available for review.

## Final ownership

Canonical History retains real immutable committed messages. Context owns bounded
historical compilation, structural/native validation, token accounting and planning.
CompactionResources reads existing canonical, Journal, trajectory, lifecycle,
checkpoint, snapshot and authorized settled Artifact authorities at one finite cut;
each query returns detached working material and closes its transaction.

The durable store owns checkpoint content, activations/head, CAS, execution fencing,
local completion and idempotent operation receipts. The Agent Loop owns the latest
immutable semantic input version, current execution, cancellation, actual starts,
retries, tools and settlement. Provider adapters translate wire representations and
validate native replay/continuation. RequestSnapshot owns exact started input.
Runtime Client/App Server/WebUI/TUI read and label execution and projection domains
separately; projected calls never create Tool executions or usage statistics.

The canonical occurrence index, derived atomically from actual Tool messages,
answers physical settlement checks. A projected Original call paired with a Derived
result cannot create a false recovery repair. Recovery loads only outstanding
physical occurrence dependencies in addition to the bounded canonical working set.

## Durable and request contracts

Development SQLite schema **59** adds `projection_checkpoints`,
`projection_activations`, `projection_head` and `compaction_operations`, with required
column, uniqueness and dependency checks. It also indexes outstanding canonical
Tool occurrences. Earlier development stores are rejected, without migration.

A checkpoint contains ordered Original scoped immutable references or materialized
Derived typed bodies/provenance, an independent complete base inclusion frontier,
expected head, resource cut, fingerprint and trusted cumulative file metadata.
Derived messages have no canonical MessageId. Default Summary is a Runtime User
`ProjectedUserKind::CompactionContext`, retaining structured Markdown/XML sections.

Resource Read Cut, base Ledger frontier B, activation coordinate and Request History
Cut are separate domains. Tail is strictly `(B, frozen pre-start Ledger frontier]`.
An excluded message at or before B cannot return because an item omits it. Actual
activation Ledger records commit-time history growth, independently of B.

Canonical Tool occurrences, checkpoint message/block coordinates, actual-request
message/block coordinates and wire correlation labels are distinct. Compilation
validates all four Original/Derived pairings, reordered blocks/results, complete
batches, repeated raw labels and orphan/ambiguity rejection. Wire encoding uses
stable occurrence labels; opaque signed/encrypted replay units are not edited.

Checkpoint commit's SQLite COMMIT is its linearization point. Before it, one
transaction validates operation/fingerprint, expected head, execution fence and
immutable/mutable dependencies; inserts content, activation, head, metadata, local
CompactionCompleted and receipt. Same operation/content returns the same receipt;
conflicting content fails. Unknown outcomes look up that operation without rerunning
the summary. Hot installation reloads the current head under current ownership;
confirmed success cannot be followed by a contradictory failure fact.

Cancellation-versus-compaction grant and cancellation-versus-request start use
separate owner gates. Manual compaction has no actual model-request start, Agent
Turn, tool authorization or dynamic contributor invocation.

Authorized correction or overflow compaction advances the active logical generation's
frozen input version. Transient retries reuse the latest version and do not resample
contributors, uploads or image selection. Context K is visible once on every request,
introduced only by the first actual request; replaying that exact start receipt
retains its original introduction. New retries have new request/snapshot/publication/
provisional-output coordinates, equivalent semantic fingerprints and independent
execution/usage facts. Candidate and actual semantic fingerprints are distinct.

Content-changing image/upload transforms produce request-local Derived content;
materialization is frozen before final fit and start. Reconstruction verifies the
retained snapshot and immutable dependencies only. It never reruns compaction,
contributors, paths or image discovery. Native continuation refers to actual accepted
Assistant output of the previous request, with complete native replay validation.
Default Summary always clears primary continuation and compiles the full new history.

Historical activations survive repeated clone/fork/clone at the same Ledger frontier.
Source origin and destination installation differ. Destination-owned canonical/typed/
Artifact closure survives source deletion; inherited activations emit no destination
CompactionCompleted. Missing expected dependencies fail explicitly.

Archive **v4**, request stream **4**, projection stream **1** use an explicit positive
inspection allowlist. Exported requests/projections report `exact_reconstruction:
false` and missing components when exact bytes/native state are omitted. Historical
snapshot loss does not invalidate an independently complete active checkpoint;
missing active dependencies blocks preparation. Exact input reconstruction, current
resendability and reproducibility remain distinct.

App Server **47** / Runtime Client **69** expose bounded historical projection pages,
occurrence-bearing typed details, frozen request coordinates/associations and explicit
`session/forkProjection` activation cuts. Generated v47 replaces v46. WebUI/TUI show
local/inherited origin independently of canonical transcript adjacency. Web history
reads are on demand; pages replace bounded working material.

Removed: canonical CompactionSummary kind/insertion, Surface Replace/span ownership,
old commit_compaction/checkpoint-statistics API/storage, dead StructuralIndex/span API, canonical summary markers,
old snapshot Surface authority, reconstruction-time media/upload resampling and
retired generated v46. Ordinary canonical append/cache bookkeeping remains.

## Section 13 regression mapping

Names below are actual tests in existing authoritative owner suites. Prefixes are
abbreviated where repeated; final commands execute their real Cargo/libtest discovery.

| Section 13 requirement | Owner and test names |
| --- | --- |
| L100 exclusion and M101 Tail | `durable::sqlite::projection::tests::checkpoint_base_excludes_m96_through_m100_and_only_m101_enters_tail` |
| Distinct same-L activations and commit-time activation frontier | `same_ledger_frontier_retains_both_activation_boundaries`; `activation_ledger_is_commit_time_frontier_and_not_checkpoint_base` in that same suite |
| Lifecycle reads exclude future completion | `durable::sqlite::tests::compaction_tool_lifecycle_at_cut_never_reads_later_completion`; `compaction_resource_paging_never_observes_future_lifecycle_or_ledger_growth` |
| Unrelated Journal/append growth vs actual fencing | projection tests `unrelated_journal_appends_and_post_base_canonical_growth_do_not_stale_a_candidate`; `execution_owner_transition_fences_a_prepared_checkpoint_grant` |
| Summary progress independent of staged Context | `scripted_suites::context::compaction_pipeline::staged_context_cannot_mask_historical_summary_growth` (20 → 100 historical tokens; 1,000 staged tokens must not mask growth) |
| Four pairings, multiple calls and block/result reorder | `context::checkpoint::tests::all_four_tool_pairings_preserve_batch_and_distinct_block_coordinates`; `durable::sqlite::tests::all_four_projection_tool_pairings_start_exact_requests_without_recovery_execution` |
| Repeated wire IDs, ambiguity/orphans | checkpoint `repeated_wire_strings_are_separate_occurrences_and_ambiguous_copies_fail`; adapter `repeated_wire_ids_are_paired_by_occurrence_in_each_explicit_protocol`; external durable `transcript_tool_occurrences_do_not_alias_across_attempts_and_reopen` |
| Native reasoning and accepted output | checkpoint `changed_native_reasoning_does_not_inherit_signed_or_encrypted_state`; Chat `chat_reasoning_replay_dialects_are_explicit`, `chat_omit_ignores_unavailable_reasoning`; provider `incomplete_or_changed_native_thinking_never_dispatches`, `complete_stateless_output_replays_text_and_calls_once_and_rejects_edits`, `stored_continuation_rejects_omitted_remote_coverage_and_changed_output`, `continuation_maps_actual_accepted_output_even_when_another_assistant_follows` |
| Fresh Inbound / carryover anchor | `scripted_suites::context::engine::derived_source_mention_cannot_replace_core_fresh_inbound`; `scripted_suites::agent::carryover::fresh_inbound_anchor_places_carryover_before_the_pending_inbound_turn`, `request_only_anchors_and_degradation_are_deterministic`; context engine `missing_continuation_anchor_never_silently_loosens_selection` |
| Derived media, uploads and frozen replay | four-pair actual-start test includes request-local Derived Tool image downgrade; `model::uploads::tests::exact_escaped_ordered_projection_preserves_body_and_plain_turns`; `model::transformations::tests::upload_tags_cannot_change_roles_body_or_existing_provenance`; `model::snapshot::tests::frozen_derived_input_reconstructs_without_dynamic_selection` |
| Overflow → compaction → transient keeps I2 | `scripted_suites::agent::retry::transient_retry_does_not_reset_overflow_budget` |
| Correction → overflow → transient keeps I3 and correction | `corrective_overflow_transient_retry_retains_latest_input_and_context` in the same retry suite |
| K visible once / no contributor rerun / new request identities | retry `transient_retry_uses_shared_identity_and_frozen_request`; `transient_retry_does_not_regenerate_status_or_contributors`; context `overflow_retry_reuses_the_admitted_context_generation` |
| Lost same-request start receipt preserves Introduced K | `durable::sqlite::tests::model_turn_start_commits_context_snapshot_and_event_atomically` and `model_turn_start_fault_at_each_stage_rolls_back_everything`; Agent `idempotent_start_receipt_does_not_republish_live_facts` |
| Independent usage/terminal/start facts | retry `failed_request_usage_is_latest_known_snapshot_and_not_duplicated`; `failed_partial_publication_settles_before_retry_and_stays_noncanonical` |
| Cancellation before/after grant | projection `cancellation_before_grant_prevents_commit_and_after_grant_preserves_authorized_durability`; pipeline `cancellation_before_compaction_installs_nothing`, `cancellation_during_summary_installs_nothing` |
| Idempotence/conflict/uncertain commit | projection `operation_receipts_are_idempotent_conflicting_and_never_rewind_a_newer_head`; `unknown_commit_outcome_recovers_the_same_operation_without_reexecution`; SQLite projection `every_checkpoint_transaction_stage_fault_exposes_only_the_old_complete_state` |
| Older receipt cannot regress hot head | `scripted_suites::context::compaction_pipeline::delayed_older_receipt_installs_current_projection_head` |
| Failed hot install cannot start stale input | `scripted_suites::context::runtime_integration::committed_checkpoint_hot_reconciliation_never_starts_from_stale_input` |
| Clone/fork/clone-again/source deletion | `local_runtime::session::tests::projection_activation_closure_survives_clone_fork_clone_again_and_source_deletion` |
| Public typed history and same-L activation fork | `local_runtime::session_runtime_manager::tests::protocol::projection_history_and_activation_fork_cross_native_app_server_boundary` |
| Missing expected projection vs eligible earlier candidate | projection `missing_head_content_or_original_dependencies_never_fall_back_to_identity`; `activation_ledger_is_commit_time_frontier_and_not_checkpoint_base`; native session artifact lineage `artifact_tree_preserves_selected_occupancy_and_native_history_after_restart` |
| Old lossy input / independently valid current execution | `durable::sqlite::tests::incomplete_historical_snapshot_does_not_block_independently_complete_current_checkpoint_or_restart`; native archive `lossy_archive_reports_missing_old_encrypted_input_without_invalidating_current_checkpoint` |
| Exact frozen requests after restart | `durable::sqlite::tests::request_start_is_atomic_and_reconstructs_from_durable_history`; `exact_frozen_request_needs_no_surface_history_replay`; `model::snapshot::tests::missing_original_or_redacted_derived_bytes_are_non_reconstructible` |
| Resource paging/cumulative/early oversize | `durable::sqlite::tests::compaction_resource_memory_bounds_include_all_pages_and_oversized_single_records`; `compaction_tool_lifecycle_at_cut_never_reads_later_completion`, `compaction_byte_pages_resume_without_decoding_the_next_record` |
| No ordinary retired history hydration | projection `ordinary_projection_preparation_does_not_hydrate_retired_summary_provenance` (1,000 retired sources; only retained Original and later Tail bodies decode) |
| Real provider-neutral requests/media | external conformance `chat_parallel_images_switch_and_reconstruct`, `responses_parallel_images_switch_and_reconstruct`, `seventeen_parallel_image_reads_leave_a_valid_provider_continuation`, `compaction_invokes_the_real_provider_on_both_summary_policies`, `repeated_compaction_invokes_the_real_provider_on_both_summary_policies` |
| Real process-death atomicity | `boundary_suites::durable::process_death::conformance::compaction_checkpoint_receipt_and_activation_are_atomic` at `before:commit_projection` / `after:commit_projection`; `an_adopted_turn_survives_the_recovery_terminal_of_a_second_death` |
| Canonical vs projection presentation | Web `reads historical activations and typed occurrences on demand without manufacturing execution rows`, TUI `presents local projection provenance separately from canonical facts` and its inherited variant, native projection protocol test, browser `compaction.spec.ts` / `compaction-summary.spec.ts` |

Race tests establish ordering at actual owner boundaries: mutex-protected commit
and request-start grants, SQLite transaction stages/COMMIT, post-COMMIT Journal
staging plus receipt-return channel, published watch signals and manual deadline
clocks. The late-receipt test parks O1 after COMMIT and store-lock release, commits
O2, then releases O1 and checks hot activation P2. Hot-read faults occur after real
commit. Process-death tests kill only after the child acknowledges its named boundary.
No sleeps or probabilistic timing were added as concurrency proof.

## Validation evidence and environment

Logs live at `/tmp/rustx-466-*.log`. The completed local validation matrix is
recorded below; current remote CI status is available on PR #477. Earlier failed runs exposed obsolete canonical
summary/Surface assertions, unfrozen request fixtures and a missing management-open
integrity function registration; those were corrected without weakening the new
architectural validation. A request association inspection regression also caught
an oversized wire identity; bounded detail now omits oversized identities explicitly.

R01 originally failed at libc inotify initialization with EMFILE under host UID1000,
independently of rustX and despite a large process descriptor limit. UID-level
inotify instance exhaustion was demonstrated; no quota/sysctl or unrelated process
was changed. Later the host watcher and Office admission probes recovered. The
unchanged real R01 test passed in the full native run and again in a network-disabled
container under UID60066, with read-only test/runtime-library mounts and an owned
scratch directory (1 test passed, 185.04 seconds). Earlier 256 inotify init/close
cycles in that separate UID returned descriptor count 4 to 4. No runtime lifetime
workaround was introduced.

Native fixture tests use command-local `TMPDIR=/var/tmp/rustx-466-tests` because
shared `/tmp/.git` and `/tmp/rustx.toml` contaminate ancestor discovery. Shared files
were not removed. Binaries are built before process tests; test executables are not
recompiled while same-path child fixtures run. At least two libtest threads preserve
existing child-readiness formatting. All provider traffic uses scripted fixtures or
the local Provider Emulator; no live model credentials/APIs are used.

Confirmed validation commands (command-local `TMPDIR` and emulator requirement
apply to native fixture runs):

| Command / selector | Confirmed result |
| --- | --- |
| `git diff --check` and staged diff check | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass after final progress fix |
| Same Clippy command with `--all-features` | Pass after final progress fix |
| `cargo build --bins` and bins plus `document_artifact_fixture --locked` | Pass |
| `cargo test --workspace -- --test-threads=2` | Final full run with two libtest workers: all 4,003 enabled tests pass, including library 3,386; cfg 31; conformance 26; contracts 29; durable 129; process 57; provider 179; subagent 47; tools 110; doctests 9. Eight existing ignored tests retain their documented purpose. |
| `cargo test --lib scripted_suites::context:: -- --test-threads=4` | 68 pass, including the new red/green progress counterexample |
| All-features contract selector, `--skip boundary_suites::` | 3,293 passed; generated-description drift and two 30-second protocol liveness failures repaired/rechecked below |
| All-features App Server schema owner | Pass after regeneration |
| All-features protocol owner selector, two libtest workers | All 67 pass; failed cases also pass individually; internal runtime workers and race gates unchanged |
| `cargo test --test contracts --test provider --all-features` | 29 + 179 pass; five existing opt-in live tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | All 200 pass, including real process-death and provider-emulator boundaries |
| All-features external boundary selectors from CI | cfg 31, conformance 26, durable 129, process 60, subagent 47, tools 135 pass |
| `cargo run [--all-features] --example check_test_lanes -- --job rust-contracts/rust-boundaries` | Both Linux coverage checks pass |
| `cargo test --test tools -- --test-threads=4` recheck | All 110 pass; unchanged startup-diagnostic case also passed individually (41.70s), without timeout increases |
| `cargo test --doc` | All 9 pass |
| `uv sync --frozen`; `uv run --frozen pytest` | Pass; all 54 provider-emulator Python tests pass |
| Notification Python unittest discovery | All 15 pass |
| `pnpm --dir dev typecheck`; `pnpm --dir dev test` | Pass; all 41 tests pass |
| `pnpm --dir tui typecheck`; `pnpm --dir tui test` | Pass; latest clean native client run: 956 pass, one existing macOS ACL skip |
| `pnpm --dir web-console typecheck`; `pnpm --dir web-console test` | Pass; latest run: 2,530 pass, four existing macOS Office/Seatbelt skips |
| `pnpm --dir web-console build` | Pass; source provenance and production artifact checks pass |
| `pnpm --dir protocol/app-server generate/check/typecheck` | Pass; v47 generated artifacts have no unstaged drift against the intended staged contracts |
| Web `check:i18n`, `check:provenance`, notices | Pass: 276 source records and 264 production dependency notices |
| Office sandbox admission | Pass |
| `pnpm --dir web-console test:e2e` | Final complete pinned-Docker run: all 316 pass (19.9 minutes). Startup trio also passed separately (14.2 seconds). See producer-barrier correction below. |

Earlier failures exposed obsolete canonical summary/Surface fixture expectations,
unfrozen starts, incomplete tool batches, lineage fixtures dropping projections,
missing management-open digest registration and oversized request-detail wire
identities. These were repaired with stronger owner checks. No assertions, test
coverage or timeouts were weakened. The final progress regression failed the old
comparison before the fix, then passed with all 68 Context owner tests. Fit still
prices the complete core overlay; progress cannot borrow its token cost.

The three existing ignored library tests are two explicit measurements and the
committed document-fixture generator. The five ignored provider tests require live
credentials and were deliberately not run. macOS-only Rust/Desktop and filesystem/
Seatbelt coverage requires platform CI; this Linux host does not claim that result.
All enabled Linux native/provider/client/browser validation completed. Final full
workspace logs: `/tmp/rustx-466-final-workspace-two-workers.log`; browser:
`/tmp/rustx-466-browser-barrier-full.log`. Final all-features Context owner: 68 pass;
both Linux lane-coverage checks pass on the final source.

Four-worker broad native runs exposed intermittent existing 30-second protocol
liveness caps and two-minute managed-Python preparation caps. All failing cases
passed isolated or complete owner rechecks; the final full workspace passed with
two libtest workers. This limits concurrency between tests, not the actors inside
each concurrency test: internal runtime workers, channels/barriers/watches, named
process-death points and deadlines are unchanged. CI selections/configuration were
not altered. The clean TUI and WebUI runs were serialized after native compilation.

The full browser run exposed a startup measurement waiting for the second response
(`Plan recorded.`) while the native pipeline was still streaming its three Todo
calls. It implicitly imposed the UI assertion's five-second wait on the entire
producer pipeline, despite this measurement asserting only ordering. The test now
uses the emulator's existing `response_completed` count-2 barrier before the unchanged
UI assertion. Browser performance timestamps and ordering assertions remain intact;
no timeout value, assertion, retry, skip or screenshot tolerance changed. The complete
316-case rerun passed with all three startup samples.

## Review risks

Native signatures/encrypted replay remain provider-owned opaque state; validation
can establish preserved input and supported replay semantics, not reproduce a model
generation. Inspection archives intentionally omit exact private/native input and
are explicitly non-reconstructible. Logical byte/item limits bound allocations;
they are not an OS RSS sandbox. macOS-only platform coverage requires macOS CI.

## Three-finding follow-up at 3252f6a

The subsequent F1–F3 review repairs bounded prefix planning, accumulated WebUI
historical pagination and universal historical Derived provenance. The detailed
selection policy, cancellation checkpoints, compiler/durable ownership and evidence
are recorded in `context-engine.md` and `pr-477-architecture-review-repairs.md`.
These repairs do not change schema 59, App Server v48 or Runtime Client v69,
continuation/retry authority, activation lineage or transaction linearization.

## N1 candidate-search correction

The 64-boundary sampler introduced by the F1 performance repair could incorrectly
report `CannotFit` while an unsampled historical prefix fit. N1 replaces it with
complete monotone interval search for the production JSON-byte estimator and an
explicit `SearchIncomplete` outcome for bounded arbitrary-estimator searches.
Keep-recent preference remains largest feasible preferred boundary, otherwise the
earliest feasible fallback. A single rendered transcript and cached exact prices
bound production materialization to O(B log N). The subsequent production media
repair replaces dynamic estimator wrappers with immutable generation/operation
media capture and shared request materialization; supported image/upload histories
now retain complete DefaultTokenEstimator search. Search incompleteness
starts no Summary or Checkpoint and settles once through the existing Agent owner.
See `context-engine.md` for the proof and `pr-477-architecture-review-repairs.md`
for deterministic red/green and full-validation evidence. Schema/protocol ownership,
canonical history, cancellation grants, exact snapshots and retry versions remain
unchanged.

N1 local validation passed: workspace 4,178; all-features contracts 3,488 + 208;
all-features boundaries 200 + 432; emulator Python 54; Dev 41; TUI 1,025 with one
existing platform skip; Web 2,585 with four existing platform skips; pinned browser
acceptance 5/5. Both Clippy modes, generated drift, lane coverage, builds, typechecks,
i18n/provenance and sandbox checks passed. After strengthening cancellation to a
synchronous first-query hook, both complete Context owners passed 83 tests each and
both Clippy modes passed again. Full command/count and intermediate-failure evidence
is retained in the repair record. Remote CI must be checked on the pushed N1 HEAD.

## Production media candidate discovery repair

Image, Session-upload and mixed-media 129-boundary Agent/SQLite regressions reproduce
the former incomplete search and now select the sole feasible boundary 2. Production
media uses bounded frozen upload paths and the union of historical/primary selected
images, with no candidate-time I/O. Shared upload-then-image materialization prices
the exact provider-neutral representation used by request staging. Native replay,
request snapshots, retries, canonical facts and transactional commit authority keep
their existing owners. An independent exhaustive fixture oracle and suffix tests
cover newest-16, repeated references, fixed Context, text-only projection and Derived
media. Work counters and resolver-triggered cancellation prove finite work and
no unauthorized subsequent reads/requests/commits. See the existing repair record
for final validation evidence; no schema/protocol version changes are required.

Production-media local validation passed on settled source: workspace 4,189;
all-features contracts 3,499 + 208; all-features boundaries 200 + 432; emulator Python
54; Dev 41; TUI 1,025 (one existing platform skip); Web 2,585 (four existing platform
skips); pinned browser acceptance 5/5. Both Clippy modes, native builds, Linux lane
coverage, protocol drift/typechecks, Web build/i18n/provenance and sandbox checks
passed. One repeat's managed-package setup failed fetching PyPI rich metadata;
the unchanged isolated test and complete workspace rerun passed after endpoint
recovery. Exact commands/counts and failure evidence remain in the repair record.
Current-head remote CI is reported on PR #477 after the final push.

## Final request-admission, wire, observation and accounting repair

Final semantic preparation now mandates complete adapter replay/translation preflight
before input freeze and request-start commit, including no-continuation and recovered
inputs. Transient retries reuse the validated frozen version. Captured translations
verify that the existing shared Tool correlation encoder agrees with frozen occurrence
associations for Original/Derived combinations and supported native continuations;
unsafe encrypted replay remains rejected. Historical observation pagination/detail
selects eligibility inside durable SQL before hydration, with authoritative continuation
cursors under one short read snapshot. Default Tool pricing and media fingerprints use
the existing bounded Tool projection rather than execution metadata.

The JSON-byte monotonicity proof remains sound after message-local Tool normalization,
with no candidate-time resource I/O. The 129-boundary N1/media regressions and generic
non-monotone estimator contract remain required owner coverage. No canonical facts, write transaction boundaries, cancellation gates or lineage
ownership are changed. Nullable Tool wire correlation for fully native-covered input
requires App Server v49 / Runtime Client v70 and regenerated current-only artifacts. Exact validation evidence is maintained in the existing PR repair record.


### Final PR #477 runtime contract repair

F1–F4 now establish mandatory final adapter admission before Snapshot freeze/start,
adapter-owned nullable actual wire correlation (App Server v49 / Runtime Client v70),
durable historical eligibility before body hydration, and bounded model-visible Tool
pricing/fingerprints. Canonical facts, COMMIT/start arbitration, frozen retries and
lineage owners remain unchanged. Eight new deterministic owner tests and the complete
local matrix passed; exact commands, counts, intermediate repairs and proofs are in
[the existing architecture repair record](pr-477-architecture-review-repairs.md#settled-f1f4-local-validation).
Exact pushed-HEAD CI is reported in PR #477 independently of local validation.
