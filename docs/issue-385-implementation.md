# Issue #385 implementation record

The ownership graph is now explicit: User/Workspace own authored definitions;
Session Catalog state owns selections and explicit adoption; Attempts own immutable
execution snapshots; the App Server owns process bindings. `SourceTarget` is the
single native source authority (`user` or an authorized canonical `workspace`).
Source persistence and the captured application inputs are one native transaction.
After that transaction commits, the existing configuration coordinator owns all
application work; cancelling a browser or RPC future cannot cancel it.

Protocol v15 became v16. Source methods accept `SourceTarget` and no Session
parameter. Session operations are under `session/*`, including `session/settings`,
`session/models`, `session/setModel`, `session/configuration`, and the existing
explicit adoption operation. Generated Rust, JSON Schema, TypeScript, fixtures,
Web, TUI and documentation are updated atomically.

## Acceptance evidence

- C01 -> `tests/scripted/app_server/configuration.rs::c01_c02_c03_zero_session_source_authority_is_inert_and_isolated`; `tui/test/integration.test.ts::C01 C02 C12 C20 real TUI Settings author without Sessions or runtime allocation`; browser `settings-ownership.spec.ts` first test.
- C02 -> the same zero-Session native, TUI and browser tests, including Product Host canonical Workspace authorization.
- C03 -> the isolation portion of `c01_c02_c03_zero_session_source_authority_is_inert_and_isolated` and the User-only discovery regressions in `tests/scripted/app_server/configuration.rs`.
- C04 -> `c04_workspace_override_empty_and_removal_keep_distinct_source_intent` and `settings.test.ts` reset/override assertions.
- C05 -> `c05_broken_source_retains_revision_inventory_and_explicit_validated_repair` and `settings.test.ts` semantic rejection/repair assertions.
- C06 -> `c06_zero_session_process_hot_restart_reopen_and_revert`, `t03_mixed_application_and_true_process_binding_restart`, and `settings.test.ts::process state comes from native classification`.
- C07 -> `c07_zero_session_commit_transfers_ownership_before_lost_response` and the real browser lost-write path.
- C08 -> `t12_lost_source_write_response_does_not_cancel_native_application`, `settings.test.ts::repairs uncertain writes`, and the browser wire-level lost-write test.
- C09 -> `settings.test.ts::C09 Workspace A/B drafts survive navigation and Session focus without retargeting`, notification preservation, and the browser A/B draft path.
- C10 -> `settings.test.ts::C10 Workspace revocation disables mutation and preserves local draft`, stale response fencing, and the browser unregister path.
- C11 -> `c11_c12_model_selection_and_source_authoring_have_disjoint_durable_owners`, the native Session model tests, and the real browser Session isolation path.
- C12 -> `c11_c12_model_selection_and_source_authoring_have_disjoint_durable_owners` plus `tui/test/integration.test.ts` source/Session request assertions.
- C13 -> `session-configuration.test.tsx::C13 ready native eligibility submits exactly the inspected candidate and binding`, unknown-read retention, and the browser exact-candidate path.
- C14 -> `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`, `t11_admission_gate_orders_busy_adoption_without_cancelling_attempt`, and `session-configuration.test.tsx::C13/C15`.
- C15 -> the native admission gate test's deterministic settlement watch, the Web `C15 native Busy remains visible and becomes eligible on settlement observation without polling`, and the real browser native gate test.
- C16 -> `session-configuration.test.tsx::C16 lost adoption response rereads committed native state exactly once, never replays` and `late success A cannot hide newly observed candidate B`, plus the browser lost-adoption test.
- C17 -> `t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture`, `t02_later_tool_batch_and_model_step_keep_admitted_registry_policy`, and the existing Attempt/resource/history conformance suites.
- C18 -> `t15_unload_load_does_not_adopt_pending_context`, `t06_t15_configuration_save_keeps_cold_sessions_outside_residency_budget`, and the real browser reconnect/reopen path.
- C19 -> `t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate`, `t05_complete_policy_registry_publishes_while_instructions_remain_pending`, and the independent-unit browser coverage.
- C20 -> protocol generation/drift tests, `tui/test/integration.test.ts` rejection of removed ordinary commands, and the absence of legacy source methods in generated v16 contracts.

## Validation record

The implementation run executed formatting, all-target/all-feature Rust check,
warnings-denied Clippy, binary build, native unit/integration/boundary/conformance
suites, protocol generation and drift checks, Web typecheck/tests/build, TUI
typecheck/tests, the real provider emulator, real App Server + Product Host browser
E2E, and real App Server TUI integration. The final PR description records the
exact command results and any CI state.
