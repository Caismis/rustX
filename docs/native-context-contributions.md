# Native context contribution lifecycle (#383)

Native extensions remain compiled-in, closed Rust configuration. Adding a
producer using context requires its domain implementation, composition binding,
and tests, not Agent Loop, assembler, or adapter business logic.

## Ownership and boundaries

- Configuration preparation/publication, Session adoption, complete Attempt
  capture, derived-work inheritance, and resource retirement remain #380's
  authority. `NativeContextComposition::bind` consumes the admitted capability
  snapshot; contributors never query global latest configuration.
- At Attempt admission, configuration/resources/composition/read capabilities
  freeze, not mutable domain state. Goal owns its read-only revision capture.
  Todo owns its committed task list and bounded projection. Background owns its
  registry. Status owns reminder qualification, not those authorities.
- At logical-step preparation, `ContributorInputSnapshot` supplies finite
  execution facts, Surface, and `ContributionOpportunities`. Each registered
  producer captures/evaluates once even when FreshInbound and PostToolBatch
  coexist. These opportunities cannot schedule work.
- `ContextAssembly::assemble` validates provenance and structure before
  deterministic whole-proposal acceptance. Order is common semantic lane,
  stable producer identity, deferred/request-time phase, authoritative originating
  fact order, contributor-local order. User lanes are ClaimedInbound,
  RuntimeToolObservation, ExtensionEnvironment, TaskData. Goal and Status share
  TaskData. Registration and completion order have no effect.
- `AgentExecution::stage_context` binds runtime message identities in the sole
  `AcceptedContext`. Content, typed presentation, anchors, and semantic receipts
  stay together. No preparation advances durable reminder state.
- `PreStepPolicy` precedes the established cancellation gate.
  `start_model_turn` acquires that gate and checks cancellation. If cancellation
  won, no request startup effect exists. If startup won, the gate covers
  `ConversationStore::commit_model_turn_start`: one SQLite transaction commits
  canonical messages/Surface, RequestSnapshot, ModelRequestStarted, contribution
  receipts and their materialized heads. Failure rolls everything back.
  Post-commit installation is mechanical runtime work; there is no extension
  on-commit callback. Later cancellation cannot erase a committed startup.
- Every actual provider request gets its own exact RequestSnapshot. Transport
  retry, overflow recovery, and malformed-generation correction reuse the same
  logical-step accepted state and canonical identities without receipt advancement.
  Compaction may change projection, not domain capture. The next primary logical
  step resets accepted state and samples current domain revisions.
- ToolResultObserver remains a settled-tool read seam. Composition binds its
  producer; deferred proposals resolve against the same frozen registry and
  use the same validation, budgeting, ordering, and transaction. Observers cannot
  alter results, append history, or request another model invocation.

The accepted record is `ContributionStart` in `src/context/contribution.rs`.
Conversation persistence is canonical authority, not the Event Journal or a
client cache. Receipt heads are keyed by serialized producer identity plus local
semantic key, so two producers' `active` keys cannot collide. Receipt algorithms
remain domain-owned. Optional acquisition failures may be isolated with a
diagnostic; mandatory failure, invalid provenance, duplicate identity/key, and
structural/persistence failure cannot become success.

## Real producers and clients

`GoalContextContributor` owns Goal JSON and wording; Goal is User task data.
Root-only scope and admitted settlement/authorization remain unchanged. Visible
older Goal history never suppresses a new-step revision.

`StatusContextContributor` is an ordinary outer contributor. Internally,
`AgentStatusSectionProducer` captures an owned evaluation for Time, Background,
or Todo. The aggregator orders sources, validates typed payloads, isolates
optional acquisition failure, and admits complete sections under 4096 UTF-8
bytes. It never slices serialized structured content. Dropped sections carry no
receipts. A test section uses that same interface without capture/evaluation
dispatch changes.

No contribution means no new history, not deletion. Live Todo/Goal/Background
UI reads current domain projections. Historical Status observations and Trace
read the committed accepted presentation and its frozen native anchor. The
runtime mechanically publishes typed observations after commit; rendered text
is never parsed into state.

## Intentional contract changes

- Context compatibility ABI 5: common TaskData placement, native registration,
  shared deferred/request-time proposal vocabulary.
- SQLite schema 43: producer-scoped contribution heads and generic logical-step
  progress; RequestSnapshot stores `contributions`, not a separate status start.
  No migration or legacy reader is provided.
- App Server v20: Trace additions carry producer identity; request detail carries
  typed accepted contribution metadata. Rust schema, generated TS, protocol
  fixtures, Runtime Client, archive, Trace, TUI, Web, and dev dependencies are
  adapted together. The existing Runtime Client endpoint version is independent.
- Removed native Goal/Status assembler inputs, generic Goal rendering, separate
  `frozen_agent_status`, special startup receipt path, central section behavior
  dispatch, and the alternate native deferred identity marker.

## Acceptance matrix

Names below are exact test function names unless a browser/TS title is quoted.
Source tests are under `tests/scripted` (compiled as library suites); domain tests
live with their owners.

