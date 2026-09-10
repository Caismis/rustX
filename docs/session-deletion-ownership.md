# Session deletion ownership reconnaissance

Issue #254 establishes authority, not a deletion operation.

| Resource | Authoritative owner / durable evidence | Cascade | Blocker | Shared/external | Restart |
| --- | --- | --- | --- | --- | --- |
| Session metadata and graph | SessionCatalog record, node membership | Record only; catalog is shared | No | Catalog file contains other Sessions | Yes |
| Node lineage | Unique catalog node ConversationId + SQLite bound identity | Yes | Live access | No | Yes |
| Node artifacts and managed output | Private node lineage allocation; tool runtime binds the same ConversationId | Yes | Live access | No | Yes |
| Child lineage | Typed SubagentOwnershipCommitted in parent's durable store | Yes | Live access | No | Yes |
| Child private execution roots | Exclusively allocated incarnation inside the child's semantic allocation | Yes | Live access | No | Yes |
| Child inspection socket / liveness | Identity-derived routing; OS lock is liveness only | Disposable sidecars | Live access | Not ownership evidence | Marker survives, lock does not |
| Subagent worktree / branch | Durable workspace ownership, terminal resource and disposal facts | Never implicit | Until fully disposed | User source may be present | Yes |
| Workflow worktree / branch / candidate | Durable Workflow workspace ownership, settlement and disposal | Never implicit | Until fully disposed | User source may be present | Yes |
| Environments / capability resources / caches | Runtime resource composition, independent of node artifacts | No | No | Shared | Yes |
| Config / credentials / source | User and project authority | No | No | External | Yes |

`/tree` nodes are members of one Session. `/fork` and `/clone` materialize independent
Sessions; origin references never grant ownership. The existing durable child
commit includes both parent (envelope) and child ConversationId. These typed
native facts can be read without model-visible text searches or a live registry.
The current local child composer does not install another subagent registry.

Management gaps found: catalog `open_existing` creates `sessions/`; lineage
management and child inspection use creating SQLite opens. Child artifacts are
currently incarnation-private and removed by explicit execution settlement.
Workspace directories can occur below node artifact roots: the entire node
root must not become an unconditional recursive deletion target.

## Final authority and exclusion contract

`SessionDeletionPreflight::acquire` opens and locks the canonical runtime root
**before** reading any ownership. It returns a guard-bearing snapshot, not a
plan that may be used after releasing exclusion. The snapshot includes every
node lineage and transitive child exactly once, a digest of observed catalog
and typed durable facts, and separate workspace blockers. Catalog-wide unique
ownership is checked; ambiguous ownership, cycles, missing stores and unsafe
identities fail closed. Provenance is not traversed. No deletion operation is
implemented.

Root identity is `canonicalize`, followed by an `O_DIRECTORY | O_NOFOLLOW`
open. The directory inode itself is locked with nonblocking `flock`: shared
for participants, exclusive for preflight. It is never deleted or replaced by
product lifecycle work. A permanent `.product-writer.lock` additionally admits
one native Session controller. The controller acquires both before opening the
catalog or composing storage. Child IPC explicitly carries the canonical
`product_root`; children acquire shared access before preparing storage.
Inspection acquires shared access before routing or opening a durable store.
Native runtime/store handles retain their access guards. Last guard release
ends access; abnormal process exit closes descriptors through OS semantics.
Equivalent spellings and symlinks to the root identify the same inode; distinct
roots are independent. This is a local Unix product boundary, not a lock
against a hostile administrator replacing the directory inode.

The exclusive OS acquisition is the authority point. Native ownership reads,
live-owner exclusion, and the revision observation all occur while that guard
is held. A live participant causes `WouldBlock` before snapshot derivation.
Workspace blockers still prohibit any future cleanup even under exclusion.
Dropping the snapshot releases exclusivity. A copied identity/path does not
carry authority. Future physical cleanup must remain under the guard and use
non-following filesystem operations; this issue implements no such cleanup.

The inspection liveness file remains a distinct stable lock inode. Its mere
presence has no ownership meaning. Probing an unlocked stale file does not
unlink it: unlink-after-unlock can split a racing owner's lock domain. A later
owner reuses it, and its socket bind replaces stale routing only after obtaining
the lease. The product lifecycle lock also covers this lease.

## Development storage boundary

Catalog schema **4 -> 5** establishes separated workspace allocation semantics.
SQLite development schema **31 -> 32** uses rollback journaling (`DELETE`),
`synchronous=FULL`, and the same canonical linear Conversation tables. Read-only
WAL can create shared-memory sidecars; existing-only management instead requires
rollback-journal storage and never initializes, binds an identity, checkpoints,
or recovers a database. A hot journal requiring recovery is an explicit read
failure; ordinary runtime startup owns recovery. Startup explicitly traverses
known graph lineages and typed child ownership, opening existing stores with
read/write authority to recover journals before any read-only Session selection.
`recover_existing` requires and retains the product writer guard and never uses
SQLite CREATE flags. This recovery is not part of management or preflight. There is no migration or
compatibility reader. Child IPC **19 -> 20** adds explicit product-root identity.

