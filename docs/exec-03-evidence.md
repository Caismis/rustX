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
prefix and the fragment in flight. The saturation fixture observes all
four queue slots occupied while Health progresses. The restart and
same-instance tests below cover physical scratch cleanup and receipt loss.

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

The Linux mount fixture places the only copy of the Skill, its reference
and its script alongside 2,050 decoy assets in the Client view. It follows
the linked collection, package and marker only within that Workspace.
An external link is skipped and a hidden non-UTF-8 name consumes one scan
slot without reaching the Server. Its first
model request contains the metadata and real Skill path. Native Read brings
the instructions into the third request, then reads the reference while
Native Bash runs the script. The fourth request contains both results. Only
the Client mount contains the script's single start marker, and the ignored
package stays absent. The local Skill group had 27 passing tests and the
configuration group had 359 passing tests and two opt-in cases.

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
Client version. A reconciliation regression removes and replaces the
Server path while the Client applies two distinct policy and Agent
revisions. An atomic swap of the Client's physical Agent directory for a
file then causes only the capabilities unit to fail. The validated scalar
policy still advances, and the last admitted Agent remains active until
the Client's directory is restored.

The existing real Server SIGKILL regression now checks configuration
authority on restart. A sibling Session remains admitted and can read the
original Session's cold history and settings. The original Session's
cold attach and input write are refused while its remote Client claim
remains outstanding. The settings revision and the Server-local decoy
document still match the inputs to the refused operations.

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

The bound runtime can reconcile again while the same Server pathname names
an unrelated file. The physical Workspace authority remains with the
original Client descriptor. Explicit Server authoring verifies its root
inode across capture. A prepared draft revalidates its own root before
transfer.

`client_source_watch_reloads_guidance_and_skill_metadata_after_registration`
uses a real Linux Client in a mount namespace with no Server workspace
directory. It waits for watch registration and the initial rescan, edits
AGENTS.md and the Skill marker, then waits for both values to appear in the
runtime. It retargets only an intermediate link in a separate directory,
then modifies only the resolved target of a linked ignore file. The live
catalog follows the retarget and removes and restores the ignored Skill.
The test also retargets to a missing path and creates the target under a
separate directory. The Skill returns without an edit to either link.
It next parks the old watch after physical capture, severs the old
pair, and changes both sources while detached. The same Client supplies a
new pair and the runtime applies the new values. Releasing the old capture
leaves the new values intact, without any model request. Drain reaps the
Client.

Saved-output byte paging uses the same admitted locator and Conversation
as line reads. The real Native Read fixture imports a single long line
with a four-byte UTF-8 character split by the first range boundary. The
first result provides the next valid offset, the second returns the whole
character and remaining bytes, and a caller pointing inside the character
receives a typed failure. The paired wire fixture rejects a second
Conversation and matches Grep by the published filename. The execution
handshake refuses revision 5 on both lanes before business dispatch.

The Workflow editor can list an authored HTTP source for an unclaimed
Workspace, including when an unbound Session is selected. The authoring
catalog starts no stdio or Managed Python process in that state. A reserved
Workspace requires its exact Session and pins that Client for capture and
listing. A different Session, an unavailable Client document or an
outstanding recovery record prevents using the Server's Workspace view.
The recovery check observes the original claim without consuming a receipt.

The same admission applies to the MCP diagnostics. The
`workspace_mcp_management_requires_its_binding_only_after_reservation`
regression queries both public RPCs against a real HTTP peer, rejects a
Session from another Workspace, and parks a third list request while the
Workspace is reserved. Releasing the peer's response cannot make the old
authored result current. Subsequent queries leave the exact list count at
three and the business invocation count at zero.

An explicit Doctor plan verifies the authored Workspace directory identity
before opening a Client or probing a source. A same-path replacement fails
that plan before a process starts. A queued draft independently checks its
physical identity at transfer. A stale draft supplies no prepared runtime
to the new Session. Its ordinary first preparation still connects to the
admitted HTTP source once when the Workspace is a valid directory.

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

