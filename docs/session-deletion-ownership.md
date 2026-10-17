# Session deletion ownership reconnaissance

Issue #254 establishes authority, not a deletion operation.

| Resource | Authoritative owner / durable evidence | Cascade | Blocker | Shared/external | Restart |
| --- | --- | --- | --- | --- | --- |
| Session metadata and graph | SessionCatalog record, node membership | Record only; catalog is shared | No | Catalog file contains other Sessions | Yes |
| Node lineage | Unique catalog node ConversationId + SQLite bound identity | Yes | External access after managed retirement | No | Yes |
| Node artifacts and managed output | Private node lineage allocation; tool runtime binds the same ConversationId | Yes | External access after managed retirement | No | Yes |
| Child lineage | Typed SubagentOwnershipCommitted in parent's durable store | Yes | External access after managed retirement | No | Yes |
| Child private execution roots | Exclusively allocated incarnation inside the child's semantic allocation | Yes | External access after managed retirement | No | Yes |
| Child inspection socket / liveness | Identity-derived routing; OS lock is liveness only | Disposable sidecars | External access after managed retirement | Not ownership evidence | Marker survives, lock does not |
| Shared Session workspace | User/project authority; frozen as the absolute binding in ownership facts | Never | No | External project files | Yes |
| Agent activation physical settlement | Terminal `physical_settlement_proven`, later `SubagentPhysicalSettlementProven`, proven admission rollback | No | Until proven | No | Yes |
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

`DeletionTargetSnapshot::inspect` returns a finite native target and semantic
ownership revision without destructive allocation exclusion, or fails closed on
an unproven activation barrier.
`DeletionExclusion::acquire` separately claims private allocations only after the
runtime manager fences admission and proves writer retirement. It never
traverses provenance, scans directories for ownership, or performs deletion.
Catalog-wide unique ownership is owned by
`local_runtime::session_ownership::SessionOwnership`, shared with archive and child
inspection. It derives graph roots, typed child relationships, identity validity,
parents and Session attribution under the product ownership freeze. Ambiguity,
cycles, missing stores and unsafe identities fail closed before selecting a
Session. Deletion then adds activation settlement barriers and revision
semantics under that same freeze; archive adds only historical cut frontiers. Neither owns
a separate descendant traversal. The Session graph stays above linear stores.

### Canonical allocation authority

Canonical `ProductRoot` is the sole authority for rustX-owned product storage
paths. Runtime-owned allocations derive within that canonical domain; separately
authored absolute path spellings are not storage identities. Startup resolves
aliases before catalog creation, Conversation allocation, or child staging.
Management resolves existing product identity without creating missing state.
`confined()` still rejects escapes and symlinks below that root; it does not
canonicalize arbitrary caller-authored private paths into authority.

`SubagentSpawnPlan` retains `ProductRoot`. Child IPC version 28 carries its canonical
root, the child Conversation identity, and a validated incarnation name. It carries
no second absolute runtime root. The child derives
`subagents/<ConversationId>/<incarnation>/` and requires that exact private
allocation to exist. Artifacts, output, Skills and child-private environments
are derived beneath it; the stable database and inspection sidecars derive from
the Conversation identity.

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
  child ownership transactions take compatible `OwnershipMutation`
  guards. Ordinary
  user/model/assistant/tool execution facts
  do not take this root lock. Runtime child identity reservation takes waitable
  `runtime_ownership_admission()` before Conversation allocation serialization;
  it releases after reservation/incarnation creation, before process staging.
  Cancellation is rechecked after waiting, before reservation. The later,
  separate durable Subagent ownership commit
  uses waitable `runtime_ownership_admission()` before registry/durability/lifecycle
  commit mutexes; a transient inspection freeze delays it without rejecting a
  valid staged child. Blocking OS admission runs only in the blocking pool. Its
  shared guard covers both the ownership Journal event and registry publication,
  and releases before capacity waits or driver handoff. Target `ConversationExclusion` guards provide the
  separate exclusive private-resource access authority.

### Ordering and linearization

1. Controller admission linearizes at acquisition of `.product-writer.lock`,
   before native catalog mutation/composition. Its lifetime is independent of a
   deletion preview.
2. Inspection acquires `OwnershipSnapshot` before reading the catalog or durable
   ownership facts. This freezes graph/blocker transitions and global uniqueness
   evidence throughout derivation, acquisition and snapshot lifetime.
3. Under that freeze, derive the target and validate global unique ownership.
   Each journal traversal captures a finite high watermark before paging; later
   unrelated ordinary events cannot extend the traversal indefinitely.
   Unrelated ordinary activity is permitted and is not revision input.
