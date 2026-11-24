# Execution Protocol and Binding Arbitration

Status: accepted. The current implementation includes binding arbitration,
authenticated local and WebSocket bootstrap, bounded filesystem queries and
mutations, watches, supervised process execution, bounded same-instance
recovery, Client-owned storage and stdio MCP, and streamed Hosted reception.
The [EXEC-01](exec-01-evidence.md), [EXEC-02](exec-02-evidence.md) and
[EXEC-03](exec-03-evidence.md) ledgers record the evidence and verification gates.

This ADR grounds its decisions in the verified server/client separation
plan `rustx-server-client-development-plan-v1.3-2026-11-16.md`
(SHA-256 `563462bf8da10b626491316b57143e9baeae95d1c5f899d2326e38a4dd90c9bc`)
and in these issues:

- First implementation issue:
  <https://github.com/Caismis/rustX/issues/576>
- Live-instance recovery and re-admission proof:
  <https://github.com/Caismis/rustX/issues/577>
- Hosted, configuration and MCP integration:
  <https://github.com/Caismis/rustX/issues/578>
- Installed-product integration:
  <https://github.com/Caismis/rustX/issues/579>
- Earlier Server/Client groundwork
  (<https://github.com/Caismis/rustX/issues/571>) is still open at this
  base; nothing here depends on it.

Fixed reference commits from the plan (frozen snapshots, not
"current HEAD" claims; only the plan hash above was re-verified): rustX
`d82d7f9b8d9dd4f5064dba8d377103c8c6049d28`, Codex
`ee344ca9458351a6ea102795fa73110ad3ede5dc`, Pi
`22fc026b872b4a7df22ffa94e8fa00b873128054`. EXEC-01 began on
`f0cd4f59d`. The stacked EXEC-03 branch starts at
`5ff54d8327917bca2cdbc9647d7da197afe8beed` on the EXEC-01 branch
after it incorporated EXEC-02.

## Decision

### Crate and naming boundary

The execution boundary uses these workspace members.

- `rustx-execution-protocol` (`crates/execution-protocol`): the single
  wire definition — identity newtypes, `Hello`, `Admission`, `Reject`,
  `HelloReply`, `ClientRequest`/`ClientReply`, `ReadError`, `DecimalU64`,
  `Bootstrap`, the `Capability` vocabulary, the `transport` record
  framing, and the frozen resource bounds. Both sides serialize these
  exact DTOs; there is no duplicated contract type.
- `rustx-execution-transport` (`crates/execution-transport`): paired
  channel transport, TLS and frame deadlines, shared by both processes.
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
  Execution Client binary `rustx-execution-client`. Its runtime dependencies
  include the protocol and transport crates, descriptor-relative filesystem
  and document readers, and the physical process supervisor. The production
  dependency tree excludes the Agent Server, canonical store and model adapters.
- The main Agent Server remains the existing `rustx` package. Its
  `package_io` module uses `rustx-confined-fs` for Server-owned files.
  Physical queries on an admitted Workspace use the Client.

The implemented Client supports authenticated local and WebSocket binding,
bounded file queries and tree mutations, conditional writes and edits,
supervised processes, workspace observation, file watches, Workspace
Python, managed output, stdio MCP and Managed Python. The authenticated Hello advertises the
individual services that this Client implements. The Server checks the
specific capability before dispatch.

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

The Client opens a pair with a revision 8 Hello. It retains
`physical_settlement_proven` in terminal process snapshots and requires the
same-instance recovery handshake after a transport loss. Managed Python
source capture and generic stdio launch carry exact binding identity in
this revision. Older revisions
are rejected during bootstrap or Hello, before execution admission:

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

Every lane's Hello passes the same checks, under one mutex hold, in
this order: the credential verifier first in constant time — no identity
field may be probed without valid authentication — then `version` equals
`PROTOCOL_VERSION` (incompatible versions are explicitly rejected; no
implicit downgrade), then session, environment, instance, and generation
each equal the reservation, then the workspace by literal `OsStr` bytes
(so `/work//root` and `/work/./root` never alias `/work/root`), then —
the data lane only — the advertised capabilities cover the required set.
A `Hello` is never trusted to select required capabilities or the
expected instance — those come only from the reservation.

`Reject` carries machine-stable codes only. No reject, error, log, or
`Debug` output echoes a submitted credential or any secret-bearing field.
`BindingCredential` is exactly 32 random bytes as 64 lowercase hex
characters; it is non-`Debug` (redacted), has no `Display`, and malformed
wire forms — wrong length, uppercase, non-hex — are rejected without
echoing the submitted value.

### Paired lanes (implemented)

Admission is always a *pair* of independently authenticated physical
channels to the same binding — never a dispatch owner and a side door.
The first message on each channel is the tagged `PeerHello::{Data,
Control}`.

- **Data half first.** A validated `PeerHello::Data(Hello)` allocates a
  pending attempt: a fresh 256-bit `PairingNonce` challenge (redacted
  `Debug`, only its SHA-256 verifier retained), a unique attempt id, and
  the absolute pairing deadline — the same ~5-second budget the accept
  armed, covering every lane's wait. Issuing a challenge consumes
  nothing: `phase` stays `Reserved` and the attempt is not dispatchable.
- **One attempt per Session, never superseded.** A second authenticated
  data `Hello` while an attempt is live is refused `AlreadyConnected` —
  a parallel claim cannot evict the honest pair's half, and at most one
  pending attempt (and one watchdog) exists per Session.
- **Challenge then store.** The `DataAdmission{admission, challenge}`
  reply goes on the wire before the channel is stored into the attempt.
  A correct nonce can only be learned from that reply, yet scheduling
  lets a `ControlHello` legitimately arrive in the store window — a
  verified nonce with no channel yet *waits* on a per-attempt signal,
  bounded by the attempt's own deadline, never a refusal of the honest
  pair. A wrong nonce never waits and never burns the attempt.
- **The control half commits exactly once.** `PeerHello::Control
  (ControlHello{…identity…, connection_nonce})` re-runs the identical
  identity gate (capabilities excluded), verifies the nonce against the
  pending verifier, and rechecks the data peer's liveness at the commit
  itself — never relying on the watchdog having already won. The
  transition to `Live` happens exactly once under the mutex; the
  control lane's admission delivery is clamped to
  `min(data_deadline, control_deadline)` so a delayed second half can
  never stretch the budget, and a committed pair whose admission cannot
  be delivered is detached with both lanes shut — a binding never stays
  live on a pair the peer does not know it owns.
- **The pending half is disposable.** A watchdog (one bounded thread)
  polls the held lane for peer bytes or a close — either kills the
  attempt and the reservation stays `Reserved` — and exits immediately
  when the attempt settles by commit, discard, or fence (a cancel pipe
  wakes it; it never sleeps on past settlement).
- **`Pending` and `Rejected` are honest outcomes.** The data half's
  accept reports `ChannelAccept::Pending`; the control half commits to
  `Live` or rejects `InvalidPairing`/`AlreadyConnected`. A synthetic
  `Binding::connect` exists only behind `cfg(test)` for pure-arbitration
  tests — the product path never calls it.

### The control lane (implemented)

The control lane is an independently authenticated physical channel —
not tunneled over the data wire, and never a second dispatch owner. The
Client paces a continuous turn loop: each `ControlPoll{sequence,
receipt, health}` carries a fresh nonce and acknowledges the previously
delivered request; the Server's `ControlTurn{sequence, health,
request}` must echo both before its request counts. Idle polls pace by
a bounded period under per-frame deadlines — no busy-spin, and a
delivered command's receipt arrives on the very next poll.

- `ControlRequest::{Health{nonce}, CancelMutation{ticket}, Revoke}` map
  to exact `ControlReply` receipts — `Healthy{nonce}`,
  `MutationCancelled{ticket, disposition}`, `Revoked` — each verified
  for the exact echo by the `ControlHandle` caller.
- `CancelDisposition::{CancelledBeforeCommit, AlreadyCrossed, Unknown}`
  is the Client's own arbitration: `CancelledBeforeCommit` proves the
  publish never crossed; `AlreadyCrossed` that it had; `Unknown` that
  no live ticket matched — never claimed "cancelled with no effect",
  never retried.
- The command budget is four queued plus in-flight commands. Health
  probes receive `WouldBlock` immediately when full. Cancellation,
  StopWatch, and Revoke can wait for a slot within the original absolute
  command deadline. A delivered Revoke uses that same deadline for its
  physical settlement receipt. A lost lane leaves an admitted Client's
  settlement unconfirmed unless the binding recorded a valid receipt.
- The `ControlHandle` is cloneable and independent of the data
  connection's dispatch lock: health and cancel progress while a data
  write is physically blocked mid-send (proven on both transports with
  capped socket buffers and a real non-draining peer) and while every
  one of the 8 endpoint data permits is held by a live exchange.
- A lost control lane terminates the pair: the binding detaches or
  revokes, both lanes' physical shutdown handles are seized without
  touching the data lock, and no dispatch may start — never an implied
  release or replacement. The Client mirrors it: its control worker's
  RAII fence revokes local commit arbitration and interrupts the data
  channel on *any* exit — protocol violation, EOF, or orderly stop.

### Linearization and lifecycle (implemented)

One mutex is the binding arbitration point: Hello validation, pairing
commit, disconnect, and revocation all linearize under it. The phase
machine is `Reserved → Live → Detached`, plus terminal `Revoked` from
any phase, and `Live` always means a *committed pair* of lanes.

- A failed Hello consumes nothing: `Reserved` stays `Reserved`, `Live`
  stays `Live`.
- While `Live` — or while a pending attempt is negotiating — a second
  data Hello gets `AlreadyConnected`: at most one data lane per
  Session binding at a time.
- `disconnect(connection)` moves `Live → Detached` only for the current
  live pair, seizing both lanes' shutdown handles. Losing either lane
  detaches the pair identically — a stale or foreign disconnect is
  ignored.
- A recoverable transport loss opens a 50-second Server admission window.
  Re-admission requires the memory-only credential derived from the first
  authenticated pairing challenge and launch credential. The new pair must
  match the original reservation and admitted capabilities and gets fresh
  lane identities. The launch bundle alone cannot reconnect. A revoked
  owner, protocol violation or expired window cannot re-admit work.
- The Client retains process records, output cursors, stdin receipts and
  published output across this exchange. Per-lane mutation arbitration and
  watcher registrations are retired before reattachment; ticket numbers
  continue monotonically, and automatic Workspace watches register and
  rescan on the recovered lane. Old-lane watch hints cannot publish.
- The Client begins physical cleanup after at most 80 seconds from its
  detection of the loss, or on earlier authenticated terminal refusal.
  This timer does not prove cleanup. Existing operation deadlines may
  finish work or shorten the period in which a caller can observe it.
- An idle control poll runs every 100 ms and an ordinary control frame
  expires after 2 seconds. Under bounded worker scheduling, one frame in
  flight and the next poll with its frame deadline give a 4.1-second
  detection-skew budget. Revoke continues to use its absolute command
  deadline. Neither delayed polling nor a failed handshake renews recovery.
- Logical Session deletion and Host drain close business admission
  immediately. A transport loss before sending Revoke or before obtaining
  its receipt keeps the same instance's cleanup admission under the fixed
  recovery window. Host drain accepts cleanup only until its overall
  shutdown deadline. Both pairing and the exact Revoke confirmation check
  that deadline under the binding lock, including receipts from commands
  already in flight when drain starts. The recovered Client returns its
  previously proved physical settlement or finishes the remaining cleanup. The Server
  acknowledges the final control poll before closing the pair.
- The Session recovery owner is written to a bounded private staging file,
  synced, published atomically without replacement and directory-synced
  before first admission. Inventory readers only observe committed owners.
  An interrupted publication leaves no partial owner for another Workspace
  to read.

### Independent Linux local settlement

The `interactive-supervisor client-domain` role owns a Bubblewrap PID
namespace for the local sibling Client. The same Client becomes PID 1.
Before it executes, the trusted init transfers its own pidfd over a private
socket. The host guardian retains that descriptor. Before the launch grant,
a companion watcher acknowledges its own copy of the same descriptor and
then monitors a private socket for guardian loss. The Server holds the only
writer of a separate lifetime pipe. EOF on that pipe lets the guardian
signal the pinned init. If the guardian dies, EOF on the private socket
lets the watcher signal the same init. The Client inherits neither lifetime
channel. Linux finishes teardown, including descendants in other process
groups or sessions, before the pidfd reports exit. The watcher has no
receipt authority. The per-operation supervisor and START and cancellation
gates still run inside the namespace.

The independent helper retains the original private scratch directory
descriptor. After namespace exit it removes that exact directory and
publishes a durable receipt. The receipt authenticates the persisted
reservation including Server incarnation, Session, Client instance,
generation and Workspace. The helper's exit, a kill request and signals
to a numeric PID cannot authorize replacement. The binding fence consumes
the proof once. Revoke accepted through the authenticated pair remains a
separate physical settlement path.

A local Client allocates its Managed Python and Workspace mutation staging
directories through the independent watcher. The watcher authenticates the
exact live namespace init and accepts an anchored parent descriptor. The
Client verifies the returned device and inode before using the private
directory. Python stages share the filesystem with the published Store.
Workspace stages share it with the destination's existing parent. A single
mutation stage serves synchronous data requests and is released after its
commit or refusal. Only the existing filesystem publish gate can make
Workspace bytes visible. A write that crosses the gate and loses its
cleanup receipt reports CommitUnknown. Authenticated Revoke closes staging
admission and confirms release once the physical workers settle. After
namespace exit the watcher reclaims the exact outstanding directories before
the guardian can publish settlement. Completed Workspace files and published
Python fingerprints survive that release.

A separately trusted remote deployment host can receive a one-time
settlement grant through `--execution-settlement-grant-out`. The Server
publishes the grant to a new owner-only file outside the Client bootstrap.
Its receipt is valid only for the previous Session, incarnation, Client
instance, generation and Workspace. The deployment owner must retain the
grant outside the Client and its descendants. It may publish the receipt
only after independently proving that its original physical domain and
outstanding writers are settled or isolated. A deployment with no such
owner remains fenced after unconfirmed death.

The grant contains the exact Server `receipt_path` and a 32-byte capability
encoded as `proof_base64`. The trusted host returns those bytes through an
authenticated private transfer. An operator publishes them atomically at
that path as an owner-only regular file. The Server validates the binding's
original persisted verifier and consumes the receipt before replacement
admission on restart. The grant never appears in protocol Hello, ordinary
management authentication or the Client's launch bundle.

The local App Server waits 1, 2 and 4 seconds before attempting successive
replacements. It permits at most three attempts in a rolling 60 seconds,
with backoff reset after five stable minutes. A failed replacement launch
ends that owner. A replacement keeps the Session identity and advances its
generation. A restarted Server establishes a fresh incarnation. Neither
path reissues previous business input or reconstructs a missing outcome.

A killed independent helper, a missing receipt or a scratch identity
mismatch leaves the durable admission fence in place. The independent
receipt covers the Linux process namespace and its private temporary output.
It does not authorize deletion of arbitrary Workspace names or published
Managed Python Store directories. Independently operated remote Clients and
macOS local Clients still require authenticated settlement from their original
owner. A remote provider cannot use a local domain receipt to clear its fence.

### Binding status and bootstrap

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
- Publication retains the proof of what it created: the `create_new`
  file descriptor, the verified parent-directory descriptor, and the
  file name. Teardown — failure cleanup and endpoint `Drop` alike —
  removes the bundle only while `fstatat(AT_SYMLINK_NOFOLLOW)` through
  that descriptor still proves the same type, device, and inode; a
  replaced, symlinked, FIFO, or identical-bytes foreign file at the
  path is preserved untouched, and no pathname is ever reopened, so a
  FIFO replacement cannot block drain.
- The published `Bootstrap` carries only the reservation's own facts —
  socket path, protocol version, session, environment, instance,
  generation, workspace, and the credential. It never carries
  capabilities: the Client claims exactly what it implements from its
  own build, so a tampered bundle can never make it advertise a feature
  it lacks.

`Binding::accept_local(stream)` performs the handshake on one accepted
`UnixStream` under the shared `transport::DeadlineStream` wrapper:

- One absolute deadline is armed once at accept entry —
  `Instant::now() + LOCAL_HANDSHAKE_TIMEOUT`, the single shared 5 s
  constant owned by `rustx-execution-protocol::transport` — and covers
  the entire handshake: the Hello read and parse, arbitration, the
  admission write, and restoring blocking mode at `finish()`. It
  is an absolute deadline, not an inactivity timeout: `DeadlineStream`
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
  sequence), and
  `edit_file(...)` (`PrepareEdit` sharing the same ticketed
  commit/abort) — never a raw
  message path. Each first consults the capabilities the peer actually
  advertised in its Hello: a connection that never claimed `ReadRange`,
  `DirectoryMetadata`, `TreeMutations`, `ConditionalWrite`, `ConditionalEdit`,
  `ReadLines`, or
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

### WebSocket transport (implemented)

The shared `rustx-execution-transport` crate carries every record the
same typed way on both channel kinds: `RecordChannel::{Unix, WebSocket}`
sends exactly one bounded `EncodedRecord` per WebSocket binary message
and decodes it through the same `decode_record_body` JSON bound as the
Unix length-prefixed frame — identical `MAX_RECORD_BYTES`, identical
serde, identical application semantics. The WebSocket RFC 6455 framing
and TLS record layer are transport mechanics only; they carry no
protocol meaning and are not a substitute for the independent
control/backpressure work still tracked under T11 — RFC ping/pong does
not discharge it.

The Execution Client validates the bootstrap URL before any connect. It
requires `wss://`, a literal IPv4 or bracketed IPv6 address, and the
exact `/execution/v8` path. A port may be omitted to use 443. Userinfo,
queries, fragments, hostnames, and non-ASCII or overlong URLs are refused.
TLS verifies the certificate and IP SAN before the HTTP upgrade on both
lanes. The upgrade rejects a wrong path and browser `Origin` headers.

