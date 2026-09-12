# Runtime resources and executable authority

See [canonical named Agent resources](subagent-resources.md) for schema 8
Agent files, discovery/admission, bounded roots, source provenance, and frozen
reload/child contracts.


The shared launch analysis loads authorized local resources and invokes the same
Workflow static compiler without executors, Sessions, or online schemas. Initial
composition consumes its compiled catalogs; runtime resource reload retains its
existing explicit ownership. Pure native Tool metadata selection is shared with
runtime registration selection, not a checker-specific Tool interpreter. See
[configuration diagnostics](configuration-diagnostics.md) for deferred online facts.

External discovery is inert until host trust and explicit source activation
admit preparation. See [source activation and credentials](source-activation.md)
for TOML settings, whole-source replacement, secret authority, and the exact
publication/retirement/reconnect frontiers. `--no-tools` is model exposure
control and does not disable external preparation.

## Exact Tool authority and native defaults

An ordinary main Agent sees and can invoke exactly its frozen selected registry.
Revision zero has no executable Tool authority; a prepared candidate publishes
the selection. Default selection includes available built-ins named by
`default_tools` (Read is an ordinary member) and admitted main Workflow tools.
External Tools require the main Agent's explicit `tools.sources` selection;
materialization for another Agent never exposes them to main. `--no-builtin-tools` removes all built-ins from this
default selection, including generated Subagent/Workflow tools.
`--tools a,b` narrows selection to those applicable registered names; external
identities must first be selected through `tools.sources`.
`--exclude-tools a,b` subtracts last, without reinsertion. Exclusions resolve
against applicable availability, so excluding an already unselected available
identity is valid, while unknown/ineligible or ambiguous names fail.
`--no-tools` selects zero ordinary tools, including generated dispatchers.

### Ordinary selection is one of two planes

Every filter above addresses the **ordinary capability plane**. A Native Agent
Extension may also contribute a model-facing Tool, and that Tool belongs to
`extensions`, not to any selector here:

```text
  ordinary selected Tool capabilities        every flag in this section
+ enabled extension-provided Tool surfaces   extensions.<name>.enabled
+ already-admitted domain terminal protocols Workflow output, ...
```

Neither plane filters the other. `--no-tools` selects zero *ordinary*
capabilities and does not disable an independently composed extension, so a
truly Tool-free model request needs no ordinary Tools **and** no Tool-providing
extension. `--no-builtin-tools` removes ordinary built-ins, not every Tool that
happens to be implemented in Rust: the classification is semantic, not
incidental to where the implementation lives.

