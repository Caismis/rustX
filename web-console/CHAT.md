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

The composer retains at most eight File drafts, each at most 256 KiB (2 MiB total).
Pick/drop order is preserved. Effective capabilities come from `settings/model`;
unsupported image/file input refuses before upload or turn admission, retaining
text and files. Supported drafts upload sequentially and enter one typed
`turn/start` (or native steer) content sequence with opaque artifact references.
There is no optimistic canonical user message. Currently installed provider
adapters support text input only: no provider multimodal translation is added.

`artifact/upload` and `artifact/read` use the existing conversation ArtifactStore
through Runtime Client and the typed App Server route. One-shot base64 is limited
to 256 KiB decoded / 349,528 encoded bytes, below the existing 1 MiB frame/request
limit. No chunks or upload accumulator are needed. The browser admits at most two
outstanding artifact operations; the server retains its existing 16 in-flight
requests per connection and configured finite connection bound. All operations
use existing attachment/incarnation admission and operation leases.

Upload is a mutation: a lost response is uncertain and is never replayed. Read is
a replaceable read. Transfer failure preserves drafts; retry is explicit. Aborted
browser reads discard results after fencing. A completed but unused upload stays
conversation-owned until Session deletion. No turn is manufactured to discover
provider rejection. The native upload gate checks the effective modality too.

Artifact allocation reserves and syncs each identity before returning it. Cold
reopen scans reserved/written IDs; byte writers use create-new, never truncate.
Read accepts only the owner's opaque ID form, rejects symlinks/non-regular files,
and limits reads even if a file grows. Paths never occur in carrier DTOs/errors.
Session allocation/access and deletion remain native owners; cold runtime unload
preserves artifact bytes. Browser code has no artifact-directory or arbitrary
filesystem read access. Tool artifact MIME metadata controls presentation only,
not provider admission.

Per visible view, ArtifactResources retains at most 16 URLs / 4 MiB artifact bytes
and two outstanding reads, with no read queue. Images/files are loaded explicitly.
Cards release URLs on unmount/retry/decode failure. View/attachment/connection
replacement disposes all URLs and fences unresolved reads. A failed image never
retries automatically. Draft preview URLs are separate, at most eight / 2 MiB,
and are revoked when their draft card is removed/replaced/unmounted. Artifact
payloads are omitted from the finite protocol diagnostic log.