`Binding::reserve_websocket(reservation, listen, bootstrap_path,
public_url)` reserves a numeric loopback listener. Port `0` selects an
ephemeral port. A direct reservation generates a fresh TLS identity and
publishes its `wss://` URL and base64 DER certificate in the owner-only
bootstrap. That certificate is the Client's only trust anchor for the
reservation. With `--execution-public-url`, the Client validates the
operator's public ingress using WebPKI and, optionally, `--tls-ca-file`.
The operator must secure the trusted cleartext hop from that ingress to
the loopback listener. The Server authenticates the Client Hello and
ignores proxy headers. Both modes publish the bundle through the same
`create_new`, `0600` discipline as Unix.
One Session holds exactly one endpoint mode — a `Unix` listener or a
`WebSocket` listener, chosen at reservation — and a colliding Session
claim reuses the existing reservation or is refused, never allocates a
second Client.

`admit_websocket_stream_deadline` runs the identical Hello/admission
arbitration on an accepted TCP stream: credential first, then protocol
version and identity, one absolute deadline from upgrade through
admission, and one live connection at most. The App Server's remote accept loop is bounded
(at most four concurrent upgrades) and keeps accepting after failed or
malformed upgrades until drain. Each accepted peer
carries the absolute deadline armed at accept time, so a flood of
queued upgrades cannot outlive it, and the loop settles in-flight
workers under a bounded wait. A reconnect uses the existing binding and
never creates a replacement Client instance.
Revocation shuts a duplicated raw `TcpStream` — independent of any
writer lock — which wakes a blocked WebSocket read exactly the way the
Unix `shutdown` does. Close semantics mirror the Unix contract:
`receive_optional` returns `Ok(None)` only for a genuine RFC 6455
Close frame (the analogue of clean TCP EOF between records); a
reset-without-close or a truncated frame mid-message is a transport
error, never a silent end — the same distinction
`read_optional_record` draws for the Unix stream.

