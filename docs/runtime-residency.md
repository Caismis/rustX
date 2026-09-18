# Runtime residency

Durable Session lifetime, runtime residency, and client attachment lifetime are
independent. One user process can load and execute different Conversations at the
same time. One durable `ConversationId` has at most one writable live
`ConversationRuntime` in that process. In v1, one durable Session also has at most
one writable resident Conversation/node. Different Sessions remain concurrently
resident; there is no process-global active Session or automatic branch switch.

```text
SessionController                         UserConfigManager
  durable Session/catalog/graph              current user/project sources
  persisted selections + revision            per-Session resolution
  native allocation/deletion authority
          | acquire_session (retained allocation before cold resolution)
          +----------------------------------+
                                             |
                                  SessionRuntimeManager
                                  per-Conversation residency
                                             |
                                      ResidentRuntime
                                  live incarnation + composition
                                  retained allocation + projection host
                                             |
                                    ConversationRuntime
                                  execution / admission / settlement
                                             ^
                                             |
                                ManagedRuntimeClient
                                weak incarnation handle; owned facts only
```

`SessionRuntimeManager` reuses `LocalConversationCore::compose_with_access`, the
same native composition used by the CLI. It does not construct a second Tool,
Capability, Context, Model, recovery or projection subsystem. Each composition
has independent cwd, resolved configuration, tool/background/interaction
registries, capability generations, history and Event Journal. Mutable MCP,
Subagent and Workflow state is not shared between Conversations.

The manager owns its registry. `SessionController` retains only a one-time
allocation claim for the process runtime owner, never the registry or a live
composition. Constructing a second owner from the same controller is rejected;
request handlers clone the existing manager. That allocation lasts for the
controller's process lifetime, including after erroneous early manager drop.
The native `ProductController` excludes a second controller for the same user
root. This is process-local ownership, not a multi-process lease design.

## State and synchronization

Absence from the internal registry represents no resident runtime (`Unloaded` in diagnostics). This is never a durable Session status. Entries are `Loading(flight)`,
`Loaded(ResidentRuntime)`, or `Unloading(retained runtime, flight)`. There is no
Running/Idle/Waiting mirror. Replacement uses the same `Unloading -> Loading`
transition after quiescence, with one shared operation result across the handoff.

The registry mutex protects Conversation entries and a small `by_session` ownership
index (`SessionId -> ConversationId`). The index is not a second state machine:
it covers Loading, Loaded, Unloading and replacement handoff for the same entry.
Session ownership and the initial Loading flight are installed atomically under
this mutex. Same-Session/same-Conversation callers join/reuse that entry; a different
Conversation returns typed `SessionAlreadyResident`, without composing, switching
nodes, or draining the resident. Terminal removal clears both indexes atomically.
Failed shutdown retains both; replacement retains the Session claim throughout.

The registry mutex protects map membership. A process-local atomic counter
allocates incarnation identities, including across controller reopen. Every
critical section is synchronous and short: observe, claim, allocate an identity,
or publish. No configuration read, filesystem access, recovery, external
preparation, provider work or shutdown is awaited under it. Long work belongs to
a Conversation's transition task and watch channel. Different Conversations do
not wait for each other's flights.

The composition mutex belongs to one private `ResidentRuntime`, the sole strong
owner of its optional host/core. It never spans an await. The registry and its
transition machinery retain that owner. Public `ManagedRuntime` identities contain
only metadata and weak registry/resident references. Flight results retain these
identities, never the resident owner. `ManagedRuntimeClient` holds only a weak
identity reference. Neither handle can extend composition/allocation lifetime.

Client control resolves ConversationId plus RuntimeIncarnationId against Loaded,
then uses the Conversation-local composition slot for a bounded synchronous
operation. No registry guard is held during runtime work. Unload takes this slot
after native shutdown, so any operation that resolved just before unload either
finishes before resource release or observes the empty slot. Operations return
owned facts, never runtime, host, subscription or allocation handles. The native
seam currently offers inbound submission and existing-host projection snapshots;
transport/attachment routing is left to its later owner.

A flight's watch sender retains exactly one terminal result. Waiters clone that
result and never retain a watch borrow across an await. `send_replace` also
retains completion when all callers have disconnected. A late waiter sees the
same completion, not an empty notification or a second composition attempt.

