# Session-owned workspace uploads

A user upload is a durable Session-owned mutable workspace resource. ArtifactId
is neither its canonical identity nor its model access contract. Tool-generated
managed artifacts, text spill and background output retain their separate owners.

## Allocation and commit

The native `SessionController::upload` accepts an addressed Session/node and an
ordered vector of safe basenames plus decoded bytes. App Server routing proves
which admitted Session is addressed; attachment and runtime lifetimes do not own
files. Web, TUI and headless clients can all use the same Session API. A remote
client must transfer bytes, never assume its local path exists on the server.

Files live at:

```
<admitted-workspace>/.agents/uploads/<session-id>/<batch-id>/<safe-name>
```

The catalog records each random runtime-allocated batch, its admitted absolute
workspace, ordered names, opaque receipt tokens and readiness. Cwd changes do not
reinterpret old allocations. Canonical references contain only `batch_id` and
`name`; the owning Session resolves them through its registry after restart.

1. Retain Session allocation access to exclude deletion; serialize allocation
   preparation through the existing controller preparation owner.
2. Durably commit the allocation claim to the catalog before filesystem writes.
3. Open directories component by component through directory descriptors with
   `O_NOFOLLOW`, create the batch exclusively, create files with `O_EXCL`, write
   complete bytes and sync files and directory entries. Reopen and verify the
   batch identity before readiness.
4. Durably commit readiness. **This is the semantic upload commit point.** Only
   then return usable server receipts. Catalog rename is visibility; successful
   directory durability barriers are required before claiming success.

A materialization failure leaves owned, unready cleanup work. Response loss after
commit is uncertain to the client. It does not revoke ownership and never permits
a blind mutation replay. Successful upload alone commits no conversational User
message. Failed turn admission requires no browser rollback or file deletion.

Basenames reject absolute/path-shaped names, both separators, traversal, NUL,
control characters, reserved platform names and invalid filename forms. No client
supplies a destination path. Upload workspaces cannot themselves be inside an
`.agents/uploads` hierarchy: Session cleanup roots must never nest. Existing
files/batches are never overwritten.
Uploads remain ordinary mutable files; edits after commit are intentional workspace
semantics. They may appear in VCS status. rustX never edits ignore files.

## Canonical metadata and model projection

Turn input contains user text plus completed server receipt references. The
Session validates ownership and readiness and creates `uploaded_file` canonical
facts. A client cannot serialize a canonical upload struct to grant itself trust.
Cross-Session receipt reuse fails. Transcript cards read typed metadata directly.

The shared `model::uploads` projection resolves files through the Session owner
and renders exactly one prefix per upload-bearing User message:

```xml
<user_uploaded_files>
  <file name="report.pdf" path="/project/.agents/uploads/ses_01900000-0000-7000-8000-000000000001/batch/report.pdf" />
  <file name="data.csv" path="/project/.agents/uploads/ses_01900000-0000-7000-8000-000000000001/batch/data.csv" />
</user_uploaded_files>

Please analyze these files.
```

Order is canonical upload order. Attributes are XML-escaped. After the two-newline
separator, user-authored text bytes are preserved. No-upload turns are unchanged.
Neither file bytes nor system-prompt changes enter this projection. Images use
this same filesystem contract without requiring provider-native image modality.
The model uses ordinary native filesystem tools with the absolute paths.

Local Runtime supplies the core-owned `UploadProjectionResolver` capability;
Context Engine and Tool runtime know no catalog path or persistence implementation.
Context estimates apply the same rendering before measuring input and retained
conversation budgets. Provider adapters contain no upload XML logic. Request
Snapshots freeze resolved paths separately from canonical identity so historical
request replay does not consult changing workspace state.

## Fork, branch and cleanup

Same-Session branches reuse the registry and perform no file copies. Independent
fork/clone scans exactly the prepared canonical Ledger cut plus the selected User
message restored into the editor. It copies only those required current file bytes
into destination-owned allocations, preserving batch/basename identities. Copies
and durability barriers complete before the existing catalog publication boundary.
A missing required source fails before visibility.

The selected boundary is outside destination history but is returned faithfully as
ordered `UserInputBlock` draft input: text and destination-issued upload receipts.
Its uploads therefore belong to the destination before publication too. Source
receipts cannot be reused in the destination. Same-Session branch drafts receive
receipts from the shared Session registry. The TUI preserves those receipts and
exact restored text through later submission; no client invents canonical facts.

Every private independent Session preparation has one durable claim. Its Session ID
owns the native private Session/conversation allocation; its frozen workspace list
owns destination uploads (the list may be empty). Low-level copying never cleans
or consumes claims. The controller cleans the complete frozen workset outside the
catalog mutex, and only successful cleanup permits claim removal. Cleanup failure
returns both operation and cleanup errors while retaining the claim. App Server
and local CLI startup retry that same workset. Publication atomically consumes the
claim into the visible Session.

Same-Session branch publication failure has a different cleanup owner: it removes
only the unpublished conversation directory. It never discards the existing
Session or touches shared uploads. Pre-rename failure retains its original
`NotCommitted` semantics; post-rename visibility returns the authoritative node
and durability diagnostic without unpublished cleanup.

Publication transfers the prepared upload registry into the destination Session
atomically. Source deletion cannot invalidate the destination. Uploads are mutable
workspace files, so the copy captures their current bytes, not immutable originals.

Session deletion freezes every historical admitted workspace allocation alongside
its existing finite cleanup record. It derives only `.agents/uploads/<session-id>`
roots, never accepts a client cleanup path, and runs outside catalog metadata
locks. Descriptor-relative recursive cleanup does not follow symlinks. Ancestor
symlink substitution fails closed; missing residue is idempotent. Failure retains
the existing committed-cleanup-pending result. Recovery retries exactly the frozen
record without scanning arbitrary workspaces, and cannot delete the workspace or
another Session's root.

## Protocol and schema boundaries

App Server v10 is the one mandatory vocabulary; its WebSocket subprotocol is
`rustx.app-server.v10`. `session/upload` replaces the old user carrier. `artifact/read`
remains for Tool-managed artifact presentation only. Session catalog schema 12,
SQLite schema 42 and native Runtime Client version 42 reject older development
contracts without migrations or compatibility modes.

JSON/base64 is a bounded current carrier: 1–8 files, at most 256 KiB each and
512 KiB per batch, additionally subject to the 1 MiB JSON frame limit. Core
Session methods receive decoded bytes, leaving a direct seam for a future binary
carrier without adding a storage abstraction.

## Artifact audit

- Obsolete user carrier: removed from App Server, runtime client, Web and TUI
  method vocabulary; its capacity/provider-modality tests are replaced.
- Canonical uploads: `UploadedFileRef`, no ArtifactId, provider ID or absolute path.
- `FileReference`/`ImageReference`: retained for genuine managed Tool outputs,
  including MCP images and background-result inbound notifications.
- `ArtifactStore`, managed output, executor and background owners: their existing
  allocation, spill, capacity and output semantics are retained.
- `artifact/read` and Web `ArtifactResources`: retained solely for managed Tool
  galleries; uploaded-file transcript cards never invoke them.
- Markdown parser `ImageReference`: unrelated Markdown syntax tree, unchanged.
