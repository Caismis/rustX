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

## Phase-one gates

`bootstrap` = `rustx-execution-client --test bootstrap`, selected by rust-fast and rust-boundaries. `member unit` = package-qualified libtest, selected by the Linux FAST unit step and FULL contracts. Listed lane ownership does not certify a green run on a new revision.

The merge of `origin/main` (50d0e5728) removed the macOS CI lanes (`rust-platform-core`, `rust-platform-boundaries`) wholesale: upstream retired its macOS runners, so this branch's Execution-Client coverage there went with them rather than being ported. CI proof is therefore Linux-only today; native macOS execution of the `bootstrap`/`bins` suites is pending an independent native run and is not claimed here.

| ID | Required PR1 proof | Current implementer / test | Lane and evidence at initial audit |
| --- | --- | --- | --- |
| T01 | Server-only work without any execution dependency | `app_server::process`, `standalone_server_reads_cold_sessions_without_workspace_or_helpers` | Partial; Linux process test passed: two real catalog IDs, no workspace at either recorded path, only the Server executable on PATH. No model-call or output-capture proof yet |
| T02 | Separate filesystem views for read/write/Bash through product | `explicit_composition_reads_from_client_mount_namespace`, `filesystem_operations_use_the_private_mount_view`, `background_live_output_is_readable_while_running_and_reused_at_settlement` | Linux mount proofs cover the App Server query/Write/argv path and native Agent Read/Write/Bash/Grep. The writable overlay exercises Client directory creation, file copy and recursive removal while the Server sees the original empty mount point. Server watch registration still opens native root pins and needs migration. Separate macOS gate applies |
| T03 | Exactly one Session dispatch owner | `Binding::connect`, `concurrent_hellos_commit_exactly_one_live_connection`; `session_runtime_manager::execution` claim registry and endpoint dispatch | Partial; member unit at `b75d78c75` tests one Binding, not competing Session reservations; `5fddad74f` proves one endpoint claim per Session with dropped-waiter safety; `the_endpoint_dispatches_one_bounded_read_for_its_own_session` proves the stored connection is the sole serialized dispatch owner — `fs/readFile`, `fs/getMetadata`, `fs/readDirectory`, and `fs/writeFile` dispatch today |
| T04 | Root, child and Workflow route to exact Session | `SessionExecutionScope` in the Root runtime and Workflow native tools, `local_runtime::subagent_child` | Partial. Root dispatch uses its exact Session. Workflow reuses the bound Workspace in the same runtime. The separate Subagent child still needs authorized IPC dispatch, and the combined cross-role test is pending |
| T05 | Cancel before dispatch/model/START causes zero corresponding side effects | `runtime::conversation_runtime::attempt`, `revocation_before_native_start_never_executes_an_independent_command`, `agent_bash_has_separate_registration_and_a_client_owned_start_gate` | Partial. Held revocation and UI close refuse App Server dispatch before START. Held physical Client Bash START yields zero business marker and zero output, with one operation identity. Cross-role model request and file-commit evidence remains incomplete |
| T06 | Parallel results commit in call order, unique terminal is last | `client_bash_reversed_settlement_keeps_canonical_order` | Passed for two parallel Root Agent Bash operations on one real Client. A channel gates the first on the other's physical settlement, both have one START marker, exactly two scripted model requests follow the expected flow, canonical tool results retain the input order and the attempt emits one final terminal after them. Pending cross-role coverage under T04 |
| T11 | Real control/cancel/bidirectional health progress with saturated data | `detach_during_data_and_control_backpressure`; split Unix/WebSocket send blockage, pairing and data-permit tests | Linux integration passed. The real Execution Client answers an actual UI command. The proxy parks a Client metadata reply and the first health receipt. Four control slots stay occupied and the fifth probe returns `WouldBlock`. After real UI WebSocket EOF, the keyed cancellation progresses while the data reply remains parked. The business socket closes before the data gate opens. All four health probes receive their own receipts. Agent-flow saturation across other roles and the future recovery cases remain open |
| T18 | Authenticate the binding before Workspace configuration | `execution_enrollment_precedes_client_and_workspace_configuration_readiness`, production launcher enrollment | Passed on Linux for trusted remote enrollment. The durable Session is inspectable before Client startup. Unbound browsing fails, the authenticated Client browses its Workspace, and malformed Workspace configuration refuses Agent attach without disabling history or management |
| T24 | Independently built and launched Server and Client, isolated credentials/dependencies | Client package + dependency tree, `Launcher.start`, `app_server::process::local_execution`, Server-only process test in T01; `rustx-process-supervisor` is an independently built physical package whose `bash-supervisor`/`interactive-supervisor` helpers stage beside the Client | Partial; isolated runtime launch, cold catalog and the paired-flag two-process composition verified (`explicit_composition_launches_and_reaps_the_sibling_client`: trusted sibling, empty environment, sealed stdio, drained reaping); the process kernel builds and tests standalone in `rustx-execution-client` with no root/runtime dependency. Complete product runtime dependency audit outstanding |
| T26 | Client revocation wins before file commit or START | `client_applied_control_wins_before_file_publication`, `a_revoked_binding_after_prepare_sends_no_commit_frame`, `agent_bash_has_separate_registration_and_a_client_owned_start_gate`, Client mutation gate tests | The real paired Client proves zero effects before file publication for cancel and binding revoke. Both cases send exactly one prepare and one commit frame, return typed `CommitCancelled`, and leave the Workspace empty. Real Bash cancellation before START produces no business marker. Mid-crossing disposition is tested at the Client mutation gate |
| T28 | Reject wrong Session, credential, instance, generation and duplicate | `wrong_hello_fields_never_consume_a_reservation`, `credential_replay_after_eof_never_reconnects`; `session_runtime_manager::execution` claim registry | Partial; member unit + bootstrap; `5fddad74f` proves distinct per-Session binding material and delete/drain revocation; `fs/readFile`, `fs/getMetadata`, `fs/readDirectory`, and `fs/writeFile` Session routing is live — an uncomposed Session returns `ExecutionUnavailable` and never serves or mutates another Session's bytes |
| T29 | UI command ownership distinct from accepted Agent execution | `detach_during_data_and_control_backpressure`, `detached_operations_release_without_touching_siblings`, PTY and drain suite | Combined two-process Linux test passed. It asserts one register, one START, a correlated `CancellationRequested`, a physical `Cancelled` snapshot, the original five ordered stdout bytes, zero stderr, exact release cursor, one `OutputReleased` and Client exit. A distinct UI operation survives detach in the sibling scenario. Later transport recovery remains #577 |
| T31 | No Client: truthful truncation, zero Server spill/start/wait | `ToolOutputCapture::PreviewOnly` in `tools::output`, `ManagedOutputContinuation::Unavailable` in `tools::mcp::result`, `tools::mcp::result::tests`, `mcp::unix_tests::foreground_mcp_result_normalization_is_collective_and_complete` | Partial; foreground MCP overflow now settles a typed `Unavailable` with accurate `original_bytes` and no locator — `foreground_mcp_overflow_never_attempts_storage_and_is_unavailable` and `foreground_mcp_overflow_never_writes_storage` prove zero save attempts against forced-open/write failure fixtures, the `assert_unavailable_no_save` helper asserts zero `r-`/`e-` files and no Read/Grep promise, and the real rmcp stdio boundary test verifies the same through a staged peer. A remote `is_error` keeps `Failed` at overflow (`foreground_mcp_overflow_keeps_a_reported_remote_error`); multi-fragment crossings count once (`foreground_mcp_text_image_text_overflow_preserves_order`). Structured JSON renders through a bounded streaming writer, and text blocks retain only bounded head/tail previews — no speculative complete-output retention. Still open: a counter-based whole-Server no-Client fixture (Client-launch attempt counting beyond this path) and the optional Client save binding itself; background MCP capture still needs its own evidence. Session-bound native Bash stores its continuation on the Client |
| T32 | Complete normalized import including displayed prefix | `protocol::save` `SaveSelection` + `SaveCapture` reference, decode boundary `decode_chunk`, and `save-envelopes.json` | Partial; contract and executable fixtures only; no storage, no Host import |
| T33 | Save faults cannot replay won business execution | `protocol::save` state machine, fault-injected store fixtures and `expired_chunk_seal_and_publish_are_deadline_gated_before_any_state_change` | Contract evidence for typed failure, deadline and cleanup. Production disk-full/disconnect behavior depends on the later save store and caller wiring under #577/#578 |
| T34 | One selection at threshold; absence or generation change never retargets | `protocol::save::SaveSelection::observe` (exact threshold, one lookup at crossing, `Absent` and replacement owners never rebind, deadline min(now+5s, call) tagged once) | Partial; pure selector tests only; Host driver not implemented |
| T35 | Publication acknowledgement versus one canonical tool commit | `protocol::save::SaveCapture` defines the publication receipt and candidate identity independently of the business result | Contract only. The production save-acknowledgement versus canonical commit race depends on #577/#578 |
| T36 | Small result/error/validated End share claim gate, Begin never claims | `app_server::host_upload` `HostUploadSlot`/`ValidatedEnd` (opaque, End-only) and `app_server::callbacks::tests::only_a_verified_end_can_compete_with_an_ordinary_reply` racing a small success or valid JSON-RPC error through the real `Callbacks::respond` in both orders, with subscription recheck at claim | Partial; claim arbitration exercised with a cfg(test) fixture body only; no production receiver (#578) |
| T37 | Independent budgets and business/capture/readability facts | `app_server::host_upload` upload-record budgets: 1024 items, 64 KiB per chunk, 1 MiB per result, as schema constraints and verifier refusals | Partial; budgets independent of transport frame and management limits; business/capture/readability facts unimplemented; no raw pre-parse byte bound in a production path |
| T41 | Seeded local/remote semantic equivalence | `remote_execution_serves_session_reads_over_websocket`; execution-client `websocket` suite (9 tests); `rustx-execution-transport` suite (14 tests) | Partial; real two-process App Server + operator-spawned staged Client over genuine RFC 6455 upgrade — `fs/readFile`/`getMetadata`/`writeFile` cross the WS channel with byte- and revision-identical semantics, second-Session `ExecutionUnavailable` parity, `0600` `create_new` bundle proof, and clean Client exit on channel EOF. Member-level WS proofs: credential/session/version/generation tamper stays `Reserved`, plaintext non-loopback + wrong path never connect, malformed upgrade never consumes the reservation, slow trickling peer shares one absolute deadline, revocation wakes an idle WS read, EOF never allows replacement, `AlreadyConnected` on a second live claim. Transport level: one `EncodedRecord` per binary message, text/oversize/malformed rejected pre-deserialize, wrong path/`Origin` refused, deadline-bounded silent peer, Unix+WS shutdown wakeups, real `wss` with fixture CA + IP SAN (positive), wrong-CA/wrong-SAN negatives proving no record before TLS validation. Remote `wss` is operator-ingress-terminated TLS documented to forward to loopback only; no Server-side TLS listener shipped; RFC ping/pong is not T11 control/backpressure — that stays Open |
| T42 | Bounded line/match projection: transferred bytes, client reads, memory, RPCs | `read_range_serves_verified_bytes_at_a_sparse_file_boundary`, `a_fully_consumed_empty_file_is_a_valid_read`, `a_size_contradiction_retires_the_stream_and_detaches_the_binding`, `read_lines_pages_a_large_file_in_single_bounded_exchanges`, `read_lines_reports_end_reasons_and_keeps_stable_semantics`, `read_lines_stale_revision_is_a_conflict_not_a_read`, `read_lines_refuses_non_utf8_and_keeps_the_connection_live`, `read_document_decodes_every_whitelisted_format`, `read_document_serves_deterministic_pages_and_stale_revisions_conflict`, `read_document_refuses_unsupported_corrupt_oversized_and_textless`, confined-fs `text` scanner unit matrix, document-reader unit matrix, `search_serves_the_shared_universe_and_never_follows_links_outside`, execution-client `search::tests` matrix, `a_search_page_is_verified_and_a_dishonest_one_retires`, `unbound_sessions_refuse_search_without_server_traversal`, tool-plane glob/grep suite | Partial; bootstrap proves byte range plus verified `FileStamp` source descriptor, directory-page name/cursor discipline, and one bounded `ReadLines`/`Lines` typed exchange per page over the real Client — early and late pages of a ~1 MiB file each return in one RPC with whole-source `total_lines`/stamp (`read_lines_pages_a_large_file_in_single_bounded_exchanges`); the shared `rustx_confined_fs::scan_text` scanner the page and the native Read tool both run on is covered by an independent edge-case matrix (empty, trailing newline, CRLF/BOM, limits, offset-past-end, UTF-8 failure in the skipped tail, code points split across read blocks, giant first line, exact byte/line caps with terminal newline) and `ReadDocument`/`DocumentPage` adds the same bounded page over the decoded Markdown of the committed fixture corpus — all four whitelisted formats decode through the real Client to deterministic, source-stamped pages with `logical_bytes` independent of the compressed source, stale revisions conflict, and the unsupported-extension/corrupt/over-cap/no-text-layer refusals are distinct typed codes that never retire the connection; the native Agent Read tool now dispatches both page kinds through the Session's admitted Client via `SessionExecutionScope` — `tools::native::read` runs its whole matrix against a real `SessionRuntimeManager`-reserved, staged-Client route, `agent_read_dispatches_only_through_the_bound_session_scope` proves one bound Session succeeds while a second Session on the same lexical path answers unavailable with no Client prepared, and an unbound route is an honest `Session execution unavailable` with no Server-local filesystem branch; native `read_image` now routes its physical bytes through the same scope as bounded `ReadRange` chunks — `a_multichunk_png_commits_exact_artifact_bytes` fetches a ~148 KiB PNG across multiple wire chunks through the real Client and commits byte-exact immutable artifacts that outlive the source, `an_unbound_session_refuses_image_reads_without_touching_artifacts` allocates nothing and leaks nothing, scripted-peer endpoint tests prove an over-bound stamp stops after one `ReadRange` (`an_oversized_image_source_reads_one_range_and_stays_live`), a mid-fetch replacement is an honest live `conflict` (`a_replacement_between_image_chunks_is_an_honest_conflict`), and a same-revision stamp change retires (`a_stamped_mismatch_between_image_chunks_retires_the_connection`); the `with_active`-gated artifact commit keeps cancellation deterministic on both sides of the durable store (`image_cancellation_on_each_side_of_artifact_commit_is_deterministic`); native `glob`/`grep` now dispatch one bounded `Search` exchange through the same scope — `crates/execution-client/src/search.rs` runs the shared descriptor-relative universe (hidden visible, ignore files not applied, links never followed, symlinked roots refused, lexical order) under the frozen traversal budgets, `search_serves_the_shared_universe_and_never_follows_links_outside` proves two links to outside never crossed and an invalid-UTF8 tail excludes a whole file over the real Client socket, the `search::tests` matrix covers file roots, `SearchNeedsDirectory`, budget refusals (an 8 MiB single line fails as `SearchBudgetExceeded`, never OOM) and the hit-limit proof, `a_search_page_is_verified_and_a_dishonest_one_retires` proves the Server retires an over-count page, and the tool-plane glob/grep suite runs its full contract against the real bound route plus an honest unbound refusal; the conditional edit now runs the real Client end to end — `a_conditional_edit_commits_bom_crlf_and_multiple_edits` proves BOM/CRLF preservation and a fuzzy anchor through `PrepareEdit`/`CommitWrite` on the real socket, `edit_refusals_are_typed_and_the_connection_stays_live` proves typed ambiguity/no-change/oversize refusals stage nothing and keep the connection live, `a_denied_edit_gate_aborts_and_a_replaced_target_conflicts` proves a gate denial aborts the private stage and leaf/ancestor swaps are pre-commit denials, `a_malformed_edit_request_is_an_error_not_a_stage` proves a malformed wire request is a stable `Error` with no stage, and the endpoint tests prove `EditPrepared` validation (zero ticket, wrong revision, over-cap `published_size`), contradicting refusals, and `CommitUnknown` retirement; the native Agent Write/Edit tools now route their physical mutations through the same admitted-Client boundary — preflight binds a ticketed `PreparedMutation` under the logical interpreted path (`WriteExpectation::Current`), permission classification compares `PLAN.md` against the logical workspace path the Client proved with no Server-local canonicalize/stat probe, `run_write`/`run_edit` consume exactly one commit-or-abort decision on the live ticket, `OutcomeUnknown` from the publish crossing is never overwritten by a racing cancellation, denied/cancelled preflights abort the ticket explicitly while a dropped owner arranges bounded cleanup, the whole `tool_plane` write/edit matrix runs against a real staged Client (`unbound_sessions_refuse_mutations_without_server_side_write` proves no Server-local fallback — no file created, original untouched), `write_creates_missing_parents_through_the_client_staged_subtree` proves the staged-subtree publish, `parallel_prepared_mutations_commit_and_abort_independently` proves two tickets on one connection decide independently, `a_write_over_the_chunk_limit_prepares_and_commits` proves the >64 KiB `BeginWrite`/`WriteChunk` transfer, and the managed-output suite proves containment failures mutate nothing; the ticketed map (≤8 preparations, one shared in-memory payload budget) holds preapproval bytes in Client memory so nothing is staged or enumerable before the grant; still missing: loaded Server and queue high-water measurements, the WorkspaceChanges baseline/snapshot capture path (a mutation no longer passes through `changes.capture` — the Server holds no descriptor to snapshot), and progress/settlement reporting parity for multi-chunk writes |
| T44 | Minimal reservation before browsing, no unbound access or unrelated startup | `execution_enrollment_precedes_client_and_workspace_configuration_readiness`, `an_unbound_workspace_argument_never_connects` | The production bootstrap path passes on Linux. Cold management stays available during invalid Workspace configuration. The complete product picker and native macOS evidence remain open |
| T54 | Candidate/sequence/offset integrity, late and mixed identities rejected | `app_server::host_upload` verifier and slot refusals (gap, out-of-order, conflict, foreign candidate on every step, late, duplicate-outside-window) tested; candidate identity requires connection and subscription incarnations | Partial; contract and verifier tests; no receiver or live Host subscription binding (#578) |
| T55 | Final encoded size including envelope and escaping | `host_upload_framing_counts_the_encoded_envelope` and generated Rust, JSON and TypeScript fixtures | Exact one-MiB JSONL with the full envelope, base64 and escaped text passes framing. A parseable frame one byte larger is refused at the reader. Production Host receiver belongs to #578 |

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

The remaining #576 production work has these owners.

- `src/app_server/filesystem.rs` still starts the native watch in the
  Server's view. Route registration, physical observer cleanup and change
  hints through the Session binding. Keep UI watch identity and notification
  fencing on the Server.
- `src/workspace_changes` still owns Git baseline, file snapshots and
  comparison reads on the Server. Move those physical operations while
  preserving the Server's attempt association and normalized result commit.
- `src/local_runtime/subagent_child.rs` still composes a separate runtime
  without execution routing. Forward authorized native child operations
  through its existing parent IPC and prove Root, Subagent and Workflow
  share the exact binding under T04.
- The ordinary Workspace configuration owner still reads Workspace sources
  on the Server. Trusted launch can enroll and browse without it, and
  malformed configuration stays a typed attach failure. Remote resolution
  of the Workspace source and the Workspace Python owner's physical
  preparation and selection remain open. Managed Python preparation and
  invocation remain #577.

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
directory creation, removal and copy implementations were removed. The native
watcher still belongs to the Server.

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

The repository environment benchmark also ran successfully with the tree
mutation candidate. Each workload explicitly composes its measured Session,
runs five warmups and 20 write/read/watch/argv/unwatch/remove cycles, and
tests four simultaneously running commands and eight active watchers.
All four modes recorded zero model calls and only the Server and bound
Client remained after each command workload. The watcher still runs
on the Server in this sample.

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

**Measured Client boundary, not a full Agent benchmark.** On Linux
`7.0.0-43-generic` x86_64, the debug Client (binary SHA-256
`f0d0e7d899716cfa8bf89cb7c46759255aa34b3341d390978d6f135c5f8dd004`)
served the following near-EOF requests to a Python UDS measuring peer.
Reproduce with `python3 test-support/measure_execution_reads.py /absolute/path/to/rustx-execution-client`.

The table below is **historical, pre-pairing data**: it was collected
before the control lane existed, so no `ControlPoll`/`ControlTurn`
traffic ran during those windows. The probe now performs the real
paired handshake (fresh 256-bit challenge + `ControlHello`) and answers
the continuous poll loop on a second lane; its rows carry
`"probe": "paired-v2"` and report `client_process_rchar` separately from
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

Record local commands, actual head SHA, Actions checkout SHA, required Linux/macOS jobs, explicit skips and the final `full-verification` / Merge verification URL here before requesting final review. No final-head verification is claimed by this initial ledger.

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
- Pending: a product-level test driving a saturated control lane through a
  real UI command (the saturation is covered at the control handle only);
  the separate Client trace-authority fix is recorded below.

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
