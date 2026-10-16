# Issue #467: bounded observation and transport isolation

This ledger records the RFC v3 implementation at the existing execution, durable,
publication, Runtime Client, connection, registration, and transport owners.
The pre-review PR HEAD `6f9cfbdb3ebd4b145ba5d54c044a91b0832fada3` had local validation passing, including the full Native, client and
317-case browser commands recorded below. The browser filesystem qualification
and unavailable local macOS checks remain explicit. Earlier epoch sections are
historical diagnostics; they do not override the pre-review results or imply
current repaired-source or remote CI success.
The normative contract is in [invariants](invariants.md), the public methods and
lock order in [App Server protocol](app-server-protocol.md), and source/read-cut
semantics in [conversation reading](conversation-reading.md).

## PR #478 repair and latest-main integration

Historical validation at `b0928347`: the repaired source passed **4,784 Rust tests** (zero failures,
ten existing ignores) with `RUST_TEST_THREADS=1`, including the new publication
contention tests and corrected real-process readiness handshakes. Earlier merged production/client validation passed **317 browser cases** under
the filesystem qualifications recorded in the historical sections. That is not
a passing Web result for `b0928347`. Strict Clippy, binary build and test-lane
discovery passed for that historical source.
That head’s Linux contract and boundary lanes passed. Its Web lane
passed 315 browser cases but exceeded the unchanged preparation budget in both
300-Attempt reading fixtures; bounded failure diagnostics now retain their exact
phase and original native process counters. Local exact preparation runs completed
in 208.378s and 223.145s, which do not override the CI failure. The merge macOS
lane exhausted its existing 70-minute compilation budget; the follow-up serializes
compiler jobs and completed native compilation. Its library run found three
fixture-root failures (3,178 passed): raw macOS TempDir ancestors hit the intended
no-symlink reader admission before file-identity checks. The fixtures now use
their exact canonical roots; production admission and assertions are unchanged.
The external macOS targets were not reached and a new platform run is required.
[Repair history](issue-467-repair-history.md) records every intervening failure
and environment qualification. PR #478 remains open for independent review and
must not be merged by this task.

### Current review corrections (local validation complete)

The current correction starts at `b0928347`. Current-attempt cancellation now
selects and cancels under the native coordinator mutex shared with admission and
settlement; the Host never consults a published Attempt or prepares observation
to authorize that control. Exact-ID cancellation retains its native identity check.
Manual compaction maps all five fields from that invocation’s native
`ManualCompactionOutcome`, without reading the latest presentation context.

TUI and Web cancellation dispatch use the exact live attachment authority even
when its observation is unavailable or behind. They do not treat a projected
Attempt as the command target. Web binds its acknowledgement marker to the
returned native Attempt identity, and cancellation does not request or await a
checkpoint refresh. The existing contiguous observation fold owns settlement.

