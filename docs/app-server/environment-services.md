# App Server environment and management services (v79)

Issue #533 extends the shared-control foundation in #532 / PR #540. The branch
starts at dependency-complete `origin/main`,
`57447d2e825f914e80bbfecc68d373fcab487446`. The fixed Codex reference remains
`rust-v0.166.0` / `6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`.

The public contract is the Rust-derived v79 schema and TypeScript in
`protocol/app-server`. v74 artifacts and transport negotiation are replaced.
Existing Web and TUI presentation uses the new generated imports and version;
these services add no controls, layouts or alternative client package.

## Admission and native authority

Capability Token, digest-backed Capability Token, Signed Bearer Token and trusted
stdio retain #532's admission rules. Every authenticated, initialized ordinary
client has the same API eligibility. JWT role/scope claims confer no extra or
reduced API access. Upgrade-time expiry does not revoke an established socket.

The native User/Host process policy grants environment roots:

```toml
[app_server]
environment_roots = ["/absolute/workspace"]
```

An empty list grants no environment access. Workspace-authored configuration
cannot grant these roots. Public source authoring uses the existing User source
validation, revision/CAS and automatic application owner. Grant removal fences
new work and cancels commands and watches admitted under the removed root. A
replacement directory at the same path does not inherit the old inode grant;
remove the grant and explicitly grant the replacement. A configured but
unopenable root is unavailable. Root grants do not imply an OS sandbox: the
existing Unix restrictions and the host user's OS permissions remain effective.
The trusted host still owns user-to-process isolation and deployment policy.

Environment paths, including command cwd, must be absolute UTF-8 paths under a
configured root. Parent components are rejected. Directory resolution uses
retained descriptors, no-follow opens and ancestor-edge identity checks. Generic
file reads/writes/copies refuse symlink traversal; metadata and listings describe
links without following them, and removal unlinks the link itself. Root removal
is refused. Both sides of a cross-root copy require their own current grant.
Requests cannot grant roots by supplying cwd, a Thread or a preview filename.

These are native resource constraints, not client roles or additional tokens.
Private Product Host plumbing remains separately owned; it does not gate any of
these public methods. Preview, committed deliveries, uploads and original
Downloads retain their existing native source identities and publication owners.

## Commands and PTYs

| Method/event | Contract |
| --- | --- |
| `command/exec` | Direct argv execution; no shell interpretation, model, Thread, Attempt or canonical ToolCall. The final response follows native process-tree settlement and all emitted output. |
| `command/exec/write` | Base64 stdin bytes, EOF, or both for this connection's process. A receipt reports that write/EOF operation, not command completion. Reads continue while a bounded input batch is backpressured; an incomplete write with possible partial effects returns `outcome_unknown`. |
| `command/exec/resize` | Validate and apply a PTY size for this connection's process. |
| `command/exec/terminate` | Request native cancellation. Receipt does not prove termination. |
| `command/exec/outputDelta` | Base64 stdout/stderr chunks with an exact per-process observation sequence. PTY business output is merged into stdout. |

A request example, using the actual mandatory fields:

```json
{"jsonrpc":"2.0","id":"exec-1","method":"command/exec","params":{"command":["/usr/bin/printf","%s","literal"],"cwd":"/absolute/workspace","env":{},"processId":"program-1","tty":false,"streamStdin":true,"streamStdoutStderr":true,"outputBytesCap":65536,"timeoutMs":60000,"size":null}}
```

`tty` implies streaming stdin and output. A caller-supplied `processId` is required
for streaming, PTY and follow-up control. It is unique among active commands on
that connection; another connection may use exactly the same string. Reconnect
cannot recover, transfer or restart that process. The connection owner cancels
its independent commands on disconnect and shutdown; accepted Agent work retains
its existing server-owned lifetime. Host drain cancels connection environment
resources before waiting for their counted request owners, so a long command's
execution deadline cannot hold graceful shutdown open. Cancellation observed
during directory preflight refuses a new process launch.

The actual native START frame is the authorization boundary for business work.
Root revocation and the Host/connection/command cancellation family arbitrate
with that write through their existing short native commit gates. Readiness is
selected alongside cancellation, deadlines and supervisor events; only the
nonblocking five-byte write holds the grants. No Host admission, root registry
or environment-I/O lock covers that write. Revocation that wins the grant
prevents business spawn even if a gated supervisor already exists. START that
wins retains the ordinary cancellation and physical-settlement contract. A
partial frame closes the control write half and reports process-control failure;
no termination frame is appended to its prefix. The read half, retained anchor
and child owner remain alive until physical settlement is proven.

