# Crash-safe Session deletion

SESSION-DELETE-02 extends the ownership authority in
[session-deletion-ownership.md](session-deletion-ownership.md). It is Session
control, never durable inbound, an Agent Tool, or canonical conversation input.

## Authority and state

Catalog schema **6** has one document containing live `sessions` and `deletions`.
A deletion record contains SessionId, the #254 semantic SHA-256 ownership revision,
and sorted semantic scopes: node `(SessionNodeId, ConversationId)` or child
`(ConversationId, parent ConversationId)`. No arbitrary absolute paths, messages,
workspace paths, or execution history are copied. ProductRoot derives each exact
private allocation and child routing socket. Independent fork/clone provenance is not ownership.

The persisted phase is `cleanup_pending` or `deleted`. Absence from `deletions`
and presence in `sessions` means live. Pre-commit rejection is an operation result,
not persisted state. Durability uncertainty is a publication outcome, not a second
on-disk flag: after a crash only the atomically selected document is authoritative.
Cleanup-in-progress retains `cleanup_pending`; there is no durable worker lease.
The frozen record is the cleanup authority, and the private allocation locks
exclude conflicting cleanup/access. No root freeze or catalog mutex spans removal.

```text
live --fresh preflight + matching revision--> atomic catalog replacement
  |                                           |
  +-- rejected / pre-rename failure: live      +-- durability unproven: no cleanup
                                              |
                                              +-- parent fsync succeeds
                                                     |
                                               cleanup_pending
                                                     |
                                          idempotent frozen-scope removal
                                                     |
                                       all removal directory barriers succeed
                                                     |
                                     atomic final publication + parent fsync
                                                     |
                                                   deleted
```

## Commit points and uncertainty

`SessionCatalog::commit_delete` reacquires #254 preflight. Preview has already
returned a finite snapshot and released all guards; no confirmation holds a lock.
Execute compares the native semantic revision before accepting workspace state.
Target membership or workspace authority changes return `stale`. Live access and
the current Session return typed blockers. Existing records return existing state
without discovering another target.

The logical **visibility** point is rename of the fsynced temporary catalog over
`catalog.json`. One document removes the live row and adds the complete frozen
record. The existing atomic writer distinguishes errors before rename from errors
after rename. `commit_under_ownership` updates in-memory visibility even when the
latter occurs. It uses the already-held #254 exclusive ownership snapshot;
ordinary catalog commits acquire their usual shared ownership mutation guard.

The logical **durability** boundary is successful fsync of the catalog's parent
directory and its ancestry following the temporary-file fsync and rename. The
ancestry barriers also persist naming entries for directories created during this
startup; an immediate-parent-only barrier would not prove their reachability. Only success mints an
owned `CleanupWork`. `CommittedDurabilityUncertain` means visibility changed but
its durability barrier failed: this attempt performs **no physical cleanup**.
Recovery republishes the existing record, including file and directory barriers,
before minting cleanup authority. A recovery publication error conservatively
returns uncertainty and never starts removal. A repeated lookup of an existing
record confirms file/directory persistence before reporting its phase.

Finalization also distinguishes pre-rename failure (cleanup pending) from
post-rename uncertainty (possibly visible `deleted`, but no final success yet).
Only confirmed final persistence returns `Deleted`. Repeated terminal lookup or
explicit recovery confirms the barrier before reporting final success.

## Access and monotonic identities

ConversationAccess first obtains its existing shared allocation lock, then checks
the authoritative catalog for a deleted scope. If access wins, fresh preflight
cannot obtain exclusive target authority. If commit wins, admission rejects the
residual allocation. This requires no global ownership freeze for unrelated access.
Native startup and explicit child allocation also check identity before creating
private state. Direct child inspection uses the same ConversationAccess admission.
Read-only SQLite management never creates missing stores.

Completed records retain the Session/node/Conversation identities and frozen scope,
without history. These are identity reservations inside the existing catalog,
not a second tombstone database or a history archive. Both normal allocation and
explicit prepared identity publication reject reuse. Catalog validation rejects
live/deleted collisions. Every catalog publication checks that existing deletion
records survive unchanged except for the monotonic pending-to-deleted transition;
a stale in-memory catalog cannot erase a tombstone or revive a session.