## Load

1. `SessionController::resolve_session_target` reads the durable Session/node and
   Conversation identity without `ConversationAccess`. No managed allocation
   authority exists before the registry claim.
2. Under the registry mutex, the manager checks the Session fence and installs
   `by_session` plus `Loading(flight)` atomically. Only that flight's owner may
   call `acquire_session` for the exact resolved node, obtaining current persisted
   settings and native allocation access. Other callers join without acquiring
   redundant allocation handles. Deletion can identify every blocking managed
   acquisition, including one parked before storage acquisition.
3. The manager resolves current sources using `UserConfigManager`, converts the
   acquired persisted selections to `SessionConfigInput`, and performs ordinary
   trust/credential admission. Effective configuration is never durable authority.
4. Native composition and durable recovery run with the allocation retained.
   The existing `RuntimeClientHost` binds while the runtime remains inert.
5. The flight owner installs a fresh `RuntimeIncarnationId` and its inert
   composition, then calls the infallible native activation boundary outside the
   registry mutex. Activation may synchronously admit recovered SQLite work.
   The slot remains Loading and other callers join the flight until the manager
   publishes Loaded and the shared ready result, with no intervening await.

A warm load returns the existing incarnation. It does not resolve files, restart,
mutate composition, or transfer residency ownership to its caller. Existing safe
runtime-owned live updates retain their existing semantics. Composition changes
require explicit replacement.

Failed composition removes `Loading` and its Session claim before broadcasting
its error. Every waiter receives the same error and the next load can retry. The operation is a supervised
flight rather than work owned by the first request: callers receive no task abort
handle. Cancelling the claimant or any waiter does not cancel composition. Each
transition task owns a `TerminalGuard`; panic or executor destruction publishes a
terminal failure and clears Loading. If an internal panic interrupts synchronous
activation, the potentially activated candidate is retained fail-closed as
Unloading rather than allowing a second writer. Blocking resolution also retains allocation
inside its own closure, so it cannot keep reading after its authority is released.
A process host must keep its executor alive for residency and explicitly unload
runtimes before ordinary process shutdown.

## Unload and replacement

Installing `Unloading` wins residency exclusion. It does not compete with the
runtime's execution gate. `ConversationRuntime::shutdown` takes the existing
coordinator lock and commits `Running -> Draining`; inbound acceptance uses that
same lock. Acceptance either wins first and is settled by the runtime, or sees
the closed gate and commits nothing. The manager adds no execution gate.

Unload commits only after native shutdown returns success and proves quiescence.
The manager releases its host/core and retained allocation, removes the entry
and its Session claim, and publishes success. Durable Session and Conversation
data remains. A later load performs ordinary cold recovery. Previously issued
client/identity handles remain values, but are non-owning and fail explicitly with
`StaleIncarnation`. They cannot keep the runtime or allocation alive, prevent
native deletion, or reopen residency. No owning runtime/endpoint API escapes the
manager. Narrow test-only inspection clones are used solely for native gate tests.

A shutdown failure leaves `Unloading` with the old composition and its exact
terminal diagnostic. Load, unload and replacement observers receive that failure;
the manager cannot claim successful unload or publish a new writer without proven
quiescence. This is an isolated Conversation failure, not a poisoned manager.

A load racing unload joins the unload flight, then retries cold load after success.
Flight terminals explicitly distinguish `Resident`, `WriterAbsent(operation_result)`
and `RetirementUnproven(error)`. Deletion requires absence proof, not operation
success: an acquisition/composition error after proven retirement does not poison
delete. Native shutdown/projection uncertainty retains the unproven writer and fence.
An unload racing load waits for the load result and then drains that incarnation.
An unload racing replacement waits for replacement and drains its resulting
incarnation. Concurrent explicit replacements serialize: each explicit replacement
must cross its own quiescence boundary; ordinary load only reuses or waits.

Replacement retains allocation through old shutdown. Successful native quiescence
is the writer-transfer boundary: the old incarnation's admission is permanently
closed before a new composition can exist. The manager removes the old core,
marks writer absence proven and changes the entry to Loading, then reacquires
the exact node and current persisted settings. The registered flight spans this
allocation-free handoff through new composition and publication.
Failure after old shutdown leaves Unloaded and retryable, never a fictional
rollback to the old composition. Other Conversations remain unchanged.

