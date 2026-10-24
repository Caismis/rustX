# Native Scheduler

The App Server process owns one supervised Scheduler wake loop. Durable temporal
state belongs to each destination conversation's existing SQLite database. A
schedule atomically transfers either an ordinary Runtime Message to Pending
Inbound or a pinned Workflow request to the conversation-owned execution queue.
ConversationRuntime owns canonical adoption and Workflow execution.

## Native control and timing

`SessionRuntimeManager::schedule_command`, `schedule_get`, and `schedule_list`
address an explicit Session and node. Commands have stable caller-generated
identities and revisioned mutations. These are trusted native product controls;
App Server v57 exposes `schedule/list`, `schedule/get`, `schedule/command` and
`schedule/recoverStorage`. The manager derives the conversation binding from
catalog authority. Root model Tools expose `get_schedule`, `list_schedules` and
`schedule_command` with native current-Human authorization described below.
Reads retain native read allocation and never compose an Agent or open a writer.

Supported rules are offset-bearing absolute `At`, relative `After` normalized to
`At` at the first committed command, and fixed-rate intervals of at least 60
seconds. Instants are stored in UTC, with supported future years through 9999.
The first interval target is acceptance time plus the period. After downtime or
backpressure, one admission selects the latest elapsed occurrence aligned to the
saved anchor, then saves the first future occurrence. Missed ticks never create
an unbounded queue.

Cron uses exactly pinned `croner` 4.0.2 with the existing `chrono-tz` IANA data.
It accepts five numeric fields with `* , - /`, no seconds, year, names, macros or
extensions. Expressions are bounded to 256 bytes, zones to 128 bytes. Whitespace
is normalized; wildcard provenance is preserved. The standard DOM/DOW policy
uses OR when both fields are restricted. A fixed time in a DST gap moves to the
first valid local time; in an overlap it runs once at the earlier instant.
Wildcard/step expressions may visit both UTC passes. Forward/backward golden
fixtures cover these policies and latest-only catch-up without enumerating missed
ticks. Saved UTC candidates remain valid across timezone-data changes. Cron's
native final UTC year is 4998, leaving timezone-offset headroom inside Croner's
search bound. Unsatisfiable/no-future authoring fails. Final admission marks a
recurring rule Completed with an explicit exhaustion reason; a permanent saved
rule failure disables it with a bounded reason and admits no target.
Scheduled Workflow starts use the same admission transaction and independent durable lifecycle.

The complete issue #85 contract and acceptance matrix are defined in
[the implementation plan](issue-85-scheduler-implementation-plan.md).

## Product reads and Workflow preparation

The Web conversation header opens a read-only Scheduler panel for the exact
Session/node captured when opened. TUI `/schedules [node-id] [after-ordinal]`
reads the explicit node, or resolves the displayed Session's native active node.
Neither reader attaches or activates a runtime. A list returns bounded headers
without reminder text; Web fetches one body with `schedule/get` on demand.
Ordinals/revisions/generations use lossless wire decimal strings. Late responses
from retired connection authorities or replaced pages are discarded. A completed
schedule means no future occurrence; a receipt proves admission, not success.

`runtime::workflow::scheduled` now prepares an inert contract from an immutable
configured native resource snapshot. It validates root Workflow permission,
arguments, native registrations and every static Tool, Agent and Model dependency,
including nested blocks. Compiler contract 2 includes the actual provider wire
model IDs of both Agent primary and explicit summary invocations in the pinned
profile digest; credential rotation leaves that identity unchanged. It pins exact
binding/workspace, definition/compiler,
derived step/Agent/Model/retained-output/time bounds, native Agent profile v4,
provider endpoint and selected source materialization identities, authorized
environment, effective extensions and inherited execution/approval policies.
Same-provider credential rotation and unrelated resource revisions are allowed;
semantic dependency changes refuse reconstruction. Descriptor and argument bounds
are 64 KiB and 16 KiB. Provider/HTTP source endpoints must be HTTP(S), without
embedded user information, query or fragment. No credential values are persisted.

Preparation may construct native model adapters off-side, but starts no requests,
Tools, children, Workflow run, interaction or authority mutation. Its returned
resources confer no execution authority. Workflow create/update obtains this
preparation from an already composed exact-node runtime; an unavailable preparation
refuses authoring without activating a runtime. Due admission and recovery can run
cold through StorageOnly. The accepted request retains its own contract and
arguments even after editing, deleting or pruning the schedule definition.

