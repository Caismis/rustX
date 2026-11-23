# EXEC-03 implementation and verification

[#578](https://github.com/Caismis/rustX/issues/578) is open. This branch starts at
`5ff54d8327917bca2cdbc9647d7da197afe8beed` on
[`feat/576-execution-boundary`](https://github.com/Caismis/rustX/pull/580),
which includes the merged EXEC-02 PR #598. The issue's pinned research baselines
are rustX `d82d7f9b8d9dd4f5064dba8d377103c8c6049d28`, Codex
`ee344ca9458351a6ea102795fa73110ad3ede5dc`, and Pi
`22fc026b872b4a7df22ffa94e8fa00b873128054`.

## Hosted reception

The App Server admits the four Hosted result messages in v81. The callback
broker binds each candidate to the authenticated connection, its current
subscription, the native invocation and the Host-selected upload ID. The
incremental verifier validates End before using the existing response claim.
The native invocation owns cancellation and the canonical ToolResult.

The encoded message ceiling is 1 MiB. Begin permits up to 1,024 descriptors
and 16 MiB of declared raw text. Each independently validated chunk holds at
most 64 KiB of raw data. The broker holds up to 32 receive and verification
handoff buffers, caps active candidates at four per Host connection and
eight per conversation and per Session, and caps recipient slots at eight per
callback. The existing callback limit is 128 pending requests. Abort and
expiry replace the candidate with a bounded refusal; a new Begin sweeps
expired candidates before allocating.

For a large candidate, preview buffers retain 16 KiB of head and 8 KiB of
tail text at most, before projecting at most 16 KiB for the model. For a
small candidate, its ordered items retain at most 16 KiB of text in total
and up to 1,024 string entries. The verifier also holds up to 1,024
descriptors and lengths, the last chunk's metadata, one digest and pending
UTF-8 bytes. An owned permit covers the buffer through the response claim.
These are payload and object-count bounds. Allocator capacity, transient
base64/UTF-8 decoding and encoded message storage have separate costs.
The Server creates no upload files or per-chunk canonical history.

Hosted validation continues without a Client. The authoritative result
retains its success or failure and includes the original byte count when
truncated. A single lookup at the normalized preview crossing selects the
live Client. The same binding owns all subsequent save exchanges. The Client
checks the owner and chunk before appending, then advances the ledger only
after physical append. End competes independently with ordinary replies.
The winning callback can seal and publish its exact bytes. Losing callbacks
notify their worker to cancel and leave cleanup with the admitted storage
owner. Host claim never takes the operation cancellation gate recursively. The Client retains the file and slot until confirmed removal. An
unconfirmed publish receipt leaves disposal with the binding.

The physical store uses separate capture permits from Bash. Each Client has
16 slots, at most 17 MiB per file and 4,096 operation identities per binding.
Published locators remain owned by the binding for its lifetime. Ranged
logical Read and bounded Grep validate the exact Conversation and locator.
The files live under a private Client directory. The Server can retain up to
four fragments per writer across at most sixteen imports, plus a bounded
prefix and the fragment in flight. Offline cleanup after a killed Client
and same-instance reconnect remain open.

## Workspace instructions

For a bound Session, Workspace AGENTS.md uses the authenticated Client's
same descriptor-relative, stamped read as rustx.toml. The Server retains
the admitted document snapshot and verifies it again before applying the
resource candidate. A missing leaf is accepted only after the Client proves
its Workspace root. Invalid or unavailable instructions fail resource
adoption while independent configuration can still apply. The User
instructions are read by the Server and applied before Workspace
instructions. Workspace Skill discovery and automatic watch and repair
still require a Client-owned implementation.

## Deterministic coverage

| Required tests | Evidence and open coverage |
| --- | --- |
| T01, T04, T06 | The real WebSocket Host fixture submits a Root tool call with no Execution endpoint and asserts two model requests and one canonical ToolResult. The standalone process fixture serves HTTP MCP from Server loopback and verifies one tool commit and a last terminal. Shared child routes and ordered parallel commits still need EXEC-03 integration evidence. |
| T13, T14, T15, T17, T18 | Session input/configuration/resource layering, safe activation and bootstrap remain open for EXEC-03. |
| T16 | Model reset and freeze still need cross-entry integration evidence. |
| T19, T20, T21, T30, T38 | MCP declaration cache, optional startup, Client stdio and foreground-path consolidation remain open. |
| T22, T40, T47 | The isolated Workspace test covers Client-owned AGENTS.md on cold load and explicit reconciliation, BOM stripping and retention of the last valid instructions. Skill discovery, automatic watching and reconnect repair remain open. |
| T31, T32, T33, T45 | Hosted and HTTP fixtures cover absent Client, bounded model output, no locator and no Client startup. The wire-level Save fixture reads and searches the complete published text through an authenticated Client and checks cross-Conversation refusal. The Hosted fixture observes both physical candidates before End and the winning file after loser removal. Native Read retrieves the winning prefix, Native Grep finds the middle marker on a bounded line, and a different Conversation on the same binding is refused. The multichunk image fixture reads the independently delivered artifact after reaping the Client. The Hosted refusal fixture covers interruption during writes; generation change and cancellation still need composed evidence. |
| T36, T50 | An earlier Begin leaves room for a second Host to win. Broker tests run verified End and ordinary replies/errors in both orders, and reject a stale subscription. Existing domain-invalid response handling consumes the claim. |
| T37, T46, T52, T55 | The WebSocket fixture submits a result above 1 MiB with and without a Client. The encoded framing regression measures the complete record inclusive of escaping. The HTTP adapter caps response bytes before decoding, and the process fixture checks oversized responses do not replay the effectful call. Per-process occupancy remains open. |
| T51, T53 | Incomplete and digest-invalid End attempts leave the callback open for another eligible response. The real Client checks the retained file length on each append. A fixture changes that length after a confirmed prefix, then verifies the winning business result stays successful with no locator and the private file is removed. Native disk exhaustion and cancellation at the save cut remain open. |
| T54, T56, T57 | Duplicate, stale and foreign steps, Abort and replacement are checked. Virtual time proves fixed total and idle expiry. Saturation checks another Host can claim and Abort returns capacity. The Client unlink-failure regression checks quota remains occupied until a confirmed removal. Save behavior across a generation change and control progress under save saturation still need composed tests. |
| T71, T72, T73 | Credential rotation with frozen leases and stale candidates still needs EXEC-03 integration evidence. |

## HTTP MCP and managed output

The existing rmcp streamable HTTP transport stays on the Server. It bounds
SSE events at 16 MiB, runs up to 16 ordinary POSTs per transport, and retains
a separate control POST with a five-second timeout. The pinned reqwest
adapter independently caps successful JSON bodies at 16 MiB and error bodies
at 64 KiB. A declared excess is rejected before reading. A chunked excess is rejected before extending
the bounded accumulation. The decoder may hold
the response bytes alongside the parsed result and its allocations. Sixteen
ordinary requests could therefore buffer up to 256 MiB of JSON bodies per
transport in addition to decode and transport overhead. These bounds cover
retained body bytes.

For a valid HTTP business result, text blocks are scrubbed using the
credential snapshot of the connection that performed the call, then added in
order to the bounded preview and, after the spill threshold, to the same
Client writer as Hosted. The importer excludes image Base64, raw envelopes
and structured data. Tasks use the same translation when obtaining their
final result. A transport refusal after dispatch preserves the unknown
business outcome; the caller does not repeat the underlying call.

## Linux fixture measurements

The two-Host fixture sent 1,200,053 raw winning text bytes and persisted
1,200,056 normalized bytes. The concurrently staged loser owned 65,558
physical bytes before End. Its file was removed by the original binding.
The fixture asserts two model requests and one canonical ToolResult for
each case.

| Scenario | Host to Server WebSocket bytes | Server to Host WebSocket bytes | Save RPCs | Server to Client save bytes | Client to Server save bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| No Client | 1,625,151 | 414,229 | 0 | 0 | 0 |
| Successful save with staged loser | 1,713,887 | 409,406 | 28 | 1,701,162 | 12,182 |
| Confirmed staging followed by storage refusal | 1,625,151 | 414,754 | 5 | 177,030 | 1,695 |

These were Linux debug runs of
`three_clients_broadcast_first_response_error_and_invalid_payload_commit_once`
on `d47c095f75358d7bb60ff464c77dd1865f3cc1c4`.
The WebSocket counts include handshake, setup and notifications over the
three attachment sockets. The Unix measurement includes typed Save records
and their length prefixes. It excludes handshake, health and revocation.
The successful save includes two Begin, 23 Chunk, one Seal, one Publish and
one loser Abort. The refusal includes the failed append, the cleanup reply
and zero publication requests.

In a separate untraced run, sampling procfs every 10 ms recorded 204,564 KiB
peak RSS for the entire Host harness and its in-process Server, with 3.35 s
user and 0.15 s system CPU. The two Client children peaked at 22,784 and
22,836 KiB respectively. The full six-case fixture took 9.114 s including
startup and teardown. The Host harness memory includes test peers and
payload generation. Installed-product latency, Linux standalone Hosted peak
RSS, internal queue high-water and profiling across multiple concurrent
Sessions remain open.

The standalone HTTP process fixture reused the same measure for a 327,680-byte
result. Its App Server peaked at 188,688 KiB (0.79 s user and 0.03 s system
CPU), its Client at 22,868 KiB, and whole-fixture time was 1.982 s. The
oversized JSON case peaked at 185,724 KiB in App Server, required one remote
call and completed in 2.146 s with zero Client launches. These are individual
debug fixture observations. The HTTP service and provider run in the
separate fixture process. Startup, VCS and SQLite state are included.

## Local Linux verification

- Formatting, workspace check and strict workspace Clippy passed.
- The callback broker and upload request-ID filters, real WebSocket
  conformance, real HTTP import, no-Client HTTP overflow, and HTTP over-limit
  no-replay tests passed. The isolated mount proof and committed artifact
  read after Client exit passed.
- Existing focused regressions passed for model change resets, ordinary MCP
  lease retirement, credential changes invalidating draft preparation, and
  refusal of obsolete MCP background policy. These cover their existing
  owners. Cross-entry model integration and credential rotation through the
  full Session stack remain open.
- The real Execution Client saved and read normalized output through the
  wire. The execution boundary crates passed 412 tests with three ignored,
  including a confirmed unlink retry. The pinned rmcp fork passed two
  HTTP response-cap regressions.
- The generated protocol check and type check passed. Dev type check and
  44 tests passed. TUI type check and 1,365 tests passed with one skip.
  Web Console type check, build and 5,018 tests passed with four skips.
- `git diff --check` passed.

Final FULL and Merge verification, native macOS, whole-frame HTTP parser
occupancy and Browser acceptance remain open.
