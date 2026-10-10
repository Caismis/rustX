# Session-owned workspace uploads

A user upload is a durable Session-owned mutable workspace resource. ArtifactId
is neither its canonical identity nor its model access contract. Tool-generated
managed artifacts, text spill and background output retain their separate owners.

## Allocation and commit

The native `SessionController::upload` accepts an addressed Session/node and an
ordered vector of safe basenames plus raw bytes. `upload_correlated` also takes
the required operation identity; native callers of `upload` receive one before
allocation. App Server routing proves
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

## Child conversations

Child inputs use the same Session-owned upload transfer and receipt validation.
`agent/sendMessage` accepts text plus receipts, or receipts without text. Native
routing validates Session ownership before passing canonical `UploadedFileRef`
values through active guidance or resumed admission. The child model resolver is
bound to the owning Session rather than assuming that its Conversation is a root
Session node. No duplicate child upload store or client-supplied path is used.

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

App Server v41 is mandatory (`rustx.app-server.v41`). Session catalog schema 15
retains native upload operation correlation and known pre-ready failures alongside
each allocation, and adds the execution-ownership generation described in
[the App Server contract](app-server-protocol.md). SQLite schema 49 is unchanged.

### Control and data contract

`session/uploadPrepare { target, operation_id, files: [{ name, size }] }` validates
the exact writable attachment, safe metadata, native policy and transfer capacity
before receiving payload. `operation_id` is 32 lowercase hexadecimal characters,
created and retained by the client before dispatch. A random 256-bit capability
returns as a relative `/session-upload/<capability>` path, valid for 60 seconds and
single use. Transport API keys are never in this path. Remote clients resolve only
against their selected native WebSocket origin; an owned stdio child supplies an
explicit loopback port bound to that exact capability path. Neither Product Host paths nor Workspace association
confer upload authority.

The separate socket negotiates `rustx.session-upload.v1`. For each declared file,
the server sends `next` and accepts one nonempty binary message of at most 64 KiB,
never exceeding the remaining declared length. Files follow metadata order; empty
files consume no binary message. After the exact declared bytes, the server sends
`finish` and requires the client's `finish` terminal marker. Early termination,
extra bytes in place of that marker, oversized messages and nonbinary data chunks
fail before native allocation. The whole receive phase has a 60-second deadline.
No HTTP request body or Content-Length parser is involved. The ordinary JSON
WebSocket still rejects binary messages and retains its 1 MiB bound.

The carrier hands bounded byte vectors to `SessionController`, which alone claims,
materializes, synchronizes, verifies and commits ready. The carrier's `settled` or
`check` message carries no receipt: clients read `session/uploadStatus` for the exact
operation. A prepared/active carrier is unresolved even before native allocation.
Native states distinguish absent, unresolved, known pre-ready failure and ready.
Ready returns the original receipt identities, including after response loss or
restart. A failed materialization retains native-owned cleanup residue. An
unresolved durable claim never authorizes Retry. A new explicit retry after absent
or failed evidence uses a new operation identity; old allocations are never reused.

### Finite budgets

The typed initialization capability `upload_policy` is the only policy source:
`max_file_bytes = 2,097,152`; `max_files_per_transfer = 8` and
`max_transfer_bytes = 4,194,304` bound one preparation. Separately,
`max_uploads_per_user_input = 8` and `max_upload_bytes_per_user_input = 4,194,304`
bound one submitted User input, even across multiple transfers. Two prepared-or-active
transfers (`max_concurrent_transfers`) each allow 65,536-byte messages (`max_chunk_bytes`). There is no pending transfer admission
queue. Prepared descriptors retain permits; expiry releases unused preparations.
Consumption retains the permit through native settlement even if its reply is lost.

Native payload buffers total at most 8 MiB across both transfers. Reserve a further
512 KiB for bounded socket/frame working buffers (8.5 MiB aggregate, excluding
metadata and OS TCP buffers). Maximum admitted concurrent file materialization is
8 MiB. Committed Session storage and native partial residue are retained until native
cleanup; this is an in-flight budget, not a total Session disk quota. Browser sending
has at most one 64 KiB slice in flight per transfer. TUI local-file reading is bounded
by the advertised file limit; its byte vector and Blob copies are separate client
memory, not native storage (at most three 2 MiB backing allocations plus a
64 KiB slice and the one-byte oversize sentinel). Full bounded buffering avoids a second staging owner
or a speculative streaming storage strategy. Filesystem work runs on the blocking
pool, leaving cancellation, detach and status available on the control socket.