Physical allocation lock inodes may be removed only after commit. Unlike live
allocations, their identities can never be admitted again, so removal cannot split
normal access into a newly created lock domain. Permanent product/controller lock
inodes and workspace storage are not cleanup targets.

## Cleanup and recovery

The supervisor drops its catalog mutex and the preflight drops root/target guards
before dispatching `CleanupWork::run` through Tokio `spawn_blocking`. The work owns
its ProductRoot, frozen record and retained controller lifetime, not a catalog
reference. It takes exclusive allocation access for each exact frozen private
unit. Child routing sockets derive from the frozen child ID; their root-directory
removal barrier also precedes completion. Recursive removal does not follow symlinks; confinement failures retain
pending state. Missing units count as success. Each removed unit's surviving parent
is fsynced, including on retries where removal was already visible.

No per-item cursor is needed: replaying this finite idempotent plan is the progress
model. A crash after any subset simply repeats the same scopes. Empty Session
container directories may remain; they grant no identity and contain no owned
Conversation data. Workspace disposal, project sources, environments, caches,
config and credentials are outside the plan.

`LocalSessionProduct::compose` recovers pending deletions after controller admission
and before ordinary live-store recovery or runtime composition. It does not recover
or activate the deleted Conversation. Startup is single-owner and has no supervisor
mutex yet. `session_delete_recover` is the explicit asynchronous retry boundary.
There is no timer, queue, retention sweep, or distributed worker. Cleanup failures
leave committed state for the next boundary. Cancelling a request can leave a
blocking cleanup running or an unfinalized record; either is safely recoverable.

## Public protocol

Runtime Client protocol **28** adds `session_delete_preview`, `session_delete`
(SessionId + expected_target_revision only), and `session_delete_recover`.
All return `session_deletion` with a discriminated native result:

| Status | Meaning |
| --- | --- |
| `preview` | Finite semantic scopes, target revision and display name; no guards |
| `stale` | Ownership changed; obtain a new preview |
| `blocked` | Current Session, in use, unresolved workspace, or invalid ownership |
| `not_found` | No live Session or retained deletion identity |
| `committed_cleanup_pending` | Logically unavailable; exact frozen work awaits retry |
| `committed_durability_uncertain` | Publication visible, barrier not proven; recover before cleanup/final success |
| `deleted` | Physical cleanup and final metadata persistence confirmed |

Ordinary pre-visibility I/O errors remain Session errors; they cannot hide a
post-visibility outcome. The SDK deletion contract lives in `sdk/runtime-client`;
the existing TUI protocol imports it. Rust and TypeScript roundtrip the same JSON
fixtures. The full confirmation UI is left to #257. Retention/GC policy remains
out of scope (issue #259 currently concerns Todo extensions, not retention).

## Deterministic evidence

`src/local_runtime/session/tests/deletion_tests/lifecycle.rs` covers pre-rename
failure, uncertain rename, pending/final transitions, final-publication failures,
fresh exclusion/stale preview, repeated work, explicit identity reuse, stale
catalog rejection, path substitution, independent fork/clone history and further
lineage materialization, and shared protocol fixtures.

The process-death test starts the real Rust test subprocess, waits for a flushed
boundary message, then kills and reaps it at `logical_commit` and `cleanup_item`.
It verifies surviving frozen scopes and deliberately corrupts unrelated live
storage before recovery to prove no ownership rediscovery occurs.

One test holds a conflicting root snapshot throughout recursive cleanup. Another
parks the real blocking cleanup worker with a scoped gate and acquires the
supervisor catalog mutex using `try_lock`, while asserting no terminal result has
been emitted. These are synchronization proofs, not timing assumptions. Existing
#254 cross-process, alias, child access, retained-workspace and disposal tests remain.
The provider-backed A/B conformance test now executes B deletion over Runtime Client
and verifies A remains active with no additional provider request.