The native Bash supervisor's direct-argv path shares its startup gate, process
membership, timeout/cancellation arbitration, containment and reap proof. Command
stdin and PTY descriptors are separate from supervisor control. Cwd identity is
verified inside the supervisor before the business process starts. PTYs use the
existing Unix ownership model and safe nix/rustix terminal operations. PTY EOF
reads the current canonical terminal settings and sends three configured EOF
characters, allowing for a pending literal-next quote and a pending input line.
Quoted control bytes retain their normal terminal meaning. Raw mode, a disabled
EOF character, or an EOF setting conflicting with another active control or
input translation rejects `closeStdin` before writing that batch; a terminal has
no portable stdin half-close in those modes.

PTY master and slave descriptors are allocated atomically close-on-exec through
the existing rustix PTY operations. Neither the command nor a concurrently
spawned command inherits another terminal's raw endpoints; only its explicitly
assigned stdin/stdout/stderr cross the native execution boundary.
The lifecycle supervisor catches terminal foreground-group signals (including
INT, QUIT and HUP) without inheriting ignored or blocked dispositions into the
business program. Exec restores its default dispositions, so applications can
handle Ctrl-C/Ctrl-\\ or terminate normally. Physical containment without a
business exit status produces a process-control failure promptly; it does not
wait for the command deadline or invent a successful exit.

Only `ToolEnvironment`'s four native baseline variables and explicit validated
entries reach the business program. Parent process/provider/App Server credentials
are not inherited. Native runtime-owned environment keys cannot be overridden.
Explicit null environment entries add nothing; there is no parent environment to
unset. Provider credentials and App Server admission credentials remain distinct.

Timeout defaults to 60 seconds and is bounded at one hour. Output capture defaults
to 64 KiB per stream and is bounded at 256 KiB per stream. Unbounded timeout/output
options are intentionally absent. Invalid combinations and unknown fields fail
before spawn. Binary output is preserved as base64. Buffered responses contain
base64 captures and separate truncation flags. Streamed captures are empty in the
final response; a final `capReached` delta explicitly reports truncation. Ordering
within each stream is preserved; `sequence` orders the owner's observations of
the separate pipes and does not invent an OS-wide stdout/stderr clock.

The final result distinguishes completed, timed out, terminated and process-control
failure. Natural nonzero exit is still completed with its real exit code. Signals
may have no numeric exit code. Native inability to prove physical cleanup is never
converted into successful settlement. Capacity remains with the physical owner,
including after loss of the RPC waiter. A connection admits at most four independent
commands; control retains its reserved lane. Streaming output uses bounded 8 KiB
batches: first bytes are published promptly, short reads accumulate while a
notification awaits acceptance, and acceptance progress flushes short tails.
EOF and the retention cap force pending tails out; stdin, cancellation and
settlement remain active in the same actor select. Per-process observation
sequences describe emitted batches. Output queues are bounded; a saturated
consumer is retired, and its processes are cancelled and drained by the same
native owner. The I/O actor cooperates between completed turns, including after
the capture cap, so a continuously ready PTY cannot fill the queue entirely
within one task poll while excluding transport/control scheduling. The final output receipt is fenced at actual transport acceptance,
not merely at enqueue. Commands are not durable jobs and are never replayed.

## Filesystem operations

| Method | Contract |
| --- | --- |
| `fs/readFile` | Revision-checked base64 chunk with offset and continuation. |
| `fs/writeFile` | Single-file atomic replace/create with an expected revision. |
| `fs/readDirectory` | Bounded, sorted direct-entry page and revision. |
| `fs/getMetadata` | Native type, size, identity, revision and optional modification time. |
| `fs/createDirectory` | One directory or bounded path-wise parent creation. |
| `fs/remove` | File/link unlink or bounded recursive removal; missing handling is explicit. |
| `fs/copy` | Authorized source/destination; atomic individual files, bounded recursive trees. |
| `fs/watch`, `fs/unwatch`, `fs/changed` | Connection-local, non-recursive change hints with resync/retirement facts. |
| `conversation/changes`, `conversation/changeDiff` | Existing native captured file changes and Diff for the exact attached Conversation. |

```json
{"jsonrpc":"2.0","id":"read-1","method":"fs/readFile","params":{"path":"/absolute/workspace/file","offset":"0","maxBytes":65536,"expectedRevision":null}}
{"jsonrpc":"2.0","id":"write-1","method":"fs/writeFile","params":{"path":"/absolute/workspace/file","dataBase64":"aGk=","expectedRevision":"revision-from-native-read"}}
```

