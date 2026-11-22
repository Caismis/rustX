# EXEC-02 implementation and evidence

[#577](https://github.com/Caismis/rustX/issues/577) builds on
`feat/576-execution-boundary` at
`d465c0d5b4cb6b355aacd773e3b4f667e0414894`. The four-issue series uses
PR #580 as the integration branch. The pinned design references and the
acceptance matrix are in the [execution protocol ADR](execution-protocol.md).

## Initial lifecycle changes

| ID | Executable evidence | Coverage |
| --- | --- | --- |
| T08 | `same_input_attaches_until_the_receipt_is_claimed` | A real `cat` consumes the original bytes once. Identical retries attach before and after output release. Changed bytes or EOF intent conflict. Once the receipt is claimed, the same tuple has no replay evidence. This covers the Client registry on the same generation. |
| T09 | `client_sigkill_keeps_other_sessions_and_history_available` | A Linux process fixture kills the real Client after the command writes an independent marker. A second Session executes, a third remains readable with no Client, and the first process closes its socket. Host drain retains that binding's unconfirmed receipt. |
| T48, T58, T59 | `duplicate_register_attaches_and_second_start_cannot_spawn_twice`, `cancelled_background_prepare_explicitly_discards_its_capture_without_reviving`, `accepted_bash_cancelled_before_start_keeps_the_empty_output_readable` | Duplicate registration and START reuse the same identity and produce exactly one launch. A preparation owner releases the unaccepted capture and recovers its quota by explicit discard. An accepted Job cancels after the first driver poll but before START reaches the real Client, launches no business process, and retains its exact empty locator through Native Read. |
| T60, T61 | `replay_uses_the_original_registration_after_the_cwd_disappears`, `cancelled_before_register_uses_one_reserved_identity_at_capacity`, `a_foreign_key_is_refused_before_lookup` | Filesystem changes do not override the original registration. A failed START cannot revive. At 4,096 observed identities, existing active and terminal records keep their capacity. The first valid registration fixes the fingerprint of a cancel-before-register tombstone. The Client rejects foreign instance or generation keys. |
| T63 | `dropping_a_worker_without_physical_proof_keeps_revoke_unconfirmed`, `real_supervisor_wait_failure_keeps_the_unknown_proof_after_output_release` | The Client test injects a final wait error after a real process produced a numeric exit status. The operation publishes Unknown without an exit code, retains its unconfirmed physical responsibility across journal release, and prevents a confirmed Revoke. |
| T63 | `child_cleanup_requires_physical_settlement_before_and_after_journal_release`, `unknown_command_settlement_preserves_the_binding_and_output` | A proxy projects the same typed snapshot from a real Client. Delegated cleanup queries for proof in both release orderings, preserves the runtime root, and publishes no quiescence proof. The App Server projects the domain failure with its captured output and continues using the pair. The physical wait fault itself is exercised in the separate Client test above. |
| T63 | `unused_delegation_is_sealed_against_late_effectful_dispatch`, `reserved_crash_prefixes_recover_exact_identity_without_reuse` | A child can publish its local drain after sealing unused delegated dispatch. Preparing a child and losing the parent before it sent retained work preserves the exact activation's prior recovery contract. A retained-work send and unused sealing exclude one another. |

## Open gates

- Bounded same-instance reconnection and the two detection orders still
  require transport changes and virtual-clock tests. The current code
  fences the pair on disconnect.
- Replacement admission and Server restart still require independent
  attestation tied to the old physical domain. Client guard evidence lives
  in process memory. The Linux SIGKILL case observes one business socket
  and exercises isolation between Sessions.
- Physical candidate Spill, offline cleanup, and the Client-owned Managed
  Python Store and stdio MCP still require implementation. Native macOS
  fault injection and final exact-head verification remain open.
