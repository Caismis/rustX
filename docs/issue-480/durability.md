# WAL/NORMAL durability and ownership

File-backed conversation storage has one policy: SQLite WAL with connection-local
`synchronous=NORMAL`, foreign keys and the existing five-second bounded writer
busy handler. Derived observation-readiness writes also acquire IMMEDIATE
transactions before their read/modify phase, avoiding WAL snapshot-upgrade
failures that bypass busy waiting. Read-only transactions stay deferred.
In-memory SQLite retains its native MEMORY journal. No runtime
mode, configuration switch, schema migration or transaction batching is introduced.
The logical schema remains 69. The bundled SQLite is upgraded from 3.46.0 to
3.53.2 through rusqlite 0.40.2: the upstream WAL-reset fix is necessary for the
multi-connection writer/checkpoint topology. See [SQLite WAL documentation](https://www.sqlite.org/wal.html).
This dependency change is an explicit variable in the before/after comparison.

Successful commits are the publication and process-crash durability boundary
while the operating system and filesystem remain healthy. Process termination,
including SIGKILL, does not permit losing committed facts. Power loss, OS failure
or storage failure can lose recent committed transactions; there is no fixed
loss window or promise that every commit reached stable media. P, U and C retain
separate ordered transactions and receipts. The existing commit/rollback and
post-publication error classifications remain authoritative. No observed I/O,
lock, disk-full or permission failure is relabeled as success or retried blindly.

## Connections and sidecars

- New and writer-owned existing stores validate the development schema and bound
  identity before requesting WAL. The returned journal mode is checked, so a
  refused conversion cannot silently leave another production policy.
- Startup recovery opens without CREATE, validates schema and identity, then
  configures WAL/NORMAL under the ProductController and allocation lifetime pin.
  A compatible DELETE store can transition only on this writer-owned path or
  explicit writer open. Obsolete schemas remain rejected, without migration.
- Management opens existing WAL stores read-only, validates binding/schema and
  configures connection-local policy. It never initializes schema, converts the
  journal or repairs application state. SQLite may read/rebuild physical WAL/SHM
  state; read-only is not a promise that no filesystem sidecar can appear.
- Multi-query management head and transcript-page reads hold a scoped native
  read transaction. WAL writers may commit concurrently; the returned view stays
  within one snapshot. A gate proves a head read sees the older coherent state
  while another native writer commits, then sees the new state on its next read.
- Presentation shares the validated connection owner. Its scoped cancellation
  handler and immediate busy refusal restore the writer policy on release.
  Connection admission still yields Tokio workers as introduced in PR 482.
- Archive uses existing read-write connections solely for BEGIN IMMEDIATE writer
  exclusion; it refuses non-WAL storage without conversion. Fork reads SQLite
  transactions and creates destination stores through the standard writer path.

SQLite owns WAL/SHM files, reader snapshots and checkpoint state. No application
code deletes a live sidecar or treats the main database alone as a live snapshot.
The default 1,000-page automatic checkpoint remains; no checkpoint is forced on
each semantic write. Last connection close may checkpoint and remove sidecars.
A read-only connection that outlives the writer can leave WAL files after all
handles close; SQLite recovers them on reopen and a later writable close can
complete checkpoint/cleanup. This is not authority to unlink those files. The
paired benchmark accounts for that final writable close separately.
A reader can delay checkpointing
only for the lifetime of its native read transaction/statement; no archive read
transaction survives preflight or ZIP backpressure. Allocation deletion requires
exclusive lifetime ownership and removes the entire retired allocation after
connections/pins end, including any residual WAL/SHM files.

The native input certificate samples SQLite data_version after pinning its read
snapshot. A concurrent WAL commit does not invalidate the older snapshot's token
mid-read; the next transaction observes the changed token. The deterministic
regression validates both the old value/token and the next snapshot invalidation.

## Archive and Fork

The archive order is ProductRoot ownership freeze, allocation pins, then SQLite
BEGIN IMMEDIATE barriers sorted by ConversationId. All legitimate SQLite writers,
including other processes, encounter those writer locks. Each barrier remains
held while subsequent frontiers and settled artifact inventories are captured.
Their common instant is after the last finite capture and before the first
rollback. Existing immutable prefixes, original artifact witnesses, settlement
headers, reference closure and verified-buffer copying remain the authority.

Capture contention fails immediately with Storage; no waiting on a later writer
while retaining earlier locks. Cancellation is checked before/after acquisition
and while inventorying artifacts. Partial failure/cancellation drops connections,
rolling back prior barriers and releasing pins/freeze. Successful capture rolls
back all barriers and releases the ownership freeze before record preflight,
compression or delivery. Only allocation pins and immutable bounds survive.
Capture retains the native ResourceReadCut and a finite head identity. The existing
history compiler validates that captured checkpoint/Tail during preflight, even
if another writer activates a newer head after barriers release; it never substitutes
the newer live head. This factors existing validation without changing Source
representation or performing the optimization reserved for Issue 481.
Prepared cuts permit new writes and TRUNCATE checkpoints during gated ZIP output.
The completed archive contains its own bytes and survives later source deletion.

Fork retains its single native SQLite lineage transaction and copies selected
immutable facts/artifacts into a new allocation before catalog publication. It
neither copies the main database nor shares live source state. A regression pins
an older read snapshot, commits source content into WAL, captures lineage, commits
later source content, installs a Fork, then removes the source fixture allocation
and reads the exact independent destination. Existing artifact witness, closure
and process-death installation tests remain required.

## Finite non-SQLite synchronization audit

| Owner / operation | Decision and reason |
|---|---|
| ProductRoot creation (`local_storage::sync_directory`) | Sync only the owned root, retaining fallible directory error detection. Remove recursive ancestor fsyncs: their sole additional purpose was newly-created ancestor survival across machine failure. |
| Catalog rename and deletion confirmation (`session::sync_directory`) | Sync only the immediate owned catalog directory; remove ancestor traversal for the same reason. Keep temp-file write/sync, atomic rename and distinct error-after-visibility classification. |
| Reservation namespace/root barrier, marker and namespace directory | Retain. These are explicit identity admission failure boundaries, including deterministic error handling; removal without an equivalent fallible completion proof is unsupported. |
| Artifact create reservation file and directory (`artifacts.rs`) | Retain fallible identity publication. Allocation consumes an ID before failure, preventing reuse. |
| Artifact seal content sync, seal manifest sync, directory sync and final reservation-header sync (`artifacts/seal.rs`) | Retain. Surface observed errors before returning successful settlement; preserve content-before-reference, original witness validation and final-header publication. Buffer flush and same-buffer verified copying are untouched. |
| Upload child/parent directory creation, batch content publication, copy/materialization, cleanup and unlink directories (`session/uploads.rs`) | Retain bounded owned-file/directory completion barriers and their failure propagation. No recursive unrelated ancestor traversal is added. Removing them is not needed for this WAL change and lacks a replacement error-detection proof. |
| Allocation retirement/unlink parent (`session/deletion.rs`) | Retain fallible cleanup and post-publication error classification, with the catalog ancestor relaxation above. Lifetime exclusion still precedes removal. |

Retaining these bounded synchronization calls does not reinstate a product-wide
machine-failure guarantee. The finite change removes only barriers supported by
that waived guarantee, not fallible writes, finish, visibility or identity checks.
