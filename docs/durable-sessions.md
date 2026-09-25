# Durable Sessions, runtime residency, and client focus

One user runtime root has one native `ProductController` and one
`SessionController`. Opening that controller creates no Session, selects no
Session, resolves no configuration, and starts no runtime. A Session remains a
graph: `Session -> SessionNode -> Conversation`. Each node has its own linear
Conversation identity, canonical history and retained Surface operations.

The catalog owns identity, name, timestamps, graph, Session-local `active_node`
(the default node inside that graph), explicit settings, settings revision,
lineage origins, and pending deletion authority. This node pointer does not
identify a globally open Session. The catalog also owns the bounded derived
display projection (`display_preview`). The projection is display metadata,
not canonical history: canonical history is never reconstructed from it, and
the catalog never re-derives it while listing. It reaches the catalog through
exactly three seams — the one-shot publisher armed on the root runtime, which
commits it after the first canonical root-lineage ordinary user commit; the
frozen-seed derivation carried into a clone/fork visibility commit; and the
explicit, idempotent repair seam run at reopen/compose/recovery, which
derives the line from the root conversation store and never manufactures a
subject from a later message. Preview publication is a metadata-only commit:
it never touches `updated_at`, and the catalog generation moves only when a
real change commits (an already-present projection commits nothing).

Canonical commitment and projection publication are two separate commit
points. A `None` projection is therefore either a legitimately empty
projection (no ordinary user message yet, or a first one with no renderable
text) or an unrepaired publication gap; it is not necessarily a short-lived
condition, and nothing may treat it as one. When publication does commit, the
catalog records a post-commit summary invalidation — recorded only after the
visibility point, including the visible-but-durability-uncertain outcome — so
a live client has a defined convergence path (`session/summaryInvalidated`);
a no-op repair writes nothing and announces nothing.

Repair belongs to the **Session** and reads the Session's **root** lineage,
whatever node is being composed: a Session reopened directly onto a branch
repairs to the root's first ordinary user message, never the branch's. Only
*arming the live publisher* is root-runtime-specific — it requires that the
composed runtime is the root runtime and that the root lineage has no ordinary
user boundary yet. At interactive startup the derived repair folds into the
existing planned startup transaction, so a launch that fails to compose still
writes nothing.
`create_session`, `list_sessions`,
`read_session`, `rename_session`, `tree`, `fork_session`, `read_settings`,
`replace_settings`, `acquire_session`, and deletion all address explicit
identities. Creation always allocates an independent Session. Lists include
unused Sessions and have no global active marker. The monotonic accepted-inbound
usage classifier remains available by identity; it no longer controls creation
or catalog visibility.

Runtime residency is a separate lifetime. `SessionRuntimeManager` owns single-flight
load/unload, multiple resident runtimes, and one writable live incarnation per
Conversation. In v1, one Session may have only one resident Conversation/node;
different Sessions remain concurrent. Successful unload releases composition and
allocation even when stale client handles remain. See [runtime residency](runtime-residency.md) for synchronization and
client lifetime contracts.
`SessionAccess` provides the Session snapshot, selected node, explicit settings
and revision, plus retained Conversation allocation access. Different
Conversations can hold these accesses concurrently. Allocation access is
necessary destructive exclusion, not a replacement for the manager's single-writer
runtime admission.

Client focus is routing/UI state. Ordinary local TUI switching uses the same
App Server child and changes only the focused attachment. Remote TUI and Web
Console use WebSocket to an externally managed process. A focus change never
quiesces another Session, unloads its runtime, or replaces the process.

An explicit node route does not change catalog bytes, generation, timestamps,
or the Session graph default. Only an explicit graph operation changes the
Session-local default node. Changing the loaded node uses confirmed, targeted
unload/cold attachment; other Sessions remain independent. The CLI cold-resume
path also requires an explicit Session identity, with an optional node.

Session identifiers are scoped to their product root. Two independent users'
processes can allocate the same identifier spelling; routing to the right user
process is the higher-level host's responsibility, never an in-server tenant key.
See the [host acceptance and dogfooding flows](app-server-acceptance.md).

## Persisted configuration classification

`SessionPersistentState` is explicit input, not effective configuration:

| Persisted field | Reason |
| --- | --- |
| `cwd` | Absolute execution and project-resolution context; never ambient process cwd or an OS sandbox. |
| `model: Option<SessionModelConfig>` | Intentional whole-model selection: model reference, reasoning profile, request parameters, output cap and summary policy. `None` uses current source defaults. |