## Durable Workflow execution and recovery

A native start identity is derived from the immutable TriggerId. Atomic admission
creates `Accepted`; final native resource/model/approval/lifecycle validation
issues the unique `Claimed` capability before any graph effect. Preparation runs
off the coordinator lock. Transient dependency unavailability parks Accepted work
with a monotonic retry; semantic mismatch durably rejects it before execution.

The conversation's foreground slot, capability lease and counted lifecycle owner
cover preparation, execution and physical settlement. Scheduled runs share the
existing Workflow block executor and Tool/Agent/Model/Review owners. Their run
origin names the native start, with no synthetic Attempt, ToolCall or turn zero.
A total monotonic deadline propagates cancellation but still joins native work.
Each external node commits an effect receipt before dispatch and records its exact
native owner. A completed run commits its bounded Return value and digest with
settlement. While its native owner remains registered, reconcile advances physical
proof and wakes that owner; only that owner publishes its pending terminal result.
A graph-finished publication does not transfer this authority. A Schedule
admission receipt does not claim this execution succeeded.

Reopening or reconciling Claimed work never restarts its graph. The native owner
reconciles exact Agent ownership/terminal/physical publications and Auxiliary Call
terminal audit. Historical interactions close through their original durable
scope and are never recreated as live waiters. A recovered run with positive
quiescence evidence settles OutcomeUnknown, preserving uncertainty about external
results. A missing proof remains recovery_blocked and keeps both scheduling
capacity and Session deletion blocked. Repeated cancel/read/reconcile neither
forces retirement nor provides another executor. Bash and external tools lacking
a retained positive native proof remain blocked; process absence and elapsed time
cannot substitute for one.

App Server `workflow/scheduled/getRecovery` reads one bounded effect page (1–100)
and the committed result. `workflow/scheduled/cancel` durably records cancellation
intent before signaling a live owner, or settles an unclaimed Accepted request.
`workflow/scheduled/reconcile` consumes only native evidence. These methods take
exact Session/node/start identities, with no force or caller-provided proof.
Read-only Web recovery and TUI `/workflow-recovery <start-id> [node-id]
[after-effect-ordinal]` preserve exact-node/connection authority and never activate
or mutate work.

## Atomic admission and command identity

A due occurrence fires at one immediate SQLite transaction commit. The transaction
rechecks state and revision using a fresh RuntimeClock sample, accepts a
Runtime-origin ordinary Message through the existing inbox transaction participant,
or creates the pinned Workflow Accepted request, and advances the schedule with
its typed trigger receipt and generation. Failure before
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
write no receipt. The backend also accepts trusted native Human-request authority,
scoped by Human MessageId and original AttemptId. Up to eight successful mutations
share a persisted transactional budget; concurrent commands cannot exceed it.
Retries return the original receipt and usage. Validation errors, no-ops and
stale CAS do not spend budget, while deletion/pruning never refunds it. Runtime
controls use a separate namespace. The authority is not a serializable model
argument.

A model mutation borrows the current root AgentExecution's live native Human
grant. Fresh inbound selection follows the existing Goal rule: the latest ordinary
Human Message in native batch order replaces the scope, while Runtime input during
that request preserves it without renewing the budget. Historical transcript,
Continuation, independent Runtime-only activation, child execution and fixed
Workflow nodes cannot establish a grant. Restart does not infer one from history;
a fresh Human handoff is required when exact request authority is unavailable.
Goal consumption and Schedule budget are independent. Canonical Tool occurrences
define command identity even when provider call IDs repeat across turns. Native
cancellation and lifecycle checks fence the transaction; exact committed retries
resolve before Workflow preparation. Model instructions require explicit user
scheduling intent, but provenance does not prove the model interpreted that intent
correctly.

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

Workflow preparation refusals return the typed Invalid command result and do not
fence subsequent commands or due admission. Required-write failures fence further
Scheduler writes for that conversation.
After unloading any runtime, `recover_schedule_storage` explicitly reopens and
reconciles the native store before removing this fence. It cannot bypass an
unproven writer. An unreadable store is never interpreted as empty.

