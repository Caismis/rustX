# Crash-safe Session deletion

SESSION-DELETE-02 extends the exclusion authority in
[session-deletion-ownership.md](session-deletion-ownership.md). Deletion is native
Session control, never durable inbound, an Agent Tool, or canonical input.

## Authoritative state

Catalog schema **7** contains live `sessions`, pending `deletions`, a publication
`generation`, and the existing Session/node allocation high-water marks. There is
one document and one publication boundary. Previous development schemas are
rejected; there is no migration or compatibility reader.

```text
live Session
    -> atomic removal of live row + installation of frozen cleanup record
    -> confirmed durable logical commit
    -> idempotent physical cleanup of the frozen workset
    -> durable removal of the cleanup record
    -> absent
```

`DeletionRecord` contains the Session identity, #254 semantic ownership revision,
and exact node `(SessionNodeId, ConversationId)` / child
`(ConversationId, parent ConversationId)` scopes. Trusted ProductRoot and these
identities derive private allocations and child routing sockets. Independent
fork/clone provenance is not ownership. No workspace paths or conversation history
are copied into the record.

Every persisted deletion record is pending cleanup authority. There is no
`DeletionPhase`, completed record, deletion archive, or tombstone collection.
Cleanup progress is replay of the finite idempotent workset, not a durable worker
lease or per-item cursor. Completed worksets do not accumulate in `catalog.json`.
Pre-commit rejection and durability uncertainty are operation outcomes, not extra
persisted phases. A crash selects one complete catalog document atomically.

## Publication, visibility, and durability

Preview acquires a finite #254 snapshot and releases every guard before returning.
Execute acquires a fresh preflight and compares the native `target_revision`.
Changed semantic ownership is `Stale`; current-Session, access and workspace
collisions remain typed pre-commit blockers. An existing pending record returns
pending state without a second discovery or snapshot.

