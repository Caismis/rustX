# EXEC-01 acceptance ledger

This is the implementation/evidence ledger for [#576](https://github.com/Caismis/rustX/issues/576), not a second test registry. Cargo discovery and the [lane checker](../examples/check_test_lanes.rs) are authoritative for test execution. The [ADR](execution-protocol.md) owns the wire contract, frozen research revisions, and T01-T73 phase index. "Partial" below means that the named proof does not cover the whole PR1 requirement.

Initial audit: original implementation base `f0cd4f59d7dbdd2730cf262d0afa964af79d9f39`, reviewed branch head `a24cbf6e6059490c8337c34bcbfc2829e5287d6a`, fetched main `15d95ec1ba5f5a19c3975c52633b6bfec8a08135`. #571 merged into the branch of #570, not into this main. Do not infer an accepted workspace layout from its merged badge.

## Phase-one gates

`bootstrap` = `rustx-execution-client --test bootstrap`, selected by rust-fast, rust-boundaries, rust-platform-core and rust-platform-boundaries. `member unit` = package-qualified libtest, selected by the Linux FAST unit step and FULL contracts. Listed lane ownership does not certify a green run on a new revision.

| ID | Required PR1 proof | Current implementer / test | Lane and evidence at initial audit |
| --- | --- | --- | --- |
| T01 | Server-only work without any execution dependency | `app_server::process`, `standalone_server_reads_cold_sessions_without_workspace_or_helpers` | Partial; Linux process test passed: two real catalog IDs, no workspace at either recorded path, only the Server executable on PATH. No model-call or output-capture proof yet |
| T02 | Separate filesystem views for read/write/Bash through product | `LocalConnection::read_range`, `range_read_from_private_mount_view` | Partial; opt-in Linux mount proof exercised at `60fd6087d`, not product routing |
| T03 | Exactly one Session dispatch owner | `Binding::connect`, `concurrent_hellos_commit_exactly_one_live_connection` | Partial; member unit at `b75d78c75` tests one Binding, not competing Session reservations |
| T04 | Root, child and Workflow route to exact Session | `local_runtime::subagent_child`, parent-side execution routing not implemented | Open; needs scripted routing and child IPC proof |
| T05 | Cancel before dispatch/model/START causes zero corresponding side effects | `runtime::conversation_runtime::attempt` has model-turn cancellation tests; remote START absent | Partial; root unit is not remote gate evidence |
| T06 | Parallel results commit in call order, unique terminal is last | Server Agent Loop; no out-of-order execution-channel fixture | Open; needs scripted contract |
| T11 | Real control/cancel/bidirectional health progress with saturated data | Execution Protocol queue constants; synchronous range exchange | Open; constants are not a congestion test |
| T18 | Authenticated bootstrap before config, distinguish unknown | `Binding::reserve_local`; product create still calls `initial_binding` first | Partial; bootstrap unit/binary test only |
| T24 | Independently built and launched Server and Client, isolated credentials/dependencies | Client package + dependency tree, `Launcher.start`, Server-only process test in T01 | Partial; isolated runtime launch and cold catalog verified. Final separate build and complete product runtime dependency audit outstanding |
| T26 | Revocation wins before Client file commit / START | `revocation_linearizes_before_any_new_request` | Partial; bootstrap proves read admission, not mutation or spawn |
| T28 | Reject wrong Session, credential, instance, generation and duplicate | `wrong_hello_fields_never_consume_a_reservation`, `credential_replay_after_eof_never_reconnects` | Partial; member unit + bootstrap; Session routing still required |
| T29 | UI command ownership distinct from accepted Agent execution | `app_server::command`; still Server-local | Open; existing UI ownership tests do not exercise Client supervision |
| T31 | No Client: truthful truncation, zero Server spill/start/wait | Server output projection; no cross-boundary save selection | Open; needs counter-based no-Client fixture |
| T32 | Complete normalized import including displayed prefix | Optional-save contract in ADR | Open; no executable fixture |
| T34 | One selection at threshold; absence or generation change never retargets | Optional-save contract in ADR | Open; no executable fixture |
| T36 | Small result/error/validated End share claim gate, Begin never claims | `app_server::callbacks`; chunked candidate not wired | Open; needs arbitration fixture |
| T37 | Independent budgets and business/capture/readability facts | ADR; budget constants in `rustx-execution-protocol` | Open; finite constants alone insufficient |
| T41 | Seeded local/remote semantic equivalence | UDS only | Open; remote transport/conformance missing |
| T42 | Bounded line/match projection: transferred bytes, client reads, memory, RPCs | `read_range_serves_verified_bytes_at_a_sparse_file_boundary` | Partial; bootstrap proves byte range only, no line/search scaling evidence |
| T44 | Minimal reservation before browsing, no unbound access or unrelated startup | `an_unbound_workspace_argument_never_connects`; `workspace-host.test.ts` proves folder registration schedules no Python; standalone Server test leaves stale preparation handoff untouched | Partial; product picker still needs Session authorization before browsing |
| T54 | Candidate/sequence/offset integrity, late and mixed identities rejected | ADR pending Host and save messages | Open; needs fixtures |
| T55 | Final encoded size including envelope and escaping | `transport::{encode_record,read_record}` frame cap | Partial; member unit framing tests; Host envelope boundary cases missing |

## Measurement record

Before changing selection or buffering algorithms, retain `a24cbf6e6` as the read-only comparison revision. Do not equate selecting a byte range with selecting lines or matches. Use the same release toolchain/profile, host, private roots and file generator for both variants. Record the generator seed, file byte lengths, offsets, UTF-8/escaping distribution and expected selected bytes. Use growing inputs and a fixed selected result, including near EOF. Count bytes at the Client file reader and complete encoded frames in both directions (including prefix), not just decoded result size. Count requests, not syscalls. Record Server and Client peak memory separately, data/control high-water marks and elapsed time per repetition. Report unsupported baseline operations as not comparable. No before/after delta or end-to-end performance result has yet been measured.

## Final-head gate

Record local commands, actual head SHA, Actions checkout SHA, required Linux/macOS jobs, explicit skips and the final `full-verification` / Merge verification URL here before requesting final review. No final-head verification is claimed by this initial ledger.