Teardown fences before it closes: the binding is marked `Revoked` and
its duplicated shutdown handle seized in one atomic step under the
binding lock — new admissions die at the gate immediately — and no
registry, slot, or residency lock is ever held while network work
runs. An idle stored connection then earns the orderly close: the
Close frame is sent and flushed under a bounded teardown budget and
the transport is shut down, never waiting for the peer's answering
Close. A busy exchange, an in-flight handshake, or a committed stream
not yet stored is interrupted by the seized handle's raw shutdown
instead, so a stuck peer can never stall a fence or a drain;
revocation and protocol failure close abruptly by the same design.

The operator entry point is `rustx app-server --execution-session
SESSION --execution-ws-listen 127.0.0.1:PORT --execution-bootstrap-out
/private/dir/bundle [--execution-public-url wss://…/execution/v8]`,
which conflicts with the local `--local-execution-*` composition flags
and never launches a Client: the Session's workspace is resolved only
from the trusted catalog, the reservation is published, and management
keeps serving while the accept loop waits. The operator privately
provisions the bundle (and any `--tls-ca-file`) to an independently
started `rustx-execution-client --binding-file … --workspace …`; no
Server environment or credential material is transmitted to it.

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
limit: u32, expected_revision, skip_hidden }` is answered by
`ClientReply::Directory { revision, entries, next_offset, skipped }`: at most `limit` (never more than
`MAX_DIRECTORY_PAGE_ENTRIES`, 128) `PageEntry` records carrying a
sorted name plus its own `EntryStamp`, a directory `FileStamp`
revision for the expected-revision check, and a continuation cursor
that is exactly `offset + entries.len() + skipped` while a full page is
consumed. Only Skill discovery may request hidden names be skipped. Those
names still count toward both the cursor and the physical listing budget. For both, an empty `relative_path` names the workspace root
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

`ResolveResourceEntry` is used by Workspace Skill discovery. The Client
resolves at most 32 links under the admitted directory using descriptor-relative
metadata and link reads. Broken, cyclic and external targets are treated as
missing. The reply carries the verified physical path relative to the
admitted Workspace, its entry stamp and whether resolution crossed a link.
Ordinary environment queries and file tools use their no-follow operations.

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
relative_path, expectation, data_base64 }` carries the strict
relative path, a typed `WriteExpectation` — `Current` retains whatever
the leaf currently is (the pre-authorization object an Agent holds
across its permission gate, never itself a permission to publish),
`Absent` is create-only, `Revision` requires the exact current stamp —
and the whole payload — base64, length checked before decode, decoded
bound `MAX_RAW_CHUNK_BYTES` — answered by `ClientReply::WritePrepared {
ticket, original }`. A payload over `MAX_RAW_CHUNK_BYTES` transfers as
`ClientRequest::BeginWrite { relative_path, expectation, total }`
followed by `WriteChunk { ticket, offset, data_base64 }` records at
deterministic absolute offsets — a foreign ticket, gap, or overrun is
`invalid` — up to `MAX_WRITE_BYTES` total. The ticket is a
`DecimalU64` the Client issues monotonically from `1` per admitted
connection, never reused and never issued past `u64::MAX`; at most
`MAX_PREPARATIONS_PER_CONNECTION` (8) preparations are live per
connection and their planned payloads share a `8 × MAX_WRITE_BYTES`
in-memory budget — a full map or budget is a stable `busy` reply.
`ClientRequest::CommitWrite { ticket }` and `ClientRequest::AbortWrite
{ ticket }` act only on the exact live ticket — a zero, foreign,
stale, or repeated ticket is a stable `invalid` and leaves the live
preparation untouched. A commit
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
target descriptor — plus, for a target below missing components, the
deepest existing ancestor and the missing interior names — and the
verified original stamp without mutating the named file. The planned
payload stays in Client memory under the ticket until the commit
grant: nothing is staged on disk before authorization, so preapproval
bytes are never enumerable through search or metadata. `stage` writes
the payload into a private `O_EXCL` `0600` sibling
`.rustx-write-{uuid}` under the retained parent — or, for a missing
subtree, builds the interior directories and leaf under a private
owned sibling staging directory, then `renameat` publishes the whole
subtree into the first missing name — and `StagedWrite::publish`
re-verifies the chain and the leaf stamp at the retained descriptors
before `linkat`/`renameat` crosses — the check and the publish are
honest pre-commit verifications, never a claimed CAS; external
writers can still rename between them. The private staging mode is a
best-effort same-euid privacy measure, not a sandbox: it does not
protect against an arbitrary malicious process running as the same
uid. Abort and failure cleanup removes only the owned private stage —
a foreign replacement under the staging name is never deleted. The Server side keeps
the same `LocalConnection` locked across prepare, the caller's
`authorize` gate run on the verified prepared `original`, and the
commit or abort dispatch — the path is never reparsed or reopened.
Once the first commit byte is attempted, a transport failure,
malformed reply, or mismatched ticket retires the stream and returns
an `io::Error` wrapping `rustx_execution_server::WriteOutcomeUnknown`
— the published state is unknowable and the operation is never
retried; an explicit well-formed `CommitUnknown` for the live ticket
maps to the same error *and* retires the connection: the publish may
have crossed, the ticket is consumed, and no later accounting from
that peer can be trusted. The `WriteOutcomeUnknown` marker downcasts through
`error.get_ref()`. A binding revoked after the prepare but before the
commit is dispatched gates the commit frame out entirely — the revoke
shuts the retained duplicate stream down, the idle Client exits on
EOF, and its dropped write state cleans the unpublished stage.

