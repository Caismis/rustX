# Native Scheduler

The App Server process owns one supervised Scheduler wake loop. Durable temporal
state belongs to each destination conversation's existing SQLite database. A
schedule transfers work into ordinary Pending Inbound; model execution and
canonical adoption remain ConversationRuntime responsibilities.

## Implemented native surface

`SessionRuntimeManager::schedule_command`, `schedule_get`, and `schedule_list`
address an explicit Session and node. Commands have stable caller-generated
identities and revisioned mutations. These are trusted native product controls;
there is no model Schedule capability, Scheduler RPC, TUI editor, or Web editor
in this slice. The manager derives the conversation binding from catalog authority.
Reads retain native read allocation and never compose an Agent or open a writer.

Supported rules are offset-bearing absolute `At`, relative `After` normalized to
`At` at the first committed command, and fixed-rate intervals of at least 60
seconds. Instants are stored in UTC, with supported future years through 9999.
The first interval target is acceptance time plus the period. After downtime or
backpressure, one admission selects the latest elapsed occurrence aligned to the
saved anchor, then saves the first future occurrence. Missed ticks never create
an unbounded queue. Cron and scheduled Workflow starts are not enabled.

The complete issue #85 contract and remaining engineering gates are defined in
[the implementation plan](issue-85-scheduler-implementation-plan.md). This native
inbound foundation does not complete that milestone.

## Atomic admission and command identity

A due occurrence fires at one immediate SQLite transaction commit. The transaction
rechecks state and revision using a fresh RuntimeClock sample, accepts a
Runtime-origin ordinary Message through the existing inbox transaction participant,
and advances the schedule with its trigger receipt and generation. Failure before
commit rolls back the target, sequence, receipt, and advancement together.
Concurrent wakes and stale updates cannot admit that revision twice.

The trigger identity contains the conversation-scoped schedule ordinal, admission
revision, and UTC occurrence. Its deterministic message/correlation namespace is
reserved to this compound admission. Ordinary inbound producers cannot forge it.
The occurrence time is the immutable producer timestamp; admission time is
recorded separately. No Scheduler code appends canonical messages.

A command retry first compares its authored-input digest and returns its original
compact committed receipt, including resolved time and revision. It does this
before sampling acceptance time or consulting the current schedule definition.
Receipts contain no reminder payload and survive deletion/pruning. Reusing a key
with different input is refused. Semantic no-ops preserve revision/generation and
write no receipt. Model request grants and their eight-command budgets remain an
explicit later implementation gate; native product controls do not mint them.

## Writer ownership and retirement

The manager's single registry contains finite `StorageOnly` flights alongside
Loading/Loaded/Unloading. StorageOnly is a conversation writer reservation, not a
resident runtime or a second Session slot. Up to 64 cold flights are admitted
independently of the runtime quota. Node B may remain resident while node A in
the same Session receives a durable scheduled message.

Hot commands retain the existing OperationLease and use the mailbox's Running
commit/publication boundary. Cold commands acquire native allocation under the
registered flight, revalidate catalog identity, open an existing-only writer
under ProductController, and acquire ownership-mutation participation before the
final counted commit permit. Load and replacement join the same registered
flight. Session retirement enumerates every matching storage and runtime entry.

Caller cancellation cannot abandon an admitted command. A flight publishes writer
absence only after SQLite explicitly closes and allocation access releases. Close
failure retains the writer reservation and blocks replacement/deletion, while an
already committed command receipt remains truthful. A task panic is reported;
cleanup either proves closure or preserves an unproven reservation.

Required-write failures fence further Scheduler writes for that conversation.
After unloading any runtime, `recover_schedule_storage` explicitly reopens and
reconciles the native store before removing this fence. It cannot bypass an
unproven writer. An unreadable store is never interpreted as empty.

## Capacity, retention, and lineage

Titles contain at most 120 Unicode scalar values, and reminder text at most
16 KiB. Each conversation has at most 256 Enabled plus Disabled definitions.
Successful writes prune Completed display rows to at most 200 entries and remove
rows older than 30 days. The monotonically allocated ordinal is never reused.

Each schedule may have one outstanding native pending message across all its
revisions and edits. There are at most 256 Scheduler-origin pending targets per
conversation, including deleted or pruned definitions. Indexed native pending
queries enforce these limits in the admission transaction. Adoption releases
capacity; schedule deletion, edits, and display retention do not. Ordinary pending
Human/other-producer messages do not consume this Scheduler-specific quota.

Enabled schedules block Session deletion. Fresh deletion preflight includes the
Scheduler generation in its revision; retirement and the counted commit permit
establish the trigger-versus-delete winner. Already accepted ordinary inbound
follows the existing confirmed Session deletion policy. Fork and clone copy
history without copying schedule definitions, command receipts, or temporal
control authority.

## Discovery, activation, and shutdown

The driver rebuilds its index from a finite catalog/node snapshot using read-only
native header probes. It starts another full pass no later than 30 seconds after
the previous pass finishes, even when no schedule is indexed. Command hints can
prompt an earlier pass. A stale observation cannot replace a newer generation.
Unavailable stores remain visible in driver diagnostics and are retried on later
passes. There is no fixed 30-second discovery guarantee independent of inventory
size. Bounded/cancellable scan budgets and supported-scale measurements are still
required by S2 before release acceptance.

Due work and scans use batches of at most 64 and yield between batches. Blocked
candidates wait 30 monotonic seconds; rotation prevents the first blocked batch
from starving later definitions. UTC decisions remain separate from monotonic
waits, and waits are capped at 30 seconds to detect wall-clock changes.

Only verified Scheduler-origin pending identities produce activation hints,
including when the schedule is Completed or deleted. After claiming the exact
runtime slot and allocation, activation rechecks that the target is still pending
before composition. Ordinary Human/Runtime pending work and text labels cannot
cause Scheduler activation. A conflicting resident node is never evicted and the
Session's selected node never changes. Accepted work may wait for explicit target
activation; a committed receipt means admission, not model completion.

Host drain closes Scheduler command/due admission under the host-to-registry lock
order, cancels the one wake loop, joins counted readers and registered storage/load
flights, and preserves the host's final runtime inventory pass. A successful drain
has zero driver, reader, commit, and writer owners. Durable schedules and accepted
targets survive process shutdown. Client disconnection has no scheduling effect.

## Remaining implementation gates

S2 still requires enforced scan-operation budgets, pass-bound and scale/idle
measurements, richer management observations, and complete retention/quota gates.
S3 requires the published Cron dependency spike and golden calendar results.
S4a/S4b require pinned Workflow resource reconstruction and native durable
start/claim/settlement/recovery. S5 requires request-scoped model authority, App
Server/Runtime Client protocol controls, TUI/Web projections, and final conformance.
These capabilities must use the existing owners described in the plan.
