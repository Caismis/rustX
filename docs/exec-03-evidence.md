# EXEC-03 implementation and verification

[#578](https://github.com/Caismis/rustX/issues/578) is open. This branch starts at
`5ff54d8327917bca2cdbc9647d7da197afe8beed` on
[`feat/576-execution-boundary`](https://github.com/Caismis/rustX/pull/580),
which includes the merged EXEC-02 PR #598. The issue's pinned research baselines
are rustX `d82d7f9b8d9dd4f5064dba8d377103c8c6049d28`, Codex
`ee344ca9458351a6ea102795fa73110ad3ede5dc`, and Pi
`22fc026b872b4a7df22ffa94e8fa00b873128054`.

## Hosted reception

The App Server admits the four Hosted result messages in v81. The callback
broker binds each candidate to the authenticated connection, its current
subscription, the native invocation and the Host-selected upload ID. The
incremental verifier validates End before using the existing response claim.
The native invocation owns cancellation and the canonical ToolResult.

The encoded message ceiling is 1 MiB. Begin permits up to 1,024 descriptors
and 16 MiB of declared raw text. Each independently validated chunk holds at
most 64 KiB of raw data. The broker holds up to 32 receive and verification
handoff buffers, caps active candidates at four per Host connection and
eight per conversation and per Session, and caps recipient slots at eight per
callback. The existing callback limit is 128 pending requests. Abort and
expiry replace the candidate with a bounded refusal; a new Begin sweeps
expired candidates before allocating.

For a large candidate, preview buffers retain 16 KiB of head and 8 KiB of
tail text at most, before projecting at most 16 KiB for the model. For a
small candidate, its ordered items retain at most 16 KiB of text in total
and up to 1,024 string entries. The verifier also holds up to 1,024
descriptors and lengths, the last chunk's metadata, one digest and pending
UTF-8 bytes. An owned permit covers the buffer through the response claim.
These are payload and object-count bounds. Allocator capacity, transient
base64/UTF-8 decoding and encoded message storage have separate costs.
The Server creates no upload files or per-chunk canonical history.

Hosted validation continues without a Client. The authoritative result
retains its success or failure and includes the original byte count when
truncated. A single lookup at the normalized preview crossing selects the
live Client. The same binding owns all subsequent save exchanges. The Client
checks the owner and chunk before appending, then advances the ledger only
after physical append. End competes independently with ordinary replies.
The winning callback can seal and publish its exact bytes. Losing callbacks
notify their worker to cancel and leave cleanup with the admitted storage
owner. Host claim never takes the operation cancellation gate recursively. The Client retains the file and slot until confirmed removal. An
unconfirmed publish receipt leaves disposal with the binding.

The physical store uses separate capture permits from Bash. Each Client has
16 slots, at most 17 MiB per file and 4,096 operation identities per binding.
Published locators remain owned by the binding for its lifetime. Ranged
logical Read and bounded Grep validate the exact Conversation and locator.
The files live under a private Client directory. The Server can retain up to
four fragments per writer across at most sixteen imports, plus a bounded
prefix and the fragment in flight. Offline cleanup after a killed Client
and same-instance reconnect remain open.

## Workspace instructions

For a bound Session, Workspace AGENTS.md uses the authenticated Client's
same descriptor-relative, stamped read as rustx.toml. The Server retains
the admitted document snapshot and verifies it again before applying the
resource candidate. A missing leaf is accepted only after the Client proves
its Workspace root. Invalid or unavailable instructions fail resource
adoption while independent configuration can still apply. The User
instructions are read by the Server and applied before Workspace
instructions. A bound Workspace watch now triggers the same stamped
reconciliation. The native watcher continues serving Server-owned sources.

## Workspace Skill discovery

An admitted Workspace uses the existing descriptor-relative metadata,
directory and range RPCs for Skill discovery. User collections use the
Server filesystem. Both sources apply the same collection traversal,
ignore rules and frontmatter parser. A package marker ends enumeration;
discovery reads only the frontmatter with 4 KiB of read-ahead. The Client
tree publishes paths that its ordinary Read tool can open. A failed read
of the Workspace collection rejects the candidate and retains the last
adopted snapshot. An absent collection is an empty source.

The Linux mount fixture places the only copy of the Skill and its 2,050
assets in the Client view. Its first model request contains only Skill
metadata and the real path. Native Read brings the body into the third
request, and the ignored package stays absent. The local Skill group had
27 passing tests and the configuration group had 359 passing tests and
two opt-in cases.

## Workspace Agent and Workflow definitions

Bound configuration reads Agent TOML and Workflow YAML through the
admitted Client. The Server uses the same parsers, whole-definition
shadowing, and source-level Workflow authorization as authored discovery.
Each definition is read under the Client's directory-entry stamp. A final
directory observation rejects changes after the file read. The separate
native Managed Python source still has its existing authority check.

The Linux mount fixture has no Server directory at the Workspace path.
Cold load and explicit reconciliation both apply the Client's definitions
and guidance. A hook modifies a Workflow after capture and before final
verification. The next application reports failed capabilities and keeps
the previous Agent and Workflow; invalid higher-scope Agent TOML also keeps
the last valid generation. A second mount fixture gives User, Client and
Server-side decoy Agents different contents. The bound runtime selects the
Client version.

## Bound Workspace source observation

A Session observer registers descriptor watches on its admitted Client. The
plan covers the root documents, fixed resource directories, and the
directories and marker or ignore files seen by Skill discovery. Changes are
hints. The Server captures the current Workspace facts again and validates
the exact Session endpoint before publication. Retirement or overflow
invalidates coverage and triggers recapture. Registration always precedes
the rescan. Watches retain their slots until confirmed stop or binding
settlement.

Each binding budgets 128 slots for configuration watches separately from the
32 public file-observer slots. The watcher uses the existing coalesced
16-path notices. It rechecks registrations at most every five seconds when
coverage is incomplete. A detached binding preserves last-good state. The
same-instance recovery test retires the previous watch before registering a replacement and
rescanning. Until the reserved binding retires, explicit reconciliation and
automatic source tracking select the Client view. The real mount fixture
also puts a repairable decoy at the same Server pathname and verifies its
original bytes survive reconciliation. The same fixture calls the public
Skills catalog and reads two revision-matched instruction pages while the
Server directory is absent. An unbound Session is refused. The Web Console
test changes the Session at the same path and releases the old reply after
the new catalog has arrived.

`client_source_watch_reloads_guidance_and_skill_metadata_after_registration`
uses a real Linux Client in a mount namespace with no Server workspace
directory. It waits for watch registration and the initial rescan, edits
AGENTS.md and the Skill marker, then waits for both values to appear in the
runtime before stopping the observer and reaping the Client.

## Session-authored configuration

`thread/start` accepts optional `session_config` and `session_resources`.
`session/inputsWrite` can replace or clear either field, or validate and
commit both under one settings revision. Unknown or invalid candidates
preserve the last valid state. Settings reads expose only the presence of
the authored fields. Session config is limited to 128 KiB and Session
resources to 512 KiB and 128 definitions. A reference can select an
existing Session's saved config or resources at its exact settings revision
and in the same Workspace. Creation and editing capture the saved bytes
under the catalog lock, then validate them for the target Session.
The target owns the captured input after commit.

The scripted App Server test
`session_inputs_commit_together_and_require_permission_for_a_replaced_workflow`
verifies that a Session definition replaces the Workspace definition and
requires an explicit Session Workflow grant. It then applies the grant and
a named Agent, rejects stale and invalid writes, reopens the catalog to
verify durable inputs, and confirms that clearing the two overrides
reveals the Workspace Workflow. It creates a Session referencing both
fields, then rejects a stale source reference in a joint update without
clearing the target's other field. The fake provider receives no requests.

## MCP declaration cache

A Server process retains at most 32 validated declaration sets for 30
minutes. Its process-private digests cover the isolated Conversation root,
Workspace, source identity, effective transport and resolved credential
inputs. Stdio also requires the current physical admission tuple. The
cache records up to 512 KiB of encoded declarations per entry. It skips
declarations that echo known credentials. Live listing bounds the whole
catalog independently of individual frame limits.

An eligible startup may compose a fresh execution owner with retained
declarations. First use still negotiates and lists against the live peer
under the invocation's cancellation and deadline. A mismatch refuses
dispatch for that frozen declaration. Workflow dependencies and explicitly
selected tool names require live discovery at admission. Source invalidation clears cached metadata and fences an
in-flight observation for the same source. An older observation cannot
overwrite a newer one.

## Client stdio MCP

For a bound Session the Client reads `.agents/mcp.toml` and the Server
validates the captured revision again before applying configuration. A lost
read can leave MCP resources unavailable and retains the last adopted
configuration through the existing application owner. The Server still reads
its User document and resolves the effective credentials before connection.
The Client owns stdio supervision and negotiates the peer's protocol. The
Server uses the original admission data and resumes the SDK request sequence,
then sends and receives through the bound data lane. The protocol ADR records
the memory and transport bounds. Managed Python uses the same framed stdio
owner with an exact Client capture key. The delegated relay assigns a
separate physical identity, admits only frozen launches and authorized tool
names, and retains physical responsibility through Client close or binding
settlement.

The initial MCP-only Linux mount test observed one `server/discover`, one legacy
`initialize`, one `tools/list`, one `tools/call` and two fake-provider
requests. Its 1,100,000-byte text body crossed multiple execution pages and
both markers were visible in the model continuation. The supervisor suite
passed 82 tests, including the new START cut. Strict workspace Clippy and
eight Managed Python tool regressions passed. The latter exercise stdio and
the existing fingerprinted Store and do not measure Client-side preparation.

A separate Linux debug run of the same mount fixture took 2.046 seconds
including startup and teardown. Sampling procfs every 10 ms recorded peak
RSS of 185,232 KiB for the App Server and 35,008 KiB for the Execution Client.
The sample excludes the provider fixture and supervised MCP child from those
two process totals. Whole-lane traffic, queue occupancy and concurrent
session measurements remain open.

A separate trace counted 35 stdio exchanges through the execution data lane,
with 10,392 bytes sent to Client and 1,477,567 bytes returned to Server,
including the four-byte frame lengths. Eighteen replies carried data pages.
The counts exclude Save, handshake and other control exchanges. Idle polling
depends on the scheduler and is part of the observed total.

`child_relay_reaps_its_running_stdio_session_on_the_original_client` observes
a real subprocess on a separate socket, retires its relay, verifies EOF
after physical close, then reads exact bytes on the original parent binding.
`child_stdio_uses_parent_identity_and_keeps_unconfirmed_cleanup` checks the
independent physical identity, immutable launch and tool authorization, and
that an unconfirmed Close keeps ownership. The complete Agent Loop and
Workflow ordering tests remain separate acceptance requirements.

In the isolated mount run, the real App Server's child fixture discovers
its Agent definition only in the Client view. The child model receives
Write, Edit, Read and Bash results in tool order. The logical Server path
has neither the definition nor the file created by the child. Shutdown
waits for its Workspace Python probe to stop. This covers the real native
child path independently of the stdio relay fixture.

## Deterministic coverage

| Required tests | Evidence and open coverage |
| --- | --- |
| T01, T04, T06 | The real WebSocket Host fixture submits a Root tool call with no Execution endpoint and asserts two model requests and one canonical ToolResult. The standalone process fixture serves HTTP MCP from Server loopback and verifies one tool commit and a last terminal. The child relay tests cover exact physical stdio routing. `shipped_parallel_reverses_completion_without_git_or_parent_orchestration` exercises reverse completion in a real Workflow. `fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call` verifies rooted interaction and one canonical outer result. The isolated Root/Subagent/Workflow composition below checks the shared Session and exact provider sequence. Reverse completion and its canonical ordering are exercised in the separate Workflow fixture. |
| T13, T14, T15, T17, T18 | The Session input RPC covers definition shadowing, explicit permission, joint CAS, stale and invalid candidates, durable readback, and clearing overrides. Isolated cold load covers Client sources. The complete bootstrap and concurrent-activation composition remain open. |
| T16 | `primary_model_selection_resets_primary_overrides_and_preserves_summary_policy` and `attempt_started_freezes_the_model_across_a_mid_attempt_switch` cover the Runtime Client entry. `r04_manager_selection_and_attempt_admission_have_both_complete_orders` exercises admission races through the App Server. `t05_t09_workflow_only_agent_model_rebuilds_frozen_execution` and `direct_model_text_and_json_have_one_http_call_and_one_canonical_outer_result` exercise frozen Workflow profiles and the direct-model provider. A common acceptance trace across UI and delegated entry points remains open. |
| T19, T20, T21, T30, T38 | The Root mount-view stdio fixture serves a tool using declarations and a program visible only to the Client. It exercises protocol negotiation, ordered tool discovery and a foreground call whose output needs multiple execution frames and a published continuation. The child relay has physical cleanup and authorization proofs. Cache tests exercise four boundedness and identity conditions, plus a real held HTTP catalog, and verify that required live discovery bypasses reuse. Full delegated composition and composed stdio Tasks and interactions remain open. |
| T22, T40, T47 | Isolated Workspace tests cover Client-owned AGENTS.md, Agent TOML and Workflow YAML on cold load and explicit reconciliation. The verification hook rejects a changed in-flight capture and preserves the last valid generation. The independent mount fixture covers Skill metadata discovery and actual Native Read through the same Client without enumerating package assets. The Linux watch fixture verifies automatic guidance and Skill metadata updates after physical registration. Overflow and retired-watch races across reconnect still require composed proof. |
| T31, T32, T33, T45 | Hosted and HTTP fixtures cover absent Client, bounded model output, no locator and no Client startup. The wire-level Save fixture reads and searches the complete published text through an authenticated Client and checks cross-Conversation refusal. The Hosted fixture observes both physical candidates before End and the winning file after loser removal. Native Read retrieves the winning prefix, Native Grep finds the middle marker on a bounded line, and a different Conversation on the same binding is refused. The multichunk image fixture reads the independently delivered artifact after reaping the Client. Real Client refusal, cancellation after Seal, and late publication across replacement each preserve the already validated result. |
| T36, T50 | An earlier Begin leaves room for a second Host to win. Broker tests run verified End and ordinary replies/errors in both orders, and reject a stale subscription. Existing domain-invalid response handling consumes the claim. |
| T37, T46, T52, T55 | The WebSocket fixture submits a result above 1 MiB with and without a Client. The encoded framing regression measures the complete record inclusive of escaping. The HTTP adapter caps response bytes before decoding, and the process fixture checks oversized responses do not replay the effectful call. Per-process occupancy remains open. |
| T51, T53 | Incomplete and digest-invalid End attempts leave the callback open for another eligible response. The real Client checks the retained file length on each append. A fixture changes that length after a confirmed prefix, then verifies the winning business result stays successful with no locator and the private file is removed. The real Client confirms both native disk exhaustion and cancellation after a physical Seal, with no publication or business replay in either case. |
| T54, T56, T57 | Duplicate, stale and foreign steps, Abort and replacement are checked. Virtual time proves fixed total and idle expiry. Saturation checks another Host can claim and Abort returns capacity. The Client unlink-failure regression checks quota remains occupied until a confirmed removal. In `stream_save_saturated`, the Client creates an empty physical candidate, the proxy holds its Begin reply, and five fragments exceed the four-slot Save queue. All Host receive acknowledgements and real Client Health still complete. Releasing the gate delivers exactly one correlated Abort with confirmed removal and no Chunk, Seal or Publish. The validated End produces one successful business result with unavailable storage. The lost-receipt case below composes a real Host publication, late receipt loss and same-instance recovery. A separate paired-Client test refuses the retired generation’s Save key on a replacement. The standalone Linux case below tests a real local Client generation change and a late publication receipt. |
| T71, T72, T73 | A real HTTP fixture holds an old lease across activation with identical tool schemas. New and old leases send their respective credentials. A failed candidate preserves the previous version and the old connection closes after its last lease. Concurrent activation and cancellation across the full Session stack remain open. |

## HTTP MCP and managed output

The existing rmcp streamable HTTP transport stays on the Server. It bounds
SSE events at 16 MiB, runs up to 16 ordinary POSTs per transport, and retains
a separate control POST with a five-second timeout. The pinned reqwest
adapter independently caps successful JSON bodies at 16 MiB and error bodies
at 64 KiB. A declared excess is rejected before reading. A chunked excess is rejected before extending
the bounded accumulation. The decoder may hold
the response bytes alongside the parsed result and its allocations. Sixteen
ordinary requests could therefore buffer up to 256 MiB of JSON bodies per
transport in addition to decode and transport overhead. These bounds cover
retained body bytes.

An empty exact source selection carries no live-discovery demand. The
process-level startup tests keep explicitly disabled Python sources
Unprepared, including an unusable Store and a malformed package, while native
tools remain available.

For a valid HTTP business result, text blocks are scrubbed using the
credential snapshot of the connection that performed the call, then added in
order to the bounded preview and, after the spill threshold, to the same
Client writer as Hosted. The importer excludes image Base64, raw envelopes
and structured data. Tasks use the same translation when obtaining their
final result. A transport refusal after dispatch preserves the unknown
business outcome; the caller does not repeat the underlying call.

## Linux fixture measurements

The two-Host fixture sent 1,200,053 raw winning text bytes and persisted
1,200,056 normalized bytes. The concurrently staged loser owned 65,558
physical bytes before End. Its file was removed by the original binding.
The fixture asserts two model requests and one canonical ToolResult for
each case.

| Scenario | Host to Server WebSocket bytes | Server to Host WebSocket bytes | Save RPCs | Server to Client save bytes | Client to Server save bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| No Client | 1,625,151 | 414,229 | 0 | 0 | 0 |
| Successful save with staged loser | 1,713,887 | 409,406 | 28 | 1,701,162 | 12,182 |
| Confirmed staging followed by storage refusal | 1,625,151 | 414,754 | 5 | 177,030 | 1,695 |

These were Linux debug runs of
`three_clients_broadcast_first_response_error_and_invalid_payload_commit_once`
on `d47c095f75358d7bb60ff464c77dd1865f3cc1c4`.
The WebSocket counts include handshake, setup and notifications over the
three attachment sockets. The Unix measurement includes typed Save records
and their length prefixes. It excludes handshake, health and revocation.
The successful save includes two Begin, 23 Chunk, one Seal, one Publish and
one loser Abort. The refusal includes the failed append, the cleanup reply
and zero publication requests.

The same Host fixture also forwards a real Save Publish and captures the
Client's Published reply before closing both lanes. The Host observes one
canonical successful business result with unavailable storage and one model
continuation. Only after terminal publication does the original Client
reattach. The proxy checks the exact prior Workspace observation identity
receives Accepted on Release and observes zero Save requests on the recovered
lane. The original 1,200,056-byte publication remains private until the real
Client acknowledges Revoke and exits.

The disk-exhaustion variant mounts an independent 8 KiB tmpfs in the real
Client. A kernel probe reaches ENOSPC before starting the Client. The wire
proxy confirms one candidate allocation, a real first write refusal, one
confirmed Abort, and zero Seal or Publish requests. Host validation still
commits one successful bounded result with unavailable storage and no retry.
The cancellation variant waits for validated End and the Client's physical
Seal, withholds the receipt, and cancels the originating Attempt. The Client
acknowledges removal of both the losing and winning candidates. No Publish
is sent. Exactly one successful business ToolResult commits with an
unavailable continuation, and the fake provider sees exactly one request.

`a_published_host_output_cannot_cross_a_local_client_generation` starts the
standalone App Server with its local Client supervisor. A proxy holds the
original Client’s actual Publish receipt after confirming the physical file
length. SIGKILL of the original namespace init forces replacement. The exact
private storage is gone by generation 2 admission. The held receipt is
released against the old lane after canonical completion. The replacement
reads the original Workspace and receives zero Save requests. The durable
result counts one successful Host ToolResult with an unavailable locator,
and the provider counts one tool call and one bounded model continuation.

In a separate untraced run, sampling procfs every 10 ms recorded 204,564 KiB
peak RSS for the entire Host harness and its in-process Server, with 3.35 s
user and 0.15 s system CPU. The two Client children peaked at 22,784 and
22,836 KiB respectively. The full six-case fixture took 9.114 s including
startup and teardown. The Host harness memory includes test peers and
payload generation. Installed-product latency, Linux standalone Hosted peak
RSS, internal queue high-water and profiling across multiple concurrent
Sessions remain open.

The standalone HTTP process fixture reused the same measure for a 327,680-byte
result. Its App Server peaked at 188,688 KiB (0.79 s user and 0.03 s system
CPU), its Client at 22,868 KiB, and whole-fixture time was 1.982 s. The
oversized JSON case peaked at 185,724 KiB in App Server, required one remote
call and completed in 2.146 s with zero Client launches. These are individual
debug fixture observations. The HTTP service and provider run in the
separate fixture process. Startup, VCS and SQLite state are included.

## Shared Session routing and foreground Managed MCP

`a_real_child_executes_native_tools_in_the_frozen_nested_workspace` mounts an
independent Client Workspace with authored Agent and Workflow files absent
from the Server view. Root Read and Workflow Read observe distinct Client
markers. The delegated child writes, edits, reads and executes Bash through
the same Session's binding. The fake provider observes four initial Root
requests, five child requests and exactly one typed child-completion handoff
on Root. The Workflow adds no model request. All four child tool results
reach the provider in the original order. The fake uv subprocess holds a
Unix socket, whose peer credentials identify the process in the Linux host
PID namespace. Normal drain closes the socket
and confirms the subprocess and Client have exited.

`a_real_managed_fastmcp_task_completes_through_one_tool_result` and
`a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction` now
exercise the admitted Client's venv and physical stdio connection. The Task
case observes `2026-07-28`, exact one execution marker and one terminal tool
result. The MRTR case observes two physical call rounds and one Interaction
request/settlement pair, with the exact human answer in the final result.
Both use the native close path and await a clean Client exit. The other
resource and cancellation combinations remain individually mapped below.

## Local Linux verification

- Formatting, workspace check and strict workspace Clippy passed.
- The callback broker and upload request-ID filters, real WebSocket
  conformance, real HTTP import, no-Client HTTP overflow, and HTTP over-limit
  no-replay tests passed. The isolated mount proof and committed artifact
  read after Client exit passed.
- Existing focused regressions passed for model change resets, ordinary MCP
  lease retirement, credential changes invalidating draft preparation, and
  refusal of obsolete MCP background policy. These cover their existing
  owners. Cross-entry model integration and credential rotation through the
  full Session stack remain open.
- The MCP adapter suite passed 162 tests. A non-foreground binding is
  rejected before the real stdio program can write a start marker. The
  adapter's legacy Server-local MCP output sink has been removed. An
  unknown result without a correlated reply retains its status and carries
  no locator.
- Browser acceptance ran 42 targeted cases across pinned Linux browser
  runs. Workspace drafts and read-only catalog tests used real HTTP MCP.
  The two desktop/mobile switch regressions observe both cancellation of
  the held POST and deletion of its HTTP session. Root and child execution
  tests used the real bound Client for stdio. The two resource-authoring
  cases passed their separate rerun after editing the HTTP URL field.
- The real Execution Client saved and read normalized output through the
  wire. The execution boundary crates passed 412 tests with three ignored,
  including a confirmed unlink retry. The pinned rmcp fork passed two
  HTTP response-cap regressions.
- The generated protocol check and type check passed. Dev type check and
  44 tests passed. TUI type check and 1,365 tests passed with one skip.
  Web Console type check, build and 5,018 tests passed with four skips.
- `git diff --check` passed.

Final FULL and Merge verification, native macOS, whole-frame HTTP parser
occupancy and the complete Browser acceptance matrix remain open.
