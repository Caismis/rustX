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

`SessionDeletionPreflight::acquire` returns a finite native target, workspace
blockers, a semantic ownership revision and retained OS exclusion. It never
traverses provenance, scans directories for ownership, or performs deletion.
Catalog-wide unique ownership is checked; ambiguity, cycles, missing stores and
unsafe identities fail closed. The Session graph stays above linear stores.

### Four separate responsibilities

- `ProductRoot` establishes canonical identity with `canonicalize` and an
  `O_DIRECTORY | O_NOFOLLOW` directory open. Identity alone claims no access.
- `ProductController` holds an exclusive nonblocking `flock` on permanent
  `.product-writer.lock`. This admits one native Session/catalog controller.
  It does **not** lock every Conversation or exclude its own preflight.
- `ConversationAccess` holds a shared `flock` on the native allocation directory
  selected by catalog ConversationId or typed durable child ownership. This is
  an identity-derived allocation, not a recursive filesystem discovery rule.
  Native runtime, durable store, artifact/output store clones and streaming
  writers retain the guard. Child composition, inspection and liveness leases
  acquire the same allocation access before touching private state.
- `OwnershipSnapshot` exclusively locks the canonical product-root directory
  against **ownership transitions only**. Catalog publication and typed durable
  child/workspace ownership transactions take compatible `OwnershipMutation`
  guards. Explicit Workflow disposal retains its guard across physical cleanup
  and durable settlement, rather than just individual event commits. Ordinary
  user/model/assistant/tool execution facts
  do not take this root lock. Target `ConversationExclusion` guards provide the
  separate exclusive private-resource access authority.

### Ordering and linearization

1. Controller admission linearizes at acquisition of `.product-writer.lock`,
   before native catalog mutation/composition. Its lifetime is independent of a
   deletion preview.
2. Preflight acquires `OwnershipSnapshot` before reading the catalog or durable
   ownership facts. This freezes graph/blocker transitions and global uniqueness
   evidence throughout derivation, acquisition and snapshot lifetime.
3. Under that freeze, derive the target and validate global unique ownership.
   Each journal traversal captures a finite high watermark before paging; later
   unrelated ordinary events cannot extend the traversal indefinitely.
   Unrelated ordinary activity is permitted and is not revision input.
4. Acquire exclusive target allocation guards in ascending `ConversationId`
   order. The final successful acquisition is **exclusive deletion authority**.
   A live target Conversation, child, inspector or detached private writer makes
   acquisition fail. Partial acquisition is dropped without cleanup.
5. Compute the canonical semantic revision while the freeze and target guards
   remain held. Return those guards together with the snapshot. No detached
   target list or copied token carries authority.
6. Drop the snapshot to release all target locks and the ownership freeze.
   Abnormal process death closes descriptors through OS semantics.

All acquisitions are nonblocking. A writer already holding Conversation access
must acquire the ownership-mutation guard before its SQLite transaction or
catalog publication; conflict returns an error before mutation. Preflight takes
root ownership freeze then sorted target locks. There is no blocking wait cycle,
lock upgrade, release/reacquire gap, or deletion based on a pre-lock snapshot.
Ownership-changing work is serialized for the bounded preflight lifetime;
unrelated ordinary execution continues. Preview callers must drop the snapshot
before awaiting human confirmation. A later operation reacquires and compares
the semantic token under fresh authority; #255 owns that product workflow.

`WorkspaceManager` owns physical workspace lifecycle and its optional local
`Arc<ConversationAccess>`. Local Conversation composition binds the same access
to the manager and the concrete SQLite store. The manager takes its process-local
disposal mutex, then acquires `OwnershipMutation` from its own local access before
reading retained facts. It retains authority through the durable Started commit,
physical worktree removal, branch compare-delete and durable Settled commit.
Every retry takes this authority even when Started is already durable. SQLite
transaction locks are acquired only after lifecycle authority; nested event
commits take compatible shared mutation guards. All OS acquisitions remain
nonblocking, so there is no SQLite/lifecycle wait cycle or lock upgrade.

`ConversationStore` contains only backend-independent durable semantic operations.
It returns no local OS lifecycle authority. Concrete `SqliteConversationStore`
retains its composed access internally to exclude individual ownership-sensitive
transactions, including Started; this complements the manager's spanning guard.
Synchronization wraps durable operations, rather than being supplied by them.

