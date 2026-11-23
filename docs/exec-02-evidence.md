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
| T48, T58, T59 | `duplicate_register_attaches_and_second_start_cannot_spawn_twice`, `cancelled_background_prepare_explicitly_discards_its_capture_without_reviving`, `accepted_bash_cancelled_before_executor_construction_settles_output`, `accepted_bash_cancelled_before_first_operation_poll_settles_output`, `accepted_bash_cancelled_before_start_keeps_the_empty_output_readable` | Duplicate registration and START reuse the same identity and produce exactly one launch. A preparation owner releases the unaccepted capture by explicit discard. Three cancellation cuts on an accepted Job cover before executor construction, before the first operation poll, and after the first driver poll but before Client START. Each has zero business launches, exactly one canonical terminal and the same empty locator readable through Native Read. |
| T63 | `failed_bash_capture_unlink_retains_its_owner_and_quota_for_retry`, `child_drain_uses_the_physical_output_and_capture_receipts` | A Client unlink failure leaves the exact file, cleanup handle, and quota in place. A retry verifies removal before returning `OutputReleased` and returning the permit. The delegated relay requires the discard acknowledgement to release capture responsibility. |
| T60, T61 | `replay_uses_the_original_registration_after_the_cwd_disappears`, `cancelled_before_register_uses_one_reserved_identity_at_capacity`, `a_foreign_key_is_refused_before_lookup` | Filesystem changes do not override the original registration. A failed START cannot revive. At 4,096 observed identities, existing active and terminal records keep their capacity. The first valid registration fixes the fingerprint of a cancel-before-register tombstone. The Client rejects foreign instance or generation keys. |
| T63 | `dropping_a_worker_without_physical_proof_keeps_revoke_unconfirmed`, `real_supervisor_wait_failure_keeps_the_unknown_proof_after_output_release`, `revocation_reclaims_other_captures_after_an_unlink_failure` | A final wait fault after a real process produced an exit status yields Unknown without an exit code. Its physical responsibility survives journal release and shutdown remains unconfirmed while an unrelated Bash capture is deleted. A separate unlink fault retains the failed file and permit while the sweep removes another capture. |
| T11 (capture), T63 | `lazy_foreground_allocation_does_not_block_cancel_or_health`, `background_capture_preparation_keeps_control_independent_and_settles_its_owner` | Barriers pause foreground overflow allocation and background registration independently. The Client control worker acknowledges another operation's pre-START cancellation and then Health before allocation resumes. Background preparation tests exact replay, a conflicting fingerprint, the START gate, and file and quota settlement after acceptance, cancellation, or Revoke. |
| T63 | `unreadable_foreground_capture_keeps_its_owner_until_discard_succeeds`, `foreground_bash_discards_unavailable_capture_after_journal_release` | The Client test injects append and rollback failures, then confirms that a failed unlink leaves the unavailable file and permit in place until a successful discard. The Server boundary runs Native Bash with a real Client and projects an Unavailable capture and one discard conflict through a proxy. It verifies journal release precedes both discard attempts, the real Client removes the file on retry, and Health succeeds on the same binding. |
| T63 | `foreground_bash_reclaims_capture_after_query_expires_before_dispatch`, `promotable_bash_keeps_capture_until_private_foreground_ownership_finishes` | Real data-lane contention exhausts only the BashCapture query's budget before send. The Client receives no capture query. Native Bash returns Unavailable, and journal release precedes confirmed deletion. A promotable stage keeps its file readable until the registry finishes the private execution without publishing a job. Health succeeds on the same binding, which then accepts all 32 capture reservations. |
| T63 | `promotable_bash_discards_client_unavailable_capture_once` | The proxy projects a Client-reported Unavailable into a promotable Bash that finishes privately. Journal release precedes the single successful discard. Foreground finish reuses the same owner, whose claim rejects another request before data admission. The binding still accepts all 32 capture reservations and executes another command. |
| T63 | `discard_proves_a_missing_capture_is_unlinked_and_preserves_replacements`, `discard_retains_quota_until_the_last_capture_link_is_gone`, `discard_reclaims_unlinked_capture_with_a_replacement_at_its_name` | Renames and extra hard links retain ownership and quota. After external unlink removes the final link, repeat discard confirms removal and returns the permit even if a replacement occupies the original name. The replacement bytes remain intact. |
| T63 | `child_cleanup_requires_physical_settlement_before_and_after_journal_release`, `unknown_command_settlement_preserves_the_binding_and_output` | A proxy projects the same typed snapshot from a real Client. Delegated cleanup queries for proof in both release orderings, preserves the runtime root, and publishes no quiescence proof. The App Server projects the domain failure with its captured output and continues using the pair. The physical wait fault itself is exercised in the separate Client test above. |
| T63 | `unused_delegation_is_sealed_against_late_effectful_dispatch`, `reserved_crash_prefixes_recover_exact_identity_without_reuse` | A child can publish its local drain after sealing unused delegated dispatch. Preparing a child and losing the parent before it sent retained work preserves the exact activation's prior recovery contract. A retained-work send and unused sealing exclude one another. |
| T63 | `lost_delegated_resource_receipts_keep_physical_cleanup_unconfirmed` | A real Client prepares a Bash capture or mutation, or publishes a write. The proxy drops the original receipt, substitutes an invalid ticket, or returns `CommitUnknown` and gates Client-side teardown. The parent keeps the child root and records no quiescence proof while the binding's physical settlement remains unconfirmed. |
| T61 | `revision_one_cannot_admit_either_lane_in_revision_two`, `websocket_session_version_and_generation_mismatches_stay_reserved` | The revised snapshot is mandatory in wire revision 2. Revision 1 is refused during Client bootstrap or on either Server Hello before a business lane exists. |
| T63 (stdio START cut) | `cancellation_at_the_retained_anchor_prevents_interactive_start` | A shared supervisor waits for the anchor acknowledgement, then observes cancellation before writing START. The fixture proves an absent business marker and settlement of the exact retained anchor. The same driver runs Client-owned stdio. |

