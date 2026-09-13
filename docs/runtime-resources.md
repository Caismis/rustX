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

Root and named Agents resolve one [Agent Profile](agent-profiles.md) against
admitted resources. Each frozen registry exposes only that Agent's selection.
Root selects `agent.tools`, `agent.extensions`, `agent.agents` and
`agent.workflows`, and subtracts `agent.disabled_skills` from the eligible
Skill catalog it automatically sees. Named defaults use their independently
authored profile — including an exact `skills` list — against generation
authority, never the root registry as a ceiling.

`--tools a,b` is an explicit host profile layer selecting admitted ordinary
names. Unknown or ambiguous CLI names fail. `--no-builtin-tools` and `--no-tools`
restrict model exposure; `--exclude-tools a,b` subtracts last. None activates a
source or changes invocation policy. Profile capabilities unavailable in the
current generation produce typed diagnostics and suppression.

### Ordinary selection is one of two planes

Every filter above addresses the **ordinary capability plane**. A Native Agent
Extension may also contribute a model-facing Tool, and that Tool belongs to
`extensions`, not to any selector here:

```text
  ordinary selected Tool capabilities        every flag in this section
+ enabled extension-provided Tool surfaces   agent.extensions.<name>.enabled
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
extension — in `agent.tools.builtin`, `--tools`, `--exclude-tools`, a named
Subagent's `tools.builtin`, and a Workflow's admitted capability set. It is not
an ordinary available capability at all, so it never appears in the available
catalog those selectors resolve against. See
[Native Agent Extensions](launch-configuration.md#native-agent-extensions).

`--no-tools` conflicts with `--tools`, `--exclude-tools`, and
`--no-builtin-tools`; `--tools` conflicts with `--no-builtin-tools`.
Explicit CLI lists reject empty values/entries, duplicates, unknown or
unavailable identities, and names shared by multiple applicable origins.
There is no registration-order precedence. Use `--no-tools`, not an empty
`--tools` list. The Rust selection boundary validates resolved intent too.
Complete-profile empty dimensions are valid; unavailable profile selections
produce typed diagnostics and suppression. These filters do not grant source activation or alter independently admitted
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

`capabilities::selection` owns source-qualified Tool resolution. Workflow Tool
leaves use `ExactToolSelector`, which admits only one builtin or source Tool.
Agents, including Workflow Agent overrides, use `ToolSelectionDocument` and its
internal `AgentToolSelection` requests. MCP and Managed Python share
`tools.sources` with `SourceToolSelection::All | Exact` capability selection.
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
selectors and uses the shared source identity/availability machinery. Its strict
static dependency contract is separate from complete Agent Profile suppression.
Managed Python retains typed package source identity above its materializer.
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

## Named Agent Profiles

A generation's `AgentCatalog` is configuration/resource-generation state,
never live execution state. Off-side loading reads canonical named TOML
Agent Profile resources and their explicit project-instruction files:

```text
strict AgentProfileDocument
    ↓
AgentProfile
    ↓
generation-scoped resolve_agent_profile
    ↓
