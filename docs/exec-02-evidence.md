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
| T48, T58, T59 | `duplicate_register_attaches_and_second_start_cannot_spawn_twice`, `cancelled_background_prepare_frees_its_capture_without_reviving` | A query misses while the original registration is parked. The same-identity registration and concurrent STARTs reach the Client first, then the original attaches. A supervisor gate holds business START. Both argv and Bash record exactly one physical launch. Bash preparation returns the same capture allocation, and a cancelled preparation removes it before START. |
| T60, T61 | `replay_uses_the_original_registration_after_the_cwd_disappears`, `cancelled_before_register_uses_one_reserved_identity_at_capacity`, `a_foreign_key_is_refused_before_lookup` | Filesystem changes do not override the original registration. A failed START cannot revive. At 4,096 observed identities, existing active and terminal records keep their capacity. The first valid registration fixes the fingerprint of a cancel-before-register tombstone. The Client rejects foreign instance or generation keys. |
| T63 | `dropping_a_worker_without_physical_proof_keeps_revoke_unconfirmed` | Returning or dropping the worker without physical proof keeps Revoke unresolved. The guard persists that fact independently of output and business settlement. The test exercises the guard transition. |

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
