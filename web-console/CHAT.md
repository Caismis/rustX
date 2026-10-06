# Incremental Session projection and Chat ownership (#420)

`protocol/app-server/projection.ts` folds native events below React. The connection
owner validates the complete Session/Conversation/incarnation/attachment target and
connection generation, compares exact decimal cursors, ignores consumed cursors and
requires the next cursor to be exactly one greater. No provider-specific state or
mutation replay exists in this fold.

Native `read_domains_updated` publishes the bounded decorated transcript, statistics,
context occupancy and Todo view when durable observations change these domains.
Publication suffixes do not dirty these domains. Native projection/cursor publication
remains under the host lock, but blocking Store reads run outside it and install
only after their revision/frontier fence validates. Attempt settlement ends execution
independently of downstream durable enrichment. Trace remains a separate read capability.
Only `trace_changed` triggers a Trace repair; `read_domains_updated` does not. `session/trace.records` requests bounded
lifecycle repairs returned as `page.updates`, so older loaded records do not require
a Session snapshot. `tool_call_assembled.arguments_json` preserves native JSON number
spelling exactly instead of asking JavaScript to reserialize provider arguments.

Initial attachment carries a snapshot plus cursor. Snapshot recovery discards unsafe
incremental continuation; the native bounded replay owns events arriving during the
read. After installing C, subscribe after C and consume C+1 onward. Events at/below C
are no-ops. Acquisition-time resync retains the required subscribe and marks the old
continuation untrusted; attachment returns to `attached` after that subscribe succeeds.
Repeated resync coalesces; read failures leave controls
stale. Replacement clears acquisition work and existing exact target/generation fences
reject late replies. Explicit readback operations never replay a write.

Canonical commits replace the message's provisional content, using the same
`MessageSeat` and `Message` component in one keyed sibling list. Suffixes cannot
recreate a retired in-flight message. Multiple messages in one Attempt retain their
own native identities.

ChatViewport has one automatic scroll assignment, in its queued RAF callback.
React pre-mutation capture and ResizeObserver only capture/dirty layout. Multiple
commits preserve the original reading anchor until the frame. Tail following uses
the existing 24px threshold. Actual user movement updates intent synchronously;
a queued frame reads that newer intent. Programmatic writes and browser clamping
are attributed against the last written position. Prepending enters reading mode
even for a formerly short transcript. Reading mode tracks the first visible native
row, with bounded next/previous-row and clamped-offset fallbacks. Image/Markdown/
reasoning growth and shrink pass through the same observer/frame. Explicit latest
or a natural user return to the tail restores following. Unmount cancels the frame.
Trajectory retains its own interaction and virtualization owner.

Subscriptions are responsible for transcript/history and exact action guards,
activity, Goal/Todo/queue, totals, and Trace separately. Immutable unchanged fields
retain references. Goal/queue controls observe their own native read-domain identity;
other token updates cannot release a control's readback fence.

TUI already folded events into a terminal-specific presentation model. Its ordinary
settlement snapshot loop is removed; native read-domain facts enrich the existing
history adapter. Cursor gaps and coalesced recovery now use the same protocol
contract. Its terminal renderer and normalized state are deliberately retained.
No new state-store package or compatibility decoder was introduced.

# Agent Conversation ownership and resource bounds

rustX owns canonical messages and history. The browser renders two authoritative
read products and retains only replaceable read caches:

- `session/attach`, `session/snapshot`, `session/subscribe`: current/live projection.
  Contiguous notifications advance the native read model through the deterministic
  client fold. Initialization, recovery and explicit reconciliation replace from
  authority; transient state never survives an authoritative replacement.
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
Historical interaction/publication audits are read-only; a native `ask_user`
questionnaire's request and settlement audits are told by its call's question row
instead (matched by interaction id on the loaded page), so they render no body. Live Approval,
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
The native `ask_user` call (ToolId `tool-ask-user`) uses Harness's question row:
`bindings/ask-user.ts` reads its verdict from the persisted arguments and result
(waiting, `N/M answered`, cancelled for a decline, interrupted for a cancelled
call) and pairs each answer with its question by the echoed `question_index`, so
an omitted (skipped) question reads as not answered. A failed call, or a record it
cannot pair, keeps the generic card with the raw input and output.
Background execution uses its native ExecutionId, not a fabricated call identity.
No subcall nesting is inferred from adjacency or Tool names.

Inside a settled Attempt, `bindings/step-groups.ts` applies Harness's step
grouping (its default `detailed` mode, which groups history only): reasoning,
Tool calls and bodied records collect into one group until a reply (visible
Assistant text, refusal or media) or an independent message closes it. Interaction
audits join the group of the Attempt they name. A group renders whole where it
starts, so canonical order never changes; its collapsed title ranks the top three
Tool categories by distinct calls (`Read files, ran commands…`, `向用户提出了问题`,
or `Analysis completed` for reasoning alone) and its body is capped and scrolls.
A live or page-cut Attempt keeps every row in place.

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
runtime's. The client folds `agent_status_composed`, including its exact native
eviction identity. Cold attach and recovery install the native bounded window;
ordinary events advance it without a snapshot reread.

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
resync discard unsafe history and invalidate in-flight reads. Native bounded replay
joins the acquired snapshot cursor; timestamps and lexical IDs never order events.

