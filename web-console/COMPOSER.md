# Agent Composer and native controls

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
| Queue | Conversation inbound mailbox | `snapshot.inbound.pending`, `snapshot.attempt` | exact pending edit/remove (WEB-06) |

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

The dock has no mutation. Todo tasks are never written to `rustx.toml`,
Agent TOML, `SessionPersistentState` or browser storage. Whether
Todo is composed is `agent.plugins.todo` configuration at User/Workspace or
named-Agent scope; there is no Session extension override and the dock offers no
configuration control.

### Goal

The dock renders the current `GoalSnapshot` — objective, durable phase, blocked
reason, consumed/budget rounds and durable revision — plus `GoalView.armed`.
Absent extension, no Goal and terminal `complete` occupy no composer space.
`armed` is process-local activation: an activation-only change updates the label
(`Ongoing` / `Inactive`) and never changes the displayed revision or an open draft.

Controls are exactly the ones GoalDomain assigns to users: pause, resume
(re-arm), edit objective and edit budget. Create, block and complete remain native
model declarations; the Web `/goal` opens these existing controls, and there is no clear.

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

The browser validates only input grammar and protocol representability: a non-empty
objective, and a budget that is a positive decimal integer the wire `u32` of
`GoalMutation::Budget.rounds` can represent exactly. The draft is parsed through
`BigInt` and refused above `4294967295`, so a value that would round, reach
`Infinity` or serialize as JSON `null` never becomes a typed mutation.

Everything semantic stays server-owned: the `1..=100` round-budget range, the
consumption floor and transition legality belong to GoalDomain. A representable
out-of-domain value is sent and its typed refusal is shown. Goal controls never
write extension enablement, and disabling the extension deletes no Goal state.

### Queue and composer delivery

Queue rows are the native pending inbound items, in inbound sequence order, with
their provenance (for example a Goal continuation). Exact pending edit/remove use the native inbound revision contract; presentation
never reorders or adopts mailbox entries.

`turn/start` and `turn/steer` dispatch to the same native `submit_inbound`
(`src/app_server/connection.rs`). **Send** while idle dispatches `turn/start`.
While the authoritative attempt is running, the delivery selector offers **Queue**
(`turn/start`) and **Steer (same mailbox)** (`turn/steer`). Both enter the same
native mailbox and drain at a safe boundary; Steer does not promise interruption,
priority, or a separate execution mode. Returning to idle always uses `turn/start`,
even if Steer was previously selected. The selected delivery is presentation only.

A submission passes three presentation stages:

