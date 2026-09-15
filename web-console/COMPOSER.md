# Composer context docks (WEB-04)

The composer column is one stack with a fixed order:

```text
Todo
Goal
Queue
Composer
```

`ComposerContextStack` owns that order, the shared column width and the vertical
rhythm. Each dock owns only its own presentation state (Todo/Queue disclosure,
the Goal draft, action feedback and a control lock pending authority). A dock
appearing or disappearing transfers nothing to another dock, and the whole stack is
keyed by Session, so no dock state crosses Session views. Presentation is adapted
from the pinned DeepSeek Harness `TodoPanel`, `GoalBar`, `QueueDock` and
composer-stack geometry; see [PROVENANCE.md](PROVENANCE.md).

## Authority

Every dock reads the replaceable authoritative `RuntimeClientSnapshot` through
`src/bindings/composer-context.ts`. The client still has no event fold: a
`session/event` invalidates, `session/snapshot` replaces. Transcript pages, Trace
entries and historical `todo`/Goal Tool facts stay visible in Chat and Trajectory
as history; they never feed a dock.

| Dock | Native owner | Projection | Controls |
| --- | --- | --- | --- |
| Todo | Conversation-owned `ConversationTodoList` | `snapshot.todos` | none |
| Goal | `GoalDomain` (durable) + process-local activation | `snapshot.goal` (`GoalView`) | `goal/control` pause, resume, edit objective, edit budget |
| Queue | Conversation inbound mailbox | `snapshot.inbound.pending`, `snapshot.attempt` | none |

No App Server protocol change was needed: all three facts were already typed in
the common snapshot.

### Todo

`snapshot.todos` is present exactly when the attached runtime composes the Todo
extension. Three states stay distinct:

- absent: no dock;
- composed with an empty list: a bounded "No current tasks" strip;
- composed with tasks: a collapsed per-status summary and a bounded list in native
  task order. Only native statuses render (`pending`, `in_progress`,
  `completed`); `deleted` tombstones are dependency targets, not current work.
  `blocked_by` is shown as the native relation (`after #1`), never as an invented
  status. An in-progress task shows its `active_form`.

The dock has no mutation. Todo tasks are never written to `settings.toml`,
`rustx.toml`, Agent TOML, `SessionPersistentState` or browser storage. Whether
Todo is composed is `agent.extensions.todo` configuration at User/Workspace or
named-Agent scope; there is no Session extension override and the dock offers no
configuration control.

### Goal

The dock renders the current `GoalSnapshot` — objective, durable phase, blocked
reason, consumed/budget rounds and durable revision — plus `GoalView.armed`.
Absent extension, no Goal and terminal `complete` occupy no composer space.
`armed` is process-local activation: an activation-only change updates the label
(`Ongoing` / `Inactive`) and never changes the displayed revision or an open draft.

Controls are exactly the ones GoalDomain assigns to users: pause, resume
(re-arm), edit objective and edit budget. Create, block and complete remain `/goal`
and model declarations; there is no clear.

Every mutation is `goal/control` `mutate` with the rendered authoritative
`GoalRef` as its CAS token. Nothing is ever retried, and no newer revision is ever
substituted into a sent mutation.

**Mutation outcome is not projection convergence.**

```text
goal/control applied or refused   -> outcome known
successful session/snapshot after -> authoritative Goal observed
```

`AppServerClient.controlGoal` reports both:

| Outcome | Meaning | Dock |
| --- | --- | --- |
| `applied`, `observed: true` | mutation committed and a later snapshot read succeeded | unlocked on the new observation |
| `applied`, `observed: false` | mutation committed, reread failed | old GoalRef stays rendered; controls locked |
| `rejected`, `observed: true` | native refusal (e.g. stale CAS), reread succeeded | reason shown; unlocked on the reread Goal |
| `rejected`, `observed: false` | native refusal, reread failed | reason shown; controls locked |
| `uncertain` | response lost after transmission | uncertain notice; controls locked; the global diagnostic remains |
| `obsolete` | connection or attachment changed | nothing applies to the current view |

`observed` is true only when a snapshot request issued after the outcome succeeds
for the same attachment; a coalesced in-flight refresh is re-marked dirty so it
cannot count. A locked dock unlocks only when the client replaces the snapshot,
which happens only after a successful authoritative read (a later event refresh,
resync or reconnect) — never on request completion, a timer or a rerender. The
embedded `current` of a native `GoalRejection` is never adopted.

Revision-only changes (autonomous round admission) keep an open draft; a change to
the authoritative objective or budget drops the matching draft so it can never be
written over content its author did not see.

The browser validates only input grammar: a non-empty objective and a positive
integer budget. The round-budget range, the consumption floor and transition
legality belong to GoalDomain; a syntactically valid out-of-domain value is sent
and its typed refusal is shown. Goal controls never write extension enablement,
and disabling the extension deletes no Goal state.

### Queue and composer delivery

Queue rows are the native pending inbound items, in inbound sequence order, with
their provenance (for example a Goal continuation). The dock is read-only: queue
edit/remove and per-row steer belong to WEB-06.

`turn/start` and `turn/steer` dispatch to the same native `submit_inbound`
(`src/app_server/connection.rs`). The composer therefore has one delivery action,
labelled from the authoritative attempt: **Send** while idle and **Queue** while an
attempt is running, when input waits for the next safe-boundary drain. The former
separate Steer button implied a second mode rustX does not have and was removed.

A submission passes three presentation stages:

```text
turn/start in flight
    -> composer transport only ("Awaiting acknowledgement…"); not a Queue row,
       not counted as queued
inbound_accepted { message_id }
    -> may appear as an accepted provisional Queue row keyed by that MessageId
authoritative snapshot contains that MessageId (pending, messages or transcript)
    -> the native projection owns presentation; the provisional row is removed
```

Text, order and queue length never settle a provisional row. A lost `turn/start`
response has no MessageId: it creates no Queue row, keeps the `OutcomeUncertain`
diagnostic and is never replayed; a later authoritative snapshot shows whatever the
runtime committed. Accepted rows are shown only while an attempt runs, because an
idle send is admitted directly rather than queued.

## Lifecycle

Disconnect, remount, route change and unmount send nothing: they do not cancel
queued work, disarm or mutate Goal, modify Todo, settle a mutation or invent a
terminal state. Connection loss clears only accepted provisional rows (presentation,
never a claim about server work); the last observation stays visible but inert.
Reconnect and reload rebuild every dock from the attach snapshot; disclosure state
may reset. Old-generation and old-attachment results are fenced by the existing
client generation/target checks.

## Layout

Every dock is the composer card width minus four 8px dock insets and centred on the
same axis, in normal flow (no fixed or sticky positioning). Todo and Queue lists are
bounded at 180px; Goal text ellipsizes and wraps below its actions on narrow
viewports. The real-server browser test measures the alignment at 1440px and 390px.

## Relationship to #319 / PR #320

PR #320 (#319) replaces the ArtifactId user-upload path with Session-owned
workspace uploads and moves the App Server protocol to v4. This slice wraps the
existing `InputBar` without touching upload behaviour; its only composer changes
are the single delivery action and removal of the `steer` send flag. It is built on
`main` before #320 and must be rebased onto `main` after #320 merges, keeping v4
`session/upload` receipts and this single delivery action, then revalidated. No
v3/v4 or old/new upload compatibility path is added.
