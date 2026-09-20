# Agent Conversation ownership and resource bounds

rustX owns canonical messages and history. The browser renders two authoritative
read products and retains only replaceable read caches:

- `session/attach`, `session/snapshot`, `session/subscribe`: current/live projection.
  Notifications coalesce into a dirty bit and trigger authoritative replacement.
  No client event log, event fold or locally assembled Assistant survives repair.
- `session/transcript { before, limit }`: older durable transcript pages. The wire
  field is `before`; it is the exclusive `RuntimeClientTranscriptCursor` returned
  as `next_cursor`. This never updates `RuntimeClientCursor`, used for live reads
  and subscription/resync only.

## Composition

`app/agent/AgentTranscript` composes the pinned Harness Chat column and Message,
Reasoning and Tool presentation. `app/agent/Message` binds canonical blocks to
those pure components and the audited Markdown/code renderer. Durable entries
remain in native cursor order. Streaming uses the server's message ID; a canonical message
of that identity wins immediately, and settled/replaced attempts lose stale
partials. Surface messages outside the loaded page are not appended as history.
Typed current context is disclosed separately. Subagent, Workflow and background
activity are current adjuncts, not fabricated historical placements. Foreground
Tools occupy their canonical Assistant block position.
Historical interaction/publication audits are read-only; live Approval,
Questionnaire and Review retain existing typed settlement and uncertain-outcome
controls. Todo/Goal/Queue projection remains in the composer docks. WEB-05 adds
native historical actions as described below; it never derives canonical history.

## Native Tool projection

`RuntimeClientTranscriptEntry.tool_calls` contains native `ForegroundToolExecution`
records in the Assistant's block order. The durable owner resolves each result
through the persisted canonical occurrence/result-message index, even when the
result lies beyond the requested page. Provider call IDs may repeat across Attempts.
The runtime projection supplies live state only for the exact native
`(message_id, block_index)` occurrence (also checking call/Tool identity);
a settled durable record wins. React never joins call and result messages.

`bindings/tools.ts` selects Bash, Read, Write/Edit and Glob/Grep views by native
ToolId. Unknown identities use the same generic Harness card. Inputs, text/JSON
output, native status and managed image/file attachments remain truthful subsets.
Edit/Write diffs show **requested changes**, not an inferred filesystem diff.
Background execution uses its native ExecutionId, not a fabricated call identity.
No process folding or subcall nesting is inferred from adjacency or Tool names.

## Agent Status annotations

One composed Agent Status is one historical, request-scoped fact of the
conversation. It is **not** current Todo, Goal, Queue or live execution state:
those are `snapshot.todos`, `snapshot.goal`, `snapshot.inbound` and the native live
projections. `snapshot.statuses` is the runtime's bounded window of past
compositions in composition order, oldest first — a list, not a latest value.

The Runtime Client already publishes where each composition belongs, so the browser
never infers a position (Issue #194):

```text
opportunities.fresh_inbound        -> the exact inbound message identity
opportunities.post_tool_batch      -> the durable transcript cursor of the
                                      settled ToolResult batch
```

`bindings/agent-status.ts` selects exactly one anchor per composition:

```text
fresh_inbound present              -> anchor = its target_message_id
else post_tool_batch.transcript_anchor present
                                   -> anchor = that transcript cursor
else                               -> unplaced; nothing is drawn
```

`FreshInbound` wins unconditionally when both exist: an exact message identity is a
stronger fact than a position, and choosing it without consulting the loaded page is
what keeps a doubly-eligible composition from being drawn twice. Anchor selection
never depends on what is on screen. If the selected `FreshInbound` target is off
page, the composition stays undrawn — it never falls back to a visible
`PostToolBatch` anchor, and it never relocates to a neighbouring message, the latest
Assistant response, the streaming response or a global panel. Paging its anchor in
later reveals it at that position.

`agentStatusPlacement` builds finite `byMessageId` / `byCursor` indexes from the
authoritative window, collapsing repeated observations of one `status_message_id` to
one composition before any rendering happens — a React key is not a deduplication
mechanism. `statusesAt` merges both indexes for one transcript entry and orders them
by the runtime's composition ordinal, so a message-anchored and a position-anchored
composition that resolve to the same row still interleave correctly. Nothing reads a
timestamp, a label, status text, or an array index as an identity.

The transcript traverses the complete ordered page. A standalone `ToolResult` body is
suppressed — the native call projection already renders that content beside its call —
but suppressing a body does not erase the entry's authoritative transcript position:
`AgentTranscript` separates *content visibility* from *annotation-anchor existence*, so
a `PostToolBatch` anchor on a suppressed entry renders an annotation-only slot in the
exact right place. An entry with neither a body nor an annotation contributes no row
and no scroll anchor.

`app/agent/AgentStatus` renders the typed `sections` vocabulary (`temporal`,
`background_executions`, `todo`) as a subordinate `note`, collapsed to a one-line
summary with the shared `DisclosureRow` affordance. It is not a conversation speaker,
a user bubble or a current-state panel, and it carries no response actions.
`AgentStatusView.rendered` is the exact text the model saw: it stays diagnostics and is
never parsed for semantics, identity, placement or current tasks.

The canonical Agent Status Context message is request-scoped model history that never
becomes a transcript item. It is excluded from the generic Context / "Current context"
disclosure by typed context kind — never by text matching — so one composition has
exactly one representation and no stale Todo section is presented as live state. Other
context kinds are unaffected, and the Inspector keeps the raw window, including
`rendered`, under **Agent Status history**.

The browser owns no status retention. The window, its bound and its eviction are the
runtime's; the client has no event fold (a `session/event` invalidates, a
`session/snapshot` replaces), so cold attach, live observation and resync converge on
the same identity, placement and order by construction.

