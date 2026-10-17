# Issue 480 implementation map

Dependency-complete baseline: `029e50f45d6a24c1452b5d0768274fdd661cda6f`.
PR 482 merged at 2026-10-17T17:24:33Z; GitHub metadata and fetched ancestry
checked before creating the dedicated worktree. The full handoff SHA-256 is
`62477495cb90e7434d9261d71af0358bba1685cab22582425031dece185ce080`.

## Owners and linearization points (before behavior changes)

- `SqliteConversationStore::open` creates/configures writer-owned databases;
  validates existing development schemas before journal configuration.
- `open_existing` opens bound, existing stores read-only. The baseline rejects
  WAL headers before SQLite can create sidecars. Management must never initialize
  schema or repair application state. `recover_existing` separately holds the
  ProductController and allocation lifetime access and opens read-write.
- `ConnectionOwner` leases one connection, handing Tokio workers back during
  synchronous waits. The explicit SQLite busy handler follows SQLite's bounded
  five-second schedule. Preserve both mechanisms.
- Semantic write methods own transactions; successful COMMIT precedes staged
  Journal receipts and publication. P, U and C remain independently committed.
  Neither a storage PRAGMA nor archive preparation may merge those stages.
- Presentation uses cancellable scoped connection leases. SQL progress/busy
  policy is restored on guard release. Historical cuts remain native facts.
- SessionCatalog owns atomic temp-file/rename publication and distinguishes
  failures before visibility from errors after visibility. ProductRoot owns
  reservations and ownership admission, not ordinary execution writes.
- Archive takes ProductRoot ownership freeze, validates global membership,
  pins each allocation, then acquires sorted ConversationId SQLite barriers.
  Baseline SHARED reader barriers block rollback-journal writers. All finite
  frontiers and settled artifact inventories must coexist before the first
  barrier releases. Preflight and ZIP delivery read immutable prefixes only;
  they must not retain a live transaction or block ordinary writers.
- Fork selects lineage in one SQLite read transaction, materializes required
  immutable artifacts into a fresh allocation, initializes the destination,
  then publishes it through the catalog. It does not copy the main database.
- Artifact reservation locks protect writer admission and settlement. Seal
  verification binds the original witness, and export verifies the buffer it
  subsequently writes. Content-before-reference and final-header publication
  remain mandatory. Allocation pins protect deletion independently of SQL.
- Deletion owns lifecycle retirement and removes the allocation, including
  SQLite sidecars, only after its exclusive lifetime proof. Never unlink an
  active WAL or use main-file-only copies as a snapshot.

## Finite synchronization audit scope

`runtime/local_storage.rs`: directory publication and permanent identity
reservation marker/file/directory synchronization. `local_runtime/session.rs`:
catalog temp write, file sync, rename and ancestor directory sync.
`session/deletion.rs`: allocation retirement and directory entry removal.
`session/uploads.rs`: workspace resource publication and materialization.
`tools/artifacts/seal.rs`: content synchronization, manifest construction,
original-witness verification, manifest/directory sync and final header sync.

Audit each barrier independently. Buffer flushing, fallible writes/finish,
original identity verification and atomic visibility are not power-loss-only
barriers. Preserve observed errors and post-publication classification. The final
transition commit will record which machine-failure-only barriers are relaxed.

## Proposed WAL capture protocol to validate

Use SQLite's own writer exclusion (`BEGIN IMMEDIATE`) on separately validated
existing read-write archive connections, with no creation/conversion/repair.
Acquire in ConversationId order under the existing ownership freeze; all writers
must cross SQLite regardless of their runtime handle or process. Capture finite
immutable frontiers while exclusions overlap, rollback every barrier, then do
preflight and delivery with statement-scoped readers. This uses the existing
append-only archive authority rather than copying databases or pinning WAL
snapshots across backpressure. Failure/cancellation drops owned connections and
releases earlier acquisitions; deterministic tests must prove that cleanup.

This proposal is not yet implemented or validated. Benchmark BEFORE precedes it.