Inside the Client, the crossing itself arbitrates against the control
lane: `StagedWrite::publish_with_gate` runs every pre-crossing
verification first, then hands the one `linkat`/`renameat` (or subtree
`NOREPLACE` rename) closure to the caller's gate, which runs it under
the per-ticket arbitration lock. `Crossed` is recorded iff the syscall
physically succeeded — a `CancelMutation` that parks on that lock mid-
crossing receives the true post-syscall answer, a cancel that reached
the cell first means the crossing never runs, and a failed crossing is
never `AlreadyCrossed`. Revocation is checked under the same lock, so
a reported `Revoked` linearizes against every in-flight gate; a commit
parked between grant and gate is fenced, not silently crossed. On the
Server side, `LocalExecutionEndpoint::commit_prepared` arms the
invocation's cancellation listener the moment the commit frame hits
the wire and forwards `CancelMutation` on the paired control lane —
an independent physical path the data exchange cannot jam — retaining
the watcher until physical settlement.

### Conditional edits (implemented)

`ClientRequest::PrepareEdit { relative_path, expectation,
replacements }` reuses the same ticketed publish kernel as a write —
it is a first phase only, never a second mutation path. The Client
retains the exact named target with `WriteTarget::prepare` (the
expectation is `Revision` or `Current`: an edit always names an
existing original, so `Absent` is malformed), reads it back through
`WriteTarget::read_original_bounded` — the *retained descriptor*,
never the path reopened — capped at `MAX_EDIT_FILE_BYTES` (2 MiB) and
at the stamped size, applies the shared `rustx-text-edit` algorithm
(exact anchors first, then the per-edit NFKC fuzzy fallback, ranges
resolved in the original snapshot, BOM and dominant line ending
preserved), and holds the planned result bytes in memory under the
ticket — staging happens only under the commit grant, exactly like a
write.
`ClientReply::EditPrepared { ticket, original, published_size }`
reports the verified source stamp and the exact committed byte count;
`ClientReply::EditRefused { reason }` carries only the stable typed
`EditRefusal` — no path, no anchor text — and nothing is staged. The
wire request is bounded: at most `MAX_EDIT_REPLACEMENTS` (128)
replacements with at most `MAX_EDIT_REQUEST_BYTES` (64 KiB) aggregate
old+new bytes, and the fuzzy provenance vectors themselves are capped
(`MAX_FUZZY_PROJECTION_ENTRIES`), so a pathological source refuses
`TooLarge` rather than growing the mapping without bound.

`LocalConnection::edit_file(relative_path, expected_revision,
replacements, authorize)` runs the identical shape as `write_file`
under `Capability::ConditionalEdit`: one absolute deadline armed once
covers the prepare exchange, the authorization window, and the commit
or abort — it is never rearmed. The `original` stamp must echo the
exact expected revision, `published_size` must stay within the file
cap, and a typed `EditRefused` projects to a `RefusedEdit`-downcastable
error while the connection stays live; a refusal whose indices or
shape contradict the request retires it. Commit and abort are the
shared `CommitWrite`/`AbortWrite` frames, and a correctly-ticketed
`CommitUnknown` or any malformed postcommit reply retires the
connection with `WriteOutcomeUnknown` — a possibly-crossed publish is
never an ordinary retryable denial.

The native Write and Edit tools route their physical operations
through this path: the Agent binds a `BoundMutationTarget` during
preflight — the logical interpreted path plus the ticketed
`PreparedMutation` — classifies permission against the *logical*
workspace path the Client proved, and the executor commits or aborts
the exact ticket. A denied or cancelled preflight aborts the ticket
explicitly; a dropped owner arranges bounded cleanup without blocking
async code. No Server-local physical mutation remains for these
tools.

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
- The credential exchange runs under the same `transport::DeadlineStream`
  budget the Server enforces: one absolute deadline armed once on the
  established socket (`Instant::now() + LOCAL_HANDSHAKE_TIMEOUT`),
  covering the Hello write, the `HelloReply` read, admission
  verification, and `finish()` restoring blocking mode before the client
  enters its serving loop — never re-armed on a partial record.
- The Hello advertises `version: PROTOCOL_VERSION` and the fixed
  capability set `{Connection, ReadRange, DirectoryMetadata, TreeMutations,
  ConditionalWrite, ConditionalEdit, ReadLines, DocumentPages,
  BoundedSearch}` — the
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
  Read tool — `Search` answered through its bounded shared-universe
  traversal and streaming matcher (see *Bounded workspace search*) — and
  `PrepareWrite`/`CommitWrite`/`AbortWrite` answered through the
  `WriteTarget`/`StagedWrite` kernel by the per-connection `Writes`
  owner. EOF
  after a successful handshake never permits the credential file to be
  replayed: the binding is `Detached` and the Server answers
  `RecoveryRequired`.

### Bounded workspace search (implemented)

`ClientRequest::Search { relative_root, spec }` carries one typed
`SearchSpec` — `Glob { pattern, limit }` or `Grep { pattern, glob,
literal, ignore_case, context, limit }` — over `relative_root`, the
strict workspace-relative path the Server already confined. It is
answered by `ClientReply::SearchPage { body, matches, hit_limit,
byte_limit, short_lines }`: `body` is only the selected records (the
sorted root-relative paths for Glob, the `path:line:`/`path-line-`
rows for Grep) — the Server alone projects the model-facing notices
and the exact no-results text, and never parses display text back
into authority because Unix names can carry literal controls.

The Client's `search` module enumerates one shared file universe
descriptor-relative below the retained `PackageDirectory`: `names`
capped per directory before allocation, `entry_metadata` without
following, `descend` for real directories, hidden and ignored files
included, symlink and special entries skipped, a symlinked root
refused, a file root a whole universe for Grep only. The universe is
a point-in-time enumeration, not an atomic snapshot: paths sort
globally bytewise lexical; every retained file pins its enumeration
stamp and reopens only through `root.file` on the single shared
root-relative prefix joined to its own suffix — a path whose target
changed since enumeration is a conflict, and a searched file is
re-verified descriptor-wise and name-wise before its records
publish. Glob matches `globset` compiled
with `literal_separator`; Grep streams the verified descriptor
through `grep_searcher::Searcher` under an 8 MiB working buffer after
a whole-file `validate_utf8` pass — a file whose bytes break anywhere
contributes nothing. Both physical passes read through a
deadline-bounded reader so a single slow source cannot outrun the
operation deadline by more than one in-flight kernel call, which maps
to `Unavailable`. `hit_limit` is a proof: it is set only when a
further match was observed and excluded, and a line that cannot fit
the working buffer is the stable `SearchBudgetExceeded`, never an
allocation explosion. Every traversal budget — depth 64, 4096 names
per directory, 8 MiB retained path bytes charged on both owned forms
of each path, and 65,536 visited entries —
fails closed on the same typed code rather than returning a silent
partial universe.

The wire bounds are `MAX_SEARCH_PATTERN_BYTES` = 8 KiB per pattern or
file-filter glob, `MAX_SEARCH_CONTEXT_LINES` = 20, a `body` bounded by
`MAX_TEXT_BYTES` = 50 KiB, and the Grep line preview at 500 Unicode
scalars — all enforced on encode before the frame can grow. The
Server verifies the reply the same way it verifies a page: `body`
inside the text bound, `matches` within the requested limit, a zero
count pairing only with an empty body, `hit_limit` only at a proven
full limit, `short_lines` impossible on Glob — any deviation retires
the connection. The operation rides the same absolute exchange
deadline, data budget, and single writer permit as the page reads
through `SessionExecutionScope::search`; the native `glob`/`grep`
executors interpret the root lexically, compile patterns Server-side
for their own diagnostics, dispatch the typed spec, and project the
exact pre-migration truncation notices. There is no Server-local
traversal and no unbound fallback — an unadmitted Session answers
`Session execution unavailable`.

### Session-bound tree mutations

`TreeMutations` negotiates one `MutateTree` exchange for directory creation,
removal, and copy. The Server admits the caller and translates both copy
paths against the same exact Session Workspace. The Client refuses an
empty mutation target, interior dot components and no-follow traversal
violations. Recursive operations have a depth bound of 32 and a total
visit bound of 4,096 entries. Copies stream at most 256 MiB through the
confined-fs file publication kernel.