`ChatViewport` measures stable row keys before React mutates the DOM. Prepending
keeps that row's viewport offset and enters history reading. ResizeObserver marks the frame dirty; its single commit
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
SQLite stores this provenance atomically in the existing bootstrap row,
checks it on repeated initialization, and refuses obsolete stores without migration.
The native `After` validator accepts local evidence and inherited provenance through
one shared projection, while still requiring the exact destination append revision.

The tail follows Harness `MessageIconActions`: Copy, Regenerate (rustX's only
extra lineage action, seated where Harness places extra actions) and one branch
action, Branch into a new Session (`session/fork`), then the Turn usage pill and
time. A completed response already names its exact boundary, so both lineage
actions run on the click, as Harness `forkAt` does: no confirming chooser opens,
the response actions lock until the transition settles, and a failure surfaces as
the App notice. In-Session Branch is reached through `/branch` and Session tree.
Branch into a new Session uses `side: after` and the immutable Surface revision that first appended the
closing response. The resulting prefix includes that Assistant response and the
composer is empty. The native owner validates the exact response/revision pair
and durable completion. Compaction and later appends do not change that historical
cut. Unknown revisions and mismatched boundaries fail visibly, without refreshing
or replaying the mutation. `session/tree` resolves the attached Conversation's
node, never the Session's mutable default. Independent Fork still copies native
uploads in the inherited prefix before publication; in-Session Branch (`/branch`)
shares the Session's upload ownership.

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

Paperclip, drop and paste use one client-lifetime intake owner and the native
advertised policy. Valid and rejected selections remain visible; an over-count
selection produces one bounded summary. Filename is not identity. Any rejected,
pending, failed or uncertain item gates Send until resolved or removed.

Uploads use metadata preparation, a separate bounded binary capability socket,
and exact-operation status reads. Ready means native receipts were read after the
ready commit. Retry is available only after absent/pre-ready-failure evidence;
uncertain uploads offer Check status. Removing a card releases browser presentation
resources and never rolls back native storage. Native authority and Composer binding
fence completion publication; reconnect does not retransmit files.

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

The mandatory App Server vocabulary is v35 (`rustx.app-server.v35` and generated
`protocol/app-server/v35.ts` / `v35.schema.json`). v12 and earlier initialization and
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
start from zero; these two quantities are deliberately separate. Its `timing`
follows Harness session statistics rather than a per-response exact aggregate:
LLM time sums measured requests' dispatch-to-terminal spans, Tool time sums
foreground Tool start-to-terminal pairs within their Attempt, TTFT is the mean
over requests that produced output, and TPS divides output tokens by decode time
over requests that report both. Each figure is absent until measured.

The Composer Context seat renders only compaction lifecycle and occupies no
stack seat while idle. Under the Composer card, the Harness Detailed-mode dock
shows three pills, each opening a trigger-anchored stat dialog (Base UI
Popover): Turn/Step counts with whole-session decode speed (LLM time, Tool time,
mean TTFT, TPS from native `statistics.timing`), reported tokens with cache hit
(uncached and cached input, output, and an explicit usage-report coverage row
when some requests did not report), and the last measured request's context
ring. The ring reads `last_request_occupancy` only while the view is connected
and attached; its numerator is the provider measurement, and its System prompt /
Tool definitions / Messages parts are the native `ceil(bytes / 4)` breakdown, the
messages part being the measured remainder. No Web tokenization or browser-clock
timing is used.

#364 merged as `3063ebd6`. Its native `GenerationEvidence` is the single request
clock contract consumed independently by Trace and completed-response projections.
Chat never reads Trace. The completed Turn's actions carry the Harness Turn
usage pill: its dialog shows the exact total, the requests' distinct models,
cache hit, uncached and cached input, and output with reasoning. The process
header owns the Turn duration (successful Attempt completion minus start); the
per-response timing (first-request TTFT from dispatch, summed
first-output-to-terminal work, output speed over fully covered positive
generation spans) stays native evidence. Unknown
endpoints/usage remain absent; zero measured generation is zero with no rate.
Known no-output requests add no decode span; missing request evidence invalidates
exact aggregate generation. Failed requests with evidence remain included.

Immutable bootstrap provenance preserves response timing and usage through
Branch/Fork/reopen/deeper lineage without copying source execution records.
Destination execution totals remain destination-local. Mandatory versions are
App Server v35, Runtime Client v53, SQLite v44, and Session catalog v13, with no
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

## New Conversation and completed process (#402)

