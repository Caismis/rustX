# Execution Protocol and Binding Arbitration

Status: accepted — layers 1–3 implemented (binding arbitration; local
bootstrap with authenticated handshake; and the first physical
operations: bounded revision-checked range reading, entry metadata,
and bounded directory pages of the bound workspace). Remaining
contracts below are frozen in shape and pending implementation in
later layers.

This ADR grounds its decisions in the verified server/client separation
plan `rustx-server-client-development-plan-v1.3-2026-11-16.md`
(SHA-256 `563462bf8da10b626491316b57143e9baeae95d1c5f899d2326e38a4dd90c9bc`)
and in these issues:

- First implementation issue:
  <https://github.com/Caismis/rustX/issues/576>
- Live-instance recovery and re-admission proof:
  <https://github.com/Caismis/rustX/issues/577>
- Follow-on execution boundary work:
  <https://github.com/Caismis/rustX/issues/578>
- Follow-on integration work:
  <https://github.com/Caismis/rustX/issues/579>
- Earlier Server/Client groundwork
  (<https://github.com/Caismis/rustX/issues/571>) is still open at this
  base; nothing here depends on it.

Fixed reference commits from the plan (frozen snapshots, not
"current HEAD" claims; only the plan hash above was re-verified): rustX
`d82d7f9b8d9dd4f5064dba8d377103c8c6049d28`, Codex
`ee344ca9458351a6ea102795fa73110ad3ede5dc`, Pi
`22fc026b872b4a7df22ffa94e8fa00b873128054`. The actual implementation
base of this branch is `f0cd4f59d`.

## Decision

### Crate and naming boundary

The workspace has four execution members; the names are fixed.

- `rustx-execution-protocol` (`crates/execution-protocol`): the single
  wire definition — identity newtypes, `Hello`, `Admission`, `Reject`,
  `HelloReply`, `ClientRequest`/`ClientReply`, `ReadError`, `DecimalU64`,
  `Bootstrap`, the `Capability` vocabulary, the `transport` record
  framing, and the frozen resource bounds. Both sides serialize these
  exact DTOs; there is no duplicated contract type.
- `rustx-execution-server` (`crates/execution-server`): a library for
  binding arbitration and local acceptance owned by the Agent Server.
  It is not a new Agent, not a binary, and not a transport deployment.
  It deliberately does not depend on the confined-filesystem crate: the
  Server arbitrates and dispatches, it never touches the workspace.
- `rustx-confined-fs` (`crates/confined-fs`): the physical
  descriptor-relative filesystem primitives — the proven
  `PackageDirectory` and the bounded revision-checked range, metadata,
and directory-page readers —
  factored out of the Agent Server itself so both the runtime and the
  Client share exactly one implementation. No Agent, cancellation,
  store, or process dependency.
- `rustx-execution-client` (`crates/execution-client`): the physical
  Execution Client binary `rustx-execution-client`. Its only production
  dependencies are the protocol crate, `rustx-confined-fs`, and `nix`;
  it has no dependency on `rustx` or on the canonical store/model.
- The main Agent Server remains the existing `rustx` package; its
  `package_io` module and environment file readers are thin wrappers
  over `rustx-confined-fs`, preserving every existing semantic.

These layers perform authenticated negotiation, binding arbitration,
local bootstrap, and six physical operations — the bounded
revision-checked range read, entry metadata, bounded directory
pages, the two-phase conditional write, the bounded
logical-text page, and the whitelisted structured-document
page. There is no production
dispatch beyond them,
no process spawn of business work, and no remote
transport; none may exist until the gates below exist. Nothing claims
unimplemented I/O: a runtime Hello advertises `Capability::Connection`,
`Capability::ReadRange`, `Capability::DirectoryMetadata`,
`Capability::ConditionalWrite`, `Capability::ReadLines`, and
`Capability::DocumentPages`, exactly
what the Client implements, never
`Files`.

### Identities

`SessionId`, `EnvironmentId`, `ClientInstanceId`, `ConnectionId`, and
`Generation` are distinct wire newtypes. A `ClientInstanceId` is one
physical Client process lifetime — it is an identity, never a credential.
A `ConnectionId` is one transport connection — it is never the execution
owner, and the Server generates it fresh per accept (16 bytes of OS
randomness), never taking it from the peer or the transport. `Generation`
is the binding generation: same-instance reconnects keep it, instance
replacement increments it. On the wire a generation is exactly one
canonical positive decimal string — no sign, no leading zero, no
padding, no `u64` overflow — and malformed numeric wire forms are
rejected, never coerced. `DecimalU64` — the wire type for byte offsets —
is the same canonical decimal grammar widened to admit `"0"`, and both
share one parser, so there is exactly one notion of a decimal number on
this wire.

### Record framing (implemented)

`rustx_execution_protocol::transport::{read_record, read_optional_record,
write_record}` is the single synchronous framing for `Read`/`Write`
streams: a 4-byte
big-endian length prefix followed by exactly one JSON DTO. The
`MAX_ENCODED_FRAME_BYTES` bound covers the whole frame, prefix included,
so payloads are limited to `MAX_RECORD_BYTES` (1 MiB − 4). Lengths of
zero or over the bound are rejected from the prefix alone, before any
allocation or body read. Serialization is split from emission:
`encode_record` serializes into a bounded sink that fails at the limit
— no oversized payload `Vec` is ever allocated — and returns an
`EncodedRecord` whose `write_to` emits the prefix and bytes;
`write_record` is the composition. Nothing reaches the wire until the
whole record fits, and a caller that encodes first can fail before
touching the stream at all. Parse and length
errors are static messages that never expose payload bytes.
`read_optional_record` additionally distinguishes the two ends of a
stream: an EOF before the first prefix byte is a clean idle close
(`Ok(None)`), while EOF anywhere inside a record — partial prefix or
partial body — is a truncation error, never a silent end.

### Negotiation (implemented)

The Client's first and only binding-opening message is protocol v1:

```rust
pub struct Hello {
    pub version: u16,
    pub session: SessionId,
    pub environment: EnvironmentId,
    pub instance: ClientInstanceId,
    pub generation: Generation,
    pub workspace: PathBuf,
    pub capabilities: BTreeSet<Capability>,
    pub credential: BindingCredential,
}
```

`Binding::reserve(reservation) -> io::Result<(Binding, BindingCredential)>`
fixes the full expected identity — Session, Environment, Client instance,
generation, the exact normalized absolute workspace, and the required
capabilities — and issues the credential from OS randomness. The `Binding`
stores only the SHA-256 verifier of the credential; the credential itself
is returned once, for delivery through the trusted channel. The workspace
check is purely lexical (`is_normalized_absolute_path`): absolute, root
plus normal components only, no `.`/`..`, redundant, or trailing
separators, and the filesystem is never touched.

`Binding::connect(&hello, connection)` checks, under one mutex hold, in
this order: the credential verifier first in constant time — no identity
field may be probed without valid authentication — then `version` equals
`PROTOCOL_VERSION` (incompatible versions are explicitly rejected; no
implicit downgrade), then session, environment, instance, and generation
each equal the reservation, then the workspace by literal `OsStr` bytes
(so `/work//root` and `/work/./root` never alias `/work/root`), then the
advertised capabilities cover the required set. On success exactly one
connection is committed `Live` and an `Admission` is returned. A `Hello`
is never trusted to select required capabilities or the expected
instance — those come only from the reservation.

`Reject` carries machine-stable codes only. No reject, error, log, or
`Debug` output echoes a submitted credential or any secret-bearing field.
`BindingCredential` is exactly 32 random bytes as 64 lowercase hex
characters; it is non-`Debug` (redacted), has no `Display`, and malformed
wire forms — wrong length, uppercase, non-hex — are rejected without
echoing the submitted value.

### Linearization and lifecycle (implemented)

One mutex is the binding arbitration point: Hello validation, connection
commit, disconnect, and revocation all linearize under it. Two barrier-
synchronized Hellos produce exactly one `Live` winner. The phase machine
is `Reserved → Live → Detached`, plus terminal `Revoked` from any phase.

- A failed Hello consumes nothing: `Reserved` stays `Reserved`, `Live`
  stays `Live`.
- While `Live`, a second Hello gets `AlreadyConnected` — at most one
  live connection per Session binding.
- `disconnect(connection)` moves `Live → Detached` only for the current
  live connection. A stale disconnect — the late EOF of a replaced
  connection — is ignored.
- `Detached` rejects every Hello with `RecoveryRequired`. EOF never
  reopens the reservation, so even the fully valid reserved identity
  cannot replay its own credential after EOF — the static bootstrap file
  grants no second admission. Re-admission requires the live-instance
  recovery proof of #577, which this layer deliberately does not fake.
- `revoke()` is immediate and terminal; a revoked binding rejects every
  Hello and is never dispatchable again.
- `status()` is observation only. Nothing may dispatch on a snapshot —
  the phase may change before any use — so there is no `dispatchable()`
  predicate to race against.

### Local bootstrap (implemented)

`Binding::reserve_local(reservation, socket_path, bootstrap_path)
-> io::Result<(Binding, UnixListener)>` reserves the binding and
publishes the trusted-channel bundle:

- Both paths must be normalized absolute direct children of an existing
  owner-only private directory (`0700`, owned by the process euid,
  verified without following symlinks). World-accessible or
  foreign-owned locations are refused, so an endpoint or credential can
  never be planted there.
- The socket is bound fresh: a preexisting filesystem entry is an error,
  never silently unlinked.
- The bootstrap file is written `create_new` — never overwriting — as
  exactly one framed `Bootstrap` record, and `fchmod`'d to `0600` so the
  process umask cannot weaken or strengthen it.
- Failed publication leaves nothing: a bound socket path is removed on
  bootstrap-write failure; a partially written bootstrap file is removed
  by the file that created it — a preexisting `bootstrap_path` is
  refused, not deleted.
- The published `Bootstrap` carries only the reservation's own facts —
  socket path, protocol version, session, environment, instance,
  generation, workspace, and the credential. It never carries
  capabilities: the Client claims exactly what it implements from its
  own build, so a tampered bundle can never make it advertise a feature
  it lacks.

`Binding::accept_local(stream)` performs the handshake on one accepted
`UnixStream` under the shared `transport::Handshake` wrapper:

- One absolute deadline is armed once at accept entry —
  `Instant::now() + LOCAL_HANDSHAKE_TIMEOUT`, the single shared 5 s
  constant owned by `rustx-execution-protocol::transport` — and covers
  the entire handshake: the Hello read and parse, arbitration, the
  admission write, and restoring blocking mode at `finish()`. It
  is an absolute deadline, not an inactivity timeout: `Handshake`
  runs the stream nonblocking for the handshake's duration — every
  I/O is attempted directly and a `WouldBlock` waits in `poll` for
  at most the *remaining* budget, re-checking the deadline after each
  I/O and each wait, so a stalled or trickling peer consumes the same
  fixed budget and never re-arms it per byte or per record. Socket
  timeouts are deliberately not the mechanism: `poll` readiness alone
  never ends a record — `POLLHUP` is answered by a real `read`, so a
  peer that closes immediately after delivering the final frame still
  drains buffered bytes and then reports honest clean EOF, which a
  `setsockopt` on an already fully closed socket cannot express on
  every platform.
- The deadline is re-checked inside the binding mutex immediately
  before the `Reserved → Live` commit: a peer that lets it expire
  during arbitration gets `TimedOut` and the binding stays `Reserved`.
- The connection identity is generated fresh per accept from OS
  randomness.
- The reply is the typed `HelloReply` (`Result<Admission, Reject>`)
  written as one framed record.
- On success the committed connection returns as `LocalConnection`,
  an RAII guard whose drop disconnects the binding; if writing the
  admission or restoring blocking mode fails, the committed connection is
  disconnected and the stream shut down — a binding never stays live on
  a connection the peer does not know it owns, and never keeps a peer
  whose post-handshake blocking mode could not be restored. `finish()`
  restores blocking mode only on success; a failed handshake
  stream is shut down still nonblocking.
- `LocalConnection` exposes only typed, Server-initiated exchanges —
  `ping()` (`Ping`/`Pong`), `read_range(...)` (`ReadRange`/`Range`),
  `entry_metadata(...)` (`GetMetadata`/`Metadata`),
  `directory_page(...)` (`ReadDirectory`/`Directory`),
  `read_lines(...)` (`ReadLines`/`Lines`),
  `read_document(...)` (`ReadDocument`/`DocumentPage`), and
  `write_file(...)` (the `PrepareWrite`/`CommitWrite`/`AbortWrite`
  sequence) — never a raw
  message path. Each first consults the capabilities the peer actually
  advertised in its Hello: a connection that never claimed `ReadRange`,
  `DirectoryMetadata`, `ConditionalWrite`, `ReadLines`, or
  `DocumentPages` gets
  `io::ErrorKind::Unsupported` and
  no request is written. Every exchange serializes the request with
  `encode_record` *before* the gate, so an unencodable request fails
  without touching the binding or the wire, then re-acquires the binding
  mutex to prove this connection is still the live one before a byte is
  written: a revocation or detach that wins the mutex first denies the
  exchange outright, so no request can race past a revoke. Exactly one
  request is sent and exactly one reply consumed; a `Range` reply is
  trusted only after its base64 encoding fits the encoded-length bound
  before decoding and decodes within the requested bound, its offset
  echoes the request, its `next_offset` continues the delivered bytes
  exactly (which must then fill the bound) without overflow, and its
  stamp checks out: the revision is a 64-character lowercase-hex
  SHA-256 stamp matching any expected revision, the identity is a
  bounded `dev:ino` pair of two canonical decimals, and the stamped
  size agrees with the delivered range — offset and end may not pass
  it, `next_offset` is present exactly while the end stays short of
  it, and a continued reply must have filled the bound. A transport
  failure, truncation, unexpected reply type, malformed `Range`, or
  offset overflow retires the stream —
  `usable` is dropped, the binding is disconnected, and the socket is
  shut down; it is never reused with a reply accounting that cannot be
  trusted.

### Workspace confinement and range reads (implemented)

The physical filesystem discipline lives in `rustx-confined-fs`, factored
out of `rustx` — the same `PackageDirectory` the Agent Server's package
I/O was built on, not a parallel implementation. A root is opened once,
component by component from `/` with `O_NOFOLLOW | O_DIRECTORY`, and held
by descriptor; every retained ancestor identity is re-verified before
each operation so a replaced directory is detected, never silently
followed. `child` accepts exactly one normal component (or `.`), `names`
bounds listings, `contains` proves descent by identity, and `file`
resolves a path whose components are all normal, descriptor-relative
with no symlink traversal — there is no way to address `..`, an
absolute path, a non-normal component, or a special file.

`rustx_confined_fs::read_range` is the bounded revision-checked read the
Client serves: `max_bytes` must be within `1..=limit` (the Client's
limit is `MAX_RAW_CHUNK_BYTES`, 64 KiB; the root runtime keeps its own
`CHUNK_BYTES`), the file is opened through the caller's no-follow
opener, its `FileMetadata` revision is stamped, the read seeks to
`offset` and takes at most `max_bytes` — never a whole-file load — then
the descriptor metadata and a freshly re-opened path's metadata must
equal the pre-read stamp or the answer is a conflict, never unverified
bytes. A supplied `expected_revision` likewise conflicts instead of
serving a newer file. `FileMetadata`/`FileChunk` keep their established
serde names and `schemars` schemas; the root crate re-exports them
rather than duplicating the business types.

On the wire the operation is `ClientRequest::ReadRange { relative_path,
offset: DecimalU64, max_bytes: u32, expected_revision: Option<String>
}` answered by `ClientReply::Range { data_base64, offset, next_offset,
metadata }` or `ClientReply::Error { kind }` with the stable
`ReadError` codes `invalid`, `denied`, `not-found`, `conflict`,
`invalid-utf8`, `unsupported-document`, `document-too-large`,
`document-decode-failed`, `no-text-layer`, `unavailable` — no raw
paths, parser messages, or payload details cross
back. `metadata`
is a `FileStamp { size: DecimalU64, revision, identity,
modified_at_ms }`: the source descriptor of the file the bytes were
read from, projected from the same `FileChunk.metadata` the confined
read already verified — the Client performs no second stat for the
reply, and a nonregular result is denied rather than stamped. The wire
file type is implicitly regular; `Range` never describes another kind.
The Client validates `relative_path` to normal components only and
enforces `1..=64 KiB` before touching the filesystem.

The two query operations share the same exchange discipline.
`ClientRequest::GetMetadata { relative_path }` is answered by
`ClientReply::Metadata { metadata: EntryStamp }` — an `EntryStamp`
pairs the `FileStamp` descriptor with an explicit `EntryKind`
(`regular-file`, `directory`, `symlink`, `other`), stamped by
`fstatat` with `AT_SYMLINK_NOFOLLOW` through the retained
`PackageDirectory` so a leaf symlink reports itself, never its target.
`ClientRequest::ReadDirectory { relative_path, offset: u32,
limit: u32, expected_revision }` is answered by `ClientReply::Directory
{ revision, entries, next_offset }`: at most `limit` (never more than
`MAX_DIRECTORY_PAGE_ENTRIES`, 128) `PageEntry` records carrying a
sorted name plus its own `EntryStamp`, a directory `FileStamp`
revision for the expected-revision check, and a continuation cursor
that is exactly `offset + entries.len()` while a full page is
delivered. For both, an empty `relative_path` names the workspace root
itself — the only path that may; every other path is validated to
normal components, intermediate symlinks are refused, and the Server
side applies the same name discipline on the reply: strictly
increasing single components of at most 255 bytes each — never `/`,
`.`, or `..`, while control characters remain ordinary literal name
data on both DTO sides, matching the established product listing
contract — plus valid stamps, and `MAX_DIRECTORY_NAMES` (4096) as
the physical listing bound. A name
the wire shape cannot honestly carry — a non-UTF-8 member — is
reported `invalid` by the Client rather than emitted. Malformed replies — wrong cursor,
unordered or illegal names, bad stamps — retire the stream and detach
the binding exactly like a malformed `Range`.

### Logical text pages (implemented)

`ClientRequest::ReadLines { relative_path, offset: DecimalU64,
limit: Option<DecimalU64>, expected_revision: Option<String> }` is
answered by `ClientReply::Lines { metadata: FileStamp, text,
total_lines: DecimalU64, shown_lines: u32, end: LineEnd }` or a typed
`Error` — the bounded logical-text page the native Read tool projects,
scanned physically by the Client under its workspace view. The file
must be a leaf — never the root — and the offset is 1-based with any
`limit` nonzero; both are refused `invalid` before touching the
filesystem.

One scanner owns the contract on both sides: `rustx_confined_fs`
`scan_text` streams the source in fixed blocks — the file never lands
in memory — with at most the three tail bytes of an incomplete UTF-8
sequence carried between blocks. It validates UTF-8 across the
*whole* source, including bytes after the requested page (a rejected
page never hides an invalid tail; the wire code is `invalid-utf8`,
a semantic refusal that never retires a healthy connection), counts
every byte and `'\n'`-element — the trailing empty line included — and
stages only the bounded output prefix and the current bounded line: a
giant line is counted, never allocated. `text` is the shown elements
joined by `'\n'` — at most `MAX_TEXT_BYTES` (50 KiB) — `total_lines`
is the whole source's `split('\n')` count, `shown_lines` the shown
element count (up to `MAX_TEXT_LINES`, 2000, counted elements plus
one when a complete page keeps the trailing empty), and `end` is the
`LineEnd` mirror of the scanner's stop reason: `complete`,
`requested-limit`, `line-limit`, `byte-limit`,
`first-line-too-large` (the first selected line's full byte length),
or `offset-past-end`. The wire `MAX_TEXT_BYTES`/`MAX_TEXT_LINES`
constants are the wire policy the Client hands the scanner — one
policy, no duplicate bound.

Like every physical read, the file is stamped before the scan and
proven unchanged after it — the retained descriptor plus a freshly
opened path — or the answer is `conflict`, never unverified text; a
supplied `expected_revision` conflicts instead of serving a changed
file. On the Server side the reply is verified before trust: the
stamp, the nonzero `total_lines`, `text` within the byte bound whose
`'\n'`-split reproduces exactly `shown_lines`, `end` consistent with
the request and the page's own accounting, and every count bounded
by the stamped source size. A malformed or untruthful `Lines` reply
retires the stream exactly like a malformed `Range`.

### Structured document pages (implemented)

`ClientRequest::ReadDocument { relative_path, offset: DecimalU64,
limit: Option<DecimalU64>, expected_revision: Option<String> }` is
answered by `ClientReply::DocumentPage { source: FileStamp,
logical_bytes: DecimalU64, text, total_lines: DecimalU64,
shown_lines: u32, end: LineEnd }` or a typed `Error` — the same
complete-line page contract as `Lines`, projected over the decoded
Markdown of a whitelisted structured document (`.pdf`, `.docx`,
`.xlsx`, `.pptx`). There is no caller-supplied MIME type or URI on the
wire: the Client chooses the decoder by extension against the one
rustX whitelist — the same `rustx-document-reader` classification,
`MAX_DOCUMENT_SOURCE_BYTES` acquisition bound, and parser policy the
native Read tool runs — and every other extension is the distinct
`unsupported-document` refusal, never conflated with a missing
capability or a parser failure.

The document is opened no-follow, stamped before acquisition, and
refused `document-too-large` when the stamped size exceeds the
32 MiB source bound — before a single byte is read or a parser step
runs. The bounded acquisition is capped at the stamped size (a file
that keeps appending can never stretch the decode) and must observe
exactly the stamped byte count; a shrink mid-read is `conflict`. The
decoded Markdown then pages under the same `PageLimits` the plain
text scan uses, and the retained descriptor plus a freshly opened
path must still name the decoded source or the answer is `conflict`
— an unstable or corrupt source never leaks a page. `source` stamps
the physical document; `logical_bytes` reports the decoded Markdown
length, independent of the compressed source's size. A parser
rejection is `document-decode-failed`; a document with no
extractable text layer is `no-text-layer` — OCR and image inference
never substitute, and parser messages never cross the wire.

The decoder's own policy constants live in `rustx-document-reader`
(which cannot depend on the protocol crate); the wire policy —
`MAX_DOCUMENT_SOURCE_BYTES` and `MAX_DOCUMENT_LOGICAL_BYTES` (64
MiB) — lives in this crate, and the Client proves the identity at
build time. On the Server side the reply is verified before trust:
the `source` stamp, the source and logical bounds, and the shared
complete-line accounting every `Lines` reply answers to — with
`logical_bytes` playing the source-size role. A malformed or
untruthful `DocumentPage` retires the stream exactly like a
malformed `Range`; a typed semantic refusal never does.

### Conditional writes (implemented)

There is no raw write request. `ClientRequest::PrepareWrite {
relative_path, expected_revision, data_base64 }` carries the strict
relative path, an optional revision the retained original must equal
(`None` means create-only), and the whole payload — base64, length
checked before decode, decoded bound `MAX_RAW_CHUNK_BYTES` — answered
by `ClientReply::WritePrepared { ticket, original }`. The ticket is a
`DecimalU64` the Client issues monotonically from `1` per admitted
connection, never reused and never issued past `u64::MAX`; exactly one
preparation may be live per connection. `ClientRequest::CommitWrite {
ticket }` and `ClientRequest::AbortWrite { ticket }` act only on the
exact live ticket — a zero, foreign, stale, or repeated ticket is a
stable `invalid` and leaves the live preparation untouched. A commit
answers `ClientReply::Written { ticket, metadata }` carrying the
published descriptor's stamp, or `ClientReply::CommitUnknown { ticket
}` when publication may have crossed but the completion is unknowable;
an abort answers `ClientReply::WriteAborted { ticket }`. The Server
trusts a `Written` receipt only when its stamp is valid *and* its
`size` equals the exact submitted byte count — a receipt describing
different bytes cannot prove the publish, so it reports the unknown
outcome and retires like any other post-commit violation.

The Client implements the sequence on the shared
`rustx-confined-fs` kernel: `WriteTarget::prepare` retains the exact
target descriptor and verified original stamp without mutating the
named file, `stage` writes the payload into a private `O_EXCL` sibling
`.rustx-write-{uuid}` under the retained parent, and
`StagedWrite::publish` re-verifies the chain and the leaf stamp at the
retained descriptors before `linkat`/`renameat` crosses — the check and
the publish are honest pre-commit verifications, never a claimed CAS;
external writers can still rename between them. The Server side keeps
the same `LocalConnection` locked across prepare, the caller's
`authorize` gate run on the verified prepared `original`, and the
commit or abort dispatch — the path is never reparsed or reopened.
Once the first commit byte is attempted, a transport failure,
malformed reply, or mismatched ticket retires the stream and returns
an `io::Error` wrapping `rustx_execution_server::WriteOutcomeUnknown`
— the published state is unknowable and the operation is never
retried; an explicit well-formed `CommitUnknown` for the live ticket
maps to the same error without retiring, because the reply accounting
stayed intact. The `WriteOutcomeUnknown` marker downcasts through
`error.get_ref()`. A binding revoked after the prepare but before the
commit is dispatched gates the commit frame out entirely — the revoke
shuts the retained duplicate stream down, the idle Client exits on
EOF, and its dropped write state cleans the unpublished stage.

`rustx-execution-client --binding-file PATH --workspace PATH` is the
physical Client:

- Argv carries only paths; the environment is unused; the credential
  never appears in argv or env.
- The bootstrap record's `version` must equal `PROTOCOL_VERSION` before
  the workspace is opened or the socket touched; a foreign version is
  refused before any connection work.
- The bootstrap file is opened `O_NOFOLLOW | O_NONBLOCK`, then proven to
  be a regular file, singly linked, euid-owned, and exactly `0600`.
  The direct parents of both the binding file and the socket path must
  be non-symlink owner-only `0700` directories owned by this euid — a
  spoofed endpoint cannot be reached. The socket path itself must exist
  as a real socket, not a symlink.
- The file must contain exactly one framed record — trailing bytes are
  corruption — and all errors are static redacted messages.
- The workspace is local policy, never a server request: `--workspace`
  must equal the bootstrap workspace byte for byte (`OsStr` equality on
  a normalized absolute path) or the client refuses to connect at all —
  it never silently follows a path the server named.
- The workspace root is opened with `PackageDirectory::open` before
  connecting and its descriptor is held for the whole session; every
  read re-verifies the retained ancestor identities.
- Before the Hello — and therefore before the credential — is sent, the
  connected peer's euid must equal this process's euid (`SO_PEERCRED`
  on Linux, `getpeereid` on macOS): a socket planted by another user
  never sees the credential.
- The credential exchange runs under the same `transport::Handshake`
  budget the Server enforces: one absolute deadline armed once on the
  established socket (`Instant::now() + LOCAL_HANDSHAKE_TIMEOUT`),
  covering the Hello write, the `HelloReply` read, admission
  verification, and `finish()` restoring blocking mode before the client
  enters its serving loop — never re-armed on a partial record.
- The Hello advertises `version: PROTOCOL_VERSION` and the fixed
  capability set `{Connection, ReadRange, DirectoryMetadata,
  ConditionalWrite, ReadLines, DocumentPages}` — the
  operations this build physically implements — constructed by the
  client, never read from the mutable bundle.
- After sending the Hello it reads one `HelloReply`; a reject is
  reported by its stable code, and a matching `Admission` must echo
  exactly this binding's fields — protocol version included — before
  the client proceeds.
- The client then serves only typed requests until a clean EOF before a
  record ends it: `Ping` → `Pong`, `ReadRange` answered through the
  retained `PackageDirectory` via `rustx_confined_fs::read_range`,
  `GetMetadata`/`ReadDirectory` answered through its
  `entry_metadata`/`directory_page`, `ReadLines` answered through the
  retained `PackageDirectory` and the shared `scan_text` scanner,
  `ReadDocument` answered through `rustx-document-reader` — the same
  whitelist, bounded acquisition, and parser policy as the native
  Read tool — and
  `PrepareWrite`/`CommitWrite`/`AbortWrite` answered through the
  `WriteTarget`/`StagedWrite` kernel by the per-connection `Writes`
  owner. EOF
  after a successful handshake never permits the credential file to be
  replayed: the binding is `Detached` and the Server answers
  `RecoveryRequired`.

### Authenticated connection is not readiness (frozen)

`Admission` proves authenticated connection ownership only. It does not
report workspace, configuration, MCP, Python, or resource-discovery
readiness, and it does not grant dispatch. Those are separate facts with
separate later reports; nothing may collapse them into one `Ready`.
Connected means connected — never "environment ready".

### Credential delivery (frozen contract)

Credentials are server-issued high-entropy secrets, distinct from ordinary
App Server management tokens. The local channel is the owner-only
bootstrap file above (0600 inside a 0700 owner-only directory). Remote
deployment wires equivalent secrecy and the Client verifies the Server's
TLS identity. Credential revocation and replacement retire the old
binding's dispatch rights — authentication is not a one-time handshake
grant, and the binding verifies on every connect rather than trusting a
past handshake. The launcher arguments and identity persistence fields
are fixed by the layer that ships the launcher; the owner-only
requirement is fixed now.

## Frozen resource bounds (PR1)

These values are the registered bounded-resource constants
(`rustx_execution_protocol::*`); implementations may not exceed them and
no second set may exist.

| Bound | Value |
| --- | --- |
| Encoded transport frame (envelope included) | `MAX_ENCODED_FRAME_BYTES` = 1 MiB |
| Raw content chunk before encoding | `MAX_RAW_CHUNK_BYTES` = 64 KiB |
| Data records per connection | `MAX_DATA_RECORDS_PER_CONNECTION` = 8 |
| Control records per connection | `MAX_CONTROL_RECORDS_PER_CONNECTION` = 4 |
| Optional Server → Client save absolute deadline | `OPTIONAL_SAVE_DEADLINE` = 5 s |

The encoded-frame bound is enforced on the final serialized bytes —
envelope, metadata, and escape expansion included — at the read/parse
boundary before allocation; raw chunk size never relaxes it. Data and
control records travel on separate bounded queues so cancellation, health,
and control progress are guaranteed even when data records saturate the
connection. The 5-second save deadline is an independent finite budget for
one optional save; it never extends the calling operation's remaining
deadline and never re-arms on slow writes.

## Frozen pending contracts

These are normative shapes frozen by this ADR. They are not implemented in
this layer, and code must not fake them.

### START: pre-execution atomic registration

The Server assigns `OperationId`s — foreground and background alike —
fixed before first dispatch under the same Server incarnation, Session,
Client instance, and generation. The Client atomically registers the
identity, normalized parameter fingerprint, and cancel/start state before
any business spawn; only the first-creation owner reaches the local START
gate. Concurrent or late same-identity same-parameter requests attach to
the original record (`Starting`, running, cancelled, failed, settled);
same-identity different-parameter requests are explicitly rejected, never
overwrite. A query is observation, not a launch permit: a miss proves
nothing about an in-flight START. Retried identities keep minimal
tombstones until the generation stops accepting their retries; capacity
refuses new STARTs, never LRU-forgets live tombstones. A new instance
answers `Unknown` to old identities — no cross-instance exactly-once.
Background Bash additionally keeps prepare-then-accept: `Prepare`
registers the owner and allocates the unique managed file without
starting the process; `START` verifies acceptance evidence at the same
registration.

### Server → Client optional save

Complete HTTP MCP and winning Host results may be saved into a
Client-owned unpublished staging object under the owning namespace —
never a Server/Host-chosen path. Chunks carry deterministic offsets;
identical re-delivered chunks dedupe by object/offset/content, and
conflicts, gaps, or reordering reject. Sealing verifies accumulated
length, continuity, and the end digest. Only the winning, domain-valid
result with real save evidence may be published. `Complete` means fully
sealed with a confirmed published locator — allocation receipts, sent
bytes, a received `End`, or a won claim are not enough. `Partial`
publishes only evidenced contiguous prefix and says so; `Unavailable`
says the full text was not saved or cannot be retrieved. Business
completeness and save completeness settle separately: a save failure
after a complete result keeps the business verdict with a bounded
preview and diagnosis — the business is never redone and arbitration
never reopens.

### Host chunked results

Four message semantics — `Begin`, `Chunk`, `End`, `Abort` — share the
existing response arbitration with ordinary single-message results and
errors. A candidate identity binds the Server incarnation, the original
callback request/invocation, the Host connection/subscription
incarnation, and an `UploadId`. `Begin` registers a bounded receiving
state and never claims the response, never blocks other eligible Hosts.
`Chunk` carries candidate identity, content item/order, exact offset, and
bytes; it updates incremental length/digest/bounded preview and may flow
into unpublished Client staging — it never aggregates the full text on
the Server, never writes a Server file, never commits per-chunk
canonical facts. `End` verifies envelope completeness, contiguous range,
declared length, and the incremental digest, then performs exactly one
claim attempt in the original lifecycle/correlation gate; the first
complete valid result (or a contract-conformant complete JSON-RPC error)
wins. `Abort` retires the candidate without a result. Winner-only
publication; losers' uploads and staging are cancelled and cleaned by
ownership, and losing paths are never published. Transport/integrity
verification precedes claim; business-domain validation keeps its
original post-claim order.

### Canonical byte layout and metadata

Chunk offsets are absolute byte offsets within the serialized content
stream of their item; the declared total is the sum of item bytes. The
result digest covers a canonical little-endian layout so escaping or
message splitting can never change content identity:

```text
content_item  := type_tag u16le | byte_length u64le | content bytes
digest_input  := item_count u32le | content_item* | declared_total u64le
result_digest := SHA-256(digest_input)    (32 bytes; lowercase hex on the wire)
```

`type_tag` assigns one code point per result content kind at schema
freeze; item order is significant and preserved. Immutable result
metadata — content-item count, per-item type and declared length — is
fixed at `Begin`, verified again at `End`, and bounded. Chunk bytes
append to the incremental digest; re-derived metadata never mutates the
envelope.

## Explicitly not claimed

- No remote transport exists: only the synchronous length-prefixed
  record framing and the local Unix-socket bootstrap are implemented.
  WSS/TLS, health budgets, and recovery windows are pending.
- No spawn, MCP session, Python, managed output, or any file
  operation beyond the range read, the two query operations, the
  two-phase conditional write, and the two logical-text page
  operations exists
  behind `Admission`;
  `Binding::status()` is observation only, and a `Live` phase is a
  connection fact, not an integration claim. `Capability::Files` and
  everything it implies stay unadvertised until that capability lands.
- No readiness reporting exists; no file/config/MCP/Python readiness is
  implied by a live connection.
- No Client instance replacement, generation increment, or death-cleanup
  evidence path exists; a detached binding rejects every Hello with
  `RecoveryRequired` until #577 lands.
- The only caller inside `rustx` today is the native composition layer:
  `SessionRuntimeManager::reserve_local_execution` claims one
  process-owned `LocalExecutionEndpoint` per Session (binding, listener,
  and owner-only bootstrap in a private directory) for the trusted
  composer. The endpoint now carries the first dispatch path: the
  composer hands an accepted Client stream through
  `admit_local_stream`, and `read_range` performs one bounded
  synchronous exchange at a time — a concurrent dispatch is refused as
  `WouldBlock` busy rather than queued, a workspace check rejects
  paths outside the Session's reserved workspace lexically, and the
  binding's live-connection gate authorizes every send at dispatch
  time. `app-server --local-execution-session
  --local-execution-workspace` is that caller: the paired flags claim the
  Session's endpoint, launch the trusted sibling Client (sibling-of-
  `current_exe()` only — never PATH or HOME — with an empty environment,
  `/` cwd, sealed stdio and `kill_on_drop`), race its connect against
  `LOCAL_HANDSHAKE_TIMEOUT`, admit it through the blocking handshake, and
  stop+reap it inside the drain deadline; bare mode composes nothing.
  `fs/readFile`, `fs/getMetadata`, `fs/readDirectory`, and
  `fs/writeFile` now route
  through this owner: the App Server resolves the request's `sessionId`
  through the non-creating `execution_endpoint` lookup (absence is
  `ExecutionUnavailable`, never a fallback), authorizes the admitted
  workspace against the configured environment roots before and after
  the exchange, and projects the returned `RangeRead`, `EntryStamp`,
  or `DirectoryListing` — including their source-descriptor
  `FileStamp`s — into the wire shapes. The write adds its own
  mutation admission: the workspace grant and the caller-closed flag
  are checked before the prepare and again inside the commit gate on
  the retained prepared original, `WriteOutcomeUnknown` maps to
  `ErrorData::OutcomeUnknown`, and a known-successful commit is never
  reported as cancelled or rolled back. What is proven is the
  authenticated binding, the retained target, the two-phase commit,
  and the unknown-outcome contract; revoking a Client while its stage
  is unpublished closes the physical stream so the stage is dropped —
  what is *not* yet proven is revoking a commit already inside the
  Client's physical publish gate, which remains control work for a
  later change. Every other
  filesystem method still flows through the prior environment readers
  (delegating to `rustx-confined-fs`); their migration remains
  outstanding.

## Test ownership (T01–T73) and optional O-SUM

Each contract below maps to its primary owning PR(s) per the plan's
deterministic test matrix. The evidence already landed is marked in
§ Verification; everything else is pending evidence in its owning layer,
not skipped.

| Test | Contract | PR |
| --- | --- | --- |
| T01 | Client-less model loop, internal tools, HTTP MCP, eligible Host callbacks work | 1/3 |
| T02 | Read/write/bash with a truly separate filesystem view | 1 |
| T03 | Two Clients/connections on one Session: one dispatchable | 1/2 |
| T04 | Session/Root/Subagent/Workflow routing correctness | 1/3 |
| T05 | Cancellation before dispatch or model request: zero execution | 1/2 |
| T06 | Parallel results in call order; terminal state unique and last | 1/3 |
| T07 | Same-instance recovery reruns nothing, no duplicate output | 2 |
| T08 | Stdin safe retransmission without duplicate input | 2 |
| T09 | One Client failure kills no other Session or Server-only work | 2 |
| T10 | Stale results/watch/cleanup never pollute a new instance | 2 |
| T11 | Output/spill backpressure never starves control/cancel/health | 1/2 |
| T12 | UI detach vs Host disconnect vs Execution disconnect distinguished | 2/4 |
| T13 | Three-layer config, default/explicit paths, missing vs empty | 3 |
| T14 | Explicit config path does not move the `.agents` resource root | 3 |
| T15 | Workflow permission independent of YAML definition | 3 |
| T16 | Model/Profile selection consistent across entries | 3 |
| T17 | Session inject/restore, candidate failure, safe activation | 3 |
| T18 | Bootstrap has no circular dependency; unknown config never fakes empty | 1/3 |
| T19 | MCP declaration cache isolated by identity and invalidated | 3 |
| T20 | Optional MCP init never blocks the first inference round | 3 |
| T21 | MCP versions, MRTR/Tasks, resources, interaction, cancel intact | 2/3 |
| T22 | Skills stay plain read/bash with real reachable assets | 3 |
| T23 | Committed history/previews/artifacts readable without Client | 4 |
| T24 | Server/Client independent builds and permission boundary | 1/4 |
| T25 | Three artifacts' Linux/macOS entry, install, exit | 4 |
| T26 | Revocation before local file-commit/START: zero side effects | 1 |
| T27 | After Server restart, unfinished old resources bar new env work | 2 |
| T28 | Binding auth never reusable across Sessions or old instances | 1/2 |
| T29 | UI-direct commands are not Agent attempts; disconnect cleans up | 1/2 |
| T30 | MCP/Host/Python foreground; Bash background contract kept | 2/3 |
| T31 | No Client: oversized Server results honestly truncated | 1/3 |
| T32 | Oversized Server results imported whole to Client, readable | 1/3 |
| T33 | Save interruption/disk-full/timeout never redoes won business | 2/3 |
| T34 | Save selection point vs late/replaced Client correct | 1/2 |
| T35 | Write-ack vs canonical commit race: exactly one result | 2 |
| T36 | Host broadcast/first-complete/domain-validation order kept | 1/3 |
| T37 | Frame/result/concurrency/memory/model/save budgets enforced separately | 1/3 |
| T38 | HTTP fixed on Server; no Client-side localhost proxying | 3 |
| T39 | Spill ownership stable across fork/clone/restart/delete | 2 |
| T40 | Watch overflow/reconnect repairs instead of faking history | 2/3 |
| T41 | Local two-process and remote protocol env semantics identical | 1/4 |
| T42 | Large file/spill read/grep never full-loads or full-pulls | 1/4 |
| T43 | One-way faults and detection skew meet recovery budgets | 2 |
| T44 | Minimal Session binding before directory browse; no pool | 1/4 |
| T45 | Delivered artifacts vs temporary Spill stay separate | 3/4 |
| T46 | Host streaming receive never aggregates full text on Server | 3/4 |
| T47 | Skill narrow discovery and resource-snapshot invalidation | 3 |
| T48 | Foreground/background share idempotent START; unique accept | 2 |
| T49 | Session delete offline cleanup neither over-claims nor revives | 2/4 |
| T50 | Host Begin never preempts; first complete valid result claims | 3 |
| T51 | Host complete upload vs Client save failure settle separately | 2/3 |
| T52 | Over-1-MiB bounded complete results received without a Client | 3/4 |
| T53 | Incomplete/corrupt uploads never take business response rights | 3 |
| T54 | Duplicate/late/cross-candidate chunks never splice or republish | 1/3 |
| T55 | Message size judged on encoded bytes, not raw text | 1/3 |
| T56 | Host candidate resources bounded; cleanup before quota release | 2/3 |
| T57 | Client generation change mid-upload keeps business integrity | 2/3 |
| T58 | Concurrent duplicate STARTs (fg/bg) produce one execution | 2 |
| T59 | Query miss with in-flight START still never double-starts | 2 |
| T60 | Terminal/cancel/failed retries never revive; tombstones kept | 2 |
| T61 | Old Operation on a new instance reports Unknown only | 2 |
| T62 | Client killed around spawn: cleanup/admission windows proven | 2 |
| T63 | Death-pipe FD leak/supervisor death never cause false takeover | 2 |
| T64 | Descendant escape and PID/PGID reuse never fake cleanup | 2 |
| T65 | Server-control disconnect vs Client death stay distinct | 2/4 |
| T66 | Managed Python fingerprint inputs unchanged on Client | 2 |
| T67 | Same Store/fingerprint concurrency: one physical prepare owner | 2 |
| T68 | Corruption never repaired in place; per-call permission bind | 2 |
| T69 | Private env lifetime distinct from Session/Spill/process | 2/4 |
| T70 | Store reuse domain and racing publish proven exactly | 2 |
| T71 | Credential activation keeps legal old leases; new admission uses new generation | 3 |
| T72 | Ordinary rotation never hides cancel/expiry/remote refusal nor replays | 3 |
| T73 | Source save, candidate failure, activation, stale discovery distinct | 3 |

Optional O-SUM (only if explicitly selected; not a release gate):

| Test | Contract | Status |
| --- | --- | --- |
| O01 | Bounded locator reaches summary input under long-body pressure | optional |
| O02 | No fabricated paths/integrity facts; states stay distinct | optional |
| O03 | Summary-input quality only; no persistent Spill mechanism | optional |

## Verification

Commands for this layer (run from the repository root):

```sh
cargo fmt --all -- --check
cargo clippy -p rustx-execution-client -p rustx-execution-server -p rustx-execution-protocol -p rustx-confined-fs --all-targets --locked -- -D warnings
cargo test -p rustx-execution-client -p rustx-execution-server -p rustx-execution-protocol -p rustx-confined-fs --locked
cargo check -p rustx --all-targets --all-features --locked
cargo tree -p rustx-execution-client -e normal --locked
cargo run --profile test --all-features --locked --example check_test_lanes -- --job rust-fast
```

Distinct mount-view evidence (Linux, opt-in, requires bubblewrap — a
read-only T02 fragment, not a CI gate). One script runs both proofs:
the boundary-layer RPC and the product-entrance composition:

```sh
test-support/prove_execution_mount_view.sh
```

Landed evidence: canonical generation parsing; exact credential wire
form with redaction and no echo in errors; workspace normalization; the
credential verifier checked before any field probe; literal `OsStr`
workspace comparison rejecting `/work//root`; wrong-field rejection
without reservation consumption; a barrier-synchronized two-Hello race
committing exactly one live connection; immediate terminal revocation;
stale-EOF tolerance; `RecoveryRequired` on any re-admission after EOF —
including replay of the same static credential — so no credential file
grants a second session; `reserve_local` private-directory enforcement
and leftover-free publication; `accept_local` handshake with a fresh
server-generated `ConnectionId` and RAII disconnect; and real
two-process tests over a Unix socket that drive the built
`rustx-execution-client` binary through handshake, `Ping`/`Pong`, clean
shutdown, post-EOF replay rejection, `0644` file/`0755` directory and
hardlink denial, no retry or artifacts on reject, caller-controlled
reaping without orphans, and — for this layer — exact bounded bytes at
a sparse-file marker near EOF with correct offset/`next_offset` and a
full source-descriptor stamp (size, revision, `dev:ino` identity,
mtime) verified against independently opened metadata — plus an
exact-EOF read returning empty bytes under the same stamp and a fully
consumed empty file — stale-revision conflict,
symlink/`..`/absolute-path denial,
unbound-workspace refusal before connect, foreign bootstrap-version
refusal before connect, revoke-before-read denial, tampered-bundle
capability refusal, admitted-capability gating (a `Connection`-only peer
gets `Unsupported` without a request), stream retirement on protocol
violations, and clean-EOF versus truncated-record exit status. On Linux
the opt-in `range_read_from_private_mount_view` proof reads the marker
through a `bwrap` private bind mount of the same absolute workspace path
while the server side proves the marker stays `NotFound` before and
after the RPC — a read-only T02 fragment. Its product-entrance
counterpart, `explicit_composition_reads_from_client_mount_namespace`
in the `process` target, composes the real App Server pair with a
test-only sibling launcher that wraps the staged Client in the same
`bwrap` layout (a fixture artifact only — production resolves the
sibling executable directly and has no override): the product
`fs/readFile`, `fs/getMetadata`, and `fs/readDirectory` calls then
serve the marker bytes and the source stamp from the Client's mount
view, the listing shows the marker and never the Server-side decoy,
the decoy read is `EnvironmentResourceNotFound` with no Server-local
fallback, and the Server's own view keeps the marker `NotFound` and
the decoy intact before and after.
T03/T24/T28's connection-singularity,
dependency-boundary, and cross-instance rejection portions and T42's
bounded-read portion have their first coverage here; the remainder of
the matrix is pending in its owning layer. The process-owned Session
claim adds a further limited T03/T28 fragment — registry-level proof
that one claimant wins preparation, that a dropped waiter never strands
the claim, that two Sessions never share binding material, and that
delete/drain revokes — but it dispatches no work and proves no recovery,
so the full contracts stay pending in their owning layers.
