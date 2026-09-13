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

Runtime residency is a separate lifetime. #287 owns single-flight load/unload,
multiple resident runtimes, and one writable live runtime per Conversation.
`SessionAccess` provides the Session snapshot, selected node, explicit settings
and revision, plus retained Conversation allocation access. Different
Conversations can hold these accesses concurrently. Allocation access is
necessary destructive exclusion, not a replacement for #287's single-writer
runtime admission.

Client focus is routing/UI state. `LocalSessionClient` and
`LocalSessionAttachment` are the existing CLI's single-runtime composition and
protocol adapter. Their attached identity is local and is never serialized.
Their catalog commands cannot shut down another runtime. An ordinary open is a
metadata read; it does not publish focus. A startup plan carries a routing node
separately from its optional durable document mutation. An explicit `--node`
(including the existing default) never changes catalog bytes, generation, timestamps,
or the Session graph default. Only an explicit graph operation such as
`set_current_node` changes that default. `SessionRoute` preserves the durable
snapshot and identifies the client node separately; the legacy wire translation
stays inside the local adapter until #288. CLI cold resume requires `--session`
(and optionally `--node`); implicit `--continue` is refused. The existing wire
transition vocabulary is temporarily retained until #288; ordinary transitions
return `restart_required: false`. The wire's committed-durability result still
carries an exact fork editor payload. The local TUI routes by the returned identity when reopening its own subprocess;
it never asks the catalog for a global focus. Shared-process runtime residency
is not implemented by this catalog change.

## Persisted configuration classification

`SessionPersistentState` is explicit input, not effective configuration:

| Persisted field | Reason |
| --- | --- |
| `cwd` | Absolute execution and project-resolution context; never ambient process cwd or an OS sandbox. |
| `config: Option<PathBuf>` | Explicit project-document selection. Its current content is reread, never copied. Omission retains cwd-based current discovery. |
| `model: Option<SessionModelConfig>` | Intentional whole-model selection: model reference, reasoning profile, request parameters, output cap and summary policy. `None` uses current source defaults. |
| `skill_paths: Vec<PathBuf>` | Explicit additional Skill source selections; the contents and availability remain current source authority. |
| `no_automatic_skills`, `no_builtin_tools`, `no_direct_tools` | Explicit narrowing switches. False and omission have the same input semantics. They grant no capability. |
| `tools: Option<Vec<String>>` | Exact direct Tool selection; omission and an explicitly empty selection remain distinct in storage. Resolution still applies its existing validity rules. |
| `exclude_tools: Option<Vec<String>>` | Explicit exclusions, preserving omission versus explicit empty replacement. |

User bindings (`settings`, `models`, runtime root, HOME/XDG roots, credentials)
are process/user authority. Agent profiles, user policy, model/provider catalogs,
MCP and Python definitions, Skill content, defaults, instructions and resource
catalogs remain current source content. Registries, connections, credentials,
resource generations, provenance, prospective/admitted effective configuration,
and execution state are never persisted as Session authority.

Cold load acquires the native `ProductController` before opening the authoritative
catalog and reading persisted settings. That owner remains retained through
resolution, admission and composition, excluding competing settings publishers.
The complete settings value and revision come from this one catalog snapshot;
no catalog mutex spans resolution or runtime composition. Cold load reconstructs
`SessionConfigInput` and uses #285's
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

Load versus delete uses existing OS allocation locking. Access acquires the
shared allocation lock before checking catalog membership. Deletion preflight
requires exclusive target allocations under frozen ownership. If access wins,
delete reports in-use; if deletion commits first, allocation admission rejects
removed membership even when files remain. Recursive cleanup runs outside the
catalog mutex and outside root ownership exclusion, using the frozen pending
record. Recovery never rediscovers a new deletion workset.

Deletion preview, execution, and recovery on `SessionController` remain
crate-private. `DeletionScope`, `DeletionRecord`, previews, blockers, and internal
results are not public native DTOs: their frozen scopes are cleanup authority.
#288 will define bounded public control-plane projections. Compile-fail API
regressions enforce this boundary.

## Schema

Schema 8 removes the product-global `active_session`, permits an empty catalog,
and stores explicit Session settings with a settings revision. The reader checks
the schema before decoding its changed layout. All older development schemas are
explicitly refused; there is no migration. In particular, schema 7 materialized
model defaults cannot be distinguished from intentional overrides. Silently
retaining those defaults would invent Session-owned intent. Reopen reconstructs
all Sessions without inventing client focus or runtime residency.