`App` owns a center route: New Conversation is an explicit draft route, distinct
from a native Session route. Its text, File objects, selected registered Workspace
handle and optional Session model intent are presentation data. Opening it or
changing its Workspace creates nothing native. The Product Host lists, adopts and
resolves Workspace handles; the browser never supplies an arbitrary path.

`AppServerClient.firstSubmissions` owns the pending first input for the client
lifetime. Native `session/create` commits identity and initial model configuration
without composing a runtime. Its decoded acknowledgement synchronously publishes
the Session-scoped operation, then `App` navigates to the Conversation, even when
attach or the sidebar catalog is parked. `ConversationComposer` only observes;
remounting never restarts continuation. The owner retains ordered File references,
text and acknowledged receipts together until admission or explicit discard.

`firstSubmitPort` transfers the expected navigation fence once and preserves
endpoint, generation, authority revision, exact attachment and Conversation fences.
Unrelated navigation prevents further dispatch without erasing acknowledged facts.
A retired connection's intent remains inspectable and discardable, never replayable.
Attach/readiness gates uploads and `turn/start`, not Conversation visibility.
Creation success is not admission; pending intent is not canonical history. The
localized composer distinguishes preparation, upload, admission and uncertainty.
Conflicting input/model actions remain disabled until admission or explicit discard.

Display uses explicit choice, then browser preference, then the native default
projection. Creation sends only explicit choice or browser preference; omission
lets the native creation owner capture its current default. There is no post-create
initial-model mutation or repair snapshot. Later intentional model changes remain
native operations. Catalog membership invalidation independently refreshes the
existing catalog. See [startup ownership and evidence](../docs/issue-419/ownership.md).

Composer model choices come only from the Workspace read's native
`session_models` catalog, the one the created Session serves from `session/models`.
Order, reasoning profiles and the default profile are native. A configuration-only
model is never offered. Native unavailability, or a draft model the catalog stops
publishing, blocks Send without creating a Session.

The composer permission seat binds the same Workspace target actor and semantic
approval unit as Settings. It offers only native `policy` and `full_access`, uses
the exact Host Workspace handle and revision, and rereads confirmed commits through
the existing transaction coordinator. A confirmation gates elevation. The source
controls future admission; an already-admitted Attempt remains frozen. Composer
model intent is different: it never authors the Workspace default model.

App Server v35 / Runtime Client v53 project one `turn_process` owner on exact
canonical Assistant and Tool members. Native Journal identities, whole-process
counts and an immutable control cursor survive unsuccessful settlement and
bounded paging. Failed/stopped processes stay open; successful final-answer,
inline-reasoning and TurnTail contracts remain unchanged. Agent Status retains
its independent native anchor. See [terminal process ownership](../docs/issue-406/terminal-process-ownership.md)
for control placement, reconstruction and the deliberate selective lineage policy.

Reasoning uses the compact variant of the existing Markdown renderer; ordinary
answers use normal Markdown. Parsing, streaming and sanitization are shared.
Exact native Tool IDs select adapted Harness Terminal, Diff, Read and Search
bodies. Bash uses its native JSON `combined` field, never output-text inference.
Write/Edit show explicitly labelled requested changes, not an invented filesystem
baseline. Read/Search preserve bounded opaque native output without fabricating
line coordinates, match counts or parsed structure. Unknown Tool IDs retain the
bounded generic body and all renderers retain native lifecycle and artifacts.

### PR #404 review corrections

A known pre-commit rejection returns first-submit to editable drafting with its
error and draft retained. Typed uncertain outcomes remain inspection-only. The
client owner records the native acknowledgement before the next effect checks
authority, so a stale continuation retains the real Session rather than erasing
its commit. Attach/upload/send remain fenced.

New Conversation reads submission readiness from the existing Workspace Settings
target and approval-unit transaction. Send is disabled (the draft stays editable)
while writing, awaiting authoritative observation, uncertain, conflicted, holding
unapplied intent or otherwise denied by the target. A post-commit source read
settles this fence: new Session composition resolves canonical sources; it does
not wait for application to unrelated resident Sessions. The runtime still freezes
approval when admitting each Attempt. No permission queue or replay is added.

Completed native owners are indexed even when only final text is loaded. An exact
Conversation/Attempt-owned Status can supply the only foldable presentation. A
separate finite disclosure seat precedes the earliest controlled entry body or
anchored Status. The Status stays after its original anchor; an independent User
body stays outside the fold, before that seat. Pagination may move the seat but
never changes the native disclosure key or canonical membership.

The selected Composer has one retained intake owner. Incompatible Session,
Conversation, draft binding or authority replacement retires its File references;
same-binding remount/reconnect retains recovery state. Client disposal clears all
intakes. First-submit sealing transfers ownership to FirstSubmissions before create;
admission/discard releases those files. A known rejected create returns files to the original live intake for editing.
User-input count/byte limits are distinct from transfer limits and are independently
validated by the native receipt collection owner before turn admission.
