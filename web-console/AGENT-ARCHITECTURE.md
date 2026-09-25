# WEB-RESET-02 Agent ownership

Base: `204f7ccc8fbaf4bc1b6842e02e8d0d68f19d5837`.
Harness: `ddefc45fbc7f8e46dd73185e68295696d1297887`.

Pre-implementation classification:

| Surface | Class | Boundary |
| --- | --- | --- |
| Conversation header, Chat column, user bubble, reasoning | B | Native transcript order and exact live message identity; no event assembler |
| Composer card, footer seats, keyboard and focus | B | Native textarea draft and typed commands; native Send/Queue/Steer/Stop |
| Tool row/tree and specialized bodies | B | Native canonical Tool projection; no browser call/result pairing |
| Approval / Questionnaire takeover | B | Native pending interaction and exact response schema; drafts only are local |
| Model / reasoning menu and popup | B | Native catalog, Session selection and revision; no provider inference |
| Permission selector | B | CFG3 source CAS and automatic native application; attempt policy stays frozen |
| Agent CSS, disclosure and shared primitives | A | Pinned Harness source with bounded environment adaptations |
| Todo / Goal / Queue, lineage, activity, transport notices | C | Existing native owners; Harness seats/primitives |

`presentation/agent` imports only presentation and React. `app/agent` and
`bindings` translate native DTOs into finite render props. AppServerClient owns
transport generations, attachment/incarnation fences, read caches and uncertainty.
No presentation surface owns execution, canonical messages, Tool settlement,
interaction lifetime or durable configuration.

WEB-12 replaces the exact Attempt line with a deterministic product projection:
Working, Queued, Stopping, connection recovery, Needs verification and actionable
failure. Idle has no status chrome. A request to stop is never a successful stop;
unresolved request evidence survives even a suggestive settled snapshot.
Tool/Job/Agent/Workflow activity remains product UI. Child Agent cards expose
their durable identity and current activation for continuation; raw Agent status,
native mailbox data and revision evidence live in the existing Inspector. Interaction cards retain their questions, Tool arguments and review
subjects, while routing IDs and review instance metadata move to diagnostics.
Goal/Queue changes are limited to diagnostic text removal; their layout, native
references, CAS, controls and the WEB-11 composer contract are unchanged.

CFG3 Save transfers desired source configuration to native reconciliation.
The current Attempt retains captured policy; later independent Attempts can use
new policy. Native context candidates require explicit Session adoption. The
permission menu renders native desired/effective/captured facts without classifying
cache impact or publication behavior.

Canonical Tool results carry `ToolCallOccurrenceRef` (Assistant MessageId plus
block index). SQLite schema 40 validates this owner and atomically maintains the
derived `canonical_tool_calls` index for bounded cross-page reads. Canonical
history carries the relationship through clone, fork and tree copies, which remap
Assistant MessageIds and preserve provider correlation IDs. The foreground
projection carries the same occurrence; only that occurrence can receive its live
state. Provider call/Tool IDs alone are never historical identity. No unproven
turn folding or inferred subcall nesting is supported.

## Issue #402: draft and completed-process presentation

`App` owns an explicit New Conversation / Session center route. The shared
Harness-derived composer accepts browser File drafts before Session creation;
`firstSubmitMachine` owns the one creation/application/upload/send sequence.
`firstSubmitPort` fences endpoint, connection generation, authority revision,
navigation and the exact native attachment. Settings target actors are shared
with the permission seat; no configuration coordinator lives in the composer.

App Server v22 / Runtime Client v48 publishes `completed_process` on canonical
Assistant and Tool entries using native Attempt event evidence. Its origin and
local final-message identity survive pagination and lineage remapping. Only
those facts create process disclosure membership; final answers and actions
stay outside. Agent Status retains its native anchor and shares disclosure only
when its exact Conversation/Attempt matches a loaded completed process. Live or
unclassified annotations remain visible. No canonical cache is modified.

## Jobs and continuable child Agents (#411)

`RuntimeFacts` renders two native snapshot domains. `jobs` contains finite detached
Tool invocations keyed by `job_id`; a terminal Job never resumes. Status is an
immediate read, Wait targets that exact Job's physical settlement, and Cancel
uses the native settlement contract. Proactive runtime events trigger the normal
snapshot refresh, so terminal output appears without status polling.

`agents` contains durable child conversations keyed by `agent_id`. A card remains
mounted and identifiable as its native state changes Active → Stopping → Inactive
→ Active. `activation_id` identifies the latest finite activation and
`current_activation` identifies the current one. An activation ID is never used
as the Agent row key. Transcript reads address the durable Agent; committed child
messages and final reports use the canonical transcript renderer. Older pages
come from the native transcript endpoint, not a browser result cache. An open
transcript refreshes when native activation facts change, including settlement
and resume. Child artifact names render without borrowing the parent artifact
reader: the native API currently exposes only parent-scoped artifact reads.

Send message always sends the same `agent/sendMessage` operation. React never
chooses between steering and resuming. The registry atomically admits Active
input, creates an Inactive activation, or rejects Stopping input. Interrupt ends
only the current activation. Wait captures its target in the runtime and cannot
be retargeted by a subsequent resume. A pending wait does not disable interruption
or messaging. Requests are never retried after a lost response; reconnect replaces
the complete projection from native authority. Jobs and Agents remain scoped to
the exact current attachment.

Historical native Tool cards distinguish `job_*` from `subagent`, `list_agents`,
`send_message`, `wait_agent`, and `interrupt_agent`. Historical results never
replace the live roster. Trace records retain individual activation evidence and
carry native Agent correlation rather than deriving child identity from an
activation string.

Reference source inspected: DeepSeek Harness
`packages/client/ui-subagent/src/client/sidebar-chat/index.tsx` and
`packages/client/ui-jobs/src/client/JobListAction.tsx` at
`477b4f420553e8a52c2fbccc464d7561b239c443`.
Adopted stable child-conversation detail, separate finite Job rows, retained output,
and snapshot-owned controls. Rejected the historical one-shot/continuable mode
branch and client lifecycle bookkeeping: rustX has one continuable Agent model.
No new upstream presentation source was imported or baseline repinned.