The physical stdio owner retains the negotiated request context when
`server/discover` selects an older supported MCP revision. Subsequent
requests carry that version and retain their existing progress tokens.
The credential catalog test runs tools, resources and templates through a
real bound Client, checks that the peer receives literal and referenced
credentials, and verifies that native inventory redacts the echoed fields.

Three MCP boundary tests now drive a real Client-owned stdio session.
The MRTR test observes exactly two `tools/call` requests and the typed
answer before the remote task is created. The task input test checks one
Questionnaire and one `tasks/update`. The cancellation test holds the
second poll before dispatch, observes one physical `tasks/get` and one
cooperative `tasks/cancel`, and retains the unknown outcome. Each waits
for the physical stdio close before reaping its paired Client.

The finite MCP management route can also select an exact Session. The
mounted Workspace regression reads its Client-only MCP definition and
observes one HTTP catalog request from Server, five declarations and zero
business calls. The real Client stdio catalog keeps both resource pages
on one connection and waits for its independently observed process
to close. A mixed HTTP and stdio draft opens no process or speculative
connection before Session admission. When a bound Session replaces a
prepared Server-authoring draft, the draft settles before ordinary source
admission. The two connection generations each list once.

The initial MCP-only Linux mount test observed one `server/discover`, one legacy
`initialize`, one `tools/list`, one `tools/call` and two fake-provider
requests. Its 1,100,000-byte text body crossed multiple execution pages and
both markers were visible in the model continuation. The supervisor suite
passed 82 tests, including the new START cut. Strict workspace Clippy and
eight Managed Python tool regressions passed. The latter exercise stdio and
the existing fingerprinted Store and do not measure Client-side preparation.

The Client resolves a Workspace-authored relative command from the
document's source base. A contained sibling path may reach an executable
linked within the admitted Workspace. The process fixture uses a sibling
program and a Client-only link, and the physical unit test rejects links
and relative commands that leave the Workspace. User-authored relative
commands and cwd are interpreted by the admitted Client. Absolute system
executables and PATH discovery retain their Client-side meaning.

