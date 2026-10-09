# Explicit delivery and safe Session-file previews

`present` is an ordinary native Tool registered through `ToolDefinition`,
`ToolExecutor`, and `NativeToolRegistration`. The normal Tool Plane and Agent Loop
own invocation, cancellation, settlement, and atomic canonical Tool-result commit.
There is no delivery database, special Agent Loop branch, or browser model state.
`present` belongs only to the root Session-capable Agent plane. Named child
selection produces the existing ScopeUnsupported admission diagnostic; child
invocation overrides reject it as ChildUnsafeSelector, and frozen child
composition cannot register it. Child-only Conversations have no published
Session filesystem mapping, so this Tool is intentionally unavailable there.
Child-file promotion or delivery ownership is not part of this feature.

## Declaration

Input is `{ "files": [{ "path": "报告 file.md", "description": "Report" }] }`.
There are 1–8 ordered entries. A path is 1–4096 UTF-8 bytes; descriptions are
optional, at most 512 Unicode characters, and preserved verbatim, including empty
strings. Paths use the Agent's native filesystem vocabulary: relative to the
Session cwd, or absolute inside that exact canonical cwd. `.` components normalize;
any `..`, symlink component, missing file, or non-regular target fails the whole
call. Every entry, including duplicates, is validated. The first normalized
occurrence wins, preserving its input position and description. A later successful
invocation is a separate declaration and owns one separate terminal Tool result.
No cross-invocation suppression or replay occurs.

Write, Edit, Bash, Python, MCP-driven commands, or another process may produce the
file. Declaration reads metadata, not content; it creates, copies, publishes,
executes, and mutates nothing in the filesystem. Oversized regular files may be
declared but their content read fails explicitly. Cancelled, failed, denied,
interrupted, or malformed invocations cannot produce successful delivery cards.
Ordinary writes and prose such as “I created report.md” do not declare delivery.
Tool-owned JSON property names and ambiguous basenames confer no authority.

The typed `ToolExecutionResult.deliveries` field is separate from `content` and
managed `artifacts`. `AgentLoopExecution::commit_tool_result_batch` prepares normal
`MessageBlock::Tool` blocks, then atomically calls
`ConversationStore::append_canonical_batch_with_events`. The transcript renders
cards only from successful committed Tool messages. Earlier execution-completion
observations do not authorize cards. Cards sit outside completed activity's
collapsed seat while keeping its native message/turn anchors.

## Two closed identities

Managed `FileReference` / `ArtifactId` refers to immutable Conversation-owned
ArtifactStore bytes and uses `artifact/read`. Existing allocation, spill,
capacity, and lineage materialization semantics remain unchanged.

A `SessionFileReference` identifies an original Conversation, cwd device/inode
identity (lossless strings), normalized relative path, original basename,
description, and inert MIME classification. It is a reference to a mutable file,
not a snapshot or an ArtifactId. File modification or regular leaf replacement
between reads is visible on reopening. Deletion reports Missing; revocation
reports Unauthorized; missing original Session/node/mapping or replacement of
its root reports Unavailable. File-read bodies never enter delivery metadata,
transcript/event snapshots, or browser storage.

Clone, fork, and tree-branch cuts retain delivery references verbatim. They do
not remap them to the destination Conversation, copy workspace outputs, or search
for a same-named file. The original Conversation must still belong to a published,
non-deleting Session node in the native catalog. Cold reopen resolves that same
catalog identity and current native mapping. A deleted or unpublished original
lineage is unavailable even if an unrelated Session has the same cwd spelling or
filename. Child-only Conversations without a catalog filesystem mapping are also
unavailable; this feature invents no preservation guarantee for them.

Changing the original Session's Workspace association cannot retarget a delivery:
the newly resolved cwd must have the recorded root identity. A viewing Session's
current node/cwd is never used to resolve the historical file. Preview/navigation
acquires native read/allocation ownership without starting or resuming an Agent,
Attempt, or model request.

## Authorized read and containment

App Server v38 has no public `session/fileRead` Method. An ordinary authenticated
App Server connection cannot enter the file-read seam, even with the exact
attachment, canonical Tool message ID, delivery index, Session cwd, and reference.
Initialize client names are metadata and have no authorization role.

Two transport-authenticated callers enter one native owner,
`app_server::delivery_access`: the private Product Host lane below, and an App
Server connection holding *delivery access*. Each passes a cancellation authority
minted by its transport authentication; no JSON field, client name, coordinate,
path or root list can manufacture one.

### Delivery access for App Server clients

