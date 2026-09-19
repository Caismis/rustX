# Session archive export

`rustx-session-archive/v1` is a ZIP64/DEFLATE inspection archive produced by
`src/session_archive.rs`. `SessionArchiveProducer` is a native library owner,
independent of App Server, Web, TUI and Trace. This is neither a canonical log nor
an import, recovery or persistence format. SQLite uses schema 42 inherited from main; export changes no durable encoding.

## Logical files and authority

All JSON is UTF-8. JSONL entries contain one archive-v1 logical value per
line, ordered by the corresponding native immutable append coordinate. Mixed
historical/private native types cross explicit archive projections, not raw serde. Empty
histories have empty entries. Logical file schemas are version 1 in this archive;
the manifest also identifies the rustX package version and native durable schema.

| Entry | Authority |
| --- | --- |
| `manifest.json` | Description of the cut, native Session metadata, cwd, graph nodes and lineage, frontiers, schemas, artifacts and omissions |
| `sessions/<ConversationId>/journal.jsonl` | Projected Event Journal envelopes, including exact sequence and request-owned settled generation evidence; infrastructure diagnostics excluded |
| `sessions/<ConversationId>/messages.jsonl` | Accepted Message Ledger values, including messages no longer on the active Surface |
| `sessions/<ConversationId>/surface.jsonl` | Immutable Conversation Surface operations/revisions |
| `sessions/<ConversationId>/requests.jsonl` | Explicit `ArchiveRequestSnapshotV1` projection of immutable Request Snapshots |
| `sessions/<ConversationId>/generations.jsonl` | Request ID, source Journal sequence and existing `GenerationEvidence`; a convenience index of Journal-owned facts |
| `sessions/<ConversationId>/publication_audits.jsonl` | Settled noncanonical publication audit values |
| `sessions/<ConversationId>/inherited_responses.jsonl` | Immutable native completed-response lineage provenance from bootstrap, never reconstructed destination execution |
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
catalog. `local_runtime::session_ownership::SessionOwnership` traverses every
Session graph root and typed durable child ownership commit into one global
Conversation identity map. It validates identities, parent relationships and
unique Session ownership before selecting the requested Session's nodes and
Conversations. Deletion and child inspection consume this same native primitive;
archive does not rediscover descendants. Duplicate/cyclic/cross-Session ownership
fails with `CorruptAuthority` before any archive cut barrier is acquired, including
when both competing allocations contain readable bytes. Required unreadable
history fails closed. No agents are loaded, resumed, attached or synthesized.
The ownership freeze remains held throughout validation and cut capture.

Normal Subagent preparation first reserves its child identity using the same
waitable `ProductRoot::runtime_ownership_admission()`. The lock order is product
admission, Conversation allocation serialization, global uniqueness check, then
exclusive reservation-directory creation and physical incarnation allocation.
The allocator rechecks cancellation after waiting and releases admission on
return, before process spawn, composition or Ready. Reservation is private staging,
not Session membership. There is one reservation algorithm accepting an existing
ownership guard; it never reacquires the product lock. Management initialization
uses that algorithm under its existing fail-fast mutation guard.

Live Subagent membership linearizes at the durable `SubagentOwnershipCommitted`
event, not at allocation or the Ready handshake. `SubagentRegistry` acquires
`ProductRoot::runtime_ownership_admission()` before its registry/durability/lifecycle
commit mutexes. The uncontended shared admission is immediate; contention with
an ownership snapshot waits in Tokio's blocking pool, never on a Tokio worker
and never under those runtime mutexes. The admission spans the ownership event
and Running-record publication, then releases before capacity waiting, rollback
or driver handoff. The mailbox ownership-commit capability requires that guard.
Cancellation, capacity, runtime drain and durability are rechecked after admission.

Thus the ownership commit either precedes the snapshot (claim and child both
included), or follows cut capture/release (claim and child both excluded). A
transient read-only freeze delays a valid staged commit instead of returning a
start failure. Holding or slowly consuming the prepared ZIP holds no product
ownership admission or snapshot. Deletion preview also releases its snapshot
before managed writer retirement; neither snapshot reader waits on registry
commit locks.

For this finite membership, capture opens each database read-only, establishes a
rollback-journal SQLite SHARED barrier and reads its immutable append frontiers.
Previously acquired barriers remain held until all members have been captured.
It also captures each store's bounded artifact identity/settled-length inventory
(at most 256 native identities per Conversation), without reading artifact bodies.
Artifact writers acquire exclusive ownership on the existing `artifact_N.reserved`
file **before** creating/publishing `artifact_N.bin`, and retain it through their
whole lifetime. An archive reader first opens the already-published byte file,
then attempts shared reservation ownership. A reservation without bytes cannot
be locked by archive inspection; visible bytes with an active writer are refused.
Thus inspection can never steal writer admission in the publication window.
Shared reader admission proves the one-shot writer has ended. Native create-new
allocation prevents reopening settled identities for writing. Required artifacts that were
absent or still being written at capture fail subsequent preflight, rather than
being silently read at a later instant.