A real rotation fixture opens eight Client-owned stdio peers, admits an
old lease, then replaces all eight with the same model-facing schemas.
Sixteen distinct physical starts coexist under the allocation bound.
The old lease can reach its original eight peers until it is dropped.
Retirement physically closes those peers and a ninth demanded source is
refused before starting a process or replacing the adopted snapshot.

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
| T01, T04, T06 | The real WebSocket Host fixture submits a Root tool call with no Execution endpoint and asserts two model requests and one canonical ToolResult. The standalone process fixture serves HTTP MCP from Server loopback and verifies one tool commit and a last terminal. The child relay tests cover exact physical stdio routing. `shipped_parallel_reverses_completion_without_git_or_parent_orchestration` exercises reverse completion in a real Workflow. `fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call` verifies rooted interaction and one canonical outer result. The isolated Root/Subagent/Workflow composition below checks the shared Session and exact provider sequence. `client_bash_reversed_settlement_keeps_canonical_order` holds the first real Bash behind a socket until the second settles. Both start exactly once, two model requests occur, and the durable results retain the original order before the sole final Attempt terminal. |
| T13, T14, T15, T17, T18 | The Session input RPC covers definition shadowing, explicit permission, joint CAS, stale and invalid candidates, durable readback, and clearing overrides. With a real Client admitted, the fake provider holds an Attempt while a joint config and Workflow replacement commits. The capability unit is ready while the admitted catalog remains frozen. The exact next two provider requests offer the old and new Workflow descriptions in order. Isolated cold load covers Client sources, and the real restart test preserves cold history and rejects attach and input validation against the Server decoy while the former Client claim is outstanding. |
| T16 | `primary_model_selection_resets_primary_overrides_and_preserves_summary_policy` and `attempt_started_freezes_the_model_across_a_mid_attempt_switch` cover the Runtime Client entry. `r04_manager_selection_and_attempt_admission_have_both_complete_orders` exercises admission races through the App Server. `t05_t09_workflow_only_agent_model_rebuilds_frozen_execution` and `direct_model_text_and_json_have_one_http_call_and_one_canonical_outer_result` exercise frozen Workflow profiles and the direct-model provider. The pinned browser holds the first provider stream across a model change and reload, then verifies the new model's default profile and output budget on the second request. The native Subagent tests run parent and child processes against one provider and assert the inherited invocation and an explicit child-model change, including reasoning, temperature, seed and budget. Each native trace contains three unchanged parent requests and one child request. Each entry uses its own fixture. |
| T19, T20, T21, T30, T38 | The Root mount-view stdio fixture serves a tool using declarations and a program visible only to the Client. It exercises protocol negotiation, ordered tool discovery and a foreground call whose output needs multiple execution frames and a published continuation. The child relay has physical cleanup and authorization proofs. Cache tests exercise four boundedness and identity conditions, plus a real held HTTP catalog, and verify that required live discovery bypasses reuse. The real Client-owned Managed Python Store fixtures below cover foreground Tasks and MRTR. The bound management inventory uses one physical stdio process across both resource pages and waits for its witnessed exit. The independent HTTP probe requires zero business calls. |
| T22, T40, T47 | Isolated Workspace tests cover Client-owned AGENTS.md, Agent TOML and Workflow YAML on cold load and explicit reconciliation. The verification hook rejects a changed in-flight capture and preserves the last valid generation. The independent mount fixture covers Skill metadata discovery and actual Native Read through the same Client without enumerating package assets. The Linux watch fixture verifies automatic guidance and Skill metadata updates after physical registration and rejects a retired watch's captured result across authenticated reattachment. Native watch overflow and the reattachment path have separate proofs. |
| T31, T32, T33, T45 | Hosted and HTTP fixtures cover absent Client, bounded model output, no locator and no Client startup. The wire-level Save fixture reads and searches the complete published text through an authenticated Client and checks cross-Conversation refusal. The Hosted fixture observes both physical candidates before End and the winning file after loser removal. Native Read retrieves the winning prefix, Native Grep finds the middle marker on a bounded line, and a different Conversation on the same binding is refused. The multichunk image fixture reads the independently delivered artifact after reaping the Client. Real Client refusal, cancellation after Seal, and late publication across replacement each preserve the already validated result. |
| T36, T50 | An earlier Begin leaves room for a second Host to win. Broker tests run verified End and ordinary replies/errors in both orders, and reject a stale subscription. Existing domain-invalid response handling consumes the claim. |
| T37, T46, T52, T55 | The WebSocket fixture submits a result above 1 MiB with and without a Client. The encoded framing regression measures the complete record inclusive of escaping. HTTP process fixtures pass a near-limit response and refuse an oversized one. Each settles one call and one canonical result, with zero Client starts or Server Spill in the unbound cases. The process samples below record RSS and encoded body bytes with up to four independent HTTP responses held concurrently. |
| T51, T53 | Incomplete and digest-invalid End attempts leave the callback open for another eligible response. The real Client checks the retained file length on each append. A fixture changes that length after a confirmed prefix, then verifies the winning business result stays successful with no locator and the private file is removed. The real Client confirms both native disk exhaustion and cancellation after a physical Seal, with no publication or business replay in either case. |
| T54, T56, T57 | Duplicate, stale and foreign steps, Abort and replacement are checked. Virtual time proves fixed total and idle expiry. Saturation checks another Host can claim and Abort returns capacity. The Client unlink-failure regression checks quota remains occupied until a confirmed removal. In `stream_save_saturated`, the Client creates an empty physical candidate, the proxy holds its Begin reply, and five fragments exceed the four-slot Save queue. All Host receive acknowledgements and real Client Health still complete. Releasing the gate delivers exactly one correlated Abort with confirmed removal and no Chunk, Seal or Publish. The validated End produces one successful business result with unavailable storage. The lost-receipt case below composes a real Host publication, late receipt loss and same-instance recovery. A separate paired-Client test refuses the retired generation’s Save key on a replacement. The standalone Linux case below tests a real local Client generation change and a late publication receipt. |
| T71, T72, T73 | A real HTTP fixture holds an old lease across activation with identical tool schemas. A failed candidate preserves the previous version. `bound_credential_rotation_preserves_the_admitted_attempt_and_closes_after_last_lease` admits the original Attempt, captures a changed Workspace document through the Client and prepares it while the provider reply is held. The original Attempt makes one call with its original credential, and the next makes one with the rotated credential. The HTTP peer observes the original session's successful DELETE after its last lease. Four fake-provider requests contain both original business results and neither credential. This test covers ordinary rotation. Cancellation and remote refusal are exercised by the separate transport tests. |