The bound User `rustx.toml`, HOME, fixed User resource root and runtime root
are process bindings. Provider credentials belong to their complete winning
Provider definition. Agent profiles, user policy, model/provider catalogs,
MCP and Python definitions, Skill content, defaults, instructions and resource
catalogs remain current source content. Registries, connections, credentials,
resource generations, provenance, prospective/admitted effective configuration,
and execution state are never persisted as Session authority.

Cold load acquires the native `ProductController` before opening the authoritative
catalog and reading persisted settings. That owner remains retained through
resolution, admission and composition, excluding competing settings publishers.
The complete settings value and revision come from this one catalog snapshot;
no catalog mutex spans resolution or runtime composition. Cold load reconstructs
`SessionConfigInput` and uses the CFG3
`UserConfigManager::resolve_session` and admission. No second resolver exists.
Current authorization and availability are checked again. Missing explicit
models/resources and invalid selections fail through existing typed resolution
or admission outcomes; an old selection never grants authority or triggers a
fallback to a different model or broader selection.

## Commit points

Creation privately initializes and validates Conversation storage before catalog
publication. Preparation occurs outside the controller metadata mutex. A
preparation gate serializes identity allocation, independently of metadata reads.
Blocking preparation retains that guard even when its async caller is cancelled;
unpublished private storage is never mistaken for a completed Session.
The catalog writes/fsyncs a temporary document, compares the persisted generation
under the native publication lock, and renames the document. Rename is visibility
for create, rename, graph changes, settings CAS and logical deletion. Directory
fsync is the subsequent durability barrier. Before visibility, readers see the
old complete document; afterwards they see the new complete document, even if the
durability barrier fails. Committed uncertainty must not be retried as though
nothing happened.

Settings use a per-Session revision. `replace_settings(id, expected, settings)`
compares and advances it in the same publication. A stale edit cannot overwrite a
winner. A committed-but-uncertain write consumes the revision. Name and graph
metadata do not consume settings revisions.

Fork/clone captures an exact Surface revision and the canonical/history cut
through that revision. Retained source allocation access excludes deletion until
publication. Later source appends cannot change the copied boundary. The existing
lineage-cut algorithm preserves compaction provenance, historical boundaries,
identity remapping and transient editor content. A fork prompt is not accepted
input until explicitly submitted to the destination.

The runtime manager fences Session admission and retires managed writers before
delete acquires exclusive target allocations under frozen ownership. Preview only
inspects ownership and never requires runtime absence. Independent allocation
owners remain genuine resource conflicts. After deletion commits, allocation
admission rejects removed membership even when files remain. Recursive cleanup runs outside the
catalog mutex and outside root ownership exclusion, using the frozen pending
record. Recovery never rediscovers a new deletion workset.

Deletion preview, execution, and recovery on `SessionController` remain
crate-private. `DeletionScope`, `DeletionRecord`, previews, blockers, and internal
results are not public native DTOs: their frozen scopes are cleanup authority.
App Server v23 exposes bounded public control-plane projections. Compile-fail API
regressions enforce this boundary.

## Conversation identity reservation (Issue #387)

Conversation identity consumption is separate from published Session ownership
and from canonical conversation history:

```text
conversation-reservations/<ConversationId>   this ID is consumed, once, forever
Session Catalog / Session graph             published Session/Conversation ownership
ConversationStore conversation.sqlite       canonical history and semantic durability
```

### The exclusive operation (A)

One narrow storage-owner primitive reserves a `ConversationId`:
`ProductRoot::reserve_conversation`. It creates a marker file named after the
canonical identity inside the private root-level `conversation-reservations/`
namespace with `O_CREAT|O_EXCL` (`create_new`). That exclusive create is the one
exclusive allocation linearization point for the identity: exactly one caller
observes success, and every later caller observes `AlreadyExists` without
overwriting the consuming marker. There is **no** check-then-create and **no**
enumeration of existing Session directories or Conversation allocations. The
marker is private: it is not a Session catalog, a runtime registry, execution
authorization, or canonical history, and no caller interprets a marker path, a
`SQLite` filename, a directory's existence, or a traversal algorithm as the
reservation result.

Every allocation caller uses this same primitive:

| Caller | Owner |
| --- | --- |
| new root Session | `SessionCatalog::allocate_ids` |
| clone / independent fork | `SessionCatalog::allocate_ids` |
| branch/tree node | `SessionCatalog::prepare_tree_node` |
| first Session | `SessionCatalog::create_unpublished_with_identities` |
| child/subagent Conversation | `PhysicalChildRuntimeRoot::allocate` |

Preparation then creates the private allocation directory with an exclusive
filesystem operation. `SessionCatalog::create_conversation_allocation` is
preparation, not identity allocation.

### Persistence before durable success (B)

