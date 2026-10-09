# Agent Composer and native controls

WEB-16 uses one resident ConversationComposer/AgentComposer for both the hero
and native Session. First submission changes an explicit binding without a key
or remount. Draft/attachment and input-trigger state remain in that composer;
only explicit navigation resets the draft. See
[the ownership contract](../docs/issue-406/conversation-surface.md).

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
owned by a Session-keyed ConversationDocks child, so no dock state crosses Session
views. The composer itself is not keyed by Session. Presentation is adapted
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
| Goal | `GoalDomain` durable phase | `snapshot.goal` (`GoalView`) | `goal/control` pause, resume, edit objective, edit budget |
| Queue | Conversation inbound mailbox | `snapshot.inbound.pending`, `snapshot.attempt` | exact pending edit/remove (WEB-06) |

No App Server protocol change was needed: all three facts were already typed in
the common snapshot.

### Todo

`snapshot.todos` is present exactly when the attached runtime composes the Todo
extension. The native facts stay distinct; two of them share one visual result:

- absent: no dock;
- composed with an empty current list: no dock. The list is a current fact, not an
  absence, but an empty card is not capability discovery — that belongs to the
  capability/settings surfaces. A deleted-only list is empty after the presentation
  filter and is therefore also no dock. There is no "No current tasks" strip, no
  Todo wrapper and no reserved composer-stack height (Issue #377, matching the
  pinned Harness `TodoPanel`, which renders nothing for an empty plan);
- composed with tasks: a collapsed per-status summary and a bounded list in native
  task order. Only native statuses render (`pending`, `in_progress`,
  `completed`); `deleted` tombstones are dependency targets, not current work.
  `blocked_by` is shown as the native relation (`after #1`), never as an invented
  status. An in-progress task shows its `active_form`. A non-empty all-completed
  list is not an empty list: it keeps its dock and a `2 completed` summary, because
  a just-finished plan stays useful while the answer is read.

The dock starts collapsed and owns only its disclosure state. When the visible
list disappears, disclosure resets; a later non-empty list is a new presentation
and starts collapsed. Ordinary non-empty updates — a task completing, a task
arriving — never close an open list.

Todo lifetime is native rustX authority. Harness's own plan is turn-scoped and
clears on the next `turn/start`; that rule belongs to its `todo/write` domain and
is deliberately **not** imported. Nothing in the browser clears or mutates Todo on
user input, turn start, Assistant completion, navigation, reconnect, timers or
disclosure changes. The dock disappears exactly when the authoritative current
visible list is empty.

