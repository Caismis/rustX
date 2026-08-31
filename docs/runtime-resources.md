# Runtime resources and executable authority

`RuntimeResourceSnapshot` is the immutable process-local owner of loaded
resource-derived authority. One generation contains the ordered project
context files and concatenated bytes, the compact Skill catalog and discovered
Skill source identities, the agent profile and extension System Sections, the
admitted `SubagentCatalog` of named subagent definitions with the
capability-source availability of that same generation, and the compatible
immutable `CapabilitySnapshot` containing the Tool definitions and executors.

This object is not a conversation fact and is not a durable resource database.
`RuntimeResourceRevision` is only the process-local identity recorded with a
request. `ContextGeneration` continues to describe accepted context proposal
ownership and is not a resource lifetime.

## System authority

Canonical history contains only User, Assistant, and Tool messages. Project
instructions, child agent profile/persona, certified extension instructions,
and compact Skill guidance are typed request-time System Sections in this
total order:

1. `CoreRuntimeIdentity`
2. `AgentProfile`
3. `WorkspaceInstructions`
4. `CertifiedExtension` sorted by stable logical identity
5. `NativeCapabilityGuidance`

Their rendered value, `ModelRequest.effective_system_prompt`, is the only
System authority. OpenAI Chat Completions, OpenAI Responses, and Anthropic
adapters translate that value to their protocol field when non-empty and send
no System instruction when it is empty. They never reconstruct authority from
history.

Every `RequestSnapshot` stores the exact rendered prompt, exact ordered
sections, Tool definitions, capability revision, model state, and resource
revision by value. Historical reconstruction reads that snapshot and its
historical Surface revision only; it never reruns discovery or extension
logic.

Context continuity is a property of primary model requests: each primary
request combines its selected canonical Surface revision with the exact
attempt-pinned System and Tool authority. A maintenance summary invocation is
not a second continuity history, and merely retaining historical messages does
not make their former executable authority current.

