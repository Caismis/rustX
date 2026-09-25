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
Tool/Subagent/Workflow activity remains product UI; exact execution identifiers,
raw Agent status, native mailbox data and revision evidence live in the existing
Inspector. Interaction cards retain their questions, Tool arguments and review
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

App Server v21 / Runtime Client v47 publishes `completed_process` on canonical
Assistant and Tool entries using native Attempt event evidence. Its origin and
local final-message identity survive pagination and lineage remapping. Only
those facts create process disclosure membership; final answers and actions
stay outside. Agent Status retains its native anchor and shares disclosure only
when its exact Conversation/Attempt matches a loaded completed process. Live or
unclassified annotations remain visible. No canonical cache is modified.