Authoritative layout:

```text
runtime/                              stable lifecycle directory inode
  .product-writer.lock                permanent controller lock inode
  sessions/catalog.json               shared file; target is one Session record
  sessions/<SessionId>/conversations/<ConversationId>/
    conversation.sqlite               bound identity and linear durable authority
    artifact_*.bin                    Conversation-private semantic artifacts
    tool-output/                      Conversation-owned managed output
  subagents/<ConversationId>/          owned only through a durable parent commit
    conversation.sqlite
    inspection liveness sidecar
    incarnation-*/                    exclusively allocated child-private resources
  workspaces/worktrees/               separate explicit workspace disposal domain
  environments/                       shared; outside the cascade
```

The node and semantic child allocations are private resource units because
native composition reserves them for the bound Conversation, not because a
prefix resembles an identity. An unreferenced allocation is not assigned to a
Session by scanning. Project files, shared capability and environment state,
cache, configuration and credentials never enter this projection. The shared
catalog file and stable lifecycle/controller lock inodes are not Session-owned
files. Retained or undisposed worktrees and branches remain blockers, with their
durable disposal records intact.

## Deterministic regression map

| Issue requirement | Regression |
| --- | --- |
| 1. All `/tree` nodes exactly once | `deletion_tree_membership_excludes_independent_fork_clone_and_shared_resources` |
| 2. Independent fork excluded | Same test uses native fork preparation/publication |
| 3. Independent clone excluded | Same test uses native clone preparation/publication |
| 4. Ownership survives parent process death | `deletion_cross_process_parent_death_preserves_nested_ownership` (real subprocess, committed-facts gate, kill/reap) |
| 5. Nested descendants exactly once | `deletion_nested_durable_children_restart_and_workspace_blocker`, plus the real parent-death test |
| 6. Unrelated ownership excluded | Tree/fork/clone test and nested test's unreferenced child allocation |
| 7. Shared resources excluded | Tree/fork/clone test creates separate shared classes and checks exact private membership |
| 8. Worktree is blocker | `deletion_nested_durable_children_restart_and_workspace_blocker` checks blocker path is outside every private target |
| 9. Cross-process conflicts | `cross_process_writer_exclusion_aliases_and_crash_release` (real process gate) |
| 10. Abnormal exit releases lock | Same real-process test uses kill/reap, then acquires a successor |
| 11. Aliases / symlinks share domain | Same real-process test checks canonical, lexical and symlink spellings |
| 12. Independent roots | Same real-process test acquires another root while the first is held |
| 13. Live child / inspection blocks | `cross_process_child_or_inspector_blocks_exclusive_until_release` (real process), `deletion_live_inspection_blocks_and_stale_marker_is_reusable` (native inspection lease) |
| 14. Stale marker recovery | Native inspection lease test drops owner, probes stale inode, reacquires |
| 15. Non-creating unknown lookup | `deletion_unknown_and_missing_lookup_never_creates_state`, `management_lock_lookup_is_noncreating_and_paths_fail_closed` |
| 16. Malformed/cyclic/escaped metadata | `deletion_tampered_catalog_and_symlink_escape_fail_closed`, `deletion_duplicate_and_cyclic_child_identity_fail_closed` |

The process helpers `lifecycle_process_gate` and `deletion_process_writer_gate`
are inert without their test environment rendezvous. Readiness is announced only
after acquisition/commit; commands or process death determine release. No sleeps,
arbitrary delays or probabilistic race windows establish these assertions.

The implementation does not add nested child execution to the local child
composer. The recursive projection consumes native descendant facts if present;
the test exercises this contract without introducing another executor or graph.
Actual Session deletion transactions, logical commit, interrupted-delete recovery,
physical cleanup, Runtime Client deletion protocol, TUI/CLI actions, force-kill,
automatic workspace disposal, batch deletion, trash and distributed coordination
are deliberately deferred.

The real-process regression
`deletion_hot_journal_reads_are_nonmutating_and_startup_recovers_owned_stores`
kills a writer after an uncommitted cache spill. Preflight refuses recovery and
leaves both database and journal bytes unchanged; explicit writer startup then
recovers the existing ownership tree and allows a complete preflight.

Existing-only opens inspect SQLite's documented format header before opening
the engine: even a read-only journal-mode query on a WAL database can create
sidecars. `deletion_wal_metadata_is_rejected_before_sqlite_can_create_sidecars`
proves rejection of this obsolete/tampered format without adding any file or
changing database bytes. This is a format refusal, not a legacy reader.