Symmetrically, no selector can switch an extension on. `todo` is provided by
the Todo Agent Extension and is rejected — with a diagnostic naming the
extension — in `defaultTools`, `--tools`, `--exclude-tools`, a named
Subagent's `tools.builtin`, and a Workflow's admitted capability set. It is not
an ordinary available capability at all, so it never appears in the available
catalog those selectors resolve against. See
[Native Agent Extensions](launch-configuration.md#native-agent-extensions).

`--no-tools` conflicts with `--tools`, `--exclude-tools`, and
`--no-builtin-tools`; `--tools` conflicts with `--no-builtin-tools`.
All explicit lists reject empty values/entries, duplicates, unknown or
unavailable identities, and names shared by multiple applicable origins.
There is no registration-order precedence. Use `--no-tools`, not an empty
`--tools` list. The Rust selection boundary validates resolved intent too.
These filters do not grant source activation or alter independently admitted
child/Workflow capabilities. An invisible dispatcher cannot initiate execution.

Native product defaults are independent typed axes:

| Tool | Execution | Concurrency | Approval |
| --- | --- | --- | --- |
| Read | foreground_only | parallel | never |
| Write | foreground_only | sequential | always |
| Edit | foreground_only | sequential | always |
| Glob | foreground_only | parallel | never |
| Grep | foreground_only | parallel | never |
| Bash | model_selectable | sequential | always |

Read owns its bytes, decoder and projection per invocation; document caching
is disabled. Glob/Grep own their traversal, matcher, searcher and collector per
invocation. They share no mutable cursor or cache requiring sequential
scheduling. Write/Edit remain exclusive batch barriers around file mutations.
Parallel scheduling does not promise a filesystem snapshot against outside
writers. Bash's required `execution_mode` chooses foreground/background
ownership independently of concurrency and approval.

Missing `nativeTools`, missing entries and missing axes all retain each
tool's product default. For example, `"read": {"approval": "always"}`
retains foreground/parallel, and a partial Bash override retains
model-selectable execution. `execution`, `ask_user` and Workflow terminal
protocols retain their fixed domain owners. The extension-provided `todo` Tool
is outside this table for a stronger reason — it is not an ordinary native
capability, and its policy belongs to its own extension.

A lazy model-visible Skill catalog requires native Read in that domain's
frozen Tool authority. Otherwise the catalog is omitted, without enabling
Read or injecting Skill bodies. Discovery, package snapshots and child Skill
admission remain independent. A child admitting Read can use its frozen
Skills even when the main Agent excludes Read.

Native policy is sampled into canonical definitions at registry construction,
published in the capability generation and pinned at attempt lease acquisition.
Request compilation and call preflight use that same immutable registry;
model capability flags never silently remove its definitions. Existing request
validation rejects Tools for a model without Tool-call support; select
`--no-tools` **and** disable every Tool-providing extension
(`"extensions": { "todo": { "enabled": false } }`) to use such a model with no
Tools at all.
Preflight freezes mode, concurrency and approval in the prepared invocation.
The approval rendezvous settles before the executor-start frontier. FullAccess
bypasses only this Tool permission gate: it grants no tool, changes no
execution axis, answers no Questionnaire/Workflow Review, and bypasses no
project trust. Later configuration cannot mutate a pinned invocation.
`workflow_output` is added only by an admitted Workflow Agent's terminal
owner, never by ordinary main selection; its exactly-once settlement and
cancellation linearization remain unchanged.

## Launch authority and resource paths

Project trust permits project resources, never Tool approval policy.
`approvalMode`, `nativeTools`, and `mcpToolPolicies` belong exclusively to host
settings, including execution/concurrency members of those policy objects.

Project-origin Skills, canonical Agent TOML/`agents_md.files`, and
path-valued MCP `command`/`cwd` must resolve inside the canonical trusted
workspace. Project instructions, Workflow files and automatic `.agents` resource
roots obey the same containment boundary. Absolute paths, traversal, symlink
targets and an external `--config` cannot grant another worktree's authority.
User/CLI explicit resources retain host authority and their domain validation.
Native file-tool arguments and command argument strings remain execution
semantics, not sandboxed paths.

Checks run before initial resource loading and again for reload candidates from
launch-pinned slots. Symlink changes are re-evaluated against physical targets;
failed candidates leave the previous generation intact. This does not claim
race-proof syscall isolation against a hostile local OS user. See the complete
[launch authority policy](launch-configuration.md#project-resource-path-authority).

Frozen project MCP bindings preserve the original resource workspace through
child admission and reconnect. The shared connect boundary checks local
command/cwd targets before every process spawn; it does not rediscover settings.

## Resource ownership

`capabilities::selection` owns `ToolSelector` and exact source-qualified Tool
resolution. Named Subagents and fixed Workflows consume it directly. MCP and Managed Python share `tools.sources` with `All` or `Exact` selection.
See [ordinary Tool source selection](tool-source-selection.md).

Foreground Leaf/Composite policy is immutable registration metadata, not an
executor trait declaration or model-facing schema field. Catalog equality includes
this policy but deliberately excludes executor pointer identity. Explicit reload's
`force_publish` and frozen MCP binding comparison publish changed materializations
even with identical definitions. Old snapshots retain their immutable catalog and
executor Arcs; they cannot see a replacement executor through a new generation.

WF-02 retains executable registrations (including frozen foreground policy) in
the generation's `AvailableToolCatalog`, independently of its active model
`ToolRegistry`. A fixed Workflow explicitly admits source-qualified Builtin/MCP
selectors and resolves them using the same availability and identity rules as
named subagents. Managed Python retains typed package source identity above its materializer.
An inactive but available capability can serve a Workflow without widening the
model surface. Unavailable sources, unknown selectors, changed identities and
ineligible orchestration capabilities fail closed. Registration reconstruction
preserves its private admitted policy; fixed-leaf preparation rejects Composite
substitution. No later resource reload replaces in-flight authority.

`RuntimeResourceSnapshot` is the immutable process-local owner of loaded
resource-derived authority. One generation contains the ordered project
context files and concatenated bytes, the compact Skill catalog and discovered
Skill source identities, the agent profile and extension System Sections, the
admitted `AgentCatalog` of named subagent definitions with the
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

A generation's `AgentCatalog` is configuration/resource-generation state,
never live execution state. A loader builds it off-side — reading each
canonical role Markdown resource and explicit project-instruction files —
validates every definition against the very capability candidate it is about
to publish, and only then does the candidate commit. A definition that names
an unknown capability, model, or Skill therefore rejects the whole candidate,
and the previous complete generation stays authoritative in every half.

`AgentDocument.timeout_ms` is an optional, definition-level positive
millisecond value. Admission rejects zero, malformed values, and values above
the rustX-owned 86,400,000 millisecond (24 hour) maximum; it never clamps an
invalid value. The validated `SubagentExecutionDeadline` is part of the
definition's semantic digest and is copied into `ResolvedSubagentSpec`, so an
attempt keeps the deadline of the immutable generation it admitted even if a
later reload changes the current configuration. The model-facing `subagent`
call has no deadline field and cannot set or extend it.

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
`(agent, definition_digest)` identity, the optional whole-lifecycle execution
deadline, the instruction document, the
completely resolved model invocation, the exact source-qualified capability
identities across Builtin and typed MCP/Managed Python ToolSources together with the exact
admitted
`ToolDefinition` of each, the selected Skills' immutable
`SkillId` + `SkillVersionId` bindings with their model-visible catalog
metadata, and the exact project-instruction chain. The child consumes that
value and reinterprets nothing — it never reads `rustx.toml`, never reopens
`models.toml`, never runs the ancestor discovery described below, never
rediscovers Skills, and never widens or substitutes Tool identity.

Parent resolution is **semantic authority**; child composition is **physical
materialization**. The distinction is what makes "frozen" true rather than
nominal:

- the model crosses as a `FrozenModelSpec` — a resolved invocation carrying
  the provider binding, protocol, context window, output budget, reasoning
  profile and its semantic enabled state, effective request parameters,
  effective capabilities, and compat metadata — not as a
  `SessionModelConfig` plus a catalog path. The child builds the provider
  adapter from the frozen binding. Admitted credential values transfer through
  the private child environment, with references on the control channel;
  normal serialization redacts literals. It never re-resolves a model against a mutable
  catalog file that may have changed since the parent froze it;
- each Builtin capability crosses as its exact admitted `ToolDefinition`, so
  a generation's non-default execution, concurrency, or approval policy is
  the policy the child actually registers. The child reconstructs the native
  implementation for that name under the frozen policy and **fails closed**
  if the reconstruction does not equal the frozen definition;
- each MCP capability crosses as `server_id` plus the canonical name, the
  exact admitted `ToolDefinition`, and a deterministic **cross-process**
  `SourceToolIdentity`. The process-local MCP invalidation epoch stabilizes one
  process's catalog read and means nothing in another process, so the child
  connects the server itself, performs its own `tools/list`, recomputes the
  identity from what the server actually publishes, and refuses to start on
  a missing or changed definition. A selected Python tool crosses exactly
  this way: as the frozen binding of its synthesized `python:<folder>`
  server, verified through the same `SourceToolIdentity` — a workspace edit
  after the freeze fails the child's preparation closed instead of
  substituting a different server;
- each Skill crosses as `SkillId` + `SkillVersionId` plus its catalog
  metadata and the materialization source it was frozen from. A host path is
  a source, never identity: the bytes behind a path can change without the
  path changing, so the child copies the exact frozen file set into its own
  runtime root, re-proves the version digest over the copy, and remaps the
  model-visible location onto that copy. Progressive disclosure is untouched
  — no `SKILL.md` body is preloaded;
- the specification additionally carries a `ResolvedSubagentMaterialization`
  plane holding **only** the sources the selection actually needs: the MCP
  server bindings of the selected tools (a selected Python tool contributes
  its synthesized server's binding; no Python store root crosses at all). An
  agent that selects one MCP tool has no
  second binding to widen to.

### What `definition_digest` is, and is not

`SubagentDefinitionDigest` is the identity of the **named definition
itself** — the normalized semantics configuration declares for that agent
(name, description, instruction document, explicit model reference,
whole-lifecycle execution deadline, selector set, Skill selector set,
project-instruction policy). It is deliberately
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

### Retained subagent workspaces

`runtime::workspace` owns the native `WorkspaceManager`, `WorkspacePolicy`,
and physical `WorkspaceLease`. The Subagent registry and process driver consume
that owner; workspace types are not re-exported through `runtime::subagent`.
The lease currently follows the staged-child to process-driver settlement
path described below. Workflow run retention and node borrowing are still
pending WF-03 work.

An isolated subagent starts from the captured committed source `HEAD`. If the
child leaves no source change, rustX removes its runtime-created worktree and
branch during normal terminal settlement. If it changes the worktree — either
through uncommitted source edits or a child commit — rustX retains the exact
worktree as a handoff. Retained work is not merged, copied, rebased, stashed,
or removed automatically.

Disposal is an explicit user/runtime-client operation on the terminal
subagent, exposed in the TUI as the confirmed `D` action. It discards the
retained worktree, including uncommitted source changes, and removes only the
runtime-created branch proven to belong to that worktree. The model-facing
`execution` intrinsic cannot dispose workspaces, and clients cannot supply an
arbitrary filesystem path or Git ref. The parent model learns of retained
work only as a runtime-authored semantic fact in the canonical inbound
terminal publication — folded into the runtime-authored terminal notice of
a successful publication, or into the one runtime-authored terminal message
of a failed/cancelled/interrupted one — that the child's isolated changes
were retained and are not
applied to its workspace — never as a physical path, branch, or commit
(Issue #192).

The post-terminal resource lifecycle is separate from the child's absorbing
logical `Succeeded`, `Failed`, `Cancelled`, or `Interrupted` state:

```text
None -> Retained -> DisposalInProgress -> WorktreeRemoved -> Disposed
          ^                 |
          |                 +-- durable intent authorizes exact continuation
          |
PreservedUnresolved -- exact re-proof --> DisposalInProgress
```

`None` means no runtime-owned isolated worktree remains. `Retained` means the
worktree remains and rustX has a complete `WorkspaceHandoff`.
`PreservedUnresolved` means a runtime-created worktree may remain but final
inspection could not prove that stronger handoff. It is durable state, not a
missing handoff: the immutable `WorkspaceSnapshot` in the ownership fact
retains the source repository, logical relative workspace, deterministic
physical root, runtime branch, base commit, and subagent identity. Recovery
restores this state without inspecting the filesystem and never manufactures a
handoff.

Before deletion from `Retained`, and before a later retry from
`PreservedUnresolved`, rustX re-verifies the source repository identity,
deterministic path, exact physical Git worktree registration, worktree `HEAD`,
branch attachment, branch value, and snapshot relationship. Any stale,
tampered, missing, or rebound relationship fails closed and leaves the
resource `PreservedUnresolved`. Because this state has no durable terminal
handoff `HEAD`, both current heads must still equal the immutable snapshot base
before rustX can derive a disposable handoff; a changed commit remains
unresolved. Filesystem presence is not ownership proof, and filesystem absence
is not proof of prior disposal. A resource preserved
because nested supervised-process containment was unproven is stricter: Git
facts alone cannot authorize deletion, so the Runtime Client refuses the
disposal request until that containment authority is resolved.

The durable disposal intent is committed before the first destructive Git
operation. `git worktree remove --force` is the physical destructive
linearization point. Branch cleanup then uses compare-delete against the exact
recorded expected `HEAD`; if the branch moved or cleanup cannot be proven, the
worktree remains represented as `WorktreeRemoved` and the residual branch is
preserved. Only successful branch settlement and the final durable settlement
fact reach `Disposed`. Recovery resumes an authorized intent, continues exact
branch settlement for `WorktreeRemoved`, and converges an intent-authorized
fully absent resource to `already_disposed`; it never turns an unresolved or
externally missing resource into ordinary retained or successful disposal.
Repeated identity-based requests are serialized and idempotent. rustX makes no
claim that the final proof and Git command are one atomic operation against an
external Git actor; its own requests serialize, compare-delete is conditional,
and any remaining ambiguity fails closed. A shared or automatically cleaned
workspace returns `no_retained_workspace`.

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
    ├── agents/*.toml
    └── workflows/
```

Resources define themselves by existing in their canonical resource location.
Settings and Agent Profiles express selection, not existence. Agent TOML, Skill
packages, Managed Python directories and Workflow YAML form deterministic catalogs.
AgentCatalog and WorkflowCatalog are built off-side by local resource loading;
ManagedPythonCatalog records inert package identities alongside them. The existing
SkillSnapshot remains the sole authoritative frozen Skill catalog in the capability
candidate. All four publish through one PreparedRuntimeResources commit. A malformed
candidate publishes none of its catalogs, and existing snapshots remain unchanged.

Discovery is distinct from runtime readiness, selected Agent capability, frozen
active capability and invocation approval. Managed Python discovery never enters
package parsing or preparation. The runtime root (often `.rustx/`) remains generated
state, separate from these canonical authored resources.

Native launch resolves automatic Skill roots to the user configuration
directory's `skills/` and `<workspace>/.agents/skills/`. Explicit Skill paths
are layered by the Rust resolver. See [launch configuration](launch-configuration.md).

After the host grants trust, runtime creation and explicit reload load only
the resolved workspace's instructions. At most one file is selected with this precedence:

1. `AGENTS.override.md`
2. `AGENTS.md`
3. `AGENTS.MD`
4. `CLAUDE.md`
5. `CLAUDE.MD`

The selected source path and UTF-8 contents are frozen into the resource
generation. Unrelated ancestors are outside the workspace trust boundary.
Discovery never runs during ordinary request assembly.

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
addition/removal/rename/metadata, and extension/Tool configuration have no
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
successful primary model-turn start. It is a launch-scoped Native Agent
Extension (Issue #256): whether a runtime composes it at all is decided once,
at composition, from the `extensions` document, and a resource reload can
neither install nor remove it. With the extension absent, the runtime is
ordinary in every other respect and emits no status at all. One finite opportunity set may contain
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
## Candidate access is not resource rediscovery

WF-03 binds the run to a native-retained isolated candidate. Agent profile workspace policy must exactly match the run policy before acquisition. Instructions, Skills, model/profile configuration, capability allowlists, Python ToolVersion identities, MCP definitions, approval and deadlines continue to come from the invoking immutable generation. Candidate cwd never triggers ancestor instruction walks or project capability discovery. Current MCP bindings and nested orchestration are rejected for candidate consumers instead of being silently reconfigured. Noncandidate Workflows acquire no Git resource. See [WF-03 ownership and freeze rules](workflow-programs.md#run-scoped-candidate-workspace-wf-03).

CandidateScope also owns live currentness: internal Workflow committed values carry the exact checked candidate through constructions and Parallel exports. Stale values fail at Branch, Return and execution-input consumption; history never reacquires authority.

After physical users settle, hashing/watch/Git uncertainty is
PhysicalSettlement. Absorbing scope settlement preserves the native resource and
releases process-local active registration, enabling the supported exact native
re-proof/disposal path only within its durable authority: without a trusted
terminal HEAD, checkout and branch HEAD must still equal the immutable
acquisition base. Recovery also requires current source identity to match the
durable `WorkflowWorkspaceSettled.recovery_guard.reference`. That guard is a
`CandidateRecoveryGuard` copied from the native owner's last proven candidate;
it is separate from `candidate`, which describes exact terminal state and is
absent when final inspection is unresolved. Repairing inspection conditions
cannot authorize deleting source that differs from the guard. An advanced-HEAD
unresolved candidate remains retained for explicit user/manual recovery;
readability cannot fabricate terminal commit authority. NestedContainment means
physical descendants remain unproven: active ownership remains, and Git-only
disposal is forbidden even after reopening. Durable disposal intent and exact
native identity allow continuation after physical removal despite failed
settlement append; a still-present checkout must pass candidate-content
verification before removal. No resource recovery restores Workflow execution.

Candidate Agent structured output carries process-local applicability from the
post-node `WorkspaceAccess::finish(false)` reference, through native physical
settlement and the registry's unique terminal result into Workflow's
`CommittedValue`. No mutation binds A to A; a writer binds its output to
produced B. Failed inspection or unresolved containment grants no successful
local candidate-bound output. Machine-review A cannot authorize a later B, and
Event Journal output/correlation remains historical rather than execution
authority. Agent applicability remains process-local. SQLite development schema
28 adds the separate durable resource recovery guard; Child IPC 18 and Runtime
Client 20 are unchanged. Old stores are rejected without migration.

A recovery guard grants no execution or access authority and is not
model-visible. A failed writer inspection retains prior guard A, preserving any
unproven B. A successful writer proof of B followed by final-run inspection
failure retains guard B; unchanged B may recover, while later C must survive.
The native disposer rehashes source immediately before first removal. Durable
intent permits exact continuation after removal without hashing a checkout that
no longer exists.
