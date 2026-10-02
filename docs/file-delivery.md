# Explicit delivery and safe Session-file previews

`present` is an ordinary native Tool registered through `ToolDefinition`,
`ToolExecutor`, and `NativeToolRegistration`. The normal Tool Plane and Agent Loop
own invocation, cancellation, settlement, and atomic canonical Tool-result commit.
There is no delivery database, special Agent Loop branch, or browser model state.

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

App Server v31 `session/fileRead` addresses an exact existing attachment target,
canonical Tool message ID, delivery index, and authenticated native caller's
allowed-root restriction. The browser's authenticated Product Host carrier accepts
only target/message/index coordinates and Product Host authority scope. It accepts
no path, root list, or browser-provided SessionFileReference. The Product Host binds
the request to its configured rustX endpoint and current roots on every call.

The App Server's bounded weak attachment lookup permits that authenticated Product
Host to use the browser's existing exact attachment. It neither steals a controller
nor creates a second attachment/runtime. Native authorization is checked at
admission, before open/read, after read, and before publishing the response. The
canonical message must be a successful Tool result in the viewed Conversation
store. Native Session allocation ownership excludes source deletion during read.
The original Conversation resolves through SessionController's current filesystem
mapping and its cwd must exactly equal one of the Product Host's configured
allowed roots, matching the existing exact-root classification policy. The original node's
cwd mapping is rechecked before opening, before reading, and after reading;
reassociation or mapping loss fails the in-flight read. All file bytes come
from the addressed App Server's filesystem; no Host-path fallback exists. A remote
or container filesystem needs its actual native mapping and authorized Product
Host configuration; a same-spelled local path is never a fallback byte source.

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
| Product Host file operations | 2 per Host instance; temporary connections close in `finally` |
| Browser Session-file transfers / retained URLs | 2 / 2 per selected-view owner |
| Managed Artifact transfers / URLs / bytes | Existing 2 / 16 / 262,144 bytes |
| Rendered Session-file text / Markdown input | 512 KiB original bytes, fatal UTF-8 decode |
| Raster dimensions | At most 4096 on either axis and 4,194,304 pixels |
| Raster animation | Static only; animation fails inline but Download remains |
| Declaration / allowed-root list | 8 entries / 32 native roots |

Each browser Session-file response contains at most 699,052 base64 characters.
Its transient binary string, decoded text string, byte array, and Blob each
contain at most 524,288 characters or bytes as applicable; at most two reads/URLs
are owned. These values are bounded before decoding or URL allocation. A raster's
logical RGBA output is at most 16,777,216 bytes, with dimensions checked before
browser decoding. Read buffers and decoded content are effect-owned and ephemeral;
only finite Blob URLs are retained by the resource owner.

`FilePreviewResources` has a closed Artifact versus Session-file source union,
separate concrete source owners, and one presentation seat. Rendering and Download
share one authorized byte transfer and one Blob URL. Text/Markdown/code Download
uses original bytes and filename, including CRLF, spaces, and Unicode, never
rendered HTML. UTF-8 or raster decode failure retains bounded original-byte Download.
An unsupported format explicitly reports no inline viewer and remains downloadable
inside the same bound. No partial oversized download is offered.

Markdown uses rustX's existing mdast-to-React renderer: raw HTML stays literal,
URL schemes allow only HTTP/HTTPS/mailto links, embedded images remain inert alt
text, and KaTeX trust is disabled. No independent renderer, local resource loading,
remote embedded resource loading, HTML execution, or SVG inline execution is added.
Raster headers are inspected through image-size's Uint8Array API before browser
image decoding; active formats are rejected. No filesystem library API enters Web.

Each effect owns an AbortController, live publication flag, and its allocated URL.
Source replacement, retry, close/unmount, Session/node change, attachment replacement,
reconnect, native authority revision, and Product Host authority revision retire
old owners. Responses recheck captured attachment and both authority scopes before
base64 decoding or URL allocation. Obsolete successes, errors, loading completions,
and image callbacks cannot publish into the new keyed preview. Cleanup aborts
reads, unsubscribes through existing App ownership, and revokes each URL once.
Same-authority display refresh does not recreate the owner.

The existing #430/#443 RightPanel and ChatViewport remain the geometry and scroll
owners. Delivery opens no second sidebar or automatic scroll effect. Width is
presentation-only; the existing turn navigator and reading-anchor contract apply.
