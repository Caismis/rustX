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

The recommended first vertical slice is `At -> ConversationInbound`. Extend the
same transaction and driver with fixed-rate intervals, then Cron, then the native
Workflow admission seam described below. The complete #85 milestone includes both
target types; the inbound slice alone does not complete the issue.

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
| Scope and cold Sessions | One Host timer; the Session controller restores the original Session when due. Catalog/history reads do not activate it. [Service][dsh-service], [runtime][dsh-runtime] | One App Server-owned driver; exact native Session/node binding, with explicit residency conflicts. |
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
    -> resolve due target through existing Session/access/residency owners
    -> lifecycle-guarded ScheduleStore transition in target conversation SQLite
         [occurrence CAS + schedule advance + native target admission + fact]
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
        | Workflow { definition_id, definition_digest, args },
  state: Enabled | Disabled | Completed,
  next_due_utc, last_committed_trigger?, last_failure?
}
TriggerId = (ScheduleId, admission_revision, occurrence_utc)
```

The binding identifies a native user Session graph node and its conversation;
the destination cannot follow UI focus or silently switch to the current node.
Both target kinds execute in that same conversation ownership domain. V1 excludes
cross-conversation targets and child-Agent targets. Changing the binding requires
creating a new schedule. IDs are never reused, including after deletion.

`revision` advances on every committed mutation, including occurrence advancement;
a semantic no-op preserves it. `admission_revision` is the revision read before
the successful firing transaction. `Completed` means no future temporal
occurrence remains, not that target execution succeeded. Disabled state preserves
its reason; failures are bounded diagnostics, not another execution state machine.

Initial limits to implement and exercise: title at most 120 Unicode scalar values;
text or serialized Workflow arguments at most 16 KiB; at most 256 retained
schedules per conversation; list page size 1–100. These are native admission
limits, not transport truncation. Native target limits still apply independently.

## Ownership separation

Preserve the issue's division: Goal owns WHAT, Workflow HOW, Scheduler WHEN,
Subagent WHO, and Tool DO. Scheduler owns schedule definitions, revisions and
occurrence evidence. Target owners own execution, cancellation and recovery.
The driver receives narrow admission capabilities, not a model adapter or a
general-purpose SQL callback.

## Trigger linearization

For inbound, extend the existing atomic Goal/inbox pattern with a dedicated
`accept_scheduled_inbound` operation:

1. Resolve the exact destination and acquire its existing operation/lifecycle
   admission guard. Prepare bounded input without performing target work.
2. Under the mailbox publication boundary, begin one SQLite immediate transaction.
   Read the current schedule and validate identity, revision, `Enabled`, binding,
   and due state against a freshly sampled UTC decision time.
3. Derive the selected occurrence and immutable `TriggerId`. Call
   `accept_inbound_tx` with ordinary `InboundKind::Message`, runtime provenance,
   and a stable correlation derived from that trigger.
4. Advance the schedule, increase its revision, store its last trigger/admission
   receipt, and append the bounded Scheduler fact in **the same transaction**.
5. Commit. Only now is the occurrence fired. Publish observations and notify the
   existing coordinator after commit; a lost wake does not undo accepted work.

For Workflow, step 3 instead calls a Workflow-owned transaction participant that
inserts the durable start receipt. It is not a subsequent non-idempotent start
call. The remaining transaction and firing boundary are the same.

Do not nest public methods that start separate transactions. Introduce narrow
typed compound operations and backend-private helpers, following the inbox seam.
Suggested lock order is target access lease -> target lifecycle admission ->
mailbox/publication boundary -> SQLite; driver inventory locks are released before
target acquisition. No target restoration or asynchronous I/O occurs under SQLite
or the driver inventory lock. Confirm this order against all control/delete paths
when implementing the first slice.

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
Workflow definition snapshot.

## Target: Workflow

This is an implementation dependency within #85, not a thin adapter around
`run_foreground`. Add a Workflow-owned durable start contract before exposing the
target in any schema:

- `admit_scheduled_start_tx(trigger, frozen_definition_ref, args, execution_binding)`
  validates the native definition and writes a unique start request in the same
  conversation transaction as schedule advancement. Return a stable native run
  identity. Repeating the trigger returns that identity or a content conflict.
- A Workflow worker acquires the accepted start request and enters the existing
  executor with explicit scheduled provenance. Refactor common invocation context
  so it does not invent a Human Attempt or model ToolCall for scheduled work.
  Definition/capability freezing remains owned by Workflow/resource composition.
- Bind the schedule to a native definition identity and digest. A changed or
  missing definition requires explicit schedule revision; never silently execute
  replacement code. Persist sufficient native frozen-input evidence for the
  Workflow owner to recover an accepted request without consulting a changed
  authoring file.

The bounded first contract is `Accepted -> Claimed -> Settled`, with durable
owner-controlled transitions. Commit `Claimed` before the first execution effect.
Recovery may claim an `Accepted` request; it must never start a second executor
for a `Claimed` request. If interrupted execution cannot be recovered using native
child/execution evidence, settle it as `Interrupted/OutcomeUnknown`, retaining its
run identity and performing no automatic rerun. A crash immediately after claim
may therefore leave work unexecuted; report that uncertainty honestly. Full graph
checkpoint/resume is a separate Workflow capability, not a Scheduler guarantee.

Failure after durable admission is a Workflow-owned start/settlement failure and
does not refund or refire the occurrence. Dependency/input refusal before the
transaction commits is not a firing. Durable start evidence cannot be replaced
by current best-effort `WorkflowStarted` journal events. Tests must cover crash
before claim, after claim, after first effect and before terminal publication.

## Target: Conversation inbound

Use `UserSource::Runtime` and ordinary `InboundKind::Message`; add typed Scheduler
provenance/correlation through the native metadata path. The bounded body carries
the schedule identity, occurrence time and user-authored text as data. Scheduled
input is not new Human authorization and cannot inherit Goal-creation authority.
Do not classify it as a special Agent Loop or compaction message.

Busy conversations accept into their ordinary pending inbox. No active attempt is
cancelled or steered by Scheduler; ordinary safe-boundary adoption applies. No
destination sequence is allocated by Scheduler and no canonical ledger row is
written by its code. The atomic inbox seam owns transcript/reference publication.

For a cold target, load the exact node through
[`SessionRuntimeManager::load`](../src/local_runtime/session_runtime_manager/lifecycle.rs).
The current manager permits one resident conversation per Session. If another node
of that Session is resident, report `target_busy` and keep the schedule due; do not
evict it, switch current-node selection, or redirect delivery. Capacity/configuration
failures are likewise explicit. Recheck the clock and revision after restoration.

## Relationship to Goal

Keep existing [Goal admission](goal-extension.md) unchanged. An active Goal may
continue through its own idle admission while a future schedule remains armed.
Scheduler does not create, resume, pause, complete or budget a Goal implicitly.
Any later scheduled Goal operation must call the Goal owner explicitly. A
scheduled message must not masquerade as a fresh Human instruction.

## Durability

Add the schedule table and narrow `ScheduleStore` capability to the existing
conversation backend. Keep schedule state, revision, next target and latest
trigger receipt in one row/transaction. Workflow start requests belong to a
Workflow-owned table in that same database. The current Session catalog is a
[separately committed document](../src/local_runtime/session.rs), so it cannot be
part of the firing transaction.

Discover schedules by enumerating the durable user Session graph and reading
bounded schedule headers from its conversation stores through the existing
read-only access/archive infrastructure. Scan one store/page at a time without
loading transcripts, providers or live Agents. Maintain at most the earliest
target per conversation in a rebuildable in-memory index, then page due rows for
that conversation. Schedule and Session changes invalidate this index after
commit. Startup scanning is O(conversations + schedule headers); measure it before
adding any durable acceleration index. An unreadable store is surfaced as an
explicit unavailable target, not silently treated as an empty schedule set.

Loaded management operations use the existing writer capability; cold management
uses the same Session access/allocation fencing without composing an Agent.
Neither may open a second independent writer around a resident owner. Session
deletion fencing also covers schedule management and in-flight admission; a
tombstoned Session cannot be restored by a stale timer. Fork/clone copy canonical
history according to existing rules, but do not copy active schedules, trigger
authority or Workflow start requests. A copied historical message is not a timer.

Do not add a scheduler database, JSON sidecar, external queue, compatibility
migration or fallback authority. Follow the repository's schema-version policy
for the new native layout.

## Recovery

| Durable point reached before failure | Required restart behavior |
| --- | --- |
| Due only in memory; transaction absent/rolled back | Recompute from saved state; latest-only catch-up may admit one occurrence. |
| Inbound and schedule transaction committed; wake/result lost | Return existing evidence; ordinary inbox recovery owns adoption. No new message. |
| Workflow request and schedule transaction committed; worker not yet claimed | Workflow owner may claim the same accepted request exactly once. |
| Workflow claim committed; executor/terminal state uncertain | Workflow owner reconciles native evidence or reports unknown/interrupted; Scheduler never replays it. |
| Display notification missing | Refetch native state; a missing journal/UI entry is not permission to fire. |

Transient pre-admission residency/capacity refusal keeps the original durable due
obligation and retries after a fixed 30-second local backoff, or earlier on a
relevant owner availability notification. There is one driver, not one retry task
per schedule. No catch-up backlog is created. Permanent input/definition refusal
disables the schedule with a bounded reason through a revisioned mutation.
Durability errors fence the affected admission path and require recovery; they
must not become a hot automatic retry loop. An uncertain commit is reconciled by
reading authoritative receipt/state before another admission attempt.

## Cancellation / disable / drain

Disabling or deleting prevents future uncommitted occurrences. Already accepted
messages, Workflow starts and executions belong to their native cancellation and
Session-deletion policies. A client disconnect cancels neither a schedule nor an
admitted target.

Drain closes Scheduler command/due admission first, using a gate shared by final
trigger commit. It cancels the wake wait, settles admitted transaction holders and
any cold-load operations, and joins the driver. Work already transferred then
drains through native target owners before stores close. Do not hold a commit
permit while waiting for restoration; reacquire and recheck after it returns.
The fence takes effect after any pre-existing commit holders leave, so no trigger
can commit after the established boundary. Zero driver/wake tasks and no orphaned
restoration flight are required for successful quiescence. Durable Enabled state
survives shutdown.

## Timer implementation

Use existing [`RuntimeClock`](../src/runtime/types.rs) for UTC decisions and
[`MonotonicClock`](../src/runtime/monotonic.rs) for elapsed waits. One supervised
task waits on the earliest indexed deadline, change notification or shutdown.
Translate wall-time distance into a monotonic wait capped at 30 seconds; this
periodic clock recheck detects forward jumps even if the original deadline was
far away. A backward jump cannot fire early because admission rechecks UTC.

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
be a derived view rather than a durable event on every timer tick. Journal and UI
failure cannot undo admission; the schedule/target tables remain authoritative.

Expose typed create/get/list/update/enable/disable/delete operations through the
App Server and thin Runtime Client controls. Model tools, if composed, call the
same owner, bind to the caller's user conversation, and require explicit user
scheduling intent; children cannot grant themselves this capability. Full catalog
reads and edits must not start Agents. Mutation results include committed revision
and stale-conflict information. Keep APIs clearly proposed until schema/code land.

The UI/TUI projects title, rule, explicit zone, next UTC/local time, lifecycle,
overdue timing, last admission receipt and bounded failure. A receipt says
“accepted by conversation/Workflow,” not “task succeeded.” Execution links use
native message/run identity. Reconnect refetches state; clients never own timers.

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
| Clock jumps | Rollback during cold restore does not fire; forward jump is detected within the capped wait. |
| Residency and lifecycle | Cold exact-node restore, another node resident, capacity refusal, Session deletion race, and no schedule inheritance on fork/clone. |
| Drain | Pause target restoration and trigger transaction at barriers; fence closes, existing holders settle, all driver/load work is accounted for. |
| Disconnect/observation | Drop TUI/Web clients and fail projection notification; native schedules and target admissions retain the same behavior. |
| Workflow crash windows | Before/after claim and before/after effects; no second executor for a claimed start, honest unknown outcome where recovery cannot prove completion. |

## Documentation

Keep this proposal linked from the development plan. Each implementation slice
updates architecture/invariants with the actual owner and transaction boundary,
and the App Server/control documentation with only implemented commands. Add
time/misfire/DST examples and describe the difference between completed schedules,
accepted messages, Workflow settlement and Goal completion. Remove superseded
future-only descriptions once the complete native contract exists.

## Non-goals

Fleet; distributed/HA scheduling; leases or leader election; external notification;
arbitrary target callbacks; DSH/Cordis compatibility; scheduler-owned execution;
cross-conversation delivery; automatic cancellation after disable; importing active
schedules through fork/clone; Workflow graph checkpoints; a second durable queue.

## Acceptance criteria and implementation sequence

| Slice | Owned changes | Exit evidence |
| --- | --- | --- |
| S1: At/inbound vertical slice | New `src/scheduler.rs` domain and `src/durable/scheduler.rs` capability; SQLite schedule module; compound inbox operation; one runtime driver; native control entry. | A controlled-clock one-shot works end to end; rollback/restart/duplicate-wake/drain tests pass before adding recurrence. |
| S2: lifecycle and management | Wire App Server/Session manager ownership, exact-node cold load, CAS update/disable/delete, catalog discovery and Session deletion/fork rules. | Cold targets work; conflicts are explicit; disconnect and drain do not change durable intent. |
| S3: interval and Cron | Add pure occurrence calculations, latest-only policy and the Cron dependency gate; bound payloads, lists and wake batches. | Calendar and restart fixtures pass; unsupported Cron cannot be authored or decoded as executable. |
| S4: Workflow native start | Add Workflow-owned durable request/claim/settlement and scheduled provenance; share native execution without a fabricated ToolCall. | All crash windows retain one run identity, never repeat claimed work, and expose truthful failure/unknown state. |
| S5: product and final conformance | Thin model/control adapters, TUI/Web projections, protocol generation, architecture/invariant updates and complete matrix. | Both target types satisfy #85; required repository checks pass; no Fleet or distributed owner added. |

File names for new modules are proposed boundaries; keep SQL, temporal arithmetic,
driver lifecycle and target execution in their respective owners. Extend backend
contracts with semantic operations, not a generic scheduler repository API.

The main estimate risks are cold-store discovery/ownership and the Workflow start
contract. Resolve these in S1/S2 and S4 respectively, not by claiming that the
existing foreground Workflow API already provides durable handoff. Cron's exact
published version and fixture results remain an explicit implementation gate.

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