## HTTP MCP and managed output

The existing rmcp streamable HTTP transport stays on the Server. It bounds
SSE events at 16 MiB and runs up to 16 ordinary POSTs per transport. The
Server shares 32 ordinary POST receive/decode slots across all transports.
A slot covers JSON buffering and decoding or ends when the SDK hands off
an SSE stream. The separate control POST keeps its five-second timeout. The
pinned reqwest adapter caps successful JSON bodies at 16 MiB and error bodies
at 64 KiB. A declared excess is rejected before reading. A chunked excess is
rejected before extending the bounded accumulation. The decoder may hold the
response bytes alongside the parsed result and its allocations. At
most 512 MiB of JSON body bytes can be in these ordinary decoders across
the Server, plus parsed output and transport overhead. SSE response bodies
use two shared pools. Up to 128 POST responses can carry progress and
results, including the regression batch of 64 held requests. Up to 32 GET
bodies cover subscriptions and Last-Event-ID resumes. Each parser has the
16 MiB event bound, for up to 2,560 MiB of encoded event payload across
the two pools, plus parser and result allocations. Header classification
precedes body admission, so a GET that declines SSE with 405 always
reaches the caller.
Each pool refuses excess bodies before event parsing and returns slots at
EOF or drop. JSON receives and control traffic use their own budgets.

Three real Linux Server-only fixtures ran against the same HTTP implementation.
Procfs was sampled every 5 ms, excluding the separate `rustx init` enrollment
process from the Server RSS and CPU columns. These are debug fixture timings
including startup and settlement. Each fixture observed one business call,
two provider requests, one canonical ToolResult at most 64 KiB, one terminal,
zero Client starts and no Server Spill.

| Input text | Server peak RSS | Server user/system CPU observed | Fixture wall time |
| --- | ---: | ---: | ---: |
| 16,385 bytes, accepted | 190,088 KiB | 0.70 / 0.05 s | 0.991 s |
| 16,773,120 bytes, accepted | 224,160 KiB | 1.67 / 0.08 s | 2.658 s |
| 16,777,217 bytes, refused | 190,796 KiB | 0.69 / 0.05 s | 4.799 s |

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
The private save matrix also kills the real Client after allocation,
after its last accepted write and after sealing. None has an authoritative
publication, and each produces one successful bounded business result with
unavailable storage. A separate cut kills the Client after canonical
commit. The historical Complete receipt is preserved while the old
physical directory is removed. Neither the replacement nor late old
receipts can turn any of these into another Save or ToolResult.

On `010da0b9c`, a Linux debug run of that standalone scenario took 2.831 s
including enrollment and fixture teardown. The active App Server peaked at
175,092 KiB, with 0.61 s user and 0.05 s system CPU. The original Client
peaked at 25,324 KiB and the replacement at 23,384 KiB. Both Clients used
under 0.06 s sampled CPU. A separate enrollment Server peaked at 121,292
KiB. The 10 ms procfs samples report individual process high-water marks.
These figures describe replacement and a single Host. The earlier
three-Host run has a different workload and in-process Server.

In a separate untraced run, sampling procfs every 10 ms recorded 204,564 KiB
peak RSS for the entire Host harness and its in-process Server, with 3.35 s
user and 0.15 s system CPU. The two Client children peaked at 22,784 and
22,836 KiB respectively. The full six-case fixture took 9.114 s including
startup and teardown. The Host harness memory includes test peers and
payload generation. Installed-product latency, Linux standalone Hosted peak
RSS, internal queue high-water and profiling across multiple concurrent
Sessions remain open.