`delivery/read` and `delivery/locate` serve trusted clients such as the TUI. Delivery
access is granted only when the connection is established:

- **stdio:** `--stdio-delivery-access` delegates it to the stdio owner — the process
  that spawned the server and owns its pipes. That owner already runs with the same
  user authority. The TUI passes it for the child it spawns. It is the owner's explicit
  composition choice, never implied by stdio.
- **WebSocket:** `--delivery-access-token-file` names a separate 43–128 character
  owner-only credential. The file must be regular, unreadable by other users, and
  is never followed through a symlink. Startup rejects reuse of the transport token
  or the Product Host secret. A client offers `rustx-delivery-access.<secret>`
  beside `rustx-token.<token>`. A wrong value fails the handshake with 401, and the
  response selects only the public subprotocol. The secret never enters ordinary TUI
  or Web configuration, URLs, logs or protocol fields.

Without access both Methods fail `session_file_read` / `unauthorized` before any
lookup. With it, the target must be one of the connection's own attachments. Native
lookup then resolves the exact committed Tool-result message and delivery index to
the original Conversation, its current catalog mapping and recorded root
device/inode, exactly as for the Product Host. The difference is the root policy:
the Product Host additionally requires a currently registered Workspace root; a
delivery-access connection is native-process authority and uses the original
mapping alone. Connection close cancels the connection's authority and native
credential removal cancels every grant, so admitted reads fail at their next fence
and publish nothing. Reads share the two native permits, which are released only
when the descriptor read physically settles.

Each request is one operation owned by its connection, registered under its exact
JSON-RPC id before native admission:

```text
register (id) -> admission -> fences -> physical settlement -> response queued
  -> transport writer, once every earlier record is written:
     [shared] decide + the transport accepts it   (publication linearization point)
  -> rest of the record
delivery/cancel (same connection, same id): Running -> Cancelled, request token cancelled
revocation (credential, connection, close, detach, drain): [exclusive] authority revoked
```

`delivery/cancel` is accepted only while the request is running here. The request's
token, a child of the connection's delivery authority, then fails its next native
fence. The request still answers exactly once, with `delivery_cancelled`, after its
native work settled. The serialized response waits in the ordinary bounded
outbound queue with its publication owner.

**Publication linearization point.** A delivery response is published when its
transport accepts it, and only then: the stdio pipe takes its first bytes, or
tungstenite takes its WebSocket frame. The writer reaches that point only after
every earlier record has been written. There, in one synchronous step with no
suspension, it decides which record to offer, offers it to the transport, and
settles the request only if the transport accepted. The record offered is the
produced response, or the same id's typed failure when a cancellation was
accepted, the connection's delivery authority was revoked, or the attachment was
detached. While the transport accepts nothing (backpressure), nothing is decided,
and the next offer decides again. Accepted bytes and frames are never retracted.
If the transport ends before accepting the response, nothing of it is sent.
Cancelling or revoking never closes the connection or affects unrelated responses.
The in-process caller decides the same way, with its return as the acceptance.

**Ordering against cancellation and revocation.** The step above holds two
locks: the request's own state lock, and the shared side of one host-wide
revocation order (`delivery_access::Revocations`, a reader-writer lock).

- *Request cancellation* (`delivery/cancel`) changes the request's state under
  its state lock. A cancel that completes before the step is observed by it and
  wins. One that arrives during the step waits until the transport accepted, and
  is then refused (`accepted: false`).
- *Revocation of delivery authority* runs in the exclusive side of the revocation
  order, and nowhere else. That covers WebSocket credential replacement or
  removal, connection revocation (drain revokes every connection first), close
  (authority and every attachment in one revocation), attachment detach, and
  Product Host disconnect or credential replacement. A revocation that completes
  before the step is observed by its decision. One that arrives during the step
  waits until the transport has accepted, and is ordered after it.

The authority a success needs is current when the connection's authority token is
not cancelled and its exact attachment is attached; every change of either to
revoked is one of the revocations above. So no cancellation or revocation can
complete between the decision and the acceptance. One that completes before the
acceptance prevents the success; one that overlaps it is ordered after it. Neither
lock is held across a suspension or an I/O wait: the shared side covers one
non-suspending transport poll, and the exclusive side covers a token cancellation
or an attachment detach. Publications share the order, so they never wait for one
another, only briefly for a revocation in progress. The lock order is fixed: the
WebSocket stream, then the revocation order, then the request's state. Revocations
take the connection's route table before the revocation order and never take
either of the others. No path takes them in another order. A Session runtime
ending its residency is not a revocation; an attached route pins residency until
after its detach.

