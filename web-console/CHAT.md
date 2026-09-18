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

## Historical Fork, Branch and Retry (WEB-05)

User-message rows offer Fork, Branch and Retry / Regenerate. Command discovery also
opens Fork/Branch boundary selection. `session/boundaries` supplies the exact User
MessageId and Surface revision; `session/tree` resolves the **attached Conversation**
to its native node, never to the Session's potentially different default node.
The frozen selection includes Session, attachment/incarnation, node, revision and
message. No stale-revision error triggers refresh-and-retry. Native immutable older
revisions remain valid cuts; unknown revisions and invalid boundaries fail visibly.

Fork creates an independent Session. Branch creates an in-Session node. Both native
cuts end **before** the selected User message and return its `editor_content` as an
uncommitted draft. The UI opens only the acknowledged destination and restores its
native text/upload receipts. Independent Fork upload copying is entirely #319's
native responsibility, including editor-boundary uploads before publication.
In-Session branches share Session-owned uploads; the browser copies no files.

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

The mandatory App Server vocabulary is v8 (`rustx.app-server.v8` and generated
`protocol/app-server/v8.ts` / `v8.schema.json`). v7 and earlier initialization and
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