A live HTTP test shares one response slot between independent client
owners. One POST holds a partial JSON body, a waiting request is cancelled
before reaching the peer, and a control notification succeeds during the
hold. Completing the JSON lets the next POST enter. Its SSE handoff
returns the slot while the response stream stays live. The peer observes
the exact Last-Event-ID on GET while the POST stream occupies its own pool.
A second GET and a second POST each refuse at their own cap. The 405
receipt, a JSON POST and control work remain available. EOF and drop
return their respective slots.

The 64-call manual-clock regression retains all real POST streams while
every progress notification reaches its own request before subscription.
All 64 calls receive both progress occurrences and settle once at the
hard deadline. The original assertions and deadline policy are preserved.

The Linux debug Server built at `83fb1b117` also ran separate Sessions in
one process with its binary staged alone. A loopback peer flushed 12 MiB
of each JSON body and waited at a gate until every response reached that
cut. The peer then completed each near-limit response. The one-Session
run is the baseline for this sample. Body sizes include the encoded JSON
envelope. RSS and CPU came from procfs.

| Sessions | Complete HTTP body bytes | RSS before / held / peak (KiB) | Initialize and attach / all prefixes / finish after release (s) | User / system CPU after attachment (s) |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 16,773,193 | 183,040 / 219,672 / 242,156 | 1.018 / 0.472 / 0.822 | 1.89 / 0.06 |
| 2 | 33,546,386 | 187,768 / 238,348 / 283,684 | 1.924 / 0.478 / 0.842 | 2.77 / 0.11 |
| 4 | 67,092,772 | 194,028 / 274,548 / 362,912 | 6.208 / 0.507 / 1.199 | 6.04 / 0.21 |

Each Session made one MCP business call, two business requests to the fake
provider and one auxiliary provider request. Each ledger recorded one
ToolResult of 17,022–17,023 bytes followed by one terminal event. Journal
event JSON totaled 26,770–26,776 bytes per Session. The Server scratch
directory stayed empty. These debug measurements cover process-wide memory
with up to four simultaneous bodies.

The Linux release Server from the same source repeated the workload with
the same body byte counts, one commit and one terminal per Session.

| Sessions | Release RSS before / held / peak (KiB) | Initialize and attach / all prefixes / finish after release (s) | User / system CPU after attachment (s) |
| ---: | ---: | ---: | ---: |
| 1 | 66,200 / 99,664 / 120,776 | 0.171 / 0.115 / 0.096 | 0.24 / 0.08 |
| 2 | 70,940 / 123,424 / 166,832 | 0.312 / 0.129 / 0.147 | 0.49 / 0.20 |
| 4 | 76,052 / 156,908 / 241,164 | 0.637 / 0.104 / 0.168 | 0.66 / 0.25 |

The independently staged Linux release Server at `e92dba4c3` also ran
16 and 32 Sessions. The fixture acknowledged each turn admission, then
waited for every peer to flush its 12 MiB prefix before releasing the
remaining bytes. At 32 Sessions, all 32 JSON receive slots held incomplete
bodies concurrently and the peer had flushed 402,653,184 bytes.

| Sessions | Complete HTTP body bytes | RSS before / held / peak (KiB) | Initialize and attach / all prefixes / finish after release (s) | User / system CPU after attachment (s) |
| ---: | ---: | ---: | ---: | ---: |
| 16 | 268,371,088 | 104,060 / 349,096 / 736,200 | 6.136 / 4.907 / 0.304 | 1.87 / 0.83 |
| 32 | 536,742,176 | 159,572 / 631,784 / 1,354,124 | 18.199 / 3.035 / 0.574 | 4.32 / 3.32 |

The 32-Session run observed one business call and two fake business
provider requests per Session. Each ledger contained one 17,022–17,023-byte
ToolResult and one final terminal, with 26,767–26,780 bytes of Journal
events. The Server scratch stayed empty and no Execution Client ran.
RSS includes decoded results and allocator overhead for the entire Server.
Concurrent local compilation and the different turn-admission sequence
limit latency comparisons with the smaller runs.

The debug and release observations used different build profiles under
concurrent host load. Compare within each profile's sample.

