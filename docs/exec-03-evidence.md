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
eight per conversation, and caps recorded recipient slots at eight per
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
truncated. Truncation uses `Unavailable` and carries no path. Physical
candidate Spill and publication depend on the Client capability still
listed in [EXEC-02's open gates](exec-02-evidence.md#open-gates).

## Deterministic coverage

| Required tests | Evidence and open coverage |
| --- | --- |
| T01, T04, T06 | The real WebSocket Host fixture submits a Root tool call with no Execution endpoint and asserts two model requests and one canonical ToolResult. Server HTTP placement, shared child routes and ordered parallel commits still need EXEC-03 integration evidence. |
| T13, T14, T15, T17, T18 | Session input/configuration/resource layering, safe activation and bootstrap remain open for EXEC-03. |
| T16 | Model reset and freeze still need cross-entry integration evidence. |
| T19, T20, T21, T30, T38 | MCP declaration cache, optional startup, Client stdio and foreground-path consolidation remain open. |
| T22, T40, T47 | Client Skill/AGENTS discovery and watch repair remain open. |
| T31, T32, T33, T45 | The real WebSocket fixture checks truncated output with an absent Client endpoint before and after the turn, and an `Unavailable` continuation. Client Spill, save interruption and artifact integration remain open. |
| T36, T50 | An earlier Begin leaves room for a second Host to win. Broker tests run verified End and ordinary replies/errors in both orders, and reject a stale subscription. Existing domain-invalid response handling consumes the claim. |
| T37, T46, T52, T55 | The WebSocket fixture submits a result above 1 MiB. The encoded framing regression measures the complete record inclusive of escaping. HTTP MCP JSON-body bounds, with-Client streaming and measured per-process occupancy remain open. |
| T51, T53 | Incomplete and digest-invalid End attempts leave the callback open for another eligible response. Disk failure and end-to-end cancellation at the save cut remain open. |
| T54, T56, T57 | Duplicate, stale and foreign steps, Abort and replacement are checked. Virtual time proves fixed total and idle expiry. Saturation checks another Host can claim and Abort returns capacity. An aggregate Session cap across conversations remains open. Delayed Client cleanup and generation-specific save tests require real Spill. |
| T71, T72, T73 | Credential rotation with frozen leases and stale candidates still needs EXEC-03 integration evidence. |

The pinned rmcp HTTP client bounds SSE events at 16 MiB and runs up to 16
ordinary POSTs per transport, with one separate control POST and a five-second
control request timeout. Its reqwest adapter
reads successful application/json replies with `response.json()` and
non-success replies with `response.text()` before rustX can inspect a
logical result. The current result projection's model-facing bounds apply
after that buffering. A bounded read before parsing needs to cover both
branches, including bodies that lack Content-Length. The existing rmcp
transport retains session and routing ownership.

## Local Linux verification

- `cargo fmt --all -- --check`, `cargo check --workspace --all-features
  --all-targets --locked`, and `cargo clippy --workspace --all-targets
  --all-features --locked -- -D warnings` passed.
- Focused library filters for callbacks, upload contract, wire, framing,
  legacy negotiation and request ID admission passed 50 tests. Six existing
  wire tests remain ignored.
- The Hosted conformance and process filters passed two WebSocket tests and
  one actual-process test with the provider emulator required. The contracts
  suite passed 31 tests.
- Generated App Server protocol `check` and `typecheck` passed. TUI and Web
  Console type checks and Web source provenance passed. TUI passed 1,365 tests
  with one skip. Web Console unit tests passed 5,018 tests with four skips.
- `git diff --check` passed.

Final FULL, Merge verification, per-process runtime measurements and native
macOS evidence remain open.
