# Session lifecycle ownership

Design checkpoint for PR #457. This contract precedes the actor implementation.

## Authorities

One client-owned XState v5 actor owns each demanded Session in one App Server
process authority lifetime. React owns presentation only. The actor owns desired
Node, intent revision, operation ordering, retained claim, admitted observation,
selected Node evidence, deletion/recovery obligations and lifecycle outcome.
`SessionView` is a projection, never an input that can restore actor authority.

The lifecycle port performs finite native/Host operations. AppServerClient owns
WebSocket correlation, capacity, final admission validation and transport outcome
classification. Native owns Route validity, runtime incarnation, selected Node,
resident Conversation and durable deletion. A retained target is cleanup identity,
not control permission. Detach releases the Route; it does not unload Residency.

## State and transaction shape

The actor has detached, opening, attached, switching, releasing, deleting,
recovering, unresolved and retired phases. Identity, generation, monotonic intent
revision, exact native claim and observation proof are context facts. A bounded
FIFO of explicitly admitted gestures and one active correlated transaction are
owned by this actor. There is no client-side parallel queue or lifecycle reducer.
The registry supplies a client-wide work bound, not operation scheduling.

Intent and transaction settlement are distinct. RELEASE can revoke intent while
the active transaction remains opening or switching. An accepted later Open can
wait behind Release, but cannot replace the immutable identity of an older Open.
Completion events carry the original operation token and generation. The actor
accepts each terminal event once. Async port work sends events to the stable
actor; changing intent never cancels its completion listener or restarts work.
No state-scoped promise invocation may discard a transmitted result.

## Gesture matrix

| Phase / obligation | OPEN | RELEASE | SWITCH_NODE | DELETE |
| --- | --- | --- | --- | --- |
| Detached, no unresolved native work | Admit fixed Node | Local released settlement | Reject: no admitted claim | Admit reviewed native revision |
| Opening | Equivalent Node joins/queues; conflicting Node rejects unless cleanup already precedes it | Revoke now; queue cleanup | Reject | Revoke Open admission; serialize deletion after its settlement |
| Attached with exact proof | Same Node refresh; other Node rejects | Revoke now; detach exact claim | Revoke now; submit exact old claim and new Node | Revoke now; submit reviewed revision |
| Releasing | Queue explicit Open; native residency still decides whether Node is compatible | Join cleanup | Reject | Serialize behind cleanup |
| Switching, including queued Release | Reject every Open | Revoke now; queue cleanup | Reject | Reject synchronously with no state/RPC side effects |
| Deleting / deletion uncertainty | Reject | Revoke local interest; cleanup only when evidence permits | Reject | Reject until authoritative deletion inspection |
| Unresolved retained claim | Reject acquisition until explicit cleanup/inspection | Exact cleanup if available; never invent absence | Reject | Only explicit reviewed native deletion, after active switch settlement |
| Disconnected / retired authority | No native dispatch | Revoke locally | Reject | No native dispatch |

Delete requires a connected authority before admission and releases prior Open
interest. A proven unsent Delete retires locally without automatic reopening;
transmitted uncertainty still requires native inspection.

The exclusion follows the outstanding switch transaction, not the last queued
gesture and not the presence of a browser target. Deletion admitted first removes
the proof needed for Switch. Different Sessions remain independent.

## Native events and settlement