The revision-six SSE receiver was measured separately with one incomplete
event per Session and a Server binary staged by itself. Each peer flushed
12 MiB of event text before a shared release gate, then sent the rest of
one near-limit event. The final column measures procfs CPU after attachment.
The measured Server SHA-256 was
`2f19ea96e128b3458cea90ca2da3a7aa9843cec6ec6d3364fccf5b2004055a4e`.

| Sessions and occupied POST SSE slots | Complete HTTP event-body bytes | RSS before / held / peak (KiB) | Attach / all prefixes / finish after release (s) | User / system CPU (s) |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 16,773,212 | 58,868 / 106,484 / 147,072 | 1.105 / 0.593 / 0.313 | 0.47 / 0.39 |
| 32 | 536,742,784 | 160,572 / 1,429,432 / 2,404,476 | 24.479 / 2.040 / 0.957 | 21.58 / 6.77 |

At 32 Sessions the peer held 402,653,184 text bytes across 32 SSE events.
Each Session issued one MCP invocation, two fake-provider business
requests and one auxiliary request. Each Journal had one ToolResult of
17,022–17,023 bytes, one final terminal and 26,770–26,783 event bytes.
Server scratch stayed empty. RSS includes parser and result decoding
allocations. Concurrent compilation and the different HTTP framing limit
comparisons with the JSON runs. The recorded samples isolate JSON and SSE
in separate runs.

The staged release Server at `2f307cb83d39a5e093e09504cb79284c468ec0b4`
filled all 128 POST SSE slots from independent Sessions on four WebSocket
connections. Each event held a 3 MiB prefix until all 128 peers reached
the gate, then completed a body just under 4 MiB. The Server received
536,358,400 encoded bytes, with 402,653,184 text bytes flushed before
release. RSS rose from 451,424 KiB after attachment to 1,436,428 KiB at
the gate and peaked at 1,973,096 KiB. Startup and attachment took
20.479 s, the last prefix reached the gate after 2.401 s, and terminal
settlement took 1.615 s after release. Procfs recorded 15.02 s user CPU
and 10.33 s system CPU after attachment.

Each of the 128 ledgers held one canonical tool commit and one terminal.
The peer counted 128 MCP calls, 256 fake-provider business requests and
128 auxiliary requests. Server scratch stayed empty. The 32-stream
sample used a different per-event size and root revision. This sample
verifies full POST parser occupancy. The independent subscription test
above verifies the separate GET quota and resume handling.

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
The fixture adds its own mount wrapper around the Client and uses the
Client's private mutation staging inside that mount. The production
guardian authorizes access to its staging socket by the supervised
Client's actual PID.

`a_real_managed_fastmcp_task_completes_through_one_tool_result` and
`a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction` now
exercise the admitted Client's venv and physical stdio connection. The Task
case observes `2026-07-28`, exact one execution marker and one terminal tool
result. The MRTR case observes two physical call rounds and one Interaction
request/settlement pair, with the exact human answer in the final result.
Both use the native close path and await a clean Client exit. The other
resource and cancellation combinations remain individually mapped below.

## Local Linux verification

- The `007d77cd8` feature slice passed 10 focused native regressions,
  including the exact-Session MCP paths, original stdio process close,
  ordinary and deferred draft ownership, and reverse Bash settlement.
  Web Console type checking and 77 focused tests passed.
- Formatting, workspace check and strict workspace Clippy passed on the
  earlier revisions listed with their evidence.
- The callback broker and upload request-ID filters, real WebSocket
  conformance, real HTTP import, no-Client HTTP overflow, and HTTP over-limit
  no-replay tests passed. The isolated mount proof and committed artifact
  read after Client exit passed.
- Focused regressions passed for model change resets, ordinary MCP lease
  retirement, credential changes invalidating draft preparation and refusal
  of obsolete MCP background policy. The bound Session rotation test adds
  four real requests to the fake provider, two MCP calls and successful
  close receipts from the HTTP peer. The model-selection entry points have
  individual request traces as recorded under T16.
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
  The 24-file upload case waits for the distinct native readiness receipts
  before checking the exact DOM count, destination bytes and zero model
  requests. The ten Workflow canvas cases also passed in the pinned
  browser. They observe the canvas finish movement before comparing the
  exact transform.