The production manager is composed with the native parent Conversation and shared
by its subagent registry and Workflow execution. Child composition currently
creates no WorkspaceManager or nested subagent/Workflow registry; its physical
workspace use remains parent-owned. Embedded/headless fixture managers can use
`WorkspaceManager::new` without local Session coordination. Native management
uses `with_local_lifecycle` with `ConversationAccess::existing` for the selected
catalog-owned allocation and canonical `ProductRoot`. This existing-only binding
works for historical Conversations without activating or switching their runtime
and without reading synchronization authority from a durable backend or workspace
path. The disposal race fixtures use this historical-management binding.

`WorkflowWorkspaceDisposalStarted` is the durable destructive admission boundary
and independently participates in event-level ownership exclusion, like the
subagent disposal Started event. If preflight wins the ownership freeze first,
disposal fails admission without publishing Started or touching Git; retry after
snapshot release can proceed. If disposal wins first, preflight returns
`WouldBlock` throughout physical cleanup and settlement. Guard drop (including
error return) or OS process death releases live authority. The store's ordinary
Conversation access also remains alive for the complete call. Recovery of a
previous interrupted disposal still uses existing durable intent and settlement
semantics; this adds no recovery transaction or deletion state machine.

Started alone does not change the final blocker semantics and is not added to
`ownership_revision` or the preflight blocker projection. The spanning exclusion
prevents observation of an in-flight live disposal; settled physical/disposal
state remains the revision authority. The real WorkspaceManager path is covered
by `deletion_preflight_first_excludes_workflow_destructive_admission_until_release`
and `deletion_workflow_disposal_first_excludes_preflight_through_physical_settlement`,
using barriers before removal and between worktree and branch removal.
`deletion_workspace_owner_excludes_preflight_without_store_lifecycle` additionally
proves the local manager supplies spanning authority when its semantic store has
no local lifecycle binding; no fake lock capability is implemented by the store.

A live product controlling A can call `LocalSessionSupervisor::deletion_preflight`
for historical B without switching, detaching or restarting A. A's controller
admission and Conversation access do not conflict with B's target locks.
Unrelated live children likewise do not block B. Actual target access does.

Equivalent root spellings and symlinks resolve to the same inode; distinct roots
are independent. Stable lock/allocation inodes must not be replaced by product
cleanup while authority is retained. This is local Unix coordination, not a
boundary against an administrator replacing inodes. Retained workspace blockers
still prohibit future physical cleanup even with target exclusivity.

### Semantic ownership revision

The length-delimited canonical vocabulary starts with
`rustx/session-deletion-ownership/v1`, followed by target SessionId, sorted node
membership `(node id, parent node id, ConversationId)`, sorted owned Conversation
edges and root-relative private allocation identities, and sorted workspace
blockers. Each blocker includes owning Conversation, native resource identity,
repository/worktree/branch authority, and final disposal-relevant state (owned,
retained exact head/dirty state, unresolved safety reason, or branch-only).
Diagnostics are excluded. Fully disposed resources disappear from blockers.

The revision changes for target node/child/nested ownership changes and blocker
transitions. It does not change for unrelated activity or metadata, target
ordinary execution, messages, model/tool history, timestamps, journal sequence,
request identities, provenance, selection, catalog formatting/order or mtimes.
A Workflow run's attempt identity is included only as part of its actual native
workspace resource identity. Global uniqueness checks inspect broader state but
never hash that unrelated state. A conflicting claim fails closed instead of
producing a normal new revision.

The inspection liveness file remains a distinct stable lock inode. Its mere
presence has no ownership meaning. Probing an unlocked stale file does not
unlink it: unlink-after-unlock can split a racing owner's lock domain. A later
owner reuses it, and its socket bind replaces stale routing only after obtaining
the lease. The product lifecycle lock also covers this lease.


### Workflow-owned workspaces and borrowing children

A Workflow Agent borrowing a Workflow-owned candidate/worktree does not acquire
independent physical workspace disposal authority.

```text
Session
  +-- Workflow W
  |     +-- owns worktree X (sole disposal authority)
  +-- child C
        +-- owns child Conversation/private runtime state
        +-- borrows X from W; cannot own or dispose X
```