General multipart/resumable protocols and a new HTTP request-body parser were not
introduced. Explicit binary message framing reuses existing supported browser,
Node and tungstenite stacks. Archive-style native capability admission remains the
architectural pattern. See [the reference audit and decision](issue-434-decision.md).

Image decoding, `read_image`, provider image counts/token budgets, managed Artifact
reads and Session-file previews keep their independent policies. Uploading an image
does not invoke image processing or create a provider-native image block.

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

### Execution admission, drain and receipt-set authority

Preparation reserves finite transport intent and native exclusion; it does not
admit semantic upload execution. The binary handshake consumes the capability
under the same host mutex as `begin_drain`. Drain revokes every unconsumed
preparation. Each preparation owns one tracked supervisor containing its expiry
future and optional loopback listener; normal settlement cancels expiry immediately.
The transfer permit remains held until that supervisor has physically exited. A handshake losing this
boundary cannot mutate native storage. A winning handshake owns a separate host
upload-operation guard through carrier/native settlement. Ordinary request capacity
is independent. Neither control disconnect nor data disconnect after native handoff
cancels the native obligation. Truncated receive settles without allocation.

Drain waits for upload guards and tracked supervisor termination as well as
protocol/runtime owners. `finish_drain` requires zero upload guards, an empty
transfer registry and zero tracked actors. Forced-shutdown diagnostics expose
active uploads and remaining actors without asserting settlement. Remote transport
joins admitted carrier work, and owned stdio listeners obey the same host admission.
Unused expiry closes the prepared listener and releases capacity after actor exit.
Permanent attachment detach/close promptly revokes only unconsumed preparations
through this same owner. An exact status read can then report native Absent; it
cannot remain Unresolved solely because a dead capability remains until expiry.
Consumed work survives detach and remains server-owned through native settlement.

`SessionController::uploaded_content` validates the entire receipt collection once
for both `turn/start` and `turn/steer`, before inbound admission. It checks count,
Session ownership, ready status and the sum of native `UploadEntry.admitted_bytes`.
This size is persisted with the original claim and retained across native copies;
later mutable-file metadata and client claims cannot redefine admission policy.
Catalog validation rejects empty/over-count allocations, oversized admitted file
sizes, and checked aggregate admitted sizes exceeding the transfer bound. This
also applies on reopen and to copied subsets; current mutable bytes need not equal
the original admitted fact.
Canonical block order remains the submitted order. Multi-file native transfers
remain supported; Web and TUI ordinarily transfer one file at a time.

Materialization and final verification failures share durable Failed settlement,
retaining cleanup ownership. A ready publication proven NotCommitted also settles
Failed. Failure to persist that classification leaves uncertain evidence. A
post-rename ready durability diagnostic is never overwritten as Failed: the
catalog's visible ready record and original receipts remain the read-repair authority.

Browser intake owns File references until removal, successful admission/clear,
incompatible binding retirement or client disposal. There is one selected Composer
owner; same-binding remount/reconnect retains it. Only committed semantic
Composer activation retires an incompatible owner. Speculative or abandoned
React render never clears, rebinds or retires the committed draft. Native authority
replacement and client disposal explicitly retire applicable owners. First-submit sealing transfers File ownership to
FirstSubmissions synchronously; intake is empty before create starts. A known creation rejection returns files and local IDs to the still-live original
intake for editing; a retired origin cannot reclaim them. Otherwise the retained
first-submission owner keeps the intent until admission/discard.

### Client recovery evidence

Both immediate intake and retained first submission preserve typed upload evidence.
Ready captures the original receipt; Absent/Failed permits explicit Retry with a
fresh operation ID. Unresolved or an unrepairable transport loss permits only
Check status for the original operation, never byte replay. Before transport
dispatch, local refusal or loss of a queued prepare is a known no-side-effect
failure: Retry is legal with a fresh operation ID, with no status read or carrier.
Once mutation dispatch begins, response loss remains uncertain (including a send
exception without proof of non-dispatch). An explicit server prepare refusal
publishes no new intent; the client reads the exact operation to distinguish no
commit from a previously known operation. A captured receipt survives subsequent
local authority loss and pauses continuation without allocating again.

Queued files stopped before dispatch are locally known to have no side effect.
Rebinding makes them retryable, never uncertain, and never automatically dispatches
them. Only dispatched/fenced work needs reconciliation. Unavailable DataTransfer
file items are visible rejected metadata, not fabricated zero-byte files.