- The real Execution Client saved and read normalized output through the
  wire. The execution boundary crates passed 412 tests with three ignored,
  including a confirmed unlink retry. The pinned rmcp fork passed two
  HTTP response-cap regressions.
- The generated protocol check and type check passed. Dev type check and
  44 tests passed. TUI type check and 1,365 tests passed with one skip.
  Web Console type check, build and 5,020 tests passed with four skips.
- `git diff --check` passed.

At `2f307cb83d39a5e093e09504cb79284c468ec0b4`, the prepared Linux
workspace passed 7,539 Rust unit and integration tests with 48 existing
ignores. The workspace run and the explicit doctest run each passed the
same 15 doctests. Workspace all-target/all-feature check, strict Clippy,
strict rustdoc and the workspace release binary build passed. Dev passed
44 tests and TUI passed 1,366 tests with one skip. The cold pinned browser
fixture passed all seven MCP editor and development launcher cases after
separating the fixture optimizer cache and scanning the lazy editor at
startup. The local full browser run with staged native executables
reported 791 passes, two skips and two failed visibility assertions.
The Office trace still had a pending Product Host document-preview
request. The mobile Workflow trace still showed the child review
running. The Office test now observes the real conversion response.
Both Workflow viewports await the fake provider's completed scenario.
The original visibility assertions and their time budgets are retained.
The pinned-browser rerun passed both viewport cases and the Office case
twice each, for six passes.
The 12 opt-in Linux mount proofs passed with the prepared binaries. The
App Server mount composition hit its original 60-second liveness guard
while three builds were active. It passed on an unchanged rerun, along
with the five in-crate cases and the real child fixture.

The independently built debug Server (`cargo build -p rustx --bin rustx
--locked`) ran with only that executable in its staging directory. One
real HTTP MCP response carried 16,773,193 JSON body bytes. Server peak RSS
was 175,320 KiB at baseline, 216,404 KiB with a 12 MiB prefix held, and
240,116 KiB after completion. The peer counted one call and two
fake-provider requests. Durable history had one 17,022-byte model tool
message and one Attempt terminal. Server scratch stayed empty.

The independent release Client build
(`cargo build -p rustx-execution-client --bin rustx-execution-client
--release --locked`) ran from a directory containing only the Client.
The revision-six UDS probe used three fresh Clients per operation. The
normal Client dependency graph excludes the Server and reqwest. The
normal Server graph omits the Client executable crate.

| Operation | Application data RPCs | Data bytes to/from Client | Median elapsed | Highest Client RSS |
| --- | ---: | ---: | ---: | ---: |
| Lines from a 67,108,877-byte source | 1 | 99 / 265 | 56.07 ms | 12,248 KiB |
| Grep over the same source | 1 | 149 / 132 | 19.27 ms | 13,472 KiB |
| Publish 2 MiB with create-only expectation | 34 | 2,798,628 / 2,212 | 1,308.87 ms | 13,988 KiB |

Median Client startup ranged from 2.52 to 2.85 ms. Median shutdown,
including confirmed Revoke, ranged from 45.23 to 95.00 ms. These warm-cache
probe measurements include competing local builds, especially disk
contention during publication. The debug Server and release Client
used different workloads and build profiles.