## Paging and reconnect

The cache holds at most 512 entries and an 8 MiB conservative UTF-16 serialization
budget; each older request asks for at most 64 entries. It is a contiguous durable
window, never canonical persistence. Current refresh preserves it only with a
matching durable cursor and fact identity. Current entries win overlaps. Missing
continuity or a capacity overflow replaces the window with the current page;
capacity replacement has a visible diagnostic. Unsettled native Tool projections
outside a fresh page also force a visible window rebase; historical assembled state
is never retained indefinitely as a substitute for rereading terminal authority. At the entry bound, Return to
latest explicitly replaces the window before more paging.

Older responses require the same connection generation, complete attachment
target and window epoch. Ordinary overlapping live refresh does not advance that
epoch, allowing live append while history is pending. Reconnect, reattach and
resync discard history and invalidate in-flight reads. There is no event replay
repair and no timestamp or lexical-ID ordering.

`ChatViewport` measures stable row keys before React mutates the DOM. Prepending
keeps that row's viewport offset and enters history reading. ResizeObserver
restores the same anchor after image/Markdown/layout growth. Only a reader at the
bottom follows new output; programmatic scroll delivery and shrink clamps do not
reassign that ownership. No timeout or sleep determines layout correctness.

## Completed-response tails and historical lineage (WEB-14)

Ordinary User rows expose Copy and their persisted timestamp when present. They
have no primary Fork/Branch/Retry toolbar. Final Assistant content has at most one
completed-response tail, tied to the native destination `closing_message_id` and explicit original execution `origin`.
For local responses, the Runtime Client joins canonical acceptance with a successful Attempt terminal;
provider completion and intermediate tool/model requests do not create tails.

Branch/Fork/Clone preserve finalized historical responses through immutable native
bootstrap provenance. `CompletedResponseProvenance` carries the destination
closing/Retry addresses, original execution owner, completion time, and exact
optional usage. The same identity map remaps messages, Surface operations, and
these addresses; a missing retained Retry input removes Retry. The origin remains
unchanged across deeper copies and is never a destination execution identity.
No source events, requests, recovery pointers, or live execution state are copied.
SQLite schema 42 stores this provenance atomically in the existing bootstrap row,
checks it on repeated initialization, and refuses obsolete stores without migration.
The native `After` validator accepts local evidence and inherited provenance through
one shared projection, while still requiring the exact destination append revision.

The tail's lineage menu exposes Branch in this Session and Fork to new Session.
Both use `side: after` and the immutable Surface revision that first appended the
closing response. The resulting prefix includes that Assistant response and the
composer is empty. The native owner validates the exact response/revision pair
and durable completion. Compaction and later appends do not change that historical
cut. Unknown revisions and mismatched boundaries fail visibly, without refreshing
or replaying the mutation. `session/tree` resolves the attached Conversation's
node, never the Session's mutable default. Independent Fork still copies native
uploads in the inherited prefix before publication; in-Session Branch shares the
Session's upload ownership.

Command discovery retains explicit pre-input boundary selection for native draft
restoration. Retry uses `side: before`, the input in the final request's frozen
Surface, and native returned editor content. These are distinct from the tail's
post-response continuation cut.

