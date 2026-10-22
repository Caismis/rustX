# Auxiliary retries and automatic titles: review evidence

This change implements [#475](https://github.com/Caismis/rustX/issues/475).
The development branch starts at `df06c3071398aed6d795e8837beb002773dae668`
from `origin/main`. Its prerequisites are already integrated:

| Contract | Integrated commit |
| --- | --- |
| #474 / PR #490: supervised auxiliary executor | `df06c3071398aed6d795e8837beb002773dae668` |
| #473 / PR #486: auxiliary durable facts | `0f3d978e43e2174d5b26feff6bccefc1bf25f71d` |
| #481 / PR #484: bounded request indexes | `6771928953a9b2815a27fd784f6d39c5ccde8b56` |
| #480 / PR #483: Store observation isolation | `3c355f152d405a201482d53b6e45497df4f44a7f` |
| #467 / PR #478: bounded transport and observation | `0d5c4f2eea484999fca9566eeb1cd2059dd6e0a1` |
| #466 / PR #477: immutable-history Compaction | `8cb41624eb2dc2ca87e4409961f42553ceb029cf` |
| PR #489: auxiliary module layering | `832b9e542adcb039a6fb5c5439f4f1e10e56f668` |

The PR records the final source SHA and its CI runs. This file records stable
ownership and test selectors; the normative contracts remain in
[auxiliary-execution.md](auxiliary-execution.md),
[configuration.md](configuration.md), [context-engine.md](context-engine.md)
and [durable-sessions.md](durable-sessions.md).

## Owners and commit boundaries

| Owner | Boundary |
| --- | --- |
| `runtime/auxiliary` | Reserves ordinary/protected capacity, confirms Call/start writes, owns one physical stream at a time, confirms failed-request + retry-scheduled transactions, joins execution/writer cleanup before releasing capacity. |
| `AuxiliaryAuthority` / `CompactionBudget` | Reserves original parent start capacity before Store start; confirms exact receipts once, refunds proven noncommit, retains uncertainty. Deadline and allowance survive semantic Call changes. |
| `durable/sqlite/auxiliary` | One SQLite transaction arbitrates Title source association, first start consumption, per-Call start bound and zero-request terminal release. Native source bindings grant no capability to arbitrary callers. |
| Native Session model coordinator | Freezes current Title invocation, timeout and resource generation at admission. Retries never resolve current settings again. |
| `local_runtime/session_title.rs` | One root-local preactivation watcher; finite metadata and capacity signals; exact indexed first Human read; business ownership survives model settlement through Catalog publication. |
| `SessionCatalog` | Conditional/idempotent generated metadata publication and invalidation after visibility, including committed errors. No Catalog or Store lock spans inference. Manual rename remains a separate operation. |
| Native Clone/Fork preparation | Copies saved manual/generated labels independently of the message cut, marks generated provenance inherited and transfers no execution or source quota. |

There is no compatibility reader, migration, alternate Summary execution path,
Title job queue, independent Title quota counter or generic operation scheduler.
The old Summary-specific model-policy type is replaced by the shared auxiliary
policy. The current App Server artifacts replace the previous version.

| Format domain | Final version and reason |
| --- | --- |
| SQLite | 75: retry settlement, source/start transactions and indexed first-Human metadata. |
| Session Catalog | 17: required generated-title metadata and local/inherited provenance. |
| Runtime Client / App Server / WebSocket handshake | 72 / 52 / `rustx.app-server.v52`: shared Title configuration, generated labels and independently optional Usage metrics. |
| Event envelope | 1, unchanged framing; contained fact changes are gated by SQLite. |
| Configuration authoring | 10, unchanged framing; omitted optional Title policy follows Session. |
| Inspection archive | v8: manifest contains Session generated metadata. Journal6, auxiliary audit3, inherited auxiliary audit3, inherited responses2 and inherited turns2 include retry/partial-Usage changes. Other stream schemas stay unchanged. |

## Limits and delivery semantics

* A Call has three physical starts, with two/four-second local backoff and a
  maximum acceptable single wait of 60 seconds. Provider Retry-After is never
  shortened. Settlement consumes elapsed delay. The original absolute deadline
  is five minutes; consumers can only narrow authority.
* Compaction has twelve confirmed or unresolved starts and three global input
  shrinks across Calls, retries and carryover. Automatic Compaction reentries
  in one logical Agent step retain the original allowance and deadline while
  each checkpoint retains its own exact operation receipt.
* Capacity remains four/three total/general per Conversation and 32/24 per
  process. Title uses general capacity; backoff retains the original permits.
* Title consumes at most 4,096 bytes of the exact first ordinary Human text.
  Empty, oversized or otherwise unusable first text does not select a later
  Human. Title preserves the resolved invocation's complete output budget and
  reasoning parameters, with the existing generation safety policy bounding all
  generated bytes. Domain acceptance separately requires a nonempty single line
  of at most 120 characters and 480 UTF-8 bytes.
* First confirmed Title start permanently consumes automatic eligibility.
  Only that live Call can make its remaining technical attempts. A crash or
  domain rejection can permanently leave the preview; no output replay or
  regenerate feature is introduced. Confirmed zero-start settlement may release
  its exact association for a later native wake.
* Journal settlement precedes the separate Catalog transaction. Losing output
  between those commits is bounded best-effort delivery. Visible Catalog errors
  retain the title and publish summary invalidation. Observed usage is a subtotal,
  not complete remote billing.

## #473 decision mapping

| Decision | Production and verification evidence |
| --- | --- |
| D01 | Shared auxiliary executor; native Summary and Title; `runtime::auxiliary` tests and composed Summary conformance. |
| D02 | Existing distinct operation/Call/request identities; fresh retry RequestIds; `retries_exhaust_at_three_fresh_physical_requests`. |
| D03 | Existing owner-scoped physical event family; Store, primary reconstruction and conformance regression lanes. |
| D04 | Confirmed starts before dispatch; existing executor dispatch gates and process-death fixtures, plus Title source transactions. |
| D05 | `transient_retry_confirms_failure_before_fresh_start_and_counts_settlement_latency`; no next dispatch before exact confirmation. |
| D06 | `context::compaction_budget` and bounded repair/candidate fixtures retain original deadline and allowance. |
| D07 | Summary/Title buffer output; partial attempt discarded. BTW provisional delivery belongs to #476. |
| D08 | Domain validation and Catalog/checkpoint publication remain consumer-owned; Title acceptance and Catalog fault tests. |
| D09 | Existing `runtime::auxiliary::stream_tests` retain EOF and stream-integrity gates. |
| D10 | Existing waiter loss, writer supervision and drain tests; native title owner retained through its Catalog tail. |
| D11 | Existing capacity tests and `bounded_fanout_uses_ordinary_children_and_scope_close_settles_existing_calls`; no parent permit wait or implicit queue. |
| D12 | Summary and Title ship end to end here; Classification/BTW remain #476. Synthetic fixtures add no product APIs. |

## #473 verification mapping

| Row | Tests / lane |
| --- | --- |
| V01–V03 | Existing `durable::sqlite::auxiliary`, mixed-owner Store/reconstruction tests, contracts/provider and boundary lanes; immutable Summary receipts unchanged. |
| V04 | Existing native registration/drain and scope-close gates; bounded fan-out fixture; Title first-start/zero-terminal race. |
| V05 | Existing auxiliary process-death boundaries and new durable Title restart/start-consumption tests. |
| V06 | `retries_commit_atomically_preserve_each_request_evidence_and_confirm_exact_receipts`, real writer commit/receipt-loss tests and Catalog visible-error fixture. |
| V07 | `cancellation_after_retry_scheduled_is_call_only_and_retains_permits_through_confirmation`; zero-request source release tests. |
| V08 | `retry_keeps_selected_occurrences_and_captured_media_after_source_mutation`; exact selected occurrences and no recapture. |
| V09 | Retry executor fixtures, absolute delay/deadline tests and normalized adapter permanent/transient cases. `specific_context_code_survives_a_broad_permanent_http_type` (OpenAI/Anthropic) and the SDK counterpart preserve semantic overflow codes independently of retry permission; `overflow_compact_and_retry_succeeds` and `a_rejected_summary_request_replans_against_a_smaller_budget` consume that normalized error. |
| V10 | Existing stream integrity/refusal/tool/guard/cancellation matrix in `runtime::auxiliary::stream_tests`. |
| V11 | `successful_partial_usage_is_never_labeled_complete_or_zero_filled`, `observed_subtotals_preserve_partial_fields_uncertainty_and_exact_request_identity`, adapter partial-usage fixtures and `partial_usage_without_input_retains_report_but_cannot_manufacture_occupancy`. `output_only_terminals_preserve_primary_inherited_and_compacted_occupancy` retains prior readings, accepts explicit zero and preserves Compaction's explicit clear; `occupancy_lookup_skips_output_only_history_before_the_bounded_limit` checks constant SQL work. Web/TUI distinguish missing fields from reported zero. |
| V12–V14 | Existing executor panic/waiter/cleanup tests, process-death and native App Server manager unload tests; new Title end-to-end manager fixtures. Result delivery/TTL remains #476. |
| V15 | `context::compaction_budget`, `summary_retry_reuses_frozen_wire_input_and_commits_one_checkpoint`, existing exact Compaction receipt/current-head tests. `cancellation_observed_by_compaction_budget_preserves_the_attempt_reason` cancels synchronously during budget construction and preserves UserRequested/RuntimeShutdown without Summary starts, checkpoints or failure events. |
| V16 | Existing selected-input/media, immutable-history, lineage and archive regression lanes. |
| V17 | `canonical::first_human` and `first_timestamp_less_human` finite metadata tests; `local_runtime::session::title` native Clone/historical Fork fixtures. |
| V18 | `retry_title` concurrent real SQLite starts/associations/restart, `zero_request_cleanup_preserves_other_calls_coalesced_capacity_wakes`, `title_lifecycle_busy_waits_for_commit_release_without_capacity_feedback`, Catalog rename/visibility faults and native `session_title` tests. |
| V19–V20 | Real Classification and BTW product paths remain explicitly scoped to #476; bounded repair/candidate/fan-out fixtures verify shared composition here. |
| V21 | Existing shared adapter preflight; explicit/follow Title invocation and invalid policy tests; selected media retry fixture. |
| V22 | Current-layout rejection tests, Catalog missing-field rejection, archive provenance and regenerated native/client schema checks. |
| V23 | Existing bounded errors/audit/framing tests, Title byte/text validation, finite first-Human query plan and client rendering tests. |

Title settings additionally have inline/deferred source/effective/model-policy,
whole-state model preservation, TOML/native parameter and Web/TUI tests. The
real browser Title fixture uses independent primary and Title provider scripts
so cross-consumer request order is not treated as deterministic. Unrelated strict
provider fixtures use a saved manual name; their primary request assertions are
preserved.

## Validation record

The PR lists executed commands and actual results. The final head must pass the
repository FULL profile (`full-verification`), including Rust quality,
contracts/provider, boundary/process-death and discovery checks, protocol
generation, dev/TUI/Web checks, translations/provenance, real transport/browser
acceptance and the applicable macOS Store/process lanes. Tests use the existing
provider emulator and no live inference credentials. Linux local results do not
stand in for macOS execution.

## Follow-up Title review regressions

* `anthropic_title_preserves_follow_and_explicit_thinking_budgets_and_text_limits`
  runs native Title ownership through the real Anthropic adapter and HTTP emulator.
  Both Session-following and explicit choices send max_tokens=4096 with a valid
  1024-token thinking budget. More than 480 bytes of reasoning does not become
  title text. Exact 120-character/480-byte titles publish; oversized text is
  rejected after one settled request with no corrective retry or renewed source.
* `title_lifecycle_busy_waits_for_commit_release_without_capacity_feedback`
  holds the actual lifecycle gate after the Store query returns and manually
  polls the production owner to Pending. Neither contention nor a coalesced
  external capacity release causes a second source read while the gate is held.
  Unlock causes exactly one new read and one native Title Call.

* `lifecycle_refusal_neither_reserves_capacity_nor_loses_release_wakes` uses an
  isolated process capacity domain to assert that repeated lifecycle refusals
  publish no capacity notifications, while a real external release stays unseen
  across the lifecycle wait.

A fresh independent subagent reviewed the complete follow-up diff and repeated
its audit after test corrections; both passes reported no actionable findings.
The 100 focused owner tests passed with no failures or ignored tests, including
all three regressions above, auxiliary execution/writer/stream contracts, model
policy, lifecycle, cancellation and Catalog Title modules.

## Linux boundary job budget

[FULL #1474](https://github.com/Caismis/rustX/actions/runs/39811600857)
reached the Linux job's 25-minute ceiling after all 810 selected boundary
tests passed (three existing conformance cases ignored), before its final
lane-coverage check. The prior #1472 run hit the same ceiling during external
targets. The check annotation confirms a 25m0s job timeout. In #1474, the
libclang download took 7m43s, external targets compiled in 5m37s, and the lib
harness compiled in 6m13s before 115.18 seconds of boundary execution. The Linux boundary job now has the same finite
45-minute ceiling as rust-contracts, covering preparation, complete execution
and final coverage validation. Its selected targets and individual test
liveness limits are unchanged. A complete final-head FULL run remains required.

The existing workflow checker passed all 24 tests, and native Linux discovery
for `rust-boundaries` passed with all selected targets covered.