ResolvedAgentProfile + canonical typed diagnostics
```

Root and named Agents use this same semantic boundary. Named defaults resolve
against the admitted generation, independently of the root's visible toolbar.
Valid unavailable selections are diagnosed and suppressed: an unavailable
builtin, an undefined/inactive/unprepared/failed ToolSource, an Exact Tool absent
from a ready source, or a missing Skill, named Agent or admitted Workflow leaves
the remaining profile usable. Known scope-ineligible capabilities in a complete
one-shot child profile follow the shared scope-suppression policy.

Malformed authoring still rejects the candidate: unknown fields, wrong types,
malformed typed identities, duplicate exact selections, unknown Extensions,
invalid extension configuration and invalid native bounds are hard errors.
Explicit model configuration/reference validation remains with the model owner
and can also fail; model errors are not generic capability suppression.
Failed preparation preserves the previous complete generation.

Dynamic invocation overrides are a separate strict authorization boundary.
Absent dimensions use named defaults; present dimensions replace completely,
including explicit empty replacements. Model-authored and Workflow-authored
overrides must pass their typed authority inputs and validity checks. An
unauthorized or invalid override is refused, never made successful by suppressing
its request. Workflow static program admission also retains its own validation.

`AgentProfileDocument.timeout_ms` is an optional, definition-level positive
millisecond value. Admission rejects zero, malformed values, and values above
the rustX-owned 86,400,000 millisecond (24 hour) maximum; it never clamps an
invalid value. The validated `SubagentExecutionDeadline` is part of the
definition's semantic digest and is copied into `ResolvedSubagentSpec`, so an
attempt keeps the deadline of the immutable generation it admitted even if a
later reload changes the current configuration. The model-facing `subagent`
call has no deadline field and cannot set or extend it.

The capability-source availability carried alongside the catalog distinguishes
source unavailability from an Exact Tool missing in a ready source. Both produce
typed profile diagnostics and suppression. Selection cannot activate a source,
create a resource or change host invocation/approval policy.

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

`NamedAgentDefinitionDigest` is the identity of the **named definition
itself** — the normalized semantics configuration declares for that agent
(name, description, instructions, complete explicit model policy,
whole-lifecycle execution deadline, Tools, Skills, Extensions, Agent and Workflow
selections, project-instruction and workspace policies). Its current framing is
`rustx-agent-definition-v5`. It is deliberately
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

### Skill sources and discovery

There is one Skill model. Sources decide *where* packages may be discovered;
validation decides *which* packages enter the effective catalog; Agent Profiles
decide *which admitted identities* an Agent selects; and every Agent loads a
Skill's contents lazily, only when it needs them.

```text
configured Skill sources
        |
        v
discover candidate packages per source
        |
        v
validate each candidate independently
        |
        +-- valid   -> source-local candidate
        |
        +-- invalid -> excluded + typed generation diagnostic
        |
        v
same-scope logical-identity conflict elimination
        |
        v
explicit > workspace > global merge
        |
        v
frozen generation SkillCatalog / SkillSnapshot
        |
        +--> root visible set = eligible catalog - agent.disabled_skills
        +--> named Agent explicit Skill selection
        +--> Workflow child explicit/default Skill selection
        |
        v