4. For confirmed deletion only, after manager retirement proof, acquire exclusive
   target allocation guards in ascending `ConversationId`
   order. The final successful acquisition is **exclusive deletion authority**.
   A live target Conversation, child, inspector or detached private writer makes
   acquisition fail. Partial acquisition is dropped without cleanup.
5. Inspection computes the canonical semantic revision under the ownership
   freeze, without target guards. Final deletion holds both the snapshot and
   destructive exclusion while revalidating the confirmed revision. A copied
   token carries no destructive authority.
6. Drop the inspection snapshot to release the ownership freeze. Separately
   drop destructive exclusion to release target locks.
   Abnormal process death closes descriptors through OS semantics.

All acquisitions are nonblocking. A writer already holding Conversation access
must acquire the ownership-mutation guard before its SQLite transaction or
catalog publication; conflict returns an error before mutation. Inspection takes
the root ownership freeze; confirmed deletion additionally takes sorted target locks. There is no blocking wait cycle,
lock upgrade, release/reacquire gap, or deletion based on a pre-lock snapshot.
Ownership-changing work is serialized for the bounded preflight lifetime;
unrelated ordinary execution continues. Preview callers must drop the snapshot
before awaiting human confirmation. A later operation reacquires and compares
the semantic token under fresh authority; #255 owns that product workflow.

`ConversationStore` contains only backend-independent durable semantic operations.
It returns no local OS lifecycle authority. Concrete `SqliteConversationStore`
retains its composed access internally to exclude individual ownership-sensitive
transactions. Synchronization wraps durable operations, rather than being
supplied by them.

A live product controlling A can call `DeletionTargetSnapshot::inspect` (the
native preflight behind App Server `session/deletePreview`) for either A or B
without switching, detaching or restarting A. Inspection
retains no target allocation locks. At final deletion, independent target access
blocks destructive exclusion; unrelated live children do not.

Equivalent root spellings and symlinks resolve to the same inode; distinct roots
are independent. Stable lock/allocation inodes must not be replaced by product
cleanup while authority is retained. This is local Unix coordination, not a
boundary against an administrator replacing inodes.

### Semantic ownership revision

The length-delimited canonical vocabulary starts with
`rustx/session-deletion-ownership/v2`, followed by target SessionId, sorted node
membership `(node id, parent node id, ConversationId)`, and sorted owned
Conversation edges and root-relative private allocation identities. Issue #462
removed workspace blockers from the vocabulary and bumped its tag; a v1 token
never matches. Unproven activation barriers fail inspection instead of
contributing to the revision.

The revision changes for target node/child/nested ownership changes. It does not change for unrelated activity or metadata, target
ordinary execution, messages, model/tool history, timestamps, journal sequence,
request identities, provenance, selection, catalog formatting/order or mtimes.
Global uniqueness checks inspect broader state but
never hash that unrelated state. A conflicting claim fails closed instead of
producing a normal new revision.

The inspection liveness file remains a distinct stable lock inode. Its mere
presence has no ownership meaning. Probing an unlocked stale file does not
unlink it: unlink-after-unlock can split a racing owner's lock domain. A later
owner reuses it, and its socket bind replaces stale routing only after obtaining
the lease. The product lifecycle lock also covers this lease.


### Shared workspace and activation barriers (Issue #462)

Agents and Workflows run in the Session's frozen workspace binding and own no
workspace resource, so deletion never removes, inspects or blocks on project
files. Preflight still validates every ownership fact: a durable Agent's
resumed activations must repeat its admitted identity and exact binding, and a
Workflow child can never hold durable Agent authority. An activation terminal
without proven physical settlement, or an activation admission whose rollback
was not proven, keeps the Session undeletable. A pre-reserved activation's
parent physical lease also blocks destructive exclusion until its exact proof.

## Development storage boundary

