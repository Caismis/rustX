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

App Server v34 has no public `session/fileRead` Method. An ordinary authenticated
App Server connection cannot enter the file-read seam, even with the exact
attachment, canonical Tool message ID, delivery index, Session cwd, and reference.
Initialize client names are metadata and have no authorization role.

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

Native fences run at authenticated admission, before canonical lookup/allocation,
before open, after leaf open immediately before bytes, after byte/edge verification,
and before response serialization/delivery. They check captured host authority,
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
| Native Session-file reads | 2 owned reads per AppServerHost; excess fails, no unbounded queue |
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

The closed PDF, OOXML and HTML viewer families extend this ownership contract;
see [advanced document previews](document-previews.md) for their admission,
conversion, isolation, resource and platform limits.
