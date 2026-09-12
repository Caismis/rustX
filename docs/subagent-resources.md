# Canonical named Agent resources

Goal belongs only to the root conversation in v1. The shared closed extension
syntax accepts Goal in role definitions and invocation selections, but the
effective composition is rejected by `unsupported_child_scope` during resolution,
before staging/spawn/ownership commit. It is never silently dropped or inherited.
Workflow overrides use the same scope check. [Goal extension](goal-extension.md)
does not change one-shot Subagent terminal semantics.

## Session ownership and local lifecycle exclusion (Issue #254)

Session deletion cascades along durable ownership, never provenance. `/tree`
nodes belong to the same Session; `/fork` and `/clone` materialize independent
Sessions. Catalog membership and native typed child ownership commits establish
the finite target. Retained worktrees and branches are blockers requiring
explicit disposal, not implicit cleanup targets. Shared environments, capability
resources, caches, config, credentials and project files remain outside it.

Canonical `ProductRoot` identity, `ProductController` admission and target
Conversation lifecycle access are separate. Preflight freezes ownership
transitions, derives native ownership, then locks only target Conversation
allocations exclusively in sorted identity order. A live unrelated Session and
its Runtime Client remain usable; actual target runtime/child/inspection/private
writer access blocks exclusivity. Ordinary activity does not hold the ownership
freeze. Guards release through drop or OS process death; aliases share identity.
The semantic revision hashes only target membership, owned allocations and
final workspace-blocker state, never raw catalog bytes or execution history.
Management reads never create missing stores or directories. See
[the ownership and storage contract](session-deletion-ownership.md) for the exact
lock order, acquisition/release points, participant lifetimes and regression map.


Named Agents are strict TOML resources discovered by filename. Resources define
themselves by existing in their canonical resource location. Settings and Agent
Profiles express selection, not existence.

```toml
# rustx.toml: selection only
[subagents]
max_concurrent = 4
main = []
workflow = ["reviewer"]
```

The identity `reviewer` comes from `.agents/agents/reviewer.toml` in the trusted
workspace or `agents/reviewer.toml` in the host configuration directory. Names
use the existing 1–64 byte bounded ASCII identity rules. There is no `name` or
`id` field, Markdown/frontmatter reader, registration array, or migration reader.

```toml
# .agents/agents/reviewer.toml
description = "Review one bounded request"
instructions = "Review the supplied proposal and return a concise result."
skills = ["review-guidance"]

[tools]
builtin = ["read"]

[agents_md]
inherit = false
files = [".agents/agents/reviewer/AGENTS.md"]
```

Files are limited to 1 MiB and strict typed TOML; unknown fields, duplicate keys,
invalid types and invalid native bounds reject the complete candidate. Primary
instructions are explicit TOML data, subject to the native 64 KiB bound. The
canonical type is `AgentDocument`, with [agent.schema.json](../schemas/agent.schema.json).
The temporary document preserves named-Agent execution semantics; unified main
and child Agent profiles are a subsequent architecture step.

| Field | Type and meaning |
| --- | --- |
| `description` | Required nonempty string, at most 512 bytes; routing text only. |
| `model` | Optional `provider/model` reference. Omission inherits the invoking attempt's frozen effective model, including reasoning and request contracts. Explicit references use the native model catalog. |
| `timeout_ms` | Optional integer, 1–86,400,000; the whole-child lifecycle deadline. |
| `tools.builtin` | Exact array of **ordinary** native Tool names; default empty. A Tool provided by an Agent Extension (`todo`) is rejected here by name: compose it under `extensions` instead. |
| `tools.sources` | Map of typed source identities to `"all"` or exact Tool-name arrays; default empty. MCP and `python:<package>` use this same selection vocabulary. |
| `skills` | Exact Skill-name array; default empty. |
| `agents_md.inherit` | Boolean, default true; include the parent's frozen project guidance. |
| `agents_md.files` | Ordered supplemental guidance paths; default empty, at most eight. They are distinct project instructions, never the primary role body. |
| `worktree.enabled` | Boolean, default false. |
| `worktree.requireCleanParent` | Boolean, default true; applies when Git isolation is enabled. |
| `extensions.agentStatus` | This role's own closed [Native Agent Extension](launch-configuration.md#native-agent-extensions) composition: `enabled` (default true), `time.enabled`/`time.timezone`, `background.enabled`. Omission means this role's built-in defaults, never the invoking root Agent's configuration. |
| `extensions.todo` | `enabled` (default true). Composes the child conversation's own task list, its model-facing `todo` Tool, and its bounded status presentation — or none of them. |

