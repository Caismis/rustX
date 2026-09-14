# Runtime residency

Durable Session lifetime, runtime residency, and client attachment lifetime are
independent. One user process can load and execute different Conversations at the
same time, but one durable `ConversationId` has at most one writable live
`ConversationRuntime` in that process.

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
                                      ManagedRuntime
                                  live incarnation + composition
                                  retained allocation + projection host
                                             |
                                    ConversationRuntime
                                  execution / admission / settlement
                                             ^
                                             |
                                external client attachments
                                independent, disposable connections
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

Absence from the registry represents `Unloaded`. Entries are `Loading(flight)`,
`Loaded(ManagedRuntime)`, or `Unloading(retained runtime, flight)`. There is no
Running/Idle/Waiting mirror. Replacement uses the same `Unloading -> Loading`
transition after quiescence, with one shared operation result across the handoff.

The registry mutex protects map membership. A process-local atomic counter
allocates incarnation identities, including across controller reopen. Every
critical section is synchronous and short: observe, claim, allocate an identity,
or publish. No configuration read, filesystem access, recovery, external
preparation, provider work or shutdown is awaited under it. Long work belongs to
a Conversation's transition task and watch channel. Different Conversations do
not wait for each other's flights.

The composition mutex belongs to one `ManagedRuntime`. It protects the optional
host/core retained by that incarnation. It never spans an await. Unload removes
the composition only after shutdown; retained `ManagedRuntime` handles then
identify the old incarnation but cannot recreate its core or endpoint.

A flight's watch sender retains exactly one terminal result. Waiters clone that
result and never retain a watch borrow across an await. `send_replace` also
retains completion when all callers have disconnected. A late waiter sees the
same completion, not an empty notification or a second composition attempt.

## Load

1. `SessionController::acquire_session` resolves the explicit Session/node,
   acquires native `ConversationAccess`, and returns the exact persisted settings
   and revision while the catalog transaction remains coherent. The catalog
   mutex is then released. Allocation is retained through resolution and native
   composition and passed into the tool/storage/workspace owners.
2. Installing `Loading(flight)` under the registry mutex is the same-id load
   claim linearization point. Other callers observing that entry join its result.
   Only the claimant starts composition.
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

Failed composition removes `Loading` before broadcasting its error. Every waiter
receives the same error and the next load can retry. The operation is a supervised
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
The manager releases its host/core and retained allocation, removes the entry,
and publishes success. Durable Session and Conversation data remains. A later
load performs ordinary cold recovery. Already-issued low-level runtime, storage
or endpoint clones remain closed by the native lifecycle and may conservatively
retain resources/allocation until their holders release them; they never preserve
writer or current-incarnation authority. Merely retaining a `ManagedRuntime`
identity does not retain a removed composition.

A shutdown failure leaves `Unloading` with the old composition and its exact
terminal diagnostic. Load, unload and replacement observers receive that failure;
the manager cannot claim successful unload or publish a new writer without proven
quiescence. This is an isolated Conversation failure, not a poisoned manager.

A load racing unload joins the unload flight, then retries cold load after success.
An unload racing load waits for the load result and then drains that incarnation.
An unload racing replacement waits for replacement and drains its resulting
incarnation. Concurrent explicit replacements serialize: each explicit replacement
must cross its own quiescence boundary; ordinary load only reuses or waits.

Replacement retains allocation through old shutdown. Successful native quiescence
is the writer-transfer boundary: the old incarnation's admission is permanently
closed before a new composition can exist. The manager removes the old core,
changes the entry to Loading, reacquires exact persisted settings while retaining
the original allocation, then resolves/composes/recover/binds/publishes/activates.
Failure after old shutdown leaves Unloaded and retryable, never a fictional
rollback to the old composition. Other Conversations remain unchanged.

`RuntimeIncarnationId` identifies a live composition, distinct from durable
Conversation identity, Session identity, AttemptId, cursors and connection IDs.
Replacement preserves ConversationId and changes incarnation. `is_current` is the
semantic seam for future stale-control rejection; Unloading is no longer current.
No JSON-RPC DTO or transport protocol is introduced here.

## Allocation and deletion

Native shared allocation access and destructive exclusion remain the only
load/delete authority. Load-first holds allocation even while resolution or
composition is blocked; deletion preflight/commit rejects in-use ownership.
Delete-first commits removal before physical cleanup; acquisition fails closed
even while the files still exist. The manager has no deleted flag or deletion
lock. No catalog mutex remains held for runtime residency.

## Client lifetime and scope

The manager retains the existing projection host without any connected client.
An attachment can start a turn, disconnect, and reconnect after completion without
unloading, cancelling, settling an interaction, or deleting a Session. Attachment
routing/control for App Server will build on this owned host. The local CLI's
single-runtime adapter remains operational over the same semantic composition;
it is not an alternate App Server residency path.

This change does not implement #288 protocol, #36 WebSocket transport, #291 idle
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