Every successful `mkdirat`, `unlinkat` and file publication crosses under
the Client's generation grant. Revocation fences later changes. Success
answers `TreeMutated`. An ordinary error means no named change crossed.
Partial changes or a failed post-publication check answer
`TreeOutcomeUnknown`. A lost or invalid receipt after admission also
returns `OutcomeUnknown` to the App Server caller and retires the pair.

### Session-bound file observers

With `FileWatches`, `Watch::Register` retains a file or directory descriptor
under the authenticated Session's confined Workspace. Registration has a
Server-created incarnation and checks the native source identity before
and after attaching. `Watch::Poll` returns a coalesced, source-relative
delta of at most 16 paths and 16 KiB of escaped path bytes. Each Client
binding holds at most 32 observers. An unavailable or changed identity
starts retirement. A poll that has yet to obtain native cleanup proof
returns `Retiring` for the exact watch identity. The Client retains its
slot while native cleanup is pending. A bound configuration observer
resyncs on the first retirement notice and periodically while coverage
remains incomplete. The final retired resync hint confirms slot release.

`StopWatch` uses the independent control lane. Its exact keyed receipt
distinguishes physical cleanup from an unknown result. A detached or
revoked Client stops its native observers before reporting clean exit.
The App Server retains connection-local notification ownership and fences
stale hints from a prior incarnation when a UI ID is reused.

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
| Search pattern or file-filter glob | `MAX_SEARCH_PATTERN_BYTES` = 8 KiB |
| Grep context lines | `MAX_SEARCH_CONTEXT_LINES` = 20 |
| Search traversal depth / visited entries | `MAX_SEARCH_DEPTH` = 64 / `MAX_SEARCH_ENTRIES` = 65,536 |
| Retained search path bytes | `MAX_SEARCH_PATH_BYTES` = 8 MiB |
| Grep searcher working buffer | `MAX_SEARCH_BUFFER_BYTES` = 8 MiB |
| Edit source and result bytes | `MAX_EDIT_FILE_BYTES` = 2 MiB |
| Edit request: replacements / aggregate bytes | `MAX_EDIT_REPLACEMENTS` = 128 / `MAX_EDIT_REQUEST_BYTES` = 64 KiB |

The encoded-frame bound is enforced on the final serialized bytes —
envelope, metadata, and escape expansion included — at the read/parse
boundary before allocation; raw chunk size never relaxes it. Data and
control records travel on separate bounded queues so cancellation, health,
and control progress are guaranteed even when data records saturate the
connection. The 5-second save deadline is an independent finite budget for
one optional save; it never extends the calling operation's remaining
deadline and never re-arms on slow writes.

## Operation and publication contracts

The Server and Client implement the following contracts on the negotiated
execution binding.

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
tombstones until the generation stops accepting their retries. The
4,096-identity generation limit includes active entries. A new entry
reserves its future tombstone and no transition frees that slot for a new
identity. Replays use the admitted fingerprint and cwd descriptor. A new instance
answers `Unknown` to old identities — no cross-instance exactly-once.
Background Bash additionally keeps prepare-then-accept: `Prepare`
registers the owner and allocates the unique managed file without
starting the process; `START` verifies acceptance evidence at the same
registration.

### Server → Client optional save

The Client negotiates `ManagedOutput` and admits at most 16 candidates,
17 MiB per file and 4,096 identities per binding. It appends validated,
contiguous new bytes. Seal fixes the raw length and SHA-256. Publish
makes the opaque locator readable only by the owning Conversation. Abort
and expiry hold the file and quota until unlink is proved. Graceful data
teardown and Revoke attempt cleanup. Offline cleanup after Client death
requires physical-domain attestation. See the Hosted reception contract
and [EXEC-03 evidence](exec-03-evidence.md).

Both ordinary Hosted replies and validated uploads use the same result
projection. The claimed business response fixes success or failure before
Save is attempted. Text items are joined with one newline on the bounded
preview and complete-output path, including empty items. A result exceeding
the preview limit retains the exact Client locator if publication succeeds,
or an unavailable continuation with the business result intact.

### Client-owned stdio MCP

The Server admits the configured source and freezes its credential resolution.
An `Open` names one operation and its canonical source identity on the
exact authenticated Session binding, then supplies the selected Client
program, Workspace-relative cwd, explicit environment and allowed protocol
versions. The parent validates the exact frozen SourceId grant of a
delegated Open and uses that grant for tools, resources and replay checks. The Client validates the cwd in
its Workspace, supervises the physical process and negotiates the physical MCP
session. The control directory for both supervision sockets is private to the
Client. Cancellation and the business START frame share the process gate.
The Server receives the negotiated peer metadata and next unused request ID,
then uses the existing ToolCall, Tasks and interaction owners. A process close
is acknowledged only after supervision proves physical settlement. A transport
failure after dispatch leaves the effect unknown and does not replay the call.
A Receive that was refused before dispatch retains its existing sequence and
offset. A delegated relay has a separate bounded reply budget to report such
refusals after the physical dispatch budget expires.

One effective capability generation admits up to eight Client-owned stdio
sources, including Managed Python. A binding retains at most sixteen physical
sessions and 4,096 stdio identities. This fits a complete candidate alongside
its predecessor and lets admitted old leases finish before physical close.
Retiring sessions and other callers still consume physical slots. A candidate
that exceeds remaining physical capacity cannot displace a last-good source.
A session has four queued outbound records of at most 512 KiB, one send in flight
and a five-second physical send bound. The Client limits each inbound JSON
record to 16 MiB before parsing. It retains one incoming queue entry, one
pending record and one paged record. The Server assembles at most one 16 MiB
record per receiving task, with a single item in the SDK input queue. Each
execution page carries at most 64 KiB with checked sequence and offset. The
wire encoding, parse allocations and transient serialization add to these
payload budgets. Negotiation has a 20-second physical deadline and admission
metadata is capped at 64 KiB. Startup failures can carry up to 64 KiB of
complete UTF-8 stderr from the correlated process after physical settlement.
Incomplete captures carry an explicit marker and no preview bytes. The
Server redacts the complete text with the frozen source credentials before
trimming or applying the final diagnostic bound. An absent binding provides
no stdio capability.
HTTP requests, their credentials and their network connection remain on the
Server.

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

App Server v81 includes `uploadScope` on each delivered `item/tool/call`.
A Host combines that scope with the callback request ID, thread, turn, tool
call and its own `UploadId` to address a candidate. Scope is specific to the
receiving connection and subscription. Resubscription produces a new scope
and invalidates the old one.

`Begin` reserves receiver state. `Chunk` validates and appends bounded
metadata, a digest and text preview. Identical replay of the last accepted
chunk returns `duplicate: true` and leaves both counters and deadlines
unchanged. `Abort` discards that receiver state. Each request gets a
correlated `host_upload_accepted` acknowledgement or a typed
`host_upload_rejected` error. Acknowledging a frame does not commit a
canonical ToolResult.

`End` verifies continuity, UTF-8, immutable descriptors, byte counts and
digest before competing with ordinary JSON-RPC replies and errors at the
original callback claim. The first eligible complete response wins. The
normal tool runtime still validates and commits the result. A complete
response that fails business validation keeps its original claim. As with
ordinary replies, multiple Hosts may already have performed external work.

The broker allows 32 live receive or handoff buffers, four active candidates
per Host connection and eight per conversation and per Session. Each pending callback has
up to eight recipient slots, including retired candidates. A new UploadId
cannot reopen a slot under the same subscription. Admission stops at the
first capacity limit. The receiver deadline is the earlier of 60 seconds
from Begin and the invocation's fixed hard deadline. The idle budget is
10 seconds from the last accepted new chunk. A frame for an expired
candidate retires it. New Begin admission reaps expired candidates before
checking active capacity. Detach, cancellation and callback resolution
release their receiver state. The native invocation continues to own its own
deadline and cancellation.

The model receives at most 16 KiB of text. The preview keeps native newlines
between items, including empty items. The digest and original-byte count use
the unchanged wire items.