The exclusive create alone is not a power-loss guarantee. Namespace and marker
**visibility** are separate from their **durability**: observing an existing
`conversation-reservations/` directory does not prove that initialization
durability already completed. Before local storage reports the reservation
layout initialized, and before it reports durable reservation success, it
establishes every required parent-directory barrier itself:

```text
exclusive ConversationId allocation (create-new marker)   linearization point
marker file fsync                                         marker data durable
reservation-namespace directory fsync                     marker entry durable
product-root directory fsync                              namespace entry durable
product-root ancestry fsync (ProductRoot::create)         root entry durable
```

The barrier list names logical durability operations (one marker `sync_all`
request and one directory `sync_all` request per entry the barrier visits). It
is not a syscall budget: `SQLite`/library-internal filesystem work and physical
device I/O are separate evidence layers and are not claimed by these names.

A later caller re-establishes the product-root barrier whenever it observes the
namespace, so an initializer that created the directory and then failed or
died before the barrier, a failed barrier that left visible residue, and a
retry all complete the obligation instead of inferring it. The supported crash
model is: a reservation observed as successful survives process death and an
orderly restart of the same filesystem, and the parent-entry barriers are the
best-effort local expression of power-loss durability. A power loss between the
exclusive create and its fsync may lose the marker; that is the ordinary
durability limit of the supported local platform, not a claim of universal
atomic durability, and process-death tests prove process-death semantics rather
than every physical power-loss interleaving.

### Result and retained state on persistence failure (C)

If the marker create or its fsync fails, no reservation success is reported and
no allocation is published. If a reservation succeeds but a **later**
initialization, validation, publication, cancellation, or cleanup step fails,
the marker is retained: the identity stays consumed. rustX never unlinks a
reservation and never treats orphan cleanup or ordinary Session deletion as
permission to reuse the identity. A caller that loses a reservation race
retries with a fresh identity through the native allocation owner, bounded by
the existing finite retry budget; it never reuses the conflicting identity and
never overwrites the prior marker. Persistent I/O or format failures surface
instead of being hidden by an unbounded retry.

### Old-layout boundary

Absence of a marker does not prove an identity was never allocated. A populated
root whose `sessions/` tree exists without the reservation namespace predates
this contract and is refused at the storage owner — including at child/subagent
entry points that can bypass catalog loading — before any allocation. There is
no backfill, migration, dual allocation mode, compatibility scan, or automatic
deletion, and the old Session-directory scan is not retained as a fallback. A
genuinely fresh root (no namespace and no `sessions/` tree) initializes normally.
The format decision is linearized on the exclusive namespace creation: fresh
initialization creates the namespace strictly before any `sessions/` tree, so an
observer that reads the namespace absent and then sees a `sessions/` tree
re-checks the namespace and accepts a concurrently initialized new-format root
rather than refusing it as a legacy layout. Interrupted fresh-root
initialization is safe because nothing was reserved yet; a surviving namespace
is completed on the next call, and a lost namespace over an absent `sessions/`
tree is still fresh. Deleting only `sessions/catalog.json` or only the reservation
namespace is never a reset: manual reset deletes the whole runtime root and
recreates the Sessions, and rustX never deletes or reinterprets old data.

## Schema

The local-root reservation boundary is independent of the catalog schema; the
catalog record layout did not change, so schema 13 remains current. See
[Issue #387 validation](issue-387-validation.md) for the R01-R14 mapping and
measurements.

Schema 9 adds the Session-owned upload registry, frozen historical workspace roots
and private copy-preparation cleanup claims. It retains the schema 8 rules:
no product-global `active_session`, an allowed empty catalog, and explicit Session
settings with a settings revision. The reader checks
the schema before decoding its changed layout. All older development schemas are
explicitly refused; there is no migration. In particular, schema 7 materialized
model defaults cannot be distinguished from intentional overrides. Silently
retaining those defaults would invent Session-owned intent. Reopen reconstructs
all Sessions without inventing client focus or runtime residency.

See [Session-owned workspace uploads](session-uploads.md) for receipt admission, model paths, fork copies and durable cleanup.

## Session lifecycle (App Server v23)

Create, open/resume, switch, fork/branch, and delete operate on durable Sessions.
Opening implicitly reuses or composes a runtime. Close view releases an attachment
only. Runtime residency belongs to `SessionRuntimeManager`, appears in explicit
diagnostics, and is absent from ordinary Session lists. Confirmed deletion fences
admission and retires the runtime before destructive exclusion and revision
revalidation; the current Session is supported. No replacement Session is created
when the last one is deleted. See [deletion lifecycle](session-deletion-lifecycle.md).
