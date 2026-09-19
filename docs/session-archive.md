# Session archive export

`rustx-session-archive/v1` is a ZIP64/DEFLATE inspection archive produced by
`src/session_archive.rs`. `SessionArchiveProducer` is a native library owner,
independent of App Server, Web, TUI and Trace. This is neither a canonical log nor
an import, recovery or persistence format. SQLite remains schema 41.

## Logical files and authority

All JSON is UTF-8. JSONL entries contain one existing native logical value per
line, ordered by the corresponding native immutable append coordinate. Empty
histories have empty entries. Logical file schemas are version 1 in this archive;
the manifest also identifies the rustX package version and native durable schema.

| Entry | Authority |
| --- | --- |
| `manifest.json` | Description of the cut, native Session metadata, cwd, graph nodes and lineage, frontiers, schemas, artifacts and omissions |
| `sessions/<ConversationId>/journal.jsonl` | Event Journal envelopes, including exact sequence and request-owned settled generation evidence |
| `sessions/<ConversationId>/messages.jsonl` | Accepted Message Ledger values, including messages no longer on the active Surface |
| `sessions/<ConversationId>/surface.jsonl` | Immutable Conversation Surface operations/revisions |
| `sessions/<ConversationId>/requests.jsonl` | Immutable Request Snapshot bodies |
| `sessions/<ConversationId>/generations.jsonl` | Request ID, source Journal sequence and existing `GenerationEvidence`; a convenience index of Journal-owned facts |
| `sessions/<ConversationId>/publication_audits.jsonl` | Settled noncanonical publication audit values |
| `artifacts/<ConversationId>/<ArtifactId>/content` | ArtifactStore bytes; one recorded display descriptor stays in the manifest and original references |

Artifact IDs are conversation-scoped in the native store (`artifact_1` can occur
in different Conversations). Deduplication therefore uses native ownership plus
ArtifactId, never a filesystem path. ZIP CRC32 provides per-entry accidental
corruption detection, not authenticity. The manifest does not become authority.
An absent generation terminal means it had not settled at the cut. A terminal
with null evidence records historically unavailable timing, never invented times.
Workspace uploads are mutable workspace resources, not immutable ArtifactStore
objects: their recorded references/request-time projection are preserved; the
manifest explicitly states that historical uploaded bytes are unavailable.

## Exact cut

Preparation acquires the existing native product ownership freeze and reads the
catalog and selected Session's graph nodes. Existing native typed child-ownership
traversal enumerates descendants before acquiring execution read barriers; no
agents are loaded, resumed, attached or synthesized. Ownership cannot change
under the freeze. Missing or ambiguous children fail the whole preparation.
Duplicate/cyclic ownership fails explicitly.

For this finite membership, capture opens each database read-only, establishes a
rollback-journal SQLite SHARED barrier and reads its immutable append frontiers.
Previously acquired barriers remain held until all members have been captured.
It also captures each store's bounded artifact identity/settled-length inventory
(at most 256 native identities per Conversation), without reading artifact bodies.
Artifact writers hold an exclusive native file lock for their lifetime; shared
reader admission proves their writer has ended. Native create-new allocation
prevents reopening settled identities for writing. Required artifacts that were
absent or still being written at capture fail subsequent preflight, rather than
being silently read at a later instant.

The linearization point is completion of these bounded frontier/settled-length
captures, immediately before the first database barrier is released: all prefixes,
lineage and settled bytes coexist at that instant. The full record-validation and
artifact-reference scan happens after releasing those barriers, using only the
immutable prefixes. No large history scan holds execution read barriers.
Each cut records Journal sequence, Ledger position, Surface revision, Request
Snapshot insertion frontier and publication-audit insertion frontier. Generation
membership is exactly the captured Journal prefix. Request start/completion
marker columns and mutable runtime/recovery tables are not exported.

All SQLite read barriers and the ownership freeze are released before archive
serialization or ZIP work. Subsequent reads select only immutable rows through
the captured frontiers. Artifact references are a function of those immutable
prefixes, so their finite domain cannot grow. Required artifact handles and
lengths are preflighted before the descriptor is returned. A slow consumer cannot
include later messages, requests, evidence or descendants. Allocation lifetime
pins prevent deletion while an archive owns the historical readers; these are
not execution locks. Capture can briefly delay durable commits while establishing
its cross-database barriers; it never holds them during compression/download.

## Streaming and failure

Native readers fetch one logical record at a time, ending each SQLite statement
before writing ZIP output. Artifact readers consume at most 64 KiB at a time.
The producer's bounded channel holds two chunks, each at most 64 KiB, with at most
one additional writer chunk waiting for capacity. ZIP metadata and manifest
identity metadata scale with included entries; serialization memory scales with
one native logical record, not the complete Session or archive. There is no full
ZIP allocation or server-side output file.

