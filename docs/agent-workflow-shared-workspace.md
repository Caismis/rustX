# Shared Agent and Workflow workspace (Issue #462)

Issue #462 removes two ownership domains that sat on top of ordinary Agent and
Workflow execution: the per-Agent Git worktree and the run-scoped Workflow
Candidate. Every Agent and Workflow now runs in the Session's frozen workspace
binding, Review is ordinary typed data, and the five Agent controls are one
automatically composed Root-only capability group.

## Behavior

- **Agent controls.** `subagent`, `list_agents`, `send_message`, `wait_agent`
  and `interrupt_agent` are composed together, exactly once each, for the Root
  Agent when `agent.agents` admits at least one valid named Agent, and are absent
  otherwise. They cannot be authored in `tools.builtin`, an invocation override
  or a native policy table. Named and Workflow children never receive them, and
  nested subagent delegation is unsupported. See
  [configuration](configuration.md#automatic-agent-controls-462).
- **Shared workspace.** Admission freezes the Session's workspace as one
  absolute path. Every activation of the same Agent runs exactly there, sees the
  parent's uncommitted state, and leaves its effects in place on success,
  failure, interruption or cancellation. rustX discovers, allocates and disposes
  no worktree, branch or copy. A missing or unusable binding fails before any
  allocation, with no fallback directory and no replacement directory.
- **Workflows.** A Workflow owns no workspace resource. Agent and Tool nodes act
  in the shared workspace; a successful run returns exactly its declared output.
  See [the Workflow contract](workflow-programs.md#shared-session-workspace-issue-462).
- **Review.** The subject is a Plan only. The result `{accepted, feedback}` gates
  later work only through an explicit Branch on `accepted`; Review takes no
  filesystem lease and a rejection never rolls back an earlier write.

## Ownership map

The retired `AgentWorkspace` state machine carried non-Git obligations. Each now
lives with the native owner that already enforced it:

| Former obligation | Native owner now |
| --- | --- |
| In-process resume exclusion (one resume winner) | `SubagentRegistry` mutex and the Agent's `ResumeReservation` |
| Cross-process exclusion of a second physical activation | `ParentPhysicalLease` for the exact activation allocation |
| `AwaitingPhysicalProof` | Registry `recovery_pending`/`recovery_unreserved`, checked per exact activation of the Agent; a late proof for activation N cannot release N+1 |
| Poisoned after failed rollback | `ResumeReservation` completion `Failed`, then the durable `AgentActivationAdmission` rollback fact |
| Unproven terminal settlement | Activation `physical_settlement_proven = false` until `SubagentPhysicalSettlementProven` |
| Orphaned activation without a terminal | `publication_abandoned`, which no physical proof clears |
| Git inspection, retained handoff, disposal and its poison | Deleted; no replacement |
| Workflow Candidate scope, freeze, validation borrow, acceptance, disposal | Deleted; no replacement |

An Agent is Unavailable while any of its own obligations is open. Session
deletion keeps the same physical and admission barriers and never removes,
inspects or blocks on project files.

## Integration with native child input and inspection

PR #461's attachment-bearing input remains owned by the existing native
admission path. Text-only Tool calls delegate to that same path; typed controls
can supply upload references. Active delivery and resumed delegation retain
canonical acceptance acknowledgements, input provenance and cancellation.
Resume checks the frozen directory off the Registry mutex before allocating an
incarnation, then checks it again during preparation. Attachment input does not
bypass either check.

Child conversation, Turn, Trace and cut-bounded history reads retain native
ownership. Child `present` uses the native Artifact/file-delivery authority;
sharing a Session path does not authorize another registered root. Delivery
still checks the exact child binding against the Session mapping and holds its
physical read authority through cancellation or detachment.

SQLite admits `SubagentOwnershipCommitted` only when Workflow kind and Workflow
origin agree, and the typed node's run names the event's parent Conversation.
The existing store/envelope Conversation check remains independent. Rejection
rolls back ownership, lifecycle and private authority together.
`commit_workflow_agent_terminal` remains the single atomic successful terminal
and output transition: Workflow, run, full node instance, visit, loop and
parallel identity must match the committed owner, including on retries.

The native control reader remains alive after a child Result until physical
anchors settle. Post-Result protocol violations still invalidate success;
logical completion cannot discharge unproven containment or publication.
No shared workspace directory is a cleanup or rollback resource.

Regression evidence includes
`workflow_ownership_requires_matching_kind_origin_and_parent_conversation`,
`normal_agent_creation_and_resume_ownership_keep_their_native_admission_paths`,
`workflow_terminal_rejects_inconsistent_and_substituted_typed_identities`, the
three `resume_workspace_preflight_*` tests (including attachment input), native
child delivery tests and post-Result containment tests. The real child
`a_real_child_executes_native_tools_in_the_frozen_nested_workspace` still uses
production Write, Edit, Read and Bash calls and verifies the resulting bytes,
Git refs, worktree list and directory identity independently.

## Format and version inventory

The table records the shared-workspace integration's format changes. Old data
is refused at its boundary and left untouched: nothing is migrated, rewritten,
rehashed or deleted. Each version domain has its own owner and negotiation.
Issue #467 subsequently advances SQLite to 58, Parent/Child IPC to 33, Runtime
Client to 69, App Server to 48 and Session catalog to 16. Event envelope 1 remains
unchanged. The current generated App Server pair is `v48.*`.

| Boundary | Owner | Main before integration | PR before integration | Combined | Breaking format / rejection | Proof |
| --- | --- | --- | --- | --- | --- | --- |
| SQLite conversation store | `src/durable/sqlite.rs`, `SQLITE_SCHEMA_VERSION` | 55 | 51 | 57 | Shared path ownership and Candidate removal combined with canonical child history, input provenance and context checkpoints; old versions return `SchemaVersionMismatch` without writes | `pre_integration_stores_are_refused_and_left_untouched`, Workflow ownership/output atomicity tests |
| Parent/child IPC | `src/runtime/subagent/ipc.rs`, `SUBAGENT_IPC_VERSION` | 31 | 31 | 32 | Frozen workspace path plus attachment-bearing input and inherited provenance; either v31 Hello shape fails before composition | `both_version_31_hello_formats_are_rejected_before_runtime_composition`, real child attachment and native Tool suites |
| Runtime Client | `src/runtime_client/types.rs`, `RUNTIME_CLIENT_PROTOCOL_VERSION` | 66 | 62 | 68 | Shared Agent workspace path, no disposal/Candidate, with child Turn/Trace/history and typed attachments; negotiation rejects other versions | `attachment_request_correlation_and_version_negotiation` |
| App Server | `src/app_server/protocol.rs`, `APP_SERVER_PROTOCOL_VERSION` | 44 | 40 | 46 | Runtime Client changes reflected in RPC and generated schemas; initialize and WebSocket negotiation reject old peers | `current_app_server_documentation_matches_negotiated_version`, generated `v50.*`, `pnpm check` |
| Agent definition digest | `SUBAGENT_DEFINITION_DIGEST_VERSION` | v5 | v6 | v6 | Worktree authoring removed; old frozen digests do not match | `sub258_the_profile_framing_is_versioned` and definition digest tests |
| Execution profile digest | `SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION` | v3 | v4 | v4 | Shared binding and automatic controls; old frozen digests do not match | `sub258_the_profile_framing_is_versioned` |
| Deletion revision token | `rustx/session-deletion-ownership` tag | v1 | v2 | v2 | No disposable workspace ownership; old tokens are stale | `deletion_never_removes_the_shared_session_workspace` |
| Agent profile TOML | `agent.schema.json` | `[worktree]` | absent | absent | Unknown field rejected | `retired_agent_worktree_authoring_is_refused` |
| Workflow YAML | `workflow.schema.json` | `workspace:` | absent | absent | Unknown field rejected | `retired_workspace_authoring_and_projection_fields_are_gone` |
| Review subject | `ReviewSubject` | `plan`/`candidate` | `plan` | `plan` | Unknown variant/field rejected | `retired_candidate_review_vocabulary_is_refused` |
| Agent wire workspace | `RuntimeClientAgent.workspace` | object | path | path | Old object does not decode as the new path | `the_runtime_client_projection_carries_the_named_identity` |

Event envelope 1, Session catalog 15 and `CURRENT_RUNTIME_SCHEMA_VERSION`
retain main's versions; this integration does not add another format change to
those owners. Strict authoring decoders reject removed `worktree` and
`agent_workspaces` fields. Child IPC 31 was independently assigned to two
incompatible formats; neither is accepted as IPC 32.

## Developer upgrade contract

There is no compatibility layer. After upgrading, start from a fresh Session
store. Unsupported store and catalog versions, and older protocol peers, are
refused explicitly; persisted data is left exactly as it was. The shared-workspace integration retained Session catalog version 15;
Issue #467 advances it to 16 with bounded identity metadata. Worktrees and branches
that earlier builds created under the product root are no longer managed by
rustX; inspect and remove them with ordinary Git commands if they are no longer
wanted. Remove `[worktree]` tables from Agent profiles, `workspace:` keys from
Workflows, `candidate` Review subjects, and the five Agent controls from any
`tools.builtin` list.

## Regression map

Lanes: **C** = `rust-contracts` (`cargo test --lib --bins --examples -- --skip
boundary_suites::`, `--test contracts --test provider`), **B** = `rust-boundaries`
(`boundary_suites::` and the external targets, provider emulator required),
**T** = `tui`, **P** = `app-server-protocol`, **W** = `web-console`.

### Group A — Native tool composition

| ID | Tests | Lane |
| --- | --- | --- |
| A01, A03, A13 | `domain_tool_metadata_matches_registered_authority` | C |
| A02, A04, A06, A15 | `dispatch_allowlists_do_not_expose_child_or_workflow_private_tools` | C |
| A05, A15 | `agent_controls_cannot_be_authored_in_root_or_named_profiles`, `sub258_the_model_cannot_name_its_own_delegation_authority` | C |
| A07 | `recursive_and_execution_selections_are_rejected_at_admission`, `cfg273_delegation_composes_dispatcher_without_granting_child_tools` | B, C |
| A08, A09, A10, A14 | `issue462_agent_control_group_follows_one_admitted_generation` | C |
| A11 | `real_child_resumes_same_identity_history_and_frozen_authority` | B |
| A12 | Unchanged Job, native Tool, Skill, MCP and Plugin suites | C, B |
| A16 | `fixed_admission_rejects_orchestration_background_and_composite_leaves` | C |

### Group B — Shared ordinary Agent execution and recovery

| ID | Tests | Lane |
| --- | --- | --- |
| B01, B02, B03 | `a_real_child_shares_the_session_workspace_subdirectory`, `a_nested_binding_child_never_discovers_ancestor_project_instructions` | B, C |
| B04, B05 | `an_ordinary_child_shares_a_dirty_git_workspace_without_worktrees`, `a_real_child_shares_the_session_workspace_subdirectory` | C, B |
| B06 | `child_approval_allow_routes_to_child_and_runs_exact_invocation`, `child_approval_deny_routes_to_child_and_never_starts_executor` | C |
| B07 | `a_subagent_child_runs_end_to_end_through_the_real_process_stack` | B |
| B08, B15 | `real_child_resumes_same_identity_history_and_frozen_authority`, `proven_child_loss_has_identical_resume_authority_before_and_after_reopen` | B, C |
| B09 | `unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent`, `recovered_physical_receipt_requires_owner_release_and_durable_proof` | C |
| B10 | `agent411_crash_reconciliation_cannot_invent_physical_resume_proof`, `agent411_recovery_never_reopens_unproven_physical_containment` | C |
| B11 | `sibling_agents_run_concurrently_in_one_workspace` | C |
| B12 | `deletion_never_removes_the_shared_session_workspace`, `deletion_nested_durable_children_restart_without_owning_the_shared_workspace` | C |
| B13 | `a_child_may_select_a_capability_that_is_available_but_inactive_for_the_parent`, `cfg273_delegation_composes_dispatcher_without_granting_child_tools` | B, C |
| B14 | `sub258_an_override_restating_the_defaults_is_equivalent_to_none`, `sub258_an_empty_extension_override_composes_nothing`, `sub258_override_selections_are_canonically_normalized` | C |
| B16 | `concurrent_resume_reserves_exactly_one_activation_before_preparation` | C |
| B17 | `unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent`, `cold_shared_agent_deletion_retains_unproven_terminal_evidence` | C |
| B18 | `a_terminal_publication_failure_is_retried_then_abandoned` | C |
| B19 | `agent411_resume_reservation_recovery_requires_rollback_containment_proof`, `interrupted_authority_publication_reopens_without_logical_facts`, `initial_uncommitted_physical_authority_consumes_activation_identity_on_restart` | C |
| B20 | `pre_reserved_wait_settles_independently_of_another_agents_recovery`, `unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent` | C |
| B21 | `a_missing_workspace_binding_fails_without_fallback_or_creation` | C |
| B22 | `agent411_crash_reconciliation_cannot_invent_physical_resume_proof`, `a_terminal_publication_failure_is_retried_then_abandoned` | C |

### Group C — Workflow without Candidate

| ID | Tests | Lane |
| --- | --- | --- |
| C01, C05, C06, C07, C12 | `implement_check_repair_runs_in_the_shared_session_workspace`, `shipped_repair_uses_real_writes_checks_and_root_human_decisions`, `shipped_repair_exhaustion_leaves_completed_writes_without_another_body` | C, B |
| C02, C13 | `retired_workspace_authoring_and_projection_fields_are_gone`, `retired_candidate_review_vocabulary_is_refused` | C |
| C03, C04, C17 | `explicit_review_branch_gates_implementation` | C |
| C08 | `cancellation_during_execution_drains_native_settlement_before_node_terminal`, `cancellation_at_second_iteration_frontier_consumes_and_starts_nothing_new`, `parallel_runtime_cancellation_drains_every_admitted_child` | C |
| C09 | `rejection_after_a_write_never_rolls_it_back` | C |
| C10, C18 | `pending_review_does_not_block_an_independent_parallel_branch` | C |
| C11 | `workflow_output_latch_is_exactly_once_and_cancel_is_terminal`, `projection_rejects_oversized_or_missing_parts_without_parsing_text` | C |
| C14 | `review_response_is_bound_to_exact_visit_and_presented_subject`, `review_wrong_instance_subject_and_kind_leave_original_pending_detach_resync_is_live_only` | C |
| C15 | `sub258_override_selections_are_canonically_normalized` | C |
| C16 | `published_initialization_error_retains_live_registry_obligation_until_exact_proof`, Session deletion barrier tests above | C |
| C19 | `a_workflow_child_cannot_be_resumed_through_agent_controls`, `workflow_activations_do_not_become_continuable_agent_rows` | C |
| C20 | `a_workflow_child_closes_native_lifecycle_without_parent_inbound` | C |
| C21 | `workflow_agent_value_and_child_terminal_are_one_atomic_transition` | C |
| C22 | `tool_node_admission_is_capability_plane_selection_without_agents`, `shipped_repair_checker_tampering_cannot_redefine_frozen_verification` | C, B |
| C23 | `every_native_non_success_survives_the_actual_outer_adapter_once`, `workflow_output_and_cancellation_have_one_linearized_winner`, `repeated_outer_tool_correlation_does_not_reuse_run_identity` | C |

### Group D — Format, protocol and UI conformance

| ID | Tests | Lane |
| --- | --- | --- |
| D01 | `committed_rust_artifacts_are_current`, `cfg235_checked_in_schemas_match_authoritative_generation`, `pnpm check` and `pnpm typecheck` | C, P, T, W |
| D02 | `retired_agent_worktree_authoring_is_refused`, `retired_workspace_authoring_and_projection_fields_are_gone`, Web settings round-trip test | C, W |
| D03 | Web `settings-units` Agent controls note, TUI `convergence.test.ts` | W, T |
| D04 | TUI `subagent-activity`, `workflow-projection`, `hitl` tests; Web `subagents` and `chat` tests | T, W |
| D05 | Session archive tests and `queued_old_review_cannot_resurrect_a_settled_workflow_node` | C |
| D06 | `pre_integration_stores_are_refused_and_left_untouched` | C |
| D07 | `a_version_30_hello_is_refused_before_composition`, `attachment_request_correlation_and_version_negotiation` | C |
| D08 | `proven_child_loss_has_identical_resume_authority_before_and_after_reopen`, `real_child_resumes_same_identity_history_and_frozen_authority` | C, B |
| D09 | Full unchanged suites in every lane | C, B, T, P, W |