Catalog schema **4 -> 5** establishes separated workspace allocation semantics.
Issue #480 replaces the original schema-33 rollback-journal policy with WAL/NORMAL
without changing logical schema. Existing-only management validates bound WAL
stores and never initializes, converts journal mode or repairs application state.
SQLite can create/rebuild physical WAL/SHM sidecars even on read-only access;
management therefore makes no no-sidecar promise. A compatible DELETE store or
hot rollback journal requires explicit writer-owned startup recovery first.
Startup traverses known graph lineages and typed child ownership before read-only
Session selection. `recover_existing` retains the product writer and allocation
guards, never uses CREATE, validates the current schema/identity, and establishes
WAL/NORMAL. There is no obsolete-schema migration or compatibility reader.
See [the fault and lifetime contract](issue-480/durability.md). Child IPC **20 -> 22** (after integrating Issue #256) carries canonical product identity and an incarnation name instead of an independent absolute child runtime path. Obsolete wire shapes are rejected.

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
  environments/                       shared; outside the cascade
```

The node and semantic child allocations are private resource units because
native composition reserves them for the bound Conversation, not because a
prefix resembles an identity. An unreferenced allocation is not assigned to a
Session by scanning. Project files, shared capability and environment state,
cache, configuration and credentials never enter this projection. The shared
catalog file and stable lifecycle/controller lock inodes are not Session-owned
files.

## Deterministic regression map

The original tree/fork/clone, nested restart ownership, unknown/missing lookup,
malformed/cyclic metadata and stale-inspection regressions remain. The revised exclusion and semantic-token contracts are proved by:

| Contract | Exact regression |
| --- | --- |
| Real live product A completes work while B inspection is retained; A inspection succeeds and destructive exclusion blocks | `active_session_a_executes_while_historical_b_preflight_retains_authority` (real provider emulator, Runtime Client, native product) |
| Target child blocks; unrelated child does not | `deletion_target_child_access_blocks_but_unrelated_child_access_does_not` |
| Target inspection blocks; stale marker reused | `deletion_live_inspection_blocks_and_stale_marker_is_reusable` |
| Cross-process access/destructive conflict, SIGKILL release, aliases, distinct roots | `cross_process_target_conflicts_aliases_death_and_independent_roots` (**real subprocess**) |
| Actual target preflight versus target/unrelated child access and competing preflight | `deletion_cross_process_target_child_unrelated_child_and_destructive_conflict` (**real subprocesses, kill/reap**) |
| Controller excludes another controller but coexists with preflight | `cross_process_controller_admission_is_independent_of_target_exclusion` (**real subprocess**) |
| Parent death retains nested durable ownership | `deletion_cross_process_parent_death_preserves_nested_ownership` (**real subprocess**) |
| Non-creating identity/access lookup and confinement | `management_lock_lookup_is_noncreating_and_paths_fail_closed`, `deletion_unknown_and_missing_lookup_never_creates_state` |
| Unrelated metadata/activity, target ordinary events and catalog encoding preserve token | `deletion_revision_ignores_unrelated_metadata_and_irrelevant_execution_history`, plus the real A/B product test |
| Target nodes, children and nested children change token | `deletion_revision_changes_for_target_nodes_children_and_nested_children` |
| Deletion never removes the shared Session workspace or a settled Agent's effects | `deletion_never_removes_the_shared_session_workspace` |
| Unproven terminal, unproven rollback and pre-reserved leases block deletion | `cold_shared_agent_deletion_retains_unproven_terminal_evidence`, `resumed_admission_deletion_requires_exact_generation_settlement_proof`, `pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof` |
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
Session deletion transactions, logical commit, interrupted-delete recovery,
physical cleanup and App Server/TUI deletion control are specified separately in
[Session deletion lifecycle](session-deletion-lifecycle.md). Force-kill, batch
deletion, trash and distributed coordination are outside
this ownership contract.

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

Existing-only Surface-head validation runs in one SQLite read transaction. The head, checkpoint, and immutable operation history therefore come from the same committed snapshot even while another connection performs ordinary execution. The transaction neither creates storage nor recovers journals; it ends when the bounded read returns.

Version 32 on main freezes Issue #258’s effective child `profile_digest`; version
33 additionally requires this Session deletion and local management contract.
A v32 database is rejected, without migration or compatibility decoding.
Invocation overrides affect tools, skills, and extensions, never ProductRoot or
the Session workspace binding. Capability-profile metadata, including
`definition_digest` and `profile_digest`, is excluded from deletion revisions.

## SESSION-DELETE-02 lifecycle extension

The ownership/preflight protocol above now feeds schema 7's atomic logical
commit and frozen cleanup record. Preview releases all guards before confirmation;
execute reacquires them. Conversation admission checks catalog deletion authority
after acquiring its allocation lock. Cleanup begins only after file/parent-directory
durability and runs outside the root freeze and supervisor catalog mutex.
See [session-deletion-lifecycle.md](session-deletion-lifecycle.md) for commit points,
uncertainty, recovery, allocator monotonicity, catalog generations, bounded App Server protocol v35 and test mapping.

Preview never retains `ConversationExclusion` while awaiting user confirmation.
Final inspection remains under the ownership snapshot through exclusion acquisition,
revision revalidation and catalog commit. Runtime residency alone is no blocker.