Above the preview threshold the Server selects the Session's live
`ManagedOutput` binding once. Four queued fragments per writer and a
process-wide limit of sixteen writers bound the save transport. The save
deadline is the earlier of five seconds from binding selection and the
invocation hard deadline. It covers the prefix, continuation chunks, seal and
publish. A full
writer window abandons saving while the Host continues to send and verify
its result. The native cancellation signal can cut both timelines earlier.
Only the winner requests publication, and a confirmed reply allows Read and
Grep through `/.rustx-output/import/` for the owning Conversation. Without
confirmed publication the bounded preview carries `Unavailable`. The receiver
creates no Server files and commits no durable record per chunk.

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
freeze. Implemented contract values: `0x0001` is `inputText` (UTF-8
text, the only defined tag); `0x0000` is reserved and refused; every other
tag is refused as unknown. Item order is significant and preserved; a
zero-length item carries a header and no bytes. Independent upload-record
budgets (not the transport frame limit, not the management limit): at most
1024 items per result, 64 KiB raw per chunk, 16 MiB raw total per result. The
1 MiB fully encoded message limit still applies to every frame. Immutable result
metadata — content-item count, per-item type and declared length — is
fixed at `Begin`, verified again at `End`, and bounded. Chunk bytes
append to the incremental digest; re-derived metadata never mutates the
envelope.

## Production routing and settlement

- The authenticated WebSocket and Unix socket transports use the same
  Session, operation and recovery contracts. An independent control lane
  carries health, process and mutation cancellation, and Revoke. The
  Linux UI-detach test holds a data reply, occupies all four control slots
  and verifies Client settlement and exact output release. The Client
  file-publication test verifies the physical mutation cancellation cut.
  The Hosted saturation test holds a Save reply while receiving Host
  chunks and completing Client Health.
