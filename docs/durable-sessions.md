# Durable Sessions, runtime residency, and client focus

One user runtime root has one native `ProductController` and one
`SessionController`. Opening that controller creates no Session, selects no
Session, resolves no configuration, and starts no runtime. A Session remains a
graph: `Session -> SessionNode -> Conversation`. Each node has its own linear
Conversation identity, canonical history and retained Surface operations.

The catalog owns identity, name, timestamps, graph, Session-local `active_node`
(the default node inside that graph), explicit settings, settings revision,
lineage origins, and pending deletion authority. This node pointer does not
identify a globally open Session. `create_session`, `list_sessions`,
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
App Server v16 exposes bounded public control-plane projections. Compile-fail API
regressions enforce this boundary.

## Schema

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

## Session lifecycle (App Server v16)

Create, open/resume, switch, fork/branch, and delete operate on durable Sessions.
Opening implicitly reuses or composes a runtime. Close view releases an attachment
only. Runtime residency belongs to `SessionRuntimeManager`, appears in explicit
diagnostics, and is absent from ordinary Session lists. Confirmed deletion fences
admission and retires the runtime before destructive exclusion and revision
revalidation; the current Session is supported. No replacement Session is created
when the last one is deleted. See [deletion lifecycle](session-deletion-lifecycle.md).