The GitHub Linux [Boundary job for `2f307cb83`](https://github.com/Caismis/rustX/actions/runs/45257812808/job/137984751641)
passed 882 Rust tests with 12 existing ignores, the 55 provider-emulator
tests, and the lane inventory check. The [browser coverage job](https://github.com/Caismis/rustX/actions/runs/45257812808/job/138008766504)
verified all 795 registered cases, with 793 passes and two intentional skips.
These jobs used the earlier fixture configuration.

The same [FULL run](https://github.com/Caismis/rustX/actions/runs/45257812808)
finished with 14 successful jobs and the two expected skips.
[Merge verification](https://github.com/Caismis/rustX/actions/runs/45257812808/job/138025301211)
recorded head `2f307cb83d39a5e093e09504cb79284c468ec0b4`, base
`5ff54d8327917bca2cdbc9647d7da197afe8beed` and tested merge
`bf61a71e945eefdb13a050f8ee8f86d2bc5ada15`.

The preceding hosted [Boundary job](https://github.com/Caismis/rustX/actions/runs/45252112988/job/137964851571)
lost rustc to SIGKILL during Server test compilation. The same compile and
staging command under a four-CPU, 15,005,900 KiB cgroup with swap disabled
reproduced one OOM kill. Increasing codegen units and enabling incremental
compilation in separate runs each reproduced that kill. The original
flags compiled and staged the entire Boundary target set in a 20 GiB
cgroup in 12m 28s, with a 17,680,314,368-byte cgroup peak and zero OOM
kills. The successful hosted Boundary job kept its original settings.

Later concurrent local verification exposed two test-fixture races. The
Scheduler drain fixture waits for the read counter to settle to the one
deliberately parked reader after the driver exits. The test verifies
that reader's gate is still occupied and releases both gates on unwind
so a failed assertion can finish reporting. The repaired case passed 16
individual runs, the 42-case concurrent Scheduler slice and the full
Server library run. The Workspace Python alias fixture encountered
`ETXTBSY` on a newly written executable under parallel process launch.
A separate writer process now finishes before the harness invokes the
fixture executable. All 67 Workspace Python tests passed six consecutive
runs with their original interpreter and witness assertions.

The fixture and evidence follow-ups require their own exact-head FULL
and Merge verification. Native macOS verification is deferred.

## Review fixes after b91af242

| Boundary | Regression |
|---|---|
| Managed Python physical ownership | `python_source_capture_keeps_revocation_open_before_first_chunk` parks the actual capture before its first allocation. Revocation retains the worker while the byte charge is zero. |
| Delegated MCP source identity | `child_stdio_uses_parent_identity_and_keeps_unconfirmed_cleanup` opens two grants with identical launch parameters and checks cross-source tool and resource denial and Open replay. |
| Stdio Receive backpressure | `child_relay_reaps_its_running_stdio_session_on_the_original_client` holds the local writer and then the parent data connection. The sole real process emits one reply, delivered at the original cursor after both refused polls. |
| Recoverable Revoke | `revoke_lane_cuts_keep_only_the_original_cleanup_authority` checks three control disconnect cuts. `real_client_recovers_after_losing_its_revoke_receipt` cuts a real Client's final poll. `drain_accepts_the_original_receipt_after_revoke_lane_loss` covers Host drain and the recovered final receipt. |
| Durable recovery publication | `unpublished_owner_never_fences_an_unrelated_workspace` observes another Workspace during private staging, simulates an interrupted publication, and verifies that a committed owner remains exclusive. |
| Watch retirement and catalog coverage | `replacing_a_skill_refreshes_the_catalog_while_native_watch_retirement_is_held` runs an isolated Client with the old descriptor held during retirement. The Server publishes the replacement Skill while the gate is held. `pending_native_retirement_keeps_independent_watches_live` and `native_retirement_starts_resync_before_returning_the_watch_identity` cover the native slot and bound observer transitions. |
| Managed Python observation | `client_package_watch_refreshes_permissions_and_nested_source_from_the_same_binding` edits the permissions document and a nested Python module independently. It checks the digests and permissions before, during and after an admitted Attempt. |
| Bound Web resource reads | `bound-resources.test.tsx` mounts the resource surface with a Client-only Skill, a different Server MCP revision and a late reply after Session switch. `bound_workspace_mcp_catalog_uses_client_definitions_and_server_network` exercises the real mount-split source read and probe with an absent Server directory and a Server decoy. |

Execution revision 7 rejects revision 6 on both lanes. The App Server's
v81 contract adds `mcp/sourceRead` to return a redacted Client document and
its probe revision. The Product Host can identify an exact registration
when its path is absent. Physical Host operations remain denied, and
the App Server admits the bound read against the selected Session.