| Issue scenario | Deterministic proof |
| --- | --- |
| Absent/no-op and optional empty semantics | `issue383_production_composed_noop_and_optional_failure_preserve_runtime_semantics`; `ext259_todo_and_agent_status_are_independent_and_change_no_loop_semantics`; `issue383_noop_and_absent_extensions_share_the_cancellation_boundary` (same pre-start gate, cancel then release) |
| Production-composed test producer | `issue383_native_deferred_and_request_time_share_registered_provenance` |
| Shuffled registration | `issue383_shuffled_native_registration_and_producer_scoped_receipts` |
| Duplicate identity / invalid provenance | `issue383_duplicate_native_registration_rejects_attempt_before_start`; `issue383_mandatory_and_integrity_failure_prevent_startup` (actual foreign native metadata) |
| Both opportunities, one evaluation | `fresh_inbound_and_post_tool_batch_are_one_combined_set`; `module_matching_both_opportunities_captures_and_evaluates_once` |
| Reversed parallel completion | `parallel_tool_results_commit_in_canonical_call_order`; `deferred_post_tool_context_never_interleaves_between_sibling_results` (per-tool channels release/acknowledge B before releasing A) |
| Domain change after capture | `issue383_domain_changes_after_capture_only_reach_the_next_logical_step` (watch capture/release) |
| Cancellation wins | `issue383_native_preparation_cancellation_commits_nothing` (watch parks producer, cancellation completes, then release; zero provider/start/snapshot/receipt) |
| Startup wins | `issue383_native_start_wins_and_receipts_remain_truthful` (StartBoundaryPause inside cancellation gate; release transaction before cancellation can acquire gate; one truthful startup/provider call, receipt/snapshot preserved) |
| Startup persistence failure | `issue383_native_start_transaction_failure_rolls_back_receipts` (fault scripts after canonical append, receipt event, and receipt head; transaction rollback, zero provider/start/snapshot/receipt) |
| Transport retry and corrective request | `issue383_retry_and_corrective_requests_freeze_goal_and_native_receipts` (provider watch, mutate Goal/probe, release failure; retry old revision, next step new revision) |
| Overflow | `overflow_retry_reuses_the_admitted_context_generation`; `overflow_retry_preserves_pending_fresh_inbound_and_context_generation`; `transient_retry_does_not_regenerate_status_or_contributors` |
| Proposal/section budget exclusion | `issue383_common_budget_drops_content_and_receipt_together`; `global_admission_uses_utf8_bytes_whole_sections_and_continues` |
| Same local key, different producers | `issue383_shuffled_native_registration_and_producer_scoped_receipts` |
| Optional versus mandatory/integrity | `issue383_mandatory_and_integrity_failure_prevent_startup`; `optional_evaluation_failure_is_isolated_but_payload_integrity_fails` |
| Todo update/complete/clear and history | `every_settled_call_publishes_the_complete_list`; `todo_status_reads_only_the_committed_snapshot`; `todo_status_has_no_reminder_for_empty_or_fully_terminal_work`; `clear_drops_every_task_and_restarts_the_id_allocator`; `an_unusable_newest_snapshot_fails_the_rebuild_instead_of_reviving_an_older_one`; Web composer-context: “follows native projection changes and never reconstructs from historical todo Tool facts”, “R10: an all-completed list is not an empty list, and turn boundaries never clear the dock”, “R07: clearing current Todo retires the dock while the historical Agent Status annotation stays at its anchor” |
| Goal new revision despite visible history | `issue383_retry_and_corrective_requests_freeze_goal_and_native_receipts`; `goal84_natural_intent_creates_from_human_with_stable_tools_and_current_context` |
| Concurrent Conversation ownership | `issue383_concurrent_conversations_keep_domain_snapshots_and_receipts_isolated` (both captures acknowledge before shared release; same producer/key, different domain revisions/receipts) |
| Parent/child scope | `ext259_todo_is_supported_in_child_scope`; `goal84_root_only_scope_is_enforced_for_definition_model_and_workflow_overrides`; `two_concurrent_children_of_one_agent_stay_unambiguously_correlated` |
| Recovery/compaction/historical authority | `typed_status_metadata_survives_durable_restart_and_surface_rebuild`; `compaction_retiring_visible_status_reopens_surface_eligibility`; `retry_and_recovery_requests_introduce_no_duplicate_context`; `historical_status_and_history_never_revive_background_ownership` |
| Configuration changes during execution | `t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture`; `t02_later_tool_batch_and_model_step_keep_admitted_registry_policy`; domain revision tests above |
| Disable extension while admitted | `issue383_disabled_extension_keeps_admitted_configuration_until_settlement` (provider header gate, disable publication, release old steps, adopt for next Attempt); `ext259_disabling_todo_preserves_history_and_re_enabling_reconstructs_it` |
| Test Status section | `section_contract_captures_once_and_evaluates_owned_input` |
| Trace typed context and real clients | `canonical_context_is_projected_from_the_frozen_request_identities`; `all_context_assembly_semantic_pairs_project_from_durable_request_start`; browser `composer.spec.ts` “native Todo, Goal and Queue docks follow the real App Server through control, loss and reload”; `chat.spec.ts` actual historical Trace inspection |
| Terminal uniqueness and terminal-last | All three new startup cancellation/failure tests explicitly inspect durable terminal count and final Journal event; `issue383_mandatory_and_integrity_failure_prevent_startup` |

Race timeouts are liveness guards only. Watches, channels, the existing startup
gate, and transaction fault scripts establish ordering; no sleeps or scheduler
yields prove the new races.

Accepted `ContextGeneration` membership is derived after final admission from
surviving User contributions and request-time System sections. Registration,
invocation, empty output, isolated optional acquisition failure, and fully dropped
proposals do not establish membership. Each accepted owner appears once in stable
identity order with its authoritative registered attestation; native static System
owners remain represented when their sections are present. Same-step retries reuse
this frozen generation without invoking contributors again.

The persisted subset is covered by
`issue383_common_budget_drops_content_and_receipt_together` and
`issue383_production_composed_noop_and_optional_failure_preserve_runtime_semantics`.
`accepted_generation_retains_system_only_owners_and_registered_attestation` covers
System-only extensions (including failed optional User acquisition) and all static
native System owners. `accepted_generation_retains_partial_survivors_and_system_only_budget_exclusions`
covers partial survival, full exclusion, System-only survival, authoritative
attestations, and registration-order independence.