The linearization point is completion of these bounded frontier/settled-length
captures, immediately before the first database barrier is released: all prefixes,
lineage and settled bytes coexist at that instant. The full record-validation and
artifact-reference scan happens after releasing those barriers, using only the
immutable prefixes. No large history scan holds execution read barriers.
Each cut records Journal sequence, Ledger position, Surface revision, Request
Snapshot insertion frontier, publication-audit insertion frontier and immutable
bootstrap-row presence. Inherited response records come only from that bootstrap;
readers decode one response at a time using the native JSON row. Generation
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

App Server v11 added authenticated `session/exportPrepare { session_id }`, returning
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

## Safety and native authority audit

Archive v1 is a deliberate historical inspection contract. Adding fields to
`RequestSnapshot` or its invocation does not add archive fields: the private
`ArchiveRequestSnapshotV1` / invocation DTOs in `src/session_archive/projection.rs`
name each exported field. They include request/Attempt/Step/retry and provisional
Assistant identity, Surface revision, frozen prompt/System sections, model/protocol,
context/output limits, reasoning state/profile, historical Tool definitions,
capability/context generations, request context IDs, request-time upload projection,
carryover content/source/anchor and Agent Status facts.

Request options use the single closed native policy in `src/model/inspection.rs`,
shared with Trace without depending on Trace. `invocation.request_options` contains
only frequency_penalty, logit_bias, logprobs, min_p, n, presence_penalty,
repetition_penalty, response_format, seed, stop, temperature, top_k, top_logprobs and
top_p. `omitted_option_count` records how many other options were omitted, without
exporting their names or values. Unknown future opaque keys stay excluded.
Request continuation, raw request_params, invocation capabilities/adapter compat
configuration and process-local runtime_resource_revision are deliberately absent.

| Authority | v1 boundary and ownership rationale |
| --- | --- |
| Journal | Explicit envelope and exhaustive event classification. Pure identity/measurement/control facts use native encoding. Tool execution results explicitly retain Tool-owned content and structured facts while projecting their status and managed-output continuation. Mixed events project typed model failure/retry/timing evidence and runtime failure classes, excluding raw ModelError message/provider_code, unnormalized provider finish codes, runtime/executor diagnostic prose, workspace cleanup diagnostics and recovery comparison guards. New event variants require an explicit classification. |
| Ledger | User and Tool native values are accepted model-visible historical content, including authored JSON and Tool failure feedback. Assistant projection names identity/content and reasoning text; provider_state is never serialized. Other Assistant blocks contain authored text, Tool calls or artifact references. |
| Surface | Direct native encoding: only structural operations over canonical message identities. |
| Requests | Explicit v1 DTO and shared closed option allowlist described above. No durable snapshot or invocation flattening. |
| Publication audits | Direct native encoding: settled identities, timestamps and committed-for-release text/reasoning/refusal/Tool proposal content; no continuation or provider bindings. |
| Generations | Explicit index of request ID, Journal sequence and native GenerationEvidence, whose fields are provider-independent numeric offsets. |
| Inherited responses | Direct native CompletedResponseProvenance: lineage identities, timestamp, normalized token counts and timing measurements only; no provider binding or destination execution claim. |
| Manifest/lineage | Explicit manifest fields and archive metadata structs; native SessionSnapshot/SessionNode contain public identity, topology, authored name and timestamps only. No Session configuration is read. |
| Artifact metadata/bytes | Native File/Image/Tool artifact descriptors contain artifact identity and authored display metadata; bytes are Session-owned tool content. Paths never become identity. |

Journal `ToolExecutionStatus` values all cross one exhaustive v1 projection:
`success` and `timed_out` retain their kinds; `cancelled` retains its typed
`reason` and `phase`; `failed`, `denied` and `outcome_unknown` retain their distinct
kinds with `diagnostic_unavailable: "executor diagnostic excluded"`. Their native
`error`, `reason` and `detail` prose is absent. The helper covers native invocation
completion, Workflow candidate invocation, Workflow failure status, and the
nested status in ordinary `ToolExecutionCompleted.result`. Workflow failure's
outer diagnostic and native settlement-control diagnostics remain excluded.
Native Prepared/Started/Progress and closed lifecycle facts retain their existing
historical fields; both status and lifecycle matches require new variants to
receive an archive decision.

Journal `ToolExecutionCompleted.result.managed_output` also uses an exhaustive
v1 projection. `complete` preserves its exact locator; `partial` preserves its
exact locator and state; `unavailable` preserves its state. The latter two carry
`diagnostic_unavailable: "output-storage diagnostic excluded"` instead of native
output-storage diagnostic prose. No locator is normalized or redacted. Absent
continuations remain null. Duration, exit code, artifact references, truncation
flags/byte counts and workflow identity remain structured historical facts;
Tool-owned content remains intact.

A decoded durable-archive regression covers all three continuation states and
compares an original canonical Tool message containing a Partial continuation
against its archived value. Its diagnostic remains in canonical history while
the same diagnostic is absent from the Journal projection.