Current Todo comes only from `snapshot.todos`. It is never reconstructed from a
historical `todo` Tool call/result, and never from a Todo section inside a
historical Agent Status composition — see
[CHAT.md](CHAT.md#agent-status-annotations).

The dock has no mutation. Todo tasks are never written to `rustx.toml`,
Agent TOML, `SessionPersistentState` or browser storage. Whether
Todo is composed is `agent.plugins.todo` configuration at User/Workspace or
named-Agent scope; there is no Session extension override and the dock offers no
configuration control.

### Goal

The dock renders the current `GoalSnapshot` — objective, durable phase, blocked
reason, consumed/budget rounds and durable revision. Absent extension, no Goal
and terminal `complete` occupy no composer space.

Durable `GoalPhase` is the one Goal lifecycle authority (Issue #351):

```text
Active   = rustX is authorized to continue pursuing the objective whenever
           the owning ConversationRuntime reaches an eligible safe idle
           admission boundary
Paused   = continuation is not authorized
Blocked  = continuation is not authorized
Complete = terminal
```

The status label is that phase and nothing else — `Active Goal`, `Paused Goal`,
`Blocked Goal`. There is no activation flag on the wire, so the dock can never
render an "Inactive Goal", and no `data-goal-armed` attribute exists. An
admitted autonomous round advances the revision and the consumed count without
disturbing an open draft.

Controls are exactly the ones GoalDomain assigns to users: one lifecycle control
chosen by the phase — Pause for `active`, Resume for `paused`/`blocked`, never
both — plus edit objective and edit budget. There is no separate Play/arm step
after Resume: a successful `Paused|Blocked -> Active` restores continuation
eligibility by itself. Create, block and complete remain native model
declarations; the Web `/goal` opens these existing controls, and there is no clear.

Native Goal calls render compact activity instead of generic Tool cards:
`Goal started`, `Goal completed`, `Goal blocked`, and `Goal checked`, only after
successful execution. Assembled/running/failure/cancellation/unknown states remain
explicit and claim no successful transition. Activity is historical evidence;
the dock still reads only the authoritative Goal projection. Collapsed Execution
details retain exact Tool ID, model name, call ID, arguments, result and lifecycle.
Trajectory continues to expose exact native Trace identity and payloads.

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
The pure `app/composer/submission-policy.ts` boundary derives the primary seat
from the authoritative running state, actionable draft, typed command/discovery
classification, upload readiness, acknowledgement state, message block and
cancellation availability. It returns an action kind, truthful label/tooltip,
disabled state and, for ordinary messages, the existing client delivery argument.

| Current facts | Primary seat | Operation |
| --- | --- | --- |
| Idle, actionable message | Send | `turn/start` (`send`) |
| Idle, empty | disabled Send | none |
| Running, empty or owner-blocked | Stop, disabled if cancellation unavailable/pending | existing `cancelTurn` |
| Running, ready message, Queue preference | Queue | `turn/start` (`send`) |
| Running, ready message, Steer preference | Steer | `turn/steer` (`steer`) |
| Slash/discovery draft | Run command or Review command | typed selection/adjudication; never message delivery |
| Upload unresolved or acknowledgement pending | disabled submit; acknowledgement status below card | none |

Plain Enter and clicking the message primary button use the same pure policy:

| Native state | Busy Enter preference | Enter / primary button | Ctrl/Cmd+Enter |
| --- | --- | --- | --- |
| Idle | either | Send (`turn/start`) | Send (`turn/start`) |
| Running | Queue | Queue (`turn/start`) | Steer (`turn/steer`) |
| Running | Steer | Steer (`turn/steer`) | Queue (`turn/start`) |

Shift+Enter remains a native textarea newline. IME composition/keyCode 229
bypasses submission. Commands stay typed commands; this preference cannot turn
slash input into a prompt. Empty Enter never invokes Stop. Both message transports
enter the existing native mailbox; Steer promises neither interruption nor priority.
Acknowledgment locks, disabled/submission gates, unresolved uploads (including
failed/uncertain), first-submission retention and exact draft consumption are unchanged.
There is no browser queue, replay or text-only fallback.

The **Send behavior while busy** row on the General Settings page edits
`app/composer/preferences.ts`, one shared origin/device browser presentation owner,
matching DeepSeek Harness's General `composer-enter` row; the Composer renders no
selector of its own and only reads the preference.
The closed union is `queue | steer`, default `queue`, stored as a raw value under
`rustx-composer-busy-enter-v1`. Valid values survive remount and browser reload;
missing, corrupt or unreadable storage defaults to Queue. Denied writes leave the
live page preference usable. No Session, Workspace or native configuration changes,
migrations or cross-device synchronization are involved. Labels and modifier help
are localized in English/Chinese and the primary action names its actual delivery.

### Scoped double Escape

Two independent, unmodified Escape keydowns in the focused **message textarea**,
within an inclusive **500 ms** window, request cancellation. This is the supported
Composer region; toolbar controls, other editors, Chat, dialogs and hidden Sessions
cannot participate. The first press arms only a short-lived exact scope. The
second resolves the same scope and clears the arm **before** invoking the operation.
A fresh press after expiry starts a new sequence; held-key repeat cannot complete it.

Identity includes the actual client object, client generation, native attachment
target (Session, Conversation, runtime incarnation and attachment ID), resident
composer binding and exact Attempt ID. The sequence resets on replacement of any
of these, cancellation becoming unavailable, Attempt settlement, editor blur,
window blur, composition beginning, command/preference menu opening or consumed
Escape, expiry and unmount. Modified, repeated, prevented and IME events reset
without arming. Command-menu Escape dismisses only the menu and preserves the draft.
Existing modal/menu focus owners arbitrate before the editor; there is no global
cancellation shortcut or DOM exception list.

The Stop button and double Escape converge on `ConversationSeat` →
`AppServerClient.cancelTurn(expected)` → native `turn/cancel`. The internal expected
target captures generation, native target and Attempt ID. The client refuses an
obsolete expected target and revalidates eligibility again beside actual socket
dispatch. `turn/cancel` is an exact lifecycle control operation in the existing
bounded two-request control lane. Ordinary RPC capacity cannot queue it; a full
control lane refuses it locally, definitely unsent. No protocol change is involved.
The exact local cancellation operation owns its in-flight/stopping marker. A
failure known to precede transmission removes only that operation's marker,
even if its transport/generation has been retired. An older continuation cannot
clear a successor marker. A transmitted request with a lost reply stays uncertain;
an acknowledged request stays acknowledged even if its subsequent refresh fails.
Only authoritative Attempt projection determines
settlement. A lost reply follows existing refresh/reconnect repair; it is never
replayed or retried, and there is no browser cancellation queue. Further gestures
while cancellation is pending create no duplicate request. Escape never consumes text, selection, receipts, queued input or attachments.

The browser recognizes a gesture and chooses an existing operation; it does not
prove, own, retry or settle execution. Native/App Server cancellation and native
inbound Queue/Steer admission remain authoritative.

The current Web binding addresses root Sessions, with no continuable-child input
scope; it therefore has no independent Stop + Send exception.

A Stop click only requests cancellation. The existing client fences repeated
requests through acknowledgement/uncertainty until an authoritative attempt
snapshot settles it. Message readiness and cancellation availability are separate:
a pending model change can block messages without disabling native cancellation.

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
queued work, pause or mutate Goal, modify Todo, settle a mutation or invent a
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

### Editor and toolbar (WEB-11)

The native textarea starts at the Harness **36px** one-line floor. `rows={1}`
only supplies the initial native fallback; `useTextareaAutosize` measures content
before paint on mount/draft changes, and ResizeObserver measures width changes
(including mobile/sidebar and hidden interaction takeover restoration). Deletion
shrinks it again. CSS owns the existing `--dsh-composer-text-max-height` cap
(336px in the Conversation seat); the textarea is the **only draft scrollport**.
The inset, card and attachment rail do not become nested vertical draft scrollers.
The existing theme typography is inherited; current Appearance exposes theme only,
not a composer font-size setting. Session/conversation keys and restored-input
validation are unchanged. Drafts are not persisted or replayed on remount.

The card retains its width axis, 22px corners, elevation, 12px gap and toolbar
rhythm. Left: compact `+` command launcher, quiet paperclip intake accessory and
permission control (effective policy is available on its tooltip; native application state
keeps the effective/desired distinction and action visible). Right: bounded model control and one primary action. The
paperclip remains keyboard/touch accessible because rustX has no typed file-intake
slash command; it uses an unfilled accessory treatment instead of a second `+`
launcher. The receipt rail appears inside the card only when populated. Controls
wrap when needed on narrow widths; desktop remains one toolbar row.

Permanent mailbox explanation and keyboard hint chrome are removed. Only an
in-flight acknowledgement occupies the low-priority status seat below the card.
Upload errors, ordered-input refusal, App-level uncertainty, durability failure
and reconnect diagnostics remain visible.

## Uploads

User uploads are native Session-owned files under App Server v35. The dedicated
intake owner normalizes picker/drop/paste, preserves duplicate selections through
local identities, and keeps per-file accepted/rejected outcomes. Raw over-count
selections are atomically rejected with one summary. Directory entries are detected
through DataTransfer items; paste reads file items once and inserts plain text at
the textarea selection. IME and existing command/Enter/Escape ownership remain.
The attachment button tooltip exposes server policy in English and Chinese.

Ready cards carry native receipts. All other outcomes gate submission except
accepted draft files owned by the retained first-submission lifecycle. Retry needs
authoritative no-commit evidence; Check status only reads the original operation.
A ready repair reuses its receipt without uploading again. The first-submission
owner retains operation identities and receipts outside React; recovery never
creates another Session or automatically sends a turn. Continue is a separate
explicit gesture after reconciliation. Removal never claims native rollback.

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
| `/model` | `session/model` + `session/models`, then `session/setModel` with an exact catalog model reference and native defaults |
| Approval mode control | Native Workspace Approval source-unit CAS; desired/published/running distinction and automatic native application |
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
one implementation. The active empty editor exposes Stop; a ready message replaces that seat with
Queue submission (Ctrl/Cmd+Enter selects Steer). Stop acknowledgement only records a cancellation request.
The native attempt phase alone supplies terminal presentation; disconnect is inert.

`AgentControls` reads exact `session/models` and `session/model` data. Both the
composer menu and `/model` popup advertise only returned model references and
Model Profile IDs. The native default is represented by omitting the profile.
A selection has two facts. The configured Profile (`SessionModelConfig.profile`)
is the user's intent: absent follows the Model's `default_profile`, present pins
that Profile even after the default moves. The effective Profile
(`ModelInvocationView.profile`) is what the invocation resolves to and is only
displayed. Each control offers three typed gestures (`ModelSelectionIntent`):
choose a Model (choosing the selected Model keeps its pinned Profile), pin a
named Profile, or follow the Model default ("Model default profile"). A gesture
changes the configured intent, never compared to the effective Profile, so
pinning the current default and returning to following it are real changes and
only repeating the configured choice is a no-op. A same-Model Profile gesture
replaces only `profile` in the whole-state `SessionModelConfig`, keeping request
overrides, output limit and explicit Summary for native validation; a different
Model starts from its native defaults. Draft Sessions and the new-Session
preference keep a pinned and an absent Profile distinct.
`session/setModel` is the dedicated operation for later intentional Session model changes.
Initial explicit intent belongs to `session/create.settings.model`; a projected
native default is omitted. See [startup ownership](../docs/issue-419/ownership.md).
The later model-change operation prepares a
complete binding and commits through the idle/admission gate and model baseline
fence. Acknowledged or uncertain model changes require authoritative reads before
dependent Send/lineage actions. Catalog caches and menus are discarded on attachment
replacement.

The permission menu offers `policy` and `full_access`. Native source projection
resolves desired policy; the browser does not overlay config. Save submits the
exact Workspace revision and typed approval unit. Native reconciliation applies
policy automatically to future independent Attempts. Running labels retain
`attempt.execution_settings.approval_mode`; current policy comes from the native
snapshot. Settings presents native context adoption, failures and restart state.

Native pending interactions take over the composer seat, preserving the hidden
local draft. Approval uses native Allow/Deny; Questionnaire supports schema-defined
choice/multichoice/boolean/text/numeric/custom values and page navigation in
Harness's question flow: a recommended first choice is the implicit draft, a
single choice advances to the next question, the custom answer row is always
visible, Skip omits a question from the submission and the card's dismiss action
sends the native decline. Only `interaction/respond` or permitted
`interaction/cancel` sends a response. Escape, collapsing the card, navigation and
unmount never settle anything. Successful acknowledgements
keep the pending surface until snapshot absence proves settlement. Lost replies
stay uncertain, cannot be resent and repair through native reconnect/reread.

### Direct compaction and context seat (#435)

`/compact` consumes its command draft at dispatch, independently of the result.
The client claims duplicate ownership before any asynchronous boundary. Unlike
selector commands, it opens no CommandPanel and needs no second confirmation.
Text entered afterward remains editable and is never cleared by settlement.
Session-scoped request evidence survives component remount; native context events
and snapshot repair supply actual progress, release, diagnostics, and occupancy.
Uncertain responses require authoritative read repair and are never auto-retried.
See `docs/context-engine.md` for the request/start/commit/release contract.

PR #409 (`f268175bb8d31010706e7070aae80d2b46b7aced`) removed the earlier
ConversationStats occupancy widget and changed its test to assert absence while
introducing resident Turn presentation. The inspected diff establishes that change,
but not a separate meter-specific rationale. The restored capability lives in the
current Composer context seat; cumulative ConversationStats and Turn tails retain
their present roles.

The selected Composer has one retained intake owner. Only committed semantic
Composer activation retires an incompatible Session, Conversation or draft binding.
Speculative/abandoned render cannot clear, rebind or retire the committed File owner.
Native authority replacement explicitly retires obsolete owners;
same-binding remount/reconnect retains recovery state. Client disposal clears all
intakes. First-submit sealing transfers ownership to FirstSubmissions before create;
admission/discard releases those files. A known rejected create returns files to the original live intake for editing.
User-input count/byte limits are distinct from transfer limits and are independently
validated by the native receipt collection owner before turn admission.

Upload recovery preserves typed evidence in both immediate and first-submission
owners: native Absent/Failed permits an explicit fresh-operation Retry; Unresolved
or unrepairable response loss permits only exact-operation Check status. A queued
file stopped before dispatch is known retryable, not uncertain. Rebinding never
restarts it automatically. Failed browser file extraction displays a removable
rejection without creating an empty File.