## Capacity, retention, and lineage

Titles contain at most 120 Unicode scalar values, and reminder text at most
16 KiB. Each conversation has at most 256 Enabled plus Disabled definitions.
Successful writes and authoritative reconciliation prune Completed display rows
to at most 200 entries and remove rows older than 30 days. The monotonically allocated ordinal is never reused.

Each schedule may have one outstanding native pending Message or nonterminal
Workflow request across all its revisions and edits. There are at most 256 Scheduler-origin pending targets per
conversation, including deleted or pruned definitions. Indexed native pending
queries enforce these limits in the admission transaction. Message adoption or
proven Workflow settlement releases capacity; schedule deletion, edits, and display
retention do not. Workflow bodies/effects/results use the same 30-day/200-row
display retention, while compact start identities survive for deduplication. Ordinary pending
Human/other-producer messages do not consume this Scheduler-specific quota.

Enabled schedules and nonterminal Workflow starts block Session deletion, even
when the schedule definition no longer exists. Fresh deletion preflight includes the
Scheduler generation in its revision; retirement and the counted commit permit
establish the trigger-versus-delete winner. Already accepted ordinary inbound
follows the existing confirmed Session deletion policy. Fork and clone copy
history without copying schedule definitions, command receipts, or temporal
control authority.

## Discovery, activation, and shutdown

The driver rebuilds its index from a finite catalog/node snapshot through native
read-only probes. It resolves the exact binding under the live catalog, retains
shared allocation exclusion, rechecks that live authority, and validates the
existing database's header, complete schema shape and conversation identity.
It never reloads the complete catalog from disk for each database. Prepared reads
cannot reopen a tombstoned allocation; unrelated metadata edits remain allowed.

Each read has a one-second monotonic deadline covering allocation/open/validation/
query/close, a bounded 25 ms SQLite busy wait, a two-million VM-step ceiling and an
exact-connection interrupt scope. At most 64 physical reader tasks may exist.
Timeout/caller cancellation interrupts SQL; a physically delayed filesystem call
keeps its counted allocation/reader owner until actual release. Drain waits for
that tail. Unix header opens refuse symlinks and use nonblocking regular files.
An OS/filesystem stall can delay physical retirement, but cannot occupy an
unbounded number of workers or hold up subsequent logical probes.

Scans visit at most 64 stores per round. Dirty hints name individual conversations
in a deduplicated set capped at 1,024; overflow is counted and periodic discovery
repairs it. Dirty refresh has a one-second phase budget. Due dispatch has a
two-second phase budget and at most 64 actions; keyset headers page past a blocked
prefix without decoding reminder payloads. Dispatch preserves service age across
header refreshes, visiting never-served and then least-recently-served sources
before recently serviced ones. Parked activation sources cannot repeatedly take
priority over a healthy due source. Loaded targets wake through an independent
notification handle, without waiting on composition under the global registry.
The next full pass starts at most 30 seconds after completion, including an empty index and continuous dirty work.

For inventory size N, let R = max(1, ceil(N / 64)). The logical pass bound is
`Tpass = 1s + N * 1s + R * (1s + 2s + 0.1s)`. The lost-hint discovery bound is
`2 * Tpass + 30s` while the process runs and the store is readable; it is not a
fixed 30-second SLA. Diagnostics expose inventory/progress, bound/overruns, read
and dispatch failures, dropped hints and each source's availability, last success,
generation, due time, enabled count, pending count and execution wait. Unavailable
sources retain prior facts and rejoin after repair. Older results cannot overwrite
newer generations. Only current catalog membership removes an indexed source.

UTC decisions remain separate from monotonic waits, which are capped at 30
seconds. Blocked sources park for that interval after paging their candidates.
Supported-scale and idle release evidence lives in
[the performance gate](issue-85-scheduler-performance.md).

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

## Validation and format gates

S4b and current-Human model authority extend the native inbound foundation and
S4a preparation seam. The integration and resource results are recorded in
[validation](issue-85-scheduler-validation.md) and
[performance acceptance](issue-85-scheduler-performance.md). The current formats
are SQLite 81, App Server 57, Runtime Client 76 and child IPC 35, with Session
catalog 18. Older formats are refused without compatibility readers or migrations.