| Event | Required transition |
| --- | --- |
| ATTACH_SENT | Record actual transport dispatch for this exact operation; no claim or observation is invented. |
| ATTACH_ACK | Retain exact native target even if intent was revoked. Verify Session/Node-to-Conversation and Snapshot identity. Publish observation only for current generation and unchanged Open admission. |
| ATTACH_REJECTED (unsent) | No native claim. Retire obsolete Open without error publication; final Release becomes detached. |
| ATTACH_REJECTED (known native failure) | Surface current failure; native refusal proves this request acquired no target. |
| ATTACH_REJECTED (unknown) | Retain uncertainty; absence of a target is not proof of absence. No replay. |
| DETACH_ACK / exact stale-Route evidence | Retire only the correlated claim. Queued Open may then be admitted, subject to native Residency. |
| DETACH_REJECTED / unknown | Retain cleanup/uncertainty. Never claim successful detachment. |
| SWITCH_ACK | Record committed native Node/Conversation and old Route retirement even after Release or NATIVE_ROUTE_CLOSED. Never restore released control. No successor Attachment is implied. |
| SWITCH_REJECTED (unsent) | Retain old cleanup claim, revoke execution, no residency transition inferred. |
| SWITCH_REJECTED (native failure) | May follow unload/selection. Clear unproven selection, retain exact cleanup evidence, fail closed. Explicit inspection/recovery only. |
| SWITCH_REJECTED (unknown) | Keep unresolved outcome, no replay or stale-node automatic Open. |
| DELETE_ACK | Native deleted/not-found retires actor claim; committed-cleanup/durability outcomes retain explicit recovery obligation; stale/blocked keeps live Session but never restores old control. |
| DELETE_REJECTED (unsent) | Clear deleting; no native deletion or uncertainty is invented, and the old Open interest stays released. |
| DELETE_REJECTED / unknown | Preserve native outcome classification and existing deletion inspection requirement. |
| NATIVE_ROUTE_CLOSED | Revoke matching claim observation. It cannot settle an outstanding switch transaction or erase its committed result. |
| TRANSPORT_LOST | Revoke all old-generation proofs synchronously; discard unsent queued gestures, let transport classify active work. Transfer unknown diagnostics, never invent cancellation. |
| TRANSPORT_RESTORED | Update generation; no replay of unresolved mutations. Native inspection is read-only; renewed attachment requires authorized Open intent and a fresh exact admission. |
| AUTHORITY_REPLACED | Revoke and retire old lifetime; old results never publish into replacement. Preserve diagnostics before stopping actors with outstanding obligations. |
| RECOVER | Explicit native inspection/recovery, fenced to authority and generation; no speculative mutation or hidden retry. |

## Linearization and publication

OPEN admission fixes its selector and revision before native work. An omitted
selector is resolved once by authoritative Session inspection before attach
submission. RELEASE/SWITCH/DELETE revoke the exact control proof synchronously in
the accepted actor transition. Rejected Open/Switch/Delete gestures do not mutate actor or projection. Release
always revokes local interest, even if capacity prevents enqueueing native cleanup.
The transport rechecks the actor-provided proof immediately before WebSocket send,
including after Host validation and bounded RPC waiting. Sent operations are not
retroactively classified as unsent. Navigation freshness authorizes dispatch and
navigation callbacks; it does not erase a correlated native result for an unchanged
Session/Node intent lifetime. Equivalent newer navigation can observe that exact
claim without another attach. Release changes the intent revision, so this rule
cannot revive a released proof. An unrevoked observation can be refreshed during
resynchronization, while execution controls remain disabled until refresh settles.

Native Switch commits selection after old-runtime unload and before successor
composition. A composition error therefore does not prove A survived. Native
Delete fences/settles writers before confirming its revision and durable deletion.
Only correlated native evidence settles these facts. Generation and exact claim
identity fence publication; presentation flags do not authorize native settlement.

The actor projects lifecycle facts synchronously after each accepted transition.
Presentation payloads (Snapshot, history, model intent and statistics) remain with
their existing owners, but can publish only under actor-issued identity proofs.
Generic target-bound control RPCs capture this same proof, so direct request calls
cannot bypass lifecycle admission. Native cancellation and cleanup keep their
existing lanes and already-transmitted settlement semantics.

## Lifetime and bounds