Unbounded native u64 values, including file sizes/offsets and output sequence,
follow the existing exact decimal-string wire codec. Page indices and bounded
counts remain safe JSON integers. Use generated types and wire fixtures.

Read/write frames carry at most 512 KiB of decoded file data. Reads can continue
through a large file with the same expected revision. Existing native upload and
chunked delivery contracts remain available for their existing source domains;
this method does not synthesize an upload or committed delivery. Listings retain
at most 4,096 direct names and return at most 128 entries. Oversized directories
fail explicitly. Recursive copies/removals visit at most 4,096 entries and depth
32; copies have a 256 MiB byte budget and stream fixed-size buffers. These limits
bound work instead of hydrating an unbounded directory tree.

`expectedRevision: null` creates an absent file; it cannot overwrite an existing
file. Replacements require the current native revision. Revision evidence includes
file identity, length, mode, mtime and ctime with native precision. Native API writes
serialize through the environment I/O owner. Root grants are retained during
physical work; short native cancellation/commit gates order each publication
against revocation without holding the process-policy lock across file I/O. A temporary sibling receives complete
bytes, existing mode and file sync before revision/ancestor validation and atomic
publication. Create uses a non-replacing link commit; replace uses rename. Directory
sync follows publication. Pre-commit failure removes the temporary and preserves
the target. A failure after publication reports unknown outcome and requires a
read. External OS writers are not covered by a cross-process transaction: a final
check and rename are not a filesystem-wide compare-and-swap instruction.

Directory/tree operations can be partial on failure. Re-read source and destination
before deciding the next action; never automatically replay an uncertain mutation.
Commands, Agent tools and file RPCs can share a Workspace under their own native
owners, without a whole-Workspace lock or transaction guarantee.

Native registration retains the descriptor for the exact authorized source;
replacing and restoring its pathname cannot redirect the observer. Linux uses
that descriptor through `/proc/self/fd` and translates only its exact event prefix
back to authorized paths. macOS registers vnode descriptors directly, including
immediate files and child-directory metadata without observing grandchildren.
Directory membership is bounded to 4,096 names and the instance shares 1,024
child-descriptor slots. Initial exhaustion refuses registration; subsequent
reconciliation failure retires the watch with a resync hint. Coalesced vnode hints
always require rereads. Native teardown retains these descriptors and admission
through actual observer exit; macOS cancellation wakes its queue without polling.
If concurrent environment I/O prevents a callback's identity check, the same
watch owner schedules a single coalesced deferred check. Native callbacks never
wait for the I/O gate. The owner preserves a valid directory identity or retires
a replaced/unavailable source even if the old inode emits no more events. At
most one blocking check and one pending recheck wake-up exist per watch; physical
capacity remains held through that check, including after disconnect.
Unwatch also cancels any pending retirement-notification receipt wait; a live
client need not consume a withdrawn notice to return physical observer capacity.

Watch IDs belong to one connection and cannot address another connection's watch.
There are at most eight per connection and 32 shared native file-observation
slots across the host. The native callback/retirement lifetime retains its slot. Unwatch, disconnect, root retirement and
shutdown retire the native watcher. Graceful process shutdown waits for native
callback destruction and returned observation capacity after transport teardown;
dropping a notify handle alone is not a physical-settlement proof. Delayed retirement is fenced by exact watch
incarnation, and queued old hints are discarded after unwatch. Automatic
retirement closes a per-incarnation publication gate
before dropping the native watcher and queuing its terminal notice. Ordinary
callback admission and queue publication share that short gate, so the terminal
notice is the incarnation's last visible event, including callbacks that passed
identity validation earlier. Closing ordinary publication does not withdraw the
terminal transport fence; unwatch/disconnect can still withdraw that notice.
Unwatch closes the old gate and fence under the registry's identity-removal
boundary before another watch can reuse the ID. Metadata inspection, native
Drop and receipt waits stay outside the publication gate. Events contain at
most 16 paths. Native overflow, unavailable coherent inspection or replacement
requires authoritative reads. Events are not a canonical file journal; reconnect
always requires a reread and fresh watch registration, never mutation replay.

## Native management matrix