What "accepts" means is each transport's own write step:

- **stdio** writes to a non-blocking pipe. The first write that takes any bytes
  puts them in the kernel pipe buffer; a full pipe takes none and returns pending.
  This is the physical boundary.
- **WebSocket** distinguishes five stages of a response: (1) queued as an App
  Server record; (2) held by an adapter (absent here, see below); (3) accepted by
  tungstenite; (4) written to the kernel socket buffer; (5) received by the peer.
  The publication point is (3). The reader and the writer share one
  `WebSocketStream`, each holding it for one non-suspending poll. Under that lock
  the writer first flushes every earlier frame, including any pong the reader
  queued, to the socket. Then it waits for tungstenite to be ready, decides, and
  hands the frame over with tungstenite's synchronous `start_send`. tungstenite
  writes the frame to the socket at once (write buffer size 0). If the socket
  buffer is full at that instant, the frame waits in tungstenite's write buffer,
  which holds at most one message, and is sent when the peer reads. It is not
  retracted. tungstenite exposes no socket writability before taking a frame,
  so this one message is the documented gap between (3) and (4). Stage (2) does
  not exist: `futures_util`'s `SplitSink` parked a frame in its own slot until a
  later flush obtained the shared lock and forwarded it, which is neither
  undecided nor accepted. The writer therefore does not use it.

The Product Host lane publishes its one response through the same publication
owner, the same WebSocket stream owner and the same revocation order.

A connection carries at most 16 requests in flight; a seventeenth ends it. The
server answers `delivery/cancel` without awaiting native work. The TUI client
counts a request until its response arrives, admits at most 15 ordinary requests,
and reserves the sixteenth slot for `delivery/cancel`. Cancellations take that slot
one at a time, in abort order; one whose request settled first is dropped, since
that response is already the outcome. Ordinary requests can therefore neither
block a cancellation nor turn it into a seventeenth request. A server that refuses
an owed cancellation breaks the contract, and the client ends the connection, so
the cancelled request settles with that terminal failure instead of silently
running on.

`delivery/locate` walks the same descriptors, runs the same fences and returns the
absolute server path plus the verified leaf device/inode, without bytes or a size
bound. A location is a server path, never a client path. A client may open it
locally only when it can show that it shares the server's filesystem (see the TUI
below); path spelling and loopback addresses are not evidence.

### Product Host lane

The launcher provisions a separate 256-bit process-ephemeral Product Host secret
in an owner-only file, passes its path with `--product-host-token-file`, and puts
its value only in Node's private Host configuration. It differs from the browser's
ordinary transport token. The browser bootstrap, catalog, JS bundle, URLs,
transcript, localStorage and ordinary protocol log never receive it. Independently
managed deployments must provision the same separate owner-only credential to
this native process and its trusted Product Host; without it file reads are
unavailable. Native startup rejects reuse of the ordinary credential, symlink or
non-regular secret files, and files readable by other users.

Only `/product-host/file-read` WebSocket admission accepts the private
`rustx.product-host.file-read.v2` subprotocol plus `rustx-product-host.<secret>`.
The response selects only the public subprotocol name, never the secret. Ordinary
transport credentials cannot authenticate this lane, and this credential cannot
authenticate the ordinary App Server lane. Handshake admission creates a native
cancellation authority; no ordinary JSON field can manufacture it. The socket
accepts one bounded internal read payload and creates no attachment or Agent.
On cancellation it revokes publication immediately, then joins the exact admitted
read before acknowledging a clean WebSocket close. In particular a descriptor
read running on the blocking pool must release its native permit first. Previously
dropping the socket's result receiver left that detached read alive, so a
browser-only cancellation change could not establish physical settlement. The
Node Host now waits for the clean close acknowledgement or a valid terminal read
response; abnormal transport termination retains the bounded Host admission slot
as unavailable. The private payload and all public App Server schemas are unchanged.

The browser's authenticated Product Host HTTP carrier accepts only authority scope
and exact target/message/index coordinates. The Node Host supplies canonical roots
from **current registrations**, after scope/endpoint admission. Configured picker
locations alone do not authorize file bytes (even though display classification
may describe them as operator-authorized locations). No browser association cache
or browser-supplied Workspace ID grants authority. Native lookup resolves the
original committed fact and current original Session mapping; its cwd must exactly
match a currently registered root. Viewing forks never substitute their cwd.
Remote/container reads use the addressed native filesystem, with no Host fallback.