```text
turn/start or turn/steer in flight
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

## Uploads

User uploads are Session-owned workspace uploads on App Server **v6**. The stack
wraps `app/agent/AgentComposer` and the pinned Harness card/footer seats: a draft transfers
through `session/upload` first, and either delivery action carries the resulting
typed `UploadReceipt`s as ordinary content.

```text
send(sessionId, text, uploadReceipts, delivery) -> turn/start or turn/steer
```

There is no `artifact/upload` user path or upload compatibility path. Upload receipts are content only:
they never participate in the accepted-`MessageId` identity contract above, and an
uncertain upload leaves its draft card uncertain rather than being replayed.

## Commands (WEB-05)

Slash commands are browser input grammar and presentation, not a server interpreter.
`app/commands/registry.ts` is the single closed command catalog. Stable `CommandId`
values are separate from labels and aliases (including Chinese spellings). A
leading slash reserves the whole trimmed draft: no arguments, interpolation or
shell grammar. Unknown/unsupported slash input is visibly refused and retained;
it never falls through to a model prompt. Inline slashes and URLs remain text.

`/` or the composer `+` opens discovery. Matching uses the pinned Harness ordered
subsequence scorer: prefix first, strongest alignment next, registry order for
ties. Filtering includes identity, label and aliases. Up/Down wraps, Enter picks,
Escape and outside pointer dismiss. The composer keeps focus during discovery;
selectors take search focus and return it on dismissal. IME Enter is untouched.
`+` preserves a non-command draft instead of replacing it. Commands with draft
uploads are refused rather than dropping attachments. A successful selector consumes
only the exact draft captured at invocation, if it has not since changed; dismissal
or failure keeps it. This covers exact tokens, aliases, fuzzy `/mdl`, and bare `/`
discovery alike. A successful `/tools` native read consumes its invoking draft while
keeping the capability panel open; consumption and panel dismissal are separate.

| Command | Typed integration |
| --- | --- |
| `/model` | `settings/model` + `settings/models`, then `settings/setModel` with an exact catalog model reference and native defaults |
| Approval mode control | Native Workspace Approval source-unit CAS; desired/published/running distinction and explicit Reload |
| `/compact` | Native `context/compact`, available with no active Attempt; pending inbound alone is permitted by native maintenance |
| `/new` | `session/create` using native Session cwd; attach/open only after success |
| `/fork` | Native exact user-boundary selection and independent `session/fork` |
| `/branch` | Native exact user-boundary selection and in-Session `session/branch`, stable execution-idle only |
| `/goal` | Focus existing GoalDomain-backed dock controls when a current Goal is visible |
| `/tools` | Read-only `resources/read` capability inspection |

`/settings` is omitted from the typed grammar; Settings remains available in the shell.
Stable execution-idle means no active Attempt, no authoritative pending inbound,
and no acknowledged MessageId still awaiting projection reconciliation. Destructive
lineage switching additionally requires no unresolved `turn/start`/`turn/steer`
request in AppServerClient's current-generation pipeline (`lineageSwitchSafe`).
This guard covers historical Branch/Retry, Session-tree switches and open selectors.
The four stages are unresolved transport -> acknowledged MessageId evidence ->
authoritative pending/canonical/Attempt observation -> genuinely idle. The first
two stages are not a browser queue; the client atomically hands request ownership
to acknowledged evidence without publishing a transient safe state. Compact intentionally
uses a different native precondition: the coordinator can perform maintenance while
inbound awaits adoption; concurrent Attempt/maintenance/resource-reload conflicts
remain native refusals. Fork does not replace the source and is not idle-gated.

The flat editor fails closed on restored input outside `Upload* + nonempty Text?`;
it never combines/reorders arbitrary native `editor_content`. See CHAT.md for the
explicit editable limitation. Retry submits native blocks directly without it.

No provider, Workspace, resource or MCP editor is introduced. The Goal command does
not invent user authority to create or complete a Goal.

`CommandSession` binds typed operations to an exact AttachmentTarget and connection
generation. Navigation/dismissal epochs revoke UI continuations, not committed
mutations. After a side-effecting response loss, no step is replayed and no later
step continues. Reconnect lists Sessions, reacquires intended attachments, rereads
snapshots/settings; reopening a selector rereads model/current/catalog state.
Unknown outcomes remain diagnostics: inspect the Session list/tree rather than
guessing which operation succeeded. A failed mutation locks that selection until
the user closes it and rereads authority. The fixed Todo → Goal → Queue → Composer
stack and accepted-MessageId echo contract remain intact. Exact accepted queue edit/remove uses native CAS; no browser queue authority exists.

## Harness Agent controls (#346)

`AgentComposer` binds native draft/upload/command operations to the pinned Harness
editor, tools, modes and trailing seats. IME composition (including keyCode 229),
Enter/Shift+Enter, focus restoration, upload receipts and command discovery keep
one implementation. The active empty editor exposes Stop; a nonempty draft adds
Queue/Steer submission. Stop acknowledgement only records a cancellation request.
The native attempt phase alone supplies terminal presentation; disconnect is inert.

`AgentControls` reads exact `settings/models` and `settings/model` data. Both the
composer menu and `/model` popup advertise only returned model references and
reasoning profile IDs. The native default is represented by omitting the profile.
`settings/setModel` is the current target-bound live mutation. `settings/selectModel`
is the distinct durable Session revision-CAS authoring operation; it does not
replace the live Agent operation. An acknowledged or uncertain model change fences
dependent Send/lineage actions until a fresh authoritative snapshot is read.
Catalog caches and open menus are discarded on attachment/generation replacement.

The permission menu offers only `policy` and `full_access`. Native
`SourceSettings.prospective_approval_mode` resolves desired source policy; the
browser does not parse config or compute an overlay. Save uses the exact Workspace
source revision and Approval semantic unit. It never publishes a runtime generation.
The running label reads `attempt.execution_settings.approval_mode`; idle effective
policy reads `snapshot.effective_approval_mode`. Pending Reload is explicit. Apply
saved policy invokes existing `configuration/reload` only while idle; native busy
and publication checks remain final. This is an Agent control, not a CFG3 editor.

Native pending interactions take over the composer seat, preserving the hidden
local draft. Approval uses native Allow/Deny; Questionnaire supports schema-defined
choice/multichoice/boolean/text/numeric/custom values and page navigation. Only
`interaction/respond` or permitted `interaction/cancel` sends a response. Escape,
dismissal, navigation and unmount never settle anything. Successful acknowledgements
keep the pending surface until snapshot absence proves settlement. Lost replies
stay uncertain, cannot be resent and repair through native reconnect/reread.