| Area | Public API and native authority |
| --- | --- |
| Provider/Model/Profile sources, capabilities and availability | `configuration/sourcesRead`, `configuration/sourcePage`, `configuration/sourceValueRead`; native redacted source resolution and model catalogs. Source/effective/admitted views remain distinct. |
| Session model selection | `session/model`, `session/models`, `session/setModel`, `session/selectModel`; native catalog validation and frozen Attempt admission. |
| Usage | `session/statistics`, `agent/statistics`; actual native usage only. Upstream billing/quota/rate limits without native facts remain unavailable. |
| Configuration authoring and application | `configuration/sourceWrite`, `configuration/reconcile`, `session/configuration`, chunked application/source/effective reads; existing CAS, provenance, authority-before-precedence and automatic safe-boundary application. |
| Skills effective catalog, metadata and content | `skills/list`, `skills/read`; the existing bounded Skill discovery/source owner with revision-checked instruction chunks. Selection remains `configuration/sourceWrite` with native Skills/Agent mutations. |
| MCP configuration and secret inputs | `configuration/sourceWrite` with native MCP mutation, redacted source reads and retained secret references. |
| MCP connection/preparation and reload | `resources/read`, effective configuration and application projections, `configuration/reconcile`, `mcp/probe`; existing Capability Coordinator, probe, transport, credentials and physical cleanup. |
| MCP tool/resource/template inventory | `mcp/catalog` gathers a complete finite inventory at the source revision on one native probe connection, then physically closes it; no remote continuation token escapes that owner. |
| MCP business calls and resource content | Existing authorized native Tool Runtime, including MCP Resource tools; management provides no unrestricted tools/call or content shortcut. |

Skills reads resolve effective names through native source authority, rather than
accepting a client-authored file path. `skills/list` returns compact descriptors
with native catalog/provenance and `metadata_revision`; pages stop at 32 entries
or 128 KiB, with at most 64 KiB / 32 diagnostics and explicit truncation.
`skills/read` selects `kind: "instructions"` for raw SKILL.md bytes or
`kind: "metadata"` for native parsed JSON metadata. Each read retains at most
64 KiB, with exact decimal-text `offset` and `next_offset`. Metadata uses its
independent definition revision; instruction revision is the same descriptor
identity/size/timestamp evidence used by native file reads. Large Skill bodies
remain valid and are never loaded in full. Continue with `expected_revision`;
source replacement/mutation requires an authoritative reread. Native blocking
jobs retain host admission through completion after a waiter disconnects.

MCP management follows up to 32 bounded remote continuations on the same native
connection, including an empty opaque token. Cycles, more than 128 total
descriptors, or an inventory above 64 KiB fail as `list_failed`; partial catalogs
are never labeled complete. Tool, resource and template descriptions are
credential-redacted, and business invocation is unavailable through this API.
The native connection owner includes every accepted literal environment/header
value and every resolved sensitive reference in one private redaction set.
Authorization and Proxy-Authorization Bearer inputs contribute both the complete
header value and the token component, for literal and referenced headers alike.
Redaction applies to
typed peer-owned catalog fields and arbitrary schema JSON keys and strings.
Native envelope keys, catalog kinds, capabilities, error codes, connection
generations and counts retain their original values. The final redacted catalog
must still fit the same 64 KiB bound.

Peer error messages and JSON data are redacted before escaping, formatting or
truncation. Opaque transport diagnostics report their native category, transport
type and operation phase without replaying potentially escaped peer text.
Diagnostics retain the existing 1 KiB bound. A stderr preview is eligible only
after complete, unclipped capture is already proven; incomplete capture is
omitted without waiting for optional pipe EOF after physical settlement.
Neither private redaction values nor resolved credentials enter serialized or
debug configuration views.
Catalog kind is `tools`, `resources` or `templates`; `expected_revision` binds
the authored MCP source before and after native discovery.
Probe/catalog native tasks retain counted host admission through physical close
even after their RPC waiter disconnects. Host drain cancels their native resource
I/O signal and waits for the same cleanup owner before claiming settlement.
Each instance admits at most four native management probes/catalogs. Unproven
physical cleanup retains its native owner, capacity slot and bounded failure
evidence; later RPC completion or response loss cannot make host drain report a
clean settlement.

Catalog pages and content chunks retain source provenance. Configuration writes and their execution application remain
separate facts. Existing admitted Attempts retain their frozen resources/models.
Credential fields remain write-only or redacted. MCP authentication continues to
use the already-supported native credential/environment/header flows; a management
catalog is not an OAuth issuer, login product or plugin marketplace.

The single external-client example remains `examples/app-server/connect.mjs`:
initialize and read `server/info`. Automated protocol/process/browser tests, rather
than extra demo applications, prove the other contracts. See the consolidated
[delivery and validation evidence](../issue-533/README.md).