Compaction's summary invocation is intentionally outside that primary
lineage. It is assembled from the runtime-owned summary instruction and the
exact planned retired historical messages only. It sends no Tools, primary
Effective System Prompt, project instructions, Skill catalog, extension Tool
definitions, or primary continuation; it does not share a provider prefix or
KV cache and does not recurse through the Agent Loop. Its result is the body
of the fixed structured Markdown summary contract (Issue #140); the committed
summary additionally carries typed cumulative file-operation metadata derived
from the retired span's canonical tool calls, never from the generated prose.
Historical Status observations may be summarized as
past evidence, but the summary is never current runtime authority.

## Named subagent definitions

A generation's `SubagentCatalog` is configuration/resource-generation state,
never live execution state. A loader builds it off-side — reading each
definition's instruction document and explicit project-instruction files —
validates every definition against the very capability candidate it is about
to publish, and only then does the candidate commit. A definition that names
an unknown capability, model, or Skill therefore rejects the whole candidate,
and the previous complete generation stays authoritative in every half.

The capability-source availability carried alongside the catalog is what lets
resolution distinguish two different facts:

- a *source* that is unavailable in this generation keeps the runtime healthy
  and blocks only the agents that explicitly require it;
- a selector whose source authority is present but that names an unknown
  capability is a static configuration error.

`CapabilitySnapshot` stays focused on executable capability identity — its
revision advances only when the effective committed executable set changes —
so this control-plane availability is carried on the resource generation that
needs it rather than distorting the capability revision's meaning.

Resolution binds to the generation the *invoking attempt* owns. An attempt
receives its `Arc<RuntimeResourceSnapshot>` at admission and hands each
foreground tool invocation an `AttemptSubagentContext` over exactly that
generation, so a reload that commits a newer generation cannot be observed by
an in-flight attempt. A reload additionally refuses while an attempt is live.

A resolved specification freezes everything the child needs: the
`(agent, definition_digest)` identity, the instruction document, the
completely resolved model invocation, the exact source-qualified capability
identities across Builtin/MCP/Python together with the exact admitted
`ToolDefinition` of each, the selected Skills' immutable
`SkillId` + `SkillVersionId` bindings with their model-visible catalog
metadata, and the exact project-instruction chain. The child consumes that
value and reinterprets nothing — it never reads `rustx.jsonc`, never reopens
`models.jsonc`, never runs the ancestor discovery described below, never
rediscovers Skills, and never widens or substitutes Tool identity.

Parent resolution is **semantic authority**; child composition is **physical
materialization**. The distinction is what makes "frozen" true rather than
nominal:

- the model crosses as a `FrozenModelSpec` — a resolved invocation carrying
  the provider binding, protocol, context window, output budget, reasoning
  profile and its semantic enabled state, effective request parameters,
  effective capabilities, and compat metadata — not as a
  `SessionModelConfig` plus a catalog path. The child builds the provider
  adapter from the frozen binding and resolves the declared credential
  source against its own process environment, which is rustX's existing
  credential boundary; it never re-resolves a model against a mutable
  catalog file that may have changed since the parent froze it;
- each Builtin capability crosses as its exact admitted `ToolDefinition`, so
  a generation's non-default execution, concurrency, or approval policy is
  the policy the child actually registers. The child reconstructs the native
  implementation for that name under the frozen policy and **fails closed**
  if the reconstruction does not equal the frozen definition;
- each MCP capability crosses as `server_id` plus the canonical name, the
  exact admitted `ToolDefinition`, and a deterministic **cross-process**
  `McpToolIdentity`. The process-local MCP invalidation epoch stabilizes one
  process's catalog read and means nothing in another process, so the child
  connects the server itself, performs its own `tools/list`, recomputes the
  identity from what the server actually publishes, and refuses to start on
  a missing or changed definition;
- each Python capability crosses as its exact immutable `ToolVersionId`. A
  workspace is not `ToolVersion` authority after resolution: the child opens
  that exact published version from the shared content-addressed store and
  revalidates its digest, so a newer same-named version can never substitute
  it;
- each Skill crosses as `SkillId` + `SkillVersionId` plus its catalog
  metadata and the materialization source it was frozen from. A host path is
  a source, never identity: the bytes behind a path can change without the
  path changing, so the child copies the exact frozen file set into its own
  runtime root, re-proves the version digest over the copy, and remaps the
  model-visible location onto that copy. Progressive disclosure is untouched
  — no `SKILL.md` body is preloaded;
- the specification additionally carries a `ResolvedSubagentMaterialization`
  plane holding **only** the sources the selection actually needs: the MCP
  server bindings of the selected tools, and a shared Python store root only
  when a Python tool is selected. An agent that selects one MCP tool has no
  second binding to widen to.

### What `definition_digest` is, and is not

`SubagentDefinitionDigest` is the identity of the **named definition
itself** — the normalized semantics configuration declares for that agent
(name, description, instruction document, explicit model reference, selector
set, Skill selector set, project-instruction policy). It is deliberately
*not* a digest of the full effective child runtime.

Everything the invoking generation contributes at resolution time —
inherited project instructions, the exact admitted Skill *versions*, the
resolved capability definitions, the resolved model invocation — is
invoking-generation state, not definition state. Two children started from
the same definition under two different generations therefore share a digest
while legitimately differing in resolved resources. The durable identity
`(agent, definition_digest)` answers "which named definition is this child
running?", which is exactly what ownership, recovery, and the Runtime Client
projection need; it never claims to answer "which exact effective runtime is
this child?".

## Project instruction discovery

### Workspace-owned Agent resources

Project-authored resources whose purpose is to define or guide Agent behavior
share the workspace-owned `.agents/` namespace:

```text
workspace/
├── AGENTS.md
└── .agents/
    ├── skills/
    ├── tools/
    ├── subagents/
    └── workflows/
```

This is an ownership namespace, not one implicit activation mechanism. Skills
and Python tools retain their automatic discovery contracts. Subagent profiles
and native Workflows remain explicit configuration surfaces: a Subagent must
be defined and admitted, and a Workflow id must be listed in
`workflows.definitions`. The configured runtime root (often `.rustx/`) is
runtime-owned/generated state and is not the canonical home for these
project-authored resources.

`.agents/skills/` is the canonical project layout. Skill discovery retains its
pre-existing automatic roots `~/.rustx/skills/`, `~/.agents/skills/`,
`<workspace>/.rustx/skills/`, and `<workspace>/.agents/skills/`; retaining the
`.rustx/skills/` roots does not make them the canonical project layout.

At runtime creation or explicit reload, applicable directories are traversed
deterministically from filesystem root to the workspace/cwd. At most one file
is selected per directory with this precedence:

1. `AGENTS.override.md`
2. `AGENTS.md`
3. `AGENTS.MD`
4. `CLAUDE.md`
5. `CLAUDE.MD`

Selected source paths and UTF-8 contents retain that root-to-leaf order and
are concatenated deterministically. Discovery never runs during ordinary
request assembly.

## Lifecycle and external edits

| Operation | Resource behavior |
| --- | --- |
| runtime creation / cold reopen / resume | discover once and publish generation 1 |
| ordinary primary request or tool continuation | reuse the attempt-pinned generation |
| automatic overflow compaction or manual `/compact` | reuse frozen inputs; no discovery |
| Runtime Client detach/reattach | no discovery |
| fork/clone/tree historical projection | select history only; no discovery |
| explicit `/reload` or runtime reload API | prepare a complete candidate and atomically publish it for future attempts |

Before reload or cold recreation, edits to project instructions, Skill
addition/removal/rename/frontmatter, and extension/Tool configuration have no
effect. A Skill catalog freezes only compact metadata and the discovered host
path/source identity. An already-discovered `SKILL.md` remains ordinary file
content: native Read observes its current body at execution time and returns
the normal read error if the file disappeared. Reload never rewrites an old
ToolResult.

Compaction is not a resource reload boundary. It does not discover, refresh,
suppress, resurrect, or serialize resources. An admitted attempt keeps its
one pinned resource/capability pair across primary requests, continuations,
and automatic compaction. A cold reopen is different: it may load current
resources for a newly admitted attempt, while old summaries and old
RequestSnapshots remain historical values and no synthetic resource-change
message is added to canonical history.

## Resource authority versus transcript history

Resource generations and transcript history have separate owners. The
process-local `RuntimeResourceSnapshot` supplies current System sections,
Skill guidance, Tool definitions, and executors for a newly admitted request;
the durable transcript resolves only visible message bodies and explicit
publication/interaction audits from their canonical owners. Resource edits,
explicit reload, the current `AGENTS.md`/Skill catalog, and the publication of
a new resource revision therefore create no ordinary transcript item.

An old `RequestSnapshot` retains its exact System bytes, ordered sections,
Tool definitions, and resource revision by value. Reconstructing that request
does not read the current resource generation. Conversely, cold reopen loads a
fresh resource generation for the first new request while retaining the same
durable transcript order. The transcript is bootstrapped and paged separately
from current resources, and it never becomes a resource cache or a source of
execution authority.

## Admission, pinning, and reload

Attempt admission and reload share one synchronization boundary. Admission
pins one `Arc<RuntimeResourceSnapshot>` and acquires a lease for that exact
snapshot's compatible `CapabilitySnapshot`; every model turn and tool
continuation in the attempt uses that pair.

Reload closes a narrow admission gate, verifies there is no active attempt,
pending Questionnaire/Approval interaction, or manual compaction, and retains one
counted lifecycle admission while releasing the synchronous state lock before
asynchronous discovery/preparation. On success it commits the complete
capability candidate and publishes the complete resource generation before
reopening admission — as **one** observation carrying the capability
snapshot, its availability, and the resource snapshot, which folds into one
`ResourceGenerationUpdated` Runtime Client event at one cursor. The
capability half is deliberately not published separately: the projection
worker folds on its own task and is woken by every enqueue, so two enqueues
are two folds a subscriber can be scheduled between, and two events are two
cursors an incremental client can sit between. Either would expose the new
capability generation beside the retired resource generation, a pairing no
runtime state ever had. On failure it keeps the old pair and reopens the gate.
Thus an admitted attempt lands wholly before or wholly after reload and cannot
observe mixed project, Skill, extension, or Tool generations. Reload emits no
canonical message.
Dropping/cancelling a reload while preparation is in flight releases the gate
without publishing its candidate. Explicit reload inputs republish their Tool
registry even when schemas are unchanged, because executor configuration is
not encoded in model-facing definitions.

## Capability publication and MCP physical ownership

`RuntimeResourceSnapshot` is the live product resource authority. Before a
conversation runtime claims a `CapabilityCoordinator`, the coordinator may be
used as a standalone prepare/commit owner during composition. The claim then
transfers one private publication authority to `ConversationRuntime`; the
ordinary coordinator `commit` API is rejected after that point. A live
capability change must therefore come through `reload_resources()`.

Reload holds the runtime admission lock while its runtime-owned publication
operation commits the matching `CapabilitySnapshot` and assigns the new
`RuntimeResourceSnapshot`, then clears the reload gate. Attempt admission
uses that same lock, so no attempt can enter between the capability swap and
resource assignment. The capability snapshot also carries the immutable MCP
lease authority for its own physical generation. An attempt admitted with
resource generation A can therefore acquire only A's MCP leases, even if B
becomes coordinator-current later.

Each prepared candidate owns every MCP runtime it connects. The capability
commit linearization transfers those runtimes into the published generation;
rejected, stale, cancelled, or dropped candidates retire and settle their own
physical runtimes. A published generation becomes retired when superseded,
but explicit attempt/background leases keep it alive until those owners
settle. Successful close proves physical reclamation and removes the
generation from the retirement registry. `McpError::PhysicalSettlement` means
that proof was not established: the registry retains the generation and the
failure as authoritative evidence, the runtime fences healthy admission via
its existing drain lifecycle, and shutdown reports the failure. This
post-publication settlement failure is distinct from a pre-publication reload
failure: the new generation remains the logical current authority in the
former case.

The failure is persistent runtime authority even while the conversation is
inactive. Installing the runtime callback replays failures already retained
by the retirement registry. MCP PhysicalSettlement failure publication and
the `ConversationLifecycle` transition to `Draining` are one runtime
coordinator linearization point: the callback's single coordinator critical
section records the persistent latch, performs any current-attempt
cancellation arbitration, and closes healthy admission before releasing the
coordinator lock. The latch is diagnostic/admission evidence retained by the
coordinator; `ConversationLifecycle` remains the generic gate for semantic
work. There is no interval after the authoritative failure transition in
which the runtime is still healthily `Running`. If activation orders first,
the same failure operation immediately transitions the running runtime into
drain before later ordinary admission. A ready-at-reload failure is therefore
returned synchronously as `PostPublicationSettlementFailed`, while a failure
that only becomes ready after a legitimate background lease settles uses the
same atomic failure-drain transition asynchronously.

`wait_close_attempt()` is the complete terminal-publication boundary, not a
notification that the underlying `close()` future merely returned. The close
task publishes generation settlement state, retirement-registry evidence, the
runtime fencing callback, and lifecycle-admission release before setting its
completion flag and notifying waiters. Consequently a reload cannot return
success while a ready superseded generation's physical failure is still being
published.

## Historical observations and lineage

Agent Status is an optional runtime-sourced canonical User context fact for a
successful primary model-turn start. One finite opportunity set may contain
FreshInbound, PostToolBatch, or both; a complete settled tool batch marks the
attempt-local PostToolBatch member only after canonical ToolResult settlement,
and never creates a model turn. Multiple status messages may remain in their
canonical order until normal Surface replacement/compaction removes them; old
status is never scanned to reconstruct Todo suppression. A generation that
loses cancellation-versus-start arbitration is never committed or projected,
and its Todo emission/head settlement is absent as well.

Fork, clone, and tree operations project a historical prefix. At a selected
human User boundary, the selected message and context/status belonging to that
old turn and later are excluded while earlier status facts may remain.
Destination-unique `MessageId` and `ToolCallId` values are remapped. Retained
`ToolExecutionId`, `SubagentId`, and background identifiers remain opaque
history and never reacquire live owners. Future destination requests use the
destination runtime's current resources and current Session intent.