All four `runtime_client::host::tests::native_cancellation_tests::` and four
`runtime_client::host::tests::compaction_head_tests::` tests pass on the combined
source. Cancellation proves exact settled-A display versus active native B,
a held native cancellation receipt across successor admission, real source
unavailability, and an actually blocked physical read plus held preparation lock.
Compaction proves actual optional-source failure, valid consecutive compactions
with a held first receipt, and a lost JSONL response over a Tokio duplex connection.
The latter is not an OS-pipe test. Model-call and native terminal counts remain exact.
The unchanged reading fixtures and complete local validation now pass as recorded
below. Remote results are recorded in [PR #478 checks](https://github.com/Caismis/rustX/pull/478/checks)
for the exact pushed commit; historical results are not proof for the repaired source.

The current incremental Request Source shares augmentation on the native
selected-input tree. It retains one finite Request root and its original capture
witness instead of recopying every historical descriptor, field row and content
segment on each append. The former transient fragment import, message-shape
packing and `OriginalSpan` request-header implementations have been removed.
The independent flat byte/header producer remains a test oracle. The current
contracts and validation requirements are recorded under
[bounded work and source identity](#bounded-work-and-source-identity).

### Integrated native and client validation (review202)

On the frozen integrated source,
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features`
passed **4,821 tests, zero failures, nine existing ignores** in 3,272.496s.
The library contributed 4,140 passes/four ignores; external provider tests
contributed 179 passes/five ignored credentialed tests. All 47 subagent tests
passed, including `definitions::skill_version_identity_is_frozen_across_the_boundary`.
No live provider test was enabled.
`cargo run --all-features --example check_test_lanes` passed and confirmed
Linux contract/boundary discovery and coverage. Strict all-target/all-feature
Clippy had passed on this same production/test source in review200.

The preceding four-worker run passed 4,137 library tests but failed three initial
file-read fixture setups under their unchanged 30s guard. All five file-read
boundary tests then passed in isolation in 7.93s, and again in the complete serial
run. Their prior failure evidence and unproven cold-validator/contention hypothesis
remain in the repair history; the serial pass does not claim arbitrary parallel
scheduling robustness or final-head CI success.

Client checks passed: protocol generation/drift and TypeScript, dev TypeScript
and 41 tests, TUI TypeScript and 1,163 tests with one existing skip, and Web
TypeScript plus 2,828 tests with four existing skips. The isolated canonical Web
production build, i18n and provenance checks passed. The fake provider suite
passed 55 tests. Both original reading selectors then passed independently on the default
filesystem with the pinned browser, 300 Attempts and unchanged 240s setup limit:
distant-reading one pass in 3.5m (page assertions 4.1s); Fork-reading one pass
in 3.2m (page assertions 6.2s). Wrapper times 260.172s/193.437s include native/web
setup and are not the fixture durations. The complete `pnpm --dir web-console test:e2e` run then passed all **317 cases**,
zero failures/skips, in 23.2m (wrapper 1,401.974s), on the default filesystem.
Both original reading cases passed again in that full run. No fixture timeout,
workload, screenshot baseline or assertion changed. Remote CI must be read from PR #478 checks for the exact pushed commit;
these local results do not substitute for it.

### Native command regression evidence

| Requirement | Exact deterministic selector | Controlled interleaving and evidence |
| --- | --- | --- |
| R1-A: stale settled display, native successor active | `runtime_client::host::tests::native_cancellation_tests::cancellation_selects_native_successor_while_real_source_fault_retains_settled_display` | Native A settles, then a real SQLite trigger refuses optional event-witness capture during genuine B submission/execution. The original published settled-A attempt remains exact while the provider request-count watch proves B was invoked. A separate thread holds observation preparation before the actual cancellation RPC. Its correlated response names native B; original J contains exactly one Completed terminal for A and one Cancelled terminal for B; two model calls total. Unrelated nonattempt cursor advancement is permitted. No native fact, canonical write, or admission receipt is fabricated. |
| R1-B: successor cannot inherit cancellation | `runtime_client::host::tests::native_cancellation_tests::held_cancellation_receipt_keeps_selected_identity_after_native_successor_admission` | A cfg hook signals only after native cancellation has returned and physically holds the RPC response. The retained native settlement receipt proves A cancelled, then real successor B reaches its parked provider call. Releasing the response returns original request ID 51 and A's identity. A stale exact native cancellation naming A is refused. Releasing B's model gate yields Completed, with one original terminal per attempt and exactly two model calls. |
| R1-C: unavailable observation is not a cancellation prerequisite | `runtime_client::host::tests::native_cancellation_tests::native_cancellation_remains_available_during_real_observation_source_failure` | The original occupancy Source BLOB is made unavailable through SQLite; the actual subscriber reports typed Unavailable. With observation preparation held, the correlated cancellation RPC succeeds for the exact active native attempt while observation remains unavailable. The native settlement receipt and original J prove one Cancelled terminal, with one model call. The original BLOB is restored for cleanup. |
| R1-D: blocked physical observation read | `runtime_client::host::tests::native_cancellation_tests::native_cancellation_does_not_join_or_cancel_the_blocked_physical_observation_read` | An existing physical-reader hook signals actual blocked Source work. Observation preparation is also held. Cancellation succeeds and the exact native attempt settles Cancelled while the reader's cancellation signal is still absent. Only subsequent explicit cleanup cancels/releases and joins that original read. One model call and one original Cancelled terminal. |
| R2-A: committed receipt despite unavailable published context | `runtime_client::host::tests::compaction_head_tests::manual_compaction_receipt_survives_unavailable_published_context` | A real optional `projection_activations.observation_header` capture trigger fails inside its savepoint. Native compaction J and the immutable checkpoint still commit; published latest compaction remains old/absent and checkpoint reads report typed Unavailable. The successful response exactly matches the one original CompactionCompleted outcome and native checkpoint/activation; two model calls and no extra compaction. |
| R2-B: late A response after valid B commit | `runtime_client::host::tests::compaction_head_tests::manual_compaction_wire_receipt_keeps_a_after_a_valid_successor_b_commits` | A oneshot signals native A's committed outcome and holds its response. A genuine intervening Attempt supplies new history, then B commits through the real endpoint. The published context names B. Releasing and JSON-encoding/decoding A's endpoint response still gives request ID 41 and A's exact original outcome; B's response has ID 42 and B's outcome. Two original compactions and four model calls remain exact. |
| R2-C: lost response, reconcile without replay | `runtime_client::host::tests::compaction_head_tests::manual_compaction_lost_stdio_response_reconciles_without_replaying_operation` | The real JSONL serve/dispatch/write path runs over `tokio::io::duplex`, not OS pipes or a spawned stdio process. After the native committed-outcome signal, the client is dropped before the held response is released; the writer reports OutputBrokenPipe. Reattachment and the existing checkpoint reconcile that same outcome without another CompactContext request. Original compaction count stays one and model calls stay two. |
| Existing synchronous compaction publication contract | `runtime_client::host::tests::compaction_head_tests::committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked` | The actual worker is parked before the second compaction. Its checkpoint and contiguous replay already carry the exact second native head, Source cut and J frontier; one ContextCompacted replay fact, two original completion events and four model calls. Unparking preserves the same head. |

Client cancellation evidence on review189 includes TUI
`native cancellation forwards under stale unavailable observation and returns its actual owner`
and Web `unavailable stale A forwards cancellation and retains native B receipt without a repair read`.
They inject unavailable observation while keeping the real attachment authority,
then assert the exact native acceptance identity and no checkpoint request.
`native B acknowledgement survives stale A folds and permits a genuine observed successor`
holds the receipt/observation order and proves unrelated stale-A publication does
not mislabel or discard B's acknowledgement. The ordinary-RPC saturation test
keeps those slots occupied while cancellation resolves through its reserved lane,
with zero observation rereads. Retired attachment dispatch remains refused.

The earlier 30-test fragment group and eight request-shape passes describe the
pre-incremental implementation. Its header-emission and Native imported-shape
selectors were removed with that implementation and are not current acceptance
proofs. Current Source tests compare the composed representation with independent
flat serialization, every field, page and original range, and preserve reopen,
corruption, native-commit and original-witness checks. Native default structural
encoding, path and physical source-copy tests remain active.

On the compiled epoch-153 artifact, seven of the eight migrated request-shape
tests passed. The remaining test imposed a global successor-capture failure after
a selected leaf was corrupted; the current test instead proves that the successor
retains the original expected hash and that selecting the corrupted leaf still
fails. That test correction and the subsequent removal of retired code require a
fresh integrated run. The exact 20-by-4 Journal/rank admission matrix, normalized
Request/archive paths and persistent-manifest scaling tests are current selectors
below; no historical full-run result validates the final combined source.

### Repository and ownership

The repair starts from PR HEAD `6f9cfbdb3ebd4b145ba5d54c044a91b0832fada3` and merges
`origin/main` at `8cb41624eb2dc2ca87e4409961f42553ceb029cf` into the existing
`issue-467-bounded-observation` worktree. Main contributes PR #477 / Issue #466's
immutable-history model-context projection. The old base was
`97096020d64ea00218493a0a99b9b805821eeb28`. No unmerged PR is a dependency.

Execution and canonical history retain their existing owners. Original/Derived
provenance and frozen Attempt model input remain authoritative; display reads do
not reconstruct deprecated canonical compaction summaries. The original Completed-J
occupancy witness carries a finite projection head, so compaction head and context
publish atomically before asynchronous refinement. Lost commitment acknowledgments
reuse that original receipt, without repeating semantic work.

Composition shutdown first settles the semantic runtime, then closes presentation
reads and joins the original Projection Worker and every outstanding physical
Source read. The read previously could retain the Store Arc and exclusive product
storage lock after semantic shutdown. Cancelling a shutdown waiter now leaves the
original JoinHandle in its supervised slot; another waiter joins the same task.
A separate physical-read slot survives worker panic/abort and is drained before
successful ownership release. Dropping a handle is not claimed as physical proof.
Runtime Manager retirement waits for admitted operations and supervised drain;
internal inspection tasks retain their JoinSet/capacity ownership until termination.
Explicitly retained external leases still own storage until released.

The subsequent browser run exposed an admitted Observe wait that outlived listener
stop. Inspection shutdown now permanently retires its exact endpoint registrations
before cancelling socket readiness and joining all connection tasks. Retirement
shares initialization's identity lock, then detaches at the native subscription
lock; a racing Observe either closes or fails admission. Physical Source reads are
still awaited. The shutdown method borrows its owner and retains the original
listener handle across cancellation. Listener/connection join failure remains an
explicit persistent failure after surviving tasks drain; it is not release proof.

Source failure is typed observation unavailability, distinct from Replay eviction.
A retained historical checkpoint is not evidence that the current native source is
serviceable. Checkpoint/subscription admission consults current serviceability;
failed native reads fence the affected registration without inventing a cursor or
execution outcome. Only a successful read at the current dependency fence clears
the failure. Explicit recovery requests one bounded read through the existing
Projection Worker; concurrent requests at the same fence share that work. WebUI
and TUI preserve Applied, stop automatic resynchronization, and fence late results
by original registration/activation. Real Child Unix recovery preserves activation
and never falls back to a second Parent projection or semantic replay.

Current independent versions: App Server **50**, WebSocket **rustx.app-server.v50**,
Runtime Client **71**, SQLite **65**, Runtime Event **1**, Subagent Control **33**,
private Product Host **4**, Session catalog **16**. Generated contracts agree.

### Bounded work and source identity

Schema 65 replaces the prior per-Request copied segments and shape packs with
one native persistent selected-input tree and optional Source augmentation on
those same nodes. Request metadata binds the exact root; full historical input is
materialized only by explicit consumers. Publication and terminal checks use a
metadata-only path without selected-message hydration. Native semantic SHA and provider input identity
remain unchanged. SQL mutation certificates and post-INSERT validation retain
canonical trigger rejection and rollback behavior.

The same immutable compiled root now survives staging, selected-input assembly,
context anchors, default measurement, native Request commitment and provider input.
Exact incremental SHA state extends only for a certified append; edits, reordered
selection, changed media and compaction invalidate that evidence. Public fingerprint
algorithms and flat provider wire encoding are unchanged. Arbitrary estimators
still receive the whole original input; only the established JSON-byte estimator
uses the exact aggregate with its original rounding. Post-COMMIT evidence binds
the exact prepared values rather than reconstructing the whole Request a second
time. Canonical contributor snapshots separately share the existing Ledger bodies
through the Surface's exact selection; they never substitute model Derived input.

Incremental selected-input and Source maintenance does not make all request work
constant. Provider request materialization and exact wire fingerprinting still
visit actual input. Exact per-Request Tool association metadata is also serialized
into each Request and decoded by publication open/terminal/settlement checks;
ordinary frame staging does not perform that metadata read. The native validated
association proof removes repeated canonical-body validation, but does not remove
this metadata cost. Production observation Source omits the association array and
serves its own bounded authenticated pages. The scaling evidence must distinguish
these costs rather than claim constant work for an entire Tool-bearing request.

Source roots retain original capture witnesses and bind exact native identities.
Fragments are authenticated pages capped at 64 KiB, with fragment depth below 16
and native path height at most 32. Range reads emit at most 64 pieces and return an
advancing partial chunk when needed; previews admit at most 512 pieces and retain
the 32-node display budget. The old schema-64 two-segment bound is retired rather
than attributed to composed reads. Tests count actual page, node and metadata
work and compare every original byte, field and continuation against the old
serialization oracle. Optional Source failure still rolls back independently of
native Request, canonical and Journal commitment. Authenticating a reused subtree
also follows its original capture path: worst-case node work can scale with
changed-path depth times origin-path depth, each bounded by 32. The implementation
does not claim a single-path visit count or maintain another cross-request cache.

The source-copy path pins one original file owner, reads at most 64 KiB at a time,
and copies only its certified extent. Physical ownership remains with that read
until termination. The new incremental storage tests and browser scaling evidence
must pass on the integrated source before these changes are treated as validated;
previous full-run passes below belong to the earlier source.

Prepared capability and Trace heads retain bounded normalized DTOs and their exact
encoded/conservative admission charges under their original owner. Changed records
replace that owner; held checkpoint seeds retain the previous rows and cut. Lazy
Trace preparation occurs off the Projection lock. Final full-record bounds,
checkpoint fitting, frontiers and normalization still apply. Exact UTF-8 string
admission charges raw bytes once plus escape expansion, with an original-byte-walk
oracle at every budget boundary.

### Deterministic repair evidence

The original repair selectors below were discovered and executed by final109's
complete Rust command; later opening-adoption tests are also covered by repair112
and final113. The latest full-run results remain distinguished below. The original T01–T20 matrix
below remains the architecture-wide mapping.

| Test selector | Boundary and synchronization evidence |
| --- | --- |
| `composition_shutdown_retry_joins_physical_read_before_immediate_recomposition` | Actual blocking Store read signals entry and logical cancellation; channel release gates physical termination. Cancelling the first shutdown waiter retains the original join handle. Retry stays pending until release, then weak owners expire and immediate real-lock recomposition succeeds. |
| `explicit_checkpoint_repairs_a_real_source_fault_on_the_original_worker_without_native_work` | Actual SQLite finite occupancy-source fault held through public checkpoint/subscription refusal; exact original BLOB restoration followed by public checkpoint wakes the original worker, publishes one read transition, preserves native cuts and makes zero model requests. |
| `unix_checkpoint_source_repair_keeps_the_current_thread_executor_serviceable` | Real Unix socket and JSONL server on a single-thread executor prove that checkpoint recovery yields the executor while the original Projection worker completes; persistent SQL fault stays unavailable and restoration advances one display transition without semantic work. |
| `projection_drain_joins_residual_read_after_worker_panic_and_waiter_cancellation` and `projection_drain_joins_residual_read_after_worker_abort_and_waiter_cancellation` | An actually blocked Source read retains its physical permit across worker failure and cancellation of the first drain waiter; retry stays pending until channel release, then preserves the worker outcome and proves both handles joined. |
| `committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked` | Two real native compactions with the read worker parked at the second completion; exact Completed-J, checkpoint cut and full contiguous replay already contain head activation two before background refinement. |
| `source_failure_is_unavailable_until_same_cut_native_read_repair` | Failed native read after a published checkpoint, no intervening semantic event, typed checkpoint/subscriber refusal, and successful same-cut source repair. |
| `read_failure_requires_successful_source_read_at_the_current_dependency_revision` | Installation under the Projection lock rejects obsolete read results; only the matching dependency fence clears source failure. |
| `failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut` | Native failure wakes observation without fabricating a cursor or execution fact. |
| `t18_child_source_failure_recovers_over_unix_at_the_same_activation_without_native_work` | Real Unix inspection transport preserves Child activation, typed unavailability and original source identity across repair without cold fallback or semantic work. |
| `manual_compaction_lost_commit_ack_publishes_the_original_receipt_after_owner_release` | Actual SQLite post-commit failure and held semantic owner prove original receipt publication ordering, unchanged canonical history and one compaction transition. |
| `giant_original_provenance_retains_scope_digest_and_exact_canonical_metadata_source` | A large original identity retains ancestor conversation scope and digest while its local canonical locator remains authoritative for deferred metadata. |
| `derived_provenance_admits_exact_byte_bounded_prefixes_without_copying_giant_aliases` | Combined encoded-byte admission retains exact provenance prefixes and explicit truncation. |


Opening-adoption regression selectors (six Store tests and one production-wire/Fork test):

| Selector | Proof |
| --- | --- |
| `uploaded_frozen_request_retains_the_exact_original_retry_boundary` | Real idle adoption, AttemptStart and upload-rendered Derived input; native retry identity/C1, reconstructed model request, RequestStart frontier and exact replay receipt remain original. |
| `retry_opening_source_keeps_native_frozen_input_validation` | Forged canonical digest fails before native RequestStart; adoption cannot bypass model-input integrity. |
| `uploaded_retry_deferred_identity_uses_the_same_original_anchor_as_its_locator` | A giant opening identity is deferred; both requests around later steering retain C1 and exact original UTF-8 ID bytes. |
| `retry_eligibility_uses_original_adoption_and_native_user_classification` | Nine source/content cases preserve Human/delegated-Agent text eligibility and exclude unadopted context, notices, Unicode whitespace and text-free media. |
| `retry_capture_ignores_optional_display_holes_and_other_attempt_steering` | Actual deletion of the optional C header cannot refuse native R/J or change captured opening identity; selected display read returns its exact missing-source error, C1 remains, and no hidden repair occurs. Previous-Attempt steering never opens a later Attempt. |
| `a_projection_summary_without_adoption_cannot_invent_a_retry_opening` | A real committed context projection and frozen Summary without adoption confer no retry authority. |
| `opening_adoption_retry_is_identical_in_live_wire_and_fork_lineage_after_steering` | Opening A, first request, later steering B, second request and settlement use the production bounded transcript serializer; real native lineage/Fork bootstrap preserves original A/C1 and execution origin without executing the copied Attempt. |

Additional bounded-work regression selectors follow. Unqualified composed-Source
selectors use `durable::sqlite::tests::request_fragment_tests::`; the three
unqualified same-transaction validation selectors and normalized load/archive
selectors use `durable::sqlite::tests::request_snapshot_validation_tests::`.
This is a current proof directory, not a claim that the final integrated run has
completed.

| Selector | Proof |
| --- | --- |
| `durable::sqlite::tests::request_fragment_tests::t08_t12_t14_t19_t20_canonical_fragments_match_full_source_every_field_and_original_range` | Independent flat producer versus composed Source: every original byte, selected field and advancing page/range. |
| `t08_t12_t19_parametric_request_holes_reject_root_corruption_before_allocation_and_restore_exactly` | SQL admission bounds and exact restored root metadata; no corrupt-reference expansion. |
| `t10_t12_t19_parametric_hole_identity_cannot_be_rehashed_or_laundered_into_new_requests` | Original capture witness prevents rebasing and later-request laundering. |
| `t10_t14_t19_parametric_root_insert_failure_preserves_native_request_and_recovers_missing_augmentation` | Optional-index savepoint failure leaves native commitments intact. |
| `t08_t19_exact_j_rank_pair_variants_preserve_admission_and_every_original_byte_at_the_16k_boundary` | Independent full producer for all 80 Journal/rank width pairs, including equal-width sums with different admission outcomes. |
| `t08_t14_t19_certified_request_roots_finalize_once_with_exact_legacy_geometry_and_witness` | Current composed root geometry and original witness versus the independent flat oracle. The historical selector name remains; retired packed-shape finalization is not exercised. |
| `t10_t14_t19_certified_root_finalization_failure_rolls_back_only_optional_source` | Actual root-insert failure trigger and healthy subsequent capture; no native rollback. |
| `model::selected_input::tests::selected_append_insert_replace_share_old_leaves_with_logarithmic_actual_node_work` | Persistent-root sharing and actual leaf/branch allocation counters at 100, 300, 1,000 and the 8,192-item boundary. |
| `durable::sqlite::tests::manifest_store_tests::native_manifest_warm_store_appends_are_delta_and_depth_bounded_at_100_300_1000` | Native warm append work and original full-input identity, without historical input reconstruction. |
| `durable::sqlite::tests::request_snapshot_validation_tests::normalized_request_metadata_publication_never_hydrates_selected_input_bodies` | SQLite authorizer refuses original input-body reads while scalar publication validation succeeds; explicit historical reconstruction still requires those bodies. |
| `normalized_request_resource_budget_charges_metadata_and_the_exact_flat_input_array` and `archived_normalized_request_preserves_the_exact_original_flat_snapshot` | Full-load budget boundary and exact archived original flat Snapshot JSON after normalized storage. |
| `normalized_request_metadata_budget_rejects_before_returning_any_owned_json` | SQL rejects metadata above the caller budget before returning an owned String; counters distinguish metadata rejection, exact metadata admission and full-budget reconstruction. |
| `normalized_request_digest_guards_every_reader_before_materializing_malformed_cells` | Every native Request digest reader admits only a 64-byte text cell; corrupt-size/type rejection returns zero owned digest bytes, preserves J and retains exact reconstruction after restoration. |
| `t14_t19_t20_provider_outcomes_decode_the_same_transaction_snapshot_once` | Old two-read versus new one-read materialization counters, for success and failure. |
| `t14_t19_snapshot_reuse_preserves_original_provider_validation_errors_and_precedence` | Independent original-path oracle across twelve malformed/foreign/duplicate cases. |
| `t14_t19_t20_publication_p_u_c_and_audit_each_reuse_one_validated_snapshot` | Actual public P/U/C/audit transactions retain all native start/outcome checks. |
| `runtime_client::checkpoint::prepared_capability_tests::` | Three exhaustive budget/normalization/exact-source-owner tests. |
| `runtime_client::checkpoint::prepared_trace_tests::` and `prepared_trace_owner_reuses_exact_rows_and_retires_after_native_terminal_install` | Four tests covering every budget, rejection/count frontiers, serde normalization, real native Tool transitions and held old cuts. |
| `runtime::read_layout::source_copy_tests::` | Four tests: exact original extent, bounded writes, actual file/index open counts, EOF/destination failure and owner release. |
| `t01_t15_string_budget_matches_original_byte_walk_at_every_boundary` | Original UTF-8/escape admission oracle for every surrounding budget. |

The original model no-op test now establishes the initial published Eligible state
before admitting an Attempt; waiting for that obsolete state afterward could miss
a coalesced transition. Background conformance preserves the initial dispatch
assertion, then waits for Running on the exact parked Job before reading status;
Starting→Running legitimately changes its native collection cut. Neither fix
retries mutations, accepts expired cuts, changes a deadline, or removes an assertion.

A retained-progress timeout occurred in earlier full runs while the same module
passed alone. Failure-only diagnostics now read one native J frontier and at most
12 indexed facts, with the original 10-second guard and all 256 retained facts
unchanged. Production progress semantics were not changed. Run 102 passed the
unchanged workload in the complete suite; earlier failures remain recorded in the
[repair history](issue-467-repair-history.md), not relabeled as passes or proven
infrastructure failures. Remote CI remains an independent required gate.

### Browser-discovered integration repairs

Browser103 exposed three distinct issues, without changing its existing assertions:

- A new Observe after RuntimeShutdown waited for its ten-second deadline while
  the inspection listener was trying to join connections. Exact endpoint retirement
  now wakes the pure wait while physical Source work remains joined.
- Upload rendering in main produces a Derived input, and later steering can
  change the last frozen input within one Attempt. Retry authority must instead
  come from the original opening adoption, shared by live and inherited responses.
  The final correction captures that original native identity inside request-start
  using canonical eligibility and adoption order, independently of optional display
  sources. Frozen Original/Derived model context cannot grant retry authority. The
  same captured anchor supplies the optional deferred retry-ID field.
- The exact Compaction Completed-J bootstrap already held its original bounded
  summary, but Trace assembly discarded it. Trace now retains that original preview
  and truncation flag. A later activation cannot replace historical content. No
  canonical summary Message, new read authority or schema change is introduced;
  truncated Summary/Raw views explicitly show Content deferred.

The added selectors are `runtime_client::endpoint::retirement_tests::` (three),
`local_runtime::live_inspection::shutdown_tests::` (two),
`t20_real_unix_shutdown_retires_the_exact_pending_observe_before_its_deadline`,
`request_retry_tests::` (six),
`runtime_client::response::tests::retry::` (one), and
`trace_preview_tests::` (two).
The existing `t20_real_unix_shutdown_joins_admitted_physical_source_worker` now
also cancels and retries the original shutdown waiter. These tests use installed
native subscriptions, immediate future polls, exact channels, actual Unix sockets
and native transaction facts. Web component tests add four summary identity and
truncation cases. Repair107 passed all twelve selected native tests (eleven new and one extended),
strict Clippy, binary build and all-target compilation. Browser108 passed all six
originally failed cases unchanged. Browser103 remains a failed historical run.

### Current validation commands

Final127's complete Rust command passed **4,784 tests, zero failures, ten
existing ignores** in 2,212.844 seconds: 4,103 library, 661 external-target and
20 example-target passes. The exact original failing test passed within the
47-test Subagent target and had also passed alone. All 237 native and 451 external
boundary tests are included; these subsets do not add distinct tests to the total.
The earlier final121 run passed all 4,782 then-existing tests at four libtest
threads before the two publication-barrier regressions were added.

Rust repair commands use `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4` and
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. Earlier runs used `RUST_TEST_THREADS=4`;
final126/127 use `RUST_TEST_THREADS=1` to separate cross-test storage contention
from each test's internal concurrency. Per-test budgets, barriers, work and
filesystem remain unchanged. Real provider APIs were not invoked. Existing
opt-in/credentialed ignores and client skips remain; none was added.

The two new `runtime_client::host::tests::settlement_tests::` cases positively
observe lock contention at drained-preparation and in-progress encoding before
releasing each boundary. Both pass alone and in the full command. The exact
cross-process ownership test also passes with serial libtest after child harnesses
explicitly select terse output. Failure-only diagnostics retain exact request,
publication and deadline evidence without issuing recovery operations. The
intervening failures, direct Linux I/O wait evidence, and macOS compiler timeout
are preserved in the repair history, not classified as proven external failures.

Client/production behavior is unchanged by the final cfg-only test repairs;
clients114 and the complete browser120 remain the local client evidence. Remote
Web at merge HEAD also passed all 317 browser cases and 2,823 unit cases. The
macOS compiler job now runs one Cargo compiler process at a time, retaining its
original debug level, libtest concurrency, selectors, assertions and 70-minute
budget. The next pushed-head CI result is required to validate that resource change.

| Exact command | Result | Repair run |
| --- | --- | --- |
| `git diff --check` | Passed | final127 final inspection |
| `cargo fmt --all -- --check` | Passed (4.530s) | final127 |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed (73.846s) | final127 |
| `cargo build --bins` | Passed (79.229s) | final126; subsequent source changes are cfg-only |
| `cargo test --all-targets --all-features --no-run` | Passed (147.206s) | repair107 |
| `cargo test --lib --all-features runtime_client::host::tests::settlement_tests:: -- --nocapture` | 2 passed (160.844s including compile) | repair122; both also pass final127 |
| `cargo test --lib --all-features scripted_suites::runtime_client::protocol:: -- --nocapture` | 14 passed (0.566s) | repair122 |
| `cargo test --lib --all-features local_runtime::session_runtime_manager::tests::model_selection_noop:: -- --nocapture` | 6 passed (1.379s) | repair122 |
| `cargo test --lib --all-features local_runtime::session::tests::deletion_tests::deletion_cross_process_parent_death_preserves_nested_ownership -- --exact --nocapture` | 1 passed (164.542s including compile; 0.58s execution) | final127, serial libtest |
| `cargo test --lib --all-features runtime_client::endpoint::retirement_tests:: -- --nocapture` | Passed (160.112s) | repair107 |
| `cargo test --lib --all-features local_runtime::live_inspection::shutdown_tests:: -- --nocapture` | Passed (0.273s) | repair107 |
| `cargo test --lib --all-features t20_real_unix_shutdown_ -- --nocapture` | Passed (0.479s) | repair107 |
| `cargo test --lib --all-features request_retry_tests:: -- --nocapture` | Passed (177.713s) | final113 |
| `cargo test --lib --all-features trace_preview_tests:: -- --nocapture` | Passed (0.313s) | repair107 |
| `cargo test --lib --all-features runtime_client::response::tests:: -- --nocapture` | Passed (58.444s) | repair112 |
| `cargo test --lib --all-features scripted_suites::agent::tool_progress:: -- --nocapture` | Passed (164.508s) | final121 |
| `cargo test --all-targets --all-features` | Passed (2,212.844s), 4,784 tests | final127, serial libtest |
| `cargo test --all-features --test subagent definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact --nocapture` | Passed (0.843s) | remaining117 |
| `cargo test --all-features --test subagent` | Passed (35.549s) | remaining117 |
| `cargo test --lib --all-features -- boundary_suites::` | Passed (80.332s) | remaining117 |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Passed (149.023s) | remaining117 |
| `cargo run --all-features --example check_test_lanes` | Passed (16.182s) | final127 |
| `pnpm --dir protocol/app-server check` | Passed (79.945s including generator compile) | final127 |
| `uv run --frozen pytest` | Passed (0.359s) | remaining117 |
| `pnpm --dir protocol/app-server typecheck` | Passed (0.873s) | clients114 |
| `pnpm --dir dev typecheck` | Passed (1.400s) | clients114 |
| `pnpm --dir dev test` | Passed (2.730s) | clients114 |
| `pnpm --dir tui typecheck` | Passed (3.699s) | clients114 |
| `pnpm --dir tui test` | Passed (35.800s) | clients114 |
| `pnpm --dir web-console typecheck` | Passed (13.023s) | clients114 |
| `pnpm --dir web-console test` | Passed (35.241s) | clients114 |
| `pnpm --dir web-console build` | Passed (5.890s) | clients114 |
| `pnpm --dir web-console check:i18n` | Passed (0.879s) | clients114 |
| `pnpm --dir web-console check:provenance` | Passed (1.061s) | clients114 |
| `bash web-console/scripts/browser-tests.sh test/e2e/child-capabilities.spec.ts test/e2e/commands.spec.ts test/e2e/compaction.spec.ts` | Passed (195.696s) | browser108 |
| `pnpm --dir web-console test:e2e` | Passed (1499.771s) | browser120 |

Linux/macOS remote results must be checked against the exact pushed HEAD in
[PR #478 checks](https://github.com/Caismis/rustX/pull/478/checks). Old-head CI
failure is not represented as repaired-head success. The dedicated worktree and
branch remain available for review.

Earlier repair commands, failed runs, environment diagnostics and intermediate
results are preserved in [the chronological repair history](issue-467-repair-history.md).
That history contains obsolete intermediate statuses and versions; this section
is the current repair evidence. Pre-review records below are separately historical.

## Historical pre-review complete-command evidence

All Native validation used `CARGO_INCREMENTAL=0` and required Provider Emulator
availability (`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`). The full Rust command used
`RUST_TEST_THREADS=4`. Epoch numbers identify retained command logs, not versions.
Within this pre-review record, later changes after Native111 were confined to
clients/tests and documentation. The main integration and subsequent Native
repairs are recorded separately above.

| Command | Pre-review result |
| --- | --- |
| `git diff --check` | Passed (114; repeated after documentation edits). |
| `cargo fmt --all -- --check` | Passed (111). |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed (111). |
| `cargo build --bins` | Passed (111). |
| `cargo test --all-targets --all-features --no-run` | Passed (111); expected targets discovered. |
| `cargo test --all-targets --all-features` | Passed (111), 4,604 passed; four opt-in measurements, one corpus generator and five credential-dependent live-provider tests ignored. |
| `cargo run --all-features --example check_test_lanes` | Passed (111); every runnable Linux test assigned. |
| `pnpm --dir protocol/app-server check` | Passed (111 and final staged-tree rerun), including generated schema/TypeScript/fixtures drift. |
| `pnpm --dir protocol/app-server typecheck` | Passed (113). |
| `pnpm --dir dev typecheck` | Passed (111). |
| `pnpm --dir dev test` | Passed (111), 41 tests. |
| `pnpm --dir tui typecheck` | Passed (113). |
| `pnpm --dir tui test` | Passed (113), 1,153 passed / one existing macOS ACL skip. |
| `pnpm --dir web-console typecheck` | Passed (114). |
| `pnpm --dir web-console test` | Passed (114), 2,809 passed / four existing macOS Office skips. |
| `pnpm --dir web-console build` | Passed (114). |
| `pnpm --dir web-console test:e2e` | Passed (114), all 317 cases, zero failures/skips, 1,403.459 seconds; fresh tmpfs temporary data, as qualified below. |
| `pnpm --dir web-console check:i18n` | Passed (114). |
| `pnpm --dir web-console check:provenance` | Passed (114), 276 source records / 264 package notices. |
| `uv run --frozen pytest` in `test-support/fake-provider` | Passed (107), 54 tests; unchanged provider source. |
| `python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v` | Passed (107), 15 tests; unchanged notification source. |
| `node web-console/scripts/check-office-sandbox.ts` | Passed (107), actual Linux sandbox admission. |

Real Stdio/WebSocket, Child Unix IPC, storage/restart/range and publication gates
are part of the full Native and client commands, with separately executed selectors
recorded below. The standalone Office sandbox preflight also passed. macOS-only
execution is not available in this Linux workspace and remains a distinct CI gate.
Historical failed commands are retained in their epoch sections; their later
passing reruns do not erase the original diagnostic evidence.

## Ownership and commit boundaries

| Owner | Authority and boundary |
| --- | --- |
| Agent Loop and supervised Tool owners | Execution admission, cancellation, physical completion and terminal settlement. Observation teardown has no execution cancellation capability. |
| Conversation Runtime / Durable Store | Canonical messages, native Journal facts, pending inbound inbox and source retention. Finite observation layouts derive from the original transaction; a derived-layout failure cannot refuse or replace its semantic commit. |
| InteractionCoordinator / Child route owner | Pending interaction decision and cancellation authority. Immutable bounded byte layouts are observation evidence, not a second interaction owner. |
| Publication Plane | Assembler → Coalescer → Frame → durable staging commit → native observation → user release. Frame receipts prove the readable frontier; reads cannot flush, stage or settle. |
| Runtime Client Projection | One native cursor for a complete bounded display transition, finite checkpoint descriptors, native Replay and exact cut evidence. Source callbacks carry complete canonical replacement facts; delayed callbacks cannot roll a later published cut backwards. |
| Subagent Registry / inspection owners | Registry owns Child activation. Runtime Client `ClientInner` and the Unix listener own physical inspection capacity; logical cancellation does not release a permit still owned by a physical task. |
| App Server Connection | Root attachments, connection-local request associations, authorization and delivery coordination. Exact admission tokens fence delayed cleanup; no durable idempotency registry. |
| Registration owner / Writer | PendingReceipt → Following → RecoveryRequired → Retired. Final validation, bounded candidate selection, one non-suspending physical acceptance and finite disposition commit share one boundary. |
| Client observation state | Applied cursor after complete contiguous fold, exact target/activation/view fencing, finite byte caches. Eviction never changes Applied or native facts. |

Checkpoint capture freezes finite descriptors under Projection synchronization,
materializes outside it, validates the original native cut, and shares two attempts
and one five-second deadline. The Journal, canonical, pending and transcript source
frontiers remain distinct. Canonical replacement is complete or explicitly
unavailable when derived source evidence cannot be obtained. The loaded transcript window is a cache at its captured
cut; canonical commitment does not invent transcript rows or Tool completion.
Strict independent checkpoint/event fixtures compare every wire-visible field,
including empty block frontiers and C-before-J result facts.

Provider terminal outcome P, Publication terminal outcome U, and Canonical
acceptance C retain their original P → U → C ordering. Streaming release is a
separate per-frame boundary: the original durable frame transaction commits,
its finite receipt is installed as publication observation, then the producer
returns to user-facing release. A later canonical Tool result may exist without
its execution-terminal J fact, and a successful Tool J fact may precede result C;
neither is substituted for the other. Public reads cannot create either fact.

The concrete checkpoint owner is `ClientInner::materialize_checkpoint`: under
the published-state mutex it validates the published Source and clones the finite
`Arc<CheckpointSeed>`. After releasing the mutex, it builds the candidate,
materializes ContentBindings and explicitly requested Trace records, and fits
the bounded checkpoint. It then validates that exact cut under the published-state
fence before returning. `ReadDomainCut` remains part of native worker preparation,
not a new read-side projection owner. A failed candidate consumes the
same two-attempt/five-second budget, not a fresh deadline. Domain Page/Value,
transcript and ContentRead likewise select the already-published source; they
do not prepare the worker or drain its queued facts. The parked-worker proof is
`runtime_client::host::tests::pure_read_tests::t08_t14_t19_pages_and_value_reads_leave_native_progress_queued`.

| Finite boundary | Current owner and limit |
| --- | --- |
| Complete transport record | `app_server/transport`: 1,048,576 encoded bytes including envelope/ID/escaping. |
| Checkpoint | `runtime_client/checkpoint.rs`: 786,432 target bytes; item budget 16,384; ordinary page 32 items; native head 8 items; two attempts under one five-second deadline. Final encoding still checks aggregate size. |
| Source byte window | Ordinary segmented canonical/Journal/state sources: 65,536 returned decoded bytes and at most two finite segment rows. Composed Request Source: at most 64 emitted pieces per range, possibly a shorter positive chunk; authenticated pages ≤65,536 bytes, fragment depth <16 and native height ≤32. Previews admit at most 512 pieces under the 32-node display budget. Metadata has independent finite-column guards. |
| Native Replay | `runtime_client/projection.rs`: 4,096 records and 16 MiB; initial-cut evidence is charged to this owner and expires with its Replay cursor. A retained Trace cache separately pins only its actual represented cut. |
| Request admission | Per connection: eight Observe, six ordinary and two control associations. The existing shared AppServerHost residual-work counter separately reserves controls at `max_connections × 14` ordinary / `max_connections × 16` total; retired connections cannot free still-running operations. |
| Child inspection | `ClientInner.inspection_capacity`: eight physical inspection permits. One admission-time 15-second deadline survives Registry, Unix initialization, materialization and fallback. Blocking Source work owns its permit until physical exit; the Unix listener also bounds and reaps its workset. |
| Trace/UI working sets | Native Trace refresh admits eight interests; the Web cache keeps at most eight detail entries/16 MiB. A selected request holds one advancing child page and one replaceable ≤64 KiB original byte window. |
| Selected non-Request Trace detail | `trace/inspection.rs` and `trace_observation/inspection.rs`: explicit finite head, capped at 512 KiB; eight canonical heads initially; advancing selected pages at most 32 items. Original bodies use existing ≤64 KiB range reads. The complete transport encoder remains the final record bound. |

Native attempt admission publishes the frozen Admitted view and its model/settings
as one complete checkpoint transition before the Agent Loop starts execution.
It does not publish AttemptStarted. A prepared Started witness that arrives after
the exact native occurrence is already installed does not reset its disposable
progress or advance the published cursor. Terminal Journal facts still settle that
occurrence. Current-cut foreground pages may retain progress from the identical
published seed; historical pages reconstruct only their captured durable facts.

The acceptance lock order is carrier retirement fence → route table →
registration → attachment/native Projection.
The WebSocket writer acquires the Socket first, completes readiness/previous Flush,
then enters the registration boundary only for the final tungstenite handoff.
Replacement acquires neither Socket nor the carrier fence. Teardown marks the
carrier retired and releases that fence before closing connection routes.
No source I/O, readiness wait or Flush is held
under registration/Projection coordination. Stdio acceptance is its first positive
nonblocking write; that accepted JSON record and delimiter are then finished even
after replacement. Unaccepted records retire on EOF or shutdown without requiring
physical readiness. WebSocket Flush cannot retract an accepted Frame.

App Server is v48 (`rustx.app-server.v48`), Runtime Client v69, Subagent Control v33,
SQLite layout v58, and Session catalog v16. The independent private Product Host
file-read handshake is v4 (`rustx.product-host.file-read.v4`), with a finite
canonical locator replacing its obsolete message-ID-only source. Runtime Event
envelope remains v1.
These are independent domains; the new storage layout indexes existing authority
and does not introduce a second history or durable display-event journal.

## Method and receipt audit

All 98 current `Method` variants are classified exhaustively by `Method::may_commit`;
the native routing match is also exhaustive. There are 66 read variants, 31 variants
that can commit domain/connection resources, and `Goal`, whose `Show` is a read.
The internal Runtime Client has 39 current request variants; none reintroduces a full-snapshot endpoint. Its synchronous/async dispatch retains bounded typed refusal for requests requiring the other path.
An encoding failure after a successful potentially committing operation reports
`OutcomeUnknown`; definite admission refusal is separate. No client automatically
replays a mutation. The audit covers each current variant in these groups:

| Variants | Receipt / read boundary |
| --- | --- |
| Initialize; SessionAttach, SessionDetach, SessionSubscribe | Finite authority/capability or attachment/registration identity. AttachmentStatus reconciles same-connection receipt loss; Subscribe owns a pending physical receipt. |
| SessionCreate, SessionName, SessionFork, SessionBranch, SessionSwitchNode | Finite Session metadata and immutable editor locator; editor bytes and node origin are independent bounded reads. Post-commit failed readback remains OutcomeUnknown. |
| SessionDelete, SessionRecoverDeletion, SessionExportPrepare | Existing finite deletion/durability outcome and download identity; no embedded archive or node graph. |
| SourcesWrite, ConfigurationReconcile, AdoptConfiguration | Exact finite revision/application outcome; source/effective/application documents are independent bounded fields and pages. |
| ModelSet, ModelSelect | Exact settings revision; catalog selection and current settings are independent native reads. |
| SessionUploadPrepare | Finite transfer identity/outcome; admitted manifests/chunks retain existing upload bounds and uncertainty ownership. |
| TurnStart, TurnSteer, InboundEdit, InboundRemove, TurnCancel | Finite inbound identity/sequence/CAS or cancellation outcome, never copied canonical history. |
| InteractionRespond, InteractionCancel; JobCancel; AgentSendMessage, AgentInterrupt; DeliveryCancel, AgentObserveCancel | Existing exact decision/action/cancellation owner with finite outcome or identity. Observation cancellation has no execution cancellation capability. |
| CompactContext; Goal | Finite maintenance/Goal display facts and source descriptors; original Compaction/Goal authority and model-context semantics are unchanged. Goal Show is read-only. |
| AgentArtifactRead, AgentDeliveryRead, AgentDeliveryLocate, ArtifactRead, DeliveryRead, DeliveryLocate | Existing protected artifact/delivery authority and transfer bounds; ContentRead cannot widen those permissions. |
| SessionUploadStatus; ServerInfo, ServerDiagnostics; SessionList, SessionRead, SessionSummary, SessionNode, SessionTree, SessionBoundaries, SessionDeletePreview, SessionAttachmentStatus | Bounded identity/metadata/diagnostics or advancing native catalog pages. |
| SessionHistory, SessionTraceHistory, SessionTraceHistoryDetail, SessionTracePage, SessionTraceValueRead, SessionTraceContentRead, SessionStatistics, SessionNodeValueRead, SessionNodeOriginHistory, SessionHistoryContentRead, SessionHistoryPage, SessionHistoryValueRead, SessionEditorRead | Exact historical native source/cut, finite headers, advancing pages and bounded range/value reads; no node activation. |
| Trace, TraceDetail, TraceLocateTool, Transcript, ConversationTurns; AgentTrace, AgentTraceDetail, AgentTraceLocateTool, AgentTranscript, AgentTurns, AgentStatistics | Bounded exact-cut live/retained projection and directories; typed unavailable/cut/source failures remain distinct. |
| ModelGet, ModelCatalog, Capability; JobStatus, JobList, JobWait; AgentStatus, AgentList, AgentObserve, AgentWait | Finite current heads and source descriptors/pages; Observe alone follows the Child's own native Replay with admission/deadline ownership. |
| SessionCheckpoint, ConversationContentRead, ConversationCheckpointPage, ConversationCheckpointValueRead | One native checkpoint cut or bounded source selection/range. No read-side flush, commit, settlement or model work. |
| ConfigurationGet, ConfigurationValueRead, ConfigurationPage, SourcesRead, SourcesPage, SourcesValueRead, McpProbe, SettingsRead, SettingsValueRead, SessionConfiguration, SessionApplicationValueRead | Bounded settings/effective/source/application fields, exact native revisions and advancing collection/value reads. |

## Bounded source evidence

Native canonical, Journal and inherited provenance records remain authoritative.
Display identities may omit an oversized inline alias while retaining its original
numeric locator or immutable field selector. Original bytes are available through
explicit bounded reads at the advertised cut. Public Turn navigation uses the
returned native ordinal and gesture-captured cut; native execution/control IDs
remain required. A missing source is an explicit observation failure, never an
empty result, fabricated identity or request to mutate execution.

Inherited Turn headers carry a finite row witness over the header kind, original
ordinal, validated native payload digest and exact encoded header bytes. Native
and observed headers have separate witnesses. Reads verify this binding before
accepting owner, preview, cursor or byte-coordinate fields, without loading the
original payload or member list. The optional header savepoint can fail while the
original bootstrap, canonical history and inherited provenance remain committed.

Inherited Assistant membership retains the original paired foreign key over
Ledger position and MessageId. Its finite read witness binds the numeric position,
original identity digest, Turn/Response ordinals and original membership digest.
A different valid canonical position cannot reuse that witness. The witness is
derived read evidence; it neither replaces the paired native relationship nor
confers source or filesystem authorization.

Both read paths guard SQL column types and byte lengths **before** `row.get`
allocates Rust metadata. Turn header reads select at most a 16,384-byte BLOB,
64-byte native digest and 32-byte witness. Membership lookup selects one indexed
row containing three 32-byte digests/witnesses, a 64-byte digest and scalar
ordinals. Corrupt oversized columns become unavailable through SQL `CASE`
expressions, including when a diagnostic fixture bypasses schema checks. The
membership proof measures at most 160 returned metadata bytes and checks that
its query plan uses neither a full scan nor a temporary B-tree.

| Source proof | Contract exercised |
| --- | --- |
| `inherited_turn_row_witness_rejects_changed_owner_preview_cursor_and_encoded_bytes` | Reject changed finite header fields while original provenance and its row evidence remain unchanged. |
| `inherited_turn_evidence_sql_bounds_precede_rust_metadata_allocation` | Reject oversized header/digest/witness columns before Rust allocation, with original payload columns denied. |
| `inherited_turn_header_row_witness_failure_preserves_original_bootstrap` | Optional witness-write failure preserves the original bootstrap and returns explicit observation unavailability. |
| `inherited_turn_header_row_witness_columns_are_required_by_schema` | Require the current witness columns and bounded table shape; no old-layout compatibility path. |
| `t08_t12_t14_t19_giant_native_membership_witness_binds_exact_numeric_c` | Resolve giant original membership through numeric C with original ID/body columns denied; reject a substituted Ledger position. |
| `t10_t14_t19_membership_read_witness_failure_preserves_native_lineage` | Preserve original canonical, Turn and Response authority when the optional membership witness write fails. |
| `t08_t14_t19_membership_sql_bounds_reject_corruption_before_metadata_allocation` | Reject oversized membership metadata at SQL selection, retain exact native lineage and measure the finite read workset. |

Native request-start and provider-outcome probes use the existing
`events_request_id_kind_idx` over original Journal JSON. They never use optional
observation headers as semantic authority. The start probe decodes at most two
selected original envelopes and still rejects duplicate starts. The outcome
probe takes the earliest original Completed and Failed candidate separately,
then selects their earliest native sequence. Original request/envelope ownership
validation, provider terminal refusal and transaction durability remain unchanged.
No schema, index, event authority or model-context semantics were added.

The pinned epoch-105 new-store path creates all current tables, indexes and root
metadata and verifies their shape inside one existing SQLite `Immediate`
transaction. Only a successful verification commits that schema birth. This
removes repeated DDL commits without weakening native `FULL` synchronization or
`DELETE` journal mode; `bind_identity` remains its separate original native
transaction. It introduces no migration or compatibility path for layout 58.
The new deterministic selectors
`durable::sqlite::tests::new_schema_birth_rolls_back_late_ddl_and_commits_once_with_full_durability`
and
`durable::sqlite::tests::new_schema_birth_rolls_back_when_shape_validation_fails`
respectively inject a late real index-name conflict and an authorizer refusal
during final shape validation. They require rollback of every newly created
object/root row, preservation of pre-existing sentinel bytes, successful retry,
one measured schema commit, unchanged durability pragmas and normal identity
binding/reopen. Both exact tests passed at epoch 105 in 7.342s and 0.030s,
respectively (`/tmp/467-focused105-results.json`, logs
`/tmp/467-focused105-0.log` and `/tmp/467-focused105-1.log`).

The auxiliary-write audit found that bounded event fallback JSON and several
derived coordinates could still refuse the original Journal commit on SQL
failure. The pinned closure initializes the existing row availability marker to
unavailable and clears it only after all auxiliary writes succeed. The clear is
itself optional. Independent savepoints preserve already-captured own lifecycle
facts when a different layout fails, and witness reads OR the final committed row
marker rather than retaining an initial unavailable bit. Native Tool ownership,
canonical-call correlation, lifecycle settlement, original J/T allocation and P
remain mandatory. The two new trigger regressions,
`t10_t14_t19_bounded_event_fallback_failure_keeps_original_terminal_and_lifecycle`
and `t10_t14_t19_assistant_tool_enrichment_failure_keeps_native_c_and_tool_owner`,
passed again on the linked epoch-99 binary in 0.200s and 0.082s respectively
(`/tmp/467-final-focused99-results.json`, logs
`/tmp/467-final-focused99-3.log` and `/tmp/467-final-focused99-4.log`). The first
injects fallback-JSON, marker-clear, transcript-coordinate and witness SQL
failures, retaining the exact original Failed/Cancelled J, native T, terminal
absorption and explicit Source unavailability. The second retains original
Assistant C bytes, mandatory canonical-call ownership and commit sequence, and
the actual Tool occurrence despite derived Assistant/Tool-coordinate failures.
Both verify a subsequent unfaulted native event has available Source evidence;
no canonical Tool result or execution settlement is invented. At that historical stage, these focused passes did not establish the then-pending
full-suite or browser results. Current integrated results are recorded above.

Recoverable readiness has a separate existing owner flag.
`observation_readiness_missing` fences a row until `observe_journal` has installed
its native readiness layout; the optional repair clears only that flag. It is
not folded into the permanent `observation_unavailable` marker. Receipt batches
OR both indexed flags and selected witnesses inspect both, so a readiness-only
repair can recover while a simultaneous unrelated auxiliary fault remains
unavailable. The deterministic regressions are
`durable::sqlite::tests::t10_t19_t20_readiness_layout_failure_never_refuses_native_journal_commit`
and
`durable::sqlite::tests::t10_t19_t20_readiness_repair_preserves_independent_auxiliary_failure`.
The first exposed the stale marker in the epoch-102 full run; the repaired
implementation and strengthened cases are compiled in 104. At that historical stage, their presence in discovery did not substitute for
full execution. The current integrated run executes both regressions.

The epoch-95 retained delivery database isolated a separate healthy-path failure:
generic event-field paging interpreted original Tool result properties named
`content` and `collections` as Source metadata. Successful Tool J8 therefore
retained its native outcome but fell back to an unavailable observation before
the later Tool C. Both structural field readers now decode bindings/frontiers
only at `request_observation_fields`' exact `messages.<ordinal>` path, whose
native `message_value` producer appends that metadata. Original Tool, state and
arbitrary request JSON keep their own property semantics.
`t08_t12_t19_successful_tool_j_before_c_keeps_original_content_and_available_source`
passed in 0.152s at epoch 96 (`/tmp/467-new-focused96-0.log`): successful J precedes
result C, original opaque fields and range bytes survive, Source remains available,
and each range reads at most two finite rows/64 KiB. The companion
`t08_t12_t19_request_metadata_requires_the_original_message_producer_path`
passed on the linked epoch-99 binary in 0.066s
(`/tmp/467-final-focused99-1.log`). It distinguishes safe inline JSON from unsafe
opaque integers retained through exact selected bytes, rejects metadata inference
at both generic readers, and preserves positive intentional message bindings and
frontiers. The successful Tool J-before-C proof also passed again in 0.090s
(`/tmp/467-final-focused99-0.log`).

Pending adoption now isolates only derived `pending` segment/frontier cleanup in
the existing optional savepoint. Original Pending deletion, canonical C,
`InboundTurnAdopted` J, transcript identity, pending count and mutation revision
remain owned by the native adoption transaction.
`t09_t12_t19_pending_layout_cleanup_failure_cannot_refuse_native_adoption`
passed in 0.075s on epoch 99 (`/tmp/467-final-focused99-2.log`). Actual SQLite
DELETE triggers separately fail segment and frontier cleanup: native adoption and
healthy Source receipts still commit, while the retained derived rows cannot be
read using an expired cut, the removed ordinal under a fresh cut, or a foreign
Conversation. A later real ordinal remains readable and unfaulted cleanup works.
All five Source cases are recorded in `/tmp/467-final-focused99-results.json`;
these focused passes leave full-suite and browser validation pending.

The actual stdio delivery reproduction passed 1/1 with no skips in 81.118s against
the default epoch-95 binary (`/tmp/467-tui-delivery96.log`). Exact command, from
`tui/`:

```sh
RUSTX_BINARY=/home/capi/Documents/codes/rustX-issue-467/target/debug/rustx RUSTX_REQUIRE_PROVIDER_EMULATOR=1 node --test --test-name-pattern='lists, renders, saves byte-exactly' test/delivery-integration.test.ts > /tmp/467-tui-delivery96.log 2>&1
```

This selected result does not establish full TUI, browser or final-suite success.

All three exact tests in
`durable::sqlite::tests::request_probe_tests` passed on the linked epoch-86 binary
(`/tmp/467-final-focused86-results.json`):

| Exact test | Executed result and evidence |
| --- | --- |
| `t14_t19_t20_native_request_probes_skip_unrelated_giant_original_journal_rows` | Passed in 2.346s. Sixteen genuinely appended unrelated original events each have an identity larger than 1 MiB; indexed VM work remains unchanged, only the two selected small original envelopes are materialized, absent requests load no rows, and the native cut is unchanged. |
| `t14_t19_native_request_probe_rejects_duplicate_original_starts_and_owner_mismatch` | Passed in 0.054s. Deliberately corrupted original selected ownership is rejected; two original starts still fail as duplicate authority. |
| `t14_t19_native_request_outcome_probe_keeps_earliest_original_success_or_failure` | Passed in 0.609s. Both success-before-failure and failure-before-success retain the earliest original Journal outcome. |

Canonical, Audit and Live references remain the three closed public content
sources. A public read checks its published release evidence or captured native
cut before the Store reads exact source identity, representation/version and
frontier in one transaction. Ordinary segmented ranges use SQL slices from at
most two 65,536-byte rows; at most 65,536 decoded bytes are returned. Live block
metadata uses at most two finite metadata rows, with the selected slice copied
into the same bounded reply. Positioned binary reads retain the original managed
artifact lock and length authority. These reads do not stage, flush or settle.
Selected delivery metadata likewise retains the original successful Tool C,
numeric Ledger position, delivery ordinal and native cut; the protected file
action separately applies its original scope, filesystem and Delivery Access
authorization. Reference caches retain finite evidence, not body leases or an
independent source frontier.

Standalone Trace pages retain their own complete native `read_cut`; cached rows
keep the real installation cut through unrelated publications and Replay eviction.
The currently published Trace cut remains pinned by that existing cache owner.
An embedded page may inherit its outer checkpoint/event cut only when it represents
that exact cut; a retained older head carries its original explicit cut. Closed Value/Content field selectors preserve
original J/C identity and outcome evidence; lifecycle replacement cannot discard
immutable request fields.

Selected non-Request detail now dispatches on its exact original Journal anchor
and returns `TraceInspectionHead`, not a partial `TraceDetail`. Tool heads retain
proposal/result fields, canonical occurrence and artifacts; User/Context,
Assistant and Compaction heads retain original canonical locators and block
frontiers. Adoption membership advances beyond the eight-item prefix in original
ordinal order. Existing numeric-L block/tool pages use the same captured cut
through Root, historical Session and authenticated cold Child routes. No reader
recaptures a later Ledger frontier or creates a Runtime cursor.

Tool Schema is selected only through the exact accepted Assistant C/J and its
original frozen RequestStart/tool ordinal. Original provisional identity and
native Attempt/Step establish that relation at the existing commit boundary;
fixed Tool identity digests validate the selected original ordinal at read time.
Neither the latest request, reused wire call ID nor the current Tool registry can
supply a definition. Available, proved absent and unavailable are distinct typed
states. Catalog/relation/body-layout failures are isolated in optional savepoints,
including the existing accepted-request join, so they cannot refuse native J/C
or hide independent Assistant/Tool facts. Original schema bytes use the existing
Request field Page/Value index; payload/result bytes and protected artifacts keep
their existing authorities. All 14 named native/Source/real-Child proofs below
passed at epoch 108; current full-suite and new client execution remain separate
gates.

Refresh batches admit at most eight interests while
the loaded cache remains bounded. Successful canonical Tool results alone own
the delivery head; original collection indices can advance beyond its eight-item
prefix. Selected metadata and protected file reads preserve original grant,
catalog and filesystem authorization independently of execution settlement.

Workflow subdomains select the existing `workflows` source owner; a path such
as `workflows.runs` does not create another source handle. The
`t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut`
regression checks that owner and subdomain resolve the same native source,
Page/Value retain the original captured cut and run, and an actual subsequent
Workflow revision still expires the old mutable-domain cut. This fixes an absent
owner-key mapping; no expiry retry or new-cut stamping conceals it. The exact
regression passed at epoch 86 in 0.116s
(`/tmp/467-final-focused86-3.log`).

The existing native Workflow publication owner releases preceding read-model
revisions after every successfully appended Workflow observability fact, using
that original committed Journal sequence. The former start/terminal-only
restriction hid intermediate child and checker state until the outer Tool
settled. A failed append still releases nothing: no callback, original Journal
or native cut changes. The exact regression
`t10_t14_failed_workflow_append_does_not_release_uncommitted_source_revisions`
passed at epoch 99 in 1.712s (`/tmp/467-final-focused99-6.log`).

Native Workflow revisions remain distinct from Runtime cursors. A complete
source-backed replacement may coalesce native revisions while advancing exactly
one Runtime cursor; it retains replay and initial-cut evidence instead of
clearing the newly published record. The
`t14_t20_workflow_native_revision_gaps_preserve_complete_replay_and_checkpoint`
regression passed at epoch 99 in 0.092s (`/tmp/467-final-focused99-7.log`), checking
native revisions 37 then 41, contiguous replay and whole-checkpoint equivalence
including all event metadata and frontiers. An older mutable Workflow domain cut
still expires after a genuine new revision.

The full external conformance target passed 26/26 at epoch 99 in 105.40s
(`/tmp/467-conformance99.log`), including all nine Workflow cases. At the held
second-writer provider gate, the tampering case waits for the actual native
revision to appear in event metadata, captures one checkpoint, and verifies its
exact Page/Value source plus original Journal facts: two implementation
admissions, one completed first checker and no Workflow terminal. Physical gates,
request counts and deadlines remain unchanged. At epoch 99, full Rust, TUI and
browser checks remained separate pending gates; later execution is recorded below.

Cold Child Trace reads retain the immutable historical Source tuple captured by
TraceProjection. Its revision-zero cut is distinct from a durable Host's
publication/cache cut. The Registry authenticates the original Child CID and
Activation; selected Trace, Journal, context, occupancy, identity and delivery
fields/pages read the original Store at that same historical cut. The wrapper
removes only its authenticated Activation for the native historical read and
restores it on the reply after verifying that CID/Activation still match the
Registry. The pinned epoch-105 durable checkpoint path applies that same
authenticated binding to the checkpoint's retained embedded Trace head before
returning the outer checkpoint. It preserves the Trace head's original
Source/revision/J tuple, rather than borrowing the outer checkpoint's newer
publication cut. The extended exact T18 selector is
`boundary_suites::subagent::conformance::observation::t18_live_durable_only_and_reactivation_preserve_source_and_execution_identity`.
It reads the real embedded terminal result and request detail at the original
A0 cut, verifies unchanged native source state, then checks that real A1
reactivation rejects that retained A0 selection. This extended exact T18 case passed at 105 in 64.606s
(`/tmp/467-focused105-2.log`). Active/Stopping Child reads continue through the
actual live Peer and its native cut. No historical reader activates a node, adopts a newer Source cut
or infers terminal settlement from observation absence. The genuine Child
continuation case, including original A0/A1 reads, mutation-cut expiry, frozen
history and recovered resume, passed at epoch 89 in 5.07s
(`/tmp/467-continuation89.log`).

Recovered Registry construction now installs the existing selected-source layout
cache alongside the Agent's native index and activation records under the same
Registry lock. This closes the missing-cache panic before the recovered Agent's
first publication without adding a fallback, cursor or execution transition.
`t15_t20_recovered_agent_installs_its_selected_source_before_any_publication`
passed at epoch 90 in 0.063s (`/tmp/467-new-focused90-2.log`), proving immediate
selected-source reads, unchanged original Journal/cut, and the same cache owner
before any republish.

Production transcript decoration requires the page's original Source cut and
uses only bounded numeric selectors. The former exported cutless decorator and
its inherited lookup graph are removed from production. Explicitly named
`cfg(test)` native oracles retain full original-history comparisons. The
`cutless_response_decoration_is_rejected_without_changing_native_page` regression
requires both production decorators to reject an absent cut before any mutation.

## Client cut validation and executed evidence

The epoch-104 deferred Trace path retains a positive generated `CheckpointPage`
in the existing `TraceDetailEntry`; it never casts that header into a complete
`TraceDetail`. Root, Child and historical readers use the page's own returned
cut. The wrapper checks exact owner, cache epoch and retained page identity
before and after reads. Root checkpoint supersession evicts deferred pages and
inline entries with content bindings whose readers capture the retired authority;
complete inline bytes without those bindings remain cached. The existing UTF-8
cache weight includes the retained page, and eviction changes no native fact.

Options, prompt, Tool catalog and Diff use fixed selected request fields and
existing Page/Value methods. Selected pages must match domain, cut,
`cursor == read_cut.revision`, contiguous original offsets and continuation for
these request-field pages. Canonical `.tools` pages separately preserve sparse
original block coordinates, while `.blocks` pages are contiguous. The
first original byte reply fixes H; later windows preserve H, representation and
source. A streaming UTF-8 decoder retains split character bytes, and original JSON
windows are never parsed/reencoded or rounded. The original predecessor prompt
is selected only through an exact native predecessor ID/shared-source match to a
loaded request. A missing loaded predecessor offers the existing earlier-history
gesture, rather than asserting native absence or inventing a nearby request.

`web-console/test/trace-deferred-detail.test.tsx` has 15 cases covering these
boundaries, positive deferred evidence versus unavailable errors, foreign cuts,
late Root/Child/historical replies, field/page replacement, both Root inline
retention cases, malformed cursor and exact unsafe-u64/UTF-8 bytes. The focused
six-suite run passed 159 cases before the final cursor case, then the final
15-case file and fresh typecheck passed
(`/tmp/467-source-trace104-focused.log`,
`/tmp/467-source-trace104-cursor-focused.log`,
`/tmp/467-source-trace104-cursor-typecheck.log`). These are scripted-client/UI
proofs; actual Composer and historical Diff browser reruns are still required.

Root checkpoint installation validates the returned snapshot and read-cut CID
against the exact attachment target, requires no Child Activation, and requires
checkpoint Cursor to equal native cut revision. Present Source evidence must
retain that CID, the cut's Journal frontier, and its duplicated Ledger, Surface
and transcript-count coordinates. An absent optional Source cut remains absent;
validation does not invent one. A repair older than locally Applied is rejected
before replacing either presentation state or its checkpoint.

Root event handling performs the same checks before folding scope-bearing
`checkpoint_advanced` and `projection_transition` carriers, including nested
carriers. Matching outer Target and Registration cannot authorize a foreign inner
cut. Child handling requires its actual CID/Activation and each record's own
Cursor/revision; the batch's final Cursor is not substituted for intermediate
records. Checkpoint source evidence is checked before replacing the Child
baseline. Leaf facts acquire no invented Cursor or identity. Both clients retain
prior state and Applied when a cut is refused. Root recovery remains bounded;
the Child watch reports observation unavailability and explicit Transcript repair
can establish a new checkpoint before following resumes. No guard fabricates an
execution outcome or retries a mutation.

Root release also retires the TUI observation chain immediately. Both initial
attachment and repair check the exact acquisition epoch, release/closed state
and chain after the Subscribe await, before accepting a receipt or folding its
buffered events. Deterministic tests deliver a valid late receipt after Detach
or node switch, for both acquisition paths, and retain the previous state,
checkpoint and Applied baseline. An immediate release from the prepared-attach
callback starts no subscription or later configuration acquisition.

The deterministic client tests exercise correlated responses and notifications
through the actual client handlers over scripted transports. They cover foreign
snapshot/read-cut/Source CID, unexpected Child Activation, stale revisions,
checkpoint/event Cursor mismatch, inconsistent Source coordinates, nested invalid
carriers, valid repair and contiguous following. A positive two-record Child batch
checks its distinct intermediate cuts. The rendered AgentCard test refuses a
same-CID/Activation stale inner revision before rendering its message, performs
explicit native-checkpoint repair, then follows the next Cursor without execution
control calls. These are client contract proofs, not new native Socket evidence.

Authored fixtures were corrected to supply their actual Session CID and complete
transition metadata: reconnect no longer advances Cursor while retaining revision
zero, repair snapshots no longer borrow another fixture Conversation, and the
Child transcript continuation now constructs its checkpoint and record at Cursor
one together. Existing identity, continuity, no-replay, cancellation, DurableOnly,
file-byte and provider-count assertions remain unchanged.

| Executed client command | Result and evidence |
| --- | --- |
| `node --test test/subagent-transcript.test.ts test/bounded-observation.test.ts test/root-checkpoint.test.ts` from `tui` | 52 passed, no failures or skips; `/tmp/467-tui-pure-observation96.log`. This includes the formerly blocked Child continuation fixture. |
| `pnpm exec vitest run test/root-checkpoint.test.ts test/child-observation.test.ts` from `web-console` | 22 passed across two files; `/tmp/467-web-pure-observation96.log`. |
| Earlier focused actual-client/AgentCard checks | Web 48 passed and TUI 40 passed; `/tmp/467-web-child-cut-focused90-2.log` and `/tmp/467-tui-child-cut-focused90-2.log`. All four lightweight client typechecks passed for that pin. |
| `node --test test/app-server-client.test.ts test/reconnect-observation.test.ts test/session-focus.test.ts test/root-checkpoint.test.ts` from `tui` | 100 passed, no failures or skips at epoch 97; `/tmp/467-tui-late-receipt97-focused.log`. Includes four late-receipt release cases and immediate prepared release. TUI typecheck also passed (`/tmp/467-tui-late-receipt97-typecheck.log`). |
| Full TUI epoch 93 | 1,100 passed, 7 failed, 1 existing skip; `/tmp/467-final-tui93-client-full.log`. Six real delivery/Child/interrupt failures still required Source diagnostics; one pure Child continuation was stopped after its invalid authored cut prevented the expected next request. The corrected pure case passed in the 52-test run above. |

The epoch-102 status below supersedes the earlier client failure counts; final
client/native safety nets and complete browser validation remain outstanding. The
fresh-bin Stdio delivery repro at epoch 96 is separate evidence: its isolated
log reports one pass and no failure (`/tmp/467-tui-delivery96.log`). That result
does not establish the remote delivery or full-TUI safety net. Failure-only
diagnostics preserve original native record/cut evidence and the owned fixture
Store for finite Source inspection; they do not issue repair reads or change the
original wait/assertions.

Source inspection identified the real native failure in retained Journal 8:
`observation_request::page_from` and `field_header_at` interpreted arbitrary
original properties named `content`/`collections` as display bindings/frontiers.
The successful original Tool result contained Text content, so this optional
indexing attempt failed while original execution continued through canonical C3.
Metadata decoding now requires the actual `request_observation_fields`
`messages/<native ordinal>` producer path; ordinary Tool/state/schema properties
remain original values. The focused
`t08_t12_t19_successful_tool_j_before_c_keeps_original_content_and_available_source`
regression passed at epoch 96 (0.152s). The related metadata-origin fixture and successful-Tool regression both passed
again among the eleven focused epoch-102 proofs. Complete native/client reruns
remain separate gates.

### Accepted output during queue failure

Every enqueue/encoding failure in the common transport serve loop now retires
the carrier before dropping unaccepted work, then joins its existing writer.
An already accepted Stdio record retains its exact remainder and LF. A physical
write failure or the unchanged ten-second writer deadline takes precedence over
the enqueue error; the enqueue error is returned if the accepted record finishes.
This adds no queue, execution cancellation or new physical timeout.

The real OS-pipe regression
`t17_t20_stdio_queue_overflow_finishes_the_physically_accepted_record` passed on
epoch 100 in 1.19s (`/tmp/467-pipe-overflow100.log`). The original pipe accepted
one byte; its incomplete prefix could not parse as a response. A one-shot gate
held the remainder while 33 further bounded replies filled the 32-record queue
and caused overflow. The connection retired while the writer remained owned;
releasing the gate produced exactly the original ParseError JSON and one LF,
then physical EOF, with no queued successor record or provider request.

The same test previously failed its unchanged 30-second fixture liveness guard
at epoch 99 (`/tmp/467-final-focused99-8.log`). That run had no stage diagnostics,
so the blocked await remains unlocalized. The only intervening test change added
finite stage prints; it did not alter a gate, assertion, timeout or production
path. Printing can change scheduling and does not establish why the earlier run
failed. The successful epoch-100 trace proves this executed interleaving; it is
not a claim that the earlier liveness failure has an identified cause. Keep the
stage evidence and require the complete safety net separately.

## Deterministic owner barriers

The existing M9b cancellation-versus-start boundary has two `cfg(test)` pauses:
`pre_start` before arbitration, and `pre_commit` inside the original start gate
immediately before the durable commit. `PhasePause::park` enters
`tokio::task::block_in_place` **before** announcing its ordinal and waiting on its
existing synchronous release channel. This hands off the worker so a control task
in its unstealable LIFO slot can progress; both original arbitration points and
their lock ownership stay intact. It introduces no production execution owner,
clock change or altered commit boundary.

Other race proofs retain the original native owner gates, physical handoff
receipts, exact request/registration tokens and explicit worker/listener exit.
Execution waiters capture their existing native handoff waiter or receipt ordinal
before admission and then require the same Attempt in an original fact or
authoritative replacement checkpoint. Predicates inspect complete records at their actual native cursor;
fixtures do not create synthetic fact-cloned event records.

The exclusive `LocalConversationRuntime::shutdown(&mut self)` owner boundary
first awaits the existing native runtime drain, then cancels presentation reads,
closes the observation queue and joins its worker and finite in-flight Source
read. The same worker slot retains its JoinHandle while awaiting completion;
cancelling a shutdown waiter releases its mutex guard and a retry still joins
that original worker. Storage remains owned until the composition and its handles
are dropped.
The headless owner has no presentation worker and awaits only native shutdown.
`LocalConversationShutdownError` keeps native settlement failure separate from
the finite cancelled/panicked projection-join classification; a failed downstream
join does not relabel an already-quiescent execution as `Draining`.

Child terminal cleanup closes the existing live inspection server before awaiting
this composition-owner boundary. A cleanup failure produces a bounded diagnostic
and returns the original semantic result unchanged, including an already-accepted
Result frame. Connection close, attachment detach and observation cancellation do
not invoke this owner shutdown.

The updated external durable requirements 05, 06 and 14 exercise detach/reattach,
headless-to-interactive reopen and resource-refresh reopen through that explicit
owner boundary. Requirement 12 subscribes to the original native activation's
Eligible publication before capturing its unchanged-cursor paging baseline. The
pending-mutation isolation fixture uses a genuinely absent foreign occurrence,
then proves the other Store's real selected occurrence succeeds without changing
the first Store's bytes. These descriptions are ownership/proof contracts. The
complete durable target passed again at 86-2 (147/147,
`/tmp/467-external86-2.log`); later fixture pins still require a fresh safety net. The
existing `blocked_read_materialization_does_not_own_shutdown_or_projection_drain`
test independently exercises the underlying cancelled-read/worker-join seam.
The new `projection_drain_retry_retains_the_original_worker_after_waiter_cancellation`
proof gates that actual Source task after cancellation, drops the first pending
join waiter, verifies the retry remains pending, then releases and joins the same
worker. It passed again in the focused epoch-86 run (27.743s); the existing blocked
read/physical-drain proof also passed (1.112s). The explicit release gate, rather
than elapsed time, establishes both orderings. These results are recorded in
`/tmp/467-final-focused86-results.json` and do not establish a full library pass.

## Deterministic RFC matrix

Tests remain in their owning suites under `tests/README.md`. Fully qualified Rust
selectors below come from captured native discovery; `publication::` selects the
external `--test durable` target, and the CLI EOF case selects `--test process`.
Other qualified Rust selectors select `--lib`. File paths identify JavaScript
consumer suites, not Rust discovery. Names below identify
the contract proofs; a real boundary is exercised where the invariant concerns
Stdio, WebSocket, Unix inspection, SQLite or a Child process. Wall-clock deadlines
are liveness guards, never race evidence.

| RFC | Implemented tests / consumers | Invariants | Synchronization or real boundary |
| --- | --- | --- | --- |
| T01 | `app_server::transport::tests::t01_complete_envelope_exact_byte_limit_and_one_byte_overflow`, `app_server::protocol::lifecycle_protocol_tests::t01_request_id_budget_counts_encoded_utf8_and_escaping`, `local_runtime::session_runtime_manager::tests::protocol::t01_invalid_identity_precedes_method_and_connection_admission`, bounded response substitution and typed cancellation tests | 02, 03, 14 | Actual capped serializer at 1,048,576 bytes and one-byte overflow; domain/admission counters remain unchanged on rejected identities. |
| T02 | `local_runtime::session_runtime_manager::tests::protocol::child_delivery::bounded_observation::t02_t03_large_child_observation_keeps_parent_websocket_and_other_session_following` | 01, 02, 13, 14, 16, 17 | Actual Child process + Unix inspection + authenticated TCP WebSocket; provider gate holds original Child execution while oversized observation is read. |
| T03 | The exact T02/T03 selector above; `local_runtime::session_runtime_manager::tests::transports::t16_pending_receipt_supersession_and_stale_queue_progression` | 01, 04, 07, 14 | Independent Session starts and reaches native terminal events on the same carrier; pending Child pages advance 2 → 3 at the same cut. |
| T04 | `boundary_suites::runtime_client::observation::t04_native_unix_initialize_and_checkpoint_never_carry_an_eight_mib_snapshot` (`tests/boundary/runtime_client/observation.rs`) | 02, 04, 14, 16 | Actual Unix peer initialization carries only bounded identity/capabilities; explicit checkpoint reads replace the old full snapshot. |
| T05 | `protocol/app-server/observation.ts` ObservationChain, `tui/test/reconnect-observation.test.ts`, `local_runtime::session_runtime_manager::tests::transports::t17_partial_event_disconnect_replays_from_locally_applied_cursor` (`tests/scripted/app_server/transports.rs`), Child observation/hide/reopen tests | 05, 06, 08, 14, 17 | Exact native cursor identity, contiguous fold and locally Applied reconnect cursor; deferred content is unnecessary for advancement. |
| T06 | `runtime_client::projection::tests::t06_t14_oversized_transition_is_one_foldable_checkpoint_and_converges`, `runtime_client::host::tests::t06_t14_many_live_blocks_keep_every_descriptor_after_inline_budget_exhaustion`, real WebSocket oversized-notification recovery in `tests/scripted/app_server/oversized_notification.rs` | 02, 05, 06, 16 | Native publication constructs one complete bounded transition. A deterministic transport-budget fault fences the affected registration; the independent checkpoint includes the original native canonical commitment and a fresh subscription resumes contiguously while held execution and a sibling Session continue. |
| T07 | `local_runtime::session_runtime_manager::tests::transports::t07_oversized_controlled_response_keeps_publication_owner_at_websocket_handoff`, existing Delivery Access acceptance/cancel/revoke tests | 01, 07, 15 | Actual final tungstenite handoff; original publication owner arbitrates substituted errors and exactly one accepted response. |
| T08 | `publication::t08_large_canonical_content_exact_byte_chunks_survive_restart` (`--test durable`), `boundary_suites::durable::content::t08_t20_content_range_reads_touch_at_most_two_finite_sqlite_rows`, `boundary_suites::durable::content::t08_t20_request_source_ranges_use_bounded_authenticated_composed_pages`, `boundary_suites::durable::content::t08_t12_t19_binary_canonical_artifact_ranges_are_frozen_owned_and_restartable`, `durable::sqlite::tests::t08_t14_t19_agent_statistics_use_fixed_headers_with_giant_original_bodies`, `runtime_client::host::tests::pure_read_tests::t08_t14_t19_pages_and_value_reads_leave_native_progress_queued`; `durable::sqlite::tests::t08_t14_t15_t19_trace_headers_and_scalar_ranges_never_read_growing_original_bodies` | 02, 10, 11, 12, 14, 16 | Ordinary segmented Text/raw/CanonicalJson ranges return ≤65,536 decoded bytes from ≤2 finite rows. The Request scalar range selector checks actual composed-page work: ≤160 page handoffs of ≤64 KiB, one ≤16 KiB root, no native-node traversal or header decoding for its scalar-only fixture, exact bytes and advancing chunks. General composed reads retain the separate ≤64-piece bound; the retired two-segment representation is not asserted. Binary uses frozen retained ManagedArtifactStore authority with positioned reads under its shared lock. Statistics read one ≤16 KiB original-J aggregate and no full event/request/message bodies. Exact bytes survive reopen. New finite-head selectors are listed in the [epoch-107 proof directory](#epoch-107-finite-inspection-proof-directory): original tiny/giant Tool payload, result and frozen Schema, canonical-kind dispatch and bounded Store-only source reads; all 14 additions passed in the exact epoch-108 run. |
| T09 | `boundary_suites::durable::content::t09_t12_pending_edits_never_alias_immutable_canonical_content`, `publication::t09_t12_t19_committed_live_prefix_fixed_identity_and_frontier`, `web-console/test/trace-deferred-detail.test.tsx`: exact selected-window extent/representation and late-owner rejection | 09, 10, 12, 15 | Native revision/stream replacement with old reads in flight; old references remain fixed or explicitly expire. |
| T10 | `app_server::t10_stdio_cli_eof_preserves_gated_attempt_and_original_canonical_commit` (`--test process`), `local_runtime::session_runtime_manager::tests::transports::adapter_disconnect_preserves_active_execution_and_replacement_authority`, `local_runtime::subagent_child::tests::t10_t18_t19_closed_observation_source_preserves_native_child_completion`, real Web/TUI parity | 01, 13, 17 | Provider/Tool gates and actual carrier/listener close; original incarnation, activation, request counts and settlement remain authoritative. |
| T11 | `boundary_suites::subagent::conformance::observation::t11_t12_t20_nested_pending_reads_follow_authenticated_direct_routes_over_two_unix_peers`, `web-console/test/bounded-observation-native.test.ts` (real disconnect and byte-exact WebUI/TUI recovery), `tui/test/integration.test.ts` (same operations/projection over Stdio and WebSocket), `local_runtime::session_runtime_manager::tests::transports::t11_t15_effective_configuration_ranges_survive_model_commit_on_real_websocket` | 05, 08, 10, 13, 17 | Actual Root/Middle/Leaf Unix routing and client carriers; many bounded configuration chunks preserve unrelated held execution and carrier progress. The real Child live/cold finite-head selector and native large-J identity proof in the [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) cover shared native routes. All 14 directory proofs passed at 108. The four real TUI Root Delivery/Child Stdio/WebSocket cases passed at 111 with zero skips. The final 113 Child cases additionally select the actual canonical Tool result collection and reconstruct its original questionnaire bytes; browser114 Chat separately validates the advertised image block and original binary bytes, preview and reconnect. |
| T12 | Actual Unix source-substitution tests, `boundary_suites::durable::content::t12_t14_t19_source_cut_provenance_rejects_old_journal_with_new_canonical_frontier`, `boundary_suites::durable::content::t08_t12_canonical_locator_namespaces_preserve_literal_ids_and_full_inline_id_budget`, binary retention and protected Delivery tests | 09, 10, 12, 15 | Server validates native ownership, representation, cut, revision and frontier; modified references grant no new authority. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds exact original retry/catalog ownership, foreign Child Activation rejection, wrong real RequestStart/ordinal and missing/corrupt-layout negatives; all 14 directory proofs passed at 108. |
| T13 | `local_runtime::session_runtime_manager::tests::protocol::t13_attach_identity_checkpoint_and_subscribe_preserve_the_native_cut`, `local_runtime::session_runtime_manager::tests::protocol::t13_lost_huge_root_attach_receipt_is_reconciled_without_second_admission`, giant catalog selection and immutable editor/origin tests | 03, 04, 14, 17 | Bounded identity/revision receipts, same-connection AttachmentStatus after an unaccepted receipt and independent checkpoint over a 2.4 MiB retained message; no uncertain mutation replay. |
| T14 | `web-console/test/incremental-equivalence.test.ts`, `runtime_client::host::tests::incremental_projection_independent_snapshot_capture`, `runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture`, `runtime_client::projection::tests::t14_t20_retained_cut_lookup_uses_exact_metadata_and_bounded_cursor_index`, `runtime_client::projection::tests::t14_native_admission_publishes_frozen_model_before_execution_starts`, `runtime_client::projection::tests::t14_t19_worker_read_fence_keeps_one_native_cut_before_publication`, `boundary_suites::durable::content::t08_t14_t19_response_decoration_keeps_one_frozen_native_journal_cut`, `runtime_client::projection::tests::t14_t19_complete_canonical_head_survives_an_earlier_delayed_callback` | 05, 06, 08, 10, 11, 12 | Strict structural fold equivalence at real native cuts; held C1/C2, empty block frontier, C-before-J and Audit/terminal facts remain distinct. |
| T15 | `boundary_suites::durable::content::t15_t20_message_and_transcript_headers_advance_without_source_body_reads`, `boundary_suites::durable::content::t08_t15_t20_missing_middle_descriptor_never_skips_native_history`, `runtime_client::host::tests::owner_tests::t08_t12_t14_t19_cold_giant_turn_page_navigates_by_native_ordinal`, Source/block/journal/settings/model/editor/page suites, `runtime_client::projection::tests::t15_many_small_message_pages_advance_under_both_native_budgets`, `runtime_client::projection::tests::t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut`, collection page clients | 02, 06, 10, 14, 16 | Indexed native ordinals and exact cut/revision continuation; item count and aggregate encoded bytes both bound pages; giant items advance through finite descriptors. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds 19-member User/Context inspection paging beyond eight and Source selected-member validation, with original order and advancing continuation; all 14 directory proofs passed at 108. |
| T16 | `local_runtime::session_runtime_manager::tests::transports::t16_pending_receipt_supersession_and_stale_queue_progression`, `local_runtime::session_runtime_manager::tests::transports::t16_physical_acceptance_wins_before_replacement_no_lock_cycle`, `local_runtime::session_runtime_manager::tests::transports::t16_pending_physical_pipe_replacement_advances_without_readiness`, `runtime_client::host::tests::resubscription_is_superseded_before_local_handle_publication` (Host); transport proofs in `tests/scripted/app_server/transports.rs` | 01, 07, 14 | Real full OS pipe and tungstenite Frame handoff; hooks gate before/inside acceptance with valid winners, Pending revalidation, RecoveryRequired and Discarded progression. |
| T17 | `local_runtime::session_runtime_manager::tests::transports::t17_partial_event_disconnect_replays_from_locally_applied_cursor`, `local_runtime::session_runtime_manager::tests::transports::t17_partial_os_pipe_record_finishes_after_registration_replacement`, `local_runtime::session_runtime_manager::tests::transports::t17_stdio_input_eof_finishes_the_physically_accepted_output_record`, `local_runtime::session_runtime_manager::tests::transports::t17_stdio_teardown_retires_unaccepted_record_without_write_readiness`, `local_runtime::session_runtime_manager::tests::transports::t17_t20_stdio_queue_overflow_finishes_the_physically_accepted_record`, late TUI Subscribe receipt release tests, `local_runtime::session_runtime_manager::tests::transports::t16_t17_websocket_frame_handoff_precedes_physical_write_failure` (same transport suite) | 07, 08, 17 | Actual partial OS-pipe writes and EOF; incomplete parse never advances Applied, first positive write finishes its record, later Flush is not acceptance. EOF/shutdown discards a zero-acceptance queued record even when physical write readiness remains blocked. |
| T18 | `boundary_suites::subagent::conformance::observation::t18_live_durable_only_and_reactivation_preserve_source_and_execution_identity`, actual Unix old-activation initialization, closed inspection listener tests | 01, 10, 13, 17 | Live → DurableOnly → new Activation; retained content remains readable and observation absence never fabricates execution completion. |
| T19 | `scripted_suites::agent::publication::no_frame_is_released_before_its_staging_commit`, `runtime_client::host::tests::t19_staged_frame_is_projected_before_observer_returns_to_release`, `runtime_client::projection::tests::t14_t19_prepared_started_witness_preserves_exact_live_progress_and_terminal_facts`, `publication::t19_staging_receipts_pin_each_committed_frame_without_later_source_reads`, `publication::t19_raw_tool_json_retains_original_bytes_after_canonical_commit`, full durable Publication suite, binary/UTF-8 split proofs; separate original-source expiry and native event-fold proofs (combined expired-ref/Replay proof at 105 below) | 09, 10, 11, 12, 15 | Frame staging commit and release-authorized evidence before observation; parked Audit sink and failed staging cannot expose new bytes or be repaired by reads. The [epoch-107 directory](#epoch-107-finite-inspection-proof-directory) adds C-without-J frozen inspection, original giant schema/payload ranges, independent optional-layout failures and native J identity distinct from safe byte coordinates; all 14 directory proofs passed at 108. |
| T20 | `local_runtime::session_runtime_manager::tests::protocol::child_delivery::control_intent::t14_t20_two_execution_intents_bypass_six_waits_and_eight_unpolled_observations`, `local_runtime::session_runtime_manager::tests::protocol::child_delivery::control_intent::reconnect::t14_t20_reaped_websockets_preserve_global_control_reserves_over_native_waits`, `app_server::admission::tests::t20_admission_cancellation_deduplication_and_exact_cleanup`, `local_runtime::session_runtime_manager::tests::protocol::t20_observation_cancel_after_admission_before_first_poll_never_dispatches`, `runtime_client::agent_observation::tests::t20_real_unix_initialize_and_checkpoint_share_one_absolute_deadline`, `runtime_client::agent_observation::tests::t20_cancelled_physical_sql_worker_retains_inspection_capacity_until_exit`, `boundary_suites::durable::content::t08_t19_t20_canonical_json_orders_unbounded_keys_by_original_utf8`, `durable::sqlite::tests::t14_t19_t20_transcript_source_prefix_recovers_after_4097_later_owner_releases`, `app_server::transport::tests::t20_complete_record_candidate_capacity_obeys_the_physical_byte_budget`, checkpoint encoder capacity and fragmented Stdio input reservation proofs, `boundary_suites::subagent::conformance::observation::t20_real_unix_listener_bounds_and_reaps_its_physical_workset`, `runtime_client::projection::tests::t20_replay_byte_budget_covers_every_event_and_evicts_in_cursor_order`, predecode client guards | 01, 03, 04, 07, 08, 14, 17 | Actual reaped TCP WebSocket connections leave twelve physical waits resident; two fresh waits fill the ordinary ceiling14, while two original-owner control intent receipts are physically received before terminal release. One shared Host counter preserves14/16 reserves. Manual clock conserves one 15-second deadline; blocked physical SQL retains permits; actual listeners reap JoinSet tasks; finite encoded/decoded worksets and exact token fencing. |

## Integrated boundary coverage

The N/B/C labels refer to the integrated boundaries in
[invariants](invariants.md#bounded-conversation-observation-issue-467-rfc-v3),
not additional owners or alternate protocols. Every INV-01–17 appears in the RFC
matrix above. The table identifies implementation and proof ownership; discovery
is not execution evidence.

| Boundary | Existing implementation owner | RFC / invariant cross-reference and proof location |
| --- | --- | --- |
| N1 / B1 | `app_server/admission.rs`, Connection associations | T01/T13/T20; INV-03/04/14/17. Exact admission token, duplicate/cancel/deadline and delayed cleanup tests; identity receipts in `tests/scripted/app_server/protocol.rs`. |
| N2 / B2 | `app_server/registration.rs`, common transport writer, native Host subscription | T07/T16/T17; INV-01/07/08/14/15. `tests/scripted/app_server/transports.rs` gates actual pipe acceptance and tungstenite Frame handoff; replacement and native subscription installation share the slot. |
| N3 / C1 | Runtime Client Projection, checkpoint capture, Store read domains | T06/T14/T15; INV-02/05/06/10/11/12/16. Independent Host/canonical captures, exact retained-cut lookup, worker-fence and complete canonical replacement tests. |
| N4 / B3 / C2 | Publication staging/release and original Store ContentRef selectors | T08/T09/T12/T19; INV-09/10/11/12/15/16. `tests/durable/publication.rs` and `tests/boundary/durable/content.rs` retain distinct staged/released, Live/Audit/C and immutable/range evidence. |
| N5 / C3 / C4 | Registry, `runtime_client/agent_observation.rs`, authenticated Unix Peer, listener/physical task owners | T02/T03/T10/T11/T18/T20; INV-01/04/13/14/17. `tests/boundary/subagent/conformance/observation.rs` exercises direct nested routes, original Child CID/Activation and Live/DurableOnly; physical SQL and canceled drain proofs retain permits/JoinHandles until exit. No Parent full Child mirror is introduced. |

All 60 unique qualified library selectors in the T01–T20 matrix are present in
the epoch-111 library discovery (3,935 tests; `/tmp/467-discovery111-0.txt`).
All five named external selectors were also confirmed using `--list` on the
current compiled durable and process targets, including the CLI EOF proof in
`tests/process/app_server.rs`. Discovery establishes execution eligibility;
the scoped run records below separately establish which tests passed. These
are distinct test binaries.
The matrix also includes external process and JavaScript tests, which are not
members of library discovery. T11's real client parity is distinct from T14's
source-captured strict fold oracle. T16 covers both valid winners, zero-byte
Pending replacement, RecoveryRequired and stale queue progression; T17 separately
covers accepted remainders, EOF, overflow and post-handoff failure. These named
cases do not assert exhaustive exploration of all scheduler interleavings.

## Recorded checkpoint 102: incomplete historical evidence

Results below are scoped to their actual source/binary epoch. The epoch-102
`cargo test --all-targets --all-features` exited 101 after 3,907 passed, one failed
and five ignored library tests; the readiness-marker failure described above
stopped it before a complete external-target pass. Its command record is
`/tmp/467-final-validation102-native-full-results.json`. A new complete epoch-104
run has not been executed at this documentation pin. Root serializes native and
browser execution with `CARGO_INCREMENTAL=0`,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` and `RUST_TEST_THREADS=4`.

| Execution | Recorded result | Remaining scope |
| --- | --- | --- |
| Quality 102 | Diff, formatting, full Clippy, default bins and all-target/all-feature no-run compile all passed. `/tmp/467-final-validation102-quality-results.json`. | Compilation does not establish execution of all discovered tests. |
| Focused native 102 | All 11 exact proofs passed. `/tmp/467-focused102-results.json`. Includes general JSON-pointer semantics, exact integer/opaque JSON, bounded layout traversal and previews, original metadata producer paths, and both physical two-row SQLite range proofs. | No full-suite inference. |
| Independent captures 102 | Both native capture tests passed (1.440s / 0.309s) and regenerated the actual full-runtime and canonical fixtures. `/tmp/467-captures102-results.json`. | Strict all-field Node comparison is a separate consumer test; captures are not Socket traces. |
| Protocol 102 | Generation, staging the four generated artifacts and `protocol/app-server check` passed. `/tmp/467-protocol102-results.json`. | Final later-source drift must still be checked if source changes. |
| Static clients 101/101b | All four typechecks, dev 41/41, i18n, provenance (276 source records / 264 notices) and provider emulator Python 54/54 passed. The initial provenance mismatch was corrected and rerun. | Later client edits need their own current checks. |
| Full clients 102 | TUI 1,122 passed / one existing platform skip. Web 2,731 passed / one failed / four existing skips, 2,736 discovered. `/tmp/467-final-validation102-client-full-results.json`. | Web navigation/control-readiness repair is pinned with focused tests/typecheck; its full rerun is pending. TUI readiness follow-up is owned separately and does not turn this earlier pass into final-source evidence. |
| Full browser 100 | 274 passed / 43 failed / 317 discovered; artifacts preserved under `/tmp/467-browser100-results`. | Complete rerun after repairs remains required. |
| Affected browser 101 | 24 passed / two failed / 26 discovered. `/tmp/467-browser101-focused-results.json`, `/tmp/467-browser101-focused.log`. All 19 Settings cases, four model-error variants and the incremental 200-update / exact scroll assertions passed. | Both failures exhausted the unchanged 240-second setup budget while preparing 300 native attempts. This is not a passing reading gate. |

The epoch-102 traversal regression compares more than 10,000 original fields
against the independent legacy oracle and proves the repeated point-SELECT work
is removed. The schema lookup retains general JSON-pointer fallback; native
integer and opaque JSON proofs preserve exact wire semantics. These focused
results validate the narrow performance changes' bounds/equivalence, not their
ability to finish the normal browser workload. No optimized compiler profile,
reduced workload or increased timeout substitutes for the failed reading gate.

The two fresh capture tests are
`runtime_client::host::tests::incremental_projection_independent_snapshot_capture`
and
`runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture`.
Their consumer is `web-console/test/incremental-equivalence.test.ts`, which compares
all actual serialized fields, cuts, frontiers, C/J facts and native cursors against
independent Checkpoint(D); it only omits JavaScript `undefined` properties that do
not exist in JSON. Vocabulary checks remain separate. The current full-client
failure does not justify weakening those or any native assertions.

Concrete missing final evidence remains: a complete current all-target Rust
run; current full Web/TUI after the final client pins; the unchanged normal
300-attempt reading gate on the latest performance build; and a complete pinned
browser rerun after the epoch-100 repairs. Final command logs and any CI results
must be added from actual execution. Earlier scoped passes and implemented proof
names do not fill these gaps.

The integrated main remains `97096020d64ea00218493a0a99b9b805821eeb28`; the original
development base was `3c4b85a627e87383f244600e8b19887f103b82f0`. Independent
Compaction PR #477 is not an implementation dependency. No final commit, push or
PR success is claimed in this ledger.

### Browser 103: Chat prepend assertion scope (execution pending)

The affected browser run exposed a fixture assertion spanning two intentional
viewport gestures. Its Chat trace records the first bounded click growing anchor
rows from 12 to 60. Before the second click, the real conversation scrollport is
at 3,699px; Playwright scrolls the newly distant Load earlier button into view,
returning it to zero. That deliberate movement exactly matches the failed final
anchor delta. The second page still reaches 102 anchors, Answer 0 and no further
Load earlier button. Evidence is the `call@112` / `call@118` snapshots and action
logs in `web-console/test-results/chat-native-history-rich-s-bfa12--and-native-upload-receipts/trace.zip`.

The fixture now checks the unchanged <2px original-anchor invariant immediately
after the first real bounded prepend, before any subsequent click can move the
viewport. Its existing default assertion deadline allows ChatViewport's scheduled
animation-frame restoration after the DOM row count grows; no tolerance or
configured timeout changes. Remaining pages still require genuine clicks, and
all 34 turns, Answer 0, exhaustion and native/provider assertions remain. This is
a fixture pin, not a passing browser result; the completed epoch-103 affected
run sampled the previous fixture.


## Historical checkpoint 105: focused/client passes, final validation open

At epoch 104, diff/format, full all-target/all-feature Clippy, default bins and
all-target/all-feature **no-run** compilation passed
(`/tmp/467-final-validation104-quality-results.json`). Native library discovery
contains 3,918 cases. Neither that compile nor the focused/capture passes execute
the complete test set. Full `cargo test --all-targets --all-features` at this epoch
is still required; Root will record its result separately.

Full Web attempts 104 and 104b exited nonzero
(`/tmp/467-final-validation104-client-results.json`,
`/tmp/467-final-validation104b-client-results.json`): the Office converter deadline
and a native timeout remain failed run evidence, not skips or passes. The
serialized full epoch-104 rerun then passed **2,748 tests with four existing
skips / 2,752 total** in 377.826s, using
`pnpm --dir web-console test --maxWorkers=1`
(`/tmp/467-final-web104-serial-results.json`,
`/tmp/467-final-web104-serial.log`). It includes the final selected-page cursor
case; neither earlier failed attempt is relabeled. The new epoch-105 expired-reference consumer case was added afterward; this
104 pass does not cover that case. Its genuine regenerated capture and later
105b execution are recorded below.

The full epoch-100 browser run recorded 274 passed / 43 failed / 317 total.
Affected epoch 103 recorded 93 passed / five failed / 98 total
(`/tmp/467-browser103-results.json`, `/tmp/467-browser103-1.log`): Chat prepend
assertion scope, Composer Options availability, historical Diff availability and
both normal 300-attempt reading setups. The pinned UI/fixture/performance repairs
require actual epoch-104 browser execution. No screenshot, reduced workload,
longer timeout, optimized profile or focused mocked pass replaces that gate.

### Executed epoch-105 native and client checks

| Execution | Actual result and evidence | Scope |
| --- | --- | --- |
| Quality 105 | Diff, formatting, full all-target/all-feature Clippy, default bins and all-target/all-feature no-run compile passed; 3,920 library tests discovered. `/tmp/467-final-validation105-quality-results.json`. | This does not execute the complete library/external targets or CI lane selection. |
| Three exact native proofs 105 | Both schema-birth rollback/commit-count tests passed in 7.342s / 0.030s; the extended T18 embedded-Trace Activation/selected-read/reactivation test passed in 64.606s. `/tmp/467-focused105-results.json`. | Real native SQLite/Unix/Registry boundaries described above; no full-suite inference. |
| Genuine native captures 105 | `runtime_client::host::tests::incremental_projection_independent_snapshot_capture` passed 51.208s and `runtime_client::projection::tests::t14_native_canonical_boundary_independent_checkpoint_capture` passed 0.550s. `/tmp/467-captures105-results.json`. | Fixtures were regenerated by actual native test output, including the original expired-LiveRef witness and fresh native Replay through D. |
| Full default Web 105b | 2,749 passed / four existing skips / 2,753 total, 35.770s. `/tmp/467-final-web105b-client.log`. | Includes the strict all-field capture consumers and new genuine expired-LiveRef/Replay pure-fold case. |
| Full default TUI 105b | 1,127 passed / one existing platform skip / 1,128 total, 36.407s. `/tmp/467-final-tui105b-client.log`. | Original byte, cancellation, registration and client fold assertions retained. |
| Web build 105b | Passed 5.700s; `/tmp/467-final-web-build105b-client.log`. | These three client/build commands are recorded in `/tmp/467-final-validation105b-client-results.json`; browser execution is separate. |

The earlier full default Web105 attempt failed only the real native parity
fixture's unchanged ten-second provider-gate wait. Its captured native state was
still **Admitted**, request count zero; the provider had not reached the execution
gate (`/tmp/467-final-web105-client.log`). The fixture now observes actual native
**Running** before starting that provider-gate wait. The original 120-second
case, ten-second provider wait and all semantic assertions remain unchanged; no
production code or timeout was relaxed. The later full105b pass does not erase
that earlier failed run.

### Default-disk browser105: failed evidence, separate from tmpfs106

The focused default-disk run remains incomplete in
`/tmp/467-browser105-1.log`. Chat failed its five-second native preparation
checkpoint/transcript poll before any `page.goto`; its old failure-only Inspector
lookup on a blank page then masked that original stage until the case deadline.
The diagnostic-only catch is corrected separately. Composer failed the
five-second `Plan recorded.` assertion while its original Attempt still showed
three running Tools. These observations do not establish a UI104 Trace defect
or prove execution had already completed.

The first normal 300-attempt reading preparation exhausted its unchanged
240-second budget. Read-only native sampling captured an actual worker in
`fsync → ext4_sync_file → rq_qos_throttle`, with host I/O pressure around
90% (`/tmp/467-reading105-first-fsync-stack.log`,
`/tmp/467-reading105-first-sample.json`). That identifies a concrete blocked
filesystem operation, not a completed reading/performance gate or proof that
every remaining delay has the same cause. These failed default-disk results are
not superseded by a successful disk performance claim.

The later complete epoch-106 diagnostic run used
`TMPDIR=/dev/shm/r4.tXR0dP` and finished **311 passed / six failed / 317 total**,
as recorded below. Both unchanged 300-attempt reading preparations and browser
cases passed there. Scenarios, counts, protocol/security assertions, 120-second
cases and 240-second reading preparation were unchanged. Native processes still
used real SQLite, managed files and registered paths on the host; only their
temporary filesystem changed. This establishes the recorded tmpfs execution,
not physical-media durability, default-disk latency or the default-disk reading
performance gate. Current full Rust and CI-lane execution remain separate gates.

### Proof closure and remaining validation gates

- **T19 / C2 integrated expired-LiveRef case:** the actual native Source-expiry
  proof is `publication::t09_t12_t19_committed_live_prefix_fixed_identity_and_frontier`;
  strict captured Checkpoint/Event folds are separate. The `expired-stream` case
  in `web-console/test/incremental-vocabulary.test.ts` is synthetic vocabulary
  coverage, not proof that a genuine expired LiveRef remains in native Replay
  while Applied reaches the following terminal/commit without a ContentRead.
  The existing
  `runtime_client::host::tests::incremental_projection_independent_snapshot_capture`
  is now extended to assert typed Host/SQLite expiry of that untouched nonempty
  reference, then subscribe from C only after expiry and export native Replay
  through D. The added exact consumer test is
  `T19 genuine expired LiveRef remains in native Replay and Applied reaches D without materialization`
  in `web-console/test/incremental-equivalence.test.ts`. It requires the genuine
  cfg-only capture witness and strict endpoint equality. The actual Rust capture
  passed at 105 (51.208s) and regenerated that witness; the pure consumer passed
  in the complete Web105b run. Thus this specific proof gap is closed by actual
  execution, without hand-editing fixture JSON, adding a content reader or
  hiding any native cut/frontier/settlement difference.
- **Historical epoch-108 execution:** the native capture, protocol and full
  Rust/CI-lane driver was still running at this checkpoint. Its failure and
  the complete repaired-source epoch-111 pass are recorded below.
  Epoch-108 diff/format/Clippy/no-run and all 14 exact finite-inspection proofs
  passed. Earlier full Web/TUI105b results remain scoped to that source epoch.
  The latest complete browser is the qualified tmpfs106 **311/6** result; its
  six repaired cases and the new native/client layer require fresh browser
  execution. Root owns the serialized final execution ledger.
- **CI/platform selection:** `tests/README.md` and the current CI lane checker
  distinguish in-crate `boundary_suites::` from external durable/process/subagent/
  tools/conformance/cfg3 targets. A successful safety-net run does not prove CI
  selection coverage. macOS discovery/platform tests and remote CI status cannot
  be inferred from this Linux workspace. Required Provider Emulator/uv and real
  binaries must fail closed; ignored measurements and existing platform skips
  must remain explicitly reported.

The earlier architecture/matrix review did not establish final product closure:
the subsequent real browser exposed the Trace kind-dispatch defect recorded
below. Its repair and the isolated optional accepted-request join passed the
14 exact epoch-108 proofs; older or focused suites do not establish full closure. This document makes no
complete/green, final commit/push or PR-success claim.

## Checkpoint 106 browser evidence and 107–108 repair scope

The complete unchanged 317-case browser command, `pnpm --dir web-console
 test:e2e`, finished with **311 passed / six failed** in 1,654.188 seconds
(`/tmp/467-final-validation106-browser-tmpfs-results.json`). Its temporary native
runtime, SQLite and workspaces used `TMPDIR=/dev/shm/r4.tXR0dP`; the pinned
Playwright image, provider workloads, screenshot tolerances and test deadlines
were unchanged. This is actual native/IPC/HTTP/browser evidence on tmpfs, **not
physical-media durability or default-disk latency evidence**. Both original
300-attempt reading preparations and their browser cases passed. The earlier
failed default-disk run remains a failed run.

The six failures identified these concrete boundaries. Repairs are pinned;
fresh complete browser execution is still required:

| Failure | Owner and correction |
| --- | --- |
| Chat Tool inspector | Shared native Trace detail dispatched every advertised kind through the Request-only reader. Non-Request kinds now select an explicit finite typed inspection head. Tool schema selection derives from the exact original frozen request and canonical Tool occurrence; raw payload/result readers and original artifact authority remain separate. |
| Child Inspect disclosure | Fixture opened the Tool before the original completed process-membership transition, which legitimately folds the process. Wait for that exact native membership, then perform the original reveal and Inspect gestures. Attempt terminal and membership are distinct facts. |
| Compaction questionnaire | The component retired a pending-interaction read on an unrelated checkpoint revision, then left its successor permanently loading. Ownership uses the actual pending domain revision, finite header/source and view identity; the original server cut is retained. Held-response tests cover unrelated checkpoint advancement and real Page/Value owner replacement. |
| DOCX preview | The assertion began while the actual Office conversion was still running. The fixture waits for its exact operation's complete successful response body before the unchanged DOM assertion; production conversion deadlines and cancellation/security tests are preserved. |
| Settings observation | The fixture gated an initial convergence read needed to enable its own field. Establish initial form readiness before arming the original post-write response gates; all write/read interleavings and no-replay assertions remain. |
| Historical Trajectory Diff | A conditional locator count skipped the explicit bounded read while detail admission was still pending. Wait for the captured original field selector, then perform the same reads and assertions. |

All 14 native finite-head, Store relationship and real-Child proofs passed at
epoch 108 (`/tmp/467-focused108-results.json`). Original-request catalog, Tool relation and adoption-member
layouts remain optional derived observation evidence. Their savepoint failure
cannot refuse native canonical or Journal commits. Public metadata reads use
fixed columns before growing IDs and bounded indexed seeks; no original giant
Message/Request deserialization is added. Missing or corrupt evidence is typed
unavailable rather than a fabricated absent schema or a different request.

The first epoch-107 quality run passed diff/format but failed Clippy on one new
range tuple call, one unchecked platform-width conversion and two function-size
lint violations. Those fixes and the optional missing-request-body-layout
isolation are saved. **107c Clippy passed in 70.345s** and unchanged production
bins built in **78.638s**. Epoch 108 then passed diff/format, all-target/all-feature
Clippy (**72.306s**) and no-run compilation (**153.061s**), with **3,934** library
tests discovered. `/tmp/467-final-validation108-quality-results.json` and
`/tmp/467-discovery108.txt` record those boundaries; compilation alone is not
execution.

The earlier epoch-107 diagnostic failed two test expectations: an invalid foreign
ConversationId fixture and a comparison of canonical ResultMetadata against a full
ToolExecutionResult. Both are retained as failed diagnostic history. The repaired
result proof compares the exact metadata contract, follows the advertised
same-cut canonical blocks, reads every giant text byte through bounded original
payload references, and reconstructs the complete original result before and
after terminal J. It preserves the old heads and independent C-without-J outcome.
All 14 exact selectors passed at 108, including this proof and the real Child
live/cold route; `/tmp/467-focused108-results.json` contains their individual logs.

Real TUI 107 Root Delivery (two) and Child transcript (two) tests passed over
Stdio/WebSocket, **four passed / zero skips**
(`/tmp/467-tui-real107-results.json`). Protocol/dev typechecks, dev **41/41**,
provider **54/54**, notification **15/15** and Office preflight passed at 107.
The initial focused Web run including the 20 new Messages tests passed **187**
tests. At that historical checkpoint, its type correction and strict Request
guard still required final checks. New inspection-head client parity was not
inferred from the four existing real TUI cases. Native captures108, protocol
and full Rust/CI-lane execution were still running; later results below supersede
that pending status.


### Epoch-107 finite inspection proof directory

These **14 tests all passed at epoch 108** in the exact serial runner
(`/tmp/467-focused108-results.json`), and all selectors are present in the
3,934-test library discovery (`/tmp/467-discovery108.txt`). They extend the
existing T01–T20 matrix, not a new acceptance numbering scheme. The directory
records each tested boundary; it does not establish a complete Rust or browser
pass. New Web/TUI finite-head consumers still require their final generated and
client checks; the native Child route proof alone is not T11 client parity.

| Exact selector | Matrix rows | Actual boundary and evidence asserted by the saved test |
| --- | --- | --- |
| `durable::sqlite::tests::t08_t12_t15_t19_trace_definition_uses_exact_original_retry_catalog_without_body_reads` | T08/T12/T15/T19 | Actual retry 0/1 requests reuse wire Tool identities. The later request exists before the first accepted C; each original C still selects its own catalog and first matching Tool ordinal. Authorizer denies original J/C/R and growing alias columns while exact giant schema ranges advance. |
| `durable::sqlite::tests::t10_t12_t19_trace_definition_auxiliary_failure_preserves_native_c_j_and_own_lifecycle` | T08/T12/T19 | Actual SQLite auxiliary-write fault leaves original C/J and independent own facts committed; Schema failure cannot become another request or execution failure. |
| `durable::sqlite::tests::t10_t12_t19_trace_definition_missing_and_ambiguous_catalogs_never_become_absent` | T12/T19 | Missing/ambiguous optional relationships report unavailable; absence requires the original complete catalog. |
| `durable::sqlite::tests::t08_t12_t15_t19_trace_adoption_advances_and_validates_only_selected_native_members` | T08/T12/T15/T19 | Original adoption ordinal/L membership advances beyond its prefix. Selected-member witnesses, cut bounds and authorizer/work assertions reject wrong original membership without loading all bodies. |
| `durable::sqlite::tests::t10_t12_t19_trace_adoption_index_failure_never_refuses_native_adoption` | T08/T12/T15/T19 | An actual optional membership-index failure cannot refuse or replace native adoption, canonical rows or Journal facts. Missing derived evidence stays explicit. |
| `durable::sqlite::tests::t12_t19_trace_definition_rejects_another_real_request_and_later_matching_ordinal` | T12/T19 | Wrong existing RequestStart, wrong existing ordinal, later duplicate Tool ID and cleared positive ordinal cannot select a different schema or fabricate absence. |
| `durable::sqlite::tests::t10_t12_t19_missing_request_body_layout_never_hides_own_assistant_tool_facts` | T08/T12/T19 | A trigger rejects the entire optional Request field layout. Original Assistant/Tool J, canonical proposal and exact argument bytes remain serviceable; the optional accepted-request/Schema join fails separately. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_tool_schema_unavailable_preserves_own_lifecycle_and_original_fields` | T08/T12/T19 | Clearing optional Schema readiness leaves the complete own Tool record, original fields, canonical history, Journal bytes and native cut unchanged. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_tool_inspection_has_frozen_schema_and_bounded_original_ranges` | T08/T12/T19 | Real durable Host checkpoint and historical readers return the same typed Tool head; tiny/giant original payload and frozen definition chunks match independent original values, retain ≤2-row/64 KiB range bounds, reject foreign CID and do not mutate the checkpoint. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_tool_inspection_keeps_c_without_j_and_old_cut_after_terminal` | T08/T19 | Capture before result C, commit the actual Tool C without terminal J, then append the real terminal. Result outcome/artifact/original bytes appear only at the C-inclusive cut; C alone does not invent terminal J, and both old heads remain equal. Exact metadata plus all canonical block payload chunks reconstruct the entire original giant result, including another read at that frozen cut after terminal J; every range retains the two-row/64 KiB bound. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_trace_assistant_compaction_and_lifecycle_heads_use_their_own_kind` | T08/T19 | Request retains its indexed reader; Assistant, unsettled Compaction and lifecycle anchors use their actual kinds. Original canonical block pages match independent same-cut Store headers; unsettled Compaction invents no summary C. |
| `runtime_client::trace::tests::inspection_tests::t08_t14_t19_all_advertised_canonical_kinds_have_advancing_inspection_heads` | T08/T15/T19 | Real acceptance/adoption of 19 Human or Runtime messages yields User/Context heads, eight initial members and advancing pages preserving all original identities/order; each canonical page matches its independent Store source. |
| `runtime_client::trace::tests::inspection_tests::t11_t19_trace_and_journal_sources_keep_native_sequence_above_safe_byte_coordinates` | T11/T19 | The native Store assigns original J above 2^53 after sequence-boundary setup. Trace/Journal string identities still select the exact original terminal bytes; zero identities and unsafe byte offsets remain rejected. |
| `boundary_suites::subagent::conformance::tool_inspection::t08_t12_t18_t19_tool_inspection_uses_real_child_live_and_cold_original_sources` | T08/T11/T12/T19 | Real Child runtime, Unix inspection and Registry settlement hold the model at an actual provider gate, then move Live → DurableOnly. Both routes read the original Tool head/frozen definition at exact CID/Activation/cut, reject foreign Activation and preserve the two original model requests and native source. The gate releases on fixture drop. |

These proofs cover existing Source, Projection/read, Registry and carrier owners.
They add no parent Child-history mirror, execution identity alias, cursor owner,
content source family or schema-registry fallback. The 14 exact proof passes and
quality results above are recorded execution, while remaining client/full Rust,
CI-lane and browser results stay owned by the execution ledger rather than being
inferred from this directory.

## Historical validation checkpoints 108–111

The full epoch-108 Rust command (`RUST_TEST_THREADS=4 cargo test --all-targets
--all-features`, with the required provider emulator) executed 3,934 library tests:
3,929 passed, five ignored measurement/generator tests, and no library failures. The command
then stopped in the conformance target: 25 passed and
`agent_loop::image_tool_round_trip_and_text_model_switch` failed while its fixture
looked up the immutable original AttemptStarted through an independently mutable
full transcript cut. Actual pending adoption may expire that full cut. The fixture
now queries the existing bounded original Journal index through one captured J
frontier, preserving exact Conversation/Attempt identity, original terminal
uniqueness and the independent complete original-J terminal-last oracle. It does
not retry, restamp or weaken production read-cut validation. A separate real-disk
Store regression captures that interleaving explicitly:
`durable::sqlite::tests::t08_t12_t19_pending_adoption_expires_full_cut_but_preserves_indexed_original_start`.
The complete repaired-source Rust command subsequently passed at epoch 111,
as recorded below.

After that command stopped, direct execution of its immutable compiled external
binaries passed the contracts, durable (147 tests), process, provider, subagent
(47 tests) and tools (135 tests) targets. Those diagnostics do not substitute for
a complete current-source run. The lane checker was not executed by the stopped
full-command runner. Logs and exact commands are in
`/tmp/467-final-validation108-native-full-results.json` and
`/tmp/467-external108-diagnostic-results.json`.

Epoch-110 full Web tests executed 2,793 tests: 2,788 passed, four existing skips,
and one failure at the five-second deadline of the real PTY delayed-ABA retirement
test. The epoch-109 obsolete Tool-detail fixture had been updated to the actual
finite-head/read-cut contract and passed in this run. The PTY failure is being
localized at its existing native ownership/retirement awaits, without increasing
its deadline.

Epoch-110 full TUI tests executed 1,144 tests: 1,141 passed, one existing skip,
and the two real Child cases (Stdio and WebSocket) failed. Their new selected
canonical-block body assertion attempted a generic ValueRead and received the
explicit expired-cut error. The source contract and both clients' advertised
canonical-content readers are under review; the assertion must reconstruct the
original bytes using the authoritative advertised source. The provider cleanup
reported its unreleased gated scenario after this primary failure; that cleanup
error is not the primary cause. Web build was not reached by the stopped runners.

The following epoch-111 results supersede these failed runs. The historical
failures remain recorded; a local pass does not imply remote CI success.

### Executed epoch-111 results

- `git diff --check`, `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo build --bins`, and `cargo test --all-targets --all-features --no-run`: passed.
- `RUST_TEST_THREADS=4 cargo test --all-targets --all-features`, with
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`: passed in 940.778 seconds. Library:
  3,930 passed / five ignored measurement/generator tests; external cfg3_catalog 26,
  cfg3_managed_output 5, conformance 26, contracts 31, durable 147, process 62,
  provider 175 (five existing live-provider ignores), subagent 47, and tools 135
  all passed. Example tests: lane checker 17 and list workload checker three passed.
- The five library ignores are four opt-in measurements (`issue419_measure_native_cold_load`,
  `stage_profile_real_create_pipeline`, the new `measure_native_request_index_complete_300_turn_source`,
  and the new `t19_native_publication_encoding_profile`) plus the existing
  document fixture generator. No correctness test was disabled. The five
  provider ignores require external live-provider credentials.
- `cargo run --all-features --example check_test_lanes`: passed; every runnable
  Linux test is assigned to a CI lane. This is local discovery evidence, not a
  claim that remote CI or macOS execution has completed.
- Both exact new mutation-boundary proofs passed: the two-handle SQLite
  pending-adoption test (5.547 seconds) and the original failing
  `agent_loop::image_tool_round_trip_and_text_model_switch` (25.216 seconds).
  The latter also passed inside the full command above.
- `pnpm --dir protocol/app-server check`, its `typecheck`, Dev `typecheck` and
  `test`, Web `typecheck`, `check:i18n`, and `check:provenance`: passed. Source
  provenance verifies 276 records and notices for 264 production packages.

The canonical-content failure was a concrete client contract error. Native
canonical PageItems provide finite inline metadata and explicit ContentBindings;
they do not advertise a whole-item ValueRead. Web and TUI now use only those
original ContentRefs for bytes, including explicit reads of small raw JSON whose
parsed header is already complete. Sparse Tool pages retain original block
coordinates, independently of Tool count. An advertised continuation returning
an empty page is rejected, while a manually requested out-of-range empty page
remains valid. TUI Root and Child selection owners retain only the current page
and bounded byte window; late pages cannot replace a newer selection.

The repaired Web focused suites passed 64 tests. TUI focused suites passed 148
tests after Root/Child exact-source, sparse-coordinate, late-selection and
advertised-continuation coverage; TUI typecheck passed. The real carrier rerun,
full clients and fresh browser validation are recorded separately below.

Epoch-111 real carrier and full-client gates passed:

| Command / actual boundary | Result |
| --- | --- |
| `node --test --test-name-pattern='lists, renders, saves byte-exactly\|shows metadata without the delivery credential' test/delivery-integration.test.ts` (TUI directory; required emulator and current binary) | Two real Stdio/WebSocket cases passed, zero skips; 3.436 seconds. |
| `node --test --test-name-pattern='native child transcript' test/integration.test.ts` (same environment) | Both real Child Stdio/WebSocket cases passed, zero skips; 6.794 seconds. Original finite Tool metadata and advertised raw argument bytes, exact Child ownership, provider request counts and reconnection assertions remain active. |
| `pnpm --dir tui typecheck` and `pnpm --dir web-console typecheck` | Passed on the final continuation-validation source. |
| `pnpm --dir web-console test` | 2,795 passed / zero failed / four existing skips; 2,799 discovered. The real delayed-ABA PTY case passed in this complete run. The earlier timeout remains recorded; no production lifecycle/deadline change was inferred from the isolated pass. |
| `pnpm --dir tui test` | 1,151 passed / zero failed / one existing skip; 1,152 discovered, including the repaired real Child cases. |
| `pnpm --dir web-console build` | Passed, including native Host modules and bundled-artifact provenance. |

Current-source browser validation starts with the six previously failing files
on the default temporary filesystem, followed by the complete pinned-browser
suite. Browser results are not implied by the client passes above.

### Browser epoch-111: nine repaired scenarios pass; Tool result controls incomplete

The six previously failing files executed ten current-source browser cases on
the default temporary filesystem with the pinned Chromium image: nine passed
and Chat failed (277.405 seconds including fixture startup). Child capabilities,
all four Compaction locale/theme cases, real Office document preview, both
Settings observation cases and historical Trajectory passed. The complete
command and output are recorded in `/tmp/467-browser-111-focused-results.json`
and `/tmp/467-browser-111-focused.log`; failure artifacts were retained separately.

Chat's new result-body assertion exposed an actual client omission. ResultMetadata
correctly excludes canonical result content, and its `artifacts` may be empty
while an image block owns `artifact_1`. Native already advertises the positive
`record.collections` canonical result-block source and its exact ContentRefs.
Web Tool Result and TUI Child Tool inspection omitted those collection controls.
The fix reuses bounded canonical collection controls under the original cut;
it does not fabricate a result Message or request a whole-body ValueRead.
The test must separately verify original metadata and the advertised image-block
bytes while retaining its artifact preview and provider-count assertions.

A related ownership review found incidental callback identity changes could
reset DeferredContent's byte window during an unrelated parent rerender even
when source/cut/reader ownership was unchanged. The repair must distinguish that
case from actual reader, cut, source or view retirement, with held-response and
continuation evidence. These client changes require fresh client/browser runs;
the epoch-111 Rust pass remains evidence for unchanged Native sources.


## Historical pre-review client validation epochs 113–114

The Tool result omission recorded at browser111 is repaired in both clients.
Web `TraceToolFields` selects the original `record.collections` source under the
retained cut and mounts one metadata field or one canonical collection at a time.
A sole advertised collection remains reachable when metadata is absent or a
previous selected field disappears. The current canonical page is at most 32
items; explicit content reads replace one at-most-65,536-byte window. TUI Child
`b`/`p` uses the same advertised Tool result collections as Root; no synthetic
result Message or whole-item ValueRead was introduced.

Reader callbacks now remain stable across an unrelated same-owner rerender, while
held replies check the latest ownership predicate, reader identity, cut and
source before decoding or applying bytes. Deterministic tests cover held Content
and Value responses, reader replacement, owner retirement, and valid unchanged
owner continuation. The final five focused Web suites passed 62 tests; a first
new assertion matched both a field label and a byte window and was corrected to
select the exact `pre` window before the complete rerun.

| Current-source command | Result |
| --- | --- |
| `pnpm --dir protocol/app-server typecheck` (113) | Passed. |
| `pnpm --dir tui typecheck` (113) | Passed. |
| `pnpm --dir tui test` (113) | 1,153 passed, zero failed, one existing skip; 1,154 discovered, 36.341 seconds. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUSTX_BINARY=target/debug/rustx node --test --test-name-pattern='native child transcript' test/integration.test.ts` (TUI directory, absolute binary path, 113) | Two actual Stdio/WebSocket Child cases passed, zero skips, 6.959 seconds. Exact original Tool metadata, questionnaire result bytes, held provider execution, request counts and reconnect ownership remain asserted. |
| `git diff --check` (114) | Passed. |
| `pnpm --dir web-console typecheck` (114) | Passed, 12.631 seconds. |
| `pnpm --dir web-console test` (114) | 2,809 passed, zero failed, four existing skips; 2,813 discovered, 34.331 seconds including preparation. |
| `pnpm --dir web-console build` (114) | Passed, 5.692 seconds. |
| `pnpm --dir web-console check:i18n` (114) | Passed. |
| `pnpm --dir web-console check:provenance` (114) | Passed; 276 source records and 264 production package notices. |

At this pre-review checkpoint, Native sources were unchanged from the complete
epoch-111 Rust and lane passes. Its Chat and complete browser results are recorded
below; both passed before the original commit and PR publication.

A fresh main fetch at epoch114 still resolves to
`97096020d64ea00218493a0a99b9b805821eeb28`. Open PR #477 advanced to
`843dd7dd17ef86cb3a14253dc8bf4dfaf86d3926`; its model-context projection head
publication integrity, complete timeline record admission and fixed fingerprint
bounds were inspected. That PR remains unmerged and is not an implementation
dependency. This change preserves the separate existing Compaction owner.


The isolated current-source Chat browser run passed on the default temporary
filesystem: `bash scripts/browser-tests.sh test/e2e/chat.spec.ts` from
`web-console`, one case passed in 47.309 seconds including fixture preparation.
Its exact original ResultMetadata, canonical image header and Base64 bytes,
artifact lightbox, provider request counts, reconnect and upload assertions remain.
The complete `pnpm --dir web-console test:e2e` command then passed all 317
scenarios, with zero failures or skips, in 1,403.459 seconds (23.3 minutes):
`TMPDIR=/dev/shm/r4.x0j58s0d`, the unchanged pinned browser, workloads and deadlines.
This environment qualifies real carrier/SQLite/source behavior, not physical-media
fsync latency; native epoch111 exercised real-disk Store/restart/range boundaries.


Final browser evidence is `/tmp/467-browser-114-full-results.json` and
`/tmp/467-browser-114-full.log`; the repository's final Playwright artifacts remain
available in the worktree. The earlier failed browser evidence was preserved,
including default-disk writeback throttling and the actual Tool collection UI gap.
Both 300-Attempt preparations completed under their original 240-second budgets.
No screenshot baseline, race assertion, workload or correctness timeout was relaxed.

That pre-review pre-publication fetch again confirmed main
`97096020d64ea00218493a0a99b9b805821eeb28` and unmerged PR #477 head
`843dd7dd17ef86cb3a14253dc8bf4dfaf86d3926`. There is no known unresolved local
contract failure in the mapped T01–T20 evidence. Remote CI and macOS execution
must be reported from their actual run states, independently of these local passes.

Pre-review staged-tree `git diff --cached --check` and protocol regeneration/drift checks
passed. The index contains v48 generated contracts and all intended source/tests,
with v47 artifacts removed and no logs, databases or browser recordings staged.


### Controlled full-chain measurements (review190)

The default debug App Server binary completed the same 1000-Attempt measurement
scenario plus exact Source landing, Fork and detach in 600.121 seconds, versus
864.990 seconds for the preserved pre-full-chain review172 binary. Physical
cleanup completed at 602.350 seconds; the original process and temporary storage
were absent. Native CPU fell from 972.04 to 669.45 seconds. Physical writes stayed
approximately 7.85 GB: the change does not claim to eliminate durable writes.
There were no concurrent task-owned builds/tests. Both runs recorded high and
variable host I/O pressure, so these are qualified measurements, not a guaranteed
speedup on other machines. The formal browser fixture remains 300 Attempts/240s.

| Attempt bucket | Preserved baseline median ms | Shared full-chain median ms |
| --- | ---: | ---: |
| 1–50 | 538.731 | 516.556 |
| 251–300 | 715.535 | 554.062 |
| 501–550 | 836.302 | 574.533 |
| 751–800 | 974.549 | 582.782 |
| 951–1000 | 1093.178 | 602.734 |

The separate 300-Attempt scripted native profile passed with 600 preparation
leaf visits, zero old items revisited, zero estimator materialized items and
144,709 incremental hash bytes. Its measured Attempt interval was 222.121s,
compared with 201.407s in review174; overall scripted elapsed time did **not**
improve in this run. Inclusive prepare/start/commit/evidence phases were
4.852/90.565/84.602/0.071 seconds; the former post-commit reconstruction phase
was 19.304 seconds. These phases overlap and must not be added.

Source consumed 73.017 inclusive seconds: current-request scalar metadata capture
52.318s, capture18.975s (including new-leaf15.762s and authentication1.798s),
paths0.859s and length0.786s. Scalar capture omits messages and its per25-Request
cost stayed approximately4.33–4.44s. It is measured per-request work, not evidence
of old-message traversal or justification for a new cross-request cache. Source
query counts include authentication counts and are themselves included in native
index counts. No old canonical bytes, publication checks or storage exclusion
were removed to obtain these results. The complete native/client/browser validation above is separate from these
qualified performance measurements.