## Restart admission fence

The App Server and standalone Server persist one exclusive record for the
exact Session binding before either lane can admit a Client. The record
contains identity and Workspace authority. The credential remains in its
private bootstrap. The live binding owner releases the record only after
an acknowledged physical Revoke or proof that the pair never admitted
work. An unconfirmed disconnection leaves the record in place across
Server restart.

Two execution-server tests cover exact owner release and unconfirmed
retirement. The Linux process regression
`server_sigkill_keeps_prior_client_binding_unavailable_after_restart`
admits the Client and reads a fixture file, sends SIGKILL to the real
Server, then verifies that a fresh Server refuses the original Session
binding. An independent Session admits its own Client and the original
Session's history remains accessible. The same process test then commits
deletion of the offline Session and checks that both the initial cleanup
and retry report cleanup pending. The private database and the Client's
Workspace file remain intact. Standalone history composition retains an
unavailable execution scope while the fence is present. The conformance
test `a_crash_after_the_request_start_commit_never_resends_the_request`
reconstructs the frozen request after restart, observes its indeterminate
settlement and asserts exactly one provider request.
Independent attestation and cleanup-only
reconnection remain open.

Client save expiry is owned by a maintenance thread outside the data and
control lanes. An abandoned unpublished file is retried after its deadline
even if Abort could not be sent and no later save request arrives. The
descriptor and its slot remain held until removal; an already published
file is excluded from expiry, including when its publication reply was
lost. The storage regression injects one unlink failure, verifies retained
capacity, then proves retry releases exactly one slot and the published
file remains readable.

## Same-instance reattachment

| ID | Evidence | Scope |
| --- | --- | --- |
| T07, T08 | `same_instance_recovery_preserves_the_process_and_output_cursor` | Two real paired channels are replaced. The existing process starts once, a retained stdin sequence is acknowledged without a second write, and pages continue at the exact prior byte offset. The eventual Revoke returns a physical receipt. |
| T10, T57 | `credential_replay_after_eof_never_reconnects`, `websocket_recovery_reauthenticates_both_lanes_for_the_same_instance` | The launch bundle cannot reattach a detached binding. Both TLS lanes authenticate again with fresh lane identities and the original binding identity. In-flight mutation tickets from the old lane cannot be recycled. |
| T10, T47 | `reattachment_retires_old_watchers_before_admitting_a_rescan` | The reconnecting Client rejects the old WatchId. A direct rescan reads changes made while detached. The new watcher reports a later change with its own identity, without publishing the older change as a new hint. |
| T43, T65 | `either_one_way_control_failure_recovers_the_same_authenticated_instance`, `a_silent_control_peer_can_recover_after_the_probe_expires`, `recovery_admission_is_closed_at_the_fixed_deadline`, `detached_retention_covers_either_detection_order` | Real Unix Client recovers after either half close and after its control exchange times out. Logical time tests step the exact Server cutoff and the two detection orders under the control frame scheduling assumption. Client death and native macOS failure remain separate gates. |
| Offline deletion | `an_offline_deleted_session_accepts_only_its_original_cleanup_owner` | After durable logical deletion, admission remains closed and the original Client reattaches solely to return one Revoke receipt. Cleanup recovery then completes deletion and preserves the Workspace file. |

The live protocol uses a 100 ms idle control poll and a 2 s normal frame
deadline in both directions. The recovery model allows one in-flight frame
and the next idle poll and frame budget (4.1 s) to observe a one-way loss.
It assumes that both workers can run within the frame deadlines. A stalled
process or scheduler beyond those assumptions can shorten recoverability.
The Server admits until 50 s after its observation, and the detached Client
starts cleanup by 80 s after its own observation. Expiry records no receipt.

The existing Store publishes an environment with an owner-only sibling stage
and an exclusive rename under the cancellation grant. The two preparation
race tests hold the physical fixture at `uv sync`. Independent Store instances
both build, verify the same published winner and remove their own stages. A
different actor claiming the fingerprint with an empty directory keeps its
original inode and content while preparation fails. This establishes the
exclusive Store publication cut. The production Managed Python migration
to the Client remains open.

## Open gates

- Replacement admission and post-restart environment recovery still require
  independent attestation tied to the original physical domain. The durable
  admission fence retains the unresolved identity. The Linux SIGKILL case
  observes one business socket and exercises isolation between Sessions.
- The physical candidate Spill writer, Root stdio and delegated stdio relay
  run through the bound Client in EXEC-03. The Client-owned Managed Python
  Store, full restart cleanup and complete child integration remain open.
  Native macOS fault injection and final exact-head verification remain open.