This restriction belongs to the Journal, not canonical Tool history. A canonical
Tool message retains its exact result, including model-visible failure feedback,
authored text/JSON and legitimate paths. Tool-owned Journal result content also
remains intact. No string, credential-pattern or path scanner is used. The decoded
ZIP regression exercises all three diagnostic-bearing statuses in all four event
families, and compares the canonical Tool message against its original value.

These safe direct native contracts remain subject to this ownership rule when
extended; an infrastructure/private field requires an archive projection before
it can be exposed. The archive does not scan or scrub content resembling a secret.
Canonical authored text, model reasoning, Tool results and workflow agent output
are preserved even if their content resembles authorization material. Recorded
execution/workspace paths remain intact. Arbitrary Tool-authored JSON is neither
configuration authority nor a source of guessed artifact references.

The producer never reads credential stores, process environment or configuration.
A decoded-ZIP regression proves that API/authorization/executor/unknown request
parameters, continuation, private reasoning state and provider diagnostics/codes
are absent, while authored secret-looking text and temperature remain present.

## Typed preparation failures

`SessionArchivePrepareError` contains only closed semantic reasons, never raw
OS/provider strings or implementation paths. Missing/unreadable descendants and
required unavailable/unsettled artifacts become v12
`archive_preparation_failed { reason: descendant_unavailable | artifact_unavailable }`.
Other reasons distinguish unavailable Conversation history, corrupt authority,
storage/cut failure and cancellation. Unknown Session and capacity conditions use
existing `unknown_session` and `request_capacity` protocol failures. The RPC message
is fixed native wording for the reason, e.g. “Cannot export complete Session: a
required descendant is missing or unreadable”. No successful descriptor is issued.

Web displays this message through the existing error channel and never invokes
the browser download callback on failure. TUI displays the same safe diagnostic;
preparation failure happens before local file creation. Neither consumer retries
preparation automatically. Native dispatch tests assert these exact errors against
generated fixtures consumed by both client tests, including failure coalescing,
no download/file creation and Session-ID-only RPC parameters.

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

## Base integration

PR #369 merged while these review repairs were in progress. Its main commit
`908399021b649ab7603498f43bdc5673f94c9352` already uses App Server v10,
SQLite 42 and catalog 12. The rebased archive PR therefore advances the complete
mandatory App Server vocabulary to v11, without a compatibility alias. This is
not an archive-format change: archive v1 remains unpublished and is corrected in
place. No SQLite/catalog schema increment is introduced by archive export.

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

## Final ownership/concurrency regressions

- `archive_reader_cannot_steal_writer_admission_at_publication` parks the real
  writer with synchronous channels before reservation ownership and again after
  byte publication but before `open_writer` returns. Direct archive reads and
  inventory scans run at both boundaries; writer admission/write succeeds and
  bytes become archiveable only after writer drop. No sleeps or retries.
- `archive_global_ownership_rejects_same_child_across_sessions_before_cut` creates
  competing child claims with readable allocations under both Sessions, then
  repeats with one missing allocation. Both exports fail `CorruptAuthority`.
- `archive_global_ownership_rejects_two_parents_and_cycles_before_cut` covers two
  parent claims and a cycle. The ownership regressions also assert deletion
  rejection and install a capture hook that must never be reached.

Existing decoded archive safety, typed preflight, live-write cut, large artifact,
deduplication, cancellation, bounded backpressure and corruption tests remain.
No protocol, archive or durable schema changes accompany these internal repairs.

The actual registry `prepare`/`commit` seam has deterministic regressions in
`src/runtime/subagent/registry/tests/archive_ownership.rs`: archive-first and
commit-first orderings decode the ZIP and relate every included parent ownership
claim to exactly one manifest child with the matching parent. A capture hook and
single explicit future poll establish contention without sleeps. The child is
staged using the existing real-process/control test seam; ownership events are
never forged. Additional cases change cancellation, capacity, drain and durability
while admission is waiting and prove rejection/rollback without publication.

`archive_wins_before_production_child_reservation` additionally crosses the real
`prepare -> allocate_child_runtime_root -> PhysicalChildRuntimeRoot::allocate ->
reserve_conversation_directory_under` path. The archive is parked after ownership
inspection; prepare reaches allocation and stays pending until capture releases
its snapshot. Only then does a controlled process peer substitute for child
staging. No directory, SQLite history or ownership event is fabricated by the
test. It proves reservation succeeds without publishing membership, admission is
released before staging, and ordinary commit/start/settlement still succeed.
`cancelled_production_reservation_wait_creates_no_child_allocation` proves a
cancellation received during that wait creates neither a reservation nor a process.

Ownership API audit: `runtime_ownership_admission` serves the two Subagent
transitions above. Fail-fast `ownership_mutation` remains for catalog publication,
management database initialization, explicit Conversation startup and explicit
retained-workspace disposal. SQLite's typed ownership-event guard also remains
fail-fast: Subagent membership publication is now protected by the outer runtime
admission; other terminal/workflow settlement events retain their existing policy.
That latter runtime use is a separate potential contention boundary, not changed
by this reservation repair. No blanket conversion of management mutations was made.