Every catalog mutation takes root ownership exclusion first (shared for ordinary
mutations, the existing exclusive #254 preflight for deletion), then a short
exclusive publication lock on the stable `sessions` directory inode. Under that
lock it compares the actual persisted generation against the writer's observed
generation and the planned document's generation. A mismatch rejects the write
before replacement. Successful publication advances the generation. This is a
generic compare-and-publish boundary: stale clones and stale plans cannot erase
newer names, graph changes, live membership, or pending deletions. The catalog
file itself is replaced and therefore is not the lock inode. These locks provide
exclusion, not durability; neither is held across recursive cleanup.

The logical visibility/linearization point is rename of the fsynced temporary
catalog over `catalog.json`. That single document removes live membership and
installs the complete frozen workset. Pre-rename failure leaves the old document
authoritative and performs no cleanup. Post-rename failure updates in-memory
visibility and returns `CommittedDurabilityUncertain`, never an ordinary
pre-commit error.

The durability boundary is successful fsync of the parent directory and its
ancestry after file fsync and rename. The ancestry barriers also persist naming
entries created during startup. Only confirmed durable logical commit grants an
owned `CleanupWork`. Uncertain logical publication starts no cleanup. Recovery
republishes the same frozen authority with file/directory barriers before granting
cleanup authority; it never scans ownership again.

After every frozen removal and its filesystem barrier succeeds, finalization
publishes a document **without** the pending record. A pre-rename finalization
failure retains the exact pending record for retry. An uncertain final rename
returns `CommittedDurabilityUncertain`; it may have made absence visible, but does
not return `Deleted`. Restart sees either the same pending work (safe to replay)
or absence. Explicit recovery of absence confirms file/directory persistence
without reconstructing a workset. `Deleted` is emitted by confirmed successful
finalization; subsequent deletion/recovery returns `NotFound`. A delayed duplicate
worker cannot recreate a record or regress absence to pending.

## Allocation and access are separate from cleanup history

Native Session and node identities use `next_session_ordinal` and
`next_node_ordinal`. Publication advances each high-water mark past the actual
chosen ordinal, including gaps skipped for orphan allocations. Deletion never
rewinds these counters. A new Session uses `session-N` / `conversation-N`; tree
nodes use `node-M` / `conversation-node-M`. Native child IDs derive recursively
from their parent Conversation and its durable subagent ordinal allocator.
Published identities therefore cannot be allocated again after deletion.

The private prepared-Session publication path accepts only native identities at
or above the current allocation marks with matching Conversation identity. Tree
publication similarly rejects an earlier node ordinal. There is no public import
API accepting arbitrary Session IDs and no identity reservation database.

ConversationAccess acquires the existing allocation lock before checking catalog
admission. Pending frozen scopes reject access immediately. After completion,
native Session/Conversation identities below the high-water mark must still have
a live catalog owner; retired native root domains also reject their derived child
IDs. Residue, a stale projection, or a copied allocation is not live authority.
The check does not rediscover the deletion graph. New child creation still uses
the parent's native ordinal authority; unsupported arbitrary IDs are not a
promise of permanent cross-allocation reservation.

## Cleanup and recovery

The supervisor releases its catalog mutex and preflight drops root/target guards
before dispatching `CleanupWork::run` through `spawn_blocking`. Work owns its
ProductRoot, frozen record and controller lifetime, not a catalog reference. It
takes exclusive access to one exact frozen private allocation at a time. Missing
units are idempotent success; each surviving parent receives a persistence barrier,
including on retry after a previously visible unlink. Child socket removal and its
root-directory barrier precede completion too. Confinement failures retain pending
state, and recursive cleanup does not follow symlinks.

Empty Session container directories may remain; they contain no Conversation
source of truth and grant no identity. Projects, workspaces, environments, caches,
configuration and credentials are outside cleanup. Retained worktrees require
explicit disposal and continue to block preflight.

`LocalSessionProduct::compose` recovers pending records after controller admission
and before ordinary live-store recovery or runtime composition.
`session_delete_recover` is the explicit asynchronous retry boundary. Neither
recovery path activates a deleted Conversation, processes Pending Inbound, restores
agents, calls a model, or initializes semantic services. Cancellation can leave a
worker running or an unfinalized record; both converge at the next recovery
boundary. No queue, timer, distributed worker, or retention policy is introduced.

## Bounded Runtime Client contract

Protocol **28** adds `session_delete_preview`, `session_delete`
(`session_id` + `expected_target_revision` only), and `session_delete_recover`.
`runtime_client::session_deletion` owns independent DTOs:
`RuntimeClientSessionDeletePreview`, `RuntimeClientSessionDeletionBlocker`, and
`RuntimeClientSessionDeletionResult`. The Session-control owner explicitly maps
native outcomes in `supervisor::project_session_deletion`.

The frozen deletion workset is recovery authority, not public control-plane data.
Native deletion types and raw supervisor operations are crate-private. Public
Session deletion control is provided by `RuntimeClientSessionControl`.

Only a successful fresh preview supplies an externally usable `target_revision`.
A stale execution response invalidates the old confirmation but never supplies
the replacement execution token. It contains only `status: stale` and
`session_id`. The caller must obtain a new `session_delete_preview`, present its
updated scope summary for confirmation, and then execute with that revision.
Repeated stale executions cannot obtain the replacement token. Internal execute
still recomputes and compares the current semantic revision.

Preview carries identity, target revision, display name (at most 256 Unicode scalar
values), and node/Conversation/child counts. Workspace blockers expose a count;
other blockers expose a discriminant. Pending and uncertain outcomes carry only
Session identity and status. The wire contains no frozen records, scopes, child
lists, private paths, or storage diagnostics. Its size is independent of graph
size except for decimal count widths. Internal diagnostics remain native; ordinary
pre-commit failures use a bounded protocol error without private storage paths.

| Status | Meaning |
| --- | --- |
| `preview` | Bounded confirmation metadata; no guards |
| `stale` | Ownership changed; obtain a new preview |
| `blocked` | Current Session, in use, workspace, or invalid ownership |
| `committed_cleanup_pending` | Logically unavailable; frozen cleanup awaits retry |
| `committed_durability_uncertain` | Publication durability unproven; recover before further cleanup/final success |
| `deleted` | This finalization confirmed cleanup and durable record removal |
| `not_found` | No live Session or pending record, including completed deletion |

The existing TypeScript Runtime Client mirror in `tui/src/protocol/types.ts`
mirrors these protocol-28 DTOs. There is no separate deletion SDK. Shared Rust/TypeScript
fixtures validate the wire contract, not deletion persistence. Interactive deletion
UX (#257) and retention/lifecycle policy remain outside this change.

## Deterministic evidence

`session/tests/deletion_tests/lifecycle.rs` covers publication fault windows,
exact pending authority across cleanup failure/restart, persisted record removal,
repeated absent recovery, sequential deletions without history growth, native
allocation monotonicity, generic stale metadata/plan writes, and stale resurrection
after record removal. Large ownership worksets exercise the real protocol mapper
and prove bounded counts, names, blockers and results.

The process-death regression kills/reaps a real subprocess at flushed pipe gates
immediately after durable commit and after one durable cleanup item. Restart
recovers the same scopes even with unrelated storage deliberately corrupted.
Another test holds conflicting root exclusion throughout cleanup; a gated real
worker test acquires the supervisor mutex with `try_lock` before releasing the
worker. No sleeps establish these synchronization facts. Existing #254 blockers,
stale preview, fork/clone survivors and provider-backed active-A/deleted-B
conformance remain part of validation.

## TUI entry point

`/resume` exposes Ctrl+D for preview-first confirmed historical deletion. Cancel
is the default focused action. The TUI submits only the captured SessionId and
native target revision, then reconciles paginated visibility from the native
boundary. Cleanup retry uses `session_delete_recover`; no client storage or
model-visible operation participates. See [TUI deletion help](../tui/README.md#delete-historical-sessions-inside-resume).