Dropping the consumer closes the bounded channel and cancels the producer.
Cancellation is checked between native pages/records/artifact chunks. The stream
has an explicit successful-completion marker: producer disappearance is an error.
Missing required files fail preflight; later I/O failures terminate the HTTP
response without its final chunk, so a truncated transfer cannot report success.

## Transport and clients

App Server v10 adds authenticated `session/exportPrepare { session_id }`, returning
`session_archive { download }`. No destination path exists in the request type.
The descriptor contains a deterministic filename, a 60-second lifetime and a
256-bit single-use capability at `/session-archive/<capability>`. It authorizes
only consumption of that prepared finite cut. Existing transport credentials,
browser bootstrap proofs and API keys never appear in the URL. Failed/expired/
consumed capabilities return HTTP 401. Responses disable caching and referrers.
At most 16 preparations, retained descriptors and active downloads share one native capacity bound. This is ephemeral preparation,
not a durable export job system.

Remote clients resolve the relative route against their selected App Server's
HTTP(S) origin (ws → http; wss → https). TLS reverse proxies must forward the
archive route as well as WebSocket upgrades. Owned stdio children expose a
loopback stream port in the same descriptor. Remote clients reject loopback-port
redirection and foreign paths. App Server works without any Web installation.

Web's **Session actions → Export** performs native preparation, coalesces rapid
concurrent gestures and hands an anchor URL to the browser download manager. It
never reads archive bytes into JS or creates a Blob. Preparation errors use the
existing alert surface; transfer failures belong to the browser download manager.
The capability originates in an authenticated App Server request, so browser
cookies or weakened Host authentication are unnecessary.

TUI **`/export [output-path]`** is in the normal command registry, autocomplete,
help and dispatcher. Its default is `./rustx-session-<SessionId>.zip`. `~/` resolves
against the TUI user's home. Paths (including paths containing spaces) remain on
the TUI machine, never in RPC parameters. Parent directories must exist. The TUI
creates the destination exclusively, awaits each incremental write, syncs/closes
on success and removes its partial file on failure/cancellation. Existing files
are never overwritten. A failed local cleanup may leave a partial file; no
success is reported for the failed export.

## Safety

The producer reads typed Session history, never configuration, credentials,
process environment, secret stores, synchronization state or Trace DTOs. It
explicitly excludes Request Snapshot continuation and Assistant reasoning
provider-private state, while retaining authored reasoning/text and legitimate
recorded paths. It does not scan or scrub content that resembles a secret.
Ordinary tool-owned JSON remains ordinary tool content; keys such as
`artifact_id` in arbitrary JSON do not become artifact references.

## Harness reference

Inspected read-only:
`deepseek-ai/deepseek-harness@ddefc45fbc7f8e46dd73185e68295696d1297887`, including
`packages/session-query/session-log-export/` archive/controller/route and tests,
Session persistence/format documentation, and Connection browser-auth/Fetch
routing. Adopted: native preparation, ordinary Session action, deterministic
filename, browser-native download, deduplicated gestures, chunked ZIP output,
pull-driven backpressure, cancellation and fail-loud missing descendants.

Intentional differences: rustX exposes its separate native authorities plus a
manifest; it does not adopt Harness's canonical Session log, live flush model,
or whole-root string serialization. rustX captures a cross-Conversation immutable
cut before streaming. Authenticated RPC mints one scoped capability instead of
independent HEAD/GET preparation, preserving the exact prepared cut and supporting
headless App Server/TUI consumers. There is no compression setting or new UI screen.

## Validation

Native archive fixtures live in `src/local_runtime/session/tests/archive_tests.rs`
and `src/session_archive.rs`. Client tests are `tui/test/session-export.test.ts`
and `web-console/test/session-export.test.ts`; browser/remote-TUI logical archive
comparison is `web-console/test/e2e/session-export.spec.ts`.

Use repository CI commands: `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo build --bins`,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features`;
`uv sync --frozen` and `uv run --frozen pytest` in `test-support/fake-provider`;
`pnpm generate`, `pnpm check`, `pnpm typecheck` in `protocol/app-server`;
`pnpm typecheck`, `pnpm test` in `tui`; `pnpm typecheck`, `pnpm test`,
`pnpm check:provenance`, `pnpm build`, `pnpm test:e2e` in `web-console`.
The E2E script owns the pinned Chromium container; do not replace it with host
Chromium. `git diff --check` must also pass.
