# WEB-12 product / diagnostic boundary

WEB-13 follow-up: the default Connection Sidebar/modal and manual transport form
are removed. Local launch authenticates and connects automatically. Recovery shows
Reconnect / Show details. Explicit Remote App Server configuration and mode choice
live in Settings → Connection; transport details remain advanced. See [CONNECTION.md](CONNECTION.md).

Base: `66475c83ed7ff1cbb2be880b040913023a463f55`.
This audit precedes implementation. Native authority and WEB-11 composer behavior
remain inputs, not redesign targets.

Display metadata is native-owned: `session/list` is bounded fuzzy catalog browsing;
`session/summary` is an exact SessionId read through the durable controller without
runtime composition. Web `view.summary` is only a replaceable off-page observation,
not catalog authority. A first canonical user message invalidates an unnamed label;
the check completes only after a successful exact read (including no text preview).
Failure remains retryable, without polling, mutation replay or LLM naming.

| Current item | Classification | Final treatment | Reason |
| --- | --- | --- | --- |
| Session selection | Product | Sidebar Session browser only; top Session strip deleted | One selector, no duplicate tabs/dropdown |
| Current Session header | Product | Identity/status/actions and Chat/Trajectory, not a Session picker | Identify current work |
| Session display title | Product | Explicit name > native first-message preview > New session | Reuse native catalog, no browser summarization |
| Session ID | Developer Inspector | Identity / scoped runtime facts | Never a product fallback or accessible label |
| Close browser view | Product | Corresponding Sidebar row menu; View options → Close all views for unlisted/restored views | Explicitly release controllers, free the 32-view capacity, retain server work; no eviction |
| Automatic LLM title | Remove | Not implemented | Deterministic offline naming only; manual rename retained |
| Workspace identity/location, cwd/path | Product | Quiet location under title | Identify execution location without lifecycle suffix |
| Chat / Trajectory | Product | Header view tabs | Conversation and execution history navigation |
| Session tree | Product | Session menu and existing command | Native branch/history navigation |
| Model controls | Product | Existing composer seat | Choose the model |
| Permission controls | Product | Existing composer seat | Understand and configure permission |
| Todo | Product | Existing dock | Work progress |
| Goal | Product | Existing dock; revision in Inspector | Objective and budget controls |
| Queue | Product | Existing dock; concise status | Review/edit pending input |
| Composer | Product | Preserve WEB-11 | Send/Queue/Steer/Stop and attachments |
| Runtime activity | Product | Concise progress | Understand ongoing work |
| Subagent activity | Product | Named Agent and progress, no execution IDs | Delegated work visibility |
| Workflow activity | Product | Named Workflow and steps, no attempt IDs | Workflow progress |
| Background Tool activity | Product | Tool name and result/progress | Background work visibility |
| Raw Agent status | Developer Inspector | Execution section | Unstructured native diagnostics |
| Attachment state / intent / ID | Developer Inspector | Attachment section | Internal controller lifecycle |
| Runtime incarnation / residency | Developer Inspector | Attachment section | Exact native lifetime evidence |
| Attempt ID / exact phase / outcome | Developer Inspector | Execution section | Product has deterministic status projection |
| Generation / revision / CAS | Developer Inspector | Configuration and domain evidence | Exact fencing evidence; Settings keeps authoring conflicts |
| Connection generation | Developer Inspector | Protocol section | Socket fencing detail |
| Resync | Recovery | Conditional Retry connection | Existing refresh operation when observation is unavailable |
| Open Session | Navigation / recovery | Opens the durable Session and acquires its client relationship | Residency is implicit in native admission |
| Detach | Remove | Remove standalone button; closing a view retains client release | Redundant lifecycle chrome |
| Session deletion / branch switching | Product action | Confirm delete or switch branch | Runtime retirement is internal to the manager |
| Cancellation state | Recovery | Stopping; uncertainty remains Needs verification | Request is not settlement |
| Uncertain-operation evidence | Developer Inspector | Exact request and reconciliation evidence | Ordinary notice remains actionable without raw IDs |
| Durability failure | Recovery | Visible storage failure; exact evidence in Inspector | Never imply durable success |
| Transport / protocol details | Developer Inspector | Existing bounded wire log | No new diagnostics system |
| Historical interaction / assistant recovery evidence | Settings / advanced | Explicit per-entry disclosure; no ID in its summary | Durable history and recovery need inspectable evidence, not permanent raw text |
| Message IDs in accessible names | Remove | Role-based message names; existing canonical anchors retained | Screen readers need content and role, not internal identity |
| Scope / origin / effective / conflict / apply lifetime | Settings / advanced | Preserve existing CFG3 controls | Safe authoring requires provenance |

The pure product projection reads client/native observations. It owns no timer,
transition, retry, persistence, settlement or queue. Inspector receives only
observations and the browser-local protocol log, never the runtime client.

Sidebar status includes only its Session's uncertainty. Selected Inspector sections
are Session-scoped; Global / other Session diagnostics explicitly separates other
and unscoped evidence. Connection's advanced disclosure permits explicit browser-local
acknowledgement of reviewed non-interaction notices (no RPC or inferred outcome).
Interaction uncertainty still requires native reconciliation. Queue conflicts say
"Use latest version" without changing CAS. Deletion shows the display title and
ownership counts; its exact target revision remains an internal operation argument.

Harness inspection: Epic baseline `c291e7961a515f6d7af9304e7fd1d257929aef26`
and re-fetched current `ddefc45fbc7f8e46dd73185e68295696d1297887`.
Header title/actions/utilities/corner and view-tab hierarchy inform the composition;
existing imported shell/primitives remain the implementation baseline. No Harness
runtime, slot system, docking framework or persistence is adopted.