Role extension composition is **independently authored**: root Agent extensions
and named-Subagent extensions are separate compositions, and a child never
implicitly inherits the root's set. A role that omits `extensions` composes the
built-in defaults, not whatever the invoking runtime happens to run with.

```toml
description = "Implement a bounded change."
instructions = "Implement the delegated task."

[tools]
builtin = ["read", "write", "edit", "bash"]

[extensions.todo]
enabled = true
```

A Todo-enabled child owns **its own** list, over its own conversation and its
own Ledger. It is never the parent's list, never merges into it, and two
concurrently running Todo-enabled children never observe or mutate each other's.
The child's final result remains the existing bounded Subagent report or
Workflow structured output; its list is working state, and its internals do not
enter parent canonical history.

The invoking Agent's own frozen composition does reach the resolver, but only
as **delegation authority** for an explicit invocation override — never as an
inheritance source. A child composes an extension for exactly two reasons: its
definition authored it, or an entitled caller asked for it. See
[Delegation authority](#delegation-authority).

A named role is the **default** child execution profile, not the final one. One
invocation may replace `tools`, `skills`, and `extensions` for exactly that
child; see [Invocation-scoped overrides](#invocation-scoped-overrides). Every
other field — model, instructions, `timeout_ms`, `agents_md`, `worktree` — belongs
to the definition alone and has no per-call form.

No role field overrides host-owned approval policy, external-source enablement,
credentials, tool execution policies, model capabilities, or Workflow ownership.
Names such as “reviewer,” prose such as “read-only,” and source provenance grant
no authority.

## Invocation-scoped overrides

One `subagent` call, or one Workflow `Agent` node, may carry an optional
`override` object. It has exactly three dimensions and no others:

```json
{
  "agent": "reviewer",
  "task": "Review the implementation",
  "override": {
    "tools": {"builtin": ["read", "grep", "bash"]},
    "skills": ["rust-review"],
    "extensions": {"agentStatus": {"enabled": true}}
  }
}
```

### Replacement semantics

One rule covers all three dimensions:

> A **missing** dimension uses the definition's value. A **present** dimension
> **replaces** that dimension completely.

| Written | Effective result |
| --- | --- |
| no `override` | the definition's `tools`, `skills`, and `extensions`, exactly |
| `"override": {}` | identical to omitting it |
| `"tools": {"builtin": ["bash"]}` | exactly Bash — **not** the role's tools plus Bash |
| `"tools": {}` | no ordinary selected tools |
| `"skills": []` | no selected Skills |
| `"extensions": {}` | no composed native extension |

Replacement is per dimension and independent: an override naming only `skills`
leaves tools and extensions exactly as the role authored them. There is no
additive or subtractive mode, no `addTools`/`removeTools`, no wildcard, and no
recursive merge; a present `tools` or `extensions` object is never merged with
the role's corresponding object.

`"extensions": {}` deserves its own line because the *authoring* document
defaults differ deliberately. A role that omits `extensions` composes the
built-in defaults, which include Agent Status. An override that writes
`"extensions": {}` composes **nothing**: presence, not emptiness, is what
selects an extension in an override, so every closed extension member is
explicitly named or absent. Absent means "not composed", never "use a default".

An explicit `null` is not an accepted spelling for any dimension, or for
`override` itself. Unknown fields, non-goal dimensions, and unknown extension
names are rejected by the same strict boundary that rejects them in role
TOML. Duplicate and out-of-order selectors are canonically normalized
exactly as a definition normalizes them, so an override restating the defaults
is the same effective profile as omitting the override.

### Delegation authority

An override says what the caller *asked for*. Whether the caller may ask is a
separate, typed, native decision, and the two launch sites differ:

```text
main `subagent` Tool   dynamic delegation
                       allowed[d] = authorized role baseline[d]
                                    UNION
                                    invoking Agent's frozen admitted profile[d]

Workflow Agent node    trusted static program data
                       allowed[d] = whatever the Workflow's admitted
                                    runtime generation authorizes
```

The union is an authorization **ceiling**, not a merge: it decides what may be
requested, never what the child ends up selecting.

For the main model:

- a role default stays delegable even when the invoking model does not expose
  that capability at all — a named role is an independent projection of the
  generation, not a subset of the parent's toolbar;
- the parent's contribution is its **frozen admitted execution profile**: the
  exact model-facing tool registry of the invoking attempt, the Skills that
  attempt can actually see, and the extension composition its runtime is
  executing against. It is not the generation's whole available catalog, not a
  live mutable registry, not the next generation, and not current configuration;
- a capability known only to the generation, held only by some other role, or
  merely compiled into the executable is not delegable;
- authority is compared by exact native identity — `ToolId`, and
  `SkillId` + `SkillVersionId` — never by display name or role prose. A Builtin
  `search` can never stand in for an MCP server's `search`, and a rewritten Skill
  package is a different identity from the one a caller froze;
- tools, Skills, and extensions are separate authorization domains. Holding one
  never implies holding another, and a dimension the caller did not override is
  never judged against the parent's registry at all;
- extensions are authorized *by configuration*, not by name, and the union is
  taken at that granularity. A composition is not a set of identities, so
  "the role authorizes the whole composition **or** the invoking Agent does"
  would be strictly narrower than a union: it refuses a request whose
  contributors are each legitimately held, only because no single source holds
  all of them at once. Each behavior-affecting contributor is authorized
  independently instead:

  ```text
  Agent Status composed at all  some source composes Agent Status
  time.enabled = true           some source composes Agent Status with Time
                                enabled AND the same EFFECTIVE timezone
  background.enabled = true     some source composes Agent Status with
                                Background enabled
  Todo composed at all          some source composes Todo
  a contributor set to false    narrowing; needs no authority at all
  extensions omitted entirely   narrowing; needs no authority at all
  ```

  Todo has no contributor axis, so it needs no per-contributor union: the
  extension is either composed or not. Note what is *not* being authorized —
  access to anyone's task list. A child composes its own, so the question is
  only whether this caller may ask for the capability at all. Todo gets no
  widening rule of its own: a main-model override remains bounded by
  role ∪ invoking Agent, and a Workflow override by the admitted generation.

  So a role holding UTC Time with Background off, and an invoking Agent holding
  Background with Time off, together authorize a child with both on — Time from
  the role, Background from the invoking Agent, nothing manufactured.

  Timezone authority is decided on the zone the child would **render**, never
  on whether `time.timezone` was written. An omitted zone renders UTC, so it is
  a request for UTC and needs an authority that itself renders UTC; a role
  rendering `Asia/Shanghai` does not cover it. Treating absence as
  "unspecified" would have made it a wildcard that manufactured UTC out of any
  timezone authority at all.

A Workflow Agent node's override is compiled program data. It is validated at
compilation and again during resource-generation preparation, and it is not
reachable from model output, node input values, task text, or any expression or
interpolation language. It may therefore legitimately exceed both the role's
defaults and the invoking main model's capabilities — never the admitted
generation.

Authorization is not source availability. Unknown, unavailable, inert, and
unauthorized stay four distinct outcomes, and a requested capability is never
silently dropped.

### What an override never reaches

Model, instructions/body, timeout, workspace/worktree policy, `AGENTS.md`
policy, approval policy, credentials, source enablement, and arbitrary external
configuration have no per-call form. There are no continuable children, no
generic inheritance, no additive or removal syntax, no wildcards, and no plugin
SDK.

Selecting a Skill grants no Tool: Skill visibility and dependency rules are
unchanged, so a Skill selected under an empty tool set stays a Skill selection
and never implicitly adds Read. Conversely, an extension-provided model Tool is
governed by effective extension composition and is never removed by the ordinary
`tools` allowlist.

Extension authorization and child-scope support are independent checks. An
extension a caller is fully entitled to compose still fails deterministically,
before the child is staged, when one-shot child execution cannot own it.

## Roots, replacement, and admission

Discovery enumerates the user `agents/*.toml` and trusted project
`.agents/agents/*.toml` roots, validates identities, sorts them, and resolves
collisions before parsing. Project Agent overrides user Agent as one whole
resource, with no field-level merging. Each directory scan has a 1024-entry
bound and the resulting catalog obeys the native Agent count bound. Incidental
non-TOML files do not define Agents; malformed canonical TOML rejects the candidate.

`subagents.main` and `subagents.workflow` remain independent selection lists.
They resolve against the discovered catalog. Neither selection bypasses source
authority, capability validation or invocation approval.

Project role and supplemental paths must remain inside the admitted canonical
workspace. Project supplemental relative paths resolve from that workspace,
regardless of where `--config` points. User supplemental paths resolve from the
known user `agents` root and remain inside it. Symlink targets and traversal
are checked against the owning boundary. Missing resources fail the candidate.
These checks do not claim syscall isolation against an actively hostile OS user.
An untrusted project activates no project roles, Skills, Workflows or instructions.

Local launch's automatic Skill roots are the known user configuration directory's
`skills` and the workspace's `.agents/skills`. The standalone Skill discovery
default uses user and workspace `.agents/skills`; neither reads `.rustx/skills`.
Explicit Skill paths retain the existing user/project/CLI ownership validation.
No user files are deleted or migrated.

Implicit project guidance reads at most one file at the admitted workspace root,
using existing precedence: `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`,
`CLAUDE.md`, `CLAUDE.MD`. It never traverses above that boundary or descends into
child worktrees. Global ancestor guidance is not a source. Supplemental role
guidance is explicit and ordered after inherited guidance.

## Checking, reload, and child ownership

`rustx config check` uses the same role loader as runtime preparation. It checks
file bounds, trust, TOML, discovery/admission references, statically
known model, Skill and Tool/source contracts, and every statically knowable
reference in a Workflow Agent node's invocation override. It performs no model, Tool, Python,
MCP, package-preparation or network work, creates no Session/runtime state, and
writes no trust. Unknown online Tool identities remain deferred to their source
owner rather than being invented by static analysis.

`rustx config show --sources` reports prospective `roles` keyed by identity:
`identity`, `selected` path, `layer` (`user` or `project`), and optional
`overridden` lower-precedence path. It does not expose role bodies or credentials
and does not claim to describe an existing running Session. Untrusted sources
remain excluded and the normal trust diagnostic explains the exclusion. Invalid
role resources report their source file and authoring field path.

Parsing finishes before native catalog construction returns. Discovery and
independent admissions validate before capability/model/Skill validation of the
same off-side candidate. `LocalRuntimeResourceLoader::prepare` builds that complete
candidate. `ConversationRuntime::reload_resources` commits capabilities and swaps
`state.resources` under the existing coordinator lock, then emits one coherent
resource observation. Failed or cancelled preparation leaves the previous snapshot
authoritative. There is no additional registry, executor, epoch, or publisher.

`SubagentResolver::resolve` freezes `ResolvedSubagentSpec` from the invoking
generation during native preflight, before process staging and durable ownership
commit. It contains instructions, complete model authority, exact Tool policy,
Skill identities, project guidance, workspace policy, deadline, and the
effective native Agent Extension composition. `ResolvedSubagentSpec`, not the
role document and not the raw override payload, is the complete immutable child
execution contract. A specification frozen from R1 retains R1 after R2
publishes; a later resolution receives R2.
Normal reload still obeys the existing runtime quiescence requirements.

The resolution order is the contract:

```text
1. admission      is this role callable in this domain at all
2. structural     is the override a well-formed child selection
3. replacement    effective[d] = override[d] if present else definition[d]
4. dependency     do the EFFECTIVE selections resolve in this generation
5. authorization  may this caller delegate the effective selections
6. child scope    can a one-shot child own the effective extensions
7. freeze         model, instructions, guidance, materialization, identity
```

Step 3 preceding step 4 is what makes a replaced-away default stop being a
requirement: a role whose default MCP tool is offline still starts when the
invocation replaced that dimension, because the offline selector is no longer
part of the effective invocation. The role's separate catalog-admission
validation is unaffected and still rejects a statically invalid definition.

Every failure is decided before a child process is staged, before any
override-specific execution resource is acquired, and long before durable
ownership commits. Cleanup after a spawn is not an authorization boundary.
Cancellation semantics are unchanged at resolution, preparation, and commit: a
pre-commit cancellation publishes no owned child and leaves no partially
materialized resources.

The invoking root Agent's own `extensions` composition participates only as one
half of the explicit delegation ceiling. It is never implicit child
inheritance: a child composes an extension because a definition authored it or
because a caller asked for it and was entitled to.

### Effective execution-profile identity

The two digests are **separate identities** and neither is derived from the
other:

```text
SubagentDefinitionDigest              identity of the SOURCE named definition
                                      (its routing description, its DEFAULT
                                      tool/Skill/extension selections, its
                                      authored spellings)

ResolvedSubagentSpec::profile_digest  identity of the FINAL FROZEN effective
                                      child execution contract, after
                                      replacement
```

`SubagentDefinitionDigest` continues to identify the source definition, and no
longer uniquely identifies one child once overrides exist. The profile digest
obeys one rule:

> The digest identifies the semantic final frozen execution profile, not its
> authoring history, and no behavior-affecting frozen field may be omitted.

So the source definition digest is deliberately **not** part of the profile
preimage. A routing description never executes; a default Tool, Skill, or
extension selection that an invocation replaced completely stops existing
before the child is frozen. Both would otherwise split one effective profile
into two identities. Every behavior-affecting field the definition digest
summarizes is framed directly instead, as its final frozen value.

The identity is derived from the frozen contract rather than stored beside it,
so it is frozen exactly as strongly as the contract and cannot disagree with
the specification it labels; the child recomputes the same value from the same
frozen bytes. The invoking attempt also commits it durably with child
ownership, so it survives a restart unchanged (see
[Durable execution identity](#durable-execution-identity)).

The versioned canonical framing (`rustx-subagent-profile-v3`) covers:

```text
agent name, instructions, workspace policy, execution deadline
frozen model decision   model reference, protocol, context window,
                        model and effective output budgets, reasoning profile
                        and semantics, effective and declared capabilities,
                        compat, effective request parameters
frozen summary policy   "follows the session primary", or an explicit
                        invocation framed by the SAME helper as the primary,
                        so a summary model is identified exactly as
                        completely as the primary one
project instruction chain   path AND content, in order
effective tools         origin, exact ToolId, model-facing name, and the
                        COMPLETE frozen ToolDefinition the child executes:
                        description, canonical input schema, and the
                        execution, concurrency, approval and replay policies.
                        An external source tool additionally frames its frozen
                        cross-process identity
effective Skills        exact SkillId + SkillVersionId, and the
                        model-visible name AND description, both of which
                        cross to the child verbatim
materialization plane   exactly the external source identities required. The
                        bindings behind them are physical (transport, resource
                        root) or secret (credentials); the one behavior they
                        carry — the invocation policy a server imposes on its
                        tools — is already framed exactly, through each external source
                        tool's cross-process identity above
effective extensions    the closed composition's EFFECTIVE framing: an
                        omitted timezone frames as the UTC it renders, and a
                        DISABLED Time contributor's timezone frames as one
                        inactive sentinel because no zone executes
```

Values whose serialization carries authoring shape are framed by their
effective semantics rather than through that serializer: an omitted
`time.timezone` frames as the UTC it renders, and `ModelCompat` frames its five
translation decisions rather than only the ones a catalog spelled out. Two
values that behave identically are one effective profile.

### A stable capability id is not a semantic contract

A `ToolId` identifies a capability; it does not summarize the semantics that
capability was frozen with. The same `tool-read` identity can be frozen with a
different model-facing description, a different input schema, or different
execution, concurrency, approval or replay policies, and the child executes
**the frozen definition** rather than one it looks up by id. So a Builtin tool
is framed by its complete `ToolDefinition`, field by field, and every field is
included:

```text
id, name, origin           the capability and the name the model calls
description, input_schema  the model-facing contract
execution_policy           attempt-owned / conversation-owned / model-selected
concurrency_policy         in-batch sequential barrier vs parallel group
approval_policy            whether an eligible invocation stops for a human
replay_policy              whether re-execution after an unknown outcome is
                           permitted. Included deliberately: it is a frozen
                           declaration today, it crosses the Runtime Client
                           boundary into the child's observable projection, and
                           its consuming recovery policy is a later milestone —
                           so it cannot be shown irrelevant
```

`input_schema` is framed through the rustX-owned canonical JSON writer that the
cross-process MCP Tool identity already uses — object keys sorted recursively,
array order preserved because it is semantic in JSON Schema, and rustX-owned
number formatting and escaping — so no `serde_json` map implementation or
feature flag can move a digest, and two schemas that differ only in object key
insertion order are one profile.

A Skill's `catalog_entry.description` is framed for the same reason. The
child does not re-derive that metadata from the materialized package: it takes
the parent's frozen strings verbatim and remaps only `location`. So the
description reaches the child's model exactly as frozen and drives progressive
disclosure — deciding whether the model opens the Skill at all — which
`version_id` does not capture on its own. The Skill's `source_root` and its
`files` list stay out: the first is a materialization source rather than an
identity, and the second is represented exactly by `version_id`, which hashes
every package-relative path and its bytes.

An external source tool frames the same complete definition **and** its frozen
`SourceToolIdentity`. The identity is not redundant framing: it is an
independently frozen field that gates the child's startup, since the child
recomputes it from its own `tools/list` and refuses to run on a mismatch. The
profile digest frames that frozen value; it never performs the verification
itself, which remains the child's cross-process materialization check.

### A disabled contributor's configuration does not execute

Once `time.enabled` is `false` the Time contributor never runs, so no timezone
executes and none may distinguish the profile:

```text
time enabled  = true    the effective zone is framed; omitted == explicit UTC
time enabled  = false   every zone spelling, including omission, frames as one
                        inactive sentinel
```

A child frozen with Time off emits no Time contribution whatever its `timezone`
says. That a later configuration edit could re-enable Time is irrelevant: the
composition this digest identifies is frozen, and re-enabling Time changes
`time.enabled`, which is framed. Authorization reads the same rule — a disabled
Time contributor needs no timezone authority, and an enabled one needs
authority for its **effective** zone. The *source-definition* digest keeps
distinguishing an authored disabled zone, because it identifies the source
document rather than the behavior.

The framing deliberately excludes:

```text
source-definition-only  the definition digest, and with it the role's routing
  provenance            description and its replaced-away defaults
desired model config    FrozenModelSpec::configured records what was ASKED
                        for; the resolved invocations are the authority
provider binding        provider identity, endpoints, credential sources, and
                        any admitted credential value
execution identities    SubagentId, conversation/agent ids, tool call id
timestamps              nothing time-derived enters the preimage
physical paths          staging roots, Skill source roots, remapped locations
raw payload formatting  the override's key order, whitespace, duplicates, or
                        whether an override was written at all
```

The provider-binding exclusion is a contract, not an omission: rotating a
credential or repointing an endpoint at the same model leaves the identity
unchanged, on the primary invocation and on an explicit summary alike.

### Durable execution identity

> Once `SubagentOwnershipCommitted` exists, both the source
> `definition_digest` and the effective `profile_digest` are durable execution
> facts and survive restart unchanged.

The ownership commit is the one boundary that writes them, from the frozen
`ResolvedSubagentSpec`. Recovery restores exactly the committed values and
never recomputes either from the current role definition or the current
resource generation: both are mutable, and a reload that redefines the same
agent name must not relabel an already-committed child. Runtime Client projects
both beside each other for a live child and a recovery-projected one alike.

Only the digests are persisted. The effective Tool, Skill, and extension bodies
they summarize are deliberately not durable: the digest is the bounded
correlation fact.

Excluding the raw payload is the point: two invocations that produce the same
effective profile — no override, and an override restating the defaults — share
one identity, and the authorized Tool and Workflow paths agree for equivalent
inputs. Semantically unordered collections are canonically normalized before
framing; the project instruction chain, whose order is meaning, is framed in
order.

The digest is identity and diagnostic/recovery correlation over an
already-authorized contract. It is never an authorization token: no resolution,
admission, or execution path consults it to decide what a child may do. Runtime
Client projects it beside `definition_digest` as a bounded correlation
identity, never alongside the effective selections themselves.

Child composition and `FrozenSubagentResourceLoader` consume this frozen native
specification. Workspace/Git worktree acquisition supplies physical workspace
ownership only. It never restarts launch resolution, reads role roots, walks an
AGENTS.md chain, discovers Skills, or widens MCP/Python/Tool authority. The child
materializes the frozen extension composition exactly as it materializes every
other frozen decision: it never rereads `rustx.toml`, host or project
configuration, or role files to reinterpret which extensions it owns. Skill bodies
retain their established progressive-disclosure semantics.

Schema 8 removes inline role payloads and `instructionsFile`; older runtime
document versions are rejected. There is one authoring path and no compatibility
reader or automatic migration.

### Capability identity and Session ownership

Invocation overrides select the effective child tools, skills, and extensions;
`profile_digest` records that admitted execution profile alongside
`definition_digest`. Neither identity grants filesystem or disposal authority.
Canonical `ProductRoot` alone determines native private allocations. Child IPC
22 carries that product identity, child Conversation identity, and incarnation,
without a second absolute runtime-root authority or a new profile-digest field.
A Workflow child borrowing a Workflow-owned workspace still owns its child
Conversation but adds no independent physical workspace blocker. Session deletion
revisions project resource ownership and exclude capability-profile metadata.

Session deletion freezes child ownership into catalog scope identities before
any cleanup. After durable logical commit, the same ConversationAccess admission
rejects child inspection or restored private allocation even while files remain.
Cleanup removes the exact child allocation and identity-derived root-level routing
socket; retained workspace resources still block preflight and require explicit
disposal. See [Session deletion lifecycle](session-deletion-lifecycle.md).

Source demand, trust granularity, typed resolution and frozen child identity rules
are specified in [ordinary Tool source selection](tool-source-selection.md).
