# EXEC-01 acceptance ledger

This is the implementation/evidence ledger for [#576](https://github.com/Caismis/rustX/issues/576), not a second test registry. Cargo discovery and the [lane checker](../examples/check_test_lanes.rs) are authoritative for test execution. The [ADR](execution-protocol.md) owns the wire contract, frozen research revisions, and T01-T73 phase index. "Partial" below means that the named proof does not cover the whole PR1 requirement.

The PR baseline for the Agent Bash work was `feat/576-execution-boundary`
at `54eade136bd16200bcb1b73e24dd5bd3c6c08c54`, with PR #580 targeting
`main` at `50d0e5728851143b42dfa70f53458eb4f3ebf0b7`. Its
[CI run](https://github.com/Caismis/rustX/actions/runs/44690282490)
failed in the Rust FAST core. The local foreground/background output test
attempted a native Read with no admitted Client. Merge verification failed
and the FULL jobs were skipped.

Initial audit: original implementation base `f0cd4f59d7dbdd2730cf262d0afa964af79d9f39`, reviewed branch head `a24cbf6e6059490c8337c34bcbfc2829e5287d6a`, fetched main `15d95ec1ba5f5a19c3975c52633b6bfec8a08135`. #571 merged into the branch of #570, not into this main. Do not infer an accepted workspace layout from its merged badge.

## Review follow-up on `98bb34197`

- The proxy holds the real Client's first `InputAccepted` while `cat`
  echoes the complete input over a separate socket. Closing the proxy's
  return path gives the public API `outcome_unknown`. The binding retires
  after one admission, with no replay, no additional echoed bytes, and
  no final receipt request. A preceding malformed input returns
  `invalid_params` before dispatch and leaves sequence one available.

## Review follow-up on `3939bf702`

- Before expiring the final input receipt request, both cleanup regressions
  observe the full input echoed by the real Client's `cat` process. The public
  response is `outcome_unknown`, with zero receipt requests sent. The Open
  binding retires and the Revoking binding delivers its original `Revoked`
  receipt to complete drain.
- A Client test pauses after a real one-byte pipe write and injects an error on
  the next write. It observes exactly that byte on stdout and claims one
  `Failed` receipt. A separate real Client boundary test takes an expired
  ordinary poll through the final claim, observes the matching `Failed`
  receipt on the wire and `outcome_unknown` at the public API, then runs
  another command on the same binding.

## Review follow-up on `bf38fcd00`

- Each accepted input retains a Server receipt owner. An ordinary polling
  failure leads to a final bounded claim or abandonment. If no reply proves
  disposal, the owner fences an Open binding or leaves cleanup with the
  terminal owner.
- The real Client accepts one input in both regressions. The test holds the
  data lock and expires the final exchange before send. An Open binding is
  retired. During terminal settlement the original `Revoked` receipt stays
  deliverable and Host drain succeeds. The proxy observes zero input receipt
  frames in both cases.

## Review follow-up on `f4c0519b6`

- Public configuration reconciliation parks before reading the authenticated
  Client's Workspace document with an invalidated MCP catalog. Unload releases
  the old Runtime, and Session deletion acquires the allocation while
  reconciliation is still parked. Resuming it produces no further discovery
  for the retired incarnation.
- A real Client accepts WorkspaceChanges Begin. Dropping the Server record
  queues Release behind the data writer. While the Client's actual Revoked
  receipt is held, the queued cleanup finishes without sending Release or
  closing the control lane. Delivering the original receipt completes drain.
- A command writes bytes and EOF on real Client stdin. The input call is held
  before polling the receipt until the command exits and the Client
  acknowledges ReleaseOutput. The original input then reports Applied.
  With physical EOF held, the Client admits eight controls, rejects the
  ninth before assigning its sequence, and retains the first Pending
  receipt. Abandoning another pending receipt permits the same ninth
  sequence, and the first receipt returns Applied exactly once.

## Review follow-up on `f4545e8a2`

- The physical Client publishes one write and returns `Written`. The
  proxy holds the data receipt while the watcher observes `Unknown` for
  that same ticket. With a real `Revoked` receipt also held, the watcher
  completes its fencing decision and lets the original write and drain
  settle in order. The data lane sees one commit request.
- The cancelled preparation regression now waits for that ticket's
  cleanup task to complete its final fencing decision while the Revoke
  receipt is still held. The writer and data permit assertions continue
  to cover the cleanup's admission.

## Review follow-up on `d03310bb8`

- The cancelled preparation regression parks a real Client's prepared
  write reply, then queues its Abort behind the Server writer permit.
  While the actual `Revoked` receipt is parked, cleanup relinquishes its
  data permit without an Abort frame. Drain succeeds and the file stays
  absent after the Client exits.
- The delegated relay retains a write ticket and a live Bash capture
  through the production forwarding path. Revocation removes the capture
  before the receipt. The relay joins settlement while the receipt is
  held and reports its result after delivery, with zero publication or
  business START.
- Client control-worker tests park START just before worker scheduling
  and Discard just before releasing the removed capture. Each reports one
  outstanding owner during Revoke and receives `Revoked` only after the
  capture directory is gone. The parked START creates no business marker.
- Authored configuration rejects a regular file replacing the captured
  Workspace directory at the same path. The existing MCP preparation
  regression keeps coverage of a replacement directory.

## Review follow-up on `e162692ff`

- Session deletion fences admission locally, then waits outside the
  residency mutex. While the proxy holds the Client's actual `Revoked`
  receipt, another Session still admits operations. Dropping the receipt
  stops durable deletion and Host drain reports the retained failure.
- The Bash discard race holds a real Client's `Revoked` receipt after
  verifying that the Client already removed the capture. The queued
  discard hands off to terminal settlement, sends no Discard frame,
  and leaves the control lane open for the receipt to reach the Server.
- The Client-only configuration case continues to cover a Workspace
  absent at initial admission. A separate overlay regression removes
  then replaces a previously admitted Server resource root. Independent
  policy continues to apply and the Workspace Agent remains adopted.
  Restoring the original root reopens resource adoption.
  The existing authored configuration regressions still report a redirected
  source and require fresh MCP preparation when a queued owner goes stale.
- WorkspaceChanges tests a same-index retry against a completed result.
  The retained result remains claimable after its comparison source is
  removed. The original deadline is unchanged and a different index is
  refused while the first result awaits delivery.

## Review follow-up on `26d3458ba`

- The Client-only Workspace regression exercises cold load and explicit
  Workspace reconciliation with no directory in the Server view during
  either operation. Authored Workspace resource selection has an explicit
  diagnostic. Server-side discovery remains within #578's existing scope.
- The binding tracks physical settlement from pair commit through
  transport loss. The externally launched Client test cuts its control
  lane after a confirmed healthy exchange. Host drain reports unconfirmed
  settlement with no file observers. The Revoke regression holds a delivered
  command beyond the idle frame budget and releases its valid receipt
  within the command deadline. The preceding held-receipt failure case
  still checks deadline expiry.
- WorkspaceChanges protects its first Summary and outstanding Comparison
  across capacity reclamation. Matching polls claim the result. Retention
  under pressure expires 60 seconds after the result is produced.
- The START race holds Server dispatch after its cancellation check,
  confirms Client cancellation for the registered key, then lets START
  return the real zero-output terminal snapshot. The product test checks
  no business marker, one acknowledged release, and subsequent command and
  file read on the same binding. Separate regressions cover authoritative
  Register refusal without a tombstone, terminal data arriving during an
  unresolved cancel, PTY read loss, and unsent Bash capture disposal.
- The Python scratch permission test sets a permissive umask in a
  subprocess, executes the installed guard, and checks both directory modes
  and final removal. The positional argv proof counts one argument with
  an empty value. A trusted TLS connection reaches and rejects malformed
  WebSocket upgrade input. macOS native validation remains deferred.

## Review follow-up on `95b8fbbf4`

- Bound configuration takes the Workspace path from the reservation. The
  Client reports root identity and document contents. Capacity and wire
  I/O share one deadline, and publication checks the source and binding
  identities after the read. Only absence on the initial range can clear
  a Workspace document. The generation and data-contention regressions
  use explicit gates in the scripted configuration harness.
- Physical mutation owners validate delegated commit receipts. Child
  retirement waits for shared data capacity within its cleanup deadline.
  Nested writes sync all containing directories before publication.
- Workspace Changes retains at most 512 MiB or 65,536 unique loose Git
  objects per binding, whichever limit is reached first. The private
  index and objects are staged together. This caps retained objects.
  One in-flight Git snapshot still has temporary scratch use. Completed
  observations no longer deny new observation admission indefinitely.
- Direct loopback WebSocket reservations provision a binding-specific
  TLS certificate. The operator-ingress mode continues to require a
  trusted local forwarding hop. Idle remote reservations shut down
  without a Client. Native Watcher visibility and Unix peer ownership
  have macOS code fixes. Native macOS compilation and tests were
  intentionally deferred by the maintainer.
- Workspace configuration documents, file watches, and execution
  operations use the Client. The authored resource catalog and the
  automatic config-source watcher still use Server-owned sources.
  Host-managed workspaces, resource discovery and upload reception
  remain [#578](https://github.com/Caismis/rustX/issues/578) work.
- The second review adds regression coverage for cancelled prepare
  competing for the writer, mutation cancellation waiting behind four
  watch-stop messages, stale data-lane EOF at pair commit, and a withheld
  Client revocation outside the Host lock. Real process cases exercise
  empty argv values and defer pipe stdin acknowledgement until physical
  EOF.
- Local Linux follow-up ran the five execution-related member packages
  with staged companions. The run passed 440 tests with 3 existing
  ignores. The focused Server harness passed both configuration
  cases, the cancelled-prepare writer race, and the stalled-revoke
  drain case. Five real App Server process cases passed, including
  the bootstrap and reap path, TLS and empty argv, and the canonical
  MCP overflow fixture. The saturation test uses real watch-stop
  control messages and a scripted control peer. The separate
  two-process Client gate proves zero publication after confirmed
  cancellation.
- On the [reviewed Linux FULL run](https://github.com/Caismis/rustX/actions/runs/44783497166),
  Merge verification passed. The plan intentionally skipped the FAST
  `rust-fast` and `documentation-contracts` jobs. FULL selected the
  corresponding `rust-contracts`, `rust-boundaries`, and legacy suites.
  Those results apply to the reviewed revision.

## Phase-one gates

`bootstrap` = `rustx-execution-client --test bootstrap`, selected by rust-fast and rust-boundaries. `member unit` = package-qualified libtest, selected by the Linux FAST unit step and FULL contracts. Listed lane ownership does not certify a green run on a new revision.

The merge of `origin/main` (50d0e5728) removed the macOS CI lanes (`rust-platform-core`, `rust-platform-boundaries`) wholesale: upstream retired its macOS runners, so this branch's Execution-Client coverage there went with them rather than being ported. CI proof is therefore Linux-only today; native macOS execution of the `bootstrap`/`bins` suites is pending an independent native run and is not claimed here.

| ID | Required PR1 proof | Current implementer / test | Lane and evidence at initial audit |
| --- | --- | --- | --- |
| T01 | Server-only work without any execution dependency | `standalone_server_reads_cold_sessions_without_workspace_or_helpers`, `standalone_server_commits_a_model_turn_without_a_client_or_helpers` | Linux process tests passed. The isolated Server lists two cold Sessions with missing Workspaces. A separate isolated launch submits exactly one loopback model request, settles its canonical answer and leaves the Workspace empty. T31 separately exercises overflow without Client storage |
| T02 | Separate filesystem views for read/write/Bash through product | `explicit_composition_reads_from_client_mount_namespace`, `filesystem_operations_use_the_private_mount_view`, `background_live_output_is_readable_while_running_and_reused_at_settlement` | Linux mount proofs cover the App Server query/Write/argv path and native Agent Read/Write/Bash/Grep. The writable overlay exercises Client directory creation, file copy, recursive removal, watch delivery, Git baseline, first-mutation capture and comparison release while the Server sees the original empty mount point. Separate macOS gate applies |
| T03 | Exactly one Session dispatch owner | `concurrent_claims_publish_one_endpoint_that_every_later_claim_shares`, `concurrent_hellos_commit_exactly_one_live_connection`, `a_dropped_caller_never_strands_the_claim` | The barrier-controlled Session competitors publish one shared endpoint, and the competing Hellos produce one live connection. The endpoint dispatch and revocation tests prove the stored Client is the sole serialized dispatch owner |
| T04 | Root, child and Workflow route to exact Session | `SessionExecutionScope`, delegated child IPC, `delegated_write_preserves_the_commit_frame_and_unknown_cut`, `shipped_repair_*` | Root and native Workflow use the exact Session route. A real Subagent relays files and processes through its parent and shares that authenticated binding. The three shipped Workflow repair cases subsequently passed with the provider emulator in the relay slice. EXEC-03 adds `a_real_child_executes_native_tools_in_the_frozen_nested_workspace`. Distinct Root and Workflow reads and the real child share one private mount and one authenticated Session. The fake provider records ten requests across the Root, child and handoff |
| T05 | Cancel before dispatch/model/START causes zero corresponding side effects | `cancellation_before_model_turn_start_never_starts_the_request`, `revocation_before_native_start_never_executes_an_independent_command`, `client_applied_control_wins_before_file_publication`, `agent_bash_has_separate_registration_and_a_client_owned_start_gate` | The deterministic model test asserts zero requests, Request Snapshots and request-start facts, and one cancelled terminal. The App Server and Client-held gates prove zero process markers and zero published files at the respective crossings. Child-specific combined evidence remains open |
| T06 | Parallel results commit in call order, unique terminal is last | `client_bash_reversed_settlement_keeps_canonical_order`, `shipped_repair_*` | Two parallel native Bash operations on one real Client finish in reverse physical order. The test proves one START per operation, two model requests, tool results in call order, and one final terminal. The shipped Workflow fixtures cover separately admitted child writers followed by physical check commands |
| T11 | Real control/cancel/bidirectional health progress with saturated data | `detach_during_data_and_control_backpressure`; split Unix/WebSocket send blockage, pairing and data-permit tests | Linux integration passed. The real Execution Client answers an actual UI command. The proxy parks a Client metadata reply and the first health receipt. Four control slots stay occupied and the fifth probe returns `WouldBlock`. After real UI WebSocket EOF, the keyed cancellation progresses while the data reply remains parked. The business socket closes before the data gate opens. All four health probes receive their own receipts. Agent-flow saturation across other roles and the future recovery cases remain open |
| T18 | Authenticate the binding before Workspace configuration | `execution_enrollment_precedes_client_and_workspace_configuration_readiness`, production launcher enrollment | Passed on Linux for trusted remote enrollment. The durable Session is inspectable before Client startup. Unbound browsing fails, the authenticated Client browses its Workspace, and malformed Workspace configuration refuses Agent attach without disabling history or management |
| T24 | Independently built and launched Server and Client, isolated credentials/dependencies | T01 standalone Server, Client-only build and paired admission, `explicit_composition_launches_and_reaps_the_sibling_client`, both normal Cargo dependency graphs | Linux Server-only and Client-only builds passed. Only the staged Server executable is present in the cold catalog and model tests. The admitted sibling uses sealed protocol stdio and a trusted runtime-discovery allowlist of HOME, PATH and XDG_DATA_HOME. The Client's production graph excludes the Agent Server, model adapters and canonical storage. The Server's Client crate dependency is test-only. Native macOS and installed-package proof remain open |
| T26 | Client revocation wins before file commit or START | `client_applied_control_wins_before_file_publication`, `a_revoked_binding_after_prepare_sends_no_commit_frame`, `agent_bash_has_separate_registration_and_a_client_owned_start_gate`, Client mutation gate tests | The real paired Client proves zero effects before file publication for cancel and binding revoke. Both cases send exactly one prepare and one commit frame, return typed `CommitCancelled`, and leave the Workspace empty. Real Bash cancellation before START produces no business marker. Mid-crossing disposition is tested at the Client mutation gate |
| T28 | Reject wrong Session, credential, instance, generation and duplicate | `wrong_hello_fields_never_consume_a_reservation`, `credential_replay_after_eof_never_reconnects`, `two_sessions_sharing_one_workspace_hold_distinct_binding_material` | The negative Hello matrix leaves the reservation untouched. A second live claim is refused. The real Client cannot replay a credential after EOF. An unbound Session returns `ExecutionUnavailable` for queries and mutations even when another Session uses the same path. The EXEC-02 ledger records separate same-instance recovery and independent Linux replacement evidence |
| T29 | UI command ownership distinct from accepted Agent execution | `detach_during_data_and_control_backpressure`, `detached_operations_release_without_touching_siblings`, PTY and drain suite | Combined two-process Linux test passed. It asserts one register, one START, a correlated `CancellationRequested`, a physical `Cancelled` snapshot, the original five ordered stdout bytes, zero stderr, exact release cursor, one `OutputReleased` and Client exit. A distinct UI operation survives detach in the sibling scenario. Transport recovery has separate evidence in EXEC-02 |
| T31 | No Client: truthful truncation, zero Server spill/start/wait | `ToolOutputCapture::PreviewOnly` in `tools::output`, `ManagedOutputContinuation::Unavailable` in `tools::mcp::result`, `tools::mcp::result::tests`, `mcp::unix_tests::foreground_mcp_result_normalization_is_collective_and_complete`, `standalone_server_reports_mcp_overflow_without_launching_a_client_or_spilling` | Foreground MCP overflow now settles a typed `Unavailable` with accurate `original_bytes` and no locator — `foreground_mcp_overflow_never_attempts_storage_and_is_unavailable` and `foreground_mcp_overflow_never_writes_storage` prove zero save attempts against forced-open/write failure fixtures, the `assert_unavailable_no_save` helper asserts zero `r-`/`e-` files and no Read/Grep promise, and the real rmcp stdio boundary test verifies the same through a staged peer. A remote `is_error` keeps `Failed` at overflow (`foreground_mcp_overflow_keeps_a_reported_remote_error`); multi-fragment crossings count once (`foreground_mcp_text_image_text_overflow_preserves_order`). Structured JSON renders through a bounded streaming writer, and text blocks retain only bounded head/tail previews — no speculative complete-output retention. The standalone App Server over stdio runs one real loopback HTTP MCP call and two loopback model requests. The canonical tool result succeeds with the exact original byte count and `Unavailable`, then one completed terminal. A launch-counting sibling and private temporary directory prove zero Client starts and zero Spill files. Optional Client saving and the broader HTTP/Host integration follow in #577/#578. Session-bound native Bash stores its continuation on the Client |
| T32 | Complete normalized import including displayed prefix | `protocol::save` `SaveSelection` + `SaveCapture` reference, decode boundary `decode_chunk`, and `save-envelopes.json` | The contract fixtures and the EXEC-03 real Host receiver verify complete normalized output, the original prefix, a middle match, exact final bytes and ordinary Native Read/Grep |
| T33 | Save faults cannot replay won business execution | `protocol::save` state machine, fault-injected store fixtures and `expired_chunk_seal_and_publish_are_deadline_gated_before_any_state_change` | Contract deadlines and cleanup, plus the EXEC-03 real ENOSPC, interrupted saving and post-Seal cancellation cases. The known business result remains canonical without retry |
| T34 | One selection at threshold; absence or generation change never retargets | `protocol::save::SaveSelection::observe` (exact threshold, one lookup at crossing, `Absent` and replacement owners never rebind, deadline min(now+5s, call) tagged once) | The Client Save driver and Host receiver now use the frozen selection. A real replacement refuses the old candidate even when the new operation bytes match. EXEC-03 tests a real lost publication receipt before canonical commit and a late receipt after binding replacement |
| T35 | Publication acknowledgement versus one canonical tool commit | `protocol::save::SaveCapture` defines the publication receipt and candidate identity independently of the business result | EXEC-03 exercises real physical publication with a lost or late receipt, one canonical result, and no inferred locator or business replay |
| T36 | Small result/error/validated End share claim gate, Begin never claims | `app_server::host_upload` `HostUploadSlot`/`ValidatedEnd` (opaque, End-only) and `app_server::callbacks::tests::only_a_verified_end_can_compete_with_an_ordinary_reply` racing a small success or valid JSON-RPC error through the real `Callbacks::respond` in both orders, with subscription recheck at claim | The production EXEC-03 receiver exercises verified End with an earlier blocked candidate, competing ordinary replies, and a stale subscription |
| T37 | Independent budgets and business/capture/readability facts | `app_server::host_upload` verifier, `host_upload_framing_counts_the_encoded_envelope`, `protocol::save` | The verifier enforces independent item, chunk, result and candidate limits. The actual JSONL reader checks the complete escaped envelope before parsing. Optional Save and Host receiver behavior are separately specified. The receiver and production Host import belong to #578 |
| T39 | Spill disposition across fork, clone, restart and deletion | Conversation-scoped native Bash locator and frozen Save candidate identity | Native continuations stay with the owning Conversation and live binding. Cross-instance cleanup, restart and durable retrieval are follow-up #577 work |
| T41 | Seeded local/remote semantic equivalence | `websocket_and_unix_transports_serve_identical_seeded_operations`, `remote_execution_serves_session_reads_over_websocket`; execution-client `websocket` suite (9 tests); `rustx-execution-transport` suite (14 tests) | Partial; real two-process App Server + operator-spawned staged Client over genuine RFC 6455 upgrade — `fs/readFile`/`getMetadata`/`writeFile` cross the WS channel with byte- and revision-identical semantics, second-Session `ExecutionUnavailable` parity, `0600` `create_new` bundle proof, and clean Client exit on channel EOF. Member-level WS proofs: credential/session/version/generation tamper stays `Reserved`, plaintext non-loopback + wrong path never connect, malformed upgrade never consumes the reservation, slow trickling peer shares one absolute deadline, revocation wakes an idle WS read, EOF never allows replacement, `AlreadyConnected` on a second live claim. Transport level: one `EncodedRecord` per binary message, text/oversize/malformed rejected pre-deserialize, wrong path/`Origin` refused, deadline-bounded silent peer, Unix+WS shutdown wakeups, real `wss` with fixture CA + IP SAN (positive), wrong-CA/wrong-SAN negatives proving no record before TLS validation. Direct loopback `wss` authenticates a reservation-specific TLS certificate. Operator ingress forwards to a trusted loopback-only listener; T11 has a separate product-level proof with a WebSocket UI and local execution transport. Equivalent saturation over remote execution WebSocket remains part of the broader transport matrix |
| T42 | Bounded line/match projection: transferred bytes, client reads, memory, RPCs | `read_range_serves_verified_bytes_at_a_sparse_file_boundary`, `a_fully_consumed_empty_file_is_a_valid_read`, `a_size_contradiction_retires_the_stream_and_detaches_the_binding`, `read_lines_pages_a_large_file_in_single_bounded_exchanges`, `read_lines_reports_end_reasons_and_keeps_stable_semantics`, `read_lines_stale_revision_is_a_conflict_not_a_read`, `read_lines_refuses_non_utf8_and_keeps_the_connection_live`, `read_document_decodes_every_whitelisted_format`, `read_document_serves_deterministic_pages_and_stale_revisions_conflict`, `read_document_refuses_unsupported_corrupt_oversized_and_textless`, confined-fs `text` scanner unit matrix, document-reader unit matrix, `search_serves_the_shared_universe_and_never_follows_links_outside`, execution-client `search::tests` matrix, `a_search_page_is_verified_and_a_dishonest_one_retires`, `unbound_sessions_refuse_search_without_server_traversal`, tool-plane glob/grep suite | Partial; bootstrap proves byte range plus verified `FileStamp` source descriptor, directory-page name/cursor discipline, and one bounded `ReadLines`/`Lines` typed exchange per page over the real Client — early and late pages of a ~1 MiB file each return in one RPC with whole-source `total_lines`/stamp (`read_lines_pages_a_large_file_in_single_bounded_exchanges`); the shared `rustx_confined_fs::scan_text` scanner the page and the native Read tool both run on is covered by an independent edge-case matrix (empty, trailing newline, CRLF/BOM, limits, offset-past-end, UTF-8 failure in the skipped tail, code points split across read blocks, giant first line, exact byte/line caps with terminal newline) and `ReadDocument`/`DocumentPage` adds the same bounded page over the decoded Markdown of the committed fixture corpus — all four whitelisted formats decode through the real Client to deterministic, source-stamped pages with `logical_bytes` independent of the compressed source, stale revisions conflict, and the unsupported-extension/corrupt/over-cap/no-text-layer refusals are distinct typed codes that never retire the connection; the native Agent Read tool now dispatches both page kinds through the Session's admitted Client via `SessionExecutionScope` — `tools::native::read` runs its whole matrix against a real `SessionRuntimeManager`-reserved, staged-Client route, `agent_read_dispatches_only_through_the_bound_session_scope` proves one bound Session succeeds while a second Session on the same lexical path answers unavailable with no Client prepared, and an unbound route is an honest `Session execution unavailable` with no Server-local filesystem branch; native `read_image` now routes its physical bytes through the same scope as bounded `ReadRange` chunks — `a_multichunk_png_commits_exact_artifact_bytes` fetches a ~148 KiB PNG across multiple wire chunks through the real Client and commits byte-exact immutable artifacts that outlive the source, `an_unbound_session_refuses_image_reads_without_touching_artifacts` allocates nothing and leaks nothing, scripted-peer endpoint tests prove an over-bound stamp stops after one `ReadRange` (`an_oversized_image_source_reads_one_range_and_stays_live`), a mid-fetch replacement is an honest live `conflict` (`a_replacement_between_image_chunks_is_an_honest_conflict`), and a same-revision stamp change retires (`a_stamped_mismatch_between_image_chunks_retires_the_connection`); the `with_active`-gated artifact commit keeps cancellation deterministic on both sides of the durable store (`image_cancellation_on_each_side_of_artifact_commit_is_deterministic`); native `glob`/`grep` now dispatch one bounded `Search` exchange through the same scope — `crates/execution-client/src/search.rs` runs the shared descriptor-relative universe (hidden visible, ignore files not applied, links never followed, symlinked roots refused, lexical order) under the frozen traversal budgets, `search_serves_the_shared_universe_and_never_follows_links_outside` proves two links to outside never crossed and an invalid-UTF8 tail excludes a whole file over the real Client socket, the `search::tests` matrix covers file roots, `SearchNeedsDirectory`, budget refusals (an 8 MiB single line fails as `SearchBudgetExceeded`, never OOM) and the hit-limit proof, `a_search_page_is_verified_and_a_dishonest_one_retires` proves the Server retires an over-count page, and the tool-plane glob/grep suite runs its full contract against the real bound route plus an honest unbound refusal; the conditional edit now runs the real Client end to end — `a_conditional_edit_commits_bom_crlf_and_multiple_edits` proves BOM/CRLF preservation and a fuzzy anchor through `PrepareEdit`/`CommitWrite` on the real socket, `edit_refusals_are_typed_and_the_connection_stays_live` proves typed ambiguity/no-change/oversize refusals stage nothing and keep the connection live, `a_denied_edit_gate_aborts_and_a_replaced_target_conflicts` proves a gate denial aborts the private stage and leaf/ancestor swaps are pre-commit denials, `a_malformed_edit_request_is_an_error_not_a_stage` proves a malformed wire request is a stable `Error` with no stage, and the endpoint tests prove `EditPrepared` validation (zero ticket, wrong revision, over-cap `published_size`), contradicting refusals, and `CommitUnknown` retirement; the native Agent Write/Edit tools now route their physical mutations through the same admitted-Client boundary — preflight binds a ticketed `PreparedMutation` under the logical interpreted path (`WriteExpectation::Current`), permission classification compares `PLAN.md` against the logical workspace path the Client proved with no Server-local canonicalize/stat probe, `run_write`/`run_edit` consume exactly one commit-or-abort decision on the live ticket, `OutcomeUnknown` from the publish crossing is never overwritten by a racing cancellation, denied/cancelled preflights abort the ticket explicitly while a dropped owner arranges bounded cleanup, the whole `tool_plane` write/edit matrix runs against a real staged Client (`unbound_sessions_refuse_mutations_without_server_side_write` proves no Server-local fallback — no file created, original untouched), `write_creates_missing_parents_through_the_client_staged_subtree` proves the staged-subtree publish, `parallel_prepared_mutations_commit_and_abort_independently` proves two tickets on one connection decide independently, `a_write_over_the_chunk_limit_prepares_and_commits` proves the >64 KiB `BeginWrite`/`WriteChunk` transfer, and the managed-output suite proves containment failures mutate nothing; the ticketed map (≤8 preparations, one shared in-memory payload budget) holds preapproval bytes in Client memory so nothing is staged or enumerable before the grant; the real Client now supplies both WorkspaceChanges baseline and matching Write/Edit capture (see the observation proof below); the release measurement below includes a 64 MiB source, wire bytes and RPCs, both process high-water memory and observed queue depth. Internal scheduler queue high-water marks and multi-chunk write progress parity remain open |
| T44 | Minimal reservation before browsing, no unbound access or unrelated startup | `execution_enrollment_precedes_client_and_workspace_configuration_readiness`, `an_unbound_workspace_argument_never_connects` | The production bootstrap path passes on Linux. Cold management stays available during invalid Workspace configuration. The complete product picker and native macOS evidence remain open |
| T54 | Candidate/sequence/offset integrity, late and mixed identities rejected | `app_server::host_upload` verifier and slot refusals (gap, out-of-order, conflict, foreign candidate on every step, late, duplicate-outside-window) tested; candidate identity requires connection and subscription incarnations | The EXEC-03 receiver exercises live Host subscription identity, a valid competing End, stale replies and a real Client Save under data saturation |
| T55 | Final encoded size including envelope and escaping | `host_upload_framing_counts_the_encoded_envelope` and generated Rust, JSON and TypeScript fixtures | Exact one-MiB JSONL with the full envelope, base64 and escaped text passes framing. A parseable frame one byte larger is refused at the reader. EXEC-03 exercises the production receiver with a multi-frame stream above one MiB |

## Native Agent Bash evidence

The Agent production registration sends a distinct shell registration to the
Session's Execution Client. The client registers the operation and, for an
explicit background call or a promotion candidate, creates its live capture
before the Server may commit background ownership. Server admission, the
Client's START grant, and canonical results remain separate facts. Shell
registration requires the Agent Bash capability. UI `command/exec` keeps
its argv contract.

The normal Client owns stdout/stderr decoding, preview capture, the bounded
continuation file, and physical cancellation and settlement. Read/Grep and
job-output tail queries use the exact Session binding and Conversation owner.
The Bash registration no longer installs a Server-local process or capture
executor. The explicit supervisor diagnostic fixtures remain available to
regression tests.

- **T02 / T42.** `background_live_output_is_readable_while_running_and_reused_at_settlement`
  runs the native Write/Read/Bash/Grep route against the real Client. Set
  `RUSTX_MOUNT_VIEW_PROOF=1` through `test-support/prove_execution_mount_view.sh`
  to use the Linux private mount view. The FIFO holds the business process
  while Read and Grep inspect its committed prefix. The final locator
  returns the complete ordered output. A foreign Conversation cannot open it.
- **T05 / T26.** `agent_bash_has_separate_registration_and_a_client_owned_start_gate`
  asserts the wrong registration kind is refused, a conflicting identity
  cannot mutate the spec, the held START cancellation produces zero bytes
  and zero business marker, and release/discard remove the captured output
  without authorizing a retry.
- **T06.** `client_bash_reversed_settlement_keeps_canonical_order` gates the
  first real Client process on the second's confirmed settlement. It
  checks one START marker per operation, two scripted model requests,
  original call order in the transcript, and one final attempt terminal.
- **T55.** `host_upload_framing_counts_the_encoded_envelope` feeds the real
  JSONL record reader an upload envelope with base64, Unicode, JSON-escaped
  controls and exact one-mebibyte framing. An otherwise parseable record
  one byte over the cap is rejected by the record reader. The future
  Host receiver remains assigned to #578.

Local Linux evidence from this slice includes 43 Client unit tests, 40
bootstrap tests and a separate successful run of the opt-in range mount
proof. The 29 native Bash integration tests pass. The isolated native tool
fixture, Root settlement-order fixture, JobWait unload/replacement proof
and Host framing test each pass. The initial isolation attempt exposed a
test-fixture bug. Bubblewrap's parent-death signal fired when the setup
thread exited. The fixture now retains that thread through Client cleanup.
Only the run with the repaired fixture is counted as Agent mount evidence.

The first full Linux workspace test run at `100098eac` passed 6,279
root tests, failed 19, and ignored 28; Cargo stopped at the failed root
harness. Three background-output and sixteen crash/recovery conformance
rows were still composing with no admitted Execution Client or checking a
Server spill pathname. In the follow-up, those rows run through a real
Client. The recovery rows reuse the exact catalog Session for lineage
cuts, and the live output test queries bounded Client lines while the
command is waiting on a FIFO. The focused combined suite passes 73
tests with no failures or ignores. A release workspace binary build also
passed on `100098eac`. The follow-up still requires the complete workspace
test and remote verification lanes.

On `2739eb617`, the full Linux rerun passed all 6,298 root tests,
then stopped after 12 public conformance failures (41 passed and 3
ignored in that harness). The later execution launcher and drain changes
passed the eight targeted process tests covering the prior seven Linux
FAST failures and the new enrollment path. The two-process file commit
test also passed both control decisions. The complete conformance suite
has to be rerun on that code before a full result can be recorded.
The next targeted run exposed that a large accepted image batch could
exhaust all eight Client data slots before the remaining images are sent.
Image reads now wait for a slot under the existing absolute deadline and
cancellation, before allocating file bytes or sending a request. Transport
capacity and four-range verification remain unchanged. All six affected
provider conformance cases pass, including the 17-image batch and the two
provider request count it already asserts. The three Workflow repair cases
now use the separate Subagent process’s Session relay described below.

The watcher migration makes `fs/watch` require the exact `sessionId`.
The Client retains the physical watch descriptors and coalesces up to
16 paths within a 16 KiB encoded-path budget. The Server owns the UI
watch subscription and its publication fence. Unwatch settles on the
independent control lane. Linux evidence includes the two Client
descriptor-replacement tests, the private-mount watch with a Client-only
sentinel, and all 28 existing App Server environment boundary tests on
the watcher slice. Native macOS descriptor and teardown coverage still
requires a macOS runner.

## Session relay and Workspace Python

Root and native Workflow dispatch share one live Session binding. The staged
Subagent receives the same Client identity over startup IPC. Its parent
forwards only the frozen native capabilities and checks the owning
Conversation and mutation ticket or process key. The scoped relay tests
cover a checked conditional write commit and an honest unknown commit cut.
The three shipped repair Workflows passed with physical child writes and
root checks. The Linux mount run of the separate real-process Subagent
fixture observed the Write, Edit, Read and Bash results in exact order. The
Server's ordinary Workspace path contained neither the child's file nor
the Python probe marker. Session drain reaped the live Client Python probe.
The seeded tree sequence passed over UDS and WebSocket in the same process
test, including byte-exact binary copy and a typed path refusal.

The independent Client owns Workspace Python preparation, guard and
interpreter selection. Local checks pass 66 Client library tests, 42
bootstrap tests, 9 WebSocket tests and strict Client Clippy. The explicit
Linux mount test verifies the actual interpreter under the Client-only
`.venv` and no matching Server-side path. The replacement regression
asserts zero probes, records or `.venv` after the bound root changes. Native
macOS verification remains open. Managed Python is owned by #577.

The detached-stdio process test controls two independent real Clients with
separate Unix stream gates. One completes exactly one Workspace Python
preparation after stdio EOF. The other is cancelled by Server SIGTERM;
its uv process closes the stream and exits without creating a `.venv`.
The six Workflow Code regressions pass with a valid venv and an explicit
Client readiness receipt, including language selection, large typed results,
retry, and error branches. Three crash recovery regressions pass with the
real Client bound to the same durable Session used by the recovered turn.

Trusted enrollment and authorized inspection do not run Workspace
configuration, discover Skills or wait for MCP or uv. Unavailable or invalid
Workspace configuration remains an attach failure. Fresh App Server loads
and explicit reconciliation of a reserved Session read the authenticated
Client's `rustx.toml`, then parse, validate and publish on the Server. The
source capture proves the Workspace root and rechecks the stamped document.
An unconnected reservation never uses the same Server-side path as a source.
The path-keyed authoring and Session-creation caches remain separate from
bound Session captures. Workspace resource discovery and native automatic
source watching still consult the Server filesystem, including guidance,
roles, Workflows, Skills and MCP/Managed Python catalogs. Server-invisible
Workspace resource trees and general authoring still need a Client-backed
source.

The real WebSocket enrollment regression proves browsing and Agent attach
both refuse before the Client is admitted, even with a valid document in
the Server filesystem. With the Client admitted, browsing succeeds and a
malformed document still prevents attach. The Linux mount proof observes
the Client-only 34,567 ms hard deadline on cold attach and the revised
45,678 ms value after explicit reconciliation. The invalid Server decoy
stays byte-for-byte unchanged.

## WorkspaceChanges physical observation

The Server's `ChangeRecording` associates a canonical Attempt with a fresh
operation ID on the live Session binding. It waits for the acknowledged
baseline before Tool dispatch and sends a capture request ahead of a bound
Write/Edit commit. The Client owns Git and descriptor-relative captures in
its physical mount view. Its binding reuses a private Git index. Each
attempt's fallback capture directory is released with its retained record.
The Server publishes its change card only while the Attempt remains active.
A missing observation receipt produces an unavailable card. Canonical tool
settlement remains independent.

The Linux private-mount test uses a Git repository and an ignored file visible
only to a real Execution Client. It checks two changed paths, both exact
before/after comparisons, release, and absence from the Server's view. The
scripted Agent Write/Edit test checks the real Client's capture of the old
and new text and the unchanged four model-facing messages. Two runtime tests
check the single terminal and handoff when cancellation wins before and
during final observation publication. The Client member suite retains the
Git, sparse checkout, raw path, cancellation and filesystem tests from the
previous physical owner. A nested Workspace also checks that a rename's
out-of-scope endpoint cannot become a comparison source. The Linux Client
run passed 128 member tests and 42 bootstrap tests, with one deliberate
opt-in test in each harness. The separate mount run and all three Server
observation tests passed. Workspace all-target/all-feature check and strict
Clippy on the execution members passed.

## UI detach under data and control pressure

The Linux `detach_during_data_and_control_backpressure` boundary test uses
the real App Server WebSocket listener and staged Execution Client. It
parks one actual data-lane reply and every control slot through a health
receipt barrier. After the UI carrier is reaped, the Client receives the
exact keyed cancel. The business process's own TCP descriptor closes
while data remains blocked. The driver then receives the physical
snapshot and releases that exact final journal cursor after data resumes.
The test requires a successful Client exit after host drain. The proxy
can observe terminal pipe closure during cleanup.

## Session-bound tree mutations

`fs/createDirectory`, `fs/remove` and `fs/copy` now require the same
`sessionId` as reads and writes. The Server resolves paths lexically and
checks the environment policy and open UI connection before sending.
The Client holds the authenticated generation grant at each descriptor-relative
physical crossing. Copies stream through the existing confined-fs publication
kernel. A clean pre-crossing failure retains its typed code. Partial changes
or a lost receipt become `OutcomeUnknown` and retire the pair. The Server's
directory creation, removal and copy implementations were removed. The Client also now owns
the native watcher and the Server owns its subscription projection.

The real Client bootstrap suite includes
`a_partial_tree_copy_is_reported_unknown_and_retires_the_pair`. It proves
one published file survives a later refusal to follow a symbolic link.
The opt-in `filesystem_operations_use_the_private_mount_view` exercise also
proves a refused pre-wire gate never creates the first missing directory and
that unlinking a symbolic link preserves its target. All four migrated
confined-write fault tests pass on the Client, including generation revocation
during staging and post-publication sync failure.

## Measurement record

### Loaded App Server and Client

The Linux x86_64 release build at `fed2f8c42` ran one Session through the
production App Server stdio and paired loopback Execution WebSocket routes.
A forwarding peer counted complete serialized records inside WebSocket
payloads, excluding the WebSocket header, handshake and UI JSONL. The
Session was created with a loopback provider URL that could never reach
a model, and the workload issued zero model requests. The measured
binary SHA-256 values were Server
`1cddc6eba9da8dce61d24de5b19e90d2811469414ffee91284d149de74b38d17` and Client
`c5d49ab2118a4a2307b9cd9179df306fbe3e460791abec9a1fece7003d5582a2`.

The loaded Server and Client reached 55,232 KiB and 9,784 KiB peak RSS
respectively. A Server-only initialization on the same profile reached
41,624 KiB. Paired Server initialization took 20.01 ms and the separately
launched Client reached authenticated admission in 5.84 ms. The Server-only
sample took 455.75 ms during other local build activity. These are single
startup observations with different cache and system load conditions.

| Load | Repetitions | Median completion | Workload throughput | Data RPCs, total | Server to Client bytes | Client to Server bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Sequential create-only writes, 64 KiB each | 20 | 7.52 ms | 7.56 MiB/s | 40 | 1,750,031 | 4,822 |
| Sequential reads, 64 KiB each | 20 | 1.49 ms | 41.66 MiB/s | 20 | 1,980 | 1,752,180 |
| UI argv command, emit 1 MiB, retain 64 KiB | 5 | 68.53 ms | 14.58 MiB/s | 49 | 14,965 | 451,352 |
| UI argv command, emit 8 MiB, retain 64 KiB | 5 | 68.54 ms | 116.16 MiB/s | 50 | 15,230 | 451,625 |

All reads matched their written bytes. All commands reported exit 0 and
retained the requested 64 KiB with the truncation bit set. The corresponding
control heartbeat totals were 20, 0, 3 and 4 round trips, with 1,258, 0,
190 and 252 payload bytes per direction. The peak wire-visible data
in-flight count was one. No non-heartbeat control request was in flight
during the measured operations. A second run on the same release pair
verified all ten `ReleaseOutput.final_sequence` cursors at 65,536 bytes.
Journal pages leave retained segments in place until release, so the
operation's Client journal reached 64 KiB in each command. The probes
measured UI argv completion, including polling and wire work. The
scheduler's internal queue high-water marks remain unmeasured. The following native Bash
sample measures a different capture configuration and cannot use this
workload as a speed baseline.

The same Linux release workload was repeated at
`734b943566f664023717017de59f87a6c0a67276`. Loaded peak RSS was 47,312 KiB
on the Server and 9,900 KiB on the Client. Server-only peak RSS was
34,552 KiB. Paired Server initialization took 203.25 ms and Client
admission took 12.06 ms. Server-only initialization took 1,591.26 ms.
This run overlapped other builds and is unsuitable for startup or
synchronous write performance comparisons.

| Load | Median completion | Throughput | Data RPCs, total | Server to Client bytes | Client to Server bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| 20 sequential create-only writes, 64 KiB each | 150.96 ms | 0.23 MiB/s | 40 | 1,750,031 | 4,822 |
| 20 sequential reads, 64 KiB each | 1.14 ms | 42.61 MiB/s | 20 | 1,980 | 1,752,180 |
| 5 UI argv commands, emit 1 MiB, retain 64 KiB | 68.45 ms | 14.64 MiB/s | 50 | 15,220 | 451,625 |
| 5 UI argv commands, emit 8 MiB, retain 64 KiB | 68.00 ms | 117.26 MiB/s | 50 | 15,230 | 451,625 |

Peak wire-visible data in flight was one record. All ten command output
journals reached 65,536 bytes and released at that exact cursor. In the
separate pressure fixture, all four control slots were occupied and the
fifth probe returned `WouldBlock`. The held-writer regression
`the_control_lane_progresses_while_every_data_permit_is_parked` also proves
all eight data permits were occupied during control traffic. Internal
scheduler high-water measurements for the production load remain open.

The paired UDS measuring peer ran 18 separate Client probes against the same
release executable. For the 67,108,877-byte source, line selection took one
RPC with 99 bytes sent and 265 bytes returned, a 56.27 ms median, and a
Client peak of 8,528 KiB. Grep took one RPC with 149 bytes sent and 132 bytes
returned, a 21.34 ms median, and a Client peak of 9,392 KiB. The probe peer
reached 18,764 KiB and 18,808 KiB respectively. It is a measuring peer with
different runtime dependencies from the product Server.

The same Linux release workload was repeated at
`ec9342c49692893eca14bba7c1b9d3ec32fae10f`. Loaded peak RSS was 46,696
KiB on the Server and 9,352 KiB on the Client. The Server-only process
reached 33,596 KiB. Server-only initialization, paired Server
initialization and Client admission took 16.74, 20.34 and 30.32 ms.

| Load | Median completion | Throughput | Data RPCs, total | Server to Client bytes | Client to Server bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| 20 sequential create-only writes, 64 KiB each | 4.75 ms | 11.17 MiB/s | 40 | 1,750,031 | 4,822 |
| 20 sequential reads, 64 KiB each | 1.69 ms | 35.27 MiB/s | 20 | 1,980 | 1,752,180 |
| 5 UI argv commands, emit 1 MiB, retain 64 KiB | 68.58 ms | 14.51 MiB/s | 50 | 15,220 | 451,625 |
| 5 UI argv commands, emit 8 MiB, retain 64 KiB | 69.07 ms | 97.63 MiB/s | 55 | 16,620 | 452,965 |

The journal again released at exactly 65,536 bytes for every command. The
wire data high-water mark was one in-flight record. The independent
67,108,877-byte probe again used one data RPC per query. Line selection
transferred 99/265 bytes, with a 56.76 ms median and 8,456 KiB peak Client
RSS. Grep transferred 149/132 bytes, with a 21.51 ms median and 9,704 KiB
peak Client RSS. Each probe ran three times. These are descriptive runs
under different host load and do not estimate a causal performance delta.

The release measurement at `910646ca5f859bc9834cbcd8ea5bf37b0c48fdde`
recorded 46,336 KiB Server and 9,476 KiB Client peak RSS under load.
The Server-only process reached 33,716 KiB. Server-only initialization,
paired Server initialization and Client admission took 10.98, 15.99 and
27.07 ms respectively. Each measured command released its journal at
65,536 bytes. The peak wire-visible data in flight was one record.

| Load | Median completion | Throughput | Data RPCs, total | Server to Client bytes | Client to Server bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| 20 sequential create-only writes, 64 KiB each | 2.42 ms | 23.44 MiB/s | 40 | 1,750,031 | 4,822 |
| 20 sequential reads, 64 KiB each | 1.13 ms | 54.35 MiB/s | 20 | 1,980 | 1,752,180 |
| 5 UI argv commands, emit 1 MiB, retain 64 KiB | 67.11 ms | 14.78 MiB/s | 50 | 15,220 | 451,625 |
| 5 UI argv commands, emit 8 MiB, retain 64 KiB | 67.50 ms | 118.07 MiB/s | 50 | 15,230 | 451,625 |

Line selection and Grep returned one selected result from 67,108,877
source bytes. Each used one RPC. Line selection transferred 99/265 bytes
in a 56.73 ms median with 8,380 KiB Client peak RSS. Grep transferred
149/132 bytes in an 18.94 ms median with 9,668 KiB peak RSS. Each probe
was repeated three times. These two Client-only measurements describe
actual scan cost without charging the synthetic measurement peer to the
product Server. The queue saturation test is separate from the sampled
throughput, which is sequential and subject to host load.

The repository environment benchmark also ran successfully with the tree
mutation candidate. Each workload explicitly composes its measured Session,
runs five warmups and 20 write/read/watch/argv/unwatch/remove cycles, and
tests four simultaneously running commands and eight active watchers.
All four modes recorded zero model calls and only the Server and bound
Client remained after each command workload. The watcher still ran
on the Server at the measured revision.

| Debug workload | Median cycle | Peak Server RSS | Peak Client RSS |
| --- | ---: | ---: | ---: |
| One UI carrier | 481.14 ms | 123,128 KiB | 23,436 KiB |
| Three UI carriers | 140.06 ms | 123,860 KiB | 23,060 KiB |
| Three Session records, one measured binding | 129.48 ms | 125,884 KiB | 23,432 KiB |
| Three UI carriers, one idle and paused | 127.42 ms | 123,660 KiB | 23,204 KiB |

The debug Server binary SHA-256 is
`f2ae5fe448122e581225cdae5432f9decf61f47dbac9e03c664803425299b15d`.
The benchmark's working-tree source digest is
`c6220465ed108555f3f0c0a35b0fcc89062ac821e81ebfc9665f63c7031b8e4c`.
Other local builds ran during measurement. The modes use different
process states and are unsuitable for causal comparisons.

### Client scans, writes and Agent Bash

The v4 Linux wire probe ran three fresh release Client and measuring-peer
pairs per workload. The release Client was built from `f6430047c` and had
SHA-256 `ee1054ef89c706cf346cb291157e1c6a384a00134acdd645513110e7fe103b70`.
Each data request followed verified pairing and Ping. Shutdown required a
physical Revoke receipt and the Client process exit. The peer held one
data exchange at a time. The Write used Begin, 32 contiguous 64 KiB chunks
and one Commit, then read the published bytes outside the timing sample.

| Workload | Data RPCs | Server to Client bytes | Client to Server bytes | Client peak KiB | Measuring peer peak KiB | Startup median ms | Transfer median ms | Shutdown median ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ReadLines, 64 MiB source and 13 selected bytes | 1 | 99 | 265 | 9,200 | 18,860 | 2.82 | 59.61 | 44.19 |
| Grep, 64 MiB source and 27 selected bytes | 1 | 149 | 132 | 10,260 | 18,832 | 2.84 | 18.69 | 84.93 |
| Write, 2 MiB | 34 | 2,798,628 | 2,212 | 11,020 | 19,032 | 2.81 | 19.65 | 84.20 |

Effective warm-cache source throughput was 1,074 MiB/s for ReadLines and
3,425 MiB/s for Grep. Write delivered 102 MiB/s including RPC encoding,
round trips and physical publication. Client logical I/O recorded 67,108,877
bytes read for lines, 134,217,897 bytes read for search and 2,097,152 bytes
written for Write. The independent idle control lane sent 0–67 bytes while
an individual measured operation was underway. The peer footprint above
does not measure the product Server. The previous product Server/Client
table measures UI traffic and a different binary. Neither table measures
the internal output queue high-water under simultaneous full Sessions, so
their peak memory and latency figures do not establish a comparative speedup.

**Measured Client boundary, not a full Agent benchmark.** On Linux
`7.0.0-43-generic` x86_64, the debug Client (binary SHA-256
`f0d0e7d899716cfa8bf89cb7c46759255aa34b3341d390978d6f135c5f8dd004`)
served the following near-EOF requests to a Python UDS measuring peer.
Reproduce with `python3 test-support/measure_execution_reads.py /absolute/path/to/rustx-execution-client`.

The table below is **historical, pre-pairing data**: it was collected
before the control lane existed, so no `ControlPoll`/`ControlTurn`
traffic ran during those windows. The probe now performs the real
paired handshake (fresh 256-bit challenge + `ControlHello`) and answers
the continuous poll loop on a second lane. The paired-v4 results above
report `client_process_rchar` separately from
control bytes sent and expected scan volume. Process snapshots cannot
attribute server-side sends to physical reads by the Client.
Pre-pairing and paired-v2 numbers are not directly comparable — the
paired lane adds a fixed ~100 ms poll cadence plus its frame bytes —
and this table is left frozen rather than silently recomputed.
The fixture is deterministic: blocks of 128-byte lines followed by the same
unterminated 13-byte UTF-8 marker containing a quote, backslash and tab. Each
row has three fresh peer/Client runs, warm page cache, one outstanding
request, one data RPC (handshake/calibration excluded), no save. TX/RX count
actual framed bytes including both four-byte prefixes. Elapsed time is the
median request/reply round trip; HWM is the maximum Client VmHWM across the
three processes, including startup. `rchar` is a process counter calibrated
against a no-file Ping: it measures logical reads, not storage-device I/O.

| Operation | Input bytes | Selected bytes | TX/RX bytes | Client rchar excluding socket | Client HWM KiB | Median ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| ReadLines | 1,048,589 | 13 | 97/262 | 1,048,589 | 14,128 | 8.80 |
| ReadLines | 8,388,621 | 13 | 98/263 | 8,388,621 | 14,048 | 69.96 |
| ReadLines | 67,108,877 | 13 | 99/265 | 67,108,877 | 14,208 | 510.23 |
| Grep | 1,048,589 | 25 | 149/130 | 2,097,260 | 17,716 | 4.68 |
| Grep | 8,388,621 | 26 | 149/131 | 16,777,324 | 17,416 | 32.03 |
| Grep | 67,108,877 | 27 | 149/132 | 134,217,836 | 17,400 | 243.09 |

The Client reads the full source for line counts and UTF-8 validation.
Grep validates before searching the same descriptor; its observed `rchar`
exceeded two source lengths by 82 bytes for each size. That extra count is
not attributed to input content. Grep's displayed line-number width changes
with input size, but each result contains exactly one match. The Python peer
HWM was 18,492–18,680 KiB: **not** product Server RSS. These measurements do
not establish a release-mode regression delta, queue saturation behavior, or
Agent Server memory. Revision `a24cbf6e6` supported neither operation over
the wire: a range-read comparison would measure different work.

Before changing selection or buffering algorithms, retain `a24cbf6e6` as the read-only comparison revision. Do not equate selecting a byte range with selecting lines or matches. Use the same release toolchain/profile, host, private roots and file generator for both variants. Record the generator seed, file byte lengths, offsets, UTF-8/escaping distribution and expected selected bytes. Use growing inputs and a fixed selected result, including near EOF. Count bytes at the Client file reader and complete encoded frames in both directions (including prefix), not just decoded result size. Count requests, not syscalls. Record Server and Client peak memory separately, data/control high-water marks and elapsed time per repetition. Report unsupported baseline operations as not comparable. No before/after delta or end-to-end performance result has yet been measured.

### Revision-five Linux paired probe

The updated probe ran three fresh debug Client processes per workload from
binary SHA-256
`18b9ada2a74357316621d27f4c009137368b85687de03776f03770056f50bf10`.
Each sample verified both authenticated lanes and Ping, then required a
physical Revoke receipt and a successful Client exit. Build activity ran
on the same host. These figures describe the debug probe with one
in-flight request and have a different profile and load from the
revision-four release figures above.

| Workload | Data RPCs | Client logical I/O | TX/RX framed bytes | Client peak KiB | Startup median ms | Transfer median ms | Shutdown median ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ReadLines, 64 MiB and 13 selected bytes | 1 | 67,108,877 read | 99/265 | 26,496 | 5.46 | 525.11 | 80.49 |
| Grep, 64 MiB and 27 selected bytes | 1 | 134,217,897 read | 149/132 | 29,724 | 5.53 | 245.46 | 59.72 |
| Write, 2 MiB | 34 | 2,097,152 written | 2,798,628/2,212 | 27,324 | 9.67 | 1,322.24 | 49.72 |

The median source rates were 122 MiB/s for ReadLines, 261 MiB/s for Grep
and 1.51 MiB/s for Write including commit. The Python measuring peer peaked
at 18,824 KiB. Workload control traffic ranged from 133 to 1,872 bytes.
The probe's one in-flight request measures its own scheduling limit.
The product Server memory and queue saturation measurements use the
separate App Server fixtures.

The release Client built from `83fb1b117` ran the same three workloads
with only the Client executable in its staging directory. Each fresh
process completed authenticated UDS pairing and Ping, then exited after
the physical Revoke receipt. The data RPC counts and framed byte totals
match the debug sample above.

| Workload | Client peak KiB | Startup median ms | Transfer median ms | Shutdown median ms | Source throughput (MiB/s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| ReadLines, 64 MiB | 9,880 | 2.79 | 90.69 | 10.37 | 706 |
| Grep, 64 MiB | 11,200 | 2.84 | 30.08 | 71.01 | 2,128 |
| Write and commit, 2 MiB | 11,400 | 2.67 | 23.15 | 77.42 | 86.4 |

Each row is the median of three warm-cache runs. The Python peer's peak
RSS was 18,872–18,892 KiB across the workload medians. Build profile and
concurrent host load differ from the earlier samples.

### Paired read probe during Agent Bash development

Collected with the debug Client binary
`aa8215bb65f78dcdb514b8a8a1b465f917c1da00fb3a1deeef44b4c6f7b713cb`
on Linux 7.0.0-43-generic x86_64. Each row is three iterations, one data RPC
per iteration. The paired control lane is live. TX/RX report data frames
including framing. Peak memory includes Client startup.

| Operation | Source bytes | Selected bytes | TX / RX bytes | Client peak KiB | Median ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| ReadLines | 1,048,589 | 13 | 97 / 262 | 20,856 | 8.76 |
| ReadLines | 8,388,621 | 13 | 98 / 263 | 20,648 | 67.50 |
| ReadLines | 67,108,877 | 13 | 99 / 265 | 20,600 | 492.58 |
| Grep | 1,048,589 | 25 | 149 / 130 | 23,744 | 3.32 |
| Grep | 8,388,621 | 26 | 149 / 131 | 23,880 | 30.15 |
| Grep | 67,108,877 | 27 | 149 / 132 | 23,928 | 244.48 |

The measurement peer is Python. It cannot establish product Server RSS,
write throughput, Bash start-to-completion latency or output queue high-water
marks. The historical pre-pairing results use a different transport and Client
build and cannot establish a regression delta.

### Bash capture and standalone Server probes

Three Linux debug samples per size used the same measuring peer and the
staged Client
`862519c865c5ce4d30e9c0fbab9f69acf464caf88f057ed3dc02bf0eb7506830`.
The command emitted fixed 32-byte lines through pipes. The peer queried
operation state every 2 ms, verified the Client's exact final stdout count,
requested the bounded previews, released the raw journal and read a
single continuation line. RPC and encoded byte counts cover registration
through that line query. Timings include process launch, capture and the
poll interval. Throughput is captured MiB divided by observed START-to-
settlement duration.

| Output | RPCs | Data TX / RX bytes | Client startup median | First running observed | Settlement median | Capture MiB/s | Client peak KiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 MiB | 19 | 4,425 / 38,406 | 4.72 ms | 6.58 ms | 30.26 ms | 33.04 | 21,584 |
| 8 MiB | 27–28 | 6,090–6,298 / 40,215–40,441 | 4.30 ms | 6.58 ms | 47.50 ms | 168.42 | 21,852 |

The settled raw journal cursor was zero for every sample. The raw journal
was disabled for these calls. This does not measure supervisor pipe
occupancy or the highest data/control queue occupancy under contention.

The standalone debug Server
`c29ca24ee8699c67d8df9d3402d60f35c1e8bfe7ff7beccbe82347c7ca602099`
ran in a directory containing only the Server executable, with a private
empty HOME/runtime root and no children observed after initialization.
One JSONL initialize RPC sent/received 191/351 bytes, with 487.73 ms
median startup and 92,540 KiB maximum Server VmHWM across three launches.
This cold management-only Server measurement is separate from the
Client-only Bash capture probe. The Bash path at the PR baseline has
no equivalent Server/Client split to serve as a paired regression baseline.
A 1 MiB deterministic binary workspace write on the same Client used
one BeginWrite, 16 chunks and one CommitWrite. The 18 data RPCs sent
1,399,365 bytes and received 1,220 bytes including framing. Three runs
measured a 26.67 ms median through the verified commit (37.50 MiB/s)
and 21,648 KiB maximum Client VmHWM. Each published file matched the
input byte for byte. Measurements exclude fixture creation and verification
reads. Agent Server memory during execution and queue high-water
telemetry remain unmeasured.

## Final-head gate

The [FULL run on `a47bd44c1`](https://github.com/Caismis/rustX/actions/runs/44904383504)
passed Linux Boundary after that job disabled test debug information.
Both prior Boundary attempts on `d7ff97b57` lost the hosted runner during
the compile and staging step. Locally the previous settings triggered a
cgroup OOM at the runner's reported available-memory budget of about
14.4 GiB with swap disabled. The new settings compiled and staged the
same targets under that limit with zero OOM kills. The local Boundary
retry passed 847 Rust tests and 55 provider-emulator tests, with nine
existing Rust ignores, and passed the lane inventory check.

Deterministic contracts on the same GitHub run failed the assertion
that a completed Compaction's old Runtime is gone after unload while
delivery of its receipt is parked. A status read or recovery could still
hold the native Runtime across composition removal. Retirement now
waits for the same status guard before releasing the composition. A
finished Agent Attempt also drops its native Runtime reference before
acknowledging its lifecycle admission. The status regression parks the
actual recovery reader while it owns the native Runtime and checks
that unload waits and releases the allocation before returning.

The [FULL run on `ec9342c49692893eca14bba7c1b9d3ec32fae10f`](https://github.com/Caismis/rustX/actions/runs/44744451236)
passed generated protocol, Format and lint, TUI, Web/Host/launcher, and
Browser input build. Linux Boundary, Deterministic contracts, all three
Browser shards, and Merge verification failed. The local prepared
workspace test stopped in the Server library with 6,145 passed, seven
failed, and 27 ignored. The seven failures affected three recovery
fixtures that enrolled the wrong Session and four Workflow Code fixtures
that substituted Python symlinks for a venv. The corrected targeted runs
pass. The independent Linux process suite also passes the corrected
detached-stdio Python case.

Local pinned Chromium runs passed native Bash promotion, Stop in both
languages, replay of removed capabilities in both layouts, native child
settlement, file delivery, Session deletion, and uploads. The fork test
reuses the two actual upload receipts returned by the fork after starting
an authorized Client for the destination Session and observes exactly four
mock-provider requests. New exact-head FULL and Merge verification and a
native macOS run are still required.

The [FULL run on `142be9864d440bd5770794861a8da0fc6d913086`](https://github.com/Caismis/rustX/actions/runs/44751385601)
was still in progress at the 30-minute checkpoint. Local exact-head
format, check, strict Clippy, generated protocol, TypeScript and 15 Rust
doctests passed. The prepared workspace test stopped after 6,152 passed,
two failed and 27 ignored in the Server library. The Bash deadline case
used an unbound fixture. Its corrected test waits for the real Bash PID
before advancing the manual clock, observes exactly two model requests,
confirms the process exited and keeps the single terminal assertion. The
other failure was unavailable Managed Python capability preparation. Its
unchanged isolated rerun passed in 66.27 seconds.
The Linux Boundary job then reached the external Tools target. Its two
failures were Skill/Bash fixtures without a Client. Both pass with the
existing real Session-bound fixture, using their original content and
success assertions.

At `910646ca5f859bc9834cbcd8ea5bf37b0c48fdde`, formatting, workspace
all-feature check and strict Clippy, release binary build, generated
protocol, TypeScript, 15 doctests, five Linux mount cases, and all seven
external Boundary targets (494 passed and five ignored) passed locally.
The [CI Boundary job](https://github.com/Caismis/rustX/actions/runs/44757936244/job/136363087961)
also passed 291 in-crate Boundary tests, all seven external targets,
and the Client bootstrap and WebSocket tests. Its inventory check
still sought the moved Python suite in the Server library. The required
suite now points to the Client's 66 `workspace_python::tests::` cases.
The local workspace run reached 6,152 passed and 27 ignored before two
Managed Python setup failures. The catalog warmup timed out; the other
case hit a PyPI wheel download timeout. Both passed unchanged when
rerun separately, so the full run remains recorded as failed.

The [FULL run on `55c8cdc94b1a515e4da921156c9befd376a43ee7`](https://github.com/Caismis/rustX/actions/runs/44770182145)
passed the Linux Boundary test steps. Deterministic contracts reached the
Client Workspace Python library and failed 11 cases before their intended
assertions because that job did not install `uv`. The contracts job now
uses the repository's Linux Python and `uv` setup. The system interpreter
and Python 3.12 provide the two real minor versions required by the
interpreter-selection cases. Locally the same revision passed all 6,154
Server library tests. Four external Workflow cases then hit an ignored
virtual environment generated under the reference example. All four
passed with the same test executable after that generated directory was
moved outside the source tree.

The [Linux Boundary run on `0f2444ca1`](https://github.com/Caismis/rustX/actions/runs/44776520497/job/136428338987)
exposed the Server returning its watch permit before removing the retired
UI watch ID. The directory/file replacement scenario could observe all
permits returned and still get `EnvironmentCapacityOrConflict` on reuse.
The Server now releases the slot and settles explicit unwatch after the
old registration is removed. The terminal-order test holds the notification
receipt and asserts that the old registration still owns its permit.

## Command cancellation, detach, and release (UI-owned `command/exec`)

- Cancellation has one owner: `terminate` and UI detach only signal the
  operation's entry; its driver starts exactly one control-lane delivery
  (`CancelProcess`) off the reactor, within one absolute budget covering
  queue wait and the correlated receipt. Only a refusal proven to precede
  the queue (`WouldBlock`) is retried inside that budget. An undeliverable
  or `Unknown` cancel fails closed unless the operation is already proven
  settled: the binding is fenced and its sockets shut without waiting on a
  wedged data exchange. A delivered, known cancel keeps the pair live. A
  receipt is never physical containment; the driver keeps waiting for the
  Client's settled snapshot.
- A detached operation (UI delivery failed or the UI closed) keeps its
  driver: it stops emitting, tracks settlement, and releases the journal
  through its exact final sequence without draining it (release is a
  discard, not a delivery). Pre-START returns deliver the cancellation first.
- Release retries only a proven pre-wire `Busy` within one absolute budget;
  any other answer is final and never relaunches. A failed release never
  changes the known business outcome.
- Tests: `an_unknown_cancel_cuts_the_pair_while_a_data_exchange_is_parked` and
  `a_delivered_known_cancel_keeps_the_pair_live` (real admitted pair, control
  peer holds its acknowledgement, data peer parked on the wire),
  `detached_operations_release_without_touching_siblings` (five
  sequential detached operations on one Session, each physically terminated,
  an independent sibling keeps running to its own result),
  `release_retries_prewire_busy_then_releases`, and the execution-server test
  `a_saturated_control_lane_refuses_a_cancel_explicitly_and_delivers_it_once_a_slot_frees`.
- `detach_during_data_and_control_backpressure` proves the combined case
  through a real UI connection and staged Execution Client. It holds the
  data reply and all four health receipts, cuts the UI carrier, and observes
  business process exit before unblocking data. It then verifies the final
  journal cursor, its exact release receipt and Client exit.

## Supervisor trace authority removed (Execution Client)

- The Client process no longer reads `RUSTX_TEST_SUPERVISION_TRACE` in
  production: `Operations::new` carries no diagnostics, and the physical
  runner receives `None` for every operation outside tests.
- The native revocation scenario (`revocation_before_native_start_never_executes_an_independent_command`,
  12 arms) proves the physical spawn by reading the business process's own
  pid (`printf '%s\n' "$$"`, then `exec sleep 600`), and requires that pid
  to be physically gone after the operation drains.
- The START hold is owned per operation (`arm_start_hold(&ops, &key)`), not a
  process-wide singleton. The Client test
  `a_selected_start_hold_is_owned_by_exactly_one_operation` starts an
  unheld operation first; the held operation is cancelled while parked and
  classifies as `CancelledBeforeStart` with zero business marker.
- Regression `poisoned_trace_variable_is_never_authority` runs a real staged
  Client behind a launcher that sets only the poisoned trace variable, then
  runs a bound command; the supplied path is never created.

## Optional save contract (EXEC-01 slice)

- Wire: `SaveRequest`/`SaveReply` with stable domain codes, Client-allocated
  `CandidateId`, owner binding, no Server/Host paths, no locator before publish.
- Reference: pure state transitions in `protocol::save` (no filesystem writes), one selection and one candidate per import,
  covering exact threshold selection, absence at crossing, deadline clamping,
  digest-only chunk ledgers with a one-chunk duplicate window, separate
  seal/publish, prefix publish requiring nonzero evidenced bytes, and terminal abort.
- Fixtures: `crates/execution-protocol/fixtures/save-envelopes.json`, round-tripped
  through the outer wire framing by `save::tests`.
- Publish, probe, and cleanup are physical: `SaveCapture::apply(request, now,
  storage)` calls a `SaveStorage` only after every check passes. A refused
  receipt publishes nothing; probe reports storage readability only after
  publish; abort reports the storage cleanup outcome; steps other than abort
  and probe are deadline-gated before any state change.
- Client decline proof: `save_is_declined_as_unavailable_without_touching_the_workspace`
  and `a_reservation_requiring_managed_output_is_rejected_by_the_real_client`
  (`crates/execution-client/tests/bootstrap.rs`).
- Not implemented and not advertised: Client storage, Host integration,
  readability probing against real optional-save storage.