Preflight validates `WorkspaceSnapshot.borrowed_from` against the preceding
native `WorkflowWorkspaceOwned` fact in the same Conversation: valid run and
canonical ownership-event identity, Workflow child ownership kind, and exact
owner snapshot equality after removing only the borrow marker. The Workflow
snapshot must itself be independently owned and isolated. Missing, foreign or
mismatched authority fails closed; paths alone never prove the relationship.
The immutable Workflow ownership fact remains available for this validation
after its physical blocker is disposed.

Every child Conversation remains in the target, but any number of valid
borrowers produce only the Workflow's single physical workspace blocker. Only
Workflow disposal changes that blocker to branch-only and then removes it.
No child terminal publication is needed, including after a crash immediately
following child ownership commit. Borrowing adds no extra workspace resource to
the semantic revision; new child Conversations still change target scope.

The real subprocess regression
`deletion_borrowed_workspace_crash_before_child_terminal_disposes_only_workflow`
uses a committed-facts gate and kill/reap before reopening preflight, then
verifies one Workflow blocker, branch-only settlement and zero blockers after
complete Workflow disposal, without any child terminal event. The
`deletion_borrowed_workspace_missing_workflow_owner_fails_closed`,
`deletion_borrowed_workspace_mismatched_workspace_fails_closed`,
`deletion_borrowed_workspace_foreign_or_invalid_run_fails_closed`, and
`deletion_borrowed_workspace_multiple_children_share_one_disposal_owner`
regressions cover invalid authority and multiple legitimate borrowers.

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
compatibility reader. Child IPC **20 -> 21** (after integrating Issue #256) adds explicit product-root identity.

Authoritative layout:

```text
runtime/                              stable ownership-transition lock inode
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

The original tree/fork/clone, nested restart ownership, unknown/missing lookup,
malformed/cyclic metadata, retained workspace and stale-inspection regressions
remain. The revised exclusion and semantic-token contracts are proved by:

| Contract | Exact regression |
| --- | --- |
| Real live product A completes work while B preflight is retained; target A blocks | `active_session_a_executes_while_historical_b_preflight_retains_authority` (real provider emulator, Runtime Client, native product) |
| Target child blocks; unrelated child does not | `deletion_target_child_access_blocks_but_unrelated_child_access_does_not` |
| Target inspection blocks; stale marker reused | `deletion_live_inspection_blocks_and_stale_marker_is_reusable` |
| Cross-process access/destructive conflict, SIGKILL release, aliases, distinct roots | `cross_process_target_conflicts_aliases_death_and_independent_roots` (**real subprocess**) |
| Actual target preflight versus target/unrelated child access and competing preflight | `deletion_cross_process_target_child_unrelated_child_and_destructive_conflict` (**real subprocesses, kill/reap**) |
| Controller excludes another controller but coexists with preflight | `cross_process_controller_admission_is_independent_of_target_exclusion` (**real subprocess**) |
| Parent death retains nested durable ownership | `deletion_cross_process_parent_death_preserves_nested_ownership` (**real subprocess**) |
| Non-creating identity/access lookup and confinement | `management_lock_lookup_is_noncreating_and_paths_fail_closed`, `deletion_unknown_and_missing_lookup_never_creates_state` |
| Unrelated metadata/activity, target ordinary events and catalog encoding preserve token | `deletion_revision_ignores_unrelated_metadata_and_irrelevant_execution_history`, plus the real A/B product test |
| Target nodes, children and nested children change token | `deletion_revision_changes_for_target_nodes_children_and_nested_children` |
| Retention, partial disposal and full disposal change token | `deletion_revision_tracks_retained_and_partial_and_complete_disposal` |
| No ownership transition races the snapshot; ordinary events still succeed | `deletion_snapshot_freezes_native_ownership_commits_but_not_ordinary_events` |
| Detached artifact/output stores and streaming writers retain access | `deletion_detached_private_stores_and_writers_retain_target_access` |
| Cross-Session claims fail rather than yield a token | `deletion_cross_session_child_claim_is_ambiguous_not_a_revision_change` |
| Cycles, malformed identities and symlink escapes fail | `deletion_duplicate_and_cyclic_child_identity_fail_closed`, `deletion_tampered_catalog_and_symlink_escape_fail_closed` |

Process helpers announce readiness only after acquisition/commit; stdin gates or
kill/reap determine release. No sleeps or probabilistic race windows prove these
contracts. The provider-backed product test awaits native settlement.

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