Retry is `session/branch` → authoritative destination identity → unload the idle
source runtime → attach the exact new node → `turn/start` with the returned
`editor_content` **once**. The User message is not duplicated: it was excluded from
the copied prefix. Original Assistant responses remain canonical in the original
node; no response text is replaced and no browser alternate-response store exists.
The Session tree button reads native nodes and can reopen the original lineage.
Branch, Retry and tree switching require `lineageSwitchSafe(view)`: the existing
`executionIdle(view)` observation plus no unresolved inbound transport request on
the current attached view. `executionIdle` remains unchanged: an observed
snapshot with no active Attempt, no authoritative `inbound.pending`, and no
acknowledged-but-not-yet-projected `view.submissions`. The native manager allows
one resident Conversation per Session. An accepted MessageId is evidence of native
ownership even before an Attempt appears; it is not browser queue authority.
Existing exact-MessageId reconciliation removes that evidence when native pending
or canonical history names it. `AppServerClient` separately publishes a per-Session
count of actual pending `turn/start`/`turn/steer` requests, including requests waiting
for a socket slot. Admission may commit before the acknowledgement reaches the
browser. Receiving success atomically hands that count to exact MessageId evidence;
a known rejection clears it without inventing a submission. Connection loss marks
sent mutations uncertain and invalidates the generation; reconnect rereads authority
and does not retain old transport counts or replay requests. No timer or React
send-button flag declares idle.
The guard is checked again after branch publication, before unload: if work arrived,
the committed node remains discoverable in Session tree, but no switch/retry occurs.
Fork does not unload the independent source and remains available during execution.
Native admission and shutdown remain the final authority; this product guard does
not turn an observation into a server-side idle reservation.

Editable Fork/Branch restore accepts only `Upload*` followed by at most one
**nonempty** Text block. Other ordered native shapes (interleaving, multiple Text
blocks, or an empty Text block that the flat sender would drop) produce a visible
refusal and disabled composer before decomposition. No blocks are reordered or
sent; the committed lineage is unaffected. Use an ordered-block client for those
inputs. Retry is not restricted by this editor: it sends returned blocks directly.
Restored upload rows use `(batch_id, token)`, not batch alone, as React identity.

Explicit model choice belongs to Session intent. Approval policy belongs to the
published configuration generation. Cold composition rereads current source bytes
and revalidates the Session's optional explicit model.
The browser never copies its cached model/policy values into a new runtime.

Navigation, dismissal or reconnect can obsolete a continuation after the native
mutation committed. It then neither redirects the user nor starts another step.
Response loss at branch, unload, attach or turn admission stops the flow without
replay. Reconnect and Session list/tree inspection repair observable state; when
the exact outcome is unknown, retain uncertainty. Original node navigation intent
is retained after reading its authoritative tree identity, so reconnect does not
silently substitute a different default node.

## Attachments

Paperclip, drop and paste transfer bytes through native `session/upload`. Draft
cards distinguish uploading, complete, failed and uncertain states. Send includes
only completed server receipts, in draft order; no model-modality preflight is
needed. Uploaded images are workspace files under this contract.

The current carrier limits are eight files, 256 KiB per file and 512 KiB per batch.
The 1 MiB request bound also includes base64/JSON overhead. Browser encoding is
transport-only; the Session owner receives ordinary decoded bytes. Removing a
draft card revokes its preview URL but does not delete a committed workspace file.

Canonical transcripts render `uploaded_file { batch_id, name }` attachment cards
without XML parsing, host filesystem browsing, or artifact readback. The native
model projection resolves absolute paths and adds `<user_uploaded_files>` while
preserving the user body. Reload/reconnect restores authoritative transcript facts.
Lost mutation responses remain uncertain; no upload or turn is automatically retried.

Tool artifact galleries are a separate domain: ArtifactResources retains at most
16 URLs / 4 MiB and two outstanding reads. Those cards release URLs on unmount,
retry and decode failure. Draft object URLs are preview-only (at most eight files),
never durable identity. Upload bytes are redacted in the protocol log.

See [the Session upload contract](../docs/session-uploads.md) for lifetime,
no-overwrite rules, mutable file semantics, fork copies and deletion recovery.

## WEB-02 review corrections

The mandatory App Server vocabulary is v15 (`rustx.app-server.v15` and generated
`protocol/app-server/v15.ts` / `v15.schema.json`). v12 and earlier initialization and
WebSocket offers are rejected; there is no compatibility mode. Runtime Client
retains its independently versioned contract.

Foreground, background and canonical Tool results all expose their typed
artifact galleries. Subagents and Workflows remain current/live adjuncts, with
native identity, lifecycle, wait, bounded diagnostic and run-budget facts in
product cards. No raw protocol dump serves as their primary Chat presentation.

