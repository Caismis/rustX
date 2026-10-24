# Durable Sessions, runtime residency, and client focus

One user runtime root has one native `ProductController` and one
`SessionController`. Opening that controller creates no Session, selects no
Session, resolves no configuration, and starts no runtime. A Session remains a
graph: `Session -> SessionNode -> Conversation`. Each node has its own linear
Conversation identity, canonical history and retained Surface operations.

Each started model request has an explicit owner in its durable Journal fact.
Agent start atomically records the Snapshot-derived owner with request Context and
Snapshot; keyed owner reads and terminal checks reject missing or contradictory
evidence. Ownership is not inferred from an ID prefix or absent fields. SQLite
schema 80 combines Scheduler definitions and Human-request mutation budgets with
frozen request fingerprints that include the native JSON Schema capability and
optional prepared response schema. Both creating and existing-only opens reject
earlier branch formats before any historical request is decoded or reconstructed;
there is no migration. Schema 80 also includes bounded `WorkflowNodeWaiting`
Journal facts and reserved Auxiliary Call identities. The Event envelope remains
version 1.

The catalog owns identity, manual name, generated title/provenance, timestamps,
graph, Session-local `active_node`
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
| `model: Option<SessionModelConfig>` | Intentional whole-model selection: model reference, Model Profile, request parameters, output cap and summary policy. `None` uses current source defaults. |

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

Fork/clone captures canonical history and invocation-time execution provenance
under one native read cut. Historical model context additionally selects a typed
ProjectionActivationId independently of the canonical Ledger frontier. Two
activations at the same frontier remain distinct through repeated copies. Source
origin and destination installation are recorded separately; destination-owned
canonical dependencies, checkpoint bodies and authorized Artifacts survive source
deletion. `session/forkProjection` selects that exact historical boundary through
the existing lineage lifecycle. A fork prompt remains unaccepted editor content
until explicitly submitted to the destination.


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
App Server v35 exposes bounded public control-plane projections. Compile-fail API
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

Issue #480 defines success against process death while the operating system and
filesystem remain healthy. Power loss, kernel failure and storage failure are
outside the newest-commit guarantee. Namespace visibility alone is not successful
completion: a caller must perform the owned reservation barriers and propagate
observed failures before reporting success.

```text
exclusive ConversationId allocation (create-new marker)   linearization point
marker file sync                                          fallible completion
reservation-namespace directory sync                      marker entry completion
product-root directory sync                               namespace entry completion
owned product-root sync (ProductRoot::create)               root completion
```

Recursive ancestor synchronization is removed. The retained operations surface
reservation/publication errors and preserve existing failure classifications;
they do not establish a product-wide machine-failure durability guarantee. The
list names logical calls, not a syscall budget or physical device traffic.

A later caller re-establishes the product-root barrier whenever it observes the
namespace. Initialization that failed or died before completion, visible residue,
and retry do not permit inferring successful completion from directory existence.
A successful reservation survives process death and ordinary restart on the same
healthy filesystem. Tests exercise that fault model, not physical power loss.
See the [finite synchronization audit](issue-480/durability.md).

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
reservation record layout did not change; the current Catalog schema is 17,
which also carries independent generated-title metadata. See
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

## Session lifecycle (App Server v35)

Create, open/resume, switch, fork/branch, and delete operate on durable Sessions.
Opening implicitly reuses or composes a runtime. Close view releases an attachment
only. Runtime residency belongs to `SessionRuntimeManager`, appears in explicit
diagnostics, and is absent from ordinary Session lists. Confirmed deletion fences
admission and retires the runtime before destructive exclusion and revision
revalidation; the current Session is supported. No replacement Session is created
when the last one is deleted. See [deletion lifecycle](session-deletion-lifecycle.md).


Automatic titles use an indexed native first ordinary Human occurrence, including a
Human without a timestamp. Its exact Conversation and Message identities and a
bounded text outcome are committed with the canonical message; preview remains
independent. Seeded history is marked inherited by its immutable bootstrap
boundary and does not grant local execution eligibility.

The Store binds a native Title admission to that occurrence atomically with
CallStarted. At most one association is current. A zero-request terminal releases
only its own association; a confirmed RequestStarted consumes eligibility
permanently. RequestStarted enforces an ordinal below three in the same SQLite
transaction. No separate Title quota is maintained. Recovery after exclusion of
the old live owner can settle a stranded zero-request Call, but never dispatches
historical work. Exact receipt replay cannot consume twice or clear a newer Call.

A transient retry commits ModelRequestFailed and AuxiliaryRetryScheduled in one
transaction. The retry carries bounded delay and request evidence, and enables
only the next ordinal. Every request keeps its own Usage and uncertainty even
when a later request succeeds. Call-only cancellation can settle a backoff whose
physical request is already closed. SQLite schema 80 rejects earlier layouts;
there is no migration or reconstruction path.


Catalog schema 17 persists `generated_title` separately from the manual `name`.
Catalog Summary readers in native, Web and TUI prefer manual name, generated
title text, opening preview, then exact Session identity. The TUI active header
uses a Session snapshot, which carries saved labels and identity but no preview. Listing remains Catalog-owned
and never opens Conversation Stores. The Root-local Title owner is installed
before activation, holds one operation and one coalesced wake, and keeps the
business owner until the separate Catalog publication tail settles.

Title publication conditionally checks the Session still exists, its local root
source matches, and neither manual name nor generated title is already saved.
An idempotent repeat of the same source/candidate commits nothing. A visible
metadata change advances the Catalog generation once and emits summary
invalidation even when the durable acknowledgement fails. The owner may confirm
that same visible candidate once more through the native durability barrier; this
does not repeat the model request, advance generation or emit another invalidation. Publication does not
change settings revision or latest-Human activity. Manual renaming before
publication refuses it; renaming afterwards takes display precedence while
retaining the generated provenance and recorded model costs.

Switching nodes within one Session retains the single Root title owner and source
eligibility. Independent Clone and Fork capture already-persisted manual/generated
labels as a snapshot, including when the selected historical cut precedes the
message that motivated the label. Inherited generated provenance names the source
Session but carries no local source identity, in-flight task, output candidate or
request allowance. Source rename/deletion cannot alter the copied label. Copied
history alone cannot authorize Title generation; an empty-history destination
can become eligible when its first new qualifying Human commits.