The registry belongs to AppServerClient, not React. Actors are created only by
explicit lifecycle demand. Closing a tab cannot drop active transactions. A settled
released or definitively failed actor with no claim, pending command or recovery obligation can retire;
its non-authoritative display data may remain in the existing bounded view cache.
An unresolved obligation remains bounded by existing client diagnostics/work
limits. Connection loss invalidates generation; authority replacement retires the
whole old namespace only after transport has classified outstanding requests.
The client admits at most 64 lifecycle operations and 64 live/retiring actors.
Idle actors without native obligations are pruned toward a 32-entry cache;
existing bounded Session diagnostics retain their non-authoritative errors.
Even capacity-refused Release revokes local control synchronously; it reports
failure to enqueue cleanup rather than fabricating native settlement.
Transport disconnect can retire a preparing, provably unsent Host wait immediately.
A submitted request retains its actor until transport classifies the outcome.
Explicit disconnected diagnostic acknowledgement may retire its diagnostic actor.
Known retained Open intent may acquire fresh authority after reconnect; unknown
transmitted outcomes never trigger that acquisition. Native deletion inspection
is read-only and does not retry a mutation.

Presentation availability is computed by the same actor admission guards. The
transport accepts lifecycle RPCs only through an ephemeral actor port capability;
it does not interpret machine state names. Published targets and proofs are frozen.

No background recovery actor, polling, automatic mutation retry or durable browser
queue is introduced.

## Required evidence

Machine tests drive the real graph with deferred port results and exact SENT/ACK
boundaries. Existing client tests prove final transport fencing and UI projection.
Real App Server tests prove A Unloaded/B Loading at the composition gate, selected
B, B Loaded after success/lost ACK or Unloaded after composition failure, original
Route retirement, incompatible A acquisition refusal, and explicit recovery with
one claim and no provider execution. Browser fixtures are not Residency authority.

### Executable evidence and superseded assertions

| Test surface | Deterministic boundary | Native effects / authority |
| --- | --- | --- |
| `session-lifecycle.test.ts` | Held Host admission, then final Release | 0 attach, 0 detach, detached, 0 claims |
| Same | Sent Attach, Release, held ACK, held Detach | 1 attach + 1 detach; obsolete ACK retains one cleanup claim, never observation |
| Same | Equivalent Opens behind one held ACK | 1 attach, 1 claim; conflicting Node rejected |
| Same | Switch ACK after Release and/or old Route closure | 1 switch; committed B, 0 claims, no restored control |
| Same and `attachment-node.test.ts` | Switch active, Release queued, Delete attempted | 0 delete requests; B settles independently of released interest |
| `attachment-node.test.ts` | Eight occupied RPC slots or deferred final validation | Revoked work never sent; reservations released |
| Same | ACK decoded before lifecycle continuation | Delete still refused; native B identity retained |
| Same | Direct generic lifecycle RPC | All five mutation methods refused without an actor capability |
| `session-surface.test.tsx` | Held Switch ACK followed by Release | Delete button follows actor admission; 1 attach + 1 switch, 0 detach/delete |
| `subagents.test.tsx` | Native T1 Release/T2 Open, held old meter result | Exactly 2 meter reads; late T1 cannot overwrite T2 |
| `session-lifecycle.test.ts` | 64 admitted gestures, then Release | Immediate proof revocation despite full queue; no invented cancellation |
| Same | Authority replacement with sent Switch/Delete | Old actor retained until one terminal result, then stopped; no replacement publication |
| Same | 100 clean released Sessions | At most 32 idle actors; 0 pending operations |
| Native `switch_residency_settlement_survives_failure_and_lost_acknowledgement` | Runtime Manager before-compose gate | Native selected B and A Unloaded; exact Route/Residency assertions for success, failed composition and lost ACK |

Tests no longer manufacture attachment authority by writing a mock SessionView.
The meter fencing regression now performs real client Attach/Release operations.
A lost deletion followed by read-only inspection no longer implicitly reattaches.
An omitted Open selector is resolved through `session/read`, rather than stale
catalog metadata. A rejected Open on a retained failed-detach claim reports its
failure without calling that claim authoritatively attached. These changed
assertions remove accidental behavior; they do not relax native request counts.