Registration-root changes synchronously abort the Host's owned reads after the
metadata commit; later operations rebuild policy from the current registrations.
Host close/replacement and caller disconnect abort too. Rename/reorder alone keep
the same file policy. Socket disconnect, process shutdown, or native credential
replacement/removal cancels captured native authority. Credential replacement is a
native owner seam, never a public RPC; a restarted native process uses a new secret.

### Native fences and containment (both callers)

Native fences run at authenticated admission, before canonical lookup/allocation,
before open, after leaf open immediately before bytes, after byte/edge verification,
and at publication (the transport's acceptance of the response, on either lane). They check captured host authority,
attachment read authority, original mapping, exact root and recorded device/inode.
Publication retires with its owning socket; the Host rechecks its scope, operation
abort and root availability before returning to the browser. Allocation ownership
excludes physical Session deletion during reads. Revocation before these fences
fails or closes the operation without publishing bytes; completed prior reads are
not retroactively erased. Preview/navigation starts no Agent or model request.

The security opener uses the existing nix dependency. Starting at `/`, it retains
ancestor directory descriptors opened with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`.
The recorded cwd device/inode must match. `fstatat(AT_SYMLINK_NOFOLLOW)` confirms a
regular leaf before `openat(O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)`; `fstat` must match
the observed leaf identity and regular kind. Every ancestor edge and leaf is
verified against the retained descriptors before reading and again before return.
Replacing an ancestor or leaf cannot redirect an already-open descriptor; a
replacement observed during the operation fails explicitly. There is no
`realpath -> later ordinary open` containment boundary. A preliminary canonical
cwd lookup supplies the current mapping only; the descriptor walk and root identity
check enforce containment at open/read.

Directories, FIFOs, symlinks, devices, and other special files are rejected.
Nonblocking leaf open prevents a raced FIFO from blocking. Reads retain at most
limit+1 bytes, detect size/growth overflow, and never return truncated success.
Replacement races can fail; this is not an atomic snapshot of concurrent in-place
writes. Reopening deliberately reads current accessible bytes.

File failures use the closed `session_file_read` error with Missing, Unauthorized,
Unavailable, NotRegular, Replaced, TooLarge, Capacity, or ReadFailed reasons.
Diagnostics expose no unrestricted path. Stale attachment/runtime failures retain
the ordinary native failure vocabulary.

## Finite resources and viewers

| Policy | Limit |
| --- | --- |
| Session-file preview and Download | 524,288 bytes (512 KiB), inclusive |
| Session-file base64 carrier | 699,052 characters; below the 1 MiB native frame cap |
| Native Session-file reads | 2 owned reads per AppServerHost, shared by the Product Host lane and delivery-access connections; excess fails, no unbounded queue |
| Product Host file operations | 2 physical read obligations per Host instance; clean close/terminal response releases admission, unknown settlement keeps its slot unavailable |
| Browser private Host reads / retained preview URLs | 2 private permits / 3 aggregate URLs: 2 visible panes plus 1 transient Download |
| Managed Artifact transfers / URLs / bytes | Existing independent 2 / 16 / 262,144 bytes; preview leases separately obey the aggregate 3-URL limit |
| Rendered Session-file text / Markdown input | 512 KiB original bytes, fatal UTF-8 decode |
| Raster dimensions | At most 4096 on either axis and 4,194,304 pixels |
| Raster animation | Static only; animation fails inline but Download remains |
| Declaration / allowed-root list | 8 entries / 32 native roots |

Each browser Session-file response contains at most 699,052 base64 characters.
Its transient binary string, decoded text string, byte array, and Blob each
contain at most 524,288 characters or bytes as applicable; at most two reads
and three URLs (two visible panes plus one transient Download) are owned. These values are bounded before decoding or URL allocation. A raster's
logical RGBA output is at most 16,777,216 bytes, with dimensions checked before
browser decoding. Read buffers and decoded content are effect-owned and ephemeral;
only finite Blob URLs are retained by the resource owner.

`FilePreviewCoordinator` preserves the closed Artifact versus Session-file union
and separate concrete source authorities. The Session preview workspace owns
logical occurrences; one `FilePreviewLease` owns each visible selected body. At
most two bodies own original URLs, with a separate single transient Download
lease. Download does not navigate tabs or panes, always reauthorizes original
bytes and revokes its URL exactly once. Text/Markdown/code Download preserves
original bytes and filename, including CRLF, spaces and Unicode, never rendered
HTML. UTF-8 or raster decode failure keeps original Download available.
An unsupported format explicitly reports no inline viewer and remains downloadable
inside the same bound. No partial oversized download is offered.

Markdown uses rustX's existing mdast-to-React renderer: raw HTML stays literal,
URL schemes allow only HTTP/HTTPS/mailto links, embedded images remain inert alt
text, and KaTeX trust is disabled. No independent renderer, local resource loading,
remote embedded resource loading, HTML execution, or SVG inline execution is added.
Raster headers are inspected through image-size's Uint8Array API before browser
image decoding; active formats are rejected. No filesystem library API enters Web.

Each active occurrence lease owns its source, cancellation signal and original URL;
each mounted viewer additionally fences its asynchronous presentation callbacks.
Source replacement, retry, close/unmount, Session/node change, attachment replacement,
reconnect, native authority revision, and Product Host authority revision retire
old owners. Responses recheck captured attachment and both authority scopes before
base64 decoding or URL allocation. Obsolete successes, errors, loading completions,
and image callbacks cannot publish into the new keyed preview. Cleanup aborts
reads, unsubscribes through existing App ownership, and revokes each URL once.
Same-authority display refresh does not recreate the owner. Hidden tabs and
background Sessions retain only bounded view-state metadata, never URLs, file
bytes, workers or live derivation demand. Reload persists none of this workspace
state. The separate existing inline Conversation Artifact owner retains its
original 16-URL policy; these are not hidden preview documents.

Browser Session-file original reads use an exact private HTTP operation token. The Host
acknowledges the token in response headers after registering one of its two
active cancellation handles. Cancellation sends the token and exact Host scope;
the browser keeps the original response open until the native/Host read finally
settles. Waiting current intents are finite (two pane demands and one Download)
and removed immediately when their lease retires. Unknown transport settlement
fails private Host read admission closed for that Host authority. Converter-only
settlement uncertainty belongs to document admission and does not disable raw
Session-file reads or original Download; a lost document terminal witness can
conceal a native read and therefore still fails private read admission closed.
Ordinary managed Artifact preview/Download uses public `artifact/read` and its
independent ArtifactResources transfer limit, so none of these private settlement
failures disables it. Artifact document reauthorization instead uses the private
read seam and participates in private admission for the whole derivation. This avoids treating an aborted
fetch as physical read completion; no public native Method, authority or tab
registry is introduced.

The existing #430/#443 RightPanel and ChatViewport remain the geometry and scroll
owners. Delivery opens no second sidebar or automatic scroll effect. Width is
presentation-only; the existing turn navigator and reading-anchor contract apply.

## TUI consumption

The TUI renders and lists only committed facts. `CorrelatedTool.resultCommitted` is
true only when a card's settled lifecycle comes from the committed canonical
Tool-result message. The card shell passes `deliveries` to renderers only for such
a successful result, and passes an empty list for a successful foreground
settlement that has not committed yet. The pure `tool-present` renderer draws the
declared paths from the arguments as the call, and `Delivered N files` plus each
committed file, in canonical order, as the result. Malformed shapes fall back to the
generic card. Status remains the card shell's runtime lifecycle.

`/files` pages the focused Session's committed history through `session/transcript`
(one bounded native page per explicit request, newest result first). It keeps no
index or cache. Each entry shows the filename, type, description, original relative
path, original Conversation scope, `message#i/n` address and action availability.
Reconnect, cold resume and forks read the same canonical records.

Actions live in the TUI-owned `app-server/delivery-files.ts`. Renderers and command
definitions never touch the filesystem.

- **Save** calls `delivery/read` and checks the base64 bound before decoding. It
  then publishes one complete file at the typed client-local destination, or
  leaves the destination unchanged. Linux and macOS run the same code, made only
  of POSIX calls (`open` with `O_CREAT|O_EXCL`, `fsync`, `link`, `lstat`,
  `unlink`, `fstat`):

  ```text
  open(<parent as spelled>/.rustx-save-<128-bit hex>, O_WRONLY|O_CREAT|O_EXCL, 0600)   held: F
    -> 64 KiB writes through F (abort? between chunks) -> fsync F
    -> lstat(destination) absent?      otherwise refused (exists / uninspectable), no link
    -> abort?                          publication admission: the last cancellation point
    -> link(staged name, destination)  publication commit: atomic, never replaces an entry
    -> lstat(destination) is F's device/inode?   published | uncertain
    -> unlink(staged name), once; fresh observations decide residue; close F
  ```

  This is the pattern git uses to publish loose objects (a temporary file in the
  target directory, written, linked to its final name, then unlinked). It is the
  strongest no-clobber publication that Linux and macOS both provide through
  Node's filesystem API.

  **Trust model.** The destination's parent directory, including its effective
  access policy (mode bits, ACLs, sharing configuration), is chosen by the
  user and trusted. Save controls only the permissions it requests, and grants
  nothing further:

  - A process with no effective access to the parent or to the staged file is
    kept out by the kernel: it cannot reach the staged name, the destination
    or the staged bytes.
  - A principal that the parent's inherited ACL grants read or write on new
    files (see Permissions) can read, or change, the staged and saved bytes.
    Save neither prevents this nor detects changed content. Write access to a
    file's content does not let anyone rename or remove its directory entry.
  - A process that can change directory entries in the parent can create,
    replace, rename or delete entries there. That includes this user's own
    processes; other users when the parent is group- or world-writable without
    the sticky bit (in a sticky directory such as `/tmp`, other users cannot
    rename or remove this user's entries); and, on macOS, anyone holding the
    parent's `delete_child` ACL right or an inherited `delete` right on the
    file itself (`delete` removes a name but cannot add one).
  - Privileged processes bypass permissions.

  The last three groups are trusted not to interfere. Against those that can
  change directory entries Save guarantees honesty, not prevention. Two of
  Save's steps take names, because neither platform offers a portable call
  that links a file by descriptor (`linkat(AT_EMPTY_PATH)` and `O_TMPFILE` are
  Linux-only, and Node exposes neither) or that removes a name only while it
  names a given file. Save never claims more than its own handle shows, and a
  matching UID is never treated as proof that an object belongs to this save.

  **Permissions.** Mode bits and effective access are separate guarantees.

  - *Mode bits.* F is created with mode `0600`, set by the creating `open`
    itself, so there is no window before a later `chmod`. A umask can only
    remove bits from a creation mode, never add them, so under any umask
    (`0000`, `0002`, `0022`, …) F's group and other bits stay clear. The
    published destination is the same inode, so it has the same mode. Nothing
    changes a mode after publication.
  - *Effective access* is decided by the filesystem's whole authorization
    model, not by those bits alone. On macOS (APFS, HFS+) a new file inherits
    its directory's `file_inherit` ACL entries at creation. The kernel checks
    an ACL before the mode bits and falls back to the bits only for rights the
    ACL left undecided. So an inherited allow entry can give another principal
    read or write on a `0600` file, and an inherited deny entry can withhold a
    right the bits grant. On Linux a directory's POSIX default ACL is masked
    by the creation mode, so `0600` leaves its named users and groups no
    effective rights (`mask::---`). ACLs on other models, such as NFSv4 on a
    network share, follow the server's rules. Privileged processes keep their
    access everywhere.

  Save does not strip, rewrite or add ACL entries. A directory's inherited
  policy is how a user shares a folder, and the user chose this destination.
  Node also has no call that creates a file with an explicit ACL, or without
  inheritance, so overriding the policy would mean rewriting it after
  creation, with a window between the two. For other users the default is
  private: `chmod` the file to share it. Authorization for delivery bytes on
  the App Server does not depend on the local file's permissions.

  **Ownership** is the handle F returned by the exclusive create. `O_EXCL` fails
  on any existing entry, a symlink or a directory included, so F is a file this
  save created, and every byte is written through F, never by name. A name, an
  owner or a file type is never taken as evidence that an entry is this save's.

  **Publication.** Just before admission, Save looks at the destination. If an
  entry is there (file, directory, symlink, dangling symlink), the save is
  refused as "already exists"; if it cannot be inspected (for example
  `ENAMETOOLONG`, `ENOTDIR`, `EACCES`), the save is refused as "cannot be
  inspected", since publication could never be shown there. Either way no link
  is dispatched, so that refusal is definite. `link` then creates the destination name atomically and fails with
  `EEXIST` for any existing entry, so an existing file, a concurrent save or an
  external writer is never overwritten. F is complete and synced before the link, so a partial file is
  never visible at the destination. The link reads the staged *name*, so the
  published file is bound to F by evidence after the call, not by the call
  itself: the save is reported **published** only when the destination,
  observed by `lstat` after the commit, names F's device and inode. A
  destination naming anything else is reported as uncertain, never as saved. A
  filesystem that cannot hard-link (`EPERM`, `ENOTSUP`, `EOPNOTSUPP`, `ENOSYS`;
  for example FAT, exFAT and some network shares) fails the link; the outcome is
  uncertain like any failed link (see below), and its message says that the
  filesystem may not support hard links. There is no rename or copy fallback and
  no retry.

  **Cleanup** is one `unlink` of the staged name, whatever the outcome, separate
  from publication. Its result rests on observations taken after that unlink,
  never on the earlier one that proved publication, because the entries may have
  changed since. Each conclusion rests on a single observation, which is atomic
  on its own. Observations made at different instants are never combined into
  one snapshot.

  - **removed**: F's own link count (`fstat`) is 0, so F has no name anywhere.
    Or the destination, observed by `lstat`, names F and that same `lstat`
    reports a link count of 1, so the destination is F's only link.
  - **remains**: F's count is 2 or more. One name holds one link, so a link
    besides any destination exists. Or the staged name still names F. Or the
    destination names F with a count of 2 or more.
  - **unknown**: none of these. Examples: F has one link but neither the staged
    name nor the destination names it, the destination cannot be inspected, or
    the counts cannot be read. Without an observation that attributes F's
    remaining link, Save neither calls it removed nor guesses where it is.

  So a staged name moved away after publication, with the destination since
  removed, leaves F linked once at a name no observation finds: the result is
  `unknown`, never `removed`.

  A file that remains or is unknown is reported as residue (`staged:
  "remains" | "unknown"`) under the name it was created with, and it counts as
  a local effect still owed to the user after `/files` retires. An absent
  staged name is not proof that the file is gone. `unlink` never removes a directory, and
  nothing ever removes the destination. Within the trust model the staged name
  only ever names F. A trusted actor who can change the parent's entries and
  substitutes that name just before the unlink has their entry removed in F's
  place. That is a limit of pathname deletion, which Save cannot prevent. Save
  reports residue whenever F is still linked other than at the destination.

  Cancellation at or before admission publishes nothing. The admission check and
  the dispatch of `link` run in one synchronous step; once dispatched, the link's
  own result decides, and a later cancellation neither removes the file nor
  reports it as unsaved.

  **Supported model.** The destination may be on a local filesystem or on a
  network filesystem (NFSv3, NFSv4.0, SMB) that may perform a request and then
  answer a retransmission of it. Anyone the parent's policy allows may change
  the parent's entries at any time. The answer to a link that was dispatched
  therefore describes only the last execution of the request, not the first.

  The outcome is decided by evidence, in this order:

  1. **Published** if the destination, observed after the link, names F (same
     device/inode), whatever `link` answered. A network filesystem can fail a
     retransmitted link that it performed. A successful link of the staged name
     does not show which file that name held.
  2. **Refused** only when no link was dispatched: the destination already held
     an entry, or could not be inspected, before admission; or the save failed
     or was cancelled before admission. These are definite.
  3. **Uncertain** for every other dispatched link. No error code proves that
     the first execution created nothing. A retransmission answers `EEXIST`
     because the first execution created the name, `ENOENT` because someone
     then moved the staged name or renamed the parent, `EACCES` or `EPERM`
     because permissions then changed. An absent destination, or one naming
     another file, does not prove refusal either: the entry may have been
     created and then removed or replaced by someone else. On a local
     filesystem a failed link created nothing, but Save cannot tell a local
     answer from a retransmitted one, and says so. A successful link after
     which the destination does not name F is uncertain too.
     `DeliveryUncertainError` records whether `link` itself succeeded
     (`linked`), carries a link error as its cause, and records what the
     destination showed (absent, foreign, or uninspectable with its error).
     Nothing retries the link or touches the destination; the user inspects it.

  Since every staging step happens in the destination's parent, the ordinary
  local refusals (no write permission, read-only or full filesystem, missing
  parent) already fail the exclusive create or a write, before any link, and so
  remain definite. The uncertain class is what is left once a link has been sent.

  The outcomes are distinct: saved; saved with a residue warning; not saved; not
  saved with residue; outcome unknown, with or without residue. Each residue is
  either a staged file that was not removed or one whose removal could not be
  established. A cleanup result never replaces the publication outcome, and an
  ambiguous one is reported, never turned into success. "Saved" states
  that this save's link created the destination entry and that, right after,
  the entry named the complete file this save created and wrote. It makes no
  claim about later: anyone who may write the parent can rename or replace that
  entry afterwards. F's bytes are synced before the link. The parent directory
  is not synced, so whether the new name itself survives a system crash is up
  to the filesystem; the atomicity of publication does not depend on it.
- **Staging path.** The staged file is created beside the destination, in
  `dirname(destination)`, the destination's own spelling of its parent
  (`dirname` only strips the last component). The OS therefore resolves it
  exactly as `link` resolves the destination, on the same filesystem. No Save
  path is built with `path.join`/`resolve`: in `link/../report.md` with `link` a
  symlink, the OS resolves `link` before `..`, so lexical folding would stage in
  a different directory than the one the destination is created in. Both names
  are resolved again at each step. If the parent is renamed or replaced between
  staging and the commit, the link fails (`ENOENT`) and nothing is published,
  but that `ENOENT` cannot prove it, so the outcome is uncertain. The staged
  file, still in the renamed directory, is reported as residue under the
  spelling it was created with. The random part of the staged name
  only avoids collisions; `O_EXCL` is what establishes ownership.
- **Destination spelling.** The typed path is used exactly as typed. Leading,
  trailing and inner spaces are part of the name, and whitespace only decides
  whether the input is blank. `~`/`~/` expand to the user's home, and a relative
  path is prefixed with the TUI's cwd without folding `.` or `..`, which the OS
  resolves (lexical folding names a different file when a component is a
  symlink).
- **Destination.** The delivered name is the server's identity for the file and is
  never altered. It prefills the editable destination only when it renders as
  itself; a name carrying a terminal control, C1 or bidi character leaves the field
  empty and asks for an explicit path. The destination Input holds only renderable
  text: a paste that would add such a character is refused whole.
- **Open** is best effort, by contract, in one trusted local environment: this
  TUI spawned the App Server (ownership `owned_child`, established at
  construction), so the server's paths are this user's paths on this machine, and
  the connection holds delivery access. Anywhere else Open is refused and Save is
  offered. `delivery/locate` returns the verified leaf, and the TUI requires its
  own `lstat` of that path to report the same regular-file device/inode (not a
  symlink, directory or other file) before it launches the platform opener
  (`xdg-open`/`open`, argv only, no shell). That check is an availability check at
  one instant, not a security guarantee. The opener and the application it starts
  resolve the pathname again, later, by themselves, and no portable opener accepts
  a descriptor, so a rename or replacement in that window can make them open
  another file, as in any file manager. The commit is the spawn: cancellation
  before it launches nothing, and after it the launch is not reported as undone.
  Exit 0 is reported as the opener accepting the request, never as an application
  opening, and never as proof of which file it showed. A non-zero exit or a
  failed spawn is reported as the opener's rejection. Save is the
  identity-preserving action.
- **Remote** (`--connect`) never interprets a server path locally. With
  `--delivery-access-token-file`, Save writes bytes on the client machine. Without
  it, `/files` shows metadata and reports both actions as unavailable.

Each `/files` overlay owns one abort scope. Every way the interaction ends goes
through the app's overlay close: Escape on the list, another overlay replacing it,
Session focus change, attachment or snapshot replacement, disconnect, terminal
failure and quit. Closing aborts the scope, which cancels the native request
(`delivery/cancel`) and every uncommitted local effect. Escape during an action
aborts just that action. Each outcome belongs to one operation token, so a
selector never shows a stale or duplicate outcome. Each action records its own
external effect as it happens (`LocalEffect`): `committed` when Save dispatches its
link or Open spawns its opener, and `residue` when Save leaves its staged file
behind or cannot show it removed.
Retirement can stop an action that has not committed but cannot unsay one that
has. After retirement, an action with a committed effect or residue reports its
terminal outcome once on the transient surface: saved, not saved, unknown, opener
accepted, or opener rejected. It never writes into the retired selector or a
successor surface, and a committed effect is never described as "nothing was saved
or opened". An action that did neither is not reported. Outcome text is sanitized
before it is drawn. Missing,
unauthorized, unavailable, replaced, oversized (>512 KiB), capacity and disconnected
failures are reported explicitly. No action starts an Agent, Tool or model request.

## Web presentation

The Web adapts DeepSeek Harness `ui-deliverables` presentation, not its runtime.
`bindings/present.ts` maps the native foreground lifecycle to Harness phases:
assembled→preparing, running, success→ok, cancelled→stopped, and other terminals→error.
It also turns the committed message's typed `deliveries` into card views. The
`PresentRow` call row shows the phase and the declared paths, and expands to the
recorded result text. It never renders cards. `PresentedFileCard` cards appear only
for successful committed Tool messages: a whole-card Preview gesture opens the
existing PreviewWorkspace, and a separate Download action uses the existing original-byte
owner. They show a filename/description hierarchy and file-type glyph in a
one-row or two-column layout, collapsed to four cards behind an `All N files`
toggle. Harness Host open/reveal phases, desktop metadata, Cordis and DSH Session
events are excluded. The browser keeps using the Product Host lane; delivery
access is never given to the browser.

The closed PDF, OOXML and HTML viewer families extend this ownership contract;
see [advanced document previews](document-previews.md) for their admission,
conversion, isolation, resource and platform limits.