Artifact Blob construction accepts safe authoritative MIME values from typed
Tool/file metadata. Semantic image references without MIME use an empty Blob
MIME, allowing the browser image decoder to inspect bounded image bytes. No
filename inference or durable browser metadata store is introduced. Real
Chromium acceptance uses a stdio MCP Tool that returns a PNG through the native
Tool result/artifact pipeline; it proves actual decode, original-image dialog
and a fresh load after reconnect. Current providers remain text-only, so the
subsequent model continuation is refused natively rather than translating image
input into an unsupported provider request.

## Trajectory view boundary

Chat and Trajectory share one App Server attachment and subscription. Their view
selector is presentation state. Trace has its own bounded cache/cursor/epoch;
Chat's transcript cursor is never used for Trace. Switching views leaves native
execution untouched. The raw developer inspector remains a separate tool.

Trajectory keeps one contiguous loaded history interval. A newest tail without
shared records replaces that interval and its paging epoch; one selected record
can remain separately inspector-visible and receive native lifecycle repairs.

See [Native Trace](../docs/trace.md) for server projection ownership, request/retry
grouping, current snapshot repair, redaction and deliberate inspector omissions.

Upload uncertainty includes typed `committed_durability_uncertain` RPC failures,
as well as response loss. Such drafts remain uncertain and require reconciliation
through authoritative state/reconnect; no mutation is automatically replayed.

Canonical Tool results require an exact `occurrence` reference (Assistant
MessageId and block index). The native derived index supplies cross-page results;
Chat never correlates historical results by provider `ToolCallId`. Lineage copies
remap the occurrence owner and retain the provider correlation ID. See the
[Agent protocol contract](../docs/app-server-protocol.md#agent-read-projections-346).


## Response usage and conversation statistics

`RuntimeClientTranscriptEntry.completed_response` is a derived, client-neutral
read model. The journal read is bounded to the native published snapshot frontier.
Inherited bootstrap provenance joins the same projection without execution events.
Only requested page identities retain summaries; canonical content remains in the
Ledger. Request usage is summed across the exact Attempt only when all actual
requests reported usage. Optional cache/reasoning buckets survive only when every
included report supplies them. Missing facts remain absent, including timing.

`RuntimeClientTranscriptPage.statistics` reports whole-Conversation execution totals
from the durable journal, with explicit request/report coverage. The composer reads
these native totals from the newest snapshot. Older page responses never replace
newer totals. Compaction does not erase journal usage. Fork/Branch create fresh
Conversation execution epochs, as established by native lineage. Inherited tails
retain their own historical usage while destination cumulative execution totals
start from zero; these two quantities are deliberately separate.

The Context owner exposes the last provider-measured request occupancy paired with
that exact request snapshot's model capacity. It is labeled **Last request context**,
excludes unsent input, and disappears after compaction or a newer unmeasured request.
No Web tokenization or browser-clock timing is used.

#364 merged as `3063ebd6`. Its native `GenerationEvidence` is the single request
clock contract consumed independently by Trace and completed-response projections.
Chat never reads Trace. `Ran for` is successful Attempt completion minus Attempt
start, including tools and retries. Details distinguish first-request TTFT from
dispatch (never Attempt-start latency), summed first-output-to-terminal model
work, and output speed over fully covered positive generation spans. Unknown
endpoints/usage remain absent; zero measured generation is zero with no rate.
Known no-output requests add no decode span; missing request evidence invalidates
exact aggregate generation. Failed requests with evidence remain included.

Immutable bootstrap provenance preserves response timing and usage through
Branch/Fork/reopen/deeper lineage without copying source execution records.
Destination execution totals remain destination-local. Mandatory versions are
App Server v15, Runtime Client v44, SQLite v42, and Session catalog v12, with no
old protocol artifacts or compatibility readers.

Projection cost is currently O(J + R): indexed 128-event batches over the captured
Journal prefix plus R inherited response summaries from bootstrap. The finite
read cut is a correctness boundary, not an O(1) performance claim. Lineage validation
and copying share one fold rather than scanning the Journal twice. #364 uses
bounded indexed Trace reads and supplies no shared incremental response accumulator.
A future native checkpoint/index can remove repeated folds; React must not cache
execution authority to solve this cost.

Presentation follows Harness `ddefc45f`: compact 28px icon actions, 8px gaps,
hover/focus reveal for older rows, always visible actions on no-hover devices,
36px narrow-layout action targets, existing accessible Tooltip/Modal primitives,
and separate code-block copying. No additional icon library is introduced.
