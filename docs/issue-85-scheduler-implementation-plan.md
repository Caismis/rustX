# Issue #85: native Scheduler implementation proposal

Research date: 2026-10-22. This document fills in the implementation choices for
[#85](https://github.com/Caismis/rustX/issues/85), following its ownership and
acceptance skeleton. It is a proposal for implementation, not a statement of
shipped behavior. Fleet, distributed execution, and remote scheduling are outside
this scope. No runtime implementation or DSH source is introduced by this note.

Source baselines:

- rustX: [`df06c3071398aed6d795e8837beb002773dae668`](https://github.com/Caismis/rustX/commit/df06c3071398aed6d795e8837beb002773dae668).
- DeepSeek Harness: [`687f282e8625c6b8d11ff58211a90524be2e4317`](https://github.com/deepseek-ai/deepseek-harness/commit/687f282e8625c6b8d11ff58211a90524be2e4317),
  the current `master` inspected for this research, release merge dsh 0.2.1-rc.2.
- Croner candidate: [`24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83`](https://github.com/Hexagon/croner-rust/commit/24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83).
  This source reports version 4.0.2; a published version and its lockfile must be
  selected and tested before adding the dependency.

## Goal

Implement one native owner of durable temporal eligibility. A successful trigger
means one occurrence and its native target admission committed together. It does
not mean that a model completed, a Workflow succeeded, or an external side effect
happened exactly once.

The first vertical slice is `At -> ConversationInbound`, including the shared
hot/cold writer protocol and durable admission without target activation. Extend
that transaction and driver with fixed-rate intervals, then Cron, then the native
Workflow admission seam below. The complete #85 milestone includes both targets;
the inbound slice alone does not complete the issue. Timing guarantees concern
durable admission. Execution may wait for the exact destination to become resident.

## Why this is a native domain

The current [inbox owner](../src/durable/sqlite/inbox.rs) already provides
`accept_inbound_tx`, durable producer correlation, and atomic Goal-round plus
inbound acceptance. The [mailbox](../src/runtime/inbound.rs) holds the lifecycle
commit guard, publishes the committed observation, and wakes ordinary admission.
Those are the appropriate seams for Scheduler.

By contrast, the current [WorkflowRuntime](../src/runtime/workflow.rs) exposes
`run_foreground`, takes `AttemptSubagentContext` and `ToolCallId`, allocates a
process-local invocation ordinal, and keeps run state in memory. Its journal
events are explicitly best-effort observations. An idempotent durable
`start(trigger_id, ...)` API does **not** already exist.

### DSH findings and their application

The inspected DSH implementation is Host-wide. Older descriptions of a
Session-local event-log scheduler describe a superseded implementation; the
current Host tasks are not reconstructed from those historical events.

| Concern | DSH evidence | Proposed rustX treatment |
| --- | --- | --- |
| Durable authority | A storage-domain task row contains the original Session, timing rule, status, latest receipt and retained receipt history. [Storage][dsh-storage] | Use the existing target conversation SQLite store; no new database or Session event replay. |
| Scope and cold Sessions | One Host timer; the Session controller restores the original Session when due. Catalog/history reads do not activate it. [Service][dsh-service], [runtime][dsh-runtime] | One App Server-owned driver; exact native binding, durable cold admission, and explicit execution wait when another node is resident. |
| Mutation ordering | Create/update/delete and dispatch share one FIFO. Update compares the complete expected record. [Service][dsh-service] | Serialize local operations where useful, but enforce revision and occurrence CAS in SQLite. A mutex alone is insufficient after restart. |
| Missed work | Every/Daily/Weekly/Cron select the latest due occurrence. Same-Session recurring records may share a follow-up. [Domain][dsh-domain] | Adopt latest-only catch-up; keep one native admission per schedule occurrence initially. Cross-schedule message batching is unnecessary. |
| Delivery | `followup` appends inbox work, Session flush acknowledges persistence, then the task put advances state and saves its receipt. [Runtime][dsh-runtime] | Commit trigger advancement and native target admission in one transaction. |
| Crash gap | Inbox flush and task put are different durable writes; a crash between them may deliver again. DSH explicitly documents this limitation. [Subsystem][dsh-spec] | Do not reproduce this sequence. A saved delivery receipt by itself does not prove #85's exactly-once admission. |
| Time rules | Explicit zones; saved UTC targets survive restart. DSH implements its own five-field numeric Cron parser using Temporal. [Domain][dsh-domain] | Reuse `chrono`/`chrono-tz`; evaluate a mature parser rather than copying that parser or adding a cron engine. |
| Failure/drain | Failed tasks stay stored, without their own retry timer; a later wake/management change/restart can retry. Disposal clears the timer and awaits delivery. [Runtime][dsh-runtime], [tests][dsh-runtime-tests] | Define bounded retry and an admission fence explicitly; drain the driver before closing its stores. |
| Product surface | Task title, instruction, next time, active/inactive catalog and delivery receipts; four model tools use the same service as the UI. [Subsystem][dsh-spec] | Use one native control service and honest admission receipts. Presentation is a projection. |

DSH is a design reference for time, management and presentation. Its Cordis
service, persistence adapters, compatibility decoders and execution path are not
dependencies of this implementation.

## Target topology

```text
App Server SchedulerDriver (one supervised wake loop)
    -> discover exact Session/node schedule summaries
    -> acquire target writer through SessionRuntimeManager
    -> fenced ScheduleStore transition in target conversation SQLite
         [occurrence CAS + schedule advance + native target admission + receipt]
             -> ordinary Pending Inbound -> ConversationRuntime
             -> durable Workflow start receipt -> WorkflowRuntime
```

The driver lives alongside `SessionRuntimeManager`, not in a browser, connection,
or individual Agent Loop. It operates while the App Server is running; stopping
the process suspends delivery until restart. It neither starts the OS process nor
acts as a system daemon.

## Domain model

Keep one record per schedule, with typed identifiers and bounded values:

```text
ScheduleSnapshot {
  id, revision, title,
  binding: { session_id, node_id, conversation_id },
  spec: At { at_utc }
      | Interval { anchor_utc, every_seconds }
      | Cron { expression, time_zone },
  target: ConversationInbound { text }
        | Workflow { pinned_start_spec, args },
  state: Enabled | Disabled | Completed,
  next_due_utc, last_committed_trigger?, last_failure?
}
TriggerId = (ScheduleId, admission_revision, occurrence_utc)
```

The binding identifies a native user Session graph node and its conversation;
the destination cannot follow UI focus or silently switch to the current node.
Both target kinds execute in that same conversation ownership domain. V1 excludes
cross-conversation targets and child-Agent targets. Changing the binding requires
creating a new schedule. Allocate IDs from `(ConversationId, schedule_ordinal)`;
an atomic next-ordinal counter survives schedule deletion/pruning for the lifetime
of the conversation. Conversation identities already have native non-reuse
authority. Counter exhaustion refuses creation; do not wrap or recreate an old ID.

`revision` advances on every committed mutation, including occurrence advancement;
a semantic no-op preserves it. `admission_revision` is the revision read before
the successful firing transaction. `Completed` means no future temporal
occurrence remains, not that target execution succeeded. Disabled state preserves
its reason; failures are bounded diagnostics, not another execution state machine.

Initial limits: title at most 120 Unicode scalar values; text or serialized
Workflow arguments at most 16 KiB; at most 256 **nonterminal** schedules
(`Enabled` plus `Disabled`) per conversation; list page size 1–100. Completed
rows have a separate display-retention cap of 200 records and 30 days, pruned on
writes/reconciliation. Delete removes the row and frees its applicable quota;
creation is never limited by lifetime creation count. Last-trigger display data
is pruned with that row. No per-schedule tombstone is required for ID non-reuse.

Native inbound correlation, Workflow start ownership and consumed Human-command
receipts follow their own authority lifetimes; deleting a schedule or pruning
display history must not remove their deduplication/ownership evidence. They do
not consume the 256-schedule quota. Native target limits apply independently.

## Ownership separation

Preserve the issue's division: Goal owns WHAT, Workflow HOW, Scheduler WHEN,
Subagent WHO, and Tool DO. Scheduler owns schedule definitions, revisions and
occurrence evidence. Target owners own execution, cancellation and recovery.
The driver receives narrow admission capabilities, not a model adapter or a
general-purpose SQL callback.

### Hot/cold writer protocol

For each Conversation, Scheduler mutations participate in the same native writer
ownership and lifecycle fencing as runtime-owned mutations. SQLite serialization
and `ConversationAccess` alone do not establish this ownership.

The existing [`SessionController::acquire_session`](../src/local_runtime/session_controller.rs)
retains allocation access without composing a runtime, and explicitly delegates
single-writer admission to the manager. [`ConversationAccess`](../src/runtime/local_storage.rs)
is shared deletion exclusion; `ProductController` excludes a second product
writer process. [`open_existing`](../src/durable/sqlite/schema.rs) is read-only.
The existing guarded `recover_existing` path demonstrates an existing-only writer
open, but is not a public Scheduler mutation escape hatch.

Add one narrow manager operation for schedule storage transitions. Extend its
existing per-conversation registry with a finite `StorageOnly` flight, carrying
Session/node identity, an existing-only store and allocation retention. This is
a manager-owned writer state, not a second registry or lock manager. The flight
has the same terminal/retirement certainty discipline as Loading/Unloading.

| Observed state under the manager registry | Required transition |
| --- | --- |
| Loaded exact conversation | Borrow its native mutation capability; enter its Running/lifecycle and publication guards. Never open another writer. |
| Loading or Unloading | Release locks, wait for the registered flight, then resolve again. An unproven retirement remains closed. |
| StorageOnly exact conversation | Serialize bounded storage operations in that flight; runtime load waits for the flight to close its writer and prove release. |
| Absent | Register StorageOnly before acquiring allocation/opening a writer. Revalidate catalog identity and Session retirement, then open the existing database under ProductController authority. |
| Session retiring/deleted, process draining, or writer retirement unproven | Refuse admission. Never reinterpret this as an absent slot. |

The runtime load path must observe StorageOnly before installing Loading. If load
wins first, a storage command joins Loading and then uses the resident capability;
if storage wins, load waits for its committed result, writer closure and allocation
release. Unload never detaches a capability while its mutation permit is held.
Failure/caller cancellation still settles the registered flight; no caller owns
an untracked store or an unjoined cleanup task.

`by_session` continues to enforce one **runtime** residency per Session. StorageOnly
is a per-conversation writer reservation, so it may coexist with another node's
runtime without activating that node. Update registry checks, capacity accounting,
idle reaping, retirement enumeration and drain inventory together: storage flights
are bounded admitted operations, not resident Agents. Session retirement must
find every matching storage flight, including ones absent from `by_session`.

All paths retain the existing product ownership-mutation participation before
registry/allocation/commit locking. Under short registry sections, reserve/recheck
the operation and retirement state; release the mutex before acquisition or waits.
At commit, hold the shared process/Session admission permit, allocation retention,
and hot lifecycle/publication guard or cold StorageOnly commit guard, then SQLite.
Do not wait for a flight or provider while holding ownership/commit locks. A cold
guard authorizes only the closed schedule operations and scheduled inbox/start
acceptance; it does not mark a ConversationRuntime Running or enable arbitrary
mailbox writes. Hot and cold paths call the same backend transaction participant.

## Trigger linearization

For inbound, extend the existing atomic Goal/inbox pattern with a dedicated
`accept_scheduled_inbound` operation:

1. Resolve the exact destination through the writer protocol above and prepare
   bounded input without activating an Agent or performing target work.
2. Under the hot publication boundary or cold commit guard, begin one SQLite
   immediate transaction.
   Read the current schedule and validate identity, revision, `Enabled`, binding,
   and due state against a freshly sampled UTC decision time.
3. Derive the selected occurrence and immutable `TriggerId`. Call
   `accept_inbound_tx` with ordinary `InboundKind::Message`, runtime provenance,
   and a stable correlation derived from that trigger.
4. Advance the schedule/revision and store the authoritative trigger receipt,
   native target correlation and store-level scheduler generation in **the same
   transaction**. These required writes either all commit or all roll back.
5. Commit. Only now is the occurrence fired. Publish observations; wake an
   already-resident coordinator or expose accepted work awaiting activation.
   A lost wake or Journal publication does not undo accepted work.

For Workflow, step 3 instead calls a Workflow-owned transaction participant that
inserts the durable start receipt. It is not a subsequent non-idempotent start
call. The remaining transaction and firing boundary are the same.

Do not nest public methods that start separate transactions. Use typed compound
operations and backend-private helpers, following the inbox seam and the writer
protocol above. Scheduler Journal events are optional post-commit observations,
not participants in this transaction. Failure of a required receipt/target/state
write aborts admission; failure to publish an observation after commit does not.
Driver inventory locks are released before target acquisition. No asynchronous
I/O occurs under SQLite or the inventory lock.

A duplicate wake must re-read state. Once a revision advances it cannot admit
that revision again. If its receipt remains available, a duplicate request returns
the existing receipt; otherwise it returns stale/already-advanced, never performs
the target call. Receipt display retention must not weaken revision fencing.

The existing inbound correlation check compares source, kind, content and
timestamp as well as identity. Use `occurrence_utc` for the producer timestamp
and a deterministic envelope; do not rebuild retries with a new `now()` timestamp.
Record decision/admission time separately in the trigger receipt.

## Temporal semantics

### At

Accept an offset-bearing RFC 3339 instant and store UTC. Reject offset-free text,
invalid instants and a time that is no longer future at command acceptance.
Relative “after” UI/tool input, if exposed, normalizes to `At` using the accepted
command's clock reading; it is not a fourth durable schedule kind.

A committed but overdue one-shot fires once after restart. Its successful target
admission makes it `Completed`; target failure does not reactivate it.

### Interval

Use fixed-rate intervals with `every_seconds >= 60`. The persisted anchor `A` is
the first due instant: accepted creation time plus the interval `P`. For saved
next target `D` and decision time `N >= D`:

```text
selected = D + floor((N - D) / P) * P
next     = selected + P
```

All values remain aligned to `A`. Commit exactly one catch-up admission, then the
first future target; never iterate through a downtime backlog or derive the next
time from completion. Example: anchor 10:00, period 10 minutes, restart at 10:37
admits 10:30 once and stores 10:40. Checked arithmetic rejects overflow.

### Cron

Recommend Croner 4.x with its `chrono` backend, reusing existing `chrono` and
`chrono-tz`. The inspected source exposes forward and backward occurrence search,
explicit seconds/year parser settings, and zoned time support.
[Parser][croner-parser], [search implementation][croner-lib], [dependency manifest][croner-cargo].

V1 accepts five numeric fields, spaces and `* , - /` syntax. A small shape/character
guard rejects macros, names, seconds/year fields and extensions such as `L W # ? +`;
the library owns range/step validation, day matching and occurrence search. Set
seconds/year to disallowed, leave sloppy ranges and alternative weekdays off, and
keep the library's standard DOM/DOW policy. Preserve the accepted expression
apart from whitespace; do not canonicalize `*` into full ranges because wildcard
provenance can affect semantics. A textual change may legitimately create a new
revision even when two expressions happen to match the same dates.

Store an explicit IANA zone and the committed next UTC instant. No host-local zone
or browser default may enter durable meaning. Restart preserves that instant;
subsequent searches use the binary's bundled timezone data. A timezone/dependency
upgrade must rerun the calendar fixtures and document changed future behavior.

Adopt the pinned library's DST policy rather than promising DSH parity. Croner's
documented fixed-time gap behavior moves to the first valid time on that date,
where DSH skips the nonexistent local occurrence. In an overlap Croner runs a
fixed-time expression once at the earlier instant, while wildcard/step expressions
may match both passes; DSH resolves ambiguous local candidates to the earlier
instant. [Croner DST policy][croner-readme], [DSH Cron tests][dsh-cron-tests].

Golden examples for `America/New_York`: `30 2 * * *` on 2026-03-08
expects Croner's gap adjustment to 03:00 local (07:00Z); `30 1 * * *` on
2026-11-01 selects 01:30 EDT (05:30Z) once. `*/30 * * * *` must distinguish
both UTC passes of the repeated hour. These are dependency-gate expectations,
not results already verified by this research.

Latest-only catch-up applies to Cron too: select the latest library occurrence
at or before `N`, never before persisted `D`, and store the first occurrence
strictly after `N`. Preserve a due committed `D` as a candidate even if updated
timezone rules no longer regenerate it. Search around the decision using the
library's forward/backward APIs, not by enumerating all missed minutes. Bound the
accepted year range and reject unsatisfiable/no-future expressions; distinguish
range exhaustion from storage failure. An exhausted recurring schedule becomes
`Completed` with a reason after its final admission.

**Dependency gate:** before enabling Cron, run a small dependency spike against
the selected published version for five-field rejection, DOM/DOW wildcard cases,
DST gaps/overlaps, reverse search, impossible dates and the final representable
year. Pin the dependency and golden results. This research inspected source; it
has not validated that candidate by executing it. Failure of this gate leaves
Cron unavailable and #85's final Cron decision explicit, rather than introducing
a handwritten parser or changing semantics silently.

## Schedule updates

Every update, enable, disable and delete carries an expected revision. Return a
typed conflict with the current bounded snapshot; clients must reread and must
not retry a stale write automatically.

Content/title-only edits preserve the next target. A timing edit starts a new
rule from the accepted edit time; a changed interval gets a new anchor. Enable
resumes the saved rule with the same latest-only catch-up policy. A completed
schedule is terminal; create a new identity to schedule again.

An update or disable committing before trigger CAS prevents that old occurrence.
A trigger committing first owns its target permanently; the stale mutation
conflicts and a later successful disable affects future occurrences only. The
same rule applies to delete. New revisions never alter an admitted payload or
Workflow execution contract.

## Target: Workflow

This is an implementation dependency within #85. The minimum durable request is
Workflow-owned; neither a serialized `WorkflowProgram` nor a live context is its
persistence format:

```text
ScheduledWorkflowStart {
  trigger_id, start_id, run_id, binding, accepted_at,
  definition: { id, digest, compiler_contract_version },
  args, execution_limits,
  execution_contract: { workspace_binding, root_authority, dependency_manifest },
  observed_resource_revision,
  state: Accepted | Claimed | Settled(outcome)
}
```

The native Workflow owner prepares a bounded `pinned_start_spec` when the target
is created or explicitly rebound by an authorized update. It resolves/validates
the definition and input contract through existing resource composition, then
captures semantic identities and limits. If that preparation cannot be performed,
creating/rebinding the Workflow target fails; an ID/digest alone is insufficient.
The spec is immutable within a schedule revision. Due admission copies it, args
and binding into `ScheduledWorkflowStart` and commits the unique start/run identity
with schedule advancement. This also works through StorageOnly; it does not
promise that external resources are still available at execution time.

| Dependency | Persist at admission | Reconstruct/check before Claim |
| --- | --- | --- |
| Program | Native definition ID/digest, compiler-contract version, input digest/args, execution/agent/retained-output/time limits | Compile the currently available definition through WorkflowCatalog; require the same identity and limits. A missing/changed definition or incompatible compiler contract produces a durable `RejectedBeforeStart`. |
| Tools and Agent profiles | Bounded dependency manifest: exact selectors, tool origin/schema/materialization identities, effective profile/model/endpoint parameters, instruction/Skill/project-instruction identities, authorized environment identities, extensions and execution policy | Reuse native frozen-composition/profile digests where available; fill only missing Workflow-specific manifest fields. Require the same effective execution contract and current authorization for every capability. A global numeric resource revision is audit evidence, not an equality check across restarts. |
| Workspace and authority | Exact native execution binding and user/root authority reference; no inherited Human Attempt or ToolCall | Reacquire the existing binding and validate current root permissions. Permission removal or a changed binding rejects; newer configuration never widens the recorded capability set. Ordinary workspace file contents are execution input, not a checkpoint of the workspace. |
| Live resources | No credentials, connections, processes, registry pointers, handles or cancellation tokens | Materialize current credentials for the same provider identity, MCP/tool bindings, model adapters and SubagentRegistry through their native owners. Rotation is allowed only when semantic identity and authorization remain satisfied. |

Reconstruction is allowed to consult current authoring/configuration. It is
allowed to **fail** rather than reproduce an old environment. Do not persist an
executable closure, revive deleted capabilities, keep an archive of credentials,
or substitute a newer definition. Bound the manifest by existing Workflow
node/resource limits and reject an oversized spec during preparation. S4 must
enumerate the concrete fields covered by each reused digest before code lands.

The Workflow owner prepares the executable off to the side, then under native
configuration/publication and lifecycle fencing revalidates the exact prepared
contract and CASes `Accepted -> Claimed`. Keep the immutable prepared resources
alive across that boundary. No node execution or child start occurs during
preparation. After claim, the ordinary native capability/cancellation rules still
apply. Share existing block execution and child supervision via a small invocation
origin (`Tool` or `Scheduled`); do not fabricate a model ToolCall or Human Attempt.

Recovery of an `Accepted` request repeats that preparation and validation. An
identity/authority mismatch or permanent unavailability commits
`Settled(RejectedBeforeStart { reason })` with zero execution effects. Transient
availability/capacity failure keeps Accepted with bounded retry and a visible
waiting reason; explicit cancellation can settle it without execution. Dependency
changes cannot turn a prior request into a different run. Admitted requests retain
their own contract when the schedule is edited or deleted.

Commit `Claimed` before the first execution effect. Recovery must never start a
second executor for a Claimed request: reconcile native child/execution evidence,
or settle `Interrupted/OutcomeUnknown` after handling outstanding native ownership.
A crash immediately after claim can leave work unexecuted; report the uncertainty.
Full graph checkpoint/resume is outside #85. Failure/rejection after acceptance
does not refund or refire the occurrence; it is a Workflow-owned outcome. Rebinding
or disabling the recurring schedule requires an explicit schedule operation.
Best-effort `WorkflowStarted` events cannot replace these durable states.

## Target: Conversation inbound

Use `UserSource::Runtime` and ordinary `InboundKind::Message` without extending
the canonical message schema. The native admission adapter owns a reserved
correlation namespace derived from TriggerId and a deterministic MessageId. The
existing durable `inbound_correlation` binds this to the allocated inbound sequence;
the atomic trigger receipt records the same identities. Ordinary callers cannot
write Scheduler-reserved correlation/identity values. A body label is display
data and never authorization or proof of provenance.

The bounded body contains occurrence time and the user-authored text. Historical
producer correlation survives schedule deletion and display-history pruning under
the ordinary inbound retention contract; native receipt reads may report pruned
detail without fabricating it. Scheduled input does not mint Human authority for
Goal or Schedule control and never creates a special Agent Loop path.

Busy conversations accept into their ordinary pending inbox. No active attempt is
cancelled or steered by Scheduler; ordinary safe-boundary adoption applies. No
destination sequence is allocated by Scheduler and no canonical ledger row is
written by its code. The atomic inbox seam owns transcript/reference publication.

For a cold target, StorageOnly admits the ordinary durable inbox item without
loading an Agent. If node B remains resident while node A's schedule is due, A's
trigger still commits to A's database. Its visible state is **accepted, waiting
for target activation**, not overdue/target_busy. This is the v1 product choice:
durable admission works independently of runtime residency, while execution has
no bounded-start promise when another node holds that Session's runtime slot.

After admission, the manager may load the exact node using
[`load`](../src/local_runtime/session_runtime_manager/lifecycle.rs) when the Session
has no conflicting runtime and capacity permits. Never evict B, switch the
Session's selected node, or redirect to its current conversation. A later explicit
activation of A adopts the pending message through ordinary recovery. Capacity
and configuration failures are execution-wait reasons, not reasons to fire again.
Native owner reconciliation also finds pending Scheduler-origin inbox/Accepted
Workflow work after a missed activation hint or restart. Show this distinction in
creation/update acknowledgments and task details, including the bound node and
execution wait reason. Recurrence continues to mean one admission per selected
occurrence; waiting execution is not evidence that a trigger was missed.

## Relationship to Goal

Keep existing [Goal admission](goal-extension.md) unchanged. An active Goal may
continue through its own idle admission while a future schedule remains armed.
Scheduler does not create, resume, pause, complete or budget a Goal implicitly.
Any later scheduled Goal operation must call the Goal owner explicitly. A
scheduled message must not masquerade as a fresh Human instruction. Schedule
authorization is separate from Goal authorization; creating either domain does
not mint or renew authority in the other.

## Durability

Add the schedule table and narrow `ScheduleStore` capability to the existing
conversation backend. Keep schedule state, revision, next target and latest
trigger receipt in one transaction. One conversation-level control row owns the
next schedule ordinal and scheduler generation; every schedule write, including
delete/prune, advances that generation. Native Schedule command receipts retain
consumed Human authorization independently of display history. Workflow start
requests belong to the Workflow owner in the same database. The current Session
catalog is a [separately committed document](../src/local_runtime/session.rs);
it cannot participate in the firing transaction.

### Discovery and reconciliation

Enumerate the durable user Session graph and read bounded schedule headers
through native read-only allocation access. Scan one store/page at a time without
loading transcripts, providers or Agents. Read the earliest Enabled due time and
store generation in one read transaction. Maintain at most one earliest target
per conversation in a rebuildable index; page due rows when processing it.
The same pass asks the native inbox/Workflow owners for a bounded pending-activation
summary, including when no Enabled schedule remains. That summary is derived from
pending inbox/nonterminal Workflow ownership, not a duplicate execution queue. Feed it to
the manager's exact-node activation path; native owners still decide adoption or
claim. Thus losing a post-commit activation hint does not strand accepted work.

Three mechanisms serve different purposes:

- Startup takes a finite catalog/node snapshot and visits every listed root
  conversation, including those with no current schedules. It schedules the
  following authoritative pass even if the index is empty.
- Post-commit notifications mark a conversation dirty for prompt reread. Accept
  only index observations at least as new as that conversation's observed store
  generation. Do not let a stale in-progress scan overwrite a newer notification
  read; do not clear dirty work arriving after its scan frontier.
- A full authoritative pass begins at most 30 seconds after the previous pass
  finishes. It refreshes catalog membership and visits all conversations again,
  not only known index entries. UTC rechecking alone is not reconciliation.

Each pass has a stable finite inventory, bounded cancellable read attempts and
fair progress between scan pages and due batches. An unreadable database becomes
an explicit unavailable entry with last-success/error information; it is never
silently treated as empty and cannot stop visits to other stores. Probe it on
subsequent passes. Successful reads after repair restore discovery; a separate
native write-health fence still requires successful owner recovery before writing.
Remove an entry as deleted only from current catalog authority.

Let `Tpass` be the enforced upper bound to complete a finite inventory pass under
its bounded read and dispatch budgets. A schedule committed just after its store
was visited, or in a node created just after the catalog snapshot, is discovered
within `2 * Tpass + 30s` while its store remains readable and the process runs.
The implementation must derive `Tpass` from inventory size and bounded operation
budgets, expose scan progress/health, and test that bound with manual clocks.
It must not advertise a fixed 30-second discovery SLA for arbitrarily many
conversations. Unavailable stores have an explicit availability exception; once
readable, they rejoin the same bound. Generation fencing plus periodic rereading
closes both notification-loss and scan/write races without a new durable index.

Measure startup/full-pass duration, opens, bytes and maximum resident/index memory
at 100, 1,000 and 10,000 historical conversations, with sparse active schedules,
one unreadable store and writes during traversal. Record supported-scale latency
before release. Add a durable acceleration index only if these measurements
justify it; such an index cannot replace the native authority.

### Session deletion and lineage

Choose explicit blocking semantics: **Enabled schedules and nonterminal scheduled
Workflow starts block Session deletion.** Disabled/Completed schedules and settled
start receipts are ordinary data removed with the Session's existing database
cleanup. The user disables/deletes active schedules and cancels/settles pending
Workflow work through their owners before requesting deletion again. Deletion
preview never silently disables a schedule or cancels an Accepted start.

Extend [`session_deletion.rs`](../src/local_runtime/session_deletion.rs) ownership
preflight with counts/identities of Enabled schedules and Accepted/Claimed starts;
include their native generations in the delete-preview revision. A Workflow's
unknown logical outcome is not proof that its children/processes are physically
settled. Preserve all existing native ownership barriers until that proof exists.
Already accepted ordinary inbound follows the existing confirmed Session deletion
policy; deleting its owning Session is not delivery to a different target.

Perform an initial non-destructive blocker check. For an actual delete, the manager
sets `retiring_sessions` under the shared registry boundary, refuses new storage,
trigger, claim and runtime admissions, and waits for **all** Session storage flights
and runtime writers to retire. Existing root ownership freeze and sorted
ConversationExclusion then protect fresh preflight, revision validation and the
catalog tombstone commit. Required authority changes invalidate a stale preview.

If a trigger wins first, fresh deletion preflight sees its committed schedule and
target ownership; a recurring Enabled schedule or Accepted Workflow blocks deletion.
If retirement wins first, trigger admission is refused. A one-shot inbound already
accepted is handled as ordinary Session content under the refreshed delete revision.
After the catalog tombstone commits, stale timers cannot reopen residue. Cleanup
failure remains the existing committed-cleanup-pending outcome, never restored
scheduling authority. Cancellation/unproven writer retirement retains the existing
closed-admission behavior; do not force deletion around a StorageOnly flight.

Fork/clone copy canonical history according to existing rules, but do not copy
schedule/control authority, command receipts or Workflow start requests into the
new conversation. Historical scheduled messages remain history, not active timers.
Do not add a scheduler database, sidecar, queue, compatibility migration or fallback
authority. Follow the native schema-version policy for the new tables.

## Recovery

| Durable point reached before failure | Required restart behavior |
| --- | --- |
| Due only in memory; transaction absent/rolled back | Recompute from saved state; latest-only catch-up may admit one occurrence. |
| Inbound and schedule transaction committed; wake/result lost | Return existing evidence; ordinary inbox recovery owns adoption. No new message. |
| Workflow request and schedule transaction committed; worker not yet claimed | Reconstruct and validate the pinned contract; claim once or durably reject before execution. |
| Workflow claim committed; executor/terminal state uncertain | Workflow owner reconciles native evidence or reports unknown/interrupted; Scheduler never replays it. |
| Index/activation/display notification missing | Authoritative passes rediscover schedules and native pending work; refetch display. A missing Journal/UI entry is not permission to fire. |

Transient pre-admission writer/capacity refusal keeps the original durable due
obligation and retries after a fixed 30-second local backoff, or earlier on an
owner availability notification. Another node's runtime residency does not refuse
cold durable admission. There is one driver, not one retry task per schedule.
Permanent pre-admission input/authority refusal disables the schedule with a
bounded reason through a revisioned mutation. A Workflow dependency rejection
after acceptance belongs to that start request; it cannot roll back the trigger.
Durability errors fence the affected admission path and require recovery; they
must not become a hot automatic retry loop. An uncertain commit is reconciled by
reading authoritative receipt/state before another admission attempt.

## Cancellation / disable / drain

Disabling or deleting a schedule prevents future uncommitted occurrences. Already
accepted messages, Workflow starts and executions retain native ownership. Session
deletion follows the blocking contract above. A client disconnect cancels neither
a schedule nor an admitted target.

Drain closes Scheduler command/due admission first, using a gate shared by final
trigger commit. It cancels the wake wait, settles admitted transaction holders and
all StorageOnly flights and activation operations, and joins the driver. Work
already transferred then drains through native target owners before stores close.
Accepted but unclaimed work may remain durable for restart. Do not hold a commit
permit while waiting for target acquisition; reacquire and recheck afterwards.
The fence takes effect after any pre-existing commit holders leave, so no trigger
can commit after the established boundary. Zero driver/wake tasks and no orphaned
storage/activation flight are required for successful quiescence. Durable Enabled state
survives shutdown.

## Timer implementation

Use existing [`RuntimeClock`](../src/runtime/types.rs) for UTC decisions and
[`MonotonicClock`](../src/runtime/monotonic.rs) for elapsed waits. One supervised
task waits on the earliest indexed deadline, change notification or shutdown.
Translate wall-time distance into a monotonic wait capped at 30 seconds; this
periodic clock recheck detects forward jumps even if the original deadline was
far away. Independently budget authoritative reconciliation passes as specified
above; an empty index still has a scan deadline. A backward jump cannot fire early
because admission rechecks UTC.

Use a fixed bounded batch (initially 64 candidates), yield between batches, and
order candidates by due instant then stable identity. Target preparation must be
bounded and cancellable through the native owner so one unavailable Session
cannot block the entire scheduler indefinitely. Neither the heap nor a `last_run`
variable determines firing. No per-schedule OS thread, per-schedule timer task,
leader election or configurable scheduling framework is needed.

## Event Journal / projection

Define native facts for created/updated/enabled/disabled/deleted and
occurrence-admitted/target-admission-failed. Include trigger identity, decision
time and destination receipt; avoid duplicating large payloads. “Became due” may
be a derived view rather than a durable event on every timer tick. The authoritative
ScheduleTriggerReceipt is required in the firing transaction. Optional Scheduler
Journal events are appended **after** that commit with stable native event IDs;
their absence or failure is an observation gap and cannot roll back or re-admit
the target. Existing mandatory target-owned writes, if any, remain required and
abort their transaction on failure. Projection/notification failures after commit
likewise leave authority intact. Do not promise a complete historical Journal if
the corresponding optional append was lost.

Expose typed create/get/list/update/enable/disable/delete operations through the
App Server and thin Runtime Client controls. Both model and client adapters call
the same native owner under the authorization contract below. Catalog reads and
ordinary schedule edits do not activate Agents; preparing/rebinding a Workflow
target uses native resource preparation without executing its graph. Mutation
results include committed revision and stale-conflict information.

The UI/TUI projects title, rule, explicit zone, next UTC/local time, lifecycle,
overdue timing, last admission receipt and bounded failure. A receipt says
“accepted by conversation/Workflow,” not “task succeeded.” Execution links use
native message/run identity. Show accepted-but-waiting activation separately from
an overdue uncommitted trigger. Reconnect refetches state; clients never own timers.

### Control authorization

Follow the existing [Goal Human-origin seam](../src/agent/execution/request.rs)
and [native adapter](../src/tools/native/goal.rs), with an independent Schedule
grant and durable consumption. No model argument may supply an authority origin,
Human message ID, destination binding, scope override or grant renewal.

| Caller / operation | Required authority |
| --- | --- |
| Trusted App Server user control | The existing authenticated product-control context authorizes the exact user Session/node. Record `RuntimeControl` and a stable native command ID. This route cannot be invoked by presenting text that claims to be a user. |
| Root model mutation | Composed root Schedule-control capability, current root execution authority, and a live native grant tied to the actual fresh Human MessageId and AttemptId. Resolve these through execution context, never model fields. |
| Model read/list | Composed root read capability, scoped to the caller's exact conversation. It grants no mutation authority. |
| Child Agent or Workflow Agent | No Schedule mutation grant, regardless of tool arguments or copied Human text. |
| Goal/Scheduler/other Runtime-origin work without a fresh Human handoff | No Schedule mutation grant. Historical Human messages and existing schedule receipts cannot supply one. |

At a fresh-inbound boundary, AgentExecution may bind a grant to the latest ordinary
Human input only if it is the last `InboundKind::Message` in the accepted batch and this is an
eligible root execution. Ordinary tool/model steps with no new inbound retain
that grant. New ordinary inbound replaces it; a non-Human last Message clears it.
Derived Context contributions do not create a new request or renew its grant.
Settlement, cancellation, drain or loss of the root capability also invalidate
the live grant. Merely adopting a Scheduler message never creates or renews one.
A model invocation with a missing/expired grant returns a typed authority refusal.
The native mutation boundary holds the grant's request-identity fence and rechecks
current root capability/cancellation alongside lifecycle admission. Recovery may
restore an exact admitted request's fresh provenance, but cannot search older
history to manufacture a grant or renew a consumed one.

V1 permits one successful Schedule mutation per initiating Human request. In the
same SQLite transaction as that mutation, insert a native command receipt keyed
by the Human MessageId, containing AttemptId, command identity, bounded command
digest and result identity/revision. An exact retry returns the prior result;
another mutation for that consumed Human request is refused, including after
restart or schedule deletion. Failed validation, semantic no-op, stale CAS or
pre-commit cancellation consumes nothing. An uncertain commit is resolved from that receipt,
not by allocating a new command identity. Keep these compact authority receipts
for the conversation's lifetime; they are not the prunable schedule display log.

The grant proves real Human origin, scope and non-reuse. It does **not** prove the
semantic meaning of arbitrary prose. Tool instructions must require explicit user
scheduling intent and prohibit deriving it from task complexity or reminder text,
as Goal does; no keyword classifier or claim of machine-verified intent is added.
Direct user controls express their intent through a concrete typed command.

Models may create a target in their own binding and, with fresh authority plus
expected revision, edit title, reminder text/Workflow args, timing, enabled state,
or delete it. A Workflow rebind goes through native spec preparation and the same
authority checks; models cannot write digests, resource manifests or budgets that
override native policy. No mutation changes Session/node ownership or enlarges
root authority. Future scheduled deliveries and Workflow execution carry execution
intent only, not authority to create, extend or rewrite schedules.
Due admission derives its execution intent from the committed Enabled schedule;
it neither requires nor consumes a live Human mutation grant. Current native
target execution eligibility and the pinned Workflow contract still apply.

## Deterministic tests

Use independently controlled UTC and monotonic clocks, transaction fault injection
and channels/barriers. Advancing Tokio time alone does not simulate UTC jumps.
DSH's [runtime tests][dsh-runtime-tests] and [Cron tests][dsh-cron-tests] provide
useful case ideas; do not import their semantics without the decisions above.

| Required proof | Fixture / assertion |
| --- | --- |
| At fires once | Two wakes plus reopen produce one trigger receipt and one inbound sequence. |
| Interval survives restart | 10:00/10-minute anchor, stop/reopen at 10:37, one 10:30 admission and next 10:40; repeat reopen changes nothing. |
| Catch-up is bounded | Long downtime admits at most one occurrence per schedule; no iteration over missed intervals. |
| Cron policy | Numeric grammar, DOM/DOW stars/full ranges, leap day, impossible date, explicit zones, DST fixed/wildcard gap/overlap, reverse search, timezone-data change and exhaustion. |
| Concurrent wakes | Barrier before CAS: one commits; the other reports stale/already-admitted. |
| Mutation races | Both orderings of disable/update/delete versus admission; stale clients cannot overwrite advancement. |
| Native transfer | Inbound uses ordinary pending/adoption; Workflow request/claim identity remains stable across retries and restart. |
| Atomicity | Fail after target-row insertion but before schedule update, and before commit: neither side survives. Kill after commit but before wake: both survive. |
| Correlation | Identical retries return the same sequence; changed timestamp/payload with the same trigger is refused. |
| Clock jumps | Rollback during writer acquisition does not fire; forward jump is detected within the capped wait. |
| Hot/cold writer ownership | Race StorageOnly reservation/open/commit/close against load and unload in both orders; one writer, ordered publication, no stale capability and no replacement after uncertain retirement. |
| Non-current node | Keep B resident indefinitely while A becomes due; A receives exactly one durable admission and shows waiting-for-activation, B is untouched, and later loading A adopts that same message. |
| Residency and lineage | Cold admission, runtime capacity/configuration waits, and no active schedule/control/start authority inherited on fork/clone. |
| Drain | Pause storage acquisition, trigger commit and activation at barriers; the fence closes, existing holders settle, and every storage/load/driver flight is accounted for. |
| Disconnect/observation | Drop TUI/Web clients and fail projection notification; native schedules and target admissions retain the same behavior. |
| Workflow crash windows | Before/after claim and before/after effects; no second executor for a claimed start, honest unknown outcome where recovery cannot prove completion. |
| Workflow reconstruction | Change/remove definition, tool schema/origin, Skill/profile, root permission or workspace binding between Accepted and Claim; zero graph effects and a durable rejection. Unrelated resource revision changes and same-provider credential rotation do not force a false mismatch. |
| Claim race | Change configuration after preparation but before claim; the owner either retains/revalidates the exact admitted execution contract or refuses. Restart after refusal cannot claim that request. |
| Index loss/races | Drop the only creation notification while index is empty; mutate just after store visit; create a node after catalog snapshot; deliver an older scan after a newer dirty read. Discovery satisfies the documented pass bound. |
| Partial discovery failure | One unreadable store does not hide other schedules; repair/reopen restores discovery within the bound; no assumption that an unreadable store contains zero schedules. |
| Journal boundary | Inject required receipt failure before commit: neither target nor advancement survives. Inject optional Journal/notification failure after commit: one target survives and is never refired. |
| Session deletion | Enabled and Accepted/Claimed blockers are visible before deletion; both trigger-versus-retirement orderings have one winner; fresh generations reject stale preview; cleanup failure cannot revive schedules. |
| Quotas and non-reuse | More than 256 sequential create/complete/delete operations still allow creation; 256 simultaneous Enabled/Disabled rows reject the next; pruning/deletion never reuses IDs or removes native deduplication authority. |
| Model authority | Forged/missing Human origin, child calls, Runtime-only activation, intervening fresh Runtime input, revoked root capability and reused Human receipts all refuse mutation. Exact retry after commit/crash returns the existing result; failed CAS consumes no grant. |

## Documentation

Keep this proposal linked from the development plan. Each implementation slice
updates architecture/invariants with the actual owner and transaction boundary,
and the App Server/control documentation with only implemented commands. Add
time/misfire/DST examples and describe the difference between completed schedules,
accepted messages, Workflow settlement and Goal completion. Remove superseded
future-only descriptions once the complete native contract exists. Document the
accepted-but-waiting node behavior, deletion blockers, quota/retention distinction,
reconciliation latency bound and one-mutation-per-Human-request tool contract in
their owning product/API sections.

## Non-goals

Fleet; distributed/HA scheduling; leases or leader election; external notification;
arbitrary target callbacks; DSH/Cordis compatibility; scheduler-owned execution;
cross-conversation delivery; automatic cancellation after disable; importing active
schedules through fork/clone; Workflow graph checkpoints; a second durable queue.

## Acceptance criteria and implementation sequence

| Slice | Owned changes | Exit evidence |
| --- | --- | --- |
| S0: ownership contract prerequisite | Map manager StorageOnly/load/unload/delete transitions, product/allocation locks, hot/cold commit permits and post-commit publication; enumerate all existing writer entry points. | Verify the transition/lock table against source before S1; no Scheduler SQL writer bypasses the native manager. |
| S1: At/inbound vertical slice | New Scheduler domain/capability, finite StorageOnly path in the existing manager, compound inbox admission, authoritative receipt/generation, one driver and typed user-control entry. | A controlled-clock one-shot works for hot and cold targets; writer races, rollback/restart, duplicate wake and drain tests pass. B's residency cannot prevent A's durable admission. |
| S2: lifecycle and management | CAS updates, authority-bound control receipts, independent quotas/retention, periodic catalog/store reconciliation and explicit deletion/fork rules. | Lost notifications meet the pass bound; deletion has one winner; repeated historical creation does not exhaust active quota; scan-scale measurements recorded. |
| S3: interval and Cron | Add pure occurrence calculations, latest-only policy and the Cron dependency gate; bound payloads, lists and wake batches. | Calendar and restart fixtures pass; unsupported Cron cannot be authored or decoded as executable. |
| S4a: Workflow contract prerequisite | Inventory reusable executor, frozen-profile/materialization and supervision seams; fix the exact bounded ScheduledWorkflowStart fields/digest coverage and reconstruction refusal rules. | Demonstrate a matching reconstruction and an Accepted request rejected after dependency change, without introducing graph checkpointing or serialized runtime handles. |
| S4b: Workflow native start | Add native request/claim/settlement and scheduled origin, share existing execution, and extend deletion/retirement ownership inspection. | One run identity across crash windows; no second executor after claim; truthful rejection/unknown outcomes and physical-ownership barriers. |
| S5: product and final conformance | Thin model adapters with fresh Human authority, TUI/Web projections, protocol generation, documentation and complete matrix. | Both targets satisfy #85; fresh-authority and execution-wait behavior are visible; required checks pass; no Fleet/distributed owner added. |

File names for new modules are proposed boundaries; keep SQL, temporal arithmetic,
driver lifecycle and target execution in their respective owners. Extend backend
contracts with semantic operations, not a generic scheduler repository API.

The main estimate risks are cold-store ownership/discovery and Workflow resource
reconstruction. S0 is a prerequisite to S1, and S4a precedes executor changes in
S4b. The chosen semantic contracts above are the implementation target; concrete
resource-manifest field coverage, scan budget measurements and Cron's published
version/fixture results remain explicit engineering gates. Do not expand these
gates into a general lock service, execution snapshot store or Workflow redesign.

## Depends on

The native Workflow and Goal boundaries from #83/#84 (including later Goal
lifecycle changes), existing durable inbox/correlation, Session access and
residency fencing, and runtime supervision. No Fleet dependency is introduced.

## Blocks

M10 product/soak and release acceptance remains blocked until the complete #85
contract, including native target transfer and deterministic recovery, is proven.
This research/design branch does not close #85.

[dsh-service]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/src/index.ts
[dsh-runtime]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/src/runtime.ts
[dsh-storage]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/src/storage.ts
[dsh-domain]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/src/domain.ts
[dsh-spec]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/docs/subsystems/schedule.md
[dsh-runtime-tests]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/tests/runtime.spec.ts
[dsh-cron-tests]: https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/schedule/schedule/tests/cron.spec.ts
[croner-readme]: https://github.com/Hexagon/croner-rust/blob/24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83/README.md#daylight-saving-time-dst-handling
[croner-parser]: https://github.com/Hexagon/croner-rust/blob/24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83/src/parser.rs
[croner-lib]: https://github.com/Hexagon/croner-rust/blob/24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83/src/lib.rs
[croner-cargo]: https://github.com/Hexagon/croner-rust/blob/24c9bbb02dbcb498a8bbf31a33f1e6416e75ac83/Cargo.toml
