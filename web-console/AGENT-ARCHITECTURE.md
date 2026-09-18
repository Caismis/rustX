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
| Permission selector | B | CFG3 source CAS and explicit Reload; attempt policy stays frozen |
| Agent CSS, disclosure and shared primitives | A | Pinned Harness source with bounded environment adaptations |
| Todo / Goal / Queue, lineage, activity, transport notices | C | Existing native owners; Harness seats/primitives |

`presentation/agent` imports only presentation and React. `app/agent` and
`bindings` translate native DTOs into finite render props. AppServerClient owns
transport generations, attachment/incarnation fences, read caches and uncertainty.
No presentation surface owns execution, canonical messages, Tool settlement,
interaction lifetime or durable configuration.

CFG3 Save changes desired source configuration. Reload publishes a generation;
an active attempt keeps its admitted policy. The Agent control must display this
boundary explicitly, including pending Reload, rather than imply a Save changed
execution. No Session approval override or compatibility settings API is added.

Canonical Tool results carry `ToolCallOccurrenceRef` (Assistant MessageId plus
block index). SQLite schema 40 validates this owner and atomically maintains the
derived `canonical_tool_calls` index for bounded cross-page reads. Canonical
history carries the relationship through clone, fork and tree copies, which remap
Assistant MessageIds and preserve provider correlation IDs. The foreground
projection carries the same occurrence; only that occurrence can receive its live
state. Provider call/Tool IDs alone are never historical identity. No unproven
turn folding or inferred subcall nesting is supported.