`RuntimeIncarnationId` identifies a live composition, distinct from durable
Conversation identity, Session identity, AttemptId, cursors and connection IDs.
Replacement preserves ConversationId and changes incarnation. `is_current` is the
semantic check also used by the native client façade; Unloading is no longer current.
The App Server protocol binds attachment controls to this incarnation identity.

## Allocation and deletion

The manager Session fence linearizes deletion against runtime admission, including
composition already in flight. Deletion joins native retirement before requesting
destructive allocation exclusion from the catalog owner. Inspection itself permits
resident writers. Any remaining independent allocation owner is a resource conflict.
The catalog commits removal before physical cleanup; subsequent allocation access
rejects removed membership even while files remain. No catalog mutex remains held
for runtime residency.

## Client lifetime and scope

The manager retains the existing projection host without any connected client.
An attachment can start a turn, disconnect, and reconnect after completion without
unloading, cancelling, settling an interaction, or deleting a Session. Attachment
routing/control for App Server uses non-owning host handles. Stale attachment,
subscription and endpoint handles cannot retain the resident composition or its
allocation after successful unload. The local CLI's
single-runtime adapter remains operational over the same semantic composition;
it is not an alternate App Server residency path.

The [App Server protocol](app-server-protocol.md) defines the public connection
boundary. Residency does not implement #36 stdio JSONL/WebSocket bindings or the
standalone App Server entry point, #291 idle
TTL/quotas/resource governance, or distributed/multi-process ownership.

## Deterministic evidence

`tests/scripted/app_server/` is compiled as the manager's in-crate owner suite.
It uses post-claim/pre-compose and pre-shutdown watch gates, join counters, existing
native coordinator/attempt-exit gates, and HTTP provider header gates. It covers
shared result identity and failures, cancelled claimant/unload requests, task
panic cleanup, both admission winners, load/unload ordering, replacement writer
transfer, native load/delete exclusion, failure isolation, disconnected clients,
completed-work recovery and unknown external tool outcomes. Timeouts bound test
liveness only; elapsed time is never evidence for a race outcome.

The Session-branch regression creates two real durable graph nodes, parks the first
node at post-claim/pre-compose and pre-shutdown watches, and proves the second node
is rejected before composition. A different Session remains in its provider gate
and completes normally. Successful unload then permits the other node to claim.
Lifetime regressions retain production client and identity handles across native
unload and deletion; real delete preflight/commit succeeds while those handles
remain stale. Runtime weak references prove old execution ownership is gone after
both unload and replacement, while the new incarnation's client can read its host.

A separate replacement test parks new composition after old shutdown, verifies
that the Session claim still rejects another node, and injects a composition panic.
The terminal guard clears both indexes, allowing the other node to load.

## Session deletion admission (#359)

Session existence is durable product state. Attachment is a client relationship.
Residency is an internal process resource lifecycle. Opening a Session reuses or
composes a runtime; closing a view only detaches. Clients never manage unload.

`SessionRuntimeManager::delete_session` inserts the Session identity into
`retiring_sessions` under the registry mutex. This is the deletion admission
linearization point, including when `by_session` has no Conversation entry.
Load and replacement claims consult that same fence. Operation leases and attach
pins consult it under the same mutex. Operations admitted first drain normally;
operations arriving after the fence cannot enter the old runtime.

A Loading flight that completes after fencing activates under the existing native
boundary, but its terminal owner installs Unloading instead of publishing Loaded
or a usable identity. The original flight completes only after native shutdown
and projection drain. Replacement uses the same publication rule. Idle eviction
and deletion join the same retirement flight. No global lock spans shutdown.

Only proven retirement permits destructive exclusion and catalog deletion.
Unproven shutdown retains its composition, allocation, counted Unloading slot,
and Session admission fence. Cancellation never reopens that fence. Safe stale
confirmation/resource rejection after proven retirement can release admission;
durability uncertainty retains the fence. Catalog authority prevents deleted
identities from reopening after successful deletion.