- `Binding::status()` observes the connection. Dispatch depends on the
  individual negotiated capability and the readiness of the named service.
  App Server's `command/exec` registers one
  operation per UI `processId` on the Session's admitted endpoint,
  drives it through the typed exchanges on the data lane, and cancels
  it on the control lane; there is no Server-local spawn, PTY, or
  drain path behind it. Process identity is the full
  `ProcessOperationKey` — server incarnation, Session, Client
  instance, generation, and operation — echoed on every reply
  including `Unknown`; `Running` is published only after the
  physical START witness inside the Client (the outer supervisor
  helper's own spawn is not business START), exit only after the
  real `ShellExited` event. A `Settled` snapshot finalizes the driver
  outcome and output accounting. Its `physical_settlement_proven` field
  attests that the operation never started or that its physical
  containment and reap are confirmed. An unconfirmed snapshot carries
  `Unknown` with no exit code. A physically settled operation can also
  have an `Unknown` business outcome. The Server projects the outcome,
  byte totals and truncation flags into `ExecResult`. Output is an ordered
  interleaved journal addressed by a global byte-boundary cursor with
  validated per-stream offsets and capped retained prefixes;
  `ReleaseOutput` at the exact final cursor frees the bulky buffers
  while a minimal tombstone retains identity and physical settlement
  evidence. Delegated cleanup requires that evidence even after a prior
  output release, or a confirmed binding-wide Revoke from the terminal
  owner. The parent owns the execution relay and uses its settlement
  before publishing a delegated child's quiescence and removing its
  runtime root. A child can attest its local drain directly if it seals
  its relay-side dispatch boundary before any request that might leave
  retained work crosses it. This preserves recovery after a parent
  crash during an unused child's preparation. The parent's data relay
  takes responsibility before sending a registration, mutation preparation,
  or commit. Validated receipts attach that responsibility to the operation
  or ticket, discharge it, or leave it unresolved. A lost or invalid
  receipt and `CommitUnknown` leave the relay dependent on confirmed
  binding settlement even if it never received a ticket. Input controls are
  admitted as `InputAccepted` and their physical outcome — `Pending`,
  `Applied`, `Failed`, `Rejected`, or `Unknown` — is correlated by
  `InputReceipt`. A pending poll keeps the receipt and a terminal poll
  takes it. The Server can explicitly abandon a pending receipt without
  withdrawing its already-admitted physical input. At most eight receipts
  may be outstanding per operation. The Client refuses admission before
  assigning the next sequence when that capacity is full. `ReleaseOutput`
  frees the journal while any unclaimed input receipts remain with the
  same operation. The Server owns each accepted receipt through a terminal
  claim, acknowledged abandonment, or binding teardown. If the final
  exchange fails, the receipt owner fences an Open binding. An existing
  terminal owner keeps the control lane to receive `Revoked`.
  A process exchange whose request may have reached the Client without a
  verified reply retires the binding. Public input reports `outcome_unknown`
  if this happens while waiting for `InputAccepted`, if the final receipt
  is unavailable, or if it reports `Failed`. Pre-dispatch refusals keep
  their original result. A collected `Failed` receipt releases its slot
  and may describe an already-applied prefix.
  The Client retains a fingerprint for each outstanding input receipt.
  Retransmitting the same operation and sequence with identical bytes and
  flags attaches to that receipt without enqueuing input. Conflicting
  content or an old sequence whose evidence was released is refused. The
  Client holds at most four unreleased operations per Session within the
  generation's shared live-and-retired identity budget.
  Native Agent Bash uses a separate `RegisterAgentBash` message and
  negotiates `AgentBash` on the same authenticated binding. Ordinary
  `Register` refuses a shell spec. The shell spec freezes the command,
  cwd, configured environment, timeout, Conversation owner, live
  capture choice, and raw stdout budget as one fingerprint. Timeout zero
  permits an adopted Bash job to keep its lifecycle; argv commands
  still require a physical timeout.
- Agent Bash registers its Client-owned capture before accepting a
  background job. A promoted invocation keeps the same operation and
  locator. Direct foreground capture allocates on the first combined
  preview overflow. Both allocation paths perform filesystem calls
  outside the operation registry lock, so cancellation and health
  can proceed during creation. Background preparation reserves the
  operation identity before allocating. Matching replays share its
  result and START is unavailable until registration commits. If
  cancellation or revocation wins, the counted allocation owner
  rolls back the file and retains any unconfirmed removal for cleanup.
  Capture produces separate 16 KiB stdout, stderr
  and combined previews. Each stream is decoded incrementally before
  multiplexing. Complete native captures have a 64 MiB file bound
  and at most 32 live/published files occupy a binding at once, so the
  per-binding disk data ceiling is 2 GiB. Storage failures distinguish
  a readable prefix from unavailable output. Releasing the process
  journal retains published capture until disposal or binding shutdown.
  Background acceptance marks the live locator as published before
  releasing the runner. Cancellation of an accepted Job first settles
  its preparation, including when the executor has not been constructed
  or its operation has not been polled. The START gate still refuses
  dispatch after cancellation. Client settlement reports complete empty
  output only after verifying the retained file. Unaccepted preparations
  can explicitly discard their capture. After the journal is released,
  the Server discards a capture that the Client reports as unavailable.
  A private capture also stays with the Server's cleanup owner when a
  failed query yields an unavailable result without a locator.
  A promotable capture stays readable until ownership is decided. If the
  execution finishes in the foreground, its registry uses the driver's
  capture owner to discard it. Both paths share one disposal claim,
  which is taken before requesting data admission. One claimant receives
  the discard receipt or transfers cleanup to binding teardown. Discard
  confirms that the retained descriptor has no directory links. If
  unlinking is needed, it first checks the named object's identity.
  A renamed capture retains its owner and quota. A replacement file is
  left alone even when the original capture can already be released.
- The absolute Bash continuation identifier is scoped to the owning
  Conversation and Client binding. Native Read and Grep send a typed
  query to that Client, and the Client opens its own retained capture.
  The locator does not authorize a workspace write or disclose the
  Client's physical scratch path. UI job-output tails are bounded by the
  same preview limit. This capture capability is independent of the
  optional `Save` contract.
- An authenticated connection has no implied configuration, MCP, or
  interpreter readiness. `WorkspacePythonStatus` reports only the Client's
  interpreter readiness under the separate `WorkspacePython` capability.
- Independent replacement after Client death or Server restart requires
  physical-domain evidence beyond same-instance proof. An unresolved old
  binding continues to exclude replacement admission.
- The App Server reserves one endpoint from the Session catalog under the
  residency owner. The explicitly local Agent entry and the App Server's
  local execution option launch the trusted sibling over that same paired
  Unix protocol. They pass an allowlist for runtime discovery and seal
  standard I/O.
  Normal shutdown confirms Client-side revocation, closes the data transport
  and reaps an owned sibling. A busy data exchange is interrupted by the
  binding fence.
- The trusted App Server owner can enroll a minimal Session and reserve its
  execution binding independently of Workspace configuration and runtime
  initialization. Use `--local-execution-workspace` without an existing
  Session ID to enroll and launch locally. Use `--execution-workspace`
  with `--execution-ws-listen` and `--execution-bootstrap-out` to enroll a
  remote Session. The enrolled Workspace must be a normalized absolute path
  under a configured environment root. The new Session is durable and
  listable if a later Client startup fails. When resuming that setup, use
  the existing Session ID. The credential stays in the private bootstrap.
- App Server file operations look up the exact `sessionId` without
  allocating a binding. Authorized browsing works after Client admission
  even when Workspace configuration is invalid and Agent runtime attach
  fails. A missing current binding reports `ExecutionUnavailable`.
- Session-authored inputs are optional `session_config` and
  `session_resources` fields on `thread/start`. `session/inputsWrite`
  uses the current Session settings revision to validate and persist either
  or both fields as one candidate. Omitted fields retain their values.
  Clearing a field reveals its lower sources. Activation follows the safe
  configuration application boundary. Session-authored Workflows require
  an explicit Session grant even when a lower definition had permission.
  The settings read projection discloses presence only. Each field can
  reference the saved input of an existing Session at its exact settings
  revision in the same Workspace. Creation and editing resolve those
  references as part of candidate capture, then validate the copied input
  against the target Session.

- Fresh App Server configuration capture for a reserved Session reads
  `rustx.toml` and Workspace `AGENTS.md` through the authenticated
  Client. The Server uses the proven
  root identity, a one-MiB whole-document limit, a five-second wire budget
  per capture, and the same stamped `ReadRange` chunks as other files. It
  verifies the document again before admission. The initial read alone
  can report absence. A disappearance mid-read is a conflict. Binding
  authority supplies the Workspace path without a Server-side Workspace
  probe for either document. Capture waits for data admission within the
  same wire budget and runs outside the configuration publication lock.
  Publication rechecks the current binding and source generations.
  The Server parses and authorizes the document, combines it with User
  policy, and applies native units. Bound runtime capture reads Workspace
  MCP declarations, Agent TOML, Workflow YAML, and Skill discovery inputs
  through that same Client. Agent and Workflow collections use stamped
  directory pages and bounded file reads. The Server rechecks each
  collection before publication and retains the adopted generation when a
  definition changes during capture. Cold loading and explicit Workspace
  reconciliation can succeed with no corresponding directory on the Server.
  After Server restart, the durable physical claim still controls Workspace
  authority. While it awaits settlement, cold attach and Session input
  validation report the source unavailable. Standalone submission applies
  the same physical fence before composing a live runtime. Cold history
  and settings reads continue to use Server-owned records.
  The Server still owns its User sources and the path-keyed Session-creation
  catalog. Bound Workspace observation uses Client descriptor watches
  with a separate 128-slot budget. The observed paths come from the
  configuration layout and Skill discovery. Coalesced hints and coverage
  loss cause a fresh capture fenced by the endpoint and application ticket.
  [EXEC-02](exec-02-evidence.md) records Client-owned Managed Python source
  discovery and same-instance reconnection.
- Agent tools resolve the binding through a non-owning
  `SessionExecutionScope`. The physical side performs bounded line,
  search and document work and returns the verified projection. Artifact
  validation, commit and model-facing references remain on the Server.
  Data exchanges share the endpoint's finite permits and absolute deadline.
  Malformed or lost replies retire the pair when framing or ownership
  becomes uncertain.

### Managed Python sources and stdio

Execution revision 4 adds a typed `PythonSource` data exchange and an
optional exact source key on the existing stdio MCP launch. Revision 3
peers are rejected before paired admission or business dispatch. The
Server can list logical names, capture an authored package, observe and
prepare its frozen identity, and release that capture. Package contents,
interpreter paths, uv output and the Store stay on Client. The capture
reply returns only a content digest and bounded authorization TOML. Missing
collections, observed invalid shapes and transport failures remain distinct.

Execution revision 5 adds `ReadSavedOutputRange`. It checks the same
Conversation and published locator as the line and Grep requests. Reusing
the verified range reply caps each fetch at 50 KiB. Native Read exposes
`byte_offset` for saved outputs and resumes at a complete UTF-8 boundary.
Revision 4 peers are refused before either lane admits business work.

Execution revision 6 returns the contained symlink paths verified during
resource resolution. Each reply carries at most 32 hops of up to 4,096
UTF-8 bytes each. An unresolved resource can also name the first absent or
obstructed component beneath a verified Workspace directory. Configuration
watches cover these paths, the intermediate links, and the resolved ignore-file
target.

Execution revision 7 adds source IDs to stdio MCP Open, typed watch
retirement, and bounded directory dependencies to Python capture receipts.
Client package coverage includes the package root and all observed nested
directories up to 128 paths and 16 KiB of escaped path bytes. If full
coverage cannot be registered, the configuration observer periodically
recaptures.

Execution revision 8 adds typed startup stderr evidence to stdio MCP.
Both lanes reject revision 7 before dispatch.

Each capture key includes the original Session binding identity. The Client
allows at most 128 names per authored scope, 256 retained capture identities,
four capture workers and four preparation workers per binding. A capture is
limited to 512 MiB and the binding to 1 GiB of frozen inputs. A stdio launch
can name a prepared identity and its negotiation versions. An exact managed
launch supplies no caller program, cwd, environment or argv. The Client runs
FastMCP in the bound Workspace cwd with its own prepared venv and sealed
source path. The same stdio connection and cleanup protocol handles the
process. A delegated child receives a source key for the parent's frozen
launch and its authorized tool names. The parent relay owns the physical
stdio session and validates the child launch bytes.

The Client environment Store is under its trusted `XDG_DATA_HOME` or
`HOME/.local/share` and disjoint from the Workspace and Client scratch. Its
fingerprint includes package identity, selected relative paths and exact
bytes, Client Python and uv path/version, pinned FastMCP and OS/architecture.
Permissions stay attached to the current candidate and outside the fingerprint.
The Store verifies the manifest, source bytes and launch interpreter before
reuse. Within one Client, overlapping builds of one fingerprint attach to
a stable coordinator. Independent Clients share only the on-disk result and
publish with an exclusive rename under the cancellation grant. Losing
staging directories are owned by the preparing Client. Published environments
outlive Session deletion. Revocation waits for the actual physical workers,
source cleanup and stdio settlement before confirming the binding.

Workflow tool catalogs can name the exact admitted Session for physical
listing. A Workspace catalog checks its authored target against that Session's
authorized logical Workspace before reading declarations or listing tools.
The authoring catalog can still show inert names without a binding.

### Root, Subagent and Workflow dispatch

The Root carries a non-owning `SessionExecutionScope` for the exact
Session. Native Workflow calls reuse the Root's scope. A separately staged
Subagent receives data and control Unix descriptors over its private startup
IPC, along with the exact Client admission and connection identity. The
parent retains the authenticated Client lanes and never provisions a second
binding or sends a binding credential to the child.

The parent checks the child's frozen native capabilities, Conversation and
operation identities and prepared mutation tickets before forwarding. Data
shares the Session's existing bounded admission and writer. The independent
control lane carries keyed cancellation even when data stalls. The
physical binding owner validates the mutation ticket, stamp and committed
byte count before relaying a successful write. Retirement waits within
one deadline for shared data capacity, then aborts child preparations and
settles or releases child process records before reporting cleanup. A lost physical commit or cleanup receipt retains
its unknown disposition and fences the binding. The Server still owns
canonical results and the order in which calls commit.

### Completion and drain

The Client's active process guard keeps physical settlement separate from
the command's outcome. Once a supervisor is spawned, the guard needs both
terminal containment and successful reap of that supervisor. If the worker
ends without that proof, the unresolved count survives the worker and any
output release. Revoke and clean shutdown require the count to reach zero.
This evidence is local to the running Client. See the
[EXEC-02 evidence ledger](exec-02-evidence.md) for death and recovery scope.

Discarded mutation preparations retain their ticket until a bounded
Abort can wait for the same data and writer admission as other Agent
requests. Abort is idempotent for an issued ticket whose preparation
has already settled. A mutation cancel waits for queue capacity in
the same absolute deadline as its keyed control receipt. If the
receipt is unknown and commit is still unresolved, the Server fences
the pair and preserves the authoritative data outcome. Neither path
reissues the commit. A pending data lane must pass an immediate socket
readiness check during pair commit, independently of the watchdog.

Application Server drain closes Host and Session dispatch admission in a
synchronous step that stores the settlement plan. Async drain takes that
plan. One absolute shutdown deadline starts before admission closes.
Independent Client pairs revoke concurrently outside the Host lock. At
pair commit the binding starts tracking physical settlement. A valid
Revoke receipt resolves it even if the transport subsequently detaches.
A reservation that never paired has no Client work to settle. Detachment
without a receipt leaves settlement unconfirmed. After sending Revoke,
the control worker waits for its receipt until the original command
deadline. Session deletion closes admission under the registry lock and
settles the binding outside that lock. It shares the outcome with Host
drain. An unconfirmed result prevents durable deletion and stays in the
process inventory. Streamed pipe stdin reports a close as applied
only once the physical input connection has been shut down. The argv
contract accepts empty positional arguments and rejects empty program
names and embedded NULs. An authoritative Register refusal ends cleanup
ownership. A terminal START receipt drives the output and release state,
including a cancelled operation with an empty journal. A failed cancel
checks the most recent terminal evidence before fencing. When terminal
settlement already owns a binding, a failed mutation cancel leaves the
control lane available for Revoke. The original commit receipt still
determines the mutation outcome. Private Bash
capture discard waits for bounded data admission and retries an explicit
unlink conflict once within the original deadline. If release is still
unconfirmed, the open binding is retired. Once terminal settlement closes
ordinary data admission, queued capture discard and prepared-ticket
cleanup transfer to it. A child relay with unfinished cleanup waits for
the same physical settlement result. The Client counts capture owners
from START admission through worker exit, including blocking allocation,
and from Discard admission through file release. It tries every idle
capture during teardown even if a different process lacks settlement
proof or a different capture could not be removed. Unconfirmed physical
work or capture removal still prevents a Revoke confirmation. Non-EOF
PTY read errors preserve output loss in the
terminal disposition.

### Workspace Python readiness

`PrepareWorkspacePython` schedules one Client-owned preparation. The
independent `WorkspacePythonStatus` reports `Unscheduled`, `Preparing`,
`Ready`, or `Unavailable`. Configuration and management do not wait for
preparation. The Client verifies the opened Workspace identity, resolves
`.python-version`, `pyproject.toml` and `.venv` in its own view, and runs uv
and interpreter probes with its process supervisor. Agent Shell START
applies the verified interpreter or the guarded unready environment. An
authorized Shell START may schedule preparation when the runtime loaded
before admission. Session revocation drains the physical worker. The
process-private uv discovery and fallback guard directories are created
with mode 0700. The Client removes them during orderly exit. The
local composer waits for and reaps that Client. UI argv commands have
an independent environment contract. Managed Python remains
under [#577](https://github.com/Caismis/rustX/issues/577).

### WorkspaceChanges observation

The Server associates a Workspace observation with one canonical Attempt. It
uses an independent operation ID with the same authenticated Session,
incarnation, instance and generation as the other Agent tool exchanges. The
Client advertises `WorkspaceChanges` and owns the physical observation.
`Begin` schedules the baseline while the model is running. The Server waits
for `BaselineReady` before dispatching tools and sends `Capture` with a
Workspace-relative file path before an approved native Write/Edit commit.

Git runs under the same process supervisor on the Client. It uses a private
index and object directory, literal path scopes for nested workspaces and a
restricted copy of the repository configuration. Each candidate stages
its Git objects and index privately. The binding retains up to 512 MiB
and 65,536 unique loose objects. Exceeding either limit discards the
candidate and preserves the previous index and existing observations.
A completed Finish owns its first Summary delivery. A completed Compare
owns its indexed Comparison delivery. The pending delivery protects the
record until a matching poll claims it or 60 seconds pass after completion.
A duplicate Compare for the same index joins its pending or ready result
within this window. It does not start another worker or renew the deadline.
A new Begin at capacity may reclaim delivered or expired history.
The quota covers retained data. A single serialized Git attempt may
temporarily use additional scratch before the quota check. A rename that crosses the
Workspace edge is observed only through its in-scope endpoint. Files outside
Git coverage use private, descriptor-relative captures. Summary and indexed
comparison requests are bounded by 500 returned paths and 896 KiB of encoded
reply content. There is one in-flight comparison per record and at most 32
records per Client binding. Completed or failed records can be retired to
make room for new observations. A retired record with an outstanding
physical worker keeps its slot until that worker settles.

`Finish` initiates the final physical snapshot. The Server checks the reply
and publishes the normalized summary under the active Attempt's cancellation
gate. The card is independent of canonical ToolResults and is excluded from
model input. A dropped or evicted Server record sends `Release` to cancel
remaining observation work and drop its comparison captures. If the
Server cannot confirm that Release on the same binding, it fences the
pair. Revocation waits for in-progress physical captures before it can
acknowledge settlement. Connection shutdown cancels the workers and
disposes of the binding's private Git index and objects.

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

Workspace verification commands from the repository root. Prepare matching
native test artifacts as described in the [test guide](../tests/README.md).
The native preparation wrapper takes a single Cargo package per invocation.

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --all-features --doc --locked
cargo build --workspace --bins --release --locked
pnpm --dir protocol/app-server check
pnpm --dir protocol/app-server typecheck
```

The independent builds use `cargo build -p rustx --bin rustx --release
--locked` and `cargo build -p rustx-execution-client --bin
rustx-execution-client --release --locked`. Compare their production
dependencies with `cargo tree -p rustx --edges normal --locked` and
`cargo tree -p rustx-execution-client --edges normal --locked`.
The repository's [test guide](../tests/README.md) describes the actual
FULL jobs and Merge verification.

The opt-in Linux mount script requires Bubblewrap. It runs the standalone
Client proof and the App Server composition, including native Read,
Write, Bash, Managed Python, stdio MCP, configuration and child routing.

```sh
test-support/prove_execution_mount_view.sh
```

Initial EXEC-01 evidence: canonical generation parsing; exact credential wire
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
the opt-in `filesystem_operations_use_the_private_mount_view` proof reads the
marker through a writable `bwrap` private bind mount of the reserved
Workspace. It then creates, copies, and removes entries in that view.
The Server's Workspace path remains empty. Its product-entrance
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
the decoy intact before and after. The same proof also runs a bound
foreground `command/exec` (`/bin/bash -c 'cat marker; printf client >
process-marker'`) through the Client's view: it exits `0`, its stdout is
the rewritten marker, the result file exists only in the physical
directory, and the Server's view keeps it absent. An unbound Session's
command is `ExecutionUnavailable`. The launcher gives the Client a
private `tmpfs` TMPDIR and `--dev /dev` (a plain `--ro-bind / /` leaves
`/dev` `nodev`, so the supervisor's `/dev/null` open fails), with the
staged `bash-supervisor` beside it. This proves the App Server's argv
`command/exec` path. The same script also runs the Agent tool-plane
fixture `background_live_output_is_readable_while_running_and_reused_at_settlement`
with an independent Linux mount view. The fixture uses the native tools
and the real Client to prove the file write, read, running Bash
continuation, and its complete terminal read.
The process layer adds its own evidence inside the Client crate
(`process::tests`, real spawns through the staged `bash-supervisor`
sibling): foreign-key refusal before lookup, `/bin/pwd` observed in a
non-root workspace subdirectory, authorized environment with the
runtime baseline winning, duplicate-register attach plus a second
`Start` producing exactly one marker, cancel-before-START writing
nothing, the barrier-gated START-hold race where a cancel parked
inside the runner's own transaction is classified by the real
crossing flag, streamed stdin delivered in order with conflicting
sequences refused, PTY write/resize/EOT, four-slot capacity refusing
a fifth until release, forged or early `ReleaseOutput` rejected, an
exact-cursor release answering later replays with the real settled
snapshot, five sequential released operations never exhausting
capacity, and generation revoke fencing a pending start. On the
product side, the App Server environment scenarios drive the same
operations through a real staged Client: PTY and piped commands,
stdin bytes and close, resize, cap/timeout, output-before-final
ordering, UI detach cancelling only the issuing connection's
operation, host drain cancelling commands before waiting for request
settlement, and `revocation_before_native_start` — the Server-side
dispatch hold plus the business process's own marker and PID proving
zero business spawn when revocation wins and exactly one when START
does. This is a T05 fragment (cancel before dispatch → zero
execution) and the first T29 implementation evidence (UI-direct
commands create no Attempt; disconnect cleans only the issuing
connection's operations); the process kernel's independent build and
the supervisor-helper staging are a further T24 fragment, with Agent
Server retains canonical Bash settlement and projection.
These cases establish initial T03/T24/T28 connection-singularity,
dependency-boundary and cross-instance rejection proofs, and T42
bounded-read evidence. The process-owned Session claim adds registry proof
that one claimant wins preparation, that a dropped waiter never strands
the claim, that two Sessions never share binding material, and that
delete and drain revoke. The three evidence ledgers above record the
product execution and recovery proofs and the verification results.
