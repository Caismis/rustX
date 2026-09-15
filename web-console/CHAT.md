# Chat ownership and resource bounds (WEB-02)

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

`Conversation` renders durable entries by native cursor order and identity.
`ChatMessage` binds canonical user/Assistant/Tool facts to the WEB-01 Markdown
renderer and cards. Streaming uses the server's message ID; a canonical message
of that identity wins immediately, and settled/replaced attempts lose stale
partials. Surface messages outside the loaded page are not appended as history.
Typed current context is disclosed separately. Subagent, Workflow, foreground and
background activity are current adjuncts, not fabricated historical placements.
Historical interaction/publication audits are read-only; live Approval,
Questionnaire and Review retain existing typed settlement and uncertain-outcome
controls. No Todo/Goal/queue derivation, Trace, or active retry/fork control is added.

## Paging and reconnect

The cache holds at most 512 entries and an 8 MiB conservative UTF-16 serialization
budget; each older request asks for at most 64 entries. It is a contiguous durable
window, never canonical persistence. Current refresh preserves it only with a
matching durable cursor and fact identity. Current entries win overlaps. Missing
continuity or a capacity overflow replaces the window with the current page;
capacity replacement has a visible diagnostic. At the entry bound, Return to
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

The mandatory App Server vocabulary is v4 (`rustx.app-server.v4` and generated
`protocol/app-server/v4.ts` / `v4.schema.json`). v1 initialization and v1-only
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