existing lazy Read-based Skill loading
```

For v1 there are exactly two **automatic** sources:

| Source | Root |
| --- | --- |
| `global` | `~/.agents/skills` |
| `workspace` | `<workspace>/.agents/skills` |

```text
precedence:
explicit --skill > workspace > global
```

The global root is resolved from the launch owner's captured home directory.
It is never a rustX configuration directory, never shell-expanded at a use
site, and `~/.config/rustx/skills` is not a source, an alias, a fallback, or a
migration path.

The session policy selects which automatic roots are scanned:

```toml
[skills]
sources = ["global", "workspace"]
```

`global` and `workspace` are the only accepted identities; there is no `all`,
no include/exclude pair, no wildcard, no regex, and no custom root registry.
An unknown identity, a duplicate entry, and an unknown field are hard
configuration authoring errors. An explicitly empty array selects no automatic
source. **Array order is not precedence** — precedence is the architectural
rule above. The policy is launch-scoped: a reload rescans the roots the launch
resolved, but never installs or removes a source authority under a running
composition.

**A source the launch did not select is completely inert.** It has zero effect
on discovery, validation, startup, reload, diagnostics, and publication. Under
`sources = ["global"]` or `sources = []`, `<workspace>/.agents/skills` is not a
Skill root at all — it is an unrelated directory that happens to share the name,
and an invalid or redirected one cannot fail startup or reload. Skill root
validation therefore belongs to the Skill source owner, which validates exactly
the roots the launch resolved and froze; the generic workspace resource layer
never independently authorizes a Skill root, because it cannot see the source
policy.

`--skill <path>` remains a separate launch authority for an explicit package
directory, a `SKILL.md`, or a collection root. It passes through the same
Agent Skills package validation, the same identity validation, the same
containment rules, the same deterministic conflict handling, and the same
generation freeze; it is not a bypass. Because explicit launch intent is the
highest-precedence layer everywhere else in rustX, an explicit package wins
the same logical identity against both automatic sources. A `--skill` path
that does not exist is a launch error — it is authored intent, not discovered
content. `--no-skills` disables automatic discovery entirely.

Discovery is bounded by its source: an accepted candidate's canonical root must
stay inside its own source's canonical root. Global and workspace are different
authorities, so containment is always package-in-source, never
package-in-workspace.

Resource bounding is per **logical source**, not per root. One source may
aggregate several roots — every `--skill` collection path and every explicitly
named package path feeds the one `explicit` source — and all of them draw from
one cumulative candidate budget:

| Bound | Scope |
| --- | --- |
| `MAX_SKILL_ROOT_ENTRIES` | one collection *directory*: the cost of one `read_dir` |
| `MAX_SOURCE_SKILL_PACKAGES` | one logical *source*, cumulative across every root it aggregates |
| `MAX_EXPLICIT_SKILL_PATHS` | the number of `--skill` paths the launch may name |

The budget is charged against candidates *before* validation, because the work
being bounded is the per-candidate validation itself: an excluded malformed
package still costs a `SKILL.md` parse and a package walk. An automatic source
that overruns its budget is excluded whole, with a `source_budget_exceeded`
fact, and every other source is unaffected; an overrun of the explicit
authority is a launch error, like every other failure of authored launch
intent. Charging the bound per root instead would silently multiply the ceiling
by the number of configured roots.

Every **admitted** Skill location is the canonical absolute host path,
whatever spelling the caller configured: discovery canonicalizes once, at the
filesystem authority boundary, so a published location can never be
re-resolved against a different base and reach a different file. This covers
the effective package, its provenance, and the provenance of any package it
shadowed. A diagnostic about a *configured root* echoes the configured
spelling instead, because an absent or unusable root has no canonical host
identity to report.

**One malformed Skill package never suppresses unrelated valid Skills.** A
candidate that fails validation is excluded and represented by a typed
generation-scoped diagnostic; the rest of its source still publishes. Two
distinct packages resolving to one logical identity *in the same scope* exclude
every conflicting definition and emit one deterministic conflict fact —
discovery never picks a winner by enumeration order. A valid workspace package
shadowing a valid global one is intentional, not an error: the shadow is kept
as generation provenance (effective identity, winning source and location, and
each shadowed source and location) and never reaches the model-facing catalog.

The typed facts frozen with each generation are:

| Diagnostic | Severity | Meaning |
| --- | --- | --- |
| `source_root_missing` | fact | The root does not exist; an empty set, never a failure. |
| `source_root_invalid` | warning | The root exists but cannot be scanned; only that source is excluded. |
| `source_budget_exceeded` | warning | The source offered more candidates than its cumulative budget; only that source is excluded. |
| `package_invalid` | warning | One candidate failed Agent Skills validation; the typed cause is preserved. |
| `package_escapes_source` | warning | One candidate resolved outside its own source root. |
| `duplicate_identity` | warning | One scope defines the identity more than once; every definition is excluded. |
| `shadowed` | fact | A higher-precedence source won the same identity. |

Root `disabled_skills` adds its own `disabled_skill_absent` Agent Profile
diagnostic for an identity the effective catalog does not contain. Diagnostics
are computed once with the generation, canonically ordered, and never
re-emitted per model turn.

A later generation may change discovered and effective Skills for future work.
It never mutates an already admitted attempt, child, or running Workflow: an
attempt admitted against generation R1 keeps R1's frozen Skill identities,
versions, and locations after R2 publishes.

A rediscovery is a publication **no-op only when the complete generation is
unchanged** — the executable Skill semantics *and* every generation-scoped
Skill fact:

```text
publication no-op = same bindings, visible bindings, catalog and locations
                  + same effective provenance
                  + same typed diagnostics
```

The last two lines are why publication equivalence is a distinct, stronger
concept than execution equivalence. Both of these leave the executable catalog
byte-identical and must still publish a new generation:

- a **diagnostics-only** change: a newly added malformed package excludes
  itself, so nothing an execution observes changes while the generation now
  owns an exclusion fact;
- a **provenance-only** change: a lower-precedence source starts offering an
  identity the winner already owned, so the winner, its version binding, and
  its published location are unchanged while the generation now owns shadowing
  provenance.

Collapsing either into a no-op would leave inspection describing a filesystem
state that no longer exists. Neither half is model-visible: both travel beside
the catalog, never inside it.

See [launch configuration](launch-configuration.md) and
[Agent Profiles](agent-profiles.md).

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
